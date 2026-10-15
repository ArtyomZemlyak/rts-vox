use super::*;
use crate::blast::{
    battle::{
        command_tests::{arena, order, same},
        Order,
    },
    RangeUnitKind,
};

#[cfg(test)]
#[test]
fn lifecycle_setup_queue_spill_ammunition_and_combat_restore_match_direct_execution() {
    use crate::blast::battle::{commands::BattleLifecycle as L, Weapon};
    let world = arena(RangeUnitKind::Tank);
    let mut direct = BlastRange::restore(world.config, &world.snapshot()).unwrap();
    let mut r = CommandRuntime::new(world, 7, bounds().runtime).unwrap();
    let schedule = [
        (0, Command::BattleLifecycle(L::FreeControl)),
        (0, order(0, Order::Move, Some([210, 336]))),
        (0, Command::BattleLifecycle(L::Autonomy { enabled: true })),
        (0, Command::BattleLifecycle(L::StartBattle)),
        (0, Command::BattleLifecycle(L::Autonomy { enabled: false })),
        (
            0,
            Command::SetBattleWeapon {
                actor: 0,
                weapon: Some(Weapon::Blast),
            },
        ),
        (
            30,
            Command::SetBattleWeapon {
                actor: 0,
                weapon: Some(Weapon::Energy),
            },
        ),
        (
            60,
            Command::SetBattleWeapon {
                actor: 0,
                weapon: Some(Weapon::Laser),
            },
        ),
        (
            90,
            Command::SetBattleWeapon {
                actor: 0,
                weapon: None,
            },
        ),
    ];
    let receipts: Vec<_> = schedule
        .into_iter()
        .enumerate()
        .map(|(seq, (tick, command))| r.submit(request(seq as u64, tick, command)).unwrap())
        .collect();
    assert_eq!(
        receipts
            .iter()
            .take(6)
            .map(|r| r.assigned_tick)
            .collect::<Vec<_>>(),
        vec![0, 0, 1, 1, 2, 2]
    );
    let mut replica = restored(&r);
    let mut outcomes = Vec::new();
    for tick in 0..240 {
        for receipt in receipts
            .iter()
            .filter(|receipt| receipt.assigned_tick == tick)
        {
            outcomes.push((receipt.id, direct.execute_command(receipt.envelope.command)));
        }
        direct.step();
        r.step().unwrap();
        replica.step().unwrap();
        same(&direct, r.world());
        same(r.world(), replica.world());
        assert_eq!(r.state_hash(), replica.state_hash());
        if [0, 1, 29, 60, 100].contains(&tick) {
            replica = restored(&r);
        }
    }
    assert!(r.world().battle.actors.iter().any(|a| a.fired > 0));
    assert!(r.world().units.iter().any(|u| u.revision > 0));
    for (id, expected) in outcomes {
        assert_eq!(r.receipt(id).unwrap().outcome, Some(expected));
        assert_eq!(r.receipt(id), replica.receipt(id));
    }
    assert_eq!(
        r.receipt(receipts[4].id).unwrap().outcome,
        Some(Outcome::Rejected)
    );
    assert_eq!(
        r.receipt(receipts[7].id).unwrap().outcome,
        Some(Outcome::Rejected)
    );
}

#[cfg(test)]
#[test]
fn lifecycle_wire_is_strict_and_pending_values_participate_in_hash() {
    use crate::blast::battle::{commands::BattleLifecycle as L, Weapon};
    let mut hashes = std::collections::BTreeSet::new();
    for command in [
        Command::BattleLifecycle(L::StartBattle),
        Command::BattleLifecycle(L::FreeControl),
        Command::BattleLifecycle(L::Autonomy { enabled: false }),
        Command::BattleLifecycle(L::Autonomy { enabled: true }),
        Command::SetBattleWeapon {
            actor: 0,
            weapon: None,
        },
        Command::SetBattleWeapon {
            actor: 0,
            weapon: Some(Weapon::Blast),
        },
        Command::SetBattleWeapon {
            actor: 1,
            weapon: Some(Weapon::Blast),
        },
        Command::SetBattleWeapon {
            actor: 0,
            weapon: Some(Weapon::Canister),
        },
        Command::SetBattleWeapon {
            actor: 0,
            weapon: Some(Weapon::Siege),
        },
    ] {
        let mut r = CommandRuntime::new(arena(RangeUnitKind::Tank), 7, bounds().runtime).unwrap();
        r.submit(request(0, 0, command)).unwrap();
        assert!(hashes.insert(r.state_hash()));
        restored(&r);
        let bytes = r.checkpoint().to_bytes();
        // World length, metadata, watermarks, count, receipt header/outcome.
        let tag = 20 + r.checkpoint().world.len() + 64 + 49;
        assert_eq!(
            bytes[tag],
            if matches!(command, Command::BattleLifecycle(_)) {
                6
            } else {
                7
            }
        );
        let mut mutations = vec![(tag, 255), (tag + 9, 255)];
        if let Command::BattleLifecycle(lifecycle) = command {
            mutations.push((tag + 1, 1)); // Reserved world target must be zero.
            if matches!(lifecycle, L::Autonomy { .. }) {
                mutations.push((tag + 10, 2));
            }
        }
        for (at, value) in mutations {
            let mut bad = bytes.clone();
            bad[at] = value;
            resign(&mut bad);
            assert!(
                Checkpoint::from_bytes(&bad, bounds()).is_none(),
                "{command:?} at {at}"
            );
        }
        for end in tag..=tag + 9 {
            let mut bad = bytes[..end].to_vec();
            bad.extend_from_slice(&[0; 8]);
            resign(&mut bad);
            assert!(Checkpoint::from_bytes(&bad, bounds()).is_none());
        }
    }
}

#[cfg(test)]
#[test]
fn weapon_configuration_pending_receipts_hash_and_live_battle_portable_restore() {
    let mut r = owner(RangeUnitKind::Tank);
    let initial = r.world().config;
    let mut changed = initial;
    changed.energy += 1;
    let mut invalid = changed;
    invalid.wave_work_per_tick = 0;
    let schedule = [(0, changed), (25, changed), (50, invalid), (100, initial)];
    let receipts: Vec<_> = schedule
        .iter()
        .enumerate()
        .map(|(seq, &(tick, config))| {
            r.submit(request(
                seq as u64,
                tick,
                Command::ConfigureWeapon { config },
            ))
            .unwrap()
        })
        .collect();
    let mut altered = r.checkpoint();
    let Command::ConfigureWeapon { config } = &mut altered.inbox.entries[0].envelope.command else {
        panic!()
    };
    config.gravity += 1;
    assert_ne!(
        r.state_hash(),
        CommandRuntime::restore(initial, altered)
            .unwrap()
            .state_hash()
    );
    let mut replica = restored(&r);
    let mut fragments = false;
    for tick in 0..240 {
        r.step().unwrap();
        replica.step().unwrap();
        same(r.world(), replica.world());
        assert_eq!(r.state_hash(), replica.state_hash());
        fragments |= !r.world().matter.debris.fragments.is_empty();
        if [0, 24, 49, 80, 150].contains(&tick) {
            let bytes = r.checkpoint().to_bytes();
            let cp = Checkpoint::from_bytes(&bytes, bounds()).unwrap();
            assert_eq!(bytes, cp.to_bytes());
            let mut wrong = initial;
            wrong.gravity = 9000;
            wrong.edits_per_tick = 1;
            replica = CommandRuntime::restore(wrong, cp).unwrap();
            assert_eq!(r.state_hash(), replica.state_hash());
        }
    }
    assert!(fragments && r.world().battle.actors.iter().any(|a| a.fired > 0));
    for (receipt, outcome) in receipts.into_iter().zip([
        Outcome::Applied,
        Outcome::Unchanged,
        Outcome::Rejected,
        Outcome::Applied,
    ]) {
        assert_eq!(r.receipt(receipt.id), replica.receipt(receipt.id));
        assert_eq!(r.receipt(receipt.id).unwrap().outcome, Some(outcome));
    }
}

#[cfg(test)]
#[test]
fn density_commands_replay_pending_live_combat_and_reset_without_revaluing_matter() {
    use crate::blast::battle::commands::MaterialField;
    let mut r = owner(RangeUnitKind::Tank);
    let schedule = [
        (0, 3, 100_000),
        (25, 1, 121),
        (65, 3, 1),
        (100, 9, 1),
        (140, 3, 785),
        (165, 1, 120),
        (180, 9, 80),
    ];
    let receipts: Vec<_> = schedule
        .iter()
        .enumerate()
        .map(|(seq, &(tick, material, value))| {
            r.submit(request(
                seq as u64,
                tick,
                Command::TuneMaterial {
                    material,
                    field: MaterialField::Density,
                    value,
                },
            ))
            .unwrap()
        })
        .collect();
    let mut replica = restored(&r);
    let mut fragments = false;
    for tick in 0..240 {
        r.step().unwrap();
        replica.step().unwrap();
        same(r.world(), replica.world());
        assert_eq!(r.state_hash(), replica.state_hash());
        fragments |= !r.world().matter.debris.fragments.is_empty();
        assert!(r.world().matter.economy.ledger.balanced());
        if [24, 66, 120, 150, 190].contains(&tick) {
            replica = restored(&r);
        }
    }
    assert!(fragments && r.world().battle.actors.iter().any(|a| a.fired > 0));
    for receipt in receipts {
        assert_eq!(r.receipt(receipt.id), replica.receipt(receipt.id));
        assert!(r.receipt(receipt.id).unwrap().outcome.unwrap().accepted());
    }
}

#[cfg(test)]
#[test]
fn material_commands_preserve_live_combat_ledger_and_pending_portable_restore() {
    use crate::blast::battle::commands::MaterialField;
    let mut r = owner(RangeUnitKind::Tank);
    let mut direct =
        crate::blast::BlastRange::restore(r.world().config, &r.world().snapshot()).unwrap();
    let schedule = [
        (0, 3, MaterialField::Strength, 550),
        (35, 3, MaterialField::Melt, 380),
        (70, 1, MaterialField::Strength, 25),
        (140, 3, MaterialField::Strength, 500),
    ];
    let receipts: Vec<_> = schedule
        .iter()
        .enumerate()
        .map(|(seq, &(tick, material, field, value))| {
            r.submit(request(
                seq as u64,
                tick,
                Command::TuneMaterial {
                    material,
                    field,
                    value,
                },
            ))
            .unwrap()
        })
        .collect();
    let mut replica = restored(&r);
    let mut fragments = false;
    for tick in 0..240 {
        for &(when, material, field, value) in &schedule {
            if tick != when {
                continue;
            }
            let mut d = *direct.matter.economy.mats.get(material).unwrap();
            match field {
                MaterialField::Strength => d.strength = value,
                MaterialField::Melt => d.melt = value,
                MaterialField::Density => unreachable!(),
            }
            direct.matter.economy.mats.replace(d).unwrap();
        }
        direct.step();
        r.step().unwrap();
        replica.step().unwrap();
        same(&direct, r.world());
        same(r.world(), replica.world());
        assert_eq!(r.state_hash(), replica.state_hash());
        fragments |= !r.world().matter.debris.fragments.is_empty();
        if [24, 80, 120, 150].contains(&tick) {
            replica = restored(&r);
        }
    }
    assert!(fragments && r.world().battle.actors.iter().any(|a| a.fired > 0));
    for receipt in receipts {
        assert_eq!(r.receipt(receipt.id), replica.receipt(receipt.id));
        assert!(r.receipt(receipt.id).unwrap().outcome.unwrap().accepted());
    }
}

#[cfg(test)]
#[test]
fn pending_material_id_field_and_value_participate_in_runtime_hash_and_missing_ids_reject() {
    use crate::blast::battle::commands::MaterialField;
    let mut r = owner(RangeUnitKind::Tank);
    r.submit(request(
        0,
        0,
        Command::TuneMaterial {
            material: 3,
            field: MaterialField::Strength,
            value: 17,
        },
    ))
    .unwrap();
    for command in [
        Command::TuneMaterial {
            material: 2,
            field: MaterialField::Strength,
            value: 17,
        },
        Command::TuneMaterial {
            material: 3,
            field: MaterialField::Melt,
            value: 17,
        },
        Command::TuneMaterial {
            material: 3,
            field: MaterialField::Density,
            value: 17,
        },
        Command::TuneMaterial {
            material: 3,
            field: MaterialField::Strength,
            value: 18,
        },
    ] {
        let mut changed = r.checkpoint();
        changed.inbox.entries[0].envelope.command = command;
        let changed = CommandRuntime::restore(r.world().config, changed).unwrap();
        assert_ne!(r.state_hash(), changed.state_hash());
    }
    let receipt = r
        .submit(request(
            1,
            0,
            Command::TuneMaterial {
                material: u16::MAX,
                field: MaterialField::Melt,
                value: 17,
            },
        ))
        .unwrap();
    let mut replica = restored(&r);
    r.step().unwrap();
    replica.step().unwrap();
    assert_eq!(
        r.receipt(receipt.id).unwrap().outcome,
        Some(Outcome::Rejected)
    );
    assert_eq!(r.receipt(receipt.id), replica.receipt(receipt.id));
    restored(&replica);
}

#[cfg(test)]
#[test]
fn debris_commands_keep_live_combat_and_pending_portable_restore_identical() {
    use crate::blast::battle::commands::DebrisField;
    let mut r = owner(RangeUnitKind::Tank);
    let mut direct =
        crate::blast::BlastRange::restore(r.world().config, &r.world().snapshot()).unwrap();
    let schedule = [
        (0, DebrisField::DragPct, 17),
        (35, DebrisField::PushPct, 110),
        (70, DebrisField::MaxFlying, 1),
        (90, DebrisField::PileCell, 4),
        (140, DebrisField::MaxFlying, 4096),
    ];
    let receipts: Vec<_> = schedule
        .iter()
        .enumerate()
        .map(|(seq, &(tick, field, value))| {
            r.submit(request(
                seq as u64,
                tick,
                Command::TuneDebris { field, value },
            ))
            .unwrap()
        })
        .collect();
    let mut replica = restored(&r);
    let mut fragments = false;
    for tick in 0..240 {
        for &(when, field, value) in &schedule {
            if tick != when {
                continue;
            }
            match field {
                DebrisField::DragPct => direct.matter.tuning.drag_pct = value,
                DebrisField::PushPct => direct.matter.tuning.push_pct = value,
                DebrisField::MaxFlying => direct.matter.tuning.max_flying = value,
                DebrisField::PileCell => direct.matter.tuning.pile_cell = value,
                _ => unreachable!(),
            }
        }
        direct.step();
        r.step().unwrap();
        replica.step().unwrap();
        same(&direct, r.world());
        same(r.world(), replica.world());
        assert_eq!(r.state_hash(), replica.state_hash());
        fragments |= !r.world().matter.debris.fragments.is_empty();
        if [24, 80, 120, 150].contains(&tick) {
            replica = restored(&r);
        }
    }
    assert!(fragments && r.world().battle.actors.iter().any(|a| a.fired > 0));
    for receipt in receipts {
        assert_eq!(r.receipt(receipt.id), replica.receipt(receipt.id));
        assert!(r.receipt(receipt.id).unwrap().outcome.unwrap().accepted());
    }
}

#[cfg(test)]
#[test]
fn pending_debris_field_and_value_both_participate_in_runtime_hash() {
    use crate::blast::battle::commands::DebrisField;
    let mut r = owner(RangeUnitKind::Tank);
    r.submit(request(
        0,
        0,
        Command::TuneDebris {
            field: DebrisField::DragPct,
            value: 17,
        },
    ))
    .unwrap();
    for command in [
        Command::TuneDebris {
            field: DebrisField::DragPct,
            value: 18,
        },
        Command::TuneDebris {
            field: DebrisField::FrictionPct,
            value: 17,
        },
    ] {
        let mut changed = r.checkpoint();
        changed.inbox.entries[0].envelope.command = command;
        let changed = CommandRuntime::restore(r.world().config, changed).unwrap();
        assert_ne!(r.state_hash(), changed.state_hash());
    }
}

#[cfg(test)]
#[test]
fn motion_commands_restore_applied_values_and_pending_edits_during_real_combat() {
    use crate::blast::battle::commands::MotionField;
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
        let mut r = owner(kind);
        let mut direct =
            crate::blast::BlastRange::restore(r.world().config, &r.world().snapshot()).unwrap();
        let schedule = [
            (0, MotionField::Speed, 19),
            (35, MotionField::Stiffness, 23),
            (85, MotionField::Thrust, 34),
            (120, MotionField::Step, 4),
        ];
        let receipts: Vec<_> = schedule
            .iter()
            .enumerate()
            .map(|(seq, &(tick, field, value))| {
                r.submit(request(
                    seq as u64,
                    tick,
                    Command::TuneMotion {
                        kind,
                        include_gun: true,
                        field,
                        value,
                    },
                ))
                .unwrap()
            })
            .collect();
        let mut replica = restored(&r); // all commands pending
        let mut fired = false;
        let mut damaged = false;
        for tick in 0..240 {
            for &(when, field, value) in &schedule {
                if when == tick {
                    for u in direct
                        .units
                        .iter_mut()
                        .chain(std::iter::once(&mut direct.gun))
                    {
                        match field {
                            MotionField::Speed => u.motion.speed = value,
                            MotionField::Stiffness => u.motion.chassis.stiffness = value,
                            MotionField::Thrust => u.motion.chassis.thrust = value,
                            MotionField::Step => u.motion.step = value,
                            _ => unreachable!(),
                        }
                    }
                }
            }
            direct.step();
            r.step().unwrap();
            replica.step().unwrap();
            same(&direct, r.world());
            assert_eq!(r.state_hash(), replica.state_hash());
            same(r.world(), replica.world());
            fired |= r.world().battle.actors.iter().any(|a| a.fired > 0);
            damaged |= r.world().units.iter().any(|u| u.revision > 0);
            if [24, 70, 150].contains(&tick) {
                replica = restored(&r);
            }
        }
        assert!(
            fired && damaged,
            "combat must exercise projectiles and physical damage: {kind:?}"
        );
        for receipt in receipts {
            assert_eq!(r.receipt(receipt.id), replica.receipt(receipt.id));
            assert!(r.receipt(receipt.id).unwrap().outcome.unwrap().accepted());
        }
    }
}

#[cfg(test)]
#[test]
fn pending_motion_values_participate_in_runtime_hash() {
    use crate::blast::battle::commands::MotionField;
    let mut r = owner(RangeUnitKind::Tank);
    r.submit(request(
        0,
        0,
        Command::TuneMotion {
            kind: RangeUnitKind::Tank,
            include_gun: true,
            field: MotionField::Speed,
            value: 19,
        },
    ))
    .unwrap();
    let before = r.state_hash();
    let mut changed = r.checkpoint();
    if let Command::TuneMotion { value, .. } = &mut changed.inbox.entries[0].envelope.command {
        *value += 1;
    }
    let changed = CommandRuntime::restore(r.world().config, changed).unwrap();
    assert_ne!(before, changed.state_hash());
}

fn owner(kind: RangeUnitKind) -> CommandRuntime {
    let mut world = arena(kind);
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
fn request(sequence: u64, tick: u32, command: Command) -> Envelope {
    Envelope {
        epoch: 7,
        producer: 0,
        sequence,
        requested_tick: tick,
        command,
    }
}
fn plan(sequence: u64, tick: u32, order: AiOrder) -> PlanEnvelope {
    PlanEnvelope {
        epoch: 7,
        producer: 1,
        sequence,
        decision_tick: tick,
        decisions: vec![Decision { actor: 1, order }],
    }
}
fn bounds() -> DecodeLimits {
    DecodeLimits {
        world_bytes: 32 * 1024 * 1024,
        runtime: Limits {
            retained: 16,
            producers: 2,
            per_tick: 2,
        },
        plan_decisions: 2,
    }
}
fn restored(r: &CommandRuntime) -> CommandRuntime {
    let bytes = r.checkpoint().to_bytes();
    let checkpoint = Checkpoint::from_bytes(&bytes, bounds()).unwrap();
    assert_eq!(bytes, checkpoint.to_bytes());
    let replica = CommandRuntime::restore(r.world().config, checkpoint).unwrap();
    assert_eq!(r.state_hash(), replica.state_hash());
    same(r.world(), replica.world());
    replica
}

#[test]
fn portable_wire_v1_has_a_fixed_little_endian_conformance_vector() {
    let checkpoint = Checkpoint {
        world: vec![0xaa, 0xbb],
        inbox: Inbox {
            epoch: 7,
            limits: Limits {
                retained: 2,
                producers: 1,
                per_tick: 3,
            },
            next: vec![0],
            next_id: 0,
            entries: vec![],
            plans: vec![],
        },
    };
    // Protocol fixture only: the two world bytes are deliberately opaque here.
    let golden = "525453434d443031010000000200000000000000aabb07000000000000000200000000000000010000000000000003000000000000000000000000000000000000000000000000000000000000000000000000000000909b421b5f6dd2ad";
    let expected: Vec<_> = (0..golden.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&golden[at..at + 2], 16).unwrap())
        .collect();
    let bytes = checkpoint.to_bytes();
    assert_eq!(bytes, expected);
    assert_eq!(bytes.len(), bytes.capacity());
    let limits = DecodeLimits {
        world_bytes: 2,
        runtime: checkpoint.inbox.limits,
        plan_decisions: 0,
    };
    assert_eq!(
        Checkpoint::from_bytes(&expected, limits)
            .unwrap()
            .to_bytes(),
        expected
    );
}

#[test]
fn portable_results_preserve_queued_formation_and_failed_path_search() {
    let mut r = owner(RangeUnitKind::Tank);
    let queued = r
        .submit(request(
            0,
            0,
            Command::FormationMove {
                actor: 0,
                destination: [-1, 336],
                fallback: [210, 336],
            },
        ))
        .unwrap()
        .id;
    let failed = r
        .submit(request(1, 0, order(0, Order::Move, Some([-1, 336]))))
        .unwrap()
        .id;
    r.step().unwrap();
    assert_eq!(r.receipt(queued).unwrap().outcome, Some(Outcome::Queued));
    assert_eq!(
        r.receipt(failed).unwrap().outcome,
        Some(Outcome::PathFailed)
    );
    let replica = restored(&r);
    assert_eq!(r.receipt(queued), replica.receipt(queued));
    assert_eq!(r.receipt(failed), replica.receipt(failed));
}

#[test]
fn portable_mixed_pending_ingress_keeps_scheduler_results_ack_and_watermarks() {
    let mut r = owner(RangeUnitKind::Tank);
    let old = r
        .submit(request(0, 0, order(0, Order::Hold, None)))
        .unwrap();
    r.step().unwrap();
    r.acknowledge(old.id).unwrap();
    let due = r.world().enemy_ai_observation().unwrap().decision_tick;
    let a = r
        .submit(request(1, due, order(0, Order::Hold, None)))
        .unwrap();
    let b = r
        .submit(request(2, due, order(0, Order::Auto, None)))
        .unwrap();
    let c = r
        .submit(request(3, due, order(usize::MAX, Order::Hold, None)))
        .unwrap();
    assert_eq!(c.assigned_tick, due + 1);
    let id = r.submit_plan(plan(0, due, AiOrder::Attack(0))).unwrap();
    let mut replica = restored(&r);
    while r.world().tick <= due + 1 {
        r.step().unwrap();
        replica.step().unwrap();
        assert_eq!(r.state_hash(), replica.state_hash());
        same(r.world(), replica.world());
    }
    for receipt in [a, b, c] {
        assert_eq!(r.acknowledge(receipt.id), replica.acknowledge(receipt.id));
    }
    assert_eq!(r.acknowledge_plan(id), replica.acknowledge_plan(id));
    let mut replica = restored(&replica);
    assert_eq!(
        r.submit(request(0, due + 2, order(0, Order::Hold, None))),
        Err(AdmissionError::Sequence { expected: 4 })
    );
    let input = request(4, due + 2, order(0, Order::Hold, None));
    assert_eq!(r.submit(input), replica.submit(input));
    assert_eq!(r.state_hash(), replica.state_hash());
}

#[test]
fn portable_completed_plan_preserves_actual_selected_rejection_and_fallback() {
    let mut r = owner(RangeUnitKind::TurretT1);
    let id = r
        .submit_plan(plan(0, 0, AiOrder::Move([450, 336])))
        .unwrap();
    r.step().unwrap();
    let PlanState::Executed { attempts, .. } = &r.plan_receipt(id).unwrap().state else {
        panic!()
    };
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, Outcome::Rejected);
    assert!(attempts[1].fallback && attempts[1].outcome.accepted());
    let mut replica = restored(&r);
    assert_eq!(r.acknowledge_plan(id), replica.acknowledge_plan(id));
    assert_eq!(r.state_hash(), replica.state_hash());
}

#[test]
fn portable_cancelled_plan_keeps_a_loadable_terminal_world() {
    let mut r = owner(RangeUnitKind::Tank);
    r.step().unwrap();
    let tick = r.world().enemy_ai_observation().unwrap().decision_tick;
    let id = r.submit_plan(plan(0, tick, AiOrder::Hold)).unwrap();
    r.world.units[1].status.alive = false;
    r.step().unwrap();
    assert!(matches!(
        r.plan_receipt(id).unwrap().state,
        PlanState::Cancelled { .. }
    ));
    let replica = restored(&r);
    assert_eq!(r.plan_receipt(id), replica.plan_receipt(id));
}

#[test]
fn portable_commands_keep_invalid_gameplay_inputs_until_normal_execution() {
    let mut r = owner(RangeUnitKind::Tank);
    let inputs = [
        order(usize::MAX, Order::Auto, None),
        order(usize::MAX, Order::Hold, Some([i32::MIN, i32::MAX])),
        order(usize::MAX, Order::Move, Some([-1, 336])),
        order(usize::MAX, Order::Attack(usize::MAX), None),
        order(usize::MAX, Order::Ground([i32::MIN, 7, i32::MAX]), None),
        order(usize::MAX, Order::Dig([i32::MIN, 7, i32::MAX]), None),
        Command::FormationMove {
            actor: usize::MAX,
            destination: [-1, 7],
            fallback: [123, -9],
        },
    ];
    let ids: Vec<_> = inputs
        .iter()
        .enumerate()
        .map(|(seq, &command)| r.submit(request(seq as u64, 1, command)).unwrap().id)
        .collect();
    let mut replica = restored(&r);
    for _ in 0..5 {
        r.step().unwrap();
        replica.step().unwrap();
    }
    for id in ids {
        assert_eq!(r.receipt(id).unwrap().outcome, Some(Outcome::Rejected));
        assert_eq!(r.receipt(id), replica.receipt(id));
    }
    restored(&replica);
}

fn resign(bytes: &mut [u8]) {
    let end = bytes.len() - 8;
    let hash = bytes[..end].iter().fold(0xcbf29ce484222325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    bytes[end..].copy_from_slice(&hash.to_le_bytes());
}

#[test]
fn portable_wire_rejects_damage_truncation_versions_tags_and_trailing_data() {
    let mut r = owner(RangeUnitKind::Tank);
    r.submit(request(0, 0, order(0, Order::Hold, None)))
        .unwrap();
    let bytes = r.checkpoint().to_bytes();
    for end in [0, 1, 8, 12, 19, 20, bytes.len() - 9, bytes.len() - 1] {
        assert!(Checkpoint::from_bytes(&bytes[..end], bounds()).is_none());
    }
    for at in [0, 8, 12, 20, bytes.len() - 1] {
        let mut bad = bytes.clone();
        bad[at] ^= 1;
        assert!(Checkpoint::from_bytes(&bad, bounds()).is_none());
    }
    for at in [0, 8] {
        let mut bad = bytes.clone();
        bad[at] ^= 2;
        resign(&mut bad);
        assert!(Checkpoint::from_bytes(&bad, bounds()).is_none());
    }
    let world_end = 20 + r.checkpoint().world.len();
    // metadata 40 + watermarks 16 + count 8 + receipt header 48
    for at in [
        world_end + 64 + 48,
        world_end + 64 + 49,
        world_end + 64 + 58,
        world_end + 64 + 59,
    ] {
        let mut bad = bytes.clone();
        bad[at] = 255;
        resign(&mut bad);
        assert!(
            Checkpoint::from_bytes(&bad, bounds()).is_none(),
            "tag offset {at}"
        );
    }
    let mut bad = bytes[..bytes.len() - 8].to_vec();
    bad.push(0);
    bad.extend_from_slice(&[0; 8]);
    resign(&mut bad);
    assert!(Checkpoint::from_bytes(&bad, bounds()).is_none());
}

#[test]
fn portable_wire_applies_caller_bounds_before_payload_allocation() {
    let mut r = owner(RangeUnitKind::Tank);
    r.submit_plan(plan(0, 0, AiOrder::Hold)).unwrap();
    let bytes = r.checkpoint().to_bytes();
    let mut limits = bounds();
    limits.world_bytes = 1;
    assert!(Checkpoint::from_bytes(&bytes, limits).is_none());
    limits = bounds();
    limits.runtime.retained = 15;
    assert!(Checkpoint::from_bytes(&bytes, limits).is_none());
    limits = bounds();
    limits.runtime.producers = 1;
    assert!(Checkpoint::from_bytes(&bytes, limits).is_none());
    limits = bounds();
    limits.runtime.per_tick = 1;
    assert!(Checkpoint::from_bytes(&bytes, limits).is_none());
    limits = bounds();
    limits.plan_decisions = 0;
    assert!(Checkpoint::from_bytes(&bytes, limits).is_none());
    let world_end = 20 + r.checkpoint().world.len();
    // Length/limits/counts, including the first plan decision count.
    for at in [
        12,
        world_end + 8,
        world_end + 16,
        world_end + 24,
        world_end + 56,
        world_end + 64,
        world_end + 72 + 48,
    ] {
        let mut bad = bytes.clone();
        bad[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        resign(&mut bad);
        assert!(
            Checkpoint::from_bytes(&bad, bounds()).is_none(),
            "length offset {at}"
        );
    }
}

#[test]
fn portable_restore_rejects_forged_ids_order_watermarks_capacity_and_timeline() {
    let mut r = owner(RangeUnitKind::Tank);
    r.submit(request(0, 0, order(0, Order::Hold, None)))
        .unwrap();
    r.submit(request(1, 0, order(0, Order::Auto, None)))
        .unwrap();
    let base = r.checkpoint();
    for case in 0..10 {
        let mut bad = base.clone();
        match case {
            0 => bad.inbox.entries[0].id.epoch = 99,
            1 => bad.inbox.entries[0].envelope.epoch = 99,
            2 => bad.inbox.entries[0].envelope.producer = 2,
            3 => bad.inbox.entries[0].envelope.sequence = 2,
            4 => bad.inbox.entries[1].id = bad.inbox.entries[0].id,
            5 => bad.inbox.entries.swap(0, 1),
            6 => bad.inbox.next[0] = 3,
            7 => bad.inbox.limits.per_tick = 1,
            8 => bad.inbox.entries[0].outcome = Some(Outcome::Applied),
            9 => bad.inbox.entries[0].envelope.requested_tick = 1,
            _ => unreachable!(),
        }
        let result = Checkpoint::from_bytes(&bad.to_bytes(), bounds())
            .and_then(|cp| CommandRuntime::restore(r.world().config, cp));
        assert!(result.is_none(), "forged case {case}");
    }
}

#[test]
fn portable_restore_rejects_pending_mismatch_and_impossible_attempts() {
    let mut r = owner(RangeUnitKind::TurretT1);
    r.submit_plan(plan(0, 0, AiOrder::Move([450, 336])))
        .unwrap();
    let mut bad = r.checkpoint();
    bad.inbox.plans[0].envelope.decisions[0].order = AiOrder::Hold;
    assert!(CommandRuntime::restore(
        r.world().config,
        Checkpoint::from_bytes(&bad.to_bytes(), bounds()).unwrap()
    )
    .is_none());
    r.step().unwrap();
    let base = r.checkpoint();
    for case in 0..6 {
        let mut bad = base.clone();
        let PlanState::Executed { tick, attempts } = &mut bad.inbox.plans[0].state else {
            panic!()
        };
        match case {
            0 => *tick += 1,
            1 => {
                attempts.pop();
            }
            2 => attempts[0].fallback = true,
            3 => attempts[1].fallback = false,
            4 => attempts[0].outcome = Outcome::Applied,
            5 => attempts[1].decision.actor = 0,
            _ => unreachable!(),
        }
        assert!(
            CommandRuntime::restore(
                r.world().config,
                Checkpoint::from_bytes(&bad.to_bytes(), bounds()).unwrap()
            )
            .is_none(),
            "attempt case {case}"
        );
    }
}
