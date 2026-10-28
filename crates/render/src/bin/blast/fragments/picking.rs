//! Click-only inspection of the same geometry used by individual and batched draws.
use super::*;
use sim_core::burst::Fragment;
#[derive(Clone, Copy, PartialEq)]
pub struct Hit {
    pub id: u64,
    pub generation: u64,
    local: Vec3,
    geometry: (u64, u8, u16, u64, u8),
}
fn geometry(f: &Fragment) -> (u64, u8, u16, u64, u8) {
    (
        f.shape,
        f.side,
        f.mm,
        f.chunk.as_ref().map_or(0, |c| c.hash()),
        rubble_variant(f),
    )
}
impl Hit {
    /// A minimum 100 mm cell in the piece's own grid, following its turn.
    /// A world-aligned box would cut a single settled voxel into blue/gold halves.
    pub fn cell_bounds(&self, f: &Fragment) -> Option<(Vec3, Vec3)> {
        if f.chunk.is_some() || self.id != f.id || self.geometry != geometry(f) {
            return None;
        }
        let c = sim_core::burst::shape_centre(f.shape, f.side);
        let centre = Vec3::from([c.x, c.y, c.z].map(|v| v.raw() as f32 / fx::ONE_RAW as f32))
            + Vec3::splat(0.5);
        let step = voxel::unit::WORLD_MM as f32 / f.mm.max(1) as f32;
        let lo = ((self.local + centre) / step).floor() * step - centre;
        Some((lo, lo + Vec3::splat(step)))
    }
    pub fn point(&self, f: &Fragment) -> Option<Vec3> {
        (self.id == f.id && self.geometry == geometry(f))
            .then(|| dense::transform(f).transform_point(self.local))
    }
}
fn triangle(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (ab, ac) = (b - a, c - a);
    let p = direction.cross(ac);
    let det = ab.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let offset = origin - a;
    let u = offset.dot(p) / det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = offset.cross(ab);
    let v = direction.dot(q) / det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = ac.dot(q) / det;
    (t >= 0.0).then_some(t)
}
pub fn pick(scene: &Scene, ray: Ray3d, limit: f32) -> Option<Hit> {
    let mut end = limit;
    let mut best = None;
    let mut templates = BTreeMap::new();
    // No per-frame scan or index maintenance: this runs only on a pipette click.
    for f in &scene.sim.matter.debris.fragments {
        let centre = pos([f.position.x, f.position.y, f.position.z]);
        let (lo, hi) = f.bounds();
        if super::super::laboratory::tools::entry(
            ray,
            centre + pos(lo) - Vec3::splat(0.001),
            centre + pos(hi) + Vec3::splat(0.001),
        )
        .is_none_or(|(t, _)| t > end)
        {
            continue;
        }
        let inverse = dense::transform(f).compute_affine().inverse();
        let origin = inverse.transform_point3(ray.origin);
        // Keep this unnormalised: triangle t then remains world-ray distance.
        let direction = inverse.transform_vector3(*ray.direction);
        let buffers = if let Some(c) = &f.chunk {
            let Some(unit) = scene.sim.units.get(c.template as usize) else {
                continue;
            };
            let template = templates
                .entry(unit.kind.index())
                .or_insert_with(|| sim_core::blast::RangeUnit::of_kind(unit.kind, [0, 0, 0]).body);
            render::model::section_to_mesh(template, c)
                .into_iter()
                .map(|(_, b)| b)
                .collect::<Vec<_>>()
        } else {
            let variant = rubble_variant(f);
            vec![if variant == 0 {
                render::model::fragment_to_mesh(f.shape, f.side, [1.0; 4])
            } else {
                render::model::rubble_pile_to_mesh(variant - 1, [1.0; 4])
            }]
        };
        for buf in buffers {
            for indices in buf.idx.chunks_exact(3) {
                let [a, b, c] = std::array::from_fn(|i| Vec3::from(buf.pos[indices[i] as usize]));
                if let Some(t) = triangle(origin, direction, a, b, c).filter(|t| *t < end) {
                    end = t;
                    best = Some(Hit {
                        id: f.id,
                        generation: scene.generation,
                        local: origin + direction * (t + 0.0001),
                        geometry: geometry(f),
                    });
                }
            }
        }
    }
    best
}

/// Geometry is identical for individual draws and presentation batches.
pub fn surface(scene: &Scene, f: &Fragment) -> Vec<render::geom::MeshBuf> {
    if let Some(c) = &f.chunk {
        let Some(unit) = scene.sim.units.get(c.template as usize) else {
            return Vec::new();
        };
        let template = sim_core::blast::RangeUnit::of_kind(unit.kind, [0, 0, 0]).body;
        render::model::section_to_mesh(&template, c)
            .into_iter()
            .map(|(_, b)| b)
            .collect()
    } else {
        let variant = rubble_variant(f);
        vec![if variant == 0 {
            render::model::fragment_to_mesh(f.shape, f.side, [1.0; 4])
        } else {
            render::model::rubble_pile_to_mesh(variant - 1, [1.0; 4])
        }]
    }
}
pub fn pose(f: &Fragment) -> Transform {
    dense::transform(f)
}
