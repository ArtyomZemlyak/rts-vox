//! Reproducible visual QA save: a moving tank loses its turret attachment.
//! The cut is an artificial fixture, not a claim about the stock weapon power.
use sim_core::{Order, Reference, Tuning};
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "out/whole-sections/fixture.bin".into());
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
    let control = r.world.spawn_kind(470, 460, 0, 1).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: control });
    r.world.push_order(Order::MoveTo {
        unit: id,
        x: 540,
        z: 384,
    });
    for _ in 0..90 {
        r.world.tick();
    }
    let bodies = r.world.material_bodies.as_mut().unwrap();
    let body = bodies.body(id).unwrap();
    let plane = body.turret_y - 1;
    assert!(body.core.1 < plane);
    let mut cells = Vec::new();
    for z in 0..body.vol.nz {
        for x in 0..body.vol.nx {
            if body.vol.get(x as usize, plane as usize, z as usize) {
                cells.push([x, plane, z]);
            }
        }
    }
    let n = cells.len();
    let generation = bodies.generations[id as usize];
    let zero = bodies.frames(&r.world.units)[id as usize].unwrap()[0]
        .x
        .scale(fx::Fx::ZERO);
    for batch in cells.chunks(768) {
        assert!(bodies.queue_cells(
            id,
            generation,
            batch.to_vec(),
            u64::MAX / 4,
            0,
            zero,
            tuning.impact.max_pending
        ));
    }
    // Store before the next combat phase: removal, accounting, separation and
    // falling all happen through World.tick after the viewer loads the game.
    let bytes = sim_core::save::save(&r);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
    println!("fixture: moving tank {id}, attachment layer {plane}, requested cells {n}, position ({},{}), save {path}",r.world.units.x[id as usize],r.world.units.z[id as usize]);
}
