use super::*;
use crate::blast::battle::{
    command_tests::{arena, same},
    Order,
};
use crate::blast::RangeUnitKind;

#[cfg(test)]
fn owner() -> CommandRuntime {
    let mut world = arena(RangeUnitKind::Tank);
    let mut tuning = *world.enemy_ai_tuning();
    tuning.decision_ticks = 30;
    world.configure_enemy_ai(tuning).unwrap();
    world.start_battle().unwrap();
    CommandRuntime::new(
        world,
        7,
        Limits {
            retained: 16,
            producers: 2,
            per_tick: 2,
        },
    )
    .unwrap()
}

#[cfg(test)]
fn order(sequence: u64, tick: u32) -> Envelope {
    Envelope {
        epoch: 7,
        producer: 0,
        sequence,
        requested_tick: tick,
        command: Command::Order {
            actor: 0,
            order: Order::Hold,
            destination: None,
        },
    }
}

#[cfg(test)]
fn plan(sequence: u64, decision_tick: u32) -> PlanEnvelope {
    PlanEnvelope {
        epoch: 7,
        producer: 1,
        sequence,
        decision_tick,
        decisions: vec![Decision {
            actor: 1,
            order: AiOrder::Hold,
        }],
    }
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
    let copy = CommandRuntime::restore(r.world.config, cp).unwrap();
    assert_eq!(r.state_hash(), copy.state_hash());
    same(r.world(), copy.world());
    copy
}

#[cfg(test)]
#[test]
fn reset_retires_pending_and_completed_ingress_and_isolates_new_ids() {
    let mut r = owner();
    let completed = r.submit(order(0, 0)).unwrap();
    let completed_plan = r.submit_plan(plan(0, 0)).unwrap();
    r.step().unwrap();
    let pending = r.submit(order(1, 30)).unwrap();
    let pending_plan = r.submit_plan(plan(1, 30)).unwrap();
    let acknowledged = r.submit(order(2, 1)).unwrap();
    r.step().unwrap();
    r.acknowledge(acknowledged.id).unwrap();
    r = replica(&r);
    let bits = r.world.world.bits().as_ptr();
    let config = r.world.config;
    let layout = r.world.layout.clone();
    let retired = r
        .reset_world(7, config, layout, &Validator::default())
        .unwrap();
    assert_eq!((retired.epoch, retired.next_epoch, retired.tick), (7, 8, 2));
    assert_eq!(
        retired.commands().collect::<Vec<_>>(),
        vec![
            RetiredCommand::Completed(Receipt {
                outcome: Some(Outcome::Applied),
                ..completed
            }),
            RetiredCommand::Cancelled(pending),
        ]
    );
    let plans = retired.plans().collect::<Vec<_>>();
    assert!(
        matches!(plans[0], RetiredPlan::Completed(p) if p.id == completed_plan && matches!(p.state, PlanState::Executed { tick: 0, .. }))
    );
    assert!(
        matches!(plans[1], RetiredPlan::Cancelled(p) if p.id == pending_plan && p.state == PlanState::Delivered)
    );
    assert_eq!(bits, r.world.world.bits().as_ptr());
    assert_eq!((r.epoch(), r.retained(), r.world.tick), (8, 0, 0));
    assert_eq!(r.world.battle.phase, Phase::Setup);
    assert!(r.world.battle.ai.pending.is_none() && r.world.battle.plan_execution.is_none());
    r = replica(&r);
    let before = r.state_hash();
    assert_eq!(r.submit(order(3, 2)), Err(AdmissionError::Epoch));
    assert_eq!(r.submit_plan(plan(2, 30)), Err(AdmissionError::Epoch));
    assert!(r.acknowledge(completed.id).is_none());
    assert!(r.acknowledge_plan(completed_plan).is_none());
    assert_eq!(before, r.state_hash());
    let new = r
        .submit(Envelope {
            epoch: 8,
            ..order(0, 0)
        })
        .unwrap();
    assert_eq!(new.id.ordinal, completed.id.ordinal);
    r.step().unwrap();
    assert!(r.acknowledge(completed.id).is_none());
    assert_eq!(
        r.acknowledge(new.id).unwrap().outcome,
        Some(Outcome::Applied)
    );
    assert_eq!(r.retained(), 0);
}

#[cfg(test)]
#[test]
fn reset_validation_stale_epoch_exhaustion_and_reservation_fail_before_mutation() {
    let mut r = owner();
    r.submit(order(0, 20)).unwrap();
    r.submit_plan(plan(0, 0)).unwrap();
    let validator = Validator::default();
    let config = r.world.config;
    for case in 0..5 {
        let mut config = config;
        let mut layout = r.world.layout.clone();
        let mut epoch = 7;
        match case {
            0 => epoch = 6,
            1 => config.edits_per_tick = 0,
            2 => layout.as_mut().unwrap().units[0].at[0] = i32::MAX,
            3 => {
                layout = None;
                config.energy = 0;
            }
            4 => r.inbox.limits.retained = usize::MAX, // deterministic allocation failure
            _ => unreachable!(),
        }
        let before = r.state_hash();
        let checkpoint = r.checkpoint().to_bytes();
        let bits = r.world.world.bits().as_ptr();
        let result = r.reset_world(epoch, config, layout, &validator);
        match case {
            0 => assert!(matches!(result, Err(ResetError::Epoch))),
            4 => assert!(matches!(result, Err(ResetError::Allocation(_)))),
            _ => assert!(matches!(result, Err(ResetError::World(_)))),
        }
        assert_eq!(before, r.state_hash());
        assert_eq!(checkpoint, r.checkpoint().to_bytes());
        assert_eq!(bits, r.world.world.bits().as_ptr());
        r.inbox.limits.retained = 16;
    }
    let mut exhausted =
        CommandRuntime::new(arena(RangeUnitKind::Tank), u64::MAX, r.inbox.limits).unwrap();
    let before = exhausted.state_hash();
    assert!(matches!(
        exhausted.reset_world(u64::MAX, config, r.world.layout.clone(), &validator),
        Err(ResetError::Exhausted)
    ));
    assert_eq!(before, exhausted.state_hash());
    // Recoverable failure does not consume a previously accepted AI plan.
    r.step().unwrap();
    assert!(matches!(
        r.inbox.plans[0].state,
        PlanState::Executed { tick: 0, .. }
    ));
}

#[cfg(test)]
#[test]
fn reset_full_queue_is_retired_without_drain_or_silent_drop() {
    let mut r = owner();
    for sequence in 0..16 {
        r.submit(order(sequence, 30)).unwrap();
    }
    assert_eq!(r.submit(order(16, 30)), Err(AdmissionError::Full));
    let retired = r
        .reset_world(
            7,
            r.world.config,
            r.world.layout.clone(),
            &Validator::default(),
        )
        .unwrap();
    assert_eq!(retired.commands().len(), 16);
    assert!(retired
        .commands()
        .all(|entry| matches!(entry, RetiredCommand::Cancelled(_))));
    assert_eq!(r.world.tick, 0);
    for sequence in 0..16 {
        r.submit(Envelope {
            epoch: 8,
            ..order(sequence, 30)
        })
        .unwrap();
    }
    r = replica(&r);
    assert_eq!(r.retained(), 16);
}

#[cfg(test)]
#[test]
fn reset_modes_sizes_and_navigation_pool_match_fresh_factories() {
    let mut r = owner();
    assert!(r.world.set_navigation_workers(2));
    let validator = Validator::default();
    let config = r.world.config;
    let battle = r.world.layout.clone().unwrap();
    let mut solo = battle.clone();
    solo.battle = false;
    solo.units.remove(0);
    let mut larger = solo.clone();
    larger.size[0] += 128;
    larger.size[2] += 128;
    for layout in [Some(solo), Some(larger), None, Some(battle)] {
        r.world.world.queue_dig(90, 8, 450);
        r.world.world.flush();
        r.world.world.queue_dig(91, 8, 450);
        let tuning = *r.world.enemy_ai_tuning();
        let mut fresh = if let Some(layout) = &layout {
            let mut fresh = BlastRange::with_layout(config, layout.clone()).unwrap();
            fresh.configure_enemy_ai(tuning).unwrap();
            fresh
        } else {
            BlastRange::new(config)
        };
        let _retired = r
            .reset_world(r.epoch(), config, layout, &validator)
            .unwrap();
        assert_eq!(r.world.worker_counts()[1], 2);
        assert!(r.world.navigation_pool.is_some());
        same(&r.world, &fresh);
        assert_eq!(r.world.world.bits(), fresh.world.bits());
        for _ in 0..12 {
            r.step().unwrap();
            fresh.step();
            same(&r.world, &fresh);
        }
        let _copy = replica(&r);
    }
}

#[cfg(test)]
#[test]
fn reset_combat_and_post_reset_checkpoint_keep_real_shots_and_damage() {
    let mut r = owner();
    for _ in 0..180 {
        r.step().unwrap();
    }
    assert!(r.world.battle.actors.iter().any(|a| a.fired > 0));
    let config = r.world.config;
    let layout = r.world.layout.clone().unwrap();
    let tuning = *r.world.enemy_ai_tuning();
    let _retired = r
        .reset_world(7, config, Some(layout.clone()), &Validator::default())
        .unwrap();
    let mut direct = BlastRange::with_layout(config, layout).unwrap();
    direct.configure_enemy_ai(tuning).unwrap();
    direct.set_ai_vs_ai(true);
    direct.start_battle().unwrap();
    for (sequence, command) in [
        BattleLifecycle::Autonomy { enabled: true },
        BattleLifecycle::StartBattle,
    ]
    .into_iter()
    .enumerate()
    {
        r.submit(Envelope {
            epoch: 8,
            command: Command::BattleLifecycle(command),
            ..order(sequence as u64, 0)
        })
        .unwrap();
    }
    r = replica(&r);
    for tick in 0..240 {
        r.step().unwrap();
        direct.step();
        same(&r.world, &direct);
        if tick == 60 || tick == 200 {
            r = replica(&r);
        }
    }
    assert!(r.world.battle.actors.iter().any(|a| a.fired > 0));
    assert!(r.world.units.iter().any(|u| !u.removed.is_empty()));
}

#[cfg(test)]
#[test]
fn reset_rejects_old_ai_frame_even_when_physical_start_state_matches() {
    let mut r = CommandRuntime::new(
        arena(RangeUnitKind::Tank),
        7,
        Limits {
            retained: 16,
            producers: 2,
            per_tick: 2,
        },
    )
    .unwrap();
    r.submit(Envelope {
        command: Command::BattleLifecycle(BattleLifecycle::StartBattle),
        ..order(0, 0)
    })
    .unwrap();
    let physical_start = r.world.snapshot();
    let frame = r.step_record_ai().unwrap().unwrap();
    let _retired = r
        .reset_world(
            7,
            r.world.config,
            r.world.layout.clone(),
            &Validator::default(),
        )
        .unwrap();
    r.submit(Envelope {
        epoch: 8,
        command: Command::BattleLifecycle(BattleLifecycle::StartBattle),
        ..order(0, 0)
    })
    .unwrap();
    assert_eq!(physical_start, r.world.snapshot());
    let before = r.state_hash();
    assert!(r.step_replay_ai(Some(frame)).is_err());
    assert_eq!(before, r.state_hash());
    let mut copy = replica(&r);
    let frame = r.step_record_ai().unwrap();
    copy.step_replay_ai(frame).unwrap();
    assert_eq!(copy.state_hash(), r.state_hash());
}

#[cfg(test)]
#[test]
fn reset_preserves_reconfigured_plan_result_and_allows_terminal_world_reset() {
    let mut r = owner();
    let id = r.submit_plan(plan(0, 0)).unwrap();
    r.submit(Envelope {
        command: Command::ConfigureEnemyAi {
            tuning: *r.world.enemy_ai_tuning(),
        },
        ..order(0, 0)
    })
    .unwrap();
    r.step().unwrap();
    r.world.battle.phase = Phase::Victory;
    let retired = r
        .reset_world(
            7,
            r.world.config,
            r.world.layout.clone(),
            &Validator::default(),
        )
        .unwrap();
    assert!(
        matches!(retired.plans().next().unwrap(), RetiredPlan::Completed(p) if p.id == id && p.state == PlanState::Reconfigured { tick: 0 })
    );
    assert_eq!(r.world.battle.phase, Phase::Setup);
    let _copy = replica(&r);
}
