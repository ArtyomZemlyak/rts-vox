//! **D-8 «Мод»** — контент грузится данными (`docs/14-demo-scenes.md`).
//!
//! ```text
//! d8_mod --seed 42 --dump-hash
//! ```
//!
//! Сцена проверяет не производительность, а **утверждение**: мод грузится
//! тем же путём, что ванильный контент, и не может обойти отложенные
//! очереди.
//!
//! Проверяется это четырьмя способами, и все четыре обязаны сойтись:
//!
//! 1. `data/` и `mods/mushroom/` читает одна и та же функция `load_pack`;
//! 2. загруженный из `data/` танк совпадает с `voxel::unit::tank()`
//!    **воксель в воксель** — то есть данные и код дают одно и то же тело;
//! 3. юнит мода воюет наравне: попадание в двигатель останавливает его,
//!    попадание в ядро убивает;
//! 4. правки мира от мода идут через ту же очередь и упираются в те же
//!    два потолка, что правки ядра.
//!
//! Плюс отрицательные проверки: столкновение номеров материалов — ошибка
//! загрузки, а не «последний побеждает»; путь из данных не выходит
//! за пределы пака.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use demo::RunHash;
use modding::{load_pack, merge_materials, Pack, PartTag, UnitModel};
use voxel::store::Mat;
use voxel::topo::unit::CoreBufs;
use voxel::unit::{tank, Part, UnitBody};
use voxel::{Generator, World};

struct Args {
    seed: u64,
    root: PathBuf,
    dump_hash: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            seed: 42,
            root: PathBuf::from("."),
            dump_hash: false,
        }
    }
}

fn parse() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        match k.as_str() {
            "--seed" => {
                a.seed = it
                    .next()
                    .ok_or("--seed: нет значения")?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            "--root" => a.root = PathBuf::from(it.next().ok_or("--root: нет значения")?),
            "--dump-hash" => a.dump_hash = true,
            "--help" | "-h" => {
                println!("D-8 «Мод»\n\n  --seed N --root КАТАЛОГ --dump-hash\n");
                std::process::exit(0);
            }
            o => return Err(format!("неизвестный ключ: {o}")),
        }
    }
    Ok(a)
}

/// Отображение метки из данных в роль воксела движка.
///
/// Единственное место, где два перечисления встречаются. `match` без
/// ветки `_` — если в данных появится метка, которой нет в движке,
/// это будет ошибка сборки, а не молча броня.
fn part_of(t: PartTag) -> Part {
    match t {
        PartTag::Armour => Part::Armour,
        PartTag::Core => Part::Core,
        PartTag::Engine => Part::Engine,
        PartTag::Weapon => Part::Weapon,
        PartTag::Sensor => Part::Sensor,
    }
}

/// Собрать движковый том из загруженной модели.
///
/// Ядро сажается **после** расстановки вокселей и по тому же правилу,
/// что у зашитого в код танка. Другого правила нет — в этом весь смысл.
fn body_of(m: &UnitModel) -> Result<UnitBody, String> {
    let mut b = UnitBody::new(m.nx, m.ny, m.nz, m.core_size);
    for &(x, y, z, tag) in &m.voxels {
        b.set(x as usize, y as usize, z as usize, part_of(tag));
    }
    if !b.seat_core() {
        return Err(format!("{}: ядро не садится на разметку", m.name));
    }
    for &(min, max, axis) in &m.weapon_paths {
        b.push_weapon_path(voxel::unit::WeaponPath { min, max, axis });
    }
    Ok(b)
}

/// Совпадают ли два тома целиком: и геометрия, и роли.
fn same_body(a: &UnitBody, b: &UnitBody) -> bool {
    if (a.vol.nx, a.vol.ny, a.vol.nz) != (b.vol.nx, b.vol.ny, b.vol.nz) {
        return false;
    }
    if (a.core, a.core_size) != (b.core, b.core_size) {
        return false;
    }
    if a.weapon_paths() != b.weapon_paths() {
        return false;
    }
    for z in 0..a.vol.nz as usize {
        for y in 0..a.vol.ny as usize {
            for x in 0..a.vol.nx as usize {
                if a.vol.get(x, y, z) != b.vol.get(x, y, z) {
                    return false;
                }
                if a.vol.get(x, y, z) && a.part_at(x, y, z) != b.part_at(x, y, z) {
                    return false;
                }
            }
        }
    }
    true
}

/// Выбить у юнита все воксели указанной роли.
fn shoot_out(b: &mut UnitBody, p: Part) -> u32 {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    let mut hit = 0;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                if b.vol.get(x, y, z) && b.part_at(x, y, z) == p {
                    b.remove(x, y, z);
                    hit += 1;
                }
            }
        }
    }
    hit
}

/// Мир из одного материала: сцене нужна не генерация, а очередь.
struct Solid;
impl Generator for Solid {
    fn material(&self, _x: u32, _y: u32, _z: u32) -> Mat {
        2
    }
}

fn ok(v: bool) -> &'static str {
    if v {
        "да"
    } else {
        "НЕТ"
    }
}

fn run() -> Result<bool, String> {
    let a = parse()?;
    println!("D-8 «Мод» — контент грузится данными\n");

    let vanilla_dir = a.root.join("data");
    let mod_dir = a.root.join("mods").join("mushroom");

    // --- 1. Один путь загрузки.
    let load = |d: &Path| -> Result<Pack, String> {
        load_pack(d).map_err(|e| format!("{}: {e}", d.display()))
    };
    let vanilla = load(&vanilla_dir)?;
    let modded = load(&mod_dir)?;

    println!("{:<44} {:>10} {:>10}", "пак", "материалов", "юнитов");
    for p in [&vanilla, &modded] {
        println!(
            "{:<44} {:>10} {:>10}",
            p.name,
            p.materials.len(),
            p.units.len()
        );
    }

    // --- 2. Реестр: с модом и без.
    let plain = merge_materials(std::slice::from_ref(&vanilla)).map_err(|e| e.to_string())?;
    // Порядок паков не должен влиять ни на что, кроме сообщения об ошибке.
    let both = merge_materials(&[vanilla, modded]).map_err(|e| e.to_string())?;
    let (h_plain, h_both) = (plain.hash(), both.hash());
    let mod_changes_hash = h_plain != h_both;

    println!("\n{:<44} {:>21}", "реестр материалов", "");
    println!("{:<44} {:>21}", "  без мода, материалов", plain.len());
    println!("{:<44} {:>21}", "  с модом, материалов", both.len());
    println!("{:<44} {:>21x}", "  хеш без мода", h_plain);
    println!("{:<44} {:>21x}", "  хеш с модом", h_both);
    println!(
        "{:<44} {:>21}",
        "  мод меняет хеш партии",
        ok(mod_changes_hash)
    );

    // Порядок загрузки паков не влияет на хеш.
    let v2 = load(&vanilla_dir)?;
    let m2 = load(&mod_dir)?;
    let swapped = merge_materials(&[m2, v2]).map_err(|e| e.to_string())?;
    let order_free = swapped.hash() == h_both;
    println!("{:<44} {:>21}", "  порядок паков не влияет", ok(order_free));

    // Столкновение номеров — ошибка, а не переопределение.
    let m3 = load(&mod_dir)?;
    let m4 = load(&mod_dir)?;
    let clash = merge_materials(&[m3, m4]).is_err();
    println!("{:<44} {:>21}", "  тот же мод дважды — ошибка", ok(clash));

    // --- 3. Данные и код дают один и тот же танк.
    let packs = [load(&vanilla_dir)?, load(&mod_dir)?];
    let tank_model = packs[0]
        .units
        .iter()
        .find(|u| u.name == "Танк")
        .ok_or("в ванильном паке нет танка")?;
    let from_data = body_of(tank_model)?;
    let from_code = tank();
    let identical = same_body(&from_data, &from_code);

    println!("\n{:<44} {:>10} {:>10}", "танк", "из данных", "из кода");
    println!(
        "{:<44} {:>10} {:>10}",
        "  размер",
        format!(
            "{}×{}×{}",
            from_data.vol.nx, from_data.vol.ny, from_data.vol.nz
        ),
        format!(
            "{}×{}×{}",
            from_code.vol.nx, from_code.vol.ny, from_code.vol.nz
        )
    );
    println!(
        "{:<44} {:>10} {:>10}",
        "  вокселей",
        from_data.solid_count(),
        from_code.solid_count()
    );
    println!(
        "{:<44} {:>10} {:>10}",
        "  ядро",
        format!("{:?}", from_data.core),
        format!("{:?}", from_code.core)
    );
    println!(
        "{:<44} {:>21}",
        "  совпадают воксель в воксель",
        ok(identical)
    );

    // --- 4. Юнит мода воюет наравне.
    let spore_model = packs[1]
        .units
        .iter()
        .find(|u| u.name == "Споровик")
        .ok_or("в моде нет споровика")?;
    let spore = body_of(spore_model)?;
    let mut bufs = CoreBufs::default();

    println!("\n{:<44} {:>21}", "юнит мода", spore_model.name);
    println!(
        "{:<44} {:>21}",
        "  размер",
        format!("{}×{}×{}", spore.vol.nx, spore.vol.ny, spore.vol.nz)
    );
    println!("{:<44} {:>21}", "  вокселей", spore.solid_count());

    let intact = spore.status(&mut bufs);
    let mut hurt = spore.clone();
    let engine_hit = shoot_out(&mut hurt, Part::Engine);
    let after_engine = hurt.status(&mut bufs);
    let mut dead = spore.clone();
    let core_hit = shoot_out(&mut dead, Part::Core);
    let after_core = dead.status(&mut bufs);

    println!(
        "{:<44} {:>6} {:>7} {:>6}",
        "  состояние", "цел", "-двиг", "-ядро"
    );
    println!(
        "{:<44} {:>6} {:>7} {:>6}",
        "    жив",
        ok(intact.alive),
        ok(after_engine.alive),
        ok(after_core.alive)
    );
    println!(
        "{:<44} {:>6} {:>7} {:>6}",
        "    двигатель",
        ok(intact.engine),
        ok(after_engine.engine),
        ok(after_core.engine)
    );
    println!(
        "{:<44} {:>6} {:>7} {:>6}",
        "    орудие",
        ok(intact.weapon),
        ok(after_engine.weapon),
        ok(after_core.weapon)
    );
    println!(
        "{:<44} {:>21}",
        "  снято вокселей двигателя / ядра",
        format!("{engine_hit} / {core_hit}")
    );

    let combat_ok = intact.alive
        && intact.engine
        && intact.weapon
        && after_engine.alive
        && !after_engine.engine
        && !after_core.alive
        && engine_hit > 0
        && core_hit > 0;

    // --- 5. Мод не обходит очередь.
    //
    // «Правки мода» здесь — просто правки, поставленные через тот же
    // публичный вход. Другого входа нет, и это ровно то, что сцена
    // обязана показать: обойти очередь нельзя, потому что мимо неё
    // писать не через что.
    let mut w = World::new(128, 64, 128, 64, Box::new(Solid));
    w.max_materialize_per_tick = 1;
    let want = 400usize;
    let mut rng = fx::rng::Rng::new(a.seed, 0xD08);
    let mut asked = Vec::with_capacity(want);
    for _ in 0..want {
        let (x, y, z) = (rng.below(120) + 4, rng.below(56) + 4, rng.below(120) + 4);
        w.queue_dig(x, y, z);
        asked.push((x, y, z));
    }
    let mut hash = RunHash::new();
    let mut ticks = 0u64;
    let mut worst_applied = 0usize;
    let mut worst_mat = 0usize;
    while w.queued() > 0 && ticks < 10_000 {
        ticks += 1;
        let st = w.flush();
        worst_applied = worst_applied.max(st.applied);
        worst_mat = worst_mat.max(st.materialized);
        hash.feed_edits(ticks, w.edits());
    }
    let all_landed = asked.iter().all(|&(x, y, z)| w.material(x, y, z) == 0);

    println!("\n{:<44} {:>21}", "правки мода через очередь", "");
    println!("{:<44} {:>21}", "  поставлено", want);
    println!("{:<44} {:>21}", "  тиков на дренаж", ticks);
    println!(
        "{:<44} {:>21}",
        "  правок за тик, максимум",
        format!("{worst_applied} при потолке {}", w.max_edits_per_tick)
    );
    println!(
        "{:<44} {:>21}",
        "  чанков за тик, максимум",
        format!("{worst_mat} при потолке {}", w.max_materialize_per_tick)
    );
    println!("{:<44} {:>21}", "  все правки применены", ok(all_landed));

    let queue_respected = worst_applied <= w.max_edits_per_tick
        && worst_mat <= w.max_materialize_per_tick
        && ticks > 1
        && all_landed;

    // --- 6. Хеш воспроизводим.
    let h1 = hash.get();
    let mut w2 = World::new(128, 64, 128, 64, Box::new(Solid));
    w2.max_materialize_per_tick = 1;
    let mut rng2 = fx::rng::Rng::new(a.seed, 0xD08);
    for _ in 0..want {
        let (x, y, z) = (rng2.below(120) + 4, rng2.below(56) + 4, rng2.below(120) + 4);
        w2.queue_dig(x, y, z);
    }
    let mut hash2 = RunHash::new();
    let mut t2 = 0u64;
    while w2.queued() > 0 && t2 < 10_000 {
        t2 += 1;
        w2.flush();
        hash2.feed_edits(t2, w2.edits());
    }
    let reproducible = hash2.get() == h1;

    // --- 7. Путь из данных не выходит за пределы пака.
    let escape = std::fs::read_to_string(mod_dir.join("units/sporeling.ron"))
        .map_err(|e| e.to_string())?
        .replace("units/sporeling.vox", "../../data/units/tank.vox");
    let tmp = std::env::temp_dir().join("rts-vox-d8-escape");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("units")).map_err(|e| e.to_string())?;
    std::fs::write(tmp.join("units/x.ron"), escape).map_err(|e| e.to_string())?;
    let escaped = matches!(modding::load_pack(&tmp), Err(modding::LoadError::Escape(_)));
    let _ = std::fs::remove_dir_all(&tmp);

    println!("\n{:<44} {:>21x}", "хеш прогона", h1);
    println!("{:<44} {:>21}", "  воспроизводим", ok(reproducible));
    println!("{:<44} {:>21}", "путь наружу пака отвергнут", ok(escaped));

    if a.dump_hash {
        println!("seed={} registry={:016x} hash={:016x}", a.seed, h_both, h1);
    }

    let all = mod_changes_hash
        && order_free
        && clash
        && identical
        && combat_ok
        && queue_respected
        && reproducible
        && escaped;
    println!(
        "\nмод грузится как ванильный контент и не обходит очередь: {}",
        if all { "ДА" } else { "НЕТ" }
    );
    Ok(all)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("D-8: {e}");
            ExitCode::FAILURE
        }
    }
}
