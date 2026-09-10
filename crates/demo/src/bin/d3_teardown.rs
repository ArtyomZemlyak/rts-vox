//! **D-3 «Разбор танка»** — воксельные юниты с помеченными подсистемами
//! (`docs/14-demo-scenes.md`, ADR-095).
//!
//! ```text
//! d3_teardown --seed 42 --shots 400 --dump-hash
//! ```
//!
//! Один танк, один стрелок, попадания по выбранным местам. Отстрел
//! гусеницы останавливает, отстрел орудия обезоруживает, пробитие
//! к ядру убивает. Броня работает как геометрия, а не как число.
//!
//! **Порог:** боевая фаза на 3 456 вокселях тела укладывается в бюджет,
//! и связность частей с ядром пересчитывается **за время попадания,
//! а не тика**.

use std::process::ExitCode;
use std::time::Instant;

use demo::RunHash;
use fx::rng::Rng;
use voxel::topo::unit::CoreBufs;
use voxel::unit::{tank, Part, UnitBody};

/// Порог на пересчёт связности после одного попадания, микросекунд.
/// Тик 10 мс; попаданий за тик до двух тысяч (ADR-079), значит
/// пересчёт обязан стоить единицы микросекунд, а не десятки.
const BUDGET_US: f64 = 5.0;

struct Args {
    seed: u64,
    shots: usize,
    radius: i64,
    dump_hash: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args { seed: 42, shots: 400, radius: 2, dump_hash: false }
    }
}

fn parse() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut num = |what: &str| -> Result<u64, String> {
            it.next()
                .ok_or_else(|| format!("{what}: нет значения"))?
                .parse::<u64>()
                .map_err(|e| format!("{what}: {e}"))
        };
        match k.as_str() {
            "--seed" => a.seed = num("--seed")?,
            "--shots" => a.shots = num("--shots")? as usize,
            "--radius" => a.radius = num("--radius")? as i64,
            "--dump-hash" => a.dump_hash = true,
            "--help" | "-h" => {
                println!("D-3 «Разбор танка»\n\n  --seed N --shots N --radius N --dump-hash\n");
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
}

/// Снаряд: сфера радиуса `r` вокруг точки. Возвращает, сколько вокселей
/// снял.
fn hit(u: &mut UnitBody, x: i64, y: i64, z: i64, r: i64) -> u32 {
    let mut n = 0;
    for dz in -r..=r {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy + dz * dz > r * r {
                    continue;
                }
                let (px, py, pz) = (x + dx, y + dy, z + dz);
                if px < 0 || py < 0 || pz < 0 {
                    continue;
                }
                let (px, py, pz) = (px as usize, py as usize, pz as usize);
                if px >= u.vol.nx as usize || py >= u.vol.ny as usize || pz >= u.vol.nz as usize {
                    continue;
                }
                n += u.remove(px, py, pz) as u32;
            }
        }
    }
    n
}

/// Найти воксель заданной подсистемы — точка прицеливания.
fn aim(u: &UnitBody, p: Part) -> Option<(i64, i64, i64)> {
    for z in 0..u.vol.nz as usize {
        for y in 0..u.vol.ny as usize {
            for x in 0..u.vol.nx as usize {
                if u.vol.get(x, y, z) && u.part_at(x, y, z) == p {
                    return Some((x as i64, y as i64, z as i64));
                }
            }
        }
    }
    None
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut bufs = CoreBufs::new();
    let mut hash = RunHash::new();

    println!("D-3 «Разбор танка» — воксельные юниты с подсистемами");
    let t0 = tank();
    println!(
        "том {}×{}×{} = {} вокселей, твёрдых {}, память тома {} КиБ\n",
        t0.vol.nx,
        t0.vol.ny,
        t0.vol.nz,
        t0.vol.voxels(),
        t0.solid_count(),
        t0.bytes() / 1024
    );

    // --- 1. Прицельный отстрел: каждая подсистема по очереди.
    println!("{:<22} {:>8} {:>8} {:>8} {:>8} {:>10}", "выстрел", "жив", "двиг.", "оруд.", "сенс.", "снято");
    let mut ok = true;
    for target in [Part::Engine, Part::Weapon, Part::Sensor, Part::Core] {
        let mut t = tank();
        let mut removed = 0;
        // Бьём в одну точку, пока подсистема не откажет.
        let mut shots = 0;
        let s = loop {
            let Some((x, y, z)) = aim(&t, target) else { break t.status(&mut bufs) };
            removed += hit(&mut t, x, y, z, a.radius);
            shots += 1;
            let s = t.status(&mut bufs);
            if !s.get(target) || shots > 200 {
                break s;
            }
        };
        println!(
            "{:<22} {:>8} {:>8} {:>8} {:>8} {:>10}",
            format!("по {} ({shots})", target.name()),
            s.alive,
            s.engine,
            s.weapon,
            s.sensor,
            removed
        );
        hash.feed(s.attached as u64);
        hash.feed(s.detached as u64);
        // Отстрел подсистемы обязан её отключить, а ядро — убить.
        if target == Part::Core {
            if s.alive {
                println!("  ПРОБИТИЕ К ЯДРУ НЕ УБИЛО");
                ok = false;
            }
        } else {
            if s.get(target) {
                println!("  {} не отключилась", target.name());
                ok = false;
            }
            if !s.alive {
                println!("  танк умер от потери {} — это не пробитие ядра", target.name());
                ok = false;
            }
        }
    }

    // --- 2. Случайный обстрел: сколько живёт танк и что отваливается.
    let mut t = tank();
    let mut r = Rng::new(a.seed, 0x3D);
    let (mut fired, mut removed, mut detached_peak) = (0usize, 0u32, 0u32);
    let mut lost = [0usize; 5];
    let mut prev = t.status(&mut bufs);
    let t_start = Instant::now();
    for _ in 0..a.shots {
        let x = r.below(t.vol.nx as u32) as i64;
        let y = r.below(t.vol.ny as u32) as i64;
        let z = r.below(t.vol.nz as u32) as i64;
        removed += hit(&mut t, x, y, z, a.radius);
        fired += 1;
        let s = t.status(&mut bufs);
        detached_peak = detached_peak.max(s.detached);
        // Отказ засчитывается, **пока ядро живо**. Иначе гибель гасит
        // все подсистемы разом, и метрика показывает один и тот же
        // выстрел для всех четырёх строк — цифра, похожая на правду.
        if s.alive {
            for p in [Part::Engine, Part::Weapon, Part::Sensor] {
                if prev.get(p) && !s.get(p) {
                    lost[p as usize] = fired;
                }
            }
        }
        if prev.alive && !s.alive {
            lost[Part::Core as usize] = fired;
        }
        prev = s;
        hash.feed(s.attached as u64 * 31 + s.detached as u64);
        if !s.alive {
            break;
        }
    }
    let us_per_shot = t_start.elapsed().as_secs_f64() * 1e6 / fired as f64;

    println!("\n{:<30} {:>14}", "случайный обстрел", "");
    println!("{:<30} {:>14}", "выстрелов до гибели", fired);
    println!("{:<30} {:>14}", "снято вокселей", removed);
    println!("{:<30} {:>14}", "пик отвалившегося от ядра", detached_peak);
    for p in [Part::Engine, Part::Weapon, Part::Sensor, Part::Core] {
        let n = lost[p as usize];
        println!(
            "{:<30} {:>14}",
            format!("{} отказал на выстреле", p.name()),
            if n == 0 { "не отказал".to_string() } else { n.to_string() }
        );
    }

    // --- 3. Цена пересчёта связности.
    println!("\n{:<30} {:>14.3}", "мкс на выстрел (с пересчётом)", us_per_shot);
    let mut t = tank();
    hit(&mut t, 12, 6, 6, 3);
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t1 = Instant::now();
        let mut acc = 0u64;
        for _ in 0..1000 {
            acc += t.status(&mut bufs).attached as u64;
        }
        std::hint::black_box(acc);
        best = best.min(t1.elapsed().as_secs_f64() * 1e6 / 1000.0);
    }
    println!("{:<30} {:>14.3}", "мкс на пересчёт связности", best);
    println!(
        "{:<30} {:>14.0}",
        "таких пересчётов в тик 10 мс",
        10_000.0 / best
    );

    // Снятие вокселей отдельно от пересчёта: в потоке они складываются,
    // и надо знать, что из них дорого.
    println!(
        "\n{:<12} {:>10} {:>14} {:>14} {:>16}",
        "радиус", "вокселей", "снятие, мкс", "пересчёт, мкс", "2000 попаданий, мс"
    );
    for r in [1i64, 2, 3, 4] {
        let cells = (2 * r + 1).pow(3);
        let mut best_hit = f64::MAX;
        for _ in 0..5 {
            let mut t = tank();
            let t1 = Instant::now();
            let mut acc = 0u32;
            for k in 0..200 {
                acc += hit(&mut t, 12, 6, (k % 12) as i64, r);
            }
            std::hint::black_box(acc);
            best_hit = best_hit.min(t1.elapsed().as_secs_f64() * 1e6 / 200.0);
        }
        println!(
            "{r:<12} {cells:>10} {best_hit:>14.3} {best:>14.3} {:>16.2}",
            2000.0 * (best_hit + best) / 1000.0
        );
    }
    println!("(2000 попаданий за тик — верхняя оценка ADR-079)");

    println!("\nхеш прогона: {:016x}", hash.get());
    if a.dump_hash {
        println!("seed={} shots={} hash={:016x}", a.seed, a.shots, hash.get());
    }

    let verdict = if best <= BUDGET_US { "ПОРОГ ДЕРЖИТСЯ" } else { "ПОРОГ ПРЕВЫШЕН" };
    println!("порог {BUDGET_US} мкс на пересчёт: {best:.3} мкс — {verdict}");
    if best > BUDGET_US {
        ok = false;
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
