//! Manual release benchmark for resetting damaged chunks on the 1 km range.
//! Kept in a simulation bench so the renderer never writes world state.
use modding::explosion::ExplosionConfig;
use modding::range::RangeLayout;
use sim_core::blast::{placement::Validator, BlastRange};
use std::time::Instant;

fn main() {
    let layout = RangeLayout::from_ron(include_str!("../../../../data/range-km.ron"))
        .expect("stock kilometre range");
    let config = ExplosionConfig::from_ron(include_str!("../../../../data/energy-cannon.ron"))
        .expect("stock energy weapon");
    let mark = Instant::now();
    let mut sim = BlastRange::with_layout(config.clone(), layout.clone()).expect("range");
    println!(
        "RESET initial-world ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    sim = sim
        .rebuilt_with_layout(config.clone(), layout.clone(), &Validator::default())
        .expect("warm reset");

    for run in 0..3 {
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
        sim = sim
            .rebuilt_with_layout(config.clone(), layout.clone(), &Validator::default())
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
