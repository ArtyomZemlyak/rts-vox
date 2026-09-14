//! Exact reuse of contact sums at identical integer snapshot centres.
//!
//! The contact law already reads integer X/Z, independently of chassis,
//! faction and subcell motion. Equal centres therefore have equal unclamped
//! sums. Multiplicity is applied AFTER each pair's integer rounding; each
//! unit's speed clamp is applied only when reading the final sum.
use crate::index::Grid;

#[derive(Default)]
pub(crate) struct Contacts {
    starts: Vec<usize>,
    points: Vec<Point>,
    slots: Vec<usize>,
    scratch: Vec<u32>,
    pushes: Vec<(i32, i32)>,
    /// Legacy unit-pair work, retained for comparable reports.
    pub equivalent_pairs: u64,
    /// Actual group-pair visits, including rejected distances.
    pub group_pairs: u64,
}

struct Point {
    x: i32,
    z: i32,
    count: u32,
}

impl Contacts {
    pub fn prepare(&mut self, grid: &Grid, xs: &[i32], zs: &[i32], r2: i32) {
        self.starts.clear();
        self.points.clear();
        self.slots.resize(xs.len(), 0);
        for b in 0..grid.w * grid.h {
            self.starts.push(self.points.len());
            self.scratch.clear();
            self.scratch.extend_from_slice(grid.bucket_items(b));
            self.scratch
                .sort_unstable_by_key(|&j| (xs[j as usize], zs[j as usize], j));
            for &j in &self.scratch {
                let j = j as usize;
                let (x, z) = (xs[j], zs[j]);
                debug_assert_eq!(
                    grid.bucket_of(x, z),
                    b,
                    "contact grid must match its snapshot"
                );
                if self.points.last().is_none_or(|p| p.x != x || p.z != z) {
                    self.points.push(Point { x, z, count: 0 });
                }
                let group = self.points.len() - 1;
                self.points[group].count += 1;
                self.slots[j] = group;
            }
        }
        self.starts.push(self.points.len());
        self.pushes.clear();
        self.pushes.resize(self.points.len(), (0, 0));
        self.equivalent_pairs = 0;
        self.group_pairs = 0;
        for (i, p) in self.points.iter().enumerate() {
            let (mut px, mut pz) = self.pushes[i];
            let (mut equivalent, mut groups) = (0u64, 0u64);
            grid.for_each_bucket_near(p.x, p.z, |b| {
                equivalent += grid.bucket_items(b).len() as u64;
                // Neighbourhood membership is symmetric. Handle each pair
                // once and write both endpoints; all additions wrap, with
                // no intermediate clamp, so regrouping preserves the sum.
                let lo = self.starts[b].max(i + 1);
                for j in lo..self.starts[b + 1] {
                    let q = &self.points[j];
                    groups += 1;
                    let (dx, dz) = (p.x - q.x, p.z - q.z);
                    let d2 = dx * dx + dz * dz;
                    if d2 == 0 || d2 >= r2 {
                        continue;
                    }
                    let w = (r2 - d2) * 64 / r2;
                    px = px.wrapping_add((dx * w / 16).wrapping_mul(q.count as i32));
                    pz = pz.wrapping_add((dz * w / 16).wrapping_mul(q.count as i32));
                    // Evaluate signed rounding for the reverse endpoint too.
                    self.pushes[j].0 = self.pushes[j]
                        .0
                        .wrapping_add((-dx * w / 16).wrapping_mul(p.count as i32));
                    self.pushes[j].1 = self.pushes[j]
                        .1
                        .wrapping_add((-dz * w / 16).wrapping_mul(p.count as i32));
                }
            });
            self.pushes[i] = (px, pz);
            self.equivalent_pairs += equivalent * p.count as u64;
            self.group_pairs += groups;
        }
    }

    #[inline]
    pub fn push(&self, slot: usize, cap: i32) -> (i32, i32) {
        let (x, z) = self.pushes[self.slots[slot]];
        (x.clamp(-cap, cap), z.clamp(-cap, cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compare(
        xs: &[i32],
        zs: &[i32],
        live: &[u32],
        grid: &mut Grid,
        c: &mut Contacts,
        radius: i32,
    ) {
        grid.rebuild(xs, zs, live);
        c.prepare(grid, xs, zs, radius * radius);
        let mut examined = 0u64;
        for &i in live {
            let i = i as usize;
            let (mut x, mut z) = (0i32, 0i32);
            // Independent per-entity oracle: no weighted centres or preclamp.
            grid.for_each_near(xs[i], zs[i], |j| {
                examined += 1;
                let j = j as usize;
                let (dx, dz) = (xs[i] - xs[j], zs[i] - zs[j]);
                let d2 = dx * dx + dz * dz;
                if d2 == 0 || d2 >= radius * radius {
                    return;
                }
                let w = (radius * radius - d2) * 64 / (radius * radius);
                x = x.wrapping_add(dx * w / 16);
                z = z.wrapping_add(dz * w / 16);
            });
            for cap in [0, 1, 24, 125, i32::MAX] {
                assert_eq!(
                    c.push(i, cap),
                    (x.clamp(-cap, cap), z.clamp(-cap, cap)),
                    "slot {i}"
                );
            }
        }
        assert_eq!(c.equivalent_pairs, examined);
    }

    #[test]
    fn exact_on_dense_sparse_and_clamped_edge_snapshots() {
        let mut rng = fx::rng::Rng::new(246, 1);
        let mut c = Contacts::default();
        for cell in [15, 60, 125] {
            let mut grid = Grid::new(500, cell, 1000);
            for spread in [1, 8, 60, 700] {
                let xs: Vec<_> = (0..1000).map(|_| rng.range(-10, spread)).collect();
                let zs: Vec<_> = (0..1000).map(|_| rng.range(-10, spread)).collect();
                let live: Vec<_> = (0..1000).filter(|i| i % 7 != 0).collect();
                compare(&xs, &zs, &live, &mut grid, &mut c, 60);
                let reused: Vec<_> = (0..1000).filter(|i| i % 3 == 0).collect();
                compare(&zs, &xs, &reused, &mut grid, &mut c, 24);
                compare(&xs, &zs, &[], &mut grid, &mut c, 60);
            }
        }
    }

    #[test]
    fn multiplicity_reuses_work_without_rounding_or_clamping_early() {
        let xs: Vec<_> = (0..5000).map(|i| [100, 117, 91][i % 3]).collect();
        let zs: Vec<_> = (0..5000).map(|i| [100, 103, 110][i % 3]).collect();
        let live: Vec<_> = (0..5000).collect();
        let mut grid = Grid::new(500, 60, 5000);
        let mut c = Contacts::default();
        compare(&xs, &zs, &live, &mut grid, &mut c, 60);
        assert_eq!(c.group_pairs, 3);
        assert_eq!(c.equivalent_pairs, 25_000_000);
    }
}
