//! Скелет симуляции: SoA-хранилище, явный список фаз (ADR-048), хеш состояния.
//!
//! Существует ради замера, которого требует `../../docs/11-roadmap.md`:
//! «фазы движения и боя под нагрузкой, а не пустыми. Прогон "5000 сущностей
//! с пустыми фазами" не способен провалиться там, где должен».
//!
//! Содержательны здесь четыре фазы: `movement`, `combat`, `visibility`
//! и `hash`. Остальные одиннадцать — заглушки, как и предусмотрено ADR-048:
//! они существуют, участвуют в порядке и ничего не делают.

#![forbid(unsafe_code)]

pub mod energy;
pub mod ground;
pub mod shot;
pub mod index;
pub mod matter;
pub mod shield;

use fx::rng::Rng;
use fx::Fx;
use index::Grid;
pub use modding::Tuning;

/// Сторона мира в мировых единицах.
pub const WORLD: i32 = 2000;
/// Сторона нав-клетки.
pub const NAV_CELL: i32 = 2;
/// Сторона нав-сетки.
pub const NAV: usize = (WORLD / NAV_CELL) as usize;

/// Тиков в секунде. Скорость юнита задана в клетках **в секунду**,
/// поэтому шаг за тик делится на это число. Голая тридцатка в формуле
/// шага один раз уже стоила дорого: эталонный сценарий разводил
/// стороны на расстояние, которое они не проходили за прогон (ADR-116).
pub const TICKS_PER_SECOND: i32 = 30;


/// Событие тика — то, что рендеру надо показать, а симуляции знать
/// незачем.
///
/// Объявлено контрактом `docs/02-architecture.md` («взрывы, смерти,
/// завершение постройки») и до ADR-167 не существовало: рендер видел
/// только воксельный мир, и ни выстрела, ни взрыва на экране быть
/// не могло в принципе.
///
/// **Это выход, а не состояние.** События не входят в хеш тика: они
/// вычисляются из него, и хешировать их значило бы хешировать одно
/// и то же дважды. Живут ровно один тик — копятся за него и стираются
/// в начале следующего.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimEvent {
    /// Взрыв: где рвануло и какого радиуса воронка.
    Blast { x: i32, y: i32, z: i32, radius: i32 },
    /// Смерть юнита: кто и где.
    Death { unit: u32, x: i32, y: i32, z: i32 },
    /// Луч: откуда и куда. Живёт один тик — ровно столько, сколько
    /// длится сам выстрел (ADR-169).
    Beam {
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        owner: u8,
    },
}

/// Список фаз тика — окончательный (ADR-048). Порядок объявлен фиксированным
/// навсегда: его изменение меняет хеши всех golden-файлов.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    ApplyCommands,
    ModPreTick,
    VoxelEdits,
    Destruction,
    Fluids,
    /// Диффузия разрежённых слоёв: урон, температура, насыщение (ADR-097).
    /// Заглушка до этапа 6; место занято сейчас, пока golden-файлов нет.
    Diffusion,
    Bodies,
    NavUpdate,
    Movement,
    Combat,
    Logistics,
    Economy,
    Visibility,
    ModPostTick,
    Cleanup,
    Hash,
}

pub const PHASES: [Phase; 16] = [
    Phase::ApplyCommands,
    Phase::ModPreTick,
    Phase::VoxelEdits,
    Phase::Destruction,
    Phase::Fluids,
    Phase::Diffusion,
    Phase::Bodies,
    Phase::NavUpdate,
    Phase::Movement,
    Phase::Combat,
    Phase::Logistics,
    Phase::Economy,
    Phase::Visibility,
    Phase::ModPostTick,
    Phase::Cleanup,
    Phase::Hash,
];

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::ApplyCommands => "apply_commands",
            Phase::ModPreTick => "mod_pre_tick",
            Phase::VoxelEdits => "voxel_edits",
            Phase::Destruction => "destruction",
            Phase::Fluids => "fluids",
            Phase::Diffusion => "diffusion",
            Phase::Bodies => "bodies",
            Phase::NavUpdate => "nav_update",
            Phase::Movement => "movement",
            Phase::Combat => "combat",
            Phase::Logistics => "logistics",
            Phase::Economy => "economy",
            Phase::Visibility => "visibility",
            Phase::ModPostTick => "mod_post_tick",
            Phase::Cleanup => "cleanup",
            Phase::Hash => "hash",
        }
    }
}

/// Юниты, SoA. Удаление — через free-list, никогда `swap_and_pop`
/// (`CLAUDE.md`): перестановка соседей молча меняет исход ничьей.
pub struct Units {
    pub x: Vec<i32>,
    /// Высота: воксель, на котором юнит стоит. Без земли — ноль.
    ///
    /// Юнит двумерен по движению и трёхмерен по положению: он ходит
    /// по полу нав-клетки, а высота этого пола меняется от копки.
    /// Без третьей координаты нельзя ни посадить его на отвалившийся
    /// кусок, ни сказать, откуда он упал (ADR-135).
    pub y: Vec<i32>,
    pub z: Vec<i32>,
    /// Дробная часть положения: движение идёт в `Fx`, сетка — в целых.
    pub fx: Vec<Fx>,
    pub fz: Vec<Fx>,
    pub owner: Vec<u8>,
    pub speed: Vec<i32>,
    /// Прикреплённых к ядру вокселей — то, что у нас вместо скалярного HP.
    pub voxels: Vec<u16>,
    pub alive: Vec<bool>,
    pub target: Vec<u32>,
    /// Устойчивый номер тела, на котором юнит едет; ноль — стоит
    /// на земле. Индексом ссылаться нельзя (ADR-118).
    pub riding: Vec<u64>,
    /// Смещение относительно начала координат тела.
    pub ride_off: Vec<(i32, i32, i32)>,
    /// Высота, с которой юнит падает; `NOT_FALLING` — стоит на опоре.
    ///
    /// Как и `ride_off`, в хеш не входит: хеш ловит расхождение по `y`
    /// и по вокселям, а они меняются тем же падением на том же тике.
    pub fall_from: Vec<i32>,
    /// Вертикальная скорость падения, вокселей **за тик**. Растёт
    /// на тяготение каждый тик полёта (ADR-174).
    ///
    /// В хеш не входит по той же причине, что `fall_from`: разошедшаяся
    /// скорость разводит `y`, а `y` хешируется.
    pub fall_v: Vec<Fx>,
    /// Дробная часть высоты, пока юнит падает. Без неё падение шло бы
    /// только целыми вокселями за тик, то есть не ускорялось бы
    /// вовсе: первая секунда свободного падения — меньше воксела
    /// за тик.
    pub fall_f: Vec<Fx>,
    /// Тик, раньше которого юнит не ищет цель заново. Поиск цели —
    /// самая дорогая часть боя, и держать её потиковой незачем: цель,
    /// живая и в радиусе, остаётся целью (ADR-116).
    next_search: Vec<u32>,
    /// Тик, раньше которого юнит не выстрелит. Перезарядка —
    /// состояние юнита, а не свойство выстрела (ADR-164).
    next_shot: Vec<u32>,
    /// Место оружия в реестре. Место, а не номер: номер ищется
    /// перебором, а это горячий путь (ADR-169).
    pub weapon: Vec<u16>,
    free: Vec<u32>,
    live_ids: Vec<u32>,
}

pub const NO_TARGET: u32 = u32::MAX;

/// Настройки для тестов — **из того же файла**, по которому идёт игра.
///
/// Не литералы: числа тика живут в `data/sim.ron` и больше нигде
/// (ADR-140). `include_str!` разворачивается на сборке, никакого
/// ввода-вывода в тесте не появляется.
#[cfg(test)]
pub(crate) fn test_tuning() -> Tuning {
    Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("data/sim.ron")
}

/// Реестр оружия для тестов — из тех же данных, что у игры.
#[cfg(test)]
pub(crate) fn test_weapons() -> modding::weapons::Weapons {
    modding::weapons::from_ron(include_str!("../../../data/weapons.ron"))
        .expect("data/weapons.ron")
}

/// Юнит стоит на опоре, а не падает.
pub const NOT_FALLING: i32 = i32::MIN;

/// Урон от падения с высоты `fallen` вокселей.
///
/// **Первый воксель бесплатен** (ADR-036): падение на один воксель
/// даёт нулевой урон и читается как шаг вниз. Иначе копатель платил бы
/// здоровьем за каждый снятый у себя под ногами воксель, а забой —
/// это то, где он стоит всю игру.
///
/// Правило одно для всех случаев — сам выкопал, отделился кусок,
/// завалило оседающей материей, — поэтому и функция одна.
pub fn fall_damage(fallen: u32, per_voxel: u16) -> u32 {
    fallen.saturating_sub(1).saturating_mul(per_voxel as u32)
}

/// Проходов по неподвижной точке прицела (ADR-173).
///
/// Не игровое число, а точность метода: подъём ствола зависит
/// от времени полёта, а время — от подъёма. Три прохода доводят
/// невязку на полной дальности до сотых долей воксела, четвёртый
/// уже ничего не меняет.
const AIM_ITERS: u32 = 3;

/// Ускорение свободного падения **за тик**, в `Fx`.
///
/// В данных оно задано вокселями в секунду за секунду: в `data/*.ron`
/// живут целые числа, а за тик выходит дробь (ADR-173). Перевод —
/// одно точное деление в Q32.32, и делает его тот, кто считает,
/// а не тот, кто хранит.
///
/// Число одно на всех: юнит, обломок и снаряд ускоряются одинаково.
pub fn gravity_per_tick(t: &Tuning) -> Fx {
    Fx::from_ratio(t.gravity, TICKS_PER_SECOND * TICKS_PER_SECOND)
}

impl Default for Units {
    fn default() -> Self {
        Self::new()
    }
}

impl Units {
    pub fn new() -> Units {
        Units {
            x: Vec::new(),
            y: Vec::new(),
            z: Vec::new(),
            fx: Vec::new(),
            fz: Vec::new(),
            owner: Vec::new(),
            speed: Vec::new(),
            voxels: Vec::new(),
            alive: Vec::new(),
            target: Vec::new(),
            riding: Vec::new(),
            ride_off: Vec::new(),
            fall_from: Vec::new(),
            fall_v: Vec::new(),
            fall_f: Vec::new(),
            next_search: Vec::new(),
            next_shot: Vec::new(),
            weapon: Vec::new(),
            free: Vec::new(),
            live_ids: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn live(&self) -> &[u32] {
        &self.live_ids
    }

    pub fn spawn(&mut self, x: i32, z: i32, owner: u8, speed: i32, voxels: u16) -> u32 {
        if let Some(i) = self.free.pop() {
            let k = i as usize;
            self.x[k] = x;
            self.y[k] = 0;
            self.z[k] = z;
            self.fx[k] = Fx::ZERO;
            self.fz[k] = Fx::ZERO;
            self.owner[k] = owner;
            self.speed[k] = speed;
            self.voxels[k] = voxels;
            self.alive[k] = true;
            self.target[k] = NO_TARGET;
            self.riding[k] = 0;
            self.ride_off[k] = (0, 0, 0);
            self.fall_from[k] = NOT_FALLING;
            self.fall_v[k] = Fx::ZERO;
            self.fall_f[k] = Fx::ZERO;
            self.next_search[k] = 0;
            self.next_shot[k] = 0;
            self.weapon[k] = 0;
            return i;
        }
        self.x.push(x);
        self.y.push(0);
        self.z.push(z);
        self.fx.push(Fx::ZERO);
        self.fz.push(Fx::ZERO);
        self.owner.push(owner);
        self.speed.push(speed);
        self.voxels.push(voxels);
        self.alive.push(true);
        self.target.push(NO_TARGET);
        self.riding.push(0);
        self.ride_off.push((0, 0, 0));
        self.fall_from.push(NOT_FALLING);
        self.fall_v.push(Fx::ZERO);
        self.fall_f.push(Fx::ZERO);
        self.next_search.push(0);
        self.next_shot.push(0);
        self.weapon.push(0);
        (self.x.len() - 1) as u32
    }

    /// Освобождение слота. Слот попадает в free-list и переиспользуется
    /// позже; соседи не двигаются.
    pub fn kill(&mut self, i: u32) {
        let k = i as usize;
        if !self.alive[k] {
            return;
        }
        self.alive[k] = false;
        self.free.push(i);
    }

    /// Пересобрать список живых. Порядок — по возрастанию индекса, всегда.
    pub fn refresh_live(&mut self) {
        self.live_ids.clear();
        for i in 0..self.x.len() {
            if self.alive[i] {
                self.live_ids.push(i as u32);
            }
        }
    }
}

/// Мир: земля, юниты, поле направлений, индекс.
pub struct World {
    /// Воксельная часть. `None` — мира из вокселей нет вовсе.
    ///
    /// Необязательна нарочно: стенд, мерящий пять тысяч юнитов, не
    /// обязан строить под них двести пятьдесят шесть кубов породы,
    /// а фазы земли на пустом месте не делают ничего. Это не
    /// «опционально включаемая механика», а отсутствие данных.
    pub ground: Option<ground::Ground>,
    pub units: Units,
    /// Направление потока по нав-клеткам: 0 = −Z, 1 = −X, 2 = +X, 3 = +Z,
    /// 255 = стоять. По одному полю на владельца.
    pub flow: Vec<Vec<u8>>,
    /// **Два индекса, а не один.** Ведро равно радиусу запроса, иначе запрос
    /// перебирает лишних: с одним ведром по дальности стрельбы расталкивание
    /// радиусом 3 осматривало бы всех в радиусе 24 — в шестьдесят раз больше,
    /// чем нужно.
    pub grid_near: Grid,
    pub grid_far: Grid,
    pub tick: u64,
    /// Снимок положений на начало фазы движения. Без него расталкивание
    /// читает уже сдвинутых соседей, то есть зависит от порядка обхода —
    /// и параллелить его нельзя ни при каком разбиении.
    snap_x: Vec<i32>,
    snap_z: Vec<i32>,
    /// Смещения, посчитанные параллельно и применяемые одной серийной
    /// фазой (`CLAUDE.md`: клэмп применяется один раз, в конце).
    delta: Vec<(i32, i32)>,
    /// Замер: сколько сущностей осмотрели фазы за тик.
    pub examined_move: u64,
    pub examined_combat: u64,
    /// Замер: сколько шагов упёрлось в рельеф. Ноль на карте с землёй
    /// значит, что юниты по ней не ходят.
    pub blocked_moves: u64,
    /// Числа тика: урон падения, периоды, потолки. **Из данных**
    /// (ADR-140), поэтому не поля с литералами, а один слепок,
    /// пришедший снаружи.
    pub tuning: Tuning,
    /// Точка сбора владельца — то, к чему считается поле.
    flow_target: [Option<(i32, i32)>; 2],
    /// Поле направлений: общий граф, секторы и по полю на владельца
    /// (ADR-153).
    nav_field: nav::sectors::NavField,
    /// Узлов получило конечное время в последнем опубликованном поле.
    pub flow_reachable: usize,
    /// Годуновских обновлений ушло на последнее поле.
    pub flow_updates: u64,
    /// Полей опубликовано за прогон.
    pub flow_solves: u64,
    /// Тиков ушло на последнее поле.
    pub flow_ticks: u32,
    /// Узлов и рёбер в графе последнего поля.
    pub flow_nodes: usize,
    pub flow_edges: usize,
    /// Секторов поля пересчитано и взято готовыми за прогон.
    pub flow_solved_sectors: usize,
    pub flow_reused_sectors: usize,
    /// Компонент связности при последнем решении. Единица — карта
    /// цела; тысяча — она рассыпана, и это надо видеть.
    pub flow_components: usize,
    /// Оружие из данных. Вид задаёт, как приходит урон (ADR-169).
    pub weapons: modding::weapons::Weapons,
    /// Снаряды в воздухе. Бой больше не мгновенный (ADR-164).
    pub shots: shot::Shots,
    /// Замер: выстрелов лучом и попаданий лучом.
    pub beams_fired: u64,
    pub beams_hit: u64,
    /// Замер: сколько раз луч не выстрелил, потому что цель закрыта.
    pub beams_blocked: u64,
    /// Замер: выстрелов сделано, попаданий по юнитам, упоров в породу.
    pub shots_fired: u64,
    pub shots_hit: u64,
    pub shots_blocked: u64,
    /// Замер: выстрелов не сделано, потому что воздух полон.
    pub shots_refused: u64,
    /// Замер: юнитов погибло за всё время жизни мира.
    ///
    /// Считается прямо, а не как убыль живых: с подкреплениями (ADR-161)
    /// число живых держится ровным, и смерти по нему не видны вовсе.
    pub deaths: u64,
    /// Замер: взрывов было, вокселей поставлено в очередь на снос,
    /// юнитов задето взрывной волной.
    pub blasts: u64,
    pub blast_voxels: u64,
    pub blast_hits: u64,
    /// Накопитель урона за тик. Взрыв задевает многих, и один юнит
    /// может попасть под несколько: складывать надо до применения,
    /// иначе исход зависит от порядка снарядов.
    dmg_acc: Vec<u32>,
    /// События прошедшего тика — рендеру. Буфер переиспользуется:
    /// событий за тик тысячи, и выделять его заново незачем.
    events: Vec<SimEvent>,
    /// Замер: сколько юнитов действительно искали цель за тик.
    pub searches: u64,
    /// Замер: в скольких тиках поиск упёрся в потолок.
    pub search_capped: u64,
    /// Переиспользуемый список тех, кому нужен поиск. Бой идёт каждый
    /// тик, и выделять его заново незачем.
    need_search: Vec<u32>,
    pub phase_ns: [u64; 16],
}

const DIRS: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];

impl World {
    pub fn new(owners: usize, capacity: usize, tuning: Tuning) -> World {
        World::with_cells(owners, capacity, tuning.separation, tuning.range, tuning)
    }

    /// Привязать реестр оружия.
    ///
    /// Отдельным вызовом, а не полем конструктора: оружие приходит
    /// из пака, как материалы, и мир без него законен — просто
    /// не стреляет.
    pub fn attach_weapons(&mut self, w: modding::weapons::Weapons) {
        self.weapons = w;
    }

    /// Привязать воксельный мир. До этого фазы земли — заглушки.
    pub fn attach_ground(&mut self, g: ground::Ground) {
        self.ground = Some(g);
    }

    /// Сколько наносекунд ушло на фазу за всё время жизни мира.
    ///
    /// В наносекундах, а не в миллисекундах: доля — это плавающее,
    /// а его в симуляции нет (`CLAUDE.md`). Делит на тысячу тот,
    /// кто печатает.
    pub fn phase_ns_of(&self, p: Phase) -> u64 {
        match PHASES.iter().position(|&x| x == p) {
            Some(i) => self.phase_ns[i],
            None => 0,
        }
    }

    /// Направление поля под юнитом — **замерщику**, а не движению:
    /// движение читает поле само. Нужно, чтобы отличить «юниту некуда
    /// идти» от «юнит стоит в толпе»: без этого «застрял» — не диагноз.
    pub fn dir_under(&self, i: usize) -> u8 {
        let Some(g) = self.ground.as_ref() else { return 255 };
        let st = g.nav.step() as i32;
        let (gw, gh) = g.nav.dims();
        let (cx, cz) = ((self.units.x[i] / st).max(0) as u32, (self.units.z[i] / st).max(0) as u32);
        if cx >= gw || cz >= gh {
            return 255;
        }
        match g.nav.level_index_near(cx, cz, self.units.y[i]) {
            Some(li) => self.nav_field.dir_at(self.units.owner[i] as usize % 2, cx, cz, li),
            None => 255,
        }
    }

    /// События прошедшего тика: взрывы и смерти.
    ///
    /// Рендер **читает** их и никогда не пишет: нарушение границы
    /// слоёв — откат (`CLAUDE.md`).
    pub fn events(&self) -> &[SimEvent] {
        &self.events
    }

    /// Земля для чтения — рендеру и сценам.
    pub fn ground(&self) -> Option<&ground::Ground> {
        self.ground.as_ref()
    }

    /// Земля для правки — **сценарию**, который ставит правки в очередь.
    /// Рендер сюда не ходит: он состояние читает, а не пишет
    /// (`CLAUDE.md`, границы слоёв).
    pub fn ground_mut(&mut self) -> Option<&mut ground::Ground> {
        self.ground.as_mut()
    }

    /// Размеры вёдер — параметр, а не константа: они задают обмен между
    /// ценой пересборки индекса и ценой запроса, и оптимум зависит
    /// от плотности. Мелкое ведро дёшево в запросе и дорого в пересборке:
    /// при стороне 3 на карте 2000 это 445 тысяч корзин, которые надо
    /// обнулить каждый тик.
    pub fn with_cells(
        owners: usize,
        capacity: usize,
        near: i32,
        far: i32,
        tuning: Tuning,
    ) -> World {
        World {
            units: Units::new(),
            flow: (0..owners).map(|_| vec![255u8; NAV * NAV]).collect(),
            grid_near: Grid::new(WORLD, near, capacity),
            grid_far: Grid::new(WORLD, far, capacity),
            snap_x: Vec::new(),
            ground: None,
            snap_z: Vec::new(),
            delta: Vec::new(),
            tick: 0,
            examined_move: 0,
            examined_combat: 0,
            blocked_moves: 0,
            tuning,
            flow_target: [None, None],
            nav_field: nav::sectors::NavField::new(2),
            flow_reachable: 0,
            flow_updates: 0,
            flow_solves: 0,
            flow_ticks: 0,
            flow_nodes: 0,
            flow_edges: 0,
            flow_solved_sectors: 0,
            flow_reused_sectors: 0,
            flow_components: 0,
            weapons: modding::weapons::Weapons::new(),
            shots: shot::Shots::new(),
            beams_fired: 0,
            beams_hit: 0,
            beams_blocked: 0,
            shots_fired: 0,
            shots_hit: 0,
            shots_blocked: 0,
            shots_refused: 0,
            deaths: 0,
            blasts: 0,
            blast_voxels: 0,
            blast_hits: 0,
            dmg_acc: Vec::new(),
            events: Vec::new(),
            searches: 0,
            search_capped: 0,
            need_search: Vec::new(),
            phase_ns: [0; 16],
        }
    }

    /// Поставить юнита **на поверхность** его клетки.
    ///
    /// Без этого он начинает на нулевой высоте, и первый же тик
    /// притягивает его к ближайшему уровню — то есть к **самому
    /// нижнему**, на дно пещеры. На D-7 так и было: пять тысяч юнитов
    /// стояли в подземельях, медиана высоты 78 при поверхности 120,
    /// и связаны эти подземелья не были (ADR-138).
    pub fn spawn_ground(&mut self, x: i32, z: i32, owner: u8, speed: i32, voxels: u16) -> u32 {
        let id = self.units.spawn(x, z, owner, speed, voxels);
        if let Some(g) = self.ground.as_ref() {
            let st = g.nav.step() as i32;
            let (gw, gh) = g.nav.dims();
            let (cx, cz) = ((x / st).max(0) as u32, (z / st).max(0) as u32);
            if cx < gw && cz < gh {
                let t = g.nav.top_at(cx, cz);
                if t != nav::layer::NO_FLOOR {
                    self.units.y[id as usize] = t as i32;
                }
            }
        }
        id
    }

    /// Поле направлений: все идут к точке сбора своего владельца.
    ///
    /// **По прямой.** Земли эта заливка не знает и знать не может:
    /// она есть и без воксельного мира. Когда земля привязана,
    /// её накрывает решение эйконала (`solve_flow`), а прямая
    /// остаётся тем, с чего поле начинается.
    pub fn fill_flow_towards(&mut self, owner: usize, tx: i32, tz: i32) {
        self.flow_target[owner] = Some((tx, tz));
        self.nav_field.set_target(owner, tx, tz);
        let f = &mut self.flow[owner];
        for cz in 0..NAV {
            for cx in 0..NAV {
                let (dx, dz) = (
                    tx - (cx as i32 * NAV_CELL),
                    tz - (cz as i32 * NAV_CELL),
                );
                let d = if dx.abs() >= dz.abs() {
                    if dx > 0 {
                        2
                    } else if dx < 0 {
                        1
                    } else {
                        255
                    }
                } else if dz > 0 {
                    3
                } else {
                    0
                };
                f[cz * NAV + cx] = d;
            }
        }
    }

    /// Поле направлений по навигации: эйконал на сетке нав-слоя.
    ///
    /// До этого юниты шли к цели **по прямой**, а порода их просто
    /// останавливала: на D-7 три с половиной тысячи шагов за тик
    /// упирались в камень при трёх тысячах живых юнитов, то есть
    /// каждый юнит каждый тик (ADR-138).
    ///
    /// Поле наземное: сетка берёт нижний уровень клетки. Юнит на крыше
    /// поле по крыше не получит — для этого узлом должна стать пара
    /// «клетка и уровень».
    /// Продвинуть поле направлений одного владельца на порцию работы.
    ///
    /// Владельцы идут по очереди, по одному за тик: два поля разом
    /// удваивают порцию, а порция и есть то, что держит тик.
    /// Продвинуть расчёт поля на порцию работы.
    ///
    /// Стадий четыре — граф, компоненты, секторы, поле, — и все идут
    /// с потолками из данных (ADR-152, ADR-153). Юниты всё это время
    /// читают опубликованное поле.
    pub fn step_flow(&mut self) {
        use nav::godunov::h_exact_floor;
        let b = nav::sectors::FieldBudget {
            cells: self.tuning.flow_cells_per_tick,
            scan: self.tuning.flow_scan_per_tick,
            sectors: self.tuning.flow_sectors_per_tick,
        };
        let Some(g) = self.ground.as_ref() else { return };
        // Правки тика метят секторы: пересобирать надо задетые.
        let (gw, _) = g.nav.dims();
        let step = g.nav.step();
        for e in g.edits() {
            self.nav_field.mark_cell(e.x / step, e.z / step, gw);
        }
        let nav = &self.ground.as_ref().expect("земля").nav;
        if self.nav_field.step(nav, b, &h_exact_floor) {
            self.flow_nodes = self.nav_field.nodes;
            self.flow_components = self.nav_field.components;
            self.flow_reachable = self.nav_field.reached;
            self.flow_ticks = self.nav_field.last_ticks;
            self.flow_solves = self.nav_field.published;
            self.flow_edges = self.nav_field.portals;
            self.flow_solved_sectors = self.nav_field.solved_sectors;
            self.flow_reused_sectors = self.nav_field.reused_sectors;
        }
    }

    /// Посчитать поле до конца. Для сцен и тестов, которым отставание
    /// мешает: в тике так нельзя, там оно порциями.
    pub fn solve_flow(&mut self) {
        if self.ground.is_none() {
            return;
        }
        let before = self.nav_field.published;
        let mut guard = 0;
        while self.nav_field.published == before {
            self.step_flow();
            guard += 1;
            assert!(guard < 1_000_000, "поле не сошлось");
        }
    }

    /// Один тик на заданном числе воркеров. Хеш обязан совпадать
    /// при любом их числе (`CLAUDE.md`).
    pub fn tick_par(&mut self, threads: usize) -> u64 {
        use std::time::Instant;
        let mut hash = 0u64;
        // События живут один тик: рендер читает их между тиками.
        self.events.clear();
        for (pi, &phase) in PHASES.iter().enumerate() {
            let t0 = Instant::now();
            match phase {
                Phase::VoxelEdits => self.phase_ground_edits(),
                Phase::Destruction => self.phase_ground_destruction(),
                Phase::Bodies => self.phase_ground_bodies(),
                Phase::NavUpdate => {
                    self.phase_ground_nav();
                    self.phase_flow();
                    self.phase_nav_update();
                }
                Phase::Movement => {
                    self.movement_deltas(threads);
                    self.apply_deltas();
                }
                Phase::Combat => self.phase_combat(),
                Phase::Visibility => self.phase_visibility(),
                Phase::Cleanup => self.phase_cleanup(),
                Phase::Hash => hash = self.phase_hash(),
                _ => {}
            }
            self.phase_ns[pi] += t0.elapsed().as_nanos() as u64;
        }
        self.tick += 1;
        hash
    }

    /// Один тик: все пятнадцать фаз в объявленном порядке.
    pub fn tick(&mut self) -> u64 {
        use std::time::Instant;
        let mut hash = 0u64;
        // События живут один тик: рендер читает их между тиками.
        self.events.clear();
        for (pi, &phase) in PHASES.iter().enumerate() {
            let t0 = Instant::now();
            match phase {
                Phase::VoxelEdits => self.phase_ground_edits(),
                Phase::Destruction => self.phase_ground_destruction(),
                Phase::Bodies => self.phase_ground_bodies(),
                Phase::NavUpdate => {
                    self.phase_ground_nav();
                    self.phase_flow();
                    self.phase_nav_update();
                }
                Phase::Movement => self.phase_movement(),
                Phase::Combat => self.phase_combat(),
                Phase::Visibility => self.phase_visibility(),
                Phase::Cleanup => self.phase_cleanup(),
                Phase::Hash => hash = self.phase_hash(),
                // Остальные — заглушки (ADR-048): существуют, ничего не делают.
                _ => {}
            }
            self.phase_ns[pi] += t0.elapsed().as_nanos() as u64;
        }
        self.tick += 1;
        hash
    }

    /// Фазы земли. Без воксельного мира они ровно те же заглушки,
    /// какими были: занятое место в списке, а не ветвление механики.
    fn phase_ground_edits(&mut self) {
        if let Some(g) = self.ground.as_mut() {
            g.phase_edits();
        }
    }

    fn phase_ground_destruction(&mut self) {
        if let Some(g) = self.ground.as_mut() {
            g.phase_destruction();
        }
    }

    /// Фаза тел: посадка наездников, шаг падения, следование за телом.
    ///
    /// Порядок внутри фазы важен и держится на ADR-133: тело,
    /// отвалившееся в этом тике, в нём же не падает, поэтому посадка
    /// успевает случиться до первого его движения.
    fn phase_ground_bodies(&mut self) {
        let dmg = self.tuning.fall_damage_per_voxel;
        let Self { ground, units: u, .. } = self;
        let Some(g) = ground.as_mut() else { return };

        // 1. Посадка на свежие тела: кто оказался над куском, едет
        //    на нём (ADR-099).
        for si in 0..g.bodies.spawned.len() {
            let id = g.bodies.spawned[si];
            let Some(bi) = g.bodies.index_of(id) else { continue };
            let b = &g.bodies.list[bi];
            let (ox, oy, oz) = (b.ox, b.oy, b.oz);
            let (sx, sz) = (b.sx as i64, b.sz as i64);
            let top = b.voxels.iter().map(|v| v.y as i64).max().unwrap_or(0) + 1;
            let mut riders: Vec<u32> = Vec::new();
            for k in 0..u.live_ids.len() {
                let i = u.live_ids[k] as usize;
                if u.riding[i] != 0 {
                    continue;
                }
                let (dx, dz) = (u.x[i] as i64 - ox, u.z[i] as i64 - oz);
                let dy = u.y[i] as i64 - oy;
                if dx >= 0 && dx < sx && dz >= 0 && dz < sz && dy >= 0 && dy <= top {
                    u.riding[i] = id;
                    u.ride_off[i] = (dx as i32, dy as i32, dz as i32);
                    riders.push(i as u32);
                }
            }
            g.stats.rides += riders.len() as u64;
            g.bodies.list[bi].riders = riders;
        }

        // 2. Шаг падения.
        g.phase_bodies();

        // 3. Наездники следуют за телом; севшие получают урон по высоте
        //    падения (ADR-036). Тело ищется по устойчивому номеру.
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            if u.riding[i] == 0 {
                continue;
            }
            if let Some(bi) = g.bodies.index_of(u.riding[i]) {
                let b = &g.bodies.list[bi];
                u.x[i] = (b.ox + u.ride_off[i].0 as i64).max(0) as i32;
                u.y[i] = (b.oy + u.ride_off[i].1 as i64).max(0) as i32;
                u.z[i] = (b.oz + u.ride_off[i].2 as i64).max(0) as i32;
                continue;
            }
            // Тела нет: приземлилось на этом шаге и само сказало,
            // сколько пролетело.
            let fallen = g
                .bodies
                .landed_log
                .iter()
                .find(|l| l.id == u.riding[i])
                .map(|l| l.fallen)
                .unwrap_or(0);
            let hit = fall_damage(fallen, dmg);
            u.voxels[i] = u.voxels[i].saturating_sub(hit.min(u16::MAX as u32) as u16);
            g.stats.fall_damage += hit as u64;
            g.stats.max_fall = g.stats.max_fall.max(fallen);
            u.riding[i] = 0;
        }
    }

    /// Пересчёт поля по расписанию. Период — из данных, а не «когда
    /// успеем»: разное отставание у разных клиентов это десинк (ADR-089).
    fn phase_flow(&mut self) {
        if self.ground.is_none() {
            return;
        }
        self.step_flow();
    }

    fn phase_ground_nav(&mut self) {
        if let Some(g) = self.ground.as_mut() {
            g.phase_nav();
        }
    }

    /// Индекс перестраивается один раз за тик, до фаз, которые его читают.
    fn phase_nav_update(&mut self) {
        self.units.refresh_live();
        self.grid_near
            .rebuild(&self.units.x, &self.units.z, &self.units.live_ids);
        self.grid_far
            .rebuild(&self.units.x, &self.units.z, &self.units.live_ids);
    }

    /// Движение: чтение направления из поля плюс расталкивание соседей.
    ///
    /// Расталкивание — единственное место фазы, где стоимость нелинейна
    /// по числу юнитов: она линейна по числу **пар в радиусе**, а это
    /// плотность, а не количество.
    fn phase_movement(&mut self) {
        self.movement_deltas(1);
        self.apply_deltas();
    }

    /// Подготовить снимок к фазе движения. Отделено от счёта, чтобы
    /// постоянный пул мог звать счёт много раз без пересборки снимка.
    pub fn prepare_movement(&mut self) {
        self.snap_x.clear();
        self.snap_x.extend_from_slice(&self.units.x);
        self.snap_z.clear();
        self.snap_z.extend_from_slice(&self.units.z);
        self.delta.clear();
        self.delta.resize(self.units.x.len(), (0, 0));
    }

    /// Смещение одного юнита. Читает только снимок и индекс —
    /// значит вызов из любого потока даёт то же самое.
    #[inline]
    pub fn movement_kernel(&self, i: usize) -> (i32, i32) {
        let u = &self.units;
        let (ux, uz) = (self.snap_x[i], self.snap_z[i]);
        let (cx, cz) = (
            (ux / NAV_CELL).clamp(0, NAV as i32 - 1) as usize,
            (uz / NAV_CELL).clamp(0, NAV as i32 - 1) as usize,
        );
        let d = self.flow[u.owner[i] as usize][cz * NAV + cx];
        let (mut vx, mut vz) = if d < 4 {
            let (dx, dz) = DIRS[d as usize];
            (dx * u.speed[i], dz * u.speed[i])
        } else {
            (0, 0)
        };
        let r2 = self.tuning.separation * self.tuning.separation;
        let (sx, sz) = (&self.snap_x, &self.snap_z);
        self.grid_near.for_each_near(ux, uz, |j| {
            let j = j as usize;
            if j == i {
                return;
            }
            let (dx, dz) = (ux - sx[j], uz - sz[j]);
            let d2 = dx * dx + dz * dz;
            if d2 == 0 || d2 >= r2 {
                return;
            }
            let w = (r2 - d2) * 64 / r2;
            vx += dx * w / 16;
            vz += dz * w / 16;
        });
        (vx, vz)
    }

    /// Живые ли слот. Нужен пулу, чтобы не считать дырки free-list.
    #[inline]
    pub fn slot_alive(&self, i: usize) -> bool {
        self.units.alive[i]
    }

    pub fn slots(&self) -> usize {
        self.units.x.len()
    }

    /// Записать смещение — вызывается пулом в свой кусок буфера.
    pub fn take_delta(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.delta)
    }

    pub fn put_delta(&mut self, d: Vec<(i32, i32)>) {
        self.delta = d;
    }

    /// Посчитать смещения. Читает **только снимок**, пишет только
    /// в свой слот `delta` — значит фаза делится на потоки по слотам
    /// и результат от разбиения не зависит.
    fn movement_deltas(&mut self, threads: usize) {
        let u = &self.units;
        self.snap_x.clear();
        self.snap_x.extend_from_slice(&u.x);
        self.snap_z.clear();
        self.snap_z.extend_from_slice(&u.z);
        self.delta.clear();
        self.delta.resize(u.x.len(), (0, 0));

        let (sx, sz) = (&self.snap_x, &self.snap_z);
        let grid = &self.grid_near;
        let flow = &self.flow;
        // Земля есть — направление берётся из **объёмного** поля по узлу
        // «клетка и уровень»: юнит на крыше идёт по крышам, юнит
        // в туннеле — по туннелю (ADR-138). Без земли поле прежнее,
        // плоское: эталонный сценарий воксельного мира не строит.
        let vol = self.ground.as_ref().map(|g| (&g.nav, &self.nav_field));
        let r2 = self.tuning.separation * self.tuning.separation;

        let compute = |i: usize| -> (i32, i32) {
            let (ux, uz) = (sx[i], sz[i]);
            let d = match vol {
                Some((nav, field)) => {
                    let st = nav.step() as i32;
                    let (gw, gh) = nav.dims();
                    let (cx, cz) = ((ux / st).max(0) as u32, (uz / st).max(0) as u32);
                    if cx >= gw || cz >= gh {
                        255
                    } else {
                        match nav.level_index_near(cx, cz, u.y[i]) {
                            Some(li) => field.dir_at(u.owner[i] as usize % 2, cx, cz, li),
                            None => 255,
                        }
                    }
                }
                None => {
                    let (cx, cz) = (
                        (ux / NAV_CELL).clamp(0, NAV as i32 - 1) as usize,
                        (uz / NAV_CELL).clamp(0, NAV as i32 - 1) as usize,
                    );
                    flow[u.owner[i] as usize][cz * NAV + cx]
                }
            };
            let (mut vx, mut vz) = if d < 4 {
                let (dx, dz) = DIRS[d as usize];
                (dx * u.speed[i], dz * u.speed[i])
            } else {
                (0, 0)
            };
            grid.for_each_near(ux, uz, |j| {
                let j = j as usize;
                if j == i {
                    return;
                }
                let (dx, dz) = (ux - sx[j], uz - sz[j]);
                let d2 = dx * dx + dz * dz;
                if d2 == 0 || d2 >= r2 {
                    return;
                }
                let w = (r2 - d2) * 64 / r2;
                vx += dx * w / 16;
                vz += dz * w / 16;
            });
            (vx, vz)
        };

        let live = &u.live_ids;
        if threads <= 1 {
            let mut examined = 0u64;
            for &i in live.iter() {
                let i = i as usize;
                examined += grid.near_count(sx[i], sz[i]) as u64;
                self.delta[i] = compute(i);
            }
            self.examined_move += examined;
            return;
        }

        // Разбиение по слотам: каждый поток пишет в свой кусок `delta`,
        // куски не пересекаются, и порядок внутри куска фиксирован.
        let n = self.delta.len();
        let block = n.div_ceil(threads);
        let mut rest: &mut [(i32, i32)] = &mut self.delta;
        let mut parts: Vec<(usize, &mut [(i32, i32)])> = Vec::with_capacity(threads);
        let mut base = 0usize;
        while !rest.is_empty() {
            let take = block.min(rest.len());
            let (head, tail) = rest.split_at_mut(take);
            parts.push((base, head));
            base += take;
            rest = tail;
        }
        std::thread::scope(|sc| {
            for (offset, part) in parts {
                let compute = &compute;
                sc.spawn(move || {
                    for (k, slot) in part.iter_mut().enumerate() {
                        let i = offset + k;
                        if !u.alive[i] {
                            continue;
                        }
                        *slot = compute(i);
                    }
                });
            }
        });
    }

    /// Применить смещения. Серийная фаза: интегрирование в `Fx` и перенос
    /// целой части в сетку.
    fn apply_deltas(&mut self) {
        let fall_dmg = self.tuning.fall_damage_per_voxel;
        // Ускорение за тик считается **один раз на фазу**, а не на юнита:
        // это точное деление в `i128`, и повторять его пять тысяч раз
        // незачем.
        let g_tick = gravity_per_tick(&self.tuning);
        let Self { units: u, delta, ground, blocked_moves, .. } = self;
        // Счётчики падения — те же, что у наездников: правило одно,
        // значит и отчёт один.
        let (mut fall_damage_acc, mut max_fall_acc) = (0u64, 0u32);
        let (dmg_acc, max_acc) = (&mut fall_damage_acc, &mut max_fall_acc);
        // Земля, если она есть, решает, куда можно шагнуть. Перепад —
        // свойство ребра (ADR-121), поэтому спрашивается переход,
        // а не клетка.
        let terrain = ground.as_ref().map(|g| (&g.nav, g.extent()));
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let (vx, vz) = delta[i];
            let step_x = Fx::from_ratio(vx, TICKS_PER_SECOND);
            let step_z = Fx::from_ratio(vz, TICKS_PER_SECOND);
            let nx = u.fx[i].add(step_x);
            let nz = u.fz[i].add(step_z);
            let carry_x = nx.to_int_trunc();
            let carry_z = nz.to_int_trunc();
            u.fx[i] = nx.sub(Fx::from_int(carry_x));
            u.fz[i] = nz.sub(Fx::from_int(carry_z));
            let (x0, z0) = (u.x[i], u.z[i]);
            let mut x1 = (x0 + carry_x).clamp(0, WORLD - 1);
            let mut z1 = (z0 + carry_z).clamp(0, WORLD - 1);
            if let Some((nav, (ex, ez))) = terrain {
                let st = nav.step() as i32;
                let (gw, gh) = nav.dims();
                let cell = |x: i32, z: i32| -> Option<(u32, u32)> {
                    if x < 0 || z < 0 || x >= ex || z >= ez {
                        return None;
                    }
                    let (cx, cz) = ((x / st) as u32, (z / st) as u32);
                    if cx >= gw || cz >= gh {
                        return None;
                    }
                    Some((cx, cz))
                };
                let mut y = u.y[i];
                let here = cell(x0, z0);
                // **Опора исчезла — юнит падает** (ADR-036). Правило одно
                // для всех случаев: сам выкопал под собой, отделился кусок,
                // завалило оседающей материей. Падение на один воксель
                // даёт нулевой урон и читается как шаг вниз.
                //
                // Раньше высота просто притягивалась к ближайшему уровню,
                // то есть юнит над свежей ямой оказывался на её дне тем же
                // тиком и без урона (ADR-139).
                let unsupported = u.riding[i] == 0
                    && here.is_some_and(|c| !nav.has_level(c.0, c.1, y));
                if unsupported {
                    let c = here.expect("клетка есть");
                    match nav.level_below(c.0, c.1, y) {
                        Some(l) => {
                            let l = l as i32;
                            // Падает по вертикали: горизонтальный шаг
                            // на этом тике отменяется.
                            x1 = x0;
                            z1 = z0;
                            if l == y - 1 && u.fall_from[i] == NOT_FALLING {
                                // **Один воксель — шаг вниз, а не падение**
                                // (ADR-036: «читается как шаг вниз»).
                                // Копатель снимает воксель под собой всю
                                // игру, и заставлять его треть секунды
                                // висеть над собственной ямой — это
                                // не физика, а заикание (ADR-174).
                                y = l;
                            } else {
                                if u.fall_from[i] == NOT_FALLING {
                                    u.fall_from[i] = y;
                                    u.fall_v[i] = Fx::ZERO;
                                    u.fall_f[i] = Fx::ZERO;
                                }
                                // Скорость растёт на тяготение, высота —
                                // на скорость. Первые тики скорость
                                // меньше воксела, поэтому дробная часть
                                // копится отдельно.
                                u.fall_v[i] = u.fall_v[i].add(g_tick);
                                let nf = u.fall_f[i].add(u.fall_v[i]);
                                let drop = nf.to_int_trunc();
                                u.fall_f[i] = nf.sub(Fx::from_int(drop));
                                // **Опора не проскакивается.** `level_below`
                                // даёт ближайший уровень снизу, и клэмп
                                // по нему сажает юнита ровно на него,
                                // какой бы ни была скорость. Проверка
                                // `has_level` на конечной высоте
                                // пропустила бы пол (ADR-174).
                                y = (y - drop).max(l);
                                if y == l {
                                    let fallen = (u.fall_from[i] - y).max(0) as u32;
                                    let hit = fall_damage(fallen, fall_dmg);
                                    u.voxels[i] =
                                        u.voxels[i].saturating_sub(hit.min(u16::MAX as u32) as u16);
                                    *dmg_acc += hit as u64;
                                    *max_acc = (*max_acc).max(fallen);
                                    u.fall_from[i] = NOT_FALLING;
                                    u.fall_v[i] = Fx::ZERO;
                                    u.fall_f[i] = Fx::ZERO;
                                }
                            }
                        }
                        None => {
                            // Ниже юнита уровней нет: он либо ещё не
                            // поставлен на землю (высота ноль), либо его
                            // завалило оседающей материей. Выталкивается
                            // на нижний уровень клетки — вверх, без урона:
                            // урон берётся с высоты падения, а падения
                            // не было.
                            if let Some(&l) = nav.levels_at(c.0, c.1).first() {
                                y = l as i32;
                            }
                            u.fall_from[i] = NOT_FALLING;
                            u.fall_v[i] = Fx::ZERO;
                            u.fall_f[i] = Fx::ZERO;
                            x1 = x0;
                            z1 = z0;
                        }
                    }
                    u.y[i] = y;
                } else {
                    // Уровень выбирается по текущей высоте: с крыши
                    // переходят на крышу, а не на землю под ней (ADR-136).
                    // Оси проверяются по очереди — упёршийся шаг по одной
                    // не отменяет другую.
                    if let Some(c0) = here {
                        match cell(x1, z0) {
                            None => x1 = x0,
                            Some(c1) if c1 != c0 => match nav.step_to(c0, y, c1) {
                                Some(l) => y = l as i32,
                                None => {
                                    x1 = x0;
                                    *blocked_moves += 1;
                                }
                            },
                            _ => {}
                        }
                        let a = cell(x1, z0);
                        match (a, cell(x1, z1)) {
                            (_, None) => z1 = z0,
                            (Some(a), Some(b)) if a != b => match nav.step_to(a, y, b) {
                                Some(l) => y = l as i32,
                                None => {
                                    z1 = z0;
                                    *blocked_moves += 1;
                                }
                            },
                            _ => {}
                        }
                    }
                    if u.riding[i] == 0 {
                        u.y[i] = y;
                    }
                }
            }
            u.x[i] = x1;
            u.z[i] = z1;
        }
        if let Some(g) = ground.as_mut() {
            g.stats.fall_damage += fall_damage_acc;
            g.stats.max_fall = g.stats.max_fall.max(max_fall_acc);
        }
    }

    /// Бой: поиск цели и урон.
    ///
    /// Две части, и разделены они не для красоты. Поиск цели пишет только
    /// в `target[i]` — слоты не пересекаются, значит фаза параллелится.
    /// Урон пишет в **чужие** слоты, поэтому идёт серийно; накопитель
    /// складывается обычным сложением, потому что фаза серийная.
    fn phase_combat(&mut self) {
        // Сначала летят те, кто уже в воздухе, потом стреляют новые:
        // иначе снаряд успевал бы родиться и попасть в одном тике,
        // то есть время полёта было бы нулевым у первого шага.
        self.step_shots();
        let u = &mut self.units;
        let mut examined = 0u64;
        let r2 = self.tuning.range * self.tuning.range;

        // Часть первая: проверка уже выбранной цели. O(1) на юнита.
        //
        // Цель, живая, вражеская и в радиусе, остаётся целью — искать
        // заново незачем. В разреженном бою это ничего не меняет,
        // в плотном меняет всё: там поиск обходит весь сгусток, и цена
        // его квадратична по плотности, а не по числу юнитов (ADR-116).
        let tick = self.tick as u32;
        let period = self.tuning.retarget_period;
        let mut need = std::mem::take(&mut self.need_search);
        need.clear();
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let t = u.target[i];
            if t != NO_TARGET {
                let tt = t as usize;
                let (dx, dz) = (u.x[i] - u.x[tt], u.z[i] - u.z[tt]);
                let valid =
                    u.alive[tt] && u.owner[tt] != u.owner[i] && dx * dx + dz * dz <= r2;
                if !valid {
                    // Цель была и пропала — этот ищет вне очереди.
                    u.target[i] = NO_TARGET;
                    u.next_search[i] = tick;
                }
            }
            // Срок — **единственное** условие попадания в очередь поиска.
            //
            // Первая редакция ставила в очередь ещё и всякого, у кого
            // цели нет. В разреженном бою целей нет у тысяч юнитов, они
            // попадали в очередь каждый тик, потолок срабатывал все
            // тысячу тиков подряд, и откат после безрезультатного
            // поиска не работал вовсе. Потолок при этом маскировал
            // ошибку: цена выглядела приличной, потому что была
            // обрезана сверху.
            if tick >= u.next_search[i] {
                need.push(i as u32);
            }
        }

        // Часть вторая: собственно поиск, с потолком.
        //
        // Начало обхода сдвигается тиком. Без сдвига потолок всегда
        // отсекал бы один и тот же хвост списка, и юниты с большими
        // номерами не стреляли бы никогда. Сдвиг — чистая функция тика,
        // то есть детерминирован.
        let cap = self.tuning.max_target_searches.max(1);
        let n = need.len();
        let start = if n == 0 { 0 } else { tick as usize % n };
        let take = cap.min(n);
        if take < n {
            self.search_capped += 1;
        }
        for step in 0..take {
            let i = need[(start + step) % n] as usize;
            let (ux, uz, mine) = (u.x[i], u.z[i], u.owner[i]);
            let mut best = i32::MAX;
            let mut best_j = NO_TARGET;
            self.grid_far.for_each_near(ux, uz, |j| {
                examined += 1;
                let jj = j as usize;
                if u.owner[jj] == mine || !u.alive[jj] {
                    return;
                }
                let (dx, dz) = (ux - u.x[jj], uz - u.z[jj]);
                let d2 = dx * dx + dz * dz;
                if d2 > r2 {
                    return;
                }
                // Ничья разрешается **линейным индексом**, а не порядком
                // обхода (`CLAUDE.md`).
                if d2 < best || (d2 == best && j < best_j) {
                    best = d2;
                    best_j = j;
                }
            });
            u.target[i] = best_j;
            // Срок следующего пересмотра разводится по юнитам, иначе
            // они собираются в когорты и фаза снова становится пиковой
            // раз в период (ADR-090).
            let jitter = ((i as u32).wrapping_mul(2_654_435_761) >> 29) as u32;
            u.next_search[i] = tick + period + jitter;
        }
        self.searches += take as u64;
        self.need_search = need;
        self.examined_combat += examined;

        self.fire();
    }

    /// Выстрелы: у кого есть цель и кто перезарядился.
    ///
    /// Раньше здесь стоял мгновенный урон — `damage[цель] += 1` за тик
    /// с каждого стреляющего, сквозь горы и без времени на дорогу.
    /// Теперь вылетает снаряд (ADR-164).
    fn fire(&mut self) {
        use modding::weapons::WeaponKind;
        if self.weapons.is_empty() {
            return;
        }
        let (range, cap) = (self.tuning.range, self.tuning.max_shots);
        let g_tick = gravity_per_tick(&self.tuning);
        let tick = self.tick as u32;
        self.dmg_acc.clear();
        self.dmg_acc.resize(self.units.x.len(), 0);
        let Self {
            units: u,
            shots,
            weapons,
            ground,
            shots_fired,
            shots_refused,
            beams_fired,
            beams_hit,
            beams_blocked,
            dmg_acc,
            events,
            ..
        } = self;
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let tgt = u.target[i];
            if tgt == NO_TARGET || tick < u.next_shot[i] {
                continue;
            }
            let wi = (u.weapon[i] as usize).min(weapons.len() - 1);
            let w = *weapons.at(wi);
            let tt = tgt as usize;
            let from = (u.x[i], u.y[i], u.z[i]);
            let to = (u.x[tt], u.y[tt], u.z[tt]);

            match w.kind {
                WeaponKind::Laser => {
                    // Луч приходит в тот же тик — но по той же линии
                    // огня, что и снаряд. Породу он не прожигает:
                    // упёрся, значит не попал.
                    //
                    // **Закрытая цель выстрела не стоит.** Линия
                    // проверяется до выстрела, и если она закрыта,
                    // очередь не тратится: стрелок ждёт, пока цель
                    // выйдет, а не палит в холм (ADR-170).
                    let clear = match ground.as_ref() {
                        Some(g) => voxel::los::line_of_sight(&g.world, from, to).clear,
                        None => true,
                    };
                    if !clear {
                        *beams_blocked += 1;
                        continue;
                    }
                    *beams_fired += 1;
                    dmg_acc[tt] += w.damage as u32;
                    *beams_hit += 1;
                    events.push(SimEvent::Beam {
                        x0: from.0,
                        y0: from.1,
                        z0: from.2,
                        x1: to.0,
                        y1: to.1,
                        z1: to.2,
                        owner: u.owner[i],
                    });
                    u.next_shot[i] = tick + w.reload;
                }
                WeaponKind::Bolt => {
                    // Потолок на снаряды в воздухе. Упёршийся выстрел
                    // **не тратит перезарядку**: иначе юнит терял бы
                    // очередь из-за чужой стрельбы.
                    if shots.alive_count() >= cap {
                        *shots_refused += 1;
                        continue;
                    }
                    // Тиков ровно столько, чтобы пересечь дальность.
                    let life = ((range as i64 * TICKS_PER_SECOND as i64)
                        / w.speed.max(1) as i64
                        + 2)
                    .clamp(1, u16::MAX as i64) as u16;
                    let step = Fx::from_ratio(w.speed, TICKS_PER_SECOND);
                    let dx = Fx::from_int(to.0 - from.0);
                    let dy = Fx::from_int(to.1 - from.1);
                    let dz = Fx::from_int(to.2 - from.2);
                    let flat = dx.mul(dx).add(dz.mul(dz));
                    let d2 = flat.add(dy.mul(dy));
                    if d2.raw() == 0 {
                        continue;
                    }
                    // **Прицел с упреждением по высоте** (ADR-173).
                    // Снаряд падает всю дорогу, и целиться надо выше
                    // цели ровно на то, что он потеряет за полёт:
                    // `g*t*t/2`. Без этого каждый выстрел уходит
                    // в землю за несколько вокселей до цели.
                    //
                    // **Корня здесь нет.** Время полёта равно длине
                    // прицельного вектора на скорость за тик, а подъём
                    // квадратичен по времени — значит он зависит
                    // от **квадрата** длины, который считается без
                    // корня: `подъём = k * |прицел|²`, где
                    // `k = g / (2 * шаг²)` (ADR-071: корень —
                    // последнее средство).
                    let k = g_tick.div(step.mul(step).mul(Fx::from_int(2)));
                    // Подъём зависит от времени полёта, а время —
                    // от подъёма: это неподвижная точка, и берётся
                    // она простой итерацией от прямой наводки.
                    // При земном тяготении и полной дальности второй
                    // проход правит ответ на полтора процента, третий —
                    // на сотые доли.
                    let mut rise = k.mul(d2);
                    for _ in 0..AIM_ITERS {
                        let a = dy.add(rise);
                        let next = k.mul(flat.add(a.mul(a)));
                        // Итерация сходится, только пока цель стволу
                        // по силам. За этой границей она расходится,
                        // и продолжать значит целиться в зенит: берём
                        // последнее сошедшееся приближение.
                        if next.raw() > rise.raw().saturating_mul(2) {
                            break;
                        }
                        rise = next;
                    }
                    let ay = dy.add(rise);
                    // Направление считается по **поднятому** прицелу,
                    // и длина берётся от него же: нормировать на
                    // расстояние до цели значило бы разогнать снаряд
                    // тем сильнее, чем выше поднят ствол.
                    let len = flat.add(ay.mul(ay)).sqrt();
                    if len.raw() == 0 {
                        continue;
                    }
                    // Воронка — свойство **снаряда**, а не общей
                    // настройки: она задана оружием, а снаряд летит
                    // и переживает того, кто его выпустил.
                    shots.spawn(
                        from,
                        (Fx::ZERO, Fx::ZERO, Fx::ZERO),
                        (dx.div(len).mul(step), ay.div(len).mul(step), dz.div(len).mul(step)),
                        u.owner[i],
                        w.damage,
                        life,
                        (w.blast_radius, w.blast_damage),
                    );
                    *shots_fired += 1;
                    u.next_shot[i] = tick + w.reload;
                }
            }
        }
        // Урон луча применяется один раз, как и урон снарядов: порядок
        // стрелков не должен решать исход.
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let d = dmg_acc[i];
            if d == 0 {
                continue;
            }
            u.voxels[i] = u.voxels[i].saturating_sub(d.min(u16::MAX as u32) as u16);
        }
        shots.refresh_live();
    }

    /// Шаг снарядов: подвинуть, упереться в породу, попасть по юниту.
    ///
    /// Порода проверяется **целочисленным лучом** (`voxel::los`) — тем
    /// же, что и линия огня, и с той же канонизацией направления
    /// (ADR-113). Иначе снаряд пролетал бы там, где стрелок цели
    /// не видит.
    fn step_shots(&mut self) {
        let r2 = self.tuning.shot_hit_radius * self.tuning.shot_hit_radius;
        let height = self.tuning.unit_height as i32;
        // **Снаряд подчиняется тяготению** (ADR-173). Луча здесь нет
        // вовсе: он не летит, а приходит в тот же тик, и ускорять
        // у него нечего.
        let g_tick = gravity_per_tick(&self.tuning);
        self.dmg_acc.clear();
        self.dmg_acc.resize(self.units.x.len(), 0);
        let Self {
            units: u,
            shots,
            ground,
            grid_far,
            shots_hit,
            shots_blocked,
            dmg_acc,
            blasts,
            blast_voxels,
            blast_hits,
            events,
            ..
        } = self;
        for k in 0..shots.live_ids.len() {
            let si = shots.live_ids[k];
            let s = si as usize;
            if !shots.alive[s] {
                continue;
            }
            if shots.life[s] == 0 {
                shots.kill(si);
                continue;
            }
            shots.life[s] -= 1;

            let from = (shots.x[s], shots.y[s], shots.z[s]);
            // Сначала тяготение меняет скорость, потом скорость —
            // положение. Порядок фиксирован: обратный даёт другую
            // траекторию, то есть другой хеш.
            //
            // **Вычитание, а не сложение.** `vy` — скорость по оси
            // мира, а ось смотрит вверх; сложение разгоняло бы снаряд
            // в небо. У юнита и у тела скорость падения хранится
            // величиной, и знак там снимается вычитанием высоты.
            shots.vy[s] = shots.vy[s].sub(g_tick);
            let nx = shots.fx[s].add(shots.vx[s]);
            let ny = shots.fy[s].add(shots.vy[s]);
            let nz = shots.fz[s].add(shots.vz[s]);
            let (cx, cy, cz) = (nx.to_int_trunc(), ny.to_int_trunc(), nz.to_int_trunc());
            shots.fx[s] = nx.sub(Fx::from_int(cx));
            shots.fy[s] = ny.sub(Fx::from_int(cy));
            shots.fz[s] = nz.sub(Fx::from_int(cz));
            let to = (from.0 + cx, from.1 + cy, from.2 + cz);

            // Где рванёт, если рванёт.
            let mut hit_at: Option<(i32, i32, i32)> = None;
            // Порода на пути. Оба конца исключены, поэтому стрелок
            // не заслоняет сам себя на первом тике.
            if let Some(g) = ground.as_ref() {
                let (ex, ez) = g.extent();
                let ey = g.world.ny as i32;
                if to.0 < 0 || to.2 < 0 || to.0 >= ex || to.2 >= ez || to.1 < 0 || to.1 >= ey {
                    shots.kill(si);
                    continue;
                }
                let los = voxel::los::line_of_sight(&g.world, from, to);
                if !los.clear {
                    shots.x[s] = los.blocked_at.0;
                    shots.y[s] = los.blocked_at.1;
                    shots.z[s] = los.blocked_at.2;
                    *shots_blocked += 1;
                    shots.kill(si);
                    hit_at = Some(los.blocked_at);
                }
            }
            if hit_at.is_none() {
                shots.x[s] = to.0;
                shots.y[s] = to.1;
                shots.z[s] = to.2;
            }

            // Кого задело. Ничья разрешается **номером юнита**,
            // а не порядком обхода сетки.
            let mine = shots.owner[s];
            let (mut best, mut best_j) = (i32::MAX, NO_TARGET);
            if hit_at.is_none() {
            grid_far.for_each_near(to.0, to.2, |j| {
                let jj = j as usize;
                if !u.alive[jj] || u.owner[jj] == mine {
                    return;
                }
                if (u.y[jj] - to.1).abs() > height {
                    return;
                }
                let (dx, dz) = (u.x[jj] - to.0, u.z[jj] - to.2);
                let d2 = dx * dx + dz * dz;
                if d2 > r2 {
                    return;
                }
                if d2 < best || (d2 == best && j < best_j) {
                    best = d2;
                    best_j = j;
                }
            });
            }
            if best_j != NO_TARGET {
                let j = best_j as usize;
                dmg_acc[j] += shots.damage[s] as u32;
                *shots_hit += 1;
                shots.kill(si);
                hit_at = Some(to);
            }

            // Взрыв. Воронка идёт в **ту же очередь правок**, что копка:
            // потолок за тик и откладывание у них общие, и боевая правка
            // не имеет права обойти очередь (`CLAUDE.md`).
            let Some(at) = hit_at else { continue };
            let br = shots.blast_r[s];
            let (br2, blast_dmg) = (br * br, shots.blast_d[s]);
            if br <= 0 {
                continue;
            }
            *blasts += 1;
            events.push(SimEvent::Blast { x: at.0, y: at.1, z: at.2, radius: br });
            if let Some(g) = ground.as_mut() {
                let (ex, ey, ez) = (g.world.nx as i32, g.world.ny as i32, g.world.nz as i32);
                for dy in -br..=br {
                    for dz in -br..=br {
                        for dx in -br..=br {
                            if dx * dx + dy * dy + dz * dz > br2 {
                                continue;
                            }
                            let (x, y, z) = (at.0 + dx, at.1 + dy, at.2 + dz);
                            if x < 0 || y < 0 || z < 0 || x >= ex || y >= ey || z >= ez {
                                continue;
                            }
                            g.world.queue_dig(x as u32, y as u32, z as u32);
                            *blast_voxels += 1;
                        }
                    }
                }
            }
            // Взрывная волна не разбирает своих: урон всем в радиусе.
            grid_far.for_each_near(at.0, at.2, |j| {
                let jj = j as usize;
                if !u.alive[jj] {
                    return;
                }
                let (dx, dz) = (u.x[jj] - at.0, u.z[jj] - at.2);
                let dy = u.y[jj] - at.1;
                if dx * dx + dy * dy + dz * dz > br2 {
                    return;
                }
                dmg_acc[jj] += blast_dmg as u32;
                *blast_hits += 1;
            });
        }
        // Урон применяется **один раз**, в конце фазы: иначе исход
        // зависел бы от порядка снарядов, а не от множества попаданий.
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let d = dmg_acc[i];
            if d == 0 {
                continue;
            }
            u.voxels[i] = u.voxels[i].saturating_sub(d.min(u16::MAX as u32) as u16);
        }
        shots.refresh_live();
    }

    /// Видимость: здесь заглушка с реалистичной формой доступа —
    /// настоящая мерилась отдельно (ADR-070, ADR-072).
    fn phase_visibility(&mut self) {}

    /// Уборка: мёртвые уходят в free-list.
    fn phase_cleanup(&mut self) {
        let mut dead: Vec<u32> = Vec::new();
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k];
            if self.units.voxels[i as usize] == 0 {
                dead.push(i);
            }
        }
        self.deaths += dead.len() as u64;
        for i in dead {
            let k = i as usize;
            self.events.push(SimEvent::Death {
                unit: i,
                x: self.units.x[k],
                y: self.units.y[k],
                z: self.units.z[k],
            });
            self.units.kill(i);
        }
    }

    /// Хеш состояния. Только `wrapping_add` и `wrapping_mul`: единственная
    /// ассоциативная пара, безопасная для параллельной редукции
    /// (`CLAUDE.md`).
    fn phase_hash(&mut self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k] as usize;
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(self.units.x[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.z[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.fx[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.fz[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.voxels[i] as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.y[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.riding[i]);
            w = w.wrapping_mul(31).wrapping_add(self.units.target[i] as u64);
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        // Снаряды — состояние, а не украшение: не войди они в хеш,
        // полёт можно было бы переписать целиком и увидеть тот же
        // хеш до бита (ADR-116).
        for k in 0..self.shots.live_ids.len() {
            let i = self.shots.live_ids[k] as usize;
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(self.shots.x[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.shots.y[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.shots.z[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.shots.fx[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.shots.fz[i].raw() as u64);
            // Вертикаль хешируется наравне с горизонталью: пока снаряд
            // летел по прямой, `fy` не менялась и её отсутствие ничего
            // не стоило. С тяготением вертикальная скорость — состояние,
            // которое эволюционирует само, и не войди она в хеш,
            // интегрирование можно было бы переписать и увидеть тот же
            // хеш до бита (ADR-116, ADR-173).
            w = w.wrapping_mul(31).wrapping_add(self.shots.fy[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.shots.vy[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.shots.life[i] as u64);
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        if let Some(g) = &self.ground {
            h = h.wrapping_add(g.hash());
        }
        h
    }
}

/// Заселение мира: две армии, каждая идёт на позицию другой.
/// Эталонный сценарий: две линии лицом к лицу, идущие навстречу.
///
/// **Стороны обязаны сойтись, и сойтись быстро.** Первая редакция
/// ставила их по углам карты, в 1600 клетках друг от друга. Скорость
/// задана в клетках **в секунду**, шаг делится на тридцать, значит за
/// тысячу тиков — тридцать три секунды — сторона проходит сто тридцать
/// клеток из тысячи шестисот. Они не встречались никогда: пять тысяч
/// живых после тысячи тиков, ноль целей, ни одной смерти.
///
/// Цена ошибки не в самом сценарии, а в том, что на нём стоит
/// golden-хеш. Фаза боя выполнялась каждый тик, всегда впустую, и
/// **её поведение не входило в хеш вовсе** — правку прицеливания
/// можно было внести и увидеть тот же хеш до бита (ADR-116).
///
/// Фронты ставятся ровно на дальность выстрела: бой начинается
/// с первых тиков, и хеш покрывает его даже на коротком прогоне.
/// Сваливать стороны в одну кучу нельзя — свалка это отдельный
/// случай с другой ценой (`--dense` в D-7).
/// Поставить одного юнита в строй своей стороны.
///
/// Живёт отдельно, потому что мест расстановки два: начальная
/// и подкрепление (ADR-161). Разойдись они — подкрепление приходило бы
/// не туда, где стоял строй, и сцена перестала бы быть той же сценой.
fn spawn_in_line(w: &mut World, r: &mut Rng, owner: u8) -> u32 {
    let (ex, ez) = w.ground().map(|g| g.extent()).unwrap_or((WORLD, WORLD));
    let (midx, midz) = (ex / 2, ez / 2);
    let gap = w.tuning.range / 2;
    let spread = (ex / 6).clamp(4, 100);
    let depth = (ez / 4).clamp(4, 150);
    let bx = if owner == 0 {
        midx - gap - spread
    } else {
        midx + gap + spread
    };
    let x = (bx + r.range(-spread, spread)).clamp(0, ex - 1);
    let z = (midz + r.range(-depth, depth)).clamp(0, ez - 1);
    w.spawn_ground(x, z, owner, r.range(2, 6), 300)
}

pub fn populate(w: &mut World, n: usize, seed: u64) {
    let mut r = Rng::new(seed, 0xA1);
    // Если земля есть, сетка у мира и у неё **одна**: юниты стоят
    // в воксельных координатах, а не на отвлечённой плоскости
    // (ADR-134). Без земли — прежний размах.
    let (ex, ez) = w.ground().map(|g| g.extent()).unwrap_or((WORLD, WORLD));
    let (midx, midz) = (ex / 2, ez / 2);
    let gap = w.tuning.range / 2;
    let spread = (ex / 6).clamp(4, 100);
    for k in 0..n {
        let _ = spawn_in_line(w, &mut r, (k % 2) as u8);
    }
    w.fill_flow_towards(0, midx + gap + spread, midz);
    w.fill_flow_towards(1, midx - gap - spread, midz);
    w.units.refresh_live();
}

/// Сторона мира эталонного сценария.
///
/// Меньше умолчания генератора (512³): гейт запускают руками и в CI,
/// и он обязан оставаться быстрым. Больше сектора навигации во много
/// раз, иначе иерархия в нём не участвует.
pub const REF_SIDE: u32 = 256;

/// Сколько копателей в эталонном сценарии и как глубоко они грызут
/// за тик. Копка — экономика проекта, и сценарий, который не копает,
/// не проверяет главного.
const REF_DIGGERS: usize = 24;
const REF_RADIUS: i32 = 2;

/// Сколько юнитов сторона добирает за тик, пока не вернулась
/// к заявленной численности.
const REF_REINFORCE: usize = 4;

/// Мир эталонного сценария — **в одном месте**.
///
/// Высота рельефа задаётся абсолютными вокселями, а `surface_height`
/// обрезает её по высоте мира: `clamp(8, ny - 8)`. Умолчание генератора
/// (`base_height` 140 при `ny` 256) в мире вдвое ниже упирается
/// в обрезку **каждой колонкой** — карта выходит плоской, размах восемь
/// вокселей вместо восьмидесяти, и навигация по объёму на ней не
/// нагружается вовсе (ADR-157).
///
/// Плоской карта становится молча: ошибку видно только замером размаха,
/// а он ровно на границе обрезки — то самое значение, совпавшее точно,
/// которое обязано вызывать подозрение.
pub fn ref_params() -> worldgen::Params {
    worldgen::Params::sized(REF_SIDE, REF_SIDE / 2, REF_SIDE)
}

/// **Эталонный сценарий — один на всех:** и на `sim-cli`, и на тесты
/// детерминизма.
///
/// Живёт здесь, а не в `main.rs`, по той же причине, по которой написан
/// этот комментарий. Пока сценарий собирался в `sim-cli`, тесты гоняли
/// мир **без земли** — и гейт печатал хеш того же мира без земли. Пять
/// фаз тика из шестнадцати не входили в хеш вовсе: правки вокселей,
/// разрушение, тела, навигация и поле направлений (ADR-157). Это второй
/// случай той же болезни после ADR-116.
///
/// Лечится он не тестом на фазу, а тем, что **прогнать сценарий можно
/// ровно одним способом** — через [`Reference::tick`]. Копка идёт
/// внутри него: разнеси её по вызывающим, и они разойдутся снова.
pub struct Reference {
    pub world: World,
    /// Сторона мира: копатели переселяются по ней, а мир у показа
    /// и у гейта разный (ADR-172).
    side: u32,
    /// Заявленная численность стороны. Бой затухает без подкреплений,
    /// и прогон на десять тысяч тиков восемь тысяч из них хеширует
    /// почти пустое поле (ADR-161).
    per_side: usize,
    /// Генератор нужен и дальше: копатель, догрызший свой столб,
    /// переходит на новый и начинает с поверхности.
    terrain: worldgen::Terrain,
    /// Копатели: где грызут и куда двинутся дальше.
    diggers: Vec<(i32, i32, i32)>,
    rng: Rng,
}

impl Reference {
    /// Стороны вооружены **по-разному**: нулевая болтом, первая лучом.
    ///
    /// Одинаковое оружие у обеих сторон не проверяет разбора по виду:
    /// ветка луча могла бы не вызываться ни разу, и хеш этого
    /// не заметил бы (ADR-169).
    pub fn new(
        seed: u64,
        units: usize,
        tuning: Tuning,
        weapons: modding::weapons::Weapons,
    ) -> Reference {
        Reference::with_params(seed, units, tuning, weapons, ref_params())
    }

    /// Тот же сценарий на **другом** мире.
    ///
    /// Гейту нужен один мир — на нём стоит хеш, и менять его нельзя.
    /// Показу нужен другой: поплоще, чтобы бой было видно, и побольше,
    /// чтобы строй не стоял в куче. Развести их параметрами дешевле,
    /// чем заводить второй сценарий (ADR-172).
    pub fn with_params(
        seed: u64,
        units: usize,
        tuning: Tuning,
        weapons: modding::weapons::Weapons,
        params: worldgen::Params,
    ) -> Reference {
        let terrain = worldgen::Terrain { params, seed };
        let vw = voxel::World::new(
            params.nx,
            params.ny,
            params.nz,
            1 << 16,
            Box::new(worldgen::Terrain { params, seed }),
        );
        let g = ground::Ground::new(vw, &tuning);
        let mut world = World::new(2, units.max(1), tuning);
        world.attach_ground(g);
        world.attach_weapons(weapons);
        populate(&mut world, units, seed);
        // Сторона выбирает оружие по своему номеру: разводить их
        // жребием значило бы поставить разбор по виду в зависимость
        // от генератора случайных чисел.
        let kinds = world.weapons.len().max(1);
        for k in 0..world.units.len() {
            world.units.weapon[k] = (world.units.owner[k] as usize % kinds) as u16;
        }

        // Копатели ставятся на поверхность, вразнос по карте: копка
        // в одной точке — это другой случай с другой ценой.
        let mut rng = Rng::new(seed, 0xD16);
        let mut diggers = Vec::with_capacity(REF_DIGGERS);
        for _ in 0..REF_DIGGERS {
            let x = rng.below(params.nx) as i32;
            let z = rng.below(params.nz) as i32;
            let y = terrain.surface(x as u32, z as u32) as i32;
            diggers.push((x, y, z));
        }
        Reference {
            world,
            side: params.nx,
            per_side: units.max(1).div_ceil(2),
            terrain,
            diggers,
            rng,
        }
    }

    /// Тик сценария: копка, подкрепление, потом сам тик.
    pub fn tick(&mut self) -> u64 {
        self.dig();
        self.reinforce();
        self.world.tick()
    }

    /// То же на нескольких воркерах. Число воркеров задаётся явно
    /// и в состояние не входит: хеш обязан совпасть с однопоточным.
    pub fn tick_par(&mut self, threads: usize) -> u64 {
        self.dig();
        self.reinforce();
        self.world.tick_par(threads)
    }

    /// Сторона добирает потери до заявленной численности.
    ///
    /// Без этого бой кончается: к двухтысячному тику живых впятеро
    /// меньше начального, к десятитысячному — полсотни, и восемь тысяч
    /// тиков из десяти фаза боя работает вхолостую. Это то же самое,
    /// от чего лечил ADR-116, только растянутое во времени (ADR-161).
    ///
    /// Подкрепление идёт **струйкой**, а не залпом: залп по достижении
    /// порога дал бы пилу, а струйка держит численность ровно.
    fn reinforce(&mut self) {
        let mut live = [0usize; 2];
        for &i in self.world.units.live() {
            live[(self.world.units.owner[i as usize] & 1) as usize] += 1;
        }
        let mut added = false;
        // Стороны обходятся по номеру, а не по числу потерь: порядок
        // расстановки обязан быть от данных, а не от хода боя.
        for owner in 0..2u8 {
            let have = live[owner as usize];
            let kinds = self.world.weapons.len().max(1);
            for _ in 0..REF_REINFORCE.min(self.per_side.saturating_sub(have)) {
                let id = spawn_in_line(&mut self.world, &mut self.rng, owner);
                // Подкрепление вооружается как своя сторона, иначе
                // строй со временем перемешал бы оружие.
                self.world.units.weapon[id as usize] = (owner as usize % kinds) as u16;
                added = true;
            }
        }
        if added {
            self.world.units.refresh_live();
        }
    }

    /// Каждый копатель снимает шарик породы и уходит вглубь.
    ///
    /// Догрызший свой столб **переходит на новый**, а не начинает тот же
    /// сверху. Иначе копка кончается: за тысячу тиков двадцать четыре
    /// столба выбираются насквозь, дальше шарик снимает воздух, правок
    /// не возникает — и прогон на десять тысяч тиков девять десятых
    /// времени хеширует неподвижный мир (ADR-159).
    fn dig(&mut self) {
        let Some(g) = self.world.ground_mut() else { return };
        let (nx, ny, nz) = (g.world.nx as i32, g.world.ny as i32, g.world.nz as i32);
        let r = REF_RADIUS;
        let mut moved: Vec<usize> = Vec::new();
        for (k, d) in self.diggers.iter_mut().enumerate() {
            for dy in -r..=r {
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy + dz * dz > r * r {
                            continue;
                        }
                        let (x, y, z) = (d.0 + dx, d.1 + dy, d.2 + dz);
                        if x < 0 || y < 0 || z < 0 || x >= nx || y >= ny || z >= nz {
                            continue;
                        }
                        g.world.queue_dig(x as u32, y as u32, z as u32);
                    }
                }
            }
            d.1 -= 1;
            if d.1 <= 1 {
                moved.push(k);
            }
        }
        // Переселение — после обхода: земля занята заимствованием,
        // а порядок остаётся порядком в массиве, а не обхода.
        for k in moved {
            let x = self.rng.below(self.side) as i32;
            let z = self.rng.below(self.side) as i32;
            let y = self.terrain.surface(x as u32, z as u32) as i32;
            self.diggers[k] = (x, y, z);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Эталонный сценарий обязан **действительно воевать**.
    ///
    /// Это не проверка боя, а проверка сценария: пока стороны стояли
    /// в разных углах, фаза боя работала вхолостую и в golden-хеш
    /// не входила. Тест сторожит именно это — что хеш что-то покрывает
    /// (ADR-116).
    #[test]
    fn the_reference_scenario_actually_fights() {
        let mut r = Reference::new(42, 2000, test_tuning(), test_weapons());
        let before = r.world.units.live().len();
        let mut ever_targeted = false;
        for _ in 0..400 {
            r.tick();
            ever_targeted |= r
                .world
                .units
                .live()
                .iter()
                .any(|&i| r.world.units.target[i as usize] != NO_TARGET);
        }
        assert!(ever_targeted, "никто ни разу не взял цель");
        // Смерти считаются прямо. Убыль живых мерилом быть перестала,
        // когда появились подкрепления: они держат численность ровной,
        // и выбитый строй по ней неотличим от нетронутого (ADR-161).
        assert!(
            r.world.deaths > 0,
            "никто не погиб: живых {} из {before}",
            r.world.units.live().len()
        );
        assert!(r.world.examined_combat > 0);
        // Снаряды долетают. Стрельба, которая ни во что не попадает,
        // это не бой (ADR-164).
        assert!(
            r.world.shots_fired > 0 && r.world.shots_hit > 0,
            "выстрелов {}, попаданий {}",
            r.world.shots_fired,
            r.world.shots_hit
        );
    }

    /// **Эталонный сценарий обязан трогать землю.**
    ///
    /// Сестра теста на бой и та же болезнь: пока сценарий собирался
    /// в `sim-cli` без земли, пять фаз тика из шестнадцати работали
    /// вхолостую и в golden-хеш не входили вовсе — правки вокселей,
    /// разрушение, тела, навигация, поле направлений. Переписать любую
    /// из них можно было не меняя хеша (ADR-157).
    ///
    /// Тест проверяет не копку, а **сценарий**: что хеш что-то покрывает.
    #[test]
    fn the_reference_scenario_actually_works_the_ground() {
        let mut r = Reference::new(42, 500, test_tuning(), test_weapons());
        assert!(r.world.ground().is_some(), "у эталона нет земли");
        let (mut edits, mut topo, mut falls) = (0u64, 0u64, 0u64);
        for _ in 0..200 {
            r.tick();
            let g = r.world.ground().expect("земля пропала");
            edits += g.stats.edits as u64;
            topo += g.stats.topo_dirty as u64;
            falls += g.stats.fall_damage;
        }
        assert!(edits > 0, "правок вокселей ноль — фаза копки вхолостую");
        assert!(topo > 0, "чанков связности ноль — разрушение вхолостую");
        assert!(falls > 0, "урона от падения ноль — фаза тел вхолостую");
        // Навигация: фаза не имеет права быть мгновенной, если она
        // что-то делает. Нулевое время — признак заглушки.
        assert!(
            r.world.phase_ns_of(Phase::NavUpdate) > 0,
            "навигация не заняла времени вовсе"
        );
    }

    /// Потолок поисков не имеет права зависеть от числа воркеров:
    /// он часть состояния, а не расписания.
    #[test]
    fn the_search_cap_does_not_change_with_worker_count() {
        let run = |threads: usize| {
            let mut w = World::new(2, 2000, test_tuning());
            populate(&mut w, 2000, 7);
            let mut h = 0;
            for _ in 0..120 {
                h = if threads > 1 { w.tick_par(threads) } else { w.tick() };
            }
            (h, w.searches, w.units.live().len())
        };
        assert_eq!(run(1), run(4));
        assert_eq!(run(1), run(8));
    }

    /// Один сид — один хеш. Базовое свойство, без которого нет ничего.
    #[test]
    fn same_seed_same_hash() {
        let run = || {
            let mut r = Reference::new(42, 2000, test_tuning(), test_weapons());
            let mut h = 0u64;
            for _ in 0..60 {
                h = r.tick();
            }
            h
        };
        assert_eq!(run(), run());
    }

    /// **Юнит никогда не оказывается внутри непроходимого воксела.**
    ///
    /// Главный инвариант навигации (`docs/06-navigation.md`). Формулируется
    /// **только на маске проходимости**: она обновляется в тот же тик,
    /// а поле направлений отстаёт намеренно, и на нём инварианта нет.
    ///
    /// Здесь проверяется **падение**. Поездка — тоже законное
    /// исключение, но её проверяет `a_rider_is_not_inside_solid_rock`
    /// на построенном случае: выпрашивать редкое событие у большой
    /// сцены оказалось хрупко — сторож ломался от каждой правки
    /// баланса, трижды подряд (ADR-172).
    ///
    /// Исключений из канона применимо два.
    ///
    /// **Падение на исчезнувшей опоре.** Юнит, под которым выкопали,
    /// летит вниз — и в этот момент законно находится там, где опоры нет.
    ///
    /// **Поездка на отделившемся куске** (ADR-099). Наездник стоит
    /// на теле, а не на рельефе, и уровня под ним нет по построению.
    /// В перечне канона этого случая не было: перечень писался до
    /// ADR-099, а поездок в эталоне не случалось ни одной, пока
    /// не появились воронки (ADR-165).
    ///
    /// Прочие исключения канона (раздавливание телом, оседание материи,
    /// воздушный слой, граница щита) относятся к механикам этапа 6.
    ///
    /// Сцена обязана нагружать то, что заявляет: копают под ногами,
    /// иначе инвариант проверяется на неподвижной земле и не значит
    /// ничего. Поэтому берётся эталон — он копает (ADR-159).
    #[test]
    fn a_unit_is_never_inside_solid_rock() {
        let mut r = Reference::new(42, 600, test_tuning(), test_weapons());
        let mut ever_falling = 0u64;
        let mut checked = 0u64;
        // Четыреста тиков, а не двести: с тех пор как половина строя
        // перешла на луч (ADR-169), копают вдвое меньше, и за двести
        // тиков не случается ни одной поездки — то есть второе
        // исключение не проверяется вовсе. Замер: 200 тиков дают 0
        // поездок, 400 дают 8.
        for t in 1..=400 {
            r.tick();
            let g = r.world.ground().expect("земля");
            let st = g.nav.step() as i32;
            let (gw, gh) = g.nav.dims();
            for &i in r.world.units.live() {
                let i = i as usize;
                let (ux, uz, uy) = (r.world.units.x[i], r.world.units.z[i], r.world.units.y[i]);
                let (cx, cz) = ((ux / st).max(0) as u32, (uz / st).max(0) as u32);
                if cx >= gw || cz >= gh {
                    continue;
                }
                checked += 1;
                if r.world.units.fall_from[i] != NOT_FALLING {
                    ever_falling += 1;
                    continue;
                }
                // Наездник законно вне рельефа: он на теле.
                // Проверяет это `a_rider_is_not_inside_solid_rock`.
                if r.world.units.riding[i] != 0 {
                    continue;
                }
                assert!(
                    g.nav.has_level(cx, cz, uy),
                    "тик {t}, юнит {i} в ({ux}, {uy}, {uz}) — клетка ({cx}, {cz}) \
не имеет уровня {uy}, не падает, едет на теле {}: уровни {:?}",
                    r.world.units.riding[i],
                    g.nav.levels_at(cx, cz)
                );
            }
        }
        assert!(checked > 100_000, "проверено всего {checked} положений");
        assert!(
            ever_falling > 0,
            "никто ни разу не падал — сцена не копает под ногами, \
и инвариант проверен на неподвижной земле"
        );
    }

    /// **Копка не кончается к середине прогона.**
    ///
    /// Сторож против затухания сценария. Первая редакция копателя грызла
    /// свой столб сверху вниз и, дойдя до дна, начинала тот же столб
    /// заново — по уже выкопанному. Правки шли первые шестьдесят тиков,
    /// дальше шарик снимал воздух, и прогон на десять тысяч тиков
    /// хешировал неподвижный мир (ADR-159).
    ///
    /// Проверять «копка была» недостаточно: она была. Проверять надо,
    /// что она **есть во второй половине**.
    #[test]
    fn the_reference_keeps_digging_to_the_end() {
        let mut r = Reference::new(42, 300, test_tuning(), test_weapons());
        let mut half = [0u64; 2];
        for t in 0..400 {
            r.tick();
            let g = r.world.ground().expect("земля");
            half[usize::from(t >= 200)] += g.stats.edits as u64;
        }
        assert!(half[0] > 0, "копки не было и в первой половине");
        assert!(
            half[1] * 4 > half[0],
            "копка затухла: {} правок в первой половине против {} во второй",
            half[0],
            half[1]
        );
    }

    /// **Бой не кончается к середине прогона.**
    ///
    /// Сестра сторожа копки. Без подкреплений строй выбивается: к
    /// двухтысячному тику живых впятеро меньше начального, к
    /// десятитысячному — полсотни, и восемь тысяч тиков из десяти фаза
    /// боя работает вхолостую (ADR-161).
    ///
    /// Проверять «бой был» бесполезно — он был. Проверяется, что он
    /// **есть во второй половине**, и что строй не выбит.
    #[test]
    fn the_reference_keeps_fighting_to_the_end() {
        let mut r = Reference::new(42, 600, test_tuning(), test_weapons());
        for _ in 0..200 {
            r.tick();
        }
        let mid = r.world.examined_combat;
        let alive_mid = r.world.units.live().len();
        for _ in 0..200 {
            r.tick();
        }
        let late = r.world.examined_combat - mid;
        let alive_end = r.world.units.live().len();
        assert!(mid > 0 && late > 0, "боя не было: {mid} и {late} осмотров по половинам");
        // Настоящий признак затухания — **численность**, а не число
        // осмотров: на коротком прогоне редеющий строй ищет цели даже
        // усерднее, и по осмотрам затухание неотличимо от боя.
        // Допуск два процента: подкрепление держит численность ровно,
        // но упереться порогом в само равенство значило бы падать
        // от одной лишней смерти. Проседание без подкрепления — 11%.
        assert!(
            alive_end * 50 >= alive_mid * 49,
            "строй выбивается: {alive_mid} живых на середине, {alive_end} в конце — подкрепление не догоняет потери, и к концу долгого прогона бой кончится"
        );
    }

    /// Плоская земля до `FLAT_GY` и ничего больше: сцена для баллистики,
    /// где мешать снаряду может только тяготение.
    struct Flat;
    const FLAT_GY: u32 = 8;
    impl voxel::Generator for Flat {
        fn material(&self, _x: u32, y: u32, _z: u32) -> voxel::store::Mat {
            if y < FLAT_GY {
                2
            } else {
                voxel::store::AIR
            }
        }
    }

    /// Двое на плоскости, лицом к лицу, на заданном расстоянии.
    fn duel(t: Tuning, ws: modding::weapons::Weapons, apart: i32) -> World {
        let vw = voxel::World::new(64, 48, 64, 4096, Box::new(Flat));
        let mut w = World::new(2, 8, t);
        w.attach_ground(ground::Ground::new(vw, &t));
        w.attach_weapons(ws);
        w.spawn_ground(32 - apart / 2, 32, 0, 0, 300);
        w.spawn_ground(32 + apart / 2, 32, 1, 0, 300);
        w.units.refresh_live();
        w
    }

    fn bolt(speed: i32) -> modding::weapons::Weapons {
        modding::weapons::from_ron(&format!(
            "( weapons: [ ( id: 1, kind: Bolt, speed: {speed}, damage: 30, reload: 30, \
blast_radius: 0, blast_damage: 0 ) ] )"
        ))
        .expect("реестр")
    }

    /// **Снаряд летит по дуге, а не по прямой** (ADR-173).
    ///
    /// До этого тяготения у снаряда не было вовсе: канон обещал падение
    /// с ускорением телам (`04-world.md`), и ADR-164 прямо записал,
    /// что снаряду придётся повторить.
    ///
    /// Проверяется вертикальная скорость одного снаряда за весь полёт:
    /// у ствола она **положительна** — то есть ствол поднят, упреждение
    /// есть, — дальше строго убывает и к концу уходит в минус, то есть
    /// снаряд прошёл вершину и падает.
    ///
    /// Скорость, а не высота: при земном тяготении, скорости сорок
    /// клеток в секунду и дальности в двадцать клеток вершина дуги
    /// поднимается над стрелком на **0.6 воксела** — в целых
    /// координатах её попросту не видно. Выбрать ради видимости
    /// тяготение побольше значило бы проверять не ту сцену, в которую
    /// играют.
    #[test]
    fn a_bolt_flies_an_arc_and_the_aim_leads_by_height() {
        let mut w = duel(test_tuning(), bolt(40), 20);
        // Следить надо за **одним** снарядом от рождения до гибели:
        // сводка по всем живым лжёт, потому что свежий снаряд
        // начинает полёт заново.
        let mut track: Vec<Fx> = Vec::new();
        for _ in 0..60 {
            w.tick();
            if w.shots.is_empty() || !w.shots.alive[0] {
                if !track.is_empty() {
                    break;
                }
                continue;
            }
            track.push(w.shots.vy[0]);
        }
        assert!(w.shots_fired > 0, "никто не стрелял");
        assert!(track.len() > 3, "снаряд прожил {} тиков", track.len());
        assert!(
            track[0].raw() > 0,
            "ствол не поднят: скорость у дула {:?}, то есть упреждения нет",
            track[0]
        );
        for k in 1..track.len() {
            assert!(
                track[k].raw() < track[k - 1].raw(),
                "вертикальная скорость не убывает — тяготение не действует: {track:?}"
            );
        }
        assert!(
            track[track.len() - 1].raw() < 0,
            "снаряд так и не начал падать: {track:?}"
        );
        assert!(w.shots_hit > 0, "дуга есть, а попаданий нет");
    }

    /// **Без упреждения по высоте бой умирает.** Снаряд, пущенный прямо
    /// в цель, теряет `g*t*t/2` за дорогу и на полной дальности уходит
    /// в землю за несколько вокселей до неё.
    ///
    /// Тяготение здесь втрое земного — оснастка, а не баланс: она
    /// доводит потерю высоты до величины, которую допуск попадания
    /// по высоте (рост юнита) заведомо не покрывает. С упреждением
    /// попадания есть; без него их ноль, и это проверено снятием
    /// упреждения.
    #[test]
    fn the_aim_leads_enough_to_reach_the_far_edge_of_the_range() {
        let mut t = test_tuning();
        t.gravity = 60;
        let apart = t.range - 2;
        let mut w = duel(t, bolt(40), apart);
        for _ in 0..200 {
            w.tick();
        }
        assert!(w.shots_fired > 0, "никто не стрелял");
        assert!(
            w.shots_hit > 0,
            "с {} выстрелов ни одного попадания на дальности {apart}: \
снаряд уходит в землю, не долетев",
            w.shots_fired
        );
    }

    /// **Луч тяготению не подчиняется** (ADR-173): он не снаряд, он
    /// приходит в тот же тик по прямой линии огня, и ускорять у него
    /// нечего.
    ///
    /// Проверяется сильнее, чем «попадания есть»: сцена с одними лучами
    /// прогоняется при земном тяготении и при стократном, и **хеш тика
    /// обязан совпасть до бита**. Совпадение значит, что от тяготения
    /// в этой сцене не зависит ничего — ни луч, ни то, что он задевает.
    #[test]
    fn gravity_does_not_touch_the_laser() {
        let laser = || {
            modding::weapons::from_ron(
                "( weapons: [ ( id: 2, kind: Laser, speed: 0, damage: 12, reload: 15, \
blast_radius: 0, blast_damage: 0 ) ] )",
            )
            .expect("реестр")
        };
        let run = |gravity: i32| -> (u64, u64, u64) {
            let mut t = test_tuning();
            t.gravity = gravity;
            let mut w = duel(t, laser(), 20);
            let mut h = 0;
            for _ in 0..120 {
                h = w.tick();
            }
            (h, w.beams_fired, w.beams_hit)
        };
        let earth = run(test_tuning().gravity);
        let heavy = run(test_tuning().gravity * 100);
        assert!(earth.2 > 0, "лучи не попадали ни разу — сцена ничего не проверяет");
        assert_eq!(
            earth, heavy,
            "стократное тяготение изменило сцену из одних лучей: \
хеш, выстрелы и попадания {earth:?} против {heavy:?}"
        );
    }

    /// **Гора закрывает, прокопанный ход открывает.**
    ///
    /// Главное, ради чего снаряд стал объектом: раньше урон приходил
    /// мгновенно и сквозь породу. Канон обещает обратное с самого
    /// начала — «линия огня учитывает воксельную геометрию: холм
    /// закрывает, и это меняется, когда холм срыли» (`05-units.md`).
    ///
    /// Тест ставит двоих по разные стороны стены и требует **обоих**
    /// исходов: со стеной ни одного попадания, с проходом — попадания
    /// есть. Проверять только первое бесполезно: стрельба, которая
    /// не попадает никогда, тоже даёт ноль.
    #[test]
    fn a_ridge_stops_the_shot_and_a_tunnel_lets_it_through() {
        /// Земля до `GY` и стена во всю карту по `x == WALL`.
        ///
        /// Стена **толстая**: воронки прогрызают породу (ADR-165),
        /// и на тонкой стене тест проверял бы не «порода закрывает»,
        /// а «пробить её за двести тиков не успели». Толщина берётся
        /// с запасом на обстрел, и целость стены проверяется в конце.
        struct Ridge;
        const GY: u32 = 8;
        const WALL: u32 = 32;
        const HALF: u32 = 6;
        impl voxel::Generator for Ridge {
            fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
                if y < GY {
                    return 2;
                }
                if x.abs_diff(WALL) <= HALF && y < GY + 12 {
                    return 3;
                }
                voxel::store::AIR
            }
        }

        // `tunnel` — прокопать в стене ход на высоте пола.
        // `blast` — радиус воронки; ноль означает «проверяем только луч».
        let run = |tunnel: bool, blast: i32| -> (u64, u64, u64, bool) {
            let t = test_tuning();
            // Воронка задаётся **оружием**, а не настройкой мира
            // (ADR-169), поэтому реестр собирается под нужный радиус.
            let ws = modding::weapons::from_ron(&format!(
                "( weapons: [ ( id: 1, kind: Bolt, speed: 40, damage: 30, reload: 30, \
blast_radius: {blast}, blast_damage: 10 ) ] )"
            ))
            .expect("реестр");

            let mut vw = voxel::World::new(64, 48, 64, 4096, Box::new(Ridge));
            if tunnel {
                for x in WALL - HALF..=WALL + HALF {
                    for y in GY..GY + 6 {
                        for z in 28..36 {
                            vw.queue_dig(x, y, z);
                        }
                    }
                }
                while vw.queued() > 0 {
                    vw.begin_tick();
                    vw.flush();
                }
            }
            let g = ground::Ground::new(vw, &t);
            let mut w = World::new(2, 8, t);
            w.attach_ground(g);
            w.attach_weapons(ws);
            // По разные стороны стены, на одной высоте и в створе хода.
            w.spawn_ground(WALL as i32 - 8, 32, 0, 0, 300);
            w.spawn_ground(WALL as i32 + 8, 32, 1, 0, 300);
            w.units.refresh_live();
            for _ in 0..200 {
                w.tick();
            }
            // Стена обязана достоять: иначе «ноль попаданий» значит
            // лишь «не успели пробить».
            let g = w.ground().expect("земля");
            let intact = !tunnel
                && g.world.solid(WALL, GY + 1, 32)
                && g.world.solid(WALL, GY + 3, 32);
            (w.shots_fired, w.shots_hit, w.shots_blocked, intact)
        };

        // Воронка выключена: проверяется ровно луч, а не то, успели ли
        // пробить стену.
        let (fired_wall, hit_wall, blocked_wall, intact) = run(false, 0);
        assert!(fired_wall > 0, "со стеной никто не стрелял вовсе");
        assert!(intact, "стена не устояла без воронок — тест проверяет не то");
        assert_eq!(
            hit_wall, 0,
            "сквозь стену прошло {hit_wall} попаданий из {fired_wall} выстрелов"
        );
        assert!(
            blocked_wall > 0,
            "ни один снаряд не упёрся в стену: {fired_wall} выстрелов"
        );

        let (fired_open, hit_open, _, _) = run(true, 0);
        assert!(
            hit_open > 0,
            "через прокопанный ход не прошло ни одного попадания \
из {fired_open} выстрелов — сцена не проверяет ничего"
        );
    }

    /// **Воронки проламывают стену, сквозь которую луч не проходил.**
    ///
    /// Обратная сторона предыдущего теста и главное следствие воронок
    /// (ADR-165): порода закрывает не навсегда. Стена в тринадцать
    /// вокселей, которую снаряд без воронки не пробивает ни разу
    /// за двести тиков, с воронками перестаёт быть стеной.
    ///
    /// Тест сторожит **обе** стороны: без воронок ноль попаданий,
    /// с воронками попадания появляются. Одна сторона без другой
    /// ничего не значит.
    #[test]
    fn a_blast_breaches_a_wall_a_bullet_cannot() {
        struct Wall;
        const GY: u32 = 8;
        const AT: u32 = 32;
        const HALF: u32 = 6;
        impl voxel::Generator for Wall {
            fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
                if y < GY {
                    return 2;
                }
                if x.abs_diff(AT) <= HALF && y < GY + 12 {
                    return 3;
                }
                voxel::store::AIR
            }
        }
        let run = |blast: i32| -> (u64, bool) {
            let t = test_tuning();
            // Воронка задаётся **оружием**, а не настройкой мира
            // (ADR-169), поэтому реестр собирается под нужный радиус.
            let ws = modding::weapons::from_ron(&format!(
                "( weapons: [ ( id: 1, kind: Bolt, speed: 40, damage: 30, reload: 30, \
blast_radius: {blast}, blast_damage: 10 ) ] )"
            ))
            .expect("реестр");

            let vw = voxel::World::new(64, 48, 64, 4096, Box::new(Wall));
            let g = ground::Ground::new(vw, &t);
            let mut w = World::new(2, 8, t);
            w.attach_ground(g);
            w.attach_weapons(ws);
            w.spawn_ground(AT as i32 - 8, 32, 0, 0, 300);
            w.spawn_ground(AT as i32 + 8, 32, 1, 0, 300);
            w.units.refresh_live();
            for _ in 0..200 {
                w.tick();
            }
            let solid = w.ground().expect("земля").world.solid(AT, GY + 1, 32);
            (w.shots_hit, solid)
        };

        let (hit_plain, solid_plain) = run(0);
        assert_eq!(hit_plain, 0, "без воронок сквозь стену прошло {hit_plain}");
        assert!(solid_plain, "без воронок стена всё равно исчезла");

        let (hit_blast, solid_blast) = run(2);
        assert!(!solid_blast, "воронки не тронули стену");
        assert!(
            hit_blast > 0,
            "стену проломили, а попаданий нет: {hit_blast}"
        );
    }

    /// **События совпадают со счётчиками и живут ровно один тик.**
    ///
    /// Событие — выход, а не состояние: в хеш оно не входит, значит
    /// ошибку в нём не поймает ни golden, ни паритет воркеров. Сторож
    /// нужен именно поэтому.
    ///
    /// Проверяется трое: взрывов столько же, сколько насчитала
    /// симуляция; смертей столько же; буфер не копится между тиками.
    #[test]
    fn events_match_the_counters_and_last_one_tick() {
        let mut r = Reference::new(42, 600, test_tuning(), test_weapons());
        let (mut blasts, mut deaths) = (0u64, 0u64);
        let mut worst = 0usize;
        for t in 1..=200 {
            let (b0, d0, m0) = (r.world.blasts, r.world.deaths, r.world.beams_fired);
            r.tick();
            let (nb, nd, nm) = (
                r.world.blasts - b0,
                r.world.deaths - d0,
                r.world.beams_fired - m0,
            );
            let (mut eb, mut ed, mut em) = (0u64, 0u64, 0u64);
            for e in r.world.events() {
                match e {
                    SimEvent::Blast { .. } => eb += 1,
                    SimEvent::Death { .. } => ed += 1,
                    SimEvent::Beam { .. } => em += 1,
                }
            }
            assert_eq!(eb, nb, "тик {t}: взрывов {nb}, событий о взрывах {eb}");
            assert_eq!(ed, nd, "тик {t}: смертей {nd}, событий о смертях {ed}");
            assert_eq!(em, nm, "тик {t}: лучей {nm}, событий о лучах {em}");
            // Каждый вид сверен поимённо, значит и целое сходится:
            // лишнего в буфере нет.
            assert_eq!(
                (eb + ed + em) as usize,
                r.world.events().len(),
                "тик {t}: в буфере есть событие неизвестного вида"
            );
            worst = worst.max(r.world.events().len());
            blasts += nb;
            deaths += nd;
        }
        assert!(blasts > 0, "за прогон не рвануло ни разу");
        assert!(deaths > 0, "за прогон никто не погиб");
        assert!(worst > 0, "за прогон не случилось ни одного события");
    }

    /// **Смерть сообщается там, где юнит стоял.**
    ///
    /// Событие берётся **до** освобождения места: свободный список
    /// переиспользует место, и координаты после — уже чужие.
    #[test]
    fn a_death_is_reported_where_the_unit_stood() {
        let mut r = Reference::new(42, 600, test_tuning(), test_weapons());
        let mut seen = 0;
        for _ in 0..200 {
            // Положения до тика: событие обязано указать на одно из них.
            let before: std::collections::BTreeSet<(u32, i32, i32, i32)> = r
                .world
                .units
                .live()
                .iter()
                .map(|&i| {
                    let k = i as usize;
                    (i, r.world.units.x[k], r.world.units.y[k], r.world.units.z[k])
                })
                .collect();
            r.tick();
            for e in r.world.events() {
                if let SimEvent::Death { unit, x, y, z } = *e {
                    // Юнит мог сдвинуться за тик, поэтому сверяется
                    // не точка, а то, что он вообще был живым.
                    assert!(
                        before.iter().any(|&(i, _, _, _)| i == unit),
                        "погиб юнит {unit}, которого не было в живых"
                    );
                    assert!(
                        x != i32::MIN && y != i32::MIN && z != i32::MIN,
                        "координаты смерти не заполнены: ({x}, {y}, {z})"
                    );
                    seen += 1;
                }
            }
        }
        assert!(seen > 0, "ни одной смерти за прогон");
    }

    /// **Луч не проходит сквозь породу и не роет её.**
    ///
    /// Два отличия луча от болта, и оба проверяются здесь. Он приходит
    /// в тот же тик — значит попадает **или не попадает** сразу,
    /// без полёта. И он не оставляет воронки — стена, которую болт
    /// проламывает за двести тиков (ADR-165), под лучом стоит.
    #[test]
    fn a_laser_neither_pierces_rock_nor_digs_it() {
        struct Wall;
        const GY: u32 = 8;
        const AT: u32 = 32;
        const HALF: u32 = 6;
        impl voxel::Generator for Wall {
            fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
                if y < GY {
                    return 2;
                }
                if x.abs_diff(AT) <= HALF && y < GY + 12 {
                    return 3;
                }
                voxel::store::AIR
            }
        }
        // `wall` — стоит ли стена между стрелками.
        let run = |wall: bool| -> (u64, u64, bool, u64) {
            let t = test_tuning();
            let ws = modding::weapons::from_ron(
                "( weapons: [ ( id: 2, kind: Laser, speed: 0, damage: 12, \
reload: 8, blast_radius: 0, blast_damage: 0 ) ] )",
            )
            .expect("реестр");
            let mut vw = voxel::World::new(64, 48, 64, 4096, Box::new(Wall));
            if !wall {
                for x in AT - HALF..=AT + HALF {
                    for y in GY..GY + 8 {
                        for z in 24..40 {
                            vw.queue_dig(x, y, z);
                        }
                    }
                }
                while vw.queued() > 0 {
                    vw.begin_tick();
                    vw.flush();
                }
            }
            let g = ground::Ground::new(vw, &t);
            let mut w = World::new(2, 8, t);
            w.attach_ground(g);
            w.attach_weapons(ws);
            w.spawn_ground(AT as i32 - 8, 32, 0, 0, 300);
            w.spawn_ground(AT as i32 + 8, 32, 1, 0, 300);
            w.units.refresh_live();
            for _ in 0..200 {
                w.tick();
            }
            let solid = w.ground().expect("земля").world.solid(AT, GY + 1, 32);
            (w.beams_fired, w.beams_hit, solid, w.beams_blocked)
        };

        let (fired, hit, solid, blocked) = run(true);
        assert_eq!(fired, 0, "по закрытой цели выстрелили {fired} раз");
        assert_eq!(hit, 0, "сквозь стену прошло {hit} попаданий лучом");
        assert!(blocked > 0, "стена не остановила ни одной попытки");
        assert!(solid, "луч проел стену — воронки у него быть не должно");

        let (fired_open, hit_open, _, _) = run(false);
        assert!(hit_open > 0, "без стены луч не попал ни разу — проверять нечего");
        // **Луч не мажет.** Линия проверяется до выстрела, поэтому
        // выстрел и попадание — одно и то же событие (ADR-170).
        assert_eq!(
            fired_open, hit_open,
            "выстрелов {fired_open}, попаданий {hit_open} — луч промахнулся"
        );
    }

    /// **Стороны вооружены разным, и обе ветки разбора работают.**
    ///
    /// Ветка, которая ни разу не вызвана, не проверена ничем: хеш
    /// её не покрывает, потому что она не меняет состояние (ADR-169).
    #[test]
    fn both_weapon_kinds_are_actually_used() {
        let mut r = Reference::new(42, 600, test_tuning(), test_weapons());
        assert!(r.world.weapons.len() >= 2, "в реестре меньше двух видов");
        for _ in 0..200 {
            r.tick();
        }
        assert!(r.world.shots_fired > 0, "болтом не стреляли ни разу");
        assert!(r.world.beams_fired > 0, "лучом не стреляли ни разу");
        // И обе стороны действительно разного вооружения.
        let mut seen = [false; 2];
        for &i in r.world.units.live() {
            let k = i as usize;
            seen[(r.world.units.weapon[k] as usize).min(1)] = true;
        }
        assert!(seen[0] && seen[1], "обе стороны с одним оружием: {seen:?}");
    }

    /// **Наездник не считается «внутри породы».**
    ///
    /// Второе исключение инварианта (ADR-165), и проверяется оно
    /// на **построенном** случае, а не выпрошенном у большой сцены:
    /// юнит на шляпке гриба, ножку подрезают, шляпка падает вместе
    /// с ним. Под ним в этот момент нет уровня — и не должно быть.
    ///
    /// Случай построен намеренно: та же проверка на эталонном
    /// сценарии зависела от того, сколько в нём копают, и ломалась
    /// от каждой правки баланса.
    #[test]
    fn a_rider_is_not_inside_solid_rock() {
        /// Гриб: ножка до шляпки, шляпка над ней.
        struct Mushroom;
        impl voxel::Generator for Mushroom {
            fn material(&self, x: u32, y: u32, z: u32) -> voxel::store::Mat {
                if y < 8 {
                    return 2;
                }
                // Ножка.
                if (28..36).contains(&x) && (28..36).contains(&z) && y < 44 {
                    return 3;
                }
                // Шляпка.
                if (20..44).contains(&x) && (20..44).contains(&z) && (40..44).contains(&y) {
                    return 3;
                }
                voxel::store::AIR
            }
        }
        let t = test_tuning();
        let vw = voxel::World::new(64, 64, 64, 100_000, Box::new(Mushroom));
        let mut w = World::new(1, 4, t);
        w.attach_ground(ground::Ground::new(vw, &t));
        let id = w.units.spawn(32, 32, 0, 0, 300) as usize;
        w.units.refresh_live();
        w.tick();
        let y0 = w.units.y[id];
        assert!(y0 > 40, "юнит не встал на шляпку: y = {y0}");

        // Подрезать ножку.
        {
            let g = w.ground_mut().expect("земля");
            for y in 9..21 {
                for z in 28..36 {
                    for x in 28..36 {
                        if g.world.solid(x, y, z) {
                            g.world.queue_dig(x, y, z);
                        }
                    }
                }
            }
        }

        let mut rode = 0u32;
        let mut checked = 0u32;
        for tick in 0..60 {
            w.tick();
            let g = w.ground().expect("земля");
            let st = g.nav.step() as i32;
            let (cx, cz) = (
                (w.units.x[id] / st).max(0) as u32,
                (w.units.z[id] / st).max(0) as u32,
            );
            let uy = w.units.y[id];
            checked += 1;
            if w.units.riding[id] != 0 {
                rode += 1;
                continue;
            }
            if w.units.fall_from[id] != NOT_FALLING {
                continue;
            }
            assert!(
                g.nav.has_level(cx, cz, uy),
                "тик {tick}: юнит в ({}, {uy}, {}) без уровня, не падает и не едет",
                w.units.x[id],
                w.units.z[id]
            );
        }
        assert!(checked > 0);
        assert!(rode > 0, "юнит ни разу не ехал — случай не построился");
        assert!(
            w.units.y[id] < y0,
            "юнит не поехал вниз: {} против {y0}",
            w.units.y[id]
        );
    }

    /// **Два мира, идущие вперемешку, совпадают тик в тик.**
    ///
    /// `same_seed_same_hash` гоняет прогоны **подряд**, и скрытое общее
    /// состояние — статик, потокоместное, кэш, зависящий от порядка
    /// аллокаций — в обоих прогонах проходит одну и ту же
    /// последовательность, то есть не видно. Здесь миры чередуются,
    /// и такая зависимость расходится сразу.
    ///
    /// Сравнение **потиковое**, а не по итогу: расхождение ловится там,
    /// где внесено, а не через тысячу тиков (`docs/03-determinism.md`,
    /// SyncTest).
    #[test]
    fn two_worlds_run_side_by_side_match_every_tick() {
        let mut a = Reference::new(42, 800, test_tuning(), test_weapons());
        let mut b = Reference::new(42, 800, test_tuning(), test_weapons());
        for t in 1..=150 {
            let ha = a.tick();
            let hb = b.tick();
            assert_eq!(ha, hb, "миры разошлись на тике {t}: {ha:016x} против {hb:016x}");
        }
    }

    /// **Требование `CLAUDE.md`: один сценарий на 1/2/4/8 воркерах даёт
    /// один хеш.** Держится оно на том, что фаза движения читает снимок,
    /// а не уже сдвинутых соседей: без снимка расталкивание зависит
    /// от порядка обхода, и параллелить его нельзя ни при каком разбиении.
    #[test]
    fn one_hash_across_worker_counts() {
        let run = |threads: usize| {
            let mut r = Reference::new(77, 3000, test_tuning(), test_weapons());
            let mut h = 0u64;
            for _ in 0..80 {
                h = r.tick_par(threads);
            }
            h
        };
        let base = run(1);
        for t in [2usize, 4, 8] {
            assert_eq!(run(t), base, "хеш разошёлся на {t} воркерах");
        }
    }

    /// Разные сиды — разные хеши. Ловит вырождение хеша в константу.
    #[test]
    fn different_seeds_differ() {
        let run = |seed| {
            let mut w = World::new(2, 4096, test_tuning());
            populate(&mut w, 500, seed);
            let mut h = 0u64;
            for _ in 0..30 {
                h = w.tick();
            }
            h
        };
        assert_ne!(run(1), run(2));
    }

    /// Состояние обязано **меняться**: тест на «фазы пустые и потому быстрые».
    #[test]
    fn state_actually_moves() {
        let mut w = World::new(2, 4096, test_tuning());
        populate(&mut w, 500, 7);
        // Поле направлений четырёхстороннее, поэтому половина юнитов идёт
        // чисто по Z: проверять надо положение, а не одну координату.
        let before: Vec<(i32, i32)> = w
            .units
            .x
            .iter()
            .zip(w.units.z.iter())
            .map(|(&a, &b)| (a, b))
            .collect();
        for _ in 0..30 {
            w.tick();
        }
        let moved = w
            .units
            .x
            .iter()
            .zip(w.units.z.iter())
            .zip(before.iter())
            .filter(|((a, b), (pa, pb))| **a != *pa || **b != *pb)
            .count();
        assert!(moved > 450, "сдвинулось только {moved} из 500");
        assert!(w.examined_move > 0, "расталкивание никого не осмотрело");
    }

    /// Free-list, а не `swap_and_pop`: убийство юнита не двигает соседей.
    /// Именно перестановка соседей молча меняет исход ничьей.
    #[test]
    fn kill_does_not_move_neighbours() {
        let mut w = World::new(2, 64, test_tuning());
        for k in 0..10 {
            w.units.spawn(k * 10, 0, 0, 1, 100);
        }
        w.units.refresh_live();
        let before: Vec<i32> = w.units.x.clone();
        w.units.kill(4);
        w.units.refresh_live();
        assert_eq!(w.units.x, before, "убийство сдвинуло чужие слоты");
        assert_eq!(w.units.live(), &[0, 1, 2, 3, 5, 6, 7, 8, 9]);
        // Переиспользование слота — того же, а не нового.
        let n = w.units.len();
        let id = w.units.spawn(999, 999, 1, 1, 100);
        assert_eq!(id, 4);
        assert_eq!(w.units.len(), n, "хранилище выросло вместо переиспользования");
    }

    /// Выбор цели не зависит от порядка обхода: при равном расстоянии
    /// побеждает меньший индекс.
    #[test]
    fn ties_resolve_by_index() {
        let mut w = World::new(2, 64, test_tuning());
        let me = w.units.spawn(100, 100, 0, 0, 100);
        // Две цели на одинаковом расстоянии.
        let a = w.units.spawn(110, 100, 1, 0, 100);
        let b = w.units.spawn(90, 100, 1, 0, 100);
        assert!(a < b);
        w.units.refresh_live();
        w.tick();
        assert_eq!(w.units.target[me as usize], a, "ничья ушла не меньшему индексу");
    }
}
