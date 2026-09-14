//! **D-6 «Формирование поля боя»** — разрушаемость меняет линии огня
//! и проходимость (`docs/14-demo-scenes.md`).
//!
//! ```text
//! d6_battlefield --seed 42 --dump-hash
//! ```
//!
//! Гряда разделяет две позиции: стрелять нельзя и обойти нельзя.
//! Игрок срывает гребень — открывается линия огня; прокапывает коридор
//! у основания — открывается проход, и маршрут по графу порталов
//! идёт через него.
//!
//! **Порог:** видимость и навигация видят изменение **в тот же тик**,
//! что и копка; перестройка графа порталов укладывается в бюджет.

use std::process::ExitCode;
use std::time::Instant;

use demo::RunHash;
use nav::godunov::{Cost, BLOCKED};
use nav::portals::{route, PortalGraph, RouteWs};
use nav::scene::Grid;
use voxel::los::line_of_sight;
use voxel::store::{Mat, AIR};
use voxel::{Generator, World};

/// Бюджет на перестройку графа порталов после копки, миллисекунд.
/// Полная пересборка 512×512 стоит 5.27 мс (ADR-106); здесь карта
/// вчетверо меньше по площади.
const BUDGET_MS: f64 = 3.0;

/// Гряда поперёк карты: пол, стена от края до края, свободный воздух.
struct Ridge {
    nx: u32,
    ny: u32,
    nz: u32,
    /// Где стоит гряда по X.
    at: u32,
    /// Полутолщина.
    half: u32,
    /// До какой высоты.
    top: u32,
}

impl Generator for Ridge {
    fn material(&self, x: u32, y: u32, z: u32) -> Mat {
        if x >= self.nx || y >= self.ny || z >= self.nz {
            return AIR;
        }
        if y < 8 {
            return 2;
        }
        if x.abs_diff(self.at) <= self.half && y < self.top {
            return 3;
        }
        AIR
    }
}

struct Args {
    seed: u64,
    side: u32,
    dump_hash: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            seed: 42,
            side: 256,
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
            "--side" => a.side = num("--side")? as u32,
            "--dump-hash" => a.dump_hash = true,
            "--help" | "-h" => {
                println!("D-6 «Формирование поля боя»\n\n  --seed N --side N --dump-hash\n");
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
}

/// Сетка проходимости из мира, достижимая от стартовой клетки.
///
/// **Три вещи, и каждая обязательна.**
///
/// 1. **Уровень пола ищется снизу вверх, а не сверху вниз.** Поверхностная
///    мерка берёт верх колонки и потому видит верх отвесной гряды как
///    проходимый, а прокопанный под ней коридор — не видит вовсе.
///    Ровно та слепота, которую назвал ADR-108.
/// 2. **Перепад высоты — свойство ребра, а не клетки.** Локальный
///    критерий не работает ни в какую сторону: строгий («блокировать
///    клетку, если хоть у одного соседа перепад велик») запрещает ходить
///    вдоль стены и убивает прокопанный коридор; мягкий («достижима,
///    если хоть к одному соседу перепад мал») оставляет проходимой
///    вершину отвесной гряды, потому что её соседи — та же вершина.
/// 3. **Достижимость решается графом, а не клеткой.** Вершина гряды
///    сама по себе проходима; недостижима она потому, что ни одно
///    ребро не ведёт к ней с земли. Высота пола и посильный перепад
///    отдаются `nav::Grid`, и рёбер через уступ там не возникает
///    (ADR-121). Раньше сцена обходила это заливкой снаружи.
///
/// Это ещё не объёмная навигация (ADR-039): в колонке берётся **один**
/// уровень, нижний. Настоящая держала бы их несколько.
fn passability(w: &World, height: u32, max_step: u32) -> (Grid, Vec<u32>) {
    let step = 2u32;
    let (nw, nh) = ((w.nx / step) as usize, (w.nz / step) as usize);
    let mut floor = vec![u32::MAX; nw * nh];

    for cz in 0..nh {
        for cx in 0..nw {
            let mut best = u32::MAX;
            for dz in 0..step {
                for dx in 0..step {
                    let (x, z) = (cx as u32 * step + dx, cz as u32 * step + dz);
                    // Снизу вверх: первый уровень, где под ногами твёрдое,
                    // а над головой просвет в рост.
                    let mut y = 1;
                    while y + height < w.ny {
                        if w.solid(x, y - 1, z) {
                            let mut clear = 0;
                            while clear < height && !w.solid(x, y + clear, z) {
                                clear += 1;
                            }
                            if clear >= height {
                                best = best.min(y);
                                break;
                            }
                        }
                        y += 1;
                    }
                }
            }
            floor[cz * nw + cx] = best;
        }
    }

    // Высота и перепад отдаются графу порталов: **перепад — свойство
    // ребра** (ADR-121). Заливка снаружи здесь была, пока движок про
    // высоту не знал; теперь она не нужна, и это заодно проверка,
    // что не нужна.
    let mut g = Grid::new(nw, nh, 1 as Cost);
    g.max_step = max_step as u16;
    for (i, &f) in floor.iter().enumerate() {
        let idx = g.idx(i % nw + 1, i / nw + 1);
        if f == u32::MAX {
            g.cost[idx] = BLOCKED;
        } else {
            g.cost[idx] = 1;
            g.floor[idx] = f.min(u16::MAX as u32 - 1) as u16;
        }
    }
    (g, floor)
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };

    let side = a.side;
    let ridge = Ridge {
        nx: side,
        ny: 64,
        nz: side,
        at: side / 2,
        half: 5,
        top: 40,
    };
    let mut w = World::new(
        side,
        64,
        side,
        20_000,
        Box::new(Ridge {
            nx: side,
            ny: 64,
            nz: side,
            at: side / 2,
            half: 5,
            top: 40,
        }),
    );
    let mut hash = RunHash::new();

    // Стрелок и цель по разные стороны гряды, на уровне пола.
    let shooter = ((side / 4) as i32, 9, (side / 2) as i32);
    let target = ((side * 3 / 4) as i32, 9, (side / 2) as i32);
    // Гребень выше линии огня — сорвать надо именно его.
    let eye_hi = (shooter.0, 30, shooter.2);
    let tgt_hi = (target.0, 30, target.2);

    println!("D-6 «Формирование поля боя»");
    println!(
        "мир {side}×64×{side}, гряда на x={} толщиной {} до высоты {}\n",
        ridge.at,
        ridge.half * 2 + 1,
        ridge.top
    );

    println!("{:<40} {:>10} {:>10}", "проверка", "до", "после");

    // --- 1. Линия огня закрыта грядой.
    let los0 = line_of_sight(&w, shooter, target);
    let los_hi0 = line_of_sight(&w, eye_hi, tgt_hi);

    // --- 2. Проходимость: обойти нельзя, гряда от края до края.
    let t0 = Instant::now();
    // Рост четыре воксела (2 м), подъём не больше двух (1 м).
    let (g0, _) = passability(&w, 4, 2);
    let (pg0, st0) = PortalGraph::build(&g0);
    let build0_ms = t0.elapsed().as_secs_f64() * 1e3;
    let mut ws = RouteWs::new(pg0.portals.len().max(1));
    let from = g0.idx((shooter.0 / 2) as usize + 1, (shooter.2 / 2) as usize + 1);
    let to = g0.idx((target.0 / 2) as usize + 1, (target.2 / 2) as usize + 1);
    let r0 = route(&pg0, &g0, &mut ws, from, to);

    // --- 3. Срываем гребень: копаем сверху донизу до уровня глаз.
    let mut dug_crest = 0u64;
    for y in 20..40u32 {
        for z in (side / 2 - 6)..(side / 2 + 6) {
            for x in (ridge.at - ridge.half)..=(ridge.at + ridge.half) {
                if w.solid(x, y, z) {
                    w.queue_dig(x, y, z);
                }
            }
        }
    }
    while w.queued() > 0 {
        dug_crest += w.flush().applied as u64;
    }
    let los_hi1 = line_of_sight(&w, eye_hi, tgt_hi);
    let los1 = line_of_sight(&w, shooter, target);
    // Снос гребня меряется отдельно от коридора: он открывает линию
    // огня и при этом **отнимает** переходы — дно выемки на двадцать
    // вокселей ниже соседнего гребня, и это обрыв. Одно число на две
    // разные правки скрыло бы и то, и другое (ADR-143).
    let (g_crest, _) = passability(&w, 4, 2);
    let links_crest = g_crest.links();
    let route_crest = {
        let (pg, _) = PortalGraph::build(&g_crest);
        let mut ws = RouteWs::new(pg.portals.len().max(1));
        route(&pg, &g_crest, &mut ws, from, to).found
    };

    // --- 4. Прокапываем коридор у основания.
    let mut dug_gap = 0u64;
    for y in 8..14u32 {
        for z in (side / 2 - 3)..(side / 2 + 3) {
            for x in (ridge.at - ridge.half)..=(ridge.at + ridge.half) {
                if w.solid(x, y, z) {
                    w.queue_dig(x, y, z);
                }
            }
        }
    }
    while w.queued() > 0 {
        dug_gap += w.flush().applied as u64;
    }
    let los2 = line_of_sight(&w, shooter, target);

    let t1 = Instant::now();
    let (g1, _) = passability(&w, 4, 2);
    let (pg1, st1) = PortalGraph::build(&g1);
    let build1_ms = t1.elapsed().as_secs_f64() * 1e3;
    let mut ws1 = RouteWs::new(pg1.portals.len().max(1));
    let r1 = route(&pg1, &g1, &mut ws1, from, to);

    println!(
        "{:<40} {:>10} {:>10}",
        "линия огня на уровне пола", los0.clear, los2.clear
    );
    println!(
        "{:<40} {:>10} {:>10}",
        "линия огня над гребнем", los_hi0.clear, los_hi1.clear
    );
    println!("{:<40} {:>10} {:>10}", "маршрут найден", r0.found, r1.found);
    // Снос гребня открывает линию огня и маршрута не даёт: дно выемки
    // на двадцать вокселей ниже соседнего гребня. Дорогу открывает
    // коридор — и стоит он **минус два** перехода (ADR-143).
    println!(
        "{:<40} {:>10} {:>10}",
        "  после одного лишь сноса гребня", "", route_crest
    );
    println!(
        "{:<40} {:>10} {:>10}",
        "порталов в графе", st0.portals, st1.portals
    );
    // Не булева проходимость, а **рёбра**: над грядой небо, и клетка
    // на её гребне проходима что до подрыва, что после. Сцена, мерившая
    // клетки, показывала одно и то же число дважды (ADR-117, ADR-143).
    println!(
        "{:<40} {:>10} {:>10}",
        "проходимых нав-клеток",
        g0.passable(),
        g1.passable()
    );
    println!(
        "{:<40} {:>10} {:>10}",
        "переходов между клетками",
        g0.links(),
        g1.links()
    );
    println!(
        "{:<40} {:>10} {:>10}",
        "  из них после сноса гребня", "", links_crest
    );

    println!("\n{:<40} {:>12}", "работа", "");
    println!("{:<40} {:>12}", "снято на гребне", dug_crest);
    println!("{:<40} {:>12}", "снято в коридоре", dug_gap);
    println!("{:<40} {:>12.3}", "сборка графа до, мс", build0_ms);
    println!("{:<40} {:>12.3}", "сборка графа после, мс", build1_ms);
    println!("{:<40} {:>12}", "шагов луча", los2.steps);

    hash.feed(dug_crest);
    hash.feed(dug_gap);
    hash.feed(st1.portals as u64);
    hash.feed(g1.passable() as u64);
    hash.feed(g1.links() as u64);
    hash.feed(r1.cost as u64);
    hash.feed(los2.steps as u64);

    println!("\nхеш прогона: {:016x}", hash.get());
    if a.dump_hash {
        println!("seed={} side={side} hash={:016x}", a.seed, hash.get());
    }

    let mut ok = true;
    // Гряда обязана закрывать и огонь, и проход — иначе сцена ни о чём.
    if los0.clear {
        println!("гряда не закрыла линию огня — сцена ни о чём");
        ok = false;
    }
    if r0.found {
        println!("маршрут нашёлся до прокопки — гряда не перекрыла карту");
        ok = false;
    }
    // Срыв гребня обязан открыть верхнюю линию, коридор — нижнюю
    // и маршрут.
    if !los_hi1.clear {
        println!("гребень сорван, а линия над ним не открылась");
        ok = false;
    }
    if !los2.clear {
        println!("коридор прокопан, а линия огня по полу закрыта");
        ok = false;
    }
    if !r1.found {
        println!("коридор прокопан, а маршрут не нашёлся");
        ok = false;
    }
    // Промежуточная проверка: срыв гребня НЕ открывает линию по полу.
    if los1.clear {
        println!("срыв гребня открыл линию по полу — геометрия сцены неверна");
        ok = false;
    }

    // Порог берётся по ХУДШЕЙ из двух сборок: на нетронутой карте
    // граф не дешевле, а проверять надо обе.
    let worst_build = build0_ms.max(build1_ms);
    let verdict = if worst_build <= BUDGET_MS {
        "держится"
    } else {
        "ПРЕВЫШЕН"
    };
    println!("порог {BUDGET_MS} мс на перестройку графа: {worst_build:.3} мс — {verdict}");
    if worst_build > BUDGET_MS {
        ok = false;
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
