//! **Петля замкнута — тестом, а не глазами.**
//!
//! Копатель грызёт забой → материя попадает в бак → бак едет на склад →
//! у стороны растёт запас → из запаса строится завод → завод делает
//! машину → машина встаёт под управление. Каждое звено проверяется
//! отдельно, и все вместе — прогоном.
//!
//! Числа берутся **из тех же данных, что у игры** (`include_str!`
//! разворачивается на сборке, ввода-вывода в тесте не появляется):
//! литералы здесь были бы вторым источником правды и разошлись бы молча.

use sim_core::economy::NO_CONTAINER;
use sim_core::matter::Sink;
use sim_core::{ground::Ground, Order, Outcome, World};
use voxel::store::{Mat, AIR};

/// Плита породы до `TOP`, воздух выше. Порода одна на весь мир:
/// сцена проверяет движение материи, а не залегание.
struct Slab;
const TOP: u32 = 40;
/// Номер породы из `data/materials.ron`.
const ROCK: Mat = 2;

impl voxel::Generator for Slab {
    fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
        if y < TOP {
            ROCK
        } else {
            AIR
        }
    }
}

fn tuning() -> sim_core::Tuning {
    sim_core::Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("data/sim.ron")
}

fn kinds() -> modding::Kinds {
    modding::kinds::from_ron(
        include_str!("../../../data/kinds.ron"),
        sim_core::TICKS_PER_SECOND as u32,
    )
    .expect("data/kinds.ron")
}

fn mats() -> modding::Registry {
    modding::data::materials_from_ron(include_str!("../../../data/materials.ron"))
        .expect("data/materials.ron")
}

/// Номера типов из `data/kinds.ron`. Явные, как везде (ADR-028).
const CRAWLER: u16 = 2;
const FACTORY: u16 = 10;
const DEPOT: u16 = 11;
const TANK: u16 = 1;

fn world() -> World {
    sized_world(128)
}

fn sized_world(side: u32) -> World {
    let vw = voxel::World::new(side, 64, side, 1 << 16, Box::new(Slab));
    let t = tuning();
    let mut w = World::new(2, 64, t);
    w.attach_ground(Ground::new(vw, &t));
    w.attach_weapons(
        modding::weapons::from_ron(include_str!("../../../data/weapons.ron"))
            .expect("data/weapons.ron"),
    );
    w.attach_economy(kinds(), mats());
    w
}

/// **Выкопанное доходит до бака.**
///
/// Первое звено: копатель грызёт забой, и материя оказывается в его
/// контейнере, а не в отладочном счётчике.
#[test]
fn digging_fills_the_tank() {
    let mut w = world();
    let d = w.spawn_kind(30, 30, 0, CRAWLER).expect("краулер");
    w.units.refresh_live();
    let c = w.units.store[d as usize];
    assert_ne!(c, NO_CONTAINER, "у краулера нет контейнера");
    // Тело уже сделано из материи: машина, поставленная на карту,
    // из чего-то состоит. Везёт она то, что сверх тела.
    let body = w
        .econ
        .as_ref()
        .expect("учёт")
        .kinds
        .get(CRAWLER)
        .expect("краулер")
        .cost;
    assert_eq!(w.econ.as_ref().expect("учёт").ledger.held(c), body);
    assert_eq!(w.cargo(d as usize), 0, "новый краулер уже с грузом");
    w.push_order(Order::Dig {
        unit: d,
        x: 30,
        y: TOP as i32 - 1,
        z: 30,
    });
    for t in 0..8 {
        w.tick();
        let e = w.econ.as_ref().expect("учёт");
        println!(
            "тик {t}: выкопано {} везёт {}",
            e.mined,
            w.cargo(d as usize)
        );
    }
    let e = w.econ.as_ref().expect("учёт");
    assert!(e.mined > 0, "копатель не набрал ничего");
    assert!(w.cargo(d as usize) > 0, "бак пуст");
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
}

/// **Полный бак едет на склад, и у стороны растёт запас.**
///
/// Второе и третье звено разом: копатель сам переходит в возку, сам
/// находит склад и сам разгружается. Игрок при этом отдал **один**
/// приказ — копать.
#[test]
fn a_full_tank_is_hauled_to_a_depot_and_the_side_gets_richer() {
    // Склад **далеко**: возка должна быть поездкой, а не разгрузкой
    // на месте. Иначе сцена проверяет перевод между контейнерами,
    // а не то, что бак едет.
    let mut w = sized_world(256);
    let dep = w.spawn_kind(210, 210, 0, DEPOT).expect("склад");
    let d = w.spawn_kind(40, 40, 0, CRAWLER).expect("краулер");
    w.units.refresh_live();
    assert_eq!(w.units.site[dep as usize], 0, "склад обязан быть достроен");
    // Забой в стороне от копателя: шар снимает породу у него из-под
    // ног, если целиться себе под гусеницы.
    w.push_order(Order::Dig {
        unit: d,
        x: 100,
        y: TOP as i32 - 1,
        z: 40,
    });
    let before = w.econ.as_ref().expect("учёт").stock(0);
    let start = (w.units.x[d as usize], w.units.z[d as usize]);
    let mut hauled = false;
    let mut moved = false;
    for _ in 0..1200 {
        w.tick();
        if w.units.stance[d as usize] == sim_core::STANCE_HAUL {
            hauled = true;
            if (w.units.x[d as usize], w.units.z[d as usize]) != start {
                moved = true;
            }
        }
        if w.econ.as_ref().expect("учёт").delivered > 0 {
            break;
        }
    }
    let e = w.econ.as_ref().expect("учёт");
    assert!(hauled, "краулер не поехал на склад с полным баком");
    assert!(moved, "краулер с полным баком стоял на месте");
    assert!(e.delivered > 0, "на склад не пришло ничего");
    assert!(e.stock(0) > before, "запас стороны не вырос");
    // **Сток с первого дня сработал.** Выход переработки меньше
    // единицы — единственное, что не даёт партии стать вечной
    // (`docs/07-economy.md`).
    assert!(
        e.sunk(Sink::RefiningLoss) > 0,
        "переработка не списала ничего — выход равен единице, стока нет"
    );
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
}

/// **Полный склад не уничтожает груз.**
///
/// Переработка списывает в объявленный сток всё взятое, что не ушло
/// приёмнику. Сдай полный бак в полный запас — и груз целиком уйдёт
/// в «выход переработки < 1»: это было бы не стоком, а уничтожением
/// груза полным складом, и выглядело бы как работающая переработка.
#[test]
fn a_full_stock_does_not_eat_the_cargo() {
    let mut w = world();
    let _dep = w.spawn_kind(40, 30, 0, DEPOT).expect("склад");
    let d = w.spawn_kind(44, 30, 0, CRAWLER).expect("краулер");
    w.units.refresh_live();
    w.tick();
    // Запас забит под потолок: место кончилось.
    {
        let e = w.econ.as_mut().expect("учёт");
        let stock = e.banks[0].stock;
        let room = e.ledger.room(stock);
        e.ledger.reveal(e.ground, room);
        e.ledger.transfer(e.ground, stock, room);
        assert_eq!(e.ledger.room(stock), 0, "запас не полон");
    }
    let lost_before = w.econ.as_ref().expect("учёт").sunk(Sink::RefiningLoss);
    // Забой рядом: копатель наберёт бак и приедет к складу.
    w.push_order(Order::Dig {
        unit: d,
        x: 60,
        y: TOP as i32 - 1,
        z: 30,
    });
    for _ in 0..600 {
        w.tick();
    }
    let e = w.econ.as_ref().expect("учёт");
    assert!(
        w.cargo(d as usize) > 0,
        "груз пропал: полный склад его съел"
    );
    assert_eq!(
        e.sunk(Sink::RefiningLoss),
        lost_before,
        "полный склад списал груз в сток переработки"
    );
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
}

/// **Постройка растёт из запаса, воксель за вокселем.**
#[test]
fn a_building_grows_out_of_the_stock_voxel_by_voxel() {
    let mut w = world();
    let b = w.spawn_kind(40, 40, 0, CRAWLER).expect("краулер");
    w.units.refresh_live();
    // Запас наполняется через землю: материя приходит в экономику
    // только оттуда, и тест не имеет права её выдумывать.
    {
        let e = w.econ.as_mut().expect("учёт");
        let stock = e.banks[0].stock;
        e.ledger.reveal(e.ground, 2_000_000);
        e.ledger.transfer(e.ground, stock, 2_000_000);
    }
    w.push_order(Order::Build {
        unit: b,
        kind: FACTORY,
        x: 44,
        z: 40,
    });
    w.tick();
    let site = w.units.job[b as usize];
    assert_ne!(site, sim_core::NO_TARGET, "недострой не появился");
    let s = site as usize;
    assert!(w.units.site[s] > 0, "недострой достроен сразу");
    let full = w
        .econ
        .as_ref()
        .expect("учёт")
        .kinds
        .get(FACTORY)
        .expect("завод")
        .voxels;
    assert!(
        w.units.voxels[s] < full / 4,
        "недострой начинается почти готовым: {} из {full}",
        w.units.voxels[s]
    );
    let mut grew = 0u32;
    let mut last = w.units.voxels[s];
    for _ in 0..2000 {
        w.tick();
        if w.units.voxels[s] > last {
            grew += 1;
            last = w.units.voxels[s];
        }
        if w.units.site[s] == 0 {
            break;
        }
    }
    assert_eq!(w.units.site[s], 0, "завод так и не достроился");
    assert!(
        grew > 4,
        "том рос не постепенно, а рывком: приростов {grew}"
    );
    let e = w.econ.as_ref().expect("учёт");
    assert_eq!(e.built, 1);
    assert!(e.spent > 0, "на стройку ничего не потрачено");
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
    // Вложенное лежит **в теле постройки**: тело юнита — контейнер.
    assert!(
        e.ledger.held(w.units.store[s]) > 0,
        "в постройке нет материи"
    );
}

/// **Группа строит одну постройку, а не по одной на каждого.**
///
/// Приказ отдаётся каждому выделенному юниту отдельно, и без проверки
/// «недострой тут уже есть» щелчок по месту при двадцати выделенных
/// краулерах разметил бы двадцать фундаментов в одной точке.
///
/// Ловится это только так: все приказы применяются в **одном** дренаже,
/// а список живых пересобирают в начале тика — значит второй приказ
/// первого не увидит, если искать по списку живых.
#[test]
fn a_group_builds_one_thing_not_one_each() {
    let mut w = world();
    let mut crew = Vec::new();
    for k in 0..6i32 {
        crew.push(w.spawn_kind(40 + k * 4, 40, 0, CRAWLER).expect("краулер"));
    }
    w.units.refresh_live();
    {
        let e = w.econ.as_mut().expect("учёт");
        let stock = e.banks[0].stock;
        e.ledger.reveal(e.ground, 4_000_000);
        e.ledger.transfer(e.ground, stock, 4_000_000);
    }
    let before = w.units.live().len();
    for &id in &crew {
        w.push_order(Order::Build {
            unit: id,
            kind: DEPOT,
            x: 60,
            z: 40,
        });
    }
    w.tick();
    assert_eq!(
        w.econ.as_ref().expect("учёт").sites().len(),
        1,
        "шесть приказов разметили {} фундаментов вместо одного",
        w.econ.as_ref().expect("учёт").sites().len()
    );
    assert_eq!(w.units.live().len(), before + 1, "появился лишний юнит");
    // И все шестеро работают над одним и тем же.
    let site = w.units.job[crew[0] as usize];
    for &id in &crew {
        assert_eq!(w.units.job[id as usize], site, "краулеры строят разное");
    }
    // Достроив, они снимают его из списка: иначе к покойнику или
    // к готовой постройке пристраивались бы следующие приказы.
    for _ in 0..4000 {
        w.tick();
        if w.units.site[site as usize] == 0 {
            break;
        }
    }
    assert_eq!(w.units.site[site as usize], 0, "склад не достроился");
    assert!(
        w.econ.as_ref().expect("учёт").sites().is_empty(),
        "достроенный остался в списке недостроев"
    );
}

/// **Завод делает машину за материю и время, и она выходит живой.**
///
/// Проверяется и то, и другое: деньги вперёд времени не двигают.
/// Иначе богатая сторона получала бы армию мгновенно.
#[test]
fn a_factory_turns_matter_and_time_into_a_machine() {
    let mut w = world();
    let f = w.spawn_kind(40, 40, 0, FACTORY).expect("завод");
    w.units.refresh_live();
    {
        let e = w.econ.as_mut().expect("учёт");
        let stock = e.banks[0].stock;
        e.ledger.reveal(e.ground, 4_000_000);
        e.ledger.transfer(e.ground, stock, 4_000_000);
    }
    let before = w.units.live().len();
    w.push_order(Order::Produce {
        unit: f,
        kind: TANK,
    });
    w.tick();
    assert_eq!(
        w.econ
            .as_ref()
            .expect("учёт")
            .line(f)
            .map(|l| l.queue.len()),
        Some(1),
        "заказ не встал в очередь"
    );
    let ticks_need = {
        let e = w.econ.as_ref().expect("учёт");
        e.kinds.get(TANK).expect("танк").build_ticks
    };
    for _ in 0..ticks_need.saturating_sub(2) {
        w.tick();
    }
    assert_eq!(
        w.units.live().len(),
        before,
        "машина вышла раньше срока — время не считается"
    );
    for _ in 0..8 {
        w.tick();
    }
    let e = w.econ.as_ref().expect("учёт");
    assert_eq!(e.produced, 1, "завод не выпустил машину");
    assert!(w.units.live().len() > before, "машины нет среди живых");
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
    // Машина вышла **под управление**: у неё свой тип, своё оружие
    // и свой контейнер-тело.
    let made = *w.units.live().iter().find(|&&i| i != f).expect("машина");
    let k = made as usize;
    assert_eq!(w.units.owner[k], 0);
    assert_ne!(w.units.store[k], NO_CONTAINER);
    let cost = e.kinds.get(TANK).expect("танк").cost;
    assert_eq!(
        e.ledger.held(w.units.store[k]),
        cost,
        "материя заказа не переехала в тело машины"
    );
}

/// **Бедная сторона не строит.** Запас — это ограничение, а не
/// украшение: без материи очередь стоит, а не идёт бесплатно.
#[test]
fn an_empty_stock_stops_production() {
    let mut w = world();
    let f = w.spawn_kind(40, 40, 0, FACTORY).expect("завод");
    w.units.refresh_live();
    let before = w.units.live().len();
    w.push_order(Order::Produce {
        unit: f,
        kind: TANK,
    });
    for _ in 0..600 {
        w.tick();
    }
    let e = w.econ.as_ref().expect("учёт");
    assert_eq!(e.produced, 0, "машина вышла из пустого запаса");
    assert_eq!(w.units.live().len(), before);
}

/// **Отмена заказа — не сток.** Цикл «заказал — отменил» иначе стал бы
/// способом сжигать материю мимо перечня (`docs/07-economy.md`).
#[test]
fn cancelling_an_order_returns_the_matter() {
    let mut w = world();
    let f = w.spawn_kind(40, 40, 0, FACTORY).expect("завод");
    w.units.refresh_live();
    {
        let e = w.econ.as_mut().expect("учёт");
        let stock = e.banks[0].stock;
        e.ledger.reveal(e.ground, 4_000_000);
        e.ledger.transfer(e.ground, stock, 4_000_000);
    }
    let full = w.econ.as_ref().expect("учёт").stock(0);
    w.push_order(Order::Produce {
        unit: f,
        kind: TANK,
    });
    for _ in 0..40 {
        w.tick();
    }
    let mid = w.econ.as_ref().expect("учёт").stock(0);
    assert!(mid < full, "за сорок тиков не уплачено ничего");
    w.push_order(Order::Cancel { unit: f });
    w.tick();
    let e = w.econ.as_ref().expect("учёт");
    assert_eq!(e.stock(0), full, "уплаченное не вернулось в запас");
    assert!(e.line(f).is_none(), "пустая очередь осталась висеть");
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
}

/// **Склад поднимает потолок, а его гибель — роняет** (ADR-043).
///
/// Порядок при разрушении фиксирован каноном, и излишек над упавшим
/// потолком уходит в **объявленный** сток, а не пропадает молча.
#[test]
fn a_depot_raises_the_cap_and_its_death_lowers_it() {
    let mut w = world();
    let dep = w.spawn_kind(40, 40, 0, DEPOT).expect("склад");
    w.units.refresh_live();
    let base = tuning().base_store;
    w.tick();
    let with = w.econ.as_ref().expect("учёт").cap(0);
    assert!(with > base, "склад не поднял потолок: {with} против {base}");
    // Наполнить сверх базового потолка — через землю, как всегда.
    {
        let e = w.econ.as_mut().expect("учёт");
        let stock = e.banks[0].stock;
        e.ledger.reveal(e.ground, with);
        e.ledger.transfer(e.ground, stock, with);
    }
    assert!(w.econ.as_ref().expect("учёт").stock(0) > base);
    // Снести склад. Двух тиков: уборка идёт после экономики, и потолок
    // пересчитывается уже без него — на следующем тике.
    w.units.voxels[dep as usize] = 0;
    w.tick();
    w.tick();
    let e = w.econ.as_ref().expect("учёт");
    assert_eq!(e.cap(0), base, "потолок не упал обратно");
    assert!(e.stock(0) <= base, "запас остался выше упавшего потолка");
    assert!(
        e.sunk(sim_core::matter::Sink::OverCap) > 0,
        "излишек над упавшим потолком пропал мимо стока"
    );
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
}

/// **Подбитая машина оставляет обломок, и его можно реклеймить.**
///
/// Реклейм — **та же операция, что копка** (ADR-017): приказ копать
/// в точку обломка переливает его содержимое в бак. Никакого
/// отдельного механизма нет и быть не должно.
#[test]
fn a_dead_machine_leaves_a_wreck_that_is_reclaimed_by_digging() {
    let mut w = world();
    let dead = w.spawn_kind(60, 60, 0, TANK).expect("танк");
    let taker = w.spawn_kind(62, 60, 0, CRAWLER).expect("краулер");
    w.units.refresh_live();
    // Тело танка — контейнер, и в нём уже лежит его стоимость:
    // машина, поставленная на карту, из чего-то состоит.
    {
        let e = w.econ.as_ref().expect("учёт");
        let cost = e.kinds.get(TANK).expect("танк").cost;
        assert_eq!(e.ledger.held(w.units.store[dead as usize]), cost);
    }
    w.units.voxels[dead as usize] = 0;
    w.tick();
    let e = w.econ.as_ref().expect("учёт");
    assert_eq!(e.wrecks.len(), 1, "обломка не осталось");
    assert!(
        e.sunk(Sink::ContainerBreach) > 0,
        "доля при разрушении не списана в объявленный сток"
    );
    let in_wreck = e.ledger.held(e.wrecks[0].held);
    assert!(in_wreck > 0);
    // Копать в точку обломка — и он перельётся в бак.
    let (wx, wy, wz) = (e.wrecks[0].x, e.wrecks[0].y, e.wrecks[0].z);
    w.push_order(Order::Dig {
        unit: taker,
        x: wx,
        y: wy,
        z: wz,
    });
    for _ in 0..40 {
        w.tick();
        if w.econ.as_ref().expect("учёт").reclaimed > 0 {
            break;
        }
    }
    let e = w.econ.as_ref().expect("учёт");
    assert!(e.reclaimed > 0, "обломок не реклеймится копкой");
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
}

/// **У стороны не осталось живых — она проиграла.**
///
/// Условие простое и проверяемое, и постройка считается живой наравне
/// с машиной: сторона с одним складом ещё в игре.
#[test]
fn a_side_with_nothing_alive_has_lost() {
    let mut w = world();
    let mine = w.spawn_kind(20, 20, 0, TANK).expect("танк");
    let theirs = w.spawn_kind(100, 100, 1, DEPOT).expect("склад");
    w.units.refresh_live();
    w.tick();
    assert_eq!(
        w.outcome(),
        Outcome::Fighting,
        "партия кончилась, не начавшись"
    );
    // Склад — не машина, но он живой: сторона ещё в игре.
    assert_eq!(w.living(1), 1);
    w.units.voxels[theirs as usize] = 0;
    w.tick();
    assert_eq!(
        w.outcome(),
        Outcome::Won(0),
        "потерявшая всё сторона не проиграла"
    );
    assert_eq!(w.outcome().winner(), Some(0));
    assert!(w.decided_at.is_some(), "тик исхода не записан");
    // Ничья — законный исход, а не отсутствие исхода.
    w.units.voxels[mine as usize] = 0;
    w.tick();
    assert_eq!(w.outcome(), Outcome::Draw);
}

/// **Полный бак останавливает копку.**
///
/// Канон (`docs/07-economy.md`): «правка не применяется, работа
/// копателя стоит и прогресс не набирается». Наивная альтернатива —
/// воксель снят, материя не принята — была бы стоком вне перечня.
#[test]
fn a_full_tank_stops_the_digging() {
    let mut w = world();
    let d = w.spawn_kind(30, 30, 0, CRAWLER).expect("краулер");
    w.units.refresh_live();
    // Склада нет: разгрузиться некуда, и бак останется полным.
    w.push_order(Order::Dig {
        unit: d,
        x: 30,
        y: TOP as i32 - 1,
        z: 30,
    });
    let mut full_at = None;
    for t in 0..200u32 {
        w.tick();
        if w.units.stance[d as usize] == sim_core::STANCE_HAUL && full_at.is_none() {
            full_at = Some(t);
        }
    }
    assert!(full_at.is_some(), "бак так и не наполнился");
    let e = w.econ.as_ref().expect("учёт");
    let tank = e.kinds.get(CRAWLER).expect("краулер").tank;
    assert!(
        e.mined <= tank,
        "выкопано {} при баке {tank} — материя снята и не принята",
        e.mined
    );
    assert!(
        e.balanced(),
        "материя не сходится: {}",
        e.ledger.imbalance()
    );
}
