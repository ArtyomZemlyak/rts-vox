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

use crate::{ground, shot, Order, Reference, Tuning, Units, World};
use fx::blob::{Blob, Scan};
use fx::rng::Rng;

/// Подпись файла. Восемь байт, чтобы не спутать с чем угодно другим.
pub const MAGIC: [u8; 8] = *b"RTSVOXSV";

/// Версия раскладки. Меняется **вместе** с любым полем формата.
pub const VERSION: u32 = 1;

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
            LoadError::WorldMismatch => write!(
                f,
                "мир в сохранении не совпал с миром из сида и параметров"
            ),
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
    for &(x, y, z) in &r.diggers {
        b.i32(x);
        b.i32(y);
        b.i32(z);
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
    if buf.len() < MAGIC.len() + 4 || buf[..MAGIC.len()] != MAGIC {
        return Err(LoadError::NotASave);
    }
    let mut s = Scan::new(&buf[MAGIC.len()..]);
    let v = s.u32();
    if v != VERSION {
        return Err(LoadError::Version {
            found: v,
            want: VERSION,
        });
    }

    let seed = s.u64();
    let params = load_params(&mut s);
    let side = s.u32();
    let per_side = s.usize();
    let nd = s.list(12);
    let diggers: Vec<(i32, i32, i32)> = (0..nd).map(|_| (s.i32(), s.i32(), s.i32())).collect();
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
        1 << 16,
        Box::new(worldgen::Terrain { params, seed }),
    );
    let g = ground::Ground::new(vw, &tuning);
    let mut world = World::new(2, 1, tuning);
    world.attach_ground(g);
    world.attach_weapons(weapons);
    load_world(&mut s, &mut world, &tuning)?;

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
}

fn load_world(s: &mut Scan, w: &mut World, t: &Tuning) -> Result<(), LoadError> {
    w.tick = s.u64();
    load_units(s, &mut w.units)?;
    load_shots(s, &mut w.shots);
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
}

fn load_units(s: &mut Scan, u: &mut Units) -> Result<(), LoadError> {
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
    if !s.ok() || u.x.len() != n {
        return Err(LoadError::Truncated);
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
    b.i32s(&sh.blast_r);
    b.u16s(&sh.blast_d);
    b.u16s(&sh.life);
    b.bools(&sh.alive);
    b.u32s(&sh.free);
    b.u32s(&sh.live_ids);
    b.usize(sh.alive_n);
}

fn load_shots(s: &mut Scan, sh: &mut shot::Shots) {
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
    sh.blast_r = s.i32s();
    sh.blast_d = s.u16s();
    sh.life = s.u16s();
    sh.alive = s.bools();
    sh.free = s.u32s();
    sh.live_ids = s.u32s();
    sh.alive_n = s.usize();
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

    /// **Не наш файл — внятный отказ, а не разбор наугад.**
    #[test]
    fn a_foreign_file_is_refused() {
        assert_eq!(load(b"", tune(), guns()).unwrap_err(), LoadError::NotASave);
        assert_eq!(
            load(b"PNG\x0d\x0a\x1a\x0a\x00\x00\x00\x00", tune(), guns()).unwrap_err(),
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
        let e = load(&b.v, tune(), guns()).unwrap_err();
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
    #[test]
    fn a_truncated_save_is_an_error_not_a_panic() {
        let mut r = Reference::new(3, 40, tune(), guns());
        for _ in 0..5 {
            r.tick();
        }
        let full = save(&r);
        // Шаг по срезам, а не каждый байт: файл большой, а проверка
        // здесь на «не паникует», и одного среза на килобайт довольно.
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
