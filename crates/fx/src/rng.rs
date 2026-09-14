//! Детерминированный RNG с явным сидом. Единственный источник случайности
//! в симуляции (`CLAUDE.md`).
//!
//! PCG32: состояние `u64`, выход `u32`. Выбран, потому что помещается
//! в два слова (дёшево класть в сейв и в golden-хеш), проходит TestU01
//! BigCrush и не имеет платформенно-зависимых операций.

/// PCG32 XSH-RR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
    inc: u64,
}

const MUL: u64 = 6_364_136_223_846_793_005;

impl Rng {
    /// Поток задаётся вторым аргументом: два генератора с одним сидом
    /// и разными потоками независимы.
    pub const fn new(seed: u64, stream: u64) -> Rng {
        let inc = stream.wrapping_shl(1) | 1;
        let mut r = Rng { state: 0, inc };
        r.state = r.state.wrapping_mul(MUL).wrapping_add(inc);
        r.state = r.state.wrapping_add(seed);
        r.state = r.state.wrapping_mul(MUL).wrapping_add(inc);
        r
    }

    /// Состояние целиком — **два слова**, как обещано в шапке файла.
    ///
    /// Нужно сохранению партии (ADR-229): генератор переживает тик,
    /// значит он состояние, а состояние, которое нельзя записать,
    /// нельзя и продолжить.
    pub const fn state(self) -> (u64, u64) {
        (self.state, self.inc)
    }

    /// Восстановить генератор из записанного состояния.
    ///
    /// Обратна [`Rng::state`] и только ей: `inc` обязан быть нечётным
    /// по построению PCG, и нечётность проверяется здесь, а не
    /// оставляется вызывающему — чётный `inc` даёт вырожденный поток.
    pub const fn from_state(state: u64, inc: u64) -> Rng {
        Rng {
            state,
            inc: inc | 1,
        }
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MUL).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let lo = self.next_u32() as u64;
        let hi = self.next_u32() as u64;
        hi.wrapping_shl(32) | lo
    }

    /// Равномерно в `[0, n)`. Отбраковка вместо остатка: остаток даёт
    /// смещение, зависящее от `n`, и это видно на балансе.
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        debug_assert!(n > 0);
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % n;
            }
        }
    }

    /// Равномерно в `[lo, hi]`.
    #[inline]
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        debug_assert!(hi >= lo);
        let span = (hi as i64 - lo as i64 + 1) as u32;
        lo.wrapping_add(self.below(span) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42, 0);
        let mut b = Rng::new(42, 0);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn streams_are_independent() {
        let mut a = Rng::new(42, 0);
        let mut b = Rng::new(42, 1);
        let mut same = 0;
        for _ in 0..1000 {
            if a.next_u32() == b.next_u32() {
                same += 1;
            }
        }
        assert!(same < 3, "потоки коррелируют: {same}");
    }

    #[test]
    fn below_is_unbiased_enough() {
        let mut r = Rng::new(7, 0);
        let mut buckets = [0u32; 10];
        for _ in 0..100_000 {
            buckets[r.below(10) as usize] += 1;
        }
        for (i, &b) in buckets.iter().enumerate() {
            assert!((9_000..=11_000).contains(&b), "корзина {i}: {b}");
        }
    }
}
