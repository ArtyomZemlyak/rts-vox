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

#[path = "range_fracture.rs"]
mod fracture;

#[path = "range_ground_melt.rs"]
mod ground_melt;

#[cfg(test)]
#[path = "range_fragment_admission_tests.rs"]
mod fragment_admission_tests;

#[cfg(test)]
#[path = "range_section_tests.rs"]
mod section_tests;

// A signed 21-bit coordinate per axis fits the game's current and planned
// world sizes. Biasing each component preserves the old lexicographic order
// of `(position, seed)` while making a queued pair 16 instead of 24 bytes.
const FELL_BITS: u32 = 21;
const FELL_MASK: u64 = (1 << FELL_BITS) - 1;
const FELL_BIAS: i64 = 1 << (FELL_BITS - 1);
fn fell_key(p: [i32; 3]) -> u64 {
    debug_assert!(p.iter().all(
        |&v| (i64::from(v) + FELL_BIAS) >= 0 && (i64::from(v) + FELL_BIAS) <= FELL_MASK as i64
    ));
    p.into_iter().fold(0, |key, v| {
        (key << FELL_BITS) | (i64::from(v) + FELL_BIAS) as u64
    })
}
fn fell_cell(key: u64) -> [i32; 3] {
    [42, 21, 0].map(|shift| (((key >> shift) & FELL_MASK) as i64 - FELL_BIAS) as i32)
}

pub struct Matter {
    batch: Option<fracture::Batch>,
    /// Derived phase-local admission guard, rebuilt from the saved heat field.
    /// It is not additional capacity and never survives a simulation tick.
    thermal_reserve: u32,
    body_heat_reserve: u32,
    damage_waiting: bool,
    damage_tick: u64,
    pub debris: Bursts,
    /// Diagnostic attempts since construction/load; excluded from snapshots
    /// and authoritative hashes. Never used to choose work or damage.
    pub admission: AdmissionProfile,
    /// Cells per side of the current weapon's debris pieces (ADR-284).
    fragment_box: u8,
    pub economy: Economy,
    pub tuning: modding::tuning::Blast,
    pub remaining_mj: u64,
    pub supplied_mj: u64,
    pub spent_mj: u64,
    bodies: Vec<u32>,
    /// Fixed pack used to create initial bodies and fuel, reconstructed on load.
    source_densities: Vec<(modding::Mat, u32)>,
    /// Snapshot57: fragment quantities are intrinsic, possibly mixed-density.
    pub(super) density_history: bool,
    section_work: u32,
    section_jobs: u32,
    section_cursor: usize,
    /// Yielding cells still to be released after a cut, each with the cut
    /// cell it hangs from. Ordered, bounded per tick, saved whole.
    felling: BTreeSet<(u64, u64)>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdmissionProfile {
    /// Body fracture: applied, unaffordable, pool deferred.
    pub body: [u64; 3],
    /// Ground: applied, unaffordable, no room, claim cap, duplicate, frame/outside.
    pub ground: [u64; 6],
    /// Wave geometry: bound preparations, actor maps prepared, world resumes, early ground returns.
    /// Observations only; like the other counters, excluded from state/save.
    pub wave_geometry: [u64; 4],
    /// Cursor-proven ground-only wave calls and contact visits; diagnostic only.
    pub wave_ground_only: [u64; 2],
    /// Terrain melt: attempts, applied, capacity scans, fragment records visited.
    /// Diagnostic only; never persisted or used for admission.
    pub terrain_melt: [u64; 4],
    /// Body melt: applied, pool deferred.
    pub melt: [u64; 2],
    /// Fragment scans through claim_room (not all capacity/physics queries).
    pub room_scans: u64,
    pub room_items: u64,
    /// All ordinary_fragment_room scans, including felling and terrain melt.
    /// Does not include direct Debris queries or wake's pool classification.
    pub ordinary_room_scans: u64,
    pub ordinary_room_items: u64,
    /// Felling records: unique arrivals, visits (including retries), retired,
    /// released cells, capacity deferrals. Diagnostic only, absent in save/hash.
    pub felling: [u64; 5],
}
#[derive(Debug, Default)]
pub struct FellingProfile {
    pub pending: usize,
    pub positions: usize,
    pub seeds: usize,
    pub yielding: usize,
    pub claimed: usize,
    pub outside: usize,
}

/// Root and articulated section extraction only append moving chunks. No
/// physics, claim confirmation or compaction runs inside this reservation.
/// Recount on the next phase, never retain capacity across ticks or saves.
#[derive(Default)]
struct SectionRoom {
    available: Option<u32>,
    #[cfg(test)]
    recount: bool,
}
impl SectionRoom {
    fn take(&mut self, matter: &mut Matter) -> bool {
        #[cfg(test)]
        if self.recount {
            self.available = None;
        }
        let available = self
            .available
            .get_or_insert_with(|| matter.scan_claim_room());
        if *available == 0 {
            return false;
        }
        *available -= 1;
        true
    }
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
        let source_densities = economy
            .mats
            .definitions()
            .map(|d| (d.id, d.density))
            .collect();
        Self {
            source_densities,
            density_history: false,
            batch: None,
            thermal_reserve: 0,
            body_heat_reserve: 0,
            damage_waiting: false,
            damage_tick: 0,
            debris: Bursts::default(),
            admission: AdmissionProfile::default(),
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
    /// Density of initial stock, never a live registry price. The fixed world
    /// pack is reconstructed before restoring its existing bodies and fuel.
    pub(super) fn source_density(&self, material: modding::Mat) -> u32 {
        self.source_densities
            .binary_search_by_key(&material, |d| d.0)
            .map_or(0, |i| self.source_densities[i].1)
    }
    /// Read capacity once per atomic fracture batch. Body components and
    /// successful terrain claims consume the same conservative reservations.
    pub(crate) fn claim_room(&mut self) -> u32 {
        self.batch
            .as_ref()
            .and_then(|batch| batch.room)
            .unwrap_or_else(|| self.scan_claim_room())
    }
    pub(super) fn reserve_heat(&mut self, body: usize, ground: usize) {
        let limit = if self.damage_waiting {
            crate::burst::routine_fragment_share(
                self.debris.fragment_room(&self.tuning),
                true,
                self.damage_tick,
            ) as usize
        } else {
            self.tuning.max_flying as usize
        };
        self.body_heat_reserve = body.min(limit) as u32;
        self.thermal_reserve = body.saturating_add(ground).min(limit) as u32;
    }
    /// Rebuilt from authoritative continuations; never saved as scheduler state.
    pub(super) fn reserve_damage(&mut self, waiting: bool, tick: u64) {
        self.damage_waiting = waiting;
        self.damage_tick = tick;
    }
    fn routine_fragment_room(&mut self) -> u32 {
        let available = self.ordinary_fragment_room();
        if !self.damage_waiting {
            return available;
        }
        crate::burst::routine_fragment_share(
            available.saturating_add(self.thermal_reserve),
            true,
            self.damage_tick,
        )
        .saturating_sub(self.thermal_reserve)
    }
    fn ordinary_fragment_room(&mut self) -> u32 {
        self.admission.ordinary_room_scans += 1;
        self.admission.ordinary_room_items += self.debris.fragments.len() as u64;
        self.debris
            .fragment_room(&self.tuning)
            .saturating_sub(self.thermal_reserve)
    }
    fn scan_claim_room(&mut self) -> u32 {
        self.admission.room_scans += 1;
        self.admission.room_items += self.debris.fragments.len() as u64;
        self.ordinary_fragment_room()
    }
    fn consume_body_slot(&mut self) {
        if let Some(room) = self.batch.as_mut().and_then(|batch| batch.room.as_mut()) {
            assert!(*room > 0);
            *room -= 1;
        }
    }
    pub fn felling_pending(&self) -> usize {
        self.felling.len()
    }
    /// Stable identity of the first pending record, without a full queue scan.
    /// Not a work selector: felling still uses its existing ordered queue.
    pub fn felling_head(&self) -> Option<(u64, u64)> {
        self.felling.first().copied()
    }
    /// Read-only, potentially expensive diagnostic for a completed bench.
    /// It never influences the felling order or simulation state.
    pub fn felling_profile(&self, w: &World, f: &Felling) -> FellingProfile {
        let mut result = FellingProfile {
            pending: self.felling.len(),
            ..FellingProfile::default()
        };
        let mut last = None;
        let mut seeds = std::collections::BTreeSet::new();
        for &(p, seed) in &self.felling {
            let (p, seed) = (fell_cell(p), fell_cell(seed));
            if last != Some(p) {
                result.positions += 1;
                last = Some(p);
            }
            seeds.insert(seed);
            if p[0] < 0
                || p[1] < 0
                || p[2] < 0
                || p[0] >= w.nx as i32
                || p[1] >= w.ny as i32
                || p[2] >= w.nz as i32
            {
                result.outside += 1;
                continue;
            }
            let q = p.map(|v| v as u32);
            result.yielding += usize::from(f.materials.contains(&w.material(q[0], q[1], q[2])));
            result.claimed += usize::from(self.debris.claimed(q));
        }
        result.seeds = seeds.len();
        result
    }
    /// What hangs from a cut falls: connected yielding matter no lower than
    /// the cut and within the radius. Below the cut a stump keeps standing.
    fn spread(&mut self, p: [i32; 3], seed: [i32; 3], radius: i32) {
        let seed_key = fell_key(seed);
        for a in 0..3 {
            for d in [-1, 1] {
                let mut q = p;
                q[a] += d;
                let far = (0..3).map(|k| (q[k] - seed[k]).pow(2)).sum::<i32>() > radius * radius;
                if !far && q[1] >= seed[1] {
                    self.admission.felling[0] +=
                        u64::from(self.felling.insert((fell_key(q), seed_key)));
                }
            }
        }
    }
    /// Cut every listed cell now: the hull stands in them. The fracture work
    /// is the vehicle's, supplied and spent in the same call; the kinetic
    /// share throws the pieces away from the hull. A full pool stops the cut
    /// where it is, and the hull keeps standing in the rest.
    pub fn fell(&mut self, w: &mut World, cells: &[[u32; 3]], at: [i32; 3], f: &Felling) -> usize {
        self.fell_in(w, cells, at, f, true)
    }
    /// One atomic claim-only phase. All hull cuts and the existing frontier
    /// share a lazy capacity scan; no confirmation, physics, heat reservation
    /// or tuning mutation can intervene while this method owns Matter/World.
    pub fn felling_phase<'a>(
        &mut self,
        w: &mut World,
        cuts: impl IntoIterator<Item = (&'a [[u32; 3]], [i32; 3])>,
        f: &Felling,
    ) {
        let mut room = None;
        for (cells, at) in cuts {
            self.fell_with_room(w, cells, at, f, true, &mut room);
        }
        self.step_felling_with_room(w, f, true, &mut room);
    }
    fn fell_in(
        &mut self,
        w: &mut World,
        cells: &[[u32; 3]],
        at: [i32; 3],
        f: &Felling,
        optimized: bool,
    ) -> usize {
        self.fell_with_room(w, cells, at, f, optimized, &mut None)
    }
    fn fell_with_room(
        &mut self,
        w: &mut World,
        cells: &[[u32; 3]],
        at: [i32; 3],
        f: &Felling,
        optimized: bool,
        room: &mut Option<u32>,
    ) -> usize {
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
            let claimed = if optimized {
                let capacity = room.get_or_insert_with(|| self.routine_fragment_room());
                self.debris.claim_voxel_with_room(
                    w,
                    p,
                    m,
                    at,
                    motion,
                    self.fragment_box,
                    &self.tuning,
                    capacity,
                )
            } else {
                self.debris
                    .claim_voxel(w, p, m, at, motion, self.fragment_box, &self.tuning)
            };
            if !claimed {
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
        self.step_felling_in(w, f, true);
    }
    fn step_felling_in(&mut self, w: &mut World, f: &Felling, optimized: bool) {
        self.step_felling_with_room(w, f, optimized, &mut None);
    }
    fn step_felling_with_room(
        &mut self,
        w: &mut World,
        f: &Felling,
        optimized: bool,
        room: &mut Option<u32>,
    ) {
        let mut left = f.cells_per_tick;
        while left > 0 {
            let Some((p, seed)) = self.felling.pop_first() else {
                break;
            };
            let (p, seed) = (fell_cell(p), fell_cell(seed));
            left -= 1;
            self.admission.felling[1] += 1;
            self.admission.felling[2] += 1;
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
            let claimed = if optimized {
                let capacity = room.get_or_insert_with(|| self.routine_fragment_room());
                self.debris
                    .claim_voxel_with_room(w, q, m, seed, 0, 2, &self.tuning, capacity)
            } else {
                self.debris.claim_voxel(w, q, m, seed, 0, 2, &self.tuning)
            };
            if !claimed {
                self.felling.insert((fell_key(p), fell_key(seed)));
                self.admission.felling[2] -= 1;
                self.admission.felling[4] += 1;
                break;
            }
            self.admission.felling[3] += 1;
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
        self.melt_with_room(w, p, &mut None, false)
    }
    /// Claim-only terrain pass. A reservation never survives confirmation,
    /// fragment physics, tuning changes, or the end of the caller's pass.
    fn melt_with_room(
        &mut self,
        w: &mut World,
        p: [u32; 3],
        room: &mut Option<u32>,
        thermal: bool,
    ) -> bool {
        self.admission.terrain_melt[0] += 1;
        let m = w.material(p[0], p[1], p[2]);
        if m == 0 || self.economy.mats.get(m).is_none() || !claimable(w, p.map(|v| v as i32)) {
            return false;
        }
        let room = room.get_or_insert_with(|| {
            self.admission.terrain_melt[2] += 1;
            self.admission.terrain_melt[3] += self.debris.fragments.len() as u64;
            if thermal {
                self.debris
                    .fragment_room(&self.tuning)
                    .saturating_sub(self.body_heat_reserve)
            } else {
                self.ordinary_fragment_room()
            }
        });
        let at = p.map(|v| v as i32);
        let applied = self
            .debris
            .claim_voxel_with_room(w, p, m, at, 0, 2, &self.tuning, room);
        self.admission.terrain_melt[1] += u64::from(applied);
        applied
    }
    /// Heat has already softened this cell. Transfer it without explosive
    /// fracture work or a launch impulse, like melted terrain.
    pub fn melt_unit(&mut self, id: usize, u: &RangeUnit, part: usize, cell: [usize; 3]) -> bool {
        let mut room = self.claim_room();
        let applied = self.melt_unit_with_room(id, u, part, cell, &mut room);
        if applied {
            self.consume_body_slot();
        }
        applied
    }
    /// Ordered thermal pass: only successful single-piece melts consume the
    /// caller's reservations; fragments cannot settle or wake inside the pass.
    pub(crate) fn melt_unit_with_room(
        &mut self,
        id: usize,
        u: &RangeUnit,
        part: usize,
        cell: [usize; 3],
        room: &mut u32,
    ) -> bool {
        if *room == 0 {
            self.admission.melt[1] += 1;
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
        let amount = u64::from(self.source_density(3));
        let held = self.economy.ledger.create(Kind::Debris, 0, u64::MAX);
        assert_eq!(
            self.economy.ledger.transfer(self.bodies[id], held, amount),
            amount
        );
        let density = self.source_density(3);
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
        *room -= 1;
        self.admission.melt[0] += 1;
        true
    }
    pub fn ground(&mut self, w: &mut World, p: [i32; 3], at: [i32; 3]) -> ClaimResult {
        if !claimable(w, p) {
            self.admission.ground[5] += 1;
            return ClaimResult::Indestructible;
        }
        let m = material(w, p);
        let (cost, motion) = self.cost(m);
        if cost > self.remaining_mj {
            self.admission.ground[1] += 1;
            return ClaimResult::Unaffordable;
        }
        let mut room = self.claim_room();
        let applied = self.debris.claim_voxel_checked(
            w,
            p.map(|v| v as u32),
            m,
            at,
            motion,
            self.fragment_box,
            &self.tuning,
            &mut room,
        );
        if let Some(cached) = self.batch.as_mut().and_then(|batch| batch.room.as_mut()) {
            *cached = room;
        }
        if let Err(reason) = applied {
            use crate::burst::GroundClaimError;
            self.admission.ground[match reason {
                GroundClaimError::NoRoom => 2,
                GroundClaimError::ClaimLimit => 3,
                GroundClaimError::PendingCell => 4,
                GroundClaimError::Outside | GroundClaimError::Frame => 5,
            }] += 1;
            return ClaimResult::Deferred;
        }
        self.admission.ground[0] += 1;
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
        self.claim_cell(id, 0, &u.body, cell, pose, Some(at))
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
        self.claim_cell(
            id,
            seg + 1,
            &u.body.segments[seg].body,
            cell,
            frame,
            Some(at),
        )
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
        self.claim_cell(id, part, body, cell, frame, None)
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
        part: usize,
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
            self.admission.body[1] += 1;
            return ClaimResult::Unaffordable;
        }
        let reserved_component = self
            .batch
            .as_ref()
            .is_some_and(|batch| batch.room.is_some());
        let admitted = if reserved_component {
            self.batch.as_mut().unwrap().reserve(
                id,
                part,
                cell,
                *frame,
                self.fragment_box,
                body.part_at(cell[0], cell[1], cell[2]) as u8,
                &self.tuning,
            )
        } else {
            self.claim_room() > 0
        };
        if !admitted {
            self.admission.body[2] += 1;
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
        let amount = u64::from(self.source_density(3));
        let held = self.economy.ledger.create(Kind::Debris, 0, u64::MAX);
        assert_eq!(
            self.economy.ledger.transfer(self.bodies[id], held, amount),
            amount
        );
        let mass_g = u64::from(self.source_density(3))
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
        if let Some(batch) = &mut self.batch {
            batch.record(
                self.debris.fragments.len() - 1,
                id,
                part,
                cell,
                *frame,
                self.fragment_box,
                &self.tuning,
            );
        }
        // Component admission already debited (or returned) the exact number
        // of final pieces. The scalar path still reserves one emitted cell.
        if !reserved_component {
            self.consume_body_slot();
        }
        self.admission.body[0] += 1;
        self.pay(cost);
        ClaimResult::Applied
    }
    /// Detached segments leave whole, with the body's motion, as chunks that
    /// keep their cells and shape; matter moves with them. A full pool only
    /// postpones the rest (ADR-278).
    pub fn shed_segments(&mut self, units: &mut [RangeUnit]) {
        self.shed_segments_budget(units, self.section_work);
    }
    pub fn shed_segments_budget(&mut self, units: &mut [RangeUnit], budget: u32) {
        self.shed_segments_in(units, budget, &mut SectionRoom::default());
    }
    fn shed_segments_in(
        &mut self,
        units: &mut [RangeUnit],
        mut budget: u32,
        room: &mut SectionRoom,
    ) {
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
                    if !job.ready() || !room.take(self) {
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
                    let amount = u64::from(count) * u64::from(self.source_density(3));
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
                    let density = self.source_density(3);
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
                        // Its cells are placed by the captured segment frame:
                        // the basis is the turn since it left (ADR-468), and
                        // the frame's axes here turned it twice.
                        basis: crate::burst::IDENTITY,
                        mass_g: u64::from(density)
                            * u64::from(self.tuning.density_kg_m3_per_density)
                            * u64::from(mm).pow(3)
                            / 1_000_000
                            * u64::from(count),
                        count,
                        shape: 1,
                        side: 2,
                        chunk: Some(Box::new(chunk)),
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
        let amount = count as u64 * u64::from(self.source_density(3));
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
        self.step_sections_in(units, &mut SectionRoom::default())
    }
    /// One lazy capacity scan for root chunks followed by detached limbs.
    pub(super) fn step_unit_sections(&mut self, units: &mut [RangeUnit]) {
        let mut room = SectionRoom::default();
        let work = self.step_sections_in(units, &mut room);
        self.shed_segments_in(units, work, &mut room);
    }
    fn step_sections_in(&mut self, units: &mut [RangeUnit], room: &mut SectionRoom) -> u32 {
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
                if !job.ready() || !room.take(self) {
                    break;
                }
                committed = true;
                let (body, part) = job.commit(&u.body);
                let count = part.cells.len() as u32;
                let amount = u64::from(count) * u64::from(self.source_density(3));
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
                let density = self.source_density(3);
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
                    chunk: Some(Box::new(chunk)),
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
        self.flush_observed(
            w,
            material,
            gravity,
            tick,
            hulls,
            yielding,
            |_| {},
            &|_, _| {},
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn flush_observed(
        &mut self,
        w: &World,
        material: bool,
        gravity: i32,
        tick: u64,
        hulls: &[crate::burst::Hull],
        yielding: &[voxel::store::Mat],
        mut completed: impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        // Unclaimed compare-and-remove edits are ablation, even alongside a
        // conventional wave. Claimed edits belong to debris, not this sink.
        for &(edit, old) in w.completed() {
            if edit.expected_material() == Some(old)
                && old != AIR
                && !self.debris.claimed([edit.x, edit.y, edit.z])
            {
                self.annihilate_cells(&[([edit.x, edit.y, edit.z], old)]);
            }
        }
        // Extra pieces keep clear of the thermal reserve (ADR-659).
        self.debris.confirm_reserving(
            w.completed(),
            &mut self.economy,
            &self.tuning,
            self.thermal_reserve,
        );
        for &(by, mat) in w.taken() {
            // A voxel consumed by fire went to the combustion sink whatever
            // the weapon; the rest of an annihilating weapon's take is its.
            // A cell the collapse layer dug is nobody's take (ADR-300): it
            // falls as a body, crumbles into pieces, or was sunk already.
            let sink = if by == crate::heat::FIRE {
                Sink::Combustion
            } else if by == voxel::world::NOBODY
                || by & 0xffff0000 == 0xfffe0000
                || super::economy::dig_slot(by).is_some()
            {
                // A digger's take was credited to its tank (ADR-672).
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
        completed("debris_confirm");
        self.debris.step_fragments_reserved(
            w,
            &mut self.economy,
            &self.tuning,
            Fx::from_ratio(gravity, crate::TICKS_PER_SECOND.pow(2)),
            tick,
            crate::burst::Surroundings { hulls, yielding },
            self.thermal_reserve,
            self.damage_waiting,
            completed,
            task,
        );
        self.thermal_reserve = 0;
        self.body_heat_reserve = 0;
        self.damage_waiting = false;
    }
    pub fn save(&self, b: &mut Blob) {
        assert!(self.batch.is_none(), "fracture batches are atomic");
        b.u8(self.fragment_box);
        b.u64(self.remaining_mj);
        b.u64(self.supplied_mj);
        b.u64(self.spent_mj);
        b.usize(self.section_cursor);
        b.u32(self.felling.len() as u32);
        for &(p, seed) in &self.felling {
            let (p, seed) = (fell_cell(p), fell_cell(seed));
            for v in p.iter().chain(seed.iter()) {
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
        self.load_version(s, w, units, true)
    }
    /// `claim_materials`: the save holds each claim's material (ADR-659,
    /// snapshot version 61).
    pub fn load_version(
        &mut self,
        s: &mut Scan,
        w: &World,
        units: &[RangeUnit],
        claim_materials: bool,
    ) -> Option<()> {
        self.admission = AdmissionProfile::default();
        self.thermal_reserve = 0;
        self.body_heat_reserve = 0;
        self.damage_waiting = false;
        self.damage_tick = 0;
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
        // A kilometre-scale battle can have over 65k crown cells awaiting
        // support updates. Bound an untrusted count by the encoded bytes as
        // well as an absolute ceiling instead of rejecting valid snapshots.
        if n > 1 << 24 || n > s.left() / 24 {
            return None;
        }
        self.felling.clear();
        for _ in 0..n {
            let p: [i32; 3] = std::array::from_fn(|_| s.i32());
            let seed: [i32; 3] = std::array::from_fn(|_| s.i32());
            let dims = [w.nx, w.ny, w.nz];
            if (0..3).any(|a| {
                p[a] < -1
                    || p[a] > dims[a] as i32
                    || seed[a] < 0
                    || seed[a] >= dims[a] as i32
                    || [p[a], seed[a]].into_iter().any(|v| {
                        let shifted = i64::from(v) + FELL_BIAS;
                        shifted < 0 || shifted > FELL_MASK as i64
                    })
            }) || !self.felling.insert((fell_key(p), fell_key(seed)))
            {
                return None;
            }
        }
        let templates = crate::damage::Bodies::new(
            units
                .iter()
                .map(|u| match u.kind {
                    modding::range::RangeUnitKind::Tank => voxel::unit::tank(),
                    modding::range::RangeUnitKind::Artillery => voxel::unit::artillery(),
                    modding::range::RangeUnitKind::ArtilleryT2 => voxel::unit::artillery_t2(),
                    modding::range::RangeUnitKind::ArtilleryTurretT2 => {
                        voxel::unit::artillery_turret_t2()
                    }
                    modding::range::RangeUnitKind::Strider => voxel::unit::strider(),
                    modding::range::RangeUnitKind::Spider => voxel::unit::spider(),
                    modding::range::RangeUnitKind::TurretT1 => voxel::unit::turret_t1(),
                    modding::range::RangeUnitKind::TurretT2 => voxel::unit::turret_t2(),
                    modding::range::RangeUnitKind::Base => voxel::unit::command_base(),
                })
                .collect(),
        );
        self.debris = Bursts::load_material(
            s,
            Some(w),
            &self.tuning,
            Some(&templates),
            0,
            false,
            claim_materials,
        )?;
        let n = s.usize();
        // An atomic body batch can use up to 64 temporary cell containers for
        // each final piece. Retired donor slots remain on the Ledger free list;
        // they are not live fragments, but must round-trip with the snapshot.
        // Also bound by encoded bytes before allocating an untrusted count.
        let ledger_limit = (self.tuning.max_fragments as usize)
            .saturating_mul(4usize.pow(3))
            .saturating_add(self.tuning.max_resting_fragments as usize)
            .saturating_add(units.len().saturating_mul(3).saturating_add(3));
        if n > ledger_limit || n > s.left() / 18 {
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
                        * u64::from(self.source_density(3))
            {
                return None;
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for f in &self.debris.fragments {
            let c = l.try_get(f.held)?;
            // A resting pile may combine material revealed at different densities.
            // Its ledger quantity and mass are conserved state, not a valuation
            // at today's registry setting. Old formats retain exact validation.
            let valid_quantity = if self.density_history {
                self.economy
                    .mats
                    .get(f.material)
                    .is_some_and(|d| d.density != 0)
                    && if f.paint.is_some() {
                        c.held == u64::from(f.count) * u64::from(self.source_density(f.material))
                    } else {
                        c.held >= u64::from(f.count)
                            && c.held <= u64::from(f.count) * u64::from(modding::MAX_DENSITY)
                    }
            } else {
                c.held == u64::from(f.count) * self.economy.per_voxel(f.material)
            };
            if !c.alive || c.kind != Kind::Debris || !ids.insert(f.held) || !valid_quantity {
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

#[cfg(test)]
mod felling_key_tests {
    use super::{fell_cell, fell_key, Matter};
    use fx::blob::Blob;
    use std::collections::BTreeSet;

    #[test]
    fn shared_felling_phase_matches_separate_claim_batches_at_pool_boundaries() {
        struct Grove;
        impl voxel::Generator for Grove {
            fn material(&self, _: u32, y: u32, _: u32) -> u16 {
                if y < 4 {
                    1
                } else if y < 12 {
                    4
                } else {
                    0
                }
            }
        }
        let snapshot = |m: &Matter, w: &voxel::World| {
            let mut b = Blob::new();
            m.save(&mut b);
            w.save(&mut b);
            b.v
        };
        let cuts = [
            (vec![[4, 4, 4], [5, 4, 4], [4, 4, 4]], [4, 4, 4]),
            (vec![[5, 4, 4], [6, 4, 4], [7, 4, 4]], [6, 4, 4]),
            (vec![[12, 4, 4], [0, 4, 4], [13, 4, 4]], [12, 4, 4]),
        ];
        for cap in [0, 1, 3, 64] {
            for reserve in [0, 1, 4] {
                let mut actual = Matter::new(&[]);
                let mut reference = Matter::new(&[]);
                let mut world = voxel::World::new(32, 16, 32, 4096, Box::new(Grove));
                let mut old_world = voxel::World::new(32, 16, 32, 4096, Box::new(Grove));
                let mut f = modding::navigation::Navigation::from_ron(include_str!(
                    "../../../data/range-navigation.ron"
                ))
                .unwrap()
                .fell;
                f.cells_per_tick = 31;
                for tick in 0..4 {
                    for m in [&mut actual, &mut reference] {
                        // Invalidate at phase boundaries: capacity and thermal
                        // reservations change after the initial exhausted pass.
                        m.tuning.max_flying = if tick < 2 { cap } else { 128 };
                        m.tuning.max_fragments = if tick < 2 { cap } else { 128 };
                        m.tuning.max_claims = if tick < 2 { cap } else { 128 };
                        m.reserve_heat(0, if tick < 2 { reserve } else { 0 });
                    }
                    let fast = actual.admission.ordinary_room_scans;
                    actual.felling_phase(
                        &mut world,
                        cuts.iter().map(|(cells, at)| (cells.as_slice(), *at)),
                        &f,
                    );
                    for (cells, at) in &cuts {
                        reference.fell(&mut old_world, cells, *at, &f);
                    }
                    reference.step_felling(&mut old_world, &f);
                    assert!(actual.admission.ordinary_room_scans - fast <= 1);
                    assert_eq!(
                        snapshot(&actual, &world),
                        snapshot(&reference, &old_world),
                        "cap={cap}, reserve={reserve}, tick={tick}"
                    );
                    assert_eq!(actual.admission.felling, reference.admission.felling);
                    world.flush();
                    old_world.flush();
                }
                assert!(
                    actual.admission.ordinary_room_scans < reference.admission.ordinary_room_scans
                );
            }
        }
    }

    #[test]
    fn batched_felling_claims_preserve_frontier_edits_and_pool_deferral() {
        struct Grove;
        impl voxel::Generator for Grove {
            fn material(&self, x: u32, y: u32, z: u32) -> u16 {
                if y < 4 {
                    1
                } else {
                    [4, 5, 3, 0][((x + y + z) % 4) as usize]
                }
            }
        }
        let initial: BTreeSet<_> = (2..24)
            .flat_map(|x| {
                (4..12).flat_map(move |y| {
                    (2..24).flat_map(move |z| {
                        [[x, 2, z], [x, 3, z]].map(|seed| (fell_key([x, y, z]), fell_key(seed)))
                    })
                })
            })
            .chain(
                [
                    ([-1, 4, 2], [2, 2, 2]),
                    ([0, 4, 2], [2, 2, 2]),
                    ([64, 4, 2], [63, 2, 2]),
                ]
                .map(|(p, seed)| (fell_key(p), fell_key(seed))),
            )
            .collect();
        let snapshot = |m: &Matter, w: &voxel::World| {
            let mut b = Blob::new();
            m.save(&mut b);
            w.save(&mut b);
            b.v
        };
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for budget in [1, 31, 1023, 4096] {
                for (capacity, flying, fragments) in [
                    (0, 4096, 32768),
                    (7, 4096, 32768),
                    (4096, 3, 32768),
                    (4096, 4096, 5),
                    (4096, 4096, 32768),
                ] {
                    let mut reference = Matter::new(&[]);
                    let mut actual = Matter::new(&[]);
                    reference.felling = initial.clone();
                    actual.felling = initial.clone();
                    reference.tuning.max_claims = capacity;
                    actual.tuning.max_claims = capacity;
                    reference.tuning.max_flying = flying;
                    actual.tuning.max_flying = flying;
                    reference.tuning.max_fragments = fragments;
                    actual.tuning.max_fragments = fragments;
                    let mut old_world = voxel::World::new(64, 32, 64, 4096, Box::new(Grove));
                    let mut world = voxel::World::new(64, 32, 64, 4096, Box::new(Grove));
                    let mut f = modding::navigation::Navigation::from_ron(include_str!(
                        "../../../data/range-navigation.ron"
                    ))
                    .unwrap()
                    .fell;
                    f.cells_per_tick = budget;
                    let cuts: Vec<_> = (2..10)
                        .flat_map(|x| (4..8).flat_map(move |y| (2..10).map(move |z| [x, y, z])))
                        .chain([[0, 5, 1], [3, 4, 3], [3, 4, 3]])
                        .collect();
                    assert_eq!(
                        actual.fell_in(&mut world, &cuts, [3, 4, 3], &f, true),
                        reference.fell_in(&mut old_world, &cuts, [3, 4, 3], &f, false)
                    );
                    assert_eq!(snapshot(&actual, &world), snapshot(&reference, &old_world));
                    for tick in 0..4 {
                        if tick == 2 {
                            reference.tuning.max_claims = 4096;
                            actual.tuning.max_claims = 4096;
                            reference.tuning.max_flying = 4096;
                            actual.tuning.max_flying = 4096;
                            reference.tuning.max_fragments = 32768;
                            actual.tuning.max_fragments = 32768;
                        }
                        reference.step_felling_in(&mut old_world, &f, false);
                        pool.install(|| actual.step_felling_in(&mut world, &f, true));
                        let [arrivals, visits, retired, released, deferrals] =
                            actual.admission.felling;
                        assert_eq!(visits, retired + deferrals);
                        assert!(released <= retired);
                        assert_eq!(
                            initial.len() as u64 + arrivals - retired,
                            actual.felling.len() as u64,
                            "diagnostics must account for deduplication and retries"
                        );
                        assert_eq!(
                            snapshot(&actual, &world),
                            snapshot(&reference, &old_world),
                            "workers={workers}, budget={budget}, capacity={capacity}, tick={tick}"
                        );
                        if tick == 0
                            && budget == 4096
                            && capacity == 4096
                            && flying == 4096
                            && fragments == 32768
                        {
                            assert!(world.queued() > 0);
                            assert!(actual.debris.claims() > 0);
                            assert!(
                                actual.felling.iter().any(|key| !initial.contains(key)),
                                "new frontier must be admitted during this phase"
                            );
                        }
                        world.flush();
                        old_world.flush();
                        assert_eq!(snapshot(&actual, &world), snapshot(&reference, &old_world));
                    }
                }
            }
        }
    }

    #[test]
    fn packed_felling_queue_preserves_signed_position_and_seed_order() {
        let cells = [
            [-1, 0, 0],
            [0, -1, 0],
            [0, 0, -1],
            [0, 0, 0],
            [10000, 191, 10000],
            [10001, 192, 10001],
        ];
        for p in cells {
            assert_eq!(fell_cell(fell_key(p)), p);
        }
        let old: BTreeSet<_> = cells
            .into_iter()
            .flat_map(|p| cells.into_iter().map(move |seed| (p, seed)))
            .collect();
        let packed: BTreeSet<_> = old
            .iter()
            .map(|&(p, seed)| (fell_key(p), fell_key(seed)))
            .collect();
        assert_eq!(
            old.into_iter().collect::<Vec<_>>(),
            packed
                .into_iter()
                .map(|(p, seed)| (fell_cell(p), fell_cell(seed)))
                .collect::<Vec<_>>()
        );
    }
}
