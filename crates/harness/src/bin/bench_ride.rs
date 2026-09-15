//! Во что обходится физика корпуса на тик.
//!
//! Замер, а не оценка: фаза обязана уложиться в бюджет тика
//! (`docs/10-budgets.md`), и число опор — главный рычаг цены.

use std::time::Instant;

use fx::Fx;
use phys::{step, Chassis, Ride, Support, V3};

/// Ходовая, близкая к гусеничной машине. Натяг — под вес на опору,
/// как в фазе хода (ADR-234).
fn ride(supports: usize) -> Ride {
    let gravity = Fx::from_ratio(98, 10);
    Ride {
        stiffness: Fx::from_int(40),
        damping: Fx::from_int(8),
        travel: Fx::from_ratio(1, 2),
        clearance: Fx::from_ratio(1, 2),
        inertia: Fx::from_int(4),
        gravity,
        preload: gravity.div(Fx::from_int(supports.max(1) as i32)),
        angular_damping: Fx::from_int(3),
        min_up: Fx::from_ratio(91, 100),
        settle: Fx::from_ratio(1, 2),
    }
}

/// Опоры по прямоугольнику: сколько просили, поровну с двух бортов.
fn supports(n: usize) -> Vec<Support> {
    let half = (n / 2).max(1);
    let w = Fx::from_ratio(3, 2);
    let mut out = Vec::with_capacity(n);
    for i in 0..half {
        // Разнести по длине от носа к корме.
        let t = Fx::from_ratio(i as i32 * 2, half as i32).sub(Fx::ONE);
        let x = t.mul(Fx::from_int(3));
        out.push(Support::new(x, w));
        if out.len() < n {
            out.push(Support::new(x, w.neg()));
        }
    }
    out
}

fn main() {
    let units: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    let ticks: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    let dt = Fx::from_ratio(1, 30);

    println!("физика корпуса: {units} машин, {ticks} тиков");
    println!(
        "{:>6} {:>12} {:>14} {:>14}",
        "опор", "фаза, мс", "на машину, нс", "на опору, нс"
    );

    for n in [2usize, 4, 6, 8, 12] {
        let r = ride(n);
        let s = supports(n);
        // Рельеф под опорами: небольшой уклон, чтобы работали все ветки.
        let g: Vec<Fx> = s
            .iter()
            .map(|p| {
                p.x.mul(Fx::from_ratio(1, 8))
                    .sub(p.z.mul(Fx::from_ratio(1, 16)))
            })
            .collect();
        let mut cs: Vec<Chassis> = (0..units).map(|_| Chassis::resting(Fx::ZERO, &r)).collect();
        // Разогрев: первые тики оседают, и мерить их отдельно нечестно.
        for _ in 0..30 {
            for c in cs.iter_mut() {
                step(c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, dt);
            }
        }
        let t = Instant::now();
        for _ in 0..ticks {
            for c in cs.iter_mut() {
                step(c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, dt);
            }
        }
        let el = t.elapsed().as_secs_f64();
        let per_tick_ms = el / ticks as f64 * 1e3;
        let per_unit_ns = el / (ticks * units) as f64 * 1e9;
        println!(
            "{n:>6} {per_tick_ms:>12.3} {per_unit_ns:>14.1} {:>14.1}",
            per_unit_ns / n as f64
        );
        // Выдача, чтобы оптимизатор не выбросил цикл целиком.
        let sum: i64 = cs.iter().map(|c| c.y.raw()).fold(0i64, i64::wrapping_add);
        if sum == 0x5EED {
            println!("невозможное совпадение {sum}");
        }
    }
}
