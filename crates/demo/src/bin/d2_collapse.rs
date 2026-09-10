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

use voxel::collapse::CollapseLayer;
use demo::layers::MeshLayer;
use demo::RunHash;
use fx::rng::Rng;
use meshing::Opts;
use voxel::bodies::Bodies;
use voxel::store::{Mat, AIR};
use voxel::{Edit, Generator, World};

/// Жёсткий лимит живых тел.
const MAX_BODIES: usize = 64;
/// Урон юниту за воксель падения (ADR-036). Число балансное и здесь
/// стоит только потому, что сцена не грузит данные.
const FALL_DAMAGE_PER_VOXEL: u32 = 3;

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

/// Юнит: позиция, здоровье и тело, на котором он едет.
struct Unit {
    x: u32,
    y: u32,
    z: u32,
    hp: u32,
    /// Устойчивый номер тела; ноль — стоит на земле. Индексом
    /// ссылаться нельзя: приземлившееся тело уходит из списка
    /// сдвигом, и все индексы за ним съезжают (ADR-118).
    riding: u64,
    /// Смещение относительно начала координат тела.
    off: (i64, i64, i64),
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
}

impl Default for Args {
    fn default() -> Args {
        Args { seed: 42, ticks: 400, side: 192, diggers: 24, units: 40, dump_hash: false, burst: false }
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
    promoted_total: usize,
    frontier_max: usize,
    graph_chunks: usize,
    graph_bytes: usize,
    graph_nodes: usize,
    graph_edges: usize,
    rounds_max: usize,
    rounds_capped: bool,
    riders_total: usize,
    killed: usize,
    ms_collapse: f64,
    ms_bodies: f64,
    quads: usize,
}

fn run(a: &Args) -> Outcome {
    let g = Mushrooms { nx: a.side, ny: 64, nz: a.side };
    let mut world = World::new(a.side, 64, a.side, 8192, Box::new(Mushrooms { nx: a.side, ny: 64, nz: a.side }));
    let mut mesh = MeshLayer::new(&world, Opts::default());
    let mut collapse = CollapseLayer::new(&world);
    let mut bodies = Bodies::new(MAX_BODIES);
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

    // Юниты стоят на шляпках — их и уронит.
    let mut units: Vec<Unit> = Vec::new();
    for _ in 0..a.units {
        let (cx, cz) = targets[r.below(targets.len().max(1) as u32) as usize];
        let x = (cx as i64 + r.range(-6, 6) as i64).clamp(1, a.side as i64 - 2) as u32;
        let z = (cz as i64 + r.range(-6, 6) as i64).clamp(1, a.side as i64 - 2) as u32;
        if g.material(x, Mushrooms::CAP_TOP - 1, z) == AIR {
            continue;
        }
        units.push(Unit {
            x,
            // Стоит **на** шляпке: первый свободный над ней.
            y: Mushrooms::CAP_TOP,
            z,
            hp: 100,
            riding: 0,
            off: (0, 0, 0),
        });
    }

    let (mut ms_collapse, mut ms_bodies) = (0.0f64, 0.0f64);
    let (mut detached, mut overflow, mut cells, mut landed) = (0usize, 0usize, 0usize, 0usize);
    let (mut peak_bodies, mut riders_total, mut killed) = (0usize, 0usize, 0usize);
    let (mut graph_nodes, mut graph_edges, mut rounds_max) = (0usize, 0usize, 0usize);
    let mut rounds_capped = false;
    let (mut promoted_total, mut frontier_max) = (0usize, 0usize);
    let (mut max_fall, mut damage_total) = (0u32, 0u64);

    for tick in 1..=a.ticks {
        // Подрезаем грибы по очереди: гриб k начинает разрушаться
        // на тике k*12. Так тела появляются вразнобой, а не все разом,
        // и лимит проверяется потоком, а не одним залпом.
        let step = if a.burst { 1 } else { 12 };
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
                        if world.solid(x as u32, y, z as u32) {
                            world.queue_dig(x as u32, y, z as u32);
                        }
                    }
                }
            }
        }
        let st = world.flush();
        hash.feed_edits(tick, world.edits());
        mesh.apply(&world, world.edits());

        // Каскад отделения.
        let t = Instant::now();
        let before = bodies.list.len();
        // Список правок копируется: каскад берёт мир исключительно,
        // а правки — это его же поле.
        let tick_edits: Vec<Edit> = world.edits().to_vec();
        let cs = collapse.apply(&mut world, &tick_edits, &mut bodies);
        ms_collapse += t.elapsed().as_secs_f64() * 1e3;
        detached += cs.detached;
        overflow += cs.too_big;
        graph_nodes = graph_nodes.max(cs.nodes);
        graph_edges = graph_edges.max(cs.edges);
        rounds_max = rounds_max.max(cs.rounds);
        promoted_total += cs.promoted;
        frontier_max = frontier_max.max(cs.frontier);
        rounds_capped |= cs.rounds_capped;
        cells += cs.cells;

        // Юниты садятся на новые тела: тот, кто оказался над куском,
        // едет на нём (ADR-099).
        for bi in before..bodies.list.len() {
            let (ox, oy, oz) = (bodies.list[bi].ox, bodies.list[bi].oy, bodies.list[bi].oz);
            let (sx, sz) = (bodies.list[bi].sx as i64, bodies.list[bi].sz as i64);
            // Габарит по высоте: юнит стоит НА куске, то есть не выше
            // его верха плюс воксель. Первая редакция брала здесь
            // константу двенадцать под тогдашнюю геометрию, и высокая
            // ножка сразу выключила все поездки (ADR-118).
            let sy = bodies.list[bi]
                .voxels
                .iter()
                .map(|v| v.y as i64)
                .max()
                .unwrap_or(0)
                + 1;
            let mut riders = Vec::new();
            for (ui, u) in units.iter_mut().enumerate() {
                if u.riding != 0 || u.hp == 0 {
                    continue;
                }
                let (dx, dz) = (u.x as i64 - ox, u.z as i64 - oz);
                // Юнит едет, если он в габаритах куска по X и Z
                // и стоит не выше его верха: шляпка толщиной четыре
                // воксела плюс запас на неровность.
                let uy = u.y as i64;
                if dx >= 0 && dx < sx && dz >= 0 && dz < sz && uy >= oy && uy <= oy + sy {
                    u.riding = bodies.list[bi].id;
                    u.off = (dx, u.y as i64 - oy, dz);
                    riders.push(ui as u32);
                }
            }
            riders_total += riders.len();
            bodies.list[bi].riders = riders;
        }

        // Правки от отрыва применяются до падения: мир обязан увидеть
        // дыру в тот же тик.
        let st2 = world.flush();
        hash.feed_edits(tick * 2 + 1, world.edits());
        mesh.apply(&world, world.edits());
        for e in world.edits() {
            collapse.invalidate_at(e.x, e.y, e.z);
        }
        let _ = (st, st2);

        // Падение.
        let t = Instant::now();
        let bs = bodies.step(&mut world);
        ms_bodies += t.elapsed().as_secs_f64() * 1e3;
        landed += bs.landed;
        peak_bodies = peak_bodies.max(bs.alive);

        // Наездники следуют за телом; севшие на землю получают урон.
        //
        // Тело ищется по **устойчивому номеру**. Пока ссылались индексом,
        // приходилось при каждой посадке ссаживать вообще всех — иначе
        // наездник поехал бы за чужим телом, — и высота падения в отчёте
        // выходила два воксела вместо четырнадцати (ADR-118).
        for u in units.iter_mut() {
            if u.riding == 0 {
                continue;
            }
            if let Some(bi) = bodies.index_of(u.riding) {
                let b = &bodies.list[bi];
                u.x = (b.ox + u.off.0).max(0) as u32;
                u.y = (b.oy + u.off.1).max(0) as u32;
                u.z = (b.oz + u.off.2).max(0) as u32;
                continue;
            }
            // Тела больше нет: оно приземлилось на этом шаге и само
            // сказало, сколько пролетело.
            let fallen = bodies
                .landed_log
                .iter()
                .find(|l| l.id == u.riding)
                .map(|l| l.fallen)
                .unwrap_or(0);
            let dmg = fallen * FALL_DAMAGE_PER_VOXEL;
            max_fall = max_fall.max(fallen);
            damage_total += dmg as u64;
            u.hp = u.hp.saturating_sub(dmg);
            if u.hp == 0 {
                killed += 1;
            }
            u.riding = 0;
        }

        hash.feed(bs.alive as u64);
        hash.feed(bodies.refused as u64);
    }

    for u in &units {
        hash.feed(u.hp as u64);
        hash.feed(u.x as u64 * 1_000_003 + u.y as u64 * 1009 + u.z as u64);
    }

    Outcome {
        hash: hash.get(),
        max_fall,
        damage_total,
        detached,
        overflow,
        cells,
        landed,
        peak_bodies,
        refused: bodies.refused,
        promoted_total,
        frontier_max,
        graph_chunks: collapse.known_chunks(),
        graph_bytes: collapse.graph_bytes(),
        graph_nodes,
        graph_edges,
        rounds_max,
        rounds_capped,
        units_placed: units.len(),
        riders_total,
        killed,
        ms_collapse,
        ms_bodies,
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
    println!("мир {}×64×{}, {} копателей, {} юнитов, {} тиков\n", a.side, a.side, a.diggers, a.units, a.ticks);

    println!("{:<30} {:>14}", "каскад отделения", "");
    println!("{:<30} {:>14}", "кусков отвалилось", o.detached);
    println!("{:<30} {:>14}", "вокселей в телах", o.cells);
    println!("{:<30} {:>14}", "не влезло в потолок тела", o.overflow);
    println!("{:<30} {:>14}", "узлов в графе компонент", o.graph_nodes);
    println!("{:<30} {:>14}", "рёбер в графе", o.graph_edges);
    println!("{:<30} {:>14}", "чанков в графе", o.graph_chunks);
    println!("{:<30} {:>14}", "  втянуто за прогон", o.promoted_total);
    println!("{:<30} {:>14}", "  ждало втягивания, максимум", o.frontier_max);
    println!("{:<30} {:>11.2} МиБ", "память графа", o.graph_bytes as f64 / 1024.0 / 1024.0);
    println!("{:<30} {:>14}", "раундов разбора, максимум", o.rounds_max);
    println!(
        "{:<30} {:>14}",
        "упёрлось в потолок раундов",
        if o.rounds_capped { "ДА" } else { "нет" }
    );
    println!("{:<30} {:>14}", "тел приземлилось", o.landed);
    println!("{:<30} {:>14}", "пик живых тел", o.peak_bodies);
    println!("{:<30} {:>14}", "отказано по лимиту", o.refused);

    println!("\n{:<30} {:>14}", "юниты на кусках (ADR-099)", "");
    println!("{:<30} {:>14}", "юнитов на шляпках", o.units_placed);
    println!("{:<30} {:>14}", "поездок", o.riders_total);
    println!("{:<30} {:>14}", "погибло при падении", o.killed);
    println!("{:<30} {:>14}", "самое долгое падение, вокселей", o.max_fall);
    println!("{:<30} {:>14}", "урона от падения всего", o.damage_total);
    // Ноль погибших здесь — арифметика, а не умолчание, и её надо видеть.
    println!(
        "{:<30} {:>14}",
        "смертельное падение, вокселей",
        100u32.div_ceil(FALL_DAMAGE_PER_VOXEL)
    );
    println!(
        "{:<30} {:>14}",
        "  а в чанк помещается",
        voxel::topo::CS
    );

    println!("\n{:<30} {:>14}", "время, мс на тик", "");
    println!("{:<30} {:>14.3}", "каскад", o.ms_collapse / n);
    println!("{:<30} {:>14.3}", "падение тел", o.ms_bodies / n);
    println!("{:<30} {:>14}", "квадов всего", o.quads);

    let mut ok = true;

    // Порог 1: реплей воспроизводится бит-в-бит.
    let again = run(&a);
    let replay_ok = again.hash == o.hash;
    println!("\nреплей: {:016x} против {:016x} — {}", o.hash, again.hash,
             if replay_ok { "совпал" } else { "РАЗОШЁЛСЯ" });
    if !replay_ok {
        ok = false;
    }

    // Порог 2: лимит тел не превышен ни при каком входе.
    let limit_ok = o.peak_bodies <= MAX_BODIES;
    println!(
        "лимит тел: пик {} из {MAX_BODIES} — {}",
        o.peak_bodies,
        if limit_ok { "держится" } else { "ПРЕВЫШЕН" }
    );
    if !limit_ok {
        ok = false;
    }

    // Порог 3: сцена обязана что-то уронить, иначе она ничего не проверяет.
    if o.detached == 0 {
        println!("НИЧЕГО НЕ ОТВАЛИЛОСЬ — сцена ничего не проверяет");
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

