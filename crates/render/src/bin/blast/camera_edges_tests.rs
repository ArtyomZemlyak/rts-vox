//! Exercise the actual framing system, including pan input and HUD placement.
use super::*;
use bevy::camera::{CameraProjection, RenderTargetInfo};

fn app() -> (App, Entity) {
    let mut app = panel::hud::tests::app();
    app.init_resource::<pilot::Pilot>()
        .init_resource::<fire::Effects>()
        .init_resource::<moving::Visual>()
        .init_resource::<streaming::Streaming>()
        .init_resource::<profile::Profile>()
        .init_resource::<Time<Real>>()
        .add_systems(Update, framing.after(panel::actions));
    {
        let mut edit = app.world_mut().resource_mut::<Workshop>();
        edit.zone = edit
            .layout
            .as_ref()
            .unwrap()
            .zones
            .iter()
            .position(|z| z.min[0] > 8000 && z.min[1] > 8000)
            .unwrap();
    }
    let entity = app
        .world_mut()
        .spawn((
            GasCamera,
            Camera::default(),
            Transform::default(),
            Projection::Orthographic(OrthographicProjection::default_3d()),
        ))
        .id();
    (app, entity)
}

fn viewport(app: &mut App, entity: Entity, size: UVec2) {
    app.world_mut()
        .get_mut::<Camera>(entity)
        .unwrap()
        .computed
        .target_info = Some(RenderTargetInfo {
        physical_size: size,
        scale_factor: 1.0,
    });
}

fn project(app: &App, entity: Entity, viewport: UVec2, point: Vec3) -> Vec2 {
    let world = app.world();
    let transform = world.get::<Transform>(entity).unwrap();
    let Projection::Orthographic(p) = world.get::<Projection>(entity).unwrap() else {
        panic!("orthographic view")
    };
    let bevy::camera::ScalingMode::FixedVertical {
        viewport_height: height,
    } = p.scaling_mode
    else {
        panic!("fixed height")
    };
    let half = Vec2::new(height * viewport.x as f32 / viewport.y as f32, height) * 0.5;
    let p = OrthographicProjection {
        area: Rect::from_corners(-half, half),
        ..p.clone()
    };
    let mut camera = Camera::default();
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: viewport,
        scale_factor: 1.0,
    });
    camera.computed.clip_from_view = p.get_clip_from_view();
    camera
        .world_to_viewport(&GlobalTransform::from(*transform), point)
        .unwrap()
}

#[test]
fn camera_edges_reach_all_corners_after_zoom_orbit_and_view_fitting() {
    let (mut app, entity) = app();
    let size = app.world().resource::<Scene>().sim.size();
    for view in [1, 6] {
        app.world_mut().resource_mut::<Scene>().view = view;
        for pixels in [
            UVec2::new(1600, 900),
            UVec2::new(2560, 1080),
            UVec2::new(900, 1200),
        ] {
            viewport(&mut app, entity, pixels);
            let ui = panel::hud::scale(pixels.as_vec2());
            let centre = Vec2::new(
                pixels.x as f32 * 0.5,
                (pixels.y as f32 + (72.0 - 238.0) * ui) * 0.5,
            );
            for zoom in [0.3, 1.0, 3.0] {
                for orbit in [0.0, 1.2, 3.8] {
                    for pitch in [-0.4, 0.4] {
                        for signs in [
                            Vec2::ONE,
                            -Vec2::ONE,
                            Vec2::new(-1.0, 1.0),
                            Vec2::new(1.0, -1.0),
                        ] {
                            {
                                let mut edit = app.world_mut().resource_mut::<Workshop>();
                                edit.zoom = zoom;
                                edit.orbit = orbit;
                                edit.pitch = pitch;
                                edit.pan = Vec2::ZERO;
                                edit.pan_by(signs * 50000.0);
                            }
                            app.update();
                            let p = Vec3::new(
                                if signs.x > 0.0 { size[0] as f32 } else { 0.0 },
                                34.0,
                                if signs.y > 0.0 { size[2] as f32 } else { 0.0 },
                            );
                            let pixel = project(&app, entity, pixels, p);
                            assert!(pixel.distance(centre) < 1.0,
                                "edge not centred: view={view} pixels={pixels} zoom={zoom} orbit={orbit} pitch={pitch} signs={signs} pixel={pixel} centre={centre}");
                            // Zoom again at the boundary, without another
                            // drag to repair its focus. Both zoom directions
                            // must preserve the point the player was viewing.
                            app.world_mut().resource_mut::<Workshop>().zoom =
                                if zoom < 1.0 { 3.0 } else { 0.3 };
                            app.update();
                            let pixel = project(&app, entity, pixels, p);
                            assert!(pixel.distance(centre) < 1.0,
                                "zoom lost edge: view={view} signs={signs} pixel={pixel} centre={centre}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn camera_edges_reverse_immediately_after_repeated_outward_drags() {
    let (mut app, entity) = app();
    let pixels = UVec2::new(1600, 900);
    viewport(&mut app, entity, pixels);
    app.world_mut().resource_mut::<Scene>().view = 6;
    for _ in 0..8 {
        app.world_mut()
            .resource_mut::<Workshop>()
            .pan_by(Vec2::splat(50000.0));
        app.update();
    }
    let at_edge = app.world().resource::<Workshop>().pan;
    app.world_mut()
        .resource_mut::<Workshop>()
        .pan_by(Vec2::splat(-12.0));
    app.update();
    assert!(
        (app.world().resource::<Workshop>().pan - (at_edge - Vec2::splat(12.0))).length() < 0.01
    );
    let size = app.world().resource::<Scene>().sim.size();
    let pixel = project(
        &app,
        entity,
        pixels,
        Vec3::new(size[0] as f32 - 12.0, 34.0, size[2] as f32 - 12.0),
    );
    assert!(pixel.distance(Vec2::new(800.0, 367.0)) < 1.0);
}

#[test]
fn battle_wheel_zooms_close_and_preserves_cursor_focus_at_4k() {
    use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
    let (mut app, entity) = app();
    app.init_resource::<ButtonInput<MouseButton>>()
        .add_message::<MouseWheel>()
        .add_systems(Update, workbench::input.before(framing));
    let pixels = UVec2::new(3840, 2160);
    viewport(&mut app, entity, pixels);
    app.world_mut().entity_mut(entity).insert(GlobalTransform::default());
    let cursor = Vec2::new(2180.0, 1050.0);
    let mut window = Window { resolution: (3840, 2160).into(), focused: true, ..default() };
    window.set_cursor_position(Some(cursor));
    let window_id = app.world_mut().spawn(window).id();
    app.update();
    fn sync(app: &mut App, id: Entity, pixels: UVec2) {
        let t = *app.world().get::<Transform>(id).unwrap();
        let Projection::Orthographic(p) = app.world().get::<Projection>(id).unwrap() else { panic!("projection") };
        let bevy::camera::ScalingMode::FixedVertical { viewport_height: height } = p.scaling_mode else { panic!("height") };
        let half = Vec2::new(height * pixels.x as f32 / pixels.y as f32, height) * 0.5;
        let clip = OrthographicProjection { area: Rect::from_corners(-half, half), ..p.clone() }.get_clip_from_view();
        *app.world_mut().get_mut::<GlobalTransform>(id).unwrap() = GlobalTransform::from(t);
        app.world_mut().get_mut::<Camera>(id).unwrap().computed.clip_from_view = clip;
    }
    sync(&mut app, entity, pixels);
    let camera = app.world().get::<Camera>(entity).unwrap();
    let gt = app.world().get::<GlobalTransform>(entity).unwrap();
    let ray = camera.viewport_to_world(gt, cursor).unwrap();
    let y = app.world().resource::<Scene>().sim.ground_level() as f32;
    let point = ray.origin + *ray.direction * ((y - ray.origin.y) / ray.direction.y);
    let before = camera.world_to_viewport(gt, point).unwrap();
    for amount in [2.0, 24.0] {
        app.world_mut().write_message(MouseWheel { unit: MouseScrollUnit::Line,
            x: 0.0, y: amount, window: window_id, phase: bevy::input::touch::TouchPhase::Moved });
        app.update();
        sync(&mut app, entity, pixels);
        let after = app.world().get::<Camera>(entity).unwrap().world_to_viewport(
            app.world().get::<GlobalTransform>(entity).unwrap(), point).unwrap();
        assert!(before.distance(after) < 20.0, "zoom moved focus {before:?} to {after:?}");
    }
    assert!(app.world().resource::<Workshop>().zoom < 0.02);
    let Projection::Orthographic(p) = app.world().get::<Projection>(entity).unwrap() else { panic!("projection") };
    let bevy::camera::ScalingMode::FixedVertical { viewport_height } = p.scaling_mode else { panic!("height") };
    assert!(viewport_height < 100.0, "close battle view remains too wide: {viewport_height}");
}

#[test]
fn battle_camera_reaches_low_angle_and_stays_outside_terrain_and_units() {
    use modding::range::{RangeBlock, RangeUnitDef, RangeUnitKind};
    let offset = orbit_offset_mode(Vec3::new(-155.0, 420.0, 320.0), 0.0, -1.5, false, true);
    let angle = offset.y.atan2(Vec2::new(offset.x, offset.z).length()).to_degrees();
    assert!((2.9..=3.1).contains(&angle), "minimum angle: {angle}");
    let editor = orbit_offset_mode(Vec3::new(-155.0, 420.0, 320.0), 0.0, -1.5, true, true);
    assert!((editor.y.atan2(Vec2::new(editor.x, editor.z).length()).to_degrees() - 5.0).abs() < 0.1);
    let mut layout = modding::laboratory::Definition::stock().layout;
    layout.battle = true;
    layout.size = [640, 192, 640];
    layout.clear_min = [1, 1]; layout.clear_max = [639, 639];
    layout.gun = [32, 24, 64]; layout.gun_point = true; layout.target = [90, 16, 64];
    layout.blocks = vec![RangeBlock { min: [100, 16, 100], max: [160, 96, 160], material: 1 }];
    layout.units = vec![RangeUnitDef { kind: RangeUnitKind::Tank, allied: true, at: [320, 16, 320], weapon: None }];
    let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
    sim.set_physics_workers(1); sim.set_navigation_workers(1);
    let inside = Vec3::new(130.0, 30.0, 130.0);
    let cleared = battle_camera_clearance(&sim, inside);
    assert!(cleared.y >= sim_core::blast::motion::floor(&sim.world, 130, 130) as f32 + 3.0);
    let free = Vec3::new(130.0, 150.0, 130.0);
    assert_eq!(battle_camera_clearance(&sim, free), free);
    let u = &sim.units[0];
    let (lo, hi) = u.bounds();
    let solid = (lo[1].max(20)..hi[1]).find_map(|y| (lo[0]..hi[0]).find_map(|x|
        (lo[2]..hi[2]).find_map(|z| u.solid([x,y,z]).then_some([x,y,z])))).expect("tank has voxels");
    let eye = Vec3::new(solid[0] as f32 + 0.1, solid[1] as f32 + 0.1, solid[2] as f32 + 0.1);
    let cleared = battle_camera_clearance(&sim, eye);
    assert!(cleared.y >= hi[1] as f32 + 3.0, "camera remains inside the tank: {solid:?}, {cleared:?}");
}

#[test]
fn commander_start_centres_camera_on_allied_spawn_once() {
    use modding::range::{RangeBlock, RangeUnitDef, RangeUnitKind};
    let mut layout = modding::laboratory::Definition::stock().layout;
    layout.battle = true; layout.size = [640, 192, 640];
    layout.clear_min = [1, 1]; layout.clear_max = [639, 639];
    layout.gun = [32, 24, 64]; layout.gun_point = true; layout.target = [90, 16, 64];
    layout.blocks = vec![RangeBlock { min: [0,0,0], max: [640,16,640], material: 1 }];
    layout.units = vec![RangeUnitDef { kind: RangeUnitKind::Commander, allied: true, at: [440,16,400], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Commander, allied: false, at: [120,16,120], weapon: None }];
    let sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    let mut workshop = Workshop::new(Some(layout.clone()), false);
    workshop.initial_commander_focus = Some(layout.units[0].at);
    let mut app = panel::hud::tests::app_with_scene(sim, workshop);
    app.init_resource::<pilot::Pilot>().init_resource::<fire::Effects>()
        .init_resource::<moving::Visual>().init_resource::<streaming::Streaming>()
        .init_resource::<profile::Profile>().init_resource::<Time<Real>>()
        .add_systems(Update, framing.after(panel::actions));
    let entity = app.world_mut().spawn((GasCamera, Camera::default(), Transform::default(),
        Projection::Orthographic(OrthographicProjection::default_3d()))).id();
    let pixels = UVec2::new(3840, 2160);
    viewport(&mut app, entity, pixels);
    app.update();
    let ui = panel::hud::scale(pixels.as_vec2());
    let centre = Vec2::new(pixels.x as f32 * 0.5, (pixels.y as f32 + (72.0 - 238.0) * ui) * 0.5);
    let commander = Vec3::new(440.0, 34.0, 400.0);
    let pixel = project(&app, entity, pixels, commander);
    assert!(pixel.distance(centre) < 2.0, "commander start not centred: {pixel:?} vs {centre:?}");
    assert!(app.world().resource::<Workshop>().initial_commander_focus.is_none());
    let pan = app.world().resource::<Workshop>().pan;
    app.world_mut().resource_mut::<Workshop>().pan_by(Vec2::new(-40.0, 0.0));
    app.update();
    assert_ne!(app.world().resource::<Workshop>().pan, pan, "camera must not keep snapping back");
}
