//! Exact presentation clipping of original debris meshes, including batch members.
use super::*;
use bevy::mesh::VertexAttributeValues;

#[derive(Component, Clone)]
pub struct Clipped {
    source: Handle<Mesh>,
    result: Option<Handle<Mesh>>,
    bounds: (Vec3, Vec3),
    key: Option<([u32; 14], u8)>,
    side: i8,
}
fn bounds(mesh: &Mesh) -> Option<(Vec3, Vec3)> {
    let VertexAttributeValues::Float32x3(points) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)? else {
        return None;
    };
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for &p in points {
        lo = lo.min(Vec3::from(p));
        hi = hi.max(Vec3::from(p));
    }
    (!points.is_empty()).then_some((lo, hi))
}
fn side(bounds: (Vec3, Vec3), transform: &Transform, plane: Vec4) -> i8 {
    let mut low = f32::INFINITY;
    let mut high = f32::NEG_INFINITY;
    for x in [bounds.0.x, bounds.1.x] {
        for y in [bounds.0.y, bounds.1.y] {
            for z in [bounds.0.z, bounds.1.z] {
                let d = plane
                    .truncate()
                    .dot(transform.transform_point(Vec3::new(x, y, z)))
                    + plane.w;
                low = low.min(d);
                high = high.max(d);
            }
        }
    }
    if low >= 0.0 {
        1
    } else if high < 0.0 {
        -1
    } else {
        0
    }
}
#[allow(clippy::type_complexity)]
pub fn draw(
    mut commands: Commands,
    lab: Res<Laboratory>,
    mut meshes: ResMut<Assets<Mesh>>,
    parents: Query<&Transform>,
    mut views: Query<
        (
            Entity,
            &Transform,
            Option<&ChildOf>,
            &mut Mesh3d,
            Option<&Clipped>,
        ),
        Or<(
            With<fragments::Surface>,
            With<fragments::batches::RestingBatch>,
            With<bodies::FallingBody>,
        )>,
    >,
) {
    let plane = (lab.active() && lab.tools.section.enabled).then(|| lab.tools.section.plane());
    for (entity, local, parent, mut current, clipped) in &mut views {
        let transform = parent
            .and_then(|p| parents.get(p.parent()).ok())
            .map_or(*local, |p| p.mul_transform(*local));
        let mut c = if let Some(c) = clipped {
            c.clone()
        } else {
            if plane.is_none() {
                continue;
            }
            let Some(bounds) = meshes.get(&current.0).and_then(bounds) else {
                continue;
            };
            Clipped {
                source: current.0.clone(),
                result: None,
                bounds,
                key: None,
                side: 1,
            }
        };
        let expected = if c.side == 1 {
            &c.source
        } else {
            c.result.as_ref().unwrap_or(&c.source)
        };
        if current.0 != *expected {
            c.source = current.0.clone();
            if let Some(b) = meshes.get(&c.source).and_then(bounds) {
                c.bounds = b;
            }
            c.key = None;
        }
        if plane.is_none() {
            current.0 = c.source.clone();
            if let Some(result) = &c.result {
                meshes.remove(result.id());
            }
            commands.entity(entity).remove::<Clipped>();
            continue;
        }
        let plane = plane.unwrap();
        // A retained source can swap its colour array without replacing its
        // handle. Reclip on metric transitions instead of retaining old paint.
        let key = (section::clip_key(&transform, plane), lab.metric_mode());
        if c.key == Some(key) {
            continue;
        }
        let next = side(c.bounds, &transform, plane);
        if next == 1 {
            current.0 = c.source.clone();
        } else if !(next == -1 && c.side == -1 && c.result.is_some()) {
            let result = if next < 0 {
                Some(section::section_mesh(&MeshBuf::default()))
            } else {
                meshes
                    .get(&c.source)
                    .and_then(|m| section::clip_mesh(m, &transform, plane, None))
            };
            if let Some(result) = result {
                if let Some(handle) = &c.result {
                    let _ = meshes.insert(handle.id(), result);
                    current.0 = handle.clone();
                } else {
                    let handle = meshes.add(result);
                    current.0 = handle.clone();
                    c.result = Some(handle);
                }
            }
        }
        c.side = next;
        c.key = Some(key);
        commands.entity(entity).insert(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_debris_sections_cover_far_instances_parent_pose_and_restore_latest_source() {
        let mut app = metric_fragments::tests::global_app(6);
        app.add_systems(Update, draw);
        let original = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(section::section_mesh(&render::model::fragment_to_mesh(
                1,
                1,
                [0.3, 0.4, 0.5, 1.0],
            )));
        let mut entities = Vec::new();
        for x in -80..80 {
            entities.push(
                app.world_mut()
                    .spawn((
                        fragments::Surface,
                        Mesh3d(original.clone()),
                        Transform::from_xyz(x as f32, 0.0, 0.0),
                    ))
                    .id(),
            );
        }
        let parent = app
            .world_mut()
            .spawn(Transform::from_xyz(3.0, 0.0, 0.0))
            .id();
        let child = app
            .world_mut()
            .spawn((
                fragments::Surface,
                Mesh3d(original.clone()),
                Transform::default(),
                ChildOf(parent),
            ))
            .id();
        {
            let mut lab = app.world_mut().resource_mut::<Laboratory>();
            lab.tools.section.enabled = true;
            lab.tools.section.axis = 0;
            lab.tools.section.anchor = [0; 3];
        }
        let before = app.world().resource::<Scene>().sim.state_hash();
        app.update();
        assert_ne!(app.world().get::<Mesh3d>(entities[0]).unwrap().0, original);
        assert_eq!(
            app.world().get::<Mesh3d>(entities[159]).unwrap().0,
            original
        );
        assert_eq!(app.world().get::<Mesh3d>(child).unwrap().0, original);
        app.world_mut()
            .get_mut::<Transform>(parent)
            .unwrap()
            .translation
            .x = -3.0;
        app.update();
        assert_ne!(app.world().get::<Mesh3d>(child).unwrap().0, original);
        let replacement =
            app.world_mut()
                .resource_mut::<Assets<Mesh>>()
                .add(section::section_mesh(&render::model::fragment_to_mesh(
                    1,
                    1,
                    [0.9, 0.2, 0.1, 1.0],
                )));
        app.world_mut().get_mut::<Mesh3d>(entities[0]).unwrap().0 = replacement.clone();
        app.update();
        app.world_mut()
            .resource_mut::<Laboratory>()
            .tools
            .section
            .enabled = false;
        app.update();
        assert_eq!(
            app.world().get::<Mesh3d>(entities[0]).unwrap().0,
            replacement
        );
        assert_eq!(app.world().get::<Mesh3d>(child).unwrap().0, original);
        assert_eq!(app.world().resource::<Scene>().sim.state_hash(), before);
    }
    #[test]
    fn section_bounds_classify_transformed_debris_on_all_axes_and_sides() {
        let bounds = (Vec3::splat(-1.0), Vec3::ONE);
        for axis in 0..3 {
            for sign in [-1.0, 1.0] {
                let mut n = Vec3::ZERO;
                n[axis] = sign;
                let plane = n.extend(0.0);
                assert_eq!(side(bounds, &Transform::default(), plane), 0);
                assert_eq!(
                    side(bounds, &Transform::from_translation(n * 3.0), plane),
                    1
                );
                assert_eq!(
                    side(bounds, &Transform::from_translation(n * -3.0), plane),
                    -1
                );
            }
        }
    }
}
