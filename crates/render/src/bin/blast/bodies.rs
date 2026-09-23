//! Falling terrain of the range (ADR-300): a cohesive piece the collapse
//! layer tore loose — wall, rock, a trunk — falls whole and is drawn as
//! the cubes it is, until it lands and bakes back into the world.
use super::*;
use std::collections::{BTreeMap, HashSet};

type Face = ([i32; 3], [f32; 3], [[f32; 3]; 4]);

#[derive(Component)]
pub struct FallingBody(u64);

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
        let occupied: HashSet<_> = b.voxels.iter().map(|v| (v.x, v.y, v.z)).collect();
        let mut pos = Vec::<[f32; 3]>::new();
        let mut nrm = Vec::<[f32; 3]>::new();
        let mut col = Vec::<[f32; 4]>::new();
        let mut idx = Vec::<u32>::new();
        for v in &b.voxels {
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
            for (off, normal, corners) in faces {
                let nx = i32::from(v.x) + off[0];
                let ny = i32::from(v.y) + off[1];
                let nz = i32::from(v.z) + off[2];
                if nx >= 0
                    && ny >= 0
                    && nz >= 0
                    && occupied.contains(&(nx as u16, ny as u16, nz as u16))
                {
                    continue;
                }
                let start = pos.len() as u32;
                pos.extend(corners);
                nrm.extend([normal; 4]);
                col.extend([fragments::material_colour(v.mat); 4]);
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
