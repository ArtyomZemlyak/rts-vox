//! Форма забоя против числа квадов (Q-17, ADR-069).
//!
//! Запуск: `cargo run --release -p harness --bin bench_cut`
//!
//! Записанное «до шести раз» снято с **шахматки** — синтетического худшего
//! случая, где ни один квад не сливается ни с одним. Забой не шахматка,
//! и разница между осевым и рваным забоем — это отдельное число, которое
//! до сих пор никто не мерил.
//!
//! Меряется то, что упирается в видеопамять: **квадов на чанк
//! в установившемся режиме** и, следовательно, буфер на карту.

use std::time::Instant;

use harness::{Cut, Dig, World};

const TICKS: usize = 600;

fn main() {
    println!("мир 4×4 чанка 62³, 300 копателей, {TICKS} тиков\n");
    println!(
        "{:<12} {:>12} {:>12} {:>14} {:>12} {:>14}",
        "форма", "снято", "квадов", "квадов/чанк", "к осевому", "ремеш, мкс/тик"
    );

    let mut base_quads = 0f64;
    for cut in [Cut::Boxy, Cut::Sphere, Cut::Organic] {
        let mut best_ms = f64::MAX;
        let (mut quads, mut dug) = (0usize, 0usize);
        for _ in 0..3 {
            let mut w = World::new(4, 4, 21);
            let mut dig = Dig::new(&w, 300, 2, 3);
            dig.cut = cut;
            let mut out = Vec::new();
            let mut total = 0usize;
            let t0 = Instant::now();
            for _ in 0..TICKS {
                dig.step(&mut w, &mut out);
                total += out.len();
                w.flush_mesh();
            }
            best_ms = best_ms.min(t0.elapsed().as_secs_f64() * 1e3);
            quads = w.quad_total();
            dug = total;
        }
        let per_chunk = quads as f64 / 16.0;
        if cut == Cut::Boxy {
            base_quads = per_chunk;
        }
        println!(
            "{:<12} {dug:>12} {quads:>12} {per_chunk:>14.0} {:>11.2}x {:>14.1}",
            format!("{cut:?}"),
            per_chunk / base_quads,
            best_ms * 1000.0 / TICKS as f64
        );
    }

    println!();
    println!("«снято» — сколько вокселей убрано всего: формы снимают разный объём,");
    println!("и сравнивать надо квады на снятый воксель, а не только на чанк.\n");

    // Пересчёт на карту: сколько буферов квадов при вокселе 0.5 м (ADR-094).
    println!(
        "{:<12} {:>16} {:>18} {:>18}",
        "форма", "КиБ на чанк", "2×2 км выкопана", "влезает в 8 ГБ"
    );
    for cut in [Cut::Boxy, Cut::Sphere, Cut::Organic] {
        let mut w = World::new(4, 4, 21);
        let mut dig = Dig::new(&w, 300, 2, 3);
        dig.cut = cut;
        let mut out = Vec::new();
        for _ in 0..TICKS {
            dig.step(&mut w, &mut out);
            w.flush_mesh();
        }
        let kib = w.quad_total() as f64 / 16.0 * 8.0 / 1024.0;
        // 2×2 км при вокселе 0.5 м = 4000 вокселей по стороне, глубина 128 м
        // = 256 вокселей. Чанк мешинга 62³.
        let chunks = (4000f64 / 62.0).ceil() * (4000f64 / 62.0).ceil() * (256f64 / 62.0).ceil();
        let gib = kib * chunks / 1024.0 / 1024.0;
        println!(
            "{:<12} {kib:>16.1} {:>15.2} ГиБ {:>18}",
            format!("{cut:?}"),
            gib,
            if gib < 6.0 { "да" } else { "НЕТ" }
        );
    }
    println!();
    println!("порог 6 ГиБ — 8 ГБ видеопамяти целевой карты (ADR-093) минус");
    println!("битовый объём 512 МБ, текстуры и то, что забирает драйвер.");
    println!("«выкопана» — верхняя граница: все чанки в установившемся режиме.");
}
