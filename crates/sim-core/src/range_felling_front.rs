//! Ordered (position, seed) frontier. Pending hull contacts share spatial
//! bitmap words; nearby propagation seeds share a bounded inline word.
use std::collections::{BTreeMap, BTreeSet};

type Pair = (u64, u64);

#[derive(Clone, Default)]
pub(super) struct Front {
    words: BTreeMap<u64, u64>,
    contacts: usize,
    nearby: BTreeMap<u64, u128>,
    nearby_len: usize,
    branches: BTreeSet<Pair>,
    #[cfg(test)]
    reference: Option<BTreeSet<Pair>>,
}

impl Front {
    pub fn len(&self) -> usize {
        #[cfg(test)]
        if let Some(reference) = &self.reference {
            return reference.len();
        }
        self.contacts + self.nearby_len + self.branches.len()
    }
    pub fn words(&self) -> usize {
        self.words.len() + self.nearby.len() + self.branches.len()
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    fn word(p: u64) -> (u64, u64) {
        (p >> 6, 1 << (p & 63))
    }
    // Storage only, independent of the configured propagation radius. Eight
    // sorted nonzero u16 lanes fit one word. Far seeds and a ninth nearby
    // seed retain their full identity in `branches`; nothing is discarded.
    fn relative((p, seed): Pair) -> Option<u16> {
        if (p | seed) >> 63 != 0 {
            return None;
        }
        let delta = std::array::from_fn::<_, 3, _>(|a| {
            let shift = [42, 21, 0][a];
            ((seed >> shift) & super::FELL_MASK) as i32 - ((p >> shift) & super::FELL_MASK) as i32
        });
        if delta.iter().any(|&v| !(-16..16).contains(&v)) {
            return None;
        }
        Some(
            delta
                .into_iter()
                .fold(0u16, |key, v| (key << 5) | (v + 16) as u16)
                + 1,
        )
    }
    fn absolute(p: u64, relative: u16) -> Pair {
        let code = relative - 1;
        let cell = super::fell_cell(p);
        let seed = std::array::from_fn(|a| cell[a] + i32::from((code >> [10, 5, 0][a]) & 31) - 16);
        (p, super::fell_key(seed))
    }
    fn lane(word: u128, index: usize) -> u16 {
        (word >> (index * 16)) as u16
    }
    fn locate(word: u128, code: u16) -> (usize, bool) {
        for i in 0..8 {
            let lane = Self::lane(word, i);
            if lane == 0 || lane >= code {
                return (i, lane == code);
            }
        }
        (8, false)
    }
    pub fn contains(&self, pair: &Pair) -> bool {
        #[cfg(test)]
        if let Some(reference) = &self.reference {
            return reference.contains(pair);
        }
        if pair.0 != pair.1 {
            if let Some(code) = Self::relative(*pair) {
                if self
                    .nearby
                    .get(&pair.0)
                    .is_some_and(|&word| Self::locate(word, code).1)
                {
                    return true;
                }
            }
            return self.branches.contains(pair);
        }
        let (key, bit) = Self::word(pair.0);
        self.words.get(&key).is_some_and(|mask| mask & bit != 0)
    }
    pub fn insert(&mut self, pair: Pair) -> bool {
        #[cfg(test)]
        if let Some(reference) = &mut self.reference {
            return reference.insert(pair);
        }
        if pair.0 != pair.1 {
            if let Some(code) = Self::relative(pair) {
                // Overflow records stay there even when inline room opens.
                if self.branches.contains(&pair) {
                    return false;
                }
                let word = self.nearby.entry(pair.0).or_default();
                let (index, present) = Self::locate(*word, code);
                if present {
                    return false;
                }
                if Self::lane(*word, 7) == 0 {
                    let shift = index * 16;
                    let low = *word & ((1u128 << shift) - 1);
                    *word = low
                        | (u128::from(code) << shift)
                        | ((*word & !((1u128 << shift) - 1)) << 16);
                    self.nearby_len += 1;
                    return true;
                }
            }
            return self.branches.insert(pair);
        }
        let (key, bit) = Self::word(pair.0);
        let mask = self.words.entry(key).or_default();
        let fresh = *mask & bit == 0;
        *mask |= bit;
        self.contacts += usize::from(fresh);
        fresh
    }
    pub fn remove(&mut self, pair: &Pair) -> bool {
        #[cfg(test)]
        if let Some(reference) = &mut self.reference {
            return reference.remove(pair);
        }
        if pair.0 != pair.1 {
            if let Some(code) = Self::relative(*pair) {
                if let std::collections::btree_map::Entry::Occupied(mut entry) =
                    self.nearby.entry(pair.0)
                {
                    let (index, present) = Self::locate(*entry.get(), code);
                    if present {
                        let shift = index * 16;
                        let low = *entry.get() & ((1u128 << shift) - 1);
                        let high = if index == 7 {
                            0
                        } else {
                            (*entry.get() >> (shift + 16)) << shift
                        };
                        *entry.get_mut() = low | high;
                        if *entry.get() == 0 {
                            entry.remove();
                        }
                        self.nearby_len -= 1;
                        return true;
                    }
                }
            }
            return self.branches.remove(pair);
        }
        let (key, bit) = Self::word(pair.0);
        let std::collections::btree_map::Entry::Occupied(mut entry) = self.words.entry(key) else {
            return false;
        };
        if *entry.get() & bit == 0 {
            return false;
        }
        *entry.get_mut() &= !bit;
        if *entry.get() == 0 {
            entry.remove();
        }
        self.contacts -= 1;
        true
    }
    fn first_contact(&self) -> Option<Pair> {
        let (&word, &mask) = self.words.first_key_value()?;
        let p = (word << 6) | u64::from(mask.trailing_zeros());
        Some((p, p))
    }
    pub fn first(&self) -> Option<Pair> {
        #[cfg(test)]
        if let Some(reference) = &self.reference {
            return reference.first().copied();
        }
        [
            self.first_contact(),
            self.nearby
                .first_key_value()
                .map(|(&p, &word)| Self::absolute(p, word as u16)),
            self.branches.first().copied(),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    pub fn pop_first(&mut self) -> Option<Pair> {
        #[cfg(test)]
        if let Some(reference) = &mut self.reference {
            return reference.pop_first();
        }
        let pair = self.first()?;
        if pair.0 != pair.1 {
            if let Some(mut entry) = self.nearby.first_entry() {
                if Self::absolute(*entry.key(), *entry.get() as u16) == pair {
                    *entry.get_mut() >>= 16;
                    if *entry.get() == 0 {
                        entry.remove();
                    }
                    self.nearby_len -= 1;
                    return Some(pair);
                }
            }
            return self.branches.pop_first();
        }
        let mut entry = self.words.first_entry().expect("contact head");
        let mask = entry.get_mut();
        *mask &= *mask - 1;
        if *mask == 0 {
            entry.remove();
        }
        self.contacts -= 1;
        Some(pair)
    }
    pub fn iter(&self) -> impl Iterator<Item = Pair> + '_ {
        let mut words = self.words.iter();
        let (mut key, mut mask) = (0, 0u64);
        let mut contacts = std::iter::from_fn(move || {
            if mask == 0 {
                let (&next_key, &next_mask) = words.next()?;
                key = next_key;
                mask = next_mask;
            }
            let p = (key << 6) | u64::from(mask.trailing_zeros());
            mask &= mask - 1;
            Some((p, p))
        })
        .peekable();
        let mut branches = self.branches.iter().copied().peekable();
        let mut nearby = self
            .nearby
            .iter()
            .flat_map(|(&p, &word)| {
                (0..8)
                    .map(move |i| Self::lane(word, i))
                    .take_while(|&code| code != 0)
                    .map(move |code| Self::absolute(p, code))
            })
            .peekable();
        #[cfg(test)]
        let mut reference = self.reference.as_ref().map(|r| r.iter());
        std::iter::from_fn(move || {
            #[cfg(test)]
            if let Some(reference) = &mut reference {
                return reference.next().copied();
            }
            let next = [contacts.peek(), nearby.peek(), branches.peek()]
                .into_iter()
                .flatten()
                .min()
                .copied()?;
            if contacts.peek() == Some(&next) {
                contacts.next()
            } else if nearby.peek() == Some(&next) {
                nearby.next()
            } else {
                branches.next()
            }
        })
    }
    #[cfg(test)]
    pub fn reference(pairs: impl IntoIterator<Item = Pair>) -> Self {
        let mut reference = BTreeSet::new();
        for pair in pairs {
            reference.insert(pair);
        }
        Self {
            reference: Some(reference),
            ..Self::default()
        }
    }
}
impl FromIterator<Pair> for Front {
    fn from_iter<T: IntoIterator<Item = Pair>>(iter: T) -> Self {
        let mut result = Self::default();
        for pair in iter {
            result.insert(pair);
        }
        result
    }
}
#[cfg(test)]
#[path = "range_felling_front_tests.rs"]
mod tests;
