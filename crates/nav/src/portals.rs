//! Граф порталов: персональный маршрут юнита, у которого своя цель (Q-63).
//!
//! Flow field строится **на маршрут** и делится всеми, кто по нему идёт
//! (ADR-096). Юниту с собственной целью поле не подходит: пять тысяч
//! целей это пять тысяч полей.
//!
//! Ответ — иерархия: сетка режется на секторы, границы между соседними
//! секторами дают **порталы**, и маршрут ищется по графу порталов,
//! а не по клеткам. Внутри сектора юнит идёт локально.
//!
//! **Детерминизм.** Ничьи в очереди разрешаются индексом узла, а не
//! порядком вставки; `HashMap` не используется нигде — только плотные
//! массивы. Это те же правила, что у всего остального в `sim-core`.

use crate::godunov::BLOCKED;
use crate::scene::Grid;

/// Сторона сектора в клетках. Та же, что у эйконала (ADR-066).
pub const SEC: usize = 16;

/// Портал — непрерывный отрезок границы между двумя секторами, где
/// проходимо с обеих сторон. Узлом графа служит его середина.
#[derive(Clone, Copy, Debug)]
pub struct Portal {
    /// Клетка-представитель со стороны сектора `a`.
    pub cell_a: u32,
    /// Она же со стороны сектора `b`.
    pub cell_b: u32,
    pub sector_a: u32,
    pub sector_b: u32,
    /// Ширина отрезка в клетках — по ней видно, узкий проход или широкий.
    pub width: u16,
}

/// Ребро внутри сектора: из портала `from` в портал `to` за `cost`.
#[derive(Clone, Copy)]
struct Edge {
    to: u32,
    cost: u32,
}

pub struct PortalGraph {
    pub portals: Vec<Portal>,
    /// CSR: рёбра, сгруппированные по исходному порталу.
    starts: Vec<u32>,
    edges: Vec<Edge>,
    /// Порталы каждого сектора, тоже CSR.
    sec_starts: Vec<u32>,
    sec_portals: Vec<u32>,
    pub sx: usize,
    pub sy: usize,
    w: usize,
}

/// Сколько работы стоила сборка — чтобы бенчмарк мерил не таймер, а объём.
#[derive(Default, Clone, Copy, Debug)]
pub struct BuildStats {
    pub portals: usize,
    pub edges: usize,
    /// Сколько раз пришлось решать «расстояние внутри сектора».
    pub sector_solves: usize,
}

impl PortalGraph {
    pub fn build(g: &Grid) -> (PortalGraph, BuildStats) {
        let (w, h) = (g.w, g.h);
        // Секторы нарезаются по **внутренней** области: рамка сетки
        // непроходима и в секторы не входит.
        let sx = (w - 2).div_ceil(SEC);
        let sy = (h - 2).div_ceil(SEC);
        let mut st = BuildStats::default();
        let mut portals: Vec<Portal> = Vec::new();

        let sec_of = |x: usize, y: usize| -> u32 { (((y - 1) / SEC) * sx + (x - 1) / SEC) as u32 };

        // Вертикальные границы: между колонкой x и x+1, где они в разных
        // секторах.
        for sxi in 0..sx.saturating_sub(1) {
            let x = 1 + sxi * SEC + SEC - 1;
            if x + 1 >= w - 1 {
                continue;
            }
            for syi in 0..sy {
                let y0 = 1 + syi * SEC;
                let y1 = (y0 + SEC).min(h - 1);
                let mut run_start: Option<usize> = None;
                for y in y0..=y1 {
                    // Портал — не «обе клетки проходимы», а «между ними
                    // есть переход»: непосильный перепад ребра не даёт
                    // (ADR-121).
                    let open = y < y1 && g.linked(y * w + x, y * w + x + 1);
                    // Непрерывность отрезка считается по **обеим** осям:
                    // через шов и вдоль него. Иначе прокопанный у земли
                    // коридор и гребень над ним склеиваются в один отрезок
                    // — пары через шов связаны обе, — и представителем
                    // оказывается середина, то есть гребень, до которого
                    // с земли не дойти.
                    let joins = open
                        && run_start.is_some()
                        && g.linked((y - 1) * w + x, y * w + x)
                        && g.linked((y - 1) * w + x + 1, y * w + x + 1);
                    let close = run_start.is_some() && (!open || !joins);
                    if close {
                        let s = run_start.take().expect("проверено");
                        let mid = (s + y - 1) / 2;
                        portals.push(Portal {
                            cell_a: (mid * w + x) as u32,
                            cell_b: (mid * w + x + 1) as u32,
                            sector_a: sec_of(x, mid),
                            sector_b: sec_of(x + 1, mid),
                            width: (y - s) as u16,
                        });
                    }
                    if open && run_start.is_none() {
                        run_start = Some(y);
                    }
                }
            }
        }
        // Горизонтальные границы.
        for syi in 0..sy.saturating_sub(1) {
            let y = 1 + syi * SEC + SEC - 1;
            if y + 1 >= h - 1 {
                continue;
            }
            for sxi in 0..sx {
                let x0 = 1 + sxi * SEC;
                let x1 = (x0 + SEC).min(w - 1);
                let mut run_start: Option<usize> = None;
                for x in x0..=x1 {
                    let open = x < x1 && g.linked(y * w + x, (y + 1) * w + x);
                    let joins = open
                        && run_start.is_some()
                        && g.linked(y * w + x - 1, y * w + x)
                        && g.linked((y + 1) * w + x - 1, (y + 1) * w + x);
                    let close = run_start.is_some() && (!open || !joins);
                    if close {
                        let s = run_start.take().expect("проверено");
                        let mid = (s + x - 1) / 2;
                        portals.push(Portal {
                            cell_a: (mid + y * w) as u32,
                            cell_b: (mid + (y + 1) * w) as u32,
                            sector_a: sec_of(mid, y),
                            sector_b: sec_of(mid, y + 1),
                            width: (x - s) as u16,
                        });
                    }
                    if open && run_start.is_none() {
                        run_start = Some(x);
                    }
                }
            }
        }
        st.portals = portals.len();

        // Порталы по секторам. Портал принадлежит **обоим** своим секторам.
        let nsec = sx * sy;
        let mut cnt = vec![0u32; nsec + 1];
        for p in &portals {
            cnt[p.sector_a as usize + 1] += 1;
            cnt[p.sector_b as usize + 1] += 1;
        }
        for i in 0..nsec {
            cnt[i + 1] += cnt[i];
        }
        let sec_starts = cnt.clone();
        let mut fill = cnt.clone();
        let mut sec_portals = vec![0u32; sec_starts[nsec] as usize];
        for (pi, p) in portals.iter().enumerate() {
            for s in [p.sector_a, p.sector_b] {
                sec_portals[fill[s as usize] as usize] = pi as u32;
                fill[s as usize] += 1;
            }
        }

        // Рёбра внутри сектора: расстояние от каждого портала до каждого
        // другого, посчитанное Дейкстрой по клеткам сектора.
        let mut starts = vec![0u32; portals.len() + 1];
        let mut edges: Vec<Edge> = Vec::new();
        let mut per_portal: Vec<Vec<Edge>> = vec![Vec::new(); portals.len()];
        let mut dist = vec![u32::MAX; w * h];
        let mut touched: Vec<usize> = Vec::new();

        for s in 0..nsec {
            let list = &sec_portals[sec_starts[s] as usize..sec_starts[s + 1] as usize];
            if list.len() < 2 {
                continue;
            }
            let (ox, oy) = (1 + (s % sx) * SEC, 1 + (s / sx) * SEC);
            let (ex, ey) = ((ox + SEC).min(w - 1), (oy + SEC).min(h - 1));
            for &pi in list.iter() {
                st.sector_solves += 1;
                let p = portals[pi as usize];
                // Стартуем с той клетки портала, что лежит в этом секторе.
                let start = if p.sector_a as usize == s { p.cell_a } else { p.cell_b } as usize;
                dijkstra_in_box(g, start, [ox, oy, ex, ey], &mut dist, &mut touched);
                for &qi in list.iter() {
                    if qi == pi {
                        continue;
                    }
                    let q = portals[qi as usize];
                    let goal =
                        if q.sector_a as usize == s { q.cell_a } else { q.cell_b } as usize;
                    let d = dist[goal];
                    if d != u32::MAX {
                        per_portal[pi as usize].push(Edge { to: qi, cost: d });
                    }
                }
                for &t in touched.iter() {
                    dist[t] = u32::MAX;
                }
                touched.clear();
            }
        }
        // Плюс само пересечение портала — переход между его сторонами.
        for (pi, p) in portals.iter().enumerate() {
            let c = g.cost[p.cell_b as usize] as u32;
            per_portal[pi].push(Edge { to: pi as u32, cost: c });
        }
        for (i, v) in per_portal.iter().enumerate() {
            starts[i + 1] = starts[i] + v.len() as u32;
            edges.extend_from_slice(v);
        }
        st.edges = edges.len();

        (
            PortalGraph { portals, starts, edges, sec_starts, sec_portals, sx, sy, w },
            st,
        )
    }

    #[inline]
    pub fn sector_of(&self, cell: usize) -> u32 {
        let (x, y) = (cell % self.w, cell / self.w);
        (((y - 1) / SEC) * self.sx + (x - 1) / SEC) as u32
    }

    pub fn portals_of(&self, sector: u32) -> &[u32] {
        let s = sector as usize;
        &self.sec_portals[self.sec_starts[s] as usize..self.sec_starts[s + 1] as usize]
    }
}

/// Дейкстра внутри прямоугольника. Ничьи разрешаются индексом клетки,
/// поэтому результат не зависит от порядка вставки в очередь.
fn dijkstra_in_box(
    g: &Grid,
    start: usize,
    bx: [usize; 4],
    dist: &mut [u32],
    touched: &mut Vec<usize>,
) {
    let [ox, oy, ex, ey] = bx;
    use std::collections::BinaryHeap;
    use std::cmp::Reverse;
    let w = g.w;
    let mut heap: BinaryHeap<Reverse<(u32, usize)>> = BinaryHeap::new();
    dist[start] = 0;
    touched.push(start);
    heap.push(Reverse((0, start)));
    while let Some(Reverse((d, i))) = heap.pop() {
        if d > dist[i] {
            continue;
        }
        let (x, y) = (i % w, i / w);
        for (nx, ny) in [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ] {
            if nx < ox || nx >= ex || ny < oy || ny >= ey {
                continue;
            }
            let ni = ny * w + nx;
            let c = g.cost[ni];
            if c == BLOCKED {
                continue;
            }
            // Тот же перепад и внутри сектора: иначе маршрут по графу
            // нашёлся бы, а идти по нему было бы некуда.
            if !g.linked(i, ni) {
                continue;
            }
            let nd = d + c as u32;
            if nd < dist[ni] {
                if dist[ni] == u32::MAX {
                    touched.push(ni);
                }
                dist[ni] = nd;
                heap.push(Reverse((nd, ni)));
            }
        }
    }
}

/// Переиспользуемые буферы поиска: выделение на запрос мерило бы аллокатор.
pub struct RouteWs {
    g: Vec<u32>,
    f: Vec<u32>,
    seen: Vec<u32>,
    epoch: u32,
    came: Vec<u32>,
}

impl RouteWs {
    pub fn new(n: usize) -> RouteWs {
        RouteWs {
            g: vec![0; n],
            f: vec![0; n],
            seen: vec![0; n],
            epoch: 0,
            came: vec![u32::MAX; n],
        }
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct RouteStats {
    pub found: bool,
    pub hops: u32,
    pub expanded: u32,
    pub cost: u32,
}

/// Маршрут от клетки к клетке по графу порталов.
///
/// Возвращает статистику, а не путь: бенчмарк меряет цену поиска,
/// а список порталов юнит получает тем же обходом `came`.
pub fn route(
    pg: &PortalGraph,
    g: &Grid,
    ws: &mut RouteWs,
    from: usize,
    to: usize,
) -> RouteStats {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let mut st = RouteStats::default();
    let (sa, sb) = (pg.sector_of(from), pg.sector_of(to));
    if sa == sb {
        // Один сектор — граф не нужен, юнит идёт локально.
        st.found = true;
        return st;
    }
    ws.epoch += 1;
    let ep = ws.epoch;
    let w = pg.w;
    let hcost = |p: u32| -> u32 {
        let c = pg.portals[p as usize].cell_a as usize;
        let (px, py) = ((c % w) as i64, (c / w) as i64);
        let (tx, ty) = ((to % w) as i64, (to / w) as i64);
        (px.abs_diff(tx) + py.abs_diff(ty)) as u32
    };

    // Ничьи по `f` разрешаются индексом портала — не порядком вставки.
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    for &p in pg.portals_of(sa) {
        let pt = pg.portals[p as usize];
        let cell = if pt.sector_a == sa { pt.cell_a } else { pt.cell_b } as usize;
        let (px, py) = ((cell % w) as i64, (cell / w) as i64);
        let (fx, fy) = ((from % w) as i64, (from / w) as i64);
        let d = (px.abs_diff(fx) + py.abs_diff(fy)) as u32 * g.cost[cell].max(1) as u32;
        if ws.seen[p as usize] != ep || d < ws.g[p as usize] {
            ws.seen[p as usize] = ep;
            ws.g[p as usize] = d;
            ws.f[p as usize] = d + hcost(p);
            ws.came[p as usize] = u32::MAX;
            heap.push(Reverse((ws.f[p as usize], p)));
        }
    }

    while let Some(Reverse((f, p))) = heap.pop() {
        if ws.seen[p as usize] != ep || f > ws.f[p as usize] {
            continue;
        }
        st.expanded += 1;
        let pt = pg.portals[p as usize];
        if pt.sector_a == sb || pt.sector_b == sb {
            st.found = true;
            st.cost = ws.g[p as usize];
            let mut cur = p;
            while ws.came[cur as usize] != u32::MAX {
                st.hops += 1;
                cur = ws.came[cur as usize];
            }
            return st;
        }
        let (s, e) = (pg.starts[p as usize] as usize, pg.starts[p as usize + 1] as usize);
        for ed in &pg.edges[s..e] {
            let ng = ws.g[p as usize] + ed.cost;
            let t = ed.to as usize;
            if ws.seen[t] != ep || ng < ws.g[t] {
                ws.seen[t] = ep;
                ws.g[t] = ng;
                ws.f[t] = ng + hcost(ed.to);
                ws.came[t] = p;
                heap.push(Reverse((ws.f[t], ed.to)));
            }
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{build, Scene};

    /// Граф обязан находить маршрут везде, где его находит полный поиск
    /// по клеткам. Расхождение здесь — это юнит, который «не видит»
    /// существующего прохода.
    #[test]
    fn portal_route_agrees_with_full_search() {
        for scene in [Scene::Surface, Scene::Tunnels, Scene::Maze] {
            let g = build(scene, 128, 5);
            let (pg, _) = PortalGraph::build(&g);
            let mut ws = RouteWs::new(pg.portals.len().max(1));

            // Полный поиск по клеткам от одной точки — эталон достижимости.
            let start = (1..g.h - 1)
                .flat_map(|y| (1..g.w - 1).map(move |x| y * g.w + x))
                .find(|&i| g.cost[i] != BLOCKED)
                .expect("сцена непроходима");
            let mut dist = vec![u32::MAX; g.w * g.h];
            let mut touched = Vec::new();
            dijkstra_in_box(&g, start, [1, 1, g.w - 1, g.h - 1], &mut dist, &mut touched);

            let mut r = fx::rng::Rng::new(9, 0x50);
            let (mut checked, mut miss) = (0, 0);
            for _ in 0..400 {
                let to = (r.below((g.w * g.h) as u32)) as usize;
                if dist[to] == u32::MAX {
                    continue;
                }
                checked += 1;
                if !route(&pg, &g, &mut ws, start, to).found {
                    miss += 1;
                }
            }
            assert!(checked > 50, "{}: мало достижимых целей", scene.name());
            assert_eq!(miss, 0, "{}: не найдено {miss} из {checked}", scene.name());
        }
    }

    /// Порядок обхода не влияет на результат: два прогона по одному
    /// и тому же входу дают одно и то же.
    #[test]
    fn route_is_order_independent() {
        let g = build(Scene::Tunnels, 128, 7);
        let (pg, _) = PortalGraph::build(&g);
        let mut a = RouteWs::new(pg.portals.len().max(1));
        let mut b = RouteWs::new(pg.portals.len().max(1));
        let mut r = fx::rng::Rng::new(4, 0x51);
        for _ in 0..200 {
            let from = r.below((g.w * g.h) as u32) as usize;
            let to = r.below((g.w * g.h) as u32) as usize;
            if g.cost[from] == BLOCKED || g.cost[to] == BLOCKED {
                continue;
            }
            let x = route(&pg, &g, &mut a, from, to);
            // Второй буфер «холодный», первый уже с историей эпох.
            let y = route(&pg, &g, &mut b, from, to);
            assert_eq!(x.found, y.found);
            assert_eq!(x.cost, y.cost);
            assert_eq!(x.hops, y.hops);
        }
    }
}

#[cfg(test)]
mod height_tests {
    use super::*;
    use crate::godunov::BLOCKED;
    use crate::scene::Grid;

    /// Плато на востоке поднято на `rise`, плоское поле на западе.
    /// Ни одной непроходимой клетки — разница только в высоте пола.
    fn two_plateaus(n: usize, rise: u16, ramp: bool) -> Grid {
        let mut g = Grid::new(n, n, 1);
        g.max_step = 1;
        let mid = n / 2;
        for y in 1..g.h - 1 {
            for x in 1..g.w - 1 {
                let i = y * g.w + x;
                g.floor[i] = if x > mid { rise } else { 0 };
                // Пандус: одна полоса, по которой высота растёт по шагу.
                if ramp && y == g.h / 2 && x > mid {
                    g.floor[i] = ((x - mid) as u16).min(rise);
                }
            }
        }
        g
    }

    /// **Отвесный уступ не даёт портала.** До ADR-121 граф строился
    /// по «клетка проходима с обеих сторон» и о высоте не знал:
    /// маршрут прокладывался прямо через стену (Q-68).
    #[test]
    fn a_cliff_gives_no_portal_and_no_route() {
        let g = two_plateaus(64, 8, false);
        let (graph, st) = PortalGraph::build(&g);
        assert!(st.portals > 0, "порталов нет вовсе — сцена не та");
        // Ни один портал не пересекает уступ.
        for p in &graph.portals {
            let (ax, bx) = (p.cell_a as usize % g.w, p.cell_b as usize % g.w);
            let (fa, fb) = (g.floor[p.cell_a as usize], g.floor[p.cell_b as usize]);
            assert!(
                fa.abs_diff(fb) <= g.max_step,
                "портал через уступ: x {ax}->{bx}, высота {fa}->{fb}"
            );
        }
        // И маршрута с запада на восток нет.
        let mut ws = RouteWs::new(graph.portals.len());
        let start = (g.h / 2) * g.w + 2;
        let goal = (g.h / 2) * g.w + (g.w - 3);
        let r = route(&graph, &g, &mut ws, start, goal);
        assert!(!r.found, "маршрут нашёлся сквозь отвесный уступ");
    }

    /// **Пандус даёт проход.** Та же геометрия, но с полосой, по которой
    /// высота растёт посильным шагом.
    #[test]
    fn a_ramp_opens_the_route() {
        let g = two_plateaus(64, 8, true);
        let graph = PortalGraph::build(&g).0;
        let mut ws = RouteWs::new(graph.portals.len());
        let start = (g.h / 2) * g.w + 2;
        let goal = (g.h / 2) * g.w + (g.w - 3);
        let r = route(&graph, &g, &mut ws, start, goal);
        assert!(r.found, "по пандусу пройти не удалось");
    }

    /// Сетка без заданной высоты обязана вести себя ровно как раньше:
    /// `max_step` по умолчанию не ограничивает ничего.
    #[test]
    fn a_grid_without_heights_is_unchanged() {
        let mut g = Grid::new(64, 64, 1);
        // Стена посередине с проходом — обычная сцена.
        for y in 1..g.h - 1 {
            let i = y * g.w + 32;
            if y != g.h / 2 {
                g.cost[i] = BLOCKED;
            }
        }
        let flat = PortalGraph::build(&g).0;
        let mut g2 = g.clone();
        // Высота есть, но перепад не ограничен.
        for v in g2.floor.iter_mut() {
            *v = 100;
        }
        let hilly = PortalGraph::build(&g2).0;
        assert_eq!(flat.portals.len(), hilly.portals.len());
    }
}
