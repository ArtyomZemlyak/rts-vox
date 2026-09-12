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
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::{
    Atmosphere, AtmosphereEnvironmentMapLight, CascadeShadowConfigBuilder,
    DirectionalLightShadowMap, SunDisk,
};
use bevy::mesh::Indices;
use bevy::anti_alias::fxaa::Fxaa;
use bevy::pbr::{AtmosphereSettings, ScreenSpaceAmbientOcclusion};
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
use render::tex::tile_image;
use render::{yaw_of, yaw_to, Camera as RtsCamera, ChunkGrid, V3};
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
    /// Процедурная атмосфера: небо, дымка по глубине и карта окружения
    /// для отражений (ADR-198). Выключенная возвращает плоскую заливку
    /// и линейный туман — то, с чем эту цену и сравнивают.
    sky: bool,
    /// Фаска на ребре воксела плиткой в `base_color` (ADR-199).
    /// Выключенная снимает и текстуру, и её UV с вершин — иначе
    /// в замер попала бы только половина цены.
    chamfer: bool,
}

/// Время суток и глубина дымки.
///
/// Отдельно от `Fx`, потому что это не «включено или нет», а число:
/// высота солнца перекрашивает сцену целиком, и подбирать её,
/// пересобирая крейт, — это по две с половиной минуты на попытку.
/// Значения по умолчанию — из `look`, и там же они и живут.
#[derive(Resource, Clone, Copy)]
struct Sky {
    /// Высота солнца над горизонтом, радианы.
    elevation: f32,
    /// Азимут солнца, радианы.
    azimuth: f32,
    /// Метров атмосферы на воксель мира — ручка глубины.
    haze: f32,
    /// Экспозиция камеры, EV100.
    ev100: f32,
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

/// Стареющий эффект: событие симуляции живёт один тик (`SimEvent`),
/// а увидеть его надо дольше. Возраст считается в **кадрах**, потому
/// что глаз смотрит кадры, а не тики.
#[derive(Component)]
struct Fade {
    age: u32,
    life: u32,
    kind: Kind,
    /// Радиус воронки в мировых вокселях; у прочих эффектов — своя
    /// мерка длины.
    size: f32,
}

/// Что именно стареет. Разные вещи гаснут по-разному: шар раздувается
/// и остывает, луч гаснет не раздуваясь, свет просто исчезает.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Огненный шар воронки.
    Blast,
    /// Луч и дульная вспышка: гаснут, не меняя размера.
    Beam,
    Muzzle,
    /// Точечный свет от вспышки: живёт меньше самой вспышки.
    Light,
}

/// Какие номера снарядов были живы в прошлом кадре.
///
/// Выстрела среди событий симуляции нет, а дульная вспышка нужна.
/// Новый снаряд — это номер, которого в прошлом кадре не было; иначе
/// вспышка загоралась бы на каждом кадре полёта.
#[derive(Resource, Default)]
struct Seen {
    was: Vec<bool>,
    live: Vec<bool>,
}

/// Обломок: летит по параболе и крутится. **Это декорация.** Он ничего
/// не весит, никуда не попадает и в состояние симуляции не входит —
/// границу слоёв это не задевает, потому что читает он только событие.
#[derive(Component)]
struct Debris {
    vel: Vec3,
    spin: Vec3,
    age: u32,
}

/// Общие для всех меши и материалы.
#[derive(Resource)]
struct Art {
    /// Корпус и башня по сторонам: цвет вершины несёт роль воксела,
    /// а тело у сторон разное, поэтому мешей четыре.
    /// Меш корпуса **по отделке**: сталь, стекло и резина идут
    /// разными материалами, а материал у меша один (ADR-211).
    /// Пустая отделка ручки не получает и сущности не заводит.
    hull: [[Option<Handle<Mesh>>; look::FINISHES]; 2],
    turret: [[Option<Handle<Mesh>>; look::FINISHES]; 2],
    /// Шар единичного радиуса: вспышка, дуло, огонь воронки. Размер
    /// задаёт масштаб, потому что размер у каждого свой и меняется
    /// каждый кадр.
    ball: Handle<Mesh>,
    /// Единичный столбик по оси Y: поворот и масштаб превращают его
    /// и в луч, и в трассер.
    bar: Handle<Mesh>,
    /// Кубик обломка.
    chip: Handle<Mesh>,
    /// Материал отделки. У сторон он **общий**: цвет несут вершины,
    /// а сталь остаётся сталью, какого бы цвета ни был корпус.
    finish: [Handle<StandardMaterial>; look::FINISHES],
    tracer: [Handle<StandardMaterial>; 2],
    /// Лестница остывания огня воронки: от белого к тёмно-красному.
    /// Материалов столько, сколько ступеней, потому что `StandardMaterial`
    /// общий у всех, кто на него ссылается, — покрасить **один** шар,
    /// не задев соседние, иначе нельзя.
    boom: Vec<Handle<StandardMaterial>>,
    /// То же для луча, по стороне.
    ray: [Vec<Handle<StandardMaterial>>; 2],
    /// И для дульной вспышки.
    muzzle: Vec<Handle<StandardMaterial>>,
    /// Пыль и обломки: порода по стороне не различается.
    dust: Handle<StandardMaterial>,
    /// Материал рельефа по классу поверхности.
    ground: [Handle<StandardMaterial>; look::CLASSES],
}

/// Светящийся материал: цвет **за пределами единицы** прямо
/// в `base_color`, смешивание сложением.
///
/// Не через `emissive`, и это не вкус: `pbr.wgsl` при `unlit`
/// отдаёт `base_color` и **не зовёт** расчёт света вовсе, а эмиссия
/// живёт внутри него (`apply_pbr_lighting`). Сочетание `unlit` плюс
/// `emissive` не рисует ничего — проверено картинкой, где пропали
/// разом все трассеры, лучи и вспышки.
///
/// Расчёт света здесь не нужен: огонь не освещён, он светит. Заодно
/// уходит и блик неба, который на чёрном шаре читался бы серой плёнкой.
fn glow(rgb: [f32; 3], a: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::LinearRgba(LinearRgba::new(rgb[0], rgb[1], rgb[2], a)),
        // Сложением: огонь прибавляется к тому, что за ним, и потому
        // не требует сортировки по глубине и не спорит с порядком,
        // в котором Bevy его нарисует.
        alpha_mode: AlphaMode::Add,
        unlit: true,
        ..default()
    }
}

/// Лестница остывания: `n` материалов от горячего к остывшему.
///
/// Смешивание **сложением**: огонь прибавляется к тому, что за ним,
/// и потому не требует сортировки по глубине и не спорит с порядком,
/// в котором Bevy их нарисует. Прозрачность здесь — множитель
/// прибавки, а не «сквозь него видно».
fn ramp(
    m: &mut Assets<StandardMaterial>,
    n: usize,
    hot: [f32; 3],
    cold: [f32; 3],
    fade_at: f32,
) -> Vec<Handle<StandardMaterial>> {
    (0..n)
        .map(|i| {
            let t = if n <= 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
            // Остывание по квадрату: пик виден один кадр, а не треть
            // всей жизни.
            let k = (1.0 - t) * (1.0 - t);
            let e = [
                cold[0] + (hot[0] - cold[0]) * k,
                cold[1] + (hot[1] - cold[1]) * k,
                cold[2] + (hot[2] - cold[2]) * k,
            ];
            // Прозрачность держится, пока эффект не отжил `fade_at`
            // своей жизни, и потом уходит в ноль **по квадрату**.
            // Линейная оставляла в конце большой бледный шар: огонь
            // раздувается, и его площадь растёт быстрее, чем падает
            // яркость.
            let a = ((1.0 - t) / (1.0 - fade_at)).clamp(0.0, 1.0);
            let a = a * a;
            m.add(glow([e[0], e[1], e[2]], a))
        })
        .collect()
}

/// Целое из трёх координат — для разлёта обломков.
///
/// Свой хеш, а не `rand`: два прогона одной сцены обязаны дать
/// одинаковые снимки, иначе «до» и «после» сравнивать нечем. В хеш
/// состояния это не входит и входить не может — рендер его не считает.
fn scatter(x: i32, y: i32, z: i32, k: usize) -> u32 {
    let mut h = 0x9E37_79B9u32;
    for v in [x as u32, y as u32, z as u32, k as u32] {
        h ^= v.wrapping_mul(0x85EB_CA6B);
        h = h.rotate_left(13).wrapping_mul(0xC2B2_AE35);
    }
    h ^= h >> 16;
    h
}

/// Дробь `0..1` из хеша.
fn frac(h: u32) -> f32 {
    (h & 0xFF_FFFF) as f32 / 16_777_216.0
}

fn main() {
    let mut units = 240usize;
    let mut seed = 42u64;
    // Показу нужен мир побольше, чем гейту: строй в полтысячи машин
    // на маленькой карте стоит стеной (ADR-172).
    let mut side = 384u32;
    // Размах рельефа сцены показа, в вокселях. **Не десять.** На
    // десяти вокселях (пять метров на сто девяносто два) карта плоская,
    // и склон получается в один воксель на много шагов: рельеф читается
    // не холмами, а горизонталями топографической карты. На тридцати
    // появляются лощины и гребни, а проходимость держится — дыр
    // в навигации по-прежнему нет (ADR-212).
    let mut relief = 30u32;
    let mut speed = 1u32;
    let mut shot: Option<String> = None;
    let mut after = 8.0f32;
    let mut at_frame = 0u64;
    let mut frames = 0u64;
    let mut fx = Fx {
        cascades: look::SHADOW_CASCADES,
        bloom: true,
        ssao: look::SSAO,
        paint: true,
        sky: true,
        chamfer: true,
    };
    let mut sky = Sky {
        elevation: look::SUN_ELEVATION,
        azimuth: look::SUN_AZIMUTH,
        haze: look::HAZE_METRES_PER_VOXEL,
        ev100: look::EV100,
    };
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
            "--sky" => fx.sky = v != "0",
            "--chamfer" => fx.chamfer = v != "0",
            "--sun" => sky.elevation = v.parse().unwrap_or(sky.elevation),
            "--sun-az" => sky.azimuth = v.parse().unwrap_or(sky.azimuth),
            "--haze" => sky.haze = v.parse().unwrap_or(sky.haze),
            "--ev" => sky.ev100 = v.parse().unwrap_or(sky.ev100),
            "--frames" => frames = v.parse().unwrap_or(frames),
            "--yaw" => yaw = v.parse().unwrap_or(yaw),
            "--pitch" => pitch = v.parse().unwrap_or(pitch),
            "--dist" => dist = v.parse().unwrap_or(dist),
            "--help" => {
                println!(
                    "battle [--units N] [--side N] [--relief N] [--seed N] \
[--speed тиков-на-кадр] [--shot файл] [--shot-after сек] [--shot-frame N] [--frames N] \
[--yaw] [--pitch] [--dist] [--shadows каскадов] [--bloom 0|1] [--ssao 0|1] [--paint 0|1] \
[--sky 0|1] [--chamfer 0|1] \
[--sun высота] [--sun-az азимут] [--haze метров-на-воксель] [--ev EV100]"
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
        .insert_resource(sky)
        .insert_resource(Prof::default())
        .insert_resource(Seen::default())
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
        .add_systems(Update, (step, effects, control, cull, snap))
        .run();
}

/// Цвет из палитры `look` — в цвет Bevy. Одно место, чтобы нигде
/// не потерялось, что числа там в sRGB.
fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut mediums: ResMut<Assets<ScatteringMedium>>,
    mut sim: ResMut<Sim>,
    view: Res<View>,
    fx: Res<Fx>,
    sky: Res<Sky>,
) {
    // Уровень земли: генератор кладёт поверхность на половину высоты
    // мира (`worldgen::Params::sized`). Нужен атмосфере — иначе строй
    // стоял бы километрами выше уровня моря, а там воздух реже.
    let ground = sim.r.world.ground().expect("земля").world.ny as f32 * 0.5;
    // Юниты — **настоящий воксельный том** из `voxel::unit`, а не
    // кубик: силуэт корпуса, башни и ствола читается, и видно, куда
    // смотрит юнит (ADR-172). Тело у сторон **разное** (ADR-176):
    // на дистанции RTS форму видно, а оттенок съедает туман.
    //
    // Масштаб — канонический: воксель тела вдвое мельче мирового
    // (ADR-094).
    let bodies = [voxel::unit::tank(), voxel::unit::strider()];
    let uv = fx.chamfer;
    let mut ubufs: [MeshBuf; look::FINISHES] = std::array::from_fn(|_| MeshBuf::default());
    let mut unit_mesh = |body: &voxel::unit::UnitBody,
                         tint: [f32; 3],
                         part: Slice,
                         meshes: &mut Assets<Mesh>|
     -> [Option<Handle<Mesh>>; look::FINISHES] {
        for b in ubufs.iter_mut() {
            b.clear();
        }
        // **Тело юнита разворачивается с геометрической фаской**
        // (ADR-206), а не с нарисованной. Разделение по предмету:
        // рельефу текстурная фаска (ADR-199) — там миллионы вокселей
        // и пересборка каждый тик; телу геометрическая — там тысячи,
        // и меш строится раз на тип. Только геометрия меняет силуэт;
        // текстура красит грань, оставляя её плоской.
        let ty = body.turret_y as usize;
        let layers = match part {
            Slice::All => (0, usize::MAX),
            Slice::Hull if ty == 0 => (0, usize::MAX),
            Slice::Hull => (0, ty),
            Slice::Turret if ty == 0 => (0, 0),
            Slice::Turret => (ty, usize::MAX),
        };
        render::model::body_to_mesh(
            body,
            tint,
            UNIT_SCALE,
            layers,
            render::model::BEVEL,
            &mut ubufs,
        );
        // Пустая отделка ручки не получает: у безбашенного тела
        // башня пуста целиком, и заводить под неё сущность
        // с нулевым мешем незачем.
        std::array::from_fn(|k| {
            (!ubufs[k].idx.is_empty()).then(|| meshes.add(buf_to_mesh(&ubufs[k], uv)))
        })
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
    // У брони текстурной фаски **нет**: ребро у неё срезано
    // по-настоящему, и нарисованная тень легла бы второй линией
    // внутри и без того скошенной грани (ADR-209).
    // **Материал говорит, из чего вещь сделана.** Цвет вершины
    // подменяет `base_color` целиком (`pbr_fragment.wgsl`), поэтому
    // раскраска сторон остаётся при себе, а шероховатость
    // и металличность делают из неё сталь, стекло или резину
    // (ADR-211).
    let finish_mat = |f: look::Finish, m: &mut Assets<StandardMaterial>| {
        let (rough, metal) = look::finish_surface(f);
        m.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: rough,
            metallic: metal,
            // Обнажённое ядро светится само: увидеть его надо сразу,
            // а отражённого света внутри разбитого корпуса нет.
            emissive: if f == look::Finish::Core {
                let g = look::CORE_GLOW;
                LinearRgba::rgb(g[0], g[1], g[2])
            } else {
                LinearRgba::BLACK
            },
            ..default()
        })
    };
    let painted = fx.paint;
    // **У каждого класса своя поверхность**: своя плитка цвета, своя
    // карта нормалей, своя шероховатость. Пока плитка была одна,
    // земля отличалась от камня только оттенком, и оба читались
    // краской (ADR-211).
    let tiles = fx.chamfer;
    let ground_mat = |c: look::Class, i: &mut Assets<Image>, m: &mut Assets<StandardMaterial>| {
        let (rough, metal) = look::surface(c);
        let (albedo, normal) = if tiles {
            (
                Some(i.add(tile_image(look::surface_tile(c), true))),
                Some(i.add(tile_image(look::normal_tile(c), false))),
            )
        } else {
            (None, None)
        };
        m.add(StandardMaterial {
            base_color: if painted { Color::WHITE } else { srgb(FLAT_GROUND) },
            base_color_texture: albedo,
            normal_map_texture: normal,
            // UV приходят **в вокселях**, а плитка накрывает несколько:
            // без этого сжатия узор повторялся бы на каждом кубе
            // правильной сеткой.
            uv_transform: bevy::math::Affine2::from_scale(Vec2::splat(1.0 / look::TILE_VOXELS)),
            perceptual_roughness: rough,
            metallic: metal,
            ..default()
        })
    };
    // Трассер светится, а не освещён.
    let tracer = |c: [f32; 3], m: &mut Assets<StandardMaterial>| m.add(glow(c, 1.0));
    let art = Art {
        hull,
        turret,
        ball: meshes.add(Sphere::new(1.0).mesh().ico(2).expect("шар вспышки"))
            ,
        bar: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        chip: meshes.add(Cuboid::new(
            look::DEBRIS_SIZE,
            look::DEBRIS_SIZE,
            look::DEBRIS_SIZE,
        )),
        finish: look::FINISH_ALL.map(|f| finish_mat(f, &mut materials)),
        tracer: [
            tracer([9.0, 6.4, 1.8], &mut materials),
            tracer([9.0, 4.2, 1.1], &mut materials),
        ],
        boom: ramp(
            &mut materials,
            look::BLAST_STEPS,
            look::BLAST_HOT,
            look::BLAST_COLD,
            0.15,
        ),
        ray: [
            ramp(&mut materials, look::BEAM_FRAMES as usize, [1.0, 2.6, 4.2], [0.10, 0.26, 0.42], 0.2),
            ramp(&mut materials, look::BEAM_FRAMES as usize, [4.2, 1.5, 0.45], [0.42, 0.15, 0.05], 0.2),
        ],
        muzzle: ramp(
            &mut materials,
            look::MUZZLE_FRAMES as usize,
            [22.0, 14.0, 5.0],
            [3.0, 1.4, 0.3],
            0.1,
        ),
        dust: materials.add(StandardMaterial {
            base_color: srgb(look::DUST_SRGB),
            perceptual_roughness: 0.95,
            ..default()
        }),
        ground: [
            ground_mat(look::Class::Soil, &mut images, &mut materials),
            ground_mat(look::Class::Rock, &mut images, &mut materials),
            ground_mat(look::Class::Ore, &mut images, &mut materials),
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
            let h = meshes.add(buf_to_mesh(&bufs[k], fx.chamfer));
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
        // По сущности на отделку. Системы движения ищут по метке
        // и обновляют их все разом одним и тем же преобразованием:
        // корпус остаётся корпусом, из скольких бы материалов он
        // ни был собран.
        for (k, m) in art.finish.iter().enumerate() {
            if let Some(h) = art.hull[owner][k].clone() {
                commands.spawn((
                    Mesh3d(h),
                    MeshMaterial3d(m.clone()),
                    Transform::from_xyz(0.0, -1000.0, 0.0),
                    Visibility::Hidden,
                    UnitMark(i),
                ));
            }
            if let Some(h) = art.turret[owner][k].clone() {
                commands.spawn((
                    Mesh3d(h),
                    MeshMaterial3d(m.clone()),
                    Transform::from_xyz(0.0, -1000.0, 0.0),
                    Visibility::Hidden,
                    TurretMark(i),
                ));
            }
        }
    }

    let eye = view.cam.eye();
    let t = view.cam.target;
    let side = view.side;
    let cam = commands
        .spawn((
            Camera3d::default(),
            Camera {
                // Небо за краем мира на случай, когда атмосферы нет:
                // её плагин требует вычислительных шейдеров и на машине
                // без них молча не загрузится.
                clear_color: ClearColorConfig::Custom(srgb(look::SKY_COLOUR_SRGB)),
                ..default()
            },
            // Тонмаппинг тут не украшение: свет задан в люксах, и без
            // него всё ярче половины улетает в белое.
            Tonemapping::AcesFitted,
            // Экспозиция идёт в паре с яркостью солнца: 130 000 люкс
            // на входе атмосферы — это не «ярче», а «до фильтрации»
            // (ADR-198).
            Exposure { ev100: sky.ev100 },
            Transform::from_xyz(eye.x, eye.y, eye.z).looking_at(Vec3::new(t.x, t.y, t.z), Vec3::Y),
        ))
        .id();
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
    if fx.sky {
        // Атмосфера считается на камеру: её настройки — это размеры
        // таблиц и число выборок, то есть цена, а не вид.
        commands.entity(cam).insert((
            AtmosphereSettings {
                aerial_view_lut_max_distance: look::HAZE_RANGE_M,
                ..default()
            },
            // Отражения и заполняющий свет **снимаются с настоящего
            // неба**. Это и есть ответ на «руда без карты окружения
            // выходит чёрной» из ADR-175: карта окружения появилась.
            AtmosphereEnvironmentMapLight {
                intensity: look::SKY_IBL,
                size: UVec2::splat(look::SKY_IBL_SIZE),
                ..default()
            },
        ));
        // Планета. Её преобразование — это масштаб сцены: рассеяние
        // считается в метрах, а мир у нас в вокселях, и растянутый
        // воксель даёт видимую глубину (ADR-198).
        let k = 1.0 / sky.haze;
        let medium = mediums.add(ScatteringMedium::earth(256, 256));
        let atmo = commands.spawn(Atmosphere::earth(medium)).id();
        // Отдельной вставкой, а не полем набора: у `Atmosphere` есть
        // хук на добавление, который ставит планету под сцену сам,
        // и спорить с ним в одном наборе — значит зависеть от порядка.
        // Компонента `Transform` здесь нет намеренно: была бы —
        // распространение преобразований переписало бы это значение.
        commands.entity(atmo).insert(GlobalTransform::from(
            Transform::from_scale(Vec3::splat(k))
                .with_translation(Vec3::new(0.0, ground - EARTH_RADIUS_M * k, 0.0)),
        ));
    } else {
        // Старая картинка целиком, чтобы было с чем сравнивать: плоская
        // заливка неба и линейный туман того же цвета.
        commands.entity(cam).insert(DistanceFog {
            color: srgb(look::SKY_COLOUR_SRGB),
            falloff: FogFalloff::Linear { start: side * 0.35, end: side * 1.15 },
            ..default()
        });
    }

    // Солнце: белое, с каскадными тенями и видимым диском. Тень здесь
    // не эффект — без неё рельеф читается как плоская заливка, а юнит
    // не стоит на земле, а висит над ней.
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
        // Диск солнца рисует атмосфера; без неё компонент не значит
        // ничего и потому ставится всегда.
        SunDisk {
            angular_size: look::SUN_DISK_SIZE,
            intensity: look::SUN_DISK_INTENSITY,
        },
        Transform::default()
            .looking_to(Vec3::from(look::sun_dir_at(sky.elevation, sky.azimuth)), Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: fx.cascades.max(1),
            minimum_distance: 1.0,
            first_cascade_far_bound: side * look::SHADOW_FIRST_FRAC,
            maximum_distance: side * look::SHADOW_MAX_FRAC,
            overlap_proportion: 0.2,
        }
        .build(),
    ));
    commands.insert_resource(art);
}

/// Радиус планеты у `Atmosphere::earth`, метры. Повторён здесь, потому
/// что Bevy держит его приватной константой, а вынести планету под
/// сцену без него нельзя.
const EARTH_RADIUS_M: f32 = 6_360_000.0;

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

fn buf_to_mesh(b: &MeshBuf, uv: bool) -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.pos.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm.clone())
        .with_inserted_indices(Indices::U32(b.idx.clone()));
    if !b.col.is_empty() && b.col.len() == b.pos.len() {
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, b.col.clone());
    }
    // UV кладутся только когда плитка включена: атрибут задаёт ключ
    // конвейера, и без него шейдер не читает текстуру вовсе.
    if uv && b.uv.len() == b.pos.len() {
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, b.uv.clone());
        // Касательные — вместе с UV и только с ними: карта нормалей
        // без них не читается вовсе, а без карты они лишний байт
        // на вершину (ADR-211).
        if b.tan.len() == b.pos.len() {
            m.insert_attribute(Mesh::ATTRIBUTE_TANGENT, b.tan.clone());
        }
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
    mut seen: ResMut<Seen>,
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
                let m = buf_to_mesh(&bufs[k], fx.chamfer);
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
    // Положение — **с дробной частью**. Целая часть меняется раз
    // в несколько тиков, и корпус от этого переставлялся скачком
    // на полметра пять раз в секунду: рывки, которые владелец видел,
    // наполовину были отсюда (ADR-220).
    let at = |i: usize| {
        Vec3::new(
            u.x[i] as f32 + u.fx[i].to_f64_lossy() as f32,
            u.y[i] as f32,
            u.z[i] as f32 + u.fz[i].to_f64_lossy() as f32,
        )
    };
    let alive = |i: usize| i < u.len() && u.alive[i];
    // Курс — из симуляции, а не из дробной части положения.
    let hull_yaw = |i: usize| Some(yaw_of(u.head[i]));
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
    //
    // Трассер вытянут **по скорости**, а не по осям: снаряд за кадр
    // проходит несколько вокселей, и кубик в его положении читается
    // пунктиром из точек. Штрих обязан перекрывать разрыв между
    // кадрами, оттого он и длиннее пути за тик.
    for e in shots.iter() {
        commands.entity(e).despawn();
    }
    {
        let s = &sim.r.world.shots;
        for &id in s.live() {
            let i = id as usize;
            let p = Vec3::new(
                s.x[i] as f32 + s.fx[i].to_f64_lossy() as f32,
                s.y[i] as f32 + s.fy[i].to_f64_lossy() as f32,
                s.z[i] as f32 + s.fz[i].to_f64_lossy() as f32,
            );
            let v = Vec3::new(
                s.vx[i].to_f64_lossy() as f32,
                s.vy[i].to_f64_lossy() as f32,
                s.vz[i].to_f64_lossy() as f32,
            );
            let speed = v.length();
            let (dir, len) = if speed > 1e-4 {
                (v / speed, speed * look::TRACER_STRETCH)
            } else {
                (Vec3::Y, look::TRACER_W * 2.0)
            };
            commands.spawn((
                Mesh3d(art.bar.clone()),
                MeshMaterial3d(art.tracer[usize::from(s.owner[i] == 1)].clone()),
                Transform::from_translation(p)
                    .with_rotation(Quat::from_rotation_arc(Vec3::Y, dir))
                    .with_scale(Vec3::new(look::TRACER_W, len, look::TRACER_W)),
                ShotMark,
            ));
        }
    }

    // Дульная вспышка. Выстрела среди событий нет, поэтому она ставится
    // по **появлению** снаряда: номер, которого в прошлом кадре
    // не было, — это новый выстрел. Свободный список у снарядов
    // (`shot.rs`) означает, что номер может вернуться, и различить
    // «тот же» от «новый на том же месте» иначе нельзя.
    {
        let s = &sim.r.world.shots;
        // Обе половины растут вместе: меняются они местами, и короткая
        // после первого же обмена становится текущей.
        if seen.live.len() < s.len() {
            seen.live.resize(s.len(), false);
            seen.was.resize(s.len(), false);
        }
        for f in seen.live.iter_mut() {
            *f = false;
        }
        for &id in s.live() {
            let i = id as usize;
            seen.live[i] = true;
            if seen.was[i] {
                continue;
            }
            let p = Vec3::new(
                s.x[i] as f32 + s.fx[i].to_f64_lossy() as f32,
                s.y[i] as f32 + s.fy[i].to_f64_lossy() as f32,
                s.z[i] as f32 + s.fz[i].to_f64_lossy() as f32,
            );
            commands.spawn((
                Mesh3d(art.ball.clone()),
                MeshMaterial3d(art.muzzle[0].clone()),
                Transform::from_translation(p).with_scale(Vec3::splat(look::MUZZLE_R)),
                Fade { age: 0, life: look::MUZZLE_FRAMES, kind: Kind::Muzzle, size: look::MUZZLE_R },
            ));
            commands.spawn((
                PointLight {
                    color: srgb(look::MUZZLE_SRGB),
                    intensity: look::MUZZLE_LIGHT_CD,
                    range: 14.0,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_translation(p),
                Fade { age: 0, life: 2, kind: Kind::Light, size: look::MUZZLE_LIGHT_CD },
            ));
        }
        let Seen { was, live } = &mut *seen;
        core::mem::swap(was, live);
    }

    for ev in sim.r.world.events() {
        match *ev {
            SimEvent::Blast { x, y, z, radius } => {
                // Радиус воронки берётся у события, а не выдумывается:
                // прежний шар был одного размера при любой воронке,
                // и по картинке нельзя было отличить царапину от ямы.
                let r = (radius as f32).max(1.0);
                // Центр воронки — **в земле**, и шар, поставленный
                // туда, закопан целиком (`look::BLAST_LIFT`).
                let p = Vec3::new(x as f32, y as f32 + r * look::BLAST_LIFT, z as f32);
                commands.spawn((
                    Mesh3d(art.ball.clone()),
                    MeshMaterial3d(art.boom[0].clone()),
                    Transform::from_translation(p).with_scale(Vec3::splat(r * look::BLAST_R0)),
                    Fade { age: 0, life: look::BLAST_FRAMES, kind: Kind::Blast, size: r },
                ));
                commands.spawn((
                    PointLight {
                        color: srgb(look::BLAST_LIGHT_SRGB),
                        intensity: look::BLAST_LIGHT_CD * r,
                        range: r * 10.0,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    Transform::from_translation(p),
                    Fade {
                        age: 0,
                        life: look::BLAST_LIGHT_FRAMES,
                        kind: Kind::Light,
                        size: look::BLAST_LIGHT_CD * r,
                    },
                ));
                spawn_debris(&mut commands, &art.chip, &art.dust, p, r, look::DEBRIS_BLAST, (x, y, z));
            }
            SimEvent::Beam { x0, y0, z0, x1, y1, z1, owner } => {
                // Луч — вытянутый кубик от стрелка к цели, гаснущий
                // за несколько кадров: событие приходит один раз,
                // а увидеть его надо дольше.
                let a = Vec3::new(x0 as f32, y0 as f32 + look::BEAM_LIFT, z0 as f32);
                let b = Vec3::new(x1 as f32, y1 as f32 + look::BEAM_LIFT, z1 as f32);
                let d = b - a;
                let len = d.length().max(0.001);
                let side = usize::from(owner == 1);
                commands.spawn((
                    Mesh3d(art.bar.clone()),
                    MeshMaterial3d(art.ray[side][0].clone()),
                    Transform::from_translation((a + b) * 0.5)
                        .with_rotation(Quat::from_rotation_arc(Vec3::Y, d / len))
                        .with_scale(Vec3::new(look::BEAM_W, len, look::BEAM_W)),
                    Fade { age: 0, life: look::BEAM_FRAMES, kind: Kind::Beam, size: side as f32 },
                ));
                // Вспышка у дула луча — та же, что у ствола: стреляет
                // и тот и другой, и видно это одинаково.
                commands.spawn((
                    Mesh3d(art.ball.clone()),
                    MeshMaterial3d(art.muzzle[0].clone()),
                    Transform::from_translation(a).with_scale(Vec3::splat(look::MUZZLE_R)),
                    Fade { age: 0, life: look::MUZZLE_FRAMES, kind: Kind::Muzzle, size: look::MUZZLE_R },
                ));
            }
            SimEvent::Death { x, y, z, .. } => {
                // Смерть событием была всегда и не показывалась ничем:
                // юнит просто исчезал. Теперь от него остаются обломки.
                let p = Vec3::new(x as f32, y as f32, z as f32);
                spawn_debris(&mut commands, &art.chip, &art.dust, p, 2.0, look::DEBRIS_DEATH, (x, y, z));
            }
        }
    }
}

/// Разлёт обломков из точки. Направление — из хеша координат, а не
/// из `rand`: два прогона одной сцены обязаны дать одинаковый снимок.
fn spawn_debris(
    commands: &mut Commands,
    chip: &Handle<Mesh>,
    dust: &Handle<StandardMaterial>,
    at: Vec3,
    r: f32,
    n: usize,
    key: (i32, i32, i32),
) {
    for k in 0..n {
        let h = scatter(key.0, key.1, key.2, k);
        let ang = frac(h) * core::f32::consts::TAU;
        let up = look::DEBRIS_UP * (0.55 + frac(h >> 7));
        let out = look::DEBRIS_OUT * (0.4 + frac(h >> 13)) * r.min(4.0);
        let vel = Vec3::new(ang.cos() * out, up, ang.sin() * out);
        commands.spawn((
            Mesh3d(chip.clone()),
            MeshMaterial3d(dust.clone()),
            Transform::from_translation(at + vel * 2.0),
            Debris {
                vel,
                spin: Vec3::new(
                    frac(h >> 3) - 0.5,
                    frac(h >> 9) - 0.5,
                    frac(h >> 17) - 0.5,
                ) * 0.35,
                age: 0,
            },
        ));
    }
}

/// Всё, что стареет, кроме обломков: у каждого своё — либо свет,
/// либо материал, либо и то и другое.
type FadeQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Fade,
        &'static mut Transform,
        Option<&'static mut PointLight>,
        Option<&'static mut MeshMaterial3d<StandardMaterial>>,
    ),
    Without<Debris>,
>;

/// Старение эффектов: вспышки, лучи, свет и обломки.
///
/// Одна система на всё, потому что стареют они одинаково — по кадру
/// за кадр, — а отличаются только тем, что делают с возрастом.
fn effects(
    mut commands: Commands,
    art: Res<Art>,
    mut fades: FadeQuery,
    mut debris: Query<(Entity, &mut Debris, &mut Transform)>,
) {
    for (e, mut f, mut tr, light, mat) in fades.iter_mut() {
        if f.age >= f.life {
            commands.entity(e).despawn();
            continue;
        }
        let t = f.age as f32 / f.life as f32;
        match f.kind {
            Kind::Blast => {
                // Раздувается быстро в начале и медленно в конце:
                // корень, а не прямая. Ударная волна так и выглядит.
                let k = look::BLAST_R0 + (look::BLAST_R1 - look::BLAST_R0) * t.sqrt();
                tr.scale = Vec3::splat(f.size * k);
                if let Some(mut m) = mat {
                    let i = (f.age as usize).min(art.boom.len() - 1);
                    m.0 = art.boom[i].clone();
                }
            }
            Kind::Beam => {
                if let Some(mut m) = mat {
                    let side = f.size as usize;
                    let i = (f.age as usize).min(art.ray[side].len() - 1);
                    m.0 = art.ray[side][i].clone();
                }
            }
            Kind::Muzzle => {
                tr.scale = Vec3::splat(f.size * (1.0 + t * 0.8));
                if let Some(mut m) = mat {
                    let i = (f.age as usize).min(art.muzzle.len() - 1);
                    m.0 = art.muzzle[i].clone();
                }
            }
            Kind::Light => {
                if let Some(mut l) = light {
                    // Свет гаснет по квадрату: линейный оставляет
                    // заметную ступеньку на последнем кадре.
                    l.intensity = f.size * (1.0 - t) * (1.0 - t);
                }
            }
        }
        f.age += 1;
    }

    for (e, mut d, mut tr) in debris.iter_mut() {
        if d.age >= look::DEBRIS_FRAMES {
            commands.entity(e).despawn();
            continue;
        }
        d.vel.y -= look::DEBRIS_G;
        let v = d.vel;
        tr.translation += v;
        tr.rotation *= Quat::from_euler(EulerRot::XYZ, d.spin.x, d.spin.y, d.spin.z);
        // Последнюю треть жизни обломок оседает: осколок, зависший
        // в воздухе и исчезнувший, заметнее, чем его отсутствие.
        let t = d.age as f32 / look::DEBRIS_FRAMES as f32;
        if t > 0.66 {
            tr.scale = Vec3::splat(((1.0 - t) / 0.34).max(0.01));
        }
        d.age += 1;
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
