//! Two independently commanded walkers pivot in opposite directions, then walk.
use sim_core::{Order, Reference, Tuning};
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "out/walker-turn/fixture.bin".into());
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
    for z in [330, 438] {
        let id = r.world.spawn_kind(410, z, 0, 3).unwrap();
        ids.push(id);
        r.world.push_order(Order::Hold { unit: id });
    }
    r.world.units.refresh_live();
    r.world.tick();
    for (&unit, z) in ids.iter().zip([360, 408]) {
        r.world.push_order(Order::MoveTo { unit, x: 270, z });
    }
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, sim_core::save::save(&r)).unwrap();
    let mut csv = String::from("tick,unit,x,z,head,gait,speed_raw,yaw_rate\n");
    let mut pivots = [0; 2];
    for tick in 0..300 {
        r.world.tick();
        for (k, &id) in ids.iter().enumerate() {
            let i = id as usize;
            let u = &r.world.units;
            if u.vel[i] == fx::Fx::ZERO && u.yaw_step[i] != 0 {
                pivots[k] += 1;
                assert_ne!(u.gait[i], 0);
            }
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                r.world.tick,
                id,
                u.x[i],
                u.z[i],
                u.head[i],
                u.gait[i],
                u.vel[i].raw(),
                u.yaw_rate[i]
            ));
        }
        if tick == 4 {
            assert!(pivots.iter().all(|&n| n > 0));
        }
    }
    for &id in &ids {
        assert!(r.world.units.x[id as usize] < 400);
    }
    assert!(r.world.econ.as_ref().unwrap().balanced());
    std::fs::write(std::path::Path::new(&path).with_extension("csv"), csv).unwrap();
    println!(
        "two walkers: stationary pivot ticks {:?}; 300 ticks; balanced=true",
        pivots
    );
}
