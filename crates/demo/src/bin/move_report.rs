//! Рывки хода — числом, а не глазами.
//!
//! Владелец видит дёрганья на экране; часть их — симуляция, часть —
//! рендер (ADR-220). Отделить одно от другого можно только замером
//! **по тикам**: здесь эталонный сценарий гонится headless, и по
//! каждому живому юниту сравнивается шаг этого тика с шагом
//! предыдущего. Скачок скорости больше предела разгона за тик —
//! рывок симуляции, и у него есть источник: толчок соседей, упор
//! в породу, перенос высоты. Всё, чего здесь нет, — рендер.
//!
//! ```text
//! move_report [--ticks N] [--units N] [--seed N] [--warm N] [--trace K]
//! ```

use std::process::ExitCode;

use sim_core::{Reference, TICKS_PER_SECOND};

struct Args {
    ticks: u64,
    units: usize,
    seed: u64,
    /// Тиков прогрева: строй трогается с места, и первые секунды
    /// нагружают разгон, а не ход.
    warm: u64,
    /// Сколько тиков печатать след одного юнита.
    trace: u64,
    /// Какого юнита вести; без ключа — первого живого.
    trace_id: Option<usize>,
    /// Сколько первых крупных событий печатать с подробностями.
    events: u64,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        ticks: 1000,
        units: 5000,
        seed: 42,
        warm: 100,
        trace: 0,
        trace_id: None,
        events: 0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut num = |name: &str| -> Result<u64, String> {
            it.next()
                .ok_or_else(|| format!("{name} без значения"))?
                .parse::<u64>()
                .map_err(|e| format!("{name}: {e}"))
        };
        match k.as_str() {
            "--ticks" => a.ticks = num("--ticks")?,
            "--units" => a.units = num("--units")? as usize,
            "--seed" => a.seed = num("--seed")?,
            "--warm" => a.warm = num("--warm")?,
            "--trace" => a.trace = num("--trace")?,
            "--trace-id" => a.trace_id = Some(num("--trace-id")? as usize),
            "--events" => a.events = num("--events")?,
            "--help" => {
                println!(
                    "move_report [--ticks N] [--units N] [--seed N] [--warm N] [--trace K] \
                     [--trace-id I] [--events N]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ {other}")),
        }
    }
    Ok(a)
}

fn data(name: &str) -> std::path::PathBuf {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data")
        .join(name);
    if here.exists() {
        here
    } else {
        std::path::PathBuf::from("data").join(name)
    }
}

/// Положение юнита с дробной частью, курс, скорость, высота.
#[derive(Clone, Copy, Default)]
struct Pose {
    x: f64,
    z: f64,
    head: i32,
    y: i32,
    alive: bool,
}

/// Счётчики за прогон. Пороги — в мировых единицах в секунду
/// **за тик**: при вокселе 10 см единица это 10 см/с.
#[derive(Default)]
struct Tally {
    unit_ticks: u64,
    /// Скачок скорости за тик, по порогам: > 1, 2, 5, 10, 20 ед/с.
    dv: [u64; 5],
    dv_max: f64,
    /// Скачок скорости, когда юнит целиком упёрся: скорость есть,
    /// шага нет.
    stalls: u64,
    /// Угловой рывок: смена скорости доворота за тик больше
    /// половины предела (в двоичных единицах угла).
    dhead: [u64; 3],
    dhead_max: i32,
    /// Перенос высоты за тик: ≥ 1, ≥ 2, ≥ 5 вокселей.
    dy: [u64; 3],
    dy_max: i32,
}

const DV_STEPS: [f64; 5] = [1.0, 2.0, 5.0, 10.0, 20.0];

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let tuning = match modding::load_tuning(&data("sim.ron")) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("data/sim.ron: {e}");
            return ExitCode::from(2);
        }
    };
    let weapons = match modding::weapons::load(&data("weapons.ron")) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("data/weapons.ron: {e}");
            return ExitCode::from(2);
        }
    };
    let mut r = Reference::new(a.seed, a.units, tuning, weapons);
    let tps = TICKS_PER_SECOND as f64;

    let mut prev: Vec<Pose> = Vec::new();
    let mut prev_step: Vec<(f64, f64)> = Vec::new();
    let mut prev_dhead: Vec<i32> = Vec::new();
    let mut t = Tally::default();
    let mut blocked0 = 0u64;
    let trace_id = a
        .trace_id
        .unwrap_or_else(|| r.world.units.live().first().copied().unwrap_or(0) as usize);
    let mut shown = 0u64;

    let snapshot = |r: &Reference, out: &mut Vec<Pose>| {
        let u = &r.world.units;
        out.clear();
        out.resize(u.len(), Pose::default());
        for &i in u.live() {
            let i = i as usize;
            out[i] = Pose {
                x: u.x[i] as f64 + u.fx[i].to_f64_lossy(),
                z: u.z[i] as f64 + u.fz[i].to_f64_lossy(),
                head: u.head[i],
                y: u.y[i],
                // Убитый слот остаётся в списке живых до пересборки:
                // без этой проверки подкрепление, вставшее в его слот,
                // читалось бы как прыжок через полкарты.
                alive: u.alive[i] && u.voxels[i] > 0,
            };
        }
    };

    for tick in 0..a.warm + a.ticks {
        snapshot(&r, &mut prev);
        if tick == a.warm {
            blocked0 = r.world.blocked_moves;
        }
        r.tick();
        let n = r.world.units.len();
        prev_step.resize(n, (0.0, 0.0));
        prev_dhead.resize(n, 0);
        let u = &r.world.units;
        for &i in u.live() {
            let i = i as usize;
            if i >= prev.len() || !prev[i].alive {
                prev_step[i] = (0.0, 0.0);
                prev_dhead[i] = 0;
                continue;
            }
            let p = prev[i];
            let x = u.x[i] as f64 + u.fx[i].to_f64_lossy();
            let z = u.z[i] as f64 + u.fz[i].to_f64_lossy();
            let step = (x - p.x, z - p.z);
            let dhead = fx::angle::diff(u.head[i], p.head);
            let dy = u.y[i] - p.y;
            if tick >= a.warm {
                t.unit_ticks += 1;
                let (ps, pd) = (prev_step[i], prev_dhead[i]);
                let dv = ((step.0 - ps.0).powi(2) + (step.1 - ps.1).powi(2)).sqrt() * tps;
                t.dv_max = t.dv_max.max(dv);
                for (k, s) in DV_STEPS.iter().enumerate() {
                    if dv > *s {
                        t.dv[k] += 1;
                    }
                }
                if u.vel[i].raw() > 0 && step.0 == 0.0 && step.1 == 0.0 {
                    t.stalls += 1;
                }
                let dd = (dhead - pd).abs();
                t.dhead_max = t.dhead_max.max(dd);
                // Пороги: 3°, 6°, 12° за тик в двоичных единицах.
                for (k, deg) in [3, 6, 12].iter().enumerate() {
                    if dd > deg * fx::angle::FULL / 360 {
                        t.dhead[k] += 1;
                    }
                }
                let ady = dy.abs();
                t.dy_max = t.dy_max.max(ady);
                if shown < a.events && (dv > 100.0 || dd > fx::angle::FULL / 15 || ady >= 5) {
                    shown += 1;
                    println!(
                        "событие t={tick} i={i}: ({:.2},{:.2})->({x:.2},{z:.2}) y {}->{} \
                         head {}->{} vel={:.2} dv={dv:.1} dhead={dd} riding={} fall={}",
                        p.x,
                        p.z,
                        p.y,
                        u.y[i],
                        p.head,
                        u.head[i],
                        u.vel[i].to_f64_lossy(),
                        u.riding[i],
                        u.fall_from[i] != sim_core::NOT_FALLING
                    );
                }
                for (k, v) in [1, 2, 5].iter().enumerate() {
                    if ady >= *v {
                        t.dy[k] += 1;
                    }
                }
            }
            if a.trace > 0 && i == trace_id && tick < a.trace {
                println!(
                    "t={tick:4} x={x:9.3} z={z:9.3} y={:4} head={:6} vel={:7.3} dv={:7.3} dhead={dhead:5}",
                    u.y[i],
                    u.head[i],
                    u.vel[i].to_f64_lossy(),
                    ((step.0 - prev_step[i].0).powi(2) + (step.1 - prev_step[i].1).powi(2)).sqrt() * tps,
                );
            }
            prev_step[i] = step;
            prev_dhead[i] = dhead;
        }
    }
    let blocked = r.world.blocked_moves - blocked0;
    let per = |v: u64| -> f64 { v as f64 * 1000.0 / t.unit_ticks.max(1) as f64 };
    println!(
        "рывки хода: сид {}, юнитов {}, тиков {} после {} прогрева, юнито-тиков {}",
        a.seed, a.units, a.ticks, a.warm, t.unit_ticks
    );
    println!("{:<44} {:>10} {:>12}", "событие", "всего", "на тыс. ю-т");
    for (k, s) in DV_STEPS.iter().enumerate() {
        println!(
            "{:<44} {:>10} {:>12.2}",
            format!("скачок скорости за тик > {s:.0} ед/с"),
            t.dv[k],
            per(t.dv[k])
        );
    }
    println!(
        "{:<44} {:>10.2}",
        "наибольший скачок скорости, ед/с", t.dv_max
    );
    println!(
        "{:<44} {:>10} {:>12.2}",
        "скорость есть, шага нет (упор)",
        t.stalls,
        per(t.stalls)
    );
    println!(
        "{:<44} {:>10} {:>12.2}",
        "шагов упёрлось в породу",
        blocked,
        blocked as f64 / a.ticks as f64
    );
    for (k, deg) in [3, 6, 12].iter().enumerate() {
        println!(
            "{:<44} {:>10} {:>12.2}",
            format!("смена доворота за тик > {deg}°"),
            t.dhead[k],
            per(t.dhead[k])
        );
    }
    println!(
        "{:<44} {:>10.1}",
        "наибольшая смена доворота, °",
        t.dhead_max as f64 * 360.0 / fx::angle::FULL as f64
    );
    for (k, v) in [1, 2, 5].iter().enumerate() {
        println!(
            "{:<44} {:>10} {:>12.2}",
            format!("перенос высоты за тик >= {v} вокс."),
            t.dy[k],
            per(t.dy[k])
        );
    }
    println!(
        "{:<44} {:>10}",
        "наибольший перенос высоты, вокс.", t.dy_max
    );
    ExitCode::SUCCESS
}
