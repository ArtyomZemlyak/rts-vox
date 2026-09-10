//! Инвалидация видимости от копки — вторая половина ADR-070.
//!
//! Запуск: `cargo run --release -p nav --bin bench_vision_dig`
//!
//! Мерится не «правка воксела», а **переворот непрозрачности нав-клетки**:
//! нав-сетка грубее воксельной именно затем, чтобы снятие одного воксела
//! не меняло граф. Цена подаётся как функция числа перевёрнутых клеток —
//! это устойчивая часть. Правдоподобное число клеток берётся из модели
//! копки и помечено как модель.

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use nav::godunov::BLOCKED;
use nav::scene::{build, Grid, Scene};
use nav::vision::*;
use nav::vision_incr::{insert, refresh, CountGrid, LocalMask};
use nav::vision_index::SensorIndex;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn make_sensors(g: &Grid, n: usize, radius: u16, seed: u64) -> Vec<Sensor> {
    let mut r = Rng::new(seed, 0x5150);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let x = r.below(g.w as u32 - 2) + 1;
        let y = r.below(g.h as u32 - 2) + 1;
        if g.cost[y as usize * g.w + x as usize] == BLOCKED {
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

/// Клетки, перевёрнутые копкой за тик. Копают не по всей карте: правки
/// кучкуются вокруг забоев, и каждый копатель сам является сенсором.
fn flipped_cells(g: &Grid, sensors: &[Sensor], diggers: usize, per_digger: usize, seed: u64) -> Vec<(usize, usize)> {
    let mut r = Rng::new(seed, 0xD16);
    let mut out = Vec::with_capacity(diggers * per_digger);
    for k in 0..diggers {
        // Копатель — один из сенсоров: забой там, где стоит юнит.
        let s = sensors[(k * 7919) % sensors.len()];
        for _ in 0..per_digger {
            let x = (s.x as i32 + r.range(-2, 2)).clamp(1, g.w as i32 - 2) as usize;
            let y = (s.y as i32 + r.range(-2, 2)).clamp(1, g.h as i32 - 2) as usize;
            out.push((x, y));
        }
    }
    out
}

fn main() {
    let n = 1024usize;
    let radius = 14u16;
    let g = build(Scene::Surface, n, 7);
    let sensors = make_sensors(&g, 5000, radius, 11);
    let mut idx = SensorIndex::new(g.w, g.h, radius as usize, sensors.len());

    println!("нав-сетка {}×{}, 5000 сенсоров, радиус {radius} клеток", g.w, g.h);

    section("1. Пересборка индекса сенсоров");
    let mut best = f64::MAX;
    for _ in 0..20 {
        let t0 = Instant::now();
        idx.rebuild(&sensors);
        best = best.min(t0.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "полная пересборка на 5000 сенсоров: {best:.4} мс на тик (счётная сортировка, CSR)"
    );

    section("2. Цена как функция числа перевёрнутых клеток");
    println!("это устойчивая часть замера: она не зависит от модели копки.\n");
    println!(
        "{:>10} {:>12} {:>12} {:>14} {:>14} {:>12}",
        "клеток", "затронуто", "проверено", "запрос, мс", "пересчёт, мс", "всего, мс"
    );
    let mut mask = VisMask::new(g.w, g.h);
    for &(diggers, per_digger) in &[
        (10usize, 1usize),
        (50, 1),
        (100, 2),
        (300, 2),
        (800, 4),
        (2000, 4),
    ] {
        let cells = flipped_cells(&g, &sensors, diggers, per_digger, 5);
        idx.rebuild(&sensors);

        let mut touched = Vec::new();
        let mut examined = 0u64;
        let mut q = f64::MAX;
        for _ in 0..10 {
            let t0 = Instant::now();
            idx.begin();
            touched.clear();
            let mut e = 0u64;
            for &(x, y) in cells.iter() {
                e += idx.touch(&sensors, x, y, &mut touched);
            }
            examined = e;
            q = q.min(t0.elapsed().as_secs_f64() * 1e3);
        }

        let mut rc = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &si in touched.iter() {
                acc += shadowcast(&g, sensors[si as usize], &mut mask);
            }
            black_box(acc);
            rc = rc.min(t0.elapsed().as_secs_f64() * 1e3);
        }

        println!(
            "{:>10} {:>12} {:>12} {:>14.4} {:>14.3} {:>12.3}",
            cells.len(),
            touched.len(),
            examined,
            q,
            rc,
            q + rc
        );
    }
    println!("(«проверено» — сколько сенсоров индекс осмотрел, чтобы найти затронутых)");

    section("3. Полный тик: движение плюс копка");
    println!("движение даёт двадцатую часть сенсоров (ADR-070), копка — своё сверху.\n");
    println!(
        "{:>16} {:>10} {:>12} {:>12} {:>12} {:>14}",
        "копателей", "клеток", "от копки", "всего", "мс на тик", "доля тика 33 мс"
    );
    let movers = sensors.len() / 20;
    for &(diggers, per_digger) in &[(0usize, 0usize), (100, 2), (300, 2), (800, 4), (2000, 4)] {
        let cells = if diggers == 0 {
            Vec::new()
        } else {
            flipped_cells(&g, &sensors, diggers, per_digger, 5)
        };
        idx.rebuild(&sensors);

        let mut total = f64::MAX;
        let mut from_dig = 0usize;
        let mut all = 0usize;
        for _ in 0..5 {
            let t0 = Instant::now();
            idx.begin();
            let mut touched = Vec::new();
            for &(x, y) in cells.iter() {
                idx.touch(&sensors, x, y, &mut touched);
            }
            from_dig = touched.len();
            // Сменившие клетку добавляются в тот же набор: сенсор,
            // который и двигался, и накрыт правкой, пересчитывается один раз.
            for si in 0..movers as u32 {
                idx.mark(si, &mut touched);
            }
            all = touched.len();
            let mut acc = 0u64;
            for &si in touched.iter() {
                acc += shadowcast(&g, sensors[si as usize], &mut mask);
            }
            black_box(acc);
            total = total.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        println!(
            "{:>16} {:>10} {:>12} {:>12} {:>12.3} {:>13.1}%",
            diggers,
            cells.len(),
            from_dig,
            all,
            total,
            100.0 * total / 33.3
        );
    }

    section("4. Полная конструкция: локальные маски плюс сетка счётчиков");
    println!("наивная схема неполна — она умеет добавлять видимость и не умеет снимать.");
    println!("здесь у каждого сенсора своя маска диска, а видимость группы — счётчик.\n");
    let mut cg = CountGrid::new(g.w, g.h);
    let mut masks: Vec<LocalMask> = sensors
        .iter()
        .map(|s| LocalMask::new(s.radius as usize))
        .collect();
    println!(
        "память: маска {} байт на сенсор × 5000 = {} КиБ; сетка счётчиков {} КиБ",
        masks[0].bytes(),
        masks[0].bytes() * sensors.len() / 1024,
        cg.bytes() / 1024
    );
    let t0 = Instant::now();
    for (i, &s) in sensors.iter().enumerate() {
        insert(&g, s, &mut masks[i], &mut cg);
    }
    let build_ms = t0.elapsed().as_secs_f64() * 1e3;
    println!(
        "первичная вставка всех 5000: {build_ms:.2} мс (делается один раз, не каждый тик)"
    );
    println!("видимых клеток у группы: {}", cg.count_visible());

    println!(
        "\n{:>12} {:>16} {:>16} {:>12}",
        "сенсоров", "наивно, мс", "с вычитанием, мс", "накладные"
    );
    let mut plain = VisMask::new(g.w, g.h);
    for k in [50usize, 250, 1000, 2500, 5000] {
        let mut a = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &s in sensors[..k].iter() {
                acc += shadowcast(&g, s, &mut plain);
            }
            black_box(acc);
            a = a.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        let mut b = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for i in 0..k {
                acc += refresh(&g, sensors[i], &mut masks[i], &mut cg);
            }
            black_box(acc);
            b = b.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        println!(
            "{k:>12} {a:>16.3} {b:>16.3} {:>11.2}x",
            b / a
        );
    }
    println!("(«наивно» — каст в общую маску: быстрее, но снимать видимость не умеет,");
    println!(" то есть архитектурой быть не может)");

    section("5. Полный тик в полной конструкции");
    println!(
        "{:>16} {:>10} {:>12} {:>12} {:>14}",
        "копателей", "клеток", "пересчёт", "мс на тик", "доля тика 33 мс"
    );
    for &(diggers, per_digger) in &[(0usize, 0usize), (100, 2), (300, 2), (800, 4), (2000, 4)] {
        let cells = if diggers == 0 {
            Vec::new()
        } else {
            flipped_cells(&g, &sensors, diggers, per_digger, 5)
        };
        idx.rebuild(&sensors);
        let mut total = f64::MAX;
        let mut all = 0usize;
        for _ in 0..5 {
            let t0 = Instant::now();
            idx.begin();
            let mut touched = Vec::new();
            for &(x, y) in cells.iter() {
                idx.touch(&sensors, x, y, &mut touched);
            }
            for si in 0..movers as u32 {
                idx.mark(si, &mut touched);
            }
            all = touched.len();
            let mut acc = 0u64;
            for &si in touched.iter() {
                acc += refresh(&g, sensors[si as usize], &mut masks[si as usize], &mut cg);
            }
            black_box(acc);
            total = total.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        println!(
            "{:>16} {:>10} {:>12} {:>12.3} {:>13.1}%",
            diggers,
            cells.len(),
            all,
            total,
            100.0 * total / 33.3
        );
    }

    section("6. Что если инвалидация считалась бы по правкам, а не по клеткам");
    // Правки вокселей идут на воксельной сетке 1 м, нав-клетка 2 м, и
    // подавляющее большинство правок не переворачивает непрозрачность.
    println!("нав-клетка 2 м против воксела 1 м: одна клетка — четыре колонки,");
    println!("и переворот требует снятия последней перекрывающей колонки.\n");
    println!(
        "{:>18} {:>16} {:>16} {:>16}",
        "правок за тик", "клеток при 100%", "при 10%", "при 2%"
    );
    for edits in [600usize, 2000, 8000, 20000] {
        println!(
            "{:>18} {:>16} {:>16} {:>16}",
            edits,
            edits,
            edits / 10,
            edits / 50
        );
    }
    println!("\nдоля переворотов — свойство формы забоя, здесь не измерена;");
    println!("устойчивая часть — таблица раздела 2, по числу перевёрнутых клеток.");
}
