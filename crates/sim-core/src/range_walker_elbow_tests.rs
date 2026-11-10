use super::*;

fn settle(u: &mut RangeUnit, target: [i32; 3]) {
    for _ in 0..160 { u.set_arm_targets([target; 4], 0); }
}
fn bend(u: &RangeUnit, arm: usize) -> i32 {
    let upper = u.seg_frames[10 + arm * 4];
    let gun = u.seg_frames[13 + arm * 4];
    fx::angle::diff(fx::angle::atan2(upper.x.dot(gun.y).raw(),
        upper.x.dot(gun.x).raw()), 0).abs()
}
fn joints(u: &RangeUnit) {
    for arm in 0..4 {
        let first = 10 + arm * 4;
        for child in first + 1..first + 4 {
            let a = u.body.segments[child].attach;
            let expected = u.seg_frames[usize::from(a.parent) - 1].world([
                Fx::from_int(i32::from(a.at.0)), Fx::from_int(i32::from(a.at.1)),
                Fx::from_int(i32::from(a.at.2))]);
            assert!(expected.sub(u.seg_frames[child].base).len_sq() < Fx::from_ratio(1, 1000),
                "arm {arm}: separated joint {child}");
        }
    }
}

#[test]
fn walker_elbows_really_flex_and_keep_all_links_and_bores_connected() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Walker04, [600, 16, 600]);
    for arm in 0..4 { assert!(bend(&u, arm) > fx::angle::FULL * 30 / 360, "rigid rest arm"); }
    joints(&u);
    settle(&mut u, [1400, 560, 600]);
    let high: [i32; 4] = std::array::from_fn(|arm| bend(&u, arm));
    joints(&u);
    settle(&mut u, [1400, 40, 600]);
    joints(&u);
    for arm in 0..4 {
        let low = bend(&u, arm);
        assert!(low > high[arm] + fx::angle::FULL * 8 / 360, "arm {arm}: fixed elbow angle");
        let frame = u.seg_frames[13 + arm * 4];
        let p = frame.world([Fx::from_int(4), frame.centre[1], frame.centre[2]]);
        let delta = V3::new(Fx::from_int(1400), Fx::from_int(40), Fx::from_int(600)).sub(p);
        assert!(frame.x.dot(delta).div(delta.len_sq().sqrt()) > Fx::from_ratio(998, 1000),
            "arm {arm}: bent arm no longer points its bore at the target");
    }
}

#[test]
fn flexed_walker_pose_is_derived_exactly_from_its_saved_targets() {
    let mut original = RangeUnit::of_kind(RangeUnitKind::Walker04, [600, 16, 600]);
    settle(&mut original, [1200, 250, 1100]);
    let mut restored = RangeUnit::of_kind(RangeUnitKind::Walker04, [600, 16, 600]);
    restored.arm_targets = original.arm_targets;
    restored.update_pose(0);
    joints(&restored);
    for index in 10..30 {
        let a = original.seg_frames[index]; let b = restored.seg_frames[index];
        assert_eq!((a.base, a.x, a.y, a.z), (b.base, b.x, b.y, b.z));
    }
}

#[test]
fn walker_bent_pose_native_fixture_roundtrips_through_the_real_snapshot() {
    use modding::range::{RangeLayout, RangeBlock, RangeUnitDef};
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size = [1600, 512, 1408]; l.battle = true; l.idle_enemies = true;
    l.course = None; l.ready = None; l.shields.clear();
    l.clear_min = [1, 1]; l.clear_max = [1599, 1407];
    l.blocks = vec![RangeBlock { min: [0; 3], max: l.size.map(|v| v as i32), material: 0 },
        RangeBlock { min: [0; 3], max: [1600, 16, 1408], material: 1 }];
    l.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Walker04, allied: true, at: [420, 16, 650], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Walker04, allied: true, at: [1050, 16, 650], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Tank, allied: false, at: [1400, 16, 1100], weapon: None }];
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut sim = BlastRange::with_saved_layout(cfg, l).unwrap();
    sim.set_physics_workers(1); sim.set_navigation_workers(1);
    assert!(sim.start_battle().is_ok());
    settle(&mut sim.units[0], [1400, 40, 650]);
    settle(&mut sim.units[1], [1800, 560, 650]);
    let saved = sim.snapshot();
    let copy = BlastRange::restore(cfg, &saved).unwrap();
    assert_eq!(sim.state_hash(), copy.state_hash());
    for id in 0..2 { joints(&copy.units[id]); }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/walker-elbows-541");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("preview.bin"), saved).unwrap();
}
