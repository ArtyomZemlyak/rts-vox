//! Reuse the editor's cached real template meshes for ghosts and catalogue images.
use super::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use modding::laboratory::catalog::Element;
use workbench::ghost::{Ghost, Templates};

impl Laboratory {
    pub fn ghost_pose(&self, ghost: &Ghost) -> Option<(Transform, bool)> {
        let c = &self.constructor;
        c.hover?;
        if !matches!(
            c.mode,
            constructor::Mode::Place | constructor::Mode::Part | constructor::Mode::Move
        ) {
            return None;
        }
        let p = c.preview.as_ref()?;
        let g = &p.geometry;
        let pose = match ghost {
            Ghost::Block(slot) => {
                let b = g.blocks.get(*slot)?;
                let lo = Vec3::from(b.min.map(|v| v as f32));
                let hi = Vec3::from(b.max.map(|v| v as f32));
                Transform::from_translation((lo + hi) * 0.5).with_scale(hi - lo)
            }
            Ghost::Shield(slot) => {
                let s = g.shields.get(*slot)?;
                Transform::from_translation(Vec3::from(s.at.map(|v| v as f32)))
                    .with_scale(Vec3::splat(s.radius as f32))
            }
            Ghost::Unit(kind) | Ghost::UnitCopy(kind, _) => {
                let slot = if let Ghost::UnitCopy(_, slot) = ghost {
                    *slot
                } else {
                    0
                };
                let (i, u) = g
                    .units
                    .iter()
                    .enumerate()
                    .filter(|(_, u)| u.kind == *kind)
                    .nth(slot)?;
                Transform::from_translation(Vec3::from(u.at.map(|v| v as f32))).with_rotation(
                    Quat::from_rotation_y(-(g.unit_yaw(i) as f32) * std::f32::consts::FRAC_PI_2),
                )
            }
        };
        Some((pose, p.issue.is_some()))
    }
}

pub fn bounds(layout: &RangeLayout, templates: &Templates) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for b in &layout.blocks {
        if b.material == 0 {
            continue;
        }
        lo = lo.min(Vec3::from(b.min.map(|v| v as f32)));
        hi = hi.max(Vec3::from(b.max.map(|v| v as f32)));
    }
    for (i, u) in layout.units.iter().enumerate() {
        if templates.0.iter().any(|(kind, _)| *kind == u.kind) {
            let t = Transform::from_translation(Vec3::from(u.at.map(|v| v as f32))).with_rotation(
                Quat::from_rotation_y(-(layout.unit_yaw(i) as f32) * std::f32::consts::FRAC_PI_2),
            );
            let (min, max) = templates.bounds(u.kind);
            for x in [min.x, max.x] {
                for y in [min.y, max.y] {
                    for z in [min.z, max.z] {
                        let p = t.transform_point(Vec3::new(x, y, z));
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
            }
        }
    }
    (lo, hi)
}
pub fn outline(layout: &RangeLayout, templates: &Templates, gizmos: &mut Gizmos, colour: Color) {
    let (lo, hi) = bounds(layout, templates);
    if lo.is_finite() && hi.is_finite() {
        gizmos.cube(
            Transform::from_translation((lo + hi) * 0.5).with_scale(hi - lo + Vec3::splat(0.7)),
            colour,
        );
    }
    for s in &layout.shields {
        gizmos.sphere(
            Isometry3d::from_translation(Vec3::from(s.at.map(|v| v as f32))),
            s.radius as f32,
            colour,
        );
    }
}

const W: usize = 160;
const H: usize = 112;
fn tint(material: u16) -> [f32; 3] {
    match material {
        1 | 8 => [0.52, 0.43, 0.32],
        10 => [0.76, 0.61, 0.36],
        12 => [0.37, 0.61, 0.51],
        11 => [0.16, 0.61, 0.80],
        2 => [0.65, 0.66, 0.63],
        3 => [0.39, 0.57, 0.70],
        4 => [0.26, 0.58, 0.31],
        5 => [0.53, 0.35, 0.18],
        6 => [0.50, 0.52, 0.56],
        _ => [0.48, 0.70, 0.86],
    }
}
/// Orthographic thumbnail of actual authored cuboids and full unit template meshes.
/// Called once per asset, one asset per frame; never creates or ticks a SIM.
pub fn thumbnail(element: &Element, templates: &Templates) -> Image {
    let mut vertices: Vec<(Vec3, [f32; 3])> = Vec::new();
    let mut triangles: Vec<[usize; 3]> = Vec::new();
    for b in &element.blocks {
        if b.material == 0 {
            continue;
        }
        let start = vertices.len();
        for bits in 0..8 {
            vertices.push((
                Vec3::new(
                    if bits & 1 == 0 { b.min[0] } else { b.max[0] } as f32,
                    if bits & 2 == 0 { b.min[1] } else { b.max[1] } as f32,
                    if bits & 4 == 0 { b.min[2] } else { b.max[2] } as f32,
                ),
                tint(b.material),
            ));
        }
        for [a, b, c] in [
            [0, 2, 3],
            [0, 3, 1],
            [4, 5, 7],
            [4, 7, 6],
            [0, 1, 5],
            [0, 5, 4],
            [2, 6, 7],
            [2, 7, 3],
            [0, 4, 6],
            [0, 6, 2],
            [1, 3, 7],
            [1, 7, 5],
        ] {
            triangles.push([start + a, start + b, start + c]);
        }
    }
    for (i, u) in element.units.iter().enumerate() {
        if let Some((_, mesh)) = templates.0.iter().find(|(kind, _)| *kind == u.kind) {
            let start = vertices.len();
            let yaw = element.unit_yaws.get(i).copied().unwrap_or(0);
            let q = Quat::from_rotation_y(-(yaw as f32) * std::f32::consts::FRAC_PI_2);
            let at = Vec3::from(u.at.map(|v| v as f32));
            vertices.extend(
                mesh.pos
                    .iter()
                    .map(|p| (at + q * Vec3::from(*p), [0.45, 0.64, 0.79])),
            );
            triangles.extend(mesh.idx.chunks_exact(3).map(|t| {
                [
                    start + t[0] as usize,
                    start + t[1] as usize,
                    start + t[2] as usize,
                ]
            }));
        }
    }
    // Shields are a real spherical volume, represented by three great-circle strips.
    for s in &element.shields {
        let at = Vec3::from(s.at.map(|v| v as f32));
        for axis in 0..3 {
            for n in 0..32 {
                let start = vertices.len();
                for (k, r) in [(n, 0.97), (n, 1.0), (n + 1, 0.97), (n + 1, 1.0)] {
                    let angle = k as f32 * std::f32::consts::TAU / 32.0;
                    let mut p = Vec3::ZERO;
                    p[(axis + 1) % 3] = angle.cos() * s.radius as f32 * r;
                    p[(axis + 2) % 3] = angle.sin() * s.radius as f32 * r;
                    vertices.push((at + p, [0.35, 0.80, 0.95]));
                }
                triangles.extend([
                    [start, start + 1, start + 2],
                    [start + 1, start + 3, start + 2],
                ]);
            }
        }
    }
    let right = Vec3::new(0.707, 0.0, 0.707);
    let up = Vec3::new(0.35, 0.87, -0.35);
    let front = right.cross(up).normalize();
    let project = |p: Vec3| Vec3::new(p.dot(right), -p.dot(up), p.dot(front));
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for (p, _) in &vertices {
        let p = project(*p);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let span = hi - lo;
    let scale = ((W - 24) as f32 / span.x.max(1.0)).min((H - 20) as f32 / span.y.max(1.0));
    let centre = (lo + hi) * 0.5;
    let screen: Vec<_> = vertices
        .iter()
        .map(|(p, _)| {
            let v = project(*p) - centre;
            Vec3::new(
                v.x * scale + W as f32 * 0.5,
                v.y * scale + H as f32 * 0.5,
                v.z,
            )
        })
        .collect();
    let mut pixels = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            let v = 20 + (y * 10 / H) as u8;
            pixels[i..i + 4].copy_from_slice(&[v, v + 9, v + 17, 255]);
        }
    }
    let mut depths = vec![f32::NEG_INFINITY; W * H];
    for [a, b, c] in triangles {
        let [p, q, r] = [screen[a], screen[b], screen[c]];
        let cross =
            |a: Vec3, b: Vec3, c: Vec3| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        let area = cross(p, q, r);
        if area.abs() < 0.0001 {
            continue;
        }
        let normal = (vertices[b].0 - vertices[a].0)
            .cross(vertices[c].0 - vertices[a].0)
            .normalize_or_zero();
        let shade = 0.54 + 0.46 * normal.dot(Vec3::new(-0.3, 0.85, 0.4).normalize()).abs();
        let colour = vertices[a]
            .1
            .map(|v| (v * shade * 255.0).clamp(0.0, 255.0) as u8);
        let min = p.min(q).min(r);
        let max = p.max(q).max(r);
        for y in (min.y.floor().max(0.0) as usize)..=(max.y.ceil().min((H - 1) as f32) as usize) {
            for x in (min.x.floor().max(0.0) as usize)..=(max.x.ceil().min((W - 1) as f32) as usize)
            {
                let s = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, 0.0);
                let u = cross(q, r, s) / area;
                let v = cross(r, p, s) / area;
                let w = 1.0 - u - v;
                if u < -0.001 || v < -0.001 || w < -0.001 {
                    continue;
                }
                let d = u * p.z + v * q.z + w * r.z;
                let i = y * W + x;
                if d > depths[i] {
                    depths[i] = d;
                    pixels[i * 4..i * 4 + 3].copy_from_slice(&colour);
                }
            }
        }
    }
    Image::new(
        Extent3d {
            width: W as u32,
            height: H as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}
