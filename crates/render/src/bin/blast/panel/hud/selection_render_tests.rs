//! GPU readback: state-only UI tests cannot detect retained border pixels.
use super::*;
use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    render::{
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::TextureFormat,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{sync::mpsc, time::Duration};

fn snapshot(app: &mut App, target: &Handle<Image>, name: &str) -> Image {
    // A stale frame must not be confused with an async readback's latency.
    for _ in 0..8 {
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    let (sender, receiver) = mpsc::channel();
    app.world_mut()
        .spawn(Screenshot::image(target.clone()))
        .observe(move |capture: On<ScreenshotCaptured>| {
            sender.send(capture.image.clone()).unwrap();
        });
    for _ in 0..600 {
        app.update();
        if let Ok(image) = receiver.try_recv() {
            let label = std::env::var("SELECTION_GPU_LABEL").unwrap_or_else(|_| "current".into());
            let directory = format!("../../out/selection-rectangle-20261016/{label}");
            std::fs::create_dir_all(&directory).unwrap();
            image
                .clone()
                .try_into_dynamic()
                .unwrap()
                .save(format!("{directory}/{name}.png"))
                .unwrap();
            return image;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("GPU screenshot timeout: {name}");
}

fn cyan_pixels(image: &Image) -> usize {
    (80..300)
        .flat_map(|y| (80..500).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let color = image.get_color_at(x, y).unwrap().to_srgba();
            color.green > 0.5 && color.blue > 0.6 && color.red > 0.2
        })
        .count()
}

#[test]
#[ignore = "Requires a working GPU; sequential render readback qualification."]
fn selection_outline_pixels_disappear_after_release() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>(),
    )
    .insert_resource(Workshop::new(None, false))
    .add_systems(Update, refresh_selection_box);
    while app.plugins_state() != PluginsState::Ready {
        std::thread::sleep(Duration::from_millis(10));
    }
    app.finish();
    app.cleanup();
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            640,
            360,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    app.world_mut().spawn((
        Camera3d::default(),
        RenderTarget::Image(target.clone().into()),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.08, 0.08, 0.08)),
            ..default()
        },
        Msaa::Off,
    ));
    let camera = app
        .world_mut()
        .spawn((
            super::super::ui_camera(),
            RenderTarget::Image(target.clone().into()),
        ))
        .id();
    let mut window = Window::default();
    window.resolution.set(640.0, 360.0);
    window.set_cursor_position(Some(Vec2::new(400.0, 250.0)));
    let window = app.world_mut().spawn(window).id();
    let selection = app
        .world_mut()
        .spawn((
            SelectionBox,
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                border: UiRect::all(px(1)),
                ..default()
            },
            BorderColor::all(Color::srgba(0.55, 0.9, 1.0, 0.9)),
            UiTargetCamera(camera),
        ))
        .id();
    // Keep a normal HUD item present when the selection box disappears.
    app.world_mut().spawn((
        Node {
            width: px(30),
            height: px(30),
            ..default()
        },
        BackgroundColor(Color::srgb(0.6, 0.1, 0.1)),
        UiTargetCamera(camera),
    ));
    for _ in 0..120 {
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    exercise(&mut app, &target, window, selection, "small");
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            3840,
            1949,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    {
        let world = app.world_mut();
        let mut cameras = world.query::<&mut RenderTarget>();
        for mut camera in cameras.iter_mut(world) {
            *camera = RenderTarget::Image(target.clone().into());
        }
        world
            .entity_mut(window)
            .get_mut::<Window>()
            .unwrap()
            .resolution
            .set(3840.0, 1949.0);
    }
    exercise(&mut app, &target, window, selection, "maximized");
}

fn exercise(app: &mut App, target: &Handle<Image>, window: Entity, selection: Entity, label: &str) {
    let idle = snapshot(app, target, &format!("{label}-idle"));
    assert_eq!(cyan_pixels(&idle), 0);
    assert!(idle.get_color_at(10, 10).unwrap().to_srgba().red > 0.4);
    {
        let mut edit = app.world_mut().resource_mut::<Workshop>();
        edit.drag_start = Some(Vec2::new(120.0, 100.0));
        edit.drag_current = Some(Vec2::new(400.0, 250.0));
    }
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    assert!(cyan_pixels(&snapshot(app, target, &format!("{label}-drag"))) > 300);
    app.world_mut().resource_mut::<Workshop>().drag_current = Some(Vec2::new(450.0, 280.0));
    let moved = snapshot(app, target, &format!("{label}-moved"));
    assert!(cyan_pixels(&moved) > 300);
    for y in 130..200 {
        for x in 398..402 {
            assert!(
                moved.get_color_at(x, y).unwrap().to_srgba().green < 0.5,
                "old right edge remains at {x},{y}, {label}"
            );
        }
    }
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    let released = snapshot(app, target, &format!("{label}-released"));
    assert_eq!(
        app.world().get::<Node>(selection).unwrap().display,
        Display::None
    );
    assert_eq!(cyan_pixels(&released), 0);
    assert!(released.get_color_at(10, 10).unwrap().to_srgba().red > 0.4);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    app.world_mut()
        .entity_mut(window)
        .get_mut::<Window>()
        .unwrap()
        .focused = false;
    assert_eq!(
        cyan_pixels(&snapshot(app, target, &format!("{label}-unfocused"))),
        0
    );
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    app.world_mut()
        .entity_mut(window)
        .get_mut::<Window>()
        .unwrap()
        .focused = true;
    assert_eq!(
        cyan_pixels(&snapshot(app, target, &format!("{label}-refocused"))),
        0
    );
}
