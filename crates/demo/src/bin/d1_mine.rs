//! **D-1 «Шахта»** — копка как экономика (`docs/14-demo-scenes.md`).
//!
//! ```text
//! d1_mine --seed 42 --ticks 600 --diggers 300 --dump-hash
//! ```
//!
//! Показывает постоянную нагрузку на ремеш, связность и навигацию
//! **одновременно**, органическим забоем с рваными стенами (ADR-098).
//!
//! **Порог:** мешинг, связность и навигация вместе ≤ 3 мс на тик
//! при трёхстах копателях.
//!
//! **Ловушка, которую сцена обязана поймать:** швы между чанками.
//! Ремеш инкрементальный, и неверно обновлённый паддинг соседа даёт
//! лишние квады на границе — видно не сразу, поэтому проверяется
//! сверкой с полной пересборкой, а не глазами.

use std::process::ExitCode;
use std::time::Instant;

use demo::dig::Dig;
use meshing::layer::MeshLayer;
use sim_core::ground::Ground;
use sim_core::{Phase, World as SimWorld};
use demo::{RunHash, Terrain};
use meshing::Opts;
use voxel::World;

/// Порог сцены: три подсистемы вместе, миллисекунд на тик.
/// Бюджет тика (`docs/10-budgets.md`).
///
/// Было три: сцена мерила только ремеш, связность и навигацию. Теперь
/// она гоняет **весь тик земли** — дренаж очереди, каскад отделения,
/// падение тел, связность, проходимость, — то есть ровно то, что мерит
/// бюджет тика (ADR-126).
const BUDGET_MS: f64 = 10.0;

struct Args {
    seed: u64,
    ticks: u64,
    diggers: usize,
    clusters: usize,
    side: u32,
    dump_hash: bool,
    verify_seams: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            seed: 42,
            ticks: 600,
            diggers: 300,
            clusters: 8,
            side: 256,
            dump_hash: false,
            verify_seams: true,
        }
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
            "--ticks" => a.ticks = num("--ticks")?,
            "--diggers" => a.diggers = num("--diggers")? as usize,
            "--clusters" => a.clusters = num("--clusters")? as usize,
            "--side" => a.side = num("--side")? as u32,
            "--dump-hash" => a.dump_hash = true,
            "--no-verify" => a.verify_seams = false,
            "--help" | "-h" => {
                println!(
                    "D-1 «Шахта»\n\n  --seed N --ticks N --diggers N --clusters N --side N\n  \
                     --dump-hash   напечатать хеш прогона\n  --no-verify   не сверять швы\n"
                );
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
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
        ny: a.side / 2,
        nz: a.side,
        ..Default::default()
    };
    let terrain = Terrain { params, seed: a.seed };

    let t0 = Instant::now();
    // Потолок правок за тик (ADR-013, ADR-045). Триста копателей дают
    // порядка 1 300 правок; потолок держится выше, чтобы сцена мерила
    // подсистемы, а не очередь.
    let world = World::new(
        params.nx,
        params.ny,
        params.nz,
        4096,
        Box::new(Terrain { params, seed: a.seed }),
    );
    let gen_ms = t0.elapsed().as_secs_f64() * 1e3;

    let mut mesh = MeshLayer::new(&world, Opts::default());
    let mut dig = Dig::clustered(
        &world,
        |x, z| terrain.surface(x, z),
        a.diggers,
        2,
        a.clusters,
        a.seed ^ 0xD16,
    );

    // Сцена больше не собирает порядок фаз сама: земля живёт в движке,
    // и `sim.tick()` прогоняет дренаж, каскад, тела, связность
    // и проходимость в объявленном порядке (ADR-125). Рост юнита четыре
    // воксела (2 м при вокселе 0.5 м), нав-клетка два воксела
    // по стороне (ADR-078).
    let mut sim = SimWorld::new(1, 1);
    sim.attach_ground(Ground::new(world, 64, 4, 2));

    let mut hash = RunHash::new();
    let mut ms_mesh = 0.0f64;
    let (mut cas_relabel, mut cas_faces, mut cas_rounds) = (0u64, 0u64, 0u64);
    let (mut edits_total, mut dirty_mesh, mut dirty_topo, mut survived) = (0u64, 0u64, 0u64, 0u64);
    let mut queued_peak = 0usize;
    let (mut nav_flips, mut nav_seen) = (0u64, 0u64);
    // Перепись уровней до копки: надо различить пещеры генератора
    // и ходы, прокопанные за прогон.
    let census0 = {
        let g = sim.ground().expect("земля");
        g.nav.level_census(&g.world)
    };
    let (mut nav_moved, mut nav_drop) = (0u64, 0u64);
    let (mut nav_lost, mut nav_gained) = (0u64, 0u64);

    for tick in 1..=a.ticks {
        {
            let g = sim.ground.as_mut().expect("земля");
            dig.step(&mut g.world, &|x, z| terrain.surface(x, z));
        }
        // Тик движка: дренаж, каскад, тела, связность, проходимость.
        sim.tick();

        let g = sim.ground.as_ref().expect("земля");
        edits_total += g.stats.edits as u64;
        queued_peak = queued_peak.max(g.stats.queued);
        dirty_topo += g.stats.topo_dirty as u64;
        survived += g.stats.topo_survived as u64;
        // Навигация мерится **высотой пола и рёбрами**, а не булевой
        // проходимостью: над поверхностью всегда небо, поэтому она
        // не переворачивается почти никогда, и сцена, мерившая только
        // её, показывала ровный ноль (ADR-117).
        let ns = g.stats.nav;
        nav_seen += ns.seen as u64;
        nav_flips += ns.flips as u64;
        nav_moved += ns.floor_moved as u64;
        nav_drop += ns.floor_drop;
        nav_lost += ns.edges_lost;
        nav_gained += ns.edges_gained;
        cas_relabel += g.stats.collapse.relabelled as u64;
        cas_faces += g.stats.collapse.faces as u64;
        cas_rounds += g.stats.collapse.rounds as u64;
        hash.feed_edits(tick, g.edits());

        // Мешинг — работа кадра, а не тика: его в списке фаз нет.
        let t = Instant::now();
        dirty_mesh += mesh.apply(&g.world, g.edits()) as u64;
        ms_mesh += t.elapsed().as_secs_f64() * 1e3;
    }
    // Разбивку по фазам ведёт движок: сцена её больше не собирает.
    let ph = |p: Phase| sim.phase_ns_of(p) as f64 / 1e6;
    let ms_drain = ph(Phase::VoxelEdits) + ph(Phase::Destruction) + ph(Phase::Bodies);
    let ms_topo = ph(Phase::NavUpdate);

    let n = a.ticks as f64;
    let total = (ms_mesh + ms_topo + ms_drain) / n;
    hash.feed(edits_total);
    hash.feed(mesh.quad_total() as u64);

    println!("D-1 «Шахта» — копка как экономика");
    println!(
        "мир {}×{}×{} вокселей ({} м при вокселе 0.5 м), генерация {gen_ms:.0} мс",
        params.nx,
        params.ny,
        params.nz,
        params.nx / 2
    );
    println!(
        "{} копателей в {} забоях, {} тиков, забой органический\n",
        a.diggers, a.clusters, a.ticks
    );

    println!("{:<28} {:>14}", "за тик", "значение");
    println!("{:<28} {:>14.1}", "правок применено", edits_total as f64 / n);
    println!("{:<28} {:>14}", "пик очереди", queued_peak);
    println!("{:<28} {:>14.1}", "грязных чанков мешинга", dirty_mesh as f64 / n);
    println!("{:<28} {:>14.1}", "грязных чанков связности", dirty_topo as f64 / n);
    println!(
        "{:<28} {:>13.2}%",
        "правок выжило после A",
        100.0 * survived as f64 / edits_total.max(1) as f64
    );
    println!("{:<28} {:>14.1}", "нав-клеток пересчитано", nav_seen as f64 / n);
    println!("{:<28} {:>14.1}", "  у них сдвинулся пол", nav_moved as f64 / n);
    println!("{:<28} {:>14.2}", "  пол опустился, вокселей", nav_drop as f64 / n);
    println!("{:<28} {:>14.1}", "  стало нельзя стоять", nav_flips as f64 / n);
    let (cells_with, cells_multi, levels) = {
        let g = sim.ground().expect("земля");
        g.nav.level_census(&g.world)
    };
    println!("{:<28} {:>14.1}", "рёбер разорвано", nav_lost as f64 / n);
    println!("{:<28} {:>14.1}", "рёбер появилось", nav_gained as f64 / n);
    // Насколько неверна одноуровневая навигация на этой карте.
    println!();
    println!("{:<28} {:>14} {:>14}", "уровни в колонке", "до копки", "после");
    println!("{:<28} {:>14} {:>14}", "клеток с полом", census0.0, cells_with);
    println!(
        "{:<28} {:>13.1}% {:>13.1}%",
        "  из них многоуровневых",
        100.0 * census0.1 as f64 / census0.0.max(1) as f64,
        100.0 * cells_multi as f64 / cells_with.max(1) as f64
    );
    println!("{:<28} {:>14} {:>14}", "уровней в подколонках", census0.2, levels);

    println!("\n{:<28} {:>14}", "время, мс на тик", "");
    println!("{:<28} {:>14.3}", "дренаж очереди", ph(Phase::VoxelEdits) / n);
    println!("{:<28} {:>14.3}", "каскад отделения", ph(Phase::Destruction) / n);
    {
        let c = &sim.ground().expect("земля").stats.collapse;
        println!("{:<28} {:>14.1}", "  чанков размечено", cas_relabel as f64 / n);
        println!("{:<28} {:>14.1}", "  граней пересчитано", cas_faces as f64 / n);
        println!("{:<28} {:>14}", "  собрано составов", c.gathered);
        println!("{:<28} {:>14}", "  узлов в графе", c.nodes);
        println!("{:<28} {:>14}", "  рёбер в графе", c.edges);
        println!("{:<28} {:>14}", "  не влезло в тело", c.too_big);
        println!("{:<28} {:>14}", "  чанков втянуто", c.promoted);
        println!("{:<28} {:>14}", "  ждёт втягивания", c.frontier);
        println!("{:<28} {:>14.2}", "  раундов", cas_rounds as f64 / n);
        println!("{:<28} {:>14.2}", "  мкс на разметку", 1000.0 * ph(Phase::Destruction) / cas_relabel.max(1) as f64);
    }
    println!("{:<28} {:>14.3}", "падение тел", ph(Phase::Bodies) / n);
    println!("{:<28} {:>14.3}", "связность и проходимость", ms_topo / n);
    println!("{:<28} {:>14.3}", "мешинг", ms_mesh / n);
    println!("{:<28} {:>14.3}", "итого", total);

    println!("\n{:<28} {:>14}", "геометрия и память", "");
    println!("{:<28} {:>14}", "квадов всего", mesh.quad_total());
    println!("{:<28} {:>14}", "живых чанков мешинга", mesh.live_chunks());
    println!(
        "{:<28} {:>11.1} МиБ",
        "состояние мешера",
        mesh.state_bytes() as f64 / 1024.0 / 1024.0
    );
    println!(
        "{:<28} {:>11.1} МиБ",
        "слой изменений мира",
        sim.ground().expect("земля").world.diff_bytes() as f64 / 1024.0 / 1024.0
    );
    println!(
        "{:<28} {:>11.1} МиБ",
        "битовый объём",
        sim.ground().expect("земля").world.bit_bytes() as f64 / 1024.0 / 1024.0
    );

    let mut ok = true;

    if a.verify_seams {
        // Швы: инкрементальный ремеш обязан дать то же число квадов,
        // что полная пересборка. Иначе паддинг соседа обновлён неверно.
        let vw = &sim.ground().expect("земля").world;
        let live = mesh.live_indices();
        let mut bad = 0usize;
        for &i in live.iter() {
            if mesh.rebuilt_quad_count(vw, i) != quad_of(&mesh, i) {
                bad += 1;
            }
        }
        println!(
            "\nшвы: сверено {} чанков, расхождений {}",
            live.len(),
            bad
        );
        if bad != 0 {
            ok = false;
        }
    }

    println!("\nхеш прогона: {:016x}", hash.get());
    if a.dump_hash {
        println!("seed={} ticks={} hash={:016x}", a.seed, a.ticks, hash.get());
    }

    let verdict = if total <= BUDGET_MS { "ПОРОГ ДЕРЖИТСЯ" } else { "ПОРОГ ПРЕВЫШЕН" };
    println!("порог {BUDGET_MS} мс на тик: {total:.3} мс — {verdict}");
    if total > BUDGET_MS {
        ok = false;
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Число квадов чанка по индексу — через публичный обход слоя.
fn quad_of(m: &MeshLayer, i: usize) -> usize {
    m.quad_at(i)
}
