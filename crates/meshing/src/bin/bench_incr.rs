//! Инкрементальный ремеш против полной пересборки, плюс масштабирование
//! мешинга по потокам.
//!
//! Запуск: `cargo run --release -p mesh --bin bench_incr`
//!
//! Приём замера: `apply_edits` читает новое состояние из переданного объёма,
//! поэтому применение того же списка правок с **прежним** объёмом их
//! откатывает. Значит инкремент можно мерить прямым циклом «туда-обратно»,
//! без вычитания полной пересборки — а вычитание здесь и не работало:
//! сигнал в шестьдесят раз меньше вычитаемого.

use std::hint::black_box;
use std::sync::Barrier;
use std::time::Instant;

use fx::rng::Rng;
use meshing::bits::{bits_from_chunk, BitVolume};
use meshing::incr::IncMesher;
use meshing::scene::{build, Scene};
use meshing::{Chunk, Mesher, Opts, AIR, CS};

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Забой: сфера, выкопанная там, где есть что копать.
fn dig_sphere(c: &mut Chunk, r: &mut Rng, radius: i32) -> Vec<(u8, u8, u8)> {
    // Центр выбирается из твёрдых клеток, иначе «правок» выходит ноль.
    let mut centre = None;
    for _ in 0..200 {
        let (x, y, z) = (
            r.below(CS as u32) as usize + 1,
            r.below(CS as u32) as usize + 1,
            r.below(CS as u32) as usize + 1,
        );
        if c.get(x, y, z) != AIR {
            centre = Some((x as i32, y as i32, z as i32));
            break;
        }
    }
    let Some((cx, cy, cz)) = centre else {
        return Vec::new();
    };
    let mut edits = Vec::new();
    for dz in -radius..=radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy + dz * dz > radius * radius {
                    continue;
                }
                let (x, y, z) = (cx + dx, cy + dy, cz + dz);
                if x < 1 || y < 1 || z < 1 || x > CS as i32 || y > CS as i32 || z > CS as i32 {
                    continue;
                }
                let (x, y, z) = (x as usize, y as usize, z as usize);
                if c.get(x, y, z) == AIR {
                    continue;
                }
                c.set(x, y, z, AIR);
                edits.push((x as u8, y as u8, z as u8));
            }
        }
    }
    edits
}

/// Рассеянные правки по всему чанку — худший случай для инкремента.
fn scatter_edits(c: &mut Chunk, r: &mut Rng, n: usize) -> Vec<(u8, u8, u8)> {
    let mut edits = Vec::new();
    for _ in 0..n {
        let (x, y, z) = (
            r.below(CS as u32) as usize + 1,
            r.below(CS as u32) as usize + 1,
            r.below(CS as u32) as usize + 1,
        );
        if c.get(x, y, z) == AIR {
            continue;
        }
        c.set(x, y, z, AIR);
        edits.push((x as u8, y as u8, z as u8));
    }
    edits
}

/// Прямой замер: применение и откат одним и тем же списком.
fn time_apply(
    inc: &mut IncMesher,
    before: &BitVolume,
    after: &BitVolume,
    edits: &[(u8, u8, u8)],
) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let reps = 400usize;
        let t0 = Instant::now();
        for _ in 0..reps {
            inc.apply_edits(after, edits, Opts::default());
            inc.apply_edits(before, edits, Opts::default());
        }
        let el = t0.elapsed().as_secs_f64() * 1e6 / (2 * reps) as f64;
        black_box(inc.last_dirty_slices);
        best = best.min(el);
    }
    best
}

fn time_full(inc: &mut IncMesher, bits: &BitVolume) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let reps = 300usize;
        let t0 = Instant::now();
        for _ in 0..reps {
            inc.rebuild_all(bits, Opts::default());
            black_box(inc.last_dirty_slices);
        }
        best = best.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
    }
    best
}

fn main() {
    println!("чанк {CS}³, инкремент против полной пересборки");

    section("1. Забой: правки кучкой");
    println!("всего пар «грань, слой» — 372.\n");
    println!(
        "{:<18} {:>7} {:>8} {:>8} {:>10} {:>14} {:>13} {:>10}",
        "сцена",
        "радиус",
        "правок",
        "слоёв",
        "доля слоёв",
        "инкремент, мкс",
        "полная, мкс",
        "выигрыш"
    );
    let mut inc = IncMesher::new();
    for scene in [Scene::Smooth, Scene::Cavern, Scene::CavernAndScatter] {
        for radius in [1i32, 2, 4, 8] {
            let mut c = build(scene, 11);
            let before = bits_from_chunk(&c);
            let mut r = Rng::new(radius as u64, 3);
            let edits = dig_sphere(&mut c, &mut r, radius);
            let after = bits_from_chunk(&c);
            inc.rebuild_all(&before, Opts::default());
            inc.apply_edits(&after, &edits, Opts::default());
            let slices = inc.last_dirty_slices;
            inc.apply_edits(&before, &edits, Opts::default());

            let fast = time_apply(&mut inc, &before, &after, &edits);
            let fullm = time_full(&mut inc, &after);
            println!(
                "{:<18} {radius:>7} {:>8} {slices:>8} {:>9.1}% {:>14.2} {:>13.2} {:>9.1}x",
                scene.name(),
                edits.len(),
                100.0 * slices as f64 / 372.0,
                fast,
                fullm,
                fullm / fast
            );
        }
    }

    println!(
        "
состояние мешера на чанк: {} КиБ — это и есть цена инкрементальности",
        inc.state_bytes() / 1024
    );
    println!(
        "при 115 горячих чанках — {} МиБ",
        inc.state_bytes() * 115 / 1024 / 1024
    );

    section("2. Порог: когда полная пересборка дешевле");
    println!("правки рассеяны по всему чанку — худший случай для инкремента.\n");
    println!(
        "{:>10} {:>10} {:>14} {:>14} {:>14}",
        "правок", "слоёв", "инкремент, мкс", "полная, мкс", "что дешевле"
    );
    for n in [50usize, 200, 800, 3000, 8000, 20000] {
        let mut c = build(Scene::Cavern, 11);
        let before = bits_from_chunk(&c);
        let mut r = Rng::new(n as u64, 9);
        let edits = scatter_edits(&mut c, &mut r, n);
        let after = bits_from_chunk(&c);
        inc.rebuild_all(&before, Opts::default());
        inc.apply_edits(&after, &edits, Opts::default());
        let slices = inc.last_dirty_slices;
        inc.apply_edits(&before, &edits, Opts::default());

        let fast = time_apply(&mut inc, &before, &after, &edits);
        let fullm = time_full(&mut inc, &after);
        println!(
            "{:>10} {slices:>10} {fast:>14.2} {fullm:>14.2} {:>14}",
            edits.len(),
            if fast < fullm {
                "инкремент"
            } else {
                "полная"
            }
        );
    }

    section("3. Масштабирование мешинга по потокам");
    println!("пул постоянный, барьер на тик — как в замере эйконала.\n");
    let chunks: Vec<Chunk> = (0..128u64)
        .map(|s| build(Scene::CavernAndScatter, 1000 + s))
        .collect();
    let volumes: Vec<BitVolume> = chunks.iter().map(bits_from_chunk).collect();
    let cores = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(1);

    let mut serial = f64::MAX;
    {
        let mut m = Mesher::new();
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0usize;
            for (c, b) in chunks.iter().zip(volumes.iter()) {
                acc += m.mesh_from_bits(b, c, Opts::default()).quads;
            }
            black_box(acc);
            serial = serial.min(t0.elapsed().as_secs_f64() * 1e3);
        }
    }
    println!(
        "однопоточно, 128 разных чанков: {serial:.2} мс ({:.1} мкс на чанк)",
        serial * 1000.0 / chunks.len() as f64
    );
    println!(
        "{:>8} {:>12} {:>12} {:>12}",
        "потоков", "мс", "ускорение", "эффективн."
    );
    for threads in [1usize, 2, 4, 6, 7, 8] {
        if threads > cores {
            continue;
        }
        let barrier = Barrier::new(threads + 1);
        let chunk_sz = chunks.len().div_ceil(threads);
        let reps = 40usize;
        let trials = 5usize;
        let mut best = f64::MAX;
        std::thread::scope(|sc| {
            for w in 0..threads {
                let barrier = &barrier;
                let lo = (w * chunk_sz).min(chunks.len());
                let hi = ((w + 1) * chunk_sz).min(chunks.len());
                let part_c = &chunks[lo..hi];
                let part_b = &volumes[lo..hi];
                sc.spawn(move || {
                    let mut m = Mesher::new();
                    for _ in 0..(reps * trials) {
                        barrier.wait();
                        let mut acc = 0usize;
                        for (c, b) in part_c.iter().zip(part_b.iter()) {
                            acc += m.mesh_from_bits(b, c, Opts::default()).quads;
                        }
                        black_box(acc);
                        barrier.wait();
                    }
                });
            }
            for _ in 0..trials {
                let t0 = Instant::now();
                for _ in 0..reps {
                    barrier.wait();
                    barrier.wait();
                }
                best = best.min(t0.elapsed().as_secs_f64() * 1e3 / reps as f64);
            }
        });
        println!(
            "{threads:>8} {best:>12.2} {:>11.2}x {:>11.0}%",
            serial / best,
            100.0 * serial / best / threads as f64
        );
    }
    println!("(эйконал давал 4.9–5.7x на семи; мешинг читает память иначе)");
}
