//! Read-only view of actual matter; no lifetime, random velocity or gravity here.
use super::*;
use std::collections::{BTreeMap, HashMap};
#[cfg(test)]
#[path = "fragments/lifecycle_tests.rs"]
mod lifecycle_tests;
#[path = "fragments/slots.rs"]
mod slots;
#[derive(Component)]
pub struct Piece(u64, u64, bool);
#[derive(Default)]
pub struct Cache {
    meshes: BTreeMap<render::model::FragmentMeshKey, Handle<Mesh>>,
    materials: BTreeMap<u16, Handle<StandardMaterial>>,
    chunks: BTreeMap<u64, Vec<Handle<Mesh>>>,
    slots: HashMap<u64, slots::Slot>,
    epoch: u64,
}
/// The cube colour of a range material, shared with falling bodies.
pub fn material_colour(material: u16) -> [f32; 4] {
    look::to_linear(match material {
        1 | 8 => [0.25, 0.20, 0.13],
        2 => [0.42, 0.40, 0.34],
        3 => [0.29, 0.32, 0.33],
        4 => [0.17, 0.22, 0.10],
        5 => [0.27, 0.20, 0.12],
        7 => [0.23, 0.25, 0.12],
        _ => [0.33, 0.33, 0.29],
    })
}
pub fn pos(v: [fx::Fx; 3]) -> Vec3 {
    Vec3::from(v.map(|a| a.to_f64_lossy() as f32))
}
#[allow(clippy::too_many_arguments)]
pub fn draw(
    mut commands: Commands,
    scene: Res<Scene>,
    visual: Res<moving::Visual>,
    art: Res<WitnessArt>,
    mut cache: Local<Cache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut old: Query<(Entity, &Piece, &mut Transform)>,
) {
    let fragments = &scene.sim.matter.debris.fragments;
    cache.epoch = cache.epoch.wrapping_add(1);
    let epoch = cache.epoch;
    let alpha = if scene.realtime && !scene.paused {
        (scene.acc * 30.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let mut chunk_builds = 0;
    for f in fragments {
        let piece_key = f.chunk.as_ref().map_or((0, false), |c| {
            (c.hash(), !scene.sim.units[c.template as usize].status.alive)
        });
        let make_transform = || {
            let current = pos([f.position.x, f.position.y, f.position.z]);
            let basis = f.basis.map(|v| pos([v.x, v.y, v.z]));
            let rotation =
                Quat::from_mat3(&Mat3::from_cols(basis[0], basis[1], basis[2])).normalize();
            let scale = if f.chunk.is_some() {
                1.0
            } else {
                (f.count as f32 / f.shape.count_ones() as f32).cbrt() * f.mm as f32
                    / voxel::unit::WORLD_MM as f32
            };
            Transform::from_translation(current)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(scale))
        };
        if let Some(slot) = cache.slots.get_mut(&f.id) {
            if slot.matches(piece_key, f) && old.contains(slot.entity) {
                slot.seen = epoch;
                if let Some(transform) = slot.sample(
                    f,
                    visual.fragments.get(&f.id).copied(),
                    alpha,
                    make_transform,
                ) {
                    if let Ok((_, p, mut tr)) = old.get_mut(slot.entity) {
                        debug_assert_eq!((p.0, p.1, p.2), (f.id, piece_key.0, piece_key.1));
                        if *tr != transform {
                            *tr = transform;
                        }
                    }
                }
                continue;
            }
            let entity = slot.entity;
            if old.contains(entity) {
                commands.entity(entity).despawn();
            }
            cache.slots.remove(&f.id);
            cache.chunks.remove(&f.id);
        }
        let current = make_transform();
        let transform = slots::interpolate(current, visual.fragments.get(&f.id).copied(), alpha);
        if let Some(chunk) = &f.chunk {
            if chunk_builds >= 2 {
                continue;
            }
            chunk_builds += 1;
            let unit = &scene.sim.units[chunk.template as usize];
            let dead = !unit.status.alive;
            let kind = unit.kind;
            let template = &art.pristine[kind.index()];
            let mut handles = Vec::new();
            let entity = commands
                .spawn((
                    Piece(f.id, chunk.hash(), dead),
                    transform,
                    Visibility::default(),
                ))
                .with_children(|parent| {
                    for (finish, mut buf) in render::model::section_to_mesh(template, chunk) {
                        if dead {
                            wreck::charred(&mut buf);
                        }
                        let handle = meshes.add(super::mesh(&buf));
                        handles.push(handle.clone());
                        parent.spawn((
                            Mesh3d(handle),
                            MeshMaterial3d(if dead {
                                art.burned_materials[finish].clone()
                            } else {
                                art.materials[finish].clone()
                            }),
                            Transform::default(),
                        ));
                    }
                })
                .id();
            cache.slots.insert(
                f.id,
                slots::Slot::new(entity, piece_key, f, current, transform, epoch),
            );
            cache.chunks.insert(f.id, handles);
            continue;
        }
        let colour = if let Some(p) = f.paint {
            let part = match p[2] {
                1 => voxel::Part::Core,
                2 => voxel::Part::Engine,
                3 => voxel::Part::Weapon,
                4 => voxel::Part::Sensor,
                _ => voxel::Part::Armour,
            };
            look::to_linear(look::finish_srgb(
                look::finish_of(p[1], part),
                [0.22, 0.32, 0.27],
            ))
        } else {
            material_colour(f.material)
        };
        let key = (f.material, f.shape, f.side, f.paint);
        let mesh = cache
            .meshes
            .entry(key)
            .or_insert_with(|| {
                let c = sim_core::burst::shape_centre(f.shape, f.side);
                let centre = pos([c.x, c.y, c.z]);
                let mut combined: Option<Mesh> = None;
                let side = u32::from(f.side.clamp(1, 4));
                for i in 0..side * side * side {
                    if f.shape & (1 << i) != 0 {
                        let offset = Vec3::new(
                            (i % side) as f32,
                            ((i / side) % side) as f32,
                            (i / (side * side)) as f32,
                        ) - centre;
                        let mut cube =
                            Mesh::from(Cuboid::new(0.98, 0.98, 0.98)).translated_by(offset);
                        cube.insert_attribute(
                            Mesh::ATTRIBUTE_COLOR,
                            vec![colour; cube.count_vertices()],
                        );
                        if let Some(m) = combined.as_mut() {
                            m.merge(&cube).expect("same fragment attributes");
                        } else {
                            combined = Some(cube);
                        }
                    }
                }
                meshes.add(combined.expect("nonempty physical shape"))
            })
            .clone();
        let material = cache
            .materials
            .entry(f.material)
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    metallic: if f.material == 3 { 0.65 } else { 0.0 },
                    perceptual_roughness: if f.material == 3 { 0.63 } else { 0.95 },
                    ..default()
                })
            })
            .clone();
        let entity = commands
            .spawn((
                Piece(f.id, 0, false),
                Mesh3d(mesh),
                MeshMaterial3d(material),
                transform,
            ))
            .id();
        cache.slots.insert(
            f.id,
            slots::Slot::new(entity, piece_key, f, current, transform, epoch),
        );
    }
    let Cache { slots, chunks, .. } = &mut *cache;
    slots.retain(|id, slot| {
        if slot.seen == epoch {
            true
        } else {
            if old.contains(slot.entity) {
                commands.entity(slot.entity).despawn();
            }
            chunks.remove(id);
            false
        }
    });
}
