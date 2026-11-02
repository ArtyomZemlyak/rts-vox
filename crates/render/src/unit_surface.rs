//! Shared material maps in physical local coordinates. Generated once per
//! finish, with mipmaps; damage meshes retain the same surface coordinates.
use crate::{
    geom::MeshBuf,
    look::{self, Finish, Tile},
};
use bevy::prelude::*;

const PERIOD_METRES: f32 = 1.6;
fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(73856093) ^ (y as u32).wrapping_mul(19349663);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    (h & 65535) as f32 / 65535.0
}
fn noise(u: f32, v: f32, n: i32) -> f32 {
    let (x, y) = (u * n as f32, v * n as f32);
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (a, b) = (smooth(x - x.floor()), smooth(y - y.floor()));
    let h = |x: i32, y: i32| hash(x.rem_euclid(n), y.rem_euclid(n));
    let top = h(ix, iy) * (1.0 - a) + h(ix + 1, iy) * a;
    let bot = h(ix, iy + 1) * (1.0 - a) + h(ix + 1, iy + 1) * a;
    top * (1.0 - b) + bot * b
}
struct Sample {
    albedo: [f32; 3],
    rough: f32,
    metal: f32,
    height: f32,
}
fn sample(f: Finish, u: f32, v: f32) -> Sample {
    let broad = noise(u, v, 8);
    let grain = noise(u, v, 96);
    let scratches = ((u * 64.0 + noise(u, v, 8) * 0.18).fract() - 0.5).abs();
    let scratch = (1.0 - scratches * 42.0).max(0.0) * (noise(u, v, 16) - 0.55).max(0.0) * 2.2;
    let worn = ((noise(u, v, 32) - 0.77) * 2.8)
        .clamp(0.0, 0.55)
        .max(scratch * 0.28);
    let (shade, rough, metal, relief) = match f {
        Finish::Plate => (
            0.74 + broad * 0.17 - worn * 0.08,
            0.68 + grain * 0.12 - worn * 0.12,
            0.08 + worn * 0.50,
            grain * 0.08 - worn * 0.12,
        ),
        Finish::Gun => (
            0.80 + broad * 0.11 + scratch * 0.16,
            0.27 + grain * 0.14,
            0.96,
            grain * 0.035 - scratch * 0.18,
        ),
        Finish::Steel => (
            0.75 + broad * 0.17 + scratch * 0.18,
            0.42 + grain * 0.18 - scratch * 0.20,
            0.94,
            grain * 0.09 - scratch * 0.20,
        ),
        Finish::Rubber => (0.78 + grain * 0.16, 0.90 + grain * 0.08, 0.0, grain * 0.22),
        Finish::Optic => (0.98, 0.09, 0.0, 0.0),
        Finish::Core | Finish::Energy => (1.0, 0.40, 0.0, 0.0),
        Finish::Ivory => (
            0.90 + broad * 0.08 - worn * 0.04,
            0.52 + grain * 0.08,
            0.65 + worn * 0.20,
            grain * 0.04 - worn * 0.05,
        ),
    };
    Sample {
        albedo: [shade; 3],
        rough,
        metal,
        height: relief,
    }
}
pub fn maps(f: Finish) -> [Tile; 3] {
    let colour = look::build_tile(
        |u, v| sample(f, u, v).albedo.map(srgb_to_linear),
        linear_to_srgb,
    );
    let props = look::build_tile(
        |u, v| {
            let s = sample(f, u, v);
            [1.0, s.rough, s.metal]
        },
        |v| v,
    );
    let normal = look::build_normal_tile(|u, v| {
        let e = 1.0 / look::TILE as f32;
        let dx = sample(f, u + e, v).height - sample(f, u - e, v).height;
        let dy = sample(f, u, v + e).height - sample(f, u, v - e).height;
        Vec3::new(-dx, -dy, 1.0).normalize().to_array()
    });
    [colour, props, normal]
}
fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Project onto each face's dominant plane, with a tangent orthogonal to its
/// actual bevel normal. Coordinates travel with the part, never with the world.
pub fn map(buf: &mut MeshBuf, start: usize, metres_per_unit: f32) {
    for i in start..buf.pos.len() {
        let n = Vec3::from_array(buf.nrm[i]);
        let an = n.abs();
        let (u, v) = if an.x >= an.y && an.x >= an.z {
            (Vec3::Z, Vec3::Y)
        } else if an.y >= an.z {
            (Vec3::X, Vec3::Z)
        } else {
            (Vec3::X, Vec3::Y)
        };
        let pos = Vec3::from_array(buf.pos[i]) * metres_per_unit / PERIOD_METRES;
        let tangent = (u - n * n.dot(u)).normalize();
        let handedness = n.cross(tangent).dot(v).signum();
        buf.uv.push([pos.dot(u), pos.dot(v)]);
        buf.tan.push(tangent.extend(handedness).to_array());
    }
}
pub fn materials(
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> [Handle<StandardMaterial>; look::FINISHES] {
    look::FINISH_ALL.map(|f| {
        let [colour, props, normal] = maps(f);
        let colour = images.add(crate::tex::tile_image(colour, true));
        let props = images.add(crate::tex::tile_image(props, false));
        let normal = images.add(crate::tex::tile_image(normal, false));
        let glow = look::finish_glow(f);
        materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(colour),
            metallic: 1.0,
            perceptual_roughness: 1.0,
            metallic_roughness_texture: Some(props),
            normal_map_texture: Some(normal),
            emissive: LinearRgba::rgb(glow[0], glow[1], glow[2]),
            ..default()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surfaces_tile_and_keep_dielectrics_distinct_from_bare_metal() {
        for f in look::FINISH_ALL {
            for v in [0.0, 0.2, 0.51, 0.91] {
                assert!((sample(f, 0.0, v).height - sample(f, 1.0, v).height).abs() < 0.00001);
            }
        }
        assert!(sample(Finish::Rubber, 0.3, 0.4).rough > 0.85);
        assert_eq!(sample(Finish::Rubber, 0.3, 0.4).metal, 0.0);
        assert!(sample(Finish::Steel, 0.3, 0.4).metal > 0.9);
        assert!(sample(Finish::Optic, 0.3, 0.4).rough < 0.12);
    }
    #[test]
    fn texture_scale_and_tangents_survive_body_scale_and_damage() {
        let mut b = voxel::unit::tank();
        for z in 0..10 {
            for y in 0..12 {
                for x in 0..18 {
                    b.remove(x, y, z);
                }
            }
        }
        let mut a = std::array::from_fn(|_| MeshBuf::default());
        let mut c = std::array::from_fn(|_| MeshBuf::default());
        let centre = crate::model::root_centre(&b);
        for (scale, out) in [(1.0, &mut a), (0.1, &mut c)] {
            crate::model::body_to_mesh(
                &b,
                [0.2; 3],
                scale,
                (0, usize::MAX),
                crate::model::BEVEL,
                centre,
                out,
            );
        }
        for (a, c) in a.iter().zip(&c) {
            assert_eq!(a.uv.len(), a.pos.len());
            assert_eq!(a.tan.len(), a.pos.len());
            for (u, v) in a.uv.iter().zip(&c.uv) {
                assert!((u[0] - v[0]).abs() < 0.00001 && (u[1] - v[1]).abs() < 0.00001);
            }
            for (n, t) in a.nrm.iter().zip(&a.tan) {
                let n = Vec3::from_array(*n);
                let t3 = Vec3::new(t[0], t[1], t[2]);
                assert!(n.dot(t3).abs() < 0.00001);
                assert!((t3.length_squared() - 1.0).abs() < 0.00001);
                assert_eq!(t[3].abs(), 1.0);
            }
        }
    }
}
