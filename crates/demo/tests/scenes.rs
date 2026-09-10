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

// ---------------------------------------------------------------- D-2

use voxel::bodies::Bodies;
use voxel::store::{Mat, AIR};
use voxel::{Edit, Generator};

/// Гриб: земля, ножка, шляпка. Весь гриб внутри одного чанка 32³ —
/// иначе ступень B вернёт `Overflow`, потому что ступени C нет.
struct Mushroom;

impl Generator for Mushroom {
    fn material(&self, x: u32, y: u32, z: u32) -> Mat {
        if y < 34 {
            return 2;
        }
        let (dx, dz) = (x as i64 - 16, z as i64 - 16);
        let d2 = dx * dx + dz * dz;
        if y < 40 {
            return if d2 <= 9 { 3 } else { AIR };
        }
        if y < 44 && d2 <= 81 {
            return 4;
        }
        AIR
    }
}

fn cut_stem(w: &mut World, level: u32) {
    for dz in -4i64..=4 {
        for dx in -4i64..=4 {
            let (x, z) = ((16 + dx) as u32, (16 + dz) as u32);
            if w.solid(x, level, z) {
                w.queue_dig(x, level, z);
            }
        }
    }
}

/// **Подрезанная ножка роняет шляпку.** Без этого сцена D-2 ничего
/// не показывает: каскад может «работать» и не отделять ничего.
#[test]
fn d2_cutting_the_stem_detaches_the_cap() {
    let mut w = World::new(32, 64, 32, 100_000, Box::new(Mushroom));
    let mut collapse = demo::collapse::CollapseLayer::new(&w);
    let mut bodies = Bodies::new(16);

    cut_stem(&mut w, 35);
    w.flush();
    let edits: Vec<Edit> = w.edits().to_vec();
    let st = collapse.apply(&mut w, &edits, &mut bodies);
    assert!(st.detached > 0, "ничего не отделилось");
    assert_eq!(st.overflow, 0, "кусок не поместился в чанк");
    assert!(st.cells > 500, "отвал подозрительно мал: {}", st.cells);
    w.flush();

    // Шляпка ушла из мира в тело.
    assert!(!w.solid(16, 42, 16), "шляпка осталась на месте");
    assert!(!bodies.list.is_empty(), "тела не появилось");
}

/// **Лимит тел жёсткий.** Кусок сверх лимита остаётся в мире целым:
/// «иногда падает, иногда нет» — это разное состояние у разных клиентов.
#[test]
fn d2_body_limit_refuses_rather_than_drops() {
    let mut w = World::new(32, 64, 32, 100_000, Box::new(Mushroom));
    let mut collapse = demo::collapse::CollapseLayer::new(&w);
    let mut bodies = Bodies::new(0);

    cut_stem(&mut w, 35);
    w.flush();
    let edits: Vec<Edit> = w.edits().to_vec();
    let st = collapse.apply(&mut w, &edits, &mut bodies);
    assert_eq!(st.detached, 0, "тело появилось при нулевом лимите");
    assert!(bodies.refused > 0, "отказ не засчитан");
    w.flush();
    assert!(w.solid(16, 42, 16), "отказанная шляпка исчезла из мира");
}

/// Каскад детерминирован: тот же вход — тот же состав тел, включая
/// порядок вокселей внутри каждого.
#[test]
fn d2_cascade_is_deterministic() {
    let run = || -> Vec<(i64, i64, i64, usize)> {
        let mut w = World::new(32, 64, 32, 100_000, Box::new(Mushroom));
        let mut collapse = demo::collapse::CollapseLayer::new(&w);
        let mut bodies = Bodies::new(16);
        let mut trace = Vec::new();
        for t in 0..12u32 {
            cut_stem(&mut w, 35 + t % 3);
            w.flush();
            let edits: Vec<Edit> = w.edits().to_vec();
            collapse.apply(&mut w, &edits, &mut bodies);
            w.flush();
            bodies.step(&mut w);
            w.flush();
            for b in &bodies.list {
                trace.push((b.ox, b.oy, b.oz, b.voxels.len()));
            }
        }
        trace
    };
    let a = run();
    assert!(!a.is_empty(), "каскад ничего не сделал");
    assert_eq!(a, run());
}
