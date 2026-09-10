//! Рост памяти игрока о мире: сценарий «противник копает всю партию вне
//! обзора» (Q-49, ADR-029).
//!
//! Запуск: `cargo run --release -p harness --bin bench_memory`
//!
//! Память игрока — копия мира, какой он её видел. Пока противник копает вне
//! обзора, копия расходится с истиной. Вопрос не в том, сколько стоит копия,
//! а в том, **как быстро растёт число разошедшихся чанков** и когда потолок
//! запомненного придётся вводить.

use std::hint::black_box;

use fx::rng::Rng;
use harness::{Dig, World};
use mesh::CS as MCS;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Чанк 32³ с палитрой на чанк — 16 КиБ (ADR-081). Чанк мешинга 62³
/// покрывает восемь таких.
const KIB_PER_MESH_CHUNK: f64 = 8.0 * 16.0;

fn main() {
    println!("память игрока — копия мира; расхождение растёт по мере копки вне обзора\n");

    section("1. Сколько чанков разошлось за время партии");
    println!("карта 16×16 чанков мешинга (992 м, 256 чанков), тик 30 Гц.\n");
    println!(
        "{:>10} {:>10} {:>10} {:>14} {:>14} {:>16}",
        "копателей", "забоев", "секунд", "разошлось", "доля мира", "диф, МиБ"
    );
    for (diggers, clusters) in [(50usize, 2usize), (300, 4), (300, 64), (1000, 16)] {
        for secs in [5usize, 30, 120] {
            let mut w = World::new(16, 16, 5);
            let total = w.chunk_count();
            let mut dig = Dig::clustered(&w, diggers, 2, 3, clusters);
            let mut out = Vec::new();
            let mut diverged = vec![false; total];
            let ticks = secs * 30;
            for _ in 0..ticks {
                dig.step(&mut w, &mut out);
                for &(x, _, z) in out.iter() {
                    diverged[(z / MCS) * w.nx + (x / MCS)] = true;
                }
                w.flush_mesh();
            }
            black_box(w.quad_total());
            let n = diverged.iter().filter(|&&v| v).count();
            println!(
                "{diggers:>10} {clusters:>10} {secs:>10} {n:>14} {:>13.0}% {:>16.1}",
                100.0 * n as f64 / total as f64,
                n as f64 * KIB_PER_MESH_CHUNK / 1024.0
            );
        }
    }
    println!("(«разошлось» — чанков, где истина отличается от того, что игрок видел)");

    section("2. Насколько глубоко разошлось: не только сколько чанков, но и сколько правок");
    println!(
        "{:>10} {:>10} {:>14} {:>16} {:>16}",
        "секунд", "разошлось", "правок всего", "правок на чанк", "если помнить всё"
    );
    let mut w = World::new(16, 16, 5);
    let total = w.chunk_count();
    let mut dig = Dig::clustered(&w, 300, 2, 3, 4);
    let mut out = Vec::new();
    let mut diverged = vec![0u32; total];
    let mut edits = 0u64;
    for sec in 1..=120usize {
        for _ in 0..30 {
            dig.step(&mut w, &mut out);
            edits += out.len() as u64;
            for &(x, _, z) in out.iter() {
                diverged[(z / MCS) * w.nx + (x / MCS)] += 1;
            }
            w.flush_mesh();
        }
        if [5usize, 15, 30, 60, 120].contains(&sec) {
            let n = diverged.iter().filter(|&&v| v > 0).count();
            println!(
                "{sec:>10} {n:>10} {edits:>14} {:>16.0} {:>15.1}М",
                edits as f64 / n.max(1) as f64,
                total as f64 * KIB_PER_MESH_CHUNK / 1024.0
            );
        }
    }
    println!("(«если помнить всё» — вся карта в памяти игрока при палитре на чанк)");

    section("3. Что это значит для потолка запомненного");
    let per_player = total as f64 * KIB_PER_MESH_CHUNK / 1024.0;
    println!("вся карта 992 м в памяти одного игрока: {per_player:.1} МиБ");
    println!();
    println!("{:>10} {:>18} {:>18}", "игроков", "если помнить всё", "плюс истина");
    for players in [2usize, 4, 8] {
        println!(
            "{players:>10} {:>17.1}М {:>17.1}М",
            per_player * players as f64,
            per_player * (players + 1) as f64
        );
    }
    println!();
    println!("на карте 2 км с полной высотой (256) то же самое — 512 МиБ на игрока,");
    println!("то есть при восьми игроках 4.6 ГиБ. Вот там потолок и нужен.");

    let mut r = Rng::new(1, 1);
    black_box(r.next_u32());
}
