//! Derived membership only: queue order lives in the authoritative FIFO.
//! Lookup has no unordered iterator, so hash bucket layout cannot select work.
use std::hash::BuildHasherDefault;
#[derive(Default)]
pub(super) struct Positions(
    fx::lookup::Lookup<[u32; 3], (), BuildHasherDefault<rustc_hash::FxHasher>>,
);
impl Positions {
    pub fn contains(&self, p: &[u32; 3]) -> bool {
        self.0.contains_key(p)
    }
    pub fn insert(&mut self, p: [u32; 3]) -> bool {
        self.0.insert(p, ()).is_none()
    }
    pub fn remove(&mut self, p: &[u32; 3]) -> bool {
        self.0.remove(p).is_some()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
}
