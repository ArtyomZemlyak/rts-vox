//! Authoritative heat and secondary combustion (ADR-277).
//!
//! Heat lives on fuel cells, in the wave units the weapons declare as
//! `energy`; without conduction, heat on anything that cannot burn would
//! be bookkeeping with no consequence, so stone and soil hold none. A burst
//! deposits heat on the fuel within its reach, most at the centre and none
//! at the edge; heat cools by a share per tick; a voxel that reaches its
//! kindling lights, feeds its neighbours for its burn time and is then
//! consumed. Work is bounded per tick and visited in cell order from a
//! cursor, so a large fire only burns slower, never faster than the budget.
//! The image of fire and smoke reads these events; it decides nothing.
use fx::blob::{Blob, Scan};
use modding::heat::HeatTuning;
use modding::Registry;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use voxel::{store::AIR, World};

/// The edit slot fire digs by: the ledger sends what it took to the
/// combustion sink instead of crediting a unit.
pub const FIRE: u32 = u32::MAX - 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeatCell {
    pub heat: u32,
    /// Ticks of burning left; zero while the voxel merely stands hot.
    pub burn: u16,
}

#[derive(Clone, Debug, Default)]
pub struct Heat {
    cells: BTreeMap<[u32; 3], HeatCell>,
    cursor: Option<[u32; 3]>,
    /// Voxels lit this tick, for the image of fire; transient.
    pub ignited: Vec<[u32; 3]>,
    /// Voxels consumed this tick; the ledger sinks their matter. Transient.
    pub burned: BTreeSet<[u32; 3]>,
    pub burned_total: u64,
    pub burned_by_material: [u64; 16],
    /// Voxels at their melting point this tick, for the range to turn into
    /// debris (ADR-290). Transient; a cell the pool cannot take stays hot.
    pub melting: Vec<[u32; 3]>,
    pub melted_total: u64,
}

const HEAT_CAP: u32 = 1_000_000;

impl Heat {
    /// Ordered bounded source slice for range contact heat. No full-map scan.
    pub(crate) fn batch_after(&self, cursor: Option<[u32; 3]>, limit: usize) -> Vec<[u32; 3]> {
        use std::ops::Bound::{Excluded, Unbounded};
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
    /// Exchange spends stored heat, never burn time or fuel.
    pub(crate) fn take_heat(&mut self, p: [u32; 3], amount: u32) {
        if let Some(c) = self.cells.get_mut(&p) {
            c.heat = c.heat.saturating_sub(amount);
            if c.heat == 0 && c.burn == 0 {
                self.cells.remove(&p);
            }
        }
    }
    pub fn hot(&self) -> usize {
        self.cells.len()
    }
    /// Forecast only this field's next bounded source slice, using saved heat
    /// rather than the transient `melting` list from a previous tick.
    pub(crate) fn melting_slots(
        &self,
        world: &World,
        mats: &Registry,
        work: usize,
        limit: usize,
        claimed: impl Fn([u32; 3]) -> bool,
    ) -> usize {
        if limit == 0 || self.cells.is_empty() {
            return 0;
        }
        let start = self.cursor.unwrap_or([u32::MAX; 3]);
        // Terrain heat's cursor is the next source, inclusive. Unit heat and
        // exchange instead save their last processed source.
        self.cells
            .range(start..)
            .chain(self.cells.range(..start))
            .take(work.max(1))
            .filter(|&(p, c)| {
                c.burn == 0
                    && !claimed(*p)
                    && !world.on_frame(p[0], p[1], p[2])
                    && mats
                        .get(world.material(p[0], p[1], p[2]))
                        .is_some_and(|m| m.melt > 0 && c.heat >= m.melt)
            })
            .take(limit)
            .count()
    }
    pub fn burning(&self) -> usize {
        self.cells.values().filter(|c| c.burn > 0).count()
    }
    /// Every voxel alight now, in the map's order: the picture draws a
    /// flame where the heat says one burns (P-22). Read-only.
    pub fn burning_cells(&self) -> impl Iterator<Item = [u32; 3]> + '_ {
        self.cells
            .iter()
            .filter(|(_, c)| c.burn > 0)
            .map(|(p, _)| *p)
    }
    pub fn cell(&self, p: [u32; 3]) -> HeatCell {
        self.cells.get(&p).copied().unwrap_or_default()
    }
    pub fn hottest(&self) -> Option<([u32; 3], HeatCell)> {
        self.cells
            .iter()
            .max_by_key(|(p, c)| (c.heat, std::cmp::Reverse(**p)))
            .map(|(p, c)| (*p, *c))
    }
    /// Heat on one solid cell; air holds none.
    pub fn add(&mut self, w: &World, p: [u32; 3], heat: u32) {
        if heat == 0 || !inside(w, p) || !w.solid(p[0], p[1], p[2]) {
            return;
        }
        let c = self.cells.entry(p).or_default();
        c.heat = c.heat.saturating_add(heat).min(HEAT_CAP);
    }
    /// A burst: fuel within `radius` of `at` receives up to `per_cell`,
    /// falling off with the square of the distance to nothing at the edge.
    ///
    /// The shares are gathered in the map's own key order and join it in
    /// one merge (ADR-461). A shell over the concrete floor heats some
    /// forty thousand meltable cells, and inserting them one at a time
    /// cost seven milliseconds of the impact tick. Every cell still
    /// receives one share, so how they join decides nothing.
    pub fn deposit(
        &mut self,
        w: &World,
        mats: &Registry,
        at: [i32; 3],
        radius: i32,
        per_cell: u32,
    ) {
        if radius <= 0 || per_cell == 0 {
            return;
        }
        let r2 = radius * radius;
        let mut cursor = World::mat_cursor();
        let mut takes = (AIR, false);
        let mut shares: Vec<([u32; 3], u32)> = Vec::new();
        // The key orders by x, then y, then z: so does the walk.
        for x in (at[0] - radius).max(0)..(at[0] + radius + 1).min(w.nx as i32) {
            let x2 = (x - at[0]).pow(2);
            for y in (at[1] - radius).max(0)..(at[1] + radius + 1).min(w.ny as i32) {
                let xy2 = x2 + (y - at[1]).pow(2);
                if xy2 >= r2 {
                    continue;
                }
                let reach = (r2 - xy2 - 1).isqrt();
                for z in (at[2] - reach).max(0)..(at[2] + reach + 1).min(w.nz as i32) {
                    let p = [x as u32, y as u32, z as u32];
                    if !w.solid(p[0], p[1], p[2]) {
                        continue;
                    }
                    let m = w.material_cached(&mut cursor, p[0], p[1], p[2]);
                    if takes.0 != m {
                        takes = (
                            m,
                            m != AIR && mats.get(m).is_some_and(|d| d.kindling > 0 || d.melt > 0),
                        );
                    }
                    if !takes.1 {
                        continue;
                    }
                    let d2 = xy2 + (z - at[2]).pow(2);
                    let share = (u64::from(per_cell) * (r2 - d2) as u64 / r2 as u64) as u32;
                    if share > 0 {
                        shares.push((p, share));
                    }
                }
            }
        }
        // A few shares into a large ledger go one at a time; otherwise the
        // two ordered runs merge in one pass and the tree is built once.
        if shares.len().saturating_mul(8) < self.cells.len() {
            for (p, h) in shares {
                let c = self.cells.entry(p).or_default();
                c.heat = c.heat.saturating_add(h).min(HEAT_CAP);
            }
            return;
        }
        let cell = |heat: u32| HeatCell {
            heat: heat.min(HEAT_CAP),
            ..HeatCell::default()
        };
        let mut hot = std::mem::take(&mut self.cells).into_iter().peekable();
        let mut new = shares.into_iter().peekable();
        let mut merged = Vec::with_capacity(hot.len() + new.len());
        loop {
            let order = match (hot.peek(), new.peek()) {
                (Some((a, _)), Some((b, _))) => a.cmp(b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => break,
            };
            match order {
                std::cmp::Ordering::Less => merged.extend(hot.next()),
                std::cmp::Ordering::Greater => {
                    merged.extend(new.next().map(|(p, h)| (p, cell(h))));
                }
                std::cmp::Ordering::Equal => {
                    let (p, mut c) = hot.next().expect("peeked");
                    let (_, h) = new.next().expect("peeked");
                    c.heat = c.heat.saturating_add(h).min(HEAT_CAP);
                    merged.push((p, c));
                }
            }
        }
        self.cells = merged.into_iter().collect();
    }
    /// One tick over at most `cells_per_tick` hot cells. `claimed` names
    /// cells another process already owns; they wait untouched.
    pub fn step(
        &mut self,
        w: &mut World,
        mats: &Registry,
        t: &HeatTuning,
        claimed: impl Fn([u32; 3]) -> bool,
    ) {
        self.step_observed(w, mats, t, claimed, &|_, _| {});
    }
    pub(crate) fn step_observed(
        &mut self,
        w: &mut World,
        mats: &Registry,
        t: &HeatTuning,
        claimed: impl Fn([u32; 3]) -> bool,
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        self.step_in_observed(w, mats, t, claimed, (true, task));
    }
    #[cfg(test)]
    fn step_in(
        &mut self,
        w: &mut World,
        mats: &Registry,
        t: &HeatTuning,
        claimed: impl Fn([u32; 3]) -> bool,
        parallel: bool,
    ) {
        self.step_in_observed(w, mats, t, claimed, (parallel, &|_, _| {}));
    }
    fn step_in_observed(
        &mut self,
        w: &mut World,
        mats: &Registry,
        t: &HeatTuning,
        claimed: impl Fn([u32; 3]) -> bool,
        observations: (bool, &(impl Fn(usize, &'static str) + Sync)),
    ) {
        let (parallel, task) = observations;
        let _total = crate::burst::PhysicsTask::new(
            task,
            "physics_ground_heat_begin",
            "physics_ground_heat_end",
        );
        self.ignited.clear();
        self.burned.clear();
        self.melting.clear();
        if self.cells.is_empty() {
            self.cursor = None;
            return;
        }
        let budget = t.cells_per_tick.max(1);
        // Capture each old value during the ordered range walk instead of
        // looking it up in the tree again for both gain and cooling passes.
        // Gains are still committed after reading all burning sources.
        let mut batch: Vec<([u32; 3], HeatCell)> = match self.cursor {
            Some(c) => self
                .cells
                .range(c..)
                .map(|(k, c)| (*k, *c))
                .take(budget)
                .collect(),
            None => Vec::new(),
        };
        let first_len = batch.len();
        if batch.len() < budget {
            let bound = self.cursor.unwrap_or([u32::MAX; 3]);
            batch.extend(
                self.cells
                    .range(..bound)
                    .map(|(k, c)| (*k, *c))
                    .take(budget - batch.len()),
            );
        }
        // The world is frozen throughout this phase: burning only queues
        // edits. Read each material and a burning cell's solid neighbours
        // independently; gains and bounded heat admission remain ordered.
        let read = |&(p, c): &([u32; 3], HeatCell), material| {
            let share = if c.burn == 0 {
                0
            } else {
                mats.get(material)
                    .map_or(0, |d| d.fuel / u32::from(t.burn_ticks.max(1)))
            };
            let mut solid = 0u8;
            if share > 0 {
                for (bit, q) in neighbours(w, p) {
                    if w.solid(q[0], q[1], q[2]) {
                        solid |= 1 << bit;
                    }
                }
            }
            (material, share, solid)
        };
        let reads: Vec<_> = if parallel && batch.len() >= 1024 {
            let mut reads = vec![(AIR, 0, 0); batch.len()];
            // Each disjoint source/output block owns its chunk cursor.
            // Reuse never crosses a phase or observes edits queued below.
            batch
                .par_chunks(64)
                .zip(reads.par_chunks_mut(64))
                .for_each(|(sources, outputs)| {
                    let _task = crate::burst::PhysicsTask::new(
                        task,
                        "physics_ground_heat_read_begin",
                        "physics_ground_heat_read_end",
                    );
                    let mut cursor = World::mat_cursor();
                    for (source, output) in sources.iter().zip(outputs) {
                        let p = source.0;
                        let material = w.material_cached(&mut cursor, p[0], p[1], p[2]);
                        *output = read(source, material);
                    }
                });
            reads
        } else {
            let _task = crate::burst::PhysicsTask::new(
                task,
                "physics_ground_heat_read_begin",
                "physics_ground_heat_read_end",
            );
            batch
                .iter()
                .map(|source| {
                    let p = source.0;
                    read(source, w.material(p[0], p[1], p[2]))
                })
                .collect()
        };
        // Burning cells feed their solid neighbours; gains are gathered
        // first so the order of the batch cannot favour a direction.
        let _commit = crate::burst::PhysicsTask::new(
            task,
            "physics_ground_heat_commit_begin",
            "physics_ground_heat_commit_end",
        );
        let mut gain: BTreeMap<[u32; 3], u32> = BTreeMap::new();
        for (&(p, _), &(_, share, solid)) in batch.iter().zip(&reads) {
            if solid == 0 {
                continue;
            }
            for bit in 0..6 {
                if solid & (1 << bit) == 0 {
                    continue;
                }
                let mut q = p;
                if bit % 2 == 0 {
                    q[bit / 2] -= 1;
                } else {
                    q[bit / 2] += 1;
                }
                *gain.entry(q).or_insert(0) += share;
            }
        }
        // Preserve the former tree mutation order as an independent oracle.
        #[cfg(test)]
        if !parallel {
            let mut last = None;
            for ((p, mut c), (m, _, _)) in batch.into_iter().zip(reads) {
                last = Some(p);
                if m == AIR {
                    // Taken by something else: its heat went with it.
                    self.cells.remove(&p);
                    continue;
                }
                if claimed(p) {
                    continue;
                }
                c.heat = c
                    .heat
                    .saturating_add(gain.remove(&p).unwrap_or(0))
                    .min(HEAT_CAP);
                let def = mats.get(m).copied();
                if c.burn > 0 {
                    c.burn -= 1;
                    if c.burn == 0 {
                        w.queue_dig_by(p[0], p[1], p[2], FIRE);
                        self.burned.insert(p);
                        self.burned_total += 1;
                        self.burned_by_material[usize::from(m) & 15] += 1;
                        self.cells.remove(&p);
                        continue;
                    }
                } else if let Some(d) = def {
                    // Judged on the heat reached, before this tick's cooling: a
                    // deposit equal to the kindling lights the voxel.
                    if d.kindling > 0 && d.fuel > 0 && c.heat >= d.kindling {
                        c.burn = t.burn_ticks;
                        self.ignited.push(p);
                    }
                    // A meltable voxel at its point gives way (ADR-290); the
                    // range takes it as debris and forgets its heat.
                    if d.melt > 0 && c.heat >= d.melt {
                        self.melting.push(p);
                    }
                }
                if c.heat > 0 {
                    let lost = (c.heat * t.cooling_milli / 1000).max(1);
                    c.heat = c.heat.saturating_sub(lost);
                }
                if c.heat == 0 && c.burn == 0 {
                    self.cells.remove(&p);
                } else {
                    self.cells.insert(p, c);
                }
            }
            for (q, g) in gain {
                let c = self.cells.entry(q).or_default();
                c.heat = c.heat.saturating_add(g).min(HEAT_CAP);
            }
            self.cursor = last.and_then(|l| {
                self.cells
                    .range((std::ops::Bound::Excluded(l), std::ops::Bound::Unbounded))
                    .next()
                    .map(|(k, _)| *k)
            });
            return;
        }
        // The batch follows two disjoint key ranges. Mutate their existing
        // entries by one ordered walk, rather than searching the tree again
        // for every update. Deletions wait until both ranges have been visited;
        // neighbour gains are still admitted after the entire batch.
        let batch_len = batch.len();
        let Heat {
            cells,
            cursor,
            ignited,
            burned,
            burned_total,
            burned_by_material,
            melting,
            ..
        } = self;
        let mut inputs = reads.into_iter();
        let mut removed = Vec::new();
        let mut last = None;
        let mut commit = |p: &[u32; 3], entry: &mut HeatCell| {
            let (m, _, _) = inputs.next().expect("one read per batch entry");
            let p = *p;
            last = Some(p);
            if m == AIR {
                removed.push(p);
                return;
            }
            if claimed(p) {
                return;
            }
            let mut c = *entry;
            c.heat = c
                .heat
                .saturating_add(gain.remove(&p).unwrap_or(0))
                .min(HEAT_CAP);
            let def = mats.get(m).copied();
            if c.burn > 0 {
                c.burn -= 1;
                if c.burn == 0 {
                    w.queue_dig_by(p[0], p[1], p[2], FIRE);
                    burned.insert(p);
                    *burned_total += 1;
                    burned_by_material[usize::from(m) & 15] += 1;
                    removed.push(p);
                    return;
                }
            } else if let Some(d) = def {
                if d.kindling > 0 && d.fuel > 0 && c.heat >= d.kindling {
                    c.burn = t.burn_ticks;
                    ignited.push(p);
                }
                if d.melt > 0 && c.heat >= d.melt {
                    melting.push(p);
                }
            }
            if c.heat > 0 {
                let lost = (c.heat * t.cooling_milli / 1000).max(1);
                c.heat = c.heat.saturating_sub(lost);
            }
            if c.heat == 0 && c.burn == 0 {
                removed.push(p);
            } else {
                *entry = c;
            }
        };
        if let Some(start) = *cursor {
            for (p, entry) in cells.range_mut(start..).take(first_len) {
                commit(p, entry);
            }
        }
        let bound = cursor.unwrap_or([u32::MAX; 3]);
        for (p, entry) in cells.range_mut(..bound).take(batch_len - first_len) {
            commit(p, entry);
        }
        debug_assert!(inputs.next().is_none());
        for p in removed {
            cells.remove(&p);
        }
        for (q, g) in gain {
            let c = cells.entry(q).or_default();
            c.heat = c.heat.saturating_add(g).min(HEAT_CAP);
        }
        *cursor = last.and_then(|l| {
            cells
                .range((std::ops::Bound::Excluded(l), std::ops::Bound::Unbounded))
                .next()
                .map(|(k, _)| *k)
        });
    }
    /// A voxel the range took: its heat went with it.
    pub fn forget(&mut self, p: [u32; 3]) {
        self.cells.remove(&p);
    }
    pub fn save(&self, b: &mut Blob) {
        b.u32(self.cells.len() as u32);
        for (p, c) in &self.cells {
            for v in p {
                b.u32(*v);
            }
            b.u32(c.heat);
            b.u32(u32::from(c.burn));
        }
        b.bool(self.cursor.is_some());
        if let Some(c) = self.cursor {
            for v in c {
                b.u32(v);
            }
        }
        b.u64(self.burned_total);
        for n in self.burned_by_material {
            b.u64(n);
        }
        b.u64(self.melted_total);
    }
    pub fn load(s: &mut Scan, w: &World, t: &HeatTuning) -> Option<Self> {
        let mut h = Self::default();
        let n = s.u32() as usize;
        if n as u64 > cell_count(w.nx, w.ny, w.nz) {
            return None;
        }
        let mut previous = None;
        for _ in 0..n {
            let p = [s.u32(), s.u32(), s.u32()];
            let heat = s.u32();
            let burn = s.u32();
            if !inside(w, p)
                || heat > HEAT_CAP
                || burn > u32::from(t.burn_ticks)
                || (heat == 0 && burn == 0)
                || previous.is_some_and(|q| q >= p)
            {
                return None;
            }
            previous = Some(p);
            h.cells.insert(
                p,
                HeatCell {
                    heat,
                    burn: burn as u16,
                },
            );
        }
        if s.bool() {
            let c = [s.u32(), s.u32(), s.u32()];
            if !inside(w, c) {
                return None;
            }
            h.cursor = Some(c);
        }
        h.burned_total = s.u64();
        h.burned_by_material = std::array::from_fn(|_| s.u64());
        h.melted_total = s.u64();
        s.ok().then_some(h)
    }
}

/// Число ячеек мира в `u64`: у километровой карты 10000×192×10000
/// оно за пределом `u32`, и произведение в `u32` паниковало при
/// загрузке любого её снимка — `overflow-checks` включены во всех
/// профилях.
fn cell_count(nx: u32, ny: u32, nz: u32) -> u64 {
    u64::from(nx) * u64::from(ny) * u64::from(nz)
}
fn inside(w: &World, p: [u32; 3]) -> bool {
    p[0] < w.nx && p[1] < w.ny && p[2] < w.nz
}
fn neighbours(w: &World, p: [u32; 3]) -> impl Iterator<Item = (usize, [u32; 3])> + '_ {
    (0..6).filter_map(move |bit| {
        let a = bit / 2;
        let mut q = p;
        if bit % 2 == 0 {
            if p[a] == 0 {
                return None;
            }
            q[a] -= 1;
        } else {
            q[a] += 1;
            if !inside(w, q) {
                return None;
            }
        }
        Some((bit, q))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Объём километровой карты не помещается в `u32`.
    #[test]
    fn the_kilometre_map_volume_is_counted_without_overflow() {
        assert_eq!(cell_count(10_000, 192, 10_000), 19_200_000_000);
        assert!(cell_count(10_000, 192, 10_000) > u64::from(u32::MAX));
    }
    #[test]
    fn occupied_rows_deposit_the_same_heat_as_the_voxel_reference() {
        let mut w = World::new(96, 64, 96, 100, Box::new(Flat));
        w.queue_dig(31, 18, 31);
        w.queue_edit(voxel::Edit::put(65, 21, 32, 3));
        w.flush();
        let registry = mats();
        // Bursts one after another into the same ledger: the first joins an
        // empty map, the rest merge with cells already hot (ADR-461).
        let mut reference = Heat::default();
        let mut actual = Heat::default();
        for at in [
            [32, 20, 32],
            [64, 20, 32],
            [-4, 8, 1],
            [92, 63, 94],
            [40, 18, 30],
        ] {
            let radius: i32 = 32;
            let r2 = radius * radius;
            for z in at[2] - radius..=at[2] + radius {
                for y in at[1] - radius..=at[1] + radius {
                    for x in at[0] - radius..=at[0] + radius {
                        let d2 = (x - at[0]).pow(2) + (y - at[1]).pow(2) + (z - at[2]).pow(2);
                        if d2 >= r2 || x < 0 || y < 0 || z < 0 {
                            continue;
                        }
                        let p = [x as u32, y as u32, z as u32];
                        if !inside(&w, p) {
                            continue;
                        }
                        let m = w.material(p[0], p[1], p[2]);
                        if m != AIR
                            && registry
                                .get(m)
                                .is_some_and(|d| d.kindling > 0 || d.melt > 0)
                        {
                            reference.add(&w, p, (800u64 * (r2 - d2) as u64 / r2 as u64) as u32);
                        }
                    }
                }
            }
            actual.deposit(&w, &registry, at, radius, 800);
            assert_eq!(actual.cells, reference.cells, "deposit at {at:?}");
        }
    }
    struct Flat;
    impl voxel::Generator for Flat {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            if y < 16 {
                1
            } else if (30..34).contains(&x) && (30..34).contains(&z) && y < 20 {
                // A block of fuel on the ground.
                4
            } else {
                0
            }
        }
    }
    fn tuning() -> HeatTuning {
        HeatTuning::from_ron(include_str!("../../../data/heat.ron")).unwrap()
    }
    fn mats() -> Registry {
        modding::data::materials_from_ron(include_str!("../../../data/range-materials.ron"))
            .unwrap()
    }
    #[test]
    fn melt_reservation_forecast_uses_the_inclusive_saved_cursor_and_bounded_work() {
        let mut w = World::new(64, 64, 64, 100, Box::new(Flat));
        let mut heat = Heat::default();
        for x in 5..8 {
            w.queue_edit(voxel::Edit::put(x, 20, 5, 3));
        }
        w.flush();
        for x in 5..8 {
            heat.add(&w, [x, 20, 5], if x == 6 { 1000 } else { 1 });
        }
        heat.cursor = Some([6, 20, 5]);
        let registry = mats();
        assert_eq!(heat.melting_slots(&w, &registry, 1, 1, |_| false), 1);
        assert_eq!(heat.melting_slots(&w, &registry, 1, 1, |_| true), 0);
        assert_eq!(heat.melting_slots(&w, &registry, 1, 0, |_| false), 0);
        let mut saved = Blob::new();
        heat.save(&mut saved);
        let restored = Heat::load(&mut Scan::new(&saved.v), &w, &tuning()).unwrap();
        assert_eq!(restored.melting_slots(&w, &registry, 1, 1, |_| false), 1);
        let mut actual = heat.clone();
        let mut t = tuning();
        t.cells_per_tick = 1;
        actual.step(&mut w, &registry, &t, |_| false);
        assert_eq!(actual.melting, vec![[6, 20, 5]]);
        heat.cursor = Some([7, 20, 5]);
        assert_eq!(heat.melting_slots(&w, &registry, 2, 1, |_| false), 0);
        assert_eq!(heat.melting_slots(&w, &registry, 3, 1, |_| false), 1);
    }
    #[test]
    fn parallel_world_reads_and_ordered_commit_match_reference_across_workers() {
        struct Grid;
        impl voxel::Generator for Grid {
            fn material(&self, x: u32, y: u32, z: u32) -> u16 {
                if y < 15 {
                    1
                } else {
                    [0, 3, 4, 5, 4, 5, 17][((x + y + z) % 7) as usize]
                }
            }
        }
        let registry = mats();
        let mut initial = Heat::default();
        for x in 0..19 {
            for y in 15..30 {
                for z in 0..19 {
                    initial.cells.insert(
                        [x, y, z],
                        HeatCell {
                            heat: if (x + z) % 3 == 0 { HEAT_CAP } else { 120 },
                            burn: match (x + y + z) % 4 {
                                0 => 1,
                                1 => 2,
                                _ => 0,
                            },
                        },
                    );
                }
            }
        }
        initial.cursor = Some([8, 20, 3]);
        let snapshot = |h: &Heat, w: &World| {
            let mut b = Blob::new();
            h.save(&mut b);
            w.save(&mut b);
            (b.v, h.ignited.clone(), h.burned.clone(), h.melting.clone())
        };
        for workers in [1, 2, 4, 6, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for budget in [0, 1, 31, 1023, 1024, 4096] {
                let mut t = tuning();
                t.cells_per_tick = budget;
                let mut serial = initial.clone();
                let mut parallel = initial.clone();
                let mut reference_world = World::new(64, 48, 64, 4096, Box::new(Grid));
                let mut world = World::new(64, 48, 64, 4096, Box::new(Grid));
                for tick in 0..8 {
                    if tick == 4 {
                        serial.cursor = None;
                        parallel.cursor = None;
                    }
                    let reference_calls = std::cell::RefCell::new(Vec::new());
                    let claimed = |p: [u32; 3]| {
                        let mut calls = reference_calls.borrow_mut();
                        let rank = calls.len() as u32;
                        calls.push(p);
                        (p[0] + rank) % 7 == tick % 7
                    };
                    serial.step_in(&mut reference_world, &registry, &t, claimed, false);
                    let trace = crate::task_trace::Trace::default();
                    let calls = pool.install(|| {
                        // A non-Sync callback stays on the ordered commit path.
                        let calls = std::cell::RefCell::new(Vec::new());
                        parallel.step_in_observed(
                            &mut world,
                            &registry,
                            &t,
                            |p| {
                                let mut calls = calls.borrow_mut();
                                let rank = calls.len() as u32;
                                calls.push(p);
                                (p[0] + rank) % 7 == tick % 7
                            },
                            (true, &|slot, event| trace.record(slot, event)),
                        );
                        calls.into_inner()
                    });
                    trace.balanced(&[
                        "physics_ground_heat",
                        "physics_ground_heat_read",
                        "physics_ground_heat_commit",
                    ]);
                    assert_eq!(calls, reference_calls.into_inner());
                    assert_eq!(
                        snapshot(&parallel, &world),
                        snapshot(&serial, &reference_world),
                        "workers={workers}, budget={budget}, tick={tick}"
                    );
                    if budget == 4096 && tick == 0 {
                        assert!(!parallel.ignited.is_empty());
                        assert!(!parallel.burned.is_empty());
                        assert!(!parallel.melting.is_empty());
                        assert!(world.queued() > 0);
                        assert!(parallel.burned_total > 0);
                    }
                    world.flush();
                    reference_world.flush();
                    assert_eq!(
                        snapshot(&parallel, &world),
                        snapshot(&serial, &reference_world)
                    );
                }
            }
        }
        let mut empty = Heat::default();
        empty.step_in(
            &mut World::new(64, 48, 64, 4096, Box::new(Grid)),
            &registry,
            &tuning(),
            |_| false,
            true,
        );
        assert!(empty.cells.is_empty() && empty.cursor.is_none());
    }
    #[test]
    fn bare_soil_cools_to_nothing_and_fuel_lights_spreads_and_burns_out() {
        let mut w = World::new(64, 32, 64, 4096, Box::new(Flat));
        let (t, mats) = (tuning(), mats());
        let mut h = Heat::default();
        // Bare soil holds no heat: nothing there can burn.
        h.deposit(&w, &mats, [10, 16, 10], 3, 60);
        assert_eq!((h.hot(), h.burning()), (0, 0));
        // A burst beside the block lights only the fuel it reaches, less
        // towards the edge of its reach.
        h.deposit(&w, &mats, [28, 16, 32], 6, 60);
        assert!(h.hot() > 0 && h.hot() < 64, "reach: {}", h.hot());
        assert!(h.cell([30, 16, 32]).heat > h.cell([33, 16, 32]).heat);
        h.deposit(&w, &mats, [10, 16, 10], 3, 0);
        let mut hot = Heat::default();
        hot.add(&w, [30, 16, 30], 30);
        let mut h = hot;
        let mut lit = 0;
        let (mut first, mut last) = (None, None);
        for tick in 0..900 {
            h.step(&mut w, &mats, &t, |_| false);
            w.flush();
            if !h.ignited.is_empty() {
                first.get_or_insert(tick);
                last = Some(tick);
                lit += h.ignited.len();
            }
            if lit > 0 && h.burning() == 0 && h.hot() == 0 {
                assert!(tick > 60, "the block burned out too fast");
                break;
            }
        }
        assert_eq!(
            h.burned_total, 64,
            "the whole block did not burn: lit {lit}"
        );
        assert_eq!(h.burned_by_material[4], 64);
        // A front, not a flash: the far corner lights well after the first cell.
        assert!(
            last.unwrap() - first.unwrap() >= 8,
            "the fire did not spread, it flashed: lit {first:?}..{last:?}"
        );
        assert_eq!((h.burning(), h.hot()), (0, 0));
        assert_eq!(w.material(31, 17, 31), AIR);
        assert_eq!(w.material(31, 15, 31), 1, "soil burned");
    }
}
