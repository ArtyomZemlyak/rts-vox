//! Derived exact-input memo for changes between successive join inputs.
//! Never caches world support, physics admission, or an approximate fingerprint.
use super::*;
pub(super) type Key = ([i32; 3], Mat, Option<[u8; 3]>, u16);
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Input {
    pub key: Key,
    pub index: usize,
    pub id: u64,
    pub geometry: fragment_join::Geometry,
}
#[derive(Default)]
pub(super) struct Memo {
    previous: Vec<Input>,
    pub current: Vec<Input>,
    valid: bool,
    #[cfg(test)]
    pub hits: u64,
}
#[allow(dead_code)]
impl Memo {
    pub fn can_update(&self) -> bool {
        self.valid
    }
    pub fn start_full(&mut self) {
        self.current.clear();
    }
    pub fn start_update(&mut self) {
        assert!(self.can_update());
        self.current.clone_from(&self.previous);
    }
    pub fn update(&mut self, input: Input) {
        match self.current.binary_search_by_key(&input.id, |input| input.id) {
            Ok(index) => self.current[index] = input,
            Err(index) => self.current.insert(index, input),
        }
    }
    pub fn remove(&mut self, id: u64) {
        if let Ok(index) = self.current.binary_search_by_key(&id, |input| input.id) {
            self.current.remove(index);
        }
    }
    pub fn invalidate(&mut self) {
        self.valid = false;
    }
    pub fn push(
        &mut self,
        index: usize,
        key: Key,
        f: &Fragment,
        geometry: fragment_join::Geometry,
    ) {
        self.current.push(Input {
            key,
            index,
            id: f.id,
            geometry,
        });
    }
    pub fn changed(&mut self) -> Option<BTreeSet<Key>> {
        if !self.valid {
            return None;
        }
        let sorted = self.previous.windows(2).all(|pair| pair[0].id < pair[1].id)
            && self.current.windows(2).all(|pair| pair[0].id < pair[1].id);
        let changed = if sorted {
            self.changed_sorted()
        } else {
            self.changed_hash()
        };
        #[cfg(test)]
        if changed.is_empty() {
            self.hits = self.hits.wrapping_add(1);
        }
        Some(changed)
    }
    fn changed_sorted(&self) -> BTreeSet<Key> {
        let mut changed = BTreeSet::new();
        let (mut old, mut new) = (0, 0);
        while old < self.previous.len() || new < self.current.len() {
            if new == self.current.len()
                || (old < self.previous.len() && self.previous[old].id < self.current[new].id)
            {
                changed.insert(self.previous[old].key);
                old += 1;
            } else if old == self.previous.len() || self.previous[old].id > self.current[new].id {
                changed.insert(self.current[new].key);
                new += 1;
            } else {
                if self.previous[old].key != self.current[new].key {
                    changed.insert(self.previous[old].key);
                    changed.insert(self.current[new].key);
                }
                if self.previous[old].geometry != self.current[new].geometry {
                    changed.insert(self.current[new].key);
                }
                old += 1;
                new += 1;
            }
        }
        changed
    }
    fn changed_hash(&self) -> BTreeSet<Key> {
        let mut previous_by_id: FxLookup<u64, (usize, Key, fragment_join::Geometry)> =
            FxLookup::with_capacity(self.previous.len());
        for (index, input) in self.previous.iter().enumerate() {
            previous_by_id.insert(input.id, (index, input.key, input.geometry));
        }
        let mut matched = vec![false; self.previous.len()];
        let mut changed = BTreeSet::new();
        for input in &self.current {
            if let Some((index, key, geometry)) = previous_by_id.get(&input.id) {
                matched[*index] = true;
                if *key != input.key {
                    changed.insert(*key);
                    changed.insert(input.key);
                }
                if *geometry != input.geometry {
                    changed.insert(input.key);
                }
            } else {
                changed.insert(input.key);
            }
        }
        for (index, input) in self.previous.iter().enumerate() {
            if !matched[index] {
                changed.insert(input.key);
            }
        }
        changed
    }
    pub fn finish(&mut self) {
        // Transfers are visible as removed donors and changed heads in the next
        // input, which invalidates exactly their old and new keys.
        self.valid = true;
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(index: usize, id: u64, x: i32) -> Input {
        let fragment = Fragment {
            id,
            position: V3::ZERO,
            velocity: V3::ZERO,
            material: 2,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1000,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: 1,
            resting: true,
        };
        Input {
            key: ([x, 0, 0], 2, None, 100),
            index,
            id,
            geometry: fragment_join::Geometry::of(&fragment).unwrap(),
        }
    }

    #[test]
    fn sorted_hash_and_unsorted_changes_match() {
        let previous = vec![input(0, 1, 0), input(1, 2, 1), input(2, 3, 2)];
        let current = vec![input(0, 1, 0), input(1, 2, 4), input(2, 4, 2)];
        let memo = Memo {
            previous: previous.clone(),
            current: current.clone(),
            valid: true,
            ..Default::default()
        };
        let expected = memo.changed_hash();
        assert_eq!(memo.changed_sorted(), expected);
        let mut unsorted = Memo {
            previous: previous.into_iter().rev().collect(),
            current,
            valid: true,
            ..Default::default()
        };
        assert_eq!(unsorted.changed().unwrap(), expected);
    }
}
