use super::*;

fn settled(u: &mut RangeUnit, target: [i32; 3], arm: usize) {
    for _ in 0..200 {
        u.set_arm_targets([target, target, [arm as i32, 0, 0], [0; 3]], u.motion.drive.head);
    }
}
fn joints(u: &RangeUnit) {
    let crown = u.aimed_frame(&u.pose, u.turret_delta, u.turret_pitch);
    let rest = crate::damage::segment_frames(&crown, &u.body, 0, 0);
    for arm in 0..2 {
        let first = 10 + arm * 4;
        assert!(rest[first].base.sub(u.seg_frames[first].base).len_sq() < Fx::from_ratio(1, 1000), "shoulder detached");
        for child in first + 1..first + 4 {
            let a = u.body.segments[child].attach;
            let expected = u.seg_frames[usize::from(a.parent) - 1].world([
                Fx::from_int(i32::from(a.at.0)), Fx::from_int(i32::from(a.at.1)), Fx::from_int(i32::from(a.at.2))]);
            assert!(expected.sub(u.seg_frames[child].base).len_sq() < Fx::from_ratio(1, 1000), "separated joint {child}");
        }
    }
}
fn gun_pitch(u: &RangeUnit, arm: usize) -> i32 {
    let x = u.seg_frames[13 + arm * 4].x;
    fx::angle::diff(fx::angle::atan2(x.y.raw(), x.x.mul(x.x).add(x.z.mul(x.z)).sqrt().raw()), 0)
}
fn elbow(u: &RangeUnit, arm: usize) -> i32 {
    let upper = u.seg_frames[10 + arm * 4];
    let gun = u.seg_frames[13 + arm * 4];
    fx::angle::diff(fx::angle::atan2(upper.x.dot(gun.y).raw(), upper.x.dot(gun.x).raw()), 0).abs()
}

#[test]
fn commander_articulated_tools_flex_keep_joints_and_aim_from_the_real_bore() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Commander, [600, 16, 600]);
    joints(&u);
    for arm in 0..2 { assert!(gun_pitch(&u, arm) < degrees(-35), "idle arm should hang down"); }
    for arm in 0..2 {
        settled(&mut u, [1200, 200, 620], arm);
        joints(&u);
        let low = elbow(&u, arm);
        let f = u.seg_frames[13 + arm * 4];
        let tip = &u.body.segments[13 + arm * 4].body;
        let muzzle = f.world([Fx::from_int(i32::from(tip.vol.nx)), Fx::from_ratio(i32::from(tip.vol.ny), 2), Fx::from_ratio(i32::from(tip.vol.nz), 2)]);
        let d = point([1200, 200, 620]).sub(muzzle);
        assert!(f.x.dot(d).div(d.len_sq().sqrt()) > Fx::from_ratio(998, 1000), "tool must aim from its actual muzzle");
        assert!(gun_pitch(&u, 1 - arm) < degrees(-35), "unused tool should remain lowered");
        settled(&mut u, [1200, 600, 620], arm);
        assert!(elbow(&u, arm) > low + degrees(8), "elbow should change with elevation");
        joints(&u);
    }
}

#[test]
fn commander_tools_have_bounded_slew_and_restore_without_advancing_the_pose() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Commander, [600, 16, 600]);
    let target = [1000, 250, 720];
    for _ in 0..25 { u.set_arm_targets([target, target, [0;3], [0;3]], 0); }
    let mut restored = RangeUnit::of_kind(RangeUnitKind::Commander, [600, 16, 600]);
    restored.arm_targets = u.arm_targets;
    restored.update_pose(0);
    assert_eq!(u.seg_frames, restored.seg_frames);
    for _ in 0..120 {
        let old = u.seg_frames[17].x;
        let target = [800, 20, 540];
        u.set_arm_targets([target, target, [1,0,0], [0;3]], 0);
        restored.set_arm_targets([target, target, [1,0,0], [0;3]], 0);
        assert_eq!(u.seg_frames, restored.seg_frames);
        assert!(old.dot(u.seg_frames[17].x) > Fx::from_ratio(99,100), "tool switch snapped");
        joints(&u);
        let prior = u.seg_frames.clone();
        u.update_pose(0);
        assert_eq!(u.seg_frames, prior, "pose queries must not advance the servo");
    }
}

#[test]
fn commander_free_arms_follow_the_walk_cycle_without_moving_the_aimed_tool() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Commander, [600,16,600]);
    u.motion.drive.vel = Fx::from_int(u.motion.speed);
    u.motion.phase = fx::angle::QUARTER;
    u.update_pose(0);
    let first = [gun_pitch(&u,0),gun_pitch(&u,1)];
    u.motion.phase = fx::angle::QUARTER * 3;
    u.update_pose(0);
    assert!(gun_pitch(&u,0) < first[0] - degrees(5));
    assert!(gun_pitch(&u,1) > first[1] + degrees(5));
    settled(&mut u,[1200,200,620],0);
    let gun = u.seg_frames[13];
    u.motion.phase = fx::angle::QUARTER;
    u.update_pose(0);
    assert_eq!(gun,u.seg_frames[13],"walking must not shake the aimed weapon");
}

#[test]
fn commander_forearms_and_tools_clear_the_torso_in_the_working_sector() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Commander,[600,16,600]);
    for target in [[1200,30,300],[1200,30,900],[1200,600,300],[1200,600,900],
        [650,20,600],[600,20,200],[600,20,1000],[200,30,200],[200,30,1000],[200,600,600]] {
        for arm in 0..2 {
            settled(&mut u,target,arm);
            for index in [12,13,16,17] {
                let body = &u.body.segments[index].body;
                let f = u.seg_frames[index];
                let mut intersections = 0;
                for z in 0..usize::from(body.vol.nz) { for y in 0..usize::from(body.vol.ny) { for x in 0..usize::from(body.vol.nx) {
                    if !body.vol.get(x,y,z) {continue;}
                    let p = f.world([x,y,z].map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1,2))));
                    let p = coords(p);
                    if let Some([x,y,z]) = u.root_local(p,&u.pose,u.turret_delta,u.turret_pitch) {
                        if u.body.vol.get(x,y,z) {intersections+=1;}
                    }
                }}}
                assert_eq!(intersections,0,"tool {index} intersects torso toward {target:?}");
            }
        }
    }
}

#[test]
fn commander_arms_stay_in_safe_sector_and_relax_after_movement() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Commander, [600,16,600]);
    settled(&mut u, [100,0,100], 1);
    for arm in 0..2 {
        let f = u.seg_frames[13 + arm*4];
        let yaw = fx::angle::atan2(f.x.z.raw(), f.x.x.raw());
        assert!(fx::angle::diff(yaw,u.motion.drive.head).abs() <= degrees(36));
        assert!(gun_pitch(&u,arm) >= degrees(-76));
    }
    u.motion.pos[0] = Fx::from_int(650);
    u.motion.drive.head = fx::angle::QUARTER;
    u.update_pose(fx::angle::QUARTER);
    for _ in 0..200 { u.relax_commander_arms(fx::angle::QUARTER); }
    for arm in 0..2 {
        assert!(gun_pitch(&u,arm) < degrees(-35));
        let f = u.seg_frames[13+arm*4];
        assert!(f.x.dot(u.pose.x) > Fx::from_ratio(1,3));
    }
    joints(&u);
}

#[test]
fn commander_articulated_scene_roundtrips_and_provides_native_pose_fixtures() {
    use modding::range::{RangeLayout, RangeBlock, RangeUnitDef};
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    layout.size = [1024, 192, 768]; layout.clear_min = [1,1]; layout.clear_max = [1023,767];
    layout.battle = true; layout.idle_enemies = true; layout.ready = None; layout.course = None;
    layout.shields.clear();
    layout.blocks = vec![RangeBlock {min:[0;3], max:[1024,192,768], material:0},
        RangeBlock {min:[0;3], max:[1024,16,768], material:1}];
    layout.units = vec![RangeUnitDef {kind:RangeUnitKind::Commander, allied:true, at:[360,16,360], weapon:None},
        RangeUnitDef {kind:RangeUnitKind::Tank, allied:true, at:[500,16,480], weapon:None},
        RangeUnitDef {kind:RangeUnitKind::Tank, allied:false, at:[900,16,600], weapon:None}];
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut sim = BlastRange::with_layout(cfg, layout).unwrap();
    sim.set_physics_workers(1); sim.set_navigation_workers(1);
    sim.start_battle().unwrap();
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/commander-arms-545");
    std::fs::create_dir_all(&out).unwrap();
    for (name, arm) in [("cannon",0), ("fabricator",1), ("idle",2)] {
        if arm < 2 { settled(&mut sim.units[0], [850,90,380], arm); }
        else { for _ in 0..200 { sim.units[0].relax_commander_arms(0); } }
        assert_eq!(sim.battle_muzzles(0).len(),1,"articulation must retain a connected cannon");
        let physical = super::super::weapons::walker_muzzles(&sim.units[0]);
        assert_eq!(sim.battle_muzzles(0)[0],physical[0].1);
        assert!(sim.commander_tool_tip(0).is_some(),"fabrication emitter must remain connected");
        let snapshot = sim.snapshot();
        let copy = BlastRange::restore(cfg, &snapshot).unwrap();
        assert_eq!(sim.state_hash(),copy.state_hash());
        assert_eq!(sim.units[0].seg_frames,copy.units[0].seg_frames);
        joints(&copy.units[0]);
        std::fs::write(out.join(format!("{name}.bin")),snapshot).unwrap();
    }
}
