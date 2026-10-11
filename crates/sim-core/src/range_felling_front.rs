//! Ordered (position, seed) frontier. Pending hull contacts share spatial
//! bitmap words; propagation records retain their full independent seeds.
use std::collections::{BTreeMap, BTreeSet};

type Pair = (u64, u64);

#[derive(Clone, Default)]
pub(super) struct Front {
    words: BTreeMap<u64, u64>,
    contacts: usize,
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
        self.contacts + self.branches.len()
    }
    pub fn words(&self) -> usize {
        self.words.len() + self.branches.len()
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    fn word(p: u64) -> (u64, u64) {
        (p >> 6, 1 << (p & 63))
    }
    pub fn contains(&self, pair: &Pair) -> bool {
        #[cfg(test)]
        if let Some(reference) = &self.reference {
            return reference.contains(pair);
        }
        if pair.0 != pair.1 {
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
        match (self.first_contact(), self.branches.first().copied()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
    pub fn pop_first(&mut self) -> Option<Pair> {
        #[cfg(test)]
        if let Some(reference) = &mut self.reference {
            return reference.pop_first();
        }
        let pair = self.first()?;
        if pair.0 != pair.1 {
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
        #[cfg(test)]
        let mut reference = self.reference.as_ref().map(|r| r.iter());
        std::iter::from_fn(move || {
            #[cfg(test)]
            if let Some(reference) = &mut reference {
                return reference.next().copied();
            }
            match (contacts.peek(), branches.peek()) {
                (Some(a), Some(b)) if a < b => contacts.next(),
                (_, Some(_)) => branches.next(),
                _ => contacts.next(),
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
