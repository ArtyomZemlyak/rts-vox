//! Reproducible profiling input: one ordinary ground-fire order, unchanged SIM.
use sim_core::blast::battle::Order;
use sim_core::blast::BlastRange;
use std::{io::Write, time::Instant};
#[path = "../src/bin/blast/profile/core.rs"]
mod core;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let output = args
        .get(1)
        .expect("profile_nuclear OUTPUT_DIRECTORY [TICKS]");
    let ticks = args.get(2).map_or(0, |v| v.parse::<usize>().unwrap());
    let base =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let editor =
        modding::range::RangeEditor::from_ron(include_str!("../../../data/range-editor.ron"))
            .unwrap();
    let preset = modding::range_skirmish::stock(18).unwrap();
    let layout = preset
        .apply_to(base)
        .unwrap()
        .with_ground_level(sim_core::blast::FLOOR + editor.subsoil_units)
        .unwrap();
    let target = [layout.target[0], layout.ground_level, layout.target[2]];
    let config =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut sim = BlastRange::with_layout(config, layout).unwrap();
    assert!(sim.set_physics_workers(4));
    assert!(sim.set_navigation_workers(8));
    sim.start_battle().unwrap();
    assert!(sim.battle_order(0, Order::Ground(target), None));
    std::fs::create_dir_all(output).unwrap();
    std::fs::write(format!("{output}/nuclear-start.snap"), sim.snapshot()).unwrap();
    println!(
        "snapshot tick={} target={target:?} hash={:016x}",
        sim.tick,
        sim.state_hash()
    );
    if ticks == 0 {
        return;
    }
    let epoch = Instant::now();
    let mut trace = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        trace.push(core::Tick::record(&mut sim, 0, epoch));
    }
    core::save(&format!("{output}/nuclear.csv"), &trace).unwrap();
    println!(
        "nuclear ticks={} hash={:016x} shots={:?}",
        sim.tick,
        sim.state_hash(),
        sim.battle
            .actors
            .iter()
            .map(|a| a.fired)
            .collect::<Vec<_>>()
    );
}
