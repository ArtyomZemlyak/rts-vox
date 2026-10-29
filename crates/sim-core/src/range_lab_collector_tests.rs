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
    let e = modding::laboratory::experiments::stock().into_iter()
        .find(|e| e.id == "crawler-base").unwrap();
    let mut l = e.layout;
    l.size = [256, 128, 256];
    l.clear_min = [1, 1]; l.clear_max = [255, 255];
    l.gun = [96, 24, 128]; l.target = [190, 18, 128];
    l.units[0].at = [120, 16, 128]; l.units[1].at = [48, 16, 128];
    l.blocks[0].min = [180, 1, 120]; l.blocks[0].max = [202, 16, 136];
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    assert_eq!(r.units[0].kind, RangeUnitKind::Crawler);
    assert_eq!(r.units[1].kind, RangeUnitKind::Base);
    assert!(r.battle_order(0, Order::Dig([190, 8, 128]), None), "unit status: {:?}; digs: {}; battle: {}; phase: {:?}", r.units[0].status, r.mining().is_some_and(|m|m.digs(RangeUnitKind::Crawler)), r.battle_mode(), r.battle.phase);
    let mut moved = false;
    let mut copied = None;
    for tick in 0..1500 {
        r.step();
        moved |= r.units[0].motion.pos[0].to_int_trunc() > 125;
        if tick == 20 {
            let mut c = BlastRange::restore(r.config, &r.snapshot()).unwrap();
            c.set_physics_workers(1); c.set_navigation_workers(1);
            copied = Some(c);
        } else if tick < 25 {
            if let Some(c) = copied.as_mut() { c.step(); assert_eq!(r.state_hash(), c.state_hash()); }
        }
        assert!(r.matter.economy.ledger.balanced());
        if r.side_store(true).unwrap().0 > 0 { break; }
    }
    assert!(moved, "crawler never approached the face");
    assert!(r.side_store(true).unwrap().0 > 0, "crawler failed to deliver to the base: pos {:?}, target {:?}, enabled {}, blocked {}, status {:?}, tank {:?}, hauling {}, phase {:?}", r.units[0].motion.pos.map(Fx::to_int_trunc),r.units[0].motion.destination,r.units[0].motion.enabled,r.units[0].motion.blocked,r.units[0].status,r.tank_held(0),r.hauling(0),r.battle.phase);
}

#[test]
fn real_crawler_reclaims_actual_debris_into_its_own_tank() {
    use super::super::battle::Order;
    let e = modding::laboratory::experiments::stock().into_iter().find(|e| e.id == "crawler-base").unwrap();
    let mut l = e.layout;
    l.size = [256, 128, 256]; l.clear_min = [1, 1]; l.clear_max = [255, 255];
    l.gun = [96, 24, 128]; l.target = [150, 16, 128];
    l.units[0].at = [120, 16, 128]; l.units[1].at = [48, 16, 128]; l.blocks.clear();
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_free_control().unwrap();
    let id = piece(&mut r, [145, 16, 128], 2);
    assert!(r.battle_order(0, Order::Dig([145, 16, 128]), None));
    for _ in 0..80 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
        if !r.matter.debris.fragments.iter().any(|f| f.id == id) { break; }
    }
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == id), "real unit did not reclaim the fragment");
    assert!(r.tank_held(0).unwrap() >= 240, "reclaimed material did not enter the unit tank");
}

#[test]
fn real_crawler_delivers_a_partial_load_when_salvage_face_is_empty() {
    use super::super::battle::Order;
    let e = modding::laboratory::experiments::stock().into_iter().find(|e| e.id == "crawler-base").unwrap();
    let mut l = e.layout;
    l.size = [256, 128, 256]; l.clear_min = [1, 1]; l.clear_max = [255, 255];
    l.gun = [96, 24, 128]; l.target = [145, 21, 128];
    l.units[0].at = [120, 16, 128]; l.units[1].at = [48, 16, 128]; l.blocks.clear();
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_free_control().unwrap();
    let id = piece(&mut r, [145, 16, 128], 2);
    assert!(r.battle_order(0, Order::Dig([145, 21, 128]), None));
    for _ in 0..100 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
        if r.side_store(true).unwrap().0 > 0 { break; }
    }
    assert!(!r.matter.debris.fragments.iter().any(|f| f.id == id));
    assert!(r.side_store(true).unwrap().0 > 0, "small load was left in the crawler");
    assert_eq!(r.tank_held(0), Some(0));
}

fn drill_range(material: u16) -> BlastRange {
    use super::super::battle::Order;
    let e = modding::laboratory::experiments::stock().into_iter().find(|e| e.id == "crawler-base").unwrap();
    let mut l = e.layout;
    l.size = [256, 128, 256]; l.clear_min = [1, 1]; l.clear_max = [255, 255];
    l.gun = [96, 24, 128]; l.target = [190, 8, 128];
    l.units[0].at = [155, 16, 128]; l.units[1].at = [48, 16, 128];
    l.blocks[0].min = [180, 1, 120]; l.blocks[0].max = [202, 16, 136]; l.blocks[0].material = material;
    // Capacity must not limit this material comparison.
    l.economy.as_mut().unwrap().kind_mut(modding::range::RangeUnitKind::Crawler).unwrap().tank = 900_000;
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_free_control().unwrap();
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
    for _ in 0..12 {
        rock.step(); steel.step();
        assert!(rock.matter.economy.ledger.balanced() && steel.matter.economy.ledger.balanced());
    }
    let rock_cells = rock.mining().unwrap().cells;
    let steel_cells = steel.mining().unwrap().cells;
    assert!(steel_cells > 0 && rock_cells > steel_cells, "rock={rock_cells}, steel={steel_cells}");
    assert!(rock.tank_held(0).unwrap() > 0 && steel.tank_held(0).unwrap() > 0);
}

#[test]
fn crawler_drill_follows_the_hull_even_if_turret_aim_changes() {
    let mut r = drill_range(6);
    let u = &mut r.units[0];
    u.update_pose(u.motion.drive.head + fx::angle::FULL / 4);
    let tip = voxel::unit::CRAWLER_DRILL_TIP;
    let local = tip.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2)));
    let expected = u.pose.world(local);
    assert_eq!(u.world_pos(tip), [expected.x, expected.y, expected.z].map(Fx::to_int_trunc));
}

#[test]
fn crawler_field_starts_with_reclaimable_debris_and_delivers_to_base() {
    use super::super::battle::Order;
    let layout = modding::range::RangeLayout::from_ron(include_str!("../../../data/range-crawler-field.ron")).unwrap();
    assert!(layout.units.iter().all(|u| u.allied));
    assert_eq!(layout.units.len(), 2);
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, layout).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_free_control().unwrap();
    assert!(r.matter.debris.fragments.len() >= 20);
    assert!(r.matter.debris.fragments.iter().all(|f| f.resting));
    assert!(r.matter.economy.ledger.balanced());
    let f = &r.matter.debris.fragments[0];
    let id = f.id;
    let point = [f.position.x, f.position.y, f.position.z].map(Fx::to_int_trunc);
    assert!(r.battle_order(0, Order::Dig(point), None));
    let mut collected = false;
    let mut loaded = false;
    for _ in 0..900 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
        collected |= !r.matter.debris.fragments.iter().any(|f| f.id == id);
        loaded |= r.tank_held(0).unwrap_or(0) > 0;
        if collected && loaded && r.side_store(true).unwrap().0 > 0 { break; }
    }
    assert!(collected, "the selected fragment was not collected");
    assert!(loaded, "no cargo reached the crawler tank");
    assert!(r.side_store(true).unwrap().0 > 0, "cargo did not reach the base");
    assert!(r.battle.actors.iter().all(|a| a.fired == 0));
    let rock_face = [416, 27, 248];
    assert!(r.battle_order(0, Order::Dig(rock_face), None));
    let mut drilled = false;
    for tick in 0..1200 {
        r.step();
        if tick % 10 == 0 {
            drilled = (400..432).any(|x| (232..260).any(|z| r.world.material(x, 27, z) != 6));
            if drilled { break; }
        }
    }
    assert!(drilled, "crawler did not reach or drill the scenario rock face");
    assert!(r.matter.economy.ledger.balanced());
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("scenario snapshot");
    assert_eq!(r.state_hash(), restored.state_hash());
}
