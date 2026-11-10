use super::*;

fn arena() -> BlastRange {
    use modding::range::{RangeLayout,RangeUnitDef,RangeBlock};
    let mut l=RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size=[2048,192,640]; l.clear_min=[1,1]; l.clear_max=[2047,639];
    l.battle=true; l.idle_enemies=true; l.course=None; l.shields.clear();
    l.blocks=vec![RangeBlock{min:[0;3],max:[2048,192,640],material:0},
        RangeBlock{min:[0;3],max:[2048,16,640],material:1}];
    l.units=vec![RangeUnitDef{kind:RangeUnitKind::Tank,allied:true,at:[170,16,336],weapon:None},
        RangeUnitDef{kind:RangeUnitKind::Tank,allied:false,at:[1800,16,336],weapon:None}];
    let c=modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r=BlastRange::with_layout(c,l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r
}

#[test]
fn idle_aim_mobile_roster_slews_with_limits_and_follows_reverse_and_stop() {
    let limits=crate::turret::Limits::of(modding::turret::Turret::from_ron(
        include_str!("../../../data/turret.ron")).unwrap(),crate::TICKS_PER_SECOND);
    for kind in RangeUnitKind::ALL {
        let mut u=RangeUnit::of_kind(kind,[1000,16,1000]);
        let mut a=Actor::of(&u);
        a.turret=crate::turret::Traverse::new(fx::angle::QUARTER);
        if kind.emplacement() || !u.status.weapon {
            follow(&mut u,&mut a,limits);
            assert_eq!(a.turret.angle,fx::angle::QUARTER,"{kind:?}");
            continue;
        }
        u.motion.drive.vel=Fx::ONE;
        if kind.artillery() || kind==RangeUnitKind::Spider {
            a.elevation=crate::turret::Traverse::new(fx::angle::FULL/32);
            u.turret_pitch=fx::angle::FULL/32;
        }
        follow(&mut u,&mut a,limits);
        assert_ne!(a.turret.angle,0,"must slew, not snap: {kind:?}");
        for velocity in [Fx::ONE,Fx::ONE.neg(),Fx::ZERO] {
            u.motion.drive.vel=velocity;
            let goal=u.motion.travel_heading();
            for _ in 0..600 {
                let old=a.turret;
                follow(&mut u,&mut a,limits);
                assert!(fx::angle::diff(a.turret.angle,old.angle).abs()<=limits.speed,"{kind:?}");
                assert!((a.turret.rate-old.rate).abs()<=limits.accel,"{kind:?}");
            }
            assert!(fx::angle::diff(a.turret.angle,goal).abs()<=limits.tolerance,"{kind:?}: {} vs {goal}",a.turret.angle);
            assert_eq!(u.turret_delta,fx::angle::diff(a.turret.angle,u.motion.drive.head));
            assert!(u.turret_pitch.abs()<=limits.tolerance,"{kind:?} did not lower the barrel");
        }
        if kind==RangeUnitKind::Walker04 {
            for arm in 0..4 { assert!(u.seg_frames[13+arm*4].x.x>Fx::from_ratio(9,10),"arm {arm} stayed behind"); }
        }
    }
}

#[test]
fn idle_aim_free_control_and_battle_move_then_acquire_nearby_enemy_and_restore() {
    for fighting in [false,true] {
        let mut r=arena();
        if fighting {r.start_battle().unwrap();} else {r.start_free_control().unwrap();}
        assert!(r.set_battle_weapon(0,Some(Weapon::Blast)));
        r.battle.actors[1].order=Order::Hold;
        r.battle.actors[0].turret=crate::turret::Traverse::new(fx::angle::QUARTER);
        r.units[0].update_pose(fx::angle::QUARTER);
        assert!(r.battle_order(0,Order::Move,Some([330,336])));
        for _ in 0..10 {r.step();}
        let mut saved=BlastRange::restore(r.config,&r.snapshot()).expect("travel actuator restores");
        saved.set_physics_workers(2); saved.set_navigation_workers(2);
        for _ in 0..110 {r.step();saved.step();assert_eq!(r.state_hash(),saved.state_hash());}
        assert!(fx::angle::diff(r.battle.actors[0].turret.angle,r.units[0].motion.travel_heading()).abs()
            <=r.turret_limits.tolerance,"fighting={fighting}");
        if fighting {
            let at=r.units[0].at;
            r.units[1].motion.pos=[at[0],16,at[2]+160].map(Fx::from_int); r.units[1].update_pose(0);
            for _ in 0..10 {r.step();}
            assert_eq!(r.battle.actors[0].target,Some(1));
            assert!(fx::angle::diff(r.battle.actors[0].turret.angle,r.units[0].motion.drive.head)>0,
                "a nearby hostile must override travel alignment");
        }
    }
}

#[test]
fn idle_aim_manual_first_person_keeps_its_aim_without_firing() {
    let mut r=arena();
    r.start_free_control().unwrap();
    assert!(r.battle_order(0,Order::Pilot{throttle:0,turn:0,aim:[170,22,500],fire:false},None));
    for _ in 0..150 {r.step();}
    assert!(fx::angle::diff(r.battle.actors[0].turret.angle,fx::angle::QUARTER).abs()<r.turret_limits.tolerance);
    assert_eq!(r.battle.actors[0].fired,0);
}

#[test]
fn idle_aim_physical_range_gun_retains_travel_heading_after_arrival() {
    let original=arena();
    let mut l=original.layout.clone().unwrap(); l.battle=false; l.units.clear();
    let mut r=BlastRange::with_layout(original.config,l).unwrap(); drop(original);
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.control(); r.set_auto_attack(false);
    r.turret=crate::turret::Traverse::new(fx::angle::QUARTER); r.gun.update_pose(r.turret.angle);
    let from=r.gun.at;
    assert!(r.order_move([from[0]+160,from[2]]));
    for _ in 0..180 {r.step();}
    assert!(r.gun.motion.destination.is_none());
    assert!(fx::angle::diff(r.turret.angle,r.gun.motion.drive.head).abs()<=r.turret_limits.tolerance);
    assert_eq!(r.gun.turret_delta,fx::angle::diff(r.turret.angle,r.gun.motion.drive.head));
    let back=BlastRange::restore(r.config,&r.snapshot()).expect("range travel aim restores");
    assert_eq!(r.state_hash(),back.state_hash());
    let out=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/idle-aim-544");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("travel.bin"),r.snapshot()).unwrap();
}

#[test]
fn idle_aim_walker_bearings_return_and_restore_without_a_combat_target() {
    use modding::range::{RangeLayout,RangeUnitDef,RangeBlock};
    let mut l=RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    let mut at=[500,16,500];
    let (lo,_)=RangeUnit::of_kind(RangeUnitKind::Walker04,at).bounds();
    at[0]+=(32-lo[0]).max(0); at[2]+=(32-lo[2]).max(0);
    let (_,hi)=RangeUnit::of_kind(RangeUnitKind::Walker04,at).bounds();
    let enemy=[hi[0]+120,16,at[2]];
    l.size=[(enemy[0]+80).max(1536), (hi[1]+32).max(256), (hi[2]+32).max(1280)]
        .map(|v|((v as u32+15)/16)*16);
    l.clear_min=[1,1]; l.clear_max=[l.size[0] as i32-1,l.size[2] as i32-1];
    l.battle=true; l.idle_enemies=true; l.course=None; l.shields.clear();
    let size=l.size.map(|v|v as i32);
    l.blocks=vec![RangeBlock{min:[0;3],max:size,material:0},
        RangeBlock{min:[0;3],max:[size[0],16,size[2]],material:1}];
    l.units=vec![RangeUnitDef{kind:RangeUnitKind::Walker04,allied:true,at,weapon:None},
        RangeUnitDef{kind:RangeUnitKind::Tank,allied:false,at:enemy,weapon:None}];
    let c=modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r=BlastRange::with_layout(c,l).unwrap();
    r.set_physics_workers(1);r.set_navigation_workers(1);r.start_free_control().unwrap();
    for _ in 0..10 {r.step();}
    assert!(r.units[0].arm_targets.is_some());
    let mut saved=BlastRange::restore(r.config,&r.snapshot()).expect("idle arm bearings restore");
    saved.set_physics_workers(2);saved.set_navigation_workers(2);
    for _ in 0..25 {r.step();saved.step();assert_eq!(r.state_hash(),saved.state_hash());}
    assert!(r.units[0].status.alive);
}
