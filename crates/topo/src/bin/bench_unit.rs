//! Связность частей юнита с ядром — строка бюджета, которую ADR-016
//! требовал измерить ещё на этапе 0.
//!
//! Запуск: `cargo run --release -p topo --bin bench_unit`

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use topo::stage_a::Adjacency;
use topo::unit::{cannot_detach, core_connectivity, core_connectivity_reference, CoreBufs, UnitVol};
use topo::unit_scene::{apply_hit, apply_hit_at, build_unit, UnitShape};

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

fn main() {
    println!("тома юнитов при отношении масштабов k=2 (ADR-019): воксель тела вдвое мельче мирового");

    let mut b = CoreBufs::new();

    section("1. Цена проверки на целом юните");
    println!(
        "{:<20} {:>10} {:>9} {:>10} {:>12} {:>14} {:>9}",
        "юнит", "вокселей", "твёрдых", "колонок", "флуд, мкс", "по вокселям, мкс", "выигрыш"
    );
    for shape in UnitShape::ALL {
        let v = build_unit(shape, 1);
        let core = shape.core();
        let rep = core_connectivity(&v, &mut b, core, 2);
        assert!(rep.core_alive, "{}: ядро в пустоте", shape.name());
        let fast = time_us(|| core_connectivity(&v, &mut b, core, 2).attached as u64, 20_000);
        let slow = time_us(|| core_connectivity_reference(&v, core, 2).0 as u64, 2_000);
        println!(
            "{:<20} {:>10} {:>9} {:>10} {:>12.3} {:>14.2} {:>8.1}x",
            shape.name(),
            v.voxels(),
            v.solid_count(),
            rep.cols_touched,
            fast,
            slow,
            slow / fast
        );
    }
    println!("(«по вокселям» — обход стеком, то, что пишут по умолчанию)");

    section("2. Цена по мере разрушения");
    println!("проверка стоит уцелевшего, а не объёма.\n");
    println!(
        "{:<20} {:>10} {:>10} {:>10} {:>12} {:>12}",
        "юнит", "попаданий", "твёрдых", "отвал", "колонок", "мкс"
    );
    for shape in UnitShape::ALL {
        for hits in [0usize, 2, 8, 20] {
            let mut v = build_unit(shape, 1);
            let core = shape.core();
            let mut r = Rng::new(7, hits as u64);
            for _ in 0..hits {
                apply_hit(&mut v, &mut r, 2);
            }
            let rep = core_connectivity(&v, &mut b, core, 2);
            if !rep.core_alive {
                println!("{:<20} {hits:>10}   ядро разрушено — связность не нужна", shape.name());
                continue;
            }
            let us = time_us(|| core_connectivity(&v, &mut b, core, 2).attached as u64, 20_000);
            println!(
                "{:<20} {hits:>10} {:>10} {:>10} {:>12} {:>12.3}",
                shape.name(),
                v.solid_count(),
                rep.detached,
                rep.cols_touched,
                us
            );
        }
    }

    section("3. Прицельный выстрел по тонкой части");
    println!("случайные попадания почти ничего не отрывают; отрывает прицельный.\n");
    println!(
        "{:<20} {:>12} {:>10} {:>10} {:>14}",
        "юнит", "снято", "отвал", "колонок", "мкс"
    );
    for shape in UnitShape::ALL {
        let Some((tx, ty, tz)) = shape.thin_part() else {
            println!("{:<20}   тонких частей нет", shape.name());
            continue;
        };
        let mut v = build_unit(shape, 1);
        let core = shape.core();
        let mut removed = Vec::new();
        let n = apply_hit_at(&mut v, tx, ty, tz, 2, &mut removed);
        let rep = core_connectivity(&v, &mut b, core, 2);
        let us = time_us(|| core_connectivity(&v, &mut b, core, 2).attached as u64, 20_000);
        println!(
            "{:<20} {n:>12} {:>10} {:>10} {:>14.3}",
            shape.name(),
            rep.detached,
            rep.cols_touched,
            us
        );
    }

    section("4. Ступень A на юните: как часто флуд можно не звать");
    println!("ступень A корректна только по одному вокселю за раз, поэтому залп");
    println!("применяется пошагово: снял воксель — проверил в текущем состоянии.\n");
    let adj = Adjacency::new();
    println!(
        "{:<20} {:>10} {:>10} {:>14} {:>12} {:>14}",
        "юнит", "залпов", "снято", "залпов с флудом", "доля, %", "нс на воксель"
    );
    for shape in UnitShape::ALL {
        let mut triggered = 0usize;
        let mut volleys = 0usize;
        let mut voxels = 0usize;
        let mut ns_total = 0.0f64;
        for seed in 0..40u64 {
            let mut v = build_unit(shape, 1);
            let core = shape.core();
            let mut r = Rng::new(seed, 3);
            for _ in 0..6 {
                let (cx, cy, cz) = (
                    r.range(0, v.nx as i32 - 1),
                    r.range(0, v.ny as i32 - 1),
                    r.range(0, v.nz as i32 - 1),
                );
                let mut removed = Vec::new();
                let n = {
                    // Пошаговое снятие с проверкой на каждом шаге.
                    let mut acc = 0u32;
                    let mut probe = Vec::new();
                    apply_hit_at(&mut v, cx, cy, cz, 2, &mut probe);
                    for &(x, y, z) in probe.iter() {
                        acc += 1;
                        removed.push((x, y, z));
                    }
                    acc
                };
                if n == 0 {
                    continue;
                }
                volleys += 1;
                voxels += n as usize;
                let t0 = Instant::now();
                let mut need_flood = false;
                for &(x, y, z) in removed.iter() {
                    if !cannot_detach(&adj, &v, x, y, z) {
                        need_flood = true;
                        break;
                    }
                }
                ns_total += t0.elapsed().as_secs_f64() * 1e9;
                if need_flood {
                    triggered += 1;
                    let rep = core_connectivity(&v, &mut b, core, 2);
                    black_box(rep.attached);
                }
                if !core_connectivity(&v, &mut b, core, 2).core_alive {
                    break;
                }
            }
        }
        println!(
            "{:<20} {volleys:>10} {voxels:>10} {triggered:>14} {:>11.1}% {:>14.1}",
            shape.name(),
            100.0 * triggered as f64 / volleys.max(1) as f64,
            ns_total / voxels.max(1) as f64
        );
    }

    section("5. Смета боя");
    let v = build_unit(UnitShape::Tank, 1);
    let core = UnitShape::Tank.core();
    let per_unit = time_us(|| core_connectivity(&v, &mut b, core, 2).attached as u64, 20_000);
    println!("проверка одного танка целиком: {per_unit:.3} мкс\n");
    println!(
        "{:>18} {:>14} {:>16} {:>16}",
        "проверок за тик", "мс на тик", "доля тика 10 мс", "на 5 воркерах"
    );
    for n in [50usize, 200, 500, 1500, 5000] {
        let ms = per_unit * n as f64 / 1000.0;
        println!(
            "{n:>18} {ms:>14.3} {:>15.1}% {:>16.3}",
            100.0 * ms / 10.0,
            ms / 5.0
        );
    }
    println!("\nADR-016: связность считается по накоплению повреждений, а не на каждое");
    println!("попадание, и ступень A снимает почти все вызовы. Верхняя строка — потолок.");

    section("6. Поток разных юнитов против повтора одного");
    let units: Vec<UnitVol> = (0..64u64).map(|s| build_unit(UnitShape::Tank, s)).collect();
    let one = time_us(
        || core_connectivity(&units[0], &mut b, core, 2).attached as u64,
        20_000,
    );
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut acc = 0u64;
        for u in units.iter() {
            acc += core_connectivity(u, &mut b, core, 2).attached as u64;
        }
        black_box(acc);
        best = best.min(t0.elapsed().as_secs_f64() * 1e6 / units.len() as f64);
    }
    println!(
        "повтор одного {one:.3} мкс, поток из 64 разных {best:.3} мкс — x{:.2}",
        best / one
    );
    println!("(том танка — 512 байт масок, он влезает в L1 в обоих случаях)");
}
