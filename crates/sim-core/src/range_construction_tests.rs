use super::*;
use crate::blast::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout};

fn range() -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.idle_enemies = true;
    layout.size = [3200, 1024, 3200];
    layout.blocks = vec![
        RangeBlock { min: [0, 0, 0], max: [3200, 1024, 3200], material: 0 },
        RangeBlock { min: [0, 0, 0], max: [3200, 16, 3200], material: 1 },
    ];
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [3136, 3136];
    layout.units = vec![
        RangeUnitDef { allied: true, kind: RangeUnitKind::GrebenStorage, at: [900, 16, 1000], weapon: None },
        RangeUnitDef { allied: false, kind: RangeUnitKind::Tank, at: [2900, 16, 2800], weapon: None },
    ];
    let mut economy = modding::range_economy::RangeEconomy::stock();
    economy.bank.as_mut().unwrap().base_store = 1 << 40;
    layout.economy = Some(economy);
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    ).unwrap();
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let seeded = 1 << 30;
    let economy = &mut r.matter.economy;
    assert_eq!(economy.ledger.reveal(economy.ground, seeded), seeded);
    assert_eq!(economy.ledger.transfer(economy.ground, store, seeded), seeded);
    r.start_battle().unwrap();
    r
}

#[test]
fn greben_starts_with_eight_real_drones_and_one_crawler() {
    let r = range();
    assert_eq!(r.units.iter().filter(|u| u.kind == RangeUnitKind::SkobaDrone).count(), 8);
    assert_eq!(r.units.iter().filter(|u| u.kind == RangeUnitKind::Crawler).count(), 1);
    assert!(r.units.iter().filter(|u| u.kind == RangeUnitKind::SkobaDrone).all(|u| u.status.alive));
}

#[test]
fn drones_split_six_two_and_build_two_turrets_from_real_batches() {
    let mut r = range();
    let first = r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000]).unwrap();
    let second = r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500]).unwrap();
    assert!(r.set_construction_quota(first, 6));
    assert!(r.set_construction_quota(second, 2));
    r.step();
    let sites = r.construction_sites();
    assert_eq!(sites[first].assigned, 6);
    assert_eq!(sites[second].assigned, 2);
    for _ in 0..6000 {
        r.step();
        if r.construction_sites().iter().all(|s| s.complete) { break; }
    }
    let sites = r.construction_sites();
    assert!(sites.iter().all(|s| s.complete), "{sites:?}");
    assert_eq!(r.units.iter().filter(|u| u.kind == RangeUnitKind::TurretT1).count(), 2);
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert!(c.trips >= 2 && c.delivered > 0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn a_shot_down_loaded_drone_leaves_one_reclaimable_batch() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000]).unwrap();
    assert!(r.set_construction_quota(0, 1));
    let mut loaded = None;
    for _ in 0..500 {
        r.step();
        loaded = (0..r.units.len()).find(|&id| r.drone_cargo(id).is_some());
        if loaded.is_some() { break; }
    }
    let id = loaded.expect("a drone picked up a batch");
    let before = r.side_store(true).unwrap().0;
    r.units[id].status.alive = false;
    r.construction_phase();
    assert!(r.drone_cargo(id).is_none());
    assert_eq!(r.side_store(true).unwrap().0, before);
    assert_eq!(r.wreck_cargo(id), Some(332_800));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_upgrade_buy_four_parked_drones_only_once() {
    let mut r = range();
    let crawler = r.units.iter().position(|u| u.kind == RangeUnitKind::Crawler).unwrap();
    assert!(r.upgrade_crawler_drones(crawler));
    assert!(!r.upgrade_crawler_drones(crawler));
    let parked: Vec<_> = r.units.iter().enumerate()
        .filter(|(_, u)| u.kind == RangeUnitKind::SkobaDrone && u.body.vol.get(64, 16, 26))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(parked.len(), 4);
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert!(parked.iter().all(|id| c.drones.get(&(*id as u16)).is_some_and(|d| usize::from(d.home) == crawler)));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn a_loaded_flight_restores_and_continues_bit_for_bit() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000]).unwrap();
    for _ in 0..500 {
        r.step();
        if (0..r.units.len()).any(|id| r.drone_cargo(id).is_some()) { break; }
    }
    assert!((0..r.units.len()).any(|id| r.drone_cargo(id).is_some()));
    let bytes = r.snapshot();
    let mut restored = BlastRange::restore(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        &bytes,
    ).expect("loaded flight restores");
    assert_eq!(r.state_hash(), restored.state_hash());
    for _ in 0..120 {
        r.step();
        restored.step();
    }
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn ready_menu_scenarios_start_real_construction() {
    for id in [24, 25, 26] {
        let preset = modding::range_skirmish::stock(id).unwrap();
        let layout = modding::range::RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        let layout = preset.apply_to(layout).unwrap();
        let mut r = BlastRange::with_layout(
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
            layout,
        ).unwrap();
        assert_eq!(r.units.iter().filter(|u| u.kind == RangeUnitKind::SkobaDrone).count(), 8);
        if matches!(id, 24 | 26) {
            let factory = if id == 24 { 0 } else { 1 };
            assert_eq!(r.production_queue(factory), Some(vec![RangeUnitKind::Tank]));
            assert!(r.side_store(true).unwrap().0 > 0);
        } else {
            let sites = r.construction_sites();
            assert_eq!(sites.len(), 2);
            assert_eq!((sites[0].quota, sites[1].quota), (6, 2));
        }
        r.start_battle().unwrap();
        r.step();
        if id == 25 {
            assert!((0..r.units.len()).any(|id| r.drone_leg(id) == Some(Leg::ToSource)));
        }
    }
}
