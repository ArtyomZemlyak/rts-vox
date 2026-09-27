use super::*;

struct Terrain;
impl Generator for Terrain {
    fn material(&self, x: u32, y: u32, z: u32) -> Mat {
        if y < 9 + (x + z) % 7 {
            2
        } else {
            AIR
        }
    }
}
#[test]
fn column_and_pristine_reset_match_material_bits_and_old_snapshots() {
    let mut world = World::new(73, 67, 45, 100, Box::new(Terrain));
    let pristine = World::new(73, 67, 45, 100, Box::new(Terrain));
    for p in [[1, 4, 3], [33, 8, 34], [65, 63, 43], [32, 31, 32]] {
        world.queue_edit(Edit::put(p[0], p[1], p[2], if p[1] < 9 { 0 } else { 3 }));
    }
    while world.queued() > 0 {
        world.flush();
    }
    let snapshot = snapshot::Snapshot::capture(&world, [0, 0, 0], [73, 67, 45]).unwrap();
    let old_hash = snapshot.hash();
    for (x, z) in [(1, 3), (33, 34), (65, 43), (32, 32), (73, 0)] {
        for start in [0, 7, 31, 32, 63, 67, u32::MAX] {
            let mut column = [999; 80];
            world.column(x, z, start, &mut column);
            for (dy, &m) in column.iter().enumerate() {
                let expected = start
                    .checked_add(dy as u32)
                    .map_or(0, |y| world.material(x, y, z));
                assert_eq!(m, expected);
            }
        }
    }
    world.queue_dig(7, 3, 7);
    let pointer = world.bits.as_ptr();
    world.restore_pristine();
    assert_eq!(world.bits.as_ptr(), pointer, "keep the dense allocation");
    assert_eq!(world.bits, pristine.bits);
    assert_eq!(world.ceiling, pristine.ceiling);
    assert_eq!(world.queued(), 0);
    assert_eq!(world.touched_chunks(), 0);
    assert_eq!(snapshot.hash(), old_hash);
    assert_eq!(snapshot.material([1, 4, 3]), 0);
    assert_eq!(world.material(1, 4, 3), 2);
}

#[test]
fn changing_generator_regions_updates_bounds_and_preserves_old_snapshot() {
    struct Changed;
    impl Generator for Changed {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if (31..67).contains(&x) && z >= 33 {
                if y < 63 {
                    3
                } else {
                    0
                }
            } else {
                Terrain.material(x, y, z)
            }
        }
    }
    let mut world = World::new(73, 67, 45, 100, Box::new(Terrain));
    let old = snapshot::Snapshot::capture(&world, [0, 0, 0], [73, 67, 45]).unwrap();
    world.queue_dig(1, 4, 3);
    world.flush();
    world.reset_generator_regions(
        Box::new(Changed),
        &[([31, 33], [67, 100]), ([-3, -2], [0, 0])],
    );
    let fresh = World::new(73, 67, 45, 100, Box::new(Changed));
    assert_eq!(world.bits, fresh.bits);
    assert_eq!(world.ceiling, fresh.ceiling);
    assert_eq!(old.material([40, 62, 35]), 0);
    assert_eq!(world.material(40, 62, 35), 3);
}

#[test]
fn sparse_reset_restores_several_heights_in_one_column() {
    let mut world = World::new(73, 67, 45, 100, Box::new(Terrain));
    let pristine = World::new(73, 67, 45, 100, Box::new(Terrain));
    for edit in [
        Edit::dig(33, 4, 34),
        Edit::put(33, 31, 34, 3),
        Edit::put(33, 63, 34, 3),
        Edit::dig(33, 63, 34),
        Edit::put(65, 63, 43, 3),
    ] {
        world.queue_edit(edit);
    }
    while world.queued() > 0 {
        world.flush();
    }
    assert!(world.reset_dirty.as_ref().is_some_and(|p| p.len() >= 4));
    world.restore_pristine();
    assert_eq!(world.bits, pristine.bits);
    assert_eq!(world.ceiling, pristine.ceiling);
    assert_eq!(world.touched_chunks(), 0);
}

#[test]
fn loaded_or_untracked_chunks_take_the_full_reset_path() {
    let mut world = World::new(73, 67, 45, 100, Box::new(Terrain));
    let pristine = World::new(73, 67, 45, 100, Box::new(Terrain));
    world.queue_dig(33, 4, 34);
    world.queue_edit(Edit::put(65, 63, 43, 3));
    world.flush();
    let mut blob = fx::blob::Blob::new();
    world.save(&mut blob);
    let mut loaded = World::new(73, 67, 45, 100, Box::new(Terrain));
    assert!(loaded.load(&mut fx::blob::Scan::new(&blob.v)));
    assert!(loaded.reset_dirty.is_none());
    loaded.restore_pristine();
    assert_eq!(loaded.bits, pristine.bits);
    assert_eq!(loaded.ceiling, pristine.ceiling);
    assert_eq!(loaded.touched_chunks(), 0);

    // Exhaustion of the optional hint takes the same conservative path.
    world.reset_dirty = None;
    world.restore_pristine();
    assert_eq!(world.bits, pristine.bits);
    assert_eq!(world.ceiling, pristine.ceiling);
}

/// **Сброс не трогает настроек мира** (ADR-464): потолок
/// материализации — данные, а не состояние, и восстановление карты
/// прежде ставило вместо него четыре из конструктора, по обоим путям
/// сброса.
#[test]
fn a_reset_keeps_the_configured_materialization_cap() {
    let mut world = World::new(73, 67, 45, 100, Box::new(Terrain));
    world.max_materialize_per_tick = 9;
    for restore in [false, true] {
        world.queue_edit(Edit::put(33, 30, 34, 3));
        while world.queued() > 0 {
            world.flush();
        }
        if restore {
            world.reset_generator_regions(Box::new(Terrain), &[([0, 0], [73, 45])]);
        } else {
            world.restore_pristine();
        }
        assert_eq!(world.max_materialize_per_tick, 9);
    }
}
