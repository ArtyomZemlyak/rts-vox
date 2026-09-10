//! Q-63: сколько стоит персональный маршрут юнита по графу порталов.
//!
//! Запуск: `cargo run --release -p nav --bin bench_portals`
//!
//! Flow field строится на маршрут и делится всеми, кто по нему идёт
//! (ADR-096). Юнит с собственной целью поля не получает: пять тысяч целей
//! это пять тысяч полей по 0.36 мс. Вопрос в том, что стоит альтернатива.
//!
//! Меряются три величины: сборка графа, запрос маршрута и то, сколько
//! запросов помещается в бюджет тика.

use std::hint::black_box;
use std::time::Instant;

use fx::rng::Rng;
use nav::godunov::BLOCKED;
use nav::portals::{route, PortalGraph, RouteWs, SEC};
use nav::scene::{build, Scene};

const TICK_MS: f64 = 10.0;

fn best_of<F: FnMut()>(n: usize, mut f: F) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..n {
        let t0 = Instant::now();
        f();
        best = best.min(t0.elapsed().as_secs_f64());
    }
    best
}

fn section(t: &str) {
    println!("\n=== {t} ===");
}

fn main() {
    println!("сектор {SEC}×{SEC}, бюджет тика {TICK_MS} мс\n");

    section("1. Сборка графа: сколько порталов даёт геометрия");
    println!(
        "{:<10} {:>7} {:>10} {:>10} {:>12} {:>14} {:>12}",
        "сцена", "сторона", "секторов", "порталов", "на сектор", "рёбер", "сборка, мс"
    );
    for scene in [Scene::Open, Scene::Surface, Scene::Tunnels, Scene::Maze] {
        for n in [128usize, 512] {
            let g = build(scene, n, 5);
            let mut stats = None;
            let s = best_of(3, || {
                let (pg, st) = PortalGraph::build(&g);
                stats = Some((pg.sx * pg.sy, st));
                black_box(pg.portals.len());
            });
            let (nsec, st) = stats.unwrap();
            println!(
                "{:<10} {n:>7} {nsec:>10} {:>10} {:>12.1} {:>14} {:>12.2}",
                scene.name(),
                st.portals,
                st.portals as f64 / nsec as f64,
                st.edges,
                s * 1e3
            );
        }
    }

    section("2. Запрос маршрута: сколько стоит один");
    println!("буфер переиспользуется — выделение на запрос мерило бы аллокатор.\n");
    println!(
        "{:<10} {:>7} {:>12} {:>12} {:>12} {:>16}",
        "сцена", "сторона", "мкс/запрос", "раскрыто", "прыжков", "запросов в тик"
    );
    for scene in [Scene::Open, Scene::Surface, Scene::Tunnels, Scene::Maze] {
        for n in [128usize, 512] {
            let g = build(scene, n, 5);
            let (pg, _) = PortalGraph::build(&g);
            let mut ws = RouteWs::new(pg.portals.len().max(1));

            // Пары «откуда-куда» выбираются заранее: генерация внутри
            // таймера мерила бы генератор.
            let mut r = Rng::new(13, 0x63);
            let mut pairs = Vec::new();
            while pairs.len() < 2000 {
                let a = r.below((g.w * g.h) as u32) as usize;
                let b = r.below((g.w * g.h) as u32) as usize;
                if g.cost[a] != BLOCKED && g.cost[b] != BLOCKED {
                    pairs.push((a, b));
                }
            }

            let (mut exp, mut hops, mut found) = (0u64, 0u64, 0u64);
            let s = best_of(3, || {
                let (mut e, mut h, mut f) = (0u64, 0u64, 0u64);
                for &(a, b) in pairs.iter() {
                    let st = route(&pg, &g, &mut ws, a, b);
                    e += st.expanded as u64;
                    h += st.hops as u64;
                    f += st.found as u64;
                }
                exp = e;
                hops = h;
                found = f;
            });
            let us = s * 1e6 / pairs.len() as f64;
            let _ = found;
            println!(
                "{:<10} {n:>7} {us:>12.2} {:>12.1} {:>12.1} {:>16.0}",
                scene.name(),
                exp as f64 / pairs.len() as f64,
                hops as f64 / pairs.len() as f64,
                TICK_MS * 1000.0 / us
            );
        }
    }

    section("3. Сравнение с полем");
    println!("поле на маршрут (ADR-096): 0.36–0.41 мс на процессоре, 17.5 мкс");
    println!("на видеокарте. Оно обслуживает **любое** число юнитов с этой целью.\n");
    let g = build(Scene::Tunnels, 512, 5);
    let (pg, _) = PortalGraph::build(&g);
    let mut ws = RouteWs::new(pg.portals.len().max(1));
    let mut r = Rng::new(21, 0x63);
    let mut pairs = Vec::new();
    while pairs.len() < 5000 {
        let a = r.below((g.w * g.h) as u32) as usize;
        let b = r.below((g.w * g.h) as u32) as usize;
        if g.cost[a] != BLOCKED && g.cost[b] != BLOCKED {
            pairs.push((a, b));
        }
    }
    let s = best_of(3, || {
        for &(a, b) in pairs.iter() {
            black_box(route(&pg, &g, &mut ws, a, b));
        }
    });
    println!(
        "5 000 персональных маршрутов на карте 512×512: {:.2} мс — {:.0}% бюджета тика",
        s * 1e3,
        s * 1e3 / TICK_MS * 100.0
    );
    println!(
        "то же полями: 5 000 целей × 0.36 мс = {:.0} мс, то есть {:.0} бюджетов",
        5000.0 * 0.36,
        5000.0 * 0.36 / TICK_MS
    );

    section("4. Что стоит перестройка при копке");
    println!("копка меняет проходимость, и граф обязан за ней успевать.\n");
    let g = build(Scene::Tunnels, 512, 5);
    let full = best_of(3, || {
        black_box(PortalGraph::build(&g).0.portals.len());
    });
    println!("полная пересборка графа 512×512: {:.2} мс", full * 1e3);
    println!(
        "это {:.0}% бюджета тика — то есть каждый тик пересобирать нельзя,",
        full * 1e3 / TICK_MS * 100.0
    );
    println!("нужна инкрементальная: только секторы, где менялась проходимость.");
    println!("сколько это стоит — не измерено, инкремента ещё нет.");
}
