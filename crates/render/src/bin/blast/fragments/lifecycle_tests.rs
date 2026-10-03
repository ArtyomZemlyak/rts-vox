use super::*;

fn app() -> App {
    let sim = BlastRange::new(config(false, false));
    let scene = Scene {
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
    };
    let pristine = Arc::new(std::array::from_fn(|_| voxel::unit::tank()));
    let art = WitnessArt {
        materials: std::array::from_fn(|_| Handle::default()),
        burned_materials: std::array::from_fn(|_| Handle::default()),
        skins: Arc::new(std::array::from_fn(|i| SkinBindings::new(&pristine[i]))),
        pristine,
        seen: Vec::new(),
        pending: Vec::new(),
        emitted: Vec::new(),
        next_mesh: 0,
        generation: 0,
    };
    let mut app = App::new();
    app.insert_resource(scene)
        .insert_resource(art)
        .init_resource::<moving::Visual>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_systems(Update, draw);
    app
}
fn piece(id: u64) -> sim_core::burst::Fragment {
    use fx::Fx;
    use sim_core::damage::V3;
    sim_core::burst::Fragment {
        id,
        position: V3::new(Fx::from_int(id as i32), Fx::ZERO, Fx::ZERO),
        velocity: V3::ZERO,
        material: 3,
        paint: None,
        mm: 100,
        basis: [
            V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
            V3::new(Fx::ZERO, Fx::ONE, Fx::ZERO),
            V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
        ],
        mass_g: 1,
        count: 1,
        shape: 1,
        side: 1,
        chunk: None,
        held: 0,
        resting: true,
    }
}
fn pieces(app: &mut App) -> BTreeMap<u64, Entity> {
    let world = app.world_mut();
    world
        .query::<(Entity, &Piece)>()
        .iter(world)
        .map(|(e, p)| (p.0, e))
        .collect()
}
#[test]
fn native_piece_lifecycle_reuses_sleeps_rebuilds_and_retires_entities() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments = vec![piece(7), piece(9)];
    app.update();
    let first = pieces(&mut app);
    assert_eq!(first.len(), 2);
    app.world_mut().clear_trackers();
    app.update();
    assert_eq!(pieces(&mut app), first);
    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, (With<Piece>, Changed<Transform>)>()
            .iter(world)
            .count(),
        0
    );

    // A physical wake updates its existing entity, even without a tick change.
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments[0]
        .position
        .y = fx::Fx::from_int(4);
    app.update();
    assert_eq!(pieces(&mut app), first);
    assert_eq!(
        app.world()
            .entity(first[&7])
            .get::<Transform>()
            .unwrap()
            .translation
            .y,
        4.0
    );

    // Reset can reuse physical IDs with different geometry/material.
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.generation += 1;
        scene.sim.matter.debris.fragments = vec![piece(7)];
        scene.sim.matter.debris.fragments[0].material = 2;
    }
    app.update();
    let reset = pieces(&mut app);
    assert_eq!(reset.len(), 1);
    assert_ne!(reset[&7], first[&7]);
    assert!(app.world().get_entity(first[&9]).is_err());

    // Recover if another presentation system removed a cached entity.
    app.world_mut().despawn(reset[&7]);
    app.update();
    let restored = pieces(&mut app);
    assert_eq!(restored.len(), 1);
    assert_ne!(restored[&7], reset[&7]);
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments
        .clear();
    app.update();
    assert!(pieces(&mut app).is_empty());
}
