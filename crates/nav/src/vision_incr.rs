//! Инкрементальная видимость: локальная маска на сенсор плюс сетка счётчиков.
//!
//! Наивная схема «штамповать всех в одну общую маску» **неполна**: она умеет
//! добавлять видимость и не умеет её снимать. Когда сенсор сдвинулся или
//! копка изменила геометрию, старый вклад надо вычесть, а вычесть его можно,
//! только зная, каким он был.
//!
//! Отсюда конструкция:
//! - **локальная маска на сенсор** — диск `(2R+1)²` бит, слово на строку — 232 байта при R=14;
//! - **сетка счётчиков** по карте: сколько сенсоров группы видят клетку;
//! - обновление одного сенсора — вычесть старую маску, посчитать новую,
//!   прибавить. Видимость группы — «счётчик больше нуля».
//!
//! Счётчик, а не бит, потому что снятие вклада одного сенсора не должно
//! гасить клетку, которую видит другой.

use crate::vision::Sensor;
use crate::godunov::BLOCKED;
use crate::scene::Grid;

/// Маска одного сенсора в его собственных координатах.
#[derive(Clone)]
pub struct LocalMask {
    pub r: usize,
    side: usize,
    words_per_row: usize,
    bits: Vec<u64>,
    /// Где стоял сенсор, когда маска считалась.
    pub cx: u16,
    pub cy: u16,
    pub valid: bool,
}

impl LocalMask {
    pub fn new(r: usize) -> LocalMask {
        let side = 2 * r + 1;
        let words_per_row = side.div_ceil(64);
        LocalMask {
            r,
            side,
            words_per_row,
            bits: vec![0; words_per_row * side],
            cx: 0,
            cy: 0,
            valid: false,
        }
    }

    pub fn bytes(&self) -> usize {
        self.bits.len() * 8
    }

    #[inline(always)]
    fn clear(&mut self) {
        self.bits.iter_mut().for_each(|v| *v = 0);
    }

    #[inline(always)]
    fn set_local(&mut self, lx: usize, ly: usize) {
        self.bits[ly * self.words_per_row + (lx >> 6)] |= 1u64 << (lx & 63);
    }
}

/// Сетка счётчиков видимости группы.
pub struct CountGrid {
    pub w: usize,
    pub h: usize,
    pub c: Vec<u16>,
}

impl CountGrid {
    pub fn new(w: usize, h: usize) -> CountGrid {
        CountGrid {
            w,
            h,
            c: vec![0; w * h],
        }
    }

    pub fn bytes(&self) -> usize {
        self.c.len() * 2
    }

    pub fn visible(&self, x: usize, y: usize) -> bool {
        self.c[y * self.w + x] > 0
    }

    pub fn count_visible(&self) -> usize {
        self.c.iter().filter(|&&v| v > 0).count()
    }

    /// Прибавить или вычесть вклад локальной маски. `delta` только ±1:
    /// счётчик — это число сенсоров, а не вес.
    fn apply(&mut self, m: &LocalMask, add: bool) {
        if !m.valid {
            return;
        }
        let r = m.r as i64;
        let (cx, cy) = (m.cx as i64, m.cy as i64);
        for ly in 0..m.side {
            let gy = cy - r + ly as i64;
            if gy < 0 || gy >= self.h as i64 {
                continue;
            }
            let row = gy as usize * self.w;
            let base = ly * m.words_per_row;
            for wi in 0..m.words_per_row {
                let mut w = m.bits[base + wi];
                while w != 0 {
                    let b = w.trailing_zeros() as usize;
                    w &= w - 1;
                    let gx = cx - r + (wi * 64 + b) as i64;
                    if gx < 0 || gx >= self.w as i64 {
                        continue;
                    }
                    let i = row + gx as usize;
                    if add {
                        self.c[i] += 1;
                    } else {
                        debug_assert!(self.c[i] > 0, "счётчик ушёл в минус: вклад вычтен дважды");
                        self.c[i] -= 1;
                    }
                }
            }
        }
    }
}

/// Теневой каст в локальную маску сенсора.
pub fn shadowcast_local(g: &Grid, s: Sensor, m: &mut LocalMask) -> u64 {
    debug_assert_eq!(m.r, s.radius as usize);
    m.clear();
    m.cx = s.x;
    m.cy = s.y;
    m.valid = true;
    let mut visits = 1u64;
    m.set_local(m.r, m.r);
    for &(xx, xy, yx, yy) in OCTANTS.iter() {
        visits += cast_local(g, s, 1, 1.0, 0.0, xx, xy, yx, yy, m);
    }
    visits
}

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

#[allow(clippy::too_many_arguments)]
fn cast_local(
    g: &Grid,
    s: Sensor,
    row: i32,
    mut start: f64,
    end: f64,
    xx: i32,
    xy: i32,
    yx: i32,
    yy: i32,
    m: &mut LocalMask,
) -> u64 {
    let mut visits = 0u64;
    if start < end {
        return visits;
    }
    let r = s.radius as i32;
    let mut blocked = false;
    let mut new_start = start;
    let mut dist = row;
    while dist <= r && !blocked {
        let dy = -dist;
        for dx in dy..=0 {
            let l_slope = (dx as f64 - 0.5) / (dy as f64 + 0.5);
            let r_slope = (dx as f64 + 0.5) / (dy as f64 - 0.5);
            if start < r_slope {
                continue;
            }
            if end > l_slope {
                break;
            }
            visits += 1;
            let ox = dx * xx + dy * xy;
            let oy = dx * yx + dy * yy;
            let cx = s.x as i32 + ox;
            let cy = s.y as i32 + oy;
            if cx < 1 || cy < 1 || cx >= g.w as i32 - 1 || cy >= g.h as i32 - 1 {
                continue;
            }
            if dx * dx + dy * dy <= r * r {
                m.set_local((r + ox) as usize, (r + oy) as usize);
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
                visits += cast_local(g, s, dist + 1, start, l_slope, xx, xy, yx, yy, m);
                new_start = r_slope;
            }
        }
        dist += 1;
    }
    visits
}

/// Обновить одного сенсора: снять старый вклад, посчитать новый, прибавить.
/// Это единственная корректная форма инкрементального обновления.
pub fn refresh(g: &Grid, s: Sensor, m: &mut LocalMask, cg: &mut CountGrid) -> u64 {
    cg.apply(m, false);
    let v = shadowcast_local(g, s, m);
    cg.apply(m, true);
    v
}

/// Первичная вставка сенсора, у которого вклада ещё не было.
pub fn insert(g: &Grid, s: Sensor, m: &mut LocalMask, cg: &mut CountGrid) -> u64 {
    let v = shadowcast_local(g, s, m);
    cg.apply(m, true);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{build, Scene};

    fn same_as_global(scene: Scene, radius: u16) {
        let g = build(scene, 96, 3);
        let sensors: Vec<Sensor> = (0..12)
            .map(|k| Sensor {
                x: 10 + (k % 4) * 20,
                y: 10 + (k / 4) * 25,
                radius,
            })
            .collect();
        let mut global = crate::vision::VisMask::new(g.w, g.h);
        for &s in &sensors {
            crate::vision::shadowcast(&g, s, &mut global);
        }
        let mut cg = CountGrid::new(g.w, g.h);
        let mut masks: Vec<LocalMask> = sensors.iter().map(|s| LocalMask::new(s.radius as usize)).collect();
        for (i, &s) in sensors.iter().enumerate() {
            insert(&g, s, &mut masks[i], &mut cg);
        }
        for y in 1..g.h - 1 {
            for x in 1..g.w - 1 {
                assert_eq!(
                    cg.visible(x, y),
                    global.get(x, y),
                    "{} ({x},{y})",
                    scene.name()
                );
            }
        }
    }

    /// Сетка счётчиков обязана давать ровно ту же видимость, что общая маска.
    #[test]
    fn count_grid_matches_global_mask() {
        for scene in [Scene::Surface, Scene::Craters, Scene::Tunnels, Scene::Maze] {
            for r in [8u16, 14] {
                same_as_global(scene, r);
            }
        }
    }

    /// **Главное свойство.** Вычесть вклад и прибавить обратно — тождество.
    /// Без него инкремент накапливает мусор и расходится за сотню тиков.
    #[test]
    fn refresh_is_idempotent_when_nothing_changed() {
        let g = build(Scene::Surface, 96, 3);
        let sensors: Vec<Sensor> = (0..10)
            .map(|k| Sensor {
                x: 12 + (k % 5) * 15,
                y: 12 + (k / 5) * 30,
                radius: 12,
            })
            .collect();
        let mut cg = CountGrid::new(g.w, g.h);
        let mut masks: Vec<LocalMask> = sensors.iter().map(|s| LocalMask::new(s.radius as usize)).collect();
        for (i, &s) in sensors.iter().enumerate() {
            insert(&g, s, &mut masks[i], &mut cg);
        }
        let before = cg.c.clone();
        for _ in 0..5 {
            for (i, &s) in sensors.iter().enumerate() {
                refresh(&g, s, &mut masks[i], &mut cg);
            }
        }
        assert_eq!(cg.c, before, "пересчёт без изменений сдвинул счётчики");
    }

    /// Сенсор сдвинулся — старый вклад обязан исчезнуть полностью.
    #[test]
    fn moving_a_sensor_removes_its_old_contribution() {
        let g = build(Scene::Open, 96, 3);
        let mut cg = CountGrid::new(g.w, g.h);
        let mut m = LocalMask::new(10);
        let a = Sensor { x: 20, y: 20, radius: 10 };
        insert(&g, a, &mut m, &mut cg);
        assert!(cg.visible(20, 20));
        let b = Sensor { x: 70, y: 70, radius: 10 };
        refresh(&g, b, &mut m, &mut cg);
        assert!(!cg.visible(20, 20), "старый вклад остался");
        assert!(cg.visible(70, 70), "новый вклад не появился");
        assert_eq!(cg.c.iter().map(|&v| v as usize).sum::<usize>(), {
            let mut probe = CountGrid::new(g.w, g.h);
            let mut m2 = LocalMask::new(10);
            insert(&g, b, &mut m2, &mut probe);
            probe.c.iter().map(|&v| v as usize).sum::<usize>()
        });
    }

    /// Два сенсора на одной клетке: снятие одного не гасит клетку.
    #[test]
    fn overlap_is_counted_not_flagged() {
        let g = build(Scene::Open, 64, 3);
        let mut cg = CountGrid::new(g.w, g.h);
        let a = Sensor { x: 30, y: 30, radius: 8 };
        let b = Sensor { x: 32, y: 30, radius: 8 };
        let mut ma = LocalMask::new(8);
        let mut mb = LocalMask::new(8);
        insert(&g, a, &mut ma, &mut cg);
        insert(&g, b, &mut mb, &mut cg);
        assert!(cg.c[30 * cg.w + 31] >= 2, "перекрытие не сосчитано");
        let far = Sensor { x: 55, y: 55, radius: 8 };
        refresh(&g, far, &mut ma, &mut cg);
        assert!(cg.visible(31, 30), "клетку погасил уход соседа");
    }
}
