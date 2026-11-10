use super::ai::{
    AiOrder, Decision, Observation, Policy, Rules, UnitObservation, OBSERVATION_VERSION,
};
use super::*;
use modding::explosion::Delivery;
use modding::range::RangeUnitDef;

#[test]
fn range_scenario_continues_after_last_enemy_but_standard_battle_ends() {
    let mut ordinary = arena_of([RangeUnitKind::Tank, RangeUnitKind::Tank]);
    ordinary.start_battle().unwrap();
    ordinary.units[1].status.alive = false;
    ordinary.battle_tick();
    assert_eq!(ordinary.battle.phase, Phase::Victory);

    let mut sandbox = arena_of([RangeUnitKind::Tank, RangeUnitKind::Tank]);
    sandbox.layout.as_mut().unwrap().endless_battle = true;
    sandbox.start_battle().unwrap();
    let restored = BlastRange::restore(sandbox.config.clone(), &sandbox.snapshot()).expect("scenario save restores");
    assert!(restored.layout.as_ref().unwrap().endless_battle);
    sandbox.units[1].status.alive = false;
    sandbox.battle_tick();
    assert_eq!(sandbox.battle.phase, Phase::Fighting);
    assert!(sandbox.battle_order(0, Order::Move, Some([200, 336])));
}

#[path = "range_nuclear_tests.rs"]
mod nuclear_tests;

#[test]
fn commander_turns_before_firing_at_a_sideways_target() {
    let mut r = arena_of([RangeUnitKind::Commander, RangeUnitKind::Tank]);
    let p = r.units[0].motion.pos.map(Fx::to_int_trunc);
    r.units[1].motion.pos = [p[0], p[1], p[2] + 300].map(Fx::from_int);
    let head = r.units[1].motion.drive.head;
    r.units[1].update_pose(head);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    for _ in 0..300 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 { break; }
    }
    assert!(r.battle.actors[0].fired > 0, "commander did not turn and fire: head {}, yaw rate {}, turret {}, order {:?}, arm {:?}, target {:?}, alive {}", r.units[0].motion.drive.head, r.units[0].motion.drive.yaw_rate, r.battle.actors[0].turret.angle, r.battle.actors[0].order, r.units[0].arm_targets, aim_point(&r.units[1]), r.units[0].status.alive);
    assert!(fx::angle::diff(fx::angle::QUARTER, r.units[0].motion.drive.head).abs() <= fx::angle::FULL * 21 / 360);
}

#[test]
fn commander_rotates_hull_before_pointing_tools_sideways() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Commander, [400, 16, 400]);
    let target = [400, 80, 800];
    assert!(!u.commander_face(target));
    let mut previous = u.motion.drive.head;
    for _ in 0..600 {
        u.set_arm_targets([target, target, [0, 0, 0], [0, 0, 0]], u.motion.drive.head);
        let gun = &u.seg_frames[13];
        let yaw = fx::angle::atan2(gun.x.z.raw(), gun.x.x.raw());
        assert!(fx::angle::diff(yaw, u.motion.drive.head).abs() <= fx::angle::FULL * 36 / 360);
        if u.commander_face(target) { break; }
        previous = u.motion.drive.head;
    }
    assert_ne!(previous, 0, "the hull must turn towards a sideways target");
    assert!(u.commander_face(target), "the hull must reach the working sector");
}

#[test]
fn walker_long_stride_slows_cadence_without_changing_speed() {
    let walker = RangeUnit::of_kind(RangeUnitKind::Walker04, [400, 16, 400]);
    let gait = walker.motion.chassis.gait.unwrap();
    let mut original = walker.motion.chassis;
    original.gait.as_mut().unwrap().stride_cm = gait.stride_cm / 3;
    let long = crate::drive::StepCycle::of(&walker.motion.chassis).unwrap();
    let short = crate::drive::StepCycle::of(&original).unwrap();
    let travelled = Fx::from_int(10);
    let now = long.advance(travelled, 0);
    let before = short.advance(travelled, 0);
    assert!(now > 0 && (before - 3 * now).abs() <= 3);
    let strider = RangeUnit::of_kind(RangeUnitKind::Strider, [400, 16, 400]);
    assert_eq!(walker.motion.speed, (strider.motion.speed * 9 / 10).max(1));
}

#[test]
fn commander_moves_slower_and_tools_aim_gradually() {
    let mut commander = RangeUnit::of_kind(RangeUnitKind::Commander, [400, 16, 400]);
    let walker = RangeUnit::of_kind(RangeUnitKind::Walker04, [400, 16, 400]);
    assert!(commander.motion.speed * 3 <= walker.motion.speed);
    let target = [600, 80, 700];
    commander.set_arm_targets([target, target, [0, 0, 0], [0, 0, 0]], 0);
    let first = commander.arm_targets.unwrap()[0];
    assert_ne!(first, target);
    commander.set_arm_targets([target, target, [0, 0, 0], [0, 0, 0]], 0);
    let second = commander.arm_targets.unwrap()[0];
    let distance = |p: [i32; 3]| (0..3).map(|i| i64::from(p[i] - target[i]).pow(2)).sum::<i64>();
    assert!(distance(second) < distance(first));
    let construction_target = [200, 16, 200];
    commander.set_arm_targets([construction_target, construction_target, [1, 0, 0], [0, 0, 0]], 0);
    let changed = commander.arm_targets.unwrap();
    assert_ne!(changed[1], construction_target, "tool switch must retain inertia");
    assert_ne!(changed[0], second, "cannon must gradually relax after switching tools");
    assert!((0..3).all(|axis| (changed[0][axis] - second[axis]).abs() <= 80));
}

#[test]
fn commander_keeps_its_gun_forward_until_the_enemy_enters_range() {
    let mut r = arena_of([RangeUnitKind::Commander, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    std::sync::Arc::make_mut(&mut r.battle.tuning).energy_range = 100;
    let p = r.units[0].motion.pos.map(Fx::to_int_trunc);
    r.units[1].motion.pos = [p[0], p[1], p[2] + 300].map(Fx::from_int);
    let head = r.units[1].motion.drive.head;
    r.units[1].update_pose(head);
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    for _ in 0..5 { r.battle_tick(); }
    assert_eq!(r.battle.actors[0].turret.angle, r.units[0].motion.drive.head);
    assert_ne!(r.units[0].arm_targets.unwrap()[0], aim_point(&r.units[1]));
    assert_eq!(r.battle.actors[0].fired, 0);
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("commander pose restores");
    assert_eq!(r.units[0].arm_targets, restored.units[0].arm_targets);
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn commander_fires_energy_at_an_enemy() {
    let mut r = arena_of([RangeUnitKind::Commander, RangeUnitKind::Tank]);
    let origin = r.units[0].motion.pos.map(Fx::to_int_trunc);
    r.units[1].motion.pos = [origin[0] + 300, origin[1], origin[2]].map(Fx::from_int);
    let head = r.units[1].motion.drive.head;
    r.units[1].update_pose(head);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.trace_fire(true);
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    for _ in 0..180 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 { break; }
    }
    assert!(r.battle.actors[0].fired > 0, "commander never fired: gate {:?}, phase {:?}, status {:?}", r.battle.fire_gate(0), r.battle.phase, r.units[0].status);
    assert_eq!(r.units[0].arm_targets.unwrap()[2], [0, 0, 0]);
    let gun = r.units[0].seg_frames[13];
    let muzzle = r.battle_muzzles(0)[0];
    let target = aim_point(&r.units[1]);
    let delta = phys::V3::new(
        Fx::from_int(target[0]).sub(muzzle[0]),
        Fx::from_int(target[1]).sub(muzzle[1]),
        Fx::from_int(target[2]).sub(muzzle[2]),
    );
    assert!(gun.x.dot(delta) > delta.len_sq().sqrt().mul(Fx::from_ratio(95, 100)), "cannon points away from target");
}

#[test]
fn commander_battle_energy_hits_apply_real_damage_in_both_directions() {
    for kinds in [[RangeUnitKind::Commander, RangeUnitKind::Tank],
        [RangeUnitKind::Tank, RangeUnitKind::Commander],
        [RangeUnitKind::Commander, RangeUnitKind::Commander]] {
        let mut r = arena_of(kinds);
        r.set_physics_workers(1); r.set_navigation_workers(1);
        r.start_battle().unwrap(); r.battle.trace_fire(true);
        r.battle.actors[1].order = Order::Hold;
        if kinds[0] == RangeUnitKind::Tank {
            assert!(r.set_battle_weapon(0, Some(Weapon::Energy)));
        }
        assert!(r.battle_order(0, Order::Attack(1), None));
        let mut impacts = 0;
        let mut waves = 0;
        for _ in 0..240 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            impacts += r.battle.impacts.iter().filter(|i| i.weapon == Weapon::Energy).count();
            waves = waves.max(r.battle.waves.len());
            if r.units[1].annihilated_mm3() > 0 { break; }
        }
        assert!(r.units[1].annihilated_mm3() > 0,
            "no damage for {kinds:?}: fired {}, gate {:?}, impacts {impacts}, waves {waves}, target {:?}, muzzle {:?}, dents {}/{}, status {:?}",
            r.battle.actors[0].fired, r.battle.fire_gate(0), aim_point(&r.units[1]), r.battle_muzzles(0),
            r.units[1].dents.len(), r.units[1].segment_dents().len(), r.units[1].status);
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn repeated_same_move_does_not_keep_resetting_the_commanders_energy_charge() {
    let mut r = arena_of([RangeUnitKind::Commander, RangeUnitKind::Tank]);
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_battle().unwrap(); r.battle.trace_fire(true);
    r.battle.actors[1].order = Order::Hold;
    let destination = [340, 336];
    for _ in 0..180 {
        assert_eq!(r.battle_group_move(&[(0, destination)], destination), 1);
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 { break; }
    }
    assert!(r.battle.actors[0].fired > 0,
        "repeated move starved firing: gate {:?}, pos {:?}, charge {:?}",
        r.battle.fire_gate(0), r.units[0].motion.pos, r.battle.actors[0].charge);
}

#[test]
fn commander_battle_damage_still_applies_after_three_minutes_of_ground_fire() {
    let mut r = arena_of([RangeUnitKind::Commander, RangeUnitKind::Tank]);
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_battle().unwrap(); r.battle.trace_fire(true);
    r.battle.actors[1].order = Order::Hold;
    r.battle.actors[1].weapon = Some(Weapon::Energy);
    assert!(r.battle_order(0, Order::Ground([320, 15, 470]), None));
    for _ in 0..(crate::TICKS_PER_SECOND as u32 * 180) {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    assert!(r.battle.actors[0].fired > 5, "aging scene never fired");
    // Test each direction from the same aged state: the commander's first
    // successful shot can destroy the tank before it could return fire.
    let mut reversed = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    reversed.set_physics_workers(1); reversed.set_navigation_workers(1);
    reversed.battle.trace_fire(true);
    assert!(r.battle_order(0, Order::Attack(1), None));
    let before = r.units[1].annihilated_mm3();
    for _ in 0..300 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.units[1].annihilated_mm3() > before { break; }
    }
    assert!(r.units[1].annihilated_mm3() > before,
        "aged battle lost commander damage: tick {}, gate {:?}, shots {}, rounds {}, waves {}, held {:?}",
        r.tick, r.battle.fire_gate(0), r.battle.actors[0].fired, r.battle.rounds.len(), r.battle.waves.len(), r.battle.wave_held);
    r = reversed;
    assert!(r.battle_order(0, Order::Hold, None));
    assert!(r.economy_ai_order(1, Order::Attack(0), false));
    let before = r.units[0].annihilated_mm3();
    for _ in 0..300 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.units[0].annihilated_mm3() > before { break; }
    }
    assert!(r.units[0].annihilated_mm3() > before,
        "aged battle lost enemy damage: gate {:?}, shots {}, waves {}, held {:?}",
        r.battle.fire_gate(1), r.battle.actors[1].fired, r.battle.waves.len(), r.battle.wave_held);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn commander_mining_does_not_make_it_immune_to_an_enemy_energy_tank() {
    let base = arena_of([RangeUnitKind::Commander, RangeUnitKind::Tank]);
    let mut layout = base.layout.clone().unwrap();
    layout.size = [3200, 256, 3200];
    layout.clear_min = [1, 1]; layout.clear_max = [3199, 3199];
    layout.units[0].at = [1666, 16, 1666];
    layout.units[1].at = [2166, 16, 1666];
    layout.units[1].weapon = Some(modding::range::RangeLoadout::Energy);
    let mut economy = modding::range_economy::RangeEconomy::stock();
    economy.production = None;
    let commander = economy.kind_mut(RangeUnitKind::Commander).unwrap();
    commander.dig_radius = 12; commander.dig_edits_per_tick = 512;
    layout.economy = Some(economy);
    let mut r = BlastRange::with_layout(base.config, layout).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_battle().unwrap(); r.battle.trace_fire(true);
    assert!(r.economy_ai_order(1, Order::Attack(0), false));
    assert!(r.battle_order(0, Order::Dig([1620, 15, 1666]), None));
    let mut impacts = 0;
    for _ in 0..240 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        impacts += r.battle.impacts.iter().filter(|i| i.weapon == Weapon::Energy).count();
        if r.units[0].annihilated_mm3() > 0 { break; }
    }
    assert!(r.units[0].annihilated_mm3() > 0,
        "mining commander immune: shots {}, impacts {impacts}, waves {}, dents {}/{}, gate {:?}, target {:?}",
        r.battle.actors[1].fired, r.battle.waves.len(), r.units[0].dents.len(), r.units[0].segment_dents().len(),
        r.battle.fire_gate(1), aim_point(&r.units[0]));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn energy_tank_attacking_a_commander_seeks_a_clear_firing_line() {
    let base = arena_of([RangeUnitKind::Tank, RangeUnitKind::Commander]);
    let mut layout = base.layout.clone().unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [305, 16, 280], max: [325, 150, 392], material: 3,
    });
    let mut r = BlastRange::with_layout(base.config, layout).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1);
    r.start_battle().unwrap(); r.battle.trace_fire(true);
    r.battle.actors[1].order = Order::Hold;
    assert!(r.set_battle_weapon(0, Some(Weapon::Energy)));
    assert!(r.battle_order(0, Order::Attack(1), None));
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.battle_tick();
    assert_eq!(r.battle.fire_gate(0), Some(FireGate::Approach),
        "energy weapon aims through unbreakable cover instead of finding a firing position");
    assert!(r.units[0].motion.destination.is_some() || r.units[0].motion.searching());
    assert!(r.battle.rounds.is_empty());
    for _ in 0..360 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.units[1].annihilated_mm3() > 0 { break; }
    }
    assert!(r.units[1].annihilated_mm3() > 0,
        "detour never damaged commander: gate {:?}, pos {:?}, destination {:?}, shots {}",
        r.battle.fire_gate(0), r.units[0].at, r.units[0].motion.destination, r.battle.actors[0].fired);
    // Deliberately attacking terrain retains its physical impact behaviour.
    let destination = r.units[0].motion.destination;
    let fired = r.battle.actors[0].fired;
    assert!(r.battle_order(0, Order::Ground([310, 30, 336]), None));
    r.battle.actors[0].next = r.tick;
    for _ in 0..100 {
        r.tick += 1;
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.battle_tick();
        if r.battle.actors[0].fired > fired { break; }
    }
    assert!(r.battle.actors[0].fired > fired, "explicit ground fire was blocked");
    assert_eq!(r.units[0].motion.destination, destination);
}

#[test]
fn projectile_impact_events_keep_the_incident_side_of_a_tall_wall() {
    for weapon in [
        Weapon::Blast,
        Weapon::Canister,
        Weapon::Siege,
        Weapon::Energy,
        Weapon::Annihilation,
        Weapon::Kinetic,
    ] {
        let mut r = arena_of([RangeUnitKind::Tank; 2]);
        for y in 50..110 {
            r.world.queue_edit(voxel::Edit::put(300, y, 336, 3));
        }
        while r.world.queued() > 0 {
            r.world.flush();
        }
        let mut b = Battle::new(&r.units, 180);
        b.rounds.push(Round {
            owner: 0,
            weapon,
            pos: [Fx::from_int(299), Fx::from_int(80), Fx::from_int(336)],
            previous: [Fx::from_int(299), Fx::from_int(80), Fx::from_int(336)],
            velocity: [Fx::from_int(300), Fx::ZERO, Fx::ZERO],
            distance: Fx::from_int(100),
        });
        r.battle_flight(&mut b);
        assert_eq!(b.impacts.len(), 1, "{weapon:?} lost its contact");
        let impact = b.impacts[0];
        assert_eq!(impact.at[0], 300);
        assert!(
            impact.from[0] < impact.at[0],
            "{weapon:?} lost the incident side"
        );
        assert_eq!(material(&r.world, impact.from), 0);
        // Events are transient presentation data, not a new saved SIM state.
        r.battle = b;
        let hash = r.state_hash();
        let bytes = r.snapshot();
        r.battle.impacts[0].from = [0; 3];
        assert_eq!(r.state_hash(), hash);
        assert_eq!(r.snapshot(), bytes);
    }
}

fn canister_salvo() -> BlastRange {
    let mut r = arena_of([RangeUnitKind::ArtilleryT2, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    assert!(r.battle_order(0, Order::Attack(1), None));
    for _ in 0..160 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert_eq!(r.battle.actors[0].fired, 1, "T2 never fired");
    r
}

#[test]
fn canister_fires_a_deterministic_lofted_salvo_and_replays_area_bursts() {
    let mut r = canister_salvo();
    assert_eq!(r.battle_weapon(0), Some(Weapon::Canister));
    assert_eq!(
        r.battle_attack_range(0),
        Some(r.battle.tuning.canister.range)
    );
    assert!(!r.set_battle_weapon(0, Some(Weapon::Blast)));
    assert!(!r.set_battle_weapon(1, Some(Weapon::Canister)));
    assert_eq!(r.battle.rounds.len(), r.battle.tuning.canister.pellets);
    let velocities: std::collections::BTreeSet<_> = r
        .battle
        .rounds
        .iter()
        .map(|p| {
            assert_eq!(p.owner, 0);
            assert_eq!(p.weapon, Weapon::Canister);
            p.velocity.map(|v| v.0)
        })
        .collect();
    assert_eq!(velocities.len(), r.battle.tuning.canister.pellets);
    assert_eq!(r.state_hash(), canister_salvo().state_hash());
    assert_eq!(
        r.battle.actors[0].next,
        r.tick - 1 + r.battle.tuning.canister.reload_ticks
    );
    assert!(r.battle_order(0, Order::Hold, None));
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).expect("T2 salvo snapshot");
    let before = r.units[1].body.volume_mm3();
    let mut contacts = 0;
    let mut area_burst = false;
    for _ in 0..240 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        replay.battle.ai.next_tick = replay.tick + replay.battle.ai.tuning.decision_ticks;
        replay.step();
        assert_eq!(r.state_hash(), replay.state_hash());
        contacts += r
            .battle
            .impacts
            .iter()
            .filter(|i| i.weapon == Weapon::Canister)
            .count();
        area_burst |= !r.battle.waves.is_empty();
    }
    assert!(contacts > 1, "pellets never made separate contacts");
    assert!(area_burst, "explosive submunitions produced no splash");
    assert!(
        r.units[1].body.volume_mm3() < before,
        "canister caused no material damage"
    );
    assert!(r.matter.economy.ledger.balanced());
    let mut legacy = r.snapshot();
    legacy[8..12].copy_from_slice(&47u32.to_le_bytes());
    assert!(BlastRange::restore(r.config, &legacy).is_none());
    let old = arena(RangeUnitKind::Tank);
    let mut legacy = old.snapshot();
    legacy[8..12].copy_from_slice(&47u32.to_le_bytes());
    assert!(BlastRange::restore(old.config, &legacy).is_some());
}

#[test]
fn canister_capacity_defers_the_whole_salvo_then_allows_reload_and_movement() {
    let mut r = arena_of([RangeUnitKind::ArtilleryT2, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    std::sync::Arc::make_mut(&mut r.battle.tuning).max_rounds =
        r.battle.tuning.canister.pellets - 1;
    assert!(r.battle_order(0, Order::Attack(1), None));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    assert_eq!(r.battle.actors[0].fired, 0);
    assert!(r.battle.rounds.is_empty());
    std::sync::Arc::make_mut(&mut r.battle.tuning).max_rounds = r.battle.tuning.canister.pellets;
    r.step();
    assert_eq!(r.battle.actors[0].fired, 1);
    assert_eq!(r.battle.rounds.len(), r.battle.tuning.canister.pellets);
    let start = r.units[0].at;
    assert!(r.battle_order(0, Order::Move, Some([260, 400])));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    assert_ne!(r.units[0].at, start, "T2 is not mobile");
}

#[test]
fn canister_reserves_wave_capacity_for_the_entire_salvo() {
    let mut r = arena_of([RangeUnitKind::ArtilleryT2, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    // A side owns half of the ceiling (ADR-649): one slot short of a salvo.
    std::sync::Arc::make_mut(&mut r.battle.tuning).max_waves =
        2 * r.battle.tuning.canister.pellets - 1;
    assert!(r.battle_order(0, Order::Attack(1), None));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    assert_eq!(r.battle.actors[0].fired, 0);
    assert!(r.battle.rounds.is_empty());
    std::sync::Arc::make_mut(&mut r.battle.tuning).max_waves += 1;
    r.step();
    assert_eq!(r.battle.actors[0].fired, 1);
    assert_eq!(r.battle.rounds.len(), r.battle.tuning.canister.pellets);
    assert_eq!(r.pending_kinetic_impacts(), 0);
    assert!(r.battle_order(0, Order::Hold, None));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn autonomous_salvo_bypasses_saturated_round_and_wave_admission_and_restores() {
    let mut r = arena_of([RangeUnitKind::ArtilleryT2, RangeUnitKind::Tank]);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    assert!(r.set_ai_vs_ai(true));
    r.start_battle().unwrap();
    r.battle.actors[0].order = Order::Attack(1);
    r.battle.actors[1].order = Order::Hold;
    let tuning = std::sync::Arc::make_mut(&mut r.battle.tuning);
    tuning.max_rounds = 1;
    tuning.max_waves = 1;
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert_eq!(
        r.battle.actors[0].fired, 1,
        "AI salvo stopped at a full admission budget"
    );
    assert_eq!(r.battle.rounds.len(), 6);
    // Exercise a queue beyond the stock 64 rounds and 16 wave slots, without
    // needing a long-running full-map battle in this regression test.
    while r.battle.rounds.len() < 66 {
        let round = &r.battle.rounds[0];
        let copy = Round {
            owner: round.owner,
            weapon: round.weapon,
            pos: round.pos,
            previous: round.previous,
            velocity: round.velocity,
            distance: round.distance,
        };
        r.battle.rounds.push(copy);
    }
    let restored =
        BlastRange::restore(r.config, &r.snapshot()).expect("restore over-cap AI rounds");
    assert_eq!(restored.battle.rounds.len(), 66);
    assert!(r.battle_order(0, Order::Hold, None));
    assert!(!r.battle.autonomous);
    let restored = BlastRange::restore(r.config, &r.snapshot())
        .expect("restore accepted rounds after manual takeover");
    assert_eq!(restored.battle.rounds.len(), 66);
}

fn arena(kind: RangeUnitKind) -> BlastRange {
    arena_of([kind, kind])
}

fn canister_ground_salvo(layout: RangeLayout) -> BlastRange {
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    let target = aim_point(&r.units[1]);
    assert!(r.battle_order(0, Order::Ground(target), None));
    for _ in 0..160 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert_eq!(r.battle.actors[0].fired, 1);
    assert_eq!(r.battle.rounds.len(), r.battle.tuning.canister.pellets);
    assert!(r.battle_order(0, Order::Hold, None));
    r
}

#[test]
fn canister_pellets_stop_at_thick_steel_cover_without_damaging_the_hidden_tank() {
    let mut layout = arena_of([RangeUnitKind::ArtilleryT2, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [300, 16, 240],
        // A physically elevated muzzle can loft above the former 96-high
        // wall. Full-height cover isolates interception, not low-cover arcs.
        max: [332, layout.size[1] as i32, 432],
        material: 3,
    });
    let mut r = canister_ground_salvo(layout);
    let before = r.units[1].body.volume_mm3();
    let mut contacts = 0;
    for _ in 0..160 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        for impact in &r.battle.impacts {
            assert_eq!(impact.weapon, Weapon::Canister);
            assert!((296..=336).contains(&impact.at[0]), "missed steel cover");
            contacts += 1;
        }
        if r.battle.rounds.is_empty() && r.battle.waves.is_empty() {
            break;
        }
    }
    assert!(r.battle.rounds.is_empty(), "pellets did not resolve");
    assert!(contacts > 1, "no independent cover contacts");
    assert_eq!(r.units[1].body.volume_mm3(), before);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn canister_shield_intercepts_individual_pellets_and_replays_after_overload() {
    let mut layout = arena_of([RangeUnitKind::ArtilleryT2, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.shields.push(modding::range::RangeShieldDef {
        at: [450, 32, 336],
        radius: 80,
    });
    let mut r = canister_ground_salvo(layout);
    let before = r.units[1].body.volume_mm3();
    let mut intercepted = false;
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.shields[0].hits > 0 {
            intercepted = true;
            break;
        }
    }
    assert!(intercepted, "the dome never intercepted a pellet");
    assert!(r.shields[0].charge < r.shield_tuning.capacity);
    assert_eq!(r.units[1].body.volume_mm3(), before);
    assert!(
        r.battle.rounds.iter().filter(|p| p.owner == 0).count() < r.battle.tuning.canister.pellets
    );
    assert!(
        !r.battle.rounds.is_empty(),
        "one shield hit erased the entire fan"
    );
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..30 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        replay.battle.ai.next_tick = replay.tick + replay.battle.ai.tuning.decision_ticks;
        replay.step();
        assert_eq!(r.state_hash(), replay.state_hash());
    }
    assert!(r.matter.economy.ledger.balanced());
}

/// An allied unit of the first kind facing an enemy of the second.
fn arena_of(kinds: [RangeUnitKind; 2]) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.size = [640, 192, 640];
    l.blocks.clear();
    l.shields.clear();
    l.course = None;
    l.clear_min = [64, 64];
    l.clear_max = [600, 600];
    l.units = vec![
        RangeUnitDef {
            weapon: None,
            kind: kinds[0],
            at: [170, 16, 336],
            allied: true,
        },
        RangeUnitDef {
            weapon: None,
            kind: kinds[1],
            at: [450, 16, 336],
            allied: false,
        },
    ];
    if kinds[0] == RangeUnitKind::Walker04 {
        l.size = [2400, 768, 1600];
        l.clear_min = [64, 64];
        l.clear_max = [2300, 1500];
        l.units[0].at = [500, 16, 800];
        l.units[1].at = [1500, 16, 800];
    }
    if matches!(
        kinds[0],
        RangeUnitKind::KilFactoryT1
            | RangeUnitKind::KilFactoryT2
            | RangeUnitKind::KilFactoryT3
            | RangeUnitKind::KilFactoryT4
    ) {
        l.size = [1600, 512, 1600];
        l.clear_min = [64, 64];
        l.clear_max = [1536, 1536];
        l.units[0].at = [500, 16, 800];
        l.units[1].at = [1300, 16, 800];
    }
    if l.units.iter().any(|def| {
        let (lo, hi) = RangeUnit::of_kind(def.kind, def.at).bounds();
        (0..3).any(|a| lo[a] < 1 || hi[a] >= l.size[a] as i32 - 1)
    }) {
        l.size = [4096, 2048, 4096];
        l.clear_max = [4032, 4032];
        l.units[0].at = [1200, 16, 1600];
        l.units[1].at = [3000, 16, 1600];
    }
    BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        l,
    )
    .unwrap()
}

fn walker_arena(order: Order) -> BlastRange {
    let mut layout = arena_of([RangeUnitKind::Walker04, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units[1].at = [1500, 16, 600];
    layout.units.extend([
        RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [1500, 16, 733],
            allied: false,
            weapon: None,
        },
        RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [1500, 16, 866],
            allied: false,
            weapon: None,
        },
        RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [1500, 16, 1000],
            allied: false,
            weapon: None,
        },
    ]);
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    for actor in &mut r.battle.actors[1..] {
        actor.order = Order::Hold;
    }
    r.battle.actors[0].order = order;
    r.battle.actors[0].target = Some(1);
    r
}

fn step_until_walker_beams(r: &mut BlastRange) {
    for _ in 0..180 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if !r.battle.walker_lasers.is_empty() {
            return;
        }
    }
    panic!("Walker 04 never fired");
}

#[test]
fn walker_aims_during_reload_instead_of_waiting_to_start() {
    let mut r = walker_arena(Order::Attack(1));
    r.battle.actors[0].next = r.tick + 100;
    r.battle.ai.next_tick = r.tick + 200;
    r.step();
    assert!(r.units[0].arm_targets.is_some(), "arms did not begin tracking during reload");
    assert!(r.battle.walker_lasers.is_empty(), "fired during reload");
}

#[test]
fn walker_does_not_fire_at_a_ground_point() {
    let mut r = walker_arena(Order::Ground([1500, 0, 800]));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + 200;
        r.step();
        assert!(r.battle.walker_lasers.is_empty(), "fired at ground");
    }
    assert_eq!(r.battle.actors[0].fired, 0);
}

#[test]
fn walker04_forearms_do_not_intersect_in_forward_working_sectors() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Walker04, [600, 16, 600]);
    for target in [[1400, 100, 600], [1400, 80, 300], [1400, 200, 900]] {
        for _ in 0..80 { u.set_arm_targets([target; 4], 0); }
        for a in 0..4 {
            for ai in [12 + a * 4, 13 + a * 4] {
                let body = &u.body.segments[ai].body;
                for z in 0..usize::from(body.vol.nz) { for y in 0..usize::from(body.vol.ny) { for x in 0..usize::from(body.vol.nx) {
                    if !body.vol.get(x, y, z) { continue; }
                    let world = u.seg_frames[ai].world([Fx::from_ratio(x as i32 * 2 + 1, 2), Fx::from_ratio(y as i32 * 2 + 1, 2), Fx::from_ratio(z as i32 * 2 + 1, 2)]);
                    let root = u.pose.local(world).map(Fx::to_int_trunc);
                    if root[0] >= 0 && root[1] >= 0 && root[2] >= 0 && root[0] < i32::from(u.body.vol.nx) && root[1] < i32::from(u.body.vol.ny) && root[2] < i32::from(u.body.vol.nz) {
                        assert!(!u.body.vol.get(root[0] as usize, root[1] as usize, root[2] as usize), "arm {a} intersects the torso while aiming at {target:?}");
                    }
                    for b in (a + 1)..4 { for bi in [12 + b * 4, 13 + b * 4] {
                        let p = u.seg_frames[bi].local(world).map(Fx::to_int_trunc);
                        let other = &u.body.segments[bi].body;
                        if p[0] >= 0 && p[1] >= 0 && p[2] >= 0 && p[0] < i32::from(other.vol.nx) && p[1] < i32::from(other.vol.ny) && p[2] < i32::from(other.vol.nz) {
                            assert!(!other.vol.get(p[0] as usize, p[1] as usize, p[2] as usize), "arms {a} and {b} overlap while aiming at {target:?}");
                        }
                    }}
                }}}
            }
        }
    }
}

#[test]
fn walker04_has_four_separate_matching_bearing_modules() {
    let u = RangeUnit::of_kind(RangeUnitKind::Walker04, [600, 16, 600]);
    let mounts: Vec<_> = (0..4).map(|arm| u.body.segments[10 + arm * 4].attach.at).collect();
    assert_eq!(u.body.segments.len(), 30);
    for arm in 0..4 {
        assert_eq!(mounts[arm].1, u.body.segments[26 + arm].attach.at.1);
        if arm > 0 { assert!(mounts[arm].1 > mounts[arm - 1].1); }
        assert!(!u.body.segments[26 + arm].attach.turret);
    }
    for part in 0..4 {
        let reference = &u.body.segments[10 + part].body.vol;
        for arm in 1..4 {
            let other = &u.body.segments[10 + arm * 4 + part].body.vol;
            assert_eq!((reference.nx, reference.ny, reference.nz), (other.nx, other.ny, other.nz));
            for z in 0..usize::from(reference.nz) { for y in 0..usize::from(reference.ny) { for x in 0..usize::from(reference.nx) {
                assert_eq!(reference.get(x, y, z), other.get(x, y, z), "arm {arm}, part {part}");
            }}}
        }
    }
}

#[test]
fn walker04_independent_bearings_aim_in_every_direction() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Walker04, [600, 16, 600]);
    for yaw in [0, fx::angle::FULL / 4, fx::angle::FULL / 2, fx::angle::FULL * 3 / 4] {
        let target = [600 + fx::angle::cos(yaw).mul(Fx::from_int(800)).to_int_trunc(),
            70, 600 + fx::angle::sin(yaw).mul(Fx::from_int(800)).to_int_trunc()];
        for _ in 0..100 { u.set_arm_targets([target; 4], yaw); }
        for arm in 0..4 {
            let frame = u.seg_frames[13 + arm * 4];
            let muzzle = frame.world(frame.centre);
            let dx = Fx::from_int(target[0]).sub(muzzle.x);
            let dz = Fx::from_int(target[2]).sub(muzzle.z);
            let bearing = fx::angle::atan2(dz.raw(), dx.raw());
            let actual = fx::angle::atan2(frame.x.z.raw(), frame.x.x.raw());
            assert!(fx::angle::diff(bearing, actual).abs() < fx::angle::FULL * 4 / 360,
                "arm {arm} failed to aim at yaw {yaw}");
        }
    }
}

#[test]
fn walker04_focus_pitch_keeps_feet_planted_and_shoulders_attached() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Walker04, [600, 16, 600]);
    let rest = u.seg_frames.clone();
    let feet = [rest[4].world(rest[4].centre), rest[9].world(rest[9].centre)];
    u.turret_pitch = -fx::angle::FULL * 8 / 360;
    for _ in 0..80 { u.set_arm_targets([[1400, 50, 600]; 4], 0); }
    for (i, foot) in [4, 9].into_iter().enumerate() {
        assert_eq!(u.seg_frames[foot].world(u.seg_frames[foot].centre), feet[i]);
    }
    for arm in 0..4 {
        let index = 10 + arm * 4;
        let attach = u.body.segments[index].attach;
        let offset = [attach.at.0 - 40, 0, attach.at.2 - 40].map(|v| Fx::from_int(i32::from(v)));
        let mount = u.seg_frames[26 + arm];
        let expected = mount.base.add(mount.x.scale(offset[0].div(mount.scale)))
            .add(mount.z.scale(offset[2].div(mount.scale)));
        assert!(u.seg_frames[index].world(u.seg_frames[index].centre)
            .sub(expected).len_sq() < Fx::from_ratio(1, 1000),
            "arm {arm} separated from its socket");
    }
    for (arm, muzzle) in weapons::walker_muzzles(&u) {
        let delta = phys::V3::new(Fx::from_int(1400).sub(muzzle[0]),
            Fx::from_int(50).sub(muzzle[1]), Fx::from_int(600).sub(muzzle[2]));
        assert!(u.seg_frames[13 + arm * 4].x.dot(delta).div(delta.len_sq().sqrt())
            > Fx::from_ratio(998, 1000), "arm {arm} misses its focused target");
    }
}

#[test]
fn walker04_one_bearing_can_turn_without_dragging_the_other_three() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Walker04, [600, 16, 600]);
    let original = [[1400, 170, 600]; 4];
    for _ in 0..160 { u.set_arm_targets(original, 0); }
    let before = u.seg_frames.clone();
    let mut targets = original;
    targets[2] = [600, 170, 1400];
    for _ in 0..160 { u.set_arm_targets(targets, 0); }
    for arm in [0, 1, 3] {
        let before = before[26 + arm]; let after = u.seg_frames[26 + arm];
        assert!(before.x.dot(after.x) > Fx::from_ratio(999, 1000), "uncommanded bearing {arm} moved");
    }
    assert!(before[28].x.dot(u.seg_frames[28].x).abs() < Fx::from_ratio(1, 3), "bearing did not orbit the body's axis");
    assert_eq!(u.motion.drive.head, 0, "aim dragged the chassis");
    u.shed(28);
    assert!(u.seg_gone[18..22].iter().all(|gone| *gone), "destroyed bearing retained its arm");
    assert!(u.seg_gone[10..18].iter().all(|gone| !*gone), "other bearings lost their arms");
}

#[test]
fn walker04_lost_arm_does_not_renumber_the_remaining_guns() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Walker04, [300, 16, 300]);
    u.set_arm_targets(
        [
            [1000, 100, 300],
            [300, 100, 1000],
            [100, 100, 300],
            [300, 100, 100],
        ],
        0,
    );
    let original = weapons::walker_muzzles(&u);
    assert_eq!(original.len(), 4);
    u.seg_gone[13] = true;
    assert_eq!(weapons::walker_muzzles(&u), original[1..]);
    u.seg_gone[10 + 2 * 4] = true;
    assert_eq!(weapons::walker_muzzles(&u), vec![original[1], original[3]]);
}

#[test]
fn walker04_aiming_keeps_each_arm_attached_to_its_own_bearing() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Walker04, [300, 16, 300]);
    let rest = u.seg_frames.clone();
    let shoulder = |u: &RangeUnit, arm: usize| {
        let f = u.seg_frames[10 + arm * 4];
        f.world(f.centre)
    };
    let shoulders: Vec<_> = (0..4).map(|arm| shoulder(&u, arm)).collect();
    u.set_arm_targets(
        [
            [1000, 80, 300],
            [300, 100, 1000],
            [100, 60, 300],
            [300, 180, 100],
        ],
        0,
    );
    for arm in 0..4 {
        let mount = u.seg_frames[26 + arm];
        let old_mount = rest[26 + arm];
        let local = old_mount.local(shoulders[arm]);
        assert!(mount.world(local).sub(shoulder(&u, arm)).len_sq() < Fx::from_ratio(1, 1000),
            "arm {arm} left its rotating bearing");
        let start = 10 + arm * 4;
        for (parent, child) in [(start, start + 1), (start + 2, start + 3)] {
            let joint = rest[child].world(rest[child].centre);
            let local = rest[parent].local(joint);
            let actual = u.seg_frames[child].world(u.seg_frames[child].centre);
            assert!(
                u.seg_frames[parent].world(local).sub(actual).len_sq() < Fx::from_ratio(1, 1000),
                "arm {arm} separated at segment {child}"
            );
        }
    }
    assert!((55..80).all(|x| (0..u.body.vol.ny as usize).all(|y| (0..80).all(|z| !u.body.vol.get(x,y,z)))),
        "fixed root contains ghost arm rails beyond the shoulder assembly");
}

#[test]
fn walker04_aims_four_independent_arms_at_four_enemies_without_moving_their_mounts() {
    let mut r = walker_arena(Order::Auto);
    let mounts: Vec<_> = (0..4)
        .map(|arm| r.units[0].body.segments[10 + arm * 4].attach.at)
        .collect();
    step_until_walker_beams(&mut r);
    assert!(!r.battle.walker_lasers.is_empty());
    assert_eq!(r.units[0].turret_pitch, 0);
    let targets = r.units[0].arm_targets.expect("four independent targets");
    let unique: std::collections::BTreeSet<_> = targets.into_iter().collect();
    assert_eq!(unique.len(), 4);
    // Slower arms can still be in transit when the first arm fires.
    assert!(targets.iter().any(|target| (1..r.units.len()).any(|id|
        (0..3).all(|axis| (aim_point(&r.units[id])[axis] - target[axis]).abs() <= 20))));
    assert!(r.battle.walker_lasers.iter().all(|trace| trace.hit), "ready arms should not fire into empty space");
    let origins: std::collections::BTreeSet<_> = r
        .battle
        .walker_lasers
        .iter()
        .map(|trace| trace.from.map(|v| v.raw()))
        .collect();
    assert_eq!(origins.len(), r.battle.walker_lasers.len());
    assert_eq!(r.battle_muzzles(0).len(), 4);
    for (arm, mount) in mounts.into_iter().enumerate() {
        assert_eq!(r.units[0].body.segments[10 + arm * 4].attach.at, mount);
    }
    assert!(r.units[1..].iter().any(|unit| unit.revision > 0));
}

#[test]
fn walker04_focus_aims_separate_mounts_and_preserves_aim_on_restore() {
    let mut r = walker_arena(Order::Attack(1));
    for _ in 0..180 {
        let old = r.battle.actors[0].elevation;
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        let new = r.battle.actors[0].elevation;
        assert!((new.rate - old.rate).abs() <= r.turret_limits.accel);
        assert!(new.rate.abs() <= r.turret_limits.speed);
        if !r.battle.walker_lasers.is_empty() { break; }
    }
    assert!(!r.battle.walker_lasers.is_empty(), "pitch {} goals {:?} muzzles {:?} axes {:?}", r.units[0].turret_pitch, r.units[0].arm_targets, weapons::walker_muzzles(&r.units[0]), (0..4).map(|arm| r.units[0].seg_frames[13 + arm * 4].x).collect::<Vec<_>>());
    assert_eq!(r.units[0].turret_pitch, 0, "independent bearings must not pitch the central spine");
    let target = aim_point(&r.units[1]);
    assert!(r.units[0].arm_targets.unwrap().iter().any(|p| (0..3).all(|axis| (p[axis] - target[axis]).abs() <= 20)));
    assert_eq!(r.battle_weapon(0), Some(Weapon::Laser));
    assert_eq!(
        r.battle_attack_range(0),
        Some(motion::spider_tuning().laser.range)
    );
    assert!(
        r.units[1].revision > 0,
        "focused beams did no physical work: traces {:?}, target {:?}, turret {}, axes {:?}", r.battle.walker_lasers, target, r.battle.actors[0].turret.angle, (0..4).map(|arm| r.units[0].seg_frames[13 + arm * 4].x).collect::<Vec<_>>()
    );
    let before = r.battle_muzzles(0);
    let bytes = r.snapshot();
    let mut stage = String::new();
    let mut copy = BlastRange::restore_observed(r.config, &bytes, |section, offset, unit| {
        stage = format!("{section} at {offset} unit={unit:?}")
    })
    .unwrap_or_else(|| panic!("focused Walker 04 restore refused at {stage}"));
    assert_eq!(copy.snapshot(), bytes);
    assert_eq!(copy.battle_muzzles(0), before);
    assert!(BlastRange::restore(r.config, &bytes[..bytes.len() - 1]).is_none());
    let mut invalid = bytes.clone();
    let last = invalid.len() - 4;
    invalid[last..].copy_from_slice(&i32::MAX.to_le_bytes());
    assert!(BlastRange::restore(r.config, &invalid).is_none());
    for _ in 0..12 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        copy.battle.ai.next_tick = copy.tick + copy.battle.ai.tuning.decision_ticks;
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    r.battle.actors[0].order = Order::Hold;
    for _ in 0..30 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    assert!(r.units[0].turret_pitch.abs() < fx::angle::FULL / 360,
        "crown did not return to rest: {}", r.units[0].turret_pitch);
}

#[test]
fn razlom_fires_seven_parallel_rays_and_does_not_replace_the_old_t3_artillery() {
    let mut r = arena_of([RangeUnitKind::RazlomT3, RangeUnitKind::Tank]);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    for _ in 0..180 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if !r.battle.razlom_lasers.is_empty() {
            break;
        }
    }
    assert_eq!(r.battle.razlom_lasers.len(), 7);
    assert!(r.battle.lasers.is_empty());
    let direction = |trace: &weapons::Trace| {
        let delta = std::array::from_fn::<_, 3, _>(|axis| trace.to[axis].sub(trace.from[axis]));
        let length = delta
            .iter()
            .fold(0.0f64, |sum, v| {
                let v = v.to_f64_lossy();
                sum + v * v
            })
            .sqrt();
        delta.map(|v| v.to_f64_lossy() / length)
    };
    let central = direction(&r.battle.razlom_lasers[3]);
    for trace in &r.battle.razlom_lasers {
        let other = direction(trace);
        let dot: f64 = (0..3).map(|axis| central[axis] * other[axis]).sum();
        assert!(
            dot > 0.999,
            "rays converged instead of forming a front: {dot}"
        );
    }
    let old = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    assert_eq!(old.battle_weapon(0), Some(Weapon::Siege));
    assert_eq!(r.battle_weapon(0), Some(Weapon::Laser));
}

#[test]
fn razlom_wave_kills_t1_keeps_travelling_and_restores_exactly() {
    let mut r = arena_of([RangeUnitKind::RazlomT3, RangeUnitKind::Tank]);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    let mut restored = None;
    let mut reset = false;
    let mut beyond = false;
    for _ in 0..600 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if restored.is_none() && !r.battle.rounds.is_empty() {
            let mut replay = BlastRange::restore(r.config, &r.snapshot()).expect("wave snapshot");
            for point in [[224, 56, 333], [224, 55, 333], [254, 51, 333]] {
                assert_eq!(
                    r.units[1].solid(point),
                    replay.units[1].solid(point),
                    "contact changed at {point:?}"
                );
            }
            assert_eq!(
                r.state_hash(),
                replay.state_hash(),
                "state changed by wave snapshot"
            );
            let snapshot = r.snapshot();
            let from = r.battle.rounds[0].pos;
            let target = std::array::from_fn(|axis| {
                from[axis]
                    .add(
                        r.battle.rounds[0].velocity[axis]
                            .mul(Fx::from_ratio(1, crate::TICKS_PER_SECOND)),
                    )
                    .to_int_trunc()
            });
            let mut tuning = motion::spider_tuning().laser.clone();
            tuning.range = 31;
            let config = r.battle.laser_config;
            let actual = r.laser_ray(
                from,
                target,
                Some(0),
                &tuning,
                &config,
                (&mut |_| {}, &|_, _| {}),
            );
            let expected = replay.laser_ray(
                from,
                target,
                Some(0),
                &tuning,
                &config,
                (&mut |_| {}, &|_, _| {}),
            );
            assert_eq!(
                actual.to, expected.to,
                "same saved ray produced different contacts"
            );
            r = BlastRange::restore(r.config, &snapshot).unwrap();
            r.set_physics_workers(1);
            r.set_navigation_workers(1);
            replay = BlastRange::restore(replay.config, &snapshot).unwrap();
            replay.set_physics_workers(1);
            replay.set_navigation_workers(1);
            restored = Some(replay);
            reset = true;
        }
        if let Some(replay) = &mut restored {
            if reset {
                r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
                r.step();
                reset = false;
            }
            replay.battle.ai.next_tick = replay.tick + replay.battle.ai.tuning.decision_ticks;
            replay.step();
            assert_eq!(r.tick, replay.tick);
            assert_eq!(r.units[1].motion.pos, replay.units[1].motion.pos);
            assert_eq!(r.units[1].pose, replay.units[1].pose);
            assert_eq!(r.units[1].turret_delta, replay.units[1].turret_delta);
            assert_eq!(r.units[1].seg_frames, replay.units[1].seg_frames);
            assert_eq!(r.battle.rounds.len(), replay.battle.rounds.len());
            let differences: Vec<_> = r
                .battle
                .rounds
                .iter()
                .zip(&replay.battle.rounds)
                .map(|(a, b)| {
                    (
                        a.pos.map(|v| v.raw()),
                        b.pos.map(|v| v.raw()),
                        a.distance.raw(),
                        b.distance.raw(),
                    )
                })
                .collect();
            assert!(
                r.battle
                    .rounds
                    .iter()
                    .zip(&replay.battle.rounds)
                    .all(|(a, b)| {
                        a.owner == b.owner
                            && a.weapon == b.weapon
                            && a.pos == b.pos
                            && a.velocity == b.velocity
                            && a.distance == b.distance
                    }),
                "{differences:?}"
            );
            assert_eq!(r.state_hash(), replay.state_hash());
        }
        beyond |= r
            .battle
            .razlom_lasers
            .iter()
            .any(|trace| trace.to[0] > Fx::from_int(460));
        if !r.units[1].status.alive && beyond {
            break;
        }
    }
    if let Some(trace) = r.battle.razlom_lasers.get(3) {
        let at = trace.to.map(Fx::to_int_trunc);
        eprintln!(
            "wave end={at:?} material={} from={:?} to={:?}",
            material(&r.world, at),
            trace.from,
            trace.to
        );
    }
    if let Some(trace) = r.battle.razlom_lasers.get(3) {
        eprintln!("last trace hit={} serial={}", trace.hit, trace.serial);
    }
    for round in &r.battle.rounds {
        eprintln!(
            "active wave owner={} pos={:?} velocity={:?} distance={:?}",
            round.owner, round.pos, round.velocity, round.distance
        );
    }
    eprintln!(
        "target alive={} core={:?} removed={} wave rounds={} phase={:?}",
        r.units[1].status.alive,
        r.units[1].integrity.core,
        r.units[1].removed.len(),
        r.battle.rounds.len(),
        r.battle.phase
    );
    assert!(!r.units[1].status.alive, "T1 survived the wave");
    assert!(beyond, "wave died with its first target");
    assert!(restored.is_some(), "no persistent wave was produced");
}

#[test]
fn razlom_wave_carves_ground_and_fades_on_dense_cover() {
    let mut layout = arena_of([RangeUnitKind::RazlomT3, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.size[1] = 256;
    layout.units[1].at = [600, 16, 336];
    layout.blocks.push(modding::range::RangeBlock {
        min: [0, 0, 0],
        max: [640, 256, 640],
        material: 0,
    });
    layout.blocks.push(modding::range::RangeBlock {
        min: [0, 0, 0],
        max: [640, 16, 640],
        material: 1,
    });
    layout.blocks.push(modding::range::RangeBlock {
        min: [300, 16, 300],
        max: [520, 128, 372],
        material: 3,
    });
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Ground([600, 16, 336]), None));
    let mut fired = false;
    for _ in 0..600 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        fired |= !r.battle.rounds.is_empty() || !r.battle.razlom_lasers.is_empty();
        if fired && r.battle.rounds.is_empty() {
            break;
        }
    }
    let carved = (300..320).any(|x| (16..=20).any(|y| (300..372).any(|z| !r.world.solid(x, y, z))));
    assert!(carved, "wave did not carve the ground");
    assert!(
        r.battle.rounds.is_empty(),
        "dense cover did not dissipate the wave"
    );
    assert!(
        r.units[1].status.alive,
        "dissipated wave reached the distant target"
    );
}

#[test]
fn razlom_single_front_opens_a_continuous_passage_and_visibly_loses_strength() {
    let mut layout = arena_of([RangeUnitKind::RazlomT3, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.size[1] = 256;
    layout.idle_enemies = true;
    layout.units[1].at = [600, 16, 336];
    layout.blocks.push(modding::range::RangeBlock {
        min: [0, 0, 0],
        max: [640, 256, 640],
        material: 0,
    });
    layout.blocks.push(modding::range::RangeBlock {
        min: [0, 0, 0],
        max: [640, 16, 640],
        material: 1,
    });
    layout.blocks.push(modding::range::RangeBlock {
        min: [300, 16, 280],
        max: [312, 48, 392],
        material: 2,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    r.battle_order(0, Order::Ground([600, 32, 336]), None);
    let mut fired = false;
    let mut faded = false;
    for _ in 0..400 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if !r.battle.razlom_lasers.is_empty() {
            fired = true;
            faded |= r
                .battle
                .razlom_lasers
                .iter()
                .any(|t| t.strength < Fx::from_ratio(9, 10));
            r.battle_order(0, Order::Hold, None);
        }
        if fired && r.battle.rounds.is_empty() {
            break;
        }
    }
    assert!(fired && faded && r.battle.rounds.is_empty());
    let removed = (300..312)
        .flat_map(|x| (28..36).flat_map(move |y| (320..352).map(move |z| [x, y, z])))
        .filter(|p| !r.world.solid(p[0], p[1], p[2]))
        .count();
    assert!(
        removed >= 2800,
        "front left sparse pinholes instead of a passage: {removed}/3072"
    );
    assert!(r.matter.economy.ledger.balanced());
    assert!(BlastRange::restore(r.config, &r.snapshot()).is_some());
}

#[test]
fn razlom_retries_a_full_fragment_pool_instead_of_losing_the_shot() {
    let mut r = arena_of([RangeUnitKind::RazlomT3, RangeUnitKind::Tank]);
    let mut b = Battle::new(&r.units, 180);
    let pos = [Fx::from_int(421), Fx::from_int(22), Fx::from_int(336)];
    b.rounds.push(Round {
        owner: 0,
        weapon: Weapon::Laser,
        pos,
        previous: pos,
        velocity: [Fx::from_int(120), Fx::ZERO, Fx::ZERO],
        distance: Fx::from_int(motion::spider_tuning().laser.range),
    });
    let cap = r.matter.tuning.max_fragments;
    r.matter.tuning.max_fragments = 0;
    r.battle_flight(&mut b);
    assert_eq!(b.rounds.len(), 1, "full debris pool discarded the wave");
    assert_eq!(b.rounds[0].pos, pos, "wave passed through deferred material");
    assert!(r.units[1].removed.is_empty());
    r.matter.tuning.max_fragments = cap;
    r.battle_flight(&mut b);
    assert!(!r.units[1].removed.is_empty(), "wave never resumed damage");
    assert_eq!(b.rounds.len(), 1);
    assert!(b.rounds[0].pos[0] > pos[0]);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn razlom_stock_scenario_reaches_tanks_without_crossing_cell_bounds() {
    let preset = modding::range_skirmish::stock(22).unwrap();
    let base =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let layout = preset.apply_to(base).unwrap();
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    assert_eq!(r.units[1..4].iter().map(|u| (u.kind, u.at[2])).collect::<Vec<_>>(),
        vec![(RangeUnitKind::Tank, 2400); 3]);
    assert!(r.units[1..4].windows(2).all(|w| w[0].at[0] < w[1].at[0]));
    assert_eq!(r.units[4].kind, RangeUnitKind::Strider);
    assert_eq!(r.units[5].kind, RangeUnitKind::Strider);
    let initial: Vec<_> = r.units.iter().map(|u| u.body.solid_count()).collect();
    r.start_battle().unwrap();
    for _ in 0..360 {
        r.step();
    }
    assert!(
        r.units.iter().skip(1).any(|u| !u.status.alive),
        "the stock wave killed no T1 tank"
    );
    let hit: Vec<_> = r.units.iter().enumerate().filter(|(i, u)| *i > 0 &&
        (!u.status.alive || u.body.solid_count() < initial[*i])).map(|(i, _)| i).collect();
    assert!([1, 2, 3].iter().filter(|i| hit.contains(i)).count() >= 2,
        "the wave must reach multiple tanks in the row: damaged={hit:?}");
    assert!(
        BlastRange::restore(r.config, &r.snapshot()).is_some(),
        "wave damage must debit body material and leave a restorable scene"
    );
}

#[test]
fn kil_produces_a_tank_and_upgrades_to_t4_without_losing_its_queue() {
    let mut layout = arena_of([RangeUnitKind::KilFactoryT1, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.size[1] = 512;
    // Leave room for a physical tank to clear the gate and free the lane.
    layout.units[0].at = [800, 16, 800];
    let mut economy = modding::range_economy::RangeEconomy::stock();
    economy.bank.as_mut().unwrap().base_store = 1 << 40;
    // This queue/upgrade test has no warehouse; use the same isolated
    // supply configuration as the factory production fixture.
    economy.production.as_mut().unwrap().supply_radius = 0;
    economy.production.as_mut().unwrap().pipeline_cost_per_cell = 0;
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
    assert_eq!(
        r.execute_battle_command(battle::commands::Command::Produce {
            actor: 0,
            kind: RangeUnitKind::Tank,
        }),
        battle::commands::Outcome::Applied
    );
    let queue = r.production_queue(0);
    for tier in 2..=4 {
        for _ in 0..20 {
            r.step();
        }
        assert!(r.upgrade_kil(0));
        let next = match tier {
            2 => RangeUnitKind::KilFactoryT2,
            3 => RangeUnitKind::KilFactoryT3,
            _ => RangeUnitKind::KilFactoryT4,
        };
        assert_eq!(r.units[0].kind, next);
        assert_eq!(r.production_queue(0), queue);
        assert_eq!(r.units[0].at, [800, 16, 800]);
    }
    assert!(!r.upgrade_kil(0));
    for _ in 0..240 {
        r.step();
    }
    assert_eq!(r.units.len(), 3, "phase {:?} factory {:?} queue {:?} progress {:?} store {:?}", r.battle.phase, r.units[0].status, r.production_queue(0), r.production_progress(0), r.side_store(true));
    assert_eq!(r.units[2].kind, RangeUnitKind::Tank);
    assert_eq!(
        r.execute_battle_command(battle::commands::Command::Produce {
            actor: 0,
            kind: RangeUnitKind::Tank,
        }),
        battle::commands::Outcome::Applied
    );
    for _ in 0..300 {
        r.step();
    }
    assert_eq!(r.units.len(), 4, "first pos {:?}, blocked {}, order {:?}, phase {:?}, queue {:?}, progress {:?}", r.units[2].at, r.units[2].motion.blocked, r.battle.actors[2].order, r.battle.phase, r.production_queue(0), r.production_progress(0));
    assert_eq!(r.units[3].kind, RangeUnitKind::Tank);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn staratel_drill_is_not_processed_as_a_ballistic_weapon() {
    let mut r = arena_of([RangeUnitKind::Crawler, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[0].order = Order::Auto;
    r.battle.actors[1].order = Order::Hold;
    for _ in 0..4 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    assert_eq!(r.battle_weapon(0), None);
    assert!(r.battle.rounds.is_empty());
    assert!(r.battle.lasers.is_empty());
}

#[test]
fn staratel_scenario_collects_mines_and_unloads_at_its_base() {
    let preset = modding::range_skirmish::stock(23).unwrap();
    let layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let layout = preset.apply_to(layout).unwrap();
    for unit in &layout.units {
        let mut isolated = layout.clone();
        isolated.units = vec![unit.clone()];
        isolated.initial_debris.clear();
        let result = BlastRange::with_layout(
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
            isolated,
        );
        assert!(
            result.is_ok(),
            "placement of {:?} failed: {:?}",
            unit.kind,
            result.err()
        );
    }
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    r.start_battle().unwrap();
    let mut collected = false;
    let mut unloaded = false;
    let mut drilled = false;
    let mut script = preset.script;
    for _ in 0..2400 {
        while script.first().is_some_and(|next| next.tick <= r.tick) {
            let next = script.remove(0);
            let order = match next.order {
                modding::range_skirmish::ScriptedOrder::Attack(target) => Order::Attack(target),
                modding::range_skirmish::ScriptedOrder::Dig(face) => Order::Dig(face),
                modding::range_skirmish::ScriptedOrder::Reclaim(face) => Order::Reclaim(face),
            };
            let accepted = r.battle_order(next.actor, order, None);
            assert!(
                accepted,
                "script tick={} order={order:?} phase={:?} actor={} alive={} at={:?} digs={:?}",
                next.tick,
                r.battle.phase,
                next.actor,
                r.units[next.actor].status.alive,
                r.units[next.actor].at,
                r.mining().map(|m| m.digs(r.units[next.actor].kind))
            );
        }
        r.step();
        collected |= r.tank_held(0).is_some_and(|held| held > 0);
        unloaded |= r.side_store(true).is_some_and(|(amount, _)| amount > 0);
        drilled |= (1100..1104)
            .any(|x| (16..20).any(|y| (2680..2684).any(|z| r.world.material(x, y, z) != 6)));
        if unloaded && drilled {
            break;
        }
    }
    eprintln!(
        "crawler at={:?} destination={:?} tank={:?} store={:?} leg={:?}",
        r.units[0].at,
        r.units[0].motion.destination,
        r.tank_held(0),
        r.side_store(true),
        r.hauling(0)
    );
    assert!(collected, "Staratel collected no debris or rock");
    assert!(unloaded, "Staratel never unloaded at the base");
    assert!(
        drilled,
        "Staratel never drilled the authored rock after collecting debris"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn lightning_tank_fires_on_enemy_and_survives_snapshot_restore() {
    let mut layout = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units[1].at = [300, 16, 336];
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.units[1].status.weapon = false;
    assert_eq!(r.battle_weapon(0), Some(Weapon::Lightning));
    assert!(r.battle_order(0, Order::Attack(1), None));
    let mut hit = false;
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.lightning.iter().any(|trace| trace.hit) {
            hit = true;
            break;
        }
    }
    assert!(hit, "electric vehicle never hit its target");
    assert!(
        r.thermals.hot() > 0
            || !r.units[1].removed.is_empty()
            || !r.units[1].seg_removed.is_empty(),
        "electric contact did not heat or damage the target"
    );
    assert!(r.units[0].status.alive);
    let mut last = "";
    let mut restored =
        BlastRange::restore_observed(r.config, &r.snapshot(), |stage, _, _| last = stage)
            .unwrap_or_else(|| panic!("restore electric unit failed at {last}"));
    for _ in 0..4 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        restored.battle.ai.next_tick = restored.tick + restored.battle.ai.tuning.decision_ticks;
        r.step();
        restored.step();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
}

#[test]
fn electric_support_amplifies_and_stops_when_the_helper_leaves() {
    let base = lightning_tuning();
    assert_eq!((base.work, base.heat, base.fracture_j), (5000, 30000, 10));
    let boosted = supported_lightning(1);
    assert_eq!(
        (
            boosted.range,
            boosted.work,
            boosted.heat,
            boosted.reload_ticks
        ),
        (
            base.range * 2,
            base.work * 2,
            base.heat * 2,
            base.reload_ticks / 2
        )
    );
    assert_eq!(supported_lightning(99).work, base.work * 5);
    let mut layout = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units[1].at = [400, 16, 336];
    layout.units.push(RangeUnitDef {
        kind: RangeUnitKind::LightningTank,
        at: [170, 16, 220],
        allied: true,
        weapon: None,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(2, Order::Support(0), None));
    assert_eq!(r.electric_support_count(0), 1);
    assert_eq!(r.battle_attack_range(0), Some(base.range * 2));
    let original = r.units[2].motion.pos;
    r.units[2].motion.pos[0] = Fx::from_int(500);
    assert_eq!(
        r.electric_support_count(0),
        0,
        "out-of-range helper still grants a bonus"
    );
    r.units[2].motion.pos = original;
    assert!(
        !r.battle_order(0, Order::Support(2), None),
        "support cycle accepted"
    );
    assert!(
        !r.battle_order(2, Order::Support(1), None),
        "enemy accepted as supported tank"
    );
    assert!(r.battle_order(0, Order::Attack(1), None));
    let helper_volume = r.units[2].body.volume_mm3();
    let snapshot = r.snapshot();
    let restored = BlastRange::restore(r.config, &snapshot).expect("support order snapshot");
    assert_eq!(restored.battle.actors[2].order, Order::Support(0));
    assert_eq!(restored.state_hash(), r.state_hash());
    let mut link_from_muzzle = false;
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        let muzzles = r.battle_muzzles(2);
        link_from_muzzle |= r
            .battle
            .lightning
            .iter()
            .any(|trace| muzzles.contains(&trace.from));
        if r.battle.actors[0].fired >= base.burst_ticks && r.battle.actors[0].burst == 0 {
            break;
        }
    }
    assert!(
        r.battle.actors[0].fired >= base.burst_ticks,
        "supported tank did not fire"
    );
    assert_eq!(r.battle.actors[2].fired, 0, "supporter fired at the enemy");
    assert_eq!(
        r.units[2].body.volume_mm3(),
        helper_volume,
        "support link damaged its emitter"
    );
    assert!(
        link_from_muzzle,
        "support link did not originate at the helper's muzzle"
    );
    assert!(r.battle.actors[0].next.saturating_sub(r.tick) <= boosted.reload_ticks);
    assert!(r.matter.economy.ledger.balanced());
    assert!(r.battle_order(2, Order::Hold, None));
    assert_eq!(r.battle.actors[2].order, Order::Hold);
    assert_eq!(r.electric_support_count(0), 0);
    assert_eq!(r.battle_attack_range(0), Some(base.range));
}

#[test]
fn electric_support_approaches_the_target_and_cancels_on_loss() {
    let mut layout = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units.push(RangeUnitDef {
        kind: RangeUnitKind::LightningTank,
        at: [170, 16, 100],
        allied: true,
        weapon: None,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    assert!(r.battle_order(2, Order::Support(0), None));
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.step();
    assert_eq!(r.electric_support_count(0), 0);
    assert!(
        r.units[2].motion.destination.is_some(),
        "distant helper never started approaching"
    );
    assert_eq!(r.battle.actors[2].fired, 0);
    r.units[0].status.alive = false;
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.step();
    assert_eq!(r.battle.actors[2].order, Order::Hold);
    assert_eq!(r.electric_support_count(0), 0);
}

#[test]
fn lightning_pulse_stops_at_a_shield() {
    let mut layout = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units[1].at = [360, 16, 336];
    layout.shields.push(modding::range::RangeShieldDef {
        at: [360, 32, 336],
        radius: 80,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    let target = aim_point(&r.units[1]);
    assert!(r.battle_order(0, Order::Ground(target), None));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.shields[0].hits > 0 {
            break;
        }
    }
    assert!(
        r.shields[0].hits > 0,
        "shield did not intercept electric pulse"
    );
    assert!(r.shields[0].charge < r.shield_tuning.capacity);
    assert_eq!(r.thermals.hot(), 0, "pulse heated the shielded unit");
}

#[test]
fn lightning_chains_to_nearby_hostile_units() {
    let mut layout = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units[1].at = [300, 16, 336];
    layout.units.push(RangeUnitDef {
        kind: RangeUnitKind::Tank,
        at: [300, 16, 405],
        allied: false,
        weapon: None,
    });
    layout.units.push(RangeUnitDef {
        kind: RangeUnitKind::Tank,
        at: [300, 16, 474],
        allied: false,
        weapon: None,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.battle.actors[2].order = Order::Hold;
    r.battle.actors[3].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    let mut reached = false;
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.lightning.len() >= 2 {
            reached = r.thermals.cells().any(|(cell, _)| cell.unit == 2)
                || !r.units[2].removed.is_empty()
                || !r.units[2].seg_removed.is_empty();
            if reached {
                break;
            }
        }
    }
    assert!(reached, "secondary arc did not heat the neighbouring enemy");
    assert!(r.battle.lightning.len() <= 4, "unbounded chain");
    assert!(
        !r.thermals.cells().any(|(cell, _)| cell.unit == 0),
        "emitter hit itself"
    );
}

#[test]
fn lightning_secondary_reaches_the_extended_range_but_not_beyond_it() {
    for (distance, expected) in [(280, true), (360, false)] {
        let mut layout = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank])
            .layout
            .unwrap();
        layout.units[0].at = [170, 16, 160];
        layout.units[1].at = [300, 16, 160];
        layout.units.push(RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [300, 16, 160 + distance],
            allied: false,
            weapon: None,
        });
        let config =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(config, layout).unwrap();
        r.set_physics_workers(1);
        r.set_navigation_workers(1);
        r.start_battle().unwrap();
        r.battle.actors[1].order = Order::Hold;
        r.battle.actors[2].order = Order::Hold;
        assert!(r.battle_order(0, Order::Attack(1), None));
        let mut observed = None;
        for _ in 0..100 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if r.battle.lightning.first().is_some_and(|arc| arc.hit) {
                observed = Some(r.battle.lightning.len() > 1);
                if expected && observed == Some(true) {
                    assert!(
                        r.thermals.cells().any(|(cell, _)| cell.unit == 2)
                            || !r.units[2].removed.is_empty()
                            || !r.units[2].seg_removed.is_empty(),
                        "extended arc did not affect its target"
                    );
                }
                break;
            }
        }
        assert_eq!(observed, Some(expected), "distance={distance}");
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn lightning_burst_lasts_sixteen_ticks_and_respects_cooldown() {
    let mut r = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank]);
    for y in 16..90 {
        for z in 310..362 {
            r.world.queue_edit(voxel::Edit::put(300, y, z, 3));
        }
    }
    while r.world.queued() > 0 {
        r.world.flush();
    }
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Ground([300, 35, 336]), None));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert_eq!(r.battle.actors[0].fired, 1);
    for _ in 1..16 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    assert_eq!(r.battle.actors[0].fired, 16);
    assert_eq!(r.battle.actors[0].burst, 0);
    assert!(r.battle.actors[0].next > r.tick + 20);
}

#[test]
fn lightning_secondary_arc_is_blocked_by_cover() {
    let mut layout = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units[1].at = [300, 16, 336];
    layout.units.push(RangeUnitDef {
        kind: RangeUnitKind::Tank,
        at: [300, 16, 405],
        allied: false,
        weapon: None,
    });
    layout.blocks.push(modding::range::RangeBlock {
        min: [250, 16, 365],
        max: [350, 100, 375],
        material: 3,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.battle.actors[2].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    let mut blocked = false;
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.lightning.len() > 1 {
            let arc = r.battle.lightning[1];
            let end = arc.to.map(Fx::to_int_trunc);
            blocked = arc.hit && (365..375).contains(&end[2]);
            break;
        }
    }
    assert!(blocked, "cover did not intercept the secondary arc");
    assert!(
        !r.thermals.cells().any(|(cell, _)| cell.unit == 2),
        "arc passed through the wall"
    );
}

#[test]
fn lightning_hold_interrupts_the_burst_and_starts_reload() {
    let mut r = arena_of([RangeUnitKind::LightningTank, RangeUnitKind::Tank]);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    let target = aim_point(&r.units[1]);
    assert!(r.battle_order(0, Order::Ground(target), None));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert!(r.battle.actors[0].burst > 0);
    let fired = r.battle.actors[0].fired;
    assert!(r.battle_order(0, Order::Hold, None));
    assert_eq!(r.battle.actors[0].burst, 0);
    assert!(r.battle.actors[0].next >= r.tick + lightning_tuning().reload_ticks);
    for _ in 0..4 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        assert!(r.battle.lightning.is_empty());
    }
    assert_eq!(r.battle.actors[0].fired, fired);
}

#[test]
fn lightning_mission_places_the_electric_vehicle() {
    let base = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let preset = modding::range_skirmish::stock(19).unwrap();
    let layout = preset.apply_to(base).unwrap();
    assert_eq!(layout.units[0].kind, RangeUnitKind::LightningTank);
    super::super::placement::Validator::default()
        .validate(&layout)
        .unwrap();
}

#[test]
fn eidolon_tank_has_working_systems_and_fires_a_real_armour_piercing_round() {
    let mut layout = arena_of([RangeUnitKind::EidolonTank, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units[1].at = [330, 16, 336];
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    assert!(
        r.units[0].status.alive
            && r.units[0].status.engine
            && r.units[0].status.weapon
            && r.units[0].status.sensor
    );
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert_eq!(r.battle_weapon(0), Some(Weapon::Kinetic));
    assert!(r.battle_order(0, Order::Attack(1), None));
    let mut shot = false;
    let mut contact = false;
    for _ in 0..180 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        shot |= r
            .battle
            .rounds
            .iter()
            .any(|round| round.owner == 0 && round.weapon == Weapon::Kinetic);
        contact |= r
            .battle
            .impacts
            .iter()
            .any(|impact| impact.weapon == Weapon::Kinetic);
        if shot && contact {
            break;
        }
    }
    assert!(
        shot && contact,
        "eidolon gun must emit and resolve a kinetic round"
    );
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).expect("restore eidolon vehicle");
    for _ in 0..4 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        replay.battle.ai.next_tick = replay.tick + replay.battle.ai.tuning.decision_ticks;
        r.step();
        replay.step();
        assert_eq!(r.state_hash(), replay.state_hash());
    }
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn eidolon_tank_mission_uses_valid_placement() {
    let base = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let layout = modding::range_skirmish::stock(20)
        .unwrap()
        .apply_to(base)
        .unwrap();
    assert_eq!(layout.units[0].kind, RangeUnitKind::EidolonTank);
    super::super::placement::Validator::default()
        .validate(&layout)
        .unwrap();
}

#[test]
fn eidolon_tank_fires_two_weaker_shells_from_distinct_muzzles() {
    let mut r = arena_of([RangeUnitKind::EidolonTank, RangeUnitKind::Tank]);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    let normal = r.battle.shot_config(Weapon::Kinetic, RangeUnitKind::Tank);
    let twin = r
        .battle
        .shot_config(Weapon::Kinetic, RangeUnitKind::EidolonTank);
    assert_eq!(twin.energy * 2, normal.energy);
    assert_eq!(twin.material_energy_j * 2, normal.material_energy_j);
    assert_eq!(twin.shell_speed, normal.shell_speed);
    assert_eq!(r.battle_muzzles(0).len(), 2);
    assert!(r.battle_order(0, Order::Attack(1), None));
    for _ in 0..100 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    let muzzles = r.battle_muzzles(0);
    let rounds: Vec<_> = r
        .battle
        .rounds
        .iter()
        .filter(|round| round.owner == 0)
        .collect();
    assert_eq!(rounds.len(), 2, "both barrels must fire in the same salvo");
    assert_ne!(rounds[0].previous, rounds[1].previous);
    assert!(
        muzzles
            .iter()
            .all(|p| rounds.iter().any(|round| round.previous == *p)),
        "rounds did not originate at the physical muzzles"
    );
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("restore two flying rounds");
    assert_eq!(
        restored
            .battle
            .rounds
            .iter()
            .filter(|round| round.owner == 0)
            .count(),
        2
    );
    assert_eq!(
        restored
            .battle
            .shot_config(Weapon::Kinetic, RangeUnitKind::EidolonTank)
            .energy,
        twin.energy
    );
}

#[test]
fn eidolon_tank_retains_the_surviving_barrel() {
    let mut r = arena_of([RangeUnitKind::EidolonTank, RangeUnitKind::Tank]);
    assert_eq!(r.battle_muzzles(0).len(), 2);
    for y in 14..20 {
        for z in 17..21 {
            r.units[0].body.remove(50, y, z);
        }
    }
    r.units[0].refresh_damage(0);
    assert!(r.units[0].status.weapon);
    assert_eq!(r.battle_muzzles(0).len(), 1);
    for y in 14..20 {
        for z in 33..37 {
            r.units[0].body.remove(50, y, z);
        }
    }
    r.units[0].refresh_damage(0);
    assert!(!r.units[0].status.weapon);
    assert!(r.battle_muzzles(0).is_empty());
}

#[test]
fn eidolon_tank_follows_a_move_order_with_tracked_speed() {
    let mut r = arena_of([RangeUnitKind::EidolonTank, RangeUnitKind::Tank]);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    let start = r.units[0].motion.pos[0];
    assert_eq!(r.units[0].motion.speed, r.units[1].motion.speed);
    r.start_free_control().unwrap();
    assert!(r.battle_order(0, Order::Move, Some([270, 336])));
    for _ in 0..100 {
        r.step();
        if r.units[0].motion.pos[0] > start.add(Fx::from_int(20)) {
            break;
        }
    }
    assert!(
        r.units[0].motion.pos[0] > start.add(Fx::from_int(20)),
        "eidolon tank did not advance"
    );
}

#[test]
fn eidolon_tank_and_t1_have_comparable_duel_survival() {
    for kinds in [
        [RangeUnitKind::EidolonTank, RangeUnitKind::Tank],
        [RangeUnitKind::Tank, RangeUnitKind::EidolonTank],
    ] {
        let mut r = arena_of(kinds);
        r.set_physics_workers(1);
        r.set_navigation_workers(1);
        r.start_battle().unwrap();
        r.battle.actors[0].order = Order::Attack(1);
        r.battle.actors[1].order = Order::Attack(0);
        r.units[0].motion.enabled = false;
        r.units[1].motion.enabled = false;
        for _ in 0..720 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if r.units.iter().any(|u| !u.status.alive || !u.status.weapon) {
                break;
            }
        }
        eprintln!(
            "duel {kinds:?} tick={} fired={:?} alive={:?} weapons={:?} cores={:?} revisions={:?}",
            r.tick,
            r.battle.actors.iter().map(|a| a.fired).collect::<Vec<_>>(),
            r.units.iter().map(|u| u.status.alive).collect::<Vec<_>>(),
            r.units.iter().map(|u| u.status.weapon).collect::<Vec<_>>(),
            r.units.iter().map(|u| u.integrity.core).collect::<Vec<_>>(),
            r.units.iter().map(|u| u.revision).collect::<Vec<_>>()
        );
        let first = r.tick;
        assert!(
            r.units.iter().any(|u| !u.status.alive),
            "neither unit was defeated"
        );
        for _ in 0..20 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if r.units.iter().all(|u| !u.status.alive) {
                break;
            }
        }
        assert!(
            r.units.iter().all(|u| !u.status.alive),
            "the already-fired salvo did not finish the other tank"
        );
        assert!(
            r.tick - first <= 20,
            "the two defeat times drifted too far apart"
        );
        eprintln!("duel {kinds:?}: defeat ticks {first}, {}", r.tick);
    }
    let mut defeat_ticks = Vec::new();
    for kinds in [
        [RangeUnitKind::Tank, RangeUnitKind::Tank],
        [RangeUnitKind::Tank, RangeUnitKind::EidolonTank],
        [RangeUnitKind::EidolonTank, RangeUnitKind::Tank],
    ] {
        let mut r = arena_of(kinds);
        r.set_physics_workers(1);
        r.set_navigation_workers(1);
        r.start_battle().unwrap();
        r.battle.actors[0].order = Order::Attack(1);
        r.battle.actors[1].order = Order::Hold;
        r.units[0].motion.enabled = false;
        r.units[1].motion.enabled = false;
        for _ in 0..720 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if !r.units[1].status.alive {
                break;
            }
        }
        eprintln!(
            "damage {kinds:?} tick={} fired={} target_alive={} core={:?}",
            r.tick, r.battle.actors[0].fired, r.units[1].status.alive, r.units[1].integrity.core
        );
        assert!(
            !r.units[1].status.alive,
            "target survived the comparison budget"
        );
        defeat_ticks.push(r.tick);
    }
    assert!(
        defeat_ticks[1].abs_diff(defeat_ticks[2]) <= 20,
        "one-sided defeat times differ: {defeat_ticks:?}"
    );
}

#[test]
fn group_move_queues_every_unit_under_the_search_start_budget() {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Strider,
        at: [170, 16, 440],
        allied: true,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut grouped = BlastRange::with_layout(config, layout).unwrap();
    grouped.start_battle().unwrap();
    let destinations = [(0, [530, 350]), (2, [500, 430])];
    let fallback = [400, 400];
    let new = grouped.battle_group_move(&destinations, fallback);
    assert_eq!(new, 2);
    for &(id, destination) in &destinations {
        assert_eq!(grouped.battle.actors[id].order, Order::Move);
        assert_eq!(grouped.units[id].motion.destination, Some(destination));
        assert!(!grouped.units[id].motion.searching());
    }
    grouped.step();
    assert_eq!(
        grouped
            .units
            .iter()
            .filter(|u| u.motion.started_search())
            .count(),
        1
    );
    assert_eq!(grouped.battle_group_move(&[(0, [-2, 350])], fallback), 1);
    assert_eq!(grouped.units[0].motion.destination, Some(fallback));
    assert!(grouped.battle_order(0, Order::Hold, None));
    assert_eq!(grouped.units[0].motion.destination, None);
}

#[test]
fn destroying_a_command_base_wins_even_with_enemy_armour_alive() {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Base,
        at: [150, 16, 120],
        allied: true,
    });
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Base,
        at: [480, 16, 550],
        allied: false,
    });
    let mut battle = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    assert_eq!(battle.battle_base_integrity(), Some([100, 100]));
    assert_eq!(battle.battle_weapon(3), None);
    assert!(!battle.units[3].status.weapon && !battle.units[3].status.engine);
    battle.battle.actors[0].order = Order::Attack(3);
    battle.battle.actors[0].target = Some(3);
    assert_eq!(battle.battle_weapon(0), Some(Weapon::Kinetic));
    battle.start_battle().unwrap();
    let in_flight = [Fx::from_int(300), Fx::from_int(150), Fx::from_int(300)];
    battle.battle.rounds.push(Round {
        owner: 0,
        weapon: Weapon::Kinetic,
        pos: in_flight,
        previous: in_flight,
        velocity: [Fx::from_int(1), Fx::ZERO, Fx::ZERO],
        distance: Fx::from_int(100),
    });
    battle.units[3].status.alive = false;
    battle.battle_tick();
    assert!(battle.units[1].status.alive);
    assert!(!battle.battle.rounds.is_empty());
    assert_eq!(battle.battle_base_integrity(), Some([100, 0]));
    assert_eq!(battle.battle.phase, Phase::Victory);
    battle.battle.phase = Phase::Fighting;
    battle.units[3].status.alive = true;
    battle.units[2].status.alive = false;
    battle.battle_tick();
    assert_eq!(battle.battle.phase, Phase::Defeat);
    battle.battle.phase = Phase::Fighting;
    battle.battle.rounds.clear();
    battle.battle.waves.clear();
    battle.units[0].status.alive = false;
    battle.units[1].status.alive = false;
    battle.units[2].status.alive = true;
    battle.units[3].status.alive = true;
    battle.battle_tick();
    assert_eq!(battle.battle.phase, Phase::Draw);
}

#[test]
fn free_control_moves_groups_without_starting_damage_or_ai_and_replays() {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Strider,
        at: [170, 16, 440],
        allied: true,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_free_control().unwrap();
    assert!(!r.battle_order(0, Order::Attack(1), None));
    assert!(!r.battle_order(0, Order::Ground([450, 20, 336]), None));
    assert!(!r.battle_order(0, Order::Auto, None));
    assert!(!r.battle_order(1, Order::Move, Some([300, 336])));
    assert_eq!(
        r.battle_group_move(&[(0, [280, 336]), (2, [280, 440])], [280, 388]),
        2
    );
    let before: Vec<_> = r.units.iter().map(|u| u.motion.pos).collect();
    for _ in 0..120 {
        r.step();
    }
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..280 {
        r.step();
        replay.step();
        assert_eq!(r.state_hash(), replay.state_hash());
    }
    assert!(r.units[0].motion.pos[0] > before[0][0].add(Fx::from_int(60)));
    assert!(r.units[2].motion.pos[0] > before[2][0].add(Fx::from_int(60)));
    assert_eq!(r.units[1].motion.pos, before[1]);
    assert_eq!(r.battle.phase, Phase::Setup);
    assert!(r.battle.actors.iter().all(|a| a.fired == 0));
    assert!(r.units.iter().all(|u| u.revision == 0));
    assert!(r.battle_order(0, Order::Hold, None));
    r.start_battle().unwrap();
    assert!(r.start_free_control().is_err());
    for _ in 0..180 {
        r.step();
    }
    assert!(r.battle.actors.iter().any(|a| a.fired > 0));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn distant_stationary_garrisons_draw_after_the_mobile_attackers_are_gone() {
    let mut layout = arena_of([RangeUnitKind::TurretT1, RangeUnitKind::TurretT2])
        .layout
        .unwrap();
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Base,
        at: [150, 16, 120],
        allied: true,
    });
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Base,
        at: [480, 16, 550],
        allied: false,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut near = BlastRange::with_layout(config, layout.clone()).unwrap();
    near.start_battle().unwrap();
    near.battle_tick();
    assert_eq!(near.battle.phase, Phase::Fighting);

    layout.size = [1808, 192, 640];
    layout.clear_max = [1700, 600];
    layout.units[1].at = [1600, 16, 336];
    layout.units[3].at = [1600, 16, 550];
    let mut far = BlastRange::with_layout(config, layout).unwrap();
    far.start_battle().unwrap();
    far.battle_tick();
    assert_eq!(far.battle.phase, Phase::Draw);
}

#[test]
fn stranded_gun_does_not_keep_a_remote_base_assault_fighting() {
    let mut layout = arena_of([RangeUnitKind::Tank, RangeUnitKind::Artillery])
        .layout
        .unwrap();
    layout.size = [4096, 192, 640];
    layout.clear_max = [4000, 600];
    layout.units = vec![
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Base,
            at: [150, 16, 336],
            allied: true,
        },
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [2800, 16, 336],
            allied: true,
        },
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Artillery,
            at: [3000, 16, 336],
            allied: false,
        },
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Base,
            at: [3800, 16, 336],
            allied: false,
        },
    ];
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut battle = BlastRange::with_layout(config, layout).unwrap();
    battle.start_battle().unwrap();
    battle.battle.ai.next_tick = u32::MAX;
    battle.units[1].status.weapon = false;
    battle.units[2].mobility.traction = Fx::ZERO;
    battle.battle.actors[2].order = Order::Attack(0);
    battle.battle.actors[2].target = Some(0);
    battle.battle_tick();
    assert_eq!(battle.battle.phase, Phase::Draw);
}

#[test]
fn northern_base_approach_lets_an_assault_tank_answer_the_turret() {
    let mut battle = northern_base_battle();
    for _ in 0..2000 {
        battle.step();
        if battle.battle.actors[3].fired > 0 {
            assert_eq!(battle.battle.actors[3].target, Some(1),
                "first assault shot ignored the defending turret: tick={}, at={:?}, turret_shots={}",
                battle.tick, battle.units[3].at, battle.battle.actors[1].fired);
            break;
        }
        if !battle.units[3].status.alive {
            break;
        }
    }
    assert!(battle.battle.actors[3].fired > 0, "tank failed to answer: at={:?}, order={:?}, target={:?}, turret_shots={}, revision={}, dest={:?}, blocked={}",
        battle.units[3].at, battle.battle.actors[3].order, battle.battle.actors[3].target,
        battle.battle.actors[1].fired, battle.units[3].revision,
        battle.units[3].motion.destination, battle.units[3].motion.blocked);
}

fn northern_base_battle() -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    layout.units = vec![
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Base,
            at: [1530, 16, 630],
            allied: true,
        },
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::TurretT1,
            at: [1550, 16, 450],
            allied: true,
        },
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Base,
            at: [8820, 16, 8870],
            allied: false,
        },
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [3000, 16, 760],
            allied: false,
        },
    ];
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut battle = BlastRange::with_layout(config, layout).unwrap();
    assert!(battle.set_ai_vs_ai(true));
    battle.start_battle().unwrap();
    battle
}

#[test]
fn pursuit_keeps_a_lateral_firing_goal_until_it_leaves_weapon_reach() {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [300, 16, 240],
        max: [332, 96, 432],
        material: 3,
    });
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    assert!(r.battle_order(0, Order::Attack(1), None));
    r.battle.actors[1].order = Order::Hold;
    r.battle.actors[0].target = Some(1);
    let goal = [300, 516]; // 234 units from the target, clear around the wall.
    let from = r.battle_muzzles(0)[0].map(Fx::to_int_trunc);
    let target = aim_point(&r.units[1]);
    assert!(
        !voxel::los::line_of_sight(
            &r.world,
            (from[0], from[1], from[2]),
            (target[0], target[1], target[2])
        )
        .clear
    );
    assert!(
        voxel::los::line_of_sight(
            &r.world,
            (goal[0], from[1], goal[1]),
            (target[0], target[1], target[2])
        )
        .clear
    );
    // A previously planned detour still under way. Isolate pursuit orders
    // from drive/route latency, which the northern-track regression covers.
    r.units[0].motion.stop();
    r.units[0].motion.path = vec![goal];
    r.units[0].motion.destination = Some(goal);
    for _ in 0..6 {
        let mut battle = std::mem::replace(&mut r.battle, Battle::new(&[], 1));
        r.battle_orders(&mut battle, |_| {}, &|_, _| {});
        r.battle = battle;
        assert_eq!(r.units[0].motion.destination, Some(goal));
        assert!(!r.units[0].motion.searching(), "usable flank was replanned");
        assert_eq!(r.battle.actors[0].fired, 0, "wall was ignored");
        r.tick += r.battle.tuning.pursuit_ticks;
    }
    // Changing the actual weapon reach must invalidate the old position.
    std::sync::Arc::make_mut(&mut r.battle.tuning).kinetic_range = 220;
    let mut battle = std::mem::replace(&mut r.battle, Battle::new(&[], 1));
    r.battle_orders(&mut battle, |_| {}, &|_, _| {});
    r.battle = battle;
    assert_ne!(r.units[0].motion.destination, Some(goal));
}

#[test]
fn pursuit_does_not_restart_the_identical_obscured_fallback_route() {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [350, 16, 64],
        max: [366, 96, 600],
        material: 3,
    });
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    assert!(r.battle_order(0, Order::Attack(1), None));
    r.battle.actors[1].order = Order::Hold;
    r.battle.actors[0].target = Some(1);
    let target = aim_point(&r.units[1]);
    let muzzle_y = r.battle_muzzles(0)[0][1].to_int_trunc();
    let goal = r.visible_flank(
        0,
        target,
        [250, 336],
        muzzle_y,
        r.battle.tuning.pursuit_ticks,
        r.battle.tuning.firing_spacing,
    );
    assert!(!r.firing_line_clear(0, target, goal, muzzle_y));
    r.units[0].motion.stop();
    r.units[0].motion.path = vec![goal];
    r.units[0].motion.destination = Some(goal);
    for _ in 0..6 {
        let mut battle = std::mem::replace(&mut r.battle, Battle::new(&[], 1));
        r.battle_orders(&mut battle, |_| {}, &|_, _| {});
        r.battle = battle;
        assert_eq!(r.units[0].motion.destination, Some(goal));
        assert!(
            !r.units[0].motion.searching(),
            "identical fallback restarted a usable route"
        );
        assert_eq!(r.battle.actors[0].fired, 0);
        // Repeat the same flank phase, with unchanged target and geometry.
        r.tick += 2 * r.battle.tuning.pursuit_ticks;
    }
}

#[test]
fn pursuit_finishes_a_checked_partial_flank_before_choosing_another() {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [350, 16, 64],
        max: [366, 96, 600],
        material: 3,
    });
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    assert!(r.battle_order(0, Order::Attack(1), None));
    r.battle.actors[1].order = Order::Hold;
    r.battle.actors[0].target = Some(1);
    let goal = [300, 516];
    r.units[0].motion.stop();
    r.units[0].motion.path = vec![[250, 516]];
    r.units[0].motion.destination = Some(goal);
    r.units[0].motion.partial_path = true;
    for _ in 0..6 {
        let mut battle = std::mem::replace(&mut r.battle, Battle::new(&[], 1));
        r.battle_orders(&mut battle, |_| {}, &|_, _| {});
        r.battle = battle;
        assert_eq!(r.units[0].motion.destination, Some(goal));
        assert!(
            !r.units[0].motion.searching(),
            "pursuit abandoned its checked prefix"
        );
        r.tick += r.battle.tuning.pursuit_ticks;
    }
    // Ending the prefix permits a fresh approach to the still obscured target.
    r.units[0].motion.path.clear();
    let mut battle = std::mem::replace(&mut r.battle, Battle::new(&[], 1));
    r.battle_orders(&mut battle, |_| {}, &|_, _| {});
    r.battle = battle;
    assert_ne!(r.units[0].motion.destination, Some(goal));
}

#[test]
fn tank_eventually_recovers_after_northern_turret_hits_its_tracks() {
    let mut battle = northern_base_battle();
    for _ in 0..1500 {
        battle.step();
        if battle.units[3].revision > 0 {
            break;
        }
    }
    assert!(
        battle.units[3].revision > 0,
        "turret never hit the approaching tank"
    );
    assert!(battle.units[3].status.alive && battle.units[3].mobility.traction > Fx::ZERO);
    battle.battle.ai.next_tick = u32::MAX;
    battle.battle.actors[1].order = Order::Hold;
    let hit = battle.units[3].at;
    let shots_at_hit = battle.battle.actors[3].fired;
    // Physical support corrections may choose a different budgeted route.
    // Keep the original progress deadline unless a search is actually pending;
    // only that case receives a bounded extension, not a lower distance bar.
    for _ in 0..900 {
        battle.step();
    }
    let at_deadline = battle.units[3].at;
    let moved_at_deadline = (at_deadline[0] - hit[0]).abs() + (at_deadline[2] - hit[2]).abs();
    if moved_at_deadline <= 80 && battle.units[3].motion.searching() {
        for _ in 0..300 {
            battle.step();
            let at = battle.units[3].at;
            if (at[0] - hit[0]).abs() + (at[2] - hit[2]).abs() > 80 {
                break;
            }
        }
    }
    let now = battle.units[3].at;
    let moved = (now[0] - hit[0]).abs() + (now[2] - hit[2]).abs();
    assert!(moved > 80,
        "damaged tank stayed trapped: hit={hit:?}, now={now:?}, shots_after_hit={}, traction={}, path={:?}, blocked={}, recovery={:?}",
        battle.battle.actors[3].fired - shots_at_hit, battle.units[3].mobility.traction.to_f64_lossy(),
        battle.units[3].motion.path, battle.units[3].motion.blocked,
        battle.units[3].motion.recovery_state());
}

#[test]
fn annihilation_hit_on_base_ends_a_real_battle() {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.units[1].at = [500, 16, 500];
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Base,
        at: [150, 16, 120],
        allied: true,
    });
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Base,
        at: [450, 16, 336],
        allied: false,
    });
    let mut battle = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    battle.start_battle().unwrap();
    let restored = BlastRange::restore(battle.config, &battle.snapshot())
        .expect("battle with both command bases restores before the first hit");
    assert_eq!(battle.state_hash(), restored.state_hash());
    battle.battle.actors[1].order = Order::Hold;
    battle.battle.ai.next_tick = u32::MAX;
    assert!(battle.set_battle_weapon(0, Some(Weapon::Annihilation)));
    assert!(battle.battle_order(0, Order::Attack(3), None));
    let mut hit = false;
    for _ in 0..600 {
        battle.step();
        hit |= battle
            .battle
            .impacts
            .iter()
            .any(|i| i.weapon == Weapon::Annihilation);
        if battle.battle.phase != Phase::Fighting {
            break;
        }
    }
    assert!(hit, "annihilation bolt never reached the command base");
    assert!(
        battle.units[1].status.alive,
        "enemy armour must survive the base objective"
    );
    assert_eq!(
        battle.battle.phase,
        Phase::Victory,
        "base alive={} core={} shots={} damage={}",
        battle.units[3].status.alive,
        battle.units[3].integrity.core.to_f64_lossy(),
        battle.battle.actors[0].fired,
        battle.units[3].revision
    );
    let restored = BlastRange::restore(battle.config, &battle.snapshot())
        .expect("battle with a destroyed command base restores");
    assert_eq!(battle.state_hash(), restored.state_hash());
    let mut restored = restored;
    let shots: Vec<_> = battle.battle.actors.iter().map(|a| a.fired).collect();
    let finished_tick = battle.tick;
    for _ in 0..180 {
        battle.step();
        restored.step();
        assert_eq!(battle.battle.phase, Phase::Victory);
        assert_eq!(battle.state_hash(), restored.state_hash());
        assert!(battle
            .battle
            .actors
            .iter()
            .zip(&shots)
            .all(|(actor, &fired)| actor.fired == fired));
        assert!(battle.matter.economy.ledger.balanced());
    }
    assert_eq!(battle.tick, finished_tick + 180);
}

#[test]
fn enemy_annihilation_hit_on_our_base_causes_defeat() {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.units[0].at = [100, 16, 500];
    layout.units[1].at = [350, 16, 120];
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Base,
        at: [150, 16, 120],
        allied: true,
    });
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Base,
        at: [480, 16, 550],
        allied: false,
    });
    let mut battle = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    battle.start_battle().unwrap();
    battle.units[0].status.alive = false;
    battle.battle.actors[0].order = Order::Hold;
    battle.battle.actors[1].weapon = Some(Weapon::Annihilation);
    for _ in 0..600 {
        battle.step();
        if battle.battle.phase != Phase::Fighting {
            break;
        }
    }
    assert_eq!(battle.battle.phase, Phase::Defeat);
    assert_eq!(battle.battle.actors[1].order, Order::Attack(2));
    assert!(battle.units[3].status.alive);
    assert_eq!(battle.battle_base_integrity().unwrap()[0], 0);
}

#[test]
fn enemy_rules_focus_fire_and_switch_only_for_a_better_target() {
    let mut tuning =
        modding::battle_ai::Tuning::from_ron(include_str!("../../../data/battle-ai.ron")).unwrap();
    tuning.switch_margin = 20;
    let mut observation = Observation {
        version: OBSERVATION_VERSION,
        observed_tick: 20,
        decision_tick: 30,
        size: [640, 192, 640],
        units: vec![
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 0,
                allied: true,
                alive: true,
                armed: true,
                mobile: true,
                can_reposition: true,
                kind: RangeUnitKind::Tank,
                position: [100, 100],
                ordered_target: None,
            },
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 1,
                allied: true,
                alive: true,
                armed: false,
                mobile: true,
                can_reposition: true,
                kind: RangeUnitKind::Tank,
                position: [350, 100],
                ordered_target: None,
            },
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 2,
                allied: false,
                alive: true,
                armed: true,
                mobile: true,
                can_reposition: true,
                kind: RangeUnitKind::Tank,
                position: [230, 100],
                ordered_target: Some(1),
            },
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 3,
                allied: false,
                alive: true,
                armed: true,
                mobile: true,
                can_reposition: true,
                kind: RangeUnitKind::Tank,
                position: [250, 100],
                ordered_target: Some(1),
            },
        ],
    };
    let rules = Rules {
        tuning: &tuning,
        radii: &modding::battle_ai::Radii::stock(),
        controlled_allied: false,
    };
    assert_eq!(
        rules.decide(&observation),
        vec![
            Decision {
                actor: 2,
                order: AiOrder::Attack(0)
            },
            Decision {
                actor: 3,
                order: AiOrder::Attack(0)
            }
        ]
    );
    observation.units[0].alive = false;
    assert_eq!(
        rules.decide(&observation),
        vec![
            Decision {
                actor: 2,
                order: AiOrder::Attack(1)
            },
            Decision {
                actor: 3,
                order: AiOrder::Attack(1)
            }
        ]
    );
    observation.units[0].alive = true;
    observation.units[2].ordered_target = Some(0);
    observation.units[3].ordered_target = Some(0);
    observation.units[0].position = [265, 100];
    assert_eq!(rules.decide(&observation)[0].order, AiOrder::Attack(0));
}

#[test]
fn enemy_base_does_not_vote_on_targets_or_receive_attack_orders() {
    let mut tuning =
        modding::battle_ai::Tuning::from_ron(include_str!("../../../data/battle-ai.ron")).unwrap();
    tuning.armed_bonus = 0;
    tuning.mobile_bonus = 0;
    let mut units = vec![
        UnitObservation {
            attack_ranges: [1600; RangeUnitKind::COUNT],
            id: 0,
            allied: true,
            alive: true,
            armed: true,
            mobile: true,
            can_reposition: true,
            kind: RangeUnitKind::Tank,
            position: [100, 100],
            ordered_target: None,
        },
        UnitObservation {
            attack_ranges: [1600; RangeUnitKind::COUNT],
            id: 1,
            allied: true,
            alive: true,
            armed: true,
            mobile: true,
            can_reposition: true,
            kind: RangeUnitKind::Tank,
            position: [500, 100],
            ordered_target: None,
        },
        UnitObservation {
            attack_ranges: [1600; RangeUnitKind::COUNT],
            id: 2,
            allied: false,
            alive: true,
            armed: true,
            mobile: true,
            can_reposition: true,
            kind: RangeUnitKind::Tank,
            position: [230, 100],
            ordered_target: None,
        },
        UnitObservation {
            attack_ranges: [1600; RangeUnitKind::COUNT],
            id: 3,
            allied: false,
            alive: true,
            armed: false,
            mobile: false,
            can_reposition: false,
            kind: RangeUnitKind::Base,
            position: [550, 100],
            ordered_target: None,
        },
        UnitObservation {
            attack_ranges: [1600; RangeUnitKind::COUNT],
            id: 4,
            allied: true,
            alive: true,
            armed: false,
            mobile: false,
            can_reposition: false,
            kind: RangeUnitKind::Base,
            position: [50, 100],
            ordered_target: None,
        },
    ];
    assert_eq!(
        Rules {
            tuning: &tuning,
            radii: &modding::battle_ai::Radii::stock(),
            controlled_allied: false
        }
        .decide(&Observation {
            version: OBSERVATION_VERSION,
            observed_tick: 20,
            decision_tick: 30,
            size: [640, 192, 640],
            units: units.clone(),
        }),
        vec![Decision {
            actor: 2,
            order: AiOrder::Attack(0),
        }]
    );
    units[0].alive = false;
    units[1].alive = false;
    assert_eq!(
        Rules {
            tuning: &tuning,
            radii: &modding::battle_ai::Radii::stock(),
            controlled_allied: false
        }
        .decide(&Observation {
            version: OBSERVATION_VERSION,
            observed_tick: 40,
            decision_tick: 50,
            size: [640, 192, 640],
            units,
        }),
        vec![Decision {
            actor: 2,
            order: AiOrder::Attack(4),
        }]
    );
}

#[test]
fn autonomous_battle_orders_both_sides_and_replays() {
    let mut r = arena(RangeUnitKind::Tank);
    assert!(r.set_ai_vs_ai(true));
    r.start_battle().unwrap();
    r.step();
    assert_eq!(r.battle.actors[0].order, Order::Attack(1));
    assert_eq!(r.battle.actors[1].order, Order::Attack(0));
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert!(replay.battle.autonomous);
    for _ in 0..20 {
        r.step();
        replay.step();
        assert_eq!(r.state_hash(), replay.state_hash());
    }
}

#[test]
fn manual_order_takes_over_allied_ai_without_disabling_the_enemy() {
    let mut r = arena(RangeUnitKind::Tank);
    assert!(r.set_ai_vs_ai(true));
    r.start_battle().unwrap();
    r.step();
    assert!(r.battle.autonomous);
    assert!(!r.battle_order(1, Order::Hold, None));
    assert!(
        r.battle.autonomous,
        "a rejected enemy order must not take over"
    );
    assert!(r.battle_order(0, Order::Hold, None));
    assert!(!r.battle.autonomous);
    let mut restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..60 {
        r.step();
        restored.step();
        assert_eq!(r.battle.actors[0].order, Order::Hold);
        assert_eq!(r.battle.actors[1].order, Order::Attack(0));
        assert_eq!(r.state_hash(), restored.state_hash());
    }
}

#[test]
fn rules_defend_local_fronts_instead_of_chasing_a_global_target() {
    let tuning =
        modding::battle_ai::Tuning::from_ron(include_str!("../../../data/battle-ai.ron")).unwrap();
    let unit = |id, allied, x| UnitObservation {
        attack_ranges: [1600; RangeUnitKind::COUNT],
        id,
        allied,
        alive: true,
        armed: true,
        mobile: true,
        can_reposition: true,
        kind: RangeUnitKind::Tank,
        position: [x, 100],
        ordered_target: None,
    };
    let observation = Observation {
        version: OBSERVATION_VERSION,
        observed_tick: 0,
        decision_tick: 0,
        size: [10000, 192, 10000],
        units: vec![
            unit(0, true, 100),
            unit(1, true, 9000),
            unit(2, false, 200),
            unit(3, false, 8900),
        ],
    };
    let red = Rules {
        tuning: &tuning,
        radii: &modding::battle_ai::Radii::stock(),
        controlled_allied: false,
    }
    .decide(&observation);
    let blue = Rules {
        tuning: &tuning,
        radii: &modding::battle_ai::Radii::stock(),
        controlled_allied: true,
    }
    .decide(&observation);
    assert_eq!(red[0].order, AiOrder::Attack(0));
    assert_eq!(red[1].order, AiOrder::Attack(1));
    assert_eq!(blue[0].order, AiOrder::Attack(2));
    assert_eq!(blue[1].order, AiOrder::Attack(3));
}

#[test]
fn mobile_guns_switch_to_the_base_after_field_weapons_are_disabled() {
    let tuning =
        modding::battle_ai::Tuning::from_ron(include_str!("../../../data/battle-ai.ron")).unwrap();
    let observation = Observation {
        version: OBSERVATION_VERSION,
        observed_tick: 0,
        decision_tick: 0,
        size: [10000, 192, 10000],
        units: vec![
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 0,
                allied: true,
                alive: true,
                armed: true,
                mobile: true,
                can_reposition: true,
                kind: RangeUnitKind::Tank,
                position: [8000, 4000],
                ordered_target: Some(1),
            },
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 1,
                allied: false,
                alive: true,
                armed: false,
                mobile: true,
                can_reposition: true,
                kind: RangeUnitKind::Strider,
                position: [8100, 4000],
                ordered_target: None,
            },
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 2,
                allied: false,
                alive: true,
                armed: false,
                mobile: false,
                can_reposition: false,
                kind: RangeUnitKind::Base,
                position: [1500, 600],
                ordered_target: None,
            },
        ],
    };
    let rules = Rules {
        tuning: &tuning,
        radii: &modding::battle_ai::Radii::stock(),
        controlled_allied: true,
    };
    assert_eq!(rules.decide(&observation)[0].order, AiOrder::Attack(2));
}

#[test]
fn observation_does_not_call_a_tank_mobile_without_traction() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    r.units[0].mobility.traction = Fx::ZERO;
    assert!(r.units[0].status.engine);
    assert!(!r.enemy_ai_observation().unwrap().units[0].mobile);
    assert!(!r.enemy_ai_observation().unwrap().units[0].can_reposition);
}

#[test]
fn observation_does_not_call_an_unsupported_walker_mobile() {
    let mut r = arena(RangeUnitKind::Strider);
    r.start_battle().unwrap();
    assert!(r.units[0].motion.legs_lost.len() >= 2);
    for leg in r.units[0].motion.legs_lost.iter_mut().skip(1) {
        *leg = true;
    }
    assert!(r.units[0].status.engine);
    assert!(!r.enemy_ai_observation().unwrap().units[0].mobile);
    assert!(!r.enemy_ai_observation().unwrap().units[0].can_reposition);
}

#[test]
fn observation_keeps_reposition_capability_while_a_walkers_legs_are_buckled() {
    let mut r = arena(RangeUnitKind::Strider);
    r.start_battle().unwrap();
    r.units[0].motion.legs_buckled.fill(true);
    let observation = r.enemy_ai_observation().unwrap();
    assert!(!observation.units[0].mobile);
    assert!(observation.units[0].can_reposition);
}

#[test]
fn ai_does_not_cancel_a_blocked_hulls_escape_manoeuvre() {
    let mut r = arena(RangeUnitKind::Tank);
    std::sync::Arc::make_mut(&mut r.battle.tuning).kinetic_range = 100;
    r.start_battle().unwrap();
    r.battle.autonomous = true;
    r.battle.actors[0].order = Order::Attack(1);
    r.battle.actors[0].target = Some(1);
    r.units[0].motion.destination = Some([250, 336]);
    r.units[0].motion.blocked = true;
    assert!(r.units[0].motion.recovering());
    r.battle_tick();
    assert_eq!(r.units[0].motion.destination, Some([250, 336]));
    assert!(r.units[0].motion.recovering());
}

#[test]
fn artillery_attacks_the_base_until_an_armed_defender_is_close() {
    let tuning =
        modding::battle_ai::Tuning::from_ron(include_str!("../../../data/battle-ai.ron")).unwrap();
    let mut observation = Observation {
        version: OBSERVATION_VERSION,
        observed_tick: 0,
        decision_tick: 0,
        size: [10000, 192, 10000],
        units: vec![
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 0,
                allied: true,
                alive: true,
                armed: true,
                mobile: true,
                can_reposition: true,
                kind: RangeUnitKind::Artillery,
                position: [1000, 1000],
                ordered_target: None,
            },
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 1,
                allied: false,
                alive: true,
                armed: true,
                mobile: true,
                can_reposition: true,
                kind: RangeUnitKind::Tank,
                position: [4000, 4000],
                ordered_target: None,
            },
            UnitObservation {
                attack_ranges: [1600; RangeUnitKind::COUNT],
                id: 2,
                allied: false,
                alive: true,
                armed: false,
                mobile: false,
                can_reposition: false,
                kind: RangeUnitKind::Base,
                position: [8500, 8500],
                ordered_target: None,
            },
        ],
    };
    let rules = Rules {
        tuning: &tuning,
        radii: &modding::battle_ai::Radii::stock(),
        controlled_allied: true,
    };
    assert_eq!(rules.decide(&observation)[0].order, AiOrder::Attack(2));
    observation.units[1].position = [1800, 1200];
    assert_eq!(rules.decide(&observation)[0].order, AiOrder::Attack(1));
}

#[test]
fn observed_weapon_reach_uses_euclidean_boundaries_and_safe_coordinate_extremes() {
    let mut r = arena_of([RangeUnitKind::ArtilleryTurretT2, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    let mut observation = r.enemy_ai_observation().unwrap();
    let mut actor = observation.units[0];
    actor.id = 41;
    // This 3-4-5 boundary checks the metric independently of stock balance.
    actor.attack_ranges.fill(1600);
    actor.position = [4400, 4900];
    observation.units[1].position = [5480, 5210];
    assert!(actor.in_weapon_reach(&observation.units[1]));
    observation.units[1].position = [5360, 6180];
    assert!(actor.in_weapon_reach(&observation.units[1]));
    observation.units[1].position[0] += 1;
    assert!(!actor.in_weapon_reach(&observation.units[1]));
    actor.attack_ranges.fill(0);
    assert!(!actor.in_weapon_reach(&observation.units[1]));
    actor.attack_ranges.fill(i32::MAX);
    actor.position = [i32::MIN, i32::MIN];
    observation.units[1].position = [i32::MAX, i32::MAX];
    assert!(!actor.in_weapon_reach(&observation.units[1]));
}

#[test]
fn observation_ranges_follow_live_weapon_tuning_and_target_ammunition() {
    for kind in RangeUnitKind::ALL {
        let mut r = arena_of([kind, RangeUnitKind::Tank]);
        r.start_battle().unwrap();
        r.battle.actors[0].order = Order::Attack(1);
        r.battle.actors[0].target = Some(1);
        for target_kind in RangeUnitKind::ALL {
            r.units[1].kind = target_kind;
            let observation = r.enemy_ai_observation().unwrap();
            assert_eq!(
                observation.units[0].attack_ranges[target_kind.index()],
                r.battle_attack_range(0).unwrap_or(0),
                "{kind:?} vs {target_kind:?}"
            );
        }
        if matches!(kind, RangeUnitKind::Tank | RangeUnitKind::Artillery) {
            for weapon in [
                Weapon::Blast,
                Weapon::Kinetic,
                Weapon::Energy,
                Weapon::Annihilation,
            ] {
                assert!(r.set_battle_weapon(0, Some(weapon)));
                let observation = r.enemy_ai_observation().unwrap();
                assert!(observation.units[0]
                    .attack_ranges
                    .iter()
                    .all(|&range| Some(range) == r.battle_attack_range(0)));
            }
        }
        if kind.canister() {
            std::sync::Arc::make_mut(&mut r.battle.tuning)
                .canister
                .range = 777;
            assert!(r.enemy_ai_observation().unwrap().units[0]
                .attack_ranges
                .iter()
                .all(|&range| range == 777));
        }
        r.units[0].status.weapon = false;
        assert_eq!(
            r.enemy_ai_observation().unwrap().units[0].attack_ranges,
            [0; RangeUnitKind::COUNT]
        );
    }
}

#[test]
fn autonomous_immobile_artillery_fires_at_local_defender_and_replays() {
    let mut layout = arena_of([RangeUnitKind::ArtilleryTurretT2, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.size = [2000, 192, 2000];
    layout.clear_max = [1936, 1936];
    layout.units.push(RangeUnitDef {
        allied: false,
        kind: RangeUnitKind::Base,
        at: [1850, 16, 1850],
        weapon: None,
    });
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    r.start_battle().unwrap();
    r.battle.autonomous = true;
    r.submit_enemy_ai_plan(
        r.battle.ai.next_tick,
        vec![
            Decision {
                actor: 1,
                order: AiOrder::Hold,
            },
            Decision {
                actor: 2,
                order: AiOrder::Hold,
            },
        ],
    )
    .unwrap();
    r.step();
    assert_eq!(r.battle.actors[0].order, Order::Attack(1));
    let mut restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for tick in 0..300 {
        for sim in [&mut r, &mut restored] {
            // External Hold orders isolate return fire; the autonomous allied
            // controller continues making real decisions on its normal ticks.
            if let Some(observation) = sim
                .enemy_ai_observation()
                .filter(|o| o.decision_tick == sim.tick)
            {
                let plan = observation
                    .units
                    .iter()
                    .filter(|u| !u.allied && u.alive)
                    .map(|u| Decision {
                        actor: u.id,
                        order: AiOrder::Hold,
                    })
                    .collect();
                sim.submit_enemy_ai_plan(observation.decision_tick, plan)
                    .unwrap();
            }
            sim.step();
        }
        if tick % 50 == 49 {
            assert_eq!(r.state_hash(), restored.state_hash());
        }
    }
    assert!(
        r.battle.actors[0].fired > 0,
        "the chosen defender was never fired on"
    );
    assert!(
        r.units[1].revision > 0,
        "the autonomous attack never damaged the defender"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn outnumbered_army_keeps_a_rear_guard_without_abandoning_its_siege() {
    let tuning =
        modding::battle_ai::Tuning::from_ron(include_str!("../../../data/battle-ai.ron")).unwrap();
    let unit = |id, allied, kind, position| UnitObservation {
        attack_ranges: [1600; RangeUnitKind::COUNT],
        id,
        allied,
        alive: true,
        armed: kind != RangeUnitKind::Base,
        mobile: !matches!(kind, RangeUnitKind::Base | RangeUnitKind::TurretT1),
        can_reposition: !matches!(kind, RangeUnitKind::Base | RangeUnitKind::TurretT1),
        kind,
        position,
        ordered_target: None,
    };
    let mut observation = Observation {
        version: OBSERVATION_VERSION,
        observed_tick: 0,
        decision_tick: 0,
        size: [10000, 192, 10000],
        units: vec![
            unit(0, true, RangeUnitKind::Base, [1000, 1000]),
            unit(1, true, RangeUnitKind::Tank, [1100, 1000]),
            unit(2, true, RangeUnitKind::Artillery, [1200, 900]),
            unit(3, true, RangeUnitKind::Tank, [5000, 5000]),
            unit(4, false, RangeUnitKind::Base, [9000, 9000]),
            unit(5, false, RangeUnitKind::Tank, [6000, 6000]),
            unit(6, false, RangeUnitKind::Tank, [6200, 6000]),
            unit(7, false, RangeUnitKind::Strider, [6400, 6000]),
            unit(8, false, RangeUnitKind::Artillery, [9000, 8500]),
        ],
    };
    let rules = Rules {
        tuning: &tuning,
        radii: &modding::battle_ai::Radii::stock(),
        controlled_allied: true,
    };
    let orders = rules.decide(&observation);
    assert_eq!(orders[0].order, AiOrder::Move([1300, 1300]));
    assert_eq!(orders[1].order, AiOrder::Attack(4));
    assert!(matches!(orders[2].order, AiOrder::Attack(_)));
    observation.units[5].position = [1500, 1300];
    assert_eq!(rules.decide(&observation)[0].order, AiOrder::Attack(5));
    observation.units[1].ordered_target = Some(5);
    observation.units[5].position = [2800, 1300];
    assert_eq!(rules.decide(&observation)[0].order, AiOrder::Attack(5));
    observation.units[1].ordered_target = None;
    assert_eq!(
        rules.decide(&observation)[0].order,
        AiOrder::Move([1300, 1300])
    );
    observation.units[5].position = [6000, 6000];
    observation.units[6].alive = false;
    assert_eq!(
        rules.decide(&observation)[0].order,
        AiOrder::Move([1300, 1300])
    );
    observation.units[7].alive = false;
    assert!(matches!(
        rules.decide(&observation)[0].order,
        AiOrder::Attack(_)
    ));
}

#[test]
fn autonomous_guard_does_not_chase_across_the_map_after_arriving() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    r.battle.autonomous = true;
    r.battle.ai.next_tick = u32::MAX;
    r.battle.actors[0].order = Order::Move;
    r.units[0].motion.destination = None;
    r.battle_tick();
    assert_eq!(r.battle.actors[0].order, Order::Move);
    assert_eq!(r.units[0].motion.destination, None);
    assert_eq!(r.battle.actors[0].target, Some(1));
}

#[test]
fn strategic_road_waypoints_reach_both_command_sectors() {
    let roads = RangeLayout::from_ron(include_str!("../../../data/range-km.ron"))
        .unwrap()
        .roads;
    for (mut from, goal) in [([1460, 760], [8820, 8870]), ([8900, 8650], [1530, 630])] {
        let mut visited = std::collections::BTreeSet::new();
        for _ in 0..100 {
            let Some(next) = ai::road_waypoint(&roads, from, goal) else {
                break;
            };
            assert_ne!(next, from);
            assert!(visited.insert(next), "road guidance looped at {next:?}");
            from = next;
        }
        assert!(
            (from[0] - goal[0]).abs() + (from[1] - goal[1]).abs() < 500,
            "road guidance stopped at {from:?}, goal {goal:?}"
        );
    }
    // The northern switchback used to keep siege artillery circling the
    // junction marker around (2930, 590) instead of taking the next leg.
    let next = ai::road_waypoint(&roads, [2833, 558], [8820, 8870]).unwrap();
    assert!(
        next[1] > 590,
        "artillery must enter the outgoing road: {next:?}"
    );
    let next = ai::road_waypoint(&roads, [4366, 773], [8820, 8870]).unwrap();
    assert!(
        next[0] > 4450,
        "a broad turn must not send the hull back: {next:?}"
    );
}

#[test]
fn external_ai_plan_is_validated_scheduled_and_saved() {
    let mut r = arena(RangeUnitKind::Tank);
    assert!(r.enemy_ai_observation().is_none());
    r.start_battle().unwrap();
    let observation = r.enemy_ai_observation().unwrap();
    assert_eq!(observation.version, OBSERVATION_VERSION);
    assert_eq!(observation.decision_tick, r.tick);
    assert!(r
        .submit_enemy_ai_plan(
            observation.decision_tick,
            vec![Decision {
                actor: 0,
                order: AiOrder::Hold
            }]
        )
        .is_err());
    assert!(r
        .submit_enemy_ai_plan(
            observation.decision_tick,
            vec![Decision {
                actor: 1,
                order: AiOrder::Attack(1)
            }]
        )
        .is_err());
    let plan = vec![Decision {
        actor: 1,
        order: AiOrder::Hold,
    }];
    r.submit_enemy_ai_plan(observation.decision_tick, plan.clone())
        .unwrap();
    assert!(r
        .submit_enemy_ai_plan(observation.decision_tick, plan)
        .is_err());
    let mut restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert_eq!(r.state_hash(), restored.state_hash());
    r.step();
    restored.step();
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(r.battle.actors[1].order, Order::Hold);
    assert!(r
        .submit_enemy_ai_plan(
            observation.decision_tick,
            vec![Decision {
                actor: 1,
                order: AiOrder::Auto
            }]
        )
        .is_err());
    for _ in 0..r.battle.ai.tuning.decision_ticks {
        r.step();
        restored.step();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
    assert_eq!(
        r.battle.actors[1].order,
        Order::Attack(0),
        "missing model plan must fall back to rules"
    );
}

#[test]
fn external_move_uses_the_regular_pathfinder_and_replays() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    let tick = r.enemy_ai_observation().unwrap().decision_tick;
    assert!(r
        .submit_enemy_ai_plan(
            tick,
            vec![Decision {
                actor: 1,
                order: AiOrder::Move([-1, 400]),
            }],
        )
        .is_err());
    r.submit_enemy_ai_plan(
        tick,
        vec![Decision {
            actor: 1,
            order: AiOrder::Move([480, 420]),
        }],
    )
    .unwrap();
    r.step();
    assert_eq!(r.battle.actors[1].order, Order::Move);
    assert!(r.units[1].motion.destination.is_some());
    let mut restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..3 {
        r.step();
        restored.step();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
}

#[test]
fn custom_enemy_rules_survive_snapshot() {
    let mut r = arena(RangeUnitKind::Tank);
    let mut tuning = r.battle.ai.tuning;
    tuning.decision_ticks = 8;
    tuning.focus_fire = false;
    assert!(r.configure_enemy_ai(tuning).is_ok());
    assert_eq!(r.battle.ai.tuning, tuning);
    let restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert_eq!(restored.battle.ai.tuning, tuning);
    assert_eq!(restored.state_hash(), r.state_hash());
    tuning.decision_ticks = 0;
    assert!(r.configure_enemy_ai(tuning).is_err());
}
#[test]
fn setup_is_quiet_and_commands_reject_enemy_and_friendly_fire_orders() {
    let mut r = arena(RangeUnitKind::Tank);
    for _ in 0..60 {
        r.step();
    }
    assert!(r.battle.rounds.is_empty());
    assert_eq!(r.battle.actors[0].fired, 0);
    assert!(!r.battle_order(0, Order::Attack(1), None));
    r.start_battle().unwrap();
    assert!(!r.battle_order(1, Order::Move, Some([400, 400])));
    assert!(!r.battle_order(0, Order::Attack(0), None));
    assert!(r.battle_order(0, Order::Attack(1), None));
    assert_eq!(r.battle.phase, Phase::Fighting);
}
#[test]
fn both_tanks_fire_and_take_real_spatial_damage() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    for _ in 0..750 {
        r.step();
    }
    assert!(
        r.battle.actors.iter().all(|a| a.fired > 0),
        "shots {:?}",
        r.battle.actors.iter().map(|a| a.fired).collect::<Vec<_>>()
    );
    assert!(
        r.units.iter().all(|u| u.revision > 0),
        "damage {:?}",
        r.units.iter().map(|u| u.revision).collect::<Vec<_>>()
    );
    assert!(r.matter.economy.ledger.balanced());
    assert!(r.battle.waves.len() <= r.battle.tuning.max_waves);
}

#[test]
fn artillery_drives_and_fires_high_explosive_by_default() {
    let mut r = arena_of([RangeUnitKind::Artillery, RangeUnitKind::Tank]);
    assert!(r.units[0].status.alive && r.units[0].status.engine && r.units[0].status.weapon);
    assert_eq!(r.battle_weapon(0), Some(Weapon::Blast));
    assert_eq!(r.battle_weapon(1), Some(Weapon::Kinetic));
    assert_eq!(
        r.units[0].body.extent_mm().0,
        voxel::unit::tank().extent_mm().0
    );
    assert_eq!(r.battle_muzzles(0).len(), 1);
    r.start_battle().unwrap();
    assert!(r.battle_order(0, Order::Move, Some([230, 336])));
    let start = r.units[0].at;
    for _ in 0..300 {
        r.step();
    }
    assert!(r.units[0].at[0] > start[0], "artillery did not drive");
    assert!(r.battle.actors[0].fired > 0, "artillery did not fire");
    assert!(r
        .battle
        .impacts
        .iter()
        .all(|i| i.weapon != Weapon::MachineGun));
}

#[test]
fn stationary_turrets_traverse_fire_and_refuse_move_orders() {
    for (kind, weapon) in [
        (RangeUnitKind::TurretT1, Weapon::Kinetic),
        (RangeUnitKind::TurretT2, Weapon::Energy),
    ] {
        let mut r = arena_of([kind, RangeUnitKind::Tank]);
        assert!(r.units[0].status.alive && r.units[0].status.weapon);
        assert!(!r.units[0].status.engine);
        assert_eq!(r.battle_weapon(0), Some(weapon));
        assert_eq!(r.battle_muzzles(0).len(), 1);
        r.start_battle().unwrap();
        r.battle.ai.next_tick = u32::MAX;
        r.battle.actors[1].order = Order::Hold;
        assert!(!r.battle_order(0, Order::Move, Some([200, 336])));
        assert!(r.battle_order(0, Order::Attack(1), None));
        let start = r.units[0].at;
        for _ in 0..350 {
            r.step();
        }
        assert_eq!(r.units[0].at, start, "{kind:?} left its emplacement");
        assert!(r.battle.actors[0].fired > 0, "{kind:?} did not fire");
        let restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
}
/// **A lobbed shell over the world ceiling comes back down.** The firing
/// solution rises `g·T²/8` above the muzzle; past the ceiling the round used
/// to retire as a miss, so every target in the far part of a tank's reach
/// was out of reach. A low ceiling and a long lane reproduce it: the tanks
/// stand 460 apart, and the shell tops out about 150 in a 128-high world.
#[test]
fn a_shell_lobbed_over_the_ceiling_still_lands() {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.size = [640, 128, 640];
    l.blocks.clear();
    l.shields.clear();
    l.course = None;
    l.clear_min = [64, 64];
    l.clear_max = [600, 600];
    l.units = [(100, true), (560, false)]
        .map(|(x, allied)| RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [x, 16, 336],
            allied,
        })
        .to_vec();
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        l,
    )
    .unwrap();
    // The lob is the high-explosive shell's (ADR-456).
    let high_explosive = |r: &mut BlastRange| {
        for actor in &mut r.battle.actors {
            actor.weapon = Some(Weapon::Blast);
        }
    };
    high_explosive(&mut r);
    r.start_battle().unwrap();
    let mut above = false;
    for _ in 0..750 {
        r.step();
        above |=
            r.battle.rounds.iter().any(|round| {
                (round.weapon != Weapon::MachineGun) && round.pos[1] >= Fx::from_int(128)
            });
    }
    assert!(
        above,
        "no shell rose over the ceiling: the lane is too short"
    );
    assert!(
        r.units.iter().all(|u| u.revision > 0),
        "a shell over the ceiling never landed: damage {:?}",
        r.units.iter().map(|u| u.revision).collect::<Vec<_>>()
    );
    let mut b = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    high_explosive(&mut b);
    for _ in 0..60 {
        r.step();
        b.step();
    }
    assert_eq!(r.state_hash(), b.state_hash());
}

#[test]
fn dual_machine_guns_keep_owner_and_survive_stop() {
    let mut r = arena(RangeUnitKind::Strider);
    r.start_battle().unwrap();
    for _ in 0..15 {
        r.step();
    }
    let fired = r.battle.actors[0].fired;
    assert!(fired > 0);
    assert!(r.battle_order(0, Order::Hold, None));
    let mut hits = [0; 2];
    let mut last = Vec::new();
    for _ in 0..150 {
        r.step();
        for t in &r.battle.traces {
            hits[usize::from(t.hit)] += 1;
            last = vec![t.from, t.to];
        }
    }
    assert_eq!(r.battle.actors[0].fired, fired);
    assert!(r.units[0].revision > 0, "enemy never shot back: fired={:?} hits={hits:?} last={last:?} turrets={:?} targets={:?}",
        r.battle.actors.iter().map(|a|a.fired).collect::<Vec<_>>(),
        r.battle.actors.iter().map(|a|a.turret.angle).collect::<Vec<_>>(),
        r.battle.actors.iter().map(|a|a.target).collect::<Vec<_>>());
    assert!(
        r.units[1].revision > 0,
        "own bullets disappeared after stop"
    );
    assert!(r.matter.economy.ledger.balanced());
}
#[test]
fn replay_with_multiple_projectiles_and_waves() {
    // Explicitly equip both tanks with high-explosive rounds: new tanks
    // otherwise start with armour-piercing ammunition.
    let high_explosive = |r: &mut BlastRange| {
        slow_blasts(r);
        for actor in &mut r.battle.actors {
            actor.weapon = Some(Weapon::Blast);
        }
    };
    let mut a = arena(RangeUnitKind::Tank);
    high_explosive(&mut a);
    a.start_battle().unwrap();
    for _ in 0..180 {
        a.step();
        if !a.battle.rounds.is_empty() {
            break;
        }
    }
    assert!(!a.battle.rounds.is_empty());
    let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
    high_explosive(&mut b);
    assert_eq!(a.state_hash(), b.state_hash());
    let mut waves = 0;
    for _ in 0..150 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        waves = waves.max(a.battle.waves.len());
    }
    assert!(waves > 0, "the replay carried no wave");
}

// Explicit delayed-wave fixture for save/cursor tests. Stock detonations may
// finish during impact; every restored fixture must use these same rules.
pub(super) fn slow_blasts(r: &mut BlastRange) {
    for c in [
        &mut r.battle.shell_config,
        &mut r.battle.canister_config,
        &mut r.battle.energy_config,
        &mut r.battle.anni_config,
    ] {
        c.wave_work_per_tick = 256;
    }
}
#[test]
fn cover_blocks_enemy_machine_gun_and_winner_is_reported() {
    let mut r = arena(RangeUnitKind::Strider);
    for x in 295..305 {
        for y in 16..110 {
            for z in 260..410 {
                r.world.queue_edit(voxel::Edit::put(x, y, z, 3));
            }
        }
    }
    while r.world.queued() > 0 {
        r.world.flush();
    }
    r.start_battle().unwrap();
    // Isolate cover interception; faster walkers can otherwise flank this wall.
    for unit in &mut r.units {
        unit.motion.speed = 0;
    }
    for _ in 0..150 {
        r.step();
    }
    assert!(
        r.units.iter().all(|u| u.revision == 0),
        "bullets ignored cover"
    );
    r.units[1].status.alive = false;
    r.battle.rounds.clear();
    r.battle_tick();
    assert_eq!(r.battle.phase, Phase::Victory);
}
#[test]
fn manual_move_preserves_identity_and_actually_drives() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    let before = r.units[0].at;
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Move, Some([230, 430])));
    for _ in 0..180 {
        r.step();
    }
    assert!(r.units[0].at[2] > before[2] + 20);
    assert!(r.allied(0));
    assert!(!r.allied(1));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn every_battle_chassis_can_fire_during_a_move_order() {
    for kind in [
        RangeUnitKind::Tank,
        RangeUnitKind::Strider,
        RangeUnitKind::Spider,
    ] {
        let mut r = arena_of([kind, RangeUnitKind::Tank]);
        r.start_battle().unwrap();
        r.battle.ai.next_tick = u32::MAX;
        r.battle.actors[1].order = Order::Hold;
        // Isolate firing on an established route: a siege beam can end this
        // tiny duel before the first route search has finished.
        r.units[0].status.weapon = false;
        r.units[1].status.weapon = false;
        assert!(r.battle_order(0, Order::Move, Some([260, 430])), "{kind:?}");
        let start = r.units[0].motion.pos;
        for _ in 0..180 {
            r.step();
            if r.units[0].motion.pos != start {
                break;
            }
        }
        assert_ne!(r.units[0].motion.pos, start, "{kind:?} never moved");
        assert!(r.units[0].motion.destination.is_some());
        r.units[0].status.weapon = true;
        let mut fired_while_moving = false;
        for _ in 0..180 {
            assert_eq!(r.battle.actors[0].order, Order::Move);
            let before = r.battle.actors[0].fired;
            r.step();
            if r.battle.actors[0].fired > before
                && r.units[0].motion.pos != start
                && r.units[0].motion.destination.is_some()
            {
                fired_while_moving = true;
                break;
            }
        }
        assert!(fired_while_moving, "{kind:?} did not shoot on its route");
    }
}

#[test]
fn enemy_ai_move_orders_keep_firing_for_every_chassis() {
    for kind in [
        RangeUnitKind::Tank,
        RangeUnitKind::Strider,
        RangeUnitKind::Spider,
    ] {
        let mut r = arena_of([RangeUnitKind::Tank, kind]);
        r.start_battle().unwrap();
        r.battle.actors[0].order = Order::Hold;
        r.units[0].status.weapon = false;
        r.units[1].status.weapon = false;
        let tick = r.enemy_ai_observation().unwrap().decision_tick;
        r.submit_enemy_ai_plan(
            tick,
            vec![Decision {
                actor: 1,
                order: AiOrder::Move([360, 430]),
            }],
        )
        .unwrap();
        r.step();
        assert_eq!(r.battle.actors[1].order, Order::Move);
        r.battle.ai.next_tick = u32::MAX;
        let start = r.units[1].motion.pos;
        for _ in 0..180 {
            r.step();
            if r.units[1].motion.pos != start {
                break;
            }
        }
        assert_ne!(r.units[1].motion.pos, start, "enemy {kind:?} never moved");
        assert!(r.units[1].motion.destination.is_some());
        r.units[1].status.weapon = true;
        let mut fired_while_moving = false;
        for _ in 0..180 {
            let before = r.battle.actors[1].fired;
            r.step();
            fired_while_moving |= r.battle.actors[1].fired > before
                && r.units[1].motion.pos != start
                && r.units[1].motion.destination.is_some();
            if fired_while_moving {
                break;
            }
        }
        assert!(
            fired_while_moving,
            "enemy {kind:?} stopped shooting on the move"
        );
    }
}

#[test]
fn attack_order_keeps_an_existing_battle_route() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    r.battle.ai.next_tick = u32::MAX;
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Move, Some([260, 430])));
    let route = r.units[0].motion.destination;
    assert!(r.battle_order(0, Order::Attack(1), None));
    assert_eq!(r.units[0].motion.destination, route);
    let mut shot = false;
    for _ in 0..180 {
        let before = r.battle.actors[0].fired;
        r.step();
        shot |= r.battle.actors[0].fired > before && r.units[0].motion.destination.is_some();
    }
    assert!(shot, "explicit attack stopped the moving tank");
}

#[test]
fn ground_fire_keeps_an_existing_battle_route() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    r.battle.ai.next_tick = u32::MAX;
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Move, Some([260, 430])));
    let route = r.units[0].motion.destination;
    assert!(r.battle_order(0, Order::Ground([450, 16, 336]), None));
    assert_eq!(r.units[0].motion.destination, route);
    let mut shot = false;
    for _ in 0..180 {
        let before = r.battle.actors[0].fired;
        r.step();
        shot |= r.battle.actors[0].fired > before && r.units[0].motion.destination.is_some();
    }
    assert!(shot, "ground fire stopped the moving tank");
}

#[test]
fn simultaneous_impacts_keep_both_waves_and_replay() {
    let mut a = arena(RangeUnitKind::Tank);
    slow_blasts(&mut a);
    a.start_battle().unwrap();
    for actor in &mut a.battle.actors {
        actor.order = Order::Hold;
    }
    for owner in 0..2 {
        let u = &a.units[1 - owner];
        let p = u
            .world_pos([
                u.body.core.0 as usize,
                u.body.core.1 as usize,
                u.body.core.2 as usize,
            ])
            .map(Fx::from_int);
        a.battle.rounds.push(Round {
            owner,
            weapon: Weapon::Blast,
            pos: p,
            previous: p,
            velocity: [Fx::ZERO; 3],
            distance: Fx::from_int(100),
        });
    }
    a.step();
    assert_eq!(a.battle.waves.len(), 2, "one impact replaced another");
    let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
    slow_blasts(&mut b);
    for _ in 0..50 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
    }
    assert!(a.units.iter().all(|u| u.revision > 0));
    assert!(a.matter.economy.ledger.balanced());
}

/// Auto ammunition remains available when the player selects "by target";
/// new tanks otherwise start with armour-piercing rounds.
#[test]
fn a_tank_loads_armour_piercing_for_a_tank_and_high_explosive_for_a_walker() {
    for (enemy, kinetic) in [(RangeUnitKind::Tank, true), (RangeUnitKind::Strider, false)] {
        let mut r = arena_of([RangeUnitKind::Tank, enemy]);
        std::sync::Arc::make_mut(&mut r.battle.tuning).tank_ammo =
            modding::range_battle::TankAmmo {
                tank: Delivery::Kinetic,
                strider: Delivery::Blast,
                ground: Delivery::Blast,
            };
        assert!(r.set_battle_weapon(0, None));
        r.start_battle().unwrap();
        // The enemy stands and holds its fire.
        r.battle.ai.next_tick = u32::MAX;
        r.battle.actors[1].order = Order::Hold;
        let fired = (0..600).find_map(|_| {
            r.step();
            r.battle.rounds.iter().find(|x| x.owner == 0).map(|x| {
                (
                    (x.weapon != Weapon::MachineGun),
                    (x.weapon == Weapon::Kinetic),
                )
            })
        });
        assert_eq!(fired, Some((true, kinetic)), "against a {enemy:?}");
    }
}

/// **The battle's armour-piercing round breaks the hull along its line,
/// with no wave** (ADR-456), and keeps its kind through a snapshot.
#[test]
fn a_battle_armour_piercing_round_breaks_the_hull_without_a_wave() {
    let mut a = arena(RangeUnitKind::Tank);
    a.start_battle().unwrap();
    // The enemy holds until its next decision, after the test.
    a.battle.ai.next_tick = a.tick + a.battle.ai.tuning.decision_ticks;
    for actor in &mut a.battle.actors {
        actor.order = Order::Hold;
    }
    let u = &a.units[1];
    let core = u
        .world_pos([
            u.body.core.0 as usize,
            u.body.core.1 as usize,
            u.body.core.2 as usize,
        ])
        .map(Fx::from_int);
    // Level, forty units short of the core, at the round's own speed.
    let from = [core[0].sub(Fx::from_int(40)), core[1], core[2]];
    let speed = Fx::from_int(a.battle.kinetic_config.shell_speed);
    a.battle.rounds.push(Round {
        owner: 0,
        weapon: Weapon::Kinetic,
        pos: from,
        previous: from,
        velocity: [speed, Fx::ZERO, Fx::ZERO],
        distance: Fx::from_int(400),
    });
    let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
    assert!(
        (b.battle.rounds[0].weapon == Weapon::Kinetic),
        "the round lost its kind"
    );
    let whole = a.units[1].removed.len();
    let mut struck = false;
    for _ in 0..10 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        struck |= a.battle.impacts.iter().any(|i| i.weapon == Weapon::Kinetic);
    }
    assert!(struck, "no armour-piercing impact");
    assert!(a.battle.waves.is_empty(), "the solid round started a wave");
    assert!(a.units[1].removed.len() > whole, "the hull was not broken");
    assert!(a.battle.rounds.is_empty());
    assert!(a.matter.economy.ledger.balanced());
}

/// **The battle's armour-piercing round glances off a hull's side as the
/// single gun's does** (ADR-454, ADR-456): it flies on, turned off the
/// plate, and the plate keeps a dent, not a hole.
#[test]
fn a_battle_armour_piercing_round_glances_off_a_hull_side() {
    let mut a = arena(RangeUnitKind::Tank);
    a.start_battle().unwrap();
    a.battle.ai.next_tick = a.tick + a.battle.ai.tuning.decision_ticks;
    for actor in &mut a.battle.actors {
        actor.order = Order::Hold;
    }
    // The side belt of the ADR-454 tests, in the enemy hull's own frame.
    let u = &a.units[1];
    let cell = [30, 11, 0];
    assert_eq!(u.body.part_at(30, 11, 0), voxel::Part::Armour);
    let f = u.root_frame(cell[1]);
    let centre = f.world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
    // A hair outside the plate, drifting into it at a shallow angle.
    let start = centre
        .add(f.x.scale(Fx::from_ratio(1, 10)))
        .sub(f.z.scale(Fx::from_ratio(13, 20)));
    let start = [start.x, start.y, start.z];
    assert!(!u.solid(start.map(Fx::to_int_trunc)));
    let v =
        f.x.scale(Fx::from_int(280))
            .add(f.z.scale(Fx::from_int(40)));
    a.battle.rounds.push(Round {
        owner: 0,
        weapon: Weapon::Kinetic,
        pos: start,
        previous: start,
        velocity: [v.x, v.y, v.z],
        distance: Fx::from_int(400),
    });
    a.step();
    assert!(
        a.battle.impacts.iter().any(|i| i.weapon == Weapon::Kinetic),
        "the round never touched the hull"
    );
    let round = a.battle.rounds.iter().find(|r| r.owner == 0);
    let round = round.expect("the round stopped in the hull");
    let turned = f.z.dot(phys::V3::new(
        round.velocity[0],
        round.velocity[1],
        round.velocity[2],
    ));
    assert!(turned < Fx::ZERO, "the round was not turned off the plate");
    assert!(
        a.units[1].removed.is_empty(),
        "a glancing round bored the hull"
    );
    assert!(!a.units[1].dents.is_empty());
}

#[test]
fn rotated_turret_cells_are_hit_where_they_are_drawn() {
    let mut r = arena(RangeUnitKind::Tank);
    let u = &mut r.units[0];
    u.update_pose(fx::angle::FULL / 4);
    let mut tested = 0;
    for x in 40..u.body.vol.nx as usize {
        for y in u.body.turret_y as usize..u.body.vol.ny as usize {
            for z in 0..u.body.vol.nz as usize {
                if u.body.vol.get(x, y, z) {
                    let p = u.world_pos([x, y, z]);
                    assert_eq!(u.hit(p), Some((0, [x, y, z])));
                    tested += 1;
                }
            }
        }
    }
    assert!(tested > 10);
}

#[test]
fn aim_follows_surviving_core_instead_of_shooting_through_a_hole() {
    let mut r = arena(RangeUnitKind::Strider);
    let u = &mut r.units[1];
    let first = aim_point(u);
    let (_, [x, y, z]) = u.hit(first).expect("aim must hit solid body");
    assert!(u.body.remove(x, y, z));
    assert!(!u.solid(first));
    let next = aim_point(u);
    assert_ne!(next, first);
    assert!(u.solid(next));
}

#[test]
fn auto_approaches_when_out_of_range_and_dead_order_returns_to_auto() {
    let mut r = arena(RangeUnitKind::Tank);
    std::sync::Arc::make_mut(&mut r.battle.tuning).tank_range = 100;
    std::sync::Arc::make_mut(&mut r.battle.tuning).kinetic_range = 100;
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    let before = r.units[0].at;
    for _ in 0..60 {
        r.step();
    }
    assert!(r.units[0].at[0] > before[0] + 5);
    assert_eq!(r.battle.actors[0].fired, 0, "cannot shoot outside range");
    r.battle.actors[0].order = Order::Attack(1);
    r.units[1].status.alive = false;
    r.battle_tick();
    assert_eq!(r.battle.actors[0].order, Order::Auto);
    assert_eq!(r.battle.phase, Phase::Victory);
}

#[test]
fn snapshot_rejects_projectile_positions_outside_the_range() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    for _ in 0..180 {
        r.step();
        if !r.battle.rounds.is_empty() {
            break;
        }
    }
    assert!(!r.battle.rounds.is_empty());
    r.battle.rounds[0].pos[0] = Fx::from_int(1_000_000);
    assert!(BlastRange::restore(r.config, &r.snapshot()).is_none());
}

#[test]
fn a_far_target_does_not_restart_its_route_search_on_each_pursuit_tick() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    layout.units = vec![
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [128, 16, 336],
            allied: true,
        },
        RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [1480, 16, 460],
            allied: false,
        },
    ];
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap(),
        layout,
    )
    .unwrap();
    let tuning = std::sync::Arc::make_mut(&mut r.battle.tuning);
    tuning.tank_range = 100;
    tuning.kinetic_range = 100;
    tuning.pursuit_ticks = 1;
    r.start_battle().unwrap();
    r.units[0].motion.set_global_search_limit(0);
    r.step();
    r.units[0]
        .motion
        .set_global_search_limit(r.navigation.global_nodes);
    assert!(r.units[0].motion.searching(), "route was trivial");
    for _ in 0..30 {
        r.step();
        if r.units[0].motion.frontier_nodes() > 0 || !r.units[0].motion.path.is_empty() {
            break;
        }
    }
    assert!(
        r.units[0].motion.frontier_nodes() > 0 || !r.units[0].motion.path.is_empty(),
        "search never progressed: nodes={}, searching={}, destination={:?}, last_plan={:?}",
        r.units[0].motion.frontier_nodes(),
        r.units[0].motion.searching(),
        r.units[0].motion.destination,
        r.units[0].motion.last_plan
    );
}

/// **A long pursuit reaches targets that keep moving** (P-56, ADR-462).
/// A tank with a short reach sets out across the training ground after
/// two tanks that drive back and forth and never stop. Each new goal of
/// the pursuit is searched while the hull drives on: it takes the new
/// route up at speed instead of stopping for it, closes to its reach
/// again and again and fires; the run replays bit for bit from a snapshot
/// taken in the middle of the chase.
#[test]
fn a_long_pursuit_reaches_targets_that_keep_moving() {
    long_pursuit(false, false);
}

#[test]
fn a_long_pursuit_with_shared_search_budget_reaches_moving_targets() {
    long_pursuit(true, false);
}

#[test]
fn whole_map_routes_pursue_targets_with_immediate_move_orders_and_replay() {
    long_pursuit(true, true);
}

fn long_pursuit(borrow_idle: bool, global: bool) {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    let unit = |at: [i32; 3], allied| RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Tank,
        at,
        allied,
    };
    layout.units = vec![
        unit([3260, 16, 2080], true),
        unit([3900, 16, 2080], false),
        unit([3900, 16, 2760], false),
    ];
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut a = BlastRange::with_layout(config, layout).unwrap();
    // A short reach: the pursuer has to close in on each target in turn.
    let short = |r: &mut BlastRange| {
        let tuning = std::sync::Arc::make_mut(&mut r.battle.tuning);
        tuning.tank_range = 150;
        tuning.kinetic_range = 150;
        // This measures pursuit, not lethality. Keep both moving targets
        // alive for the whole route and replay even as stock ammunition evolves.
        r.battle.kinetic_config.energy = 1;
        if !global {
            // Keep the suspended-frontier regressions on the fine planner
            // for both pursuer and targets. Instant target orders change
            // their trajectory too, so test that case independently.
            for unit in &mut r.units {
                unit.motion.set_global_search_limit(0);
            }
        }
        if !borrow_idle {
            // Preserve the original slow-frontier regression: multiple
            // route replacements must complete while the hull is at speed.
            r.navigation.search_resume_per_unit = r.navigation.search_nodes_per_tick;
        }
    };
    short(&mut a);
    a.start_battle().unwrap();
    // The targets shuttle between two points each and never stop.
    let legs = [[[3900, 2480], [3900, 2080]], [[3450, 2760], [3900, 2760]]];
    let steer = |r: &mut BlastRange| {
        if r.battle.ai.pending.is_some() || r.battle.ai.next_tick < r.tick {
            return;
        }
        let leg = (r.tick / 300) as usize % 2;
        let plan: Vec<_> = [1usize, 2]
            .into_iter()
            .filter(|&id| r.units[id].status.alive)
            .map(|id| Decision {
                actor: id,
                order: AiOrder::Move(legs[id - 1][leg]),
            })
            .collect();
        let tick = r.battle.ai.next_tick;
        r.submit_enemy_ai_plan(tick, plan).unwrap();
    };
    let start = a.units[0].motion.pos;
    let cruise = Fx::from_int(a.units[0].motion.speed / 2);
    let (mut restarts, mut taken_at_speed, mut first_shot) = (0, 0, None);
    let mut replay = None;
    for tick in 0..2400 {
        steer(&mut a);
        let searching = a.units[0].motion.searching();
        let previous_goal = a.units[0].motion.destination;
        a.step();
        let m = &a.units[0].motion;
        restarts += usize::from(m.started_search());
        // A deferred or immediate route to a new goal is accepted while
        // the hull is driving at more than half its speed.
        taken_at_speed += usize::from(
            (searching || previous_goal != m.destination)
                && !m.searching()
                && m.path.last().copied() == m.destination
                && m.drive.vel > cruise,
        );
        if a.battle.actors[0].fired > 0 {
            first_shot.get_or_insert(tick);
        }
        if let Some(b) = replay.as_mut().filter(|_| tick < 1500) {
            steer(b);
            b.step();
            assert_eq!(a.state_hash(), b.state_hash(), "diverged at {tick}");
        }
        if tick == 1200 {
            let mut b = BlastRange::restore(config, &a.snapshot()).unwrap();
            short(&mut b);
            replay = Some(b);
        }
    }
    let travelled = (a.units[0].motion.pos[0].sub(start[0]).to_int_trunc()).abs()
        + (a.units[0].motion.pos[2].sub(start[2]).to_int_trunc()).abs();
    eprintln!("pursuit borrowed={borrow_idle} first_shot={first_shot:?} restarts={restarts} moving_replacements={taken_at_speed} shots={}", a.battle.actors[0].fired);
    assert!(
        travelled > 400,
        "the pursuer stayed near its start: {travelled}"
    );
    // Stopping for every new goal and restarting the search on every
    // tick of braking, the pursuer started 139 searches and fired first
    // at tick 1137; holding the search but still stopping for it, it
    // fired first at 1665 and six rounds in all; then 589 and fifteen.
    //
    // Requalified with the braking-curve turn (ADR-648). The bounds held
    // one trajectory, not a property: shifting only the targets' shuttle
    // period (240…360 ticks) moves the first shot under either turn law.
    //
    // | variant | old turn: first shot mean / worst, shots | new turn |
    // |---|---|---|
    // | fine planner  | 952 / 1242, 11.6  | 1101 / 1229, 11.0 |
    // | whole-map     | 1263 / 1878, 9.4  | 1255 / 1688, 9.4  |
    //
    // At this period the old turn fired first at 700 (fine) and 983
    // (whole-map); the new one at 976 and 1688. Only the fine-planner
    // bounds reject the regressions above: the whole-map bounds hold the
    // measured spread and would pass the second one (1665, six rounds),
    // which lies inside it.
    assert!(
        taken_at_speed >= if borrow_idle { 1 } else { 2 },
        "routes taken up at speed: {taken_at_speed}"
    );
    assert!(
        restarts <= 10,
        "the route search restarted {restarts} times"
    );
    assert!(
        first_shot.is_some_and(|t| t < if global { 1750 } else { 1100 }),
        "first shot at {first_shot:?}"
    );
    assert!(
        a.battle.actors[0].fired >= if global { 5 } else { 10 },
        "fired {}",
        a.battle.actors[0].fired
    );
}

/// **Sixteen machines fight the training battle** (ADR-466, P-55). The
/// stock four of the training battle in four groups, the layout's limit:
/// both sides fire and take damage, and the fight replays bit for bit
/// from a snapshot taken in its middle.
#[test]
fn sixteen_machines_fight_the_training_battle_and_replay() {
    let t = modding::range_battle::Tuning::from_ron(include_str!("../../../data/range-battle.ron"))
        .unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    layout.gun = t.zone.spawn;
    layout.target = t.zone.target;
    layout.zones.push(t.zone.clone());
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    layout.units = (0..4)
        .flat_map(|k: i32| {
            let (dx, dz) = (70 * (k / 2), 90 * (k % 2));
            t.units.iter().cloned().map(move |mut u| {
                u.at[0] += if u.allied { -dx } else { dx };
                u.at[2] += dz;
                u
            })
        })
        .collect();
    assert_eq!(layout.units.len(), 16);
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut a = BlastRange::with_layout(config, layout).unwrap();
    a.start_battle().unwrap();
    for _ in 0..360 {
        a.step();
    }
    let mut b = BlastRange::restore(config, &a.snapshot()).unwrap();
    assert!(b.set_navigation_workers(1));
    for tick in 0..90 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash(), "diverged at {tick}");
    }
    for side in [true, false] {
        assert!(
            (0..a.units.len()).any(|id| a.allied(id) == side && a.battle.actors[id].fired > 0),
            "a side never fired"
        );
        assert!(
            (0..a.units.len()).any(|id| a.allied(id) == side && a.units[id].revision > 0),
            "a side took no damage"
        );
    }
}

#[test]
fn a_moving_target_changes_the_finished_pursuit_goal() {
    let mut r = arena(RangeUnitKind::Tank);
    let tuning = std::sync::Arc::make_mut(&mut r.battle.tuning);
    tuning.tank_range = 100;
    tuning.kinetic_range = 100;
    tuning.pursuit_ticks = 1;
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.step();
    for _ in 0..30 {
        if !r.units[0].motion.searching() {
            break;
        }
        r.step();
    }
    assert!(!r.units[0].motion.searching());
    let before = r.units[0].motion.destination.expect("initial approach");
    r.units[1].motion.pos[0] = Fx::from_int(520);
    let head = r.battle.actors[1].turret.angle;
    r.units[1].update_pose(head);
    r.step();
    let after = r.units[0].motion.destination.expect("updated approach");
    assert!(after[0] > before[0] + 25, "{before:?} -> {after:?}");
}

/// **Tank squads crossing the field all engage** (ADR-467, P-63). Four
/// tanks a side in two columns start from opposite corners of the
/// training ground, out of range, and all attack one target a side. The
/// allies lined up with the target used to drive for the same firing
/// position and jostle there, and a tank boxed in by its standing allies
/// crawled a weaving partial route long after they had driven off: two
/// never fired in 1500 ticks and one fired at tick 1356. Every tank fires
/// within 700 ticks now, and the firing positions of one side keep the
/// data's spacing.
#[test]
fn tank_squads_crossing_the_field_all_engage() {
    let t = modding::range_battle::Tuning::from_ron(include_str!("../../../data/range-battle.ron"))
        .unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    layout.gun = t.zone.spawn;
    layout.target = t.zone.target;
    layout.zones.push(t.zone.clone());
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    layout.units = [true, false]
        .into_iter()
        .flat_map(|allied| {
            (0..4).map(move |k| {
                let (dx, dz) = (80 * (k % 2), 80 * (k / 2));
                RangeUnitDef {
                    weapon: None,
                    kind: RangeUnitKind::Tank,
                    allied,
                    at: if allied {
                        [3240 + dx, 16, 2060 + dz]
                    } else {
                        [4060 - dx, 16, 2740 - dz]
                    },
                }
            })
        })
        .collect();
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut a = BlastRange::with_layout(config, layout).unwrap();
    // Keep the focus targets alive until every pursuer has found a firing
    // position; stock lethality has independent frontal-hit regressions.
    a.battle.kinetic_config.energy = 1;
    a.start_battle().unwrap();
    let spacing = i64::from(a.battle.tuning.firing_spacing);
    let mut first = vec![None; a.units.len()];
    for tick in 0..700 {
        a.step();
        for (id, shot) in first.iter_mut().enumerate() {
            if a.battle.actors[id].fired > 0 {
                shot.get_or_insert(tick);
            }
        }
        // Firing positions of one side keep their spacing.
        for i in 0..a.units.len() {
            for j in i + 1..a.units.len() {
                if a.allied(i) != a.allied(j) {
                    continue;
                }
                if let (Some(p), Some(q)) =
                    (a.units[i].motion.destination, a.units[j].motion.destination)
                {
                    let (x, z) = (i64::from(p[0] - q[0]), i64::from(p[1] - q[1]));
                    assert!(
                        x * x + z * z >= spacing * spacing,
                        "tick {tick}: firing positions of {i} and {j} too close: {p:?} {q:?}"
                    );
                }
            }
        }
    }
    assert!(first.iter().all(Option::is_some), "first shots: {first:?}");
}

#[test]
fn manual_ammunition_preserves_cooldown_order_and_snapshot() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    r.battle.actors[0].order = Order::Attack(1);
    r.battle.actors[0].next = 123;
    assert!(r.set_battle_weapon(0, Some(Weapon::Blast)));
    assert_eq!(r.battle.actors[0].next, 123);
    assert_eq!(r.battle.actors[0].order, Order::Attack(1));
    assert_eq!(r.battle_attack_range(0), Some(r.battle.tuning.tank_range));
    assert!(!r.set_battle_weapon(1, Some(Weapon::Blast)));
    assert!(!r.set_battle_weapon(0, Some(Weapon::MachineGun)));
    assert!(r.set_battle_weapon(0, Some(Weapon::Kinetic)));
    assert_eq!(
        r.battle_attack_range(0),
        Some(r.battle.tuning.kinetic_range)
    );
    let restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(restored.battle.actors[0].weapon, Some(Weapon::Kinetic));
    assert!(r.set_battle_weapon(0, None));
    let mut walker = arena(RangeUnitKind::Strider);
    assert!(!walker.set_battle_weapon(0, Some(Weapon::Kinetic)));
    assert_eq!(
        walker.battle_attack_range(0),
        Some(walker.streams.tuning.machine_gun.range)
    );
}

#[test]
fn artillery_high_explosive_outreaches_energy_turrets_but_other_ammo_keeps_its_range() {
    let mut battle = arena(RangeUnitKind::Artillery);
    assert_eq!(battle.battle_weapon(0), Some(Weapon::Blast));
    assert_eq!(
        battle.battle_attack_range(0),
        Some(battle.battle.tuning.artillery_range)
    );
    assert!(battle.battle.tuning.artillery_range > battle.battle.tuning.energy_range);
    assert!(battle.set_battle_weapon(0, Some(Weapon::Kinetic)));
    assert_eq!(
        battle.battle_attack_range(0),
        Some(battle.battle.tuning.kinetic_range)
    );
}

#[test]
fn manually_selected_shell_is_fired_even_at_a_tank() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    r.battle.ai.next_tick = u32::MAX;
    r.battle.actors[1].order = Order::Hold;
    assert!(r.set_battle_weapon(0, Some(Weapon::Blast)));
    let round = (0..180).find_map(|_| {
        r.step();
        r.battle.rounds.iter().find(|p| p.owner == 0).map(|p| {
            (
                (p.weapon != Weapon::MachineGun),
                (p.weapon == Weapon::Kinetic),
            )
        })
    });
    assert_eq!(round, Some((true, false)));
    assert!(r.set_battle_weapon(0, Some(Weapon::Kinetic)));
    assert!(r
        .battle
        .rounds
        .iter()
        .any(|p| p.owner == 0 && p.weapon != Weapon::Kinetic));
}

#[test]
fn weapon_range_controls_pursuit_and_old_snapshots_default_to_auto() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    r.battle.ai.next_tick = u32::MAX;
    r.battle.actors[1].order = Order::Hold;
    let t = std::sync::Arc::make_mut(&mut r.battle.tuning);
    t.tank_range = 100;
    t.kinetic_range = 400;
    assert_eq!(r.battle_weapon(0), Some(Weapon::Kinetic));
    r.step();
    assert!(r.units[0].motion.destination.is_none());
    assert!(r.set_battle_weapon(0, Some(Weapon::Blast)));
    r.step();
    assert!(r.units[0].motion.destination.is_some());
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    let mut old = r.snapshot();
    old[8..12].copy_from_slice(&41u32.to_le_bytes());
    old.truncate(old.len() - r.battle.actors.len() - 2);
    let old = BlastRange::restore(r.config, &old).unwrap();
    assert!(old.battle.actors.iter().all(|a| a.weapon.is_none()));
}

#[test]
fn battle_machine_guns_use_paired_finite_flight_and_shared_burst_timing() {
    let mut r = arena(RangeUnitKind::Strider);
    r.start_battle().unwrap();
    r.battle.ai.next_tick = u32::MAX;
    r.battle.actors[1].order = Order::Hold;
    // Empty elevated point: measure speed and cadence without a destroyed target
    // ending the fight, or the opponent damaging a barrel between salvos.
    assert!(r.battle_order(0, Order::Ground([540, 100, 336]), None));
    let tuning = r.streams.tuning.machine_gun.clone();
    let speed = r.battle.bullet_config.shell_speed;
    let mut shots = Vec::new();
    for _ in 0..180 {
        let before = r.battle.actors[0].fired;
        r.step();
        if r.battle.actors[0].fired != before {
            shots.push(r.tick);
            if shots.len() == 1 {
                let rounds: Vec<_> = r.battle.rounds.iter().filter(|p| p.owner == 0).collect();
                assert_eq!(rounds.len(), 2, "both barrels must emit a live bullet");
                assert_ne!(rounds[0].previous, rounds[1].previous);
                for round in rounds {
                    let length = round
                        .velocity
                        .iter()
                        .fold(Fx::ZERO, |n, v| n.add(v.mul(*v)))
                        .sqrt();
                    assert!((length.sub(Fx::from_int(speed))).abs() < Fx::ONE);
                    assert!(
                        round.pos[0] < Fx::from_int(540),
                        "hit before the bullet arrived"
                    );
                }
            }
        }
        if shots.len() == 16 {
            break;
        }
    }
    assert_eq!(shots.len(), 16, "a full burst plus its next opening salvo");
    for (i, &shot) in shots[..15].iter().enumerate() {
        assert_eq!(
            shot - shots[0],
            modding::clock::stream_ticks(i as u32 * tuning.interval_ticks)
        );
    }
    assert_eq!(
        shots[15] - shots[0],
        modding::clock::stream_ticks(tuning.burst_ticks - 1) + tuning.reload_ticks
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn energy_weapons_keep_identity_in_flight_and_replay_their_own_wave() {
    for weapon in [Weapon::Energy, Weapon::Annihilation] {
        let mut r = arena(RangeUnitKind::Tank);
        slow_blasts(&mut r);
        r.start_battle().unwrap();
        r.battle.actors[1].order = Order::Hold;
        r.battle.ai.next_tick = u32::MAX;
        assert!(r.set_battle_weapon(0, Some(weapon)));
        assert!(r.battle_order(0, Order::Attack(1), None));
        for _ in 0..160 {
            r.step();
            if !r.battle.rounds.is_empty() {
                break;
            }
        }
        let shot = r.battle.rounds.first().expect("energy charge never fired");
        assert_eq!(shot.weapon, weapon);
        let velocity = shot.velocity;
        let next = r.battle.actors[0].next;
        assert!(r.set_battle_weapon(0, Some(Weapon::Kinetic)));
        assert_eq!(r.battle.actors[0].next, next);
        assert!(r.battle_order(0, Order::Hold, None));
        r.step();
        if let Some(shot) = r.battle.rounds.first() {
            assert_eq!(shot.weapon, weapon);
            assert_eq!(shot.velocity, velocity, "direct bolt gained gravity");
        }
        // A valid future AI deadline for snapshot validation, with quiet enemies.
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        let mut replay = BlastRange::restore(r.config, &r.snapshot()).expect("in-flight snapshot");
        slow_blasts(&mut replay);
        let mut impact = false;
        let mut wave = false;
        for _ in 0..120 {
            r.step();
            replay.step();
            assert_eq!(r.state_hash(), replay.state_hash());
            impact |= r.battle.impacts.iter().any(|i| i.weapon == weapon);
            if r.battle.waves.iter().any(|w| w.weapon == weapon) {
                wave = true;
                // Restore during the delayed wave as well, not just during flight.
                replay = BlastRange::restore(r.config, &r.snapshot()).expect("typed wave snapshot");
                slow_blasts(&mut replay);
            }
        }
        assert!(impact && wave, "bolt never produced its typed impact/wave");
        assert!(
            r.units[1].annihilated_mm3() > 0,
            "energy wave did no ablative work"
        );
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn energy_loadouts_share_direct_reach_and_preserve_old_snapshot_choices() {
    let mut r = arena(RangeUnitKind::Tank);
    for weapon in [Weapon::Energy, Weapon::Annihilation] {
        let range = modding::attack_ranges::stock().energy;
        assert!(r.set_battle_weapon(0, Some(weapon)));
        assert_eq!(r.battle_attack_range(0), Some(range));
        let restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        assert_eq!(restored.battle_weapon(0), Some(weapon));
    }
    r.set_battle_weapon(0, Some(Weapon::Kinetic));
    // No active waves: 42 and 43 have the same bytes for legacy ammunition.
    let mut old = r.snapshot();
    old[8..12].copy_from_slice(&42u32.to_le_bytes());
    old.pop(); // Version 46's autonomous battle flag is absent from 42.
    old.pop(); // Version 45's manual-route flag is absent from 42.
    assert_eq!(
        BlastRange::restore(r.config, &old)
            .unwrap()
            .battle_weapon(0),
        Some(Weapon::Kinetic)
    );
    r.set_battle_weapon(0, Some(Weapon::Annihilation));
    let mut invalid = r.snapshot();
    invalid[8..12].copy_from_slice(&42u32.to_le_bytes());
    invalid.pop();
    invalid.pop();
    assert!(BlastRange::restore(r.config, &invalid).is_none());
}

#[test]
fn mixed_annihilation_and_shell_waves_keep_their_material_accounting() {
    let mut r = arena(RangeUnitKind::Tank);
    // Setup suppresses orders, but already flying rounds and waves keep working.
    for (weapon, x) in [(Weapon::Annihilation, 280), (Weapon::Blast, 370)] {
        let at = [x, 25, 430].map(Fx::from_int);
        r.battle.rounds.push(Round {
            owner: 0,
            weapon,
            pos: at,
            previous: at,
            velocity: [Fx::ZERO, Fx::from_int(-1200), Fx::ZERO],
            distance: Fx::from_int(1000),
        });
    }
    let mut kinds = Vec::new();
    for _ in 0..8 {
        r.step();
        kinds.extend(r.battle.impacts.iter().map(|i| i.weapon));
    }
    assert!(kinds.contains(&Weapon::Annihilation) && kinds.contains(&Weapon::Blast));
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..100 {
        r.step();
        replay.step();
        assert_eq!(r.state_hash(), replay.state_hash());
        assert!(r.matter.economy.ledger.balanced());
    }
    assert!(r.removed > 0);
    assert!(
        r.matter.economy.ledger.sunk_total() > 0,
        "ablated terrain was not accounted"
    );
    assert!(
        r.matter.spent_mj > 0,
        "ordinary shell lost its fracture budget"
    );
}

#[test]
fn stock_battle_kinetic_kills_a_frontal_tank() {
    for (distance, expected) in [(280, [24, 126]), (500, [36, 138]), (800, [52, 154])] {
        let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
        layout.size[0] = 1024;
        layout.clear_max[0] = 1000;
        layout.units[1].at[0] = layout.units[0].at[0] + distance;
        let config =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(config, layout).unwrap();
        r.start_battle().unwrap();
        r.units[1].motion.drive.head = fx::angle::HALF;
        r.units[1].update_pose(fx::angle::HALF);
        r.battle.actors[1].turret.angle = fx::angle::HALF;
        r.battle.actors[1].order = Order::Hold;
        assert_eq!(r.battle_weapon(0), Some(Weapon::Kinetic));
        r.battle.actors[0].order = Order::Attack(1);
        let mut hits = Vec::new();
        for _ in 0..900 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if r.battle.impacts.iter().any(|p| p.weapon == Weapon::Kinetic) {
                hits.push(r.tick);
            }
            if !r.units[1].status.alive {
                break;
            }
        }
        assert!(!r.units[1].status.alive, "hits {hits:?}");
        println!("distance={distance} hits={hits:?}");
        assert_eq!(
            hits,
            expected.map(modding::clock::stream_ticks),
            "distance={distance}"
        );
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn heavy_defenders_under_controlled_frontal_fire() {
    for kind in [RangeUnitKind::Spider, RangeUnitKind::TurretT2] {
        let mut r = arena_of([RangeUnitKind::Tank, kind]);
        r.start_battle().unwrap();
        r.units[1].motion.drive.head = fx::angle::HALF;
        r.units[1].update_pose(fx::angle::HALF);
        r.battle.actors[1].turret.angle = fx::angle::HALF;
        r.battle.actors[1].order = Order::Hold;
        r.battle.actors[0].order = Order::Attack(1);
        r.battle.actors[0].weapon = Some(Weapon::Kinetic);
        let mut hits = 0;
        for _ in 0..600 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if r.battle.impacts.iter().any(|p| p.weapon == Weapon::Kinetic) {
                hits += 1;
                eprintln!("defender={kind:?} hits={hits} alive={} armed={} core={:?} removed={} segments={} pos={:?}", r.units[1].status.alive, r.units[1].status.weapon, r.units[1].integrity.core, r.units[1].removed.len(), r.units[1].seg_removed.len(), r.units[1].at);
            }
            if !r.units[1].status.alive {
                break;
            }
        }
        assert!(hits >= 6, "{kind:?}: only {hits} impacts");
        assert!(
            r.units[1].status.alive && r.units[1].status.weapon,
            "{kind:?}: {:?}",
            r.units[1].status
        );
        assert!(
            r.units[1].integrity.core < Fx::ONE,
            "{kind:?}: core never damaged"
        );
        assert!(r.matter.economy.ledger.balanced());
    }
}

fn t2_defense_layout() -> RangeLayout {
    let mut layout = arena_of([RangeUnitKind::Tank, RangeUnitKind::TurretT2])
        .layout
        .unwrap();
    layout.units = (0..5)
        .map(|i| RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [170, 16, 240 + i * 48],
            allied: true,
        })
        .chain(std::iter::once(RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::TurretT2,
            at: [450, 16, 336],
            allied: false,
        }))
        .collect();
    layout
}

#[test]
fn t2_defends_against_five_stock_tanks() {
    let layout = t2_defense_layout();
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();

    for actor in &mut r.battle.actors[..5] {
        actor.order = Order::Attack(5);
        actor.weapon = Some(Weapon::Kinetic);
    }
    r.units[5].motion.drive.head = fx::angle::HALF;
    r.units[5].update_pose(fx::angle::HALF);
    r.battle.actors[5].turret.angle = fx::angle::HALF;
    r.battle.actors[5].order = Order::Auto;
    for _ in 0..3600 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        let old_status = r.units[5].status;
        r.step();
        if r.battle.impacts.iter().any(|p| p.weapon == Weapon::Kinetic)
            || (old_status.weapon && !r.units[5].status.weapon)
        {
            let u = &r.units[5];
            let min = [u.body.core.0, u.body.core.1, u.body.core.2].map(usize::from);
            let width = usize::from(u.body.core_size);
            let mut min_slice = width * width;
            for axis in 0..3 {
                for slice in 0..width {
                    let mut n = 0;
                    for a in 0..width {
                        for b in 0..width {
                            let mut p = min;
                            p[axis] += slice;
                            p[(axis + 1) % 3] += a;
                            p[(axis + 2) % 3] += b;
                            n += usize::from(u.body.vol.get(p[0], p[1], p[2]));
                        }
                    }
                    min_slice = min_slice.min(n);
                }
            }
            eprintln!(
                "T2 tick={} alive={} armed={} core={} min_slice={}/{} removed={} shots={}",
                r.tick,
                u.status.alive,
                u.status.weapon,
                u.integrity.core.mul(Fx::from_int(100)).to_int_trunc(),
                min_slice,
                width * width,
                u.removed.len(),
                r.battle.actors[5].fired
            );
        }
        if r.battle.phase != Phase::Fighting {
            break;
        }
    }
    eprintln!(
        "T2 vs five: tick={} phase={:?} alive={:?} armed={} core={:?} shots={:?}",
        r.tick,
        r.battle.phase,
        r.battle_counts(),
        r.units[5].status.weapon,
        r.units[5].integrity.core,
        r.battle.actors.iter().map(|a| a.fired).collect::<Vec<_>>()
    );
    assert_eq!(r.battle.phase, Phase::Defeat);
    assert!(r.units[5].status.alive && r.units[5].status.weapon);
    assert!(r.matter.economy.ledger.balanced());
}

/// Exercise the default enemy policy, pursuit and live motion together. A
/// short reach makes the enemy walk around the wall before it can engage.
fn enemy_detours_to_firing_range(kind: RangeUnitKind) {
    let mut r = arena_of([RangeUnitKind::Tank, kind]);
    let t = std::sync::Arc::make_mut(&mut r.battle.tuning);
    t.kinetic_range = 100;
    r.streams.tuning.machine_gun.range = 100;
    for x in 300..310 {
        for y in 16..90 {
            for z in 260..410 {
                r.world.queue_edit(voxel::Edit::put(x, y, z, 3));
            }
        }
    }
    while r.world.queued() > 0 {
        r.world.flush();
    }
    r.start_battle().unwrap();
    assert!(r.battle_order(0, Order::Hold, None));
    let mut detoured = false;
    let mut first_shot = None;
    let mut blocked = 0;
    let mut firing_position = None;
    for tick in 0..1800 {
        r.step();
        let m = &r.units[1].motion;
        if let Some(goal) = m.destination {
            if let Some(prior) = firing_position {
                assert_eq!(goal, prior, "a stationary target moved our firing position");
            } else {
                firing_position = Some(goal);
            }
        }
        let x = m.pos[0].to_int_trunc();
        let z = m.pos[2].to_int_trunc();
        detoured |= (280..330).contains(&x) && !(240..430).contains(&z);
        blocked += usize::from(m.blocked);
        assert!(r.units[1].status.alive);
        if r.battle.actors[1].fired > 0 {
            first_shot = Some(tick);
            break;
        }
    }
    assert!(detoured, "{kind:?} never went around the wall: fired={first_shot:?} at={:?} dest={:?} path={:?} weapon={:?} drive={:?} blocked={} hull={:?}",
        r.units[1].at,r.units[1].motion.destination,r.units[1].motion.path,r.battle_weapon(1),
        r.units[1].motion.drive, r.units[1].motion.blocked, r.units[1].motion.hull);
    assert!(
        first_shot.is_some(),
        "{kind:?} never engaged: at={:?}, blocked={blocked}, dest={:?}, path={:?}",
        r.units[1].at,
        r.units[1].motion.destination,
        r.units[1].motion.path
    );
    eprintln!(
        "enemy detour {kind:?}: first_shot={first_shot:?}, blocked_ticks={blocked}, at={:?}",
        r.units[1].at
    );
}

#[test]
fn enemy_tank_detours_around_cover_and_engages() {
    enemy_detours_to_firing_range(RangeUnitKind::Tank);
}

#[test]
fn enemy_walker_detours_around_cover_and_engages() {
    enemy_detours_to_firing_range(RangeUnitKind::Strider);
}

#[test]
fn siege_laser_finishes_tanks_from_each_side_and_drops_the_dead_target() {
    for offset in [[-180, 0], [180, 0], [0, -180], [0, 180]] {
        let mut layout = arena_of([RangeUnitKind::Spider, RangeUnitKind::Tank])
            .layout
            .unwrap();
        layout.clear_min = [16, 16];
        layout.clear_max = [624, 624];
        layout.units[1].at = [320, 16, 320];
        layout.units[0].at = [320 + offset[0], 16, 320 + offset[1]];
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(c, layout).unwrap();
        r.start_battle().unwrap();
        r.battle.actors[1].order = Order::Hold;
        r.units[1].status.weapon = false;
        assert!(r.battle_order(0, Order::Attack(1), None));
        let mut death = None;
        for tick in 0..600 {
            if r.battle.ai.pending.is_none() && r.battle.phase == Phase::Fighting {
                r.submit_enemy_ai_plan(
                    r.battle.ai.next_tick,
                    if r.units[1].status.alive {
                        vec![Decision {
                            actor: 1,
                            order: AiOrder::Hold,
                        }]
                    } else {
                        vec![]
                    },
                )
                .unwrap();
            }
            r.step();
            if !r.units[1].status.alive {
                death = Some(tick);
                break;
            }
        }
        eprintln!("siege {offset:?}: death={death:?} core={:?} removed={} melted={} fragments={} hot={} at={:?}", r.units[1].integrity.core, r.units[1].removed.len(), r.thermals.melted, r.matter.debris.fragments.len(), r.thermals.hot(), r.units[1].at);
        assert!(
            death.is_some(),
            "beam could not finish tank from {offset:?}"
        );
        r.step();
        assert!(
            r.battle.lasers.is_empty(),
            "beam persisted after target death"
        );
        assert_eq!(r.battle.actors[0].target, None);
    }
}

#[test]
fn spider_walks_on_eight_feet_then_burns_a_target_without_self_damage_and_replays() {
    let mut r = arena_of([RangeUnitKind::Spider, RangeUnitKind::Tank]);
    assert_eq!(r.units[0].motion.feet.len(), 8);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    // Keep the gait trial separate from the now-active move-order laser.
    r.units[0].status.weapon = false;
    let step = |r: &mut BlastRange| {
        if r.battle.phase == Phase::Fighting && r.battle.ai.pending.is_none() {
            let plan = if r.units[1].status.alive {
                vec![Decision {
                    actor: 1,
                    order: AiOrder::Hold,
                }]
            } else {
                Vec::new()
            };
            r.submit_enemy_ai_plan(r.battle.ai.next_tick, plan).unwrap();
        }
        r.step();
    };
    assert!(r.battle_order(0, Order::Move, Some([300, 200])));
    let mut swung = vec![false; 8];
    let mut swing_patterns = std::collections::BTreeSet::new();
    for _ in 0..1200 {
        step(&mut r);
        let mut pattern = 0u8;
        for (i, f) in r.units[0].motion.feet.iter().enumerate() {
            swung[i] |= !f.planted();
            pattern |= u8::from(!f.planted()) << i;
        }
        swing_patterns.insert(pattern);
        for (a, b) in [(0, 2), (1, 3), (4, 6), (5, 7)] {
            assert_ne!(
                pattern & (1 << a | 1 << b),
                1 << a | 1 << b,
                "both feet of support {} lifted",
                r.units[0].body.legs[a].support
            );
        }
        if r.units[0].motion.destination.is_none() {
            break;
        }
    }
    assert!(
        r.units[0].motion.destination.is_none(),
        "spider stuck at {:?}",
        r.units[0].motion.pos
    );
    assert!(swung.iter().all(|v| *v), "legs never swung: {swung:?}");
    assert!(
        swing_patterns.len() >= 5,
        "legs still move in synchronized groups: {swing_patterns:?}"
    );
    assert_eq!(r.battle_weapon(0), Some(Weapon::Laser));
    assert_eq!(
        r.battle_attack_range(0),
        Some(motion::spider_tuning().laser.range)
    );
    r.units[0].status.weapon = true;
    let own = r.units[0].body.volume_mm3();
    let victim = r.units[1].body.volume_mm3();
    assert!(r.battle_order(0, Order::Attack(1), None));
    let mut beams = 0;
    let mut replay = None;
    for tick in 0..360 {
        step(&mut r);
        beams += r.battle.lasers.len();
        // Snapshot a live beam, rather than the already-ended siege duel.
        if replay.is_none() && !r.battle.lasers.is_empty() {
            replay = Some(BlastRange::restore(r.config, &r.snapshot()).unwrap());
        } else if let Some(other) = replay.as_mut() {
            step(other);
            assert_eq!(
                r.state_hash(),
                other.state_hash(),
                "laser replay tick {tick}"
            );
        }
    }
    assert!(beams >= 2, "laser never sustained: {beams}");
    assert!(
        !r.units[1].status.alive,
        "siege beam did not destroy the tank"
    );
    assert!(
        r.units[1].body.volume_mm3() < victim,
        "laser did not melt armour"
    );
    assert_eq!(
        r.units[0].body.volume_mm3(),
        own,
        "own laser damaged spider"
    );
}

/// Worker pools are transient: restoring chooses the roster's default,
/// whereas an explicit serial override survives starting/restarting a fight.
#[test]
fn battle_worker_pool_restore_and_start_preserve_the_same_fight() {
    let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
        "../../../data/battle-world.ron"
    ))
    .unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    layout.course.as_mut().unwrap().patrols.clear();
    layout.units = preset.units;
    layout.blocks.extend(preset.blocks);
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    for (count, workers) in [(24, 4), (60, 8)] {
        let mut layout = layout.clone();
        layout.units.truncate(count);
        let mut parallel = BlastRange::with_layout(config, layout).unwrap();
        assert!(parallel.navigation_pool.is_none());
        assert!(parallel.set_ai_vs_ai(true));
        let setup = parallel.snapshot();
        parallel.start_battle().unwrap();
        assert_eq!(
            parallel
                .navigation_pool
                .as_ref()
                .unwrap()
                .current_num_threads(),
            workers
        );
        for _ in 0..20 {
            parallel.step();
        }
        // Restore with fine searches in flight, two at least: the pool
        // walks continuations only then. Spider 22 used to search from its
        // pad for hundreds of ticks; it now leaves it by a lattice entry
        // (ADR-691), so wait for pursuit searches instead.
        let fine = |r: &BlastRange| {
            r.units
                .iter()
                .filter(|u| u.motion.fine_work().is_some())
                .count()
        };
        for _ in 0..600 {
            if fine(&parallel) >= 2 {
                break;
            }
            parallel.step();
        }
        assert!(fine(&parallel) >= 2, "no two fine searches in flight");
        let mut serial = BlastRange::restore(config, &parallel.snapshot()).unwrap();
        assert_eq!(
            serial
                .navigation_pool
                .as_ref()
                .unwrap()
                .current_num_threads(),
            workers
        );
        assert_eq!(serial.state_hash(), parallel.state_hash());
        assert!(serial.set_navigation_workers(1));
        assert!(serial.navigation_pool.is_none());
        let mut restarted = BlastRange::restore(config, &setup).unwrap();
        assert!(restarted.set_navigation_workers(1));
        restarted.start_battle().unwrap();
        assert!(restarted.navigation_pool.is_none());
        drop(restarted);
        let tasks: Vec<[std::sync::atomic::AtomicUsize; 24]> = (0..count)
            .map(|_| std::array::from_fn(|_| std::sync::atomic::AtomicUsize::new(0)))
            .collect();
        for tick in 0..20 {
            serial.step();
            parallel.step_observed_tasks(
                |_| {},
                |_| {},
                |_, _| {},
                |id, event| {
                    assert!(rayon::current_thread_index().unwrap() < workers);
                    let stage = match event {
                        "route_begin" => 0,
                        "route_end" => 1,
                        "ground_begin" => 2,
                        "ground_end" => 3,
                        "local_clone_begin" => 4,
                        "local_clone_end" => 5,
                        "local_advance_begin" => 6,
                        "local_advance_end" => 7,
                        "terrain_advance_begin" => 8,
                        "terrain_advance_end" => 9,
                        "route_forget_begin" => 10,
                        "route_forget_end" => 11,
                        "route_query_begin" => 12,
                        "route_query_end" => 13,
                        "sweep_0_begin" => 14,
                        "sweep_0_end" => 15,
                        "sweep_1_begin" => 16,
                        "sweep_1_end" => 17,
                        "sweep_2_begin" => 18,
                        "sweep_2_end" => 19,
                        "sweep_3_begin" => 20,
                        "sweep_3_end" => 21,
                        "feet_begin" => 22,
                        "feet_end" => 23,
                        _ => unreachable!(),
                    };
                    tasks[id][stage].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                },
            );
            assert_eq!(
                serial.state_hash(),
                parallel.state_hash(),
                "count={count} tick={tick}"
            );
        }
        let mut route_tasks = 0;
        let mut continuations = 0;
        for task in tasks {
            let count = task.map(|v| v.into_inner());
            assert_eq!(count[0], count[1]);
            assert_eq!(count[2], count[3]);
            assert_eq!(count[2], 20);
            assert_eq!(count[22], 20);
            assert_eq!(count[22], count[23]);
            assert_eq!(count[4], count[5]);
            assert_eq!(count[6], count[7]);
            assert_eq!(count[4], count[6]);
            assert_eq!(count[8], count[9]);
            assert_eq!(count[10], count[11]);
            assert_eq!(count[12], count[13]);
            assert!(count[12] <= count[10]);
            assert!(count[10] <= count[4] + count[8]);
            for batch in 0..4 {
                assert_eq!(count[14 + 2 * batch], count[15 + 2 * batch]);
                assert!(count[14 + 2 * batch] <= count[12]);
            }
            assert!(count[4] + count[8] <= count[0]);
            continuations += count[4] + count[8];
            route_tasks += count[0];
        }
        assert!(route_tasks > 0, "observer never saw a continuation task");
        assert!(continuations > 0, "observer never saw an active search");
    }
}

#[test]
fn authored_loadouts_fire_for_either_ai_team_and_survive_snapshot_and_restart() {
    use modding::range::RangeLoadout;
    for (loadout, weapon) in [
        (RangeLoadout::Blast, Weapon::Blast),
        (RangeLoadout::Kinetic, Weapon::Kinetic),
        (RangeLoadout::Energy, Weapon::Energy),
        (RangeLoadout::Annihilation, Weapon::Annihilation),
    ] {
        let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
        for unit in &mut layout.units {
            unit.weapon = Some(loadout);
        }
        let config =
            ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        let mut sim = BlastRange::with_layout(config, layout.clone()).unwrap();
        assert!(sim.set_battle_weapon(0, Some(Weapon::Kinetic)));
        // Scenario rebuild restores authored data after a runtime ammo change.
        sim = BlastRange::with_layout(config, layout.clone()).unwrap();
        assert_eq!(sim.battle_weapon(0), Some(weapon));
        assert!(sim.set_ai_vs_ai(true));
        let setup = sim.snapshot();
        sim.start_battle().unwrap();
        for _ in 0..160 {
            sim.step();
            if sim.battle.actors.iter().all(|a| a.fired > 0) {
                break;
            }
        }
        for id in 0..2 {
            assert_eq!(sim.battle_weapon(id), Some(weapon));
            assert!(
                sim.battle.actors[id].fired > 0,
                "side {id} never fired {weapon:?}"
            );
        }
        let mut restored = BlastRange::restore(config, &sim.snapshot()).unwrap();
        for _ in 0..10 {
            sim.step();
            restored.step();
            assert_eq!(sim.state_hash(), restored.state_hash());
        }
        let mut restarted = BlastRange::restore(config, &setup).unwrap();
        assert!(restarted.battle.autonomous);
        restarted.start_battle().unwrap();
        assert_eq!(restarted.battle_weapon(1), Some(weapon));
        let fresh = BlastRange::with_layout(config, layout).unwrap();
        assert_eq!(fresh.battle_weapon(0), Some(weapon));
        assert_eq!(fresh.battle_weapon(1), Some(weapon));
    }
}

/// **The tick does not depend on the physics pool's size** (ADR-652).
/// The stock world battle, 150 ticks, with one, two, four and eight
/// physics workers: one hash.
#[test]
fn the_world_battle_hash_does_not_depend_on_physics_workers() {
    let run = |workers: usize| {
        let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
            "../../../data/battle-world.ron"
        ))
        .unwrap();
        let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        layout.battle = true;
        layout.units = preset.units;
        layout.blocks.extend(preset.blocks);
        layout.course.as_mut().unwrap().patrols.clear();
        let config =
            ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        assert!(sim.set_physics_workers(workers));
        assert_eq!(sim.worker_counts()[0], workers);
        assert!(sim.set_ai_vs_ai(true));
        sim.start_battle().unwrap();
        let mut blast = false;
        for _ in 0..150 {
            sim.step();
            blast |= sim.battle.impacts.iter().any(|i| {
                matches!(
                    i.weapon,
                    Weapon::Blast | Weapon::Canister | Weapon::Energy | Weapon::Annihilation
                )
            });
        }
        // ADR-116: the parallel phases this pins have to do work in it.
        assert!(
            sim.battle.actors.iter().any(|a| a.fired > 0),
            "nobody fired"
        );
        assert!(blast, "no explosive projectile impacted");
        assert!(
            sim.matter.debris.fragments.len() > 100,
            "the debris phase was idle"
        );
        assert!(!sim.rubble.is_empty(), "no rubble to refresh");
        sim.state_hash()
    };
    let one = run(1);
    for workers in [2, 4, 8] {
        assert_eq!(run(workers), one, "{workers} physics workers");
    }
    let mut r = BlastRange::new(
        ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap(),
    );
    assert!(!r.set_physics_workers(0) && !r.set_physics_workers(9));
}

#[test]
fn world_ai_armies_fire_standard_ammunition_without_annihilation() {
    let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
        "../../../data/battle-world.ron"
    ))
    .unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    layout.units = preset.units;
    layout.blocks.extend(preset.blocks);
    layout.course.as_mut().unwrap().patrols.clear();
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut sim = BlastRange::with_layout(config, layout).unwrap();
    assert!(sim.set_ai_vs_ai(true));
    for side in [true, false] {
        let weapons: std::collections::BTreeSet<_> = (0..sim.units.len())
            .filter(|&id| sim.allied(id) == side)
            .filter_map(|id| sim.battle_weapon(id).map(|w| w as u8))
            .collect();
        let expected = [
            Weapon::MachineGun,
            Weapon::Blast,
            Weapon::Kinetic,
            Weapon::Energy,
            Weapon::Laser,
            Weapon::Canister,
            Weapon::Siege,
        ]
        .into_iter()
        .map(|weapon| weapon as u8)
        .collect();
        assert_eq!(weapons, expected, "unexpected guns on side {side}");
    }
    sim.start_battle().unwrap();
    let mut observed = std::collections::BTreeSet::new();
    for _ in 0..600 {
        sim.step();
        for round in &sim.battle.rounds {
            assert_ne!(round.weapon, Weapon::Annihilation);
            observed.insert((sim.allied(round.owner), round.weapon as u8));
        }
    }
    for side in [true, false] {
        for weapon in [
            Weapon::Kinetic,
            Weapon::Energy,
            Weapon::Canister,
            Weapon::Siege,
        ] {
            assert!(
                observed.contains(&(side, weapon as u8)),
                "{weapon:?} never flew on side {side}; tick={}, observed={observed:?}",
                sim.tick
            );
        }
    }
    eprintln!(
        "world AI content: tick={}, live-round weapon/side coverage={observed:?}",
        sim.tick
    );
    assert!(sim.matter.economy.ledger.balanced());
}

#[test]
fn battle_weapon_codes_fit_diagnostic_counters() {
    let mut counts = [0usize; Weapon::COUNT];
    for weapon in [
        Weapon::MachineGun,
        Weapon::Blast,
        Weapon::Kinetic,
        Weapon::Energy,
        Weapon::Annihilation,
        Weapon::Laser,
        Weapon::Canister,
        Weapon::Siege,
        Weapon::Nuclear,
        Weapon::Lightning,
    ] {
        assert_eq!(Weapon::from_code(weapon as u8), Some(weapon));
        counts[weapon as usize] += 1;
    }
    assert!(counts.into_iter().all(|n| n == 1));
    assert!(Weapon::from_code(Weapon::COUNT as u8).is_none());
}

#[test]
fn machine_gun_continuation_capacity_reserves_both_walker_barrels() {
    let mut r = arena(RangeUnitKind::Strider);
    r.start_battle().unwrap();
    r.battle.ai.next_tick = u32::MAX;
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Ground([540, 100, 336]), None));
    r.streams.tuning.kinetic.max_jobs = 1;
    for _ in 0..90 {
        r.step();
    }
    assert_eq!(r.battle.actors[0].fired, 0);
    assert!(r.battle.rounds.is_empty());
    r.streams.tuning.kinetic.max_jobs = 2;
    for _ in 0..30 {
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert_eq!(r.battle.actors[0].fired, 1);
    assert_eq!(r.battle.rounds.len(), 2);
}

#[test]
fn t2_defends_against_five_tanks_from_each_cardinal_direction() {
    for turn in 0..4 {
        let mut layout = t2_defense_layout();
        layout.size = [768, 192, 768];
        layout.clear_max = [704, 704];
        for (i, unit) in layout.units[..5].iter_mut().enumerate() {
            unit.at[2] = 196 + i as i32 * 70;
        }
        for unit in &mut layout.units {
            let mut p = [unit.at[0] - 450, unit.at[2] - 336];
            for _ in 0..turn {
                p = [-p[1], p[0]];
            }
            unit.at[0] = 384 + p[0];
            unit.at[2] = 384 + p[1];
        }
        let config =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(config, layout).unwrap();
        r.start_battle().unwrap();
        let attacker_head = fx::angle::diff(turn * fx::angle::QUARTER, 0);
        let defender_head = fx::angle::diff(attacker_head + fx::angle::HALF, 0);
        for id in 0..6 {
            let head = if id == 5 {
                defender_head
            } else {
                attacker_head
            };
            r.units[id].motion.drive.head = head;
            r.units[id].update_pose(head);
            r.battle.actors[id].turret.angle = head;
            r.battle.actors[id].order = if id == 5 {
                Order::Auto
            } else {
                Order::Attack(5)
            };
            if id < 5 {
                r.battle.actors[id].weapon = Some(Weapon::Kinetic);
            }
        }
        for _ in 0..3600 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if r.battle.phase != Phase::Fighting {
                break;
            }
        }
        eprintln!(
            "cardinal={turn} tick={} phase={:?} counts={:?} core={:?} armed={}",
            r.tick,
            r.battle.phase,
            r.battle_counts(),
            r.units[5].integrity.core,
            r.units[5].status.weapon
        );
        assert_eq!(r.battle.phase, Phase::Defeat, "approach={turn}");
        assert!(r.units[5].status.alive && r.units[5].status.weapon);
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn t2_energy_round_preserves_class_power_after_owner_loss_and_save() {
    let mut r = arena_of([RangeUnitKind::Tank, RangeUnitKind::TurretT2]);
    slow_blasts(&mut r);
    r.start_battle().unwrap();
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    for actor in &mut r.battle.actors {
        actor.order = Order::Hold;
    }
    let ordinary = r.battle.energy_config.energy;
    assert_eq!(
        r.battle
            .shot_config(Weapon::Energy, RangeUnitKind::Tank)
            .energy,
        ordinary
    );
    let heavy = r
        .battle
        .shot_config(Weapon::Energy, RangeUnitKind::TurretT2)
        .energy;
    assert_eq!(
        heavy,
        ordinary * r.battle.tuning.turret_t2_power_milli / 1000
    );
    assert!(heavy > ordinary && heavy < ordinary * 2);
    // An already emitted bolt will hit empty ground after its gun loses power.
    let p = [300, 20, 100].map(Fx::from_int);
    r.battle.rounds.push(Round {
        owner: 1,
        weapon: Weapon::Energy,
        pos: p,
        previous: p,
        velocity: [Fx::ZERO, Fx::from_int(-120), Fx::ZERO],
        distance: Fx::from_int(100),
    });
    // This is a scripted severing of the whole barrel, not one kinetic hit.
    // Fund the fixture independently of the armour-piercing round's balance.
    r.matter.begin(r.battle.shell_config.material_energy_j, 1);
    let config = r.battle.kinetic_config;
    let u = &mut r.units[1];
    let path = u.body.weapon_paths()[0];
    for y in path.min[1]..path.max[1] {
        for z in path.min[2]..path.max[2] {
            let cell = [45, usize::from(y), usize::from(z)];
            if !u.body.vol.get(cell[0], cell[1], cell[2]) {
                continue;
            }
            assert!(matches!(
                r.matter.body(1, u, cell, &u.root_frame(cell[1]), u.at),
                matter::ClaimResult::Applied
            ));
            u.detached += 1;
            u.receive_cell(cell, 1_000_000, &config);
        }
    }
    u.refresh_damage(1);
    assert!(u.status.alive && !u.status.weapon);
    let mut last = ("header", 0, None);
    let mut copy = BlastRange::restore_observed(r.config, &r.snapshot(), |s, o, u| {
        last = (s, o, u);
    })
    .unwrap_or_else(|| panic!("T2 contact restore refused at {last:?}"));
    slow_blasts(&mut copy);
    for _ in 0..30 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
        if !r.battle.waves.is_empty() {
            break;
        }
    }
    assert_eq!(r.battle.waves.len(), 1);
    let wave = &r.battle.waves[0].wave.wave;
    assert_eq!(wave.pressure_at([wave.size() / 2; 3]), heavy);
    let mut wave_copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    slow_blasts(&mut wave_copy);
    for _ in 0..30 {
        r.step();
        wave_copy.step();
    }
    assert_eq!(r.state_hash(), wave_copy.state_hash());
    assert!(r.matter.economy.ledger.balanced());
    assert_eq!(r.battle.energy_config.energy, ordinary);
}

#[test]
fn t2_energy_spider_survives_three_actual_hits_with_its_weapon() {
    let mut r = arena_of([RangeUnitKind::TurretT2, RangeUnitKind::Spider]);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.units[1].motion.enabled = false;
    assert!(r.battle_order(0, Order::Attack(1), None));
    let mut hits = 0;
    for _ in 0..400 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        hits += r
            .battle
            .impacts
            .iter()
            .filter(|i| i.weapon == Weapon::Energy)
            .count();
        if !r.units[1].status.alive
            || !r.units[1].status.weapon
            || hits >= 3 && r.battle.waves.is_empty() && r.world.queued() == 0
        {
            break;
        }
    }
    assert!(hits >= 3, "the T2 fixture fired only {hits} impacts");
    assert!(
        r.units[1].status.alive && r.units[1].status.weapon,
        "spider lost its core or weapon after {hits} hits; core={:?}",
        r.units[1].integrity.core
    );
    assert!(
        r.units[1].revision > 0,
        "the impacts never reached the spider"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn reinforced_t2_still_loses_a_severed_barrel_and_a_cut_core_section() {
    let template = voxel::unit::turret_t2();
    let model = crate::integrity::Model::new(&template);
    let mut body = template.clone();
    let path = body.weapon_paths()[0];
    for y in path.min[1]..path.max[1] {
        for z in path.min[2]..path.max[2] {
            body.remove(45, usize::from(y), usize::from(z));
        }
    }
    let (status, report) = model.evaluate(&body, &mut CoreBufs::new());
    assert!(status.alive && !status.weapon);
    assert_eq!(report.core, Fx::ONE);
    let mut body = template;
    let min = [body.core.0, body.core.1, body.core.2].map(usize::from);
    let n = usize::from(body.core_size);
    for x in min[0]..min[0] + n {
        for z in min[2]..min[2] + n {
            body.remove(x, min[1] + n / 2, z);
        }
    }
    let (status, _) = model.evaluate(&body, &mut CoreBufs::new());
    assert!(!status.alive && !status.weapon);
}

#[test]
fn stationary_spider_survives_stock_tank_fire_from_eight_directions() {
    for offset in [
        [-280, 0],
        [-200, -200],
        [0, -280],
        [200, -200],
        [280, 0],
        [200, 200],
        [0, 280],
        [-200, 200],
    ] {
        let mut layout = arena_of([RangeUnitKind::Tank, RangeUnitKind::Spider])
            .layout
            .unwrap();
        layout.size = [896, 192, 896];
        layout.clear_min = [16, 16];
        layout.clear_max = [880, 880];
        layout.units[1].at = [448, 16, 448];
        layout.units[0].at = [448 + offset[0], 16, 448 + offset[1]];
        let config =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(config, layout).unwrap();
        r.start_battle().unwrap();
        r.battle.actors[1].order = Order::Hold;
        r.battle.actors[0].order = Order::Attack(1);
        r.battle.actors[0].weapon = Some(Weapon::Kinetic);
        let mut hits = 0;
        for _ in 0..700 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if r.battle.impacts.iter().any(|p| p.weapon == Weapon::Kinetic) {
                hits += 1;
                eprintln!("spider {offset:?} tick={} hits={hits} alive={} armed={} core={:?} removed={} segments={} feet={:?} at={:?}",r.tick,r.units[1].status.alive,r.units[1].status.weapon,r.units[1].integrity.core,r.units[1].removed.len(),r.units[1].seg_removed.len(),r.units[1].seg_gone,r.units[1].at);
            }
            if !r.units[1].status.alive || hits >= 6 {
                break;
            }
        }
        assert_eq!(hits, 6, "direction={offset:?}: too few real impacts");
        assert!(r.battle_order(0, Order::Hold, None));
        for _ in 0..30 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
        }
        eprintln!(
            "settled spider direction={offset:?} core={:?} pending={} status={:?}",
            r.units[1].integrity.core,
            r.pending_kinetic_impacts(),
            r.units[1].status
        );
        assert_eq!(r.pending_kinetic_impacts(), 0);
        assert!(r.battle.rounds.is_empty());
        assert!(
            r.units[1].status.alive && r.units[1].status.weapon,
            "direction={offset:?}: {:?}",
            r.units[1].status
        );
        assert!(r.units[1].motion.destination.is_none());
        assert_eq!(
            [r.units[1].at[0], r.units[1].at[2]],
            [448, 448],
            "spider evaded the trial"
        );
        r.battle.actors[1].order = Order::Attack(0);
        let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        for _ in 0..300 {
            let next = r.tick + r.battle.ai.tuning.decision_ticks;
            r.battle.ai.next_tick = next;
            replay.battle.ai.next_tick = next;
            r.step();
            replay.step();
            assert_eq!(r.state_hash(), replay.state_hash());
            if r.battle.actors[1].fired > 0 {
                break;
            }
        }
        assert!(
            r.battle.actors[1].fired > 0,
            "direction={offset:?}: damaged laser never fired"
        );
        assert!(!r.battle.lasers.is_empty());
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn stationary_spider_survives_two_tank_salvos_and_can_return_fire() {
    let mut layout = arena_of([RangeUnitKind::Tank, RangeUnitKind::Spider])
        .layout
        .unwrap();
    layout.size = [896, 192, 896];
    layout.clear_min = [16, 16];
    layout.clear_max = [880, 880];
    layout.units[1].at = [448, 16, 448];
    layout.units[0].at = [728, 16, 348];
    layout.units.push(RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Tank,
        at: [728, 16, 548],
        allied: true,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    for id in [0, 2] {
        r.battle.actors[id].order = Order::Attack(1);
        r.battle.actors[id].weapon = Some(Weapon::Kinetic);
    }
    let mut hits = 0;
    for _ in 0..400 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        hits += r
            .battle
            .impacts
            .iter()
            .filter(|p| p.weapon == Weapon::Kinetic)
            .count();
        if hits >= 4 || !r.units[1].status.alive {
            break;
        }
    }
    assert!(
        (4..=6).contains(&hits),
        "two salvos, including simultaneous arrivals: {hits}"
    );
    for id in [0, 2] {
        assert!(r.battle.actors[id].fired >= 2);
        assert!(r.battle_order(id, Order::Hold, None));
    }
    for _ in 0..30 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
    }
    eprintln!(
        "two tank spider: hits={hits} core={:?} status={:?}",
        r.units[1].integrity.core, r.units[1].status
    );
    assert_eq!(r.pending_kinetic_impacts(), 0);
    assert!(r.units[1].status.alive && r.units[1].status.weapon);
    assert_eq!([r.units[1].at[0], r.units[1].at[2]], [448, 448]);
    r.battle.actors[1].order = Order::Attack(0);
    for _ in 0..300 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[1].fired > 0 {
            break;
        }
    }
    assert!(r.battle.actors[1].fired > 0);
    assert!(!r.battle.lasers.is_empty());
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn spider_reactor_and_laser_remain_physically_severable() {
    let mut body = voxel::unit::spider();
    let model = crate::integrity::Model::new(&body);
    assert!(u16::from(body.core.1) + u16::from(body.core_size) <= u16::from(body.turret_y));
    let x = usize::from(body.weapon_paths()[0].max[0]) - 3;
    for y in 0..usize::from(body.vol.ny) {
        for z in 0..usize::from(body.vol.nz) {
            body.remove(x, y, z);
        }
    }
    let (status, report) = model.evaluate(&body, &mut CoreBufs::new());
    assert!(status.alive && !status.weapon);
    assert_eq!(report.core, Fx::ONE);
    let mut body = voxel::unit::spider();
    let model = crate::integrity::Model::new(&body);
    let min = [body.core.0, body.core.1, body.core.2].map(usize::from);
    let n = usize::from(body.core_size);
    for x in min[0]..min[0] + n {
        for z in min[2]..min[2] + n {
            body.remove(x, min[1] + n / 2, z);
        }
    }
    let (status, _) = model.evaluate(&body, &mut CoreBufs::new());
    assert!(!status.alive && !status.weapon);
}

#[test]
fn spider_laser_opening_uses_current_terrain_and_keeps_close_and_legacy_aim_safe() {
    let mut r = arena_of([RangeUnitKind::Spider, RangeUnitKind::Tank]);
    let mut tuning = motion::spider_tuning().clone();
    let from = [200, 60, 336].map(Fx::from_int);
    let target = [450, 35, 336];
    let first = spider_laser_aim(&r.world, from, target, 0, &tuning);
    assert_eq!(first, [330, 15, 336]);
    r.world
        .queue_dig(first[0] as u32, first[1] as u32, first[2] as u32);
    r.world.flush();
    assert_eq!(
        spider_laser_aim(&r.world, from, target, 0, &tuning),
        [330, 14, 336]
    );
    let mut previous = first[0];
    for sample in 1..=18 {
        let aim = spider_laser_aim(&r.world, from, target, sample, &tuning);
        assert!((previous..=target[0]).contains(&aim[0]));
        assert!((14..=target[1]).contains(&aim[1]));
        previous = aim[0];
    }
    assert_eq!(
        spider_laser_aim(&r.world, from, target, 18, &tuning),
        target
    );
    assert_eq!(
        spider_laser_aim(&r.world, from, [400, 35, 390], 19, &tuning),
        [400, 35, 390]
    );
    let close = spider_laser_aim(&r.world, from, [210, 35, 336], 0, &tuning);
    assert!(
        (205..210).contains(&close[0]),
        "opening point behind muzzle: {close:?}"
    );
    assert_eq!(
        spider_laser_aim(&r.world, from, [200, 35, 336], 0, &tuning),
        [200, 35, 336]
    );
    tuning.laser.interval_ticks = 2;
    assert_eq!(spider_laser_aim(&r.world, from, target, 9, &tuning), target);
    tuning.laser_sweep = None;
    assert_eq!(spider_laser_aim(&r.world, from, target, 0, &tuning), target);
}

fn step_spider_opening_trial(r: &mut BlastRange) {
    // Keep the independent target stationary without an invalid saved AI clock.
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.step();
}

#[test]
fn spider_laser_opening_heats_the_visible_ground_then_armour_and_replays() {
    let mut layout = arena_of([RangeUnitKind::Spider, RangeUnitKind::Tank])
        .layout
        .unwrap();
    // Use meltable paving: ordinary soil has neither melt nor kindling and
    // intentionally does not retain heat in the existing material contract.
    layout.blocks.push(modding::range::RangeBlock {
        min: [300, 14, 300],
        max: [425, 16, 375],
        material: 3,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    let own_volume = r.units[0].body.volume_mm3();
    for _ in 0..180 {
        step_spider_opening_trial(&mut r);
        if !r.battle.lasers.is_empty() {
            break;
        }
    }
    assert_eq!(r.battle.lasers.len(), 1, "no opening beam");
    let first = r.battle.lasers[0];
    assert!(first.hit);
    assert!(
        first.to[0].to_int_trunc() < r.units[1].at[0] - 20,
        "first beam skipped ground: {first:?}"
    );
    assert!(first.to[1].to_int_trunc() <= 16);
    assert!(
        r.heat.hot() > 0 || r.heat.melted_total > 0,
        "visible ground contact neither heated nor melted the paving"
    );
    assert!(
        !r.thermals.cells().any(|(cell, _)| cell.unit == 1),
        "target heated before beam reached it"
    );
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    let mut target_heated = false;
    for _ in 0..100 {
        step_spider_opening_trial(&mut r);
        step_spider_opening_trial(&mut replay);
        assert_eq!(
            r.state_hash(),
            replay.state_hash(),
            "mid-sweep restore diverged"
        );
        target_heated |= r.thermals.cells().any(|(cell, _)| cell.unit == 1)
            || !r.units[1].removed.is_empty()
            || !r.units[1].seg_removed.is_empty();
        assert!(
            !r.thermals.cells().any(|(cell, _)| cell.unit == 0),
            "emitter heated itself"
        );
        if !r.units[1].status.alive {
            break;
        }
    }
    assert!(target_heated, "opening pass never reached armour");
    assert!(
        !r.units[1].status.alive,
        "sustained siege beam failed to finish target"
    );
    assert_eq!(r.units[0].body.volume_mm3(), own_volume);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn spider_laser_explicit_point_uses_the_servo_without_an_automatic_ground_opening() {
    let mut r = arena_of([RangeUnitKind::Spider, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    let target = aim_point(&r.units[1]);
    assert!(r.battle_order(0, Order::Ground(target), None));
    for _ in 0..180 {
        step_spider_opening_trial(&mut r);
        if !r.battle.lasers.is_empty() {
            break;
        }
    }
    assert_eq!(r.battle.lasers.len(), 1);
    assert!(
        r.battle.lasers[0].to[1].to_int_trunc() > 16,
        "point order acquired automatic ground opening"
    );
    assert!(
        r.thermals.cells().any(|(cell, _)| cell.unit == 1)
            || !r.units[1].removed.is_empty()
            || !r.units[1].seg_removed.is_empty()
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn laser_firing_position_classifies_first_cover_in_firing_direction_and_live_edits() {
    let mut r = arena_of([RangeUnitKind::Spider, RangeUnitKind::Tank]);
    for (x, material) in [(300, 3), (340, 4)] {
        r.world.queue_edit(voxel::Edit::put(x, 80, 336, material));
    }
    r.world.flush();
    let from = [250, 80, 336].map(Fx::from_int);
    let target = [400, 80, 336];
    assert!(
        !laser_requires_flank(&r.world, &r.matter.economy.mats, from, target),
        "meltable front cover should be breached"
    );
    assert!(
        laser_requires_flank(
            &r.world,
            &r.matter.economy.mats,
            target.map(Fx::from_int),
            [250, 80, 336]
        ),
        "foliage first from the other side must cause a flank"
    );
    r.world.queue_dig(300, 80, 336);
    r.world.flush();
    assert!(
        laser_requires_flank(&r.world, &r.matter.economy.mats, from, target),
        "live hole exposed the forest"
    );
    r.world.queue_dig(340, 80, 336);
    r.world.flush();
    assert!(!laser_requires_flank(
        &r.world,
        &r.matter.economy.mats,
        from,
        target
    ));
    for material in [1, 6, 7] {
        r.world.queue_edit(voxel::Edit::put(320, 80, 336, material));
        r.world.flush();
        assert!(
            laser_requires_flank(&r.world, &r.matter.economy.mats, from, target),
            "inert terrain {material} allowed an unproductive beam"
        );
    }
}

#[test]
fn enemy_spider_leaves_a_forested_line_of_fire_engages_and_replays() {
    let mut layout = arena_of([RangeUnitKind::Tank, RangeUnitKind::Spider])
        .layout
        .unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [320, 16, 260],
        max: [330, 120, 410],
        material: 4,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    assert!(r.battle_order(0, Order::Hold, None));
    let start = r.units[1].motion.pos;
    let mut replay = None;
    let mut first = None;
    let mut replans_before_fire = 0;
    for tick in 0..1200 {
        r.step_observed(|phase| {
            if first.is_none() && phase == "orders_retarget" {
                replans_before_fire += 1;
            }
        });
        if let Some(other) = replay.as_mut() {
            BlastRange::step(other);
            assert_eq!(
                r.state_hash(),
                other.state_hash(),
                "forested firing position replay tick {tick}"
            );
        } else if r.units[1].motion.destination.is_some() {
            replay = Some(BlastRange::restore(r.config, &r.snapshot()).unwrap());
        }
        if !r.battle.lasers.is_empty() {
            first.get_or_insert(tick);
        }
        if !r.units[0].status.alive {
            break;
        }
    }
    eprintln!(
        "spider forest first={first:?} at={:?} shots={} target_alive={}",
        r.units[1].at, r.battle.actors[1].fired, r.units[0].status.alive
    );
    assert!(replay.is_some(), "never planned a firing position");
    assert_ne!(
        r.units[1].motion.pos, start,
        "spider kept shooting from its blocked starting point"
    );
    assert!(first.is_some(), "spider never reached a firing position");
    assert_eq!(
        replans_before_fire, 1,
        "stationary target repeatedly restarted the lateral route"
    );
    assert!(
        !r.units[0].status.alive,
        "spider never damaged/finished the target beyond the forest"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn spider_breaches_meltable_cover_without_leaving_its_firing_position() {
    let mut layout = arena_of([RangeUnitKind::Spider, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [300, 16, 260],
        max: [304, 120, 410],
        material: 3,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Attack(1), None));
    let start = r.units[0].motion.pos;
    for _ in 0..400 {
        step_spider_opening_trial(&mut r);
        assert_eq!(
            r.units[0].motion.pos, start,
            "laser abandoned meltable cover"
        );
        if !r.units[1].status.alive {
            break;
        }
    }
    assert!(r.heat.melted_total > 0, "steel cover never melted");
    assert!(
        !r.units[1].status.alive,
        "laser did not reach the unit after breaching steel"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn forested_laser_preserves_move_route_and_explicit_point_fire() {
    let mut layout = arena_of([RangeUnitKind::Spider, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [320, 16, 260],
        max: [330, 120, 410],
        material: 4,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    let dest = [220, 480];
    assert!(r.battle_order(0, Order::Move, Some(dest)));
    for _ in 0..4 {
        step_spider_opening_trial(&mut r);
        assert_eq!(
            r.units[0].motion.destination,
            Some(dest),
            "blocked laser replaced a manual move"
        );
        assert!(r.battle.lasers.is_empty());
    }
    assert!(r.battle_order(0, Order::Ground([325, 80, 336]), None));
    for _ in 0..180 {
        step_spider_opening_trial(&mut r);
        if !r.battle.lasers.is_empty() {
            break;
        }
    }
    assert_eq!(
        r.battle.lasers.len(),
        1,
        "explicit foliage attack never fired"
    );
    assert!(r.battle.lasers[0].hit);
    assert!(r.heat.burning() > 0, "explicit beam did not ignite foliage");
    assert_eq!(
        r.units[0].motion.destination,
        Some(dest),
        "point fire redirected the existing move route"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn artillery_turret_lofts_over_low_cover_splashes_neighbours_and_replays() {
    let mut layout = arena_of([RangeUnitKind::ArtilleryTurretT2, RangeUnitKind::Tank])
        .layout
        .unwrap();
    layout.units.push(RangeUnitDef {
        allied: false,
        kind: RangeUnitKind::Tank,
        at: [450, 16, 384],
        weapon: None,
    });
    layout.units.push(RangeUnitDef {
        allied: false,
        kind: RangeUnitKind::Tank,
        at: [450, 16, 288],
        weapon: None,
    });
    layout.blocks.push(modding::range::RangeBlock {
        min: [300, 16, 250],
        max: [315, 48, 425],
        material: 2,
    });
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_battle().unwrap();
    for a in &mut r.battle.actors {
        a.order = Order::Hold;
    }
    assert!(r.battle_order(0, Order::Attack(1), None));
    let start = r.units[0].motion.pos;
    let before: Vec<_> = r.units.iter().map(|u| u.body.volume_mm3()).collect();
    let mut peak = 0;
    let mut impacts = Vec::new();
    let mut replay = None;
    let mut wave = false;
    for _ in 0..330 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        assert_eq!(r.units[0].motion.pos, start, "fixed artillery moved");
        for p in &r.battle.rounds {
            if p.owner == 0 {
                peak = peak.max(p.pos[1].to_int_trunc());
            }
        }
        for p in &r.battle.impacts {
            if p.weapon == Weapon::Canister {
                impacts.push(p.at);
            }
        }
        wave |= !r.battle.waves.is_empty();
        if let Some(other) = replay.as_mut() {
            let other: &mut BlastRange = other;
            other.battle.ai.next_tick = other.tick + other.battle.ai.tuning.decision_ticks;
            other.step();
            assert_eq!(r.state_hash(), other.state_hash());
        } else if !r.battle.waves.is_empty() {
            replay = Some(BlastRange::restore(r.config, &r.snapshot()).unwrap());
        }
        if r.battle.actors[0].fired == 1 {
            r.battle.actors[0].order = Order::Hold;
            if let Some(other) = replay.as_mut() {
                other.battle.actors[0].order = Order::Hold;
            }
        }
    }
    eprintln!(
        "loft peak={peak} impacts={impacts:?} remaining={:?}",
        r.units
            .iter()
            .map(|u| u.body.volume_mm3())
            .collect::<Vec<_>>()
    );
    assert!(peak > 65, "rounds flew flat");
    assert!(wave && replay.is_some());
    assert_eq!(
        impacts.len(),
        r.battle.tuning.canister.pellets,
        "salvo lost a submunition"
    );
    assert!(
        impacts.iter().all(|p| p[0] > 315),
        "low cover caught the arc"
    );
    for (id, &was) in before.iter().enumerate().take(4).skip(1) {
        assert!(
            r.units[id].body.volume_mm3() < was,
            "splash missed neighbouring tank {id}"
        );
    }
    let mut old = r.snapshot();
    old[8..12].copy_from_slice(&50u32.to_le_bytes());
    assert!(
        BlastRange::restore(r.config, &old).is_none(),
        "old canister semantics silently migrated"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn canister_reaches_full_attack_range_on_a_high_arc_and_saves_above_world_ceiling() {
    let mut l = arena_of([RangeUnitKind::ArtilleryTurretT2, RangeUnitKind::Tank])
        .layout
        .unwrap();
    let target_x = l.units[0].at[0] + modding::attack_ranges::stock().canister;
    l.size[0] = ((target_x + 300 + 15) / 16 * 16) as u32;
    l.units[1].at[0] = target_x;
    l.clear_max[0] = target_x + 200;
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.start_battle().unwrap();
    for a in &mut r.battle.actors {
        a.order = Order::Hold;
    }
    assert!(r.battle_order(0, Order::Attack(1), None));
    let mut contacted = false;
    let mut replay = None;
    let flight_ticks = modding::attack_ranges::stock().canister
        * r.battle.tuning.canister.flight_time_milli
        * crate::TICKS_PER_SECOND
        / (r.battle.canister_config.shell_speed * 1000);
    for _ in 0..flight_ticks + 180 {
        r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
        r.step();
        if r.battle.actors[0].fired > 0 {
            r.battle.actors[0].order = Order::Hold;
        }
        if let Some(other) = replay.as_mut() {
            let other: &mut BlastRange = other;
            other.battle.ai.next_tick = other.tick + other.battle.ai.tuning.decision_ticks;
            other.step();
            assert_eq!(r.state_hash(), other.state_hash());
        } else if r.battle.rounds.iter().any(|p| p.pos[1] > Fx::from_int(500)) {
            replay = Some(BlastRange::restore(r.config, &r.snapshot()).unwrap());
        }
        if r.battle
            .impacts
            .iter()
            .any(|i| i.weapon == Weapon::Canister && (i.at[0] - target_x).abs() < 100)
        {
            contacted = true;
            break;
        }
    }
    assert!(replay.is_some(), "never rose above the world ceiling");
    assert!(
        contacted,
        "full-range submunition vanished before reaching its target"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn t2_canister_finishes_a_stock_tank_with_at_most_three_salvos() {
    for kind in [RangeUnitKind::ArtilleryT2, RangeUnitKind::ArtilleryTurretT2] {
        let mut r = arena_of([kind, RangeUnitKind::Tank]);
        r.start_battle().unwrap();
        r.battle.actors[1].order = Order::Hold;
        assert!(r.battle_order(0, Order::Attack(1), None));
        for _ in 0..1000 {
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            r.step();
            if !r.units[1].status.alive {
                break;
            }
        }
        eprintln!(
            "T2 balance {kind:?} tick={} salvos={} core={} alive={}",
            r.tick, r.battle.actors[0].fired, r.units[1].integrity.core.0, r.units[1].status.alive
        );
        assert!(!r.units[1].status.alive, "T2 never finished its target");
        assert!(r.battle.actors[0].fired <= 3, "T2 took too many salvos");
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[path = "range_siege_tests.rs"]
mod siege_tests;

#[test]
fn player_commander_defends_without_automatically_pursuing_distant_enemy() {
    let mut r = arena_of([RangeUnitKind::Commander, RangeUnitKind::Tank]);
    std::sync::Arc::make_mut(&mut r.battle.tuning).energy_range = 100;
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert_eq!(r.battle.actors[0].order, Order::Auto);
    for _ in 0..3 { r.battle_tick(); }
    assert!(r.units[0].motion.destination.is_none(), "idle player commander pursued a distant target");
    assert!(r.battle_order(0, Order::Attack(1), None));
    r.battle_tick();
    assert!(r.units[0].motion.destination.is_some(), "explicit attack must still allow pursuit");
}
