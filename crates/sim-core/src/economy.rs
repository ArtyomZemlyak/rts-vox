//! Экономика стороны: запас, стройка, производство.
//!
//! **Это то, что замыкает петлю.** До неё копка снимала воксели
//! и материя никуда не шла: `matter.rs` умел переводить между
//! контейнерами, но контейнеров в живой партии не было ни одного,
//! а сцена D-5 гоняла учёт отдельно от мира (ADR-230).
//!
//! Устройство целиком следует `docs/07-economy.md`:
//!
//! - **Материя всегда в контейнере.** Бак краулера, запас стороны,
//!   недострой, тело готовой машины, обломок — всё это контейнеры
//!   [`Ledger`]. Начисление — перевод, а не создание.
//! - **Земля тоже контейнер.** Мир из вокселей ленив, и пересчитать
//!   материю всей карты вперёд — это обойти миллионы вокселей на
//!   старте партии. Поэтому содержимое земли учёт **узнаёт** по мере
//!   копки: [`Ledger::reveal`].
//! - **Перечень стоков закрыт.** Здесь срабатывают четыре из шести:
//!   выход переработки меньше единицы (склад), распыление воронкой
//!   (взрыв), потери при разрушении контейнера (гибель машины),
//!   излишек над упавшим потолком (разрушен склад).
//!
//! **Ни одного балансного числа здесь нет.** Стоимость, время, ёмкость
//! и доли — в `data/kinds.ron` и `data/sim.ron`.

use crate::matter::{resource_of, Amount, ContainerId, Kind, Ledger, Sink};
use modding::kinds::Kinds;
use modding::{Registry, Tuning};

/// Тип не задан: юнит пришёл не из таблицы типов. Так живут стенды
/// и эталонный сценарий — им экономика не нужна, и платить за неё
/// они не обязаны.
pub const NO_KIND: u16 = u16::MAX;

/// Контейнера нет. У большинства юнитов его и не бывает: возить
/// и хранить умеют немногие типы.
pub const NO_CONTAINER: ContainerId = ContainerId::MAX;

/// Запас одной стороны.
///
/// Один контейнер на сторону, а не по контейнеру на склад: канон
/// (ADR-043) говорит «склады поднимают **потолок хранимой материи
/// игрока**», то есть запас общий, а склад — вместимость.
#[derive(Clone, Copy, Debug)]
pub struct Bank {
    pub stock: ContainerId,
    /// Потолок на прошлый пересчёт — чтобы отчёт мог его показать,
    /// не заглядывая в учёт.
    pub cap: Amount,
}

/// Очередь одного завода.
///
/// Живёт отдельным списком, а не полем юнита: очередь это `VecDeque`,
/// и держать её у каждого из пяти тысяч слотов значило бы пять тысяч
/// пустых очередей.
#[derive(Clone, Debug)]
pub struct Line {
    /// Слот завода. Список держится **отсортированным по нему**:
    /// ничьи разрешаются линейным индексом, а не порядком постановки
    /// (`CLAUDE.md`).
    pub unit: u32,
    /// Что делает — места в реестре типов, FIFO.
    pub queue: std::collections::VecDeque<u16>,
    /// Заготовка: контейнер с уже вложенной материей.
    ///
    /// Контейнер, а не число: материя ни в один момент не существует
    /// вне контейнера (`docs/07-economy.md`), и отмена заказа — это
    /// перевод обратно в запас, а не прибавка к счётчику.
    pub held: ContainerId,
    /// Тиков идёт передний заказ.
    pub ticks: u32,
}

/// Обломок: контейнер с материей и место, где он лежит.
///
/// Реклейм — **та же операция, что копка** (ADR-017): приказ копать
/// в точку обломка переливает его содержимое в бак. Никакого
/// отдельного механизма.
#[derive(Clone, Copy, Debug)]
pub struct Wreck {
    pub held: ContainerId,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Учёт партии целиком.
pub struct Economy {
    /// Таблица типов из `data/kinds.ron`.
    pub kinds: Kinds,
    /// Материалы: плотность решает, сколько материи в вокселе.
    pub mats: Registry,
    pub ledger: Ledger,
    /// Земля. Содержимое узнаётся по мере копки.
    pub ground: ContainerId,
    pub banks: Vec<Bank>,
    pub(crate) lines: Vec<Line>,
    pub wrecks: Vec<Wreck>,
    /// Слоты **недостроенных** построек, по возрастанию номера.
    ///
    /// Отдельный список, а не обход всех юнитов: приказ «строить»
    /// ищет, к чему пристроиться, и обходить ради этого пять тысяч
    /// слотов на каждый приказ — это потолок приказов, умноженный
    /// на число слотов, то есть способ занять тик приказами (ADR-114).
    pub(crate) sites: Vec<u32>,

    // --- замер. Ни одно из этих чисел не влияет на симуляцию.
    /// Выкопано из земли всего.
    pub mined: Amount,
    /// Доставлено на склады (после переработки).
    pub delivered: Amount,
    /// Потрачено на стройку и производство.
    pub spent: Amount,
    /// Реклеймлено из обломков.
    pub reclaimed: Amount,
    /// Построек достроено и машин выпущено.
    pub built: u64,
    pub produced: u64,
    /// Поисков склада сделано и в скольких тиках поиск упёрся в потолок.
    pub dock_searches: u64,
    pub dock_capped: u64,
}

impl Economy {
    /// Завести учёт. Земля заводится **пустой**: её содержимое войдёт
    /// в сумму по мере того, как его выкопают.
    pub fn new(kinds: Kinds, mats: Registry, owners: usize, t: &Tuning) -> Economy {
        let mut ledger = Ledger::new();
        let ground = ledger.create(Kind::Ground, 0, Amount::MAX);
        let banks = (0..owners.max(1))
            .map(|_| {
                let stock = ledger.create(Kind::Depot, 0, t.base_store);
                Bank { stock, cap: t.base_store }
            })
            .collect();
        Economy {
            kinds,
            mats,
            ledger,
            ground,
            banks,
            lines: Vec::new(),
            wrecks: Vec::new(),
            sites: Vec::new(),
            mined: 0,
            delivered: 0,
            spent: 0,
            reclaimed: 0,
            built: 0,
            produced: 0,
            dock_searches: 0,
            dock_capped: 0,
        }
    }

    /// Запас стороны.
    pub fn stock(&self, owner: u8) -> Amount {
        match self.banks.get(owner as usize) {
            Some(b) => self.ledger.held(b.stock),
            None => 0,
        }
    }

    /// Потолок запаса стороны.
    pub fn cap(&self, owner: u8) -> Amount {
        self.banks.get(owner as usize).map(|b| b.cap).unwrap_or(0)
    }

    /// Сколько материи в вокселе этого материала. Ресурс = объём ×
    /// плотность (`CLAUDE.md`); объём одного воксела — единица.
    pub fn per_voxel(&self, m: modding::Mat) -> Amount {
        match self.mats.get(m) {
            Some(d) => resource_of(1, d.density),
            // Материала нет в реестре — значит он ничего не стоит.
            // Молча считать его дорогим нельзя: это создавало бы
            // материю из дыры в данных.
            None => 0,
        }
    }

    /// Недостроенные постройки, по возрастанию номера слота.
    pub fn sites(&self) -> &[u32] {
        &self.sites
    }

    /// Записать недострой. Порядок — по слоту: ничьи разрешаются
    /// линейным индексом, а не порядком постановки (`CLAUDE.md`).
    pub(crate) fn add_site(&mut self, unit: u32) {
        if let Err(i) = self.sites.binary_search(&unit) {
            self.sites.insert(i, unit);
        }
    }

    /// Снять недострой: достроен или погиб.
    pub(crate) fn drop_site(&mut self, unit: u32) {
        if let Ok(i) = self.sites.binary_search(&unit) {
            self.sites.remove(i);
        }
    }

    /// Очередь завода, если она заведена.
    pub fn line(&self, unit: u32) -> Option<&Line> {
        self.lines
            .binary_search_by_key(&unit, |l| l.unit)
            .ok()
            .map(|i| &self.lines[i])
    }

    /// Очередь завода; заводится при первом заказе.
    ///
    /// Список держится отсортированным по слоту: обход заводов идёт
    /// по возрастанию номера, а не по порядку заказов, иначе два
    /// завода, тянущие из одного запаса, зависели бы от истории
    /// (`CLAUDE.md`, ничьи по линейному индексу).
    pub fn line_mut(&mut self, unit: u32) -> &mut Line {
        match self.lines.binary_search_by_key(&unit, |l| l.unit) {
            Ok(i) => &mut self.lines[i],
            Err(i) => {
                let held = self.ledger.create(Kind::Body, 0, Amount::MAX);
                self.lines.insert(
                    i,
                    Line { unit, queue: std::collections::VecDeque::new(), held, ticks: 0 },
                );
                &mut self.lines[i]
            }
        }
    }

    /// Уплачено за передний заказ завода.
    pub fn line_paid(&self, unit: u32) -> Amount {
        self.line(unit).map(|l| self.ledger.held(l.held)).unwrap_or(0)
    }

    /// Снять очередь завода, вернув заготовку в запас.
    ///
    /// Зовётся при отмене последнего заказа. Гибель завода идёт другим
    /// путём — [`Economy::wreck_line`]: там заготовка не возвращается,
    /// а гибнет вместе с ним.
    pub fn drop_line(&mut self, unit: u32, stock: ContainerId) {
        let Ok(i) = self.lines.binary_search_by_key(&unit, |l| l.unit) else { return };
        let held = self.lines[i].held;
        self.ledger.transfer(held, stock, self.ledger.held(held));
        // Остаток, не влезший в запас, — излишек над потолком:
        // объявленный сток, а не тихая пропажа.
        let rest = self.ledger.held(held);
        if rest > 0 {
            self.ledger.sink(held, rest, Sink::OverCap);
        }
        self.lines.remove(i);
    }

    /// Завод погиб: заготовка разделяет его судьбу.
    ///
    /// Доля теряется по данным, остаток лёг бы обломком — но обломок
    /// у завода уже есть, свой. Заготовка **уходит в него**, поэтому
    /// здесь только потеря доли и перевод.
    pub fn wreck_line(&mut self, unit: u32, into: ContainerId) {
        let Ok(i) = self.lines.binary_search_by_key(&unit, |l| l.unit) else { return };
        let held = self.lines[i].held;
        self.ledger.transfer(held, into, self.ledger.held(held));
        let rest = self.ledger.held(held);
        if rest > 0 {
            self.ledger.sink(held, rest, Sink::ContainerBreach);
        }
        self.lines.remove(i);
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// Хеш экономики. **Всё, что переживает тик, входит в хеш**
    /// (`CLAUDE.md`): заведи запас стороны или очередь производства
    /// и не внеси — их можно переписать целиком и увидеть тот же хеш
    /// до бита (ADR-116).
    ///
    /// Земля в хеш входит своим содержимым: оно растёт ровно на то,
    /// что выкопано, и расхождение в копке видно сразу.
    pub fn hash(&self) -> u64 {
        let mut h = 0u64;
        for (i, b) in self.banks.iter().enumerate() {
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(self.ledger.held(b.stock));
            w = w.wrapping_mul(31).wrapping_add(b.cap);
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        for (i, l) in self.lines.iter().enumerate() {
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(l.unit as u64);
            w = w.wrapping_mul(31).wrapping_add(self.ledger.held(l.held));
            w = w.wrapping_mul(31).wrapping_add(l.ticks as u64);
            for (k, q) in l.queue.iter().enumerate() {
                w = w.wrapping_mul(31).wrapping_add((*q as u64).wrapping_add(k as u64));
            }
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        for (i, wr) in self.wrecks.iter().enumerate() {
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(self.ledger.held(wr.held));
            w = w.wrapping_mul(31).wrapping_add(wr.x as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(wr.y as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(wr.z as u32 as u64);
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        h = h.wrapping_add(self.ledger.held(self.ground).wrapping_mul(0x9E37_79B9));
        h = h.wrapping_add(self.ledger.sunk_total().wrapping_mul(0x85EB_CA6B));
        h
    }

    /// Отчёт: сошлась ли материя. Тесты и сцены зовут это, тик — нет.
    pub fn balanced(&self) -> bool {
        self.ledger.balanced()
    }

    pub fn sunk(&self, s: Sink) -> Amount {
        self.ledger.sunk(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> Kinds {
        modding::kinds::from_ron(include_str!("../../../data/kinds.ron"), 30).expect("kinds")
    }

    fn mats() -> Registry {
        modding::data::materials_from_ron(include_str!("../../../data/materials.ron"))
            .expect("data/materials.ron")
    }

    fn econ() -> Economy {
        Economy::new(kinds(), mats(), 2, &crate::test_tuning())
    }

    /// **Земля отдаёт ровно столько, сколько из неё выкопали.**
    /// Перевод в бак не может создать материю: сумма в контейнерах
    /// плюс стоки равна тому, что вошло в экономику.
    #[test]
    fn revealing_the_ground_keeps_the_books() {
        let mut e = econ();
        assert!(e.balanced(), "пустой учёт не сходится");
        let tank = e.ledger.create(Kind::Hauler, 0, 1000);
        e.ledger.reveal(e.ground, 600);
        assert_eq!(e.ledger.transfer(e.ground, tank, 600), 600);
        assert!(e.balanced(), "расхождение {}", e.ledger.imbalance());
        assert_eq!(e.ledger.initial(), 600, "в экономику вошло не то, что выкопали");
    }

    /// **Очереди заводов лежат по возрастанию слота.** Два завода,
    /// тянущие из одного запаса, не имеют права зависеть от того,
    /// в каком порядке им отдали приказы.
    #[test]
    fn lines_are_kept_in_slot_order() {
        let mut e = econ();
        for u in [7u32, 2, 9, 4] {
            e.line_mut(u).queue.push_back(0);
        }
        let got: Vec<u32> = e.lines().iter().map(|l| l.unit).collect();
        assert_eq!(got, vec![2, 4, 7, 9]);
        let stock = e.banks[0].stock;
        e.drop_line(4, stock);
        let got: Vec<u32> = e.lines().iter().map(|l| l.unit).collect();
        assert_eq!(got, vec![2, 7, 9]);
    }

    /// **Плотность материала решает выход.** Богатая руда — материал
    /// с большей плотностью, а не модификатор (ADR-100).
    #[test]
    fn richer_ore_is_worth_more_per_voxel() {
        let e = econ();
        let poor = e.per_voxel(16);
        let rich = e.per_voxel(18);
        assert!(poor > 0 && rich > poor, "бедное {poor}, богатое {rich}");
        assert_eq!(e.per_voxel(0), 0, "воздух чего-то стоит");
    }

    /// **Хеш видит запас.** Не войди он — запас стороны можно было бы
    /// переписать целиком и увидеть тот же хеш до бита (ADR-116).
    #[test]
    fn the_hash_sees_the_stock() {
        let mut a = econ();
        let b = econ();
        assert_eq!(a.hash(), b.hash(), "два одинаковых учёта дали разный хеш");
        a.ledger.reveal(a.ground, 100);
        let stock = a.banks[0].stock;
        a.ledger.transfer(a.ground, stock, 100);
        assert_ne!(a.hash(), b.hash(), "запас не вошёл в хеш");
    }
}
