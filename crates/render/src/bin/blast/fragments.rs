//! Read-only view of actual matter; no lifetime, random velocity or gravity here.
use super::*;
use std::collections::BTreeMap;
#[derive(Component)]
pub struct Piece(u64, u64);
#[derive(Default)]
pub struct Cache {
    meshes: BTreeMap<render::model::FragmentMeshKey, Handle<Mesh>>,
    materials: BTreeMap<u16, Handle<StandardMaterial>>,
    chunks: BTreeMap<u64, Vec<Handle<Mesh>>>,
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
    let ids: BTreeMap<_, _> = fragments
        .iter()
        .map(|f| (f.id, f.chunk.as_ref().map_or(0, |c| c.hash())))
        .collect();
    let mut entities = BTreeMap::new();
    for (e, p, _) in &mut old {
        if ids.get(&p.0) != Some(&p.1) {
            commands.entity(e).despawn();
            cache.chunks.remove(&p.0);
        } else {
            entities.insert(p.0, e);
        }
    }
    let alpha = if scene.realtime && !scene.paused {
        (scene.acc * 30.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let mut chunk_builds = 0;
    for f in fragments {
        let current = pos([f.position.x, f.position.y, f.position.z]);
        let position = visual
            .fragments
            .get(&f.id)
            .copied()
            .unwrap_or(current)
            .lerp(current, alpha);
        let basis = f.basis.map(|v| pos([v.x, v.y, v.z]));
        let rotation = Quat::from_mat3(&Mat3::from_cols(basis[0], basis[1], basis[2])).normalize();
        let scale = if f.chunk.is_some() {
            1.0
        } else {
            (f.count as f32 / f.shape.count_ones() as f32).cbrt() * f.mm as f32
                / voxel::unit::WORLD_MM as f32
        };
        let transform = Transform::from_translation(position)
            .with_rotation(rotation)
            .with_scale(Vec3::splat(scale));
        if let Some(&entity) = entities.get(&f.id) {
            if let Ok((_, _, mut tr)) = old.get_mut(entity) {
                *tr = transform;
            }
            continue;
        }
        if let Some(chunk) = &f.chunk {
            if chunk_builds >= 2 {
                continue;
            }
            chunk_builds += 1;
            let kind = scene.sim.units[chunk.template as usize].kind;
            let template = &art.pristine[usize::from(kind == RangeUnitKind::Strider)];
            let mut handles = Vec::new();
            commands
                .spawn((Piece(f.id, chunk.hash()), transform, Visibility::default()))
                .with_children(|parent| {
                    for (finish, buf) in render::model::section_to_mesh(template, chunk) {
                        let handle = meshes.add(super::mesh(&buf));
                        handles.push(handle.clone());
                        parent.spawn((
                            Mesh3d(handle),
                            MeshMaterial3d(art.materials[finish].clone()),
                            Transform::default(),
                        ));
                    }
                });
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
            look::to_linear(match f.material {
                1 | 8 => [0.25, 0.20, 0.13],
                2 => [0.42, 0.40, 0.34],
                3 => [0.29, 0.32, 0.33],
                4 => [0.17, 0.22, 0.10],
                5 => [0.27, 0.20, 0.12],
                7 => [0.23, 0.25, 0.12],
                _ => [0.33, 0.33, 0.29],
            })
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
        commands.spawn((
            Piece(f.id, 0),
            Mesh3d(mesh),
            MeshMaterial3d(material),
            transform,
        ));
    }
}
