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
/// Build the energy collars from authored weapon paths, so a chassis switch
/// places one pair around each actual barrel instead of the tank's old muzzle.
pub fn emitters(
    commands: &mut Commands,
    scene: &Scene,
    cube: &Handle<Mesh>,
    coil: &Handle<StandardMaterial>,
) {
    let body = &scene.sim.gun.body;
    let scale = body.mm as f32 / voxel::unit::WORLD_MM as f32;
    let centre = render::model::root_centre(body);
    let origin = Vec3::from(GUN.map(|v| v as f32));
    let radius = match scene.sim.gun.kind {
        RangeUnitKind::Strider => 2.35,
        RangeUnitKind::Tank => 3.0,
        RangeUnitKind::Spider => 12.0,
    };
    for (barrel, path) in body.weapon_paths().iter().enumerate() {
        if path.axis != 0 {
            continue;
        }
        let muzzle = Vec3::new(
            (f32::from(path.max[0]) + 1.0 - centre[0]) * scale,
            (f32::from(path.min[1]) + f32::from(path.max[1])) * 0.5 * scale,
            ((f32::from(path.min[2]) + f32::from(path.max[2])) * 0.5 - centre[2]) * scale,
        );
        for x in [-8.0, -3.0] {
            for (offset, size) in [
                (Vec3::new(x, radius, 0.0), Vec3::new(1.3, 0.7, radius * 2.2)),
                (
                    Vec3::new(x, -radius, 0.0),
                    Vec3::new(1.3, 0.7, radius * 2.2),
                ),
                (Vec3::new(x, 0.0, radius), Vec3::new(1.3, radius * 2.2, 0.7)),
                (
                    Vec3::new(x, 0.0, -radius),
                    Vec3::new(1.3, radius * 2.2, 0.7),
                ),
            ] {
                let base = Transform::from_translation(origin + muzzle + offset).with_scale(size);
                commands.spawn((
                    Mesh3d(cube.clone()),
                    MeshMaterial3d(coil.clone()),
                    base,
                    Emitter(barrel),
                    Gun {
                        base,
                        turret: true,
                        segment: None,
                    },
                ));
            }
        }
    }
}
pub fn refresh(
    mut commands: Commands,
    scene: Res<Scene>,
    witness: Res<WitnessArt>,
    art: Res<Art>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut chassis: ResMut<Chassis>,
    old: Query<Entity, With<Gun>>,
) {
    if chassis.0 == scene.sim.gun.kind {
        return;
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    spawn(&mut commands, &mut meshes, &witness.materials, &scene);
    emitters(&mut commands, &scene, &art.cube, &art.coil);
    chassis.0 = scene.sim.gun.kind;
}
