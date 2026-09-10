//! Масштабирование секторного решателя по потокам.
//!
//! Существует потому, что множитель «на семи воркерах» во всех бюджетах
//! получен делением однопоточного числа на семь и ни разу не был замерен.
//! Секторы независимы и пишут в непересекающиеся буферы — но это довод,
//! а не результат.
//!
//! Зависимостей нет. **Пул постоянный**: потоки поднимаются один раз
//! и синхронизируются барьером на каждый тик, как и требует `CLAUDE.md`
//! («пул создаётся с явно зафиксированным числом потоков»). Замер через
//! `thread::scope` на каждый тик мерил бы стоимость создания потоков
//! Windows, а не решатель.

use std::hint::black_box;
use std::sync::Barrier;
use std::time::Instant;

use nav::godunov::{h_exact_floor, HTable, BLOCKED, INF};
use nav::scene::{build, Grid, Scene};
use nav::solver::{fim_ws, fmm, Workspace};

/// Один грязный сектор: сетка плюс граничные условия от соседей.
pub struct Sector {
    g: Grid,
    seeds: Vec<(usize, u32)>,
}

fn make_sectors(scene: Scene, n: usize, count: usize) -> Vec<Sector> {
    // Одна большая сцена, из неё нарезаются непересекающиеся секторы — так
    // геометрия и граничные условия остаются согласованными между собой.
    let side = ((count as f64).sqrt().ceil() as usize).max(1) + 1;
    let big_n = side * (n + 2);
    let big = build(scene, big_n, 7);
    let goals = {
        let (cx, cy) = ((big.w - 1) / 2, (big.h - 1) / 2);
        let mut best = None;
        let mut best_d = usize::MAX;
        for y in 1..big.h - 1 {
            for x in 1..big.w - 1 {
                let i = y * big.w + x;
                if big.cost[i] == BLOCKED {
                    continue;
                }
                let d = cx.abs_diff(x) + cy.abs_diff(y);
                if d < best_d {
                    best_d = d;
                    best = Some(i);
                }
            }
        }
        vec![best.expect("сцена целиком непроходима")]
    };
    let (tbig, _) = fmm(&big, &goals, &h_exact_floor);

    let mut out = Vec::with_capacity(count);
    'outer: for sy in 0..side {
        for sx in 0..side {
            if out.len() >= count {
                break 'outer;
            }
            let (ox, oy) = ((n + 2) * sx, (n + 2) * sy);
            if ox + n + 2 >= big.w || oy + n + 2 >= big.h {
                continue;
            }
            let mut g = Grid::new(n + 2, n + 2, 10);
            for y in 1..g.h - 1 {
                for x in 1..g.w - 1 {
                    g.cost[y * g.w + x] = big.cost[(oy + y) * big.w + (ox + x)];
                }
            }
            let mut seeds = Vec::new();
            for y in 1..g.h - 1 {
                for x in 1..g.w - 1 {
                    if !(x == 1 || y == 1 || x == g.w - 2 || y == g.h - 2) {
                        continue;
                    }
                    let v = tbig[(oy + y) * big.w + (ox + x)];
                    if v != INF && g.cost[y * g.w + x] != BLOCKED {
                        seeds.push((y * g.w + x, v));
                    }
                }
            }
            if !seeds.is_empty() {
                out.push(Sector { g, seeds });
            }
        }
    }
    out
}

/// Однопоточно, без всякой синхронизации. Нижняя граница.
fn run_serial(sectors: &[Sector], htab: &HTable, reps: usize, trials: usize) -> f64 {
    let n = sectors[0].g.w * sectors[0].g.h;
    let mut ws = Workspace::new(n);
    let mut best = f64::MAX;
    for _ in 0..trials {
        let t0 = Instant::now();
        for _ in 0..reps {
            let mut acc = 0u64;
            for sec in sectors {
                acc = acc
                    .wrapping_add(fim_ws(&mut ws, &sec.g, &sec.seeds, &|f, d| htab.h(f, d)).updates);
            }
            black_box(acc);
        }
        best = best.min(t0.elapsed().as_secs_f64() * 1e3 / reps as f64);
    }
    best
}

/// Постоянный пул, барьер на тик.
fn run_pool(sectors: &[Sector], threads: usize, htab: &HTable, reps: usize, trials: usize) -> f64 {
    let barrier = Barrier::new(threads + 1);
    let chunk = sectors.len().div_ceil(threads);
    let parts: Vec<&[Sector]> = sectors.chunks(chunk).collect();
    let ticks = reps * trials;
    let mut best = f64::MAX;

    std::thread::scope(|s| {
        for w in 0..threads {
            let barrier = &barrier;
            let part: &[Sector] = parts.get(w).copied().unwrap_or(&[]);
            let htab = &*htab;
            let n = sectors[0].g.w * sectors[0].g.h;
            s.spawn(move || {
                let mut ws = Workspace::new(n);
                for _ in 0..ticks {
                    barrier.wait();
                    let mut acc = 0u64;
                    for sec in part {
                        acc = acc.wrapping_add(
                            fim_ws(&mut ws, &sec.g, &sec.seeds, &|f, d| htab.h(f, d)).updates,
                        );
                    }
                    black_box(acc);
                    barrier.wait();
                }
            });
        }
        for _ in 0..trials {
            let t0 = Instant::now();
            for _ in 0..reps {
                barrier.wait();
                barrier.wait();
            }
            best = best.min(t0.elapsed().as_secs_f64() * 1e3 / reps as f64);
        }
    });
    best
}

/// Цена самого барьера: те же такты, но работы ноль.
fn barrier_overhead(threads: usize, reps: usize) -> f64 {
    let barrier = Barrier::new(threads + 1);
    let mut best = f64::MAX;
    std::thread::scope(|s| {
        for _ in 0..threads {
            let barrier = &barrier;
            s.spawn(move || {
                for _ in 0..(reps * 3) {
                    barrier.wait();
                    barrier.wait();
                }
            });
        }
        for _ in 0..3 {
            let t0 = Instant::now();
            for _ in 0..reps {
                barrier.wait();
                barrier.wait();
            }
            best = best.min(t0.elapsed().as_secs_f64() * 1e3 / reps as f64);
        }
    });
    best
}

fn main() {
    let htab = HTable::new(255);
    let cores = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(1);
    println!("логических ядер: {cores}");

    println!("\nцена одного барьера на тик, без всякой работы:");
    println!("{:>8} {:>12}", "потоков", "мс/тик");
    for threads in [2usize, 4, 7, 8] {
        println!("{threads:>8} {:>12.4}", barrier_overhead(threads, 2000));
    }

    for (scene, n) in [
        (Scene::Surface, 16usize),
        (Scene::Craters, 16),
        (Scene::Surface, 32),
    ] {
        for count in [150usize, 400] {
            let sectors = make_sectors(scene, n, count);
            let reps = if n == 16 { 300 } else { 100 };
            let serial = run_serial(&sectors, &htab, reps, 5);
            println!(
                "\n=== {} сектор {n}x{n}, {} грязных ===",
                scene.name(),
                sectors.len()
            );
            println!(
                "однопоточно без синхронизации: {serial:.4} мс/тик ({:.2} мкс на сектор)",
                serial * 1000.0 / sectors.len() as f64
            );
            println!(
                "{:>8} {:>12} {:>10} {:>12}",
                "потоков", "мс/тик", "ускорение", "эффективн."
            );
            for threads in [1usize, 2, 4, 6, 7, 8] {
                if threads > cores {
                    continue;
                }
                let ms = run_pool(&sectors, threads, &htab, reps, 5);
                println!(
                    "{threads:>8} {ms:>12.4} {:>9.2}x {:>11.0}%",
                    serial / ms,
                    100.0 * serial / ms / threads as f64
                );
            }
        }
    }
    println!("\n(ускорение считается от однопоточного прогона без синхронизации)");
}
