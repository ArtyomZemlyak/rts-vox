//! Bounded visual envelope of observed damage, never an authoritative wave.
use super::{volume::Volume, *};

const SECTORS: usize = 8;

#[derive(Clone, Copy)]
pub(crate) struct Front {
    pub at: Vec3,
    pub born: f32,
    reach: f32,
    from: [f32; SECTORS],
    to: [f32; SECTORS],
    changed: [f32; SECTORS],
}
impl Front {
    pub fn new(at: Vec3, born: f32, reach: f32) -> Self {
        Self {
            at,
            born,
            reach,
            from: [0.0; SECTORS],
            to: [0.0; SECTORS],
            changed: [born; SECTORS],
        }
    }
    pub fn accepts(&self, at: Vec3, now: f32) -> bool {
        let d = at - self.at;
        now >= self.born
            && now - self.born < 10.0
            && d.x * d.x + d.z * d.z <= self.reach * self.reach
            && d.y.abs() <= self.reach
    }
    fn radius(&self, sector: usize, now: f32) -> f32 {
        self.from[sector]
            + (self.to[sector] - self.from[sector])
                * smooth01((now - self.changed[sector]) * sim_core::TICKS_PER_SECOND as f32)
    }
    pub fn observe(&mut self, at: Vec3, now: f32) {
        if !self.accepts(at, now) {
            return;
        }
        let d = at - self.at;
        let radius = Vec2::new(d.x, d.z).length();
        if radius < 1.0 {
            return;
        }
        let sector = ((d.z.atan2(d.x) / std::f32::consts::TAU * SECTORS as f32).round() as i32)
            .rem_euclid(SECTORS as i32) as usize;
        if radius > self.to[sector] + 0.5 {
            self.from[sector] = self.radius(sector, now);
            self.to[sector] = radius.min(self.reach);
            self.changed[sector] = now;
        }
    }
    fn ring(&self, now: f32) -> Option<Ring> {
        let min = (0..SECTORS)
            .filter(|i| self.to[*i] > 0.0)
            .map(|i| self.radius(i, now))
            .reduce(f32::min)?;
        let radius = std::array::from_fn(|i| {
            if self.to[i] > 0.0 {
                self.radius(i, now)
            } else {
                min
            }
        });
        let lead = std::array::from_fn(|i| {
            if self.to[i] > 0.0 {
                1.0 - smooth01((now - self.changed[i]) / 0.55)
            } else {
                0.0
            }
        });
        let wake = std::array::from_fn(|i| {
            if self.to[i] > 0.0 {
                1.0 - smooth01((now - self.changed[i]) / 2.0)
            } else {
                0.0
            }
        });
        wake.iter()
            .any(|v| *v > 0.001)
            .then_some(Ring { radius, lead, wake })
    }
}

struct Ring {
    radius: [f32; SECTORS],
    lead: [f32; SECTORS],
    wake: [f32; SECTORS],
}
impl Ring {
    fn along(&self, angle: f32) -> (f32, f32, f32) {
        let q = (angle / std::f32::consts::TAU * SECTORS as f32).rem_euclid(SECTORS as f32);
        // f32 rem_euclid can round a tiny negative angle to the divisor.
        let i = q.floor() as usize % SECTORS;
        let j = (i + 1) % SECTORS;
        let t = smooth01(q.fract());
        let mix = |v: &[f32; SECTORS]| v[i] + (v[j] - v[i]) * t;
        (mix(&self.radius), mix(&self.lead), mix(&self.wake))
    }
}

pub(super) fn append(buf: &mut MeshBuf, front: &Front, now: f32, fade: f32, pixel: f32) {
    let Some(ring) = front.ring(now) else {
        return;
    };
    let radius = ring.radius.iter().copied().fold(0.0, f32::max);
    if radius < 1.0 {
        return;
    }
    // A thin compression crest and low rolling dust share one cubic surface.
    // A distant ring keeps its radius but samples fewer subpixel cubes.
    let width = (radius * 0.065).clamp(12.0, 110.0);
    let full_edge = (radius + width * 1.5) / 24.0;
    let side = if pixel > full_edge { 25 } else { 49 };
    let half = (side - 1) as f32 * 0.5;
    let edge = (radius + width * 1.5) / half;
    let width = width.max(edge * 1.3);
    let origin = front.at + Vec3::new(-(half + 0.5) * edge, 0.0, -(half + 0.5) * edge);
    let mut surface = Volume::new(origin, edge, IVec3::new(side, 7, side));
    let profile = |p: Vec3| {
        let d = p - front.at;
        let angle = d.z.atan2(d.x);
        let (r, lead, wake) = ring.along(angle);
        let distance = Vec2::new(d.x, d.z).length();
        let crest = (1.0 - (distance - r).abs() / width).max(0.0);
        let trail = (1.0 - ((distance - r) / width + 1.5).abs() / 1.5).max(0.0);
        let roll = if trail * wake > 0.0 {
            0.78 + 0.22 * (distance / width - now * 1.8 + angle * 5.0).sin()
        } else {
            1.0
        };
        (crest, trail, lead, wake, roll)
    };
    // Angular interpolation, distance and profiles depend only on X/Z.
    // Share them across heights and exposed faces of the same column.
    let mut profiles = Vec::with_capacity((side * side) as usize);
    for z in 0..side {
        for x in 0..side {
            profiles.push(profile(
                origin + Vec3::new(x as f32 + 0.5, 0.0, z as f32 + 0.5) * edge,
            ));
        }
    }
    surface.fill_columns(|p| {
        let x = ((p.x - origin.x) / edge) as usize;
        let z = ((p.z - origin.z) / edge) as usize;
        let (crest, trail, lead, wake, roll) = profiles[z * side as usize + x];
        front.at.y + (crest * crest * lead * width * 0.85).max(trail * wake * width * 0.8 * roll)
    });
    surface.append(buf, |_, q| {
        let (crest, trail, lead, wake, _) = profiles[(q.z * side + q.x) as usize];
        let compression = crest * crest * lead;
        let rgb = Vec3::new(0.27, 0.21, 0.145).lerp(Vec3::new(0.95, 0.96, 0.94), compression);
        rgb.extend(fade * (compression * 0.20 + trail * wake * 0.18))
            .to_array()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distant_dust_front_uses_fewer_cubes_and_keeps_the_observed_ring() {
        let mut f = Front::new(Vec3::ZERO, 0.0, 2048.0);
        for i in 0..SECTORS {
            let angle = i as f32 * std::f32::consts::TAU / SECTORS as f32;
            f.observe(Vec3::new(angle.cos(), 0.0, angle.sin()) * 1000.0, 1.0);
        }
        let mut near = MeshBuf::default();
        let mut far = MeshBuf::default();
        append(&mut near, &f, 1.06, 1.0, 0.0);
        append(&mut far, &f, 1.06, 1.0, 100.0);
        assert!(!far.is_empty() && far.pos.len() < near.pos.len() * 3 / 4);
        let extent = |b: &MeshBuf| b.pos.iter().map(|p| p[0].abs()).fold(0.0, f32::max);
        assert!((extent(&far) - extent(&near)).abs() < 150.0);
        assert!(far.col.iter().all(|c| c[3] <= 0.38));
    }
    #[test]
    fn observed_front_is_monotone_bounded_and_interpolates_for_one_tick() {
        let mut f = Front::new(Vec3::ZERO, 2.0, 2048.0);
        assert!(f.ring(2.0).is_none());
        f.observe(Vec3::X * 100.0, 2.0);
        assert_eq!(f.radius(0, 2.0), 0.0);
        let halfway = f.radius(0, 2.0 + 0.5 / sim_core::TICKS_PER_SECOND as f32);
        assert!((49.9..50.1).contains(&halfway));
        f.observe(Vec3::X * 80.0, 2.02);
        assert_eq!(f.to[0], 100.0);
        f.observe(Vec3::X * 3000.0, 2.03);
        assert_eq!(f.to[0], 100.0);
        assert!(!f.accepts(Vec3::X, 12.0));
        assert!(f.ring(4.1).is_none());
    }
    #[test]
    fn damage_sectors_keep_separate_fronts_without_retreat_or_an_instant_full_ring() {
        let mut f = Front::new(Vec3::ZERO, 0.0, 2048.0);
        f.observe(Vec3::X * 800.0, 1.0);
        f.observe(Vec3::Z * 240.0, 1.0);
        let ring = f
            .ring(1.0 + 1.1 / sim_core::TICKS_PER_SECOND as f32)
            .unwrap();
        assert_eq!(ring.along(0.0).0, 800.0);
        assert_eq!(ring.along(std::f32::consts::FRAC_PI_2).0, 240.0);
        assert_eq!(ring.along(std::f32::consts::PI).1, 0.0);
        for angle in [
            -f32::EPSILON * 0.01,
            -std::f32::consts::TAU,
            std::f32::consts::TAU,
        ] {
            let (r, lead, wake) = ring.along(angle);
            assert!(r.is_finite() && lead.is_finite() && wake.is_finite());
        }
        let mut b = MeshBuf::default();
        append(&mut b, &f, 1.04, 1.0, 0.0);
        assert!(!b.is_empty());
        assert!(b.pos.len() < 30_000);
        assert!(b
            .col
            .iter()
            .all(|c| c.iter().all(|v| v.is_finite()) && c[3] <= 0.52));
        assert!(b
            .nrm
            .iter()
            .all(|n| n.iter().filter(|&&v| v != 0.0).count() == 1));
    }
}
