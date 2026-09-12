//! Масштабирование фазы движения на **постоянном** пуле.
//!
//! Запуск: `cargo run --release -p sim --bin bench_movement_pool`
//!
//! Замер через `thread::scope` на каждый тик показал, что параллелизм
//! убыточен: создание потоков Windows дороже самой фазы. Здесь пул
//! поднимается один раз, синхронизация — барьером, как в замере эйконала.

use std::hint::black_box;
use std::sync::Barrier;
use std::time::Instant;

use sim_core::{populate, World};

/// Числа тика — из `data/sim.ron`, как и в игре (ADR-140).
fn tuning() -> sim_core::Tuning {
    sim_core::Tuning::from_ron(include_str!("../../../../data/sim.ron")).expect("data/sim.ron")
}

fn main() {
    let cores = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(1);
    println!("логических ядер: {cores}");
    println!("фаза движения: снимок положений, расталкивание, поле направлений\n");

    for n in [2500usize, 5000, 10000] {
        let mut w = World::with_cells(2, n * 2, tuning().separation, tuning().range, tuning());
        populate(&mut w, n, 42);
        for _ in 0..5 {
            w.tick();
        }
        w.prepare_movement();
        let slots = w.slots();

        // Однопоточно, без всякой синхронизации.
        let mut buf = w.take_delta();
        let mut serial = f64::MAX;
        for _ in 0..5 {
            let reps = 200usize;
            let t0 = Instant::now();
            for _ in 0..reps {
                for (i, slot) in buf.iter_mut().enumerate() {
                    if w.slot_alive(i) {
                        *slot = w.movement_kernel(i);
                    }
                }
            }
            black_box(buf[0]);
            serial = serial.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
        }
        println!("=== {n} сущностей ===");
        println!("однопоточно: {serial:.1} мкс");
        println!("{:>8} {:>12} {:>12} {:>12}", "потоков", "мкс", "ускорение", "эффективн.");

        for threads in [1usize, 2, 4, 6, 7, 8] {
            if threads > cores {
                continue;
            }
            let barrier = Barrier::new(threads + 1);
            let block = slots.div_ceil(threads);
            let reps = 200usize;
            let trials = 5usize;
            let mut best = f64::MAX;

            let mut rest: &mut [sim_core::drive::Want] = &mut buf;
            let mut parts: Vec<(usize, &mut [sim_core::drive::Want])> = Vec::new();
            let mut base = 0usize;
            while !rest.is_empty() {
                let take = block.min(rest.len());
                let (head, tail) = rest.split_at_mut(take);
                parts.push((base, head));
                base += take;
                rest = tail;
            }

            std::thread::scope(|sc| {
                for (offset, part) in parts {
                    let barrier = &barrier;
                    let w = &w;
                    sc.spawn(move || {
                        for _ in 0..(reps * trials) {
                            barrier.wait();
                            for (k, slot) in part.iter_mut().enumerate() {
                                let i = offset + k;
                                if w.slot_alive(i) {
                                    *slot = w.movement_kernel(i);
                                }
                            }
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
                    best = best.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
                }
            });
            println!(
                "{threads:>8} {best:>12.1} {:>11.2}x {:>11.0}%",
                serial / best,
                100.0 * serial / best / threads as f64
            );
        }
        println!();
        w.put_delta(buf);
        buf = w.take_delta();
        black_box(buf.len());
    }
    println!("(для сравнения: тот же счёт через thread::scope на каждый тик давал 0.52x на восьми)");
}
