//! Falling terrain of the range (ADR-300): a cohesive piece the collapse
//! layer tore loose — wall, rock, a trunk — falls whole and is drawn as
//! the cubes it is, until it lands and bakes back into the world.
use super::*;
use std::collections::BTreeMap;

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
    } else if mat == 2 && grain > 0.94 {
        shade *= 0.68;
    } else if mat == 5 {
        // Long grain along the trunk; no regular checkerboard on end faces.
        shade *= 0.80 + noise([source[0], source[1].div_euclid(24), source[2]]) * 0.28;
    }
    for channel in &mut c[..3] {
        *channel *= shade;
    }
    c
}

pub fn draw(
    mut commands: Commands,
    scene: Res<Scene>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material: Local<Option<Handle<StandardMaterial>>>,
    mut old: Query<(Entity, &FallingBody, &Mesh3d, &mut Transform)>,
) {
    let bodies = &scene.sim.bodies.list;
    let mut entities = BTreeMap::new();
    for (e, b, mesh, _) in &mut old {
        if bodies.iter().any(|x| x.id == b.0) {
            entities.insert(b.0, e);
        } else {
            commands.entity(e).despawn();
            meshes.remove(mesh.0.id());
        }
    }
    for b in bodies {
        // The body's origin, the fraction of the next voxel it has fallen
        // taken off its height, so the fall reads smoothly.
        let fy = b.fy.to_f64_lossy() as f32;
        let angle = b.tilt_angle as f32 * std::f32::consts::TAU / 65536.0;
        let rotation = match b.tilt_axis {
            1 => Quat::from_rotation_x(angle),
            2 => Quat::from_rotation_z(angle),
            _ => Quat::IDENTITY,
        };
        let transform = Transform::from_translation(Vec3::new(
            b.ox as f32 + f32::from(b.pivot_x),
            b.oy as f32 - fy,
            b.oz as f32 + f32::from(b.pivot_z),
        ))
        .with_rotation(rotation);
        if let Some(&entity) = entities.get(&b.id) {
            if let Ok((_, _, _, mut tr)) = old.get_mut(entity) {
                *tr = transform;
            }
            continue;
        }
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
                col.extend([surface_colour(v.mat, source, normal); 4]);
                idx.extend([start, start + 1, start + 2, start, start + 2, start + 3]);
            }
        }
        if pos.is_empty() {
            continue;
        }
        let mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nrm)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col)
        .with_inserted_indices(Indices::U32(idx));
        let handle = material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    perceptual_roughness: 0.95,
                    ..default()
                })
            })
            .clone();
        commands.spawn((
            FallingBody(b.id),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(handle),
            transform,
        ));
    }
}
