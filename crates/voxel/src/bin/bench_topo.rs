//! Каскад связности ADR-055 в числах.
//!
//! Запуск: `cargo run --release -p topo --bin bench_topo`
//!
//! Главное здесь — строка бюджета «локальный пересчёт связности внутри
//! чанка», помеченная как слабое звено сметы: при 5 мкс каскад стоит
//! 0.1 мс, при 100 мкс — полмиллисекунды и доминирует.

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use voxel::topo::scene::{build, Scene};
use voxel::topo::stage_a::{Adjacency};
use voxel::topo::stage_b::{probe, probe_layered, FloodBufs, Verdict};
use voxel::topo::{reference_labels, Chunk, Components, CS};

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn time_us(mut f: impl FnMut() -> u64, reps: usize) -> f64 {
    black_box(f());
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut acc = 0u64;
        for _ in 0..reps {
            acc = acc.wrapping_add(f());
        }
        black_box(acc);
        best = best.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
    }
    best
}

fn solid_cells(c: &Chunk) -> Vec<(u8, u8, u8)> {
    let mut v = Vec::new();
    for z in 0..CS {
        for y in 0..CS {
            for x in 0..CS {
                if c.get(x, y, z) {
                    v.push((x as u8, y as u8, z as u8));
                }
            }
        }
    }
    v
}

fn main() {
    println!("чанк {CS}³, колонка по X — один u32");

    section("1. Локальный пересчёт связности внутри чанка");
    println!("та самая строка «не определён», названная слабым звеном сметы.\n");
    println!(
        "{:<18} {:>9} {:>8} {:>8} {:>12} {:>14} {:>10}",
        "сцена", "твёрдых", "пробегов", "компонент", "пробеги, мкс", "по вокселям, мкс", "выигрыш"
    );
    let mut comp = Components::new();
    for scene in Scene::ALL {
        let c = build(scene, 42);
        let n = comp.build(&c);
        let runs = comp.runs.len();
        let fast = time_us(|| comp.build(&c) as u64, 2000);
        let slow = time_us(
            || reference_labels(&c).iter().filter(|&&v| v >= 0).count() as u64,
            200,
        );
        println!(
            "{:<18} {:>9} {:>8} {:>8} {:>12.2} {:>14.1} {:>9.1}x",
            scene.name(),
            c.solid_count(),
            runs,
            n,
            fast,
            slow,
            slow / fast
        );
    }
    println!("(«по вокселям» — эталонный обход 32³, он же то, что делают наивные реализации)");

    section("2. Ступень A: частота срабатывания и цена");
    println!("записанное в ADR-055: пропускает 0.23% правок, цена O(1).\n");
    let adj = Adjacency::new();
    println!(
        "{:<18} {:>10} {:>14} {:>14} {:>12}",
        "сцена", "правок", "прошло дальше", "доля, %", "нс на правку"
    );
    let mut total_edits = 0usize;
    let mut total_passed = 0usize;
    for scene in Scene::ALL {
        let c = build(scene, 42);
        let cells = solid_cells(&c);
        if cells.is_empty() {
            continue;
        }
        let mut passed = 0usize;
        for &(x, y, z) in cells.iter() {
            let m = adj.gather(&c, x as usize, y as usize, z as usize);
            if !adj.cannot_split(m) {
                passed += 1;
            }
        }
        let ns = time_us(
            || {
                let mut acc = 0u64;
                for &(x, y, z) in cells.iter() {
                    let m = adj.gather(&c, x as usize, y as usize, z as usize);
                    acc += adj.cannot_split(m) as u64;
                }
                acc
            },
            20,
        ) * 1000.0
            / cells.len() as f64;
        total_edits += cells.len();
        total_passed += passed;
        println!(
            "{:<18} {:>10} {:>14} {:>13.2}% {:>12.1}",
            scene.name(),
            cells.len(),
            passed,
            100.0 * passed as f64 / cells.len() as f64,
            ns
        );
    }
    println!(
        "\nпо всем сценам: {} правок, прошло {} — {:.2}%",
        total_edits,
        total_passed,
        100.0 * total_passed as f64 / total_edits as f64
    );

    section("3. Ступень B: сколько клеток обходит двусторонний флуд");
    println!("строгое чередование по клетке против чередования по слоям.\n");
    let mut bufs = FloodBufs::new();
    println!(
        "{:<18} {:>10} {:>9} {:>9} {:>9} {:>12} {:>12}",
        "сцена", "событий", "среднее", "медиана", "p99", "по слоям ср.", "мкс/событие"
    );
    for scene in Scene::ALL {
        let c = build(scene, 42);
        let cells = solid_cells(&c);
        if cells.is_empty() {
            continue;
        }
        // Ступень B зовётся только для тех, кого пропустила A.
        let survivors: Vec<(u8, u8, u8)> = cells
            .iter()
            .copied()
            .filter(|&(x, y, z)| !adj.cannot_split(adj.gather(&c, x as usize, y as usize, z as usize)))
            .collect();
        if survivors.is_empty() {
            println!("{:<18} {:>10}", scene.name(), 0);
            continue;
        }
        let mut strict: Vec<u32> = Vec::with_capacity(survivors.len());
        let mut layered_sum = 0u64;
        for &(x, y, z) in survivors.iter() {
            let mut cut = c.clone();
            cut.set(x as usize, y as usize, z as usize, false);
            let (_, t) = probe(&cut, &mut bufs, x as usize, y as usize, z as usize, 512);
            strict.push(t);
            let (_, tl) = probe_layered(&cut, &mut bufs, x as usize, y as usize, z as usize, 512);
            layered_sum += tl as u64;
        }
        strict.sort_unstable();
        let mean = strict.iter().map(|&v| v as u64).sum::<u64>() as f64 / strict.len() as f64;
        let median = strict[strict.len() / 2];
        let p99 = strict[(strict.len() * 99 / 100).min(strict.len() - 1)];
        let us = time_us(
            || {
                let mut acc = 0u64;
                for &(x, y, z) in survivors.iter() {
                    let (_, t) = probe(&c, &mut bufs, x as usize, y as usize, z as usize, 512);
                    acc += t as u64;
                }
                acc
            },
            20,
        ) / survivors.len() as f64;
        println!(
            "{:<18} {:>10} {:>9.1} {:>9} {:>9} {:>12.1} {:>12.3}",
            scene.name(),
            survivors.len(),
            mean,
            median,
            p99,
            layered_sum as f64 / survivors.len() as f64,
            us
        );
    }

    section("4. Вердикты: что вообще происходит после ступени A");
    println!(
        "{:<18} {:>12} {:>12} {:>12}",
        "сцена", "связано", "отвал", "потолок"
    );
    for scene in Scene::ALL {
        let c = build(scene, 42);
        let cells = solid_cells(&c);
        let (mut conn, mut det, mut ovf) = (0usize, 0usize, 0usize);
        for &(x, y, z) in cells.iter() {
            if adj.cannot_split(adj.gather(&c, x as usize, y as usize, z as usize)) {
                continue;
            }
            let mut cut = c.clone();
            cut.set(x as usize, y as usize, z as usize, false);
            match probe(&cut, &mut bufs, x as usize, y as usize, z as usize, 512).0 {
                Verdict::Connected => conn += 1,
                Verdict::Detached { .. } => det += 1,
                Verdict::Overflow => ovf += 1,
            }
        }
        println!("{:<18} {conn:>12} {det:>12} {ovf:>12}", scene.name());
    }

    section("5. Смета каскада на тик");
    let c = build(Scene::TunnelAndScatter, 42);
    let cells = solid_cells(&c);
    let survivors: Vec<(u8, u8, u8)> = cells
        .iter()
        .copied()
        .filter(|&(x, y, z)| !adj.cannot_split(adj.gather(&c, x as usize, y as usize, z as usize)))
        .collect();
    let rate = survivors.len() as f64 / cells.len() as f64;
    let a_ns = time_us(
        || {
            let mut acc = 0u64;
            for &(x, y, z) in cells.iter() {
                acc += adj.cannot_split(adj.gather(&c, x as usize, y as usize, z as usize)) as u64;
            }
            acc
        },
        20,
    ) * 1000.0
        / cells.len() as f64;
    let b_us = time_us(
        || {
            let mut acc = 0u64;
            for &(x, y, z) in survivors.iter() {
                acc += probe(&c, &mut bufs, x as usize, y as usize, z as usize, 512).1 as u64;
            }
            acc
        },
        20,
    ) / survivors.len().max(1) as f64;
    let ccl_us = time_us(|| comp.build(&c) as u64, 2000);
    println!("сцена «ход плюс обстрел»: ступень A {a_ns:.1} нс/правку, доля прошедших {:.2}%, ступень B {b_us:.3} мкс/событие, пересчёт компонент {ccl_us:.2} мкс\n", 100.0 * rate);
    println!(
        "{:>14} {:>14} {:>14} {:>16} {:>12}",
        "правок за тик", "A, мс", "B, мс", "грязных чанков", "всего, мс"
    );
    for edits in [600usize, 2000, 8000, 20000] {
        let a_ms = a_ns * edits as f64 / 1e6;
        let b_ms = b_us * (edits as f64 * rate) / 1e3;
        // Грязных чанков грубо: правки кучкуются, оценим по 60 правок на чанк.
        let chunks = (edits / 60).max(1);
        let c_ms = ccl_us * chunks as f64 / 1e3;
        println!(
            "{edits:>14} {a_ms:>14.3} {b_ms:>14.3} {:>16.3} {:>12.3}",
            c_ms,
            a_ms + b_ms + c_ms
        );
    }
    println!("(«грязных чанков» — пересчёт локальных компонент, по 60 правок на чанк)");
    println!("записанная в ADR-055 оценка устойчивого режима при 2000 правках: 0.1–0.5 мс");

    // Ссылка на Rng, чтобы генератор сцен не выглядел неиспользуемым.
    let _ = Rng::new(1, 1);
}
