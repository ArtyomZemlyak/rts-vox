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
    link_pivot, mark, mark_ball, mark_cyl, mount, plated_turret, ring, rod, running_gear, seat,
    skirt, slab,
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
pub fn artillery() -> UnitBody {
    let mut u = UnitBody::new_scaled(60, 32, 30, 4, BODY_MM);
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

    // A continuous breech and rising barrel let connectivity damage disable
    // this specific gun. The thick muzzle brake makes its role legible.
    fill(&mut u, (35, 46), (20, 27), (12, 18), Part::Weapon);
    for (x, y) in [(45, 23), (50, 24), (55, 25)] {
        fill_f(
            &mut u,
            (x, (x + 6).min(60)),
            (y, y + 3),
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

    let mut u = seat(u, "артиллерия");
    u.turret_y = 16;
    u.push_weapon_path(WeaponPath {
        min: [35, 20, 12],
        max: [60, 29, 18],
        axis: 0,
    });
    u
}

/// T2 tracked assault artillery. Thick cheek plates protect the breech;
/// the broad, short canister bore and ammunition pods distinguish its tier.
pub fn artillery_t2() -> UnitBody {
    let mut u = artillery();
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
    fn battle_templates_begin_with_supported_root_voxels() {
        for (name, body) in [
            ("tank", tank()),
            ("artillery", artillery()),
            ("turret T1", turret_t1()),
            ("turret T2", turret_t2()),
            ("base", command_base()),
            ("strider", strider()),
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

    // Четыре ноги. Колено выносится наружу, ступня возвращается
    // под корпус — отсюда изгиб, которого у столба не бывает.
    //
    // **Нога — повёрнутые бруски, а не лесенка** (ADR-231): бедро
    // и голень — сегменты, наведённые от сустава к суставу. Бедренный
    // шар — в корневом томе, он неподвижен относительно корпуса.
    // Колено, лодыжка и ступня — **тоже сегменты** (ADR-234): нога
    // переставляется походкой, и всё, что на ней, обязано ехать
    // с ней. Кости из корня убраны: при качании ноги они торчали бы
    // из бруска; объём ноги под урон — вид, как и сам сегмент.
    //
    // Все звенья крепятся к корпусу, а не друг к другу: рамки покоя
    // складывает тело, рамки шага считает рендер по шарнирам ноги,
    // и родитель ему не нужен.
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
            // The weight-bearing link stays solid, while the dark collars,
            // raised armour and outside ram make its direction legible.
            mark(&mut thigh, (0, 2), (0, 8), (0, 8), FINISH_GUN);
            mark(&mut thigh, (11, 13), (0, 8), (0, 8), FINISH_GUN);
            fill_f(
                &mut thigh,
                (2, 11),
                (7, 9),
                (1, 7),
                Part::Armour,
                FINISH_PLATE,
            );
            fill_f(
                &mut thigh,
                (3, 10),
                (4, 6),
                (7, 9),
                Part::Armour,
                FINISH_STEEL,
            );
            fill_f(
                &mut thigh,
                (5, 10),
                (4, 5),
                (8, 10),
                Part::Armour,
                FINISH_GUN,
            );
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
            mark(&mut shin, (0, 2), (0, 6), (0, 6), FINISH_GUN);
            mark(&mut shin, (9, 11), (0, 6), (0, 6), FINISH_RUBBER);
            fill_f(
                &mut shin,
                (2, 9),
                (5, 7),
                (0, 6),
                Part::Armour,
                FINISH_PLATE,
            );
            fill_f(
                &mut shin,
                (3, 9),
                (3, 4),
                (6, 8),
                Part::Armour,
                FINISH_STEEL,
            );
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
            mark_ball(&mut knee, (4, 4, 4), 4, FINISH_STEEL);
            fill_f(
                &mut knee,
                (6, 9),
                (2, 7),
                (2, 7),
                Part::Armour,
                FINISH_PLATE,
            );
            u.push_segment(Attach::flat((kx as i16, 15, kz as i16), (4, 4, 4)), knee);
            let mut ankle = UnitBody::new_scaled(7, 7, 7, 1, BODY_MM);
            ball(&mut ankle, (3, 3, 3), 3, Part::Armour);
            mark_ball(&mut ankle, (3, 3, 3), 3, FINISH_STEEL);
            fill_f(&mut ankle, (5, 7), (2, 6), (2, 5), Part::Armour, FINISH_GUN);
            u.push_segment(Attach::flat((ax as i16, 4, az as i16), (3, 3, 3)), ankle);
            // A split toe shows the walking direction even in the tiny RTS
            // silhouette; the broad rubber sole remains the contact surface.
            let mut foot = UnitBody::new_scaled(12, 5, 10, 1, BODY_MM);
            fill_f(
                &mut foot,
                (0, 11),
                (0, 1),
                (0, 10),
                Part::Armour,
                FINISH_RUBBER,
            );
            fill(&mut foot, (0, 10), (1, 3), (1, 9), Part::Armour);
            fill_f(
                &mut foot,
                (3, 7),
                (3, 5),
                (3, 7),
                Part::Armour,
                FINISH_STEEL,
            );
            for z in [(1, 4), (6, 9)] {
                fill_f(&mut foot, (9, 12), (1, 3), z, Part::Armour, FINISH_PLATE);
                fill_f(&mut foot, (11, 12), (1, 2), z, Part::Armour, FINISH_STEEL);
            }
            u.push_segment(Attach::flat((ax as i16, 4, az as i16), (4, 4, 4)), foot);
            // Опоры подвески идут «нос по +Z, нос по −Z, корма по +Z,
            // корма по −Z»; нос корпуса — +X.
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
            // Root joint is material in the body; animated leg links are
            // separate geometry and do not substitute for this attachment.
            u.push_load_path(WeaponPath {
                min: [(hx - 3) as u8, 25, (hz - 3) as u8],
                max: [(hx + 4) as u8, 32, (hz + 4) as u8],
                axis: 1,
            });
        }
    }

    // Корпус — линза: сужается и книзу, и кверху. Книзу обязательно:
    // с плоским днищем бедренные шары сливаются с корпусом в одно
    // пятно, и машина перестаёт читаться стоящей на ногах.
    for y in 26..38 {
        let k = if y < 30 {
            (30 - y) as i32
        } else if y < 34 {
            0
        } else {
            (y - 34) as i32
        };
        disc(&mut u, (20, 20), 13 - k, (y, y + 1), Part::Armour);
    }
    // A recessed waist and angled side armour separate the four leg roots
    // from the turret, instead of reading as a single square body.
    mark(&mut u, (8, 33), (29, 30), (8, 33), FINISH_GUN);
    for &(z, side) in &[(8, -1), (32, 1)] {
        skirt(
            &mut u,
            12,
            16,
            37,
            z,
            side,
            TURN / 16,
            7,
            2,
            false,
            Part::Armour,
            FINISH_PLATE,
        );
    }
    fill(&mut u, (27, 32), (32, 37), (14, 26), Part::Armour);
    for z in [14, 24] {
        fill_f(
            &mut u,
            (30, 33),
            (34, 36),
            (z, z + 2),
            Part::Armour,
            FINISH_STEEL,
        );
    }
    // Двигатель в корме корпуса, выхлопной стакан назад.
    fill(&mut u, (9, 16), (28, 36), (14, 26), Part::Engine);
    for z in [15usize, 25] {
        cyl_f(
            &mut u,
            0,
            (2, 10),
            (33, z as i32),
            2,
            0,
            Part::Engine,
            FINISH_GUN,
        );
        fill_f(
            &mut u,
            (2, 4),
            (32, 34),
            (z - 1, z + 2),
            Part::Engine,
            FINISH_STEEL,
        );
    }
    for z in (15..26).step_by(3) {
        fill_f(
            &mut u,
            (8, 10),
            (32, 35),
            (z, z + 1),
            Part::Engine,
            FINISH_STEEL,
        );
    }

    // The neck is exposed between the hull and a chamfered, rotating head.
    ball(&mut u, (20, 38, 20), 6, Part::Armour);
    plated_turret(
        &mut u,
        (12, 29),
        (40, 47),
        (12, 28),
        (6, 3),
        TURN / 16,
        7,
        2,
        true,
        Part::Armour,
        FINISH_PLATE,
    );
    fill(&mut u, (15, 26), (47, 48), (14, 26), Part::Armour);
    // Independent mantlets and cooling collars make both functional barrels
    // readable from above. Their root voxels remain the damage-bearing path.
    for z in [16, 24] {
        fill_f(
            &mut u,
            (24, 31),
            (42, 47),
            (z - 3, z + 3),
            Part::Weapon,
            FINISH_GUN,
        );
        cyl_f(
            &mut u,
            0,
            (28, 40),
            (44, z as i32),
            2,
            0,
            Part::Weapon,
            FINISH_GUN,
        );
        cyl_f(
            &mut u,
            0,
            (32, 35),
            (44, z as i32),
            3,
            0,
            Part::Weapon,
            FINISH_STEEL,
        );
        cyl_f(
            &mut u,
            0,
            (38, 40),
            (44, z as i32),
            2,
            0,
            Part::Weapon,
            FINISH_STEEL,
        );
    }
    // Small glazed slit in a dark sensor housing; the former white block
    // dominated the whole silhouette at gameplay distance.
    fill_f(
        &mut u,
        (22, 29),
        (47, 49),
        (17, 23),
        Part::Sensor,
        FINISH_GUN,
    );
    fill_f(
        &mut u,
        (28, 30),
        (47, 49),
        (18, 22),
        Part::Sensor,
        FINISH_OPTIC,
    );
    for z in [10, 28] {
        fill_f(
            &mut u,
            (15, 20),
            (44, 47),
            (z, z + 2),
            Part::Sensor,
            FINISH_GUN,
        );
        fill_f(
            &mut u,
            (19, 21),
            (45, 47),
            (z, z + 2),
            Part::Sensor,
            FINISH_OPTIC,
        );
    }

    // Ядро ставится **последним**: шея-шар задевает середину корпуса,
    // и один перекрашенный ею воксель ломал сплошной куб затравки.
    fill(&mut u, (17, 23), (29, 35), (17, 23), Part::Core);

    let mut u = seat(u, "шагоход");
    // Вращается всё, что выше корпуса: голова со стволами.
    u.turret_y = 38;
    for z in [16, 24] {
        u.push_weapon_path(WeaponPath {
            min: [24, 42, z - 2],
            max: [40, 47, z + 3],
            axis: 0,
        });
    }
    u
}

/// Giant eight-legged assault platform, with a single axial laser and
/// independently damageable articulated legs. Coarser 200 mm armour keeps
/// its large silhouette affordable without inflating the voxel count.
pub fn spider() -> UnitBody {
    let mut u = UnitBody::new_scaled(88, 52, 80, 8, 200);
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
    fill(&mut u, (40, 49), (29, 38), (35, 45), Part::Core);
    let mut u = seat(u, "паук");
    u.turret_y = 40;
    u.push_weapon_path(WeaponPath {
        min: [56, 42, 37],
        max: [88, 49, 44],
        axis: 0,
    });
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
    fn twin_barrels_are_independent_and_resampling_preserves_the_rule() {
        let mut body = strider();
        let mut core = CoreBufs::new();
        assert!(body.status(&mut core).weapon);
        for (barrel, z) in [16usize, 24].into_iter().enumerate() {
            for y in 40..49 {
                for pz in z - 2..z + 3 {
                    body.remove(35, y, pz);
                }
            }
            assert_eq!(body.status(&mut core).weapon, barrel == 0);
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
        let s = strider();
        // 4×5×4 м при вокселе 0.1 м — том 40×50×40.
        assert_eq!((s.vol.nx, s.vol.ny, s.vol.nz), (40, 50, 40));
        assert_eq!(
            s.extent_mm(),
            (4_000, 5_000, 4_000),
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
        let s = strider();
        let mut b = CoreBufs::new();
        let st = s.status(&mut b);
        assert!(st.alive && st.engine && st.weapon && st.sensor, "{st:?}");
        assert_eq!(st.detached, 0, "часть тела не связана с ядром");
    }

    /// **Два тела не спутать.** Разные габариты — это то, ради чего
    /// второе тело и заводилось.
    #[test]
    fn the_two_bodies_do_not_look_alike() {
        let (t, s) = (tank(), strider());
        assert_ne!(
            (t.vol.nx, t.vol.ny, t.vol.nz),
            (s.vol.nx, s.vol.ny, s.vol.nz),
            "силуэты совпадают — это одно тело в двух цветах"
        );
        // Танк длиннее, чем высокий; шагоход наоборот.
        assert!(t.vol.nx > t.vol.ny, "танк не длинный");
        assert!(s.vol.ny > s.vol.nx, "шагоход не высокий");
    }

    /// **Линия башни лежит внутри тома и отрезает непустую часть.**
    /// Линия за габаритом даёт пустую башню, линия в нуле — тело,
    /// которое поворачивается целиком; и то и другое молча.
    #[test]
    fn the_turret_line_cuts_something_off() {
        for b in [tank(), strider()] {
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
        let s = strider();
        assert!(
            s.wreck(0).extent_mm().1 * 2 <= s.extent_mm().1,
            "полая машина не осела и вдвое: {} из {}",
            s.wreck(0).extent_mm().1,
            s.extent_mm().1
        );
        for b in [tank(), strider()] {
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
    fn artillery_t2_has_connected_vitals_a_wider_bore_and_more_armour() {
        let t1 = artillery();
        let t2 = artillery_t2();
        assert_eq!(t1.extent_mm(), t2.extent_mm());
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
