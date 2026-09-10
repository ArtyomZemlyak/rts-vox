//! Замеры эйконала. Печатает таблицы; порогов не проверяет — это разведка,
//! а не регрессионный тест.
//!
//! Запуск: `cargo run --release -p nav --bin bench_eikonal`

use std::cell::RefCell;
use std::hint::black_box;
use std::time::Instant;

use nav::godunov::*;
use nav::scene::{build, Grid, Scene};
use nav::solver::*;

fn goals_center(g: &Grid) -> Vec<usize> {
    let (cx, cy) = ((g.w - 1) / 2, (g.h - 1) / 2);
    let mut best = None;
    let mut best_d = usize::MAX;
    for y in 1..g.h - 1 {
        for x in 1..g.w - 1 {
            let i = y * g.w + x;
            if g.cost[i] == BLOCKED {
                continue;
            }
            let d = cx.abs_diff(x) + cy.abs_diff(y);
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
    }
    vec![best.expect("сцена целиком непроходима")]
}

fn bits(v: u32) -> u32 {
    32 - v.leading_zeros()
}

// ---------------------------------------------------------------------------

fn section(title: &str) {
    println!("\n=== {title} ===");
}

/// 1. Сколько стоит одно годуновское обновление, в наносекундах.
fn bench_h_variants() {
    section("1. Цена одного обновления ячейки");

    // Пары (f, d) берутся из настоящего решения, а не из равномерного шума:
    // распределение `d` сильно смещено к нулю, и это меняет предсказание
    // ветвления.
    let g = build(Scene::Surface, 256, 7);
    let goals = goals_center(&g);
    let rec: RefCell<Vec<(u32, u32)>> = RefCell::new(Vec::new());
    {
        let h = |f: u32, d: u32| {
            rec.borrow_mut().push((f, d));
            h_exact_floor(f, d)
        };
        let _ = lsm(&g, &goals, &h);
    }
    let pairs = rec.into_inner();
    let n = pairs.len();
    let two_d = pairs.iter().filter(|&&(f, d)| d < f).count();
    println!(
        "выборка: {n} обновлений со сцены surface 256x256, из них двумерная ветвь {:.1}%",
        100.0 * two_d as f64 / n as f64
    );

    let sqrt12 = SqrtTable12::new();
    let htab = HTable::new(255);
    let htab_small = HTable::new(63);
    let uni = UniformTable::new(10);
    println!(
        "таблицы: h(255) = {} КБ, h(63) = {} КБ, sqrt12 = 8 КБ, uniform(10) = {} Б",
        htab.bytes() / 1024,
        htab_small.bytes() / 1024,
        uni.bytes()
    );

    let reps = (40_000_000 / n).max(4);

    // Замер обязан быть родовым, а не через `dyn`: в решателе `h`
    // встраивается, и косвенный вызов маскировал бы всю разницу.
    fn run<F: Fn(u32, u32) -> u32>(name: &str, pairs: &[(u32, u32)], reps: usize, f: F) -> f64 {
        let mut acc = 0u64;
        for &(c, d) in pairs.iter() {
            acc = acc.wrapping_add(f(c, d) as u64);
        }
        black_box(acc);
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t0 = Instant::now();
            let mut acc = 0u64;
            for _ in 0..reps {
                for &(c, d) in black_box(pairs).iter() {
                    acc = acc.wrapping_add(f(c, d) as u64);
                }
            }
            let el = t0.elapsed();
            black_box(acc);
            best = best.min(el.as_secs_f64() * 1e9 / (reps * pairs.len()) as f64);
        }
        println!("  {name:<28} {best:>6.2} нс/обновление");
        best
    }

    let base = run("isqrt, флор", &pairs, reps, h_exact_floor);
    run("isqrt, к ближайшему", &pairs, reps, h_exact_round);
    let t12 = run("таблица корня 12 бит", &pairs, reps, |f, d| sqrt12.h(f, d));
    let th = run("таблица h(f,d), 64 КБ", &pairs, reps, |f, d| htab.h(f, d));
    let ths = run("таблица h(f,d), 4 КБ", &pairs, reps, |f, d| htab_small.h(f.min(63), d));
    let tu = run("равномерная, 1D", &pairs, reps, |_f, d| uni.h(d));
    println!(
        "  ускорение к isqrt: sqrt12 x{:.2}, h64K x{:.2}, h4K x{:.2}, uniform x{:.2}",
        base / t12,
        base / th,
        base / ths,
        base / tu
    );
}

/// 2. Сколько обновлений на достижимую ячейку тратит каждый решатель.
fn bench_solvers() {
    section("2. Обновлений на достижимую ячейку");
    println!(
        "{:<10} {:>5} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "сцена", "n", "прох.%", "fmm", "lsm", "fim", "fsm", "g-s", "jacobi"
    );
    let h = h_exact_floor;
    for scene in Scene::ALL {
        for n in [16usize, 32, 64, 128, 256] {
            let g = build(scene, n, 7);
            let goals = goals_center(&g);
            let (_, s_fmm) = fmm(&g, &goals, &h);
            let (_, s_lsm) = lsm(&g, &goals, &h);
            let (_, s_fim) = fim(&g, &goals, &h);
            let (_, s_fsm) = fsm(&g, &goals, &h);
            let (_, s_gs) = gauss_seidel(&g, &goals, &h);
            let (_, s_jac) = jacobi(&g, &goals, &h);
            println!(
                "{:<10} {:>5} {:>7.1} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2}",
                scene.name(),
                n,
                100.0 * g.passable() as f64 / g.inner_area() as f64,
                s_fmm.per_cell(),
                s_lsm.per_cell(),
                s_fim.per_cell(),
                s_fsm.per_cell(),
                s_gs.per_cell(),
                s_jac.per_cell(),
            );
        }
    }
    println!("\nчисло проходов/раундов (fsm и lsm считают развёртки, fim — итерации):");
    println!(
        "{:<10} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "сцена", "n", "lsm", "fim", "fsm", "g-s", "jacobi"
    );
    for scene in Scene::ALL {
        for n in [32usize, 128] {
            let g = build(scene, n, 7);
            let goals = goals_center(&g);
            println!(
                "{:<10} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8}",
                scene.name(),
                n,
                lsm(&g, &goals, &h).1.passes,
                fim(&g, &goals, &h).1.passes,
                fsm(&g, &goals, &h).1.passes,
                gauss_seidel(&g, &goals, &h).1.passes,
                jacobi(&g, &goals, &h).1.passes,
            );
        }
    }
}

/// 3. Реальная стенка на секторе — то, из чего берётся бюджет тика.
///
/// Сектор пересчитывается **от граничных значений соседей**, а не от цели
/// внутри: именно так работает пересчёт грязного сектора. Буферы
/// переиспользуются, иначе замер меряет аллокатор.
fn bench_sector_wall() {
    section("3. Время на сектор при пересчёте от границы");
    let htab = HTable::new(255);
    let h = |f: u32, d: u32| htab.h(f, d);
    println!(
        "{:<10} {:>4} {:>10} {:>10} {:>9} {:>9} {:>12} {:>12}",
        "сцена", "n", "lsm мкс", "fim мкс", "lsm об/яч", "fim об/яч", "150 сект/7", "400 сект/7"
    );
    for scene in [Scene::Surface, Scene::Craters, Scene::Tunnels, Scene::Maze] {
        for n in [16usize, 32] {
            // Большая сцена, полное решение — источник граничных значений.
            let big = build(scene, n * 4, 7);
            let (tbig, _) = fmm(&big, &goals_center(&big), &h);

            // Сектор n x n плюс кольцо граничных условий вокруг него;
            // рамка самой сетки остаётся непроходимой.
            let (ox, oy) = (big.w / 2 - n / 2 - 2, big.h / 2 - n / 2 - 2);
            let mut g = Grid::new(n + 2, n + 2, 10);
            for y in 1..g.h - 1 {
                for x in 1..g.w - 1 {
                    g.cost[y * g.w + x] = big.cost[(oy + y) * big.w + (ox + x)];
                }
            }
            // Граничные условия — внутреннее кольцо, значения из полного решения.
            let mut seeds: Vec<(usize, u32)> = Vec::new();
            for y in 1..g.h - 1 {
                for x in 1..g.w - 1 {
                    if !(x == 1 || y == 1 || x == g.w - 2 || y == g.h - 2) {
                        continue;
                    }
                    let v = tbig[(oy + y) * big.w + (ox + x)];
                    if v != INF && g.cost[y * g.w + x] != BLOCKED {
                        seeds.push((y * g.w + x, v));
                    }
                }
            }
            if seeds.is_empty() {
                println!("{:<10} {:>4}   (сектор недостижим)", scene.name(), n);
                continue;
            }

            let mut ws = Workspace::new(g.w * g.h);
            let reps = 20_000;
            let mut best = [f64::MAX; 2];
            let mut upc = [0.0f64; 2];
            for _ in 0..5 {
                let t0 = Instant::now();
                for _ in 0..reps {
                    black_box(lsm_ws(&mut ws, &g, &seeds, &h).updates);
                }
                best[0] = best[0].min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
                upc[0] = lsm_ws(&mut ws, &g, &seeds, &h).per_cell();

                let t0 = Instant::now();
                for _ in 0..reps {
                    black_box(fim_ws(&mut ws, &g, &seeds, &h).updates);
                }
                best[1] = best[1].min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
                upc[1] = fim_ws(&mut ws, &g, &seeds, &h).per_cell();
            }
            let fast = best[0].min(best[1]);
            println!(
                "{:<10} {:>4} {:>10.3} {:>10.3} {:>9.2} {:>9.2} {:>12.3} {:>12.3}",
                scene.name(),
                n,
                best[0],
                best[1],
                upc[0],
                upc[1],
                fast * 150.0 / 7.0 / 1000.0,
                fast * 400.0 / 7.0 / 1000.0,
            );
        }
    }
    println!("(последние две колонки — мс на тик при 150 и 400 грязных секторах на 7 воркерах, по лучшему решателю)");
}

/// 4. Разрядность поля значений.
fn bench_bit_width() {
    section("4. Разрядность поля значений");
    let h = h_exact_floor;
    println!(
        "{:<10} {:>5} {:>10} {:>7} {:>12} {:>10}",
        "сцена", "n", "max T", "бит", "ничьих, %", "стоимости"
    );
    for scene in Scene::ALL {
        for n in [64usize, 256] {
            let g = build(scene, n, 7);
            let goals = goals_center(&g);
            let (t, _) = fmm(&g, &goals, &h);
            let m = max_finite(&t);
            let (ties, total) = tie_fraction(&g, &t);
            let cmin = g.cost.iter().copied().filter(|&c| c != BLOCKED).min().unwrap_or(0);
            let cmax = g.cost.iter().copied().max().unwrap_or(0);
            println!(
                "{:<10} {:>5} {:>10} {:>7} {:>11.1}% {:>10}",
                scene.name(),
                n,
                m,
                bits(m),
                100.0 * ties as f64 / total.max(1) as f64,
                format!("{cmin}..{cmax}")
            );
        }
    }
    println!("\nчто нужно полю, если оно глобальное (значение растёт с длиной пути):");
    for (cost, label) in [
        (10u32, "ровный рельеф, стоимость 10"),
        (100, "болото, стоимость 100"),
        (255, "потолок 8-битной стоимости"),
    ] {
        for span in [2000u32, 4000] {
            let worst = span * cost;
            println!("  {label:<32} карта {span} м: max T = {worst:>9}, {} бит", bits(worst));
        }
    }
    println!("\nчто нужно полю, если оно секторное (значения хранятся от минимума сектора):");
    for sector in [16u32, 32] {
        for cost in [10u32, 100, 255] {
            // Диаметр пути внутри сектора в худшем случае — змейка по всем ячейкам.
            let worst_diag = sector * sector;
            let worst = worst_diag * cost;
            println!(
                "  сектор {sector:>2}, стоимость {cost:>3}: типично {:>7} ({} бит), змейка через весь сектор {:>7} ({} бит)",
                3 * sector * cost,
                bits(3 * sector * cost),
                worst,
                bits(worst)
            );
        }
    }
}

/// 5. Q-61: сколько бит нужно, чтобы **направление** оставалось устойчивым.
fn bench_direction_stability() {
    section("5. Устойчивость направления против разрядности (Q-61)");
    let h = h_exact_floor;
    println!("эталон — та же геометрия со стоимостями x128 (7 дополнительных бит).");
    println!("«иначе» — доля ячеек с другим направлением; «во вред» — доля, где выбранный");
    println!("сосед в эталоне строго хуже минимального, то есть это не ничья, а ошибка.\n");
    print!("{:<10} {:>5}", "сцена", "n");
    for lbl in ["как есть", "x2", "x4", "x16", "x64"] {
        print!(" {:>18}", lbl);
    }
    println!();
    for scene in [Scene::Surface, Scene::Craters, Scene::Road, Scene::Tunnels, Scene::Maze] {
        for n in [64usize, 128] {
            let g = build(scene, n, 7);
            let goals = goals_center(&g);
            let gref = scale_costs(&g, 128);
            let (tref, _) = fmm(&gref, &goals, &h);
            print!("{:<10} {:>5}", scene.name(), n);
            for s in [1u16, 2, 4, 16, 64] {
                let gs = scale_costs(&g, s);
                let (ts, _) = fmm(&gs, &goals, &h);
                let (differs, harmful) = dir_quality(&g, &tref, &ts);
                print!(" {:>8.2}/{:<9.2}", differs, harmful);
            }
            println!();
        }
    }
    println!("(в каждой паре: слева «иначе», справа «во вред», в процентах)");
}

/// 6. Смещение вдоль пути: флор против округления к ближайшему и к чётному.
fn bench_rounding_bias() {
    section("6. Смещение вдоль пути: режим округления против разрядности");
    println!("отклонение max T от почти точного решения (стоимости x128), в процентах длины пути.\n");
    println!(
        "{:<10} {:>5} {:>7} {:>10} {:>10} {:>10}",
        "сцена", "n", "масштаб", "флор", "к ближ.", "к чётн."
    );
    for scene in [Scene::Open, Scene::Surface, Scene::Maze, Scene::Road] {
        for n in [128usize] {
            let g = build(scene, n, 7);
            let goals = goals_center(&g);
            let gref = scale_costs(&g, 128);
            let (tr, _) = fmm(&gref, &goals, &h_exact_floor);
            let truth = max_finite(&tr) as f64 / 128.0;
            for s in [1u16, 4, 16] {
                let gs = scale_costs(&g, s);
                let sf = s as f64;
                let a = max_finite(&fmm(&gs, &goals, &h_exact_floor).0) as f64 / sf;
                let b = max_finite(&fmm(&gs, &goals, &h_exact_round).0) as f64 / sf;
                let c = max_finite(&fmm(&gs, &goals, &h_exact_round_even).0) as f64 / sf;
                println!(
                    "{:<10} {:>5} {:>7} {:>9.3}% {:>9.3}% {:>9.3}%",
                    scene.name(),
                    n,
                    format!("x{s}"),
                    100.0 * (a - truth) / truth,
                    100.0 * (b - truth) / truth,
                    100.0 * (c - truth) / truth,
                );
            }
        }
    }
}


/// 7. Цена дробных бит. Дробные биты у стоимости чинят направление (раздел 5),
/// но выбивают стоимость из диапазона таблицы, и обновление возвращается
/// к целочисленному корню. Это надо посчитать, а не выбирать на глаз.
fn bench_fractional_bits_price() {
    section("7. Цена дробных бит: таблица против корня, на секторе");
    let htab = HTable::new(255);
    println!(
        "{:<10} {:>4} {:>16} {:>16} {:>16} {:>10}",
        "сцена", "n", "x1 таблица мкс", "x16 корень мкс", "4+4 таблица мкс", ""
    );
    for scene in [Scene::Surface, Scene::Craters, Scene::Tunnels] {
        for n in [16usize, 32] {
            let big = build(scene, n * 4, 7);
            let (tbig, _) = fmm(&big, &goals_center(&big), &h_exact_floor);
            let (ox, oy) = (big.w / 2 - n / 2 - 2, big.h / 2 - n / 2 - 2);
            let mut g = Grid::new(n + 2, n + 2, 10);
            for y in 1..g.h - 1 {
                for x in 1..g.w - 1 {
                    g.cost[y * g.w + x] = big.cost[(oy + y) * big.w + (ox + x)];
                }
            }
            let mut seeds: Vec<(usize, u32)> = Vec::new();
            for y in 1..g.h - 1 {
                for x in 1..g.w - 1 {
                    if !(x == 1 || y == 1 || x == g.w - 2 || y == g.h - 2) {
                        continue;
                    }
                    let v = tbig[(oy + y) * big.w + (ox + x)];
                    if v != INF && g.cost[y * g.w + x] != BLOCKED {
                        seeds.push((y * g.w + x, v));
                    }
                }
            }
            if seeds.is_empty() {
                continue;
            }
            let g16 = scale_costs(&g, 16);
            let seeds16: Vec<(usize, u32)> = seeds.iter().map(|&(i, v)| (i, v * 16)).collect();

            let mut ws = Workspace::new(g.w * g.h);
            let reps = 20_000;
            let mut a = f64::MAX;
            let mut b = f64::MAX;
            for _ in 0..5 {
                let t0 = Instant::now();
                for _ in 0..reps {
                    black_box(fim_ws(&mut ws, &g, &seeds, &|f, d| htab.h(f, d)).updates);
                }
                a = a.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
                let t0 = Instant::now();
                for _ in 0..reps {
                    black_box(fim_ws(&mut ws, &g16, &seeds16, &h_exact_floor).updates);
                }
                b = b.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
            }
            // Третий вариант: стоимость зажата в 1..15 (4 бита целых),
            // потом x16 — тогда f <= 240 и таблица h(f,d) снова покрывает
            // диапазон. Это и есть предлагаемая раскладка.
            let mut gc = g.clone();
            for c in gc.cost.iter_mut() {
                if *c != BLOCKED {
                    *c = (*c / 2).clamp(1, 15) * 16;
                }
            }
            let seedsc: Vec<(usize, u32)> = seeds.iter().map(|&(i, v)| (i, v * 8)).collect();
            let mut c3 = f64::MAX;
            for _ in 0..5 {
                let t0 = Instant::now();
                for _ in 0..reps {
                    black_box(fim_ws(&mut ws, &gc, &seedsc, &|f, d| htab.h(f, d)).updates);
                }
                c3 = c3.min(t0.elapsed().as_secs_f64() * 1e6 / reps as f64);
            }
            let fits = true;
            let _ = fits;
            println!(
                "{:<10} {:>4} {:>16.3} {:>16.3} {:>16} {:>10}",
                scene.name(),
                n,
                a,
                b,
                format!("{c3:.3}"),
                ""
            );
        }
    }
    println!("(x16 = четыре дробных бита у стоимости; там, где стоимость больше 15,");
    println!(" таблица h(f,d) на 255 уже не покрывает диапазон и остаётся только корень)");
}

fn main() {
    bench_h_variants();
    bench_solvers();
    bench_sector_wall();
    bench_bit_width();
    bench_direction_stability();
    bench_rounding_bias();
    bench_fractional_bits_price();
}
