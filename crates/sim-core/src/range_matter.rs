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
}

pub struct Matter {
    pub debris: Bursts,
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
                let n = u.body.solid_count() as u64 * amount;
                economy.ledger.create(Kind::Body, n, n)
            })
            .collect();
        Self {
            debris: Bursts::default(),
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
            if m == AIR || !f.materials.contains(&m) || self.debris.claimed(p) {
                continue;
            }
            let (cost, motion) = self.cost(m);
            if !self.debris.claim_voxel(w, p, m, at, motion, &self.tuning) {
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
            if m == AIR || !f.materials.contains(&m) || self.debris.claimed(q) {
                continue;
            }
            if !self.debris.claim_voxel(w, q, m, seed, 0, &self.tuning) {
                self.felling.insert((p, seed));
                break;
            }
            self.spread(p, seed, f.radius);
        }
    }
    pub fn begin(&mut self, joules: u32) {
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
    pub fn ground(&mut self, w: &mut World, p: [i32; 3], at: [i32; 3]) -> ClaimResult {
        let m = material(w, p);
        let (cost, motion) = self.cost(m);
        if cost > self.remaining_mj {
            return ClaimResult::Unaffordable;
        }
        if !self
            .debris
            .claim_voxel(w, p.map(|v| v as u32), m, at, motion, &self.tuning)
        {
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
        let (cost, motion) = self.cost(3);
        if cost > self.remaining_mj {
            return ClaimResult::Unaffordable;
        }
        if self.debris.fragment_room(&self.tuning) == 0 {
            return ClaimResult::Deferred;
        }
        let position = pose.world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
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
            direction = pose.y;
        }
        let amount = self.economy.per_voxel(3);
        let held = self.economy.ledger.create(Kind::Debris, 0, u64::MAX);
        assert_eq!(
            self.economy.ledger.transfer(self.bodies[id], held, amount),
            amount
        );
        let d = self.economy.mats.get(3).unwrap();
        let mass_g = u64::from(d.density)
            * u64::from(self.tuning.density_kg_m3_per_density)
            * u64::from(u.body.mm).pow(3)
            / 1_000_000;
        let [x, y, z] = cell;
        self.debris.emit_body_fragment(
            Fragment {
                id: 0,
                position,
                velocity: V3::ZERO,
                material: 3,
                paint: Some([1, u.body.finish_at(x, y, z), u.body.part_at(x, y, z) as u8]),
                mm: u.body.mm,
                basis: [pose.x, pose.y, pose.z],
                mass_g,
                count: 1,
                shape: 1,
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
    pub fn step_sections(&mut self, units: &mut [RangeUnit]) {
        let n = units.len();
        if n == 0 {
            return;
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
                    chunk: Some(chunk),
                    held,
                    resting: false,
                });
                if budget == 0 {
                    break;
                }
            }
            left -= start - budget;
            if committed {
                u.update_response();
            }
            if !job.finished() {
                u.section = Some(job);
            }
        }
    }
    pub fn flush(&mut self, w: &World, material: bool, gravity: i32) {
        self.debris
            .confirm(w.completed(), &mut self.economy, &self.tuning);
        if !material {
            for &(_, mat) in w.taken() {
                let amount = self.economy.per_voxel(mat);
                self.economy.ledger.reveal(self.economy.ground, amount);
                self.economy
                    .ledger
                    .sink(self.economy.ground, amount, Sink::Annihilation);
            }
        }
        self.debris.step_fragments(
            w,
            &mut self.economy,
            &self.tuning,
            Fx::from_ratio(gravity, crate::TICKS_PER_SECOND.pow(2)),
        );
    }
    pub fn save(&self, b: &mut Blob) {
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
        self.debris = Bursts::load_material(s, Some(w), &self.tuning, Some(&templates), 0)?;
        let n = s.usize();
        if n > self.tuning.max_fragments as usize + units.len() + 2 {
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
                || c.held != u.body.solid_count() as u64 * self.economy.per_voxel(3)
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
