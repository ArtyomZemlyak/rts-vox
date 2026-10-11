//! Snapshot allocation/serialization, validated restore and destruction costs.
//! File IO and hashes are outside measured intervals. No simulation changes.
use modding::explosion::ExplosionConfig;
use sim_core::blast::BlastRange;
use std::{collections::BTreeMap, hint::black_box, time::Instant};

fn report(name: &str, mut values: Vec<u128>) {
    values.sort_unstable();
    let n = values.len();
    println!(
        "operation={name} n={n} mean_ms={:.6} p50_ms={:.6} p99_ms={:.6} max_ms={:.6}",
        values.iter().sum::<u128>() as f64 / n as f64 / 1e6,
        values[n / 2] as f64 / 1e6,
        values[n * 99 / 100] as f64 / 1e6,
        values[n - 1] as f64 / 1e6
    );
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("snapshot path, optional advance ticks, rounds");
    let advance: u32 = args.get(2).map_or(0, |v| v.parse().unwrap());
    let rounds: usize = args.get(3).map_or(8, |v| v.parse().unwrap());
    assert!((1..=100).contains(&rounds));
    let config = ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron")).unwrap();
    let input = std::fs::read(path).unwrap();
    let mut world = BlastRange::restore(config, &input).expect("input snapshot");
    assert!(world.set_physics_workers(4));
    assert!(world.set_navigation_workers(8));
    for _ in 0..advance {
        world.step();
    }
    let expected = world.state_hash();
    let bytes = world.snapshot();
    // Existing pure progress callbacks delimit entered decoder sections.
    // Keep this diagnostic pass outside the uninstrumented operation samples.
    let mut sections: BTreeMap<&str, (u32, u128)> = BTreeMap::new();
    let mut section = "prefix";
    let mut mark = Instant::now();
    let restored = BlastRange::restore_observed(config, &bytes, |name, _, _| {
        let now = Instant::now();
        let entry = sections.entry(section).or_default();
        entry.0 += 1;
        entry.1 += now.duration_since(mark).as_nanos();
        section = name;
        mark = now;
    })
    .expect("warm restore");
    let entry = sections.entry(section).or_default();
    entry.0 += 1;
    entry.1 += mark.elapsed().as_nanos();
    assert_eq!(expected, restored.state_hash());
    drop(restored);
    let mut saves = Vec::with_capacity(rounds);
    let mut loads = Vec::with_capacity(rounds);
    let mut drops = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        let saved = black_box(world.snapshot());
        saves.push(start.elapsed().as_nanos());
        assert_eq!(saved, bytes);
        let start = Instant::now();
        let restored = black_box(BlastRange::restore(config, black_box(&saved)).unwrap());
        loads.push(start.elapsed().as_nanos());
        assert_eq!(restored.state_hash(), expected);
        let start = Instant::now();
        drop(restored);
        drops.push(start.elapsed().as_nanos());
    }
    assert_eq!(world.state_hash(), expected);
    println!(
        "tick={} snapshot_bytes={} hash={expected:016x} advance={advance}",
        world.tick,
        bytes.len()
    );
    for (name, (calls, ns)) in sections {
        println!(
            "warm_restore_section={name} calls={calls} total_ms={:.6}",
            ns as f64 / 1e6
        );
    }
    report("snapshot", saves);
    report("restore", loads);
    report("drop_restored_world", drops);
}
