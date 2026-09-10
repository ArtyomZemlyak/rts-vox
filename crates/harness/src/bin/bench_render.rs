//! Бюджет кадра: марш затенения против марша солнца (ADR-063).
//!
//! Запуск: `cargo run --release -p harness --bin bench_render`
//!
//! ADR-063 держится на двух числах: 221 миллион итераций марша на кадр
//! при полном затенении и солнце маршем, против 7.4 миллиона при солнце
//! из каскадных карт. Оба получены расчётом. Здесь считаются настоящие
//! итерации на настоящей геометрии.
//!
//! Мерится **число итераций**, а не наносекунды: наносекунда на процессоре
//! к видеокарте не переносится, а итерация переносится — это то, чем
//! измеряется работа марша в самом ADR-063.

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use harness::pyramid::{march_flat, march_pyramid, Pyramid};
use harness::{Dig, World};
use mesh::CS as MCS;

/// Разрешение, на котором считает ADR-063.
const PIX_W: u64 = 2560;
const PIX_H: u64 = 1440;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Точка на поверхности плюс нормаль вверх — то, что даёт греди-меш
/// после растеризации. Марш идёт **от поверхности**, а не от камеры.
fn surface_points(w: &World, n: usize, seed: u64) -> Vec<[f32; 3]> {
    let mut r = Rng::new(seed, 0x51);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let x = r.below(w.voxels_x() as u32) as usize;
        let z = r.below(w.voxels_z() as u32) as usize;
        let mut y = MCS - 2;
        while y > 1 && !w.get(x, y, z) {
            y -= 1;
        }
        if y < 4 {
            continue;
        }
        out.push([x as f32 + 0.5, y as f32 + 1.5, z as f32 + 0.5]);
    }
    out
}

/// Полушарие вверх: направления затенения.
fn hemi(r: &mut Rng) -> [f32; 3] {
    loop {
        let (a, b, c) = (
            r.range(-100, 100) as f32 / 100.0,
            r.range(0, 100) as f32 / 100.0,
            r.range(-100, 100) as f32 / 100.0,
        );
        let len = (a * a + b * b + c * c).sqrt();
        if len > 0.3 {
            return [a / len, b / len, c / len];
        }
    }
}

fn main() {
    let w = World::new(10, 10, 21);
    let p = Pyramid::build(&w);
    println!(
        "мир {}×{}×{} вокселей; пирамида: L0 {} МиБ, верхние уровни {} КиБ",
        w.voxels_x(),
        MCS,
        w.voxels_z(),
        p.bytes_l0() / 1024 / 1024,
        p.bytes_upper() / 1024
    );
    println!("экран {PIX_W}×{PIX_H} = {:.2} млн пикселей\n", (PIX_W * PIX_H) as f64 / 1e6);

    let pts = surface_points(&w, 20000, 5);
    let mut r = Rng::new(7, 7);

    section("1. Затенение: луч ограничен радиусом");
    println!("радиус задан в метрах и ограничивает худший случай по построению.\n");
    println!(
        "{:>10} {:>16} {:>16} {:>12} {:>14}",
        "радиус, м", "итераций плоско", "итераций пирам.", "выигрыш", "попало"
    );
    let mut ao_iters_4 = 0.0f64;
    for radius in [2.0f32, 4.0, 8.0, 16.0] {
        let (mut flat, mut pyr, mut hit) = (0u64, 0u64, 0usize);
        for &o in pts.iter() {
            let d = hemi(&mut r);
            let a = march_flat(&w, o, d, radius);
            let b = march_pyramid(&p, &w, o, d, radius);
            flat += a.iters as u64;
            pyr += b.iters as u64;
            hit += a.hit as usize;
        }
        let (f, q) = (flat as f64 / pts.len() as f64, pyr as f64 / pts.len() as f64);
        if radius == 4.0 {
            ao_iters_4 = q;
        }
        println!(
            "{radius:>10} {f:>16.1} {q:>16.1} {:>11.2}x {:>13.1}%",
            f / q,
            100.0 * hit as f64 / pts.len() as f64
        );
    }

    section("2. Солнце: луч не ограничен ничем");
    println!("солнце обязано выйти из ямы и дальше скользит над рельефом.\n");
    println!(
        "{:>14} {:>16} {:>16} {:>12} {:>14}",
        "высота солнца", "итераций плоско", "итераций пирам.", "выигрыш", "в тени"
    );
    let mut sun_iters = 0.0f64;
    for elev in [15i32, 30, 45, 60] {
        let e = (elev as f32).to_radians();
        let d = [e.cos() * std::f32::consts::FRAC_1_SQRT_2, e.sin(), e.cos() * std::f32::consts::FRAC_1_SQRT_2];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let d = [d[0] / len, d[1] / len, d[2] / len];
        let (mut flat, mut pyr, mut hit) = (0u64, 0u64, 0usize);
        for &o in pts.iter() {
            let a = march_flat(&w, o, d, 600.0);
            let b = march_pyramid(&p, &w, o, d, 600.0);
            flat += a.iters as u64;
            pyr += b.iters as u64;
            hit += a.hit as usize;
        }
        let (f, q) = (flat as f64 / pts.len() as f64, pyr as f64 / pts.len() as f64);
        if elev == 45 {
            sun_iters = q;
        }
        println!(
            "{elev:>13}° {f:>16.1} {q:>16.1} {:>11.2}x {:>13.1}%",
            f / q,
            100.0 * hit as f64 / pts.len() as f64
        );
    }

    section("3. То же на изрытом мире");
    println!("копка меняет геометрию, и пирамида на ней работает иначе.\n");
    let mut dug = World::new(10, 10, 21);
    let mut dig = Dig::clustered(&dug, 400, 2, 3, 8);
    let mut out = Vec::new();
    for _ in 0..600 {
        dig.step(&mut dug, &mut out);
        dug.flush_mesh();
    }
    let pd = Pyramid::build(&dug);
    let pts_d = surface_points(&dug, 20000, 5);
    println!(
        "{:<18} {:>16} {:>16} {:>12}",
        "нагрузка", "итераций плоско", "итераций пирам.", "выигрыш"
    );
    for (name, radius, dir_sun) in [("затенение 4 м", 4.0f32, false), ("солнце 45°", 600.0, true)] {
        let e = 45f32.to_radians();
        let sd = {
            let d = [e.cos() * std::f32::consts::FRAC_1_SQRT_2, e.sin(), e.cos() * std::f32::consts::FRAC_1_SQRT_2];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            [d[0] / l, d[1] / l, d[2] / l]
        };
        let (mut flat, mut pyr) = (0u64, 0u64);
        for &o in pts_d.iter() {
            let d = if dir_sun { sd } else { hemi(&mut r) };
            flat += march_flat(&dug, o, d, radius).iters as u64;
            pyr += march_pyramid(&pd, &dug, o, d, radius).iters as u64;
        }
        let (f, q) = (flat as f64 / pts_d.len() as f64, pyr as f64 / pts_d.len() as f64);
        println!("{name:<18} {f:>16.1} {q:>16.1} {:>11.2}x", f / q);
    }

    section("4. Сколько это итераций на кадр");
    println!("три конфигурации из ADR-063, теперь с измеренными итерациями на луч.\n");
    let px = (PIX_W * PIX_H) as f64;
    let rows: [(&str, f64, f64); 3] = [
        ("затенение 2 луча полного разрешения плюс солнце маршем", 2.0 * px, px),
        ("затенение 1 луч половинного плюс солнце маршем", px / 4.0, px),
        ("затенение 1 луч половинного, солнце из каскадных карт", px / 4.0, 0.0),
    ];
    println!(
        "{:<56} {:>16} {:>14}",
        "конфигурация", "итераций/кадр", "записано"
    );
    let recorded = ["221 млн", "173 млн", "7.4 млн"];
    for (i, (name, ao_rays, sun_rays)) in rows.iter().enumerate() {
        let total = ao_rays * ao_iters_4 + sun_rays * sun_iters;
        println!("{name:<56} {:>13.1} млн {:>14}", total / 1e6, recorded[i]);
    }
    println!();
    println!(
        "измерено: затенение {ao_iters_4:.1} итераций на луч, солнце {sun_iters:.1}"
    );
    println!(
        "доля солнца в первой конфигурации: {:.0}%",
        100.0 * px * sun_iters / (2.0 * px * ao_iters_4 + px * sun_iters)
    );

    section("5. Память пирамиды");
    println!(
        "верхние уровни на этот мир: {} КиБ на {} блоков 64³",
        p.bytes_upper() / 1024,
        p.bytes_upper() / (16 * 16 * 2 + 8)
    );
    let blocks_2km = (2000usize / 64 + 1) * (256 / 64) * (2000 / 64 + 1);
    println!(
        "экстраполяция на карту 2×2 км высотой 256: L0 {} МиБ, верхние уровни {:.2} МиБ",
        blocks_2km * 64 * 64 * 8 / 1024 / 1024,
        blocks_2km as f64 * (16.0 * 16.0 * 2.0 + 8.0) / 1024.0 / 1024.0
    );
    println!("(ADR-061: «при ветвлении 4³ она весит 2 МиБ и влезает в L2 с запасом»)");

    section("6. Время на процессоре — только как порядок");
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t0 = Instant::now();
        let mut acc = 0u64;
        for &o in pts.iter() {
            acc += march_pyramid(&p, &w, o, hemi(&mut r), 4.0).iters as u64;
        }
        black_box(acc);
        best = best.min(t0.elapsed().as_secs_f64() * 1e9 / pts.len() as f64);
    }
    println!("луч затенения на процессоре: {best:.0} нс");
    println!("(на видеокарту не переносится — там тысячи лучей параллельно;");
    println!(" переносится число итераций, а не наносекунда)");
}
