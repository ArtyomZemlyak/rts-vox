//! Сохранение партии: **чистая функция в буфер и обратно** (ADR-229).
//!
//! Файлов здесь нет и быть не может: `sim-core` не знает про диск
//! (`CLAUDE.md`, границы слоёв). Открывает файл тот, кто запускает, —
//! `sim-cli`.
//!
//! # Что пишется, а что нет
//!
//! Мир двухслойный (`docs/04-world.md`). Процедурный слой — чистая
//! функция от сида, и это проверено повоксельно
//! (`worldgen::tests::column_matches_per_voxel_material`), поэтому
//! в файл он не идёт вовсе: идут **сид и параметры генератора**.
//! Идёт **слой правок** — то, что выкопали и взорвали.
//!
//! Производные слои, которые восстанавливаются из мира, тоже
//! не пишутся: связность и проходимость собираются заново
//! конструкторами. Что восстанавливается, а что нет — **замерено
//! зондом**, а не угадано: пересобранные `TopoLayer` и `NavLayer`
//! дают тот же хеш, пересобранные каскад и поле направлений — нет.
//! Оба поэтому записываются целиком, вместе с незаконченной работой:
//! место в цикле — это отставание в тиках, а отставание одинаково
//! у всех клиентов или не одинаково вовсе (ADR-089).
//!
//! # Формат
//!
//! Двоичный, от младшего байта, с явными размерами полей
//! (`fx::blob`). Не `serde` поверх живых структур: раскладка тогда
//! задаётся тем, как сегодня объявлены поля, и правка структуры молча
//! меняет формат файла.
//!
//! ```text
//! 0  8  "RTSVOXSV"
//! 8  4  версия, u32
//! 12 …  тело
//! ```
//!
//! **Версия растёт при любом изменении раскладки.** Незнакомую версию
//! читатель отвергает с внятным сообщением, а не разбирает наугад.
//! Версия 10 читает версии 8 и 9, перенося старую скорость/дробную
//! высоту полёта подвижного корпуса в состояние подвески (ADR-253).

use crate::{ground, shot, Order, Reference, Tuning, Units, World};
use fx::blob::{Blob, Scan};
use fx::rng::Rng;

/// Подпись файла. Восемь байт, чтобы не спутать с чем угодно другим.
pub const MAGIC: [u8; 8] = *b"RTSVOXSV";

/// Версия раскладки. Меняется **вместе** с любым полем формата.
pub const VERSION: u32 = 10;

/// Почему файл не прочитан.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// Не наш файл.
    NotASave,
    /// Наш, но другой версии.
    Version { found: u32, want: u32 },
    /// Оборвался или испорчен.
    Truncated,
    /// Мир в файле не тот, что собрался из сида и параметров.
    WorldMismatch,
    /// Номер вида ходовой, которого в коде нет.
    UnknownChassis(u8),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotASave => write!(f, "это не сохранение rts-vox: подпись файла не та"),
            LoadError::Version { found, want } => write!(
                f,
                "сохранение версии {found}, а читать умеем {want}: \
                 формат менялся, старый файл не подходит"
            ),
            LoadError::Truncated => write!(f, "сохранение обрезано или испорчено"),
            LoadError::WorldMismatch => {
                write!(f, "мир в сохранении не совпал с миром из сида и параметров")
            }
            LoadError::UnknownChassis(c) => {
                write!(f, "в сохранении вид ходовой {c}, которого в коде нет")
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Записать партию в буфер.
///
/// Снимается **между тиками**: все фазы отработали, следующая начнётся
/// с `ApplyCommands`. В другой точке тика состояние половинчато,
/// и продолжить его нельзя.
pub fn save(r: &Reference) -> Vec<u8> {
    let mut b = Blob::new();
    b.v.extend_from_slice(&MAGIC);
    b.u32(VERSION);

    // Сценарий: сид, параметры генератора, копатели, генератор
    // случайных чисел.
    b.u64(r.seed);
    save_params(&mut b, &r.terrain.params);
    b.u32(r.side);
    b.usize(r.per_side);
    b.u32(r.diggers.len() as u32);
    for d in &r.diggers {
        // Свежесть столба — состояние копателя, а не украшение:
        // следующий тик по ней решает, ставить ли в очередь шар
        // целиком (ADR-227). Потеряй её сохранение — загруженная
        // партия разошлась бы с продолженной на первом же тике.
        b.i32(d.x);
        b.i32(d.y);
        b.i32(d.z);
        b.u32(u32::from(d.fresh));
    }
    let (st, inc) = r.rng.state();
    b.u64(st);
    b.u64(inc);

    save_world(&mut b, &r.world);
    b.v
}

/// Прочитать партию из буфера.
///
/// Числа тика и оружие приходят **снаружи**, из `data/*.ron`: это
/// не состояние партии, а правила, по которым она идёт (ADR-140).
/// Сохранять их значило бы заморозить баланс в файле сохранения.
pub fn load(
    buf: &[u8],
    tuning: Tuning,
    weapons: modding::weapons::Weapons,
) -> Result<Reference, LoadError> {
    load_with(buf, tuning, weapons, None)
}

/// То же, но с таблицей типов и материалами — для партии
/// с экономикой (ADR-230).
///
/// Таблица приходит **снаружи**, как числа тика и оружие: это
/// правила, а не состояние. Партия с экономикой, открытая без
/// таблицы, читаться не будет — `WorldMismatch`, а не тихая партия
/// без построек.
pub fn load_with(
    buf: &[u8],
    tuning: Tuning,
    weapons: modding::weapons::Weapons,
    econ: Option<(modding::Kinds, modding::Registry)>,
) -> Result<Reference, LoadError> {
    if buf.len() < MAGIC.len() + 4 || buf[..MAGIC.len()] != MAGIC {
        return Err(LoadError::NotASave);
    }
    let mut s = Scan::new(&buf[MAGIC.len()..]);
    let v = s.u32();
    if v != VERSION && v != 8 && v != 9 {
        return Err(LoadError::Version {
            found: v,
            want: VERSION,
        });
    }

    let seed = s.u64();
    let params = load_params(&mut s);
    let side = s.u32();
    let per_side = s.usize();
    let nd = s.list(16);
    let diggers: Vec<crate::Digger> = (0..nd)
        .map(|_| crate::Digger {
            x: s.i32(),
            y: s.i32(),
            z: s.i32(),
            fresh: s.u32() != 0,
        })
        .collect();
    let rng = Rng::from_state(s.u64(), s.u64());
    if !s.ok() {
        return Err(LoadError::Truncated);
    }

    // Мир собирается **из сида**, как при первом запуске, и только
    // потом на него ложится слой правок. Это и есть смысл формата:
    // процедурный слой не хранится.
    let terrain = worldgen::Terrain { params, seed };
    let vw = voxel::World::new(
        params.nx,
        params.ny,
        params.nz,
        tuning.max_voxel_edits_per_tick as usize,
        Box::new(worldgen::Terrain { params, seed }),
    );
    let g = ground::Ground::new(vw, &tuning);
    // Ёмкость индекса соседей — единица: сколько юнитов в файле, ещё
    // неизвестно, а индекс всё равно пересобирается каждый тик и растёт
    // сам. Это предвыделение, а не правило.
    let mut world = World::new(2, 1, tuning);
    world.attach_ground(g);
    world.attach_weapons(weapons);
    if let Some((kinds, mats)) = econ {
        world.attach_economy(kinds, mats);
    }
    load_world(&mut s, &mut world, &tuning, v)?;

    Ok(Reference {
        world,
        side,
        per_side,
        terrain,
        diggers,
        rng,
        seed,
    })
}

/// Параметры генератора — девятнадцать чисел, каждое своим полем.
///
/// Перечислены руками и в фиксированном порядке: набор полей
/// у `Params` растёт, и файл обязан ломаться на версии, а не
/// разъезжаться молча.
fn save_params(b: &mut Blob, p: &worldgen::Params) {
    b.u32(p.nx);
    b.u32(p.ny);
    b.u32(p.nz);
    b.u32(p.base_height);
    b.u32(p.relief);
    b.u32(p.feature);
    b.u32(p.detail);
    b.u32(p.soil);
    b.u32(p.lava_depth);
    b.u32(p.deep_at);
    b.u32(p.vein_threshold);
    b.u32(p.terrace);
    b.u32(p.terrace_lo);
    b.u32(p.terrace_hi);
    b.u32(p.bed);
    b.u32(p.plains);
    b.u32(p.plains_period);
    b.u32(p.plains_floor);
}

fn load_params(s: &mut Scan) -> worldgen::Params {
    worldgen::Params {
        nx: s.u32(),
        ny: s.u32(),
        nz: s.u32(),
        base_height: s.u32(),
        relief: s.u32(),
        feature: s.u32(),
        detail: s.u32(),
        soil: s.u32(),
        lava_depth: s.u32(),
        deep_at: s.u32(),
        vein_threshold: s.u32(),
        terrace: s.u32(),
        terrace_lo: s.u32(),
        terrace_hi: s.u32(),
        bed: s.u32(),
        plains: s.u32(),
        plains_period: s.u32(),
        plains_floor: s.u32(),
    }
}

fn save_world(b: &mut Blob, w: &World) {
    b.u64(w.tick);
    save_units(b, &w.units);
    save_shots(b, &w.shots);
    match &w.ground {
        Some(g) => {
            b.bool(true);
            g.save(b);
        }
        None => b.bool(false),
    }
    // Поле направлений своей стороны: точка сбора плюс само поле.
    // Плоская заливка по точке не пишется — она восстанавливается
    // той же `fill_flow_towards`, которой была получена.
    for t in w.flow_target.iter() {
        match *t {
            Some((x, z)) => {
                b.bool(true);
                b.i32(x);
                b.i32(z);
            }
            None => b.bool(false),
        }
    }
    w.nav_field.save(b);
    // Очередь приказов: единственный вход снаружи, и она переживает
    // тик по построению — упёршийся в потолок приказ откладывается
    // (ADR-195).
    b.u32(w.orders.len() as u32);
    for o in w.orders.iter() {
        save_order(b, *o);
    }
    // Счётчики прогона. В хеш не входят, но это отчёт о партии:
    // загруженная партия, у которой смертей стало ноль, — не та же
    // партия.
    for v in [
        w.examined_move,
        w.examined_combat,
        w.blocked_moves,
        w.flow_updates,
        w.flow_solves,
        w.beams_fired,
        w.beams_hit,
        w.beams_blocked,
        w.shots_fired,
        w.shots_hit,
        w.shots_blocked,
        w.shots_refused,
        w.deaths,
        w.blasts,
        w.blast_voxels,
        w.blast_hits,
        w.searches,
        w.search_capped,
        w.orders_applied,
        w.orders_dropped,
        w.orders_capped,
    ] {
        b.u64(v);
    }
    save_econ(b, w.econ.as_ref());
    b.bool(w.material_bodies.is_some());
    if let Some(bodies) = &w.material_bodies {
        bodies.save(b);
    }
    w.bursts.save(b);
}

/// Учёт партии: контейнеры, склады, очереди заводов, обломки,
/// недострои и счётчики замера (ADR-229 поверх ADR-230).
///
/// Таблица типов и материалы **не пишутся**: они приходят из
/// `data/*.ron` тем же путём, что числа тика и оружие. Партия,
/// открытая с другим паком, — это другая партия, и подменить её
/// файлом сохранения нельзя.
fn save_econ(b: &mut Blob, e: Option<&crate::economy::Economy>) {
    let Some(e) = e else {
        b.bool(false);
        return;
    };
    b.bool(true);
    let (cs, sunk, initial, free_head, free) = e.ledger.snapshot();
    b.u32(cs.len() as u32);
    for c in &cs {
        b.u8(c.kind.code());
        b.u64(c.held);
        b.u64(c.cap);
        b.bool(c.alive);
    }
    b.u64s(&sunk);
    b.u64(initial);
    b.u32(free_head);
    b.u32s(&free);
    b.u32(e.ground);
    let (banks, lines, wrecks, sites) = e.parts();
    b.u32(banks.len() as u32);
    for k in banks {
        b.u32(k.stock);
        b.u64(k.cap);
    }
    b.u32(lines.len() as u32);
    for (unit, queue, held, ticks) in &lines {
        b.u32(*unit);
        b.u16s(queue);
        b.u32(*held);
        b.u32(*ticks);
    }
    b.u32(wrecks.len() as u32);
    for w in wrecks {
        b.u32(w.held);
        b.i32(w.x);
        b.i32(w.y);
        b.i32(w.z);
    }
    b.u32s(sites);
    for v in e.counters() {
        b.u64(v);
    }
}

/// Учёт из файла на **уже заведённую** экономику: таблица типов
/// и материалы в ней от данных, всё остальное — из файла.
fn load_econ(s: &mut Scan, w: &mut World) -> Result<(), LoadError> {
    if !s.bool() {
        w.econ = None;
        return Ok(());
    }
    let Some(e) = w.econ.as_mut() else {
        // Файл с экономикой, открытый без таблицы типов: типов нет,
        // значит и построек не будет. Молча дать пустую партию нельзя.
        return Err(LoadError::WorldMismatch);
    };
    let nc = s.list(18);
    let mut cs = Vec::with_capacity(nc.min(1 << 20));
    for _ in 0..nc {
        let kind = crate::matter::Kind::from_code(s.u8()).ok_or(LoadError::WorldMismatch)?;
        cs.push(crate::matter::Container {
            kind,
            held: s.u64(),
            cap: s.u64(),
            alive: s.bool(),
        });
    }
    let sunk = s.u64s();
    let initial = s.u64();
    let free_head = s.u32();
    let free = s.u32s();
    if !s.ok() {
        return Err(LoadError::Truncated);
    }
    // Учёт принимается только сошедшийся: начальная сумма обязана
    // равняться сумме в контейнерах плюс сумме по стокам. Иначе
    // партия продолжилась бы с материей, взявшейся ниоткуда.
    let ledger = crate::matter::Ledger::from_snapshot(cs, sunk, initial, free_head, free)
        .ok_or(LoadError::WorldMismatch)?;
    e.ledger = ledger;
    e.ground = s.u32();
    let nb = s.list(12);
    let banks = (0..nb)
        .map(|_| crate::economy::Bank {
            stock: s.u32(),
            cap: s.u64(),
        })
        .collect();
    let nl = s.list(13);
    let lines = (0..nl)
        .map(|_| (s.u32(), s.u16s(), s.u32(), s.u32()))
        .collect();
    let nw = s.list(16);
    let wrecks = (0..nw)
        .map(|_| crate::economy::Wreck {
            held: s.u32(),
            x: s.i32(),
            y: s.i32(),
            z: s.i32(),
        })
        .collect();
    let sites = s.u32s();
    if !s.ok() {
        return Err(LoadError::Truncated);
    }
    e.set_parts(banks, lines, wrecks, sites);
    let mut c = [0u64; 6];
    for v in c.iter_mut() {
        *v = s.u64();
    }
    e.set_counters(c);
    if !s.ok() {
        return Err(LoadError::Truncated);
    }
    Ok(())
}

fn load_world(s: &mut Scan, w: &mut World, t: &Tuning, version: u32) -> Result<(), LoadError> {
    w.tick = s.u64();
    load_units(s, &mut w.units, version)?;
    load_shots(s, &mut w.shots)?;
    if s.bool() {
        let Some(g) = w.ground.as_mut() else {
            return Err(LoadError::WorldMismatch);
        };
        if !g.load(s) {
            return Err(LoadError::WorldMismatch);
        }
        // Связность и проходимость — чистые функции мира, поэтому
        // собираются заново по уже восстановленному миру, а не
        // читаются из файла.
        g.topo = voxel::topo::layer::TopoLayer::new(&g.world);
        g.nav = nav::layer::NavLayer::new(&g.world, t.unit_height, t.nav_cell, t.max_step);
    } else {
        w.ground = None;
    }
    for o in 0..2usize {
        if s.bool() {
            let (x, z) = (s.i32(), s.i32());
            w.fill_flow_towards(o, x, z);
        }
    }
    if !w.nav_field.load(s) {
        return Err(LoadError::Truncated);
    }
    let n = s.list(5);
    w.orders.clear();
    for _ in 0..n {
        w.orders.push_back(load_order(s));
    }
    let c = [
        &mut w.examined_move,
        &mut w.examined_combat,
        &mut w.blocked_moves,
        &mut w.flow_updates,
        &mut w.flow_solves,
        &mut w.beams_fired,
        &mut w.beams_hit,
        &mut w.beams_blocked,
        &mut w.shots_fired,
        &mut w.shots_hit,
        &mut w.shots_blocked,
        &mut w.shots_refused,
        &mut w.deaths,
        &mut w.blasts,
        &mut w.blast_voxels,
        &mut w.blast_hits,
        &mut w.searches,
        &mut w.search_capped,
        &mut w.orders_applied,
        &mut w.orders_dropped,
        &mut w.orders_capped,
    ];
    for p in c {
        *p = s.u64();
    }
    if !s.ok() {
        return Err(LoadError::Truncated);
    }
    load_econ(s, w)?;
    if s.bool() {
        let Some(bodies) = w.material_bodies.as_mut() else {
            return Err(LoadError::WorldMismatch);
        };
        if !bodies.load(s, w.units.len(), t.impact.max_pending) {
            return Err(LoadError::WorldMismatch);
        }
    } else {
        w.material_bodies = None;
    }
    w.bursts = crate::burst::Bursts::load_material(
        s,
        w.ground.as_ref().map(|g| &g.world),
        &t.blast,
        w.material_bodies.as_ref(),
        w.units.len(),
    )
    .ok_or(LoadError::WorldMismatch)?;
    if !w.bursts.fragments.is_empty() || !w.bursts.corpses.is_empty() {
        let Some(e) = &w.econ else {
            return Err(LoadError::WorldMismatch);
        };
        let wrecks: std::collections::BTreeSet<_> = e.wrecks.iter().map(|w| w.held).collect();
        for f in w.bursts.fragments.iter().chain(w.bursts.corpses.values()) {
            let Some(c) = e.ledger.try_get(f.held) else {
                return Err(LoadError::WorldMismatch);
            };
            let corpse = f.chunk.as_ref().is_some_and(|c| c.body.is_some());
            if !c.alive
                || c.kind != crate::matter::Kind::Debris
                || ((f.resting || corpse) && !wrecks.contains(&f.held))
                || (corpse && c.held == 0)
            {
                return Err(LoadError::WorldMismatch);
            }
        }
    }
    if !s.ok() {
        return Err(LoadError::Truncated);
    }
    Ok(())
}

/// Приказ — видом и полями, а не номером варианта в перечислении.
fn save_order(b: &mut Blob, o: Order) {
    match o {
        Order::MoveTo { unit, x, z } => {
            b.u8(0);
            b.u32(unit);
            b.i32(x);
            b.i32(z);
        }
        Order::AttackMove { unit, x, z } => {
            b.u8(8);
            b.u32(unit);
            b.i32(x);
            b.i32(z);
        }
        Order::Hold { unit } => {
            b.u8(1);
            b.u32(unit);
        }
        Order::Release { unit } => {
            b.u8(2);
            b.u32(unit);
        }
        Order::Attack { unit, target } => {
            b.u8(3);
            b.u32(unit);
            b.u32(target);
        }
        Order::Dig { unit, x, y, z } => {
            b.u8(4);
            b.u32(unit);
            b.i32(x);
            b.i32(y);
            b.i32(z);
        }
        // Приказы экономики (ADR-230). Тип пишется **номером из
        // данных**, каким он и приехал: место в реестре зависит
        // от того, какие типы загружены, и сохранённая партия,
        // прочитанная с другим паком, приказала бы строить другое.
        Order::Build { unit, kind, x, z } => {
            b.u8(5);
            b.u32(unit);
            b.u32(u32::from(kind));
            b.i32(x);
            b.i32(z);
        }
        Order::Produce { unit, kind } => {
            b.u8(6);
            b.u32(unit);
            b.u32(u32::from(kind));
        }
        Order::Cancel { unit } => {
            b.u8(7);
            b.u32(unit);
        }
    }
}

/// Неизвестный вид приказа — не паника: он читается как `Hold`
/// у несуществующего юнита и отбрасывается фазой. Испорченный файл
/// ловится проверкой целостности, а не разбором каждого поля.
fn load_order(s: &mut Scan) -> Order {
    match s.u8() {
        0 => Order::MoveTo {
            unit: s.u32(),
            x: s.i32(),
            z: s.i32(),
        },
        1 => Order::Hold { unit: s.u32() },
        2 => Order::Release { unit: s.u32() },
        3 => Order::Attack {
            unit: s.u32(),
            target: s.u32(),
        },
        4 => Order::Dig {
            unit: s.u32(),
            x: s.i32(),
            y: s.i32(),
            z: s.i32(),
        },
        5 => Order::Build {
            unit: s.u32(),
            kind: s.u32() as u16,
            x: s.i32(),
            z: s.i32(),
        },
        6 => Order::Produce {
            unit: s.u32(),
            kind: s.u32() as u16,
        },
        7 => Order::Cancel { unit: s.u32() },
        8 => Order::AttackMove {
            unit: s.u32(),
            x: s.i32(),
            z: s.i32(),
        },
        _ => Order::Hold { unit: u32::MAX },
    }
}

/// Юниты, SoA — каждый массив своим полем.
///
/// Свободный список пишется, а не выводится из `alive`: он
/// упорядочивается только в `refresh_live`, а сохранение снимается
/// между тиками, когда порядок уже мог измениться выдачей мест.
fn save_units(b: &mut Blob, u: &Units) {
    b.u32(u.len() as u32);
    b.i32s(&u.x);
    b.i32s(&u.y);
    b.i32s(&u.z);
    b.fxs(&u.fx);
    b.fxs(&u.fz);
    b.u8s(&u.owner);
    b.i32s(&u.speed);
    b.i32s(&u.head);
    b.fxs(&u.vel);
    b.u8s(&u.chassis.iter().map(|c| c.code()).collect::<Vec<u8>>());
    save_v3s(b, &u.up);
    save_v3s(b, &u.spin);
    b.fxs(&u.load);
    b.u16s(&u.voxels);
    b.bools(&u.alive);
    b.u32s(&u.target);
    b.u64s(&u.riding);
    b.u32(u.ride_off.len() as u32);
    for &(x, y, z) in &u.ride_off {
        b.i32(x);
        b.i32(y);
        b.i32(z);
    }
    b.i32s(&u.fall_from);
    b.fxs(&u.fall_v);
    b.fxs(&u.fall_f);
    b.u32s(&u.next_search);
    b.u32s(&u.next_shot);
    b.u16s(&u.weapon);
    b.u8s(&u.stance);
    b.i32s(&u.goal_x);
    b.i32s(&u.goal_y);
    b.i32s(&u.goal_z);
    b.u32s(&u.free);
    b.u32s(&u.live_ids);
    // Приращения подвески (ADR-225). Пересчитываются каждый тик
    // в фазе хода — но не у всякого юнита: едущий на чужом куске
    // её пропускает, и стало бы у него после загрузки не то, что
    // было до неё.
    b.fxs(&u.accel);
    b.i32s(&u.yaw_step);
    // Угловая скорость и фаза шага — состояние (ADR-234), и в файл
    // идут наравне с курсом: без них загруженная машина начинала бы
    // доворот заново, а шагоход — с ноги, которой не поднимал.
    b.i32s(&u.yaw_rate);
    b.i32s(&u.gait);
    b.fxs(&u.heave);
    b.fxs(&u.heave_v);
    // Экономика (ADR-230). Пишется всегда: «поля ничего не значат
    // без типа» — это про хеш, а не про сохранение, и партия
    // без экономики отдаёт здесь нули.
    b.u16s(&u.kind);
    b.u32s(&u.store);
    b.u64s(&u.site);
    b.u32(u.face.len() as u32);
    for &(x, y, z) in &u.face {
        b.i32(x);
        b.i32(y);
        b.i32(z);
    }
    b.u32s(&u.job);
}

fn load_units(s: &mut Scan, u: &mut Units, version: u32) -> Result<(), LoadError> {
    let n = s.u32() as usize;
    u.x = s.i32s();
    u.y = s.i32s();
    u.z = s.i32s();
    u.fx = s.fxs();
    u.fz = s.fxs();
    u.owner = s.u8s();
    u.speed = s.i32s();
    u.head = s.i32s();
    u.vel = s.fxs();
    let codes = s.u8s();
    u.chassis = Vec::with_capacity(codes.len());
    for c in codes {
        match crate::ChassisKind::from_code(c) {
            Some(k) => u.chassis.push(k),
            None => return Err(LoadError::UnknownChassis(c)),
        }
    }
    u.up = load_v3s(s);
    u.spin = load_v3s(s);
    u.load = s.fxs();
    u.voxels = s.u16s();
    u.alive = s.bools();
    u.target = s.u32s();
    u.riding = s.u64s();
    let m = s.list(12);
    u.ride_off = (0..m).map(|_| (s.i32(), s.i32(), s.i32())).collect();
    u.fall_from = s.i32s();
    u.fall_v = s.fxs();
    u.fall_f = s.fxs();
    u.next_search = s.u32s();
    u.next_shot = s.u32s();
    u.weapon = s.u16s();
    u.stance = s.u8s();
    u.goal_x = s.i32s();
    u.goal_y = s.i32s();
    u.goal_z = s.i32s();
    u.free = s.u32s();
    u.live_ids = s.u32s();
    u.accel = s.fxs();
    u.yaw_step = s.i32s();
    u.yaw_rate = s.i32s();
    u.gait = s.i32s();
    u.heave = if version >= 9 {
        s.fxs()
    } else {
        vec![fx::Fx::ZERO; n]
    };
    u.heave_v = if version >= 9 {
        s.fxs()
    } else {
        vec![fx::Fx::ZERO; n]
    };
    u.kind = s.u16s();
    u.store = s.u32s();
    u.site = s.u64s();
    let nf = s.list(12);
    u.face = (0..nf).map(|_| (s.i32(), s.i32(), s.i32())).collect();
    u.job = s.u32s();
    if !s.ok()
        || u.x.len() != n
        || u.heave.len() != n
        || u.heave_v.len() != n
        || u.speed.len() != n
        || u.fall_from.len() != n
        || u.fall_f.len() != n
        || u.fall_v.len() != n
    {
        return Err(LoadError::Truncated);
    }
    if version < 10 {
        for i in 0..n {
            if u.speed[i] > 0 && u.fall_from[i] != crate::NOT_FALLING {
                u.heave[i] = u.heave[i].sub(u.fall_f[i]);
                u.heave_v[i] = u.fall_v[i]
                    .neg()
                    .mul(fx::Fx::from_int(crate::TICKS_PER_SECOND));
                u.fall_f[i] = fx::Fx::ZERO;
            }
        }
    }
    Ok(())
}

fn save_shots(b: &mut Blob, sh: &shot::Shots) {
    b.i32s(&sh.x);
    b.i32s(&sh.y);
    b.i32s(&sh.z);
    b.fxs(&sh.fx);
    b.fxs(&sh.fy);
    b.fxs(&sh.fz);
    b.fxs(&sh.vx);
    b.fxs(&sh.vy);
    b.fxs(&sh.vz);
    b.u8s(&sh.owner);
    b.u16s(&sh.damage);
    b.u16s(&sh.weapon_id);
    b.u32s(&sh.source);
    b.u32s(&sh.source_generation);
    b.i32s(&sh.blast_r);
    b.u16s(&sh.blast_d);
    b.u16s(&sh.life);
    b.bools(&sh.alive);
    b.u32s(&sh.free);
    b.u32s(&sh.live_ids);
    b.usize(sh.alive_n);
}

fn load_shots(s: &mut Scan, sh: &mut shot::Shots) -> Result<(), LoadError> {
    sh.x = s.i32s();
    sh.y = s.i32s();
    sh.z = s.i32s();
    sh.fx = s.fxs();
    sh.fy = s.fxs();
    sh.fz = s.fxs();
    sh.vx = s.fxs();
    sh.vy = s.fxs();
    sh.vz = s.fxs();
    sh.owner = s.u8s();
    sh.damage = s.u16s();
    sh.weapon_id = s.u16s();
    sh.source = s.u32s();
    sh.source_generation = s.u32s();
    sh.blast_r = s.i32s();
    sh.blast_d = s.u16s();
    sh.life = s.u16s();
    sh.alive = s.bools();
    sh.free = s.u32s();
    sh.live_ids = s.u32s();
    sh.alive_n = s.usize();
    let n = sh.x.len();
    if !s.ok()
        || [
            sh.y.len(),
            sh.z.len(),
            sh.fx.len(),
            sh.fy.len(),
            sh.fz.len(),
            sh.vx.len(),
            sh.vy.len(),
            sh.vz.len(),
            sh.owner.len(),
            sh.damage.len(),
            sh.weapon_id.len(),
            sh.source.len(),
            sh.source_generation.len(),
            sh.blast_r.len(),
            sh.blast_d.len(),
            sh.life.len(),
            sh.alive.len(),
        ]
        .iter()
        .any(|v| *v != n)
        || sh.live_ids.iter().chain(&sh.free).any(|i| *i as usize >= n)
        || sh.live_ids.windows(2).any(|p| p[0] >= p[1])
        || sh.alive_n != sh.alive.iter().filter(|v| **v).count()
    {
        return Err(LoadError::Truncated);
    }
    Ok(())
}

/// Вектор — тремя сырыми `i64`. Иначе и нельзя: `Fx` это `i64`,
/// а плавающего в симуляции нет (`CLAUDE.md`).
fn save_v3s(b: &mut Blob, v: &[phys::V3]) {
    b.u32(v.len() as u32);
    for p in v {
        b.fx(p.x);
        b.fx(p.y);
        b.fx(p.z);
    }
}

fn load_v3s(s: &mut Scan) -> Vec<phys::V3> {
    let n = s.list(24);
    (0..n)
        .map(|_| phys::V3 {
            x: s.fx(),
            y: s.fx(),
            z: s.fx(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ошибка чтения — или паника теста. `Reference` не `Debug`
    /// и быть им не обязан: это состояние партии, а не отчёт.
    fn err(buf: &[u8]) -> LoadError {
        match load(buf, tune(), guns()) {
            Err(e) => e,
            Ok(_) => panic!("прочиталось то, что читаться не должно"),
        }
    }

    /// **Не наш файл — внятный отказ, а не разбор наугад.**
    #[test]
    fn a_foreign_file_is_refused() {
        assert_eq!(err(b""), LoadError::NotASave);
        assert_eq!(
            err(b"PNG\x0d\x0a\x1a\x0a\x00\x00\x00\x00"),
            LoadError::NotASave
        );
    }

    /// **Незнакомая версия отвергается, а не читается как своя.**
    ///
    /// Это то самое правило, ради которого версия стоит в первых
    /// байтах: формат сохранений — то, что нельзя менять молча.
    #[test]
    fn an_unknown_version_is_refused_by_name() {
        let mut b = Blob::new();
        b.v.extend_from_slice(&MAGIC);
        b.u32(VERSION + 7);
        let e = err(&b.v);
        assert_eq!(
            e,
            LoadError::Version {
                found: VERSION + 7,
                want: VERSION
            }
        );
        assert!(
            format!("{e}").contains("формат менялся"),
            "сообщение обязано говорить, что произошло: {e}"
        );
    }

    /// **Обрезанное сохранение не паникует.** Читают его с диска,
    /// а диск врёт.
    /// Мир здесь **маленький нарочно**: проверяется разбор буфера,
    /// а не генератор, а каждая попытка чтения строит мир из сида
    /// заново. На эталонных 256×128×256 шестьдесят четыре попытки
    /// стоили бы под две минуты — и мерили бы worldgen.
    #[test]
    fn a_truncated_save_is_an_error_not_a_panic() {
        let small = worldgen::Params::sized(64, 64, 64);
        let mut r = Reference::with_params(3, 40, tune(), guns(), small);
        for _ in 0..5 {
            r.tick();
        }
        let full = save(&r);
        // Шаг по срезам, а не каждый байт: проверка здесь
        // на «не паникует», и шестидесяти четырёх срезов довольно.
        let step = (full.len() / 64).max(1);
        for cut in (MAGIC.len() + 4..full.len()).step_by(step) {
            assert!(
                load(&full[..cut], tune(), guns()).is_err(),
                "обрезка на {cut} байте прошла незамеченной"
            );
        }
    }

    fn tune() -> Tuning {
        Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("data/sim.ron")
    }

    fn guns() -> modding::weapons::Weapons {
        modding::weapons::from_ron(include_str!("../../../data/weapons.ron"))
            .expect("data/weapons.ron")
    }
}
