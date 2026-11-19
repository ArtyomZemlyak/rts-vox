//! Ordered (position, seed) frontier. Pending hull contacts share spatial
//! bitmap words; nearby propagation seeds share a bounded inline word.
//! Transient verified-pristine masks avoid repeating procedural terrain reads
//! on deferred records without changing saved bytes, order, or visit quotas.
use std::collections::{BTreeMap, BTreeSet};

type Pair = (u64, u64);

#[derive(Default)]
pub(super) struct Cursor {
    synced: bool,
    word: Option<(u64, u64, u64)>,
    nearby: Option<(u64, u128, u8, u8)>,
    branch: Option<Pair>,
}

impl Cursor {
    #[inline]
    pub fn invalidate(&mut self) {
        self.synced = false;
    }
}

#[derive(Clone, Default)]
pub(super) struct Front {
    words: BTreeMap<u64, (u64, u64)>,
    contacts: usize,
    nearby: BTreeMap<u64, (u128, u8)>,
    nearby_len: usize,
    branches: BTreeSet<Pair>,
    verified_profile: Option<([u32; 3], [u16; 4], u8)>,
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
    #[inline]
    pub(super) fn has_reference(&self) -> bool {
        #[cfg(test)]
        {
            self.reference.is_some()
        }
        #[cfg(not(test))]
        {
            false
        }
    }
    /// Bind transient verified-pristine masks to `(world_dims, f.materials)`.
    /// Any change in world dimensions or yielding material set clears hints.
    pub(super) fn sync_profile(&mut self, dims: [u32; 3], materials: &[u16]) {
        let next = (materials.len() <= 4).then(|| {
            let mut mats = [0u16; 4];
            mats[..materials.len()].copy_from_slice(materials);
            mats[..materials.len()].sort_unstable();
            (dims, mats, materials.len() as u8)
        });
        if self.verified_profile != next {
            for (_, verified) in self.words.values_mut() {
                *verified = 0;
            }
            for (_, verified) in self.nearby.values_mut() {
                *verified = 0;
            }
            self.verified_profile = next;
        }
    }
    #[inline]
    pub(super) fn contact_word(&self, word_key: u64) -> u64 {
        self.words.get(&word_key).map_or(0, |&(mask, _)| mask)
    }
    #[inline]
    pub(super) fn insert_contact(&mut self, p: u64, verified_pristine: bool) -> (bool, u64) {
        #[cfg(test)]
        if let Some(reference) = &mut self.reference {
            return (reference.insert((p, p)), 0);
        }
        let (key, bit) = Self::word(p);
        let (mask, verified) = self.words.entry(key).or_default();
        let fresh = *mask & bit == 0;
        *mask |= bit;
        if verified_pristine && self.verified_profile.is_some() {
            *verified |= bit;
        }
        self.contacts += usize::from(fresh);
        (fresh, *mask)
    }
    #[inline]
    pub(super) fn mark_verified(&mut self, pair: &Pair) {
        #[cfg(test)]
        if self.reference.is_some() {
            return;
        }
        if self.verified_profile.is_none() {
            return;
        }
        if pair.0 == pair.1 {
            let (key, bit) = Self::word(pair.0);
            if let Some((mask, verified)) = self.words.get_mut(&key) {
                if *mask & bit != 0 {
                    *verified |= bit;
                }
            }
        } else if let Some(code) = Self::relative(*pair) {
            if let Some((word, verified)) = self.nearby.get_mut(&pair.0) {
                let (index, present) = Self::locate(*word, code);
                if present {
                    *verified |= 1u8 << index;
                }
            }
        }
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
                    .is_some_and(|&(word, _)| Self::locate(word, code).1)
                {
                    return true;
                }
            }
            return self.branches.contains(pair);
        }
        let (key, bit) = Self::word(pair.0);
        self.words.get(&key).is_some_and(|&(mask, _)| mask & bit != 0)
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
                let (word, verified) = self.nearby.entry(pair.0).or_default();
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
                    let v_mask = (1u8 << index) - 1;
                    *verified = (*verified & v_mask) | ((*verified & !v_mask) << 1);
                    self.nearby_len += 1;
                    return true;
                }
            }
            return self.branches.insert(pair);
        }
        self.insert_contact(pair.0, false).0
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
                    let (word, verified) = *entry.get();
                    let (index, present) = Self::locate(word, code);
                    if present {
                        let shift = index * 16;
                        let low = word & ((1u128 << shift) - 1);
                        let high = if index == 7 {
                            0
                        } else {
                            (word >> (shift + 16)) << shift
                        };
                        let next_word = low | high;
                        if next_word == 0 {
                            entry.remove();
                        } else {
                            let v_low = verified & ((1u8 << index) - 1);
                            let v_high = if index == 7 {
                                0
                            } else {
                                (verified >> (index + 1)) << index
                            };
                            *entry.get_mut() = (next_word, v_low | v_high);
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
        if entry.get().0 & bit == 0 {
            return false;
        }
        entry.get_mut().0 &= !bit;
        entry.get_mut().1 &= !bit;
        if entry.get().0 == 0 {
            entry.remove();
        }
        self.contacts -= 1;
        true
    }
    fn first_contact(&self) -> Option<Pair> {
        let (&word, &(mask, _)) = self.words.first_key_value()?;
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
                .map(|(&p, &(word, _))| Self::absolute(p, word as u16)),
            self.branches.first().copied(),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    #[cfg(test)]
    pub fn pop_first(&mut self) -> Option<Pair> {
        #[cfg(test)]
        if let Some(reference) = &mut self.reference {
            return reference.pop_first();
        }
        let pair = self.first()?;
        if pair.0 != pair.1 {
            if let Some(mut entry) = self.nearby.first_entry() {
                if Self::absolute(*entry.key(), entry.get().0 as u16) == pair {
                    entry.get_mut().0 >>= 16;
                    entry.get_mut().1 >>= 1;
                    if entry.get().0 == 0 {
                        entry.remove();
                    }
                    self.nearby_len -= 1;
                    return Some(pair);
                }
            }
            return self.branches.pop_first();
        }
        let mut entry = self.words.first_entry().expect("contact head");
        let (mask, verified) = entry.get_mut();
        let lowest = *mask & mask.wrapping_neg();
        *mask &= !lowest;
        *verified &= !lowest;
        if *mask == 0 {
            entry.remove();
        }
        self.contacts -= 1;
        Some(pair)
    }
    /// Ordered successor without walking the prefix of a million-entry
    /// frontier. Used only after a capacity refusal within an atomic phase.
    #[cfg(test)]
    pub fn after(&self, pair: Pair) -> Option<Pair> {
        use std::ops::Bound::{Excluded, Unbounded};
        #[cfg(test)]
        if let Some(reference) = &self.reference {
            return reference.range((Excluded(pair), Unbounded)).next().copied();
        }
        let contact = self
            .words
            .range((pair.0 >> 6)..)
            .find_map(|(&word, &(bits, _))| {
                let mut bits = bits;
                while bits != 0 {
                    let p = (word << 6) | u64::from(bits.trailing_zeros());
                    bits &= bits - 1;
                    if (p, p) > pair {
                        return Some((p, p));
                    }
                }
                None
            });
        let nearby = self.nearby.range(pair.0..).find_map(|(&p, &(word, _))| {
            (0..8)
                .map(|i| Self::lane(word, i))
                .take_while(|&lane| lane != 0)
                .map(|lane| Self::absolute(p, lane))
                .find(|&next| next > pair)
        });
        let branch = self
            .branches
            .range((Excluded(pair), Unbounded))
            .next()
            .copied();
        [contact, nearby, branch].into_iter().flatten().min()
    }
    fn sync_cursor(&self, after: Option<Pair>, cursor: &mut Cursor) {
        use std::ops::Bound::{Excluded, Unbounded};
        if cursor.synced {
            return;
        }
        let start_word = after.map_or(0, |prev| prev.0 >> 6);
        cursor.word = self
            .words
            .range(start_word..)
            .find_map(|(&w, &(mask, verified))| {
                let valid = if let Some(prev) = after.filter(|prev| (prev.0 >> 6) == w) {
                    let low = prev.0 & 63;
                    if prev.0 > prev.1 {
                        mask & (u64::MAX << low)
                    } else if low == 63 {
                        0
                    } else {
                        mask & (u64::MAX << (low + 1))
                    }
                } else {
                    mask
                };
                (valid != 0).then_some((w, valid, verified))
            });
        let start_p = after.map_or(0, |prev| prev.0);
        cursor.nearby = self
            .nearby
            .range(start_p..)
            .find_map(|(&p, &(word, verified))| {
                for i in 0..8 {
                    let lane = Self::lane(word, i);
                    if lane == 0 {
                        break;
                    }
                    if after.is_none_or(|prev| Self::absolute(p, lane) > prev) {
                        return Some((p, word, verified, i as u8));
                    }
                }
                None
            });
        cursor.branch = match after {
            Some(prev) => self.branches.range((Excluded(prev), Unbounded)).next().copied(),
            None => self.branches.first().copied(),
        };
        cursor.synced = true;
    }
    /// Peek a contiguous run of verified contact bits inside the current word
    /// that strictly precedes any nearby/branch record and fits `max_take`.
    pub(super) fn peek_verified_contact_run(
        &self,
        after: Option<Pair>,
        max_take: u32,
        cursor: &mut Cursor,
    ) -> Option<(u64, u64, u64, u32, u64, u64)> {
        #[cfg(test)]
        if self.reference.is_some() {
            return None;
        }
        if max_take == 0 || self.verified_profile.is_none() {
            return None;
        }
        self.sync_cursor(after, cursor);
        let (w, bits, verified) = cursor.word?;
        let unverified = bits & !verified;
        let prefix_mask = if unverified == 0 {
            u64::MAX
        } else {
            (1u64 << unverified.trailing_zeros()) - 1
        };
        let verified_prefix = bits & prefix_mask;
        let avail = verified_prefix.count_ones();
        if avail < 2 {
            return None;
        }
        let (take_bits, take_n) = if avail <= max_take {
            (verified_prefix, avail)
        } else {
            let mut rem = verified_prefix;
            let mut keep = 0u64;
            for _ in 0..max_take {
                let b = rem & rem.wrapping_neg();
                keep |= b;
                rem &= !b;
            }
            (keep, max_take)
        };
        let first_p = (w << 6) | u64::from(take_bits.trailing_zeros());
        let last_p = (w << 6) | (63 - u64::from(take_bits.leading_zeros()));
        let head_nearby = cursor
            .nearby
            .map(|(p, word, _, i)| Self::absolute(p, Self::lane(word, usize::from(i))));
        if head_nearby.is_some_and(|hn| hn <= (last_p, last_p))
            || cursor.branch.is_some_and(|hb| hb <= (last_p, last_p))
        {
            return None;
        }
        Some((w, first_p, last_p, take_n, bits & !take_bits, verified))
    }
    #[inline]
    pub(super) fn advance_contact_word(
        &self,
        w: u64,
        rem_after: u64,
        verified: u64,
        cursor: &mut Cursor,
    ) {
        cursor.word = if rem_after != 0 {
            Some((w, rem_after, verified))
        } else {
            w.checked_add(1).and_then(|nw| {
                self.words
                    .range(nw..)
                    .next()
                    .map(|(&nw, &(nb, nv))| (nw, nb, nv))
            })
        };
    }
    /// Read-only successor returning `(pair, is_verified_pristine)`.
    pub(super) fn next_verified(
        &self,
        after: Option<Pair>,
        cursor: &mut Cursor,
    ) -> Option<(Pair, bool)> {
        use std::ops::Bound::{Excluded, Unbounded};
        #[cfg(test)]
        if self.reference.is_some() {
            let next = match after {
                Some(prev) => self.after(prev),
                None => self.first(),
            };
            return next.map(|p| (p, false));
        }
        self.sync_cursor(after, cursor);
        let head_word = cursor.word.map(|(w, bits, _)| {
            let p = (w << 6) | u64::from(bits.trailing_zeros());
            (p, p)
        });
        let head_nearby = cursor
            .nearby
            .map(|(p, word, _, i)| Self::absolute(p, Self::lane(word, usize::from(i))));
        let head_branch = cursor.branch;
        let best = [head_word, head_nearby, head_branch]
            .into_iter()
            .flatten()
            .min()?;
        let is_verified = if head_word == Some(best) {
            let (w, mut bits, verified) = cursor.word.unwrap();
            let lowest = bits & bits.wrapping_neg();
            let v = self.verified_profile.is_some() && (verified & lowest != 0);
            bits &= !lowest;
            cursor.word = if bits != 0 {
                Some((w, bits, verified))
            } else {
                w.checked_add(1).and_then(|nw| {
                    self.words
                        .range(nw..)
                        .next()
                        .map(|(&nw, &(nb, nv))| (nw, nb, nv))
                })
            };
            v
        } else if head_nearby == Some(best) {
            let (p, word, verified, i) = cursor.nearby.unwrap();
            let v = self.verified_profile.is_some() && (verified & (1u8 << i) != 0);
            let next_i = i + 1;
            cursor.nearby = if next_i < 8 && Self::lane(word, usize::from(next_i)) != 0 {
                Some((p, word, verified, next_i))
            } else {
                p.checked_add(1).and_then(|np| {
                    self.nearby
                        .range(np..)
                        .next()
                        .map(|(&np, &(nw, nv))| (np, nw, nv, 0))
                })
            };
            v
        } else {
            cursor.branch = self
                .branches
                .range((Excluded(best), Unbounded))
                .next()
                .copied();
            false
        };
        Some((best, is_verified))
    }
    /// Read-only successor using cached per-stream heads inside an atomic call.
    /// Call `cursor.invalidate()` whenever `Front` is mutated (`insert`/`remove`).
    #[cfg(test)]
    pub(super) fn next(&self, after: Option<Pair>, cursor: &mut Cursor) -> Option<Pair> {
        self.next_verified(after, cursor).map(|(pair, _)| pair)
    }
    pub fn iter(&self) -> impl Iterator<Item = Pair> + '_ {
        let mut words = self.words.iter();
        let (mut key, mut mask) = (0, 0u64);
        let mut contacts = std::iter::from_fn(move || {
            if mask == 0 {
                let (&next_key, &(next_mask, _)) = words.next()?;
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
            .flat_map(|(&p, &(word, _))| {
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
