//! Видимость: единственная строка бюджета симуляции, помеченная
//! «не определён». Здесь она определяется.
//!
//! Запуск: `cargo run --release -p nav --bin bench_vision`

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use nav::scene::{build, Grid, Scene};
use nav::vision::*;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn sensors(g: &Grid, n: usize, radius: u16, seed: u64) -> Vec<Sensor> {
    let mut r = Rng::new(seed, 0x5150);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let x = r.below(g.w as u32 - 2) + 1;
        let y = r.below(g.h as u32 - 2) + 1;
        if g.cost[y as usize * g.w + x as usize] == nav::godunov::BLOCKED {
            continue;
        }
        out.push(Sensor {
            x: x as u16,
            y: y as u16,
            radius,
        });
    }
    out
}

fn main() {
    // Карта 2×2 км при нав-клетке 2 м — 1024×1024.
    let n = 1024usize;
    println!("нав-сетка {n}×{n} (карта 2×2 км при клетке 2 м)");

    for scene in [Scene::Surface, Scene::Craters, Scene::Tunnels] {
        let g = build(scene, n, 7);
        section(&format!("сцена {}", scene.name()));
        println!(
            "{:>8} {:>8} {:>14} {:>14} {:>12} {:>10}",
            "радиус", "сенсоров", "лучами, мс", "каст, мс", "выигрыш", "клеток/сенс"
        );
        for radius in [10u16, 20] {
            let ss = sensors(&g, 5000, radius, 11);
            let mut mask = VisMask::new(g.w, g.h);

            // Луч на клетку.
            let mut best_rays = f64::MAX;
            let mut steps = 0u64;
            for _ in 0..3 {
                let t0 = Instant::now();
                mask.clear();
                let mut acc = 0u64;
                for &s in ss.iter() {
                    acc += per_cell_rays(&g, s, &mut mask);
                }
                let el = t0.elapsed().as_secs_f64() * 1e3;
                steps = acc;
                black_box(mask.count());
                best_rays = best_rays.min(el);
            }

            // Теневой каст.
            let mut best_cast = f64::MAX;
            let mut visits = 0u64;
            for _ in 0..3 {
                let t0 = Instant::now();
                mask.clear();
                let mut acc = 0u64;
                for &s in ss.iter() {
                    acc += shadowcast(&g, s, &mut mask);
                }
                let el = t0.elapsed().as_secs_f64() * 1e3;
                visits = acc;
                black_box(mask.count());
                best_cast = best_cast.min(el);
            }

            println!(
                "{radius:>8} {:>8} {best_rays:>14.2} {best_cast:>14.2} {:>11.1}x {:>10.0}",
                ss.len(),
                best_rays / best_cast,
                visits as f64 / ss.len() as f64
            );
            println!(
                "         шагов луча всего {:.1} млн против {:.1} млн посещений у каста",
                steps as f64 / 1e6,
                visits as f64 / 1e6
            );
        }
    }

    section("Инкрементальность: пересчёт только у сенсоров, сменивших клетку");
    let g = build(Scene::Surface, n, 7);
    let ss = sensors(&g, 5000, 14, 11);
    let mut mask = VisMask::new(g.w, g.h);
    let mut full = f64::MAX;
    for _ in 0..3 {
        let t0 = Instant::now();
        mask.clear();
        let mut acc = 0u64;
        for &s in ss.iter() {
            acc += shadowcast(&g, s, &mut mask);
        }
        black_box(acc);
        full = full.min(t0.elapsed().as_secs_f64() * 1e3);
    }
    println!("полный пересчёт всех 5000 сенсоров: {full:.2} мс на тик");
    println!(
        "{:>10} {:>12} {:>14} {:>16}",
        "доля", "сенсоров", "мс на тик", "доля тика 33 мс"
    );
    // Юнит при 3 м/с и клетке 2 м меняет клетку раз в ~20 тиков при 30 Гц.
    for (num, den, label) in [(1usize, 1usize, "все"), (1, 5, "1/5"), (1, 20, "1/20"), (1, 40, "1/40")] {
        let k = (ss.len() * num / den).max(1);
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &s in ss[..k].iter() {
                acc += shadowcast(&g, s, &mut mask);
            }
            black_box(acc);
            best = best.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        println!(
            "{label:>10} {k:>12} {best:>14.2} {:>15.1}%",
            100.0 * best / 33.3
        );
    }
    println!("(юнит при 3 м/с и клетке 2 м меняет клетку раз в ~20 тиков при 30 Гц)");

    section("Композит группы: слияние масок");
    let parts: Vec<VisMask> = (0..8)
        .map(|k| {
            let mut m = VisMask::new(g.w, g.h);
            for &s in ss[k * 100..(k + 1) * 100].iter() {
                shadowcast(&g, s, &mut m);
            }
            m
        })
        .collect();
    let mut acc = VisMask::new(g.w, g.h);
    let mut best = f64::MAX;
    for _ in 0..20 {
        let t0 = Instant::now();
        acc.clear();
        for p in parts.iter() {
            acc.or_from(p);
        }
        black_box(acc.count());
        best = best.min(t0.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "слияние 8 масок {}×{} ({} КиБ каждая): {best:.3} мс",
        g.w,
        g.h,
        acc.bits.len() * 8 / 1024
    );
}
