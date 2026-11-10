//! Wide local continuations keep ordered clearance callbacks and search state.
use super::*;
use modding::range::RangeUnitKind;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Ground;
impl voxel::Generator for Ground {
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        let height = if (270..310).contains(&x) { 20 } else { 16 };
        if y < height {
            1
        } else if (335..340).contains(&x) && (290..310).contains(&z) && y < 28 {
            4
        } else {
            0
        }
    }
}
fn bytes(m: &Motion) -> Vec<u8> {
    let mut b = Blob::new();
    m.save(&mut b);
    b.v
}
fn queries(q: &RefCell<Vec<HuntClearance>>) -> Vec<([Fx; 2], i32, bool)> {
    q.borrow()
        .iter()
        .map(|q| (q.position, q.head, q.clear))
        .collect()
}

#[test]
fn wide_local_sweeps_preserve_clearance_order_state_and_resume_across_workers() {
    let body = voxel::unit::spider();
    let rubble = nav::vehicle::Rubble::new();
    let prepared = AtomicUsize::new(0);
    let mut refused = 0;
    let mut edits = 0;
    let mut reloads = 0;
    for workers in [1, 4, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        for obstacle_x in [90, 240, 380, 520] {
            let mut w = World::new(640, 96, 640, 4096, Box::new(Ground));
            w.prepare_navigation([4, 5]);
            let blocker = Obstacle { height: Fx::ZERO,
                moving: false,
                pos: [
                    Fx::from_int(obstacle_x),
                    Fx::from_int(16),
                    Fx::from_int(300),
                ],
                head: angle::FULL / 8,
                half: [Fx::from_int(6), Fx::from_int(10)],
            };
            let mut old = Motion::new([160, 16, 300], RangeUnitKind::Spider, &body);
            assert!(old.shape(&body).half.iter().all(|&n| n >= 40));
            old.navigation.global_nodes = 0;
            old.navigation.search_nodes_per_tick = 1;
            old.blocked_ticks = 1;
            assert!(old.move_to(&w, [490, 300], &body, &[blocker], &rubble));
            assert!(old.searching());
            let mut fast = old.clone();
            for tick in 0..32 {
                if !old.searching() {
                    break;
                }
                assert!(fast.searching());
                old.allow_resume(64);
                fast.allow_resume(64);
                let changed = if tick == 5 {
                    edits += 1;
                    assert_ne!(w.material(280, 19, 300), 0);
                    w.queue_dig(280, 19, 300);
                    w.flush();
                    assert_eq!(w.material(280, 19, 300), 0);
                    let mut c = nav::vehicle::Changed::default();
                    c.column(280, 300);
                    c
                } else {
                    nav::vehicle::Changed::default()
                };
                let old_checked = RefCell::new(Vec::new());
                let old_hunt = old.hunt.take().unwrap();
                let a = old.advance_hunt_mode::<false>(
                    old_hunt,
                    &w,
                    &body,
                    &[blocker],
                    &rubble,
                    &changed,
                    Some(&old_checked),
                    |_| {},
                    |_, _| {},
                );
                let owned = fast.hunt.take().unwrap();
                let (b, checked) = pool.install(|| {
                    let checked = RefCell::new(Vec::new());
                    let result = fast.advance_hunt_mode::<true>(
                        owned,
                        &w,
                        &body,
                        &[blocker],
                        &rubble,
                        &changed,
                        Some(&checked),
                        |_| {},
                        |_, begin| {
                            if begin {
                                prepared.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                    );
                    (result, queries(&checked))
                });
                refused += checked.iter().filter(|q| !q.2).count();
                assert_eq!(queries(&old_checked), checked);
                old.apply_hunt(a, tick);
                fast.apply_hunt(b, tick);
                assert!(
                    bytes(&old) == bytes(&fast),
                    "workers={workers}, blocker={obstacle_x}, tick={tick}"
                );
                if tick == 3 {
                    reloads += 1;
                    let saved = bytes(&fast);
                    assert!(fast.load(&mut Scan::new(&saved)));
                    assert!(old.load(&mut Scan::new(&saved)));
                    assert_eq!(bytes(&old), bytes(&fast));
                }
            }
        }
    }
    assert!(
        prepared.load(Ordering::Relaxed) > 0,
        "wide local geometry must actually use worker batches"
    );
    assert!(
        refused > 0,
        "nearby hull clearance must actually refuse candidates"
    );
    assert!(edits > 0, "search must continue across a terrain edit");
    assert!(
        reloads > 0,
        "search must continue after loading saved state"
    );
}
