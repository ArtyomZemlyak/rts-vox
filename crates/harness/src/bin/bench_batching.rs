//! Копить и считать пачкой: сколько это даёт **на процессоре**.
//!
//! Запуск: `cargo run --release -p harness --bin bench_batching`
//!
//! Замер на видеокарте (`gpu/bench_submit`) показал, что пачка выигрывает
//! там за счёт постоянной цены обращения: 49 мкс за отправку против 1.9 мкс
//! за следующий диспатч. На процессоре постоянной цены обращения нет,
//! и выигрыш может взяться только с одного места — **если грязные
//! множества соседних тиков пересекаются**. Один и тот же сектор,
//! испачканный на тиках T и T+2, при пачке решается один раз, а не дважды.
//!
//! Это не рассуждение, а измеримая величина: отношение суммы потиковых
//! размеров грязного множества к размеру их объединения.
//!
//! **Копатели обязаны идти вразнобой.** `Dig` продвигает копателя раз
//! в три тика по `age % 3`, а `age` у всех стартует с нуля — то есть все
//! четыреста продвигаются одновременно. В таком стенде каждый третий тик
//! толстый, а два пустых, и окно из двух тиков **по построению** не может
//! ничего склеить: оно всегда ловит один толстый и один пустой. Ниже
//! меряются обе модели, и разница между ними — целиком свойство стенда.
//!
//! Отставание, которым платят за пачку, разрешено не всему. `CLAUDE.md`
//! разрешает его **рендеру** (значит, ремешу) и **интегрированию flow
//! field** (значит, эйконалу). Проходимость, связность и коллизия обязаны
//! видеть дыру в тот же тик, поэтому их здесь нет.

use std::time::Instant;

use fx::rng::Rng;
use harness::{Dig, World};
use meshing::CS as MCS;

/// Сторона нав-сектора в вокселях: 16 нав-клеток по 2 воксела (ADR-066).
const SECTOR_VOX: usize = 32;
const TICKS: usize = 600;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

/// Развести копателей по фазе продвижения.
fn stagger(dig: &mut Dig) {
    let mut r = Rng::new(77, 0xA6E);
    for d in dig.diggers.iter_mut() {
        d.age = r.below(3);
    }
}

/// Средний размер объединения грязного множества за окно `win` тиков,
/// приведённый к одному тику.
fn per_tick_union(per_tick: &[Vec<u32>], win: usize, universe: usize) -> f64 {
    let mut stamp = vec![u32::MAX; universe];
    let mut union = 0usize;
    let mut windows = 0usize;
    for (w, chunk) in per_tick.chunks(win).enumerate() {
        windows += 1;
        for t in chunk {
            for &e in t {
                if stamp[e as usize] != w as u32 {
                    stamp[e as usize] = w as u32;
                    union += 1;
                }
            }
        }
    }
    union as f64 / (windows * win) as f64
}

struct Trace {
    sectors: Vec<Vec<u32>>,
    chunks: Vec<Vec<u32>>,
    n_sectors: usize,
    n_chunks: usize,
}

fn trace(staggered: bool) -> Trace {
    let mut w = World::new(10, 10, 21);
    let mut dig = Dig::clustered(&w, 400, 2, 3, 8);
    if staggered {
        stagger(&mut dig);
    }
    let mut out = Vec::new();
    let sw = w.voxels_x().div_ceil(SECTOR_VOX);
    let n_sectors = sw * w.voxels_z().div_ceil(SECTOR_VOX);
    let n_chunks = w.chunk_count();
    let mut sectors = Vec::with_capacity(TICKS);
    let mut chunks = Vec::with_capacity(TICKS);
    let mut sec_seen = vec![u32::MAX; n_sectors];
    let mut chk_seen = vec![u32::MAX; n_chunks];
    for t in 0..TICKS {
        dig.step(&mut w, &mut out);
        let (mut s, mut c) = (Vec::new(), Vec::new());
        for &(x, _, z) in out.iter() {
            let si = (z / SECTOR_VOX) * sw + x / SECTOR_VOX;
            if sec_seen[si] != t as u32 {
                sec_seen[si] = t as u32;
                s.push(si as u32);
            }
            let ci = (z / MCS) * w.nx + x / MCS;
            if chk_seen[ci] != t as u32 {
                chk_seen[ci] = t as u32;
                c.push(ci as u32);
            }
        }
        sectors.push(s);
        chunks.push(c);
        w.flush_mesh();
    }
    Trace {
        sectors,
        chunks,
        n_sectors,
        n_chunks,
    }
}

fn main() {
    println!("копка: 400 копателей, 8 забоев, {TICKS} тиков");

    let sync = trace(false);
    let stag = trace(true);

    section("1. Ритм стенда: копатели в фазе против копателей вразнобой");
    println!("доля тиков, в которые не выкопано ничего.\n");
    for (name, t) in [("в фазе", &sync), ("вразнобой", &stag)] {
        let empty = t.sectors.iter().filter(|v| v.is_empty()).count();
        let max = t.sectors.iter().map(|v| v.len()).max().unwrap_or(0);
        println!(
            "{name:<12} пустых тиков {:>5.1}%, самый толстый {max} секторов",
            100.0 * empty as f64 / TICKS as f64
        );
    }

    section("2. Склеиваются ли грязные множества между тиками");
    println!("если каждый тик пачкает свои, непересекающиеся, — пачка даёт ноль.\n");
    println!(
        "{:>5} {:>26} {:>26}",
        "окно", "нав-секторов на тик", "чанков меша на тик"
    );
    println!(
        "{:>5} {:>12} {:>13} {:>12} {:>13}",
        "", "в фазе", "вразнобой", "в фазе", "вразнобой"
    );
    let b = [
        per_tick_union(&sync.sectors, 1, sync.n_sectors),
        per_tick_union(&stag.sectors, 1, stag.n_sectors),
        per_tick_union(&sync.chunks, 1, sync.n_chunks),
        per_tick_union(&stag.chunks, 1, stag.n_chunks),
    ];
    for win in [1usize, 2, 4, 8, 16, 32] {
        let v = [
            per_tick_union(&sync.sectors, win, sync.n_sectors),
            per_tick_union(&stag.sectors, win, stag.n_sectors),
            per_tick_union(&sync.chunks, win, sync.n_chunks),
            per_tick_union(&stag.chunks, win, stag.n_chunks),
        ];
        println!(
            "{win:>5} {:>12.1} {:>13.1} {:>12.1} {:>13.1}",
            v[0], v[1], v[2], v[3]
        );
    }
    println!();
    println!(
        "{:>5} {:>26} {:>26}",
        "окно", "экономия по секторам", "экономия по чанкам"
    );
    for win in [2usize, 4, 8, 16, 32] {
        let v = [
            per_tick_union(&sync.sectors, win, sync.n_sectors),
            per_tick_union(&stag.sectors, win, stag.n_sectors),
            per_tick_union(&sync.chunks, win, sync.n_chunks),
            per_tick_union(&stag.chunks, win, stag.n_chunks),
        ];
        println!(
            "{win:>5} {:>12.2}x {:>12.2}x {:>12.2}x {:>12.2}x",
            b[0] / v[0],
            b[1] / v[1],
            b[2] / v[2],
            b[3] / v[3]
        );
    }

    section("3. Ремеш пачкой: настоящее время, а не счёт");
    println!("ремеш — рендер, отставание ему разрешено (`CLAUDE.md`).");
    println!("копатели вразнобой.\n");
    println!(
        "{:>5} {:>16} {:>16} {:>12}",
        "окно", "мс за 600 тиков", "мкс на тик", "выигрыш"
    );
    let mut base = 0.0f64;
    for win in [1usize, 2, 4, 8, 16] {
        let mut best = f64::MAX;
        for _ in 0..3 {
            let mut w = World::new(10, 10, 21);
            let mut dig = Dig::clustered(&w, 400, 2, 3, 8);
            stagger(&mut dig);
            let mut out = Vec::new();
            let t0 = Instant::now();
            for t in 0..TICKS {
                dig.step(&mut w, &mut out);
                if (t + 1) % win == 0 {
                    w.flush_mesh();
                }
            }
            w.flush_mesh();
            best = best.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        if win == 1 {
            base = best;
        }
        println!(
            "{win:>5} {best:>16.1} {:>16.1} {:>11.2}x",
            best * 1000.0 / TICKS as f64,
            base / best
        );
    }
    println!();
    println!("в цифре сидит и сама копка, поэтому выигрыш ремеша занижен;");
    println!("верхняя граница — колонка «чанков меша, вразнобой» из раздела 2.");

    section("4. Чему отставание запрещено");
    println!("проходимость, связность породы, коллизия тел — тот же тик");
    println!("(`CLAUDE.md`, «мир меняется для симуляции в тот же тик»).");
    println!("им пачка недоступна ни при каком выигрыше.");
}
