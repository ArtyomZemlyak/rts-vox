//! Сцены для замеров. Смысл файла — не дать мерить на равномерном шуме:
//! число обновлений на ячейку у эйконала зависит от геометрии сильнее, чем
//! от решателя, и «30% случайных препятствий» в нашей игре недостижимо.

use crate::godunov::{Cost, BLOCKED};
use fx::rng::Rng;

/// Сетка с рамкой в одну непроходимую ячейку — чтобы в горячем цикле
/// не было проверок границ.
#[derive(Clone)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub cost: Vec<Cost>,
}

impl Grid {
    pub fn new(inner_w: usize, inner_h: usize, fill: Cost) -> Grid {
        let (w, h) = (inner_w + 2, inner_h + 2);
        let mut cost = vec![BLOCKED; w * h];
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                cost[y * w + x] = fill;
            }
        }
        Grid { w, h, cost }
    }

    #[inline(always)]
    pub fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    pub fn passable(&self) -> usize {
        self.cost.iter().filter(|&&c| c != BLOCKED).count()
    }

    pub fn inner_area(&self) -> usize {
        (self.w - 2) * (self.h - 2)
    }

    fn block_rect(&mut self, x0: usize, y0: usize, wd: usize, ht: usize) {
        for y in y0..(y0 + ht).min(self.h - 1) {
            for x in x0..(x0 + wd).min(self.w - 1) {
                self.cost[y * self.w + x] = BLOCKED;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    /// Ровное поле, стоимость одинакова. Лучший случай.
    Open,
    /// Поверхность: здания и валуны кучками, стоимость рельефа гуляет.
    Surface,
    /// Поверхность после обстрела: много мелких завалов.
    Craters,
    /// Подземелье: почти всё сплошное, ходы прокопаны. Наша механика.
    Tunnels,
    /// Лабиринт: длинные геодезические. Худший случай для эйконала.
    Maze,
    /// Ровное поле, но стоимость от 1 до 100 — проверка динамического
    /// диапазона, который по разбору стоит семи бит разрядности.
    Road,
}

impl Scene {
    pub fn name(self) -> &'static str {
        match self {
            Scene::Open => "open",
            Scene::Surface => "surface",
            Scene::Craters => "craters",
            Scene::Tunnels => "tunnels",
            Scene::Maze => "maze",
            Scene::Road => "road",
        }
    }

    pub const ALL: [Scene; 6] = [
        Scene::Open,
        Scene::Surface,
        Scene::Craters,
        Scene::Tunnels,
        Scene::Maze,
        Scene::Road,
    ];
}

pub fn build(scene: Scene, n: usize, seed: u64) -> Grid {
    let mut r = Rng::new(seed, scene as u64);
    match scene {
        Scene::Open => Grid::new(n, n, 10),
        Scene::Surface => surface(n, &mut r),
        Scene::Craters => craters(n, &mut r),
        Scene::Tunnels => tunnels(n, &mut r),
        Scene::Maze => maze(n, &mut r),
        Scene::Road => road(n, &mut r),
    }
}

fn surface(n: usize, r: &mut Rng) -> Grid {
    let mut g = Grid::new(n, n, 10);
    // Рельеф: стоимость 8..16, крупными пятнами.
    for y in 1..g.h - 1 {
        for x in 1..g.w - 1 {
            let patch = ((x / 8) * 31 + (y / 8) * 17) as u32;
            let c = 8 + (patch % 9) as Cost;
            let i = g.idx(x, y);
            g.cost[i] = c;
        }
    }
    // Здания: прямоугольники 3x3..6x6, порядка 8% площади.
    let target = n * n * 8 / 100;
    let mut blocked = 0;
    while blocked < target {
        let wd = r.range(3, 6) as usize;
        let ht = r.range(3, 6) as usize;
        let x = r.below((n - wd) as u32) as usize + 1;
        let y = r.below((n - ht) as u32) as usize + 1;
        g.block_rect(x, y, wd, ht);
        blocked += wd * ht;
    }
    g
}

fn craters(n: usize, r: &mut Rng) -> Grid {
    let mut g = Grid::new(n, n, 10);
    let target = n * n * 20 / 100;
    let mut blocked = 0;
    while blocked < target {
        // Обломок: крестик или пара клеток, не куб.
        let x = r.below(n as u32) as usize + 1;
        let y = r.below(n as u32) as usize + 1;
        let sz = r.range(1, 2) as usize;
        g.block_rect(x, y, sz, sz);
        blocked += sz * sz;
    }
    g
}

fn tunnels(n: usize, r: &mut Rng) -> Grid {
    // Всё сплошное, ходы прокапываются случайным блужданием от входов.
    let mut g = Grid::new(n, n, BLOCKED);
    let entrances = (n / 16).max(2);
    let steps = n * 12;
    for e in 0..entrances {
        let mut x = (r.below(n as u32) as usize + 1).clamp(1, g.w - 2);
        let mut y = if e == 0 { 1 } else { r.below(n as u32) as usize + 1 };
        for _ in 0..steps {
            // Коридор шириной 2 — юнит должен пролезать.
            for dy in 0..2 {
                for dx in 0..2 {
                    let (xx, yy) = ((x + dx).min(g.w - 2), (y + dy).min(g.h - 2));
                    let i = g.idx(xx, yy);
                    g.cost[i] = 10;
                }
            }
            match r.below(4) {
                0 => x = (x + 1).min(g.w - 3),
                1 => x = x.saturating_sub(1).max(1),
                2 => y = (y + 1).min(g.h - 3),
                _ => y = y.saturating_sub(1).max(1),
            }
        }
    }
    g
}

fn maze(n: usize, r: &mut Rng) -> Grid {
    // Гребёнка со сдвинутыми проходами: геодезическая идёт змейкой через всю
    // сетку, то есть фронт эйконала обязан развернуться n/4 раз.
    let mut g = Grid::new(n, n, 10);
    let period = 4;
    let mut y = 1 + period;
    let mut left = true;
    while y < g.h - 1 {
        for x in 1..g.w - 1 {
            let i = g.idx(x, y);
            g.cost[i] = BLOCKED;
        }
        // Проход у одного края, с небольшим дрожанием.
        let gap = if left { 1 } else { g.w - 4 };
        let jitter = r.below(2) as usize;
        for x in gap..(gap + 2).min(g.w - 1) {
            let i = g.idx((x + jitter).min(g.w - 2), y);
            g.cost[i] = 10;
        }
        left = !left;
        y += period;
    }
    g
}

fn road(n: usize, r: &mut Rng) -> Grid {
    let mut g = Grid::new(n, n, 100);
    // Дороги: стоимость 1 против 100 у болота — отношение сто к одному.
    let lanes = (n / 24).max(2);
    for _ in 0..lanes {
        let y = r.below(n as u32) as usize + 1;
        for x in 1..g.w - 1 {
            let i = g.idx(x, y);
            g.cost[i] = 1;
        }
        let x = r.below(n as u32) as usize + 1;
        for y in 1..g.h - 1 {
            let i = g.idx(x, y);
            g.cost[i] = 1;
        }
    }
    g
}
