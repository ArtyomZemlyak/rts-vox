//! Инструмент, которым сделаны `.vox` в `data/` и `mods/`.
//!
//! Лежит в репозитории не для игры, а ради происхождения: бинарный файл,
//! про который неизвестно, чем он сделан, нельзя ни проверить, ни
//! пересобрать. `cargo run --release -p demo --bin mkdata` переписывает
//! их с нуля, и D-8 обязана после этого дать тот же хеш.
//!
//! Ванильный танк экспортируется **из `voxel::unit::tank()`**, то есть
//! из того самого тела, что зашито в движок. Это делает утверждение
//! «данные и код дают один и тот же юнит» проверяемым: D-8 сверяет
//! загруженный том с `tank()` воксель в воксель.

use modding::vox::{write, VoxModel};
use modding::PartTag;
use std::path::Path;
use voxel::unit::{tank, Part, UnitBody};

/// Номера цветов. Совпадают с `palette` в RON юнита — там же они
/// и объясняются человеку.
fn colour(p: Part) -> u8 {
    match p {
        Part::Armour => 1,
        Part::Core => 2,
        Part::Engine => 3,
        Part::Weapon => 4,
        Part::Sensor => 5,
    }
}

fn from_body(b: &UnitBody) -> VoxModel {
    let (nx, ny, nz) = (b.vol.nx, b.vol.ny, b.vol.nz);
    let mut voxels = Vec::new();
    for z in 0..nz as usize {
        for y in 0..ny as usize {
            for x in 0..nx as usize {
                if b.vol.get(x, y, z) {
                    voxels.push((x as u8, y as u8, z as u8, colour(b.part_at(x, y, z))));
                }
            }
        }
    }
    VoxModel {
        nx: nx as u32,
        ny: ny as u32,
        nz: nz as u32,
        voxels,
    }
}

/// Гриб-споровик: юнит мода. Ножка, шляпка, сенсор на макушке.
/// Строится процедурно — редактора здесь нет, а форма должна быть
/// заметно другой, иначе сцена ничего не покажет.
///
/// Размер — те же 3.5 × 4.0 × 3.5 м, что и раньше, выраженные новым
/// вокселем 100 мм (ADR-219): гриб остался прежним, сетка стала
/// вдвое с половиной мельче.
fn sporeling() -> VoxModel {
    let (nx, ny, nz) = (35u8, 40u8, 35u8);
    let (cx, cz) = (17i32, 17i32);
    let mut voxels = Vec::new();
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let (dx, dz) = (x as i32 - cx, z as i32 - cz);
                let r2 = dx * dx + dz * dz;
                // Ножка снизу, шляпка сверху.
                let stalk = y < 22 && r2 <= 56;
                let cap = (22..35).contains(&y) && r2 <= 225 - (y as i32 - 22) * 12;
                if !stalk && !cap {
                    continue;
                }
                let tag = if (13..22).contains(&x) && (15..22).contains(&y) && (13..22).contains(&z)
                {
                    PartTag::Core
                } else if y < 7 {
                    PartTag::Engine
                } else if cap && y == 22 && r2 >= 125 {
                    PartTag::Weapon
                } else if y >= 32 {
                    PartTag::Sensor
                } else {
                    PartTag::Armour
                };
                let c = match tag {
                    PartTag::Armour => 1,
                    PartTag::Core => 2,
                    PartTag::Engine => 3,
                    PartTag::Weapon => 4,
                    PartTag::Sensor => 5,
                };
                voxels.push((x, y, z, c));
            }
        }
    }
    VoxModel {
        nx: nx as u32,
        ny: ny as u32,
        nz: nz as u32,
        voxels,
    }
}

fn put(path: &str, bytes: &[u8]) {
    let p = Path::new(path);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).expect("каталог");
    }
    std::fs::write(p, bytes).expect("запись");
    println!("{path}: {} байт", bytes.len());
}

fn main() {
    let t = from_body(&tank());
    println!("танк: {} вокселей", t.voxels.len());
    put("data/units/tank.vox", &write(&t));

    let s = sporeling();
    println!("споровик: {} вокселей", s.voxels.len());
    put("mods/mushroom/units/sporeling.vox", &write(&s));
}
