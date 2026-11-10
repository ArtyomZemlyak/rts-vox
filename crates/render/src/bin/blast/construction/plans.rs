//! Pooled, nonphysical construction holograms, sharing placement templates.
use super::*;
use bevy::{
    camera::visibility::RenderLayers,
    light::{NotShadowCaster, NotShadowReceiver},
};

#[derive(Component)]
pub struct PlanMesh;
struct Entry {
    site: usize,
    ghost: Entity,
    brackets: Vec<Entity>,
}
struct Art {
    cube: Handle<Mesh>,
    ghosts: [Handle<StandardMaterial>; 3],
    frames: [Handle<StandardMaterial>; 3],
}
#[derive(Resource, Default)]
pub struct Visuals {
    entries: Vec<Entry>,
    art: Option<Art>,
    generation: Option<u64>,
}
fn styles(scene: &Scene) -> BTreeMap<usize, usize> {
    let mut styles = BTreeMap::new();
    for (id, u) in scene.sim.units.iter().enumerate().filter(|(_, u)| {
        u.status.alive
            && matches!(
                u.kind,
                RangeUnitKind::Commander | RangeUnitKind::SkobaDrone | RangeUnitKind::GrebenStorage
            )
    }) {
        let _ = u;
        for (index, p) in scene.sim.order_queue(id).into_iter().enumerate() {
            if let sim_core::blast::battle::Order::Build(site) = p.order {
                let state = usize::from(index > 0);
                styles
                    .entry(site)
                    .and_modify(|s: &mut usize| *s = (*s).min(state))
                    .or_insert(state);
            }
        }
    }
    styles
}
fn brackets(lo: Vec3, hi: Vec3, y: f32) -> Vec<Transform> {
    let mut out = Vec::with_capacity(12);
    let arm = ((hi.x - lo.x).min(hi.z - lo.z) * 0.22).clamp(3.0, 14.0);
    for (x, sx) in [(lo.x, 1.0), (hi.x, -1.0)] {
        for (z, sz) in [(lo.z, 1.0), (hi.z, -1.0)] {
            out.push(
                Transform::from_xyz(x + sx * arm * 0.5, y, z).with_scale(Vec3::new(arm, 0.7, 0.7)),
            );
            out.push(
                Transform::from_xyz(x, y, z + sz * arm * 0.5).with_scale(Vec3::new(0.7, 0.7, arm)),
            );
            out.push(Transform::from_xyz(x, y + 2.2, z).with_scale(Vec3::new(0.7, 4.4, 0.7)));
        }
    }
    out
}
pub fn update(
    mut commands: Commands,
    scene: Res<Scene>,
    edit: Res<Workshop>,
    lab: Res<laboratory::Laboratory>,
    templates: Option<Res<Templates>>,
    mut visuals: ResMut<Visuals>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut parts: Query<
        (
            &mut Visibility,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        With<PlanMesh>,
    >,
    mut gizmos: Gizmos,
) {
    let Some(templates) = templates else {
        return;
    };
    if visuals.generation != Some(scene.generation) {
        for entry in visuals.entries.drain(..) {
            commands.entity(entry.ghost).despawn();
            for e in entry.brackets {
                commands.entity(e).despawn();
            }
        }
        visuals.generation = Some(scene.generation);
    }
    let sites: Vec<_> = if scene.sim.battle_mode() && !lab.active() {
        scene
            .sim
            .construction_sites()
            .into_iter()
            .filter(|s| {
                !s.complete && !s.cancelled && !s.destroyed && (s.allied || s.unit.is_some())
            })
            .collect()
    } else {
        Vec::new()
    };
    if !sites.is_empty() && visuals.art.is_none() {
        let colours = [
            Color::srgb(0.18, 0.9, 0.95),
            Color::srgb(0.49, 0.52, 1.0),
            Color::srgb(0.95, 0.67, 0.25),
        ];
        let ghosts = std::array::from_fn(|i| {
            materials.add(StandardMaterial {
                base_color: colours[i].with_alpha([0.10, 0.18, 0.12][i]),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })
        });
        let frames = colours.map(|c| {
            materials.add(StandardMaterial {
                base_color: c,
                unlit: true,
                ..default()
            })
        });
        visuals.art = Some(Art {
            cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            ghosts,
            frames,
        });
    }
    for entry in &visuals.entries {
        if !sites.iter().any(|s| s.id == entry.site) {
            for entity in std::iter::once(entry.ghost).chain(entry.brackets.iter().copied()) {
                if let Ok((mut visible, _, _)) = parts.get_mut(entity) {
                    if *visible != Visibility::Hidden {
                        *visible = Visibility::Hidden;
                    }
                }
            }
        }
    }
    let states = styles(&scene);
    for site in &sites {
        let Some(mesh) = templates.mesh(site.kind) else {
            continue;
        };
        let state = states
            .get(&site.id)
            .copied()
            .unwrap_or(if site.quota > 0 { 0 } else { 2 });
        let art = visuals.art.as_ref().unwrap();
        let ghost_colour = art.ghosts[state].clone();
        let frame_colour = art.frames[state].clone();
        let cube = art.cube.clone();
        let pos = Vec3::from(site.at.map(|v| v as f32));
        let rotation = Quat::from_rotation_y(if site.allied {
            0.0
        } else {
            std::f32::consts::PI
        });
        let pose = Transform::from_translation(pos).with_rotation(rotation);
        let (local_lo, local_hi) = templates.bounds(site.kind);
        let a = rotation * local_lo;
        let b = rotation * local_hi;
        let frames = brackets(pos + a.min(b), pos + a.max(b), pos.y + 0.45);
        if !visuals.entries.iter().any(|e| e.site == site.id) {
            if let Some(entry) = visuals
                .entries
                .iter_mut()
                .find(|entry| !sites.iter().any(|s| s.id == entry.site))
            {
                entry.site = site.id;
                commands.entity(entry.ghost).insert(Mesh3d(mesh.clone()));
            } else {
                let ghost = commands
                    .spawn((
                        PlanMesh,
                        Mesh3d(mesh),
                        MeshMaterial3d(ghost_colour.clone()),
                        pose,
                        Visibility::Visible,
                        RenderLayers::layer(1),
                        NotShadowCaster,
                        NotShadowReceiver,
                    ))
                    .id();
                let mut ids = Vec::new();
                for transform in &frames {
                    ids.push(
                        commands
                            .spawn((
                                PlanMesh,
                                Mesh3d(cube.clone()),
                                MeshMaterial3d(frame_colour.clone()),
                                *transform,
                                Visibility::Visible,
                                RenderLayers::layer(1),
                                NotShadowCaster,
                                NotShadowReceiver,
                            ))
                            .id(),
                    );
                }
                visuals.entries.push(Entry {
                    site: site.id,
                    ghost,
                    brackets: ids,
                });
            }
        }
        let entry = visuals.entries.iter().find(|e| e.site == site.id).unwrap();
        for (entity, pose, colour) in std::iter::once((entry.ghost, pose, &ghost_colour)).chain(
            entry
                .brackets
                .iter()
                .zip(frames)
                .map(|(&e, t)| (e, t, &frame_colour)),
        ) {
            if let Ok((mut visible, mut transform, mut material)) = parts.get_mut(entity) {
                if *visible != Visibility::Visible {
                    *visible = Visibility::Visible;
                }
                if *transform != pose {
                    *transform = pose;
                }
                if material.0 != *colour {
                    material.0 = colour.clone();
                }
            }
        }
    }
    if lab.active() {
        return;
    }
    for actor in edit.battle_selected_ids(&scene.sim) {
        let queue = scene.sim.order_queue(actor);
        if queue.len() < 2 {
            continue;
        }
        let mut from = Vec3::from(scene.sim.units[actor].at.map(|v| v as f32)) + Vec3::Y * 3.0;
        for p in queue {
            let at = match p.order {
                sim_core::blast::battle::Order::Build(site) => scene
                    .sim
                    .construction_site_state(site)
                    .filter(|s| !s.cancelled && !s.destroyed)
                    .map(|s| s.at),
                sim_core::blast::battle::Order::Move => p
                    .destination
                    .map(|[x, z]| [x, sim_core::blast::motion::floor(&scene.sim.world, x, z), z]),
                sim_core::blast::battle::Order::Attack(id)
                | sim_core::blast::battle::Order::Repair(id) => {
                    scene.sim.units.get(id).map(|u| u.at)
                }
                sim_core::blast::battle::Order::Dig(at)
                | sim_core::blast::battle::Order::Reclaim(at)
                | sim_core::blast::battle::Order::Ground(at) => Some(at),
                _ => None,
            };
            if let Some(at) = at {
                let to = Vec3::from(at.map(|v| v as f32)) + Vec3::Y * 3.0;
                gizmos.line(from, to, Color::srgba(0.49, 0.65, 1.0, 0.7));
                from = to;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foundation_is_twelve_bounded_shared_mesh_instances() {
        let frames = brackets(
            Vec3::new(-20.0, 0.0, -10.0),
            Vec3::new(20.0, 30.0, 10.0),
            16.45,
        );
        assert_eq!(frames.len(), 12);
        assert!(frames
            .iter()
            .all(|t| t.translation.is_finite() && t.scale.cmpgt(Vec3::ZERO).all()));
        assert!(frames
            .iter()
            .all(|t| t.scale.x <= 14.0 && t.scale.z <= 14.0));
    }
}
