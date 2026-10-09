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
