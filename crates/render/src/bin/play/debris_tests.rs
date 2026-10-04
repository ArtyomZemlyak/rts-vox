//! Exercise the actual game draw system without a window or simulation ticks.
use super::*;
use fx::Fx;
use sim_core::{burst::Fragment, damage::V3 as PhysV3};

fn app() -> App {
    let tuning = sim_core::Tuning::from_ron(include_str!("../../../../../data/sim.ron")).unwrap();
    let weapons =
        modding::weapons::from_ron(include_str!("../../../../../data/weapons.ron")).unwrap();
    let mut r = Reference::with_params(42, 0, tuning, weapons, worldgen::Params::sized(32, 32, 32));
    r.world.material_bodies = Some(sim_core::damage::Bodies::new(vec![voxel::unit::tank()]));
    let settings =
        render::stream::Stream::from_ron(include_str!("../../../../../data/render.ron")).unwrap();
    let terrain = TerrainScene::new(&r.world.ground().unwrap().world, settings);
    let mut materials = Assets::<StandardMaterial>::default();
    let art = Art {
        hull: Vec::new(),
        turret: Vec::new(),
        segs: Vec::new(),
        legs: Vec::new(),
        shot: Handle::default(),
        blast: Handle::default(),
        beam: Handle::default(),
        finish: std::array::from_fn(|_| materials.add(StandardMaterial::default())),
        wreck_finish: std::array::from_fn(|_| materials.add(StandardMaterial::default())),
        tracer: Handle::default(),
        ray: std::array::from_fn(|_| Handle::default()),
        boom: Handle::default(),
        ground: std::array::from_fn(|_| materials.add(StandardMaterial::default())),
    };
    let mut app = App::new();
    app.insert_resource(Sim {
        r,
        terrain,
        speed: 1,
        was: 1,
        clock: TickClock::default(),
    })
    .insert_resource(View {
        cam: RtsCamera {
            target: V3::new(8.0, 8.0, 8.0),
            yaw: 0.0,
            pitch: 0.9,
            distance: 20.0,
            fovy: 0.87,
            aspect: 1.0,
            near: 1.0,
            far: 100.0,
        },
        side: 32.0,
        body_mm: 100,
    })
    .insert_resource(art)
    .insert_resource(materials)
    .init_resource::<Assets<Mesh>>()
    .init_resource::<DebrisShown>()
    .add_systems(Update, debris);
    app
}
fn piece(id: u64) -> Fragment {
    Fragment {
        id,
        position: PhysV3::new(Fx::from_int(8), Fx::from_int(8), Fx::from_int(8)),
        velocity: PhysV3::ZERO,
        material: 3,
        paint: None,
        mm: 100,
        basis: sim_core::burst::IDENTITY,
        mass_g: 1,
        count: 1,
        shape: 1,
        side: 2,
        chunk: None,
        held: 0,
        resting: true,
    }
}
fn turn(f: &mut Fragment) {
    f.basis = [
        PhysV3::UP,
        PhysV3::new(Fx::ONE.neg(), Fx::ZERO, Fx::ZERO),
        PhysV3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
    ];
}
fn put(app: &mut App, f: Fragment) {
    app.world_mut()
        .resource_mut::<Sim>()
        .r
        .world
        .bursts
        .fragments = vec![f];
}
fn entity(app: &App, id: u64) -> Entity {
    app.world().resource::<DebrisShown>().poses[&id].entity
}
fn mesh(app: &App, id: u64) -> Handle<Mesh> {
    app.world()
        .entity(entity(app, id))
        .get::<Mesh3d>()
        .unwrap()
        .0
        .clone()
}
fn assert_turned(app: &App, id: u64) {
    let tr = app
        .world()
        .entity(entity(app, id))
        .get::<Transform>()
        .unwrap();
    assert!((tr.rotation * Vec3::X - Vec3::Y).length() < 0.0001);
}
fn section(app: &App, id: u64, count: usize) -> Fragment {
    use sim_core::{
        burst::chunk::{Chunk, Shape},
        damage::Frame,
    };
    let sim = app.world().resource::<Sim>();
    let body = &sim.r.world.material_bodies.as_ref().unwrap().templates[0];
    let cells: Vec<_> = (0..body.vol.nz)
        .flat_map(|z| (0..body.vol.ny).flat_map(move |y| (0..body.vol.nx).map(move |x| [x, y, z])))
        .filter(|c| body.vol.get(c[0] as usize, c[1] as usize, c[2] as usize))
        .take(count)
        .collect();
    let mut shape = Shape::default();
    for &c in &cells {
        shape.add(c, body.turret_y);
    }
    let frame = Frame::new(body, PhysV3::ZERO, 0, PhysV3::UP);
    let mut f = piece(id);
    f.count = cells.len() as u32;
    f.chunk = Some(Box::new(Chunk::new(0, 0, cells, [frame; 2], shape)));
    f
}

#[test]
fn debris_new_pieces_start_in_their_physical_rotation() {
    let mut app = app();
    let mut f = piece(7);
    turn(&mut f);
    put(&mut app, f);
    app.update();
    assert_turned(&app, 7);
}
#[test]
fn debris_preserved_sections_turn_with_physics_without_rebuilding_meshes() {
    let mut app = app();
    let f = section(&app, 7, 6);
    put(&mut app, f);
    app.update();
    let first = entity(&app, 7);
    let handles = app.world().resource::<DebrisShown>().chunks[&7].clone();
    turn(
        &mut app
            .world_mut()
            .resource_mut::<Sim>()
            .r
            .world
            .bursts
            .fragments[0],
    );
    app.update();
    assert_eq!(entity(&app, 7), first);
    assert_eq!(app.world().resource::<DebrisShown>().chunks[&7], handles);
    assert_turned(&app, 7);
}
#[test]
fn debris_merge_replaces_mesh_and_material_but_keeps_tick_interpolation() {
    let mut app = app();
    put(&mut app, piece(7));
    app.update();
    let original = mesh(&app, 7);
    {
        let mut sim = app.world_mut().resource_mut::<Sim>();
        sim.r.world.tick += 1;
        sim.clock.debt = DT * 0.25;
        let f = &mut sim.r.world.bursts.fragments[0];
        f.shape = 3;
        f.count = 2;
        f.material = 1;
        f.position.x = Fx::from_int(12);
    }
    app.update();
    let merged = mesh(&app, 7);
    assert_ne!(original, merged, "a merged heap still shows its old shape");
    let meshes = app.world().resource::<Assets<Mesh>>();
    assert_eq!(
        meshes.get(&merged).unwrap().count_vertices(),
        2 * meshes.get(&original).unwrap().count_vertices()
    );
    let e = app.world().entity(entity(&app, 7));
    assert_eq!(
        e.get::<MeshMaterial3d<StandardMaterial>>().unwrap().0,
        app.world().resource::<Art>().ground[look::class(1) as usize]
    );
    assert!((e.get::<Transform>().unwrap().translation.x - 9.0).abs() < 0.0001);
}
#[test]
fn debris_reused_id_can_change_between_pieces_and_sections_and_recover_despawn() {
    let mut app = app();
    put(&mut app, piece(7));
    app.update();
    let first = entity(&app, 7);
    let f = section(&app, 7, 6);
    put(&mut app, f);
    app.update();
    assert!(app.world().get_entity(first).is_err());
    let part = entity(&app, 7);
    assert!(app.world().entity(part).get::<Mesh3d>().is_none());
    let handles = app.world().resource::<DebrisShown>().chunks[&7].clone();
    app.world_mut().despawn(part);
    app.update();
    assert_ne!(entity(&app, 7), part);
    assert!(handles
        .iter()
        .all(|h| app.world().resource::<Assets<Mesh>>().get(h).is_none()));
    let handles = app.world().resource::<DebrisShown>().chunks[&7].clone();
    put(&mut app, piece(7));
    app.update();
    assert!(app.world().resource::<DebrisShown>().chunks.is_empty());
    assert!(handles
        .iter()
        .all(|h| app.world().resource::<Assets<Mesh>>().get(h).is_none()));
    assert!(app
        .world()
        .entity(entity(&app, 7))
        .get::<Mesh3d>()
        .is_some());
    app.world_mut()
        .resource_mut::<Sim>()
        .r
        .world
        .bursts
        .fragments
        .clear();
    app.update();
    assert!(app.world().resource::<DebrisShown>().poses.is_empty());
}
#[test]
fn debris_section_build_quota_defers_new_geometry_without_losing_candidates() {
    let mut app = app();
    let sections = (1..=3).map(|id| section(&app, id, 6)).collect();
    app.world_mut()
        .resource_mut::<Sim>()
        .r
        .world
        .bursts
        .fragments = sections;
    app.update();
    assert_eq!(app.world().resource::<DebrisShown>().poses.len(), 2);
    app.update();
    assert_eq!(app.world().resource::<DebrisShown>().poses.len(), 3);
}

#[test]
fn debris_changed_sections_keep_moving_while_waiting_for_the_build_quota() {
    let mut app = app();
    let sections = (1..=3).map(|id| section(&app, id, 6)).collect();
    app.world_mut()
        .resource_mut::<Sim>()
        .r
        .world
        .bursts
        .fragments = sections;
    app.update();
    app.update();
    let old: Vec<_> = (1..=3).map(|id| entity(&app, id)).collect();
    let mut sections: Vec<_> = (1..=3).map(|id| section(&app, id, 12)).collect();
    for f in &mut sections {
        f.position.x = Fx::from_int(12);
        turn(f);
    }
    {
        let mut sim = app.world_mut().resource_mut::<Sim>();
        sim.r.world.bursts.fragments = sections;
        sim.r.world.tick += 1;
        sim.clock.debt = DT * 0.5;
    }
    app.update();
    assert_ne!(entity(&app, 1), old[0]);
    assert_ne!(entity(&app, 2), old[1]);
    assert_eq!(entity(&app, 3), old[2], "third rebuild exceeded its quota");
    for id in 1..=3 {
        assert_turned(&app, id);
        assert_eq!(
            app.world()
                .entity(entity(&app, id))
                .get::<Transform>()
                .unwrap()
                .translation
                .x,
            10.0
        );
    }
    app.update();
    assert_ne!(entity(&app, 3), old[2]);
    assert_eq!(
        app.world()
            .entity(entity(&app, 3))
            .get::<Transform>()
            .unwrap()
            .translation
            .x,
        10.0
    );
    assert!(old.into_iter().all(|e| app.world().get_entity(e).is_err()));
}

#[test]
fn debris_reused_section_updates_corpse_finish_and_releases_old_meshes() {
    use sim_core::{
        burst::chunk::{Chunk, Shape},
        damage::Frame,
    };
    let mut app = app();
    let f = section(&app, 7, 6);
    put(&mut app, f);
    app.update();
    let old = entity(&app, 7);
    let handles = app.world().resource::<DebrisShown>().chunks[&7].clone();
    let body = app
        .world()
        .resource::<Sim>()
        .r
        .world
        .material_bodies
        .as_ref()
        .unwrap()
        .templates[0]
        .clone();
    let shape = Shape::from_body(&body);
    let frame = Frame::new(&body, PhysV3::ZERO, 0, PhysV3::UP);
    let mut f = piece(7);
    f.chunk = Some(Box::new(Chunk::from_body(0, 0, body, [frame; 2], shape)));
    put(&mut app, f);
    app.update();
    assert!(app.world().get_entity(old).is_err());
    assert!(handles
        .iter()
        .all(|h| app.world().resource::<Assets<Mesh>>().get(h).is_none()));
    let parent = entity(&app, 7);
    let world = app.world_mut();
    let mut count = 0;
    for (child_of, material) in world
        .query::<(&ChildOf, &MeshMaterial3d<StandardMaterial>)>()
        .iter(world)
    {
        if child_of.parent() == parent {
            count += 1;
            assert!(world.resource::<Art>().wreck_finish.contains(&material.0));
            assert!(!world.resource::<Art>().finish.contains(&material.0));
        }
    }
    assert!(count > 0);
}

#[test]
fn debris_unchanged_picture_stays_clean_and_culling_retires_section_assets() {
    let mut app = app();
    let f = section(&app, 7, 6);
    put(&mut app, f);
    app.update();
    let first = entity(&app, 7);
    let handles = app.world().resource::<DebrisShown>().chunks[&7].clone();
    app.world_mut().clear_trackers();
    app.update();
    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, Changed<Transform>>()
            .iter(world)
            .count(),
        0
    );
    app.world_mut().resource_mut::<View>().cam.target.x = 1_000_000.0;
    app.update();
    assert!(app.world().get_entity(first).is_err());
    assert!(handles
        .iter()
        .all(|h| app.world().resource::<Assets<Mesh>>().get(h).is_none()));
    app.world_mut().resource_mut::<View>().cam.target.x = 8.0;
    app.update();
    assert_ne!(entity(&app, 7), first);
}
