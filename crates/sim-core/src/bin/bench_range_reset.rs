//! Manual release benchmark for resetting damaged chunks on the 1 km range.
//! Kept in a simulation bench so the renderer never writes world state.
use modding::explosion::ExplosionConfig;
use modding::range::RangeLayout;
use sim_core::blast::{placement::Validator, BlastRange};
use std::time::Instant;

#[path = "bench_range_reset/battle.rs"]
mod battle;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let number = |flag: &str, default| {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map_or(default, |w| w[1].parse::<u32>().expect(flag))
    };
    let runs = number("--runs", 3);
    assert!(runs > 0, "--runs must be positive");
    if let Some(mode) = args.windows(2).find(|w| w[0] == "--battle") {
        battle::run(
            &mode[1],
            runs,
            number("--ticks", 600),
            number("--physics-workers", 4) as usize,
            number("--nav-workers", 4) as usize,
        );
        return;
    }
    let layout = RangeLayout::from_ron(include_str!("../../../../data/range-km.ron"))
        .expect("stock kilometre range");
    let config = ExplosionConfig::from_ron(include_str!("../../../../data/energy-cannon.ron"))
        .expect("stock energy weapon");
    let mark = Instant::now();
    let cold_validator = Validator::default();
    println!(
        "RESET cold-validator ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    cold_validator.validate(&layout).expect("cold validation");
    println!(
        "RESET cold-validation ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    let mut sim = BlastRange::with_layout(config, layout.clone()).expect("range");
    println!(
        "RESET initial-world ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    let validator = Validator::default();
    println!(
        "RESET validator-init ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    sim = sim
        .rebuilt_with_layout(config, layout.clone(), &validator)
        .expect("warm reset");

    for run in 0..runs {
        for dz in 0..10 {
            for dx in 0..10 {
                sim.world.queue_dig(3220 + dx * 32, 8, 2120 + dz * 32);
            }
        }
        let mark = Instant::now();
        while sim.world.queued() != 0 {
            sim.world.flush();
        }
        assert_eq!(sim.world.touched_chunks(), 100);
        println!(
            "RESET heavy-damage-setup run={run} chunks=100 ms={:.3}",
            mark.elapsed().as_secs_f64() * 1000.0
        );
        let mark = Instant::now();
        let mut phase = mark;
        sim = sim
            .rebuilt_with_layout_observed(config, layout.clone(), &validator, |name| {
                println!(
                    "RESET phase run={run} name={name} ms={:.3}",
                    phase.elapsed().as_secs_f64() * 1000.0
                );
                phase = Instant::now();
            })
            .expect("reset");
        assert_eq!(sim.world.touched_chunks(), 0);
        println!(
            "RESET heavy-damage-reset run={run} ms={:.3}",
            mark.elapsed().as_secs_f64() * 1000.0
        );
    }
    for dz in 0..10 {
        for dx in 0..10 {
            sim.world.queue_dig(3220 + dx * 32, 8, 2120 + dz * 32);
        }
    }
    while sim.world.queued() != 0 {
        sim.world.flush();
    }
    let mark = Instant::now();
    sim.world.restore_pristine();
    println!(
        "RESET world-only ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
}
