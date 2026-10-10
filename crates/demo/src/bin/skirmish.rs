//! Матч на полигоне до исхода, headless (ADR-205, ADR-663).
//!
//! ```text
//! skirmish [--preset breakthrough|pincer|spider|turrets|canister|world]
//!          [--patience N] [--no-ai] [--whole-map] [--workers N]
//! ```
//!
//! Печатает, чем кончилось и на каком тике. Это ответ на вопрос, может
//! ли партия с конечными силами вообще закончиться, и чем: боем или
//! сроком.

use std::process::ExitCode;
use std::time::Instant;

use demo::range_match::{Match, Outcome, Preset, Setup};

fn parse() -> Result<Setup, String> {
    let mut s = Setup::default();
    let mut args = std::env::args().skip(1);
    while let Some(k) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} без значения"));
        match k.as_str() {
            "--preset" => {
                let v = value("--preset")?;
                s.preset =
                    Preset::parse(&v).ok_or_else(|| format!("--preset: нет сценария {v}"))?;
            }
            "--patience" => {
                s.patience = value("--patience")?
                    .parse()
                    .map_err(|e| format!("--patience: {e}"))?;
            }
            "--workers" => {
                s.workers = value("--workers")?
                    .parse()
                    .map_err(|e| format!("--workers: {e}"))?;
            }
            "--no-ai" => s.ai_vs_ai = false,
            "--whole-map" => s.whole_map = true,
            "--help" | "-h" => {
                let names: Vec<_> = Preset::ALL.iter().map(|p| p.name()).collect();
                println!(
                    "skirmish [--preset {}] [--patience N] [--no-ai] [--whole-map] [--workers N]",
                    names.join("|")
                );
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ {other}")),
        }
    }
    Ok(s)
}

fn main() -> ExitCode {
    let s = match parse() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    println!(
        "матч: {}, срок {} тиков, ИИ {}, {}",
        s.preset.name(),
        s.patience,
        if s.ai_vs_ai {
            "у обеих сторон"
        } else {
            "только у противника"
        },
        if s.whole_map || s.preset == Preset::World {
            "вся карта"
        } else {
            "угол карты до участка"
        }
    );
    let t0 = Instant::now();
    let mut m = match Match::new(s) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let build = t0.elapsed().as_secs_f64() * 1e3;
    let start = m.tick();
    let t1 = Instant::now();
    let (out, hash) = m.play_out();
    let ms = t1.elapsed().as_secs_f64() * 1e3;
    let alive = m.alive();

    let what = match out {
        Outcome::Won(o) => format!("сторона {o} выиграла бой"),
        Outcome::OnPoints(o) => format!("сторона {o} победила по счёту живых"),
        Outcome::Draw if alive == [0, 0] => "обе стороны погибли".to_string(),
        Outcome::Draw => "ничья".to_string(),
        Outcome::Fighting => "не может быть".to_string(),
    };
    println!("\n{what} на тике {}", m.tick());
    println!("{:<30} {:>12} {:>12}", "", "сторона 0", "сторона 1");
    println!("{:<30} {:>12} {:>12}", "осталось живых", alive[0], alive[1]);
    let fired = m.fired();
    println!("{:<30} {:>12} {:>12}", "выстрелов", fired[0], fired[1]);
    println!("\n{:<30} {:>12}", "погибло за матч", m.dead());
    println!("{:<30} {:>12}", "обрушено клеток", m.sim.collapsed_cells);
    println!(
        "{:<30} {:>12}",
        "обломков в конце",
        m.sim.matter.debris.fragments.len()
    );
    println!(
        "{:<30} {:>12}",
        "баланс материи",
        if m.sim.matter.economy.ledger.balanced() {
            "сходится"
        } else {
            "НЕ СХОДИТСЯ"
        }
    );
    println!(
        "\n{:<30} {:>9.1} мс   {:<30} {:>9.3} мс/тик",
        "сборка полигона",
        build,
        "тик",
        ms / (m.tick() - start).max(1) as f64
    );
    println!("хеш состояния в конце: {hash:016x}");
    ExitCode::SUCCESS
}
