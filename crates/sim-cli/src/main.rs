//! Headless-прогон симуляции: гейт этапа 0 из `docs/11-roadmap.md`.
//!
//! ```text
//! sim-cli --seed 42 --ticks 1000 --dump-hash
//! ```
//!
//! Ни рендера, ни окна, ни видеокарты. Весь тестовый контур проекта
//! headless (`analysis/09-gpu-compute.md`, раздел 2), и этот бинарник —
//! то, чем он запускается вручную.
//!
//! **Хеш печатается на stdout одной строкой и является контрактом.**
//! Он же сравнивается между Windows и Linux и между числом воркеров.

use std::process::ExitCode;
mod timing;

struct Args {
    seed: u64,
    ticks: u64,
    units: usize,
    threads: usize,
    dump_hash: bool,
    every: u64,
    world_hash: bool,
    stats: bool,
    /// Куда записать состояние после прогона (ADR-229).
    save: Option<std::path::PathBuf>,
    /// Откуда взять состояние вместо заселения с нуля.
    load: Option<std::path::PathBuf>,
    /// Печатать размер файла и время записи-чтения.
    save_stats: bool,
    profile_csv: Option<std::path::PathBuf>,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            seed: 42,
            ticks: 1000,
            units: 5000,
            threads: 1,
            dump_hash: false,
            every: 0,
            world_hash: false,
            stats: false,
            save: None,
            load: None,
            save_stats: false,
            profile_csv: None,
        }
    }
}

fn usage() -> &'static str {
    "\
sim-cli — headless-прогон симуляции

  --seed N        сид (по умолчанию 42)
  --ticks N       сколько тиков (1000)
  --units N       сколько юнитов заселить (5000)
  --threads N     число воркеров; фиксируется явно, не по числу ядер (1)
  --every N       печатать хеш каждые N тиков, а не только итоговый
  --dump-hash     напечатать итоговый хеш состояния
  --stats         напечатать счётчики прогона: время по фазам,
                  землю, стрельбу, смерти — то, чем меряют ADR
  --world-hash    напечатать хеш сгенерированного мира и выйти
  --save ФАЙЛ     записать состояние после прогона (ADR-229)
  --load ФАЙЛ     продолжить с записанного состояния, а не с нуля;
                  сид и заселение при этом берутся из файла
  --profile-csv FILE  записать время каждого тика/фазы и нагрузку в CSV
  --save-stats    напечатать размер файла и время записи-чтения
  --help          это сообщение
"
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
            "--units" => a.units = num("--units")? as usize,
            "--threads" => a.threads = num("--threads")?.max(1) as usize,
            "--every" => a.every = num("--every")?,
            "--dump-hash" => a.dump_hash = true,
            "--world-hash" => a.world_hash = true,
            "--stats" => a.stats = true,
            "--save-stats" => a.save_stats = true,
            "--profile-csv" => {
                a.profile_csv = Some(std::path::PathBuf::from(
                    it.next()
                        .ok_or_else(|| "--profile-csv: missing path".to_string())?,
                ))
            }
            "--save" => {
                a.save = Some(std::path::PathBuf::from(
                    it.next()
                        .ok_or_else(|| "--save: нет имени файла".to_string())?,
                ))
            }
            "--load" => {
                a.load = Some(std::path::PathBuf::from(
                    it.next()
                        .ok_or_else(|| "--load: нет имени файла".to_string())?,
                ))
            }
            "--help" | "-h" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("неизвестный ключ: {other}")),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
    };

    if a.world_hash {
        // Мир **эталонного сценария**, а не умолчание генератора: флаг,
        // печатающий хеш мира, которым сценарий не пользуется, — это
        // ровно та же ошибка, что и сам ADR-157.
        let p = sim_core::ref_params();
        println!(
            "world seed={} {}x{}x{} hash={:016x}",
            a.seed,
            p.nx,
            p.ny,
            p.nz,
            worldgen::world_hash(&p, a.seed)
        );
        return ExitCode::SUCCESS;
    }

    // Числа тика — из данных, а не из исходника (ADR-140). Файл
    // ищется рядом с корнем репозитория, потом в текущем каталоге:
    // headless-прогон запускают и оттуда, и отсюда.
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/sim.ron");
    let path = if here.exists() {
        here
    } else {
        std::path::PathBuf::from("data/sim.ron")
    };
    let tuning = match modding::load_tuning(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return std::process::ExitCode::FAILURE;
        }
    };
    // Сценарий собирает `sim-core`, а не этот файл: тесты детерминизма
    // обязаны гонять ровно то, на чём стоит хеш (ADR-157).
    let weapons = {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/weapons.ron");
        let path = if here.exists() {
            here
        } else {
            std::path::PathBuf::from("data/weapons.ron")
        };
        match modding::weapons::load(&path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("{e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    };
    // Ввод-вывод живёт здесь, а не в симуляции: `sim-core` отдаёт
    // буфер и принимает буфер, файл открывает вызывающий
    // (`CLAUDE.md`, границы слоёв).
    let mut r = match &a.load {
        None => {
            // Замер холодного старта — чтобы было с чем сравнивать
            // загрузку. Мир строится из генератора в обоих случаях:
            // процедурный слой не хранится, он воспроизводится
            // (ADR-229).
            let t0 = std::time::Instant::now();
            let r = sim_core::Reference::new(a.seed, a.units, tuning, weapons);
            if a.save_stats {
                println!("холодный старт: {} мкс", t0.elapsed().as_nanos() / 1000);
            }
            r
        }
        Some(p) => {
            let buf = match std::fs::read(p) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("{}: {e}", p.display());
                    return ExitCode::FAILURE;
                }
            };
            // Замер разбора отделён от замера чтения с диска: диск —
            // не формат, и мерить их одним числом значит не померить
            // ни того, ни другого.
            let t0 = std::time::Instant::now();
            let r = match sim_core::save::load(&buf, tuning, weapons) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}: {e}", p.display());
                    return ExitCode::FAILURE;
                }
            };
            let ns = t0.elapsed().as_nanos();
            if a.save_stats {
                println!(
                    "загрузка: {} байт, разбор {} мкс, тик {}",
                    buf.len(),
                    ns / 1000,
                    r.world.tick
                );
            }
            r
        }
    };

    let mut hash = 0u64;
    // Итоги по земле копятся по тикам: `g.stats` обнуляется каждый тик,
    // и последний снимок говорит про последний тик, а не про прогон.
    let (mut edits, mut topo, mut falls, mut rides, mut worst_q) = (0u64, 0u64, 0u64, 0u64, 0usize);
    // Самое долгое падение за прогон. Обнуляется каждый тик вместе
    // с остальной землёй, поэтому максимум копится здесь.
    let mut max_fall = 0u32;
    let mut backlog = 0u64;
    let (mut spawned, mut detached, mut too_big) = (0u64, 0u64, 0u64);
    // Застрявшие: живой юнит, не сдвинувшийся за последнюю сотню тиков.
    // Это конечная проверка навигации — не «есть ли направление»,
    // а «дошёл ли кто-нибудь куда-нибудь».
    let mut snap: Vec<(u32, i32, i32)> = Vec::new();
    let mut timing = timing::Timings::default();
    let profiling = a.stats || a.profile_csv.is_some();
    for t in 1..=a.ticks {
        let stamp = profiling.then(|| timing::start(&r.world));
        hash = if a.threads > 1 {
            r.tick_par(a.threads)
        } else {
            r.tick()
        };
        if let Some(stamp) = stamp {
            timing.record(&r.world, stamp);
        }
        if let Some(g) = r.world.ground() {
            edits += g.stats.edits as u64;
            topo += g.stats.topo_dirty as u64;
            falls += g.stats.fall_damage;
            max_fall = max_fall.max(g.stats.max_fall);
            rides += g.stats.rides;
            worst_q = worst_q.max(g.stats.queued);
            // Потолок, срабатывающий **каждый** тик, — не предохранитель,
            // а постоянное ухудшение: он скрывает то, что ограничивает
            // (`CLAUDE.md`, ADR-114, ADR-116). Считается по тому, остался
            // ли после тика хвост в очереди.
            backlog += u64::from(g.stats.queued > 0);
            spawned += g.bodies.spawned.len() as u64;
            detached += g.stats.collapse.detached as u64;
            too_big += g.stats.collapse.too_big as u64;
        }
        if a.stats && t + 100 == a.ticks {
            snap = r
                .world
                .units
                .live()
                .iter()
                .map(|&i| {
                    let i = i as usize;
                    (i as u32, r.world.units.x[i], r.world.units.z[i])
                })
                .collect();
        }
        if a.every > 0 && t % a.every == 0 {
            println!("tick={t} hash={hash:016x}");
        }
    }

    if profiling {
        if let Err(e) = timing.report(a.profile_csv.as_deref()) {
            eprintln!("timing report: {e}");
            return ExitCode::FAILURE;
        }
    }
    if a.stats {
        // Что сценарий на самом деле нагружает. Хеш, совпавший точно
        // после правки боя, обязан быть объяснён, а не принят на веру
        // (`docs/analysis/20-measurement-machine.md`, правило пятое).
        // Что из земли сценарий трогает, а что стоит: фаза, у которой
        // ноль и работы, и времени, в хеш не входит (ADR-116, ADR-157).
        for p in sim_core::PHASES {
            println!(
                "  фаза {:<14} {:>10} мкс",
                p.name(),
                r.world.phase_ns_of(p) / 1000
            );
        }
        let mut occupancy = std::collections::BTreeMap::<(i32, i32), usize>::new();
        for &i in r.world.units.live() {
            *occupancy
                .entry((r.world.units.x[i as usize], r.world.units.z[i as usize]))
                .or_default() += 1;
        }
        println!(
            "  distinct centres {} max occupancy {} examined move {}",
            occupancy.len(),
            occupancy.values().copied().max().unwrap_or(0),
            r.world.examined_move
        );
        println!("  contact group pairs {}", r.world.contact_group_pairs);
        for (name, ns) in ["projectiles", "targeting", "fire", "materials"]
            .iter()
            .zip(r.world.combat_ns)
        {
            println!("    combat/{name:<11} {:>10} мкс", ns / 1000);
        }
        for (name, ns) in ["topology", "terrain", "fields", "indices"]
            .iter()
            .zip(r.world.navigation_ns)
        {
            println!("    navigation/{name:<10} {:>10} мкс", ns / 1000);
        }
        for (name, ns) in ["requests", "drive", "suspension"]
            .iter()
            .zip(r.world.movement_ns)
        {
            println!("    movement/{name:<10} {:>10} мкс", ns / 1000);
        }
        // Рельеф сценария: плоская карта не нагружает навигацию по объёму,
        // а плоской она становится молча — обрезкой высоты (ADR-157).
        {
            let pr = sim_core::ref_params();
            let (mut lo, mut hi, mut sum) = (u32::MAX, 0u32, 0u64);
            for z in (0..pr.nz).step_by(4) {
                for x in (0..pr.nx).step_by(4) {
                    let h = worldgen::surface_height(&pr, a.seed, x, z);
                    lo = lo.min(h);
                    hi = hi.max(h);
                    sum += h as u64;
                }
            }
            let n = ((pr.nx / 4) * (pr.nz / 4)) as u64;
            println!(
                "  рельеф: высота {lo}..{hi} (размах {}), средняя {}, потолок мира {}",
                hi - lo,
                sum / n.max(1),
                pr.ny
            );
        }
        if r.world.ground().is_some() {
            let cap = r
                .world
                .ground()
                .map(|g| g.world.max_edits_per_tick)
                .unwrap_or(0);
            println!(
                "  земля за прогон: правок {edits}, чанков связности {topo}, урона от падения {falls}, поездок {rides}, самое долгое падение {max_fall}"
            );
            println!(
                "  очередь: худшая {worst_q}, тиков с хвостом {backlog} из {}, потолок правок {cap}",
                a.ticks
            );
            println!(
                "  каскад: отделений {detached}, тел создано {spawned}, не влезло в потолок {too_big}, поездок {rides}"
            );
        } else {
            println!("  земли нет");
        }
        // Стоять и стрелять — не «застрять». Считаются отдельно:
        // застрявшим называется тот, кто не сдвинулся и цели не имеет.
        let (mut still, mut shooting, mut blind) = (0usize, 0usize, 0usize);
        for &(i, x, z) in snap.iter() {
            let i = i as usize;
            if !r.world.units.alive[i] || r.world.units.x[i] != x || r.world.units.z[i] != z {
                continue;
            }
            if r.world.units.target[i] != sim_core::NO_TARGET {
                shooting += 1;
            } else {
                still += 1;
                // «Застрял» — не диагноз, пока не сказано почему.
                // Нет направления — поле до него не дошло; есть —
                // значит держит теснота или обрыв под ногами.
                if r.world.dir_under(i) == 255 {
                    blind += 1;
                }
            }
        }
        if !snap.is_empty() {
            println!(
                "  стоят сто тиков: {still} без цели ({blind} из них без направления), {shooting} с целью (стреляют), из {} живых на тике {}",
                snap.len(),
                a.ticks - 100
            );
        }
        println!(
            "  лучом: выстрелов {}, попаданий {}, не стреляли по закрытой цели {}",
            r.world.beams_fired, r.world.beams_hit, r.world.beams_blocked
        );
        println!(
            "  стрельба: выстрелов {}, попаданий {}, упоров в породу {}, отказов по потолку {}, в воздухе {}",
            r.world.shots_fired,
            r.world.shots_hit,
            r.world.shots_blocked,
            r.world.shots_refused,
            r.world.shots.alive_count()
        );
        println!(
            "  взрывы: {}, вокселей в очередь {}, задето волной {}",
            r.world.blasts, r.world.blast_voxels, r.world.blast_hits
        );
        // Смерти считаются прямо, а не как убыль живых: с подкреплениями
        // численность держится ровной, и выбитый строй по ней
        // неотличим от нетронутого (ADR-164). Мерило силы боя — это.
        println!("  смертей за прогон: {}", r.world.deaths);
        let u = &r.world.units;
        let mut heave = Vec::new();
        let mut peak_v = 0.0f64;
        for &id in u.live() {
            let i = id as usize;
            heave.push(u.heave[i].to_f64_lossy());
            peak_v = peak_v.max(u.body_vy(i).to_f64_lossy().abs());
        }
        heave.sort_by(f64::total_cmp);
        if !heave.is_empty() {
            println!(
                "  suspension: offset_min={:.6} median={:.6} max={:.6} max_vertical_speed={:.6}",
                heave[0],
                heave[heave.len() / 2],
                heave[heave.len() - 1],
                peak_v
            );
            let mut extreme = u.live().to_vec();
            extreme.sort_by_key(|&id| std::cmp::Reverse(u.heave[id as usize].abs().raw()));
            for id in extreme.into_iter().take(5) {
                let i = id as usize;
                println!("    suspension unit={id} kind={} xyz={},{},{} offset={:.4} vy={:.4} fall={} riding={} load={:.4}",
                    u.kind[i],u.x[i],u.y[i],u.z[i],u.heave[i].to_f64_lossy(),u.body_vy(i).to_f64_lossy(),
                    u.fall_from[i],u.riding[i],u.load[i].to_f64_lossy());
                if let Some(g) = r.world.ground() {
                    let (nw, nh) = g.nav.dims();
                    let (ex, ez) = g.extent();
                    let (hc, hs) = sim_core::ride::heading_of(u.head[i]);
                    let floors = sim_core::ride::supports_of(r.world.tuning.chassis(u.chassis[i]))
                        .map(|s| {
                            let x = (u.x[i] + s.x.mul(hc).sub(s.z.mul(hs)).to_int_trunc())
                                .clamp(0, ex - 1) as u32;
                            let z = (u.z[i] + s.x.mul(hs).add(s.z.mul(hc)).to_int_trunc())
                                .clamp(0, ez - 1) as u32;
                            let (cx, cz) = (
                                (x * nw / ex as u32).min(nw - 1),
                                (z * nh / ez as u32).min(nh - 1),
                            );
                            g.nav
                                .levels_at(cx, cz)
                                .iter()
                                .copied()
                                .min_by_key(|&y| (y as i32 - u.body_cell_y(i)).abs())
                                .unwrap_or(g.nav.floor_at(cx, cz))
                        });
                    println!(
                        "      support floors={floors:?} body_y={:.4}",
                        u.body_y(i).to_f64_lossy()
                    );
                }
            }
        }
        let live = r.world.units.live().len();
        let with_target = r
            .world
            .units
            .live()
            .iter()
            .filter(|&&i| r.world.units.target[i as usize] != sim_core::NO_TARGET)
            .count();
        println!(
            "живых {live}, с целью {with_target}, осмотрено на бою {}, поисков {}, тиков в потолок {}",
            r.world.examined_combat, r.world.searches, r.world.search_capped
        );
        for o in 0..2u8 {
            let (mut sx, mut sz, mut n) = (0i64, 0i64, 0i64);
            for &i in r.world.units.live() {
                if r.world.units.owner[i as usize] == o {
                    sx += r.world.units.x[i as usize] as i64;
                    sz += r.world.units.z[i as usize] as i64;
                    n += 1;
                }
            }
            if n > 0 {
                println!("  сторона {o}: {n} юнитов, центр ({}, {})", sx / n, sz / n);
            }
        }
    }

    if a.dump_hash || a.every == 0 {
        println!(
            "seed={} ticks={} units={} threads={} hash={:016x}",
            a.seed, a.ticks, a.units, a.threads, hash
        );
    }

    if let Some(p) = &a.save {
        let t0 = std::time::Instant::now();
        let buf = sim_core::save::save(&r);
        let build_ns = t0.elapsed().as_nanos();
        let t1 = std::time::Instant::now();
        if let Err(e) = std::fs::write(p, &buf) {
            eprintln!("{}: {e}", p.display());
            return ExitCode::FAILURE;
        }
        let disk_ns = t1.elapsed().as_nanos();
        if a.save_stats {
            println!(
                "сохранение: {} байт, сборка {} мкс, запись на диск {} мкс, тик {}",
                buf.len(),
                build_ns / 1000,
                disk_ns / 1000,
                r.world.tick
            );
            if let Some(g) = r.world.ground() {
                println!(
                    "  слой правок: тронутых чанков {}, в памяти {} байт; каскад знает {} чанков",
                    g.world.touched_chunks(),
                    g.world.diff_bytes(),
                    g.collapse.known_chunks()
                );
            }
        }
    }
    ExitCode::SUCCESS
}
