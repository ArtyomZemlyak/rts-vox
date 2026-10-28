//! A read-only, bounded display chassis around the simulation volume.
//! Three cached meshes, shared materials, no lights, shadows or SIM geometry.
use super::*;
use bevy::light::{NotShadowCaster, NotShadowReceiver};

#[derive(Resource)]
pub struct Enabled(pub bool);
#[derive(Component)]
pub struct Part;
#[derive(Component)]
pub struct Glass;
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    size: [u32; 3],
    ground: i32,
    lab: bool,
}
#[derive(Default)]
pub struct Cache {
    key: Option<Key>,
    meshes: Vec<Handle<Mesh>>,
    opaque: Option<Handle<StandardMaterial>>,
    glass: Option<Handle<StandardMaterial>>,
}
struct Model {
    chassis: MeshBuf,
    markings: MeshBuf,
    glass: MeshBuf,
}
const DARK: [f32; 4] = [0.020, 0.030, 0.044, 1.0];
const PANEL: [f32; 4] = [0.039, 0.057, 0.076, 1.0];
const EDGE: [f32; 4] = [0.093, 0.139, 0.174, 1.0];
const CYAN: [f32; 4] = [0.080, 0.52, 0.66, 1.0];
const AMBER: [f32; 4] = [0.72, 0.39, 0.095, 1.0];

fn quad(b: &mut MeshBuf, corners: [Vec3; 4], normal: Vec3, colour: [f32; 4]) {
    let i = b.pos.len() as u32;
    b.pos.extend(corners.map(|v| v.to_array()));
    b.nrm.extend([normal.to_array(); 4]);
    b.col.extend([colour; 4]);
    b.idx.extend([i, i + 1, i + 2, i, i + 2, i + 3]);
}
pub(super) fn block(b: &mut MeshBuf, lo: Vec3, hi: Vec3, colour: [f32; 4]) {
    for axis in 0..3 {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        for positive in [false, true] {
            let mut p = lo;
            p[axis] = if positive { hi[axis] } else { lo[axis] };
            let mut du = Vec3::ZERO;
            let mut dv = Vec3::ZERO;
            du[u] = hi[u] - lo[u];
            dv[v] = hi[v] - lo[v];
            let mut corners = [p, p + du, p + du + dv, p + dv];
            if !positive {
                corners.swap(1, 3);
            }
            let mut normal = Vec3::ZERO;
            normal[axis] = if positive { 1.0 } else { -1.0 };
            // Baked face contrast, independent of additional scene lighting.
            let gain = if axis == 1 && positive { 1.0 } else { 0.68 };
            quad(
                b,
                corners,
                normal,
                [
                    colour[0] * gain,
                    colour[1] * gain,
                    colour[2] * gain,
                    colour[3],
                ],
            );
        }
    }
}
fn top(b: &mut MeshBuf, x: [f32; 2], z: [f32; 2], y: f32, colour: [f32; 4]) {
    quad(
        b,
        [
            Vec3::new(x[0], y, z[0]),
            Vec3::new(x[0], y, z[1]),
            Vec3::new(x[1], y, z[1]),
            Vec3::new(x[1], y, z[0]),
        ],
        Vec3::Y,
        colour,
    );
}
// All four panes face INTO the tank. Back-face culling removes the near
// panes automatically from either side of the map, without a transparent lid.
fn wall(
    b: &mut MeshBuf,
    axis: usize,
    high: bool,
    size: [f32; 3],
    along: [f32; 2],
    y: [f32; 2],
    inset: f32,
    colour: [f32; 4],
) {
    let sign = if high { -1.0 } else { 1.0 };
    let at = if high {
        size[axis] + 0.8 - inset
    } else {
        -0.8 + inset
    };
    let mut c = if axis == 0 {
        [
            Vec3::new(at, y[0], along[0]),
            Vec3::new(at, y[1], along[0]),
            Vec3::new(at, y[1], along[1]),
            Vec3::new(at, y[0], along[1]),
        ]
    } else {
        [
            Vec3::new(along[0], y[0], at),
            Vec3::new(along[1], y[0], at),
            Vec3::new(along[1], y[1], at),
            Vec3::new(along[0], y[1], at),
        ]
    };
    if high {
        c.swap(1, 3);
    }
    let mut normal = Vec3::ZERO;
    normal[axis] = sign;
    quad(b, c, normal, colour);
}
fn tank_top(key: Key) -> f32 {
    key.ground as f32 + (key.size[0].min(key.size[2]) as f32 * 0.17).clamp(40.0, 128.0)
}
fn inside_tank(key: Key, camera: Vec3) -> bool {
    camera.x > 0.0
        && camera.x < key.size[0] as f32
        && camera.z > 0.0
        && camera.z < key.size[2] as f32
        && camera.y < tank_top(key)
}
fn build(key: Key) -> Model {
    let s = key.size.map(|v| v as f32);
    let ground = key.ground as f32;
    let scale = (s[0].min(s[2]) / 640.0).clamp(0.5, 4.0);
    let rim = 8.0 * scale;
    let mut chassis = MeshBuf::default();
    let mut markings = MeshBuf::default();
    let mut glass = MeshBuf::default();
    // Dark studio floor, then a finite sparse grid. Nothing is placed on
    // top of the playable ground, and no per-world-voxel objects are created.
    let reach = s[0].max(s[2]) * 6.0;
    let floor = -15.0 * scale;
    top(
        &mut chassis,
        [-reach, s[0] + reach],
        [-reach, s[2] + reach],
        floor,
        [0.009, 0.017, 0.026, 1.0],
    );
    let margin = s[0].max(s[2]) * 0.8;
    for i in 0..=64 {
        let t = i as f32 / 64.0;
        let x = -margin + t * (s[0] + margin * 2.0);
        let z = -margin + t * (s[2] + margin * 2.0);
        let major = i % 8 == 0;
        let width = if major { 0.45 } else { 0.18 } * scale;
        let colour = if major {
            [0.026, 0.061, 0.079, 1.0]
        } else {
            [0.015, 0.030, 0.043, 1.0]
        };
        top(
            &mut markings,
            [x - width, x + width],
            [-margin, s[2] + margin],
            floor + 0.04,
            colour,
        );
        top(
            &mut markings,
            [-margin, s[0] + margin],
            [z - width, z + width],
            floor + 0.05,
            colour,
        );
    }
    block(
        &mut chassis,
        Vec3::new(-rim * 1.5, -10.0 * scale, -rim * 1.5),
        Vec3::new(s[0] + rim * 1.5, -1.0, s[2] + rim * 1.5),
        DARK,
    );
    for axis in [0, 2] {
        let other = 2 - axis;
        for high in [false, true] {
            let edge = if high { s[axis] } else { 0.0 };
            let a = if high { edge + 0.25 } else { edge - rim };
            let b = if high { edge + rim } else { edge - 0.25 };
            let mut lo = Vec3::new(-rim, -2.0, -rim);
            let mut hi = Vec3::new(s[0] + rim, ground - 3.0, s[2] + rim);
            lo[axis] = a;
            hi[axis] = b;
            block(&mut chassis, lo, hi, PANEL);
            lo.y = ground - 3.0;
            hi.y = ground - 1.3;
            block(&mut chassis, lo, hi, EDGE);
            // A recessed light rail and a calibrated segmented outer rail.
            let width = (b - a) * 0.12;
            lo[axis] = (a + b) * 0.5 - width;
            hi[axis] = (a + b) * 0.5 + width;
            lo.y = ground - 1.25;
            hi.y = ground - 1.0;
            block(&mut markings, lo, hi, CYAN);
            for i in 0..32 {
                let t = (i as f32 + 0.5) / 32.0 * s[other];
                let mut p = Vec3::ZERO;
                let mut q = Vec3::ZERO;
                p[axis] = a;
                q[axis] = b;
                p[other] = t - scale * 0.5;
                q[other] = t + scale * 0.5;
                p.y = ground - 0.9;
                q.y = ground - 0.65;
                block(&mut markings, p, q, if i % 8 == 0 { AMBER } else { EDGE });
                // Cooling slots / pixel packets underneath the calibrated lip.
                p[axis] = if high { b } else { a - scale * 0.8 };
                q[axis] = p[axis] + scale * 0.8;
                p[other] = t - scale * 2.0;
                q[other] = t + scale * 2.0;
                p.y = ground - 7.0 * scale;
                q.y = ground - 4.0 * scale;
                block(&mut markings, p, q, if i % 8 == 1 { CYAN } else { DARK });
            }
        }
    }
    // Small corner housings and pin/contact details make the terrain read
    // as a removable simulated specimen rather than a flat boundary line.
    for x in [-rim * 0.5, s[0] + rim * 0.5] {
        for z in [-rim * 0.5, s[2] + rim * 0.5] {
            let r = rim * 0.68;
            block(
                &mut chassis,
                Vec3::new(x - r, -5.0, z - r),
                Vec3::new(x + r, ground - 0.5, z + r),
                EDGE,
            );
            block(
                &mut markings,
                Vec3::new(x - r * 0.6, ground - 0.4, z - r * 0.6),
                Vec3::new(x + r * 0.6, ground - 0.1, z + r * 0.6),
                AMBER,
            );
        }
    }
    if key.lab {
        let bottom = ground + 0.25;
        let ceiling = tank_top(key);
        for axis in [0, 2] {
            let span = s[2 - axis];
            for high in [false, true] {
                // Slightly graduated sheet, no screen sampling, reflections,
                // refraction, transmission or new fullscreen effect.
                wall(
                    &mut glass,
                    axis,
                    high,
                    s,
                    [0.0, span],
                    [bottom, ceiling],
                    0.0,
                    [0.10, 0.43, 0.50, 0.028],
                );
                wall(
                    &mut markings,
                    axis,
                    high,
                    s,
                    [0.0, span],
                    [ceiling - 0.7 * scale, ceiling],
                    0.06,
                    EDGE,
                );
                for along in [[0.0, 0.7 * scale], [span - 0.7 * scale, span]] {
                    wall(
                        &mut markings,
                        axis,
                        high,
                        s,
                        along,
                        [bottom, ceiling],
                        0.06,
                        CYAN,
                    );
                }
                for tick in 1..=16 {
                    let y = bottom + (ceiling - bottom) * tick as f32 / 17.0;
                    let length = if tick % 4 == 0 { 8.0 } else { 4.0 } * scale;
                    for along in [
                        [1.8 * scale, (1.8 * scale + length).min(span)],
                        [(span - 1.8 * scale - length).max(0.0), span - 1.8 * scale],
                    ] {
                        wall(
                            &mut markings,
                            axis,
                            high,
                            s,
                            along,
                            [y, y + 0.25 * scale],
                            0.06,
                            if tick % 4 == 0 { CYAN } else { EDGE },
                        );
                    }
                }
            }
        }
    }
    Model {
        chassis,
        markings,
        glass,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    scene: Res<Scene>,
    enabled: Res<Enabled>,
    mut cache: Local<Cache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut old: Query<(Entity, Option<&Glass>, &mut Visibility), With<Part>>,
    cameras: Query<&Transform, With<GasCamera>>,
) {
    let key = Key {
        size: scene.sim.size(),
        ground: scene.sim.ground_level(),
        lab: scene.sim.layout.as_ref().is_some_and(|l| l.laboratory),
    };
    if !enabled.0 {
        for (_, _, mut visible) in &mut old {
            if *visible != Visibility::Hidden {
                *visible = Visibility::Hidden;
            }
        }
        return;
    }
    if cache.key == Some(key) {
        let inside = key.lab && cameras.iter().any(|c| inside_tank(key, c.translation));
        for (_, glass, mut visible) in &mut old {
            let want = if glass.is_some() && inside {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
            if *visible != want {
                *visible = want;
            }
        }
        return;
    }
    for (e, _, _) in &old {
        commands.entity(e).despawn();
    }
    for handle in cache.meshes.drain(..) {
        meshes.remove(handle.id());
    }
    let opaque = cache
        .opaque
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                perceptual_roughness: 1.0,
                ..default()
            })
        })
        .clone();
    let glass = cache
        .glass
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                cull_mode: Some(bevy::render::render_resource::Face::Back),
                ..default()
            })
        })
        .clone();
    let model = build(key);
    for (buf, transparent) in [
        (model.chassis, false),
        (model.markings, false),
        (model.glass, true),
    ] {
        if buf.pos.is_empty() {
            continue;
        }
        let handle = meshes.add(mesh(&buf));
        cache.meshes.push(handle.clone());
        let mut entity = commands.spawn((
            Part,
            Mesh3d(handle),
            MeshMaterial3d(if transparent {
                glass.clone()
            } else {
                opaque.clone()
            }),
            Transform::default(),
            Visibility::Inherited,
            NotShadowCaster,
            NotShadowReceiver,
        ));
        if transparent {
            entity.insert(Glass);
            if cameras.iter().any(|c| inside_tank(key, c.translation)) {
                entity.insert(Visibility::Hidden);
            }
        }
    }
    cache.key = Some(key);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surround_cache_survives_reset_without_asset_growth_and_never_writes_sim() {
        let mut layout = modding::laboratory::Definition::stock().layout;
        layout.laboratory = true;
        let sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
        let mut app = panel::hud::tests::app_with_scene(sim, Workshop::new(Some(layout), false));
        app.insert_resource(Enabled(true))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, update);
        let camera = app
            .world_mut()
            .spawn((GasCamera, Transform::from_xyz(-200.0, 300.0, 900.0)))
            .id();
        let before = app.world().resource::<Scene>().sim.state_hash();
        app.update();
        let initial_assets = app.world().resource::<Assets<Mesh>>().len();
        let initial_materials = app.world().resource::<Assets<StandardMaterial>>().len();
        assert_eq!((initial_assets, initial_materials), (3, 2));
        let handles = |app: &mut App| {
            let w = app.world_mut();
            w.query_filtered::<&Mesh3d, With<Part>>()
                .iter(w)
                .map(|m| m.0.id())
                .collect::<Vec<_>>()
        };
        let initial = handles(&mut app);
        for _ in 0..20 {
            app.update();
        }
        app.world_mut().resource_mut::<Scene>().generation += 1;
        app.update();
        assert_eq!(
            handles(&mut app),
            initial,
            "same-size restart must reuse geometry"
        );
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation = Vec3::new(320.0, 30.0, 320.0);
        app.update();
        let w = app.world_mut();
        assert_eq!(
            *w.query_filtered::<&Visibility, With<Glass>>()
                .single(w)
                .unwrap(),
            Visibility::Hidden
        );
        app.world_mut().resource_mut::<Enabled>().0 = false;
        app.update();
        let w = app.world_mut();
        assert!(w
            .query_filtered::<&Visibility, With<Part>>()
            .iter(w)
            .all(|v| *v == Visibility::Hidden));
        app.world_mut().resource_mut::<Enabled>().0 = true;
        app.update();
        assert_eq!(handles(&mut app), initial);
        assert_eq!(app.world().resource::<Scene>().sim.state_hash(), before);
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .layout
            .as_mut()
            .unwrap()
            .laboratory = false;
        app.update();
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 2);
        assert_eq!(
            app.world().resource::<Assets<StandardMaterial>>().len(),
            initial_materials
        );
        let w = app.world_mut();
        assert_eq!(w.query_filtered::<Entity, With<Glass>>().iter(w).count(), 0);
    }
    #[test]
    fn surround_has_bounded_geometry_even_on_the_largest_map() {
        for size in [[128, 128, 128], [640, 192, 640], [100000, 4096, 80000]] {
            for lab in [false, true] {
                let m = build(Key {
                    size,
                    ground: 16,
                    lab,
                });
                let total: usize = [&m.chassis, &m.markings, &m.glass]
                    .iter()
                    .map(|b| b.pos.len())
                    .sum();
                assert!(total < 16000, "unbounded decoration: {total}");
                assert_eq!(m.glass.pos.len(), if lab { 16 } else { 0 });
                for b in [&m.chassis, &m.markings, &m.glass] {
                    assert_eq!(b.pos.len(), b.col.len());
                    assert_eq!(b.pos.len(), b.nrm.len());
                    assert!(b.pos.iter().flatten().all(|v| v.is_finite()));
                    for t in b.idx.chunks_exact(3) {
                        let p = [t[0], t[1], t[2]].map(|i| Vec3::from(b.pos[i as usize]));
                        assert!(
                            (p[1] - p[0])
                                .cross(p[2] - p[0])
                                .dot(Vec3::from(b.nrm[t[0] as usize]))
                                > 0.0
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn surround_panes_face_inward_and_never_cover_the_near_side() {
        let key = Key {
            size: [640, 192, 640],
            ground: 16,
            lab: true,
        };
        let m = build(key);
        for camera in [
            Vec3::new(-200.0, 180.0, 900.0),
            Vec3::new(900.0, 180.0, -200.0),
        ] {
            let mut visible = 0;
            for (p, n) in m.glass.pos.chunks_exact(4).zip(m.glass.nrm.chunks_exact(4)) {
                let centre = p.iter().map(|p| Vec3::from(*p)).sum::<Vec3>() / 4.0;
                let normal = Vec3::from(n[0]);
                if normal.dot(camera - centre) > 0.0 {
                    visible += 1;
                    // A visible pane is on the far side of the tank centre.
                    assert!(normal.dot(Vec3::new(320.0, centre.y, 320.0) - centre) > 0.0);
                }
            }
            assert_eq!(visible, 2);
        }
        assert!(inside_tank(key, Vec3::new(320.0, 40.0, 320.0)));
        assert!(!inside_tank(key, Vec3::new(320.0, 400.0, 320.0)));
    }
}
