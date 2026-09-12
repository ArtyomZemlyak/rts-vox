//! Типы юнитов — данные, а не код.
//!
//! `docs/05-units.md` объявляет у юнита поле `kind` — «индекс в таблице
//! типов из данных». Здесь эта таблица разбирается: стоимость, время
//! сборки, ёмкость бака, скорость стройки, вместимость склада, список
//! производимого.
//!
//! **Номер задаётся явно**, как у материалов и у оружия (ADR-028).
//! Позиция в списке не значит ничего: переставить строки можно,
//! поменять номер — значит сломать все сохранения. Столкновение
//! номеров — ошибка загрузки, а не «последний выиграл».
//!
//! **Тайминги в игровых секундах, а не в тиках** (`docs/07-economy.md`).
//! Перевод в тики происходит **один раз здесь**, при загрузке: число
//! баланса не должно молча зависеть от частоты тика.

use crate::ChassisKind;
use serde::Deserialize;

/// Количество материи. То же, что `sim_core::matter::Amount`: тип
//// живёт там, а здесь повторён числом, потому что `modding` от
/// симуляции не зависит.
pub type Amount = u64;

/// Вид ходовой в данных. Дубликат [`ChassisKind`] не нужен: он уже
/// перечисление в коде с числами в файле (ADR-220).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
enum ChassisTag {
    Tracked,
    Walking,
}

impl From<ChassisTag> for ChassisKind {
    fn from(t: ChassisTag) -> ChassisKind {
        match t {
            ChassisTag::Tracked => ChassisKind::Tracked,
            ChassisTag::Walking => ChassisKind::Walking,
        }
    }
}

/// Строка таблицы типов, как она лежит в файле.
#[derive(Clone, Debug, Deserialize)]
struct KindRow {
    id: u16,
    name: String,
    body: String,
    voxels: u16,
    speed: i32,
    chassis: ChassisTag,
    weapon: u16,
    cost: Amount,
    build_secs: u32,
    tank: Amount,
    dig: bool,
    build_rate: Amount,
    store: Amount,
    produces: Vec<u16>,
}

/// Тип юнита, готовый к работе. Тайминги уже в тиках.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindDef {
    pub id: u16,
    pub name: String,
    /// Имя тела из `voxel::roster::all()`. Рендер берёт то, что здесь
    /// названо, а не то, что угадал по номеру.
    pub body: String,
    pub voxels: u16,
    /// Мировых единиц в секунду. Ноль — постройка: она не едет.
    pub speed: i32,
    pub chassis: ChassisKind,
    /// Номер оружия из `data/weapons.ron`; ноль — безоружен.
    pub weapon: u16,
    /// Материи на сборку. Она же остаётся в теле готовой машины:
    /// тело юнита — контейнер (`docs/07-economy.md`).
    pub cost: Amount,
    /// Тиков на сборку заводом. У постройки ноль: её скорость задают
    /// строители.
    pub build_ticks: u32,
    /// Ёмкость бака, единиц материи. Ноль — возить не умеет.
    pub tank: Amount,
    pub dig: bool,
    /// Материи **за тик**, которую юнит вливает в недострой.
    pub build_rate: Amount,
    /// Насколько постройка поднимает потолок запаса стороны (ADR-043).
    pub store: Amount,
    pub produces: Vec<u16>,
}

impl KindDef {
    /// Постройка ли. Постройка — это тип с нулевой скоростью: она
    /// не едет, её ставят. Отдельного флага нет намеренно — два
    /// источника одной величины разошлись бы молча (ADR-115).
    pub fn is_building(&self) -> bool {
        self.speed == 0
    }
}

#[derive(Deserialize)]
struct KindsFile {
    kinds: Vec<KindRow>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum KindError {
    /// Номер занят.
    Clash(u16),
    /// Ноль — «нет типа», и типом быть не может.
    ZeroId,
    /// Числа, при которых тип бессмыслен.
    Nonsense(u16, &'static str),
    /// Постройка обещает делать тип, которого в таблице нет.
    UnknownProduct(u16, u16),
}

impl std::fmt::Display for KindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KindError::Clash(id) => write!(f, "номер типа {id} занят"),
            KindError::ZeroId => write!(f, "номер типа не может быть нулём"),
            KindError::Nonsense(id, why) => write!(f, "тип {id}: {why}"),
            KindError::UnknownProduct(id, p) => {
                write!(f, "тип {id} обещает делать тип {p}, которого нет")
            }
        }
    }
}

/// Реестр типов: плотный массив с поиском по номеру.
///
/// Массив, а не карта: `HashMap` в итерируемом пути запрещён
/// (`CLAUDE.md`), а типов десятки.
#[derive(Default, Clone, Debug)]
pub struct Kinds {
    defs: Vec<KindDef>,
}

impl Kinds {
    pub fn new() -> Kinds {
        Kinds::default()
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn all(&self) -> &[KindDef] {
        &self.defs
    }

    pub fn insert(&mut self, d: KindDef) -> Result<(), KindError> {
        if d.id == 0 {
            return Err(KindError::ZeroId);
        }
        if self.defs.iter().any(|x| x.id == d.id) {
            return Err(KindError::Clash(d.id));
        }
        if d.voxels == 0 {
            return Err(KindError::Nonsense(d.id, "ноль вокселей у ядра — юнит мёртв при рождении"));
        }
        if d.cost == 0 {
            return Err(KindError::Nonsense(d.id, "бесплатный тип ломает экономику"));
        }
        if d.is_building() && d.build_ticks != 0 {
            return Err(KindError::Nonsense(
                d.id,
                "у постройки время сборки задают строители, а не она сама",
            ));
        }
        if !d.is_building() && d.build_ticks == 0 {
            return Err(KindError::Nonsense(d.id, "машина без времени сборки выходит мгновенно"));
        }
        if d.dig && d.tank == 0 {
            return Err(KindError::Nonsense(d.id, "копает, а везти некуда: бак нулевой"));
        }
        self.defs.push(d);
        // Порядок по номеру: обход реестра не должен зависеть от того,
        // в каком порядке грузились паки.
        self.defs.sort_unstable_by_key(|x| x.id);
        Ok(())
    }

    pub fn get(&self, id: u16) -> Option<&KindDef> {
        self.defs.iter().find(|x| x.id == id)
    }

    /// Место в плотном массиве по номеру — для горячего пути.
    pub fn index_of(&self, id: u16) -> Option<usize> {
        self.defs.iter().position(|x| x.id == id)
    }

    pub fn at(&self, i: usize) -> &KindDef {
        &self.defs[i]
    }

    /// Хеш набора типов. Мод, поменявший стоимость, меняет партию,
    /// и это обязано быть видно (ADR-097).
    pub fn hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for (i, d) in self.defs.iter().enumerate() {
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(d.id as u64);
            w = w.wrapping_mul(31).wrapping_add(d.voxels as u64);
            w = w.wrapping_mul(31).wrapping_add(d.speed as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(d.weapon as u64);
            w = w.wrapping_mul(31).wrapping_add(d.cost);
            w = w.wrapping_mul(31).wrapping_add(d.build_ticks as u64);
            w = w.wrapping_mul(31).wrapping_add(d.tank);
            w = w.wrapping_mul(31).wrapping_add(d.build_rate);
            w = w.wrapping_mul(31).wrapping_add(d.store);
            for p in &d.produces {
                w = w.wrapping_mul(31).wrapping_add(*p as u64);
            }
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        h
    }
}

/// Разобрать таблицу типов.
///
/// `ticks_per_second` приходит снаружи: частота тика — свойство
/// симуляции, а `modding` от неё не зависит. Перевод секунд в тики
/// делается **здесь и один раз**, чтобы число баланса не зависело
/// от частоты молча (`docs/07-economy.md`).
pub fn from_ron(s: &str, ticks_per_second: u32) -> Result<Kinds, String> {
    let f: KindsFile = ron::from_str(s).map_err(|e| e.to_string())?;
    let tps = ticks_per_second.max(1);
    let mut k = Kinds::new();
    for r in f.kinds {
        // Скорость стройки — за **секунду** в файле и за **тик**
        // в памяти. Делится с округлением вверх: строитель, у которого
        // вышло ноль за тик, не строил бы вовсе.
        let per_tick = |per_sec: Amount| -> Amount {
            if per_sec == 0 {
                0
            } else {
                (per_sec / tps as Amount).max(1)
            }
        };
        k.insert(KindDef {
            id: r.id,
            name: r.name,
            body: r.body,
            voxels: r.voxels,
            speed: r.speed,
            chassis: r.chassis.into(),
            weapon: r.weapon,
            cost: r.cost,
            build_ticks: r.build_secs.saturating_mul(tps),
            tank: r.tank,
            dig: r.dig,
            build_rate: per_tick(r.build_rate),
            store: r.store,
            produces: r.produces,
        })
        .map_err(|e| e.to_string())?;
    }
    if k.is_empty() {
        return Err("в таблице типов нет ни одного типа".to_string());
    }
    // Обещание «умею делать» проверяется **после** загрузки всех
    // строк: порядок строк в файле ничего не значит, и завод вправе
    // стоять раньше того, что он делает.
    let ids: Vec<u16> = k.all().iter().map(|d| d.id).collect();
    for d in k.all() {
        for p in &d.produces {
            if !ids.contains(p) {
                return Err(KindError::UnknownProduct(d.id, *p).to_string());
            }
        }
    }
    Ok(k)
}

/// Прочитать таблицу типов с диска.
pub fn load(path: &std::path::Path, ticks_per_second: u32) -> Result<Kinds, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    from_ron(&text, ticks_per_second).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TPS: u32 = 30;

    fn shipped() -> Kinds {
        from_ron(include_str!("../../../data/kinds.ron"), TPS).expect("data/kinds.ron")
    }

    /// **Файл в репозитории — тот, по которому идёт игра, и он обязан
    /// разбираться.**
    #[test]
    fn the_shipped_table_loads() {
        let k = shipped();
        assert!(k.len() >= 4, "типов всего {}", k.len());
        assert!(k.get(10).is_some(), "завода нет");
        assert!(k.get(11).is_some(), "склада нет");
    }

    /// **Столкновение номеров — ошибка загрузки.** То же правило, что
    /// у материалов (ADR-028) и у оружия.
    #[test]
    fn a_clashing_id_is_refused() {
        let src = r#"( kinds: [
            ( id: 1, name: "a", body: "танк", voxels: 10, speed: 5, chassis: Tracked, weapon: 0,
              cost: 100, build_secs: 1, tank: 0, dig: false, build_rate: 0, store: 0, produces: [] ),
            ( id: 1, name: "b", body: "танк", voxels: 10, speed: 5, chassis: Tracked, weapon: 0,
              cost: 100, build_secs: 1, tank: 0, dig: false, build_rate: 0, store: 0, produces: [] ),
        ] )"#;
        let e = from_ron(src, TPS).expect_err("столкновение номеров прошло");
        assert!(e.contains("занят"), "{e}");
    }

    /// **Обещание «умею делать» проверяется.** Завод, обещающий тип,
    /// которого нет, молча не производил бы ничего — и выглядело бы
    /// это как сломанная очередь, а не как ошибка данных.
    #[test]
    fn an_unknown_product_is_refused() {
        let src = r#"( kinds: [
            ( id: 10, name: "завод", body: "завод", voxels: 10, speed: 0, chassis: Tracked,
              weapon: 0, cost: 100, build_secs: 0, tank: 0, dig: false, build_rate: 0,
              store: 0, produces: [7] ),
        ] )"#;
        let e = from_ron(src, TPS).expect_err("неизвестный продукт прошёл");
        assert!(e.contains("которого нет"), "{e}");
    }

    /// **Секунды переводятся в тики один раз, при загрузке.**
    /// Число баланса не должно зависеть от частоты тика молча
    /// (`docs/07-economy.md`).
    #[test]
    fn seconds_become_ticks_once_at_load() {
        let k = shipped();
        let tank = k.get(1).expect("танк");
        assert_eq!(tank.build_ticks % TPS, 0, "время сборки не кратно тику");
        assert!(tank.build_ticks >= TPS, "танк собирается меньше секунды");
        let doubled = from_ron(include_str!("../../../data/kinds.ron"), TPS * 2).expect("разбор");
        assert_eq!(
            doubled.get(1).expect("танк").build_ticks,
            tank.build_ticks * 2,
            "при вдвое частом тике сборка обязана занимать вдвое больше тиков"
        );
    }

    /// **Постройка — это нулевая скорость, а не отдельный флаг.**
    /// Два источника одной величины разошлись бы молча (ADR-115).
    #[test]
    fn a_building_is_a_kind_that_does_not_move() {
        let k = shipped();
        assert!(k.get(10).expect("завод").is_building());
        assert!(!k.get(1).expect("танк").is_building());
    }

    /// **Копатель без бака отвергается.** Он снимал бы воксели
    /// и терял материю — то есть был бы стоком вне перечня
    /// (`docs/07-economy.md`).
    #[test]
    fn a_digger_without_a_tank_is_refused() {
        let src = r#"( kinds: [
            ( id: 2, name: "краулер", body: "танк", voxels: 10, speed: 5, chassis: Tracked,
              weapon: 0, cost: 100, build_secs: 1, tank: 0, dig: true, build_rate: 0,
              store: 0, produces: [] ),
        ] )"#;
        let e = from_ron(src, TPS).expect_err("копатель без бака прошёл");
        assert!(e.contains("бак"), "{e}");
    }

    /// **Хеш видит смену стоимости.** Мод, поменявший цену танка,
    /// меняет партию, и молчать об этом нельзя (ADR-097).
    #[test]
    fn the_hash_sees_a_changed_cost() {
        let a = shipped();
        let src = include_str!("../../../data/kinds.ron").replace("cost: 400000", "cost: 400001");
        let b = from_ron(&src, TPS).expect("разбор");
        assert_ne!(a.hash(), b.hash(), "смена стоимости не сдвинула хеш");
    }
}
