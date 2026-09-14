//! A saved, reproducible penetration test for the persistent wreck renderer.
//! Parallel material rays cut down through the roof and core, visible from the
//! RTS camera; this is a diagnostic impact, not the stock gun dose.
use fx::Fx;
use sim_core::{Order, Reference, Tuning};
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "out/dead-hulls/fixture.bin".into());
    let tuning = Tuning::from_ron(include_str!("../../../../data/sim.ron")).unwrap();
    let weapons = modding::weapons::from_ron(include_str!("../../../../data/weapons.ron")).unwrap();
    let kinds = modding::kinds::from_ron(include_str!("../../../../data/kinds.ron"), 30).unwrap();
    let mats =
        modding::data::materials_from_ron(include_str!("../../../../data/materials.ron")).unwrap();
    let mut params = worldgen::Params::sized(768, 128, 768);
    params.relief = 1;
    let mut r = Reference::with_params(42, 0, tuning, weapons, params);
    r.world.attach_economy(kinds, mats);
    let id = r.world.spawn_kind(230, 384, 0, 1).unwrap();
    let control = r.world.spawn_kind(470, 440, 0, 1).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: control });
    r.world.push_order(Order::MoveTo {
        unit: id,
        x: 540,
        z: 384,
    });
    for _ in 0..180 {
        r.world.tick();
    }
    let bodies = r.world.material_bodies.as_mut().unwrap();
    let body = bodies.shared_body(id).unwrap();
    let mut slices = std::collections::BTreeSet::new();
    for z in 0..body.vol.nz {
        for y in 0..body.vol.ny {
            for x in 0..body.vol.nx {
                if body.vol.get(x as usize, y as usize, z as usize)
                    && body.part_at(x as usize, y as usize, z as usize)
                        == body.part_at(
                            body.core.0 as usize,
                            body.core.1 as usize,
                            body.core.2 as usize,
                        )
                {
                    slices.insert((x, z));
                }
            }
        }
    }
    let mut energy = 0u64;
    for (x, z) in &slices {
        let a = [
            Fx::from_int(*x as i32).add(Fx::from_ratio(1, 2)),
            Fx::from_int(body.vol.ny as i32 + 1),
            Fx::from_int(*z as i32).add(Fx::from_ratio(1, 2)),
        ];
        let end = [a[0], Fx::from_int(-1), a[2]];
        let mut dose = 0;
        for y in 0..body.vol.ny as usize {
            if body.vol.get(*x as usize, y, *z as usize) {
                dose += tuning.impact.work_kj_m3[body.part_at(*x as usize, y, *z as usize) as usize]
                    as u64
                    * (body.mm as u64).pow(3)
                    / 1000;
            }
        }
        assert!(bodies.queue(id, a, end, dose, tuning.impact.max_pending as usize));
        energy += dose;
    }
    let bytes = sim_core::save::save(&r);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
    println!("core penetration fixture: {} parallel rays, {} mJ material work, position ({},{}), save {path}",slices.len(),energy,r.world.units.x[id as usize],r.world.units.z[id as usize]);
    for _ in 0..180 {
        r.world.tick();
    }
    assert!(!r.world.units.alive[id as usize]);
    assert_eq!(r.world.bursts.corpses.len(), 1);
    assert!(r.world.econ.as_ref().unwrap().balanced());
    println!(
        "fixture verified: remaining hull {} cells, {} fragments, damage queue {}",
        r.world.bursts.corpses.values().next().unwrap().count,
        r.world.bursts.fragments.len(),
        r.world.material_bodies.as_ref().unwrap().pending.len()
    );
}
