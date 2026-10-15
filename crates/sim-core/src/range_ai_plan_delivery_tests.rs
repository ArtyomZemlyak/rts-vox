use super::*;
use crate::blast::{
    ai::{AiOrder, Decision, Policy, Rules},
    battle::{
        command_tests::{arena, order, same},
        commands::CommandPhase,
        Order, Phase,
    },
    RangeUnitKind,
};
fn owner(capacity: usize, budget: usize) -> CommandRuntime {
    let mut world = arena(RangeUnitKind::Tank);
    world.start_battle().unwrap();
    CommandRuntime::new(
        world,
        7,
        Limits {
            retained: capacity,
            producers: 2,
            per_tick: budget,
        },
    )
    .unwrap()
}
fn envelope(producer: usize, sequence: u64, tick: u32, command: Command) -> Envelope {
    Envelope {
        epoch: 7,
        producer,
        sequence,
        requested_tick: tick,
        command,
    }
}
fn hold(sequence: u64, tick: u32) -> Envelope {
    envelope(0, sequence, tick, order(0, Order::Hold, None))
}

fn plan(sequence: u64, tick: u32, order: AiOrder) -> PlanEnvelope {
    PlanEnvelope {
        epoch: 7,
        producer: 0,
        sequence,
        decision_tick: tick,
        decisions: vec![Decision { actor: 1, order }],
    }
}

#[test]
fn common_rules_adapter_covers_living_inactive_actors_without_selecting_them() {
    let mut world = arena(RangeUnitKind::Tank);
    world.start_battle().unwrap();
    world.units[1].status.weapon = false;
    world.units[1].status.engine = false;
    let observation = world.enemy_ai_observation().unwrap();
    let rules = Rules {
        tuning: world.enemy_ai_tuning(),
        radii: &modding::battle_ai::Radii::stock(),
        controlled_allied: false,
    };
    assert!(rules.decide(&observation).is_empty());
    let decisions = rules.whole_plan(&observation);
    assert_eq!(decisions, plan(0, 0, AiOrder::Hold).decisions);
    let mut r = CommandRuntime::new(
        world,
        7,
        Limits {
            retained: 2,
            producers: 1,
            per_tick: 1,
        },
    )
    .unwrap();
    let id = r
        .submit_plan(PlanEnvelope {
            decisions,
            ..plan(0, 0, AiOrder::Hold)
        })
        .unwrap();
    r.step().unwrap();
    assert!(r.world.units[1].status.alive);
    assert_eq!(
        r.plan_receipt(id).unwrap().state,
        PlanState::Executed {
            tick: 0,
            attempts: vec![]
        }
    );
}

#[test]
fn whole_plan_validation_is_atomic_and_shares_command_ids_sequences_and_capacity() {
    let mut r = owner(2, 1);
    let initial = r.state_hash();
    for bad in [
        PlanEnvelope {
            epoch: 6,
            ..plan(0, 0, AiOrder::Hold)
        },
        PlanEnvelope {
            producer: 2,
            ..plan(0, 0, AiOrder::Hold)
        },
        plan(1, 0, AiOrder::Hold),
        plan(0, 99, AiOrder::Hold),
        PlanEnvelope {
            decisions: vec![],
            ..plan(0, 0, AiOrder::Hold)
        },
        PlanEnvelope {
            decisions: vec![Decision {
                actor: 0,
                order: AiOrder::Hold,
            }],
            ..plan(0, 0, AiOrder::Hold)
        },
        plan(0, 0, AiOrder::Attack(usize::MAX)),
        plan(0, 0, AiOrder::Move([-1, 336])),
        PlanEnvelope {
            decisions: vec![
                Decision {
                    actor: 1,
                    order: AiOrder::Hold
                };
                3
            ],
            ..plan(0, 0, AiOrder::Hold)
        },
    ] {
        assert!(r.submit_plan(bad).is_err());
        assert_eq!(initial, r.state_hash());
    }
    let id = r.submit_plan(plan(0, 0, AiOrder::Hold)).unwrap();
    assert_eq!(r.plan_receipt(id).unwrap().state, PlanState::Delivered);
    assert!(r.acknowledge_plan(id).is_none());
    let before = r.state_hash();
    assert!(matches!(
        r.submit_plan(plan(1, 0, AiOrder::Hold)),
        Err(AdmissionError::Plan("AI plan already submitted"))
    ));
    assert_eq!(before, r.state_hash());
    let manual = r.submit(hold(1, 0)).unwrap();
    assert_eq!(manual.id.ordinal, id.ordinal + 1);
    assert_eq!(r.retained(), 2);
    assert_eq!(
        r.submit_plan(plan(2, 0, AiOrder::Auto)),
        Err(AdmissionError::Full)
    );
    r.step().unwrap();
    assert!(matches!(
        r.plan_receipt(id).unwrap().state,
        PlanState::Executed { tick: 0, .. }
    ));
    let wrong_epoch = CommandId { epoch: 6, ..id };
    assert!(r.acknowledge_plan(wrong_epoch).is_none());
    assert_eq!(r.submit(hold(2, 1)), Err(AdmissionError::Full));
    assert!(r.acknowledge_plan(id).is_some());
    assert_eq!(
        r.submit_plan(plan(0, 0, AiOrder::Hold)),
        Err(AdmissionError::Sequence { expected: 2 })
    );
    let tick = r.world().enemy_ai_observation().unwrap().decision_tick;
    let id = r.submit_plan(plan(2, tick, AiOrder::Attack(0))).unwrap();
    assert_eq!(id.ordinal, 2);
    let before = r.state_hash();
    assert!(r.submit_plan(plan(3, 0, AiOrder::Hold)).is_err());
    assert_eq!(before, r.state_hash());
}

#[test]
fn retained_plan_storage_does_not_inherit_unbounded_caller_spare_capacity() {
    let mut r = owner(2, 1);
    let mut decisions = Vec::with_capacity(4096);
    decisions.push(Decision {
        actor: 1,
        order: AiOrder::Hold,
    });
    assert!(decisions.capacity() > r.world().units.len());
    let id = r
        .submit_plan(PlanEnvelope {
            decisions,
            ..plan(0, 0, AiOrder::Hold)
        })
        .unwrap();
    assert!(r.plan_receipt(id).unwrap().envelope.decisions.capacity() <= r.world().units.len());
    assert!(
        r.world
            .battle
            .plan_execution
            .as_ref()
            .unwrap()
            .attempts
            .capacity()
            <= 2 * r.world().units.len()
    );
    r.step().unwrap();
    assert!(matches!(
        r.plan_receipt(id).unwrap().state,
        PlanState::Executed { .. }
    ));
}

#[test]
fn plan_reports_selected_rejection_and_actual_fallback_in_the_ai_phase() {
    let mut world = arena(RangeUnitKind::TurretT1);
    world.start_battle().unwrap();
    world.start_battle_command_capture(20).unwrap();
    let mut r = CommandRuntime::new(
        world,
        7,
        Limits {
            retained: 3,
            producers: 1,
            per_tick: 1,
        },
    )
    .unwrap();
    let id = r
        .submit_plan(plan(0, 0, AiOrder::Move([450, 336])))
        .unwrap();
    r.step().unwrap();
    let PlanState::Executed { tick, attempts } = &r.plan_receipt(id).unwrap().state else {
        panic!("unexecuted plan")
    };
    assert_eq!(*tick, 0);
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[0],
        PlanAttempt {
            decision: Decision {
                actor: 1,
                order: AiOrder::Move([450, 336])
            },
            outcome: Outcome::Rejected,
            fallback: false
        }
    );
    assert!(attempts[1].fallback && attempts[1].outcome.accepted());
    assert_eq!(attempts[1].decision.actor, 1);
    let attempts = attempts.clone();
    // Independent observer must agree on outcome, source order and phase.
    let trace = r
        .world
        .battle
        .command_capture
        .take()
        .unwrap()
        .finish(r.world.tick);
    assert_eq!(trace.records.len(), 2);
    for (record, attempt) in trace.records.iter().zip(&attempts) {
        assert_eq!(record.outcome, attempt.outcome);
        assert_eq!(record.phase, CommandPhase::AfterMotion);
        assert_eq!(record.assigned_tick, 0);
    }
}

#[test]
fn pending_plan_checkpoint_replays_execution_and_preserves_completed_receipts() {
    let mut r = owner(4, 1);
    r.step().unwrap();
    let before = r.state_hash();
    assert_eq!(
        r.submit_plan(plan(0, 0, AiOrder::Hold)),
        Err(AdmissionError::Plan("stale or unscheduled AI tick"))
    );
    assert_eq!(before, r.state_hash());
    let tick = r.world().enemy_ai_observation().unwrap().decision_tick;
    let id = r.submit_plan(plan(0, tick, AiOrder::Attack(0))).unwrap();
    let mut replica = CommandRuntime::restore(r.world().config, r.checkpoint()).unwrap();
    assert_eq!(r.state_hash(), replica.state_hash());
    while r.world().tick <= tick {
        r.step().unwrap();
        replica.step().unwrap();
        same(r.world(), replica.world());
        assert_eq!(r.state_hash(), replica.state_hash());
        assert_eq!(r.plan_receipt(id), replica.plan_receipt(id));
    }
    assert!(
        matches!(r.plan_receipt(id).unwrap().state, PlanState::Executed { tick: applied, .. } if applied == tick)
    );
    let mut restored = CommandRuntime::restore(r.world().config, r.checkpoint()).unwrap();
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(r.acknowledge_plan(id), restored.acknowledge_plan(id));
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn delivered_actor_that_dies_before_decision_is_reported_as_not_selected() {
    let mut r = owner(2, 1);
    r.step().unwrap();
    let tick = r.world().enemy_ai_observation().unwrap().decision_tick;
    let id = r.submit_plan(plan(0, tick, AiOrder::Hold)).unwrap();
    // Isolate phase validation from the geometry of a weapon hit. Production
    // callers cannot mutate the owned world; death here models earlier damage.
    r.world.units[1].status.alive = false;
    r.world.tick = tick;
    r.step().unwrap();
    let PlanState::Executed { attempts, .. } = &r.plan_receipt(id).unwrap().state else {
        panic!("unexecuted plan")
    };
    assert!(attempts.is_empty());
    assert_eq!(r.plan_receipt(id).unwrap().envelope.decisions.len(), 1);
}

#[test]
fn battle_end_cancels_future_plan_and_keeps_the_snapshot_loadable() {
    let mut r = owner(2, 1);
    r.step().unwrap();
    let tick = r.world().enemy_ai_observation().unwrap().decision_tick;
    assert!(tick > r.world().tick);
    let id = r.submit_plan(plan(0, tick, AiOrder::Hold)).unwrap();
    r.world.units[1].status.alive = false;
    let end_tick = r.world().tick;
    r.step().unwrap();
    assert_eq!(r.world().battle.phase, Phase::Victory);
    assert_eq!(
        r.plan_receipt(id).unwrap().state,
        PlanState::Cancelled { tick: end_tick }
    );
    assert!(r.world().battle.ai.pending.is_none());
    let restored = CommandRuntime::restore(r.world().config, r.checkpoint()).unwrap();
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(r.plan_receipt(id), restored.plan_receipt(id));
}

#[test]
fn owned_whole_plans_match_direct_delivery_through_real_tank_and_spider_battles() {
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Spider] {
        let mut world = arena(kind);
        world.start_battle().unwrap();
        let mut direct =
            crate::blast::BlastRange::restore(world.config, &world.snapshot()).unwrap();
        let mut r = CommandRuntime::new(
            world,
            7,
            Limits {
                retained: 4,
                producers: 1,
                per_tick: 1,
            },
        )
        .unwrap();
        let mut sequence = 0;
        let mut pending = None;
        let mut fired = false;
        let mut damaged = false;
        for _ in 0..240 {
            if let Some(observation) = r.world().enemy_ai_observation() {
                if observation.decision_tick == r.world().tick
                    && observation
                        .units
                        .iter()
                        .any(|unit| unit.id == 1 && unit.alive)
                    // Real combat may kill the target before the next AI
                    // decision. The admission API correctly rejects Attack
                    // on a dead target; this fixture compares valid delivery.
                    && observation.units.iter().any(|unit| unit.id == 0 && unit.alive)
                {
                    let envelope = plan(sequence, observation.decision_tick, AiOrder::Attack(0));
                    direct
                        .submit_enemy_ai_plan(envelope.decision_tick, envelope.decisions.clone())
                        .unwrap();
                    pending = Some(r.submit_plan(envelope).unwrap());
                    sequence += 1;
                }
            }
            direct.step();
            r.step().unwrap();
            same(&direct, r.world());
            if let Some(id) = pending.take() {
                let receipt = r.acknowledge_plan(id).unwrap();
                assert!(matches!(receipt.state, PlanState::Executed { .. }));
            }
            fired |= direct.battle.actors.iter().any(|actor| actor.fired > 0);
            damaged |= direct.units.iter().any(|unit| unit.revision > 0);
        }
        assert!(
            sequence > 1 && fired && damaged,
            "empty combat fixture {kind:?}"
        );
    }
}
