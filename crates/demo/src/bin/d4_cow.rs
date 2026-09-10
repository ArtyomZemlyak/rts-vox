//! **D-4 «Копирование при записи»** — общий шаблонный том
//! (`docs/14-demo-scenes.md`, ADR-018).
//!
//! ```text
//! d4_cow --units 1000 --dump-hash
//! ```
//!
//! Тысяча одинаковых танков занимает память одного тома плюс тысячу
//! слотов, пока в них не попали. Первое попадание стоит **полной копии**,
//! второе — записи одного байта.
//!
//! **Порог:** память на тысячу целых и на тысячу повреждённых измеряется
//! и записывается; разрыв между ними — цена механики.

use std::process::ExitCode;

/// Бюджет тика, мс (`docs/10-budgets.md`).
const BUDGET_MS: f64 = 10.0;
/// Сколько бюджета тика разрешено занять залпу по свежему флоту.
const VOLLEY_LIMIT: f64 = 40.0;
use std::time::Instant;

use demo::RunHash;
use fx::rng::Rng;
use voxel::fleet::Fleet;
use voxel::topo::unit::CoreBufs;
use voxel::unit::tank;

struct Args {
    seed: u64,
    units: usize,
    dump_hash: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args { seed: 42, units: 1000, dump_hash: false }
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
            "--units" => a.units = num("--units")? as usize,
            "--dump-hash" => a.dump_hash = true,
            "--help" | "-h" => {
                println!("D-4 «Копирование при записи»\n\n  --seed N --units N --dump-hash\n");
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
}

fn mib(b: usize) -> f64 {
    b as f64 / 1024.0 / 1024.0
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut hash = RunHash::new();
    let one = tank().bytes();

    println!("D-4 «Копирование при записи» — общий шаблонный том");
    println!("танк: том {} КиБ, {} твёрдых вокселей\n", one / 1024, tank().solid_count());

    // --- 1. Память: целые против повреждённых.
    let mut f = Fleet::new(vec![tank()]);
    let ids: Vec<u32> = (0..a.units).map(|_| f.spawn(0)).collect();
    let pristine = f.bytes();
    let no_cow = f.bytes_without_cow();

    println!("{:<34} {:>12} {:>12}", "состояние флота", "МиБ", "к целым");
    println!("{:<34} {:>12.3} {:>12}", "все целы", mib(pristine), "1.0x");

    // Тронуть каждого один раз — худший случай для COW.
    let mut r = Rng::new(a.seed, 0xC0);
    let t0 = Instant::now();
    for &id in &ids {
        let x = r.below(24) as usize;
        let y = r.below(12) as usize;
        let z = r.below(12) as usize;
        f.body_mut(id).remove(x, y, z);
    }
    let first_us = t0.elapsed().as_secs_f64() * 1e6 / a.units as f64;
    let damaged = f.bytes();
    println!(
        "{:<34} {:>12.3} {:>11.1}x",
        "каждый получил по попаданию",
        mib(damaged),
        damaged as f64 / pristine as f64
    );
    println!(
        "{:<34} {:>12.3} {:>11.1}x",
        "если бы COW не было",
        mib(no_cow),
        no_cow as f64 / pristine as f64
    );

    // --- 2. Цена попадания: первое против второго.
    let t0 = Instant::now();
    for &id in &ids {
        let x = r.below(24) as usize;
        let y = r.below(12) as usize;
        let z = r.below(12) as usize;
        f.body_mut(id).remove(x, y, z);
    }
    let second_us = t0.elapsed().as_secs_f64() * 1e6 / a.units as f64;

    println!("\n{:<34} {:>12}", "цена попадания, мкс", "");
    println!("{:<34} {:>12.3}", "первое (копия тома)", first_us);
    println!("{:<34} {:>12.3}", "второе (запись байта)", second_us);
    println!("{:<34} {:>11.1}x", "первое дороже второго в", first_us / second_us.max(1e-9));

    // --- 3. Сколько первых попаданий помещается в тик.
    println!(
        "\n{:<34} {:>12.0}",
        "первых попаданий в тик 10 мс",
        10_000.0 / first_us
    );
    println!(
        "{:<34} {:>12.0}",
        "вторых попаданий в тик 10 мс",
        10_000.0 / second_us.max(1e-9)
    );

    // --- 4. Состояние читается одинаково у целого и повреждённого.
    let mut b = CoreBufs::new();
    let mut fresh = Fleet::new(vec![tank()]);
    let clean = fresh.spawn(0);
    let s_clean = fresh.status(clean, &mut b);
    let s_dirty = f.status(ids[0], &mut b);
    hash.feed(s_clean.attached as u64);
    hash.feed(s_dirty.attached as u64);
    hash.feed(f.stats().copies);
    hash.feed(damaged as u64);

    println!("\n{:<34} {:>12} {:>12}", "состояние", "целый", "повреждённый");
    println!(
        "{:<34} {:>12} {:>12}",
        "прикреплено к ядру", s_clean.attached, s_dirty.attached
    );
    println!("{:<34} {:>12} {:>12}", "жив", s_clean.alive, s_dirty.alive);

    let st = f.stats();
    println!(
        "\nживых {}, повреждённых {}, копий тома {}",
        st.live, st.damaged, st.copies
    );

    println!("\nхеш прогона: {:016x}", hash.get());
    if a.dump_hash {
        println!("seed={} units={} hash={:016x}", a.seed, a.units, hash.get());
    }

    let mut ok = true;
    // Порог 1: целые обязаны стоить на два порядка меньше, чем тома.
    let gain = no_cow as f64 / pristine as f64;
    println!(
        "выигрыш на целом флоте: {gain:.0}x — {}",
        if gain > 100.0 { "держится" } else { "СЛИШКОМ МАЛ" }
    );
    if gain <= 100.0 {
        ok = false;
    }
    // Порог 2: копий ровно столько, сколько юнитов — ни больше.
    if st.copies != a.units as u64 {
        println!("копий {} при {} юнитах — COW копирует лишнее", st.copies, a.units);
        ok = false;
    }
    // Порог 3: накладные расходы COW в худшем случае — шаблон плюс
    // слоты, то есть ровно вес целого флота. Больше быть не должно.
    if damaged > no_cow + pristine {
        println!(
            "повреждённый флот весит {} при пределе {}",
            damaged,
            no_cow + pristine
        );
        ok = false;
    }
    // Величина со знаком: копия при записи не копирует метки вокселей
    // (ADR-120), поэтому даже полностью побитый флот весит **меньше**,
    // чем отдельный том на юнита. Минус здесь — не ошибка мерки.
    println!(
        "COW против отдельного тома на юнита: {:+.2}% — {}",
        100.0 * (damaged as f64 - no_cow as f64) / no_cow as f64,
        if damaged <= no_cow + pristine { "в пределах" } else { "ПРЕВЫШЕНЫ" }
    );

    // Риск, который эти числа делают видимым: залп по свежему флоту.
    // Порог здесь есть, и он жёсткий: величина уже стоила одного
    // открытого вопроса (ADR-120), и регрессия обязана падать, а не
    // печататься.
    let volley_ms = 5000.0 * first_us / 1000.0;
    let volley_pct = volley_ms / BUDGET_MS * 100.0;
    println!(
        "
залп по 5 000 целым юнитам: {volley_ms:.2} мс — {volley_pct:.0}% бюджета тика \
(порог {VOLLEY_LIMIT:.0}%)"
    );
    if volley_pct > VOLLEY_LIMIT {
        println!("залп вышел за порог");
        ok = false;
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
