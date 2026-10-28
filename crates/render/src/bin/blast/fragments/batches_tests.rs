use super::*;
use bevy::mesh::VertexAttributeValues;
use fx::Fx;
use sim_core::damage::V3;

// Inspect the actual meshes emitted by draw, including the pending-batch fallback.
// Position, normal and colour together catch resizing, disappearing pieces and
// accidental loss of the ID-selected rubble variant or source paint.
fn picture(app: &mut App) -> (Vec<[i64; 10]>, usize, usize) {
    let world = app.world_mut();
    let mut query = world.query::<(
        &Mesh3d,
        &Transform,
        Option<&Piece>,
        Option<&batches::RestingBatch>,
    )>();
    let meshes = world.resource::<Assets<Mesh>>();
    let mut vertices = Vec::new();
    let (mut entities, mut indices) = (0, 0);
    for (handle, transform, piece, batch) in query.iter(world) {
        if piece.is_none() && batch.is_none() {
            continue;
        }
        entities += 1;
        let mesh = meshes.get(&handle.0).unwrap();
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap();
        let normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .unwrap()
            .as_float3()
            .unwrap();
        let Some(VertexAttributeValues::Float32x4(colours)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("missing source colours")
        };
        for ((p, n), c) in positions.iter().zip(normals).zip(colours) {
            let p = transform.transform_point(Vec3::from(*p));
            let n = transform.rotation * Vec3::from(*n);
            let values = [p.x, p.y, p.z, n.x, n.y, n.z, c[0], c[1], c[2], c[3]];
            vertices.push(values.map(|v| (v * 10000.0).round() as i64));
        }
        indices += mesh.indices().unwrap().len();
    }
    vertices.sort_unstable();
    (vertices, indices, entities)
}
fn fixture() -> Vec<sim_core::burst::Fragment> {
    (0..1024)
        .map(|i| {
            let mut f = piece(i + 1);
            f.position = V3::new(
                Fx::from_int((i % 32) as i32 * 2),
                Fx::from_int(12),
                Fx::from_int((i / 32) as i32 * 2),
            );
            f.material = [2, 3, 4][i as usize % 3];
            if i % 5 == 0 {
                f.paint = Some([0, 65, 170]);
            }
            f.mm = if i % 7 == 0 { 200 } else { 100 };
            if i % 17 == 0 {
                f.shape = u64::MAX;
                f.side = 4;
                f.count = 64;
            } else if i % 3 == 0 {
                f.shape = 3;
                f.side = 2;
                f.count = 2;
            }
            if i % 11 == 0 {
                f.basis = [
                    V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
                    V3::UP,
                    V3::new(Fx::ONE.neg(), Fx::ZERO, Fx::ZERO),
                ];
            }
            f
        })
        .collect()
}
fn compare(control: &mut App, batched: &mut App) -> usize {
    let before = batched.world().resource::<Scene>().sim.matter.debris.hash();
    control.update();
    batched.update();
    assert_eq!(
        batched.world().resource::<Scene>().sim.matter.debris.hash(),
        before,
        "render changed authoritative debris"
    );
    let (a, ai, _) = picture(control);
    let (b, bi, entities) = picture(batched);
    assert_eq!(
        a, b,
        "published and pending geometry must exactly cover the original picture"
    );
    assert_eq!(ai, bi, "triangles were lost or duplicated");
    entities
}
#[test]
fn resting_batches_preserve_every_visible_piece_through_publication_wakeup_edits_and_reclaim() {
    let (mut control, mut optimized) = (app(), app());
    optimized.init_resource::<batches::Enabled>();
    for app in [&mut control, &mut optimized] {
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .matter
            .debris
            .fragments = fixture();
    }
    for _ in 0..8 {
        compare(&mut control, &mut optimized);
    }
    assert!(
        compare(&mut control, &mut optimized) < 40,
        "stationary meshes were not grouped"
    );
    // Reclaim at the front shifts every source slot and crosses 1024 ->1023.
    // It must neither dissolve all batches nor republish unrelated spatial tiles.
    let published = |app: &mut App| -> std::collections::BTreeSet<Entity> {
        let w = app.world_mut();
        w.query_filtered::<Entity, With<batches::RestingBatch>>()
            .iter(w)
            .collect()
    };
    let old = published(&mut optimized);
    for app in [&mut control, &mut optimized] {
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .matter
            .debris
            .fragments
            .remove(0);
    }
    compare(&mut control, &mut optimized);
    let retained = published(&mut optimized).intersection(&old).count();
    assert!(
        retained >= old.len() - 2,
        "one reclaimed piece rebuilt unrelated spatial batches"
    );
    // Wake one piece and move it with interpolation. Every other piece stays
    // present even while its old group is replaced under the build budget.
    for app in [&mut control, &mut optimized] {
        let f = &app.world().resource::<Scene>().sim.matter.debris.fragments[0];
        let previous = (f.id, pos([f.position.x, f.position.y, f.position.z]));
        app.world_mut()
            .resource_mut::<moving::Visual>()
            .fragments
            .capture([previous]);
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.paused = false;
        scene.acc = 0.0125;
        let f = &mut scene.sim.matter.debris.fragments[0];
        f.resting = false;
        f.position.x = f.position.x.add(Fx::ONE);
        f.velocity.x = Fx::ONE;
    }
    for _ in 0..3 {
        compare(&mut control, &mut optimized);
    }
    for app in [&mut control, &mut optimized] {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.paused = true;
        let f = &mut scene.sim.matter.debris.fragments[0];
        f.resting = true;
        f.velocity = V3::ZERO;
        let f = &mut scene.sim.matter.debris.fragments[33];
        f.mm = 50;
        f.shape = 7;
        f.side = 2;
        f.count = 3;
        f.paint = Some([0, 211, 21]);
        f.material = 3;
        // Keep batching above its admission threshold when testing compaction.
        let mut extra = piece(2048);
        extra.position = V3::new(Fx::from_int(3), Fx::from_int(12), Fx::from_int(3));
        scene.sim.matter.debris.fragments.push(extra);
        scene.sim.matter.debris.fragments.remove(99);
        scene.sim.matter.debris.fragments.swap(1, 100);
    }
    for _ in 0..8 {
        compare(&mut control, &mut optimized);
    }
    // Missing an ECS entity must republish it without hiding its source pieces.
    let entity = {
        let w = optimized.world_mut();
        w.query_filtered::<Entity, With<batches::RestingBatch>>()
            .iter(w)
            .next()
            .unwrap()
    };
    optimized.world_mut().despawn(entity);
    compare(&mut control, &mut optimized);
    for app in [&mut control, &mut optimized] {
        let mut s = app.world_mut().resource_mut::<Scene>();
        s.generation += 1;
        s.sim.matter.debris.fragments = fixture();
    }
    for _ in 0..8 {
        compare(&mut control, &mut optimized);
    }
    for app in [&mut control, &mut optimized] {
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .matter
            .debris
            .fragments
            .clear();
    }
    assert_eq!(compare(&mut control, &mut optimized), 0);
}
