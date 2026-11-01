use super::*;

#[path = "range_nuclear_radius_tests.rs"]
mod radius;

#[path = "range_nuclear_environment_tests.rs"]
mod environment;

fn nuclear_arena() -> BlastRange {
    let original = arena_of([RangeUnitKind::NuclearSilo, RangeUnitKind::Base]);
    let config = original.config;
    let mut l = original.layout.unwrap();
    l.size = [4096, 192, 1024];
    l.clear_min = [64, 64];
    l.clear_max = [4000, 980];
    l.units[0].at = [170, 16, 512];
    l.units[1].at = [3000, 16, 512];
    let mut r = BlastRange::with_layout(config, l).unwrap();
    r.set_ai_vs_ai(true);
    r.start_battle().unwrap();
    r
}

#[test]
fn nuclear_is_manual_stationary_and_stop_cancels_preparation() {
    let mut r = nuclear_arena();
    assert!(r.battle.nuclear_config.unit_ablation_resistance > 0);
    assert!(r.units[0].status.alive && r.units[0].status.weapon);
    assert_eq!(r.battle_weapon(0), Some(Weapon::Nuclear));
    assert_eq!(
        r.battle_attack_range(0),
        Some(modding::attack_ranges::stock().nuclear)
    );
    assert!(!r.battle_order(0, Order::Auto, None));
    assert!(!r.battle_order(0, Order::Attack(1), None));
    assert!(!r.battle_order(0, Order::Move, Some([200, 512])));
    let reach = r.battle.tuning.nuclear.range;
    std::sync::Arc::make_mut(&mut r.battle.tuning).nuclear.range = 100;
    assert!(!r.battle_order(0, Order::Ground([3000, 15, 650]), None));
    std::sync::Arc::make_mut(&mut r.battle.tuning).nuclear.range = reach;
    for _ in 0..90 {
        r.step();
    }
    assert_eq!(r.battle.actors[0].fired, 0);
    assert!(r.battle_order(0, Order::Ground([3000, 15, 650]), None));
    for _ in 0..20 {
        r.step();
    }
    assert!(r.battle_charge(0).is_some());
    assert_eq!(r.battle.actors[0].target, None);
    assert!(r.battle_order(0, Order::Hold, None));
    for _ in 0..90 {
        r.step();
    }
    assert_eq!(r.battle.actors[0].fired, 0);
    assert!(r.battle.rounds.is_empty());
}

#[test]
fn nuclear_launch_is_one_missile_with_real_crater_and_flight_replay() {
    let mut r = nuclear_arena();
    assert!(r.battle_order(0, Order::Ground([3000, 15, 650]), None));
    let mut flying = false;
    let mut impact = false;
    let mut soil = 0;
    let mut replica: Option<BlastRange> = None;
    for _ in 0..720 {
        r.step();
        if let Some(copy) = &mut replica {
            copy.step();
            assert_eq!(copy.state_hash(), r.state_hash());
        }
        if !flying && !r.battle.rounds.is_empty() {
            flying = true;
            assert_eq!(r.battle.rounds.len(), 1);
            assert_eq!(r.battle.actors[0].order, Order::Hold);
            assert!(
                !r.battle_order(0, Order::Ground([3000, 15, 650]), None),
                "reload admitted another launch"
            );
            assert!(r.battle.actors[0].next >= r.tick + r.battle.tuning.nuclear.reload_ticks - 1);
            replica =
                Some(BlastRange::restore(r.config, &r.snapshot()).expect("nuclear flight restore"));
        }
        impact |= r.battle.impacts.iter().any(|i| i.weapon == Weapon::Nuclear);
        soil += r
            .world
            .completed()
            .iter()
            .filter(|(e, b)| e.mat == 0 && matches!(b, 1 | 7 | 8))
            .count();
        if impact && soil > 0 && r.units[1].revision > 0 {
            break;
        }
    }
    assert!(
        flying && impact && soil > 0,
        "flight={flying} impact={impact} soil={soil}"
    );
    assert_eq!(r.battle.actors[0].fired, 1);
    assert!(
        r.units[1].revision > 0,
        "nuclear blast left the base intact"
    );
    assert!(r.matter.economy.ledger.balanced());
    // Completing reload never replays an already consumed launch order.
    r.battle.actors[0].next = r.tick;
    for _ in 0..10 {
        r.step();
    }
    assert_eq!(r.battle.actors[0].fired, 1);
}

#[test]
fn nuclear_stock_mission_destroys_fortifications_and_restores_large_wave_budget() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let tuning = Tuning::from_ron(include_str!("../../../data/range-battle.ron")).unwrap();
    if !layout.zones.iter().any(|z| z.name == tuning.zone.name) {
        layout.zones.push(tuning.zone);
    }
    let mut scenario = modding::range_skirmish::stock(18).unwrap();
    let distant = scenario.units.len();
    for (kind, z) in [
        (RangeUnitKind::Tank, 6750),
        (RangeUnitKind::Spider, 7000),
        (RangeUnitKind::TurretT2, 7250),
    ] {
        scenario.units.push(modding::range::RangeUnitDef {
            kind,
            at: [6200, 16, z],
            allied: false,
            weapon: None,
        });
    }
    let target = scenario.zone.target;
    let layout = scenario.apply_to(layout).unwrap();
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, layout).unwrap();
    r.start_battle().unwrap();
    assert_eq!(r.battle.nuclear_config.propagation_cell_units(), 64);
    let observation = r.enemy_ai_observation().unwrap();
    assert!(observation
        .units
        .iter()
        .skip(1)
        .all(|u| !u.armed && !u.mobile && !u.can_reposition));
    let positions: Vec<_> = r.units.iter().map(|u| u.motion.pos).collect();
    for _ in 0..90 {
        r.step();
    }
    assert_eq!(
        r.units.iter().map(|u| u.motion.pos).collect::<Vec<_>>(),
        positions
    );
    assert!(r.battle.actors.iter().all(|a| a.fired == 0));
    assert!(r
        .battle
        .actors
        .iter()
        .skip(1)
        .all(|a| a.order == Order::Hold));
    let restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert!(restored.layout.as_ref().unwrap().idle_enemies);
    assert_eq!(restored.state_hash(), r.state_hash());
    assert!(r.battle_order(0, Order::Ground(target), None));
    let mut restored_wave = false;
    for _ in 0..720 {
        r.step();
        if !restored_wave && !r.battle.waves.is_empty() {
            assert!(r.battle.waves[0].energy > u64::from(u32::MAX) * 1000);
            let copy =
                BlastRange::restore(r.config, &r.snapshot()).expect("strategic wave restore");
            assert_eq!(copy.state_hash(), r.state_hash());
            assert_eq!(copy.battle.waves[0].energy, r.battle.waves[0].energy);
            restored_wave = true;
        }
    }
    assert!(restored_wave);
    assert!(r.units[0].status.alive, "distant silo was destroyed");
    assert!(
        r.units[distant..]
            .iter()
            .all(|u| u.status.alive && u.revision == 0),
        "receivers beyond 260 m suffered remote damage"
    );
    assert!(
        r.units[1..4].iter().all(|u| !u.status.alive),
        "fortifications survived"
    );
    assert_eq!(r.battle.actors[0].fired, 1);
    assert!(r.removed > 0);
    assert!(r.matter.economy.ledger.balanced());
}
