//! Годуновское обновление ячейки для уравнения эйконала, целочисленное.
//!
//! Все варианты обязаны быть **монотонными по обоим соседям** — на этом стоит
//! порядок-независимость (ADR-058), а с ней вся схема параллелизма.
//! Условие монотонности проверяется исчерпывающе в тестах, а не постулируется.

/// Стоимость прохода через ячейку, целое. `0` — непроходимо.
pub type Cost = u16;
/// Значение поля.
pub type Time = u32;

pub const BLOCKED: Cost = 0;
pub const INF: Time = u32::MAX;

// ---------------------------------------------------------------------------
// Ядро: h(f, d)
//
// Двумерное обновление сводится к `T = a + h(f, d)`, где `a = min(A, B)`,
// `d = |A - B|`. Это тождество, а не приближение: (a + b + s)/2 = a + (d+s)/2,
// и `2a` чётно, поэтому целочисленное деление ничего не теряет.
//
// При `d >= f` работает одномерная ветвь и `h = f`.
// При `d < f`  — двумерная: `h = (d + sqrt(2f^2 - d^2)) / 2`.
//
// Ключевое следствие для реализации: **дискриминант зависит только от `f` и
// `d`, оба узкие**. `2f^2 - d^2 <= 2*255^2 = 130050` — 17 бит, а не 66.
// ---------------------------------------------------------------------------

/// Точный вариант: целочисленный корень из дискриминанта, флор.
#[inline(always)]
pub fn h_exact_floor(f: u32, d: u32) -> u32 {
    if d >= f {
        return f;
    }
    let disc = 2 * f * f - d * d;
    (d + disc.isqrt()) >> 1
}

/// Точный вариант с округлением к ближайшему (амендмент к ADR-058).
#[inline(always)]
pub fn h_exact_round(f: u32, d: u32) -> u32 {
    if d >= f {
        return f;
    }
    let disc = 2 * f * f - d * d;
    (d + isqrt_round(disc) + 1) >> 1
}

/// Корень с округлением к ближайшему, без плавающей точки.
/// `round(sqrt(x)) = n`, где `n = isqrt(x)`, если `x <= n^2 + n`, иначе `n+1`:
/// граница между `n` и `n+1` это `(n + 0.5)^2 = n^2 + n + 0.25`.
#[inline(always)]
pub fn isqrt_round(x: u32) -> u32 {
    let n = x.isqrt();
    if x > n * n + n {
        n + 1
    } else {
        n
    }
}

/// Округление к ближайшему с ничьими к чётному. Единственный вариант без
/// систематического сдвига: `+1` в `h_exact_round` округляет половинки вверх
/// и накапливает завышение вдоль пути.
#[inline(always)]
pub fn h_exact_round_even(f: u32, d: u32) -> u32 {
    if d >= f {
        return f;
    }
    let disc = 2 * f * f - d * d;
    let num = d + isqrt_round(disc);
    let q = num >> 1;
    // Ничья ровно при нечётном `num`: истинное значение это q + 0.5.
    if num & 1 == 1 && q & 1 == 1 {
        q + 1
    } else {
        q
    }
}

/// Таблица корня на 12 бит, как записано в `06-navigation.md`.
/// Ограничение, которого в тексте нет: индекс 12 бит покрывает дискриминант
/// только до 4095, то есть **стоимость до 45**, а не до 255.
pub struct SqrtTable12 {
    t: Box<[u16; 4096]>,
}

impl Default for SqrtTable12 {
    fn default() -> Self {
        Self::new()
    }
}

impl SqrtTable12 {
    /// Максимальная стоимость, при которой таблица применима: 2*45^2 = 4050.
    pub const MAX_COST: u32 = 45;

    pub fn new() -> SqrtTable12 {
        let mut t = Box::new([0u16; 4096]);
        for (i, e) in t.iter_mut().enumerate() {
            *e = (i as u32).isqrt() as u16;
        }
        SqrtTable12 { t }
    }

    #[inline(always)]
    pub fn h(&self, f: u32, d: u32) -> u32 {
        if d >= f {
            return f;
        }
        let disc = 2 * f * f - d * d;
        (d + self.t[disc as usize] as u32) >> 1
    }
}

/// Таблица самого `h(f, d)`: ни корня, ни деления в горячем цикле.
/// Треугольная раскладка, смещение `f*(f-1)/2`.
pub struct HTable {
    t: Vec<u16>,
    max_cost: u32,
}

impl HTable {
    pub fn new(max_cost: u32) -> HTable {
        let n = (max_cost * (max_cost + 1) / 2) as usize;
        let mut t = vec![0u16; n];
        for f in 1..=max_cost {
            let base = (f * (f - 1) / 2) as usize;
            for d in 0..f {
                t[base + d as usize] = h_exact_floor(f, d) as u16;
            }
        }
        HTable { t, max_cost }
    }

    /// Размер в байтах — единственное, что решает, живёт ли она в L1.
    pub fn bytes(&self) -> usize {
        self.t.len() * 2
    }

    pub fn max_cost(&self) -> u32 {
        self.max_cost
    }

    #[inline(always)]
    pub fn h(&self, f: u32, d: u32) -> u32 {
        if d >= f {
            return f;
        }
        let base = (f * (f - 1) / 2) as usize;
        self.t[base + d as usize] as u32
    }
}

/// Равномерная стоимость: таблица одномерна и всегда в L1.
pub struct UniformTable {
    t: Vec<u16>,
    f: u32,
}

impl UniformTable {
    pub fn new(f: u32) -> UniformTable {
        let t = (0..f).map(|d| h_exact_floor(f, d) as u16).collect();
        UniformTable { t, f }
    }

    pub fn bytes(&self) -> usize {
        self.t.len() * 2
    }

    #[inline(always)]
    pub fn h(&self, d: u32) -> u32 {
        if d >= self.f {
            return self.f;
        }
        self.t[d as usize] as u32
    }
}

// ---------------------------------------------------------------------------
// Полное обновление ячейки из двух осевых минимумов.
// ---------------------------------------------------------------------------

/// Кандидат на новое значение, или `INF`, если ни один сосед не задан.
#[inline(always)]
pub fn update2d(a: Time, b: Time, f: Cost, h: impl Fn(u32, u32) -> u32) -> Time {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    if lo == INF {
        return INF;
    }
    let fc = f as u32;
    let d = if hi == INF { fc } else { (hi - lo).min(fc) };
    lo + h(fc, d)
}

/// Трёхмерное обновление. Здесь деление на три — настоящее, а не сдвиг.
#[inline(always)]
pub fn update3d(a: Time, b: Time, c: Time, f: Cost) -> Time {
    let mut n = [a, b, c];
    n.sort_unstable();
    if n[0] == INF {
        return INF;
    }
    let f = f as u64;
    let a0 = n[0] as u64;
    let t1 = a0 + f;
    if n[1] == INF || t1 <= n[1] as u64 {
        return t1 as Time;
    }
    let b0 = n[1] as u64;
    let d = b0 - a0;
    let t2 = a0 + ((d + (2 * f * f - d * d).isqrt()) >> 1);
    if n[2] == INF || t2 <= n[2] as u64 {
        return t2 as Time;
    }
    // Три оси: T = (S + sqrt(3f^2 - ((a-b)^2 + (b-c)^2 + (c-a)^2))) / 3.
    let c0 = n[2] as u64;
    let s = a0 + b0 + c0;
    let spread = (b0 - a0) * (b0 - a0) + (c0 - b0) * (c0 - b0) + (c0 - a0) * (c0 - a0);
    let disc = 3 * f * f - spread;
    ((s + disc.isqrt()) / 3) as Time
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Главный тест проекта.** Условие монотонности в терминах `h`:
    /// увеличение дальнего соседа на 1 (`d` растёт) не должно уменьшать
    /// результат, а увеличение ближнего на 1 (`a` растёт, `d` падает)
    /// не должно уменьшать его же — то есть `h` не убывает и растёт
    /// не быстрее чем на 1 за шаг.
    fn assert_monotone(name: &str, f: u32, h: impl Fn(u32, u32) -> u32) {
        let mut prev = h(f, 0);
        for d in 1..=(2 * f) {
            let cur = h(f, d.min(f));
            assert!(cur >= prev, "{name}: f={f} d={d} убывает {prev} -> {cur}");
            assert!(
                cur - prev <= 1,
                "{name}: f={f} d={d} шаг больше 1: {prev} -> {cur}"
            );
            prev = cur;
        }
    }

    #[test]
    fn exact_floor_monotone_all_costs() {
        for f in 1..=255u32 {
            assert_monotone("floor", f, h_exact_floor);
        }
    }

    #[test]
    fn exact_round_monotone_all_costs() {
        for f in 1..=255u32 {
            assert_monotone("round", f, h_exact_round);
        }
    }

    /// Округление к чётному тоже обязано быть монотонным, иначе им нельзя
    /// пользоваться независимо от того, лучше ли оно по смещению.
    #[test]
    fn round_even_monotone_all_costs() {
        for f in 1..=255u32 {
            assert_monotone("round-even", f, h_exact_round_even);
        }
    }

    #[test]
    fn sqrt_table_12_monotone_within_its_range() {
        let t = SqrtTable12::new();
        for f in 1..=SqrtTable12::MAX_COST {
            assert_monotone("sqrt12", f, |f, d| t.h(f, d));
        }
    }

    #[test]
    fn h_table_matches_exact() {
        let t = HTable::new(255);
        for f in 1..=255u32 {
            for d in 0..=(2 * f) {
                let d = d.min(f);
                assert_eq!(t.h(f, d), h_exact_floor(f, d), "f={f} d={d}");
            }
        }
    }

    #[test]
    fn boundary_agrees_with_1d() {
        // При d = f двумерная ветвь обязана дать ровно f.
        for f in 1..=255u32 {
            let disc = 2 * f * f - f * f;
            assert_eq!((f + disc.isqrt()) >> 1, f, "f={f}");
        }
    }

    #[test]
    fn round_never_below_floor_and_within_one() {
        for f in 1..=255u32 {
            for d in 0..f {
                let lo = h_exact_floor(f, d);
                let hi = h_exact_round(f, d);
                assert!(hi >= lo && hi - lo <= 1, "f={f} d={d}: {lo} {hi}");
            }
        }
    }

    #[test]
    fn round_sqrt_identity() {
        // Верхняя граница выборки — 2*255^2, то есть весь рабочий диапазон
        // дискриминанта.
        for x in 0u32..200_000 {
            let reference = (x as f64).sqrt().round() as u32;
            assert_eq!(isqrt_round(x), reference, "x={x}");
        }
    }

    /// Порядок-независимость требует монотонности и в трёхмерном случае —
    /// там деление на три, и заранее не очевидно, что она сохраняется.
    #[test]
    fn update3d_monotone_in_each_neighbour() {
        let base = 10_000u32;
        // Исчерпывающе по малым стоимостям, с шагом по большим: полный перебор
        // при f=255 это 134 миллиона троек, и он ничего не добавляет.
        for f in [1u16, 2, 3, 5, 7, 11, 16, 24, 32, 45, 64, 100, 160, 255] {
            let span = (f as u32) * 2 + 2;
            let step = (span / 48).max(1);
            for i in (0..span).step_by(step as usize) {
                for j in (0..span).step_by(step as usize) {
                    for k in (0..span).step_by(step as usize) {
                        let (a, b, c) = (base + i, base + j, base + k);
                        let t = update3d(a, b, c, f);
                        for bump in [
                            (a + 1, b, c),
                            (a, b + 1, c),
                            (a, b, c + 1),
                        ] {
                            let t2 = update3d(bump.0, bump.1, bump.2, f);
                            assert!(
                                t2 >= t,
                                "3d немонотонно: f={f} ({a},{b},{c})={t} -> {bump:?}={t2}"
                            );
                        }
                    }
                }
            }
        }
    }
}
