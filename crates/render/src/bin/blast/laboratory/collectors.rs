//! Cached laboratory collector/depot visuals. Reads the ordinary SIM ledger.
use super::*;
#[derive(Component)]
pub struct Part {
    index: usize,
    kind: u8,
}
#[derive(Default)]
pub struct Visual {
    assets: Option<[Handle<Mesh>; 3]>,
    material: Option<Handle<StandardMaterial>>,
    preview: Option<Handle<StandardMaterial>>,
    slots: usize,
    poses: Vec<(Vec3, Vec3, u32, u64)>,
}
fn boxes(parts: &[([f32; 3], [f32; 3], [f32; 4])]) -> Mesh {
    let mut b = MeshBuf::default();
    for &(lo, hi, c) in parts {
        super::super::surround::block(&mut b, Vec3::from(lo), Vec3::from(hi), c);
    }
    mesh(&b)
}
fn models() -> [Mesh; 3] {
    let dark = [0.035, 0.055, 0.07, 1.0];
    let steel = [0.16, 0.22, 0.25, 1.0];
    let yellow = [0.78, 0.40, 0.055, 1.0];
    let cyan = [0.08, 0.62, 0.72, 1.0];
    [
        boxes(&[
            ([-6.0, 0.0, -6.0], [6.0, 2.3, -3.4], dark),
            ([-6.0, 0.0, 3.4], [6.0, 2.3, 6.0], dark),
            ([-5.0, 1.8, -4.5], [5.5, 3.8, 4.5], yellow),
            ([-5.0, 3.8, -4.5], [-3.8, 7.0, 4.5], steel),
            ([-3.8, 3.8, -4.5], [2.4, 6.2, -3.4], steel),
            ([-3.8, 3.8, 3.4], [2.4, 6.2, 4.5], steel),
            ([2.4, 3.8, -4.5], [4.0, 6.2, 4.5], yellow),
            ([4.0, 2.4, -3.0], [8.5, 3.4, 3.0], dark),
            ([7.7, 1.2, -3.8], [9.0, 3.2, 3.8], cyan),
            ([-5.2, 5.3, -0.6], [-4.6, 7.5, 0.6], cyan),
        ]),
        boxes(&[([-3.7, 0.0, -3.3], [2.3, 1.0, 3.3], [0.37, 0.30, 0.21, 1.0])]),
        boxes(&[
            ([-10.0, -0.3, -10.0], [10.0, 0.25, 10.0], steel),
            ([-10.0, 0.25, -10.0], [-8.8, 0.55, 10.0], cyan),
            ([8.8, 0.25, -10.0], [10.0, 0.55, 10.0], cyan),
            ([-10.0, 0.25, -10.0], [10.0, 0.55, -8.8], cyan),
            ([-10.0, 0.25, 8.8], [10.0, 0.55, 10.0], cyan),
            ([-13.0, 0.0, -5.0], [-10.0, 7.0, 5.0], yellow),
            ([-13.1, 4.0, -3.0], [-12.9, 6.0, 3.0], cyan),
        ]),
    ]
}
pub fn draw(
    mut commands: Commands,
    lab: Res<Laboratory>,
    scene: Res<Scene>,
    mut cache: Local<Visual>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut parts: Query<(&Part, &mut Transform, &mut Visibility)>,
) {
    let devices = scene
        .sim
        .layout
        .as_ref()
        .map(|l| l.lab_devices.as_slice())
        .unwrap_or_default();
    let needed = if lab.active() {
        devices
            .iter()
            .rposition(|d| d.collector.is_some())
            .map_or(0, |i| i + 1)
    } else {
        0
    };
    let preview = lab
        .tools
        .device_preview
        .filter(|_| lab.active() && lab.constructor.mode == constructor::Mode::Collector);
    if (needed > 0 || preview.is_some()) && cache.assets.is_none() {
        cache.assets = Some(models().map(|m| meshes.add(m)));
        cache.material = Some(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            ..default()
        }));
        let material = materials.add(StandardMaterial {
            base_color: Color::srgba(0.2, 1.0, 0.5, 0.65),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        commands.spawn((
            Part { index: 0, kind: 3 },
            Mesh3d(cache.assets.as_ref().unwrap()[0].clone()),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            Visibility::Hidden,
            bevy::light::NotShadowCaster,
        ));
        cache.preview = Some(material);
    }
    while cache.slots < needed {
        let index = cache.slots;
        for kind in 0..3u8 {
            commands.spawn((
                Part { index, kind },
                Mesh3d(cache.assets.as_ref().unwrap()[kind as usize].clone()),
                MeshMaterial3d(cache.material.as_ref().unwrap().clone()),
                Transform::default(),
                Visibility::Hidden,
                bevy::light::NotShadowCaster,
            ));
        }
        cache.slots += 1;
    }
    cache
        .poses
        .resize(needed, (Vec3::ZERO, Vec3::ZERO, u32::MAX, u64::MAX));
    let alpha = if scene.paused {
        1.0
    } else {
        (scene.acc * sim_core::TICKS_PER_SECOND as f32).clamp(0.0, 1.0)
    };
    for (i, d) in devices.iter().enumerate().take(needed) {
        let p = Vec3::from(d.at.map(|v| v as f32));
        let old = &mut cache.poses[i];
        if old.3 != scene.generation {
            *old = (p, p, scene.sim.tick, scene.generation);
        } else if old.2 != scene.sim.tick {
            *old = (old.1, p, scene.sim.tick, scene.generation);
        }
    }
    for (part, mut transform, mut visibility) in &mut parts {
        if part.kind == 3 {
            if let Some((at, valid)) = preview {
                *transform = Transform::from_translation(Vec3::from(at.map(|v| v as f32)))
                    .with_rotation(Quat::from_rotation_y(
                        -(lab.constructor.turns as f32) * std::f32::consts::FRAC_PI_2,
                    ));
                *visibility = Visibility::Inherited;
                if let Some(mut material) =
                    cache.preview.as_ref().and_then(|h| materials.get_mut(h))
                {
                    material.base_color = if valid {
                        Color::srgba(0.2, 1.0, 0.5, 0.65)
                    } else {
                        Color::srgba(1.0, 0.15, 0.1, 0.65)
                    };
                }
            } else {
                *visibility = Visibility::Hidden;
            }
            continue;
        }
        let Some(d) = devices.get(part.index).filter(|_| part.index < needed) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let Some(c) = &d.collector else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let held = d
            .container
            .map_or(0, |id| scene.sim.matter.economy.ledger.held(id));
        let state = cache.poses[part.index];
        let p = state.0.lerp(state.1, alpha);
        let rotation = Quat::from_rotation_y(-(d.turns as f32) * std::f32::consts::FRAC_PI_2);
        *transform = Transform {
            translation: if part.kind == 2 {
                Vec3::from(c.dock.map(|v| v as f32))
            } else {
                p
            },
            rotation: if part.kind == 2 {
                Quat::IDENTITY
            } else {
                rotation
            },
            ..default()
        };
        if part.kind == 1 {
            transform.translation += Vec3::Y * 3.8;
            transform.scale.y = held as f32 / c.capacity as f32 * 2.4;
        }
        *visibility = if part.kind == 1 && held == 0 {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collector_visual_reuses_meshes_and_restart_does_not_mutate_sim() {
        let mut app = inspector_tests::inspection_app();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, draw.after(actions));
        integration_tests::enter(&mut app);
        let index = app
            .world()
            .resource::<Laboratory>()
            .experiments
            .iter()
            .position(|e| e.id == "crawler-salvage")
            .unwrap();
        integration_tests::click(&mut app, LabAction::Experiment(index));
        let before = app.world().resource::<Scene>().sim.state_hash();
        for _ in 0..12 {
            app.update();
        }
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 3);
        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 2);
        assert_eq!(app.world().resource::<Scene>().sim.state_hash(), before);
        integration_tests::click(&mut app, LabAction::Experiment(index));
        app.update();
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 3);
        let w = app.world_mut();
        assert_eq!(w.query::<&Part>().iter(w).count(), 4);
        let lab = w.resource::<Laboratory>();
        assert!(ui::enabled(lab, LabAction::Tool(7)));
    }
}
