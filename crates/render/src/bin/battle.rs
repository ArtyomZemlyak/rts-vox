//! Окно боя: симуляция крутится, рендер её показывает.
//!
//! ```text
//! battle --units 2000 --shot кадр.png --shot-after 8
//! ```
//!
//! Управление то же, что у `view`: **WASD** — панорама, **Q/E** —
//! поворот, **R/F** — приближение, **Esc** — выход.
//!
//! **Рендер читает состояние и никогда не пишет в него** (`CLAUDE.md`,
//! границы слоёв). Отсюда и устройство: симуляция тикает сама по себе,
//! а окно каждый кадр берёт у неё положения юнитов, снаряды и события
//! прошедшего тика.
//!
//! Рельеф пересобирается **только там, где копали**: `MeshLayer` знает
//! грязные чанки, и полная пересборка мира каждый кадр стоила бы
//! на два порядка больше (ADR-155).

use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use meshing::layer::MeshLayer;
use meshing::{Opts, CS};
use render::geom::{chunk_origin, quads_to_mesh, MeshBuf};
use render::{Camera as RtsCamera, ChunkGrid, V3};
use sim_core::{Reference, SimEvent};

/// Симуляция и всё, что нужно, чтобы её показать.
#[derive(Resource)]
struct Sim {
    r: Reference,
    mesh: MeshLayer,
    grid: ChunkGrid,
    /// Ручка меша каждого чанка: пересобирается только грязный.
    chunks: Vec<Option<Handle<Mesh>>>,
    buf: MeshBuf,
    /// Тиков симуляции на кадр. Ноль — пауза.
    speed: u32,
}

/// Состояние камеры между кадрами.
#[derive(Resource)]
struct View {
    cam: RtsCamera,
    visible: usize,
}

/// Снимок и выход по расписанию — чтобы окно можно было запускать
/// из терминала и получать картинку без рук.
#[derive(Resource)]
struct Shot {
    path: Option<String>,
    after: f32,
    done: bool,
    frames: u64,
    seen: u64,
}

#[derive(Component)]
struct Chunk(usize);

/// Юнит: помечен номером, чтобы кадр за кадром двигать своё.
#[derive(Component)]
struct UnitMark(usize);

/// Снаряд в полёте. Метка без номера: снаряды пересоздаются каждый
/// кадр целиком, их число скачет, и держать за ними сущности дороже,
/// чем родить заново.
#[derive(Component)]
struct ShotMark;

/// Отметка взрыва живёт несколько кадров: событие приходит один раз,
/// а показать его надо дольше одного кадра.
#[derive(Component)]
struct BlastMark(u32);

/// Общие для всех меши и материалы.
#[derive(Resource)]
struct Art {
    unit: Handle<Mesh>,
    shot: Handle<Mesh>,
    blast: Handle<Mesh>,
    beam: Handle<Mesh>,
    side: [Handle<StandardMaterial>; 2],
    tracer: Handle<StandardMaterial>,
    ray: [Handle<StandardMaterial>; 2],
    boom: Handle<StandardMaterial>,
    rock: Handle<StandardMaterial>,
}

fn main() {
    let mut units = 2000usize;
    let mut seed = 42u64;
    let mut speed = 1u32;
    let mut shot: Option<String> = None;
    let mut after = 8.0f32;
    let mut frames = 0u64;
    let (mut yaw, mut pitch, mut dist) = (0.7f32, 0.75f32, 0.45f32);
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--units" => units = v.parse().unwrap_or(units),
            "--seed" => seed = v.parse().unwrap_or(seed),
            "--speed" => speed = v.parse().unwrap_or(speed),
            "--shot" => shot = Some(v.clone()),
            "--shot-after" => after = v.parse().unwrap_or(after),
            "--frames" => frames = v.parse().unwrap_or(frames),
            "--yaw" => yaw = v.parse().unwrap_or(yaw),
            "--pitch" => pitch = v.parse().unwrap_or(pitch),
            "--dist" => dist = v.parse().unwrap_or(dist),
            "--help" => {
                println!(
                    "battle [--units N] [--seed N] [--speed тиков-на-кадр] \
[--shot файл] [--shot-after сек] [--frames N] [--yaw] [--pitch] [--dist]"
                );
                return;
            }
            other => {
                eprintln!("неизвестный ключ {other}");
                return;
            }
        }
    }

    let t0 = Instant::now();
    let tuning = {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/sim.ron");
        let path = if here.exists() { here } else { std::path::PathBuf::from("data/sim.ron") };
        match modding::load_tuning(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                return;
            }
        }
    };
    let weapons = {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/weapons.ron");
        let path = if here.exists() { here } else { std::path::PathBuf::from("data/weapons.ron") };
        match modding::weapons::load(&path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("{e}");
                return;
            }
        }
    };
    let r = Reference::new(seed, units, tuning, weapons);
    let g = r.world.ground().expect("у эталона есть земля");
    let mesh = MeshLayer::new(&g.world, Opts::default());
    let (nx, ny, nz) = mesh.dims();
    let grid = ChunkGrid { nx, ny, nz, cs: CS as u32 };
    let side = sim_core::REF_SIDE as f32;
    println!(
        "мир {}×{}×{}, чанков {}, юнитов {units}, сборка {:.0} мс",
        g.world.nx,
        g.world.ny,
        g.world.nz,
        grid.len(),
        t0.elapsed().as_secs_f64() * 1e3
    );

    let cam = RtsCamera {
        target: V3::new(side * 0.5, side * 0.28, side * 0.5),
        yaw,
        pitch,
        distance: side * dist,
        fovy: 0.87,
        aspect: 16.0 / 9.0,
        near: 1.0,
        far: side * 4.0,
    };

    let n = grid.len();
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "rts-vox — бой".into(),
                present_mode: PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(Sim { r, mesh, grid, chunks: vec![None; n], buf: MeshBuf::default(), speed })
        .insert_resource(View { cam, visible: 0 })
        .insert_resource(Shot { path: shot, after, done: false, frames, seen: 0 })
        .add_systems(Startup, setup)
        .add_systems(Update, (step, control, cull, snap))
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sim: ResMut<Sim>,
    view: Res<View>,
) {
    let art = Art {
        unit: meshes.add(Cuboid::new(2.0, 3.0, 2.0)),
        shot: meshes.add(Cuboid::new(0.8, 0.8, 0.8)),
        blast: meshes.add(Sphere::new(2.5)),
        // Единичный столбик по оси Y: поворот и масштаб превращают
        // его в луч любой длины и направления.
        beam: meshes.add(Cuboid::new(0.5, 1.0, 0.5)),
        side: [
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.25, 0.55, 0.95),
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.45, 0.15),
                ..default()
            }),
        ],
        tracer: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.95, 0.5),
            emissive: LinearRgba::rgb(3.0, 2.6, 0.8),
            ..default()
        }),
        ray: [
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.5, 0.9, 1.0),
                emissive: LinearRgba::rgb(0.6, 3.0, 5.0),
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.4, 0.9),
                emissive: LinearRgba::rgb(4.0, 0.6, 3.0),
                ..default()
            }),
        ],
        boom: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.5, 0.15),
            emissive: LinearRgba::rgb(6.0, 2.0, 0.4),
            ..default()
        }),
        rock: materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.52, 0.48),
            perceptual_roughness: 0.95,
            ..default()
        }),
    };

    // Рельеф целиком — один раз. Дальше пересобираются только грязные.
    let t = Instant::now();
    let Sim { r, mesh, grid, chunks, buf, .. } = &mut *sim;
    let grid = *grid;
    let w = &r.world.ground().expect("земля").world;
    for (i, slot) in chunks.iter_mut().enumerate().take(grid.len()) {
        let quads = mesh.rebuilt_quads(w, i);
        if quads.is_empty() {
            continue;
        }
        buf.clear();
        quads_to_mesh(&quads, [0.0, 0.0, 0.0], buf);
        let h = meshes.add(buf_to_mesh(buf));
        *slot = Some(h.clone());
        let o = chunk_origin(i, grid.nx, grid.ny);
        commands.spawn((
            Mesh3d(h),
            MeshMaterial3d(art.rock.clone()),
            Transform::from_xyz(o[0], o[1], o[2]),
            Chunk(i),
        ));
    }
    println!("рельеф собран за {:.0} мс", t.elapsed().as_secs_f64() * 1e3);

    // Юниты: по кубику на живого. Число мест фиксировано — юниты
    // гибнут и воскресают в тех же местах (свободный список).
    let cap = sim.r.world.units.len().max(1);
    for i in 0..cap {
        let owner = usize::from(sim.r.world.units.owner.get(i).copied().unwrap_or(0) == 1);
        commands.spawn((
            Mesh3d(art.unit.clone()),
            MeshMaterial3d(art.side[owner].clone()),
            Transform::from_xyz(0.0, -1000.0, 0.0),
            Visibility::Hidden,
            UnitMark(i),
        ));
    }

    let eye = view.cam.eye();
    let t = view.cam.target;
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(eye.x, eye.y, eye.z).looking_at(Vec3::new(t.x, t.y, t.z), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight { shadow_maps_enabled: false, ..default() },
        Transform::from_xyz(1.0, 2.0, 0.6).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(art);
}

fn buf_to_mesh(b: &MeshBuf) -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.pos.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm.clone())
        .with_inserted_indices(Indices::U32(b.idx.clone()))
}

/// Тик симуляции и перенос её состояния в сцену.
#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sim: ResMut<Sim>,
    art: Res<Art>,
    mut units: Query<(&UnitMark, &mut Transform, &mut Visibility), Without<ShotMark>>,
    shots: Query<Entity, With<ShotMark>>,
    blasts: Query<(Entity, &mut BlastMark)>,
) {
    for _ in 0..sim.speed {
        sim.r.tick();
    }

    // Рельеф: только грязные чанки. Список берётся у мешинга, а не
    // угадывается по правкам.
    {
        let Sim { r, mesh, grid, chunks, buf, .. } = &mut *sim;
        let grid = *grid;
        let g = r.world.ground().expect("земля");
        // Правки копируются: мешеру нужен и мир, и они, а лежат оба
        // в земле. Копия за тик — сотни правок, это дешевле, чем
        // расщеплять `Ground`.
        let edits: Vec<voxel::Edit> = g.edits().to_vec();
        mesh.apply(&g.world, &edits);
        let dirty: Vec<usize> = mesh.dirty().iter().map(|&i| i as usize).collect();
        let w = &g.world;
        for i in dirty {
            let quads = mesh.rebuilt_quads(w, i);
            buf.clear();
            quads_to_mesh(&quads, [0.0, 0.0, 0.0], buf);
            let m = buf_to_mesh(buf);
            match chunks[i].clone() {
                Some(h) => {
                    let _ = meshes.insert(h.id(), m);
                }
                None => {
                    if quads.is_empty() {
                        continue;
                    }
                    let h = meshes.add(m);
                    chunks[i] = Some(h.clone());
                    let o = chunk_origin(i, grid.nx, grid.ny);
                    commands.spawn((
                        Mesh3d(h),
                        MeshMaterial3d(art.rock.clone()),
                        Transform::from_xyz(o[0], o[1], o[2]),
                        Chunk(i),
                    ));
                }
            }
        }
    }

    // Юниты.
    for (m, mut tr, mut vis) in units.iter_mut() {
        let i = m.0;
        if i >= sim.r.world.units.len() || !sim.r.world.units.alive[i] {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        tr.translation = Vec3::new(
            sim.r.world.units.x[i] as f32,
            sim.r.world.units.y[i] as f32 + 1.5,
            sim.r.world.units.z[i] as f32,
        );
    }

    // Снаряды: их число скачет, поэтому сущности создаются и убираются.
    for e in shots.iter() {
        commands.entity(e).despawn();
    }
    for &s in sim.r.world.shots.live() {
        let i = s as usize;
        commands.spawn((
            Mesh3d(art.shot.clone()),
            MeshMaterial3d(art.tracer.clone()),
            Transform::from_xyz(
                sim.r.world.shots.x[i] as f32,
                sim.r.world.shots.y[i] as f32,
                sim.r.world.shots.z[i] as f32,
            ),
            ShotMark,
        ));
    }

    // Взрывы: событие приходит один раз, отметка живёт несколько кадров.
    for (e, mut b) in blasts {
        if b.0 == 0 {
            commands.entity(e).despawn();
        } else {
            b.0 -= 1;
        }
    }
    for ev in sim.r.world.events() {
        match *ev {
            SimEvent::Blast { x, y, z, .. } => {
                commands.spawn((
                    Mesh3d(art.blast.clone()),
                    MeshMaterial3d(art.boom.clone()),
                    Transform::from_xyz(x as f32, y as f32, z as f32),
                    BlastMark(6),
                ));
            }
            SimEvent::Beam { x0, y0, z0, x1, y1, z1, owner } => {
                // Луч — вытянутый кубик от стрелка к цели. Живёт
                // столько же, сколько вспышка: событие приходит один
                // раз, а увидеть его надо дольше одного кадра.
                let a = Vec3::new(x0 as f32, y0 as f32 + 1.5, z0 as f32);
                let b = Vec3::new(x1 as f32, y1 as f32 + 1.5, z1 as f32);
                let d = b - a;
                let len = d.length().max(0.001);
                commands.spawn((
                    Mesh3d(art.beam.clone()),
                    MeshMaterial3d(art.ray[usize::from(owner == 1)].clone()),
                    Transform::from_translation((a + b) * 0.5)
                        .with_rotation(Quat::from_rotation_arc(Vec3::Y, d / len))
                        .with_scale(Vec3::new(1.0, len, 1.0)),
                    BlastMark(2),
                ));
            }
            SimEvent::Death { .. } => {}
        }
    }
}

/// Камера RTS. Считает `render::Camera`, Bevy получает готовый взгляд.
fn control(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut view: ResMut<View>,
    mut cams: Query<&mut Transform, With<Camera3d>>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
        return;
    }
    let dt = time.delta_secs();
    let pan = view.cam.distance * 0.6 * dt;
    let (s, c) = (view.cam.yaw.sin(), view.cam.yaw.cos());
    let mut d = (0.0f32, 0.0f32);
    if keys.pressed(KeyCode::KeyW) {
        d.1 -= 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        d.1 += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        d.0 -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        d.0 += 1.0;
    }
    view.cam.target.x += (d.0 * c - d.1 * s) * pan;
    view.cam.target.z += (d.0 * s + d.1 * c) * pan;
    if keys.pressed(KeyCode::KeyQ) {
        view.cam.yaw -= dt;
    }
    if keys.pressed(KeyCode::KeyE) {
        view.cam.yaw += dt;
    }
    if keys.pressed(KeyCode::KeyR) {
        view.cam.distance = (view.cam.distance * (1.0 - dt)).max(8.0);
    }
    if keys.pressed(KeyCode::KeyF) {
        view.cam.distance *= 1.0 + dt;
    }
    let eye = view.cam.eye();
    let t = view.cam.target;
    for mut tr in cams.iter_mut() {
        *tr =
            Transform::from_xyz(eye.x, eye.y, eye.z).looking_at(Vec3::new(t.x, t.y, t.z), Vec3::Y);
    }
}

/// Отсечение пирамидой — нашим счётом (ADR-142).
fn cull(
    mut view: ResMut<View>,
    sim: Res<Sim>,
    windows: Query<&Window>,
    mut chunks: Query<(&Chunk, &mut Visibility)>,
) {
    if let Ok(w) = windows.single() {
        let (ww, hh) = (w.width(), w.height());
        if hh > 0.0 {
            view.cam.aspect = ww / hh;
        }
    }
    let f = view.cam.frustum();
    let mut seen = 0usize;
    for (c, mut vis) in chunks.iter_mut() {
        let (lo, hi) = sim.grid.aabb(c.0);
        let on = f.intersects_aabb(lo, hi);
        *vis = if on { Visibility::Inherited } else { Visibility::Hidden };
        seen += usize::from(on);
    }
    view.visible = seen;
}

/// Снимок по времени и выход по кадрам.
///
/// Срок в секундах, а не в кадрах: пока конвейеры не собраны,
/// не рисуется ничего, и кадры от этого идут быстрее (ADR-166).
fn snap(
    mut commands: Commands,
    time: Res<Time>,
    sim: Res<Sim>,
    mut shot: ResMut<Shot>,
    mut exit: MessageWriter<AppExit>,
) {
    shot.seen += 1;
    if !shot.done && time.elapsed_secs() >= shot.after {
        if let Some(p) = shot.path.clone() {
            println!(
                "снимок на {:.1} с, тик {}: живых {}, снарядов {}, взрывов {}",
                time.elapsed_secs(),
                sim.r.world.tick,
                sim.r.world.units.live().len(),
                sim.r.world.shots.alive_count(),
                sim.r.world.blasts
            );
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        }
        shot.done = true;
    }
    if shot.frames != 0 && shot.seen >= shot.frames {
        exit.write(AppExit::Success);
    }
}
