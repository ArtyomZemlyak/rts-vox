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
