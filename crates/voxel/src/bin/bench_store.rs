//! Хранение: запись в row-packed против плотного, память, копирование
//! в память игрока (ADR-024, ADR-029).
//!
//! Запуск: `cargo run --release -p store --bin bench_store`

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use voxel::store::{Dense, Paletted, RowPacked, CS, ROWS};

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Рельеф со слоями — то, что лежит в чанке на самом деле.
fn scene(seed: u64) -> Dense {
    let mut d = Dense::new();
    let mut r = Rng::new(seed, 1);
    for z in 0..CS {
        for x in 0..CS {
            let h = 12 + (r.below(10) as usize);
            for y in 0..h.min(CS) {
                let m = if y + 2 >= h {
                    1
                } else if r.below(64) == 0 {
                    3
                } else {
                    2
                };
                d.set(x, y, z, m);
            }
        }
    }
    d
}

fn main() {
    println!("чанк {CS}³, строка — 32 воксела вдоль X, {ROWS} строк\n");

    section("1. Память");
    println!(
        "{:<24} {:>13} {:>13} {:>13} {:>14}",
        "содержимое", "плотный", "построчный", "палитра чанка", "экономия палитры"
    );
    for (name, d) in [
        ("пустой", Dense::new()),
        ("рельеф со слоями", scene(1)),
        ("сплошная порода", {
            let mut d = Dense::new();
            for i in d.v.iter_mut() {
                *i = 2;
            }
            d
        }),
        ("шум из шести материалов", {
            let mut d = Dense::new();
            let mut r = Rng::new(5, 5);
            for i in d.v.iter_mut() {
                *i = r.below(6) as u16;
            }
            d
        }),
    ] {
        let p = RowPacked::from_dense(&d);
        let q = Paletted::from_dense(&d);
        println!(
            "{name:<24} {:>11} Б {:>11} Б {:>11} Б {:>13.2}x",
            d.bytes(),
            p.bytes(),
            q.bytes(),
            d.bytes() as f64 / q.bytes() as f64
        );
    }
    println!("(в построчный включены 16 байт заголовка строки: палитра, длина, ширина)");
    println!("(палитра чанка: 4 бита на воксель до шестнадцати материалов, дальше 8)");

    section("2. Чтение");
    let d = scene(2);
    let p = RowPacked::from_dense(&d);
    let mut r = Rng::new(7, 7);
    let probes: Vec<(usize, usize, usize)> = (0..4096)
        .map(|_| {
            (
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
            )
        })
        .collect();

    let time = |mut f: Box<dyn FnMut() -> u64>, reps: usize| -> f64 {
        black_box(f());
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for _ in 0..reps {
                acc = acc.wrapping_add(f());
            }
            black_box(acc);
            best = best.min(t0.elapsed().as_secs_f64() * 1e9 / (reps * probes.len()) as f64);
        }
        best
    };

    let dense_read = {
        let d = &d;
        let probes = &probes;
        time(
            Box::new(move || {
                let mut a = 0u64;
                for &(x, y, z) in probes.iter() {
                    a += d.get(x, y, z) as u64;
                }
                a
            }),
            2000,
        )
    };
    let packed_read = {
        let p = &p;
        let probes = &probes;
        time(
            Box::new(move || {
                let mut a = 0u64;
                for &(x, y, z) in probes.iter() {
                    a += p.get(x, y, z) as u64;
                }
                a
            }),
            2000,
        )
    };
    let q = Paletted::from_dense(&d);
    let pal_read = {
        let q = &q;
        let probes = &probes;
        time(
            Box::new(move || {
                let mut a = 0u64;
                for &(x, y, z) in probes.iter() {
                    a += q.get(x, y, z) as u64;
                }
                a
            }),
            2000,
        )
    };
    println!("случайное чтение: плотный {dense_read:.2} нс, построчный {packed_read:.2} нс (x{:.2}), палитра чанка {pal_read:.2} нс (x{:.2})", packed_read / dense_read, pal_read / dense_read);

    // Последовательное чтение строки — то, что делает мешинг и марш.
    let dense_row = {
        let d = &d;
        time(
            Box::new(move || {
                let mut a = 0u64;
                for y in 0..CS {
                    for z in 0..CS {
                        for x in 0..CS {
                            a += d.get(x, y, z) as u64;
                        }
                    }
                }
                a
            }),
            60,
        ) * probes.len() as f64
            / (CS * CS * CS) as f64
    };
    let packed_row = {
        let p = &p;
        time(
            Box::new(move || {
                let mut a = 0u64;
                for y in 0..CS {
                    for z in 0..CS {
                        for x in 0..CS {
                            a += p.get(x, y, z) as u64;
                        }
                    }
                }
                a
            }),
            60,
        ) * probes.len() as f64
            / (CS * CS * CS) as f64
    };
    println!("подряд по строкам: плотный {dense_row:.2} нс, упакованный {packed_row:.2} нс — x{:.2}", packed_row / dense_row);

    section("3. Запись — то, ради чего этот замер и делался");
    println!(
        "{:<26} {:>13} {:>13} {:>13} {:>12} {:>12}",
        "нагрузка", "плотный", "построчный", "палитра", "палитра/плотн", "перепаковок"
    );
    for (name, edits, mats) in [
        ("копка: снятие в пустоту", 20_000usize, 1u32),
        ("бой: два материала", 20_000, 2),
        ("стройка: шесть материалов", 20_000, 6),
    ] {
        let base = scene(3);
        let mut r = Rng::new(11, mats as u64);
        let cells: Vec<(usize, usize, usize, u16)> = (0..edits)
            .map(|_| {
                (
                    r.below(CS as u32) as usize,
                    r.below(CS as u32) as usize,
                    r.below(CS as u32) as usize,
                    if mats == 1 { 0 } else { r.below(mats) as u16 },
                )
            })
            .collect();

        let mut dd = base.clone();
        let mut best_d = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            for &(x, y, z, m) in cells.iter() {
                dd.set(x, y, z, m);
            }
            black_box(dd.get(0, 0, 0));
            best_d = best_d.min(t0.elapsed().as_secs_f64() * 1e9 / cells.len() as f64);
        }

        let mut pp = RowPacked::from_dense(&base);
        let mut best_p = f64::MAX;
        let mut repacks = 0u64;
        for _ in 0..5 {
            let mut fresh = RowPacked::from_dense(&base);
            let t0 = Instant::now();
            for &(x, y, z, m) in cells.iter() {
                fresh.set(x, y, z, m);
            }
            black_box(fresh.get(0, 0, 0));
            best_p = best_p.min(t0.elapsed().as_secs_f64() * 1e9 / cells.len() as f64);
            repacks = fresh.repacks;
            pp = fresh;
        }
        black_box(pp.bytes());
        let mut best_q = f64::MAX;
        let mut widenings = 0u64;
        for _ in 0..5 {
            let mut fresh = Paletted::from_dense(&base);
            let t0 = Instant::now();
            for &(x, y, z, m) in cells.iter() {
                fresh.set(x, y, z, m);
            }
            black_box(fresh.get(0, 0, 0));
            best_q = best_q.min(t0.elapsed().as_secs_f64() * 1e9 / cells.len() as f64);
            widenings = fresh.widenings;
        }
        let _ = widenings;
        println!(
            "{name:<26} {best_d:>13.2} {best_p:>13.2} {best_q:>13.2} {:>11.2}x {repacks:>12}",
            best_q / best_d
        );
    }

    section("4. Копирование чанка в память игрока (ADR-029)");
    let src = scene(4);
    let mut dst = Dense::new();
    let mut best = f64::MAX;
    for _ in 0..20 {
        let t0 = Instant::now();
        let reps = 2000;
        for _ in 0..reps {
            dst.v.copy_from_slice(&src.v);
            black_box(dst.v[0]);
        }
        best = best.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
    }
    let packed = RowPacked::from_dense(&src);
    let mut best_p = f64::MAX;
    for _ in 0..20 {
        let t0 = Instant::now();
        let reps = 2000;
        for _ in 0..reps {
            let c = packed.clone();
            black_box(&c);
        }
        best_p = best_p.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
    }
    let pal = Paletted::from_dense(&src);
    let mut best_q = f64::MAX;
    for _ in 0..20 {
        let t0 = Instant::now();
        let reps = 2000;
        for _ in 0..reps {
            let c = pal.clone();
            black_box(&c);
        }
        best_q = best_q.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
    }
    println!("копия плотного чанка ({} КиБ): {best:.2} мкс", src.bytes() / 1024);
    println!("копия построчного ({} КиБ): {best_p:.2} мкс — тысяча аллокаций на чанк", packed.bytes() / 1024);
    println!("копия с палитрой чанка ({} КиБ): {best_q:.2} мкс", pal.bytes() / 1024);
    println!();
    println!(
        "{:>16} {:>16} {:>16} {:>16}",
        "чанков за тик", "плотный, мс", "палитра, мс", "доля бюджета"
    );
    for n in [10usize, 50, 200, 1000] {
        println!(
            "{n:>16} {:>16.3} {:>16.3} {:>15.1}%",
            best * n as f64 / 1000.0,
            best_q * n as f64 / 1000.0,
            100.0 * best_q * n as f64 / 1000.0 / 10.0
        );
    }
    println!("\n(ADR-029: чанк копируется в память игрока при входе в обзор;");
    println!(" число таких чанков за тик и есть открытая часть этой строки)");
}
