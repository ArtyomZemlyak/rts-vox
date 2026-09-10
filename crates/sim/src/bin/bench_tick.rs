//! Тик под нагрузкой: 5000 сущностей с **непустыми** фазами движения и боя.
//!
//! Запуск: `cargo run --release -p sim --bin bench_tick`
//!
//! Дорожная карта требует именно этого: «прогон 5000 сущностей с пустыми
//! фазами не способен провалиться там, где должен».

use std::hint::black_box;
use std::time::Instant;

use sim::{populate, Phase, World, PHASES, RANGE, SEPARATION, WORLD};

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Прогон N тиков, возвращает миллисекунды на тик и разбивку по фазам.
fn run(n_units: usize, ticks: usize, seed: u64, spread: i32) -> (f64, [u64; 15], u64, u64, usize) {
    run_cells(n_units, ticks, seed, spread, SEPARATION, RANGE)
}

fn run_cells(
    n_units: usize,
    ticks: usize,
    seed: u64,
    spread: i32,
    near: i32,
    far: i32,
) -> (f64, [u64; 15], u64, u64, usize) {
    let mut w = World::with_cells(2, n_units * 2, near, far);
    populate_spread(&mut w, n_units, seed, spread);
    // Прогрев: индекс и кэш.
    for _ in 0..5 {
        w.tick();
    }
    w.phase_ns = [0; 15];
    w.examined_move = 0;
    w.examined_combat = 0;
    let t0 = Instant::now();
    let mut h = 0u64;
    for _ in 0..ticks {
        h = w.tick();
    }
    black_box(h);
    let ms = t0.elapsed().as_secs_f64() * 1e3 / ticks as f64;
    let live = w.units.live().len();
    (ms, w.phase_ns, w.examined_move, w.examined_combat, live)
}

/// Заселение с управляемой плотностью: разброс задаёт, насколько тесно стоят.
fn populate_spread(w: &mut World, n: usize, seed: u64, spread: i32) {
    let mut r = fx::rng::Rng::new(seed, 0xA1);
    for k in 0..n {
        let owner = (k % 2) as u8;
        let (bx, bz) = if owner == 0 {
            (WORLD / 4, WORLD / 4)
        } else {
            (WORLD * 3 / 4, WORLD * 3 / 4)
        };
        let x = (bx + r.range(-spread, spread)).clamp(0, WORLD - 1);
        let z = (bz + r.range(-spread, spread)).clamp(0, WORLD - 1);
        w.units.spawn(x, z, owner, r.range(2, 6), 300);
    }
    w.fill_flow_towards(0, WORLD * 3 / 4, WORLD * 3 / 4);
    w.fill_flow_towards(1, WORLD / 4, WORLD / 4);
    w.units.refresh_live();
    let _ = populate;
}

fn main() {
    println!(
        "мир {WORLD}×{WORLD} мировых единиц, радиус расталкивания {SEPARATION}, дальность {RANGE}"
    );
    println!("бюджет тика — 10 мс при 30 Гц\n");

    section("1. Время тика по числу сущностей");
    println!(
        "{:>10} {:>12} {:>14} {:>16} {:>16} {:>12}",
        "сущностей", "мс на тик", "доля бюджета", "осмотров/движ.", "осмотров/бой", "на сущность"
    );
    for n in [500usize, 1000, 2500, 5000, 10000] {
        let (ms, _, em, ec, _) = run(n, 200, 42, 150);
        println!(
            "{n:>10} {ms:>12.3} {:>13.1}% {:>16.1} {:>16.1} {:>11.2}",
            100.0 * ms / 10.0,
            em as f64 / 200.0 / n as f64,
            ec as f64 / 200.0 / n as f64,
            ms * 1000.0 / n as f64
        );
    }
    println!("(«осмотров» — сколько соседей фаза перебрала на сущность за тик)");

    section("2. Разбивка по фазам, 5000 сущностей");
    let (ms, phases, _, _, live) = run(5000, 200, 42, 150);
    println!("живых к концу прогона: {live}\n");
    println!("{:<16} {:>12} {:>12}", "фаза", "мкс на тик", "доля");
    let total: u64 = phases.iter().sum();
    for (i, &p) in PHASES.iter().enumerate() {
        let us = phases[i] as f64 / 200.0 / 1000.0;
        if us < 0.001 && p != Phase::Movement && p != Phase::Combat {
            continue;
        }
        println!(
            "{:<16} {us:>12.2} {:>11.1}%",
            p.name(),
            100.0 * phases[i] as f64 / total.max(1) as f64
        );
    }
    println!("{:<16} {:>12.2}", "итого", ms * 1000.0);
    println!("(одиннадцать фаз-заглушек занимают нули и потому не показаны)");

    section("3. Плотность решает больше, чем количество");
    println!("одно и то же число сущностей, разный разброс по карте.\n");
    println!(
        "{:>10} {:>10} {:>12} {:>16} {:>16}",
        "сущностей", "разброс", "мс на тик", "осмотров/движ.", "осмотров/бой"
    );
    for spread in [600i32, 300, 150, 75, 40] {
        let (ms, _, em, ec, _) = run(5000, 100, 42, spread);
        println!(
            "{:>10} {spread:>10} {ms:>12.3} {:>16.1} {:>16.1}",
            5000,
            em as f64 / 100.0 / 5000.0,
            ec as f64 / 100.0 / 5000.0
        );
    }
    println!("(разброс — половина стороны квадрата, в котором стоит армия)");

    section("4. Чем это было бы без индекса");
    // Полный перебор пар — то, что получается, если индекс не строить.
    for n in [500usize, 2500, 5000] {
        let pairs = (n as f64) * (n as f64);
        println!(
            "{n:>6} сущностей: полный перебор — {:.1} млн пар за тик, {:.1} мс при 1 нс на пару",
            pairs / 1e6,
            pairs / 1e6
        );
    }


    section("5. Размер ведра индекса: обмен между пересборкой и запросом");
    println!("5000 сущностей, разброс 150. Ведро дальнего индекса — 24.\n");
    println!(
        "{:>12} {:>14} {:>14} {:>14} {:>14} {:>16}",
        "ведро", "корзин", "nav_update мкс", "movement мкс", "итого мс", "осмотров/движ."
    );
    for near in [3i32, 6, 12, 24, 48] {
        let (ms, phases, em, _, _) = run_cells(5000, 100, 42, 150, near, RANGE);
        let buckets = ((WORLD / near) as usize + 1).pow(2);
        println!(
            "{near:>12} {buckets:>14} {:>14.1} {:>14.1} {ms:>14.3} {:>16.1}",
            phases[6] as f64 / 100.0 / 1000.0,
            phases[7] as f64 / 100.0 / 1000.0,
            em as f64 / 100.0 / 5000.0
        );
    }
    println!("(ведро 3 — радиус расталкивания; дальше запрос дорожает, пересборка дешевеет)");

    section("6. Тот же обмен на плотной толпе");
    println!("5000 сущностей, разброс 40 — армия стоит вплотную.\n");
    println!(
        "{:>12} {:>14} {:>14} {:>14} {:>16}",
        "ведро", "nav_update мкс", "movement мкс", "итого мс", "осмотров/движ."
    );
    for near in [3i32, 6, 12, 24] {
        let (ms, phases, em, _, _) = run_cells(5000, 100, 42, 40, near, RANGE);
        println!(
            "{near:>12} {:>14.1} {:>14.1} {ms:>14.3} {:>16.1}",
            phases[6] as f64 / 100.0 / 1000.0,
            phases[7] as f64 / 100.0 / 1000.0,
            em as f64 / 100.0 / 5000.0
        );
    }

    section("7. Детерминизм");
    let a = {
        let mut w = World::new(2, 8192);
        populate_spread(&mut w, 4000, 99, 150);
        let mut h = 0;
        for _ in 0..120 {
            h = w.tick();
        }
        h
    };
    let b = {
        let mut w = World::new(2, 8192);
        populate_spread(&mut w, 4000, 99, 150);
        let mut h = 0;
        for _ in 0..120 {
            h = w.tick();
        }
        h
    };
    println!("два прогона одного сида на 120 тиков: {a:#018x} и {b:#018x} — {}",
        if a == b { "совпали" } else { "РАЗОШЛИСЬ" });
}
