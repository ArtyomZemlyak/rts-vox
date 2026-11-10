//! Preparation cost of terrain-only continuations, separate from motion.
use super::*;
use modding::range::RangeUnitKind;

struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _x: u32, y: u32, _z: u32) -> u16 {
        u16::from(y < 16)
    }
}

fn fixture() -> (World, UnitBody, Motion, nav::vehicle::Rubble) {
    let w = World::new(1024, 128, 1024, 100, Box::new(Flat));
    let b = voxel::unit::tank();
    let mut m = Motion::new([280, 16, 280], RangeUnitKind::Tank, &b);
    let rubble = nav::vehicle::Rubble::new();
    m.allow_coarse(Some(0));
    assert!(m.move_to(&w, [880, 880], &b, &[], &rubble));
    m.allow_coarse(None);
    assert_eq!(m.coarse_waited(), Some(0));
    (w, b, m, rubble)
}

fn saved(m: &Motion) -> Vec<u8> {
    let mut b = Blob::new();
    m.save(&mut b);
    m.save_coarse(&mut b);
    b.v
}

#[test]
fn terrain_continuations_never_request_hull_obstacles() {
    let (w, b, mut m, rubble) = fixture();
    let changed = nav::vehicle::Changed::default();
    let unwanted = || -> Vec<Obstacle> { panic!("terrain-only search requested hull obstacles") };
    assert_eq!(
        m.continue_coarse(&w, &b, unwanted, &rubble, 1, &changed, 0, true, false),
        0
    );
    assert_eq!(m.coarse_waited(), Some(1));
    assert_eq!(
        m.continue_coarse(&w, &b, unwanted, &rubble, 1, &changed, 1, false, false),
        0
    );
    assert_eq!(m.coarse_waited(), Some(1));
    for tick in 2..100 {
        m.continue_coarse(&w, &b, unwanted, &rubble, tick, &changed, 4096, true, false);
        if m.coarse.is_none() {
            break;
        }
    }
    assert!(m.coarse.is_none() && m.path.last() == Some(&[880, 880]));
    m.continue_coarse(&w, &b, unwanted, &rubble, 100, &changed, 0, true, false);
    let (_, _, mut displaced, _) = fixture();
    displaced.pos[0] = displaced.pos[0].add(Fx::from_int(displaced.navigation.retarget_units + 1));
    displaced.continue_coarse(&w, &b, unwanted, &rubble, 1, &changed, 4096, true, false);
    assert!(displaced.coarse.is_none());
    assert_eq!(displaced.destination, Some([880, 880]));
}

#[test]
fn fine_fallback_requests_obstacles_once_and_matches_the_direct_fine_search() {
    struct Wall;
    impl voxel::Generator for Wall {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            if y < 16 {
                1
            } else if (480..496).contains(&x) && z < 600 && y < 60 {
                3
            } else {
                0
            }
        }
    }
    let changed = nav::vehicle::Changed::default();
    for refused in [false, true] {
        let (mut w, b, mut actual, rubble) = fixture();
        let budget = if refused { u64::MAX } else { 1 };
        if refused {
            w = World::new(1024, 128, 1024, 100, Box::new(Wall));
            let cfg = nav::vehicle::coarse::Config {
                nodes: 1,
                ..actual.coarse_config()
            };
            let search = nav::vehicle::coarse::Search::new(
                [w.nx, w.nz],
                actual.shape(&b),
                [actual.pos[0], actual.pos[2]],
                [880, 880],
                cfg,
            )
            .unwrap();
            assert!(search
                .clone()
                .advance(&w, &rubble, u64::MAX)
                .0
                .unwrap()
                .is_err());
            actual.coarse = Some(Box::new(CoarsePending { search, waited: 0 }));
        }
        actual.blocked_ticks = 1; // Fine fallback must actually use local blockers.
        let obstacle = Obstacle { height: Fx::ZERO,
            moving: false,
            pos: [Fx::from_int(315), Fx::ZERO, Fx::from_int(315)],
            head: 0,
            half: [Fx::from_int(8); 2],
        };
        let mut expected = actual.clone();
        expected.coarse = None;
        expected.start_fine(&w, [880, 880], &b, &[obstacle], &rubble, None, |_| {});
        let mut calls = 0;
        actual.continue_coarse(
            &w,
            &b,
            || {
                calls += 1;
                vec![obstacle]
            },
            &rubble,
            1,
            &changed,
            budget,
            true,
            true,
        );
        assert_eq!(calls, 1, "refused={refused}");
        assert_eq!(saved(&actual), saved(&expected), "refused={refused}");
    }
}

/// Isolate the two continuation call sites' preparation. These hulls do not
/// move and the next corridor costs more than either offered budget.
#[test]
#[ignore = "manual coarse obstacle scaling measurement; run alone in release"]
fn measure_coarse_obstacle_scaling() {
    use std::hint::black_box;
    use std::time::Instant;
    let (w, b, m, rubble) = fixture();
    let changed = nav::vehicle::Changed::default();
    println!("units,budget,median_us,min_us,max_us");
    for count in [64, 512, 5000] {
        let positions: Vec<_> = (0..count).map(|_| Obstacle::of(&m, &b)).collect();
        let mut motions = vec![m.clone(); count];
        for budget in [0, 1] {
            let mut sample = || {
                for (id, motion) in motions.iter_mut().enumerate() {
                    let others = || {
                        positions
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| *i != id)
                            .map(|(_, p)| *p)
                            .collect()
                    };
                    let spent = motion.continue_coarse(
                        black_box(&w),
                        &b,
                        others,
                        &rubble,
                        1,
                        &changed,
                        black_box(budget),
                        true,
                        false,
                    );
                    assert_eq!(spent, 0);
                }
                black_box(&motions);
            };
            for _ in 0..8 {
                sample();
            }
            let mut times = Vec::new();
            for _ in 0..9 {
                let start = Instant::now();
                sample();
                times.push(start.elapsed().as_secs_f64() * 1e6);
            }
            times.sort_by(f64::total_cmp);
            println!(
                "{count},{budget},{:.3},{:.3},{:.3}",
                times[4], times[0], times[8]
            );
            assert!(motions.iter().all(|m| m.coarse_work() == Some(0)));
        }
    }
}
