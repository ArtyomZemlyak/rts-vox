use super::*;
use std::collections::BTreeMap;

#[test]
fn pending_dig_counts_match_an_ordered_reference() {
    let mut digs = PendingDigs::new(8);
    let mut reference = BTreeMap::<(usize, usize), u32>::new();
    let mut rng = 0x125a_70d1u32;
    for i in 0..40_000 {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let key = (
            (rng as usize >> 7) % 8,
            (rng as usize >> 10) % (CS * CS * CS),
        );
        if i % 3 == 0 && reference.contains_key(&key) {
            digs.remove(key);
            let n = reference.get_mut(&key).unwrap();
            *n -= 1;
            if *n == 0 {
                reference.remove(&key);
            }
        } else {
            digs.add(key);
            *reference.entry(key).or_default() += 1;
        }
        assert_eq!(digs.contains(key), reference.contains_key(&key));
        if i % 1000 == 0 {
            let copy = digs.clone();
            for &key in reference.keys() {
                assert!(copy.contains(key));
            }
        }
    }
    for (key, count) in reference {
        for _ in 0..count {
            assert!(digs.contains(key));
            digs.remove(key);
        }
        assert!(!digs.contains(key));
    }
    for voxel in [0, 63, 64, CS * CS * CS - 1] {
        for _ in 0..1000 {
            digs.add((0, voxel));
        }
    }
    digs.clear();
    assert!(digs.totals.iter().all(|&n| n == 0));
    assert!(digs.chunks.iter().all(Option::is_none));
}

#[test]
fn conditional_replacement_cannot_restore_a_removed_or_changed_voxel() {
    struct Sand;
    impl Generator for Sand {
        fn material(&self, _: u32, _: u32, _: u32) -> Mat {
            10
        }
    }
    let mut world = World::new(32, 32, 32, 128, Box::new(Sand));
    world.queue_transfer(Edit::dig(4, 4, 4));
    world.queue_transfer(Edit::replace_if(4, 4, 4, 10, 12));
    world.queue_transfer(Edit::put(5, 4, 4, 3));
    world.queue_transfer(Edit::replace_if(5, 4, 4, 10, 12));
    world.queue_transfer(Edit::replace_if(6, 4, 4, 10, 12));
    world.flush_transfers();
    assert_eq!(world.material(4, 4, 4), AIR);
    assert_eq!(world.material(5, 4, 4), 3);
    assert_eq!(world.material(6, 4, 4), 12);
}
