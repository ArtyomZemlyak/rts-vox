//! One cached, depth-tested emissive surface patch. No lights or SIM writes.
use super::*;
use render::geom::MeshBuf;
#[derive(Component)]
pub struct Highlight;
#[derive(Clone, PartialEq)]
enum Key {
    Fragment(fragments::picking::Hit),
    Falling(u64, u64, [u16; 3]),
    World(u64, u32, [i32; 3]),
}
#[derive(Default)]
pub struct Cache {
    key: Option<Key>,
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    previous: Option<MeshBuf>,
    fragment_slot: usize,
}
const LIFT: f32 = 0.001;

// Sutherland-Hodgman clipping preserves only existing surface triangles.
// Never add a closing face across empty space or an exposed clipping plane.
fn clip_polygon(input: &[Vec3], axis: usize, limit: f32, sign: f32) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(input.len() + 1);
    if input.is_empty() {
        return out;
    }
    let mut a = *input.last().unwrap();
    let mut da = (a[axis] - limit) * sign;
    for &b in input {
        let db = (b[axis] - limit) * sign;
        if (da >= 0.0) != (db >= 0.0) {
            out.push(a.lerp(b, da / (da - db)));
        }
        if db >= 0.0 {
            out.push(b);
        }
        a = b;
        da = db;
    }
    out
}
fn clipped(source: &[MeshBuf], pose: Transform, lo: Vec3, hi: Vec3, local: bool) -> MeshBuf {
    let mut out = MeshBuf::default();
    let inverse = pose.compute_affine().inverse();
    for buf in source {
        for t in buf.idx.chunks_exact(3) {
            let mut polygon: Vec<_> = t
                .iter()
                .map(|i| pose.transform_point(Vec3::from(buf.pos[*i as usize])))
                .collect();
            for axis in 0..3 {
                polygon = clip_polygon(&polygon, axis, lo[axis], 1.0);
                polygon = clip_polygon(&polygon, axis, hi[axis], -1.0);
            }
            if polygon.len() < 3 {
                continue;
            }
            let normal = (pose.rotation * Vec3::from(buf.nrm[t[0] as usize])).normalize_or_zero();
            for i in 1..polygon.len() - 1 {
                let points = [polygon[0], polygon[i], polygon[i + 1]];
                if (points[1] - points[0])
                    .cross(points[2] - points[0])
                    .length_squared()
                    < 1e-14
                {
                    continue;
                }
                let start = out.pos.len() as u32;
                for p in points {
                    let p = p + normal * LIFT;
                    out.pos.push(
                        if local {
                            inverse.transform_point3(p)
                        } else {
                            p
                        }
                        .to_array(),
                    );
                    out.nrm.push(
                        if local {
                            pose.rotation.inverse() * normal
                        } else {
                            normal
                        }
                        .to_array(),
                    );
                    out.col.push([1.0; 4]);
                }
                out.idx.extend([start, start + 1, start + 2]);
            }
        }
    }
    out
}
// Clip in the owner's cell lattice, then move the patch with its actual mesh.
fn attached_cell(source: &[MeshBuf], pose: Transform, lo: Vec3, hi: Vec3, local: bool) -> MeshBuf {
    let mut out = clipped(source, Transform::IDENTITY, lo, hi, false);
    for (p, n) in out.pos.iter_mut().zip(&mut out.nrm) {
        let normal = Vec3::from(*n);
        let point = Vec3::from(*p) + normal * (LIFT / pose.scale.x - LIFT);
        *p = if local {
            point
        } else {
            pose.transform_point(point)
        }
        .to_array();
        if !local {
            *n = (pose.rotation * normal).to_array();
        }
    }
    out
}
fn cube(lo: Vec3) -> MeshBuf {
    let mut out = MeshBuf::default();
    for axis in 0..3 {
        for side in [0.0, 1.0] {
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            let mut normal = Vec3::ZERO;
            normal[axis] = side * 2.0 - 1.0;
            let points: Vec<_> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                .into_iter()
                .map(|(x, y)| {
                    let mut p = lo;
                    p[axis] += side;
                    p[u] += x;
                    p[v] += y;
                    p
                })
                .collect();
            let start = out.pos.len() as u32;
            out.pos.extend(points.iter().map(|p| p.to_array()));
            out.nrm.extend([normal.to_array(); 4]);
            out.col.extend([[1.0; 4]; 4]);
            if (points[1] - points[0])
                .cross(points[2] - points[0])
                .dot(normal)
                > 0.0
            {
                out.idx
                    .extend([start, start + 1, start + 2, start, start + 2, start + 3]);
            } else {
                out.idx
                    .extend([start, start + 2, start + 1, start, start + 3, start + 2]);
            }
        }
    }
    out
}
fn scalar(v: fx::Fx) -> f32 {
    v.raw() as f32 / fx::ONE_RAW as f32
}
fn vector(v: sim_core::damage::V3) -> Vec3 {
    Vec3::new(scalar(v.x), scalar(v.y), scalar(v.z))
}
fn frame_pose(f: &sim_core::damage::Frame) -> Transform {
    let rotation =
        Quat::from_mat3(&Mat3::from_cols(vector(f.x), vector(f.y), vector(f.z))).normalize();
    let scale = 1.0 / scalar(f.scale);
    let centre = Vec3::from(f.centre.map(scalar));
    Transform::from_translation(vector(f.base) - rotation * (centre * scale))
        .with_rotation(rotation)
        .with_scale(Vec3::splat(scale))
}
fn world_surface(scene: &Scene, at: [i32; 3]) -> MeshBuf {
    let lo = Vec3::from(at.map(|v| v as f32));
    let hi = lo + Vec3::ONE;
    for u in scene
        .sim
        .units
        .iter()
        .chain((!scene.sim.point_gun()).then_some(&scene.sim.gun))
    {
        if let Some((part, cell)) = u.hit(at) {
            let (body, frame) = if part == 0 {
                (&u.body, u.root_frame(cell[1]))
            } else {
                (&u.body.segments[part - 1].body, u.seg_frames[part - 1])
            };
            let source = render::model::inspection_cell_mesh(body, cell);
            let pose = frame_pose(&frame);
            let point = pose
                .compute_affine()
                .inverse()
                .transform_point3(lo + Vec3::splat(0.5));
            let cell = Vec3::from(cell.map(|v| v as f32));
            let point = point.clamp(cell, cell + Vec3::splat(0.99999));
            let step = voxel::unit::WORLD_MM as f32 / body.mm.max(1) as f32;
            let low = (point / step).floor() * step;
            return attached_cell(&source, pose, low, low + Vec3::splat(step), false);
        }
    }
    if at.iter().any(|v| *v < 0) {
        return MeshBuf::default();
    }
    if sim_core::blast::material(&scene.sim.world, at) == 0
        && scene.sim.water.amount(at.map(|v| v as u32)) == 0
    {
        return MeshBuf::default();
    }
    clipped(&[cube(lo)], Transform::IDENTITY, lo, hi, false)
}
fn same_mesh(a: &MeshBuf, b: &MeshBuf) -> bool {
    a.pos == b.pos && a.nrm == b.nrm && a.idx == b.idx
}
fn hide(cache: &Cache, query: &mut Query<(&mut Transform, &mut Visibility), With<Highlight>>) {
    if let Some(e) = cache.entity {
        if let Ok((_, mut v)) = query.get_mut(e) {
            v.set_if_neq(Visibility::Hidden);
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    lab: Res<Laboratory>,
    scene: Res<Scene>,
    visual: Res<moving::Visual>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: Local<Cache>,
    mut query: Query<(&mut Transform, &mut Visibility), With<Highlight>>,
) {
    let Some(at) = lab.tools.selected.filter(|_| {
        lab.active() && (lab.ui.tab == 14 || lab.constructor.mode == constructor::Mode::Inspect)
    }) else {
        hide(&cache, &mut query);
        return;
    };
    let (key, pose) = if let Some(hit) = lab.tools.fragment {
        if hit.generation != scene.generation {
            hide(&cache, &mut query);
            return;
        }
        let pieces = &scene.sim.matter.debris.fragments;
        let index = if pieces
            .get(cache.fragment_slot)
            .is_some_and(|f| f.id == hit.id)
        {
            Some(cache.fragment_slot)
        } else {
            pieces.iter().position(|f| f.id == hit.id)
        };
        let Some(index) = index else {
            hide(&cache, &mut query);
            return;
        };
        cache.fragment_slot = index;
        let f = &pieces[index];
        if hit.point(f).is_none() {
            hide(&cache, &mut query);
            return;
        }
        let mut pose = fragments::picking::pose(f);
        if scene.realtime && !scene.paused {
            let alpha = (scene.acc * sim_core::TICKS_PER_SECOND as f32).clamp(0.0, 1.0);
            pose.translation = visual
                .fragments
                .get(&f.id)
                .copied()
                .unwrap_or(pose.translation)
                .lerp(pose.translation, alpha);
        }
        (Key::Fragment(hit), pose)
    } else if let Some((id, local)) = lab.tools.falling {
        let Some(b) = scene.sim.bodies.list.iter().find(|b| b.id == id) else {
            hide(&cache, &mut query);
            return;
        };
        if !b.voxels.iter().any(|v| [v.x, v.y, v.z] == local) {
            hide(&cache, &mut query);
            return;
        }
        (Key::Falling(scene.generation, id, local), bodies::pose(b))
    } else {
        (
            Key::World(scene.generation, scene.sim.tick, at),
            Transform::IDENTITY,
        )
    };
    if cache.key.as_ref() != Some(&key) || cache.mesh.is_none() {
        let buf = match key {
            Key::Fragment(hit) => {
                let f = &scene.sim.matter.debris.fragments[cache.fragment_slot];
                let source = fragments::picking::surface(&scene, f);
                let pose = fragments::picking::pose(f);
                if let Some((lo, hi)) = hit.cell_bounds(f) {
                    attached_cell(&source, pose, lo, hi, true)
                } else {
                    let lo = Vec3::from(at.map(|v| v as f32));
                    clipped(&source, pose, lo, lo + Vec3::ONE, true)
                }
            }
            Key::Falling(_, id, local) => {
                let b = scene.sim.bodies.list.iter().find(|b| b.id == id).unwrap();
                let lo = Vec3::new(
                    local[0] as f32 - b.pivot_x as f32,
                    local[1] as f32,
                    local[2] as f32 - b.pivot_z as f32,
                );
                clipped(&[cube(lo)], Transform::IDENTITY, lo, lo + Vec3::ONE, true)
            }
            Key::World(_, _, at) => world_surface(&scene, at),
        };
        if !cache
            .previous
            .as_ref()
            .is_some_and(|old| same_mesh(old, &buf))
        {
            let next = meshes.add(super::super::mesh(&buf));
            if let Some(old) = cache.mesh.replace(next.clone()) {
                meshes.remove(old.id());
            }
            if let Some(e) = cache.entity {
                if query.contains(e) {
                    commands.entity(e).insert(Mesh3d(next));
                } else {
                    cache.entity = None;
                }
            }
            cache.previous = Some(buf);
        }
        cache.key = Some(key);
    }
    if cache.previous.as_ref().is_none_or(|b| b.is_empty()) {
        hide(&cache, &mut query);
        return;
    }
    if let Some(e) = cache.entity {
        if let Ok((mut tr, mut v)) = query.get_mut(e) {
            tr.set_if_neq(pose);
            v.set_if_neq(Visibility::Visible);
            return;
        }
    }
    let material = cache
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgba(1.0, 0.65, 0.08, 0.62),
                emissive: LinearRgba::new(3.2, 1.8, 0.08, 1.0),
                emissive_exposure_weight: 0.0,
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                ..default()
            })
        })
        .clone();
    let mesh = cache.mesh.as_ref().unwrap().clone();
    cache.entity = Some(
        commands
            .spawn((
                Highlight,
                Mesh3d(mesh),
                MeshMaterial3d(material),
                pose,
                Visibility::Visible,
                bevy::light::NotShadowCaster,
            ))
            .id(),
    );
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
