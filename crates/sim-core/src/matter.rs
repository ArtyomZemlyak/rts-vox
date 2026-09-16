//! Материя: контейнеры, переводы и закрытый перечень стоков.
//!
//! **Главное правило** (`docs/07-economy.md`): материя ни в один момент
//! не существует вне контейнера. Любое начисление — **перевод между
//! двумя контейнерами**, а не создание. Земля тоже контейнер.
//!
//! Без этого правила инвариант сохранения проверяет только то, что стоки
//! перечислены, а источники остаются кодом, которому тест верит на слово.
//! С ним инвариант **структурный**: перевод не может создать материю,
//! потому что симметричен по построению. Здесь это выражено типами —
//! [`Ledger::transfer`] физически не умеет менять сумму.
//!
//! **Перечень стоков закрыт** (ADR-046). Новый способ уменьшить
//! количество материи требует строки в таблице `07-economy.md`,
//! а не нового метода здесь.

/// Количество материи. Целое: материя сохраняется, а деление
/// с остатком в плавающем типе сохранение и ломает.
pub type Amount = u64;

/// Идентификатор контейнера.
pub type ContainerId = u32;

/// Закрытый перечень стоков. Ровно таблица из `docs/07-economy.md`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Sink {
    /// Выход переработки меньше единицы: руда в строительный материал.
    RefiningLoss = 0,
    /// Оружие, явно помеченное как уничтожающее материю.
    Annihilation = 1,
    /// Осознанная конверсия своей материи в энергию (ADR-022).
    ToEnergy = 2,
    /// Доля содержимого, теряемая при разрушении контейнера (ADR-021).
    ContainerBreach = 3,
    /// Излишек над упавшим потолком игрока: разрушен склад (ADR-043).
    OverCap = 4,
    /// Порода, снятая воронкой от попадания (ADR-165).
    ///
    /// Обломком она не становится намеренно: воронок в эталонном
    /// прогоне 362 749 вокселей за тысячу тиков, а живых тел не больше
    /// `max_bodies` = 256. Бой уменьшает запасы карты — это свойство
    /// игры, а не недосмотр (ADR-185).
    Pulverised = 5,
    /// Горящий воксель топлива выгорел (ADR-276): тепло ушло соседям,
    /// материя — сюда.
    Combustion = 6,
}

pub const SINKS: [Sink; 7] = [
    Sink::RefiningLoss,
    Sink::Annihilation,
    Sink::ToEnergy,
    Sink::ContainerBreach,
    Sink::OverCap,
    Sink::Pulverised,
    Sink::Combustion,
];

impl Sink {
    pub fn name(self) -> &'static str {
        match self {
            Sink::RefiningLoss => "выход переработки < 1",
            Sink::Annihilation => "аннигиляция",
            Sink::ToEnergy => "конверсия материи в энергию",
            Sink::ContainerBreach => "потери при разрушении контейнера",
            Sink::OverCap => "излишек над упавшим потолком игрока",
            Sink::Pulverised => "распыление воронкой",
            Sink::Combustion => "сгорание",
        }
    }
}

/// Что за контейнер. Влияет только на читаемость отчётов: правила
/// перевода одинаковы для всех.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Земля. Из неё копают; она же принимает возврат породы.
    Ground,
    /// Бак краулера.
    Hauler,
    /// Обломок — то, во что превращается разрушенный контейнер.
    Debris,
    /// Склад с потолком.
    Depot,
    /// Тело юнита.
    Body,
}

impl Kind {
    /// Номер вида контейнера в файле сохранения. Задаётся **явно**,
    /// а не порядком в перечислении: порядок переставят, и старые
    /// файлы прочитаются как другие контейнеры (ADR-028, ADR-229).
    pub fn code(self) -> u8 {
        match self {
            Kind::Ground => 0,
            Kind::Hauler => 1,
            Kind::Debris => 2,
            Kind::Depot => 3,
            Kind::Body => 4,
        }
    }

    /// Вид по номеру. Неизвестный — не паника: испорченный файл
    /// ловится проверкой целостности, а не разбором каждого поля.
    pub fn from_code(c: u8) -> Option<Kind> {
        match c {
            0 => Some(Kind::Ground),
            1 => Some(Kind::Hauler),
            2 => Some(Kind::Debris),
            3 => Some(Kind::Depot),
            4 => Some(Kind::Body),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Container {
    pub kind: Kind,
    pub held: Amount,
    /// Потолок. `Amount::MAX` — без потолка (земля).
    pub cap: Amount,
    pub alive: bool,
}

/// Учёт материи: контейнеры плюс счётчики стоков.
///
/// Инвариант, ради которого всё это существует:
/// **начальная сумма == сумма в контейнерах + сумма по стокам**.
pub struct Ledger {
    containers: Vec<Container>,
    /// Списано по каждому стоку. Длина — из самого перечня, а не
    /// числом: шестой сток ронял этот массив по выходу за границу,
    /// и ловилось это не сборкой, а паникой в проперти-тесте (ADR-185).
    sunk: [Amount; SINKS.len()],
    initial: Amount,
    free_head: u32,
    free: Vec<u32>,
    /// Derived membership cache; the serialized linked list is authoritative.
    free_slots: Vec<bool>,
}

const NIL: u32 = u32::MAX;

impl Default for Ledger {
    fn default() -> Ledger {
        Ledger::new()
    }
}

impl Ledger {
    pub fn new() -> Ledger {
        Ledger {
            containers: Vec::new(),
            sunk: [0; SINKS.len()],
            initial: 0,
            free_head: NIL,
            free: Vec::new(),
            free_slots: Vec::new(),
        }
    }

    /// Завести контейнер. **Содержимое при заведении входит в начальную
    /// сумму** — это единственное место, где материя появляется, и оно
    /// нужно, чтобы мир вообще с чего-то начался.
    pub fn create(&mut self, kind: Kind, held: Amount, cap: Amount) -> ContainerId {
        self.initial += held;
        let c = Container {
            kind,
            held,
            cap,
            alive: true,
        };
        if self.free_head != NIL {
            let id = self.free_head;
            self.free_head = self.free[id as usize];
            self.free_slots[id as usize] = false;
            self.containers[id as usize] = c;
            return id;
        }
        self.containers.push(c);
        self.free.push(NIL);
        self.free_slots.push(false);
        (self.containers.len() - 1) as ContainerId
    }

    /// **Земля отдаёт то, что в ней лежало.** Содержимое карты входит
    /// в учёт по мере того, как его выкапывают.
    ///
    /// Мир из вокселей ленив: чанк строится из генератора при первом
    /// касании, и пересчитать материю всей карты вперёд — это обойти
    /// восемь миллионов вокселей на старте партии. Поэтому земля —
    /// контейнер, содержимое которого учёт **узнаёт**, а не получает
    /// разом.
    ///
    /// Начальная сумма растёт ровно на столько же, на сколько
    /// содержимое, поэтому инвариант остаётся точным: сумма в живых
    /// контейнерах плюс стоки равна всему, что когда-либо вошло
    /// в экономику. Создать материю этим нельзя — величина приходит
    /// из объёма снятых вокселей на плотность их материала, то есть
    /// из мира.
    pub fn reveal(&mut self, id: ContainerId, n: Amount) -> Amount {
        if n == 0 || !self.containers[id as usize].alive {
            return 0;
        }
        self.initial += n;
        self.containers[id as usize].held += n;
        n
    }

    /// Освободить слот разрушенного и пустого контейнера.
    ///
    /// Без этого свободный список [`Ledger::create`] не пополняется
    /// никогда, и партия на десять тысяч тиков копит по контейнеру
    /// на каждую погибшую машину.
    ///
    /// **Непустой контейнер не освобождается**: его материя пропала бы
    /// из суммы мимо всех стоков. Это та же дыра, что ловил
    /// property-тест у перевода в мёртвого.
    pub fn release(&mut self, id: ContainerId) -> bool {
        let Some(c) = self.containers.get(id as usize) else {
            return false;
        };
        if c.alive || c.held != 0 || self.free_slots[id as usize] {
            return false;
        }
        self.free_slots[id as usize] = true;
        self.free[id as usize] = self.free_head;
        self.free_head = id;
        true
    }

    pub fn retire_empty_debris(&mut self, id: ContainerId) -> bool {
        let Some(c) = self.containers.get_mut(id as usize) else {
            return false;
        };
        if !c.alive || c.held != 0 || c.kind != Kind::Debris {
            return false;
        }
        c.alive = false;
        self.release(id)
    }
    pub fn try_get(&self, id: ContainerId) -> Option<&Container> {
        self.containers.get(id as usize)
    }

    pub fn get(&self, id: ContainerId) -> &Container {
        &self.containers[id as usize]
    }

    pub fn held(&self, id: ContainerId) -> Amount {
        self.containers[id as usize].held
    }

    pub fn room(&self, id: ContainerId) -> Amount {
        let c = &self.containers[id as usize];
        c.cap.saturating_sub(c.held)
    }

    /// Перевод между контейнерами. **Сумму не меняет по построению:**
    /// сколько убыло у одного, столько прибыло у другого.
    ///
    /// Возвращает, сколько удалось перевести: меньше запрошенного,
    /// если у источника не хватило или у приёмника нет места.
    pub fn transfer(&mut self, from: ContainerId, to: ContainerId, want: Amount) -> Amount {
        if from == to {
            return 0;
        }
        // **Разрушенный контейнер не участвует в переводах.** Иначе
        // материя уходит из живого, приходит в мёртвого — и пропадает
        // из суммы, потому что мёртвых она не считает. Это поймал
        // property-тест: расхождение было отрицательным, то есть
        // материя исчезала бесследно, минуя все стоки.
        if !self.containers[from as usize].alive || !self.containers[to as usize].alive {
            return 0;
        }
        let avail = self.containers[from as usize].held;
        let room = self.room(to);
        let n = want.min(avail).min(room);
        if n == 0 {
            return 0;
        }
        self.containers[from as usize].held -= n;
        self.containers[to as usize].held += n;
        n
    }

    /// Списать материю в объявленный сток. **Единственный способ**
    /// уменьшить сумму в контейнерах.
    pub fn sink(&mut self, from: ContainerId, want: Amount, s: Sink) -> Amount {
        if !self.containers[from as usize].alive {
            return 0;
        }
        let n = want.min(self.containers[from as usize].held);
        self.containers[from as usize].held -= n;
        self.sunk[s as usize] += n;
        n
    }

    /// Разрушить контейнер: доля содержимого теряется (ADR-021),
    /// остальное уходит в обломок.
    ///
    /// `loss_num`/`loss_den` — доля потерь из данных, а не константа
    /// в коде (`CLAUDE.md`, «данные, не код»).
    pub fn breach(
        &mut self,
        id: ContainerId,
        loss_num: Amount,
        loss_den: Amount,
    ) -> (ContainerId, Amount, Amount) {
        if !self.containers[id as usize].alive {
            // Разрушить уже разрушенное — не работа. Возвращаем сам
            // контейнер, чтобы вызывающий не завёл пустышку.
            return (id, 0, 0);
        }
        let held = self.containers[id as usize].held;
        // Целочисленно и вниз: потеря не может превысить содержимое.
        let lost = held * loss_num / loss_den.max(1);
        self.sink(id, lost, Sink::ContainerBreach);
        let rest = self.containers[id as usize].held;
        // Обломок заводится **пустым**, иначе его содержимое попало бы
        // в начальную сумму вторым экземпляром.
        let debris = self.create(Kind::Debris, 0, Amount::MAX);
        self.transfer(id, debris, rest);
        self.containers[id as usize].alive = false;
        (debris, lost, rest)
    }

    /// Снизить потолок склада: излишек уходит в сток (ADR-043).
    pub fn lower_cap(&mut self, id: ContainerId, cap: Amount) -> Amount {
        self.containers[id as usize].cap = cap;
        let held = self.containers[id as usize].held;
        if held > cap {
            self.sink(id, held - cap, Sink::OverCap)
        } else {
            0
        }
    }

    /// Переработка: из `want` руды выходит `out_num/out_den`
    /// строительного материала, остальное — сток.
    ///
    /// **`want` соразмеряет вызывающий, а не этот метод.** Всё взятое,
    /// что не ушло приёмнику, списывается в сток: сдай полный бак
    /// в полный склад — и груз уйдёт туда целиком. Это не сток,
    /// а уничтожение груза полным приёмником, и выглядит оно как
    /// работающая переработка. Считать `want` надо от **места**
    /// у приёмника: `room * out_den / out_num` (ADR-230).
    pub fn refine(
        &mut self,
        from: ContainerId,
        to: ContainerId,
        want: Amount,
        out_num: Amount,
        out_den: Amount,
    ) -> Amount {
        let taken = want.min(self.containers[from as usize].held);
        let out = taken * out_num / out_den.max(1);
        let moved = self.transfer(from, to, out);
        // Всё, что взяли, но не перевели, — потери переработки.
        self.sink(from, taken - moved, Sink::RefiningLoss);
        moved
    }

    pub fn in_containers(&self) -> Amount {
        self.containers
            .iter()
            .filter(|c| c.alive)
            .map(|c| c.held)
            .sum()
    }

    pub fn sunk(&self, s: Sink) -> Amount {
        self.sunk[s as usize]
    }

    pub fn sunk_total(&self) -> Amount {
        self.sunk.iter().sum()
    }

    pub fn initial(&self) -> Amount {
        self.initial
    }

    /// **Главный инвариант проекта.** Сумма в живых контейнерах плюс
    /// всё, что ушло в объявленные стоки, равна тому, с чего начали.
    pub fn balanced(&self) -> bool {
        self.in_containers() + self.sunk_total() == self.initial
    }

    /// Разница, если инвариант нарушен, — для сообщения об ошибке.
    pub fn imbalance(&self) -> i128 {
        (self.in_containers() as i128 + self.sunk_total() as i128) - self.initial as i128
    }

    /// Снимок учёта для сохранения (ADR-229): контейнеры, стоки,
    /// начальная сумма и свободный список — всё, из чего он состоит.
    ///
    /// Наружу отдаётся **копией**, а не ссылкой на поля: учёт хранит
    /// инвариант «начальное == в контейнерах + по стокам», и место,
    /// где его можно править по кусочкам, было бы дырой в нём.
    pub fn snapshot(&self) -> (Vec<Container>, Vec<Amount>, Amount, u32, Vec<u32>) {
        (
            self.containers.clone(),
            self.sunk.to_vec(),
            self.initial,
            self.free_head,
            self.free.clone(),
        )
    }

    /// Учёт из снимка. Отказ — если снимок не сходится сам с собой:
    /// испорченный файл не должен становиться партией, в которой
    /// материя не сохраняется.
    pub fn from_snapshot(
        containers: Vec<Container>,
        sunk: Vec<Amount>,
        initial: Amount,
        free_head: u32,
        free: Vec<u32>,
    ) -> Option<Ledger> {
        if sunk.len() != SINKS.len()
            || free.len() != containers.len()
            || containers
                .iter()
                .any(|c| c.held > c.cap || (!c.alive && c.held != 0))
            || free
                .iter()
                .any(|&id| id != NIL && id as usize >= containers.len())
        {
            return None;
        }
        let mut free_slots = vec![false; containers.len()];
        let mut cursor = free_head;
        while cursor != NIL {
            let c = containers.get(cursor as usize)?;
            if c.alive || c.held != 0 || free_slots[cursor as usize] {
                return None;
            }
            free_slots[cursor as usize] = true;
            cursor = free[cursor as usize];
        }
        // Validate in a wider type: corrupt totals must return None, not overflow
        // a release build before the save can be rejected.
        let total = containers
            .iter()
            .filter(|c| c.alive)
            .map(|c| u128::from(c.held))
            .sum::<u128>()
            + sunk.iter().map(|&n| u128::from(n)).sum::<u128>();
        if total != u128::from(initial) {
            return None;
        }
        let mut l = Ledger {
            containers,
            sunk: [0; SINKS.len()],
            initial,
            free_head,
            free,
            free_slots,
        };
        l.sunk.copy_from_slice(&sunk);
        l.balanced().then_some(l)
    }

    pub fn live_containers(&self) -> usize {
        self.containers.iter().filter(|c| c.alive).count()
    }
}

/// Ресурс из объёма и плотности: **`CLAUDE.md`, «ресурс = объём ×
/// плотность материала»**. Богатая руда — материал с большей
/// плотностью, а не модификатор (ADR-100).
#[inline]
pub fn resource_of(voxels: Amount, density: u32) -> Amount {
    voxels * density as Amount
}

#[cfg(test)]
mod tests {
    #[test]
    fn releasing_a_slot_twice_cannot_reuse_a_live_container() {
        let mut l = Ledger::new();
        let id = l.create(Kind::Debris, 0, 100);
        assert!(l.retire_empty_debris(id));
        assert!(!l.release(id));
        let (c, s, i, h, f) = l.snapshot();
        let mut l = Ledger::from_snapshot(c, s, i, h, f).unwrap();
        assert!(!l.release(id));
        assert_eq!(l.create(Kind::Body, 11, 100), id);
        assert_ne!(l.create(Kind::Body, 19, 100), id);
        assert_eq!(l.held(id), 11);
        assert!(l.balanced());
    }
    #[test]
    fn corrupt_free_lists_and_overflowing_totals_are_rejected() {
        let empty = Container {
            kind: Kind::Debris,
            held: 0,
            cap: 100,
            alive: false,
        };
        let sunk = vec![0; SINKS.len()];
        assert!(Ledger::from_snapshot(vec![empty], sunk.clone(), 0, 0, vec![0]).is_none());
        assert!(Ledger::from_snapshot(vec![empty], sunk.clone(), 0, 1, vec![NIL]).is_none());
        assert!(Ledger::from_snapshot(vec![empty], sunk.clone(), 0, NIL, vec![]).is_none());
        let live = Container {
            alive: true,
            ..empty
        };
        assert!(Ledger::from_snapshot(vec![live], sunk.clone(), 0, 0, vec![NIL]).is_none());
        let huge = Container {
            held: u64::MAX,
            cap: u64::MAX,
            ..live
        };
        assert!(Ledger::from_snapshot(vec![huge, huge], sunk, 0, NIL, vec![NIL; 2]).is_none());
    }

    /// **Таблица стоков в каноне и перечень в коде — один список.**
    ///
    /// `CLAUDE.md`: «Единственный нормативный перечень стоков материи —
    /// таблица в `docs/07-economy.md`. Новый сток обязан добавить строку
    /// туда, а не утверждать, что она там есть». Сверял это человек
    /// глазами, и ADR-165 ввёл сток, строки не добавив (ADR-185).
    ///
    /// Имена сверяются дословно: список, совпадающий только числом
    /// строк, разошёлся бы по смыслу молча. На момент написания
    /// два имени из пяти были написаны в коде иначе, чем в каноне.
    ///
    /// `include_str!` разворачивается на сборке — ввода-вывода в тесте
    /// не появляется.
    #[test]
    fn the_canon_table_and_the_code_list_are_one_list() {
        const CANON: &str = include_str!("../../../docs/07-economy.md");
        let mut rows: Vec<String> = Vec::new();
        let mut inside = false;
        for line in CANON.lines() {
            let t = line.trim();
            if t.starts_with("| Сток |") {
                inside = true;
                continue;
            }
            if !inside {
                continue;
            }
            if !t.starts_with('|') {
                break;
            }
            if t.starts_with("|---") {
                continue;
            }
            let name = t.split('|').nth(1).unwrap_or("").trim().to_lowercase();
            if !name.is_empty() {
                rows.push(name);
            }
        }
        assert!(!rows.is_empty(), "таблица стоков в каноне не нашлась");
        let code: Vec<String> = SINKS.iter().map(|s| s.name().to_lowercase()).collect();
        assert_eq!(
            rows, code,
            "перечень стоков в каноне и в коде разошёлся:\nканон {rows:?}\nкод   {code:?}"
        );
    }
    use super::*;

    fn ground(l: &mut Ledger, n: Amount) -> ContainerId {
        l.create(Kind::Ground, n, Amount::MAX)
    }

    #[test]
    fn transfer_cannot_change_the_total() {
        let mut l = Ledger::new();
        let g = ground(&mut l, 1000);
        let h = l.create(Kind::Hauler, 0, 200);
        assert_eq!(l.transfer(g, h, 500), 200, "бак переполнился");
        assert!(l.balanced());
        assert_eq!(l.transfer(h, g, 1000), 200);
        assert!(l.balanced());
        assert_eq!(l.held(g), 1000);
    }

    /// Перевод в самого себя — ноль, а не удвоение. Классическая
    /// дыра: `held -= n; held += n` на одном контейнере прошло бы,
    /// а на разных индексах через один буфер — нет.
    #[test]
    fn self_transfer_is_a_no_op() {
        let mut l = Ledger::new();
        let g = ground(&mut l, 100);
        assert_eq!(l.transfer(g, g, 50), 0);
        assert_eq!(l.held(g), 100);
        assert!(l.balanced());
    }

    /// Разрушение контейнера: доля теряется в объявленный сток,
    /// остальное уходит в обломок. Сумма сходится.
    #[test]
    fn breach_loses_a_declared_share_and_the_rest_becomes_debris() {
        let mut l = Ledger::new();
        let g = ground(&mut l, 1000);
        let h = l.create(Kind::Hauler, 0, 1000);
        l.transfer(g, h, 400);
        let (debris, lost, rest) = l.breach(h, 1, 4);
        assert_eq!(lost, 100);
        assert_eq!(rest, 300);
        assert_eq!(l.held(debris), 300);
        assert_eq!(l.sunk(Sink::ContainerBreach), 100);
        assert!(l.balanced(), "расхождение {}", l.imbalance());
    }

    /// Реклейм обломка чужим краулером — обычный перевод, тем же кодом,
    /// что копка (`docs/07-economy.md`).
    #[test]
    fn reclaiming_debris_is_just_a_transfer() {
        let mut l = Ledger::new();
        let g = ground(&mut l, 1000);
        let mine = l.create(Kind::Hauler, 0, 1000);
        l.transfer(g, mine, 400);
        let (debris, _, _) = l.breach(mine, 1, 10);
        let theirs = l.create(Kind::Hauler, 0, 1000);
        let got = l.transfer(debris, theirs, 1000);
        assert_eq!(got, 360);
        assert_eq!(l.held(theirs), 360);
        assert!(l.balanced());
    }

    /// Упавший потолок склада: излишек уходит в сток, а не исчезает.
    #[test]
    fn lowering_the_cap_sinks_the_excess() {
        let mut l = Ledger::new();
        let g = ground(&mut l, 1000);
        let d = l.create(Kind::Depot, 0, 1000);
        l.transfer(g, d, 800);
        assert_eq!(l.lower_cap(d, 500), 300);
        assert_eq!(l.sunk(Sink::OverCap), 300);
        assert!(l.balanced());
    }

    /// Переработка с выходом меньше единицы — единственный сток,
    /// доступный с первого дня (`07-economy.md`).
    #[test]
    fn refining_below_one_is_a_sink() {
        let mut l = Ledger::new();
        let g = ground(&mut l, 1000);
        let out = l.create(Kind::Depot, 0, Amount::MAX);
        assert_eq!(l.refine(g, out, 500, 7, 10), 350);
        assert_eq!(l.sunk(Sink::RefiningLoss), 150);
        assert_eq!(l.held(g), 500);
        assert!(l.balanced());
    }

    /// **Разрушенный контейнер не принимает материю.** Иначе она уходит
    /// из живого, приходит в мёртвого и пропадает из суммы, минуя все
    /// стоки. Дыру нашёл property-тест ниже; этот проверяет её адресно.
    #[test]
    fn a_breached_container_neither_gives_nor_takes() {
        let mut l = Ledger::new();
        let g = ground(&mut l, 1000);
        let h = l.create(Kind::Hauler, 0, 1000);
        l.transfer(g, h, 400);
        l.breach(h, 1, 4);
        assert!(!l.get(h).alive);
        assert_eq!(l.transfer(g, h, 100), 0, "мёртвый принял материю");
        assert_eq!(l.transfer(h, g, 100), 0, "мёртвый отдал материю");
        assert_eq!(
            l.sink(h, 100, Sink::Annihilation),
            0,
            "мёртвый списал материю"
        );
        assert!(l.balanced(), "расхождение {}", l.imbalance());
    }

    /// **Главный property-тест проекта.** Случайная последовательность
    /// операций не имеет права нарушить сохранение материи.
    #[test]
    fn matter_is_conserved_under_random_operations() {
        let mut r = fx::rng::Rng::new(17, 0xEC0);
        for round in 0..200u64 {
            let mut l = Ledger::new();
            let mut ids = vec![ground(&mut l, 10_000)];
            for _ in 0..8 {
                let kind = match r.below(4) {
                    0 => Kind::Hauler,
                    1 => Kind::Debris,
                    2 => Kind::Depot,
                    _ => Kind::Body,
                };
                let cap = if r.below(2) == 0 { 500 } else { Amount::MAX };
                ids.push(l.create(kind, 0, cap));
            }
            for _ in 0..300 {
                let a = ids[r.below(ids.len() as u32) as usize];
                let b = ids[r.below(ids.len() as u32) as usize];
                let n = r.below(400) as Amount;
                match r.below(6) {
                    0 => {
                        l.sink(a, n, SINKS[r.below(SINKS.len() as u32) as usize]);
                    }
                    1 => {
                        if l.get(a).alive && l.held(a) > 0 {
                            let (d, _, _) = l.breach(a, r.below(5) as Amount, 10);
                            ids.push(d);
                        }
                    }
                    2 => {
                        l.lower_cap(a, n);
                    }
                    3 => {
                        l.refine(a, b, n, r.below(11) as Amount, 10);
                    }
                    _ => {
                        l.transfer(a, b, n);
                    }
                }
                assert!(l.balanced(), "раунд {round}: расхождение {}", l.imbalance());
            }
        }
    }

    /// Ресурс считается объёмом на плотность, и богатая руда — просто
    /// материал с большей плотностью (ADR-100).
    #[test]
    fn richer_ore_yields_more_from_the_same_volume() {
        assert_eq!(resource_of(100, 3), 300);
        assert!(resource_of(100, 7) > resource_of(100, 3));
    }
}
