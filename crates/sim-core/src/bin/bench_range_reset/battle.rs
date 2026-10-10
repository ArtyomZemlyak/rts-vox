//! Repeated real combat, common reset and restart in one process.
//! Clocks and diagnostics stay outside the authoritative simulation.
use modding::{explosion::ExplosionConfig, range::RangeLayout};
use sim_core::blast::{placement::Validator, BlastRange};
use std::time::Instant;

pub(super) fn run(mode: &str, runs: u32, ticks: u32, physics: usize, navigation: usize) {
    assert!(ticks > 1, "--ticks must exceed one restart probe tick");
    assert!((1..=8).contains(&physics) && (1..=8).contains(&navigation));
    let mut layout =
        RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    match mode {
        "world" => {
            let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
                "../../../../../data/battle-world.ron"
            ))
            .unwrap();
            layout.units = preset.units;
            layout.blocks.extend(preset.blocks);
        }
        "canister" => {
            let preset = modding::range_skirmish::Skirmish::from_ron(include_str!(
                "../../../../../data/skirmish-canister.ron"
            ))
            .unwrap();
            layout.gun = preset.zone.spawn;
            layout.target = preset.zone.target;
            layout.zones.push(preset.zone);
            layout.units = preset.units;
            layout.blocks.extend(preset.blocks);
        }
        _ => panic!("--battle: world or canister"),
    }
    let config =
        ExplosionConfig::from_ron(include_str!("../../../../../data/explosion.ron")).unwrap();
    let validator = Validator::default();
    let mut sim = BlastRange::with_layout(config, layout.clone()).unwrap();
    configure(&mut sim, physics, navigation);
    let fresh = sim.state_hash();
    let bits = sim.world.bits().as_ptr();
    assert!(sim.set_ai_vs_ai(true));
    sim.start_battle().unwrap();
    let cold_restart = Instant::now();
    sim.step();
    let cold_restart_us = cold_restart.elapsed().as_micros();
    let first_tick = sim.state_hash();
    let mut end_hash = None;
    println!("RESET_BATTLE fixture={mode} runs={runs} ticks={ticks} rayon={} physics={physics} navigation={navigation} ai_vs_ai=true fresh={fresh:016x}", rayon::current_num_threads());
    println!("RESET_BATTLE cold_restart_us={cold_restart_us} first_hash={first_tick:016x}");
    for cycle in 1..=runs {
        let active = Instant::now();
        while sim.tick < ticks {
            sim.step();
        }
        let active_us = active.elapsed().as_micros();
        let hash = sim.state_hash();
        assert_eq!(
            *end_hash.get_or_insert(hash),
            hash,
            "combat changed after reset"
        );
        assert!(sim.matter.economy.ledger.balanced());
        assert!(sim.battle.actors.iter().any(|a| a.fired > 0), "no combat");
        assert!(sim.units.iter().any(|u| u.revision > 0), "no real damage");
        let touched = sim.world.touched_chunks();
        assert!(touched > 0, "combat never changed terrain");
        let fragments = sim.matter.debris.fragments.len();
        let felling = sim.matter.felling_pending();
        let mut phase = Instant::now();
        let reset = phase;
        let mut phases = Vec::new();
        sim = sim
            .rebuilt_with_layout_observed(config, layout.clone(), &validator, |name| {
                let now = Instant::now();
                phases.push((name, now.duration_since(phase).as_micros()));
                phase = now;
            })
            .unwrap();
        let reset_us = reset.elapsed().as_micros();
        // Like the native profile, reapply owned worker controls after rebuild.
        configure(&mut sim, physics, navigation);
        assert_eq!(bits, sim.world.bits().as_ptr(), "occupancy was reallocated");
        assert_eq!(sim.state_hash(), fresh, "reset differs from fresh factory");
        assert_eq!(sim.tick, 0);
        assert_eq!(sim.world.touched_chunks(), 0);
        assert!(sim.matter.debris.fragments.is_empty());
        assert_eq!(sim.matter.felling_pending(), 0);
        assert_eq!(sim.pending_kinetic_impacts(), 0);
        assert!(sim.battle.rounds.is_empty());
        assert_eq!(sim.battle.wave_progress().count(), 0);
        assert!(sim.matter.economy.ledger.balanced());
        assert!(sim.set_ai_vs_ai(true));
        sim.start_battle().unwrap();
        let restart = Instant::now();
        sim.step();
        let restart_us = restart.elapsed().as_micros();
        assert_eq!(
            sim.state_hash(),
            first_tick,
            "restart differs from first tick"
        );
        println!("RESET_BATTLE cycle={cycle} active_us={active_us} reset_us={reset_us} restart_us={restart_us} touched={touched} fragments={fragments} felling={felling} end_hash={hash:016x}");
        for (name, us) in phases {
            println!("RESET_BATTLE_PHASE cycle={cycle} name={name} us={us}");
        }
    }
    println!(
        "RESET_BATTLE complete={runs} tick={} restart_hash={:016x}",
        sim.tick,
        sim.state_hash()
    );
}

fn configure(sim: &mut BlastRange, physics: usize, navigation: usize) {
    assert!(sim.set_physics_workers(physics));
    assert!(sim.set_navigation_workers(navigation));
    assert_eq!(sim.worker_counts(), [physics, navigation]);
}
