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
//! а окно каждый кадр берёт у неё положения юнитов, цели, снаряды
//! и события прошедшего тика.
//!
//! Рельеф пересобирается **только там, где копали**: `MeshLayer` знает
//! грязные чанки, и полная пересборка мира каждый кадр стоила бы
//! на два порядка больше (ADR-155).
//!
//! Числа картинки — свет, небо, палитра — живут в `render::look`
//! и только там (ADR-175).

use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::mesh::Indices;
use bevy::anti_alias::fxaa::Fxaa;
use bevy::pbr::ScreenSpaceAmbientOcclusion;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use meshing::layer::MeshLayer;
use meshing::{Opts, CS};
use render::geom::{
    chunk_at, chunk_origin, class_bufs, quads_to_mesh_painted, MeshBuf, Painter, Slice,
};
use render::look;
use render::{yaw_to, Camera as RtsCamera, ChunkGrid, V3};
use sim_core::{Reference, SimEvent, NO_TARGET};

/// Симуляция и всё, что нужно, чтобы её показать.
#[derive(Resource)]
struct Sim {
    r: Reference,
    mesh: MeshLayer,
    grid: ChunkGrid,
    /// Ручка меша каждого чанка **по классу поверхности**: у породы
    /// и руды разная шероховатость, а её вершина нести не может
    /// (ADR-175). Пересобирается только грязный чанк.
    chunks: Vec<[Option<Handle<Mesh>>; look::CLASSES]>,
    bufs: [MeshBuf; look::CLASSES],
    painter: Painter,
    /// Тиков симуляции на кадр. Ноль — пауза.
    speed: u32,
}

/// Состояние камеры между кадрами.
#[derive(Resource)]
struct View {
    cam: RtsCamera,
    visible: usize,
    /// Сторона мира в вокселях. Туман и каскады теней считаются от неё,
    /// а не абсолютными числами: мир показа меняется ключом `--side`,
    /// и дальности обязаны ехать вместе с ним.
    side: f32,
}

/// Снимок и выход по расписанию — чтобы окно можно было запускать
/// из терминала и получать картинку без рук.
#[derive(Resource)]
struct Shot {
    path: Option<String>,
    after: f32,
    /// Кадр, раньше которого снимок не делается. Нужен не вместо срока,
    /// а **вместе** с ним: срок ждёт конвейеры (ADR-166), кадр ждёт
    /// сцену. Тик здесь равен кадру, поэтому два прогона с разной
    /// скоростью показывают одно и то же состояние боя, а не разное.
    at_frame: u64,
    done: bool,
    frames: u64,
    seen: u64,
    /// Времена кадров, миллисекунды. Копятся окнами по сто двадцать
    /// и печатаются все окна, а не лучшее: «до» и «после» сравнимы
    /// только по одинаковым окнам одной и той же сцены.
    hist: Vec<f64>,
}

/// Что включено в картинке.
///
/// Ключи здесь не для настройки вкуса, а **для замера**: цену тени,
/// свечения и раскраски рельефа иначе не отделить друг от друга —
/// включаются они все разом, и общее число ничего не объясняет.
#[derive(Resource, Clone, Copy)]
struct Fx {
    /// Каскадов теней. Ноль — теней нет вовсе.
    cascades: usize,
    bloom: bool,
    /// Щелевое затенение. Тянет за собой префасс глубины и нормалей
    /// и **запрещает** сглаживание по подвыборкам, поэтому вместе
    /// с ним включается FXAA: без неё воксельные рёбра рассыпаются
    /// в лесенку, и затенение в складках этого не окупает.
    ssao: bool,
    /// Красить рельеф материалом мира или собирать одним цветом.
    paint: bool,
}

/// Разбор кадра по работам ремешинга.
///
/// Без него общее время кадра ничего не объясняет: в нём и тик
/// симуляции, и пересборка квадов, и раскраска, и отдача мешей Bevy,
/// и все четыре меняются вместе.
#[derive(Resource, Default)]
struct Prof {
    /// Грязных чанков за кадр.
    dirty: Vec<f64>,
    /// Пересборка квадов мешером, миллисекунды.
    quads: Vec<f64>,
    /// Раскраска и разворот в вершины, миллисекунды.
    paint: Vec<f64>,
    /// Сборка `Mesh` и отдача его Bevy, миллисекунды.
    upload: Vec<f64>,
}

impl Prof {
    /// Медиана последних `n` значений. Медиана, а не среднее: один
    /// кадр с обвалом каскада сдвигает среднее и не сдвигает медиану.
    fn med(v: &[f64], n: usize) -> f64 {
        if v.is_empty() {
            return 0.0;
        }
        let mut t: Vec<f64> = v.iter().rev().take(n).copied().collect();
        t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        t[t.len() / 2]
    }
}

/// Цвет рельефа, когда раскраска выключена. Нужен только замеру:
/// без цвета вершины материал берёт свой `base_color`, и сравнивать
/// пришлось бы белое с коричневым.
const FLAT_GROUND: [f32; 3] = [0.47, 0.37, 0.24];

/// Воксель тела к вокселю мира.
///
/// Канон: ребро воксела мира 0.5 м, воксела тела 0.25 м (ADR-094).
/// Значит ровно половина — и танк 24×12×12 выходит 12×6×6 мировых
/// вокселей, шесть метров в длину. Как и должен.
const UNIT_SCALE: f32 = 0.5;

#[derive(Component)]
struct Chunk(usize);

/// Юнит: помечен номером, чтобы кадр за кадром двигать своё.
#[derive(Component)]
struct UnitMark(usize);

/// Башня того же юнита. Отдельная сущность, потому что смотрит она
/// в другую сторону, чем корпус (ADR-176).
#[derive(Component)]
struct TurretMark(usize);

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
    /// Корпус и башня по сторонам: цвет вершины несёт роль воксела,
    /// а тело у сторон разное, поэтому мешей четыре.
    hull: [Handle<Mesh>; 2],
    turret: [Handle<Mesh>; 2],
    shot: Handle<Mesh>,
    blast: Handle<Mesh>,
    beam: Handle<Mesh>,
    side: [Handle<StandardMaterial>; 2],
    tracer: Handle<StandardMaterial>,
    ray: [Handle<StandardMaterial>; 2],
    boom: Handle<StandardMaterial>,
    /// Материал рельефа по классу поверхности.
    ground: [Handle<StandardMaterial>; look::CLASSES],
}

fn main() {
    let mut units = 240usize;
    let mut seed = 42u64;
    // Показу нужен мир поплоще и побольше, чем гейту: на холмистой
    // карте строй прячется в складках, и боя не видно (ADR-172).
    let mut side = 384u32;
    let mut relief = 10u32;
    let mut speed = 1u32;
    let mut shot: Option<String> = None;
    let mut after = 8.0f32;
    let mut at_frame = 0u64;
    let mut frames = 0u64;
    let mut fx = Fx { cascades: look::SHADOW_CASCADES, bloom: true, ssao: look::SSAO, paint: true };
    let (mut yaw, mut pitch, mut dist) = (0.7f32, 0.75f32, 0.45f32);
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--units" => units = v.parse().unwrap_or(units),
            "--side" => side = v.parse().unwrap_or(side),
            "--relief" => relief = v.parse().unwrap_or(relief),
            "--seed" => seed = v.parse().unwrap_or(seed),
            "--speed" => speed = v.parse().unwrap_or(speed),
            "--shot" => shot = Some(v.clone()),
            "--shot-after" => after = v.parse().unwrap_or(after),
            "--shot-frame" => at_frame = v.parse().unwrap_or(at_frame),
            "--shadows" => fx.cascades = v.parse().unwrap_or(fx.cascades),
            "--bloom" => fx.bloom = v != "0",
            "--ssao" => fx.ssao = v != "0",
            "--paint" => fx.paint = v != "0",
            "--frames" => frames = v.parse().unwrap_or(frames),
            "--yaw" => yaw = v.parse().unwrap_or(yaw),
            "--pitch" => pitch = v.parse().unwrap_or(pitch),
            "--dist" => dist = v.parse().unwrap_or(dist),
            "--help" => {
                println!(
                    "battle [--units N] [--side N] [--relief N] [--seed N] \
[--speed тиков-на-кадр] [--shot файл] [--shot-after сек] [--shot-frame N] [--frames N] \
[--yaw] [--pitch] [--dist] [--shadows каскадов] [--bloom 0|1] [--ssao 0|1] [--paint 0|1]"
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
    let params = worldgen::Params {
        relief,
        ..worldgen::Params::sized(side, side / 2, side)
    };
    let r = Reference::with_params(seed, units, tuning, weapons, params);
    let g = r.world.ground().expect("у эталона есть земля");
    let mesh = MeshLayer::new(&g.world, Opts::default());
    let (nx, ny, nz) = mesh.dims();
    let grid = ChunkGrid { nx, ny, nz, cs: CS as u32 };
    let side = side as f32;
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
        .insert_resource(fx)
        .insert_resource(Prof::default())
        .insert_resource(DirectionalLightShadowMap { size: look::SHADOW_MAP })
        .insert_resource(GlobalAmbientLight {
            color: srgb(look::AMBIENT_SRGB),
            brightness: look::AMBIENT_BRIGHTNESS,
            ..default()
        })
        .insert_resource(Sim {
            r,
            mesh,
            grid,
            chunks: vec![[const { None }; look::CLASSES]; n],
            bufs: class_bufs(),
            painter: Painter::default(),
            speed,
        })
        .insert_resource(View { cam, visible: 0, side })
        .insert_resource(Shot {
            path: shot,
            after,
            at_frame,
            done: false,
            frames,
            seen: 0,
            hist: Vec::new(),
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (step, control, cull, snap))
        .run();
}

/// Цвет из палитры `look` — в цвет Bevy. Одно место, чтобы нигде
/// не потерялось, что числа там в sRGB.
fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sim: ResMut<Sim>,
    view: Res<View>,
    fx: Res<Fx>,
) {
    // Юниты — **настоящий воксельный том** из `voxel::unit`, а не
    // кубик: силуэт корпуса, башни и ствола читается, и видно, куда
    // смотрит юнит (ADR-172). Тело у сторон **разное** (ADR-176):
    // на дистанции RTS форму видно, а оттенок съедает туман.
    //
    // Масштаб — канонический: воксель тела вдвое мельче мирового
    // (ADR-094).
    let bodies = [voxel::unit::tank(), voxel::unit::strider()];
    let mut ubuf = MeshBuf::default();
    let mut unit_mesh = |body: &voxel::unit::UnitBody,
                         tint: [f32; 3],
                         part: Slice,
                         meshes: &mut Assets<Mesh>| {
        ubuf.clear();
        render::geom::volume_to_mesh(body, tint, UNIT_SCALE, part, &mut ubuf);
        meshes.add(buf_to_mesh(&ubuf))
    };
    let hull = [
        unit_mesh(&bodies[0], look::SIDE_TINT[0], Slice::Hull, &mut meshes),
        unit_mesh(&bodies[1], look::SIDE_TINT[1], Slice::Hull, &mut meshes),
    ];
    let turret = [
        unit_mesh(&bodies[0], look::SIDE_TINT[0], Slice::Turret, &mut meshes),
        unit_mesh(&bodies[1], look::SIDE_TINT[1], Slice::Turret, &mut meshes),
    ];
    // Цвет несут вершины, и это не «умножается на базу»: в Bevy 0.19.1
    // цвет вершины **подменяет** `base_color` целиком
    // (`pbr_fragment.wgsl`). База здесь не значит ничего, а
    // шероховатость и металличность — значат.
    let armour = || StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.55,
        metallic: 0.25,
        ..default()
    };
    let painted = fx.paint;
    let ground_mat = |c: look::Class, m: &mut Assets<StandardMaterial>| {
        let (rough, metal) = look::surface(c);
        m.add(StandardMaterial {
            base_color: if painted { Color::WHITE } else { srgb(FLAT_GROUND) },
            perceptual_roughness: rough,
            metallic: metal,
            ..default()
        })
    };
    let art = Art {
        hull,
        turret,
        shot: meshes.add(Cuboid::new(0.8, 0.8, 0.8)),
        blast: meshes.add(Sphere::new(2.5)),
        // Единичный столбик по оси Y: поворот и масштаб превращают
        // его в луч любой длины и направления.
        beam: meshes.add(Cuboid::new(0.5, 1.0, 0.5)),
        side: [materials.add(armour()), materials.add(armour())],
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
        ground: [
            ground_mat(look::Class::Rock, &mut materials),
            ground_mat(look::Class::Ore, &mut materials),
        ],
    };

    // Рельеф целиком — один раз. Дальше пересобираются только грязные.
    let t = Instant::now();
    let Sim { r, mesh, grid, chunks, bufs, painter, .. } = &mut *sim;
    let grid = *grid;
    let w = &r.world.ground().expect("земля").world;
    let mut tris = 0usize;
    for (i, slot) in chunks.iter_mut().enumerate().take(grid.len()) {
        let quads = mesh.rebuilt_quads(w, i);
        if quads.is_empty() {
            continue;
        }
        paint(&quads, i, grid, w, fx.paint, painter, bufs);
        for k in 0..look::CLASSES {
            if bufs[k].is_empty() {
                continue;
            }
            tris += bufs[k].idx.len() / 3;
            let h = meshes.add(buf_to_mesh(&bufs[k]));
            slot[k] = Some(h.clone());
            let o = chunk_origin(i, grid.nx, grid.ny);
            commands.spawn((
                Mesh3d(h),
                MeshMaterial3d(art.ground[k].clone()),
                Transform::from_xyz(o[0], o[1], o[2]),
                Chunk(i),
            ));
        }
    }
    println!(
        "рельеф собран за {:.0} мс, треугольников {tris}",
        t.elapsed().as_secs_f64() * 1e3
    );

    // Юниты: корпус и башня отдельными сущностями. Число мест
    // фиксировано — юниты гибнут и воскресают в тех же местах
    // (свободный список).
    let cap = sim.r.world.units.len().max(1);
    for i in 0..cap {
        let owner = usize::from(sim.r.world.units.owner.get(i).copied().unwrap_or(0) == 1);
        commands.spawn((
            Mesh3d(art.hull[owner].clone()),
            MeshMaterial3d(art.side[owner].clone()),
            Transform::from_xyz(0.0, -1000.0, 0.0),
            Visibility::Hidden,
            UnitMark(i),
        ));
        commands.spawn((
            Mesh3d(art.turret[owner].clone()),
            MeshMaterial3d(art.side[owner].clone()),
            Transform::from_xyz(0.0, -1000.0, 0.0),
            Visibility::Hidden,
            TurretMark(i),
        ));
    }

    let eye = view.cam.eye();
    let t = view.cam.target;
    let side = view.side;
    let cam = commands.spawn((
        Camera3d::default(),
        Camera {
            // Небо за краем мира. Тот же цвет, что у тумана: разойдись
            // они — на горизонте появилась бы полоса.
            clear_color: ClearColorConfig::Custom(srgb(look::SKY_COLOUR_SRGB)),
            ..default()
        },
        // Тонмаппинг тут не украшение: свет задан в люксах, и без
        // него всё ярче половины улетает в белое.
        Tonemapping::AcesFitted,
        DistanceFog {
            color: srgb(look::SKY_COLOUR_SRGB),
            falloff: FogFalloff::Linear {
                start: side * look::FOG_START_FRAC,
                end: side * look::FOG_END_FRAC,
            },
            ..default()
        },
        Transform::from_xyz(eye.x, eye.y, eye.z).looking_at(Vec3::new(t.x, t.y, t.z), Vec3::Y),
    )).id();
    // Трассеры и лучи светятся сильнее единицы — свечение делает
    // видимым то, ради чего они и заведены. Отдельным вставлением,
    // а не полем набора: компонент либо есть, либо его нет.
    if fx.bloom {
        commands.entity(cam).insert(Bloom::NATURAL);
    }
    if fx.ssao {
        commands
            .entity(cam)
            .insert((Msaa::Off, ScreenSpaceAmbientOcclusion::default(), Fxaa::default()));
    }

    // Солнце: тёплое, с каскадными тенями. Тень здесь не эффект —
    // без неё рельеф читается как плоская заливка, а юнит не стоит
    // на земле, а висит над ней.
    commands.spawn((
        DirectionalLight {
            color: srgb(look::SUN_SRGB),
            illuminance: look::SUN_LUX,
            shadow_maps_enabled: fx.cascades > 0,
            // Воксельный мир — сплошные плоские грани, и на них акне
            // теней видно лучше всего. Смещение подобрано по картинке.
            shadow_depth_bias: 0.06,
            shadow_normal_bias: 2.0,
            ..default()
        },
        Transform::default().looking_to(Vec3::from(look::SUN_DIR).normalize(), Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: fx.cascades.max(1),
            minimum_distance: 1.0,
            first_cascade_far_bound: side * look::SHADOW_FIRST_FRAC,
            maximum_distance: side * look::SHADOW_MAX_FRAC,
            overlap_proportion: 0.2,
        }
        .build(),
    ));
    // Заполняющий свет неба: холодный, слабый и **без теней**. Ровный
    // ambient задаёт теневой стороне уровень, но не форму; направленный
    // подсвет даёт ей направление, и оттого теневая сторона перестаёт
    // быть дырой. Карты окружения у нас нет — заполнять больше нечем.
    commands.spawn((
        DirectionalLight {
            color: srgb(look::SKY_SRGB),
            illuminance: look::SKY_LUX,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::default().looking_to(Vec3::from(look::SKY_DIR).normalize(), Vec3::Y),
    ));
    commands.insert_resource(art);
}

/// Развернуть квады чанка в буферы классов, покрасив материалом мира.
#[allow(clippy::too_many_arguments)]
fn paint(
    quads: &[meshing::Quad],
    i: usize,
    grid: ChunkGrid,
    w: &voxel::World,
    painted: bool,
    painter: &mut Painter,
    bufs: &mut [MeshBuf; look::CLASSES],
) {
    for b in bufs.iter_mut() {
        b.clear();
    }
    if !painted {
        render::geom::quads_to_mesh(quads, [0.0, 0.0, 0.0], &mut bufs[0]);
        return;
    }
    quads_to_mesh_painted(
        quads,
        [0.0, 0.0, 0.0],
        chunk_at(i, grid.nx, grid.ny),
        |x, y, z| w.material(x, y, z),
        painter,
        bufs,
    );
}

fn buf_to_mesh(b: &MeshBuf) -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.pos.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm.clone())
        .with_inserted_indices(Indices::U32(b.idx.clone()));
    if !b.col.is_empty() && b.col.len() == b.pos.len() {
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, b.col.clone());
    }
    m
}

/// Корпуса: всё, что не башня и не снаряд. Два запроса на `Transform`
/// в одной системе разрешены только непересекающимися множествами,
/// и разводит их именно `TurretMark`.
type HullQuery<'w, 's> = Query<
    'w,
    's,
    (&'static UnitMark, &'static mut Transform, &'static mut Visibility),
    (Without<ShotMark>, Without<TurretMark>),
>;

/// Башни: та же тройка, другое множество сущностей.
type TurretQuery<'w, 's> = Query<
    'w,
    's,
    (&'static TurretMark, &'static mut Transform, &'static mut Visibility),
    Without<ShotMark>,
>;

/// Тик симуляции и перенос её состояния в сцену.
#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sim: ResMut<Sim>,
    art: Res<Art>,
    fx: Res<Fx>,
    mut prof: ResMut<Prof>,
    mut units: HullQuery,
    mut turrets: TurretQuery,
    shots: Query<Entity, With<ShotMark>>,
    blasts: Query<(Entity, &mut BlastMark)>,
) {
    for _ in 0..sim.speed {
        sim.r.tick();
    }

    // Рельеф: только грязные чанки. Список берётся у мешинга, а не
    // угадывается по правкам.
    {
        let Sim { r, mesh, grid, chunks, bufs, painter, .. } = &mut *sim;
        let grid = *grid;
        let g = r.world.ground().expect("земля");
        // Правки копируются: мешеру нужен и мир, и они, а лежат оба
        // в земле. Копия за тик — сотни правок, это дешевле, чем
        // расщеплять `Ground`.
        let edits: Vec<voxel::Edit> = g.edits().to_vec();
        mesh.apply(&g.world, &edits);
        let dirty: Vec<usize> = mesh.dirty().iter().map(|&i| i as usize).collect();
        let w = &g.world;
        prof.dirty.push(dirty.len() as f64);
        let (mut t_quads, mut t_paint, mut t_up) = (0.0f64, 0.0f64, 0.0f64);
        for i in dirty {
            let t = Instant::now();
            // Квады берутся у **живого** мешера, а не пересобираются
            // с нуля: `apply` их уже обновил, и полная пересборка
            // выбрасывала весь выигрыш инкремента — 6.8 мс за кадр
            // из десяти на ровном месте (ADR-175).
            let quads = mesh.quads(i);
            t_quads += t.elapsed().as_secs_f64() * 1e3;
            let t = Instant::now();
            paint(&quads, i, grid, w, fx.paint, painter, bufs);
            t_paint += t.elapsed().as_secs_f64() * 1e3;
            let t = Instant::now();
            for k in 0..look::CLASSES {
                let m = buf_to_mesh(&bufs[k]);
                match chunks[i][k].clone() {
                    Some(h) => {
                        let _ = meshes.insert(h.id(), m);
                    }
                    None => {
                        if bufs[k].is_empty() {
                            continue;
                        }
                        let h = meshes.add(m);
                        chunks[i][k] = Some(h.clone());
                        let o = chunk_origin(i, grid.nx, grid.ny);
                        commands.spawn((
                            Mesh3d(h),
                            MeshMaterial3d(art.ground[k].clone()),
                            Transform::from_xyz(o[0], o[1], o[2]),
                            Chunk(i),
                        ));
                    }
                }
            }
            t_up += t.elapsed().as_secs_f64() * 1e3;
        }
        prof.quads.push(t_quads);
        prof.paint.push(t_paint);
        prof.upload.push(t_up);
    }

    // Юниты. Корпус разворачивается по движению — без этого строй
    // стоит боком и выглядит россыпью кубиков, а не колонной.
    let u = &sim.r.world.units;
    let at = |i: usize| Vec3::new(u.x[i] as f32, u.y[i] as f32, u.z[i] as f32);
    let alive = |i: usize| i < u.len() && u.alive[i];
    let hull_yaw = |i: usize| {
        let (dx, dz) = (u.fx[i].raw(), u.fz[i].raw());
        if dx == 0 && dz == 0 {
            None
        } else {
            Some(yaw_to(dx as f32, dz as f32))
        }
    };
    // Башня смотрит **в цель**, и цель эта — не догадка рендера:
    // симуляция выбрала её сама и держит в `units.target` (ADR-176).
    // Читать её можно, писать туда — нельзя.
    let aim_yaw = |i: usize| {
        let t = u.target[i];
        if t == NO_TARGET {
            return None;
        }
        let j = t as usize;
        if !alive(j) {
            return None;
        }
        let (dx, dz) = ((u.x[j] - u.x[i]) as f32, (u.z[j] - u.z[i]) as f32);
        if dx == 0.0 && dz == 0.0 {
            None
        } else {
            Some(yaw_to(dx, dz))
        }
    };

    for (m, mut tr, mut vis) in units.iter_mut() {
        let i = m.0;
        if !alive(i) {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        tr.translation = at(i);
        if let Some(y) = hull_yaw(i) {
            tr.rotation = Quat::from_rotation_y(y);
        }
    }
    for (m, mut tr, mut vis) in turrets.iter_mut() {
        let i = m.0;
        if !alive(i) {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        tr.translation = at(i);
        // Нет цели — башня едет вместе с корпусом. Оставить её
        // в прежнем повороте значит показать, что юнит целится
        // в того, кого уже нет.
        if let Some(y) = aim_yaw(i).or_else(|| hull_yaw(i)) {
            tr.rotation = Quat::from_rotation_y(y);
        }
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
    prof: Res<Prof>,
    mut shot: ResMut<Shot>,
    mut exit: MessageWriter<AppExit>,
) {
    shot.seen += 1;
    if !shot.done && time.elapsed_secs() >= shot.after && shot.seen >= shot.at_frame {
        if let Some(p) = shot.path.clone() {
            println!(
                "снимок на {:.1} с, кадр {}, тик {}: живых {}, снарядов {}, взрывов {}",
                time.elapsed_secs(),
                shot.seen,
                sim.r.world.tick,
                sim.r.world.units.live().len(),
                sim.r.world.shots.alive_count(),
                sim.r.world.blasts
            );
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        }
        shot.done = true;
    }
    // Первые шестьдесят кадров — прогрев: там ещё компилируются
    // конвейеры, и мерить их как установившийся режим нечестно
    // (ADR-166). Порог тот же, что у `view`, чтобы числа сравнивались.
    if shot.seen > 60 {
        let dt = time.delta_secs_f64() * 1e3;
        shot.hist.push(dt);
        if shot.hist.len() == 120 {
            let mut v = core::mem::take(&mut shot.hist);
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
            println!(
                "кадры {}..{}: {:.2} / {:.2} / {:.2} мс (медиана/99-й/худший); \
грязных {:.0}, квады {:.2}, раскраска {:.2}, отдача {:.2} мс; тик {}",
                shot.seen - 119,
                shot.seen,
                v[60],
                v[118],
                v[119],
                Prof::med(&prof.dirty, 120),
                Prof::med(&prof.quads, 120),
                Prof::med(&prof.paint, 120),
                Prof::med(&prof.upload, 120),
                sim.r.world.tick
            );
        }
    }
    if shot.frames != 0 && shot.seen >= shot.frames {
        exit.write(AppExit::Success);
    }
}
