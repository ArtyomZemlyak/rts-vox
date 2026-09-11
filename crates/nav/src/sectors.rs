//! Граф секторов и порталов **по объёмной навигации**.
//!
//! Плоский граф порталов (`portals.rs`) режет сетку `Grid`, у которой
//! на клетку один уровень. Такой граф не видит ни туннеля под горой,
//! ни крыши: узлом ему служит клетка, а клетка — не место, где стоит
//! юнит (ADR-138). Здесь узлом служит то же, что в `volume.rs`, —
//! **клетка плюс уровень**.
//!
//! Зачем: поле направлений считается по всей карте и потому отстаёт
//! на сотню тиков (ADR-138). Архитектура (ADR-106) требует другого —
//! дальнее вести графом порталов, а эйконал решать **внутри сектора**.
//! Первое, что для этого нужно, — портал, который знает про уровни.
//!
//! **Детерминизм.** Ничьи разрешаются номером узла, а не порядком
//! вставки в очередь; `HashMap` не используется — только плотные
//! массивы и двоичный поиск.

use crate::volume::{VolField, VolGraph};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Сторона сектора в клетках. Та же, что у плоского графа и у эйконала
/// (ADR-066).
pub const SEC: u32 = 16;

/// Стоимость шага между соседними узлами — **та же, что у поля**.
///
/// Метрика у графа порталов и у эйконала обязана быть одна: цена дойти
/// до выхода сектора становится **засевом** поля внутри него, и если
/// шкалы разные, поле либо не замечает разницы между выходами, либо
/// преувеличивает её (ADR-151).
///
/// Совпадает шкала, но не метрика: здесь манхэттенское расстояние
/// (шаг на соседа), у эйконала — годуновское. По диагонали расхождение
/// доходит до сорока процентов, и это записано как ограничение, а не
/// как мелочь.
pub const STEP: u32 = crate::volume::STEP_COST as u32;

/// Портал — отрезок границы двух секторов, проходимый насквозь.
///
/// Представителем служит узел с наименьшим номером: ничья не должна
/// зависеть от того, с какого конца границу обходили.
#[derive(Clone, Copy, Debug)]
pub struct Portal {
    pub node_a: u32,
    pub node_b: u32,
    pub sector_a: u32,
    pub sector_b: u32,
    /// Сколько узлов в отрезке — по нему видно, узкий проход или широкий.
    pub width: u16,
    /// Устойчивое имя, снятое **тем графом, которым портал нарезан**.
    ///
    /// Считать его потом по новому графу нельзя: номера узлов там уже
    /// другие, и клетка у старого портала вышла бы чужая. Ошибка тихая —
    /// ключ получился бы правдоподобным и мог бы совпасть с чужим.
    key: Key,
}

#[derive(Clone, Copy)]
struct Edge {
    to: u32,
    cost: u32,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct BuildStats {
    pub portals: usize,
    pub edges: usize,
    /// Сколько раз пришлось решать «расстояние внутри сектора».
    pub sector_solves: usize,
    /// Узлов посещено этими решениями — работа, а не таймер.
    pub visited: u64,
    /// Сторон порталов, взятых из прошлого графа без пересчёта.
    pub reused_ports: usize,
    /// Рёбер, взятых оттуда же.
    pub reused_edges: usize,
    /// Секторов пересчитано.
    pub solved_sectors: usize,
}

#[derive(Default)]
pub struct SectorGraph {
    sw: u32,
    sh: u32,
    pub(crate) w: u32,
    pub portals: Vec<Portal>,
    /// CSR: рёбра, сгруппированные по исходному порталу.
    starts: Vec<u32>,
    edges: Vec<Edge>,
    /// CSR: порталы каждого сектора.
    sec_starts: Vec<u32>,
    sec_portals: Vec<u32>,
    /// Сектор каждого узла. Не вычисляется на лету: в горячем цикле
    /// это деление на каждый просмотр соседа.
    node_sector: Vec<u32>,
    /// CSR: узлы каждого сектора, в растровом порядке.
    sec_node_starts: Vec<u32>,
    sec_nodes: Vec<u32>,
    /// Незаконченная пересборка: что переиспользовать, где остановились
    /// и что уже насчитали.
    reuse: Vec<bool>,
    cursor: u32,
    bucket: Vec<Vec<Edge>>,
    prev: PrevGraph,
    dist: Vec<u32>,
    touched: Vec<u32>,
}

/// Прошлый граф, сохранённый на время пересборки.
#[derive(Default)]
struct PrevGraph {
    starts: Vec<u32>,
    edges: Vec<Edge>,
    /// Старый номер портала по новому; `u32::MAX` — портал новый.
    old_of: Vec<u32>,
    /// Новый номер по старому.
    new_of: Vec<u32>,
}

/// Стороны в порядке `volume::SIDES`: -Z, -X, +X, +Z.
const SIDE_PX: usize = 2;
/// Локальное «узла нет» — совпадает с `volume::NO_NODE`.
const NO_NODE_LOCAL: u32 = u32::MAX;
const SIDE_PZ: usize = 3;

/// Устойчивое имя портала: клетка и **номер уровня** с обеих сторон.
///
/// Номера узлов не годятся: они раздаются подряд по клеткам, и стоит
/// одной клетке получить лишний уровень, как всё, что за ней, съезжает.
/// Клетка с номером уровня переживают пересборку там, где сама клетка
/// не менялась, — а переиспользуются ровно такие (ADR-150).
type Key = (u32, u32, u32, u32);

fn key_of(g: &VolGraph, node_a: u32, node_b: u32) -> Key {
    let ca = g.cell_of(node_a as usize);
    let cb = g.cell_of(node_b as usize);
    (
        ca,
        node_a - g.first_node(ca as usize),
        cb,
        node_b - g.first_node(cb as usize),
    )
}

impl SectorGraph {
    pub fn new() -> SectorGraph {
        SectorGraph::default()
    }

    pub fn sectors(&self) -> (u32, u32) {
        (self.sw, self.sh)
    }

    /// Сектор узла.
    #[inline]
    pub fn sector_of(&self, g: &VolGraph, node: usize) -> u32 {
        let cell = g.cell_of(node);
        let (cx, cz) = (cell % self.w, cell / self.w);
        (cz / SEC) * self.sw + cx / SEC
    }

    /// Принадлежность узлов секторам — для решения в пределах зоны.
    pub fn zones(&self) -> &[u32] {
        &self.node_sector
    }

    /// Узлы сектора в растровом порядке.
    pub fn nodes_of(&self, sector: u32) -> &[u32] {
        let s = sector as usize;
        &self.sec_nodes[self.sec_node_starts[s] as usize..self.sec_node_starts[s + 1] as usize]
    }

    /// Разложить узлы по секторам. Проход по клеткам в растровом
    /// порядке, поэтому список каждого сектора тоже растровый.
    fn index_nodes(&mut self, g: &VolGraph) {
        let nsec = (self.sw * self.sh) as usize;
        self.node_sector.clear();
        self.node_sector.resize(g.nodes, u32::MAX);
        let mut cnt = vec![0u32; nsec + 1];
        let (w, h) = g.dims();
        for cz in 0..h {
            for cx in 0..w {
                let cell = (cz * w + cx) as usize;
                let sec = (cz / SEC) * self.sw + cx / SEC;
                let n0 = g.first_node(cell);
                for li in 0..g.levels_in(cell) {
                    self.node_sector[(n0 + li as u32) as usize] = sec;
                }
                cnt[sec as usize + 1] += g.levels_in(cell) as u32;
            }
        }
        for i in 0..nsec {
            cnt[i + 1] += cnt[i];
        }
        let mut at = cnt.clone();
        self.sec_nodes.clear();
        self.sec_nodes.resize(cnt[nsec] as usize, 0);
        for cz in 0..h {
            for cx in 0..w {
                let cell = (cz * w + cx) as usize;
                let sec = ((cz / SEC) * self.sw + cx / SEC) as usize;
                let n0 = g.first_node(cell);
                for li in 0..g.levels_in(cell) {
                    self.sec_nodes[at[sec] as usize] = n0 + li as u32;
                    at[sec] += 1;
                }
            }
        }
        self.sec_node_starts = cnt;
    }

    pub fn portals_of(&self, sector: u32) -> &[u32] {
        let s = sector as usize;
        &self.sec_portals[self.sec_starts[s] as usize..self.sec_starts[s + 1] as usize]
    }

    /// Вершина поиска — **сторона портала**, а не портал.
    ///
    /// Портал соединяет два сектора, и путь через него состоит из двух
    /// разных вещей: пройти сектором до портала и перешагнуть на ту
    /// сторону. Пока сторона была одна на двоих, второе не стоило
    /// ничего, и цена маршрута выходила ниже прямой линии (ADR-149).
    #[inline]
    pub fn port_of(&self, portal: u32, sector: u32) -> u32 {
        let p = &self.portals[portal as usize];
        portal * 2 + u32::from(p.sector_a != sector)
    }

    #[inline]
    pub fn portal_of_port(&self, port: u32) -> u32 {
        port / 2
    }

    /// Узел, которым сторона портала смотрит в свой сектор.
    #[inline]
    pub fn node_of_port(&self, port: u32) -> u32 {
        let p = &self.portals[(port / 2) as usize];
        if port % 2 == 0 {
            p.node_a
        } else {
            p.node_b
        }
    }

    pub fn links_of(&self, port: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        let p = port as usize;
        self.edges[self.starts[p] as usize..self.starts[p + 1] as usize]
            .iter()
            .map(|e| (e.to, e.cost))
    }

    /// Собрать граф по объёмному — целиком.
    pub fn build(&mut self, g: &VolGraph) -> BuildStats {
        let nsec = {
            let (w, h) = g.dims();
            (w.div_ceil(SEC) * h.div_ceil(SEC)) as usize
        };
        self.rebuild(g, &(0..nsec as u32).collect::<Vec<_>>())
    }

    /// Пересобрать, пересчитав **только задетые секторы**.
    ///
    /// Нарезка границ дёшева и делается целиком; дорого — решение
    /// «расстояние внутри сектора» на каждую сторону портала, и вот оно
    /// и переиспользуется.
    ///
    /// **Сектор годится к переиспользованию, если чист он сам и его
    /// набор порталов не изменился.**
    ///
    /// Правило «чист он и все четыре соседа» (так у плоского графа)
    /// на копке не работает вовсе: триста копателей пачкают тридцать
    /// секторов из шестидесяти четырёх за тик, и чистых без грязного
    /// соседа не остаётся. Но грязь у соседа мешает только тогда, когда
    /// она меняет **общую границу**, а это видно прямо: границы режутся
    /// целиком и дёшево, и набор порталов сектора можно просто сравнить
    /// с прошлым (ADR-150).
    pub fn rebuild(&mut self, g: &VolGraph, dirty: &[u32]) -> BuildStats {
        let mut st = self.rebuild_begin(g, dirty);
        while !self.rebuild_step(g, u32::MAX, &mut st) {}
        st
    }

    /// Начать пересборку: нарезать границы и решить, что уцелело.
    ///
    /// Нарезка дёшева и делается целиком; дорого — расстояния внутри
    /// секторов, и вот они считаются порциями (ADR-152).
    pub fn rebuild_begin(&mut self, g: &VolGraph, dirty: &[u32]) -> BuildStats {
        let (w, h) = g.dims();
        let first = self.w == 0;
        self.w = w;
        self.sw = w.div_ceil(SEC);
        self.sh = h.div_ceil(SEC);
        let nsec = (self.sw * self.sh) as usize;

        self.index_nodes(g);

        let mut touched = vec![false; nsec];
        for &d in dirty {
            if (d as usize) < nsec {
                touched[d as usize] = true;
            }
        }
        // Набор порталов каждого сектора **до** пересборки.
        let mut was: Vec<Vec<Key>> = (0..nsec).map(|_| Vec::new()).collect();
        if !first {
            for sc in 0..nsec as u32 {
                let mut v: Vec<Key> = self
                    .portals_of(sc)
                    .iter()
                    .map(|&i| self.portals[i as usize].key)
                    .collect();
                v.sort_unstable();
                was[sc as usize] = v;
            }
        }

        // Соответствие «новый портал — старый» по устойчивому имени.
        // Двоичный поиск, а не словарь: словарь дал бы порядок обхода,
        // зависящий от хеша.
        let mut old_key: Vec<(Key, u32)> = self
            .portals
            .iter()
            .enumerate()
            .map(|(i, p)| (p.key, i as u32))
            .collect();
        old_key.sort_unstable();
        let old_portals = std::mem::take(&mut self.portals);
        let old_starts = std::mem::take(&mut self.starts);
        let old_edges = std::mem::take(&mut self.edges);

        cut_portals(self, g, w, h);
        group(self, nsec);

        let old_of: Vec<u32> = self
            .portals
            .iter()
            .map(|p| {
                match old_key.binary_search_by(|probe| probe.0.cmp(&p.key)) {
                    Ok(i) => old_key[i].1,
                    Err(_) => u32::MAX,
                }
            })
            .collect();

        // Теперь, когда границы нарезаны заново, видно, у кого набор
        // порталов уцелел.
        let mut reuse = vec![false; nsec];
        if !first {
            for sc in 0..nsec as u32 {
                if touched[sc as usize] {
                    continue;
                }
                let mut now: Vec<Key> = self
                    .portals_of(sc)
                    .iter()
                    .map(|&i| self.portals[i as usize].key)
                    .collect();
                now.sort_unstable();
                reuse[sc as usize] = now == was[sc as usize];
            }
        }

        let st = BuildStats { portals: self.portals.len(), ..Default::default() };
        let mut new_of = vec![u32::MAX; old_portals.len()];
        for (n, &o) in old_of.iter().enumerate() {
            if o != u32::MAX {
                new_of[o as usize] = n as u32;
            }
        }
        let _ = old_portals;
        self.prev = PrevGraph {
            starts: old_starts,
            edges: old_edges,
            old_of,
            new_of,
        };
        self.reuse = reuse;
        self.cursor = 0;
        let nport = self.portals.len() * 2;
        self.bucket.clear();
        self.bucket.resize(nport, Vec::new());
        self.dist.clear();
        self.dist.resize(g.nodes, u32::MAX);
        // Переход на ту сторону — обычное ребро в один шаг.
        for pi in 0..self.portals.len() as u32 {
            self.bucket[(pi * 2) as usize].push(Edge { to: pi * 2 + 1, cost: STEP });
            self.bucket[(pi * 2 + 1) as usize].push(Edge { to: pi * 2, cost: STEP });
        }
        st
    }

    /// Досчитать не более `budget` секторов. `true` — граф собран.
    pub fn rebuild_step(&mut self, g: &VolGraph, budget: u32, st: &mut BuildStats) -> bool {
        let nsec = self.sw * self.sh;
        let mut spent = 0u32;
        while self.cursor < nsec {
            if spent >= budget {
                return false;
            }
            let s = self.cursor;
            self.cursor += 1;
            spent += 1;
            solve_sector(self, g, s, st);
        }
        // Корзины раскладываются в CSR один раз, в конце.
        let nport = self.portals.len() * 2;
        let mut starts = vec![0u32; nport + 1];
        let mut edges: Vec<Edge> = Vec::new();
        for i in 0..nport {
            starts[i + 1] = starts[i] + self.bucket[i].len() as u32;
            edges.extend_from_slice(&self.bucket[i]);
        }
        self.starts = starts;
        self.edges = edges;
        st.edges = self.edges.len();
        self.bucket.clear();
        self.prev = PrevGraph::default();
        true
    }
}

/// Нарезать границы секторов на порталы.
///
/// Два перехода через одну границу попадают в один портал, если их узлы
/// со стороны `a` соседствуют **вдоль границы**. Иначе портал был бы
/// на каждый узел, и граф раздулся бы до размеров сетки.
fn cut_portals(sg: &mut SectorGraph, g: &VolGraph, w: u32, h: u32) {
    let cell = |cx: u32, cz: u32| (cz * w + cx) as usize;
    // Вертикальные границы: между столбцами cx-1 и cx, где cx кратно SEC.
    let mut cx = SEC;
    while cx < w {
        let mut cz = 0;
        while cz < h {
            let z_end = (cz / SEC + 1) * SEC;
            let z_end = z_end.min(h);
            run_boundary(sg, g, cx, cz, z_end, true, cell);
            cz = z_end;
        }
        cx += SEC;
    }
    // Горизонтальные: между строками cz-1 и cz.
    let mut cz = SEC;
    while cz < h {
        let mut cx = 0;
        while cx < w {
            let x_end = ((cx / SEC + 1) * SEC).min(w);
            run_boundary(sg, g, cz, cx, x_end, false, cell);
            cx = x_end;
        }
        cz += SEC;
    }
}

/// Один отрезок границы: `at` — координата поперёк, `from..to` — вдоль.
fn run_boundary(
    sg: &mut SectorGraph,
    g: &VolGraph,
    at: u32,
    from: u32,
    to: u32,
    vertical: bool,
    cell: impl Fn(u32, u32) -> usize,
) {
    // Сторона перехода: поперёк границы; сторона соседства: вдоль неё.
    let (cross, along) = if vertical { (SIDE_PX, SIDE_PZ) } else { (SIDE_PZ, SIDE_PX) };
    // Крайние узлы предыдущего шага вдоль границы — чтобы понять,
    // продолжается отрезок или начинается новый.
    let mut prev: Vec<u32> = Vec::new();
    let mut cur: Vec<u32> = Vec::new();
    // Открытый отрезок: представитель и ширина.
    let mut open: Option<(u32, u32, u16)> = None;
    for t in from..to {
        let c = if vertical { cell(at - 1, t) } else { cell(t, at - 1) };
        cur.clear();
        let n0 = g.first_node(c);
        for li in 0..g.levels_in(c) {
            let node = n0 + li as u32;
            let mut nb = g.side(node as usize, cross);
            let Some(other) = nb.next() else { continue };
            // Соседство вдоль границы: продолжает ли этот узел отрезок.
            let joins = prev.iter().any(|&p| {
                g.side(p as usize, along).any(|m| m == node)
            });
            match open {
                Some((a, b, wd)) if joins => open = Some((a.min(node), b, wd + 1)),
                _ => {
                    if let Some((a, b, wd)) = open.take() {
                        push_portal(sg, g, a, b, wd);
                    }
                    open = Some((node, other, 1));
                }
            }
            cur.push(node);
        }
        if cur.is_empty() {
            if let Some((a, b, wd)) = open.take() {
                push_portal(sg, g, a, b, wd);
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    if let Some((a, b, wd)) = open.take() {
        push_portal(sg, g, a, b, wd);
    }
}

fn push_portal(sg: &mut SectorGraph, g: &VolGraph, node_a: u32, node_b: u32, width: u16) {
    // Представителем отрезка служит наименьший его узел, и вторая
    // сторона берётся у него же — иначе пара «представитель и его
    // визави» зависела бы от порядка обхода.
    let sector_a = sg.sector_of(g, node_a as usize);
    let sector_b = sg.sector_of(g, node_b as usize);
    if sector_a == sector_b {
        return;
    }
    let key = key_of(g, node_a, node_b);
    sg.portals.push(Portal { node_a, node_b, sector_a, sector_b, width, key });
}

/// Разложить порталы по секторам (CSR).
fn group(sg: &mut SectorGraph, nsec: usize) {
    let mut cnt = vec![0u32; nsec + 1];
    for p in sg.portals.iter() {
        cnt[p.sector_a as usize + 1] += 1;
        cnt[p.sector_b as usize + 1] += 1;
    }
    for i in 0..nsec {
        cnt[i + 1] += cnt[i];
    }
    let mut out = vec![0u32; cnt[nsec] as usize];
    let mut at = cnt.clone();
    for (i, p) in sg.portals.iter().enumerate() {
        out[at[p.sector_a as usize] as usize] = i as u32;
        at[p.sector_a as usize] += 1;
        out[at[p.sector_b as usize] as usize] = i as u32;
        at[p.sector_b as usize] += 1;
    }
    sg.sec_starts = cnt;
    sg.sec_portals = out;
}

/// Расстояния между сторонами порталов **одного сектора**.
///
/// Рёбер два рода: внутри сектора — дейкстра от стороны к стороне,
/// и через портал — один шаг с одной стороны на другую. Второе
/// добавлено разом в `rebuild_begin`.
fn solve_sector(sg: &mut SectorGraph, g: &VolGraph, s: u32, st: &mut BuildStats) {
    let ps: Vec<u32> = sg.portals_of(s).to_vec();
    if ps.len() < 2 {
        return;
    }
    if sg.reuse[s as usize] && ps.iter().all(|&pi| sg.prev.old_of[pi as usize] != u32::MAX) {
        // Сектор чист и набор порталов тот же: стороны берутся
        // из прошлого графа как есть, с переводом номеров.
        for &pi in ps.iter() {
            let port_i = sg.port_of(pi, s);
            let op = sg.prev.old_of[pi as usize];
            let oport = op * 2 + (port_i % 2);
            let (a, b) = (
                sg.prev.starts[oport as usize] as usize,
                sg.prev.starts[oport as usize + 1] as usize,
            );
            for k in a..b {
                let e = sg.prev.edges[k];
                // Переход на ту сторону уже добавлен.
                if e.to / 2 == op {
                    continue;
                }
                let nt = sg.prev.new_of[(e.to / 2) as usize];
                if nt == u32::MAX {
                    continue;
                }
                sg.bucket[port_i as usize].push(Edge {
                    to: nt * 2 + (e.to % 2),
                    cost: e.cost,
                });
                st.reused_edges += 1;
            }
            st.reused_ports += 1;
        }
        return;
    }
    st.solved_sectors += 1;
    let mut dist = std::mem::take(&mut sg.dist);
    let mut touched = std::mem::take(&mut sg.touched);
    for &pi in ps.iter() {
        let port_i = sg.port_of(pi, s);
        let from = sg.node_of_port(port_i);
        st.sector_solves += 1;
        st.visited += dijkstra_in_sector(sg, g, s, from, &mut dist, &mut touched) as u64;
        for &pj in ps.iter() {
            if pj == pi {
                continue;
            }
            let port_j = sg.port_of(pj, s);
            let d = dist[sg.node_of_port(port_j) as usize];
            if d != u32::MAX {
                sg.bucket[port_i as usize].push(Edge { to: port_j, cost: d });
            }
        }
        for &t in touched.iter() {
            dist[t as usize] = u32::MAX;
        }
        touched.clear();
    }
    sg.dist = dist;
    sg.touched = touched;
}

/// Дейкстра по узлам одного сектора. Возвращает число посещённых узлов.
fn dijkstra_in_sector(
    sg: &SectorGraph,
    g: &VolGraph,
    sector: u32,
    start: u32,
    dist: &mut [u32],
    touched: &mut Vec<u32>,
) -> usize {
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    dist[start as usize] = 0;
    touched.push(start);
    heap.push(Reverse((0, start)));
    let mut visited = 0usize;
    while let Some(Reverse((d, i))) = heap.pop() {
        if d > dist[i as usize] {
            continue;
        }
        visited += 1;
        for side in 0..4 {
            for m in g.side(i as usize, side) {
                if sg.node_sector[m as usize] != sector {
                    continue;
                }
                let nd = d + STEP;
                if nd < dist[m as usize] {
                    if dist[m as usize] == u32::MAX {
                        touched.push(m);
                    }
                    dist[m as usize] = nd;
                    heap.push(Reverse((nd, m)));
                }
            }
        }
    }
    visited
}

/// Переиспользуемые буферы поиска маршрута.
#[derive(Default)]
pub struct RouteWs {
    g: Vec<u32>,
    seen: Vec<u32>,
    came: Vec<u32>,
    epoch: u32,
    dist: Vec<u32>,
    touched: Vec<u32>,
    /// Порталы целевого сектора и цена хвоста до цели.
    ends: Vec<(u32, u32)>,
    /// Цена добраться до цели от каждой стороны портала.
    to_t: Vec<u32>,
    /// Засевы сектора.
    seeds: Vec<(u32, u32)>,
    /// Лучший узел каждой компоненты и цели поля.
    best: Vec<(u64, u32)>,
    goals: Vec<u32>,
    /// Место в обходе секторов; `u32::MAX` — поле досчитано.
    field_at: u32,
    pub field_st: FieldStats,
}

impl RouteWs {
    pub fn new(np: usize, nodes: usize) -> RouteWs {
        RouteWs {
            g: vec![0; np],
            seen: vec![0; np],
            came: vec![u32::MAX; np],
            epoch: 0,
            dist: vec![u32::MAX; nodes],
            touched: Vec::new(),
            ends: Vec::new(),
            to_t: Vec::new(),
            seeds: Vec::new(),
            best: Vec::new(),
            goals: Vec::new(),
            field_at: u32::MAX,
            field_st: FieldStats::default(),
        }
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct RouteStats {
    pub found: bool,
    /// Сторон порталов в маршруте: перешагнуть портал — тоже шаг.
    pub hops: u32,
    /// Сторон снято с очереди.
    pub expanded: u32,
    pub cost: u32,
    /// Ответ дан одним сравнением меток: разные компоненты — пути нет.
    pub by_island: bool,
}

/// Маршрут от узла к узлу по графу порталов.
///
/// **Разные компоненты — пути нет, и это одно сравнение** (ADR-106):
/// перебирать порталы, чтобы выяснить то, что известно из разметки,
/// незачем.
pub fn route(
    sg: &SectorGraph,
    g: &VolGraph,
    ws: &mut RouteWs,
    from: u32,
    to: u32,
) -> RouteStats {
    let mut st = RouteStats::default();
    if from == to {
        st.found = true;
        return st;
    }
    if g.comp_of(from as usize) != g.comp_of(to as usize) {
        st.by_island = true;
        return st;
    }
    let (sa, sb) = (sg.sector_of(g, from as usize), sg.sector_of(g, to as usize));
    if sa == sb {
        // Внутри сектора порталы не нужны: дальше ведёт поле.
        let d = one_to_one(sg, g, sa, from, to, &mut ws.dist, &mut ws.touched);
        st.found = d != u32::MAX;
        st.cost = if st.found { d } else { 0 };
        return st;
    }
    let nport = sg.portals.len() * 2;
    if ws.g.len() < nport {
        *ws = RouteWs::new(nport, g.nodes);
    }

    // **Одна дейкстра на сектор, а не одна на портал.** Расстояния
    // до всех сторон сектора читаются из одного решения; отдельный
    // запуск на каждый портал стоил 130–350 мкс вместо единиц (ADR-149).
    ws.ends.clear();
    dijkstra_in_sector(sg, g, sb, to, &mut ws.dist, &mut ws.touched);
    for &p in sg.portals_of(sb) {
        let port = sg.port_of(p, sb);
        let d = ws.dist[sg.node_of_port(port) as usize];
        if d != u32::MAX {
            ws.ends.push((port, d));
        }
    }
    for &t in ws.touched.iter() {
        ws.dist[t as usize] = u32::MAX;
    }
    ws.touched.clear();
    ws.ends.sort_unstable();
    if ws.ends.is_empty() {
        return st;
    }

    // Оценка снизу — манхэттенское расстояние между клетками. Шаг между
    // соседними клетками стоит `STEP` независимо от перепада, поэтому
    // оценка допустима; перебор она режет вчетверо.
    let tw = sg.w;
    let tcell = g.cell_of(to as usize);
    let (tx, tz) = ((tcell % tw) as i64, (tcell / tw) as i64);
    let hh = |port: u32| -> u32 {
        let c = g.cell_of(sg.node_of_port(port) as usize);
        let (x, z) = ((c % tw) as i64, (c / tw) as i64);
        (((x - tx).abs() + (z - tz).abs()) as u32) * STEP
    };

    ws.epoch = ws.epoch.wrapping_add(1);
    if ws.epoch == 0 {
        ws.seen.iter_mut().for_each(|v| *v = 0);
        ws.epoch = 1;
    }
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    dijkstra_in_sector(sg, g, sa, from, &mut ws.dist, &mut ws.touched);
    for &p in sg.portals_of(sa) {
        let port = sg.port_of(p, sa);
        let d = ws.dist[sg.node_of_port(port) as usize];
        if d == u32::MAX {
            continue;
        }
        ws.g[port as usize] = d;
        ws.seen[port as usize] = ws.epoch;
        ws.came[port as usize] = u32::MAX;
        heap.push(Reverse((d + hh(port), port)));
    }
    for &t in ws.touched.iter() {
        ws.dist[t as usize] = u32::MAX;
    }
    ws.touched.clear();

    let mut best = u32::MAX;
    let mut best_p = u32::MAX;
    while let Some(Reverse((f, port))) = heap.pop() {
        if ws.seen[port as usize] != ws.epoch {
            continue;
        }
        let d = ws.g[port as usize];
        if f > d + hh(port) {
            continue;
        }
        if f >= best {
            // Оценка снизу уже хуже найденного: дальше только хуже.
            break;
        }
        st.expanded += 1;
        if let Ok(k) = ws.ends.binary_search_by(|probe| probe.0.cmp(&port)) {
            let tail = ws.ends[k].1;
            if d + tail < best {
                best = d + tail;
                best_p = port;
            }
        }
        for (q, c) in sg.links_of(port) {
            let nd = d + c;
            if ws.seen[q as usize] != ws.epoch || nd < ws.g[q as usize] {
                ws.seen[q as usize] = ws.epoch;
                ws.g[q as usize] = nd;
                ws.came[q as usize] = port;
                heap.push(Reverse((nd + hh(q), q)));
            }
        }
    }
    if best == u32::MAX {
        return st;
    }
    st.found = true;
    st.cost = best;
    let mut cur = best_p;
    while cur != u32::MAX {
        st.hops += 1;
        cur = ws.came[cur as usize];
    }
    st
}

/// Что стоило одно поле по секторам.
#[derive(Default, Clone, Copy, Debug)]
pub struct FieldStats {
    /// Секторов решено.
    pub sectors: usize,
    /// Годуновских обновлений всего.
    pub updates: u64,
    /// Узлов получило направление.
    pub reached: usize,
    /// Сторон порталов, до которых цель достижима.
    pub live_ports: usize,
}

/// Поле направлений **по секторам**, а не по всей карте.
///
/// Порядок такой (ADR-106, ADR-151):
///
/// 1. Дейкстра по графу порталов **от цели** даёт каждой стороне цену
///    добраться до неё.
/// 2. Внутри каждого сектора эйконал решается с засевом в его выходах:
///    значение засева — та самая цена. Поле получается тем же, что
///    дало бы одно решение на всю карту, но считается посекторно.
///
/// Метрика у обоих шагов одна (`STEP`), иначе поле либо не заметило бы
/// разницы между выходами, либо преувеличило её.
pub fn solve_field<F>(
    sg: &SectorGraph,
    g: &VolGraph,
    f: &mut VolField,
    ws: &mut RouteWs,
    target: u32,
    h: &F,
) -> FieldStats
where
    F: Fn(u32, u32) -> u32,
{
    field_begin(sg, g, ws, target);
    while !field_step(sg, g, f, ws, u32::MAX, h) {}
    ws.field_st
}

/// Шаг нулевой и первый: цели компонент и цена выходов.
///
/// **Приказ называет точку, а не клетку** (ADR-138): компонента,
/// из которой до цели не дойти, идёт к своему узлу, ближайшему
/// к названной точке. Без этого половина карты остаётся без
/// направления и армия там просто стоит.
///
/// Обход стоит не дороже прохода по графу: дейкстра внутри сектора
/// видит только свою компоненту, а их размеры в сумме дают все узлы.
pub fn field_begin(sg: &SectorGraph, g: &VolGraph, ws: &mut RouteWs, target: u32) {
    // Перевыделение стирает всё состояние, поэтому идёт первым:
    // сброшенный после него курсор означал бы поле из нуля секторов.
    let nport = sg.portals.len() * 2;
    if ws.g.len() < nport.max(1) || ws.dist.len() != g.nodes {
        *ws = RouteWs::new(nport.max(1), g.nodes);
    }
    ws.field_st = FieldStats::default();
    ws.field_at = 0;
    ws.to_t.clear();
    ws.to_t.resize(nport, u32::MAX);
    if sg.node_sector.is_empty() || g.nodes == 0 {
        ws.field_at = u32::MAX;
        return;
    }

    let tw = sg.w;
    let tcell = g.cell_of(target as usize);
    let (tx, tz) = ((tcell % tw) as i64, (tcell / tw) as i64);
    ws.best.clear();
    ws.best.resize(g.components.max(1), (u64::MAX, NO_NODE_LOCAL));
    for n in 0..g.nodes {
        let c = g.comp_of(n) as usize;
        let cell = g.cell_of(n);
        let (x, z) = ((cell % tw) as i64, (cell / tw) as i64);
        let d = ((x - tx).pow(2) + (z - tz).pow(2)) as u64;
        // Ничьи разрешаются номером узла, а не порядком обхода.
        if (d, n as u32) < ws.best[c] {
            ws.best[c] = (d, n as u32);
        }
    }
    ws.best[g.comp_of(target as usize) as usize] = (0, target);
    ws.goals.clear();
    for &(_, n) in ws.best.iter() {
        if n != NO_NODE_LOCAL {
            ws.goals.push(n);
        }
    }
    ws.goals.sort_unstable();

    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    for gi in 0..ws.goals.len() {
        let goal = ws.goals[gi];
        let gs = sg.node_sector[goal as usize];
        dijkstra_in_sector(sg, g, gs, goal, &mut ws.dist, &mut ws.touched);
        for &p in sg.portals_of(gs) {
            let port = sg.port_of(p, gs);
            let d = ws.dist[sg.node_of_port(port) as usize];
            if d != u32::MAX && d < ws.to_t[port as usize] {
                ws.to_t[port as usize] = d;
                heap.push(Reverse((d, port)));
            }
        }
        for &t in ws.touched.iter() {
            ws.dist[t as usize] = u32::MAX;
        }
        ws.touched.clear();
    }
    while let Some(Reverse((d, port))) = heap.pop() {
        if d > ws.to_t[port as usize] {
            continue;
        }
        for (q, c) in sg.links_of(port) {
            let nd = d + c;
            if nd < ws.to_t[q as usize] {
                ws.to_t[q as usize] = nd;
                heap.push(Reverse((nd, q)));
            }
        }
    }
    ws.field_st.live_ports = ws.to_t.iter().filter(|&&v| v != u32::MAX).count();
}

/// Шаг второй и третий: развернуть засев внутри секторов и проставить
/// переходы через порталы. Не более `budget` секторов за вызов.
pub fn field_step<F>(
    sg: &SectorGraph,
    g: &VolGraph,
    f: &mut VolField,
    ws: &mut RouteWs,
    budget: u32,
    h: &F,
) -> bool
where
    F: Fn(u32, u32) -> u32,
{
    if ws.field_at == u32::MAX {
        return true;
    }
    let nsec = sg.sw * sg.sh;
    let zones = sg.zones();
    let mut spent = 0u32;
    while ws.field_at < nsec {
        if spent >= budget {
            return false;
        }
        let sec = ws.field_at;
        ws.field_at += 1;
        spent += 1;
        let nodes = sg.nodes_of(sec);
        if nodes.is_empty() {
            continue;
        }
        ws.seeds.clear();
        for &p in sg.portals_of(sec) {
            let port = sg.port_of(p, sec);
            let v = ws.to_t[port as usize];
            if v != u32::MAX {
                ws.seeds.push((sg.node_of_port(port), v));
            }
        }
        // Цели, попавшие в этот сектор, — тоже засев.
        for &goal in ws.goals.iter() {
            if sg.node_sector[goal as usize] == sec {
                ws.seeds.push((goal, 0));
            }
        }
        if ws.seeds.is_empty() {
            // До цели из этого сектора не дойти — направления нет,
            // и это честный ответ, а не пропуск.
            f.clear_zone(nodes);
            continue;
        }
        // Ничьи в засеве разрешаются номером узла, а не порядком порталов.
        ws.seeds.sort_unstable();
        crate::volume::solve_zone(g, f, nodes, zones, sec, &ws.seeds, h);
        ws.field_st.sectors += 1;
        ws.field_st.updates += f.updates;
    }

    // На выходе сектора поле молчит: внутри него идти уже некуда,
    // а перешагнуть портал — это не шаг поля, а шаг графа. Поэтому
    // сторонам, с которых выгодно перейти, направление проставляется
    // отдельно — иначе юнит доходит до границы и встаёт.
    for pi in 0..sg.portals.len() as u32 {
        for side in 0..2u32 {
            let here = pi * 2 + side;
            let there = pi * 2 + (1 - side);
            if ws.to_t[there as usize] >= ws.to_t[here as usize] {
                continue;
            }
            let a = sg.node_of_port(here);
            let b = sg.node_of_port(there);
            for k in 0..4 {
                if g.side(a as usize, k).any(|m| m == b) {
                    f.dir[a as usize] = k as u8;
                    break;
                }
            }
        }
    }
    ws.field_st.reached = f.dir.iter().filter(|&&d| d != 255).count();
    ws.field_at = u32::MAX;
    true
}

/// Расстояние между двумя узлами внутри одного сектора.
fn one_to_one(
    sg: &SectorGraph,
    g: &VolGraph,
    sector: u32,
    from: u32,
    to: u32,
    dist: &mut [u32],
    touched: &mut Vec<u32>,
) -> u32 {
    if from == to {
        return 0;
    }
    dijkstra_in_sector(sg, g, sector, from, dist, touched);
    let d = dist[to as usize];
    for &t in touched.iter() {
        dist[t as usize] = u32::MAX;
    }
    touched.clear();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::NavLayer;
    use crate::godunov::h_exact_floor;
    use crate::volume::NO_NODE;
    use voxel::store::{Mat, AIR};

    /// Земля до `GY`, гряда во всю карту по `x == WALL`, и туннель в ней
    /// при `z` в `[30, 36)` — если он есть.
    struct Ridge {
        tunnel: bool,
    }
    const GY: u32 = 8;
    const WALL: u32 = 32;
    impl voxel::Generator for Ridge {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if y < GY {
                return 2;
            }
            if x.abs_diff(WALL) <= 2 && y < 40 {
                if self.tunnel && (30..36).contains(&z) && (GY..GY + 6).contains(&y) {
                    return AIR;
                }
                return 3;
            }
            AIR
        }
    }

    fn world(tunnel: bool) -> (VolGraph, SectorGraph, NavLayer, BuildStats) {
        let w = voxel::World::new(96, 64, 96, 8192, Box::new(Ridge { tunnel }));
        let nav = NavLayer::new(&w, 4, 2, 1);
        let mut g = VolGraph::new();
        g.build(&nav);
        let mut sg = SectorGraph::new();
        let st = sg.build(&g);
        (g, sg, nav, st)
    }

    fn floor_node(g: &VolGraph, nav: &NavLayer, cx: u32, cz: u32) -> u32 {
        let li = nav
            .levels_at(cx, cz)
            .iter()
            .position(|&l| l as u32 == GY)
            .expect("уровень пола");
        let n = g.node(cx, cz, li);
        assert_ne!(n, NO_NODE, "нет узла в клетке ({cx}, {cz})");
        n
    }

    /// **Порталы находятся, и их меньше, чем узлов на границах.**
    /// Портал на узел означал бы, что нарезка не режет.
    #[test]
    fn boundaries_become_a_handful_of_portals() {
        let (_, _sg, _, st) = world(true);
        assert!(st.portals > 0, "порталов нет вовсе");
        assert!(
            st.portals < 400,
            "порталов {} — граница не склеилась в отрезки",
            st.portals
        );
        assert!(st.edges > 0, "рёбер между порталами нет");
    }

    /// **Маршрут идёт через туннель.** Плоский граф по поверхности его
    /// не видит: над ходом порода.
    #[test]
    fn a_route_goes_through_the_tunnel() {
        let (g, sg, nav, _) = world(true);
        let mut ws = RouteWs::new(sg.portals.len().max(1), g.nodes);
        let from = floor_node(&g, &nav, 4, 16);
        let to = floor_node(&g, &nav, 44, 16);
        let r = route(&sg, &g, &mut ws, from, to);
        assert!(r.found, "маршрут через туннель не найден");
        assert!(r.hops > 0, "маршрут без порталов при разных секторах");
    }

    /// **Без туннеля пути нет, и это видно одним сравнением меток.**
    #[test]
    fn a_solid_ridge_answers_by_island() {
        let (g, sg, nav, _) = world(false);
        let mut ws = RouteWs::new(sg.portals.len().max(1), g.nodes);
        let from = floor_node(&g, &nav, 4, 16);
        let to = floor_node(&g, &nav, 44, 16);
        let r = route(&sg, &g, &mut ws, from, to);
        assert!(!r.found, "маршрут нашёлся сквозь сплошную гряду");
        assert!(r.by_island, "ответ дан перебором, а не меткой компоненты");
        assert_eq!(r.expanded, 0, "перебор всё-таки был");
    }

    /// **Маршрут симметричен.** Рёбра двусторонние, значит и цена
    /// в обе стороны одна. Недопустимая оценка снизу у A\* ломает это
    /// первым делом.
    #[test]
    fn a_route_costs_the_same_both_ways() {
        let (g, sg, nav, _) = world(true);
        let mut ws = RouteWs::new(sg.portals.len().max(1), g.nodes);
        let a = floor_node(&g, &nav, 4, 16);
        let b = floor_node(&g, &nav, 44, 16);
        let ab = route(&sg, &g, &mut ws, a, b);
        let ba = route(&sg, &g, &mut ws, b, a);
        assert!(ab.found && ba.found);
        assert_eq!(ab.cost, ba.cost, "цена туда и обратно разная");
    }

    /// **Цена не ниже манхэттенского расстояния.** Шаг между соседними
    /// клетками стоит `STEP`, значит короче прямой пути нет.
    #[test]
    fn a_route_is_never_shorter_than_the_straight_line() {
        let (g, sg, nav, _) = world(true);
        let mut ws = RouteWs::new(sg.portals.len().max(1), g.nodes);
        let a = floor_node(&g, &nav, 4, 16);
        let b = floor_node(&g, &nav, 44, 16);
        let r = route(&sg, &g, &mut ws, a, b);
        let (w, _) = nav.dims();
        let (ca, cb) = (g.cell_of(a as usize), g.cell_of(b as usize));
        let d = (ca % w).abs_diff(cb % w) + (ca / w).abs_diff(cb / w);
        assert!(r.found);
        assert!(r.cost >= d * STEP, "цена {} меньше прямой {}", r.cost, d * STEP);
    }

    /// **Инкрементальная пересборка обязана дать то же, что полная.**
    ///
    /// Это единственная проверка, которая держит переиспользование
    /// честным: секторы, взятые из прошлого графа, не имеют права
    /// отличаться от пересчитанных ни одним ребром (ADR-150).
    #[test]
    fn a_rebuild_matches_a_full_build() {
        let mut w = voxel::World::new(96, 64, 96, 100_000, Box::new(Ridge { tunnel: true }));
        let mut nav = NavLayer::new(&w, 4, 2, 1);
        let mut g = VolGraph::new();
        g.build(&nav);
        let mut inc = SectorGraph::new();
        inc.build(&g);

        // Копаем яму в одном секторе и отмечаем его грязным.
        let (dx, dz) = (70u32, 70u32);
        for y in GY - 4..GY {
            for z in dz..dz + 6 {
                for x in dx..dx + 6 {
                    w.queue_dig(x, y, z);
                }
            }
        }
        let edits = w.flush();
        assert!(edits.applied > 0, "копка ничего не сняла");
        nav.apply(&w, w.edits());
        g.build(&nav);

        let (sw, _) = inc.sectors();
        let cell = (dz / 2) * (nav.dims().0) + dx / 2;
        let sector = (cell / nav.dims().0 / SEC) * sw + (cell % nav.dims().0) / SEC;
        let st = inc.rebuild(&g, &[sector]);
        assert!(st.reused_ports > 0, "не переиспользовано ничего");
        assert!(st.solved_sectors > 0, "не пересчитано ничего");

        let mut full = SectorGraph::new();
        full.build(&g);

        assert_eq!(dump(&inc), dump(&full), "инкремент разошёлся с полной сборкой");
    }

    /// Ребро в каноническом виде: имя портала, сторона, цена.
    type DumpEdge = (Key, u32, u32);

    /// Канонический вид графа: имена порталов и рёбра по ним.
    fn dump(sg: &SectorGraph) -> Vec<(Key, Vec<DumpEdge>)> {
        let mut out: Vec<(Key, Vec<DumpEdge>)> = Vec::new();
        for (i, p) in sg.portals.iter().enumerate() {
            let k = p.key;
            for side in 0..2u32 {
                let port = i as u32 * 2 + side;
                let mut es: Vec<DumpEdge> = sg
                    .links_of(port)
                    .map(|(q, c)| (sg.portals[(q / 2) as usize].key, q % 2, c))
                    .collect();
                es.sort_unstable();
                out.push(((k.0, k.1 + side * 1_000_000, k.2, k.3), es));
            }
        }
        out.sort_unstable();
        out
    }

    /// **По полю можно дойти до цели.** Из каждого узла, у которого есть
    /// направление, шаг за шагом приходишь в цель — без обрывов и петель.
    ///
    /// Это и есть проверка всей связки: дейкстра по порталам дала цену
    /// выходам, эйконал развернул её внутри секторов, а переход через
    /// портал проставлен отдельно (ADR-151). Ошибка в любом из трёх
    /// мест кончается обрывом на границе или петлёй.
    #[test]
    fn following_the_field_reaches_the_target() {
        let (g, sg, nav, _) = world(true);
        let mut f = VolField::new();
        let mut ws = RouteWs::new(sg.portals.len().max(1), g.nodes);
        let target = floor_node(&g, &nav, 44, 16);
        let st = solve_field(&sg, &g, &mut f, &mut ws, target, &h_exact_floor);
        assert!(st.sectors > 1, "секторов решено {}", st.sectors);
        assert!(st.reached > 100, "направлений всего {}", st.reached);

        let tc = g.comp_of(target as usize);
        let (mut checked, mut to_target) = (0usize, 0usize);
        for n in 0..g.nodes {
            if f.dir[n] == 255 {
                continue;
            }
            let mut cur = n as u32;
            let mut steps = 0;
            while f.dir[cur as usize] != 255 {
                let d = f.dir[cur as usize];
                let next = g
                    .side(cur as usize, d as usize)
                    .min_by_key(|&m| (f.t[m as usize], m))
                    .expect("сторона названа, а соседа нет");
                assert_eq!(
                    g.comp_of(next as usize),
                    g.comp_of(cur as usize),
                    "поле увело в другую компоненту"
                );
                cur = next;
                steps += 1;
                assert!(steps < 20_000, "поле зациклилось на пути из {n}");
            }
            // Пришли туда, где идти дальше некуда, — в цель своей
            // компоненты. У компоненты цели это сама цель.
            if g.comp_of(n) == tc {
                assert_eq!(cur, target, "из {n} пришли не в цель, а в {cur}");
                to_target += 1;
            }
            checked += 1;
        }
        assert!(checked > 100, "проверено всего {checked} узлов");
        assert!(to_target > 50, "в цель пришло всего {to_target}");
    }

    /// **Порционная пересборка равна разовой.** Потолок работы за тик
    /// не имеет права менять результат (ADR-152).
    #[test]
    fn a_budgeted_rebuild_matches_a_whole_one() {
        let (g, _, _, _) = world(true);
        let mut whole = SectorGraph::new();
        whole.build(&g);

        let mut piece = SectorGraph::new();
        let mut st = piece.rebuild_begin(&g, &[]);
        let mut rounds = 0;
        while !piece.rebuild_step(&g, 3, &mut st) {
            rounds += 1;
            assert!(rounds < 10_000, "порционная сборка не сошлась");
        }
        assert!(rounds > 1, "порция оказалась не порцией");
        assert_eq!(dump(&piece), dump(&whole), "порции разошлись с разовой сборкой");
    }

    /// **Сборка детерминирована**: два прогона дают тот же граф.
    #[test]
    fn the_build_is_deterministic() {
        let (_, a, _, sa) = world(true);
        let (_, b, _, sb) = world(true);
        assert_eq!(sa.portals, sb.portals);
        assert_eq!(sa.edges, sb.edges);
        let pa: Vec<(u32, u32)> = a.portals.iter().map(|p| (p.node_a, p.node_b)).collect();
        let pb: Vec<(u32, u32)> = b.portals.iter().map(|p| (p.node_a, p.node_b)).collect();
        assert_eq!(pa, pb, "порталы разошлись между прогонами");
    }
}
