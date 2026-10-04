use super::*;

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
        half: [48, 40],
        support: [28, 24],
        clearance: Fx::from_int(12),
        step: 4,
        min_up: Fx::from_ratio(9, 10),
        height: 30,
        span: 8,
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
