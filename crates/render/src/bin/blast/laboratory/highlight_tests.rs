use super::super::inspector_tests::{fragment, inspection_app};
use super::*;
fn drawn(app: &mut App) -> (Entity, Handle<Mesh>, Transform, Visibility) {
    let w = app.world_mut();
    let (e, m, t, v) = w
        .query_filtered::<(Entity, &Mesh3d, &Transform, &Visibility), With<Highlight>>()
        .single(w)
        .unwrap();
    (e, m.0.clone(), *t, *v)
}
fn select_fragment(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut lab: Mut<Laboratory>| {
            let scene = world.resource::<Scene>();
            let ray = Ray3d::new(Vec3::new(384.5, 90.0, 320.5), Dir3::NEG_Y);
            let hit = fragments::picking::pick(scene, ray, 100.0).unwrap();
            tools::inspector::select(&mut lab, scene, [384, 80, 320]);
            lab.tools.fragment = Some(hit);
            lab.tools.probe_stamp = None;
            tools::inspector::refresh(&mut lab, scene);
        });
}
#[test]
fn inspector_surface_glow_reuses_one_asset_follows_interpolation_and_hides_after_deletion() {
    let mut app = inspection_app();
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_systems(Update, update.after(actions));
    super::super::integration_tests::enter(&mut app);
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments = vec![fragment(1, 3, [769, 161, 641])];
    select_fragment(&mut app);
    let before = app.world().resource::<Scene>().sim.state_hash();
    app.update();
    let (entity, handle, _, visible) = drawn(&mut app);
    assert_eq!(visible, Visibility::Visible);
    let mesh = app.world().resource::<Assets<Mesh>>().get(&handle).unwrap();
    assert!(
        mesh.indices().unwrap().len() >= 24,
        "glow must have actual surface triangles, not lines"
    );
    assert_eq!(mesh.primitive_topology(), PrimitiveTopology::TriangleList);
    let material = app
        .world()
        .entity(entity)
        .get::<MeshMaterial3d<StandardMaterial>>()
        .unwrap();
    let material = app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&material.0)
        .unwrap();
    assert_eq!(material.alpha_mode, AlphaMode::Blend);
    assert!(material.emissive.red > 1.0);
    let assets = app.world().resource::<Assets<Mesh>>().len();
    for _ in 0..5 {
        app.update();
        assert_eq!(drawn(&mut app).1, handle);
    }
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), assets);
    assert_eq!(
        app.world().resource::<Scene>().sim.state_hash(),
        before,
        "highlight mutated simulation"
    );
    app.world_mut()
        .resource_mut::<moving::Visual>()
        .fragments
        .capture([(1, Vec3::new(384.5, 80.5, 320.5))]);
    {
        let mut s = app.world_mut().resource_mut::<Scene>();
        s.paused = false;
        s.acc = 0.025;
        let f = &mut s.sim.matter.debris.fragments[0];
        f.position.x = f.position.x.add(fx::Fx::from_int(2));
        f.resting = false;
    }
    app.update();
    let (next, next_mesh, pose, _) = drawn(&mut app);
    assert_eq!(next, entity);
    assert_eq!(next_mesh, handle);
    assert!(
        (pose.translation.x - 385.5).abs() < 0.0001,
        "glow detached from interpolated debris"
    );
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments
        .clear();
    app.update();
    assert_eq!(drawn(&mut app).3, Visibility::Hidden);
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), assets);
}
#[test]
fn inspector_surface_glow_clips_real_faces_without_filling_empty_corners_or_cut_planes() {
    let source = render::model::fragment_to_mesh(0x81, 2, [1.0; 4]);
    let pose = Transform::from_xyz(400.0, 80.0, 320.0);
    let empty = clipped(
        &[source],
        pose,
        Vec3::new(400.0, 79.0, 319.0),
        Vec3::new(401.0, 80.0, 320.0),
        false,
    );
    assert!(
        empty.is_empty(),
        "glow invented a cube in a sparse fragment's empty corner"
    );
    let mut triangle = MeshBuf::default();
    triangle.pos = vec![[-2.0, -2.0, 0.5], [3.0, -2.0, 0.5], [0.5, 3.0, 0.5]];
    triangle.nrm = vec![[0.0, 0.0, 1.0]; 3];
    triangle.idx = vec![0, 1, 2];
    let result = clipped(
        &[triangle],
        Transform::IDENTITY,
        Vec3::ZERO,
        Vec3::ONE,
        false,
    );
    assert!(!result.is_empty());
    for p in result.pos {
        assert!((0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]));
        assert!(
            (p[2] - (0.5 + LIFT)).abs() < 1e-6,
            "clip added a closing surface"
        );
    }
}
#[test]
fn inspector_surface_glow_stays_on_one_world_cell_and_reuses_unchanged_geometry_between_ticks() {
    let mut app = inspection_app();
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_systems(Update, update.after(actions));
    super::super::integration_tests::enter(&mut app);
    {
        let mut s = app.world_mut().resource_mut::<Scene>();
        for x in [384, 385] {
            s.sim.world.queue_transfer(voxel::Edit::put(x, 80, 320, 3));
        }
        s.sim.world.flush_transfers();
    }
    app.world_mut()
        .resource_scope(|w, mut lab: Mut<Laboratory>| {
            tools::inspector::select(&mut lab, w.resource::<Scene>(), [384, 80, 320]);
        });
    app.update();
    let (entity, handle, _, _) = drawn(&mut app);
    for p in app
        .world()
        .resource::<Assets<Mesh>>()
        .get(&handle)
        .unwrap()
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .unwrap()
        .as_float3()
        .unwrap()
    {
        assert!(
            p[0] >= 384.0 - LIFT - 0.0001 && p[0] <= 385.0 + LIFT + 0.0001,
            "neighbouring cube was recoloured"
        );
    }
    app.world_mut().resource_mut::<Scene>().sim.tick += 1;
    app.update();
    assert_eq!(drawn(&mut app).1, handle);
    super::super::integration_tests::click(&mut app, LabAction::ClearProbe);
    assert_eq!(drawn(&mut app).3, Visibility::Hidden);
    assert_eq!(drawn(&mut app).0, entity);
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1);
}

#[test]
fn inspector_surface_glow_uses_unit_surface_and_leaves_empty_world_cells_dark() {
    use fx::Fx;
    use sim_core::{
        blast::RangeUnit,
        damage::{Frame, V3},
    };
    let mut app = inspection_app();
    super::super::integration_tests::enter(&mut app);
    let mut u = RangeUnit::of_kind(RangeUnitKind::Tank, [400, 80, 320]);
    let mut body = voxel::UnitBody::new(2, 2, 2, 1);
    body.set(0, 0, 0, voxel::Part::Armour);
    u.body = body;
    u.pose = Frame::new(
        &u.body,
        V3::new(Fx::from_int(400), Fx::from_int(80), Fx::from_int(320)),
        0,
        V3::UP,
    );
    app.world_mut().resource_mut::<Scene>().sim.units = vec![u];
    let scene = app.world().resource::<Scene>();
    let before = scene.sim.state_hash();
    let mut found = false;
    for x in 398..=401 {
        for z in 318..=321 {
            for y in 80..82 {
                let at = [x, y, z];
                if scene.sim.units[0].hit(at).is_some() {
                    let patch = world_surface(scene, at);
                    assert!(!patch.is_empty());
                    found = true;
                    for p in patch.pos {
                        for a in 0..3 {
                            assert!(
                                p[a] >= at[a] as f32 - LIFT - 0.0001
                                    && p[a] <= at[a] as f32 + 1.0 + LIFT + 0.0001
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(found);
    assert!(world_surface(scene, [410, 100, 330]).is_empty());
    assert_eq!(scene.sim.state_hash(), before);
}

#[test]
fn inspector_surface_glow_covers_the_whole_unaligned_voxel_in_its_own_grid() {
    use fx::Fx;
    use sim_core::damage::V3;
    let mut app = inspection_app();
    super::super::integration_tests::enter(&mut app);
    let mut f = fragment(1, 3, [769, 161, 641]);
    f.position.y = f.position.y.add(Fx::from_ratio(1, 3));
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
    let ray = Ray3d::new(Vec3::new(384.5, 90.0, 320.5), Dir3::NEG_Y);
    let hit = fragments::picking::pick(scene, ray, 100.0).unwrap();
    let f = &scene.sim.matter.debris.fragments[0];
    let (lo, hi) = hit.cell_bounds(f).unwrap();
    let source = fragments::picking::surface(scene, f);
    let patch = attached_cell(&source, fragments::picking::pose(f), lo, hi, true);
    assert_eq!(
        patch.idx.len(),
        source.iter().map(|b| b.idx.len()).sum::<usize>(),
        "world-grid clipping left part of the selected voxel unlit"
    );
    assert!((hi - lo - Vec3::ONE).length() < 1e-6);
}
