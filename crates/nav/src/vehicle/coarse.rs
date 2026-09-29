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
    if cfg.cell < 8 || cfg.nodes == 0 {
        return None;
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
    let start = index(origin.map(|v| v + cfg.cell / 2))?;
    let end = index(to.map(|v| v + cfg.cell / 2))?;
    // A broad travel corridor, not an oriented turning/clearance proof.
    // Short maneuvers and actual movement keep the detailed footprint probe.
    let radius = shape.half[0].min(shape.half[1]) + 1;
    const PAGE: usize = 32;
    let tiles = [
        (world.nx as usize).div_ceil(8),
        (world.nz as usize).div_ceil(8),
    ];
    // A kilometre map needs only about 3 MiB for direct tile lookups. Keep
    // that fast path, but cap its size: a 10 km map would otherwise allocate
    // hundreds of MiB for every route even with only a few debris columns.
    let dense = !rubble.0.is_empty() && tiles[0] * tiles[1] <= 2_000_000;
    let mut debris_dense = if dense {
        vec![0u16; tiles[0] * tiles[1]]
    } else {
        Vec::new()
    };
    let page_width = tiles[0].div_ceil(PAGE);
    let mut debris_pages: Vec<Option<Box<[u16; PAGE * PAGE]>>> = if rubble.0.is_empty() || dense {
        Vec::new()
    } else {
        vec![None; page_width * tiles[1].div_ceil(PAGE)]
    };
    for &(p, top) in &rubble.0 {
        let x = p[0].div_euclid(8);
        let z = p[1].div_euclid(8);
        if x < 0 || z < 0 || x as usize >= tiles[0] || z as usize >= tiles[1] {
            continue;
        }
        let (x, z) = (x as usize, z as usize);
        let pile = if dense {
            &mut debris_dense[z * tiles[0] + x]
        } else {
            let page = &mut debris_pages[z / PAGE * page_width + x / PAGE];
            let page = page.get_or_insert_with(|| Box::new([0; PAGE * PAGE]));
            &mut page[z % PAGE * PAGE + x % PAGE]
        };
        *pile = (*pile).max(top);
    }
    let tile = |x: i32, z: i32| {
        let [bottom, mut top] = world.floor_tile(x as u32, z as u32, cfg.yielding).unwrap();
        if !debris_dense.is_empty() {
            top = top.max(debris_dense[z as usize * tiles[0] + x as usize]);
        } else if !debris_pages.is_empty() {
            let (x, z) = (x as usize, z as usize);
            if let Some(page) = &debris_pages[z / PAGE * page_width + x / PAGE] {
                top = top.max(page[z % PAGE * PAGE + x % PAGE]);
            }
        }
        [bottom, top]
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
                    let [bottom, mut top] =
                        world.floor_tile(x as u32, z as u32, cfg.yielding).unwrap();
                    if !debris_dense.is_empty() {
                        top = top.max(debris_dense[z as usize * tiles[0] + x as usize]);
                    } else if !debris_pages.is_empty() {
                        let (x, z) = (x as usize, z as usize);
                        if let Some(page) = &debris_pages[z / PAGE * page_width + x / PAGE] {
                            top = top.max(page[z % PAGE * PAGE + x % PAGE]);
                        }
                    }
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
        return Some(Route {
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
    if !clear(origin, point(start)) || !clear(point(end), to) {
        return None;
    }
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
    for (side, root) in [start, end].into_iter().enumerate() {
        distance[root][side] = 0;
        open[side].push(Reverse((heuristic(root, side), 0, root)));
    }
    let mut expanded = 0;
    loop {
        // Alternate the two frontiers under one shared deterministic cap.
        // A remote obstacle cannot monopolize a one-sided greedy search.
        let side = expanded % 2;
        let Reverse((_, cost, i)) = open[side].pop()?;
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
            // Join the first grid leg directly where possible, without a
            // needless backwards move to the snapped start node.
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
            if compact.len() > 6400 {
                return None;
            }
            return Some(Route {
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
            return None;
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
        assert!(plan(&world, &Rubble::new(), shape(), from, to, config()).is_none());
        assert!(plan(
            &world,
            &Rubble::new(),
            shape(),
            from,
            to,
            Config {
                local_grade: true,
                ..config()
            }
        )
        .is_none());
        world.restore_pristine();
        assert!(plan(&world, &Rubble::new(), shape(), from, to, config()).is_some());
        let rubble = Rubble::from_sorted((0..256).map(|z| ([256, z], 26)).collect());
        assert!(plan(&world, &rubble, shape(), from, to, config()).is_none());
        assert!(plan(&world, &Rubble::new(), shape(), from, to, config()).is_some());
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
            plan(&world, &r, shape(), from, to, config()).is_some(),
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
            plan(&world, &r, shape(), from, to, config()).is_none(),
            "sealed wall crossed"
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
            plan(&world, &r, shape(), from, to, config()).is_some(),
            "fresh hole not seen"
        );
    }
}
