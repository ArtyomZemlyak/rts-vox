use super::*;
use crate::blast::BlastRange;
use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};
use modding::range_economy::RangeEconomy;

fn scene(physical: bool) -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    layout.units = [RangeUnitKind::Tank, RangeUnitKind::ArtilleryT2, RangeUnitKind::Walker04]
        .into_iter().enumerate().map(|(id, kind)| RangeUnitDef {
            kind, allied: true, at: [200 + id as i32 * 180, 16, 400], weapon: None,
        }).collect();
    layout.size[1] = 768;
    let mut economy = RangeEconomy::stock();
    if !physical { economy.balance=None; economy.dig_periods.clear(); }
    for kind in &mut economy.kinds { kind.body_density = if physical { 785 } else { 0 }; }
    layout.economy = Some(economy);
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    BlastRange::with_layout(config, layout).unwrap()
}

#[test]
fn physical_economy_prices_real_body_volume_and_preserves_fragments_and_restore() {
    let mut r = scene(true);
    for id in 0..r.units.len() {
        let u = &r.units[id];
        let per = (785 * u64::from(u.body.mm).pow(3)).div_ceil(1_000_000);
        assert_eq!(r.matter.body_quantum(id, 0), per);
        let expected: u64 = std::iter::once(&u.body).chain(u.body.segments.iter().map(|s| &s.body))
            .map(|b| u64::from(b.solid_count()) * (785 * u64::from(b.mm).pow(3)).div_ceil(1_000_000)).sum();
        assert_eq!(r.matter.template_matter(u), expected);
        let held = r.matter.body_container(id);
        assert_eq!(r.matter.economy.ledger.held(held), expected);
        let cell = (0..usize::from(u.body.vol.nz)).find_map(|z| {
            (0..usize::from(u.body.vol.ny)).find_map(|y| {
                (0..usize::from(u.body.vol.nx)).find_map(|x| {
                    (u.body.vol.get(x, y, z) && u.body.part_at(x, y, z) == voxel::Part::Armour).then_some([x, y, z])
                })
            })
        }).unwrap();
        let frame = u.root_frame(cell[1]);
        r.matter.begin(u32::MAX, 4);
        assert!(matches!(r.matter.body(id, u, cell, &frame, [1, 16, 1]), ClaimResult::Applied));
        assert_eq!(r.matter.economy.ledger.held(held), expected - per);
        let fragment = r.matter.debris.fragments.last().unwrap();
        assert_eq!(r.matter.economy.ledger.held(fragment.held), per);
        assert_eq!(fragment.mass_g, per * u64::from(r.matter.tuning.density_kg_m3_per_density));
        let u = &mut r.units[id];
        assert!(u.body.remove(cell[0], cell[1], cell[2]));
        u.removed.push(cell.map(|v| v as u8));
        u.refresh_damage(id);
    }
    assert!(r.matter.economy.ledger.balanced());
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("physical body quantities restore");
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn legacy_economy_keeps_intrinsic_cell_prices_after_loading_with_new_stock() {
    let r = scene(false);
    assert_eq!(r.matter.body_quantum(1, 0), 785);
    assert_eq!(r.matter.body_quantum(2, 0), 785);
    let saved = r.snapshot();
    let restored = BlastRange::restore(r.config, &saved).expect("old economy without grades remains compatible");
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(restored.matter.body_quantum(2, 0), 785);
}

fn balance_scene(with_commander: bool) -> BlastRange {
    use modding::range::RangeBlock;
    let mut l=RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size=[2400,384,2400]; l.battle=true; l.idle_enemies=true;
    l.course=None; l.shields.clear(); l.ready=None;
    l.clear_min=[1,1]; l.clear_max=[2399,2399];
    l.blocks=vec![RangeBlock{min:[0,0,0],max:[2400,384,2400],material:0},
        RangeBlock{min:[0,0,0],max:[2400,16,2400],material:1}];
    l.units=vec![RangeUnitDef{kind:RangeUnitKind::KilFactoryT1,allied:true,at:[1400,16,1200],weapon:None}];
    l.units.push(RangeUnitDef{kind:if with_commander {RangeUnitKind::Commander} else {RangeUnitKind::Base},
        allied:true,at:if with_commander {[1000,16,1200]} else {[2100,16,2100]},weapon:None});
    let mut e=RangeEconomy::stock();
    // This matrix exercises T1 production, its upgrade and a drone delivery.
    // Do not build unrelated T3 world-voxel exit masks in a focused test.
    e.production.as_mut().unwrap().producers.retain(|p|matches!(p.kind,RangeUnitKind::KilFactoryT1|RangeUnitKind::GrebenStorage));
    for producer in &mut e.production.as_mut().unwrap().producers {
        producer.produces.retain(|k|matches!(k,RangeUnitKind::Tank|RangeUnitKind::KilFactoryT2|RangeUnitKind::SkobaDrone));
    }
    e.production.as_mut().unwrap().supply_radius=0; // isolate electrical/material accounting from pipeline payment
    e.production.as_mut().unwrap().pipeline_cost_per_cell=0;
    l.economy=Some(e);
    let c=modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let r=BlastRange::with_saved_layout(c,l).unwrap();
    r
}

fn balance_restore(r: &BlastRange) -> BlastRange {
    let mut section=("",0,None);
    BlastRange::restore_observed(r.config,&r.snapshot(),|a,b,c|section=(a,b,c))
        .unwrap_or_else(||panic!("economic snapshot refused at {section:?}"))
}

/// Economic phases at 20 Hz, without render/AI/combat/navigation cost. This
/// measures paid work, not the analytical rate or a wall-clock benchmark.
#[test]
fn balance_pacing_measures_bootstrap_and_audits_the_entire_catalogue() {
    use crate::blast::battle::Order;
    let mut measurements = Vec::new();
    for old in [true, false] {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size = [4000, 512, 3008]; l.battle = true; l.idle_enemies = true;
    l.course = None; l.shields.clear(); l.ready = None;
    l.clear_min = [1, 1]; l.clear_max = [3999, 3007];
    l.blocks = vec![
        modding::range::RangeBlock { min: [0; 3], max: [4000,512,3008], material: 0 },
        modding::range::RangeBlock { min: [0; 3], max: [4000,16,3008], material: 1 },
    ];
    l.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Commander, allied: true, at: [400,16,400], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::KilFactoryT1, allied: true, at: [2200,16,800], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Tank, allied: false, at: [3700,16,2600], weapon: None },
    ];
    let mut economy = RangeEconomy::stock();
    // Fixed pre-review reference, with the same live geometry and execution
    // path. It is test input, never an alternate gameplay economy.
    if old {
        let b = economy.balance.as_mut().unwrap();
        b.print_per_second = 2_000_000; b.commander_matter_per_second = 172_700;
        b.generation = [600,2000,500,3000,12000];
        for (kind, density) in [(RangeUnitKind::PowerPlantT1,261),
            (RangeUnitKind::PowerPlantT2,224),(RangeUnitKind::PowerPlantT3,418)] {
            economy.kind_mut(kind).unwrap().body_density = density;
        }
    }
    economy.production.as_mut().unwrap().producers.retain(|p| p.kind == RangeUnitKind::KilFactoryT1);
    economy.production.as_mut().unwrap().producers[0].produces = vec![RangeUnitKind::Tank];
    l.economy = Some(economy);
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_saved_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_battle().unwrap();
    let rules = r.economic_rules().clone();
    let tank = r.mining().unwrap().tank(0).unwrap();
    let ground = r.matter.economy.ground;
    eprintln!("PACING old={old} rules: print={} IU/s, passive={} IU/s, generation={:?}",
        rules.print_per_second, rules.commander_matter_per_second, rules.generation);
    eprintln!("CATALOG kind,matter_IU,full_print_ticks,solo_print_ticks,product_ticks,power,energy");
    for kind in RangeUnitKind::ALL {
        let price = r.matter.kind_matter(kind);
        let k = r.mining().unwrap().data().kind(kind).unwrap();
        let full = (price * crate::TICKS_PER_SECOND as u64).div_ceil(rules.print_per_second);
        let solo = (price * crate::TICKS_PER_SECOND as u64 * rules.print_power)
            .div_ceil(rules.print_per_second * rules.generation[0].min(rules.print_power));
        assert!(price > 0 && full > 0 && solo >= full, "{kind:?}");
        eprintln!("CATALOG {kind:?},{price},{full},{solo},{},{},{}",
            k.build_secs * crate::TICKS_PER_SECOND as u32, rules.generation(kind), r.construction_energy_estimate(price));
    }
    for (name, kind, pos, funded, new_limit, old_limit) in [
        ("generator_solo", RangeUnitKind::PowerPlantT1, [400,16,400], true, 280, 700),
        ("generator_with_factory", RangeUnitKind::PowerPlantT1, [2200,16,400], true, 360, 850),
        ("turret_empty", RangeUnitKind::TurretT1, [2500,16,2000], false, 900, 1600),
    ] {
        r.units[0].motion.pos = pos.map(fx::Fx::from_int);
        r.units[0].motion.drive.head = 0; r.units[0].update_pose(0);
        let held = r.tank_held(0).unwrap();
        assert_eq!(r.matter.economy.ledger.transfer(tank, ground, held), held);
        if funded { assert_eq!(r.matter.economy.ledger.generate(tank, rules.initial_matter), rules.initial_matter); }
        if name == "generator_with_factory" {
            for _ in 0..3 { assert!(r.enqueue_product(1,RangeUnitKind::Tank,true).accepted()); }
        }
        let at = [pos[0]+300,16,pos[2]];
        assert!(r.can_construct(kind, true, at), "{name}");
        let site = r.create_construction_site(kind,true,at).unwrap();
        r.set_construction_quota(site,0);
        assert!(r.battle_order(0,Order::Build(site),None));
        let mut finished = None;
        for tick in 1..=if old { old_limit } else { new_limit } {
            r.tick += 1; r.commander_matter_phase(); r.construction_phase(); r.production_phase();
            assert!(r.matter.economy.ledger.balanced());
            if r.construction_sites()[site].complete { finished = Some(tick); break; }
        }
        let ticks = finished.unwrap_or_else(|| panic!("{name} stalled: {:?}",r.construction_sites()[site]));
        eprintln!("PACING old={old} {name}: {ticks} ticks, price={} IU",r.construction_cost(kind).unwrap());
        measurements.push(ticks);
    }
    // Teleports and partial phases intentionally omit movement/AI updates;
    // their intermediate state is not a whole-tick replay checkpoint.
    // Full-tick save/restore is covered by the autonomous opening test.
    }
    assert!(measurements[3]*2 <= measurements[0], "{measurements:?}");
    assert!(measurements[4]*2 <= measurements[1], "{measurements:?}");
    assert!(measurements[5]*4 <= measurements[2]*3, "{measurements:?}");
}

#[test]
fn balance_factory_progress_requires_paid_material_and_local_power() {
    use crate::blast::battle::{commands::Outcome,Phase};
    let mut r=balance_scene(false);
    assert_eq!(r.battle.phase,Phase::Setup);
    assert_eq!(r.enqueue_product(0,RangeUnitKind::Tank,true),Outcome::Applied);
    let cost=r.production_cost(RangeUnitKind::Tank).unwrap();
    let store=r.mining.as_ref().unwrap().haul().unwrap().store(true);
    r.matter.economy.ledger.generate(store,cost);
    for _ in 0..20 { r.tick+=1; r.production_phase(); }
    assert_eq!(r.production_progress(0).unwrap().0,0,"remote base supplies no electricity");
    assert_eq!(r.production_paid(0),Some(0));
    r.units[1].motion.pos=[1400,16,1900].map(fx::Fx::from_int);
    r.units[1].update_pose(0);
    // Spend exactly half the input. No unpaid progress may accumulate.
    let ground=r.matter.economy.ground;
    r.matter.economy.ledger.transfer(store,ground,cost-cost/2);
    for _ in 0..180 { r.tick+=1; r.production_phase(); }
    let before=r.production_progress(0).unwrap();
    assert!(before.0>=98 && before.0<=100,"{before:?}");
    for _ in 0..40 { r.tick+=1; r.production_phase(); }
    assert_eq!(r.production_progress(0).unwrap(),before);
    let mut back=balance_restore(&r);
    assert_eq!(r.state_hash(),back.state_hash());
    let rest=cost-r.production_paid(0).unwrap();
    r.matter.economy.ledger.transfer(ground,store,rest);
    back.matter.economy.ledger.transfer(ground,store,rest);
    for _ in 0..110 {
        r.tick+=1; back.tick+=1; r.production_phase(); back.production_phase();
        assert_eq!(r.state_hash(),back.state_hash());
    }
    assert!(r.production_queue(0).is_none());
    assert_eq!(r.units.iter().filter(|u|u.kind==RangeUnitKind::Tank).count(),1);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn balance_factory_tiers_accelerate_paid_production_and_capture_their_rules() {
    let mut l=RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size=[4096,512,2304]; l.battle=true; l.idle_enemies=true;
    l.course=None; l.shields.clear(); l.ready=None;
    l.clear_min=[1,1]; l.clear_max=[4095,2303];
    l.blocks=vec![modding::range::RangeBlock{min:[0;3],max:[4096,512,2304],material:0},
        modding::range::RangeBlock{min:[0;3],max:[4096,16,2304],material:1}];
    let factories=[RangeUnitKind::KilFactoryT1,RangeUnitKind::KilFactoryT2,
        RangeUnitKind::KilFactoryT3,RangeUnitKind::KilFactoryT4];
    l.units=factories.into_iter().enumerate().map(|(id,kind)|RangeUnitDef{
        kind,allied:true,at:[800+id as i32*800,16,800],weapon:None}).collect();
    // Small authored power fixtures, rather than four huge T3 plant meshes.
    // This case exercises the lines under ample local power, not plant output.
    for (kind,at) in (0..4).map(|id|(RangeUnitKind::Base,[800+id*800,16,1700])) {
        l.units.push(RangeUnitDef{kind,allied:true,at,weapon:None});
    }
    let mut e=RangeEconomy::stock();
    e.balance.as_mut().unwrap().generation[1]=5000;
    let p=e.production.as_mut().unwrap(); p.supply_radius=0; p.pipeline_cost_per_cell=0;
    p.producers.retain(|p|factories.contains(&p.kind));
    for p in &mut p.producers {p.produces=vec![RangeUnitKind::Tank];}
    l.economy=Some(e);
    let cfg=modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r=BlastRange::with_saved_layout(cfg,l).unwrap();
    let cost=r.production_cost(RangeUnitKind::Tank).unwrap();
    let store=r.mining().unwrap().haul().unwrap().store(true);
    assert_eq!(r.matter.economy.ledger.generate(store,cost*4),cost*4);
    let expected=[200,134,100,50];
    for id in 0..4 {
        let (price,ticks,_) = r.production_quote(id,RangeUnitKind::Tank).unwrap();
        assert_eq!((price,ticks),(cost,expected[id]));
        assert!(r.enqueue_product(id,RangeUnitKind::Tank,true).accepted());
    }
    let mut finished=[None;4];
    for tick in 1..=205 {
        r.tick+=1; r.production_phase();
        if tick==25 {
            let back=balance_restore(&r);
            assert_eq!(r.state_hash(),back.state_hash(),"unfinished lines preserve their throughput");
        }
        for id in 0..4 {
            if finished[id].is_none() && r.production_queue(id).is_none() {finished[id]=Some(tick);}
        }
        assert!(r.matter.economy.ledger.balanced());
    }
    assert_eq!(finished,expected.map(Some));
    assert_eq!(r.units.iter().filter(|u|u.kind==RangeUnitKind::Tank).count(),4);
    assert_eq!(r.matter.economy.ledger.held(store),0,"faster lines pay the same full hull price");
    let back=balance_restore(&r);
    assert_eq!(r.state_hash(),back.state_hash());
    eprintln!("PACING factory T1/T2/T3/T4 tank completion: {finished:?} ticks, each {cost} IU");
}

#[test]
fn balance_finished_product_retries_exit_without_demanding_work_power_again() {
    let mut r=balance_scene(false);
    r.units[1].motion.pos=[1400,16,1900].map(fx::Fx::from_int); r.units[1].update_pose(0);
    let blocker=r.place_sandbox_unit(RangeUnitKind::Tank,true,[1000,16,1200],0).unwrap();
    assert!(r.free_spot(0,RangeUnitKind::Tank,true).is_none());
    let cost=r.production_cost(RangeUnitKind::Tank).unwrap();
    let store=r.mining().unwrap().haul().unwrap().store(true);
    assert_eq!(r.matter.economy.ledger.generate(store,cost),cost);
    assert!(r.enqueue_product(0,RangeUnitKind::Tank,true).accepted());
    for _ in 0..205 {r.tick+=1; r.production_phase();}
    assert_eq!(r.production_progress(0),Some((200,200,cost,cost)));
    r.units[blocker].motion.pos[2]=fx::Fx::from_int(2000); r.units[blocker].update_pose(0);
    assert!(r.free_spot(0,RangeUnitKind::Tank,true).is_some());
    // The already completed work must release even when its old power
    // source disconnects. No additional mass or productive tick is needed.
    r.units[1].motion.pos=[2300,16,2300].map(fx::Fx::from_int); r.units[1].update_pose(0);
    r.tick+=1; r.production_phase();
    assert!(r.production_queue(0).is_none());
    assert_eq!(r.units.iter().filter(|u|u.kind==RangeUnitKind::Tank).count(),2);
    assert_eq!(r.matter.economy.ledger.held(store),0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn balance_construction_and_factory_share_one_frozen_power_budget() {
    use crate::blast::battle::{commands::Outcome,Order};
    let mut r=balance_scene(true);
    let tank=r.mining.as_ref().unwrap().tank(1).unwrap();
    r.matter.economy.ledger.generate(tank,100_000_000);
    r.units[1].motion.drive.head=fx::angle::FULL/4;
    r.units[1].update_pose(fx::angle::FULL/4);
    let site=r.create_construction_site(RangeUnitKind::TurretT1,true,[1000,16,1500]).unwrap();
    r.set_construction_quota(site,0);
    assert!(r.battle_order(1,Order::Build(site),None));
    assert_eq!(r.enqueue_product(0,RangeUnitKind::Tank,true),Outcome::Applied);
    for _ in 0..200 { r.units[1].set_arm_targets([[1000,16,1500],[1000,16,1500],[1,0,0],[0;3]],fx::angle::FULL/4); }
    assert!(r.units[1].commander_arm_ready(1,[1000,16,1500]));
    let networks=r.construction_power_networks();
    assert_eq!(networks[0],Some((600,1200)));
    assert_eq!(networks[1],Some((600,1200)));
    for _ in 0..20 { r.tick+=1; r.construction_phase(); r.production_phase(); }
    let delivered=r.construction_sites()[site].delivered;
    let expected = r.economic_rules().print_per_second / 2;
    assert!(delivered.abs_diff(expected) <= 10_000,"{delivered} vs {expected}");
    assert_eq!(r.production_progress(0).unwrap().0,10,"factory cannot reuse power already assigned to printing");
    assert!(r.matter.economy.ledger.balanced());
    let back=balance_restore(&r);
    assert_eq!(r.state_hash(),back.state_hash());
}

#[test]
fn balance_factory_upgrade_is_timed_paid_and_preserves_body_value() {
    let mut r=balance_scene(false);
    r.units[1].motion.pos=[1400,16,1900].map(fx::Fx::from_int);
    r.units[1].update_pose(0);
    let store=r.mining.as_ref().unwrap().haul().unwrap().store(true);
    r.matter.economy.ledger.generate(store,300_000_000);
    let original=r.matter.economy.ledger.held(r.matter.body_container(0));
    assert!(r.upgrade_kil(0));
    assert_eq!(r.units[0].kind,RangeUnitKind::KilFactoryT1);
    let (_,ticks,_,delta)=r.production_progress(0).unwrap();
    assert_eq!(delta,r.production_cost(RangeUnitKind::KilFactoryT2).unwrap()-original);
    assert_eq!(ticks as u64,(delta * crate::TICKS_PER_SECOND as u64).div_ceil(r.economic_rules().print_per_second));
    for _ in 0..100 { r.tick+=1; r.production_phase(); }
    assert!(r.production_paid(0).unwrap()>0 && r.units[0].kind==RangeUnitKind::KilFactoryT1);
    let back=balance_restore(&r);
    assert_eq!(r.state_hash(),back.state_hash());
    for step in 100..ticks {
        // The local store holds 1650 M, less than this upgrade's 1763 M.
        // A real mine must resupply it during the job; do not assume an
        // over-cap generate() secretly filled a bigger store.
        if step==200 { assert_eq!(r.matter.economy.ledger.generate(store,15_000_000),15_000_000); }
        r.tick+=1; r.production_phase();
    }
    assert_eq!(r.units[0].kind,RangeUnitKind::KilFactoryT2,
        "progress={:?}, blocked={}, placement={:?}, factory={:?}, base={:?}",
        r.production_progress(0),r.mining.as_ref().unwrap().production().unwrap().blocked,
        crate::blast::placement::Validator::default().runtime_except(&r,RangeUnitKind::KilFactoryT2,r.units[0].at,0,Some(0)),
        r.units[0].bounds(),r.units[1].bounds());
    let body=r.matter.body_container(0);
    assert_eq!(r.matter.economy.ledger.held(body),r.matter.template_matter(&r.units[0]));
    assert_eq!(r.matter.body_quantum(0,0),12_288);
    assert!(r.production_queue(0).is_none());
    assert!(r.matter.economy.ledger.balanced());
    let back=balance_restore(&r);
    assert_eq!(r.state_hash(),back.state_hash());
}

#[test]
fn balance_one_powered_drone_completes_a_nearby_turret_within_twenty_seconds() {
    use crate::blast::battle::Order;
    let base=balance_scene(false);
    let mut l=base.layout.clone().unwrap();
    l.units=vec![
        RangeUnitDef{kind:RangeUnitKind::GrebenStorage,allied:true,at:[500,16,800],weapon:None},
        RangeUnitDef{kind:RangeUnitKind::Base,allied:true,at:[500,16,1400],weapon:None},
        RangeUnitDef{kind:RangeUnitKind::SkobaDrone,allied:true,at:[500,150,800],weapon:None},
    ];
    let mut r=BlastRange::with_saved_layout(base.config,l).unwrap();
    let store=r.mining.as_ref().unwrap().haul().unwrap().store(true);
    let cost=r.construction_cost(RangeUnitKind::TurretT1).unwrap();
    r.matter.economy.ledger.generate(store,cost);
    let site=r.create_construction_site(RangeUnitKind::TurretT1,true,[800,16,800]).unwrap();
    r.set_construction_quota(site,0);
    assert!(r.battle_order(2,Order::Build(site),None));
    let mut finished=None;
    for tick in 1..=20*crate::TICKS_PER_SECOND {
        r.tick+=1; r.construction_phase();
        if r.construction_sites()[site].complete {finished=Some(tick);break;}
    }
    assert!(finished.is_some(),"powered drone failed 20 s target: {:?}, {:?}",r.construction_sites(),r.drone_leg(2));
    eprintln!("balanced single drone: cost={cost}, completion tick={finished:?}");
    assert!(r.matter.economy.ledger.balanced());
    let back=balance_restore(&r);
    assert_eq!(r.state_hash(),back.state_hash());
}

#[test]
fn balance_unique_base_rejects_a_second_construction_but_keeps_storage_available() {
    let r=balance_scene(false);
    assert!(!r.can_construct(RangeUnitKind::Base,true,[700,16,1800]));
    assert!(r.can_construct(RangeUnitKind::GrebenStorage,true,[700,16,1800]));
}
