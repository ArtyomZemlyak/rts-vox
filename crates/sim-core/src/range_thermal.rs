//! Bounded local heat on surviving range bodies. No scalar unit hit points.
//! Conduction reads the old field; gains are committed together. Removed
//! material takes its heat with it. Cross-volume contact is a separate phase.
use super::{matter::Matter, BlastRange, RangeUnit};
use fx::{
    blob::{Blob, Scan},
    Fx,
};
use modding::unit_heat::Tuning;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};
use voxel::unit::{UnitBody, WORLD_MM};
const HEAT_CAP: u32 = 1_000_000;

/// Geometry and neighbour energy are immutable until the melting pass.
/// Capacity admission and total outflow still follow the source cursor order.
struct ConductionRead {
    heat: u32,
    neighbours: [Option<(Cell, u32)>; 6],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cell {
    pub unit: u16,
    /// Zero is the root; other values are segment index + 1.
    pub part: u8,
    pub at: [u8; 3],
}
impl Cell {
    pub fn body(self, units: &[RangeUnit]) -> Option<&UnitBody> {
        let u = units.get(usize::from(self.unit))?;
        let b = if self.part == 0 {
            &u.body
        } else {
            let i = usize::from(self.part - 1);
            if *u.seg_gone.get(i)? {
                return None;
            }
            &u.body.segments.get(i)?.body
        };
        let [x, y, z] = self.at.map(usize::from);
        (x < usize::from(b.vol.nx)
            && y < usize::from(b.vol.ny)
            && z < usize::from(b.vol.nz)
            && b.vol.get(x, y, z))
        .then_some(b)
    }
    pub fn world(self, units: &[RangeUnit]) -> Option<[Fx; 3]> {
        self.body(units)?;
        let u = &units[usize::from(self.unit)];
        let root_frame = u.root_frame(usize::from(self.at[1]));
        let frame = if self.part == 0 {
            &root_frame
        } else {
            &u.seg_frames[usize::from(self.part - 1)]
        };
        let p = frame.world(
            self.at
                .map(|v| Fx::from_int(i32::from(v)).add(Fx::from_ratio(1, 2))),
        );
        Some([p.x, p.y, p.z])
    }
    fn neighbours(self) -> impl Iterator<Item = Self> {
        (0..3).flat_map(move |axis| {
            [-1, 1].into_iter().filter_map(move |d| {
                let value = i32::from(self.at[axis]) + d;
                if !(0..=255).contains(&value) {
                    return None;
                }
                let mut q = self;
                q.at[axis] = value as u8;
                Some(q)
            })
        })
    }
    pub(super) fn save(self, b: &mut Blob) {
        b.u32(u32::from(self.unit));
        b.u8(self.part);
        for v in self.at {
            b.u8(v);
        }
    }
    pub(super) fn load(s: &mut Scan) -> Option<Self> {
        Some(Self {
            unit: u16::try_from(s.u32()).ok()?,
            part: s.u8(),
            at: [s.u8(), s.u8(), s.u8()],
        })
    }
}
/// Energy normalized by physical volume, for comparing different cell sizes.
pub fn temperature(heat: u32, mm: u16) -> u32 {
    (u64::from(heat) * u64::from(WORLD_MM).pow(3) / u64::from(mm).max(1).pow(3))
        .min(u64::from(HEAT_CAP)) as u32
}
#[derive(Default)]
pub struct Thermals {
    cells: BTreeMap<Cell, u32>,
    cursor: Option<Cell>,
    pub melted: u64,
    /// Saturated direct deposits dissipate; never silently evict another cell.
    pub rejected_heat: u64,
}
impl Thermals {
    pub(super) fn batch_after(&self, cursor: Option<Cell>, limit: usize) -> Vec<Cell> {
        let mut out: Vec<_> = cursor.map_or_else(Vec::new, |c| {
            self.cells
                .range((Excluded(c), Unbounded))
                .map(|(p, _)| *p)
                .take(limit)
                .collect()
        });
        out.extend(
            self.cells
                .iter()
                .take_while(|(p, _)| cursor.is_none_or(|c| **p <= c))
                .map(|(p, _)| *p)
                .take(limit - out.len()),
        );
        out
    }
    pub(super) fn take_heat(&mut self, p: Cell, amount: u32) {
        if let Some(h) = self.cells.get_mut(&p) {
            *h = h.saturating_sub(amount);
            if *h == 0 {
                self.cells.remove(&p);
            }
        }
    }
    pub fn cells(&self) -> impl Iterator<Item = (Cell, u32)> + '_ {
        self.cells.iter().map(|(p, h)| (*p, *h))
    }
    pub fn hot(&self) -> usize {
        self.cells.len()
    }
    /// Bounded forecast for the next melting pass. Warm smoke/glow alone
    /// must not withhold capacity from ordinary projectile damage.
    pub(super) fn melting_slots(&self, units: &[RangeUnit], t: &Tuning) -> usize {
        self.batch_after(self.cursor, t.cells_per_tick)
            .into_iter()
            .filter(|&p| {
                p.body(units).is_some_and(|body| {
                    let [x, y, z] = p.at.map(usize::from);
                    temperature(self.heat(p), body.mm)
                        >= t.softening[body.part_at(x, y, z) as usize]
                        && !units[usize::from(p.unit)].section_holds(usize::from(p.part), p.at)
                })
            })
            .count()
    }
    pub fn heat(&self, p: Cell) -> u32 {
        self.cells.get(&p).copied().unwrap_or(0)
    }
    pub fn deposit(&mut self, p: Cell, heat: u32, t: &Tuning) {
        if heat == 0 {
            return;
        }
        if self.cells.len() >= t.max_cells && !self.cells.contains_key(&p) {
            self.rejected_heat = self.rejected_heat.saturating_add(u64::from(heat));
            return;
        }
        let h = self.cells.entry(p).or_default();
        let used = heat.min(HEAT_CAP - *h);
        *h += used;
        self.rejected_heat = self.rejected_heat.saturating_add(u64::from(heat - used));
    }
    pub fn step(&mut self, units: &mut [RangeUnit], matter: &mut Matter, t: &Tuning) {
        self.step_observed(units, matter, t, |_| {}, &|_, _| {});
    }
    fn conduction_read(&self, p: Cell, units: &[RangeUnit], t: &Tuning) -> Option<ConductionRead> {
        p.body(units)?;
        let heat = self.heat(p);
        let mut neighbours = [None; 6];
        for (slot, q) in neighbours.iter_mut().zip(p.neighbours()) {
            if q.body(units).is_some() {
                let other = self.heat(q);
                if heat > other {
                    *slot = Some((q, (heat - other) * t.conduction_milli / 1000));
                }
            }
        }
        Some(ConductionRead { heat, neighbours })
    }
    fn conduction_delta(
        &mut self,
        batch: &[Cell],
        units: &[RangeUnit],
        t: &Tuning,
        parallel: bool,
        task: &(impl Fn(usize, &'static str) + Send + Sync),
    ) -> BTreeMap<Cell, i64> {
        let read = |&p: &Cell| self.conduction_read(p, units, t);
        let reads: Vec<_> = if parallel && batch.len() >= 256 && rayon::current_num_threads() > 1 {
            batch
                .par_iter()
                .with_min_len(128)
                .map_init(
                    || {
                        crate::burst::PhysicsTask::new(
                            task,
                            "physics_unit_heat_read_begin",
                            "physics_unit_heat_read_end",
                        )
                    },
                    |_, p| read(p),
                )
                .collect()
        } else {
            let _read = crate::burst::PhysicsTask::new(
                task,
                "physics_unit_heat_read_begin",
                "physics_unit_heat_read_end",
            );
            batch.iter().map(read).collect()
        };
        let _commit = crate::burst::PhysicsTask::new(
            task,
            "physics_unit_heat_admit_begin",
            "physics_unit_heat_admit_end",
        );
        let mut delta = BTreeMap::<Cell, i64>::new();
        let mut new = BTreeSet::new();
        for (&p, read) in batch.iter().zip(reads) {
            let Some(read) = read else {
                // Keep removal here: it frees capacity at precisely the old
                // source position, including when the cursor wraps.
                self.cells.remove(&p);
                continue;
            };
            let heat = read.heat;
            let cooling = if t.cooling_milli == 0 {
                0
            } else {
                (heat * t.cooling_milli / 1000).max(1)
            };
            let mut spent = i64::from(cooling);
            let mut available = heat.saturating_sub(cooling);
            for (q, requested) in read.neighbours.into_iter().flatten() {
                let share = requested.min(available);
                if share == 0 {
                    continue;
                }
                if !self.cells.contains_key(&q) && !new.contains(&q) {
                    if self.cells.len() + new.len() >= t.max_cells {
                        continue;
                    }
                    new.insert(q);
                }
                spent += i64::from(share);
                *delta.entry(q).or_default() += i64::from(share);
                available -= share;
            }
            *delta.entry(p).or_default() -= spent;
        }
        delta
    }
    fn step_observed(
        &mut self,
        units: &mut [RangeUnit],
        matter: &mut Matter,
        t: &Tuning,
        mut completed: impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Send + Sync),
    ) {
        let _total = crate::burst::PhysicsTask::new(
            task,
            "physics_unit_heat_begin",
            "physics_unit_heat_end",
        );
        let batch = self.batch_after(self.cursor, t.cells_per_tick);
        self.cursor = batch.last().copied();
        let delta = self.conduction_delta(&batch, units, t, true, task);
        completed("unit_heat_conduction");
        for (p, change) in delta {
            let heat = i64::from(self.heat(p)) + change;
            if heat <= 0 {
                self.cells.remove(&p);
            } else {
                self.cells.insert(p, heat.min(i64::from(HEAT_CAP)) as u32);
            }
        }
        completed("unit_heat_commit");
        let mut changed = BTreeSet::new();
        // Nothing can free a fragment slot during this ordered melting pass.
        // Each successful melt emits exactly one flying piece; once full,
        // further attempts would only rescan every fragment to refuse it.
        let mut room = if batch.is_empty() {
            0
        } else {
            matter.debris.fragment_room(&matter.tuning)
        };
        for p in batch {
            let Some(body) = p.body(units) else {
                self.cells.remove(&p);
                continue;
            };
            let cell = p.at.map(usize::from);
            let role = body.part_at(cell[0], cell[1], cell[2]) as usize;
            if temperature(self.heat(p), body.mm) < t.softening[role] {
                continue;
            }
            let id = usize::from(p.unit);
            let u = &mut units[id];
            // A detached section already owns its geometry and matter.
            if u.section_holds(usize::from(p.part), p.at) {
                continue;
            }
            if !matter.melt_unit_with_room(id, u, usize::from(p.part), cell, &mut room) {
                continue;
            }
            if p.part == 0 {
                u.body.remove(cell[0], cell[1], cell[2]);
                u.removed.push(p.at);
                u.dents.remove(&p.at);
                u.detached += 1;
            } else {
                let seg = p.part - 1;
                u.body.segments[usize::from(seg)]
                    .body
                    .remove(cell[0], cell[1], cell[2]);
                u.seg_removed.push((seg, p.at));
                u.seg_dents.remove(&(seg, p.at));
                u.seg_detached += 1;
            }
            u.revision += 1;
            self.cells.remove(&p);
            self.melted += 1;
            changed.insert(id);
        }
        completed("unit_heat_melt");
        for id in changed {
            units[id].refresh_damage(id);
        }
        completed("unit_heat_damage");
    }
    pub fn save(&self, b: &mut Blob) {
        b.usize(self.cells.len());
        for (p, h) in &self.cells {
            p.save(b);
            b.u32(*h);
        }
        b.bool(self.cursor.is_some());
        if let Some(p) = self.cursor {
            p.save(b);
        }
        b.u64(self.melted);
        b.u64(self.rejected_heat);
    }
    pub fn load(&mut self, s: &mut Scan, units: &[RangeUnit], t: &Tuning) -> Option<()> {
        let n = s.usize();
        if n > t.max_cells {
            return None;
        }
        self.cells.clear();
        let mut prev = None;
        for _ in 0..n {
            let p = Cell::load(s)?;
            let heat = s.u32();
            // Removed geometry can remain queued until its next budget slice.
            let u = units.get(usize::from(p.unit))?;
            let b = if p.part == 0 {
                &u.body
            } else {
                &u.body.segments.get(usize::from(p.part - 1))?.body
            };
            let dims = [b.vol.nx, b.vol.ny, b.vol.nz];
            if prev.is_some_and(|old| old >= p)
                || !(1..=HEAT_CAP).contains(&heat)
                || (0..3).any(|a| p.at[a] >= dims[a])
            {
                return None;
            }
            prev = Some(p);
            self.cells.insert(p, heat);
        }
        self.cursor = if s.bool() { Some(Cell::load(s)?) } else { None };
        if self
            .cursor
            .is_some_and(|p| usize::from(p.unit) >= units.len())
        {
            return None;
        }
        self.melted = s.u64();
        self.rejected_heat = s.u64();
        s.ok().then_some(())
    }
}
impl BlastRange {
    pub(super) fn reserve_thermal_fragments(&mut self) {
        let slots = self
            .thermals
            .melting_slots(&self.units, &self.thermal_tuning);
        let ground = self.heat.melting_slots(
            &self.world,
            &self.matter.economy.mats,
            self.heat_tuning.cells_per_tick,
            self.thermal_tuning.cells_per_tick.saturating_sub(slots),
            |p| self.matter.debris.claimed(p),
        );
        self.matter.reserve_heat(slots, ground);
    }
    #[cfg(test)]
    pub(super) fn step_unit_heat(&mut self) {
        self.step_unit_heat_observed(|_| {}, &|_, _| {});
    }
    pub(super) fn step_unit_heat_observed(
        &mut self,
        completed: impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Send + Sync),
    ) {
        self.thermals.step_observed(
            &mut self.units,
            &mut self.matter,
            &self.thermal_tuning,
            completed,
            task,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel::unit::Part;
    // Original cursor-ordered conduction, including admission and stale removal.
    fn old_delta(
        field: &mut Thermals,
        batch: &[Cell],
        units: &[RangeUnit],
        t: &Tuning,
    ) -> BTreeMap<Cell, i64> {
        let mut delta = BTreeMap::<Cell, i64>::new();
        let mut new = BTreeSet::new();
        for &p in batch {
            let heat = field.heat(p);
            if p.body(units).is_none() {
                field.cells.remove(&p);
                continue;
            }
            let cooling = if t.cooling_milli == 0 {
                0
            } else {
                (heat * t.cooling_milli / 1000).max(1)
            };
            *delta.entry(p).or_default() -= i64::from(cooling);
            let mut available = heat.saturating_sub(cooling);
            for q in p.neighbours() {
                if q.body(units).is_none() {
                    continue;
                }
                let share =
                    (heat.saturating_sub(field.heat(q)) * t.conduction_milli / 1000).min(available);
                if share == 0 {
                    continue;
                }
                if !field.cells.contains_key(&q) && !new.contains(&q) {
                    if field.cells.len() + new.len() >= t.max_cells {
                        continue;
                    }
                    new.insert(q);
                }
                *delta.entry(p).or_default() -= i64::from(share);
                *delta.entry(q).or_default() += i64::from(share);
                available -= share;
            }
        }
        delta
    }
    #[test]
    fn parallel_conduction_matches_old_admission_with_cursor_wrap_and_removed_cells() {
        let mut r = range();
        r.units.push(RangeUnit::new([200, 16, 200]));
        let b = &r.units[0].body;
        let mut cells = Vec::new();
        for x in 0..b.vol.nx {
            for y in 0..b.vol.ny {
                for z in 0..b.vol.nz {
                    let p = Cell {
                        unit: 0,
                        part: 0,
                        at: [x, y, z],
                    };
                    if p.body(&r.units).is_some() {
                        cells.push(p);
                    }
                }
            }
        }
        assert!(cells.len() >= 600);
        let mut initial: BTreeMap<_, _> = cells
            .iter()
            .step_by(2)
            .take(480)
            .enumerate()
            .map(|(i, &p)| {
                (
                    p,
                    if i % 5 == 0 {
                        HEAT_CAP
                    } else {
                        (i as u32 * 7919) % 50000 + 1
                    },
                )
            })
            .collect();
        for unit in 0..2 {
            let b = &r.units[unit].body.segments[0].body;
            let mut count = 0;
            'segment: for x in 0..b.vol.nx {
                for y in 0..b.vol.ny {
                    for z in 0..b.vol.nz {
                        let p = Cell {
                            unit: unit as u16,
                            part: 1,
                            at: [x, y, z],
                        };
                        if p.body(&r.units).is_some() {
                            initial.insert(p, HEAT_CAP / (count + 1));
                            count += 1;
                            if count == 60 {
                                break 'segment;
                            }
                        }
                    }
                }
            }
            assert_eq!(count, 60);
        }
        assert_eq!(initial.len(), 600);
        for &p in initial.keys().step_by(11) {
            let [x, y, z] = p.at.map(usize::from);
            let u = &mut r.units[usize::from(p.unit)];
            if p.part == 0 {
                u.body.remove(x, y, z);
            } else {
                u.body.segments[usize::from(p.part - 1)]
                    .body
                    .remove(x, y, z);
            }
        }
        r.units[1].seg_gone[0] = true;
        for workers in [1, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for limit in [7, 256, 512, 600] {
                for capacity in [600, 601, 610, 16384] {
                    for (cooling, conduction) in [(0, 0), (0, 150), (8, 8), (1000, 150)] {
                        for cursor in [None, initial.keys().nth(311).copied()] {
                            let mut tuning = r.thermal_tuning.clone();
                            tuning.cooling_milli = cooling;
                            tuning.conduction_milli = conduction;
                            tuning.max_cells = capacity;
                            let mut expected = Thermals {
                                cells: initial.clone(),
                                ..Default::default()
                            };
                            let batch = expected.batch_after(cursor, limit);
                            let wanted = old_delta(&mut expected, &batch, &r.units, &tuning);
                            for parallel in [false, true] {
                                let mut actual = Thermals {
                                    cells: initial.clone(),
                                    ..Default::default()
                                };
                                let got = pool.install(|| {
                                    actual.conduction_delta(
                                        &batch,
                                        &r.units,
                                        &tuning,
                                        parallel,
                                        &|_, _| {},
                                    )
                                });
                                assert_eq!(
                                    got, wanted,
                                    "workers={workers} limit={limit} capacity={capacity}"
                                );
                                assert_eq!(actual.cells, expected.cells);
                            }
                        }
                    }
                }
            }
        }
    }
    fn range() -> BlastRange {
        let c = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/flamethrower.ron"
        ))
        .unwrap();
        let mut r = BlastRange::new(c);
        r.control();
        r
    }
    fn cell(r: &BlastRange, role: Part) -> Cell {
        let b = &r.units[0].body;
        for z in 0..b.vol.nz {
            for y in 0..b.vol.ny {
                for x in 0..b.vol.nx {
                    if b.vol.get(x as usize, y as usize, z as usize)
                        && b.part_at(x as usize, y as usize, z as usize) == role
                    {
                        return Cell {
                            unit: 0,
                            part: 0,
                            at: [x, y, z],
                        };
                    }
                }
            }
        }
        panic!("missing test role");
    }
    fn energy(r: &BlastRange) -> u64 {
        r.thermals.cells().map(|(_, h)| u64::from(h)).sum()
    }
    #[test]
    fn conduction_conserves_energy_and_moves_only_to_solid_neighbours() {
        let mut r = range();
        r.thermal_tuning.cooling_milli = 0;
        r.thermal_tuning.softening = [100000; 5];
        let p = cell(&r, Part::Armour);
        let near: Vec<_> = p
            .neighbours()
            .filter(|q| q.body(&r.units).is_some())
            .collect();
        assert!(!near.is_empty());
        r.thermals.deposit(p, 600, &r.thermal_tuning);
        r.step_unit_heat();
        assert_eq!(energy(&r), 600);
        assert!(near.iter().any(|q| r.thermals.heat(*q) > 0));
        assert!(r.thermals.cells().all(|(q, _)| q == p || near.contains(&q)));
        for _ in 0..80 {
            r.step_unit_heat();
        }
        assert_eq!(energy(&r), 600);
        assert_eq!(r.thermals.melted, 0);
        assert_eq!(temperature(1000, 200), 125);
    }
    #[test]
    fn short_heating_cools_without_cutting_armour() {
        let mut r = range();
        let p = cell(&r, Part::Armour);
        r.thermals.deposit(p, 100, &r.thermal_tuning);
        for _ in 0..400 {
            r.step_unit_heat();
        }
        assert_eq!(r.thermals.hot(), 0);
        assert_eq!(r.units[0].revision, 0);
    }
    #[test]
    fn engine_fails_before_armour_and_matter_falls_without_blast_impulse() {
        let mut r = range();
        r.thermal_tuning.cooling_milli = 0;
        r.thermal_tuning.conduction_milli = 0;
        let armour = cell(&r, Part::Armour);
        let engine = cell(&r, Part::Engine);
        let held = r.matter.body_container(0);
        let before = r.matter.economy.ledger.get(held).held;
        r.thermals.deposit(armour, 250, &r.thermal_tuning);
        r.thermals.deposit(engine, 250, &r.thermal_tuning);
        r.step_unit_heat();
        assert!(armour.body(&r.units).is_some());
        assert!(engine.body(&r.units).is_none());
        assert_eq!(r.thermals.melted, 1);
        let f = r.matter.debris.fragments.last().unwrap();
        assert_eq!(f.velocity, phys::V3::ZERO);
        assert_eq!(
            r.matter.economy.ledger.get(held).held + r.matter.economy.ledger.get(f.held).held,
            before
        );
        assert_eq!(r.units[0].annihilated_mm3(), 0);
    }
    #[test]
    fn full_fragment_pool_defers_melting_without_erasing_heat_or_matter() {
        let mut r = range();
        let p = cell(&r, Part::Engine);
        r.thermal_tuning.cooling_milli = 0;
        r.thermal_tuning.conduction_milli = 0;
        r.thermals.deposit(p, 500, &r.thermal_tuning);
        let cap = r.matter.tuning.max_fragments;
        r.matter.tuning.max_fragments = 0;
        r.step_unit_heat();
        assert!(p.body(&r.units).is_some());
        assert_eq!(r.thermals.heat(p), 500);
        assert_eq!(
            r.matter.admission.melt,
            [0, 1],
            "full-pool melt must be observable"
        );
        r.matter.tuning.max_fragments = cap;
        r.step_unit_heat();
        assert!(p.body(&r.units).is_none());
    }
    #[test]
    fn melting_consumes_last_slot_and_retains_the_next_hot_cell() {
        let mut r = range();
        r.thermal_tuning.cooling_milli = 0;
        r.thermal_tuning.conduction_milli = 0;
        r.thermal_tuning.softening = [1; 5];
        let a = cell(&r, Part::Armour);
        let b = cell(&r, Part::Engine);
        for p in [a, b] {
            r.thermals.deposit(p, 1000, &r.thermal_tuning);
        }
        let before = r.units[0].body.solid_count();
        r.matter.tuning.max_fragments = 1;
        r.matter.tuning.max_flying = 1;
        r.step_unit_heat();
        assert_eq!(r.thermals.melted, 1);
        assert_eq!(r.matter.debris.fragments.len(), 1);
        assert_eq!(r.units[0].body.solid_count(), before - 1);
        let (first, waiting) = if a < b { (a, b) } else { (b, a) };
        assert!(first.body(&r.units).is_none());
        assert!(waiting.body(&r.units).is_some());
        assert_eq!(r.thermals.heat(waiting), 1000);
        assert!(r.matter.economy.ledger.balanced());
        r.matter.tuning.max_fragments = 2;
        r.matter.tuning.max_flying = 2;
        r.step_unit_heat();
        assert_eq!(r.thermals.melted, 2);
        assert!(waiting.body(&r.units).is_none());
        assert_eq!(r.thermals.hot(), 0);
        assert!(r.matter.economy.ledger.balanced());
    }
    #[test]
    fn hot_armour_reservation_blocks_terrain_without_increasing_physical_capacity() {
        let mut r = range();
        let p = cell(&r, Part::Engine);
        r.thermal_tuning.cooling_milli = 0;
        r.thermal_tuning.conduction_milli = 0;
        r.thermals.deposit(p, 500, &r.thermal_tuning);
        r.matter.tuning.max_flying = 1;
        r.matter.tuning.max_fragments = 1;
        r.reserve_thermal_fragments();
        assert_eq!(r.matter.claim_room(), 0);
        assert!(!r.matter.melt(&mut r.world, [40, 3, 40]));
        assert_eq!(r.matter.debris.claims(), 0);
        r.step_unit_heat();
        assert!(p.body(&r.units).is_none());
        assert_eq!(r.matter.debris.fragments.len(), 1);
        assert_eq!(r.matter.debris.fragment_room(&r.matter.tuning), 0);
        assert!(r.matter.economy.ledger.balanced());
        assert_eq!(r.matter.tuning.max_flying, 1);
    }
    #[test]
    fn warm_smoking_armour_does_not_reserve_projectile_capacity() {
        let mut r = range();
        let p = cell(&r, Part::Armour);
        r.thermals
            .deposit(p, r.thermal_tuning.smoke, &r.thermal_tuning);
        r.matter.tuning.max_flying = 1;
        r.matter.tuning.max_fragments = 1;
        r.reserve_thermal_fragments();
        assert_eq!(r.matter.claim_room(), 1);
        assert!(r.matter.melt(&mut r.world, [40, 3, 40]));
    }
    #[test]
    fn sleeping_debris_cannot_take_the_next_hot_armour_slot() {
        let mut r = range();
        let p = cell(&r, Part::Engine);
        r.thermal_tuning.cooling_milli = 0;
        r.thermal_tuning.conduction_milli = 0;
        assert!(r.matter.melt(&mut r.world, [40, 3, 40]));
        r.world.flush();
        r.matter
            .debris
            .confirm(r.world.completed(), &mut r.matter.economy, &r.matter.tuning);
        assert_eq!(r.matter.debris.fragments.len(), 1);
        let f = &mut r.matter.debris.fragments[0];
        f.resting = true;
        f.position.y = Fx::from_int(80); // Unsupported pile waiting to wake.
        r.matter.tuning.max_flying = 1;
        r.matter.tuning.max_fragments = 2;
        r.thermals.deposit(p, 500, &r.thermal_tuning);
        for tick in 0..60 {
            r.reserve_thermal_fragments();
            r.world.flush();
            r.matter.flush(&r.world, true, 98, tick, &[], &[]);
            assert_eq!(r.matter.debris.fragment_pools(&r.matter.tuning).flying, 0);
        }
        assert_eq!(r.matter.debris.wakeups, 0);
        r.step_unit_heat();
        assert!(p.body(&r.units).is_none());
        assert_eq!(r.matter.debris.fragment_pools(&r.matter.tuning).flying, 1);
        assert!(r.matter.economy.ledger.balanced());
    }
    #[test]
    fn thermal_budget_rotates_and_full_field_keeps_existing_heat() {
        let mut r = range();
        let p = cell(&r, Part::Engine);
        r.thermal_tuning.max_cells = 1;
        r.thermal_tuning.cells_per_tick = 1;
        r.thermal_tuning.cooling_milli = 0;
        r.thermal_tuning.softening = [100000; 5];
        r.thermals.deposit(p, 500, &r.thermal_tuning);
        let q = p.neighbours().find(|q| q.body(&r.units).is_some()).unwrap();
        r.thermals.deposit(q, 100, &r.thermal_tuning);
        r.step_unit_heat();
        assert_eq!(r.thermals.hot(), 1);
        assert_eq!(energy(&r), 500);
        assert_eq!(r.thermals.rejected_heat, 100);
        r.thermal_tuning.max_cells = 10;
        r.thermal_tuning.conduction_milli = 0;
        r.thermal_tuning.cooling_milli = 100;
        r.thermals.deposit(q, 500, &r.thermal_tuning);
        r.step_unit_heat();
        r.step_unit_heat();
        assert_eq!(r.thermals.heat(p), 450);
        assert_eq!(r.thermals.heat(q), 450);
    }
    #[test]
    fn moving_segment_retains_local_heat_and_snapshot_replays_cooling() {
        let mut a = range();
        let seg = &a.units[0].body.segments[0].body;
        let mut p = Cell {
            unit: 0,
            part: 1,
            at: [0; 3],
        };
        'outer: for z in 0..seg.vol.nz {
            for y in 0..seg.vol.ny {
                for x in 0..seg.vol.nx {
                    if seg.vol.get(x as usize, y as usize, z as usize) {
                        p.at = [x, y, z];
                        break 'outer;
                    }
                }
            }
        }
        let before = p.world(&a.units).unwrap();
        a.thermals.deposit(p, 250, &a.thermal_tuning);
        a.units[0].motion.pos[0] = a.units[0].motion.pos[0].add(Fx::from_int(20));
        a.units[0].update_pose(0);
        assert_eq!(
            p.world(&a.units).unwrap()[0].sub(before[0]),
            Fx::from_int(20)
        );
        assert_eq!(a.thermals.heat(p), 250);
        for _ in 0..3 {
            a.step();
        }
        assert!(a.thermals.hot() > 0);
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        for _ in 0..75 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
    }
    #[test]
    fn version_31_restores_a_cold_field() {
        let mut r = range();
        r.matter = Matter::new(&r.units);
        r.combustion = super::super::fire::Combustion::empty(r.units.len());
        let mut data = r.snapshot();
        let mut tail = Blob::new();
        r.save_after(&mut tail, 31);
        data.truncate(data.len() - tail.v.len());
        data[..4].copy_from_slice(&31u32.to_le_bytes());
        let restored = BlastRange::restore(r.config.clone(), &data).unwrap();
        assert_eq!(restored.thermals.hot(), 0);
        assert_eq!(r.state_hash(), restored.state_hash());
    }
}
