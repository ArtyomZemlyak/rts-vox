//! Headless-прогон эталонного сценария — боя на полигоне (ADR-662).
//!
//! ```text
//! sim-cli --ticks 600 --dump-hash
//! ```
//!
//! Ни рендера, ни окна, ни видеокарты. Весь тестовый контур проекта
//! headless (`analysis/09-gpu-compute.md`, раздел 2), и этот бинарник —
//! то, чем он запускается вручную.
//!
//! **Хеш печатается на stdout одной строкой и является контрактом.**
//! Он же сравнивается между Windows и Linux и между числом воркеров.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sim_core::blast::BlastRange;

struct Args {
    ticks: u64,
    threads: usize,
    dump_hash: bool,
    every: u64,
    stats: bool,
    /// Куда записать состояние после прогона (ADR-229).
    save: Option<PathBuf>,
    /// Откуда взять состояние вместо начала боя.
    load: Option<PathBuf>,
    /// Печатать размер файла и время записи-чтения.
    save_stats: bool,
    profile_csv: Option<PathBuf>,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            ticks: 600,
            threads: 1,
            dump_hash: false,
            every: 0,
            stats: false,
            save: None,
            load: None,
            save_stats: false,
            profile_csv: None,
        }
    }
}

fn usage() -> &'static str {
    "\
sim-cli — headless-прогон боя на полигоне

  --ticks N       сколько тиков (600)
  --threads N     воркеров физики, 1..8; фиксируется явно (1)
  --every N       печатать хеш каждые N тиков, а не только итоговый
  --dump-hash     напечатать итоговый хеш состояния
  --stats         напечатать время тика и то, что бой нагрузил:
                  выстрелы, смерти, обрушения, обломки
  --save ФАЙЛ     записать состояние после прогона (ADR-229)
  --load ФАЙЛ     продолжить с записанного состояния, а не с начала боя
  --profile-csv FILE  записать время каждого тика в CSV
  --save-stats    напечатать размер файла и время записи-чтения
  --help          это сообщение
"
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
            "--ticks" => a.ticks = num("--ticks")?,
            "--threads" => a.threads = num("--threads")? as usize,
            "--every" => a.every = num("--every")?,
            "--dump-hash" => a.dump_hash = true,
            "--stats" => a.stats = true,
            "--save-stats" => a.save_stats = true,
            "--profile-csv" | "--save" | "--load" => {
                let p = PathBuf::from(it.next().ok_or_else(|| format!("{k}: нет имени файла"))?);
                match k.as_str() {
                    "--profile-csv" => a.profile_csv = Some(p),
                    "--save" => a.save = Some(p),
                    _ => a.load = Some(p),
                }
            }
            "--help" | "-h" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ: {other}")),
        }
    }
    Ok(a)
}

/// Файл данных ищется рядом с корнем репозитория, потом в текущем
/// каталоге: headless-прогон запускают и оттуда, и отсюда.
fn data(name: &str) -> Result<String, String> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data")
        .join(name);
    let path = if here.exists() {
        here
    } else {
        Path::new("data").join(name)
    };
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn run(a: &Args) -> Result<(), String> {
    // Ввод-вывод живёт здесь, а не в симуляции: `sim-core` принимает
    // разобранные данные и буфер, файл открывает вызывающий
    // (`CLAUDE.md`, границы слоёв). Сценарий собирает `sim-core`:
    // тесты детерминизма обязаны гонять ровно то, на чём стоит хеш
    // (ADR-157).
    let config = modding::explosion::ExplosionConfig::from_ron(&data("explosion.ron")?)
        .map_err(|e| format!("explosion.ron: {e}"))?;
    let mut sim = match &a.load {
        None => {
            let layout = modding::range::RangeLayout::from_ron(&data("range-km.ron")?)
                .map_err(|e| format!("range-km.ron: {e}"))?;
            let preset = modding::range_skirmish::WorldBattle::from_ron(&data("battle-world.ron")?)
                .map_err(|e| format!("battle-world.ron: {e}"))?;
            let t0 = std::time::Instant::now();
            let sim = BlastRange::world_battle(config, layout, preset)?;
            if a.save_stats {
                println!("холодный старт: {} мкс", t0.elapsed().as_micros());
            }
            sim
        }
        Some(p) => {
            let buf = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
            // Замер разбора отделён от чтения с диска: диск — не формат.
            let t0 = std::time::Instant::now();
            let sim = BlastRange::restore(config, &buf)
                .ok_or_else(|| format!("{}: снимок не принят", p.display()))?;
            if a.save_stats {
                println!(
                    "загрузка: {} байт, разбор {} мкс, тик {}",
                    buf.len(),
                    t0.elapsed().as_micros(),
                    sim.tick
                );
            }
            sim
        }
    };
    if !sim.set_physics_workers(a.threads) {
        return Err(format!("--threads {}: допустимо 1..8", a.threads));
    }

    let mut wall_us: Vec<u64> = Vec::with_capacity(a.ticks as usize);
    let mut waves = 0usize;
    for t in 1..=a.ticks {
        let t0 = std::time::Instant::now();
        sim.step();
        wall_us.push(t0.elapsed().as_micros() as u64);
        waves = waves.max(sim.battle.wave_counts().iter().sum());
        if a.every > 0 && t % a.every == 0 {
            println!("tick={} hash={:016x}", sim.tick, sim.state_hash());
        }
    }
    let hash = sim.state_hash();

    if let Some(p) = &a.profile_csv {
        let mut csv = String::from("tick,wall_us\n");
        for (i, us) in wall_us.iter().enumerate() {
            csv.push_str(&format!("{},{us}\n", i + 1));
        }
        std::fs::write(p, csv).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    if a.stats {
        // Что сценарий на самом деле нагружает: хеш над холостой фазой
        // ничего не закрепляет (ADR-116).
        let mut sorted = wall_us.clone();
        sorted.sort_unstable();
        if let Some(&max) = sorted.last() {
            let total: u64 = sorted.iter().sum();
            println!(
                "  тик: средний {} мкс, медиана {} мкс, p95 {} мкс, худший {max} мкс",
                total / sorted.len() as u64,
                sorted[sorted.len() / 2],
                sorted[sorted.len() * 95 / 100],
            );
        }
        let fired = [true, false].map(|side| {
            (0..sim.units.len())
                .filter(|&id| sim.allied(id) == side)
                .map(|id| sim.battle.actors[id].fired)
                .sum::<u32>()
        });
        let dead = sim.units.iter().filter(|u| !u.status.alive).count();
        println!(
            "  выстрелов: синие {}, красные {}; погибло {dead} из {}",
            fired[0],
            fired[1],
            sim.units.len()
        );
        println!(
            "  обрушено клеток {}, волн одновременно до {waves}, обломков {}",
            sim.collapsed_cells,
            sim.matter.debris.fragments.len()
        );
        println!(
            "  баланс материи: {}",
            if sim.matter.economy.ledger.balanced() {
                "сходится"
            } else {
                "НЕ сходится"
            }
        );
    }

    if a.dump_hash || a.every == 0 {
        println!(
            "battle ticks={} threads={} hash={hash:016x}",
            sim.tick, a.threads
        );
    }

    if let Some(p) = &a.save {
        let t0 = std::time::Instant::now();
        let buf = sim.snapshot();
        let build_us = t0.elapsed().as_micros();
        let t1 = std::time::Instant::now();
        std::fs::write(p, &buf).map_err(|e| format!("{}: {e}", p.display()))?;
        if a.save_stats {
            println!(
                "сохранение: {} байт, сборка {build_us} мкс, запись на диск {} мкс, тик {}",
                buf.len(),
                t1.elapsed().as_micros(),
                sim.tick
            );
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
    };
    match run(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}
