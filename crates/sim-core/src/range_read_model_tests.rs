use super::*;
use modding::{explosion::ExplosionConfig, range::RangeLayout};

#[path = "range_parameters_tests.rs"]
mod parameters;

fn world() -> BlastRange {
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
            kind: RangeUnitKind::Tank,
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

#[test]
fn publication_is_owned_and_does_not_change_physics_or_save() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<View>();
    let mut sim = world();
    sim.start_battle().unwrap();
    let before = sim.snapshot();
    let old = View::new(&sim, 7);
    assert_eq!(before, sim.snapshot());
    for _ in 0..30 {
        sim.step();
    }
    let current = View::new(&sim, 7);
    assert_eq!(old.tick, 0);
    assert_eq!(old.phase, Phase::Fighting);
    assert_eq!(current.tick, 30);
    assert_eq!(current.counts, sim.battle_counts());
    assert_eq!(current.base_integrity, sim.battle_base_integrity());
    for (id, u) in current.units.iter().enumerate() {
        assert_eq!(u.position, sim.units[id].motion.pos);
        assert_eq!(u.status, sim.units[id].status);
        assert_eq!(u.weapon, sim.battle_weapon(id));
        assert_eq!(u.attack_range, sim.battle_attack_range(id));
    }
    let config = sim.config.clone();
    let restored = BlastRange::restore(config, &before).unwrap();
    assert_eq!(old, View::new(&restored, 7));
}

#[test]
fn same_tick_commands_loadout_and_damage_are_not_hidden_by_a_tick_cache() {
    let mut sim = world();
    let mut view = View::new(&sim, 1);
    let capacity = view.units.capacity();
    sim.start_battle().unwrap();
    assert!(sim.battle_order(0, Order::Attack(1), None));
    assert!(sim.set_battle_weapon(0, Some(Weapon::Kinetic)));
    sim.units[0].integrity.core = Fx::from_ratio(1, 2);
    sim.units[1].status.alive = false;
    view.update(&sim, 1);
    assert_eq!(view.tick, 0);
    assert_eq!(view.phase, Phase::Fighting);
    assert_eq!(view.units[0].order, Some(Order::Attack(1)));
    assert_eq!(view.units[0].weapon, Some(Weapon::Kinetic));
    assert_eq!(view.units[0].integrity.core, Fx::from_ratio(1, 2));
    assert_eq!(view.counts, [1, 0]);
    assert_eq!(view.units.capacity(), capacity);
    assert_eq!(view.selected_allies(Some(0), &[]), vec![0]);
    assert_eq!(
        view.selected_allies(Some(0), &[1, usize::MAX]),
        Vec::<usize>::new()
    );
}

#[test]
fn reset_replaces_generation_and_shorter_roster_without_old_slots() {
    let sim = world();
    let mut view = View::new(&sim, 8);
    let old = view.clone();
    let mut layout = sim.layout.clone().unwrap();
    layout.units.truncate(1);
    layout.units[0].allied = false;
    let next = BlastRange::with_layout(sim.config.clone(), layout).unwrap();
    view.update(&next, 9);
    assert_eq!((view.generation, view.tick, view.units.len()), (9, 0, 1));
    assert_eq!(view.phase, Phase::Setup);
    assert_eq!(view.counts, [0, 1]);
    assert!(!view.units[0].allied);
    assert!(view.selected_allies(Some(0), &[]).is_empty());
    assert_eq!((old.generation, old.units.len()), (8, 2));
}

#[test]
fn command_owner_can_publish_without_exposing_mutable_world() {
    use crate::blast::battle::commands::{runtime::*, Command};
    let mut sim = world();
    sim.start_battle().unwrap();
    let mut owner = CommandRuntime::new(
        sim,
        5,
        Limits {
            retained: 2,
            producers: 1,
            per_tick: 1,
        },
    )
    .unwrap();
    let receipt = owner
        .submit(Envelope {
            epoch: 5,
            producer: 0,
            sequence: 0,
            requested_tick: 0,
            command: Command::Order {
                actor: 0,
                order: Order::Hold,
                destination: None,
            },
        })
        .unwrap();
    let mut view = View::new(owner.world(), 5);
    owner.step().unwrap();
    view.update(owner.world(), 5);
    assert_eq!((view.generation, view.tick), (5, 1));
    assert_eq!(view.units[0].order, Some(Order::Hold));
    assert!(owner.receipt(receipt.id).unwrap().outcome.is_some());
}

#[test]
fn panel_state_distinguishes_automatic_loadout_and_tracks_same_tick_changes() {
    let mut sim = world();
    assert!(sim.set_battle_weapon(0, None));
    let mut view = View::new(&sim, 3);
    assert_eq!(view.units[0].loadout, Some(None));
    assert!(view.units[0].weapon.is_some());
    sim.auto_attack = true;
    sim.attack = Some(1);
    sim.patrols_running = true;
    sim.battle.autonomous = true;
    sim.units[0].integrity.engine = Fx::from_ratio(1, 4);
    sim.units[0].motion.route.push([160, 320]);
    sim.units[0].motion.laps = 12;
    assert!(sim.set_battle_weapon(0, Some(Weapon::Kinetic)));
    view.update(&sim, 3);
    assert_eq!(view.tick, 0);
    assert!(view.auto_attack && view.patrols_running && view.autonomous);
    assert_eq!(view.attack, Some(1));
    assert_eq!(view.units[0].loadout, Some(Some(Weapon::Kinetic)));
    assert_eq!(view.units[0].integrity.engine, Fx::from_ratio(1, 4));
    assert_eq!(view.units[0].patrol_laps, Some(12));
    sim.units[0].motion.route.clear();
    sim.battle.actors.clear();
    view.update(&sim, 4);
    assert_eq!(view.units[0].loadout, None);
    assert_eq!(view.units[0].patrol_laps, None);
}

#[test]
fn inspection_is_owned_fresh_and_counts_only_the_current_roster() {
    use crate::burst::{Fragment, IDENTITY};
    use crate::damage::V3;
    let mut sim = world();
    sim.shields.push(super::super::RangeShield {
        at: [160, 32, 320],
        radius: 40,
        charge: 75,
        down: 3,
        hits: 2,
    });
    for id in 0..3 {
        sim.matter.debris.fragments.push(Fragment {
            id,
            position: V3::ZERO,
            velocity: V3::ZERO,
            material: 2,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 0,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: 0,
            resting: id != 1,
        });
    }
    let before = sim.snapshot();
    let mut view = Inspection::new(&sim, 7);
    assert_eq!(sim.snapshot(), before);
    assert_eq!((view.fragments, view.resting_fragments), (3, 2));
    assert_eq!(
        view.shields[0],
        Shield {
            radius: 40,
            charge: 75,
            down: 3
        }
    );
    let old = view.clone();
    let capacity = view.shields.capacity();
    sim.shields[0].charge = 20;
    sim.matter.debris.fragments[1].resting = true;
    view.update(&sim, 7);
    assert_eq!(
        (view.tick, view.resting_fragments, view.shields[0].charge),
        (0, 3, 20)
    );
    assert_eq!(view.shields.capacity(), capacity);
    sim.shields.clear();
    sim.matter.debris.fragments.clear();
    view.update(&sim, 8);
    assert_eq!(
        (view.generation, view.fragments, view.resting_fragments),
        (8, 0, 0)
    );
    assert!(view.shields.is_empty());
    assert_eq!(
        (old.fragments, old.resting_fragments, old.shields[0].charge),
        (3, 2, 75)
    );
}
