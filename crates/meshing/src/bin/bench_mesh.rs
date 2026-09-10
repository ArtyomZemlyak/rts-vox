//! Перемер греди-мешинга, которого требует `docs/10-budgets.md`.
//!
//! Запуск: `cargo run --release -p mesh --bin bench_mesh`

use std::hint::black_box;
use std::time::Instant;

use meshing::scene::{build, Scene};
use meshing::{Chunk, Mesher, Opts, CS};

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Минимум из нескольких серий: планировщик Windows шумит на десятки процентов.
fn time_us(mut f: impl FnMut() -> usize, reps: usize) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut acc = 0usize;
        for _ in 0..reps {
            acc = acc.wrapping_add(f());
        }
        black_box(acc);
        best = best.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
    }
    best
}

fn quads_and_time() {
    section("1. Квады и время на чанк 62³, паддинг и маски внутри таймера");
    println!(
        "{:<16} {:>9} {:>9} {:>10} {:>9} {:>10}",
        "сцена", "твёрдых", "квадов", "квад/тыс.", "КиБ", "мкс"
    );
    let mut m = Mesher::new();
    for scene in Scene::ALL {
        let c = build(scene, 42);
        let solid = c.solid_count();
        let st = m.mesh(&c, Opts::default());
        let reps = if st.quads > 100_000 { 40 } else { 400 };
        let us = time_us(|| m.mesh(&c, Opts::default()).quads, reps);
        println!(
            "{:<16} {:>9} {:>9} {:>10.1} {:>9.1} {:>10.1}",
            scene.name(),
            solid,
            st.quads,
            1000.0 * st.quads as f64 / solid.max(1) as f64,
            st.bytes as f64 / 1024.0,
            us
        );
    }
    println!("(«квад/тыс.» — квадов на тысячу твёрдых вокселей: мера рваности, не зависящая от заполнения)");
}

fn levers() {
    section("2. Рычаги из 04-world.md, каждый на своей сцене");
    println!(
        "{:<16} {:>9} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "сцена", "база", "без низа", "клэмп 32", "клэмп 16", "клэмп 8", "по материалам"
    );
    let mut m = Mesher::new();
    for scene in [
        Scene::Smooth,
        Scene::Layered,
        Scene::Cavern,
        Scene::LayeredCavern,
        Scene::Scatter,
        Scene::CavernAndScatter,
        Scene::Random5,
    ] {
        let c = build(scene, 42);
        let base = m.mesh(&c, Opts::default()).quads as f64;
        let mut row = vec![];
        row.push(
            m.mesh(
                &c,
                Opts {
                    skip_bottom: true,
                    ..Opts::default()
                },
            )
            .quads as f64,
        );
        for r in [32usize, 16, 8] {
            row.push(
                m.mesh(
                    &c,
                    Opts {
                        max_run: r,
                        ..Opts::default()
                    },
                )
                .quads as f64,
            );
        }
        row.push(
            m.mesh(
                &c,
                Opts {
                    per_material: true,
                    ..Opts::default()
                },
            )
            .quads as f64,
        );
        print!("{:<16} {:>9.0}", scene.name(), base);
        for v in row {
            print!(" {:>11.1}%", 100.0 * (v - base) / base);
        }
        println!();
    }
    println!("(проценты — изменение числа квадов относительно базы)");
}

fn phases() {
    section("3. Из чего состоит время: маски против слияния");
    // Разделяем, подавая мешеру пустой чанк: маски строятся всегда,
    // слияния при этом нет вовсе.
    let empty = Chunk::new();
    let mut m = Mesher::new();
    let masks_only = time_us(|| m.mesh(&empty, Opts::default()).quads, 2000);
    println!("построение масок на пустом чанке (нижняя граница фазы 1+2): {masks_only:.1} мкс");
    println!(
        "{:<16} {:>10} {:>12} {:>12} {:>10}",
        "сцена", "всего мкс", "маски мкс", "слияние мкс", "нс/квад"
    );
    for scene in Scene::ALL {
        let c = build(scene, 42);
        let st = m.mesh(&c, Opts::default());
        let reps = if st.quads > 100_000 { 40 } else { 400 };
        let total = time_us(|| m.mesh(&c, Opts::default()).quads, reps);
        let merge = (total - masks_only).max(0.0);
        println!(
            "{:<16} {:>10.1} {:>12.1} {:>12.1} {:>10.1}",
            scene.name(),
            total,
            masks_only,
            merge,
            if st.quads > 0 {
                merge * 1000.0 / st.quads as f64
            } else {
                0.0
            }
        );
    }
    println!("(маски строятся за фиксированное время: 64³ бит независимо от содержимого)");
}

fn stream() {
    section("4. Поток из разных чанков против повтора одного");
    // Тот же урок, что на эйконале: повтор одного входа греет всё, что можно.
    let mut m = Mesher::new();
    for scene in [Scene::Smooth, Scene::Cavern, Scene::CavernAndScatter] {
        let chunks: Vec<Chunk> = (0..64u64).map(|s| build(scene, 1000 + s)).collect();
        let one = time_us(|| m.mesh(&chunks[0], Opts::default()).quads, 400);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0usize;
            for c in &chunks {
                acc = acc.wrapping_add(m.mesh(c, Opts::default()).quads);
            }
            black_box(acc);
            best = best.min(t0.elapsed().as_secs_f64() * 1e6 / chunks.len() as f64);
        }
        println!(
            "{:<16} повтор одного {one:>7.1} мкс, поток из 64 разных {best:>7.1} мкс — x{:.2}",
            scene.name(),
            best / one
        );
    }
}

fn budget() {
    section("5. Что это значит для бюджета кадра");
    let mut m = Mesher::new();
    let c = build(Scene::CavernAndScatter, 42);
    let chunks: Vec<Chunk> = (0..64u64).map(|s| build(Scene::CavernAndScatter, 1000 + s)).collect();
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut acc = 0usize;
        for ch in &chunks {
            acc = acc.wrapping_add(m.mesh(ch, Opts::default()).quads);
        }
        black_box(acc);
        best = best.min(t0.elapsed().as_secs_f64() * 1e6 / chunks.len() as f64);
    }
    let bits: Vec<meshing::bits::BitVolume> = chunks.iter().map(meshing::bits::bits_from_chunk).collect();
    let mut fast = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut acc = 0usize;
        for (ch, b) in chunks.iter().zip(bits.iter()) {
            acc = acc.wrapping_add(m.mesh_from_bits(b, ch, Opts::default()).quads);
        }
        black_box(acc);
        fast = fast.min(t0.elapsed().as_secs_f64() * 1e6 / chunks.len() as f64);
    }
    let st = m.mesh(&c, Opts::default());
    println!("рабочая сцена — забой с обстрелом, {} квадов на чанк", st.quads);
    println!("честное время на чанк в потоке: {best:.1} мкс обходом байтов, {fast:.1} мкс из битового объёма");
    println!(
        "{:>10} {:>13} {:>13} {:>14} {:>14}",
        "чанков", "обход 1 пот.", "из бит 1 пот.", "из бит x5", "доля кадра"
    );
    for n in [8usize, 16, 32, 64, 115] {
        let slow_ms = best * n as f64 / 1000.0;
        let fast_ms = fast * n as f64 / 1000.0;
        let par = fast_ms / 5.0;
        println!(
            "{n:>10} {slow_ms:>13.2} {fast_ms:>13.2} {par:>14.2} {:>13.0}%",
            100.0 * par / 16.7
        );
    }
    println!("(множитель 5, а не 7: измеренная эффективность пула — см. analysis/11-eikonal-bench.md)");
}

/// 6. Маски из битового объёма вместо обхода байтов.
fn from_bits() {
    section("6. Маски из битового объёма (ADR-061) вместо обхода байтов");
    println!("квады совпадают побитово на всех сценах — тест from_bits_matches_byte_scan.");
    println!(
        "{:<16} {:>12} {:>12} {:>10} {:>12}",
        "сцена", "обход мкс", "из бит мкс", "выигрыш", "экономия мкс"
    );
    let mut m = Mesher::new();
    for scene in Scene::ALL {
        let c = build(scene, 42);
        let bits = meshing::bits::bits_from_chunk(&c);
        let st = m.mesh(&c, Opts::default());
        let reps = if st.quads > 100_000 { 40 } else { 400 };
        let slow = time_us(|| m.mesh(&c, Opts::default()).quads, reps);
        let fast = time_us(|| m.mesh_from_bits(&bits, &c, Opts::default()).quads, reps);
        println!(
            "{:<16} {:>12.1} {:>12.1} {:>9.2}x {:>12.1}",
            scene.name(),
            slow,
            fast,
            slow / fast,
            slow - fast
        );
    }
    println!("(экономия постоянна по сценам: обход байтов не зависит от содержимого)");
}

fn main() {
    println!("чанк {CS}³ с паддингом до 64³, квад 8 байт");
    quads_and_time();
    levers();
    phases();
    stream();
    from_bits();
    budget();
}
