//! Derived broad phase shared by liquid displacement and granular contacts.
//! Giant boxes stay in a small fallback list instead of filling world bins.
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug)]
pub struct Obstacle {
    pub min: [i64; 3],
    pub max: [i64; 3],
}
impl Obstacle {
    fn contains(self, p: [u32; 3]) -> bool {
        (0..3).all(|a| i64::from(p[a]) >= self.min[a] && i64::from(p[a]) < self.max[a])
    }
}
pub struct Index {
    boxes: Vec<Obstacle>,
    bins: BTreeMap<[i64; 3], Vec<usize>>,
    wide: Vec<usize>,
}
impl Index {
    pub fn new(boxes: Vec<Obstacle>) -> Self {
        let mut out = Self {
            boxes,
            bins: BTreeMap::new(),
            wide: Vec::new(),
        };
        for (i, b) in out.boxes.iter().enumerate() {
            let lo = b.min.map(|v| v.div_euclid(8));
            let hi = b.max.map(|v| (v - 1).div_euclid(8));
            let count = (0..3)
                .map(|a| (hi[a] - lo[a] + 1).max(0) as u64)
                .fold(1u64, |v, n| v.saturating_mul(n));
            if count > 256 {
                out.wide.push(i);
                continue;
            }
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        out.bins.entry([x, y, z]).or_default().push(i);
                    }
                }
            }
        }
        out
    }
    pub fn contains(&self, p: [u32; 3]) -> bool {
        self.wide
            .iter()
            .chain(
                self.bins
                    .get(&p.map(|v| i64::from(v) / 8))
                    .into_iter()
                    .flatten(),
            )
            .any(|&i| self.boxes[i].contains(p))
    }
}
