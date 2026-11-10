//! Acceptance: a physical tool must aim before spending mass or emitting work.
use super::*;
use crate::blast::battle::Order;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef};

fn scene(repair: bool) -> BlastRange {
    let mut l=RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size=[1280,192,1024]; l.battle=true; l.idle_enemies=true;
    l.course=None; l.ready=None; l.shields.clear();
    l.clear_min=[1,1]; l.clear_max=[1279,1023];
    l.blocks=vec![RangeBlock{min:[0;3],max:[1280,192,1024],material:0},
        RangeBlock{min:[0;3],max:[1280,16,1024],material:1}];
    l.units=vec![RangeUnitDef{kind:RangeUnitKind::Commander,allied:true,at:[400,16,400],weapon:None},
        RangeUnitDef{kind:RangeUnitKind::Tank,allied:false,at:[1150,16,850],weapon:None}];
    if repair {l.units.push(RangeUnitDef{kind:RangeUnitKind::Tank,allied:true,at:[750,16,400],weapon:None});}
    let mut e=modding::range_economy::RangeEconomy::stock();
    e.production.as_mut().unwrap().producers.retain(|p|p.kind==RangeUnitKind::GrebenStorage);
    l.economy=Some(e);
    let cfg=modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r=BlastRange::with_saved_layout(cfg,l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_battle().unwrap();
    let tank=r.mining().unwrap().tank(0).unwrap();
    assert_eq!(r.matter.economy.ledger.generate(tank,100_000_000),100_000_000);
    r
}

#[test]
fn commander_work_waits_for_actual_build_arm_before_power_mass_and_beam() {
    let mut r=scene(false);
    let target=[750,16,400];
    let site=r.create_construction_site(RangeUnitKind::TurretT1,true,target).unwrap();
    r.set_construction_quota(site,0);
    assert!(r.battle_order(0,Order::Build(site),None));
    assert!(!r.units[0].commander_arm_ready(1,target));
    assert_eq!(r.construction_power_networks()[0].unwrap().1,0,"a lowered tool must not reserve printing power");
    let initial=r.tank_held(0).unwrap();
    let mut started=false;
    for tick in 0..100 {
        let ready=r.units[0].commander_arm_ready(1,target);
        let before=r.construction_sites()[site].delivered;
        r.step();
        let after=r.construction_sites()[site].delivered;
        if !ready {
            assert_eq!(after,before,"printing started before the tool finished raising");
            assert!(r.construction_print_traces().is_empty());
        }
        if after>0 {
            assert!(tick>0);
            assert!(r.units[0].commander_arm_ready(1,target));
            assert!(!r.construction_print_traces().is_empty());
            assert_eq!(initial-r.tank_held(0).unwrap(),after);
            started=true; break;
        }
    }
    assert!(started,"physical alignment must eventually allow paid work");
    assert!(r.matter.economy.ledger.balanced());
    let out=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/requirements-547");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("printing.bin"),r.snapshot()).unwrap();
    let mut section=("",0,None);
    let mut copy=BlastRange::restore_observed(r.config,&r.snapshot(),|a,b,c|section=(a,b,c))
        .unwrap_or_else(||panic!("active tool snapshot refused at {section:?}"));
    assert_eq!(r.state_hash(),copy.state_hash());
    r.step(); copy.step();
    assert_eq!(r.state_hash(),copy.state_hash());
    let held=r.tank_held(0);
    let delivered=r.construction_sites()[site].delivered;
    r.units[0].seg_gone[17]=true;
    // Isolate printing: a full tick still generates passive matter legitimately.
    r.construction_phase();
    assert_eq!(r.tank_held(0),held);
    assert_eq!(r.construction_sites()[site].delivered,delivered);
    assert!(r.construction_print_traces().is_empty());
}

#[test]
fn commander_work_repair_waits_for_the_real_tool_and_restores_during_aiming() {
    let mut r=scene(true);
    let cell=(0..usize::from(r.units[2].body.vol.nz)).find_map(|z| (0..usize::from(r.units[2].body.vol.ny)).find_map(|y|
        (0..usize::from(r.units[2].body.vol.nx)).find_map(|x| (r.units[2].body.vol.get(x,y,z)
            && r.units[2].body.part_at(x,y,z)==voxel::unit::Part::Armour).then_some([x,y,z])))).unwrap();
    r.matter.annihilate_body(2,1);
    assert!(r.units[2].body.remove(cell[0],cell[1],cell[2]));
    r.units[2].removed.push(cell.map(|v|v as u8)); r.units[2].refresh_damage(2);
    assert!(r.battle_order(0,Order::Repair(2),None));
    let held=r.tank_held(0).unwrap();
    r.construction_phase();
    assert_eq!(r.tank_held(0),Some(held));
    assert!(!r.units[2].body.vol.get(cell[0],cell[1],cell[2]));
    let mut copy=BlastRange::restore(r.config,&r.snapshot()).unwrap();
    for _ in 0..100 {
        r.tick+=1; copy.tick+=1;
        r.construction_phase(); copy.construction_phase();
        assert_eq!(r.state_hash(),copy.state_hash());
        if r.units[2].body.vol.get(cell[0],cell[1],cell[2]) {break;}
    }
    assert!(r.units[2].body.vol.get(cell[0],cell[1],cell[2]),"aligned arm must actually repair the missing cell");
    assert!(r.tank_held(0).unwrap()<held);
    assert!(r.matter.economy.ledger.balanced());
}
