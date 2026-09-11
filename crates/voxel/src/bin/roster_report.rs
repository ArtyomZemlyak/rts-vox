//! Отчёт по набору тел: размер, объём, память, цена проверки связности
//! и **силуэт сверху**.
//!
//! Headless и из терминала — числа в ADR берутся отсюда, а не с экрана.
//!
//! Вид сверху печатается здесь по той же причине, по которой здесь
//! печатаются байты: утверждение «силуэт читается с высоты боевой
//! камеры» иначе проверяется только глазами и только после того, как
//! соберётся рендер. Карта показывает верхний воксель каждой клетки —
//! ровно то, что видно сверху, вместе с его ролью.

use std::time::Instant;
use voxel::topo::unit::CoreBufs;
use voxel::unit::Part;
use voxel::roster::{courtyards, footprint_permille, silhouette, silhouette_distance, top_view};
use voxel::UnitBody;

fn glyph(p: Option<Part>) -> char {
    match p {
        None => '.',
        Some(Part::Armour) => '#',
        Some(Part::Core) => 'C',
        Some(Part::Engine) => 'E',
        Some(Part::Weapon) => 'W',
        Some(Part::Sensor) => 'S',
    }
}

/// Карта вида сверху, ужатая до `w` столбцов. Клетка блока берётся
/// та, что выше: сверху видно именно её.
fn print_top(body: &UnitBody, view: &[Option<Part>], w: usize) {
    let (nx, ny, nz) = (
        body.vol.nx as usize,
        body.vol.ny as usize,
        body.vol.nz as usize,
    );
    let step = nx.div_ceil(w).max(nz.div_ceil(w)).max(1);
    let mut z = 0;
    while z < nz {
        let mut line = String::new();
        let mut x = 0;
        while x < nx {
            let (mut best, mut top) = (None, 0usize);
            for dz in z..(z + step).min(nz) {
                for dx in x..(x + step).min(nx) {
                    if view[dz * nx + dx].is_none() {
                        continue;
                    }
                    let h = (0..ny).rev().find(|&y| body.vol.get(dx, y, dz)).unwrap_or(0);
                    if best.is_none() || h >= top {
                        best = view[dz * nx + dx];
                        top = h;
                    }
                }
            }
            line.push(glyph(best));
            x += step;
        }
        println!("    {line}");
        z += step;
    }
}

fn main() {
    let views = std::env::args().any(|a| a == "--views");
    // `--mm N` пересобирает весь набор в другой воксельной сетке
    // (ADR-208). Габарит в метрах не меняется — меняется, через
    // сколько вокселей он выражен.
    let mut mm: Option<u16> = None;
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        if k == "--mm" {
            mm = it.next().and_then(|v| v.parse().ok());
        }
    }
    let mut bodies = voxel::roster::all();
    if let Some(mm) = mm {
        let mut kept = Vec::with_capacity(bodies.len());
        for (name, b) in bodies {
            match b.resampled(mm) {
                Ok(r) => kept.push((name, r)),
                Err(e) => println!("{name}: в сетке {mm} мм — {}", e.why()),
            }
        }
        bodies = kept;
        println!();
    }
    println!("НАБОР ТЕЛ\n");
    println!(
        "{:<28} {:>12} {:>10} {:>9} {:>9} {:>7} {:>6} {:>9}",
        "машина", "том", "габарит, м", "твёрдых", "объём м3", "байт", "след", "связность"
    );

    let mut b = CoreBufs::default();
    let mut rows: Vec<(String, u32, u32)> = Vec::new();
    for (name, body) in &bodies {
        let (lx, ly, lz) = body.extent_mm();
        let m = |v: u32| v as f64 / 1000.0;
        // Объём в кубических миллиметрах — в кубометры.
        let vol = body.volume_mm3() as f64 / 1e9;

        // Цена одной проверки связности с ядром. Прогрев, потом замер:
        // первый проход греет кэш, и без прогрева мелкое тело выглядит
        // дороже крупного.
        for _ in 0..64 {
            std::hint::black_box(body.status(&mut b));
        }
        let n = 2000;
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(body.status(&mut b));
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;

        let fp = footprint_permille(&body.vol);
        let yards = courtyards(&body.vol);
        rows.push((name.to_string(), fp, yards));

        println!(
            "{:<28} {:>3}x{:<3}x{:<4} {:>4.1}x{:<4.1}x{:<5.1} {:>9} {:>9.1} {:>7} {:>5}‰ {:>8.2}",
            name,
            body.vol.nx,
            body.vol.ny,
            body.vol.nz,
            m(lx),
            m(ly),
            m(lz),
            body.solid_count(),
            vol,
            body.bytes(),
            fp,
            us
        );
    }

    // Разброс — то, ради чего набор и заведён.
    let lens: Vec<u32> = bodies.iter().map(|(_, b)| b.extent_mm().0).collect();
    let (lo, hi) = (
        *lens.iter().min().expect("набор пуст"),
        *lens.iter().max().expect("набор пуст"),
    );
    println!(
        "\nразброс длин: {:.1} м .. {:.1} м, отношение {}x",
        lo as f64 / 1000.0,
        hi as f64 / 1000.0,
        hi / lo
    );

    // Плотность деталей: то, ради чего колонка стала многословной.
    // Метровый воксель давал гигантам вчетверо меньше вокселей на метр,
    // и «большая машина» выходила «машиной из кубиков покрупнее».
    let bad: Vec<&str> = bodies
        .iter()
        .filter(|(_, b)| b.vol.nx as u32 * 1000 / b.extent_mm().0 != 4)
        .map(|(n, _)| *n)
        .collect();
    println!("вокселей на метр: у всех 4, исключения: {bad:?}");
    println!(
        "слов в колонке: {}",
        bodies
            .iter()
            .map(|(n, b)| format!("{n} {}", b.vol.words))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Силуэт сверху. Заполненность следа и число дворов — то, чем
    // постройки отличаются друг от друга с высоты боевой камеры.
    println!("\nСИЛУЭТ СВЕРХУ (заполненность следа, дворов)\n");
    for (name, fp, yards) in &rows {
        println!("{name:<28} {fp:>4}‰  дворов {yards}");
    }

    // Обломки. Куча вдвое грубее машины по сетке и обязана держать
    // её материю до кубического миллиметра.
    println!("
ОБЛОМКИ (машина оседает, материя сохраняется)
");
    println!(
        "{:<28} {:>12} {:>9} {:>8} {:>10} {:>11} {:>7} {:>8}",
        "машина", "куча", "вокселей", "высота", "материя м3", "вместимость", "байт", "сборка"
    );
    for (name, body) in &bodies {
        // Сборка обломка — операция на смерть машины: один проход
        // по вокселям тела. Прогрев, потом замер, как у связности.
        for _ in 0..4 {
            std::hint::black_box(body.wreck(0));
        }
        let n = 64;
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(body.wreck(0));
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;

        let w = body.wreck(0);
        let (_, wy, _) = w.extent_mm();
        let (_, by, _) = body.extent_mm();
        assert_eq!(w.matter_mm3(), body.volume_mm3(), "{name}: материя не сошлась");
        println!(
            "{:<28} {:>3}x{:<3}x{:<4} {:>9} {:>4.1}/{:<3.1} {:>10.1} {:>11.1} {:>7} {:>7.2}",
            name,
            w.vol.nx,
            w.vol.ny,
            w.vol.nz,
            w.solid_count(),
            wy as f64 / 1000.0,
            by as f64 / 1000.0,
            w.matter_mm3() as f64 / 1e9,
            w.capacity_mm3() as f64 / 1e9,
            w.bytes(),
            us
        );
    }

    // Насколько силуэты **не совпадают** как формы: общая сетка 16x16,
    // 256 клеток. Размер и так различает постройки; вопрос в форме.
    let base = voxel::roster::buildings();
    println!("
РАЗЛИЧИЕ СИЛУЭТОВ (клеток из 256, сетка 16x16)
");
    for i in 0..base.len() {
        for j in i + 1..base.len() {
            println!(
                "{:<10} / {:<10} {:>4}",
                base[i].0,
                base[j].0,
                silhouette_distance(&silhouette(&base[i].1.vol, 16), &silhouette(&base[j].1.vol, 16))
            );
        }
    }

    // Карты: по умолчанию только у построек — именно они обязаны
    // читаться сверху. `--views` печатает весь набор.
    println!();
    let base: Vec<&'static str> = base.iter().map(|(n, _)| *n).collect();
    for (name, body) in &bodies {
        let building = base.contains(name);
        if !views && !building {
            continue;
        }
        println!("{name} — вид сверху:");
        print_top(body, &top_view(body), 48);
        println!();
    }
}
