//! Окно игры: у игрока есть руки.
//!
//! ```text
//! play --units 240 --side 384
//! ```
//!
//! **Камера** — та же, что у `battle` и `view`, и нарочно: своё
//! управление в каждом окне это способ забыть, как оно устроено.
//! **WASD** — панорама, **Q/E** — поворот, **R/F** — приближение,
//! **Esc** — выход.
//!
//! **Руки:**
//!
//! - **ЛКМ** по юниту — выделить; **ЛКМ с протяжкой** — рамкой;
//!   **Shift** — добавить к выделенному. Выделяются только свои
//!   (сторона 0).
//! - **ПКМ** по земле — идти туда. **ПКМ по врагу** — атаковать его.
//!   **Ctrl+ПКМ** по породе — копать шахту.
//! - **H** — стоять, **G** — снять приказ (юнит возвращается под поле
//!   своей стороны), **Пробел** — пауза.
//! - **B/N/T** — выбрать, что строить (склад, завод, турель), затем
//!   **Ctrl+ПКМ** по месту — поставить; **Esc** — передумать.
//!   Строит выделенный краулер, и строит он из запаса стороны.
//! - **1/2/3** — заказать выделенному заводу танк, краулер, шагоход;
//!   **X** — снять последний заказ.
//!
//! **Петля** (ADR-230): краулер копает → бак наполняется → краулер сам
//! везёт его на ближайший свой склад → у стороны растёт запас →
//! из запаса строятся постройки и собираются машины. Запас, потолок
//! и силы печатаются в терминал раз в секунду: строка поверх кадра
//! требует `bevy_ui`, которого нет в пине.
//!
//! **Рендер читает состояние и никогда в него не пишет** (`CLAUDE.md`,
//! границы слоёв). Щелчок мышью не двигает юнита: он превращается
//! в приказ, приказ ложится в очередь мира, и применяет его фаза
//! `ApplyCommands` — у всех клиентов lockstep одинаково (ADR-195).
//! Отсюда и пауза: приказ, отданный на паузе, ждёт тика, а не
//! исполняется мгновенно, и это видно.
//!
//! Всё, что окно делает с симуляцией, — два вызова: `tick`
//! и `push_order`. Прочее — чтение.

use std::time::Instant;

use bevy::anti_alias::fxaa::Fxaa;
use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::mesh::Indices;
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
use render::tex::tile_image;
use render::{yaw_of, yaw_to, Camera as RtsCamera, ChunkGrid, V3};
use sim_core::{Order, Reference, SimEvent, NO_TARGET};

/// Сторона игрока. Ноль нарочно: эталонный сценарий вооружает
/// нулевую сторону болтом, и она же стоит слева.
const PLAYER: u8 = 0;

/// Насколько близко к юниту должен быть щелчок, чтобы попасть по нему,
/// в пикселях экрана. Не игровое число: это толщина пальца, а не
/// свойство мира, и в `data/*.ron` ему делать нечего.
const PICK_PX: f32 = 28.0;

/// Дальше этого протяжка считается рамкой, а не щелчком.
const DRAG_PX: f32 = 6.0;

/// Колец выделения рисуется не больше этого. Выделить можно всю армию,
/// а кольцо — это тридцать два отрезка; пять тысяч колец кладут кадр
/// ради того, чего на экране всё равно не разобрать.
const RINGS_MAX: usize = 512;

/// Сколько живёт отметка приказа, секунд. В секундах, а не в кадрах:
/// кадры у окна без вертикальной синхронизации идут втрое чаще, чем
/// у окна с ней, и отметка «на двадцать четыре кадра» гасла бы
/// по-разному на одной и той же машине.
const MARK_LIFE: f32 = 1.2;

/// Цвета оверлея.
///
/// Это **не палитра сцены** (ADR-175): оверлей не освещается, не зависит
/// от материала и в кадре не участвует — он поверх кадра. Свет, небо
/// и материалы по-прежнему живут только в `render::look`.
const SEL_COLOUR: Color = Color::srgb(0.35, 1.0, 0.45);
const RECT_COLOUR: Color = Color::srgb(0.6, 1.0, 0.7);
const MOVE_COLOUR: Color = Color::srgb(0.35, 1.0, 0.45);
const ATTACK_COLOUR: Color = Color::srgb(1.0, 0.35, 0.3);
const DIG_COLOUR: Color = Color::srgb(1.0, 0.8, 0.25);
const BUILD_COLOUR: Color = Color::srgb(0.45, 0.75, 1.0);

// Ребро воксела тела здесь не задаётся: его называет **само тело**
// (`UnitBody::mm`), а масштаб выводится делением на мировое ребро
// `voxel::unit::WORLD_MM`. Постоянная «ровно половина» тихо растянула
// бы машину, у которой ребро другое (ADR-219).

/// Симуляция и всё, что нужно, чтобы её показать.
#[derive(Resource)]
struct Sim {
    r: Reference,
    mesh: MeshLayer,
    grid: ChunkGrid,
    chunks: Vec<[Option<Handle<Mesh>>; look::CLASSES]>,
    bufs: [MeshBuf; look::CLASSES],
    painter: Painter,
    /// Во сколько раз симуляция быстрее реального времени. Ноль — пауза.
    speed: u32,
    /// Скорость до паузы: пробел обязан возвращать ту, что была.
    was: u32,
    /// Недотиканное время, секунды.
    ///
    /// **Тик привязан к часам, а не к кадру.** У `battle` тик равен
    /// кадру нарочно: там меряют, и два прогона обязаны показывать
    /// одно и то же состояние. Здесь играют, и кадры идут во сколько
    /// угодно раз чаще тридцати герц — на этой машине выходило больше
    /// ста шестидесяти. Тик на кадр означал бы игру впятеро быстрее
    /// на быстрой машине и вдвое медленнее на медленной.
    acc: f32,
}

/// Длина тика в секундах. Частота — из симуляции, а не своя:
/// скорость юнита задана в клетках в секунду и делится там же.
const DT: f32 = 1.0 / sim_core::TICKS_PER_SECOND as f32;

/// Сколько тиков окно догоняет за один кадр. Провал кадра не должен
/// превращаться в лавину тиков, которая провалит следующий.
const CATCH_UP: u32 = 8;

/// Камера между кадрами.
#[derive(Resource)]
struct View {
    cam: RtsCamera,
    side: f32,
    /// Ребро воксела тела юнита, миллиметры.
    body_mm: u16,
}

/// Что выделено и что с этим происходит.
#[derive(Resource, Default)]
struct Sel {
    /// Номера выделенных юнитов. Номера, а не ссылки: юнит гибнет,
    /// слот переиспользуется, и проверять живость всё равно придётся.
    picked: Vec<u32>,
    /// Где нажали левую кнопку — начало рамки.
    drag: Option<Vec2>,
    /// Отметка отданного приказа: точка, цвет и сколько ей жить.
    mark: Option<(Vec3, Color, f32)>,
    /// Что игрок собрался строить. `None` — ничего, и тогда Ctrl+ПКМ
    /// означает «копать», как раньше.
    build: Option<u16>,
}

/// Снимок и выход по расписанию — чтобы окно можно было запускать
/// из терминала и получать картинку без рук.
#[derive(Resource)]
struct Shot {
    path: Option<String>,
    after: f32,
    frames: u64,
    seen: u64,
    done: bool,
    /// Показ без рук: на этом кадре окно само выделяет своих и отдаёт
    /// им приказ. Существует ради проверки — мышь из терминала не
    /// нажать, а увидеть кольца и отметку приказа надо.
    demo: u64,
}

#[derive(Component)]
struct Chunk(usize);

#[derive(Component)]
struct UnitMark(usize);

#[derive(Component)]
struct TurretMark(usize);

/// Какой вид тела нарисован в слоте сейчас.
///
/// Слот переиспользуется свободным списком, и в место погибшего танка
/// встаёт склад. Без этой записи склад остался бы нарисован танком —
/// молча, до конца партии.
#[derive(Resource, Default)]
struct Shown {
    /// Вид тела и сторона на слот; `None` — сущностей у слота нет.
    at: Vec<Option<(usize, u8)>>,
}

#[derive(Component)]
struct ShotMark;

#[derive(Component)]
struct BlastMark(u32);

/// Общие меши и материалы.
#[derive(Resource)]
struct Art {
    /// Меш корпуса **по отделке**: сталь, стекло и резина идут
    /// разными материалами, а материал у меша один (ADR-211).
    /// Пустая отделка ручки не получает и сущности не заводит.
    ///
    /// Внешний индекс — **вид тела**, а не сторона: с экономикой
    /// на карте стоят завод, склад и буровая, и рисовать их танком
    /// значило бы, что игрок не видит, что построил (ADR-230).
    /// Внутренний — сторона: цвет несут вершины.
    hull: Vec<[[Option<Handle<Mesh>>; look::FINISHES]; 2]>,
    turret: Vec<[[Option<Handle<Mesh>>; look::FINISHES]; 2]>,
    shot: Handle<Mesh>,
    blast: Handle<Mesh>,
    beam: Handle<Mesh>,
    /// Материал отделки. У сторон он **общий**: цвет несут вершины,
    /// а сталь остаётся сталью, какого бы цвета ни был корпус.
    finish: [Handle<StandardMaterial>; look::FINISHES],
    tracer: Handle<StandardMaterial>,
    ray: [Handle<StandardMaterial>; 2],
    boom: Handle<StandardMaterial>,
    ground: [Handle<StandardMaterial>; look::CLASSES],
}

fn main() {
    // **Шесть машин, по три на сторону.** Пока подбирается вид и
    // управление, толпа мешает: на двухстах сорока не разобрать
    // ни силуэта, ни того, кого выделили и куда он пошёл. Нагрузку
    // меряют гейт и D-7, а не это окно; `--units` поднимает обратно.
    let mut units = 6usize;
    let mut seed = 42u64;
    // Карта под базу, а не под шесть машин: **постройки крупные**.
    // Завод — 24 метра, то есть 240 мировых вокселей при ребре 10 см,
    // и на карте в 128 вокселей он один накрывает её целиком. Пятьсот
    // двенадцать — это полсотни метров: база с заводом, складом
    // и забоем помещается, и вторая тоже.
    let mut side = 512u32;
    // Размах рельефа **не задаётся здесь числом**: его считает
    // `Params::sized` от высоты мира. Своё число у сцены уже дважды
    // расходилось с размером карты и давало плоский блин (ADR-157,
    // ADR-166), а третий раз — просто устаревало (ADR-221). Ключ
    // `--relief` по-прежнему перекрывает расчёт.
    let mut body_mm = 0u16;
    let mut relief = 0u32;
    let mut speed = 1u32;
    let mut shot: Option<String> = None;
    let mut after = 8.0f32;
    let mut frames = 0u64;
    let mut demo = 0u64;
    let (mut yaw, mut pitch, mut dist) = (0.7f32, 0.75f32, 0.45f32);
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--units" => units = v.parse().unwrap_or(units),
            "--side" => side = v.parse().unwrap_or(side),
            "--relief" => relief = v.parse().unwrap_or(relief),
            "--mm" => body_mm = v.parse().unwrap_or(body_mm),
            "--seed" => seed = v.parse().unwrap_or(seed),
            // Множитель реального времени, а не тиков на кадр:
            // единица — тридцать тиков в секунду.
            "--speed" => speed = v.parse().unwrap_or(speed),
            "--shot" => shot = Some(v.clone()),
            "--shot-after" => after = v.parse().unwrap_or(after),
            "--frames" => frames = v.parse().unwrap_or(frames),
            "--demo" => demo = v.parse().unwrap_or(demo),
            "--yaw" => yaw = v.parse().unwrap_or(yaw),
            "--pitch" => pitch = v.parse().unwrap_or(pitch),
            "--dist" => dist = v.parse().unwrap_or(dist),
            "--help" => {
                println!(
                    "play [--units N] [--side N] [--relief N] [--seed N] [--mm ребро-тела-мм] [--speed множитель] \
[--shot файл] [--shot-after сек] [--frames N] [--demo кадр] [--yaw] [--pitch] [--dist]"
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
    let kinds = {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/kinds.ron");
        let path = if here.exists() { here } else { std::path::PathBuf::from("data/kinds.ron") };
        match modding::kinds::load(&path, sim_core::TICKS_PER_SECOND as u32) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("{e}");
                return;
            }
        }
    };
    let mats = {
        let here =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/materials.ron");
        let path =
            if here.exists() { here } else { std::path::PathBuf::from("data/materials.ron") };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                return;
            }
        };
        match modding::data::materials_from_ron(&text) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                return;
            }
        }
    };
    let mut params = worldgen::Params::sized(side, side / 2, side);
    if relief > 0 {
        params.relief = relief;
    }
    let mut r = Reference::with_params(seed, units, tuning, weapons, params);
    // **Экономика включается здесь, а не в эталонном сценарии.**
    // Гейт её не ведёт, и его хеш от неё не сдвигается ни на бит
    // (ADR-230): того, чего нет, в хеше нет.
    r.world.attach_economy(kinds, mats);
    found_bases(&mut r.world, side as i32);
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
    println!(
        "ЛКМ — выделить, протяжка — рамка, Shift — добавить; \
ПКМ — идти или атаковать, Ctrl+ПКМ — копать; H — стоять, G — снять приказ, \
Пробел — пауза; WASD/QE/RF — камера, Esc — выход"
    );
    println!(
        "B/N/T — выбрать склад, завод, турель и поставить Ctrl+ПКМ (Esc — передумать); \
1/2/3 — заказать выделенному заводу танк, краулер, шагоход; X — снять заказ"
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
                title: "rts-vox".into(),
                present_mode: PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(DirectionalLightShadowMap { size: look::scene().shadow_map })
        .insert_resource(GlobalAmbientLight {
            color: srgb(look::scene().ambient_srgb),
            brightness: look::scene().ambient_brightness,
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
            was: speed.max(1),
            acc: 0.0,
        })
        .insert_resource(View { cam, side, body_mm })
        .insert_resource(Sel::default())
        .insert_resource(Shot { path: shot, after, frames, seen: 0, done: false, demo })
        .insert_resource(Shown::default())
        .insert_resource(Hud::default())
        .add_systems(Startup, setup)
        .add_systems(Update, (step, dress, control, hands, overlay, hud, cull, snap))
        .run();
}

fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

/// Номера типов из `data/kinds.ron`. Явные, а не места в реестре
/// (ADR-028): место зависит от того, какие типы загружены, и мод,
/// добавивший свой, молча переставил бы все.
const KIND_TANK: u16 = 1;
const KIND_CRAWLER: u16 = 2;
const KIND_WALKER: u16 = 3;
const KIND_FACTORY: u16 = 10;
const KIND_DEPOT: u16 = 11;
const KIND_TURRET: u16 = 12;

/// База каждой стороны: завод, склад и краулеры.
///
/// Первый завод стоит готовым **нарочно**: строить его не из чего,
/// а партия, которая не может начаться, — не партия. Всё остальное
/// игрок строит сам.
fn found_bases(w: &mut sim_core::World, side: i32) {
    let margin = (side / 6).clamp(8, 96);
    let bases = [(margin, margin), (side - margin, side - margin)];
    for (owner, &(bx, bz)) in bases.iter().enumerate() {
        let o = owner as u8;
        w.spawn_kind(bx, bz, o, KIND_FACTORY);
        let d = (side / 20).max(6);
        w.spawn_kind((bx + d).clamp(0, side - 1), bz, o, KIND_DEPOT);
        for k in 0..3i32 {
            let x = (bx - d + k * d).clamp(0, side - 1);
            let Some(id) = w.spawn_kind(x, (bz + d).clamp(0, side - 1), o, KIND_CRAWLER) else {
                continue;
            };
            // Краулер ждёт приказа у базы, а не марширует на врага:
            // без приказа юнит идёт по полю своей стороны, а копателю
            // там делать нечего.
            //
            // Через **очередь приказов**, а не записью в состояние:
            // рендер читает состояние и никогда в него не пишет
            // (`CLAUDE.md`, границы слоёв). Расстановка — тот же
            // канал, что и щелчок мыши.
            w.push_order(sim_core::Order::Hold { unit: id });
        }
    }
    w.units.refresh_live();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut gizmo_cfg: ResMut<GizmoConfigStore>,
    mut sim: ResMut<Sim>,
    view: Res<View>,
) {
    // Оверлей рисуется **поверх** рельефа. Кольцо под юнитом за холмом
    // — это кольцо, которого не видно, а выделение обязано читаться
    // всегда: иначе игрок не знает, кому отдаёт приказ.
    {
        let (cfg, _) = gizmo_cfg.config_mut::<DefaultGizmoConfigGroup>();
        cfg.depth_bias = -1.0;
        cfg.line.width = 2.5;
    }

    // Пересэмплирование **не украшение**: тело хранит своё ребро
    // в миллиметрах, и мешер ставит воксел в мировых единицах,
    // а не в клетках. Отказ пересэмплировать — не повод рисовать
    // не то: тело остаётся в прежнем масштабе и говорит об этом.
    let mm = view.body_mm;
    // **Ноль значит «как нарисовано»**, и это умолчание: тело знает своё
    // ребро само (ADR-219). Ключ `--mm` пересэмплирует принудительно —
    // он остался, чтобы сравнивать разрешения не по памяти, а рядом.
    //
    // Тела берутся **по именам из данных**: `data/kinds.ron` говорит,
    // какое тело у типа, а `voxel::roster::all()` его даёт. Угадывать
    // тело по номеру стороны нельзя — с экономикой на карте стоят
    // завод, склад и буровая (ADR-230).
    //
    // Два последних места в списке — запасные: ими рисуются юниты
    // **без типа**, которых ставит эталонная расстановка. Они идут
    // по стороне, как раньше.
    let roster = voxel::roster::all();
    let look_names: Vec<String> = {
        let e = sim.r.world.econ.as_ref().expect("экономика");
        e.kinds.all().iter().map(|d| d.body.clone()).collect()
    };
    let mut bodies: Vec<voxel::unit::UnitBody> = Vec::new();
    for name in &look_names {
        let b = roster
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| {
                eprintln!("тела «{name}» нет в наборе — рисуем танком");
                voxel::unit::tank()
            });
        bodies.push(b);
    }
    bodies.push(voxel::unit::tank());
    bodies.push(voxel::unit::strider());
    let bodies: Vec<voxel::unit::UnitBody> = bodies
        .into_iter()
        .map(|b| {
            if mm == 0 || mm == b.mm {
                return b;
            }
            match b.resampled(mm) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("тело в {mm} мм не вышло: {}", e.why());
                    b
                }
            }
        })
        .collect();
    let mut ubufs: [MeshBuf; look::FINISHES] = std::array::from_fn(|_| MeshBuf::default());
    let mut unit_mesh = |body: &voxel::unit::UnitBody,
                         tint: [f32; 3],
                         part: Slice,
                         meshes: &mut Assets<Mesh>|
     -> [Option<Handle<Mesh>>; look::FINISHES] {
        for b in ubufs.iter_mut() {
            b.clear();
        }
        // Разворот **с фаской** (ADR-206): у тела юнита меш строится
        // один раз на тип, и геометрическое скругление там дешевле
        // шейдерного и честнее — срезается силуэт, а не только свет.
        //
        // `Slice` переводится в диапазон слоёв: башня и корпус
        // вращаются врозь, поэтому и рисуются врозь.
        let ty = body.turret_y as usize;
        let layers = match part {
            Slice::All => (0, usize::MAX),
            Slice::Hull if ty == 0 => (0, usize::MAX),
            Slice::Hull => (0, ty),
            // Безбашенное тело не даёт башни вовсе: пустой диапазон,
            // а не весь том — иначе корпус нарисовался бы дважды.
            Slice::Turret if ty == 0 => (0, 0),
            Slice::Turret => (ty, usize::MAX),
        };
        render::model::body_to_mesh(
            body,
            tint,
            body.mm as f32 / voxel::unit::WORLD_MM as f32,
            layers,
            render::model::BEVEL,
            &mut ubufs,
        );
        // Пустая отделка ручки не получает: у безбашенного тела
        // башня пуста целиком, и заводить под неё сущность
        // с нулевым мешем незачем.
        std::array::from_fn(|k| {
            (!ubufs[k].idx.is_empty()).then(|| meshes.add(buf_to_mesh(&ubufs[k])))
        })
    };
    let tm = Instant::now();
    let mut hull: Vec<[[Option<Handle<Mesh>>; look::FINISHES]; 2]> = Vec::new();
    let mut turret: Vec<[[Option<Handle<Mesh>>; look::FINISHES]; 2]> = Vec::new();
    for b in &bodies {
        hull.push([
            unit_mesh(b, look::side_tint()[0], Slice::Hull, &mut meshes),
            unit_mesh(b, look::side_tint()[1], Slice::Hull, &mut meshes),
        ]);
        turret.push([
            unit_mesh(b, look::side_tint()[0], Slice::Turret, &mut meshes),
            unit_mesh(b, look::side_tint()[1], Slice::Turret, &mut meshes),
        ]);
    }
    println!(
        "тел собрано {}, за {:.0} мс",
        bodies.len(),
        tm.elapsed().as_secs_f64() * 1e3
    );
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
    // **У каждого класса своя поверхность**: своя плитка цвета, своя
    // карта нормалей, своя шероховатость. Пока плитка была одна,
    // земля отличалась от камня только оттенком (ADR-211).
    let ground_mat = |c: look::Class, i: &mut Assets<Image>, m: &mut Assets<StandardMaterial>| {
        let (rough, metal) = look::surface(c);
        m.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(i.add(tile_image(look::surface_tile(c), true))),
            normal_map_texture: Some(i.add(tile_image(look::normal_tile(c), false))),
            // UV приходят в вокселях, а плитка накрывает несколько.
            uv_transform: bevy::math::Affine2::from_scale(Vec2::splat(1.0 / look::TILE_VOXELS)),
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
        beam: meshes.add(Cuboid::new(0.5, 1.0, 0.5)),
        finish: look::FINISH_ALL.map(|f| finish_mat(f, &mut materials)),
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
            ground_mat(look::Class::Soil, &mut images, &mut materials),
            ground_mat(look::Class::Rock, &mut images, &mut materials),
            ground_mat(look::Class::Ore, &mut images, &mut materials),
        ],
    };

    // Рельеф целиком — один раз. Дальше пересобираются только грязные
    // чанки: полная пересборка каждый кадр стоит на два порядка больше
    // (ADR-155, ADR-175).
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
        paint(&quads, i, grid, w, painter, bufs);
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

    // Сущности юнитов заводятся **по мере появления юнитов**, а не
    // все разом на старте: завод выпускает машины, и слотов
    // становится больше, чем было. Делает это `dress` каждый кадр.

    let eye = view.cam.eye();
    let t = view.cam.target;
    let side = view.side;
    let cam = commands
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(srgb(look::scene().sky_colour_srgb)),
                ..default()
            },
            Tonemapping::AcesFitted,
            // Дымка — по дальности, а не по доле карты: атмосфера
            // (ADR-198) задаёт её в метрах, и окно берёт ту же
            // величину, чтобы глубина читалась одинаково в обоих.
            DistanceFog {
                color: srgb(look::scene().sky_colour_srgb),
                falloff: FogFalloff::Linear {
                    start: side * 0.35,
                    end: side * 1.6,
                },
                ..default()
            },
            Transform::from_xyz(eye.x, eye.y, eye.z).looking_at(Vec3::new(t.x, t.y, t.z), Vec3::Y),
        ))
        .id();
    commands.entity(cam).insert(Bloom::NATURAL);
    if look::scene().ssao {
        commands
            .entity(cam)
            .insert((Msaa::Off, ScreenSpaceAmbientOcclusion::default(), Fxaa::default()));
    }

    commands.spawn((
        DirectionalLight {
            color: srgb(look::scene().sun_srgb),
            illuminance: look::scene().sun_lux,
            shadow_maps_enabled: look::scene().shadow_cascades > 0,
            shadow_depth_bias: 0.06,
            shadow_normal_bias: 2.0,
            ..default()
        },
        Transform::default().looking_to(Vec3::from(look::sun_dir()).normalize(), Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: look::scene().shadow_cascades.max(1),
            minimum_distance: 1.0,
            first_cascade_far_bound: side * look::scene().shadow_first_frac,
            maximum_distance: side * look::scene().shadow_max_frac,
            overlap_proportion: 0.2,
        }
        .build(),
    ));
    // Заполняющий свет неба: раньше это была вторая лампа с своими
    // константами, теперь их нет — небо считается атмосферой. Окно
    // пока без атмосферы, поэтому подсветка берётся из палитры
    // ambient, а не выдумывается заново (ADR-209).
    commands.insert_resource(GlobalAmbientLight {
        color: srgb(look::scene().ambient_srgb),
        brightness: look::scene().ambient_brightness * 4.0,
        ..default()
    });
    commands.insert_resource(art);
}

/// Развернуть квады чанка в буферы классов, покрасив материалом мира.
fn paint(
    quads: &[meshing::Quad],
    i: usize,
    grid: ChunkGrid,
    w: &voxel::World,
    painter: &mut Painter,
    bufs: &mut [MeshBuf; look::CLASSES],
) {
    for b in bufs.iter_mut() {
        b.clear();
    }
    quads_to_mesh_painted(
        quads,
        [0.0, 0.0, 0.0],
        chunk_at(i, grid.nx, grid.ny),
        {
            // Курсор помнит чанк между выборками: почти все воксели
            // одной грани лежат в одном чанке, и деления на его
            // сторону не нужны вовсе (ADR-227).
            let mut cur = voxel::World::mat_cursor();
            move |x, y, z| w.material_cached(&mut cur, x, y, z)
        },
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

type HullQuery<'w, 's> = Query<
    'w,
    's,
    (&'static UnitMark, &'static mut Transform, &'static mut Visibility),
    (Without<ShotMark>, Without<TurretMark>),
>;

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
    time: Res<Time>,
    art: Res<Art>,
    mut units: HullQuery,
    mut turrets: TurretQuery,
    shots: Query<Entity, With<ShotMark>>,
    blasts: Query<(Entity, &mut BlastMark)>,
) {
    // Часы, а не кадры: сколько тиков задолжали, столько и делаем,
    // но не больше `CATCH_UP` за кадр.
    sim.acc += time.delta_secs() * sim.speed as f32;
    let mut done = 0u32;
    while sim.acc >= DT && done < CATCH_UP {
        // **Тик мира, а не эталонного сценария.** У сценария свои
        // копатели-оснастка и подкрепления струйкой: гейту они нужны,
        // чтобы нагрузка не затухала, а игре мешают — копают мимо
        // учёта и подсыпают машин, которых игрок не заказывал
        // (ADR-230).
        sim.r.world.tick();
        sim.acc -= DT;
        done += 1;
    }
    if sim.acc > DT * CATCH_UP as f32 {
        sim.acc = 0.0;
    }

    {
        let Sim { r, mesh, grid, chunks, bufs, painter, .. } = &mut *sim;
        let grid = *grid;
        let g = r.world.ground().expect("земля");
        let edits: Vec<voxel::Edit> = g.edits().to_vec();
        mesh.apply(&g.world, &edits);
        let dirty: Vec<usize> = mesh.dirty().iter().map(|&i| i as usize).collect();
        let w = &g.world;
        for i in dirty {
            let quads = mesh.quads(i);
            paint(&quads, i, grid, w, painter, bufs);
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
        }
    }

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

    // Недострой растёт **снизу вверх**: доля вложенной материи —
    // это доля высоты тома. Ровно то, что видит игрок под словами
    // «строится воксель за вокселем» (ADR-230).
    let grown = |i: usize| -> f32 {
        let Some(e) = sim.r.world.econ.as_ref() else { return 1.0 };
        let left = u.site[i];
        if left == 0 {
            return 1.0;
        }
        let ki = u.kind[i];
        if ki == sim_core::economy::NO_KIND {
            return 1.0;
        }
        let cost = e.kinds.at(ki as usize).cost.max(1);
        let done = cost.saturating_sub(left);
        // Не ноль: меш нулевой высоты не виден вовсе, а разметку
        // фундамента игрок обязан увидеть сразу.
        (done as f32 / cost as f32).clamp(0.06, 1.0)
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
        tr.scale = Vec3::new(1.0, grown(i), 1.0);
    }
    for (m, mut tr, mut vis) in turrets.iter_mut() {
        let i = m.0;
        // Башня недостроя не показывается: она лежит выше корпуса,
        // и на половине высоты висела бы в воздухе.
        if !alive(i) || u.site[i] != 0 {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        tr.translation = at(i);
        if let Some(y) = aim_yaw(i).or_else(|| hull_yaw(i)) {
            tr.rotation = Quat::from_rotation_y(y);
        }
    }

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

/// Какое тело рисовать в слоте: место типа в реестре либо запасное
/// место по стороне для юнита **без типа**.
fn look_of(w: &sim_core::World, i: usize, kinds: usize) -> usize {
    let k = w.units.kind[i];
    if k == sim_core::economy::NO_KIND {
        kinds + usize::from(w.units.owner[i] == 1)
    } else {
        k as usize
    }
}

/// Завести и переодеть сущности юнитов.
///
/// **Слот переиспользуется**, и в место погибшего танка встаёт склад.
/// Поэтому проверяется не «есть ли сущности», а «те ли они»: иначе
/// склад остался бы нарисован танком молча, до конца партии.
fn dress(
    mut commands: Commands,
    sim: Res<Sim>,
    art: Res<Art>,
    mut shown: ResMut<Shown>,
    hulls: Query<(Entity, &UnitMark)>,
    turrets: Query<(Entity, &TurretMark)>,
) {
    let kinds = sim.r.world.econ.as_ref().map(|e| e.kinds.len()).unwrap_or(0);
    let u = &sim.r.world.units;
    let n = u.len();
    if shown.at.len() < n {
        shown.at.resize(n, None);
    }
    let mut redress: Vec<usize> = Vec::new();
    for i in 0..n {
        let want = if u.alive[i] { Some((look_of(&sim.r.world, i, kinds), u.owner[i])) } else { None };
        if shown.at[i] != want {
            redress.push(i);
        }
    }
    if redress.is_empty() {
        return;
    }
    for (e, m) in hulls.iter() {
        if redress.contains(&m.0) {
            commands.entity(e).despawn();
        }
    }
    for (e, m) in turrets.iter() {
        if redress.contains(&m.0) {
            commands.entity(e).despawn();
        }
    }
    for i in redress {
        let want = if u.alive[i] { Some((look_of(&sim.r.world, i, kinds), u.owner[i])) } else { None };
        shown.at[i] = want;
        let Some((look, owner)) = want else { continue };
        let o = usize::from(owner == 1);
        let Some(h) = art.hull.get(look) else { continue };
        // По сущности на отделку. Системы движения ищут по метке
        // и обновляют их все разом одним и тем же преобразованием:
        // корпус остаётся корпусом, из скольких бы материалов он
        // ни был собран.
        for (k, m) in art.finish.iter().enumerate() {
            if let Some(mesh) = h[o][k].clone() {
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(m.clone()),
                    Transform::from_xyz(0.0, -1000.0, 0.0),
                    Visibility::Hidden,
                    UnitMark(i),
                ));
            }
            if let Some(mesh) = art.turret[look][o][k].clone() {
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(m.clone()),
                    Transform::from_xyz(0.0, -1000.0, 0.0),
                    Visibility::Hidden,
                    TurretMark(i),
                ));
            }
        }
    }
}

/// Камера RTS — та же, что у `battle` и `view`. Плюс пауза.
fn control(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut view: ResMut<View>,
    mut sim: ResMut<Sim>,
    mut cams: Query<&mut Transform, With<Camera3d>>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
        return;
    }
    if keys.just_pressed(KeyCode::Space) {
        if sim.speed == 0 {
            sim.speed = sim.was;
        } else {
            sim.was = sim.speed;
            sim.speed = 0;
        }
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

/// Экранная точка юнита, если он перед камерой.
fn on_screen(cam: &Camera, gt: &GlobalTransform, p: Vec3) -> Option<Vec2> {
    cam.world_to_viewport(gt, p).ok()
}

/// Ближайший к точке щелчка, но не дальше `r` пикселей.
///
/// Ничья разрешается **меньшим номером**, а не порядком обхода:
/// правило то же, что в симуляции (`CLAUDE.md`), и цена ошибки та же —
/// щелчок, выделяющий разных юнитов в одинаковой обстановке.
fn nearest(cands: &[(u32, Vec2)], at: Vec2, r: f32) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    for &(id, p) in cands {
        let d = p.distance_squared(at);
        if d > r * r {
            continue;
        }
        match best {
            Some((bd, bi)) if bd < d || (bd == d && bi <= id) => {}
            _ => best = Some((d, id)),
        }
    }
    best.map(|(_, i)| i)
}

/// Внутри ли точка прямоугольника, заданного двумя углами.
fn in_rect(p: Vec2, a: Vec2, b: Vec2) -> bool {
    let (lo, hi) = (a.min(b), a.max(b));
    p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
}

/// Отрезок параметра, на котором луч внутри ящика `[0, hi]`.
///
/// Марш вне мира бессмыслен: вокселей там нет, а шагов до дальней
/// плоскости — тысячи.
fn box_span(o: Vec3, d: Vec3, hi: Vec3) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0f32, f32::INFINITY);
    for k in 0..3 {
        let (oi, di, hk) = (o[k], d[k], hi[k]);
        if di.abs() < 1e-6 {
            if oi < 0.0 || oi > hk {
                return None;
            }
            continue;
        }
        let (mut a, mut b) = ((0.0 - oi) / di, (hk - oi) / di);
        if a > b {
            core::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
    }
    if t0 > t1 {
        None
    } else {
        Some((t0, t1))
    }
}

/// Первый занятый воксель вдоль луча и последний пустой перед ним.
///
/// Шаг мельче воксела и постоянный: это не DDA разрешения попаданий
/// (тот живёт в симуляции и считает в `Fx`), а наводка мыши — она
/// случается по щелчку, а не тысячами за тик, и цена ей никакая.
fn ray_voxel(w: &voxel::World, o: Vec3, d: Vec3) -> Option<([i32; 3], [i32; 3])> {
    let hi = Vec3::new(w.nx as f32, w.ny as f32, w.nz as f32);
    let (t0, t1) = box_span(o, d, hi)?;
    let mut prev = [-1i32, -1, -1];
    let mut t = t0;
    while t <= t1 {
        let p = o + d * t;
        let c = [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32];
        if c[0] >= 0
            && c[1] >= 0
            && c[2] >= 0
            && w.solid(c[0] as u32, c[1] as u32, c[2] as u32)
        {
            return Some((c, prev));
        }
        prev = c;
        t += 0.4;
    }
    None
}

/// Руки игрока: выделение и приказы.
///
/// **Единственное место, где окно трогает симуляцию, — `push_order`.**
/// Это не запись в состояние, а запрос в отложенную очередь, ту же
/// самую, которой пользуется мод (ADR-195). Применит его симуляция,
/// на своей фазе, одинаково у всех клиентов lockstep.
#[allow(clippy::too_many_arguments)]
fn hands(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    windows: Query<&Window>,
    cams: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut sim: ResMut<Sim>,
    mut sel: ResMut<Sel>,
    shot: Res<Shot>,
) {
    // Выделение чистится от погибших: номер живёт дольше юнита.
    {
        let u = &sim.r.world.units;
        sel.picked
            .retain(|&i| (i as usize) < u.len() && u.alive[i as usize] && u.owner[i as usize] == PLAYER);
    }
    if let Some((_, _, life)) = sel.mark.as_mut() {
        *life -= time.delta_secs();
    }
    if sel.mark.is_some_and(|(_, _, l)| l <= 0.0) {
        sel.mark = None;
    }

    // Показ без рук: выделить своих и послать их в середину карты.
    // Мышь из терминала не нажать, а проверить, что кольца и отметка
    // рисуются, надо.
    if shot.demo != 0 && shot.seen == shot.demo {
        let u = &sim.r.world.units;
        let mine: Vec<u32> = u
            .live()
            .iter()
            .copied()
            .filter(|&i| u.owner[i as usize] == PLAYER)
            .take(40)
            .collect();
        let (ex, _) = sim.r.world.ground().expect("земля").extent();
        // Высота отметки — от того, кто её получил: земля здесь
        // не плоская, а отметка на нулевой высоте оказалась бы
        // под рельефом.
        let gy = mine.first().map(|&i| u.y[i as usize]).unwrap_or(0);
        sel.picked = mine.clone();
        let (gx, gz) = (ex / 2, ex / 2);
        for id in mine {
            let k = id as usize;
            // Копателю — копать, остальным — идти. Мышь из терминала
            // не нажать, а проверить, что приказ доходит до экономики,
            // надо: без этого «материя растёт» проверяется только
            // глазами и только в окне.
            let digs = {
                let w = &sim.r.world;
                let ki = w.units.kind[k];
                ki != sim_core::economy::NO_KIND
                    && w.econ.as_ref().is_some_and(|e| e.kinds.at(ki as usize).dig)
            };
            if digs {
                let (ux, uy, uz) = {
                    let w = &sim.r.world;
                    (w.units.x[k], w.units.y[k], w.units.z[k])
                };
                sim.r.world.push_order(Order::Dig {
                    unit: id,
                    x: ux,
                    y: (uy - 1).max(2),
                    z: uz,
                });
            } else {
                sim.r.world.push_order(Order::MoveTo { unit: id, x: gx, z: gz });
            }
        }
        sel.mark = Some((
            Vec3::new(gx as f32, gy as f32, gz as f32),
            MOVE_COLOUR,
            MARK_LIFE,
        ));
    }

    let Ok(win) = windows.single() else { return };
    let Ok((cam, gt)) = cams.single() else { return };
    let cursor = win.cursor_position();
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    // Стоять и снять приказ — клавишами: они относятся к выделенному,
    // а не к точке под курсором.
    if keys.just_pressed(KeyCode::KeyH) || keys.just_pressed(KeyCode::KeyG) {
        let hold = keys.just_pressed(KeyCode::KeyH);
        let picked = sel.picked.clone();
        for id in picked {
            let o = if hold { Order::Hold { unit: id } } else { Order::Release { unit: id } };
            sim.r.world.push_order(o);
        }
    }

    // Что строить следующим Ctrl+ПКМ. Клавиша только **выбирает**;
    // ставит постройку щелчок по месту — иначе игрок не видит, куда
    // она встанет.
    if keys.just_pressed(KeyCode::KeyB) {
        sel.build = Some(KIND_DEPOT);
    }
    if keys.just_pressed(KeyCode::KeyN) {
        sel.build = Some(KIND_FACTORY);
    }
    if keys.just_pressed(KeyCode::KeyT) {
        sel.build = Some(KIND_TURRET);
    }
    if keys.just_pressed(KeyCode::Escape) {
        sel.build = None;
    }

    // Заказ заводу: цифра относится к выделенным заводам.
    let order_kind = if keys.just_pressed(KeyCode::Digit1) {
        Some(KIND_TANK)
    } else if keys.just_pressed(KeyCode::Digit2) {
        Some(KIND_CRAWLER)
    } else if keys.just_pressed(KeyCode::Digit3) {
        Some(KIND_WALKER)
    } else {
        None
    };
    if let Some(kind) = order_kind {
        let picked = sel.picked.clone();
        for id in picked {
            sim.r.world.push_order(Order::Produce { unit: id, kind });
        }
    }
    if keys.just_pressed(KeyCode::KeyX) {
        let picked = sel.picked.clone();
        for id in picked {
            sim.r.world.push_order(Order::Cancel { unit: id });
        }
    }

    if mouse.just_pressed(MouseButton::Left) {
        sel.drag = cursor;
    }
    if mouse.just_released(MouseButton::Left) {
        let from = sel.drag.take();
        if let (Some(a), Some(b)) = (from, cursor) {
            // Свои на экране: чужих не выделяем вовсе, и проверять
            // это надо здесь, а не в симуляции.
            let mut mine: Vec<(u32, Vec2)> = Vec::new();
            {
                let u = &sim.r.world.units;
                for &i in u.live() {
                    let k = i as usize;
                    if u.owner[k] != PLAYER {
                        continue;
                    }
                    let p = Vec3::new(u.x[k] as f32, u.y[k] as f32 + 1.0, u.z[k] as f32);
                    if let Some(s) = on_screen(cam, gt, p) {
                        mine.push((i, s));
                    }
                }
            }
            let mut hit: Vec<u32> = if a.distance(b) < DRAG_PX {
                nearest(&mine, b, PICK_PX).into_iter().collect()
            } else {
                mine.iter()
                    .filter(|(_, p)| in_rect(*p, a, b))
                    .map(|(i, _)| *i)
                    .collect()
            };
            if shift {
                for i in sel.picked.iter() {
                    if !hit.contains(i) {
                        hit.push(*i);
                    }
                }
            }
            // Порядок — по номеру: список выделенного не должен
            // зависеть от того, как по нему возили мышью.
            hit.sort_unstable();
            sel.picked = hit;
        }
    }

    if !mouse.just_pressed(MouseButton::Right) || sel.picked.is_empty() {
        return;
    }
    let Some(c) = cursor else { return };
    let Ok(ray) = cam.viewport_to_world(gt, c) else { return };
    let (o, d) = (ray.origin, *ray.direction);

    // Враг под курсором — приказ атаковать. Проверяется до земли:
    // юнит стоит на ней, и луч всегда упирается в землю следом.
    let mut foes: Vec<(u32, Vec2)> = Vec::new();
    {
        let u = &sim.r.world.units;
        for &i in u.live() {
            let k = i as usize;
            if u.owner[k] == PLAYER {
                continue;
            }
            let p = Vec3::new(u.x[k] as f32, u.y[k] as f32 + 1.0, u.z[k] as f32);
            if let Some(s) = on_screen(cam, gt, p) {
                foes.push((i, s));
            }
        }
    }
    if let Some(foe) = nearest(&foes, c, PICK_PX) {
        let picked = sel.picked.clone();
        for id in picked {
            sim.r.world.push_order(Order::Attack { unit: id, target: foe });
        }
        let u = &sim.r.world.units;
        let k = foe as usize;
        sel.mark = Some((
            Vec3::new(u.x[k] as f32, u.y[k] as f32, u.z[k] as f32),
            ATTACK_COLOUR,
            MARK_LIFE,
        ));
        return;
    }

    let hit = {
        let g = sim.r.world.ground().expect("земля");
        ray_voxel(&g.world, o, d)
    };
    let Some((solid, empty)) = hit else { return };
    let picked = sel.picked.clone();
    if ctrl {
        if let Some(kind) = sel.build {
            // Строить: место — **пустой** воксель перед породой,
            // то есть поверхность, а не её нутро. Материя не
            // списывается здесь: её льют строители, тик за тиком.
            let p = if empty[0] >= 0 { empty } else { solid };
            for id in picked {
                sim.r.world.push_order(Order::Build { unit: id, kind, x: p[0], z: p[2] });
            }
            sel.build = None;
            sel.mark = Some((
                Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32),
                BUILD_COLOUR,
                MARK_LIFE,
            ));
            return;
        }
        // Копать: приказ называет **занятый** воксель — тот, в который
        // упёрся луч. Копка это экономика, и щёлкают по породе.
        for id in picked {
            sim.r.world.push_order(Order::Dig {
                unit: id,
                x: solid[0],
                y: solid[1],
                z: solid[2],
            });
        }
        sel.mark = Some((
            Vec3::new(solid[0] as f32, solid[1] as f32 + 1.0, solid[2] as f32),
            DIG_COLOUR,
            MARK_LIFE,
        ));
    } else {
        // Идти: точка — **пустой** воксель перед породой, то есть то
        // место, куда юнит встанет, а не то, внутри чего он окажется.
        let p = if empty[0] >= 0 { empty } else { solid };
        for id in picked {
            sim.r.world.push_order(Order::MoveTo { unit: id, x: p[0], z: p[2] });
        }
        sel.mark = Some((
            Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32),
            MOVE_COLOUR,
            MARK_LIFE,
        ));
    }
}

/// Строка состояния между кадрами.
#[derive(Resource, Default)]
struct Hud {
    /// Что напечатали в прошлый раз: строку, которая не изменилась,
    /// печатать снова незачем.
    last: String,
    /// Секунд с прошлой печати.
    since: f32,
}

/// Строка состояния: запас, потолок, силы, исход.
///
/// **Печатается в терминал, а не рисуется в окне.** Настоящая строка
/// поверх кадра требует `bevy_ui` и `bevy_text`, а пин собран
/// с набором `3d`, где их нет: включение — правка зависимостей,
/// а её решает владелец (`CLAUDE.md`). Терминал у этого проекта
/// и так первый: «ни одна подсистема не считается готовой, пока
/// не проходит headless из терминала».
///
/// Раз в секунду и только при изменении: строка, повторяющаяся
/// шестьдесят раз в секунду, — это не отчёт, а шум.
fn hud(sim: Res<Sim>, time: Res<Time>, mut hud: ResMut<Hud>) {
    hud.since += time.delta_secs();
    if hud.since < 1.0 {
        return;
    }
    hud.since = 0.0;
    let w = &sim.r.world;
    let Some(e) = w.econ.as_ref() else { return };
    let mut mine = 0usize;
    let mut foes = 0usize;
    let mut crawlers = 0usize;
    let mut sites = 0usize;
    for &i in w.units.live() {
        let k = i as usize;
        if w.units.owner[k] == PLAYER {
            mine += 1;
            if w.units.site[k] != 0 {
                sites += 1;
            }
            let ki = w.units.kind[k];
            if ki != sim_core::economy::NO_KIND && e.kinds.at(ki as usize).dig {
                crawlers += 1;
            }
        } else {
            foes += 1;
        }
    }
    let queued: usize = e.lines().iter().map(|l| l.queue.len()).sum();
    let out = match w.outcome() {
        sim_core::Outcome::Won(o) if o == PLAYER => "  ПОБЕДА",
        sim_core::Outcome::Won(_) => "  ПОРАЖЕНИЕ",
        sim_core::Outcome::Draw => "  НИЧЬЯ",
        sim_core::Outcome::Fighting => "",
    };
    let line = format!(
        "материя {} / {} | краулеров {crawlers} | строек {sites} | в очереди {queued} \
         | своих {mine} | чужих {foes} | выкопано {} | довезено {}{out}",
        grouped(e.stock(PLAYER)),
        grouped(e.cap(PLAYER)),
        grouped(e.mined),
        grouped(e.delivered),
    );
    if line != hud.last {
        println!("{line}");
        hud.last = line;
    }
}

/// Разрядка больших чисел. Материя считается объёмом на плотность,
/// и числа у неё шестизначные: без разрядки строка не читается.
fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Оверлей: кольца под выделенными, рамка и отметка приказа.
fn overlay(
    mut giz: Gizmos,
    sim: Res<Sim>,
    sel: Res<Sel>,
    windows: Query<&Window>,
    cams: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    let flat = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    let u = &sim.r.world.units;
    let r = (sim.r.world.tuning.separation as f32) * 0.5;
    for &i in sel.picked.iter().take(RINGS_MAX) {
        let k = i as usize;
        if k >= u.len() || !u.alive[k] {
            continue;
        }
        let p = Vec3::new(u.x[k] as f32, u.y[k] as f32 + 0.2, u.z[k] as f32);
        giz.circle(Isometry3d::new(p, flat), r, SEL_COLOUR).resolution(16);
    }

    // Недострой обведён кольцом, и кольцо растёт вместе с ним:
    // «сколько осталось» игрок обязан видеть, не считая вокселей.
    if let Some(e) = sim.r.world.econ.as_ref() {
        for &i in u.live() {
            let k = i as usize;
            if u.site[k] == 0 {
                continue;
            }
            let ki = u.kind[k];
            if ki == sim_core::economy::NO_KIND {
                continue;
            }
            let cost = e.kinds.at(ki as usize).cost.max(1);
            let done = cost.saturating_sub(u.site[k]);
            // Целочисленно до самого показа: доля — это уже рендер.
            let part = (done as f64 / cost as f64) as f32;
            let p = Vec3::new(u.x[k] as f32, u.y[k] as f32 + 0.2, u.z[k] as f32);
            giz.circle(Isometry3d::new(p, flat), r * 2.0, BUILD_COLOUR).resolution(24);
            giz.circle(Isometry3d::new(p, flat), r * 2.0 * part, BUILD_COLOUR).resolution(24);
        }
    }

    if let Some((p, c, life)) = sel.mark {
        // Отметка гаснет: приказ отдан один раз, а показать его надо
        // дольше кадра.
        let k = (life / MARK_LIFE).clamp(0.0, 1.0);
        giz.circle(Isometry3d::new(p + Vec3::Y * 0.3, flat), 3.0 + 6.0 * (1.0 - k), c)
            .resolution(24);
        giz.line(p, p + Vec3::Y * 6.0, c);
    }

    // Рамка рисуется в мире, но у самой камеры: четыре угла экрана,
    // отложенные по лучам на одно и то же расстояние, дают на экране
    // ровный прямоугольник. Отдельная камера ради двумерных линий
    // обошлась бы дороже.
    let (Some(a), Ok(win), Ok((cam, gt))) = (sel.drag, windows.single(), cams.single()) else {
        return;
    };
    let Some(b) = win.cursor_position() else { return };
    if a.distance(b) < DRAG_PX {
        return;
    }
    let corners = [
        Vec2::new(a.x, a.y),
        Vec2::new(b.x, a.y),
        Vec2::new(b.x, b.y),
        Vec2::new(a.x, b.y),
    ];
    let mut pts = [Vec3::ZERO; 4];
    for (k, c) in corners.iter().enumerate() {
        let Ok(ray) = cam.viewport_to_world(gt, *c) else { return };
        pts[k] = ray.origin + *ray.direction * 5.0;
    }
    for k in 0..4 {
        giz.line(pts[k], pts[(k + 1) % 4], RECT_COLOUR);
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
    for (c, mut vis) in chunks.iter_mut() {
        let (lo, hi) = sim.grid.aabb(c.0);
        *vis = if f.intersects_aabb(lo, hi) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

/// Заголовок окна вместо панели, снимок по расписанию и выход по кадрам.
///
/// Заголовок, а не текст в кадре: панель потребовала бы `bevy_ui`,
/// то есть новой черты в пине Bevy, а сказать надо три числа.
fn snap(
    mut commands: Commands,
    time: Res<Time>,
    sim: Res<Sim>,
    sel: Res<Sel>,
    mut windows: Query<&mut Window>,
    mut shot: ResMut<Shot>,
    mut exit: MessageWriter<AppExit>,
) {
    shot.seen += 1;
    if let Ok(mut w) = windows.single_mut() {
        let live = sim.r.world.units.live().len();
        w.title = format!(
            "rts-vox — тик {} | выделено {} из {live} | приказов применено {} (ждут {}) | {}",
            sim.r.world.tick,
            sel.picked.len(),
            sim.r.world.orders_applied,
            sim.r.world.orders_pending(),
            if sim.speed == 0 { "пауза" } else { "идёт" }
        );
    }
    if !shot.done && time.elapsed_secs() >= shot.after {
        if let Some(p) = shot.path.clone() {
            let u = &sim.r.world.units;
            let under = u
                .live()
                .iter()
                .filter(|&&i| u.stance[i as usize] != sim_core::STANCE_FLOW)
                .count();
            println!(
                "снимок на {:.1} с, кадр {}, тик {}: живых {}, выделено {}, приказов применено {} (под приказом {under})",
                time.elapsed_secs(),
                shot.seen,
                sim.r.world.tick,
                u.live().len(),
                sel.picked.len(),
                sim.r.world.orders_applied
            );
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        }
        shot.done = true;
    }
    if shot.frames != 0 && shot.seen >= shot.frames {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Щелчок берёт ближайшего, а ничья уходит **меньшему номеру**.
    ///
    /// Порядок обхода здесь — порядок живых юнитов, и он же порядок
    /// номеров; правило проверяется на списке, собранном наоборот.
    #[test]
    fn a_click_takes_the_nearest_and_ties_go_to_the_lower_index() {
        let at = Vec2::new(100.0, 100.0);
        let c = vec![
            (7u32, Vec2::new(110.0, 100.0)),
            (2u32, Vec2::new(104.0, 100.0)),
            (9u32, Vec2::new(160.0, 100.0)),
        ];
        assert_eq!(nearest(&c, at, PICK_PX), Some(2));
        // Ничья: двое на одинаковом расстоянии, порядок обхода обратный.
        let tie = vec![
            (5u32, Vec2::new(110.0, 100.0)),
            (3u32, Vec2::new(90.0, 100.0)),
        ];
        assert_eq!(nearest(&tie, at, PICK_PX), Some(3), "ничья ушла не меньшему");
        // Дальше пальца — никого.
        assert_eq!(nearest(&c, Vec2::new(0.0, 0.0), PICK_PX), None);
    }

    /// Рамка ловит попавших внутрь при любом направлении протяжки.
    #[test]
    fn the_rectangle_does_not_care_which_way_it_was_dragged() {
        let (a, b) = (Vec2::new(10.0, 10.0), Vec2::new(50.0, 40.0));
        assert!(in_rect(Vec2::new(20.0, 20.0), a, b));
        assert!(in_rect(Vec2::new(20.0, 20.0), b, a), "протяжка вверх-влево не считается");
        assert!(!in_rect(Vec2::new(60.0, 20.0), a, b));
    }

    /// Отрезок внутри ящика: снаружи луч не марширует вовсе.
    #[test]
    fn the_ray_is_clipped_to_the_world_box() {
        let hi = Vec3::new(10.0, 10.0, 10.0);
        let (t0, t1) = box_span(Vec3::new(-5.0, 5.0, 5.0), Vec3::X, hi).expect("луч входит в мир");
        assert!((t0 - 5.0).abs() < 1e-3 && (t1 - 15.0).abs() < 1e-3, "{t0}..{t1}");
        // Мимо: параллельно грани и вне её.
        assert!(box_span(Vec3::new(-5.0, 50.0, 5.0), Vec3::X, hi).is_none());
        // Изнутри: отрезок начинается с нуля.
        let (t0, _) = box_span(Vec3::new(5.0, 5.0, 5.0), Vec3::X, hi).expect("изнутри");
        assert!(t0 <= 0.0 + 1e-6);
    }
}
