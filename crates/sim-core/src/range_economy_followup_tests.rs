use super::*;
use modding::range::{RangeBlock,RangeLayout};

fn scene() -> BlastRange {
    let mut l=RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size=[1600,384,1600]; l.battle=true; l.idle_enemies=true; l.course=None; l.ready=None;
    l.shields.clear(); l.clear_min=[1,1];l.clear_max=[1599,1599];
    l.blocks=vec![RangeBlock{min:[0,0,0],max:[1600,384,1600],material:0},
        RangeBlock{min:[0,0,0],max:[1600,16,1600],material:1}];
    l.units=vec![
        RangeUnitDef{kind:RangeUnitKind::Commander,allied:true,at:[500,16,700],weapon:None},
        RangeUnitDef{kind:RangeUnitKind::Base,allied:true,at:[500,16,1200],weapon:None},
    ];
    let mut e=modding::range_economy::RangeEconomy::stock();
    e.production.as_mut().unwrap().producers.retain(|p|p.kind==RangeUnitKind::GrebenStorage);
    e.balance.as_mut().unwrap().greben_crew_drones=2;
    l.economy=Some(e);
    let config=crate::blast::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    BlastRange::with_saved_layout(config,l).unwrap()
}
fn restore(r:&BlastRange)->BlastRange {
    let mut stage=("",0,None);
    BlastRange::restore_observed(r.config,&r.snapshot(),|a,b,c|stage=(a,b,c))
        .unwrap_or_else(||panic!("followup snapshot refused at {stage:?}"))
}
fn tick(r:&mut BlastRange) {
    r.tick+=1;r.construction_phase();r.production_phase();
}

#[test]
fn economy_followup_commander_repairs_from_a_nearby_store_but_cannot_spend_remote_stock() {
    let mut r=scene();
    let target=r.place_sandbox_unit(RangeUnitKind::Tank,true,[750,16,700],0).unwrap();
    assert_eq!(r.spawned().len(),1,"the first sandbox unit must retain its real container IDs");
    assert_eq!(restore(&r).state_hash(),r.state_hash());
    let cell=*connected_cells(&r.units[target].body).iter().rev().find(|p|
        r.units[target].body.part_at(p[0],p[1],p[2])==voxel::unit::Part::Armour).unwrap();
    r.matter.annihilate_body(target,1);
    r.units[target].body.remove(cell[0],cell[1],cell[2]);
    r.units[target].removed.push(cell.map(|v|v as u8));
    r.units[target].refresh_damage(target);
    let store=r.mining().unwrap().haul().unwrap().store(true);
    r.matter.economy.ledger.generate(store,10_000);
    assert_eq!(r.commander_accessible_matter(0),0,"remote store is not liquid commander stock");
    assert!(r.battle_order(0,battle::Order::Repair(target),None));
    r.construction_phase();
    assert_eq!(r.tank_held(0),Some(0));
    assert!(!r.units[target].body.vol.get(cell[0],cell[1],cell[2]));
    for _ in 0..200 { r.units[0].set_arm_targets([[750,16,700],[750,16,700],[1,0,0],[0;3]],0); }
    assert!(r.units[0].commander_arm_ready(1,[750,16,700]));
    r.units[1].motion.pos=[500,16,1000].map(Fx::from_int);
    r.units[1].update_pose(0);
    assert_eq!(r.commander_accessible_matter(0),10_000);
    r.tick+=1;r.construction_phase();
    assert!(r.units[target].body.vol.get(cell[0],cell[1],cell[2]));
    assert_eq!(r.matter.economy.ledger.held(store),0);
    assert_eq!(r.tank_held(0),Some(10_000-r.matter.body_quantum(target,0)));
    assert!(r.matter.economy.ledger.balanced());
    assert_eq!(restore(&r).state_hash(),r.state_hash());
}

#[test]
fn queued_crew_waits_for_resources_uses_power_and_returns_to_its_own_pads() {
    let mut r=scene();
    let tank=r.mining().unwrap().tank(0).unwrap();
    let store=r.mining().unwrap().haul().unwrap().store(true);
    r.matter.economy.ledger.generate(tank,100_000_000);
    let site=r.create_construction_site(RangeUnitKind::GrebenStorage,true,[900,16,700]).unwrap();
    r.set_construction_quota(site,0);
    let price=r.construction_sites()[site].required;
    // Set up a fully paid shell, then exercise real timed crew production.
    assert_eq!(r.commander_print_step(0,site,400,price),price);
    let home=r.unit_construction_site(2);
    assert_eq!(home,None,"the shell must be finished before commissioning");
    let home=2;
    let wanted=vec![RangeUnitKind::Crawler,RangeUnitKind::SkobaDrone,RangeUnitKind::SkobaDrone];
    assert_eq!(r.production_queue(home),Some(wanted.clone()));
    assert_eq!(r.units.len(),3,"no instant crew bodies");
    r.queue_greben_crew(home,0);
    assert_eq!(r.production_queue(home),Some(wanted.clone()),"completion is idempotent");
    let rest=r.tank_held(0).unwrap();
    r.matter.economy.ledger.transfer(tank,r.matter.economy.ground,rest);
    for _ in 0..20 {tick(&mut r);}
    assert_eq!(r.production_progress(home).unwrap().0,0);
    assert_eq!(r.production_paid(home),Some(0));
    let back=restore(&r);assert_eq!(back.state_hash(),r.state_hash());
    let (_,build,crawler_power)=r.production_quote(home,RangeUnitKind::Crawler).unwrap();
    let (crawler_cost,_,_)=r.production_quote(home,RangeUnitKind::Crawler).unwrap();
    let (drone_cost,drone_ticks,drone_power)=r.production_quote(home,RangeUnitKind::SkobaDrone).unwrap();
    assert!(crawler_power>drone_power && crawler_power>0);
    let total=crawler_cost+2*drone_cost;
    assert_eq!(r.matter.economy.ledger.generate(store,total),total);
    // Disconnect electricity while leaving the whole material reserve intact.
    let base=r.units[1].motion.pos;
    let commander=r.units[0].motion.pos;
    r.units[1].motion.pos=[40,16,1550].map(Fx::from_int);
    r.units[0].motion.pos=[40,16,1550].map(Fx::from_int);
    for _ in 0..10 {tick(&mut r);}
    assert_eq!(r.production_progress(home).unwrap().0,0,"no off-grid production");
    r.units[1].motion.pos=base;r.units[0].motion.pos=commander;
    for _ in 0..build-1 {tick(&mut r);}
    assert_eq!(r.units.len(),3,"crawler needs its complete work time");
    let mut back=restore(&r);
    tick(&mut r);tick(&mut back);
    assert_eq!(back.state_hash(),r.state_hash());
    assert_eq!(r.units[3].kind,RangeUnitKind::Crawler);
    assert_eq!(r.production_power(home),drone_power);
    for _ in 0..2*drone_ticks {tick(&mut r);}
    assert!(r.production_queue(home).is_none());
    assert_eq!(r.units.len(),6);
    assert_eq!(r.matter.economy.ledger.held(store),0);
    for _ in 0..200 {r.tick+=1;r.construction_phase();}
    for (rank,id) in [4,5].into_iter().enumerate() {
        assert_eq!(r.units[id].kind,RangeUnitKind::SkobaDrone);
        let c=r.mining.as_ref().unwrap().construction.as_ref().unwrap();
        assert_eq!(usize::from(c.drones[&(id as u16)].home),home);
        assert_eq!(c.drones[&(id as u16)].leg,Leg::Idle);
        assert_eq!(r.units[id].at,r.units[home].world_pos(voxel::unit::GREBEN_DRONE_PADS[rank]));
    }
    assert!(r.matter.economy.ledger.balanced());
    let back=restore(&r);assert_eq!(back.state_hash(),r.state_hash());
    eprintln!("crew work: crawler {build} ticks/{crawler_power} power; drone {drone_ticks} ticks/{drone_power} power; total {total} IU");
}

#[test]
fn queued_crew_can_be_cancelled_without_spending_matter_and_old_rules_remain_optional() {
    let mut r=scene();
    let tank=r.mining().unwrap().tank(0).unwrap();
    r.matter.economy.ledger.generate(tank,100_000_000);
    let site=r.create_construction_site(RangeUnitKind::GrebenStorage,true,[900,16,700]).unwrap();
    let price=r.construction_sites()[site].required;
    assert_eq!(r.commander_print_step(0,site,400,price),price);
    let held=r.production_paid(2).unwrap();
    assert_eq!(held,0);
    assert!(r.execute_command(battle::commands::Command::Cancel{actor:2}).accepted());
    assert_eq!(r.production_queue(2).unwrap().len(),2);
    assert_eq!(r.units.len(),3);
    assert!(r.matter.economy.ledger.balanced());
    let back=restore(&r);assert_eq!(back.state_hash(),r.state_hash());
    assert!(!modding::range_economy::Balance::legacy().queued_crews);
}
