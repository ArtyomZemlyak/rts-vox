//! Щиты и сеть энергии: две строки бюджета с пометкой «не определён».
//!
//! Запуск: `cargo run --release -p systems --bin bench_systems`

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use sim_core::energy::{Network, Node};
use sim_core::shield::{brute, Point, Shield, ShieldIndex};

const WORLD: i32 = 2000;

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn shield_scene(ns: usize, np: usize, r: i32, seed: u64) -> (Vec<Shield>, Vec<Point>) {
    let mut rng = Rng::new(seed, 1);
    let shields = (0..ns)
        .map(|_| Shield {
            x: rng.below(WORLD as u32) as i32,
            z: rng.below(WORLD as u32) as i32,
            y: rng.range(0, 40),
            r: rng.range(r / 2, r),
            owner: rng.below(2) as u8,
            charge: rng.range(1, 100),
        })
        .collect();
    let points = (0..np)
        .map(|_| Point {
            x: rng.below(WORLD as u32) as i32,
            z: rng.below(WORLD as u32) as i32,
            y: rng.range(0, 40),
            owner: rng.below(2) as u8,
        })
        .collect();
    (shields, points)
}

fn main() {
    println!("мир {WORLD}×{WORLD} мировых единиц, бюджет тика 10 мс\n");

    section("1. Щиты: предикат сам по себе");
    let (sh, pts) = shield_scene(1, 1 << 16, 80, 3);
    let s = sh[0];
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut acc = 0u64;
        for p in pts.iter() {
            acc += sim_core::shield::inside(&s, p) as u64;
        }
        black_box(acc);
        best = best.min(t0.elapsed().as_secs_f64() * 1e9 / pts.len() as f64);
    }
    println!("одна проверка «точка внутри щита»: {best:.2} нс");
    println!("(сравнение квадратов, без корня и без деления — ADR-071)");

    section("2. Щиты: пары решают, а не предикат");
    println!(
        "{:>8} {:>10} {:>14} {:>14} {:>14} {:>12}",
        "щитов", "снарядов", "пар перебором", "пар индексом", "перебор, мкс", "индекс, мкс"
    );
    for (ns, np) in [
        (8usize, 200usize),
        (32, 1000),
        (128, 2000),
        (512, 5000),
        (2000, 20000),
    ] {
        let (shields, points) = shield_scene(ns, np, 80, 5);
        let mut out = Vec::new();
        let tested_b = brute(&shields, &points, &mut out);
        let hits = out.len();
        let mut idx = ShieldIndex::new(WORLD, 80, shields.len());
        idx.rebuild(&shields);
        let tested_i = idx.pairs(&shields, &points, &mut out);
        assert_eq!(out.len(), hits, "индекс потерял пары");

        let mut a = f64::MAX;
        let mut b = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            black_box(brute(&shields, &points, &mut out));
            a = a.min(t0.elapsed().as_secs_f64() * 1e6);
            let t0 = Instant::now();
            idx.rebuild(&shields);
            black_box(idx.pairs(&shields, &points, &mut out));
            b = b.min(t0.elapsed().as_secs_f64() * 1e6);
        }
        println!(
            "{ns:>8} {np:>10} {:>14} {:>14} {a:>14.1} {b:>12.1}",
            tested_b, tested_i
        );
    }
    println!("(«пар» — сколько раз позвали предикат; в индексе сюда входит и пересборка)");

    section("3. Щиты: доля бюджета");
    let (shields, points) = shield_scene(64, 2000, 80, 7);
    let mut idx = ShieldIndex::new(WORLD, 80, shields.len());
    let mut out = Vec::new();
    let mut best = f64::MAX;
    for _ in 0..10 {
        let t0 = Instant::now();
        idx.rebuild(&shields);
        black_box(idx.pairs(&shields, &points, &mut out));
        best = best.min(t0.elapsed().as_secs_f64() * 1e6);
    }
    println!("64 щита, 2000 снарядов, индексом: {best:.1} мкс — {:.2}% тика", 100.0 * best / 1e4);

    section("4. Сеть энергии: полное обновление");
    println!(
        "{:>10} {:>10} {:>12} {:>14} {:>14}",
        "узлов", "связей", "компонент", "мкс", "нс на узел"
    );
    for (n, e) in [
        (200usize, 400usize),
        (1000, 2000),
        (5000, 10000),
        (20000, 40000),
    ] {
        let mut r = Rng::new(9, n as u64);
        let nodes: Vec<Node> = (0..n)
            .map(|i| {
                if i % 20 == 0 {
                    Node { produce: r.range(50, 200), demand: 0 }
                } else {
                    Node { produce: 0, demand: r.range(1, 12) }
                }
            })
            .collect();
        // Связи локальные: сеть тянется, а не соединяет случайные точки.
        let links: Vec<(u32, u32)> = (0..e)
            .map(|k| {
                let a = (k % n) as u32;
                let b = ((k % n) + 1 + r.below(3) as usize) % n;
                (a, b as u32)
            })
            .collect();
        let mut net = Network::new(nodes, links);
        net.update();
        let comps = net.components;
        let mut best = f64::MAX;
        for _ in 0..10 {
            let t0 = Instant::now();
            net.update();
            black_box(net.components);
            best = best.min(t0.elapsed().as_secs_f64() * 1e6);
        }
        println!(
            "{n:>10} {e:>10} {comps:>12} {best:>14.1} {:>14.1}",
            best * 1000.0 / n as f64
        );
    }

    section("5. Сеть энергии: что стоит перерезать снабжение");
    let n = 5000usize;
    let e = 10000usize;
    let mut r = Rng::new(9, 5);
    let nodes: Vec<Node> = (0..n)
        .map(|i| {
            if i % 20 == 0 {
                Node { produce: r.range(50, 200), demand: 0 }
            } else {
                Node { produce: 0, demand: r.range(1, 12) }
            }
        })
        .collect();
    let links: Vec<(u32, u32)> = (0..e)
        .map(|k| {
            let a = (k % n) as u32;
            let b = ((k % n) + 1 + r.below(3) as usize) % n;
            (a, b as u32)
        })
        .collect();
    let mut net = Network::new(nodes, links);
    net.update();
    let base = net.components;
    let mut best = f64::MAX;
    for _ in 0..10 {
        let t0 = Instant::now();
        net.cut(1234);
        net.update();
        net.restore(1234);
        net.update();
        black_box(net.components);
        best = best.min(t0.elapsed().as_secs_f64() * 1e6 / 2.0);
    }
    println!("5000 узлов: перерезать связь и пересчитать — {best:.1} мкс");
    println!("компонент до разреза: {base}");
    println!();
    println!("{:>16} {:>14} {:>16}", "разрезов за тик", "мс", "доля бюджета");
    for cuts in [1usize, 10, 50] {
        println!(
            "{cuts:>16} {:>14.3} {:>15.2}%",
            best * cuts as f64 / 1000.0,
            100.0 * best * cuts as f64 / 1000.0 / 10.0
        );
    }
    println!("\n(сеть пересчитывается целиком на каждый разрез — инкремента здесь нет)");
}
