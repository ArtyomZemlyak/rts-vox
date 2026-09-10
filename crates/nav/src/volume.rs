//! Поле направлений по **объёмной** навигации: узел — «клетка и уровень».
//!
//! Плоская сетка не годится ни в какой проекции, и это выяснено замером,
//! а не рассуждением. Сетка по **нижнему** уровню описывает подземелья:
//! на D-7 она рассыпалась на 1 618 кусков, крупнейший в 13% клеток.
//! Сетка по **поверхности** склеивает карту в один кусок на 99.7% —
//! и не видит прокопанного туннеля, потому что над ним порода, а копка
//! в этой игре и есть экономика.
//!
//! Поэтому узел здесь — не клетка, а место, где юнит стоит: клетка плюс
//! номер уровня. Ребро спрашивается у слоя как `step_to`, то есть
//! **перепад остаётся свойством ребра** (ADR-121).
//!
//! Обновление — то же годуновское, что у сеточных решателей
//! (ADR-053…055): у узла по-прежнему не больше одного соседа на каждую
//! из четырёх сторон, поэтому вся арифметика переносится как есть.
//!
//! **Работа порезана на порции.** Целиком поле D-7 считается 38 мс —
//! вчетверо больше всего тика. Поэтому расчёт идёт с потолком
//! обновлений за тик и продолжается со следующего, а юниты всё это
//! время читают **опубликованное** поле, посчитанное до конца.
//! Отставание получается в тиках и одинаково у всех клиентов (ADR-089),
//! а не «сколько успели».

use crate::godunov::{update2d, Cost, Time, INF};
use crate::layer::NavLayer;

/// Нет такого узла.
pub const NO_NODE: u32 = u32::MAX;

/// Стоимость шага. Та же десятка, на которой меряли решатели.
pub const STEP_COST: Cost = 10;

/// Порядок сторон. Совпадает с `DIRS` в симуляции: -Z, -X, +X, +Z.
/// Юнит читает номер стороны напрямую, поэтому порядок нормативен.
pub const SIDES: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];

/// Граф «клетка и уровень».
#[derive(Default)]
pub struct VolGraph {
    w: u32,
    h: u32,
    /// Первый узел клетки.
    start: Vec<u32>,
    /// Сколько узлов у клетки.
    count: Vec<u8>,
    /// Соседи узла в порядке `SIDES`.
    nb: Vec<[u32; 4]>,
    pub nodes: usize,
    pub edges: usize,
    /// Клетка, с которой продолжится сборка рёбер.
    cursor: u32,
}

impl VolGraph {
    pub fn new() -> VolGraph {
        VolGraph::default()
    }

    pub fn dims(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Узел по клетке и номеру уровня.
    #[inline]
    pub fn node(&self, cx: u32, cz: u32, li: usize) -> u32 {
        let i = (cz * self.w + cx) as usize;
        if i >= self.count.len() || li >= self.count[i] as usize {
            return NO_NODE;
        }
        self.start[i] + li as u32
    }

    /// Начать сборку: пересчитать узлы. Проход дешёвый — по клеткам,
    /// а не по узлам, — поэтому идёт целиком.
    pub fn begin(&mut self, nav: &NavLayer) {
        let (w, h) = nav.dims();
        let cells = (w * h) as usize;
        self.w = w;
        self.h = h;
        self.start.clear();
        self.start.resize(cells, 0);
        self.count.clear();
        self.count.resize(cells, 0);
        let mut nodes = 0u32;
        for cz in 0..h {
            for cx in 0..w {
                let i = (cz * w + cx) as usize;
                let n = nav.levels_at(cx, cz).len() as u8;
                self.start[i] = nodes;
                self.count[i] = n;
                nodes += n as u32;
            }
        }
        self.nodes = nodes as usize;
        self.nb.clear();
        self.nb.resize(nodes as usize, [NO_NODE; 4]);
        self.edges = 0;
        self.cursor = 0;
    }

    /// Досчитать рёбра, но не более чем для `budget` узлов.
    /// `true` — граф собран.
    pub fn build_step(&mut self, nav: &NavLayer, budget: u32) -> bool {
        let cells = self.w * self.h;
        let mut spent = 0u32;
        while self.cursor < cells {
            if spent >= budget {
                return false;
            }
            let i = self.cursor as usize;
            let (cx, cz) = (self.cursor % self.w, self.cursor / self.w);
            let levels = nav.levels_at(cx, cz);
            // Граф собирается порциями, а слой за это время меняется:
            // копка добавляет и убирает уровни. Число узлов зафиксировано
            // в `begin`, поэтому здесь берётся оно, а не сегодняшняя
            // длина списка — граф остаётся **снимком**, а не смесью
            // двух состояний с ссылками в никуда.
            for li in 0..self.count[i] as usize {
                let Some(&yv) = levels.get(li) else { continue };
                let y = yv as i32;
                let node = (self.start[i] as usize) + li;
                for (k, (dx, dz)) in SIDES.iter().enumerate() {
                    let (nx, nz) = (cx as i32 + dx, cz as i32 + dz);
                    if nx < 0 || nz < 0 || nx >= self.w as i32 || nz >= self.h as i32 {
                        continue;
                    }
                    let (nx, nz) = (nx as u32, nz as u32);
                    let Some(l) = nav.step_to((cx, cz), y, (nx, nz)) else { continue };
                    let j = (nz * self.w + nx) as usize;
                    // `step_to` вернул **высоту**, а узлу нужен номер
                    // уровня: их порядок и есть нумерация узлов.
                    let Some(lj) = nav.levels_at(nx, nz).iter().position(|&v| v == l) else {
                        continue;
                    };
                    if lj >= self.count[j] as usize {
                        continue;
                    }
                    self.nb[node][k] = self.start[j] + lj as u32;
                    self.edges += 1;
                }
            }
            spent += (self.count[i] as u32).max(1);
            self.cursor += 1;
        }
        true
    }

    /// Собрать целиком. Для тестов и замеров.
    pub fn build(&mut self, nav: &NavLayer) {
        self.begin(nav);
        while !self.build_step(nav, u32::MAX) {}
    }
}

/// Рабочее поле: время до цели, развёртка и её место.
#[derive(Default)]
pub struct VolField {
    pub t: Vec<Time>,
    /// Номер стороны из `SIDES`; 255 — идти некуда.
    pub dir: Vec<u8>,
    active: Vec<bool>,
    fixed: Vec<bool>,
    comp: Vec<u32>,
    stack: Vec<u32>,
    best: Vec<(u64, u32)>,
    /// Место в развёртке: номер обхода и клетка.
    sweep: usize,
    at: u32,
    round_changed: bool,
    /// Место в разметке компонент: узел, метка, счётчик.
    lab_at: usize,
    lab_cur: u32,
    lab_n: u32,
    /// Место в выборе целей и в публикации.
    seed_at: u32,
    pub_at: usize,
    pub components: usize,
    pub updates: u64,
    pub reachable: usize,
}

/// Четыре порядка обхода. Развёртка в обе стороны по обеим осям —
/// это и есть LSM.
const SWEEPS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

impl VolField {
    pub fn new() -> VolField {
        VolField::default()
    }

    /// Метка компоненты узла. Одинаковая метка — дорога есть.
    pub fn comp_of(&self, node: usize) -> u32 {
        self.comp[node]
    }

    /// Отвести память и сбросить курсоры. Дальше работа идёт стадиями,
    /// и у каждой свой потолок за тик.
    pub fn alloc(&mut self, g: &VolGraph) {
        let n = g.nodes;
        self.t.clear();
        self.t.resize(n, INF);
        self.dir.clear();
        self.dir.resize(n, 255);
        self.active.clear();
        self.active.resize(n, false);
        self.fixed.clear();
        self.fixed.resize(n, false);
        self.comp.clear();
        self.comp.resize(n, u32::MAX);
        self.stack.clear();
        self.updates = 0;
        self.reachable = 0;
        self.sweep = 0;
        self.at = 0;
        self.round_changed = false;
        self.lab_at = 0;
        self.lab_cur = 0;
        self.lab_n = 0;
        self.seed_at = 0;
        self.pub_at = 0;
        self.components = 0;
    }

    /// Разметить компоненты, обойдя не более `budget` узлов.
    ///
    /// Стек обхода живёт между вызовами: разметка большой компоненты
    /// не помещается в один тик, а обходить её заново с каждой порцией
    /// значило бы не считать её никогда.
    pub fn label_step(&mut self, g: &VolGraph, budget: u32) -> bool {
        let mut spent = 0u32;
        loop {
            while let Some(k) = self.stack.pop() {
                for &m in g.nb[k as usize].iter() {
                    if m != NO_NODE && self.comp[m as usize] == u32::MAX {
                        self.comp[m as usize] = self.lab_cur;
                        self.stack.push(m);
                    }
                }
                spent += 1;
                if spent >= budget {
                    return false;
                }
            }
            while self.lab_at < g.nodes && self.comp[self.lab_at] != u32::MAX {
                self.lab_at += 1;
            }
            if self.lab_at >= g.nodes {
                self.components = self.lab_n as usize;
                return true;
            }
            self.lab_cur = self.lab_n;
            self.lab_n += 1;
            self.comp[self.lab_at] = self.lab_cur;
            self.stack.push(self.lab_at as u32);
        }
    }

    /// Выбрать цели: у каждой компоненты свой узел, ближайший к точке.
    ///
    /// **Приказ называет точку, а не клетку.** Юнит, которому до неё
    /// не дойти, идёт настолько близко, насколько карта позволяет.
    /// Иначе одна воронка, накрывшая точку сбора, замораживает всю
    /// армию: на D-7 обе точки сбора после прогрева оказались в ямах
    /// на четыре и на двадцать две клетки (ADR-138).
    pub fn seed_step(&mut self, g: &VolGraph, tcx: u32, tcz: u32, budget: u32) -> bool {
        if self.seed_at == 0 {
            self.best.clear();
            self.best.resize(self.components, (u64::MAX, NO_NODE));
        }
        let cells = g.w * g.h;
        let mut spent = 0u32;
        while self.seed_at < cells {
            if spent >= budget {
                return false;
            }
            let (cx, cz) = (self.seed_at % g.w, self.seed_at / g.w);
            let i = self.seed_at as usize;
            let (dx, dz) = (cx as i64 - tcx as i64, cz as i64 - tcz as i64);
            let d = (dx * dx + dz * dz) as u64;
            for li in 0..g.count[i] as usize {
                let node = g.start[i] + li as u32;
                // Ничьи — по номеру узла (`CLAUDE.md`).
                let c = self.comp[node as usize] as usize;
                if (d, node) < self.best[c] {
                    self.best[c] = (d, node);
                }
            }
            spent += 1;
            self.seed_at += 1;
        }
        for k in 0..self.best.len() {
            let s = self.best[k].1;
            if s == NO_NODE {
                continue;
            }
            self.t[s as usize] = 0;
            self.fixed[s as usize] = true;
            for &m in g.nb[s as usize].iter() {
                if m != NO_NODE {
                    self.active[m as usize] = true;
                }
            }
        }
        true
    }

    /// Начать расчёт целиком — для тестов и замеров.
    pub fn begin(&mut self, g: &VolGraph, tcx: u32, tcz: u32) {
        self.alloc(g);
        if g.nodes == 0 {
            return;
        }
        while !self.label_step(g, u32::MAX) {}
        while !self.seed_step(g, tcx, tcz, u32::MAX) {}
    }

    /// Продолжить развёртку, потратив не более `budget` обновлений.
    /// `true` — поле сошлось.
    pub fn sweep_step<F>(&mut self, g: &VolGraph, budget: u64, h: &F) -> bool
    where
        F: Fn(u32, u32) -> u32,
    {
        if g.nodes == 0 {
            return true;
        }
        let cells = g.w * g.h;
        let mut spent = 0u64;
        loop {
            let (rx, rz) = SWEEPS[self.sweep];
            while self.at < cells {
                if spent >= budget {
                    return false;
                }
                let k = self.at;
                let (zk, xk) = (k / g.w, k % g.w);
                let cz = if rz { g.h - 1 - zk } else { zk };
                let cx = if rx { g.w - 1 - xk } else { xk };
                let i = (cz * g.w + cx) as usize;
                for li in 0..g.count[i] as usize {
                    let node = (g.start[i] as usize) + li;
                    if !self.active[node] {
                        continue;
                    }
                    if self.fixed[node] {
                        self.active[node] = false;
                        continue;
                    }
                    let nb = g.nb[node];
                    let tv = |m: u32| if m == NO_NODE { INF } else { self.t[m as usize] };
                    let a = tv(nb[1]).min(tv(nb[2]));
                    let b = tv(nb[0]).min(tv(nb[3]));
                    self.updates += 1;
                    spent += 1;
                    let q = update2d(a, b, STEP_COST, h);
                    if q < self.t[node] {
                        self.t[node] = q;
                        self.round_changed = true;
                        for &m in nb.iter() {
                            if m != NO_NODE && !self.fixed[m as usize] {
                                self.active[m as usize] = true;
                            }
                        }
                    } else {
                        self.active[node] = false;
                    }
                }
                self.at += 1;
            }
            self.at = 0;
            self.sweep += 1;
            if self.sweep == SWEEPS.len() {
                self.sweep = 0;
                if !self.round_changed {
                    return true;
                }
                self.round_changed = false;
            }
        }
    }

    /// Сторона, куда идти: сосед с наименьшим временем, ничьи — по
    /// номеру узла. Не более `budget` узлов за раз.
    pub fn publish_step(&mut self, g: &VolGraph, budget: usize) -> bool {
        let mut spent = 0usize;
        while self.pub_at < g.nodes {
            if spent >= budget {
                return false;
            }
            let node = self.pub_at;
            self.pub_at += 1;
            spent += 1;
            if self.t[node] == INF {
                continue;
            }
            self.reachable += 1;
            let nb = g.nb[node];
            let mut best = self.t[node];
            let mut best_k = 255u8;
            let mut best_m = NO_NODE;
            for (k, &m) in nb.iter().enumerate() {
                if m == NO_NODE || self.t[m as usize] == INF {
                    continue;
                }
                let tm = self.t[m as usize];
                if tm < best || (tm == best && m < best_m) {
                    best = tm;
                    best_k = k as u8;
                    best_m = m;
                }
            }
            self.dir[node] = best_k;
        }
        true
    }
}

/// Решить поле целиком. Для тестов и замеров: в тике так нельзя.
pub fn solve<F>(g: &VolGraph, f: &mut VolField, tcx: u32, tcz: u32, h: &F)
where
    F: Fn(u32, u32) -> u32,
{
    f.begin(g, tcx, tcz);
    while !f.sweep_step(g, u64::MAX, h) {}
    while !f.publish_step(g, usize::MAX) {}
}

/// Потолок работы за тик.
///
/// Три числа, а не одно: узел сборки стоит четырёх `step_to`, узел
/// публикации — четырёх чтений, а обновление — годуновского корня.
/// Общий потолок мерил бы разное одной меркой.
#[derive(Clone, Copy)]
pub struct Budget {
    /// Клеток на сборку рёбер.
    pub cells: u32,
    /// Узлов на разметку, выбор целей и публикацию.
    pub scan: u32,
    /// Годуновских обновлений на развёртку.
    pub updates: u64,
}

/// Маршрут владельца: опубликованное поле и то, что считается сейчас.
///
/// Два графа, а не один: пока идёт расчёт, юниты читают предыдущий,
/// и номера узлов у него свои. Один граф на двоих означал бы, что
/// на середине пересборки направление читается по чужой нумерации.
#[derive(Default)]
pub struct VolRoute {
    /// Граф опубликованного поля.
    pub g: VolGraph,
    /// Опубликованные направления, по узлам `g`.
    pub dir: Vec<u8>,
    wg: VolGraph,
    wf: VolField,
    stage: Stage,
    /// Тиков идёт текущий расчёт.
    pub ticks: u32,
    /// Тиков ушло на последний законченный.
    pub last_ticks: u32,
    /// Полей опубликовано.
    pub published: u64,
    pub components: usize,
    pub reachable: usize,
    pub updates: u64,
}

/// Стадии расчёта. У каждой свой потолок работы за тик: без него
/// поле D-7 забирает 38 мс в одном тике при бюджете в десять.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    Build,
    Label,
    Seed,
    Sweep,
    Publish,
}

impl VolRoute {
    pub fn new() -> VolRoute {
        VolRoute::default()
    }

    /// Есть ли что читать юниту.
    pub fn ready(&self) -> bool {
        self.published > 0
    }

    /// Направление для узла опубликованного графа.
    #[inline]
    pub fn dir_at(&self, cx: u32, cz: u32, li: usize) -> u8 {
        let node = self.g.node(cx, cz, li);
        if node == NO_NODE || node as usize >= self.dir.len() {
            return 255;
        }
        self.dir[node as usize]
    }

    /// Продвинуть расчёт на порцию работы. `true` — поле опубликовано.
    ///
    /// `b.cells` — потолок клеток на сборку рёбер, `b.scan` — узлов
    /// на разметку, выбор целей и публикацию, `updates` — потолок
    /// годуновских обновлений. Три числа, а не одно: узел сборки
    /// стоит четырёх `step_to`, а узел публикации — четырёх чтений,
    /// и общий потолок мерил бы разное одной меркой.
    ///
    /// Все три **из данных**, а не «сколько успеем»: разное отставание
    /// у разных клиентов это десинк (ADR-089).
    pub fn step<F>(&mut self, nav: &NavLayer, tcx: u32, tcz: u32, b: Budget, h: &F) -> bool
    where
        F: Fn(u32, u32) -> u32,
    {
        self.ticks += 1;
        match self.stage {
            Stage::Idle => {
                self.wg.begin(nav);
                self.stage = Stage::Build;
                return false;
            }
            Stage::Build => {
                if !self.wg.build_step(nav, b.cells) {
                    return false;
                }
                self.wf.alloc(&self.wg);
                self.stage = Stage::Label;
                return false;
            }
            Stage::Label => {
                if !self.wf.label_step(&self.wg, b.scan) {
                    return false;
                }
                self.stage = Stage::Seed;
                return false;
            }
            Stage::Seed => {
                if !self.wf.seed_step(&self.wg, tcx, tcz, b.scan) {
                    return false;
                }
                self.stage = Stage::Sweep;
                return false;
            }
            Stage::Sweep => {
                if !self.wf.sweep_step(&self.wg, b.updates, h) {
                    return false;
                }
                self.stage = Stage::Publish;
                return false;
            }
            Stage::Publish => {
                if !self.wf.publish_step(&self.wg, b.scan as usize) {
                    return false;
                }
            }
        }
        // Сошлось: меняем рабочее с опубликованным местами.
        std::mem::swap(&mut self.g, &mut self.wg);
        std::mem::swap(&mut self.dir, &mut self.wf.dir);
        self.components = self.wf.components;
        self.reachable = self.wf.reachable;
        self.updates = self.wf.updates;
        self.published += 1;
        self.last_ticks = self.ticks;
        self.ticks = 0;
        self.stage = Stage::Idle;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::godunov::h_exact_floor;
    use voxel::store::{Mat, AIR};

    /// Ровная земля до `GY`, стена по `x == WALL` во всю высоту,
    /// и туннель в ней при `z` в `[30, 36)`.
    struct Tunnel {
        tunnel: bool,
    }
    const GY: u32 = 8;
    const WALL: u32 = 32;
    impl voxel::Generator for Tunnel {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if y < GY {
                return 2;
            }
            if x.abs_diff(WALL) <= 1 && y < 40 {
                if self.tunnel && (30..36).contains(&z) && (GY..GY + 6).contains(&y) {
                    return AIR;
                }
                return 3;
            }
            AIR
        }
    }

    fn field(tunnel: bool) -> (VolGraph, VolField, NavLayer) {
        let w = voxel::World::new(64, 64, 64, 4096, Box::new(Tunnel { tunnel }));
        let nav = NavLayer::new(&w, 4, 2, 1);
        let mut g = VolGraph::new();
        g.build(&nav);
        let mut f = VolField::new();
        solve(&g, &mut f, 30, 16, &h_exact_floor);
        (g, f, nav)
    }

    fn floor_node(g: &VolGraph, nav: &NavLayer, cx: u32, cz: u32) -> usize {
        let li = nav
            .levels_at(cx, cz)
            .iter()
            .position(|&l| l as u32 == GY)
            .expect("уровень пола");
        let node = g.node(cx, cz, li);
        assert_ne!(node, NO_NODE);
        node as usize
    }

    /// **Туннель — дорога.** Стороны стены лежат в одной компоненте,
    /// хотя над туннелем тридцать два воксела породы. Плоская сетка
    /// по поверхности этой дороги не видит вовсе, а сетка по низу
    /// не видит крыш.
    #[test]
    fn a_tunnel_is_a_road() {
        let (g, f, nav) = field(true);
        let near = floor_node(&g, &nav, 8, 16);
        let far = floor_node(&g, &nav, 24, 16);
        assert_eq!(
            f.comp_of(near),
            f.comp_of(far),
            "туннель не связал стороны"
        );
    }

    /// **Без туннеля стены не обойти**, и поле это знает: стороны
    /// в разных компонентах, и у каждой своя цель.
    #[test]
    fn a_solid_wall_splits_the_map() {
        let (g, f, nav) = field(false);
        let near = floor_node(&g, &nav, 8, 16);
        let far = floor_node(&g, &nav, 24, 16);
        assert_ne!(f.comp_of(near), f.comp_of(far), "стена не разрезала карту");
        assert_ne!(f.t[near], INF, "у своей компоненты обязана быть цель");
        assert_ne!(f.t[far], INF, "у своей компоненты обязана быть цель");
    }

    /// **У крыши свой узел.** Слой видит несколько уровней, и граф
    /// обязан их различать, иначе поле снова наземное.
    #[test]
    fn a_roof_is_its_own_node() {
        let (g, _, nav) = field(true);
        let (w, h) = nav.dims();
        let mut multi = 0;
        for cz in 0..h {
            for cx in 0..w {
                if nav.levels_at(cx, cz).len() > 1 {
                    multi += 1;
                }
            }
        }
        assert!(multi > 0, "в мире нет многоуровневых клеток");
        assert!(g.nodes > (w * h) as usize, "узлов не больше клеток");
    }

    /// **Порционный расчёт даёт то же поле, что и разовый.** Потолок
    /// работы за тик не имеет права менять результат — иначе клиенты
    /// с разной нагрузкой разойдутся.
    #[test]
    fn a_budgeted_solve_matches_the_whole_one() {
        let w = voxel::World::new(64, 64, 64, 4096, Box::new(Tunnel { tunnel: true }));
        let nav = NavLayer::new(&w, 4, 2, 1);
        let (g, f, _) = field(true);
        let mut r = VolRoute::new();
        let mut ticks = 0;
        let b = Budget { cells: 37, scan: 53, updates: 111 };
        while !r.step(&nav, 30, 16, b, &h_exact_floor) {
            ticks += 1;
            assert!(ticks < 100_000, "порционный расчёт не сошёлся");
        }
        assert_eq!(r.dir.len(), f.dir.len());
        assert_eq!(r.dir, f.dir, "порционное поле разошлось с разовым");
        assert_eq!(r.g.nodes, g.nodes);
        assert!(r.last_ticks > 1, "порция оказалась не порцией");
    }
}
