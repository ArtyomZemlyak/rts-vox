//! Strategic cloud silhouette, independent of gas solver resolution and damage.
use super::*;
#[path = "nuclear/cloud.rs"]
mod cloud;
#[path = "nuclear/shock.rs"]
mod shock;
pub(super) use shock::Front as ShockFront;
#[path = "nuclear/volume.rs"]
mod volume;

pub(super) const CAP: usize = 4;
pub(super) const LIFE: f32 = 26.0;

/// Separate lift and lateral roll-out: a rising fireball becomes a mushroom
/// over several seconds instead of revealing its final silhouette at birth.
#[derive(Clone, Copy)]
struct Development {
    height: f32,
    radius: f32,
    grow: f32,
    crown: f32,
}
impl Development {
    fn at(age: f32) -> Self {
        let lift = (age - 0.35).max(0.0);
        let grow = 1.0 - (-lift * 0.34).exp();
        Self {
            height: 70.0 + 1600.0 * grow + age * 12.0,
            radius: 60.0 + 720.0 * (1.0 - (-age * 0.32).exp()),
            grow,
            crown: smooth01((age - 1.8) / 4.2),
        }
    }
    fn fireball_height(self) -> f32 {
        self.height * (0.72 + self.crown * 0.28)
    }
}

pub(super) struct Illumination {
    pub colour: [f32; 3],
    pub intensity: f32,
    pub height: f32,
}

/// A short white pulse, a growing luminous ball, then cooling. Share its
/// position with the visible fireball instead of leaving a light in the stem.
pub(super) fn illumination(age: f32) -> Illumination {
    let age = age.max(0.0);
    let thermal = smooth01(age / 0.30) * (1.0 - smooth01((age - 0.6) / 4.9));
    Illumination {
        colour: Vec3::new(1.0, 0.94, 0.78)
            .lerp(Vec3::new(1.0, 0.38, 0.075), smooth01(age / 1.4))
            .to_array(),
        intensity: if age < 5.5 {
            80_000_000_000.0 * (-age * 18.0).exp() + 32_000_000_000.0 * thermal * thermal
        } else {
            0.0
        },
        height: Development::at(age).fireball_height(),
    }
}

/// A voxelized volume with exposed axis-aligned faces. Equal world edges
/// preserve the range's cubic realism, including the flash and dust skirt.
fn billow(buf: &mut MeshBuf, at: Vec3, size: Vec3, colour: [f32; 4]) {
    let steps = if size.max_element() > 500.0 {
        15.0
    } else if size.max_element() > 250.0 {
        9.0
    } else {
        5.0
    };
    let edge = size.max_element() * 2.0 / steps;
    let dims = (size * 2.0 / edge).ceil().as_ivec3().max(IVec3::ONE);
    let origin = at - dims.as_vec3() * edge * 0.5;
    let occupied = |q: IVec3| {
        if q.cmplt(IVec3::ZERO).any() || q.cmpge(dims).any() {
            return false;
        }
        let centre = origin + (q.as_vec3() + Vec3::splat(0.5)) * edge;
        ((centre - at) / size).length_squared() <= 1.0
    };
    for z in 0..dims.z {
        for y in 0..dims.y {
            for x in 0..dims.x {
                let q = IVec3::new(x, y, z);
                if !occupied(q) {
                    continue;
                }
                let centre = origin + (q.as_vec3() + Vec3::splat(0.5)) * edge;
                for axis in 0..3 {
                    for sign in [-1, 1] {
                        let mut neighbour = q;
                        neighbour[axis] += sign;
                        if occupied(neighbour) {
                            continue;
                        }
                        let mut normal = Vec3::ZERO;
                        normal[axis] = sign as f32;
                        let mut a = Vec3::ZERO;
                        let mut b = Vec3::ZERO;
                        a[(axis + 1) % 3] = edge * 0.5;
                        b[(axis + 2) % 3] = edge * 0.5 * sign as f32;
                        let face = centre + normal * edge * 0.5;
                        let start = buf.pos.len() as u32;
                        let shade = if axis == 1 && sign > 0 {
                            1.0
                        } else if axis == 0 {
                            0.82
                        } else {
                            0.66
                        };
                        buf.pos.extend(
                            [face - a - b, face + a - b, face + a + b, face - a + b]
                                .map(|v| v.to_array()),
                        );
                        buf.nrm.extend([normal.to_array(); 4]);
                        let tint = [
                            colour[0] * shade,
                            colour[1] * shade,
                            colour[2] * shade,
                            colour[3],
                        ];
                        buf.col.extend([tint; 4]);
                        buf.idx
                            .extend([start, start + 1, start + 2, start, start + 2, start + 3]);
                    }
                }
            }
        }
    }
}

pub(super) fn append(buf: &mut MeshBuf, effects: &Effects, now: f32, smoke: bool) {
    for &(at, born) in &effects.nuclear {
        let front = effects
            .nuclear_fronts
            .iter()
            .find(|f| f.at == at && f.born == born);
        append_one(buf, at, born, now, smoke, front, effects.nuclear_pixel);
    }
}

pub(super) fn append_smoke(
    buf: &mut MeshBuf,
    effects: &Effects,
    now: f32,
    pool: Option<&bevy::tasks::TaskPool>,
) {
    let active = effects
        .nuclear
        .iter()
        .filter(|(_, born)| now - born < LIFE)
        .count();
    let Some(pool) = pool.filter(|p| p.thread_num() > 1 && active > 1) else {
        append(buf, effects, now, true);
        return;
    };
    // Each bounded event owns its scratch volume and mesh. Completion order
    // never determines transparent geometry order, even inside a layer job.
    let mut chunks = pool.scope(|scope| {
        for (index, &(at, born)) in effects.nuclear.iter().enumerate() {
            let front = effects
                .nuclear_fronts
                .iter()
                .find(|f| f.at == at && f.born == born);
            scope.spawn(async move {
                let mut part = MeshBuf::default();
                append_one(&mut part, at, born, now, true, front, effects.nuclear_pixel);
                (index, part)
            });
        }
    });
    chunks.sort_unstable_by_key(|(index, _)| *index);
    let vertices: usize = chunks.iter().map(|(_, part)| part.pos.len()).sum();
    let indices: usize = chunks.iter().map(|(_, part)| part.idx.len()).sum();
    buf.pos.reserve(vertices);
    buf.nrm.reserve(vertices);
    buf.col.reserve(vertices);
    buf.idx.reserve(indices);
    for (_, mut part) in chunks {
        let base = buf.pos.len() as u32;
        buf.pos.append(&mut part.pos);
        buf.nrm.append(&mut part.nrm);
        buf.col.append(&mut part.col);
        buf.idx.extend(part.idx.into_iter().map(|i| i + base));
    }
}

fn append_one(
    buf: &mut MeshBuf,
    at: Vec3,
    born: f32,
    now: f32,
    smoke: bool,
    front: Option<&ShockFront>,
    pixel: f32,
) {
    let age = (now - born).max(0.0);
    if age >= LIFE {
        return;
    }
    let shape = Development::at(age);
    let Development {
        height,
        radius,
        grow,
        crown,
    } = shape;
    let fade = 1.0 - smooth01((age - 17.0) / 9.0);
    if !smoke {
        // The initial white pulse gives way to a sustained orange fireball.
        if age < 5.5 {
            let r = 55.0 + 390.0 * (1.0 - (-age * 2.0).exp());
            let rgb = Vec3::new(28.0, 22.0, 12.0)
                .lerp(Vec3::new(7.0, 1.1, 0.05), smooth01(age / 0.85))
                .lerp(Vec3::new(3.0, 0.14, 0.008), smooth01(age / 5.5));
            let alpha = 1.0 - smooth01((age - 2.2) / 3.3);
            billow(
                buf,
                at + Vec3::Y * shape.fireball_height(),
                Vec3::splat(r * (1.0 - smooth01((age - 1.6) / 3.9) * 0.22)),
                rgb.extend(alpha).to_array(),
            );
            // A brief low thermal flash opens out across the ground.
            // The old spherical halo lingered like a transparent shell.
            if age < 1.25 {
                billow(
                    buf,
                    at + Vec3::Y * 60.0,
                    Vec3::new(r * 1.6, r * 0.20, r * 1.6),
                    [18.0, 11.0, 3.0, (1.0 - smooth01(age / 1.25)) * 0.14],
                );
            }
        }
        if age > 1.8 && age < 7.0 {
            for i in 0..12u32 {
                let forming = smooth01((age - 1.8 - noise(i + 711) * 1.2) / 2.4);
                if forming <= 0.01 {
                    continue;
                }
                let angle = i as f32 * 2.399963 + age * 0.018;
                let radial = Vec3::new(angle.cos(), 0.0, angle.sin());
                let r = radius * (0.22 + crown * 0.61) * (0.65 + noise(i + 91) * 0.35);
                let heat = 1.0 - smooth01((age - 1.8) / 5.2);
                billow(
                    buf,
                    at + Vec3::Y * (height - radius * (0.22 + noise(i) * 0.16)) + radial * r,
                    Vec3::new(160.0, 125.0, 160.0) * (0.3 + grow) * forming,
                    [
                        2.8 + heat * 2.0,
                        0.15 + heat * 0.70,
                        0.025,
                        heat * forming * 0.40,
                    ],
                );
            }
        }
        return;
    }
    if let Some(front) = front {
        shock::append(buf, front, now, fade, pixel);
    }
    cloud::append(buf, at, age, shape, fade, pixel);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nuclear_light_has_a_short_white_pulse_then_cools_with_its_fireball() {
        assert!(illumination(0.0).intensity > illumination(0.3).intensity);
        assert!(illumination(0.3).intensity > illumination(0.12).intensity);
        let mut previous = illumination(0.3).intensity;
        for step in 3..=60 {
            let age = step as f32 * 0.1;
            let sample = illumination(age);
            assert!(sample.intensity <= previous * 1.00001);
            assert_eq!(sample.height, Development::at(age).fireball_height());
            assert!(sample
                .colour
                .iter()
                .all(|c| c.is_finite() && (0.0..=1.0).contains(c)));
            previous = sample.intensity;
        }
        assert_eq!(illumination(5.5).intensity, 0.0);
        assert!(illumination(0.0).colour[1] > illumination(2.0).colour[1]);
    }
    #[test]
    fn cooled_cloud_keeps_smoke_without_lingering_orange_fireballs() {
        let mut effects = Effects::default();
        effects.nuclear_cloud(Vec3::ZERO, 0.0, 2048.0);
        let mut hot = MeshBuf::default();
        let mut smoke = MeshBuf::default();
        append(&mut hot, &effects, 8.0, false);
        append(&mut smoke, &effects, 8.0, true);
        assert!(hot.is_empty() && !smoke.is_empty());
        assert_eq!(illumination(8.0).intensity, 0.0);
    }
    #[test]
    fn distant_cloud_reduces_geometry_but_keeps_stages_and_cubic_faces() {
        let mut effects = Effects::default();
        effects.nuclear_cloud(Vec3::ZERO, 0.0, 2048.0);
        for age in [2.0, 6.0, 12.0, 24.0] {
            let mut near = MeshBuf::default();
            append(&mut near, &effects, age, true);
            effects.nuclear_pixel = 100.0;
            let mut far = MeshBuf::default();
            append(&mut far, &effects, age, true);
            effects.nuclear_pixel = 0.0;
            assert!(
                far.pos.len() < near.pos.len() * 3 / 4,
                "age {age}: {} / {}",
                far.pos.len(),
                near.pos.len()
            );
            assert!(!far.is_empty());
            assert!(far
                .nrm
                .iter()
                .all(|n| n.iter().filter(|&&v| v != 0.0).count() == 1));
            let top = |b: &MeshBuf| b.pos.iter().map(|p| p[1]).fold(0.0, f32::max);
            assert!((top(&near) - top(&far)).abs() < 150.0);
            assert!(far.col.iter().all(|c| c[3] <= 0.32));
        }
    }
    #[test]
    fn cloud_has_a_broad_cap_tall_stem_and_finite_lifetime() {
        let mut effects = Effects::default();
        effects.nuclear_cloud(Vec3::ZERO, 0.0, 2048.0);
        let mut buf = MeshBuf::default();
        append(&mut buf, &effects, 8.0, true);
        assert!(buf.pos.iter().any(|p| p[1] > 1500.0));
        assert!(buf.pos.iter().any(|p| p[0].abs() > 650.0 && p[1] > 1200.0));
        assert!(buf
            .pos
            .iter()
            .any(|p| p[1] > 400.0 && p[1] < 600.0 && p[0].abs() < 150.0));
        assert!(buf.pos.len() < 90_000);
        assert!(buf
            .nrm
            .iter()
            .all(|n| n.iter().filter(|&&v| v != 0.0).count() == 1));
        assert!(buf.col.iter().flatten().all(|v| v.is_finite()));
        let mut expired = MeshBuf::default();
        append(&mut expired, &effects, LIFE, true);
        assert!(expired.is_empty());
    }
    #[test]
    fn crown_rolls_out_after_the_initial_fireball_and_smoke_stays_translucent() {
        assert_eq!(Development::at(0.0).crown, 0.0);
        assert_eq!(Development::at(1.8).crown, 0.0);
        assert!(Development::at(3.0).crown < 0.25);
        assert_eq!(Development::at(6.0).crown, 1.0);
        let mut effects = Effects::default();
        effects.nuclear_cloud(Vec3::ZERO, 0.0, 2048.0);
        let mut widths = Vec::new();
        for age in [1.0, 3.0, 6.0, 12.0] {
            let mut cloud = MeshBuf::default();
            append(&mut cloud, &effects, age, true);
            let top = Development::at(age).height * 0.75;
            widths.push(
                cloud
                    .pos
                    .iter()
                    .filter(|p| p[1] > top)
                    .map(|p| p[0].abs())
                    .fold(0.0, f32::max),
            );
            assert!(cloud.col.iter().all(|c| c[3] <= 0.32));
        }
        assert!(widths[0] < widths[2] * 0.30, "{widths:?}");
        assert!(widths[1] < widths[2] * 0.85, "{widths:?}");
        assert!(widths[2] < widths[3], "{widths:?}");
    }
    #[test]
    fn cloud_geometry_remains_bounded_finite_and_cubic_through_growth_and_fade() {
        let mut effects = Effects::default();
        effects.nuclear_cloud(Vec3::ZERO, 0.0, 2048.0);
        for age in [
            0.0, 0.1, 0.5, 1.0, 2.0, 4.0, 6.0, 8.5, 9.0, 12.0, 18.0, 25.9, 26.0,
        ] {
            for smoke in [false, true] {
                let mut b = MeshBuf::default();
                append(&mut b, &effects, age, smoke);
                assert!(
                    b.pos.len() < 90_000,
                    "age={age} smoke={smoke} vertices={}",
                    b.pos.len()
                );
                assert!(b.pos.iter().flatten().all(|v| v.is_finite()));
                assert!(b
                    .col
                    .iter()
                    .all(|c| c.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&c[3])));
                assert!(b
                    .nrm
                    .iter()
                    .all(|n| n.iter().filter(|&&v| v != 0.0).count() == 1));
            }
        }
    }
}
