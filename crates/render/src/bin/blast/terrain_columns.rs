//! Temporary duplicate filter for distant-column publication, never world state.
#[derive(Clone, Copy, Default)]
struct Row {
    key: Option<[u32; 2]>,
    bits: u64,
}

pub(super) struct Columns([Row; 64]);
impl Default for Columns {
    fn default() -> Self {
        Self([Row::default(); 64])
    }
}
impl Columns {
    /// False only for a column already seen in this batch. Full keys prevent
    /// a direct-map collision from hiding an edit; a collision may revisit it.
    pub(super) fn visit(&mut self, p: [u32; 2]) -> bool {
        let key = [p[0], p[1] / 64];
        let hash = key[0].wrapping_mul(73856093) ^ key[1].wrapping_mul(19349663);
        let row = &mut self.0[hash as usize & 63];
        if row.key != Some(key) {
            row.key = Some(key);
            row.bits = 0;
        }
        let bit = 1u64 << (p[1] % 64);
        let first = row.bits & bit == 0;
        row.bits |= bit;
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn duplicate_filter_preserves_every_column_through_collisions_and_new_batches() {
        let mut expected = BTreeSet::from([[7, 9], [u32::MAX, 0]]);
        let mut actual = expected.clone();
        for batch in 0u32..4 {
            let mut filter = Columns::default();
            for i in 0u32..100000 {
                let p = if i % 7 == 0 {
                    [u32::MAX - i % 128, u32::MAX - i % 256]
                } else {
                    [(i / 4096 + batch) % 129, (i % 192) + (i / 8192) * 64]
                };
                expected.insert(p);
                if filter.visit(p) {
                    actual.insert(p);
                }
            }
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn repeated_vertical_edits_need_one_visit_per_column() {
        let mut filter = Columns::default();
        let mut visits = 0;
        for x in 0..128 {
            for _y in 0..64 {
                for z in 0..192 {
                    visits += usize::from(filter.visit([x, z]));
                }
            }
        }
        assert_eq!(visits, 128 * 192);
    }
}
