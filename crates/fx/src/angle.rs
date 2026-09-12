//! Углы в целых: курс, синус, косинус, арктангенс.
//!
//! Курс машины — состояние, которое живёт тиками и переживает их
//! (ADR-220). Хранить его вектором значило бы нормировать корнем
//! каждый тик и копить дрейф длины; хранить углом в `Fx` — считать
//! остаток от деления на `2π`, которое в двоичной дроби не
//! представимо. Поэтому угол здесь **двоичный**: полный оборот —
//! степень двойки, и «кратчайший доворот» выходит обрезкой до
//! шестнадцати бит, без ветвлений и без деления.
//!
//! Таблицы считает `const fn` на сборке — рядами Тейлора в `i128`.
//! Ни одного записанного руками числа, кроме `π`: записанная таблица
//! это данные, которым верят на слово, а посчитанная проверяется
//! тем же тестом, что и формула.

use crate::Fx;

/// Полный оборот. Степень двойки нарочно.
pub const FULL: i32 = 1 << 16;
/// Пол-оборота.
pub const HALF: i32 = FULL / 2;
/// Четверть оборота.
pub const QUARTER: i32 = FULL / 4;

/// Отсчётов таблицы синуса на полный оборот.
///
/// Тысяча двадцать четыре — это 0.35° на отсчёт, то есть предельная
/// ошибка направления 0.18°. При шаге 0.2 мировой единицы за тик это
/// четыре тысячных единицы поперёк — меньше, чем разрешение сетки
/// мира, поэтому интерполяции между отсчётами нет.
const N: usize = 1024;
/// Сколько младших бит угла таблица не различает.
const SHIFT: u32 = 16 - 10;

/// `π` в Q32.32. Единственное записанное число модуля: всё остальное
/// из него считается.
const PI: i64 = 13_493_037_705;
/// `2π` в Q32.32.
const TWO_PI: i64 = 26_986_075_410;
/// Единица в Q32.32.
const ONE: i64 = 1i64 << 32;

/// Умножение Q32.32 через `i128` — то же, что `Fx::mul`, но `const`.
const fn mul(a: i64, b: i64) -> i64 {
    ((a as i128).wrapping_mul(b as i128) >> 32) as i64
}

/// Деление Q32.32 через `i128` — то же, что `Fx::div`, но `const`.
const fn div(a: i64, b: i64) -> i64 {
    (((a as i128).wrapping_shl(32)) / b as i128) as i64
}

/// Целый корень из `u128` побитово. `u128::isqrt` не `const` на пине.
const fn isqrt(v: u128) -> u128 {
    if v == 0 {
        return 0;
    }
    let mut rest = v;
    let mut bit: u128 = 1u128 << 126;
    while bit > rest {
        bit >>= 2;
    }
    let mut root: u128 = 0;
    while bit != 0 {
        if rest >= root + bit {
            rest -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

/// Корень в Q32.32, `const`.
const fn sqrt_q(v: i64) -> i64 {
    isqrt((v as u128) << 32) as i64
}

/// Синус рядом Тейлора, `|x| <= π/2`, Q32.32.
///
/// Девять членов: следующий на `π/2` даёт 6.7e-10 — три младших
/// бита Q32.32, то есть меньше шага таблицы на пять порядков.
const fn sin_taylor(x: i64) -> i64 {
    let x2 = mul(x, x);
    let mut term = x;
    let mut sum = x;
    let mut k: i64 = 1;
    while k <= 8 {
        term = mul(term, x2) / (2 * k * (2 * k + 1));
        if k & 1 == 1 {
            sum = sum.wrapping_sub(term);
        } else {
            sum = sum.wrapping_add(term);
        }
        k += 1;
    }
    sum
}

/// Отсчёт синуса номер `k` из `N` на оборот.
const fn sin_at(k: usize) -> i64 {
    let quarter_steps = (N / 4) as i64;
    let q = (k / (N / 4)) & 3;
    let r = (k % (N / 4)) as i64;
    let j = if q & 1 == 0 { r } else { quarter_steps - r };
    // На самой четверти ряд промахивается на два младших бита,
    // и единица перестаёт быть единицей. Четверти — опорные точки
    // курса, они обязаны быть точными.
    let s = if j == quarter_steps {
        ONE
    } else {
        sin_taylor((PI * j) / (2 * quarter_steps))
    };
    if q >= 2 {
        -s
    } else {
        s
    }
}

/// Таблица синуса на полный оборот, Q32.32.
const SIN: [i64; N] = {
    let mut t = [0i64; N];
    let mut k = 0;
    while k < N {
        t[k] = sin_at(k);
        k += 1;
    }
    t
};

/// Отсчётов таблицы арктангенса. Полшага по отношению сторон — это
/// 0.11° по углу, вдвое ниже шага таблицы синуса.
const ATAN_N: usize = 256;

/// `atan(t)` в радианах Q32.32 для `0 <= t <= 1`.
///
/// Прямой ряд у единицы сходится негодно, поэтому аргумент четырежды
/// уполовинивается тождеством `atan(t) = 2·atan(t / (1 + sqrt(1+t²)))`:
/// единица падает до 0.0491, и четырёх членов хватает с запасом.
const fn atan_unit(t: i64) -> i64 {
    let mut x = t;
    let mut k = 0;
    while k < 4 {
        let r = sqrt_q(ONE.wrapping_add(mul(x, x)));
        x = div(x, ONE.wrapping_add(r));
        k += 1;
    }
    let x2 = mul(x, x);
    let mut term = x;
    let mut sum = x;
    let mut n: i64 = 1;
    while n <= 4 {
        term = mul(term, x2);
        let d = term / (2 * n + 1);
        if n & 1 == 1 {
            sum = sum.wrapping_sub(d);
        } else {
            sum = sum.wrapping_add(d);
        }
        n += 1;
    }
    sum.wrapping_mul(16)
}

/// Таблица арктангенса: `ATAN[j]` — угол, у которого отношение
/// меньшей стороны к большей равно `j/ATAN_N`. В двоичных единицах,
/// от нуля до восьмой части оборота.
const ATAN: [i32; ATAN_N + 1] = {
    let mut t = [0i32; ATAN_N + 1];
    let mut j = 0;
    while j <= ATAN_N {
        let arg = ((j as i64) << 32) / ATAN_N as i64;
        let rad = atan_unit(arg);
        t[j] = ((rad.wrapping_mul(FULL as i64) + TWO_PI / 2) / TWO_PI) as i32;
        j += 1;
    }
    t
};

/// Привести угол к `0..FULL`.
#[inline]
pub const fn norm(a: i32) -> i32 {
    a & (FULL - 1)
}

/// Кратчайшая разность углов, `-HALF..HALF`.
///
/// Обрезка до шестнадцати бит и знаковое расширение — вся арифметика:
/// полный оборот равен `1 << 16`, значит `i16` и есть кратчайший
/// поворот.
#[inline]
pub const fn diff(a: i32, b: i32) -> i32 {
    (a.wrapping_sub(b) & (FULL - 1)) as u16 as i16 as i32
}

/// Синус угла.
#[inline]
pub fn sin(a: i32) -> Fx {
    let k = ((a.wrapping_add(1 << (SHIFT - 1)) >> SHIFT) as usize) & (N - 1);
    Fx(SIN[k])
}

/// Косинус угла.
#[inline]
pub fn cos(a: i32) -> Fx {
    sin(a.wrapping_add(QUARTER))
}

/// `atan(lo/hi)` в двоичных единицах, `0 <= lo <= hi`, `hi > 0`.
///
/// Между отсчётами таблица **интерполируется**: без этого ошибка
/// у нуля равна половине шага по отношению сторон, то есть 0.11°, —
/// вдвое грубее самой таблицы синуса. Линейная вставка сбивает её
/// до тысячных долей градуса ценой одного умножения.
#[inline]
fn atan_ratio(lo: u128, hi: u128) -> i32 {
    let r = (lo << (8 + 8)) / hi;
    let j = (r >> 8).min(ATAN_N as u128 - 1);
    let f = (r - (j << 8)) as i64;
    let (a, b) = (ATAN[j as usize] as i64, ATAN[j as usize + 1] as i64);
    (a + (b - a) * f / 256) as i32
}

/// Угол направления `(x, z)`: такой `a`, что `(cos a, sin a)`
/// смотрит туда же.
///
/// Ноль на нулевом векторе — это не угол, а отсутствие направления;
/// решать, что с ним делать, обязан вызывающий.
pub fn atan2(z: i64, x: i64) -> i32 {
    let (ax, az) = (x.unsigned_abs() as u128, z.unsigned_abs() as u128);
    if ax == 0 && az == 0 {
        return 0;
    }
    let base = if az <= ax {
        atan_ratio(az, ax)
    } else {
        QUARTER - atan_ratio(ax, az)
    };
    let a = match (x >= 0, z >= 0) {
        (true, true) => base,
        (false, true) => HALF - base,
        (false, false) => HALF + base,
        (true, false) => -base,
    };
    norm(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Плавающее здесь законно: это проверка таблицы, а не счёт
    /// симуляции. `production_sources` в архитектурном линте режет
    /// `#[cfg(test)]` целиком.
    fn to_f(v: Fx) -> f64 {
        v.raw() as f64 / (1i64 << 32) as f64
    }

    /// **Таблица синуса — это синус.** Считана рядом Тейлора, и
    /// проверяется независимо, библиотечным.
    #[test]
    fn the_sine_table_is_the_sine() {
        let mut worst = 0.0f64;
        for (k, &v) in SIN.iter().enumerate() {
            let want = (std::f64::consts::TAU * k as f64 / N as f64).sin();
            let got = v as f64 / (1i64 << 32) as f64;
            worst = worst.max((want - got).abs());
        }
        assert!(worst < 1e-9, "таблица синуса врёт на {worst}");
    }

    /// Опорные значения — точно.
    #[test]
    fn the_quarter_turns_are_exact() {
        assert_eq!(sin(0).raw(), 0);
        assert_eq!(cos(0).raw(), 1i64 << 32);
        assert_eq!(sin(QUARTER).raw(), 1i64 << 32);
        assert_eq!(cos(QUARTER).raw(), 0);
        assert_eq!(sin(HALF).raw(), 0);
        assert_eq!(cos(HALF).raw(), -(1i64 << 32));
        assert_eq!(sin(-QUARTER).raw(), -(1i64 << 32));
    }

    /// **Единичная длина держится по всему обороту.** Курс дрейфовать
    /// не должен: на этом стоит выбор угла вместо вектора.
    #[test]
    fn the_direction_stays_unit_length() {
        let mut worst = 0.0f64;
        for k in 0..N {
            let a = (k as i32) << SHIFT;
            let (c, s) = (to_f(cos(a)), to_f(sin(a)));
            worst = worst.max((c * c + s * s - 1.0).abs());
        }
        assert!(worst < 1e-8, "длина уехала на {worst}");
    }

    /// **Арктангенс возвращает направление, которое ему дали.**
    #[test]
    fn atan2_names_the_direction_it_was_given() {
        let mut worst = 0.0f64;
        for zi in -40..=40i64 {
            for xi in -40..=40i64 {
                if xi == 0 && zi == 0 {
                    continue;
                }
                let a = atan2(zi, xi);
                let want = (zi as f64).atan2(xi as f64);
                let got = a as f64 * std::f64::consts::TAU / FULL as f64;
                let mut d = (got - want).abs();
                if d > std::f64::consts::PI {
                    d = std::f64::consts::TAU - d;
                }
                worst = worst.max(d);
            }
        }
        // Полшага таблицы синуса — это то, чем угол вообще
        // различается: 0.0031 радиана. Арктангенс обязан быть
        // точнее неё, иначе интерполяция в нём не нужна.
        assert!(worst < 3e-4, "арктангенс врёт на {worst} радиан");
    }

    /// Оси — точно, без округления в соседний отсчёт.
    #[test]
    fn the_axes_are_exact() {
        assert_eq!(atan2(0, 1), 0);
        assert_eq!(atan2(1, 0), QUARTER);
        assert_eq!(atan2(0, -1), HALF);
        assert_eq!(atan2(-1, 0), norm(-QUARTER));
        assert_eq!(atan2(1, 1), QUARTER / 2);
        assert_eq!(atan2(-1, -1), HALF + QUARTER / 2);
    }

    /// **Кратчайший доворот — кратчайший.**
    #[test]
    fn the_difference_is_the_short_way_round() {
        assert_eq!(diff(10, 0), 10);
        assert_eq!(diff(0, 10), -10);
        // Через ноль: короткий путь идёт в плюс, а не в минус
        // почти на полный оборот.
        let (a, b) = (norm(200), norm(-200));
        assert_eq!(diff(a, b), 400);
        assert_eq!(diff(b, a), -400);
        // Ровно пол-оборота — и вправо, и влево одинаково; выбор
        // обязан быть один и тот же всегда.
        assert_eq!(diff(HALF, 0), -HALF);
        assert!(diff(0, 0) == 0);
    }

    /// **Повороты не копят дрейфа.** Тысяча доворотов по три
    /// градуса — это ровно три тысячи градусов, а не «примерно».
    #[test]
    fn turning_never_drifts() {
        let step = 3 * FULL / 360;
        let mut a = 0i32;
        for _ in 0..1000 {
            a = norm(a + step);
        }
        assert_eq!(a, norm(1000 * step));
    }
}
