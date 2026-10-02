use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Terrain(Arc<AtomicUsize>);
impl Terrain {
    fn at(x: u32, y: u32, z: u32) -> Mat {
        if y < 5 + (x + 3 * z) % 9 {
            2
        } else if y == 34 && x.is_multiple_of(3) && z.is_multiple_of(5) {
            7 // Disconnected overhangs across chunk boundaries.
        } else {
            AIR
        }
    }
}
impl Generator for Terrain {
    fn material(&self, x: u32, y: u32, z: u32) -> Mat {
        self.0.fetch_add(1, Ordering::Relaxed);
        Self::at(x, y, z)
    }
    fn column(&self, x: u32, z: u32, y0: u32, out: &mut [Mat]) {
        for (i, value) in out.iter_mut().enumerate() {
            *value = Self::at(x, y0 + i as u32, z);
        }
    }
}
fn world(calls: &Arc<AtomicUsize>) -> World {
    World::new(73, 67, 45, 100, Box::new(Terrain(Arc::clone(calls))))
}

#[test]
fn empty_material_reads_do_not_evaluate_generator_on_any_worker() {
    let calls = Arc::new(AtomicUsize::new(0));
    let w = world(&calls);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    for workers in [1, 2, 4, 8] {
        std::thread::scope(|scope| {
            for worker in 0..workers {
                let w = &w;
                scope.spawn(move || {
                    let mut cursor = World::mat_cursor();
                    for x in (worker..w.nx).step_by(workers as usize) {
                        for z in 0..w.nz {
                            assert_eq!(w.material(x, 63, z), AIR);
                            assert_eq!(w.material_cached(&mut cursor, x, 63, z), AIR);
                        }
                    }
                    for p in [[w.nx, 0, 0], [0, w.ny, 0], [0, 0, w.nz], [u32::MAX; 3]] {
                        assert_eq!(w.material(p[0], p[1], p[2]), AIR);
                        assert_eq!(w.material_cached(&mut cursor, p[0], p[1], p[2]), AIR);
                    }
                });
            }
        });
        assert_eq!(calls.load(Ordering::Relaxed), 0, "workers={workers}");
    }
    assert_eq!(w.material(1, 1, 1), 2);
    let mut cursor = World::mat_cursor();
    assert_eq!(w.material_cached(&mut cursor, 1, 1, 1), 2);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

fn check_columns(w: &World) {
    // Column evaluation and immutable snapshots do not use the occupancy
    // shortcut, so both independently check its answers and edited layers.
    let snapshot = snapshot::Snapshot::capture(w, [0; 3], [73, 67, 45]).unwrap();
    let mut cursor = World::mat_cursor();
    for z in [0, 1, 5, 31, 32, 33, 44] {
        for x in [0, 1, 3, 31, 32, 33, 64, 65, 72] {
            let mut column = [AIR; 67];
            w.column(x, z, 0, &mut column);
            for (y, &expected) in column.iter().enumerate() {
                let y = y as u32;
                assert_eq!(w.material(x, y, z), expected, "{x},{y},{z}");
                assert_eq!(w.material_cached(&mut cursor, x, y, z), expected);
                assert_eq!(snapshot.material([x as i32, y as i32, z as i32]), expected);
            }
        }
    }
}

#[test]
fn material_reads_match_columns_and_snapshots_through_edits_load_and_reset() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut w = world(&calls);
    check_columns(&w);
    let old = snapshot::Snapshot::capture(&w, [0; 3], [73, 67, 45]).unwrap();
    w.queue_edit(Edit::put(33, 63, 33, 3));
    w.queue_dig(3, 34, 5);
    assert_eq!(w.material(33, 63, 33), AIR, "queued matter is not applied");
    assert_eq!(w.material(3, 34, 5), 7);
    w.flush();
    assert_eq!(w.material(33, 63, 33), 3);
    assert_eq!(w.material(3, 34, 5), AIR);
    assert_eq!(old.material([33, 63, 33]), AIR);
    assert_eq!(old.material([3, 34, 5]), 7);
    check_columns(&w);
    w.queue_transfer(Edit::dig(33, 63, 33));
    w.queue_transfer(Edit::put(65, 63, 44, 9));
    w.flush();
    check_columns(&w);
    let mut saved = fx::blob::Blob::new();
    w.save(&mut saved);
    let mut loaded = world(&calls);
    assert!(loaded.load(&mut fx::blob::Scan::new(&saved.v)));
    check_columns(&loaded);
    loaded.restore_pristine();
    check_columns(&loaded);
    assert_eq!(loaded.material(65, 63, 44), AIR);
    assert_eq!(loaded.material(3, 34, 5), 7);
}
