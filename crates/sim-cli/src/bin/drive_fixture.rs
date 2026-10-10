//! Three independent lanes: intact, turret lost, half the engine melted
//! (range, ADR-671). The damage is a diagnostic heat dose applied and
//! settled before the save; movement uses the shipped kind and chassis
//! data. The save holds the move orders just given; the check runs on the
//! restored save.
#[path = "../fixture.rs"]
mod fixture;
use fx::Fx;
fn main() {
    let path = fixture::path("out/damaged-drive/fixture.bin");
    let mut r = fixture::range(include_str!("../../../../data/fixture-drive.ron"));
    r.step();
    let plane = r.units[1].body.turret_y - 1;
    let attachment = fixture::root_cells(&r, 1, |p| p[1] == plane);
    fixture::melt(&mut r, 1, 0, &attachment);
    // Part::Engine is role 2 of the softening and impact tables. Cells near
    // the fuel reservoir stay cold: the dose must not light the fuel.
    let fuel = r.combustion.tuning.reservoir(r.units[2].kind).at;
    let engine: Vec<_> = fixture::root_cells(&r, 2, |[x, y, z]| {
        r.units[2]
            .body
            .part_at(usize::from(x), usize::from(y), usize::from(z)) as u8
            == 2
            && (0..3).any(|a| [x, y, z][a].abs_diff(fuel[a]) > 3)
    })
    .into_iter()
    .step_by(2)
    .collect();
    fixture::melt(&mut r, 2, 0, &engine);
    for _ in 0..120 {
        r.step();
    }
    assert_eq!(r.thermals.hot(), 0, "the dose did not settle");
    for (i, u) in r.units.iter().enumerate() {
        println!(
            "tank {i}: alive {}, cells {}, inverse mass {:?}, engine {:?}, traction {:?}, sections {:?}",
            u.status.alive,
            u.body.solid_count(),
            u.mobility.inverse_mass,
            u.mobility.engine,
            u.mobility.traction,
            fixture::root_sections(&r, i)
        );
        assert!(u.status.alive);
    }
    let m = |i: usize| r.units[i].mobility;
    assert!(
        m(1).inverse_mass > m(0).inverse_mass,
        "turret loss kept the mass"
    );
    assert_eq!(m(1).engine, Fx::ONE);
    assert!(m(2).engine > Fx::ZERO && m(2).engine < Fx::ONE);
    for i in 0..3 {
        let z = r.units[i].motion.pos[2].to_int_trunc();
        fixture::order_move(&mut r, i, [650, z]);
    }
    let mut r = fixture::save(&path, &r);
    let mut csv = String::from("tick,unit,cells,engine_pct,x_raw,speed_raw,yaw_rate\n");
    for tick in 0..240 {
        r.step();
        if [14, 29, 119, 239].contains(&tick) {
            for (i, u) in r.units.iter().enumerate() {
                let line = format!(
                    "{},{i},{},{},{},{},{}\n",
                    tick + 1,
                    u.body.solid_count(),
                    u.mobility.engine.mul(Fx::from_int(100)).to_int_trunc(),
                    u.motion.pos[0].raw(),
                    u.motion.drive.vel.raw(),
                    u.motion.drive.yaw_rate
                );
                print!("{line}");
                csv.push_str(&line);
            }
        }
    }
    let x = |i: usize| r.units[i].motion.pos[0];
    assert!(x(0) > x(2), "the damaged engine did not slow the tank");
    assert!(fixture::balanced(&r));
    std::fs::write(std::path::Path::new(&path).with_extension("csv"), csv).unwrap();
    println!("drive fixture verified: save {path}, balanced=true");
}
