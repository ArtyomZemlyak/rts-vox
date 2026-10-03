//! Exact occupancy union. Actors prepare sorted 64-cell words independently;
//! the caller merges masks rather than sorting every duplicate voxel again.
use std::{cmp::Reverse, collections::BinaryHeap};

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
