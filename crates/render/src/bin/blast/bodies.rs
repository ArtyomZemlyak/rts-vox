//! Falling terrain of the range (ADR-300): a cohesive piece the collapse
//! layer tore loose — wall, rock, a trunk — falls whole and is drawn as
//! the cubes it is, until it lands and bakes back into the world.
use super::*;
use std::collections::BTreeMap;

#[derive(Component)]
pub struct FallingBody(u64);

pub fn draw(
    mut commands: Commands,
    scene: Res<Scene>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material: Local<Option<Handle<StandardMaterial>>>,
    mut old: Query<(Entity, &FallingBody, &mut Transform)>,
) {
    let bodies = &scene.sim.bodies.list;
    let mut entities = BTreeMap::new();
    for (e, b, _) in &mut old {
        if bodies.iter().any(|x| x.id == b.0) {
            entities.insert(b.0, e);
        } else {
            commands.entity(e).despawn();
        }
    }
    for b in bodies {
        // The body's origin, the fraction of the next voxel it has fallen
        // taken off its height, so the fall reads smoothly.
        let fy = b.fy.to_f64_lossy() as f32;
        let transform =
            Transform::from_translation(Vec3::new(b.ox as f32, b.oy as f32 - fy, b.oz as f32));
        if let Some(&entity) = entities.get(&b.id) {
            if let Ok((_, _, mut tr)) = old.get_mut(entity) {
                *tr = transform;
            }
            continue;
        }
        let mut combined: Option<Mesh> = None;
        for v in &b.voxels {
            let offset = Vec3::new(
                f32::from(v.x) + 0.5,
                f32::from(v.y) + 0.5,
                f32::from(v.z) + 0.5,
            );
            let mut cube = Mesh::from(Cuboid::new(0.98, 0.98, 0.98)).translated_by(offset);
            cube.insert_attribute(
                Mesh::ATTRIBUTE_COLOR,
                vec![fragments::material_colour(v.mat); cube.count_vertices()],
            );
            if let Some(m) = combined.as_mut() {
                m.merge(&cube).expect("same body attributes");
            } else {
                combined = Some(cube);
            }
        }
        let Some(mesh) = combined else {
            continue;
        };
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
