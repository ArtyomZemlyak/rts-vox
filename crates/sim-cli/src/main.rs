//! Headless-прогон симуляции: гейт этапа 0 из `docs/11-roadmap.md`.
//!
//! ```text
//! sim-cli --seed 42 --ticks 1000 --dump-hash
//! ```
//!
//! Ни рендера, ни окна, ни видеокарты. Весь тестовый контур проекта
//! headless (`analysis/09-gpu-compute.md`, раздел 2), и этот бинарник —
//! то, чем он запускается вручную.
//!
//! **Хеш печатается на stdout одной строкой и является контрактом.**
//! Он же сравнивается между Windows и Linux и между числом воркеров.

use std::process::ExitCode;

use sim_core::{populate, World};
use worldgen::Params;

struct Args {
    seed: u64,
    ticks: u64,
    units: usize,
    threads: usize,
    dump_hash: bool,
    every: u64,
    world_hash: bool,
    stats: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            seed: 42,
            ticks: 1000,
            units: 5000,
            threads: 1,
            dump_hash: false,
            every: 0,
            world_hash: false,
            stats: false,
        }
    }
}

fn usage() -> &'static str {
    "\
sim-cli — headless-прогон симуляции

  --seed N        сид (по умолчанию 42)
  --ticks N       сколько тиков (1000)
  --units N       сколько юнитов заселить (5000)
  --threads N     число воркеров; фиксируется явно, не по числу ядер (1)
  --every N       печатать хеш каждые N тиков, а не только итоговый
  --dump-hash     напечатать итоговый хеш состояния
  --world-hash    напечатать хеш сгенерированного мира и выйти
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
            "--seed" => a.seed = num("--seed")?,
            "--ticks" => a.ticks = num("--ticks")?,
            "--units" => a.units = num("--units")? as usize,
            "--threads" => a.threads = num("--threads")?.max(1) as usize,
            "--every" => a.every = num("--every")?,
            "--dump-hash" => a.dump_hash = true,
            "--world-hash" => a.world_hash = true,
            "--stats" => a.stats = true,
            "--help" | "-h" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ: {other}")),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
    };

    if a.world_hash {
        let p = Params::default();
        println!(
            "world seed={} {}x{}x{} hash={:016x}",
            a.seed,
            p.nx,
            p.ny,
            p.nz,
            worldgen::world_hash(&p, a.seed)
        );
        return ExitCode::SUCCESS;
    }

    let mut w = World::new(2, a.units.max(1));
    populate(&mut w, a.units, a.seed);

    let mut hash = 0u64;
    for t in 1..=a.ticks {
        hash = if a.threads > 1 {
            w.tick_par(a.threads)
        } else {
            w.tick()
        };
        if a.every > 0 && t % a.every == 0 {
            println!("tick={t} hash={hash:016x}");
        }
    }

    if a.stats {
        // Что сценарий на самом деле нагружает. Хеш, совпавший точно
        // после правки боя, обязан быть объяснён, а не принят на веру
        // (`docs/analysis/20-measurement-machine.md`, правило пятое).
        let live = w.units.live().len();
        let with_target = w
            .units
            .live()
            .iter()
            .filter(|&&i| w.units.target[i as usize] != sim_core::NO_TARGET)
            .count();
        println!(
            "живых {live}, с целью {with_target}, осмотрено на бою {}, поисков {}, тиков в потолок {}",
            w.examined_combat, w.searches, w.search_capped
        );
        for o in 0..2u8 {
            let (mut sx, mut sz, mut n) = (0i64, 0i64, 0i64);
            for &i in w.units.live() {
                if w.units.owner[i as usize] == o {
                    sx += w.units.x[i as usize] as i64;
                    sz += w.units.z[i as usize] as i64;
                    n += 1;
                }
            }
            if n > 0 {
                println!("  сторона {o}: {n} юнитов, центр ({}, {})", sx / n, sz / n);
            }
        }
    }

    if a.dump_hash || a.every == 0 {
        println!(
            "seed={} ticks={} units={} threads={} hash={:016x}",
            a.seed, a.ticks, a.units, a.threads, hash
        );
    }
    ExitCode::SUCCESS
}
