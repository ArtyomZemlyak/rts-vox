//! Conserved finite-volume body melt. The existing debris slot/container owns
//! each drop; liquid has no parked/support/merge state until it freezes.
use super::*;
use modding::heat::HeatTuning;

#[derive(Default)]
pub(crate) struct State {
    pub(crate) cells: BTreeMap<u64, u32>,
    cursor: Option<u64>,
}
impl State {
    pub(crate) fn save(&self, b: &mut Blob) {
        b.usize(self.cells.len());
        for (&id, &heat) in &self.cells {
            b.u64(id);
            b.u32(heat);
        }
        b.bool(self.cursor.is_some());
        if let Some(cursor) = self.cursor {
            b.u64(cursor);
        }
    }
    pub(crate) fn load(s: &mut Scan) -> Option<Self> {
        let n = s.usize();
        if n == 0 || n > s.left() / 12 {
            return None;
        }
        let mut cells = BTreeMap::new();
        let mut previous = 0;
        for _ in 0..n {
            let id = s.u64();
            let heat = s.u32();
            if id <= previous || heat == 0 || heat > crate::heat::HEAT_CAP {
                return None;
            }
            cells.insert(id, heat);
            previous = id;
        }
        let cursor = if s.bool() { Some(s.u64()) } else { None };
        s.ok().then_some(Self { cells, cursor })
    }
    pub(crate) fn validate(&self, debris: &Bursts, t: &HeatTuning) -> Option<()> {
        let pieces: BTreeMap<_, _> = debris.fragments.iter().map(|f| (f.id, f)).collect();
        for id in self.cells.keys() {
            let f = pieces.get(id)?;
            if f.resting
                || f.chunk.is_some()
                || f.count != 1
                || f.side != 1
                || f.shape != 1
                || f.paint.is_none()
                || !t.melt_flow.iter().flatten().any(|p| p.liquid == f.material)
            {
                return None;
            }
        }
        if self.cursor.is_some_and(|id| id > debris.next_id) {
            return None;
        }
        Some(())
    }
    pub(crate) fn hash(&self, h: &mut u64) {
        if self.cells.is_empty() {
            return;
        }
        mix(h, 81);
        mix(h, self.cursor.unwrap_or(0));
        for (&id, &heat) in &self.cells {
            mix(h, id);
            mix(h, u64::from(heat));
        }
    }
}

impl Bursts {
    pub fn molten_heat(&self, id: u64) -> Option<u32> {
        self.molten.cells.get(&id).copied()
    }

    /// Sparse droplet state only. Geometry and settled contacts are the frozen
    /// beginning-of-phase snapshot already prepared by the ordinary debris pass.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step_molten(
        &mut self,
        world: &voxel::World,
        _e: &mut Economy,
        t: &Blast,
        heat: &HeatTuning,
        gravity: Fx,
        tick: u64,
        hulls: &[Hull],
    ) {
        if self.molten.cells.is_empty() {
            return;
        }
        let indices: BTreeMap<_, _> = self
            .fragments
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();
        let cursor = self.molten.cursor.unwrap_or(0);
        let batch: Vec<_> = self
            .molten
            .cells
            .range((
                std::ops::Bound::Excluded(cursor),
                std::ops::Bound::Unbounded,
            ))
            .chain(self.molten.cells.range(..=cursor))
            .map(|(&id, &h)| (id, h))
            .take(heat.cells_per_tick)
            .collect();
        self.molten.cursor = batch.last().map(|p| p.0);
        let divisor = self
            .molten
            .cells
            .len()
            .div_ceil(heat.melt_moves_per_tick.max(1))
            .max(2) as u64;
        let mut moves = 0;
        let mut motion = FragmentMotion::new(t);
        motion.bounce = Fx::ZERO;
        motion.friction = Fx::ZERO;
        motion.liquid = true;
        for (id, energy) in batch {
            let Some(&i) = indices.get(&id) else {
                self.molten.cells.remove(&id);
                continue;
            };
            let f = &mut self.fragments[i];
            let Some(flow) = heat
                .melt_flow
                .iter()
                .flatten()
                .find(|p| p.liquid == f.material)
            else {
                self.molten.cells.remove(&id);
                continue;
            };
            let lost = (u64::from(energy) * u64::from(heat.cooling_milli) / 1000).max(1) as u32;
            let energy = energy.saturating_sub(lost);
            if crate::blast::thermal::temperature(energy, f.mm) < flow.freeze {
                f.material = flow.solid;
                self.molten.cells.remove(&id);
                continue;
            }
            self.molten.cells.insert(id, energy);
            if heat.melt_moves_per_tick == 0
                || moves >= heat.melt_moves_per_tick
                || id.wrapping_add(tick) % divisor != 0
            {
                continue;
            }
            moves += 1;
            // Inelastic contact and no tumbling: a liquid drop cannot bounce,
            // become a rigid support or be merged as a solid resting pile.
            advance_fragment(
                f,
                world,
                t,
                &motion,
                gravity,
                hulls,
                &self.settled_scratch,
                u32::MAX,
                false,
            );
            let landed = f.resting;
            f.resting = false;
            if !landed {
                continue;
            }
            let (low, high) = f.bounds();
            let from = arr(f.position);
            let step = Fx::ONE.min(Fx::from_ratio(
                i32::from(f.mm),
                i32::from(voxel::unit::WORLD_MM),
            ));
            for offset in 0..4 {
                let direction =
                    [[-1, 0], [1, 0], [0, -1], [0, 1]][((id + tick + offset) % 4) as usize];
                let side = [
                    from[0].add(step.mul(Fx::from_int(direction[0]))),
                    from[1],
                    from[2].add(step.mul(Fx::from_int(direction[1]))),
                ];
                let down = [side[0], side[1].sub(step), side[2]];
                let clear = |from: [Fx; 3], to: [Fx; 3]| {
                    let lo = [0, 1, 2].map(|a| from[a].min(to[a]).add(low[a]));
                    let hi = [0, 1, 2].map(|a| from[a].max(to[a]).add(high[a]));
                    !box_contact(world, [Fx::ZERO; 3], lo, hi, [world.nx, world.ny, world.nz])
                        && !hulls.iter().any(|h| h.meets_piece([Fx::ZERO; 3], lo, hi))
                        && !self.settled_scratch.hit(lo, hi, u32::MAX, None)
                };
                // Like terrain melt, a lone supported drop only runs downhill;
                // it never random-walks across a level floor before freezing.
                if clear(from, side) && clear(side, down) {
                    f.position = v3(down);
                    break;
                }
            }
        }
        if self.molten.cells.is_empty() {
            self.molten.cursor = None;
        }
    }

    /// Exact slab entry distances, sorted by distance and persistent piece ID.
    /// The list is built once per beam, not once per ray-march sample.
    pub(crate) fn molten_ray_contacts(
        &self,
        from: [Fx; 3],
        direction: [Fx; 3],
        range: Fx,
    ) -> Vec<(Fx, u64)> {
        if self.molten.cells.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for f in &self.fragments {
            if !self.molten.cells.contains_key(&f.id) {
                continue;
            }
            let (low, high) = f.bounds();
            let centre = arr(f.position);
            let mut entry = Fx::ZERO;
            let mut exit = range;
            for a in 0..3 {
                let lo = centre[a].add(low[a]);
                let hi = centre[a].add(high[a]);
                if direction[a] == Fx::ZERO {
                    if from[a] < lo || from[a] >= hi {
                        exit = Fx::from_int(-1);
                        break;
                    }
                } else {
                    let first = lo.sub(from[a]).div(direction[a]);
                    let last = hi.sub(from[a]).div(direction[a]);
                    entry = entry.max(first.min(last));
                    exit = exit.min(first.max(last));
                }
            }
            if entry <= exit {
                hits.push((entry, f.id));
            }
        }
        hits.sort_by_key(|&(distance, id)| (distance, id));
        hits
    }

    /// Absorb only the outstanding vaporisation energy. The original container
    /// is sunk and retired exactly once; the surplus remains in the beam.
    pub(crate) fn heat_molten(
        &mut self,
        id: u64,
        offered: u32,
        t: &HeatTuning,
        e: &mut Economy,
    ) -> (u32, bool) {
        let Some(&energy) = self.molten.cells.get(&id) else {
            return (0, true);
        };
        let Some(i) = self.fragments.iter().position(|f| f.id == id) else {
            return (0, true);
        };
        let f = &self.fragments[i];
        let Some(flow) = t
            .melt_flow
            .iter()
            .flatten()
            .find(|p| p.liquid == f.material)
        else {
            return (0, false);
        };
        let base = e
            .mats
            .get(flow.solid)
            .map_or(flow.freeze, |m| m.melt.max(flow.freeze));
        let base = base.saturating_mul(t.vapor_heat_multiple);
        let threshold = (u128::from(base) * u128::from(f.mm).pow(3)
            / u128::from(voxel::unit::WORLD_MM).pow(3))
        .min(u128::from(u32::MAX)) as u32;
        let used = offered.min(threshold.saturating_sub(energy));
        let total = energy.saturating_add(used).min(crate::heat::HEAT_CAP);
        if total < threshold {
            self.molten.cells.insert(id, total);
            return (used, false);
        }
        let held = f.held;
        e.ledger
            .sink(held, u64::MAX, crate::matter::Sink::Pulverised);
        assert!(e.ledger.retire_empty_debris(held));
        e.wrecks.retain(|w| w.held != held);
        self.fragments.remove(i);
        self.molten.cells.remove(&id);
        if self.molten.cells.is_empty() {
            self.molten.cursor = None;
        }
        (used, true)
    }
}
