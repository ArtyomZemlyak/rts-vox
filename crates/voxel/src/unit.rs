//! Тело юнита: воксельный том с помеченными подсистемами.
//!
//! **Урон пространственный, броня работает как геометрия** — не число
//! (`docs/05-units.md`). У юнита есть ядро; попадание снимает воксели,
//! и что именно оно задело, решает геометрия, а не таблица.
//!
//! **Подсистем ровно три, и список закрыт** (ADR-095): двигатель,
//! орудие, сенсор. Метка — поле воксела, а не отдельная сущность:
//! никаких списков компонентов на юнита. Подсистема жива, пока жив
//! **хоть один её воксель, связанный с ядром**, — отрезанная от ядра
//! не работает, даже если цела. Для описанного ствола дополнительно
//! требуется целое сечение на всём пути от казённика до дула (ADR-259).
//!
//! **Отказ булев, а не градиентный.** Двигатель либо есть, либо нет.
//! Частичная скорость от доли уцелевших вокселей потребовала бы
//! нормировки, то есть деления в `Fx` на каждом попадании, и это
//! отдельное решение, которого нет.
//!
//! **Обломок — отдельный тип** [`Wreck`] (ADR-204): у него нет ядра
//! и нет подсистем, и отдельный тип делает их отсутствие невозможным
//! по построению. Форма обломка живёт в мировой сетке, содержимое —
//! количество материи; реклейм — та же операция, что копка (ADR-017).

use crate::shape::{
    ball, carve, crate_box, cyl, cyl_f, disc, exhaust, fill, fill_f, hatch, headlight, lid, link,
    link_pivot, mark, mark_ball, mark_cyl, mount, pack, plated_turret, ring, rod, running_gear,
    seat, skirt, slab,
};
use crate::topo::unit::{core_connectivity, CoreBufs, UnitVol};

/// Метка воксела. Ровно то, что перечисляет ADR-095, плюс броня и ядро.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Part {
    Armour = 0,
    Core = 1,
    Engine = 2,
    Weapon = 3,
    Sensor = 4,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Armour => "броня",
            Part::Core => "ядро",
            Part::Engine => "двигатель",
            Part::Weapon => "орудие",
            Part::Sensor => "сенсор",
        }
    }
}

/// Три подсистемы, у которых есть отказ. Броня и ядро не подсистемы:
/// броня это геометрия, ядро это жизнь.
pub const SUBSYSTEMS: [Part; 3] = [Part::Engine, Part::Weapon, Part::Sensor];

/// Состояние юнита после попадания.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Status {
    pub alive: bool,
    pub engine: bool,
    pub weapon: bool,
    pub sensor: bool,
    /// Уцелело вокселей, связанных с ядром.
    pub attached: u32,
    /// Отвалилось от ядра — материя для реклейма.
    pub detached: u32,
}

impl Status {
    pub fn get(&self, p: Part) -> bool {
        match p {
            Part::Engine => self.engine,
            Part::Weapon => self.weapon,
            Part::Sensor => self.sensor,
            Part::Core => self.alive,
            Part::Armour => true,
        }
    }
}

/// Почему тело не пересобралось в другую сетку.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResampleFail {
    /// Колонка тома — не больше 255 вокселей (ADR-179). Внутри —
    /// сколько потребовалось.
    TooLong(u32),
    /// Ядро в новой сетке не село: выборка его разорвала или сдвинула,
    /// и куб `core_size` перестал быть сплошным ядром. Тело с таким
    /// ядром неубиваемо, и отдавать его наружу нельзя.
    CoreLost,
}

impl ResampleFail {
    pub fn why(self) -> String {
        match self {
            ResampleFail::TooLong(n) => {
                format!("не влезает в колонку: нужно {n} вокселей при пределе 255")
            }
            ResampleFail::CoreLost => "ядро в новой сетке не село".to_string(),
        }
    }
}

#[derive(Clone)]
pub struct UnitBody {
    /// Immutable material-grade work factors by Part; 1000 is ordinary steel.
    pub ablation_milli: [u32; 5],
    pub fracture_milli: [u32; 5],
    pub vol: UnitVol,
    /// Метка на воксель — **разделяемая**.
    ///
    /// После сборки она не меняется: урон снимает воксели (`remove`),
    /// а роль оставшихся та же. Значит копия при записи (ADR-018) может
    /// её не копировать, и это шестьдесят процентов тома (ADR-120).
    /// `Arc` здесь не про потоки, а про владение: расхождения он внести
    /// не может, потому что содержимое неизменяемо.
    part: std::sync::Arc<Vec<u8>>,
    /// Маски вокселей каждой подсистемы по колонкам, как у `UnitVol`,
    /// **одним массивом** с шагом `ny * nz`.
    ///
    /// Три отдельных вектора — это три выделения памяти на каждую копию
    /// при записи, а копия делается по первому попаданию, залпом
    /// в тысячи штук. Выделение здесь дороже самих данных (ADR-120).
    /// Считаются один раз при сборке: попадание их только гасит.
    masks: Vec<u64>,
    /// Шаг между масками подсистем.
    mstride: usize,
    pub core: (u8, u8, u8),
    pub core_size: u8,
    /// Ребро воксела тела в **миллиметрах**.
    ///
    /// ADR-019 разрешает миру и телам разные масштабы, и тот же довод
    /// разрешает их между телами (ADR-178). Ребро задаётся **классом**
    /// машины (ADR-219): обычная боевая машина — [`BODY_MM`],
    /// экспериментальный класс и постройка — 250 мм, потому что
    /// в колонку тела влезает не больше 255 вокселей.
    ///
    /// Миллиметры, а не `Fx`: это константа шаблона, она не участвует
    /// в арифметике тика, и целое число исключает вопрос о округлении.
    pub mm: u16,
    /// Отделка воксела кодом (`FINISH_*`). Ноль — по роли.
    ///
    /// **Роль и материал — разные оси.** Трак гусеницы по роли броня,
    /// а на вид резина; каток — броня, а на вид сталь. Пока отделка
    /// выводилась из роли, весь корпус красился одной пластмассой
    /// (ADR-231). Хранится как роль: копия на запись, у неповреждённых
    /// тел одного типа — общая.
    finish: std::sync::Arc<Vec<u8>>,
    /// Повёрнутые части тела: наклонный лист, звено ноги, ствол
    /// под углом. У каждой свой осевой том в своей системе координат
    /// и крепление к родителю — поэтому наклонная плита это плита,
    /// а не лесенка из осевых кубов (ADR-231).
    ///
    /// Связность, ядро и подсистемы живут в корневом томе. Полигон
    /// бьёт сегменты в их кадре покоя и роняет их целиком, когда корень
    /// под креплением снят (ADR-278); общая партия их пока не видит.
    pub segments: Vec<Segment>,
    /// Ноги: какие сегменты — звенья и где их шарниры (ADR-234).
    /// Описание кинематики, а не формы: по нему полигон ставит ногу
    /// на пол (ADR-279) и роняет её целиком, когда уходит её бедро
    /// (ADR-278); рендер общей партии ставит ногу сам.
    pub legs: Vec<Leg>,
    /// Слой, с которого начинается **вращающаяся часть** тела: всё
    /// от него и выше смотрит в цель, а не по движению.
    ///
    /// Ноль значит «вращающейся части нет» — тело поворачивается
    /// целиком. Это описание формы, а не число баланса: в хеш оно не
    /// входит и ни на один исход не влияет. Живёт оно здесь, а не
    /// в рендере, по той же причине, по которой здесь живёт ядро
    /// (D-8): величина, заданная дважды, расходится молча.
    pub turret_y: u8,
    /// Alternative functional barrels, shared by damaged copies. A barrel
    /// needs a core-connected solid cell in every transverse slice. Armour
    /// may bridge the mantlet; unrelated surviving weapon cells cannot bridge
    /// a missing breech or muzzle. Empty means legacy subsystem semantics.
    weapon_paths: std::sync::Arc<Vec<WeaponPath>>,
    /// Independent load-bearing tracks or leg roots, authored in body space.
    load_paths: std::sync::Arc<Vec<WeaponPath>>,
}

/// Half-open local box along a barrel axis. This is authored geometry, not HP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeaponPath {
    pub min: [u8; 3],
    pub max: [u8; 3],
    pub axis: u8,
}

pub const MAX_WEAPON_PATHS: usize = 8;

/// Отделка воксела: ноль — по роли, остальные — явно. Коды идут
/// в том же порядке, что отделки рендера (`render::look::Finish`),
/// со сдвигом на единицу, и это сторожится тестом на стороне рендера:
/// сдвинься порядок — трак покрасился бы стеклом.
pub const FINISH_BY_PART: u8 = 0;
/// Крашеная сталь брони.
pub const FINISH_PLATE: u8 = 1;
/// Воронёная сталь: стволы и маски орудий.
pub const FINISH_GUN: u8 = 2;
/// Стекло оптики.
pub const FINISH_OPTIC: u8 = 3;
/// Резина: траки, кожухи.
pub const FINISH_RUBBER: u8 = 4;
/// Светящееся ядро.
pub const FINISH_CORE: u8 = 5;
/// Голая сталь: катки, суставы, шарниры.
pub const FINISH_STEEL: u8 = 6;
/// Зелёно-бирюзовый световой элемент энергетического оружия.
pub const FINISH_ENERGY: u8 = 7;
/// Светлая броня цвета слоновой кости из утверждённого набора юнитов.
pub const FINISH_IVORY: u8 = 8;

/// Полный оборот двоичного угла крепления — тот же, что у курса
/// (`fx::angle::FULL`): один оборот, одна единица.
pub const TURN: i32 = 1 << 16;

/// Крепление сегмента к родителю.
///
/// Углы — двоичные, как курс машины (ADR-220): оборот `TURN`,
/// вокруг `X` крен, вокруг `Y` рыскание, вокруг `Z` тангаж. Порядок
/// применения — крен, тангаж, рыскание, — задан рендером и пришпилен
/// его тестом; здесь только числа. Целые, потому что это описание
/// формы: в арифметику тика они не входят.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Attach {
    /// Родитель: ноль — корпус, дальше сегменты по порядку плюс один.
    pub parent: u8,
    /// Точка сегмента, которая садится в `at` родителя, в вокселях
    /// **сегмента**. Вокруг неё же и поворот.
    pub pivot: (u8, u8, u8),
    /// Куда садится `pivot`, в вокселях **родителя**. Может выходить
    /// за его том: сегмент не обязан касаться родителя.
    pub at: (i16, i16, i16),
    pub roll: i32,
    pub yaw: i32,
    pub pitch: i32,
    /// Едет с вращающейся частью тела, а не с корпусом.
    pub turret: bool,
}

impl Attach {
    /// Крепление без поворота к корпусу.
    pub const fn flat(at: (i16, i16, i16), pivot: (u8, u8, u8)) -> Attach {
        Attach {
            parent: 0,
            pivot,
            at,
            roll: 0,
            yaw: 0,
            pitch: 0,
            turret: false,
        }
    }

    /// Крепление, повёрнутое **по направлению**: местная ось `X`
    /// сегмента смотрит из `at` вдоль `dir`, заданного в вокселях
    /// родителя. Так бедро наводится от бедра к колену, а не углом,
    /// подобранным на глаз.
    ///
    /// Углы считаются целочисленным `atan2` из `fx`: это описание
    /// формы, и плавающей точке в томе тела делать нечего. Рыскание
    /// положительное поворачивает нос к `-Z` — тем же знаком, что
    /// курс машины у рендера, и это пришпилено его тестом.
    pub fn aimed(at: (i16, i16, i16), pivot: (u8, u8, u8), dir: (i32, i32, i32)) -> Attach {
        let (dx, dy, dz) = (dir.0 as i64, dir.1 as i64, dir.2 as i64);
        // Кратчайшая форма, `-HALF..HALF`: нога, наведённая вниз,
        // обязана читаться отрицательным тангажом, а не тремя
        // четвертями оборота вверх.
        let yaw = fx::angle::diff(fx::angle::atan2(-dz, dx), 0);
        // Корень здесь законен: он берётся один раз при сборке
        // шаблона, а не в тике (`CLAUDE.md`, ADR-071). Перед ним
        // вектор умножается: на коротких векторах целый корень
        // обрезает так, что нога из `(2, -2, 1)` уходила на три
        // градуса не туда.
        const K: i64 = 1 << 10;
        let flat = ((dx * dx + dz * dz) * K * K).isqrt();
        let pitch = fx::angle::diff(fx::angle::atan2(dy * K, flat), 0);
        Attach {
            parent: 0,
            pivot,
            at,
            roll: 0,
            yaw,
            pitch,
            turret: false,
        }
    }
}

/// Повёрнутая часть тела: свой осевой том и крепление.
#[derive(Clone)]
pub struct Segment {
    pub attach: Attach,
    pub body: UnitBody,
}

/// Нога: двухзвенная цепочка бедро → голень с шарнирами в вокселях
/// корня и сегментами, которые с ней едут (ADR-234).
///
/// Шарниры — положения **покоя**: так нога нарисована. Где она
/// на кадре, решает походка рендера по фазе шага из симуляции;
/// длины звеньев берутся из расстояний между шарнирами. Номера —
/// места в `segments`. Опора — которой из четырёх опор подвески
/// нога отвечает. Собственный сдвиг фазы позволяет нескольким ногам одной
/// опоры переставляться независимо.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Leg {
    pub hip: (i16, i16, i16),
    pub knee: (i16, i16, i16),
    pub ankle: (i16, i16, i16),
    pub thigh: u8,
    pub shin: u8,
    /// Шар колена — едет с концом бедра.
    pub knee_ball: Option<u8>,
    /// Шар лодыжки — едет с концом голени.
    pub ankle_ball: Option<u8>,
    /// Ступня — на конце голени, плоско по миру.
    pub foot: Option<u8>,
    pub support: u8,
    pub phase_shift_pct: u8,
}

impl UnitBody {
    /// Пустой том. Положение ядра здесь **не задаётся**: оно выводится
    /// из разметки вызовом `seat_core` после того, как воксели
    /// расставлены. Причина в D-8: пока положение задавалось отдельным
    /// числом, зашитый в код танк и он же, загруженный из данных,
    /// получали разные ядра — обе величины были верны, но разные, и
    /// расхождение не было видно ниоткуда.
    /// Ребро — [`BODY_MM`]: это ребро обычной боевой машины, а тело,
    /// собранное из `.vox`-модели, ею и является. Крупный класс
    /// называет своё ребро сам, через [`UnitBody::new_scaled`].
    pub fn new(nx: u8, ny: u8, nz: u8, core_size: u8) -> UnitBody {
        UnitBody::new_scaled(nx, ny, nz, core_size, BODY_MM)
    }

    /// То же, но с явным ребром воксела в миллиметрах (ADR-178).
    pub fn new_scaled(nx: u8, ny: u8, nz: u8, core_size: u8, mm: u16) -> UnitBody {
        assert!(mm > 0, "воксель нулевого размера");
        let n = nx as usize * ny as usize * nz as usize;
        // Маски подсистем лежат теми же колонками, что и том, значит
        // и шаг у них тот же: разойдутся — подсистема будет читаться
        // из чужой колонки (ADR-179).
        let words = nx.div_ceil(64) as usize;
        UnitBody {
            ablation_milli: [1000; 5],
            fracture_milli: [1000; 5],
            vol: UnitVol::new(nx, ny, nz),
            part: std::sync::Arc::new(vec![Part::Armour as u8; n]),
            masks: vec![0; 3 * ny as usize * nz as usize * words],
            mstride: ny as usize * nz as usize * words,
            core: (0, 0, 0),
            core_size: core_size.max(1),
            mm,
            finish: std::sync::Arc::new(vec![FINISH_BY_PART; n]),
            segments: Vec::new(),
            legs: Vec::new(),
            turret_y: 0,
            weapon_paths: std::sync::Arc::new(Vec::new()),
            load_paths: std::sync::Arc::new(Vec::new()),
        }
    }

    pub fn weapon_paths(&self) -> &[WeaponPath] {
        &self.weapon_paths
    }

    pub fn load_paths(&self) -> &[WeaponPath] {
        &self.load_paths
    }

    pub fn push_load_path(&mut self, path: WeaponPath) {
        let size = [self.vol.nx, self.vol.ny, self.vol.nz];
        assert!(self.load_paths.len() < MAX_WEAPON_PATHS);
        assert!(path.axis < 3);
        assert!((0..3).all(|a| path.min[a] < path.max[a] && path.max[a] <= size[a]));
        std::sync::Arc::make_mut(&mut self.load_paths).push(path);
    }

    pub fn push_weapon_path(&mut self, path: WeaponPath) {
        let size = [self.vol.nx, self.vol.ny, self.vol.nz];
        assert!(self.weapon_paths.len() < MAX_WEAPON_PATHS);
        assert!(path.axis < 3);
        assert!((0..3).all(|a| path.min[a] < path.max[a] && path.max[a] <= size[a]));
        std::sync::Arc::make_mut(&mut self.weapon_paths).push(path);
    }

    /// Whether this individual barrel still joins its breech to its muzzle.
    pub fn barrel_connected(&self, path: &WeaponPath, reached: &[u64]) -> bool {
        let axis = usize::from(path.axis);
        if (0..3).any(|a| path.min[a] >= path.max[a]) {
            return false;
        }
        let a = (axis + 1) % 3;
        let b = (axis + 2) % 3;
        (path.min[axis]..path.max[axis]).all(|slice| {
            (path.min[b]..path.max[b]).any(|j| {
                (path.min[a]..path.max[a]).any(|i| {
                    let mut p = [0usize; 3];
                    p[axis] = usize::from(slice);
                    p[a] = usize::from(i);
                    p[b] = usize::from(j);
                    reached[self.col(p[1], p[2]) + (p[0] >> 6)] & (1 << (p[0] & 63)) != 0
                })
            })
        })
    }

    /// Объявить ногу. Звенья обязаны существовать: нога, ссылающаяся
    /// на несуществующий сегмент, молча стояла бы столбом.
    pub fn push_leg(&mut self, leg: Leg) {
        let n = self.segments.len();
        for s in [
            Some(leg.thigh),
            Some(leg.shin),
            leg.knee_ball,
            leg.ankle_ball,
            leg.foot,
        ]
        .into_iter()
        .flatten()
        {
            assert!((s as usize) < n, "нога ссылается на сегмент {s}, а их {n}");
        }
        assert!(leg.support < 4, "опора {} — их четыре", leg.support);
        self.legs.push(leg);
    }

    /// Назначить вокселю отделку. Ничего не делает у пустого воксела:
    /// отделка у пустоты не бывает, и молчаливая пометка воздуха
    /// потом всплыла бы как «пятно не того цвета» после ремонта.
    pub fn set_finish(&mut self, x: usize, y: usize, z: usize, code: u8) {
        if !self.vol.get(x, y, z) {
            return;
        }
        let i = self.idx(x, y, z);
        std::sync::Arc::make_mut(&mut self.finish)[i] = code;
    }

    /// Отделка воксела кодом; ноль — по роли.
    pub fn finish_at(&self, x: usize, y: usize, z: usize) -> u8 {
        self.finish[self.idx(x, y, z)]
    }

    /// Прикрепить повёрнутую часть. Родитель обязан существовать:
    /// ноль — корпус, дальше — сегменты по порядку добавления плюс
    /// единица.
    pub fn push_segment(&mut self, attach: Attach, body: UnitBody) {
        assert!(
            (attach.parent as usize) <= self.segments.len(),
            "родитель {} не существует: сегментов {}",
            attach.parent,
            self.segments.len()
        );
        self.segments.push(Segment { attach, body });
    }

    /// Габарит тела в миллиметрах: `(длина, высота, ширина)`.
    pub fn extent_mm(&self) -> (u32, u32, u32) {
        let mm = self.mm as u32;
        (
            self.vol.nx as u32 * mm,
            self.vol.ny as u32 * mm,
            self.vol.nz as u32 * mm,
        )
    }

    /// Объём твёрдого в кубических миллиметрах.
    ///
    /// **Считается в мировых единицах, а не в вокселях** (`CLAUDE.md`):
    /// ресурс есть объём на плотность, и воксель крупной машины стоит
    /// ровно во столько раз больше воксела танка, во сколько он больше
    /// по объёму. Без этого экономика разъезжается между классами
    /// объектов — и разъезжается не сразу.
    pub fn volume_mm3(&self) -> u64 {
        let mm = self.mm as u64;
        self.solid_count() as u64 * mm * mm * mm
    }

    /// Пересобрать тело в **другой воксельной сетке**, сохранив размер.
    ///
    /// Нужно, чтобы ответить на вопрос «а как это будет выглядеть при
    /// вокселе в 10 см» замером, а не рассуждением. Габарит в метрах
    /// сохраняется, меняется только через сколько вокселей он выражен.
    ///
    /// **Деталей это не добавляет и добавить не может.** Источник их
    /// не содержит: выборка идёт ближайшим, и каждый исходный воксель
    /// становится блоком. Меняется зернистость силуэта — а с фаской
    /// в половину воксела ещё и её размер, потому что скругление
    /// привязано к вокселю, а не к метру (ADR-208).
    ///
    /// Отказ **называет причину**: их две, и они разные. Либо тело
    /// не влезает в колонку (не больше 255 вокселей, ADR-179 — при
    /// 100 мм это 25.5 м), либо ядро в новой сетке не село, и тогда
    /// машина вышла бы неубиваемой. Второе важнее первого и на глаз
    /// неотличимо, поэтому валить их в один `None` нельзя.
    /// Выборка формы в новую сетку **без посадки ядра**: сегменты ядра
    /// не имеют, и требовать его от них значило бы отказывать каждому.
    fn resample_shape(&self, mm: u16) -> Result<UnitBody, ResampleFail> {
        assert!(mm > 0, "воксель нулевого размера");
        let (sx, sy, sz) = (self.vol.nx as u32, self.vol.ny as u32, self.vol.nz as u32);
        let src = self.mm as u32;
        // Габарит в миллиметрах делится на новое ребро, с округлением
        // вверх: обрезать машину на воксель хуже, чем дать ей лишний.
        let dim = |n: u32| -> Result<u8, ResampleFail> {
            let v = (n * src).div_ceil(mm as u32);
            if v == 0 || v > 255 {
                Err(ResampleFail::TooLong(v))
            } else {
                Ok(v as u8)
            }
        };
        let (nx, ny, nz) = (dim(sx)?, dim(sy)?, dim(sz)?);
        // Размер ядра ставится **после** выборки, по тому, что от него
        // уцелело. Пропорция здесь врёт: выборка ближайшим даёт то
        // на воксель больше, то на воксель меньше, и куб `core_size`,
        // посчитанный умножением, перестаёт быть сплошным ядром —
        // ровно так не садилось ядро у трёх машин из четырнадцати.
        let mut out = UnitBody::new_scaled(nx, ny, nz, 1, mm);
        out.ablation_milli = self.ablation_milli;
        out.fracture_milli = self.fracture_milli;
        out.turret_y = ((self.turret_y as u32 * src) / mm as u32).min(255) as u8;

        for z in 0..nz as u32 {
            for y in 0..ny as u32 {
                for x in 0..nx as u32 {
                    // Середина целевого воксела в миллиметрах, потом
                    // в исходную сетку. Середина, а не угол: угол
                    // на границе двух вокселей выбирает по знаку
                    // округления, и форма ползёт в одну сторону.
                    let pick = |d: u32, n: u32| -> Option<u32> {
                        let mid = d * mm as u32 + mm as u32 / 2;
                        let i = mid / src;
                        if i < n {
                            Some(i)
                        } else {
                            None
                        }
                    };
                    let (Some(ix), Some(iy), Some(iz)) = (pick(x, sx), pick(y, sy), pick(z, sz))
                    else {
                        continue;
                    };
                    let (ix, iy, iz) = (ix as usize, iy as usize, iz as usize);
                    if !self.vol.get(ix, iy, iz) {
                        continue;
                    }
                    out.set(x as usize, y as usize, z as usize, self.part_at(ix, iy, iz));
                    // Отделка едет вместе с ролью: трак остаётся резиной
                    // при любом ребре.
                    out.set_finish(
                        x as usize,
                        y as usize,
                        z as usize,
                        self.finish_at(ix, iy, iz),
                    );
                }
            }
        }
        // Match the same centre-sample rule used for occupancy. An empty
        // resampled path stays present and disabled, never enables fallback.
        let bound = |v: u8, limit: u8| {
            (u32::from(v) * src)
                .saturating_sub(u32::from(mm) / 2)
                .div_ceil(u32::from(mm))
                .min(u32::from(limit)) as u8
        };
        let size = [nx, ny, nz];
        let resample_paths = |paths: &[WeaponPath]| {
            std::sync::Arc::new(
                paths
                    .iter()
                    .map(|p| WeaponPath {
                        min: std::array::from_fn(|a| bound(p.min[a], size[a])),
                        max: std::array::from_fn(|a| bound(p.max[a], size[a])),
                        axis: p.axis,
                    })
                    .collect(),
            )
        };
        out.weapon_paths = resample_paths(&self.weapon_paths);
        out.load_paths = resample_paths(&self.load_paths);
        Ok(out)
    }

    /// Пересэмплировать тело в другое ребро, миллиметры (ADR-208).
    ///
    /// Форма выбирается ближайшим вокселем, сегменты — каждый в своей
    /// сетке с пересчётом крепления, ядро сажается заново. Отказ
    /// называет причину: не влезло в колонку или ядро разорвано.
    pub fn resampled(&self, mm: u16) -> Result<UnitBody, ResampleFail> {
        let mut out = self.resample_shape(mm)?;
        let src = self.mm as u32;
        // Сегменты пересэмплируются каждый в своей сетке, а крепление
        // пересчитывается тем же отношением рёбер: точка посадки и ось
        // поворота — размеры, и меряются они в вокселях.
        for s in &self.segments {
            let body = s.body.resample_shape(mm)?;
            let k = |v: i32| -> i32 { (v * src as i32) / mm as i32 };
            let attach = Attach {
                pivot: (
                    k(s.attach.pivot.0 as i32).clamp(0, 255) as u8,
                    k(s.attach.pivot.1 as i32).clamp(0, 255) as u8,
                    k(s.attach.pivot.2 as i32).clamp(0, 255) as u8,
                ),
                at: (
                    k(s.attach.at.0 as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    k(s.attach.at.1 as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    k(s.attach.at.2 as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                ),
                ..s.attach
            };
            out.segments.push(Segment { attach, body });
        }
        // Ноги: шарниры — размеры в вокселях, пересчитываются тем же
        // отношением; номера звеньев те же, сегменты идут в том же
        // порядке.
        for l in &self.legs {
            let k = |v: i16| -> i16 {
                ((v as i32 * src as i32) / mm as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16
            };
            let p = |v: (i16, i16, i16)| (k(v.0), k(v.1), k(v.2));
            out.legs.push(Leg {
                hip: p(l.hip),
                knee: p(l.knee),
                ankle: p(l.ankle),
                ..*l
            });
        }
        // Наибольший куб ядра, который сядет. Убывающим перебором,
        // а не формулой: `seat_core` требует, чтобы куб был сплошным
        // ядром, а форма ядра после выборки известна только ей.
        let want = ((self.core_size as u32 * src).div_ceil(mm as u32)).clamp(1, 255) as u8;
        for cs in (1..=want).rev() {
            out.core_size = cs;
            if out.seat_core() {
                return Ok(out);
            }
        }
        // Ядро обязано сесть и в новой сетке: если выборка его
        // разорвала, тело неубиваемо, и молча отдавать такое нельзя.
        Err(ResampleFail::CoreLost)
    }

    /// Посадить затравку связности на разметку.
    ///
    /// Правило одно и живёт здесь: затравка — куб `core_size` в
    /// **минимальном углу габарита вокселей, помеченных `Core`**, и
    /// весь этот куб обязан быть ядром. Возвращает `false`, если ядра
    /// нет вовсе или куб в него не влезает: юнит без ядра неубиваем,
    /// и молча пропустить это нельзя.
    pub fn seat_core(&mut self) -> bool {
        let (nx, ny, nz) = (
            self.vol.nx as usize,
            self.vol.ny as usize,
            self.vol.nz as usize,
        );
        let mut lo = (usize::MAX, usize::MAX, usize::MAX);
        let mut any = false;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    if self.vol.get(x, y, z) && self.part_at(x, y, z) == Part::Core {
                        lo = (lo.0.min(x), lo.1.min(y), lo.2.min(z));
                        any = true;
                    }
                }
            }
        }
        if !any {
            return false;
        }
        let cs = self.core_size as usize;
        if lo.0 + cs > nx || lo.1 + cs > ny || lo.2 + cs > nz {
            return false;
        }
        for dz in 0..cs {
            for dy in 0..cs {
                for dx in 0..cs {
                    let (x, y, z) = (lo.0 + dx, lo.1 + dy, lo.2 + dz);
                    if !self.vol.get(x, y, z) || self.part_at(x, y, z) != Part::Core {
                        return false;
                    }
                }
            }
        }
        self.core = (lo.0 as u8, lo.1 as u8, lo.2 as u8);
        true
    }

    #[inline]
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.vol.ny as usize + y) * self.vol.nx as usize + x
    }

    /// Начало колонки в массиве масок.
    #[inline]
    fn col(&self, y: usize, z: usize) -> usize {
        (z * self.vol.ny as usize + y) * self.vol.words as usize
    }

    /// Расставить воксель. Единственное место, где меняются метки,
    /// и потому единственное, где разделяемый массив расщепляется.
    ///
    /// **Прежняя роль снимается с маски.** Броня, положенная поверх
    /// выхлопа, оставляла воксел в маске двигателя навсегда: `status`
    /// докладывал живой двигатель у тела, где ни одного вокселя
    /// двигателя нет, и подсистема была неубиваемой (найдено при
    /// детализации тел, ADR-224).
    pub fn set(&mut self, x: usize, y: usize, z: usize, p: Part) {
        let w = self.col(y, z) + (x >> 6);
        let bit = 1u64 << (x & 63);
        if self.vol.get(x, y, z) {
            if let Some(k) = subsystem_slot(self.part_at(x, y, z)) {
                self.masks[k * self.mstride + w] &= !bit;
            }
        }
        self.vol.set(x, y, z, true);
        let i = self.idx(x, y, z);
        std::sync::Arc::make_mut(&mut self.part)[i] = p as u8;
        if let Some(k) = subsystem_slot(p) {
            self.masks[k * self.mstride + w] |= bit;
        }
    }

    pub fn part_at(&self, x: usize, y: usize, z: usize) -> Part {
        match self.part[self.idx(x, y, z)] {
            1 => Part::Core,
            2 => Part::Engine,
            3 => Part::Weapon,
            4 => Part::Sensor,
            _ => Part::Armour,
        }
    }

    /// Снять воксель. Возвращает `true`, если он там был.
    pub fn remove(&mut self, x: usize, y: usize, z: usize) -> bool {
        if !self.vol.get(x, y, z) {
            return false;
        }
        self.vol.set(x, y, z, false);
        if let Some(k) = subsystem_slot(self.part_at(x, y, z)) {
            let c = self.col(y, z);
            self.masks[k * self.mstride + c + (x >> 6)] &= !(1u64 << (x & 63));
        }
        true
    }

    /// Геометрическое состояние после урона: что связано с затравкой ядра.
    /// Игровую жизнеспособность ядра, мотора и ходовой проверяет общий
    /// sim-core::integrity; эта низкоуровневая операция не знает баланс.
    ///
    /// Один флуд от ядра на весь запрос, а не по флуду на подсистему:
    /// маска прикреплённого считается один раз, а подсистемы читаются
    /// её пересечением со своими масками.
    pub fn status(&self, b: &mut CoreBufs) -> Status {
        let rep = core_connectivity(&self.vol, b, self.core, self.core_size);
        let reached = b.reached_cols();
        let live = |k: usize| -> bool {
            self.masks[k * self.mstride..(k + 1) * self.mstride]
                .iter()
                .zip(reached)
                .any(|(m, r)| m & r != 0)
        };
        Status {
            alive: rep.core_alive,
            engine: rep.core_alive && live(0),
            weapon: rep.core_alive
                && live(1)
                && (self.weapon_paths.is_empty()
                    || self
                        .weapon_paths
                        .iter()
                        .any(|p| self.barrel_connected(p, reached))),
            sensor: rep.core_alive && live(2),
            attached: rep.attached,
            detached: rep.detached,
        }
    }

    /// Remaining cells of a subsystem; masks are cleared with occupancy.
    pub fn subsystem_count(&self, part: Part) -> u32 {
        let Some(k) = SUBSYSTEMS.iter().position(|&p| p == part) else {
            return 0;
        };
        self.masks[k * self.mstride..(k + 1) * self.mstride]
            .iter()
            .map(|w| w.count_ones())
            .sum()
    }

    /// Call after `status` for this body with the same scratch buffer.
    pub fn connected_subsystem_count(&self, part: Part, core: &CoreBufs) -> u32 {
        let Some(k) = SUBSYSTEMS.iter().position(|&p| p == part) else {
            return 0;
        };
        self.masks[k * self.mstride..(k + 1) * self.mstride]
            .iter()
            .zip(core.reached_cols())
            .map(|(a, b)| (a & b).count_ones())
            .sum()
    }

    pub fn solid_count(&self) -> u32 {
        self.vol.solid_count()
    }

    /// Байты тома. Метка воксела — байт, и это самая дорогая часть:
    /// занятость стоит бит.
    /// Весь том, включая разделяемые метки. Для шаблона это его
    /// полный размер.
    pub fn bytes(&self) -> usize {
        self.bytes_private()
            + self.part.len()
            + self.finish.len()
            + self.weapon_paths.len() * std::mem::size_of::<WeaponPath>()
            + self.load_paths.len() * std::mem::size_of::<WeaponPath>()
    }

    /// Только собственная память тома. У копии метки общие с шаблоном,
    /// и считать их дважды — завысить расход флота вдвое с половиной.
    pub fn bytes_private(&self) -> usize {
        self.vol.cols.len() * 8 + self.masks.len() * 8
    }

    /// **Обломок этого тела.** Машина оседает на месте, вся её материя
    /// переливается в обломок.
    ///
    /// `damage_mm3` — материя, снятая попаданиями при жизни. Канон
    /// требует перелить её сюда целиком: «снятые попаданиями воксели
    /// лежат в контейнере `damage` и при смерти переливаются в обломок
    /// — материя вся там, промежуточного „нигде“ не существует»
    /// (`docs/05-units.md`). Ноль — если машина цела.
    ///
    /// **След машины, ровный слой.** Куча лежит там, где стояла
    /// машина: занята та же мировая клетка, что была занята телом
    /// хоть на какой высоте. По этому следу материя раскладывается
    /// **ровно** — высота одна на все столбы.
    ///
    /// Оседание «каждый столб отдельно» пробовалось первым и куч
    /// не давало: у машины масса стоит вертикально, и столб, полный
    /// сверху донизу, сохранял свою высоту. Шагающий разведчик так
    /// оседал с 6.0 м до 6.0 м. Ровный слой даёт то, чем обломок
    /// и является: низкое поле, в котором по очертанию ещё видно,
    /// что тут погибло.
    ///
    /// Остаток раскладывается по **первым столбам линейного
    /// индекса** — ничья разрешается индексом, а не порядком обхода
    /// (`CLAUDE.md`).
    pub fn wreck(&self, damage_mm3: u64) -> Wreck {
        // Отношение рёбер: сколько вокселей тела ложится на воксель
        // обломка по каждой оси (`07-economy.md`, «конверсия
        // масштабов»). При мировом вокселе 0.1 м это единица — сетка
        // мира не грубее сетки тела ни у одного класса (ADR-227).
        let k = (WORLD_MM / self.mm).max(1) as usize;
        let per = (k * k * k) as u64;
        // Ребро воксела обломка — **грубейшее из двух**, мирового
        // и того, которым нарисовано тело. Обломок живёт в мировой
        // сетке, пока она грубее; мельче тела он быть не может,
        // потому что колонка тома держит 255 вокселей (ADR-179),
        // а крупный класс потому и рисуется вокселем 0.25 м, что
        // он длиннее 25.5 м.
        let cell = self.mm * k as u16;
        let (nx, ny, nz) = (
            self.vol.nx as usize,
            self.vol.ny as usize,
            self.vol.nz as usize,
        );
        let (wx, wz) = (nx.div_ceil(k), nz.div_ceil(k));

        // След строится **по словам колонок**, а не обходом вокселей:
        // столб тела — это `ИЛИ` всех его высот, и биты результата
        // сразу дают занятые `x`. Поэлементный обход стоил крепости
        // 1857 мкс на смерть, словесный — 155. Приём тот же, что
        // у мешера, читающего маски вместо массива материалов
        // (ADR-068).
        let mut foot = vec![false; wx * wz];
        let words = self.vol.words as usize;
        let mut acc = vec![0u64; words];
        for z in 0..nz {
            acc.iter_mut().for_each(|w| *w = 0);
            for y in 0..ny {
                for (a, v) in acc.iter_mut().zip(self.vol.col(y, z)) {
                    *a |= *v;
                }
            }
            for (wi, mut m) in acc.iter().copied().enumerate() {
                while m != 0 {
                    let x = wi * 64 + m.trailing_zeros() as usize;
                    m &= m - 1;
                    foot[(z / k) * wx + x / k] = true;
                }
            }
        }
        let n = self.solid_count() as u64;
        let a = foot.iter().filter(|f| **f).count() as u64;
        // Высота слоя и сколько столбов получают лишний воксель.
        // Остаток округляется **вверх**: форма обязана вмещать хотя бы
        // собственную материю тела, иначе контейнеру нечем её держать.
        let (h, extra) = if a == 0 {
            (0, 0)
        } else {
            (n / (per * a), (n % (per * a)).div_ceil(per) as usize)
        };
        let hi = (h as usize + usize::from(extra > 0)).max(1);
        let mut vol = UnitVol::new(wx as u8, hi as u8, wz as u8);
        let mut left = extra;
        for (i, on) in foot.iter().enumerate() {
            if !on {
                continue;
            }
            let mut col = h as usize;
            if left > 0 {
                col += 1;
                left -= 1;
            }
            for y in 0..col {
                vol.set(i % wx, y, i / wx, true);
            }
        }
        Wreck {
            vol,
            mm: cell,
            matter: self.volume_mm3() + damage_mm3,
        }
    }
}

/// Ребро воксела **мира** в миллиметрах — десять сантиметров (ADR-227).
///
/// Обломок живёт в мировой сетке, а не в сетке тела: реклеймит его тот
/// же код, что копает землю (ADR-017), а копает он мировые воксели.
/// Исключение одно и названо в [`UnitBody::wreck`]: тело, нарисованное
/// вокселем грубее мирового, оставляет обломок в своей сетке.
pub const WORLD_MM: u16 = 100;

/// **Обломок** — то, что остаётся от тела, когда его разобрали.
///
/// **Это не `UnitBody`.** У обломка нет ядра и нет подсистем, и
/// отдельный тип делает их отсутствие невозможным по построению.
/// Канон требует того же явно: «реклейм допустим только по телам
/// класса „обломок“ и по нейтральной породе… тот же код, применённый
/// к тому живого юнита, — механика мгновенной победы»
/// (`docs/05-units.md`). Разница классов, выраженная типом, не может
/// быть забыта на месте вызова.
///
/// **Форма в чужой сетке, содержимое точное.** Форма живёт в мировой
/// сетке 0.1 м и перекладывается в неё из сетки тела; содержимое —
/// количество материи в кубических миллиметрах, и смена сетки его
/// не трогает (ADR-021, ADR-022). Две величины разные нарочно:
/// вместимость формы и количество в контейнере совпадать не обязаны,
/// как не обязаны совпадать объём склада и то, сколько в нём лежит.
#[derive(Clone)]
pub struct Wreck {
    pub vol: UnitVol,
    /// Ребро воксела обломка в миллиметрах: [`WORLD_MM`] либо ребро
    /// тела, если оно грубее мирового (ADR-227).
    pub mm: u16,
    matter: u64,
}

impl Wreck {
    /// Сколько материи в обломке, в кубических миллиметрах.
    pub fn matter_mm3(&self) -> u64 {
        self.matter
    }

    pub fn solid_count(&self) -> u32 {
        self.vol.solid_count()
    }

    /// Вместимость формы: объём её вокселей. С содержимым совпадать
    /// не обязана — см. описание типа.
    pub fn capacity_mm3(&self) -> u64 {
        let mm = self.mm as u64;
        self.solid_count() as u64 * mm * mm * mm
    }

    /// Габарит обломка в миллиметрах: `(длина, высота, ширина)`.
    pub fn extent_mm(&self) -> (u32, u32, u32) {
        let mm = self.mm as u32;
        (
            self.vol.nx as u32 * mm,
            self.vol.ny as u32 * mm,
            self.vol.nz as u32 * mm,
        )
    }

    pub fn bytes(&self) -> usize {
        self.vol.cols.len() * 8
    }

    /// **Снять воксель обломка и получить материю.** Это и есть
    /// реклейм — та же операция, что копка (ADR-017).
    ///
    /// Выдаётся **доля содержимого, а не объём воксела**: обломок
    /// контейнер, и содержимое в нём количество (ADR-022). Делится
    /// оставшееся на число оставшихся вокселей, поэтому последний
    /// воксель выносит весь остаток и деление ничего не теряет.
    /// Отдельного разрежённого слоя дробных остатков
    /// (`07-economy.md`) обломку для этого не нужно, а скрытого стока
    /// на округлении не возникает: сумма выданного равна содержимому
    /// при любом порядке снятия.
    ///
    /// Выдача на воксель к концу разбора слегка растёт — это цена
    /// того, что стока нет.
    pub fn reclaim(&mut self, x: usize, y: usize, z: usize) -> u64 {
        if !self.vol.get(x, y, z) {
            return 0;
        }
        let n = self.solid_count() as u64;
        self.vol.set(x, y, z, false);
        let take = self.matter / n;
        self.matter -= take;
        take
    }
}

#[inline]
fn subsystem_slot(p: Part) -> Option<usize> {
    match p {
        Part::Engine => Some(0),
        Part::Weapon => Some(1),
        Part::Sensor => Some(2),
        _ => None,
    }
}

/// Ребро воксела обычной боевой машины в миллиметрах (ADR-219).
///
/// Вдесятеро мельче метра и впятеро мельче мирового воксела. Столько
/// нужно, чтобы в ленту помещался каток, а в ногу — сустав: при 250 мм
/// на всю ленту приходилось четыре воксела по высоте, и ничего, кроме
/// бруска, туда не ложилось.
pub const BODY_MM: u16 = 100;

/// Local voxel at the front of the crawler's shield cutting head.
pub const CRAWLER_DRILL_TIP: [usize; 3] = [173, 30, 40];
/// Heavy tracked recycler with a closed hold, intake jaws and a separate drill.
#[allow(dead_code)]
fn legacy_crawler() -> UnitBody {
    let mut u = UnitBody::new_scaled(48, 25, 28, 4, BODY_MM);
    let a = Part::Armour;
    running_gear(&mut u, (2, 46), 8, (1, 6), a);
    running_gear(&mut u, (2, 46), 8, (22, 27), a);
    fill(&mut u, (4, 44), (2, 10), (6, 22), a);
    fill(&mut u, (5, 19), (10, 18), (6, 22), a);
    fill(&mut u, (7, 17), (4, 12), (9, 19), Part::Engine);
    fill(&mut u, (19, 25), (4, 10), (11, 17), Part::Core);
    fill_f(&mut u, (7, 16), (17, 21), (8, 20), a, FINISH_PLATE);
    fill_f(&mut u, (9, 14), (19, 21), (7, 8), Part::Sensor, FINISH_OPTIC);
    fill_f(&mut u, (20, 39), (10, 12), (7, 21), a, FINISH_STEEL);
    fill_f(&mut u, (20, 39), (12, 19), (7, 9), a, FINISH_PLATE);
    fill_f(&mut u, (20, 39), (12, 19), (19, 21), a, FINISH_PLATE);
    fill_f(&mut u, (20, 22), (12, 19), (9, 19), a, FINISH_STEEL);
    fill_f(&mut u, (38, 40), (12, 17), (9, 19), a, FINISH_STEEL);
    fill_f(&mut u, (40, 47), (3, 7), (7, 21), a, FINISH_STEEL);
    fill_f(&mut u, (46, 48), (1, 5), (4, 24), a, FINISH_GUN);
    fill_f(&mut u, (41, 47), (7, 9), (8, 20), a, FINISH_PLATE);
    for z in [5, 9, 13, 17, 21] {
        fill_f(&mut u, (46, 48), (0, 1), (z, z + 2), a, FINISH_STEEL);
    }
    for x in [23, 28, 33, 37] {
        mark(&mut u, (x, x + 1), (12, 19), (7, 9), FINISH_STEEL);
        mark(&mut u, (x, x + 1), (12, 19), (19, 21), FINISH_STEEL);
    }
    fill_f(&mut u, (9, 14), (19, 21), (20, 21), Part::Sensor, FINISH_OPTIC);
    for x in [8, 10, 12, 14] {
        mark(&mut u, (x, x + 1), (20, 21), (10, 18), FINISH_GUN);
    }
    for z in [7, 18] {
        fill_f(&mut u, (43, 45), (8, 9), (z, z + 2), Part::Sensor, FINISH_OPTIC);
    }
    cyl_f(&mut u, 0, (36, 48), (12, 14), 3, 0, Part::Weapon, FINISH_GUN);
    for x in [38, 42, 46] {
        mark_cyl(&mut u, 0, (x, x + 1), (12, 14), 3, FINISH_STEEL);
    }
    mark_cyl(&mut u, 0, (47, 48), (12, 14), 2, FINISH_OPTIC);
    fill_f(&mut u, (20, 40), (19, 22), (7, 21), a, FINISH_PLATE);
    fill_f(&mut u, (20, 22), (12, 19), (9, 19), a, FINISH_PLATE);
    for x in [23, 28, 33, 38] {
        fill_f(&mut u, (x, x + 1), (19, 23), (7, 21), a, FINISH_STEEL);
    }
    for x in 40..48 {
        let inset = (x - 40) / 3;
        fill_f(&mut u, (x, x + 1), (2, 5), (4 + inset, 7 + inset), a, FINISH_GUN);
        fill_f(&mut u, (x, x + 1), (2, 5), (21 - inset, 24 - inset), a, FINISH_GUN);
    }
    let u = seat(u, "краулер");
    let mut large = UnitBody::new_scaled(144, 75, 84, u.core_size * 3, BODY_MM);
    for z in 0..usize::from(u.vol.nz) {
        for y in 0..usize::from(u.vol.ny) {
            for x in 0..usize::from(u.vol.nx) {
                if !u.vol.get(x, y, z) { continue; }
                let part = u.part_at(x, y, z);
                let finish = u.finish_at(x, y, z);
                for dz in 0..3 { for dy in 0..3 { for dx in 0..3 {
                    let (x, y, z) = (x * 3 + dx, y * 3 + dy, z * 3 + dz);
                    large.set(x, y, z, part);
                    large.set_finish(x, y, z, finish);
                }}}
            }
        }
    }
    large.core = (u.core.0 * 3, u.core.1 * 3, u.core.2 * 3);
    large.turret_y = large.vol.ny;
    large
}

/// «Старатель»: closed bunker, four track pods and one shield cutting head.
pub fn crawler() -> UnitBody {
    let mut u = UnitBody::new_scaled(184, 52, 80, 8, BODY_MM);
    let a = Part::Armour;

    for xs in [(41, 79), (101, 139)] {
        for zs in [(2, 20), (60, 78)] {
            running_gear(&mut u, xs, 14, zs, a);
        }
    }
    fill_f(&mut u, (25, 131), (14, 22), (7, 73), a, FINISH_GUN);
    fill_f(&mut u, (25, 126), (22, 50), (10, 70), a, FINISH_IVORY);
    fill(&mut u, (30, 48), (22, 34), (28, 52), Part::Engine);
    fill(&mut u, (60, 76), (24, 36), (32, 48), Part::Core);
    for z in [(10, 13), (67, 70)] {
        mark(&mut u, (32, 118), (42, 46), z, FINISH_PLATE);
        for x in [44usize, 72, 100] {
            mark(&mut u, (x, x + 2), (22, 48), z, FINISH_STEEL);
        }
    }
    for x in [44usize, 72, 100] {
        mark(&mut u, (x, x + 2), (49, 50), (16, 64), FINISH_STEEL);
    }
    for x in [38usize, 88] {
        fill_f(&mut u, (x, x + 12), (50, 51), (26, 54), a, FINISH_IVORY);
        mark(&mut u, (x + 3, x + 9), (50, 51), (31, 49), FINISH_GUN);
    }
    fill_f(&mut u, (24, 27), (28, 44), (18, 62), a, FINISH_GUN);
    fill_f(&mut u, (24, 26), (42, 44), (18, 62), a, FINISH_STEEL);
    for z in [22usize, 54] {
        fill_f(&mut u, (24, 26), (29, 32), (z, z + 4), a, FINISH_CORE);
    }
    fill_f(&mut u, (100, 104), (32, 44), (68, 71), a, FINISH_GUN);
    for y in [33usize, 36, 39, 42] {
        fill_f(&mut u, (100, 104), (y, y + 2), (69, 71), a, FINISH_CORE);
    }
    fill_f(&mut u, (122, 127), (40, 44), (34, 40), Part::Sensor, FINISH_OPTIC);

    for z in [3usize, 71] {
        fill_f(&mut u, (126, 134), (31, 39), (z, z + 9), a, FINISH_GUN);
        fill_f(&mut u, (128, 150), (32, 38), (z + 1, z + 7), a, FINISH_IVORY);
        fill_f(&mut u, (148, 164), (28, 35), (z + 1, z + 7), a, FINISH_STEEL);
        for x in [136usize, 144, 154] {
            mark(&mut u, (x, x + 2), (31, 39), (z + 1, z + 7), FINISH_GUN);
        }
        fill_f(&mut u, (158, 167), (27, 33), (z, z + 9), a, FINISH_GUN);
    }
    for z in [(8, 16), (64, 72)] {
        fill_f(&mut u, (156, 168), (28, 34), z, a, FINISH_GUN);
    }
    fill_f(&mut u, (150, 158), (29, 35), (36, 44), a, FINISH_GUN);
    cyl_f(&mut u, 0, (156, 170), (30, 40), 28, 0, Part::Weapon, FINISH_GUN);
    cyl_f(&mut u, 0, (150, 156), (30, 40), 7, 0, Part::Weapon, FINISH_STEEL);
    for x in [150usize, 160, 166] {
        mark_cyl(&mut u, 0, (x, x + 2), (30, 40), 28, FINISH_STEEL);
    }
    for dy in -30i32..30 {
        for dz in -30i32..30 {
            let r2 = dy * dy + dz * dz;
            if r2 > 30 * 30 { continue; }
            let angle = fx::angle::norm(fx::angle::atan2(i64::from(dz), i64::from(dy)));
            let in_sector = angle * 12 % fx::angle::FULL;
            if r2 > 100 && in_sector < fx::angle::FULL / 4 { continue; }
            let finish = if r2 > 27 * 27 { FINISH_IVORY } else { FINISH_STEEL };
            let (y, z) = ((30 + dy) as usize, (40 + dz) as usize);
            fill_f(&mut u, (170, 174), (y, y + 1), (z, z + 1), Part::Weapon, finish);
        }
    }
    cyl_f(&mut u, 0, (168, 174), (30, 40), 7, 0, Part::Weapon, FINISH_GUN);
    fill_f(&mut u, (146, 154), (27, 37), (36, 44), a, FINISH_GUN);
    fill_f(&mut u, (140, 150), (34, 50), (36, 44), a, FINISH_GUN);
    fill_f(&mut u, (124, 144), (44, 52), (36, 44), a, FINISH_GUN);
    for x in [128usize, 136, 144, 152] {
        mark(&mut u, (x, x + 2), (43, 51), (36, 44), FINISH_STEEL);
    }

    let mut u = seat(u, "краулер Старатель");
    u.turret_y = u.vol.ny;
    u
}

/// Танк: 6.0 × 3.0 × 3.0 м при вокселе 100 мм — том 60×30×30.
///
/// Габарит канонический (ADR-094) и от смены сетки не меняется:
/// мельче стало зерно, а не машина.
///
/// **Всё наклонное — сегментами** (ADR-231, ADR-224): верхний
/// и нижний лобовые листы, четыре листа башни — борта и скулы, —
/// дымовые гранатомёты на скулах и антенна. Под каждым листом
/// корневой том держит ступени ([`crate::shape::mount`]): урон,
/// связность и ядро живут в нём. Башня — многоугольник из листов
/// вокруг скрытого короба: уступа, который читался свадебным тортом,
/// больше нет.
///
/// **Материал — по детали.** Корпус и башня — краска стороны; лента
/// резина, катки, погон, ступицы и поручни — голая сталь; ствол,
/// маска и выхлоп — воронёная сталь; фары и прицелы — стекло.
///
/// Что читается с игровой камеры: люки механика и заряжающего,
/// командирская башенка с приборами по кругу, прицел наводчика,
/// жалюзи моторного отсека, ящики ЗИП на полках, корзина на корме
/// башни, запасные траки на корме, фары с козырьками на носах полок,
/// буксирные крюки, бортовые экраны над носовой половиной ходовой.
///
/// Форма условная: настоящие тома приходят `.vox`-моделями из данных
/// (ADR-008), а эта нужна, чтобы сцена имела что разбирать.
pub fn tank() -> UnitBody {
    let mut u = UnitBody::new_scaled(60, 30, 30, 4, BODY_MM);
    let a = Part::Armour;

    // ---------------- Ходовая ----------------
    // Две ленты по бортам, метр высотой: замкнутая петля с траками,
    // ведущее колесо с венцом в корме, ленивец в носу, шесть катков.
    running_gear(&mut u, (2, 58), 10, (1, 7), a);
    running_gear(&mut u, (2, 58), 10, (23, 29), a);

    // ---------------- Корпус ----------------
    // Днище и нижний корпус между лентами, кормовой лист, полки
    // над лентами во всю ширину — на них стоит верхний корпус,
    // уже полок на три воксела с борта: полка остаётся полкой.
    fill(&mut u, (4, 49), (2, 11), (7, 23), a);
    fill(&mut u, (2, 4), (3, 17), (7, 23), a);
    fill(&mut u, (2, 58), (11, 13), (0, 30), a);
    fill(&mut u, (4, 48), (13, 17), (4, 26), a);
    // Верхний лобовой лист: от кромки крыши вниз-вперёд на тридцать
    // градусов до полки, три воксела толщиной, во всю ширину верхнего
    // корпуса. Шарнир — верхнее наружное ребро листа, на кромке.
    // Ступени под ним кладёт `mount` — двенадцать вокселей вглубь.
    let glacis = slab(8, 3, 22, BODY_MM, a, FINISH_PLATE);
    mount(
        &mut u,
        Attach {
            pitch: -TURN / 12,
            ..Attach::flat((48, 17, 15), (0, 3, 11))
        },
        glacis,
        12,
        0,
        a,
    );
    // Нижний лобовой лист между лентами: от передней кромки днища
    // вверх-вперёд на сорок пять градусов к носу полки. Наружная
    // грань нижняя, ступени — за ним, внутрь корпуса.
    let lower = slab(13, 2, 16, BODY_MM, a, FINISH_PLATE);
    mount(
        &mut u,
        Attach {
            pitch: TURN / 8,
            ..Attach::flat((49, 2, 15), (0, 0, 8))
        },
        lower,
        0,
        10,
        a,
    );
    // Бортовые экраны над носовой половиной ходовой: три плиты
    // с просветами, подрезанные снизу спереди. Корма открыта нарочно —
    // по ней видно, что лента огибает колёса.
    for &z in &[0usize, 29] {
        for k in 0..3usize {
            let x = 32 + k * 8;
            fill(&mut u, (x, x + 7), (6, 11), (z, z + 1), a);
            carve(&mut u, (x + 5, x + 7), (6, 7), (z, z + 1));
            // One hinge and two small bolt heads per replaceable skirt plate.
            fill_f(&mut u, (x, x + 1), (10, 12), (z, z + 1), a, FINISH_STEEL);
            for pin in [x + 2, x + 5] {
                fill_f(&mut u, (pin, pin + 1), (8, 9), (z, z + 1), a, FINISH_GUN);
            }
        }
    }
    // Полки: ящики ЗИП с крышками слева, бочка и канистры справа,
    // фары с козырьками на носах, буксирные крюки на корме.
    crate_box(&mut u, (6, 16), (13, 17), (0, 4), a);
    crate_box(&mut u, (18, 28), (13, 16), (0, 4), a);
    cyl_f(&mut u, 0, (6, 18), (14, 27), 1, 0, a, FINISH_STEEL);
    crate_box(&mut u, (20, 28), (13, 17), (26, 30), a);
    headlight(&mut u, (54, 13, 0), 4, a);
    headlight(&mut u, (54, 13, 26), 4, a);
    for &z in &[9usize, 19] {
        fill_f(&mut u, (0, 2), (4, 6), (z, z + 2), a, FINISH_STEEL);
    }
    // Запасные траки на кормовом листе, стянутые стальной планкой.
    fill_f(&mut u, (1, 2), (7, 15), (12, 18), a, FINISH_RUBBER);
    fill_f(&mut u, (0, 2), (10, 11), (12, 18), a, FINISH_STEEL);
    // Поручни по бортам верхнего корпуса — стальные скобы.
    for &z in &[3usize, 26] {
        for k in 0..3usize {
            let x0 = 8 + k * 12;
            fill_f(&mut u, (x0, x0 + 8), (15, 16), (z, z + 1), a, FINISH_STEEL);
            fill_f(&mut u, (x0, x0 + 1), (14, 15), (z, z + 1), a, FINISH_STEEL);
            fill_f(
                &mut u,
                (x0 + 7, x0 + 8),
                (14, 15),
                (z, z + 1),
                a,
                FINISH_STEEL,
            );
        }
    }
    // Deck joins and tie-downs break up the large painted shelf without
    // covering the exposed tracks or adding a noisy all-over pattern.
    for x in [10, 22, 42] {
        fill_f(&mut u, (x, x + 1), (17, 18), (5, 9), a, FINISH_STEEL);
        fill_f(&mut u, (x, x + 1), (17, 18), (21, 25), a, FINISH_STEEL);
    }
    for z in [6, 23] {
        fill_f(&mut u, (36, 43), (17, 18), (z, z + 1), a, FINISH_GUN);
        fill_f(&mut u, (38, 40), (18, 19), (z, z + 1), a, FINISH_STEEL);
    }

    // ---------------- Моторный отсек ----------------
    // Двигатель внутри кормы под самой палубой; жалюзи — прорези
    // в палубе, сквозь которые он и виден; выхлоп двумя патрубками
    // назад через кожухи.
    fill(&mut u, (5, 20), (4, 16), (8, 22), Part::Engine);
    let mut z = 9;
    while z + 2 <= 21 {
        carve(&mut u, (6, 18), (16, 17), (z, z + 2));
        z += 4;
    }
    exhaust(&mut u, (0, 4), (14, 8), 1, Part::Engine);
    exhaust(&mut u, (0, 4), (14, 21), 1, Part::Engine);

    // Ядро — в середине корпуса, под башней.
    fill(&mut u, (28, 34), (5, 11), (12, 18), Part::Core);

    // Крыша корпуса: погон — стальное кольцо, люк механика-водителя
    // перед ним, зеркало прибора наблюдения рядом.
    ring(&mut u, (30, 15), 12, 10, (17, 18), a);
    mark_cyl(&mut u, 1, (17, 18), (30, 15), 12, FINISH_STEEL);
    lid(&mut u, (44, 9), 2, 17, a);
    fill_f(&mut u, (46, 48), (17, 18), (12, 15), a, FINISH_OPTIC);

    // ---------------- Башня ----------------
    // Крыша-многоугольник со скулами у лба, скрытый короб под ней
    // и четыре листа по рёбрам с наклоном внутрь на двадцать два
    // с половиной градуса: борта и скулы. Лоб — под маску.
    let (cheek_l, cheek_r) = plated_turret(
        &mut u,
        (24, 44),
        (18, 28),
        (8, 22),
        (8, 3),
        TURN / 16,
        11,
        2,
        true,
        a,
        FINISH_PLATE,
    );
    // Seams follow the sloping sheets in their own frames, so a damaged
    // plate carries its details when it breaks away.
    for plate in u.segments.iter_mut().rev().take(4) {
        let len = plate.body.vol.nx as usize;
        let wide = plate.body.vol.nz as usize;
        for x in (4..len.saturating_sub(1)).step_by(6) {
            mark(
                &mut plate.body,
                (x, x + 1),
                (1, 2),
                (1, wide.saturating_sub(1)),
                FINISH_GUN,
            );
        }
        mark(&mut plate.body, (0, len), (1, 2), (0, 1), FINISH_STEEL);
    }
    for z in [9, 20] {
        fill_f(&mut u, (27, 38), (28, 29), (z, z + 1), a, FINISH_STEEL);
    }
    for x in [29, 35] {
        fill_f(&mut u, (x, x + 2), (28, 29), (10, 13), a, FINISH_PLATE);
        fill_f(&mut u, (x, x + 2), (28, 29), (17, 20), a, FINISH_PLATE);
    }
    // Кормовая ниша башни ниже крыши, с корзиной из стальных прутьев.
    fill(&mut u, (16, 24), (19, 27), (10, 20), a);
    for &z in &[10usize, 19] {
        fill_f(&mut u, (12, 16), (22, 23), (z, z + 1), a, FINISH_STEEL);
        fill_f(&mut u, (12, 13), (22, 27), (z, z + 1), a, FINISH_STEEL);
        fill_f(&mut u, (12, 16), (26, 27), (z, z + 1), a, FINISH_STEEL);
    }
    fill_f(&mut u, (12, 13), (22, 27), (10, 20), a, FINISH_STEEL);
    carve(&mut u, (12, 13), (23, 26), (11, 19));
    // Дымовые гранатомёты — по три трубки на каждой скуле, потомки
    // листа: смотрят наружу, вперёд и вверх в его же координатах.
    // Вперёд у левой скулы — местная `−X` (лист идёт от маски
    // назад), у правой — `+X`; наружу — `+Y`, вверх — `−Z`.
    for (parent, fwd) in [(cheek_l, -2i32), (cheek_r, 2)] {
        for k in 0..3i16 {
            let tube = link(4, 3, 3, BODY_MM, Part::Weapon, FINISH_GUN);
            let r = link_pivot(3, 3);
            u.push_segment(
                Attach {
                    parent,
                    turret: true,
                    ..Attach::aimed((2 + 2 * k, 2, 4), (0, r, r), (fwd, 6, -5))
                },
                tube,
            );
        }
    }
    // Маска орудия — литая, круглая, выступает из лба; ствол
    // с эжектором и дульным тормозом; казённик внутри короба.
    // Ствол квадратного сечения три на три: фаска делает из него
    // восьмигранник, а круг в пять вокселей она делала бусами.
    cyl_f(&mut u, 0, (42, 48), (23, 15), 3, 0, a, FINISH_GUN);
    fill(&mut u, (34, 44), (21, 26), (12, 18), Part::Weapon);
    fill(&mut u, (47, 56), (22, 25), (14, 17), Part::Weapon);
    fill(&mut u, (50, 53), (21, 26), (13, 18), Part::Weapon);
    cyl(&mut u, 0, (56, 60), (23, 15), 2, 0, Part::Weapon);
    carve(&mut u, (56, 60), (23, 24), (15, 16));

    // Крыша башни: командирская башенка с приборами по кругу, люк
    // заряжающего со скобой, прицел наводчика, антенна на корме.
    disc(&mut u, (30, 12), 3, (28, 29), a);
    ring(&mut u, (30, 12), 3, 2, (29, 30), Part::Sensor);
    hatch(&mut u, (30, 18), 2, 28, a);
    fill(&mut u, (39, 43), (28, 30), (14, 18), a);
    fill_f(&mut u, (42, 43), (28, 30), (15, 17), a, FINISH_OPTIC);
    // Антенна — с кормы ниши назад и вверх; основание в корне.
    fill_f(&mut u, (14, 16), (24, 26), (17, 19), a, FINISH_STEEL);
    rod(
        &mut u,
        (14, 25, 18),
        (7, 30, 18),
        2,
        2,
        -1,
        a,
        FINISH_STEEL,
        true,
    );

    // Close the covered pockets above the running gear and beside the
    // turret bearing. Leave the exterior wheel/track clearances intact.
    for z in [(1, 7), (23, 29)] {
        for x in [(34, 36), (43, 45)] {
            pack(&mut u, x, (10, 11), z);
        }
    }
    for z in [(1, 4), (26, 29)] {
        pack(&mut u, (49, 51), (10, 11), z);
        pack(&mut u, (52, 53), (10, 11), z);
    }
    pack(&mut u, (40, 42), (17, 18), (7, 8));
    pack(&mut u, (41, 42), (17, 18), (22, 23));
    pack(&mut u, (26, 35), (17, 18), (24, 25));

    let mut u = seat(u, "танк");
    // Башня — всё, что выше погона. Одна величина, записанная
    // один раз: линия та же, на которой обрывается разметка корпуса.
    u.turret_y = 18;
    u.push_weapon_path(WeaponPath {
        min: [34, 21, 13],
        max: [60, 26, 18],
        axis: 0,
    });
    u
}

/// Tracked self-propelled artillery. It shares the tank's 6×3 m footprint,
/// but the tall rear ammunition bustle and elevated howitzer distinguish it
/// at the RTS camera distance. Every visible part is damageable geometry.
/// T2 electrical assault vehicle: low broad carrier, raised generator and
/// split discharge prongs. Geometry and subsystems are physically damageable.
pub fn lightning_tank() -> UnitBody {
    let mut u = UnitBody::new_scaled(64, 40, 36, 4, BODY_MM);
    let a = Part::Armour;
    running_gear(&mut u, (3, 57), 10, (1, 8), a);
    running_gear(&mut u, (3, 57), 10, (28, 35), a);
    fill_f(&mut u, (4, 54), (2, 12), (8, 28), a, FINISH_STEEL);
    fill_f(&mut u, (7, 50), (11, 15), (4, 32), a, FINISH_PLATE);
    for z in [0, 31] {
        fill_f(&mut u, (9, 47), (9, 14), (z, z + 5), a, FINISH_GUN);
        for x in [12, 25, 38] {
            fill_f(&mut u, (x, x + 8), (14, 16), (z, z + 5), a, FINISH_STEEL);
        }
    }
    fill(&mut u, (7, 19), (4, 12), (11, 25), Part::Engine);
    fill(&mut u, (27, 34), (5, 12), (15, 21), Part::Core);
    pack(&mut u, (20, 43), (14, 16), (9, 27));
    plated_turret(&mut u, (18, 44), (16, 24), (8, 28), (9, 3), TURN / 16, 10, 2, true, a, FINISH_PLATE);
    // Stacked exposed transformer coils and an armoured central conductor.
    cyl_f(&mut u, 1, (24, 36), (28, 18), 4, 0, Part::Weapon, FINISH_STEEL);
    for y in [25, 29, 33] {
        ring(&mut u, (28, 18), 7, 5, (y, y + 2), Part::Weapon);
        mark_cyl(&mut u, 1, (y, y + 2), (28, 18), 7, FINISH_OPTIC);
    }
    disc(&mut u, (28, 18), 6, (36, 38), Part::Weapon);
    fill_f(&mut u, (24, 32), (38, 40), (14, 22), Part::Sensor, FINISH_OPTIC);
    // A central conductor carries the shot; paired prongs surround its muzzle.
    fill_f(&mut u, (36, 64), (19, 23), (16, 20), Part::Weapon, FINISH_GUN);
    for z in [11, 23] {
        fill_f(&mut u, (39, 61), (18, 21), (z, z + 2), Part::Weapon, FINISH_STEEL);
        fill_f(&mut u, (57, 64), (20, 23), (z, z + 2), Part::Weapon, FINISH_OPTIC);
    }
    let mut u = seat(u, "электротанк");
    u.turret_y = 16;
    u.push_weapon_path(WeaponPath { min: [36, 19, 16], max: [64, 23, 20], axis: 0 });
    // Enlarge geometry, including turret segments and weapon mounts, while
    // keeping the final damageable cells at the world's 100 mm scale.
    u.mm = 150;
    for segment in &mut u.segments { segment.body.mm = 150; }
    u.resampled(BODY_MM).expect("enlarged electrical vehicle")
}

/// Eidolon T1: a wide, swept twin-gun craft with recessed tracked pods.
pub fn eidolon_tank() -> UnitBody {
    let mut u = UnitBody::new_scaled(72, 28, 54, 4, BODY_MM);
    let a = Part::Armour;
    // Two short, recessed drive pods beneath broad wings, instead of a
    // rectangular tank hull with full-length exposed tracks.
    running_gear(&mut u, (12, 49), 8, (4, 10), a);
    running_gear(&mut u, (12, 49), 8, (44, 50), a);
    fill_f(&mut u, (12, 50), (3, 10), (17, 37), a, FINISH_STEEL);
    for x in 4..61 {
        let width = if x < 20 { 9 + (x - 4) } else if x < 40 { 25 } else { (25usize).saturating_sub((x - 40) * 3 / 4).max(5) };
        fill_f(&mut u, (x, x + 1), (9, 12), (27 - width, 27 + width), a, FINISH_PLATE);
    }
    for z in [2, 46] {
        fill_f(&mut u, (17, 46), (10, 14), (z, z + 6), a, FINISH_GUN);
        fill_f(&mut u, (20, 40), (14, 16), (z + 1, z + 5), a, FINISH_STEEL);
        fill_f(&mut u, (23, 35), (16, 17), (z + 2, z + 4), a, FINISH_OPTIC);
    }
    fill(&mut u, (12, 23), (4, 11), (14, 34), Part::Engine);
    fill(&mut u, (29, 36), (4, 11), (24, 30), Part::Core);
    // A low, arrow-shaped rotating weapons deck with two separate breeches.
    pack(&mut u, (24, 43), (11, 14), (18, 36));
    for x in 18..48 {
        let width = if x < 28 { 8 + (x - 18) / 2 } else { 13usize.saturating_sub((x - 28) / 3).max(5) };
        fill_f(&mut u, (x, x + 1), (14, 18), (27 - width, 27 + width), a, FINISH_STEEL);
    }
    fill_f(&mut u, (23, 37), (18, 21), (21, 33), a, FINISH_PLATE);
    fill_f(&mut u, (27, 33), (21, 24), (24, 30), Part::Sensor, FINISH_OPTIC);
    for z in [17, 33] {
        fill_f(&mut u, (30, 69), (15, 19), (z, z + 4), Part::Weapon, FINISH_GUN);
        fill_f(&mut u, (63, 71), (14, 20), (z - 1, z + 5), Part::Weapon, FINISH_STEEL);
        carve(&mut u, (69, 71), (16, 18), (z + 1, z + 3));
        fill_f(&mut u, (34, 50), (19, 20), (z, z + 4), a, FINISH_OPTIC);
    }
    let mut u = seat(u, "танк эйдолонов");
    u.turret_y = 14;
    for z in [17, 33] {
        u.push_weapon_path(WeaponPath { min: [30, 14, z], max: [71, 20, z + 4], axis: 0 });
    }
    u
}

pub fn artillery() -> UnitBody {
    artillery_scaled(BODY_MM)
}
fn artillery_scaled(mm: u16) -> UnitBody {
    let mut u = UnitBody::new_scaled(60, 32, 30, 4, mm);
    let a = Part::Armour;
    running_gear(&mut u, (2, 58), 10, (1, 7), a);
    running_gear(&mut u, (2, 58), 10, (23, 29), a);

    // Low tracked hull, broad front armour and separate engine bay.
    fill(&mut u, (4, 53), (2, 12), (7, 23), a);
    fill(&mut u, (3, 55), (11, 14), (3, 27), a);
    fill_f(&mut u, (50, 57), (8, 15), (6, 24), a, FINISH_PLATE);
    for z in [0, 29] {
        for x in [12, 27, 42] {
            fill_f(&mut u, (x, x + 10), (8, 12), (z, z + 1), a, FINISH_PLATE);
            fill_f(
                &mut u,
                (x + 1, x + 2),
                (10, 12),
                (z, z + 1),
                a,
                FINISH_STEEL,
            );
        }
    }
    fill(&mut u, (6, 19), (4, 12), (9, 21), Part::Engine);
    for z in (9..21).step_by(4) {
        mark(&mut u, (7, 18), (13, 14), (z, z + 2), FINISH_GUN);
    }
    exhaust(&mut u, (0, 4), (12, 9), 1, Part::Engine);
    exhaust(&mut u, (0, 4), (12, 20), 1, Part::Engine);
    fill(&mut u, (27, 34), (5, 12), (12, 18), Part::Core);

    // The fighting compartment is taller and farther aft than the tank's
    // turret. Its square bustle carries boxed charges and spare track links.
    ring(&mut u, (32, 15), 12, 9, (14, 16), a);
    fill_f(&mut u, (17, 46), (16, 24), (6, 24), a, FINISH_PLATE);
    fill_f(&mut u, (13, 22), (18, 25), (7, 23), a, FINISH_STEEL);
    fill_f(&mut u, (18, 42), (24, 26), (8, 22), a, FINISH_PLATE);
    for z in [7, 22] {
        fill_f(&mut u, (20, 41), (21, 23), (z, z + 1), a, FINISH_STEEL);
        for x in [23, 31, 39] {
            fill_f(&mut u, (x, x + 1), (22, 26), (z, z + 1), a, FINISH_GUN);
        }
    }
    for z in [10, 19] {
        crate_box(&mut u, (14, 21), (25, 28), (z, z + 5), a);
    }
    hatch(&mut u, (27, 11), 3, 26, a);
    fill_f(
        &mut u,
        (37, 41),
        (26, 28),
        (11, 15),
        Part::Sensor,
        FINISH_OPTIC,
    );

    // A continuous breech and straight bore let connectivity damage disable
    // this specific gun. The thick muzzle brake makes its role legible.
    fill(&mut u, (35, 46), (20, 27), (12, 18), Part::Weapon);
    for x in [45, 50, 55] {
        fill_f(
            &mut u,
            (x, (x + 6).min(60)),
            (25, 28),
            (13, 17),
            Part::Weapon,
            FINISH_GUN,
        );
    }
    fill_f(
        &mut u,
        (56, 60),
        (24, 29),
        (12, 18),
        Part::Weapon,
        FINISH_STEEL,
    );
    carve(&mut u, (58, 60), (26, 27), (14, 16));
    fill_f(&mut u, (42, 47), (18, 21), (10, 20), a, FINISH_STEEL);
    for z in [11, 18] {
        // Bolt the rail down to the hull at y=13 instead of floating above it.
        fill_f(&mut u, (5, 13), (13, 16), (z, z + 1), a, FINISH_STEEL);
    }

    // The covered bearing is a packed physical coupling, not an empty ring.
    pack(&mut u, (24, 41), (14, 16), (7, 24));
    let mut u = seat(u, "артиллерия");
    u.turret_y = 16;
    u.push_weapon_path(WeaponPath {
        min: [35, 24, 12],
        max: [60, 29, 18],
        axis: 0,
    });
    u
}

/// T2 tracked assault artillery. Thick cheek plates protect the breech;
/// the broad, short canister bore and ammunition pods distinguish its tier.
pub fn artillery_t2() -> UnitBody {
    let mut u = artillery_scaled(150);
    for z in [3, 24] {
        fill_f(
            &mut u,
            (19, 43),
            (16, 26),
            (z, z + 3),
            Part::Armour,
            FINISH_STEEL,
        );
        for x in [22, 31, 39] {
            fill_f(
                &mut u,
                (x, x + 2),
                (20, 27),
                (z, z + 1),
                Part::Armour,
                FINISH_GUN,
            );
        }
    }
    for z in [7, 20] {
        crate_box(&mut u, (5, 16), (14, 22), (z, z + 3), Part::Armour);
    }
    fill_f(
        &mut u,
        (34, 48),
        (19, 28),
        (11, 19),
        Part::Weapon,
        FINISH_GUN,
    );
    fill_f(
        &mut u,
        (47, 60),
        (22, 30),
        (11, 19),
        Part::Weapon,
        FINISH_STEEL,
    );
    carve(&mut u, (58, 60), (25, 27), (14, 16));
    // Place the physical muzzle at the centre of the new, wider bore.
    std::sync::Arc::make_mut(&mut u.weapon_paths)[0] = WeaponPath {
        min: [34, 22, 11],
        max: [60, 30, 19],
        axis: 0,
    };
    u
}

/// Fixed heavy howitzer: a wide steel plinth, armoured ammunition bustle,
/// connected breech, recoil rails and one damageable cluster shell barrel.
pub fn artillery_turret_t2() -> UnitBody {
    let mut u = UnitBody::new_scaled(60, 40, 40, 12, 150);
    let a = Part::Armour;
    fill_f(&mut u, (2, 58), (1, 7), (2, 38), a, FINISH_STEEL);
    fill_f(&mut u, (7, 49), (7, 17), (6, 34), a, FINISH_PLATE);
    fill(&mut u, (23, 37), (3, 17), (13, 27), Part::Core);
    ring(&mut u, (30, 20), 16, 12, (16, 19), a);
    fill_f(&mut u, (12, 46), (19, 30), (8, 32), a, FINISH_PLATE);
    fill_f(&mut u, (9, 20), (22, 33), (9, 31), a, FINISH_STEEL);
    fill_f(&mut u, (18, 41), (30, 33), (10, 30), a, FINISH_PLATE);
    fill(&mut u, (33, 48), (23, 31), (16, 24), Part::Weapon);
    fill_f(
        &mut u,
        (45, 60),
        (26, 34),
        (16, 24),
        Part::Weapon,
        FINISH_GUN,
    );
    fill_f(&mut u, (54, 60), (25, 35), (15, 25), a, FINISH_STEEL);
    carve(&mut u, (58, 60), (29, 31), (19, 21));
    for z in [12, 26] {
        fill_f(&mut u, (30, 54), (24, 28), (z, z + 2), a, FINISH_STEEL);
        for x in [16, 24, 32] {
            crate_box(&mut u, (x, x + 6), (32, 36), (z, z + 2), a);
        }
    }
    hatch(&mut u, (22, 17), 3, 33, a);
    fill_f(
        &mut u,
        (29, 33),
        (33, 37),
        (18, 22),
        Part::Sensor,
        FINISH_OPTIC,
    );
    pack(&mut u, (19, 42), (17, 19), (9, 32));
    let mut u = seat(u, "артиллерийская турель T2");
    u.turret_y = 18;
    u.push_weapon_path(WeaponPath {
        min: [33, 26, 16],
        max: [60, 34, 24],
        axis: 0,
    });
    u
}

/// Fixed launch bunker with a connected upright missile and protected core.
pub fn nuclear_silo() -> UnitBody {
    let mut u = UnitBody::new_scaled(64, 72, 64, 16, 150);
    fill_f(&mut u, (2, 62), (1, 9), (2, 62), Part::Armour, FINISH_STEEL);
    fill_f(
        &mut u,
        (7, 57),
        (9, 22),
        (7, 57),
        Part::Armour,
        FINISH_PLATE,
    );
    fill(&mut u, (24, 40), (3, 20), (24, 40), Part::Core);
    // Connected launch tube and upright missile; the command bunker is offset.
    fill_f(
        &mut u,
        (20, 44),
        (22, 48),
        (20, 44),
        Part::Weapon,
        FINISH_GUN,
    );
    fill_f(
        &mut u,
        (26, 38),
        (48, 63),
        (26, 38),
        Part::Weapon,
        FINISH_STEEL,
    );
    fill_f(
        &mut u,
        (28, 36),
        (63, 69),
        (28, 36),
        Part::Weapon,
        FINISH_GUN,
    );
    for z in [16, 44] {
        fill_f(
            &mut u,
            (17, 47),
            (22, 26),
            (z, z + 4),
            Part::Armour,
            FINISH_STEEL,
        );
    }
    fill_f(
        &mut u,
        (6, 18),
        (22, 33),
        (8, 24),
        Part::Armour,
        FINISH_PLATE,
    );
    fill_f(
        &mut u,
        (7, 12),
        (33, 38),
        (10, 16),
        Part::Sensor,
        FINISH_OPTIC,
    );
    let mut u = seat(u, "ядерная ракетная шахта");
    u.turret_y = 72; // Entire building stays fixed while targeting.
    u.push_weapon_path(WeaponPath {
        min: [26, 22, 26],
        max: [38, 69, 38],
        axis: 1,
    });
    u
}

/// Fixed T3 siege artillery: low faceted head, three charging chambers and
/// one long barrel. Separate from the wave turret «Разлом».
pub fn artillery_turret_t3() -> UnitBody {
    let mut u = UnitBody::new_scaled(112, 52, 56, 16, 150);
    let a = Part::Armour;
    fill_f(&mut u, (36, 76), (0, 5), (8, 48), a, FINISH_STEEL);
    for x in [(30, 47), (65, 82)] {
        for z in [(2, 19), (37, 54)] {
            fill_f(&mut u, x, (0, 3), z, a, FINISH_GUN);
            fill_f(&mut u, (x.0 + 1, x.1 - 1), (3, 5), (z.0 + 1, z.1 - 1), a, FINISH_STEEL);
            mark(&mut u, (x.0 + 3, x.1 - 3), (4, 5), (z.0 + 2, z.0 + 5), FINISH_PLATE);
        }
    }
    fill_f(&mut u, (38, 74), (5, 8), (10, 46), a, FINISH_GUN);
    for (ys, radius) in [((8, 11), 20isize), ((11, 14), 19), ((14, 16), 16), ((16, 19), 14)] {
        for dx in -radius..radius {
            for dz in -radius..radius {
                if dx.abs().max((dx + 1).abs()) + dz.abs().max((dz + 1).abs()) > radius * 3 / 2 { continue; }
                let x = (56 + dx) as usize; let z = (28 + dz) as usize;
                fill_f(&mut u, (x, x + 1), ys, (z, z + 1), a, FINISH_STEEL);
            }
        }
    }
    for x in [(44, 47), (65, 68)] {
        mark(&mut u, x, (14, 19), (17, 39), FINISH_PLATE);
    }
    for r in 15isize..23 {
        for t in -8isize..8 {
            let cells = [(56+r,28+t), (55-t,28+r), (55-r,27-t), (56+t,27-r)];
            for (x,z) in cells {
                let (x,z) = (x as usize,z as usize);
                fill_f(&mut u, (x,x+1), (9,18), (z,z+1), a, FINISH_STEEL);
                if t >= -6 && t < 6 {
                    carve(&mut u, (x,x+1), (11,16), (z,z+1));
                }
            }
        }
    }
    for t in -6isize..6 {
        for (x,z) in [(70,28+t),(55-t,42),(41,27-t),(56+t,13)] {
            fill_f(&mut u, (x as usize,x as usize+1), (11,16), (z as usize,z as usize+1), a, FINISH_GUN);
        }
    }
    let port_cells = |r, t| [(55-t,28+r), (55-r,27-t), (56+t,27-r)];
    for r in 15isize..17 {
        for t in [-4isize, 3] {
            for (x,z) in port_cells(r,t) {
                fill_f(&mut u, (x as usize,x as usize+1), (13,14), (z as usize,z as usize+1), a, FINISH_GUN);
            }
        }
    }
    for (r, half, bottom, top) in [(20isize, 6isize, 11usize, 16usize), (17, 4, 12, 15)] {
        for t in -half..half {
            for y in bottom..top {
                if y != bottom && y + 1 != top && t != -half && t + 1 != half { continue; }
                for (x,z) in port_cells(r,t) {
                    fill_f(&mut u, (x as usize,x as usize+1), (y,y+1), (z as usize,z as usize+1), a, FINISH_ENERGY);
                }
            }
        }
    }
    for t in -2isize..2 {
        for (x,z) in port_cells(14,t) {
            mark(&mut u, (x as usize,x as usize+1), (13,14), (z as usize,z as usize+1), FINISH_ENERGY);
        }
    }
    fill_f(&mut u, (71, 78), (12, 16), (23, 33), Part::Weapon, FINISH_STEEL);
    fill_f(&mut u, (78, 106), (11, 16), (22, 34), Part::Weapon, FINISH_STEEL);
    fill_f(&mut u, (106, 112), (10, 17), (20, 36), Part::Weapon, FINISH_STEEL);
    carve(&mut u, (110, 112), (11, 16), (22, 34));
    fill(&mut u, (48, 64), (1, 17), (20, 36), Part::Core);
    fill_f(&mut u, (53, 59), (17, 19), (25, 31), Part::Sensor, FINISH_STEEL);
    let mut u = seat(u, "осадная артиллерийская турель T3");
    u.turret_y = 8;
    u.push_weapon_path(WeaponPath { min: [71, 10, 20], max: [112, 17, 36], axis: 0 });
    u
}

/// T3 «Разлом»: broad wave turret with a recessed seven-segment emitter.
pub fn razlom_t3() -> UnitBody {
    let mut u = UnitBody::new_scaled(48, 24, 48, 8, 300);
    let a = Part::Armour;

    fill_f(&mut u, (7, 41), (0, 5), (12, 36), a, FINISH_GUN);
    for x in [6usize, 30] {
        for z in [2usize, 34] {
            fill_f(&mut u, (x, x + 13), (0, 2), (z, z + 13), a, FINISH_GUN);
            fill_f(&mut u, (x + 1, x + 12), (2, 5), (z + 1, z + 12), a, FINISH_IVORY);
            mark(&mut u, (x + 3, x + 6), (4, 5), (z + 2, z + 11), FINISH_PLATE);
            fill_f(&mut u, (x + 4, x + 9), (5, 6), (z + 4, z + 9), a, FINISH_GUN);
            for y in [2usize, 3] {
                for dz in [2usize, 5, 8] {
                    mark(&mut u, (x + 11, x + 12), (y, y + 1), (z + dz, z + dz + 2), FINISH_GUN);
                }
            }
        }
    }
    for (ys, radius) in [((5, 7), 13isize), ((7, 9), 12), ((9, 10), 9)] {
        for dx in -radius..radius {
            for dz in -radius..radius {
                if dx.abs().max((dx + 1).abs()) + dz.abs().max((dz + 1).abs()) > radius * 3 / 2 { continue; }
                let (x, z) = ((24 + dx) as usize, (24 + dz) as usize);
                fill_f(&mut u, (x, x + 1), ys, (z, z + 1), a, FINISH_GUN);
            }
        }
    }
    fill_f(&mut u, (15, 33), (9, 11), (19, 29), a, FINISH_STEEL);
    fill(&mut u, (18, 30), (2, 10), (20, 28), Part::Core);

    for z in [(4, 8), (40, 44)] {
        fill_f(&mut u, (18, 35), (10, 22), z, a, FINISH_GUN);
        fill_f(&mut u, (20, 34), (12, 21), z, a, FINISH_IVORY);
        for x in [22usize, 29] {
            mark(&mut u, (x, x + 2), (20, 21), z, FINISH_PLATE);
        }
        let edge = if z.0 == 4 { (7, 8) } else { (40, 41) };
        fill_f(&mut u, (27, 30), (15, 18), edge, a, FINISH_ENERGY);
    }
    fill_f(&mut u, (18, 31), (13, 18), (8, 40), a, FINISH_GUN);
    fill_f(&mut u, (18, 35), (18, 22), (8, 40), a, FINISH_IVORY);
    fill_f(&mut u, (18, 35), (11, 13), (8, 40), a, FINISH_IVORY);
    for z in [12usize, 24, 34] {
        mark(&mut u, (23, 34), (21, 22), (z, z + 3), FINISH_PLATE);
        mark(&mut u, (23, 34), (11, 12), (z, z + 3), FINISH_PLATE);
    }
    for x in [20usize, 27] {
        for z in [10usize, 20, 30] {
            mark(&mut u, (x, x + 1), (18, 19), (z, z + 6), FINISH_STEEL);
        }
    }
    fill_f(&mut u, (30, 31), (14, 16), (8, 40), Part::Weapon, FINISH_GUN);
    fill_f(&mut u, (24, 31), (14, 16), (22, 26), Part::Weapon, FINISH_GUN);
    for z in [11usize, 15, 19, 23, 27, 31, 35] {
        fill_f(&mut u, (30, 31), (14, 16), (z, z + 3), Part::Weapon, FINISH_ENERGY);
    }
    fill_f(&mut u, (29, 30), (17, 19), (40, 42), Part::Sensor, FINISH_OPTIC);
    for z in [8usize, 38] {
        fill_f(&mut u, (34, 35), (13, 14), (z, z + 2), a, FINISH_GUN);
        fill_f(&mut u, (34, 35), (18, 19), (z, z + 2), a, FINISH_GUN);
    }

    let mut u = seat(u, "T3 Разлом · волновая турель");
    u.turret_y = 9;
    u.push_weapon_path(WeaponPath { min: [24, 13, 8], max: [31, 18, 40], axis: 0 });
    u
}

/// «Гребень»: closed storage slabs, a fixed two-dock hub and amber fill gauges.
pub fn greben_storage_modules(modules: u8) -> UnitBody {
    assert!((1..=4).contains(&modules));
    let extra = modules.saturating_sub(2) * 40;
    let mut u = UnitBody::new_scaled(80 + extra, 104, 176, 8, 800);
    let a = Part::Armour;

    fill_f(&mut u, (0, 40), (0, 4), (0, 176), a, FINISH_GUN);
    fill_f(&mut u, (0, 40), (4, 8), (0, 176), a, FINISH_STEEL);
    fill_f(&mut u, (36, 40), (8, 24), (0, 176), a, FINISH_GUN);
    fill_f(&mut u, (0, 40), (8, 24), (0, 8), a, FINISH_GUN);
    fill_f(&mut u, (0, 40), (8, 24), (168, 176), a, FINISH_GUN);
    fill_f(&mut u, (0, 40), (8, 24), (32, 48), a, FINISH_GUN);
    fill_f(&mut u, (0, 40), (8, 24), (88, 104), a, FINISH_GUN);
    fill_f(&mut u, (0, 40), (8, 24), (144, 160), a, FINISH_GUN);
    fill_f(&mut u, (0, 40), (16, 24), (0, 176), a, FINISH_IVORY);
    for z in [8usize, 48] {
        fill_f(&mut u, (0, 4), (8, 16), (z, z + 2), a, FINISH_STEEL);
        fill_f(&mut u, (0, 4), (8, 16), (z + 22, z + 24), a, FINISH_STEEL);
        for y in [9usize, 13] {
            mark(&mut u, (0, 2), (y, y + 1), (z + 4, z + 20), FINISH_OPTIC);
        }
    }

    for index in 0..modules {
        let centre = 20usize + usize::from(index) * 40;
        fill_f(&mut u, (40, 176), (0, 8), (centre - 20, centre + 20), a, FINISH_GUN);
        fill_f(&mut u, (44, 172), (8, 104), (centre - 16, centre + 16), a, FINISH_IVORY);
        fill_f(&mut u, (40, 44), (8, 104), (centre - 16, centre + 16), a, FINISH_STEEL);
        fill_f(&mut u, (172, 176), (8, 104), (centre - 16, centre + 16), a, FINISH_GUN);
        mark(&mut u, (52, 168), (103, 104), (centre - 4, centre + 4), FINISH_PLATE);
        mark(&mut u, (44, 45), (20, 52), (centre - 2, centre + 2), FINISH_OPTIC);
        for y in [20usize, 28, 36, 44] {
            mark(&mut u, (44, 45), (y, y + 2), (centre - 2, centre + 2), FINISH_ENERGY);
        }
        for x in [64usize, 96, 128, 160] {
            mark(&mut u, (x, x + 2), (103, 104), (centre - 16, centre + 16), FINISH_GUN);
        }
        for y in [16usize, 40, 64, 88] {
            mark(&mut u, (44, 45), (y, y + 2), (centre - 16, centre + 16), FINISH_PLATE);
            mark(&mut u, (171, 172), (y, y + 2), (centre - 16, centre + 16), FINISH_GUN);
        }
    }
    fill(&mut u, (70, 78), (12, 20), (16, 24), Part::Core);
    fill_f(&mut u, (28, 36), (8, 16), (76, 84), Part::Engine, FINISH_GUN);
    fill_f(&mut u, (4, 8), (20, 24), (76, 84), Part::Sensor, FINISH_OPTIC);
    pack(&mut u, (36, 40), (8, 12), (36, 44));
    let mut u = seat(u, "Гребень · модульное хранилище");
    u.turret_y = u.vol.ny;
    u
}

pub fn greben_storage() -> UnitBody {
    greben_storage_modules(2)
}

/// Skoba's two rails carry a typed load in an open frame. The compact pose
/// keeps every rail and thruster, moving the sides together for the return.
pub fn skoba_drone(compact: bool) -> UnitBody {
    let mut u = UnitBody::new_scaled(128, 24, 62, 2, 50);
    let a = Part::Armour;
    let rails = if compact { [(26, 30), (32, 36)] } else { [(5, 9), (53, 57)] };
    for (n, z) in rails.into_iter().enumerate() {
        fill_f(&mut u, (2, 126), (16, 18), z, a, FINISH_GUN);
        fill_f(&mut u, (2, 126), (18, 20), z, a, FINISH_IVORY);
        fill_f(&mut u, (0, 2), (16, 20), z, a, FINISH_STEEL);
        fill_f(&mut u, (126, 128), (16, 20), z, a, FINISH_GUN);
        for x in (8..120).step_by(16) {
            mark(&mut u, (x, x + 8), (19, 20), z, FINISH_PLATE);
        }
        for x in [14usize, 62, 106] {
            let engine_z = if n == 0 {
                if compact { (22, 27) } else { (0, 5) }
            } else if compact { (35, 40) } else { (57, 62) };
            fill_f(&mut u, (x, x + 8), (12, 17), engine_z, a, FINISH_GUN);
            fill_f(&mut u, (x + 1, x + 7), (14, 16), engine_z, a, FINISH_STEEL);
            let glow_z = if n == 0 {
                (engine_z.0 + 1, engine_z.0 + 4)
            } else {
                (engine_z.1 - 4, engine_z.1 - 1)
            };
            mark(&mut u, (x + 2, x + 6), (12, 13), glow_z, FINISH_OPTIC);
        }
        for x in [18usize, 106] {
            fill_f(&mut u, (x, x + 4), (10, 17), z, a, FINISH_GUN);
        }
        if compact {
            fill_f(&mut u, (8, 34), (12, 17), z, a, FINISH_STEEL);
            for x in [12usize, 24] {
                mark(&mut u, (x, x + 3), (11, 12), z, FINISH_PLATE);
            }
        } else {
            let inner = if n == 0 { (8, 12) } else { (50, 54) };
            fill_f(&mut u, (2, 8), (12, 17), inner, a, FINISH_STEEL);
            let grip = if n == 0 { (10, 13) } else { (49, 52) };
            fill_f(&mut u, (8, 12), (10, 13), grip, a, FINISH_GUN);
            mark(&mut u, (10, 12), (10, 12), grip, FINISH_PLATE);
        }
    }
    let bridge = if compact { (26, 36) } else { (5, 57) };
    fill_f(&mut u, (124, 128), (16, 20), bridge, a, FINISH_GUN);
    for z in if compact { vec![26, 32] } else { vec![8, 20, 38, 50] } {
        mark(&mut u, (122, 126), (18, 20), (z, z + 4), FINISH_STEEL);
    }
    fill_f(&mut u, (112, 126), (16, 20), (29, 33), a, FINISH_GUN);
    fill_f(&mut u, (116, 124), (14, 18), (29, 33), Part::Engine, FINISH_STEEL);
    fill_f(&mut u, (122, 126), (16, 20), (29, 33), Part::Core, FINISH_CORE);
    fill_f(&mut u, (0, 4), (18, 22), rails[0], Part::Sensor, FINISH_OPTIC);
    pack(&mut u, (120, 128), (14, 20), (27, 35));
    let mut u = seat(u, if compact { "Скоба · компактный возврат" } else { "Скоба · строительный дрон" });
    u.turret_y = u.vol.ny;
    u
}

pub fn skoba() -> UnitBody {
    skoba_drone(false)
}

pub fn skoba_with_cargo() -> UnitBody {
    let mut u = skoba_drone(false);
    for x in [10usize, 38, 66, 94] {
        fill_f(&mut u, (x, x + 24), (0, 12), (11, 51), Part::Armour, FINISH_STEEL);
        mark(&mut u, (x + 4, x + 8), (0, 12), (11, 51), FINISH_PLATE);
        mark(&mut u, (x + 12, x + 20), (10, 12), (11, 12), FINISH_IVORY);
    }
    fill_f(&mut u, (10, 118), (10, 13), (29, 33), Part::Armour, FINISH_GUN);
    u
}

/// KIL: one tank factory growing on the same 640×480 U site.
pub fn kil_factory(tier: u8) -> UnitBody {
    assert!((1..=4).contains(&tier));
    let (depth, height, gate_width, gate_height) = match tier {
        1 => (22, 9, 10, 7),
        2 => (28, 13, 13, 10),
        3 => (35, 19, 19, 14),
        _ => (44, 27, 28, 22),
    };
    let mut u = UnitBody::new_scaled(80, 36, 60, 8, 800);
    let a = Part::Armour;
    fill_f(&mut u, (0, 80), (0, 2), (0, 60), a, FINISH_GUN);
    for (x, z, w, d, h) in [
        (4usize, 4usize, 13usize, 12usize, 4usize),
        (63, 4, 14, 15, 4),
        (4, 43, 15, 12, 5),
        (62, 43, 14, 13, 5),
        (4, 23, 10, 12, 3),
        (66, 23, 10, 12, 3),
    ] {
        fill_f(&mut u, (x, x + w), (2, 2 + h), (z, z + d), a, FINISH_IVORY);
        fill_f(&mut u, (x + 1, x + w - 1), (2 + h, 3 + h), (z + 1, z + d - 1), a, FINISH_STEEL);
        fill_f(&mut u, (x + 2, x + w - 2), (3 + h, 4 + h), (z + 2, z + d - 2), a, FINISH_GUN);
        for dx in [2usize, 6, 10].into_iter().filter(|dx| *dx + 2 < w) {
            mark(&mut u, (x + dx, x + dx + 1), (3, 2 + h), (z, z + 1), FINISH_GUN);
            mark(&mut u, (x + dx, x + dx + 1), (3, 2 + h), (z + d - 1, z + d), FINISH_PLATE);
        }
        for zz in (z + 3..z + d - 2).step_by(3) {
            mark(&mut u, (x, x + 1), (3, 2 + h), (zz, zz + 1), FINISH_GUN);
            mark(&mut u, (x + w - 1, x + w), (3, 2 + h), (zz, zz + 1), FINISH_PLATE);
        }
        mark(&mut u, (x, x + 1), (3, 2 + h), (z + 3, z + 5), FINISH_OPTIC);
        fill_f(&mut u, (x + w - 4, x + w - 2), (3 + h, 7 + h), (z + 2, z + 4), a, FINISH_GUN);
        fill_f(&mut u, (x + w - 5, x + w - 1), (7 + h, 8 + h), (z + 1, z + 5), a, FINISH_STEEL);
        mark(&mut u, (x + w - 4, x + w - 2), (4 + h, 5 + h), (z + 2, z + 3), FINISH_ENERGY);
    }

    let front = 3usize;
    let back = front + depth;
    let wall = if tier == 4 { 3 } else { 2 };
    fill_f(&mut u, (front, back), (2, 2 + height), (30 - gate_width / 2 - wall, 30 - gate_width / 2), a, FINISH_IVORY);
    fill_f(&mut u, (front, back), (2, 2 + height), (30 + gate_width / 2, 30 + gate_width / 2 + wall), a, FINISH_IVORY);
    fill_f(&mut u, (front, back), (2 + gate_height, 2 + height), (30 - gate_width / 2, 30 + gate_width / 2), a, FINISH_IVORY);
    fill_f(&mut u, (back - wall, back), (2, 2 + height), (30 - gate_width / 2, 30 + gate_width / 2), a, FINISH_GUN);
    for z in (30 - gate_width / 2..30 + gate_width / 2).step_by(4) {
        mark(&mut u, (back - 1, back), (4, 2 + height - 1), (z, z + 2), FINISH_IVORY);
    }
    for x in (front + 2..back - 2).step_by(4) {
        mark(&mut u, (x, x + 1), (2 + height - 1, 2 + height), (30 - gate_width / 2, 30 + gate_width / 2), FINISH_STEEL);
    }
    let ribs = match tier {
        1 => vec![front + depth / 2],
        2 => vec![front + depth / 3, front + depth * 2 / 3],
        3 => vec![front + depth / 4, front + depth / 2, front + depth * 3 / 4],
        _ => Vec::new(),
    };
    for x in ribs {
        for z in [30 - gate_width / 2 - 3, 30 + gate_width / 2 + 2] {
            fill_f(&mut u, (x, x + 2), (2, 2 + height + 2), (z, z + 2), a, FINISH_STEEL);
            mark(&mut u, (x, x + 1), (4, 2 + height), (z, z + 1), FINISH_IVORY);
        }
    }
    for z in [12usize, 44] {
        mark(&mut u, (front + 4, back - 4), (2 + height, 2 + height + 1), (z, z + 3), FINISH_PLATE);
    }
    for z in [30 - gate_width / 2 - wall - 1, 30 + gate_width / 2 + wall] {
        fill_f(&mut u, (front + 1, back - 1), (3, 5), (z, z + 1), a, FINISH_GUN);
        for x in (front + 4..back - 3).step_by(6) {
            mark(&mut u, (x, x + 2), (4, 5), (z, z + 1), FINISH_ENERGY);
        }
    }
    for z in [30 - gate_width / 2 - wall, 30 + gate_width / 2] {
        fill_f(&mut u, (1, 5), (2, 3 + gate_height), (z, z + wall), a, FINISH_STEEL);
        mark(&mut u, (1, 2), (3, 2 + gate_height), (z, z + 1), FINISH_ENERGY);
    }
    fill_f(&mut u, (1, 7), (2 + gate_height, 3 + gate_height), (30 - gate_width / 2 - wall, 30 + gate_width / 2 + wall), a, FINISH_GUN);
    for x in [front + 1, back - 3] {
        fill_f(&mut u, (x, x + 2), (2 + height, 3 + height), (30 - gate_width / 2 - wall, 30 + gate_width / 2 + wall), a, FINISH_STEEL);
    }
    let rotor_y = 2 + height + 3;
    let rotor_radius = 4 + i32::from(tier) * 2;
    fill_f(&mut u, (46, 54), (2, rotor_y), (44, 52), a, FINISH_GUN);
    for y in (5..rotor_y).step_by(4) {
        mark(&mut u, (46, 54), (y, y + 1), (44, 52), FINISH_ENERGY);
    }
    disc(&mut u, (50, 48), rotor_radius, (rotor_y, rotor_y + 1), a);
    mark_cyl(&mut u, 1, (rotor_y, rotor_y + 1), (50, 48), rotor_radius, FINISH_ENERGY);
    disc(&mut u, (50, 48), 2 + i32::from(tier), (rotor_y + 1, rotor_y + 2), a);
    mark_cyl(&mut u, 1, (rotor_y + 1, rotor_y + 2), (50, 48), 2 + i32::from(tier), FINISH_OPTIC);
    fill_f(&mut u, (22, 30), (2, 10), (26, 34), Part::Core, FINISH_GUN);
    fill_f(&mut u, (48, 54), (2, 8), (50, 56), Part::Engine, FINISH_GUN);
    fill_f(&mut u, (4, 8), (4, 8), (27, 33), Part::Sensor, FINISH_OPTIC);
    pack(&mut u, (back - wall, back), (2, 4), (27, 33));
    let mut u = seat(u, match tier { 1 => "Киль T1", 2 => "Киль T2", 3 => "Киль T3", _ => "Киль T4" });
    u.turret_y = u.vol.ny;
    u
}

/// T1 point-defence turret: a low armoured socket and one conventional gun.
/// The base stays fixed; only the upper voxels traverse toward a target.
pub fn turret_t1() -> UnitBody {
    let mut u = UnitBody::new_scaled(36, 30, 36, 3, BODY_MM);
    let a = Part::Armour;
    fill_f(&mut u, (2, 34), (1, 5), (2, 34), a, FINISH_STEEL);
    fill_f(&mut u, (5, 31), (5, 12), (5, 31), a, FINISH_PLATE);
    fill(&mut u, (14, 22), (5, 13), (14, 22), Part::Core);
    ring(&mut u, (18, 18), 12, 8, (12, 14), a);
    fill_f(&mut u, (9, 27), (14, 23), (10, 26), a, FINISH_PLATE);
    fill_f(&mut u, (10, 26), (23, 25), (11, 25), a, FINISH_STEEL);
    fill_f(
        &mut u,
        (24, 31),
        (18, 23),
        (14, 22),
        Part::Weapon,
        FINISH_GUN,
    );
    fill_f(
        &mut u,
        (29, 36),
        (19, 22),
        (16, 20),
        Part::Weapon,
        FINISH_GUN,
    );
    fill_f(
        &mut u,
        (33, 36),
        (18, 23),
        (15, 21),
        Part::Weapon,
        FINISH_STEEL,
    );
    fill_f(
        &mut u,
        (11, 16),
        (25, 27),
        (16, 20),
        Part::Sensor,
        FINISH_OPTIC,
    );
    pack(&mut u, (11, 26), (12, 14), (11, 26));
    let mut u = seat(u, "турель T1");
    u.turret_y = 13;
    u.push_weapon_path(WeaponPath {
        min: [24, 18, 14],
        max: [36, 23, 22],
        axis: 0,
    });
    u
}

/// T2 emplacement: a reinforced socket and a single energy cannon.
pub fn turret_t2() -> UnitBody {
    let mut u = UnitBody::new_scaled(48, 42, 48, 14, BODY_MM);
    let a = Part::Armour;
    fill_f(&mut u, (2, 46), (1, 6), (2, 46), a, FINISH_STEEL);
    fill_f(&mut u, (2, 46), (6, 17), (2, 46), a, FINISH_PLATE);
    fill(&mut u, (16, 32), (3, 17), (16, 32), Part::Core);
    ring(&mut u, (24, 24), 16, 12, (15, 18), a);
    fill_f(&mut u, (8, 43), (18, 31), (9, 39), a, FINISH_PLATE);
    fill_f(&mut u, (13, 35), (31, 34), (14, 34), a, FINISH_STEEL);
    for z in [9, 36] {
        fill_f(&mut u, (13, 34), (20, 28), (z, z + 3), a, FINISH_STEEL);
    }
    fill_f(
        &mut u,
        (30, 40),
        (24, 31),
        (19, 29),
        Part::Weapon,
        FINISH_GUN,
    );
    // Continuous jacket carries the barrel between its breech and muzzle.
    fill_f(&mut u, (39, 48), (24, 32), (19, 29), a, FINISH_STEEL);
    fill_f(
        &mut u,
        (39, 48),
        (26, 30),
        (21, 27),
        Part::Weapon,
        FINISH_GUN,
    );
    // A physical mantlet surrounds the breech. Its outer shell may carry
    // a chipped cannon, but does not bridge a severed muzzle.
    for z in [(16, 19), (29, 32)] {
        fill_f(&mut u, (32, 43), (22, 33), z, a, FINISH_STEEL);
    }
    fill_f(
        &mut u,
        (44, 48),
        (24, 32),
        (19, 29),
        Part::Weapon,
        FINISH_STEEL,
    );
    fill_f(
        &mut u,
        (17, 23),
        (34, 37),
        (20, 28),
        Part::Sensor,
        FINISH_OPTIC,
    );
    // Paired exposed capacitor towers identify the energy emplacement even
    // from a high RTS camera, while the cannon remains one damageable barrel.
    for z in [8, 34] {
        fill_f(&mut u, (14, 22), (28, 39), (z, z + 6), a, FINISH_STEEL);
        fill_f(&mut u, (15, 21), (39, 41), (z + 1, z + 5), a, FINISH_CORE);
        fill_f(&mut u, (16, 20), (33, 35), (z, z + 6), a, FINISH_CORE);
    }
    pack(&mut u, (13, 36), (17, 18), (13, 36));
    let mut u = seat(u, "турель T2");
    u.turret_y = 17;
    u.push_weapon_path(WeaponPath {
        min: [30, 24, 19],
        max: [48, 32, 29],
        axis: 0,
    });
    u
}

/// Destructible command base for the map-wide battle. The broad foundation,
/// corner bastions and raised command crown keep it readable at RTS height;
/// the sheltered core determines whether the base is still operational.
pub fn command_base() -> UnitBody {
    let mut u = UnitBody::new_scaled(80, 58, 80, 8, BODY_MM);
    let a = Part::Armour;
    fill_f(&mut u, (2, 78), (1, 6), (2, 78), a, FINISH_STEEL);
    fill_f(&mut u, (7, 73), (6, 16), (7, 73), a, FINISH_PLATE);
    for x in [7, 61] {
        for z in [7, 61] {
            fill_f(&mut u, (x, x + 12), (16, 38), (z, z + 12), a, FINISH_PLATE);
            fill_f(
                &mut u,
                (x + 2, x + 10),
                (38, 41),
                (z + 2, z + 10),
                a,
                FINISH_STEEL,
            );
        }
    }
    fill_f(&mut u, (16, 64), (16, 41), (16, 64), a, FINISH_PLATE);
    fill(&mut u, (31, 49), (16, 39), (31, 49), Part::Core);
    fill_f(&mut u, (19, 61), (41, 46), (19, 61), a, FINISH_STEEL);
    fill_f(&mut u, (25, 55), (46, 51), (25, 55), a, FINISH_PLATE);
    fill_f(
        &mut u,
        (33, 47),
        (51, 55),
        (33, 47),
        Part::Sensor,
        FINISH_OPTIC,
    );
    for z in [20, 56] {
        fill_f(&mut u, (28, 52), (45, 48), (z, z + 4), a, FINISH_CORE);
    }
    // Recessed entrance, cooling shutters and antenna groups break up the
    // large surfaces without introducing a non-physical decorative mesh.
    carve(&mut u, (70, 80), (7, 23), (34, 46));
    // The receiving bay is readable on the same destructible facade:
    // contrasting guide bars and a signal above each side of the entrance.
    for z in [24, 28, 48, 52] {
        mark(&mut u, (72, 73), (8, 15), (z, z + 2), FINISH_GUN);
    }
    for z in [31, 47] {
        mark(&mut u, (72, 73), (12, 15), (z, z + 2), FINISH_OPTIC);
    }
    for z in [23, 30, 47, 54] {
        // Reach the main shell at x=16; a one-cell gap left all four
        // shutter groups detached even before the base took damage.
        fill_f(&mut u, (10, 16), (23, 29), (z, z + 3), a, FINISH_GUN);
    }
    for x in [29, 50] {
        fill_f(
            &mut u,
            (x, x + 2),
            (51, 58),
            (29, 31),
            Part::Sensor,
            FINISH_STEEL,
        );
    }
    seat(u, "командная база")
}

#[cfg(test)]
mod command_base_tests {
    use super::*;
    #[test]
    fn razlom_has_a_real_recess_and_connected_gun() {
        let b = razlom_t3();
        assert!(!b.vol.get(33,15,24), "emitter mouth is not open");
        assert!(b.vol.get(30,15,24), "emitter backing is missing");
        let s = b.status(&mut CoreBufs::new());
        assert!(s.alive && s.weapon && s.sensor && s.detached == 0, "{s:?}");
        assert_eq!(b.weapon_paths().len(),1);
    }
    #[test]
    fn razlom_has_seven_segments_and_a_broad_low_head() {
        let b = razlom_t3();
        for z in [12usize,16,20,24,28,32,36] {
            assert_eq!(b.finish_at(30,14,z), FINISH_ENERGY, "segment {z}");
        }
        for z in [14usize,18,22,26,30,34] {
            assert!(!b.vol.get(33,15,z), "segments need a dark gap");
        }
        assert_eq!(b.turret_y, 9);
        assert_eq!((b.vol.nx,b.vol.ny,b.vol.nz),(48,24,48));
        assert!(!b.vol.get(35,15,24), "mouth must remain open");
    }
    #[test]
    fn greben_has_connected_modules_and_two_open_docks() {
        for modules in 1..=4 {
            let body = greben_storage_modules(modules);
            let status = body.status(&mut CoreBufs::new());
            assert!(status.alive && status.engine && status.sensor && status.detached == 0, "modules={modules}: {status:?}");
            assert_eq!(body.extent_mm().1, 83_200);
            for z in [20usize, 60] {
                assert!(!body.vol.get(0, 8, z), "dock is solid at z={z}");
                assert!(!body.vol.get(20, 12, z), "dock is blocked at z={z}");
                assert!(body.vol.get(20, 20, z), "dock roof is missing at z={z}");
            }
        }
    }
    #[test]
    fn skoba_keeps_an_open_load_bay_and_a_connected_compact_frame() {
        for compact in [false, true] {
            let body = skoba_drone(compact);
            let status = body.status(&mut CoreBufs::new());
            assert!(status.alive && status.engine && status.sensor && !status.weapon && status.detached == 0, "compact={compact}: {status:?}");
            assert_eq!((body.vol.nx, body.vol.ny, body.vol.nz), (128, 24, 62));
            assert!(!body.vol.get(64, 16, 31), "the load bay must stay open");
            assert!(!body.vol.get(0, 16, 31), "there is no front crossbar");
        }
        let open = skoba_drone(false);
        let closed = skoba_drone(true);
        assert!(open.vol.get(64, 16, 5) && open.vol.get(64, 16, 53));
        assert!(closed.vol.get(64, 16, 26) && closed.vol.get(64, 16, 32));
        assert!(!closed.vol.get(64, 16, 30) && !closed.vol.get(64, 16, 31));
        let loaded = skoba_with_cargo();
        let status = loaded.status(&mut CoreBufs::new());
        assert!(status.alive && status.detached == 0, "{status:?}");
        assert!(loaded.vol.get(70, 4, 31));
        assert!(!open.vol.get(64, 4, 31));
    }
    #[test]
    fn kil_factory_keeps_one_site_and_opens_the_gate_on_every_tier() {
        for tier in 1..=4 {
            let body = kil_factory(tier);
            let mut core = CoreBufs::new();
            let status = body.status(&mut core);
            assert!(status.alive && status.engine && status.sensor && status.detached == 0, "tier={tier}: {status:?}");
            assert_eq!((body.vol.nx, body.vol.nz), (80, 60));
            assert!(!body.vol.get(3, 2, 30), "tier={tier}: gate is solid");
            let back = match tier { 1 => 25, 2 => 31, 3 => 38, _ => 47 };
            let wall = if tier == 4 { 3 } else { 2 };
            assert!(body.vol.get(back - wall, 2, 30), "tier={tier}: rear wall is missing");
        }
    }
    #[test]
    fn battle_templates_begin_with_supported_root_voxels() {
        for (name, body) in [
            ("tank", tank()),
            ("crawler", crawler()),
            ("artillery", artillery()),
            ("turret T1", turret_t1()),
            ("turret T2", turret_t2()),
            ("artillery T3", artillery_turret_t3()),
            ("base", command_base()),
            ("strider", strider()),
            ("walker04", walker04()),
            ("razlom", razlom_t3()),
            ("greben", greben_storage()),
            ("spider", spider()),
        ] {
            assert_eq!(
                body.status(&mut CoreBufs::new()).detached,
                0,
                "{name} has unsupported starting pieces"
            );
        }
    }
    #[test]
    fn crawler_has_a_working_engine_and_a_destructible_core() {
        let mut unit = crawler();
        let status = unit.status(&mut CoreBufs::new());
        assert!(status.alive && status.engine && status.weapon);
        let (x, y, z) = unit.core;
        for dz in 0..unit.core_size as usize {
            for dy in 0..unit.core_size as usize {
                for dx in 0..unit.core_size as usize {
                    unit.remove(x as usize + dx, y as usize + dy, z as usize + dz);
                }
            }
        }
        assert!(!unit.status(&mut CoreBufs::new()).alive);
    }
    #[test]
    fn enlarged_lightning_tank_retains_ten_centimetre_cells_and_connected_weapon() {
        let unit = lightning_tank();
        assert_eq!((unit.vol.nx, unit.vol.ny, unit.vol.nz, unit.mm), (96, 60, 54, 100));
        let status = unit.status(&mut CoreBufs::new());
        assert!(status.alive && status.engine && status.weapon);
        assert_eq!(status.detached, 0);
        assert!(unit.weapon_paths().iter().any(|p| p.max[0] == 96));
    }
    #[test]
    fn crawler_drill_is_a_connected_destructible_subsystem() {
        let mut unit = crawler();
        assert!(unit.status(&mut CoreBufs::new()).weapon);
        let [x, y, z] = CRAWLER_DRILL_TIP;
        assert!(unit.vol.get(x, y, z) && unit.part_at(x, y, z) == Part::Weapon);
        let mut drill_cells = 0;
        for z in 0..unit.vol.nz as usize {
            for y in 0..unit.vol.ny as usize {
                for x in 0..unit.vol.nx as usize {
                    if unit.vol.get(x, y, z) && unit.part_at(x, y, z) == Part::Weapon {
                        unit.remove(x, y, z);
                        drill_cells += 1;
                    }
                }
            }
        }
        let status = unit.status(&mut CoreBufs::new());
        assert!(drill_cells > 0 && status.alive && status.engine && !status.weapon);
    }
    #[test]
    fn command_base_is_stationary_and_loses_when_its_core_is_destroyed() {
        let mut base = command_base();
        let status = base.status(&mut CoreBufs::new());
        assert!(status.alive && !status.engine && !status.weapon);
        assert_eq!(status.detached, 0, "base has unsupported starting pieces");
        let (x, y, z) = base.core;
        for dz in 0..base.core_size as usize {
            for dy in 0..base.core_size as usize {
                for dx in 0..base.core_size as usize {
                    base.remove(x as usize + dx, y as usize + dy, z as usize + dz);
                }
            }
        }
        assert!(!base.status(&mut CoreBufs::new()).alive);
    }
}

/// Шагоход: второе ванильное тело. 4.0 × 5.0 × 4.0 м при вокселе
/// 100 мм — том 40×50×40.
///
/// **Зачем второе тело.** Две стороны, отличающиеся только цветом, —
/// это не две армии, а одна перекрашенная: силуэт на экране важнее
/// оттенка, потому что на дистанции RTS оттенок съедает туман, а форму
/// нет. Шагоход выбран противоположным танку по обоим габаритам: танк
/// длинный и низкий, шагоход узкий и высокий, и различить их можно
/// даже по тени.
///
/// **У ноги есть суставы.** Бедро идёт от бедренного шара наружу
/// и вниз, колено — шар, голень возвращается внутрь к ступне.
/// Прежняя нога была столбом 3×3 и читалась как ножка табурета:
/// при вокселе 250 мм колено в неё не помещалось (ADR-219).
///
/// Форма условная ровно в той же мере, что у танка: настоящие тома
/// приходят `.vox`-моделями из данных (ADR-008).
pub fn strider() -> UnitBody {
    let mut u = UnitBody::new_scaled(40, 50, 40, 4, BODY_MM);
    for &hx in &[11i32, 29] {
        for &hz in &[11i32, 29] {
            let out = |v: i32, d: i32| if v < 20 { v - d } else { v + d };
            let (kx, kz) = (out(hx, 4), out(hz, 4));
            let (ax, az) = (out(hx, 2), out(hz, 2));
            ball(&mut u, (hx, 28, hz), 5, Part::Armour);
            mark_ball(&mut u, (hx, 28, hz), 5, FINISH_STEEL);
            let first = u.segments.len() as u8;
            let mut thigh = UnitBody::new_scaled(13, 8, 8, 1, BODY_MM);
            fill(&mut thigh, (0, 13), (0, 8), (0, 8), Part::Armour);
            mark(&mut thigh, (0, 2), (0, 8), (0, 8), FINISH_GUN);
            mark(&mut thigh, (11, 13), (0, 8), (0, 8), FINISH_GUN);
            fill_f(&mut thigh, (2, 11), (7, 9), (1, 7), Part::Armour, FINISH_PLATE);
            fill_f(&mut thigh, (3, 10), (4, 6), (7, 9), Part::Armour, FINISH_STEEL);
            fill_f(&mut thigh, (5, 10), (4, 5), (8, 10), Part::Armour, FINISH_GUN);
            u.push_segment(Attach::aimed((hx as i16, 27, hz as i16), (0, 4, 4), (kx - hx, 15 - 27, kz - hz)), thigh);
            let mut shin = UnitBody::new_scaled(11, 6, 6, 1, BODY_MM);
            fill(&mut shin, (0, 11), (0, 6), (0, 6), Part::Armour);
            mark(&mut shin, (0, 2), (0, 6), (0, 6), FINISH_GUN);
            mark(&mut shin, (9, 11), (0, 6), (0, 6), FINISH_RUBBER);
            fill_f(&mut shin, (2, 9), (5, 7), (0, 6), Part::Armour, FINISH_PLATE);
            fill_f(&mut shin, (3, 9), (3, 4), (6, 8), Part::Armour, FINISH_STEEL);
            u.push_segment(Attach::aimed((kx as i16, 15, kz as i16), (0, 3, 3), (ax - kx, 4 - 15, az - kz)), shin);
            let mut knee = UnitBody::new_scaled(9, 9, 9, 1, BODY_MM);
            ball(&mut knee, (4, 4, 4), 4, Part::Armour);
            mark_ball(&mut knee, (4, 4, 4), 4, FINISH_STEEL);
            fill_f(&mut knee, (6, 9), (2, 7), (2, 7), Part::Armour, FINISH_PLATE);
            u.push_segment(Attach::flat((kx as i16, 15, kz as i16), (4, 4, 4)), knee);
            let mut ankle = UnitBody::new_scaled(7, 7, 7, 1, BODY_MM);
            ball(&mut ankle, (3, 3, 3), 3, Part::Armour);
            mark_ball(&mut ankle, (3, 3, 3), 3, FINISH_STEEL);
            fill_f(&mut ankle, (5, 7), (2, 6), (2, 5), Part::Armour, FINISH_GUN);
            u.push_segment(Attach::flat((ax as i16, 4, az as i16), (3, 3, 3)), ankle);
            let mut foot = UnitBody::new_scaled(12, 5, 10, 1, BODY_MM);
            fill_f(&mut foot, (0, 11), (0, 1), (0, 10), Part::Armour, FINISH_RUBBER);
            fill(&mut foot, (0, 10), (1, 3), (1, 9), Part::Armour);
            fill_f(&mut foot, (3, 7), (3, 5), (3, 7), Part::Armour, FINISH_STEEL);
            for z in [(1, 4), (6, 9)] {
                fill_f(&mut foot, (9, 12), (1, 3), z, Part::Armour, FINISH_PLATE);
                fill_f(&mut foot, (11, 12), (1, 2), z, Part::Armour, FINISH_STEEL);
            }
            u.push_segment(Attach::flat((ax as i16, 4, az as i16), (4, 4, 4)), foot);
            let support = u8::from(hx < 20) * 2 + u8::from(hz < 20);
            u.push_leg(Leg {
                hip: (hx as i16, 27, hz as i16),
                knee: (kx as i16, 15, kz as i16),
                ankle: (ax as i16, 4, az as i16),
                thigh: first,
                shin: first + 1,
                knee_ball: Some(first + 2),
                ankle_ball: Some(first + 3),
                foot: Some(first + 4),
                support,
                phase_shift_pct: 0,
            });
            u.push_load_path(WeaponPath {
                min: [(hx - 3) as u8, 25, (hz - 3) as u8],
                max: [(hx + 4) as u8, 32, (hz + 4) as u8],
                axis: 1,
            });
        }
    }
    for y in 26..38 {
        let k = if y < 30 { (30 - y) as i32 } else if y < 34 { 0 } else { (y - 34) as i32 };
        disc(&mut u, (20, 20), 13 - k, (y, y + 1), Part::Armour);
    }
    mark(&mut u, (8, 33), (29, 30), (8, 33), FINISH_GUN);
    for &(z, side) in &[(8, -1), (32, 1)] {
        skirt(&mut u, 12, 16, 37, z, side, TURN / 16, 7, 2, false, Part::Armour, FINISH_PLATE);
    }
    fill(&mut u, (27, 32), (32, 37), (14, 26), Part::Armour);
    for z in [14, 24] {
        fill_f(&mut u, (30, 33), (34, 36), (z, z + 2), Part::Armour, FINISH_STEEL);
    }
    fill(&mut u, (9, 16), (28, 36), (14, 26), Part::Engine);
    for z in [15usize, 25] {
        cyl_f(&mut u, 0, (2, 10), (33, z as i32), 2, 0, Part::Engine, FINISH_GUN);
        fill_f(&mut u, (2, 4), (32, 34), (z - 1, z + 2), Part::Engine, FINISH_STEEL);
    }
    for z in (15..26).step_by(3) {
        fill_f(&mut u, (8, 10), (32, 35), (z, z + 1), Part::Engine, FINISH_STEEL);
    }
    ball(&mut u, (20, 38, 20), 6, Part::Armour);
    plated_turret(&mut u, (12, 29), (40, 47), (12, 28), (6, 3), TURN / 16, 7, 2, true, Part::Armour, FINISH_PLATE);
    fill(&mut u, (15, 26), (47, 48), (14, 26), Part::Armour);
    for z in [16, 24] {
        fill_f(&mut u, (24, 31), (42, 47), (z - 3, z + 3), Part::Weapon, FINISH_GUN);
        cyl_f(&mut u, 0, (28, 40), (44, z as i32), 2, 0, Part::Weapon, FINISH_GUN);
        cyl_f(&mut u, 0, (32, 35), (44, z as i32), 3, 0, Part::Weapon, FINISH_STEEL);
        cyl_f(&mut u, 0, (38, 40), (44, z as i32), 2, 0, Part::Weapon, FINISH_STEEL);
    }
    fill_f(&mut u, (22, 29), (47, 49), (17, 23), Part::Sensor, FINISH_GUN);
    fill_f(&mut u, (28, 30), (47, 49), (18, 22), Part::Sensor, FINISH_OPTIC);
    for z in [10, 28] {
        fill_f(&mut u, (15, 20), (44, 47), (z, z + 2), Part::Sensor, FINISH_GUN);
        fill_f(&mut u, (19, 21), (45, 47), (z, z + 2), Part::Sensor, FINISH_OPTIC);
    }
    fill(&mut u, (17, 23), (29, 35), (17, 23), Part::Core);
    let mut u = seat(u, "шагоход");
    u.turret_y = 38;
    for z in [16, 24] {
        u.push_weapon_path(WeaponPath { min: [24, 42, z - 2], max: [40, 47, z + 3], axis: 0 });
    }
    u
}

/// Heavy Walker 04 with four independently aimed arms.
pub fn walker04() -> UnitBody {
    fn at(p: (i32, i32, i32)) -> (i16, i16, i16) {
        (p.0 as i16, p.1 as i16, p.2 as i16)
    }
    let mut u = UnitBody::new_scaled(80, 52, 80, 4, BODY_MM);
    let a = Part::Armour;

    fill_f(&mut u, (35, 46), (15, 21), (32, 49), a, FINISH_IVORY);
    fill_f(&mut u, (35, 46), (21, 41), (34, 47), a, FINISH_IVORY);
    fill_f(&mut u, (36, 39), (24, 35), (36, 45), Part::Engine, FINISH_GUN);
    fill(&mut u, (38, 44), (23, 29), (37, 44), Part::Core);
    for y in [24, 29, 34] {
        mark(&mut u, (35, 36), (y, y + 2), (36, 45), FINISH_GUN);
    }
    for z in [34, 44] {
        mark(&mut u, (38, 43), (22, 38), (z, z + 2), FINISH_PLATE);
    }
    fill_f(&mut u, (46, 48), (24, 40), (36, 45), a, FINISH_GUN);
    fill_f(&mut u, (47, 49), (25, 39), (37, 44), a, FINISH_IVORY);
    fill_f(&mut u, (49, 50), (27, 37), (39, 42), a, FINISH_ENERGY);
    for y in [28, 32, 36] {
        mark(&mut u, (49, 50), (y, y + 1), (39, 42), FINISH_OPTIC);
    }
    mark(&mut u, (38, 43), (40, 41), (35, 40), FINISH_PLATE);
    mark(&mut u, (38, 43), (40, 41), (43, 47), FINISH_PLATE);

    disc(&mut u, (40, 40), 11, (41, 44), a);
    mark_cyl(&mut u, 1, (41, 44), (40, 40), 11, FINISH_GUN);
    disc(&mut u, (40, 40), 8, (44, 45), a);
    mark_cyl(&mut u, 1, (44, 45), (40, 40), 8, FINISH_IVORY);
    fill_f(&mut u, (37, 43), (45, 48), (37, 44), a, FINISH_GUN);
    fill_f(&mut u, (38, 44), (46, 51), (37, 44), a, FINISH_IVORY);
    fill_f(&mut u, (44, 45), (47, 49), (38, 43), Part::Sensor, FINISH_ENERGY);
    mark(&mut u, (39, 42), (50, 51), (38, 41), FINISH_PLATE);

    for sx in [-1i32, 1] {
        let hip = (40, 16, 40 + sx * 7);
        let knee = (34, 11, 40 + sx * 8);
        let ankle = (40, 4, 40 + sx * 8);
        ball(&mut u, hip, 5, a);
        mark_ball(&mut u, hip, 5, FINISH_GUN);
        let first = u.segments.len() as u8;
        let mut thigh = UnitBody::new_scaled(10, 10, 12, 1, BODY_MM);
        fill_f(&mut thigh, (0, 10), (0, 10), (0, 12), a, FINISH_IVORY);
        mark(&mut thigh, (0, 2), (0, 10), (0, 12), FINISH_GUN);
        mark(&mut thigh, (8, 10), (0, 10), (0, 12), FINISH_GUN);
        mark(&mut thigh, (2, 4), (9, 10), (2, 10), FINISH_PLATE);
        mark(&mut thigh, (7, 8), (1, 9), (1, 11), FINISH_GUN);
        u.push_segment(Attach::aimed(at(hip), (0, 5, 6), (knee.0 - hip.0, knee.1 - hip.1, knee.2 - hip.2)), thigh);

        let mut hub = UnitBody::new_scaled(11, 11, 11, 1, BODY_MM);
        ball(&mut hub, (5, 5, 5), 5, a);
        mark_ball(&mut hub, (5, 5, 5), 5, FINISH_GUN);
        fill_f(&mut hub, (8, 11), (2, 9), (2, 9), a, FINISH_IVORY);
        u.push_segment(Attach::flat(at(knee), (5, 5, 5)), hub);

        let mut shin = UnitBody::new_scaled(11, 8, 12, 1, BODY_MM);
        for z in [(0, 3), (9, 12)] {
            fill_f(&mut shin, (0, 11), (0, 8), z, a, FINISH_IVORY);
        }
        for x in [(0, 2), (9, 11)] {
            fill_f(&mut shin, x, (0, 8), (0, 12), a, FINISH_IVORY);
        }
        fill_f(&mut shin, (1, 7), (2, 6), (4, 8), a, FINISH_GUN);
        fill_f(&mut shin, (3, 7), (2, 6), (4, 8), a, FINISH_PLATE);
        fill_f(&mut shin, (5, 11), (3, 5), (5, 7), a, FINISH_STEEL);
        fill_f(&mut shin, (3, 5), (2, 6), (4, 8), a, FINISH_GUN);
        u.push_segment(Attach::aimed(at(knee), (0, 4, 6), (ankle.0 - knee.0, ankle.1 - knee.1, ankle.2 - knee.2)), shin);

        let mut ankle_hub = UnitBody::new_scaled(8, 8, 8, 1, BODY_MM);
        ball(&mut ankle_hub, (4, 4, 4), 4, a);
        mark_ball(&mut ankle_hub, (4, 4, 4), 4, FINISH_GUN);
        u.push_segment(Attach::flat(at(ankle), (4, 4, 4)), ankle_hub);
        let mut foot = UnitBody::new_scaled(18, 3, 13, 1, BODY_MM);
        fill_f(&mut foot, (0, 18), (0, 1), (0, 13), a, FINISH_RUBBER);
        fill_f(&mut foot, (1, 17), (1, 3), (1, 12), a, FINISH_IVORY);
        mark(&mut foot, (8, 11), (2, 3), (2, 11), FINISH_PLATE);
        for z in [(1, 4), (9, 12)] {
            mark(&mut foot, (15, 18), (1, 3), z, FINISH_GUN);
        }
        u.push_segment(Attach::flat(at(ankle), (7, 2, 6)), foot);
        u.push_leg(Leg {
            hip: at(hip),
            knee: at(knee),
            ankle: at(ankle),
            thigh: first,
            shin: first + 2,
            knee_ball: Some(first + 1),
            ankle_ball: Some(first + 3),
            foot: Some(first + 4),
            support: u8::from(sx > 0),
            phase_shift_pct: 0,
        });
        u.push_load_path(WeaponPath {
            min: [37, 13, (hip.2 - 3) as u8],
            max: [44, 20, (hip.2 + 4) as u8],
            axis: 1,
        });
    }

    for sy in [-1i32, 1] {
        for sx in [-1i32, 1] {
            let shoulder = (40 - sy * 9, 42, 40 + sx * 9);
            let elbow = (40 - sy * 22, 35, 40 + sx * 13);
            let wrist = (40 - sy * 33, 27, 40 + sx * 16);
            let muzzle = (40 - sy * 36, 25, 40 + sx * 17);
            ball(&mut u, shoulder, 3, a);
            mark_ball(&mut u, shoulder, 3, FINISH_STEEL);
            let mut upper = UnitBody::new_scaled(17, 4, 4, 1, BODY_MM);
            fill_f(&mut upper, (0, 17), (0, 4), (0, 4), a, FINISH_IVORY);
            mark(&mut upper, (0, 2), (0, 4), (0, 4), FINISH_GUN);
            mark(&mut upper, (14, 17), (0, 4), (0, 4), FINISH_GUN);
            mark(&mut upper, (3, 13), (3, 4), (1, 2), FINISH_PLATE);
            mark(&mut upper, (3, 13), (3, 4), (2, 3), FINISH_PLATE);
            for x in [4, 11] {
                mark(&mut upper, (x, x + 1), (3, 4), (1, 3), FINISH_STEEL);
            }
            u.push_segment(Attach { turret: true, ..Attach::aimed(at(shoulder), (0, 2, 2), (elbow.0 - shoulder.0, elbow.1 - shoulder.1, elbow.2 - shoulder.2)) }, upper);

            let mut elbow_hub = UnitBody::new_scaled(8, 8, 8, 1, BODY_MM);
            ball(&mut elbow_hub, (4, 4, 4), 3, a);
            mark_ball(&mut elbow_hub, (4, 4, 4), 3, FINISH_STEEL);
            fill_f(&mut elbow_hub, (6, 8), (2, 6), (2, 6), a, FINISH_IVORY);
            mark(&mut elbow_hub, (6, 8), (3, 5), (3, 5), FINISH_PLATE);
            u.push_segment(Attach { turret: true, ..Attach::flat(at(elbow), (4, 4, 4)) }, elbow_hub);

            let mut forearm = UnitBody::new_scaled(16, 4, 5, 1, BODY_MM);
            fill_f(&mut forearm, (0, 16), (0, 4), (0, 5), a, FINISH_IVORY);
            mark(&mut forearm, (0, 2), (0, 4), (0, 5), FINISH_GUN);
            mark(&mut forearm, (14, 16), (0, 4), (0, 5), FINISH_GUN);
            fill_f(&mut forearm, (4, 11), (3, 4), (1, 4), a, FINISH_ENERGY);
            mark(&mut forearm, (3, 12), (3, 4), (0, 1), FINISH_GUN);
            mark(&mut forearm, (3, 12), (3, 4), (4, 5), FINISH_GUN);
            mark(&mut forearm, (12, 13), (1, 3), (1, 4), FINISH_PLATE);
            for z in [1, 3] {
                mark(&mut forearm, (3, 4), (2, 3), (z, z + 1), FINISH_STEEL);
                mark(&mut forearm, (12, 13), (2, 3), (z, z + 1), FINISH_STEEL);
            }
            u.push_segment(Attach { turret: true, ..Attach::aimed(at(elbow), (0, 2, 2), (wrist.0 - elbow.0, wrist.1 - elbow.1, wrist.2 - elbow.2)) }, forearm);

            let mut laser = UnitBody::new_scaled(4, 4, 4, 1, BODY_MM);
            fill_f(&mut laser, (0, 4), (0, 4), (0, 4), Part::Weapon, FINISH_GUN);
            fill_f(&mut laser, (0, 2), (0, 4), (0, 4), Part::Weapon, FINISH_IVORY);
            mark(&mut laser, (0, 2), (2, 4), (1, 3), FINISH_PLATE);
            fill_f(&mut laser, (2, 3), (0, 4), (0, 4), Part::Weapon, FINISH_STEEL);
            fill_f(&mut laser, (3, 4), (1, 3), (1, 3), Part::Weapon, FINISH_ENERGY);
            u.push_segment(Attach { turret: true, ..Attach::aimed(at(wrist), (0, 2, 2), (muzzle.0 - wrist.0, muzzle.1 - wrist.1, muzzle.2 - wrist.2)) }, laser);

            for x in 40.min(shoulder.0).min(muzzle.0)..=40.max(shoulder.0).max(muzzle.0) {
                u.set(x as usize, 25, shoulder.2 as usize, Part::Weapon);
                u.set_finish(x as usize, 25, shoulder.2 as usize, FINISH_IVORY);
            }
            for y in 25..=shoulder.1 {
                u.set(shoulder.0 as usize, y as usize, shoulder.2 as usize, Part::Weapon);
                u.set_finish(shoulder.0 as usize, y as usize, shoulder.2 as usize, FINISH_IVORY);
            }
            for z in shoulder.2.min(muzzle.2)..=shoulder.2.max(muzzle.2) {
                u.set(muzzle.0 as usize, 25, z as usize, Part::Weapon);
                u.set_finish(muzzle.0 as usize, 25, z as usize, FINISH_IVORY);
            }
            u.push_weapon_path(WeaponPath {
                min: [muzzle.0 as u8, 22, (muzzle.2 - 3) as u8],
                max: [(muzzle.0 + 1) as u8, 28, (muzzle.2 + 4) as u8],
                axis: 0,
            });
        }
    }

    let mut u = seat(u, "Walker 04 · силовые рамы");
    u.mm = 500;
    for segment in &mut u.segments {
        segment.body.mm = 500;
    }
    u.turret_y = 41;
    u
}

/// Giant eight-legged assault platform, with a single axial laser and
/// independently damageable articulated legs. Coarser 200 mm armour keeps
/// its large silhouette affordable without inflating the voxel count.
pub fn spider() -> UnitBody {
    let mut u = UnitBody::new_scaled(88, 52, 80, 12, 200);
    for y in 25..40 {
        let inset = (y as i32 - 32).abs() / 2;
        fill_f(
            &mut u,
            ((16 + inset) as usize, (72 - inset) as usize),
            (y, y + 1),
            ((20 + inset) as usize, (60 - inset) as usize),
            Part::Armour,
            if y % 5 == 0 { FINISH_GUN } else { FINISH_PLATE },
        );
    }
    // Sacrificial side skirts and a layered crown put real armour voxels
    // between incoming fire and the engine/core. Leave the eight hip joints
    // exposed enough to remain individually damageable.
    for y in 27..39 {
        let inset = (y as i32 - 33).abs() / 3;
        for z in [(17 + inset, 23), (57, 63 - inset)] {
            fill_f(
                &mut u,
                (26, 62),
                (y, y + 1),
                (z.0 as usize, z.1 as usize),
                Part::Armour,
                if y % 4 == 0 {
                    FINISH_STEEL
                } else {
                    FINISH_PLATE
                },
            );
        }
    }
    fill_f(
        &mut u,
        (29, 60),
        (40, 43),
        (26, 54),
        Part::Armour,
        FINISH_PLATE,
    );
    // Layered glacis in front of the reactor, below the independently
    // traversing laser. These are sacrificial cells, not a health bonus.
    for y in 27..40 {
        let front = 81 - (y as i32 - 33).abs() / 2;
        fill_f(
            &mut u,
            (68, front as usize),
            (y, y + 1),
            (27, 53),
            Part::Armour,
            if y % 4 == 0 {
                FINISH_STEEL
            } else {
                FINISH_PLATE
            },
        );
    }
    for x in [30, 42, 54] {
        fill_f(
            &mut u,
            (x, x + 2),
            (43, 45),
            (28, 52),
            Part::Armour,
            FINISH_STEEL,
        );
    }
    for (row, hx) in [22i32, 34, 54, 66].into_iter().enumerate() {
        for hz in [24i32, 56] {
            let kx = hx + if hx < 44 { -7 } else { 7 };
            let kz = if hz < 40 { 9 } else { 71 };
            let ax = hx + if hx < 44 { -10 } else { 10 };
            let az = if hz < 40 { 5 } else { 75 };
            ball(&mut u, (hx, 31, hz), 5, Part::Armour);
            mark_ball(&mut u, (hx, 31, hz), 5, FINISH_STEEL);
            let first = u.segments.len() as u8;
            for (origin, end, length, width) in [
                ((hx, 31, hz), (kx, 18, kz), 22usize, 6usize),
                ((kx, 18, kz), (ax, 4, az), 16usize, 5usize),
            ] {
                let mut link = UnitBody::new_scaled(length as u8, width as u8, width as u8, 1, 200);
                fill_f(
                    &mut link,
                    (0, length),
                    (0, width),
                    (0, width),
                    Part::Armour,
                    FINISH_PLATE,
                );
                mark(&mut link, (0, 3), (0, width), (0, width), FINISH_GUN);
                mark(
                    &mut link,
                    (length - 3, length),
                    (0, width),
                    (0, width),
                    FINISH_STEEL,
                );
                mark(
                    &mut link,
                    (4, length - 3),
                    (width - 1, width),
                    (2, 3),
                    FINISH_STEEL,
                );
                // Recessed actuator bands and the long hydraulic channel are
                // part of the moving link, not separate animated meshes.
                for x in [5, length / 2, length - 5] {
                    mark(&mut link, (x, x + 1), (0, width), (0, width), FINISH_GUN);
                }
                mark(&mut link, (4, length - 3), (1, 2), (0, 1), FINISH_STEEL);
                u.push_segment(
                    Attach::aimed(
                        (origin.0 as i16, origin.1, origin.2 as i16),
                        (0, (width / 2) as u8, (width / 2) as u8),
                        (
                            end.0 - origin.0,
                            i32::from(end.1 - origin.1),
                            end.2 - origin.2,
                        ),
                    ),
                    link,
                );
            }
            for (p, radius) in [((kx, 18, kz), 4), ((ax, 4, az), 3)] {
                let edge = (radius * 2 + 1) as usize;
                let mut joint = UnitBody::new_scaled(edge as u8, edge as u8, edge as u8, 1, 200);
                ball(&mut joint, (radius, radius, radius), radius, Part::Armour);
                mark_ball(&mut joint, (radius, radius, radius), radius, FINISH_STEEL);
                u.push_segment(
                    Attach::flat(
                        (p.0 as i16, p.1, p.2 as i16),
                        (radius as u8, radius as u8, radius as u8),
                    ),
                    joint,
                );
            }
            let mut foot = UnitBody::new_scaled(12, 4, 8, 1, 200);
            fill_f(
                &mut foot,
                (0, 12),
                (0, 2),
                (0, 8),
                Part::Armour,
                FINISH_RUBBER,
            );
            fill_f(
                &mut foot,
                (2, 10),
                (2, 4),
                (1, 7),
                Part::Armour,
                FINISH_PLATE,
            );
            for x in [2, 5, 8] {
                mark(&mut foot, (x, x + 1), (3, 4), (1, 7), FINISH_STEEL);
            }
            u.push_segment(Attach::flat((ax as i16, 4, az as i16), (6, 4, 4)), foot);
            // Two legs per suspension quadrant, but not a synchronized pair:
            // the second leg starts 38% of a cycle later. With a 25% swing,
            // one foot in each quadrant stays planted at every phase.
            let support = u8::from(hx < 44) * 2 + u8::from(hz < 40);
            u.push_leg(Leg {
                hip: (hx as i16, 31, hz as i16),
                knee: (kx as i16, 18, kz as i16),
                ankle: (ax as i16, 4, az as i16),
                thigh: first,
                shin: first + 1,
                knee_ball: Some(first + 2),
                ankle_ball: Some(first + 3),
                foot: Some(first + 4),
                support,
                phase_shift_pct: if hx == 34 || hx == 66 { 38 } else { 0 },
            });
            u.push_load_path(WeaponPath {
                min: [(hx - 3) as u8, 28, (hz - 3) as u8],
                max: [(hx + 4) as u8, 36, (hz + 4) as u8],
                axis: 1,
            });
            // Raised paired radiator fins separate the leg roots from the hull.
            fill_f(
                &mut u,
                ((hx - 3) as usize, (hx + 4) as usize),
                (38, 41),
                (29 + row % 2, 32 + row % 2),
                Part::Armour,
                FINISH_STEEL,
            );
        }
    }
    fill_f(
        &mut u,
        (18, 30),
        (28, 37),
        (29, 51),
        Part::Engine,
        FINISH_GUN,
    );
    for x in (18..31).step_by(3) {
        fill_f(
            &mut u,
            (x, x + 1),
            (39, 43),
            (29, 51),
            Part::Armour,
            FINISH_STEEL,
        );
    }
    ball(&mut u, (48, 40, 40), 9, Part::Armour);
    plated_turret(
        &mut u,
        (34, 65),
        (41, 49),
        (28, 52),
        (6, 3),
        TURN / 16,
        7,
        2,
        true,
        Part::Armour,
        FINISH_PLATE,
    );
    cyl_f(
        &mut u,
        0,
        (58, 88),
        (45, 40),
        5,
        0,
        Part::Weapon,
        FINISH_GUN,
    );
    for x in [62, 68, 74, 82] {
        cyl_f(
            &mut u,
            0,
            (x, x + 2),
            (45, 40),
            6,
            0,
            Part::Weapon,
            FINISH_STEEL,
        );
    }
    fill_f(
        &mut u,
        (86, 88),
        (43, 48),
        (38, 43),
        Part::Weapon,
        FINISH_OPTIC,
    );
    fill_f(
        &mut u,
        (54, 61),
        (49, 51),
        (33, 47),
        Part::Sensor,
        FINISH_GUN,
    );
    fill_f(
        &mut u,
        (60, 62),
        (49, 51),
        (35, 45),
        Part::Sensor,
        FINISH_OPTIC,
    );
    // Raised armour cassettes break up the otherwise broad side skirts.
    // Each remains joined to the main hull, and leaves the hips uncovered.
    for x in [28, 38, 48, 58] {
        for z in [16, 62] {
            fill_f(&mut u, (x, x + 4), (30, 37), (z, z + 2), Part::Armour, FINISH_STEEL);
            fill_f(&mut u, (x + 1, x + 3), (31, 36), (z, z + 2), Part::Armour, FINISH_PLATE);
            mark(&mut u, (x + 1, x + 3), (33, 34), (z, z + 2), FINISH_GUN);
        }
    }
    // Paired sight housings flank the main laser; dark surrounds make the
    // small optical windows legible without changing the weapon path.
    for z in [30, 46] {
        fill_f(&mut u, (61, 67), (44, 49), (z, z + 4), Part::Armour, FINISH_GUN);
        fill_f(&mut u, (66, 68), (45, 48), (z + 1, z + 3), Part::Sensor, FINISH_OPTIC);
    }
    // Broad reactor stays below the laser turntable and inside the armour.
    fill(&mut u, (38, 51), (27, 40), (33, 47), Part::Core);
    // No sealed air behind the laser-head shoulder.
    pack(&mut u, (60, 62), (40, 41), (43, 45));
    let mut u = seat(u, "паук");
    u.turret_y = 40;
    let laser_path = WeaponPath {
        min: [56, 42, 37],
        max: [88, 49, 44],
        axis: 0,
    };
    // The breech cradle is part of the laser mechanism. Its two solid
    // support slices used to retain the exterior armour role, making this
    // essential neck much easier to fracture than the reinforced barrel.
    // Reclassify existing cells only: shape, finish and matter stay intact.
    for x in laser_path.min[0]..58 {
        for y in laser_path.min[1]..laser_path.max[1] {
            for z in laser_path.min[2]..laser_path.max[2] {
                let [x, y, z] = [x, y, z].map(usize::from);
                if u.vol.get(x, y, z) && u.part_at(x, y, z) == Part::Armour {
                    u.set(x, y, z, Part::Weapon);
                }
            }
        }
    }
    u.push_weapon_path(laser_path);
    u
}

/// Широкое двуногое тело для проверок опор (ADR-288): плита корпуса на
/// двух ногах по бортам с длинными ступнями, чтобы каждая нога несла и
/// пятку, и носок своей стороны. Без двух ног оно не стоит: смятая нога
/// роняет корпус на свою сторону. Стенд, не боевая машина.
pub fn biped() -> UnitBody {
    let mut u = UnitBody::new_scaled(40, 44, 40, 4, BODY_MM);
    for &hz in &[6i32, 34] {
        let hx = 20i32;
        let out = |v: i32, d: i32| if v < 20 { v - d } else { v + d };
        let (kx, kz) = (hx, out(hz, 4));
        let (ax, az) = (hx, out(hz, 2));
        ball(&mut u, (hx, 28, hz), 5, Part::Armour);
        mark_ball(&mut u, (hx, 28, hz), 5, FINISH_STEEL);
        let first = u.segments.len() as u8;
        let mut thigh = UnitBody::new_scaled(13, 8, 8, 1, BODY_MM);
        fill(&mut thigh, (0, 13), (0, 8), (0, 8), Part::Armour);
        u.push_segment(
            Attach::aimed(
                (hx as i16, 27, hz as i16),
                (0, 4, 4),
                (kx - hx, 15 - 27, kz - hz),
            ),
            thigh,
        );
        let mut shin = UnitBody::new_scaled(11, 6, 6, 1, BODY_MM);
        fill(&mut shin, (0, 11), (0, 6), (0, 6), Part::Armour);
        u.push_segment(
            Attach::aimed(
                (kx as i16, 15, kz as i16),
                (0, 3, 3),
                (ax - kx, 4 - 15, az - kz),
            ),
            shin,
        );
        let mut knee = UnitBody::new_scaled(9, 9, 9, 1, BODY_MM);
        ball(&mut knee, (4, 4, 4), 4, Part::Armour);
        u.push_segment(Attach::flat((kx as i16, 15, kz as i16), (4, 4, 4)), knee);
        let mut ankle = UnitBody::new_scaled(7, 7, 7, 1, BODY_MM);
        ball(&mut ankle, (3, 3, 3), 3, Part::Armour);
        u.push_segment(Attach::flat((ax as i16, 4, az as i16), (3, 3, 3)), ankle);
        // A long foot: heel and toe both stand.
        let mut foot = UnitBody::new_scaled(25, 3, 9, 1, BODY_MM);
        fill(&mut foot, (0, 25), (0, 3), (0, 9), Part::Armour);
        u.push_segment(Attach::flat((ax as i16, 4, az as i16), (12, 4, 4)), foot);
        // Both hips sit on the hull's midline: the side names the support.
        let support = u8::from(hz < 20);
        u.push_leg(Leg {
            hip: (hx as i16, 27, hz as i16),
            knee: (kx as i16, 15, kz as i16),
            ankle: (ax as i16, 4, az as i16),
            thigh: first,
            shin: first + 1,
            knee_ball: Some(first + 2),
            ankle_ball: Some(first + 3),
            foot: Some(first + 4),
            support,
            phase_shift_pct: 0,
        });
        u.push_load_path(WeaponPath {
            min: [(hx - 3) as u8, 25, (hz - 3) as u8],
            max: [(hx + 4) as u8, 32, (hz + 4) as u8],
            axis: 1,
        });
    }
    // The plate of a hull between the hips, engine aft, core in the middle.
    fill(&mut u, (6, 34), (26, 38), (4, 36), Part::Armour);
    fill(&mut u, (10, 18), (28, 36), (14, 26), Part::Engine);
    fill(&mut u, (18, 24), (29, 35), (17, 23), Part::Core);
    seat(u, "двуногий стенд")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пересчёт ролей по всему тому. Габарит берётся у тела, а не
    /// пишется числом: тест обязан переживать смену сетки, иначе он
    /// сторожит не роли, а свои литералы.
    fn roles(b: &UnitBody) -> [u32; 5] {
        let mut seen = [0u32; 5];
        for z in 0..b.vol.nz as usize {
            for y in 0..b.vol.ny as usize {
                for x in 0..b.vol.nx as usize {
                    if b.vol.get(x, y, z) {
                        seen[b.part_at(x, y, z) as usize] += 1;
                    }
                }
            }
        }
        seen
    }

    /// Снять всё, что помечено ролью `p`.
    fn strip_part(b: &mut UnitBody, p: Part) -> u32 {
        let mut n = 0;
        for z in 0..b.vol.nz as usize {
            for y in 0..b.vol.ny as usize {
                for x in 0..b.vol.nx as usize {
                    if b.vol.get(x, y, z) && b.part_at(x, y, z) == p {
                        n += u32::from(b.remove(x, y, z));
                    }
                }
            }
        }
        n
    }

    #[test]
    fn tank_has_every_part_and_the_right_size() {
        let t = tank();
        // 6×3×3 м при вокселе 0.1 м — том 60×30×30 (ADR-094, ADR-219).
        assert_eq!((t.vol.nx, t.vol.ny, t.vol.nz), (60, 30, 30));
        assert_eq!(
            t.extent_mm(),
            (6_000, 3_000, 3_000),
            "танк перестал быть шестиметровым"
        );
        let seen = roles(&t);
        for p in [
            Part::Armour,
            Part::Core,
            Part::Engine,
            Part::Weapon,
            Part::Sensor,
        ] {
            assert!(seen[p as usize] > 0, "{} отсутствует", p.name());
        }
        assert!(t.solid_count() > 10_000, "танк подозрительно пуст");
    }

    #[test]
    fn a_chipped_barrel_works_but_a_severed_breech_or_muzzle_does_not() {
        let original = tank();
        let mut core = CoreBufs::new();
        assert!(original.status(&mut core).weapon);
        let mut chipped = original.clone();
        assert!(chipped.remove(54, 22, 14));
        assert!(chipped.status(&mut core).weapon);
        for x in [35, 48, 54, 59] {
            let mut damaged = original.clone();
            for z in 0..damaged.vol.nz as usize {
                for y in 21..27 {
                    damaged.remove(x, y, z);
                }
            }
            let status = damaged.status(&mut core);
            assert!(status.alive && status.engine, "cut {x}: {status:?}");
            assert!(damaged.subsystem_count(Part::Weapon) > 0);
            assert!(!status.weapon, "cut {x}: {status:?}");
        }
        assert!(
            original.status(&mut core).weapon,
            "damage mutated shared metadata"
        );
    }

    #[test]
    fn four_walker_barrels_are_independent_and_resampling_preserves_the_rule() {
        let mut body = walker04();
        let mut core = CoreBufs::new();
        assert!(body.status(&mut core).weapon);
        let paths = body.weapon_paths().to_vec();
        assert_eq!(paths.len(), 4);
        assert!(paths.iter().all(|path| body.barrel_connected(path, core.reached_cols())));
        for (barrel, path) in paths.iter().enumerate() {
            let muzzle = if barrel < 2 { usize::from(path.max[0] - 1) } else { usize::from(path.min[0]) };
            for y in path.min[1]..path.max[1] {
                for z in path.min[2]..path.max[2] {
                    body.remove(muzzle, y as usize, z as usize);
                }
            }
            assert_eq!(body.status(&mut core).weapon, barrel + 1 < paths.len());
        }
        for mm in [50, 100] {
            let mut t = tank().resampled(mm).unwrap();
            assert!(t.status(&mut core).weapon, "intact {mm}mm");
            let p = t.weapon_paths()[0];
            let x = usize::from(p.max[0] - 1);
            for z in 0..t.vol.nz as usize {
                for y in 0..t.vol.ny as usize {
                    t.remove(x, y, z);
                }
            }
            assert!(!t.status(&mut core).weapon, "severed {mm}mm");
        }
        // At 200 mm centre sampling removes the bridge to the muzzle ring.
        // Metadata must not make that disconnected geometry operational.
        let coarse = tank().resampled(200).unwrap();
        let status = coarse.status(&mut core);
        assert!(status.alive && status.detached > 0);
        assert!(!status.weapon);
    }

    #[test]
    fn functional_paths_support_all_axes_and_do_not_accept_detached_slices() {
        for axis in 0..3 {
            let mut b = UnitBody::new(6, 6, 6, 1);
            fill(&mut b, (0, 6), (0, 6), (0, 6), Part::Weapon);
            b.set(0, 0, 0, Part::Core);
            assert!(b.seat_core());
            b.push_weapon_path(WeaponPath {
                min: [0; 3],
                max: [6; 3],
                axis,
            });
            let mut core = CoreBufs::new();
            assert!(b.status(&mut core).weapon);
            for a in 0..6 {
                for c in 0..6 {
                    let mut p = [0; 3];
                    p[usize::from(axis)] = 2;
                    p[(usize::from(axis) + 1) % 3] = a;
                    p[(usize::from(axis) + 2) % 3] = c;
                    b.remove(p[0], p[1], p[2]);
                }
            }
            let status = b.status(&mut core);
            assert!(status.alive && status.detached > 0);
            assert!(!status.weapon);
        }
    }

    /// Шагоход — второе ванильное тело, и требования к нему те же:
    /// все роли на месте, размер по канону, ядро село.
    #[test]
    fn strider_has_every_part_and_the_right_size() {
        let s = walker04();
        // 40×26×40 м при вокселе 0,5 м — вдвое меньше прежнего корпуса.
        assert_eq!((s.vol.nx, s.vol.ny, s.vol.nz), (80, 52, 80));
        assert_eq!(
            s.extent_mm(),
            (40_000, 26_000, 40_000),
            "шагоход сменил габарит"
        );
        let seen = roles(&s);
        for p in [
            Part::Armour,
            Part::Core,
            Part::Engine,
            Part::Weapon,
            Part::Sensor,
        ] {
            assert!(seen[p as usize] > 0, "{} отсутствует", p.name());
        }
        assert!(s.solid_count() > 4_000, "шагоход подозрительно пуст");
    }

    /// **Целый шагоход работает весь.** Ноги, корпус и голова обязаны
    /// быть связаны с ядром: несвязанная нога — это подсистема,
    /// которой нет, и заметно это только отсюда.
    #[test]
    fn an_intact_strider_works() {
        let s = walker04();
        let mut b = CoreBufs::new();
        let st = s.status(&mut b);
        assert!(st.alive && st.engine && st.weapon && st.sensor, "{st:?}");
        assert_eq!(st.detached, 0, "часть тела не связана с ядром");
    }

    /// **Два тела не спутать.** Разные габариты — это то, ради чего
    /// второе тело и заводилось.
    #[test]
    fn the_two_bodies_do_not_look_alike() {
        let (t, s) = (tank(), walker04());
        assert_ne!(
            (t.vol.nx, t.vol.ny, t.vol.nz),
            (s.vol.nx, s.vol.ny, s.vol.nz),
            "силуэты совпадают — это одно тело в двух цветах"
        );
        assert!(t.vol.nx > t.vol.ny, "танк не длинный");
        assert!(s.vol.ny >= 50 && s.vol.nx >= 70, "шагоход потерял высоту или размах рук");
    }

    /// **Линия башни лежит внутри тома и отрезает непустую часть.**
    /// Линия за габаритом даёт пустую башню, линия в нуле — тело,
    /// которое поворачивается целиком; и то и другое молча.
    #[test]
    fn the_turret_line_cuts_something_off() {
        for b in [tank(), walker04()] {
            let ty = b.turret_y as usize;
            assert!(
                ty > 0 && ty < b.vol.ny as usize,
                "линия башни {ty} вне тома"
            );
            let mut above = 0u32;
            let mut below = 0u32;
            for z in 0..b.vol.nz as usize {
                for y in 0..b.vol.ny as usize {
                    for x in 0..b.vol.nx as usize {
                        if b.vol.get(x, y, z) {
                            if y >= ty {
                                above += 1;
                            } else {
                                below += 1;
                            }
                        }
                    }
                }
            }
            assert!(above > 0 && below > 0, "по одну сторону линии пусто");
            assert_eq!(above + below, b.solid_count(), "линия потеряла воксели");
        }
    }

    /// Целый танк — все подсистемы живы. Иначе всё остальное
    /// бессмысленно.
    #[test]
    fn an_intact_tank_works() {
        let t = tank();
        let mut b = CoreBufs::new();
        let s = t.status(&mut b);
        assert!(s.alive && s.engine && s.weapon && s.sensor, "{s:?}");
        assert_eq!(s.detached, 0);
    }

    /// **Отстрел подсистемы отключает способность, не убивая юнита.**
    /// Это то, ради чего выбраны помеченные подсистемы (ADR-095).
    #[test]
    fn shooting_out_the_engine_stops_the_tank_but_does_not_kill_it() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        assert!(strip_part(&mut t, Part::Engine) > 0, "двигателя не было");
        let s = t.status(&mut b);
        assert!(s.alive, "танк умер от потери двигателя");
        assert!(!s.engine, "двигатель жив после отстрела");
        assert!(s.weapon && s.sensor, "пострадали чужие подсистемы: {s:?}");
    }

    /// **Подсистема, отрезанная от ядра, не работает, даже если цела.**
    /// Способность считается по связности (ADR-095, пункт 4).
    #[test]
    fn a_subsystem_severed_from_the_core_stops_working() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        assert!(t.status(&mut b).engine);
        // Срезаем корпус между двигателем и ядром: сам двигатель цел.
        // Двигатель у танка в корме, ядро под башней, и поперечный
        // пропил между ними отрезает первый от второго.
        for z in 0..t.vol.nz as usize {
            for y in 0..t.vol.ny as usize {
                for x in 22..27 {
                    t.remove(x, y, z);
                }
            }
        }
        let engine_left = roles(&t)[Part::Engine as usize];
        assert!(engine_left > 0, "двигатель случайно снесли — тест ни о чём");
        let s = t.status(&mut b);
        assert!(s.alive, "ядро должно быть цело");
        assert!(!s.engine, "отрезанный двигатель работает");
        assert!(s.detached > 0, "отрезанное не посчитано");
    }

    /// Пробитие к ядру убивает.
    #[test]
    fn destroying_the_core_kills() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        assert!(strip_part(&mut t, Part::Core) > 0, "ядра не было");
        let s = t.status(&mut b);
        assert!(!s.alive, "танк жив без ядра");
    }

    /// Урон пространственный: попадание в нос не трогает двигатель
    /// в корме. Броня работает как геометрия.
    #[test]
    fn damage_is_positional() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        let nose = t.vol.nx as usize - 10;
        for z in 0..t.vol.nz as usize {
            for y in 0..16 {
                for x in nose..t.vol.nx as usize {
                    t.remove(x, y, z);
                }
            }
        }
        let s = t.status(&mut b);
        assert!(s.alive && s.engine, "нос снесли, а двигатель встал: {s:?}");
    }
}

#[cfg(test)]
mod core_seat_tests {
    use super::*;

    /// Затравка обязана лежать внутри разметки ядра, иначе связность
    /// считается не от ядра.
    #[test]
    fn the_seed_cube_lies_inside_the_core_marking() {
        let t = tank();
        let cs = t.core_size as usize;
        let (cx, cy, cz) = (t.core.0 as usize, t.core.1 as usize, t.core.2 as usize);
        for dz in 0..cs {
            for dy in 0..cs {
                for dx in 0..cs {
                    assert_eq!(
                        t.part_at(cx + dx, cy + dy, cz + dz),
                        Part::Core,
                        "затравка вышла из ядра"
                    );
                }
            }
        }
    }

    /// Том без ядра обязан быть отвергнут, а не собран.
    #[test]
    fn a_body_without_a_core_is_refused() {
        let mut u = UnitBody::new(4, 4, 4, 2);
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    u.set(x, y, z, Part::Armour);
                }
            }
        }
        assert!(!u.seat_core());
    }

    /// Ядро меньше затравки — тоже отказ: куб не влезает в разметку.
    #[test]
    fn a_core_smaller_than_the_seed_is_refused() {
        let mut u = UnitBody::new(4, 4, 4, 2);
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    u.set(x, y, z, Part::Armour);
                }
            }
        }
        u.set(1, 1, 1, Part::Core);
        assert!(!u.seat_core());
    }
}

#[cfg(test)]
mod wreck_tests {
    use super::*;

    /// Разобрать обломок целиком в заданном порядке столбов.
    fn strip(w: &mut Wreck, back_to_front: bool) -> u64 {
        let (nx, ny, nz) = (w.vol.nx as usize, w.vol.ny as usize, w.vol.nz as usize);
        let mut got = 0u64;
        for i in 0..nz {
            let z = if back_to_front { nz - 1 - i } else { i };
            for j in 0..ny {
                let y = if back_to_front { ny - 1 - j } else { j };
                for k in 0..nx {
                    let x = if back_to_front { nx - 1 - k } else { k };
                    got += w.reclaim(x, y, z);
                }
            }
        }
        got
    }

    /// **Материя машины вся в обломке, до кубического миллиметра.**
    ///
    /// Это главный инвариант проекта в самом уязвимом его месте:
    /// форма перекладывается из сетки тела в мировую, и наивный
    /// пересчёт «вокселей стало вчетверо меньше» потерял бы семь
    /// восьмых. Материя переживает смену сетки потому, что она
    /// количество, а не кубики (ADR-022).
    ///
    /// Форма при этом обязана **перестать быть машиной**: обломок —
    /// куча на её следе. Без этой половины проверка прошла бы и на
    /// `wreck`, возвращающем тело как есть.
    #[test]
    fn a_wreck_holds_everything_the_machine_was_made_of() {
        let t = tank();
        let w = t.wreck(0);
        assert_eq!(
            w.matter_mm3(),
            t.volume_mm3(),
            "материя не сошлась при переходе в мировую сетку"
        );
        assert!(
            w.extent_mm().1 < t.extent_mm().1,
            "обломок ростом с машину ({} мм из {}) — это не куча",
            w.extent_mm().1,
            t.extent_mm().1
        );
    }

    /// **Снятая попаданиями броня уходит в обломок, а не в никуда.**
    ///
    /// Канон: материя из контейнера `damage` при смерти переливается
    /// в обломок, промежуточного «нигде» не существует
    /// (`docs/05-units.md`). Добивание выхода реклейма не снижает.
    #[test]
    fn damage_pours_into_the_wreck() {
        let mut t = tank();
        let whole = t.volume_mm3();
        // Снести половину длины и сложить снятое в контейнер damage.
        let mut damage = 0u64;
        let mm = t.mm as u64;
        let half = t.vol.nx as usize / 2;
        for z in 0..t.vol.nz as usize {
            for y in 0..t.vol.ny as usize {
                for x in 0..half {
                    if t.remove(x, y, z) {
                        damage += mm * mm * mm;
                    }
                }
            }
        }
        assert!(damage > 0, "ничего не снялось");
        assert_eq!(
            t.wreck(damage).matter_mm3(),
            whole,
            "добивание снизило выход — это скрытый сток"
        );
    }

    /// **Реклейм возвращает содержимое целиком и при любом порядке.**
    ///
    /// Выдаётся доля от оставшегося, а не объём воксела, поэтому
    /// деление не теряет ничего: последний воксель выносит остаток.
    /// Порядок не влияет — иначе реклейм был бы источником десинка
    /// в самой обыденной операции игры.
    #[test]
    fn reclaiming_a_wreck_returns_every_last_bit() {
        for order in [false, true] {
            let mut w = tank().wreck(1_234_567);
            let had = w.matter_mm3();
            let got = strip(&mut w, order);
            assert_eq!(got, had, "реклейм потерял материю (порядок {order})");
            assert_eq!(w.matter_mm3(), 0, "в разобранном обломке осталась материя");
            assert_eq!(w.solid_count(), 0, "в разобранном обломке остались воксели");
        }
        // Снять то, чего нет, — ноль, а не порция: иначе повторный
        // приказ двух краулеров создавал бы материю (ADR-031).
        //
        // Ячейка ищется, а не берётся углом тома: угол следа пуст
        // у любой машины со скруглённым корпусом, и на мелкой сетке
        // это уже так.
        let mut w = tank().wreck(0);
        let (nx, ny, nz) = (w.vol.nx as usize, w.vol.ny as usize, w.vol.nz as usize);
        let mut solid = None;
        'find: for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    if w.vol.get(x, y, z) {
                        solid = Some((x, y, z));
                        break 'find;
                    }
                }
            }
        }
        let (x, y, z) = solid.expect("обломок пуст");
        assert!(w.reclaim(x, y, z) > 0, "занятая ячейка не выдала материи");
        assert_eq!(w.reclaim(x, y, z), 0, "пустая ячейка выдала материю");
    }

    /// **Обломок — куча, а не машина.**
    ///
    /// След остаётся машины, высота падает до той, что даёт материя.
    /// Полая машина оседает в разы, сплошная почти не оседает — ей
    /// нечему оседать, и это правда, а не изъян модели. И форма
    /// обязана вмещать хотя бы собственную материю тела: округление
    /// остатка вверх существует ровно ради этого.
    #[test]
    fn a_wreck_is_a_pile_not_a_machine() {
        // Шагоход полый: корпус висит между ног. Первая редакция
        // осаживала каждый столб отдельно и давала ему 6.0 м из 6.0 —
        // куча ростом с машину не куча.
        let s = walker04();
        assert!(
            s.wreck(0).extent_mm().1 * 2 <= s.extent_mm().1,
            "полая машина не осела и вдвое: {} из {}",
            s.wreck(0).extent_mm().1,
            s.extent_mm().1
        );
        for b in [tank(), walker04()] {
            let w = b.wreck(0);
            let (bx, by, bz) = b.extent_mm();
            let (wx, wy, wz) = w.extent_mm();
            // Куча не крупнее машины ни по одной оси. Допуск
            // в один воксель обломка — смена сетки: тело нечётной
            // длины в его клетку целиком не ложится.
            let slack = w.mm as u32;
            assert!(
                wx <= bx + slack && wy <= by + slack && wz <= bz + slack,
                "обломок крупнее машины: {wx}x{wy}x{wz} против {bx}x{by}x{bz}"
            );
            assert!(
                w.capacity_mm3() >= b.volume_mm3(),
                "форме обломка нечем держать собственную материю тела"
            );
        }
    }
}

#[cfg(test)]
mod spider_tests {
    use super::*;
    #[test]
    fn fixed_artillery_t2_is_heavy_and_has_a_connected_protected_weapon() {
        let b = artillery_turret_t2();
        let t = tank();
        assert_eq!(b.extent_mm().0, t.extent_mm().0 * 3 / 2);
        assert!(b.volume_mm3() > t.volume_mm3() * 3);
        let st = b.status(&mut CoreBufs::new());
        assert!(st.alive && st.weapon && st.sensor, "{st:?}");
        assert!(!st.engine);
        assert_eq!(b.weapon_paths().len(), 1);
    }
    #[test]
    fn artillery_t2_has_connected_vitals_a_wider_bore_and_more_armour() {
        let t1 = artillery();
        let t2 = artillery_t2();
        assert_eq!(
            t2.extent_mm(),
            (
                t1.extent_mm().0 * 3 / 2,
                t1.extent_mm().1 * 3 / 2,
                t1.extent_mm().2 * 3 / 2
            )
        );
        assert!(t2.volume_mm3() > t1.volume_mm3());
        assert_eq!(t2.weapon_paths.len(), 1);
        assert_eq!(t2.weapon_paths[0].min[1] + t2.weapon_paths[0].max[1], 52);
        assert_eq!(t2.part_at(48, 28, 11), Part::Weapon);
        assert_eq!(t2.part_at(20, 19, 3), Part::Armour);
        let s = t2.status(&mut CoreBufs::new());
        assert!(s.alive && s.engine && s.weapon, "{s:?}");
    }
    #[test]
    fn spider_has_eight_articulated_legs_and_one_connected_heavy_weapon() {
        let body = spider();
        assert_eq!(body.legs.len(), 8);
        let support_offsets = [0, 50, 25, 75];
        let phases: std::collections::BTreeSet<_> = body
            .legs
            .iter()
            .map(|leg| (support_offsets[leg.support as usize] + leg.phase_shift_pct as i32) % 100)
            .collect();
        assert_eq!(phases.len(), 8, "every leg needs its own step phase");
        for (a, b) in [(0, 2), (1, 3), (4, 6), (5, 7)] {
            assert_eq!(body.legs[a].support, body.legs[b].support);
            assert_eq!(body.legs[a].phase_shift_pct, 0);
            assert_eq!(body.legs[b].phase_shift_pct, 38);
        }
        assert_eq!(body.weapon_paths.len(), 1);
        assert!(body.segments.len() >= 40);
        assert!(body.volume_mm3() > tank().volume_mm3() * 4);
        let status = body.status(&mut CoreBufs::new());
        assert!(status.alive && status.engine && status.weapon, "{status:?}");
    }

    #[test]
    fn spider_skirts_and_crown_cover_its_vital_compartments() {
        let body = spider();
        for z in [19, 21, 59, 61] {
            assert_eq!(body.part_at(44, 33, z), Part::Armour);
        }
        assert_eq!(body.part_at(44, 41, 40), Part::Armour);
        assert_eq!(body.part_at(42, 44, 40), Part::Armour);
        assert_eq!(body.part_at(44, 33, 40), Part::Core);
        let status = body.status(&mut CoreBufs::new());
        assert!(status.alive && status.engine && status.weapon);
    }
}
