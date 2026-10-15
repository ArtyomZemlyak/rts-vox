use fx::Fx;
use modding::{
    explosion::ExplosionConfig,
    range::{RangeLayout, RangeUnitDef, RangeUnitKind},
};
use sim_core::blast::BlastRange;

fn trial(kind: RangeUnitKind, config: &str, distance: i32) -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.size = [2048, 192, 1024];
    layout.clear_min = [0, 0];
    layout.clear_max = [2048, 1024];
    layout.blocks.clear();
    layout.course.as_mut().unwrap().ramps.clear();
    layout.course.as_mut().unwrap().patrols.clear();
    layout.gun = [128, 16, 336];
    layout.gun_kind = kind;
    layout.units = vec![RangeUnitDef {
        kind: RangeUnitKind::Tank,
        at: [128 + distance, 16, 336],
        allied: false,
        weapon: None,
    }];
    let mut sim =
        BlastRange::with_layout(ExplosionConfig::from_ron(config).unwrap(), layout).unwrap();
    sim.control();
    sim
}

#[test]
fn a_trial_projectile_closes_to_the_shared_range_and_replays_its_approach() {
    let reach = modding::attack_ranges::stock().energy;
    let mut sim = trial(
        RangeUnitKind::Tank,
        include_str!("../../../data/energy-cannon.ron"),
        reach + 101,
    );
    assert_eq!(sim.gun_attack_range(), reach);
    assert!(sim.order_attack(0));
    sim.step();
    assert!(sim.gun.motion.destination.is_some());
    assert!(sim.shell.is_none(), "fired outside its tactical range");
    let mut replay = BlastRange::restore(sim.config, &sim.snapshot()).unwrap();
    let mut fired = false;
    for _ in 0..300 {
        sim.step();
        replay.step();
        assert_eq!(sim.state_hash(), replay.state_hash());
        if sim.shell.is_some() {
            let dx = sim.units[0].motion.pos[0].sub(sim.gun.motion.pos[0]);
            let dz = sim.units[0].motion.pos[2].sub(sim.gun.motion.pos[2]);
            assert!(dx.mul(dx).add(dz.mul(dz)) <= Fx::from_int(reach).mul(Fx::from_int(reach)));
            fired = true;
            break;
        }
    }
    assert!(fired, "trial projectile never approached and fired");
}

#[test]
fn a_stationary_trial_waits_for_a_target_inside_its_actual_radius() {
    let reach = modding::attack_ranges::stock().kinetic;
    let mut sim = trial(
        RangeUnitKind::TurretT1,
        include_str!("../../../data/kinetic-shell.ron"),
        reach + 1,
    );
    assert_eq!(sim.gun_attack_range(), reach);
    sim.set_auto_attack(true);
    for _ in 0..90 {
        sim.step();
    }
    assert!(sim.attack.is_none());
    assert!(sim.shell.is_none());
    assert!(sim.gun.motion.destination.is_none());
    sim.units[0].motion.pos[0] = Fx::from_int(128 + reach - 20);
    sim.step();
    assert_eq!(sim.attack, Some(0));
}

#[test]
fn direct_trial_variants_share_the_battle_radius_without_inferring_heat_or_energy() {
    let reach = modding::attack_ranges::stock().energy;
    for text in [
        include_str!("../../../data/energy-cannon.ron"),
        include_str!("../../../data/anni-cannon.ron"),
    ] {
        let mut sim = trial(RangeUnitKind::Tank, text, reach + 1);
        assert_eq!(sim.gun_attack_range(), reach);
        let mut config = sim.config;
        config.heat_per_cell = 0;
        config.heat_units = 0;
        config.hull_impulse_milli = 0;
        sim.reconfigure(config);
        assert_eq!(sim.gun_attack_range(), reach);
    }
}
