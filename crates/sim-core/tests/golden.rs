//! Golden-хеш эталонного сценария — **тестом, а не глазами**.
//!
//! `docs/03-determinism.md` объявляет golden-хеш центральной проверкой
//! проекта: «изменение любого из перечисленного ломает golden-файлы».
//! Эталонный сценарий — бой на полигоне (ADR-662): тот же прогон,
//! что печатает `sim-cli --ticks 600 --dump-hash`, и то же число,
//! что записано в `docs/11-roadmap.md`.
//!
//! **Числа здесь и в каноне обязаны совпадать.** Если этот тест упал,
//! а изменение симуляции было намеренным, — новый хеш записывается
//! и сюда, и в `docs/11-roadmap.md`, одним коммитом. Если изменение
//! не было намеренным, тест только что нашёл десинк.
//!
//! Хеш — свёртка снимка состояния целиком, рельеф полигона входит
//! в него вместе с юнитами, обломками и волнами: отдельного слепка
//! мира, как у прежнего ядра (ADR-217), не нужно.

use modding::explosion::ExplosionConfig;
use modding::range::RangeLayout;
use modding::range_skirmish::WorldBattle;
use sim_core::blast::battle::Weapon;
use sim_core::blast::BlastRange;

/// Хеш шестисотого тика эталонного боя.
// ADR-688: непринятые контактные срезы растений сохраняются после ухода
// корпуса и продолжаются при появлении места. Windows 1/4/8 — analysis/254.
// ADR-691: начала детальных поисков на месте ограничены бюджетом тика
// (сверх него поиск ставится без работы), плотная сетка дальнего поиска оплачивается,
// корпус без коридора старта ведётся к узлу-входу. Снято на Linux
// (1/4/8 потоков физики одинаково), analysis/261.
// ADR-690 поверх ADR-691: грунт/дёрн/дорога требуют вчетверо больше
// fracture-энергии. Баланс намеренно меняет бой; точный кэш цены сам
// по себе состояния не меняет. Свежий Windows CLI, analysis/262.
// ADR-697: квоты быстрых взрывов намеренно меняют межтиковый темп
// разрушения. Windows 1/4/8 — analysis/268, формат снимка прежний.
const GOLDEN_600: u64 = 0x5131_bc7b_e4f8_74fa;

/// Сценарий собирается **из тех же данных, что у игры**: `include_str!`
/// разворачивается на сборке, ввода-вывода в тесте нет, а литералы
/// здесь были бы вторым источником правды.
fn reference() -> BlastRange {
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let preset = WorldBattle::from_ron(include_str!("../../../data/battle-world.ron")).unwrap();
    BlastRange::world_battle(config, layout, preset).unwrap()
}

/// **Эталонный бой даёт записанный хеш**, и прогон нагружает то, что
/// хеш объявляет покрытым (ADR-116): обе армии стреляют, юниты гибнут,
/// слой обрушения роняет рельеф, взрывные попадания есть, обломки летят. Хеш над
/// холостой фазой не закреплял бы ничего.
#[test]
fn the_reference_battle_hashes_what_was_recorded() {
    let mut sim = reference();
    let mut blast = false;
    for _ in 0..600 {
        sim.step();
        blast |= sim.battle.impacts.iter().any(|i| {
            matches!(
                i.weapon,
                Weapon::Blast | Weapon::Canister | Weapon::Energy | Weapon::Annihilation
            )
        });
    }
    let fired = [true, false].map(|side| {
        (0..sim.units.len())
            .filter(|&id| sim.allied(id) == side)
            .map(|id| sim.battle.actors[id].fired)
            .sum::<u32>()
    });
    let dead = sim.units.iter().filter(|u| !u.status.alive).count();
    assert!(
        fired.iter().all(|&n| n > 0),
        "сторона не стреляла: {fired:?}"
    );
    assert!(dead > 0, "за 600 тиков боя никто не погиб");
    assert!(sim.collapsed_cells > 0, "слой обрушения не работал");
    assert!(blast, "ни одного взрывного попадания");
    assert!(!sim.matter.debris.fragments.is_empty(), "нет обломков");
    assert!(sim.matter.economy.ledger.balanced());
    // Маршруты боя строятся продолжением по тикам (ADR-680): работа
    // дальнего поиска входит в прогон, а не обходит его.
    assert!(sim.coarse_work > 0, "дальний поиск не работал");
    let hash = sim.state_hash();
    assert_eq!(
        hash, GOLDEN_600,
        "хеш эталонного боя изменился: {hash:016x} вместо {GOLDEN_600:016x}.\n\
         Если симуляция менялась намеренно — записать новое число сюда \
         и в docs/11-roadmap.md одним коммитом.\n\
         Если нет — это десинк, и искать его надо сейчас."
    );
}
