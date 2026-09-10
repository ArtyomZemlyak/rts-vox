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
use demo::layers::{MeshLayer, NavLayer, TopoLayer};
use demo::{RunHash, Terrain};
use meshing::Opts;
use voxel::World;

/// Порог сцены: три подсистемы вместе, миллисекунд на тик.
const BUDGET_MS: f64 = 3.0;

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
    let mut world = World::new(
        params.nx,
        params.ny,
        params.nz,
        4096,
        Box::new(Terrain { params, seed: a.seed }),
    );
    let gen_ms = t0.elapsed().as_secs_f64() * 1e3;

    let mut mesh = MeshLayer::new(&world, Opts::default());
    let mut topo = TopoLayer::new(&world);
    // Рост юнита четыре воксела (2 м при вокселе 0.5 м), нав-клетка
    // два воксела по стороне (ADR-078).
    let mut nav = NavLayer::new(&world, 4, 2);
    let mut dig = Dig::clustered(
        &world,
        |x, z| terrain.surface(x, z),
        a.diggers,
        2,
        a.clusters,
        a.seed ^ 0xD16,
    );

    let mut hash = RunHash::new();
    let (mut ms_mesh, mut ms_topo, mut ms_nav) = (0.0f64, 0.0f64, 0.0f64);
    let (mut edits_total, mut dirty_mesh, mut dirty_topo, mut survived) = (0u64, 0u64, 0u64, 0u64);
    let mut queued_peak = 0usize;
    let (mut nav_flips, mut nav_seen) = (0u64, 0u64);

    for tick in 1..=a.ticks {
        dig.step(&mut world);
        let st = world.flush();
        edits_total += st.applied as u64;
        queued_peak = queued_peak.max(st.queued);
        hash.feed_edits(tick, world.edits());

        // Мешинг — работа кадра.
        let t = Instant::now();
        dirty_mesh += mesh.apply(&world, world.edits()) as u64;
        ms_mesh += t.elapsed().as_secs_f64() * 1e3;

        // Связность — работа тика.
        let t = Instant::now();
        let (d, s) = topo.apply(&world, world.edits());
        dirty_topo += d as u64;
        survived += s as u64;
        ms_topo += t.elapsed().as_secs_f64() * 1e3;

        // Навигация: перевороты непрозрачности нав-клетки. Клетка два
        // воксела, непрозрачность решается на уровне глаз, поэтому
        // переворачивается около процента правок (ADR-078).
        let t = Instant::now();
        let (seen, flips) = nav.apply(&world, world.edits());
        nav_seen += seen as u64;
        nav_flips += flips as u64;
        ms_nav += t.elapsed().as_secs_f64() * 1e3;
    }

    let n = a.ticks as f64;
    let total = (ms_mesh + ms_topo + ms_nav) / n;
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
    println!("{:<28} {:>14.1}", "из них перевернулось", nav_flips as f64 / n);

    println!("\n{:<28} {:>14}", "время, мс на тик", "");
    println!("{:<28} {:>14.3}", "мешинг", ms_mesh / n);
    println!("{:<28} {:>14.3}", "связность", ms_topo / n);
    println!("{:<28} {:>14.3}", "навигация", ms_nav / n);
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
        world.diff_bytes() as f64 / 1024.0 / 1024.0
    );
    println!(
        "{:<28} {:>11.1} МиБ",
        "битовый объём",
        world.bit_bytes() as f64 / 1024.0 / 1024.0
    );

    let mut ok = true;

    if a.verify_seams {
        // Швы: инкрементальный ремеш обязан дать то же число квадов,
        // что полная пересборка. Иначе паддинг соседа обновлён неверно.
        let live = mesh.live_indices();
        let mut bad = 0usize;
        for &i in live.iter() {
            if mesh.rebuilt_quad_count(&world, i) != quad_of(&mesh, i) {
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
