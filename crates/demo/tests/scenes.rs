//! Сцены обязаны быть детерминированными и обязаны ловить то, ради чего
//! их писали. Без этого сцена — ролик, а не проверка
//! (`docs/14-demo-scenes.md`).

use demo::dig::Dig;
use meshing::layer::MeshLayer;
use nav::layer::NavLayer;
use voxel::topo::layer::TopoLayer;
use demo::{RunHash, Terrain};
use meshing::Opts;
use sim_core::ground::Ground;
use sim_core::{populate, World as SimWorld};
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
    let mut nav = NavLayer::new(&world, 4, 2, 1);
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
        dig.step(&mut world, &|x, z| terrain.surface(x, z));
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
        dig.step(&mut world, &|x, z| terrain.surface(x, z));
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

/// Тяготение сцены — то же, что у игры: из `data/sim.ron` (ADR-173).
/// Литерала здесь нет нарочно: сцена обязана падать так же, как партия.
fn scene_gravity() -> fx::Fx {
    let t = modding::Tuning::from_ron(include_str!("../../../data/sim.ron"))
        .expect("data/sim.ron");
    sim_core::gravity_per_tick(&t)
}
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
    let mut collapse = voxel::collapse::CollapseLayer::new(&w);
    let mut bodies = Bodies::new(16, scene_gravity());

    cut_stem(&mut w, 35);
    w.flush();
    let edits: Vec<Edit> = w.edits().to_vec();
    let st = collapse.apply(&mut w, &edits, &mut bodies);
    assert!(st.detached > 0, "ничего не отделилось");
    assert_eq!(st.too_big, 0, "кусок не поместился в потолок тела");
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
    let mut collapse = voxel::collapse::CollapseLayer::new(&w);
    let mut bodies = Bodies::new(0, scene_gravity());

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
        let mut collapse = voxel::collapse::CollapseLayer::new(&w);
        let mut bodies = Bodies::new(16, scene_gravity());
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

// ---------------------------------------------------------------- D-3

use voxel::topo::unit::CoreBufs;
use voxel::unit::{tank, Part, UnitBody};

/// Радиус попадания **в вокселях тела** при заданном радиусе в мире.
///
/// Полметра — это полметра, сколько бы вокселей в него ни укладывалось
/// (`CLAUDE.md`: мировые единицы, не воксели). Пока ребро тела было
/// четвертью метра, здесь стояла двойка; после перехода на сто
/// миллиметров (ADR-219) та же двойка означала бы вчетверо более
/// слабый выстрел, и отстрел подсистемы переставал сходиться.
fn blast_voxels(u: &UnitBody, mm: u32) -> i64 {
    (mm / u.mm.max(1) as u32).max(1) as i64
}

/// Полметра — калибр, которым сцена отстреливает подсистемы.
const BLAST_MM: u32 = 500;

fn shoot(u: &mut UnitBody, x: i64, y: i64, z: i64, r: i64) -> u32 {
    let mut n = 0;
    for dz in -r..=r {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy + dz * dz > r * r {
                    continue;
                }
                let (px, py, pz) = (x + dx, y + dy, z + dz);
                if px < 0 || py < 0 || pz < 0 {
                    continue;
                }
                let (px, py, pz) = (px as usize, py as usize, pz as usize);
                if px < u.vol.nx as usize && py < u.vol.ny as usize && pz < u.vol.nz as usize {
                    n += u.remove(px, py, pz) as u32;
                }
            }
        }
    }
    n
}

fn aim_at(u: &UnitBody, p: Part) -> Option<(i64, i64, i64)> {
    for z in 0..u.vol.nz as usize {
        for y in 0..u.vol.ny as usize {
            for x in 0..u.vol.nx as usize {
                if u.vol.get(x, y, z) && u.part_at(x, y, z) == p {
                    return Some((x as i64, y as i64, z as i64));
                }
            }
        }
    }
    None
}

/// **Прицельный отстрел отключает ровно то, во что целились.**
/// Это то, ради чего выбраны помеченные подсистемы (ADR-095):
/// без этого воксельное тело неотличимо от скалярного HP с бронёй
/// по направлению.
#[test]
fn d3_shooting_a_subsystem_disables_only_it() {
    let mut b = CoreBufs::new();
    for target in [Part::Engine, Part::Weapon, Part::Sensor] {
        let mut t = tank();
        let mut shots = 0;
        while let Some((x, y, z)) = aim_at(&t, target) {
            {
                let r = blast_voxels(&t, BLAST_MM);
                shoot(&mut t, x, y, z, r)
            };
            shots += 1;
            let s = t.status(&mut b);
            if !s.get(target) {
                assert!(s.alive, "{}: танк умер, а целились не в ядро", target.name());
                for other in [Part::Engine, Part::Weapon, Part::Sensor] {
                    if other != target {
                        assert!(
                            s.get(other),
                            "{}: заодно отказал {}",
                            target.name(),
                            other.name()
                        );
                    }
                }
                break;
            }
            assert!(shots < 200, "{} не отказал за 200 выстрелов", target.name());
        }
        assert!(shots > 0);
    }
}

/// Пробитие к ядру убивает, и это отличается от потери подсистемы.
#[test]
fn d3_core_hit_kills_but_a_subsystem_hit_does_not() {
    let mut b = CoreBufs::new();
    let mut t = tank();
    while let Some((x, y, z)) = aim_at(&t, Part::Core) {
        {
                let r = blast_voxels(&t, BLAST_MM);
                shoot(&mut t, x, y, z, r)
            };
        if !t.status(&mut b).alive {
            break;
        }
    }
    assert!(!t.status(&mut b).alive, "танк пережил уничтожение ядра");
}

/// Разбор детерминирован: тот же обстрел — то же состояние.
#[test]
fn d3_teardown_is_deterministic() {
    let run = || -> Vec<(bool, bool, bool, bool, u32)> {
        let mut b = CoreBufs::new();
        let mut t = tank();
        let mut r = fx::rng::Rng::new(3, 0x3D);
        let mut trace = Vec::new();
        for _ in 0..40 {
            let x = r.below(t.vol.nx as u32) as i64;
            let y = r.below(t.vol.ny as u32) as i64;
            let z = r.below(t.vol.nz as u32) as i64;
            {
                let r = blast_voxels(&t, BLAST_MM);
                shoot(&mut t, x, y, z, r)
            };
            let s = t.status(&mut b);
            trace.push((s.alive, s.engine, s.weapon, s.sensor, s.attached));
            if !s.alive {
                break;
            }
        }
        trace
    };
    let a = run();
    assert!(a.len() > 3, "танк умер слишком быстро — тест ни о чём");
    assert_eq!(a, run());
}

// ---------------------------------------------------------------- D-4

use voxel::fleet::Fleet;

/// Сцена D-4 детерминирована: тот же обстрел флота даёт то же число
/// копий и ту же память.
#[test]
fn d4_cow_is_deterministic() {
    let run = || -> (u64, usize, usize) {
        let mut f = Fleet::new(vec![tank()]);
        let ids: Vec<u32> = (0..200).map(|_| f.spawn(0)).collect();
        let mut r = fx::rng::Rng::new(5, 0xC0);
        for _ in 0..600 {
            let id = ids[r.below(ids.len() as u32) as usize];
            let (x, y, z) = (
                r.below(24) as usize,
                r.below(12) as usize,
                r.below(12) as usize,
            );
            f.body_mut(id).remove(x, y, z);
        }
        (f.stats().copies, f.stats().damaged, f.bytes())
    };
    let a = run();
    assert!(a.0 > 0 && a.1 > 0);
    assert_eq!(a, run());
}

/// **Копий ровно столько, сколько тронутых юнитов.** Лишняя копия —
/// это молчаливое удвоение памяти боя.
#[test]
fn d4_copies_exactly_once_per_unit() {
    let mut f = Fleet::new(vec![tank()]);
    let ids: Vec<u32> = (0..50).map(|_| f.spawn(0)).collect();
    for &id in &ids {
        for k in 0..10 {
            f.body_mut(id).remove(12, 6, k % 12);
        }
    }
    assert_eq!(f.stats().copies, 50, "COW скопировал лишнее");
    assert_eq!(f.stats().damaged, 50);
}

/// Нетронутый юнит читается через шаблон и даёт то же состояние,
/// что свежесозданный. Иначе COW менял бы поведение, а не только память.
#[test]
fn d4_pristine_unit_reads_like_a_fresh_one() {
    let mut b = CoreBufs::new();
    let mut f = Fleet::new(vec![tank()]);
    let a = f.spawn(0);
    let c = f.spawn(0);
    f.body_mut(c).remove(12, 6, 6);
    let sa = f.status(a, &mut b);
    let sc = f.status(c, &mut b);
    assert!(f.is_pristine(a));
    assert_eq!(sa.attached, tank().solid_count(), "целый юнит читается неверно");
    assert!(sc.attached < sa.attached, "повреждение не отразилось");
}

// ---------------------------------------------------------------- D-5

use sim_core::matter::{resource_of, ContainerId, Kind, Ledger, Sink, SINKS};

/// Прогон D-5 в миниатюре: копка, доставка, подбитие, реклейм.
/// Возвращает (хеш, сработавшие стоки, худшее расхождение).
// Длина — из перечня, а не числом: шестой сток ронял этот массив
// по выходу за границу (ADR-185).
fn run_haul(seed: u64, ticks: u64) -> (u64, [u64; SINKS.len()], i128) {
    let mut l = Ledger::new();
    let ground = l.create(Kind::Ground, 1_000_000, u64::MAX);
    // Потолок и переработка подобраны так, чтобы склад реально
    // упирался в потолок: иначе сток «излишек над потолком»
    // не срабатывает и инвариант проверяется на пустом.
    let depot = l.create(Kind::Depot, 0, 2_000);
    let refined = l.create(Kind::Depot, 0, u64::MAX);
    let tanks: Vec<ContainerId> = (0..20).map(|_| l.create(Kind::Hauler, 0, 600)).collect();
    let mut alive = vec![true; tanks.len()];
    let mut debris: Vec<ContainerId> = Vec::new();
    let mut r = fx::rng::Rng::new(seed, 0xD5);
    let mut hash = RunHash::new();
    let mut worst = 0i128;

    for tick in 1..=ticks {
        for (i, &t) in tanks.iter().enumerate() {
            if !alive[i] {
                continue;
            }
            let want = resource_of(3 + r.below(4) as u64, 2 + r.below(6));
            l.transfer(ground, t, want);
            if l.room(t) == 0 {
                l.transfer(t, depot, l.held(t));
            }
        }
        if l.held(depot) > 1_500 {
            l.refine(depot, refined, 200, 7, 10);
        }
        if r.below(10) == 0 {
            let live: Vec<usize> = (0..tanks.len()).filter(|&i| alive[i]).collect();
            if !live.is_empty() {
                let i = live[r.below(live.len() as u32) as usize];
                let (d, _, rest) = l.breach(tanks[i], 1, 4);
                alive[i] = false;
                if rest > 0 {
                    debris.push(d);
                }
            }
        }
        if let Some(&d) = debris.first() {
            let live: Vec<usize> = (0..tanks.len())
                .filter(|&i| alive[i] && l.room(tanks[i]) > 0)
                .collect();
            if !live.is_empty() {
                let i = live[r.below(live.len() as u32) as usize];
                l.transfer(d, tanks[i], 200);
                if l.held(d) == 0 {
                    debris.remove(0);
                }
            }
        }
        if tick % 120 == 0 {
            l.lower_cap(depot, l.get(depot).cap / 2);
        }
        if !l.balanced() {
            worst = l.imbalance();
        }
        hash.feed(tick);
        hash.feed(l.held(ground));
        hash.feed(l.held(depot));
        for s in SINKS {
            hash.feed(l.sunk(s));
        }
    }
    let mut sunk = [0u64; SINKS.len()];
    for (k, s) in SINKS.iter().enumerate() {
        sunk[k] = l.sunk(*s);
    }
    (hash.get(), sunk, worst)
}

// ---------------------------------------------------------------------
// Матч: партия, у которой есть исход (ADR-205).
// ---------------------------------------------------------------------

/// Маленький матч — чтобы тесты оставались быстрыми.
fn small_match(seed: u64, hunt: u64) -> demo::skirmish::Skirmish {
    demo::skirmish::Skirmish::new(demo::skirmish::Setup {
        seed,
        side: 128,
        per_side: 30,
        diggers: 12,
        patience: 1500,
        hunt_period: hunt,
    })
}

/// **У партии есть исход, и обе стороны в ней воюют.**
///
/// Исход сам по себе ничего не значит: партия, где никто не выстрелил,
/// тоже «кончится» по сроку. Поэтому требуется результат боя —
/// выстрелы, попадания, воронки и смерти.
#[test]
fn a_match_ends_and_both_sides_actually_fight() {
    for seed in [42u64, 7, 99] {
        let mut m = small_match(seed, 60);
        let (out, _) = m.play_out();
        assert!(out.over(), "сид {seed}: партия не пришла к исходу");
        let w = &m.world;
        assert!(w.deaths > 0, "сид {seed}: никто не погиб");
        assert!(w.shots_fired > 0, "сид {seed}: ни одного выстрела");
        assert!(w.shots_hit > 0, "сид {seed}: ни одного попадания");
        assert!(w.blasts > 0, "сид {seed}: ни одной воронки");
        assert!(
            w.beams_fired > 0,
            "сид {seed}: лучом не стреляли — оружие роздано не всем"
        );
    }
}

/// **Тот же сид — та же партия.**
#[test]
fn a_match_is_deterministic() {
    let (mut a, mut b) = (small_match(42, 60), small_match(42, 60));
    let (oa, ha) = a.play_out();
    let (ob, hb) = b.play_out();
    assert_eq!(oa, ob, "тот же сид дал другой исход");
    assert_eq!(ha, hb, "тот же сид дал другой хеш");
    assert_eq!(a.tick, b.tick);
    assert_eq!(a.alive, b.alive);

    let mut c = small_match(43, 60);
    let (_, hc) = c.play_out();
    assert_ne!(ha, hc, "разные сиды дали один хеш");
}

/// **Охота за противником — не украшение, а условие исхода.**
///
/// Поле направлений ведёт к точке. Если точка — база противника,
/// то после первой сшибки выжившие расходятся, а стороны идут
/// в пустые углы и не встречаются больше никогда: замерено двадцать
/// тысяч тиков без единой победы (ADR-205).
///
/// Тест требует, чтобы **выключение охоты меняло партию** хотя бы
/// на одном сиде. Без этого требования параметр мог бы вообще
/// не быть подключён — ровно это и случилось при первом замере:
/// три разных периода охоты дали побитово одинаковый результат,
/// потому что ключ не разбирался.
///
/// Сравниваются исход, срок **и хеш**. Одного исхода мало: партия
/// кончается по счёту машин, и победитель по счёту переживает
/// довольно крупные перемены в ходе боя. С инерцией у машин
/// (ADR-220) все три сида стали кончаться одинаковым победителем
/// при разных потерях — то есть проверка по исходу проходила бы
/// и на неподключённом ключе. Хеш ловит ровно то, от чего тест
/// написан: побитово одинаковый результат.
#[test]
fn hunting_the_enemy_changes_how_matches_end() {
    let mut differs = 0;
    for seed in [42u64, 7, 99] {
        let mut with = small_match(seed, 60);
        let mut without = small_match(seed, 0);
        let (a, ha) = with.play_out();
        let (b, hb) = without.play_out();
        if a != b || with.tick != without.tick || ha != hb {
            differs += 1;
        }
    }
    assert!(
        differs > 0,
        "охота ни на что не влияет — либо она не работает, либо ключ не подключён"
    );
}

/// **Главный порог сцены: материя сохраняется на каждом тике.**
/// Проверять только в конце нельзя — ошибка, компенсировавшая сама
/// себя, прошла бы незамеченной.
#[test]
fn d5_matter_is_conserved_every_tick() {
    let (_, _, worst) = run_haul(42, 400);
    assert_eq!(worst, 0, "расхождение баланса {worst}");
}

/// Сцена обязана задействовать стоки, иначе инвариант проверяется
/// на пустом месте.
#[test]
fn d5_actually_exercises_the_sinks() {
    let (_, sunk, _) = run_haul(42, 400);
    assert!(sunk[Sink::RefiningLoss as usize] > 0, "переработка не сработала");
    assert!(sunk[Sink::ContainerBreach as usize] > 0, "разрушений не было");
    assert!(sunk[Sink::OverCap as usize] > 0, "потолок ни разу не срезал");
}

/// **Хеш обязан различать прогоны.** Первая редакция хешировала
/// `in_containers` и `sunk_total`, чья сумма равна `initial`
/// по самому инварианту, — и хеш не менялся ни от сида, ни от чего.
#[test]
fn d5_hash_distinguishes_runs_and_repeats_itself() {
    let a = run_haul(42, 300).0;
    assert_eq!(a, run_haul(42, 300).0, "прогон не воспроизводится");
    assert_ne!(a, run_haul(43, 300).0, "разный сид дал тот же хеш");
    assert_ne!(a, run_haul(42, 200).0, "разная длина дала тот же хеш");
}

// ---------------------------------------------------------------- D-6

use voxel::los::line_of_sight;

struct Ridge;
impl Generator for Ridge {
    fn material(&self, x: u32, y: u32, _z: u32) -> Mat {
        if y < 8 {
            return 2;
        }
        if x.abs_diff(64) <= 5 && y < 40 {
            return 3;
        }
        AIR
    }
}

fn dig_box(w: &mut World, xs: (u32, u32), ys: (u32, u32), zs: (u32, u32)) -> u64 {
    for y in ys.0..ys.1 {
        for z in zs.0..zs.1 {
            for x in xs.0..xs.1 {
                if w.solid(x, y, z) {
                    w.queue_dig(x, y, z);
                }
            }
        }
    }
    let mut n = 0;
    while w.queued() > 0 {
        n += w.flush().applied as u64;
    }
    n
}

/// **Линия огня следует за геометрией в тот же тик.** Гряда закрывает,
/// прокопанный коридор открывает — без кэша и без отложенного
/// обновления.
#[test]
fn d6_line_of_fire_follows_the_geometry() {
    let mut w = World::new(128, 64, 128, 100_000, Box::new(Ridge));
    let a = (32, 9, 64);
    let b = (96, 9, 64);
    assert!(!line_of_sight(&w, a, b).clear, "гряда не закрыла линию");

    // Срыв гребня линию по полу не открывает — она идёт ниже.
    dig_box(&mut w, (59, 70), (20, 40), (58, 70));
    assert!(
        !line_of_sight(&w, a, b).clear,
        "срыв гребня открыл линию по полу"
    );

    // Коридор у основания — открывает.
    let dug = dig_box(&mut w, (59, 70), (8, 14), (61, 67));
    assert!(dug > 0, "коридор не прокопан");
    assert!(
        line_of_sight(&w, a, b).clear,
        "коридор прокопан, а линия закрыта"
    );
}

/// Срыв гребня открывает линию **над** грядой, но только её.
#[test]
fn d6_shaving_the_crest_opens_only_the_upper_line() {
    let mut w = World::new(128, 64, 128, 100_000, Box::new(Ridge));
    let hi_a = (32, 30, 64);
    let hi_b = (96, 30, 64);
    assert!(!line_of_sight(&w, hi_a, hi_b).clear);
    dig_box(&mut w, (59, 70), (20, 40), (58, 70));
    assert!(
        line_of_sight(&w, hi_a, hi_b).clear,
        "гребень сорван, а верхняя линия закрыта"
    );
    assert!(
        !line_of_sight(&w, (32, 9, 64), (96, 9, 64)).clear,
        "заодно открылась нижняя"
    );
}

/// Правка мира видна линии огня **немедленно**: между `flush`
/// и проверкой нет ничего, что могло бы отстать.
#[test]
fn d6_one_voxel_changes_the_answer() {
    let mut w = World::new(128, 64, 128, 100_000, Box::new(Ridge));
    let a = (32, 20, 64);
    let b = (96, 20, 64);
    assert!(!line_of_sight(&w, a, b).clear);
    // Ровно одна колонка сквозь гряду.
    for x in 59..70 {
        w.queue_dig(x, 20, 64);
    }
    w.flush();
    assert!(line_of_sight(&w, a, b).clear, "сквозная дыра не открыла линию");
    // И обратно: один воксель закрывает.
    w.queue_edit(voxel::Edit { x: 64, y: 20, z: 64, mat: 3 });
    w.flush();
    assert!(!line_of_sight(&w, a, b).clear, "один воксель не закрыл линию");
}

// ---------------------------------------------------------------------
// D-8 «Мод»: контент грузится данными.
//
// Сцена архитектурная, а не скоростная, поэтому и проверки здесь — про
// утверждения, а не про миллисекунды. Пути идут от корня репозитория:
// `cargo test` работает из каталога крейта.
// ---------------------------------------------------------------------

mod d8 {
    use modding::{load_pack, merge_materials, PartTag, UnitModel};
    use std::path::{Path, PathBuf};
    use voxel::unit::{tank, Part, UnitBody};

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn part_of(t: PartTag) -> Part {
        match t {
            PartTag::Armour => Part::Armour,
            PartTag::Core => Part::Core,
            PartTag::Engine => Part::Engine,
            PartTag::Weapon => Part::Weapon,
            PartTag::Sensor => Part::Sensor,
        }
    }

    fn body_of(m: &UnitModel) -> UnitBody {
        let mut b = UnitBody::new(m.nx, m.ny, m.nz, m.core_size);
        for &(x, y, z, tag) in &m.voxels {
            b.set(x as usize, y as usize, z as usize, part_of(tag));
        }
        assert!(b.seat_core(), "{}: ядро не садится", m.name);
        b
    }

    /// Главное утверждение сцены: данные и код дают **одно и то же тело**.
    /// Разойдутся — и «мод работает как ванильный контент» станет
    /// обещанием без проверки.
    #[test]
    fn the_tank_from_data_equals_the_tank_from_code() {
        let pack = load_pack(&root().join("data")).expect("ванильный пак");
        let m = pack.units.iter().find(|u| u.name == "Танк").expect("танк");
        let a = body_of(m);
        let b = tank();
        assert_eq!((a.vol.nx, a.vol.ny, a.vol.nz), (b.vol.nx, b.vol.ny, b.vol.nz));
        assert_eq!((a.core, a.core_size), (b.core, b.core_size));
        for z in 0..a.vol.nz as usize {
            for y in 0..a.vol.ny as usize {
                for x in 0..a.vol.nx as usize {
                    assert_eq!(a.vol.get(x, y, z), b.vol.get(x, y, z), "{x} {y} {z}");
                    if a.vol.get(x, y, z) {
                        assert_eq!(a.part_at(x, y, z), b.part_at(x, y, z), "{x} {y} {z}");
                    }
                }
            }
        }
    }

    /// Мод меняет хеш партии, и порядок паков на него не влияет.
    #[test]
    fn a_mod_changes_the_registry_hash_but_load_order_does_not() {
        let v = || load_pack(&root().join("data")).expect("ваниль");
        let m = || load_pack(&root().join("mods/mushroom")).expect("мод");
        let plain = merge_materials(&[v()]).unwrap().hash();
        let both = merge_materials(&[v(), m()]).unwrap().hash();
        let swapped = merge_materials(&[m(), v()]).unwrap().hash();
        assert_ne!(plain, both, "мод обязан менять хеш");
        assert_eq!(both, swapped, "порядок паков не имеет права влиять");
    }

    /// Столкновение номеров — ошибка загрузки, а не переопределение
    /// (ADR-028). Проверяется тем же модом, поставленным дважды.
    #[test]
    fn a_material_id_clash_is_refused() {
        let m = || load_pack(&root().join("mods/mushroom")).expect("мод");
        assert!(merge_materials(&[m(), m()]).is_err());
    }

    /// Юнит мода — полноценное тело: подсистемы гаснут от попаданий,
    /// ядро убивает.
    #[test]
    fn the_mod_unit_takes_damage_like_any_other() {
        use voxel::topo::unit::CoreBufs;
        let pack = load_pack(&root().join("mods/mushroom")).expect("мод");
        let m = pack.units.first().expect("юнит мода");
        let body = body_of(m);
        let mut bufs = CoreBufs::default();
        assert!(body.status(&mut bufs).alive);
        assert!(body.status(&mut bufs).engine);

        let strip = |b: &mut UnitBody, p: Part| {
            for z in 0..b.vol.nz as usize {
                for y in 0..b.vol.ny as usize {
                    for x in 0..b.vol.nx as usize {
                        if b.vol.get(x, y, z) && b.part_at(x, y, z) == p {
                            b.remove(x, y, z);
                        }
                    }
                }
            }
        };
        let mut hurt = body.clone();
        strip(&mut hurt, Part::Engine);
        let s = hurt.status(&mut bufs);
        assert!(s.alive, "двигатель — не ядро");
        assert!(!s.engine);

        let mut dead = body.clone();
        strip(&mut dead, Part::Core);
        assert!(!dead.status(&mut bufs).alive);
    }

    /// Путь из данных не имеет права выйти за пределы пака.
    #[test]
    fn a_path_escaping_the_pack_is_refused() {
        let src = std::fs::read_to_string(root().join("mods/mushroom/units/sporeling.ron"))
            .expect("исходный ron");
        let tmp = std::env::temp_dir().join("rts-vox-d8-escape-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("units")).unwrap();
        std::fs::write(
            tmp.join("units/x.ron"),
            src.replace("units/sporeling.vox", "../../data/units/tank.vox"),
        )
        .unwrap();
        let r = load_pack(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(matches!(r, Err(modding::LoadError::Escape(_))));
    }
}

/// Прогон D-7 в уменьшенном виде.
///
/// Суть сцены — **всё в одном тике**: юниты, копка, ремеш, связность
/// и навигация. Поэтому прогон собирает связку целиком, а не части
/// по отдельности: расхождение здесь ловится там, где подсистемы
/// встречаются, а по отдельности каждая детерминирована и так.
struct Scale {
    hash: u64,
    edits: u64,
    dirty_mesh: u64,
    live: usize,
    with_target: usize,
    /// Результат боя, а не факт выбора цели. Считается **за вторую
    /// половину прогона**: подсистема, работающая только вначале
    /// и затухающая, — это уже дважды случалось (ADR-157, ADR-159).
    shots: u64,
    hits: u64,
    blasts: u64,
    deaths: u64,
}

fn run_scale(seed: u64, ticks: u64, units: usize, diggers: usize, side: u32) -> Scale {
    let params = worldgen::Params { nx: side, ny: side / 2, nz: side, ..Default::default() };
    let terrain = Terrain { params, seed };
    let world = World::new(
        params.nx,
        params.ny,
        params.nz,
        4096,
        Box::new(Terrain { params, seed }),
    );
    let t = demo::tuning();
    let mut mesh = MeshLayer::new(&world, Opts::default());
    let mut dig = Dig::clustered(
        &world,
        |x, z| terrain.surface(x, z),
        diggers,
        2,
        4,
        seed ^ 0xD16,
    );
    let mut sim = SimWorld::new(2, units.max(1), t);
    sim.attach_ground(Ground::new(world, &t));
    // **Без реестра оружия сцена боя не ведёт** (ADR-187): `fire()`
    // выходит первой строкой, и D-7, которая декларирует «всё вместе
    // в одном тике», молчала целой подсистемой.
    sim.attach_weapons(demo::weapons());
    populate(&mut sim, units, seed);

    let mut hash = RunHash::new();
    let (mut edits, mut dirty_mesh) = (0u64, 0u64);
    // Снимок на середине: всё, что после него, — работа второй
    // половины прогона.
    let (mut mid_shots, mut mid_hits, mut mid_blasts, mut mid_deaths) = (0u64, 0u64, 0u64, 0u64);
    for tick in 1..=ticks {
        if tick == ticks / 2 {
            mid_shots = sim.shots_fired;
            mid_hits = sim.shots_hit + sim.beams_hit;
            mid_blasts = sim.blasts;
            mid_deaths = sim.deaths;
        }
        {
            let g = sim.ground_mut().expect("земля");
            dig.step(&mut g.world, &|x, z| terrain.surface(x, z));
        }
        let h = sim.tick();
        // Ремеш читает те же правки, что и хеш: это и есть «в одном
        // тике». Копия правок — потому что `mesh.apply` берёт мир
        // и правки порознь, а оба живут в земле.
        let e = {
            let g = sim.ground().expect("земля");
            edits += g.stats.edits as u64;
            hash.feed_edits(tick, g.edits());
            hash.feed(h);
            g.edits().to_vec()
        };
        dirty_mesh += mesh.apply(&sim.ground().expect("земля").world, &e) as u64;
    }
    let live = sim.units.live().len();
    let with_target = sim
        .units
        .live()
        .iter()
        .filter(|&&i| sim.units.target[i as usize] != sim_core::NO_TARGET)
        .count();
    Scale {
        hash: hash.get(),
        edits,
        dirty_mesh,
        live,
        with_target,
        shots: sim.shots_fired - mid_shots,
        hits: (sim.shots_hit + sim.beams_hit) - mid_hits,
        blasts: sim.blasts - mid_blasts,
        deaths: sim.deaths - mid_deaths,
    }
}

/// **Тот же сид — тот же прогон, и связка не расходится.**
///
/// Единственная сцена, у которой этого теста не было: пороги она
/// печатала, а повторяемость никто не требовал.
#[test]
fn d7_is_deterministic() {
    let a = run_scale(42, 40, 400, 40, 480);
    let b = run_scale(42, 40, 400, 40, 480);
    assert_eq!(a.hash, b.hash, "тот же сид дал другой хеш");
    assert_eq!(a.edits, b.edits);
    assert_eq!(a.dirty_mesh, b.dirty_mesh);
    assert_eq!(a.live, b.live);
    let c = run_scale(43, 40, 400, 40, 480);
    assert_ne!(a.hash, c.hash, "разный сид дал тот же прогон");
}

/// **Сцена нагружает всё, что декларирует, и это её единственное
/// оправдание** (`CLAUDE.md`).
///
/// D-7 обещает юнитов, копку, ремеш и бой **одновременно**. Порог,
/// снятый со сцены, где молчит хоть одна подсистема, — не порог.
#[test]
fn d7_loads_every_subsystem_at_once() {
    let r = run_scale(42, 60, 600, 60, 480);
    println!(
        "D-7 уменьшенная: правок {}, грязных чанков меша {}, живых {}, с целью {}, выстрелов {}, попаданий {}, упёрлось в породу {}, отказов {}",
        r.edits, r.dirty_mesh, r.live, r.with_target, r.shots, r.hits, 0, 0
    );
    assert!(r.edits > 1000, "копка почти ничего не сняла: {}", r.edits);
    assert!(r.dirty_mesh > 100, "ремеш почти не вызывался: {}", r.dirty_mesh);
    assert!(r.live > 0, "юниты кончились");
    assert!(
        r.with_target > 20,
        "бой не начался: из {} живых никто не взял цель",
        r.live
    );
    // **Цель — не бой.** Прежняя проверка требовала факта выбора цели
    // и проходила на сцене, где реестр оружия не привязан вовсе:
    // `fire()` выходил первой строкой, и ни выстрела не случалось.
    // Заглушённый `fire()` целиком оставлял все 25 тестов сцен
    // зелёными (ADR-187).
    //
    // Требуется **результат, и во второй половине прогона**:
    // подсистема, отработавшая вначале и затухшая, — не нагрузка.
    assert!(r.shots > 0, "за вторую половину прогона не выпущено ни снаряда");
    assert!(r.hits > 0, "ни одного попадания за вторую половину");
    assert!(r.blasts > 0, "ни одной воронки за вторую половину");
    assert!(
        r.deaths > 0,
        "никто не погиб за вторую половину: бой идёт вхолостую"
    );
}
