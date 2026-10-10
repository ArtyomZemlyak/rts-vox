//! Two independently commanded striders turn in opposite directions, then
//! walk (range, ADR-671). The save holds the orders just given; the check
//! runs on the restored save.
#[path = "../fixture.rs"]
mod fixture;
use fx::Fx;
fn main() {
    let path = fixture::path("out/walker-turn/fixture.bin");
    let mut r = fixture::range(include_str!("../../../../data/fixture-turn.ron"));
    r.step();
    // Targets behind each walker, on opposite sides of its heading.
    for (unit, z) in [(0, 360), (1, 408)] {
        fixture::order_move(&mut r, unit, [270, z]);
    }
    let mut r = fixture::save(&path, &r);
    let mut csv = String::from("tick,unit,x_raw,z_raw,head,speed_raw,yaw_rate,planted\n");
    let mut pivots = [0; 2];
    let heads = [0, 1].map(|i| r.units[i].motion.drive.head);
    for _ in 0..300 {
        r.step();
        for (k, pivot) in pivots.iter_mut().enumerate() {
            let m = &r.units[k].motion;
            if m.drive.vel == Fx::ZERO && m.drive.yaw_rate != 0 {
                *pivot += 1;
            }
            let planted = m.feet.iter().filter(|f| f.planted()).count();
            csv.push_str(&format!(
                "{},{k},{},{},{},{},{},{planted}\n",
                r.tick,
                m.pos[0].raw(),
                m.pos[2].raw(),
                m.drive.head,
                m.drive.vel.raw(),
                m.drive.yaw_rate
            ));
        }
    }
    let turned = [0, 1].map(|i| {
        let d = r.units[i].motion.drive.head - heads[i];
        d.rem_euclid(fx::angle::FULL)
    });
    println!("heads {heads:?} -> turned {turned:?}, pivot ticks {pivots:?}");
    assert!(pivots.iter().all(|&n| n > 0), "no stationary pivot");
    for i in 0..2 {
        assert!(
            r.units[i].motion.pos[0] < Fx::from_int(400),
            "walker {i} did not walk back: {:?}",
            r.units[i].motion.pos
        );
    }
    assert!(fixture::balanced(&r));
    std::fs::write(std::path::Path::new(&path).with_extension("csv"), csv).unwrap();
    println!(
        "two walkers: stationary pivot ticks {pivots:?}; 300 ticks; balanced=true; save {path}"
    );
}
