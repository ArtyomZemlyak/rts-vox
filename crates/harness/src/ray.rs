//! DDA по вокселям: лучи разрешения попаданий.
//!
//! Дорожная карта требует мерить именно **горизонтальные** лучи: «вся
//! литература меряет камерные, а у нас лучи разрешения попаданий идут вдоль
//! поверхности». Камерный луч сверху втыкается в рельеф за считаные шаги;
//! луч выстрела скользит над землёй сотни метров.
//!
//! Здесь же меряется анизотропия раскладки. Битовый объём хранит колонку
//! по X одним словом, значит луч вдоль X может проглотить шестьдесят два
//! воксела одним тестом слова, а луч вдоль Z обязан идти по биту. ADR-063
//! берёт множитель 3.7 из чужого замера на смежной задаче — здесь он свой.
//!
//! Арифметика в `Fx`: разрешение попаданий живёт в симуляции, значит
//! плавающей точки быть не может. Рядом стоит вариант на `f64` — чтобы
//! цена детерминизма на этой нагрузке была числом, а не предположением.

use crate::World;
use fx::Fx;
use meshing::CS as MCS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub hit: bool,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Сколько вокселей обошёл луч — цена, а не результат.
    pub steps: u32,
    /// Сколько раз пришлось читать память.
    pub reads: u32,
}

/// Направление луча в `Fx`. Нормировать не нужно: DDA работает с отношениями.
#[derive(Clone, Copy, Debug)]
pub struct Dir {
    pub x: Fx,
    pub y: Fx,
    pub z: Fx,
}

/// Амануатидес-Ву в целочисленной сетке, шаг за шагом.
///
/// Ни одного корня: `tDelta` — это отношение, а не длина (ADR-071).
pub fn dda(w: &World, ox: i32, oy: i32, oz: i32, d: Dir, max_steps: u32) -> Hit {
    let (mut x, mut y, mut z) = (ox, oy, oz);
    let step = |v: Fx| -> i32 {
        if v.raw() > 0 {
            1
        } else if v.raw() < 0 {
            -1
        } else {
            0
        }
    };
    let (sx, sy, sz) = (step(d.x), step(d.y), step(d.z));

    // Расстояние по параметру до следующей границы по каждой оси.
    // Компонента ноль — ось никогда не пересекается, ставим бесконечность.
    let big = Fx(i64::MAX / 4);
    let delta = |v: Fx| -> Fx {
        if v.raw() == 0 {
            big
        } else {
            Fx::ONE.div(v.abs())
        }
    };
    let (dx, dy, dz) = (delta(d.x), delta(d.y), delta(d.z));
    // Стартуем из центра воксела, поэтому до первой границы — половина шага.
    let half = Fx::from_ratio(1, 2);
    let (mut tx, mut ty, mut tz) = (dx.mul(half), dy.mul(half), dz.mul(half));

    let mut reads = 0u32;
    for steps in 0..max_steps {
        reads += 1;
        if w.get_i(x, y, z) {
            return Hit {
                hit: true,
                x,
                y,
                z,
                steps,
                reads,
            };
        }
        if tx.raw() <= ty.raw() && tx.raw() <= tz.raw() {
            x += sx;
            tx = tx.add(dx);
        } else if ty.raw() <= tz.raw() {
            y += sy;
            ty = ty.add(dy);
        } else {
            z += sz;
            tz = tz.add(dz);
        }
        if !w.inside(x, y, z) {
            return Hit {
                hit: false,
                x,
                y,
                z,
                steps,
                reads,
            };
        }
    }
    Hit {
        hit: false,
        x,
        y,
        z,
        steps: max_steps,
        reads,
    }
}

/// То же самое в `f64` — только чтобы знать цену детерминизма на этой
/// нагрузке. В симуляции запрещено.
#[allow(clippy::too_many_arguments)]
pub fn dda_f64(
    w: &World,
    ox: i32,
    oy: i32,
    oz: i32,
    dxf: f64,
    dyf: f64,
    dzf: f64,
    max_steps: u32,
) -> Hit {
    let (mut x, mut y, mut z) = (ox, oy, oz);
    let sgn = |v: f64| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    };
    let (sx, sy, sz) = (sgn(dxf), sgn(dyf), sgn(dzf));
    let inf = f64::INFINITY;
    let del = |v: f64| if v == 0.0 { inf } else { 1.0 / v.abs() };
    let (dx, dy, dz) = (del(dxf), del(dyf), del(dzf));
    let (mut tx, mut ty, mut tz) = (dx * 0.5, dy * 0.5, dz * 0.5);
    let mut reads = 0u32;
    for steps in 0..max_steps {
        reads += 1;
        if w.get_i(x, y, z) {
            return Hit {
                hit: true,
                x,
                y,
                z,
                steps,
                reads,
            };
        }
        if tx <= ty && tx <= tz {
            x += sx;
            tx += dx;
        } else if ty <= tz {
            y += sy;
            ty += dy;
        } else {
            z += sz;
            tz += dz;
        }
        if !w.inside(x, y, z) {
            return Hit {
                hit: false,
                x,
                y,
                z,
                steps,
                reads,
            };
        }
    }
    Hit {
        hit: false,
        x,
        y,
        z,
        steps: max_steps,
        reads,
    }
}

/// DDA с пропуском пустого участка колонки.
///
/// Битовый объём хранит колонку по X одним словом. Пока луч идёт вдоль X,
/// не покидая своей ячейки по Y и Z, весь пройденный участок можно проверить
/// **одним тестом маски** вместо шага за шагом.
///
/// Прыгать до границы чанка нельзя: луч может выйти из ячейки по Y или Z
/// раньше. Длина прыжка ограничена ближайшим пересечением по этим осям —
/// это и есть неочевидная часть.
pub fn dda_skip(w: &World, ox: i32, oy: i32, oz: i32, d: Dir, max_steps: u32) -> Hit {
    let (mut x, mut y, mut z) = (ox, oy, oz);
    let step = |v: Fx| -> i32 {
        if v.raw() > 0 {
            1
        } else if v.raw() < 0 {
            -1
        } else {
            0
        }
    };
    let (sx, sy, sz) = (step(d.x), step(d.y), step(d.z));
    let big = Fx(i64::MAX / 4);
    let delta = |v: Fx| -> Fx {
        if v.raw() == 0 {
            big
        } else {
            Fx::ONE.div(v.abs())
        }
    };
    let (dx, dy, dz) = (delta(d.x), delta(d.y), delta(d.z));
    let half = Fx::from_ratio(1, 2);
    let (mut tx, mut ty, mut tz) = (dx.mul(half), dy.mul(half), dz.mul(half));

    let mut reads = 0u32;
    let mut steps = 0u32;
    while steps < max_steps {
        reads += 1;
        if w.get_i(x, y, z) {
            return Hit {
                hit: true,
                x,
                y,
                z,
                steps,
                reads,
            };
        }

        // Сколько шагов по X можно сделать, не покидая ячейки по Y и Z.
        let mut jumped = false;
        if sx != 0 {
            let t_limit = if ty.raw() < tz.raw() { ty } else { tz };
            if tx.raw() <= t_limit.raw() && dx.raw() > 0 {
                let span = t_limit.sub(tx).div(dx).to_int_trunc().max(0) as i64 + 1;
                // Ограничение границей чанка: за ней другое слово.
                let local = x.rem_euclid(MCS as i32) + 1;
                let room = if sx > 0 {
                    (MCS as i32 + 1 - local) as i64
                } else {
                    (local - 1) as i64
                };
                let n = span.min(room).min((max_steps - steps) as i64);
                if n >= 2 {
                    reads += 1;
                    if let Some(word) = w.column_word(x, y, z) {
                        let mask = if sx > 0 {
                            ((1u64 << n) - 1) << (local + 1)
                        } else {
                            ((1u64 << n) - 1) << (local - n as i32)
                        };
                        if word & mask == 0 {
                            x += sx * n as i32;
                            tx = tx.add(Fx(dx.raw().wrapping_mul(n)));
                            steps += n as u32;
                            jumped = true;
                            if !w.inside(x, y, z) {
                                return Hit {
                                    hit: false,
                                    x,
                                    y,
                                    z,
                                    steps,
                                    reads,
                                };
                            }
                        }
                    }
                }
            }
        }

        if !jumped {
            if tx.raw() <= ty.raw() && tx.raw() <= tz.raw() {
                x += sx;
                tx = tx.add(dx);
            } else if ty.raw() <= tz.raw() {
                y += sy;
                ty = ty.add(dy);
            } else {
                z += sz;
                tz = tz.add(dz);
            }
            steps += 1;
            if !w.inside(x, y, z) {
                return Hit {
                    hit: false,
                    x,
                    y,
                    z,
                    steps,
                    reads,
                };
            }
        }
    }
    Hit {
        hit: false,
        x,
        y,
        z,
        steps,
        reads,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(x: i32, y: i32, z: i32) -> Dir {
        Dir {
            x: Fx::from_ratio(x, 100),
            y: Fx::from_ratio(y, 100),
            z: Fx::from_ratio(z, 100),
        }
    }

    /// Луч вниз обязан попасть в поверхность там же, где её видит мир.
    #[test]
    fn downward_ray_hits_the_surface() {
        let w = World::new(1, 1, 5);
        for (x, z) in [(10usize, 10usize), (31, 44), (55, 20)] {
            let h = dda(&w, x as i32, MCS as i32 - 1, z as i32, dir(0, -100, 0), 200);
            assert!(h.hit, "луч вниз не попал в ({x},{z})");
            assert!(w.get(x, h.y as usize, z), "попал в пустоту");
            assert!(
                !w.get(x, h.y as usize + 1, z),
                "попал не в первый твёрдый сверху"
            );
        }
    }

    /// **Пропуск обязан давать тот же ответ, что шаг за шагом.** Иначе им
    /// нельзя пользоваться: расхождение — это попадание, которого не было.
    #[test]
    fn skip_matches_step_by_step() {
        let w = World::new(2, 2, 9);
        let mut r = fx::rng::Rng::new(3, 3);
        let mut checked = 0;
        for _ in 0..3000 {
            let ox = r.below(w.voxels_x() as u32) as i32;
            let oz = r.below(w.voxels_z() as u32) as i32;
            let oy = r.range(4, MCS as i32 - 2);
            let d = dir(r.range(-100, 100), r.range(-30, 30), r.range(-100, 100));
            if d.x.raw() == 0 && d.y.raw() == 0 && d.z.raw() == 0 {
                continue;
            }
            checked += 1;
            let a = dda(&w, ox, oy, oz, d, 400);
            let b = dda_skip(&w, ox, oy, oz, d, 400);
            assert_eq!(
                (a.hit, a.x, a.y, a.z),
                (b.hit, b.x, b.y, b.z),
                "расхождение из ({ox},{oy},{oz})"
            );
        }
        assert!(checked > 2500, "тест почти ничего не проверил");
    }

    /// **`Fx` и `f64` расходятся, и это надо знать числом, а не отрицать.**
    ///
    /// Расхождение происходит на ничьих: когда луч приходит в ребро или
    /// угол воксела, `tx` и `ty` равны, а целочисленное деление и деление
    /// в плавающей точке округляют по-разному. Отсюда два следствия.
    ///
    /// Первое: проверять реализацию на `Fx` сравнением с плавающей точкой
    /// **нельзя** — расхождение не баг.
    /// Второе: превью выстрела в рендере, посчитанное во float, покажет
    /// не тот воксель, который выберет симуляция. Луч рендера обязан
    /// брать тот же код, что и симуляция, а не «похожий».
    #[test]
    fn fixed_point_and_float_diverge_on_ties() {
        let w = World::new(2, 2, 13);
        let mut r = fx::rng::Rng::new(8, 1);
        let mut diff = 0;
        let mut verdict_diff = 0;
        let mut total = 0;
        for _ in 0..5000 {
            let ox = r.below(w.voxels_x() as u32) as i32;
            let oz = r.below(w.voxels_z() as u32) as i32;
            let oy = r.range(4, MCS as i32 - 2);
            let (dx, dy, dz) = (r.range(-100, 100), r.range(-30, 30), r.range(-100, 100));
            if dx == 0 && dy == 0 && dz == 0 {
                continue;
            }
            total += 1;
            let a = dda(&w, ox, oy, oz, dir(dx, dy, dz), 400);
            let b = dda_f64(
                &w,
                ox,
                oy,
                oz,
                dx as f64 / 100.0,
                dy as f64 / 100.0,
                dz as f64 / 100.0,
                400,
            );
            if (a.x, a.y, a.z) != (b.x, b.y, b.z) {
                diff += 1;
            }
            if a.hit != b.hit {
                verdict_diff += 1;
            }
        }
        // Пришпиливаем величину: если она уедет, что-то изменилось в
        // арифметике, и это надо заметить.
        assert!(
            diff * 100 <= total * 3,
            "расхождение выросло: {diff} из {total}"
        );
        assert!(diff > 0, "расхождения нет — значит тест ничего не ловит");
        assert!(
            verdict_diff * 100 <= total,
            "разошёлся сам вердикт попадания на {verdict_diff} из {total}"
        );
    }
}
