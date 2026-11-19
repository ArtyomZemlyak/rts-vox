//! Falling terrain of the range (ADR-300): a cohesive piece the collapse
//! layer tore loose — wall, rock, a trunk — falls whole and is drawn as
//! the cubes it is, until it lands and bakes back into the world.
use super::*;
use std::collections::{BTreeMap,BTreeSet};

#[path = "bodies/occupancy.rs"]
mod occupancy;

type Face = ([i32; 3], [f32; 3], [[f32; 3]; 4]);

#[derive(Component)]
pub struct FallingBody(u64);

// Evaluate once while building the exposed faces, in the source's coordinates.
// Grain and panel seams then travel with the slab instead of swimming across it
// as it tilts. This adds no meshes, textures or per-frame colour work.
fn surface_colour(mat: u16, source: [i64; 3], normal: [f32; 3]) -> [f32; 4] {
    let noise = |p: [i64; 3]| {
        let mut h = (p[0] as u32).wrapping_mul(0x8da6_b343)
            ^ (p[1] as u32).wrapping_mul(0xd816_3841)
            ^ (p[2] as u32).wrapping_mul(0xcb1a_b31f);
        h ^= h >> 16;
        h = h.wrapping_mul(0x85eb_ca6b);
        h ^= h >> 13;
        (h & 1023) as f32 / 1023.0
    };
    let grain = noise(source);
    let stain = noise(source.map(|v| v.div_euclid(18)));
    let mut c = fragments::material_colour(mat);
    let mut shade = 0.82 + grain * 0.18 + stain * 0.12;
    if mat == 3 {
        let y = source[1] + i64::from(normal[1] > 0.5);
        let z = source[2] + i64::from(normal[2] > 0.5);
        if z.rem_euclid(12) == 0 || y.rem_euclid(10) == 0 {
            shade *= 0.48;
        }
        let rust = ((stain - 0.66) / 0.34).clamp(0.0, 1.0) * 0.6;
        let oxide = look::to_linear([0.39, 0.25, 0.14]);
        for k in 0..3 {
            c[k] += (oxide[k] - c[k]) * rust;
        }
    } else if mat == 2 {
        // A detached slab is one concrete surface. Per-voxel grain made
        // its continuous face read as thousands of loose particles.
        shade = 0.94 + stain * 0.08 + grain * 0.015;
    } else if mat == 5 {
        // Long grain along the trunk; no regular checkerboard on end faces.
        shade *= 0.80 + noise([source[0], source[1].div_euclid(24), source[2]]) * 0.28;
    }
    for channel in &mut c[..3] {
        *channel *= shade;
    }
    c
}

pub(super) fn pose(b: &voxel::bodies::Body) -> Transform {
    let fy = b.fy.to_f64_lossy() as f32;
    let angle = b.tilt_angle as f32 * std::f32::consts::TAU / 65536.0;
    let rotation = match b.tilt_axis {
        1 | 3 => Quat::from_rotation_x(angle),
        2 | 4 => Quat::from_rotation_z(angle),
        _ => Quat::IDENTITY,
    };
    let mut translation = Vec3::new(
        b.ox as f32 + f32::from(b.pivot_x),
        b.oy as f32 - fy,
        b.oz as f32 + f32::from(b.pivot_z),
    );
    if b.tilt_axis >= 3 {
        let centre = Vec3::new(
            (b.sx / 2) as f32,
            (b.height() / 2) as f32,
            (b.sz / 2) as f32,
        );
        translation += centre - rotation * centre;
    }
    Transform::from_translation(translation).with_rotation(rotation)
}

pub fn draw(
    mut commands: Commands,
    scene: Res<Scene>,
    lab: Option<Res<laboratory::Laboratory>>,
    mut stamp: Local<BTreeMap<u64, (u8, i64)>>,
    mut diagnostic_material: Local<Option<Handle<StandardMaterial>>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material: Local<Option<Handle<StandardMaterial>>>,
    mut old: Query<(Entity, &FallingBody, &Mesh3d, &mut Transform)>,
) {
    let mode = lab.as_ref().map_or(0, |l| l.metric_mode());
    let bodies = &scene.sim.bodies.list;
    let live:BTreeSet<_>=bodies.iter().map(|b|b.id).collect();
    stamp.retain(|id, _| live.contains(id));
    let mut entities = BTreeMap::new();
    for (e, b, mesh, _) in &mut old {
        if live.contains(&b.0) {
            entities.insert(b.0, e);
        } else {
            commands.entity(e).despawn();
            meshes.remove(mesh.0.id());
        }
    }
    for b in bodies {
        // The body's origin, the fraction of the next voxel it has fallen
        // taken off its height, so the fall reads smoothly.
        let transform = pose(b);
        let key = (mode, if mode == 6 { b.vy.raw() } else { 0 });
        if let Some(&entity) = entities.get(&b.id) {
            if stamp.get(&b.id) == Some(&key) {
                if let Ok((_, _, _, mut tr)) = old.get_mut(entity) { *tr = transform; }
                continue;
            }
            if let Ok((_, _, mesh, _)) = old.get_mut(entity) { meshes.remove(mesh.0.id()); }
            commands.entity(entity).despawn();
        }
        stamp.insert(b.id, key);
        // One quad per exposed face. Merging an entire cube for every cell
        // gave a large fractured slab millions of hidden triangles and an
        // O(n²) mesh-build path on the frame where it detached.
        // Compute exposure once with bounded dense occupancy for compact
        // bodies. Exact capacities avoid growing four large vertex buffers
        // during the same frame that the simulation fractures the slab.
        let exposed = occupancy::visible_faces(&b.voxels);
        let face_count = exposed
            .iter()
            .map(|v| v.count_ones() as usize)
            .sum::<usize>();
        let mut pos = Vec::<[f32; 3]>::with_capacity(face_count * 4);
        let mut nrm = Vec::<[f32; 3]>::with_capacity(face_count * 4);
        let mut col = Vec::<[f32; 4]>::with_capacity(face_count * 4);
        let mut idx = Vec::<u32>::with_capacity(face_count * 6);
        for (v, visible) in b.voxels.iter().zip(exposed) {
            if visible == 0 {
                continue;
            }
            let x = f32::from(v.x) - f32::from(b.pivot_x);
            let y = f32::from(v.y);
            let z = f32::from(v.z) - f32::from(b.pivot_z);
            let faces: [Face; 6] = [
                (
                    [1, 0, 0],
                    [1.0, 0.0, 0.0],
                    [
                        [x + 1.0, y, z],
                        [x + 1.0, y + 1.0, z],
                        [x + 1.0, y + 1.0, z + 1.0],
                        [x + 1.0, y, z + 1.0],
                    ],
                ),
                (
                    [-1, 0, 0],
                    [-1.0, 0.0, 0.0],
                    [
                        [x, y, z + 1.0],
                        [x, y + 1.0, z + 1.0],
                        [x, y + 1.0, z],
                        [x, y, z],
                    ],
                ),
                (
                    [0, 1, 0],
                    [0.0, 1.0, 0.0],
                    [
                        [x, y + 1.0, z + 1.0],
                        [x + 1.0, y + 1.0, z + 1.0],
                        [x + 1.0, y + 1.0, z],
                        [x, y + 1.0, z],
                    ],
                ),
                (
                    [0, -1, 0],
                    [0.0, -1.0, 0.0],
                    [
                        [x, y, z],
                        [x + 1.0, y, z],
                        [x + 1.0, y, z + 1.0],
                        [x, y, z + 1.0],
                    ],
                ),
                (
                    [0, 0, 1],
                    [0.0, 0.0, 1.0],
                    [
                        [x, y, z + 1.0],
                        [x + 1.0, y, z + 1.0],
                        [x + 1.0, y + 1.0, z + 1.0],
                        [x, y + 1.0, z + 1.0],
                    ],
                ),
                (
                    [0, 0, -1],
                    [0.0, 0.0, -1.0],
                    [
                        [x + 1.0, y, z],
                        [x, y, z],
                        [x, y + 1.0, z],
                        [x + 1.0, y + 1.0, z],
                    ],
                ),
            ];
            for (face, (_, normal, corners)) in faces.into_iter().enumerate() {
                if visible & (1 << face) == 0 {
                    continue;
                }
                let start = pos.len() as u32;
                pos.extend(corners);
                nrm.extend([normal; 4]);
                let source = [
                    b.ox + i64::from(v.x),
                    b.oy + i64::from(b.fallen) + i64::from(v.y),
                    b.oz + i64::from(v.z),
                ];
                let diagnostic = match mode {
                    3 => if matches!(v.mat, 11 | 13 | 14 | 16) { [0.1, 0.45, 0.95, 1.0] }
                        else if scene.sim.matter.economy.mats.get(v.mat).is_some_and(|m| m.loose) { [0.95, 0.72, 0.1, 1.0] }
                        else { [0.58, 0.58, 0.62, 1.0] },
                    4 => scene.sim.matter.economy.mats.get(v.mat).map_or(laboratory::METRIC_UNKNOWN, |m| laboratory::metric_colour(m.strength, 500)),
                    5 => scene.sim.matter.economy.mats.get(v.mat).map_or(laboratory::METRIC_UNKNOWN, |m| laboratory::metric_colour(m.density, 1000)),
                    6 => laboratory::metric_colour((b.vy.raw().unsigned_abs().saturating_mul(200) / fx::ONE_RAW as u64).min(u64::from(u32::MAX)) as u32, 500),
                    10 => { let id=u32::from(v.mat); [0.18+(id*43%70) as f32/100.0,0.18+(id*67%70) as f32/100.0,0.18+(id*29%70) as f32/100.0,1.0] },
                    _ => laboratory::METRIC_UNKNOWN,
                };
                col.extend([if mode > 0 { diagnostic } else { surface_colour(v.mat, source, normal) }; 4]);
                idx.extend([start, start + 1, start + 2, start, start + 2, start + 3]);
            }
        }
        if pos.is_empty() {
            continue;
        }
        let mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nrm)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col)
        .with_inserted_indices(Indices::U32(idx));
        let handle = if mode > 0 { diagnostic_material.get_or_insert_with(|| materials.add(StandardMaterial { base_color: Color::WHITE, unlit: true, ..default() })).clone() } else { material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    perceptual_roughness: 0.95,
                    ..default()
                })
            })
            .clone() };
        commands.spawn((
            FallingBody(b.id),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(handle),
            transform,
        ));
    }
}

#[cfg(test)]
mod rolling_tests {
    use super::*;
    struct Rock;
    impl voxel::Generator for Rock {
        fn material(&self, _: u32, _: u32, _: u32) -> u16 {
            6
        }
    }
    #[test]
    fn render_roll_matches_simulated_voxel_positions() {
        let w = voxel::World::new(32, 32, 32, 100, Box::new(Rock));
        let cells: Vec<_> = (10..13)
            .flat_map(|z| (10..14).flat_map(move |y| (10..15).map(move |x| (x, y, z))))
            .collect();
        let mut body = voxel::bodies::Body::from_world_cells(&w, &cells).unwrap();
        for axis in [3, 4] {
            for angle in [0, 3000, 12000, 35000, 62000] {
                body.tilt_axis = axis;
                body.tilt_angle = angle;
                let transform = pose(&body);
                for &v in &body.voxels {
                    let p =
                        transform.transform_point(Vec3::new(v.x as f32, v.y as f32, v.z as f32));
                    let (x, y, z) = body.world_of(v);
                    let q = Vec3::new(x as f32, y as f32, z as f32);
                    assert!(
                        (p - q).abs().max_element() <= 0.51,
                        "axis={axis} angle={angle} p={p:?} q={q:?}"
                    );
                }
            }
        }
    }
}
