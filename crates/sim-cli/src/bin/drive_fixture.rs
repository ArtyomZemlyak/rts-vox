//! Three independent lanes: intact, detached turret, partly damaged engine.
//! Damage is diagnostic; movement uses the shipped type and chassis data.
use fx::Fx;
use sim_core::{Order, Reference, Tuning};
fn main() {
    let stress = std::env::args().any(|a| a == "--stress");
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "out/damaged-drive/fixture.bin".into());
    let t = Tuning::from_ron(include_str!("../../../../data/sim.ron")).unwrap();
    let weapons = modding::weapons::from_ron(include_str!("../../../../data/weapons.ron")).unwrap();
    let kinds = modding::kinds::from_ron(include_str!("../../../../data/kinds.ron"), 30).unwrap();
    let mats =
        modding::data::materials_from_ron(include_str!("../../../../data/materials.ron")).unwrap();
    let mut params = worldgen::Params::sized(768, 128, 768);
    params.relief = 1;
    let mut r = Reference::with_params(42, 0, t, weapons, params);
    r.world.attach_economy(kinds, mats);
    let mut units = Vec::new();
    for z in [260, 384, 508] {
        let id = r.world.spawn_kind(280, z, 0, 1).unwrap();
        units.push(id);
        r.world.push_order(Order::Hold { unit: id });
    }
    r.world.units.refresh_live();
    r.world.tick();
    let b = r.world.material_bodies.as_mut().unwrap();
    let template = b.shared_body(units[0]).unwrap();
    let mut stress_cell = [0u8; 3];
    let mut attachment = Vec::new();
    let mut engine = Vec::new();
    for z in 0..template.vol.nz {
        for y in 0..template.vol.ny {
            for x in 0..template.vol.nx {
                if !template.vol.get(x as usize, y as usize, z as usize) {
                    continue;
                }
                if template.part_at(x as usize, y as usize, z as usize) as u8 == 0 {
                    stress_cell = [x, y, z];
                }
                if y == template.turret_y - 1 {
                    attachment.push([x, y, z]);
                }
                // Part::Engine is the material-role index 2 used by the impact table.
                if template.part_at(x as usize, y as usize, z as usize) as u8 == 2 {
                    engine.push([x, y, z]);
                }
            }
        }
    }
    let zero = b.frames(&r.world.units)[0].unwrap()[0].x.scale(Fx::ZERO);
    for (id, cells) in [
        (units[1], attachment),
        (units[2], engine.into_iter().step_by(2).collect()),
    ] {
        for batch in cells.chunks(768) {
            assert!(b.queue_cells(
                id,
                b.generations[id as usize],
                batch.to_vec(),
                u64::MAX / 8,
                0,
                zero,
                t.impact.max_pending
            ));
        }
    }
    for _ in 0..120 {
        r.world.tick();
    }
    let b = r.world.material_bodies.as_ref().unwrap();
    assert_eq!(b.pending.len(), 0);
    assert_eq!(b.detach_pending(), 0);
    assert!(b.state(units[1]).unwrap().mobility.inverse_mass > Fx::ONE);
    assert_eq!(b.state(units[1]).unwrap().mobility.engine, Fx::ONE);
    assert!(
        b.state(units[2]).unwrap().mobility.engine > Fx::ZERO
            && b.state(units[2]).unwrap().mobility.engine < Fx::ONE
    );
    for &id in &units {
        assert!(r.world.units.alive[id as usize]);
        r.world.push_order(Order::MoveTo {
            unit: id,
            x: 650,
            z: r.world.units.z[id as usize],
        });
    }
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, sim_core::save::save(&r)).unwrap();
    let mut csv = String::from("tick,unit,mass_g,engine_pct,x_raw,speed_raw,yaw_rate\n");
    r.world.phase_ns.fill(0);
    for tick in 0..240 {
        r.world.tick();
        if tick == 14 || tick == 29 || tick == 119 || tick == 239 {
            for &id in &units {
                let s = r.world.material_bodies.as_ref().unwrap().state(id).unwrap();
                let mass =
                    s.shape.count() as u64 * (template.mm as u64).pow(3) * t.tracked.density as u64
                        / 1_000_000;
                let pct = s.mobility.engine.mul(Fx::from_int(100)).to_int_trunc();
                let x =
                    Fx::from_int(r.world.units.x[id as usize]).add(r.world.units.fx[id as usize]);
                let line = format!(
                    "{},{id},{mass},{pct},{},{},{}\n",
                    tick + 1,
                    x.raw(),
                    r.world.units.vel[id as usize].raw(),
                    r.world.units.yaw_rate[id as usize]
                );
                print!("{line}");
                csv.push_str(&line);
            }
        }
    }
    assert!(r.world.units.x[units[0] as usize] > r.world.units.x[units[2] as usize]);
    assert!(r.world.econ.as_ref().unwrap().balanced());
    std::fs::write(std::path::Path::new(&path).with_extension("csv"), csv).unwrap();
    println!(
        "drive fixture verified: save {path}, movement {} ns/tick, balanced=true",
        r.world.phase_ns_of(sim_core::Phase::Movement) / 240
    );
    if stress {
        // A CPU movement workload with real damaged roots and no terrain/war.
        // It measures cache use, not the complete 1 km renderer or mass combat.
        let mut w = sim_core::World::new(42, 5000, t);
        let mut bodies = sim_core::damage::Bodies::new(vec![(*template).clone()]);
        let cell = stress_cell;
        for i in 0..5000 {
            let id = w
                .units
                .spawn(100 + (i % 100) * 90, 100 + (i / 100) * 90, 0, 24, 300);
            bodies.spawn_at(id, 0);
            // Split the one-cell edits across the usual bounded queue.
            if !bodies.queue_cells(
                id,
                bodies.generations[id as usize],
                vec![cell],
                u64::MAX / 8,
                0,
                zero,
                t.impact.max_pending,
            ) {
                panic!("stress input exceeded pending capacity");
            }
            if i % 1000 == 999 {
                bodies.step(&t.impact.work_kj_m3, t.impact.cells_per_tick, 1000);
            }
        }
        assert_eq!(bodies.pending.len(), 0);
        assert!(bodies.state(4999).unwrap().mobility.inverse_mass > Fx::ONE);
        w.material_bodies = Some(bodies);
        w.units.refresh_live();
        for _ in 0..30 {
            w.tick();
        }
        w.phase_ns.fill(0);
        let start = std::time::Instant::now();
        for _ in 0..120 {
            w.tick();
        }
        println!("stress 5000 damaged roots: movement {} us/tick, whole workload {} us/tick; no terrain or opposing army",
            w.phase_ns_of(sim_core::Phase::Movement)/120/1000,start.elapsed().as_micros()/120);
    }
}
