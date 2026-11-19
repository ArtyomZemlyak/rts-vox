use super::*;

#[test]
fn diagonal_crater_banks_outside_the_tracks_cannot_support_the_hull() {
    struct Trench {
        diagonal: i32,
        banks: bool,
    }
    impl voxel::Generator for Trench {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            let across = (x as i32 - 128) - self.diagonal * (z as i32 - 128);
            if y < 1 || (self.banks && y < 64 && across.abs() > 23) {
                1
            } else {
                0
            }
        }
    }
    let shape = Shape {
        continuous: false, detail: 0,
        half: [30, 15],
        support: [25, 12],
        clearance: Fx::from_int(4),
        step: 3,
        min_up: Fx::from_ratio(91, 100),
        height: 30,
        span: 5,
        reach: 0,
        grade_slack: Fx::ZERO,
        ramp: Fx::from_ratio(60, 100),
    };
    for diagonal in [-1i32, 1] {
        let rubble: Rubble = (0..256)
            .flat_map(|x| {
                (0..256).filter_map(move |z| {
                    (((x - 128) - diagonal * (z - 128)).abs() > 23).then_some(([x, z], 64))
                })
            })
            .collect();
        for banks in [false, true] {
            let world = World::new(256, 96, 256, 100, Box::new(Trench { diagonal, banks }));
            world.prepare_navigation([4, 5]);
            let empty = Rubble::new();
            let rubble = if banks { &empty } else { &rubble };
            for head in [
                angle::FULL / 8,
                angle::FULL * 3 / 8,
                angle::FULL * 5 / 8,
                angle::FULL * 7 / 8,
            ] {
                if (head == angle::FULL / 8 || head == angle::FULL * 5 / 8) != (diagonal == 1) {
                    continue;
                }
                for fraction in [
                    Fx::ZERO,
                    Fx::from_ratio(1, 4),
                    Fx::from_ratio(1, 2),
                    Fx::from_ratio(3, 4),
                ] {
                    let at = [
                        Fx::from_int(128).add(fraction),
                        Fx::from_int(128).add(fraction.mul(Fx::from_int(diagonal))),
                    ];
                    let mut fast = Probe::with_yield(&world, &[4, 5]).with_rubble(rubble);
                    let mut full = Probe::new(&world).with_rubble(rubble);
                    for _ in 0..2 {
                        assert_eq!(
                            fast.support_floors(at, head, shape),
                            [Fx::ONE; 4],
                            "banks={banks} diagonal={diagonal} head={head} fraction={fraction:?}"
                        );
                        assert_eq!(full.support_floors(at, head, shape), [Fx::ONE; 4]);
                    }
                }
            }
        }
    }
}

#[test]
fn uniform_contact_proof_covers_the_rotated_patch_corners() {
    struct Floor;
    impl voxel::Generator for Floor {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < 1 {
                1
            } else {
                0
            }
        }
    }
    let world = World::new(256, 32, 256, 100, Box::new(Floor));
    world.prepare_navigation([4, 5]);
    let rubble = Rubble::from([([128, 139], 16)]);
    let shape = Shape {
        continuous: false, detail: 0,
        half: [8, 8],
        support: [0, 0],
        clearance: Fx::ONE,
        step: 8,
        min_up: Fx::ONE,
        height: 8,
        span: 0,
        reach: 0,
        grade_slack: Fx::ZERO,
        ramp: Fx::ZERO,
    };
    let at = [Fx::from_int(128); 2];
    let full = Probe::new(&world)
        .with_rubble(&rubble)
        .support_floors(at, angle::FULL / 8, shape);
    let fast = Probe::with_yield(&world, &[4, 5])
        .with_rubble(&rubble)
        .support_floors(at, angle::FULL / 8, shape);
    assert!(full.iter().all(|f| *f > Fx::ONE));
    assert_eq!(fast, full, "uniform proof missed the rotated corner");
}

#[test]
fn rubble_rectangle_tree_is_lazy_and_mutations_invalidate_it() {
    let mut r = Rubble::from_sorted((0..1024).map(|x| ([x, 0], (x % 97) as u16)).collect());
    assert!(r.1.get().is_none());
    assert_eq!(r.get(&[511, 0]), Some(&26));
    assert_eq!(r.iter().count(), 1024);
    let cold = r.clone();
    assert_eq!(r, cold);
    assert!(r.1.get().is_none() && cold.1.get().is_none());
    assert!(r.touches([511, 0], [511, 0]));
    let first = r.1.get().unwrap() as *const _;
    assert_eq!(r.upper_in([511, 0], [511, 0]), 26);
    assert_eq!(r.1.get().unwrap() as *const _, first);
    let warm = r.clone();
    assert!(warm.1.get().is_some());
    assert_eq!(r.insert([511, 0], u16::MAX), Some(26));
    assert!(r.1.get().is_none());
    assert_eq!(r.upper_in([511, 0], [511, 0]), u16::MAX);
    assert_eq!(warm.upper_in([511, 0], [511, 0]), 26);
    assert_eq!(r.remove(&[511, 0]), Some(u16::MAX));
    assert!(r.1.get().is_none());
    assert!(!r.touches([511, 0], [511, 0]));
    // Threshold crossing never retains a tree for a different column set.
    for x in 0..512 {
        r.remove(&[x, 0]);
    }
    assert!(r.1.get().is_none());
    assert!(r.touches([700, 0], [700, 0]));
    assert!(r.1.get().is_none());
}

#[test]
fn concurrent_rectangle_readers_share_one_exact_rubble_tree() {
    let r = Rubble::from_sorted((0..2048).map(|x| ([x, 0], (x % 97) as u16)).collect());
    let gate = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    gate.wait();
                    assert_eq!(r.upper_in([0, 0], [2047, 0]), 96);
                    assert!(r.touches([17, 0], [17, 0]));
                    r.1.get().unwrap() as *const _ as usize
                })
            })
            .collect();
        let trees: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        assert!(trees.iter().all(|&tree| tree == trees[0]));
    });
}

struct Flat;
impl voxel::Generator for Flat {
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
        continuous: false, detail: 0,
        half: [48, 40],
        support: [28, 24],
        clearance: Fx::from_int(12),
        step: 4,
        min_up: Fx::from_ratio(9, 10),
        height: 30,
        span: 8,
        reach: 0,
        grade_slack: Fx::ZERO,
        ramp: Fx::from_ratio(3, 5),
    }
}
fn signature(p: Pose) -> (Fx, [Fx; 4], usize, bool, bool, usize) {
    (p.height, p.floors, p.blocked, p.steep, p.tilt, p.crush)
}

#[test]
fn rubble_box_upper_matches_full_scan_across_sparse_and_dense_strips() {
    let rubble = Rubble::from_sorted(
        (-40..80)
            .flat_map(|x| (-30..40).map(move |z| ([x, z], ((x * x + z * z) % 120) as u16)))
            .collect(),
    );
    for x in [-100, -40, 0, 17, 79, 100] {
        for z in [-100, -30, 0, 39, 100] {
            for width in [0, 1, 8, 50, 200] {
                for depth in [0, 1, 20, 150] {
                    let lo = [x, z];
                    let hi = [x + width, z + depth];
                    let expected = rubble
                        .iter()
                        .filter(|(p, _)| (0..2).all(|a| p[a] >= lo[a] && p[a] <= hi[a]))
                        .map(|(_, top)| *top)
                        .max()
                        .unwrap_or(0);
                    assert_eq!(rubble.upper_in(lo, hi), expected, "{lo:?}..{hi:?}");
                }
            }
        }
    }
    assert_eq!(rubble.upper_in([3, 4], [2, 4]), 0);
    assert_eq!(Rubble::new().upper_in([0; 2], [100; 2]), 0);
}

#[test]
fn rubble_rectangles_follow_replacements_removals_clones_and_small_map_transition() {
    let check = |r: &Rubble| {
        for x in [-40, -7, 0, 15, 40] {
            for z in [-100, -5, 0, 63, 100] {
                for width in [0, 3, 70] {
                    let lo = [x, z];
                    let hi = [x + width, z + width];
                    let selected: Vec<_> = r
                        .iter()
                        .filter(|(p, _)| (0..2).all(|a| p[a] >= lo[a] && p[a] <= hi[a]))
                        .collect();
                    assert_eq!(r.touches(lo, hi), !selected.is_empty());
                    assert_eq!(
                        r.upper_in(lo, hi),
                        selected.iter().map(|(_, h)| **h).max().unwrap_or(0)
                    );
                }
            }
        }
        assert!(!r.touches([0, 1], [0, 0]));
        assert_eq!(r.upper_in([0, 1], [0, 0]), 0);
    };
    let mut columns: Vec<_> = (-8..8)
        .flat_map(|x| (-16..48).map(move |z| ([x, z], ((x * x + z * z) % 97) as u16)))
        .collect();
    columns.extend([([i32::MIN; 2], 0), ([i32::MAX; 2], u16::MAX)]);
    columns.sort_unstable_by_key(|&(p, _)| p);
    let mut r = Rubble::from_sorted(columns);
    check(&r);
    // Zero height still denotes a present column; membership is independent
    // of the maximum value. The maximum must decrease after replacement.
    assert!(r.touches([i32::MIN; 2], [i32::MIN; 2]));
    assert_eq!(r.upper_in([i32::MAX; 2], [i32::MAX; 2]), u16::MAX);
    assert_eq!(r.insert([i32::MAX; 2], 0), Some(u16::MAX));
    assert_eq!(r.upper_in([i32::MAX; 2], [i32::MAX; 2]), 0);
    let clone = r.clone();
    assert_eq!(r.insert([0, 0], u16::MAX), Some(0));
    check(&r);
    check(&clone);
    assert_eq!(clone.get(&[0, 0]), Some(&0));
    assert_eq!(r.remove(&[0, 0]), Some(u16::MAX));
    let keys: Vec<_> = r.iter().map(|(p, _)| *p).collect();
    for p in &keys[..600] {
        assert!(r.remove(p).is_some());
    }
    assert!(r.1.get().is_none(), "small maps must release the old index");
    check(&r);
    for &p in &keys[..600] {
        r.insert(p, 1);
    }
    assert!(
        r.1.get().is_none(),
        "mutations leave tree construction deferred"
    );
    check(&r);
    assert!(r.1.get().is_some());
    // Collection retains the last input value just like ordered insertion,
    // and equality depends only on the canonical columns.
    let expected = r.clone();
    let mut entries: Vec<_> = r.iter().map(|(p, h)| (*p, *h)).collect();
    entries.reverse();
    entries.insert(0, ([0, 1], 999));
    let collected: Rubble = entries.into_iter().collect();
    assert_eq!(expected, collected);
    check(&collected);
}

#[test]
fn local_rubble_upper_query_does_not_scan_distant_sleeping_history() {
    for height in [64, 1024, 4096] {
        let r = Rubble::from_sorted(
            (0..64)
                .flat_map(|x| (0..height).map(move |z| ([x, z], if z < 4 { 7 } else { 99 })))
                .collect(),
        );
        let mut examined = 0;
        let actual = r
            .rectangle_index()
            .unwrap()
            .upper(&r.0, [31, 1], [31, 2], &mut || examined += 1);
        assert_eq!(actual, 7, "a high distant heap cannot enter the answer");
        assert!(examined <= 64, "height={height}, examined={examined}");
        assert!(r.touches([31, 1], [31, 2]));
        assert!(!r.touches([31, -3], [31, -2]));
    }
}

#[test]
fn goal_refusal_matches_full_geometry_with_trees_debris_tilt_and_live_edits() {
    let mut world = World::new(384, 64, 384, 100_000, Box::new(Flat));
    for x in 152..160 {
        for z in 176..184 {
            for y in 6..32 {
                world.queue_edit(voxel::Edit::put(x, y, z, 4));
            }
        }
    }
    while world.queued() > 0 {
        world.flush();
    }
    for hard_obstacle in [false, true] {
        if hard_obstacle {
            for y in 6..32 {
                world.queue_edit(voxel::Edit::put(174, y, 178, 3));
            }
            while world.queued() > 0 {
                world.flush();
            }
        }
        world.prepare_navigation([4, 5]);
        for top in [0, 6, 8, 18, 40] {
            let rubble = Rubble::from_sorted(
                (140..184)
                    .flat_map(|x| (150..198).map(move |z| ([x, z], top)))
                    .collect(),
            );
            for head in [0, 1, 1234, angle::FULL / 8, angle::FULL / 3] {
                let pos = [
                    Fx::from_int(174).add(Fx::from_ratio(1, 3)),
                    Fx::from_int(178).sub(Fx::from_ratio(1, 5)),
                ];
                let mut goal = Probe::with_yield(&world, &[4, 5]).with_rubble(&rubble);
                // Reversed material order disables prepared tile proofs without
                // changing which materials yield: this is the exact scan oracle.
                let mut exact = Probe::with_yield(&world, &[5, 4]).with_rubble(&rubble);
                assert_eq!(
                    goal.goal_refused(pos, head, shape()),
                    exact.route_pose(pos, head, shape()).refused(),
                    "hard={hard_obstacle} top={top} head={head}"
                );
                let mut full = Probe::with_yield(&world, &[4, 5]).with_rubble(&rubble);
                let mut exact = Probe::with_yield(&world, &[5, 4]).with_rubble(&rubble);
                assert_eq!(
                    signature(full.pose(pos, head, shape())),
                    signature(exact.pose(pos, head, shape())),
                    "full pose hard={hard_obstacle} top={top} head={head}"
                );
            }
        }
    }
}

#[test]
fn goal_probe_omits_crush_work_but_full_route_keeps_vegetation_cost() {
    let mut world = World::new(384, 64, 384, 100_000, Box::new(Flat));
    for x in 130..220 {
        for z in 140..218 {
            world.queue_edit(voxel::Edit::put(x, 16, z, 4));
        }
    }
    while world.queued() > 0 {
        world.flush();
    }
    world.prepare_navigation([4, 5]);
    let pos = [174, 178].map(Fx::from_int);
    let rubble = Rubble::from_sorted(
        (140..184)
            .flat_map(|x| (150..198).map(move |z| ([x, z], 8)))
            .collect(),
    );
    let mut goal = Probe::with_yield(&world, &[4, 5]).with_rubble(&rubble);
    assert!(!goal.goal_refused(pos, 0, shape()));
    let mut full = Probe::with_yield(&world, &[4, 5]).with_rubble(&rubble);
    let result = full.route_pose(pos, 0, shape());
    assert!(!result.refused());
    assert!(
        result.crush > 4000,
        "route lost the forest cost: {}",
        result.crush
    );
    assert!(
        goal.columns * 4 < full.columns,
        "goal columns={} full={}",
        goal.columns,
        full.columns
    );
    assert_eq!(
        world.prepared_floor_upper([130, 140], [219, 217], [4, 5]),
        Some(6)
    );
    assert_eq!(
        world.prepared_bare_upper([130, 140], [219, 217], [4, 5]),
        None
    );
    world.queue_edit(voxel::Edit::put(174, 40, 178, 3));
    while world.queued() > 0 {
        world.flush();
    }
    assert_eq!(
        world.prepared_floor_upper([130, 140], [219, 217], [4, 5]),
        None
    );
    world.prepare_navigation([4, 5]);
    assert_eq!(
        world.prepared_floor_upper([130, 140], [219, 217], [4, 5]),
        Some(41)
    );
    assert!(Probe::with_yield(&world, &[4, 5])
        .with_rubble(&rubble)
        .goal_refused(pos, 0, shape()));
}

#[test]
fn speculative_prefix_stops_at_refusal_with_exact_answers_and_less_geometry() {
    let mut world = World::new(384, 64, 384, 100_000, Box::new(Flat));
    for y in 6..41 {
        world.queue_edit(voxel::Edit::put(174, y, 178, 3));
    }
    while world.queued() > 0 {
        world.flush();
    }
    world.prepare_navigation([4, 5]);
    for head in [0, angle::FULL / 8] {
        for initial in [false, true] {
            let request = StartSweep {
                shape: shape(),
                samples: (0..4)
                    .map(|i| ([174 + i, 178].map(Fx::from_int), head))
                    .collect(),
                count_crush: true,
                height: Fx::from_int(6),
                initial,
            };
            let mut eager = Probe::for_search(&world, &[4, 5], Columns::default());
            let all = eager.route_samples(request.shape, &request.samples);
            let expected = (1..=all.len())
                .find(|&n| !request.prefix_open(&all[..n]))
                .unwrap();
            assert_eq!(expected, if initial { 2 } else { 1 });
            let mut short = Probe::for_search(&world, &[4, 5], Columns::default());
            let prefix =
                short.route_prefix(request.shape, &request.samples, |p| request.prefix_open(p));
            assert_eq!(prefix.len(), expected);
            assert_eq!(
                prefix.into_iter().map(signature).collect::<Vec<_>>(),
                all[..expected]
                    .iter()
                    .copied()
                    .map(signature)
                    .collect::<Vec<_>>()
            );
            assert!(short.samples < eager.samples);
            assert!(short.columns < eager.columns);
        }
    }
}

#[test]
fn speculative_prefix_keeps_legal_samples_and_handles_empty_requests() {
    let world = World::new(384, 64, 384, 100_000, Box::new(Flat));
    let samples: Vec<_> = (0..8)
        .map(|i| ([174 + i, 178].map(Fx::from_int), i * 256))
        .collect();
    for len in [0, 1, 4, 8] {
        let request = StartSweep {
            shape: shape(),
            samples: samples[..len].to_vec(),
            count_crush: true,
            height: Fx::from_int(6),
            initial: false,
        };
        let mut eager = Probe::for_search(&world, &[4, 5], Columns::default());
        let all = eager.route_samples(request.shape, &request.samples);
        assert!(request.prefix_open(&all));
        let mut short = Probe::for_search(&world, &[4, 5], Columns::default());
        let prefix =
            short.route_prefix(request.shape, &request.samples, |p| request.prefix_open(p));
        assert_eq!(
            prefix.into_iter().map(signature).collect::<Vec<_>>(),
            all.into_iter().map(signature).collect::<Vec<_>>()
        );
        assert_eq!(
            [short.scans, short.columns, short.samples],
            [eager.scans, eager.columns, eager.samples]
        );
    }
}


#[test]
fn median5_network_matches_sorted_order_with_ties_and_extreme_values() {
    let alphabet = [i32::MIN, -1, 0, 17, i32::MAX];
    for mut code in 0..3125 {
        let values = std::array::from_fn(|_| {let v=alphabet[code%5];code/=5;v});
        let mut sorted=values;sorted.sort();
        assert_eq!(median5(values),sorted[2],"{values:?}");
    }
}

#[test]
fn column_stack_boundary_keeps_sparse_yielding_spans_and_rubble_exact() {
    struct Crown {ceiling:u32}
    impl voxel::Generator for Crown {
        fn material(&self,x:u32,y:u32,z:u32)->Mat {
            if y<16 {2} else if x==31 && z==31 && y<self.ceiling
                && (y+1==self.ceiling || !y.is_multiple_of(11)) {4} else {AIR}
        }
    }
    for len in [1,255,256,257,520] {
        let world=World::new(64,600,64,100_000,Box::new(Crown {ceiling:15+len}));
        world.prepare_navigation([4,5]);
        let tile=world.prepared_floor_tile(3,3,[4,5]).unwrap();assert_eq!(tile,[16,16]);
        assert_eq!(world.column_ceiling(31,31)-u32::from(tile[0].saturating_sub(1)),len);
        for rubble in [Rubble::new(),Rubble::from([([31,31],60)])] {
            let fast=Probe::with_yield(&world,&[4,5]).with_rubble(&rubble);
            // The same material set on this fixture, but one key disables the
            // prepared bulk-column route and reads the actual voxels in Y.
            let reference=Probe::with_yield(&world,&[4]).with_rubble(&rubble);
            assert_eq!(fast.scan(31,31),reference.scan(31,31),"len={len}");
            assert_eq!(fast.scan(30,31),reference.scan(30,31));
        }
    }
}

#[test]
fn tile_mask_stack_boundary_matches_exact_pose_with_live_edits_and_rubble() {
    struct Crown;
    impl voxel::Generator for Crown {
        fn material(&self,x:u32,y:u32,z:u32)->Mat {
            if y<16 {2} else if x==256 && z==256 && y<52 {4} else {AIR}
        }
    }
    let mut world=World::new(512,80,512,100_000,Box::new(Crown));
    world.prepare_navigation([4,5]);
    let mut rubble=Rubble::new();
    let at=[Fx::from_int(256);2];
    let mut boundaries=[false;3];
    // Axis-aligned extents cover31*31,32*32=1024 and33*33=1089 tiles.
    // A tree prevents the whole-box shortcut, forcing the per-tile mask.
    for stage in 0..3 {
        if stage==1 {rubble.insert([256,256],60);world.queue_edit(voxel::Edit::put(250,58,250,3));world.flush();}
        if stage==2 {rubble.remove(&[256,256]);world.queue_edit(voxel::Edit::dig(250,58,250));world.flush();}
        for half in [[119,119],[123,123],[127,127],[128,128],[132,128]] {
            let shape=Shape {continuous:false,detail:0,half,support:[60,60],clearance:Fx::from_int(24),step:20,min_up:Fx::from_ratio(8,10),height:24,span:0,reach:0,grade_slack:Fx::ZERO,ramp:Fx::ZERO};
            for head in [0,1234] {
                let mut fast=Probe::with_yield(&world,&[4,5]).with_rubble(&rubble);
                let mut reference=Probe::with_yield(&world,&[4]).with_rubble(&rubble);
                let (lo,hi)=fast.extent(at,head,shape);
                let tiles=(hi[0]/8-lo[0]/8+1)*(hi[1]/8-lo[1]/8+1);
                boundaries[0]|=tiles<1024;boundaries[1]|=tiles==1024;boundaries[2]|=tiles>1024;
                assert_eq!(signature(fast.pose(at,head,shape)),signature(reference.pose(at,head,shape)),"stage={stage} half={half:?} head={head}");
                assert!(fast.columns<reference.columns,"fixture bypassed per-tile certificates");
            }
        }
    }
    assert!(boundaries.into_iter().all(|v|v),"must exercise below/at/above the stack-bitset boundary");
}

#[test]
fn sloped_forest_pose_crush_flag_and_zero_cost_hunt_match_reference_oracle() {
    struct HillyForest;
    impl voxel::Generator for HillyForest {
        fn batches_columns(&self) -> bool {
            true
        }
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            let hill_of = |px: u32, pz: u32| -> u32 {
                let wave = ((px as i32 * 3 + pz as i32 * 2) / 24).rem_euclid(20);
                16 + (if wave <= 10 { wave } else { 20 - wave }) as u32
            };
            let hill = hill_of(x, z);
            if y < hill {
                return 2;
            }
            let gx = x / 24;
            let gz = z / 24;
            let tx = gx * 24 + 12;
            let tz = gz * 24 + 12;
            if x >= 48 && x < 336 && z >= 48 && z < 336 && (gx + gz) % 2 == 0 {
                let top = hill_of(tx, tz);
                if x.abs_diff(tx) <= 1 && z.abs_diff(tz) <= 1 && y >= top && y < top + 18 {
                    return 5;
                }
                if x.abs_diff(tx) <= 4 && z.abs_diff(tz) <= 4 && y >= top + 11 && y < top + 20 {
                    return 4;
                }
            }
            AIR
        }
    }
    let mut world = World::new(384, 96, 384, 100_000, Box::new(HillyForest));
    world.prepare_navigation([4, 5]);
    let rubble = Rubble::from([([180, 180], 26), ([220, 196], 25)]);
    let shape = Shape {
        continuous: false,
        detail: 4,
        half: [72, 72],
        support: [36, 36],
        clearance: Fx::from_int(18),
        step: 10,
        min_up: Fx::from_ratio(82, 100),
        height: 44,
        span: 6,
        reach: 0,
        grade_slack: Fx::ZERO,
        ramp: Fx::from_ratio(55, 100),
    };
    let flag_sig = |p: Pose| (p.height, p.floors, p.blocked, p.steep, p.tilt, p.crush > 0);
    // Material 6 is absent on this map, so &[4, 5, 6] has the exact same
    // yielding set while bypassing two-material prepared tile shortcuts.
    for z in (80..300).step_by(17) {
        for x in (80..300).step_by(19) {
            let fast = Probe::with_yield(&world, &[4, 5]).with_rubble(&rubble);
            let slow = Probe::with_yield(&world, &[4, 5, 6]).with_rubble(&rubble);
            assert_eq!(fast.scan(x, z), slow.scan(x, z), "scan mismatch at ({x}, {z})");
        }
    }
    let mut cached_columns = Columns::default();
    for stage in 0..2 {
        let mut changed = Changed::default();
        if stage == 1 {
            // Fell one trunk voxel so its 8x8 tile becomes unprepared and
            // GroundCells exercises selective per-entry invalidation.
            world.queue_edit(voxel::Edit::dig(180, 28, 180));
            world.flush();
            changed.column(180, 180);
        }
        let mut fast =
            Probe::for_local(&world, &[4, 5], cached_columns, &changed).with_rubble(&rubble);
        let mut exact = Probe::with_yield(&world, &[4, 5, 6]).with_rubble(&rubble);
        for (px, pz, head) in [
            (132, 192, 0),
            (184, 188, angle::FULL / 8),
            (236, 204, angle::FULL * 3 / 16),
        ] {
            let at = [Fx::from_int(px), Fx::from_int(pz)];
            let got = fast.pose_crush_flag(at, head, shape);
            let want = exact.pose(at, head, shape);
            assert_eq!(flag_sig(got), flag_sig(want), "stage={stage} at=({px},{pz})");
        }
        assert!(
            fast.columns < exact.columns,
            "pose_crush_flag should skip floor-clear tiles after first tree column"
        );
        cached_columns = fast.take_columns();
    }
    let cfg_zero = Search {
        cell: 8,
        min: [16, 16],
        max: [368, 368],
        nodes: 4000,
        crush_cost: 0,
        crush_cap: 0,
        margin: 2,
        heuristic_pct: 200,
    };
    let from = [Fx::from_int(112), Fx::from_int(192)];
    let goal = [288, 192];
    let full = plan(
        Probe::for_search(&world, &[4, 5], Columns::default()).with_rubble(&rubble),
        shape,
        from,
        0,
        goal,
        cfg_zero,
        |_, _| true,
    );
    let reference = plan(
        Probe::with_yield(&world, &[4, 5, 6]).with_rubble(&rubble),
        shape,
        from,
        0,
        goal,
        cfg_zero,
        |_, _| true,
    );
    assert!(!full.path.is_empty());
    assert_eq!(
        (
            &full.path,
            full.partial,
            full.stranded,
            full.exhausted,
            full.nodes,
            full.poses,
        ),
        (
            &reference.path,
            reference.partial,
            reference.stranded,
            reference.exhausted,
            reference.nodes,
            reference.poses,
        )
    );
    let priced = plan(
        Probe::for_search(&world, &[4, 5], Columns::default()).with_rubble(&rubble),
        shape,
        from,
        0,
        goal,
        Search {
            crush_cost: 1,
            crush_cap: 64,
            ..cfg_zero
        },
        |_, _| true,
    );
    assert!(
        full.scans < priced.scans,
        "zero-crush-cost search must skip forested belly column scans: {} vs {}",
        full.scans,
        priced.scans
    );
    // Sliced execution across save/load must produce the exact same route.
    let mut probe = Probe::for_search(&world, &[4, 5], Columns::default()).with_rubble(&rubble);
    let (before_save_poses, sliced) =
        match Hunt::start_bounded(&mut probe, shape, from, 0, goal, cfg_zero, &|_, _| true, 7) {
            Hunting::Done(r) => (0, r),
            Hunting::Open(hunt) => {
                let initial_poses = hunt.poses();
                let mut blob = fx::blob::Blob::new();
                hunt.save(&mut blob);
                let mut scan = fx::blob::Scan::new(&blob.v);
                let mut restored = Hunt::load_with_surface(
                    &mut scan,
                    &[4, 5],
                    shape.detail,
                    shape.continuous,
                )
                .unwrap();
                loop {
                    if let Some(done) = restored.advance_in(&world, &rubble, &|_, _| true, 11) {
                        break (initial_poses, done);
                    }
                }
            }
        };
    assert_eq!(
        (&sliced.path, sliced.nodes, before_save_poses + sliced.poses),
        (&full.path, full.nodes, full.poses)
    );
}
