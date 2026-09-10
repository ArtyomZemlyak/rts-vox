//! Настоящая фаза на видеокарте: эйконал.
//!
//! Запуск: `cargo run --release -p gpu --bin bench_phase`
//!
//! Почему именно эйконал, как устроен шейдер и почему Якоби, а не FIM —
//! в `gpu::eikonal`. Здесь только замер.
//!
//! Сверка с процессором живёт **тестом**
//! `gpu_eikonal_matches_cpu_bit_for_bit`, а не печатью в этом бенчмарке:
//! расхождение означает, что переносить фазу нельзя ни при какой скорости,
//! и такое обязано ронять `cargo test`, а не печататься мелким шрифтом.

use std::time::Instant;

use gpu::eikonal::{Solver, N};
use gpu::Gpu;

/// Грязных секторов в тике — из `11-eikonal-bench.md`.
const DIRTY: u32 = 150;
const REPS: usize = 21;

fn stat(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[0])
}

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn main() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        println!("адаптера нет");
        return;
    };
    println!("{} | драйвер {}", g.info.name, g.info.driver_info);
    let period = g.ts_period.unwrap_or(0.0) as f64;
    println!("такт таймстемпа {period} нс; медиана из {REPS} прогонов");

    let max_sectors = 4096u32;
    let s = Solver::new(&g, max_sectors);

    let qs = g.device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("ts"),
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let qbuf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("qresolve"),
        size: 16,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    // Возвращает (стенка, время на видеокарте) в наносекундах.
    let run = |sectors: u32, iters: u32| -> (f64, f64) {
        s.set_params(&g, sectors, iters);
        let mut wall = Vec::new();
        let mut dev = Vec::new();
        for r in 0..(REPS + 3) {
            let t0 = Instant::now();
            let mut e = g.device.create_command_encoder(&Default::default());
            e.write_timestamp(&qs, 0);
            s.encode(&mut e, sectors);
            e.write_timestamp(&qs, 1);
            e.resolve_query_set(&qs, 0..2, &qbuf, 0);
            g.queue.submit([e.finish()]);
            g.wait();
            let w = t0.elapsed().as_nanos() as f64;
            let t = g.read_u32(&qbuf, 4);
            let a = t[0] as u64 | ((t[1] as u64) << 32);
            let b = t[2] as u64 | ((t[3] as u64) << 32);
            if r >= 3 {
                wall.push(w);
                dev.push((b - a) as f64 * period);
            }
        }
        (stat(wall).0, stat(dev).0)
    };

    section("1. Столько же работы, сколько меряли на процессоре");
    println!("150 грязных секторов 16×16 — конфигурация из ADR-066.");
    println!("CPU: 11.9–14.2 мкс на сектор, итого 0.36–0.41 мс на пяти воркерах.\n");
    println!(
        "{:>8} {:>16} {:>14} {:>16}",
        "итераций", "на видеокарте", "стенка, мкс", "на сектор, нс"
    );
    for iters in [16u32, 32, 48, 64] {
        let (w, d) = run(DIRTY, iters);
        println!(
            "{iters:>8} {:>12.1} мкс {:>14.1} {:>16.0}",
            d / 1000.0,
            w / 1000.0,
            d / DIRTY as f64
        );
    }

    section("2. Сколько секторов помещается в ту же задержку");
    println!("работа растёт, а обращение стоит столько же — до какого предела.\n");
    println!(
        "{:>9} {:>16} {:>14} {:>16}",
        "секторов", "на видеокарте", "стенка, мкс", "на сектор, нс"
    );
    for sec in [1u32, 16, DIRTY, 600, 1500, max_sectors] {
        let (w, d) = run(sec, 48);
        println!(
            "{sec:>9} {:>12.1} мкс {:>14.1} {:>16.0}",
            d / 1000.0,
            w / 1000.0,
            d / sec as f64
        );
    }

    section("3. Пачкой: четыре тика в одной отправке");
    println!("150 секторов каждый тик против 600 раз в четыре тика.\n");
    let (w1, _) = run(DIRTY, 48);
    let (w4, _) = run(DIRTY * 4, 48);
    println!("каждый тик:     стенка {:>7.1} мкс на тик", w1 / 1000.0);
    println!(
        "раз в 4 тика:   стенка {:>7.1} мкс на тик (пачка {:.1} мкс)",
        w4 / 4000.0,
        w4 / 1000.0
    );
    println!(
        "выигрыш {:.2}x, ценой отставания поля на 4 тика",
        w1 / (w4 / 4.0)
    );

    section("4. Сверка с процессором");
    println!("живёт тестом: `cargo test --release -p gpu`");
    println!("({} ячеек в 64 секторах, побитово)", 64 * N);
}
