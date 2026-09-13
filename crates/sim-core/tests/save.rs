//! Сохранение партии — **проверкой, а не обещанием** (ADR-229).
//!
//! Одна проверка здесь главная: прогон `N` тиков, сохранение, загрузка
//! и ещё `M` тиков обязаны дать **тот же хеш**, что непрерывный прогон
//! `N + M` тиков. Всё остальное в файле — подпорки к ней.
//!
//! Сценарий тот же, что у golden-хеша: эталонный, с землёй, копкой,
//! боем и подкреплениями. Сохранение, проверенное на пустом мире,
//! ничего не проверяет — ровно та же болезнь, что ADR-116 и ADR-157.

use sim_core::save::{load, load_with, save, LoadError};
use sim_core::{Order, Reference};

const SEED: u64 = 42;
const UNITS: usize = 1200;

fn tuning() -> sim_core::Tuning {
    sim_core::Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("data/sim.ron")
}

fn weapons() -> modding::weapons::Weapons {
    modding::weapons::from_ron(include_str!("../../../data/weapons.ron")).expect("data/weapons.ron")
}

fn reference() -> Reference {
    Reference::new(SEED, UNITS, tuning(), weapons())
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
const FACTORY: u16 = 10;
const TANK: u16 = 1;

/// Эталонный сценарий **с экономикой**: завод у каждой стороны,
/// заказ на танк и приказ строить склад рядом.
///
/// Экономика не входит в эталонный прогон (ADR-230), и сценарий
/// сохранения без неё проверял бы формат, которого не касается
/// половина полей, — та же болезнь, что ADR-116.
fn with_economy() -> Reference {
    let mut r = Reference::new(SEED, UNITS / 4, tuning(), weapons());
    r.world.attach_economy(kinds(), mats());
    let (ex, ez) = r.world.ground().expect("земля").extent();
    for (owner, dx) in [(0u8, 1), (1u8, 3)] {
        let (x, z) = (ex * dx / 4, ez / 2);
        let Some(f) = r.world.spawn_kind(x, z, owner, FACTORY) else {
            panic!("завод стороны {owner} не встал");
        };
        r.world.push_order(Order::Produce { unit: f, kind: TANK });
        r.world.push_order(Order::Produce { unit: f, kind: TANK });
    }
    r
}

/// **Сохранил, загрузил, продолжил — тот же хеш.**
///
/// Ради этого всё и написано. Сравнение потиковое, а не только
/// по итогу: расхождение, найденное на сотом тике из ста, ничего
/// не говорит о том, где оно возникло.
#[test]
fn a_loaded_game_continues_bit_for_bit() {
    const N: usize = 300;
    const M: usize = 200;

    let mut straight = reference();
    for _ in 0..N {
        straight.tick();
    }
    let want: Vec<u64> = (0..M).map(|_| straight.tick()).collect();

    let mut cut = reference();
    for _ in 0..N {
        cut.tick();
    }
    let blob = save(&cut);
    let mut back = load(&blob, tuning(), weapons()).expect("сохранение не прочиталось");

    for (k, w) in want.iter().enumerate() {
        let got = back.tick();
        assert_eq!(
            got, *w,
            "расхождение на {k}-м тике после загрузки: {got:016x} вместо {w:016x}"
        );
    }
}

/// **Партия с экономикой продолжается тем же битом.**
///
/// Баки, склады, очередь завода и недострои — состояние, и потеряй
/// их файл, загруженная партия разошлась бы не сразу, а на первом
/// достроенном заказе. Потому сравнение потиковое и после загрузки,
/// а не по итогу.
#[test]
fn a_loaded_game_with_economy_continues_bit_for_bit() {
    const N: usize = 120;
    const M: usize = 80;

    let mut straight = with_economy();
    for _ in 0..N {
        straight.tick();
    }
    let want: Vec<u64> = (0..M).map(|_| straight.tick()).collect();

    let mut cut = with_economy();
    for _ in 0..N {
        cut.tick();
    }
    // Сценарий обязан нагружать то, что декларирует: к точке сохранения
    // экономика должна что-то сделать, иначе проверять нечего.
    {
        let e = cut.world.econ.as_ref().expect("экономика");
        assert!(
            e.ledger.live_containers() > 2,
            "к точке сохранения контейнеров всего {}",
            e.ledger.live_containers()
        );
        let (_, lines, _, _) = e.parts();
        assert!(!lines.is_empty(), "к точке сохранения заводы ничего не делают");
    }
    let blob = save(&cut);
    let mut back = load_with(&blob, tuning(), weapons(), Some((kinds(), mats())))
        .expect("сохранение не прочиталось");

    for (k, w) in want.iter().enumerate() {
        let got = back.tick();
        assert_eq!(
            got, *w,
            "расхождение на {k}-м тике после загрузки: {got:016x} вместо {w:016x}"
        );
    }
}

/// **Партия с экономикой не читается без таблицы типов.**
///
/// Типы приходят из данных, и открыть такую партию без них значит
/// молча получить партию без построек — то есть другую.
#[test]
fn a_game_with_economy_needs_its_kinds() {
    let mut r = with_economy();
    for _ in 0..30 {
        r.tick();
    }
    let blob = save(&r);
    assert!(
        matches!(load(&blob, tuning(), weapons()), Err(LoadError::WorldMismatch)),
        "партия с экономикой прочиталась без таблицы типов"
    );
}

/// **Сценарий обязан нагружать то, что декларирует** (`CLAUDE.md`).
///
/// Проверка выше пройдёт и на мире, в котором за триста тиков ничего
/// не произошло: нечему расходиться — нечему и совпадать. Здесь
/// проверяется, что к точке сохранения мир уже правлен, тела падали,
/// снаряды летали и кто-то погиб.
#[test]
fn the_save_point_is_not_an_empty_world() {
    let mut r = reference();
    for _ in 0..300 {
        r.tick();
    }
    let g = r.world.ground().expect("у эталона нет земли");
    assert!(
        g.world.touched_chunks() > 0,
        "слой правок пуст: сохранять нечего"
    );
    assert!(r.world.deaths > 0, "никто не погиб — боя в точке среза нет");
    assert!(
        r.world.shots_fired > 0,
        "не стреляли — снарядов в состоянии нет"
    );
    assert!(
        r.world.flow_solves > 0,
        "поле направлений не опубликовано — сохранять нечего"
    );
}

/// **Загруженный мир равен исходному по состоянию, а не только
/// по дальнейшему ходу.**
///
/// Хеш тика видит юнитов, снаряды и поток правок — но не содержимое
/// мира (это замечено ещё в ADR-217). Поэтому мир сверяется отдельно:
/// материал по разреженной решётке обязан совпасть до воксела.
#[test]
fn the_loaded_world_has_the_same_voxels() {
    let mut r = reference();
    for _ in 0..200 {
        r.tick();
    }
    let back = load(&save(&r), tuning(), weapons()).expect("сохранение не прочиталось");
    let a = r.world.ground().expect("земля").world.nx;
    let ga = &r.world.ground().expect("земля").world;
    let gb = &back.world.ground().expect("земля").world;
    assert_eq!(gb.touched_chunks(), ga.touched_chunks(), "тронуто не столько же");
    let mut same = 0u64;
    for z in (0..ga.nz).step_by(3) {
        for y in (0..ga.ny).step_by(3) {
            for x in (0..a).step_by(3) {
                assert_eq!(
                    ga.material(x, y, z),
                    gb.material(x, y, z),
                    "материал разошёлся в ({x}, {y}, {z})"
                );
                assert_eq!(ga.solid(x, y, z), gb.solid(x, y, z), "занятость ({x}, {y}, {z})");
                same += 1;
            }
        }
    }
    assert!(same > 100_000, "сверено всего {same} вокселей — решётка редка");
}

/// **Сохранение сохранённого — тот же файл.**
///
/// Порядок обхода при записи фиксирован (`CLAUDE.md`), значит запись
/// одного и того же состояния обязана давать один и тот же байт.
/// Разойдись — и сравнивать сохранения между клиентами было бы нечем.
#[test]
fn saving_a_loaded_game_gives_the_same_bytes() {
    let mut r = reference();
    for _ in 0..150 {
        r.tick();
    }
    let first = save(&r);
    let back = load(&first, tuning(), weapons()).expect("сохранение не прочиталось");
    let second = save(&back);
    assert_eq!(first.len(), second.len(), "длина файла разошлась");
    assert!(first == second, "запись загруженного дала другие байты");
}

/// **Числа тика в сохранении не заморожены.**
///
/// Потолки и тяготение — правила, по которым идёт партия, а не её
/// состояние: они живут в `data/*.ron` (ADR-140). Запиши их в файл —
/// и правка `sim.ron` перестала бы действовать на загруженную партию.
/// Проверяется прямо: загрузка с изменёнными числами обязана взять
/// изменённые.
#[test]
fn the_save_does_not_freeze_the_tuning() {
    let small = worldgen::Params::sized(64, 64, 64);
    let mut r = Reference::with_params(SEED, 200, tuning(), weapons(), small);
    for _ in 0..20 {
        r.tick();
    }
    let blob = save(&r);
    let mut t = tuning();
    t.max_bodies += 7;
    t.max_materialize_per_tick += 1;
    let back = load(&blob, t, weapons()).expect("сохранение не прочиталось");
    let g = back.world.ground().expect("земля");
    assert_eq!(
        g.bodies.max_bodies, t.max_bodies,
        "потолок тел взят из файла, а не из data/sim.ron"
    );
    assert_eq!(
        g.world.max_materialize_per_tick, t.max_materialize_per_tick as usize,
        "потолок материализаций взят из файла, а не из data/sim.ron"
    );
    assert_eq!(
        g.bodies.gravity,
        sim_core::gravity_per_tick(&t),
        "тяготение взято из файла, а не из data/sim.ron"
    );
}

/// Версия стоит в первых байтах и читается **до** разбора тела.
#[test]
fn the_version_is_in_the_first_bytes() {
    let r = reference();
    let mut blob = save(&r);
    assert_eq!(&blob[..8], b"RTSVOXSV");
    assert_eq!(u32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]), 1);
    blob[8] = 99;
    assert!(matches!(
        load(&blob, tuning(), weapons()),
        Err(LoadError::Version { found: 99, want: 1 })
    ));
}
