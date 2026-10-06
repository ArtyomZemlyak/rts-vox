//! Real order/fire gates under a controlled, gradually released projectile pool.
use super::*;

fn contention() -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [
        RangeUnitKind::Tank,
        RangeUnitKind::ArtilleryT2,
        RangeUnitKind::Tank,
    ]
    .into_iter()
    .enumerate()
    .map(|(id, kind)| modding::range::RangeUnitDef {
        weapon: None,
        kind,
        at: [170 + id as i32 * 130, 16, 336],
        allied: id != 2,
    })
    .collect();
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap(),
        layout,
    )
    .unwrap();
    r.start_battle().unwrap();
    r.tick = 100;
    r.battle.trace_fire(true);
    r.battle.actors[2].order = Order::Hold;
    for id in 0..2 {
        let a = &mut r.battle.actors[id];
        a.order = Order::Ground([600, 20, 336]);
        a.target = Some(2);
        a.turret = crate::turret::Traverse::new(0);
        a.next = 0;
        a.charge = Some(if id == 0 { 100 } else { 0 });
    }
    r.battle.actors[0].weapon = Some(Weapon::Energy);
    // This fixture isolates capacity/FIFO gates after aiming has completed.
    // Artillery now needs both axes aligned to its real lofted launch.
    let aim = artillery::solve(
        &r.units[1],
        [600, 20, 336],
        Weapon::Canister,
        &r.battle.canister_config,
        r.battle.tuning.canister.flight_time_milli,
    )
    .unwrap();
    r.battle.actors[1].turret = crate::turret::Traverse::new(aim.yaw);
    r.battle.actors[1].elevation = crate::turret::Traverse::new(aim.pitch);
    r.units[1].turret_pitch = aim.pitch;
    r.units[1].update_pose(aim.yaw);
    for _ in 0..r.battle.tuning.max_waves {
        r.battle.rounds.push(Round {
            owner: 0,
            weapon: Weapon::Energy,
            pos: [Fx::from_int(220), Fx::from_int(80), Fx::from_int(336)],
            previous: [Fx::from_int(220), Fx::from_int(80), Fx::from_int(336)],
            velocity: [Fx::ONE, Fx::ZERO, Fx::ZERO],
            distance: Fx::from_int(10),
        });
    }
    r
}
fn orders(r: &mut BlastRange) {
    let mut b = std::mem::replace(&mut r.battle, Battle::new(&r.units, 1));
    r.battle_orders(&mut b, |_| {}, &|_, _| {});
    r.battle = b;
}
fn release_one(r: &mut BlastRange) {
    // Controlled completion: all production readiness, charging and launch code runs.
    r.battle.rounds.remove(0);
    r.tick += 1;
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    let a = &mut r.battle.actors[0];
    a.next = 0;
    a.charge = Some(r.tick); // A younger one-round competitor at every released slot.
    orders(r);
    assert!(
        r.battle.waves.len() + r.battle.rounds.iter().filter(|r| r.bursts()).count()
            <= r.battle.tuning.max_waves
    );
    assert!(r.battle.rounds.len() <= r.battle.tuning.max_rounds);
}

#[test]
fn older_full_salvo_accumulates_slots_ahead_of_younger_single_rounds() {
    let mut r = contention();
    for released in 1..=6 {
        release_one(&mut r);
        if released < 6 {
            assert_eq!(r.battle.actors[1].fired, 0, "partial salvo");
            assert_eq!(
                r.battle.actors[0].fired, 0,
                "younger shell stole a reserved slot"
            );
            assert_eq!(r.battle.fire_gate(1), Some(FireGate::WaveCapacity));
            assert_eq!(r.battle.fire_gate(0), Some(FireGate::BurstQueue));
        }
    }
    assert_eq!(r.battle.actors[1].fired, 1, "T2 starved");
    assert_eq!(
        r.battle
            .rounds
            .iter()
            .filter(|r| r.weapon == Weapon::Canister)
            .count(),
        6
    );
    assert_eq!(
        r.battle.actors[1].next,
        r.tick + r.battle.tuning.canister.reload_ticks
    );
    assert_eq!(r.battle.fire_gate(1), Some(FireGate::Fired));
}

#[test]
fn cancelled_or_disarmed_salvo_does_not_hold_admission() {
    for disabled in [false, true] {
        let mut r = contention();
        if disabled {
            r.units[1].status.weapon = false;
        } else {
            r.battle.actors[1].order = Order::Hold;
        }
        release_one(&mut r);
        assert_eq!(r.battle.actors[0].fired, 1);
        assert_eq!(r.battle.actors[1].fired, 0);
        assert_eq!(
            r.battle.fire_gate(1),
            Some(if disabled {
                FireGate::Disabled
            } else {
                FireGate::Hold
            })
        );
    }
}

#[test]
fn waiting_salvo_priority_and_diagnostics_survive_replay_without_snapshot_fields() {
    let mut a = contention();
    for _ in 0..3 {
        release_one(&mut a);
    }
    let bytes = a.snapshot();
    a.battle.trace_fire(false);
    assert_eq!(a.snapshot(), bytes);
    let mut b = BlastRange::restore(a.config.clone(), &bytes).expect("waiting salvo snapshot");
    b.battle.trace_fire(true);
    assert_eq!(a.state_hash(), b.state_hash());
    for _ in 0..3 {
        release_one(&mut a);
        release_one(&mut b);
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.snapshot(), b.snapshot());
    }
    assert_eq!(a.battle.actors[1].fired, 1);
}

#[test]
fn equal_ready_deadlines_have_an_explicit_owner_tie_break() {
    let mut r = contention();
    r.battle.actors[1].charge = Some(r.tick + 1);
    release_one(&mut r);
    assert_eq!(r.battle.actors[0].fired, 1);
    assert_eq!(r.battle.actors[1].fired, 0);
}

#[test]
fn charging_or_unaligned_salvo_does_not_reserve_capacity() {
    for charging in [false, true] {
        let mut r = contention();
        if charging {
            r.battle.actors[1].charge = Some(r.tick + 100);
        } else {
            r.battle.actors[1].turret = crate::turret::Traverse::new(fx::angle::FULL / 4);
        }
        release_one(&mut r);
        assert_eq!(r.battle.actors[0].fired, 1);
        assert_eq!(r.battle.actors[1].fired, 0);
        assert_eq!(
            r.battle.fire_gate(1),
            Some(if charging {
                FireGate::Charging
            } else {
                FireGate::Aiming
            })
        );
    }
}

#[test]
fn round_capacity_wait_is_distinct_from_wave_capacity() {
    let mut r = contention();
    r.battle.rounds.clear();
    for _ in 0..r.battle.tuning.max_rounds - 1 {
        r.battle.rounds.push(Round {
            owner: 0,
            weapon: Weapon::Kinetic,
            pos: [Fx::from_int(220), Fx::from_int(80), Fx::from_int(336)],
            previous: [Fx::from_int(220), Fx::from_int(80), Fx::from_int(336)],
            velocity: [Fx::ONE, Fx::ZERO, Fx::ZERO],
            distance: Fx::from_int(10),
        });
    }
    orders(&mut r);
    assert_eq!(r.battle.fire_gate(1), Some(FireGate::RoundCapacity));
    assert_eq!(r.battle.fire_gate(0), Some(FireGate::BurstQueue));
    assert_eq!(r.battle.actors[0].fired, 0);
    assert_eq!(r.battle.actors[1].fired, 0);
}
