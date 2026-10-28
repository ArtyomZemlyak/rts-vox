use super::*;
use voxel::{Edit, Generator};
struct Air;
impl Generator for Air {
    fn material(&self, _: u32, _: u32, _: u32) -> u16 {
        0
    }
}
fn world() -> World {
    World::new(64, 64, 64, 4096, Box::new(Air))
}
fn put(w: &mut World, p: Pos, mat: u16) {
    w.queue_transfer(Edit::put(p[0], p[1], p[2], mat));
    w.flush_transfers();
}
fn wet(p: Pos, mat: u16, amount: u32) -> Water {
    let mut f = Water::new();
    f.tuning.drying_units = 0;
    f.wet.insert(p, (mat, amount));
    f.absorbed = u64::from(amount);
    f.initial = u64::from(amount);
    f
}
fn balanced(f: &Water) {
    let absorbed: u64 = f.wet.values().map(|v| u64::from(v.1)).sum();
    assert_eq!(absorbed, f.stats().absorbed_units);
    assert!(f.wet.values().all(|v| v.1 > 0));
    assert_eq!(
        f.cells().map(|(_, n)| u64::from(n)).sum::<u64>() + absorbed + f.stats().vapor_units,
        f.stats().initial_units
    );
}
#[test]
fn capillary_moisture_reaches_inside_and_above_but_not_through_stone_or_air() {
    let mut w = world();
    for x in 10..18 {
        put(&mut w, [x, 10, 10], 1);
    }
    put(&mut w, [14, 10, 10], 6);
    for y in 11..16 {
        put(&mut w, [10, y, 10], 5);
    }
    put(&mut w, [10, 10, 12], 1);
    let mut f = wet([10, 10, 10], 1, 1024);
    for _ in 0..180 {
        f.step(&w, &[]);
        balanced(&f);
    }
    assert!(
        f.moisture([13, 10, 10]) > 0,
        "interior soil never became wet"
    );
    assert!(f.moisture([10, 13, 10]) > 0, "no capillary rise into wood");
    assert_eq!(
        f.moisture([15, 10, 10]),
        0,
        "water crossed impermeable stone"
    );
    assert_eq!(f.moisture([10, 10, 12]), 0, "water jumped across air");
}
#[test]
fn capillary_materials_equalise_saturation_and_competing_flows_never_overfill() {
    let mut w = world();
    put(&mut w, [20, 10, 20], 1);
    put(&mut w, [21, 10, 20], 5);
    let mut f = wet([20, 10, 20], 1, 768);
    for _ in 0..64 {
        f.dry(&w);
        balanced(&f);
    }
    assert_eq!(f.moisture([20, 10, 20]), 512);
    assert_eq!(f.moisture([21, 10, 20]), 256);
    // Several saturated neighbours all read the same destination snapshot.
    let p = [30, 20, 30];
    put(&mut w, p, 5);
    let mut f = Water::new();
    f.tuning.drying_units = 0;
    f.tuning.capillary_units = 4096;
    for porous in &mut f.tuning.porous {
        porous.rate = porous.capacity;
    }
    for d in DIRS {
        let q = neighbour(p, d, [64; 3]).unwrap();
        put(&mut w, q, 1);
        f.wet.insert(q, (1, 1024));
        f.absorbed += 1024;
        f.initial += 1024;
    }
    for _ in 0..20 {
        f.step(&w, &[]);
        balanced(&f);
        assert!(f.moisture(p) <= 512);
        assert!(f
            .wet
            .values()
            .all(|&(mat, n)| n <= if mat == 5 { 512 } else { 1024 }));
    }
}
#[test]
fn capillary_front_is_bounded_fair_and_restores_exactly() {
    let mut w = world();
    for x in 10..26 {
        put(&mut w, [x, 10, 20], 1);
    }
    let mut f = wet([10, 10, 20], 1, 1024);
    f.tuning.moisture_checks = 1;
    assert_eq!(f.wick(&w, &[[10, 10, 20]]), 6);
    assert!(f.moisture([11, 10, 20]) > 0);
    assert_eq!(
        f.moisture([12, 10, 20]),
        0,
        "a newly wet source propagated in the same phase"
    );
    for _ in 0..35 {
        f.step(&w, &[]);
    }
    let mut b = Blob::new();
    f.save(&mut b);
    let mut copy = Water::new();
    copy.load(&mut Scan::new(&b.v), [64; 3]).unwrap();
    copy.tuning = f.tuning.clone();
    for _ in 0..180 {
        f.step(&w, &[]);
        copy.step(&w, &[]);
        balanced(&f);
        let mut a = Blob::new();
        let mut b = Blob::new();
        f.save(&mut a);
        copy.save(&mut b);
        assert_eq!(a.v, b.v);
    }
    assert!(
        f.moisture([13, 10, 20]) > 0,
        "round robin did not reach the advancing front"
    );
}
#[test]
fn capillary_removal_drying_and_disabled_transfer_preserve_volume() {
    let mut w = world();
    let p = [20, 10, 20];
    let q = [21, 10, 20];
    put(&mut w, p, 1);
    put(&mut w, q, 1);
    let mut f = wet(p, 1, 1024);
    f.tuning.capillary_units = 0;
    for _ in 0..8 {
        f.step(&w, &[]);
    }
    assert_eq!(f.moisture(q), 0);
    balanced(&f);
    f.tuning.capillary_units = 32;
    for _ in 0..8 {
        f.step(&w, &[]);
    }
    assert!(f.moisture(q) > 0);
    put(&mut w, q, 0);
    f.tuning.drying_units = 1;
    for _ in 0..40 {
        f.step(&w, &[]);
        balanced(&f);
    }
    assert_eq!(f.moisture(q), 0);
    assert!(f.stats().vapor_units > 0);
    assert!(
        f.cells().any(|(_, n)| n > 0),
        "removed wet soil did not release pore water"
    );
}
