//! Доля горячих чанков при копке плюс бое (ADR-024) и связность пустот
//! (ADR-057).
//!
//! Запуск: `cargo run --release -p harness --bin bench_hot`
//!
//! ADR-024 предполагает **компактное горячее множество**: чанки, которых
//! касаются, помещаются в распакованный слой, остальные лежат сжатыми.
//! Копка кучкуется вокруг забоев, а бой мобилен и разбросан — их сумма
//! и есть настоящее горячее множество.

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use harness::{Dig, World};
use mesh::CS as MCS;
use topo::scene::{build as build_topo, Scene as TopoScene};
use topo::Components;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Обстрел: попадания разбросаны по карте, а не сидят у забоя.
fn shell(w: &mut World, r: &mut Rng, shots: usize, out: &mut Vec<(usize, usize, usize)>) {
    for _ in 0..shots {
        let x = r.below(w.voxels_x() as u32) as usize;
        let z = r.below(w.voxels_z() as u32) as usize;
        let mut y = MCS - 2;
        while y > 1 && !w.get(x, y, z) {
            y -= 1;
        }
        for dz in 0..2usize {
            for dy in 0..2usize {
                for dx in 0..2usize {
                    let (px, py, pz) = (x + dx, y.saturating_sub(dy), z + dz);
                    if w.dig(px, py, pz) {
                        out.push((px, py, pz));
                    }
                }
            }
        }
    }
}

fn main() {
    section("1. Горячее множество: копка, бой и их сумма");
    println!("мир 8×8 чанков мешинга — 496×62×496 вокселей, 64 чанка.\n");
    println!(
        "{:<28} {:>12} {:>14} {:>16} {:>14}",
        "нагрузка", "правок/тик", "чанков/тик", "горячих за 60 тиков", "доля мира"
    );
    for (name, diggers, shots) in [
        ("только копка, 150", 150usize, 0usize),
        ("только копка, 300", 300, 0),
        ("только бой, 50 залпов", 0, 50),
        ("только бой, 200 залпов", 0, 200),
        ("копка 300 плюс бой 50", 300, 50),
        ("копка 300 плюс бой 200", 300, 200),
    ] {
        let mut w = World::new(8, 8, 5);
        let total = w.chunk_count();
        let mut dig = if diggers > 0 {
            Some(Dig::new(&w, diggers, 2, 3))
        } else {
            None
        };
        let mut r = Rng::new(9, shots as u64);
        let mut out = Vec::new();
        let mut edits = 0u64;
        let mut per_tick = 0u64;
        let mut ever = vec![false; total];
        let ticks = 60usize;
        for _ in 0..10 {
            if let Some(d) = dig.as_mut() {
                d.step(&mut w, &mut out);
            } else {
                out.clear();
            }
            shell(&mut w, &mut r, shots, &mut out);
            w.flush_mesh();
        }
        for _ in 0..ticks {
            if let Some(d) = dig.as_mut() {
                d.step(&mut w, &mut out);
            } else {
                out.clear();
            }
            shell(&mut w, &mut r, shots, &mut out);
            edits += out.len() as u64;
            for &(x, _, z) in out.iter() {
                let ci = (z / MCS) * w.nx + (x / MCS);
                ever[ci] = true;
            }
            let (dirty, _) = w.flush_mesh();
            per_tick += dirty as u64;
        }
        let hot = ever.iter().filter(|&&v| v).count();
        println!(
            "{name:<28} {:>12.1} {:>14.1} {:>16} {:>13.0}%",
            edits as f64 / ticks as f64,
            per_tick as f64 / ticks as f64,
            hot,
            100.0 * hot as f64 / total as f64
        );
    }
    println!("(«горячих за 60 тиков» — сколько разных чанков хоть раз тронули за две секунды)");

    section("2. Кучность решает всё");
    println!("300 копателей и 50 залпов на карте 16×16 чанков (992 м).");
    println!("Меняется только число забоев, в которые копатели сбиты.\n");
    println!(
        "{:>6} {:>8} {:>14} {:>16} {:>14} {:>16}",
        "забоев", "залпов", "чанков/тик", "горячих за 60", "доля мира", "горячих, МиБ"
    );
    for (clusters, shots) in [
        (1usize, 0usize), (4, 0), (16, 0), (64, 0), (300, 0),
        (4, 10), (4, 50),
    ] {
        let mut w = World::new(16, 16, 5);
        let total = w.chunk_count();
        let mut dig = Dig::clustered(&w, 300, 2, 3, clusters);
        let mut r = Rng::new(9, 50);
        let mut out = Vec::new();
        let mut ever = vec![false; total];
        let mut per_tick = 0u64;
        for _ in 0..10 {
            dig.step(&mut w, &mut out);
            shell(&mut w, &mut r, shots, &mut out);
            w.flush_mesh();
        }
        let ticks = 60usize;
        for _ in 0..ticks {
            dig.step(&mut w, &mut out);
            shell(&mut w, &mut r, shots, &mut out);
            for &(x, _, z) in out.iter() {
                let ci = (z / MCS) * w.nx + (x / MCS);
                ever[ci] = true;
            }
            let (dirty, _) = w.flush_mesh();
            per_tick += dirty as u64;
        }
        let hot = ever.iter().filter(|&&v| v).count();
        println!(
            "{clusters:>6} {shots:>8} {:>14.1} {hot:>16} {:>13.0}% {:>16.1}",
            per_tick as f64 / ticks as f64,
            100.0 * hot as f64 / total as f64,
            hot as f64 * 8.0 * 16.0 / 1024.0
        );
    }
    println!("(«горячих, МиБ» — при палитре на чанк: чанк мешинга 62³ это восемь чанков 32³ по 16 КиБ)");
    println!("(пятьдесят залпов боя рассыпаны по всей карте всегда — это нижняя граница)");

    section("3. Связность пустот: второй граф (ADR-057)");
    println!("та же конструкция, что для твёрдого, но на инвертированных колонках.\n");
    println!(
        "{:<18} {:>14} {:>14} {:>14} {:>14}",
        "сцена", "компонент тв.", "компонент пуст.", "твёрдое, мкс", "пустота, мкс"
    );
    let mut comp = Components::new();
    for scene in TopoScene::ALL {
        let c = build_topo(scene, 42);
        let solid = comp.build(&c);
        let void = comp.build_void(&c);
        let mut a = f64::MAX;
        let mut b = f64::MAX;
        for _ in 0..5 {
            let reps = 2000;
            let t0 = Instant::now();
            for _ in 0..reps {
                black_box(comp.build(&c));
            }
            a = a.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
            let t0 = Instant::now();
            for _ in 0..reps {
                black_box(comp.build_void(&c));
            }
            b = b.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
        }
        println!(
            "{:<18} {solid:>14} {void:>14} {a:>14.2} {b:>14.2}",
            scene.name()
        );
    }
    println!("(в цену пустоты входит инверсия колонок: 1024 слова)");
}
