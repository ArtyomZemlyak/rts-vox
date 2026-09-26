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

mod contacts;
pub mod drive;
pub mod economy;
pub mod energy;
pub mod gait;
pub mod ground;
pub mod heat;
pub mod index;
pub mod integrity;
pub mod mapcache;
pub mod matter;
pub mod mobility;
pub mod order;
pub mod ride;
pub mod save;
pub mod shield;
pub mod shot;
mod shot_probe;
pub mod turret;

use drive::{Drive, Limits, NO_WAY};
use fx::angle;
use fx::rng::Rng;
use fx::Fx;
use index::Grid;
pub use modding::{ChassisKind, Tuning};
pub use order::{
    Order, STANCE_ATTACK_MOVE, STANCE_BUILD, STANCE_DIG, STANCE_ENGAGED, STANCE_FLOW, STANCE_HAUL,
    STANCE_HOLD, STANCE_MOVE,
};

/// Сторона мира в мировых единицах. Тысяча метров при ребре
/// воксела 0.1 м (ADR-227).
pub const WORLD: i32 = 10_000;
/// Сторона нав-клетки — метр, как в `data/sim.ron`.
pub const NAV_CELL: i32 = 10;
/// Сторона нав-сетки.
pub const NAV: usize = (WORLD / NAV_CELL) as usize;

/// Тиков в секунде. Скорость юнита задана в клетках **в секунду**,
/// поэтому шаг за тик делится на это число. Голая тридцатка в формуле
/// шага один раз уже стоила дорого: эталонный сценарий разводил
/// стороны на расстояние, которое они не проходили за прогон (ADR-116).
pub const TICKS_PER_SECOND: i32 = 30;

pub mod blast;
mod body_wave;
pub mod burst;
pub mod damage;
mod damage_combat;
mod engagement;
pub mod skirmish;

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
    /// Предельная скорость машины, мировых единиц в секунду. Не шаг
    /// за тик: шаг считает тот, кто едет (ADR-220).
    pub speed: Vec<i32>,
    /// Курс: куда машина смотрит, двоичный угол `fx::angle`.
    ///
    /// Состояние, а не вывод из положения. Машина едет **по курсу**,
    /// а курс доворачивается с предельной угловой скоростью: без
    /// этого поворот на прямой угол занимал один тик, и диагональный
    /// путь по четырёхстороннему полю выглядел лестницей (ADR-220).
    pub head: Vec<i32>,
    /// Скорость вдоль курса, мировых единиц в секунду. Растёт
    /// с предельным ускорением, падает с предельным торможением.
    pub vel: Vec<Fx>,
    /// Вид ходовой: у гусеничной и шагающей машины пределы хода
    /// разные, и лежат они в `data/sim.ron`.
    pub chassis: Vec<ChassisKind>,
    /// Нормаль корпуса: куда смотрит «верх» машины. Почти единичная.
    ///
    /// **Состояние, а не вывод из рельефа.** Её двигает подвеска
    /// в точках опоры (ADR-225), поэтому машина клюёт на уступе
    /// и качается на кочках, а не мгновенно ложится на новый уклон.
    /// Переживает тик — значит входит в хеш (ADR-116).
    pub up: Vec<phys::V3>,
    /// Угловая скорость корпуса. Вертикальная составляющая не
    /// используется: поворотом заведует ход, а не подвеска.
    pub spin: Vec<phys::V3>,
    /// Доля опор на земле по итогам прошлого тика, от нуля до единицы.
    /// Ноль — машина висит в воздухе.
    pub load: Vec<Fx>,
    /// Physical root height relative to the navigation floor, and vertical
    /// velocity in world units per second. Kept across ticks and saves.
    pub heave: Vec<Fx>,
    pub heave_v: Vec<Fx>,
    /// Продольное ускорение за прошлый тик, мировых единиц в секунду
    /// за секунду. Считается там же, где меняется скорость, и нужно
    /// подвеске: торможение роняет корпус носом (ADR-225).
    ///
    /// В хеш не входит: это производная от `vel`, а `vel` хешируется;
    /// разойдись она — разойдётся и `vel` на том же тике.
    pub accel: Vec<Fx>,
    /// Изменение курса за прошлый тик, двоичный угол. Вместе со
    /// скоростью даёт центростремительное ускорение — крен в повороте.
    /// В хеш не входит по той же причине, что `accel`: производная
    /// от `head`.
    pub yaw_step: Vec<i32>,
    /// Угловая скорость: двоичный угол за тик (ADR-234).
    ///
    /// **Состояние, а не производная.** Машина доворачивает не мгновенно:
    /// угловая скорость разгоняется к желаемой с угловым ускорением
    /// из тяги на плече борта. Потеряй её между тиками — и разворот
    /// начинался бы заново каждый тик, то есть рывком.
    pub yaw_rate: Vec<i32>,
    /// Фаза шага, двоичный угол по кольцу цикла (ADR-234).
    ///
    /// Растёт пройденным путём, а не временем: шагоход, которого
    /// держат на месте, ногами не перебирает. Ноль у всего, что
    /// не шагает.
    pub gait: Vec<i32>,
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
    /// У подвижного корпуса это наивысшая физическая высота полёта.
    /// Состояние посадки входит в хеш (ADR-253).
    pub fall_from: Vec<i32>,
    /// Скорость вниз за тик: у подвижных тел выводится из heave_v,
    /// у неподвижных типов интегрируется прежним путём (ADR-174).
    pub fall_v: Vec<Fx>,
    /// Дробная часть высоты, пока юнит падает. Без неё падение шло бы
    /// только целыми вокселями за тик, то есть не ускорялось бы
    /// вовсе: первая секунда свободного падения — меньше воксела
    /// за тик.
    pub fall_f: Vec<Fx>,
    /// Тик, раньше которого юнит не ищет цель заново. Поиск цели —
    /// самая дорогая часть боя, и держать её потиковой незачем: цель,
    /// живая и в радиусе, остаётся целью (ADR-116).
    pub(crate) next_search: Vec<u32>,
    /// Тик, раньше которого юнит не выстрелит. Перезарядка —
    /// состояние юнита, а не свойство выстрела (ADR-164).
    pub(crate) next_shot: Vec<u32>,
    /// Место оружия в реестре. Место, а не номер: номер ищется
    /// перебором, а это горячий путь (ADR-169).
    pub weapon: Vec<u16>,
    /// Что юнит делает: идёт по полю своей стороны, идёт в точку
    /// приказа, стоит или грызёт забой (`order::STANCE_*`).
    ///
    /// Ноль — поведение **без приказа**, и свежий слот начинается
    /// с него. Приказ живёт здесь, а не в отдельной таблице: таблица
    /// по номеру юнита — это либо хеш-контейнер, либо второй свободный
    /// список, и оба запрещены `CLAUDE.md` (ADR-195).
    pub stance: Vec<u8>,
    /// Точка приказа в **мировых единицах**, не в клетках и не
    /// в вокселях (`CLAUDE.md`). Смысл задаёт `stance`: цель хода
    /// или забой. При `STANCE_FLOW` не читается.
    pub goal_x: Vec<i32>,
    pub goal_y: Vec<i32>,
    pub goal_z: Vec<i32>,

    // --- экономика (ADR-230)
    //
    /// Тип из таблицы `data/kinds.ron` — **место** в реестре, а не
    /// номер. `docs/05-units.md` называет это поле `kind`.
    /// [`economy::NO_KIND`] — юнит пришёл не из таблицы: так живут
    /// стенды и эталонный сценарий, и платить за экономику они
    /// не обязаны.
    pub kind: Vec<u16>,
    /// Контейнер юнита: бак краулера, вложенное в недострой, тело
    /// готовой машины. [`economy::NO_CONTAINER`] — контейнера нет.
    pub store: Vec<matter::ContainerId>,
    /// Сколько материи недострою ещё не хватает. Ноль — достроен.
    ///
    /// Убывающая величина, а не растущая: «достроен» это ноль,
    /// и проверять его не надо сравнивать со стоимостью типа,
    /// которая лежит в другом месте.
    pub site: Vec<matter::Amount>,
    /// Забой, к которому копатель вернётся, отвезя груз. Читается
    /// только при `STANCE_HAUL`.
    pub face: Vec<(i32, i32, i32)>,
    /// Над чем юнит работает: слот склада, недостроя или номер
    /// обломка. [`NO_TARGET`] — ни над чем.
    ///
    /// Отдельно от `target`: цель — это по кому стрелять, и фаза боя
    /// её перевыбирает. Работа приказом игрока не отменяется.
    pub job: Vec<u32>,

    pub(crate) free: Vec<u32>,
    pub(crate) live_ids: Vec<u32>,
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
    modding::weapons::from_ron(include_str!("../../../data/weapons.ron")).expect("data/weapons.ron")
}

/// Юнит стоит на опоре, а не падает.
pub const NOT_FALLING: i32 = i32::MIN;

/// Урон от падения с высоты `fallen` вокселей.
///
/// **Уступ бесплатен** (ADR-036): падение не выше `free` — того
/// перепада, который юнит перешагивает не глядя (`max_step`
/// в данных, ADR-121), — даёт нулевой урон и читается как шаг вниз.
/// Иначе копатель платил бы здоровьем за каждый снятый у себя под
/// ногами воксель, а забой — это то, где он стоит всю игру.
///
/// Бесплатным был **один воксель** — на ребре 0.5 м он и был уступом.
/// На ребре 0.1 м уступ — пять вокселей, и правило «воксель» означало
/// бы, что юнит, сходящий с полуметрового уступа, который навигация
/// считает проходимым, треть секунды висит в воздухе и платит за это
/// телом; на каждом склоне, где соседние клетки расходятся
/// на два-пять вокселей, ход превращался бы в заикание (ADR-227).
///
/// Правило одно для всех случаев — сам выкопал, отделился кусок,
/// завалило оседающей материей, — поэтому и функция одна.
pub fn fall_damage(fallen: u32, free: u32, per_voxel: u16) -> u32 {
    fallen.saturating_sub(free).saturating_mul(per_voxel as u32)
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
            head: Vec::new(),
            vel: Vec::new(),
            chassis: Vec::new(),
            up: Vec::new(),
            spin: Vec::new(),
            load: Vec::new(),
            heave: Vec::new(),
            heave_v: Vec::new(),
            accel: Vec::new(),
            yaw_step: Vec::new(),
            yaw_rate: Vec::new(),
            gait: Vec::new(),
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
            stance: Vec::new(),
            goal_x: Vec::new(),
            goal_y: Vec::new(),
            goal_z: Vec::new(),
            kind: Vec::new(),
            store: Vec::new(),
            site: Vec::new(),
            face: Vec::new(),
            job: Vec::new(),
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

    /// Authoritative body origin, shared by rendering and material collision.
    /// `y` itself remains the discrete navigation level.
    pub fn body_y(&self, i: usize) -> Fx {
        Fx::from_int(self.y[i])
            .add(self.heave[i])
            .sub(if self.fall_from[i] == NOT_FALLING {
                Fx::ZERO
            } else {
                self.fall_f[i]
            })
    }

    pub fn body_vy(&self, i: usize) -> Fx {
        if self.speed[i] > 0 || self.fall_from[i] == NOT_FALLING {
            self.heave_v[i]
        } else {
            self.fall_v[i].neg().mul(Fx::from_int(TICKS_PER_SECOND))
        }
    }

    /// Integer-only legacy rays use the nearest cell-height coordinate.
    /// Truncation would turn a submillimetre spring residual into a whole
    /// voxel drop and put a ground-level laser inside otherwise level ground.
    pub fn body_cell_y(&self, i: usize) -> i32 {
        let y = self.body_y(i);
        let half = Fx::from_ratio(1, 2);
        if y.raw() >= 0 {
            y.add(half).to_int_trunc()
        } else {
            y.sub(half).to_int_trunc()
        }
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
            // **Слот из свободного списка не наследует хода.** Курс
            // и скорость покойника означали бы, что подкрепление
            // появляется на полном ходу и носом не туда.
            self.head[k] = 0;
            self.vel[k] = Fx::ZERO;
            self.chassis[k] = ChassisKind::Tracked;
            // И наклона покойника тоже не наследует: подкрепление,
            // вставшее в слот машины, лежавшей на боку, лежало бы
            // на боку само.
            self.up[k] = phys::V3::UP;
            self.spin[k] = phys::V3::ZERO;
            self.load[k] = Fx::ZERO;
            self.heave[k] = Fx::ZERO;
            self.heave_v[k] = Fx::ZERO;
            self.accel[k] = Fx::ZERO;
            self.yaw_step[k] = 0;
            self.voxels[k] = voxels;
            self.alive[k] = true;
            self.target[k] = NO_TARGET;
            self.riding[k] = 0;
            self.ride_off[k] = (0, 0, 0);
            self.fall_from[k] = NOT_FALLING;
            self.fall_v[k] = Fx::ZERO;
            self.fall_f[k] = Fx::ZERO;
            self.yaw_rate[k] = 0;
            self.gait[k] = 0;
            self.next_search[k] = 0;
            self.next_shot[k] = 0;
            self.weapon[k] = 0;
            // **Слот из свободного списка не наследует приказ.**
            // Забудь это — и подкрепление, попавшее в место погибшего
            // копателя, пошло бы грызть его забой (ADR-195).
            self.stance[k] = order::STANCE_FLOW;
            self.goal_x[k] = 0;
            self.goal_y[k] = 0;
            self.goal_z[k] = 0;
            // **Слот из свободного списка не наследует ни типа, ни
            // контейнера, ни недостроя.** Наследуй он контейнер —
            // и подкрепление получило бы полный бак покойника,
            // то есть материю из ниоткуда. Ту же дыру закрывает
            // правило про приказ (ADR-195).
            self.kind[k] = economy::NO_KIND;
            self.store[k] = economy::NO_CONTAINER;
            self.site[k] = 0;
            self.face[k] = (0, 0, 0);
            self.job[k] = NO_TARGET;
            return i;
        }
        self.x.push(x);
        self.y.push(0);
        self.z.push(z);
        self.fx.push(Fx::ZERO);
        self.fz.push(Fx::ZERO);
        self.owner.push(owner);
        self.speed.push(speed);
        self.head.push(0);
        self.vel.push(Fx::ZERO);
        self.chassis.push(ChassisKind::Tracked);
        self.up.push(phys::V3::UP);
        self.spin.push(phys::V3::ZERO);
        self.load.push(Fx::ZERO);
        self.heave.push(Fx::ZERO);
        self.heave_v.push(Fx::ZERO);
        self.accel.push(Fx::ZERO);
        self.yaw_step.push(0);
        self.yaw_rate.push(0);
        self.gait.push(0);
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
        self.stance.push(order::STANCE_FLOW);
        self.goal_x.push(0);
        self.goal_y.push(0);
        self.goal_z.push(0);
        self.kind.push(economy::NO_KIND);
        self.store.push(economy::NO_CONTAINER);
        self.site.push(0);
        self.face.push((0, 0, 0));
        self.job.push(NO_TARGET);
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
    ///
    /// Свободный список тут же приводится в порядок — тем же правилом,
    /// что у снарядов (`shot::Shots::refresh_live`). Иначе номер
    /// выданного места зависел бы от **истории смертей**, а не от
    /// состояния: два мира с одинаковым набором живых юнитов, но
    /// разной историей, выдали бы новому юниту разные слоты. Пока
    /// состояние переигрывают с нуля, это безвредно; как только его
    /// начнут восстанавливать из сохранения — перестанет (ADR-192).
    pub fn refresh_live(&mut self) {
        self.live_ids.clear();
        self.free.sort_unstable();
        self.free.reverse();
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
    contacts: contacts::Contacts,
    pub contact_group_pairs: u64,
    shot_probe: shot_probe::Probe,
    /// Желания, посчитанные параллельно и применяемые одной серийной
    /// фазой (`CLAUDE.md`: клэмп применяется один раз, в конце).
    ///
    /// Не смещения: смещение теперь считает трансмиссия, и считает
    /// она его из **состояния** юнита — курса и скорости, — которое
    /// параллельная фаза трогать не имеет права (ADR-220).
    delta: Vec<drive::Want>,
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
    pub(crate) flow_target: [Option<(i32, i32)>; 2],
    /// Поле направлений: общий граф, секторы и по полю на владельца
    /// (ADR-153).
    pub(crate) nav_field: nav::sectors::NavField,
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
    /// Очередь приказов — **единственный вход** в симуляцию снаружи
    /// (ADR-195). Кладут в неё игрок и мод, вынимает фаза
    /// `ApplyCommands`.
    ///
    /// Длины у очереди нет по построению — ровно по той же причине,
    /// по которой её нет у очереди правок мира: потолок применяется
    /// при дренаже, иначе приказ молча терялся бы.
    pub(crate) orders: std::collections::VecDeque<Order>,
    /// Замер: приказов применено и отброшено, и в скольких тиках
    /// дренаж упёрся в потолок. Потолок, срабатывающий каждый тик, —
    /// не предохранитель, а постоянное ухудшение (ADR-114), и видно
    /// это только по счётчику.
    pub orders_applied: u64,
    pub orders_dropped: u64,
    pub orders_capped: u64,
    /// Учёт материи: запасы сторон, очереди заводов, обломки.
    ///
    /// Необязателен по той же причине, что и земля: стенд, мерящий
    /// пять тысяч юнитов, экономики не ведёт, и фазы `Logistics`
    /// с `Economy` на пустом месте остаются заглушками, какими были.
    /// Это не «включаемая механика», а отсутствие данных.
    pub econ: Option<economy::Economy>,
    pub material_bodies: Option<damage::Bodies>,
    pub bursts: burst::Bursts,
    /// Чем кончилась партия. Пересчитывается каждый тик в `Economy`.
    outcome: Outcome,
    /// Тик, на котором исход определился.
    pub decided_at: Option<u64>,
    pub phase_ns: [u64; 16],
    /// Diagnostic nanoseconds: neighbour requests, drive/contact, suspension.
    /// Never read by the simulation, saved, or hashed.
    pub movement_ns: [u64; 3],
    /// Diagnostic nanoseconds: topology, walkability, fields, unit indices.
    pub navigation_ns: [u64; 4],
    /// Diagnostic nanoseconds: projectiles, targeting, fire, material damage.
    pub combat_ns: [u64; 4],
}

/// Чем кончилась партия.
///
/// **Условие простое и проверяемое:** у стороны не осталось ничего
/// живого — ни машин, ни построек — значит она проиграла. Постройка
/// считается живой наравне с машиной: сторона с одним складом ещё
/// в игре, и отнять у неё победу нельзя.
///
/// Срок партии и победа по очкам живут выше — в матче
/// (`demo::skirmish`): это правила состязания, а не правило мира.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Ещё идёт.
    Fighting,
    /// У всех остальных не осталось живых.
    Won(u8),
    /// Живых не осталось ни у кого.
    Draw,
}

impl Outcome {
    pub fn winner(self) -> Option<u8> {
        match self {
            Outcome::Won(o) => Some(o),
            _ => None,
        }
    }

    pub fn over(self) -> bool {
        self != Outcome::Fighting
    }
}

const DIRS: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];

/// Курс одного из четырёх направлений поля.
const fn axis_way(d: (i32, i32)) -> i32 {
    if d.0 > 0 {
        0
    } else if d.0 < 0 {
        angle::HALF
    } else if d.1 > 0 {
        angle::QUARTER
    } else {
        angle::norm(-angle::QUARTER)
    }
}

/// Курсы четырёх направлений поля, в порядке `DIRS`.
///
/// **Считаются из самих `DIRS`**, а не записаны числами рядом:
/// две записи одного и того же разошлись бы молча, и юниты пошли бы
/// в сторону, обратную полю.
const DIR_WAYS: [i32; 4] = [
    axis_way(DIRS[0]),
    axis_way(DIRS[1]),
    axis_way(DIRS[2]),
    axis_way(DIRS[3]),
];

/// Куда юнит хочет ехать, двоичный угол или [`NO_WAY`].
///
/// Читает **только снимок положений, поле направлений и приказ**, и
/// ничего не пишет. Отсюда и свобода разбиения фазы на потоки.
///
/// Функция одна на оба вызова — и на ядро, и на фазу. Пока их было
/// две, они разошлись источником направления и тождество, которое
/// ядро обещает, держалось только на словах (ADR-194).
#[inline]
fn want_way(
    u: &Units,
    i: usize,
    ux: i32,
    uz: i32,
    vol: Option<(&nav::layer::NavLayer, &nav::sectors::NavField)>,
    flow: &[Vec<u8>],
    arrive: i32,
) -> i32 {
    match u.stance[i] {
        // Без приказа юнит идёт по полю своей стороны.
        STANCE_FLOW => {
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
            if d < 4 {
                DIR_WAYS[d as usize]
            } else {
                NO_WAY
            }
        }
        STANCE_HOLD | STANCE_ENGAGED => NO_WAY,
        _ => order::goal_way(ux, uz, u.goal_x[i], u.goal_z[i], arrive),
    }
}

/// Во сколько процентов предельной скорости обходится уклон впереди.
///
/// Смотрится **соседняя нав-клетка по курсу**, а не соседний воксель:
/// уклон — свойство ребра навигации (ADR-121), и внутри одной клетки
/// его нет. Порог в половину единицы по косинусу даёт восемь румбов:
/// машина, идущая ровно на север, смотрит на север, идущая на
/// северо-восток — по диагонали.
///
/// Перепад, которого ребро не держит, считается подъёмом: туда
/// машина всё равно не поедет, а подкатывать к обрыву на полном ходу
/// ей незачем.
#[inline]
fn slope_percent(
    nav: &nav::layer::NavLayer,
    x: i32,
    z: i32,
    y: i32,
    head: i32,
    climb: i32,
    descend: i32,
) -> i32 {
    let st = nav.step() as i32;
    if x < 0 || z < 0 {
        return 100;
    }
    let (cx, cz) = (x / st, z / st);
    let (gw, gh) = nav.dims();
    if cx >= gw as i32 || cz >= gh as i32 {
        return 100;
    }
    let half = Fx::from_ratio(1, 2).raw();
    let rumb = |v: Fx| -> i32 {
        if v.raw() > half {
            1
        } else if v.raw() < -half {
            -1
        } else {
            0
        }
    };
    let (ax, az) = (rumb(angle::cos(head)), rumb(angle::sin(head)));
    if ax == 0 && az == 0 {
        return 100;
    }
    let (nx, nz) = (cx + ax, cz + az);
    if nx < 0 || nz < 0 || nx >= gw as i32 || nz >= gh as i32 {
        return 100;
    }
    match nav.step_to((cx as u32, cz as u32), y, (nx as u32, nz as u32)) {
        Some(l) => match (l as i32).cmp(&y) {
            std::cmp::Ordering::Greater => climb,
            std::cmp::Ordering::Less => descend,
            std::cmp::Ordering::Equal => 100,
        },
        None => climb,
    }
}

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

    /// Привязать экономику: таблицу типов и реестр материалов.
    ///
    /// Отдельным вызовом, как оружие и земля. Мир без неё законен —
    /// он просто не ведёт учёта: копка снимает воксели, и материя
    /// никуда не идёт. Так живут стенды и эталонный сценарий, и хеш
    /// у них от появления экономики **не сдвигается ни на бит**:
    /// то, чего нет, в хеш не входит (ADR-195, ADR-230).
    pub fn attach_economy(&mut self, kinds: modding::Kinds, mats: modding::Registry) {
        let owners = self.flow.len().max(1);
        let roster = voxel::roster::all();
        let templates = kinds
            .all()
            .iter()
            .map(|k| {
                roster
                    .iter()
                    .find(|(name, _)| *name == k.body)
                    .unwrap_or_else(|| panic!("unknown body {}", k.body))
                    .1
                    .clone()
            })
            .collect();
        self.material_bodies = Some(damage::Bodies::new(templates));
        self.econ = Some(economy::Economy::new(kinds, mats, owners, &self.tuning));
    }

    /// Поставить юнита **известного типа**: числа берутся из таблицы,
    /// а не из аргументов.
    ///
    /// Вернёт `None`, если типа с таким номером нет или экономика
    /// не привязана: молча ставить юнита с нулевой скоростью нельзя —
    /// это неподвижная машина, которую никто не заказывал.
    ///
    /// **Тело заводится уже сделанным из материи.** Машина, поставленная
    /// на карту расстановкой, — это машина, которая из чего-то состоит,
    /// и её стоимость входит в начальную сумму учёта: ровно так партия
    /// с чего-то начинается (`docs/07-economy.md`, «содержимое при
    /// заведении входит в начальную сумму»). Без этого подбитая
    /// стартовая машина не оставляла бы обломка, а бак краулера
    /// «наполнялся» бы стоимостью собственного корпуса.
    pub fn spawn_kind(&mut self, x: i32, z: i32, owner: u8, kind_id: u16) -> Option<u32> {
        self.spawn_kind_inner(x, z, owner, kind_id, true)
    }

    /// То же, но **тело пустое**: материю в него ещё предстоит вложить.
    ///
    /// Так появляются недострой и заказ завода: их материя приходит
    /// переводом из запаса, а не возникает вместе с ними. Заведи их
    /// наполненными — и стройка удваивала бы материю.
    fn spawn_kind_empty(&mut self, x: i32, z: i32, owner: u8, kind_id: u16) -> Option<u32> {
        self.spawn_kind_inner(x, z, owner, kind_id, false)
    }

    fn spawn_kind_inner(
        &mut self,
        x: i32,
        z: i32,
        owner: u8,
        kind_id: u16,
        made: bool,
    ) -> Option<u32> {
        let e = self.econ.as_ref()?;
        let ki = e.kinds.index_of(kind_id)?;
        let d = e.kinds.at(ki).clone();
        let id = self.spawn_ground(x, z, owner, d.speed, d.voxels);
        let k = id as usize;
        self.units.kind[k] = ki as u16;
        if let Some(bodies) = self.material_bodies.as_mut() {
            bodies.spawn_at(id, ki as u16);
        }
        self.units.chassis[k] = d.chassis;
        self.units.weapon[k] = if d.weapon == 0 {
            0
        } else {
            self.weapons.index_of(d.weapon).unwrap_or(0) as u16
        };
        // Постройка не едет и не ходит по полю стороны: она стоит.
        if d.is_building() {
            self.units.stance[k] = STANCE_HOLD;
        }
        // Контейнер заводится **только тем, кому есть что держать**:
        // тело готовой машины, бак краулера, вместимость склада.
        // Контейнер на каждый из пяти тысяч слотов был бы пятью
        // тысячами пустых записей в учёте.
        // Контейнер один на юнита: тело и бак вместе. Тело — это
        // `cost` единиц, бак — остальное, и «сколько везёт» считается
        // вычитанием. Два контейнера на юнита стоили бы вдвое больше
        // записей ради одного вычитания.
        let cap = d.cost.saturating_add(d.tank);
        if let Some(e) = self.econ.as_mut() {
            let held = if made { d.cost } else { 0 };
            let c = e.ledger.create(matter::Kind::Body, held, cap);
            self.units.store[k] = c;
        }
        Some(id)
    }

    /// Сколько юнит везёт сверх собственного тела.
    pub fn cargo(&self, i: usize) -> matter::Amount {
        let Some(e) = self.econ.as_ref() else {
            return 0;
        };
        let c = self.units.store[i];
        let ki = self.units.kind[i];
        if c == economy::NO_CONTAINER || ki == economy::NO_KIND {
            return 0;
        }
        let cost = e.kinds.at(ki as usize).cost;
        let body = self
            .material_bodies
            .as_ref()
            .map_or(cost, |b| b.remaining_matter(i as u32, cost));
        e.ledger.held(c).saturating_sub(body)
    }

    /// Чем кончилась партия.
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// Живых у стороны, считая постройки.
    pub fn living(&self, owner: u8) -> usize {
        self.units
            .live_ids
            .iter()
            .filter(|&&i| self.units.owner[i as usize] == owner)
            .count()
    }

    /// Привязать воксельный мир. До этого фазы земли — заглушки.
    ///
    /// Здесь же индекс соседей узнаёт **настоящий** размер мира.
    /// До этого он строился по `WORLD` — десять тысяч единиц, — а мир
    /// приходит из `worldgen::Params` и бывает вчетверо, а то и
    /// вдесятеро меньше. Обнулялись вёдра мира, которого нет
    /// (ADR-189).
    pub fn attach_ground(&mut self, g: ground::Ground) {
        let (ex, ez) = g.extent();
        let side = ex.max(ez);
        self.grid_near.resize_world(side);
        self.grid_far.resize_world(side);
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
        let Some(g) = self.ground.as_ref() else {
            return 255;
        };
        let st = g.nav.step() as i32;
        let (gw, gh) = g.nav.dims();
        let (cx, cz) = (
            (self.units.x[i] / st).max(0) as u32,
            (self.units.z[i] / st).max(0) as u32,
        );
        if cx >= gw || cz >= gh {
            return 255;
        }
        match g.nav.level_index_near(cx, cz, self.units.y[i]) {
            Some(li) => self
                .nav_field
                .dir_at(self.units.owner[i] as usize % 2, cx, cz, li),
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

    /// Поставить приказ в очередь.
    ///
    /// **Единственный способ снаружи повлиять на симуляцию.** Рендер
    /// состояние читает и никогда не пишет (`CLAUDE.md`, границы
    /// слоёв), мод — тоже (`docs/09-modding.md`): оба ставят запрос
    /// в эту очередь, и применяет его фаза `ApplyCommands` того же
    /// тика у **всех** клиентов lockstep одинаково (ADR-195).
    ///
    /// Приказ не проверяется здесь: между щелчком и тиком юнит успевает
    /// погибнуть, и решать это обязана фаза, а не вызывающий.
    pub fn push_order(&mut self, o: Order) {
        self.orders.push_back(o);
    }

    /// Сколько приказов ждёт применения. Замерщику и окну: очередь,
    /// которая не пустеет, — это упёршийся потолок.
    pub fn orders_pending(&self) -> usize {
        self.orders.len()
    }

    /// Земля для чтения — рендеру и сценам.
    /// Сторона мира по X и Z: у земли своя, без неё — [`WORLD`].
    pub fn extent(&self) -> (i32, i32) {
        self.ground.as_ref().map_or((WORLD, WORLD), |g| g.extent())
    }

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
    /// при стороне 15 на карте 10000 это 445 тысяч корзин, которые надо
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
            contacts: contacts::Contacts::default(),
            contact_group_pairs: 0,
            shot_probe: shot_probe::Probe::default(),
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
            orders: std::collections::VecDeque::new(),
            orders_applied: 0,
            orders_dropped: 0,
            orders_capped: 0,
            econ: None,
            material_bodies: None,
            bursts: burst::Bursts::default(),
            outcome: Outcome::Fighting,
            decided_at: None,
            phase_ns: [0; 16],
            movement_ns: [0; 3],
            navigation_ns: [0; 4],
            combat_ns: [0; 4],
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
        // Слот мог принадлежать погибшей машине с материальным телом:
        // её состояние переживает смерть и снимается только здесь.
        // Юнит без типа тела не имеет, и разбитый корпус прежнего
        // хозяина — без мотора, прицела и ядра — не его (ADR-424).
        // Тип ставит своё тело следом (`spawn_kind_inner`).
        if let Some(b) = self.material_bodies.as_mut() {
            b.despawn(id);
        }
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
                let (dx, dz) = (tx - (cx as i32 * NAV_CELL), tz - (cz as i32 * NAV_CELL));
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
        let Some(g) = self.ground.as_ref() else {
            return;
        };
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
            self.flow_updates = self.nav_field.updates;
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
        self.tick_observed(threads, |_, _| {})
    }

    /// Один тик: все пятнадцать фаз в объявленном порядке. Это тот же
    /// тик на одном воркере: две копии цикла фаз расходились бы молча.
    pub fn tick(&mut self) -> u64 {
        self.tick_par(1)
    }

    /// Тик с наблюдателем: после каждой фазы `after` видит мир только
    /// на чтение. Для проверок состояния, которого граница тика
    /// не показывает; симуляция от наблюдателя не зависит.
    pub(crate) fn tick_observed(
        &mut self,
        threads: usize,
        mut after: impl FnMut(Phase, &World),
    ) -> u64 {
        use std::time::Instant;
        let mut hash = 0u64;
        // События живут один тик: рендер читает их между тиками.
        self.events.clear();
        for (pi, &phase) in PHASES.iter().enumerate() {
            let t0 = Instant::now();
            match phase {
                Phase::ApplyCommands => self.phase_commands(),
                Phase::VoxelEdits => self.phase_ground_edits(),
                Phase::Destruction => self.phase_ground_destruction(),
                Phase::Bodies => self.phase_ground_bodies(),
                Phase::NavUpdate => {
                    self.profile_navigation();
                }
                Phase::Movement => {
                    self.profile_movement(threads);
                }
                Phase::Combat => self.phase_combat(),
                Phase::Logistics => self.phase_logistics(),
                Phase::Economy => self.phase_economy(),
                Phase::Visibility => self.phase_visibility(),
                Phase::Cleanup => self.phase_cleanup(),
                Phase::Hash => hash = self.phase_hash(),
                // Остальные — заглушки (ADR-048): существуют, ничего не делают.
                _ => {}
            }
            self.phase_ns[pi] += t0.elapsed().as_nanos() as u64;
            after(phase, self);
        }
        self.tick += 1;
        hash
    }

    /// Фаза `ApplyCommands`: приказы игрока и мода.
    ///
    /// **Первая в списке фаз, и это не украшение.** Копка ставит
    /// правки в ту же очередь мира, что и всё остальное, а дренажит
    /// её `VoxelEdits` — следующая фаза того же тика. Стой приказ
    /// после движения, юнит шёл бы по приказу прошлого тика, а дыра
    /// появлялась бы тиком позже, чем её увидела навигация
    /// (`CLAUDE.md`: мир меняется для симуляции в тот же тик).
    ///
    /// Потолок — из данных. Упёршийся приказ **откладывается
    /// на следующий тик, а не теряется**: очередь FIFO, и порядок
    /// внутри неё тот, в котором приказы пришли (ADR-195).
    fn phase_commands(&mut self) {
        let cap = self.tuning.max_orders_per_tick.max(1);
        let mut n = 0usize;
        while n < cap {
            let Some(o) = self.orders.pop_front() else {
                break;
            };
            n += 1;
            self.apply_order(o);
        }
        if !self.orders.is_empty() {
            self.orders_capped += 1;
        }
        self.update_engagements();
        self.dig_work();
        if let (Some(g), Some(e)) = (self.ground.as_mut(), self.econ.as_ref()) {
            self.bursts.advance_material(
                &mut g.world,
                &e.mats,
                self.material_bodies.as_mut(),
                &self.tuning,
            );
        }
    }

    /// Применить один приказ.
    ///
    /// Приказ мёртвому, несуществующему или своему же по стороне —
    /// отбрасывается и считается. Это нормальный ход дел, а не ошибка:
    /// между щелчком игрока и тиком, на котором приказ применяется,
    /// адресат успевает погибнуть, и у клиентов lockstep это
    /// происходит одинаково.
    fn apply_order(&mut self, o: Order) {
        let i = o.unit() as usize;
        if i >= self.units.len() || !self.units.alive[i] {
            self.orders_dropped += 1;
            return;
        }
        // **Точка приказа проверяется до того, как станет состоянием.**
        // Приказ приходит от игрока, мода и пира lockstep, а дальше его
        // координаты идут в квадраты расстояний в `i32`: `MoveTo`
        // с `x = 100 000` ронял фазу движения переполнением у всех
        // клиентов сразу — ровно тот DoS, от которого канал приказов
        // и заведён. Ход и атака с ходом прижимаются к краю мира:
        // щелчок за картой — это «до края». Копка и стройка за краем
        // смысла не имеют и отбрасываются (ADR-417).
        let (ex, ez) = self.extent();
        let ny = self.ground.as_ref().map(|g| g.world.ny as i32);
        let inside = |x: i32, y: i32, z: i32| {
            (0..ex).contains(&x) && (0..ez).contains(&z) && ny.is_none_or(|n| (0..n).contains(&y))
        };
        let tick = self.tick as u32;
        let period = self.tuning.retarget_period;
        let u = &mut self.units;
        match o {
            Order::MoveTo { x, z, .. } => {
                u.stance[i] = STANCE_MOVE;
                u.goal_x[i] = x.clamp(0, ex - 1);
                u.goal_y[i] = 0;
                u.goal_z[i] = z.clamp(0, ez - 1);
            }
            Order::AttackMove { x, z, .. } => {
                u.stance[i] = STANCE_ATTACK_MOVE;
                u.goal_x[i] = x.clamp(0, ex - 1);
                u.goal_y[i] = 0;
                u.goal_z[i] = z.clamp(0, ez - 1);
            }
            Order::Hold { .. } => u.stance[i] = STANCE_HOLD,
            Order::Release { .. } => u.stance[i] = STANCE_FLOW,
            Order::Attack { target, .. } => {
                let t = target as usize;
                if t >= u.len() || !u.alive[t] || u.owner[t] == u.owner[i] {
                    self.orders_dropped += 1;
                    return;
                }
                // Цель назначена игроком — фаза боя не вправе сменить
                // её на ближайшую в тот же тик. Срок перевыбора тот же,
                // что у выбранной самим юнитом: приказ держится период,
                // дальше юнит снова смотрит сам (ADR-116).
                u.target[i] = target;
                u.next_search[i] = tick.wrapping_add(period);
                // И идти к ней. Точка — та, где цель **сейчас**:
                // преследование сделало бы приказ вечным, а игрок
                // щёлкает заново, и это дешевле (ADR-195).
                u.stance[i] = STANCE_ATTACK_MOVE;
                u.goal_x[i] = u.x[t];
                u.goal_y[i] = 0;
                u.goal_z[i] = u.z[t];
            }
            Order::Dig { x, y, z, .. } => {
                if !inside(x, y, z) {
                    self.orders_dropped += 1;
                    return;
                }
                u.stance[i] = STANCE_DIG;
                u.goal_x[i] = x;
                u.goal_y[i] = y;
                u.goal_z[i] = z;
                // Забой запоминается сразу: отвезя груз, копатель
                // вернётся именно сюда, а не туда, где его застал
                // полный бак.
                u.face[i] = (x, y, z);
                u.job[i] = NO_TARGET;
            }
            Order::Build { kind, x, z, .. } => {
                if !inside(x, 0, z) || !self.place_site(i, kind, x, z) {
                    self.orders_dropped += 1;
                    return;
                }
            }
            Order::Produce { kind, .. } => {
                if !self.enqueue_product(i, kind) {
                    self.orders_dropped += 1;
                    return;
                }
            }
            Order::Cancel { .. } => {
                if !self.cancel_product(i) {
                    self.orders_dropped += 1;
                    return;
                }
            }
        }
        self.orders_applied += 1;
    }

    /// Разметить недострой и послать заказчика его строить.
    ///
    /// Недострой — **обычное тело-контейнер с частично заполненным
    /// томом** (`docs/07-economy.md`), то есть обычный юнит, которому
    /// не хватает материи. Отдельной сущности «фундамент» нет, и
    /// поэтому отмена стройки не требует ничего нового: недострой
    /// гибнет как всякое тело.
    ///
    /// Материя **не списывается здесь**. Она льётся строителями,
    /// тик за тиком: «недострой считается в потолок по уже вложенной
    /// материи».
    fn place_site(&mut self, by: usize, kind_id: u16, x: i32, z: i32) -> bool {
        let owner = self.units.owner[by];
        let Some(e) = self.econ.as_ref() else {
            return false;
        };
        let Some(ki) = e.kinds.index_of(kind_id) else {
            return false;
        };
        let d = e.kinds.at(ki);
        // Строят только постройки. Машина выходит из завода, и класть
        // её фундамент посреди карты значило бы два пути к одному
        // результату — они бы разошлись.
        if !d.is_building() {
            return false;
        }
        // Строитель обязан уметь строить: иначе недострой встал бы
        // и стоял, а приказ выглядел бы исполненным.
        let bki = self.units.kind[by];
        if bki == economy::NO_KIND {
            return false;
        }
        if e.kinds.at(bki as usize).build_rate == 0 {
            return false;
        }
        // **Недострой на этом месте уже есть — вставать к нему,
        // а не класть второй.** Приказ отдаётся каждому выделенному
        // юниту отдельно, и без этого щелчок по месту при двадцати
        // выделенных краулерах разметил бы двадцать фундаментов
        // в одной точке. Заодно это и защита от мода: цикл «размечай
        // фундаменты» иначе становится способом занять слоты юнитов.
        //
        // Ничья — по **меньшему номеру слота** (`CLAUDE.md`).
        let reach = self.tuning.work_reach;
        let mut join = NO_TARGET;
        {
            // Список недостроев — свой, короткий: обходить ради этого
            // все слоты значило бы умножить потолок приказов на их
            // число. **Список живых тут не годится**: его пересобирает
            // `NavUpdate` в начале тика, а недострой заводится
            // в `ApplyCommands`, и второй приказ того же щелчка его
            // бы не увидел.
            let e = self.econ.as_ref().expect("экономика");
            for &j in &e.sites {
                let j = j as usize;
                if !self.units.alive[j]
                    || self.units.site[j] == 0
                    || self.units.owner[j] != owner
                    || self.units.kind[j] != ki as u16
                {
                    continue;
                }
                let (dx, dz) = (self.units.x[j] - x, self.units.z[j] - z);
                if dx * dx + dz * dz <= reach * reach {
                    join = j as u32;
                    break;
                }
            }
        }
        if join != NO_TARGET {
            let k = join as usize;
            self.units.stance[by] = STANCE_BUILD;
            self.units.goal_x[by] = self.units.x[k];
            self.units.goal_y[by] = self.units.y[k];
            self.units.goal_z[by] = self.units.z[k];
            self.units.job[by] = join;
            return true;
        }
        let Some(site) = self.spawn_kind_empty(x, z, owner, kind_id) else {
            return false;
        };
        let k = site as usize;
        let cost = self
            .econ
            .as_ref()
            .expect("экономика")
            .kinds
            .at(self.units.kind[k] as usize)
            .cost;
        self.units.site[k] = cost;
        // Недострой начинается с одного воксела: ноль означал бы
        // мёртвого при рождении, и уборка снесла бы его тем же тиком.
        self.units.voxels[k] = 1;
        self.econ.as_mut().expect("экономика").add_site(site);
        self.units.stance[by] = STANCE_BUILD;
        self.units.goal_x[by] = x;
        self.units.goal_y[by] = self.units.y[k];
        self.units.goal_z[by] = z;
        self.units.job[by] = site;
        true
    }

    /// Поставить заказ в очередь завода.
    fn enqueue_product(&mut self, at: usize, kind_id: u16) -> bool {
        let cap = self.tuning.max_build_queue;
        let unit = at as u32;
        let fki = self.units.kind[at];
        let Some(e) = self.econ.as_mut() else {
            return false;
        };
        if fki == economy::NO_KIND {
            return false;
        }
        // Завод обязан уметь делать именно это. Список лежит в данных,
        // и проверять его здесь — единственный способ не дать очереди
        // набраться заказами, которые никогда не выйдут.
        if !e.kinds.at(fki as usize).produces.contains(&kind_id) {
            return false;
        }
        let Some(ki) = e.kinds.index_of(kind_id) else {
            return false;
        };
        let line = e.line_mut(unit);
        if line.queue.len() >= cap {
            return false;
        }
        line.queue.push_back(ki as u16);
        true
    }

    /// Снять с завода последний заказ. Передний снимается тоже —
    /// вместе с возвратом уплаченного.
    fn cancel_product(&mut self, at: usize) -> bool {
        let unit = at as u32;
        let owner = self.units.owner[at] as usize;
        let Some(e) = self.econ.as_mut() else {
            return false;
        };
        let Some(stock) = e.banks.get(owner).map(|b| b.stock) else {
            return false;
        };
        let Ok(li) = e.lines.binary_search_by_key(&unit, |l| l.unit) else {
            return false;
        };
        if e.lines[li].queue.is_empty() {
            return false;
        }
        // Снимается **последний**, а не передний: игрок отменяет то,
        // что заказал по ошибке, а не то, что уже собирается. Если он
        // один — он же и передний, и заготовка уходит обратно в запас.
        e.lines[li].queue.pop_back();
        if e.lines[li].queue.is_empty() {
            // Отмена — **не сток**: вложенное возвращается в запас.
            // Иначе цикл «заказал — отменил» стал бы способом сжигать
            // материю мимо перечня стоков (`docs/07-economy.md`).
            e.drop_line(unit, stock);
        }
        // Очередь не пуста — передний заказ продолжается, и его
        // заготовку трогать не за что.
        true
    }

    /// Работа по стоячему приказу копать.
    ///
    /// Копатель, дошедший до забоя, снимает шар породы и **уходит
    /// на воксель вглубь**. Забой, который не углубляется, кончается
    /// за один тик: шарик снимает воздух, правок не возникает, и копка
    /// превращается в неподвижную картинку — ровно то, от чего лечил
    /// эталонный сценарий ADR-159.
    ///
    /// Правки идут в **ту же очередь мира**, что у всех остальных,
    /// и упираются в тот же потолок за тик: у приказа игрока нет
    /// обхода `MAX_VOXEL_EDITS_PER_TICK` (ADR-013).
    fn dig_work(&mut self) {
        let r = self.tuning.dig_radius;
        let reach = self.tuning.order_arrive;
        if r <= 0 {
            return;
        }
        let Self {
            units: u,
            ground,
            econ,
            ..
        } = self;
        let Some(g) = ground.as_mut() else { return };
        let (nx, ny, nz) = (g.world.nx as i32, g.world.ny as i32, g.world.nz as i32);
        let mut cur = voxel::World::mat_cursor();
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            if !u.alive[i] || u.stance[i] != STANCE_DIG {
                continue;
            }
            let (gx, gy, gz) = (u.goal_x[i], u.goal_y[i], u.goal_z[i]);
            let (ax, az) = (u.x[i] - gx, u.z[i] - gz);
            if ax * ax + az * az > reach * reach {
                // Ещё в дороге: идёт он фазой движения, как все.
                continue;
            }
            // **Копатель с полным баком не копает.** Канон
            // (`docs/07-economy.md`): «правка не применяется, работа
            // копателя стоит и прогресс не набирается». Наивная
            // альтернатива — снять воксель и не принять материю —
            // была бы стоком вне перечня.
            //
            // Отсюда и место в баке считается **здесь**, до постановки
            // правок: снять больше, чем влезет, нельзя, а узнать это
            // после дренажа уже поздно.
            let mut room = matter::Amount::MAX;
            let by = match econ.as_ref() {
                Some(e) if u.store[i] != economy::NO_CONTAINER => {
                    room = e.ledger.room(u.store[i]);
                    if room == 0 {
                        continue;
                    }
                    i as u32
                }
                _ => voxel::NOBODY,
            };
            // Места не хватило на следующий воксель — это и есть
            // «бак полон». Считать полным `room == 0` нельзя: остаток
            // почти никогда не делится на плотность материала нацело,
            // и копатель вставал бы с непустым остатком навсегда.
            let mut full = false;
            'ball: for dy in -r..=r {
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy + dz * dz > r * r {
                            continue;
                        }
                        let (x, y, z) = (gx + dx, gy + dy, gz + dz);
                        if x < 0 || y < 0 || z < 0 || x >= nx || y >= ny || z >= nz {
                            continue;
                        }
                        // **В очередь идёт только порода.** Шар в метр —
                        // 4 169 вокселей, и после первого тика почти все
                        // они воздух: пустая правка ничего не снимает,
                        // а просмотр при дренаже занимает (ADR-182).
                        // Дно мира — рамка, и её `queue_edit` отбросит
                        // сам (ADR-034): колонка без пола — это юнит,
                        // зависший в воздухе навсегда.
                        if !g.world.solid(x as u32, y as u32, z as u32) {
                            continue;
                        }
                        if by != voxel::NOBODY {
                            let m = g
                                .world
                                .material_cached(&mut cur, x as u32, y as u32, z as u32);
                            if m == voxel::store::AIR {
                                continue;
                            }
                            let per = econ.as_ref().expect("экономика").per_voxel(m);
                            if per > room {
                                full = true;
                                break 'ball;
                            }
                            room -= per;
                        }
                        g.world.queue_dig_by(x as u32, y as u32, z as u32, by);
                    }
                }
            }
            if full {
                // Слой не догрызен — забой не углубляется, и, разгрузившись,
                // копатель вернётся ровно сюда.
                u.stance[i] = STANCE_HAUL;
                u.job[i] = NO_TARGET;
                u.face[i] = (gx, gy, gz);
                continue;
            }
            u.goal_y[i] -= 1;
            // Забой уходит вглубь вместе с приказом: отвезя груз,
            // копатель вернётся туда, где остановился, а не наверх.
            u.face[i] = (gx, u.goal_y[i], gz);
            // Догрыз до дна — приказ исчерпан, юнит стоит. Вернуть его
            // под поле стороны нельзя: он ушёл бы с забоя сам, а игрок
            // этого не просил.
            if u.goal_y[i] <= 1 {
                u.stance[i] = STANCE_HOLD;
            }
        }
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
        let free = self.tuning.max_step;
        let Self {
            ground, units: u, ..
        } = self;
        let Some(g) = ground.as_mut() else { return };

        // 1. Посадка на свежие тела: кто оказался над куском, едет
        //    на нём (ADR-099).
        for si in 0..g.bodies.spawned.len() {
            let id = g.bodies.spawned[si];
            let Some(bi) = g.bodies.index_of(id) else {
                continue;
            };
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
            // сколько пролетело и где легло. Наездник ложится с ним
            // (ADR-435): он стоял на высоте прошлого тика тела, на шаг
            // выше места посадки, и падал этот шаг ещё раз сам — с уроном
            // сверх полученного за полёт тела.
            let landed = g.bodies.landed_log.iter().find(|l| l.id == u.riding[i]);
            let fallen = landed.map_or(0, |l| l.fallen);
            if let Some(l) = landed {
                u.y[i] = (l.oy + u.ride_off[i].1 as i64).max(0) as i32;
            }
            let hit = fall_damage(fallen, free, dmg);
            u.voxels[i] = u.voxels[i].saturating_sub(hit.min(u16::MAX as u32) as u16);
            g.stats.fall_damage += hit as u64;
            g.stats.max_fall = g.stats.max_fall.max(fallen);
            u.riding[i] = 0;
        }
        if let (Some(g), Some(e)) = (self.ground.as_ref(), self.econ.as_mut()) {
            self.bursts.confirm(&g.completed, e, &self.tuning.blast);
            self.bursts.step_fragments(
                &g.world,
                e,
                &self.tuning.blast,
                gravity_per_tick(&self.tuning),
                self.tick,
                burst::Surroundings::NONE,
            );
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
            let measured = g.phase_nav();
            self.navigation_ns[0] += measured[0];
            self.navigation_ns[1] += measured[1];
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

    fn profile_navigation(&mut self) {
        use std::time::Instant;
        self.phase_ground_nav();
        let t0 = Instant::now();
        self.phase_flow();
        self.navigation_ns[2] += t0.elapsed().as_nanos() as u64;
        let t0 = Instant::now();
        self.phase_nav_update();
        self.navigation_ns[3] += t0.elapsed().as_nanos() as u64;
    }

    fn profile_movement(&mut self, threads: usize) {
        use std::time::Instant;
        let t0 = Instant::now();
        self.movement_deltas(threads);
        self.movement_ns[0] += t0.elapsed().as_nanos() as u64;
        let t0 = Instant::now();
        self.apply_deltas();
        self.movement_ns[1] += t0.elapsed().as_nanos() as u64;
        let t0 = Instant::now();
        self.phase_ride();
        self.movement_ns[2] += t0.elapsed().as_nanos() as u64;
    }

    /// Подвеска: наклон корпуса от высот пола под точками опоры.
    ///
    /// Идёт **после** хода: наклон отвечает на то, где машина
    /// оказалась, а не на то, где была. Навигационный уровень остаётся
    /// дискретным, физическая высота корпуса интегрируется отдельно.
    fn phase_ride(&mut self) {
        let Some(g) = self.ground.as_ref() else {
            return;
        };
        let nav = &g.nav;
        let (ex, ez) = g.extent();
        let (nw, nh) = nav.dims();
        let dt = Fx::from_ratio(1, TICKS_PER_SECOND);
        let (mut impact_damage, mut longest_fall) = (0u64, 0u32);
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k] as usize;
            let kind = self.units.chassis[i];
            let c = self.tuning.chassis(kind);
            let mut r = ride::ride_of(c);
            if self.units.speed[i] > 0 {
                r.gravity = Fx::from_int(self.tuning.gravity);
                r.preload = r.gravity.mul(Fx::from_ratio(1, 4));
            }
            let r = self
                .material_bodies
                .as_ref()
                .and_then(|b| b.state(i as u32))
                .map_or(r, |s| s.mobility.ride(r));
            let sup = ride::supports_of(c);
            let (hc, hs) = ride::heading_of(self.units.head[i]);
            let body_before = self.units.body_y(i);
            let near = self.units.body_cell_y(i);
            let mut floors = [Fx::ZERO; 4];
            for (f, sp) in floors.iter_mut().zip(sup.iter()) {
                // Местная опора — в мировые координаты. Поворот только
                // вокруг вертикали: наклон учитывает сама физика.
                let rx = sp.x.mul(hc).sub(sp.z.mul(hs));
                let rz = sp.x.mul(hs).add(sp.z.mul(hc));
                let px = (self.units.x[i] + rx.to_int_trunc()).clamp(0, ex - 1) as u32;
                let pz = (self.units.z[i] + rz.to_int_trunc()).clamp(0, ez - 1) as u32;
                // Размер нав-клетки берётся **у самого слоя**, а не
                // из постоянной: он приходит из данных, и при другом
                // ребре воксела постоянная промахивается мимо сетки
                // в пять раз — выход за край на первом же тике.
                let (cx, cz) = (
                    (px * nw / ex.max(1) as u32).min(nw - 1),
                    (pz * nh / ez.max(1) as u32).min(nh - 1),
                );
                // Уровень **ближайший к машине**, а не первый: юнит
                // может стоять на крыше, и пол под ней — не пол ямы
                // (ADR-136).
                let mut best = nav.floor_at(cx, cz) as i32;
                let mut gap = (best - near).abs();
                for &l in nav.levels_at(cx, cz) {
                    let d = (l as i32 - near).abs();
                    if d < gap {
                        gap = d;
                        best = l as i32;
                    }
                }
                *f = Fx::from_int(best);
            }
            // Ускорение корпуса — из приращений хода за этот же тик:
            // с ним торможение роняет носом, поворот кренит наружу.
            let sway = ride::sway_of(
                self.units.accel[i],
                self.units.vel[i],
                self.units.yaw_step[i],
                self.units.head[i],
            );
            if self.units.speed[i] == 0 {
                ride::step_hull(
                    &mut self.units.up[i],
                    &mut self.units.spin[i],
                    &mut self.units.load[i],
                    &floors,
                    self.units.head[i],
                    sway,
                    &r,
                    &sup,
                    dt,
                );
            } else if self.units.riding[i] != 0 {
                // Only moving platforms own a separate vertical path.
                self.units.heave_v[i] = Fx::ZERO;
                self.units.load[i] = Fx::ONE;
            } else {
                let level = Fx::from_int(self.units.y[i]);
                let mut h = phys::Chassis {
                    y: body_before.add(r.clearance),
                    vy: self.units.heave_v[i],
                    up: self.units.up[i],
                    w: self.units.spin[i],
                    load: self.units.load[i],
                };
                phys::step(&mut h, &sup, &floors, (hc, hs), sway, &r, dt);
                self.units.heave[i] = h.y.sub(r.clearance).sub(level);
                self.units.heave_v[i] = h.vy;
                self.units.up[i] = h.up;
                self.units.spin[i] = h.w;
                self.units.load[i] = h.load;
                self.units.fall_f[i] = Fx::ZERO;
                // Contact, not a missing floor under the centre, owns flight.
                // Track the highest body position; a navigation rebase cannot
                // manufacture a drop or landing damage (ADR-253).
                if h.grounded() {
                    if self.units.fall_from[i] != NOT_FALLING {
                        let fallen =
                            (self.units.fall_from[i] - self.units.body_cell_y(i)).max(0) as u32;
                        let hit = fall_damage(
                            fallen,
                            self.tuning.max_step,
                            self.tuning.fall_damage_per_voxel,
                        );
                        self.units.voxels[i] =
                            self.units.voxels[i].saturating_sub(hit.min(u16::MAX as u32) as u16);
                        impact_damage += hit as u64;
                        longest_fall = longest_fall.max(fallen);
                    }
                    self.units.fall_from[i] = NOT_FALLING;
                    self.units.fall_v[i] = Fx::ZERO;
                } else {
                    let apex = body_before.max(self.units.body_y(i)).to_int_trunc();
                    self.units.fall_from[i] = self.units.fall_from[i].max(apex);
                    self.units.fall_v[i] =
                        h.vy.neg().max(Fx::ZERO).div(Fx::from_int(TICKS_PER_SECOND));
                }
            }
        }
        if let Some(g) = self.ground.as_mut() {
            g.stats.fall_damage += impact_damage;
            g.stats.max_fall = g.stats.max_fall.max(longest_fall);
        }
    }

    /// Prepare the phase snapshot AND its exact contact sums. This serial
    /// work is part of movement; a benchmark timing only movement_kernel
    /// measures direction lookup and clamp, not the full contact workload.
    /// The caller must first refresh live ids and rebuild grid_near from the
    /// same current positions, as phase_nav_update does in the actual tick.
    pub fn prepare_movement(&mut self) {
        self.snap_x.clear();
        self.snap_x.extend_from_slice(&self.units.x);
        self.snap_z.clear();
        self.snap_z.extend_from_slice(&self.units.z);
        self.contacts.prepare(
            &self.grid_near,
            &self.snap_x,
            &self.snap_z,
            self.tuning.separation * self.tuning.separation,
        );
        self.delta.clear();
        self.delta
            .resize(self.units.x.len(), drive::Want::default());
    }

    /// Чего хочет один юнит. Читает снимок, поле и готовую сумму контактов —
    /// значит вызов из любого потока даёт то же самое.
    ///
    /// **Это желание, а не смещение** (ADR-220): куда ехать и куда
    /// толкают соседи. Во что оно превратится, решает трансмиссия
    /// в серийной фазе, потому что ей нужно состояние хода.
    ///
    /// Источник направления у ядра и у фазы **один и тот же** —
    /// [`want_way`]. Пока их было два, они разошлись: ядро читало
    /// плоское поле всегда, а фаза при наличии земли — объёмное,
    /// и метод, документированный как «то же самое», считал другое
    /// (ADR-194).
    #[inline]
    pub fn movement_kernel(&self, i: usize) -> drive::Want {
        let (ux, uz) = (self.snap_x[i], self.snap_z[i]);
        let vol = self.ground.as_ref().map(|g| (&g.nav, &self.nav_field));
        let way = want_way(
            &self.units,
            i,
            ux,
            uz,
            vol,
            &self.flow,
            self.tuning.order_arrive,
        );
        let (px, pz) = self.contacts.push(i, self.units.speed[i]);
        drive::Want {
            way,
            push_x: px,
            push_z: pz,
        }
    }

    /// Живые ли слот. Нужен пулу, чтобы не считать дырки free-list.
    #[inline]
    pub fn slot_alive(&self, i: usize) -> bool {
        self.units.alive[i]
    }

    pub fn slots(&self) -> usize {
        self.units.x.len()
    }

    /// Забрать буфер желаний — пул пишет в свой кусок.
    pub fn take_delta(&mut self) -> Vec<drive::Want> {
        std::mem::take(&mut self.delta)
    }

    pub fn put_delta(&mut self, d: Vec<drive::Want>) {
        self.delta = d;
    }

    /// Посчитать желания. Читает **только снимок**, пишет только
    /// в свой слот `delta` — значит фаза делится на потоки по слотам
    /// и результат от разбиения не зависит.
    fn movement_deltas(&mut self, threads: usize) {
        self.prepare_movement();
        self.examined_move += self.contacts.equivalent_pairs;
        self.contact_group_pairs += self.contacts.group_pairs;
        let u = &self.units;
        let (sx, sz) = (&self.snap_x, &self.snap_z);
        let contacts = &self.contacts;
        let flow = &self.flow;
        // Земля есть — направление берётся из **объёмного** поля по узлу
        // «клетка и уровень»: юнит на крыше идёт по крышам, юнит
        // в туннеле — по туннелю (ADR-138). Без земли поле прежнее,
        // плоское: эталонный сценарий воксельного мира не строит.
        let vol = self.ground.as_ref().map(|g| (&g.nav, &self.nav_field));

        // Точка приказа читается **из состояния юнита**, а само
        // состояние приказы за эту фазу не меняют: их фаза прошла
        // первой. Значит и здесь читается снимок, и разбиение фазы
        // на потоки ничего не меняет (ADR-195).
        let arrive = self.tuning.order_arrive;
        let compute = |i: usize| -> drive::Want {
            let (ux, uz) = (sx[i], sz[i]);
            let way = want_way(u, i, ux, uz, vol, flow, arrive);
            let (px, pz) = contacts.push(i, u.speed[i]);
            drive::Want {
                way,
                push_x: px,
                push_z: pz,
            }
        };

        let live = &u.live_ids;
        if threads <= 1 {
            for &i in live.iter() {
                let i = i as usize;
                self.delta[i] = compute(i);
            }
            return;
        }

        // Разбиение по слотам: каждый поток пишет в свой кусок `delta`,
        // куски не пересекаются, и порядок внутри куска фиксирован.
        let n = self.delta.len();
        let block = n.div_ceil(threads);
        let mut rest: &mut [drive::Want] = &mut self.delta;
        let mut parts: Vec<(usize, &mut [drive::Want])> = Vec::with_capacity(threads);
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

    /// Применить желания. Серийная фаза: трансмиссия, интегрирование
    /// в `Fx` и перенос целой части в сетку.
    ///
    /// Серийная она потому, что читает и пишет **состояние хода** —
    /// курс и скорость, — а состояние за фазу меняется. Параллельная
    /// часть своё уже посчитала и лежит в `delta`.
    fn apply_deltas(&mut self) {
        let fall_dmg = self.tuning.fall_damage_per_voxel;
        // Бесплатный уступ — тот же, что перешагивает навигация
        // (`max_step`, ADR-121): ниже него спуск — шаг, выше — падение.
        let free = self.tuning.max_step;
        // Ускорение за тик считается **один раз на фазу**, а не на юнита:
        // это точное деление в `i128`, и повторять его пять тысяч раз
        // незачем.
        let g_tick = gravity_per_tick(&self.tuning);
        // Пределы хода — тоже один раз на фазу и по одному на вид
        // ходовой, а не по одному на юнита (ADR-220).
        let limits = [
            Limits::of(&self.tuning.tracked, TICKS_PER_SECOND),
            Limits::of(&self.tuning.walking, TICKS_PER_SECOND),
        ];
        let inv_tick = Fx::from_ratio(1, TICKS_PER_SECOND);
        let (climb, descend) = (
            [self.tuning.tracked.climb, self.tuning.walking.climb],
            [self.tuning.tracked.descend, self.tuning.walking.descend],
        );
        // Циклов шага на мировую единицу пути — по виду ходовой.
        // `None` у гусеничной: у неё нет походки, и фаза шага стоит.
        let stride = [&self.tuning.tracked, &self.tuning.walking].map(drive::StepCycle::of);
        let Self {
            units: u,
            delta,
            ground,
            econ,
            material_bodies,
            blocked_moves,
            ..
        } = self;
        // Счётчики падения — те же, что у наездников: правило одно,
        // значит и отчёт один.
        let (mut fall_damage_acc, mut max_fall_acc) = (0u64, 0u32);
        let (dmg_acc, max_acc) = (&mut fall_damage_acc, &mut max_fall_acc);
        // Земля, если она есть, решает, куда можно шагнуть. Перепад —
        // свойство ребра (ADR-121), поэтому спрашивается переход,
        // а не клетка.
        let terrain = ground.as_ref().map(|g| (&g.nav, g.extent()));
        // Множители по массе — из учёта; без экономики их нет вовсе,
        // и все идут номиналом.
        let drive_by_mass: &[Fx] = match econ.as_ref() {
            Some(e) => &e.drive,
            None => &[],
        };
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let want = delta[i];
            let (x0, z0) = (u.x[i], u.z[i]);
            let y0 = u.y[i];
            let was_falling = u.fall_from[i] != NOT_FALLING;
            let mut placed = false;
            let kind = u.chassis[i].code() as usize;

            // **Предельная скорость с учётом уклона.** Смотрится
            // клетка на шаг вперёд по курсу: вверх медленнее, вниз
            // быстрее. Слишком крутой перепад ребра не имеет вовсе
            // (`max_step`, ADR-121) — туда машина просто не едет,
            // и отдельного правила для этого не нужно.
            let state = material_bodies.as_ref().and_then(|b| b.state(i as u32));
            let powered = state.is_none_or(|s| {
                s.status.engine && s.status.alive && s.mobility.traction > Fx::ZERO
            });
            let engine = state.map_or(Fx::ONE, |s| s.mobility.engine.mul(s.mobility.traction));
            let mut vmax = if powered {
                Fx::from_int(u.speed[i]).mul(engine)
            } else {
                Fx::ZERO
            };
            if u.riding[i] == 0 {
                if let Some((nav, _)) = terrain {
                    let pct =
                        slope_percent(nav, x0, z0, u.y[i], u.head[i], climb[kind], descend[kind]);
                    vmax = vmax.mul(Fx::from_ratio(pct, 100));
                }
            }

            // Трансмиссия: курс доворачивается, скорость меняется,
            // и только из них получается шаг. Множитель — по массе
            // тела (ADR-234): крупная машина того же вида разгоняется
            // и доворачивает медленнее. Юнит без типа идёт номиналом
            // вида: так живут стенды и эталонный сценарий.
            let ki = u.kind[i];
            let factor = if ki == economy::NO_KIND {
                Fx::ONE
            } else {
                drive_by_mass.get(ki as usize).copied().unwrap_or(Fx::ONE)
            };
            let want_way = if powered { want.way } else { NO_WAY };
            let response = state.map_or(limits[kind], |s| {
                s.mobility.limits(&limits[kind], want_way == NO_WAY)
            });
            let (nd, mv) = drive::advance(
                Drive {
                    head: u.head[i],
                    vel: u.vel[i],
                    yaw_rate: u.yaw_rate[i],
                },
                want_way,
                vmax,
                &response,
                factor,
                inv_tick,
            );
            // Приращения — подвеске (ADR-225): продольное ускорение
            // и шаг курса за тик. Считаются здесь, где скорость и курс
            // меняются, а не восстанавливаются из прошлого тика
            // отдельным буфером.
            u.accel[i] = nd.vel.sub(u.vel[i]).mul(Fx::from_int(TICKS_PER_SECOND));
            u.yaw_step[i] = (nd.head.wrapping_sub(u.head[i])) as u16 as i16 as i32;
            u.yaw_rate[i] = nd.yaw_rate;
            u.head[i] = nd.head;
            u.vel[i] = nd.vel;

            // Толчок соседей прибавляется **к шагу**, а не к тяге:
            // это контакт тел, и разворачиваться ради него машина
            // не обязана (ADR-220).
            let step_x = mv.dx.add(Fx::from_ratio(want.push_x, TICKS_PER_SECOND));
            let step_z = mv.dz.add(Fx::from_ratio(want.push_z, TICKS_PER_SECOND));
            let (fx0, fz0) = (u.fx[i], u.fz[i]);
            let mut drop_horizontal = false;
            let nx = u.fx[i].add(step_x);
            let nz = u.fz[i].add(step_z);
            let carry_x = nx.to_int_trunc();
            let carry_z = nz.to_int_trunc();
            u.fx[i] = nx.sub(Fx::from_int(carry_x));
            u.fz[i] = nz.sub(Fx::from_int(carry_z));
            let mut x1 = (x0 + carry_x).clamp(0, WORLD - 1);
            let mut z1 = (z0 + carry_z).clamp(0, WORLD - 1);
            if was_falling && u.speed[i] > 0 && u.riding[i] == 0 {
                x1 = x0;
                z1 = z0;
                drop_horizontal = true;
            }
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
                let unsupported =
                    u.riding[i] == 0 && here.is_some_and(|c| !nav.has_level(c.0, c.1, y));
                if unsupported {
                    drop_horizontal = true;
                    let c = here.expect("клетка есть");
                    if u.speed[i] > 0 {
                        // Rebase navigation only. The same ride solver handles
                        // both supported motion and free flight for mobile bodies.
                        if let Some(l) = nav.level_below(c.0, c.1, y) {
                            y = l as i32;
                        } else if let Some(&l) = nav.levels_at(c.0, c.1).first() {
                            y = l as i32;
                            placed = true;
                            u.heave[i] = Fx::ZERO;
                            u.heave_v[i] = Fx::ZERO;
                            u.fall_from[i] = NOT_FALLING;
                            u.fall_v[i] = Fx::ZERO;
                            u.fall_f[i] = Fx::ZERO;
                        }
                        x1 = x0;
                        z1 = z0;
                    } else {
                        match nav.level_below(c.0, c.1, y) {
                            Some(l) => {
                                let l = l as i32;
                                // Падает по вертикали: горизонтальный шаг
                                // на этом тике отменяется.
                                x1 = x0;
                                z1 = z0;
                                if y - l <= free as i32 && u.fall_from[i] == NOT_FALLING {
                                    // **Уступ — шаг вниз, а не падение**
                                    // (ADR-036: «читается как шаг вниз»).
                                    // Копатель снимает воксель под собой всю
                                    // игру, и заставлять его треть секунды
                                    // висеть над собственной ямой — это
                                    // не физика, а заикание (ADR-174).
                                    // Уступ — `max_step` из данных, а не
                                    // воксель: на ребре 0.1 м воксель —
                                    // десять сантиметров, а навигация
                                    // считает проходимым полуметровый
                                    // перепад (ADR-227).
                                    y = l;
                                } else {
                                    if u.fall_from[i] == NOT_FALLING {
                                        u.fall_from[i] = y;
                                        u.fall_v[i] =
                                            u.heave_v[i].neg().mul(inv_tick).max(Fx::ZERO);
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
                                        let hit = fall_damage(fallen, free, fall_dmg);
                                        u.voxels[i] = u.voxels[i]
                                            .saturating_sub(hit.min(u16::MAX as u32) as u16);
                                        *dmg_acc += hit as u64;
                                        *max_acc = (*max_acc).max(fallen);
                                        u.fall_from[i] = NOT_FALLING;
                                        u.heave_v[i] =
                                            u.fall_v[i].neg().mul(Fx::from_int(TICKS_PER_SECOND));
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
                                placed = true;
                                u.heave[i] = Fx::ZERO;
                                u.heave_v[i] = Fx::ZERO;
                                u.fall_from[i] = NOT_FALLING;
                                u.fall_v[i] = Fx::ZERO;
                                u.fall_f[i] = Fx::ZERO;
                                x1 = x0;
                                z1 = z0;
                            }
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
            // A rejected axis must reject its fractional motion too. Otherwise
            // the rendered pose oscillates through the wall between whole cells.
            let ax = !drop_horizontal && x1 == x0 + carry_x;
            // Crossing a walkable step changes the navigation level, not
            // the physical height instantaneously, including during flight.
            // Initial placement is an explicit teleport.
            if !placed && u.riding[i] == 0 && u.speed[i] > 0 {
                u.heave[i] = u.heave[i].sub(Fx::from_int(u.y[i] - y0));
            }
            let az = !drop_horizontal && z1 == z0 + carry_z;
            if !ax {
                u.fx[i] = fx0;
            }
            if !az {
                u.fz[i] = fz0;
            }
            // Only accepted drive motion advances the gait. Contact pushes do
            // not become an extra stride. A single accepted axis needs abs,
            // both axes use the already known speed; no square root per unit.
            if let Some(st) = stride[kind] {
                let path = match (ax, az) {
                    (true, true) => nd.vel.mul(inv_tick).abs(),
                    (true, false) => mv.dx.abs(),
                    (false, true) => mv.dz.abs(),
                    (false, false) => Fx::ZERO,
                };
                u.gait[i] = u.gait[i].wrapping_add(
                    st.advance(path, if drop_horizontal { 0 } else { u.yaw_step[i] }),
                ) & (fx::angle::FULL - 1);
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
        use std::time::Instant;
        let t0 = Instant::now();
        // Сначала летят те, кто уже в воздухе, потом стреляют новые:
        // иначе снаряд успевал бы родиться и попасть в одном тике,
        // то есть время полёта было бы нулевым у первого шага.
        self.step_shots();
        self.combat_ns[0] += t0.elapsed().as_nanos() as u64;
        let t0 = Instant::now();
        let u = &mut self.units;
        let mut examined = 0u64;
        let ranges: Vec<i32> = (0..u.len())
            .map(|i| {
                if self.weapons.is_empty() {
                    return self.tuning.range;
                }
                let w = self
                    .weapons
                    .at((u.weapon[i] as usize).min(self.weapons.len() - 1));
                if w.range > 0 {
                    w.range
                } else {
                    self.tuning.range
                }
            })
            .collect();

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
            if self
                .material_bodies
                .as_ref()
                .and_then(|b| b.state(i as u32))
                .is_some_and(|s| !s.status.sensor || !s.status.alive)
            {
                u.target[i] = NO_TARGET;
                continue;
            }
            let r2 = ranges[i] * ranges[i];
            let t = u.target[i];
            if t != NO_TARGET {
                let tt = t as usize;
                let (dx, dz) = (u.x[i] - u.x[tt], u.z[i] - u.z[tt]);
                let valid = u.alive[tt] && u.owner[tt] != u.owner[i] && dx * dx + dz * dz <= r2;
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
            let range = ranges[i];
            let r2 = range * range;
            let mut best = i32::MAX;
            let mut best_j = NO_TARGET;
            self.grid_far
                .for_each_rect([ux - range, uz - range], [ux + range, uz + range], |j| {
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

        self.combat_ns[1] += t0.elapsed().as_nanos() as u64;
        let t0 = Instant::now();
        self.fire();
        self.combat_ns[2] += t0.elapsed().as_nanos() as u64;
        let t0 = Instant::now();
        self.apply_material_damage();
        self.combat_ns[3] += t0.elapsed().as_nanos() as u64;
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
        // **Ствол на теле, а не под ногами** (ADR-173). `x, y, z` юнита —
        // это его опора, то есть пол. Снаряд, пущенный с пола в пол,
        // всю дорогу скребёт по земле: дуга провисает, а целая часть
        // высоты берётся округлением вниз, и на первой же выпуклости
        // рельефа снаряд втыкается в грунт, не долетев. Подъём — половина
        // роста юнита, число из тех же данных (`unit_height`), а не
        // из головы.
        //
        // Луча это не касается: он приходит в тот же тик, провисать
        // ему нечем, и линия огня у него своя (ADR-170).
        let muzzle = self.tuning.unit_height as i32 / 2;
        let tick = self.tick as u32;
        let frames = self.material_bodies.as_ref().map(|b| b.frames(&self.units));
        let impact_cap = self.tuning.impact.max_pending;
        let impact_motion = self.tuning.impact.motion_pct;
        self.dmg_acc.clear();
        self.dmg_acc.resize(self.units.x.len(), 0);
        let Self {
            econ,
            material_bodies,
            grid_far,
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
            if u.site[i] != 0
                || econ.as_ref().is_some_and(|e| {
                    u.kind[i] != economy::NO_KIND && e.kinds.at(u.kind[i] as usize).weapon == 0
                })
            {
                continue;
            }
            if material_bodies
                .as_ref()
                .and_then(|b| b.state(i as u32))
                .is_some_and(|s| !s.status.alive || !s.status.weapon || !s.status.sensor)
            {
                continue;
            }
            let tgt = u.target[i];
            if tgt == NO_TARGET || tick < u.next_shot[i] {
                continue;
            }
            let wi = (u.weapon[i] as usize).min(weapons.len() - 1);
            let w = *weapons.at(wi);
            let range = if w.range > 0 { w.range } else { range };
            let tt = tgt as usize;
            let height_of = |id: usize| {
                material_bodies
                    .as_ref()
                    .and_then(|b| b.body(id as u32))
                    .map(|b| b.vol.ny as i32 * b.mm as i32 / voxel::unit::WORLD_MM as i32 / 2)
            };
            let from = (u.x[i], u.body_cell_y(i) + height_of(i).unwrap_or(0), u.z[i]);
            let to = (
                u.x[tt],
                u.body_cell_y(tt) + height_of(tt).unwrap_or(0),
                u.z[tt],
            );

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
                    let mut endpoint = to;
                    if let (Some(bodies), Some(frames)) =
                        (material_bodies.as_mut(), frames.as_ref())
                    {
                        if bodies.pending.len() >= impact_cap as usize {
                            continue;
                        }
                        let a = damage_combat::point(from);
                        let b = damage_combat::point(to);
                        if let Some(hit) =
                            damage_combat::trace(bodies, frames, u, grid_far, a, b, i as u32)
                        {
                            damage_combat::queue(
                                bodies,
                                hit,
                                a,
                                b,
                                w.impact_energy_j as u64 * 1000,
                                impact_cap,
                                impact_motion,
                            );
                            endpoint = damage_combat::cell(a.add(b.sub(a).scale(hit.t)));
                            *beams_hit += 1;
                        } else if bodies.body(tgt).is_none() {
                            dmg_acc[tt] += w.damage as u32;
                            *beams_hit += 1;
                        }
                    } else {
                        dmg_acc[tt] += w.damage as u32;
                        *beams_hit += 1;
                    }
                    *beams_fired += 1;
                    events.push(SimEvent::Beam {
                        x0: from.0,
                        y0: from.1,
                        z0: from.2,
                        x1: endpoint.0,
                        y1: endpoint.1,
                        z1: endpoint.2,
                        owner: u.owner[i],
                    });
                    u.next_shot[i] = tick + w.reload;
                }
                WeaponKind::Bolt => {
                    let (from, to) = (
                        (
                            from.0,
                            from.1 + if height_of(i).is_some() { 0 } else { muzzle },
                            from.2,
                        ),
                        (
                            to.0,
                            to.1 + if height_of(tt).is_some() { 0 } else { muzzle },
                            to.2,
                        ),
                    );
                    // Потолок на снаряды в воздухе. Упёршийся выстрел
                    // **не тратит перезарядку**: иначе юнит терял бы
                    // очередь из-за чужой стрельбы.
                    if shots.alive_count() >= cap {
                        *shots_refused += 1;
                        continue;
                    }
                    // Тиков ровно столько, чтобы пересечь дальность.
                    let life = ((range as i64 * TICKS_PER_SECOND as i64) / w.speed.max(1) as i64
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
                    // Подъём снаряда, который долетает, не больше пути
                    // до цели: вертикальный выстрел поднимается ровно
                    // на высоту цели. Вдвое больший потолок подъёма
                    // решений не трогает, а квадраты держит в пределах
                    // `Fx` и на карте в десять километров (ADR-430):
                    // медленный снаряд на полной дальности поднимал
                    // прицел так, что `k * d2` и `a * a` выходили за
                    // разрядность и заворачивались в мусорную скорость.
                    let cap = dx
                        .abs()
                        .add(dy.abs())
                        .add(dz.abs())
                        .mul(Fx::from_int(2))
                        .add(Fx::ONE);
                    let lift = |q: Fx| {
                        Fx(((i128::from(k.raw()) * i128::from(q.raw())) >> 32)
                            .clamp(0, i128::from(cap.raw())) as i64)
                    };
                    // Подъём зависит от времени полёта, а время —
                    // от подъёма: это неподвижная точка, и берётся
                    // она простой итерацией от прямой наводки.
                    // При земном тяготении и полной дальности второй
                    // проход правит ответ на полтора процента, третий —
                    // на сотые доли.
                    let mut rise = lift(d2);
                    for _ in 0..AIM_ITERS {
                        let a = dy.add(rise);
                        let next = lift(flat.add(a.mul(a)));
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
                    let shot = shots.spawn(
                        from,
                        (Fx::ZERO, Fx::ZERO, Fx::ZERO),
                        (
                            dx.div(len).mul(step),
                            ay.div(len).mul(step),
                            dz.div(len).mul(step),
                        ),
                        u.owner[i],
                        w.damage,
                        life,
                        (w.blast_radius, w.blast_damage),
                    );
                    shots.weapon_id[shot as usize] = w.id;
                    shots.source[shot as usize] = i as u32;
                    shots.source_generation[shot as usize] = material_bodies
                        .as_ref()
                        .and_then(|b| b.state(i as u32))
                        .map_or(0, |b| b.generation);
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
        let radius = self.tuning.shot_hit_radius;
        let height = self.tuning.unit_height as i32;
        // **Снаряд подчиняется тяготению** (ADR-173). Луча здесь нет
        // вовсе: он не летит, а приходит в тот же тик, и ускорять
        // у него нечего.
        let g_tick = gravity_per_tick(&self.tuning);
        let frames = self.material_bodies.as_ref().map(|b| b.frames(&self.units));
        let impact_cap = self.tuning.impact.max_pending;
        let impact_motion = self.tuning.impact.motion_pct;
        let burst_tuning = self.tuning.blast;
        self.dmg_acc.clear();
        self.dmg_acc.resize(self.units.x.len(), 0);
        let Self {
            material_bodies,
            bursts,
            weapons,
            units: u,
            shots,
            shot_probe,
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
            // Backpressure retains the projectile at the start of this step.
            if material_bodies
                .as_ref()
                .is_some_and(|b| b.pending.len() >= impact_cap as usize)
            {
                continue;
            }
            if material_bodies.is_some()
                && weapons
                    .get(shots.weapon_id[s])
                    .is_some_and(|w| w.blast_energy_j > 0)
                && !bursts.ready(&burst_tuning)
            {
                continue;
            }
            shots.life[s] -= 1;

            let from = (shots.x[s], shots.y[s], shots.z[s]);
            let ray_from = phys::V3::new(
                Fx::from_int(from.0).add(shots.fx[s]),
                Fx::from_int(from.1).add(shots.fy[s]),
                Fx::from_int(from.2).add(shots.fz[s]),
            );
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
            let ray_to = ray_from.add(phys::V3::new(shots.vx[s], shots.vy[s], shots.vz[s]));

            // Где рванёт, если рванёт.
            let mut hit_at: Option<(i32, i32, i32)> = None;
            // Порода на пути. Оба конца исключены, поэтому стрелок
            // не заслоняет сам себя на первом тике.
            let mut wall: Option<(i32, i32, i32)> = None;
            let mut exits_world = false;
            if let Some(g) = ground.as_ref() {
                let (ex, ez) = g.extent();
                let ey = g.world.ny as i32;
                if to.0 < 0 || to.2 < 0 || to.0 >= ex || to.2 >= ez || to.1 < 0 || to.1 >= ey {
                    if material_bodies.is_none() {
                        shots.kill(si);
                        continue;
                    }
                    exits_world = true;
                }
                let los = if material_bodies.is_some() {
                    // Collision must follow flight order; visibility's canonical
                    // endpoint order would choose the far wall on reverse shots.
                    let mut result = voxel::los::LosResult {
                        clear: true,
                        blocked_at: to,
                        steps: 0,
                    };
                    result.steps = voxel::los::walk(from, to, |x, y, z| {
                        if g.world
                            .solid(x.max(0) as u32, y.max(0) as u32, z.max(0) as u32)
                        {
                            result.clear = false;
                            result.blocked_at = (x, y, z);
                            false
                        } else {
                            true
                        }
                    });
                    result
                } else {
                    voxel::los::line_of_sight(&g.world, from, to)
                };
                if !los.clear {
                    wall = Some(los.blocked_at);
                } else if g.world.solid(to.0 as u32, to.1 as u32, to.2 as u32) {
                    // **Конец шага — тоже порода.** Линия огня исключает
                    // оба конца нарочно: цель себя не заслоняет. Снаряду
                    // же конец шага — не цель, а место, где он окажется,
                    // и на шаге в одну клетку обход между концами
                    // не проверяет **ничего**: стена в воксель толщиной
                    // пропускала бы снаряд насквозь (ADR-173).
                    wall = Some(to);
                }
            }

            // **Кого задело — ищется по всему отрезку, а не в его конце**
            // (ADR-173). Снаряд проходит за тик больше клетки, а допуск
            // попадания — воксель: цель, мимо которой его пронесло между
            // двумя тиками, в конечной точке шага не находится вовсе,
            // и чем быстрее снаряд, тем больше он протыкает насквозь.
            // Породы эта ловушка не касалась с самого начала — её
            // проверяет тот же обход, — а юниты проверялись в одной точке.
            //
            // Отрезок обрывается о породу: до того, что за стеной, снаряд
            // не долетел. Обход идёт **по ходу полёта**, поэтому первый
            // найденный и есть тот, в кого попали; ничья внутри одной
            // точки разрешается **номером юнита**, а не порядком обхода
            // сетки.
            let mine = shots.owner[s];
            let end = wall.unwrap_or(to);
            let (mut hit_unit, mut hit_point) = (NO_TARGET, None);
            shot_probe.prepare(
                grid_far,
                u,
                material_bodies.as_ref(),
                from,
                end,
                radius,
                height,
                mine,
            );
            if !shot_probe.is_empty() {
                voxel::los::walk(from, end, |px, py, pz| {
                    let j = shot_probe.at(grid_far, px, py, pz);
                    if j == NO_TARGET {
                        return true;
                    }
                    hit_unit = j;
                    hit_point = Some((px, py, pz));
                    false
                });
                // Walk excludes the endpoint. A terrain endpoint still blocks it.
                if hit_unit == NO_TARGET && wall.is_none() {
                    let j = shot_probe.at(grid_far, to.0, to.1, to.2);
                    if j != NO_TARGET {
                        hit_unit = j;
                        hit_point = Some(to);
                    }
                }
            }

            let physical_hit =
                if let (Some(bodies), Some(frames)) = (material_bodies.as_ref(), frames.as_ref()) {
                    damage_combat::trace(
                        bodies,
                        frames,
                        u,
                        grid_far,
                        ray_from,
                        ray_to,
                        if bodies
                            .state(shots.source[s])
                            .is_some_and(|b| b.generation == shots.source_generation[s])
                        {
                            shots.source[s]
                        } else {
                            NO_TARGET
                        },
                    )
                    .filter(|hit| {
                        wall.is_none_or(|p| hit.t < damage_combat::cell_entry(ray_from, ray_to, p))
                    })
                    .filter(|hit| {
                        hit_point
                            .is_none_or(|p| hit.t <= damage_combat::cell_entry(ray_from, ray_to, p))
                    })
                } else {
                    None
                };
            if let Some(hit) = physical_hit {
                hit_unit = hit.unit;
                hit_point = Some(damage_combat::cell(
                    ray_from.add(ray_to.sub(ray_from).scale(hit.t)),
                ));
                let energy = weapons.get(shots.weapon_id[s]).map_or(0, |w| {
                    let velocity = [shots.vx[s], shots.vy[s], shots.vz[s]].map(|v| {
                        v.mul(Fx::from_int(
                            TICKS_PER_SECOND * voxel::unit::WORLD_MM as i32,
                        ))
                        .to_int_trunc() as i64
                    });
                    (w.impact_energy_j as u64 * 1000)
                        .saturating_add(voxel::impact::kinetic_mj(w.mass_g, velocity))
                });
                damage_combat::queue(
                    material_bodies.as_mut().unwrap(),
                    hit,
                    ray_from,
                    ray_to,
                    energy,
                    impact_cap,
                    impact_motion,
                );
            }
            if let Some(p) = hit_point {
                if physical_hit.is_none() {
                    dmg_acc[hit_unit as usize] += shots.damage[s] as u32;
                }
                *shots_hit += 1;
                shots.x[s] = p.0;
                shots.y[s] = p.1;
                shots.z[s] = p.2;
                shots.kill(si);
                hit_at = Some(p);
            } else if let Some(b) = wall {
                shots.x[s] = b.0;
                shots.y[s] = b.1;
                shots.z[s] = b.2;
                *shots_blocked += 1;
                shots.kill(si);
                hit_at = Some(b);
            } else {
                shots.x[s] = to.0;
                shots.y[s] = to.1;
                shots.z[s] = to.2;
                if exits_world {
                    shots.kill(si);
                }
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
            events.push(SimEvent::Blast {
                x: at.0,
                y: at.1,
                z: at.2,
                radius: br,
            });
            if material_bodies.is_some() {
                if let (Some(g), Some(w)) = (ground.as_ref(), weapons.get(shots.weapon_id[s])) {
                    if w.blast_energy_j > 0 {
                        let accepted = bursts.enqueue(
                            &g.world,
                            [at.0, at.1, at.2],
                            br,
                            w.blast_energy_j,
                            &burst_tuning,
                        );
                        debug_assert!(accepted, "validated blast retained by shot backpressure");
                        // A refused blast has no job of its own: capturing
                        // would hand its bodies to whichever job is last
                        // (ADR-287).
                        if let (true, Some(bodies), Some(frames)) =
                            (accepted, material_bodies.as_ref(), frames.as_ref())
                        {
                            bursts.capture_last(bodies, frames, &burst_tuning);
                        }
                    }
                }
                continue;
            }
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
                            // **Воронка снимает породу, а не воздух.**
                            // Снаряд рвётся на теле цели, в метре над
                            // полом, и шар радиусом в метр касается земли
                            // одной точкой: из 4 169 его вокселей порода —
                            // около одного процента. Пустая правка ничего
                            // не снимает, а просмотр при дренаже занимает
                            // (ADR-182): двадцать взрывов за тик ставили
                            // в очередь 86 тысяч записей при потолке
                            // в 65 536, и очередь росла каждый тик
                            // (ADR-227). Проверка по битовому объёму
                            // детерминирована: он у всех клиентов один.
                            if !g.world.solid(x as u32, y as u32, z as u32) {
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
                let dy = u.body_cell_y(jj) - at.1;
                if dx * dx + dy * dy + dz * dz > br2 {
                    return;
                }
                // Material units accept only spatial requests. The old radial
                // counter is retained solely for the scalar benchmark fixtures.
                if material_bodies.as_ref().and_then(|b| b.body(j)).is_some() {
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

    /// Фаза `Logistics`: материя доходит до стороны.
    ///
    /// Три шага, и все три — переводы между контейнерами:
    ///
    /// 1. **Начисление выкопанного.** Мир сказал, что сняли и по чьей
    ///    просьбе; земля отдаёт эту материю, бак принимает.
    /// 2. **Реклейм обломка** — тот же перевод, что копка (ADR-017).
    /// 3. **Разгрузка на складе.** Здесь же переработка: выход меньше
    ///    единицы — единственный сток, доступный с первого дня
    ///    (`docs/07-economy.md`).
    ///
    /// Без привязанной экономики фаза остаётся заглушкой, какой была.
    fn phase_logistics(&mut self) {
        if self.econ.is_none() {
            return;
        }
        self.harvest();
        self.haul();
    }

    /// Шаг 1: выкопанное — в бак заказчика.
    ///
    /// **Начисляется по тому, что мир действительно снял**, а не по
    /// тому, что копатель просил: правка может упереться в потолок
    /// и отложиться, а воксель — оказаться уже выбитым взрывом.
    /// Начисление по просьбе создавало бы материю (ADR-031).
    ///
    /// Бесхозные снятия — обвал, запекание тела, воронка — в учёт
    /// не входят вовсе: это физика карты, а не экономика. Карта
    /// в учёте не лежит, поэтому и терять ей нечего.
    fn harvest(&mut self) {
        let Self {
            econ,
            ground,
            units,
            ..
        } = self;
        let Some(e) = econ.as_mut() else { return };
        let Some(g) = ground.as_ref() else { return };
        for &(by, mat) in g.taken() {
            if by == voxel::NOBODY {
                continue;
            }
            let i = by as usize;
            if i >= units.alive.len() || !units.alive[i] {
                continue;
            }
            let c = units.store[i];
            if c == economy::NO_CONTAINER {
                continue;
            }
            let per = e.per_voxel(mat);
            // **Больше, чем влезет, не принимаем.** Наивная
            // альтернатива — воксель снят, материя не принята —
            // была бы стоком вне перечня (`docs/07-economy.md`),
            // поэтому копатель с полным баком и не копает вовсе:
            // `dig_work` считает место заранее.
            let take = per.min(e.ledger.room(c));
            if take == 0 {
                continue;
            }
            e.ledger.reveal(e.ground, take);
            e.mined += e.ledger.transfer(e.ground, c, take);
        }
    }

    /// Шаги 2 и 3: реклейм обломков, поездка на склад и разгрузка.
    fn haul(&mut self) {
        let reach = self.tuning.work_reach;
        let cap = self.tuning.max_dock_searches;
        let (rn, rd) = (self.tuning.refine_num, self.tuning.refine_den);
        let mut searches = 0u32;
        let mut capped = false;
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k] as usize;
            let ki = self.units.kind[i];
            if ki == economy::NO_KIND {
                continue;
            }
            let c = self.units.store[i];
            if c == economy::NO_CONTAINER {
                continue;
            }
            let e = self.econ.as_mut().expect("экономика");
            let d = e.kinds.at(ki as usize);
            if d.tank == 0 {
                continue;
            }
            let owner = self.units.owner[i];

            // --- бак полон: везти на склад. Проверяется **до**
            //     реклейма: полному копателю обломок ни к чему,
            //     и просматривать их ему незачем.
            if self.units.stance[i] == STANCE_DIG && e.ledger.room(c) == 0 {
                self.units.stance[i] = STANCE_HAUL;
                self.units.job[i] = NO_TARGET;
            }

            // --- реклейм: копатель, чей забой пришёлся на обломок,
            //     сливает его тем же переводом, что и породу.
            //
            // Просмотр обломков идёт из **того же бюджета**, что поиск
            // склада: и там, и там копатель ищет, к чему приткнуться,
            // а число пар «копатель × обломок» в плотном случае растёт
            // квадратично (ADR-114). Упёршийся просмотр откладывается
            // на следующий тик: обломок никуда не денется.
            // **Реклейм — работа на месте.** Обломок сливается в бак
            // только у копателя, стоящего у своего забоя; прежде
            // проверялось лишь, что забой пришёлся на обломок, и
            // краулер через всю карту осушал его в тот же тик, а затем,
            // разгрузившись, снова — не сдвинувшись. Едущий к забою
            // и бюджет просмотра не тратит (ADR-423).
            let (ux, uz) = (self.units.x[i], self.units.z[i]);
            let (gx, gz) = (self.units.goal_x[i], self.units.goal_z[i]);
            let at_face = {
                let (ax, az) = ((ux - gx) as i64, (uz - gz) as i64);
                ax * ax + az * az <= (reach as i64) * (reach as i64)
            };
            if self.units.stance[i] == STANCE_DIG && !e.wrecks.is_empty() && at_face {
                if searches >= cap {
                    capped = true;
                    continue;
                }
                searches += 1;
                // Ничья — по **меньшему номеру обломка**, а не по
                // порядку обхода (`CLAUDE.md`).
                let mut best: Option<usize> = None;
                let mut bd = i64::MAX;
                for (wi, w) in e.wrecks.iter().enumerate() {
                    if e.ledger.held(w.held) == 0 {
                        continue;
                    }
                    let (dx, dz) = ((w.x - gx) as i64, (w.z - gz) as i64);
                    let dd = dx * dx + dz * dz;
                    let (ox, oz) = ((w.x - ux) as i64, (w.z - uz) as i64);
                    if dd > (reach as i64) * (reach as i64)
                        || ox * ox + oz * oz > (reach as i64) * (reach as i64)
                    {
                        continue;
                    }
                    if dd < bd {
                        bd = dd;
                        best = Some(wi);
                    }
                }
                if let Some(wi) = best {
                    let from = e.wrecks[wi].held;
                    let room = e.ledger.room(c);
                    let n = e.ledger.transfer(from, c, room);
                    e.reclaimed += n;
                    // Набрал полный бак с обломка — тоже везти.
                    if e.ledger.room(c) == 0 {
                        self.units.stance[i] = STANCE_HAUL;
                        self.units.job[i] = NO_TARGET;
                    }
                }
            }

            if self.units.stance[i] != STANCE_HAUL {
                continue;
            }

            // --- выбрать склад, если его нет или он погиб.
            let mut dock = self.units.job[i];
            let ok = (dock as usize) < self.units.alive.len()
                && dock != NO_TARGET
                && self.units.alive[dock as usize]
                && self.units.owner[dock as usize] == owner
                && self.units.site[dock as usize] == 0
                && self.units.kind[dock as usize] != economy::NO_KIND
                && e.kinds.at(self.units.kind[dock as usize] as usize).store > 0;
            if !ok {
                if searches >= cap {
                    // Упёршийся поиск **откладывается**, а не теряется:
                    // краулер ждёт тик и остаётся с грузом (ADR-114).
                    capped = true;
                    continue;
                }
                searches += 1;
                dock = self.nearest_dock(i);
                self.units.job[i] = dock;
                let e = self.econ.as_mut().expect("экономика");
                e.dock_searches += 1;
                if dock == NO_TARGET {
                    // Склада нет вовсе — копать дальше некуда, но
                    // и ехать некуда: юнит стоит при забое с полным
                    // баком. Это видно игроку и лечится складом.
                    continue;
                }
            }
            let (dx, dz) = (
                self.units.x[dock as usize] - self.units.x[i],
                self.units.z[dock as usize] - self.units.z[i],
            );
            self.units.goal_x[i] = self.units.x[dock as usize];
            self.units.goal_y[i] = self.units.y[dock as usize];
            self.units.goal_z[i] = self.units.z[dock as usize];
            if dx * dx + dz * dz > reach * reach {
                continue;
            }

            // --- разгрузка. Переработка с выходом меньше единицы —
            //     тот самый сток, без которого партия не кончается.
            let e = self.econ.as_mut().expect("экономика");
            let cost = e.kinds.at(ki as usize).cost;
            let body = self
                .material_bodies
                .as_ref()
                .map_or(cost, |b| b.remaining_matter(i as u32, cost));
            let cargo = e.ledger.held(c).saturating_sub(body);
            if cargo > 0 {
                let Some(bank) = e.banks.get(owner as usize).copied() else {
                    continue;
                };
                // **Больше, чем влезет в запас, не перерабатывается.**
                // Переработка списывает в сток всё взятое, что не ушло
                // приёмнику: сдай полный бак в полный склад — и груз
                // целиком уйдёт в «выход переработки < 1». Это был бы
                // не сток, а уничтожение груза полным складом, и
                // выглядело бы оно как работающая переработка.
                //
                // Отсюда: сколько руды брать, считается **от места**
                // в запасе, а не от того, сколько её в баке.
                let room = e.ledger.room(bank.stock);
                let fits = room.saturating_mul(rd) / rn.max(1);
                let want = cargo.min(fits);
                if want > 0 {
                    let got = e.ledger.refine(c, bank.stock, want, rn, rd);
                    e.delivered += got;
                }
            }
            // Разгрузился — обратно к забою. Забой запомнен приказом,
            // а не тем местом, где кончился бак.
            if e.ledger.held(c) <= body {
                let (fx, fy, fz) = self.units.face[i];
                if fy > 1 {
                    self.units.stance[i] = STANCE_DIG;
                    self.units.goal_x[i] = fx;
                    self.units.goal_y[i] = fy;
                    self.units.goal_z[i] = fz;
                } else {
                    self.units.stance[i] = STANCE_HOLD;
                }
                self.units.job[i] = NO_TARGET;
            }
        }
        if capped {
            if let Some(e) = self.econ.as_mut() {
                e.dock_capped += 1;
            }
        }
    }

    /// Ближайший достроенный склад своей стороны.
    ///
    /// Ничья — по **меньшему номеру слота**, а не по порядку обхода
    /// (`CLAUDE.md`): иначе два одинаково близких склада выбирались бы
    /// по истории списка живых.
    fn nearest_dock(&self, i: usize) -> u32 {
        let Some(e) = self.econ.as_ref() else {
            return NO_TARGET;
        };
        let owner = self.units.owner[i];
        let (ux, uz) = (self.units.x[i], self.units.z[i]);
        let mut best = NO_TARGET;
        let mut bd = i64::MAX;
        for k in 0..self.units.live_ids.len() {
            let j = self.units.live_ids[k] as usize;
            if self.units.owner[j] != owner || self.units.site[j] != 0 {
                continue;
            }
            let kj = self.units.kind[j];
            if kj == economy::NO_KIND || e.kinds.at(kj as usize).store == 0 {
                continue;
            }
            let (dx, dz) = ((self.units.x[j] - ux) as i64, (self.units.z[j] - uz) as i64);
            let d = dx * dx + dz * dz;
            if d < bd {
                bd = d;
                best = j as u32;
            }
        }
        best
    }

    /// Фаза `Economy`: стройка, производство, потолки, исход.
    fn phase_economy(&mut self) {
        if self.econ.is_none() {
            return;
        }
        self.build_work();
        self.produce_work();
        self.recount_caps();
        self.sweep_wrecks();
    }

    /// Убрать выбранные обломки.
    ///
    /// Раз за тик и по всему списку, а не на каждом реклейме: список
    /// читает каждый копатель, и пустая запись в нём — это работа
    /// на каждом из них плюс запись в хеше.
    ///
    /// Порядок сохраняется: `retain`, а не `swap_remove`. Перестановка
    /// соседей молча меняет исход ничьей в выборе ближайшего обломка
    /// (`CLAUDE.md`).
    fn sweep_wrecks(&mut self) {
        let Some(e) = self.econ.as_mut() else { return };
        self.bursts.sweep(e);
        if e.wrecks.is_empty() {
            return;
        }
        let mut empty: Vec<matter::ContainerId> = Vec::new();
        let ledger = &e.ledger;
        e.wrecks.retain(|w| {
            if ledger.held(w.held) == 0 {
                empty.push(w.held);
                false
            } else {
                true
            }
        });
        for id in empty {
            e.ledger.retire_empty_debris(id);
        }
    }

    /// Строители льют материю в недострой.
    ///
    /// Недострой растёт **вокселями**: вложенная материя переводится
    /// в прикреплённые к ядру воксели по доле от стоимости. Растёт
    /// прибавкой, а не пересчётом: пересчёт лечил бы повреждения,
    /// и недострой стал бы неуязвимым.
    fn build_work(&mut self) {
        let reach = self.tuning.work_reach;
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k] as usize;
            if self.units.stance[i] != STANCE_BUILD {
                continue;
            }
            let s = self.units.job[i];
            let alive = s != NO_TARGET
                && (s as usize) < self.units.alive.len()
                && self.units.alive[s as usize]
                && self.units.site[s as usize] > 0;
            if !alive {
                self.units.stance[i] = STANCE_HOLD;
                self.units.job[i] = NO_TARGET;
                continue;
            }
            let s = s as usize;
            let (dx, dz) = (
                self.units.x[s] - self.units.x[i],
                self.units.z[s] - self.units.z[i],
            );
            self.units.goal_x[i] = self.units.x[s];
            self.units.goal_y[i] = self.units.y[s];
            self.units.goal_z[i] = self.units.z[s];
            if dx * dx + dz * dz > reach * reach {
                continue;
            }
            let owner = self.units.owner[i];
            let ki = self.units.kind[i];
            let si = self.units.kind[s];
            if ki == economy::NO_KIND || si == economy::NO_KIND {
                continue;
            }
            let e = self.econ.as_mut().expect("экономика");
            let rate = e.kinds.at(ki as usize).build_rate;
            let (cost, full) = {
                let d = e.kinds.at(si as usize);
                (d.cost, d.voxels as u64)
            };
            let Some(bank) = e.banks.get(owner as usize).copied() else {
                continue;
            };
            let want = rate.min(self.units.site[s]);
            let moved = e.ledger.transfer(bank.stock, self.units.store[s], want);
            if moved == 0 {
                continue;
            }
            e.spent += moved;
            let before = full * (cost - self.units.site[s]) / cost.max(1);
            self.units.site[s] -= moved;
            let after = full * (cost - self.units.site[s]) / cost.max(1);
            self.units.voxels[s] =
                self.units.voxels[s].saturating_add((after - before).min(u16::MAX as u64) as u16);
            if self.units.site[s] == 0 {
                e.built += 1;
                e.drop_site(s as u32);
                self.units.voxels[s] = full.min(u16::MAX as u64) as u16;
                self.units.stance[i] = STANCE_HOLD;
                self.units.job[i] = NO_TARGET;
            }
        }
    }

    /// Заводы собирают машины.
    ///
    /// Заказ готов, когда **и уплачено, и отстояло**: деньги вперёд
    /// времени не двигают, и наоборот. Иначе богатая сторона получала
    /// бы армию мгновенно.
    fn produce_work(&mut self) {
        let mut out: Vec<(u32, u16, u8)> = Vec::new();
        {
            let Some(e) = self.econ.as_mut() else { return };
            let mut dead: Vec<u32> = Vec::new();
            for li in 0..e.lines.len() {
                let unit = e.lines[li].unit;
                let u = unit as usize;
                let ok = u < self.units.alive.len()
                    && self.units.alive[u]
                    && self.units.site[u] == 0
                    && self.units.kind[u] != economy::NO_KIND;
                if !ok {
                    dead.push(unit);
                    continue;
                }
                let Some(&ki) = e.lines[li].queue.front() else {
                    dead.push(unit);
                    continue;
                };
                let (cost, ticks_need) = {
                    let d = e.kinds.at(ki as usize);
                    (d.cost, d.build_ticks.max(1))
                };
                let owner = self.units.owner[u];
                let Some(bank) = e.banks.get(owner as usize).copied() else {
                    continue;
                };
                let held = e.lines[li].held;
                let paid = e.ledger.held(held);
                // Порция за тик — вверх: при делении вниз последний
                // грош не платился бы никогда, и заказ висел бы вечно.
                let per_tick = cost.div_ceil(ticks_need as u64);
                let want = per_tick.min(cost.saturating_sub(paid));
                if want > 0 {
                    let moved = e.ledger.transfer(bank.stock, held, want);
                    e.spent += moved;
                }
                e.lines[li].ticks = e.lines[li].ticks.saturating_add(1);
                if e.ledger.held(held) >= cost && e.lines[li].ticks >= ticks_need {
                    out.push((unit, ki, owner));
                }
            }
            for unit in dead {
                let owner = {
                    let u = unit as usize;
                    if u < self.units.owner.len() {
                        self.units.owner[u]
                    } else {
                        0
                    }
                };
                let stock = e.banks.get(owner as usize).map(|b| b.stock);
                match stock {
                    Some(s) => e.drop_line(unit, s),
                    None => e.drop_line(unit, e.ground),
                }
            }
        }

        // Выпуск — **после** обхода: заселение берёт весь мир, а
        // порядок остаётся порядком в списке очередей, а не обхода.
        for (unit, ki, owner) in out {
            let u = unit as usize;
            let gap = self.tuning.separation;
            let (ex, ez) = self
                .ground
                .as_ref()
                .map(|g| g.extent())
                .unwrap_or((WORLD, WORLD));
            // Машина выходит на расстояние расталкивания от завода:
            // ближе она физически стоять не может, и место это уже
            // задано данными.
            let x = (self.units.x[u] + gap).clamp(0, ex - 1);
            let z = self.units.z[u].clamp(0, ez - 1);
            let id = {
                let e = self.econ.as_ref().expect("экономика");
                e.kinds.at(ki as usize).id
            };
            let Some(made) = self.spawn_kind_empty(x, z, owner, id) else {
                continue;
            };
            let e = self.econ.as_mut().expect("экономика");
            let Ok(li) = e.lines.binary_search_by_key(&unit, |l| l.unit) else {
                continue;
            };
            let held = e.lines[li].held;
            let cost = e.kinds.at(ki as usize).cost;
            // Материя заказа **переезжает в тело машины**: тело юнита
            // это контейнер (`docs/07-economy.md`), и подбитая машина
            // отдаёт её обломком.
            e.ledger
                .transfer(held, self.units.store[made as usize], cost);
            e.lines[li].queue.pop_front();
            e.lines[li].ticks = 0;
            e.produced += 1;
            self.units.refresh_live();
        }
    }

    /// Пересчёт потолков запаса.
    ///
    /// Потолок стороны — база из данных плюс вместимость её
    /// **достроенных** складов (ADR-043). Недострой вместимости
    /// не даёт: иначе «начал стройку» выводило бы материю из-под
    /// потолка через бесконечные фундаменты (`docs/07-economy.md`).
    ///
    /// Упавший потолок сбрасывает излишек в объявленный сток. Порядок
    /// фиксирован каноном, и он здесь: доля при разрушении уже снята
    /// фазой уборки, потолок пересчитывается после неё.
    fn recount_caps(&mut self) {
        let base = self.tuning.base_store;
        let owners = self.econ.as_ref().map(|e| e.banks.len()).unwrap_or(0);
        let mut caps = vec![base; owners];
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k] as usize;
            if self.units.site[i] != 0 {
                continue;
            }
            let ki = self.units.kind[i];
            if ki == economy::NO_KIND {
                continue;
            }
            let o = self.units.owner[i] as usize;
            if o >= owners {
                continue;
            }
            let e = self.econ.as_ref().expect("экономика");
            caps[o] = caps[o].saturating_add(e.kinds.at(ki as usize).store);
        }
        let e = self.econ.as_mut().expect("экономика");
        for (o, cap) in caps.into_iter().enumerate() {
            e.banks[o].cap = cap;
            let stock = e.banks[o].stock;
            e.ledger.lower_cap(stock, cap);
        }
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
        let died = !dead.is_empty();
        let frames = died
            .then(|| self.material_bodies.as_ref().map(|b| b.frames(&self.units)))
            .flatten();
        for i in dead {
            let k = i as usize;
            self.events.push(SimEvent::Death {
                unit: i,
                x: self.units.x[k],
                y: self.units.body_cell_y(k),
                z: self.units.z[k],
            });
            self.wreck(k, frames.as_ref().and_then(|f| f[k]));
            self.units.kill(i);
        }
        // «Живые» — это живые (ADR-436): список пересобирала только фаза
        // `NavUpdate` в начале тика, и убитые здесь лежали в нём до
        // следующего тика — `living()` и подкрепление между тиками
        // считали мёртвых, а подкрепление запаздывало на тик.
        if died {
            self.units.refresh_live();
        }
        self.recount_outcome();
    }

    /// Разрушенная машина оставляет обломок.
    ///
    /// Порядок канонический (`docs/07-economy.md`): доля содержимого
    /// теряется по данным, остаток переходит в обломок с нейтральным
    /// владельцем (ADR-047), потолок игрока пересчитывается фазой
    /// `Economy` следующего тика.
    ///
    /// Обломок лежит **там, где погибла машина**, и реклеймится тем же
    /// приказом «копать», что и порода (ADR-017).
    fn wreck(&mut self, k: usize, frames: Option<[damage::Frame; 2]>) {
        let c = self.units.store[k];
        if c == economy::NO_CONTAINER {
            return;
        }
        let (num, den) = (self.tuning.breach_loss_num, self.tuning.breach_loss_den);
        let (x, y, z) = (self.units.x[k], self.units.body_cell_y(k), self.units.z[k]);
        let unit = k as u32;
        let Some(e) = self.econ.as_mut() else { return };
        // Недострой погиб — снять его из списка: иначе к покойнику
        // будут пристраиваться следующие приказы «строить».
        e.drop_site(unit);
        let (debris, _lost, rest) = e.ledger.breach(c, num, den);
        // Заготовка погибшего завода разделяет его судьбу — и ложится
        // в тот же обломок.
        e.wreck_line(unit, debris);
        let has_matter = e.ledger.held(debris) > 0;
        if has_matter {
            e.wrecks.push(economy::Wreck {
                held: debris,
                x,
                y,
                z,
            });
        } else {
            let _ = rest;
            e.ledger.retire_empty_debris(debris);
        }
        // Слот контейнера возвращается в свободный список: партия
        // на десять тысяч тиков иначе копит по записи на каждую
        // погибшую машину.
        e.ledger.release(c);
        self.units.store[k] = economy::NO_CONTAINER;
        if has_matter {
            self.remember_corpse(k, debris, frames);
        }
        // Выбранные обломки убирает `sweep_wrecks` — раз за тик
        // и в одном месте.
    }

    /// Пересчёт исхода: у стороны не осталось живых — она проиграла.
    fn recount_outcome(&mut self) {
        // Массив на стеке, а не `Vec`: фаза идёт каждый тик, в том
        // числе на прогоне гейта, которому исход не нужен вовсе.
        // Выделение на тик ради двух чисел — это цена, которую платят
        // все, включая тех, кто за неё ничего не получает.
        const MAX_OWNERS: usize = 8;
        let owners = self.flow.len().clamp(1, MAX_OWNERS);
        let mut live = [0usize; MAX_OWNERS];
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k] as usize;
            // **По `alive`, а не по списку живых.** Уборка пересобирает
            // список после убийств (ADR-436), но исход не должен зависеть
            // от того, кто и когда его пересобрал: отставание на тик —
            // это разный исход у разных клиентов, если один из них на
            // этом тике остановится.
            if !self.units.alive[i] {
                continue;
            }
            let o = self.units.owner[i] as usize;
            if o < owners {
                live[o] += 1;
            }
        }
        let mut standing = 0usize;
        let mut who = 0u8;
        for (o, n) in live.iter().enumerate().take(owners) {
            if *n > 0 {
                standing += 1;
                // Первый по номеру, а не по порядку обхода: ничьи
                // разрешаются линейным индексом (`CLAUDE.md`).
                if standing == 1 {
                    who = o as u8;
                }
            }
        }
        let was = self.outcome;
        self.outcome = match standing {
            0 => Outcome::Draw,
            1 => Outcome::Won(who),
            _ => Outcome::Fighting,
        };
        if was == Outcome::Fighting && self.outcome.over() && self.decided_at.is_none() {
            self.decided_at = Some(self.tick);
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
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.x[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.z[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.fx[i].raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.fz[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.voxels[i] as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.y[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.riding[i]);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.fall_from[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.fall_v[i].raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.fall_f[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.target[i] as u64);
            // Наклон корпуса переживает тик, значит входит в хеш:
            // не войди он — подвеску можно было бы переписать целиком
            // и увидеть тот же хеш до бита (ADR-116, ADR-225).
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.up[i].x.raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.up[i].z.raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.spin[i].x.raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.spin[i].z.raw() as u64);
            // **Ход — состояние.** Курс и скорость переживают тик
            // и задают, куда юнит поедет дальше; не войди они в хеш,
            // трансмиссию можно было бы переписать целиком и увидеть
            // тот же хеш до бита. Это ровно то, ради чего написан
            // ADR-116 (ADR-220).
            //
            // Курс входит **всегда**, а не только на ходу: стоящая
            // машина носом куда-то смотрит, и по положению этого
            // не видно.
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.head[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.vel[i].raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.chassis[i].code() as u64);
            // Угловая скорость и фаза шага — тоже состояние (ADR-234):
            // первая переживает тик и задаёт доворот, вторая растёт
            // путём и решает, какая нога поднята. Обе входят всегда:
            // у стоящей машины они нулевые, и хеш прогона без хода
            // от их появления не сдвинулся.
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.yaw_rate[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.gait[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.heave[i].raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.units.heave_v[i].raw() as u64);
            // Приказ — состояние: он переживает тик и задаёт движение,
            // а состояние, не входящее в хеш, можно переписать целиком
            // и увидеть тот же хеш до бита (ADR-116).
            //
            // Входит он **только когда он есть**. У юнита без приказа
            // поля приказа не значат ничего, и прогон, в котором
            // приказов не отдают, хеширует ровно то же, что хешировал
            // до появления канала: эталонный хеш от него не сдвинулся
            // ни на бит (ADR-195).
            if self.units.stance[i] != STANCE_FLOW {
                w = w.wrapping_mul(31).wrapping_add(self.units.stance[i] as u64);
                w = w
                    .wrapping_mul(31)
                    .wrapping_add(self.units.goal_x[i] as u32 as u64);
                w = w
                    .wrapping_mul(31)
                    .wrapping_add(self.units.goal_y[i] as u32 as u64);
                w = w
                    .wrapping_mul(31)
                    .wrapping_add(self.units.goal_z[i] as u32 as u64);
            }
            // Экономика юнита входит в хеш **только когда она есть**
            // (ADR-230). У юнита без типа поля экономики не значат
            // ничего, и прогон, который её не ведёт, хеширует ровно
            // то же, что хешировал до её появления: эталонный хеш
            // от неё не сдвинулся ни на бит. Правило то же, по
            // которому в хеш входит приказ (ADR-195).
            if self.units.kind[i] != economy::NO_KIND {
                w = w.wrapping_mul(31).wrapping_add(self.units.kind[i] as u64);
                w = w.wrapping_mul(31).wrapping_add(self.units.site[i]);
                w = w.wrapping_mul(31).wrapping_add(self.units.job[i] as u64);
                // Забой входит **целиком**, с высотой: пока копатель
                // везёт груз, его `goal_y` — это высота склада,
                // а глубина забоя живёт только здесь. Не войди она —
                // копатель вернулся бы на другую глубину, и хеш
                // этого не заметил бы.
                w = w
                    .wrapping_mul(31)
                    .wrapping_add(self.units.face[i].0 as u32 as u64);
                w = w
                    .wrapping_mul(31)
                    .wrapping_add(self.units.face[i].1 as u32 as u64);
                w = w
                    .wrapping_mul(31)
                    .wrapping_add(self.units.face[i].2 as u32 as u64);
                if let Some(e) = &self.econ {
                    if self.units.store[i] != economy::NO_CONTAINER {
                        w = w
                            .wrapping_mul(31)
                            .wrapping_add(e.ledger.held(self.units.store[i]));
                    }
                }
            }
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        // Снаряды — состояние, а не украшение: не войди они в хеш,
        // полёт можно было бы переписать целиком и увидеть тот же
        // хеш до бита (ADR-116).
        for k in 0..self.shots.live_ids.len() {
            let i = self.shots.live_ids[k] as usize;
            let mut w = 0u64;
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.shots.x[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.shots.y[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.shots.z[i] as u32 as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.shots.fx[i].raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.shots.fz[i].raw() as u64);
            // Вертикаль хешируется наравне с горизонталью: пока снаряд
            // летел по прямой, `fy` не менялась и её отсутствие ничего
            // не стоило. С тяготением вертикальная скорость — состояние,
            // которое эволюционирует само, и не войди она в хеш,
            // интегрирование можно было бы переписать и увидеть тот же
            // хеш до бита (ADR-116, ADR-173).
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.shots.fy[i].raw() as u64);
            w = w
                .wrapping_mul(31)
                .wrapping_add(self.shots.vy[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.shots.life[i] as u64);
            if self.material_bodies.is_some() {
                w = w
                    .wrapping_mul(31)
                    .wrapping_add(self.shots.weapon_id[i] as u64);
                w = w.wrapping_mul(31).wrapping_add(self.shots.source[i] as u64);
                w = w
                    .wrapping_mul(31)
                    .wrapping_add(self.shots.source_generation[i] as u64);
            }
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        if let Some(g) = &self.ground {
            h = h.wrapping_add(g.hash());
        }
        // Запас стороны, очереди заводов и обломки переживают тик —
        // значит входят в хеш. Не войди они, запас можно было бы
        // переписать целиком и увидеть тот же хеш до бита (ADR-116).
        if let Some(e) = &self.econ {
            h = h.wrapping_add(e.hash());
        }
        if let Some(b) = &self.material_bodies {
            h = h.wrapping_add(b.hash());
        }
        h = h.wrapping_add(self.bursts.hash());
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
    let spread = (ex / 6).clamp(20, 500);
    let depth = (ez / 4).clamp(20, 750);
    let bx = if owner == 0 {
        midx - gap - spread
    } else {
        midx + gap + spread
    };
    let x = (bx + r.range(-spread, spread)).clamp(0, ex - 1);
    let z = (midz + r.range(-depth, depth)).clamp(0, ez - 1);
    let id = w.spawn_ground(x, z, owner, r.range(50, 150), 300);
    // **Строй стоит лицом к противнику, а не носом на восток.**
    // Курс — состояние (ADR-220), и оставить его нулевым значило бы,
    // что половина армии первым делом разворачивается на месте.
    //
    // Вид ходовой у стороны свой, и задаётся он **здесь, явно**, —
    // той же рукой, что и оружие (ADR-192). Вывести его из номера
    // владельца в фазе движения значило бы спрятать правило
    // в горячем пути.
    let k = id as usize;
    w.units.head[k] = if owner == 0 { 0 } else { fx::angle::HALF };
    w.units.chassis[k] = if owner == 0 {
        ChassisKind::Tracked
    } else {
        ChassisKind::Walking
    };
    id
}

pub fn populate(w: &mut World, n: usize, seed: u64) {
    let mut r = Rng::new(seed, 0xA1);
    // Если земля есть, сетка у мира и у неё **одна**: юниты стоят
    // в воксельных координатах, а не на отвлечённой плоскости
    // (ADR-134). Без земли — прежний размах.
    let (ex, ez) = w.ground().map(|g| g.extent()).unwrap_or((WORLD, WORLD));
    let (midx, midz) = (ex / 2, ez / 2);
    let gap = w.tuning.range / 2;
    let spread = (ex / 6).clamp(20, 500);
    for k in 0..n {
        let _ = spawn_in_line(w, &mut r, (k % 2) as u8);
    }
    w.fill_flow_towards(0, midx + gap + spread, midz);
    w.fill_flow_towards(1, midx - gap - spread, midz);
    w.units.refresh_live();
}

/// Сторона мира эталонного сценария — 128 м.
///
/// Меньше умолчания генератора (256 м в стороне): гейт запускают
/// руками и в CI, и он обязан оставаться быстрым. Больше сектора
/// навигации во много раз, иначе иерархия в нём не участвует.
///
/// **Второе условие и задаёт число.** Сектор — шестнадцать нав-клеток
/// (`nav::sectors::SEC`), нав-клетка — десять вокселей из данных,
/// то есть сектор это 160 вокселей; 640 даёт четыре сектора
/// на сторону. Сохрани здесь прежние 256 вокселей — и на сторону
/// пришлось бы полтора сектора, то есть иерархии в сценарии
/// не осталось бы вовсе. Прежние 128 м карты дали бы восемь секторов
/// на сторону, но и вчетверо больше клеток навигации, поля
/// направлений и чанков каскада на каждый тик гейта (ADR-227).
pub const REF_SIDE: u32 = 640;

/// Чем вооружены стороны эталонного сценария — **номерами видов**
/// из `data/weapons.ron`, а не местами в массиве (ADR-192).
pub const REF_WEAPON_IDS: [u16; 2] = [1, 2];

/// Потолок затронутых вокселей за тик у мира эталонного сценария.
/// Одно число на всех, кто собирает этот мир — из сида или из кэша
/// карты (ADR-228): разный потолок — разный дренаж, то есть другая
/// партия.
pub const REF_MAX_EDITS: usize = 1 << 16;

/// Сколько копателей в эталонном сценарии и как глубоко они грызут
/// за тик. Копка — экономика проекта, и сценарий, который не копает,
/// не проверяет главного.
const REF_DIGGERS: usize = 24;
const REF_RADIUS: i32 = 10;

/// Сколько юнитов сторона добирает за тик, пока не вернулась
/// к заявленной численности.
const REF_REINFORCE: usize = 4;

/// Мир эталонного сценария — **в одном месте**.
///
/// Высота рельефа задаётся абсолютными вокселями, а `surface_height`
/// обрезает её по высоте мира: `clamp(8, ny - 8)`. Умолчание генератора
/// (`base_height` 700 при `ny` 1280) в мире вдвое ниже упирается
/// в обрезку **каждой колонкой** — карта выходит плоской, размах восемь
/// вокселей вместо четырёхсот, и навигация по объёму на ней не
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
    pub(crate) side: u32,
    /// Заявленная численность стороны. Бой затухает без подкреплений,
    /// и прогон на десять тысяч тиков восемь тысяч из них хеширует
    /// почти пустое поле (ADR-161).
    pub(crate) per_side: usize,
    /// Генератор нужен и дальше: копатель, догрызший свой столб,
    /// переходит на новый и начинает с поверхности.
    pub(crate) terrain: worldgen::Terrain,
    /// Копатели: где грызут и куда двинутся дальше.
    pub(crate) diggers: Vec<Digger>,
    pub(crate) rng: Rng,
    /// Сид партии. Нужен сохранению: мир восстанавливается
    /// из него, а не хранится (ADR-229).
    pub(crate) seed: u64,
}

/// Копатель эталонного сценария: центр шара и признак свежего столба.
#[derive(Clone, Copy, Debug)]
struct Digger {
    x: i32,
    y: i32,
    z: i32,
    /// Столб только что занят: следующий тик ставит в очередь шар
    /// целиком, а не один нижний слой.
    fresh: bool,
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
        // Потолок правок — из данных, как и остальные числа тика;
        // `Ground::new` поставит его же, литерала здесь нет (ADR-227).
        let vw = voxel::World::new(
            params.nx,
            params.ny,
            params.nz,
            tuning.max_voxel_edits_per_tick as usize,
            Box::new(worldgen::Terrain { params, seed }),
        );
        let g = ground::Ground::new(vw, &tuning);
        Reference::with_ground(seed, units, tuning, weapons, params, g)
    }

    /// Тот же сценарий на **готовой** земле — собранной из сида
    /// или прочитанной из кэша карты (ADR-228). Земля обязана быть
    /// той, что дал бы генератор от `params` и `seed`: сценарий
    /// этого не проверяет, за это отвечает кэш.
    pub fn with_ground(
        seed: u64,
        units: usize,
        tuning: Tuning,
        weapons: modding::weapons::Weapons,
        params: worldgen::Params,
        g: ground::Ground,
    ) -> Reference {
        let terrain = worldgen::Terrain { params, seed };
        let mut world = World::new(2, units.max(1), tuning);
        world.attach_ground(g);
        world.attach_weapons(weapons);
        populate(&mut world, units, seed);
        // Сторона выбирает оружие **явным номером вида**, а не местом
        // в массиве: место зависит от того, какие виды загружены,
        // и мод, добавивший третий, молча перевооружил бы обе стороны.
        // Ровно та ловушка, от которой ADR-028 защищает материалы
        // (ADR-192). Жребием тоже нельзя: разбор по виду попал бы
        // в зависимость от генератора случайных чисел.
        for k in 0..world.units.len() {
            let id = REF_WEAPON_IDS[world.units.owner[k] as usize % REF_WEAPON_IDS.len()];
            world.units.weapon[k] = world
                .weapons
                .index_of(id)
                .unwrap_or_else(|| panic!("вид оружия {id} не загружен"))
                as u16;
        }

        // Копатели ставятся на поверхность, вразнос по карте: копка
        // в одной точке — это другой случай с другой ценой.
        let mut rng = Rng::new(seed, 0xD16);
        let mut diggers = Vec::with_capacity(REF_DIGGERS);
        for _ in 0..REF_DIGGERS {
            let x = rng.below(params.nx) as i32;
            let z = rng.below(params.nz) as i32;
            let y = terrain.surface(x as u32, z as u32) as i32;
            diggers.push(Digger {
                x,
                y,
                z,
                fresh: true,
            });
        }
        Reference {
            world,
            side: params.nx,
            per_side: units.max(1).div_ceil(2),
            terrain,
            diggers,
            rng,
            seed,
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
    ///
    /// **В очередь идёт только то, что шар накрыл впервые** (ADR-227).
    /// На свежем столбе — шар целиком; дальше шар опускается
    /// на воксель за тик, и новых вокселей под ним ровно по одному
    /// на столб его диска: нижняя точка шара в этом столбе. Ставить
    /// в очередь шар целиком каждый тик значило бы просматривать
    /// при дренаже 4 169 записей на копателя, из которых 3 852 —
    /// пустые. На ребре 0.5 м шар в две клетки это 33 записи,
    /// и потолок просмотра их не замечал; на ребре 0.1 м двадцать
    /// четыре шара по 4 169 — это сто тысяч записей за тик при
    /// потолке в 65 536, то есть очередь росла бы каждый тик
    /// без предела.
    fn dig(&mut self) {
        let Some(g) = self.world.ground_mut() else {
            return;
        };
        let (nx, ny, nz) = (g.world.nx as i32, g.world.ny as i32, g.world.nz as i32);
        let r = REF_RADIUS;
        let mut moved: Vec<usize> = Vec::new();
        for (k, d) in self.diggers.iter_mut().enumerate() {
            let full = std::mem::take(&mut d.fresh);
            for dz in -r..=r {
                for dx in -r..=r {
                    let h2 = r * r - dx * dx - dz * dz;
                    if h2 < 0 {
                        continue;
                    }
                    // Полувысота шара в этом столбе — наибольшее `dy`
                    // с `dy² ≤ h2`; без корня, он в симуляции
                    // последнее средство (ADR-071).
                    let mut half = 0;
                    while (half + 1) * (half + 1) <= h2 {
                        half += 1;
                    }
                    let top = if full { half } else { -half };
                    for dy in -half..=top {
                        let (x, y, z) = (d.x + dx, d.y + dy, d.z + dz);
                        if x < 0 || y < 0 || z < 0 || x >= nx || y >= ny || z >= nz {
                            continue;
                        }
                        g.world.queue_dig(x as u32, y as u32, z as u32);
                    }
                }
            }
            d.y -= 1;
            if d.y <= 1 {
                moved.push(k);
            }
        }
        // Переселение — после обхода: земля занята заимствованием,
        // а порядок остаётся порядком в массиве, а не обхода.
        for k in moved {
            let x = self.rng.below(self.side) as i32;
            let z = self.rng.below(self.side) as i32;
            let y = self.terrain.surface(x as u32, z as u32) as i32;
            self.diggers[k] = Digger {
                x,
                y,
                z,
                fresh: true,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_walker_does_not_advance_its_gait_or_fractional_position() {
        let mut w = World::new(42, 0, test_tuning());
        let id = w.units.spawn(WORLD - 1, WORLD / 2, 0, 60, 100) as usize;
        w.units.chassis[id] = modding::tuning::ChassisKind::Walking;
        w.units.vel[id] = Fx::from_int(60);
        w.units.head[id] = 0;
        w.units.fx[id] = Fx::from_ratio(1, 4);
        w.units.refresh_live();
        w.delta.resize(w.units.len(), drive::Want::default());
        w.delta[id] = drive::Want {
            way: 0,
            ..drive::Want::default()
        };
        for _ in 0..60 {
            w.apply_deltas();
        }
        assert_eq!(w.units.x[id], WORLD - 1);
        assert_eq!(w.units.fx[id], Fx::from_ratio(1, 4));
        assert_eq!(w.units.gait[id], 0);
        // The same drive away from the boundary must actually walk.
        w.units.x[id] -= 100;
        w.apply_deltas();
        assert!(w.units.x[id] > WORLD - 101);
        assert_ne!(w.units.gait[id], 0);
    }

    #[test]
    fn a_stationary_pivot_advances_steps_in_both_directions_then_settles() {
        let run = |way| {
            let mut w = World::new(249, 0, test_tuning());
            let id = w.units.spawn(400, 400, 0, 28, 100) as usize;
            w.units.chassis[id] = modding::tuning::ChassisKind::Walking;
            w.units.refresh_live();
            w.delta.resize(w.units.len(), drive::Want::default());
            w.delta[id].way = way;
            w.apply_deltas();
            assert_eq!((w.units.x[id], w.units.z[id]), (400, 400));
            assert_eq!(
                (w.units.fx[id], w.units.fz[id], w.units.vel[id]),
                (Fx::ZERO, Fx::ZERO, Fx::ZERO)
            );
            assert!(w.units.gait[id] > 0, "turning without moving must step");
            let phase = w.units.gait[id];
            w.delta[id].way = NO_WAY;
            for _ in 0..100 {
                w.apply_deltas();
            }
            assert_eq!(w.units.yaw_rate[id], 0);
            let stopped = w.units.gait[id];
            for _ in 0..60 {
                w.apply_deltas();
            }
            assert_eq!(w.units.gait[id], stopped, "idle feet must stop stepping");
            phase
        };
        assert_eq!(run(angle::FULL * 5 / 12), run(-angle::FULL * 5 / 12));
        assert!(drive::StepCycle::of(&test_tuning().tracked).is_none());
    }

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
                h = if threads > 1 {
                    w.tick_par(threads)
                } else {
                    w.tick()
                };
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
        assert!(
            mid > 0 && late > 0,
            "боя не было: {mid} и {late} осмотров по половинам"
        );
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
    const FLAT_GY: u32 = 40;
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
    /// **Убитый этим тиком не числится живым после тика** (ADR-436).
    /// Список живых пересобирала только навигация в начале следующего
    /// тика, и между тиками `living()` — а с ним подкрепление эталона —
    /// считал мёртвых.
    #[test]
    fn the_dead_leave_the_living_in_the_tick_they_die() {
        let mut w = World::new(1, 4, test_tuning());
        let a = w.units.spawn(10, 10, 0, 0, 300);
        let b = w.units.spawn(40, 10, 0, 0, 300);
        w.units.refresh_live();
        w.units.voxels[b as usize] = 0;
        w.tick();
        assert!(!w.units.alive[b as usize]);
        assert_eq!(w.living(0), 1, "the dead unit is still counted");
        assert_eq!(w.units.live(), &[a]);
    }

    fn duel(t: Tuning, ws: modding::weapons::Weapons, apart: i32) -> World {
        let vw = voxel::World::new(320, 240, 320, 100_000, Box::new(Flat));
        let mut w = World::new(2, 8, t);
        w.attach_ground(ground::Ground::new(vw, &t));
        w.attach_weapons(ws);
        w.spawn_ground(160 - apart / 2, 160, 0, 0, 300);
        w.spawn_ground(160 + apart / 2, 160, 1, 0, 300);
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
    /// Скорость, а не высота: при земном тяготении, скорости двадцать
    /// метров в секунду и дальности в десять метров вершина дуги
    /// поднимается над стрелком на **три воксела**, то есть на
    /// тридцать сантиметров. Выбрать ради видимости тяготение
    /// побольше значило бы проверять не ту сцену, в которую играют.
    #[test]
    fn a_bolt_flies_an_arc_and_the_aim_leads_by_height() {
        let mut w = duel(test_tuning(), bolt(200), 100);
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

    /// **Снаряд, которому цель не по силам, не ломает прицел** (ADR-430).
    /// Медленный болт поднимал прицел так, что квадраты выходили за
    /// разрядность `Fx`: скорость у дула была мусором, а в отладочной
    /// сборке корень из отрицательного останавливал тик. Теперь подъём
    /// упирается в потолок, и снаряд уходит круто вверх со скоростью
    /// своего оружия.
    #[test]
    fn a_bolt_too_slow_for_its_target_leaves_at_its_own_speed() {
        let mut w = duel(test_tuning(), bolt(1), 100);
        for _ in 0..40 {
            w.tick();
            if w.shots_fired > 0 {
                break;
            }
        }
        assert!(w.shots_fired > 0, "никто не стрелял");
        let i = (0..w.shots.alive.len())
            .find(|&i| w.shots.alive[i])
            .expect("снаряд в воздухе");
        let v = [w.shots.vx[i], w.shots.vy[i], w.shots.vz[i]];
        let speed2 = v.iter().fold(Fx::ZERO, |a, c| a.add(c.mul(*c)));
        let step = Fx::from_ratio(1, TICKS_PER_SECOND);
        assert!(
            speed2.sub(step.mul(step)).abs() < step.mul(step).div(Fx::from_int(10)),
            "скорость у дула {v:?}"
        );
        assert!(v[1] > Fx::ZERO, "ствол не поднят: {v:?}");
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
        t.gravity = 300;
        let apart = t.range - 10;
        let mut w = duel(t, bolt(200), apart);
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
            let mut w = duel(t, laser(), 100);
            let mut h = 0;
            for _ in 0..120 {
                h = w.tick();
            }
            (h, w.beams_fired, w.beams_hit)
        };
        let earth = run(test_tuning().gravity);
        let heavy = run(test_tuning().gravity * 100);
        assert!(
            earth.2 > 0,
            "лучи не попадали ни разу — сцена ничего не проверяет"
        );
        assert_eq!(
            earth, heavy,
            "стократное тяготение изменило сцену из одних лучей: \
хеш, выстрелы и попадания {earth:?} против {heavy:?}"
        );
    }

    /// Плоский пол и стена **в один воксель толщиной** поперёк пути.
    /// Тонкая нарочно: толстую не проскочишь, и она ничего не проверяет.
    struct Wall;
    const WALL_X: u32 = 150;
    impl voxel::Generator for Wall {
        fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
            if y < FLAT_GY || (x == WALL_X && y < FLAT_GY + 40) {
                2
            } else {
                voxel::store::AIR
            }
        }
    }

    /// Сцена для **одного** снаряда: мишень владельца 1 и никакого
    /// стрелка. Снаряд запускается руками — проверяется полёт, а не
    /// прицел, и скорость нужна та, какой в данных нет.
    fn lone_target(g: impl voxel::Generator + 'static, at_x: i32) -> (World, usize) {
        let t = test_tuning();
        let vw = voxel::World::new(320, 240, 320, 100_000, Box::new(g));
        let mut w = World::new(2, 8, t);
        w.attach_ground(ground::Ground::new(vw, &t));
        w.attach_weapons(test_weapons());
        let id = w.spawn_ground(at_x, 160, 1, 0, 300) as usize;
        w.units.refresh_live();
        (w, id)
    }

    /// Снаряд владельца 0 по оси X, `vx` вокселей за тик.
    ///
    /// Высота на полметра выше пола: допуск попадания по вертикали —
    /// рост юнита, так что мишень снаряд достанет, а пола за те
    /// несколько тиков, что он живёт, не коснётся.
    fn launch(w: &mut World, from_x: i32, vx: i32) {
        w.shots.spawn(
            (from_x, FLAT_GY as i32 + 5, 160),
            (Fx::ZERO, Fx::ZERO, Fx::ZERO),
            (Fx::from_int(vx), Fx::ZERO, Fx::ZERO),
            0,
            30,
            8,
            (0, 0),
        );
        w.shots.refresh_live();
    }

    /// **Снаряд не проскакивает цель** (ADR-173). Он летит больше
    /// клетки за тик, а допуск попадания — воксель: цель, мимо которой
    /// его пронесло между двумя тиками, в конечной точке шага
    /// не находится вовсе.
    ///
    /// Двадцать пять вокселей за тик — оснастка, а не баланс: она
    /// делает дыру видимой на одном снаряде. Мишень стоит на `165`,
    /// снаряд выходит из `100`, и концы его шагов — `125`, `150`,
    /// `175`: ни в одном из них мишени нет. Проверка только конца
    /// шага даёт ноль попаданий, и на ней тест падает.
    #[test]
    fn a_fast_bolt_does_not_fly_through_its_target() {
        let (mut w, id) = lone_target(Flat, 165);
        let hp0 = w.units.voxels[id];
        launch(&mut w, 100, 25);
        for _ in 0..10 {
            w.tick();
        }
        assert_eq!(
            w.shots_hit, 1,
            "снаряд прошёл сквозь мишень: попаданий {}, упоров в породу {}",
            w.shots_hit, w.shots_blocked
        );
        assert_eq!(hp0 - w.units.voxels[id], 30, "урон пришёл не мишени");
    }

    /// **Снаряд не проскакивает опору** (ADR-173). Линия огня исключает
    /// оба конца — цель себя не заслоняет, — и на шаге в одну клетку
    /// между концами не остаётся ни одного воксела: стена толщиной
    /// в воксель пропускала бы снаряд насквозь.
    ///
    /// Стена на `150`, снаряд выходит из `100` по двадцать пять
    /// вокселей за тик, то есть встаёт на неё ровно концом второго
    /// шага. Мишень — за стеной, на `200`: пока снаряд сквозь неё
    /// проходит, мишень получает урон, и это видно.
    #[test]
    fn a_fast_bolt_stops_at_a_wall_one_voxel_thick() {
        let (mut w, id) = lone_target(Wall, 200);
        let hp0 = w.units.voxels[id];
        launch(&mut w, 100, 25);
        for _ in 0..10 {
            w.tick();
        }
        assert_eq!(
            w.shots_blocked, 1,
            "снаряд не упёрся в стену: попаданий {}, упоров {}",
            w.shots_hit, w.shots_blocked
        );
        assert_eq!(w.shots_hit, 0, "снаряд достал мишень сквозь стену");
        assert_eq!(w.units.voxels[id], hp0, "мишень за стеной получила урон");
    }

    /// Плоский пол и **кочка в полметра** посреди него.
    /// Ровно та неровность, которую юнит перешагивает не глядя:
    /// `max_step` в данных — пять вокселей (ADR-227).
    struct Bump;
    const BUMP_X: u32 = 160;
    const BUMP_H: u32 = 5;
    impl voxel::Generator for Bump {
        fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
            if y < FLAT_GY || (x == BUMP_X && y < FLAT_GY + BUMP_H) {
                2
            } else {
                voxel::store::AIR
            }
        }
    }

    /// **Ствол на теле, а не под ногами** (ADR-173).
    ///
    /// Координата юнита — его опора, то есть пол. Снаряд, пущенный
    /// из точки опоры в точку опоры, идёт по самой земле: дуга
    /// провисает, целая часть высоты берётся округлением вниз, —
    /// и первая же кочка съедает выстрел. Через кочку в один воксель
    /// стрелять нельзя, а над ней можно, и разница целиком в высоте
    /// ствола.
    ///
    /// На стрелковой сцене всего проекта это стоит трети попаданий
    /// (`docs/analysis/10-decisions-full.md`, ADR-173). Через кочку
    /// в полметра стрелять нельзя, а над ней можно.
    #[test]
    fn the_muzzle_clears_a_bump_the_unit_steps_over() {
        let t = test_tuning();
        let vw = voxel::World::new(320, 240, 320, 100_000, Box::new(Bump));
        let mut w = World::new(2, 8, t);
        w.attach_ground(ground::Ground::new(vw, &t));
        w.attach_weapons(bolt(200));
        w.spawn_ground(BUMP_X as i32 - 50, 160, 0, 0, 300);
        w.spawn_ground(BUMP_X as i32 + 50, 160, 1, 0, 300);
        w.units.refresh_live();
        for _ in 0..120 {
            w.tick();
        }
        assert!(w.shots_fired > 0, "никто не стрелял");
        assert!(
            w.shots_hit > 0,
            "с {} выстрелов ни одного попадания через кочку в полметра: \
упоров в породу {} — снаряд идёт по земле, а не над ней",
            w.shots_fired,
            w.shots_blocked
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
        const GY: u32 = 40;
        const WALL: u32 = 160;
        const HALF: u32 = 30;
        impl voxel::Generator for Ridge {
            fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
                if y < GY {
                    return 2;
                }
                if x.abs_diff(WALL) <= HALF && y < GY + 60 {
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
                "( weapons: [ ( id: 1, kind: Bolt, speed: 200, damage: 30, reload: 30, \
blast_radius: {blast}, blast_damage: 10 ) ] )"
            ))
            .expect("реестр");

            let mut vw = voxel::World::new(320, 240, 320, 100_000, Box::new(Ridge));
            if tunnel {
                for x in WALL - HALF..=WALL + HALF {
                    for y in GY..GY + 30 {
                        for z in 140..180 {
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
            w.spawn_ground(WALL as i32 - 40, 160, 0, 0, 300);
            w.spawn_ground(WALL as i32 + 40, 160, 1, 0, 300);
            w.units.refresh_live();
            for _ in 0..200 {
                w.tick();
            }
            // Стена обязана достоять: иначе «ноль попаданий» значит
            // лишь «не успели пробить».
            let g = w.ground().expect("земля");
            let intact =
                !tunnel && g.world.solid(WALL, GY + 5, 160) && g.world.solid(WALL, GY + 15, 160);
            (w.shots_fired, w.shots_hit, w.shots_blocked, intact)
        };

        // Воронка выключена: проверяется ровно луч, а не то, успели ли
        // пробить стену.
        let (fired_wall, hit_wall, blocked_wall, intact) = run(false, 0);
        assert!(fired_wall > 0, "со стеной никто не стрелял вовсе");
        assert!(
            intact,
            "стена не устояла без воронок — тест проверяет не то"
        );
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
    /// (ADR-165): порода закрывает не навсегда. Стена в шесть метров,
    /// которую снаряд без воронки не пробивает ни разу за двести
    /// тиков, с воронками перестаёт быть стеной.
    ///
    /// Тест сторожит **обе** стороны: без воронок ноль попаданий,
    /// с воронками попадания появляются. Одна сторона без другой
    /// ничего не значит.
    #[test]
    fn a_blast_breaches_a_wall_a_bullet_cannot() {
        struct Wall;
        const GY: u32 = 40;
        const AT: u32 = 160;
        const HALF: u32 = 30;
        impl voxel::Generator for Wall {
            fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
                if y < GY {
                    return 2;
                }
                if x.abs_diff(AT) <= HALF && y < GY + 60 {
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
                "( weapons: [ ( id: 1, kind: Bolt, speed: 200, damage: 30, reload: 30, \
blast_radius: {blast}, blast_damage: 10 ) ] )"
            ))
            .expect("реестр");

            let vw = voxel::World::new(320, 240, 320, 100_000, Box::new(Wall));
            let g = ground::Ground::new(vw, &t);
            let mut w = World::new(2, 8, t);
            w.attach_ground(g);
            w.attach_weapons(ws);
            w.spawn_ground(AT as i32 - 40, 160, 0, 0, 300);
            w.spawn_ground(AT as i32 + 40, 160, 1, 0, 300);
            w.units.refresh_live();
            for _ in 0..200 {
                w.tick();
            }
            // Пролом проверяется на высоте, **на которой снаряд
            // летит**, — это половина роста юнита над полом, и
            // литералом она стоять не может: рост живёт в данных.
            // Снаряд рвёт на лицевой грани, а не забравшись в породу
            // на воксель (ADR-173), поэтому воронки выедают ход вдоль
            // линии огня, а не сферу вокруг неё, и воксель у пола
            // остаётся под ходом. Замер на blast=10: на высоте полёта
            // стена снесена целиком (130..190 пусто), а у пола цела
            // от края до края (ADR-227).
            let fly = GY + t.unit_height / 2;
            let solid = w.ground().expect("земля").world.solid(AT, fly, 160);
            (w.shots_hit, solid)
        };

        let (hit_plain, solid_plain) = run(0);
        assert_eq!(hit_plain, 0, "без воронок сквозь стену прошло {hit_plain}");
        assert!(solid_plain, "без воронок стена всё равно исчезла");

        let (hit_blast, solid_blast) = run(10);
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
                    (
                        i,
                        r.world.units.x[k],
                        r.world.units.y[k],
                        r.world.units.z[k],
                    )
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
        const GY: u32 = 40;
        const AT: u32 = 160;
        const HALF: u32 = 30;
        impl voxel::Generator for Wall {
            fn material(&self, x: u32, y: u32, _z: u32) -> voxel::store::Mat {
                if y < GY {
                    return 2;
                }
                if x.abs_diff(AT) <= HALF && y < GY + 60 {
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
            let mut vw = voxel::World::new(320, 240, 320, 100_000, Box::new(Wall));
            if !wall {
                for x in AT - HALF..=AT + HALF {
                    for y in GY..GY + 40 {
                        for z in 120..200 {
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
            w.spawn_ground(AT as i32 - 40, 160, 0, 0, 300);
            w.spawn_ground(AT as i32 + 40, 160, 1, 0, 300);
            w.units.refresh_live();
            for _ in 0..200 {
                w.tick();
            }
            let solid = w.ground().expect("земля").world.solid(AT, GY + 5, 160);
            (w.beams_fired, w.beams_hit, solid, w.beams_blocked)
        };

        let (fired, hit, solid, blocked) = run(true);
        assert_eq!(fired, 0, "по закрытой цели выстрелили {fired} раз");
        assert_eq!(hit, 0, "сквозь стену прошло {hit} попаданий лучом");
        assert!(blocked > 0, "стена не остановила ни одной попытки");
        assert!(solid, "луч проел стену — воронки у него быть не должно");

        let (fired_open, hit_open, _, _) = run(false);
        assert!(
            hit_open > 0,
            "без стены луч не попал ни разу — проверять нечего"
        );
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
                if y < 40 {
                    return 2;
                }
                // Ножка.
                if (140..180).contains(&x) && (140..180).contains(&z) && y < 220 {
                    return 3;
                }
                // Шляпка.
                if (100..220).contains(&x) && (100..220).contains(&z) && (200..220).contains(&y) {
                    return 3;
                }
                voxel::store::AIR
            }
        }
        let t = test_tuning();
        let vw = voxel::World::new(320, 320, 320, 100_000, Box::new(Mushroom));
        let mut w = World::new(1, 4, t);
        w.attach_ground(ground::Ground::new(vw, &t));
        let id = w.units.spawn(160, 160, 0, 0, 300) as usize;
        w.units.refresh_live();
        w.tick();
        let y0 = w.units.y[id];
        assert!(y0 > 200, "юнит не встал на шляпку: y = {y0}");

        // Подрезать ножку.
        {
            let g = w.ground_mut().expect("земля");
            for y in 45..105 {
                for z in 140..180 {
                    for x in 140..180 {
                        if g.world.solid(x, y, z) {
                            g.world.queue_dig(x, y, z);
                        }
                    }
                }
            }
        }

        let mut rode = 0u32;
        let mut checked = 0u32;
        for tick in 0..200 {
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
    /// **Ядро движения считает то же, что боевая фаза.**
    ///
    /// `movement_kernel` документирован как «вызов из любого потока
    /// даёт то же самое» и существует ради пула. Но источник
    /// направления у него был **другой**: плоское поле вместо
    /// объёмного, которое читает `movement_deltas` при наличии земли.
    /// Метод, обещающий тождество и его не дающий, — заготовленный
    /// десинк: он сработает у первого, кто повесит на него пул
    /// (ADR-194).
    ///
    /// Мир здесь **с землёй** намеренно: на мире без земли обе
    /// ветки совпадали, и расхождения не было видно — именно поэтому
    /// его никто и не заметил.
    #[test]
    fn the_movement_kernel_matches_the_phase_it_stands_for() {
        let t = test_tuning();
        let mut r = Reference::with_params(
            42,
            200,
            t,
            test_weapons(),
            worldgen::Params::sized(480, 240, 480),
        );
        for _ in 0..40 {
            r.tick();
        }
        // Треть юнитов под приказом: у ядра три ветки — поле стороны,
        // точка приказа и «стоять», — и тождество обязано держаться
        // во всех трёх. Проверяй его только на первой, и расхождение
        // в остальных ждало бы первого, кто повесит пул (ADR-195).
        let ids: Vec<u32> = r.world.units.live().to_vec();
        for (k, &i) in ids.iter().enumerate() {
            match k % 3 {
                0 => r.world.push_order(Order::MoveTo {
                    unit: i,
                    x: 50,
                    z: 400,
                }),
                1 => r.world.push_order(Order::Hold { unit: i }),
                _ => {}
            }
        }
        r.tick();
        let w = &mut r.world;
        assert!(
            w.units
                .live()
                .iter()
                .any(|&i| w.units.stance[i as usize] == STANCE_MOVE),
            "приказы не применились — проверяется одна ветка из трёх"
        );
        w.phase_nav_update();
        w.prepare_movement();
        w.movement_deltas(1);
        let live: Vec<u32> = w.units.live().to_vec();
        assert!(
            live.len() > 20,
            "юнитов слишком мало, тест ничего не проверит"
        );
        let mut differ = 0usize;
        for &i in &live {
            let i = i as usize;
            if w.movement_kernel(i) != w.delta[i] {
                differ += 1;
            }
        }
        assert_eq!(
            differ,
            0,
            "{differ} из {} юнитов: ядро и фаза разошлись",
            live.len()
        );
        assert!(
            w.ground().is_some(),
            "мир без земли — обе ветки совпадают, тест вакуумный"
        );
    }

    /// SyncTest).
    #[test]
    fn two_worlds_run_side_by_side_match_every_tick() {
        let mut a = Reference::new(42, 800, test_tuning(), test_weapons());
        let mut b = Reference::new(42, 800, test_tuning(), test_weapons());
        for t in 1..=150 {
            let ha = a.tick();
            let hb = b.tick();
            assert_eq!(
                ha, hb,
                "миры разошлись на тике {t}: {ha:016x} против {hb:016x}"
            );
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
        assert_eq!(
            w.units.len(),
            n,
            "хранилище выросло вместо переиспользования"
        );
    }

    /// Выбор цели не зависит от порядка обхода: при равном расстоянии
    /// побеждает меньший индекс.
    #[test]
    fn ties_resolve_by_index() {
        let mut w = World::new(2, 64, test_tuning());
        let me = w.units.spawn(500, 500, 0, 0, 100);
        // Две цели на одинаковом расстоянии.
        let a = w.units.spawn(550, 500, 1, 0, 100);
        let b = w.units.spawn(450, 500, 1, 0, 100);
        assert!(a < b);
        w.units.refresh_live();
        w.tick();
        assert_eq!(
            w.units.target[me as usize], a,
            "ничья ушла не меньшему индексу"
        );
    }

    // --- канал приказов (ADR-195)

    /// Один юнит на плоскости, поле стороны — в одну сторону.
    /// Приказ будет отдан в другую, и разойтись они обязаны.
    fn lone_walker(t: Tuning) -> (World, u32) {
        let vw = voxel::World::new(480, 160, 480, 4096, Box::new(Flat));
        let mut w = World::new(2, 8, t);
        w.attach_ground(ground::Ground::new(vw, &t));
        let id = w.spawn_ground(80, 80, 0, 30, 300);
        w.units.refresh_live();
        // Поле стороны ведёт на восток: без приказа юнит идёт туда.
        w.fill_flow_towards(0, 400, 80);
        w.solve_flow();
        (w, id)
    }

    /// **Приказ уводит юнита туда, куда поле стороны его не вело.**
    ///
    /// Сторож на сам приказ: если фаза `ApplyCommands` ничего не
    /// делает, юнит уходит по полю на восток, и обе проверки падают
    /// разом — и «дошёл куда велено», и «не ушёл туда, куда вело поле».
    #[test]
    fn an_order_takes_the_unit_where_the_field_would_not() {
        let t = test_tuning();
        // Без приказа: юнит идёт по полю на восток.
        let (mut free, id) = lone_walker(t);
        for _ in 0..240 {
            free.tick();
        }
        let i = id as usize;
        assert!(
            free.units.x[i] > 200,
            "юнит без приказа не пошёл по полю: x={}",
            free.units.x[i]
        );

        // С приказом: юнит идёт на юг, к точке приказа. Тиков вдвое
        // больше, чем на проверку поля: с ходом по массе (ADR-234)
        // машина разгоняется и доворачивает не мгновенно, и дорога
        // в двести вокселей занимает секунд пятнадцать.
        let (mut w, id) = lone_walker(t);
        w.push_order(Order::MoveTo {
            unit: id,
            x: 80,
            z: 400,
        });
        for _ in 0..600 {
            w.tick();
        }
        let (x, z) = (w.units.x[i], w.units.z[i]);
        let d2 = (x - 80) * (x - 80) + (z - 400) * (z - 400);
        let arrive = t.order_arrive + 20;
        assert!(
            d2 <= arrive * arrive,
            "юнит по приказу оказался в ({x}, {z}), а звали в (80, 400)"
        );
        assert!(x < 200, "юнит всё-таки ушёл по полю стороны: x={x}");
        assert_eq!(w.orders_applied, 1);
        assert_eq!(w.orders_dropped, 0);
    }

    /// **Точка приказа за краем мира не роняет симуляцию.**
    ///
    /// Координаты приказа шли в квадраты расстояний в `i32` как есть:
    /// `MoveTo` с `x = 100 000` переполнял их в фазе движения у всех
    /// клиентов lockstep. Ход прижимается к краю, копка и стройка
    /// за краем отбрасываются (ADR-417).
    #[test]
    fn an_order_beyond_the_world_is_clamped_or_dropped() {
        let t = test_tuning();
        let (mut w, id) = lone_walker(t);
        let i = id as usize;
        w.push_order(Order::MoveTo {
            unit: id,
            x: 100_000,
            z: i32::MIN,
        });
        w.push_order(Order::Dig {
            unit: id,
            x: 10,
            y: i32::MIN,
            z: 10,
        });
        w.push_order(Order::Build {
            unit: id,
            kind: 0,
            x: -5,
            z: 40,
        });
        for _ in 0..30 {
            w.tick();
        }
        let (ex, _) = w.extent();
        assert_eq!((w.units.goal_x[i], w.units.goal_z[i]), (ex - 1, 0));
        assert_eq!(w.units.stance[i], STANCE_MOVE, "копка за краем принята");
        assert_eq!((w.orders_applied, w.orders_dropped), (1, 2));
    }

    /// **Новый приказ отменяет прежний.**
    ///
    /// Игрок щёлкает в другое место — юнит идёт туда, а не доходит
    /// до старой точки. Проверяется разворотом: сначала на юг,
    /// потом на север, и конец обязан быть севернее начала.
    #[test]
    fn a_second_order_replaces_the_first() {
        let t = test_tuning();
        let (mut w, id) = lone_walker(t);
        let i = id as usize;
        w.push_order(Order::MoveTo {
            unit: id,
            x: 80,
            z: 400,
        });
        for _ in 0..90 {
            w.tick();
        }
        let midway = w.units.z[i];
        assert!(
            midway > 120,
            "юнит не тронулся по первому приказу: z={midway}"
        );
        w.push_order(Order::MoveTo {
            unit: id,
            x: 80,
            z: 40,
        });
        for _ in 0..300 {
            w.tick();
        }
        let z = w.units.z[i];
        assert!(
            z < midway,
            "юнит не развернулся по второму приказу: z={z}, на развороте было {midway}"
        );
        assert!(
            (z - 40).abs() <= t.order_arrive + 20,
            "юнит развернулся, но до второй точки не дошёл: z={z}"
        );
        assert_eq!(w.orders_applied, 2);
    }

    /// **Приказ «стоять» держит юнита на месте.**
    #[test]
    fn a_hold_order_stops_the_unit() {
        let t = test_tuning();
        let (mut w, id) = lone_walker(t);
        w.push_order(Order::Hold { unit: id });
        let i = id as usize;
        let (x0, z0) = (w.units.x[i], w.units.z[i]);
        for _ in 0..120 {
            w.tick();
        }
        assert_eq!((w.units.x[i], w.units.z[i]), (x0, z0), "стоящий сдвинулся");
        // И отмена возвращает его под поле стороны.
        w.push_order(Order::Release { unit: id });
        for _ in 0..120 {
            w.tick();
        }
        assert!(
            w.units.x[i] > x0,
            "отменённый приказ не вернул юнита под поле"
        );
    }

    /// **Приказ входит в хеш — через движение, которое он задаёт.**
    ///
    /// Одинаковая последовательность приказов на одинаковом сиде даёт
    /// одинаковый хеш; разная — разный. Второе важнее: приказ, который
    /// хеш не замечает, можно переписать целиком и не узнать об этом
    /// (ADR-116).
    #[test]
    fn the_same_orders_hash_the_same_and_different_ones_do_not() {
        let t = test_tuning();
        let run = |gx: i32, gz: i32| -> u64 {
            let (mut w, id) = lone_walker(t);
            w.push_order(Order::MoveTo {
                unit: id,
                x: gx,
                z: gz,
            });
            let mut h = 0u64;
            for _ in 0..40 {
                h = w.tick();
            }
            h
        };
        assert_eq!(
            run(80, 400),
            run(80, 400),
            "одинаковые приказы разошлись по хешу"
        );
        assert_ne!(run(80, 400), run(400, 80), "разные приказы дали один хеш");
    }

    /// **Потолок откладывает остаток, а не теряет его.**
    ///
    /// Очередь строго FIFO: упёршийся приказ ждёт следующего тика.
    /// Потерянный приказ — это десинк у всех, кто его не потерял.
    #[test]
    fn the_order_cap_defers_the_rest_and_loses_nothing() {
        let mut t = test_tuning();
        t.max_orders_per_tick = 4;
        let mut w = World::new(2, 32, t);
        let ids: Vec<u32> = (0..10)
            .map(|k| w.units.spawn(k * 4, 0, 0, 1, 100))
            .collect();
        w.units.refresh_live();
        for &id in &ids {
            w.push_order(Order::Hold { unit: id });
        }
        assert_eq!(w.orders_pending(), 10);
        w.tick();
        assert_eq!(w.orders_applied, 4, "потолок не сработал");
        assert_eq!(w.orders_pending(), 6, "остаток пропал");
        assert_eq!(w.orders_capped, 1);
        w.tick();
        w.tick();
        assert_eq!(w.orders_applied, 10, "отложенные приказы потерялись");
        assert_eq!(w.orders_pending(), 0);
        for &id in &ids {
            assert_eq!(w.units.stance[id as usize], STANCE_HOLD);
        }
    }

    /// **Слот из свободного списка не наследует приказа покойника.**
    ///
    /// Удаление — free-list, и слот переиспользуется (`CLAUDE.md`).
    /// Забудь обнулить приказ в `spawn` — и подкрепление, попавшее
    /// в место погибшего копателя, пошло бы грызть его забой.
    #[test]
    fn a_reused_slot_never_inherits_the_order_of_the_dead() {
        let mut w = World::new(2, 8, test_tuning());
        let id = w.units.spawn(10, 10, 0, 1, 100);
        w.units.refresh_live();
        w.push_order(Order::Dig {
            unit: id,
            x: 10,
            y: 5,
            z: 10,
        });
        w.tick();
        assert_eq!(w.units.stance[id as usize], STANCE_DIG);
        w.units.kill(id);
        w.units.refresh_live();
        let again = w.units.spawn(20, 20, 1, 1, 100);
        assert_eq!(
            again, id,
            "слот не переиспользован — тест ничего не проверяет"
        );
        assert_eq!(
            w.units.stance[again as usize], STANCE_FLOW,
            "приказ пережил смерть"
        );
        assert_eq!(
            (
                w.units.goal_x[again as usize],
                w.units.goal_y[again as usize]
            ),
            (0, 0),
            "точка приказа пережила смерть"
        );
    }

    /// **Приказ мёртвому отбрасывается, а не роняет тик.**
    #[test]
    fn an_order_to_the_dead_is_dropped() {
        let mut w = World::new(2, 8, test_tuning());
        let id = w.units.spawn(10, 10, 0, 1, 100);
        w.units.refresh_live();
        w.units.kill(id);
        w.units.refresh_live();
        w.push_order(Order::MoveTo {
            unit: id,
            x: 1,
            z: 1,
        });
        w.push_order(Order::MoveTo {
            unit: 999,
            x: 1,
            z: 1,
        });
        w.tick();
        assert_eq!(w.orders_dropped, 2);
        assert_eq!(w.orders_applied, 0);
    }

    /// **Приказ атаковать держит названного, а не ближайшего.**
    ///
    /// Без него фаза боя выбирает ближайшего сама, и приказ игрока
    /// был бы отменён в том же тике, в котором отдан.
    #[test]
    fn an_attack_order_holds_the_named_enemy_over_the_nearest() {
        let mut w = World::new(2, 16, test_tuning());
        let me = w.units.spawn(32, 32, 0, 0, 300);
        let near = w.units.spawn(42, 32, 1, 0, 300);
        let far = w.units.spawn(52, 32, 1, 0, 300);
        w.units.refresh_live();
        w.push_order(Order::Attack {
            unit: me,
            target: far,
        });
        w.tick();
        assert_eq!(
            w.units.target[me as usize], far,
            "цель перевыбрана на ближайшую: приказ игрока не удержался"
        );
        // Свой своему не цель: такой приказ отбрасывается.
        w.push_order(Order::Attack {
            unit: near,
            target: far,
        });
        w.tick();
        assert_eq!(w.orders_dropped, 1, "приказ бить своего прошёл");
    }

    /// **Приказ копать снимает породу — той же очередью, что и всё.**
    ///
    /// Копка — экономика проекта, и приказ, который до вокселей
    /// не доходит, проверяет только себя.
    #[test]
    fn a_dig_order_actually_removes_rock() {
        let t = test_tuning();
        let vw = voxel::World::new(320, 160, 320, 4096, Box::new(Flat));
        let mut w = World::new(2, 8, t);
        w.attach_ground(ground::Ground::new(vw, &t));
        // **Середина нав-клетки, а не её угол.** Шар копателя —
        // метр радиусом, клетка — метр стороной, и шар, поставленный
        // в угол клетки, до дальнего её угла не достаёт: диагональ
        // в 1.41 раза длиннее. На сетке в полметра этого видно не было —
        // там клетка была два воксела, и шар накрывал её целиком
        // из любого места (ADR-227).
        let id = w.spawn_ground(165, 165, 0, 30, 300);
        w.units.refresh_live();
        let at = (FLAT_GY - 1) as i32;
        assert!(
            w.ground().expect("земля").world.solid(165, at as u32, 165),
            "под юнитом не порода — копать нечего"
        );
        w.push_order(Order::Dig {
            unit: id,
            x: 165,
            y: at,
            z: 165,
        });
        for _ in 0..30 {
            w.tick();
        }
        let g = w.ground().expect("земля");
        assert!(
            !g.world.solid(165, at as u32, 165),
            "порода на месте: приказ не копал"
        );
        // Копатель уходит в собственную шахту: опора исчезла у него
        // под ногами, и падение по ADR-036 — тот же код, что у всех.
        assert!(
            w.units.y[id as usize] < at,
            "копатель остался на поверхности: y={}",
            w.units.y[id as usize]
        );
        // И дно мира на месте: колонка без пола оставила бы юнита
        // висеть в воздухе.
        assert!(g.world.solid(165, 0, 165), "снят нижний слой мира");
        // Забой углубляется: копатель уходит вниз, а не грызёт воздух
        // на одном месте (ADR-159).
        assert!(
            w.units.goal_y[id as usize] < at,
            "забой не углубился: y остался {}",
            w.units.goal_y[id as usize]
        );
    }
}
