//! Матч до исхода, headless.
//!
//! ```text
//! skirmish --seed 42 --side 192 --per-side 60 --diggers 24 --patience 6000
//! ```
//!
//! Печатает, чем кончилось и на каком тике. Это ответ на вопрос,
//! которого до сих пор не задавали: **может ли партия вообще
//! закончиться** на карте, которую всё время копают.

use std::time::Instant;

use demo::skirmish::{Outcome, Setup, Skirmish};

fn main() {
    let mut s = Setup::default();
    let mut args = std::env::args().skip(1);
    while let Some(k) = args.next() {
        let mut v = || args.next().unwrap_or_default();
        match k.as_str() {
            "--seed" => s.seed = v().parse().unwrap_or(s.seed),
            "--side" => s.side = v().parse().unwrap_or(s.side),
            "--per-side" => s.per_side = v().parse().unwrap_or(s.per_side),
            "--diggers" => s.diggers = v().parse().unwrap_or(s.diggers),
            "--patience" => s.patience = v().parse().unwrap_or(s.patience),
            "--hunt-period" => s.hunt_period = v().parse().unwrap_or(s.hunt_period),
            "--help" | "-h" => {
                println!(
                    "skirmish [--seed N] [--side N] [--per-side N] [--diggers N] [--patience N] [--hunt-period N]"
                );
                return;
            }
            _ => {}
        }
    }

    println!(
        "матч: мир {}³, по {} машин, {} копателей, терпение {} тиков, охота раз в {}",
        s.side, s.per_side, s.diggers, s.patience, s.hunt_period
    );
    let t0 = Instant::now();
    let mut m = Skirmish::new(s);
    let build = t0.elapsed().as_secs_f64() * 1e3;
    let before = m.surface();
    let t1 = Instant::now();
    let (out, hash) = m.play_out();
    let ms = t1.elapsed().as_secs_f64() * 1e3;

    let what = match out {
        Outcome::Wiped(o) => format!("сторона {o} выбила противника подчистую"),
        Outcome::OnPoints(o) => format!("сторона {o} победила по счёту машин"),
        Outcome::Draw if m.alive == [0, 0] => "обе стороны погибли".to_string(),
        Outcome::Draw => "ничья: срок вышел при равном счёте".to_string(),
        Outcome::Fighting => "не может быть".to_string(),
    };
    println!("\n{what} на тике {}", m.tick);
    println!("{:<30} {:>12} {:>12}", "", "сторона 0", "сторона 1");
    println!(
        "{:<30} {:>12} {:>12}",
        "осталось машин", m.alive[0], m.alive[1]
    );
    let w = &m.world;
    println!("\n{:<30} {:>12}", "смертей за матч", w.deaths);
    println!("{:<30} {:>12}", "выстрелов болтом", w.shots_fired);
    println!("{:<30} {:>12}", "  попаданий", w.shots_hit);
    println!("{:<30} {:>12}", "выстрелов лучом", w.beams_fired);
    println!("{:<30} {:>12}", "  попаданий", w.beams_hit);
    println!("{:<30} {:>12}", "воронок", w.blasts);
    if let Some(g) = w.ground() {
        println!("{:<30} {:>12}", "правок мира в очереди", g.world.queued());
    }
    println!(
        "\n{:<30} {:>9.1} мс   {:<30} {:>9.3} мс/тик",
        "сборка мира",
        build,
        "тик",
        ms / m.tick.max(1) as f64
    );
    // Насколько изрыта карта. Без этого «глубокие ямы» на экране
    // остаются впечатлением: у генератора ступени в один воксель,
    // и отличить их от воронок глазом нельзя (ADR-207).
    let after = m.surface();
    let (mut cut, mut deepest, mut total) = (0usize, 0u32, 0u64);
    for (a, b) in before.iter().zip(after.iter()) {
        if b < a {
            cut += 1;
            let d = a - b;
            deepest = deepest.max(d);
            total += d as u64;
        }
    }
    let (lo, hi) = (
        *before.iter().min().unwrap_or(&0),
        *before.iter().max().unwrap_or(&0),
    );
    println!(
        "\nрельеф до матча: высота {lo}..{hi} вокселей, размах {} ({:.1} м)",
        hi - lo,
        (hi - lo) as f64 * 0.5
    );
    println!(
        "{:<30} {:>12} из {} ({:.1}%)",
        "клеток стало ниже",
        cut,
        before.len(),
        100.0 * cut as f64 / before.len().max(1) as f64
    );
    println!(
        "{:<30} {:>12} вокселей ({:.1} м)",
        "  глубже всего срыто",
        deepest,
        deepest as f64 * 0.5
    );
    println!(
        "{:<30} {:>12.2} вокселя",
        "  в среднем по просевшей",
        total as f64 / cut.max(1) as f64
    );
    println!("хеш последнего тика: {hash:016x}");

    // Победа по счёту — исход, но не добитая партия. Отчёт обязан
    // различать: выбили противника или просто оказались впереди,
    // когда вышло время.
    if !matches!(out, Outcome::Wiped(_)) {
        println!(
            "\nдобить не удалось: {} машин пережили срок",
            m.alive[0].min(m.alive[1])
        );
    }
}
