use super::*;

// Pipette tests need a quiet laboratory, not the HUD fixture's full AI battle.
pub(super) fn inspection_app() -> App {
    let layout = Definition::stock().layout;
    let sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    let workshop = Workshop::new(Some(layout), false);
    integration_tests::app_with_base(panel::hud::tests::app_with_scene(sim, workshop))
}

#[test]
fn voxel_inspector_picks_adjacent_ten_centimetre_cells_and_reads_live_state() {
    let mut app = inspection_app();
    integration_tests::enter(&mut app);
    integration_tests::click(&mut app, LabAction::Tool(4));
    {
        let mut lab = app.world_mut().resource_mut::<Laboratory>();
        assert_eq!(lab.constructor.mode, constructor::Mode::Inspect);
        assert_eq!(lab.ui.tab, 14);
        assert_eq!(lab.constructor.selected, None);
        lab.definition.editor_step = 16;
        lab.tools.edge = 8;
    }
    let a = [384, 80, 320];
    let b = [385, 80, 320];
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene
            .sim
            .world
            .queue_transfer(voxel::Edit::put(384, 80, 320, 5));
        scene
            .sim
            .world
            .queue_transfer(voxel::Edit::put(385, 80, 320, 10));
        scene.sim.world.flush_transfers();
    }
    for at in [a, b] {
        let scene = app.world().resource::<Scene>();
        let ray = Ray3d::new(Vec3::new(at[0] as f32 + 0.2, 120.0, 320.2), Dir3::NEG_Y);
        assert_eq!(tools::liquid_pick(scene, ray), Some(at));
    }
    app.world_mut()
        .resource_scope(|world, mut lab: Mut<Laboratory>| {
            let scene = world.resource::<Scene>();
            let before = scene.sim.state_hash();
            tools::inspector::select(&mut lab, scene, a);
            assert!(lab.tools.probe.contains("10 × 10 × 10 см"));
            assert!(lab.tools.probe.contains("ID 5"));
            assert!(lab.tools.probe.contains("Объём: 1000 см³"));
            for field in [
                "Плотность:",
                "Прочность:",
                "Порог плавления:",
                "Влажность:",
                "Вода:",
                "Работа деформации:",
            ] {
                assert!(lab.tools.probe.contains(field), "{field}");
            }
            tools::inspector::shift(&mut lab, scene, 0, 1);
            assert_eq!(lab.tools.selected, Some(b));
            assert!(lab.tools.probe.contains("ID 10"));
            tools::inspector::shift(&mut lab, scene, 1, -1);
            assert!(lab.tools.probe.contains("Воздух / пустота"));
            assert_eq!(scene.sim.state_hash(), before, "inspection mutated physics");
            tools::inspector::select(&mut lab, scene, a);
        });
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        let sim = &mut scene.sim;
        sim.heat.add(&sim.world, a.map(|v| v as u32), 120);
        sim.step();
    }
    app.world_mut()
        .resource_scope(|world, mut lab: Mut<Laboratory>| {
            let scene = world.resource::<Scene>();
            tools::inspector::refresh(&mut lab, scene);
            let heat = scene.sim.heat.cell(a.map(|v| v as u32)).heat;
            assert!(heat > 0);
            assert!(lab
                .tools
                .probe
                .contains(&format!("Нагрев: {heat} ед. тепла")));
            tools::inspector::select(&mut lab, scene, [0, 0, 0]);
            tools::inspector::shift(&mut lab, scene, 0, -1);
            assert_eq!(lab.tools.selected, Some([0, 0, 0]));
        });
    integration_tests::click(&mut app, LabAction::ClearProbe);
    assert_eq!(app.world().resource::<Laboratory>().tools.selected, None);
    integration_tests::click(&mut app, LabAction::Tool(0));
    integration_tests::click(&mut app, LabAction::Tab(14));
    assert_eq!(
        app.world().resource::<Laboratory>().constructor.mode,
        constructor::Mode::Inspect
    );
}

#[test]
fn voxel_inspector_focus_keeps_the_minimum_cube_in_view() {
    let mut lab = Laboratory::new(false);
    lab.tools.selected = Some([384, 20, 336]);
    lab.tools.probe_direction = Some(Vec3::NEG_Y);
    tools::inspector::focus(&mut lab);
    let camera = lab.tools.camera.unwrap();
    assert!(lab.tools.flying);
    assert!((camera.translation - Vec3::new(384.5, 20.5, 336.5)).length() - 10.0 < 0.001);
    assert!(camera.forward().dot(Vec3::NEG_Y) > 0.99);
}

pub(super) fn fragment(id: u64, material: u16, position: [i32; 3]) -> sim_core::burst::Fragment {
    use fx::Fx;
    use sim_core::{burst::IDENTITY, damage::V3};
    sim_core::burst::Fragment {
        id,
        position: V3::new(
            Fx::from_ratio(position[0], 2),
            Fx::from_ratio(position[1], 2),
            Fx::from_ratio(position[2], 2),
        ),
        velocity: V3::ZERO,
        material,
        paint: None,
        mm: 100,
        basis: IDENTITY,
        mass_g: 7850,
        count: 1,
        shape: 1,
        side: 1,
        chunk: None,
        held: 0,
        resting: true,
    }
}
#[test]
fn voxel_inspector_fragment_material_is_not_the_grass_below_and_follows_its_owner() {
    let mut app = inspection_app();
    integration_tests::enter(&mut app);
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene
            .sim
            .world
            .queue_transfer(voxel::Edit::put(384, 78, 320, 7));
        scene.sim.world.flush_transfers();
        scene.sim.matter.debris.fragments = vec![
            fragment(1, 7, [769, 159, 641]),
            fragment(2, 3, [769, 161, 641]),
        ];
    }
    app.world_mut()
        .resource_scope(|world, mut lab: Mut<Laboratory>| {
            let scene = world.resource::<Scene>();
            let before = scene.sim.state_hash();
            let ray = Ray3d::new(Vec3::new(384.5, 90.0, 320.5), Dir3::NEG_Y);
            assert_eq!(tools::liquid_pick(scene, ray), Some([384, 78, 320]));
            let hit = fragments::picking::pick(scene, ray, 11.0).unwrap();
            assert_eq!(
                hit.id, 2,
                "pick the nearest actual piece, not source order or terrain"
            );
            tools::inspector::select(&mut lab, scene, [384, 80, 320]);
            lab.tools.fragment = Some(hit);
            lab.tools.probe_stamp = None;
            tools::inspector::refresh(&mut lab, scene);
            assert!(lab.tools.probe.contains("Сталь · ID 3"));
            assert!(lab.tools.probe.contains("Осколок #2"));
            assert!(lab.tools.probe.contains("не отслеживаются"));
            assert!(!lab.tools.probe.contains("ID 7"));
            assert_eq!(scene.sim.state_hash(), before);
        });
    {
        let mut s = app.world_mut().resource_mut::<Scene>();
        s.sim.tick += 1;
        s.sim.matter.debris.fragments[1].position.x = s.sim.matter.debris.fragments[1]
            .position
            .x
            .add(fx::Fx::from_int(2));
    }
    app.world_mut()
        .resource_scope(|world, mut lab: Mut<Laboratory>| {
            let scene = world.resource::<Scene>();
            tools::inspector::refresh(&mut lab, scene);
            assert_eq!(lab.tools.selected.unwrap()[0], 386);
            assert!(lab.tools.probe.contains("Сталь · ID 3"));
        });
    {
        let mut s = app.world_mut().resource_mut::<Scene>();
        s.sim.tick += 1;
        s.sim.matter.debris.fragments.remove(1);
    }
    app.world_mut()
        .resource_scope(|world, mut lab: Mut<Laboratory>| {
            let scene = world.resource::<Scene>();
            tools::inspector::refresh(&mut lab, scene);
            assert!(lab.tools.probe.contains("удалён"));
            assert!(!lab.tools.probe.contains("ID 7"));
            tools::inspector::select(&mut lab, scene, [384, 78, 320]);
            assert!(lab.tools.probe.contains("Грунт с травой · ID 7"));
            assert!(lab.tools.fragment.is_none());
        });
    integration_tests::click(&mut app, LabAction::ClearProbe);
    assert!(app
        .world()
        .resource::<Laboratory>()
        .tools
        .fragment
        .is_none());
}
#[test]
fn voxel_inspector_fragment_pick_respects_real_holes_rotation_scale_and_occlusion() {
    let mut app = inspection_app();
    integration_tests::enter(&mut app);
    let mut f = fragment(8, 7, [800, 160, 640]);
    f.shape = 0x81;
    f.side = 2;
    f.count = 2;
    f.mm = 200;
    use fx::Fx;
    use sim_core::damage::V3;
    f.basis = [
        V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
        V3::UP,
        V3::new(Fx::ONE.neg(), Fx::ZERO, Fx::ZERO),
    ];
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments = vec![f];
    let scene = app.world().resource::<Scene>();
    let rotation = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    let ray = |x, z| {
        Ray3d::new(
            Vec3::new(400.0, 80.0, 320.0) + rotation * Vec3::new(x, 5.0, z) * 2.0,
            Dir3::NEG_Y,
        )
    };
    assert!(
        fragments::picking::pick(scene, ray(0.75, -0.75), f32::INFINITY).is_none(),
        "the empty corner of a sparse mask is not solid"
    );
    assert_eq!(
        fragments::picking::pick(scene, ray(0.5, 0.5), f32::INFINITY)
            .unwrap()
            .id,
        8
    );
    assert!(
        fragments::picking::pick(scene, ray(0.5, 0.5), 1.0).is_none(),
        "do not select behind an opaque foreground object"
    );
}
#[test]
fn voxel_inspector_preserved_steel_section_uses_its_real_mesh() {
    use fx::Fx;
    use sim_core::{
        burst::chunk::{Chunk, Shape},
        damage::{Frame, V3},
    };
    let mut app = inspection_app();
    integration_tests::enter(&mut app);
    let mut body = voxel::UnitBody::new(2, 2, 2, 1);
    body.set(0, 0, 0, voxel::Part::Armour);
    let frame = Frame::new(
        &body,
        V3::new(Fx::from_int(400), Fx::from_int(80), Fx::from_int(320)),
        0,
        V3::UP,
    );
    let shape = Shape::from_body(&body);
    let chunk = Chunk::from_body(0, 0, Arc::new(body), [frame; 2], shape);
    let mut f = fragment(9, 3, [800, 160, 640]);
    f.position = chunk.centre;
    f.chunk = Some(Box::new(chunk));
    let centre = Vec3::from(
        [f.position.x, f.position.y, f.position.z].map(|v| v.raw() as f32 / fx::ONE_RAW as f32),
    );
    let mut scene = app.world_mut().resource_mut::<Scene>();
    scene.sim.units.push(sim_core::blast::RangeUnit::of_kind(
        RangeUnitKind::Tank,
        [100, 16, 100],
    ));
    scene.sim.matter.debris.fragments = vec![f];
    let ray = Ray3d::new(centre + Vec3::Y * 10.0, Dir3::NEG_Y);
    assert_eq!(
        fragments::picking::pick(&scene, ray, f32::INFINITY)
            .unwrap()
            .id,
        9
    );
}

#[test]
fn laboratory_stand_view_toggles_to_the_whole_aquarium_and_back_without_sim_edits() {
    let mut app = inspection_app();
    integration_tests::enter(&mut app);
    let before = app.world().resource::<Scene>().sim.state_hash();
    integration_tests::click(&mut app, LabAction::StandView);
    assert_eq!(app.world().resource::<Scene>().view, 6);
    integration_tests::click(&mut app, LabAction::StandView);
    assert_eq!(app.world().resource::<Scene>().view, 1);
    integration_tests::click(&mut app, LabAction::StandView);
    integration_tests::click(&mut app, LabAction::Frame);
    assert_eq!(app.world().resource::<Scene>().view, 1);
    assert_eq!(app.world().resource::<Scene>().sim.state_hash(), before);
}
