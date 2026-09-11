//! **D-7 «Масштаб»** — пять тысяч юнитов на изрытой карте
//! (`docs/14-demo-scenes.md`).
//!
//! ```text
//! d7_scale --seed 42 --units 5000 --warmup 2000 --ticks 200 --dump-hash
//! ```
//!
//! Первая сцена, где всё измеренное по частям работает **одновременно**:
//! юниты, копка, ремеш, связность и навигация в одном тике.
//!
//! **Порог:** тик ≤ 10 мс при пяти тысячах юнитов на изрытой карте,
//! и приказ «всем в одну точку» не выводит за бюджет.
//!
//! **Карта обязана быть предварительно изрытой, а копка — продолжаться.**
//! Замер на нетронутой карте измеряет не ту задачу (ADR-090): грязные
//! множества, число квадов и рабочее множество кэша у изрытой карты
//! другие.

use std::process::ExitCode;
use std::time::Instant;

use demo::dig::Dig;
use meshing::layer::MeshLayer;
use sim_core::ground::Ground;
use sim_core::Phase as SimPhase;
use demo::{RunHash, Terrain};
use fx::rng::Rng;
use meshing::Opts;
use sim_core::{populate, World as SimWorld, WORLD};
use voxel::World;

/// Бюджет тика, миллисекунд (30 Гц).
const BUDGET_MS: f64 = 10.0;

/// Пороги по подсистемам из `docs/10-budgets.md`. Порога на целое мало:
/// часть может уехать втрое, пока целое укладывается (ADR-146).
const NAV_BUDGET_MS: f64 = 3.0;
const MESH_BUDGET_MS: f64 = 5.0;

struct Args {
    seed: u64,
    units: usize,
    warmup: u64,
    ticks: u64,
    side: u32,
    diggers: usize,
    threads: usize,
    dense: bool,
    dump_hash: bool,
    /// Второй потолок дренажа: чанков из генератора за тик (ADR-114).
    mat_cap: usize,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            seed: 42,
            units: 5000,
            warmup: 2000,
            ticks: 200,
            side: 256,
            diggers: 300,
            threads: 1,
            mat_cap: 4,
            dense: false,
            dump_hash: false,
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
            "--units" => a.units = num("--units")? as usize,
            "--warmup" => a.warmup = num("--warmup")?,
            "--mat-cap" => a.mat_cap = num("--mat-cap")? as usize,
            "--ticks" => a.ticks = num("--ticks")?,
            "--side" => a.side = num("--side")? as u32,
            "--diggers" => a.diggers = num("--diggers")? as usize,
            "--threads" => a.threads = num("--threads")?.max(1) as usize,
            "--dense" => a.dense = true,
            "--dump-hash" => a.dump_hash = true,
            "--help" | "-h" => {
                println!(
                    "D-7 «Масштаб»\n\n  --seed N --units N --warmup N --ticks N --side N\n  \
                     --diggers N --threads N --dense --dump-hash\n"
                );
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
}

/// Все в одну точку: худший случай по плотности (ADR-077).
fn populate_dense(w: &mut SimWorld, n: usize, seed: u64) {
    let mut r = Rng::new(seed, 0xA1);
    for k in 0..n {
        let owner = (k % 2) as u8;
        let (ex, ez) = w.ground().map(|g| g.extent()).unwrap_or((WORLD, WORLD));
        let x = (ex / 2 + r.range(-40, 40)).clamp(0, ex - 1);
        let z = (ez / 2 + r.range(-40, 40)).clamp(0, ez - 1);
        w.spawn_ground(x, z, owner, r.range(2, 6), 300);
    }
    let (ex, ez) = w.ground().map(|g| g.extent()).unwrap_or((WORLD, WORLD));
    w.fill_flow_towards(0, ex / 2, ez / 2);
    w.fill_flow_towards(1, ex / 2, ez / 2);
    w.units.refresh_live();
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

    println!("D-7 «Масштаб» — всё вместе в одном тике");
    println!(
        "мир {}×{}×{}, {} юнитов, {} копателей, {} воркер(ов){}\n",
        params.nx,
        params.ny,
        params.nz,
        a.units,
        a.diggers,
        a.threads,
        if a.dense { ", ВСЕ В ОДНУ ТОЧКУ" } else { "" }
    );

    let t_gen = Instant::now();
    let mut world = World::new(
        params.nx,
        params.ny,
        params.nz,
        4096,
        Box::new(Terrain { params, seed: a.seed }),
    );
    world.max_materialize_per_tick = a.mat_cap;
    let gen_ms = t_gen.elapsed().as_secs_f64() * 1e3;

    let mut mesh = MeshLayer::new(&world, Opts::default());
    let mut dig = Dig::clustered(
        &world,
        |x, z| terrain.surface(x, z),
        a.diggers,
        2,
        8,
        a.seed ^ 0xD16,
    );

    // --- Юниты и земля в одном мире: порядок фаз задаёт движок
    // (ADR-125), а не сцена.
    let t = demo::tuning();
    let mut sim = SimWorld::new(2, a.units.max(1), t);
    sim.attach_ground(Ground::new(world, &t));
    if a.dense {
        populate_dense(&mut sim, a.units, a.seed);
    } else {
        populate(&mut sim, a.units, a.seed);
    }


    let mut hash = RunHash::new();

    /// Что намерила одна фаза замера.
    struct Phase {
        ms_units: f64,
        fall_damage: u64,
        max_fall: u32,
        ms_dig: f64,
        ms_mesh: f64,
        ms_topo: f64,
        edits: u64,
        dirty_mesh: u64,
        dirty_topo: u64,
        /// Худший тик среди тех, где НЕ было первых сборок чанка.
        /// Порог применяется к нему: пик первого касания это цена
        /// разогрева, а не постоянная нагрузка.
        worst: f64,
        /// Худший среди тиков с первыми сборками — цена разогрева.
        worst_cold: f64,
        /// В каком тике был худший — без этого пик не отличить
        /// от постоянной нагрузки.
        worst_at: u64,
        /// Сколько чанков собрано с нуля за фазу.
        first_builds: u64,
        /// Разбивка худшего тика по фазам — иначе пик не объяснить.
        worst_split: [f64; 4],
        /// Чанков слоя изменений материализовано из генератора.
        materialized: u64,
        /// Самая длинная очередь правок за фазу — это и есть отставание
        /// мира от забоя, и оно обязано быть видно.
        worst_queued: usize,
        /// В скольких тиках дренаж остановил не потолок правок, а
        /// потолок материализаций.
        mat_capped_ticks: u64,
        /// Материализаций в худшем тике.
        worst_materialized: u64,
    }

    // Время фаз движок ведёт сам; сцена берёт разницу за тик.
    fn phase_split(sim: &SimWorld) -> [u64; 3] {
        [
            sim.phase_ns_of(SimPhase::VoxelEdits)
                + sim.phase_ns_of(SimPhase::Destruction)
                + sim.phase_ns_of(SimPhase::Bodies),
            sim.phase_ns_of(SimPhase::NavUpdate),
            sim.phase_ns_of(SimPhase::Movement)
                + sim.phase_ns_of(SimPhase::Combat)
                + sim.phase_ns_of(SimPhase::Cleanup),
        ]
    }

    let measure = |sim: &mut SimWorld,
                       mesh: &mut MeshLayer,
                       dig: &mut Dig,
                       hash: &mut RunHash,
                       ticks: u64|
     -> Phase {
        let mut p = Phase {
            ms_units: 0.0,
            fall_damage: 0,
            max_fall: 0,
            ms_dig: 0.0,
            ms_mesh: 0.0,
            ms_topo: 0.0,
            edits: 0,
            dirty_mesh: 0,
            dirty_topo: 0,
            worst: 0.0,
            worst_cold: 0.0,
            worst_at: 0,
            first_builds: 0,
            materialized: 0,
            worst_materialized: 0,
            worst_queued: 0,
            mat_capped_ticks: 0,
            worst_split: [0.0; 4],
        };
        let mat0 = sim.ground().expect("земля").world.materialized;
        let rebuilds0 = mesh.full_rebuilds;
        for tick in 1..=ticks {
            let before_rebuilds = mesh.full_rebuilds;
            let before_mat = sim.ground().expect("земля").world.materialized;
            let ns0 = phase_split(sim);
            let t_all = Instant::now();

            {
                let g = sim.ground.as_mut().expect("земля");
                dig.step(&mut g.world, &|x, z| terrain.surface(x, z));
            }
            let h = if a.threads > 1 { sim.tick_par(a.threads) } else { sim.tick() };
            let ns1 = phase_split(sim);
            let s_dig = (ns1[0] - ns0[0]) as f64 / 1e6;
            let s_topo = (ns1[1] - ns0[1]) as f64 / 1e6;
            let s_units = (ns1[2] - ns0[2]) as f64 / 1e6;
            p.ms_dig += s_dig;
            p.ms_topo += s_topo;
            p.ms_units += s_units;

            let g = sim.ground.as_ref().expect("земля");
            p.edits += g.stats.edits as u64;
            p.fall_damage += g.stats.fall_damage;
            p.max_fall = p.max_fall.max(g.stats.max_fall);
            p.worst_queued = p.worst_queued.max(g.stats.queued);
            p.dirty_topo += g.stats.topo_dirty as u64;

            let t = Instant::now();
            p.dirty_mesh += mesh.apply(&g.world, g.edits()) as u64;
            let s_mesh = t.elapsed().as_secs_f64() * 1e3;
            p.ms_mesh += s_mesh;

            let ms = t_all.elapsed().as_secs_f64() * 1e3;
            if mesh.full_rebuilds > before_rebuilds {
                p.worst_cold = p.worst_cold.max(ms);
            } else if ms > p.worst {
                p.worst = ms;
                p.worst_at = tick;
                p.worst_split = [s_units, s_dig, s_mesh, s_topo];
                p.worst_materialized =
                    sim.ground().expect("земля").world.materialized - before_mat;
            }
            hash.feed(h);
            hash.feed_edits(tick, sim.ground().expect("земля").edits());
        }
        p.first_builds = mesh.full_rebuilds - rebuilds0;
        p.materialized = sim.ground().expect("земля").world.materialized - mat0;
        p
    };

    // --- Фаза A: свежая карта. Копка идёт по нетронутой породе.
    let fresh = measure(&mut sim, &mut mesh, &mut dig, &mut hash, a.ticks);

    // --- Прогрев: карту надо изрыть.
    let t_warm = Instant::now();
    let mut warm_edits = 0u64;
    for _ in 0..a.warmup {
        {
            let g = sim.ground.as_mut().expect("земля");
            dig.step(&mut g.world, &|x, z| terrain.surface(x, z));
        }
        sim.tick();
        let g = sim.ground.as_ref().expect("земля");
        warm_edits += g.stats.edits as u64;
        mesh.apply(&g.world, g.edits());
    }
    let warm_s = t_warm.elapsed().as_secs_f64();

    // --- Фаза B: выработанная карта, копка продолжается.
    let worn = measure(&mut sim, &mut mesh, &mut dig, &mut hash, a.ticks);

    println!(
        "прогрев между замерами: {} тиков, {} правок, {:.1} с; перезапусков забоя {}",
        a.warmup, warm_edits, warm_s, dig.restarts
    );
    println!(
        "(тридцать минут игры это {} тиков при 30 Гц — прогрев короче, и это оговорка, а не факт)\n",
        30 * 60 * 30
    );

    let n = a.ticks as f64;
    let tot = |p: &Phase| (p.ms_units + p.ms_dig + p.ms_mesh + p.ms_topo) / n;
    let (t_fresh, t_worn) = (tot(&fresh), tot(&worn));

    println!("{:<34} {:>14} {:>14}", "тик под нагрузкой, мс", "свежая карта", "выработанная");
    println!("{:<34} {:>14.3} {:>14.3}", "юниты", fresh.ms_units / n, worn.ms_units / n);
    println!("{:<34} {:>14.3} {:>14.3}", "копка и дренаж", fresh.ms_dig / n, worn.ms_dig / n);
    println!("{:<34} {:>14.3} {:>14.3}", "ремеш", fresh.ms_mesh / n, worn.ms_mesh / n);
    println!("{:<34} {:>14.3} {:>14.3}", "связность", fresh.ms_topo / n, worn.ms_topo / n);
    println!("{:<34} {:>14.3} {:>14.3}", "итого средний", t_fresh, t_worn);
    println!(
        "{:<34} {:>14.3} {:>14.3}",
        "худший тик без первых сборок", fresh.worst, worn.worst
    );
    println!(
        "{:<34} {:>14.3} {:>14.3}",
        "худший тик с первыми сборками", fresh.worst_cold, worn.worst_cold
    );
    println!(
        "{:<34} {:>14} {:>14}",
        "  он же — номер тика", fresh.worst_at, worn.worst_at
    );
    println!(
        "{:<34} {:>14} {:>14}",
        "чанков собрано с нуля", fresh.first_builds, worn.first_builds
    );
    println!(
        "{:<34} {:>14} {:>14}",
        "чанков мира материализовано", fresh.materialized, worn.materialized
    );
    println!(
        "{:<34} {:>14} {:>14}",
        "  из них в худшем тике", fresh.worst_materialized, worn.worst_materialized
    );
    println!(
        "{:<34} {:>14} {:>14}",
        "самая длинная очередь правок", fresh.worst_queued, worn.worst_queued
    );
    println!(
        "{:<34} {:>14} {:>14}",
        "  тиков упёрлось в материализацию", fresh.mat_capped_ticks, worn.mat_capped_ticks
    );
    for (k, name) in ["  из них юниты", "  копка", "  ремеш", "  связность"]
        .iter()
        .enumerate()
    {
        println!(
            "{:<34} {:>14.3} {:>14.3}",
            name, fresh.worst_split[k], worn.worst_split[k]
        );
    }

    println!("\n{:<34} {:>14} {:>14}", "нагрузка за тик", "свежая", "выработанная");
    println!(
        "{:<34} {:>14.1} {:>14.1}",
        "правок",
        fresh.edits as f64 / n,
        worn.edits as f64 / n
    );
    println!(
        "{:<34} {:>14.1} {:>14.1}",
        "грязных чанков мешинга",
        fresh.dirty_mesh as f64 / n,
        worn.dirty_mesh as f64 / n
    );
    println!(
        "{:<34} {:>14.1} {:>14.1}",
        "грязных чанков связности",
        fresh.dirty_topo as f64 / n,
        worn.dirty_topo as f64 / n
    );

    println!("\n{:<34} {:>14}", "прочее", "");
    println!("{:<34} {:>14}", "живых юнитов", sim.units.live().len());
    println!(
        "{:<34} {:>14.1}",
        "осмотрено на движении",
        sim.examined_move as f64 / (2.0 * n)
    );
    println!(
        "{:<34} {:>14.1}",
        "осмотрено на бою",
        sim.examined_combat as f64 / (2.0 * n)
    );
    // Ноль здесь значит, что юниты по рельефу не ходят (ADR-134).
    // Поле целиком — то, чего тик не выдержит: потому оно и считается
    // порциями (ADR-138). Число печатается, чтобы порция не выглядела
    // осторожностью на пустом месте.
    {
        let t = Instant::now();
        sim.solve_flow();
        println!("{:<34} {:>11.3} мс", "поле направлений целиком", t.elapsed().as_secs_f64() * 1e3);
    }
    // Падает ли здесь вообще кто-нибудь: копатели снимают породу
    // из-под своих же, и юнит на исчезнувшей опоре обязан лететь вниз
    // (ADR-036, ADR-139). Ноль тут значил бы, что сцена этой механики
    // не грузит.
    println!(
        "{:<34} {:>14}",
        "урон от падения за прогон",
        fresh.fall_damage + worn.fall_damage
    );
    println!(
        "{:<34} {:>14}",
        "  самое долгое падение, вокселей",
        fresh.max_fall.max(worn.max_fall)
    );
    println!("{:<34} {:>14}", "решений поля направлений", sim.flow_solves);
    println!("{:<34} {:>14}", "  узлов «клетка и уровень»", sim.flow_nodes);
    println!("{:<34} {:>14}", "  рёбер", sim.flow_edges);
    println!("{:<34} {:>14}", "  компонент связности", sim.flow_components);
    // Достижимо считается по обоим владельцам, поэтому вдвое больше
    // узлов. Меньше — значит часть карты приказа не получила,
    // и фаза движения там пустая (ADR-138).
    println!("{:<34} {:>14}", "  узлов достижимо", sim.flow_reachable);
    println!("{:<34} {:>14}", "  годуновских обновлений", sim.flow_updates);
    println!("{:<34} {:>14}", "  тиков на одно поле", sim.flow_ticks);
    println!("{:<34} {:>14}", "  секторов пересчитано", sim.flow_solved_sectors);
    println!("{:<34} {:>14}", "  секторов взято готовыми", sim.flow_reused_sectors);
    println!(
        "{:<34} {:>14.1}",
        "шагов упёрлось в породу",
        sim.blocked_moves as f64 / (2.0 * n)
    );
    println!("{:<34} {:>11.1} мс", "генерация мира", gen_ms);
    println!(
        "{:<34} {:>11.1} МиБ",
        "слой изменений",
        sim.ground().expect("земля").world.diff_bytes() as f64 / 1024.0 / 1024.0
    );
    println!(
        "{:<34} {:>11.1} МиБ",
        "битовый объём",
        sim.ground().expect("земля").world.bit_bytes() as f64 / 1024.0 / 1024.0
    );
    println!(
        "{:<34} {:>11.1} МиБ",
        "состояние мешера",
        mesh.state_bytes() as f64 / 1024.0 / 1024.0
    );
    println!("{:<34} {:>14}", "квадов всего", mesh.quad_total());

    println!("\nхеш прогона: {:016x}", hash.get());
    if a.dump_hash {
        println!(
            "seed={} units={} ticks={} hash={:016x}",
            a.seed,
            a.units,
            a.ticks,
            hash.get()
        );
    }

    // Порог берётся по ХУДШЕЙ из двух точек.
    let worst_avg = t_fresh.max(t_worn);
    let worst_tick = fresh.worst.max(worn.worst);
    let worst_cold = fresh.worst_cold.max(worn.worst_cold);
    let verdict = if worst_avg <= BUDGET_MS { "ДЕРЖИТСЯ" } else { "ПРЕВЫШЕН" };
    println!("порог {BUDGET_MS} мс на тик: худший средний {worst_avg:.3} мс — {verdict}");
    println!("худший отдельный тик без первых сборок: {worst_tick:.3} мс");
    println!(
        "цена разогрева: худший тик с первыми сборками {worst_cold:.3} мс,          {} чанков собрано с нуля",
        fresh.first_builds + worn.first_builds
    );

    // Каждая подсистема отвечает за свою строку бюджета отдельно.
    let nav_ms = (fresh.ms_topo / n).max(worn.ms_topo / n);
    let mesh_ms = (fresh.ms_mesh / n).max(worn.ms_mesh / n);
    for (name, got, budget) in [
        ("связность и проходимость", nav_ms, NAV_BUDGET_MS),
        ("ремеш", mesh_ms, MESH_BUDGET_MS),
    ] {
        let v = if got <= budget { "держится" } else { "ПРЕВЫШЕН" };
        println!("  порог {budget} мс на «{name}»: {got:.3} мс — {v}");
    }

    let mut ok = worst_avg <= BUDGET_MS
        && worst_tick <= BUDGET_MS * 2.0
        && nav_ms <= NAV_BUDGET_MS
        && mesh_ms <= MESH_BUDGET_MS;
    if fresh.edits == 0 || worn.edits == 0 {
        println!("копка не идёт — сцена мерит не ту задачу");
        ok = false;
    }
    // Затухание копки — не ошибка, но его надо видеть.
    let decay = fresh.edits as f64 / worn.edits.max(1) as f64;
    println!(
        "копка на выработанной карте слабее свежей в {decay:.1} раза          ({:.0} против {:.0} правок за тик)",
        fresh.edits as f64 / n,
        worn.edits as f64 / n
    );

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
