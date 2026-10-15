//! T3 uses the shared ballistic, foundation, admission and matter services.
use super::*;

fn step_without_ai(r: &mut BlastRange) {
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.step();
}

#[test]
fn siege_single_shell_digs_soil_and_replays_flight_and_live_wave() {
    let mut r = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert_eq!(r.battle_weapon(0), Some(Weapon::Siege));
    assert_eq!(r.battle_attack_range(0), Some(2400));
    assert!(!r.battle_order(0, Order::Move, Some([220, 336])));
    assert!(!r.set_battle_weapon(0, Some(Weapon::Blast)));
    assert!(!r.set_battle_weapon(1, Some(Weapon::Siege)));
    let start = r.units[0].motion.pos;
    assert!(r.battle_order(0, Order::Ground([450, 15, 336]), None));
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
    assert_eq!(r.battle.actors[0].next, r.tick - 1 + 210);
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
    l.size[0] = 2800;
    l.clear_max[0] = 2700;
    l.units[1].at[0] = 2500;
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Ground([2500, 15, 336]), None));
    let mut replay = None;
    let mut hit = false;
    for _ in 0..500 {
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
            .any(|p| p.weapon == Weapon::Siege && (p.at[0] - 2500).abs() < 50);
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
    let mut r = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    assert!(r.set_ai_vs_ai(true));
    r.start_battle().unwrap();
    let obs = r.enemy_ai_observation().unwrap();
    assert!(!obs.units[0].mobile && !obs.units[0].can_reposition);
    assert!(obs.units[0].attack_ranges.iter().all(|&v| v == 2400));
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
    u.motion.hull.up = phys::V3::new(Fx::ONE, Fx::ONE, Fx::ZERO);
    u.update_pose(0);
    let p = u.world_pos([79, 39, 28]);
    assert!(
        (p[0] - 200).abs() > 80,
        "fixture must expose old clipping: {p:?}"
    );
    assert!(u.solid(p), "large tilted barrel became non-colliding");
    let map =
        crate::blast::HitFrames::new(&u, u.pose, &u.seg_frames, u.turret_delta, u.turret_pitch);
    assert!(map.hit(&u, p).is_some(), "blast capture clipped the barrel");
}
