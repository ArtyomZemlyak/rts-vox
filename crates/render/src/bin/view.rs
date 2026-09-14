//! Окно: сгенерированный мир под RTS-камерой.
//!
//! ```text
//! view --side 192 --seed 42
//! ```
//!
//! Управление: **WASD** — панорама, **Q/E** — поворот, **R/F** —
//! приближение, **Esc** — выход.
//!
//! **Здесь используется `bevy::mesh::Mesh`, и это нарушение записанного
//! антипаттерна** (`CLAUDE.md`: SoA-раскладка вынуждает переупаковку,
//! bevyengine/bevy#17000 — 150.8 мс на сетке 128³). Нарушение
//! **намеренное и временное**: правило записано по чужому замеру
//! на чужих данных, а своего у нас не было. Программа печатает своё —
//! сколько стоит собрать буферы и сколько отдать их Bevy, — и по нему
//! уже видно, упирается ли в это наш случай (ADR-155).

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

/// Готовая геометрия мира: по буферу на непустой чанк.
#[derive(Resource)]
struct World {
    grid: ChunkGrid,
    /// Номер чанка и его буферы.
    chunks: Vec<(usize, MeshBuf)>,
    /// Замер: сколько ушло на сборку буферов.
    build_ms: f64,
}

/// Состояние камеры между кадрами.
#[derive(Resource)]
struct View {
    cam: RtsCamera,
    /// Замер отсечения: сколько чанков видно.
    visible: usize,
}

#[derive(Component)]
struct Chunk(usize);

/// Замер устойчивой копки: копка меняет чанки **каждый тик**, и цена
/// границы с Bevy — не цена первого кадра, а цена переотдачи.
#[derive(Resource)]
struct Bench {
    /// Сколько чанков переотдавать за кадр. Ноль — не трогать.
    churn: usize,
    /// Через сколько кадров выйти. Ноль — не выходить.
    frames: u64,
    cursor: usize,
    seen: u64,
    /// Ручки чанков: без них нечего заменять.
    handles: Vec<Handle<Mesh>>,
    /// Куда сохранить снимок экрана и **через сколько секунд**.
    ///
    /// Снимок делается рендером, а не захватом экрана: так он повторяем
    /// и не зависит от того, что поверх окна.
    ///
    /// Срок в секундах, а не в кадрах, и это не мелочь. Конвейеры
    /// собираются асинхронно; пока они не готовы, не рисуется **ничего**,
    /// и кадры от этого идут вчетверо быстрее. Снимок на девяностом
    /// кадре выходил через четверть секунды после создания окна —
    /// и был пуст (ADR-166).
    shot: Option<String>,
    shot_after: f32,
    shot_done: bool,
    /// Времена кадров, миллисекунды. Печатаются все окна, а не лучшее.
    hist: Vec<f64>,
    /// Время самой переотдачи в кадре, миллисекунды.
    churn_ms: Vec<f64>,
    /// Сколько ассетов найдено на месте перед заменой. Ноль означает,
    /// что заменять нечего и замер ничего не грузит.
    present: u64,
}

fn main() {
    let mut side = 192u32;
    let mut seed = 42u64;
    let mut churn = 0usize;
    let mut frames = 0u64;
    let mut shot: Option<String> = None;
    // Секунды до снимка: раньше этого срока конвейеры ещё собираются,
    // и кадр выходит пустым.
    let mut shot_after = 4.0f32;
    let (mut yaw, mut pitch, mut dist) = (0.7f32, 0.95f32, 0.9f32);
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--side" => side = v.parse().unwrap_or(side),
            "--seed" => seed = v.parse().unwrap_or(seed),
            "--churn" => churn = v.parse().unwrap_or(churn),
            "--frames" => frames = v.parse().unwrap_or(frames),
            "--shot" => shot = Some(v.clone()),
            "--shot-after" => shot_after = v.parse().unwrap_or(shot_after),
            "--yaw" => yaw = v.parse().unwrap_or(yaw),
            "--pitch" => pitch = v.parse().unwrap_or(pitch),
            "--dist" => dist = v.parse().unwrap_or(dist),
            "--help" => {
                println!(
                    "view [--side N] [--seed N] [--churn N] [--frames N] \
[--shot файл] [--shot-after сек] [--yaw рад] [--pitch рад] [--dist доля]"
                );
                return;
            }
            other => {
                eprintln!("неизвестный ключ {other}");
                return;
            }
        }
    }

    // Параметры считает генератор: рельеф задаётся абсолютными
    // вокселями и молча обрезается в низком мире (ADR-166).
    let params = worldgen::Params::sized(side, side / 2, side);
    let w = voxel::World::new(
        params.nx,
        params.ny,
        params.nz,
        1 << 16,
        Box::new(worldgen::Terrain { params, seed }),
    );
    let mesh = MeshLayer::new(&w, Opts::default());
    let (nx, ny, nz) = mesh.dims();
    let grid = ChunkGrid {
        nx,
        ny,
        nz,
        cs: CS as u32,
    };

    let t = Instant::now();
    let mut chunks: Vec<(usize, MeshBuf)> = Vec::new();
    for i in 0..grid.len() {
        let quads = mesh.rebuilt_quads(&w, i);
        if quads.is_empty() {
            continue;
        }
        let mut buf = MeshBuf::default();
        quads_to_mesh(&quads, [0.0, 0.0, 0.0], &mut buf);
        chunks.push((i, buf));
    }
    let build_ms = t.elapsed().as_secs_f64() * 1e3;

    let tris: usize = chunks.iter().map(|(_, b)| b.idx.len() / 3).sum();
    println!(
        "мир {side}×{}×{side}, чанков {} из {}",
        params.ny,
        chunks.len(),
        grid.len()
    );
    println!("треугольников {tris}, сборка буферов {build_ms:.0} мс");

    let mid = side as f32 * 0.5;
    let cam = RtsCamera {
        target: V3::new(mid, params.ny as f32 * 0.55, mid),
        yaw,
        pitch,
        distance: side as f32 * dist,
        fovy: 0.87,
        aspect: 16.0 / 9.0,
        near: 1.0,
        far: side as f32 * 4.0,
    };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "rts-vox".into(),
                // Без этого замер меряет частоту монитора, а не работу:
                // при `Fifo` кадр выходит ровно 1000/165 = 6.06 мс
                // и на переотдачу чанков не реагирует вовсе.
                present_mode: PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(World {
            grid,
            chunks,
            build_ms,
        })
        .insert_resource(View { cam, visible: 0 })
        .insert_resource(Bench {
            churn,
            frames,
            cursor: 0,
            seen: 0,
            handles: Vec::new(),
            shot,
            shot_after,
            shot_done: false,
            hist: Vec::new(),
            churn_ms: Vec::new(),
            present: 0,
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (control, cull, churn_chunks, stats))
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world: Res<World>,
    view: Res<View>,
    mut bench: ResMut<Bench>,
) {
    let rock = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.52, 0.48),
        perceptual_roughness: 0.95,
        ..default()
    });

    // Замер, ради которого вообще написано это окно. Считается в три
    // приёма, потому что первый приём — не тот, о котором антипаттерн.
    //
    // 1. Собрать `Mesh` из наших буферов: три `clone` векторов.
    // 2. **Переупаковка SoA → чередование.** Её делает Bevy, но не здесь,
    //    а позже, в рендер-мире, в `prepare_asset`; `meshes.add` только
    //    кладёт ассет в хранилище. Вызов ниже — зонд: та же самая
    //    функция `create_packed_vertex_buffer_data`, что и у Bevy, на тех
    //    же данных. Именно она копирует по двенадцать байт на атрибут
    //    на вершину, и именно про неё bevyengine/bevy#17000.
    // 3. Отдать ассеты и породить сущности.
    let t = Instant::now();
    let built: Vec<Mesh> = world
        .chunks
        .iter()
        .map(|(_, buf)| {
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, buf.pos.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, buf.nrm.clone())
            .with_inserted_indices(Indices::U32(buf.idx.clone()))
        })
        .collect();
    let make_ms = t.elapsed().as_secs_f64() * 1e3;

    // Три прогона, печатаются все: лучший из трёх — это не замер.
    let mut bytes = 0usize;
    for run in 0..3 {
        let t = Instant::now();
        let mut total = 0usize;
        for m in built.iter() {
            total += m.create_packed_vertex_buffer_data().len();
        }
        let ms = t.elapsed().as_secs_f64() * 1e3;
        bytes = total;
        println!(
            "переупаковка, прогон {}: {:.2} мс на {} чанков — {:.3} мс на чанк",
            run + 1,
            ms,
            built.len(),
            ms / built.len().max(1) as f64
        );
    }
    println!(
        "вершинных байт {} ({:.1} МиБ)",
        bytes,
        bytes as f64 / (1 << 20) as f64
    );

    let t = Instant::now();
    for ((i, _), m) in world.chunks.iter().zip(built) {
        let o = chunk_origin(*i, world.grid.nx, world.grid.ny);
        let h = meshes.add(m);
        bench.handles.push(h.clone());
        commands.spawn((
            Mesh3d(h),
            MeshMaterial3d(rock.clone()),
            Transform::from_xyz(o[0], o[1], o[2]),
            Chunk(*i),
        ));
    }
    let add_ms = t.elapsed().as_secs_f64() * 1e3;
    println!(
        "сборка Mesh {make_ms:.1} мс, раздача ассетов {add_ms:.1} мс на {} чанков",
        world.chunks.len()
    );
    println!(
        "итого от вокселей до сцены {:.0} мс",
        world.build_ms + make_ms + add_ms
    );

    let eye = view.cam.eye();
    let t = view.cam.target;
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(eye.x, eye.y, eye.z).looking_at(Vec3::new(t.x, t.y, t.z), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(1.0, 2.0, 0.6).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Камера RTS: панорама в плоскости земли, поворот вокруг цели,
/// приближение. Считает `render::Camera`, Bevy получает готовый взгляд.
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
    // Панорама идёт в осях экрана, а не мира: иначе при повороте
    // камеры «вперёд» перестаёт быть вперёд.
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

/// Отсечение пирамидой — **нашим** счётом, а не бевевским: ради него
/// крейт и начинался (ADR-142).
fn cull(
    mut view: ResMut<View>,
    world: Res<World>,
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
        let (lo, hi) = world.grid.aabb(c.0);
        let on = f.intersects_aabb(lo, hi);
        *vis = if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        seen += usize::from(on);
    }
    view.visible = seen;
}

/// Переотдача чанков: имитация устойчивой копки. Каждый кадр `churn`
/// чанков собираются в `Mesh` заново и кладутся под ту же ручку —
/// ровно то, что делает ремешинг.
///
/// Инкремент мешинга здесь **пропадает**: `Mesh` заменяется целиком,
/// частичной записи у него нет. В этом и суть антипаттерна, а не
/// в абсолютной цене одной переупаковки.
fn churn_chunks(mut bench: ResMut<Bench>, world: Res<World>, mut meshes: ResMut<Assets<Mesh>>) {
    if bench.churn == 0 || bench.handles.is_empty() {
        return;
    }
    let n = bench.handles.len();
    let t = Instant::now();
    let mut present = 0u64;
    for k in 0..bench.churn.min(n) {
        let i = (bench.cursor + k) % n;
        let buf = &world.chunks[i].1;
        let m = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, buf.pos.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, buf.nrm.clone())
        .with_inserted_indices(Indices::U32(buf.idx.clone()));
        let id = bench.handles[i].id();
        present += u64::from(meshes.get(id).is_some());
        let _ = meshes.insert(id, m);
    }
    let ms = t.elapsed().as_secs_f64() * 1e3;
    bench.churn_ms.push(ms);
    bench.present += present;
    bench.cursor = (bench.cursor + bench.churn) % n;
}

/// Времена кадров окнами по сто двадцать. Первые шестьдесят кадров —
/// прогрев: там ещё компилируются конвейеры, и мерить их как
/// установившийся режим нечестно.
fn stats(
    mut commands: Commands,
    time: Res<Time>,
    view: Res<View>,
    mut bench: ResMut<Bench>,
    mut exit: MessageWriter<AppExit>,
) {
    bench.seen += 1;
    // Снимок — по времени и один раз.
    if !bench.shot_done && time.elapsed_secs() >= bench.shot_after {
        if let Some(path) = bench.shot.clone() {
            println!("снимок на {:.1} с: {path}", time.elapsed_secs());
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
        }
        bench.shot_done = true;
    }
    if bench.seen <= 60 {
        return;
    }
    let dt = time.delta_secs_f64() * 1e3;
    bench.hist.push(dt);
    if bench.hist.len() == 120 {
        let mut v = core::mem::take(&mut bench.hist);
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let mut c = core::mem::take(&mut bench.churn_ms);
        c.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let cm = if c.is_empty() { 0.0 } else { c[c.len() / 2] };
        let cw = if c.is_empty() { 0.0 } else { c[c.len() - 1] };
        println!(
            "кадры {}..{}: кадр {:.2} / {:.2} / {:.2} мс (медиана/99-й/худший); \
переотдача {cm:.2} / {cw:.2} мс; видно {}, переотдано {}/кадр, ассет на месте {}",
            bench.seen - 119,
            bench.seen,
            v[60],
            v[118],
            v[119],
            view.visible,
            bench.churn,
            bench.present
        );
        bench.present = 0;
    }
    if bench.frames != 0 && bench.seen >= bench.frames {
        exit.write(AppExit::Success);
    }
}
