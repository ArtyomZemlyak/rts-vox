//! T3 uses the shared ballistic, foundation, admission and matter services.
use super::*;

fn step_without_ai(r: &mut BlastRange) {
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.step();
}

#[test]
fn wave_task_observers_are_balanced_and_do_not_change_combat() {
    let mut plain = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    plain.start_battle().unwrap();
    plain.battle.actors[1].order = Order::Hold;
    assert!(plain.battle_order(0, Order::Ground([580, 15, 336]), None));
    let mut observed = BlastRange::restore(plain.config, &plain.snapshot()).unwrap();
    let counts = std::sync::Mutex::new(std::collections::BTreeMap::<&str, usize>::new());
    for _ in 0..240 {
        plain.battle.ai.next_tick = u32::MAX;
        observed.battle.ai.next_tick = u32::MAX;
        plain.step();
        observed.step_observed_all_tasks(
            |_| {},
            |_| {},
            |_, _| {},
            |_, _| {},
            |stage, event| {
                if event.starts_with("physics_wave_") {
                    assert_eq!(stage, 6);
                    *counts.lock().unwrap().entry(event).or_default() += 1;
                }
            },
        );
        assert_eq!(plain.state_hash(), observed.state_hash());
    }
    let counts = counts.into_inner().unwrap();
    for family in [
        "capture",
        "heap",
        "filter",
        "read_ground",
        "body_map",
        "apply",
    ] {
        let begin = format!("physics_wave_{family}_begin");
        let end = format!("physics_wave_{family}_end");
        let n = counts.get(begin.as_str()).copied().unwrap_or(0);
        assert!(n > 0, "unobserved family: {family}");
        assert_eq!(Some(&n), counts.get(end.as_str()), "unbalanced {family}");
    }
    assert!(plain.matter.economy.ledger.balanced());
}

#[test]
fn siege_single_shell_digs_soil_and_replays_flight_and_live_wave() {
    let mut r = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert_eq!(r.battle_weapon(0), Some(Weapon::Siege));
    assert_eq!(
        r.battle_attack_range(0),
        Some(modding::attack_ranges::stock().siege)
    );
    assert!(!r.battle_order(0, Order::Move, Some([220, 336])));
    assert!(!r.set_battle_weapon(0, Some(Weapon::Blast)));
    assert!(!r.set_battle_weapon(1, Some(Weapon::Siege)));
    let start = r.units[0].motion.pos;
    // Keep the shooter outside the now larger, physically damaging blast.
    assert!(r.battle_order(0, Order::Ground([580, 15, 336]), None));
    for _ in 0..180 {
        step_without_ai(&mut r);
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert_eq!(r.battle.actors[0].fired, 1);
    assert_eq!(
        r.battle.rounds.len(),
        1,
        "T3 must not produce a canister fan"
    );
    assert_eq!(r.battle.rounds[0].weapon, Weapon::Siege);
    assert_eq!(
        r.battle.actors[0].next,
        r.tick - 1 + r.battle.tuning.siege.reload_ticks
    );
    assert!(r.battle_order(0, Order::Hold, None));
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).expect("T3 flight snapshot");
    let mut wave_replay = None;
    let mut soil = 0;
    let mut impacts = 0;
    let mut peak = 0;
    for _ in 0..240 {
        step_without_ai(&mut r);
        step_without_ai(&mut replay);
        assert_eq!(r.state_hash(), replay.state_hash());
        if let Some(copy) = wave_replay.as_mut() {
            step_without_ai(copy);
            assert_eq!(r.state_hash(), copy.state_hash());
        } else if !r.battle.waves.is_empty() {
            wave_replay = Some(BlastRange::restore(r.config, &r.snapshot()).expect("T3 live wave"));
        }
        for p in &r.battle.rounds {
            peak = peak.max(p.pos[1].to_int_trunc());
        }
        impacts += r
            .battle
            .impacts
            .iter()
            .filter(|p| p.weapon == Weapon::Siege)
            .count();
        soil += r
            .world
            .completed()
            .iter()
            .filter(|(edit, before)| edit.mat == 0 && matches!(before, 1 | 7 | 8))
            .count();
        assert_eq!(
            r.units[0].motion.pos, start,
            "foundation moved while firing"
        );
    }
    assert_eq!(impacts, 1);
    assert!(peak > 50, "no elevated ballistic arc");
    assert!(
        soil > 0 && wave_replay.is_some(),
        "no real terrain destruction"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn siege_full_range_survives_high_altitude_snapshot() {
    let mut l = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank])
        .layout
        .unwrap();
    let target_x = l.units[0].at[0] + modding::attack_ranges::stock().siege;
    l.size[0] = ((target_x + 300 + 15) / 16 * 16) as u32;
    l.clear_max[0] = target_x + 200;
    l.units[1].at[0] = target_x;
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Ground([target_x, 15, 336]), None));
    let mut replay = None;
    let mut hit = false;
    let flight_ticks = modding::attack_ranges::stock().siege
        * r.battle.tuning.siege.flight_time_milli
        * crate::TICKS_PER_SECOND
        / (r.battle.siege_config.shell_speed * 1000);
    for _ in 0..flight_ticks + 180 {
        step_without_ai(&mut r);
        if r.battle.actors[0].fired > 0 {
            r.battle.actors[0].order = Order::Hold;
        }
        if let Some(copy) = replay.as_mut() {
            step_without_ai(copy);
            assert_eq!(r.state_hash(), copy.state_hash());
        } else if r.battle.rounds.iter().any(|p| p.pos[1] > Fx::from_int(500)) {
            replay = Some(BlastRange::restore(r.config, &r.snapshot()).expect("high T3 shell"));
        }
        hit |= r
            .battle
            .impacts
            .iter()
            .any(|p| p.weapon == Weapon::Siege && (p.at[0] - target_x).abs() < 50);
        if hit {
            break;
        }
    }
    assert!(
        replay.is_some() && hit,
        "T3 did not reach its advertised range"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn siege_stationary_ai_uses_its_actual_weapon_reach() {
    let mut layout = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank])
        .layout
        .unwrap();
    // Target the added half of the weapon's reach, not a nearby tank that
    // would also be hittable with the former balance.
    let target_x = layout.units[0].at[0] + modding::attack_ranges::stock().siege * 4 / 5;
    layout.size[0] = ((target_x + 300 + 15) / 16 * 16) as u32;
    layout.clear_max[0] = target_x + 200;
    layout.units[1].at[0] = target_x;
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, layout).unwrap();
    assert!(r.set_ai_vs_ai(true));
    r.start_battle().unwrap();
    let obs = r.enemy_ai_observation().unwrap();
    assert!(!obs.units[0].mobile && !obs.units[0].can_reposition);
    assert!(obs.units[0]
        .attack_ranges
        .iter()
        .all(|&v| v == modding::attack_ranges::stock().siege));
    for _ in 0..240 {
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert!(r.battle.actors[0].fired > 0, "AI never fired T3");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn siege_large_tilted_barrel_remains_hittable_beyond_old_eight_metre_cutoff() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::ArtilleryTurretT3, [200, 24, 200]);
    u.motion.hull.up = phys::V3::new(Fx::from_ratio(1, 4), Fx::ONE, Fx::ZERO);
    u.update_pose(0);
    let p = u.world_pos([111, 10, 28]);
    assert!(
        (p[0] - 200).abs() > 80,
        "fixture must expose old clipping: {p:?}"
    );
    assert!(u.solid(p), "large tilted barrel became non-colliding");
    let map =
        crate::blast::HitFrames::new(&u, u.pose, &u.seg_frames, u.turret_delta, u.turret_pitch);
    assert!(map.hit(&u, p).is_some(), "blast capture clipped the barrel");
}
