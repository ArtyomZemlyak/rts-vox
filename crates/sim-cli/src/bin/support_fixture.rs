//! Two stationary walkers: a hole between supports versus loss of all supports.
use sim_core::{Order, Reference, Tuning};
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "out/support-loss/fixture.bin".into());
    let t = Tuning::from_ron(include_str!("../../../../data/sim.ron")).unwrap();
    let weapons = modding::weapons::from_ron(include_str!("../../../../data/weapons.ron")).unwrap();
    let kinds = modding::kinds::from_ron(include_str!("../../../../data/kinds.ron"), 30).unwrap();
    let mats =
        modding::data::materials_from_ron(include_str!("../../../../data/materials.ron")).unwrap();
    let mut params = worldgen::Params::sized(768, 128, 768);
    params.relief = 1;
    let mut r = Reference::with_params(42, 0, t, weapons, params);
    r.world.attach_economy(kinds, mats);
    let mut ids = Vec::new();
    for z in [335, 445] {
        let id = r.world.spawn_kind(365, z, 0, 3).unwrap();
        ids.push(id as usize);
        r.world.push_order(Order::Hold { unit: id });
    }
    r.world.units.refresh_live();
    r.world.tick();
    let initial = ids
        .iter()
        .map(|&i| (r.world.units.body_y(i), r.world.units.voxels[i]))
        .collect::<Vec<_>>();
    let ground = r.world.ground_mut().unwrap();
    for (xs, zs) in [(350..390, 330..340), (350..390, 430..470)] {
        for x in xs {
            for z in zs.clone() {
                for y in 50..80 {
                    ground.world.queue_dig(x, y, z);
                }
            }
        }
    }
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    // The normal World drains these edits on its first tick after loading.
    std::fs::write(&path, sim_core::save::save(&r)).unwrap();
    let mut csv =
        String::from("tick,unit,nav_y,body_y_raw,vertical_speed_raw,load_raw,fall_from,voxels\n");
    let mut airborne = 0;
    for _ in 0..180 {
        r.world.tick();
        for (j, &i) in ids.iter().enumerate() {
            let u = &r.world.units;
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                r.world.tick,
                i,
                u.y[i],
                u.body_y(i).raw(),
                u.body_vy(i).raw(),
                u.load[i].raw(),
                u.fall_from[i],
                u.voxels[i]
            ));
            if j == 0 {
                assert!(u.body_y(i).sub(initial[j].0).abs() < fx::Fx::from_ratio(1, 100));
                assert_eq!(u.voxels[i], initial[j].1);
                assert_eq!(u.fall_from[i], sim_core::NOT_FALLING);
            } else if u.fall_from[i] != sim_core::NOT_FALLING {
                airborne += 1;
            }
        }
    }
    let i = ids[1];
    assert!(airborne > 10);
    assert!(r.world.units.body_y(i).sub(fx::Fx::from_int(50)).abs() < fx::Fx::from_ratio(1, 20));
    assert!(r.world.units.voxels[i] < initial[1].1);
    assert!(r.world.econ.as_ref().unwrap().balanced());
    std::fs::write(std::path::Path::new(&path).with_extension("csv"), csv).unwrap();
    println!("support-loss: left held without damage; right airborne={airborne} ticks, landed at 50; balanced=true");
}
