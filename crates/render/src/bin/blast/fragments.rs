//! Read-only view of actual matter; no lifetime, random velocity or gravity here.
use super::*;
use bevy::platform::collections::HashMap;
use std::collections::BTreeMap;
#[path = "fragments/dense.rs"]
mod dense;
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
    slots: dense::Slots,
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
fn piece_mesh(shape: u64, side: u8, colour: [f32; 4]) -> Mesh {
    let c = sim_core::burst::shape_centre(shape, side);
    let centre = pos([c.x, c.y, c.z]);
    let mut combined: Option<Mesh> = None;
    let side = u32::from(side.clamp(1, 4));
    for i in 0..side * side * side {
        if shape & (1 << i) != 0 {
            let offset = Vec3::new(
                (i % side) as f32,
                ((i / side) % side) as f32,
                (i / (side * side)) as f32,
            ) - centre;
            let mut cube = Mesh::from(Cuboid::new(0.98, 0.98, 0.98)).translated_by(offset);
            cube.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![colour; cube.count_vertices()]);
            if let Some(m) = combined.as_mut() {
                m.merge(&cube).expect("same fragment attributes");
            } else {
                combined = Some(cube);
            }
        }
    }
    combined.expect("nonempty physical shape")
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
    let alpha = if scene.realtime && !scene.paused {
        (scene.acc * 30.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let mut chunk_builds = 0;
    cache.slots.align(fragments);
    {
        let Cache { slots, chunks, .. } = &mut *cache;
        for slot in slots.retired.drain(..) {
            if old.contains(slot.entity) {
                commands.entity(slot.entity).despawn();
            }
            chunks.remove(&slot.id);
        }
    }
    cache.slots.prepare(
        fragments,
        &scene.sim.units,
        &visual.fragments,
        alpha,
        |entity| old.as_readonly().contains(entity),
        bevy::tasks::ComputeTaskPool::try_get().map(|pool| &**pool),
    );
    // Workers touch only their render slots and read-only physics/queries.
    // Asset and entity changes keep the original physical fragment order.
    let Cache {
        slots,
        chunks,
        meshes: cached_meshes,
        materials: cached_materials,
        ..
    } = &mut *cache;
    for (entry, f) in slots.entries.iter_mut().zip(fragments) {
        let (piece_key, current) = match entry.prepared {
            dense::Prepared::Existing(next) => {
                let slot = entry.slot.as_ref().unwrap();
                if let Some(transform) = next {
                    if let Ok((_, p, mut tr)) = old.get_mut(slot.entity) {
                        debug_assert_eq!((p.0, p.1, p.2), (f.id, slot.key.0, slot.key.1));
                        tr.set_if_neq(transform);
                    }
                }
                continue;
            }
            dense::Prepared::Replace(key, current) => (key, current),
        };
        if let Some(slot) = entry.slot.take() {
            let entity = slot.entity;
            if old.contains(entity) {
                commands.entity(entity).despawn();
            }
            chunks.remove(&f.id);
        }
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
            entry.slot = Some(slots::Slot::new(entity, piece_key, f, current, transform));
            chunks.insert(f.id, handles);
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
        let mesh = cached_meshes
            .entry(key)
            .or_insert_with(|| meshes.add(piece_mesh(f.shape, f.side, colour)))
            .clone();
        let material = cached_materials
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
        entry.slot = Some(slots::Slot::new(entity, piece_key, f, current, transform));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saturated_pile_mesh_matches_physical_bounds() {
        for (shape, side, count) in [(1, 2, 1), (u64::MAX, 4, 64), (u64::MAX, 4, 4096)] {
            for mm in [100, 200] {
                let f = sim_core::burst::Fragment {
                    id: 1,
                    position: phys_v3_zero(),
                    velocity: phys_v3_zero(),
                    material: 3,
                    paint: None,
                    mm,
                    basis: [phys_v3(1, 0, 0), phys_v3(0, 1, 0), phys_v3(0, 0, 1)],
                    mass_g: 1,
                    count,
                    shape,
                    side,
                    chunk: None,
                    held: 0,
                    resting: true,
                };
                let mesh = piece_mesh(shape, side, [1.0; 4]);
                let points = mesh
                    .attribute(Mesh::ATTRIBUTE_POSITION)
                    .unwrap()
                    .as_float3()
                    .unwrap();
                let mut lo = Vec3::splat(f32::INFINITY);
                let mut hi = Vec3::splat(f32::NEG_INFINITY);
                for p in points {
                    let p = Vec3::from(*p) * render::model::fragment_scale(&f);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
                let (low, high) = f.bounds();
                let v = |p: [fx::Fx; 3]| Vec3::from(p.map(|v| v.to_f64_lossy() as f32));
                assert!((lo - v(low)).abs().max_element() < 0.0001, "count={count}");
                assert!((hi - v(high)).abs().max_element() < 0.0001, "count={count}");
            }
        }
    }
    fn phys_v3(x: i32, y: i32, z: i32) -> sim_core::damage::V3 {
        sim_core::damage::V3::new(
            fx::Fx::from_int(x),
            fx::Fx::from_int(y),
            fx::Fx::from_int(z),
        )
    }
    fn phys_v3_zero() -> sim_core::damage::V3 {
        phys_v3(0, 0, 0)
    }
}
