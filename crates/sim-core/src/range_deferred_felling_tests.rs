//! The battle motion phase must retain contact work after a hull leaves.
use super::battle::Order;
use super::*;
use modding::range::{RangeBlock, RangeUnitDef};

#[test]
fn battle_hull_cuts_finish_after_departure_and_snapshot_restore() {
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size = [768, 128, 768];
    l.clear_min = [40, 40];
    l.clear_max = [728, 728];
    l.course = None;
    l.shields.clear();
    l.blocks = vec![RangeBlock {
        min: [120, 16, 124],
        max: [136, 32, 132],
        material: 4,
    }];
    l.units = (0..6)
        .map(|i| RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: if i == 0 {
                [80, 16, 128]
            } else {
                [300 + 60 * i, 16, 500]
            },
            allied: i < 5,
            weapon: None,
        })
        .collect();
    let mut a = BlastRange::with_layout(config, l).unwrap();
    assert!(a.set_navigation_workers(4));
    a.start_battle().unwrap();
    a.matter.tuning.max_flying = 0; // Deterministic fully occupied pool fixture.
    assert!(a.battle_order(0, Order::Move, Some([250, 128])));
    for _ in 0..180 {
        a.step();
        if !a.units[0].motion.cut.is_empty() {
            break;
        }
    }
    let touched = a.units[0].motion.cut.clone();
    assert!(
        !touched.is_empty(),
        "battle motion never touched the vegetation"
    );
    assert!(
        a.matter.felling_pending() >= touched.len(),
        "battle dropped rejected contact cells"
    );
    assert!(touched
        .iter()
        .all(|p| a.world.material(p[0], p[1], p[2]) == 4));
    // Isolate departure: no subsequent overlap can resubmit these contacts.
    a.units[0].motion.stop();
    a.units[0].motion.enabled = false;
    a.units[0].motion.pos[0] = Fx::from_int(250);
    a.units[0].update_pose(0);
    let bytes = a.snapshot();
    let mut b = BlastRange::restore(config, &bytes).unwrap();
    assert_eq!(b.snapshot(), bytes);
    assert!(b.set_navigation_workers(1));
    for r in [&mut a, &mut b] {
        r.matter.tuning.max_flying = 4096;
    }
    for _ in 0..8 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        assert!(a.matter.economy.ledger.balanced());
    }
    assert!(
        touched
            .iter()
            .all(|p| a.world.material(p[0], p[1], p[2]) == 0),
        "departed tank left contacted plants standing"
    );
    assert!(a.removed_materials[4] >= touched.len());
}

#[test]
fn autonomous_battle_reopens_a_full_debris_pool_and_finishes_felling() {
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size = [768,128,768]; l.clear_min = [40,40]; l.clear_max = [728,728];
    l.course = None; l.shields.clear();
    l.blocks = vec![RangeBlock { min:[120,16,124], max:[136,32,132], material:4 }];
    l.units = vec![
        RangeUnitDef { kind:RangeUnitKind::Tank, at:[80,16,128], allied:true, weapon:None },
        RangeUnitDef { kind:RangeUnitKind::Tank, at:[500,16,500], allied:false, weapon:None },
    ];
    let mut a=BlastRange::with_layout(config,l).unwrap();
    a.set_physics_workers(1); a.set_navigation_workers(1);
    assert!(a.set_ai_vs_ai(true)); a.start_battle().unwrap();
    a.matter.tuning.max_flying=0;
    a.matter.tuning.max_fragments=1;
    a.matter.tuning.max_resting_fragments=0;
    let cell=[122,20,126];
    assert_eq!(a.matter.fell(&mut a.world,&[cell],[122,20,126],&a.navigation.fell),0);
    assert!(a.matter.felling_pending()>0);
    // The vehicle no longer touches the cell; only saved pending work can cut it.
    for _ in 0..25 {
        a.step();
        if a.world.material(cell[0],cell[1],cell[2])==0 { break; }
    }
    assert_eq!(a.world.material(cell[0],cell[1],cell[2]),0,"deferred vegetation did not resume");
    assert!(a.matter.tuning.max_flying>0 && a.matter.tuning.max_fragments>1);
    assert!(a.matter.economy.ledger.balanced());
    let mut b=BlastRange::restore(config,&a.snapshot()).expect("expanded pool must restore");
    assert_eq!(a.matter.tuning.max_flying,b.matter.tuning.max_flying);
    for _ in 0..3 { a.step(); b.step(); assert_eq!(a.state_hash(),b.state_hash()); }
}

#[test]
fn autonomous_battle_keeps_settled_fragments_and_clears_queued_felling() {
    let config=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut l=RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size=[768,128,768]; l.clear_min=[40,40]; l.clear_max=[728,728];
    l.course=None; l.shields.clear();
    l.blocks=vec![RangeBlock{min:[120,16,124],max:[136,32,132],material:4}];
    l.units=vec![
        RangeUnitDef{kind:RangeUnitKind::Tank,at:[80,16,128],allied:true,weapon:None},
        RangeUnitDef{kind:RangeUnitKind::Tank,at:[500,16,500],allied:false,weapon:None},
    ];
    let mut r=BlastRange::with_layout(config,l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    assert!(r.set_ai_vs_ai(true)); r.start_battle().unwrap();
    r.matter.tuning.max_flying=1; r.matter.tuning.max_fragments=1; r.matter.tuning.max_resting_fragments=0;
    let first=[122,20,126];
    assert_eq!(r.matter.fell(&mut r.world,&[first],[122,20,126],&r.navigation.fell),1);
    r.world.flush();
    r.matter.debris.confirm(r.world.completed(),&mut r.matter.economy,&r.matter.tuning);
    assert_eq!(r.matter.debris.fragments.len(),1);
    let original=r.matter.debris.fragments[0].id;
    r.matter.debris.fragments[0].resting=true;
    r.matter.debris.fragments[0].velocity=phys::V3::ZERO;
    assert_eq!(r.matter.debris.fragment_room(&r.matter.tuning),0);
    let second=[132,20,126];
    assert_eq!(r.matter.fell(&mut r.world,&[second],[132,20,126],&r.navigation.fell),0);
    for _ in 0..25 {
        r.step();
        if r.world.material(second[0],second[1],second[2])==0 {break;}
    }
    assert_eq!(r.world.material(second[0],second[1],second[2]),0);
    assert!(r.matter.debris.fragments.iter().any(|f|f.id==original),"existing settled fragment vanished");
    assert!(r.matter.tuning.max_resting_fragments>0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn autonomous_battle_scales_world_drain_for_a_real_backlog() {
    let config=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut l=RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size=[768,128,768]; l.clear_min=[40,40]; l.clear_max=[728,728];
    l.course=None; l.blocks.clear(); l.shields.clear();
    l.units=vec![
        RangeUnitDef{kind:RangeUnitKind::Tank,at:[80,16,128],allied:true,weapon:None},
        RangeUnitDef{kind:RangeUnitKind::Tank,at:[500,16,500],allied:false,weapon:None},
    ];
    let mut r=BlastRange::with_layout(config,l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    assert!(r.set_ai_vs_ai(true)); r.start_battle().unwrap();
    r.world.max_edits_per_tick=1;
    for x in 300..340 { r.world.queue_edit(voxel::Edit::put(x,100,300,3)); }
    assert_eq!(r.world.queued(),40);
    r.battle.ai.next_tick=r.tick+r.battle.ai.tuning.decision_ticks;
    r.step();
    assert!(r.world.queued()<=36,"AI world drain did not increase for the backlog");
    assert_eq!(r.world.max_edits_per_tick,1,"adaptive quota leaked into the configured setting");
}
