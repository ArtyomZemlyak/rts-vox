use super::*;
const BATCH: Amount = 4_082_000;
const COMMANDER_GENERATION: u64 = 600;
const BASE_GENERATION: u64 = 2000;
const POWER_PLANT_T1_GENERATION: u64 = 1000;
fn legacy_economy() -> modding::range_economy::RangeEconomy {
    let mut e=modding::range_economy::RangeEconomy::stock();
    e.balance=None; e.dig_periods.clear();
    for k in &mut e.kinds { k.body_density=0; }
    e
}

use crate::blast::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout};

/// Keep authored unit distances, wrecks and orders, but discard the remote
/// empty part of the kilometre range during a headless scenario check.
fn compact_industrial_scenario(id: usize) -> (BlastRange, modding::range_skirmish::Skirmish, [i32; 2]) {
    let preset = modding::range_skirmish::stock(id).unwrap();
    let shift = if id == 24 || id == 25 { [4700, 6800] } else { [700, 1400] };
    let mut layout = modding::laboratory::Definition::stock().layout;
    layout.size = [3600, 512, 2400];
    layout.clear_min = [1, 1]; layout.clear_max = [3599, 2399];
    layout.gun_point = true; layout.gun = [100, 24, 100]; layout.target = [300, 24, 100];
    layout.battle = true; layout.endless_battle = true; layout.idle_enemies = preset.idle_enemies;
    layout.units = preset.units.clone();
    for u in &mut layout.units { u.at[0] -= shift[0]; u.at[2] -= shift[1]; }
    if id == 23 {
        // The idle enemy is far outside the work area. Move only that
        // passive target so the crawler cycle fits a smaller test map.
        layout.size = [2208, 512, 1808]; layout.clear_max = [2207, 1807];
        layout.units.iter_mut().find(|u| !u.allied).unwrap().at = [1900, 16, 1400];
    }
    layout.blocks = vec![
        RangeBlock { min: [0, 0, 0], max: [layout.size[0] as i32, 512, layout.size[2] as i32], material: 0 },
        RangeBlock { min: [0, 0, 0], max: [layout.size[0] as i32, 16, layout.size[2] as i32], material: 1 },
    ];
    for b in &preset.blocks {
        // The base ground above already reproduces the demo's large flat pad.
        if b.material == 0 || b.min[1] < 16 { continue; }
        let mut b = b.clone();
        b.min[0] -= shift[0]; b.max[0] -= shift[0];
        b.min[2] -= shift[1]; b.max[2] -= shift[1];
        layout.blocks.push(b);
    }
    layout.initial_debris = preset.initial_debris.clone();
    for b in &mut layout.initial_debris {
        b.min[0] -= shift[0]; b.max[0] -= shift[0];
        b.min[2] -= shift[1]; b.max[2] -= shift[1];
    }
    layout.economy = Some(legacy_economy());
    layout.ready = preset.ready.clone();
    if let Some(ready) = &mut layout.ready {
        if let Some(bank) = layout.economy.as_mut().and_then(|e| e.bank.as_mut()) {
            bank.base_store = bank.base_store.max(ready.store);
        }
        for site in &mut ready.construction { site.at[0] -= shift[0]; site.at[2] -= shift[1]; }
    }
    let mut r = BlastRange::with_layout(ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(), layout).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_battle().unwrap();
    (r, preset, shift)
}

#[test]
fn authored_kil_and_greben_demos_afford_connection_and_a_tank() {
    for id in [24, 26] {
        let (mut r, _, _) = compact_industrial_scenario(id);
        let factory = if id == 24 { 0 } else { 1 };
        let initial = r.units.len();
        assert_eq!(r.production_queue(factory), Some(vec![RangeUnitKind::Tank]));
        for _ in 0..400 {
            r.tick += 1;
            r.production_phase();
            if r.units.len() > initial { break; }
        }
        assert!(r.factory_supply_connected(factory), "scenario {id}: no warehouse connection");
        assert!(r.units.len() > initial, "scenario {id}: tank not produced; paid={:?}, stock={:?}", r.production_paid(factory), r.side_store(true));
        assert_eq!(r.units[initial].kind, RangeUnitKind::Tank);
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn greben_scenario_all_selected_drones_share_a_turret_and_have_starting_mass() {
    let (mut r, _, _) = compact_industrial_scenario(26);
    assert!(r.side_store(true).unwrap().0 >= 300_000_000);
    for _ in 0..400 {
        r.tick += 1;
        r.production_phase();
        if r.production_queue(1).is_some_and(|q| q.is_empty()) { break; }
    }
    let cost = r.construction_cost(RangeUnitKind::TurretT1).unwrap();
    assert!(r.side_store(true).unwrap().0 >= 10 * cost, "demo stock cannot support repeated construction after the queued tank");
    let drones: Vec<_> = r.units.iter().enumerate().filter(|(id, u)|
        r.allied(*id) && u.kind == RangeUnitKind::SkobaDrone && u.status.alive).map(|(id, _)| id).collect();
    assert_eq!(drones.len(), 8);
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [1550, 16, 500]).unwrap();
    assert!(r.set_construction_quota(site, 0));
    for &id in &drones { assert!(r.battle_order(id, battle::Order::Build(site), None)); }
    r.construction_phase();
    let construction = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    for &id in &drones {
        let d = &construction.drones[&(id as u16)];
        assert_eq!(d.site, Some(site as u16), "selected drone {id} was not assigned");
        assert!(d.want > 0);
    }
    let mut picked = vec![false; drones.len()];
    for _ in 0..900 {
        r.tick += 1;
        r.construction_phase();
        for (n, &id) in drones.iter().enumerate() { picked[n] |= r.drone_cargo(id).is_some(); }
        if r.construction_sites()[site].complete && picked.iter().all(|v| *v) { break; }
    }
    assert!(picked.iter().all(|v| *v), "not every selected drone picked up matter: {picked:?}");
    assert!(r.construction_sites()[site].complete, "turret was not completed");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn authored_skoba_demo_completes_both_turrets_with_starting_stock() {
    let (mut r, _, _) = compact_industrial_scenario(25);
    assert_eq!(r.construction_sites().len(), 2);
    for _ in 0..1400 {
        r.tick += 1;
        r.construction_phase();
        if r.construction_sites().iter().all(|s| s.complete) { break; }
    }
    let sites = r.construction_sites();
    assert!(sites.iter().all(|s| s.complete), "sites={sites:?}; stock={:?}", r.side_store(true));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn authored_crawler_demo_collects_then_unloads_then_bores_automatically() {
    use modding::range_skirmish::ScriptedOrder;
    let (mut r, preset, shift) = compact_industrial_scenario(23);
    let mut script = preset.script;
    let mut started_drill = false;
    let mut collected = false;
    for _ in 0..1800 {
        while script.first().is_some_and(|next| next.tick <= r.tick && (!next.after_unload || (r.hauled_mass() > 0 && !r.hauling(next.actor)))) {
            let next = script.remove(0);
            let order = match next.order {
                ScriptedOrder::Reclaim(mut p) => { p[0] -= shift[0]; p[2] -= shift[1]; battle::Order::Reclaim(p) }
                ScriptedOrder::Dig(mut p) => { p[0] -= shift[0]; p[2] -= shift[1]; started_drill = true; battle::Order::Dig(p) }
                ScriptedOrder::Attack(target) => battle::Order::Attack(target),
            };
            assert!(r.battle_order(next.actor, order, None));
        }
        r.step();
        collected |= r.tank_held(0).unwrap_or(0) > 0;
        if started_drill && r.mining().unwrap().cells > 0 { break; }
    }
    assert!(collected, "crawler did not collect debris");
    assert!(r.hauled_mass() > 0, "crawler did not unload; at={:?}, cargo={:?}, order={:?}", r.units[0].at, r.tank_held(0), r.battle.actors[0].order);
    assert!(started_drill && r.mining().unwrap().cells > 0, "drilling did not start after delivery");
    assert!(r.matter.economy.ledger.balanced());
}

fn check_automated_industrial_demo(id: usize) {
    use modding::range_skirmish::ScriptedOrder;
    let (mut r, preset, shift) = compact_industrial_scenario(id);
    if preset.mission.is_some() {
        for actor in 0..r.units.len() {
            if r.allied(actor) && r.units[actor].status.alive && r.units[actor].kind != RangeUnitKind::SkobaDrone {
                r.battle_order(actor, battle::Order::Hold, None);
            }
        }
    }
    let initial_units = r.units.len();
    let crawler = if id == 25 { 1 } else { 2 };
    let mut script = preset.script;
    let mut gathered = false;
    let mut drilling = false;
    for _ in 0..1800 {
        while script.first().is_some_and(|next| next.tick <= r.tick && (!next.after_unload || (r.hauled_mass() > 0 && !r.hauling(next.actor)))) {
            let next = script.remove(0);
            let order = match next.order {
                ScriptedOrder::Reclaim(mut p) => { p[0] -= shift[0]; p[2] -= shift[1]; battle::Order::Reclaim(p) }
                ScriptedOrder::Dig(mut p) => { p[0] -= shift[0]; p[2] -= shift[1]; drilling = true; battle::Order::Dig(p) }
                ScriptedOrder::Attack(target) => battle::Order::Attack(target),
            };
            assert!(r.battle_order(next.actor, order, None), "scenario {id}: script order rejected");
        }
        r.step();
        gathered |= r.tank_held(crawler).unwrap_or(0) > 0;
        if r.tick % 300 == 0 { eprintln!("demo {id} tick {} at {:?} order {:?} dest {:?} cargo {:?} hauling {} unloaded {}", r.tick, r.units[crawler].at, r.battle.actors[crawler].order, r.units[crawler].motion.destination, r.tank_held(crawler), r.hauling(crawler), r.hauled_mass()); }
        let industrial_done = if id == 25 {
            r.construction_sites().len() == 2 && r.construction_sites().iter().all(|site| site.complete)
        } else {
            r.units.iter().enumerate().skip(initial_units).any(|(unit, u)| u.kind == RangeUnitKind::Tank && r.allied(unit))
        };
        if industrial_done && gathered && r.hauled_mass() > 0 && drilling && r.mining().unwrap().cells > 0 { break; }
    }
    assert!(gathered && r.hauled_mass() > 0, "scenario {id}: crawler did not collect and unload; at={:?}, cargo={:?}, order={:?}, dest={:?}, blocked={}, hauling={}, collected={}, loaded={}, tick={}", r.units[crawler].at, r.tank_held(crawler), r.battle.actors[crawler].order, r.units[crawler].motion.destination, r.units[crawler].motion.blocked, r.hauling(crawler), gathered, r.hauled_mass(), r.tick);
    assert!(drilling && r.mining().unwrap().cells > 0, "scenario {id}: no drilling after unloading");
    if id == 25 {
        assert!(r.construction_sites().iter().all(|site| site.complete), "scenario {id}: unfinished sites {:?}", r.construction_sites());
    } else {
        assert!(r.units.iter().skip(initial_units).any(|u| u.kind == RangeUnitKind::Tank), "scenario {id}: no produced tank");
    }
    assert_eq!(r.battle.phase, battle::Phase::Fighting);
    assert!(r.matter.economy.ledger.balanced());
}

#[test] fn kil_demo_runs_automatically() { check_automated_industrial_demo(24); }
#[test] fn skoba_demo_runs_automatically() { check_automated_industrial_demo(25); }
#[test] fn greben_demo_runs_automatically() { check_automated_industrial_demo(26); }

fn compact_greben_range() -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.idle_enemies = true;
    layout.size = [1600, 512, 1600];
    layout.blocks = vec![
        RangeBlock { min: [0, 0, 0], max: [1600, 512, 1600], material: 0 },
        RangeBlock { min: [0, 0, 0], max: [1600, 16, 1600], material: 1 },
    ];
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [1536, 1536];
    layout.units = vec![
        RangeUnitDef { allied: true, kind: RangeUnitKind::GrebenStorage, at: [500, 16, 800], weapon: None },
        RangeUnitDef { allied: false, kind: RangeUnitKind::Tank, at: [1400, 16, 1400], weapon: None },
    ];
    layout.economy = Some(legacy_economy());
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(), layout,
    ).unwrap();
    r.start_battle().unwrap();
    r
}

#[test]
fn stop_keeps_loaded_drone_idle_and_preserves_material_until_reordered() {
    let mut r = compact_greben_range();
    let drone = r.units.iter().enumerate().find(|(id, u)| u.kind == RangeUnitKind::SkobaDrone && r.allied(*id)).unwrap().0;
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [800, 16, 800]).unwrap();
    r.set_construction_quota(site, 1);
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, BATCH * 3);
    r.matter.economy.ledger.transfer(ground, store, BATCH * 3);
    for _ in 0..200 {
        r.construction_phase();
        if r.drone_cargo(drone).is_some() { break; }
    }
    let cargo = r.drone_cargo(drone).expect("automatically assigned drone loaded");
    let held = r.matter.economy.ledger.held(cargo.container);
    assert!(r.battle_order(drone, battle::Order::Hold, None));
    let at = r.units[drone].motion.pos;
    for _ in 0..5 { r.construction_phase(); }
    assert_eq!(r.units[drone].motion.pos, at);
    assert_eq!(r.drone_cargo(drone), Some(cargo));
    assert_eq!(r.matter.economy.ledger.held(cargo.container), held);
    assert_eq!(r.battle.actors[drone].order, battle::Order::Hold);
    assert!(r.matter.economy.ledger.balanced());
    let mut saved = BlastRange::restore(r.config, &r.snapshot()).expect("stopped cargo restores");
    assert!(saved.battle_order(drone, battle::Order::Build(site), None));
    for _ in 0..300 {
        saved.construction_phase();
        if saved.drone_cargo(drone).is_none() && saved.drone_leg(drone) != Some(Leg::Returning) { break; }
    }
    assert!(saved.matter.economy.ledger.balanced());
    assert_eq!(saved.battle.actors[drone].order, battle::Order::Build(site));
}

#[test]
fn selected_drone_group_can_be_reassigned_without_losing_carried_material() {
    let mut r = compact_greben_range();
    let drones: Vec<_> = r.units.iter().enumerate().filter(|(id, u)| u.kind == RangeUnitKind::SkobaDrone && r.allied(*id)).map(|(id, _)| id).take(2).collect();
    assert_eq!(drones.len(), 2);
    let first = r.create_construction_site(RangeUnitKind::TurretT1, true, [800, 16, 800]).unwrap();
    let second = r.create_construction_site(RangeUnitKind::TurretT1, true, [1100, 16, 800]).unwrap();
    r.set_construction_quota(first, 0);
    r.set_construction_quota(second, 0);
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, BATCH * 4);
    r.matter.economy.ledger.transfer(ground, store, BATCH * 4);
    for &id in &drones { assert!(r.battle_order(id, battle::Order::Build(first), None)); }
    for _ in 0..200 {
        r.construction_phase();
        if drones.iter().all(|&id| r.drone_cargo(id).is_some()) { break; }
    }
    assert!(drones.iter().all(|&id| r.drone_cargo(id).is_some()));
    let cargo = r.drone_cargo(drones[0]).unwrap();
    let held = r.matter.economy.ledger.held(cargo.container);
    let stored = r.matter.economy.ledger.held(store);
    assert!(r.battle_order(drones[0], battle::Order::Build(second), None));
    r.construction_phase();
    let carried = r.drone_cargo(drones[0]).map_or(0, |batch| r.matter.economy.ledger.held(batch.container));
    assert_eq!(carried + r.matter.economy.ledger.held(store), held + stored);
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert!(matches!(c.drones[&(drones[0] as u16)].leg, Leg::Returning | Leg::Idle));
    assert_eq!(c.drones[&(drones[1] as u16)].site, Some(first as u16));
    assert!(r.matter.economy.ledger.balanced());
    BlastRange::restore(r.config, &r.snapshot()).expect("reassigned loaded drone restores");
}

#[test]
fn selected_drone_builds_its_assigned_site_without_automatic_substitution() {
    let mut r = compact_greben_range();
    let drone = r.units.iter().enumerate().rev().find(|(id, u)| u.kind == RangeUnitKind::SkobaDrone && r.allied(*id)).unwrap().0;
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [800, 16, 800]).unwrap();
    assert!(r.set_construction_quota(site, 0));
    let cost = r.construction_cost(RangeUnitKind::TurretT1).unwrap();
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, cost);
    assert_eq!(r.matter.economy.ledger.transfer(ground, store, cost), cost);
    assert!(r.battle_order(drone, battle::Order::Build(site), None));
    r.construction_phase();
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert_eq!(c.drones.get(&(drone as u16)).unwrap().site, Some(site as u16));
    assert_eq!(c.drones.values().filter(|d| d.site == Some(site as u16)).count(), 1);
    let mut restored = BlastRange::restore(r.config, &r.snapshot()).expect("explicit drone assignment restores");
    for _ in 0..20 { r.construction_phase(); restored.construction_phase(); }
    assert_eq!(r.state_hash(), restored.state_hash());
    for _ in 0..500 {
        r.construction_phase();
        if r.construction_sites()[site].complete { break; }
    }
    assert!(r.construction_sites()[site].complete, "selected drone must complete every delivery");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn one_drone_builds_t1_turret_near_storage_within_twenty_seconds() {
    let mut r = compact_greben_range();
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let ground = r.matter.economy.ground;
    let cost = r.construction_cost(RangeUnitKind::TurretT1).unwrap();
    assert!(r.matter.economy.ledger.room(store) >= cost, "the storage must hold a turret's materials");
    assert_eq!(r.matter.economy.ledger.reveal(ground, cost), cost);
    assert_eq!(r.matter.economy.ledger.transfer(ground, store, cost), cost);
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [800, 16, 800]).unwrap();
    let initial = r.construction_progress_views().next().expect("new site has a visible zero-progress bar");
    assert_eq!((initial.at, initial.delivered, initial.required, initial.allied), ([800, 16, 800], 0, cost, true));
    assert!(r.set_construction_quota(site, 1));
    let mut complete_at = None;
    for tick in 1..=20 * crate::TICKS_PER_SECOND {
        r.construction_phase();
        let view = &r.construction_sites()[site];
        assert!(view.assigned <= 1);
        if view.complete { complete_at = Some(tick); break; }
        let progress = r.construction_progress_views().next().expect("unfinished site remains visible");
        assert_eq!((progress.delivered, progress.required), (view.delivered, view.required));
    }
    let tick = complete_at.unwrap_or_else(|| {
        let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
        panic!("one drone must finish within 20 simulation seconds: site={:?}, drones={:?}", c.sites[site], c.drones);
    });
    eprintln!("T1 cost={cost}; completion tick={tick} at {} ticks/s", crate::TICKS_PER_SECOND);
    assert_eq!(r.construction_sites()[site].delivered, cost);
    assert!(r.construction_progress_views().next().is_none(), "completed bars disappear");
    assert_eq!(r.matter.economy.ledger.held(store), 0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn completed_storage_crew_requires_paid_material() {
    let mut r = compact_greben_range();
    let at = [700, 80, 600];
    let count = r.units.len();
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let ground = r.matter.economy.ground;
    assert_eq!(r.matter.economy.ledger.held(store), 0);
    assert!(!spawn_paid_crew_unit(&mut r, RangeUnitKind::SkobaDrone, true, at));
    assert_eq!(r.units.len(), count);
    let drone_cost = r.matter.template_matter(&RangeUnit::of_kind(RangeUnitKind::SkobaDrone, at));
    assert_eq!(r.matter.economy.ledger.reveal(ground, drone_cost), drone_cost);
    assert_eq!(r.matter.economy.ledger.transfer(ground, store, drone_cost), drone_cost);
    // A site completion temporarily takes Construction out of Mining.
    let mut construction = r.mining.as_mut().unwrap().construction.take().unwrap();
    assert!(spawn_paid_crew_unit(&mut r, RangeUnitKind::SkobaDrone, true, at));
    assert_eq!(r.units.len(), count + 1);
    assert!(!construction.drones.contains_key(&(count as u16)));
    construction.register_new_units(&r, count);
    assert!(construction.drones.contains_key(&(count as u16)));
    r.mining.as_mut().unwrap().construction = Some(construction);
    assert_eq!(r.matter.economy.ledger.held(store), 0);
    assert_eq!(r.matter.economy.ledger.held(r.matter.body_container(count)), drone_cost);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn commander_repairs_with_real_matter_and_preserves_active_order_on_restore() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.idle_enemies = true;
    layout.size = [1600, 256, 1600];
    layout.blocks = vec![
        RangeBlock { min: [0, 0, 0], max: [1600, 256, 1600], material: 0 },
        RangeBlock { min: [0, 0, 0], max: [1600, 16, 1600], material: 1 },
    ];
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [1536, 1536];
    layout.units = vec![
        RangeUnitDef { allied: true, kind: RangeUnitKind::Commander, at: [600, 16, 700], weapon: None },
        RangeUnitDef { allied: true, kind: RangeUnitKind::Tank, at: [850, 16, 700], weapon: None },
        RangeUnitDef { allied: false, kind: RangeUnitKind::Tank, at: [1300, 16, 1300], weapon: None },
    ];
    layout.economy = Some(legacy_economy());
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(), layout,
    ).unwrap();
    r.start_battle().unwrap();
    let commander = r.units.iter().position(|u| u.kind == RangeUnitKind::Commander).unwrap();
    let target = r.units.iter().enumerate().find(|(id, u)| u.kind == RangeUnitKind::Tank && r.allied(*id)).unwrap().0;
    let cells = connected_cells(&r.units[target].body);
    let p = *cells.iter().rev().find(|p| r.units[target].body.part_at(p[0], p[1], p[2]) == voxel::unit::Part::Armour).unwrap();
    let density = u64::from(r.matter.source_density(STEEL));
    let body = r.matter.body_container(target);
    let ground = r.matter.economy.ground;
    assert_eq!(r.matter.economy.ledger.transfer(body, ground, density), density);
    assert!(r.units[target].body.remove(p[0], p[1], p[2]));
    r.units[target].removed.push(p.map(|v| v as u8));
    r.units[target].refresh_damage(target);
    let tank = r.mining().unwrap().tank(commander).unwrap();
    assert_eq!(r.matter.economy.ledger.reveal(ground, density * 2), density * 2);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, density * 2), density * 2);
    assert!(r.battle_order(commander, battle::Order::Repair(target), None));
    let mut section = ("", 0, None);
    let mut restored = BlastRange::restore_observed(r.config, &r.snapshot(), |a,b,c| section = (a,b,c))
        .unwrap_or_else(|| panic!("repair order restores: {section:?}"));
    assert_eq!(restored.battle.actors[commander].order, battle::Order::Repair(target));
    r.construction_phase();
    restored.construction_phase();
    assert_eq!(r.state_hash(), restored.state_hash());
    assert!(r.units[target].body.vol.get(p[0], p[1], p[2]));
    assert_eq!(r.matter.economy.ledger.held(tank), density);
    assert!(r.matter.economy.ledger.balanced());
    BlastRange::restore(r.config, &r.snapshot()).expect("repaired body restores");
}

#[test]
fn construction_power_preserves_fractional_work_and_finishes_single_cells() {
    let density = 785;
    let total: u64 = (0..100).map(|tick| powered_work(density, 600, 1000, density, tick)).sum();
    assert_eq!(total, 60 * density);
    assert_eq!(powered_work(density, 0, 1000, density, 1), 0);
    assert_eq!(powered_work(density, 1000, 1000, density, 1), density);
    // A small material supply must receive the same power fraction as a
    // large one, rather than bypassing a shared deficit.
    let small: u64 = (0..100).map(|tick| powered_work(density * 2, 100, 1000, density, tick)).sum();
    assert_eq!(small, 20 * density);
}

#[test]
fn commander_bootstraps_turrets_at_the_intended_pace_and_keeps_books() {
    let mut r = compact_greben_range();
    let id = r.place_sandbox_unit(RangeUnitKind::Commander, true, [1000, 16, 1100], 0).unwrap();
    let tank = r.mining().unwrap().tank(id).unwrap();
    let ground = r.matter.economy.ground;
    r.commander_matter_phase();
    assert_eq!(r.tank_held(id), Some(per_second_rate(COMMANDER_MATTER_PER_SECOND, r.tick)));
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("generated matter restores");
    assert_eq!(r.state_hash(), restored.state_hash());
    let generated = r.matter.economy.ledger.held(tank);
    assert_eq!(r.matter.economy.ledger.transfer(tank, ground, generated), generated);
    let cost = r.construction_cost(RangeUnitKind::TurretT1).unwrap();
    assert_eq!(r.matter.economy.ledger.reveal(ground, cost), cost);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, cost), cost);
    let run = |r: &mut BlastRange, site: usize| -> u32 {
        assert!(r.set_construction_quota(site, 0));
        assert!(r.battle_order(id, battle::Order::Build(site), None));
        for elapsed in 1..=1600 {
            r.commander_matter_phase();
            r.construction_phase();
            r.tick += 1;
            if r.construction_sites()[site].complete { return elapsed; }
        }
        panic!("commander failed to finish turret: {:?}", r.construction_sites()[site]);
    };
    let first = r.create_construction_site(RangeUnitKind::TurretT1, true, [1250, 16, 1100]).unwrap();
    let stocked = run(&mut r, first);
    assert!((19 * crate::TICKS_PER_SECOND as u32..=21 * crate::TICKS_PER_SECOND as u32).contains(&stocked),
        "stocked turret took {stocked} ticks");
    let held = r.matter.economy.ledger.held(tank);
    assert_eq!(r.matter.economy.ledger.transfer(tank, ground, held), held);
    r.units[id].motion.pos = [1000, 16, 600].map(Fx::from_int);
    r.units[id].motion.drive.head = 0;
    r.units[id].update_pose(0);
    let second = r.create_construction_site(RangeUnitKind::TurretT1, true, [1250, 16, 600]).unwrap();
    let empty = run(&mut r, second);
    assert!((57 * crate::TICKS_PER_SECOND as u32..=63 * crate::TICKS_PER_SECOND as u32).contains(&empty),
        "empty turret took {empty} ticks");
    let capacity = r.matter.economy.ledger.get(tank).cap;
    r.matter.economy.ledger.generate(tank, capacity);
    r.commander_matter_phase();
    assert_eq!(r.tank_held(id), Some(capacity), "generation must respect tank capacity");
    assert_eq!(r.matter.economy.ledger.transfer(tank, ground, capacity), capacity);
    r.units[id].status.alive = false;
    r.commander_matter_phase();
    assert_eq!(r.tank_held(id), Some(0), "dead commanders cannot generate resources");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn commander_building_consumes_power_and_slows_when_supply_is_short() {
    let mut r = compact_greben_range();
    let id = r.place_sandbox_unit(RangeUnitKind::Commander, true, [1000, 16, 1100], 0).unwrap();
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [1250, 16, 1100]).unwrap();
    assert!(r.battle_order(id, battle::Order::Build(site), None));
    let tank = r.mining().unwrap().tank(id).unwrap();
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, BATCH);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, BATCH), BATCH);
    let density = u64::from(r.matter.source_density(STEEL));
    let work = COMMANDER_PRINT_RATE / density * density;
    let wanted = (work * COMMANDER_BUILD_DEMAND).div_ceil(COMMANDER_PRINT_RATE);
    assert_eq!(r.mining.as_ref().unwrap().construction.as_ref().unwrap().commander_allowance(&r, site), work);
    let at = r.units[id].motion.pos;
    r.units[id].motion.pos[0] = Fx::from_int(100);
    assert_eq!(r.mining.as_ref().unwrap().construction.as_ref().unwrap().commander_allowance(&r, site), 0,
        "a distant commander must not hold back drone deliveries");
    r.units[id].motion.pos = at;
    assert_eq!(r.construction_energy(true), (COMMANDER_GENERATION, wanted));
    let before = r.tank_held(id).unwrap();
    r.construction_phase();
    let slow = r.construction_sites()[site].delivered;
    assert!(slow > 0 && slow < COMMANDER_PRINT_RATE);
    let trace = r.construction_print_traces().first().expect("paid printing emits a cue");
    assert_eq!(trace.actor, id);
    let built = r.mining.as_ref().unwrap().construction.as_ref().unwrap().sites[site].unit.unwrap() as usize;
    assert!(r.units[built].hit(trace.impact.map(Fx::to_int_trunc)).is_some(),
        "the construction effect must point at a real placed voxel");
    assert_eq!(before - r.tank_held(id).unwrap(), slow);
    let base = r.place_sandbox_unit(RangeUnitKind::Base, true, [1000, 16, 400], 0).unwrap();
    assert_eq!(r.construction_energy(true), (COMMANDER_GENERATION + BASE_GENERATION, wanted));
    assert_eq!(r.construction_power_networks()[id].unwrap().0, COMMANDER_GENERATION + BASE_GENERATION);
    let base_pos = r.units[base].motion.pos;
    r.units[base].motion.pos[0] = Fx::from_int(3000);
    assert_eq!(r.construction_power_networks()[id].unwrap().0, COMMANDER_GENERATION,
        "a remote generator must not power the commander");
    r.units[base].motion.pos = base_pos;
    r.construction_phase();
    let fast = r.construction_sites()[site].delivered - slow;
    assert!(fast > slow, "additional energy must increase work: {slow} -> {fast}");
    let mut restored = BlastRange::restore(r.config, &r.snapshot()).expect("construction energy restores");
    assert_eq!(r.construction_energy(true), restored.construction_energy(true));
    r.construction_phase();
    restored.construction_phase();
    assert_eq!(r.state_hash(), restored.state_hash());
    r.units[base].status.alive = false;
    assert_eq!(r.construction_energy(true), (COMMANDER_GENERATION, wanted));
    let remaining = r.matter.economy.ledger.held(tank);
    assert_eq!(r.matter.economy.ledger.transfer(tank, ground, remaining), remaining);
    r.construction_phase();
    assert!(r.construction_print_traces().is_empty(), "no cue without a paid printing step");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn commander_builds_from_nearby_allied_storage_without_losing_matter() {
    let mut r = compact_greben_range();
    let id = r.place_sandbox_unit(RangeUnitKind::Commander, true, [900, 16, 800], 0).unwrap();
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [1150, 16, 800]).unwrap();
    assert!(r.set_construction_quota(site, 0));
    assert!(r.battle_order(id, battle::Order::Build(site), None));
    let tank = r.mining().unwrap().tank(id).unwrap();
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let ground = r.matter.economy.ground;
    assert_eq!(r.matter.economy.ledger.reveal(ground, BATCH), BATCH);
    assert_eq!(r.matter.economy.ledger.transfer(ground, store, BATCH), BATCH);
    assert_eq!(r.tank_held(id), Some(0));
    r.construction_phase();
    let built = r.construction_sites()[site].delivered;
    let held = r.tank_held(id).unwrap();
    assert!(built > 0, "a nearby stocked warehouse should let the commander build");
    assert_eq!(BATCH - r.matter.economy.ledger.held(store), built + held);
    assert!(held <= Balance::legacy().print_per_second * 2);
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("supplied construction restores");
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(r.matter.economy.ledger.transfer(tank, ground, held), held);
    let before = r.matter.economy.ledger.held(store);
    r.units[id].motion.pos = [1300, 16, 1100].map(Fx::from_int);
    r.units[id].update_pose(0);
    r.construction_phase();
    assert_eq!(r.matter.economy.ledger.held(store), before, "remote storage must not supply the commander");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn commander_turns_the_visible_body_toward_a_sideways_build_site() {
    let mut r = compact_greben_range();
    let id = r.place_sandbox_unit(RangeUnitKind::Commander, true, [1000, 16, 1100], 1).unwrap();
    assert_eq!(r.battle.actors[id].turret.angle, r.units[id].motion.drive.head);
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [1250, 16, 1100]).unwrap();
    assert!(r.set_construction_quota(site, 0));
    assert!(r.battle_order(id, battle::Order::Build(site), None));
    for _ in 0..300 {
        r.step();
        let u = &r.units[id];
        let torso = fx::angle::atan2(u.root_frame(usize::from(u.body.turret_y)).x.z.raw(),
            u.root_frame(usize::from(u.body.turret_y)).x.x.raw());
        assert!(fx::angle::diff(torso, u.motion.drive.head).abs() <= fx::angle::FULL * 22 / 360,
            "torso separated from turning chassis: head {}, torso {}, turret {}", u.motion.drive.head, torso, r.battle.actors[id].turret.angle);
        if r.construction_sites()[site].delivered > 0 { break; }
    }
    let head = r.units[id].motion.drive.head;
    assert!(fx::angle::diff(0, head).abs() <= fx::angle::FULL * 21 / 360,
        "chassis did not face the building: {head}");
    assert!(r.construction_sites()[site].delivered > 0, "commander did not start printing");
}

#[test]
fn commander_prints_only_nearby_using_its_own_matter_and_restores() {
    let mut r = range_with_commander(true);
    let commander = r.units.iter().position(|u| u.kind == RangeUnitKind::Commander).unwrap();
    let site = r
        .create_construction_site(RangeUnitKind::TurretT1, true, [2450, 16, 1800])
        .unwrap();
    let tank = r.mining().unwrap().tank(commander).unwrap();
    assert_eq!(
        r.commander_print_step(commander, site, 1000, BATCH),
        0,
        "empty cargo"
    );
    let amount = 2 * BATCH;
    let ground = r.matter.economy.ground;
    assert_eq!(r.matter.economy.ledger.reveal(ground, amount), amount);
    assert_eq!(
        r.matter.economy.ledger.transfer(ground, tank, amount),
        amount
    );
    assert_eq!(
        r.commander_print_step(commander, site, 10, BATCH),
        0,
        "out of reach"
    );
    let before = r.matter.economy.ledger.held(tank);
    let paid = r.commander_print_step(commander, site, 1000, BATCH);
    assert!(paid > 0 && paid <= BATCH);
    assert_eq!(before - r.matter.economy.ledger.held(tank), paid);
    assert_eq!(r.construction_sites()[site].delivered, paid);
    assert!(r.matter.economy.ledger.balanced());
    for (id, u) in r.units.iter().enumerate() {
        let cells = u.body.solid_count() as u64
            + u.body.segments.iter().map(|s| u64::from(s.body.solid_count())).sum::<u64>();
        let held = r.matter.economy.ledger.held(r.matter.body_container(id));
        assert_eq!(held, cells * u64::from(r.matter.source_density(STEEL)), "body {id} {:?}", u.kind);
    }
    let mut section = ("", 0, None);
    let mut restored = BlastRange::restore_observed(r.config, &r.snapshot(), |a,b,c| section = (a,b,c))
        .unwrap_or_else(|| panic!("partial printed site: {section:?}"));
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(
        r.commander_print_step(commander, site, 1000, BATCH),
        restored.commander_print_step(commander, site, 1000, BATCH)
    );
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn commander_keeps_reclaimed_matter_instead_of_hauling_it_to_a_depot() {
    let mut r = range_with_commander(true);
    let id = r.units.iter().position(|u| u.kind == RangeUnitKind::Commander).unwrap();
    let tank = r.mining().unwrap().tank(id).unwrap();
    let ground = r.matter.economy.ground;
    let amount = BATCH;
    r.matter.economy.ledger.reveal(ground, amount);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, amount), amount);
    r.battle.actors[id].order = battle::Order::Reclaim([2200, 16, 1000]);
    r.mining.as_mut().unwrap().mark_full(id);
    r.haul_phase();
    assert!(!r.hauling(id));
    assert_eq!(r.tank_held(id), Some(amount));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn commander_energy_weapon_uses_a_connected_arm_muzzle() {
    let mut r = range_with_commander(true);
    let id = r.units.iter().position(|u| u.kind == RangeUnitKind::Commander).unwrap();
    assert_eq!(r.battle_weapon(id), Some(battle::Weapon::Energy));
    let muzzles = r.battle_muzzles(id);
    assert_eq!(muzzles.len(), 1);
    let arm_muzzles = super::super::super::weapons::walker_muzzles(&r.units[id]);
    assert_eq!(muzzles[0], arm_muzzles[0].1);
    r.units[id].seg_gone[13] = true;
    assert!(r.battle_muzzles(id).is_empty(), "a severed cannon cannot fire");
}

#[test]
fn commander_build_order_prints_and_resumes_after_save() {
    let mut r = range_with_commander(true);
    let id = r.units.iter().position(|u| u.kind == RangeUnitKind::Commander).unwrap();
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [2450, 16, 1800]).unwrap();
    r.units[id].motion.pos = [2250, 16, 1550].map(Fx::from_int);
    let head = r.units[id].motion.drive.head;
    r.units[id].update_pose(head);
    let tank = r.mining().unwrap().tank(id).unwrap();
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, BATCH * 3);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, BATCH * 3), BATCH * 3);
    assert!(r.battle_order(id, battle::Order::Build(site), None));
    r.construction_phase();
    assert!(r.construction_sites()[site].delivered > 0);
    assert_eq!(r.units[id].arm_targets.unwrap()[2], [1, 0, 0]);
    let mut section = ("", 0, None);
    let mut restored = BlastRange::restore_observed(r.config, &r.snapshot(), |a,b,c| section = (a,b,c))
        .unwrap_or_else(|| panic!("active build order restores: {section:?}"));
    assert_eq!(restored.battle.actors[id].order, battle::Order::Build(site));
    assert_eq!(r.commander_tool_tip(id), restored.commander_tool_tip(id));
    r.construction_phase();
    restored.construction_phase();
    assert_eq!(r.state_hash(), restored.state_hash());
    assert!(r.matter.economy.ledger.balanced());
    r.units[id].seg_gone[17] = true;
    assert!(r.commander_tool_tip(id).is_none());
    assert!(!r.can_commander_build(id, site), "a severed tool cannot build");
}

#[test]
fn sandbox_commander_restores_with_its_tank() {
    let mut r = range();
    let id = r.place_sandbox_unit(RangeUnitKind::Commander, true, [2200, 16, 1000], 0).unwrap();
    let mut section = ("", 0, None);
    let restored = BlastRange::restore_observed(r.config, &r.snapshot(), |a,b,c| section = (a,b,c))
        .unwrap_or_else(|| panic!("sandbox commander restore: {section:?}"));
    assert_eq!(restored.units[id].kind, RangeUnitKind::Commander);
    assert_eq!(restored.tank_fill(id), r.tank_fill(id));
    assert_eq!(restored.state_hash(), r.state_hash());
}

fn range() -> BlastRange {
    range_with_commander(false)
}

#[test]
fn power_plant_can_be_placed_and_generates_only_while_alive() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.size = [640, 256, 640];
    layout.blocks = vec![RangeBlock { min: [0, 0, 0], max: [640, 256, 640], material: 0 },
        RangeBlock { min: [0, 0, 0], max: [640, 16, 640], material: 1 }];
    layout.units.clear(); layout.unit_yaws.clear(); layout.shields.clear(); layout.course = None;
    layout.gun_point = true; layout.gun = [100, 24, 100]; layout.target = [200, 24, 100];
    layout.clear_min = [1, 1]; layout.clear_max = [639, 639];
    layout.economy = Some(legacy_economy());
    let mut r = BlastRange::with_layout(ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(), layout).unwrap();
    let before = r.construction_energy(true).0;
    let id = r.place_sandbox_unit(RangeUnitKind::PowerPlantT1, true, [350, 16, 350], 0).unwrap();
    assert!(r.construction_cost(RangeUnitKind::PowerPlantT1).unwrap() > 0);
    assert_eq!(r.construction_energy(true).0, before + POWER_PLANT_T1_GENERATION);
    assert!(battle::effective_weapon(RangeUnitKind::PowerPlantT1, None, false, None, &r.battle.tuning).is_none());
    let t2 = r.place_sandbox_unit(RangeUnitKind::PowerPlantT2, true, [350, 16, 130], 0).unwrap();
    assert!(r.construction_cost(RangeUnitKind::PowerPlantT2).unwrap() > 0);
    assert_eq!(r.construction_energy(true).0, before + 3 * POWER_PLANT_T1_GENERATION);
    assert!(battle::effective_weapon(RangeUnitKind::PowerPlantT2, None, false, None, &r.battle.tuning).is_none());
    let t3 = r.place_sandbox_unit(RangeUnitKind::PowerPlantT3, true, [125, 16, 400], 0).unwrap();
    let enemy = r.place_sandbox_unit(RangeUnitKind::PowerPlantT1, false, [520, 16, 520], 0).unwrap();
    assert!(r.construction_cost(RangeUnitKind::PowerPlantT3).unwrap() > 0);
    assert_eq!(r.construction_energy(true).0, before + 7 * POWER_PLANT_T1_GENERATION);
    assert!(battle::effective_weapon(RangeUnitKind::PowerPlantT3, None, false, None, &r.battle.tuning).is_none());
    let networks = r.construction_power_networks();
    assert_eq!(networks[id].unwrap().0, 7 * POWER_PLANT_T1_GENERATION);
    assert_eq!(networks[enemy].unwrap().0, POWER_PLANT_T1_GENERATION,
        "an enemy station must never join an allied network");
    let original_pos = r.units[t3].motion.pos;
    r.units[t3].motion.pos[0] = Fx::from_int(3000);
    let separated = r.construction_power_networks();
    assert_eq!(separated[id].unwrap().0, 3 * POWER_PLANT_T1_GENERATION);
    assert_eq!(separated[t3].unwrap().0, 4 * POWER_PLANT_T1_GENERATION);
    r.units[t3].motion.pos = original_pos;
    r.units[t2].status.engine = false;
    assert_eq!(r.construction_energy(true).0, before + 5 * POWER_PLANT_T1_GENERATION,
        "a disabled reactor must stop supplying power");
    r.units[t2].status.engine = true;
    let mut stage = String::new();
    let restored = BlastRange::restore_observed(r.config, &r.snapshot(), |section, offset, unit| {
        stage = format!("{section} at {offset}, unit {unit:?}");
    }).unwrap_or_else(|| panic!("power plant restore failed: {stage}"));
    assert_eq!(restored.units[id].kind, RangeUnitKind::PowerPlantT1);
    assert_eq!(restored.units[t2].kind, RangeUnitKind::PowerPlantT2);
    assert_eq!(restored.units[t3].kind, RangeUnitKind::PowerPlantT3);
    assert_eq!(restored.construction_energy(false).0, POWER_PLANT_T1_GENERATION);
    assert_eq!(restored.construction_energy(true), r.construction_energy(true));
    r.units[id].status.alive = false;
    assert_eq!(r.construction_energy(true).0, before + 6 * POWER_PLANT_T1_GENERATION);
    r.units[t2].status.alive = false;
    assert_eq!(r.construction_energy(true).0, before + 4 * POWER_PLANT_T1_GENERATION);
    r.units[t3].status.alive = false;
    assert_eq!(r.construction_energy(true).0, before);
}

fn range_with_commander(commander: bool) -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.idle_enemies = true;
    layout.size = [3200, 1024, 3200];
    layout.blocks = vec![
        RangeBlock {
            min: [0, 0, 0],
            max: [3200, 1024, 3200],
            material: 0,
        },
        RangeBlock {
            min: [0, 0, 0],
            max: [3200, 16, 3200],
            material: 1,
        },
    ];
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [3136, 3136];
    layout.units = vec![
        RangeUnitDef {
            allied: true,
            kind: RangeUnitKind::GrebenStorage,
            at: [900, 16, 1000],
            weapon: None,
        },
        RangeUnitDef {
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [2900, 16, 2800],
            weapon: None,
        },
    ];
    if commander {
        layout.units.push(RangeUnitDef {
            allied: true,
            kind: RangeUnitKind::Commander,
            at: [2200, 16, 1000],
            weapon: None,
        });
    }
    let mut economy = legacy_economy();
    economy.bank.as_mut().unwrap().base_store = 1 << 40;
    layout.economy = Some(economy);
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let seeded = 1 << 30;
    let economy = &mut r.matter.economy;
    assert_eq!(economy.ledger.reveal(economy.ground, seeded), seeded);
    assert_eq!(
        economy.ledger.transfer(economy.ground, store, seeded),
        seeded
    );
    r.start_battle().unwrap();
    r
}

#[test]
fn greben_drones_load_and_land_on_distinct_station_pads() {
    let mut r = compact_greben_range();
    let home = r.units.iter().position(|u| u.kind == RangeUnitKind::GrebenStorage).unwrap();
    let drones: Vec<_> = r.units.iter().enumerate().filter(|(id, u)|
        r.allied(*id) && u.kind == RangeUnitKind::SkobaDrone && u.status.alive).map(|(id, _)| id).collect();
    assert_eq!(drones.len(), 8);
    let positions: std::collections::BTreeSet<_> = drones.iter().map(|&id| {
        let pad = drone_home_point(&r.units, home, id as u16, r.size);
        assert_eq!(r.units[id].motion.pos.map(Fx::to_int_trunc), pad, "drone {id} not parked on its pad");
        assert_eq!(r.units[id].skoba_form, 0, "parked drone should be open");
        assert_eq!(r.units[id].motion.drive.head, r.units[home].motion.drive.head);
        pad
    }).collect();
    assert_eq!(positions.len(), 8);
    let drone = drones[0];
    let site = r.create_construction_site(RangeUnitKind::TurretT1, true, [1000, 16, 500]).unwrap();
    r.set_construction_quota(site, 0);
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let ground = r.matter.economy.ground;
    let cost = r.construction_cost(RangeUnitKind::TurretT1).unwrap();
    r.matter.economy.ledger.reveal(ground, cost);
    assert_eq!(r.matter.economy.ledger.transfer(ground, store, cost), cost);
    assert!(r.battle_order(drone, battle::Order::Build(site), None));
    for _ in 0..100 {
        r.construction_phase();
        if matches!(r.drone_leg(drone), Some(Leg::Loading(_))) { break; }
    }
    assert!(matches!(r.drone_leg(drone), Some(Leg::Loading(_))));
    let pad = drone_home_point(&r.units, home, drone as u16, r.size);
    assert_eq!(r.units[drone].motion.pos.map(Fx::to_int_trunc), pad);
    assert!(r.drone_cargo(drone).is_some());
    let mut stage = String::new();
    let mut restored = BlastRange::restore_observed(r.config, &r.snapshot(), |name, offset, unit| stage = format!("{name} {offset} {unit:?}"))
        .unwrap_or_else(|| panic!("loading phase failed to restore at {stage}"));
    for _ in 0..20 { r.construction_phase(); restored.construction_phase(); }
    assert_eq!(r.state_hash(), restored.state_hash());
    for _ in 0..1000 {
        r.construction_phase();
        if r.construction_sites()[site].complete && r.drone_leg(drone) == Some(Leg::Idle) { break; }
    }
    assert!(r.construction_sites()[site].complete);
    assert_eq!(r.drone_leg(drone), Some(Leg::Idle));
    assert_eq!(r.units[drone].motion.pos.map(Fx::to_int_trunc), pad);
    assert_eq!(r.units[drone].skoba_form, 0);
    assert_eq!(r.units[drone].motion.drive.head, r.units[home].motion.drive.head);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn greben_starts_with_eight_real_drones_and_one_crawler() {
    let r = range();
    assert_eq!(
        r.units
            .iter()
            .filter(|u| u.kind == RangeUnitKind::SkobaDrone)
            .count(),
        8
    );
    assert_eq!(
        r.units
            .iter()
            .filter(|u| u.kind == RangeUnitKind::Crawler)
            .count(),
        1
    );
    assert!(r
        .units
        .iter()
        .filter(|u| u.kind == RangeUnitKind::SkobaDrone)
        .all(|u| u.status.alive));
}

#[test]
fn higher_tier_construction_keeps_every_part_paid_and_round_trips() {
    for kind in [
        RangeUnitKind::TurretT2,
        RangeUnitKind::ArtilleryTurretT3,
        RangeUnitKind::NuclearSilo,
    ] {
        let mut r = range();
        let at = [2300, 16, 1000];
        let id = r.create_construction_site(kind, true, at).unwrap();
        let mut site = r
            .mining
            .as_ref()
            .unwrap()
            .construction
            .as_ref()
            .unwrap()
            .sites[id]
            .clone();
        assert!(ensure_site_unit(&mut r, &mut site));
        let unit = usize::from(site.unit.unwrap());
        let body = r.matter.body_container(unit);
        let store = r.mining().unwrap().haul().unwrap().store(true);
        let density = Amount::from(r.matter.source_density(STEEL));
        let first = (BATCH / density).max(1) * density;
        for delivered in [first, site.required] {
            let previous = site.delivered;
            let amount = delivered - previous;
            assert_eq!(
                r.matter.economy.ledger.transfer(store, body, amount),
                amount
            );
            site.delivered = delivered;
            {
                let c = r.mining.as_mut().unwrap().construction.as_mut().unwrap();
                c.delivered = delivered;
                c.sites[id] = site.clone();
            }
            apply_site_progress(&mut r, unit, kind, at, site.required, previous, delivered);
            assert!(r.units[unit].status.alive, "{kind:?}");
            if delivered < site.required {
                assert!(!r.units[unit].status.weapon);
            }
            assert!(r.matter.economy.ledger.balanced());
            let mut section = ("", 0, None);
            let restored = BlastRange::restore_observed(r.config, &r.snapshot(), |a, b, c| {
                section = (a, b, c)
            })
            .unwrap_or_else(|| panic!("{kind:?} at {delivered}: {section:?}"));
            assert_eq!(
                r.state_hash(),
                restored.state_hash(),
                "{kind:?} at {delivered}"
            );
        }
        assert!(r.units[unit].removed.is_empty() && r.units[unit].seg_removed.is_empty());
    }
}

#[test]
fn a_loaded_drone_waits_without_a_home_and_returns_cargo_when_one_is_available() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500])
        .unwrap();
    r.set_construction_quota(0, 1);
    let (id, cargo) = (0..600)
        .find_map(|_| {
            r.construction_phase();
            r.mining
                .as_ref()
                .unwrap()
                .construction
                .as_ref()
                .unwrap()
                .drones
                .iter()
                .find_map(|(&id, d)| d.cargo.map(|b| (id, b.container)))
        })
        .expect("loaded delivery");
    r.set_construction_quota(0, 0);
    let homes: Vec<_> = r
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| {
            matches!(
                u.kind,
                RangeUnitKind::GrebenStorage | RangeUnitKind::Crawler
            )
        })
        .map(|(id, _)| id)
        .collect();
    for &home in &homes {
        r.units[home].status.alive = false;
    }
    let at = r.units[usize::from(id)].motion.pos;
    let amount = r.matter.economy.ledger.held(cargo);
    let store = r.side_store(true).unwrap().0;
    for _ in 0..64 {
        r.construction_phase();
    }
    assert_eq!(r.units[usize::from(id)].motion.pos, at);
    assert_eq!(r.matter.economy.ledger.held(cargo), amount);
    assert!(r.matter.economy.ledger.balanced());
    r.units[homes[0]].status.alive = true;
    for _ in 0..400 {
        r.construction_phase();
    }
    assert!(r.drone_cargo(usize::from(id)).is_none());
    assert_eq!(r.drone_leg(usize::from(id)), Some(Leg::Idle));
    assert_eq!(r.side_store(true).unwrap().0, store + amount);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn drones_split_six_two_and_build_two_turrets_from_real_batches() {
    let mut r = range();
    let first = r
        .create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    let second = r
        .create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500])
        .unwrap();
    assert!(r.set_construction_quota(first, 6));
    assert!(r.set_construction_quota(second, 2));
    r.step();
    let sites = r.construction_sites();
    assert_eq!(sites[first].assigned, 6);
    assert_eq!(sites[second].assigned, 2);
    for _ in 0..6000 {
        r.step();
        if r.construction_sites().iter().all(|s| s.complete) {
            break;
        }
    }
    let sites = r.construction_sites();
    assert!(sites.iter().all(|s| s.complete), "{sites:?}");
    assert_eq!(
        r.units
            .iter()
            .filter(|u| u.kind == RangeUnitKind::TurretT1)
            .count(),
        2
    );
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert!(c.trips >= 2 && c.delivered > 0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn a_shot_down_loaded_drone_leaves_one_reclaimable_batch() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    assert!(r.set_construction_quota(0, 1));
    let mut loaded = None;
    for _ in 0..500 {
        r.step();
        loaded = (0..r.units.len()).find(|&id| r.drone_cargo(id).is_some());
        if loaded.is_some() {
            break;
        }
    }
    let id = loaded.expect("a drone picked up a batch");
    let cargo = r.drone_cargo(id).unwrap();
    let carried = r.matter.economy.ledger.held(cargo.container);
    assert!(carried > 0 && carried <= BATCH);
    assert_eq!(carried % Amount::from(r.matter.source_density(STEEL)), 0);
    let before = r.side_store(true).unwrap().0;
    r.units[id].status.alive = false;
    r.construction_phase();
    assert!(r.drone_cargo(id).is_none());
    assert_eq!(r.side_store(true).unwrap().0, before);
    assert_eq!(r.wreck_cargo(id), Some(carried));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_upgrade_buy_four_parked_drones_only_once() {
    let mut r = range();
    let crawler = r
        .units
        .iter()
        .position(|u| u.kind == RangeUnitKind::Crawler)
        .unwrap();
    assert!(r.upgrade_crawler_drones(crawler));
    assert!(!r.upgrade_crawler_drones(crawler));
    let parked: Vec<_> = r
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.kind == RangeUnitKind::SkobaDrone && u.body.vol.get(64, 16, 26))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(parked.len(), 4);
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert!(parked.iter().all(|id| c
        .drones
        .get(&(*id as u16))
        .is_some_and(|d| usize::from(d.home) == crawler)));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn a_loaded_flight_restores_and_continues_bit_for_bit() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    for _ in 0..500 {
        r.step();
        if (0..r.units.len()).any(|id| r.drone_cargo(id).is_some()) {
            break;
        }
    }
    assert!((0..r.units.len()).any(|id| r.drone_cargo(id).is_some()));
    let bytes = r.snapshot();
    let mut restored = BlastRange::restore(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        &bytes,
    )
    .expect("loaded flight restores");
    assert_eq!(r.state_hash(), restored.state_hash());
    for _ in 0..120 {
        r.step();
        restored.step();
    }
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn manual_pilot_keeps_a_loaded_drone_from_automatic_delivery_and_restores_it() {
    let mut r = range();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    r.set_construction_quota(0, 1);
    let id = (0..600)
        .find_map(|_| {
            r.construction_phase();
            (0..r.units.len()).find(|&id| r.drone_cargo(id).is_some())
        })
        .expect("loaded drone");
    let cargo = r.drone_cargo(id);
    let at = r.units[id].motion.pos;
    let head = r.units[id].motion.drive.head;
    assert!(r.battle_order(
        id,
        battle::Order::Pilot {
            throttle: 1,
            turn: 0,
            aim: [2500, 150, 1000],
            fire: false,
        },
        None
    ));
    for _ in 0..8 {
        r.step();
    }
    let forward = r.units[id].motion.pos[0]
        .sub(at[0])
        .mul(fx::angle::cos(head))
        .add(
            r.units[id].motion.pos[2]
                .sub(at[2])
                .mul(fx::angle::sin(head)),
        );
    assert!(
        forward > Fx::ZERO,
        "manual thrust must follow the craft's existing heading"
    );
    assert_eq!(r.units[id].motion.pos[1], at[1]);
    assert_eq!(r.drone_cargo(id), cargo);
    let saved = r.snapshot();
    assert_eq!(u32::from_le_bytes(saved[8..12].try_into().unwrap()), 77);
    let forms = u32::from_le_bytes(saved[12..16].try_into().unwrap()) as usize;
    let legacy: Vec<_> = saved[..8]
        .iter()
        .chain(&saved[16 + forms * 5..])
        .copied()
        .collect();
    let old = BlastRange::restore(r.config, &legacy).expect("legacy loaded craft restores");
    assert_eq!(old.drone_cargo(id), cargo);
    assert!(old.units[id].body.vol == r.units[id].body.vol);
    let mut copy =
        BlastRange::restore(r.config, &r.snapshot()).expect("piloted loaded drone restores");
    for _ in 0..8 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
        assert_eq!(r.drone_cargo(id), cargo);
    }
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn skoba_forms_restore_damaged_cargo_cells_and_reject_malformed_metadata() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    for u in &mut layout.units {
        u.kind = RangeUnitKind::SkobaDrone;
    }
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    for (id, form) in [(0, 1), (1, 2)] {
        r.units[id].restore_skoba_form(form);
        r.units[id].refresh_damage(id);
        r.units[id].update_pose(0);
    }
    let cell = [12usize, 4, 20]; // Cargo-only cell, absent from the open frame.
    assert!(!voxel::unit::skoba().vol.get(cell[0], cell[1], cell[2]));
    assert!(r.units[1].body.remove(cell[0], cell[1], cell[2]));
    r.units[1].removed.push(cell.map(|v| v as u8));
    r.matter.annihilate_body(1, 1);
    r.units[1].refresh_damage(1);
    let bytes = r.snapshot();
    let back = BlastRange::restore(r.config, &bytes).expect("damaged loaded form restores");
    assert_eq!(bytes, back.snapshot());
    assert!(back.units[1].body.vol == r.units[1].body.vol);
    assert!(back.matter.economy.ledger.balanced());
    for (at, value) in [
        (12, u32::MAX),
        (16, u32::MAX),
        (21, 0),
        (21, r.units.len() as u32),
    ] {
        let mut bad = bytes.clone();
        bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(BlastRange::restore(r.config, &bad).is_none(), "field {at}");
    }
    for form in [0, 3, 255] {
        let mut bad = bytes.clone();
        bad[20] = form;
        assert!(BlastRange::restore(r.config, &bad).is_none());
    }
    for end in 12..26 {
        assert!(BlastRange::restore(r.config, &bytes[..end]).is_none());
    }
}

#[test]
fn ready_menu_scenarios_start_real_construction() {
    for id in [24, 25, 26] {
        let preset = modding::range_skirmish::stock(id).unwrap();
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-km.ron"))
                .unwrap();
        let layout = preset.apply_to(layout).unwrap();
        let mut r = BlastRange::with_layout(
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
            layout,
        )
        .unwrap();
        assert_eq!(
            r.units
                .iter()
                .filter(|u| u.kind == RangeUnitKind::SkobaDrone)
                .count(),
            8
        );
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

#[test]
fn construction_never_assigns_an_enemy_drone() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, false, [2300, 16, 1500])
        .unwrap();
    r.construction_phase();
    assert_eq!(r.construction_sites()[0].assigned, 0);
    assert!((0..r.units.len()).all(|id| r.drone_leg(id) != Some(Leg::ToSource)));
}

#[test]
fn a_dead_unloaded_drone_releases_its_reservation_and_is_replaced() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500])
        .unwrap();
    r.set_construction_quota(0, 1);
    r.construction_phase();
    let id = (0..r.units.len())
        .find(|&id| r.drone_leg(id) == Some(Leg::ToSource))
        .unwrap();
    assert!(r.drone_cargo(id).is_none());
    r.units[id].status.alive = false;
    r.construction_phase();
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert_eq!(c.sites[0].reserved, 0);
    assert!(c.drones[&(id as u16)].site.is_none());
    r.construction_phase();
    assert_eq!(r.construction_sites()[0].assigned, 1);
    assert_ne!(r.drone_leg(id), Some(Leg::ToSource));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn an_unstarted_site_with_empty_drones_restores() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500])
        .unwrap();
    let bytes = r.snapshot();
    let restored = BlastRange::restore(r.config, &bytes).expect("idle construction must restore");
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn a_delivery_does_not_repair_already_built_cells_or_erase_dents() {
    let mut r = range();
    let kind = RangeUnitKind::TurretT1;
    let at = [2300, 16, 1000];
    let density = r.matter.source_density(STEEL) as Amount;
    let mut site = Site {
        kind,
        allied: true,
        at,
        required: 1 << 30,
        delivered: 0,
        reserved: 0,
        quota: 1,
        unit: None,
    };
    assert!(ensure_site_unit(&mut r, &mut site));
    let id = usize::from(site.unit.unwrap());
    let cells = connected_cells(&r.units[id].body);
    let first = cells.len() / 2;
    let next = first + 100;
    let required = cells.len() as Amount * density;
    apply_site_progress(&mut r, id, kind, at, required, 0, first as Amount * density);
    let broken = cells[..first]
        .iter()
        .rev()
        .find(|p| r.units[id].body.part_at(p[0], p[1], p[2]) == voxel::unit::Part::Armour)
        .unwrap();
    let broken = *broken;
    assert!(r.units[id].body.remove(broken[0], broken[1], broken[2]));
    r.units[id].removed.push(broken.map(|v| v as u8));
    let dent = cells[0].map(|v| v as u8);
    r.units[id].dents.insert(dent, 17);
    apply_site_progress(
        &mut r,
        id,
        kind,
        at,
        required,
        first as Amount * density,
        next as Amount * density,
    );
    assert!(!r.units[id].body.vol.get(broken[0], broken[1], broken[2]));
    assert_eq!(r.units[id].dents.get(&dent), Some(&17));
    assert_eq!(r.units[id].body.solid_count() as usize, next - 1);
    assert_eq!(r.units[id].removed.len(), cells.len() - next + 1);
}

#[test]
fn a_destroyed_site_releases_drones_and_returns_loaded_material() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    r.set_construction_quota(0, 1);
    // Seed the first delivery, then destroy the construction while a new
    // loaded batch is travelling toward it.
    for _ in 0..1600 {
        r.construction_phase();
        let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
        if c.sites[0].unit.is_some() && c.drones.values().any(|d| d.cargo.is_some()) {
            break;
        }
    }
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    let unit = usize::from(c.sites[0].unit.expect("first batch built a body"));
    let (&drone, d) = c
        .drones
        .iter()
        .find(|(_, d)| d.cargo.is_some())
        .expect("next batch in flight");
    let cargo = d.cargo.unwrap().container;
    let amount = r.matter.economy.ledger.held(cargo);
    let before = r.side_store(true).unwrap().0;
    r.units[unit].status.alive = false;
    for _ in 0..400 {
        r.construction_phase();
    }
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert_eq!(c.sites[0].reserved, 0);
    assert!(c.drones.values().all(|d| d.site.is_none()));
    assert!(r.drone_cargo(usize::from(drone)).is_none());
    assert_eq!(r.side_store(true).unwrap().0, before + amount);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn empty_skoba_flies_faster_and_parking_opens_and_aligns_it() {
    let mut units = vec![
        RangeUnit::of_kind(RangeUnitKind::SkobaDrone, [300, 80, 300]),
        RangeUnit::of_kind(RangeUnitKind::GrebenStorage, [600, 16, 600]),
    ];
    units[1].motion.drive.head = fx::angle::QUARTER;
    units[1].update_pose(fx::angle::QUARTER);
    set_skoba_pose(&mut units[0], 0, true);
    let before = units[0].motion.pos[0].to_int_trunc();
    assert!(!fly(&mut units[0], [500, 80, 300]));
    assert_eq!(units[0].motion.pos[0].to_int_trunc() - before, FLY_COMPACT_SPEED);
    park_skoba(&mut units, 0, 1);
    assert_eq!(units[0].skoba_form, 0);
    assert_eq!(units[0].motion.drive.head, units[1].motion.drive.head);
    let before = units[0].motion.pos[0].to_int_trunc();
    assert!(!fly(&mut units[0], [500, 80, 300]));
    assert_eq!(units[0].motion.pos[0].to_int_trunc() - before, FLY_SPEED);
}

#[test]
fn drone_approaches_greben_above_the_pad_before_descending() {
    let mut drone = RangeUnit::of_kind(RangeUnitKind::SkobaDrone, [200, 30, 200]);
    set_skoba_pose(&mut drone, 0, true);
    let target = [300, 100, 300];
    let size = [768, 512, 768];
    assert!(!approach_home(&mut drone, target, size, true));
    assert_eq!(drone.motion.pos[0].to_int_trunc(), 200);
    assert_eq!(drone.motion.pos[2].to_int_trunc(), 200);
    for _ in 0..100 {
        let p = drone.motion.pos.map(Fx::to_int_trunc);
        if p[0] != target[0] || p[2] != target[2] {
            if p[0] != 200 || p[2] != 200 { assert!(p[1] >= 140); }
        }
        if approach_home(&mut drone, target, size, true) { break; }
    }
    assert_eq!(drone.motion.pos.map(Fx::to_int_trunc), target);
}
