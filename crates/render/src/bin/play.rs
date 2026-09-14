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

use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::DirectionalLightShadowMap;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use render::gait::{FootPlant, GaitParams, LegRig, LegRole, Stance};
use render::geom::{MeshBuf, Slice};
use render::look;
use render::model::SegFrame;
use render::occlusion::OcclusionPlugin;
use render::realtime::{hull_rotation, Poses, TickClock};
use render::recording::Recording;
use render::terrain_scene::{TerrainChunk, TerrainScene};
use render::tex::tile_image;
use render::{yaw_to, Camera as RtsCamera, V3};
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
    terrain: TerrainScene,
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
    clock: TickClock,
}

/// Длина тика в секундах. Частота — из симуляции, а не своя:
/// скорость юнита задана в клетках в секунду и делится там же.
const DT: f64 = 1.0 / sim_core::TICKS_PER_SECOND as f64;

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
struct UnitMark(usize);

#[derive(Component)]
struct TurretMark(usize);

#[derive(Component)]
struct LegMark {
    rig: usize,
    role: LegRole,
}

/// Какой вид тела нарисован в слоте сейчас.
///
/// Слот переиспользуется свободным списком, и в место погибшего танка
/// встаёт склад. Без этой записи склад остался бы нарисован танком —
/// молча, до конца партии.
#[derive(Resource, Default)]
struct Shown {
    /// Вид тела и сторона на слот; `None` — сущностей у слота нет.
    at: Vec<Option<(usize, u8, u64)>>,
}

#[derive(Default)]
struct UnitFeet {
    identity: Option<(usize, u8)>,
    legs: Vec<FootPlant>,
}

#[derive(Resource, Default)]
struct Feet(Vec<UnitFeet>);

#[derive(Component)]
struct ShotMark;

#[derive(Component)]
struct BlastMark(u32);

struct DebrisPose {
    entity: Entity,
    previous: Vec3,
    current: Vec3,
}
#[derive(Resource, Default)]
struct DebrisShown {
    chunks: std::collections::BTreeMap<u64, Vec<Handle<Mesh>>>,
    tick: u64,
    poses: std::collections::BTreeMap<u64, DebrisPose>,
    mesh: std::collections::BTreeMap<render::model::FragmentMeshKey, Handle<Mesh>>,
}

/// Сегмент тела: меш по отделкам и место в теле (ADR-231).
struct SegArt {
    mesh: [Option<Handle<Mesh>>; look::FINISHES],
    frame: SegFrame,
}
/// Меши одного вида тела: по отделке и по стороне.
///
/// Отделок пять (сталь, стекло, резина, ядро, свечение), и у каждой
/// свой материал: цвет вершины подменяет `base_color` целиком, поэтому
/// раскраска сторон остаётся при себе (ADR-211).
type BodyMeshes = [[Option<Handle<Mesh>>; look::FINISHES]; 2];

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
    hull: Vec<BodyMeshes>,
    turret: Vec<BodyMeshes>,
    /// Повёрнутые части тела, тем же индексом: вид тела, сторона
    /// (ADR-231). Едут с корпусом или с башней — по своей рамке.
    segs: Vec<[Vec<SegArt>; 2]>,
    legs: Vec<Vec<LegRig>>,
    shot: Handle<Mesh>,
    blast: Handle<Mesh>,
    beam: Handle<Mesh>,
    /// Материал отделки. У сторон он **общий**: цвет несут вершины,
    /// а сталь остаётся сталью, какого бы цвета ни был корпус.
    finish: [Handle<StandardMaterial>; look::FINISHES],
    wreck_finish: [Handle<StandardMaterial>; look::FINISHES],
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
    let mut height = 0u32;
    let mut bases = true;
    let mut scenario_path: Option<String> = None;
    let mut load_path: Option<String> = None;
    let mut cache = true;
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
    let mut record_dir: Option<String> = None;
    let mut record_seconds = 10.0f64;
    let mut demo = 0u64;
    let (mut yaw, mut pitch, mut dist) = (0.7f32, 0.90f32, 0.45f32);
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--units" => units = v.parse().unwrap_or(units),
            "--side" => side = v.parse().unwrap_or(side),
            "--height" => height = v.parse().unwrap_or(height),
            "--bases" => bases = v != "0",
            "--scenario" => scenario_path = Some(v),
            "--load" => load_path = Some(v),
            "--cache" => cache = v != "0",
            "--relief" => relief = v.parse().unwrap_or(relief),
            "--mm" => body_mm = v.parse().unwrap_or(body_mm),
            "--seed" => seed = v.parse().unwrap_or(seed),
            // Множитель реального времени, а не тиков на кадр:
            // единица — тридцать тиков в секунду.
            "--speed" => speed = v.parse().unwrap_or(speed),
            "--shot" => shot = Some(v.clone()),
            "--shot-after" => after = v.parse().unwrap_or(after),
            "--frames" => frames = v.parse().unwrap_or(frames),
            "--record" => record_dir = Some(v),
            "--record-seconds" => {
                record_seconds = v.parse().expect("record-seconds must be a number")
            }
            "--demo" => demo = v.parse().unwrap_or(demo),
            "--yaw" => yaw = v.parse().unwrap_or(yaw),
            "--pitch" => pitch = v.parse().unwrap_or(pitch),
            "--dist" => dist = v.parse().unwrap_or(dist),
            "--help" => {
                println!(
                    "play [--load save.bin] [--scenario файл.ron] [--units N] [--side N] [--height N] [--bases 0|1] [--cache 0|1] [--relief N] [--seed N] [--mm ребро-тела-мм] [--speed множитель] \
[--shot файл] [--shot-after сек] [--frames N] [--record пустая-папка] [--record-seconds сек] [--demo кадр] [--yaw] [--pitch] [--dist]"
                );
                return;
            }
            other => {
                eprintln!("неизвестный ключ {other}");
                return;
            }
        }
    }

    assert!(
        record_dir.is_none() || (speed == 1 && frames == 0 && shot.is_none()),
        "--record needs --speed 1 and cannot be combined with --frames or --shot"
    );
    let recording = Recording::new(record_dir, record_seconds).expect("record setup");
    let t0 = Instant::now();
    assert!(
        load_path.is_none() || scenario_path.is_none(),
        "use either --load or --scenario"
    );
    let scenario = scenario_path.as_ref().map(|p| {
        modding::skirmish::Skirmish::from_ron(&std::fs::read_to_string(p).expect("scenario file"))
            .expect("valid scenario")
    });
    if let Some(s) = scenario.as_ref() {
        side = s.size[0];
        height = s.size[1];
        seed = s.seed;
        units = 0;
        bases = false;
    }
    let tuning = {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/sim.ron");
        let path = if here.exists() {
            here
        } else {
            std::path::PathBuf::from("data/sim.ron")
        };
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
        let path = if here.exists() {
            here
        } else {
            std::path::PathBuf::from("data/weapons.ron")
        };
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
        let path = if here.exists() {
            here
        } else {
            std::path::PathBuf::from("data/kinds.ron")
        };
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
        let path = if here.exists() {
            here
        } else {
            std::path::PathBuf::from("data/materials.ron")
        };
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
    let mut params =
        worldgen::Params::sized(side, if height > 0 { height } else { side / 2 }, side);
    if relief > 0 {
        params.relief = relief;
    }
    if let Some(s) = scenario.as_ref() {
        sim_core::skirmish::validate_layout(s, &kinds).expect("scenario footprints");
    }
    let r = if let Some(path) = &load_path {
        let bytes = std::fs::read(path).expect("saved game file");
        sim_core::save::load_with(&bytes, tuning, weapons, Some((kinds, mats)))
            .expect("compatible saved game")
    } else {
        let ground = load_ground(params, seed, &tuning, cache);
        let mut r = Reference::with_ground(seed, units, tuning, weapons, params, ground);
        // **Экономика включается здесь, а не в эталонном сценарии.**
        // Гейт её не ведёт, и его хеш от неё не сдвигается ни на бит
        // (ADR-230): того, чего нет, в хеше нет.
        r.world.attach_economy(kinds, mats);
        if let Some(s) = scenario.as_ref() {
            for u in &s.units {
                let id = r
                    .world
                    .spawn_kind(u.at[0], u.at[1], u.owner, u.kind)
                    .expect("scenario kind");
                let order = if let Some([x, z]) = u.attack_move {
                    Order::AttackMove { unit: id, x, z }
                } else if let Some([x, z]) = u.move_to {
                    Order::MoveTo { unit: id, x, z }
                } else {
                    Order::Hold { unit: id }
                };
                r.world.push_order(order);
            }
            r.world.units.refresh_live();
        }
        if bases {
            found_bases(&mut r.world, side as i32);
        }
        r
    };
    if load_path.is_some() {
        side = r.world.ground().unwrap().world.nx;
    }
    let g = r.world.ground().expect("у эталона есть земля");
    let settings = render::stream::Stream::from_ron(include_str!("../../../../data/render.ron"))
        .expect("render.ron");
    let terrain = TerrainScene::new(&g.world, settings);
    let grid = terrain.grid;
    let focus = scenario
        .as_ref()
        .map(|s| s.focus)
        .unwrap_or([side as i32 / 2, side as i32 / 2]);
    let floor = g.nav.floor_at(
        focus[0] as u32 / g.nav.step(),
        focus[1] as u32 / g.nav.step(),
    ) as f32;
    let side = side as f32;
    println!(
        "мир {}×{}×{}, чанков {}, юнитов {}, сборка {:.0} мс",
        g.world.nx,
        g.world.ny,
        g.world.nz,
        grid.len(),
        r.world.units.live().len(),
        t0.elapsed().as_secs_f64() * 1e3
    );
    println!(
        "ЛКМ — выделить, протяжка — рамка, Shift — добавить; \
ПКМ — идти или атаковать, Shift+ПКМ — идти с боем, Ctrl+ПКМ — копать; H — стоять, G — снять приказ, \
Пробел — пауза; WASD/QE/RF — камера, Esc — выход"
    );
    println!(
        "B/N/T — выбрать склад, завод, турель и поставить Ctrl+ПКМ (Esc — передумать); \
1/2/3 — заказать выделенному заводу танк, краулер, шагоход; X — снять заказ"
    );

    let cam = RtsCamera {
        target: V3::new(focus[0] as f32, floor + 15.0, focus[1] as f32),
        yaw,
        pitch,
        distance: (side * dist).min(settings.build_radius as f32 * 0.65),
        fovy: 0.87,
        aspect: 16.0 / 9.0,
        near: 1.0,
        far: side * 4.0,
    };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "rts-vox".into(),
                resolution:
                    bevy::window::WindowResolution::new(1600, 900).with_scale_factor_override(1.0),
                present_mode: if frames == 0 {
                    PresentMode::AutoVsync
                } else {
                    PresentMode::AutoNoVsync
                },
                ..default()
            }),
            ..default()
        }))
        // Щелевое затенение по вокселям — тот же проход, что у окна
        // боя: направление одно на все окна (ADR-235).
        .add_plugins(OcclusionPlugin)
        .insert_resource(DirectionalLightShadowMap {
            size: look::scene().shadow_map,
        })
        .insert_resource(GlobalAmbientLight {
            color: srgb(look::scene().ambient_srgb),
            brightness: look::scene().ambient_brightness,
            ..default()
        })
        .insert_resource(Sim {
            r,
            terrain,
            speed,
            was: speed.max(1),
            clock: TickClock::default(),
        })
        .insert_resource(View { cam, side, body_mm })
        .insert_resource(recording)
        .insert_resource(Sel::default())
        .insert_resource(Shot {
            path: shot,
            after,
            frames,
            seen: 0,
            done: false,
            demo,
        })
        .insert_resource(Shown::default())
        .insert_resource(Poses::default())
        .insert_resource(Feet::default())
        .insert_resource(DebrisShown::default())
        .insert_resource(Hud::default())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                control, hands, dress, step, debris, overlay, hud, cull, snap,
            )
                .chain(),
        )
        .run();
}

fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

fn load_ground(
    params: worldgen::Params,
    seed: u64,
    tuning: &modding::Tuning,
    cache: bool,
) -> sim_core::ground::Ground {
    let key = sim_core::mapcache::key(&params, seed, tuning);
    let path = std::path::PathBuf::from(format!("cache/maps/map-{key:016x}.bin"));
    if cache {
        if let Ok(bytes) = std::fs::read(&path) {
            match sim_core::mapcache::from_cache(
                &bytes,
                &params,
                seed,
                tuning,
                sim_core::REF_MAX_EDITS,
            ) {
                Ok(g) => {
                    println!("map cache: {}", path.display());
                    return g;
                }
                Err(e) => eprintln!("map cache rejected: {e}"),
            }
        }
    }
    let world = voxel::World::new(
        params.nx,
        params.ny,
        params.nz,
        sim_core::REF_MAX_EDITS,
        Box::new(worldgen::Terrain { params, seed }),
    );
    let ground = sim_core::ground::Ground::new(world, tuning);
    if cache {
        let bytes = sim_core::mapcache::to_cache(&ground, key);
        if let Err(e) =
            std::fs::create_dir_all("cache/maps").and_then(|()| std::fs::write(&path, bytes))
        {
            eprintln!("map cache write: {e}");
        }
    }
    ground
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

// Bevy injects each resource independently into this system.
#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut gizmo_cfg: ResMut<GizmoConfigStore>,
    sim: Res<Sim>,
    mut mediums: ResMut<Assets<ScatteringMedium>>,
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
                         centre: [f32; 3],
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
            centre,
            &mut ubufs,
        );
        // Пустая отделка ручки не получает: у безбашенного тела
        // башня пуста целиком, и заводить под неё сущность
        // с нулевым мешем незачем.
        std::array::from_fn(|k| {
            render::model::weather(&mut ubufs[k], k);
            (!ubufs[k].idx.is_empty()).then(|| meshes.add(buf_to_mesh(&ubufs[k])))
        })
    };
    let tm = Instant::now();
    let tint = look::side_tint();
    let per_mm = 1.0 / voxel::unit::WORLD_MM as f32;
    let mut hull: Vec<BodyMeshes> = Vec::new();
    let mut turret: Vec<BodyMeshes> = Vec::new();
    let mut segs: Vec<[Vec<SegArt>; 2]> = Vec::new();
    let mut legs = Vec::new();
    for b in &bodies {
        let root = render::model::root_centre(b);
        hull.push([
            unit_mesh(b, tint[0], Slice::Hull, root, &mut meshes),
            unit_mesh(b, tint[1], Slice::Hull, root, &mut meshes),
        ]);
        turret.push([
            unit_mesh(b, tint[0], Slice::Turret, root, &mut meshes),
            unit_mesh(b, tint[1], Slice::Turret, root, &mut meshes),
        ]);
        // Сегменты: у каждого свой меш вокруг шарнира и своё место
        // в теле, сложенное по цепочке креплений (ADR-231).
        let frames = render::model::segment_frames(b, per_mm);
        legs.push(render::gait::rigs_of(b, per_mm, &frames));
        segs.push(std::array::from_fn(|o| {
            b.segments
                .iter()
                .zip(frames.iter())
                .map(|(sg, frame)| SegArt {
                    mesh: unit_mesh(
                        &sg.body,
                        tint[o],
                        Slice::All,
                        render::model::pivot_centre(&sg.attach),
                        &mut meshes,
                    ),
                    frame: *frame,
                })
                .collect()
        }));
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
    let finish_mat = |f: look::Finish, powered: bool, m: &mut Assets<StandardMaterial>| {
        let (rough, metal) = look::finish_surface(f);
        m.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: rough,
            metallic: metal,
            // Обнажённое ядро светится само: увидеть его надо сразу,
            // а отражённого света внутри разбитого корпуса нет.
            emissive: if powered && f == look::Finish::Core {
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
        // Плитка свойств умножает числа материала, и они ставятся
        // на единицу: шероховатость и металличность класса лежат
        // в плитке, крупинками, а не гранью (ADR-235).
        let _ = (rough, metal);
        let props = i.add(tile_image(look::props_tile(c), false));
        m.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(i.add(tile_image(look::surface_tile(c), true))),
            normal_map_texture: Some(i.add(tile_image(look::normal_tile(c), false))),
            metallic_roughness_texture: Some(props.clone()),
            occlusion_texture: Some(props),
            // UV приходят в вокселях, а плитка накрывает несколько.
            uv_transform: bevy::math::Affine2::from_scale(Vec2::splat(1.0 / look::TILE_VOXELS)),
            perceptual_roughness: 1.0,
            metallic: 1.0,
            ..default()
        })
    };
    let art = Art {
        hull,
        turret,
        segs,
        legs,
        shot: meshes.add(Cuboid::new(0.8, 0.8, 0.8)),
        blast: meshes.add(Sphere::new(2.5)),
        beam: meshes.add(Cuboid::new(0.5, 1.0, 0.5)),
        finish: look::FINISH_ALL.map(|f| finish_mat(f, true, &mut materials)),
        wreck_finish: look::FINISH_ALL.map(|f| finish_mat(f, false, &mut materials)),
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

    // Terrain chunks are loaded around the camera by TerrainScene each frame.

    // Сущности юнитов заводятся **по мере появления юнитов**, а не
    // все разом на старте: завод выпускает машины, и слотов
    // становится больше, чем было. Делает это `dress` каждый кадр.

    let eye = view.cam.eye();
    let t = view.cam.target;
    let cam = commands
        .spawn((
            Camera3d::default(),
            Camera::default(),
            Msaa::Off,
            Projection::Perspective(PerspectiveProjection {
                fov: view.cam.fovy,
                near: 1.0,
                far: view.cam.far,
                ..default()
            }),
            Transform::from_xyz(eye.x, eye.y, eye.z).looking_at(Vec3::new(t.x, t.y, t.z), Vec3::Y),
        ))
        .id();
    render::presentation::camera(&mut commands, cam, view.cam.distance);
    commands
        .entity(cam)
        .remove::<bevy::post_process::dof::DepthOfField>();
    commands.entity(cam).insert((
        Tonemapping::AgX,
        bevy::camera::Exposure { ev100: 14.5 },
        bevy::light::AtmosphereEnvironmentMapLight {
            intensity: 1.0,
            ..default()
        },
        bevy::render::view::ColorGrading {
            global: bevy::render::view::ColorGradingGlobal {
                post_saturation: 1.02,
                ..default()
            },
            ..default()
        },
    ));
    render::presentation::daylight_scaled(
        &mut commands,
        &mut mediums,
        t.y - 15.0,
        voxel::unit::WORLD_MM as f32 / 1000.0,
        sim.terrain.settings.build_radius as f32 * 1.4,
    );
    commands.insert_resource(GlobalAmbientLight {
        brightness: 0.0,
        ..default()
    });
    commands.insert_resource(art);
}

fn buf_to_mesh(b: &MeshBuf) -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.pos.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm.clone())
    .with_inserted_indices(Indices::U32(b.idx.clone()));
    if !b.col.is_empty() && b.col.len() == b.pos.len() {
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, b.col.clone());
    }
    if b.uv.len() == b.pos.len() && !b.uv.is_empty() {
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, b.uv.clone());
    }
    if b.tan.len() == b.pos.len() && !b.tan.is_empty() {
        m.insert_attribute(Mesh::ATTRIBUTE_TANGENT, b.tan.clone());
    }
    m
}

type HullQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static UnitMark,
        &'static mut Transform,
        &'static mut Visibility,
        Option<&'static SegFrame>,
        Option<&'static LegMark>,
    ),
    (Without<ShotMark>, Without<TurretMark>),
>;

type TurretQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static TurretMark,
        &'static mut Transform,
        &'static mut Visibility,
        Option<&'static SegFrame>,
    ),
    Without<ShotMark>,
>;

/// Тик симуляции и перенос её состояния в сцену.
#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sim: ResMut<Sim>,
    time: Res<Time<Real>>,
    record: Res<Recording>,
    art: Res<Art>,
    view: Res<View>,
    mut poses: ResMut<Poses>,
    mut feet: ResMut<Feet>,
    mut units: HullQuery,
    mut turrets: TurretQuery,
    shots: Query<Entity, With<ShotMark>>,
    blasts: Query<(Entity, &mut BlastMark)>,
) {
    // Часы, а не кадры: сколько тиков задолжали, столько и делаем,
    // но не больше `CATCH_UP` за кадр.
    if poses.visible.is_empty() {
        poses.capture(&sim.r.world);
    }
    let speed = if record.running() { sim.speed } else { 0 };
    let done = sim
        .clock
        .advance(time.delta_secs_f64(), speed, DT, CATCH_UP);
    let mut edits = Vec::new();
    let mut events = Vec::new();
    for _ in 0..done {
        // **Тик мира, а не эталонного сценария.** У сценария свои
        // копатели-оснастка и подкрепления струйкой: гейту они нужны,
        // чтобы нагрузка не затухала, а игре мешают — копают мимо
        // учёта и подсыпают машин, которых игрок не заказывал
        // (ADR-230).
        sim.r.world.tick();
        poses.capture(&sim.r.world);
        events.extend_from_slice(sim.r.world.events());
        if let Some(g) = sim.r.world.ground() {
            edits.extend_from_slice(g.edits());
        }
    }
    poses.sample(sim.clock.fraction(DT));

    {
        let Sim { r, terrain, .. } = &mut *sim;
        terrain.update(
            &r.world.ground().expect("земля").world,
            &edits,
            view.cam.target,
            &art.ground,
            &mut meshes,
            &mut commands,
        );
    }

    let u = &sim.r.world.units;
    // Положение — **с дробной частью**. Целая часть меняется раз
    // в несколько тиков, и корпус от этого переставлялся скачком
    // на полметра пять раз в секунду: рывки, которые владелец видел,
    // наполовину были отсюда (ADR-220).
    let at = |i: usize| poses.visible[i].pos;
    let alive = |i: usize| i < u.len() && u.alive[i];
    // Курс — из симуляции, а не из дробной части положения.
    let hull_yaw = |i: usize| Some(poses.visible[i].yaw);
    let aim_yaw = |i: usize| {
        let t = u.target[i];
        if t == NO_TARGET {
            return None;
        }
        let j = t as usize;
        if !alive(j) {
            return None;
        }
        let d = at(j) - at(i);
        let (dx, dz) = (d.x, d.z);
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
        let Some(e) = sim.r.world.econ.as_ref() else {
            return 1.0;
        };
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

    let kinds = sim
        .r
        .world
        .econ
        .as_ref()
        .map(|e| e.kinds.len())
        .unwrap_or(0);
    let gait_params = sim
        .r
        .world
        .tuning
        .walking
        .gait
        .as_ref()
        .map(|g| GaitParams::of(g, 10.0 / voxel::unit::WORLD_MM as f32));
    let mut leg_poses = vec![Vec::new(); u.len()];
    feet.0.resize_with(u.len(), UnitFeet::default);
    for event in &events {
        if let SimEvent::Death { unit, .. } = *event {
            if let Some(slot) = feet.0.get_mut(unit as usize) {
                *slot = UnitFeet::default();
            }
        }
    }
    for i in 0..u.len() {
        if !u.alive[i] {
            feet.0[i] = UnitFeet::default();
        }
    }
    if let Some(params) = gait_params.as_ref() {
        let nav = &sim.r.world.ground().expect("земля").nav;
        let (nx, nz) = nav.dims();
        let edge = nav.step() as f32;
        for &id in u.live() {
            let i = id as usize;
            let model = look_of(&sim.r.world, i, kinds);
            let contact = &mut feet.0[i];
            if contact.identity != Some((model, u.owner[i])) {
                contact.identity = Some((model, u.owner[i]));
                contact.legs = vec![FootPlant::default(); art.legs[model].len()];
            }
            if art.legs[model].is_empty() {
                continue;
            }
            let reach = art.legs[model]
                .iter()
                .map(|r| r.l1 + r.l2)
                .fold(0.0f32, f32::max)
                .ceil() as i32;
            let floor = |x: f32, z: f32| {
                let (cx, cz) = ((x / edge).floor() as i32, (z / edge).floor() as i32);
                if cx < 0 || cz < 0 || cx >= nx as i32 || cz >= nz as i32 {
                    return at(i).y;
                }
                let level = nav
                    .level_near(cx as u32, cz as u32, u.body_cell_y(i))
                    .map(|h| h as f32)
                    .unwrap_or(at(i).y);
                render::footing::voxel_floor(
                    &sim.r.world.ground().expect("земля").world,
                    x,
                    z,
                    level,
                    reach,
                )
                .unwrap_or(level)
            };
            let pose = poses.visible[i];
            let motion = poses.motion(i, DT as f32);
            let stance = Stance {
                grounded: u.fall_from[i] == sim_core::NOT_FALLING,
                dt: time.delta_secs() * speed as f32,
                base: pose.pos,
                rot: hull_rotation(pose.up, pose.yaw),
                phase: pose.phase,
                lift_scale: (motion.cycles * params.stride / u.speed[i].max(1) as f32 * 4.0)
                    .min(1.0),
                motion,
                params,
                floor: &floor,
            };
            leg_poses[i].extend(
                art.legs[model]
                    .iter()
                    .zip(&mut contact.legs)
                    .map(|(rig, plant)| plant.pose(rig, &stance)),
            );
        }
    }
    for (m, mut tr, mut vis, seg, leg) in units.iter_mut() {
        let i = m.0;
        if !alive(i) {
            *vis = Visibility::Hidden;
            continue;
        }
        let g = grown(i);
        // Сегмент недостроя не показывается: повёрнутая плита
        // на половине корпуса висела бы в воздухе — по той же
        // причине, по которой скрыта башня (ADR-230, ADR-231).
        if seg.is_some() && g < 1.0 {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        if let Some(y) = hull_yaw(i) {
            let frame = leg
                .and_then(|l| leg_poses[i].get(l.rig).map(|p| p.part(l.role)))
                .or(seg);
            render::model::place(&mut tr, at(i), hull_rotation(poses.visible[i].up, y), frame);
        }
        tr.scale = Vec3::new(1.0, g, 1.0);
    }
    for (m, mut tr, mut vis, seg) in turrets.iter_mut() {
        let i = m.0;
        // Башня недостроя не показывается: она лежит выше корпуса,
        // и на половине высоты висела бы в воздухе.
        if !alive(i) || u.site[i] != 0 {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        if let Some(y) = aim_yaw(i).or_else(|| hull_yaw(i)) {
            render::model::place(&mut tr, at(i), hull_rotation(poses.visible[i].up, y), seg);
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
        if b.0 <= done {
            commands.entity(e).despawn();
        } else {
            b.0 -= done;
        }
    }
    for ev in &events {
        match *ev {
            SimEvent::Blast { x, y, z, .. } => {
                commands.spawn((
                    Mesh3d(art.blast.clone()),
                    MeshMaterial3d(art.boom.clone()),
                    Transform::from_xyz(x as f32, y as f32, z as f32),
                    BlastMark(6),
                ));
            }
            SimEvent::Beam {
                x0,
                y0,
                z0,
                x1,
                y1,
                z1,
                owner,
            } => {
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

/// Persistent, automatically instanced cubes read authoritative debris poses.
/// Only their pose is interpolated; visibility and material never affect physics.
fn debris(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    sim: Res<Sim>,
    art: Res<Art>,
    view: Res<View>,
    mut shown: ResMut<DebrisShown>,
    mut transforms: Query<&mut Transform>,
) {
    let position = |x: fx::Fx, y: fx::Fx, z: fx::Fx| {
        Vec3::new(
            x.raw() as f32 / fx::ONE_RAW as f32,
            y.raw() as f32 / fx::ONE_RAW as f32,
            z.raw() as f32 / fx::ONE_RAW as f32,
        )
    };
    let target = Vec3::new(view.cam.target.x, view.cam.target.y, view.cam.target.z);
    let radius2 = (sim.terrain.settings.keep_radius as f32).powi(2);
    let mut candidates: Vec<_> = sim
        .r
        .world
        .bursts
        .fragments
        .iter()
        .chain(sim.r.world.bursts.corpses.values())
        .filter_map(|f| {
            let p = position(f.position.x, f.position.y, f.position.z);
            let d = p.distance_squared(target);
            (d < radius2).then_some((f, p, d))
        })
        .collect();
    candidates.sort_by(|a, b| {
        a.0.resting
            .cmp(&b.0.resting)
            .then(a.2.total_cmp(&b.2))
            .then(a.0.id.cmp(&b.0.id))
    });
    candidates.truncate(sim.r.world.tuning.blast.max_flying as usize);
    let visible: std::collections::BTreeSet<_> = candidates.iter().map(|a| a.0.id).collect();
    shown.poses.retain(|id, p| {
        if visible.contains(id) {
            true
        } else {
            commands.entity(p.entity).despawn();
            false
        }
    });
    shown.chunks.retain(|id, handles| {
        if visible.contains(id) {
            true
        } else {
            for h in handles {
                meshes.remove(h.id());
            }
            false
        }
    });
    let mut chunk_builds = 0;
    let advanced = shown.tick != sim.r.world.tick;
    shown.tick = sim.r.world.tick;
    let fraction = sim.clock.fraction(DT);
    for (f, p, _) in candidates {
        if let Some(chunk) = &f.chunk {
            if !shown.poses.contains_key(&f.id) {
                if chunk_builds >= 2 {
                    continue;
                }
                chunk_builds += 1;
                let template = &sim.r.world.material_bodies.as_ref().unwrap().templates
                    [chunk.template as usize];
                let mut handles = Vec::new();
                let entity = commands
                    .spawn((Transform::from_translation(p), Visibility::default()))
                    .with_children(|parent| {
                        for (finish, buf) in render::model::section_to_mesh(template, chunk) {
                            let mesh = meshes.add(buf_to_mesh(&buf));
                            handles.push(mesh.clone());
                            parent.spawn((
                                Mesh3d(mesh),
                                MeshMaterial3d(if chunk.body.is_some() {
                                    art.wreck_finish[finish].clone()
                                } else {
                                    art.finish[finish].clone()
                                }),
                                Transform::default(),
                            ));
                        }
                    })
                    .id();
                shown.chunks.insert(f.id, handles);
                shown.poses.insert(
                    f.id,
                    DebrisPose {
                        entity,
                        previous: p,
                        current: p,
                    },
                );
            }
            let pose = shown.poses.get_mut(&f.id).unwrap();
            if advanced {
                pose.previous = pose.current;
                pose.current = p;
            }
            if let Ok(mut tr) = transforms.get_mut(pose.entity) {
                tr.translation = pose.previous.lerp(pose.current, fraction);
            }
            continue;
        }
        let key = (f.material, f.shape, f.paint);
        let finish = f.paint.map(|p| {
            let part = match p[2] {
                1 => voxel::Part::Core,
                2 => voxel::Part::Engine,
                3 => voxel::Part::Weapon,
                4 => voxel::Part::Sensor,
                _ => voxel::Part::Armour,
            };
            look::finish_of(p[1], part)
        });
        let colour = if let (Some(p), Some(finish)) = (f.paint, finish) {
            look::to_linear(look::finish_srgb(
                finish,
                look::side_tint()[p[0] as usize % 2],
            ))
        } else {
            look::colour(f.material)
        };
        let material = finish.map_or_else(
            || art.ground[look::class(f.material) as usize].clone(),
            |f| art.finish[f as usize].clone(),
        );
        let basis = f.basis.map(|v| position(v.x, v.y, v.z));
        let rotation = Quat::from_mat3(&Mat3::from_cols(basis[0], basis[1], basis[2])).normalize();
        if let std::collections::btree_map::Entry::Vacant(entry) = shown.mesh.entry(key) {
            let c = sim_core::burst::shape_centre(f.shape);
            let centre = position(c.x, c.y, c.z);
            let mut combined: Option<Mesh> = None;
            for i in 0..8 {
                if f.shape & (1 << i) != 0 {
                    let offset =
                        Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32)
                            - centre;
                    let mut cube = Mesh::from(Cuboid::new(0.98, 0.98, 0.98)).translated_by(offset);
                    cube.insert_attribute(
                        Mesh::ATTRIBUTE_COLOR,
                        vec![colour; cube.count_vertices()],
                    );
                    if let Some(mesh) = combined.as_mut() {
                        mesh.merge(&cube).expect("same cube vertex attributes");
                    } else {
                        combined = Some(cube);
                    }
                }
            }
            entry.insert(meshes.add(combined.expect("nonempty authoritative fragment")));
        }
        let scale = (f.count as f32 / f.shape.count_ones().max(1) as f32).cbrt() * f.mm as f32
            / voxel::unit::WORLD_MM as f32;
        let pose = if let Some(pose) = shown.poses.get_mut(&f.id) {
            if advanced {
                pose.previous = pose.current;
                pose.current = p;
            }
            pose
        } else {
            let mesh = shown.mesh[&key].clone();
            let entity = commands
                .spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    Transform::from_translation(p).with_scale(Vec3::splat(scale)),
                ))
                .id();
            shown.poses.entry(f.id).or_insert(DebrisPose {
                entity,
                previous: p,
                current: p,
            })
        };
        if let Ok(mut tr) = transforms.get_mut(pose.entity) {
            tr.translation = pose.previous.lerp(pose.current, fraction);
            tr.scale = Vec3::splat(scale);
            tr.rotation = rotation;
        }
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
    mut meshes: ResMut<Assets<Mesh>>,
    mut shown: ResMut<Shown>,
    hulls: Query<(Entity, &UnitMark)>,
    turrets: Query<(Entity, &TurretMark)>,
) {
    let kinds = sim
        .r
        .world
        .econ
        .as_ref()
        .map(|e| e.kinds.len())
        .unwrap_or(0);
    let u = &sim.r.world.units;
    let n = u.len();
    if shown.at.len() < n {
        shown.at.resize(n, None);
    }
    let mut redress: Vec<usize> = Vec::new();
    let mut damage_builds = 0;
    for i in 0..n {
        let want = if u.alive[i] {
            Some((
                look_of(&sim.r.world, i, kinds),
                u.owner[i],
                sim.r
                    .world
                    .material_bodies
                    .as_ref()
                    .and_then(|b| b.state(i as u32))
                    .map_or(0, |b| b.revision),
            ))
        } else {
            None
        };
        if shown.at[i] != want {
            if want.is_some_and(|v| v.2 > 0) {
                // Bound expensive unique volumes. Pristine units share meshes.
                if damage_builds >= 2 {
                    continue;
                }
                damage_builds += 1;
            }
            redress.push(i);
        }
    }
    if redress.is_empty() {
        return;
    }
    for (e, m) in hulls.iter() {
        // Двоичным поиском: список собран обходом по возрастанию,
        // а сущностей у слота пять — линейный поиск по каждой из них
        // дал бы квадрат на залпе, выбившем полсотни машин разом.
        if redress.binary_search(&m.0).is_ok() {
            commands.entity(e).despawn();
        }
    }
    for (e, m) in turrets.iter() {
        // Двоичным поиском: список собран обходом по возрастанию,
        // а сущностей у слота пять — линейный поиск по каждой из них
        // дал бы квадрат на залпе, выбившем полсотни машин разом.
        if redress.binary_search(&m.0).is_ok() {
            commands.entity(e).despawn();
        }
    }
    for i in redress {
        let want = if u.alive[i] {
            Some((
                look_of(&sim.r.world, i, kinds),
                u.owner[i],
                sim.r
                    .world
                    .material_bodies
                    .as_ref()
                    .and_then(|b| b.state(i as u32))
                    .map_or(0, |b| b.revision),
            ))
        } else {
            None
        };
        shown.at[i] = want;
        let Some((look, owner, revision)) = want else {
            continue;
        };
        let o = usize::from(owner == 1);
        let Some(h) = art.hull.get(look) else {
            continue;
        };
        // По сущности на отделку. Системы движения ищут по метке
        // и обновляют их все разом одним и тем же преобразованием:
        // корпус остаётся корпусом, из скольких бы материалов он
        // ни был собран.
        let damaged = (revision > 0).then(|| {
            let bodies = sim.r.world.material_bodies.as_ref().unwrap();
            let body = bodies.body(i as u32).unwrap();
            let state = bodies.state(i as u32).unwrap();
            let removed = state.removed.iter().copied().collect();
            let centre = render::model::root_centre(body);
            let scale = body.mm as f32 / voxel::unit::WORLD_MM as f32;
            let ty = body.turret_y as usize;
            let layers = if ty == 0 {
                [(0, usize::MAX), (0, 0)]
            } else {
                [(0, ty), (ty, usize::MAX)]
            };
            layers.map(|layers| {
                let mut bufs = std::array::from_fn(|_| MeshBuf::default());
                render::model::body_to_mesh(
                    body,
                    look::side_tint()[o],
                    scale,
                    layers,
                    render::model::BEVEL,
                    centre,
                    &mut bufs,
                );
                std::array::from_fn::<_, { look::FINISHES }, _>(|k| {
                    render::model::weather(&mut bufs[k], k);
                    render::model::damage_marks(
                        &mut bufs[k],
                        body,
                        &state.dents,
                        &removed,
                        scale,
                        centre,
                    );
                    (!bufs[k].idx.is_empty()).then(|| meshes.add(buf_to_mesh(&bufs[k])))
                })
            })
        });
        let hull_mesh = damaged.as_ref().map_or(&h[o], |m| &m[0]);
        let turret_mesh = damaged.as_ref().map_or(&art.turret[look][o], |m| &m[1]);
        for (k, m) in art.finish.iter().enumerate() {
            if let Some(mesh) = hull_mesh[k].clone() {
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(m.clone()),
                    Transform::from_xyz(0.0, -1000.0, 0.0),
                    Visibility::Hidden,
                    UnitMark(i),
                ));
            }
            if let Some(mesh) = turret_mesh[k].clone() {
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(m.clone()),
                    Transform::from_xyz(0.0, -1000.0, 0.0),
                    Visibility::Hidden,
                    TurretMark(i),
                ));
            }
        }
        // Повёрнутые части тела: метка та же, что у корпуса или
        // башни, плюс рамка — система движения не отличает сегмент
        // от корня (ADR-231).
        for (segment, sg) in art.segs[look][o].iter().enumerate() {
            if revision > 0 {
                let bodies = sim.r.world.material_bodies.as_ref().unwrap();
                let body = bodies.body(i as u32).unwrap();
                let template = &bodies.templates[bodies.state(i as u32).unwrap().template as usize];
                let anchor = sg.frame.pos * (voxel::unit::WORLD_MM as f32 / body.mm as f32)
                    + Vec3::from(render::model::root_centre(body));
                let a = anchor.floor().as_ivec3();
                let lost = |p: IVec3| {
                    p.cmpge(IVec3::ZERO).all()
                        && p.cmplt(IVec3::new(
                            body.vol.nx as i32,
                            body.vol.ny as i32,
                            body.vol.nz as i32,
                        ))
                        .all()
                        && template.vol.get(p.x as usize, p.y as usize, p.z as usize)
                        && !body.vol.get(p.x as usize, p.y as usize, p.z as usize)
                };
                if lost(a) {
                    continue;
                }
            }
            for (k, m) in art.finish.iter().enumerate() {
                let Some(mesh) = sg.mesh[k].clone() else {
                    continue;
                };
                let mut e = commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(m.clone()),
                    Transform::from_xyz(0.0, -1000.0, 0.0),
                    Visibility::Hidden,
                    sg.frame,
                ));
                if sg.frame.turret {
                    e.insert(TurretMark(i));
                } else {
                    e.insert(UnitMark(i));
                    if let Some((rig, role)) = art.legs[look]
                        .iter()
                        .enumerate()
                        .find_map(|(r, l)| l.role(segment).map(|role| (r, role)))
                    {
                        e.insert(LegMark { rig, role });
                    }
                }
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
    sel: Res<Sel>,
    mut cams: Query<&mut Transform, With<Camera3d>>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::Escape) && sel.build.is_none() {
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
        view.cam.distance =
            (view.cam.distance * (1.0 + dt)).min(sim.terrain.settings.build_radius as f32 * 0.65);
    }
    if keys.just_pressed(KeyCode::Home) {
        let u = &sim.r.world.units;
        if let Some(id) = sel.picked.first().copied().or_else(|| {
            u.live()
                .iter()
                .copied()
                .find(|&i| u.owner[i as usize] == PLAYER)
        }) {
            view.cam.target.x = u.x[id as usize] as f32;
            view.cam.target.z = u.z[id as usize] as f32;
        }
    }
    view.cam.target.x = view.cam.target.x.clamp(0.0, view.side - 1.0);
    view.cam.target.z = view.cam.target.z.clamp(0.0, view.side - 1.0);
    let nav = &sim.r.world.ground().expect("земля").nav;
    let (nx, nz) = nav.dims();
    let cx = (view.cam.target.x / nav.step() as f32) as u32;
    let cz = (view.cam.target.z / nav.step() as f32) as u32;
    let floor = nav.floor_at(cx.min(nx - 1), cz.min(nz - 1)) as f32;
    view.cam.target.y += (floor + 15.0 - view.cam.target.y) * (dt * 5.0).min(1.0);
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
        if c[0] >= 0 && c[1] >= 0 && c[2] >= 0 && w.solid(c[0] as u32, c[1] as u32, c[2] as u32) {
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
    poses: Res<Poses>,
    mut sel: ResMut<Sel>,
    shot: Res<Shot>,
) {
    // Выделение чистится от погибших: номер живёт дольше юнита.
    {
        let u = &sim.r.world.units;
        sel.picked.retain(|&i| {
            (i as usize) < u.len() && u.alive[i as usize] && u.owner[i as usize] == PLAYER
        });
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
                sim.r.world.push_order(Order::MoveTo {
                    unit: id,
                    x: gx,
                    z: gz,
                });
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
            let o = if hold {
                Order::Hold { unit: id }
            } else {
                Order::Release { unit: id }
            };
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
                    let p = poses.visible.get(k).map(|p| p.pos).unwrap_or(Vec3::new(
                        u.x[k] as f32,
                        u.body_y(k).to_f64_lossy() as f32,
                        u.z[k] as f32,
                    )) + Vec3::Y;
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
    let Ok(ray) = cam.viewport_to_world(gt, c) else {
        return;
    };
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
            let p = poses.visible.get(k).map(|p| p.pos).unwrap_or(Vec3::new(
                u.x[k] as f32,
                u.body_y(k).to_f64_lossy() as f32,
                u.z[k] as f32,
            )) + Vec3::Y;
            if let Some(s) = on_screen(cam, gt, p) {
                foes.push((i, s));
            }
        }
    }
    if let Some(foe) = nearest(&foes, c, PICK_PX) {
        let picked = sel.picked.clone();
        for id in picked {
            sim.r.world.push_order(Order::Attack {
                unit: id,
                target: foe,
            });
        }
        let u = &sim.r.world.units;
        let k = foe as usize;
        sel.mark = Some((
            poses.visible.get(k).map(|p| p.pos).unwrap_or(Vec3::new(
                u.x[k] as f32,
                u.body_y(k).to_f64_lossy() as f32,
                u.z[k] as f32,
            )),
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
                sim.r.world.push_order(Order::Build {
                    unit: id,
                    kind,
                    x: p[0],
                    z: p[2],
                });
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
            let order = if shift {
                Order::AttackMove {
                    unit: id,
                    x: p[0],
                    z: p[2],
                }
            } else {
                Order::MoveTo {
                    unit: id,
                    x: p[0],
                    z: p[2],
                }
            };
            sim.r.world.push_order(order);
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
        if i > 0 && (s.len() - i).is_multiple_of(3) {
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
    poses: Res<Poses>,
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
        let p = poses.visible[k].pos + Vec3::Y * 0.2;
        giz.circle(Isometry3d::new(p, flat), r, SEL_COLOUR)
            .resolution(16);
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
            giz.circle(Isometry3d::new(p, flat), r * 2.0, BUILD_COLOUR)
                .resolution(24);
            giz.circle(Isometry3d::new(p, flat), r * 2.0 * part, BUILD_COLOUR)
                .resolution(24);
        }
    }

    if let Some((p, c, life)) = sel.mark {
        // Отметка гаснет: приказ отдан один раз, а показать его надо
        // дольше кадра.
        let k = (life / MARK_LIFE).clamp(0.0, 1.0);
        giz.circle(
            Isometry3d::new(p + Vec3::Y * 0.3, flat),
            3.0 + 6.0 * (1.0 - k),
            c,
        )
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
    let Some(b) = win.cursor_position() else {
        return;
    };
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
        let Ok(ray) = cam.viewport_to_world(gt, *c) else {
            return;
        };
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
    mut chunks: Query<(&TerrainChunk, &mut Visibility)>,
) {
    if let Ok(w) = windows.single() {
        let (ww, hh) = (w.width(), w.height());
        if hh > 0.0 {
            view.cam.aspect = ww / hh;
        }
    }
    let f = view.cam.frustum();
    for (c, mut vis) in chunks.iter_mut() {
        let (lo, hi) = sim.terrain.grid.aabb(c.0);
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
// Bevy injects each resource independently into this system.
#[allow(clippy::too_many_arguments)]
fn snap(
    mut commands: Commands,
    time: Res<Time>,
    real: Res<Time<Real>>,
    mut record: ResMut<Recording>,
    sim: Res<Sim>,
    sel: Res<Sel>,
    mut windows: Query<&mut Window>,
    mut shot: ResMut<Shot>,
    mut exit: MessageWriter<AppExit>,
) {
    let sim_time = sim.r.world.tick as f64 * DT + sim.clock.fraction(DT) as f64 * DT;
    if let Some(status) = record.capture(
        &mut commands,
        real.elapsed_secs_f64(),
        sim.r.world.tick,
        sim_time,
    ) {
        exit.write(status);
    }
    shot.seen += 1;
    if let Ok(mut w) = windows.single_mut() {
        let live = sim.r.world.units.live().len();
        w.title = format!(
            "rts-vox — тик {} | выделено {} из {live} | приказов применено {} (ждут {}) | {}",
            sim.r.world.tick,
            sel.picked.len(),
            sim.r.world.orders_applied,
            sim.r.world.orders_pending(),
            if sim.speed == 0 {
                "пауза"
            } else {
                "идёт"
            }
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
            println!("blast matter: pending={} claims={} removed={} fragments={} flying={} fracture_and_motion_mj={} kinetic_mj={} balanced={}",
                sim.r.world.bursts.pending(), sim.r.world.bursts.claims(), sim.r.world.bursts.removed,
                sim.r.world.bursts.fragments.len(), sim.r.world.bursts.fragments.iter().filter(|f| !f.resting).count(),
                sim.r.world.bursts.spent_mj, sim.r.world.bursts.motion_mj,
                sim.r.world.econ.as_ref().is_none_or(|e| e.balanced()));
            println!(
                "body debris: removed={} fragments={} flying={} kinetic_mj={} pending_damage={}",
                sim.r.world.bursts.body_removed,
                sim.r
                    .world
                    .bursts
                    .fragments
                    .iter()
                    .filter(|f| f.paint.is_some())
                    .count(),
                sim.r
                    .world
                    .bursts
                    .fragments
                    .iter()
                    .filter(|f| f.paint.is_some() && !f.resting)
                    .count(),
                sim.r.world.bursts.body_motion_mj,
                sim.r
                    .world
                    .material_bodies
                    .as_ref()
                    .map_or(0, |b| b.pending.len())
            );
            if let Some(bodies) = &sim.r.world.material_bodies {
                println!(
                    "dead hulls: records={} moving={} voxels={}",
                    sim.r.world.bursts.corpses.len(),
                    sim.r
                        .world
                        .bursts
                        .corpses
                        .values()
                        .filter(|f| !f.resting)
                        .count(),
                    sim.r
                        .world
                        .bursts
                        .corpses
                        .values()
                        .map(|f| f.count as u64)
                        .sum::<u64>()
                );
                println!(
                    "sections: records={} voxels={} largest={} pending={}",
                    sim.r
                        .world
                        .bursts
                        .fragments
                        .iter()
                        .filter(|f| f.chunk.is_some())
                        .count(),
                    sim.r
                        .world
                        .bursts
                        .fragments
                        .iter()
                        .filter(|f| f.chunk.is_some())
                        .map(|f| f.count as u64)
                        .sum::<u64>(),
                    sim.r
                        .world
                        .bursts
                        .fragments
                        .iter()
                        .filter(|f| f.chunk.is_some())
                        .map(|f| f.count)
                        .max()
                        .unwrap_or(0),
                    bodies.detach_pending()
                );
                println!(
                    "material combat: bolts={} hit={} blocked={} beams={} beam_hits={} pending={}",
                    sim.r.world.shots_fired,
                    sim.r.world.shots_hit,
                    sim.r.world.shots_blocked,
                    sim.r.world.beams_fired,
                    sim.r.world.beams_hit,
                    bodies.pending.len()
                );
                for (i, state) in bodies.slots.iter().enumerate() {
                    let Some(b) = state else {
                        continue;
                    };
                    println!("body {i}: alive={} removed={} dented={} removed_mm3={} core={} engine={} weapon={} sensor={} at=({},{},{}) stance={}",
                        u.alive[i], b.removed.len(), b.dents.len(), b.removed_mm3,
                        b.status.alive, b.status.engine, b.status.weapon, b.status.sensor, u.x[i], u.y[i], u.z[i],u.stance[i]);
                    let mm = bodies.templates[b.template as usize].mm as u64;
                    let density = sim.r.world.tuning.chassis(u.chassis[i]).density.max(1) as u64;
                    println!("mobility {i}: mass_g={} engine_pct={} inverse_mass_raw={} inverse_yaw_raw={} speed_raw={}",
                        b.shape.count() as u64*(density*mm.pow(3)/1_000_000).max(1),
                        b.mobility.engine.mul(fx::Fx::from_int(100)).to_int_trunc(),b.mobility.inverse_mass.raw(),
                        b.mobility.inverse_yaw_inertia.raw(),u.vel[i].raw());
                }
            }
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(p));
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
        assert_eq!(
            nearest(&tie, at, PICK_PX),
            Some(3),
            "ничья ушла не меньшему"
        );
        // Дальше пальца — никого.
        assert_eq!(nearest(&c, Vec2::new(0.0, 0.0), PICK_PX), None);
    }

    /// Рамка ловит попавших внутрь при любом направлении протяжки.
    #[test]
    fn the_rectangle_does_not_care_which_way_it_was_dragged() {
        let (a, b) = (Vec2::new(10.0, 10.0), Vec2::new(50.0, 40.0));
        assert!(in_rect(Vec2::new(20.0, 20.0), a, b));
        assert!(
            in_rect(Vec2::new(20.0, 20.0), b, a),
            "протяжка вверх-влево не считается"
        );
        assert!(!in_rect(Vec2::new(60.0, 20.0), a, b));
    }

    /// Отрезок внутри ящика: снаружи луч не марширует вовсе.
    #[test]
    fn the_ray_is_clipped_to_the_world_box() {
        let hi = Vec3::new(10.0, 10.0, 10.0);
        let (t0, t1) = box_span(Vec3::new(-5.0, 5.0, 5.0), Vec3::X, hi).expect("луч входит в мир");
        assert!(
            (t0 - 5.0).abs() < 1e-3 && (t1 - 15.0).abs() < 1e-3,
            "{t0}..{t1}"
        );
        // Мимо: параллельно грани и вне её.
        assert!(box_span(Vec3::new(-5.0, 50.0, 5.0), Vec3::X, hi).is_none());
        // Изнутри: отрезок начинается с нуля.
        let (t0, _) = box_span(Vec3::new(5.0, 5.0, 5.0), Vec3::X, hi).expect("изнутри");
        assert!(t0 <= 0.0 + 1e-6);
    }
}
