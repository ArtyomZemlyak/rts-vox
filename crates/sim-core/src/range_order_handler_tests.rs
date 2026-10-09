use super::*;

#[path = "range_order_handler_legacy.rs"]
mod legacy;

fn arena(kind: RangeUnitKind) -> BlastRange {
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
    BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap()
}

fn same(a: &BlastRange, b: &BlastRange) {
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(a.snapshot(), b.snapshot());
    assert!(a.matter.economy.ledger.balanced() && b.matter.economy.ledger.balanced());
}

fn controller(range: &mut BlastRange, decision: ai::Decision, side: bool, old: bool) -> bool {
    // The real tick also detaches the battle owner before running controllers.
    let empty = Battle::new(&[], range.config.unit_ablation_resistance);
    let mut battle = std::mem::replace(&mut range.battle, empty);
    let accepted = if old {
        range.legacy_ai(&mut battle, decision, side)
    } else {
        range.apply_ai_decision(&mut battle, decision, side)
    };
    range.battle = battle;
    accepted
}

#[test]
fn shared_order_handler_matches_legacy_player_controller_and_formation() {
    for kind in RangeUnitKind::ALL {
        let (mut a, mut b) = (arena(kind), arena(kind));
        for phase in [Phase::Setup, Phase::Fighting, Phase::Victory] {
            a.battle.phase = phase;
            b.battle.phase = phase;
            for (id, order, destination) in [
                (usize::MAX, Order::Hold, None),
                (1, Order::Hold, None),
                (0, Order::Attack(0), None),
                (0, Order::Attack(usize::MAX), None),
                (0, Order::Ground([-1, 16, 336]), None),
                (0, Order::Move, None),
                (0, Order::Move, Some([-1, 336])),
                (0, Order::Move, Some([230, 336])),
                (0, Order::Move, Some([230, 336])),
                (0, Order::Attack(1), None),
                (0, Order::Ground([450, 16, 336]), None),
                (0, Order::Hold, None),
                (0, Order::Hold, None),
                (0, Order::Auto, None),
            ] {
                assert_eq!(
                    a.battle_order(id, order, destination),
                    b.legacy_order(id, order, destination)
                );
                same(&a, &b);
            }
            let destinations = [
                (0, [-1, 336]),
                (1, [400, 336]),
                (usize::MAX, [100, 100]),
                (0, [240, 336]),
            ];
            assert_eq!(
                a.battle_group_move(&destinations, [210, 336]),
                b.legacy_group(&destinations, [210, 336])
            );
            same(&a, &b);
            if phase != Phase::Fighting {
                continue;
            }
            for side in [false, true] {
                let id = usize::from(!side);
                for order in [
                    ai::AiOrder::Attack(id),
                    ai::AiOrder::Attack(1 - id),
                    ai::AiOrder::Move([-1, 336]),
                    ai::AiOrder::Move([260, 336]),
                    ai::AiOrder::Move([260, 336]),
                    ai::AiOrder::Hold,
                    ai::AiOrder::Hold,
                    ai::AiOrder::Auto,
                    ai::AiOrder::Auto,
                ] {
                    let decision = ai::Decision { actor: id, order };
                    assert_eq!(
                        controller(&mut a, decision, side, false),
                        controller(&mut b, decision, side, true)
                    );
                    same(&a, &b);
                }
                let invalid = ai::Decision {
                    actor: usize::MAX,
                    order: ai::AiOrder::Auto,
                };
                assert!(!controller(&mut a, invalid, side, false));
                assert!(!controller(&mut b, invalid, side, true));
                same(&a, &b);
            }
        }
        a.battle.phase = Phase::Fighting;
        b.battle.phase = Phase::Fighting;
        a.units[1].status.alive = false;
        b.units[1].status.alive = false;
        assert!(!a.battle_order(0, Order::Attack(1), None));
        assert!(!b.legacy_order(0, Order::Attack(1), None));
        same(&a, &b);
    }
}

#[test]
fn shared_order_handler_keeps_arrival_dedup_and_spider_pulse_cancellation() {
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Spider] {
        let (mut a, mut b) = (arena(kind), arena(kind));
        for range in [&mut a, &mut b] {
            range.tick = 11;
            range.battle.phase = Phase::Fighting;
            let actor = &mut range.battle.actors[1];
            actor.order = Order::Move;
            actor.target = Some(0);
            actor.burst = 3;
            actor.charge = Some(7);
            actor.next = 19;
            actor.pursuit = 5;
            range.units[1].motion.destination = None;
            range.units[1].motion.route = vec![[460, 336], [470, 336]];
        }
        let move_order = ai::Decision {
            actor: 1,
            order: ai::AiOrder::Move([450, 336]),
        };
        assert!(controller(&mut a, move_order, false, false));
        assert!(controller(&mut b, move_order, false, true));
        same(&a, &b);
        assert_eq!(
            a.battle.actors[1].burst, 3,
            "arrived controller order must not restart a pulse"
        );
        assert_eq!(
            a.units[1].motion.route.len(),
            2,
            "maintained order cleared a route"
        );
        let hold = ai::Decision {
            actor: 1,
            order: ai::AiOrder::Hold,
        };
        assert!(controller(&mut a, hold, false, false));
        assert!(controller(&mut b, hold, false, true));
        same(&a, &b);
        assert_eq!(a.battle.actors[1].burst, 0);
        if kind == RangeUnitKind::Spider {
            assert!(a.battle.actors[1].next > 19);
            for range in [&mut a, &mut b] {
                for actor in &mut range.battle.actors {
                    actor.burst = 3;
                    actor.charge = Some(7);
                }
            }
            for order in [Order::Attack(1), Order::Auto, Order::Ground([450, 16, 336])] {
                assert_eq!(
                    a.battle_order(0, order, None),
                    b.legacy_order(0, order, None)
                );
                same(&a, &b);
                assert_eq!(a.battle.actors[0].burst, 3);
            }
            assert_eq!(
                a.battle_group_move(&[(0, [210, 336])], [210, 336]),
                b.legacy_group(&[(0, [210, 336])], [210, 336])
            );
            same(&a, &b);
            assert_eq!(a.battle.actors[0].burst, 3);
            for order in [
                ai::AiOrder::Attack(0),
                ai::AiOrder::Auto,
                ai::AiOrder::Move([400, 336]),
            ] {
                let decision = ai::Decision { actor: 1, order };
                assert_eq!(
                    controller(&mut a, decision, false, false),
                    controller(&mut b, decision, false, true)
                );
                same(&a, &b);
                assert_eq!(a.battle.actors[1].burst, 3);
            }
            assert!(a.battle_order(0, Order::Hold, None));
            assert!(b.legacy_order(0, Order::Hold, None));
            same(&a, &b);
            assert_eq!(a.battle.actors[0].burst, 0);
        }
    }
}

#[test]
fn shared_order_handler_preserves_real_combat_movement_and_midfight_restore() {
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Spider] {
        let (mut a, mut b) = (arena(kind), arena(kind));
        a.start_battle().unwrap();
        b.start_battle().unwrap();
        let start = a.units[0].motion.pos;
        let mut shots = 0;
        let mut damage = false;
        for tick in 0..400 {
            if tick == 0 || tick == 80 || tick == 220 {
                assert_eq!(
                    a.battle_order(0, Order::Attack(1), None),
                    b.legacy_order(0, Order::Attack(1), None)
                );
                let decision = ai::Decision {
                    actor: 1,
                    order: ai::AiOrder::Attack(0),
                };
                assert_eq!(
                    controller(&mut a, decision, false, false),
                    controller(&mut b, decision, false, true)
                );
            }
            if tick == 40 {
                assert!(a.battle_order(0, Order::Move, Some([210, 336])));
                assert!(b.legacy_order(0, Order::Move, Some([210, 336])));
            }
            if tick == 180 {
                assert_eq!(
                    a.battle_order(0, Order::Hold, None),
                    b.legacy_order(0, Order::Hold, None)
                );
                let decision = ai::Decision {
                    actor: 1,
                    order: ai::AiOrder::Hold,
                };
                assert_eq!(
                    controller(&mut a, decision, false, false),
                    controller(&mut b, decision, false, true)
                );
            }
            // Suppress the scheduled policy without constructing an invalid
            // scheduler state that the snapshot loader must reject.
            for range in [&mut a, &mut b] {
                range.battle.ai.next_tick = range.tick + range.battle.ai.tuning.decision_ticks;
            }
            a.step();
            b.step();
            same(&a, &b);
            shots = shots.max(a.battle.actors.iter().map(|a| a.fired).sum::<u32>());
            damage |= a.units.iter().any(|u| u.revision > 0);
            if tick == 120 {
                b = BlastRange::restore(b.config, &b.snapshot()).unwrap();
                same(&a, &b);
            }
        }
        assert!(
            shots > 0 && damage,
            "fixture never exercised real combat: {kind:?}"
        );
        assert_ne!(
            a.units[0].motion.pos, start,
            "manual route never moved: {kind:?}"
        );
    }
}
