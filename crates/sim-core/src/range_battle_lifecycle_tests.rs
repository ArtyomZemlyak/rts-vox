use super::*;
use command_tests::{arena, order, same};
use commands::{BattleLifecycle as Lifecycle, Command, Outcome};

#[cfg(test)]
#[test]
fn lifecycle_legacy_and_typed_calls_match_and_capture_exactly_once() {
    let mut typed = arena(RangeUnitKind::Tank);
    let mut legacy = arena(RangeUnitKind::Tank);
    typed.start_battle_command_capture(8).unwrap();
    legacy.start_battle_command_capture(8).unwrap();
    for (command, expected) in [
        (Lifecycle::Autonomy { enabled: true }, Outcome::Applied),
        (Lifecycle::Autonomy { enabled: true }, Outcome::Unchanged),
        (Lifecycle::FreeControl, Outcome::Applied),
        (Lifecycle::StartBattle, Outcome::Applied),
        (Lifecycle::StartBattle, Outcome::Rejected),
        (Lifecycle::FreeControl, Outcome::Rejected),
        (Lifecycle::Autonomy { enabled: false }, Outcome::Rejected),
    ] {
        let before = typed.snapshot();
        assert_eq!(
            typed.execute_command(Command::BattleLifecycle(command)),
            expected
        );
        let accepted = match command {
            Lifecycle::StartBattle => legacy.start_battle().is_ok(),
            Lifecycle::FreeControl => legacy.start_free_control().is_ok(),
            Lifecycle::Autonomy { enabled } => legacy.set_ai_vs_ai(enabled),
        };
        assert_eq!(accepted, expected.accepted());
        same(&typed, &legacy);
        if matches!(expected, Outcome::Rejected | Outcome::Unchanged) {
            assert_eq!(before, typed.snapshot());
        }
    }
    let a = typed.finish_battle_command_capture().unwrap();
    let b = legacy.finish_battle_command_capture().unwrap();
    assert_eq!(a.records, b.records);
    assert_eq!(a.records.len(), 7);
    assert!(a.complete());
}

#[cfg(test)]
#[test]
fn lifecycle_rejects_missing_team_and_nonbattle_without_changing_world() {
    let mut world = arena(RangeUnitKind::Tank);
    world.units[1].status.alive = false;
    let before = world.snapshot();
    assert_eq!(
        world.execute_battle_lifecycle(Lifecycle::StartBattle),
        Err("Нужен хотя бы один свой и один вражеский юнит")
    );
    assert_eq!(before, world.snapshot());
    let mut layout = world.layout.clone().unwrap();
    layout.battle = false;
    layout.units.clear();
    let mut world = BlastRange::with_layout(world.config, layout).unwrap();
    let before = world.snapshot();
    for command in [
        Lifecycle::StartBattle,
        Lifecycle::FreeControl,
        Lifecycle::Autonomy { enabled: true },
    ] {
        assert_eq!(
            world.execute_command(Command::BattleLifecycle(command)),
            Outcome::Rejected
        );
        assert_eq!(before, world.snapshot());
    }
    let mut direct = BlastRange::restore(world.config, &world.snapshot()).unwrap();
    let mut owner = commands::runtime::CommandRuntime::new(
        world,
        7,
        commands::runtime::Limits {
            retained: 4,
            producers: 1,
            per_tick: 1,
        },
    )
    .unwrap();
    for (sequence, command) in [
        Lifecycle::StartBattle,
        Lifecycle::FreeControl,
        Lifecycle::Autonomy { enabled: true },
    ]
    .into_iter()
    .enumerate()
    {
        let receipt = owner
            .submit(commands::runtime::Envelope {
                epoch: 7,
                producer: 0,
                sequence: sequence as u64,
                requested_tick: owner.world().tick,
                command: Command::BattleLifecycle(command),
            })
            .unwrap();
        owner.step().unwrap();
        direct.step();
        assert_eq!(
            owner.receipt(receipt.id).unwrap().outcome,
            Some(Outcome::Rejected)
        );
        same(owner.world(), &direct);
    }
}

#[cfg(test)]
#[test]
fn lifecycle_repeated_free_control_stops_new_routes_without_starting_ai() {
    let mut world = arena(RangeUnitKind::Tank);
    assert_eq!(
        world.execute_battle_lifecycle(Lifecycle::FreeControl),
        Ok(Outcome::Applied)
    );
    assert!(world
        .execute_command(order(0, Order::Move, Some([210, 336])))
        .accepted());
    assert!(world.units[0].motion.destination.is_some());
    assert_eq!(
        world.execute_battle_lifecycle(Lifecycle::FreeControl),
        Ok(Outcome::Applied)
    );
    assert!(world.units[0].motion.destination.is_none());
    assert!(world.units[0].motion.route.is_empty());
    for _ in 0..30 {
        world.step();
    }
    assert_eq!(world.battle.phase, Phase::Setup);
    assert!(world.battle.actors.iter().all(|actor| actor.fired == 0));
}

#[cfg(test)]
#[test]
fn lifecycle_ammunition_validation_noop_and_legacy_capture_match() {
    let mut typed = arena(RangeUnitKind::Tank);
    let mut legacy = arena(RangeUnitKind::Tank);
    typed.start_battle_command_capture(16).unwrap();
    legacy.start_battle_command_capture(16).unwrap();
    for (actor, weapon, expected) in [
        (0, Some(Weapon::Energy), Outcome::Applied),
        (0, Some(Weapon::Energy), Outcome::Unchanged),
        (1, Some(Weapon::Blast), Outcome::Rejected),
        (usize::MAX, None, Outcome::Rejected),
        (0, Some(Weapon::MachineGun), Outcome::Rejected),
        (0, Some(Weapon::Laser), Outcome::Rejected),
        (0, Some(Weapon::Canister), Outcome::Rejected),
        (0, None, Outcome::Applied),
    ] {
        let before = typed.snapshot();
        assert_eq!(
            typed.execute_command(Command::SetBattleWeapon { actor, weapon }),
            expected
        );
        assert_eq!(legacy.set_battle_weapon(actor, weapon), expected.accepted());
        same(&typed, &legacy);
        if expected != Outcome::Applied {
            assert_eq!(before, typed.snapshot());
        }
    }
    let a = typed.finish_battle_command_capture().unwrap();
    assert_eq!(a.records.len(), 8);
    assert_eq!(
        a.records,
        legacy.finish_battle_command_capture().unwrap().records
    );
}
