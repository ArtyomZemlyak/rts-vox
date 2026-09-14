//! Read-only sole contacts and terrain clearance for articulated feet.
use bevy::prelude::{Quat, Vec3};

#[derive(Clone, Copy, Debug, Default)]
pub struct Contact {
    pub pos: Vec3,
    pub rot: Quat,
}
impl Contact {
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            pos: self.pos.lerp(other.pos, t),
            rot: self.rot.slerp(other.rot, t),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sole {
    probes: Vec<Vec3>,
}
impl Sole {
    pub fn point(depth: f32) -> Self {
        Self {
            probes: vec![Vec3::new(0.0, -depth, 0.0)],
        }
    }

    /// Nine representative occupied bottom cells, cached once per template.
    /// Empty space between toes must not act as a supporting surface.
    pub fn of(segment: &voxel::unit::Segment, per_mm: f32) -> Self {
        let b = &segment.body;
        let k = b.mm as f32 * per_mm;
        let p = crate::model::pivot_centre(&segment.attach);
        let mut bottom = Vec::new();
        for z in 0..b.vol.nz as usize {
            for x in 0..b.vol.nx as usize {
                if let Some(y) = (0..b.vol.ny as usize).find(|&y| b.vol.get(x, y, z)) {
                    bottom.push(Vec3::new(
                        (x as f32 + 0.5 - p[0]) * k,
                        (y as f32 - p[1]) * k,
                        (z as f32 + 0.5 - p[2]) * k,
                    ));
                }
            }
        }
        if bottom.is_empty() {
            return Self::point(p[1] * k);
        }
        let lo = bottom
            .iter()
            .copied()
            .fold(Vec3::splat(f32::INFINITY), Vec3::min);
        let hi = bottom
            .iter()
            .copied()
            .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        let mut probes = Vec::new();
        for z in 0..3 {
            for x in 0..3 {
                let want = Vec3::new(
                    lo.x + (hi.x - lo.x) * x as f32 * 0.5,
                    0.0,
                    lo.z + (hi.z - lo.z) * z as f32 * 0.5,
                );
                let distance = |v: &Vec3| (v.x - want.x).powi(2) + (v.z - want.z).powi(2);
                let probe = *bottom
                    .iter()
                    .min_by(|a, b| distance(a).total_cmp(&distance(b)))
                    .unwrap();
                if !probes.contains(&probe) {
                    probes.push(probe);
                }
            }
        }
        Self { probes }
    }

    /// Fit a plane over the sole, then lift it until all sampled points clear
    /// the actual terrain. The ankle rotates independently of the hull.
    pub fn contact(&self, at: Vec3, yaw: f32, floor: &dyn Fn(f32, f32) -> f32) -> Contact {
        let heading = Quat::from_rotation_y(yaw);
        let mut mean = Vec3::ZERO;
        let mut heights = [0.0; 9];
        for (i, p) in self.probes.iter().enumerate() {
            let w = at + heading * *p;
            heights[i] = floor(w.x, w.z);
            mean += Vec3::new(p.x, heights[i], p.z);
        }
        mean /= self.probes.len() as f32;
        let (mut xx, mut xz, mut zz, mut xy, mut zy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, p) in self.probes.iter().enumerate() {
            let (x, z, y) = (p.x - mean.x, p.z - mean.z, heights[i] - mean.y);
            xx += x * x;
            xz += x * z;
            zz += z * z;
            xy += x * y;
            zy += z * y;
        }
        let det = xx * zz - xz * xz;
        let normal = if det > 1e-6 {
            let mut slope = Vec3::new((xy * zz - zy * xz) / det, 0.0, (zy * xx - xy * xz) / det);
            // Presentation joint limit: a cliff cannot turn a sole vertical.
            slope = slope.clamp_length_max(1.0);
            Vec3::new(-slope.x, 1.0, -slope.z).normalize()
        } else {
            Vec3::Y
        };
        let rot = heading * Quat::from_rotation_arc(Vec3::Y, normal);
        Contact {
            pos: Vec3::new(at.x, self.height(at, rot, floor), at.z),
            rot,
        }
    }

    fn height(&self, at: Vec3, rot: Quat, floor: &dyn Fn(f32, f32) -> f32) -> f32 {
        self.probes
            .iter()
            .map(|p| {
                let offset = rot * *p;
                floor(at.x + offset.x, at.z + offset.z) - offset.y
            })
            .fold(f32::NEG_INFINITY, f32::max)
    }

    /// The route is sampled only when planning a step. Its fixed clearance
    /// gives a continuous arc over voxel ledges instead of following their Y.
    pub fn clearance(
        &self,
        from: Contact,
        to: Contact,
        lift: f32,
        floor: &dyn Fn(f32, f32) -> f32,
    ) -> f32 {
        let samples = (from.pos.distance(to.pos).ceil() as usize).clamp(8, 32);
        let mut height = lift;
        for i in 1..samples {
            let q = i as f32 / samples as f32;
            let eased = q * q * (3.0 - 2.0 * q);
            let at = from.lerp(to, eased);
            let need = self.height(at.pos, at.rot, floor) - at.pos.y;
            height = height.max(need / (q * std::f32::consts::PI).sin() + lift);
        }
        height
    }
}

/// Nearest actual exposed voxel top around the selected navigation floor.
/// Search is bounded by the leg reach; a tunnel never defaults to the roof.
pub fn voxel_floor(world: &voxel::World, x: f32, z: f32, near: f32, reach: i32) -> Option<f32> {
    let (x, z) = (x.floor() as i32, z.floor() as i32);
    if x < 0 || z < 0 || x >= world.nx as i32 || z >= world.nz as i32 {
        return None;
    }
    let centre = near.round() as i32;
    for d in 0..=reach.max(0) {
        for y in [centre - d, centre + d] {
            if y > 0
                && y <= world.ny as i32
                && world.solid(x as u32, (y - 1) as u32, z as u32)
                && !world.solid(x as u32, y as u32, z as u32)
            {
                return Some(y as f32);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sole() -> Sole {
        Sole {
            probes: [-0.4, 0.0, 0.4]
                .into_iter()
                .flat_map(|x| [-0.25, 0.0, 0.25].map(|z| Vec3::new(x, -0.15, z)))
                .collect(),
        }
    }
    #[test]
    fn sloping_sole_matches_the_plane_without_penetration() {
        let s = sole();
        let floor = |x: f32, z: f32| 0.3 * x - 0.2 * z;
        let wanted = Vec3::new(-0.3, 1.0, 0.2).normalize();
        for yaw in [0.0, 0.7, 2.8] {
            let c = s.contact(Vec3::new(2.0, 0.0, 4.0), yaw, &floor);
            assert!((c.rot * Vec3::Y - wanted).length() < 1e-5);
            for p in &s.probes {
                let w = c.pos + c.rot * *p;
                assert!((w.y - floor(w.x, w.z)).abs() < 1e-5);
            }
        }
    }
    #[test]
    fn an_intermediate_ledge_is_cleared_with_a_continuous_arc() {
        let s = sole();
        let floor = |x: f32, _: f32| if (1.0..2.0).contains(&x) { 0.6 } else { 0.0 };
        let from = s.contact(Vec3::ZERO, 0.0, &floor);
        let to = s.contact(Vec3::X * 3.0, 0.0, &floor);
        let lift = s.clearance(from, to, 0.25, &floor);
        assert!(lift > 0.75);
        let mut old = from.pos;
        for i in 1..=1000 {
            let q = i as f32 / 1000.0;
            let mut c = from.lerp(to, q * q * (3.0 - 2.0 * q));
            c.pos.y += (q * std::f32::consts::PI).sin() * lift;
            for p in &s.probes {
                let w = c.pos + c.rot * *p;
                assert!(w.y + 1e-5 >= floor(w.x, w.z));
            }
            assert!(c.pos.distance(old) < 0.006);
            old = c.pos;
        }
        assert!(old.distance(to.pos) < 1e-5);
    }

    #[test]
    fn voxel_contacts_distinguish_small_steps_and_the_floor_of_a_tunnel() {
        struct Ground;
        impl voxel::Generator for Ground {
            fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
                if y < 8 + x % 3 || (20..24).contains(&y) {
                    1
                } else {
                    voxel::store::AIR
                }
            }
        }
        let w = voxel::World::new(32, 32, 32, 1, Box::new(Ground));
        assert_eq!(voxel_floor(&w, 3.2, 4.0, 9.0, 12), Some(8.0));
        assert_eq!(voxel_floor(&w, 5.2, 4.0, 9.0, 12), Some(10.0));
        assert_eq!(voxel_floor(&w, 3.2, 4.0, 23.0, 12), Some(24.0));
        assert_eq!(voxel_floor(&w, 3.2, 4.0, 16.0, 2), None);
        assert_eq!(voxel_floor(&w, -0.1, 4.0, 9.0, 12), None);
    }

    #[test]
    fn shipped_soles_use_occupied_cells_and_reproduce_the_rest_height() {
        let mut checked = 0;
        for (_, b) in voxel::roster::walking() {
            for leg in &b.legs {
                let Some(i) = leg.foot else {
                    continue;
                };
                let part = &b.segments[i as usize];
                let k = part.body.mm as f32 / voxel::unit::WORLD_MM as f32;
                let pivot = crate::model::pivot_centre(&part.attach);
                let sole = Sole::of(part, 1.0 / voxel::unit::WORLD_MM as f32);
                assert!((4..=9).contains(&sole.probes.len()));
                for p in &sole.probes {
                    let q = *p / k + Vec3::from_array(pivot);
                    assert!(part.body.vol.get(
                        q.x.floor() as usize,
                        q.y.round() as usize,
                        q.z.floor() as usize
                    ));
                }
                let c = sole.contact(Vec3::ZERO, 0.0, &|_, _| 0.0);
                assert!((c.pos.y - leg.ankle.1 as f32 * k).abs() < 1e-4);
                assert!(c.rot.angle_between(Quat::IDENTITY) < 1e-5);
                checked += 1;
            }
        }
        assert!(checked >= 4);
    }
}
