use super::super::GunCommand as G;
use super::*;
use crate::blast::{
    battle::command_tests::{arena, same},
    RangeUnitKind,
};

#[cfg(test)]
fn range() -> BlastRange {
    let mut layout = arena(RangeUnitKind::Tank).layout.unwrap();
    layout.battle = false;
    layout.units.remove(0);
    layout.target = [450, 24, 336];
    let mut config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    config.charge_ticks = 1;
    BlastRange::with_layout(config, layout).unwrap()
}

#[test]
fn gun_command_local_charge_detaches_an_authored_tower_with_sufficient_power() {
    let definition = modding::laboratory::Definition::stock();
    let element = modding::laboratory::catalog::stock()
        .into_iter()
        .find(|e| e.id == "collapse-tower")
        .unwrap();
    let item = modding::laboratory::catalog::Placed {
        element,
        at: [384, 16, 336],
        turns: 0,
    };
    let mut layout = modding::laboratory::catalog::compose(&definition.layout, &[item]).unwrap();
    layout.gun_point = true;
    let base = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut detached = false;
    let default_power = modding::laboratory::events::stock()
        .into_iter()
        .find(|e| e.icon == "hammer")
        .unwrap()
        .power_milli;
    for power in [250, 500, 1000, 2000] {
        let mut sim = BlastRange::with_layout(base.at_power(power), layout.clone()).unwrap();
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        sim.execute_command(Command::Gun(G::Control));
        assert!(sim
            .execute_command(Command::Gun(G::Detonate { at: [384, 28, 336] }))
            .accepted());
        let mut fallen = false;
        for _ in 0..140 {
            sim.step();
            fallen |= sim.bodies.list.iter().any(|b| b.fallen > 0);
        }
        println!(
            "charge power={power} removed={} collapsed={} fallen={fallen} pillar={:?} roof={:?}",
            sim.removed,
            sim.collapsed_cells,
            crate::blast::material(&sim.world, [384, 28, 336]),
            crate::blast::material(&sim.world, [384, 48, 336])
        );
        detached |= fallen || sim.collapsed_cells > 0;
        if power == default_power {
            assert!(
                fallen,
                "the default support charge must topple its sample tower"
            );
        }
        assert!(sim.matter.economy.ledger.balanced());
    }
    assert!(
        detached,
        "a sufficient local charge must detach the unsupported tower"
    );
    let example = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "collapse")
        .unwrap();
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let at = match example.plan.inputs[0] {
        modding::laboratory::experiments::Input::Detonate { at } => at,
        _ => panic!("local collapse trigger"),
    };
    let mut sim = BlastRange::with_layout(config, example.layout).unwrap();
    sim.set_physics_workers(1);
    sim.set_navigation_workers(1);
    sim.execute_command(Command::Gun(G::Control));
    assert!(sim
        .execute_command(Command::Gun(G::Detonate { at }))
        .accepted());
    let mut fallen = false;
    for _ in 0..200 {
        sim.step();
        fallen |= sim.bodies.list.iter().any(|b| b.fallen > 0);
    }
    println!(
        "ready collapse removed={} collapsed={} fallen={fallen}",
        sim.removed, sim.collapsed_cells
    );
    assert!(
        fallen || sim.collapsed_cells > 0,
        "ready collapse scenario must actually detach its roof"
    );
}

#[test]
fn gun_command_local_charge_uses_shared_destruction_and_replays_without_a_shell() {
    let old = range();
    let mut layout = old.layout.clone().unwrap();
    layout.gun_point = true;
    layout.units.clear();
    layout.unit_yaws.clear();
    layout.blocks.clear();
    layout.blocks.push(modding::range::RangeBlock {
        min: [420, 16, 320],
        max: [428, 40, 328],
        material: 5,
    });
    let mut sim = BlastRange::with_layout(old.config, layout).unwrap();
    sim.set_physics_workers(1);
    sim.set_navigation_workers(1);
    sim.execute_command(Command::Gun(G::Control));
    let before = sim.snapshot();
    assert_eq!(
        sim.execute_command(Command::Gun(G::Detonate { at: [-1, 24, 324] })),
        Outcome::Rejected
    );
    assert_eq!(sim.snapshot(), before);
    let mut replay = BlastRange::restore(sim.config, &before).unwrap();
    replay.set_physics_workers(1);
    replay.set_navigation_workers(1);
    for r in [&mut sim, &mut replay] {
        assert_eq!(
            r.execute_command(Command::Gun(G::Detonate { at: [424, 24, 324] })),
            Outcome::Applied
        );
        assert_eq!(r.impact, Some([424, 24, 324]));
        assert!(r.shell.is_none());
        assert_eq!(
            r.execute_command(Command::Gun(G::Detonate { at: [424, 24, 324] })),
            Outcome::Busy
        );
    }
    for _ in 0..16 {
        sim.step();
        replay.step();
    }
    assert!(
        sim.removed > 0,
        "placed charge must destroy actual material"
    );
    assert!(
        sim.shell.is_none(),
        "local trigger must not accidentally charge a projectile"
    );
    assert_eq!(sim.snapshot(), replay.snapshot());
    assert!(sim.matter.economy.ledger.balanced());
    let mut physical = range();
    let before = physical.snapshot();
    assert_eq!(
        physical.execute_command(Command::Gun(G::Detonate { at: [424, 24, 324] })),
        Outcome::Rejected
    );
    assert_eq!(physical.snapshot(), before);
}

#[test]
fn gun_command_point_emitter_has_no_chassis_and_replays_its_origin() {
    let old = range();
    let mut layout = old.layout.clone().unwrap();
    layout.gun_point = true;
    let mut sim = BlastRange::with_layout(old.config, layout).unwrap();
    sim.execute_command(Command::Gun(G::Control));
    assert!(!sim.physical_gun());
    let chassis = sim.gun.motion.pos;
    let at = [192, 48, 336];
    assert!(sim
        .execute_command(Command::Gun(G::Origin { at }))
        .accepted());
    assert_eq!(sim.muzzle().map(fx::Fx::to_int_trunc), at);
    assert_eq!(
        sim.gun.motion.pos, chassis,
        "emitter repositioning does not move a hidden vehicle"
    );
    let before = sim.snapshot();
    assert_eq!(
        sim.execute_command(Command::Gun(G::Origin { at: [-1, 48, 336] })),
        Outcome::Rejected
    );
    assert_eq!(sim.snapshot(), before);
    let mut phase = ("", 0, None);
    let restored = BlastRange::restore_observed(sim.config, &before, |p, n, id| phase = (p, n, id));
    let mut replay =
        restored.unwrap_or_else(|| panic!("point emitter restore stopped at {phase:?}"));
    assert_eq!(replay.snapshot(), before);
    assert!(sim.refire());
    assert!(replay.refire());
    assert_eq!(
        sim.execute_command(Command::Gun(G::Origin { at: [224, 48, 336] })),
        Outcome::Rejected,
        "busy origin is immutable"
    );
    for _ in 0..8 {
        sim.step();
        replay.step();
    }
    assert!(
        sim.shell.is_some() || sim.impact.is_some(),
        "point must launch real shared projectile"
    );
    assert_eq!(sim.snapshot(), replay.snapshot());
    let mut physical = range();
    let before = physical.snapshot();
    assert_eq!(
        physical.execute_command(Command::Gun(G::Origin { at })),
        Outcome::Rejected
    );
    assert_eq!(physical.snapshot(), before);
}

#[test]
fn gun_command_point_streams_ignore_dummy_chassis_damage() {
    let d = modding::laboratory::Definition::stock();
    let mut layout = d.layout.clone();
    layout.gun_point = true;
    layout.gun = [192, 48, 336];
    let cfg = ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap();
    let mut sim = BlastRange::with_layout(cfg, layout).unwrap();
    sim.control();
    sim.gun.status.alive = false;
    sim.gun.status.weapon = false;
    assert!(sim.ready());
    assert!(sim.refire());
    for _ in 0..8 {
        sim.step();
    }
    assert!(
        !sim.streams.traces.is_empty() || sim.streams.left > 0,
        "point laser uses the existing stream path"
    );
    assert_eq!(
        sim.stream_muzzles()[0].map(fx::Fx::to_int_trunc),
        [192, 48, 336]
    );
}

#[cfg(test)]
fn owner(budget: usize) -> CommandRuntime {
    CommandRuntime::new(
        range(),
        7,
        Limits {
            retained: 32,
            producers: 2,
            per_tick: budget,
        },
    )
    .unwrap()
}

#[cfg(test)]
fn request(sequence: u64, tick: u32, command: Command) -> Envelope {
    Envelope {
        epoch: 7,
        producer: 0,
        sequence,
        requested_tick: tick,
        command,
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
    same(&r.world, &copy.world);
    assert_eq!(r.state_hash(), copy.state_hash());
    copy
}

#[cfg(test)]
#[test]
fn gun_commands_match_internal_legacy_operations_and_capture_once() {
    let mut direct = range();
    let mut typed = BlastRange::restore(direct.config, &direct.snapshot()).unwrap();
    typed.start_command_capture(32).unwrap();
    let commands = [
        G::Control,
        G::Control,
        G::Move {
            destination: [200, 360],
        },
        G::Move {
            destination: [-1, 336],
        },
        G::AutoAttack { enabled: true },
        G::Attack { target: 0 },
        G::Attack { target: usize::MAX },
        G::GroundFire { at: [450, 24, 336] },
        G::GroundFire { at: [420, 24, 336] },
        G::Refire,
        G::Stop,
        G::Aim { at: None },
        G::Aim {
            at: Some([400, 24, 336]),
        },
        G::AutoAttack { enabled: false },
    ];
    for command in commands {
        let expected = match command {
            G::Origin { .. } | G::Detonate { .. } | G::LabStream { .. } => Outcome::Rejected,
            G::Control => {
                let old = direct.player;
                direct.control_inner();
                if old {
                    Outcome::Unchanged
                } else {
                    Outcome::Applied
                }
            }
            G::Move { destination } => {
                if direct.order_move_inner(destination) {
                    Outcome::Applied
                } else {
                    Outcome::PathFailed
                }
            }
            G::Attack { target } => {
                if direct.order_attack_inner(target) {
                    Outcome::Applied
                } else {
                    Outcome::Rejected
                }
            }
            G::Stop => {
                direct.order_stop_inner();
                Outcome::Applied
            }
            G::AutoAttack { enabled } => {
                direct.set_auto_attack_inner(enabled);
                Outcome::Applied
            }
            G::GroundFire { at } => {
                if direct.order_ground_fire_inner(at) {
                    Outcome::Applied
                } else {
                    Outcome::Busy
                }
            }
            G::Refire => {
                if direct.refire_inner() {
                    Outcome::Applied
                } else {
                    Outcome::Busy
                }
            }
            G::Aim { at } => {
                let old = direct.aim;
                direct.aim = at;
                if old == at {
                    Outcome::Unchanged
                } else {
                    Outcome::Applied
                }
            }
        };
        assert_eq!(
            typed.execute_command(Command::Gun(command)),
            expected,
            "{command:?}"
        );
        same(&direct, &typed);
    }
    let trace = typed.finish_command_capture().unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), commands.len());
    for (i, record) in trace.records.iter().enumerate() {
        assert_eq!(record.command, Command::Gun(commands[i]));
        assert_eq!(record.sequence, i as u64);
        assert_eq!(record.phase, super::super::CommandPhase::BetweenTicks);
    }
    direct.start_command_capture(16).unwrap();
    direct.control();
    direct.order_move([200, 360]);
    direct.order_attack(0);
    direct.set_auto_attack(true);
    direct.order_ground_fire([450, 24, 336]);
    direct.refire();
    direct.order_stop();
    direct.toggle_patrols();
    assert_eq!(direct.finish_command_capture().unwrap().records.len(), 8);
}

#[cfg(test)]
#[test]
fn gun_busy_updates_target_but_invalid_aim_is_atomic_and_clock_cannot_wrap() {
    let mut r = range();
    r.control();
    assert!(r.order_ground_fire([450, 24, 336]));
    let deadline = r.fire_tick;
    r.attack = Some(0);
    assert_eq!(
        r.execute_command(Command::Gun(G::GroundFire { at: [440, 24, 336] })),
        Outcome::Busy
    );
    assert_eq!(r.aim, Some([440, 24, 336]));
    assert_eq!(r.attack, None);
    assert_eq!(r.fire_tick, deadline);
    assert!(Outcome::Busy.admitted() && !Outcome::Busy.accepted());
    for at in [[i32::MIN, 0, 0], [0, i32::MAX, 0], [0, 0, 1025]] {
        let before = r.snapshot();
        for command in [G::Aim { at: Some(at) }, G::GroundFire { at }] {
            assert_eq!(r.execute_command(Command::Gun(command)), Outcome::Rejected);
            assert_eq!(before, r.snapshot());
        }
    }
    r.order_stop();
    for tick in [u32::MAX - 2, u32::MAX - 1, u32::MAX] {
        r.tick = tick;
        let before = r.snapshot();
        assert_eq!(r.execute_command(Command::Gun(G::Refire)), Outcome::Busy);
        assert_eq!(before, r.snapshot());
    }
}

#[cfg(test)]
#[test]
fn gun_owner_roundtrips_every_input_busy_receipt_spill_and_acknowledgement() {
    let mut r = owner(2);
    let commands = [
        Command::Gun(G::Control),
        Command::Gun(G::Aim {
            at: Some([450, 24, 336]),
        }),
        Command::Gun(G::Refire),
        Command::Gun(G::GroundFire { at: [440, 24, 336] }),
        Command::Gun(G::Stop),
        Command::Gun(G::AutoAttack { enabled: true }),
        Command::Gun(G::Attack { target: 0 }),
        Command::Gun(G::Move {
            destination: [200, 360],
        }),
        Command::Gun(G::Aim { at: None }),
        Command::SetPatrols { enabled: false },
        Command::Gun(G::Attack { target: usize::MAX }),
        Command::Gun(G::Aim {
            at: Some([i32::MAX, 0, 0]),
        }),
    ];
    let ids: Vec<_> = commands
        .into_iter()
        .enumerate()
        .map(|(i, command)| {
            let receipt = r.submit(request(i as u64, 0, command)).unwrap();
            assert_eq!(receipt.assigned_tick, i as u32 / 2);
            receipt.id
        })
        .collect();
    r = replica(&r);
    for _ in 0..6 {
        r.step().unwrap();
        r = replica(&r);
    }
    assert_eq!(r.receipt(ids[3]).unwrap().outcome, Some(Outcome::Busy));
    assert_eq!(r.receipt(ids[10]).unwrap().outcome, Some(Outcome::Rejected));
    for id in ids {
        r.acknowledge(id).unwrap();
    }
    r = replica(&r);
    assert_eq!(r.retained(), 0);
    let before = r.state_hash();
    assert_eq!(
        r.submit(request(0, r.world.tick, Command::Gun(G::Stop))),
        Err(AdmissionError::Sequence { expected: 12 })
    );
    assert_eq!(before, r.state_hash());
}

#[cfg(test)]
#[test]
fn gun_owner_matches_direct_physics_with_restore_during_flight_and_damage() {
    let mut r = owner(2);
    let mut direct = BlastRange::restore(r.world.config, &r.world.snapshot()).unwrap();
    let inputs = [G::Control, G::GroundFire { at: [450, 24, 336] }];
    for (i, input) in inputs.into_iter().enumerate() {
        r.submit(request(i as u64, 0, Command::Gun(input))).unwrap();
    }
    let mut restored_flight = false;
    let mut impact = false;
    for tick in 0..240 {
        if tick == 0 {
            direct.control_inner();
            assert!(direct.order_ground_fire_inner([450, 24, 336]));
        }
        direct.step();
        assert!(r.step_record_ai().unwrap().is_none());
        same(&direct, &r.world);
        if !restored_flight && r.world.shell.is_some() {
            r = replica(&r);
            restored_flight = true;
        }
        impact |= r.world.impact.is_some();
        if tick == 239 {
            r = replica(&r);
        }
    }
    assert!(restored_flight && impact);
    assert!(r.world.removed > 0 || r.world.units.iter().any(|u| !u.removed.is_empty()));
}

#[cfg(test)]
#[test]
fn gun_automatic_targeting_does_not_record_internal_refires_as_external_input() {
    let mut r = range();
    r.start_command_capture(4).unwrap();
    r.set_auto_attack(true);
    let mut fired = false;
    for _ in 0..180 {
        r.step();
        fired |= r.shell.is_some() || r.impact.is_some();
    }
    assert!(fired);
    let trace = r.finish_command_capture().unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), 1);
    assert_eq!(
        trace.records[0].command,
        Command::Gun(G::AutoAttack { enabled: true })
    );
}

#[cfg(test)]
#[test]
fn gun_range_owner_rejects_battle_plan_without_consuming_sequence_and_replays_no_ai() {
    let mut r = owner(1);
    let before = r.state_hash();
    assert!(matches!(
        r.submit_plan(PlanEnvelope {
            epoch: 7,
            producer: 0,
            sequence: 0,
            decision_tick: 0,
            decisions: vec![],
        }),
        Err(AdmissionError::Plan(_))
    ));
    assert_eq!(before, r.state_hash());
    r.submit(request(0, 0, Command::Gun(G::Stop))).unwrap();
    let mut copy = replica(&r);
    assert!(r.step_record_ai().unwrap().is_none());
    copy.step_replay_ai(None).unwrap();
    assert_eq!(r.state_hash(), copy.state_hash());
    let mut forged = r.checkpoint();
    forged.inbox.entries[0].assigned_tick = r.world.tick;
    assert!(CommandRuntime::restore(r.world.config, forged).is_none());
}

#[cfg(test)]
#[test]
fn gun_pending_payloads_and_patrol_values_affect_owner_hash() {
    let r = owner(1);
    let variants = [
        Command::Gun(G::Control),
        Command::Gun(G::Move {
            destination: [200, 360],
        }),
        Command::Gun(G::Move {
            destination: [201, 360],
        }),
        Command::Gun(G::Move {
            destination: [200, 361],
        }),
        Command::Gun(G::Attack { target: 0 }),
        Command::Gun(G::Attack { target: 1 }),
        Command::Gun(G::Stop),
        Command::Gun(G::AutoAttack { enabled: false }),
        Command::Gun(G::AutoAttack { enabled: true }),
        Command::Gun(G::GroundFire { at: [450, 24, 336] }),
        Command::Gun(G::GroundFire { at: [451, 24, 336] }),
        Command::Gun(G::Refire),
        Command::Gun(G::Aim { at: None }),
        Command::Gun(G::Aim {
            at: Some([450, 24, 336]),
        }),
        Command::SetPatrols { enabled: false },
        Command::SetPatrols { enabled: true },
    ];
    let mut hashes = Vec::new();
    for command in variants {
        let mut copy = replica(&r);
        copy.submit(request(0, 3, command)).unwrap();
        copy = replica(&copy);
        assert!(!hashes.contains(&copy.state_hash()));
        hashes.push(copy.state_hash());
    }
}
