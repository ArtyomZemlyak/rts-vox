use super::*;
use std::collections::BTreeSet;

fn equal(front: &Front, reference: &BTreeSet<Pair>) {
    assert_eq!(front.len(), reference.len());
    assert_eq!(front.first(), reference.first().copied());
    assert_eq!(
        front.iter().collect::<Vec<_>>(),
        reference.iter().copied().collect::<Vec<_>>()
    );
    assert_eq!(
        front
            .words
            .values()
            .map(|m| m.count_ones() as usize)
            .sum::<usize>()
            + front.branches.len(),
        front.len()
    );
    assert!(front.words.values().all(|m| *m != 0));
}

#[test]
fn word_front_keeps_every_seed_and_order_across_bit_boundaries() {
    let pairs: Vec<_> = [0, 1, 63, 64, u64::MAX]
        .into_iter()
        .flat_map(|p| {
            [0, 1, 62, 63, 64, 65, 127, 128, u64::MAX - 1, u64::MAX]
                .into_iter()
                .map(move |seed| (p, seed))
        })
        .collect();
    let mut reference = BTreeSet::new();
    let mut front = Front::default();
    for &pair in pairs.iter().rev().chain(pairs.iter()) {
        assert_eq!(front.insert(pair), reference.insert(pair));
        equal(&front, &reference);
    }
    for &pair in pairs.iter().step_by(2).chain(pairs.iter().step_by(2)) {
        assert_eq!(front.remove(&pair), reference.remove(&pair));
        equal(&front, &reference);
    }
    while let Some(pair) = reference.pop_first() {
        assert_eq!(front.pop_first(), Some(pair));
        // Refused work is still present with precisely the same identity/order.
        assert!(front.insert(pair));
        assert_eq!(front.pop_first(), Some(pair));
        equal(&front, &reference);
    }
    assert_eq!(front.pop_first(), None);
    assert_eq!(front.words(), 0);
}

#[test]
fn word_front_matches_set_through_revisits_removals_and_clear() {
    let mut front = Front::default();
    let mut reference = BTreeSet::new();
    let mut rng = 32847u64;
    for step in 0..30_000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let p = (rng >> 32) % 256;
        let seed = if step % 3 == 0 {
            p
        } else {
            ((rng >> 12) % 256) | ((rng & 1) << 63)
        };
        let pair = (p, seed);
        match (rng >> 8) % 5 {
            0 | 1 => assert_eq!(front.insert(pair), reference.insert(pair)),
            2 => assert_eq!(front.remove(&pair), reference.remove(&pair)),
            3 => assert_eq!(front.pop_first(), reference.pop_first()),
            _ => assert_eq!(front.contains(&pair), reference.contains(&pair)),
        }
        if step % 127 == 0 {
            equal(&front, &reference);
        }
        if step % 7919 == 0 {
            front.clear();
            reference.clear();
            equal(&front, &reference);
        }
    }
    equal(&front, &reference);
}

/// Build/query/drain the exact saved frontier, independent of simulation work.
/// Run alone in release with FELL_BENCH_SNAPSHOT pointing at a battle snapshot.
#[test]
#[ignore = "manual saved-frontier measurement; run alone in release"]
fn measure_saved_felling_front() {
    use std::{hint::black_box, time::Instant};
    let path = std::env::var("FELL_BENCH_SNAPSHOT").expect("snapshot path");
    let config =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let snapshot = std::fs::read(path).unwrap();
    let sim = crate::blast::BlastRange::restore(config, &snapshot).unwrap();
    assert_eq!(sim.snapshot(), snapshot, "legacy snapshot bytes changed");
    let pairs: Vec<_> = sim.matter.felling.iter().collect();
    if let Ok(path) = std::env::var("FELL_BENCH_PAIRS") {
        let bytes: Vec<_> = pairs
            .iter()
            .flat_map(|&(p, seed)| p.to_le_bytes().into_iter().chain(seed.to_le_bytes()))
            .collect();
        std::fs::write(path, bytes).unwrap();
    }
    let queries: Vec<_> = pairs.iter().step_by(3).map(|&(p, _)| (p, p)).collect();
    println!(
        "records={} words={} queries={}",
        pairs.len(),
        sim.matter.felling.words(),
        queries.len()
    );
    for run in [false, true, true, false] {
        let start = Instant::now();
        let mut front = if run {
            pairs.iter().copied().collect::<Front>()
        } else {
            Front::reference(pairs.iter().copied())
        };
        let build = start.elapsed().as_micros();
        let start = Instant::now();
        for _ in 0..8 {
            for pair in &queries {
                black_box(front.contains(black_box(pair)));
            }
        }
        let query = start.elapsed().as_micros();
        let start = Instant::now();
        for _ in 0..8 {
            for &pair in &queries {
                black_box(front.insert(black_box(pair)));
            }
        }
        let insert = start.elapsed().as_micros();
        let start = Instant::now();
        let mut count = 0;
        while let Some(pair) = front.pop_first() {
            black_box(pair);
            count += 1;
        }
        let drain = start.elapsed().as_micros();
        assert_eq!(front.len(), 0);
        println!("packed={run} build_us={build} query_us={query} insert_us={insert} drain_us={drain} drained={count}");
    }
}
