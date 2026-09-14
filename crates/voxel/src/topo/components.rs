//! Incremental six-connected extraction from a unit's disconnected mask.
//! A token scans one word or expands one cell (at most six neighbours).
//! A completed component moves out as a whole, without a size-dependent copy.
use super::unit::UnitVol;
use crate::impact::Cell;
use fx::blob::{Blob, Scan};
use std::collections::BTreeSet;

pub struct UnitComponents {
    remaining: UnitVol,
    scan: usize,
    queue: Vec<Cell>,
    head: usize,
    remaining_hash: u64,
    queue_hash: u64,
}
fn word_hash(i: usize, value: u64) -> u64 {
    let mut h = value.wrapping_add((i as u64).wrapping_mul(0x9e3779b97f4a7c15));
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}
fn cell_code(c: Cell) -> u64 {
    c[0] as u64 | ((c[1] as u64) << 8) | ((c[2] as u64) << 16)
}
impl UnitComponents {
    pub fn new(remaining: UnitVol) -> Self {
        let remaining_hash = remaining
            .cols
            .iter()
            .enumerate()
            .fold(0, |h, (i, v)| h ^ word_hash(i, *v));
        Self {
            remaining,
            scan: 0,
            queue: Vec::new(),
            head: 0,
            remaining_hash,
            queue_hash: 0,
        }
    }
    pub fn finished(&self) -> bool {
        self.scan == self.remaining.cols.len() && self.queue.is_empty()
    }
    pub fn pending_count(&self) -> usize {
        self.remaining.solid_count() as usize + self.queue.len()
    }
    pub fn contains(&self, c: Cell) -> bool {
        self.remaining
            .get(c[0] as usize, c[1] as usize, c[2] as usize)
            || self.queue.contains(&c)
    }
    pub fn contained_in(&self, mask: &UnitVol) -> bool {
        self.dims() == [mask.nx, mask.ny, mask.nz]
            && self
                .remaining
                .cols
                .iter()
                .zip(&mask.cols)
                .all(|(a, b)| a & !b == 0)
            && self
                .queue
                .iter()
                .all(|c| mask.get(c[0] as usize, c[1] as usize, c[2] as usize))
    }
    pub fn dims(&self) -> [u8; 3] {
        [self.remaining.nx, self.remaining.ny, self.remaining.nz]
    }
    fn word_of(&self, c: Cell) -> usize {
        (c[2] as usize * self.remaining.ny as usize + c[1] as usize) * self.remaining.words as usize
            + c[0] as usize / 64
    }
    fn take(&mut self, c: Cell) {
        let w = self.word_of(c);
        let old = self.remaining.cols[w];
        let new = old & !(1u64 << (c[0] & 63));
        debug_assert_ne!(old, new);
        self.remaining_hash ^= word_hash(w, old) ^ word_hash(w, new);
        self.remaining.cols[w] = new;
        self.queue_hash = self
            .queue_hash
            .wrapping_mul(0x100000001b3)
            .wrapping_add(cell_code(c));
        self.queue.push(c);
    }
    pub fn advance(&mut self, budget: &mut u32) -> Vec<Vec<Cell>> {
        let mut out = Vec::new();
        let dims = self.dims();
        while *budget > 0 && !self.finished() {
            *budget -= 1;
            if self.head < self.queue.len() {
                let c = self.queue[self.head];
                self.head += 1;
                for axis in 0..3 {
                    for d in [-1i16, 1] {
                        let v = c[axis] as i16 + d;
                        if v < 0 || v >= dims[axis] as i16 {
                            continue;
                        }
                        let mut q = c;
                        q[axis] = v as u8;
                        if self
                            .remaining
                            .get(q[0] as usize, q[1] as usize, q[2] as usize)
                        {
                            self.take(q);
                        }
                    }
                }
                if self.head == self.queue.len() {
                    out.push(std::mem::take(&mut self.queue));
                    self.head = 0;
                    self.queue_hash = 0;
                }
            } else {
                let bits = self.remaining.cols[self.scan];
                if bits == 0 {
                    self.scan += 1;
                    continue;
                }
                let row = self.scan / self.remaining.words as usize;
                let c = [
                    ((self.scan % self.remaining.words as usize) * 64
                        + bits.trailing_zeros() as usize) as u8,
                    (row % self.remaining.ny as usize) as u8,
                    (row / self.remaining.ny as usize) as u8,
                ];
                self.take(c);
            }
        }
        out
    }
    pub fn hash(&self) -> u64 {
        let mut h = self.remaining_hash;
        for v in self.dims().map(|v| v as u64).into_iter().chain([
            self.scan as u64,
            self.head as u64,
            self.queue.len() as u64,
            self.queue_hash,
        ]) {
            h = h.wrapping_mul(0x100000001b3).wrapping_add(v);
        }
        h
    }
    pub fn save(&self, b: &mut Blob) {
        for v in self.dims() {
            b.u8(v);
        }
        b.usize(self.scan);
        b.usize(self.head);
        b.u64s(&self.remaining.cols);
        b.u32(self.queue.len() as u32);
        for c in &self.queue {
            for v in c {
                b.u8(*v);
            }
        }
    }
    pub fn load(s: &mut Scan) -> Option<Self> {
        let dims = [s.u8(), s.u8(), s.u8()];
        if dims.contains(&0) {
            return None;
        }
        let scan = s.usize();
        let head = s.usize();
        let cols = s.u64s();
        let words = dims[0].div_ceil(64);
        let expected = words as usize * dims[1] as usize * dims[2] as usize;
        if cols.len() != expected || scan > expected || cols[..scan].iter().any(|v| *v != 0) {
            return None;
        }
        let last = dims[0] as u32 % 64;
        if last != 0
            && cols
                .iter()
                .enumerate()
                .any(|(i, v)| i % words as usize == words as usize - 1 && *v >> last != 0)
        {
            return None;
        }
        let remaining = UnitVol {
            nx: dims[0],
            ny: dims[1],
            nz: dims[2],
            words,
            cols,
        };
        let count = s.list(3);
        if count > remaining.voxels() || head > count || (count > 0 && head == count) {
            return None;
        }
        let mut out = Self::new(remaining);
        out.scan = scan;
        out.head = head;
        let mut seen = BTreeSet::new();
        for _ in 0..count {
            let c = [s.u8(), s.u8(), s.u8()];
            if (0..3).any(|k| c[k] >= dims[k])
                || !seen.insert(c)
                || out
                    .remaining
                    .get(c[0] as usize, c[1] as usize, c[2] as usize)
            {
                return None;
            }
            out.queue_hash = out
                .queue_hash
                .wrapping_mul(0x100000001b3)
                .wrapping_add(cell_code(c));
            out.queue.push(c);
        }
        if out.scan == expected && !out.queue.is_empty() {
            return None;
        }
        s.ok().then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shape() -> UnitVol {
        let mut v = UnitVol::new(130, 8, 7);
        for x in 60..=68 {
            v.set(x, 2, 2, true);
        }
        for y in 3..6 {
            v.set(68, y, 2, true);
        }
        v.set(69, 6, 3, true);
        v.set(4, 1, 1, true);
        v
    }
    fn run(budget: u32) -> Vec<Vec<Cell>> {
        let mut scan = UnitComponents::new(shape());
        let mut out = Vec::new();
        for _ in 0..10000 {
            let mut left = budget;
            out.extend(scan.advance(&mut left));
            assert!(left <= budget);
            if scan.finished() {
                break;
            }
        }
        assert!(scan.finished());
        out
    }
    #[test]
    fn work_slices_keep_whole_components_and_do_not_join_diagonal_corners() {
        let a = run(1);
        assert_eq!(a, run(10000));
        assert_eq!(a.iter().map(Vec::len).collect::<Vec<_>>(), [1, 12, 1]);
        let count = a.iter().map(Vec::len).sum::<usize>();
        assert_eq!(count, shape().solid_count() as usize);
    }
    #[test]
    fn scanning_and_a_half_expanded_component_resume_byte_for_byte() {
        for cut in 0..200 {
            let mut a = UnitComponents::new(shape());
            a.advance(&mut { cut });
            let mut blob = Blob::new();
            a.save(&mut blob);
            let mut reader = Scan::new(&blob.v);
            let mut b = UnitComponents::load(&mut reader).unwrap();
            assert_eq!(reader.left(), 0, "cut {cut}");
            let mut again = Blob::new();
            b.save(&mut again);
            assert_eq!(blob.v, again.v);
            assert_eq!(a.hash(), b.hash());
            for _ in 0..1000 {
                assert_eq!(a.advance(&mut 3), b.advance(&mut 3));
                assert_eq!(a.hash(), b.hash());
                if a.finished() {
                    break;
                }
            }
            assert!(a.finished() && b.finished());
        }
    }
    #[test]
    fn zero_work_and_truncated_inputs_are_safe() {
        let mut a = UnitComponents::new(shape());
        let before = a.hash();
        assert!(a.advance(&mut 0).is_empty());
        assert_eq!(a.hash(), before);
        a.advance(&mut 19);
        let mut blob = Blob::new();
        a.save(&mut blob);
        for end in 0..blob.v.len() {
            assert!(UnitComponents::load(&mut Scan::new(&blob.v[..end])).is_none());
        }
    }
}
