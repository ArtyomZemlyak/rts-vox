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
            + front
                .nearby
                .values()
                .map(|&word| (0..8).filter(|&i| Front::lane(word, i) != 0).count())
                .sum::<usize>()
            + front.branches.len(),
        front.len()
    );
    assert!(front.words.values().all(|m| *m != 0));
    for &word in front.nearby.values() {
        assert_ne!(word, 0);
        let lanes: Vec<_> = (0..8)
            .map(|i| Front::lane(word, i))
            .take_while(|&v| v != 0)
            .collect();
        assert!(lanes.windows(2).all(|pair| pair[0] < pair[1]));
        if lanes.len() < 8 {
            assert_eq!(word >> (lanes.len() * 16), 0);
        }
    }
}

#[test]
fn relative_seeds_keep_overflow_identity_after_inline_room_opens() {
    let key = super::super::fell_key;
    for cell in [[32, 32, 32], [-32, -32, -32], [1_048_570, -1_048_570, 0]] {
        let p = key(cell);
        let pairs: Vec<_> = (-24..=24)
            .map(|z| (p, key([cell[0], cell[1], cell[2] + z])))
            .chain([-16, 15, 16].into_iter().flat_map(|x| {
                [-16, 15, 16].into_iter().map(move |y| {
                    // Keep the extreme-coordinate fixture in the saved domain.
                    let base = [32, 32, cell[2]];
                    (key(base), key([base[0] + x, base[1] + y, base[2]]))
                })
            }))
            .collect();
        let mut front = Front::default();
        let mut reference = BTreeSet::new();
        for &pair in pairs.iter().rev() {
            assert_eq!(front.insert(pair), reference.insert(pair));
            equal(&front, &reference);
        }
        assert!(!front.nearby.is_empty());
        assert!(!front.branches.is_empty());
        for &pair in pairs.iter().step_by(3) {
            assert_eq!(front.remove(&pair), reference.remove(&pair));
            equal(&front, &reference);
        }
        // Reinsert every seed after removing inline lanes, including seeds
        // still in overflow. No pair may migrate into a duplicate record.
        for &pair in &pairs {
            assert_eq!(front.insert(pair), reference.insert(pair));
            assert_eq!(front.contains(&pair), reference.contains(&pair));
            equal(&front, &reference);
        }
        let restored: Front = front.iter().collect();
        equal(&restored, &reference);
        while let Some(pair) = reference.pop_first() {
            assert_eq!(front.pop_first(), Some(pair));
            equal(&front, &reference);
        }
    }
}

#[test]
fn relative_code_roundtrips_every_lane_value_and_signed_domain_edges() {
    let key = super::super::fell_key;
    for cell in [[0, 0, 0], [-1_048_560; 3], [1_048_560; 3]] {
        let p = key(cell);
        let mut previous = None;
        for x in -16..16 {
            for y in -16..16 {
                for z in -16..16 {
                    let pair = (p, key([cell[0] + x, cell[1] + y, cell[2] + z]));
                    let code = Front::relative(pair).unwrap();
                    assert_eq!(Front::absolute(p, code), pair);
                    assert!(previous.is_none_or(|old| old < code));
                    previous = Some(code);
                }
            }
        }
        for delta in [[-17, 0, 0], [0, 16, 0], [0, 0, -17]] {
            // The storage fast path is not a radius cutoff.
            let pair = (key([0; 3]), key(delta));
            assert_eq!(Front::relative(pair), None);
            let mut front: Front = [pair].into_iter().collect();
            assert!(front.contains(&pair));
            assert_eq!(front.pop_first(), Some(pair));
        }
    }
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
        "records={} words={} queries={} contact_words={} nearby_words={} overflow={}",
        pairs.len(),
        sim.matter.felling.words(),
        queries.len(),
        sim.matter.felling.words.len(),
        sim.matter.felling.nearby.len(),
        sim.matter.felling.branches.len()
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
