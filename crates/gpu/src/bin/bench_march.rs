//! Q-18: марш луча в миллисекундах, а не в итерациях.
//!
//! Запуск: `cargo run --release -p gpu --bin bench_march`
//!
//! `21-render-bench.md` посчитал итерации — величину, переносимую
//! с процессора на видеокарту. Здесь считается то, что не переносится
//! и что единственное отвечает на вопрос «влезает ли в кадр».
//!
//! Мир 1024×256×1024 при вокселе 0.5 м — это 512×128×512 метров, четверть
//! карты 2×2 км по стороне при полной глубине. Битовый объём 32 МиБ,
//! верхние уровни 0.5 МиБ.
//!
//! Лучи загружаются **один раз** и марш меряется отдельно от загрузки:
//! настоящий шейдер считает луч из индекса пикселя, а не читает его
//! из буфера.

use std::time::Instant;

use gpu::march::{build_levels, Levels, Marcher};
use gpu::Gpu;

const NX: usize = 1024;
const NY: usize = 256;
const NZ: usize = 1024;

const PIX_W: u64 = 2560;
const PIX_H: u64 = 1440;
const REPS: usize = 21;

fn stat(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[0])
}

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Рельеф с холмами, обрывами и пещерным слоем. Не шум: марш чувствителен
/// к тому, сплошная порода или дырявая, и ровный рельеф завысил бы пропуск.
fn terrain(x: usize, y: usize, z: usize) -> bool {
    let (fx, fz) = (x as f32, z as f32);
    let h = 90.0
        + (fx * 0.011).sin() * 26.0
        + (fz * 0.013).cos() * 22.0
        + (fx * 0.047 + fz * 0.031).sin() * 7.0;
    if (y as f32) >= h {
        return false;
    }
    // Пещеры: пустоты внутри породы, чтобы верхние уровни не были
    // сплошь занятыми.
    let c = (fx * 0.06).sin() * (fz * 0.055).cos() * ((y as f32) * 0.08).sin();
    c < 0.55
}

fn surface_y(x: usize, z: usize) -> usize {
    let mut y = NY - 2;
    while y > 1 && !terrain(x, y, z) {
        y -= 1;
    }
    y
}

fn main() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        println!("адаптера нет");
        return;
    };
    println!("{} | драйвер {}", g.info.name, g.info.driver_info);

    let t0 = Instant::now();
    let lv: Levels = build_levels(NX, NY, NZ, terrain);
    println!(
        "мир {NX}×{NY}×{NZ}; L0 {} МиБ, L1+L2 {} КиБ; построен за {:.1} с",
        lv.l0.len() * 4 / 1024 / 1024,
        (lv.l1.len() + lv.l2.len()) * 4 / 1024,
        t0.elapsed().as_secs_f64()
    );
    println!("экран {PIX_W}×{PIX_H} = {:.2} млн лучей", (PIX_W * PIX_H) as f64 / 1e6);

    // Лучей меряем меньше, чем пикселей: цена линейна по числу лучей,
    // и 1 млн даёт устойчивую наносекунду без 118 МиБ буфера заданий.
    let n = 1 << 20;
    let m = Marcher::new(&g, &lv, n);
    let qs = g.device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("ts"),
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let qbuf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("q"),
        size: 16,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let period = g.ts_period.unwrap_or(1.0) as f64;

    // Возвращает (нс на луч на видеокарте, среднее итераций).
    let run = |jobs: &[f32]| -> (f64, f64) {
        let rays = jobs.len() / 8;
        m.upload(&g, jobs);
        let mut dev = Vec::new();
        for _ in 0..(REPS + 3) {
            let mut e = g.device.create_command_encoder(&Default::default());
            e.write_timestamp(&qs, 0);
            m.encode(&mut e, rays);
            e.write_timestamp(&qs, 1);
            e.resolve_query_set(&qs, 0..2, &qbuf, 0);
            g.queue.submit([e.finish()]);
            g.wait();
            let t = g.read_u32(&qbuf, 4);
            let a = t[0] as u64 | ((t[1] as u64) << 32);
            let b = t[2] as u64 | ((t[3] as u64) << 32);
            dev.push((b - a) as f64 * period);
        }
        dev.drain(0..3);
        let outs = g.read_u32(&m.outs, rays * 2);
        let iters: u64 = (0..rays).map(|i| outs[i * 2 + 1] as u64).sum();
        (stat(dev).0 / rays as f64, iters as f64 / rays as f64)
    };

    let mut r = fx::rng::Rng::new(5, 0x18);

    section("1. Первичная видимость: камера над картой");
    println!("камера RTS под 45°, лучи через сетку пикселей.\n");
    println!("{:>14} {:>14} {:>16} {:>18}", "высота камеры", "итераций/луч", "нс на луч", "мс на 3.69 млн");
    let mut primary_ns = 0.0f64;
    for cam_h in [180usize, 240] {
        let mut jobs = Vec::with_capacity(n * 8);
        let (cx, cz) = (200.0f32, 200.0f32);
        for i in 0..n {
            // Прямоугольник экрана, спроецированный на сетку.
            let (px, py) = ((i % 1024) as f32 / 1024.0, (i / 1024) as f32 / 1024.0);
            let target = [px * (NX as f32) * 0.8 + 100.0, 0.0, py * (NZ as f32) * 0.8 + 100.0];
            let o = [cx, cam_h as f32, cz];
            let d = [target[0] - o[0], target[1] - o[1], target[2] - o[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            jobs.extend_from_slice(&[o[0], o[1], o[2], 2000.0, d[0] / len, d[1] / len, d[2] / len, 0.0]);
        }
        let (ns, it) = run(&jobs);
        if cam_h == 180 {
            primary_ns = ns;
        }
        println!(
            "{cam_h:>14} {it:>14.1} {ns:>16.2} {:>18.2}",
            ns * (PIX_W * PIX_H) as f64 / 1e6
        );
    }

    section("2. Затенение: луч от поверхности, радиус в метрах");
    println!("воксель 0.5 м, поэтому 4 м это 8 вокселей.\n");
    println!("{:>10} {:>14} {:>16} {:>18}", "радиус, м", "итераций/луч", "нс на луч", "мс на 3.69 млн");
    let mut ao_ns = 0.0f64;
    for radius_m in [2.0f32, 4.0, 8.0] {
        let mut jobs = Vec::with_capacity(n * 8);
        for _ in 0..n {
            let x = r.below(NX as u32 - 2) as usize + 1;
            let z = r.below(NZ as u32 - 2) as usize + 1;
            let y = surface_y(x, z) + 1;
            let (a, b, c) = (
                r.range(-100, 100) as f32 / 100.0,
                r.range(10, 100) as f32 / 100.0,
                r.range(-100, 100) as f32 / 100.0,
            );
            let len = (a * a + b * b + c * c).sqrt().max(0.3);
            jobs.extend_from_slice(&[
                x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5, radius_m * 2.0,
                a / len, b / len, c / len, 0.0,
            ]);
        }
        let (ns, it) = run(&jobs);
        if radius_m == 4.0 {
            ao_ns = ns;
        }
        println!(
            "{radius_m:>10} {it:>14.1} {ns:>16.2} {:>18.2}",
            ns * (PIX_W * PIX_H) as f64 / 1e6
        );
    }

    section("3. Солнце: луч не ограничен ничем");
    println!("{:>14} {:>14} {:>16} {:>18}", "высота солнца", "итераций/луч", "нс на луч", "мс на 3.69 млн");
    let mut sun_ns = 0.0f64;
    for elev in [15i32, 45] {
        let e = (elev as f32).to_radians();
        let d = [
            e.cos() * std::f32::consts::FRAC_1_SQRT_2,
            e.sin(),
            e.cos() * std::f32::consts::FRAC_1_SQRT_2,
        ];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let d = [d[0] / len, d[1] / len, d[2] / len];
        let mut jobs = Vec::with_capacity(n * 8);
        for _ in 0..n {
            let x = r.below(NX as u32 - 2) as usize + 1;
            let z = r.below(NZ as u32 - 2) as usize + 1;
            let y = surface_y(x, z) + 1;
            jobs.extend_from_slice(&[
                x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5, 3000.0,
                d[0], d[1], d[2], 0.0,
            ]);
        }
        let (ns, it) = run(&jobs);
        if elev == 45 {
            sun_ns = ns;
        }
        println!(
            "{elev:>13}° {it:>14.1} {ns:>16.2} {:>18.2}",
            ns * (PIX_W * PIX_H) as f64 / 1e6
        );
    }

    section("4. Бюджет кадра: три конфигурации ADR-063 в миллисекундах");
    let px = (PIX_W * PIX_H) as f64;
    println!("{:<56} {:>14}", "конфигурация", "мс на кадр");
    for (name, ao_rays, sun_rays) in [
        ("затенение 2 луча полного разрешения плюс солнце маршем", 2.0 * px, px),
        ("затенение 1 луч половинного плюс солнце маршем", px / 4.0, px),
        ("затенение 1 луч половинного, солнце из каскадных карт", px / 4.0, 0.0),
    ] {
        println!(
            "{name:<56} {:>14.2}",
            (ao_rays * ao_ns + sun_rays * sun_ns) / 1e6
        );
    }
    println!();
    println!("плюс первичная видимость маршем: {:.2} мс", primary_ns * px / 1e6);
    println!();
    println!("бюджет кадра 16.7 мс при 60 Гц. Замер на RTX 5090;");
    println!("целевая карта RTX 4060 (ADR-093) примерно втрое слабее,");
    println!("то есть числа выше надо умножать на три.");
}
