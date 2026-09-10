//! **D-5 «Отнять груз»** — материя сохраняется и её можно отобрать
//! (`docs/14-demo-scenes.md`).
//!
//! ```text
//! d5_haul --seed 42 --ticks 600 --dump-hash
//! ```
//!
//! Краулер копает породу в бак, везёт на склад. Часть краулеров
//! подбивают: бак становится обломком с потерей доли, и чужой краулер
//! его реклеймит — **тем же кодом, что копка**.
//!
//! **Порог:** сумма материи в мире неизменна с точностью до объявленных
//! стоков, и проверяется это **на каждом тике**, а не в конце.

use std::process::ExitCode;

use demo::RunHash;
use fx::rng::Rng;
use sim_core::matter::{resource_of, ContainerId, Kind, Ledger, Sink, SINKS};

/// Доля содержимого, теряемая при разрушении бака (ADR-021).
///
/// Канон (`docs/07-economy.md`) говорит: доля берётся **по данным
/// материала**. Материала у контейнера пока нет — у него есть только
/// род (земля, бак, обломок, склад, тело), — поэтому число стоит здесь
/// и переедет в данные вместе с материалом контейнера, а не раньше.
/// Числа тика уже переехали (ADR-140); это — не тик.
const BREACH_LOSS_NUM: u64 = 1;
const BREACH_LOSS_DEN: u64 = 4;
/// Выход переработки на складе: семь десятых. Тот же случай.
const REFINE_NUM: u64 = 7;
const REFINE_DEN: u64 = 10;

struct Hauler {
    tank: ContainerId,
    /// Сколько увезено на склад за жизнь — для отчёта.
    delivered: u64,
    /// `true` — едет к складу, `false` — копает.
    homebound: bool,
    alive: bool,
}

struct Args {
    seed: u64,
    ticks: u64,
    haulers: usize,
    dump_hash: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args { seed: 42, ticks: 600, haulers: 60, dump_hash: false }
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
            "--ticks" => a.ticks = num("--ticks")?,
            "--haulers" => a.haulers = num("--haulers")? as usize,
            "--dump-hash" => a.dump_hash = true,
            "--help" | "-h" => {
                println!("D-5 «Отнять груз»\n\n  --seed N --ticks N --haulers N --dump-hash\n");
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut l = Ledger::new();
    // Земля — тоже контейнер, и это не метафора: копка это перевод
    // из неё, а не создание (`docs/07-economy.md`).
    let ground = l.create(Kind::Ground, 5_000_000, u64::MAX);
    // Потолок склада низкий намеренно: при высоком переработка
    // сливает быстрее, чем склад набирает, и сток «излишек над
    // потолком» не срабатывает ни разу — сцена проверяет инвариант
    // на пустом.
    let depot = l.create(Kind::Depot, 0, 12_000);
    let refined = l.create(Kind::Depot, 0, u64::MAX);

    let mut haulers: Vec<Hauler> = (0..a.haulers)
        .map(|_| Hauler {
            tank: l.create(Kind::Hauler, 0, 900),
            delivered: 0,
            homebound: false,
            alive: true,
        })
        .collect();
    let mut debris: Vec<ContainerId> = Vec::new();

    let mut r = Rng::new(a.seed, 0xD5);
    let mut hash = RunHash::new();
    let (mut dug, mut killed, mut reclaimed, mut cap_drops) = (0u64, 0usize, 0u64, 0usize);
    let mut worst_imbalance = 0i128;

    for tick in 1..=a.ticks {
        // --- Копка: перевод из земли в бак. Ресурс = объём × плотность
        // (`CLAUDE.md`); плотность растёт с глубиной (ADR-100).
        for h in haulers.iter_mut().filter(|h| h.alive && !h.homebound) {
            let voxels = 3 + r.below(4) as u64;
            let density = 2 + r.below(6) as u64;
            let want = resource_of(voxels, density as u32);
            let got = l.transfer(ground, h.tank, want);
            dug += got;
            if l.room(h.tank) == 0 {
                h.homebound = true;
            }
        }

        // --- Доставка: бак на склад.
        for h in haulers.iter_mut().filter(|h| h.alive && h.homebound) {
            let n = l.transfer(h.tank, depot, l.held(h.tank));
            h.delivered += n;
            if l.held(h.tank) == 0 {
                h.homebound = false;
            }
        }

        // --- Переработка на складе: выход меньше единицы — сток,
        // доступный с первого дня.
        if l.held(depot) > 1000 {
            l.refine(depot, refined, 800, REFINE_NUM, REFINE_DEN);
        }

        // --- Подбили краулера: бак становится обломком с потерей доли.
        if r.below(12) == 0 {
            let live: Vec<usize> = (0..haulers.len()).filter(|&i| haulers[i].alive).collect();
            if !live.is_empty() {
                let i = live[r.below(live.len() as u32) as usize];
                let (d, _lost, rest) = l.breach(haulers[i].tank, BREACH_LOSS_NUM, BREACH_LOSS_DEN);
                haulers[i].alive = false;
                killed += 1;
                if rest > 0 {
                    debris.push(d);
                }
            }
        }

        // --- Реклейм: чужой краулер забирает обломок. Тот же перевод,
        // что копка, — никакого отдельного механизма.
        if !debris.is_empty() {
            let live: Vec<usize> = (0..haulers.len())
                .filter(|&i| haulers[i].alive && l.room(haulers[i].tank) > 0)
                .collect();
            if !live.is_empty() {
                let i = live[r.below(live.len() as u32) as usize];
                let d = debris[0];
                let got = l.transfer(d, haulers[i].tank, 300);
                reclaimed += got;
                if l.held(d) == 0 {
                    debris.remove(0);
                }
            }
        }

        // --- Склад разрушен: потолок падает, излишек уходит в сток.
        if tick % 150 == 0 {
            let cap = l.get(depot).cap / 2;
            if l.lower_cap(depot, cap) > 0 {
                cap_drops += 1;
            }
        }

        // **Инвариант проверяется каждый тик, а не в конце.** Иначе
        // ошибка, компенсировавшая сама себя, прошла бы незамеченной.
        if !l.balanced() {
            worst_imbalance = l.imbalance();
            break;
        }
        // **Хешировать `in_containers` и `sunk_total` бессмысленно:**
        // их сумма равна `initial` по самому инварианту, а хеш
        // складывает произведения — то есть каждый тик добавлял бы одну
        // и ту же константу. Первая редакция так и делала, и хеш
        // не менялся ни от сида, ни от параметров: он «проверял» ровно
        // то, что и так проверяется отдельно.
        //
        // Хешируется распределение — оно и различает прогоны.
        hash.feed(tick);
        hash.feed(l.held(ground));
        hash.feed(l.held(depot));
        hash.feed(l.held(refined));
        for s in SINKS {
            hash.feed(l.sunk(s));
        }
        hash.feed(l.live_containers() as u64);
        hash.feed(dug ^ reclaimed);
    }

    println!("D-5 «Отнять груз» — материя сохраняется и её можно отобрать");
    println!("{} краулеров, {} тиков\n", a.haulers, a.ticks);

    println!("{:<32} {:>16}", "движение материи", "единиц");
    println!("{:<32} {:>16}", "выкопано из земли", dug);
    println!("{:<32} {:>16}", "реклеймлено из обломков", reclaimed);
    println!("{:<32} {:>16}", "краулеров подбито", killed);
    println!("{:<32} {:>16}", "обломков осталось", debris.len());
    println!("{:<32} {:>16}", "падений потолка склада", cap_drops);

    println!("\n{:<32} {:>16}", "баланс", "единиц");
    println!("{:<32} {:>16}", "было изначально", l.initial());
    println!("{:<32} {:>16}", "в живых контейнерах", l.in_containers());
    for s in SINKS {
        let n = l.sunk(s);
        if n > 0 {
            println!("{:<32} {:>16}", format!("сток: {}", s.name()), n);
        }
    }
    println!("{:<32} {:>16}", "в стоках всего", l.sunk_total());
    println!(
        "{:<32} {:>16}",
        "контейнеров живых",
        l.live_containers()
    );

    let ok = l.balanced() && worst_imbalance == 0;
    println!("\nхеш прогона: {:016x}", hash.get());
    if a.dump_hash {
        println!("seed={} ticks={} hash={:016x}", a.seed, a.ticks, hash.get());
    }
    println!(
        "сохранение материи: {} — {}",
        if ok { "сходится" } else { "РАСХОЖДЕНИЕ" },
        if ok {
            format!("{} = {} + {}", l.initial(), l.in_containers(), l.sunk_total())
        } else {
            format!("расхождение {}", l.imbalance())
        }
    );

    // Сцена обязана что-то показать, иначе она проходит вхолостую.
    let mut good = ok;
    if dug == 0 || killed == 0 || reclaimed == 0 {
        println!("сцена прошла вхолостую: копка {dug}, подбито {killed}, реклейм {reclaimed}");
        good = false;
    }
    if l.sunk(Sink::ContainerBreach) == 0 || l.sunk(Sink::RefiningLoss) == 0 {
        println!("ни один сток не сработал — инвариант проверен на пустом");
        good = false;
    }

    if good {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
