//! Линия огня и видимости по битовому объёму.
//!
//! **Целочисленный обход, ни одного `f32`.** Линия огня решает, попал
//! снаряд или нет, то есть входит в состояние симуляции: плавающая
//! арифметика здесь — прямой десинк (`CLAUDE.md`). Марш рендера
//! (`gpu::march`) плавающий и живёт отдельно; общего у них только
//! битовый объём.
//!
//! Обход — трёхмерный Брезенхэм по ведущей оси. Он даёт **связную**
//! цепочку вокселей без пропусков на диагоналях: шаг фиксированной
//! длины по параметру их пропускает, и через угол между двумя блоками
//! проходил бы луч, которого быть не должно.

use crate::World;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LosResult {
    /// Луч дошёл до цели, не встретив твёрдого.
    pub clear: bool,
    /// Сколько вокселей проверено. Это и есть цена.
    pub steps: u32,
    /// Где остановились, если не дошли.
    pub blocked_at: (i32, i32, i32),
}

/// Обход отрезка целочисленным Брезенхэмом, **оба конца исключены**.
///
/// Единственный обход в проекте: линия огня и попадание снаряда обязаны
/// ходить по одним и тем же вокселям. Развести их — значит получить
/// снаряд, который летит там, где стрелок цели не видит.
///
/// Направление здесь **не канонизируется**: обход идёт от `from` к `to`,
/// то есть в ту сторону, куда летит снаряд, и первый встреченный
/// на пути — первый по ходу. Симметрия видимости нужна не полёту,
/// а взгляду, и делается она канонизацией концов **до** вызова —
/// см. `line_of_sight`.
///
/// `f` возвращает `false`, чтобы прекратить обход. Возвращается число
/// пройденных вокселей: это и есть цена.
pub fn walk(
    from: (i32, i32, i32),
    to: (i32, i32, i32),
    mut f: impl FnMut(i32, i32, i32) -> bool,
) -> u32 {
    let (mut x, mut y, mut z) = from;
    let (dx, dy, dz) = (to.0 - x, to.1 - y, to.2 - z);
    let (sx, sy, sz) = (dx.signum(), dy.signum(), dz.signum());
    let (ax, ay, az) = (dx.abs(), dy.abs(), dz.abs());
    let mut steps = 0u32;

    // Ведущая ось — самая длинная. Ошибки накапливаются целыми,
    // поэтому результат не зависит ни от порядка, ни от платформы.
    macro_rules! walk {
        ($a:ident, $b:ident, $c:ident, $sa:ident, $sb:ident, $sc:ident, $aa:expr, $ab:expr, $ac:expr) => {{
            let (mut eb, mut ec) = (2 * $ab - $aa, 2 * $ac - $aa);
            for _ in 0..$aa {
                if eb >= 0 {
                    $b += $sb;
                    eb -= 2 * $aa;
                }
                if ec >= 0 {
                    $c += $sc;
                    ec -= 2 * $aa;
                }
                eb += 2 * $ab;
                ec += 2 * $ac;
                $a += $sa;
                steps += 1;
                if (x, y, z) == to {
                    break;
                }
                if !f(x, y, z) {
                    break;
                }
            }
        }};
    }

    if ax >= ay && ax >= az {
        walk!(x, y, z, sx, sy, sz, ax, ay, az);
    } else if ay >= az {
        walk!(y, x, z, sy, sx, sz, ay, ax, az);
    } else {
        walk!(z, x, y, sz, sx, sy, az, ax, ay);
    }
    steps
}

/// Свободна ли прямая между двумя вокселями.
///
/// Оба конца **исключены**: стрелок и цель сами себя не заслоняют.
pub fn line_of_sight(
    w: &World,
    from: (i32, i32, i32),
    to: (i32, i32, i32),
) -> LosResult {
    // **Направление канонизируется.** Брезенхэм несимметричен:
    // при равной накопленной ошибке он округляет в одну сторону,
    // и путь из A в B не совпадает с путём из B в A. В игре это
    // «он меня видит, а я его нет» — и поймать такое глазами нельзя.
    //
    // Обход всегда идёт от лексикографически меньшего конца, поэтому
    // `los(a, b)` и `los(b, a)` проходят по одним и тем же вокселям.
    // Поймано тестом `visibility_is_symmetric`.
    let (from, to) = if from <= to { (from, to) } else { (to, from) };
    let (mut clear, mut blocked_at) = (true, to);
    let steps = walk(from, to, |x, y, z| {
        if w.solid(x.max(0) as u32, y.max(0) as u32, z.max(0) as u32) {
            clear = false;
            blocked_at = (x, y, z);
            false
        } else {
            true
        }
    });
    LosResult { clear, steps, blocked_at }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Mat, AIR};
    use crate::Generator;

    struct Empty;
    impl Generator for Empty {
        fn material(&self, _x: u32, _y: u32, _z: u32) -> Mat {
            AIR
        }
    }

    fn world() -> World {
        World::new(64, 64, 64, 100_000, Box::new(Empty))
    }

    fn wall(w: &mut World, x: u32) {
        for z in 0..64 {
            for y in 0..64 {
                w.queue_edit(crate::Edit::put(x, y, z, 2));
            }
        }
        w.flush();
    }

    #[test]
    fn empty_world_is_always_clear() {
        let w = world();
        assert!(line_of_sight(&w, (1, 1, 1), (60, 60, 60)).clear);
        assert!(line_of_sight(&w, (1, 32, 1), (60, 32, 60)).clear);
    }

    /// Стена закрывает линию огня, и точка блокировки лежит на ней.
    #[test]
    fn a_wall_blocks_the_line() {
        let mut w = world();
        wall(&mut w, 32);
        let r = line_of_sight(&w, (5, 20, 20), (60, 20, 20));
        assert!(!r.clear);
        assert_eq!(r.blocked_at.0, 32, "остановились не на стене: {r:?}");
    }

    /// **Дыра в стене открывает линию — в тот же вызов.** Никакого
    /// кэша между правкой и проверкой быть не должно.
    #[test]
    fn a_hole_opens_the_line_immediately() {
        let mut w = world();
        wall(&mut w, 32);
        assert!(!line_of_sight(&w, (5, 20, 20), (60, 20, 20)).clear);
        w.queue_dig(32, 20, 20);
        w.flush();
        assert!(
            line_of_sight(&w, (5, 20, 20), (60, 20, 20)).clear,
            "дыра не открыла линию в тот же тик"
        );
    }

    /// **Луч не проходит через угол между двумя блоками.** Шаг
    /// фиксированной длины по параметру именно здесь и врёт.
    #[test]
    fn the_ray_does_not_squeeze_through_a_corner() {
        let mut w = world();
        // Два блока по диагонали: между ними «щель» нулевой ширины.
        w.queue_edit(crate::Edit::put(32, 32, 32, 2));
        w.queue_edit(crate::Edit::put(33, 33, 32, 2));
        w.flush();
        // Луч идёт ровно через стык.
        let r = line_of_sight(&w, (30, 30, 32), (35, 35, 32));
        assert!(!r.clear, "луч пролез через угол: {r:?}");
    }

    /// Обход симметричен: то, что видно отсюда туда, видно и обратно.
    /// Несимметричная видимость — классический источник «он меня видит,
    /// а я его нет».
    #[test]
    fn visibility_is_symmetric() {
        let mut w = world();
        let mut r = fx::rng::Rng::new(3, 0x105);
        for _ in 0..300 {
            let (x, y, z) = (r.below(64), r.below(64), r.below(64));
            w.queue_edit(crate::Edit::put(x, y, z, 2));
        }
        w.flush();
        let mut checked = 0;
        for _ in 0..500 {
            let a = (
                r.below(64) as i32,
                r.below(64) as i32,
                r.below(64) as i32,
            );
            let b = (
                r.below(64) as i32,
                r.below(64) as i32,
                r.below(64) as i32,
            );
            if w.solid(a.0 as u32, a.1 as u32, a.2 as u32)
                || w.solid(b.0 as u32, b.1 as u32, b.2 as u32)
            {
                continue;
            }
            checked += 1;
            let ab = line_of_sight(&w, a, b).clear;
            let ba = line_of_sight(&w, b, a).clear;
            assert_eq!(ab, ba, "несимметрично: {a:?} -> {b:?}");
        }
        assert!(checked > 100);
    }

    /// Цена луча растёт линейно по длине, а не по объёму мира.
    #[test]
    fn cost_is_linear_in_length() {
        let w = world();
        let short = line_of_sight(&w, (1, 32, 32), (11, 32, 32)).steps;
        let long = line_of_sight(&w, (1, 32, 32), (61, 32, 32)).steps;
        assert_eq!(short, 10);
        assert_eq!(long, 60);
    }
}
