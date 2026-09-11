//! Отчёт по набору тел: размер, объём, память, цена проверки связности.
//!
//! Headless и из терминала — числа в ADR берутся отсюда, а не с экрана.

use std::time::Instant;
use voxel::topo::unit::CoreBufs;

fn main() {
    let bodies = voxel::roster::all();
    println!("НАБОР ТЕЛ\n");
    println!(
        "{:<24} {:>12} {:>7} {:>16} {:>10} {:>9} {:>9} {:>10}",
        "машина", "том", "воксель", "габарит, м", "твёрдых", "объём м3", "байт", "связность"
    );

    let mut b = CoreBufs::default();
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

        println!(
            "{:<24} {:>3}x{:<3}x{:<4} {:>6} {:>6.1}x{:<4.1}x{:<5.1} {:>10} {:>9.1} {:>9} {:>8.2}",
            name,
            body.vol.nx,
            body.vol.ny,
            body.vol.nz,
            format!("{} мм", body.mm),
            m(lx),
            m(ly),
            m(lz),
            body.solid_count(),
            vol,
            body.bytes(),
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

    // Проверка правила «ресурс в мировых единицах, не в вокселях».
    let tank = &bodies[0].1;
    let big = bodies.last().expect("набор пуст");
    println!(
        "воксель крепости стоит {} вокселей танка по объёму ({} мм против {} мм ребро)",
        (big.1.mm as u64).pow(3) / (tank.mm as u64).pow(3),
        big.1.mm,
        tank.mm
    );
}
