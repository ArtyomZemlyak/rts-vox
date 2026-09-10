//! Видимость: что каждый сенсор видит, и во что это складывается для группы.
//!
//! Единственная строка бюджета симуляции, где естественная стоимость на
//! порядок выше самого бюджета. Здесь она измеряется, а не оценивается.
//!
//! Выход — битовая маска по карте, потому что композит группы — это `or`,
//! точно ассоциативный, а значит безопасный для параллельной редукции
//! (`CLAUDE.md`).

use crate::godunov::BLOCKED;
use crate::scene::Grid;

/// Битовая маска видимости по карте: слово на 64 клетки строки.
pub struct VisMask {
    pub words_per_row: usize,
    pub h: usize,
    pub bits: Vec<u64>,
}

impl VisMask {
    pub fn new(w: usize, h: usize) -> VisMask {
        let words_per_row = w.div_ceil(64);
        VisMask {
            words_per_row,
            h,
            bits: vec![0; words_per_row * h],
        }
    }

    pub fn clear(&mut self) {
        self.bits.iter_mut().for_each(|v| *v = 0);
    }

    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize) {
        self.bits[y * self.words_per_row + (x >> 6)] |= 1u64 << (x & 63);
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.words_per_row + (x >> 6)] & (1u64 << (x & 63)) != 0
    }

    pub fn count(&self) -> u32 {
        self.bits.iter().map(|w| w.count_ones()).sum()
    }

    /// Композит группы. `or` ассоциативен точно, поэтому порядок слияния
    /// не наблюдаем даже при параллельной редукции.
    pub fn or_from(&mut self, other: &VisMask) {
        for (a, b) in self.bits.iter_mut().zip(other.bits.iter()) {
            *a |= *b;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sensor {
    pub x: u16,
    pub y: u16,
    pub radius: u16,
}

// ---------------------------------------------------------------------------
// Способ 1: луч на клетку. То, что считается «в лоб».
// ---------------------------------------------------------------------------

/// Для каждой клетки диска — луч до сенсора по Брезенхэму.
/// Стоимость O(клеток × длину луча).
pub fn per_cell_rays(g: &Grid, s: Sensor, out: &mut VisMask) -> u64 {
    let r = s.radius as i32;
    let (sx, sy) = (s.x as i32, s.y as i32);
    let mut steps = 0u64;
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let (tx, ty) = (sx + dx, sy + dy);
            if tx < 1 || ty < 1 || tx >= g.w as i32 - 1 || ty >= g.h as i32 - 1 {
                continue;
            }
            // Брезенхэм от сенсора к цели.
            let (mut x, mut y) = (sx, sy);
            let (adx, ady) = ((tx - sx).abs(), (ty - sy).abs());
            let (stepx, stepy) = ((tx - sx).signum(), (ty - sy).signum());
            let mut err = adx - ady;
            let mut blocked = false;
            loop {
                steps += 1;
                if x == tx && y == ty {
                    break;
                }
                let e2 = 2 * err;
                if e2 > -ady {
                    err -= ady;
                    x += stepx;
                }
                if e2 < adx {
                    err += adx;
                    y += stepy;
                }
                if g.cost[y as usize * g.w + x as usize] == BLOCKED && !(x == tx && y == ty) {
                    blocked = true;
                    break;
                }
            }
            if !blocked {
                out.set(tx as usize, ty as usize);
            }
        }
    }
    steps
}

// ---------------------------------------------------------------------------
// Способ 2: рекурсивный shadowcasting. Стоимость O(клеток).
// ---------------------------------------------------------------------------

const OCTANTS: [(i32, i32, i32, i32); 8] = [
    (1, 0, 0, 1),
    (0, 1, 1, 0),
    (0, -1, 1, 0),
    (-1, 0, 0, 1),
    (-1, 0, 0, -1),
    (0, -1, -1, 0),
    (0, 1, -1, 0),
    (1, 0, 0, -1),
];

/// Наклон как **точная дробь**, а не плавающее число.
///
/// Теневой каст сравнивает наклоны и ничего с ними не считает, поэтому
/// дробь достаточна, а плавающее — избыточно и опасно: видимость входит
/// в состояние симуляции, и `f64` там означает расхождение между
/// машинами (`CLAUDE.md`). Знаменатель держится положительным, тогда
/// сравнение `a/b` против `c/d` — это `a·d` против `c·b`.
///
/// Величины ограничены радиусом сенсора: числитель и знаменатель не
/// превосходят `2r + 1`, произведение — квадрата этого, и `i64` хватает
/// с запасом в десять порядков.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Slope {
    n: i64,
    d: i64,
}

impl Slope {
    #[inline]
    pub fn new(n: i64, d: i64) -> Slope {
        debug_assert!(d != 0, "наклон с нулевым знаменателем");
        if d < 0 {
            Slope { n: -n, d: -d }
        } else {
            Slope { n, d }
        }
    }
    #[inline]
    pub fn lt(self, o: Slope) -> bool {
        self.n * o.d < o.n * self.d
    }
    #[inline]
    pub fn gt(self, o: Slope) -> bool {
        self.n * o.d > o.n * self.d
    }
}

/// Симметричный теневой каст: каждая клетка диска посещается один раз,
/// луча на клетку нет вовсе.
pub fn shadowcast(g: &Grid, s: Sensor, out: &mut VisMask) -> u64 {
    let mut visits = 1u64;
    out.set(s.x as usize, s.y as usize);
    for &(xx, xy, yx, yy) in OCTANTS.iter() {
        visits += cast(g, s, 1, Slope::new(1, 1), Slope::new(0, 1), xx, xy, yx, yy, out);
    }
    visits
}

#[allow(clippy::too_many_arguments)]
fn cast(
    g: &Grid,
    s: Sensor,
    row: i32,
    mut start: Slope,
    end: Slope,
    xx: i32,
    xy: i32,
    yx: i32,
    yy: i32,
    out: &mut VisMask,
) -> u64 {
    let mut visits = 0u64;
    if start.lt(end) {
        return visits;
    }
    let r = s.radius as i32;
    let mut blocked = false;
    let mut new_start = start;
    let mut dist = row;
    while dist <= r && !blocked {
        let dy = -dist;
        for dx in dy..=0 {
            // (dx ∓ 0.5) / (dy ± 0.5), умноженное на два сверху и снизу.
            let l_slope = Slope::new(2 * dx as i64 - 1, 2 * dy as i64 + 1);
            let r_slope = Slope::new(2 * dx as i64 + 1, 2 * dy as i64 - 1);
            if start.lt(r_slope) {
                continue;
            }
            if end.gt(l_slope) {
                break;
            }
            visits += 1;
            let cx = s.x as i32 + dx * xx + dy * xy;
            let cy = s.y as i32 + dx * yx + dy * yy;
            if cx < 1 || cy < 1 || cx >= g.w as i32 - 1 || cy >= g.h as i32 - 1 {
                continue;
            }
            if dx * dx + dy * dy <= r * r {
                out.set(cx as usize, cy as usize);
            }
            let opaque = g.cost[cy as usize * g.w + cx as usize] == BLOCKED;
            if blocked {
                if opaque {
                    new_start = r_slope;
                } else {
                    blocked = false;
                    start = new_start;
                }
            } else if opaque && dist < r {
                blocked = true;
                visits += cast(g, s, dist + 1, start, l_slope, xx, xy, yx, yy, out);
                new_start = r_slope;
            }
        }
        dist += 1;
    }
    visits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{build, Scene};

    /// На открытом поле оба способа обязаны дать один и тот же диск.
    #[test]
    fn open_field_agrees() {
        let g = build(Scene::Open, 64, 1);
        let s = Sensor {
            x: 32,
            y: 32,
            radius: 12,
        };
        let mut a = VisMask::new(g.w, g.h);
        let mut b = VisMask::new(g.w, g.h);
        per_cell_rays(&g, s, &mut a);
        shadowcast(&g, s, &mut b);
        assert_eq!(a.count(), b.count(), "разное число видимых клеток");
        for y in 1..g.h - 1 {
            for x in 1..g.w - 1 {
                assert_eq!(a.get(x, y), b.get(x, y), "клетка ({x},{y})");
            }
        }
    }

    /// Сенсор видит себя и не видит ничего за сплошной стеной.
    #[test]
    fn wall_casts_a_shadow() {
        let mut g = build(Scene::Open, 64, 1);
        for y in 1..g.h - 1 {
            let i = y * g.w + 40;
            g.cost[i] = BLOCKED;
        }
        let s = Sensor {
            x: 32,
            y: 32,
            radius: 20,
        };
        let mut m = VisMask::new(g.w, g.h);
        shadowcast(&g, s, &mut m);
        assert!(m.get(32, 32), "сенсор не видит себя");
        assert!(m.get(39, 32), "не видит клетку перед стеной");
        assert!(m.get(40, 32), "не видит саму стену");
        assert!(!m.get(45, 32), "видит сквозь стену");
        assert!(!m.get(50, 32), "видит сквозь стену далеко");
    }

    /// Композит группы не зависит от порядка слияния.
    #[test]
    fn composite_is_order_independent() {
        let g = build(Scene::Surface, 64, 1);
        let sensors: Vec<Sensor> = (0..8)
            .map(|k| Sensor {
                x: 8 + k * 6,
                y: 20 + (k % 3) * 8,
                radius: 10,
            })
            .collect();
        let parts: Vec<VisMask> = sensors
            .iter()
            .map(|&s| {
                let mut m = VisMask::new(g.w, g.h);
                shadowcast(&g, s, &mut m);
                m
            })
            .collect();
        let mut fwd = VisMask::new(g.w, g.h);
        for p in parts.iter() {
            fwd.or_from(p);
        }
        let mut rev = VisMask::new(g.w, g.h);
        for p in parts.iter().rev() {
            rev.or_from(p);
        }
        assert_eq!(fwd.bits, rev.bits);
    }
}
