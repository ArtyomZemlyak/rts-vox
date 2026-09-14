//! Замер: **сколько из отсечённых пирамидой чанков вообще имеют
//! геометрию**.
//!
//! ADR-142 оставил отсечение по перекрытию открытым с оговоркой «сначала
//! замер, а мерить пока нечем». Мерить теперь есть чем: у сгенерированного
//! мира можно спросить число квадов в каждом чанке, а у камеры — какие
//! чанки в обзоре.
//!
//! Вопрос не в перекрытии, а в том, **насколько пирамида завышает
//! работу**. RTS-камера смотрит сверху под углом, и её пирамида
//! протыкает мир насквозь: над рельефом воздух, под ним сплошная
//! порода, и ни то ни другое не даёт ни одного квада.

use meshing::layer::MeshLayer;
use meshing::{Opts, CS};
use render::{Camera, ChunkGrid, V3};
use std::process::ExitCode;
use std::time::Instant;
use voxel::World;
use worldgen::Terrain;

struct Args {
    side: u32,
    height: u32,
    seed: u64,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        side: 384,
        height: 256,
        seed: 42,
    };
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
            "--seed" => a.seed = num("--seed")?,
            "--help" => {
                println!("bench_cull_world [--side N] [--height N] [--seed N]");
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ {other}")),
        }
    }
    Ok(a)
}

fn cam(target: V3, distance: f32, yaw: f32, far: f32) -> Camera {
    Camera {
        target,
        yaw,
        pitch: 0.95,
        distance,
        fovy: 0.87,
        aspect: 16.0 / 9.0,
        near: 1.0,
        far,
    }
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
    let terrain = Terrain {
        params,
        seed: a.seed,
    };
    let world = World::new(
        params.nx,
        params.ny,
        params.nz,
        1 << 20,
        Box::new(Terrain {
            params,
            seed: a.seed,
        }),
    );
    let mesh = MeshLayer::new(&world, Opts::default());
    let g = ChunkGrid {
        nx: a.side.div_ceil(CS as u32),
        ny: a.height.div_ceil(CS as u32),
        nz: a.side.div_ceil(CS as u32),
        cs: CS as u32,
    };

    println!(
        "отсечение против геометрии — мир {}×{}×{}",
        a.side, a.height, a.side
    );
    println!("чанк мешинга {CS}³, всего {} чанков\n", g.len());

    // Квады каждого чанка. Дорого — чанк собирается с нуля, — но это
    // эталон, а не рабочий путь.
    let t = Instant::now();
    let quads: Vec<usize> = (0..g.len())
        .map(|i| mesh.rebuilt_quad_count(&world, i))
        .collect();
    let build_ms = t.elapsed().as_secs_f64() * 1e3;
    let with_geometry = quads.iter().filter(|&&q| q > 0).count();
    let total_quads: usize = quads.iter().sum();
    println!(
        "{:<34} {:>10} {:>9.1}%",
        "чанков с геометрией во всём мире",
        with_geometry,
        with_geometry as f32 * 100.0 / g.len() as f32
    );
    println!("{:<34} {:>10}", "квадов во всём мире", total_quads);
    println!("{:<34} {:>10.0} мс\n", "сборка эталона", build_ms);

    println!(
        "{:<20} {:>8} {:>10} {:>10} {:>12}",
        "дальность", "в обзоре", "с геометрией", "доля", "квадов"
    );
    let mid = V3::new(
        a.side as f32 * 0.5,
        terrain.surface(a.side / 2, a.side / 2) as f32,
        a.side as f32 * 0.5,
    );
    let mut vis = Vec::new();
    // Углы обзора берутся по кругу: одно направление ничего не говорит
    // о карте, у которой рельеф разный с разных сторон.
    let mut worst: (usize, usize) = (0, 0);
    for distance in [150.0f32, 300.0, 600.0] {
        let (mut sum_vis, mut sum_geo, mut sum_quads) = (0usize, 0usize, 0usize);
        let yaws = 8;
        for k in 0..yaws {
            let yaw = k as f32 * std::f32::consts::TAU / yaws as f32;
            let c = cam(mid, distance, yaw, distance * 4.0);
            render::visible_chunks(&g, &c.frustum(), &mut vis);
            let geo = vis.iter().filter(|&&i| quads[i as usize] > 0).count();
            sum_vis += vis.len();
            sum_geo += geo;
            sum_quads += vis.iter().map(|&i| quads[i as usize]).sum::<usize>();
            if vis.len() > worst.0 {
                worst = (vis.len(), geo);
            }
        }
        println!(
            "{:<20.0} {:>8.1} {:>12.1} {:>9.1}% {:>12.0}",
            distance,
            sum_vis as f32 / yaws as f32,
            sum_geo as f32 / yaws as f32,
            sum_geo as f32 * 100.0 / sum_vis.max(1) as f32,
            sum_quads as f32 / yaws as f32
        );
    }
    println!(
        "\nхудший обзор: {} чанков, из них с геометрией {} — {:.1}%",
        worst.0,
        worst.1,
        worst.1 as f32 * 100.0 / worst.0.max(1) as f32
    );
    println!("Пустой чанк в обзоре стоит проверки и ничего не рисует:");
    println!("над рельефом воздух, под ним сплошная порода, квадов нет ни там, ни там.");
    ExitCode::SUCCESS
}
