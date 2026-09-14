//! DDA по вокселям: горизонтальные лучи против камерных, анизотропия
//! раскладки, цена детерминизма.
//!
//! Запуск: `cargo run --release -p harness --bin bench_ray`

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use fx::Fx;
use harness::ray::{dda, dda_f64, dda_skip, Dir};
use harness::World;
use meshing::CS as MCS;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn dir(x: i32, y: i32, z: i32) -> Dir {
    Dir {
        x: Fx::from_ratio(x, 1000),
        y: Fx::from_ratio(y, 1000),
        z: Fx::from_ratio(z, 1000),
    }
}

/// Пачка лучей одного класса: начало на поверхности, направление задано.
/// Луч в замере: начало, направление в `Fx` и оно же в `f64` для сравнения.
type Sample = (i32, i32, i32, Dir, (f64, f64, f64));

fn rays(w: &World, n: usize, seed: u64, kind: RayKind) -> Vec<Sample> {
    let mut r = Rng::new(seed, kind as u64);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let x = r.below(w.voxels_x() as u32) as i32;
        let z = r.below(w.voxels_z() as u32) as i32;
        // Поверхность.
        let mut y = MCS as i32 - 2;
        while y > 1 && !w.get_i(x, y, z) {
            y -= 1;
        }
        if y < 6 {
            continue;
        }
        let (ox, oy, oz) = (x, y + 2, z);
        let (dx, dy, dz) = match kind {
            // Ствол смотрит почти горизонтально: подъём в пределах пяти
            // процентов, как у настильного выстрела.
            RayKind::AlongX => (1000, r.range(-50, 50), 0),
            RayKind::AlongZ => (0, r.range(-50, 50), 1000),
            RayKind::Diagonal => (707, r.range(-50, 50), 707),
            RayKind::Random => {
                let a = r.range(-1000, 1000);
                let b = r.range(-1000, 1000);
                if a == 0 && b == 0 {
                    continue;
                }
                (a, r.range(-50, 50), b)
            }
            // Камерный: изометрия, сорок пять градусов сверху.
            RayKind::Camera => (500, -1000, 500),
        };
        out.push((
            ox,
            oy,
            oz,
            dir(dx, dy, dz),
            (dx as f64 / 1000.0, dy as f64 / 1000.0, dz as f64 / 1000.0),
        ));
    }
    out
}

#[derive(Clone, Copy)]
enum RayKind {
    AlongX = 0,
    AlongZ = 1,
    Diagonal = 2,
    Random = 3,
    Camera = 4,
}

impl RayKind {
    fn name(self) -> &'static str {
        match self {
            RayKind::AlongX => "горизонт. вдоль X",
            RayKind::AlongZ => "горизонт. вдоль Z",
            RayKind::Diagonal => "горизонт. по диагонали",
            RayKind::Random => "горизонт. случайно",
            RayKind::Camera => "камерный сверху",
        }
    }
}

fn main() {
    let w = World::new(6, 6, 21);
    println!(
        "мир {}×{}×{} вокселей ({} чанков {MCS}³)",
        w.voxels_x(),
        MCS,
        w.voxels_z(),
        w.nx * w.nz
    );

    let n = 4000usize;
    let max_steps = 600u32;

    section("1. Горизонтальные лучи против камерных");
    println!("максимум {max_steps} шагов; «попало» — доля лучей, нашедших твёрдое.\n");
    println!(
        "{:<24} {:>10} {:>12} {:>12} {:>14}",
        "класс луча", "попало", "шагов ср.", "чтений ср.", "нс на луч"
    );
    for kind in [
        RayKind::Camera,
        RayKind::AlongX,
        RayKind::AlongZ,
        RayKind::Diagonal,
        RayKind::Random,
    ] {
        let rs = rays(&w, n, 5, kind);
        let mut hits = 0usize;
        let mut steps = 0u64;
        let mut reads = 0u64;
        for &(x, y, z, d, _) in rs.iter() {
            let h = dda(&w, x, y, z, d, max_steps);
            hits += h.hit as usize;
            steps += h.steps as u64;
            reads += h.reads as u64;
        }
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &(x, y, z, d, _) in rs.iter() {
                acc += dda(&w, x, y, z, d, max_steps).steps as u64;
            }
            black_box(acc);
            best = best.min(t0.elapsed().as_secs_f64() * 1e9 / rs.len() as f64);
        }
        println!(
            "{:<24} {:>9.1}% {:>12.1} {:>12.1} {:>14.0}",
            kind.name(),
            100.0 * hits as f64 / rs.len() as f64,
            steps as f64 / rs.len() as f64,
            reads as f64 / rs.len() as f64,
            best
        );
    }
    println!("(камерный луч втыкается в рельеф за считаные шаги; выстрел скользит над землёй)");

    section("2. Анизотропия раскладки: пропуск по слову колонки");
    println!("колонка по X — одно слово; вдоль X пустой участок глотается одним тестом,");
    println!("вдоль Z такого пропуска нет вовсе.\n");
    println!(
        "{:<24} {:>14} {:>14} {:>10} {:>14} {:>12}",
        "класс луча", "шагом, нс", "с пропуском, нс", "выигрыш", "чтений шагом", "с пропуском"
    );
    for kind in [
        RayKind::AlongX,
        RayKind::AlongZ,
        RayKind::Diagonal,
        RayKind::Random,
        RayKind::Camera,
    ] {
        let rs = rays(&w, n, 5, kind);
        let mut r_plain = 0u64;
        let mut r_skip = 0u64;
        for &(x, y, z, d, _) in rs.iter() {
            r_plain += dda(&w, x, y, z, d, max_steps).reads as u64;
            r_skip += dda_skip(&w, x, y, z, d, max_steps).reads as u64;
        }
        let mut a = f64::MAX;
        let mut b = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &(x, y, z, d, _) in rs.iter() {
                acc += dda(&w, x, y, z, d, max_steps).steps as u64;
            }
            black_box(acc);
            a = a.min(t0.elapsed().as_secs_f64() * 1e9 / rs.len() as f64);
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &(x, y, z, d, _) in rs.iter() {
                acc += dda_skip(&w, x, y, z, d, max_steps).steps as u64;
            }
            black_box(acc);
            b = b.min(t0.elapsed().as_secs_f64() * 1e9 / rs.len() as f64);
        }
        println!(
            "{:<24} {a:>14.0} {b:>14.0} {:>9.2}x {:>14.1} {:>12.1}",
            kind.name(),
            a / b,
            r_plain as f64 / rs.len() as f64,
            r_skip as f64 / rs.len() as f64
        );
    }
    println!("(разница между строками X и Z — и есть цена линейной раскладки)");

    section("3. Цена детерминизма на этой нагрузке");
    println!(
        "{:<24} {:>14} {:>14} {:>10}",
        "класс луча", "Fx, нс", "f64, нс", "отношение"
    );
    for kind in [RayKind::Random, RayKind::Camera] {
        let rs = rays(&w, n, 5, kind);
        let mut a = f64::MAX;
        let mut b = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &(x, y, z, d, _) in rs.iter() {
                acc += dda(&w, x, y, z, d, max_steps).steps as u64;
            }
            black_box(acc);
            a = a.min(t0.elapsed().as_secs_f64() * 1e9 / rs.len() as f64);
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &(x, y, z, _, f) in rs.iter() {
                acc += dda_f64(&w, x, y, z, f.0, f.1, f.2, max_steps).steps as u64;
            }
            black_box(acc);
            b = b.min(t0.elapsed().as_secs_f64() * 1e9 / rs.len() as f64);
        }
        println!("{:<24} {a:>14.0} {b:>14.0} {:>9.2}x", kind.name(), a / b);
    }

    section("4. Тот же замер на мире, который не влезает в кэш");
    println!("первый мир — 9 МБ материала, он целиком в L3 этой машины (96 МБ).");
    println!("если анизотропия раскладки берётся из промахов кэша, она обязана");
    println!("проявиться на мире побольше.");
    let big = World::new(16, 16, 21);
    println!(
        "мир {}x{}x{}: {} МБ материала, {} МБ битового объёма",
        big.voxels_x(),
        MCS,
        big.voxels_z(),
        big.nx * big.nz * 64 * 64 * 64 / 1024 / 1024,
        big.nx * big.nz * 64 * 64 * 8 / 1024 / 1024
    );
    println!(
        "\n{:<24} {:>14} {:>14} {:>12}",
        "класс луча", "малый мир, нс", "большой, нс", "отношение"
    );
    for kind in [RayKind::AlongX, RayKind::AlongZ, RayKind::Diagonal] {
        let rs_small = rays(&w, n, 5, kind);
        let rs_big = rays(&big, n, 5, kind);
        let mut a = f64::MAX;
        let mut b = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &(x, y, z, d, _) in rs_small.iter() {
                acc += dda(&w, x, y, z, d, max_steps).steps as u64;
            }
            black_box(acc);
            a = a.min(t0.elapsed().as_secs_f64() * 1e9 / rs_small.len() as f64);
            let t0 = Instant::now();
            let mut acc = 0u64;
            for &(x, y, z, d, _) in rs_big.iter() {
                acc += dda(&big, x, y, z, d, max_steps).steps as u64;
            }
            black_box(acc);
            b = b.min(t0.elapsed().as_secs_f64() * 1e9 / rs_big.len() as f64);
        }
        println!("{:<24} {a:>14.0} {b:>14.0} {:>11.2}x", kind.name(), b / a);
    }

    section("5. Что это значит для боя");
    let rs = rays(&w, n, 5, RayKind::Random);
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut acc = 0u64;
        for &(x, y, z, d, _) in rs.iter() {
            acc += dda(&w, x, y, z, d, max_steps).steps as u64;
        }
        black_box(acc);
        best = best.min(t0.elapsed().as_secs_f64() * 1e9 / rs.len() as f64);
    }
    println!("луч разрешения попадания, шагом: {best:.0} нс");
    println!("(пропуск на случайных направлениях не помогает, а вредит)\n");
    println!(
        "{:>14} {:>14} {:>16}",
        "выстрелов/тик", "мс на тик", "доля бюджета"
    );
    for shots in [100usize, 500, 2000, 5000] {
        let ms = best * shots as f64 / 1e6;
        println!("{shots:>14} {ms:>14.3} {:>15.1}%", 100.0 * ms / 10.0);
    }
    println!("\n(дальность ограничена {max_steps} шагами — это 600 м при вокселе в метр)");
}
