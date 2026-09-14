//! Two walking lanes: level ground and an excavated stepped depression.
use sim_core::{Order, Reference, Tuning};
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "out/walker-terrain/fixture.bin".into());
    let t = Tuning::from_ron(include_str!("../../../../data/sim.ron")).unwrap();
    let weapons = modding::weapons::from_ron(include_str!("../../../../data/weapons.ron")).unwrap();
    let kinds = modding::kinds::from_ron(include_str!("../../../../data/kinds.ron"), 30).unwrap();
    let mats =
        modding::data::materials_from_ron(include_str!("../../../../data/materials.ron")).unwrap();
    let mut params = worldgen::Params::sized(768, 128, 768);
    params.relief = 1;
    let mut r = Reference::with_params(42, 0, t, weapons, params);
    r.world.attach_economy(kinds, mats);
    let ground = r.world.ground_mut().unwrap();
    for x in 360..450 {
        let depth = if (380..430).contains(&x) { 8 } else { 4 };
        for z in 400..476 {
            let top = (1..ground.world.ny)
                .rev()
                .find(|&y| ground.world.solid(x, y - 1, z))
                .unwrap();
            for y in top - depth..top {
                ground.world.queue_dig(x, y, z);
            }
        }
    }
    while r.world.ground().unwrap().world.queued() > 0 {
        r.world.tick();
    }
    let mut ids = Vec::new();
    for z in [330, 438] {
        let id = r.world.spawn_kind(325, z, 0, 3).unwrap();
        ids.push(id);
        r.world.push_order(Order::Hold { unit: id });
    }
    r.world.units.refresh_live();
    r.world.tick();
    for (&unit, z) in ids.iter().zip([330, 438]) {
        r.world.push_order(Order::MoveTo { unit, x: 580, z });
    }
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, sim_core::save::save(&r)).unwrap();
    let mut csv =
        String::from("tick,unit,x,y,z,head,gait,up_x,up_y,up_z,body_y_raw,heave_raw,heave_v_raw\n");
    let mut heights = [(i32::MAX, i32::MIN); 2];
    for _ in 0..420 {
        r.world.tick();
        for (k, &id) in ids.iter().enumerate() {
            let i = id as usize;
            let u = &r.world.units;
            heights[k].0 = heights[k].0.min(u.y[i]);
            heights[k].1 = heights[k].1.max(u.y[i]);
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                r.world.tick,
                id,
                u.x[i],
                u.y[i],
                u.z[i],
                u.head[i],
                u.gait[i],
                u.up[i].x.raw(),
                u.up[i].y.raw(),
                u.up[i].z.raw(),
                u.body_y(i).raw(),
                u.heave[i].raw(),
                u.heave_v[i].raw()
            ));
        }
    }
    assert!(
        heights[1].1 - heights[1].0 >= 8,
        "walker did not descend the test steps"
    );
    for &id in &ids {
        assert!(
            r.world.units.x[id as usize] > 450,
            "walker did not leave the pit"
        );
    }
    assert!(r.world.econ.as_ref().unwrap().balanced());
    std::fs::write(std::path::Path::new(&path).with_extension("csv"), csv).unwrap();
    println!(
        "two walking lanes: heights {:?}; 420 ticks; balanced=true",
        heights
    );
}
