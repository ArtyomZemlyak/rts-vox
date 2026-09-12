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
use render::model::SegFrame;
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

#[derive(Component)]
struct ShotMark;

#[derive(Component)]
struct BlastMark(u32);

/// Сегмент тела: меш по отделкам и место в теле (ADR-231).
struct SegArt {
    mesh: [Option<Handle<Mesh>>; look::FINISHES],
    frame: SegFrame,
}

/// Общие меши и материалы.
#[derive(Resource)]
struct Art {
    /// Меш корпуса **по отделке**: сталь, стекло и резина идут
    /// разными материалами, а материал у меша один (ADR-211).
    /// Пустая отделка ручки не получает и сущности не заводит.
    hull: [[Option<Handle<Mesh>>; look::FINISHES]; 2],
    turret: [[Option<Handle<Mesh>>; look::FINISHES]; 2],
    /// Повёрнутые части тела; едут с корпусом или с башней.
    segs: [Vec<SegArt>; 2],
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
    // Карта под шесть машин — соразмерная. На 384 они не встретятся
    // за разумное время, и смотреть будет не на что.
    let mut side = 128u32;
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
    let mut params = worldgen::Params::sized(side, side / 2, side);
    if relief > 0 {
        params.relief = relief;
    }
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
    println!(
        "ЛКМ — выделить, протяжка — рамка, Shift — добавить; \
ПКМ — идти или атаковать, Ctrl+ПКМ — копать; H — стоять, G — снять приказ, \
Пробел — пауза; WASD/QE/RF — камера, Esc — выход"
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
        .add_systems(Startup, setup)
        .add_systems(Update, (step, control, hands, overlay, cull, snap))
        .run();
}

fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
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
    let bodies = [voxel::unit::tank(), voxel::unit::strider()].map(|b| {
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
    });
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
            (!ubufs[k].idx.is_empty()).then(|| meshes.add(buf_to_mesh(&ubufs[k])))
        })
    };
    let tint = look::side_tint();
    let hull: [_; 2] = std::array::from_fn(|o| {
        unit_mesh(&bodies[o], tint[o], Slice::Hull, render::model::root_centre(&bodies[o]), &mut meshes)
    });
    let turret: [_; 2] = std::array::from_fn(|o| {
        unit_mesh(&bodies[o], tint[o], Slice::Turret, render::model::root_centre(&bodies[o]), &mut meshes)
    });
    // Сегменты: у каждого свой меш вокруг шарнира и своё место
    // в теле, сложенное по цепочке креплений (ADR-231).
    let per_mm = 1.0 / voxel::unit::WORLD_MM as f32;
    let segs: [Vec<SegArt>; 2] = std::array::from_fn(|o| {
        let frames = render::model::segment_frames(&bodies[o], per_mm);
        bodies[o]
            .segments
            .iter()
            .zip(frames)
            .map(|(s, frame)| SegArt {
                mesh: unit_mesh(
                    &s.body,
                    tint[o],
                    Slice::All,
                    render::model::pivot_centre(&s.attach),
                    &mut meshes,
                ),
                frame,
            })
            .collect()
    });
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
        segs,
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
        // Сегменты: метка корпуса или башни плюс рамка (ADR-231).
        for seg in &art.segs[owner] {
            for (k, m) in art.finish.iter().enumerate() {
                let Some(h) = seg.mesh[k].clone() else {
                    continue;
                };
                let mut e = commands.spawn((
                    Mesh3d(h),
                    MeshMaterial3d(m.clone()),
                    Transform::from_xyz(0.0, -1000.0, 0.0),
                    Visibility::Hidden,
                    seg.frame,
                ));
                if seg.frame.turret {
                    e.insert(TurretMark(i));
                } else {
                    e.insert(UnitMark(i));
                }
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
    // ambient, а не выдумывается заново.
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
            // сторону не нужны вовсе.
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
    (
        &'static UnitMark,
        &'static mut Transform,
        &'static mut Visibility,
        Option<&'static SegFrame>,
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
        sim.r.tick();
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

    for (m, mut tr, mut vis, seg) in units.iter_mut() {
        let i = m.0;
        if !alive(i) {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        if let Some(y) = hull_yaw(i) {
            render::model::place(&mut tr, at(i), Quat::from_rotation_y(y), seg);
        }
    }
    for (m, mut tr, mut vis, seg) in turrets.iter_mut() {
        let i = m.0;
        if !alive(i) {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        if let Some(y) = aim_yaw(i).or_else(|| hull_yaw(i)) {
            render::model::place(&mut tr, at(i), Quat::from_rotation_y(y), seg);
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
            sim.r.world.push_order(Order::MoveTo { unit: id, x: gx, z: gz });
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
