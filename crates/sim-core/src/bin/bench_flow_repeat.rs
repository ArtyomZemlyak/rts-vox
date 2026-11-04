//! Repeatable, windowless material-flow probe. Wall clocks never enter SIM.
use modding::{explosion::ExplosionConfig, laboratory::experiments};
use sim_core::blast::BlastRange;
use std::{collections::BTreeMap, time::Instant};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let arg = |name: &str| {
        args.windows(2)
            .find(|v| v[0] == name)
            .map(|v| v[1].as_str())
    };
    let ticks: u32 = arg("--ticks").unwrap_or("240").parse().unwrap();
    let repeats: u32 = arg("--repeats").unwrap_or("3").parse().unwrap();
    let workers: usize = arg("--workers").unwrap_or("1").parse().unwrap();
    assert!(ticks > 0 && repeats > 0 && workers > 0);
    let config = ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron")).unwrap();
    for id in [
        "sand-flow",
        "soil-slide",
        "rock-sand",
        "dune-rockfall",
        "water-fall",
        "water-vessels",
        "water-erosion",
    ] {
        if arg("--scene").is_some_and(|s| s != id) {
            continue;
        }
        let mut reference = None;
        for run in 0..repeats {
            let e = experiments::stock()
                .into_iter()
                .find(|e| e.id == id)
                .unwrap();
            if run == 0 {
                if let Some(dir) = arg("--fixture-dir") {
                    std::fs::create_dir_all(dir).unwrap();
                    std::fs::write(format!("{dir}/{id}.ron"), e.layout.to_ron().unwrap()).unwrap();
                }
            }
            let mut sim = BlastRange::with_layout(config, e.layout).unwrap();
            if args.iter().any(|v| v == "--legacy-water") {
                sim.water.tuning.pressure_checks = 2048;
                sim.water.tuning.pressure_packets = 64;
                sim.water.tuning.pressure_interval = 4;
                sim.water.tuning.transfers_per_tick = 1024;
            }
            sim.control();
            sim.set_physics_workers(workers);
            sim.set_navigation_workers(workers);
            let mut samples: BTreeMap<String, Vec<u128>> = BTreeMap::new();
            let mut checkpoints = Vec::new();
            for tick in 1..=ticks {
                let start = Instant::now();
                let mut previous = start;
                sim.step_observed(|phase| {
                    let now = Instant::now();
                    samples
                        .entry(phase.to_string())
                        .or_default()
                        .push((now - previous).as_nanos());
                    previous = now;
                });
                samples
                    .entry("TOTAL".into())
                    .or_default()
                    .push(start.elapsed().as_nanos());
                if tick % 20 == 0 || tick == ticks {
                    checkpoints.push((
                        tick,
                        sim.state_hash(),
                        sim.material_flow().0,
                        sim.water.stats().transferred,
                    ));
                }
            }
            assert_eq!(sim.water.stats().units, sim.water.stats().initial_units);
            assert!(sim.matter.economy.ledger.balanced());
            if let Some(expected) = &reference {
                assert_eq!(&checkpoints, expected, "repeat diverged: {id} run {run}");
            } else {
                reference = Some(checkpoints.clone());
            }
            println!("scene={id} run={run} checkpoints={checkpoints:?}");
            for (phase, mut v) in samples {
                let mean = v.iter().sum::<u128>() / v.len() as u128;
                v.sort_unstable();
                if phase == "TOTAL" || mean > 10_000 {
                    println!(
                        "scene={id} run={run} phase={phase} mean_us={} p95_us={} max_us={}",
                        mean / 1000,
                        v[(v.len() - 1) * 95 / 100] / 1000,
                        v[v.len() - 1] / 1000
                    );
                }
            }
        }
    }
}
