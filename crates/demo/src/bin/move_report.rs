//! Рывки хода — числом, а не глазами.
//!
//! Владелец видит дёрганья на экране; часть их — симуляция, часть —
//! рендер (ADR-220). Отделить одно от другого можно только замером
//! **по тикам**: здесь бой полигона гонится headless, и по каждой живой
//! машине сравнивается шаг этого тика с шагом предыдущего. Скачок
//! скорости больше предела разгона за тик — рывок симуляции, и у него
//! есть источник: толчок соседей или взрыва, упор, перенос высоты. Всё,
//! чего здесь нет, — рендер.
//!
//! По умолчанию — эталонный бой всей карты (ADR-662); `--preset`
//! берёт любой бой матча (`demo::range_match`).
//!
//! ```text
//! move_report [--preset NAME] [--ticks N] [--warm N] [--trace K]
//!             [--trace-id I] [--events N] [--workers N]
//! ```

use std::process::ExitCode;

use demo::range_match::{Match, Preset, Setup};
use sim_core::TICKS_PER_SECOND;

struct Args {
    preset: Preset,
    ticks: u64,
    /// Тиков прогрева: строй трогается с места, и первые секунды
    /// нагружают разгон, а не ход.
    warm: u64,
    /// Сколько тиков печатать след одного юнита.
    trace: u64,
    /// Какого юнита вести; без ключа — первого живого.
    trace_id: Option<usize>,
    /// Сколько первых крупных событий печатать с подробностями.
    events: u64,
    workers: usize,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        preset: Preset::World,
        ticks: 600,
        warm: 60,
        trace: 0,
        trace_id: None,
        events: 0,
        workers: 4,
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
            "--warm" => a.warm = num("--warm")?,
            "--trace" => a.trace = num("--trace")?,
            "--trace-id" => a.trace_id = Some(num("--trace-id")? as usize),
            "--events" => a.events = num("--events")?,
            "--workers" => a.workers = num("--workers")? as usize,
            "--preset" => {
                let v = it.next().ok_or("--preset без значения")?;
                a.preset = Preset::parse(&v).ok_or_else(|| format!("--preset: нет боя {v}"))?;
            }
            "--help" => {
                println!(
                    "move_report [--preset NAME] [--ticks N] [--warm N] [--trace K] \
                     [--trace-id I] [--events N] [--workers N]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ {other}")),
        }
    }
    Ok(a)
}

/// Положение машины с дробной частью, курс, высота.
#[derive(Clone, Copy, Default)]
struct Pose {
    x: f64,
    y: f64,
    z: f64,
    head: i32,
    alive: bool,
}

/// Счётчики за прогон. Пороги — в мировых единицах в секунду
/// **за тик**: при клетке 10 см единица это 10 см/с.
#[derive(Default)]
struct Tally {
    unit_ticks: u64,
    /// Скачок скорости за тик, по порогам: > 1, 2, 5, 10, 20 ед/с.
    dv: [u64; 5],
    dv_max: f64,
    /// Скорость есть, шага нет: машина целиком упёрлась.
    stalls: u64,
    /// Тиков, в которых ход машины упёрся (`Motion::blocked`).
    blocked: u64,
    /// Угловой рывок: смена скорости доворота за тик больше порога
    /// (в двоичных единицах угла).
    dhead: [u64; 3],
    dhead_max: i32,
    /// Перенос высоты за тик: ≥ 1, ≥ 2, ≥ 5 мировых единиц.
    dy: [u64; 3],
    dy_max: f64,
}

const DV_STEPS: [f64; 5] = [1.0, 2.0, 5.0, 10.0, 20.0];

fn snapshot(m: &Match, out: &mut Vec<Pose>) {
    out.clear();
    out.extend(m.sim.units.iter().map(|u| {
        let p = u.motion.pos.map(|v| v.to_f64_lossy());
        Pose {
            x: p[0],
            y: p[1],
            z: p[2],
            head: u.motion.drive.head,
            alive: u.status.alive && !u.kind.emplacement(),
        }
    }));
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let mut m = match Match::new(Setup {
        preset: a.preset,
        // Замер идёт фиксированное число тиков, срок матча ему не нужен.
        patience: u64::MAX,
        workers: a.workers,
        ..Setup::default()
    }) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let tps = TICKS_PER_SECOND as f64;

    let mut prev: Vec<Pose> = Vec::new();
    let mut now: Vec<Pose> = Vec::new();
    let n = m.sim.units.len();
    let mut prev_step = vec![(0.0f64, 0.0f64); n];
    let mut prev_dhead = vec![0i32; n];
    let mut t = Tally::default();
    snapshot(&m, &mut now);
    let trace_id = a
        .trace_id
        .unwrap_or_else(|| now.iter().position(|p| p.alive).unwrap_or(0));
    let mut shown = 0u64;
    let mut ran = 0u64;

    for tick in 0..a.warm + a.ticks {
        if m.outcome().over() {
            break;
        }
        std::mem::swap(&mut prev, &mut now);
        m.step();
        ran += 1;
        snapshot(&m, &mut now);
        for i in 0..n {
            let (p, c) = (prev[i], now[i]);
            if !p.alive || !c.alive {
                prev_step[i] = (0.0, 0.0);
                prev_dhead[i] = 0;
                continue;
            }
            let u = &m.sim.units[i];
            let step = (c.x - p.x, c.z - p.z);
            let dhead = fx::angle::diff(c.head, p.head);
            let dy = c.y - p.y;
            let (ps, pd) = (prev_step[i], prev_dhead[i]);
            let dv = ((step.0 - ps.0).powi(2) + (step.1 - ps.1).powi(2)).sqrt() * tps;
            let dd = (dhead - pd).abs();
            if tick >= a.warm {
                t.unit_ticks += 1;
                t.dv_max = t.dv_max.max(dv);
                for (k, s) in DV_STEPS.iter().enumerate() {
                    if dv > *s {
                        t.dv[k] += 1;
                    }
                }
                if u.motion.drive.vel.raw() > 0 && step.0 == 0.0 && step.1 == 0.0 {
                    t.stalls += 1;
                }
                if u.motion.blocked {
                    t.blocked += 1;
                }
                t.dhead_max = t.dhead_max.max(dd);
                // Пороги: 3°, 6°, 12° за тик в двоичных единицах.
                for (k, deg) in [3, 6, 12].iter().enumerate() {
                    if dd > deg * fx::angle::FULL / 360 {
                        t.dhead[k] += 1;
                    }
                }
                let ady = dy.abs();
                t.dy_max = t.dy_max.max(ady);
                for (k, v) in [1.0, 2.0, 5.0].iter().enumerate() {
                    if ady >= *v {
                        t.dy[k] += 1;
                    }
                }
                if shown < a.events && (dv > 100.0 || dd > fx::angle::FULL / 15 || ady >= 5.0) {
                    shown += 1;
                    let shove = u.motion.shove;
                    println!(
                        "событие t={tick} i={i} {:?}: ({:.2},{:.2})->({:.2},{:.2}) y {:.2}->{:.2} \
                         head {}->{} vel={:.2} dv={dv:.1} dhead={dd} blocked={} shove=({:.2},{:.2})",
                        u.kind,
                        p.x,
                        p.z,
                        c.x,
                        c.z,
                        p.y,
                        c.y,
                        p.head,
                        c.head,
                        u.motion.drive.vel.to_f64_lossy(),
                        u.motion.blocked,
                        shove.x.to_f64_lossy(),
                        shove.z.to_f64_lossy(),
                    );
                }
            }
            if a.trace > 0 && i == trace_id && tick < a.trace {
                println!(
                    "t={tick:4} x={:9.3} z={:9.3} y={:8.3} head={:6} vel={:7.3} dv={dv:7.3} dhead={dhead:5}",
                    c.x,
                    c.z,
                    c.y,
                    c.head,
                    u.motion.drive.vel.to_f64_lossy(),
                );
            }
            prev_step[i] = step;
            prev_dhead[i] = dhead;
        }
    }
    let per = |v: u64| -> f64 { v as f64 * 1000.0 / t.unit_ticks.max(1) as f64 };
    println!(
        "рывки хода: бой {}, машин {}, тиков {} (из них прогрев {}), машино-тиков {}, исход {:?}",
        a.preset.name(),
        n,
        ran,
        a.warm.min(ran),
        t.unit_ticks,
        m.outcome()
    );
    println!("{:<44} {:>10} {:>12}", "событие", "всего", "на тыс. м-т");
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
        "ход упёрся (blocked)",
        t.blocked,
        per(t.blocked)
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
            format!("перенос высоты за тик >= {v} ед."),
            t.dy[k],
            per(t.dy[k])
        );
    }
    println!(
        "{:<44} {:>10.2}",
        "наибольший перенос высоты, ед.", t.dy_max
    );
    ExitCode::SUCCESS
}
