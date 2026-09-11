//! Замер графа секторов по объёмной навигации на выработанной карте.
//!
//! Вопрос, ради которого он написан: **окупается ли иерархия**. Поле
//! направлений считается сейчас по всей карте и отстаёт на сотню тиков
//! (ADR-138); архитектура (ADR-106) требует вести дальнее графом
//! порталов, а эйконал решать внутри сектора. Прежде чем это строить,
//! надо знать цену самого графа и цену запроса по нему.

use demo::dig::Dig;
use demo::Terrain;
use nav::layer::NavLayer;
use nav::sectors::{route, RouteWs, SectorGraph, SEC};
use nav::volume::{VolGraph, NO_NODE};
use std::process::ExitCode;
use std::time::Instant;
use voxel::World;

struct Args {
    side: u32,
    height: u32,
    diggers: usize,
    ticks: u64,
    seed: u64,
}

fn parse() -> Result<Args, String> {
    let mut a = Args { side: 256, height: 128, diggers: 300, ticks: 2000, seed: 42 };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut num = |name: &str| -> Result<u64, String> {
            it.next()
                .ok_or_else(|| format!("{name} без значения"))?
                .parse::<u64>()
                .map_err(|e| format!("{name}: {e}"))
        };
        match k.as_str() {
            "--side" => a.side = num("--side")? as u32,
            "--height" => a.height = num("--height")? as u32,
            "--diggers" => a.diggers = num("--diggers")? as usize,
            "--ticks" => a.ticks = num("--ticks")?,
            "--seed" => a.seed = num("--seed")?,
            "--help" => {
                println!("nav_report [--side N] [--height N] [--diggers N] [--ticks N]");
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ {other}")),
        }
    }
    Ok(a)
}

/// Лучшее из нескольких: планировщик шумит на десятки процентов.
fn best_ms(mut f: impl FnMut(), reps: usize) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    best
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };

    let params = worldgen::Params {
        nx: a.side,
        ny: a.height,
        nz: a.side,
        ..Default::default()
    };
    let terrain = Terrain { params, seed: a.seed };
    let mut w = World::new(
        params.nx,
        params.ny,
        params.nz,
        1 << 16,
        Box::new(Terrain { params, seed: a.seed }),
    );
    let mut dig = Dig::clustered(&w, |x, z| terrain.surface(x, z), a.diggers, 2, 12, a.seed ^ 0xF10);
    let mut edits = 0u64;
    for _ in 0..a.ticks {
        dig.step(&mut w, &|x, z| terrain.surface(x, z));
        edits += w.flush().applied as u64;
    }

    let t = demo::tuning();
    let nav = NavLayer::new(&w, t.unit_height, t.nav_cell, t.max_step);
    let mut g = VolGraph::new();
    let g_ms = best_ms(|| g.build(&nav), 3);
    let (gw, gh) = nav.dims();

    println!("граф секторов по объёму — мир {}×{}×{}", a.side, a.height, a.side);
    println!("{} копателей, {} тиков, {edits} правок\n", a.diggers, a.ticks);
    println!("{:<34} {:>12}", "нав-клеток", gw * gh);
    println!("{:<34} {:>12}", "узлов «клетка и уровень»", g.nodes);
    println!("{:<34} {:>12}", "рёбер", g.edges);
    println!("{:<34} {:>12}", "компонент связности", g.components);
    println!("{:<34} {:>9.1} мс", "сборка объёмного графа", g_ms);

    let mut sg = SectorGraph::new();
    let mut st = Default::default();
    let sg_ms = best_ms(|| st = sg.build(&g), 3);
    let (sw, sh) = sg.sectors();
    println!("\n{:<34} {:>12}", "секторов", sw * sh);
    println!("{:<34} {:>12}", "  сторона сектора, клеток", SEC);
    println!("{:<34} {:>12}", "порталов", st.portals);
    println!("{:<34} {:>12}", "рёбер между порталами", st.edges);
    println!("{:<34} {:>12}", "решений «внутри сектора»", st.sector_solves);
    println!("{:<34} {:>12}", "  узлов посещено ими", st.visited);
    println!("{:<34} {:>9.1} мс", "сборка графа секторов", sg_ms);
    println!(
        "{:<34} {:>12.1}",
        "узлов на сектор, в среднем",
        g.nodes as f64 / (sw * sh).max(1) as f64
    );

    // --- Из чего состоит карта: размеры компонент.
    let mut size = vec![0u32; g.components.max(1)];
    for n in 0..g.nodes {
        size[g.comp_of(n) as usize] += 1;
    }
    let big = size
        .iter()
        .enumerate()
        .max_by_key(|(i, &v)| (v, std::cmp::Reverse(*i)))
        .map(|(i, _)| i as u32)
        .unwrap_or(0);
    let mut sorted = size.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    println!(
        "\n{:<34} {:>12} {:>8.1}%",
        "узлов в наибольшей компоненте",
        sorted[0],
        sorted[0] as f64 * 100.0 / g.nodes.max(1) as f64
    );
    println!(
        "{:<34} {:>12?}",
        "  следующие по величине",
        &sorted[1..sorted.len().min(6)]
    );
    println!(
        "{:<34} {:>12}",
        "компонент из одного узла",
        size.iter().filter(|&&v| v == 1).count()
    );

    // --- Запросы внутри наибольшей компоненты: только там маршрут
    //     вообще может существовать.
    let mut ws = RouteWs::new(sg.portals.len().max(1), g.nodes);
    let in_big: Vec<u32> = (0..g.nodes as u32)
        .filter(|&n| g.comp_of(n as usize) == big)
        .collect();
    let cell_of = |n: u32| g.cell_of(n as usize);
    let first = in_big.iter().copied().min_by_key(|&n| cell_of(n)).unwrap_or(NO_NODE);
    let last = in_big.iter().copied().max_by_key(|&n| cell_of(n)).unwrap_or(NO_NODE);
    let mid = in_big.get(in_big.len() / 2).copied().unwrap_or(NO_NODE);
    let picks: [(u32, u32, &str); 3] = [
        (first, last, "через всю компоненту"),
        (first, mid, "до середины"),
        (mid, last, "от середины"),
    ];
    println!(
        "\n{:<24} {:>8} {:>8} {:>10} {:>10}",
        "запрос", "нашёл", "порталов", "раскрыто", "мкс"
    );
    for (from, to, name) in picks {
        if from == NO_NODE || to == NO_NODE {
            println!("{name:<24} {:>8}", "нет узла");
            continue;
        }
        let mut r = Default::default();
        let us = best_ms(|| r = route(&sg, &g, &mut ws, from, to), 5) * 1e3;
        println!(
            "{name:<24} {:>8} {:>8} {:>10} {:>10.1}",
            if r.found {
                "да"
            } else if r.by_island {
                "остров"
            } else {
                "нет"
            },
            r.hops,
            r.expanded,
            us
        );
    }

    // --- Поле по секторам против поля на всю карту.
    {
        let mut f = nav::volume::VolField::new();
        let mut ws2 = RouteWs::new(sg.portals.len().max(1), g.nodes);
        let target = first;
        let mut fs = Default::default();
        let mut each: Vec<f64> = Vec::new();
        for _ in 0..5 {
            let t0 = Instant::now();
            fs = nav::sectors::solve_field(&sg, &g, &mut f, &mut ws2, target, &nav::godunov::h_exact_floor);
            each.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        let ms = each.iter().cloned().fold(f64::MAX, f64::min);
        println!("{:<34} {:>12?}", "поле, каждый прогон, мс", each.iter().map(|v| (v * 10.0).round() / 10.0).collect::<Vec<_>>());
        println!("
{:<34} {:>12}", "поле по секторам: решено секторов", fs.sectors);
        println!("{:<34} {:>12}", "  годуновских обновлений", fs.updates);
        println!("{:<34} {:>12}", "  узлов получило направление", fs.reached);
        // Кто остался без направления. Доля важнее факта: «часть узлов
        // без направления» — это не замер, а отговорка.
        let cov = nav::sectors::coverage(&sg, &g, &f, &ws2);
        println!(
            "{:<34} {:>12} {:>7.1}%",
            "  из них цели компонент", cov.goals,
            100.0 * cov.goals as f64 / cov.nodes as f64
        );
        println!(
            "{:<34} {:>12} {:>7.1}%",
            "  в глухих секторах", cov.dead,
            100.0 * cov.dead as f64 / cov.nodes as f64
        );
        println!(
            "{:<34} {:>12} {:>7.1}%",
            "  ДЫР (дойти было куда)", cov.holes,
            100.0 * cov.holes as f64 / cov.nodes as f64
        );
        println!(
            "{:<34} {:>12} {:>7.1}%",
            "    из них у границы сектора", cov.holes_touching,
            100.0 * cov.holes_touching as f64 / cov.holes.max(1) as f64
        );
        println!(
            "{:<34} {:>12} {:>7.1}%",
            "    кусок сектора без засева", cov.holes_unseeded,
            100.0 * cov.holes_unseeded as f64 / cov.holes.max(1) as f64
        );
        println!(
            "{:<34} {:>12} {:>7.1}%",
            "    засев в куске есть, не дошло", cov.holes_unreached,
            100.0 * cov.holes_unreached as f64 / cov.holes.max(1) as f64
        );
        println!(
            "{:<34} {:>12} {:>7.1}%",
            "  кусков в секторах без засева", cov.pieces_unseeded,
            100.0 * cov.pieces_unseeded as f64 / cov.pieces.max(1) as f64
        );
        println!(
            "{:<34} {:>12} {:>7.1}%",
            "    из них у границы (есть выход)", cov.pieces_unseeded_touching,
            100.0 * cov.pieces_unseeded_touching as f64 / cov.pieces_unseeded.max(1) as f64
        );
        println!("{:<34} {:>12}", "  сторон порталов живых", fs.live_ports);
        println!("{:<34} {:>9.1} мс", "  всё поле", ms);

        let mut gf = nav::volume::VolField::new();
        let gms = best_ms(
            || nav::volume::solve(&g, &mut gf, 0, 0, &nav::godunov::h_exact_floor),
            3,
        );
        println!("{:<34} {:>9.1} мс", "поле на всю карту, для сравнения", gms);
    }

    // --- Инкрементальная пересборка: копаем ещё немного и смотрим,
    //     сколько стоит догнать граф.
    println!(
        "\n{:<24} {:>10} {:>10} {:>12} {:>10}",
        "грязных секторов", "сторон", "переисп.", "пересчитано", "мс"
    );
    for more in [1u64, 20, 200] {
        let mut w2 = World::new(
            params.nx,
            params.ny,
            params.nz,
            1 << 16,
            Box::new(Terrain { params, seed: a.seed }),
        );
        let mut d2 = Dig::clustered(
            &w2,
            |x, z| terrain.surface(x, z),
            a.diggers,
            2,
            12,
            a.seed ^ 0xF10,
        );
        for _ in 0..a.ticks {
            d2.step(&mut w2, &|x, z| terrain.surface(x, z));
            w2.flush();
        }
        let mut nav2 = NavLayer::new(&w2, t.unit_height, t.nav_cell, t.max_step);
        let mut g2 = VolGraph::new();
        g2.build(&nav2);
        let mut sg2 = SectorGraph::new();
        sg2.build(&g2);

        // Ещё немного копки — и отмечаем задетые секторы.
        let mut dirty: Vec<u32> = Vec::new();
        for _ in 0..more {
            d2.step(&mut w2, &|x, z| terrain.surface(x, z));
            let st = w2.flush();
            let _ = st;
            for e in w2.edits() {
                let (cx, cz) = (e.x / t.nav_cell, e.z / t.nav_cell);
                let sec = (cz / nav::sectors::SEC) * sg2.sectors().0 + cx / nav::sectors::SEC;
                if !dirty.contains(&sec) {
                    dirty.push(sec);
                }
            }
            nav2.apply(&w2, w2.edits());
        }
        dirty.sort_unstable();
        g2.build(&nav2);
        let mut st2 = Default::default();
        let ms = best_ms(|| st2 = sg2.rebuild(&g2, &dirty), 3);
        println!(
            "{:<24} {:>10} {:>10} {:>12} {:>10.2}",
            dirty.len(),
            st2.portals * 2,
            st2.reused_ports,
            st2.solved_sectors,
            ms
        );
    }

    // --- Переиспользование секторов поля при продолжающейся копке.
    {
        let mut w3 = World::new(
            params.nx,
            params.ny,
            params.nz,
            1 << 16,
            Box::new(Terrain { params, seed: a.seed }),
        );
        let mut d3 = Dig::clustered(
            &w3,
            |x, z| terrain.surface(x, z),
            a.diggers,
            2,
            12,
            a.seed ^ 0xF10,
        );
        for _ in 0..a.ticks {
            d3.step(&mut w3, &|x, z| terrain.surface(x, z));
            w3.flush();
        }
        let mut nav3 = NavLayer::new(&w3, t.unit_height, t.nav_cell, t.max_step);
        let (cw, ch) = nav3.dims();
        let mut nf = nav::sectors::NavField::new(1);
        nf.set_target(0, (a.side / 2) as i32, (a.side / 2) as i32);
        let b = nav::sectors::FieldBudget {
            cells: t.flow_cells_per_tick,
            scan: t.flow_scan_per_tick,
            sectors: t.flow_sectors_per_tick,
        };
        let mut cycles = 0;
        let mut ticks = 0u32;
        let (mut solved, mut reused) = (0usize, 0usize);
        // Цена цикла целиком — не сумма отдельно замеренных кусков,
        // а то, что тратит сам оркестратор. Часы держит замерщик:
        // в `nav` время брать нельзя, это sim-крейт.
        let mut cycle_ms = 0.0f64;
        let mut each: Vec<f64> = Vec::new();
        while cycles < 4 && ticks < 100_000 {
            d3.step(&mut w3, &|x, z| terrain.surface(x, z));
            w3.flush();
            nav3.apply(&w3, w3.edits());
            for e in w3.edits() {
                nf.mark_cell(e.x / t.nav_cell, e.z / t.nav_cell, cw);
            }
            let t0 = Instant::now();
            let done = nf.step(&nav3, b, &nav::godunov::h_exact_floor);
            cycle_ms += t0.elapsed().as_secs_f64() * 1e3;
            if done {
                cycles += 1;
                solved = nf.solved_sectors;
                reused = nf.reused_sectors;
                each.push(cycle_ms);
                cycle_ms = 0.0;
            }
            ticks += 1;
        }
        println!("\n{:<34} {:>12} {:>12}", "секторов поля за 4 цикла", "пересчёт", "готовыми");
        println!("{:<34} {:>12} {:>12}", "", solved, reused);
        println!("{:<34} {:>12}", "  тиков на цикл", nf.last_ticks);
        println!(
            "{:<34} {:>12?}",
            "  цикл целиком, мс",
            each.iter().map(|v| (v * 10.0).round() / 10.0).collect::<Vec<_>>()
        );
        if let Some(last) = each.last() {
            println!(
                "{:<34} {:>11.1}%",
                "  из них сборка графа объёма",
                100.0 * g_ms / last
            );
        }
        println!(
            "{:<34} {:>12}",
            "  всего секторов",
            (cw.div_ceil(16) * ch.div_ceil(16)) as usize
        );
    }

    println!("\nЦена иерархии — сборка графа секторов; цена запроса — таблица выше.");
    println!("Поле по всей карте стоит 40 мс (ADR-138); сектор — это {:.0} узлов",
             g.nodes as f64 / (sw * sh).max(1) as f64);
    println!("вместо {}, то есть во столько же раз меньше работы на одно поле.", g.nodes);
    ExitCode::SUCCESS
}
