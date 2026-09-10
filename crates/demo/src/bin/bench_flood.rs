//! Замер: **сколько стоит прорыв большого озера** (ADR-145).
//!
//! Вода не воксели (ADR-049): водоём — контейнер, а `level_shown`
//! догоняет уровень со скоростью из данных. Связность при этом
//! считается на графе пустот **ниже уровня**, и подъём уровня только
//! добавляет рёбра. Значит цена прорыва — это цена подъёма уровня
//! по этому графу: сколько пустот входит под воду за шаг и сколько
//! компонент при этом сливается.
//!
//! Мерить это можно **не реализуя воду**: граф пустот у выработанной
//! карты уже есть, а уровень — просто плоскость, которую по нему
//! проводят.
//!
//! Сгенерированный мир для этого не годится: у генератора нет пещер,
//! пустота над рельефом едина, и сливаться нечему. Годится
//! **выработанная** карта — та самая механика, ради которой ADR-049
//! и написан: прокопал в озеро — затопил свою шахту.

use demo::dig::Dig;
use demo::Terrain;
use std::process::ExitCode;
use std::time::Instant;
use voxel::World;

struct Args {
    side: u32,
    height: u32,
    diggers: usize,
    ticks: u64,
    seed: u64,
    /// На сколько вокселей уровень догоняет за тик (ADR-049: скорость
    /// из данных). Оговорка замера, а не факт движка.
    rate: u32,
}

fn parse() -> Result<Args, String> {
    let mut a = Args { side: 256, height: 128, diggers: 300, ticks: 2000, seed: 42, rate: 1 };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut num = |name: &str| -> Result<u64, String> {
            it.next()
                .ok_or_else(|| format!("{name} без значения"))?
                .parse::<u64>()
                .map_err(|e| format!("{name}: {e}"))
        };
        match k.as_str() {
            "--side" => a.side = num("--side")? as u32,
            "--height" => a.height = num("--height")? as u32,
            "--diggers" => a.diggers = num("--diggers")? as usize,
            "--ticks" => a.ticks = num("--ticks")?,
            "--seed" => a.seed = num("--seed")?,
            "--rate" => a.rate = num("--rate")? as u32,
            "--help" => {
                println!("bench_flood [--side N] [--height N] [--diggers N] [--ticks N] [--rate N]");
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ {other}")),
        }
    }
    Ok(a)
}

/// Пробег пустоты в колонке: `[bot, top)`.
#[derive(Clone, Copy)]
struct Run {
    bot: u32,
    top: u32,
}

/// Корень с сжатием пути. Union-find по пробегам, а не по вокселям
/// (ADR-073): у выработанной карты пробегов на два порядка меньше.
fn find(p: &mut [u32], mut i: u32) -> u32 {
    while p[i as usize] != i {
        p[i as usize] = p[p[i as usize] as usize];
        i = p[i as usize];
    }
    i
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };

    let params = worldgen::Params {
        nx: a.side,
        ny: a.height,
        nz: a.side,
        ..Default::default()
    };
    let terrain = Terrain { params, seed: a.seed };
    let mut w = World::new(
        params.nx,
        params.ny,
        params.nz,
        1 << 16,
        Box::new(Terrain { params, seed: a.seed }),
    );

    println!("прорыв озера по выработанной карте (ADR-145)");
    println!("мир {}×{}×{}, {} копателей, {} тиков копки\n", a.side, a.height, a.side, a.diggers, a.ticks);

    // --- 1. Выработать карту.
    let t = Instant::now();
    let mut dig = Dig::clustered(&w, |x, z| terrain.surface(x, z), a.diggers, 2, 12, a.seed ^ 0xF10);
    let mut edits = 0u64;
    for _ in 0..a.ticks {
        dig.step(&mut w, &|x, z| terrain.surface(x, z));
        edits += w.flush().applied as u64;
    }
    println!("{:<38} {:>12}", "правок копки", edits);
    println!("{:<38} {:>9.1} с", "копка", t.elapsed().as_secs_f64());

    // --- 2. Пробеги пустоты по колонкам.
    let t = Instant::now();
    let (nx, ny, nz) = (w.nx, w.ny, w.nz);
    let ncol = (nx * nz) as usize;
    let mut runs: Vec<Run> = Vec::new();
    // Начало пробегов колонки в общем массиве.
    let mut col_start: Vec<u32> = Vec::with_capacity(ncol + 1);
    for z in 0..nz {
        for x in 0..nx {
            col_start.push(runs.len() as u32);
            let mut y = 0;
            while y < ny {
                if w.solid(x, y, z) {
                    y += 1;
                    continue;
                }
                let bot = y;
                while y < ny && !w.solid(x, y, z) {
                    y += 1;
                }
                runs.push(Run { bot, top: y });
            }
        }
    }
    col_start.push(runs.len() as u32);
    let scan_s = t.elapsed().as_secs_f64();
    let void_cells: u64 = runs.iter().map(|r| (r.top - r.bot) as u64).sum();
    println!("{:<38} {:>12}", "пробегов пустоты", runs.len());
    println!("{:<38} {:>12}", "вокселей пустоты", void_cells);
    println!("{:<38} {:>9.1} с", "разбор колонок", scan_s);

    // --- 3. Рёбра между пробегами соседних колонок.
    //
    // Два пробега связаны **ниже уровня**, как только уровень поднялся
    // выше их общего дна: до этого их перекрытие сухое. Поэтому у ребра
    // есть уровень включения, и весь подъём — это разовая обработка
    // отсортированного по нему списка.
    let t = Instant::now();
    let mut edges: Vec<(u32, u32, u32)> = Vec::new(); // (уровень, a, b)
    let col = |x: u32, z: u32| (z * nx + x) as usize;
    let link = |edges: &mut Vec<(u32, u32, u32)>, ca: usize, cb: usize| {
        let (sa, ea) = (col_start[ca] as usize, col_start[ca + 1] as usize);
        let (sb, eb) = (col_start[cb] as usize, col_start[cb + 1] as usize);
        let (mut i, mut j) = (sa, sb);
        while i < ea && j < eb {
            let (ra, rb) = (runs[i], runs[j]);
            let lo = ra.bot.max(rb.bot);
            let hi = ra.top.min(rb.top);
            if lo < hi {
                edges.push((lo + 1, i as u32, j as u32));
            }
            if ra.top < rb.top {
                i += 1;
            } else {
                j += 1;
            }
        }
    };
    for z in 0..nz {
        for x in 0..nx {
            if x + 1 < nx {
                link(&mut edges, col(x, z), col(x + 1, z));
            }
            if z + 1 < nz {
                link(&mut edges, col(x, z), col(x, z + 1));
            }
        }
    }
    let edge_s = t.elapsed().as_secs_f64();
    println!("{:<38} {:>12}", "рёбер между пробегами", edges.len());
    println!("{:<38} {:>9.1} с\n", "сборка рёбер", edge_s);

    // --- 4. Подъём уровня.
    let mut by_level: Vec<Vec<u32>> = vec![Vec::new(); (ny + 2) as usize];
    for (k, r) in runs.iter().enumerate() {
        by_level[(r.bot + 1) as usize].push(k as u32);
    }
    let mut e_by_level: Vec<Vec<u32>> = vec![Vec::new(); (ny + 2) as usize];
    for (k, e) in edges.iter().enumerate() {
        e_by_level[e.0 as usize].push(k as u32);
    }
    let mut parent: Vec<u32> = Vec::new();
    let mut present = vec![false; runs.len()];
    let mut alive: i64;
    // Площадь пустот на слое — тот самый `void_area_at_z` из ADR-049.
    let mut area = vec![0u64; ny as usize];
    for r in runs.iter() {
        for y in r.bot..r.top {
            area[y as usize] += 1;
        }
    }

    // Подъём гоняется пять раз, и от каждого шага берётся **лучшее**
    // время. Шаг стоит десятки микросекунд, а разброс планировщика —
    // сотни: первый прогон дал пик времени на уровне 68, где работы
    // в десять раз меньше, чем на уровне 121. Такой пик — это шум,
    // а не замер.
    let mut per_level: Vec<(u32, u32, u32, i64, f64)> = Vec::new();
    let mut sweep_ms = f64::MAX;
    for rep in 0..5 {
        parent.clear();
        parent.extend(0..runs.len() as u32);
        present.iter_mut().for_each(|p| *p = false);
        alive = 0;
        let t_all = Instant::now();
        for l in 1..=ny {
            let t = Instant::now();
            let mut added = 0u32;
            for &k in by_level[l as usize].iter() {
                present[k as usize] = true;
                alive += 1;
                added += 1;
            }
            let mut merged = 0u32;
            for &ek in e_by_level[l as usize].iter() {
                let (_, ia, ib) = edges[ek as usize];
                if !present[ia as usize] || !present[ib as usize] {
                    continue;
                }
                let (ra, rb) = (find(&mut parent, ia), find(&mut parent, ib));
                if ra != rb {
                    // Ничьи разрешаются номером, а не порядком обхода.
                    let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
                    parent[hi as usize] = lo;
                    alive -= 1;
                    merged += 1;
                }
            }
            let us = t.elapsed().as_secs_f64() * 1e6;
            if rep == 0 {
                per_level.push((l, added, merged, alive, us));
            } else {
                let e = &mut per_level[(l - 1) as usize];
                debug_assert_eq!(e.1, added, "работа шага разошлась между прогонами");
                debug_assert_eq!(e.2, merged, "работа шага разошлась между прогонами");
                e.4 = e.4.min(us);
            }
        }
        sweep_ms = sweep_ms.min(t_all.elapsed().as_secs_f64() * 1e3);
    }

    let peak_add = per_level.iter().max_by_key(|r| r.1).copied().unwrap();
    let peak_merge = per_level.iter().max_by_key(|r| r.2).copied().unwrap();
    let peak_us = per_level
        .iter()
        .max_by(|a, b| a.4.partial_cmp(&b.4).unwrap())
        .copied()
        .unwrap();
    let peak_area = area.iter().enumerate().max_by_key(|(_, &v)| v).unwrap();
    let max_alive = per_level.iter().map(|r| r.3).max().unwrap();

    println!("{:<38} {:>12}", "подъём уровня, шагов", ny);
    println!("{:<38} {:>9.1} мс", "весь подъём", sweep_ms);
    println!(
        "{:<38} {:>12} {}",
        "пик: пробегов вошло за шаг", peak_add.1,
        format_args!("(уровень {})", peak_add.0)
    );
    println!(
        "{:<38} {:>12} {}",
        "пик: слияний за шаг", peak_merge.2,
        format_args!("(уровень {})", peak_merge.0)
    );
    println!(
        "{:<38} {:>9.1} мкс {}",
        "пик: время шага", peak_us.4,
        format_args!("(уровень {})", peak_us.0)
    );
    {
        let mut by_us = per_level.clone();
        by_us.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap());
        println!("
самые дорогие шаги (уровень, вошло, слияний, компонент, мкс):");
        for r in by_us.iter().take(6) {
            println!("  {:>4} {:>8} {:>8} {:>8} {:>9.1}", r.0, r.1, r.2, r.3, r.4);
        }
        let mut by_work = per_level.clone();
        by_work.sort_by_key(|r| std::cmp::Reverse(r.2));
        println!("самые тяжёлые по работе:");
        for r in by_work.iter().take(6) {
            println!("  {:>4} {:>8} {:>8} {:>8} {:>9.1}", r.0, r.1, r.2, r.3, r.4);
        }
    }
    println!("{:<38} {:>12}", "максимум живых компонент", max_alive);
    println!(
        "{:<38} {:>12} {}",
        "пик площади пустот на слое", peak_area.1,
        format_args!("(слой {})", peak_area.0)
    );

    // Догон уровня идёт по `rate` вокселей за тик; окно тика — это
    // `rate` подряд идущих шагов.
    let r = a.rate.max(1) as usize;
    let mut worst_tick = (0usize, 0u32, 0u32, 0.0f64);
    for s in 0..per_level.len().saturating_sub(r - 1) {
        let win = &per_level[s..s + r];
        let add: u32 = win.iter().map(|w| w.1).sum();
        let mer: u32 = win.iter().map(|w| w.2).sum();
        let us: f64 = win.iter().map(|w| w.4).sum();
        if us > worst_tick.3 {
            worst_tick = (win[0].0 as usize, add, mer, us);
        }
    }
    println!(
        "\nпри догоне {} воксел(я/ей) уровня за тик худший тик:",
        a.rate
    );
    println!(
        "{:<38} {:>12}",
        "  пробегов вошло", worst_tick.1
    );
    println!("{:<38} {:>12}", "  слияний", worst_tick.2);
    println!("{:<38} {:>9.1} мкс", "  время", worst_tick.3);
    println!("{:<38} {:>12}", "  на уровне", worst_tick.0);
    ExitCode::SUCCESS
}
