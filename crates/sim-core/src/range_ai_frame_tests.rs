use super::*;
use crate::blast::{
    battle::{
        command_tests::{arena, order, same},
        commands::CommandPhase,
        Order,
    },
    RangeUnitKind,
};

#[test]
fn manual_handoff_on_ai_deadline_records_and_replays_without_allied_rules() {
    for command in [
        Command::Order {
            actor: 0,
            order: Order::Hold,
            destination: None,
        },
        Command::FormationMove {
            actor: 0,
            destination: [260, 360],
            fallback: [260, 360],
        },
        Command::FormationMove {
            actor: 0,
            destination: [-100, -100],
            fallback: [-100, -100],
        },
        Command::Order {
            actor: 1,
            order: Order::Hold,
            destination: None,
        },
    ] {
        let mut r = owner(RangeUnitKind::Tank, true);
        r.submit(Envelope {
            epoch: 7,
            producer: 0,
            sequence: 0,
            requested_tick: 0,
            command,
        })
        .unwrap();
        let mut replay = replica(&r);
        let before = replay.checkpoint().to_bytes();
        let frame = r.step_record_ai().unwrap().unwrap();
        let enemy_actor = matches!(command, Command::Order { actor: 1, .. });
        assert_eq!(frame.allied.is_some(), enemy_actor);
        let mut bad = frame.clone();
        bad.allied = if enemy_actor { None } else { Some(Vec::new()) };
        assert!(replay.step_replay_ai(Some(bad)).is_err());
        assert_eq!(before, replay.checkpoint().to_bytes());
        replay.step_replay_ai(Some(frame)).unwrap();
        assert_eq!(r.state_hash(), replay.state_hash());
        for _ in 0..31 {
            let frame = r.step_record_ai().unwrap();
            replay.step_replay_ai(frame).unwrap();
            assert_eq!(r.state_hash(), replay.state_hash());
        }
    }
}

#[cfg(test)]
#[test]
fn ai_configuration_record_replay_cancels_plan_and_rejects_missing_frame_before_mutation() {
    for valid in [true, false] {
        let mut r = owner(RangeUnitKind::Tank, true);
        let previous_frame = r.step_record_ai().unwrap().unwrap();
        let due = r.world.battle.ai.next_tick;
        let id = r
            .submit_plan(PlanEnvelope {
                epoch: 7,
                producer: 0,
                sequence: 0,
                decision_tick: due,
                decisions: vec![Decision {
                    actor: 1,
                    order: AiOrder::Hold,
                }],
            })
            .unwrap();
        let tuning = modding::battle_ai::Tuning {
            decision_ticks: if valid { 3 } else { 0 },
            ..*r.world.enemy_ai_tuning()
        };
        r.submit(Envelope {
            epoch: 7,
            producer: 0,
            sequence: 1,
            requested_tick: 1,
            command: Command::ConfigureEnemyAi { tuning },
        })
        .unwrap();
        let mut replay = replica(&r);
        let before = replay.checkpoint().to_bytes();
        let frame = r.step_record_ai().unwrap();
        assert_eq!(frame.is_some(), valid);
        if valid {
            assert!(replay.step_replay_ai(None).is_err());
            assert_eq!(before, replay.checkpoint().to_bytes());
            let mut bad = frame.clone().unwrap();
            bad.allied = None;
            assert!(replay.step_replay_ai(Some(bad)).is_err());
        } else {
            assert!(replay.step_replay_ai(Some(previous_frame)).is_err());
        }
        assert_eq!(before, replay.checkpoint().to_bytes());
        replay.step_replay_ai(frame).unwrap();
        assert_eq!(r.state_hash(), replay.state_hash());
        assert_eq!(
            r.plan_receipt(id).unwrap().state,
            if valid {
                PlanState::Reconfigured { tick: 1 }
            } else {
                PlanState::Delivered
            }
        );
        replay = replica(&replay);
        for _ in 0..31 {
            let frame = r.step_record_ai().unwrap();
            replay.step_replay_ai(frame).unwrap();
            same(r.world(), replay.world());
            assert_eq!(r.state_hash(), replay.state_hash());
        }
    }
}

#[cfg(test)]
#[test]
fn ai_configuration_preflight_respects_final_value_and_per_tick_spill() {
    let mut r = owner(RangeUnitKind::Tank, false);
    r.step().unwrap();
    r.inbox.limits.per_tick = 2;
    for (sequence, interval) in [3, 9, 0].into_iter().enumerate() {
        let tuning = modding::battle_ai::Tuning {
            decision_ticks: interval,
            ..*r.world.enemy_ai_tuning()
        };
        r.submit(Envelope {
            epoch: 7,
            producer: 0,
            sequence: sequence as u64,
            requested_tick: 1,
            command: Command::ConfigureEnemyAi { tuning },
        })
        .unwrap();
    }
    let mut replay = replica(&r);
    for tick in 1..12 {
        let frame = r.step_record_ai().unwrap();
        assert_eq!(frame.is_some(), tick == 1 || tick == 10);
        replay.step_replay_ai(frame).unwrap();
        assert_eq!(r.state_hash(), replay.state_hash());
    }
    assert_eq!(r.world.enemy_ai_tuning().decision_ticks, 9);
}

#[cfg(test)]
#[test]
fn ai_configuration_on_either_side_of_queued_launch_records_first_rules() {
    for launch_first in [true, false] {
        let mut r = prepared(&[], 2, false);
        let tuning = modding::battle_ai::Tuning {
            decision_ticks: 3,
            ..*r.world.enemy_ai_tuning()
        };
        let mut commands = [
            Command::ConfigureEnemyAi { tuning },
            Command::BattleLifecycle(BattleLifecycle::StartBattle),
        ];
        if launch_first {
            commands.reverse();
        }
        for (sequence, command) in commands.into_iter().enumerate() {
            r.submit(Envelope {
                epoch: 7,
                producer: 0,
                sequence: sequence as u64,
                requested_tick: 0,
                command,
            })
            .unwrap();
        }
        let mut replay = replica(&r);
        for tick in 0..7 {
            let frame = r.step_record_ai().unwrap();
            assert_eq!(frame.is_some(), tick % 3 == 0);
            replay.step_replay_ai(frame).unwrap();
            assert_eq!(r.state_hash(), replay.state_hash());
        }
    }
}

#[cfg(test)]
fn prepared(commands: &[BattleLifecycle], per_tick: usize, missing_enemy: bool) -> CommandRuntime {
    let mut world = arena(RangeUnitKind::Tank);
    if missing_enemy {
        let mut layout = world.layout.clone().unwrap();
        layout.units.retain(|unit| unit.allied);
        world = BlastRange::with_layout(world.config, layout).unwrap();
    }
    let mut r = CommandRuntime::new(
        world,
        7,
        Limits {
            retained: 8,
            producers: 1,
            per_tick,
        },
    )
    .unwrap();
    for (sequence, &command) in commands.iter().enumerate() {
        r.submit(Envelope {
            epoch: 7,
            producer: 0,
            sequence: sequence as u64,
            requested_tick: 0,
            command: Command::BattleLifecycle(command),
        })
        .unwrap();
    }
    r
}

#[cfg(test)]
#[test]
fn lifecycle_queued_launch_replays_first_ai_with_ordered_autonomy_and_spill() {
    use BattleLifecycle::*;
    for (commands, per_tick, first_ai, autonomous) in [
        (vec![Autonomy { enabled: true }, StartBattle], 2, 0, true),
        (vec![StartBattle, Autonomy { enabled: true }], 2, 0, false),
        (
            vec![Autonomy { enabled: true }, FreeControl, StartBattle],
            1,
            2,
            true,
        ),
        (
            vec![
                Autonomy { enabled: true },
                Autonomy { enabled: false },
                StartBattle,
            ],
            3,
            0,
            false,
        ),
    ] {
        let mut r = prepared(&commands, per_tick, false);
        let mut replay = replica(&r);
        let mut phases = 0;
        for tick in 0..31 {
            let frame = r.step_record_ai().unwrap();
            if tick <= first_ai {
                assert_eq!(frame.is_some(), tick == first_ai);
            }
            if let Some(frame) = &frame {
                assert_eq!(frame.allied.is_some(), autonomous);
                phases += 1;
            }
            replay.step_replay_ai(frame).unwrap();
            assert_eq!(r.state_hash(), replay.state_hash());
            same(r.world(), replay.world());
            if tick == first_ai {
                replay = replica(&replay);
            }
        }
        assert!(phases >= 2);
        assert_eq!(r.world.battle.autonomous, autonomous);
    }
}

#[cfg(test)]
#[test]
fn lifecycle_queued_launch_missing_or_wrong_frame_rejects_before_commands() {
    use BattleLifecycle::*;
    let mut r = prepared(&[Autonomy { enabled: true }, StartBattle], 2, false);
    let mut replay = replica(&r);
    let before = replay.checkpoint().to_bytes();
    assert!(replay.step_replay_ai(None).is_err());
    assert_eq!(before, replay.checkpoint().to_bytes());
    let frame = r.step_record_ai().unwrap().unwrap();
    for variant in 0..3 {
        let mut bad = frame.clone();
        match variant {
            0 => bad.allied = None,
            1 => bad.before_hash ^= 1,
            _ => bad.tick += 1,
        }
        assert!(replay.step_replay_ai(Some(bad)).is_err());
        assert_eq!(before, replay.checkpoint().to_bytes());
    }
    replay.step_replay_ai(Some(frame.clone())).unwrap();
    same(r.world(), replay.world());
    let after = replay.checkpoint().to_bytes();
    assert!(replay.step_replay_ai(Some(frame)).is_err());
    assert_eq!(after, replay.checkpoint().to_bytes());
}

#[cfg(test)]
#[test]
fn lifecycle_failed_launch_and_future_launch_do_not_require_ai_frame() {
    use BattleLifecycle::*;
    let mut source = prepared(&[StartBattle], 1, false);
    let frame = source.step_record_ai().unwrap().unwrap();
    for (commands, missing_enemy) in [
        (vec![StartBattle], true),
        (vec![FreeControl, StartBattle], false),
    ] {
        let mut r = prepared(&commands, 1, missing_enemy);
        let mut replay = replica(&r);
        let before = replay.checkpoint().to_bytes();
        assert!(replay.step_replay_ai(Some(frame.clone())).is_err());
        assert_eq!(before, replay.checkpoint().to_bytes());
        assert!(r.step_record_ai().unwrap().is_none());
        replay.step_replay_ai(None).unwrap();
        assert_eq!(r.state_hash(), replay.state_hash());
        same(r.world(), replay.world());
        assert_eq!(r.world.battle.phase, Phase::Setup);
    }
}

fn owner(kind: RangeUnitKind, autonomous: bool) -> CommandRuntime {
    let mut w = arena(kind);
    assert!(w.set_ai_vs_ai(autonomous));
    w.start_battle().unwrap();
    CommandRuntime::new(
        w,
        7,
        Limits {
            retained: 8,
            producers: 1,
            per_tick: 1,
        },
    )
    .unwrap()
}
fn replica(r: &CommandRuntime) -> CommandRuntime {
    let cp = Checkpoint::from_bytes(
        &r.checkpoint().to_bytes(),
        DecodeLimits {
            world_bytes: 16 * 1024 * 1024,
            runtime: r.inbox.limits,
            plan_decisions: 2,
        },
    )
    .unwrap();
    CommandRuntime::restore(r.world.config, cp).unwrap()
}
fn plan(r: &mut CommandRuntime, ai_order: AiOrder) -> CommandId {
    r.submit_plan(PlanEnvelope {
        epoch: 7,
        producer: 0,
        sequence: 0,
        decision_tick: 0,
        decisions: vec![Decision {
            actor: 1,
            order: ai_order,
        }],
    })
    .unwrap()
}

#[test]
fn portable_frames_replay_both_sides_manual_queue_and_retained_plan_receipts() {
    let mut r = owner(RangeUnitKind::Tank, true);
    let id = plan(&mut r, AiOrder::Attack(0));
    r.submit(Envelope {
        epoch: 7,
        producer: 0,
        sequence: 1,
        requested_tick: 1,
        command: order(0, Order::Hold, None),
    })
    .unwrap();
    let mut replay = replica(&r);
    r.world.start_battle_command_capture(256).unwrap();
    replay.world.start_battle_command_capture(256).unwrap();
    let mut phases = 0;
    for _ in 0..r.world().battle.ai.tuning.decision_ticks * 2 + 1 {
        let frame = r.step_record_ai().unwrap();
        let frame = frame.map(|f| {
            phases += 1;
            let bytes = f.to_bytes();
            let loaded = AiFrame::from_bytes(&bytes, 2).unwrap();
            assert_eq!(f, loaded);
            assert_eq!(bytes, loaded.to_bytes());
            loaded
        });
        replay.step_replay_ai(frame).unwrap();
        assert!(r.world.battle.ai_step.is_none() && replay.world.battle.ai_step.is_none());
        same(r.world(), replay.world());
        assert_eq!(r.state_hash(), replay.state_hash());
        assert_eq!(r.plan_receipt(id), replay.plan_receipt(id));
    }
    assert!(phases >= 3);
    let a = r.world.finish_battle_command_capture().unwrap();
    let b = replay.world.finish_battle_command_capture().unwrap();
    assert_eq!(a.records, b.records);
    assert!(a
        .records
        .iter()
        .any(|e| e.phase == CommandPhase::AfterMotion));
    assert_eq!(r.acknowledge_plan(id), replay.acknowledge_plan(id));
    assert_eq!(r.state_hash(), replay.state_hash());
}

#[test]
fn rejected_external_move_uses_recorded_fallback_and_preserves_phase_outcomes() {
    let mut r = owner(RangeUnitKind::TurretT1, false);
    let id = plan(&mut r, AiOrder::Move([450, 336]));
    let mut replay = replica(&r);
    let frame = r.step_record_ai().unwrap().unwrap();
    replay.step_replay_ai(Some(frame)).unwrap();
    assert_eq!(r.plan_receipt(id), replay.plan_receipt(id));
    let PlanState::Executed { attempts, .. } = &r.plan_receipt(id).unwrap().state else {
        panic!()
    };
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, Outcome::Rejected);
    assert!(attempts[1].fallback && attempts[1].outcome.accepted());
    same(r.world(), replay.world());
    assert_eq!(r.state_hash(), replay.state_hash());
}

#[test]
fn replay_consumes_recorded_inputs_instead_of_recomputing_rules() {
    let mut r = owner(RangeUnitKind::Tank, false);
    let mut replay = replica(&r);
    let mut frame = r.step_record_ai().unwrap().unwrap();
    assert!(matches!(frame.enemy[0].order, AiOrder::Attack(0)));
    // A deliberately different, structurally valid local record proves that
    // replay consumes it even though Rules would choose Attack in this world.
    frame.enemy[0].order = AiOrder::Hold;
    replay.step_replay_ai(Some(frame)).unwrap();
    assert_eq!(replay.world.battle.actors[1].order, Order::Hold);
    assert!(matches!(r.world.battle.actors[1].order, Order::Attack(0)));
    assert!(replay.world.battle.ai_step.is_none());
    // No replay mode/future tape leaks into save or the subsequent normal step.
    let mut restored = replica(&replay);
    for _ in 0..16 {
        replay.step().unwrap();
        restored.step().unwrap();
    }
    same(replay.world(), restored.world());
}

#[test]
fn deadline_hash_sides_duplicates_and_targets_reject_before_any_mutation() {
    let mut r = owner(RangeUnitKind::Tank, true);
    let mut replay = replica(&r);
    let f = r.step_record_ai().unwrap().unwrap();
    let initial = replay.checkpoint().to_bytes();
    assert!(replay.step_replay_ai(None).is_err());
    for variant in 0..7 {
        let mut bad = f.clone();
        match variant {
            0 => bad.tick += 1,
            1 => bad.before_hash ^= 1,
            2 => bad.allied = None,
            3 => bad.enemy[0].actor = 0,
            4 => bad.enemy.push(bad.enemy[0]),
            5 => bad.enemy[0].order = AiOrder::Attack(usize::MAX),
            _ => bad.enemy[0].order = AiOrder::Attack(1),
        }
        assert!(replay.step_replay_ai(Some(bad)).is_err());
        assert_eq!(initial, replay.checkpoint().to_bytes());
    }
    replay.step_replay_ai(Some(f.clone())).unwrap();
    let after = replay.state_hash();
    assert!(replay.step_replay_ai(Some(f)).is_err());
    assert_eq!(after, replay.state_hash());
    replay.step_replay_ai(None).unwrap();
}

#[test]
fn empty_enemy_and_terminal_steps_do_not_invoke_policy_on_replay() {
    // Real inactive actors, rather than changing derived damage flags that a
    // physical restore is allowed to recompute from intact geometry.
    let mut r = owner(RangeUnitKind::Base, true);
    let mut replay = replica(&r);
    let f = r.step_record_ai().unwrap().unwrap();
    assert!(f.enemy.is_empty() && f.allied.is_some());
    replay.step_replay_ai(Some(f)).unwrap();
    same(r.world(), replay.world());
    r.world.battle.phase = Phase::Victory;
    let mut replay = replica(&r);
    assert!(r.step_record_ai().unwrap().is_none());
    replay.step_replay_ai(None).unwrap();
    same(r.world(), replay.world());
}

fn reseal(v: &mut [u8]) {
    let end = v.len() - 8;
    let h = checksum(&v[..end]);
    v[end..].copy_from_slice(&h.to_le_bytes());
}
#[test]
fn portable_frame_decoder_bounds_counts_and_rejects_damage_and_noncanonical_tags() {
    let mut r = owner(RangeUnitKind::Tank, true);
    let f = r.step_record_ai().unwrap().unwrap();
    let bytes = f.to_bytes();
    for end in 0..bytes.len() {
        assert!(AiFrame::from_bytes(&bytes[..end], 2).is_none());
    }
    assert!(AiFrame::from_bytes(&bytes, 1).is_none());
    for offset in [0, 8, 24, 40] {
        let mut bad = bytes.clone();
        bad[offset] = 255;
        reseal(&mut bad);
        assert!(AiFrame::from_bytes(&bad, 2).is_none());
    }
    let mut bad = bytes.clone();
    bad[16] ^= 1;
    assert!(AiFrame::from_bytes(&bad, 2).is_none());
    let mut bad = bytes.clone();
    bad.insert(bad.len() - 8, 0);
    reseal(&mut bad);
    assert!(AiFrame::from_bytes(&bad, 2).is_none());
    let mut bad = bytes.clone();
    // Enemy Attack takes 17 bytes after the 32-byte header/count.
    bad[49] = 2;
    reseal(&mut bad);
    assert!(AiFrame::from_bytes(&bad, 2).is_none());
}

#[test]
fn codec_covers_all_orders_and_empty_allied_presence() {
    let f = AiFrame {
        tick: 19,
        before_hash: 42,
        enemy: vec![
            Decision {
                actor: 0,
                order: AiOrder::Attack(9),
            },
            Decision {
                actor: 1,
                order: AiOrder::Move([i32::MIN, i32::MAX]),
            },
            Decision {
                actor: 2,
                order: AiOrder::Hold,
            },
            Decision {
                actor: 3,
                order: AiOrder::Auto,
            },
        ],
        allied: Some(vec![]),
    };
    assert_eq!(AiFrame::from_bytes(&f.to_bytes(), 4), Some(f));
    let empty = AiFrame {
        tick: 0,
        before_hash: 0,
        enemy: vec![],
        allied: None,
    };
    assert_eq!(empty.to_bytes().len(), 41);
    assert_eq!(
        empty
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "5254534149463031010000000000000000000000000000000000000000000000007ef1ab6eb5173cf1"
    );
    assert_eq!(AiFrame::from_bytes(&empty.to_bytes(), 0), Some(empty));
}
