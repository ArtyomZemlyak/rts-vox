use super::{arena, order, same};
use crate::blast::{
    battle::{
        commands::{runtime::*, Command, Outcome},
        Order,
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

#[test]
fn admission_rejections_leave_state_and_watermarks_unchanged() {
    let mut r = owner(4, 1);
    let before = r.state_hash();
    let mut request = hold(0, 0);
    request.epoch = 6;
    assert_eq!(r.submit(request), Err(AdmissionError::Epoch));
    request.epoch = 7;
    request.producer = 2;
    assert_eq!(r.submit(request), Err(AdmissionError::Producer));
    assert_eq!(
        r.submit(hold(1, 0)),
        Err(AdmissionError::Sequence { expected: 0 })
    );
    assert_eq!(r.submit(hold(0, u32::MAX)), Err(AdmissionError::Exhausted));
    assert_eq!(before, r.state_hash());
    let first = r.submit(hold(0, 0)).unwrap();
    assert_eq!(first.assigned_tick, 0);
    assert_eq!(first.outcome, None);
    assert!(r.acknowledge(first.id).is_none());
    r.step().unwrap();
    let before = r.state_hash();
    assert_eq!(r.submit(hold(1, 0)), Err(AdmissionError::PastTick));
    assert_eq!(before, r.state_hash());
    assert_eq!(r.submit(hold(1, 1)).unwrap().assigned_tick, 1);
}

#[test]
fn receipt_backpressure_never_drops_work_or_allows_duplicate_execution() {
    let mut r = owner(3, 1);
    let a = r.submit(hold(0, 0)).unwrap();
    let b = r.submit(hold(1, 0)).unwrap();
    let c = r.submit(hold(2, 0)).unwrap();
    assert_eq!(
        [a.assigned_tick, b.assigned_tick, c.assigned_tick],
        [0, 1, 2]
    );
    assert_eq!(r.submit(hold(3, 0)), Err(AdmissionError::Full));
    r.step().unwrap();
    assert_eq!(r.receipt(a.id).unwrap().outcome, Some(Outcome::Applied));
    assert_eq!(r.receipt(b.id).unwrap().outcome, None);
    assert_eq!(r.submit(hold(3, 1)), Err(AdmissionError::Full));
    assert_eq!(r.acknowledge(a.id).unwrap().outcome, Some(Outcome::Applied));
    assert_eq!(
        r.submit(hold(0, 1)),
        Err(AdmissionError::Sequence { expected: 3 })
    );
    let d = r.submit(hold(3, 1)).unwrap();
    assert_eq!(d.assigned_tick, 3);
    for receipt in [b, c, d] {
        assert_eq!(r.world().tick, receipt.assigned_tick);
        r.step().unwrap();
        assert_eq!(
            r.acknowledge(receipt.id).unwrap().outcome,
            Some(Outcome::Applied)
        );
    }
    assert_eq!(r.retained(), 0);
    assert!(r.acknowledge(a.id).is_none());
    assert_eq!(
        r.submit(hold(3, 4)),
        Err(AdmissionError::Sequence { expected: 4 })
    );
}

#[test]
fn scheduling_has_no_future_head_block_and_keeps_acceptance_ties() {
    let mut r = owner(5, 2);
    let future = r.submit(hold(0, 5)).unwrap();
    let hold = r
        .submit(envelope(1, 0, 0, order(0, Order::Hold, None)))
        .unwrap();
    let auto = r
        .submit(envelope(0, 1, 0, order(0, Order::Auto, None)))
        .unwrap();
    let rejected = r
        .submit(envelope(1, 1, 0, order(usize::MAX, Order::Auto, None)))
        .unwrap();
    assert_eq!(
        [
            hold.assigned_tick,
            auto.assigned_tick,
            rejected.assigned_tick
        ],
        [0, 0, 1]
    );
    r.step().unwrap();
    assert_eq!(r.world().battle.actors[0].order, Order::Auto);
    assert_eq!(r.receipt(future.id).unwrap().outcome, None);
    r.step().unwrap();
    assert_eq!(
        r.acknowledge(rejected.id).unwrap().outcome,
        Some(Outcome::Rejected)
    );
    for _ in 2..6 {
        r.step().unwrap();
    }
    assert_eq!(
        r.acknowledge(future.id).unwrap().outcome,
        Some(Outcome::Applied)
    );
}

#[test]
fn owned_commands_match_direct_kernel_in_real_combat_and_pending_checkpoint_replay() {
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Spider] {
        let mut world = arena(kind);
        world.start_battle().unwrap();
        let mut direct =
            crate::blast::BlastRange::restore(world.config, &world.snapshot()).unwrap();
        let mut r = CommandRuntime::new(
            world,
            7,
            Limits {
                retained: 8,
                producers: 2,
                per_tick: 2,
            },
        )
        .unwrap();
        let schedule = [
            (0, order(0, Order::Attack(1), None)),
            (
                30,
                Command::FormationMove {
                    actor: 0,
                    destination: [250, 336],
                    fallback: [210, 336],
                },
            ),
            (90, order(0, Order::Hold, None)),
            (120, order(0, Order::Attack(1), None)),
        ];
        let accepted: Vec<_> = schedule
            .iter()
            .enumerate()
            .map(|(seq, &(tick, command))| {
                r.submit(envelope(0, seq as u64, tick, command)).unwrap()
            })
            .collect();
        let mut replay = None;
        let mut fired = false;
        let mut damaged = false;
        for tick in 0..240 {
            for &(when, command) in &schedule {
                if tick == when {
                    direct.execute_battle_command(command);
                }
            }
            direct.step();
            r.step().unwrap();
            same(&direct, r.world());
            fired |= direct.battle.actors.iter().any(|actor| actor.fired > 0);
            damaged |= direct.units.iter().any(|unit| unit.revision > 0);
            if let Some(replica) = &mut replay {
                let replica: &mut CommandRuntime = replica;
                replica.step().unwrap();
                assert_eq!(r.state_hash(), replica.state_hash());
                same(r.world(), replica.world());
                for receipt in &accepted {
                    assert_eq!(r.receipt(receipt.id), replica.receipt(receipt.id));
                }
            }
            if tick == 70 {
                // Two completed receipts, two pending commands; preserve both.
                let restored = CommandRuntime::restore(r.world().config, r.checkpoint()).unwrap();
                assert_eq!(restored.state_hash(), r.state_hash());
                assert_eq!(restored.retained(), 4);
                replay = Some(restored);
            }
        }
        assert!(fired && damaged, "empty combat fixture {kind:?}");
        for receipt in accepted {
            assert!(r.receipt(receipt.id).unwrap().outcome.unwrap().accepted());
        }
        let replica = replay.as_mut().unwrap();
        assert_eq!(
            r.submit(hold(3, 240)),
            Err(AdmissionError::Sequence { expected: 4 })
        );
        assert_eq!(
            replica.submit(hold(3, 240)),
            Err(AdmissionError::Sequence { expected: 4 })
        );
        // A restored clone must have room for the configured capacity, even
        // when its original Vec spare capacity was not preserved by Clone.
        assert_eq!(r.submit(hold(4, 240)), replica.submit(hold(4, 240)));
        assert_eq!(r.state_hash(), replica.state_hash());
    }
}

#[test]
fn pending_ingress_changes_authoritative_hash_before_physics() {
    let mut a = owner(4, 1);
    let mut b = owner(4, 1);
    assert_eq!(a.state_hash(), b.state_hash());
    a.submit(hold(0, 30)).unwrap();
    b.submit(envelope(0, 0, 30, order(0, Order::Auto, None)))
        .unwrap();
    same(a.world(), b.world());
    assert_ne!(a.state_hash(), b.state_hash());
}

#[test]
fn old_battle_acknowledgement_cannot_remove_a_new_battle_receipt() {
    let mut old = owner(4, 1);
    let old_receipt = old.submit(hold(0, 0)).unwrap();
    old.step().unwrap();
    let mut world = arena(RangeUnitKind::Tank);
    world.start_battle().unwrap();
    let mut new = CommandRuntime::new(
        world,
        8,
        Limits {
            retained: 4,
            producers: 2,
            per_tick: 1,
        },
    )
    .unwrap();
    let new_receipt = new
        .submit(Envelope {
            epoch: 8,
            ..hold(0, 0)
        })
        .unwrap();
    new.step().unwrap();
    assert_eq!(old_receipt.id.ordinal, new_receipt.id.ordinal);
    let before = new.state_hash();
    assert!(new.receipt(old_receipt.id).is_none());
    assert!(new.acknowledge(old_receipt.id).is_none());
    assert_eq!(before, new.state_hash());
    assert_eq!(
        new.acknowledge(new_receipt.id).unwrap().outcome,
        Some(Outcome::Applied)
    );
}

#[test]
fn owner_rejects_invalid_setup_limits_and_clock_exhaustion() {
    let limits = Limits {
        retained: 4,
        producers: 2,
        per_tick: 1,
    };
    assert!(CommandRuntime::new(arena(RangeUnitKind::Tank), 7, limits).is_ok());
    let mut terminal = arena(RangeUnitKind::Tank);
    terminal.battle.phase = crate::blast::battle::Phase::Victory;
    assert!(CommandRuntime::new(terminal, 7, limits).is_err());
    for limits in [
        Limits {
            retained: 0,
            ..limits
        },
        Limits {
            producers: 0,
            ..limits
        },
        Limits {
            per_tick: 0,
            ..limits
        },
        Limits {
            retained: usize::MAX,
            ..limits
        },
    ] {
        let mut world = arena(RangeUnitKind::Tank);
        world.start_battle().unwrap();
        assert!(CommandRuntime::new(world, 7, limits).is_err());
    }
    let mut world = arena(RangeUnitKind::Tank);
    world.start_battle().unwrap();
    world.tick = u32::MAX;
    let mut r = CommandRuntime::new(world, 7, limits).unwrap();
    let before = r.state_hash();
    assert!(r.step().is_err());
    assert_eq!(before, r.state_hash());
}
