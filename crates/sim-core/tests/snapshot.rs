//! Снимок эталонного боя — **проверкой, а не обещанием** (ADR-229,
//! ADR-662).
//!
//! Главная проверка та же, что была у сохранения прежнего ядра
//! (`tests/save.rs`): срез посреди боя, восстановление и дальнейший ход
//! обязаны дать **тот же хеш** на каждом тике, что непрерывный прогон.
//! Сценарий — эталонный бой полигона, тот же, что у golden-хеша
//! (`tests/golden.rs`): снимок, проверенный на пустом полигоне, ничего
//! не проверяет — та же болезнь, что ADR-116 и ADR-157.
//!
//! Сборка сцены километровой карты дорогая, а восстановление собирает
//! её заново, даже то, которое в конце отвергнет снимок. Поэтому сцена
//! у эталона одна: непрерывный прогон — сам исходный бой (снимок читает
//! состояние и не меняет его), на его снимке проверяются только отказы,
//! которые случаются до сборки сцены. Обрезанный снимок и снимок
//! с хвостом читаются до конца прежде, чем отвергнуться; они проверяются
//! на учебном полигоне, где сборка дешёвая, а декодер тот же.

use modding::explosion::ExplosionConfig;
use modding::range::RangeLayout;
use modding::range_skirmish::WorldBattle;
use sim_core::blast::BlastRange;

/// Тик среза. К нему эталонный бой уже стреляет с обеих сторон, кто-то
/// погиб, слой обрушения уронил рельеф, снаряды и обломки летят.
const CUT: usize = 100;
/// Сколько тиков после среза сверяются потиково.
const AFTER: usize = 100;

fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
}

/// Сценарий собирается из тех же данных, что у игры (`tests/golden.rs`).
fn reference() -> BlastRange {
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let preset = WorldBattle::from_ron(include_str!("../../../data/battle-world.ron")).unwrap();
    BlastRange::world_battle(config(), layout, preset).unwrap()
}

/// **Сценарий обязан нагружать то, что декларирует** (`CLAUDE.md`):
/// в точке среза есть что терять. Сверка хешей на холостом бою
/// прошла бы и у снимка, который ничего не пишет.
fn assert_loaded(sim: &BlastRange) {
    let fired = [true, false].map(|side| {
        (0..sim.units.len())
            .filter(|&id| sim.allied(id) == side)
            .map(|id| sim.battle.actors[id].fired)
            .sum::<u32>()
    });
    assert!(
        fired.iter().all(|&n| n > 0),
        "сторона не стреляла: {fired:?}"
    );
    assert!(
        sim.units.iter().any(|u| !u.status.alive),
        "никто не погиб — боя в точке среза нет"
    );
    assert!(sim.collapsed_cells > 0, "слой обрушения не работал");
    assert!(sim.world.touched_chunks() > 0, "рельеф не правлен");
    assert!(
        !sim.battle.rounds.is_empty(),
        "в точке среза нет летящих снарядов"
    );
    assert!(
        sim.matter.debris.fragments.iter().any(|f| !f.resting),
        "в точке среза ничего не летит"
    );
    assert!(sim.matter.economy.ledger.balanced());
}

/// **Срезал посреди боя, восстановил, продолжил — тот же хеш.**
///
/// Восстановленный бой равен исходному по состоянию, а не только по
/// дальнейшему ходу: запись восстановленного даёт те же байты, занятость
/// рельефа совпадает до воксела, материал — там, где идёт бой.
/// Сравнение потиковое: расхождение, найденное
/// на сотом тике из ста, не говорит, где оно возникло. Снаряды и куски,
/// летящие в момент среза, доживают свой полёт уже после загрузки.
/// Штатные волны могут завершиться в тик удара (ADR-697); сохранение
/// живых волн отдельно проверено instant_blast_tests и явными медленными
/// fixture в range_battle_tests, включая одновременные и типизированные.
/// Копии того же снимка с чужой версией или испорченным заголовком
/// отвергаются ([`refuses_foreign_headers_of`]).
#[test]
fn a_battle_restored_mid_fight_continues_bit_for_bit() {
    let mut sim = reference();
    for _ in 0..CUT {
        sim.step();
    }
    assert_loaded(&sim);
    let bytes = sim.snapshot();
    let mut back = BlastRange::restore(config(), &bytes).expect("снимок не принят");

    assert_eq!(back.tick, sim.tick);
    assert_eq!(back.state_hash(), sim.state_hash());
    // Порядок записи фиксирован: одно состояние — один байт.
    assert!(
        back.snapshot() == bytes,
        "запись восстановленного дала другие байты"
    );
    // Рельеф — не через снимок, а тем путём, которым его читают
    // навигация и обрушение: занятость всей карты целиком, материал —
    // там, где идёт бой. Карта в десять километров, и материал всей
    // её решёткой стоил бы дороже самого боя.
    let (a, b) = (&sim.world, &back.world);
    assert_eq!(b.touched_chunks(), a.touched_chunks());
    assert!(a.bits() == b.bits(), "занятость рельефа разошлась");
    let mut checked = 0u64;
    for u in &sim.units {
        let [ux, uy, uz] = u.at;
        for z in (uz - 64..uz + 64).step_by(2) {
            for y in (uy - 32..uy + 32).step_by(2) {
                for x in (ux - 64..ux + 64).step_by(2) {
                    let [x, y, z] = [x, y, z].map(|v| v.max(0) as u32);
                    assert_eq!(
                        a.material(x, y, z),
                        b.material(x, y, z),
                        "материал разошёлся в ({x}, {y}, {z})"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 100_000, "сверено всего {checked} вокселей");

    for k in 0..AFTER {
        sim.step();
        back.step();
        let (want, got) = (sim.state_hash(), back.state_hash());
        assert_eq!(
            got, want,
            "расхождение на {k}-м тике после восстановления: {got:016x} вместо {want:016x}"
        );
    }
    assert!(back.matter.economy.ledger.balanced());
    refuses_foreign_headers_of(&bytes);
}

/// **Чужой снимок не читается.**
///
/// Версия стоит в первых байтах и читается до разбора тела: снимок
/// версии вне принимаемого окна отвергается, а не толкуется по-новому.
fn refuses_foreign_headers_of(bytes: &[u8]) {
    let version = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    assert!(
        (27..=66).contains(&version),
        "версия {version} в первых байтах"
    );
    for foreign in [0, 26, 67, u32::MAX] {
        let mut b = bytes.to_vec();
        b[..4].copy_from_slice(&foreign.to_le_bytes());
        assert!(
            BlastRange::restore(config(), &b).is_none(),
            "принята версия {foreign}"
        );
    }
    // Флаг настроек оружия — байт, а не логическое «не ноль»:
    // любое значение, кроме нуля и единицы, — порча. Он стоит сразу
    // за версией, флагами сцены и тиком выстрела начиная с версии 59.
    let (inner, flag) = if version == 66 {
        (u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 14)
    } else {
        (version, 10)
    };
    if inner >= 59 {
        let mut b = bytes.to_vec();
        b[flag] = 2;
        assert!(BlastRange::restore(config(), &b).is_none());
    }
    for cut in [0, 4] {
        assert!(
            BlastRange::restore(config(), &bytes[..cut]).is_none(),
            "принят снимок, обрезанный до {cut} байт"
        );
    }
}

/// The flag corruption check has an explicit live-wave fixture: a fast
/// stock detonation need not leave version 59+ state at the battle cut.
#[test]
fn a_live_wave_rejects_a_non_boolean_weapon_configuration_flag() {
    let mut c = config();
    c.wave_work_per_tick = 1;
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.course = None;
    layout.blocks.clear();
    layout.shields.clear();
    layout.units.truncate(1);
    let mut r = BlastRange::with_layout(c, layout).unwrap();
    let c = r.config;
    // The reserved body-energy field makes the weapon flag part of this
    // format. A shot into bare ground can legitimately stay at version 48.
    let u = &r.units[0];
    r.aim = Some(
        u.world_pos([u.body.vol.nx, u.body.vol.ny, u.body.vol.nz].map(|n| usize::from(n) / 2)),
    );
    while r.impact.is_none() {
        r.step();
    }
    assert!(r.explosion.as_ref().is_some_and(|e| e.working()));
    let mut bytes = r.snapshot();
    let version = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    assert!((59..=65).contains(&version), "fixture version {version}");
    assert!(BlastRange::restore(c, &bytes).is_some());
    bytes[10] = 2;
    assert!(BlastRange::restore(c, &bytes).is_none());
}

/// **Обрезанный или удлинённый снимок не читается.**
///
/// Снимок посреди волны отвергается целиком, если ему не хватает
/// хоть одного байта или за его концом что-то есть, а не даёт сцену,
/// похожую на записанную. Цельный снимок того же состояния читается —
/// иначе отказ ничего бы не доказывал.
#[test]
fn a_snapshot_cut_short_or_with_a_tail_is_refused() {
    let mut r = BlastRange::new(config());
    while r.impact.is_none() {
        r.step();
    }
    for _ in 0..5 {
        r.step();
    }
    let bytes = r.snapshot();
    let back = BlastRange::restore(config(), &bytes).expect("цельный снимок не принят");
    assert_eq!(back.state_hash(), r.state_hash());
    for cut in [0, 4, bytes.len() / 2, bytes.len() - 1] {
        assert!(
            BlastRange::restore(config(), &bytes[..cut]).is_none(),
            "принят снимок, обрезанный до {cut} байт из {}",
            bytes.len()
        );
    }
    let mut tail = bytes.clone();
    tail.push(0);
    assert!(
        BlastRange::restore(config(), &tail).is_none(),
        "принят снимок с лишним хвостом"
    );
}
