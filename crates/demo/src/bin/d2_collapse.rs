//! **D-2 «Обрушение»** — разрушаемость как механика
//! (`docs/14-demo-scenes.md`).
//!
//! ```text
//! d2_collapse --seed 42 --ticks 400 --dump-hash
//! ```
//!
//! Сцена — грибы: ножка держит шляпку. Копатели едят ножки, шляпки
//! отваливаются, становятся телами и падают. Юниты, стоявшие на шляпке,
//! едут вниз вместе с ней (ADR-099) и получают урон по высоте падения
//! (ADR-036).
//!
//! **Порог:** реплей воспроизводится бит-в-бит, и число динамических
//! тел не превышает лимит **ни при каком входе**.

use std::process::ExitCode;
use std::time::Instant;

use demo::RunHash;
use fx::rng::Rng;
use meshing::layer::MeshLayer;
use meshing::Opts;
use sim_core::ground::Ground;
use sim_core::World as SimWorld;
use voxel::store::{Mat, AIR};
use voxel::{Generator, World};

/// Грибной рельеф: земля, ножка, шляпка. Подкоп ножки роняет шляпку.
///
/// **Геометрия подогнана под то, что каскад умеет.** Ступени C нет
/// (`demo::collapse`), поэтому отвал обязан помещаться в один чанк 32³:
/// иначе ступень B вернёт `Overflow` и кусок останется висеть.
/// Шаг решётки равен стороне чанка, центр гриба — в середине чанка,
/// и весь гриб живёт в чанке с индексом 1 по вертикали (32..63).
struct Mushrooms {
    nx: u32,
    ny: u32,
    nz: u32,
}

impl Mushrooms {
    /// Земля до этой высоты. Совпадает с началом чанка Y=1 плюс два
    /// слоя, чтобы гриб стоял на твёрдом внутри того же чанка.
    const GROUND: u32 = 34;
    /// Ножка высокая нарочно. Первая редакция держала её в шесть
    /// вокселей и подрезала **один** слой — шляпка падала ровно на
    /// воксель, всегда, при любом входе. Механика «юнит едет вниз
    /// и получает урон по высоте» при этом формально работала и
    /// не проверялась ничем (ADR-118).
    const STEM_TOP: u32 = 56;
    const CAP_TOP: u32 = 60;
    /// Шаг решётки грибов = сторона чанка связности.
    const PITCH: u32 = 32;
    const STEM_R: i64 = 3;
    const CAP_R: i64 = 9;
    /// Подрез идёт от этого уровня и на `CUT_H` вокселей вверх —
    /// одним тиком, иначе кусок отвалится после первого же слоя
    /// и упадёт на него же.
    const CUT: u32 = 35;
    const CUT_H: u32 = 14;

    fn nearest(&self, x: u32, z: u32) -> (i64, i64) {
        let p = Self::PITCH as i64;
        // Центр — середина чанка: 16, 48, 80, ...
        let (cx, cz) = ((x as i64 / p) * p + p / 2, (z as i64 / p) * p + p / 2);
        (cx, cz)
    }
}

impl Generator for Mushrooms {
    fn material(&self, x: u32, y: u32, z: u32) -> Mat {
        if x >= self.nx || y >= self.ny || z >= self.nz {
            return AIR;
        }
        if y < Self::GROUND {
            return 2;
        }
        let (cx, cz) = self.nearest(x, z);
        let (dx, dz) = (x as i64 - cx, z as i64 - cz);
        let d2 = dx * dx + dz * dz;
        if y < Self::STEM_TOP {
            if d2 <= Self::STEM_R * Self::STEM_R {
                return 3;
            }
            return AIR;
        }
        if y < Self::CAP_TOP && d2 <= Self::CAP_R * Self::CAP_R {
            return 4;
        }
        AIR
    }
}

struct Args {
    seed: u64,
    ticks: u64,
    side: u32,
    diggers: usize,
    units: usize,
    dump_hash: bool,
    /// Подрезать все грибы разом. Только так проверяется лимит тел:
    /// по очереди их живёт по одному, и потолок не трогается вовсе.
    burst: bool,
    /// Потолок живых тел. Ключом, а не константой: потолок
    /// переразметок растягивает залп во времени, и жёсткость лимита
    /// иначе перестаёт проверяться (ADR-133).
    /// `None` — брать из данных; флаг нужен, чтобы проверять потолок
    /// тесным значением.
    max_bodies: Option<usize>,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            seed: 42,
            ticks: 400,
            side: 192,
            diggers: 24,
            units: 40,
            dump_hash: false,
            burst: false,
            max_bodies: None,
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
            "--side" => a.side = num("--side")? as u32,
            "--diggers" => a.diggers = num("--diggers")? as usize,
            "--units" => a.units = num("--units")? as usize,
            "--dump-hash" => a.dump_hash = true,
            "--burst" => a.burst = true,
            "--max-bodies" => a.max_bodies = Some(num("--max-bodies")? as usize),
            "--help" | "-h" => {
                println!("D-2 «Обрушение»\n\n  --seed N --ticks N --side N --diggers N --units N --dump-hash --burst\n");
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
}

struct Outcome {
    hash: u64,
    max_fall: u32,
    damage_total: u64,
    detached: usize,
    overflow: usize,
    cells: usize,
    landed: usize,
    peak_bodies: usize,
    refused: usize,
    /// Сколько юнитов вообще удалось поставить на шляпки. Ноль здесь
    /// значит, что сцена не собралась, а не что механика не работает.
    units_placed: usize,
    units_on_nav_floor: usize,
    promoted_total: usize,
    frontier_max: usize,
    max_bodies: usize,
    fall_per_voxel: u32,
    graph_chunks: usize,
    graph_bytes: usize,
    graph_nodes: usize,
    graph_edges: usize,
    rounds_max: usize,
    rounds_capped: bool,
    riders_total: usize,
    killed: usize,
    ms_collapse: f64,
    quads: usize,
}

fn run(a: &Args) -> Outcome {
    let g = Mushrooms {
        nx: a.side,
        ny: 64,
        nz: a.side,
    };
    let world = World::new(
        a.side,
        64,
        a.side,
        8192,
        Box::new(Mushrooms {
            nx: a.side,
            ny: 64,
            nz: a.side,
        }),
    );
    let mut mesh = MeshLayer::new(&world, Opts::default());

    // Порядок фаз задаёт движок (ADR-125): сцена только копает
    // и смотрит, что получилось.
    // Потолок тел сцена задаёт сама: она им и проверяется, залпом
    // с тесным потолком. Остальные числа — из данных (ADR-140).
    let mut t = demo::tuning();
    if let Some(m) = a.max_bodies {
        t.max_bodies = m;
    }
    let mut sim = SimWorld::new(1, 1, t);
    sim.attach_ground(Ground::new(world, &t));

    let mut hash = RunHash::new();

    // Копатели подрезают ножки: гриб за грибом, по одному на чанк.
    let mut r = Rng::new(a.seed, 0xD2);
    let mut targets: Vec<(u32, u32)> = Vec::new();
    let p = Mushrooms::PITCH;
    let mut cz = p / 2;
    while cz + (Mushrooms::CAP_R as u32) < a.side && targets.len() < a.diggers {
        let mut cx = p / 2;
        while cx + (Mushrooms::CAP_R as u32) < a.side && targets.len() < a.diggers {
            targets.push((cx, cz));
            cx += p;
        }
        cz += p;
    }

    // Юниты стоят на шляпках — их и уронит. Юниты **движковые**:
    // посадку на кусок, поездку и урон от падения делает фаза тика,
    // а не сцена. Пока навигация брала в колонке один уровень, сцена
    // держала своих: движковые показали бы три поездки из сорока,
    // потому что тридцать семь юнитов стояли там, где навигация
    // ничего не видит (ADR-135). Объёмный слой это закрыл (ADR-136).
    let mut ids: Vec<u32> = Vec::new();
    for _ in 0..a.units {
        let (cx, cz) = targets[r.below(targets.len().max(1) as u32) as usize];
        let x = (cx as i64 + r.range(-6, 6) as i64).clamp(1, a.side as i64 - 2) as u32;
        let z = (cz as i64 + r.range(-6, 6) as i64).clamp(1, a.side as i64 - 2) as u32;
        if g.material(x, Mushrooms::CAP_TOP - 1, z) == AIR {
            continue;
        }
        // Скорость ноль: пассажир стоит, а не идёт. Сто вокселей,
        // привязанных к ядру, — это здешнее «сто здоровья».
        let id = sim.units.spawn(x as i32, z as i32, 0, 0, 100);
        // Стоит **на** шляпке: первый свободный над ней.
        sim.units.y[id as usize] = Mushrooms::CAP_TOP as i32;
        ids.push(id);
    }
    sim.units.refresh_live();

    let mut ms_collapse = 0.0f64;
    let (mut detached, mut overflow, mut cells, mut landed) = (0usize, 0usize, 0usize, 0usize);
    let (mut peak_bodies, mut riders_total) = (0usize, 0usize);
    let (mut graph_nodes, mut graph_edges, mut rounds_max) = (0usize, 0usize, 0usize);
    let mut rounds_capped = false;
    let (mut promoted_total, mut frontier_max) = (0usize, 0usize);
    let (mut max_fall, mut damage_total) = (0u32, 0u64);

    for tick in 1..=a.ticks {
        // Подрезаем грибы по очереди: гриб k начинает разрушаться
        // на тике k*12. Так тела появляются вразнобой, а не все разом,
        // и лимит проверяется потоком, а не одним залпом.
        let step = if a.burst { 1 } else { 12 };
        let w = &mut sim.ground.as_mut().expect("земля").world;
        for (k, &(cx, cz)) in targets.iter().enumerate() {
            let start = if a.burst { 0 } else { k as u64 * step };
            if tick <= start || tick > start + step {
                continue;
            }
            for y in Mushrooms::CUT..Mushrooms::CUT + Mushrooms::CUT_H {
                for dz in -(Mushrooms::STEM_R + 1)..=(Mushrooms::STEM_R + 1) {
                    for dx in -(Mushrooms::STEM_R + 1)..=(Mushrooms::STEM_R + 1) {
                        let (x, z) = (cx as i64 + dx, cz as i64 + dz);
                        if x < 0 || z < 0 || x >= a.side as i64 || z >= a.side as i64 {
                            continue;
                        }
                        if w.solid(x as u32, y, z as u32) {
                            w.queue_dig(x as u32, y, z as u32);
                        }
                    }
                }
            }
        }
        // Тик движка: дренаж, каскад, падение тел, связность.
        let t = Instant::now();
        let th = sim.tick();
        ms_collapse += t.elapsed().as_secs_f64() * 1e3;
        // Хеш тика движка — в хеш прогона: юниты теперь его, и сцена
        // проверяет их состояние тем же числом, что и sim-cli.
        hash.feed(th);

        let g = sim.ground.as_mut().expect("земля");
        let cs = g.stats.collapse;
        let bs = g.stats.bodies;
        hash.feed_edits(tick, g.edits());
        mesh.apply(&g.world, g.edits());
        detached += cs.detached;
        overflow += cs.too_big;
        graph_nodes = graph_nodes.max(cs.nodes);
        graph_edges = graph_edges.max(cs.edges);
        rounds_max = rounds_max.max(cs.rounds);
        promoted_total += cs.promoted;
        frontier_max = frontier_max.max(cs.frontier);
        rounds_capped |= cs.rounds_capped;
        cells += cs.cells;

        // Посадка на свежие куски, поездка и урон по высоте падения —
        // фаза `Bodies` (ADR-135). Сцена читает счётчики, а не повторяет
        // механику: вторая её реализация расходилась бы с первой молча.
        riders_total += g.stats.rides as usize;
        max_fall = max_fall.max(g.stats.max_fall);
        damage_total += g.stats.fall_damage;

        landed += bs.landed;
        peak_bodies = peak_bodies.max(bs.alive);

        hash.feed(bs.alive as u64);
        hash.feed(g.bodies.refused as u64);
    }

    let killed = ids
        .iter()
        .filter(|&&i| !sim.units.alive[i as usize])
        .count();

    Outcome {
        hash: hash.get(),
        max_fall,
        damage_total,
        detached,
        overflow,
        cells,
        landed,
        peak_bodies,
        refused: sim.ground().expect("земля").bodies.refused,
        promoted_total,
        frontier_max,
        max_bodies: t.max_bodies,
        fall_per_voxel: t.fall_damage_per_voxel as u32,
        graph_chunks: sim.ground().expect("земля").collapse.known_chunks(),
        graph_bytes: sim.ground().expect("земля").collapse.graph_bytes(),
        graph_nodes,
        graph_edges,
        rounds_max,
        rounds_capped,
        units_on_nav_floor: {
            let u = &sim.units;
            let g = sim.ground().expect("земля");
            let st = g.nav.step() as i32;
            let (gw, gh) = g.nav.dims();
            ids.iter()
                .filter(|&&i| {
                    let i = i as usize;
                    let (cx, cz) = ((u.x[i] / st) as u32, (u.z[i] / st) as u32);
                    cx < gw
                        && cz < gh
                        && g.nav.levels_at(cx, cz).iter().any(|&l| l as i32 == u.y[i])
                })
                .count()
        },
        units_placed: ids.len(),
        riders_total,
        killed,
        ms_collapse,
        quads: mesh.quad_total(),
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

    let o = run(&a);
    let n = a.ticks as f64;

    println!("D-2 «Обрушение» — разрушаемость как механика");
    println!(
        "мир {}×64×{}, {} копателей, {} юнитов, {} тиков\n",
        a.side, a.side, a.diggers, a.units, a.ticks
    );

    println!("{:<30} {:>14}", "каскад отделения", "");
    println!("{:<30} {:>14}", "кусков отвалилось", o.detached);
    println!("{:<30} {:>14}", "вокселей в телах", o.cells);
    println!("{:<30} {:>14}", "не влезло в потолок тела", o.overflow);
    println!("{:<30} {:>14}", "узлов в графе компонент", o.graph_nodes);
    println!("{:<30} {:>14}", "рёбер в графе", o.graph_edges);
    println!("{:<30} {:>14}", "чанков в графе", o.graph_chunks);
    println!("{:<30} {:>14}", "  втянуто за прогон", o.promoted_total);
    println!(
        "{:<30} {:>14}",
        "  ждало втягивания, максимум", o.frontier_max
    );
    println!(
        "{:<30} {:>11.2} МиБ",
        "память графа",
        o.graph_bytes as f64 / 1024.0 / 1024.0
    );
    println!("{:<30} {:>14}", "раундов разбора, максимум", o.rounds_max);
    println!(
        "{:<30} {:>14}",
        "разбор повторится следующим тиком",
        if o.rounds_capped { "да" } else { "нет" }
    );
    println!("{:<30} {:>14}", "тел приземлилось", o.landed);
    println!("{:<30} {:>14}", "пик живых тел", o.peak_bodies);
    println!("{:<30} {:>14}", "отказано по лимиту", o.refused);

    println!("\n{:<30} {:>14}", "юниты на кусках (ADR-099)", "");
    println!("{:<30} {:>14}", "юнитов на шляпках", o.units_placed);
    // Сколько из них навигация представляет. Одноуровневый слой видел
    // три из сорока: он брал в колонке нижний уровень, а шляпка висит
    // над пустотой (ADR-135). Объёмный видит все (ADR-136), и число
    // печатается дальше — потому что именно на нём стоит вся сцена.
    println!("{:<30} {:>14}", "  видит навигация", o.units_on_nav_floor);
    println!("{:<30} {:>14}", "поездок", o.riders_total);
    println!("{:<30} {:>14}", "погибло при падении", o.killed);
    println!(
        "{:<30} {:>14}",
        "самое долгое падение, вокселей", o.max_fall
    );
    println!("{:<30} {:>14}", "урона от падения всего", o.damage_total);
    // Ноль погибших здесь — арифметика, а не умолчание, и её надо видеть.
    println!(
        "{:<30} {:>14}",
        "смертельное падение, вокселей",
        100u32.div_ceil(o.fall_per_voxel.max(1))
    );
    println!("{:<30} {:>14}", "  а в чанк помещается", voxel::topo::CS);

    println!("\n{:<30} {:>14}", "время, мс на тик", "");
    println!("{:<30} {:>14.3}", "тик движка", o.ms_collapse / n);
    println!("{:<30} {:>14}", "квадов всего", o.quads);

    let mut ok = true;

    // Порог 1: реплей воспроизводится бит-в-бит.
    let again = run(&a);
    let replay_ok = again.hash == o.hash;
    println!(
        "\nреплей: {:016x} против {:016x} — {}",
        o.hash,
        again.hash,
        if replay_ok {
            "совпал"
        } else {
            "РАЗОШЁЛСЯ"
        }
    );
    if !replay_ok {
        ok = false;
    }

    // Порог 2: лимит тел не превышен ни при каком входе.
    let limit_ok = o.peak_bodies <= o.max_bodies;
    println!(
        "лимит тел: пик {} из {} — {}",
        o.peak_bodies,
        o.max_bodies,
        if limit_ok {
            "держится"
        } else {
            "ПРЕВЫШЕН"
        }
    );
    if !limit_ok {
        ok = false;
    }

    // Порог 3: сцена обязана что-то уронить, иначе она ничего не проверяет.
    if o.detached == 0 {
        println!("НИЧЕГО НЕ ОТВАЛИЛОСЬ — сцена ничего не проверяет");
        ok = false;
    }

    // Порог 4: навигация обязана представлять **каждое** положение юнита.
    // Юнит, стоящий там, где навигации нет, стоит по указу сцены,
    // и поездка его ничего не доказывает (ADR-135).
    let nav_ok = o.units_on_nav_floor == o.units_placed;
    println!(
        "навигация видит юнитов: {} из {} — {}",
        o.units_on_nav_floor,
        o.units_placed,
        if nav_ok { "все" } else { "НЕ ВСЕХ" }
    );
    if !nav_ok {
        ok = false;
    }

    // Порог 5: поездки обязаны случиться. Ноль здесь значит, что
    // механика, ради которой сцена написана, не сработала.
    if o.riders_total == 0 && o.units_placed > 0 {
        println!("НИ ОДНОЙ ПОЕЗДКИ — механика ADR-099 не сработала");
        ok = false;
    }

    if a.dump_hash {
        println!("seed={} ticks={} hash={:016x}", a.seed, a.ticks, o.hash);
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
