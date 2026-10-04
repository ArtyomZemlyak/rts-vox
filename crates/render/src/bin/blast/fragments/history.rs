//! Tick origins in physical order. Normal parallel leaves merge sorted IDs;
//! reordered imports retain the former last-write-wins lookup semantics.
use super::{HashMap, Vec3};

#[derive(Default)]
pub(crate) struct History {
    positions: Vec<(u64, Vec3)>,
    ordered: bool,
    imported: HashMap<u64, Vec3>,
}
impl History {
    pub fn capture(&mut self, positions: impl IntoIterator<Item = (u64, Vec3)>) {
        self.positions.clear();
        self.positions.extend(positions);
        self.ordered = self.positions.windows(2).all(|w| w[0].0 < w[1].0);
        self.imported.clear();
        if !self.ordered {
            self.imported.extend(self.positions.iter().copied());
        }
    }
    pub fn get(&self, id: &u64) -> Option<&Vec3> {
        if self.ordered {
            self.positions
                .binary_search_by_key(id, |p| p.0)
                .ok()
                .map(|i| &self.positions[i].1)
        } else {
            self.imported.get(id)
        }
    }
    pub fn cursor(&self, first: u64, ordered: bool) -> Cursor<'_> {
        let merge = self.ordered && ordered;
        Cursor {
            history: self,
            merge,
            index: if merge {
                self.positions.partition_point(|p| p.0 < first)
            } else {
                0
            },
        }
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        if self.ordered {
            self.positions.len()
        } else {
            self.imported.len()
        }
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
pub(crate) struct Cursor<'a> {
    history: &'a History,
    merge: bool,
    index: usize,
}
impl Cursor<'_> {
    /// IDs are increasing when constructed for an ordered leaf. A missing/new
    /// ID uses its current pose, just as the old history map did.
    pub fn next(&mut self, id: u64) -> Option<Vec3> {
        if !self.merge {
            return self.history.get(&id).copied();
        }
        let positions = &self.history.positions;
        while positions.get(self.index).is_some_and(|p| p.0 < id) {
            self.index += 1;
        }
        positions.get(self.index).filter(|p| p.0 == id).map(|p| p.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leaf_merge_matches_map_after_compaction_spawn_import_duplicates_and_reset() {
        let mut history = History::default();
        for ids in [
            vec![2, 7, 9, 12],
            vec![12, 2, 9, 7],
            vec![2, 7, 7, 12],
            vec![],
        ] {
            let positions: Vec<_> = ids
                .iter()
                .enumerate()
                .map(|(i, &id)| (id, Vec3::splat(i as f32)))
                .collect();
            let expected: HashMap<_, _> = positions.iter().copied().collect();
            history.capture(positions);
            for queries in [vec![1, 2, 3, 7, 9, 10, 12, 20], vec![12, 2, 7, 1, 20, 9]] {
                for grain in [1, 3, 2048] {
                    for leaf in queries.chunks(grain) {
                        let mut cursor =
                            history.cursor(leaf[0], leaf.windows(2).all(|w| w[0] < w[1]));
                        for &id in leaf {
                            assert_eq!(cursor.next(id), expected.get(&id).copied());
                            assert_eq!(history.get(&id), expected.get(&id));
                        }
                    }
                }
            }
        }
    }
}
