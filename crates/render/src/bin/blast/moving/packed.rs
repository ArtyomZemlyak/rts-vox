//! Exact occupancy union. Actors prepare sorted 64-cell words independently;
//! the caller merges masks rather than sorting every duplicate voxel again.
use std::{cmp::Reverse, collections::BinaryHeap};

#[derive(Default)]
pub(super) struct Scratch {
    parts: Vec<Vec<usize>>,
}

/// Split by input work, rather than by world distance: formations can occupy
/// a small corner of a large window. A word belongs to exactly one job, so OR
/// and enumeration preserve the serial union's ascending order.
fn boundaries(actors: &[&[(usize, u64)]], count: usize) -> Vec<usize> {
    let total: usize = actors.iter().map(|words| words.len()).sum();
    let end = actors
        .iter()
        .filter_map(|words| words.last().map(|&(word, _)| word + 1))
        .max()
        .unwrap_or(0);
    let mut bounds = vec![0];
    for part in 1..count {
        let target = total / count * part + total % count * part / count;
        let (mut low, mut high) = (*bounds.last().unwrap(), end);
        while low < high {
            let middle = low + (high - low) / 2;
            let prefix: usize = actors
                .iter()
                .map(|words| words.partition_point(|&(word, _)| word < middle))
                .sum();
            if prefix < target {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        bounds.push(low);
    }
    bounds.push(end);
    bounds
}

pub(super) fn union_in<'a>(
    actors: impl Iterator<Item = &'a [(usize, u64)]>,
    out: &mut Vec<usize>,
    pool: Option<&bevy::tasks::TaskPool>,
    scratch: &mut Scratch,
) {
    let actors: Vec<_> = actors.collect();
    let count: usize = actors.iter().map(|words| words.len()).sum();
    if let Some(pool) = pool.filter(|pool| pool.thread_num() >= 4 && count >= 32_768) {
        parallel_union(&actors, out, pool, scratch);
    } else {
        union(actors.into_iter(), out);
    }
}

fn parallel_union(
    actors: &[&[(usize, u64)]],
    out: &mut Vec<usize>,
    pool: &bevy::tasks::TaskPool,
    scratch: &mut Scratch,
) {
    // Four leaves bound scheduling/copy overhead even on larger shared pools.
    let bounds = boundaries(actors, pool.thread_num().clamp(1, 4));
    scratch.parts.resize_with(bounds.len() - 1, Vec::new);
    pool.scope(|scope| {
        for (range, result) in bounds.windows(2).zip(&mut scratch.parts) {
            scope.spawn(async move {
                result.clear();
                union(
                    actors.iter().map(|words| {
                        let start = words.partition_point(|&(word, _)| word < range[0]);
                        let end = words.partition_point(|&(word, _)| word < range[1]);
                        &words[start..end]
                    }),
                    result,
                );
            });
        }
    });
    // ECS/GPU publication stays on the caller, in the original cell order.
    for part in &scratch.parts {
        out.extend_from_slice(part);
    }
}

pub(super) fn pack(cells: &mut Vec<usize>, words: &mut Vec<(usize, u64)>) {
    cells.sort_unstable();
    cells.dedup();
    words.clear();
    for &cell in cells.iter() {
        let index = cell / 64;
        let bit = 1u64 << (cell % 64);
        if let Some((_, mask)) = words.last_mut().filter(|(last, _)| *last == index) {
            *mask |= bit;
        } else {
            words.push((index, bit));
        }
    }
}

pub(super) fn union<'a>(actors: impl Iterator<Item = &'a [(usize, u64)]>, out: &mut Vec<usize>) {
    let actors: Vec<_> = actors.collect();
    let mut heads = BinaryHeap::with_capacity(actors.len());
    for (actor, words) in actors.iter().enumerate() {
        if let Some(&(word, _)) = words.first() {
            heads.push(Reverse((word, actor, 0)));
        }
    }
    let mut current = None;
    let mut bits = 0;
    let emit = |word: usize, mut bits: u64, out: &mut Vec<usize>| {
        while bits != 0 {
            let bit = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            out.push(word * 64 + bit);
        }
    };
    while let Some(Reverse((word, actor, cursor))) = heads.pop() {
        if current != Some(word) {
            if let Some(previous) = current {
                emit(previous, bits, out);
            }
            current = Some(word);
            bits = 0;
        }
        bits |= actors[actor][cursor].1;
        if let Some(&(next, _)) = actors[actor].get(cursor + 1) {
            heads.push(Reverse((next, actor, cursor + 1)));
        }
    }
    if let Some(word) = current {
        emit(word, bits, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_jobs_match_serial_for_skew_overlap_reuse_and_address_edges() {
        for workers in [1, 4, 8] {
            let pool = bevy::tasks::TaskPoolBuilder::new()
                .num_threads(workers)
                .build();
            let mut scratch = Scratch::default();
            for shape in 0..5 {
                let mut words = vec![Vec::new(); 60];
                for (actor, words) in words.iter_mut().enumerate() {
                    for i in 0..(if shape == 4 { 3 } else { 1024 }) {
                        let word = match shape {
                            0 => actor * 2048 + i, // disjoint formations
                            1 => i,                // every formation overlaps
                            2 => i + usize::from(actor == 59) * 1_000_000_000,
                            3 => usize::MAX / 64 - 2048 + i, // valid last word
                            _ => actor + i,
                        };
                        words.push((
                            word,
                            if actor % 3 == 0 {
                                u64::MAX
                            } else {
                                1 << (i % 64)
                            },
                        ));
                    }
                }
                // Empty actors, and an empty entire roster on the next call.
                words[7].clear();
                let actors: Vec<_> = words.iter().map(Vec::as_slice).collect();
                let mut expected = vec![123];
                union(actors.iter().copied(), &mut expected);
                let mut actual = vec![123];
                parallel_union(&actors, &mut actual, &pool, &mut scratch);
                assert_eq!(actual, expected, "workers={workers} shape={shape}");
                let bounds = boundaries(&actors, workers.clamp(1, 4));
                assert!(bounds.windows(2).all(|b| b[0] <= b[1]));
                let mut gated = vec![123];
                union_in(
                    actors.iter().copied(),
                    &mut gated,
                    Some(&pool),
                    &mut scratch,
                );
                assert_eq!(gated, expected);
                let mut empty = Vec::new();
                parallel_union(&[], &mut empty, &pool, &mut scratch);
                assert!(empty.is_empty());
            }
        }
    }

    #[test]
    fn range_jobs_complete_in_a_nested_pool_and_do_not_split_shared_words() {
        for workers in [1, 4] {
            let pool = bevy::tasks::TaskPoolBuilder::new()
                .num_threads(workers)
                .build();
            let words = vec![vec![(0, 1), (usize::MAX / 64, u64::MAX)]; 60];
            let actors: Vec<_> = words.iter().map(Vec::as_slice).collect();
            let mut expected = Vec::new();
            union(actors.iter().copied(), &mut expected);
            assert_eq!(
                pool.scope(|scope| {
                    let pool = &pool;
                    scope.spawn(async move {
                        let mut actual = Vec::new();
                        let mut scratch = Scratch::default();
                        parallel_union(&actors, &mut actual, pool, &mut scratch);
                        actual
                    });
                }),
                vec![expected]
            );
        }
    }
    #[test]
    fn word_boundaries_and_full_usize_addresses_do_not_alias() {
        let mut a = vec![0, 63, 64, 127, 128, usize::MAX - 1, usize::MAX];
        let mut b = vec![64, usize::MAX, usize::MAX / 2];
        let mut expected: Vec<_> = a.iter().chain(&b).copied().collect();
        expected.sort_unstable();
        expected.dedup();
        let (mut wa, mut wb) = (Vec::new(), Vec::new());
        pack(&mut a, &mut wa);
        pack(&mut b, &mut wb);
        let mut result = Vec::new();
        union([wa.as_slice(), wb.as_slice()].into_iter(), &mut result);
        assert_eq!(result, expected);
    }
    #[test]
    fn overlapping_and_sparse_words_match_a_sorted_voxel_union() {
        for count in [0, 1, 3, 60] {
            for size in [0, 1, 63, 64, 65, 8193] {
                let mut actors: Vec<_> = (0..count)
                    .map(|actor| {
                        (0..size)
                            .map(|i| (i * 73 + actor * 19) % 10003)
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let mut expected: Vec<_> = actors.iter().flatten().copied().collect();
                expected.sort_unstable();
                expected.dedup();
                let mut words = vec![Vec::new(); count];
                for (cells, words) in actors.iter_mut().zip(&mut words) {
                    pack(cells, words);
                }
                let mut result = Vec::new();
                union(words.iter().map(Vec::as_slice), &mut result);
                assert_eq!(result, expected, "actors={count} size={size}");
            }
        }
    }
}
