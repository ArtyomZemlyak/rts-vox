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
    pub fn hot(&self) -> usize {
        self.cells.len()
    }
    pub fn burning(&self) -> usize {
        self.cells.values().filter(|c| c.burn > 0).count()
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
        for z in at[2] - radius..=at[2] + radius {
            for y in at[1] - radius..=at[1] + radius {
                for x in at[0] - radius..=at[0] + radius {
                    let d2 = (x - at[0]).pow(2) + (y - at[1]).pow(2) + (z - at[2]).pow(2);
                    if d2 >= r2 || x < 0 || y < 0 || z < 0 {
                        continue;
                    }
                    let p = [x as u32, y as u32, z as u32];
                    if !inside(w, p) {
                        continue;
                    }
                    let m = w.material(p[0], p[1], p[2]);
                    if m == AIR || mats.get(m).is_none_or(|d| d.kindling == 0 && d.melt == 0) {
                        continue;
                    }
                    let share = (u64::from(per_cell) * (r2 - d2) as u64 / r2 as u64) as u32;
                    self.add(w, p, share);
                }
            }
        }
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
        self.ignited.clear();
        self.burned.clear();
        self.melting.clear();
        if self.cells.is_empty() {
            self.cursor = None;
            return;
        }
        let budget = t.cells_per_tick.max(1);
        let mut batch: Vec<[u32; 3]> = match self.cursor {
            Some(c) => self
                .cells
                .range(c..)
                .map(|(k, _)| *k)
                .take(budget)
                .collect(),
            None => Vec::new(),
        };
        if batch.len() < budget {
            let bound = self.cursor.unwrap_or([u32::MAX; 3]);
            batch.extend(
                self.cells
                    .range(..bound)
                    .map(|(k, _)| *k)
                    .take(budget - batch.len()),
            );
        }
        // Burning cells feed their solid neighbours; gains are gathered
        // first so the order of the batch cannot favour a direction.
        let mut gain: BTreeMap<[u32; 3], u32> = BTreeMap::new();
        for &p in &batch {
            let c = self.cells[&p];
            if c.burn == 0 {
                continue;
            }
            let m = w.material(p[0], p[1], p[2]);
            let share = mats
                .get(m)
                .map_or(0, |d| d.fuel / u32::from(t.burn_ticks.max(1)));
            if share == 0 {
                continue;
            }
            for q in neighbours(w, p) {
                if w.solid(q[0], q[1], q[2]) {
                    *gain.entry(q).or_insert(0) += share;
                }
            }
        }
        let mut last = None;
        for p in batch {
            last = Some(p);
            let m = w.material(p[0], p[1], p[2]);
            if m == AIR {
                // Taken by something else: its heat went with it.
                self.cells.remove(&p);
                continue;
            }
            if claimed(p) {
                continue;
            }
            let mut c = self.cells[&p];
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
        if n > (w.nx * w.ny * w.nz) as usize {
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

fn inside(w: &World, p: [u32; 3]) -> bool {
    p[0] < w.nx && p[1] < w.ny && p[2] < w.nz
}
fn neighbours(w: &World, p: [u32; 3]) -> Vec<[u32; 3]> {
    let mut out = Vec::with_capacity(6);
    for a in 0..3 {
        if p[a] > 0 {
            let mut q = p;
            q[a] -= 1;
            out.push(q);
        }
        let mut q = p;
        q[a] += 1;
        if inside(w, q) {
            out.push(q);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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
