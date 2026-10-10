//! Whole-map terrain routes. The coarse corridor guides movement; live
//! oriented collision probes still authorize every step and local replanning.
use super::*;

#[derive(Clone, Copy)]
pub struct Config {
    pub cell: i32,
    pub nodes: usize,
    pub yielding: [Mat; 2],
    /// Wide articulated legs may climb a gradual grade across their broad
    /// footprint; a tracked belt still needs the strict combined height.
    pub local_grade: bool,
}

pub fn plan(
    world: &World,
    rubble: &Rubble,
    shape: Shape,
    from: [Fx; 2],
    to: [i32; 2],
    cfg: Config,
) -> Option<Route> {
    plan_explained(world, rubble, shape, from, to, cfg).ok()
}

/// Why the whole-map planner gave no route. Diagnostics only: the planner
/// itself treats every refusal alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Cell too small or no node allowance.
    Config,
    /// Start or goal off the lattice.
    OffLattice,
    /// The corridor from the start to its node, or from the goal's node
    /// to the goal, is not clear.
    StartCorridor,
    GoalCorridor,
    /// One frontier ran dry: that side is enclosed for this shape.
    /// 0 is the start's side, 1 the goal's.
    Enclosed {
        side: usize,
        expanded: usize,
    },
    /// The node allowance ran out.
    Budget,
    /// The route has more turns than a route may hold.
    TooLong,
}

/// [`plan`], with the reason for a refusal.
pub fn plan_explained(
    world: &World,
    rubble: &Rubble,
    shape: Shape,
    from: [Fx; 2],
    to: [i32; 2],
    cfg: Config,
) -> Result<Route, Refusal> {
    if cfg.cell < 8 || cfg.nodes == 0 {
        return Err(Refusal::Config);
    }
    let n = (world.nx as i32).div_euclid(cfg.cell) as usize;
    let rows = (world.nz as i32).div_euclid(cfg.cell) as usize;
    let origin = from.map(Fx::to_int_trunc);
    // One shared lattice for both directions and all units on the map.
    let anchor = [cfg.cell / 2; 2];
    let point = |i: usize| {
        [
            (i % n) as i32 * cfg.cell + anchor[0],
            (i / n) as i32 * cfg.cell + anchor[1],
        ]
    };
    let index = |p: [i32; 2]| -> Option<usize> {
        let c = [0, 1].map(|a| (p[a] - anchor[a]).div_euclid(cfg.cell));
        (c[0] >= 0 && c[1] >= 0 && c[0] < n as i32 && c[1] < rows as i32)
            .then(|| c[0] as usize + n * c[1] as usize)
    };
    let start = index(origin.map(|v| v + cfg.cell / 2)).ok_or(Refusal::OffLattice)?;
    let end = index(to.map(|v| v + cfg.cell / 2)).ok_or(Refusal::OffLattice)?;
    // A broad travel corridor, not an oriented turning/clearance proof.
    // Short maneuvers and actual movement keep the detailed footprint probe.
    let radius = shape.half[0].min(shape.half[1]) + 1;
    let debris = rubble.coarse_tiles([world.nx, world.nz]);
    let tile = |x: i32, z: i32| {
        let [bottom, top] = world.floor_tile(x as u32, z as u32, cfg.yielding).unwrap();
        [bottom, top.max(debris.top(x as usize, z as usize))]
    };
    let clear = |a: [i32; 2], b: [i32; 2]| -> bool {
        let lo = [0, 1].map(|k| a[k].min(b[k]) - radius);
        let hi = [0, 1].map(|k| a[k].max(b[k]) + radius);
        if lo[0] < 1 || lo[1] < 1 || hi[0] >= world.nx as i32 - 1 || hi[1] >= world.nz as i32 - 1 {
            return false;
        }
        if cfg.local_grade {
            for z in lo[1] / 8..=hi[1] / 8 {
                for x in lo[0] / 8..=hi[0] / 8 {
                    let [bottom, top] = tile(x, z);
                    if i32::from(top) - i32::from(bottom) > shape.step {
                        return false;
                    }
                    for (nx, nz) in [(x - 1, z), (x, z - 1)] {
                        if nx < lo[0] / 8 || nz < lo[1] / 8 {
                            continue;
                        }
                        let [next_bottom, next_top] = tile(nx, nz);
                        if i32::from(top.max(next_top)) - i32::from(bottom.min(next_bottom))
                            > shape.step
                        {
                            return false;
                        }
                    }
                }
            }
            true
        } else {
            let mut low = u16::MAX;
            let mut high = 0;
            for z in lo[1] / 8..=hi[1] / 8 {
                for x in lo[0] / 8..=hi[0] / 8 {
                    let [bottom, top] = tile(x, z);
                    low = low.min(bottom);
                    high = high.max(top);
                }
            }
            i32::from(high - low) <= shape.step
        }
    };
    // Test long straight corridors in short swept sections. Comparing the
    // heights of the entire line would mistake a gradual hill for a cliff.
    let straight = |a: [i32; 2], b: [i32; 2]| {
        let delta = [b[0] - a[0], b[1] - a[1]];
        // Each check covers the whole swept box between its endpoints;
        // eight-unit overlap repeats most tile queries for wide hulls.
        let steps = delta[0]
            .unsigned_abs()
            .max(delta[1].unsigned_abs())
            .div_ceil(cfg.cell as u32)
            .max(1) as i32;
        let mut last = a;
        for step in 1..=steps {
            let next = [0, 1].map(|axis| {
                a[axis] + (i64::from(delta[axis]) * i64::from(step) / i64::from(steps)) as i32
            });
            if !clear(last, next) {
                return false;
            }
            last = next;
        }
        true
    };
    if straight(origin, to) {
        return Ok(Route {
            path: vec![to],
            partial: false,
            stranded: false,
            nodes: 0,
            poses: 0,
            scans: 0,
            samples: 0,
            exhausted: false,
        });
    }
    // A hull in a cleared pad may have no clear corridor to its nearest
    // node: take the nearest node within two cells that has one, by
    // distance and then index.
    let start = {
        let c = [(start % n) as i32, (start / n) as i32];
        let mut candidates: Vec<(i64, usize)> = (-2..=2)
            .flat_map(|dz| (-2..=2).map(move |dx| [c[0] + dx, c[1] + dz]))
            .filter(|q| q[0] >= 0 && q[1] >= 0 && q[0] < n as i32 && q[1] < rows as i32)
            .map(|q| {
                let i = q[0] as usize + n * q[1] as usize;
                let at = point(i);
                let d = [0, 1].map(|a| i64::from(at[a] - origin[a]));
                (d[0] * d[0] + d[1] * d[1], i)
            })
            .collect();
        candidates.sort_unstable();
        candidates
            .into_iter()
            .map(|(_, i)| i)
            .find(|&i| clear(origin, point(i)))
            .ok_or(Refusal::StartCorridor)?
    };
    let mut distance = vec![[u32::MAX; 2]; n * rows];
    let mut previous = vec![[u32::MAX; 2]; n * rows];
    let mut open = [BinaryHeap::new(), BinaryHeap::new()];
    let heuristic = |i: usize, side: usize| {
        let p = point(i);
        let goal = if side == 0 { to } else { origin };
        let d = [
            (p[0] - goal[0]).unsigned_abs(),
            (p[1] - goal[1]).unsigned_abs(),
        ];
        (10 * d[0].max(d[1]) + 4 * d[0].min(d[1])) * 2 / cfg.cell as u32
    };
    // Join the first grid leg directly where possible, without a needless
    // backwards move to the snapped start node, and merge straight runs.
    let finish = |mut path: Vec<[i32; 2]>| -> Vec<[i32; 2]> {
        if (path.len() > 1 && clear(origin, path[1])) || path.first() == Some(&origin) {
            path.remove(0);
        }
        let mut compact: Vec<[i32; 2]> = Vec::new();
        let mut last = origin;
        for (k, &p) in path.iter().enumerate() {
            if let Some(&next) = path.get(k + 1) {
                let a = [p[0] - last[0], p[1] - last[1]];
                let b = [next[0] - p[0], next[1] - p[1]];
                if i64::from(a[0]) * i64::from(b[1]) == i64::from(a[1]) * i64::from(b[0])
                    && i64::from(a[0]) * i64::from(b[0]) + i64::from(a[1]) * i64::from(b[1]) > 0
                {
                    continue;
                }
            }
            compact.push(p);
            last = p;
        }
        compact
    };
    // Go as close as the corridor allows (analysis 240, C5). A goal whose
    // node has no clear corridor to it, or whose side of the lattice is
    // enclosed for this shape, used to refuse the whole route, and the hull
    // stood while the budgeted fine search crossed the map. Now a one-sided
    // search from the start returns a partial route to the reached node
    // nearest the goal, ties by index: the hull drives there, and the fine
    // search covers the rest. A node not a full cell nearer the goal than
    // the hull itself is no progress, and the refusal stands: the snapped
    // start node may lie behind the hull. `heuristic` counts twenty per
    // cell along an axis. The search shares the plan's node allowance with
    // any two-sided search that ran before it.
    let from_hull = {
        let d = [
            (origin[0] - to[0]).unsigned_abs(),
            (origin[1] - to[1]).unsigned_abs(),
        ];
        (10 * d[0].max(d[1]) + 4 * d[0].min(d[1])) * 2 / cfg.cell as u32
    };
    let approach = |refusal: Refusal, spent: usize| -> Result<Route, Refusal> {
        let allowance = cfg.nodes.saturating_sub(spent);
        let mut distance = vec![u32::MAX; n * rows];
        let mut previous = vec![u32::MAX; n * rows];
        let mut open = BinaryHeap::new();
        distance[start] = 0;
        open.push(Reverse((heuristic(start, 0), 0u32, start)));
        let mut best = (heuristic(start, 0), start);
        let mut expanded = 0;
        while let Some(Reverse((_, cost, i))) = open.pop() {
            if cost != distance[i] {
                continue;
            }
            best = best.min((heuristic(i, 0), i));
            if expanded >= allowance {
                break;
            }
            expanded += 1;
            let at = point(i);
            for d in DIR {
                let q = [at[0] + d[0] * cfg.cell, at[1] + d[1] * cfg.cell];
                let Some(j) = index(q) else {
                    continue;
                };
                let next = cost + if d[0] != 0 && d[1] != 0 { 14 } else { 10 };
                if next >= distance[j] || !clear(at, q) {
                    continue;
                }
                distance[j] = next;
                previous[j] = i as u32;
                open.push(Reverse((next + heuristic(j, 0), next, j)));
            }
        }
        if best.1 == start || best.0 + 20 > from_hull {
            return Err(refusal);
        }
        let mut path = Vec::new();
        let mut j = best.1;
        while j != start {
            path.push(point(j));
            j = previous[j] as usize;
        }
        path.push(point(start));
        path.reverse();
        let path = finish(path);
        if path.len() > 6400 {
            return Err(Refusal::TooLong);
        }
        Ok(Route {
            path,
            partial: true,
            stranded: false,
            nodes: expanded as u32,
            poses: 0,
            scans: 0,
            samples: 0,
            exhausted: false,
        })
    };
    if !clear(point(end), to) {
        return approach(Refusal::GoalCorridor, 0);
    }
    for (side, root) in [start, end].into_iter().enumerate() {
        distance[root][side] = 0;
        open[side].push(Reverse((heuristic(root, side), 0, root)));
    }
    let mut expanded = 0;
    loop {
        // Alternate the two frontiers under one shared deterministic cap.
        // A remote obstacle cannot monopolize a one-sided greedy search.
        let side = expanded % 2;
        let Some(Reverse((_, cost, i))) = open[side].pop() else {
            // Either frontier running dry means the two never meet; which
            // one ran dry first only says which side is smaller.
            return approach(Refusal::Enclosed { side, expanded }, expanded);
        };
        if cost != distance[i][side] {
            continue;
        }
        if distance[i][1 - side] != u32::MAX {
            let mut path = Vec::new();
            let mut j = i;
            while j != start {
                path.push(point(j));
                j = previous[j][0] as usize;
            }
            path.push(point(start));
            path.reverse();
            j = i;
            while j != end {
                j = previous[j][1] as usize;
                path.push(point(j));
            }
            if path.last() != Some(&to) {
                path.push(to);
            }
            let compact = finish(path);
            if compact.len() > 6400 {
                return Err(Refusal::TooLong);
            }
            return Ok(Route {
                path: compact,
                partial: false,
                stranded: false,
                nodes: expanded as u32,
                poses: 0,
                scans: 0,
                samples: 0,
                exhausted: false,
            });
        }
        if expanded >= cfg.nodes {
            return Err(Refusal::Budget);
        }
        expanded += 1;
        let at = point(i);
        for d in DIR {
            let q = [at[0] + d[0] * cfg.cell, at[1] + d[1] * cfg.cell];
            let Some(j) = index(q) else {
                continue;
            };
            let next = cost + if d[0] != 0 && d[1] != 0 { 14 } else { 10 };
            if next >= distance[j][side] || !clear(at, q) {
                continue;
            }
            distance[j][side] = next;
            previous[j][side] = i as u32;
            open[side].push(Reverse((next + heuristic(j, side), next, j)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Floor;
    impl voxel::Generator for Floor {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < 6 {
                2
            } else {
                AIR
            }
        }
    }
    fn shape() -> Shape {
        Shape {
            half: [10, 5],
            support: [8, 4],
            clearance: Fx::from_int(4),
            step: 4,
            min_up: Fx::from_ratio(9, 10),
            height: 12,
            span: 8,
            ramp: Fx::from_ratio(3, 5),
        }
    }
    fn config() -> Config {
        Config {
            cell: 16,
            nodes: 8192,
            yielding: [4, 5],
            local_grade: false,
        }
    }
    #[test]
    fn complete_route_crosses_a_kilometre_and_sweeps_around_a_thin_wall() {
        let mut world = World::new(10000, 32, 320, 100000, Box::new(Floor));
        for y in 6..26 {
            for z in 1..200 {
                world.queue_edit(voxel::Edit::put(4800, y, z, 3));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let from = [128, 128].map(Fx::from_int);
        let to = [9800, 128];
        assert!(
            plan(
                &world,
                &Rubble::new(),
                shape(),
                from,
                to,
                Config {
                    nodes: 1,
                    ..config()
                }
            )
            .is_none(),
            "budget must not return a complete route"
        );
        let cold = plan(&world, &Rubble::default(), shape(), from, to, config()).unwrap();
        world.prepare_navigation([4, 5]);
        let warm = plan(&world, &Rubble::default(), shape(), from, to, config()).unwrap();
        assert_eq!(cold.path, warm.path);
        assert_eq!(cold.nodes, warm.nodes);
        assert_eq!(warm.path.last(), Some(&to));
        assert!(!warm.partial);
        let mut ground = Probe::for_search(&world, &[4, 5], Columns::default());
        let mut at = from;
        let mut head = 0;
        for &p in &warm.path {
            let q = p.map(Fx::from_int);
            assert!(
                super::super::connects(&mut ground, shape(), at, head, q, &|_, _| true).is_some(),
                "invalid leg {at:?} -> {q:?}"
            );
            head = angle::atan2(q[1].sub(at[1]).raw(), q[0].sub(at[0]).raw());
            at = q;
        }
    }
    /// A route that goes no farther than `x`: refused, or partial and
    /// wholly on the near side (analysis 240, C5: a blocked goal gives a
    /// partial route as close as the corridor allows, never one across).
    fn stops_before(route: Option<Route>, x: i32) -> bool {
        route.is_none_or(|r| r.partial && r.path.iter().all(|p| p[0] < x))
    }
    /// A partial route that ends before `x`: the goal approached, not refused.
    fn approaches(route: Option<Route>, x: i32) -> bool {
        route.is_some_and(|r| r.partial && r.path.iter().all(|p| p[0] < x))
    }
    fn complete(route: Option<Route>) -> bool {
        route.is_some_and(|r| !r.partial)
    }
    #[test]
    fn tile_aligned_cliffs_and_live_debris_do_not_become_flat_corridors() {
        let mut world = World::new(512, 32, 256, 100000, Box::new(Floor));
        let from = [64, 128].map(Fx::from_int);
        let to = [448, 128];
        for x in 248..264 {
            for z in 0..256 {
                for y in 6..26 {
                    world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        assert!(approaches(
            plan(&world, &Rubble::new(), shape(), from, to, config()),
            248
        ));
        assert!(approaches(
            plan(
                &world,
                &Rubble::new(),
                shape(),
                from,
                to,
                Config {
                    local_grade: true,
                    ..config()
                }
            ),
            248
        ));
        world.restore_pristine();
        assert!(complete(plan(
            &world,
            &Rubble::new(),
            shape(),
            from,
            to,
            config()
        )));
        let rubble = Rubble::from_sorted((0..256).map(|z| ([256, z], 26)).collect());
        assert!(approaches(
            plan(&world, &rubble, shape(), from, to, config()),
            256
        ));
        assert!(complete(plan(
            &world,
            &Rubble::new(),
            shape(),
            from,
            to,
            config()
        )));
    }
    #[test]
    fn articulated_hull_can_route_up_a_gradual_hill() {
        struct Hill;
        impl voxel::Generator for Hill {
            fn material(&self, x: u32, y: u32, _z: u32) -> Mat {
                if y < 6 + x / 8 {
                    2
                } else {
                    AIR
                }
            }
        }
        let world = World::new(512, 96, 256, 100000, Box::new(Hill));
        let broad = Shape {
            half: [50, 50],
            support: [25, 25],
            step: 4,
            ..shape()
        };
        let from = [80, 128].map(Fx::from_int);
        let to = [432, 128];
        assert!(plan(
            &world,
            &Rubble::new(),
            broad,
            from,
            to,
            Config {
                nodes: 1,
                ..config()
            }
        )
        .is_none());
        let route = plan(
            &world,
            &Rubble::new(),
            broad,
            from,
            to,
            Config {
                nodes: 1,
                local_grade: true,
                ..config()
            },
        )
        .expect("a gradual grade is not one giant step for articulated legs");
        assert_eq!(route.path, vec![to]);
    }
    #[test]
    fn sparse_pages_refuse_fresh_debris_on_a_large_map() {
        let world = World::new(20000, 32, 10000, 100000, Box::new(Floor));
        let from = [64, 128].map(Fx::from_int);
        let to = [448, 128];
        let cfg = Config {
            nodes: 1,
            ..config()
        };
        assert_eq!(
            plan(&world, &Rubble::new(), shape(), from, to, cfg)
                .unwrap()
                .path,
            vec![to]
        );
        let rubble = Rubble::from_sorted((0..256).map(|z| ([256, z], 26)).collect());
        assert!(
            plan(&world, &rubble, shape(), from, to, cfg).is_none(),
            "large-map sparse debris was ignored"
        );
    }
    #[test]
    fn sealed_wall_forest_and_opened_gap_have_distinct_answers() {
        let mut world = World::new(512, 32, 256, 100000, Box::new(Floor));
        let from = [64, 128].map(Fx::from_int);
        let to = [448, 128];
        let r = Rubble::default();
        for y in 6..26 {
            for z in 0..256 {
                world.queue_edit(voxel::Edit::put(256, y, z, 4));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        assert!(
            complete(plan(&world, &r, shape(), from, to, config())),
            "forest is crushable"
        );
        for y in 6..26 {
            for z in 0..256 {
                world.queue_edit(voxel::Edit::put(256, y, z, 3));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        assert!(
            stops_before(plan(&world, &r, shape(), from, to, config()), 256),
            "sealed wall crossed"
        );
        // As close as the corridor allows: up to the wall, not refused.
        let near = plan(&world, &r, shape(), from, to, config()).expect("partial route");
        assert!(near.partial);
        assert!(
            near.path.last().is_some_and(|p| p[0] >= 200),
            "{:?}",
            near.path
        );
        for y in 6..26 {
            for z in 96..160 {
                world.queue_edit(voxel::Edit::dig(256, y, z));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        assert!(
            complete(plan(&world, &r, shape(), from, to, config())),
            "fresh hole not seen"
        );
    }
    /// **The cached debris tiles are the tiles a fresh build gives**, on
    /// a dense and a paged map, after every insert and removal, and a route
    /// over cached tiles equals one over a cold rubble (analysis 240, B6).
    #[test]
    fn cached_debris_tiles_follow_every_rubble_change() {
        for size in [[1024u32, 1024], [12_000, 12_000]] {
            let mut rubble = Rubble::default();
            let mut columns: Vec<([i32; 2], u16)> = Vec::new();
            let mut seed = 0x9e37_79b9u32;
            for step in 0..60 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let at = [(seed >> 8) as i32 % 900, (seed >> 20) as i32 % 900];
                if step % 4 == 3 && !columns.is_empty() {
                    let (gone, _) = columns.remove(columns.len() / 2);
                    rubble.remove(&gone);
                } else {
                    let top = 6 + (seed % 9) as u16;
                    columns.retain(|(c, _)| *c != at);
                    columns.push((at, top));
                    columns.sort_unstable();
                    rubble.insert(at, top);
                }
                let cached = rubble.coarse_tiles(size);
                let fresh = DebrisTiles::build(size, &columns);
                for &(at, _) in &columns {
                    let (x, z) = ((at[0] / 8) as usize, (at[1] / 8) as usize);
                    assert_eq!(cached.top(x, z), fresh.top(x, z), "{size:?} step {step}");
                }
                assert_eq!(cached.top(120, 120), fresh.top(120, 120));
            }
        }
        let mut world = World::new(1024, 32, 1024, 100000, Box::new(Floor));
        while world.queued() > 0 {
            world.flush();
        }
        let rubble: Rubble = (0..40).map(|k| ([300 + k, 500], 20)).collect();
        let (from, to) = ([100, 500].map(Fx::from_int), [900, 500]);
        let cold = plan(&world, &rubble.clone(), shape(), from, to, config()).unwrap();
        let warm = plan(&world, &rubble, shape(), from, to, config()).unwrap();
        let again = plan(&world, &rubble, shape(), from, to, config()).unwrap();
        assert_eq!(cold.path, warm.path);
        assert_eq!(warm.path, again.path);
        assert_eq!(cold.nodes, again.nodes);
        assert_ne!(
            cold.path,
            plan(&world, &Rubble::new(), shape(), from, to, config())
                .unwrap()
                .path,
            "the debris wall must shape the route, or this proves nothing"
        );
    }
    /// A pillar in the corridor from the stand to its nearest lattice
    /// node no longer refuses the route: the start snaps to the nearest
    /// node within two cells with a clear corridor (ADR-654). Origin
    /// (100, 128) has nearest node (104, 136); its corridor box spans x
    /// 94..110, z 122..142 and holds the pillar at x 104..110, z 138..142.
    /// Node (104, 120), as near, has a box ending at z 134.
    #[test]
    fn the_start_snaps_past_a_pillar_by_its_nearest_node() {
        let mut world = World::new(512, 32, 512, 100000, Box::new(Floor));
        for x in 104..110 {
            for z in 138..142 {
                for y in 6..26 {
                    world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let from = [100, 128].map(Fx::from_int);
        let route = plan_explained(&world, &Rubble::new(), shape(), from, [100, 400], config());
        assert!(
            route.as_ref().is_ok_and(|r| !r.partial),
            "{:?}",
            route.map(|r| r.path)
        );
    }
    /// A goal sealed in a room gives a partial route up to the room from
    /// afar, and a refusal from beside the room: a node not a full cell
    /// nearer the goal than the start is no progress (ADR-654).
    #[test]
    fn a_sealed_goal_is_approached_from_afar_and_refused_from_beside_it() {
        let mut world = World::new(512, 32, 256, 100000, Box::new(Floor));
        for y in 6..26 {
            for k in 260..=340 {
                for (x, z) in [(k, 88), (k, 168), (260, k - 172), (340, k - 172)] {
                    world.queue_edit(voxel::Edit::put(x, y, z as u32, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let r = Rubble::new();
        let goal = [300, 128];
        let far = plan_explained(
            &world,
            &r,
            shape(),
            [64, 128].map(Fx::from_int),
            goal,
            config(),
        );
        let far = far.expect("partial route from afar");
        assert!(far.partial);
        assert!(far.path.iter().all(|p| p[0] < 260), "{:?}", far.path);
        assert!(
            far.path.last().is_some_and(|p| p[0] >= 230),
            "{:?}",
            far.path
        );
        let beside = plan_explained(
            &world,
            &r,
            shape(),
            [244, 128].map(Fx::from_int),
            goal,
            config(),
        );
        assert!(beside.is_err(), "{:?}", beside.map(|r| r.path));
    }
}

const PAGE: usize = 32;

/// Highest debris top per 8×8 tile, for the coarse corridor test. Built
/// from the rubble columns once per rubble state (analysis 240, B6): every
/// route used to rebuild it, three megabytes on a kilometre map.
#[derive(Clone, Debug, Default)]
pub struct DebrisTiles {
    width: usize,
    dense: Vec<u16>,
    pages: Vec<Option<Box<[u16; PAGE * PAGE]>>>,
    page_width: usize,
}

impl DebrisTiles {
    pub(crate) fn build(size: [u32; 2], columns: &[([i32; 2], u16)]) -> Self {
        let tiles = [
            (size[0] as usize).div_ceil(8),
            (size[1] as usize).div_ceil(8),
        ];
        // A kilometre map needs only about 3 MiB for direct tile lookups. Keep
        // that fast path, but cap its size: a 10 km map would otherwise allocate
        // hundreds of MiB even with only a few debris columns.
        let dense = !columns.is_empty() && tiles[0] * tiles[1] <= 2_000_000;
        let page_width = tiles[0].div_ceil(PAGE);
        let mut out = Self {
            width: tiles[0],
            dense: if dense {
                vec![0u16; tiles[0] * tiles[1]]
            } else {
                Vec::new()
            },
            pages: if columns.is_empty() || dense {
                Vec::new()
            } else {
                vec![None; page_width * tiles[1].div_ceil(PAGE)]
            },
            page_width,
        };
        for &(p, top) in columns {
            let x = p[0].div_euclid(8);
            let z = p[1].div_euclid(8);
            if x < 0 || z < 0 || x as usize >= tiles[0] || z as usize >= tiles[1] {
                continue;
            }
            let (x, z) = (x as usize, z as usize);
            let pile = if dense {
                &mut out.dense[z * tiles[0] + x]
            } else {
                let page = &mut out.pages[z / PAGE * page_width + x / PAGE];
                let page = page.get_or_insert_with(|| Box::new([0; PAGE * PAGE]));
                &mut page[z % PAGE * PAGE + x % PAGE]
            };
            *pile = (*pile).max(top);
        }
        out
    }
    /// Highest debris top on the tile, zero with none.
    pub(crate) fn top(&self, x: usize, z: usize) -> u16 {
        if !self.dense.is_empty() {
            self.dense[z * self.width + x]
        } else if !self.pages.is_empty() {
            self.pages[z / PAGE * self.page_width + x / PAGE]
                .as_ref()
                .map_or(0, |page| page[z % PAGE * PAGE + x % PAGE])
        } else {
            0
        }
    }
}
