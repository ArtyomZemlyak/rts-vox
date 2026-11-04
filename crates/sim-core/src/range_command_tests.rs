use super::commands::{Command, CommandPhase, Outcome, Producer};
use super::*;

#[path = "range_command_runtime_tests.rs"]
mod runtime_tests;

pub(super) fn arena(kind: RangeUnitKind) -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [(170, true), (450, false)]
        .map(|(x, allied)| modding::range::RangeUnitDef {
            weapon: None,
            kind,
            at: [x, 16, 336],
            allied,
        })
        .to_vec();
    // Large logistics/factory templates no longer fit the old 64 m arena.
    let oversized = layout.units.iter().any(|def| {
        let (lo, hi) = RangeUnit::of_kind(def.kind, def.at).bounds();
        (0..3).any(|a| lo[a] < 1 || hi[a] >= layout.size[a] as i32 - 1)
    });
    if oversized {
        layout.size = [4096, 2048, 4096];
        layout.units[0].at = [1000, 16, 1600];
        layout.units[1].at = [3000, 16, 1600];
        layout.clear_max = [4032, 4032];
    }
    BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap()
}
pub(super) fn order(actor: usize, order: Order, destination: Option<[i32; 2]>) -> Command {
    Command::Order {
        actor,
        order,
        destination,
    }
}
pub(super) fn same(a: &BlastRange, b: &BlastRange) {
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(a.snapshot(), b.snapshot());
    assert!(a.matter.economy.ledger.balanced() && b.matter.economy.ledger.balanced());
}

#[test]
fn command_receipts_distinguish_rejection_failed_search_and_deferred_formation() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle_command_capture(12).unwrap();
    assert_eq!(
        r.execute_battle_command(order(0, Order::Attack(1), None)),
        Outcome::Rejected
    );
    assert_eq!(
        r.execute_battle_command(Command::FormationMove {
            actor: 0,
            destination: [-1, 336],
            fallback: [210, 336],
        }),
        Outcome::Queued
    );
    assert_eq!(r.units[0].motion.destination, Some([210, 336]));
    assert_eq!(
        r.execute_battle_command(order(0, Order::Move, Some([-1, 336]))),
        Outcome::PathFailed
    );
    assert!(Outcome::PathFailed.admitted());
    assert!(!Outcome::PathFailed.accepted());
    assert_eq!(
        r.execute_battle_command(order(0, Order::Hold, None)),
        Outcome::Applied
    );
    let trace = r.finish_battle_command_capture().unwrap();
    assert!(trace.complete());
    assert_eq!(
        trace.records.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert!(trace.records.iter().all(|r| r.producer == Producer::Player
        && r.phase == CommandPhase::BetweenTicks
        && r.requested_tick == 0
        && r.assigned_tick == 0));
}

#[test]
fn command_capture_records_external_choice_and_fallback_after_motion() {
    let mut r = arena(RangeUnitKind::TurretT1);
    r.start_battle().unwrap();
    r.start_battle_command_capture(32).unwrap();
    assert!(r.submit_enemy_ai_plan(99, vec![]).is_err());
    // Plan submission validates coordinates. Actual movement also validates
    // the actor's emplacement capability; the rules then supply a fallback.
    r.submit_enemy_ai_plan(
        0,
        vec![ai::Decision {
            actor: 1,
            order: ai::AiOrder::Move([450, 336]),
        }],
    )
    .unwrap();
    r.step();
    assert!(r.battle.ai.pending.is_none());
    let trace = r.finish_battle_command_capture().unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), 2);
    assert_eq!(trace.records[0].producer, Producer::EnemyPlan);
    assert_eq!(trace.records[0].outcome, Outcome::Rejected);
    assert_eq!(trace.records[1].producer, Producer::EnemyFallback);
    assert!(trace.records[1].outcome.accepted());
    assert!(trace
        .records
        .iter()
        .all(|r| r.phase == CommandPhase::AfterMotion
            && r.requested_tick == 0
            && r.assigned_tick == 0
            && r.sequence == 0));
}

#[test]
fn command_capture_does_not_change_real_combat_or_midfight_restore() {
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Spider] {
        let (mut a, mut b) = (arena(kind), arena(kind));
        a.set_ai_vs_ai(true);
        b.set_ai_vs_ai(true);
        a.start_battle().unwrap();
        b.start_battle().unwrap();
        a.start_battle_command_capture(4096).unwrap();
        let mut fired = false;
        let mut damage = false;
        for tick in 0..240 {
            if tick == 40 {
                assert_eq!(
                    a.battle_group_move(&[(0, [210, 336])], [210, 336]),
                    b.battle_group_move(&[(0, [210, 336])], [210, 336])
                );
            }
            if tick == 80 || tick == 160 {
                assert_eq!(
                    a.battle_order(0, Order::Attack(1), None),
                    b.battle_order(0, Order::Attack(1), None)
                );
            }
            a.step();
            b.step();
            same(&a, &b);
            fired |= a.battle.actors.iter().any(|a| a.fired > 0);
            damage |= a.units.iter().any(|u| u.revision > 0);
            if tick == 120 {
                b = BlastRange::restore(b.config, &b.snapshot()).unwrap();
                assert!(b.finish_battle_command_capture().is_none());
                // Restoring the captured state also drops only observation.
                let mut restored = BlastRange::restore(a.config, &a.snapshot()).unwrap();
                same(&a, &restored);
                assert!(restored.finish_battle_command_capture().is_none());
            }
        }
        assert!(fired && damage, "empty combat fixture: {kind:?}");
        let trace = a.finish_battle_command_capture().unwrap();
        assert!(trace.complete());
        assert_eq!(trace.start_tick, 0);
        assert_eq!(trace.end_tick, 240);
        for producer in [
            Producer::Player,
            Producer::EnemyRules,
            Producer::AlliedRules,
        ] {
            let records: Vec<_> = trace
                .records
                .iter()
                .filter(|r| r.producer == producer)
                .collect();
            assert!(!records.is_empty());
            for (sequence, record) in records.iter().enumerate() {
                assert_eq!(record.sequence, sequence as u64);
            }
        }
        assert!(trace
            .records
            .iter()
            .any(|r| r.outcome == Outcome::Unchanged));
    }
}

#[test]
fn command_capture_overflow_is_explicit_and_does_not_reject_physical_commands() {
    let (mut a, mut b) = (arena(RangeUnitKind::Tank), arena(RangeUnitKind::Tank));
    assert!(a.start_battle_command_capture(0).is_err());
    assert!(a.start_battle_command_capture(usize::MAX).is_err());
    a.start_battle_command_capture(2).unwrap();
    assert!(a.start_battle_command_capture(3).is_err());
    for request in [
        order(0, Order::Hold, None),
        order(1, Order::Hold, None),
        order(0, Order::Move, Some([210, 336])),
    ] {
        assert_eq!(
            a.execute_battle_command(request),
            b.execute_battle_command(request)
        );
        same(&a, &b);
    }
    a.battle.phase = Phase::Victory;
    b.battle.phase = Phase::Victory;
    let rejected = vec![(0, [200, 336]); 100_000];
    assert_eq!(a.battle_group_move(&rejected, [210, 336]), 0);
    assert_eq!(b.battle_group_move(&rejected, [210, 336]), 0);
    same(&a, &b);
    let trace = a.finish_battle_command_capture().unwrap();
    assert!(!trace.complete());
    assert_eq!(trace.records.len(), 2);
    assert_eq!(trace.omitted, 100_001);
    assert!(a.finish_battle_command_capture().is_none());
    a.start_battle_command_capture(1).unwrap();
    assert_eq!(
        a.execute_battle_command(order(0, Order::Hold, None)),
        Outcome::Rejected
    );
    assert_eq!(
        a.finish_battle_command_capture().unwrap().records[0].sequence,
        0
    );
}
