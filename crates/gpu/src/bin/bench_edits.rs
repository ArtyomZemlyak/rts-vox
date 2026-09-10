//! Q-42: поток правок против выемки чанка.
//!
//! Запуск: `cargo run --release -p gpu --bin bench_edits`
//!
//! Нагрузка взята из замера устойчивой копки (ADR-090): триста копателей
//! дают **1 315 правок и 16 грязных чанков мешинга за тик**. Шестьсот —
//! 2 693 правки и 35.9 чанка.
//!
//! Чанк мешинга 62³. Рендеру из него нужен материал (u16 на воксель)
//! и биты занятости. «Выемка чанка» — это перенос всего чанка целиком;
//! «поток правок» — четыре байта на правку.

use std::time::Instant;

use gpu::edits::EditApply;
use gpu::Gpu;

const REPS: usize = 51;
/// Воксель 0.5 м, карта 2×2 км на 128 м (ADR-094): 4000×256×4000.
const MAP_VOXELS: u64 = 4000 * 256 * 4000;

fn stat(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[0])
}

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn main() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        println!("адаптера нет");
        return;
    };
    println!("{} | драйвер {}", g.info.name, g.info.driver_info);
    println!("медиана из {REPS} прогонов\n");

    // Битовый объём всей карты держать не нужно для замера транспорта:
    // берём 64 МиБ, этого хватает под любые индексы теста.
    let vox: u64 = 512 << 20;
    let apply = EditApply::new(&g, vox, 1 << 18);
    // Буфер под выемку чанков — тот же транспорт, только крупнее.
    let chunk_dst = g.storage(64 << 20, "chunks");

    // Правки: индексы вразнобой по всему объёму, как настоящая копка
    // по восьми забоям.
    let mk = |n: usize| -> Vec<u32> {
        let mut r = fx::rng::Rng::new(7, 0x42);
        (0..n).map(|_| (r.below((vox / 4) as u32) << 1) | 1).collect()
    };

    section("1. Поток правок: загрузка плюс применение");
    println!("одна отправка: write_buffer списка и диспатч в одном буфере.\n");
    println!(
        "{:>10} {:>10} {:>14} {:>14} {:>12}",
        "правок", "КиБ", "стенка, нс", "нс на правку", "тиков в мс"
    );
    let mut edit_1315 = 0.0f64;
    for n in [329usize, 1315, 2693, 16384, 65536] {
        let list = mk(n);
        let mut v = Vec::new();
        for _ in 0..(REPS + 5) {
            let t0 = Instant::now();
            let mut e = g.device.create_command_encoder(&Default::default());
            apply.encode(&g, &mut e, &list);
            g.queue.submit([e.finish()]);
            g.wait();
            v.push(t0.elapsed().as_nanos() as f64);
        }
        v.drain(0..5);
        let ns = stat(v).0;
        if n == 1315 {
            edit_1315 = ns;
        }
        println!(
            "{n:>10} {:>10.1} {ns:>14.0} {:>14.1} {:>12.0}",
            n as f64 * 4.0 / 1024.0,
            ns / n as f64,
            1e6 / ns
        );
    }

    section("2. Выемка чанков: тот же тик, но переносом целиком");
    println!("чанк мешинга 62³ = 238 328 вокселей. Материал u16 — 465 КиБ,");
    println!("одни биты занятости — 29 КиБ.\n");
    println!(
        "{:>8} {:>12} {:>10} {:>14} {:>16}",
        "чанков", "что", "МиБ", "стенка, нс", "к потоку правок"
    );
    let mut pull_16 = 0.0f64;
    for (chunks, what, bytes_per) in [
        (16usize, "материал u16", 238328usize * 2),
        (16, "только биты", 238328 / 8),
        (36, "материал u16", 238328 * 2),
        (36, "только биты", 238328 / 8),
    ] {
        let bytes = chunks * bytes_per;
        let data: Vec<u32> = vec![0x5A5A_5A5A; bytes / 4];
        let src = gpu::as_bytes(&data);
        let mut v = Vec::new();
        for _ in 0..(REPS + 5) {
            let t0 = Instant::now();
            g.queue.write_buffer(&chunk_dst, 0, src);
            g.queue.submit([]);
            g.wait();
            v.push(t0.elapsed().as_nanos() as f64);
        }
        v.drain(0..5);
        let ns = stat(v).0;
        if chunks == 16 && bytes_per == 238328 * 2 {
            pull_16 = ns;
        }
        println!(
            "{chunks:>8} {what:>12} {:>10.2} {ns:>14.0} {:>15.1}x",
            bytes as f64 / 1024.0 / 1024.0,
            ns / edit_1315
        );
    }

    section("3. Ответ");
    println!(
        "устойчивая копка, 300 копателей: 1 315 правок против 16 чанков"
    );
    println!(
        "поток правок  {:>9.0} нс   ({:.1} КиБ)",
        edit_1315,
        1315.0 * 4.0 / 1024.0
    );
    println!(
        "выемка чанков {:>9.0} нс   ({:.1} МиБ)",
        pull_16,
        16.0 * 238328.0 * 2.0 / 1024.0 / 1024.0
    );
    println!("разница {:.0}x", pull_16 / edit_1315);
    println!();
    println!("и то и другое влезает в кадр 16.7 мс; вопрос в том, сколько");
    println!("от него остаётся рендеру. Выемка съедает {:.1}% кадра,", pull_16 / 16.7e6 * 100.0);
    println!("поток правок — {:.2}%.", edit_1315 / 16.7e6 * 100.0);
    println!();
    println!("на карту {MAP_VOXELS} вокселей битовый объём весит {} МиБ;", MAP_VOXELS / 8 / 1024 / 1024);
    println!("пересобирать его выемкой нельзя ни при каком раскладе.");
}
