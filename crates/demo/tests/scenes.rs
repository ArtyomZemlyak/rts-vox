//! Сцены обязаны быть детерминированными и обязаны ловить то, ради чего
//! их писали. Без этого сцена — ролик, а не проверка
//! (`docs/14-demo-scenes.md`).

use demo::dig::Dig;
use demo::layers::{MeshLayer, NavLayer, TopoLayer};
use demo::{RunHash, Terrain};
use meshing::Opts;
use voxel::World;

struct Run {
    hash: u64,
    edits: u64,
    quads: usize,
    seam_mismatches: usize,
}

fn run_mine(seed: u64, ticks: u64, diggers: usize, side: u32, verify: bool) -> Run {
    let params = worldgen::Params { nx: side, ny: side / 2, nz: side, ..Default::default() };
    let terrain = Terrain { params, seed };
    let mut world = World::new(
        params.nx,
        params.ny,
        params.nz,
        4096,
        Box::new(Terrain { params, seed }),
    );
    let mut mesh = MeshLayer::new(&world, Opts::default());
    let mut topo = TopoLayer::new(&world);
    let mut nav = NavLayer::new(&world, 4, 2);
    let mut dig = Dig::clustered(
        &world,
        |x, z| terrain.surface(x, z),
        diggers,
        2,
        4,
        seed ^ 0xD16,
    );
    let mut hash = RunHash::new();
    let mut edits = 0u64;
    for tick in 1..=ticks {
        dig.step(&mut world);
        let st = world.flush();
        edits += st.applied as u64;
        hash.feed_edits(tick, world.edits());
        mesh.apply(&world, world.edits());
        topo.apply(&world, world.edits());
        nav.apply(&world, world.edits());
    }
    hash.feed(edits);
    hash.feed(mesh.quad_total() as u64);
    let seam_mismatches = if verify {
        mesh.live_indices()
            .into_iter()
            .filter(|&i| mesh.rebuilt_quad_count(&world, i) != mesh.quad_at(i))
            .count()
    } else {
        0
    };
    Run { hash: hash.get(), edits, quads: mesh.quad_total(), seam_mismatches }
}

/// Тот же сид — тот же прогон. Без этого сцена не может быть порогом:
/// нечего сравнивать между сборками.
#[test]
fn d1_is_deterministic() {
    let a = run_mine(42, 60, 60, 128, false);
    let b = run_mine(42, 60, 60, 128, false);
    assert_eq!(a.hash, b.hash);
    assert_eq!(a.edits, b.edits);
    assert_eq!(a.quads, b.quads);
    let c = run_mine(43, 60, 60, 128, false);
    assert_ne!(a.hash, c.hash, "разный сид дал тот же прогон");
}

/// **Швы.** Инкрементальный ремеш обязан дать то же число квадов, что
/// полная пересборка каждого чанка. Это единственный способ поймать
/// неверно обновлённый паддинг соседа: без него на границах чанков
/// появляются лишние квады, и видно это не сразу.
#[test]
fn d1_incremental_remesh_matches_full_rebuild() {
    let r = run_mine(7, 120, 80, 128, true);
    assert_eq!(r.seam_mismatches, 0, "швы разошлись на {} чанках", r.seam_mismatches);
    assert!(r.edits > 1000, "копка почти ничего не сняла: {}", r.edits);
}

/// Копка обязана снимать материю, а не «работать». Прогон, который
/// ничего не выкопал, проходил бы все остальные проверки.
#[test]
fn d1_actually_digs() {
    let params = worldgen::Params { nx: 128, ny: 64, nz: 128, ..Default::default() };
    let terrain = Terrain { params, seed: 11 };
    let mut world = World::new(
        params.nx,
        params.ny,
        params.nz,
        4096,
        Box::new(Terrain { params, seed: 11 }),
    );
    let before = solid_count(&world);
    let mut dig = Dig::clustered(&world, |x, z| terrain.surface(x, z), 40, 2, 2, 11);
    for _ in 0..60 {
        dig.step(&mut world);
        world.flush();
    }
    let after = solid_count(&world);
    assert!(after < before, "мир не изменился: {before} -> {after}");
    assert!(
        world.touched_chunks() > 0,
        "слой изменений пуст, хотя копали"
    );
}

fn solid_count(w: &World) -> u64 {
    let mut n = 0u64;
    for z in (0..w.nz).step_by(3) {
        for y in (0..w.ny).step_by(3) {
            for x in (0..w.nx).step_by(3) {
                n += w.solid(x, y, z) as u64;
            }
        }
    }
    n
}
