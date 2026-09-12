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

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::camera::Exposure;
#[cfg(feature = "rt")]
use bevy::camera::CameraMainTextureUsages;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::{
    Atmosphere, AtmosphereEnvironmentMapLight, CascadeShadowConfigBuilder,
    DirectionalLightShadowMap, SunDisk,
};
use bevy::mesh::Indices;
use bevy::anti_alias::fxaa::Fxaa;
use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::pbr::{AtmosphereSettings, ScreenSpaceAmbientOcclusion};
use bevy::post_process::bloom::Bloom;
use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode};
use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
#[cfg(feature = "rt")]
use bevy::render::render_resource::TextureUsages;
#[cfg(feature = "rt")]
use bevy::solari::pathtracer::{Pathtracer, PathtracingPlugin};
#[cfg(feature = "rt")]
use bevy::solari::prelude::{RaytracingMesh3d, SolariLighting, SolariPlugins};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};
use bevy::window::PresentMode;
use meshing::layer::MeshLayer;
use meshing::{Opts, CS};
use render::edge::{EdgePlugin, Edges};
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
    /// Ручки мешей **живых** чанков — тех, что сейчас в кольце вокруг
    /// камеры (ADR-228). Разрежённая карта, а не массив по всей сетке:
    /// на километре в ней два миллиона чанков, из которых живы тысячи.
    /// `BTreeMap`, а не `HashMap`: порядок обхода стабилен, и два
    /// прогона одного замера освобождают чанки в одном порядке.
    chunks: BTreeMap<u32, ChunkArt>,
    bufs: [MeshBuf; look::CLASSES],
    painter: Painter,
    /// Тиков симуляции на кадр. Ноль — пауза.
    speed: u32,
}

/// Что окно держит на один живой чанк.
///
/// Ручка меша **по классу поверхности**: у породы и руды разная
/// шероховатость, а её вершина нести не может (ADR-175). Сущность
/// рядом с ручкой, потому что освобождение обязано снять обе: ручка
/// без сущности оставляет буфер на видеокарте, сущность без ручки —
/// пустой вызов отрисовки.
#[derive(Default)]
struct ChunkArt {
    mesh: [Option<Handle<Mesh>>; look::CLASSES],
    ent: [Option<Entity>; look::CLASSES],
}

/// Состояние камеры между кадрами.
#[derive(Resource)]
struct View {
    cam: RtsCamera,
    visible: usize,
    /// Сколько вокселей в секунду камера едет сама, вдоль +X.
    /// Ноль — стоит. Только для замера: см. ключ `--pan`.
    pan: f32,
    /// Видимая даль в вокселях: туман, каскады теней и глубина резкости
    /// считаются от неё, а не абсолютными числами.
    ///
    /// Это **сторона мира, но не больше кольца** (ADR-228). Дальше
    /// кольца показывать нечего — там ничего не построено. Каскад
    /// тени длиной в километр рисовал бы пустоту и размазал бы
    /// разрешение карты теней по карте, из которой видно сто метров.
    /// На прежних картах показа кольцо шире мира, и это ровно сторона,
    /// как было до ADR-228.
    vista: f32,
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
    /// Кадр, на котором снимок был заказан. Запись у Bevy идёт
    /// **асинхронно**, и выход сразу после заказа теряет файл: так
    /// пропал снимок карты, которая строилась двенадцать минут.
    at: u64,
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
    /// Ребро воксела тела юнита, миллиметры.
    body_mm: u16,
    /// Сколько крупных машин на сторону. Стратегия масштабная, а строй
    /// из одних танков разницы масштабов не даёт: паук вдесятеро выше
    /// танка, и видно это только когда они в одном кадре.
    bigs: usize,
    /// Трассировка лучей: 0 — растеризация, 1 — освещение Solari
    /// в реальном времени, 2 — эталонный путь-трейсер.
    ///
    /// **Отдельный уровень качества, а не основа.** Нужна аппаратная
    /// трассировка; картинка обязана быть хорошей и без неё (ADR-222).
    rt: u8,
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

/// Кольцо чанков вокруг камеры: что построено, что ждёт очереди
/// и что пора забыть (ADR-228).
#[derive(Resource)]
struct Ring {
    /// Числа из `data/render.ron`.
    s: render::stream::Stream,
    /// Состояние каждого чанка сетки, байтом на чанк.
    ///
    /// Плотный массив, а не множество: на километре это два мегабайта,
    /// а запрос по индексу — одно обращение.
    state: Vec<u8>,
    /// **Столбы**, ждущие осмотра, и **чанки**, ждущие сборки.
    ///
    /// Единица осмотра — столб, а не чанк: осмотр идёт сверху вниз
    /// и кончается на первом сплошном чанке, потому что ниже всё
    /// закрыто породой. Иначе в очередь сборки попадают подземные
    /// пещеры, которых не видно никогда.
    ///
    /// Две очереди, а не одна: осмотр чанка стоит 7–25 мкс, сборка —
    /// 1.6 мс, и общий потолок пришлось бы мерить по дорогому случаю.
    ///
    /// FIFO: упёршаяся работа откладывается на следующий кадр,
    /// а не отбрасывается, и один дорогой столб не запирает за собой
    /// очередь (`CLAUDE.md`).
    to_probe: VecDeque<u32>,
    to_build: VecDeque<u32>,
    /// Чанк, в котором была камера при последнем пересчёте кольца.
    /// Пока камера в том же чанке, пересчитывать нечего: кольцо
    /// сдвигается не плавно, а через чанк.
    at: Option<[i32; 3]>,
    /// Переиспользуемый список кольца.
    scan: Vec<u32>,
    /// Сколько кадров подряд потолок сборки срабатывал.
    ///
    /// Потолок, срабатывающий **каждый** кадр, — не предохранитель,
    /// а постоянное ухудшение (ADR-114, ADR-116), и заметить это
    /// обязано окно, а не глаз.
    capped: u64,
    /// Кадров всего — чтобы отличить разогрев от установившегося режима.
    frames: u64,
    /// Итоги для отчёта.
    probed: u64,
    built: u64,
    freed: u64,
    /// Время трёх работ, миллисекунды, накопленное за весь прогон.
    ///
    /// Вместе со счётчиками оно даёт цену **одного** чанка. Без него
    /// потолки за кадр пришлось бы подбирать на глаз, а «осмотр дешевле
    /// сборки на два порядка» осталось бы утверждением, а не замером.
    t_probe: f64,
    t_build: f64,
    t_free: f64,
}

/// Чанк не осмотрен: неизвестно даже, есть ли у него грани.
const UNSEEN: u8 = 0;
/// Чанк осмотрен и пуст — чистый воздух. Граней нет, но осмотр
/// колонки на нём **не кончается**: ниже начнётся поверхность.
const AIR: u8 = 1;
/// Чанк осмотрен и сплошной — порода. Граней нет, и осмотр колонки
/// на нём **кончается**: всё, что ниже, закрыто породой сверху.
const ROCK: u8 = 2;
/// Чанк осмотрен, грани у него есть, и он ждёт своей очереди
/// на сборку. Осмотр не повторяется: он уже оплачен.
const QUEUED: u8 = 3;
/// Чанк построен: у него есть буферы и сущности.
const BUILT: u8 = 4;

/// Разбор кадра по работам ремешинга.
///
/// Без него общее время кадра ничего не объясняет: в нём и тик
/// симуляции, и пересборка квадов, и раскраска, и отдача мешей Bevy,
/// и все четыре меняются вместе.
#[derive(Resource, Default)]
struct Prof {
    /// Тики симуляции за кадр, миллисекунды.
    tick: Vec<f64>,
    /// Применение правок тика к слою мешинга, миллисекунды: развод
    /// по чанкам и сборка тех, что окно держит.
    apply: Vec<f64>,
    /// Счётчики фаз симуляции на момент прошлого отчёта — чтобы
    /// печатать разницу, а не накопленное с начала.
    phase_ns: Vec<u64>,
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

// Ребро воксела тела здесь не задаётся: его называет **само тело**
// (`UnitBody::mm`), а масштаб выводится делением на мировое ребро
// `voxel::unit::WORLD_MM`. Постоянная «ровно половина» тихо растянула
// бы машину, у которой ребро другое (ADR-219).

/// Сколько кадров подождать после заказа снимка, прежде чем выходить.
const SHOT_GRACE: u64 = 30;

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
    hull: [[Option<Handle<Mesh>>; look::FINISHES]; 4],
    turret: [[Option<Handle<Mesh>>; look::FINISHES]; 4],
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

/// Перевод набора чисел направления в компоненты камеры (ADR-214).
///
/// Одно место на всё окно: разнесённые по коду, эти шесть компонент
/// расходятся при первой же правке, и половина направления остаётся
/// от предыдущего.
fn tonemapping(t: look::Tonemap) -> Tonemapping {
    match t {
        look::Tonemap::Aces => Tonemapping::AcesFitted,
        look::Tonemap::Tony => Tonemapping::TonyMcMapface,
        look::Tonemap::AgX => Tonemapping::AgX,
        look::Tonemap::Filmic => Tonemapping::BlenderFilmic,
    }
}

fn section(s: look::Section) -> ColorGradingSection {
    ColorGradingSection {
        saturation: s.saturation,
        contrast: s.contrast,
        gamma: s.gamma,
        gain: s.gain,
        lift: s.lift,
    }
}

fn grading(g: look::Grade) -> ColorGrading {
    ColorGrading {
        global: ColorGradingGlobal {
            exposure: g.exposure,
            temperature: g.temperature,
            tint: g.tint,
            hue: 0.0,
            post_saturation: g.post_saturation,
            midtones_range: g.midtones_range.0..g.midtones_range.1,
        },
        shadows: section(g.shadows),
        midtones: section(g.midtones),
        highlights: section(g.highlights),
    }
}

fn main() {
    // Направление читается **до** всего остального: от него зависят
    // умолчания солнца, дымки и экспозиции, а они разбираются тут же.
    {
        let a: Vec<String> = std::env::args().collect();
        if let Some(i) = a.iter().position(|s| s == "--look") {
            match a.get(i + 1).and_then(|s| look::parse_look(s)) {
                Some(l) => look::set(l),
                None => {
                    eprintln!(
                        "неизвестное направление; есть: {}",
                        look::LOOK_ALL.map(look::look_name).join(", ")
                    );
                    return;
                }
            }
        }
    }
    let sc = look::scene();
    let mut units = 240usize;
    let mut seed = 42u64;
    // Показу нужен мир побольше, чем гейту: строй в полтысячи машин
    // на маленькой карте стоит стеной (ADR-172).
    let mut side = 384u32;
    // Высота мира. Ноль — половина стороны, как было.
    //
    // Ключ существует ради **замера километра** и только ради него
    // (ADR-228). Занятость мира `voxel::World::new` заполняет разом,
    // по вызову генератора на воксел, то есть за время `nx·ny·nz`.
    // При `ny = side/2` километр — это пять · 10¹¹ вокселей: тридцать
    // часов и 62.8 ГиБ. Уменьшив глубину, километр по X и Z можно
    // построить и померить **сегодня**, не трогая симуляцию.
    let mut height = 0u32;
    // Сколько вокселей в секунду камера едет сама, вдоль +X.
    //
    // Ключ для замера, а не для игры: без движения камеры кольцо
    // ничего не освобождает — из окна, запущенного из терминала,
    // WASD никто не нажимает, и путь освобождения остаётся
    // непроверенным (ADR-228).
    let mut pan = 0.0f32;
    // Кэш карты (ADR-228): `1` — каталог `cache/maps` в корне
    // репозитория, `0` — без кэша, иначе — свой каталог. Нетронутая
    // земля строится один раз и читается с диска за секунды; на
    // километре построить её стоит минуты.
    let mut cache = String::from("1");
    // Размах рельефа **не задаётся здесь числом**: его считает
    // `Params::sized` от высоты мира. Своё число у сцены уже дважды
    // расходилось с размером карты и давало плоский блин (ADR-157,
    // ADR-166), а третий раз — просто устаревало (ADR-221). Ключ
    // `--relief` по-прежнему перекрывает расчёт.
    let mut relief = 0u32;
    let mut speed = 1u32;
    let mut shot: Option<String> = None;
    let mut after = 8.0f32;
    let mut at_frame = 0u64;
    let mut frames = 0u64;
    let mut fx = Fx {
        cascades: sc.shadow_cascades,
        bloom: true,
        ssao: sc.ssao,
        paint: true,
        sky: true,
        chamfer: true,
        body_mm: 0,
        bigs: 0,
        rt: 0,
    };
    let mut sky = Sky {
        elevation: sc.sun_elevation,
        azimuth: sc.sun_azimuth,
        haze: sc.haze_metres_per_voxel,
        ev100: sc.ev100,
    };
    let (mut yaw, mut pitch, mut dist) = (0.7f32, 0.75f32, 0.45f32);
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--units" => units = v.parse().unwrap_or(units),
            "--side" => side = v.parse().unwrap_or(side),
            "--height" => height = v.parse().unwrap_or(height),
            "--pan" => pan = v.parse().unwrap_or(pan),
            "--cache" => cache = v,
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
            "--mm" => fx.body_mm = v.parse().unwrap_or(fx.body_mm),
            "--big" => fx.bigs = v.parse().unwrap_or(fx.bigs),
            "--rt" => fx.rt = v.parse().unwrap_or(fx.rt),
            "--sun" => sky.elevation = v.parse().unwrap_or(sky.elevation),
            "--sun-az" => sky.azimuth = v.parse().unwrap_or(sky.azimuth),
            "--haze" => sky.haze = v.parse().unwrap_or(sky.haze),
            "--ev" => sky.ev100 = v.parse().unwrap_or(sky.ev100),
            "--frames" => frames = v.parse().unwrap_or(frames),
            "--yaw" => yaw = v.parse().unwrap_or(yaw),
            "--pitch" => pitch = v.parse().unwrap_or(pitch),
            "--dist" => dist = v.parse().unwrap_or(dist),
            // Разобран выше: от него зависят умолчания остальных ключей.
            "--look" => {}
            "--help" => {
                println!(
                    "battle [--units N] [--side N] [--height N] [--relief N] [--seed N] [--pan вокселей-в-сек] \
[--cache 0|1|каталог] \
[--speed тиков-на-кадр] [--shot файл] [--shot-after сек] [--shot-frame N] [--frames N] \
[--yaw] [--pitch] [--dist] [--shadows каскадов] [--bloom 0|1] [--ssao 0|1] [--paint 0|1] \
[--sky 0|1] [--chamfer 0|1] [--mm ребро-тела-мм] [--big на-сторону] [--rt 0|1|2] \
[--sun высота] [--sun-az азимут] [--haze метров-на-воксель] [--ev EV100] \
[--look {}]",
                    look::LOOK_ALL.map(look::look_name).join("|")
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
    let ring_nums = {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/render.ron");
        let path = if here.exists() { here } else { std::path::PathBuf::from("data/render.ron") };
        match render::stream::Stream::load(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                return;
            }
        }
    };
    // Имя не `ny`: ниже `mesh.dims()` отдаёт **число слоёв чанков**,
    // и одноимённая переменная его затеняет. Такое затенение уже
    // опустило камеру на дно мира — из полутора тысяч вокселей высоты
    // получилось девять, и в кадре вместо рельефа были пещеры изнутри.
    let world_h = if height > 0 { height } else { side / 2 };
    let mut params = worldgen::Params::sized(side, world_h, side);
    if relief > 0 {
        params.relief = relief;
    }
    let cache_dir: Option<std::path::PathBuf> = match cache.as_str() {
        "0" => None,
        "1" => {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            Some(if root.join("data").exists() {
                root.join("cache/maps")
            } else {
                std::path::PathBuf::from("cache/maps")
            })
        }
        dir => Some(std::path::PathBuf::from(dir)),
    };
    // Карта — из кэша, если он есть, иначе из генератора и в кэш
    // (ADR-228). Кэшируется **нетронутая** земля: слой изменений —
    // это партия, и снимок берётся до первой правки, то есть до того,
    // как землю получит сценарий.
    let g = {
        let key = sim_core::mapcache::key(&params, seed, &tuning);
        let path = cache_dir.as_ref().map(|d| d.join(format!("map-{key:016x}.bin")));
        let cached = path.as_ref().and_then(|p| {
            let t = Instant::now();
            let bytes = match std::fs::read(p) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
                Err(e) => {
                    eprintln!("кэш карты {}: {e}; карта строится заново", p.display());
                    return None;
                }
            };
            match sim_core::mapcache::from_cache(
                &bytes,
                &params,
                seed,
                &tuning,
                sim_core::REF_MAX_EDITS,
            ) {
                Ok(g) => {
                    println!(
                        "карта из кэша {}: {:.0} МиБ за {:.0} мс",
                        p.display(),
                        bytes.len() as f64 / (1024.0 * 1024.0),
                        t.elapsed().as_secs_f64() * 1e3
                    );
                    Some(g)
                }
                Err(e) => {
                    eprintln!("кэш карты {}: {e}; карта строится заново", p.display());
                    None
                }
            }
        });
        match cached {
            Some(g) => g,
            None => {
                let t = Instant::now();
                let vw = voxel::World::new(
                    params.nx,
                    params.ny,
                    params.nz,
                    sim_core::REF_MAX_EDITS,
                    Box::new(worldgen::Terrain { params, seed }),
                );
                let t_bits = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                let g = sim_core::ground::Ground::new(vw, &tuning);
                println!(
                    "карта построена: занятость {t_bits:.0} мс, навигация {:.0} мс",
                    t.elapsed().as_secs_f64() * 1e3
                );
                if let Some(p) = &path {
                    let t = Instant::now();
                    let bytes = sim_core::mapcache::to_cache(&g, key);
                    let written = p
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| std::fs::write(p, &bytes));
                    match written {
                        Ok(()) => println!(
                            "кэш карты записан: {}, {:.0} МиБ за {:.0} мс",
                            p.display(),
                            bytes.len() as f64 / (1024.0 * 1024.0),
                            t.elapsed().as_secs_f64() * 1e3
                        ),
                        Err(e) => eprintln!("кэш карты {} не записан: {e}", p.display()),
                    }
                }
                g
            }
        }
    };
    let r = Reference::with_ground(seed, units, tuning, weapons, params, g);
    let t_world = t0.elapsed().as_secs_f64() * 1e3;
    let g = r.world.ground().expect("у эталона есть земля");
    let t = Instant::now();
    let mesh = MeshLayer::new(&g.world, Opts::default());
    let (nx, ny, nz) = mesh.dims();
    let grid = ChunkGrid { nx, ny, nz, cs: CS as u32 };
    let side = side as f32;
    println!(
        "мир {}×{}×{}, чанков {}, юнитов {units}, направление «{}»; \
мир {t_world:.0} мс, слой мешинга {:.0} мс",
        g.world.nx,
        g.world.ny,
        g.world.nz,
        grid.len(),
        look::look_name(look::active()),
        t.elapsed().as_secs_f64() * 1e3
    );
    println!(
        "кольцо: сборка {} вокселей, удержание {}, \
за кадр осмотр {} / сборка {} / освобождение {}",
        ring_nums.build_radius,
        ring_nums.keep_radius,
        ring_nums.probe_per_frame,
        ring_nums.build_per_frame,
        ring_nums.free_per_frame
    );

    // Видимая даль: дальше кольца ничего не построено, и рисовать
    // туда нечего. На прежних картах показа кольцо шире мира,
    // и `vista` равна стороне — картинка та же, что была.
    let vista = side.min(ring_nums.keep_radius as f32);
    let cam = RtsCamera {
        // Высота цели считается **от высоты мира**, а не от стороны:
        // генератор кладёт поверхность на половину высоты, и при
        // `world_h = side/2` это ровно прежнее `side * 0.28`.
        // На мелкой карте километра запись через сторону увела бы
        // камеру на два с половиной километра вверх.
        target: V3::new(side * 0.5, world_h as f32 * 0.56, side * 0.5),
        yaw,
        pitch,
        // От видимой дали, а не от стороны: на километре камера
        // отъезжала бы на четыре с половиной, и в кадре стояло бы
        // одно кольцо посреди пустоты. На прежних картах показа
        // даль равна стороне, и это прежнее число.
        distance: vista * dist,
        fovy: 0.87,
        aspect: 16.0 / 9.0,
        near: 1.0,
        far: vista * 4.0,
    };

    let n = grid.len();
    let rt = fx.rt;
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "rts-vox — бой".into(),
                present_mode: PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        // Обводка силуэта: проход ставится всегда, а работает только
        // там, где на камере есть `Edges` (ADR-214).
        .add_plugins(EdgePlugin)
        .insert_resource(fx)
        .insert_resource(sky)
        .insert_resource(Prof::default())
        .insert_resource(Seen::default())
        .insert_resource(DirectionalLightShadowMap { size: sc.shadow_map })
        .insert_resource(GlobalAmbientLight {
            color: srgb(sc.ambient_srgb),
            brightness: sc.ambient_brightness,
            ..default()
        })
        .insert_resource(Sim {
            r,
            mesh,
            grid,
            chunks: BTreeMap::new(),
            bufs: class_bufs(),
            painter: Painter::default(),
            speed,
        })
        .insert_resource(Ring {
            s: ring_nums,
            state: vec![UNSEEN; n],
            to_probe: VecDeque::new(),
            to_build: VecDeque::new(),
            at: None,
            scan: Vec::new(),
            capped: 0,
            frames: 0,
            probed: 0,
            built: 0,
            freed: 0,
            t_probe: 0.0,
            t_build: 0.0,
            t_free: 0.0,
        })
        .insert_resource(View { cam, visible: 0, pan, vista })
        .insert_resource(Shot {
            path: shot,
            after,
            at_frame,
            done: false,
            at: 0,
            frames,
            seen: 0,
            hist: Vec::new(),
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (step, effects, control, cull, snap))
        // Порядок здесь не косметика: кольцо считается от камеры,
        // которую только что подвинул `control`, а отсекает
        // построенное уже `cull`. Перепутанный порядок даёт кольцо
        // от прошлого кадра и чанк, видимый на кадр позже.
        .add_systems(Update, stream.after(control).before(cull))
        ;
    // Трассировка подключается **плагинами**, а не полем набора: без
    // аппаратной поддержки их лучше не добавлять вовсе.
    #[cfg(feature = "rt")]
    if rt > 0 {
        app.add_plugins(SolariPlugins);
        if rt == 2 {
            app.add_plugins(PathtracingPlugin);
        }
    }
    #[cfg(not(feature = "rt"))]
    if rt > 0 {
        eprintln!("--rt просит трассировку, а собрано без неё: нужна сборка --features rt");
    }
    app.run();
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
    // Рельеф здесь больше не строится — его подгружает кольцо
    // (ADR-228), — поэтому симуляция читается, а не правится.
    sim: Res<Sim>,
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
    // Пересэмплирование **не украшение**: тело хранит своё ребро
    // в миллиметрах, и мешер обязан ставить воксел в мировых единицах,
    // а не в клетках. Отказ пересэмплировать — не повод рисовать
    // не то: тело остаётся в прежнем масштабе и говорит об этом.
    let mm = fx.body_mm;
    // **Ноль значит «как нарисовано»**, и это умолчание: тело знает своё
    // ребро само (ADR-219). Ключ `--mm` пересэмплирует принудительно —
    // он остался, чтобы сравнивать разрешения не по памяти, а рядом.
    // Четыре тела, а не два: обычная машина стороны и крупная.
    // Номер набора — `сторона + 2·крупная`. Разница масштабов видна
    // только когда паук и танк стоят в одном кадре.
    let bodies = [
        voxel::unit::tank(),
        voxel::unit::strider(),
        voxel::roster::siege_walker(),
        voxel::roster::commander(),
    ]
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
    });
    let uv = fx.chamfer;
    let rt = fx.rt;
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
            body.mm as f32 / voxel::unit::WORLD_MM as f32,
            layers,
            render::model::BEVEL,
            &mut ubufs,
        );
        // Пустая отделка ручки не получает: у безбашенного тела
        // башня пуста целиком, и заводить под неё сущность
        // с нулевым мешем незачем.
        std::array::from_fn(|k| {
            (!ubufs[k].idx.is_empty()).then(|| {
                let mut m = buf_to_mesh(&ubufs[k], uv);
                if rt > 0 {
                    pad_for_raytracing(&mut m);
                }
                meshes.add(m)
            })
        })
    };
    let tint = look::side_tint();
    let hull = [
        unit_mesh(&bodies[0], tint[0], Slice::Hull, &mut meshes),
        unit_mesh(&bodies[1], tint[1], Slice::Hull, &mut meshes),
        unit_mesh(&bodies[2], tint[0], Slice::Hull, &mut meshes),
        unit_mesh(&bodies[3], tint[1], Slice::Hull, &mut meshes),
    ];
    let turret = [
        unit_mesh(&bodies[0], tint[0], Slice::Turret, &mut meshes),
        unit_mesh(&bodies[1], tint[1], Slice::Turret, &mut meshes),
        unit_mesh(&bodies[2], tint[0], Slice::Turret, &mut meshes),
        unit_mesh(&bodies[3], tint[1], Slice::Turret, &mut meshes),
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
    let rt_paint = fx.rt > 0;
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
            // Цвет вершины трассировка не видит: в её формате вершины
            // цвета нет. Поэтому на трассированном пути класс красится
            // своим цветом целиком (ADR-222).
            base_color: if rt_paint {
                srgb(look::class_srgb(c))
            } else if painted {
                Color::WHITE
            } else {
                srgb(FLAT_GROUND)
            },
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

    // Рельефа здесь нет вовсе, и это главное в ADR-228: он подгружается
    // кольцом вокруг камеры, кадр за кадром, под потолком работы.
    // Прежняя редакция строила все чанки разом — на 192 метрах это
    // двенадцать минут ожидания, а на километре не кончилось бы никогда.

    // Юниты: корпус и башня отдельными сущностями. Число мест
    // фиксировано — юниты гибнут и воскресают в тех же местах
    // (свободный список).
    let cap = sim.r.world.units.len().max(1);
    for i in 0..cap {
        let owner = usize::from(sim.r.world.units.owner.get(i).copied().unwrap_or(0) == 1);
        // Первые слоты каждой стороны отдаются крупным машинам: места
        // в мире фиксированы, поэтому и набор тел выбирается по номеру.
        let owner = owner + if i / 2 < fx.bigs { 2 } else { 0 };
        // По сущности на отделку. Системы движения ищут по метке
        // и обновляют их все разом одним и тем же преобразованием:
        // корпус остаётся корпусом, из скольких бы материалов он
        // ни был собран.
        for (k, m) in art.finish.iter().enumerate() {
            if let Some(h) = art.hull[owner][k].clone() {
                let e = commands
                    .spawn((
                        Mesh3d(h.clone()),
                        MeshMaterial3d(m.clone()),
                        Transform::from_xyz(0.0, -1000.0, 0.0),
                        Visibility::Hidden,
                        UnitMark(i),
                    ))
                    .id();
                #[cfg(feature = "rt")]
                if fx.rt > 0 {
                    commands.entity(e).insert(RaytracingMesh3d(h.clone()));
                }
                let _ = (e, h);
            }
            if let Some(h) = art.turret[owner][k].clone() {
                let e = commands
                    .spawn((
                        Mesh3d(h.clone()),
                        MeshMaterial3d(m.clone()),
                        Transform::from_xyz(0.0, -1000.0, 0.0),
                        Visibility::Hidden,
                        TurretMark(i),
                    ))
                    .id();
                #[cfg(feature = "rt")]
                if fx.rt > 0 {
                    commands.entity(e).insert(RaytracingMesh3d(h.clone()));
                }
                let _ = (e, h);
            }
        }
    }

    let sc = look::scene();
    let eye = view.cam.eye();
    let t = view.cam.target;
    // Дальности картинки считаются от **видимой дали**, а не от стороны
    // мира: за кольцом ничего не построено (ADR-228). На прежних картах
    // показа кольцо шире мира, и это ровно прежнее число.
    let side = view.vista;
    let cam = commands
        .spawn((
            Camera3d::default(),
            Camera {
                // Небо за краем мира на случай, когда атмосферы нет:
                // её плагин требует вычислительных шейдеров и на машине
                // без них молча не загрузится.
                clear_color: ClearColorConfig::Custom(srgb(sc.sky_colour_srgb)),
                ..default()
            },
            // Тонмаппинг тут не украшение: свет задан в люксах, и без
            // него всё ярче половины улетает в белое. Кривая — от
            // направления: ACES дерзкая и контрастная, AgX плёночная
            // и приглушённая, и это половина разницы между ними
            // (ADR-214).
            tonemapping(sc.tonemap),
            // Цветокоррекция — **после** расчёта света, поэтому она
            // красит кадр, ничего не ломая в освещении.
            grading(sc.grade),
            // Экспозиция идёт в паре с яркостью солнца: 130 000 люкс
            // на входе атмосферы — это не «ярче», а «до фильтрации»
            // (ADR-198).
            Exposure { ev100: sky.ev100 },
            Transform::from_xyz(eye.x, eye.y, eye.z).looking_at(Vec3::new(t.x, t.y, t.z), Vec3::Y),
        ))
        .id();
    // Трассировка пишет в изображение кадра как в хранилище и не терпит
    // многократной выборки — оба требования её, а не наши.
    #[cfg(feature = "rt")]
    if fx.rt > 0 {
        commands.entity(cam).insert((
            CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING),
            Msaa::Off,
        ));
        if fx.rt == 2 {
            commands.entity(cam).insert(Pathtracer::default());
        } else {
            commands.entity(cam).insert(SolariLighting::default());
        }
    }
    // Трассеры и лучи светятся сильнее единицы — свечение делает
    // видимым то, ради чего они и заведены. Отдельным вставлением,
    // а не полем набора: компонент либо есть, либо его нет.
    if fx.bloom && sc.bloom > 0.0 {
        commands.entity(cam).insert(Bloom { intensity: sc.bloom, ..Bloom::NATURAL });
    }
    if fx.ssao {
        commands
            .entity(cam)
            .insert((Msaa::Off, ScreenSpaceAmbientOcclusion::default(), Fxaa::default()));
    }
    if sc.vignette.intensity > 0.0 {
        let v = sc.vignette;
        commands.entity(cam).insert(Vignette {
            intensity: v.intensity,
            radius: v.radius,
            smoothness: v.smoothness,
            roundness: v.roundness,
            color: srgb(v.srgb),
            ..default()
        });
    }
    // Обводка силуэта — по глубине и нормалям префасса (`render::edge`).
    // Компонент тянет за собой префасс сам, поэтому сглаживание
    // по подвыборкам приходится выключить и добрать FXAA — ровно как
    // у щелевого затенения.
    if sc.edge.strength > 0.0 {
        let e = sc.edge;
        let c = look::to_linear(e.srgb);
        commands.entity(cam).insert((
            Msaa::Off,
            Fxaa::default(),
            Edges {
                colour: Vec4::new(c[0], c[1], c[2], 1.0),
                width: e.width,
                depth: e.depth,
                normal: e.normal,
                strength: e.strength,
                steps: e.steps,
                posterise: e.posterise,
                pad0: 0.0,
                pad1: 0.0,
            },
        ));
    }
    // Глубина резкости — признак макета, а не украшение: глаз читает
    // размытый дальний план как «снято вблизи», то есть «вещь
    // маленькая». Диафрагма здесь **неправдоподобно широкая**, и это
    // умышленно: на честной оптике с фокусом в сотне метров кружок
    // рассеяния выходит в сотую долю пикселя, то есть эффекта нет.
    if sc.dof_f_stops > 0.0 {
        commands.entity(cam).insert((
            DepthPrepass,
            DepthOfField {
                mode: DepthOfFieldMode::Gaussian,
                focal_distance: view.cam.distance,
                aperture_f_stops: sc.dof_f_stops,
                // Небо Bevy считает бесконечно далёким, и без потолка
                // кружок рассеяния на нём был бы бесконечным.
                max_depth: side * 1.5,
                ..default()
            },
        ));
    }
    if fx.sky {
        // Атмосфера считается на камеру: её настройки — это размеры
        // таблиц и число выборок, то есть цена, а не вид.
        commands.entity(cam).insert((
            AtmosphereSettings {
                aerial_view_lut_max_distance: sc.haze_range_m,
                ..default()
            },
            // Отражения и заполняющий свет **снимаются с настоящего
            // неба**. Это и есть ответ на «руда без карты окружения
            // выходит чёрной» из ADR-175: карта окружения появилась.
            AtmosphereEnvironmentMapLight {
                intensity: sc.sky_ibl,
                size: UVec2::splat(sc.sky_ibl_size),
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
            color: srgb(sc.sky_colour_srgb),
            falloff: FogFalloff::Linear { start: side * 0.35, end: side * 1.15 },
            ..default()
        });
    }

    // Солнце: белое, с каскадными тенями и видимым диском. Тень здесь
    // не эффект — без неё рельеф читается как плоская заливка, а юнит
    // не стоит на земле, а висит над ней.
    commands.spawn((
        DirectionalLight {
            color: srgb(sc.sun_srgb),
            illuminance: sc.sun_lux,
            // Трассировка **заменяет** карты теней: держать обе значит
            // платить дважды за одну тень.
            shadow_maps_enabled: fx.cascades > 0 && fx.rt == 0,
            // Воксельный мир — сплошные плоские грани, и на них акне
            // теней видно лучше всего. Смещение подобрано по картинке.
            shadow_depth_bias: 0.06,
            shadow_normal_bias: 2.0,
            ..default()
        },
        // Диск солнца рисует атмосфера; без неё компонент не значит
        // ничего и потому ставится всегда.
        SunDisk {
            angular_size: sc.sun_disk_size,
            intensity: sc.sun_disk_intensity,
        },
        Transform::default()
            .looking_to(Vec3::from(look::sun_dir_at(sky.elevation, sky.azimuth)), Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: fx.cascades.max(1),
            minimum_distance: 1.0,
            first_cascade_far_bound: side * sc.shadow_first_frac,
            maximum_distance: side * sc.shadow_max_frac,
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

/// Привести меш к тому, что принимает трассировка.
///
/// **Список атрибутов обязан совпадать ровно**: позиция, нормаль, UV,
/// касательная — и больше ничего. Проверка в `bevy_solari` буквальная
/// (`is_mesh_raytracing_compatible`), и лишний атрибут не игнорируется,
/// а **выбрасывает меш из трассировки целиком**: структура ускорения
/// для него не строится, лучи проходят насквозь, и сцена выходит ровной
/// заливкой неба. Это и случилось с нашим рельефом, у которого цвет
/// лежит в вершине (ADR-222).
fn pad_for_raytracing(m: &mut Mesh) {
    let n = m.count_vertices();
    if m.contains_attribute(Mesh::ATTRIBUTE_COLOR) {
        m.remove_attribute(Mesh::ATTRIBUTE_COLOR);
    }
    if !m.contains_attribute(Mesh::ATTRIBUTE_UV_0) {
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32, 0.0]; n]);
    }
    if !m.contains_attribute(Mesh::ATTRIBUTE_TANGENT) {
        m.insert_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0f32, 0.0, 0.0, 1.0]; n]);
    }
}

/// Меш чанка рельефа с оглядкой на трассировку.
fn chunk_mesh(b: &MeshBuf, uv: bool, rt: u8) -> Mesh {
    let mut m = buf_to_mesh(b, uv || rt > 0);
    if rt > 0 {
        pad_for_raytracing(&mut m);
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
    mut ring: ResMut<Ring>,
    art: Res<Art>,
    fx: Res<Fx>,
    mut prof: ResMut<Prof>,
    mut units: HullQuery,
    mut turrets: TurretQuery,
    shots: Query<Entity, With<ShotMark>>,
    mut seen: ResMut<Seen>,
) {
    let t = Instant::now();
    for _ in 0..sim.speed {
        sim.r.tick();
    }
    prof.tick.push(t.elapsed().as_secs_f64() * 1e3);

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
        // Нерезидентный чанк строится, только если окно его держит:
        // копают и за кольцом, а там сборка ушла бы в никуда — чанк
        // забылся бы в этом же кадре и собрался бы заново в следующем
        // (ADR-228).
        let t = Instant::now();
        mesh.apply_where(&g.world, &edits, |i| chunks.contains_key(&(i as u32)));
        prof.apply.push(t.elapsed().as_secs_f64() * 1e3);
        let dirty: Vec<usize> = mesh.dirty().iter().map(|&i| i as usize).collect();
        let w = &g.world;
        prof.dirty.push(dirty.len() as f64);
        let (mut t_quads, mut t_paint, mut t_up) = (0.0f64, 0.0f64, 0.0f64);
        for i in dirty {
            // Копают и за кольцом: карта в километр, а видно сто метров.
            // Чанк, которого окно не держит, **забывается тут же** —
            // иначе слой копит по 726 КиБ на каждый когда-либо
            // задетый чанк до конца партии (ADR-228). Дыра никуда
            // не денется: мир авторитетен, и войдя в кольцо, чанк
            // соберётся из него вместе с ней.
            let Some(art_slot) = chunks.get_mut(&(i as u32)) else {
                mesh.forget(i);
                // Осмотренный и **пустой** чанк мог перестать быть
                // пустым: его копнули. Обратно в очередь осмотра —
                // иначе дыра появится только когда камера переедет
                // в соседний чанк. Чанк за кольцом уже `UNSEEN`
                // и в очередь не попадёт.
                //
                // Чанк в состоянии `QUEUED` не трогается: осмотр
                // за него уже оплачен, и сбрасывать его значило бы
                // гонять по кругу ровно то, что копают. На карте
                // 1920³ так не построился ни один чанк забоя: они
                // ждали сборки и возвращались в осмотр каждый кадр,
                // а в кадре на их месте стояли серые дыры.
                if ring.state[i] == AIR || ring.state[i] == ROCK {
                    ring.state[i] = UNSEEN;
                    let (cx, cz) =
                        (i as u32 % grid.nx, i as u32 / (grid.nx * grid.ny));
                    ring.to_probe.push_back(render::stream::column(&grid, cx, cz));
                }
                continue;
            };
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
            for (k, b) in bufs.iter().enumerate() {
                let m = chunk_mesh(b, fx.chamfer, fx.rt);
                match art_slot.mesh[k].clone() {
                    Some(h) => {
                        let _ = meshes.insert(h.id(), m);
                    }
                    None => {
                        if b.is_empty() {
                            continue;
                        }
                        let h = meshes.add(m);
                        art_slot.mesh[k] = Some(h.clone());
                        let o = chunk_origin(i, grid.nx, grid.ny);
                        art_slot.ent[k] = Some(
                            commands
                                .spawn((
                                    Mesh3d(h),
                                    MeshMaterial3d(art.ground[k].clone()),
                                    Transform::from_xyz(o[0], o[1], o[2]),
                                    Chunk(i),
                                ))
                                .id(),
                        );
                    }
                }
            }
            t_up += t.elapsed().as_secs_f64() * 1e3;
        }
        prof.quads.push(t_quads);
        prof.paint.push(t_paint);
        if prof.paint.len() % 120 == 0 {
            println!(
                "раскраска: квадов {}, из них резаных {}, выборок {}, выпущено {}",
                painter.quads, painter.split, painter.samples, painter.emitted
            );
            painter.quads = 0;
            painter.split = 0;
            painter.samples = 0;
            painter.emitted = 0;
        }
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
    // **Поворот корпуса — это курс плюс наклон.** Наклон считает
    // подвеска (ADR-225), рендер его только читает: местный «верх»
    // тела кладётся на нормаль корпуса, а нос — на курс, спрямлённый
    // поперёк этой нормали. Без второго нос уезжал бы под землю
    // на склоне.
    let hull_rot = |i: usize, yaw: f32| -> Quat {
        let n = u.up[i];
        let up = Vec3::new(
            n.x.to_f64_lossy() as f32,
            n.y.to_f64_lossy() as f32,
            n.z.to_f64_lossy() as f32,
        )
        .try_normalize()
        .unwrap_or(Vec3::Y);
        let f = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
        let f = (f - up * f.dot(up)).try_normalize().unwrap_or(Vec3::X);
        Quat::from_mat3(&Mat3::from_cols(f, up, f.cross(up)))
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
            tr.rotation = hull_rot(i, y);
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
        // Башня наклоняется вместе с корпусом — она на нём стоит, —
        // но смотрит в свою сторону.
        if let Some(y) = aim_yaw(i).or_else(|| hull_yaw(i)) {
            tr.rotation = hull_rot(i, y);
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
    // Своим ходом — до края карты и обратно незачем: замеру нужно,
    // чтобы кольцо ехало и что-то освобождало.
    if view.pan != 0.0 {
        view.cam.target.x += view.pan * dt;
    }
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

/// Чанк, в котором лежит точка. Целые со знаком, а не индекс сетки:
/// камера бывает и за краем карты, а кольцо обязано заметить, что она
/// переехала, и там.
fn chunk_of(g: ChunkGrid, p: V3) -> [i32; 3] {
    let s = g.cs as f32;
    [
        (p.x / s).floor() as i32,
        (p.y / s).floor() as i32,
        (p.z / s).floor() as i32,
    ]
}

/// Потоковая подгрузка рельефа: кольцо чанков вокруг камеры (ADR-228).
///
/// Кадр делится на три работы, и у каждой свой потолок из
/// `data/render.ron`:
///
/// 1. **Осмотр** — есть ли у чанка грани вообще. Однородный объём
///    (сплошная порода, чистый воздух) не даёт ни одного квада,
///    и таких в кольце большинство.
/// 2. **Сборка** — мешинг, раскраска материалом и отдача буфера.
/// 3. **Освобождение** — чанк ушёл дальше радиуса удержания.
///
/// Не влезшее в потолок **откладывается на следующий кадр**, а не
/// бросается: очередь FIFO, и упёршийся чанк не запирает за собой
/// остальные (`CLAUDE.md`).
#[allow(clippy::too_many_arguments)]
fn stream(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sim: ResMut<Sim>,
    mut ring: ResMut<Ring>,
    art: Res<Art>,
    fx: Res<Fx>,
    view: Res<View>,
) {
    ring.frames += 1;
    let grid = sim.grid;
    let centre = view.cam.target;

    // Пересчёт кольца — только когда камера переехала в другой чанк.
    // Кольцо сдвигается через чанк, а не плавно, и пересчитывать его
    // каждый кадр значило бы сортировать десятки тысяч расстояний
    // ради того же самого списка.
    let here = chunk_of(grid, centre);
    if ring.at != Some(here) {
        ring.at = Some(here);
        let r = ring.s.build_radius as f32;
        let mut scan = core::mem::take(&mut ring.scan);
        render::stream::ring(&grid, centre, r, &mut scan);
        ring.to_probe.clear();
        ring.to_build.clear();
        // Осмотренные чанки повторно не осматриваются: обход столба
        // проскакивает их по состоянию. А те, что уже ждали сборки,
        // возвращаются прямо в её очередь — осмотр за них оплачен.
        for &c in scan.iter() {
            ring.to_probe.push_back(c);
            let (cx, cz) = render::stream::column_at(&grid, c);
            for cy in 0..grid.ny {
                let k = grid.index(cx, cy, cz);
                if ring.state[k] == QUEUED {
                    ring.to_build.push_back(k as u32);
                }
            }
        }
        ring.scan = scan;
    }

    let (build_r, keep_r) = (ring.s.build_radius as f32, ring.s.keep_radius as f32);
    let Sim { r, mesh, chunks, bufs, painter, .. } = &mut *sim;
    let w = &r.world.ground().expect("земля").world;

    // 1. Осмотр — **столбами, сверху вниз, до первой сплошной породы**.
    //
    // Ниже неё смотреть не на что: всё закрыто сверху. Прокопанный
    // столб перестаёт быть сплошным сам, и дно едет вниз вместе
    // с забоем.
    let t = Instant::now();
    let mut left = ring.s.probe_per_frame as i64;
    while left > 0 {
        let Some(c) = ring.to_probe.pop_front() else { break };
        // Пока столб ждал очереди, кольцо могло уехать. Пропуск тут
        // дешевле, чем чистка очереди.
        if !render::stream::within(&grid, c, centre, build_r) {
            continue;
        }
        let (cx, cz) = render::stream::column_at(&grid, c);
        // Столб обходится до конца, даже если потолок уже выбран:
        // перерасход ограничен высотой сетки, а половина столба
        // на экране — это дыра в рельефе.
        for cy in (0..grid.ny).rev() {
            let k = grid.index(cx, cy, cz);
            match ring.state[k] {
                UNSEEN => {}
                // Дно столба: дальше вниз не смотрим.
                ROCK => break,
                _ => continue,
            }
            left -= 1;
            ring.probed += 1;
            match mesh.probe(w, k) {
                meshing::layer::Fill::Air => ring.state[k] = AIR,
                meshing::layer::Fill::Rock => {
                    ring.state[k] = ROCK;
                    break;
                }
                meshing::layer::Fill::Mixed => {
                    ring.state[k] = QUEUED;
                    ring.to_build.push_back(k as u32);
                }
            }
        }
    }
    ring.t_probe += t.elapsed().as_secs_f64() * 1e3;

    // 2. Сборка.
    let t = Instant::now();
    let want = ring.to_build.len();
    let mut done = 0u32;
    while done < ring.s.build_per_frame {
        let Some(i) = ring.to_build.pop_front() else { break };
        let k = i as usize;
        let (cx, _, cz) = (k as u32 % grid.nx, 0u32, k as u32 / (grid.nx * grid.ny));
        if ring.state[k] != QUEUED
            || !render::stream::within(&grid, render::stream::column(&grid, cx, cz), centre, build_r)
        {
            continue;
        }
        done += 1;
        ring.built += 1;
        let quads = mesh.scratch_quads(w, k);
        paint(quads, k, grid, w, fx.paint, painter, bufs);
        let mut slot = ChunkArt::default();
        let o = chunk_origin(k, grid.nx, grid.ny);
        for (c, b) in bufs.iter().enumerate() {
            if b.is_empty() {
                continue;
            }
            let h = meshes.add(chunk_mesh(b, fx.chamfer, fx.rt));
            let e = commands
                .spawn((
                    Mesh3d(h.clone()),
                    MeshMaterial3d(art.ground[c].clone()),
                    Transform::from_xyz(o[0], o[1], o[2]),
                    Chunk(k),
                ))
                .id();
            #[cfg(feature = "rt")]
            if fx.rt > 0 {
                commands.entity(e).insert(RaytracingMesh3d(h.clone()));
            }
            slot.mesh[c] = Some(h);
            slot.ent[c] = Some(e);
        }
        // Грани были, а буферов не вышло: все они у нижней грани,
        // которую `Opts::skip_bottom` не выпускает. Держать под это
        // запись незачем.
        if slot.mesh.iter().all(|h| h.is_none()) {
            ring.state[k] = AIR;
            continue;
        }
        ring.state[k] = BUILT;
        chunks.insert(i, slot);
    }
    ring.t_build += t.elapsed().as_secs_f64() * 1e3;
    // Потолок, срабатывающий **каждый** кадр, — не предохранитель,
    // а постоянное ухудшение (ADR-114, ADR-116). Считаем, чтобы это
    // было видно числом, а не на глаз.
    if want > ring.s.build_per_frame as usize {
        ring.capped += 1;
    }

    // 3. Освобождение. Обход идёт по живым чанкам, а не по сетке:
    // живых тысячи, в сетке километра — два миллиона.
    let t = Instant::now();
    let mut stale: Vec<u32> = Vec::new();
    for &i in chunks.keys() {
        if stale.len() >= ring.s.free_per_frame as usize {
            break;
        }
        let (cx, cz) = (i % grid.nx, i / (grid.nx * grid.ny));
        if !render::stream::within(&grid, render::stream::column(&grid, cx, cz), centre, keep_r) {
            stale.push(i);
        }
    }
    for i in stale {
        let Some(slot) = chunks.remove(&i) else { continue };
        for e in slot.ent.into_iter().flatten() {
            commands.entity(e).despawn();
        }
        // Ручки уходят вместе с записью: последняя снятая ручка
        // освобождает и буфер на видеокарте. Инкрементальное
        // состояние — отдельно, его держит слой мешинга.
        mesh.forget(i as usize);
        ring.state[i as usize] = UNSEEN;
        ring.freed += 1;
    }
    ring.t_free += t.elapsed().as_secs_f64() * 1e3;
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
    ring: Res<Ring>,
    mut prof: ResMut<Prof>,
    mut shot: ResMut<Shot>,
    mut exit: MessageWriter<AppExit>,
) {
    shot.seen += 1;
    if !shot.done && time.elapsed_secs() >= shot.after && shot.seen >= shot.at_frame {
        if let Some(p) = shot.path.clone() {
            println!(
                "снимок на {:.1} с, кадр {}, тик {}: живых {}, снарядов {}, взрывов {}; \
кольцо: живых чанков {}, ждут осмотра {}, ждут сборки {}",
                time.elapsed_secs(),
                shot.seen,
                sim.r.world.tick,
                sim.r.world.units.live().len(),
                sim.r.world.shots.alive_count(),
                sim.r.world.blasts,
                sim.chunks.len(),
                ring.to_probe.len(),
                ring.to_build.len()
            );
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        }
        shot.done = true;
        shot.at = shot.seen;
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
симуляция {:.2}, применение {:.2}; грязных {:.0}, квады {:.2}, раскраска {:.2}, \
отдача {:.2} мс; тик {}",
                shot.seen - 119,
                shot.seen,
                v[60],
                v[118],
                v[119],
                Prof::med(&prof.tick, 120),
                Prof::med(&prof.apply, 120),
                Prof::med(&prof.dirty, 120),
                Prof::med(&prof.quads, 120),
                Prof::med(&prof.paint, 120),
                Prof::med(&prof.upload, 120),
                sim.r.world.tick
            );
            // Фазы симуляции — по её собственным счётчикам, разницей
            // с прошлого отчёта. Тик на километре рос со временем,
            // и без разбивки нельзя было сказать, где (ADR-228).
            {
                let now: Vec<u64> = sim_core::PHASES
                    .iter()
                    .map(|&p| sim.r.world.phase_ns_of(p))
                    .collect();
                if prof.phase_ns.len() == now.len() {
                    let mut d: Vec<(f64, &str)> = now
                        .iter()
                        .zip(prof.phase_ns.iter())
                        .zip(sim_core::PHASES.iter())
                        .map(|((a, b), p)| (a.saturating_sub(*b) as f64 / 120.0 / 1e6, p.name()))
                        .collect();
                    let total: f64 = d.iter().map(|x| x.0).sum();
                    d.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal));
                    let top: Vec<String> =
                        d.iter().take(5).map(|(t, n)| format!("{n} {t:.2}")).collect();
                    println!("  фазы тика за кадр: всего {total:.2} мс; {}", top.join(", "));
                }
                prof.phase_ns = now;
            }
            // Кольцо отчитывается **вместе** с кадром: очередь, которая
            // не пустеет, и потолок, который срабатывает каждый кадр, —
            // это не предохранитель, а постоянное ухудшение (ADR-114,
            // ADR-116), и увидеть его надо числом.
            println!(
                "  кольцо: живых {}, осмотрено {}, собрано {}, освобождено {}; \
очередь осмотра {}, сборки {}; потолок сборки сработал в {} кадрах из {}",
                sim.chunks.len(),
                ring.probed,
                ring.built,
                ring.freed,
                ring.to_probe.len(),
                ring.to_build.len(),
                ring.capped,
                ring.frames
            );
            // Цена одного чанка — частное накопленного времени
            // и счётчика. Именно она задаёт потолки за кадр, и без неё
            // их пришлось бы подбирать на глаз.
            let per = |t: f64, n: u64| if n == 0 { 0.0 } else { t * 1e3 / n as f64 };
            println!(
                "  цена чанка: осмотр {:.1} мкс, сборка {:.1} мкс, освобождение {:.1} мкс; \
за прогон {:.0} / {:.0} / {:.0} мс",
                per(ring.t_probe, ring.probed),
                per(ring.t_build, ring.built),
                per(ring.t_free, ring.freed),
                ring.t_probe,
                ring.t_build,
                ring.t_free
            );
            // Память — то, ради чего кольцо освобождает чанки. Считается
            // здесь, а не в конце: она обязана **не расти**, а конечное
            // число этого не покажет.
            let g = sim.r.world.ground().expect("земля");
            let mib = |b: usize| b as f64 / (1024.0 * 1024.0);
            // Граф каскада — рядом: на километре именно он рос
            // с каждым тиком копки и тянул за собой фазу разрушения
            // (ADR-228).
            println!(
                "  память: занятость мира {:.0} МиБ, правки {:.1} МиБ, \
состояние мешинга {:.1} МиБ на {} чанках; граф каскада {} чанков, {:.1} МиБ",
                mib(g.world.bit_bytes()),
                mib(g.world.diff_bytes()),
                mib(sim.mesh.state_bytes()),
                sim.mesh.live_chunks(),
                g.collapse.known_chunks(),
                mib(g.collapse.graph_bytes())
            );
        }
    }
    // Выход ждёт **и** срок кадров, **и** запись снимка: заказанный
    // снимок сохраняется через несколько кадров, и выйти раньше
    // значит потерять его молча.
    let saved = shot.path.is_none() || (shot.done && shot.seen >= shot.at + SHOT_GRACE);
    if shot.frames != 0 && shot.seen >= shot.frames && saved {
        exit.write(AppExit::Success);
    }
}
