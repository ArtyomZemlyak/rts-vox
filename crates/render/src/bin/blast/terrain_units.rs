//! Disposable broad phase for a batch of terrain publication edits.
//! Buckets only reject impossible bodies; the original point/solid test follows.
type Bounds = ([i32; 3], [i32; 3]);
const EDGE: i32 = 32;
const SLOTS: usize = 64;

#[derive(Default)]
struct Bin {
    key: Option<[i32; 3]>,
    actors: Vec<usize>,
}

pub(super) struct Candidates<'a> {
    boxes: &'a [Bounds],
    bins: [Bin; SLOTS],
}
impl<'a> Candidates<'a> {
    pub(super) fn new(boxes: &'a [Bounds]) -> Self {
        Self {
            boxes,
            bins: std::array::from_fn(|_| Bin::default()),
        }
    }

    pub(super) fn at(&mut self, p: [i32; 3]) -> &[usize] {
        let key = p.map(|v| v.div_euclid(EDGE));
        let slot = (key[0] as usize).wrapping_mul(73856093)
            ^ (key[1] as usize).wrapping_mul(19349663)
            ^ (key[2] as usize).wrapping_mul(83492791);
        let bin = &mut self.bins[slot & (SLOTS - 1)];
        if bin.key != Some(key) {
            bin.key = Some(key);
            bin.actors.clear();
            let low = key.map(|v| v * EDGE);
            let high = low.map(|v| v + (EDGE - 1));
            bin.actors
                .extend(self.boxes.iter().enumerate().filter_map(|(i, (lo, hi))| {
                    (0..3)
                        .all(|a| low[a] <= hi[a] && high[a] >= lo[a])
                        .then_some(i)
                }));
        }
        &bin.actors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contains((lo, hi): Bounds, p: [i32; 3]) -> bool {
        (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a])
    }

    #[test]
    fn cached_candidates_preserve_exact_point_results_and_actor_order() {
        let mut boxes: Vec<_> = (0..80)
            .map(|i| {
                let lo = [i * 7 - 180, (i % 5) * 13 - 40, (i % 11) * 29 - 160];
                (lo, [lo[0] + 65, lo[1] + 31, lo[2] + 97])
            })
            .collect();
        boxes.extend([
            ([i32::MIN; 3], [i32::MIN + 31; 3]),
            ([i32::MAX - 31; 3], [i32::MAX; 3]),
        ]);
        let mut candidates = Candidates::new(&boxes);
        // Revisit distant buckets, including direct-map slot collisions.
        for reverse in [false, true] {
            for i in 0..8192 {
                let i = if reverse { 8191 - i } else { i };
                let p = [
                    (i * 17) % 769 - 256,
                    (i * 13) % 193 - 65,
                    (i * 31) % 641 - 256,
                ];
                let expected: Vec<_> = boxes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &b)| contains(b, p).then_some(i))
                    .collect();
                let actual: Vec<_> = candidates
                    .at(p)
                    .iter()
                    .copied()
                    .filter(|&i| contains(boxes[i], p))
                    .collect();
                assert_eq!(actual, expected, "point={p:?}");
            }
        }
        for p in [
            [i32::MIN; 3],
            [i32::MAX; 3],
            [-32; 3],
            [-1; 3],
            [0; 3],
            [31; 3],
            [32; 3],
        ] {
            let expected: Vec<_> = boxes
                .iter()
                .enumerate()
                .filter_map(|(i, &b)| contains(b, p).then_some(i))
                .collect();
            let actual: Vec<_> = candidates
                .at(p)
                .iter()
                .copied()
                .filter(|&i| contains(boxes[i], p))
                .collect();
            assert_eq!(actual, expected);
        }
    }
}
