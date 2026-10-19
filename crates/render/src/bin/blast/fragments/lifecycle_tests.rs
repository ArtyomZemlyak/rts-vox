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
        geometry: default(),
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
/// Read the actual draw-system mesh and transform, not a second scale formula.
fn assert_drawn_bounds(app: &App, entity: Entity, f: &sim_core::burst::Fragment) {
    let e = app.world().entity(entity);
    let tr = e.get::<Transform>().unwrap();
    let mesh = app
        .world()
        .resource::<Assets<Mesh>>()
        .get(&e.get::<Mesh3d>().unwrap().0)
        .unwrap();
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    let mut local_lo = lo;
    let mut local_hi = hi;
    for p in mesh
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .unwrap()
        .as_float3()
        .unwrap()
    {
        let local = Vec3::from(*p) * tr.scale;
        local_lo = local_lo.min(local);
        local_hi = local_hi.max(local);
        let p = tr.rotation * local;
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let (low, high) = f.bounds();
    assert!((lo - pos(low)).min_element() > -0.0001, "count={}", f.count);
    assert!(
        (pos(high) - hi).min_element() > -0.0001,
        "count={}",
        f.count
    );
    // Bevels cut corners inside the collision envelope. Check the unturned
    // extents exactly too, so enclosure cannot hide an incorrect mesh scale.
    let axis = |x, y, z| {
        sim_core::damage::V3::new(
            fx::Fx::from_int(x),
            fx::Fx::from_int(y),
            fx::Fx::from_int(z),
        )
    };
    let unturned = sim_core::burst::Fragment {
        basis: [axis(1, 0, 0), axis(0, 1, 0), axis(0, 0, 1)],
        chunk: None,
        ..*f
    };
    let (low, high) = unturned.bounds();
    assert!((local_lo - pos(low)).abs().max_element() < 0.0001);
    assert!((local_hi - pos(high)).abs().max_element() < 0.0001);
    assert_eq!(
        tr.translation,
        pos([f.position.x, f.position.y, f.position.z])
    );
}
#[test]
fn metal_panels_keep_the_source_teams_paint_instead_of_the_neutral_owner_tint() {
    use sim_core::{
        burst::chunk::{Chunk, Shape},
        damage::{Frame, V3},
    };
    let mut app = app();
    let template = voxel::unit::tank();
    let mut cell = None;
    'cells: for z in 0..template.vol.nz {
        for y in 0..template.vol.ny {
            for x in 0..template.vol.nx {
                if template.vol.get(x as usize, y as usize, z as usize)
                    && render::look::finish_of(
                        template.finish_at(x as usize, y as usize, z as usize),
                        template.part_at(x as usize, y as usize, z as usize),
                    ) as usize
                        == render::look::Finish::Plate as usize
                {
                    cell = Some([x, y, z]);
                    break 'cells;
                }
            }
        }
    }
    let cell = cell.expect("painted armour fixture");
    let frame = Frame::new(&template, V3::ZERO, 0, V3::UP);
    let mut layout = modding::range::RangeLayout::from_ron(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/range-cover.ron"
    )))
    .unwrap();
    layout.battle = true;
    layout.units.truncate(2);
    layout.units[0].allied = true;
    layout.units[1].allied = false;
    let mut fragments = Vec::new();
    for id in 0..2 {
        let mut shape = Shape::default();
        shape.add(cell, template.turret_y);
        let chunk = Chunk::new(id, 1, vec![cell], [frame; 2], shape);
        let mut f = piece(u64::from(id) + 1);
        f.position = chunk.centre;
        f.paint = Some([1, 0, 0]);
        f.chunk = Some(Box::new(chunk));
        fragments.push(f);
    }
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.sim.layout = Some(layout);
        scene.sim.units = (0..2)
            .map(|_| {
                sim_core::blast::RangeUnit::of_kind(modding::range::RangeUnitKind::Tank, [0, 16, 0])
            })
            .collect();
        scene.sim.matter.debris.fragments = fragments;
    }
    app.update();
    let shown = pieces(&mut app);
    let colors = |app: &mut App, entity: Entity| {
        let world = app.world_mut();
        let mut query = world.query::<(&ChildOf, &Mesh3d)>();
        let assets = world.resource::<Assets<Mesh>>();
        let mut out = Vec::new();
        for (parent, handle) in query.iter(world) {
            if parent.parent() != entity {
                continue;
            }
            if let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) = assets
                .get(&handle.0)
                .unwrap()
                .attribute(Mesh::ATTRIBUTE_COLOR)
            {
                out.extend_from_slice(colors);
            }
        }
        out
    };
    let blue = colors(&mut app, shown[&1]);
    let red = colors(&mut app, shown[&2]);
    assert!(!blue.is_empty());
    assert_eq!(blue.len(), red.len());
    assert_ne!(
        blue, red,
        "a neutral debris container must not repaint both teams orange"
    );
}
#[test]
fn queued_section_replacements_remain_visible_and_follow_physics() {
    use sim_core::{
        burst::chunk::{Chunk, Shape},
        damage::{Frame, V3},
    };
    let mut app = app();
    app.world_mut().resource_mut::<Scene>().sim.units = vec![sim_core::blast::RangeUnit::of_kind(
        modding::range::RangeUnitKind::Tank,
        [0, 16, 0],
    )];
    let template = voxel::unit::tank();
    let cell = [template.core.0, template.core.1, template.core.2];
    let frame = Frame::new(&template, V3::ZERO, 0, V3::UP);
    let fragments = (1..=48)
        .map(|id| {
            let mut shape = Shape::default();
            shape.add(cell, template.turret_y);
            let chunk = Chunk::new(0, 1, vec![cell], [frame; 2], shape);
            let mut f = piece(id);
            f.position = chunk.centre;
            f.paint = Some([1, 0, 0]);
            f.chunk = Some(Box::new(chunk));
            f
        })
        .collect();
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments = fragments;
    app.update();
    let initial_count = pieces(&mut app).len();
    assert!(
        initial_count > 2 && initial_count < 48,
        "small-panel work must be bounded and admit more than two"
    );
    for _ in 0..48 {
        app.update();
        if pieces(&mut app).len() == 48 {
            break;
        }
    }
    let original = pieces(&mut app);
    assert_eq!(original.len(), 48);
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.sim.units[0].status.alive = false;
        for f in &mut scene.sim.matter.debris.fragments {
            f.position.y = f.position.y.add(fx::Fx::ONE);
        }
    }
    for _ in 0..48 {
        app.update();
        let shown = pieces(&mut app);
        assert_eq!(
            shown.len(),
            48,
            "queued replacements must not despawn existing geometry"
        );
        for f in &app.world().resource::<Scene>().sim.matter.debris.fragments {
            let tr = app.world().entity(shown[&f.id]).get::<Transform>().unwrap();
            assert_eq!(
                tr.translation,
                pos([f.position.x, f.position.y, f.position.z])
            );
        }
    }
    let world = app.world_mut();
    assert!(world.query::<&Piece>().iter(world).all(|p| p.2));
}
#[test]
fn saturated_pile_draw_stays_inside_physics_after_inventory_growth_and_wake() {
    let mut app = app();
    let mut f = piece(7);
    f.shape = u64::MAX;
    f.side = 4;
    f.count = 64;
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments = vec![f];
    app.update();
    let first = pieces(&mut app);
    app.world_mut().clear_trackers();
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        let f = &mut scene.sim.matter.debris.fragments[0];
        f.count = 4096;
        f.mass_g = 4096;
    }
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
    assert_drawn_bounds(
        &app,
        first[&7],
        &app.world().resource::<Scene>().sim.matter.debris.fragments[0],
    );

    // The same saturated container can become airborne and rotate again.
    {
        use fx::Fx;
        use sim_core::damage::V3;
        let mut scene = app.world_mut().resource_mut::<Scene>();
        let f = &mut scene.sim.matter.debris.fragments[0];
        f.resting = false;
        f.position.y = Fx::from_int(8);
        f.mm = 200;
        let c = Fx::from_ratio(707107, 1000000);
        f.basis = [
            V3::new(c, Fx::ZERO, c),
            V3::UP,
            V3::new(c.neg(), Fx::ZERO, c),
        ];
    }
    app.update();
    assert_eq!(pieces(&mut app), first);
    let f = &app.world().resource::<Scene>().sim.matter.debris.fragments[0];
    assert_eq!((f.count, f.mass_g), (4096, 4096));
    assert_drawn_bounds(&app, first[&7], f);

    // Physical shape changes still replace the mesh, including sparse pieces.
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments[0]
        .shape = 1 | (1 << 1) | (1 << 4);
    app.update();
    let next = pieces(&mut app);
    assert_ne!(next[&7], first[&7]);
    assert_drawn_bounds(
        &app,
        next[&7],
        &app.world().resource::<Scene>().sim.matter.debris.fragments[0],
    );
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

#[test]
fn dense_parallel_draw_preserves_reordered_and_compacted_pieces() {
    bevy::tasks::ComputeTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(4).build()
    });
    let mut app = app();
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .matter
        .debris
        .fragments = (1..=4097).map(piece).collect();
    app.update();
    let first = pieces(&mut app);
    app.world_mut().clear_trackers();
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        let fragments = &mut scene.sim.matter.debris.fragments;
        fragments.rotate_left(100);
        fragments.retain(|f| f.id % 7 != 0);
        for f in fragments.iter_mut().filter(|f| f.id % 13 == 0) {
            f.position.y = fx::Fx::from_int(9);
        }
    }
    app.update();
    let next = pieces(&mut app);
    for (&id, &entity) in &first {
        if id % 7 == 0 {
            assert!(!next.contains_key(&id));
            assert!(app.world().get_entity(entity).is_err());
        } else {
            assert_eq!(next[&id], entity);
            let tr = app.world().entity(entity).get::<Transform>().unwrap();
            assert_eq!(tr.translation.y, if id % 13 == 0 { 9.0 } else { 0.0 });
        }
    }
    app.world_mut().clear_trackers();
    app.update();
    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, (With<Piece>, Changed<Transform>)>()
            .iter(world)
            .count(),
        0
    );
}
