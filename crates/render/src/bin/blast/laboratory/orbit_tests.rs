use super::*;

#[test]
fn model_orbit_frames_complete_rotated_part_and_keeps_authored_geometry() {
    let mut lab = Laboratory::new(false);
    let item = lab
        .constructor
        .catalog
        .iter()
        .find(|e| e.id == "bunker")
        .unwrap()
        .clone();
    lab.constructor.placed = vec![modding::laboratory::catalog::Placed {
        element: item,
        at: [384, 16, 400],
        turns: 1,
    }];
    lab.constructor.selected = Some(0);
    let original = lab.constructor.placed.clone();
    frame(&mut lab);
    let view = lab.tools.orbit.as_mut().unwrap();
    assert_eq!(view.pivot, Vec3::new(384.0, 34.0, 400.0));
    let pivot = view.pivot;
    let distance = view.distance;
    let before = view.pose();
    // The full part must fit above the library, including after orbiting.
    for _ in 0..16 {
        let pose = view.pose();
        for x in [360.0, 408.0] {
            for y in [16.0, 52.0] {
                for z in [376.0, 424.0] {
                    let local = pose.rotation.inverse() * (Vec3::new(x, y, z) - pose.translation);
                    let sy = 0.5 - local.y / (-local.z * (FOV.to_radians() * 0.5).tan()) * 0.5;
                    assert!(
                        (0.11..0.75).contains(&sy),
                        "part hidden by editor chrome: {sy}"
                    );
                }
            }
        }
        view.rotate(Vec2::new(65.45, 0.0));
    }
    view.rotate(Vec2::new(160.0, 70.0));
    let after = view.pose();
    assert!(before.translation.distance(after.translation) > 1.0);
    assert!((after.translation.distance(pivot) - distance).abs() < 0.001);
    assert!(after.forward().dot((pivot - after.translation).normalize()) > 0.99999);
    view.pan(Vec2::new(60.0, -40.0), 1080.0);
    let panned = view.pose();
    assert!((panned.translation - after.translation).abs_diff_eq(view.pivot - pivot, 0.001));
    assert!(panned.rotation.abs_diff_eq(after.rotation, 0.0001));
    for _ in 0..100 {
        view.zoom(10.0);
    }
    assert_eq!(view.distance, 1.2);
    for _ in 0..100 {
        view.zoom(-10.0);
    }
    assert_eq!(view.distance, 4096.0);
    assert_eq!(lab.constructor.placed, original);
    frame(&mut lab);
    assert_eq!(lab.tools.orbit.as_ref().unwrap().pivot, pivot);
}

#[test]
fn model_orbit_real_input_focus_and_exit_do_not_rotate_or_edit_sim() {
    use bevy::input::{
        keyboard::KeyboardInput,
        mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel},
    };
    let mut app = inspector_tests::inspection_app();
    app.add_message::<KeyboardInput>()
        .add_message::<MouseWheel>()
        .init_resource::<AccumulatedMouseMotion>()
        .init_resource::<Time<Real>>()
        .add_systems(Update, tools::input.before(actions));
    let mut window = Window::default();
    window.focused = true;
    app.world_mut().spawn(window);
    integration_tests::enter(&mut app);
    {
        let mut lab = app.world_mut().resource_mut::<Laboratory>();
        let item = lab
            .constructor
            .catalog
            .iter()
            .find(|e| e.id == "steel-block")
            .unwrap()
            .clone();
        lab.constructor.placed = vec![modding::laboratory::catalog::Placed {
            element: item,
            at: [384, 16, 336],
            turns: 0,
        }];
        lab.constructor.selected = Some(0);
        lab.tools.camera = Some(Transform::from_xyz(300.0, 60.0, 400.0));
        lab.tools.flying = true;
    }
    let saved = app.world().resource::<Laboratory>().tools.camera;
    integration_tests::click(&mut app, LabAction::BeginVoxelEdit);
    assert!(app.world().resource::<Laboratory>().tools.orbit.is_some());
    assert!(!app.world().resource::<Laboratory>().tools.flying);
    let hash = app.world().resource::<Scene>().sim.state_hash();
    let pivot = app
        .world()
        .resource::<Laboratory>()
        .tools
        .orbit
        .as_ref()
        .unwrap()
        .pivot;
    let pose = app.world().resource::<Laboratory>().tools.camera.unwrap();
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Middle);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .clear();
    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::new(100.0, 25.0);
    app.update();
    let new_pose = app.world().resource::<Laboratory>().tools.camera.unwrap();
    assert!(new_pose.translation.distance(pose.translation) > 1.0);
    assert!(
        new_pose
            .forward()
            .dot((pivot - new_pose.translation).normalize())
            > 0.9999
    );
    assert_eq!(hash, app.world().resource::<Scene>().sim.state_hash());
    {
        let w = app.world_mut();
        w.query::<&mut Window>().single_mut(w).unwrap().focused = false;
    }
    app.update();
    let unfocused = app.world().resource::<Laboratory>().tools.camera.unwrap();
    assert!(new_pose
        .translation
        .abs_diff_eq(unfocused.translation, 0.0001));
    assert!(new_pose.rotation.abs_diff_eq(unfocused.rotation, 0.0001));
    {
        let w = app.world_mut();
        w.query::<&mut Window>().single_mut(w).unwrap().focused = true;
    }
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Middle);
    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::ZERO;
    let distance = app
        .world()
        .resource::<Laboratory>()
        .tools
        .orbit
        .as_ref()
        .unwrap()
        .distance;
    app.world_mut().write_message(MouseWheel {
        unit: MouseScrollUnit::Line,
        x: 0.0,
        y: 1.0,
        window: Entity::PLACEHOLDER,
        phase: bevy::input::touch::TouchPhase::Moved,
    });
    app.update();
    assert!(
        app.world()
            .resource::<Laboratory>()
            .tools
            .orbit
            .as_ref()
            .unwrap()
            .distance
            < distance
    );
    integration_tests::click(&mut app, LabAction::VoxelFinish(true));
    let lab = app.world().resource::<Laboratory>();
    assert!(lab.tools.orbit.is_none() && lab.tools.flying);
    assert_eq!(lab.tools.camera, saved);
}
