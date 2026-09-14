//! **D-9 «База»** — петля экономики замкнута и партия приходит к исходу.
//!
//! ```text
//! d9_base --seed 42 --side 192 --ticks 4000 --dump-hash
//! ```
//!
//! Копатель грызёт забой → материя попадает в бак → бак едет на склад →
//! у стороны растёт запас → из запаса завод делает машины → машины
//! воюют → у кого-то не остаётся живых.
//!
//! **Чем это отличается от `skirmish`.** Тот проверяет, что партия
//! может кончиться. Эта сцена проверяет, что **экономика работает**:
//! каждое звено обязано дать ненулевое число, иначе сцена провалена.
//! Звено, которое молча не работает, выглядит как «просто медленно»,
//! и ловится только порогом (ADR-116, ADR-187).
//!
//! **Порог:**
//!
//! - каждое звено петли дало ненулевой результат;
//! - материя сошлась **на каждом тике**, а не в конце;
//! - тик уложился в бюджет `docs/10-budgets.md`.

use std::process::ExitCode;
use std::time::Instant;

use demo::skirmish::{Outcome, Setup, Skirmish};
use demo::RunHash;
use sim_core::matter::{Sink, SINKS};

/// Бюджет тика, миллисекунд (30 Гц). Тот же, что у D-7.
const BUDGET_MS: f64 = 10.0;

struct Args {
    seed: u64,
    side: u32,
    ticks: u64,
    per_side: usize,
    diggers: usize,
    dump_hash: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            seed: 42,
            side: 192,
            ticks: 4000,
            per_side: 24,
            diggers: 6,
            dump_hash: false,
        }
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
            "--side" => a.side = num("--side")? as u32,
            "--ticks" => a.ticks = num("--ticks")?,
            "--per-side" => a.per_side = num("--per-side")? as usize,
            "--diggers" => a.diggers = num("--diggers")? as usize,
            "--dump-hash" => a.dump_hash = true,
            "--help" | "-h" => {
                println!(
                    "D-9 «База»\n\n  --seed N --side N --ticks N --per-side N --diggers N --dump-hash\n"
                );
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
}

/// Разрядка больших чисел: материя считается объёмом на плотность,
/// и числа у неё шестизначные. Это показ, а не баланс.
fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "D-9 «База»: мир {}³, по {} машин и {} краулеров, {} тиков",
        a.side, a.per_side, a.diggers, a.ticks
    );

    let setup = Setup {
        seed: a.seed,
        side: a.side,
        per_side: a.per_side,
        diggers: a.diggers,
        patience: a.ticks,
        ..Setup::default()
    };
    let t0 = Instant::now();
    let mut m = Skirmish::new(setup);
    let build = t0.elapsed().as_secs_f64() * 1e3;

    let mut hash = RunHash::new();
    let mut worst = 0i128;
    let mut hauled_at = None;
    let mut built_at = None;
    let mut made_at = None;
    let t1 = Instant::now();
    let mut ticks = 0u64;
    while ticks < a.ticks && !m.outcome().over() {
        let h = m.step();
        ticks += 1;
        let e = m.world.econ.as_ref().expect("учёт");
        // **Инвариант проверяется каждый тик, а не в конце.** Ошибка,
        // компенсировавшая сама себя, иначе прошла бы незамеченной
        // (то же правило, что в D-5).
        if !e.balanced() {
            worst = e.ledger.imbalance();
            break;
        }
        if hauled_at.is_none() && e.delivered > 0 {
            hauled_at = Some(ticks);
        }
        if built_at.is_none() && e.built > 0 {
            built_at = Some(ticks);
        }
        if made_at.is_none() && e.produced > 0 {
            made_at = Some(ticks);
        }
        hash.feed(h);
    }
    let ms = t1.elapsed().as_secs_f64() * 1e3;
    let per_tick = ms / ticks.max(1) as f64;

    // Исход **матча**, а не мира: у мира правило одно — «не осталось
    // живых», а у состязания есть ещё срок и счёт (ADR-205).
    let out = m.outcome();
    let wiped = m.world.outcome();
    let e = m.world.econ.as_ref().expect("учёт");

    println!("\n{:<34} {:>16}", "звено петли", "единиц");
    println!("{:<34} {:>16}", "выкопано из земли", grouped(e.mined));
    println!(
        "{:<34} {:>16}",
        "реклеймлено из обломков",
        grouped(e.reclaimed)
    );
    println!("{:<34} {:>16}", "довезено на склады", grouped(e.delivered));
    println!(
        "{:<34} {:>16}",
        "потрачено на стройку и сборку",
        grouped(e.spent)
    );
    println!("{:<34} {:>16}", "построек достроено", e.built);
    println!("{:<34} {:>16}", "машин выпущено", e.produced);
    println!("{:<34} {:>16}", "обломков на карте", e.wrecks.len());

    println!("\n{:<34} {:>16} {:>16}", "", "сторона 0", "сторона 1");
    println!(
        "{:<34} {:>16} {:>16}",
        "запас",
        grouped(e.stock(0)),
        grouped(e.stock(1))
    );
    println!(
        "{:<34} {:>16} {:>16}",
        "потолок",
        grouped(e.cap(0)),
        grouped(e.cap(1))
    );
    println!("{:<34} {:>16} {:>16}", "живых", m.alive[0], m.alive[1]);

    println!("\n{:<34} {:>16}", "баланс материи", "единиц");
    println!(
        "{:<34} {:>16}",
        "вошло в экономику",
        grouped(e.ledger.initial())
    );
    println!(
        "{:<34} {:>16}",
        "в живых контейнерах",
        grouped(e.ledger.in_containers())
    );
    for s in SINKS {
        let n = e.sunk(s);
        if n > 0 {
            println!("{:<34} {:>16}", format!("сток: {}", s.name()), grouped(n));
        }
    }

    println!("\n{:<34} {:>16}", "когда звено заработало", "тик");
    println!(
        "{:<34} {:>16}",
        "первая разгрузка на складе",
        show(hauled_at)
    );
    println!(
        "{:<34} {:>16}",
        "первая достроенная постройка",
        show(built_at)
    );
    println!("{:<34} {:>16}", "первая выпущенная машина", show(made_at));

    let what = match out {
        Outcome::Wiped(o) => format!("сторона {o} выбила противника подчистую"),
        Outcome::OnPoints(o) => format!("сторона {o} победила по счёту на тике {ticks}"),
        Outcome::Draw if m.alive == [0, 0] => "обе стороны погибли".to_string(),
        Outcome::Draw => "ничья: срок вышел при равном счёте".to_string(),
        Outcome::Fighting => format!("срок не вышел, бой идёт ({ticks} тиков)"),
    };
    println!("\n{what}");
    if wiped.over() {
        println!("мир согласен: {wiped:?}");
    }
    println!(
        "{:<34} {:>9.1} мс   {:<18} {:>8.3} мс/тик",
        "сборка мира", build, "тик", per_tick
    );
    println!("хеш прогона: {:016x}", hash.get());
    if a.dump_hash {
        println!(
            "seed={} side={} ticks={} hash={:016x}",
            a.seed,
            a.side,
            ticks,
            hash.get()
        );
    }

    // --- порог. Сцена без порога — ролик, а не проверка.
    let mut good = true;
    let mut fail = |why: String| {
        println!("ПОРОГ: {why}");
        good = false;
    };
    if worst != 0 || !e.balanced() {
        fail(format!(
            "материя не сошлась: расхождение {}",
            e.ledger.imbalance()
        ));
    }
    if e.mined == 0 {
        fail("копка не дала ничего: первое звено петли не работает".into());
    }
    if e.delivered == 0 {
        fail("на склады не довезено ничего: возка не работает".into());
    }
    if e.built == 0 {
        fail("ни одна постройка не достроена: стройка не работает".into());
    }
    if e.produced == 0 {
        fail("заводы не выпустили ни одной машины: производство не работает".into());
    }
    if e.sunk(Sink::RefiningLoss) == 0 {
        fail("переработка не списала ничего — выход равен единице, стока нет".into());
    }
    if per_tick > BUDGET_MS {
        fail(format!(
            "тик {per_tick:.3} мс при бюджете {BUDGET_MS:.0} мс"
        ));
    }
    if good {
        println!("порог выдержан");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn show(t: Option<u64>) -> String {
    match t {
        Some(t) => t.to_string(),
        None => "не случилось".to_string(),
    }
}
