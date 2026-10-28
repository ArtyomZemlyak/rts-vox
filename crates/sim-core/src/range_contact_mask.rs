//! Exact terrain contacts for coarse pressure cells. Ordinary cells keep an
//! inline word; larger non-air cells allocate enough words for every voxel.
#[derive(Clone, Debug)]
pub(super) enum Mask {
    Small(u64),
    Wide(Box<[u64]>),
}
impl Default for Mask {
    fn default() -> Self {
        Self::Small(0)
    }
}
impl PartialEq for Mask {
    fn eq(&self, other: &Self) -> bool {
        let n = self.words().len().max(other.words().len());
        (0..n).all(|i| {
            self.words().get(i).copied().unwrap_or(0) == other.words().get(i).copied().unwrap_or(0)
        })
    }
}
impl Eq for Mask {}
impl Mask {
    pub(super) fn filled(count: usize) -> Self {
        let tail = !0u64 >> ((64 - count % 64) % 64);
        if count == 0 {
            Self::default()
        } else if count <= 64 {
            Self::Small(tail)
        } else {
            let mut words = vec![!0u64; count.div_ceil(64)];
            *words.last_mut().unwrap() = tail;
            Self::Wide(words.into_boxed_slice())
        }
    }
    pub(super) fn for_voxels(count: usize) -> Self {
        if count <= 64 {
            Self::default()
        } else {
            Self::Wide(vec![0; count.div_ceil(64)].into_boxed_slice())
        }
    }
    fn words(&self) -> &[u64] {
        match self {
            Self::Small(word) => std::slice::from_ref(word),
            Self::Wide(words) => words,
        }
    }
    pub(super) fn insert(&mut self, index: usize) {
        let (word, bit) = (index / 64, 1u64 << (index % 64));
        if let Self::Small(first) = self {
            if word == 0 {
                *first |= bit;
                return;
            }
            let mut words = vec![0; word + 1];
            words[0] = *first;
            *self = Self::Wide(words.into_boxed_slice());
        }
        if let Self::Wide(words) = self {
            if word >= words.len() {
                let mut grown = words.to_vec();
                grown.resize(word + 1, 0);
                *words = grown.into_boxed_slice();
            }
            words[word] |= bit;
        }
    }
    pub(super) fn count_ones(&self) -> u32 {
        self.words().iter().map(|w| w.count_ones()).sum()
    }
    /// Merge one occupied row into an already allocated voxel span.
    pub(super) fn insert_bits(&mut self, index: usize, bits: u64) {
        if bits == 0 {
            return;
        }
        let shift = index % 64;
        match self {
            Self::Small(word) => *word |= bits << shift,
            Self::Wide(words) => {
                let word = index / 64;
                words[word] |= bits << shift;
                if shift != 0 && bits >> (64 - shift) != 0 {
                    words[word + 1] |= bits >> (64 - shift);
                }
            }
        }
    }
    pub(super) fn rank(&self, before: usize) -> usize {
        let word = before / 64;
        let full: u32 = self.words().iter().take(word).map(|w| w.count_ones()).sum();
        let tail = self.words().get(word).copied().unwrap_or(0) & ((1u64 << (before % 64)) - 1);
        full as usize + tail.count_ones() as usize
    }
    /// Number of occupied cells in a half-open linear interval.
    pub(super) fn count_range(&self, from: usize, until: usize) -> u32 {
        if from >= until {
            return 0;
        }
        let first = from / 64;
        let last = (until - 1) / 64;
        (first..=last)
            .map(|word| {
                let lo = if word == first { from % 64 } else { 0 };
                let hi = if word == last {
                    (until - 1) % 64 + 1
                } else {
                    64
                };
                let bits = self.words().get(word).copied().unwrap_or(0);
                (bits & (!0u64 << lo) & (!0u64 >> (64 - hi))).count_ones()
            })
            .sum()
    }
    /// Material ordinal and subcell index, each once in canonical order.
    /// A resumed wide cell counts its prefix once, not again per voxel.
    pub(super) fn ranked_from(&self, from: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        let first = self.rank(from);
        self.indices_from(from)
            .enumerate()
            .map(move |(rank, cell)| (first + rank, cell))
    }
    pub(super) fn indices_from(&self, from: usize) -> impl Iterator<Item = usize> + '_ {
        self.words()
            .iter()
            .enumerate()
            .skip(from / 64)
            .flat_map(move |(word, &bits)| {
                let bits = if word == from / 64 {
                    bits & (!0u64 << (from % 64))
                } else {
                    bits
                };
                super::BitIter(bits).map(move |bit| word * 64 + bit)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interval_counts_match_independent_bits_across_words() {
        let mut mask = Mask::for_voxels(4096);
        let bits: Vec<_> = (0..4096).filter(|i| i % 3 == 0 || i % 67 == 0).collect();
        for &i in &bits {
            mask.insert(i);
        }
        for from in [0, 1, 63, 64, 65, 127, 511, 728, 4095, 4096] {
            for len in [0, 1, 16, 64, 729, 4096] {
                let until = (from + len).min(4096);
                assert_eq!(
                    mask.count_range(from, until) as usize,
                    bits.iter().filter(|&&i| i >= from && i < until).count()
                );
            }
        }
    }

    #[test]
    fn ranked_resume_matches_independent_bit_scan_for_dense_and_holey_masks() {
        for count in [64, 729, 4096] {
            for stride in [1, 3, 67] {
                let expected: Vec<_> = (0..count).filter(|i| i % stride == 0).collect();
                let mut mask = Mask::for_voxels(count);
                for &i in &expected {
                    mask.insert(i);
                }
                for from in [0, 1, 63, 64, 65, 128, 511, 728, 4095, 4096] {
                    let reference: Vec<_> = expected
                        .iter()
                        .copied()
                        .enumerate()
                        .filter(|&(_, cell)| cell >= from)
                        .collect();
                    assert_eq!(
                        mask.ranked_from(from).collect::<Vec<_>>(),
                        reference,
                        "count={count} stride={stride} from={from}"
                    );
                }
            }
        }
    }

    #[test]
    fn packed_rows_match_scalar_insertion_including_clipped_and_cross_word_rows() {
        for edge in [1usize, 4, 9, 16, 64] {
            let mut packed = Mask::for_voxels(edge.pow(3));
            let mut scalar = packed.clone();
            for z in 0..edge {
                for y in 0..edge {
                    let skip = (y + z) % edge;
                    let width = edge - skip;
                    let bits = 0xa5a5_f0f0_1234_ffffu64 >> (64 - width);
                    let index = skip + edge * (y + edge * z);
                    packed.insert_bits(index, bits);
                    for bit in 0..width {
                        if bits & (1u64 << bit) != 0 {
                            scalar.insert(index + bit);
                        }
                    }
                }
            }
            assert_eq!(packed, scalar, "edge={edge}");
            assert_eq!(packed.count_ones(), scalar.count_ones());
        }
    }
    #[test]
    fn exact_wide_mask_iteration_ranks_and_resumption_cross_word_boundaries() {
        let indices = [0, 3, 63, 64, 65, 127, 128, 511, 512, 728, 1727, 4095];
        let mut mask = Mask::default();
        for &index in &indices {
            mask.insert(index);
            mask.insert(index);
        }
        assert_eq!(mask.count_ones() as usize, indices.len());
        for from in [0, 1, 63, 64, 65, 512, 729, 4095, 4096] {
            assert_eq!(
                mask.indices_from(from).collect::<Vec<_>>(),
                indices
                    .into_iter()
                    .filter(|&i| i >= from)
                    .collect::<Vec<_>>()
            );
        }
        for (rank, &index) in indices.iter().enumerate() {
            assert_eq!(mask.rank(index), rank);
        }
        let mut narrow = Mask::default();
        narrow.insert(63);
        assert!(matches!(narrow, Mask::Small(_)));
    }
}
