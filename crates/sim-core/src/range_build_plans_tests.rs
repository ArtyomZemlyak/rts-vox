use super::*;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef};
fn scene() -> BlastRange {
    scene_with_height(384)
}
fn scene_with_height(height: u32) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size = [1600, height, 1600];
    l.battle = true;
    l.idle_enemies = true;
    l.course = None;
    l.ready = None;
    l.shields.clear();
    l.clear_min = [1, 1];
    l.clear_max = [1599, 1599];
    l.blocks = vec![
        RangeBlock {
            min: [0; 3],
            max: [1600, height as i32, 1600],
            material: 0,
        },
        RangeBlock {
            min: [0; 3],
            max: [1600, 16, 1600],
            material: 1,
        },
    ];
    l.units = vec![
        RangeUnitDef {
            kind: RangeUnitKind::Commander,
            allied: true,
            at: [250, 16, 700],
            weapon: None,
        },
        RangeUnitDef {
            kind: RangeUnitKind::Base,
            allied: true,
            at: [250, 16, 1000],
            weapon: None,
        },
    ];
    let mut economy = modding::range_economy::RangeEconomy::stock();
    // Plans here construct turrets/storage and exercise the storage's drones.
    // Unrelated T1-T4 factories need no product exit masks in this fixture.
    economy.production.as_mut().unwrap().producers
        .retain(|producer| producer.kind == RangeUnitKind::GrebenStorage);
    l.economy = Some(economy);
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut r = BlastRange::with_saved_layout(cfg, l).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r
}
#[test]
fn commander_can_place_and_start_printing_greben_with_real_world_headroom() {
    let mut legacy = scene_with_height(192);
    let at = [900, 16, 700];
    let v = super::super::placement::Validator::default();
    assert_eq!(v.runtime(&legacy, RangeUnitKind::GrebenStorage, at, 0),
        Err(super::super::placement::Rejection::Outside));
    assert!(!legacy.execute_command(Command::PlaceConstruction {
        actor: 0, kind: RangeUnitKind::GrebenStorage, at, queued: false }).accepted());
    let mut r = scene_with_height(super::super::placement::catalogue_height(16));
    r.place_sandbox_unit(RangeUnitKind::Tank, false, [1400, 16, 1400], 0).unwrap();
    r.start_battle().unwrap();
    r.units[0].motion.pos[0] = fx::Fx::from_int(600);
    r.units[0].update_pose(0);
    assert!(r.can_construct(RangeUnitKind::GrebenStorage, true, at));
    assert!(r.execute_command(Command::PlaceConstruction {
        actor: 0, kind: RangeUnitKind::GrebenStorage, at, queued: false }).accepted());
    let tank = r.mining().unwrap().tank(0).unwrap();
    let budget = r.construction_cost(RangeUnitKind::GrebenStorage).unwrap().min(10_000_000);
    r.matter.economy.ledger.generate(tank, budget);
    let before = r.tank_held(0).unwrap();
    assert!(r.can_commander_build(0, 0));
    let paid = r.commander_print_step(0, 0, 4096, budget);
    assert!(paid > 0);
    assert_eq!(r.tank_held(0), Some(before - paid));
    assert!(r.construction_sites()[0].unit.is_some());
    restore(&r);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/greben-placement-542");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("preview.bin"), r.snapshot()).unwrap();
}
fn place(r: &mut BlastRange, x: i32, queued: bool) -> usize {
    assert!(r
        .execute_command(Command::PlaceConstruction {
            actor: 0,
            kind: RangeUnitKind::TurretT1,
            at: [x, 16, 700],
            queued
        })
        .accepted());
    r.construction_sites().last().unwrap().id
}
fn restore(r: &BlastRange) -> BlastRange {
    let mut section = ("", 0, None);
    let b = BlastRange::restore_observed(r.config, &r.snapshot(), |a, b, c| section = (a, b, c))
        .unwrap_or_else(|| panic!("plans refused at {section:?}"));
    assert_eq!(b.state_hash(), r.state_hash());
    b
}
#[test]
fn blueprints_queue_sequentially_without_creating_material_and_restore_mid_plan() {
    let mut r = scene();
    let initial = r.matter.economy.ledger.initial();
    r.start_command_capture(16).unwrap();
    let first = place(&mut r, 500, false);
    let second = place(&mut r, 650, true);
    let third = place(&mut r, 800, true);
    assert_eq!(r.matter.economy.ledger.initial(), initial);
    assert_eq!(r.units.len(), 2, "unpaid holograms are not bodies");
    assert_eq!(r.battle.actors[0].order, Order::Build(first));
    assert_eq!(
        r.order_queue(0).iter().map(|p| p.order).collect::<Vec<_>>(),
        vec![
            Order::Build(first),
            Order::Build(second),
            Order::Build(third)
        ]
    );
    assert!(r
        .construction_sites()
        .iter()
        .all(|s| s.delivered == 0 && s.quota == 0));
    let trace = r.finish_command_capture().unwrap();
    assert_eq!(
        trace.records.len(),
        3,
        "internal application is not a second replay input"
    );
    let mut back = restore(&r);
    let tank = r.mining().unwrap().tank(0).unwrap();
    let tank_back = back.mining().unwrap().tank(0).unwrap();
    let price = r.construction_cost(RangeUnitKind::TurretT1).unwrap();
    r.matter.economy.ledger.generate(tank, price * 3);
    back.matter.economy.ledger.generate(tank_back, price * 3);
    for site in [first, second, third] {
        for s in [&mut r, &mut back] {
            // Exercise completion scheduling directly; print speed/power has
            // separate physical-economy tests, this fixture pays exact cells.
            assert_eq!(s.commander_print_step(0, site, 1200, price), price);
            s.order_plan_phase();
        }
        assert_eq!(r.state_hash(), back.state_hash());
    }
    assert!(r.construction_progress_views().next().is_none());
    assert!(r.matter.economy.ledger.balanced());
    restore(&r);
}
#[test]
fn cancelling_an_unpaid_blueprint_releases_space_and_cannot_mint_reclaim() {
    let mut r = scene();
    let first = place(&mut r, 500, false);
    let second = place(&mut r, 650, true);
    let initial = r.matter.economy.ledger.initial();
    assert!(!r.can_construct(RangeUnitKind::TurretT1, true, [650, 16, 700]));
    assert!(r
        .execute_command(Command::CancelLastOrder { actor: 0 })
        .accepted());
    assert!(r.construction_cancelled(second));
    assert!(!r.construction_cancelled(first));
    assert_eq!(r.battle.actors[0].order, Order::Build(first));
    assert!(r.can_construct(RangeUnitKind::TurretT1, true, [650, 16, 700]));
    assert_eq!(r.matter.economy.ledger.initial(), initial);
    assert_eq!(r.units.len(), 2);
    assert!(r.matter.debris.fragments.is_empty());
    restore(&r);
    assert!(r
        .execute_command(Command::CancelConstruction { site: first })
        .accepted());
    r.order_plan_phase();
    assert!(r.construction_progress_views().next().is_none());
    assert_eq!(r.matter.economy.ledger.initial(), initial);
    restore(&r);
}
#[test]
fn stopped_and_partly_built_site_can_be_resumed_and_destroyed_sites_hide_the_plan() {
    let mut r = scene();
    let site = place(&mut r, 500, false);
    assert_eq!(r.construction_site_at([500, 16, 700]), Some(site));
    assert!(r
        .execute_command(Command::Order {
            actor: 0,
            order: Order::Hold,
            destination: None
        })
        .accepted());
    assert!(r.can_commander_build(0, site));
    assert_eq!(r.construction_site_at([500, 16, 700]), Some(site));
    assert!(r.battle_order(0, Order::Build(site), None));
    let tank = r.mining().unwrap().tank(0).unwrap();
    r.matter.economy.ledger.generate(tank, 100_000);
    assert!(r.commander_print_step(0, site, 1000, 100_000) > 0);
    let state = r.construction_site_state(site).unwrap();
    assert!(state.delivered > 0 && !state.complete);
    assert_eq!(
        r.execute_command(Command::CancelConstruction { site }),
        Outcome::Rejected,
        "paid material remains a real resumable building"
    );
    assert!(r.can_commander_build(0, site));
    restore(&r);
    r.units[state.unit.unwrap()].status.alive = false;
    assert!(r.construction_site_state(site).unwrap().destroyed);
    assert!(r.construction_progress_views().next().is_none());
    assert!(!r.can_commander_build(0, site));
}
#[test]
fn movement_can_be_queued_between_buildings_and_new_orders_replace_the_queue() {
    let mut r = scene();
    let first = place(&mut r, 500, false);
    assert!(r
        .execute_command(Command::QueueOrder {
            actor: 0,
            order: Order::Move,
            destination: Some([900, 700])
        })
        .accepted());
    let second = place(&mut r, 1100, true);
    let price = r.construction_cost(RangeUnitKind::TurretT1).unwrap();
    let tank = r.mining().unwrap().tank(0).unwrap();
    r.matter.economy.ledger.generate(tank, price);
    assert_eq!(r.commander_print_step(0, first, 1000, price), price);
    r.order_plan_phase();
    assert_eq!(r.battle.actors[0].order, Order::Move);
    assert_eq!(r.units[0].motion.destination, Some([900, 700]));
    restore(&r);
    r.units[0].motion.stop();
    r.order_plan_phase();
    assert_eq!(r.battle.actors[0].order, Order::Build(second));
    assert!(r
        .execute_command(Command::Order {
            actor: 0,
            order: Order::Hold,
            destination: None
        })
        .accepted());
    assert!(r.build_plans.queues.is_empty());
    assert!(r.can_commander_build(0, second));
}

#[test]
fn greben_plans_wait_their_turn_and_cancelling_loaded_drones_returns_all_material() {
    let original = scene();
    let cfg = original.config;
    let mut layout = original.layout.clone().unwrap();
    drop(original);
    layout.units.extend([
        RangeUnitDef {
            kind: RangeUnitKind::GrebenStorage,
            allied: true,
            at: [1000, 16, 1000],
            weapon: None,
        },
        RangeUnitDef {
            kind: RangeUnitKind::SkobaDrone,
            allied: true,
            at: [800, 120, 1100],
            weapon: None,
        },
        RangeUnitDef {
            kind: RangeUnitKind::SkobaDrone,
            allied: true,
            at: [900, 120, 1100],
            weapon: None,
        },
    ]);
    let mut r = BlastRange::with_saved_layout(cfg, layout).unwrap();
    r.set_physics_workers(1);
    let store = r.mining().unwrap().haul().unwrap().store(true);
    r.matter.economy.ledger.generate(store, 5_000_000);
    let initial = r.matter.economy.ledger.initial();
    for (x, queued) in [(600, false), (800, true)] {
        assert!(r
            .execute_command(Command::PlaceConstruction {
                actor: 2,
                kind: RangeUnitKind::TurretT1,
                at: [x, 16, 650],
                queued
            })
            .accepted());
    }
    assert!(r.construction_site_state(0).unwrap().quota > 0);
    assert_eq!(r.construction_site_state(1).unwrap().quota, 0);
    assert!(r
        .execute_command(Command::Order {
            actor: 2,
            order: Order::Hold,
            destination: None
        })
        .accepted());
    assert_eq!(r.construction_site_state(0).unwrap().quota, 0);
    assert!(
        r.battle_order(2, Order::Build(0), None),
        "greben RMB must resume the existing site"
    );
    assert!(r
        .execute_command(Command::QueueOrder {
            actor: 2,
            order: Order::Build(1),
            destination: None
        })
        .accepted());
    for _ in 0..240 {
        r.tick += 1;
        r.order_plan_phase();
        r.construction_phase();
        if [3, 4].into_iter().any(|id| r.drone_cargo(id).is_some()) {
            break;
        }
    }
    assert!(
        [3, 4].into_iter().any(|id| r.drone_cargo(id).is_some()),
        "fixture must cancel a real loaded batch"
    );
    assert_eq!(r.construction_site_state(0).unwrap().delivered, 0);
    for site in [0, 1] {
        assert!(r
            .execute_command(Command::CancelConstruction { site })
            .accepted());
    }
    restore(&r);
    for _ in 0..240 {
        r.tick += 1;
        r.order_plan_phase();
        r.construction_phase();
    }
    assert_eq!(r.matter.economy.ledger.held(store), 5_000_000);
    assert!([3, 4].into_iter().all(|id| r.drone_cargo(id).is_none()));
    assert_eq!(r.matter.economy.ledger.initial(), initial);
    assert!(r.matter.economy.ledger.balanced());
    restore(&r);
}

#[test]
fn cancellation_history_does_not_consume_the_active_site_limit() {
    let mut r = scene();
    let initial = r.matter.economy.ledger.initial();
    for _ in 0..34 {
        let site = place(&mut r, 500, false);
        assert!(r
            .execute_command(Command::CancelConstruction { site })
            .accepted());
        r.order_plan_phase();
    }
    assert_eq!(r.construction_history_count(), 34);
    assert!(r.can_construct(RangeUnitKind::TurretT1, true, [500, 16, 700]));
    assert_eq!(r.matter.economy.ledger.initial(), initial);
    restore(&r);
    let hash = r.state_hash();
    assert_eq!(
        r.execute_command(Command::QueueOrder {
            actor: 0,
            order: Order::Build(0),
            destination: Some([1, 2])
        }),
        Outcome::Rejected
    );
    assert_eq!(r.state_hash(), hash);
}

#[test]
fn completed_movement_is_not_saved_as_an_invalid_empty_waypoint() {
    let mut r = scene();
    let site = place(&mut r, 500, false);
    assert!(r
        .execute_command(Command::FormationMove {
            actor: 0,
            destination: [300, 700],
            fallback: [300, 700]
        })
        .accepted());
    r.units[0].motion.stop();
    assert!(r
        .execute_command(Command::QueueOrder {
            actor: 0,
            order: Order::Build(site),
            destination: None
        })
        .accepted());
    assert_eq!(
        r.order_queue(0),
        vec![PlannedOrder {
            order: Order::Build(site),
            destination: None
        }]
    );
    restore(&r);
}
