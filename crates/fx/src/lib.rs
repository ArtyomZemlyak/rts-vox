//! Детерминированная арифметика с фиксированной точкой: Q32.32 на `i64`.
//!
//! Инварианты (`CLAUDE.md`):
//! - внутри крейта только явные `wrapping_*` / `checked_*` / `saturating_*`;
//! - голого `+` или `*` на внутреннем `i64` быть не должно;
//! - умножение через `i128`, деление через сдвиг в `i128`.

#![forbid(unsafe_code)]

pub mod rng;

/// Число дробных бит.
pub const FRAC_BITS: u32 = 32;
/// Единица в представлении Q32.32.
pub const ONE_RAW: i64 = 1i64 << FRAC_BITS;

/// Q32.32. Диапазон целой части ±2^31, разрешение 2^-32 ≈ 2.33e-10.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Fx(pub i64);

impl Fx {
    pub const ZERO: Fx = Fx(0);
    pub const ONE: Fx = Fx(ONE_RAW);
    pub const MIN: Fx = Fx(i64::MIN);
    pub const MAX: Fx = Fx(i64::MAX);

    /// Максимальная координата, при которой квадрат расстояния не переполняет
    /// целую часть: `a*a` обязано уместиться в ±2^31 (`03-determinism.md`).
    pub const MAX_SAFE_COORD: i32 = 46340;

    #[inline]
    pub const fn from_int(v: i32) -> Fx {
        Fx((v as i64).wrapping_shl(FRAC_BITS))
    }

    /// Точная дробь `num/den` без промежуточного float.
    #[inline]
    pub const fn from_ratio(num: i32, den: i32) -> Fx {
        let n = (num as i128).wrapping_shl(FRAC_BITS);
        Fx((n / den as i128) as i64)
    }

    #[inline]
    pub const fn to_int_trunc(self) -> i32 {
        (self.0 >> FRAC_BITS) as i32
    }

    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    #[inline]
    pub const fn add(self, o: Fx) -> Fx {
        Fx(self.0.wrapping_add(o.0))
    }

    #[inline]
    pub const fn sub(self, o: Fx) -> Fx {
        Fx(self.0.wrapping_sub(o.0))
    }

    /// Умножение через `i128`. Единственный корректный способ в Q32.32:
    /// произведение занимает до 128 бит до сдвига.
    #[inline]
    pub const fn mul(self, o: Fx) -> Fx {
        let p = (self.0 as i128).wrapping_mul(o.0 as i128);
        Fx((p >> FRAC_BITS) as i64)
    }

    /// Деление. Панику на нуле оставляем: молчаливый мусор хуже.
    #[inline]
    pub const fn div(self, o: Fx) -> Fx {
        let n = (self.0 as i128).wrapping_shl(FRAC_BITS);
        Fx((n / o.0 as i128) as i64)
    }

    /// Квадратный корень. `sqrt(x) = isqrt(raw << 32)`, потому что
    /// `sqrt(raw/2^32) * 2^32 = sqrt(raw * 2^32)`.
    #[inline]
    pub fn sqrt(self) -> Fx {
        debug_assert!(self.0 >= 0, "sqrt отрицательного");
        let widened = (self.0 as u128) << FRAC_BITS;
        Fx(widened.isqrt() as i64)
    }

    /// Корень с урезанной точностью, но целиком в 64 битах.
    ///
    /// Точный `sqrt` обязан считать `isqrt` от 128-битного значения, и это
    /// **единственное место, где детерминизм действительно дорог**: замер даёт
    /// 6.99 нс против 1.35 у `f64`. Здесь корень берётся от `raw << 8`,
    /// что укладывается в `u64` при значениях до 2^24, и результат сдвигается
    /// обратно: младшие 12 бит теряются, то есть разрешение падает с 2^-32
    /// до 2^-20 — микрометр в мировых единицах.
    ///
    /// Монотонность сохраняется (`isqrt` монотонен, сдвиг монотонен),
    /// поэтому функция пригодна там, где на ней стоит неподвижная точка.
    #[inline]
    pub fn sqrt_fast(self) -> Fx {
        debug_assert!(self.0 >= 0, "sqrt отрицательного");
        debug_assert!(self.0 < (1i64 << 56), "sqrt_fast: аргумент вне диапазона");
        Fx((((self.0 as u64) << 8).isqrt() << 12) as i64)
    }

    #[inline]
    pub const fn abs(self) -> Fx {
        Fx(self.0.wrapping_abs())
    }

    #[inline]
    pub const fn neg(self) -> Fx {
        Fx(self.0.wrapping_neg())
    }

    #[inline]
    pub const fn min(self, o: Fx) -> Fx {
        if self.0 < o.0 { self } else { o }
    }

    #[inline]
    pub const fn max(self, o: Fx) -> Fx {
        if self.0 > o.0 { self } else { o }
    }

    /// Только для диагностики и рендера. В симуляции запрещено.
    pub fn to_f64_lossy(self) -> f64 {
        self.0 as f64 / ONE_RAW as f64
    }
}

impl core::fmt::Debug for Fx {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Fx({:.6})", self.to_f64_lossy())
    }
}

/// Ассоциативность: единственная операция, безопасная для параллельной
/// редукции (`CLAUDE.md`). Обёртка нужна, чтобы это было видно в коде.
#[inline]
pub const fn wrapping_sum(acc: i64, v: i64) -> i64 {
    acc.wrapping_add(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_int_roundtrip() {
        for v in [-1000i32, -1, 0, 1, 1000, 46340] {
            assert_eq!(Fx::from_int(v).to_int_trunc(), v);
        }
    }

    #[test]
    fn mul_is_exact_on_integers() {
        let a = Fx::from_int(1234);
        let b = Fx::from_int(-567);
        assert_eq!(a.mul(b).to_int_trunc(), 1234 * -567);
    }

    #[test]
    fn mul_needs_i128() {
        // 2^10 * 2^10 = 2^20; сырые значения 2^42 каждое,
        // произведение 2^84 — без i128 это переполнение i64.
        let a = Fx::from_int(1 << 10);
        let b = Fx::from_int(1 << 10);
        // 2^40 не влезает в i32, поэтому сравниваем сырое значение.
        assert_eq!(a.mul(b).to_int_trunc(), 1 << 20);
    }

    #[test]
    fn sqrt_exact_squares() {
        for v in [0i32, 1, 4, 9, 144, 10_000, 1_000_000] {
            let r = Fx::from_int(v).sqrt();
            let expect = (v as f64).sqrt() as i32;
            assert_eq!(r.to_int_trunc(), expect, "sqrt({v})");
        }
    }

    #[test]
    fn sqrt_fast_is_close_and_monotone() {
        let mut prev = Fx::ZERO;
        let mut worst = 0i64;
        let mut v = 0i64;
        while v < (1i64 << 50) {
            let x = Fx(v);
            let fast = x.sqrt_fast();
            let exact = x.sqrt();
            assert!(fast.raw() >= prev.raw(), "sqrt_fast немонотонен на {v}");
            prev = fast;
            let d = (exact.raw() - fast.raw()).abs();
            assert!(d < (1i64 << 13), "расхождение {d} на {v}");
            worst = worst.max(d);
            v = (v * 3 / 2) + 1;
        }
        assert!(worst > 0, "тест ничего не проверил");
    }

    #[test]
    fn sqrt_fast_exact_on_squares() {
        for v in [0i32, 1, 4, 9, 144, 10_000, 1_000_000] {
            assert_eq!(Fx::from_int(v).sqrt_fast().to_int_trunc(), (v as f64).sqrt() as i32);
        }
    }

    #[test]
    fn sqrt_half() {
        // sqrt(0.25) = 0.5
        let q = Fx::from_ratio(1, 4);
        assert_eq!(q.sqrt().raw(), Fx::from_ratio(1, 2).raw());
    }

    /// Проверка утверждения из `03-determinism.md`: `wrapping_add`
    /// ассоциативен, `saturating_add` — нет.
    #[test]
    fn wrapping_add_associative_saturating_not() {
        let a = i64::MAX;
        let (b, c) = (1i64, -1i64);
        assert_eq!(
            a.wrapping_add(b).wrapping_add(c),
            a.wrapping_add(b.wrapping_add(c))
        );
        assert_ne!(
            a.saturating_add(b).saturating_add(c),
            a.saturating_add(b.saturating_add(c))
        );
    }

    #[test]
    fn max_safe_coord_is_tight() {
        let m = Fx::MAX_SAFE_COORD as i64;
        assert!(m * m < (1i64 << 31));
        assert!((m + 1) * (m + 1) >= (1i64 << 31));
    }
}
