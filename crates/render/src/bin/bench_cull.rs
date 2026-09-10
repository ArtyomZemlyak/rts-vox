//! Замер отсечения: сколько чанков в обзоре и сколько **входит в него
//! за тик**.
//!
//! Второе число открыто в `docs/10-budgets.md`: копирование чанков
//! в память игрока стоит 0.2 мс на тысячу (ADR-081), а сколько их
//! набегает за тик — не мерил никто.
//!
//! Скорости движения камеры здесь — **оговорка, а не факт**: панорама
//! сорок вокселей в секунду и поворот девяносто градусов в секунду
//! взяты как правдоподобный предел для RTS. Число входящих чанков прямо
//! пропорционально им, поэтому оно и печатается вместе с ними.

use render::{Camera, ChunkGrid, V3};
use std::time::Instant;

const TICKS_PER_SECOND: f32 = 30.0;
/// Панорама, вокселей в секунду.
const PAN: f32 = 40.0;
/// Поворот, радиан в секунду.
const SPIN: f32 = std::f32::consts::FRAC_PI_2;

fn cam(target: V3, distance: f32, yaw: f32) -> Camera {
    Camera {
        target,
        yaw,
        pitch: 0.95,
        distance,
        fovy: 0.87,
        aspect: 16.0 / 9.0,
        near: 1.0,
        far: 3000.0,
    }
}

/// Сколько элементов есть в `b` и нет в `a`. Оба списка отсортированы —
/// `visible_chunks` идёт растровым порядком.
fn entered(a: &[u32], b: &[u32]) -> usize {
    let (mut i, mut j, mut n) = (0usize, 0usize, 0usize);
    while j < b.len() {
        while i < a.len() && a[i] < b[j] {
            i += 1;
        }
        if i >= a.len() || a[i] != b[j] {
            n += 1;
        }
        j += 1;
    }
    n
}

fn main() {
    let worlds = [(1024u32, 256u32), (2048, 256)];
    println!("отсечение чанков: камера RTS, чанк мешинга 62³");
    println!(
        "оговорка: панорама {PAN} вокселей/с, поворот {:.0}°/с, {TICKS_PER_SECOND} Гц\n",
        SPIN.to_degrees()
    );

    for (side, height) in worlds {
        let g = ChunkGrid {
            nx: side.div_ceil(62),
            ny: height.div_ceil(62),
            nz: side.div_ceil(62),
            cs: 62,
        };
        println!("мир {side}×{height}×{side} — {} чанков", g.len());
        println!(
            "{:<26} {:>10} {:>10} {:>13} {:>13}",
            "дальность камеры", "видно", "% мира", "панорама", "поворот"
        );
        println!(
            "{:<26} {:>10} {:>10} {:>13} {:>13}",
            "", "", "", "среднее/пик", "среднее/пик"
        );
        for distance in [200.0f32, 400.0, 800.0] {
            let mid = V3::new(side as f32 * 0.5, 0.0, side as f32 * 0.5);
            let (mut a, mut b) = (Vec::new(), Vec::new());
            render::visible_chunks(&g, &cam(mid, distance, 0.0).frustum(), &mut a);
            let seen = a.len();

            // Ход по всей траектории, а не один шаг: чанк шириной
            // шестьдесят два воксела при панораме в полтора воксела
            // за тик входит раз в сорок тиков, и одиночный замер
            // показал бы ноль или сорок в зависимости от того, куда
            // попал.
            let ticks = 600;
            let step = PAN / TICKS_PER_SECOND;
            let (mut pan_sum, mut pan_max) = (0usize, 0usize);
            render::visible_chunks(&g, &cam(mid, distance, 0.0).frustum(), &mut a);
            for k in 1..=ticks {
                let t = V3::new(mid.x + step * k as f32, mid.y, mid.z);
                render::visible_chunks(&g, &cam(t, distance, 0.0).frustum(), &mut b);
                let n = entered(&a, &b);
                pan_sum += n;
                pan_max = pan_max.max(n);
                std::mem::swap(&mut a, &mut b);
            }

            let spin = SPIN / TICKS_PER_SECOND;
            let (mut spin_sum, mut spin_max) = (0usize, 0usize);
            render::visible_chunks(&g, &cam(mid, distance, 0.0).frustum(), &mut a);
            for k in 1..=ticks {
                render::visible_chunks(&g, &cam(mid, distance, spin * k as f32).frustum(), &mut b);
                let n = entered(&a, &b);
                spin_sum += n;
                spin_max = spin_max.max(n);
                std::mem::swap(&mut a, &mut b);
            }

            println!(
                "{:<26.0} {:>10} {:>9.1}% {:>6.1} / {:<4} {:>6.1} / {:<4}",
                distance,
                seen,
                seen as f32 * 100.0 / g.len() as f32,
                pan_sum as f32 / ticks as f32,
                pan_max,
                spin_sum as f32 / ticks as f32,
                spin_max
            );
        }

        // Цена самого прохода: перебор всех чанков.
        let c = cam(V3::new(side as f32 * 0.5, 0.0, side as f32 * 0.5), 400.0, 0.0);
        let f = c.frustum();
        let mut v = Vec::new();
        let t = Instant::now();
        let reps = 2000;
        for _ in 0..reps {
            render::visible_chunks(&g, &f, &mut v);
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / reps as f64;
        println!("{:<26} {:>10.1} мкс\n", "перебор всех чанков", us);
    }

    println!("Числа — за тик, усреднённые по шестистам тикам движения,");
    println!("и рядом пик. Первый кадр стоит всего обзора разом.");
}
