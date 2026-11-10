use super::*;
use crate::burst::{Fragment, IDENTITY};
use phys::V3;
fn lab() -> BlastRange {
    let mut l = modding::laboratory::Definition::stock().layout;
    l.size = [128, 128, 128];
    l.clear_min = [1, 1];
    l.clear_max = [127, 127];
    l.gun_point = true;
    l.gun = [32, 32, 32];
    l.target = [64, 16, 64];
    l.laboratory = true;
    l.units.clear();
    l.blocks.clear();
    l.lab_devices = vec![Device {
        kind: DeviceKind::Collector,
        at: [38, 16, 64],
        turns: 0,
        work: 0,
        dug: 0,
        container: None,
        collector: Some(Collector {
            dock: [24, 16, 64],
            area: [64, 16, 64],
            capacity: 900,
            ..Collector::default()
        }),
    }];
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.control();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r
}
fn piece(r: &mut BlastRange, at: [i32; 3], material: u16) -> u64 {
    let e = &mut r.matter.economy;
    let held = e.ledger.create(Kind::Debris, 0, u64::MAX);
    let amount = e.per_voxel(material);
    e.ledger.reveal(e.ground, amount);
    e.ledger.transfer(e.ground, held, amount);
    r.matter.debris.next_id += 1;
    let id = r.matter.debris.next_id;
    r.matter.debris.fragments.push(Fragment {
        id,
        position: V3::new(
            Fx::from_int(at[0]),
            Fx::from_int(at[1]).add(Fx::from_ratio(1, 2)),
            Fx::from_int(at[2]),
        ),
        velocity: V3::ZERO,
        material,
        paint: None,
        mm: 100,
        basis: IDENTITY,
        mass_g: 2400,
        count: 1,
        shape: 1,
        side: 1,
        chunk: None,
        held,
        resting: true,
    });
    id
}
#[test]
fn lab_collector_collects_locally_hauls_refines_and_restores_mid_route() {
    let mut r = lab();
    for x in [64, 68, 72, 76, 80, 84] {
        piece(&mut r, [x, 16, 64], 2);
    }
    let before = r.matter.economy.ledger.initial();
    let mut seen_travel = false;
    let mut copy = None;
    for tick in 0..360 {
        r.step();
        let d = &r.layout.as_ref().unwrap().lab_devices[0];
        let c = d.collector.as_ref().unwrap();
        seen_travel |= d.at[0] > 40;
        if tick < 10 {
            assert_eq!(c.picked, 0, "must drive to distant debris");
        }
        if tick == 35 {
            copy = Some(BlastRange::restore(r.config, &r.snapshot()).expect("collector snapshot"));
        } else if let Some(copy) = copy.as_mut() {
            copy.step();
            assert_eq!(r.state_hash(), copy.state_hash());
        }
        assert!(r.matter.economy.ledger.balanced());
    }
    let d = &r.layout.as_ref().unwrap().lab_devices[0];
    let c = d.collector.as_ref().unwrap();
    assert!(
        seen_travel && c.pieces == 6 && c.trips >= 2,
        "crawler failed loop: {d:?}"
    );
    assert!(c.delivered > 0 && r.matter.economy.ledger.held(d.container.unwrap()) == 0);
    assert!(r.matter.debris.fragments.is_empty());
    assert_eq!(before, r.matter.economy.ledger.initial());
}
#[test]
fn lab_collector_does_not_vacuum_flying_far_or_oversized_pieces_and_full_depot_keeps_cargo() {
    let mut r = lab();
    let near = piece(&mut r, [43, 16, 64], 2);
    piece(&mut r, [90, 90, 64], 2);
    let c = r.layout.as_mut().unwrap().lab_devices[0]
        .collector
        .as_mut()
        .unwrap();
    c.area = [38, 16, 64];
    for _ in 0..4 {
        r.step();
    }
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == near));
    let d = &r.layout.as_ref().unwrap().lab_devices[0];
    let tank = d.container.unwrap();
    let depot = d.collector.as_ref().unwrap().depot.unwrap();
    r.matter.economy.ledger.lower_cap(depot, 0);
    let held = r.matter.economy.ledger.held(tank);
    let loss = r
        .matter
        .economy
        .ledger
        .sunk(crate::matter::Sink::RefiningLoss);
    for _ in 0..80 {
        r.step();
    }
    assert!(r.matter.economy.ledger.held(tank) >= held);
    assert_eq!(
        r.matter
            .economy
            .ledger
            .sunk(crate::matter::Sink::RefiningLoss),
        loss
    );
    assert!(r.matter.economy.ledger.balanced());
}
#[test]
fn lab_collector_authored_explosion_scenario_reclaims_real_debris() {
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "crawler-salvage")
        .unwrap();
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, e.layout).unwrap();
    r.control();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    assert!(r
        .execute_command(super::super::battle::commands::Command::Gun(
            super::super::battle::commands::GunCommand::Detonate { at: [396, 19, 336] }
        ))
        .accepted());
    let mut peak = 0;
    for _ in 0..900 {
        r.step();
        peak = peak.max(r.matter.debris.fragments.len());
        assert!(r.matter.economy.ledger.balanced());
    }
    let d = &r.layout.as_ref().unwrap().lab_devices[0];
    let c = d.collector.as_ref().unwrap();
    assert!(
        peak > 0 && c.pieces > 0 && c.delivered > 0 && c.trips > 0,
        "scenario did not demonstrate salvage: {d:?}, peak={peak}"
    );
    eprintln!(
        "SALVAGE pieces={} picked={} delivered={} trips={} left={} peak={peak}",
        c.pieces,
        c.picked,
        c.delivered,
        c.trips,
        r.matter.debris.fragments.len()
    );
}
#[test]
fn lab_collectors_share_fragments_without_duplicate_credit_and_route_around_a_wall() {
    let mut r = lab();
    let mut other = r.layout.as_ref().unwrap().lab_devices[0].clone();
    other.at = [38, 16, 44];
    other.collector.as_mut().unwrap().dock = [24, 16, 44];
    r.layout.as_mut().unwrap().lab_devices.push(other);
    for x in [66, 72, 80] {
        piece(&mut r, [x, 16, 64], 2);
    }
    for y in 16..24 {
        for z in 54..74 {
            for x in 48..50 {
                r.world.queue_edit(voxel::Edit::put(x, y, z, 2));
            }
        }
    }
    r.world.flush();
    let total = r
        .matter
        .debris
        .fragments
        .iter()
        .map(|f| r.matter.economy.ledger.held(f.held))
        .sum::<u64>();
    for _ in 0..600 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
    }
    let devices = &r.layout.as_ref().unwrap().lab_devices;
    assert_eq!(
        devices
            .iter()
            .map(|d| d.collector.as_ref().unwrap().picked)
            .sum::<u64>(),
        total,
        "duplicate or missing salvage: {devices:?}"
    );
}

#[test]
fn lab_collector_restores_partially_disassembled_piece_without_duplicate_material() {
    let mut r = lab();
    r.layout.as_mut().unwrap().lab_devices[0]
        .collector
        .as_mut()
        .unwrap()
        .capacity = 20000;
    piece(&mut r, [43, 18, 64], 2);
    let f = &mut r.matter.debris.fragments[0];
    f.count = 64;
    f.side = 4;
    f.shape = u64::MAX;
    f.mass_g *= 64;
    let held = f.held;
    let e = &mut r.matter.economy;
    let extra = e.per_voxel(2) * 63;
    e.ledger.reveal(e.ground, extra);
    e.ledger.transfer(e.ground, held, extra);
    r.step();
    assert!(
        r.layout.as_ref().unwrap().lab_devices[0]
            .collector
            .as_ref()
            .unwrap()
            .spent
            > 0
    );
    assert_eq!(
        r.matter.economy.ledger.held(held),
        15360,
        "no early credit while disassembly is unfinished"
    );
    let mut back = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..20 {
        r.step();
        back.step();
        assert_eq!(r.state_hash(), back.state_hash());
        assert!(r.matter.economy.ledger.balanced());
    }
    assert_eq!(
        r.layout.as_ref().unwrap().lab_devices[0]
            .collector
            .as_ref()
            .unwrap()
            .pieces,
        1
    );
}

#[test]
fn real_crawler_drives_digs_delivers_to_a_live_base_and_restores() {
    use super::super::battle::Order;
    use modding::range::RangeUnitKind;
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "crawler-base")
        .unwrap();
    let mut l = e.layout;
    l.size = [384, 128, 320];
    l.clear_min = [1, 1];
    l.clear_max = [383, 319];
    l.gun = [96, 24, 128];
    l.target = [290, 18, 128];
    l.units[0].at = [120, 16, 128];
    l.units[1].at = [48, 16, 220];
    l.blocks[0].min = [280, 1, 120];
    l.blocks[0].max = [302, 16, 136];
    l.economy
        .as_mut()
        .unwrap()
        .kind_mut(RangeUnitKind::Crawler)
        .unwrap()
        .tank = 5200;
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    assert_eq!(r.units[0].kind, RangeUnitKind::Crawler);
    assert_eq!(r.units[1].kind, RangeUnitKind::Base);
    assert!(
        r.battle_order(0, Order::Dig([290, 8, 128]), None),
        "unit status: {:?}; digs: {}; battle: {}; phase: {:?}",
        r.units[0].status,
        r.mining().is_some_and(|m| m.digs(RangeUnitKind::Crawler)),
        r.battle_mode(),
        r.battle.phase
    );
    let mut moved = false;
    let mut copied = None;
    for tick in 0..1500 {
        r.step();
        moved |= r.units[0].motion.pos[0].to_int_trunc() > 125;
        if tick == 20 {
            let mut c = BlastRange::restore(r.config, &r.snapshot()).unwrap();
            c.set_physics_workers(1);
            c.set_navigation_workers(1);
            copied = Some(c);
        } else if tick < 25 {
            if let Some(c) = copied.as_mut() {
                c.step();
                assert_eq!(r.state_hash(), c.state_hash());
            }
        }
        assert!(r.matter.economy.ledger.balanced());
        if r.side_store(true).unwrap().0 > 0 {
            break;
        }
    }
    assert!(moved, "crawler never approached the face");
    assert!(r.side_store(true).unwrap().0 > 0, "crawler failed to deliver to the base: pos {:?}, target {:?}, enabled {}, blocked {}, status {:?}, tank {:?}, hauling {}, phase {:?}", r.units[0].motion.pos.map(Fx::to_int_trunc),r.units[0].motion.destination,r.units[0].motion.enabled,r.units[0].motion.blocked,r.units[0].status,r.tank_held(0),r.hauling(0),r.battle.phase);
}

#[test]
fn commander_reclaims_real_debris_into_its_tank() {
    use super::super::battle::Order;
    let e = modding::laboratory::experiments::stock()
        .into_iter().find(|e| e.id == "crawler-base").unwrap();
    let mut l = e.layout;
    l.size = [800, 1024, 800];
    l.clear_min = [1, 1];
    l.clear_max = [799, 799];
    l.gun = [96, 24, 128];
    l.target = [180, 16, 128];
    l.units[0].kind = modding::range::RangeUnitKind::Commander;
    l.units[0].at = [300, 16, 300];
    l.units[1].at = [650, 16, 650];
    l.blocks.clear();
    l.economy = Some(modding::range_economy::RangeEconomy::stock());
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    let id = piece(&mut r, [360, 16, 300], 2);
    assert!(r.battle_order(0, Order::Reclaim([360, 16, 300]), None));
    for _ in 0..80 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
        if !r.matter.debris.fragments.iter().any(|f| f.id == id) { break; }
    }
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == id));
    assert!(r.tank_held(0).unwrap() > 0);
    assert!(!r.hauling(0));
}

#[test]
fn real_crawler_reclaims_actual_debris_into_its_own_tank() {
    use super::super::battle::Order;
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "crawler-base")
        .unwrap();
    let mut l = e.layout;
    l.size = [256, 128, 320];
    l.clear_min = [1, 1];
    l.clear_max = [255, 319];
    l.gun = [96, 24, 128];
    l.target = [150, 16, 128];
    l.units[0].at = [120, 16, 128];
    l.units[1].at = [48, 16, 220];
    l.blocks.clear();
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    let id = piece(&mut r, [145, 16, 128], 2);
    assert!(r.battle_order(0, Order::Reclaim([145, 16, 128]), None));
    for _ in 0..80 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
        if !r.matter.debris.fragments.iter().any(|f| f.id == id) {
            break;
        }
    }
    assert!(
        !r.matter.debris.fragments.iter().any(|f| f.id == id),
        "real unit did not reclaim the fragment"
    );
    assert!(
        r.tank_held(0).unwrap() >= 240,
        "reclaimed material did not enter the unit tank"
    );
}

#[test]
fn crawler_continues_to_next_pile_before_unloading_partial_cargo() {
    use super::super::battle::Order;
    let mut l = modding::laboratory::experiments::stock().into_iter()
        .find(|e| e.id == "crawler-base").unwrap().layout;
    l.size = [640, 128, 320];
    l.clear_min = [1, 1]; l.clear_max = [639, 319];
    l.gun = [96, 24, 128]; l.target = [180, 16, 128];
    l.units[0].at = [120, 16, 128]; l.units[1].at = [48, 16, 220];
    l.blocks.clear();
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    let first = piece(&mut r, [180, 16, 128], 2);
    let second = piece(&mut r, [390, 16, 128], 2);
    assert!(r.battle_order(0, Order::Reclaim([180, 16, 128]), None));
    for _ in 0..100 {
        r.step();
        if matches!(r.battle.actors[0].order, Order::Reclaim(p) if p[0] >= 380) { break; }
    }
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == first));
    assert!(r.matter.debris.fragments.iter().any(|f| f.id == second));
    assert!(matches!(r.battle.actors[0].order, Order::Reclaim(p) if p[0] >= 380));
    assert!(!r.hauling(0), "must not unload while another nearby pile fits");
    assert!(r.tank_held(0).unwrap() > 0);
    for _ in 0..600 {
        r.step();
        if !r.matter.debris.fragments.iter().any(|f| f.id == second) { break; }
    }
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == second), "crawler must reach and reclaim the next pile");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_delivers_partial_load_instead_of_chasing_distant_debris() {
    use super::super::battle::Order;
    let mut l = modding::laboratory::experiments::stock().into_iter()
        .find(|e| e.id == "crawler-base").unwrap().layout;
    l.size = [800, 128, 320];
    l.clear_min = [1, 1]; l.clear_max = [799, 319];
    l.gun = [96, 24, 128]; l.target = [180, 16, 128];
    l.units[0].at = [120, 16, 128]; l.units[1].at = [48, 16, 220];
    l.blocks.clear();
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    let first = piece(&mut r, [180, 16, 128], 2);
    let distant = piece(&mut r, [650, 16, 128], 2);
    assert!(r.battle_order(0, Order::Reclaim([180, 16, 128]), None));
    for _ in 0..900 {
        r.step();
        if r.side_store(true).unwrap().0 > 0 { break; }
    }
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == first));
    assert!(r.matter.debris.fragments.iter().any(|f| f.id == distant));
    assert!(r.side_store(true).unwrap().0 > 0, "partial cargo was not delivered");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_prefers_a_piece_that_fits_before_returning_to_unload() {
    use super::super::battle::Order;
    let mut l = modding::laboratory::experiments::stock().into_iter()
        .find(|e| e.id == "crawler-base").unwrap().layout;
    l.size = [512, 128, 320]; l.clear_min = [1, 1]; l.clear_max = [511, 319];
    l.gun = [96, 24, 128]; l.target = [180, 16, 128];
    l.units[0].at = [120, 16, 128]; l.units[1].at = [48, 16, 220]; l.blocks.clear();
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_free_control().unwrap();
    let tank = r.mining().unwrap().tank(0).unwrap();
    let room = r.matter.economy.ledger.room(tank);
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, room - 300);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, room - 300), room - 300);
    let large = piece(&mut r, [180, 16, 128], 3);
    let small = piece(&mut r, [190, 16, 128], 2);
    assert!(r.battle_order(0, Order::Reclaim([180, 16, 128]), None));
    r.step();
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == small));
    assert!(r.matter.debris.fragments.iter().any(|f| f.id == large));
    assert!(!r.hauling(0));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_skips_oversized_piece_for_a_fitting_nearby_pile() {
    use super::super::battle::Order;
    let mut l = modding::laboratory::experiments::stock().into_iter().find(|e| e.id == "crawler-base").unwrap().layout;
    l.size = [640, 128, 320]; l.clear_min = [1, 1]; l.clear_max = [639, 319];
    l.gun = [96, 24, 128]; l.target = [180, 16, 128];
    l.units[0].at = [120, 16, 128]; l.units[1].at = [48, 16, 220]; l.blocks.clear();
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_free_control().unwrap();
    let tank = r.mining().unwrap().tank(0).unwrap();
    let room = r.matter.economy.ledger.room(tank);
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, room - 300);
    r.matter.economy.ledger.transfer(ground, tank, room - 300);
    let large = piece(&mut r, [180, 16, 128], 3);
    let small = piece(&mut r, [390, 16, 128], 2);
    assert!(r.battle_order(0, Order::Reclaim([180, 16, 128]), None));
    for _ in 0..100 {
        r.step();
        if matches!(r.battle.actors[0].order, Order::Reclaim(p) if p[0] >= 380) { break; }
    }
    assert!(matches!(r.battle.actors[0].order, Order::Reclaim(p) if p[0] >= 380));
    assert!(!r.hauling(0));
    assert!(r.matter.debris.fragments.iter().any(|f| f.id == large || f.id == small));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_reclaims_clicked_piece_then_nearby_and_emits_transfer_visuals() {
    use super::super::battle::Order;
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "crawler-base")
        .unwrap();
    let mut l = e.layout;
    l.size = [512, 128, 320];
    l.clear_min = [1, 1];
    l.clear_max = [511, 319];
    l.gun = [96, 24, 128];
    l.target = [180, 16, 128];
    l.units[0].at = [120, 16, 128];
    l.units[1].at = [48, 16, 220];
    l.blocks.clear();
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    let clicked = piece(&mut r, [180, 16, 128], 2);
    let nearby = piece(&mut r, [300, 16, 128], 3);
    let outside = piece(&mut r, [350, 16, 128], 3);
    assert!(r.battle_order(0, Order::Reclaim([180, 16, 128]), None));
    r.step();
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == clicked));
    assert!(r.matter.debris.fragments.iter().any(|f| f.id == nearby));
    let events = r.mining().unwrap().reclaim_visuals();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].crawler, 0);
    assert_eq!(events[0].material, 2);
    r.step();
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == nearby));
    assert!(r.matter.debris.fragments.iter().any(|f| f.id == outside));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_reclaim_mode_processes_destroyed_tank_and_never_digs_ground() {
    use super::super::battle::Order;
    let mut layout = modding::range::RangeLayout::from_ron(include_str!(
        "../../../data/range-crawler-field.ron"
    ))
    .unwrap();
    layout.initial_debris.clear();
    // Keep the harvesting head clear of the still-intact tank at setup.
    layout.units[0].at = [240, 16, 320];
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    let target = 2;
    let (x, y, z) = r.units[target].body.core;
    let n = r.units[target].body.core_size;
    let mut removed = 0;
    for dz in 0..n {
        for dy in 0..n {
            for dx in 0..n {
                let cell = [x + dx, y + dy, z + dz].map(usize::from);
                if r.units[target].body.vol.get(cell[0], cell[1], cell[2]) {
                    r.units[target].receive_cell(cell, u32::MAX, &cfg);
                    removed += 1;
                }
            }
        }
    }
    r.matter.annihilate_body(target, removed);
    r.units[target].refresh_damage(target);
    assert!(!r.units[target].status.alive);
    assert!(r.battle_order(0, Order::Reclaim([340, 16, 292]), None));
    let initial = r
        .matter
        .economy
        .ledger
        .held(r.matter.body_container(target));
    let chosen = piece(&mut r, [340, 16, 292], 2);
    r.step();
    assert!(
        !r.matter.debris.fragments.iter().any(|f| f.id == chosen),
        "clicked fragment must precede the nearby wreck"
    );
    assert_eq!(
        r.matter
            .economy
            .ledger
            .held(r.matter.body_container(target)),
        initial
    );
    let ground = r.world.material(340, 15, 292);
    let mut saw_hull_visual = false;
    for _ in 0..100 {
        r.step();
        saw_hull_visual |= r
            .mining()
            .unwrap()
            .reclaim_visuals()
            .iter()
            .any(|e| e.crawler == 0 && e.material == 3);
        assert!(r.matter.economy.ledger.balanced());
        // Stop at collection, before the now-faster edge pickup can finish
        // the subsequent automatic delivery to the nearby base as well.
        if r.tank_held(0)==Some(initial+240) {break;}
    }
    assert!(
        saw_hull_visual,
        "wreck transfer did not emit visual samples"
    );
    let remaining = r
        .matter
        .economy
        .ledger
        .held(r.matter.body_container(target));
    eprintln!("wreck matter before={initial} remaining={remaining}, cargo={:?} fragments={} order={:?} pos={:?} target={:?} status={:?} reclaim={:?}", r.tank_held(0), r.matter.debris.fragments.len(), r.battle.actors[0].order, r.units[0].motion.pos.map(Fx::to_int_trunc), r.units[target].motion.pos.map(Fx::to_int_trunc), r.units[0].status, r.mining().unwrap().reclaim().map(|v| (v.searches,v.reclaimed)));
    assert_eq!(
        remaining, 0,
        "tank hull was not reclaimed within five simulated seconds"
    );
    assert_eq!(
        r.tank_held(0).unwrap(),
        initial + 240,
        "crawler did not collect the entire wreck within five seconds"
    );
    assert_eq!(
        r.world.material(340, 15, 292),
        ground,
        "reclaim mode drilled terrain"
    );
    let copy = BlastRange::restore(r.config, &r.snapshot()).expect("reclaim snapshot");
    assert_eq!(r.state_hash(), copy.state_hash());
}

#[test]
fn enlarged_crawler_drill_reaches_rock_from_working_range() {
    use super::super::battle::Order;
    let mut layout = modding::range::RangeLayout::from_ron(include_str!(
        "../../../data/range-crawler-field.ron"
    ))
    .unwrap();
    layout.initial_debris.clear();
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    r.units[0].motion.pos = [247, 15, 289].map(Fx::from_int);
    r.units[0].motion.drive.head = -fx::angle::FULL / 4;
    r.units[0].update_pose(0);
    assert!(r.battle_order(0, Order::Dig([316, 27, 204]), None));
    for _ in 0..40 {
        r.dig_phase();
        r.world.flush();
        r.credit_digs();
        r.tick += 1;
        if r.mining().unwrap().cells > 0 {
            break;
        }
    }
    assert!(
        r.mining().unwrap().cells > 0,
        "drill failed at {:?}",
        r.units[0].world_pos(voxel::unit::CRAWLER_DRILL_TIP)
    );
}

#[test]
fn real_crawler_delivers_a_partial_load_when_salvage_face_is_empty() {
    use super::super::battle::Order;
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "crawler-base")
        .unwrap();
    let mut l = e.layout;
    l.size = [256, 128, 320];
    l.clear_min = [1, 1];
    l.clear_max = [255, 319];
    l.gun = [96, 24, 128];
    l.target = [145, 21, 128];
    l.units[0].at = [120, 16, 128];
    l.units[1].at = [48, 16, 220];
    l.blocks.clear();
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    let id = piece(&mut r, [145, 16, 128], 2);
    assert!(r.battle_order(0, Order::Reclaim([145, 21, 128]), None));
    for _ in 0..100 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
        if r.side_store(true).unwrap().0 > 0 {
            break;
        }
    }
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == id));
    assert!(
        r.side_store(true).unwrap().0 > 0,
        "small load was left in the crawler"
    );
    assert_eq!(r.tank_held(0), Some(0));
}

fn drill_range(material: u16) -> BlastRange {
    drill_range_with_rate(material, 64)
}

fn drill_range_with_rate(material: u16, rate: u32) -> BlastRange {
    use super::super::battle::Order;
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "crawler-base")
        .unwrap();
    let mut l = e.layout;
    l.size = [256, 128, 256];
    l.clear_min = [1, 1];
    l.clear_max = [255, 255];
    l.gun = [96, 24, 128];
    l.target = [190, 8, 128];
    l.units[0].at = [155, 16, 128];
    l.units[1].at = [48, 16, 128];
    l.blocks[0].min = [180, 1, 120];
    l.blocks[0].max = [202, 16, 136];
    l.blocks[0].material = material;
    // Capacity must not limit this material comparison.
    let crawler = l.economy
        .as_mut()
        .unwrap()
        .kind_mut(modding::range::RangeUnitKind::Crawler)
        .unwrap();
    crawler.tank = 900_000;
    crawler.dig_edits_per_tick = rate;
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    assert!(r.battle_order(0, Order::Dig([190, 8, 128]), None));
    r
}

#[test]
fn real_crawler_drill_requires_its_drive_and_cuts_hard_material_more_slowly() {
    let mut disabled = drill_range(6);
    disabled.units[0].status.weapon = false;
    disabled.dig_phase();
    assert_eq!(disabled.world.queued(), 0, "broken drill still queued cuts");
    disabled.units[0].status.weapon = true;
    disabled.units[0].status.engine = false;
    disabled.dig_phase();
    assert_eq!(disabled.world.queued(), 0, "drill ran without its engine");

    let mut rock = drill_range(6);
    let mut steel = drill_range(3);
    // Include the head deployment before measuring actual excavation.
    for _ in 0..40 {
        rock.step();
        steel.step();
        assert!(rock.matter.economy.ledger.balanced() && steel.matter.economy.ledger.balanced());
    }
    let rock_cells = rock.mining().unwrap().cells;
    let steel_cells = steel.mining().unwrap().cells;
    assert!(
        steel_cells > 0 && rock_cells > steel_cells,
        "rock={rock_cells}, steel={steel_cells}"
    );
    assert!(rock.tank_held(0).unwrap() > 0 && steel.tank_held(0).unwrap() > 0);
}

#[test]
fn crawler_drilling_rate_increases_without_creating_matter_or_exceeding_budget() {
    let mut old = drill_range_with_rate(6, 16);
    let mut tuned = drill_range(6);
    let pool = tuned.mining().unwrap().data().dig_edits_in_flight as usize;
    for _ in 0..40 {
        old.step();
        tuned.step();
        assert!(tuned.mining().unwrap().in_flight() <= pool);
        assert!(old.matter.economy.ledger.balanced() && tuned.matter.economy.ledger.balanced());
    }
    let a = old.mining().unwrap().cells;
    let b = tuned.mining().unwrap().cells;
    eprintln!("rock cells over 40 ticks: previous rate={a}, tuned rate={b}");
    assert!(b > a, "previous={a}, tuned={b}");
}

#[test]
fn stock_crawler_and_base_hold_five_tanks_after_refining() {
    let e = modding::range_economy::RangeEconomy::stock();
    let tank = super::super::RangeUnit::of_kind(modding::range::RangeUnitKind::Tank, [0; 3]);
    let cells = u64::from(tank.body.solid_count()) + tank.body.segments.iter()
        .map(|s| u64::from(s.body.solid_count())).sum::<u64>();
    let target = cells * 785 * 5;
    let bank = e.bank.unwrap();
    let raw = (target * u64::from(bank.refine_den)).div_ceil(u64::from(bank.refine_num));
    assert!(e.kind(modding::range::RangeUnitKind::Crawler).unwrap().tank >= raw);
    assert!(e.kind(modding::range::RangeUnitKind::Base).unwrap().store >= target);
}

#[test]
fn bounded_quarry_probe_reports_real_voxel_throughput() {
    use super::super::battle::Order;
    let mut l = modding::laboratory::experiments::stock().into_iter()
        .find(|e| e.id == "crawler-base").unwrap().layout;
    l.size = [384, 128, 384];
    l.clear_min = [1, 1]; l.clear_max = [383, 383];
    l.gun = [96, 24, 128]; l.target = [190, 8, 128];
    l.units[0].at = [155, 16, 128]; l.units[1].at = [48, 16, 128];
    l.blocks[0].min = [1, 1, 1]; l.blocks[0].max = [383, 16, 383];
    l.blocks[0].material = 6;
    let e = l.economy.as_mut().unwrap();
    e.dig_edits_in_flight = 1024;
    let c = e.kind_mut(modding::range::RangeUnitKind::Crawler).unwrap();
    c.tank = 160_000_000; c.dig_radius = 28; c.dig_edits_per_tick = 1024;
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_free_control().unwrap();
    assert!(r.battle_order(0, Order::Dig([190, 8, 128]), None));
    let start = std::time::Instant::now();
    for _ in 0..20 {
        r.step();
        assert!(r.mining().unwrap().in_flight() <= 1024);
        assert!(r.matter.economy.ledger.balanced());
    }
    eprintln!("quarry: cells={} matter={} elapsed_ms={}", r.mining().unwrap().cells,
        r.tank_held(0).unwrap(), start.elapsed().as_millis());
    assert!(r.mining().unwrap().cells >= 8_000, "the configured drill did not reach the measured rate");
    for _ in 20..600 {
        r.step();
        assert!(r.mining().unwrap().in_flight() <= 1024);
    }
    assert!(r.matter.economy.ledger.balanced());
    assert!(r.mining().unwrap().mined >= 6_000_000, "crawler stopped producing after its first patch");
    eprintln!("30 simulated seconds: mined={} cells={} tank={} depot={:?} elapsed_ms={}",
        r.mining().unwrap().mined, r.mining().unwrap().cells, r.tank_held(0).unwrap(),
        r.side_store(true), start.elapsed().as_millis());
    eprintln!("order={:?} pos={:?} tip={:?} queued={} inflight={} deferred={} hauling={}",
        r.battle.actors[0].order, r.units[0].motion.pos.map(Fx::to_int_trunc),
        r.units[0].world_pos(voxel::unit::CRAWLER_DRILL_TIP), r.world.queued(),
        r.mining().unwrap().in_flight(), r.mining().unwrap().deferred, r.hauling(0));
}

#[test]
fn crawler_bores_below_its_head_when_over_the_ordered_column() {
    use super::super::battle::Order;
    let mut r = drill_range(6);
    let tip = r.units[0].world_pos(voxel::unit::CRAWLER_BORE_TIP);
    let cell = [tip[0] as u32, 15, tip[2] as u32];
    r.world.queue_edit(voxel::Edit { x: cell[0], y: cell[1], z: cell[2], mat: 6, by: voxel::NOBODY });
    r.world.flush();
    assert_eq!(r.world.material(cell[0], cell[1], cell[2]), 6);
    assert!(r.battle_order(0, Order::Dig([tip[0], 8, tip[2]]), None));
    assert_eq!(r.crawler_tool_mode(0), Some(super::super::economy::CrawlerToolMode::Drill));
    for _ in 0..80 {
        r.dig_phase();
        r.world.flush();
        r.credit_digs();
        r.tick += 1;
        if r.world.material(cell[0], cell[1], cell[2]) == 0 { break; }
    }
    assert_eq!(r.world.material(cell[0], cell[1], cell[2]), 0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_bore_descends_with_the_real_open_shaft() {
    use super::super::battle::Order;
    let mut r = drill_range_with_rate(6, 1024);
    let tip = r.units[0].world_pos(voxel::unit::CRAWLER_BORE_TIP);
    assert!(r.battle_order(0, Order::Dig([tip[0], 8, tip[2]]), None));
    let start = r.crawler_bore_depth(0).unwrap();
    for _ in 0..100 {
        r.dig_phase();
        r.world.flush();
        r.credit_digs();
        r.tick += 1;
        if r.crawler_bore_depth(0).unwrap_or(start) >= start + 4 { break; }
    }
    let depth = r.crawler_bore_depth(0).unwrap();
    assert!(depth >= start + 4, "bore remained stationary: start {start}, now {depth}");
    let floor = r.world.column_ceiling(tip[0] as u32, tip[2] as u32) as i32;
    assert_eq!(depth, (tip[1] - floor.max(1)).max(0));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_reclaim_stops_when_its_shared_cutting_head_is_broken() {
    use super::super::battle::Order;
    let mut r = drill_range(6);
    let face = [190, 16, 128];
    let id = piece(&mut r, face, 2);
    assert!(r.battle_order(0, Order::Reclaim(face), None));
    assert_eq!(r.crawler_tool_mode(0), Some(super::super::economy::CrawlerToolMode::Reclaim));
    r.units[0].status.weapon = false;
    assert_eq!(r.crawler_tool_mode(0), None);
    r.dig_phase();
    assert!(r.matter.debris.fragments.iter().any(|f| f.id == id));
    assert_eq!(r.tank_held(0), Some(0));
    r.units[0].status.weapon = true;
    r.dig_phase();
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == id));
    assert!(r.tank_held(0).unwrap() > 0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_drill_follows_the_hull_even_if_turret_aim_changes() {
    let mut r = drill_range(6);
    let u = &mut r.units[0];
    u.update_pose(u.motion.drive.head + fx::angle::FULL / 4);
    let tip = voxel::unit::CRAWLER_DRILL_TIP;
    let local = tip.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2)));
    let expected = u.pose.world(local);
    assert_eq!(
        u.world_pos(tip),
        [expected.x, expected.y, expected.z].map(Fx::to_int_trunc)
    );
}

#[test]
fn crawler_field_collects_then_switches_to_drilling() {
    use super::super::battle::Order;
    let layout = modding::range::RangeLayout::from_ron(include_str!(
        "../../../data/range-crawler-field.ron"
    ))
    .unwrap();
    assert!(layout.units.iter().all(|u| u.allied));
    assert_eq!(layout.units.len(), 4);
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    assert!(r.matter.debris.fragments.len() >= 20);
    assert!(r.matter.debris.fragments.iter().all(|f| f.resting));
    assert!(r.matter.economy.ledger.balanced());
    let f = r
        .matter
        .debris
        .fragments
        .iter()
        .find(|f| {
            let p = [f.position.x, f.position.y, f.position.z].map(Fx::to_int_trunc);
            (p[0] - 314).abs() <= 4 && (p[2] - 282).abs() <= 4
        })
        .expect("starting collection target");
    let id = f.id;
    assert!(r.battle_order(0, Order::Reclaim([314, 17, 282]), None));
    let mut collected = false;
    let mut loaded = false;
    let mut charge = 0;
    let charges = [[368, 21, 292], [464, 21, 376]];
    for _ in 0..900 {
        if charge < charges.len() && r.scenario_charge(charges[charge], 20000) {
            charge += 1;
        }
        r.step();
        assert!(r.matter.economy.ledger.balanced());
        collected |= !r.matter.debris.fragments.iter().any(|f| f.id == id);
        loaded |= r.tank_held(0).unwrap_or(0) > 0;
        if collected && loaded && charge == charges.len() {
            break;
        }
    }
    assert!(collected, "the selected fragment was not collected");
    assert!(loaded, "no cargo reached the crawler tank");
    // This field deliberately contains hundreds of pieces. A partially
    // filled crawler must keep collecting them rather than unload early;
    // delivery after the local field is exhausted is tested separately.
    assert!(r.battle.actors.iter().all(|a| a.fired == 0));
    let rock_face = [316, 27, 204];
    assert!(r.battle_order(0, Order::Dig(rock_face), None));
    let mut drilled = false;
    for tick in 0..500 {
        r.step();
        if tick % 10 == 0 {
            drilled = (300..332).any(|x| (190..218).any(|z| r.world.material(x, 27, z) != 6));
            if drilled {
                break;
            }
        }
    }
    assert!(drilled, "crawler did not reach or drill the scenario rock face: position {:?}, cargo {:?}, order {:?}, destination {:?}, status {:?}, mined {}, hauling {}, tip {:?}", r.units[0].at, r.tank_held(0), r.battle.actors[0].order, r.units[0].motion.destination, r.units[0].status, r.mining().unwrap().cells, r.hauling(0), r.units[0].world_pos(voxel::unit::CRAWLER_DRILL_TIP));
    assert!(r.matter.economy.ledger.balanced());
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("scenario snapshot");
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn crawler_field_charges_damage_real_tanks_without_harming_crawler() {
    let layout = modding::range::RangeLayout::from_ron(include_str!(
        "../../../data/range-crawler-field.ron"
    ))
    .unwrap();
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_layout(cfg, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    for (id, at) in [(2, [368, 21, 292]), (3, [464, 21, 376])] {
        let mut started = false;
        for tick in 0..1200 {
            if !started {
                started = r.scenario_charge(at, 20000);
            }
            r.step();
            if started
                && tick >= 200
                && r.explosion
                    .as_ref()
                    .is_some_and(|e| e.wave.finished() && e.pending.is_empty())
            {
                break;
            }
        }
        assert!(started, "charge {id} did not start");
        assert!(
            !r.units[id].status.alive && !r.units[id].removed.is_empty(),
            "tank {id} was not destroyed"
        );
    }
    assert!(r.units[0].status.alive && r.units[1].status.alive);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_deploys_before_cutting_and_keeps_deadline_after_restore() {
    let mut r = drill_range(6);
    let face = [190, 8, 128];
    r.dig_phase();
    assert!(!r.crawler_bore_cutting(0));
    assert_eq!(r.world.queued(), 0, "ground was removed before head deployment");
    let ready = r.battle.actors[0].pursuit;
    assert!(ready >= 14);
    let mut saved = BlastRange::restore(r.config.clone(), &r.snapshot()).expect("deployment restores");
    saved.set_physics_workers(1); saved.set_navigation_workers(1);
    assert_eq!(saved.battle.actors[0].pursuit, ready);
    while r.tick < ready {
        assert!(!r.crawler_bore_cutting(0));
        r.dig_phase(); saved.dig_phase();
        assert_eq!(r.world.queued(), 0, "cut before head reached its column");
        r.tick += 1; saved.tick += 1;
    }
    assert!(r.crawler_bore_cutting(0));
    r.dig_phase(); saved.dig_phase();
    assert!(r.world.queued() > 0, "deployed head did not start cutting at {face:?}");
    assert_eq!(r.world.queued(), saved.world.queued());
    r.units[0].status.weapon = false;
    r.dig_phase();
    assert!(!r.crawler_bore_cutting(0));
    assert_eq!(r.battle.actors[0].pursuit, 0, "broken head must require redeployment");
}

#[test]
fn crawler_head_waits_until_its_entire_footprint_is_clear() {
    use super::super::battle::Order;
    let mut r = drill_range_with_rate(6, 1024);
    let tip = r.units[0].world_pos(voxel::unit::CRAWLER_BORE_TIP);
    let face = [tip[0] - 8, 8, tip[2]];
    assert!(r.battle_order(0, Order::Dig(face), None));
    let radius = voxel::unit::CRAWLER_BORE_RADIUS;
    let mut began = false;
    let start_depth = r.crawler_bore_depth(0).unwrap();
    let mut deepest = start_depth;
    for _ in 0..180 {
        r.dig_phase();
        r.world.flush();
        r.credit_digs();
        r.tick += 1;
        let p = r.crawler_bore_position(0).unwrap();
        assert_eq!([p[0], p[2]], [face[0], face[2]]);
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dz * dz > radius * radius { continue; }
                let x = face[0] + dx; let z = face[2] + dz;
                if x < 0 || z < 0 || x >= r.world.nx as i32 || z >= r.world.nz as i32 { continue; }
                let top = r.world.column_ceiling(x as u32, z as u32) as i32;
                assert!(top <= p[1].max(tip[1]), "head penetrates terrain at ({x},{z}): ceiling {top}, head {p:?}");
                if p[1] < tip[1] { assert!(top <= p[1], "head descended through solid ground at ({x},{z}): {top} > {}", p[1]); }
            }
        }
        began |= r.mining().unwrap().cells > 0;
        deepest = deepest.max(r.crawler_bore_depth(0).unwrap());
        if deepest >= start_depth + 2 { break; }
    }
    assert!(began && deepest >= start_depth + 2, "drill must keep working and descend: start {start_depth}, depth {deepest}");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_reclaims_large_wreck_from_reachable_near_edge() {
    use super::super::battle::Order;
    use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-crawler-field.ron")).unwrap();
    layout.initial_debris.clear();
    layout.blocks.clear();
    layout.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: true, at: [190, 16, 320], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: false, at: [390, 16, 320], weapon: None },
    ];
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, layout).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    r.units[1].status.alive = false;
    let face = r.units[1].motion.pos.map(Fx::to_int_trunc);
    assert!((face[0] - r.units[0].motion.pos[0].to_int_trunc()) > 110);
    let before = r.matter.economy.ledger.held(r.matter.body_container(1));
    assert!(r.battle_order(0, Order::Reclaim(face), None));
    assert!(r.units[0].motion.destination.is_none(), "already at the edge; no path into the wreck is necessary");
    assert_eq!(r.crawler_tool_mode(0), Some(super::super::economy::CrawlerToolMode::Reclaim));
    r.dig_phase();
    let after = r.matter.economy.ledger.held(r.matter.body_container(1));
    assert!(after < before, "the reachable wreck edge was not reclaimed");
    assert!(r.units[1].removed.iter().all(|cell| r.units[1].world_pos(cell.map(usize::from))[0] < face[0]),
        "must start from the side facing the collector: {:?}", r.units[1].removed.first());
    assert!(r.tank_held(0).unwrap() > 0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_can_approach_large_wreck_without_targeting_its_centre() {
    use super::super::battle::Order;
    use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-crawler-field.ron")).unwrap();
    layout.initial_debris.clear(); layout.blocks.clear();
    layout.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: true, at: [120, 16, 320], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: false, at: [390, 16, 320], weapon: None },
    ];
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, layout).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_free_control().unwrap();
    r.units[1].status.alive = false;
    let face = r.units[1].motion.pos.map(Fx::to_int_trunc);
    let before = r.matter.economy.ledger.held(r.matter.body_container(1));
    assert!(r.battle_order(0, Order::Reclaim(face), None), "edge approach was rejected");
    for _ in 0..600 {
        r.step();
        if r.matter.economy.ledger.held(r.matter.body_container(1)) < before { break; }
    }
    assert!(r.matter.economy.ledger.held(r.matter.body_container(1)) < before,
        "crawler could not reach the edge: pos {:?}, goal {:?}, blocked {}", r.units[0].motion.pos, r.units[0].motion.destination, r.units[0].motion.blocked);
    assert!(r.matter.economy.ledger.balanced());
}

fn destroyed_crawler_reclaim_scene() -> BlastRange {
    use modding::range::{RangeLayout,RangeUnitDef,RangeUnitKind};
    let mut layout=RangeLayout::from_ron(include_str!("../../../data/range-crawler-field.ron")).unwrap();
    layout.size[0]=1024;layout.clear_max[0]=1023;
    layout.initial_debris.clear();layout.blocks.clear();
    let mut economy = modding::range_economy::RangeEconomy::stock();
    // This fixture exercises real destruction, debris and reclaim, without
    // factories. Do not build every T1-T4 product's world-cell exit mask.
    economy.production.as_mut().unwrap().producers.clear();
    layout.economy=Some(economy);
    layout.units=vec![
        RangeUnitDef{kind:RangeUnitKind::Crawler,allied:true,at:[490,16,320],weapon:None},
        RangeUnitDef{kind:RangeUnitKind::Crawler,allied:false,at:[320,16,320],weapon:None},
    ];
    let cfg=modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r=BlastRange::with_layout(cfg,layout).unwrap();
    r.set_physics_workers(1);r.set_navigation_workers(1);r.start_free_control().unwrap();
    let core=r.units[1].body.core;let size=r.units[1].body.core_size;
    let mut removed=0;
    for z in core.2..core.2+size {for y in core.1..core.1+size {for x in core.0..core.0+size {
        let cell=[x,y,z].map(usize::from);
        if r.units[1].body.vol.get(cell[0],cell[1],cell[2]) {
            r.units[1].receive_cell(cell,u32::MAX,&cfg);removed+=1;
        }
    }}}
    r.matter.annihilate_body(1,removed);r.units[1].refresh_damage(1);
    assert!(!r.units[1].status.alive);
    r
}

#[test]
fn crawler_reclaims_destroyed_crawler_at_side_edge() {
    use super::super::battle::Order;
    let mut r=destroyed_crawler_reclaim_scene();
    let face=r.units[1].at;
    assert!(r.battle_order(0,Order::Reclaim(face),None));
    let mut saw_work=false;
    for tick in 0..240 {
        r.step();
        saw_work|=r.mining().unwrap().reclaim_visuals().iter().any(|v|v.crawler==0);
        if tick==40 {
            let back=BlastRange::restore(r.config,&r.snapshot()).expect("ongoing large-wreck reclaim restores");
            assert_eq!(back.state_hash(),r.state_hash());
        }
        if r.tank_held(0).unwrap()>0 {break;}
        if tick==239 {eprintln!("stalled wreck: pos={:?}, goal={:?}, order={:?}, tool={:?}, fragments={}, edits={}",r.units[0].at,r.units[0].motion.destination,r.battle.actors[0].order,r.crawler_tool_mode(0),r.matter.debris.fragments.len(),r.mining().unwrap().reclaim().unwrap().edits);}
    }
    assert!(saw_work,"large-piece reclaim must display work before the final transfer");
    assert!(r.tank_held(0).unwrap()>0,"a destroyed crawler must be reclaimable from its accessible edge");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_reclaims_destroyed_crawler_chunk_by_driving_to_its_edge() {
    use super::super::battle::Order;
    let mut r=destroyed_crawler_reclaim_scene();
    for _ in 0..80 {
        r.step();
        if r.matter.debris.fragments.iter().any(|f|f.resting && f.count>1000) {break;}
    }
    let f=r.matter.debris.fragments.iter().find(|f|f.resting && f.count>1000)
        .expect("destroyed crawler must leave a real resting chunk");
    let held=f.held;
    let amount=r.matter.economy.ledger.held(held);
    let face=[f.position.x,f.position.y,f.position.z].map(Fx::to_int_trunc);
    r.units[0].motion.pos=[590,16,320].map(Fx::from_int);r.units[0].update_pose(0);
    let (lo,hi)=r.units[0].bounds();
    assert!(lo[0]>=0 && hi[0]<(r.size[0] as i32),"approach fixture must start fully inside the map: {lo:?}..{hi:?}");
    assert!(r.battle_order(0,Order::Reclaim(face),None));
    assert!(r.units[0].motion.destination.is_some(),"a remote selected chunk requires an approach");
    let mut work_tick=None;
    for tick in 0..300 {
        r.step();
        if work_tick.is_none() && !r.mining().unwrap().reclaim_visuals().is_empty() {work_tick=Some(tick);}
        if r.tank_held(0).unwrap()>=amount {break;}
    }
    assert!(work_tick.is_some(),"approach never reached the visible piece: pos {:?}, goal {:?}, blocked {}",r.units[0].at,r.units[0].motion.destination,r.units[0].motion.blocked);
    assert!(r.tank_held(0).unwrap()>=amount,"selected chunk's matter did not reach the hold");
    assert!(!r.matter.debris.fragments.iter().any(|f|f.held==held));
    assert!(r.matter.economy.ledger.balanced());
    eprintln!("selected wreck: work began tick {work_tick:?}, collected {amount} IU at {:?}",r.units[0].at);
}
