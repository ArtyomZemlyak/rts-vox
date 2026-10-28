//! Derived exact-input memo for a join pass that found no possible transfers.
//! Never caches world support, physics admission, or an approximate fingerprint.
use super::*;
pub(super) type Key = ([i32; 3], Mat, Option<[u8; 3]>, u16);
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Input {
    pub key: Key,
    pub index: usize,
    id: u64,
    geometry: fragment_join::Geometry,
}
#[derive(Default)]
pub(super) struct Memo {
    previous: Vec<Input>,
    pub current: Vec<Input>,
    valid: bool,
    #[cfg(test)]
    pub hits: u64,
}
impl Memo {
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
    pub fn unchanged(&mut self) -> bool {
        let same = self.valid && self.previous == self.current;
        #[cfg(test)]
        if same {
            self.hits = self.hits.wrapping_add(1);
        }
        same
    }
    pub fn finish(&mut self, no_transfers: bool) {
        // A successful merge changes the next input. Do not cache a plan that
        // performed transfers, even if an external reset later reuses IDs.
        self.valid = no_transfers;
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
    }
}
