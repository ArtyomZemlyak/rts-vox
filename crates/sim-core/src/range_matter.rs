//! Adapter from the range's occluded pressure envelope to shared, persistent
//! material fragments. Geometry is removed only after reserving energy + room.
use super::{material, RangeUnit};
use crate::{
    burst::{Bursts, Fragment},
    economy::{Economy, Wreck},
    matter::{Container, Kind, Ledger, Sink},
};
use fx::{
    blob::{Blob, Scan},
    Fx,
};
use modding::navigation::Felling;
use phys::V3;
use std::collections::BTreeSet;
use voxel::{store::AIR, World};

pub enum ClaimResult {
    Applied,
    Unaffordable,
    Deferred,
    /// The world's frame (ADR-034): no edit reaches it, so no claim on it
    /// could ever be confirmed or released (ADR-420).
    Indestructible,
}

/// Whether an edit can reach this voxel: inside the world and off its frame
/// (ADR-034). The edit queue drops the rest silently, so a claim on them
/// would wait for a confirmation that never comes (ADR-420).
fn claimable(w: &World, p: [i32; 3]) -> bool {
    p.iter().all(|v| *v >= 0) && {
        let [x, y, z] = p.map(|v| v as u32);
        w.inside(x, y, z) && !w.on_frame(x, y, z)
    }
}

pub struct Matter {
    pub debris: Bursts,
    /// Cells per side of the current weapon's debris pieces (ADR-284).
    fragment_box: u8,
    pub economy: Economy,
    pub tuning: modding::tuning::Blast,
    pub remaining_mj: u64,
    pub supplied_mj: u64,
    pub spent_mj: u64,
    bodies: Vec<u32>,
    section_work: u32,
    section_jobs: u32,
    section_cursor: usize,
    /// Yielding cells still to be released after a cut, each with the cut
    /// cell it hangs from. Ordered, bounded per tick, saved whole.
    felling: BTreeSet<([i32; 3], [i32; 3])>,
}
impl Matter {
    pub fn new(units: &[RangeUnit]) -> Self {
        let t =
            crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("stock tuning");
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/range-materials.ron"))
                .expect("range material IDs");
        let mut economy = Economy::new(modding::Kinds::new(), mats, 1, &t);
        let amount = economy.per_voxel(3);
        let bodies = units
            .iter()
            .map(|u| {
                // Segments are matter of the same body (ADR-278).
                let cells = u.body.solid_count() as u64
                    + u.body
                        .segments
                        .iter()
                        .map(|s| u64::from(s.body.solid_count()))
                        .sum::<u64>();
                let n = cells * amount;
                economy.ledger.create(Kind::Body, n, n)
            })
            .collect();
        Self {
            debris: Bursts::default(),
            fragment_box: 2,
            economy,
            tuning: t.blast,
            remaining_mj: 0,
            supplied_mj: 0,
            spent_mj: 0,
            bodies,
            section_work: t.impact.detach_work_per_tick,
            section_jobs: t.impact.detach_jobs_per_tick,
            section_cursor: 0,
            felling: BTreeSet::new(),
        }
    }
    pub fn felling_pending(&self) -> usize {
        self.felling.len()
    }
    /// What hangs from a cut falls: connected yielding matter no lower than
    /// the cut and within the radius. Below the cut a stump keeps standing.
    fn spread(&mut self, p: [i32; 3], seed: [i32; 3], radius: i32) {
        for a in 0..3 {
            for d in [-1, 1] {
                let mut q = p;
                q[a] += d;
                let far = (0..3).map(|k| (q[k] - seed[k]).pow(2)).sum::<i32>() > radius * radius;
                if !far && q[1] >= seed[1] {
                    self.felling.insert((q, seed));
                }
            }
        }
    }
    /// Cut every listed cell now: the hull stands in them. The fracture work
    /// is the vehicle's, supplied and spent in the same call; the kinetic
    /// share throws the pieces away from the hull. A full pool stops the cut
    /// where it is, and the hull keeps standing in the rest.
    pub fn fell(&mut self, w: &mut World, cells: &[[u32; 3]], at: [i32; 3], f: &Felling) -> usize {
        let mut cut = 0;
        for &p in cells {
            let m = w.material(p[0], p[1], p[2]);
            if m == AIR
                || !f.materials.contains(&m)
                || self.debris.claimed(p)
                || !claimable(w, p.map(|v| v as i32))
            {
                continue;
            }
            let (cost, motion) = self.cost(m);
            if !self
                .debris
                .claim_voxel(w, p, m, at, motion, self.fragment_box, &self.tuning)
            {
                break;
            }
            self.supplied_mj += cost;
            self.spent_mj += cost;
            cut += 1;
            self.spread(p.map(|v| v as i32), p.map(|v| v as i32), f.radius);
        }
        cut
    }
    /// Release the crown hanging from earlier cuts: connected yielding cells
    /// within the radius leave as unpowered debris and fall. Nothing is paid
    /// twice, and a full pool only postpones the rest.
    pub fn step_felling(&mut self, w: &mut World, f: &Felling) {
        let mut left = f.cells_per_tick;
        while left > 0 {
            let Some((p, seed)) = self.felling.pop_first() else {
                break;
            };
            left -= 1;
            if p[0] < 0
                || p[1] < 0
                || p[2] < 0
                || p[0] >= w.nx as i32
                || p[1] >= w.ny as i32
                || p[2] >= w.nz as i32
            {
                continue;
            }
            let q = p.map(|v| v as u32);
            let m = w.material(q[0], q[1], q[2]);
            if m == AIR
                || !f.materials.contains(&m)
                || self.debris.claimed(q)
                || w.on_frame(q[0], q[1], q[2])
            {
                continue;
            }
            // Felled wood crumbles in two-cell pieces whatever the weapon.
            if !self.debris.claim_voxel(w, q, m, seed, 0, 2, &self.tuning) {
                self.felling.insert((p, seed));
                break;
            }
            self.spread(p, seed, f.radius);
        }
    }
    pub(super) fn fragment_size(&mut self, size: u32) {
        self.fragment_box = size.clamp(1, 4) as u8;
    }
    /// Cells per side of the box the current burst's pieces are grouped in.
    pub(super) fn fragment_box(&self) -> u8 {
        self.fragment_box
    }
    pub fn begin(&mut self, joules: u32, fragment_box: u32) {
        self.fragment_box = fragment_box.clamp(1, 4) as u8;
        self.remaining_mj = u64::from(joules) * 1000;
        self.supplied_mj += self.remaining_mj;
    }
    fn cost(&self, mat: u16) -> (u64, u64) {
        let d = self.economy.mats.get(mat).expect("range material");
        crate::burst::fracture_cost_mj(d.strength, voxel::unit::WORLD_MM, &self.tuning)
    }

    fn pay(&mut self, cost: u64) {
        self.remaining_mj -= cost;
        self.spent_mj += cost;
    }
    /// Loose terrain the world let go of (ADR-299, ADR-300) crumbles into
    /// pieces of its own matter. Returns pieces made and cells put back.
    pub fn collapse(
        &mut self,
        w: &mut World,
        cells: &[([u32; 3], voxel::store::Mat)],
    ) -> (usize, usize) {
        self.debris
            .pieces_from_cells(w, cells, &mut self.economy, &self.tuning, self.fragment_box)
    }
    /// Loose terrain that fell on an annihilating range (ADR-285) leaves
    /// no pieces: its matter goes to the annihilation sink at once.
    pub fn annihilate_cells(&mut self, cells: &[([u32; 3], voxel::store::Mat)]) {
        for &(_, mat) in cells {
            let amount = self.economy.per_voxel(mat);
            self.economy.ledger.reveal(self.economy.ground, amount);
            self.economy
                .ledger
                .sink(self.economy.ground, amount, Sink::Annihilation);
        }
    }
    /// A voxel that melted (ADR-290): it runs off as debris of its own
    /// material, its matter moving from the ground to the piece, with no
    /// throw. `false` when the pool has no room; it then stays hot.
    pub fn melt(&mut self, w: &mut World, p: [u32; 3]) -> bool {
        let m = w.material(p[0], p[1], p[2]);
        if m == 0 || self.economy.mats.get(m).is_none() || !claimable(w, p.map(|v| v as i32)) {
            return false;
        }
        let at = p.map(|v| v as i32);
        self.debris.claim_voxel(w, p, m, at, 0, 2, &self.tuning)
    }
    /// Heat has already softened this cell. Transfer it without explosive
    /// fracture work or a launch impulse, like melted terrain.
    pub fn melt_unit(&mut self, id: usize, u: &RangeUnit, part: usize, cell: [usize; 3]) -> bool {
        if self.debris.fragment_room(&self.tuning) == 0 {
            return false;
        }
        let root_frame = u.root_frame(cell[1]);
        let (body, frame) = if part == 0 {
            (&u.body, &root_frame)
        } else {
            (&u.body.segments[part - 1].body, &u.seg_frames[part - 1])
        };
        let [x, y, z] = cell;
        if !body.vol.get(x, y, z) {
            return false;
        }
        let amount = self.economy.per_voxel(3);
        let held = self.economy.ledger.create(Kind::Debris, 0, u64::MAX);
        assert_eq!(
            self.economy.ledger.transfer(self.bodies[id], held, amount),
            amount
        );
        let density = self.economy.mats.get(3).unwrap().density;
        let position = frame.world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
        let speed = u
            .motion
            .drive
            .vel
            .div(Fx::from_int(crate::TICKS_PER_SECOND));
        let velocity = V3::new(
            fx::angle::cos(u.motion.drive.head).mul(speed),
            u.motion.hull.vy.div(Fx::from_int(crate::TICKS_PER_SECOND)),
            fx::angle::sin(u.motion.drive.head).mul(speed),
        );
        self.debris.emit_body_fragment(
            Fragment {
                id: 0,
                position,
                velocity,
                material: 3,
                paint: Some([1, body.finish_at(x, y, z), body.part_at(x, y, z) as u8]),
                mm: body.mm,
                basis: [frame.x, frame.y, frame.z],
                mass_g: u64::from(density)
                    * u64::from(self.tuning.density_kg_m3_per_density)
                    * u64::from(body.mm).pow(3)
                    / 1_000_000,
                count: 1,
                shape: 1,
                side: 2,
                chunk: None,
                held,
                resting: false,
            },
            V3::ZERO,
            0,
            &self.tuning,
        );
        true
    }
    pub fn ground(&mut self, w: &mut World, p: [i32; 3], at: [i32; 3]) -> ClaimResult {
        if !claimable(w, p) {
            return ClaimResult::Indestructible;
        }
        let m = material(w, p);
        let (cost, motion) = self.cost(m);
        if cost > self.remaining_mj {
            return ClaimResult::Unaffordable;
        }
        if !self.debris.claim_voxel(
            w,
            p.map(|v| v as u32),
            m,
            at,
            motion,
            self.fragment_box,
            &self.tuning,
        ) {
            return ClaimResult::Deferred;
        }
        self.pay(cost);
        ClaimResult::Applied
    }
    pub fn body(
        &mut self,
        id: usize,
        u: &RangeUnit,
        cell: [usize; 3],
        pose: &crate::damage::Frame,
        at: [i32; 3],
    ) -> ClaimResult {
        self.claim_cell(id, &u.body, cell, pose, Some(at))
    }
    pub fn body_container(&self, id: usize) -> u32 {
        self.bodies[id]
    }
    /// A segment cell claimed by the wave: the same price and container as
    /// a root cell, thrown from the segment's own frame (ADR-278).
    pub fn body_segment(
        &mut self,
        id: usize,
        u: &RangeUnit,
        seg: usize,
        cell: [usize; 3],
        frame: &crate::damage::Frame,
        at: [i32; 3],
    ) -> ClaimResult {
        self.claim_cell(id, &u.body.segments[seg].body, cell, frame, Some(at))
    }
    /// A cell crushed where two hulls met (ADR-455): it pays the fracture
    /// alone and falls from where it stood instead of being thrown.
    /// `part` 0 is the root, `k` the segment `k - 1`.
    pub fn crush(
        &mut self,
        id: usize,
        u: &RangeUnit,
        part: usize,
        cell: [usize; 3],
        frame: &crate::damage::Frame,
    ) -> ClaimResult {
        let body = if part == 0 {
            &u.body
        } else {
            &u.body.segments[part - 1].body
        };
        self.claim_cell(id, body, cell, frame, None)
    }
    /// What crushing one body cell costs, millijoules: the fracture
    /// without the share a thrown piece carries (ADR-455).
    pub fn crush_price(&self) -> u64 {
        let (cost, motion) = self.cost(3);
        cost - motion
    }
    /// Work beside a burst in flight pays from a budget of its own
    /// (ADR-455): the burst's remaining energy is set aside for the call
    /// and given back after it, so a crash in mid-burst neither spends nor
    /// refills what the burst has left.
    pub fn aside<R>(&mut self, mj: u64, work: impl FnOnce(&mut Self) -> R) -> R {
        let burst = std::mem::replace(&mut self.remaining_mj, mj);
        self.supplied_mj += mj;
        let r = work(self);
        self.remaining_mj = burst;
        r
    }
    /// One body cell leaves as a piece of its own and its matter moves
    /// from the body's container into the piece. Thrown from `from` with
    /// the kinetic share of its price, or, crushed, with no speed and for
    /// the fracture alone.
    fn claim_cell(
        &mut self,
        id: usize,
        body: &voxel::unit::UnitBody,
        cell: [usize; 3],
        frame: &crate::damage::Frame,
        from: Option<[i32; 3]>,
    ) -> ClaimResult {
        let (mut cost, mut motion) = self.cost(3);
        if from.is_none() {
            (cost, motion) = (cost - motion, 0);
        }
        if cost > self.remaining_mj {
            return ClaimResult::Unaffordable;
        }
        if self.debris.fragment_room(&self.tuning) == 0 {
            return ClaimResult::Deferred;
        }
        let position = frame.world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
        let direction = match from {
            Some(at) => {
                let mut direction = position.sub(V3::new(
                    Fx::from_int(at[0]),
                    Fx::from_int(at[1]),
                    Fx::from_int(at[2]),
                ));
                direction.y = direction.y.add(
                    Fx::from_ratio(self.tuning.lift_pct as i32, 100)
                        .mul(direction.x.abs().add(direction.z.abs())),
                );
                if direction.len_sq() == Fx::ZERO {
                    frame.y
                } else {
                    direction
                }
            }
            None => frame.y,
        };
        let amount = self.economy.per_voxel(3);
        let held = self.economy.ledger.create(Kind::Debris, 0, u64::MAX);
        assert_eq!(
            self.economy.ledger.transfer(self.bodies[id], held, amount),
            amount
        );
        let d = self.economy.mats.get(3).unwrap();
        let mass_g = u64::from(d.density)
            * u64::from(self.tuning.density_kg_m3_per_density)
            * u64::from(body.mm).pow(3)
            / 1_000_000;
        let [x, y, z] = cell;
        self.debris.emit_body_fragment(
            Fragment {
                id: 0,
                position,
                velocity: V3::ZERO,
                material: 3,
                paint: Some([1, body.finish_at(x, y, z), body.part_at(x, y, z) as u8]),
                mm: body.mm,
                basis: [frame.x, frame.y, frame.z],
                mass_g,
                count: 1,
                shape: 1,
                side: 2,
                chunk: None,
                held,
                resting: false,
            },
            direction,
            motion,
            &self.tuning,
        );
        self.pay(cost);
        ClaimResult::Applied
    }
    /// Detached segments leave whole, with the body's motion, as chunks that
    /// keep their cells and shape; matter moves with them. A full pool only
    /// postpones the rest (ADR-278).
    pub fn shed_segments(&mut self, units: &mut [RangeUnit]) {
        self.shed_segments_budget(units, self.section_work);
    }
    pub fn shed_segments_budget(&mut self, units: &mut [RangeUnit], mut budget: u32) {
        for (id, u) in units.iter_mut().enumerate() {
            for &seg in &u.seg_shed {
                let i = usize::from(seg);
                if u.seg_sections[i].is_none() && u.body.segments[i].body.solid_count() > 0 {
                    u.seg_sections[i] = Some(crate::damage::SectionJob::new(
                        id as u32,
                        0,
                        u.body.segments[i].body.vol.clone(),
                    ));
                }
            }
            let mut changed = false;
            for seg in 0..u.seg_sections.len() {
                let Some(mut job) = u.seg_sections[seg].take() else {
                    continue;
                };
                loop {
                    let source = &u.body.segments[seg].body;
                    job.advance(source, &mut budget);
                    if !job.ready() || self.debris.fragment_room(&self.tuning) == 0 {
                        break;
                    }
                    let (body, part) = job.commit(source);
                    let count = part.cells.len() as u32;
                    let mm = source.mm;
                    let frame = u.seg_frames[seg];
                    let chunk = crate::burst::chunk::Chunk::new(
                        id as u16, 1, part.cells, [frame; 2], part.shape,
                    )
                    .in_segment(seg as u8);
                    let amount = u64::from(count) * self.economy.per_voxel(3);
                    let held = self.economy.ledger.create(Kind::Debris, 0, u64::MAX);
                    assert_eq!(
                        self.economy.ledger.transfer(self.bodies[id], held, amount),
                        amount
                    );
                    let speed = u
                        .motion
                        .drive
                        .vel
                        .div(Fx::from_int(crate::TICKS_PER_SECOND));
                    let mut velocity = V3::new(
                        fx::angle::cos(u.motion.drive.head).mul(speed),
                        u.motion.hull.vy.div(Fx::from_int(crate::TICKS_PER_SECOND)),
                        fx::angle::sin(u.motion.drive.head).mul(speed),
                    );
                    let vmax = Fx::from_ratio(
                        self.tuning.max_speed_mm_s as i32,
                        voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
                    );
                    if velocity.len_sq() > vmax.mul(vmax) {
                        velocity = velocity.scale(vmax.div(velocity.len_sq().sqrt()));
                    }
                    let density = self.economy.mats.get(3).unwrap().density;
                    for c in &chunk.cells {
                        u.seg_dents.remove(&(seg as u8, *c));
                        u.seg_removed.push((seg as u8, *c));
                    }
                    self.debris.emit_section(Fragment {
                        id: 0,
                        position: chunk.centre,
                        velocity,
                        material: 3,
                        paint: Some([1, 0, 0]),
                        mm,
                        basis: [frame.x, frame.y, frame.z],
                        mass_g: u64::from(density)
                            * u64::from(self.tuning.density_kg_m3_per_density)
                            * u64::from(mm).pow(3)
                            / 1_000_000
                            * u64::from(count),
                        count,
                        shape: 1,
                        side: 2,
                        chunk: Some(chunk),
                        held,
                        resting: false,
                    });
                    u.body.segments[seg].body = std::sync::Arc::unwrap_or_clone(body);
                    u.seg_detached += count as usize;
                    u.revision += u64::from(count);
                    changed = true;
                    if budget == 0 {
                        break;
                    }
                }
                if !job.finished() {
                    u.seg_sections[seg] = Some(job);
                }
                if u.body.segments[seg].body.solid_count() == 0 {
                    u.seg_shed.retain(|s| usize::from(*s) != seg);
                }
            }
            if changed {
                u.refresh_damage(id);
            }
        }
    }
    pub fn annihilate_body(&mut self, id: usize, count: usize) {
        let amount = count as u64 * self.economy.per_voxel(3);
        assert_eq!(
            self.economy
                .ledger
                .sink(self.bodies[id], amount, Sink::Annihilation),
            amount
        );
    }
    /// Same extractor and physical chunks as the game World. Disconnection
    /// was already paid for at the cut; the remaining section is transferred,
    /// never charged or annihilated a second time. A body shattered into many
    /// pieces sheds as many of them per tick as work and room allow, so the
    /// wave never waits a tick per piece.
    pub fn step_sections(&mut self, units: &mut [RangeUnit]) -> u32 {
        let n = units.len();
        if n == 0 {
            return self.section_work;
        }
        let mut left = self.section_work;
        let mut jobs = units
            .iter()
            .filter(|u| u.section.is_some())
            .count()
            .min(self.section_jobs as usize);
        for _ in 0..n {
            if jobs == 0 {
                break;
            }
            let id = self.section_cursor % n;
            self.section_cursor = (id + 1) % n;
            let u = &mut units[id];
            let Some(mut job) = u.section.take() else {
                continue;
            };
            let mut budget = left.div_ceil(jobs as u32);
            let start = budget;
            jobs -= 1;
            let mut committed = false;
            loop {
                job.advance(&u.body, &mut budget);
                if !job.ready() || self.debris.fragment_room(&self.tuning) == 0 {
                    break;
                }
                committed = true;
                let (body, part) = job.commit(&u.body);
                let count = part.cells.len() as u32;
                let amount = u64::from(count) * self.economy.per_voxel(3);
                let held = self.economy.ledger.create(Kind::Debris, 0, u64::MAX);
                assert_eq!(
                    self.economy.ledger.transfer(self.bodies[id], held, amount),
                    amount
                );
                let chunk = crate::burst::chunk::Chunk::new(
                    id as u16,
                    1,
                    part.cells,
                    [u.pose; 2],
                    part.shape,
                );
                for c in &chunk.cells {
                    u.dents.remove(c);
                }
                u.removed.extend_from_slice(&chunk.cells);
                u.detached += count as usize;
                u.revision += u64::from(count);
                u.status.detached -= count;
                u.body = std::sync::Arc::unwrap_or_clone(body);
                let speed = u
                    .motion
                    .drive
                    .vel
                    .div(Fx::from_int(crate::TICKS_PER_SECOND));
                let mut velocity = V3::new(
                    fx::angle::cos(u.motion.drive.head).mul(speed),
                    u.motion.hull.vy.div(Fx::from_int(crate::TICKS_PER_SECOND)),
                    fx::angle::sin(u.motion.drive.head).mul(speed),
                );
                let vmax = Fx::from_ratio(
                    self.tuning.max_speed_mm_s as i32,
                    voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
                );
                if velocity.len_sq() > vmax.mul(vmax) {
                    velocity = velocity.scale(vmax.div(velocity.len_sq().sqrt()));
                }
                let density = self.economy.mats.get(3).unwrap().density;
                self.debris.emit_section(Fragment {
                    id: 0,
                    position: chunk.centre,
                    velocity,
                    material: 3,
                    paint: Some([1, 0, 0]),
                    mm: u.body.mm,
                    basis: [
                        V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
                        V3::new(Fx::ZERO, Fx::ONE, Fx::ZERO),
                        V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
                    ],
                    mass_g: u64::from(density)
                        * u64::from(self.tuning.density_kg_m3_per_density)
                        * u64::from(u.body.mm).pow(3)
                        / 1_000_000
                        * u64::from(count),
                    count,
                    shape: 1,
                    side: 2,
                    chunk: Some(chunk),
                    held,
                    resting: false,
                });
                if budget == 0 {
                    break;
                }
            }
            left -= start - budget;
            let finished = job.finished();
            if !finished {
                u.section = Some(job);
            }
            // A job that ends without a commit still hands on: what was
            // cut loose outside its domain while it ran needs a job of
            // its own, or it hangs detached for good (ADR-444).
            if committed || finished {
                u.refresh_damage(id);
            }
        }
        left
    }
    pub fn flush(
        &mut self,
        w: &World,
        material: bool,
        gravity: i32,
        tick: u64,
        hulls: &[crate::burst::Hull],
        yielding: &[voxel::store::Mat],
    ) {
        self.debris
            .confirm(w.completed(), &mut self.economy, &self.tuning);
        for &(by, mat) in w.taken() {
            // A voxel consumed by fire went to the combustion sink whatever
            // the weapon; the rest of an annihilating weapon's take is its.
            // A cell the collapse layer dug is nobody's take (ADR-300): it
            // falls as a body, crumbles into pieces, or was sunk already.
            let sink = if by == crate::heat::FIRE {
                Sink::Combustion
            } else if by == voxel::world::NOBODY {
                continue;
            } else if !material {
                Sink::Annihilation
            } else {
                continue;
            };
            let amount = self.economy.per_voxel(mat);
            self.economy.ledger.reveal(self.economy.ground, amount);
            self.economy.ledger.sink(self.economy.ground, amount, sink);
        }
        self.debris.step_fragments(
            w,
            &mut self.economy,
            &self.tuning,
            Fx::from_ratio(gravity, crate::TICKS_PER_SECOND.pow(2)),
            tick,
            crate::burst::Surroundings { hulls, yielding },
        );
    }
    pub fn save(&self, b: &mut Blob) {
        b.u8(self.fragment_box);
        b.u64(self.remaining_mj);
        b.u64(self.supplied_mj);
        b.u64(self.spent_mj);
        b.usize(self.section_cursor);
        b.u32(self.felling.len() as u32);
        for (p, seed) in &self.felling {
            for v in p.iter().chain(seed) {
                b.i32(*v);
            }
        }
        self.debris.save(b);
        let (containers, sunk, initial, head, free) = self.economy.ledger.snapshot();
        b.usize(containers.len());
        for c in containers {
            b.u8(c.kind.code());
            b.u64(c.held);
            b.u64(c.cap);
            b.bool(c.alive);
        }
        for v in sunk {
            b.u64(v);
        }
        b.u64(initial);
        b.u32(head);
        b.u32s(&free);
    }
    pub fn load(&mut self, s: &mut Scan, w: &World, units: &[RangeUnit]) -> Option<()> {
        self.fragment_box = s.u8();
        if !(1..=4).contains(&self.fragment_box) {
            return None;
        }
        self.remaining_mj = s.u64();
        self.supplied_mj = s.u64();
        self.spent_mj = s.u64();
        self.section_cursor = s.usize();
        if self.section_cursor >= units.len().max(1) {
            return None;
        }
        if self.spent_mj > self.supplied_mj || self.remaining_mj > self.supplied_mj - self.spent_mj
        {
            return None;
        }
        let n = s.u32() as usize;
        if n > 1 << 16 {
            return None;
        }
        self.felling.clear();
        for _ in 0..n {
            let p: [i32; 3] = std::array::from_fn(|_| s.i32());
            let seed: [i32; 3] = std::array::from_fn(|_| s.i32());
            let dims = [w.nx, w.ny, w.nz];
            if (0..3).any(|a| {
                p[a] < -1 || p[a] > dims[a] as i32 || seed[a] < 0 || seed[a] >= dims[a] as i32
            }) || !self.felling.insert((p, seed))
            {
                return None;
            }
        }
        let templates = crate::damage::Bodies::new(
            units
                .iter()
                .map(|u| match u.kind {
                    modding::range::RangeUnitKind::Tank => voxel::unit::tank(),
                    modding::range::RangeUnitKind::Strider => voxel::unit::strider(),
                })
                .collect(),
        );
        self.debris = Bursts::load_material(s, Some(w), &self.tuning, Some(&templates), 0, false)?;
        let n = s.usize();
        if n > self.tuning.max_fragments as usize + units.len() * 2 + 2 {
            return None;
        }
        let mut containers = Vec::with_capacity(n);
        for _ in 0..n {
            let c = Container {
                kind: Kind::from_code(s.u8())?,
                held: s.u64(),
                cap: s.u64(),
                alive: s.bool(),
            };
            if c.held > c.cap {
                return None;
            }
            containers.push(c);
        }
        let sunk = (0..crate::matter::SINKS.len()).map(|_| s.u64()).collect();
        let initial = s.u64();
        let head = s.u32();
        let free = s.u32s();
        if free.len() != n
            || (head != u32::MAX && head as usize >= n)
            || free.iter().any(|&v| v != u32::MAX && v as usize >= n)
        {
            return None;
        }
        self.economy.ledger = Ledger::from_snapshot(containers, sunk, initial, head, free)?;
        let l = &self.economy.ledger;
        for (u, id) in units.iter().zip(&self.bodies) {
            let c = l.try_get(*id)?;
            if !c.alive
                || c.kind != Kind::Body
                || c.held
                    != (u.body.solid_count() as u64
                        + u.body
                            .segments
                            .iter()
                            .map(|s| u64::from(s.body.solid_count()))
                            .sum::<u64>())
                        * self.economy.per_voxel(3)
            {
                return None;
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for f in &self.debris.fragments {
            let c = l.try_get(f.held)?;
            if !c.alive
                || c.kind != Kind::Debris
                || !ids.insert(f.held)
                || c.held != u64::from(f.count) * self.economy.per_voxel(f.material)
            {
                return None;
            }
            if f.resting {
                self.economy.wrecks.push(Wreck {
                    held: f.held,
                    x: f.position.x.to_int_trunc(),
                    y: f.position.y.to_int_trunc(),
                    z: f.position.z.to_int_trunc(),
                });
            }
        }
        Some(())
    }
}
