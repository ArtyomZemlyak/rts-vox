//! Player chassis presentation, rebuilt only when the chosen template changes.
use super::*;
#[derive(Resource)]
pub struct Chassis(pub RangeUnitKind);
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    finishes: &[Handle<StandardMaterial>; look::FINISHES],
    scene: &Scene,
) {
    let body = &scene.sim.gun.body;
    let tint = [0.16, 0.25, 0.34];
    let scale = body.mm as f32 / voxel::unit::WORLD_MM as f32;
    let frames = render::model::segment_frames(body, 1.0 / voxel::unit::WORLD_MM as f32);
    let mut add_body = |b: &voxel::unit::UnitBody,
                        centre: [f32; 3],
                        pos: Vec3,
                        rot: Quat,
                        rows: (usize, usize),
                        turret: bool,
                        segment: Option<usize>| {
        let mut bufs = std::array::from_fn(|_| MeshBuf::default());
        render::model::body_to_mesh(
            b,
            tint,
            b.mm as f32 / voxel::unit::WORLD_MM as f32,
            rows,
            render::model::BEVEL,
            centre,
            &mut bufs,
        );
        for (k, buf) in bufs.iter_mut().enumerate() {
            if buf.idx.is_empty() {
                continue;
            }
            weather(buf, k);
            let base = Transform::from_translation(
                Vec3::new(GUN[0] as f32, GUN[1] as f32, GUN[2] as f32) + pos,
            )
            .with_rotation(rot);
            commands.spawn((
                Mesh3d(meshes.add(mesh(buf))),
                MeshMaterial3d(finishes[k].clone()),
                base,
                Gun {
                    base,
                    turret,
                    segment,
                },
            ));
        }
    };
    add_body(
        body,
        render::model::root_centre(body),
        Vec3::ZERO,
        Quat::IDENTITY,
        (0, body.turret_y as usize),
        false,
        None,
    );
    add_body(
        body,
        render::model::root_centre(body),
        Vec3::ZERO,
        Quat::IDENTITY,
        (body.turret_y as usize, usize::MAX),
        true,
        None,
    );
    for (id, (s, f)) in body.segments.iter().zip(frames).enumerate() {
        add_body(
            &s.body,
            render::model::pivot_centre(&s.attach),
            f.pos,
            f.rot,
            (0, usize::MAX),
            f.turret,
            Some(id),
        );
    }
    println!(
        "gun: {} mm voxels, scale {scale}, {} segments",
        body.mm,
        body.segments.len()
    );
}
pub fn refresh(
    mut commands: Commands,
    scene: Res<Scene>,
    art: Res<WitnessArt>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut chassis: ResMut<Chassis>,
    old: Query<Entity, (With<Gun>, Without<Emitter>)>,
) {
    if chassis.0 == scene.sim.gun.kind {
        return;
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    spawn(&mut commands, &mut meshes, &art.materials, &scene);
    chassis.0 = scene.sim.gun.kind;
}
