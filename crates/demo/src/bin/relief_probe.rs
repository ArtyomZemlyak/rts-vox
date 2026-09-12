//! ВРЕМЕННЫЙ зонд рельефа. Удаляется до коммита.

use nav::layer::NavLayer;
use nav::volume::VolGraph;
use std::time::Instant;
use voxel::World;
use worldgen::{mat, material, surface_height, Params, Terrain};

fn main() {
    let mut side = 384u32;
    let mut relief = 30u32;
    let mut seed = 42u64;
    let mut sized = true;
    let d = Params::default();
    let (mut terr, mut tlo, mut thi, mut bed) =
        (d.terrace, d.terrace_lo, d.terrace_hi, d.bed);
    let (mut feature, mut detail) = (d.feature, d.detail);
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--side" => side = v.parse().unwrap(),
            "--relief" => relief = v.parse().unwrap(),
            "--seed" => seed = v.parse().unwrap(),
            "--sized" => sized = v != "0",
            "--terrace" => terr = v.parse().unwrap(),
            "--lo" => tlo = v.parse().unwrap(),
            "--hi" => thi = v.parse().unwrap(),
            "--bed" => bed = v.parse().unwrap(),
            "--feature" => feature = v.parse().unwrap(),
            "--detail" => detail = v.parse().unwrap(),
            _ => {}
        }
    }
    let p = Params {
        relief,
        terrace: terr,
        terrace_lo: tlo,
        terrace_hi: thi,
        bed,
        feature,
        detail,
        ..if sized {
            Params::sized(side, side / 2, side)
        } else {
            Params { nx: side, ny: side / 2, nz: side, ..Params::default() }
        }
    };
    println!(
        "мир {}×{}×{}, base {}, relief {}, шаг {} деталь {}, терраса {} [{}..{}], пласт {}, сид {seed}",
        p.nx, p.ny, p.nz, p.base_height, p.relief, p.feature, p.detail, p.terrace, p.terrace_lo, p.terrace_hi, p.bed
    );

    // --- высоты
    let mut hs = vec![0u32; (p.nx * p.nz) as usize];
    let t = Instant::now();
    for z in 0..p.nz {
        for x in 0..p.nx {
            hs[(z * p.nx + x) as usize] = surface_height(&p, seed, x, z);
        }
    }
    let hms = t.elapsed().as_secs_f64() * 1e3;
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    let mut clamped = 0u32;
    let cap = p.ny - 8;
    for &h in &hs {
        lo = lo.min(h);
        hi = hi.max(h);
        if h == cap || h == 8 {
            clamped += 1;
        }
    }
    println!(
        "высота {lo}..{hi}, упёрлось в обрезку {:.1}%, поле высот {hms:.0} мс",
        clamped as f64 * 100.0 / hs.len() as f64
    );

    // --- уклон по вокселям
    let mut slope = [0u32; 12];
    for z in 0..p.nz - 1 {
        for x in 0..p.nx - 1 {
            let h = hs[(z * p.nx + x) as usize] as i64;
            let dx = (hs[(z * p.nx + x + 1) as usize] as i64 - h).unsigned_abs();
            let dz = (hs[((z + 1) * p.nx + x) as usize] as i64 - h).unsigned_abs();
            slope[(dx.max(dz) as usize).min(11)] += 1;
        }
    }
    let n = ((p.nx - 1) * (p.nz - 1)) as f64;
    print!("уклон вокс.:");
    for (i, c) in slope.iter().enumerate() {
        if *c > 0 {
            print!(" {i}:{:.1}%", *c as f64 * 100.0 / n);
        }
    }
    println!();

    // --- ВРЕМЕННО: цена колонки генератора и цена повоксельного пути
    {
        let mut buf = vec![mat::AIR; p.ny as usize];
        let t = Instant::now();
        let mut acc = 0u64;
        for z in 0..p.nz {
            for x in 0..p.nx {
                worldgen::column(&p, seed, x, z, 0, &mut buf);
                acc += buf[0] as u64;
            }
        }
        println!("ВРЕМЕННО колонки {:.0} мс (acc {acc})", t.elapsed().as_secs_f64() * 1e3);
        let g: Box<dyn voxel::Generator + Send + Sync> = Box::new(Terrain { params: p, seed });
        let t = Instant::now();
        let mut acc2 = 0u64;
        for z in 0..p.nz {
            for x in 0..p.nx {
                g.column(x, z, 0, &mut buf);
                acc2 += buf[0] as u64;
            }
        }
        println!("ВРЕМЕННО колонки через trait {:.0} мс (acc {acc2})", t.elapsed().as_secs_f64() * 1e3);
    }

    // --- мир и навигация
    let t = Instant::now();
    let w = World::new(p.nx, p.ny, p.nz, 1 << 16, Box::new(Terrain { params: p, seed }));
    let gen = t.elapsed().as_secs_f64() * 1e3;
    {
        let t = Instant::now();
        let topo = voxel::topo::layer::TopoLayer::new(&w);
        let topo_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let col = voxel::collapse::CollapseLayer::new(&w);
        let col_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let tu2 = demo::tuning();
        let nv = NavLayer::new(&w, tu2.unit_height, tu2.nav_cell, tu2.max_step);
        let nav2 = t.elapsed().as_secs_f64() * 1e3;
        println!("ВРЕМЕННО: topo {topo_ms:.0} мс, каскад {col_ms:.0} мс, нав {nav2:.0} мс (chunks {} {})",
            std::hint::black_box(&topo) as *const _ as usize & 1,
            std::hint::black_box(&col) as *const _ as usize & 1);
        std::hint::black_box(&nv);
    }
    let tu = demo::tuning();
    let t = Instant::now();
    let nav = NavLayer::new(&w, tu.unit_height, tu.nav_cell, tu.max_step);
    let nav_ms = t.elapsed().as_secs_f64() * 1e3;
    let (gw, gh) = nav.dims();
    println!("мир создан {gen:.0} мс, нав-слой {nav_ms:.0} мс, клеток {}×{}", gw, gh);

    // компоненты объёмного графа «клетка и уровень» — как в nav_report
    let mut g = VolGraph::new();
    let t = Instant::now();
    g.build(&nav);
    let g_ms = t.elapsed().as_secs_f64() * 1e3;
    let mut sizes = vec![0u32; g.components];
    for i in 0..g.nodes {
        let c = g.comp_of(i) as usize;
        if c < sizes.len() {
            sizes[c] += 1;
        }
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let big = sizes.first().copied().unwrap_or(0);
    println!(
        "узлов {} , рёбер {}, компонент {}, сборка {g_ms:.0} мс",
        g.nodes, g.edges, g.components
    );
    println!(
        "  наибольшая компонента {big} = {:.1}% узлов",
        big as f64 * 100.0 / g.nodes.max(1) as f64
    );
    let top: Vec<u32> = sizes.iter().take(6).copied().collect();
    println!("  крупнейшие {top:?}");
    println!("  из одного узла {}", sizes.iter().filter(|s| **s == 1).count());

    // поверхностная связность: только верхний уровень клетки
    let cells = (gw * gh) as usize;
    let mut comp = vec![u32::MAX; cells];
    let mut ssz: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    let mut standable = 0u32;
    for i in 0..cells {
        let (cx, cz) = (i as u32 % gw, i as u32 / gw);
        if !nav.standable(cx, cz) {
            continue;
        }
        standable += 1;
        if comp[i] != u32::MAX {
            continue;
        }
        let id = ssz.len() as u32;
        ssz.push(0);
        comp[i] = id;
        stack.push(i as u32);
        while let Some(c) = stack.pop() {
            ssz[id as usize] += 1;
            let (x, z) = (c % gw, c / gw);
            for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                if nx < 0 || nz < 0 || nx >= gw as i32 || nz >= gh as i32 {
                    continue;
                }
                let j = (nz as u32 * gw + nx as u32) as usize;
                if comp[j] != u32::MAX || !nav.standable(nx as u32, nz as u32) {
                    continue;
                }
                if nav.top_at(x, z).abs_diff(nav.top_at(nx as u32, nz as u32)) > tu.max_step as u16
                {
                    continue;
                }
                comp[j] = id;
                stack.push(j as u32);
            }
        }
    }
    ssz.sort_unstable_by(|a, b| b.cmp(a));
    let sbig = ssz.first().copied().unwrap_or(0);
    println!(
        "по поверхности: клеток {standable}, компонент {}, наибольшая {:.1}%",
        ssz.len(),
        sbig as f64 * 100.0 / standable.max(1) as f64
    );
    let stop: Vec<u32> = ssz.iter().take(6).copied().collect();
    println!("  крупнейшие {stop:?}");

    // --- ровные площадки: сколько клеток в квадрате 8×8 нав-клеток одной высоты
    let mut flat_runs = 0u32;
    let mut checked = 0u32;
    for cz in (0..gh.saturating_sub(4)).step_by(4) {
        for cx in (0..gw.saturating_sub(4)).step_by(4) {
            checked += 1;
            let f = nav.top_at(cx, cz);
            let mut same = true;
            for dz in 0..4 {
                for dx in 0..4 {
                    if nav.top_at(cx + dx, cz + dz) != f {
                        same = false;
                    }
                }
            }
            if same {
                flat_runs += 1;
            }
        }
    }
    println!(
        "ровных площадок 8×8 вокселей: {flat_runs} из {checked} = {:.1}%",
        flat_runs as f64 * 100.0 / checked.max(1) as f64
    );

    // --- обрывы: перепад между соседними нав-клетками
    let mut cliff = [0u32; 16];
    let mut pairs = 0u32;
    for cz in 0..gh {
        for cx in 0..gw - 1 {
            if !nav.standable(cx, cz) || !nav.standable(cx + 1, cz) {
                continue;
            }
            let d = nav.top_at(cx, cz).abs_diff(nav.top_at(cx + 1, cz));
            cliff[(d as usize).min(15)] += 1;
            pairs += 1;
        }
    }
    print!("перепад между клетками:");
    for (i, c) in cliff.iter().enumerate() {
        if *c > 0 {
            print!(" {i}:{:.1}%", *c as f64 * 100.0 / pairs.max(1) as f64);
        }
    }
    println!();

    // --- состав поверхности
    let mut hist = [0u32; 64];
    let mut cnt = 0u32;
    for z in (0..p.nz).step_by(2) {
        for x in (0..p.nx).step_by(2) {
            let h = surface_height(&p, seed, x, z);
            hist[(material(&p, seed, x, h - 1, z) as usize) & 63] += 1;
            cnt += 1;
        }
    }
    let pc = |m: u16| hist[(m as usize) & 63] as f64 * 100.0 / cnt as f64;
    let ore: f64 = [16u16, 17, 18, 24, 25, 26].iter().map(|m| pc(*m)).sum();
    println!(
        "поверхность: почва {:.1}%, порода {:.1}%, пласт {:.1}%, глубинная {:.1}%, руда {ore:.1}%",
        pc(mat::SOIL),
        pc(mat::ROCK),
        pc(mat::ROCK_PALE),
        pc(mat::DEEP_ROCK)
    );
    let mut other = 0.0;
    for (i, c) in hist.iter().enumerate() {
        let v = *c as f64 * 100.0 / cnt as f64;
        if v > 0.5 && ![0usize, 1, 2, 3, 4, 16, 17, 18, 24, 25, 26, 40].contains(&i) {
            println!("  прочее {i}: {v:.1}%");
            other += v;
        }
    }
    let _ = other;
}
