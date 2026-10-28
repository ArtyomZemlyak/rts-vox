use super::*;
use voxel::world::Edit;

fn key(e: &Edit) -> [u32; 3] {
    [e.x, e.y, e.z]
}

fn check(old: &[Edit], fresh: &[Edit]) {
    let mut backlog = CollapseBacklog::default();
    let mut expected = BTreeMap::new();
    for &e in old {
        backlog.insert(e);
        expected.insert(key(&e), e);
    }
    for &e in fresh {
        expected.insert(key(&e), e);
    }
    let got = backlog.prepare(fresh, &mut |_| {});
    assert_eq!(got, expected.into_values().collect::<Vec<_>>());
    assert_eq!(backlog.len(), 0);
    assert!(backlog.is_empty());
    assert!(backlog.cells().is_empty());
}

#[test]
fn collapse_batch_keeps_latest_edits_across_chunks_and_extreme_coordinates() {
    let old = [
        Edit::put(31, 0, 0, 2),
        Edit::put(0, 0, 32, 3),
        Edit::put(32, 31, 0, 2),
        Edit::dig(u32::MAX, 1, 0),
        Edit::put(0, u32::MAX, u32::MAX, 2),
    ];
    let mut fresh = [
        Edit::dig(31, 0, 0),
        Edit::put(u32::MAX, 1, 0, 3),
        Edit::dig(0, 0, 32),
        Edit::put(31, 0, 0, 3),
        Edit::put(1, 1, 1, 2),
        Edit::dig(1, 1, 1),
    ];
    fresh[3].by = 29;
    for (a, b) in [
        (&old[..], &fresh[..]),
        (&old, &[]),
        (&[], &fresh),
        (&[], &[]),
    ] {
        check(a, b);
    }
}

#[test]
fn collapse_batch_matches_ordered_last_writer_oracle_for_sparse_and_dense_inputs() {
    for n in [1usize, 31, 32, 257, 8193, 65539] {
        let old: Vec<_> = (0..n)
            .map(|i| Edit::put((i / 1024) as u32, (i / 32 % 32) as u32, (i % 32) as u32, 2))
            .collect();
        for stride in [1usize, 3, 79, 4096] {
            let mut fresh: Vec<_> = old
                .iter()
                .rev()
                .step_by(stride)
                .map(|e| Edit::dig(e.x, e.y, e.z))
                .collect();
            fresh.push(Edit::put(0, 0, 0, 3));
            fresh.push(Edit::put(n as u32 + 1, 1, 1, 2));
            check(&old, &fresh);
            check(&old, &fresh[..fresh.len() / 2]);
        }
    }
    let mut seed = 73u64;
    for _ in 0..24 {
        let mut edits = Vec::new();
        for i in 0..4097 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let p = [
                seed as u32 % 97,
                (seed >> 24) as u32 % 65,
                (seed >> 48) as u32 % 97,
            ];
            edits.push(if i % 3 == 0 {
                Edit::dig(p[0], p[1], p[2])
            } else {
                Edit::put(p[0], p[1], p[2], 2)
            });
        }
        check(&edits[..2048], &edits[2048..]);
    }
}

#[test]
fn collapse_batch_galloping_prefix_matches_full_linear_search_at_boundaries() {
    for n in [0usize, 1, 2, 3, 31, 32, 33, 255, 256, 257, 4097] {
        let edits: Vec<_> = (0..n).map(|i| Edit::dig(i as u32 * 3, 1, 2)).collect();
        for x in 0..n as u32 * 3 + 4 {
            for p in [[x, 0, 0], [x, 1, 2], [x, 2, 0], [u32::MAX; 3]] {
                let expected = edits.iter().take_while(|e| key(e) < p).count();
                assert_eq!(
                    collapse_prefix_before(&edits, p),
                    expected,
                    "n={n},key={p:?}"
                );
            }
        }
    }
}

#[test]
#[ignore = "manual isolated admission ABBA; not an ordinary tick or native FPS test"]
fn collapse_batch_large_admission_benchmark() {
    use std::time::Instant;
    const N: u32 = 2_476_149;
    // Frozen legacy layout and preparation, not the new drain under another
    // label. Clone/generation stays outside both preparation measurements.
    type Legacy = BTreeMap<[u32; 3], BTreeMap<[u32; 3], Edit>>;
    let mut legacy = Legacy::new();
    let mut base = CollapseBacklog::default();
    let start = Instant::now();
    for i in 0..N {
        let e = Edit::dig(i / 4096, i / 64 % 64, i % 64);
        legacy
            .entry(key(&e).map(|v| v / voxel::store::CS as u32))
            .or_default()
            .insert(key(&e), e);
    }
    let legacy_insert_us = start.elapsed().as_micros();
    let start = Instant::now();
    for i in 0..N {
        base.insert(Edit::dig(i / 4096, i / 64 % 64, i % 64));
    }
    println!(
        "admission_ingress old={N} before_us={legacy_insert_us} after_us={} single_sample=true",
        start.elapsed().as_micros()
    );
    let fresh: Vec<_> = (0..237)
        .map(|i| Edit::put(i * 3, i % 64, i % 61, 2))
        .collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    pool.install(|| {
        for repeat in 0..4 {
            let mut results = [Vec::new(), Vec::new()];
            let mut times = [0; 2];
            for which in if repeat % 2 == 0 { [0, 1] } else { [1, 0] } {
                if which == 0 {
                    let input = legacy.clone();
                    let start = Instant::now();
                    let mut all = Vec::new();
                    for cells in input.into_values() {
                        all.extend(cells.into_values());
                    }
                    all.sort_unstable_by_key(key);
                    all.extend_from_slice(&fresh);
                    results[which] = last_per_cell(&all, key).into_iter().map(|i| all[i]).collect();
                    drop(all); // The legacy caller also frees all inside admit.
                    times[which] = start.elapsed().as_micros();
                } else {
                    let mut input = base.clone();
                    let start = Instant::now();
                    results[which] = input.prepare(&fresh, &mut |_| {});
                    times[which] = start.elapsed().as_micros();
                    assert!(input.is_empty());
                }
            }
            assert_eq!(results[0], results[1]);
            println!("admission_abba repeat={repeat} old={N} fresh={} before_us={} after_us={} output={}", fresh.len(), times[0], times[1], results[0].len());
        }
    });
}

#[test]
fn collapse_bulk_ingress_keeps_last_writer_and_canonical_snapshot_order() {
    for (old_count, fresh_count) in [
        (0, 0),
        (0, 1),
        (0, 255),
        (0, 256),
        (73, 8192),
        (8192, 7),
        (8192, 256),
        (8192, 4096),
    ] {
        let mut scalar = CollapseBacklog::default();
        for i in 0..old_count {
            let e = Edit::put(i % 127, i / 127, i % 41, 3);
            scalar.insert(e);
        }
        let mut bulk = scalar.clone();
        let fresh: Vec<_> = (0..fresh_count)
            .map(|i| {
                let x = if i % 3 == 0 { u32::MAX } else { i % 127 };
                let y = if i % 5 == 0 { u32::MAX } else { i / 127 };
                let mut e = Edit::put(x, y, i % 41, if i % 2 == 0 { 2 } else { 0 });
                e.by = i;
                e
            })
            .rev()
            .chain([
                Edit::put(0, 0, 0, 2),
                Edit::dig(0, 0, 0),
                Edit::put(0, 0, 0, 7),
            ])
            .collect();
        for &e in &fresh {
            scalar.insert(e);
        }
        bulk.extend(&fresh);
        assert_eq!(scalar, bulk, "old={old_count} fresh={fresh_count}");
        assert_eq!(scalar.cells(), bulk.cells());
        assert_eq!(scalar.len(), bulk.len());
        assert_eq!(
            scalar.prepare(&fresh, &mut |_| {}),
            bulk.prepare(&fresh, &mut |_| {})
        );
    }
}
