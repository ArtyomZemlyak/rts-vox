use super::*;
use crate::blast::{
    battle::{
        command_tests::{arena, same},
        Order,
    },
    RangeUnitKind,
};
use modding::battle_ai::Tuning;

#[cfg(test)]
fn owner(started: bool, budget: usize) -> CommandRuntime {
    let mut world = arena(RangeUnitKind::Tank);
    let mut tuning = *world.enemy_ai_tuning();
    tuning.decision_ticks = 30;
    world.configure_enemy_ai(tuning).unwrap();
    if started {
        world.start_battle().unwrap();
    }
    CommandRuntime::new(
        world,
        7,
        Limits {
            retained: 16,
            producers: 2,
            per_tick: budget,
        },
    )
    .unwrap()
}

#[cfg(test)]
fn enqueue(r: &mut CommandRuntime, sequence: u64, requested_tick: u32, tuning: Tuning) -> Receipt {
    r.submit(Envelope {
        epoch: 7,
        producer: 0,
        sequence,
        requested_tick,
        command: Command::ConfigureEnemyAi { tuning },
    })
    .unwrap()
}

#[cfg(test)]
fn plan(r: &mut CommandRuntime, sequence: u64) -> CommandId {
    r.submit_plan(PlanEnvelope {
        epoch: 7,
        producer: 1,
        sequence,
        decision_tick: r.world.battle.ai.next_tick,
        decisions: vec![Decision {
            actor: 1,
            order: AiOrder::Hold,
        }],
    })
    .unwrap()
}

#[cfg(test)]
fn replica(r: &CommandRuntime) -> CommandRuntime {
    let bytes = r.checkpoint().to_bytes();
    let cp = Checkpoint::from_bytes(
        &bytes,
        DecodeLimits {
            world_bytes: 32 * 1024 * 1024,
            runtime: r.inbox.limits,
            plan_decisions: 2,
        },
    )
    .unwrap();
    assert_eq!(bytes, cp.to_bytes());
    let result = CommandRuntime::restore(r.world.config, cp).unwrap();
    same(&r.world, &result.world);
    assert_eq!(r.state_hash(), result.state_hash());
    result
}

#[cfg(test)]
#[test]
fn ai_configuration_legacy_typed_validation_and_capture_match() {
    let mut direct = owner(true, 2).world;
    let mut typed = BlastRange::restore(direct.config, &direct.snapshot()).unwrap();
    direct.start_battle_command_capture(16).unwrap();
    typed.start_battle_command_capture(16).unwrap();
    let initial = *direct.enemy_ai_tuning();
    let mut choices = vec![initial, initial];
    for variant in 0..8 {
        let mut tuning = initial;
        match variant {
            0 => tuning.decision_ticks = 0,
            1 => tuning.decision_ticks = 301,
            2 => tuning.armed_bonus = -1,
            3 => tuning.armed_bonus = 10_001,
            4 => tuning.mobile_bonus = -1,
            5 => tuning.mobile_bonus = 10_001,
            6 => tuning.switch_margin = -1,
            _ => tuning.switch_margin = 10_001,
        }
        choices.push(tuning);
    }
    choices.push(Tuning {
        decision_ticks: 1,
        focus_fire: false,
        armed_bonus: 0,
        mobile_bonus: 10_000,
        switch_margin: 0,
    });
    choices.push(Tuning {
        decision_ticks: 300,
        focus_fire: true,
        armed_bonus: 10_000,
        mobile_bonus: 0,
        switch_margin: 10_000,
    });
    for tuning in choices {
        let before = typed.snapshot();
        let expected = if tuning.validate().is_ok() {
            Outcome::Applied
        } else {
            Outcome::Rejected
        };
        let result = direct.configure_enemy_ai(tuning);
        assert_eq!(result.is_ok(), expected == Outcome::Applied);
        if expected == Outcome::Rejected {
            assert_eq!(result.unwrap_err(), "invalid battle AI tuning");
        }
        assert_eq!(
            typed.execute_command(Command::ConfigureEnemyAi { tuning }),
            expected
        );
        if expected == Outcome::Rejected {
            assert_eq!(before, typed.snapshot());
        }
        same(&direct, &typed);
    }
    let a = direct.finish_battle_command_capture().unwrap();
    let b = typed.finish_battle_command_capture().unwrap();
    assert!(a.complete() && b.complete());
    assert_eq!(a.records, b.records);
    assert_eq!(a.records.len(), 12);
}

#[cfg(test)]
#[test]
fn ai_configuration_equal_values_restart_and_finalize_pending_plan_on_its_deadline() {
    let mut r = owner(true, 2);
    r.step().unwrap();
    let due = r.world.battle.ai.next_tick;
    let id = plan(&mut r, 0);
    let tuning = *r.world.enemy_ai_tuning();
    let command = enqueue(&mut r, 0, due, tuning);
    let mut restored = replica(&r);
    while r.world.tick <= due {
        r.step().unwrap();
        restored.step().unwrap();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
    assert_eq!(
        r.receipt(command.id).unwrap().outcome,
        Some(Outcome::Applied)
    );
    assert_eq!(
        r.plan_receipt(id).unwrap().state,
        PlanState::Reconfigured { tick: due }
    );
    assert_eq!(r.world.battle.actors[1].order, Order::Attack(0));
    assert_eq!(r.world.battle.ai.next_tick, due + tuning.decision_ticks);
    assert!(r.world.battle.ai.pending.is_none() && r.world.battle.plan_execution.is_none());
    replica(&r);
    assert!(r.acknowledge_plan(id).is_some());
    replica(&r);
}

#[cfg(test)]
#[test]
fn ai_configuration_cancelled_future_plan_allows_earlier_equal_or_later_deadline_and_ack_order() {
    for interval in [3, 26, 60] {
        let mut r = owner(true, 2);
        r.step().unwrap();
        let old_due = r.world.battle.ai.next_tick;
        let old = plan(&mut r, 0);
        let mut tuning = *r.world.enemy_ai_tuning();
        tuning.decision_ticks = interval;
        let command = enqueue(&mut r, 0, 4, tuning);
        let mut restored = replica(&r);
        while r.world.tick <= 4 {
            r.step().unwrap();
            restored.step().unwrap();
            assert_eq!(r.state_hash(), restored.state_hash());
        }
        assert_eq!(
            r.plan_receipt(old).unwrap().state,
            PlanState::Reconfigured { tick: 4 }
        );
        assert_eq!(r.world.battle.ai.next_tick, 4 + interval);
        assert_eq!(old_due, 30);
        // Keep the cancelled plan but acknowledge the command before save.
        assert!(r.acknowledge(command.id).is_some());
        let new = plan(&mut r, 1);
        let new_due = r.world.battle.ai.next_tick;
        let mut restored = replica(&r);
        while r.world.tick <= new_due {
            r.step().unwrap();
            restored.step().unwrap();
            assert_eq!(r.state_hash(), restored.state_hash());
        }
        assert!(
            matches!(r.plan_receipt(new).unwrap().state, PlanState::Executed { tick, .. } if tick == new_due)
        );
        assert_eq!(r.plan_receipt(old), restored.plan_receipt(old));
        replica(&r);
        assert!(r.acknowledge_plan(new).is_some());
        replica(&r);
        assert!(r.acknowledge_plan(old).is_some());
        replica(&r);
    }
}

#[cfg(test)]
#[test]
fn ai_configuration_invalid_values_preserve_pending_plan_until_it_executes() {
    let mut r = owner(true, 2);
    r.step().unwrap();
    let due = r.world.battle.ai.next_tick;
    let id = plan(&mut r, 0);
    let mut invalid = *r.world.enemy_ai_tuning();
    invalid.decision_ticks = 0;
    let command = enqueue(&mut r, 0, 4, invalid);
    let mut restored = replica(&r);
    while r.world.tick <= due {
        r.step().unwrap();
        restored.step().unwrap();
        assert_eq!(r.state_hash(), restored.state_hash());
        if r.world.tick == 5 {
            assert_eq!(
                r.receipt(command.id).unwrap().outcome,
                Some(Outcome::Rejected)
            );
            assert_eq!(r.plan_receipt(id).unwrap().state, PlanState::Delivered);
            assert_eq!(r.world.battle.ai.next_tick, due);
            restored = replica(&r);
        }
    }
    assert!(
        matches!(r.plan_receipt(id).unwrap().state, PlanState::Executed { tick, .. } if tick == due)
    );
    assert_eq!(r.world.battle.actors[1].order, Order::Hold);
    replica(&r);
}

#[cfg(test)]
#[test]
fn ai_configuration_queue_order_setup_live_combat_and_pending_restore_match_direct() {
    let mut r = owner(false, 2);
    r.world.set_ai_vs_ai(true);
    let mut direct = BlastRange::restore(r.world.config, &r.world.snapshot()).unwrap();
    let base = *r.world.enemy_ai_tuning();
    let receipts: Vec<_> = [
        (
            0,
            Command::ConfigureEnemyAi {
                tuning: Tuning {
                    decision_ticks: 7,
                    ..base
                },
            },
        ),
        (0, Command::BattleLifecycle(BattleLifecycle::StartBattle)),
        (
            0,
            Command::ConfigureEnemyAi {
                tuning: Tuning {
                    decision_ticks: 3,
                    ..base
                },
            },
        ),
        (
            25,
            Command::ConfigureEnemyAi {
                tuning: Tuning {
                    focus_fire: false,
                    ..base
                },
            },
        ),
        (
            50,
            Command::ConfigureEnemyAi {
                tuning: Tuning {
                    decision_ticks: 0,
                    ..base
                },
            },
        ),
        (80, Command::ConfigureEnemyAi { tuning: base }),
    ]
    .into_iter()
    .enumerate()
    .map(|(sequence, (requested_tick, command))| {
        r.submit(Envelope {
            epoch: 7,
            producer: 0,
            sequence: sequence as u64,
            requested_tick,
            command,
        })
        .unwrap()
    })
    .collect();
    assert_eq!(receipts[2].assigned_tick, 1);
    let mut restored = replica(&r);
    for tick in 0..240 {
        for receipt in receipts
            .iter()
            .filter(|receipt| receipt.assigned_tick == tick)
        {
            direct.execute_command(receipt.envelope.command);
        }
        direct.step();
        r.step().unwrap();
        restored.step().unwrap();
        same(&direct, &r.world);
        assert_eq!(r.state_hash(), restored.state_hash());
        if [0, 1, 24, 50, 100].contains(&tick) {
            restored = replica(&r);
        }
    }
    assert!(r.world.battle.actors.iter().any(|actor| actor.fired > 0));
    assert!(r.world.units.iter().any(|unit| unit.revision > 0));
    for (i, receipt) in receipts.iter().enumerate() {
        assert_eq!(
            r.receipt(receipt.id).unwrap().outcome,
            Some(if i == 4 {
                Outcome::Rejected
            } else {
                Outcome::Applied
            })
        );
        assert_eq!(r.receipt(receipt.id), restored.receipt(receipt.id));
    }
}

#[cfg(test)]
#[test]
fn ai_configuration_restore_rejects_impossible_cancellation_history() {
    let mut r = owner(true, 2);
    r.step().unwrap();
    plan(&mut r, 0);
    let tuning = Tuning {
        decision_ticks: 3,
        ..*r.world.enemy_ai_tuning()
    };
    enqueue(&mut r, 0, 4, tuning);
    while r.world.tick <= 4 {
        r.step().unwrap();
    }
    plan(&mut r, 1);
    for tick in [0, 5, 31] {
        let mut bad = r.checkpoint();
        bad.inbox.plans[0].state = PlanState::Reconfigured { tick };
        assert!(CommandRuntime::restore(r.world.config, bad).is_none());
    }
    let mut bad = r.checkpoint();
    bad.inbox.plans[1].delivered_tick = 4;
    assert!(CommandRuntime::restore(r.world.config, bad).is_none());
    replica(&r);
}

#[cfg(test)]
#[test]
fn ai_configuration_every_pending_field_participates_in_portable_hash() {
    let mut hashes = std::collections::BTreeSet::new();
    for variant in 0..6 {
        let mut r = owner(false, 2);
        let mut tuning = *r.world.enemy_ai_tuning();
        match variant {
            0 => {}
            1 => tuning.decision_ticks += 1,
            2 => tuning.focus_fire = !tuning.focus_fire,
            3 => tuning.armed_bonus += 1,
            4 => tuning.mobile_bonus += 1,
            _ => tuning.switch_margin += 1,
        }
        enqueue(&mut r, 0, 0, tuning);
        assert!(hashes.insert(r.state_hash()));
        let restored = replica(&r);
        assert_eq!(r.inbox.entries, restored.inbox.entries);
    }
}

#[cfg(test)]
#[test]
fn ai_configuration_checkpoint_retains_full_capacity_of_short_lifecycle_commands() {
    let mut r = owner(false, 2);
    for sequence in 0..16 {
        r.submit(Envelope {
            epoch: 7,
            producer: 0,
            sequence,
            requested_tick: 0,
            command: Command::BattleLifecycle(BattleLifecycle::StartBattle),
        })
        .unwrap();
    }
    // With 16 shortest receipts, assuming the old 60-byte minimum rejects
    // a valid queue before decoding: each of these records is only 59 bytes.
    let mut restored = replica(&r);
    for _ in 0..8 {
        r.step().unwrap();
        restored.step().unwrap();
        assert_eq!(r.state_hash(), restored.state_hash());
        restored = replica(&r);
    }
    assert_eq!(r.retained(), 16);
    assert_eq!(r.inbox.entries[0].outcome, Some(Outcome::Applied));
    assert!(r.inbox.entries[1..]
        .iter()
        .all(|entry| entry.outcome == Some(Outcome::Rejected)));
}
