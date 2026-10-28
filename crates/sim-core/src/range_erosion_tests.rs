use super::*;
use crate::blast::granular::Flow;
use voxel::{Edit, Generator};
struct Flat;
impl Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        if y < 4 {
            6
        } else {
            0
        }
    }
}
fn put(w: &mut World, p: Pos, mat: u16) {
    w.queue_transfer(Edit::put(p[0], p[1], p[2], mat));
    w.flush_transfers();
}
fn fixture(mat: u16, saturation: u32, units: u32) -> (World, Water, Flow) {
    let mut w = World::new(64, 64, 64, 4096, Box::new(Flat));
    let p = [24, 4, 24];
    put(&mut w, p, mat);
    let mut water = Water::new();
    water.tuning.drying_units = 0;
    if let Some(m) = water.tuning.porous.iter().find(|m| m.material == mat) {
        let n = m.capacity * saturation / 1000;
        if n > 0 {
            water.wet.insert(p, (mat, n));
            water.absorbed += u64::from(n);
            water.initial += u64::from(n);
        }
    }
    water.set([25, 5, 24], 1024);
    water.initial += 1024;
    water.flux.push(Flux {
        from: [23, 5, 24],
        to: [24, 5, 24],
        units,
    });
    (w, water, Flow::new())
}
fn balanced(f: &Water) {
    assert_eq!(
        f.cells().map(|(_, n)| u64::from(n)).sum::<u64>()
            + f.wet.values().map(|v| u64::from(v.1)).sum::<u64>()
            + f.vapor,
        f.initial
    );
    assert_eq!(f.stats().units, f.initial);
}
#[test]
fn erosion_requires_current_saturation_and_material_yield() {
    for (mat, saturation, units, expected) in [
        (1, 1000, 1024, 1),
        (1, 0, 1024, 0),
        (1, 1000, 256, 0),
        (8, 1000, 1024, 0),
        (6, 0, 4096, 0),
        (5, 1000, 4096, 0),
        (10, 1000, 512, 1),
    ] {
        let (mut w, mut water, mut flow) = fixture(mat, saturation, units);
        assert_eq!(
            flow.erode(&mut w, &mut water, 0, |_| false),
            expected,
            "mat={mat} wet={saturation} flow={units}"
        );
        balanced(&water);
    }
    let (mut w, mut water, mut flow) = fixture(1, 1000, 4096);
    water.flux.clear();
    assert_eq!(
        flow.erode(&mut w, &mut water, 0, |_| false),
        0,
        "standing water eroded the bed"
    );
}
#[test]
fn erosion_moves_real_cells_displaces_water_and_preserves_save_continuation() {
    let (mut w, mut water, mut flow) = fixture(1, 1000, 1024);
    assert_eq!(flow.erode(&mut w, &mut water, 0, |_| false), 1);
    assert_eq!(w.material(24, 4, 24), 0);
    assert_eq!(w.material(25, 5, 24), 1);
    assert_eq!(water.amount([25, 5, 24]), 0);
    assert_eq!(water.amount([24, 4, 24]), 1024);
    balanced(&water);
    let mut b = Blob::new();
    water.save(&mut b);
    let mut copy = Water::new();
    copy.load(&mut Scan::new(&b.v), [64; 3]).unwrap();
    copy.tuning = water.tuning.clone();
    let mut world_blob = Blob::new();
    w.save(&mut world_blob);
    let mut ww = World::new(64, 64, 64, 4096, Box::new(Flat));
    assert!(ww.load(&mut Scan::new(&world_blob.v)));
    let mut flow_blob = Blob::new();
    flow.save(&mut flow_blob);
    let mut ff = Flow::new();
    ff.load(&mut Scan::new(&flow_blob.v), [64; 3]).unwrap();
    for tick in 1..100 {
        water.step(&w, &[]);
        copy.step(&ww, &[]);
        flow.erode(&mut w, &mut water, tick, |_| false);
        ff.erode(&mut ww, &mut copy, tick, |_| false);
        flow.step_moist(&mut w, |_| false, |p| water.repose_bonus(p));
        ff.step_moist(&mut ww, |_| false, |p| copy.repose_bonus(p));
        let mut a = Blob::new();
        let mut b = Blob::new();
        water.save(&mut a);
        copy.save(&mut b);
        flow.save(&mut a);
        ff.save(&mut b);
        w.save(&mut a);
        ww.save(&mut b);
        assert_eq!(a.v, b.v);
        balanced(&water);
    }
    assert_eq!(
        w.material(25, 4, 24),
        1,
        "transported soil did not settle downstream"
    );
}
#[test]
fn erosion_respects_claims_colliders_pending_digs_and_disabled_mode() {
    for blocked in [[24, 4, 24], [25, 5, 24]] {
        let (mut w, mut water, mut flow) = fixture(1, 1000, 1024);
        assert_eq!(flow.erode(&mut w, &mut water, 0, |p| p == blocked), 0);
        balanced(&water);
    }
    let (mut w, mut water, mut flow) = fixture(1, 1000, 1024);
    water.wet.insert([25, 5, 24], (1, 64));
    water.absorbed += 64;
    water.initial += 64;
    assert_eq!(
        flow.erode(&mut w, &mut water, 0, |_| false),
        0,
        "new grain inherited unreleased pore water"
    );
    balanced(&water);
    let (mut w, mut water, mut flow) = fixture(1, 1000, 1024);
    flow.tuning.erosion.moves_per_tick = 0;
    assert_eq!(flow.erode(&mut w, &mut water, 0, |_| false), 0);
    flow.tuning.erosion.moves_per_tick = 4;
    w.queue_edit(Edit::dig(24, 4, 24));
    assert_eq!(flow.erode(&mut w, &mut water, 0, |_| false), 0);
    balanced(&water);
}
#[test]
fn erosion_move_and_sample_limits_do_not_move_a_grain_twice() {
    let (mut w, mut water, mut flow) = fixture(1, 1000, 1024);
    water.flux.clear();
    for x in [8, 16, 24, 32, 40, 48] {
        let p = [x, 4, 24];
        if x != 24 {
            put(&mut w, p, 1);
            water.wet.insert(p, (1, 1024));
            water.absorbed += 1024;
            water.initial += 1024;
        }
        for _ in 0..3 {
            water.flux.push(Flux {
                from: [x - 1, 5, 24],
                to: [x, 5, 24],
                units: 1024,
            });
        }
    }
    flow.tuning.erosion.moves_per_tick = 2;
    flow.tuning.erosion.checks_per_tick = 4;
    assert_eq!(flow.erode(&mut w, &mut water, 0, |_| false), 2);
    assert_eq!(flow.moved, 2);
    balanced(&water);
    let remaining = (1..63).filter(|&x| w.material(x, 4, 24) == 1).count();
    let carried = (1..63).filter(|&x| w.material(x, 5, 24) == 1).count();
    assert_eq!((remaining, carried), (4, 2));
}

#[test]
fn erosion_flux_ignores_nonlocal_pressure_and_records_only_real_local_moves() {
    let mut water = Water::new();
    water.set([25, 5, 24], 4096);
    water.initial = 4096;
    assert_eq!(
        water.transfer([25, 5, 24], [30, 5, 24], 1024, [64; 3]),
        1024
    );
    assert_eq!(
        water.transfer([25, 5, 24], [25, 6, 24], 1024, [64; 3]),
        1024
    );
    assert!(water.flux.is_empty());
    assert_eq!(
        water.transfer([25, 5, 24], [26, 5, 24], 1024, [64; 3]),
        1024
    );
    assert_eq!(water.flux.len(), 1);
    balanced(&water);
}
