//! Во сколько обходится детерминизм: `Fx` против `f32` и `f64`
//! на нагрузках, которые в симуляции действительно есть.
//!
//! Запуск: `cargo run --release -p fx --bin bench_fx`

use std::hint::black_box;
use std::time::Instant;

use fx::{rng::Rng, Fx};

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn bench(name: &str, n: usize, reps: usize, mut f: impl FnMut() -> u64) -> f64 {
    black_box(f());
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut acc = 0u64;
        for _ in 0..reps {
            acc = acc.wrapping_add(f());
        }
        black_box(acc);
        best = best.min(t0.elapsed().as_secs_f64() * 1e9 / (reps * n) as f64);
    }
    println!("  {name:<26} {best:>7.3} нс/элемент");
    best
}

fn main() {
    let n = 4096usize;
    let mut r = Rng::new(1, 1);
    // Значения в разумном игровом диапазоне: координаты до километра,
    // скорости единицы метров в секунду.
    let ax: Vec<i64> = (0..n)
        .map(|_| (r.range(-2000, 2000) as i64) << 16)
        .collect();
    let bx: Vec<i64> = (0..n)
        .map(|_| (r.range(-2000, 2000) as i64) << 16)
        .collect();
    let af: Vec<f64> = ax.iter().map(|&v| v as f64 / 4294967296.0).collect();
    let bf: Vec<f64> = bx.iter().map(|&v| v as f64 / 4294967296.0).collect();
    let a32: Vec<f32> = af.iter().map(|&v| v as f32).collect();
    let b32: Vec<f32> = bf.iter().map(|&v| v as f32).collect();
    let afx: Vec<Fx> = ax.iter().map(|&v| Fx(v)).collect();
    let bfx: Vec<Fx> = bx.iter().map(|&v| Fx(v)).collect();
    let reps = 4000;

    section("1. Сложение");
    let f64_add = bench("f64", n, reps, || {
        let mut s = 0.0f64;
        for i in 0..n {
            s += af[i] + bf[i];
        }
        s.to_bits()
    });
    let f32_add = bench("f32", n, reps, || {
        let mut s = 0.0f32;
        for i in 0..n {
            s += a32[i] + b32[i];
        }
        s.to_bits() as u64
    });
    let fx_add = bench("Fx (wrapping_add)", n, reps, || {
        let mut s = 0i64;
        for i in 0..n {
            s = s.wrapping_add(afx[i].add(bfx[i]).raw());
        }
        s as u64
    });
    println!(
        "  Fx против f64 x{:.2}, против f32 x{:.2}",
        fx_add / f64_add,
        fx_add / f32_add
    );

    section("2. Умножение (у Fx — через i128)");
    let f64_mul = bench("f64", n, reps, || {
        let mut s = 0.0f64;
        for i in 0..n {
            s += af[i] * bf[i];
        }
        s.to_bits()
    });
    let f32_mul = bench("f32", n, reps, || {
        let mut s = 0.0f32;
        for i in 0..n {
            s += a32[i] * b32[i];
        }
        s.to_bits() as u64
    });
    let fx_mul = bench("Fx", n, reps, || {
        let mut s = 0i64;
        for i in 0..n {
            s = s.wrapping_add(afx[i].mul(bfx[i]).raw());
        }
        s as u64
    });
    println!(
        "  Fx против f64 x{:.2}, против f32 x{:.2}",
        fx_mul / f64_mul,
        fx_mul / f32_mul
    );

    section("3. Длина вектора: sqrt(x*x + y*y) — то, что делает движение");
    let f64_len = bench("f64", n, reps / 2, || {
        let mut s = 0.0f64;
        for i in 0..n {
            s += (af[i] * af[i] + bf[i] * bf[i]).sqrt();
        }
        s.to_bits()
    });
    let f32_len = bench("f32", n, reps / 2, || {
        let mut s = 0.0f32;
        for i in 0..n {
            s += (a32[i] * a32[i] + b32[i] * b32[i]).sqrt();
        }
        s.to_bits() as u64
    });
    let fx_len = bench("Fx (u128::isqrt)", n, reps / 2, || {
        let mut s = 0i64;
        for i in 0..n {
            let d = afx[i].mul(afx[i]).add(bfx[i].mul(bfx[i]));
            s = s.wrapping_add(d.sqrt().raw());
        }
        s as u64
    });
    let fx_len_fast = bench("Fx (sqrt_fast, 64 бита)", n, reps / 2, || {
        let mut s = 0i64;
        for i in 0..n {
            let d = afx[i].mul(afx[i]).add(bfx[i].mul(bfx[i]));
            s = s.wrapping_add(d.sqrt_fast().raw());
        }
        s as u64
    });
    println!(
        "  Fx против f64 x{:.2}, против f32 x{:.2}",
        fx_len / f64_len,
        fx_len / f32_len
    );
    println!(
        "  sqrt_fast против f64 x{:.2}, против точного Fx x{:.2} быстрее",
        fx_len_fast / f64_len,
        fx_len / fx_len_fast
    );

    section("4. Сравнение квадратов расстояний — то, чем должен быть радиус");
    // Корень для «в радиусе ли» не нужен вовсе, и это самый дешёвый способ
    // не платить за него ни в одной арифметике.
    let f64_cmp = bench("f64", n, reps, || {
        let mut c = 0u64;
        for i in 0..n {
            if af[i] * af[i] + bf[i] * bf[i] < 1_000_000.0 {
                c += 1;
            }
        }
        c
    });
    let fx_cmp = bench("Fx", n, reps, || {
        let mut c = 0u64;
        for i in 0..n {
            if afx[i].mul(afx[i]).add(bfx[i].mul(bfx[i])) < Fx::from_int(1_000_000) {
                c += 1;
            }
        }
        c
    });
    println!("  Fx против f64 x{:.2}", fx_cmp / f64_cmp);

    section("5. Что это значит");
    println!("  При 5000 юнитов и 20 арифметических операций на юнита за тик");
    println!("  разница между самой дорогой и самой дешёвой строкой выше — это");
    let worst = (fx_len - f64_len).max(fx_mul - f64_mul);
    println!("  {:.3} мс на тик.", worst * 5000.0 * 20.0 / 1e6);
    println!("  Бюджет тика — 10 мс.");
}
