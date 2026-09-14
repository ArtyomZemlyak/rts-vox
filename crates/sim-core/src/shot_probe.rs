//! Reuse legacy collision candidates across all samples of one projectile
//! step. Material bodies still use their voxel trace. No trajectory, sample,
//! hit shape or tie rule changes here.
use crate::{damage::Bodies, index::Grid, Units, NO_TARGET};

#[derive(Default)]
pub(crate) struct Probe {
    targets: Vec<Target>,
    r2: i32,
    height: i32,
}
struct Target {
    id: u32,
    x: i32,
    y: i32,
    z: i32,
    bx: usize,
    bz: usize,
}

impl Probe {
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        grid: &Grid,
        units: &Units,
        bodies: Option<&Bodies>,
        from: (i32, i32, i32),
        to: (i32, i32, i32),
        radius: i32,
        height: i32,
        owner: u8,
    ) {
        self.targets.clear();
        self.r2 = radius * radius;
        self.height = height;
        let lo = [
            from.0.min(to.0) - radius,
            from.1.min(to.1) - height,
            from.2.min(to.2) - radius,
        ];
        let hi = [
            from.0.max(to.0) + radius,
            from.1.max(to.1) + height,
            from.2.max(to.2) + radius,
        ];
        grid.for_each_bucket_swept([from.0, from.2], [to.0, to.2], |bucket| {
            for &id in grid.bucket_items(bucket) {
                let i = id as usize;
                if !units.alive[i] || units.owner[i] == owner {
                    continue;
                }
                let (x, y, z) = (units.x[i], units.body_cell_y(i), units.z[i]);
                if x < lo[0] || x > hi[0] || y < lo[1] || y > hi[1] || z < lo[2] || z > hi[2] {
                    continue;
                }
                if bodies.is_some_and(|b| b.body(id).is_some()) {
                    continue;
                }
                // Store ACTUAL index membership, not bucket_of(current pose).
                // The unit may have moved since the index was built.
                self.targets.push(Target {
                    id,
                    x,
                    y,
                    z,
                    bx: bucket % grid.w,
                    bz: bucket / grid.w,
                });
            }
        });
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    #[inline]
    pub fn at(&self, grid: &Grid, x: i32, y: i32, z: i32) -> u32 {
        let b = grid.bucket_of(x, z);
        let (bx, bz) = (b % grid.w, b / grid.w);
        let (mut best, mut id) = (i32::MAX, NO_TARGET);
        for t in &self.targets {
            if bx.abs_diff(t.bx) > 1 || bz.abs_diff(t.bz) > 1 || (t.y - y).abs() > self.height {
                continue;
            }
            let (dx, dz) = (t.x - x, t.z - z);
            let d2 = dx * dx + dz * dz;
            if d2 <= self.r2 && (d2 < best || (d2 == best && t.id < id)) {
                best = d2;
                id = t.id;
            }
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle(g: &Grid, u: &Units, p: (i32, i32, i32), r: i32, h: i32, owner: u8) -> u32 {
        let (mut best, mut id) = (i32::MAX, NO_TARGET);
        g.for_each_near(p.0, p.2, |j| {
            let i = j as usize;
            if !u.alive[i] || u.owner[i] == owner || (u.body_cell_y(i) - p.1).abs() > h {
                return;
            }
            let (dx, dz) = (u.x[i] - p.0, u.z[i] - p.2);
            let d2 = dx * dx + dz * dz;
            if d2 <= r * r && (d2 < best || (d2 == best && j < id)) {
                best = d2;
                id = j;
            }
        });
        id
    }

    #[test]
    fn matches_the_per_sample_oracle_across_buckets_edges_and_moved_units() {
        let mut u = Units::new();
        let mut rng = fx::rng::Rng::new(248, 4);
        for _ in 0..600 {
            let x = rng.range(-20, 320);
            let z = rng.range(-20, 320);
            u.spawn(x, z, rng.below(2) as u8, 10, 100);
        }
        u.refresh_live();
        let mut probe = Probe::default();
        let mut hits = 0;
        for cell in [3, 17, 60, 300] {
            let mut g = Grid::new(300, cell, u.len());
            g.rebuild(&u.x, &u.z, u.live());
            // Deliberately retain beginning-of-phase bucket membership.
            for i in 0..u.len() {
                u.x[i] += rng.range(-5, 5);
                u.z[i] += rng.range(-5, 5);
                u.y[i] = rng.range(0, 30);
            }
            for _ in 0..60 {
                let from = (rng.range(-20, 320), rng.range(0, 30), rng.range(-20, 320));
                let to = (rng.range(-20, 320), rng.range(0, 30), rng.range(-20, 320));
                for (a, b) in [(from, to), (to, from)] {
                    probe.prepare(&g, &u, None, a, b, 4, 10, 0);
                    voxel::los::walk(a, b, |x, y, z| {
                        let got = probe.at(&g, x, y, z);
                        assert_eq!(got, oracle(&g, &u, (x, y, z), 4, 10, 0));
                        hits += usize::from(got != NO_TARGET);
                        true
                    });
                    assert_eq!(probe.at(&g, b.0, b.1, b.2), oracle(&g, &u, b, 4, 10, 0));
                }
            }
        }
        assert!(hits > 100);
    }

    #[test]
    fn dense_unrelated_neighbours_do_not_enter_the_narrow_pass_and_ties_use_ids() {
        let mut u = Units::new();
        let a = u.spawn(110, 99, 1, 10, 100);
        u.spawn(110, 101, 1, 10, 100);
        for i in 0..5000 {
            u.spawn(180, 180 + i % 10, 1, 10, 100);
        }
        u.refresh_live();
        let mut g = Grid::new(300, 300, u.len());
        g.rebuild(&u.x, &u.z, u.live());
        let mut probe = Probe::default();
        probe.prepare(&g, &u, None, (100, 0, 100), (120, 0, 100), 2, 10, 0);
        assert_eq!(probe.targets.len(), 2);
        assert_eq!(probe.at(&g, 110, 0, 100), a);
        probe.prepare(&g, &u, None, (0, 0, 0), (1, 0, 0), 2, 10, 0);
        assert!(probe.is_empty());
    }
}
