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
    /// Первый сосед узла с каждой стороны, в порядке `SIDES`.
    ///
    /// Соседей на одну сторону бывает несколько: шагнуть из клетки
    /// в соседнюю можно **на любой её уровень**, отстоящий не дальше
    /// посильного перепада. Уровни отсортированы, поэтому подходящие
    /// идут подряд, и хватает первого с количеством.
    ///
    /// Один сосед на сторону — «ближайший по высоте» — делал граф
    /// **направленным**: из клетки с двумя уровнями ближайшим оказывался
    /// не тот, из которого пришли. На D-7 это стоило шестнадцати
    /// процентов узлов без направления при перепаде в воксель
    /// и девяноста шести при перепаде в четыре (ADR-141).
    nb: Vec<[u32; 4]>,
    /// Сколько соседей с каждой стороны.
    nbn: Vec<[u8; 4]>,
    /// Высота каждого узла — **снимок**, снятый в `begin`.
    ///
    /// Не обращение к слою: слой меняется, пока граф собирается
    /// порциями, и клетка, собранная на третьем тике, видела бы
    /// соседа не таким, каким тот увидит её на шестом. Рёбра тогда
    /// расходятся, обход по ним перестаёт быть разметкой компонент,
    /// и часть узлов остаётся без цели — на D-7 это было четырнадцать
    /// процентов (ADR-141).
    lev: Vec<u16>,
    /// Посильный перепад — тоже снимок, по той же причине.
    max_step: u32,
    /// Метка компоненты связности узла.
    ///
    /// Живёт у графа, а не у поля: это свойство геометрии, и читают его
    /// двое — поле (чтобы дать цель каждой компоненте) и граф секторов
    /// (чтобы ответить «пути нет» одним сравнением). Две разметки
    /// разошлись бы молча.
    comp: Vec<u32>,
    pub components: usize,
    /// Место в разметке: узел, текущая метка, счётчик.
    lab_at: usize,
    lab_cur: u32,
    lab_n: u32,
    lab_stack: Vec<u32>,
    /// Клетка узла. Заполняется в `begin` тем же проходом, что считает
    /// узлы, поэтому стоит ноль.
    node_cell: Vec<u32>,
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

    /// Соседи узла с одной стороны — подряд идущие узлы соседней клетки.
    #[inline]
    pub fn side(&self, node: usize, k: usize) -> std::ops::Range<u32> {
        let first = self.nb[node][k];
        if first == NO_NODE {
            return 0..0;
        }
        first..first + self.nbn[node][k] as u32
    }

    /// Все соседи узла.
    #[inline]
    pub fn neighbours(&self, node: usize) -> impl Iterator<Item = u32> + '_ {
        (0..4).flat_map(move |k| self.side(node, k))
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
        self.max_step = nav.max_step;
        self.lev.clear();
        self.lev.resize(nodes as usize, 0);
        self.node_cell.clear();
        self.node_cell.resize(nodes as usize, 0);
        for cz in 0..h {
            for cx in 0..w {
                let i = (cz * w + cx) as usize;
                let levels = nav.levels_at(cx, cz);
                let base = self.start[i] as usize;
                self.lev[base..base + levels.len()].copy_from_slice(levels);
                for k in 0..levels.len() {
                    self.node_cell[base + k] = i as u32;
                }
            }
        }
        self.nb.clear();
        self.nb.resize(nodes as usize, [NO_NODE; 4]);
        self.nbn.clear();
        self.nbn.resize(nodes as usize, [0; 4]);
        self.edges = 0;
        self.cursor = 0;
        self.comp.clear();
        self.comp.resize(nodes as usize, u32::MAX);
        self.lab_stack.clear();
        self.lab_at = 0;
        self.lab_cur = 0;
        self.lab_n = 0;
        self.components = 0;
    }

    /// Клетка узла — линейный индекс в сетке нав-клеток.
    #[inline]
    pub fn cell_of(&self, node: usize) -> u32 {
        self.node_cell[node]
    }

    /// Сколько уровней у клетки.
    #[inline]
    pub fn levels_in(&self, cell: usize) -> usize {
        self.count[cell] as usize
    }

    /// Первый узел клетки.
    #[inline]
    pub fn first_node(&self, cell: usize) -> u32 {
        self.start[cell]
    }

    /// Метка компоненты узла. Одинаковая метка — дорога есть.
    #[inline]
    pub fn comp_of(&self, node: usize) -> u32 {
        self.comp[node]
    }

    /// Разметить компоненты, обойдя не более `budget` узлов.
    ///
    /// Стек обхода живёт между вызовами: разметка большой компоненты
    /// не помещается в один тик, а обходить её заново с каждой порцией
    /// значило бы не считать её никогда.
    ///
    /// Метки идут по возрастанию номера первого встреченного узла:
    /// разметка — функция графа, а не порядка обхода.
    pub fn label_step(&mut self, budget: u32) -> bool {
        let mut spent = 0u32;
        loop {
            while let Some(k) = self.lab_stack.pop() {
                for side in 0..4 {
                    for m in self.side(k as usize, side) {
                        if self.comp[m as usize] == u32::MAX {
                            self.comp[m as usize] = self.lab_cur;
                            self.lab_stack.push(m);
                        }
                    }
                }
                spent += 1;
                if spent >= budget {
                    return false;
                }
            }
            while self.lab_at < self.nodes && self.comp[self.lab_at] != u32::MAX {
                self.lab_at += 1;
            }
            if self.lab_at >= self.nodes {
                self.components = self.lab_n as usize;
                return true;
            }
            self.lab_cur = self.lab_n;
            self.lab_n += 1;
            self.comp[self.lab_at] = self.lab_cur;
            self.lab_stack.push(self.lab_at as u32);
        }
    }

    /// Досчитать рёбра, но не более чем для `budget` узлов.
    /// `true` — граф собран.
    pub fn build_step(&mut self, budget: u32) -> bool {
        let cells = self.w * self.h;
        let mut spent = 0u32;
        while self.cursor < cells {
            if spent >= budget {
                return false;
            }
            let i = self.cursor as usize;
            let (cx, cz) = (self.cursor % self.w, self.cursor / self.w);
            for li in 0..self.count[i] as usize {
                let node = (self.start[i] as usize) + li;
                let y = self.lev[node] as i32;
                for (k, (dx, dz)) in SIDES.iter().enumerate() {
                    let (nx, nz) = (cx as i32 + dx, cz as i32 + dz);
                    if nx < 0 || nz < 0 || nx >= self.w as i32 || nz >= self.h as i32 {
                        continue;
                    }
                    let (nx, nz) = (nx as u32, nz as u32);
                    let j = (nz * self.w + nx) as usize;
                    let n_there = self.count[j] as usize;
                    let base = self.start[j] as usize;
                    // Все уровни соседа в пределах посильного перепада.
                    // Отсортированы, значит подходящие идут подряд.
                    //
                    // Отношение симметрично по построению: если из `a`
                    // видно `b`, то из `b` видно `a`. Разметка компонент
                    // идёт по этим же рёбрам, и односторонность сделала бы
                    // её не разметкой, а произвольным разбиением.
                    let (mut lo, mut hi) = (n_there, 0usize);
                    for lj in 0..n_there {
                        if (self.lev[base + lj] as i32 - y).unsigned_abs() <= self.max_step {
                            lo = lo.min(lj);
                            hi = lj + 1;
                        }
                    }
                    if lo >= hi {
                        continue;
                    }
                    self.nb[node][k] = self.start[j] + lo as u32;
                    self.nbn[node][k] = (hi - lo) as u8;
                    self.edges += hi - lo;
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
        while !self.build_step(u32::MAX) {}
        while !self.label_step(u32::MAX) {}
    }

    /// Записать граф в буфер (ADR-229).
    ///
    /// Пишется **весь**, вместе с местом сборки (`cursor`) и местом
    /// разметки (`lab_*`). Незаконченная сборка — это отставание
    /// в тиках, а отставание одинаково у всех клиентов или не
    /// одинаково вовсе (`CLAUDE.md`, ADR-089).
    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.u32(self.w);
        b.u32(self.h);
        b.u32s(&self.start);
        b.u8s(&self.count);
        b.u32(self.nb.len() as u32);
        for n in &self.nb {
            for &x in n {
                b.u32(x);
            }
        }
        b.u32(self.nbn.len() as u32);
        for n in &self.nbn {
            for &x in n {
                b.u8(x);
            }
        }
        b.u16s(&self.lev);
        b.u32(self.max_step);
        b.u32s(&self.comp);
        b.usize(self.components);
        b.usize(self.lab_at);
        b.u32(self.lab_cur);
        b.u32(self.lab_n);
        b.u32s(&self.lab_stack);
        b.u32s(&self.node_cell);
        b.usize(self.nodes);
        b.usize(self.edges);
        b.u32(self.cursor);
    }

    pub fn load(&mut self, s: &mut fx::blob::Scan) {
        self.w = s.u32();
        self.h = s.u32();
        self.start = s.u32s();
        self.count = s.u8s();
        let n = s.list(16);
        self.nb = (0..n).map(|_| [s.u32(), s.u32(), s.u32(), s.u32()]).collect();
        let n = s.list(4);
        self.nbn = (0..n).map(|_| [s.u8(), s.u8(), s.u8(), s.u8()]).collect();
        self.lev = s.u16s();
        self.max_step = s.u32();
        self.comp = s.u32s();
        self.components = s.usize();
        self.lab_at = s.usize();
        self.lab_cur = s.u32();
        self.lab_n = s.u32();
        self.lab_stack = s.u32s();
        self.node_cell = s.u32s();
        self.nodes = s.usize();
        self.edges = s.usize();
        self.cursor = s.u32();
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
    stack: Vec<u32>,
    best: Vec<(u64, u32)>,
    /// Место в развёртке: номер обхода и клетка.
    sweep: usize,
    at: u32,
    round_changed: bool,
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
        self.stack.clear();
        self.updates = 0;
        self.reachable = 0;
        self.sweep = 0;
        self.at = 0;
        self.round_changed = false;
        self.seed_at = 0;
        self.pub_at = 0;
        self.components = 0;
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
            self.components = g.components;
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
                let c = g.comp_of(node as usize) as usize;
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
            for m in g.neighbours(s as usize) {
                self.active[m as usize] = true;
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
        while !self.seed_step(g, tcx, tcz, u32::MAX) {}
    }

    /// Подготовить решение в зоне: обнулить **только её** узлы.
    ///
    /// Массивы общие на весь граф и заводятся один раз, а трогается
    /// только зона: иначе каждый сектор стоил бы прохода по карте.
    pub fn alloc_zone(
        &mut self,
        g: &VolGraph,
        nodes: &[u32],
        zone: &[u32],
        zone_id: u32,
        seeds: &[(u32, Time)],
    ) {
        let n = g.nodes;
        if self.t.len() != n {
            self.t.clear();
            self.t.resize(n, INF);
            self.dir.clear();
            self.dir.resize(n, 255);
            self.active.clear();
            self.active.resize(n, false);
            self.fixed.clear();
            self.fixed.resize(n, false);
        }
        for &node in nodes {
            let node = node as usize;
            self.t[node] = INF;
            self.dir[node] = 255;
            self.active[node] = false;
            self.fixed[node] = false;
        }
        self.updates = 0;
        for &(node, v) in seeds {
            if zone[node as usize] != zone_id {
                continue;
            }
            self.t[node as usize] = v;
            self.fixed[node as usize] = true;
            for k in 0..4 {
                for m in g.side(node as usize, k) {
                    if zone[m as usize] == zone_id {
                        self.active[m as usize] = true;
                    }
                }
            }
        }
    }

    /// Развёртка в пределах зоны до неподвижной точки.
    ///
    /// Обходов два, вперёд и назад по списку узлов, а не четыре: список
    /// уже в растровом порядке, и две его стороны — это те же две оси,
    /// пройденные в обе стороны.
    pub fn sweep_zone<F>(
        &mut self,
        g: &VolGraph,
        nodes: &[u32],
        zone: &[u32],
        zone_id: u32,
        h: &F,
    ) where
        F: Fn(u32, u32) -> u32,
    {
        loop {
            let mut changed = false;
            for back in [false, true] {
                for k in 0..nodes.len() {
                    let node = nodes[if back { nodes.len() - 1 - k } else { k }] as usize;
                    if !self.active[node] {
                        continue;
                    }
                    if self.fixed[node] {
                        self.active[node] = false;
                        continue;
                    }
                    let side = |j: usize| -> Time {
                        g.side(node, j)
                            .filter(|&m| zone[m as usize] == zone_id)
                            .map(|m| self.t[m as usize])
                            .min()
                            .unwrap_or(INF)
                    };
                    let a = side(1).min(side(2));
                    let b = side(0).min(side(3));
                    self.updates += 1;
                    let q = update2d(a, b, STEP_COST, h);
                    if q < self.t[node] {
                        self.t[node] = q;
                        changed = true;
                        for j in 0..4 {
                            for m in g.side(node, j) {
                                if zone[m as usize] == zone_id && !self.fixed[m as usize] {
                                    self.active[m as usize] = true;
                                }
                            }
                        }
                    } else {
                        self.active[node] = false;
                    }
                }
            }
            if !changed {
                return;
            }
        }
    }

    /// Стереть направления зоны: до цели оттуда не дойти.
    pub fn clear_zone(&mut self, nodes: &[u32]) {
        for &node in nodes {
            if (node as usize) < self.dir.len() {
                self.dir[node as usize] = 255;
                self.t[node as usize] = INF;
            }
        }
    }

    /// Направления для узлов зоны.
    pub fn publish_zone(&mut self, g: &VolGraph, nodes: &[u32], zone: &[u32], zone_id: u32) {
        for &node in nodes {
            let node = node as usize;
            if self.t[node] == INF {
                continue;
            }
            let mut best = self.t[node];
            let mut best_k = 255u8;
            let mut best_m = NO_NODE;
            for k in 0..4 {
                for m in g.side(node, k) {
                    if zone[m as usize] != zone_id {
                        continue;
                    }
                    let tm = self.t[m as usize];
                    if tm == INF {
                        continue;
                    }
                    if tm < best || (tm == best && m < best_m) {
                        best = tm;
                        best_k = k as u8;
                        best_m = m;
                    }
                }
            }
            self.dir[node] = best_k;
        }
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
                    let side = |k: usize| -> Time {
                        g.side(node, k)
                            .map(|m| self.t[m as usize])
                            .min()
                            .unwrap_or(INF)
                    };
                    let a = side(1).min(side(2));
                    let b = side(0).min(side(3));
                    self.updates += 1;
                    spent += 1;
                    let q = update2d(a, b, STEP_COST, h);
                    if q < self.t[node] {
                        self.t[node] = q;
                        self.round_changed = true;
                        for m in g.neighbours(node) {
                            if !self.fixed[m as usize] {
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
            let mut best = self.t[node];
            let mut best_k = 255u8;
            let mut best_m = NO_NODE;
            for k in 0..4 {
                for m in g.side(node, k) {
                    let tm = self.t[m as usize];
                    if tm == INF {
                        continue;
                    }
                    if tm < best || (tm == best && m < best_m) {
                        best = tm;
                        best_k = k as u8;
                        best_m = m;
                    }
                }
            }
            self.dir[node] = best_k;
        }
        true
    }

    /// Записать поле в буфер (ADR-229). Вместе с местом развёртки,
    /// местом засева и местом публикации: недосчитанное поле —
    /// такое же состояние, как досчитанное.
    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.u32s(&self.t);
        b.u8s(&self.dir);
        b.bools(&self.active);
        b.bools(&self.fixed);
        b.u32s(&self.stack);
        b.u32(self.best.len() as u32);
        for &(a, c) in &self.best {
            b.u64(a);
            b.u32(c);
        }
        b.usize(self.sweep);
        b.u32(self.at);
        b.bool(self.round_changed);
        b.u32(self.seed_at);
        b.usize(self.pub_at);
        b.usize(self.components);
        b.u64(self.updates);
        b.usize(self.reachable);
    }

    pub fn load(&mut self, s: &mut fx::blob::Scan) {
        self.t = s.u32s();
        self.dir = s.u8s();
        self.active = s.bools();
        self.fixed = s.bools();
        self.stack = s.u32s();
        let n = s.list(12);
        self.best = (0..n).map(|_| (s.u64(), s.u32())).collect();
        self.sweep = s.usize();
        self.at = s.u32();
        self.round_changed = s.bool();
        self.seed_at = s.u32();
        self.pub_at = s.usize();
        self.components = s.usize();
        self.updates = s.u64();
        self.reachable = s.usize();
    }
}

/// Решить поле **в пределах одной зоны** с заданными засевами.
///
/// Зона — обычно сектор; засевы — его выходы с уже известной ценой
/// добраться до цели. Это то, ради чего засев умеет быть не нулевым
/// (ADR-151): пересчёт сектора стартует от граничных значений, а не
/// от цели внутри него.
///
/// `nodes` — узлы зоны в растровом порядке, `zone` — принадлежность
/// узла зоне, чтобы не заглядывать за её край. Решается до конца:
/// сектор мал, рвать его на порции незачем.
pub fn solve_zone<F>(
    g: &VolGraph,
    f: &mut VolField,
    nodes: &[u32],
    zone: &[u32],
    zone_id: u32,
    seeds: &[(u32, Time)],
    h: &F,
) where
    F: Fn(u32, u32) -> u32,
{
    f.alloc_zone(g, nodes, zone, zone_id, seeds);
    f.sweep_zone(g, nodes, zone, zone_id, h);
    f.publish_zone(g, nodes, zone, zone_id);
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
        let (g, _f, nav) = field(true);
        let near = floor_node(&g, &nav, 8, 16);
        let far = floor_node(&g, &nav, 24, 16);
        assert_eq!(
            g.comp_of(near),
            g.comp_of(far),
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
        assert_ne!(g.comp_of(near), g.comp_of(far), "стена не разрезала карту");
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

    /// **Каждый узел получает направление.** У каждой компоненты своя
    /// цель, значит недостижимых узлов быть не может. Пока рёбра были
    /// односторонними, на D-7 без направления оставалось четырнадцать
    /// процентов узлов при перепаде в воксель и девяносто шесть при
    /// перепаде в четыре (ADR-141).
    #[test]
    fn every_node_gets_a_direction() {
        for tunnel in [true, false] {
            let (g, f, _) = field(tunnel);
            assert_eq!(
                f.reachable, g.nodes,
                "узлов без времени: {}",
                g.nodes - f.reachable
            );
            let blind = (0..g.nodes).filter(|&n| f.dir[n] == 255).count();
            // Без направления остаются только сами цели: идти им некуда.
            assert_eq!(blind, g.components, "узлов без направления больше, чем целей");
        }
    }

    /// **Ребро двустороннее.** Из `a` видно `b` тогда и только тогда,
    /// когда из `b` видно `a`, — иначе разметка компонент перестаёт
    /// быть разметкой.
    #[test]
    fn an_edge_is_mutual() {
        let (g, _, _) = field(true);
        for n in 0..g.nodes {
            for k in 0..4 {
                // Обратная сторона: -Z против +Z, -X против +X.
                let back = [3usize, 2, 1, 0][k];
                for m in g.side(n, k) {
                    assert!(
                        g.side(m as usize, back).any(|x| x as usize == n),
                        "ребро {n} -> {m} по стороне {k} не двустороннее"
                    );
                }
            }
        }
    }

}
