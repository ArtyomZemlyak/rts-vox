//! Устойчивая копка через все подсистемы сразу — главный бенчмарк этапа 0.
//!
//! Запуск: `cargo run --release -p harness --bin bench_dig`
//!
//! Три числа, до сих пор бравшиеся моделью, здесь измеряются:
//! правок на грязный чанк, доля правок с переворотом нав-клетки,
//! число грязных чанков за тик.

use std::hint::black_box;
use std::time::Instant;

use harness::{Dig, TopoLayer, World};
use meshing::CS as MCS;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

struct Run {
    ticks: usize,
    edits: u64,
    mesh_dirty: u64,
    topo_dirty: u64,
    survivors: u64,
    nav_flips: u64,
    ms_mesh: f64,
    ms_topo: f64,
    ms_nav: f64,
    quads_before: usize,
    quads_after: usize,
}

fn run(nx: usize, nz: usize, diggers: usize, radius: i32, ticks: usize, seed: u64) -> Run {
    let mut w = World::new(nx, nz, seed);
    let mut t = TopoLayer::new(&w);
    let mut dig = Dig::new(&w, diggers, radius, seed ^ 0x5A);
    let quads_before = w.quad_total();

    let mut out = Vec::new();
    let mut r = Run {
        ticks,
        edits: 0,
        mesh_dirty: 0,
        topo_dirty: 0,
        survivors: 0,
        nav_flips: 0,
        ms_mesh: 0.0,
        ms_topo: 0.0,
        ms_nav: 0.0,
        quads_before,
        quads_after: 0,
    };

    // Прогрев: первые тики снимают целину и не характерны.
    for _ in 0..10 {
        dig.step(&mut w, &mut out);
        w.flush_mesh();
        t.apply(&out);
        t.recompute();
    }

    for _ in 0..ticks {
        dig.step(&mut w, &mut out);
        r.edits += out.len() as u64;

        let t0 = Instant::now();
        let (md, _) = w.flush_mesh();
        r.ms_mesh += t0.elapsed().as_secs_f64() * 1e3;
        r.mesh_dirty += md as u64;

        let t0 = Instant::now();
        let (td, sv) = t.apply(&out);
        let td2 = t.recompute();
        r.ms_topo += t0.elapsed().as_secs_f64() * 1e3;
        r.topo_dirty += td.max(td2) as u64;
        r.survivors += sv as u64;

        let t0 = Instant::now();
        r.nav_flips += w.nav_flips(&out) as u64;
        r.ms_nav += t0.elapsed().as_secs_f64() * 1e3;
    }
    black_box(w.quad_total());
    r.quads_after = w.quad_total();
    r
}

fn main() {
    println!("мир из чанков мешинга {MCS}³, воксель 1 м; копатели снимают сферу за тик");
    println!("бюджет тика — 10 мс при 30 Гц, бюджет кадра — 16.7 мс\n");

    section("1. Устойчивая копка: сколько чего происходит за тик");
    println!(
        "{:>10} {:>8} {:>10} {:>12} {:>12} {:>14} {:>14}",
        "копателей",
        "радиус",
        "правок",
        "мешинг гряз.",
        "связн. гряз.",
        "правок/чанк",
        "переворотов"
    );
    for (nx, nz, diggers, radius) in [
        (4usize, 4usize, 50usize, 2i32),
        (4, 4, 150, 2),
        (4, 4, 300, 2),
        (4, 4, 300, 3),
        (6, 6, 600, 2),
    ] {
        let r = run(nx, nz, diggers, radius, 120, 7);
        let t = r.ticks as f64;
        println!(
            "{diggers:>10} {radius:>8} {:>10.1} {:>12.1} {:>12.1} {:>14.1} {:>14.2}",
            r.edits as f64 / t,
            r.mesh_dirty as f64 / t,
            r.topo_dirty as f64 / t,
            r.edits as f64 / r.mesh_dirty.max(1) as f64,
            r.nav_flips as f64 / t,
        );
    }
    println!("(«правок/чанк» — то, что в смете связности бралось равным шестидесяти)");

    section("2. Время: те же прогоны, по подсистемам");
    println!(
        "{:>10} {:>8} {:>12} {:>12} {:>12} {:>12} {:>14}",
        "копателей",
        "радиус",
        "мешинг мс",
        "связность мс",
        "навигация мс",
        "итого мс",
        "доля бюджета"
    );
    for (nx, nz, diggers, radius) in [
        (4usize, 4usize, 50usize, 2i32),
        (4, 4, 150, 2),
        (4, 4, 300, 2),
        (4, 4, 300, 3),
        (6, 6, 600, 2),
    ] {
        let r = run(nx, nz, diggers, radius, 120, 7);
        let t = r.ticks as f64;
        let (m, tp, nv) = (r.ms_mesh / t, r.ms_topo / t, r.ms_nav / t);
        println!(
            "{diggers:>10} {radius:>8} {m:>12.3} {tp:>12.3} {nv:>12.3} {:>12.3} {:>13.1}%",
            m + tp + nv,
            100.0 * (m + tp + nv) / 10.0
        );
    }
    println!("(мешинг — работа кадра, остальное — работа тика; сложены для верхней оценки)");

    section("3. Доля правок, переворачивающих нав-клетку");
    println!("это множитель ко всей смете видимости (ADR-072).\n");
    println!(
        "{:>10} {:>8} {:>12} {:>14} {:>12}",
        "копателей", "радиус", "правок/тик", "переворотов/тик", "доля"
    );
    for (diggers, radius) in [(50usize, 2i32), (150, 2), (300, 2), (300, 3), (600, 4)] {
        let r = run(4, 4, diggers, radius, 120, 7);
        let t = r.ticks as f64;
        println!(
            "{diggers:>10} {radius:>8} {:>12.1} {:>14.2} {:>11.2}%",
            r.edits as f64 / t,
            r.nav_flips as f64 / t,
            100.0 * r.nav_flips as f64 / r.edits.max(1) as f64
        );
    }

    section("4. Ступень A на настоящей копке");
    println!("на сгенерированных сценах она пропускала 0.01% (ADR-073).\n");
    println!(
        "{:>10} {:>8} {:>12} {:>14} {:>12}",
        "копателей", "радиус", "правок/тик", "выживших/тик", "доля"
    );
    for (diggers, radius) in [(50usize, 2i32), (300, 2), (300, 3), (600, 4)] {
        let r = run(4, 4, diggers, radius, 120, 7);
        let t = r.ticks as f64;
        println!(
            "{diggers:>10} {radius:>8} {:>12.1} {:>14.2} {:>11.3}%",
            r.edits as f64 / t,
            r.survivors as f64 / t,
            100.0 * r.survivors as f64 / r.edits.max(1) as f64
        );
    }

    section("5. Рост геометрии: во что превращается забой");
    println!(
        "{:>10} {:>8} {:>12} {:>12} {:>12} {:>14}",
        "копателей", "тиков", "квадов до", "квадов после", "рост", "КиБ буфера"
    );
    for ticks in [60usize, 240, 600] {
        let r = run(4, 4, 300, 2, ticks, 7);
        println!(
            "{:>10} {ticks:>8} {:>12} {:>12} {:>11.1}% {:>14.1}",
            300,
            r.quads_before,
            r.quads_after,
            100.0 * (r.quads_after as f64 - r.quads_before as f64) / r.quads_before as f64,
            r.quads_after as f64 * 8.0 / 1024.0
        );
    }
    println!("(квад 8 байт; мир — шестнадцать чанков 62³, то есть 248×62×248 вокселей)");
}
