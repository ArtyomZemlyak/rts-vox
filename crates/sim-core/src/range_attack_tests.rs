use super::*;
use modding::range::RangeUnitDef;

fn arena(kind: RangeUnitKind, weapon: Weapon) -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.size = [4096, 192, 1024];
    layout.clear_min = [0, 0];
    layout.clear_max = [4096, 1024];
    layout.course = None;
    layout.blocks.clear();
    layout.units = vec![
        RangeUnitDef {
            kind,
            at: [300, 16, 500],
            allied: true,
            weapon: None,
        },
        RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [600, 16, 500],
            allied: false,
            weapon: None,
        },
    ];
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut sim = BlastRange::with_layout(config, layout).unwrap();
    sim.start_battle().unwrap();
    if matches!(kind, RangeUnitKind::Tank | RangeUnitKind::Artillery) {
        assert!(sim.set_battle_weapon(0, Some(weapon)));
    }
    sim.battle.ai.next_tick = u32::MAX;
    sim.battle.actors[1].order = Order::Hold;
    sim.battle_order(0, Order::Attack(1), None);
    sim.battle.trace_fire(true);
    sim
}

#[test]
fn attack_radius_matches_controller_view_and_fire_boundary_for_every_loadout() {
    use RangeUnitKind::*;
    let t = modding::attack_ranges::stock();
    for (kind, weapon, reach) in [
        (Tank, Weapon::Blast, t.tank_blast),
        (Tank, Weapon::Kinetic, t.kinetic),
        (Tank, Weapon::Energy, t.energy),
        (Tank, Weapon::Annihilation, t.energy),
        (Strider, Weapon::MachineGun, t.machine_gun),
        (Spider, Weapon::Laser, t.spider_laser),
        (Artillery, Weapon::Blast, t.artillery),
        (ArtilleryT2, Weapon::Canister, t.canister),
        (ArtilleryTurretT2, Weapon::Canister, t.canister),
        (ArtilleryTurretT3, Weapon::Siege, t.siege),
        (TurretT1, Weapon::Kinetic, t.kinetic),
        (TurretT2, Weapon::Energy, t.energy),
    ] {
        let mut sim = arena(kind, weapon);
        let start = sim.units[0].motion.pos[0];
        sim.units[1].motion.pos[0] = start.add(Fx::from_int(reach + 1));
        sim.step();
        assert_eq!(sim.battle.actors[0].fired, 0, "outside {kind:?}/{weapon:?}");
        assert_eq!(
            sim.battle.fire_gate(0),
            Some(FireGate::Approach),
            "outside {kind:?}/{weapon:?}"
        );
        sim.units[0].motion.stop();
        let origin = sim.units[0].motion.pos[0];
        sim.units[1].motion.pos[0] = origin.add(Fx::from_int(reach));
        sim.step();
        assert_ne!(
            sim.battle.fire_gate(0),
            Some(FireGate::Approach),
            "boundary {kind:?}/{weapon:?}"
        );
        assert_eq!(sim.battle_attack_range(0), Some(reach));
        let view = crate::blast::read_model::View::new(&sim, 1);
        assert_eq!(view.units[0].attack_range, Some(reach));
        let obs = sim.enemy_ai_observation().unwrap();
        assert!(obs.units[0].attack_ranges.iter().all(|&r| r == reach));
        // Check an actual emitted shot inside the radius, not only metadata.
        sim.units[1].motion.pos[0] = origin.add(Fx::from_int(reach - 40));
        for _ in 0..180 {
            sim.step();
            if sim.battle.actors[0].fired > 0 {
                break;
            }
        }
        assert!(sim.battle.actors[0].fired > 0, "inside {kind:?}/{weapon:?}");
    }
}

#[test]
fn an_unarmed_base_has_no_attack_radius_in_any_consumer() {
    let mut sim = arena(RangeUnitKind::Base, Weapon::Blast);
    assert_eq!(sim.battle_attack_range(0), None);
    sim.step();
    assert_eq!(sim.battle.actors[0].fired, 0);
    assert_eq!(
        crate::blast::read_model::View::new(&sim, 1).units[0].attack_range,
        None
    );
    assert!(sim.enemy_ai_observation().unwrap().units[0]
        .attack_ranges
        .iter()
        .all(|&r| r == 0));
}
