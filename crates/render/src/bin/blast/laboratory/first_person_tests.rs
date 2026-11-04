use super::*;
#[test]
fn first_person_mouse_and_number_keys_fire_all_weapons_through_the_fixed_tick_flush() {
    fn sample(
        mut lab: ResMut<Laboratory>,
        scene: Res<Scene>,
        mouse: Res<ButtonInput<MouseButton>>,
    ) {
        live_tools::release(&mut lab, &mouse, false);
        let pose = lab.tools.camera.unwrap();
        if let Some((at, origin)) =
            live_tools::target(&lab, &scene, Ray3d::new(pose.translation, pose.forward()))
        {
            live_tools::sample(&mut lab, &scene, at, origin, &mouse);
        }
    }
    fn physics(mut lab: ResMut<Laboratory>, mut scene: ResMut<Scene>) {
        live_tools::tick(&mut lab, &mut scene);
        scene.sim.step();
    }
    for (key, tool) in [
        (KeyCode::Digit4, 3),
        (KeyCode::Digit5, 4),
        (KeyCode::Digit6, 5),
        (KeyCode::Digit7, 6),
        (KeyCode::Digit8, 7),
    ] {
        let mut app = app();
        app.add_systems(Update, (sample, flush, physics).chain().after(actions));
        app.world_mut().resource_mut::<Laboratory>().tools.camera =
            Some(Transform::from_xyz(300.5, 60.5, 300.5));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        let lab = app.world().resource::<Laboratory>();
        assert_eq!(lab.tools.live_tool, tool);
        assert!(
            lab.tools.emitting,
            "tool={tool}: the real input path never fired"
        );
        let scene = app.world().resource::<Scene>();
        if tool <= 5 {
            assert!(scene.sim.streams.emitted() > 0);
        } else {
            assert!(scene.sim.shell.is_some() || scene.sim.fire_tick != u32::MAX);
        }
        for _ in 0..12 {
            app.update();
        }
        let emitted = app.world().resource::<Scene>().sim.streams.emitted();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<Scene>().sim.streams.emitted(),
            emitted
        );
        assert!(app
            .world()
            .resource::<Laboratory>()
            .tools
            .live_target
            .is_none());
    }
}
#[test]
fn first_person_model_reuses_assets_and_view_changes_do_not_write_sim() {
    let mut app = app();
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_systems(Update, draw.after(input));
    app.world_mut().resource_mut::<Laboratory>().tools.camera =
        Some(Transform::from_xyz(300.5, 60.5, 300.5));
    let hash = app.world().resource::<Scene>().sim.state_hash();
    for _ in 0..20 {
        app.update();
    }
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1);
    assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 1);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Tab);
    app.update();
    {
        let w = app.world_mut();
        assert_eq!(
            *w.query_filtered::<&Visibility, With<ToolModel>>()
                .single(w)
                .unwrap(),
            Visibility::Hidden
        );
    }
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Tab);
    app.update();
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1);
    assert_eq!(app.world().resource::<Scene>().sim.state_hash(), hash);
}
#[test]
fn first_person_aim_stays_on_camera_ray_and_flush_uses_real_offset_laser_this_tick() {
    let mut app = app();
    let pose = Transform::from_xyz(300.5, 60.5, 300.5);
    let (at, origin) = {
        let w = app.world_mut();
        w.resource_scope(|world, mut lab: Mut<Laboratory>| {
            live_tools::action(
                &mut lab,
                &mut world.resource_mut::<Scene>(),
                LabAction::LiveTool(4),
            );
            lab.tools.camera = Some(pose);
            lab.tools.point = true;
            live_tools::target(
                &lab,
                world.resource::<Scene>(),
                Ray3d::new(pose.translation, pose.forward()),
            )
            .unwrap()
        })
    };
    assert_eq!(at, [300, 60, 268]);
    assert!(origin[0] > 300 && origin[1] < 60 && origin[2] < 300);
    app.add_systems(Update, flush.after(input));
    app.world_mut()
        .resource_mut::<Laboratory>()
        .constructor
        .pending
        .push(LabAction::LiveApply(at, origin));
    app.update();
    app.world_mut()
        .resource_scope(|world, mut lab: Mut<Laboratory>| {
            live_tools::tick(&mut lab, &mut world.resource_mut::<Scene>());
        });
    let mut scene = app.world_mut().resource_mut::<Scene>();
    scene.sim.step();
    let trace = scene
        .sim
        .streams
        .traces
        .first()
        .expect("ordinary laser trace");
    assert_eq!(trace.from.map(fx::Fx::to_int_trunc), origin);
}
fn app() -> App {
    let layout = Definition::stock().layout;
    let sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    let mut app = integration_tests::app_with_base(panel::hud::tests::app_with_scene(
        sim,
        Workshop::new(Some(layout), false),
    ));
    app.add_message::<KeyboardInput>()
        .add_message::<MouseWheel>()
        .init_resource::<AccumulatedMouseMotion>()
        .init_resource::<Time<Real>>()
        .add_systems(Update, input.before(actions));
    let mut window = Window::default();
    window.focused = true;
    app.world_mut().spawn((window, CursorOptions::default()));
    integration_tests::enter(&mut app);
    integration_tests::click(&mut app, LabAction::LiveTools);
    app.update();
    app
}
#[test]
fn first_person_flight_is_frame_rate_independent_horizontal_and_normalized() {
    let mut reference = None;
    for fps in [30, 60, 144, 200] {
        let mut pose = Transform::from_xyz(100.0, 50.0, 100.0).with_rotation(Quat::from_euler(
            EulerRot::YXZ,
            0.0,
            -1.2,
            0.0,
        ));
        let mut h = [false; 14];
        h[0] = true;
        for _ in 0..fps {
            travel(&mut pose, h, 30.0, 1.0 / fps as f32, [640, 192, 640]);
        }
        assert!((pose.translation.y - 50.0).abs() < 0.0001);
        if let Some(p) = reference {
            assert!(pose.translation.distance(p) < 0.002);
        }
        reference = Some(pose.translation);
        let start = Transform::from_xyz(100.0, 50.0, 100.0);
        let mut straight = start;
        travel(&mut straight, h, 30.0, 0.1, [640, 192, 640]);
        h[3] = true;
        let mut diagonal = start;
        travel(&mut diagonal, h, 30.0, 0.1, [640, 192, 640]);
        assert!(
            (straight.translation.distance(start.translation)
                - diagonal.translation.distance(start.translation))
            .abs()
                < 0.0001
        );
        h[8] = true;
        let mut fast = start;
        travel(&mut fast, h, 30.0, 0.1, [640, 192, 640]);
        assert!((fast.translation.distance(start.translation) - 9.0).abs() < 0.0001);
        h[10] = true;
        let mut precise = start;
        travel(&mut precise, h, 30.0, 0.1, [640, 192, 640]);
        assert!((precise.translation.distance(start.translation) - 0.6).abs() < 0.0001);
    }
}
#[test]
fn first_person_capture_tab_escape_and_focus_loss_release_without_stuck_fire() {
    let mut app = app();
    assert!(app.world().resource::<Laboratory>().tools.flight.captured);
    {
        let w = app.world_mut();
        let options = w.query::<&CursorOptions>().single(w).unwrap();
        assert_eq!(options.grab_mode, CursorGrabMode::Locked);
        assert!(!options.visible);
    }
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Tab);
    app.update();
    assert!(!app.world().resource::<Laboratory>().tools.flight.captured);
    assert!(app.world().resource::<Laboratory>().tools.live);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Tab);
    app.update();
    assert!(app.world().resource::<Laboratory>().tools.flight.captured);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Escape);
    app.update();
    assert!(
        app.world().resource::<Laboratory>().tools.live,
        "first Esc releases mouse, keeps session"
    );
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Tab);
    app.update();
    app.world_mut().resource_mut::<Laboratory>().tools.emitting = true;
    {
        let w = app.world_mut();
        w.query::<&mut Window>().single_mut(w).unwrap().focused = false;
    }
    app.update();
    assert!(!app.world().resource::<Laboratory>().tools.flight.captured);
    assert!(!app.world().resource::<Laboratory>().tools.emitting);
    {
        let w = app.world_mut();
        let c = w.query::<&CursorOptions>().single(w).unwrap();
        assert!(c.visible);
        assert_eq!(c.grab_mode, CursorGrabMode::None);
    }
    {
        let w = app.world_mut();
        w.query::<&mut Window>().single_mut(w).unwrap().focused = true;
    }
    app.update();
    assert!(
        !app.world().resource::<Laboratory>().tools.flight.captured,
        "refocus must not steal cursor"
    );
    {
        let w = app.world_mut();
        w.resource_scope(|world, mut lab: Mut<Laboratory>| {
            lab.tools.flying = false;
            live_tools::action(
                &mut lab,
                &mut world.resource_mut::<Scene>(),
                LabAction::LiveCapture,
            );
        });
    }
    app.update();
    let lab = app.world().resource::<Laboratory>();
    assert!(
        lab.tools.flying && lab.tools.flight.captured,
        "FPS button also restores disabled flight"
    );
}
#[test]
fn overview_button_and_home_leave_flight_and_can_return_to_the_same_pose() {
    for keyboard in [false, true] {
        let mut app = app();
        let camera = app
            .world_mut()
            .spawn((
                GasCamera,
                Projection::Perspective(PerspectiveProjection::default()),
            ))
            .id();
        let pose = Transform::from_xyz(300.5, 60.5, 300.5)
            .looking_at(Vec3::new(315.0, 20.0, 325.0), Vec3::Y);
        app.world_mut().resource_mut::<Laboratory>().tools.camera = Some(pose);
        let before = app.world().resource::<Scene>().sim.state_hash();
        assert!(ui::enabled(
            app.world().resource::<Laboratory>(),
            LabAction::Frame
        ));
        if keyboard {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::Home);
            app.update();
        } else {
            integration_tests::click(&mut app, LabAction::Frame);
        }
        app.update();
        let lab = app.world().resource::<Laboratory>();
        assert!(lab.tools.live, "overview must keep the live tools session");
        assert!(!lab.tools.flying && !lab.tools.flight.captured && lab.tools.camera.is_none());
        let saved = lab.tools.flight.overview_return.unwrap();
        assert!(
            saved.translation.distance(pose.translation) < 0.001
                && saved.rotation.dot(pose.rotation).abs() > 0.99999
        );
        assert_eq!(app.world().resource::<Scene>().sim.state_hash(), before);
        assert!(matches!(
            app.world().get::<Projection>(camera),
            Some(Projection::Orthographic(_))
        ));
        {
            let w = app.world_mut();
            let c = w.query::<&CursorOptions>().single(w).unwrap();
            assert_eq!(c.grab_mode, CursorGrabMode::None);
            assert!(c.visible);
        }
        if keyboard {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::Tab);
            app.update();
        } else {
            integration_tests::click(&mut app, LabAction::LiveCapture);
        }
        app.update();
        let lab = app.world().resource::<Laboratory>();
        assert!(lab.tools.flying && lab.tools.flight.captured);
        let restored = lab.tools.camera.unwrap();
        assert!(
            restored.translation.distance(pose.translation) < 0.001
                && restored.rotation.dot(pose.rotation).abs() > 0.99999
        );
    }
}
#[test]
fn first_person_source_is_right_and_below_and_retracts_before_diagonal_wall() {
    let mut app = app();
    let pose = Transform::from_xyz(300.5, 60.5, 300.5);
    let (point, scale) = muzzle(app.world().resource::<Scene>(), pose).unwrap();
    assert_eq!(scale, 1.0);
    assert!(
        point.x > pose.translation.x
            && point.y < pose.translation.y
            && point.z < pose.translation.z
    );
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        for x in 299..307 {
            for y in 55..65 {
                scene
                    .sim
                    .world
                    .queue_transfer(voxel::Edit::put(x, y, 298, 3));
            }
        }
        scene.sim.world.flush_transfers();
    }
    let (point, scale) = muzzle(app.world().resource::<Scene>(), pose).unwrap();
    assert!(scale < 1.0 && scale >= 0.15);
    assert!(point.z >= 299.0);
    let p = point.floor().to_array().map(|v| v as i32);
    assert_eq!(
        sim_core::blast::material(&app.world().resource::<Scene>().sim.world, p),
        0
    );
}
#[test]
fn first_person_capture_click_cannot_fire_until_release_and_rmb_keeps_precision_fire() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Tab);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    app.update();
    let lab = app.world().resource::<Laboratory>();
    assert!(lab.tools.flight.captured);
    assert!(!lab.tools.flight.trigger_ready);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    app.update();
    assert!(
        app.world()
            .resource::<Laboratory>()
            .tools
            .flight
            .trigger_ready
    );
    let mut mouse = ButtonInput::default();
    mouse.press(MouseButton::Left);
    mouse.press(MouseButton::Right);
    let mut lab = app.world_mut().resource_mut::<Laboratory>();
    lab.tools.emitting = true;
    lab.constructor.pending.clear();
    live_tools::release(&mut lab, &mouse, false);
    assert!(lab.constructor.pending.is_empty());
}
