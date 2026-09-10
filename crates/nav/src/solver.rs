//! Решатели уравнения эйконала на сетке. Все считают одно и то же поле;
//! отличаются порядком обхода и тем, сколько обновлений ячейки на это тратят.
//!
//! Единица учёта — **одно вычисление годуновского обновления**, а не проход.
//! Проходами Якоби и итерациями LSM меряют разное, и сравнение по ним — это
//! ровно та ошибка, из-за которой в бюджете навигации семь лет жила
//! семикратная вилка.

use crate::godunov::{update2d, Cost, Time, BLOCKED, INF};
use crate::scene::Grid;
use std::collections::BinaryHeap;

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Сколько раз посчитано годуновское обновление.
    pub updates: u64,
    /// Сколько проходов/раундов сделал решатель.
    pub passes: u32,
    /// Сколько ячеек получило конечное значение.
    pub reachable: usize,
}

impl Stats {
    pub fn per_cell_permille(&self) -> u64 {
        if self.reachable == 0 {
            0
        } else {
            self.updates * 1000 / self.reachable as u64
        }
    }
}

#[inline(always)]
fn axis_mins(t: &[Time], w: usize, i: usize) -> (Time, Time) {
    let a = t[i - 1].min(t[i + 1]);
    let b = t[i - w].min(t[i + w]);
    (a, b)
}

fn init(g: &Grid, goals: &[usize]) -> Vec<Time> {
    let mut t = vec![INF; g.w * g.h];
    for &s in goals {
        debug_assert!(g.cost[s] != BLOCKED, "цель на непроходимой ячейке");
        t[s] = 0;
    }
    t
}

fn count_reachable(t: &[Time]) -> usize {
    t.iter().filter(|&&v| v != INF).count()
}

/// Обход по порядку растра, на месте. Это то, что обычно называют «Якоби»
/// в игровой литературе, хотя настоящий Якоби — с двойным буфером.
pub fn gauss_seidel<F>(g: &Grid, goals: &[usize], h: &F) -> (Vec<Time>, Stats)
where
    F: Fn(u32, u32) -> u32,
{
    let mut t = init(g, goals);
    let mut st = Stats::default();
    loop {
        let mut changed = false;
        for y in 1..g.h - 1 {
            for x in 1..g.w - 1 {
                let i = y * g.w + x;
                let f = g.cost[i];
                if f == BLOCKED || t[i] == 0 {
                    continue;
                }
                let (a, b) = axis_mins(&t, g.w, i);
                st.updates += 1;
                let q = update2d(a, b, f, h);
                if q < t[i] {
                    t[i] = q;
                    changed = true;
                }
            }
        }
        st.passes += 1;
        if !changed {
            break;
        }
    }
    st.reachable = count_reachable(&t);
    (t, st)
}

/// Настоящий Якоби: чтение из прошлого буфера. Именно он стоит двух мегабайт
/// копий за тик, от которых отказались в ADR-058.
pub fn jacobi<F>(g: &Grid, goals: &[usize], h: &F) -> (Vec<Time>, Stats)
where
    F: Fn(u32, u32) -> u32,
{
    let mut t = init(g, goals);
    let mut next = t.clone();
    let mut st = Stats::default();
    loop {
        let mut changed = false;
        for y in 1..g.h - 1 {
            for x in 1..g.w - 1 {
                let i = y * g.w + x;
                let f = g.cost[i];
                if f == BLOCKED || t[i] == 0 {
                    continue;
                }
                let (a, b) = axis_mins(&t, g.w, i);
                st.updates += 1;
                let q = update2d(a, b, f, h);
                if q < t[i] {
                    next[i] = q;
                    changed = true;
                }
            }
        }
        t.copy_from_slice(&next);
        st.passes += 1;
        if !changed {
            break;
        }
    }
    st.reachable = count_reachable(&t);
    (t, st)
}

const DIRS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

/// Быстрые развёртки: четыре направления обхода по кругу, на месте.
pub fn fsm<F>(g: &Grid, goals: &[usize], h: &F) -> (Vec<Time>, Stats)
where
    F: Fn(u32, u32) -> u32,
{
    let mut t = init(g, goals);
    let mut st = Stats::default();
    let mut round_changed;
    loop {
        round_changed = false;
        for &(rx, ry) in DIRS.iter() {
            for yk in 1..g.h - 1 {
                let y = if ry { g.h - 1 - yk } else { yk };
                for xk in 1..g.w - 1 {
                    let x = if rx { g.w - 1 - xk } else { xk };
                    let i = y * g.w + x;
                    let f = g.cost[i];
                    if f == BLOCKED || t[i] == 0 {
                        continue;
                    }
                    let (a, b) = axis_mins(&t, g.w, i);
                    st.updates += 1;
                    let q = update2d(a, b, f, h);
                    if q < t[i] {
                        t[i] = q;
                        round_changed = true;
                    }
                }
            }
            st.passes += 1;
        }
        if !round_changed {
            break;
        }
    }
    st.reachable = count_reachable(&t);
    (t, st)
}

/// Развёртки с замком: сошедшиеся ячейки пропускаются, пока сосед не сдвинулся.
/// Это тот самый LSM из ADR-058.
pub fn lsm<F>(g: &Grid, goals: &[usize], h: &F) -> (Vec<Time>, Stats)
where
    F: Fn(u32, u32) -> u32,
{
    let mut t = init(g, goals);
    let mut st = Stats::default();
    let mut active = vec![false; g.w * g.h];
    // Активны соседи целей.
    for &s in goals {
        for nb in [s - 1, s + 1, s - g.w, s + g.w] {
            if g.cost[nb] != BLOCKED {
                active[nb] = true;
            }
        }
    }
    loop {
        let mut round_changed = false;
        for &(rx, ry) in DIRS.iter() {
            for yk in 1..g.h - 1 {
                let y = if ry { g.h - 1 - yk } else { yk };
                for xk in 1..g.w - 1 {
                    let x = if rx { g.w - 1 - xk } else { xk };
                    let i = y * g.w + x;
                    if !active[i] {
                        continue;
                    }
                    let f = g.cost[i];
                    if f == BLOCKED || t[i] == 0 {
                        active[i] = false;
                        continue;
                    }
                    let (a, b) = axis_mins(&t, g.w, i);
                    st.updates += 1;
                    let q = update2d(a, b, f, h);
                    if q < t[i] {
                        t[i] = q;
                        round_changed = true;
                        // Активация действует **в этой же развёртке**:
                        // иначе фронт не может пройти вдоль направления
                        // обхода за один проход, и весь смысл развёрток
                        // теряется.
                        for nb in [i - 1, i + 1, i - g.w, i + g.w] {
                            if g.cost[nb] != BLOCKED && t[nb] != 0 {
                                active[nb] = true;
                            }
                        }
                    } else {
                        active[i] = false;
                    }
                }
            }
            st.passes += 1;
        }
        if !round_changed {
            break;
        }
    }
    st.reachable = count_reachable(&t);
    (t, st)
}

/// Быстрый итеративный метод: узкая полоса списком, без сортировки.
pub fn fim<F>(g: &Grid, goals: &[usize], h: &F) -> (Vec<Time>, Stats)
where
    F: Fn(u32, u32) -> u32,
{
    let mut t = init(g, goals);
    let mut st = Stats::default();
    let n = g.w * g.h;
    let mut in_list = vec![false; n];
    let mut list: Vec<usize> = Vec::with_capacity(n / 4);
    for &s in goals {
        for nb in [s - 1, s + 1, s - g.w, s + g.w] {
            if g.cost[nb] != BLOCKED && !in_list[nb] {
                in_list[nb] = true;
                list.push(nb);
            }
        }
    }
    let mut next: Vec<usize> = Vec::with_capacity(n / 4);
    while !list.is_empty() {
        next.clear();
        for &i in list.iter() {
            let f = g.cost[i];
            let (a, b) = axis_mins(&t, g.w, i);
            st.updates += 1;
            let q = update2d(a, b, f, h);
            let old = t[i];
            if q < old {
                t[i] = q;
                next.push(i);
                continue;
            }
            // Сошлась: снимаем с полосы и пробуем расширить её соседями.
            in_list[i] = false;
            for nb in [i - 1, i + 1, i - g.w, i + g.w] {
                if g.cost[nb] == BLOCKED || in_list[nb] || t[nb] == 0 {
                    continue;
                }
                let (na, nbm) = axis_mins(&t, g.w, nb);
                st.updates += 1;
                let qn = update2d(na, nbm, g.cost[nb], h);
                if qn < t[nb] {
                    t[nb] = qn;
                    in_list[nb] = true;
                    next.push(nb);
                }
            }
        }
        std::mem::swap(&mut list, &mut next);
        st.passes += 1;
    }
    st.reachable = count_reachable(&t);
    (t, st)
}

/// Быстрый маршевый метод (Дейкстра с кучей) — эталон по числу обновлений.
/// В симуляции он неприемлем: порядок обхода наблюдаем через кучу, и на
/// ничьих порядок зависит от реализации кучи.
pub fn fmm<F>(g: &Grid, goals: &[usize], h: &F) -> (Vec<Time>, Stats)
where
    F: Fn(u32, u32) -> u32,
{
    let mut t = init(g, goals);
    let mut st = Stats::default();
    let mut frozen = vec![false; g.w * g.h];
    let mut heap: BinaryHeap<(std::cmp::Reverse<Time>, std::cmp::Reverse<usize>)> =
        BinaryHeap::new();
    for &s in goals {
        heap.push((std::cmp::Reverse(0), std::cmp::Reverse(s)));
    }
    while let Some((std::cmp::Reverse(tv), std::cmp::Reverse(i))) = heap.pop() {
        if frozen[i] || tv > t[i] {
            continue;
        }
        frozen[i] = true;
        st.passes += 1;
        for nb in [i - 1, i + 1, i - g.w, i + g.w] {
            if g.cost[nb] == BLOCKED || frozen[nb] {
                continue;
            }
            let (a, b) = axis_mins(&t, g.w, nb);
            st.updates += 1;
            let q = update2d(a, b, g.cost[nb], h);
            if q < t[nb] {
                t[nb] = q;
                heap.push((std::cmp::Reverse(q), std::cmp::Reverse(nb)));
            }
        }
    }
    st.reachable = count_reachable(&t);
    (t, st)
}

/// Хаотическая итерация: обход в произвольном порядке, произвольными кусками.
/// Существует ровно чтобы проверить утверждение о порядок-независимости —
/// в бюджете его нет.
pub fn chaotic<F>(g: &Grid, goals: &[usize], h: &F, seed: u64, chunk: usize) -> Vec<Time>
where
    F: Fn(u32, u32) -> u32,
{
    let mut t = init(g, goals);
    let mut cells: Vec<usize> = Vec::new();
    for y in 1..g.h - 1 {
        for x in 1..g.w - 1 {
            let i = y * g.w + x;
            if g.cost[i] != BLOCKED {
                cells.push(i);
            }
        }
    }
    let mut r = fx::rng::Rng::new(seed, 0xC0FFEE);
    // Тасование Фишера-Йетса.
    for k in (1..cells.len()).rev() {
        let j = r.below(k as u32 + 1) as usize;
        cells.swap(k, j);
    }
    loop {
        let mut changed = false;
        for block in cells.chunks(chunk.max(1)) {
            for &i in block {
                if t[i] == 0 {
                    continue;
                }
                let (a, b) = axis_mins(&t, g.w, i);
                let q = update2d(a, b, g.cost[i], h);
                if q < t[i] {
                    t[i] = q;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    t
}

// ---------------------------------------------------------------------------
// Направление потока: то, что реально читает юнит.
// ---------------------------------------------------------------------------

/// Направление на соседа с наименьшим значением. Ничьи разрешаются
/// **линейным индексом** (`CLAUDE.md`), а не порядком обхода.
pub fn flow_dirs(g: &Grid, t: &[Time]) -> Vec<u8> {
    let mut d = vec![255u8; g.w * g.h];
    for y in 1..g.h - 1 {
        for x in 1..g.w - 1 {
            let i = y * g.w + x;
            if g.cost[i] == BLOCKED || t[i] == INF {
                continue;
            }
            let nbs = [i - g.w, i - 1, i + 1, i + g.w];
            let mut best = t[i];
            let mut best_k = 255u8;
            let mut best_idx = usize::MAX;
            for (k, &nb) in nbs.iter().enumerate() {
                if g.cost[nb] == BLOCKED || t[nb] == INF {
                    continue;
                }
                if t[nb] < best || (t[nb] == best && nb < best_idx) {
                    best = t[nb];
                    best_k = k as u8;
                    best_idx = nb;
                }
            }
            d[i] = best_k;
        }
    }
    d
}

/// Сколько ячеек имеют ровно одного строго лучшего соседа, а сколько —
/// ничью. На целочисленной сетке второе не редкость, и это прямой риск.
pub fn tie_fraction(g: &Grid, t: &[Time]) -> (usize, usize) {
    let mut ties = 0;
    let mut total = 0;
    for y in 1..g.h - 1 {
        for x in 1..g.w - 1 {
            let i = y * g.w + x;
            if g.cost[i] == BLOCKED || t[i] == INF || t[i] == 0 {
                continue;
            }
            total += 1;
            let nbs = [i - g.w, i - 1, i + 1, i + g.w];
            let mut best = Time::MAX;
            let mut cnt = 0;
            for &nb in nbs.iter() {
                if g.cost[nb] == BLOCKED || t[nb] == INF {
                    continue;
                }
                if t[nb] < best {
                    best = t[nb];
                    cnt = 1;
                } else if t[nb] == best {
                    cnt += 1;
                }
            }
            if cnt > 1 {
                ties += 1;
            }
        }
    }
    (ties, total)
}

pub fn max_finite(t: &[Time]) -> Time {
    t.iter().copied().filter(|&v| v != INF).max().unwrap_or(0)
}

pub fn scale_costs(g: &Grid, s: Cost) -> Grid {
    let mut out = g.clone();
    for c in out.cost.iter_mut() {
        if *c != BLOCKED {
            *c *= s;
        }
    }
    out
}

/// Различие направлений между грубым и эталонным полем, разложенное на два:
/// - `differs` — направление просто другое;
/// - `harmful` — направление ведёт к соседу, который в **эталоне** строго хуже
///   минимального, то есть это не ничья, а настоящая ошибка.
///
/// Разделять обязательно: на целочисленной сетке ничьи массовы, и выбор
/// другого из двух одинаково коротких путей ошибкой не является.
pub fn dir_quality(g: &Grid, t_ref: &[Time], t_coarse: &[Time]) -> (u64, u64) {
    let d_ref = flow_dirs(g, t_ref);
    let d_coarse = flow_dirs(g, t_coarse);
    let mut differs = 0usize;
    let mut harmful = 0usize;
    let mut total = 0usize;
    for y in 1..g.h - 1 {
        for x in 1..g.w - 1 {
            let i = y * g.w + x;
            if g.cost[i] == BLOCKED || t_ref[i] == INF || t_ref[i] == 0 {
                continue;
            }
            total += 1;
            if d_coarse[i] == d_ref[i] {
                continue;
            }
            differs += 1;
            let nbs = [i - g.w, i - 1, i + 1, i + g.w];
            let mut best = Time::MAX;
            for &nb in nbs.iter() {
                if g.cost[nb] != BLOCKED && t_ref[nb] < best {
                    best = t_ref[nb];
                }
            }
            let k = d_coarse[i];
            if k == 255 {
                harmful += 1;
                continue;
            }
            let nb = nbs[k as usize];
            if g.cost[nb] == BLOCKED || t_ref[nb] != best {
                harmful += 1;
            }
        }
    }
    // Доля в промилле: метрика входит в отчёт стенда, а стенд читает
    // тот же крейт, что симуляция, и плавающего здесь быть не может.
    let tot = total.max(1) as u64;
    (differs as u64 * 1000 / tot, harmful as u64 * 1000 / tot)
}

/// Переиспользуемые буферы. Существуют, потому что на секторе 16x16 выделение
/// памяти сравнимо с самим решением, и замер без них меряет аллокатор.
pub struct Workspace {
    pub t: Vec<Time>,
    active: Vec<bool>,
    in_list: Vec<bool>,
    fixed: Vec<bool>,
    list: Vec<usize>,
    next: Vec<usize>,
}

impl Workspace {
    pub fn new(n: usize) -> Workspace {
        Workspace {
            t: vec![INF; n],
            active: vec![false; n],
            in_list: vec![false; n],
            fixed: vec![false; n],
            list: Vec::with_capacity(n),
            next: Vec::with_capacity(n),
        }
    }

    /// Засев произвольными значениями, а не только нулями: пересчёт сектора
    /// в архитектуре стартует от граничных значений соседних секторов,
    /// а не от цели внутри.
    fn reset(&mut self, goals: &[(usize, Time)]) {
        self.t.fill(INF);
        self.active.fill(false);
        self.in_list.fill(false);
        self.fixed.fill(false);
        self.list.clear();
        self.next.clear();
        for &(i, v) in goals {
            self.t[i] = v;
            self.fixed[i] = true;
        }
    }
}

/// LSM на переиспользуемых буферах.
pub fn lsm_ws<F>(ws: &mut Workspace, g: &Grid, goals: &[(usize, Time)], h: &F) -> Stats
where
    F: Fn(u32, u32) -> u32,
{
    ws.reset(goals);
    let mut st = Stats::default();
    for &(s, _) in goals {
        for nb in [s - 1, s + 1, s - g.w, s + g.w] {
            if g.cost[nb] != BLOCKED {
                ws.active[nb] = true;
            }
        }
    }
    loop {
        let mut round_changed = false;
        for &(rx, ry) in DIRS.iter() {
            for yk in 1..g.h - 1 {
                let y = if ry { g.h - 1 - yk } else { yk };
                for xk in 1..g.w - 1 {
                    let x = if rx { g.w - 1 - xk } else { xk };
                    let i = y * g.w + x;
                    if !ws.active[i] {
                        continue;
                    }
                    let f = g.cost[i];
                    if f == BLOCKED || ws.fixed[i] {
                        ws.active[i] = false;
                        continue;
                    }
                    let (a, b) = axis_mins(&ws.t, g.w, i);
                    st.updates += 1;
                    let q = update2d(a, b, f, h);
                    if q < ws.t[i] {
                        ws.t[i] = q;
                        round_changed = true;
                        for nb in [i - 1, i + 1, i - g.w, i + g.w] {
                            if g.cost[nb] != BLOCKED && !ws.fixed[nb] {
                                ws.active[nb] = true;
                            }
                        }
                    } else {
                        ws.active[i] = false;
                    }
                }
            }
            st.passes += 1;
        }
        if !round_changed {
            break;
        }
    }
    st.reachable = count_reachable(&ws.t);
    st
}

/// FIM на переиспользуемых буферах.
pub fn fim_ws<F>(ws: &mut Workspace, g: &Grid, goals: &[(usize, Time)], h: &F) -> Stats
where
    F: Fn(u32, u32) -> u32,
{
    ws.reset(goals);
    let mut st = Stats::default();
    for &(s, _) in goals {
        for nb in [s - 1, s + 1, s - g.w, s + g.w] {
            if g.cost[nb] != BLOCKED && !ws.in_list[nb] {
                ws.in_list[nb] = true;
                ws.list.push(nb);
            }
        }
    }
    while !ws.list.is_empty() {
        ws.next.clear();
        for k in 0..ws.list.len() {
            let i = ws.list[k];
            let (a, b) = axis_mins(&ws.t, g.w, i);
            st.updates += 1;
            let q = update2d(a, b, g.cost[i], h);
            if q < ws.t[i] {
                ws.t[i] = q;
                ws.next.push(i);
                continue;
            }
            ws.in_list[i] = false;
            for nb in [i - 1, i + 1, i - g.w, i + g.w] {
                if g.cost[nb] == BLOCKED || ws.in_list[nb] || ws.fixed[nb] {
                    continue;
                }
                let (na, nbm) = axis_mins(&ws.t, g.w, nb);
                st.updates += 1;
                let qn = update2d(na, nbm, g.cost[nb], h);
                if qn < ws.t[nb] {
                    ws.t[nb] = qn;
                    ws.in_list[nb] = true;
                    ws.next.push(nb);
                }
            }
        }
        std::mem::swap(&mut ws.list, &mut ws.next);
        st.passes += 1;
    }
    st.reachable = count_reachable(&ws.t);
    st
}
