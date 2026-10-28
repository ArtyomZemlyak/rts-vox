//! Offline probe linked against the actual game importer and UnitBody.
//! See docs/analysis/401-art-tool-pipeline.md for the bounded build command.
use std::path::Path;

use modding::PartTag;
use voxel::topo::unit::CoreBufs;
use voxel::{Part, UnitBody};

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("--vox or --pack");
    for path in args {
        if mode == "--vox" {
            match modding::vox::read(&std::fs::read(&path).unwrap()) {
                Ok(model) => println!(
                    "VOX {path}: {}x{}x{} {} cells",
                    model.nx,
                    model.ny,
                    model.nz,
                    model.voxels.len()
                ),
                Err(error) => panic!("{path}: {error:?}"),
            }
            continue;
        }
        assert_eq!(mode, "--pack");
        let pack = modding::load_pack(Path::new(&path)).unwrap();
        assert!(!pack.units.is_empty());
        for model in pack.units {
            let mut body = UnitBody::new(model.nx, model.ny, model.nz, model.core_size);
            for &(x, y, z, tag) in &model.voxels {
                let part = match tag {
                    PartTag::Armour => Part::Armour,
                    PartTag::Core => Part::Core,
                    PartTag::Engine => Part::Engine,
                    PartTag::Weapon => Part::Weapon,
                    PartTag::Sensor => Part::Sensor,
                };
                body.set(x as usize, y as usize, z as usize, part);
            }
            assert!(body.seat_core(), "invalid core cube: {path}");
            for &(min, max, axis) in &model.weapon_paths {
                body.push_weapon_path(voxel::unit::WeaponPath { min, max, axis });
            }
            for &(min, max, axis) in &model.load_paths {
                body.push_load_path(voxel::unit::WeaponPath { min, max, axis });
            }
            let healthy = body.status(&mut CoreBufs::new());
            assert!(healthy.alive && healthy.engine && healthy.weapon && healthy.sensor);
            assert_eq!(healthy.detached, 0, "disconnected imported geometry");
            assert_eq!(healthy.attached, model.voxels.len() as u32);
            let mut cut_engine = body.clone();
            let mut cut_core = body.clone();
            for &(x, y, z, tag) in &model.voxels {
                if tag == PartTag::Engine {
                    cut_engine.remove(x as usize, y as usize, z as usize);
                }
                if tag == PartTag::Core {
                    cut_core.remove(x as usize, y as usize, z as usize);
                }
            }
            let damaged = cut_engine.status(&mut CoreBufs::new());
            assert!(damaged.alive && !damaged.engine && damaged.weapon);
            assert!(!cut_core.status(&mut CoreBufs::new()).alive);
            println!(
                "PACK {path}: {} cells; all subsystems connected; engine/core removal PASS",
                healthy.attached
            );
        }
    }
}
