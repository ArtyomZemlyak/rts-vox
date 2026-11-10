use super::*;

#[test]
fn crawler_cutting_head_pivots_down_and_spins_in_its_own_plane() {
    let mut frame = ActorFrame::default();
    frame.kind = RangeUnitKind::Crawler.index();
    frame.hull = Quat::IDENTITY;
    let witness = Witness(0, Transform::default(), Some(255), false);
    frame.cutter = (0.0, 0.0);
    let vertical = frame.transform(&witness, &[]);
    assert!((vertical.rotation * Vec3::X - Vec3::X).length() < 0.001);
    frame.cutter = (0.0, 1.0);
    let spinning = frame.transform(&witness, &[]);
    assert!((spinning.rotation * Vec3::X - Vec3::X).length() < 0.001);
    assert_ne!(spinning.rotation, vertical.rotation);
    frame.cutter = (1.0, 1.0);
    let drill = frame.transform(&witness, &[]);
    assert!((drill.rotation * Vec3::X - Vec3::NEG_Y).length() < 0.001);
    let centre = drill.transform_point(Vec3::new(70.0, 26.0, 0.0));
    assert!((centre - Vec3::new(70.0, 26.0, 0.0)).length() < 0.001);
    // The simulation's lowered bore origin follows the animated head.
    // Ignore spin: the tip lies on the head axis.
    let nose = drill.transform_point(Vec3::new(81.5, 26.0, 0.0));
    let bore = Vec3::new(70.5, 14.5, 0.5);
    assert!((nose - bore).length() < 1.0, "animated tip {nose:?}, bore {bore:?}");
    frame.cutter_depth = 24.0;
    frame.cutter_offset = Vec3::new(12.0, 0.0, -8.0);
    let extended = frame.transform(&witness, &[]);
    let lowered = extended.transform_point(Vec3::new(70.0, 26.0, 0.0));
    assert!((lowered - Vec3::new(82.0, 2.0, -8.0)).length() < 0.001);
}

#[test]
fn factory_gate_opens_before_departure_and_closes_afterwards() {
    assert_eq!(gate_open(true, 0.0), 0.0);
    assert_eq!(gate_open(true, 0.25), 1.0);
    assert_eq!(gate_open(true, 5.0), 1.0);
    assert_eq!(gate_open(true, f32::INFINITY), 1.0);
    assert_eq!(gate_open(false, 0.0), 1.0);
    assert_eq!(gate_open(false, 0.25), 0.0);
    assert_eq!(gate_open(false, f32::INFINITY), 0.0);
}
use fx::Fx;
use sim_core::damage::V3;

fn scene() -> Scene {
    let mut sim = tests::occupancy_roster(8);
    sim.battle =
        sim_core::blast::battle::Battle::new(&sim.units, sim.config.unit_ablation_resistance);
    for (i, unit) in sim.units.iter_mut().enumerate() {
        unit.motion.drive.head = 913 * i as i32;
        for (j, foot) in unit.motion.feet.iter_mut().enumerate() {
            foot.contact = Some(sim_core::gait::Contact {
                pos: V3::new(
                    Fx::from_int(160 + i as i32 * 3),
                    Fx::from_int(17),
                    Fx::from_int(293 + j as i32),
                ),
                head: j as i32 * 1117,
                normal: V3::new(Fx::from_ratio(1, 10), Fx::ONE, Fx::ZERO),
            });
        }
    }
    Scene {
        sim,
        generation: 0,
        energy: false,
        anni: false,
        paused: true,
        frame: 0,
        finish: 420,
        out: None,
        all_frames: false,
        wait: 0,
        captured: 0,
        acc: 0.0,
        repeat: false,
        view: 1,
        camera_sweep: false,
        patrol: 1,
        previous_shell: None,
        fps: 60.0,
        step_us: 0,
        flame: 0,
        realtime: true,
        live_elapsed: 0.0,
        last_capture: 0,
        layout_file: None,
        power: 1000,
        uncovered: false,
        unit_override: None,
        skip_delta: true,
        notice: String::new(),
    }
}

// The former serial formulas, evaluated independently of ActorFrame.
fn reference(witness: &Witness, scene: &Scene, visual: &Visual) -> Transform {
    let unit = &scene.sim.units[witness.0];
    if unit.kind == RangeUnitKind::Walker04 && witness.2.is_some_and(|segment| segment >= 10) {
        return segment_picture(&unit.seg_frames[witness.2.expect("arm segment")]);
    }
    let p = visible(unit, visual.previous.get(witness.0), scene);
    let t = fraction(scene);
    let mut local = witness.1;
    if let Some(segment) = witness.2 {
        let hull = render::realtime::hull_rotation(p.up, p.yaw);
        for (j, rig) in visual.rigs[unit.kind.index()].iter().enumerate() {
            let Some(now) = unit
                .motion
                .feet
                .get(j)
                .and_then(|f| f.contact.as_ref())
                .map(foot_of)
            else {
                break;
            };
            let was = visual
                .previous_feet
                .get(witness.0)
                .and_then(|v| v.get(j))
                .copied()
                .flatten();
            let contact = match was {
                Some(w) if t < 1.0 => w.lerp(now, t),
                _ => now,
            };
            let mut pose = rig.solve(hull.inverse() * (contact.pos - p.pos), hull);
            pose.foot.rot = hull.inverse() * contact.rot;
            if let Some(role) = rig.role(segment) {
                local.translation = pose.part(role).pos;
                local.rotation = pose.part(role).rot;
                break;
            }
        }
    }
    let (rotation, offset) = if witness.3 {
        let now = weapon_picture(unit);
        let was = visual.battle_heads.get(witness.0).copied().unwrap_or(now);
        (was.0.slerp(now.0, t), was.1.lerp(now.1, t))
    } else {
        (render::realtime::hull_rotation(p.up, p.yaw), Vec3::ZERO)
    };
    local.translation = p.pos + offset + rotation * local.translation;
    local.rotation = rotation * local.rotation;
    local
}

fn app() -> App {
    let scene = scene();
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin));
    for (i, unit) in scene.sim.units.iter().enumerate() {
        for segment in [None]
            .into_iter()
            .chain((0..unit.body.segments.len()).map(Some))
        {
            for turret in [false, true] {
                let local = Transform::from_xyz(0.5, -0.3, 1.4)
                    .with_rotation(Quat::from_rotation_x(0.17))
                    .with_scale(Vec3::new(1.0, 2.0, 0.5));
                app.world_mut()
                    .spawn((Witness(i, local, segment, turret), Transform::default()));
            }
        }
    }
    app.insert_resource(scene)
        .init_resource::<Visual>()
        .init_resource::<Changes>()
        .add_systems(Update, animate);
    app.add_systems(
        PostUpdate,
        observe.after(bevy::transform::TransformSystems::Propagate),
    );
    app
}

#[derive(Resource, Default)]
struct Changes {
    local: Vec<usize>,
    global: Vec<usize>,
}
fn observe(
    local: Query<&Witness, Changed<Transform>>,
    global: Query<&Witness, Changed<GlobalTransform>>,
    mut changes: ResMut<Changes>,
) {
    changes.local = local.iter().map(|w| w.0).collect();
    changes.global = global.iter().map(|w| w.0).collect();
}

fn assert_reference(app: &mut App) {
    let world = app.world_mut();
    let mut query = world.query::<(&Witness, &Transform)>();
    let scene = world.resource::<Scene>();
    let visual = world.resource::<Visual>();
    assert!(visual.frame_poses.iter().any(|f| f.legs.len() == 8));
    for (witness, actual) in query.iter(world) {
        assert_eq!(
            *actual,
            reference(witness, scene, visual),
            "unit={} segment={:?}",
            witness.0,
            witness.2
        );
    }
}

#[test]
fn actual_parallel_pose_system_matches_serial_picture_without_dirtying_static_meshes() {
    let mut app = app();
    let snapshot = app.world().resource::<Scene>().sim.snapshot();
    app.update();
    assert_reference(&mut app);
    assert!(!app.world().resource::<Changes>().local.is_empty());
    assert!(!app.world().resource::<Changes>().global.is_empty());
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), snapshot);
    app.world_mut().clear_trackers();
    app.update();
    let world = app.world_mut();
    assert!(world.resource::<Changes>().local.is_empty());
    assert!(world.resource::<Changes>().global.is_empty());
    let capacity = world.resource::<Visual>().frame_poses.capacity();
    world.resource_mut::<Scene>().sim.units[2].motion.pos[0] = Fx::from_int(192);
    app.update();
    assert_reference(&mut app);
    assert_eq!(
        app.world().resource::<Visual>().frame_poses.capacity(),
        capacity
    );
    let world = app.world_mut();
    assert!(world
        .resource::<Changes>()
        .local
        .iter()
        .all(|&unit| unit == 2));
    assert!(!world.resource::<Changes>().local.is_empty());
    assert!(world
        .resource::<Changes>()
        .global
        .iter()
        .all(|&unit| unit == 2));
    assert!(!world.resource::<Changes>().global.is_empty());
}

#[test]
fn interpolated_turrets_feet_death_and_reset_keep_the_same_picture() {
    let mut app = app();
    app.update();
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../../data/range-cover.ron")).unwrap();
        layout.battle = true;
        scene.sim.layout = Some(layout);
        scene.sim.battle = sim_core::blast::battle::Battle::new(
            &scene.sim.units,
            scene.sim.config.unit_ablation_resistance,
        );
    }
    for alpha in [0.0, 0.25, 0.9, 1.0] {
        {
            let world = app.world_mut();
            let previous = world
                .resource::<Scene>()
                .sim
                .units
                .iter()
                .map(pose)
                .collect();
            let previous_feet = world
                .resource::<Scene>()
                .sim
                .units
                .iter()
                .map(feet_of)
                .collect();
            let mut visual = world.resource_mut::<Visual>();
            visual.previous = previous;
            visual.previous_feet = previous_feet;
            visual.battle_turrets = vec![65000; 8];
            let mut scene = world.resource_mut::<Scene>();
            scene.paused = false;
            scene.acc = alpha / sim_core::TICKS_PER_SECOND as f32;
            for actor in &mut scene.sim.battle.actors {
                actor.turret.angle = 300;
            }
            for unit in &mut scene.sim.units {
                unit.motion.pos[0] = unit.motion.pos[0].add(Fx::ONE);
                unit.status.alive = false;
                for foot in &mut unit.motion.feet {
                    if let Some(c) = &mut foot.contact {
                        c.pos.y = c.pos.y.add(Fx::ONE);
                    }
                }
            }
        }
        app.update();
        assert_reference(&mut app);
    }
    reset(&mut app.world_mut().resource_mut::<Visual>());
    app.update();
    assert_reference(&mut app);
}

#[test]
fn elevated_spider_picture_matches_physical_mount_and_occupancy_refreshes() {
    let mut app = app();
    let id = app
        .world()
        .resource::<Scene>()
        .sim
        .units
        .iter()
        .position(|u| u.kind == modding::range::RangeUnitKind::Spider)
        .unwrap();
    let yaw = fx::angle::FULL / 7;
    let pitch = -fx::angle::FULL / 24;
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.sim.battle.actors[id].turret = sim_core::turret::Traverse::new(yaw);
        scene.sim.battle.actors[id].elevation = sim_core::turret::Traverse::new(pitch);
        let u = &mut scene.sim.units[id];
        u.motion.hull.up = V3::new(Fx::from_ratio(1, 10), Fx::ONE, Fx::from_ratio(-1, 5));
        u.turret_pitch = pitch;
        u.update_pose(yaw);
    }
    app.update();
    let scene = app.world().resource::<Scene>();
    let u = &scene.sim.units[id];
    let physical = u.root_frame(usize::from(u.body.turret_y));
    let visual = &app.world().resource::<Visual>().frame_poses[id];
    let forward = visual.turret * Vec3::X;
    let expected = Vec3::new(
        physical.x.x.to_f64_lossy() as f32,
        physical.x.y.to_f64_lossy() as f32,
        physical.x.z.to_f64_lossy() as f32,
    );
    assert!((forward - expected).length() < 0.0001);
    let local = [
        Fx::from_ratio(u.body.vol.nx as i32, 2),
        Fx::from_int(u.body.turret_y as i32),
        Fx::from_ratio(u.body.vol.nz as i32, 2),
    ];
    let physical_mount = physical.world(local);
    let mount = Vec3::new(
        0.0,
        u.body.turret_y as f32 * u.body.mm as f32 / voxel::unit::WORLD_MM as f32,
        0.0,
    );
    let rendered_mount = visual.pos + visual.turret_offset + visual.turret * mount;
    let expected_mount = Vec3::new(
        physical_mount.x.to_f64_lossy() as f32,
        physical_mount.y.to_f64_lossy() as f32,
        physical_mount.z.to_f64_lossy() as f32,
    );
    assert!((rendered_mount - expected_mount).length() < 0.001);
    let window = TerrainWindow {
        origin: [0; 3],
        dims: [512, 128, 512],
    };
    let index = WindowIndex::new(window);
    let mut cached_unit =
        sim_core::blast::RangeUnit::of_kind(modding::range::RangeUnitKind::Spider, u.at);
    cached_unit.update_pose(yaw);
    cached_unit.turret_pitch = pitch;
    let mut old = new_occupied(&cached_unit, &index, false);
    cached_unit.turret_pitch = 0;
    assert!(update_occupied(
        &cached_unit,
        &mut old,
        &index,
        Some(window),
        false
    ));
    assert_eq!(old.turret_pitch, 0);
}

#[test]
fn every_actor_weapon_picture_follows_the_physical_elevated_frame() {
    let mut app = app();
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        for (i, u) in scene.sim.units.iter_mut().enumerate() {
            u.motion.hull.up = V3::new(Fx::from_ratio(1, 10), Fx::ONE, Fx::from_ratio(-1, 5));
            u.turret_pitch = -fx::angle::FULL / 24;
            u.update_pose(fx::angle::FULL / 7 + i as i32 * 173);
        }
    }
    app.update();
    let scene = app.world().resource::<Scene>();
    let visual = app.world().resource::<Visual>();
    for (i, u) in scene.sim.units.iter().enumerate() {
        let f = u.root_frame(usize::from(u.body.turret_y));
        let expected = Vec3::new(
            f.x.x.to_f64_lossy() as f32,
            f.x.y.to_f64_lossy() as f32,
            f.x.z.to_f64_lossy() as f32,
        );
        assert!(
            (visual.frame_poses[i].turret * Vec3::X - expected).length() < 0.0001,
            "{:?}",
            u.kind
        );
        let local = [
            Fx::from_ratio(u.body.vol.nx as i32, 2),
            Fx::from_int(u.body.turret_y as i32),
            Fx::from_ratio(u.body.vol.nz as i32, 2),
        ];
        let mount = f.world(local);
        let expected = Vec3::new(
            mount.x.to_f64_lossy() as f32,
            mount.y.to_f64_lossy() as f32,
            mount.z.to_f64_lossy() as f32,
        );
        let rendered = visual.frame_poses[i].pos
            + visual.frame_poses[i].turret_offset
            + visual.frame_poses[i].turret
                * Vec3::Y
                * (u.body.turret_y as f32 * u.body.mm as f32 / voxel::unit::WORLD_MM as f32);
        assert!((rendered - expected).length() < 0.001, "{:?}", u.kind);
    }
}

#[test]
fn player_weapon_picture_and_muzzle_follow_the_same_tilted_frame() {
    let mut scene = scene();
    scene.sim.gun.motion.hull.up = V3::new(Fx::from_ratio(1, 10), Fx::ONE, Fx::from_ratio(-1, 5));
    scene.sim.gun.turret_pitch = -fx::angle::FULL / 24;
    scene.sim.turret.angle = fx::angle::FULL / 7;
    scene.sim.gun.update_pose(scene.sim.turret.angle);
    let visual = Visual::default();
    let (rotation, offset) = visible_gun_frame(&scene, &visual);
    let physical = scene
        .sim
        .gun
        .root_frame(usize::from(scene.sim.gun.body.turret_y));
    let forward = Vec3::new(
        physical.x.x.to_f64_lossy() as f32,
        physical.x.y.to_f64_lossy() as f32,
        physical.x.z.to_f64_lossy() as f32,
    );
    assert!((rotation * Vec3::X - forward).length() < 0.0001);
    let path = scene.sim.gun.body.weapon_paths()[0];
    let local = Vec3::new(
        path.max[0] as f32 + 1.0,
        (path.min[1] as f32 + path.max[1] as f32) * 0.5,
        (path.min[2] as f32 + path.max[2] as f32) * 0.5,
    );
    let centre = Vec3::new(
        scene.sim.gun.body.vol.nx as f32 * 0.5,
        0.0,
        scene.sim.gun.body.vol.nz as f32 * 0.5,
    );
    let scale = scene.sim.gun.body.mm as f32 / voxel::unit::WORLD_MM as f32;
    let rendered = pose(&scene.sim.gun).pos + offset + rotation * ((local - centre) * scale);
    assert!((rendered - super::super::visible_muzzle(&scene, &visual)).length() < 0.001);
}

#[test]
fn walker_shoulders_elbows_and_guns_stay_joined_between_movement_and_focus_ticks() {
    let mut app = app();
    let mut unit = sim_core::blast::RangeUnit::of_kind(RangeUnitKind::Walker04, [300, 16, 300]);
    unit.set_arm_targets([[1200, 80, 300]; 4], 0);
    let previous = pose(&unit);
    let previous_head = weapon_picture(&unit);
    let previous_arms = std::array::from_fn(|i| segment_picture(&unit.seg_frames[if i < 16 { 10 + i } else { 26 + i - 16 }]));
    unit.motion.pos[0] = unit.motion.pos[0].add(Fx::from_int(30));
    unit.motion.drive.head = fx::angle::QUARTER / 2;
    unit.turret_pitch = fx::angle::QUARTER;
    unit.set_arm_targets([[900, 180, 800]; 4], fx::angle::QUARTER);
    app.world_mut().resource_mut::<Scene>().sim.units[0] = unit;
    let mut entities = Vec::new();
    for segment in 10..30 {
        entities.push(
            app.world_mut()
                .spawn((
                    Witness(0, Transform::default(), Some(segment), true),
                    Transform::default(),
                ))
                .id(),
        );
    }
    {
        let mut v = app.world_mut().resource_mut::<Visual>();
        v.previous = vec![previous];
        v.battle_heads = vec![previous_head];
        v.previous_arms = vec![Some(previous_arms)];
    }
    for alpha in [0.0, 0.25, 0.5, 0.9, 1.0] {
        {
            let mut s = app.world_mut().resource_mut::<Scene>();
            s.paused = false;
            s.acc = alpha / sim_core::TICKS_PER_SECOND as f32;
        }
        app.update();
        let w = app.world();
        let s = w.resource::<Scene>();
        let u = &s.sim.units[0];
        let p = visible(u, Some(&previous), s);
        let now = weapon_picture(u);
        let rotation = previous_head.0.slerp(now.0, alpha);
        let base = p.pos + previous_head.1.lerp(now.1, alpha);
        let centre = Vec3::new(u.body.vol.nx as f32 * 0.5, 0.0, u.body.vol.nz as f32 * 0.5);
        let scale = u.body.mm as f32 / voxel::unit::WORLD_MM as f32;
        for segment in 10..30 {
            let a = u.body.segments[segment].attach;
            let tr = w.get::<Transform>(entities[segment - 10]).unwrap();
            let at = Vec3::new(a.at.0 as f32, a.at.1 as f32, a.at.2 as f32);
            let expected = if segment >= 26 {
                base + rotation * ((at - centre) * scale)
            } else if a.parent == 0 {
                let arm = (segment - 10) / 4;
                let bearing = w.get::<Transform>(entities[26 + arm - 10]).unwrap();
                let offset = (at - Vec3::new(40.0, at.y, 40.0)) * scale;
                bearing.translation + bearing.rotation * offset
            } else {
                let parent = &u.body.segments[a.parent as usize - 1];
                let pivot = Vec3::new(parent.attach.pivot.0 as f32, parent.attach.pivot.1 as f32, parent.attach.pivot.2 as f32);
                let parent_scale = parent.body.mm as f32 / voxel::unit::WORLD_MM as f32;
                w.get::<Transform>(entities[a.parent as usize - 1 - 10])
                    .unwrap()
                    .transform_point((at - pivot) * parent_scale)
            };
            assert!(
                (tr.translation - expected).length() < 0.02,
                "separated joint segment={segment} alpha={alpha}: {:?} vs {expected:?}",
                tr.translation
            );
        }
    }
}

#[test]
fn walker_arm_picture_matches_the_physical_muzzle_after_independent_aiming() {
    let mut unit = sim_core::blast::RangeUnit::of_kind(
        modding::range::RangeUnitKind::Walker04,
        [100, 16, 100],
    );
    unit.set_arm_targets([[1200, 200, 300]; 4], 0);
    for arm in 0..4 {
        let frame = unit.seg_frames[13 + arm * 4];
        let physical = frame.world([Fx::from_int(4), Fx::from_int(2), Fx::from_int(2)]);
        let expected = Vec3::new(
            physical.x.to_f64_lossy() as f32,
            physical.y.to_f64_lossy() as f32,
            physical.z.to_f64_lossy() as f32,
        );
        let local = (Vec3::new(4.0, 2.0, 2.0) - Vec3::from(frame.centre.map(|v| v.to_f64_lossy() as f32)))
            / frame.scale.to_f64_lossy() as f32;
        let rendered = segment_picture(&frame).transform_point(local);
        assert_eq!(segment_picture(&frame).scale, Vec3::ONE, "mesh is already scaled during construction");
        assert!((rendered - expected).length() < 0.001, "arm={arm}");
    }
}

#[test]
fn commander_arm_picture_keeps_joints_connected_through_tool_changes() {
    let mut u = sim_core::blast::RangeUnit::of_kind(RangeUnitKind::Commander, [300,16,300]);
    let old_crown = weapon_picture(&u).0;
    let old: [Transform;20] = std::array::from_fn(|i| segment_picture(&u.seg_frames[10 + i.min(15)]));
    for _ in 0..100 { u.set_arm_targets([[800,180,380],[800,180,380],[1,0,0],[0;3]],0); }
    let now = weapon_picture(&u);
    let origin = pose(&u).pos + now.1;
    for alpha in [0.0,0.25,0.5,0.9,1.0] {
        let drawn: Vec<_> = (10..18).map(|index| commander_arm_picture(&u,index,now.0,origin,Some(&old),old_crown,alpha)).collect();
        for arm in 0..2 {
            let first = 10 + arm*4;
            assert!((drawn[first-10].translation - segment_picture(&u.seg_frames[first]).translation).length() < 0.01);
            for child in first+1..first+4 {
                let attach = u.body.segments[child].attach;
                let parent = &u.body.segments[usize::from(attach.parent)-1];
                let at = Vec3::new(attach.at.0 as f32,attach.at.1 as f32,attach.at.2 as f32);
                let pivot = Vec3::new(parent.attach.pivot.0 as f32,parent.attach.pivot.1 as f32,parent.attach.pivot.2 as f32);
                let expected = drawn[child-11].transform_point((at-pivot)*(parent.body.mm as f32 / voxel::unit::WORLD_MM as f32));
                assert!((drawn[child-10].translation-expected).length() < 0.02,"separated joint {child}, alpha {alpha}");
            }
        }
        if alpha == 1.0 {
            for (index,tr) in drawn.iter().enumerate() {
                let physical = segment_picture(&u.seg_frames[index+10]);
                assert!((tr.translation-physical.translation).length() < 0.02);
                assert!(tr.rotation.dot(physical.rotation).abs()>0.9999);
            }
        }
    }
}

#[test]
fn commander_arm_picture_stays_attached_while_crown_turns_and_hull_moves() {
    let mut u = sim_core::blast::RangeUnit::of_kind(RangeUnitKind::Commander,[300,16,300]);
    let old_crown = weapon_picture(&u);
    let old_origin = pose(&u).pos + old_crown.1;
    let old: [Transform;20] = std::array::from_fn(|i|segment_picture(&u.seg_frames[10+i.min(7)]));
    u.motion.pos[0]=Fx::from_int(340);
    u.motion.drive.head=fx::angle::QUARTER;
    u.turret_pitch=fx::angle::FULL/36;
    let yaw=fx::angle::QUARTER+fx::angle::FULL/24;
    u.update_pose(yaw);
    for _ in 0..60 {u.set_arm_targets([[700,180,700],[700,180,700],[1,0,0],[0;3]],yaw);}
    let now=weapon_picture(&u);
    let now_origin=pose(&u).pos+now.1;
    for alpha in [0.0,0.25,0.5,0.9,1.0] {
        let crown=old_crown.0.slerp(now.0,alpha);
        let origin=old_origin.lerp(now_origin,alpha);
        for arm in 0..2 {
            let first=10+arm*4;
            let frames: Vec<_>=(first..first+4).map(|i|commander_arm_picture(&u,i,crown,origin,Some(&old),old_crown.0,alpha)).collect();
            let a=u.body.segments[first].attach;
            let at=Vec3::new(a.at.0 as f32,a.at.1 as f32,a.at.2 as f32);
            let centre=Vec3::from(u.pose.centre.map(|v|v.to_f64_lossy() as f32));
            let expected=origin+crown*((at-centre)/u.pose.scale.to_f64_lossy() as f32);
            assert!((frames[0].translation-expected).length()<0.02,"detached shoulder {arm}, alpha {alpha}");
            for child in 1..4 {
                let a=u.body.segments[first+child].attach;
                let parent=&u.body.segments[usize::from(a.parent)-1];
                let at=Vec3::new(a.at.0 as f32,a.at.1 as f32,a.at.2 as f32);
                let pivot=Vec3::new(parent.attach.pivot.0 as f32,parent.attach.pivot.1 as f32,parent.attach.pivot.2 as f32);
                let expected=frames[child-1].transform_point((at-pivot)*(parent.body.mm as f32/voxel::unit::WORLD_MM as f32));
                assert!((frames[child].translation-expected).length()<0.02,"detached child {child}, alpha {alpha}");
            }
            if alpha==1.0 {
                for (i,tr) in frames.iter().enumerate() {
                    let expected=segment_picture(&u.seg_frames[first+i]);
                    assert!((tr.translation-expected.translation).length()<0.02);
                    assert!(tr.rotation.dot(expected.rotation).abs()>0.9999);
                }
            }
        }
    }
}

#[test]
fn crawler_picture_tracks_real_bore_front_without_frame_rate_lag() {
    use sim_core::blast::battle::Order;
    let experiment = modding::laboratory::experiments::stock().into_iter()
        .find(|e| e.id == "crawler-base").unwrap();
    let mut layout = experiment.layout;
    layout.size = [256, 128, 256]; layout.clear_min = [1, 1]; layout.clear_max = [255, 255];
    layout.gun = [96, 24, 128]; layout.target = [190, 8, 128];
    layout.units[0].at = [155, 16, 128]; layout.units[1].at = [48, 16, 128];
    layout.blocks[0].min = [180, 1, 120]; layout.blocks[0].max = [202, 16, 136];
    layout.blocks[0].material = 6;
    let crawler = layout.economy.as_mut().unwrap().kind_mut(RangeUnitKind::Crawler).unwrap();
    crawler.tank = 9_000_000; crawler.dig_edits_per_tick = 1024;
    let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
    sim.set_physics_workers(1); sim.set_navigation_workers(1);
    sim.start_free_control().unwrap();
    let tip = sim.units[0].world_pos(voxel::unit::CRAWLER_BORE_TIP);
    assert!(sim.battle_order(0, Order::Dig([tip[0] - 12, 8, tip[2] + 5]), None));
    let start = sim.crawler_bore_depth(0).unwrap();
    for _ in 0..150 {
        sim.step();
        if sim.crawler_bore_depth(0).unwrap_or(start) >= start + 4 { break; }
    }
    assert!(sim.crawler_bore_cutting(0));
    let expected = sim.crawler_bore_depth(0).unwrap() as f32;
    assert!(expected >= start as f32 + 4.0);
    let mut view = scene(); view.sim = sim; view.paused = true; view.realtime = true;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin))
        .insert_resource(view).init_resource::<Visual>().add_systems(Update, animate);
    let id = app.world_mut().spawn((Witness(0, Transform::default(), Some(255), false), Transform::default())).id();
    app.update();
    let visual = app.world().resource::<Visual>();
    let frame = &visual.frame_poses[0];
    assert!((frame.cutter_depth - expected).abs() < 0.001, "head lags behind real terrain: {} vs {expected}", frame.cutter_depth);
    assert_eq!(frame.cutter.0, 1.0, "cutting started before head was vertical");
    let drawn_tip = app.world().get::<Transform>(id).unwrap().transform_point(Vec3::new(81.5, 26.0, 0.0));
    let position = app.world().resource::<Scene>().sim.crawler_bore_position(0).unwrap();
    assert!((drawn_tip - Vec3::from(position.map(|v| v as f32))).length() < 3.0,
        "drawn tip {drawn_tip:?} is not at the physical face {position:?}");
}

#[test]
fn crawler_head_deployment_catches_up_after_skipped_frames() {
    use sim_core::blast::battle::Order;
    let experiment = modding::laboratory::experiments::stock().into_iter()
        .find(|e| e.id == "crawler-base").unwrap();
    let mut layout = experiment.layout;
    layout.size = [256, 128, 256]; layout.clear_min = [1, 1]; layout.clear_max = [255, 255];
    layout.gun = [96, 24, 128]; layout.target = [190, 8, 128];
    layout.units[0].at = [155, 16, 128]; layout.units[1].at = [48, 16, 128];
    layout.blocks[0].min = [180, 1, 120]; layout.blocks[0].max = [202, 16, 136];
    let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
    sim.set_physics_workers(1); sim.set_navigation_workers(1);
    sim.start_free_control().unwrap();
    let tip = sim.units[0].world_pos(voxel::unit::CRAWLER_BORE_TIP);
    assert!(sim.battle_order(0, Order::Dig([tip[0], 8, tip[2]]), None));
    sim.step();
    let mut view = scene(); view.sim = sim; view.paused = false; view.realtime = true; view.acc = 0.0;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin))
        .insert_resource(view).init_resource::<Visual>().add_systems(Update, animate);
    app.update();
    for _ in 0..10 { app.world_mut().resource_mut::<Scene>().sim.step(); }
    assert!(!app.world().resource::<Scene>().sim.crawler_bore_cutting(0));
    app.update();
    let frame = &app.world().resource::<Visual>().frame_poses[0];
    assert!(frame.cutter.0 >= 0.70, "head failed to catch up after a frame gap: {}", frame.cutter.0);
}
