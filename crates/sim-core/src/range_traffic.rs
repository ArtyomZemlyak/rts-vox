//! Exact immutable traffic broad phase and reusable live collision lists.
//! No neighbours are capped; geometric admission remains in Motion.
use super::*;

pub(in crate::blast) struct TrafficIndex {
    grid: crate::index::Grid,
    max_half: Fx,
}
impl TrafficIndex {
    pub(in crate::blast) fn new(traffic: &[Traffic], world: i32, cell: i32) -> Self {
        let xs: Vec<_> = traffic.iter().map(|t| t.obstacle.pos[0].to_int_trunc()).collect();
        let zs: Vec<_> = traffic.iter().map(|t| t.obstacle.pos[2].to_int_trunc()).collect();
        let active: Vec<_> = traffic.iter().enumerate()
            .filter(|(_, t)| t.intent && t.velocity >= Fx::ZERO)
            .map(|(id, _)| id as u32).collect();
        let max_half = active.iter().map(|&id| {
            let o = &traffic[id as usize].obstacle;
            o.half[0].max(o.half[1])
        }).max().unwrap_or(Fx::ZERO);
        let mut grid = crate::index::Grid::new(world, cell.max(1), active.len());
        grid.rebuild(&xs, &zs, &active);
        Self { grid, max_half }
    }
    pub(in crate::blast) fn candidates(&self, own: &Traffic, out: &mut Vec<usize>) {
        out.clear();
        // within() uses fractional centres and each other's largest half.
        // Two units cover truncation at both ends, including negative centres.
        let pad = own.passing_reach.add(self.max_half).to_int_trunc().saturating_add(2);
        let p = [own.obstacle.pos[0].to_int_trunc(), own.obstacle.pos[2].to_int_trunc()];
        self.grid.for_each_rect(p.map(|v| v.saturating_sub(pad)),
            p.map(|v| v.saturating_add(pad)), |id| out.push(id as usize));
        // Grid traversal is spatial; the original advice reduces in slot order.
        // Each centre is inserted once, so no deduplication or dropped tail.
        out.sort_unstable();
    }
}

/// The launch bay can exclude both the actor and its owning factory. Keep
/// original identities rather than reconstructing a slot by adding one.
pub(in crate::blast) struct HullNeighbors {
    obstacles: Vec<Obstacle>,
    ids: Vec<usize>,
}

/// Serial-commit broad phase. Each ordinary centre belongs to one cell;
/// oversized hulls stay in a separate list rather than inflating every query.
/// Only slot IDs are stored. The authoritative Obstacle is always read live.
pub(in crate::blast) struct LiveHulls {
    cell: i32,
    origin: [i32; 2],
    size: [usize; 2],
    heads: Vec<usize>,
    next: Vec<usize>,
    prev: Vec<usize>,
    large: Vec<usize>,
    locations: Vec<Option<usize>>,
}
impl LiveHulls {
    const NONE: usize = usize::MAX;
    pub(in crate::blast) fn new(positions: &[Obstacle], cell: i32) -> Self {
        let mut cell = cell.max(1);
        // Keep the directory linear in actor count even for a huge sparse
        // world. Cell coarsening affects candidate cost, never admission.
        let (origin, size) = loop {
            let mut lo = [i32::MAX; 2]; let mut hi = [i32::MIN; 2];
            for o in positions {
                for a in 0..2 {
                    let k = o.pos[a * 2].to_int_trunc().div_euclid(cell);
                    lo[a] = lo[a].min(k); hi[a] = hi[a].max(k);
                }
            }
            if positions.is_empty() { lo = [0; 2]; hi = [0; 2]; }
            let size = [0, 1].map(|a| (i64::from(hi[a]) - i64::from(lo[a]) + 1) as u64);
            if size[0].saturating_mul(size[1]) <= positions.len().saturating_mul(16).max(16) as u64 {
                break (lo, size.map(|n| n as usize));
            }
            cell = cell.saturating_mul(2);
        };
        let mut out = Self { cell, origin, size, heads: vec![Self::NONE; size[0] * size[1]],
            next: vec![Self::NONE; positions.len()], prev: vec![Self::NONE; positions.len()],
            large: Vec::new(), locations: vec![None; positions.len()] };
        for (id, o) in positions.iter().enumerate() { out.insert(id, o); }
        out
    }
    fn coordinate(&self, p: [i32; 2]) -> [usize; 2] {
        [0, 1].map(|a| (i64::from(p[a].div_euclid(self.cell)) - i64::from(self.origin[a]))
            .clamp(0, self.size[a] as i64 - 1) as usize)
    }
    fn location(&self, o: &Obstacle) -> Option<usize> {
        (o.half[0].abs().add(o.half[1].abs()) <= Fx::from_int(self.cell)).then(|| {
            let [x, z] = self.coordinate([o.pos[0].to_int_trunc(), o.pos[2].to_int_trunc()]);
            z * self.size[0] + x
        })
    }
    fn insert(&mut self, id: usize, o: &Obstacle) {
        let at = self.location(o);
        self.locations[id] = at;
        if let Some(at) = at {
            let old = self.heads[at];
            self.prev[id] = Self::NONE; self.next[id] = old;
            if old != Self::NONE { self.prev[old] = id; }
            self.heads[at] = id;
        } else {
            let slot = self.large.binary_search(&id).expect_err("duplicate live hull");
            self.large.insert(slot, id);
        }
    }
    pub(in crate::blast) fn relocate(&mut self, id: usize, o: &Obstacle) {
        let old = self.locations[id];
        if old == self.location(o) { return; }
        if let Some(at) = old {
            let (prev, next) = (self.prev[id], self.next[id]);
            if prev == Self::NONE { self.heads[at] = next; } else { self.next[prev] = next; }
            if next != Self::NONE { self.prev[next] = prev; }
        } else {
            self.large.remove(self.large.binary_search(&id).unwrap());
        }
        self.insert(id, o);
    }
    fn candidates(&self, positions: &[Obstacle], at: [Fx; 2], radius: Fx,
        out: &mut Vec<usize>) {
        out.clear();
        // L1 body radii cover every heading. Two units cover fractional
        // truncation at both ends, including centres outside the map.
        let pad = radius.add(Fx::from_int(self.cell)).to_int_trunc().saturating_add(2);
        let center = at.map(Fx::to_int_trunc);
        let lo = self.coordinate(center.map(|p| p.saturating_sub(pad)));
        let hi = self.coordinate(center.map(|p| p.saturating_add(pad)));
        if (hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1) >= positions.len() {
            out.extend(0..positions.len());
        } else {
            out.extend(self.large.iter().copied());
            for z in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let mut id = self.heads[z * self.size[0] + x];
                    while id != Self::NONE { out.push(id); id = self.next[id]; }
                }
            }
            out.sort_unstable();
        }
        out.retain(|&id| {
            let o = &positions[id];
            let reach = radius.add(o.half[0].abs()).add(o.half[1].abs());
            (0..2).all(|a| o.pos[a * 2].sub(at[a]).abs() <= reach)
        });
    }
}

/// Rebuilt from this step's live list. Covers any heading and every possible
/// translated trial of the installed drive, not just the first proposed pose.
#[derive(Clone, Debug, Default)]
pub(super) struct CollisionCandidates {
    ids: Vec<usize>,
}
impl CollisionCandidates {
    pub(super) fn clear(&mut self) { self.ids.clear(); }
    pub(super) fn rebuild(&mut self, own: &Obstacle, travel: Fx, others: &[Obstacle]) {
        self.ids.clear();
        let radius = own.half[0].add(own.half[1]).add(travel).add(Fx::ONE);
        self.ids.extend(others.iter().enumerate().filter(|(_, other)| {
            let reach = radius.add(other.half[0]).add(other.half[1]);
            own.overlaps_height(other.pos[1], other.height)
                && other.pos[0].sub(own.pos[0]).abs() <= reach
                && other.pos[2].sub(own.pos[2]).abs() <= reach
        }).map(|(id, _)| id));
    }
    pub(super) fn first(&self, previous: &Obstacle, candidate: &Obstacle,
        others: &[Obstacle]) -> Option<usize> {
        self.ids.iter().copied().find(|&id| hull_contact_blocks(previous, candidate, &others[id]))
    }
}
impl HullNeighbors {
    pub(in crate::blast) fn new(capacity: usize) -> Self {
        Self { obstacles: Vec::with_capacity(capacity), ids: Vec::with_capacity(capacity) }
    }
    pub(in crate::blast) fn clear(&mut self) {
        self.obstacles.clear();
        self.ids.clear();
    }
    pub(in crate::blast) fn gather(&mut self, positions: &[Obstacle], own: usize,
        ignored: Option<usize>, gun: Option<Obstacle>) {
        self.clear();
        // At most three bulk copies replace a branch and Vec::push for
        // every hull. Source slot order and ignored launch ownership stay exact.
        let len = positions.len();
        let mut skip = [own.min(len), ignored.unwrap_or(len).min(len)];
        skip.sort_unstable();
        let mut begin = 0;
        for end in skip {
            if end < begin { continue; }
            self.obstacles.extend_from_slice(&positions[begin..end]);
            self.ids.extend(begin..end);
            begin = end.saturating_add(1).min(len);
        }
        self.obstacles.extend_from_slice(&positions[begin..]);
        self.ids.extend(begin..len);
        if let Some(gun) = gun {
            self.obstacles.push(gun);
            self.ids.push(positions.len());
        }
    }
    /// Local motor/turn-preview list only. Route starts, continuations and
    /// optional long shortcuts retain their full original obstacle domain.
    pub(in crate::blast) fn gather_step(&mut self, positions: &[Obstacle], own: usize,
        ignored: Option<usize>, gun: Option<Obstacle>, index: Option<&LiveHulls>,
        region: Option<([Fx; 2], Fx)>) {
        let (Some(index), Some((at, radius))) = (index, region) else {
            self.gather(positions, own, ignored, gun);
            return;
        };
        self.obstacles.clear();
        index.candidates(positions, at, radius, &mut self.ids);
        self.ids.retain(|&id| id != own && Some(id) != ignored);
        self.obstacles.extend(self.ids.iter().map(|&id| positions[id]));
        if let Some(gun) = gun {
            self.obstacles.push(gun);
            self.ids.push(positions.len());
        }
    }
    pub(in crate::blast) fn as_slice(&self) -> &[Obstacle] { &self.obstacles }
    pub(in crate::blast) fn target(&self, index: usize) -> usize { self.ids[index] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modding::range::RangeUnitKind;
    fn sample(body: &UnitBody, i: usize, dense: bool) -> Motion {
        let at = if dense { [500, 16, 500] } else {
            [128 + (i % 16) as i32 * 256, 16, 128 + (i / 16) as i32 * 256]
        };
        let mut m = Motion::new(at, RangeUnitKind::Tank, body);
        m.enabled = true;
        m.destination = Some([at[0] + 700, at[2]]);
        m.path = vec![m.destination.unwrap()];
        m.drive.head = if i % 2 == 0 { 0 } else { angle::HALF };
        m.drive.vel = Fx::from_int(if i % 3 == 0 { 20 } else { 40 });
        m
    }
    #[test]
    fn traffic_grid_matches_all_pairs_and_prunes_distant_units() {
        let body = voxel::unit::tank();
        let mut units: Vec<_> = (0..512).map(|i| sample(&body, i, false)).collect();
        units[1].pos[0] = units[0].pos[0].add(Fx::from_int(100));
        let traffic: Vec<_> = units.iter().map(|m| Traffic::of(m, &body, true)).collect();
        let index = TrafficIndex::new(&traffic, 8192, 128);
        let mut candidates = Vec::new();
        let mut checked = 0;
        let response = crate::mobility::Mobility::intact(true);
        let mut constrained = 0;
        for (id, m) in units.iter().enumerate() {
            index.candidates(&traffic[id], &mut candidates);
            assert!(candidates.windows(2).all(|w| w[0] < w[1]));
            checked += candidates.len();
            let view = |h: Option<TrafficHint>| h.map(|h| (h.speed, h.way));
            let full = m.traffic_hint(&body, id, &traffic, response);
            let indexed = m.traffic_hint_near(&body, id, &traffic, response, &candidates);
            constrained += usize::from(full.is_some_and(|h| h.way.is_some() || h.speed < Fx::from_int(m.speed)));
            assert_eq!(view(full), view(indexed), "traffic {id}");
        }
        assert!(constrained > 0, "oracle did not exercise steering");
        assert!(checked < units.len() * units.len() / 8, "ineffective broad phase: {checked}");
    }
    #[test]
    fn traffic_grid_handles_fractional_edges_giant_hulls_and_inactive_units() {
        let body = voxel::unit::tank();
        let mut units: Vec<_> = (0..128).map(|i| sample(&body, i, false)).collect();
        for (id, m) in units.iter_mut().enumerate() {
            m.pos[0] = Fx::from_ratio((id as i32 * 197 % 2100) - 500, 2);
            m.pos[2] = Fx::from_ratio((id as i32 * 311 % 2100) - 500, 2);
            if id % 7 == 0 { m.drive.vel = Fx::ONE.neg(); }
            if id % 11 == 0 { m.enabled = false; }
        }
        let mut traffic: Vec<_> = units.iter().enumerate()
            .map(|(id, m)| Traffic::of(m, &body, id % 13 != 0)).collect();
        for (id, t) in traffic.iter_mut().enumerate() {
            if id % 9 == 0 { t.obstacle.half = [Fx::from_ratio(1401, 2), Fx::from_int(10)]; }
            if id % 5 == 0 { t.obstacle.pos[1] = Fx::from_int(700); }
        }
        let index = TrafficIndex::new(&traffic, 768, 128);
        let mut candidates = Vec::new();
        for (id, m) in units.iter().enumerate() {
            index.candidates(&traffic[id], &mut candidates);
            for (other, t) in traffic.iter().enumerate() {
                if t.intent && t.velocity >= Fx::ZERO
                    && t.obstacle.within([m.pos[0], m.pos[2]], traffic[id].passing_reach) {
                    assert!(candidates.binary_search(&other).is_ok(), "missed {other} for {id}");
                }
            }
            let view = |h: Option<TrafficHint>| h.map(|h| (h.speed, h.way));
            let response = crate::mobility::Mobility::intact(true);
            assert_eq!(view(m.traffic_hint(&body, id, &traffic, response)),
                view(m.traffic_hint_near(&body, id, &traffic, response, &candidates)));
        }
    }
    #[test]
    fn dense_traffic_has_no_candidate_cap() {
        let body = voxel::unit::tank();
        let traffic: Vec<_> = (0..512).map(|i| Traffic::of(&sample(&body, i, true), &body, true)).collect();
        let index = TrafficIndex::new(&traffic, 1024, 128);
        let mut candidates = Vec::new();
        index.candidates(&traffic[0], &mut candidates);
        assert_eq!(candidates, (0..512).collect::<Vec<_>>());
    }
    struct Flat;
    impl voxel::Generator for Flat {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 { u16::from(y < 16) }
    }
    #[test]
    fn collision_sweep_candidates_match_full_scan_for_all_trial_headings() {
        let body = voxel::unit::tank();
        let m = Motion::new([500, 16, 500], RangeUnitKind::Tank, &body);
        let mut own = Obstacle::of(&m, &body);
        let others: Vec<_> = (0..512).map(|id| {
            let mut o = own;
            o.pos[0] = Fx::from_ratio(700 + (id % 32) as i32 * 31, 2);
            o.pos[2] = Fx::from_ratio(700 + (id / 32) as i32 * 31, 2);
            o.head = angle::norm(id as i32 * 419);
            if id % 7 == 0 { o.pos[1] = Fx::from_int(600); }
            if id % 13 == 0 { o.half[0] = Fx::from_int(140); }
            o
        }).collect();
        let travel = Fx::from_int(12);
        let mut near = CollisionCandidates::default();
        for orientation in 0..8 {
            own.head = orientation * angle::FULL / 8;
            near.rebuild(&own, travel, &others);
            for direction in 0..16 {
                let way = direction * angle::FULL / 16;
                for yaw in 0..8 {
                    let mut candidate = own;
                    candidate.head = yaw * angle::FULL / 8;
                    candidate.pos[0] = own.pos[0].add(angle::cos(way).mul(travel));
                    candidate.pos[2] = own.pos[2].add(angle::sin(way).mul(travel));
                    assert_eq!(near.first(&own, &candidate, &others),
                        others.iter().position(|o| hull_contact_blocks(&own, &candidate, o)),
                        "sweep {orientation}/{direction}/{yaw}");
                }
            }
        }
        // Rebuild must forget old slots and include a newly arrived hull.
        let mut moved = others;
        moved[511] = own;
        near.rebuild(&own, travel, &moved);
        assert!(near.ids.contains(&511));
    }
    #[test]
    fn mixed_unit_traffic_grid_keeps_the_full_scan_advice() {
        let templates: Vec<_> = [RangeUnitKind::Tank, RangeUnitKind::Commander,
            RangeUnitKind::Crawler, RangeUnitKind::Walker04, RangeUnitKind::Spider,
            RangeUnitKind::Strider, RangeUnitKind::LightningTank, RangeUnitKind::SkobaDrone]
            .into_iter().map(|kind| super::super::super::RangeUnit::of_kind(kind, [500, 16, 500])).collect();
        let units: Vec<_> = (0..64).map(|id| {
            let mut m = templates[id % templates.len()].motion.clone();
            m.pos[0] = Fx::from_int(200 + (id % 8) as i32 * 160);
            m.pos[2] = Fx::from_int(200 + (id / 8) as i32 * 160);
            m.pos[1] = Fx::from_int(if id % 8 == 7 { 700 } else { 16 });
            m.enabled = true;
            m.drive.head = if id % 2 == 0 { 0 } else { angle::HALF };
            m.drive.vel = Fx::from_int(m.speed);
            let goal = [1800 - m.pos[0].to_int_trunc(), m.pos[2].to_int_trunc()];
            m.destination = Some(goal); m.path = vec![goal]; m
        }).collect();
        let traffic: Vec<_> = units.iter().enumerate().map(|(id, m)|
            Traffic::of(m, &templates[id % templates.len()].body, true)).collect();
        let index = TrafficIndex::new(&traffic, 2048, 128);
        let mut candidates = Vec::new();
        for (id, m) in units.iter().enumerate() {
            index.candidates(&traffic[id], &mut candidates);
            let body = &templates[id % templates.len()].body;
            let response = crate::mobility::Mobility::intact(true);
            let view = |h: Option<TrafficHint>| h.map(|h| (h.speed, h.way));
            assert_eq!(view(m.traffic_hint(body, id, &traffic, response)),
                view(m.traffic_hint_near(body, id, &traffic, response, &candidates)), "mixed actor {id}");
        }
    }
    #[test]
    fn contact_after_ignoring_a_launch_owner_keeps_the_real_target_slot() {
        let w = World::new(1024, 128, 1024, 65536, Box::new(Flat));
        let body = voxel::unit::tank();
        let mut m = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        m.enabled = true; m.destination = Some([500, 300]); m.path = vec![[500, 300]];
        m.drive.vel = Fx::from_int(40); m.allow_start(false);
        let mut positions = vec![Obstacle::of(&m, &body); 5];
        for (id, p) in positions.iter_mut().enumerate() {
            p.pos[0] = Fx::from_int(600 + id as i32 * 80);
        }
        positions[3] = Obstacle::of(&m, &body);
        positions[4].pos[0] = Fx::from_int(230);
        let mut neighbors = HullNeighbors::new(6);
        neighbors.gather(&positions, 3, Some(1), None);
        assert_eq!(neighbors.ids, vec![0, 2, 4]);
        m.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
            neighbors.as_slice(), 0, 98, &nav::vehicle::Rubble::new(), &Default::default());
        let (slot, _) = m.take_contact().expect("forward collision");
        assert_eq!(neighbors.target(slot), 4, "contact was attributed to the wrong actor");
        positions[4].pos[0] = Fx::from_int(250);
        neighbors.gather(&positions, 3, Some(1), Some(positions[0]));
        assert_eq!(neighbors.ids, vec![0, 2, 4, 5]);
        assert_eq!(neighbors.as_slice()[2].pos[0], Fx::from_int(250));
    }
    #[test]
    fn skipping_parked_neighbor_copies_preserves_motion_and_shove_contacts() {
        let w = World::new(1024, 512, 1024, 65536, Box::new(Flat));
        for (kind, body) in [(RangeUnitKind::Tank, voxel::unit::tank()),
            (RangeUnitKind::Commander, voxel::unit::commander())] {
            let original = Motion::new([400, 16, 500], kind, &body);
            let mut blocker = Obstacle::of(&original, &body);
            blocker.pos[0] = Fx::from_int(500);
            for driven in [false, true] {
                for pushed in [false, true] {
                    let mut full = original.clone();
                    if pushed { full.shove = V3::new(Fx::from_int(40), Fx::ZERO, Fx::ZERO); }
                    let mut reduced = full.clone();
                    assert_eq!(reduced.needs_obstacles(driven), pushed);
                    let neighbors = if reduced.needs_obstacles(driven) { & [blocker][..] } else { &[] };
                    full.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
                        &[blocker], 0, 98, &nav::vehicle::Rubble::new(), &Default::default());
                    reduced.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
                        neighbors, 0, 98, &nav::vehicle::Rubble::new(), &Default::default());
                    let mut a = Blob::new(); let mut b = Blob::new();
                    full.save(&mut a); reduced.save(&mut b); assert_eq!(a.v, b.v);
                    assert_eq!(full.take_contact(), reduced.take_contact());
                    reduced.enabled = true;
                    assert!(reduced.needs_obstacles(true));
                }
            }
        }
    }
    #[test]
    fn live_hulls_matches_exact_domain_after_relocations_and_size_changes() {
        let b = voxel::unit::tank();
        let seed = Obstacle::of(&Motion::new([100, 16, 100], RangeUnitKind::Tank, &b), &b);
        let mut positions: Vec<_> = (0..512).map(|id| {
            let mut o = seed;
            o.pos[0] = Fx::from_ratio((id * 197 % 3500) as i32 - 700, 2);
            o.pos[2] = Fx::from_ratio((id * 311 % 3500) as i32 - 700, 2);
            if id % 9 == 0 { o.half = [Fx::from_int(700), Fx::ONE]; }
            if id % 7 == 0 { o.pos[1] = Fx::from_int(800); }
            o
        }).collect();
        let mut index = LiveHulls::new(&positions, 128);
        let mut ids = Vec::new();
        for round in 0..4 {
            for id in 0..positions.len() {
                if round > 0 {
                    positions[id].pos[0] = Fx::from_ratio((id * 157 % 4400) as i32 - 1400, round + 1);
                    positions[id].pos[2] = Fx::from_int(100 * (round - 2));
                    positions[id].half = [Fx::from_int(if (id + round as usize) % 3 == 0 { 900 } else { 16 }); 2];
                    index.relocate(id, &positions[id]);
                }
                for radius in [Fx::ZERO, Fx::from_ratio(257, 2), Fx::from_int(100000)] {
                    let at = [positions[id].pos[0], positions[id].pos[2]];
                    index.candidates(&positions, at, radius, &mut ids);
                    let full: Vec<_> = positions.iter().enumerate().filter(|(_, o)| {
                        let reach = radius.add(o.half[0].abs()).add(o.half[1].abs());
                        (0..2).all(|a| o.pos[a * 2].sub(at[a]).abs() <= reach)
                    }).map(|(id, _)| id).collect();
                    assert_eq!(ids, full, "round {round}, actor {id}");
                }
            }
        }
        // A dense pile must never drop a neighbour or reorder contact ties.
        for o in &mut positions { o.pos = seed.pos; }
        let dense = LiveHulls::new(&positions, 128);
        dense.candidates(&positions, [seed.pos[0], seed.pos[2]], Fx::ONE, &mut ids);
        assert_eq!(ids, (0..512).collect::<Vec<_>>());
        let mut neighbors = HullNeighbors::new(513);
        neighbors.gather_step(&positions, 5, Some(2), Some(seed), Some(&dense),
            Some(([seed.pos[0], seed.pos[2]], Fx::ONE)));
        assert_eq!(neighbors.ids.len(), 511);
        assert!(!neighbors.ids.contains(&5) && !neighbors.ids.contains(&2));
        assert_eq!(neighbors.target(510), 512);
    }

    #[test]
    fn indexed_motor_matches_full_live_lists_for_mixed_hulls_and_shoves() {
        let w = World::new(4096, 512, 4096, 65536, Box::new(Flat));
        w.prepare_navigation([4, 5]);
        let templates: Vec<_> = [RangeUnitKind::Tank, RangeUnitKind::Commander,
            RangeUnitKind::Crawler, RangeUnitKind::Walker04].into_iter()
            .map(|kind| super::super::super::RangeUnit::of_kind(kind, [500, 16, 500])).collect();
        let mut full: Vec<_> = (0..48).map(|id| {
            let mut m = templates[id % 4].motion.clone();
            m.pos = [200 + (id % 8) as i32 * 180, 16, 200 + (id / 8) as i32 * 200].map(Fx::from_int);
            m.enabled = true;
            m.drive.vel = Fx::from_int(if id % 3 == 0 { -10 } else { 20 });
            m.drive.head = angle::norm(id as i32 * 3131);
            let at = [m.pos[0].to_int_trunc(), m.pos[2].to_int_trunc()];
            m.destination = Some([at[0] + 300, at[1] + 400]);
            m.path = vec![[at[0] + 50, at[1] + 80], m.destination.unwrap()];
            m.allow_start(false);
            m
        }).collect();
        let mut reduced = full.clone();
        let response = crate::mobility::Mobility::intact(true);
        let rubble = nav::vehicle::Rubble::new();
        let changed = Default::default();
        let mut checked = 0;
        let mut baseline = 0;
        let mut contacts = 0;
        for tick in 0..80 {
            let mut positions: Vec<_> = full.iter().enumerate()
                .map(|(id, m)| Obstacle::of(m, &templates[id % 4].body)).collect();
            let mut index = LiveHulls::new(&positions, 128);
            let mut old = HullNeighbors::new(49);
            let mut new = HullNeighbors::new(49);
            for id in 0..48 {
                let b = &templates[id % 4].body;
                for m in [&mut full[id], &mut reduced[id]] {
                    m.allow_start(false);
                    if tick == 10 && id % 3 == 0 { m.shove = V3::new(Fx::from_int(400), Fx::ZERO, Fx::from_int(-150)); }
                    if tick == 30 && id % 5 == 0 { m.pilot = Some([-1, 1]); }
                }
                let region = reduced[id].step_neighbor_region(&positions[id], response);
                assert!(region.is_some());
                // Skip an owner, preserve slot IDs and include the physical gun.
                let ignored = (id != 0).then_some(0);
                let gun = Some(positions[47]);
                old.gather(&positions, id, ignored, gun);
                new.gather_step(&positions, id, ignored, gun, Some(&index), region);
                checked += new.ids.len(); baseline += old.ids.len();
                full[id].step(&w, b, true, true, response, old.as_slice(), tick, 98, &rubble, &changed);
                reduced[id].step(&w, b, true, true, response, new.as_slice(), tick, 98, &rubble, &changed);
                let mut a = Blob::new(); let mut z = Blob::new();
                full[id].save(&mut a); reduced[id].save(&mut z);
                assert_eq!(a.v, z.v, "motion actor {id} tick {tick}");
                let a = full[id].take_contact().map(|(slot, v)| (old.target(slot), v));
                let z = reduced[id].take_contact().map(|(slot, v)| (new.target(slot), v));
                contacts += usize::from(a.is_some());
                assert_eq!(a, z, "contact actor {id} tick {tick}");
                positions[id] = Obstacle::of(&reduced[id], b);
                index.relocate(id, &positions[id]);
            }
        }
        assert!(contacts > 0, "no physical contacts exercised");
        assert!(checked < baseline * 3 / 4, "no useful candidate reduction: {checked}/{baseline}");
        reduced[0].allow_start(true);
        assert!(reduced[0].step_neighbor_region(&Obstacle::of(&reduced[0], &templates[0].body), response).is_none());
    }

}
