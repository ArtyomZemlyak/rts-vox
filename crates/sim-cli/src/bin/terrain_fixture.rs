//! Two strider lanes: level ground and a stepped depression authored in the
//! layout (range, ADR-671). The save holds the move orders just given; the
//! check runs on the restored save.
#[path = "../fixture.rs"]
mod fixture;
use fx::Fx;
fn main() {
    let path = fixture::path("out/walker-terrain/fixture.bin");
    let mut r = fixture::range(include_str!("../../../../data/fixture-terrain.ron"));
    r.step();
    for (unit, z) in [(0, 330), (1, 438)] {
        fixture::order_move(&mut r, unit, [580, z]);
    }
    let mut r = fixture::save(&path, &r);
    let mut csv = String::from(
        "tick,unit,x_raw,y_raw,z_raw,head,up_x,up_y,up_z,hull_y_raw,hull_vy_raw,load_raw,planted\n",
    );
    let mut heights = [(Fx::from_int(i32::MAX / 2), Fx::from_int(i32::MIN / 2)); 2];
    for _ in 0..420 {
        r.step();
        for (k, h) in heights.iter_mut().enumerate() {
            let m = &r.units[k].motion;
            h.0 = h.0.min(m.pos[1]);
            h.1 = h.1.max(m.pos[1]);
            let planted = m.feet.iter().filter(|f| f.planted()).count();
            csv.push_str(&format!(
                "{},{k},{},{},{},{},{},{},{},{},{},{},{planted}\n",
                r.tick,
                m.pos[0].raw(),
                m.pos[1].raw(),
                m.pos[2].raw(),
                m.drive.head,
                m.hull.up.x.raw(),
                m.hull.up.y.raw(),
                m.hull.up.z.raw(),
                m.hull.y.raw(),
                m.hull.vy.raw(),
                m.hull.load.raw()
            ));
        }
    }
    let drop = heights.map(|(lo, hi)| hi.sub(lo).to_int_trunc());
    println!(
        "height range per lane {drop:?} cells, lowest {:?}",
        heights.map(|h| h.0)
    );
    assert!(drop[0] <= 1, "the level lane changed height");
    // The floor of the depression is at 8: 0.8 m below the pad.
    assert!(
        heights[1].0 < Fx::from_int(9),
        "walker did not descend the test steps"
    );
    for i in 0..2 {
        assert!(
            r.units[i].motion.pos[0] > Fx::from_int(450),
            "walker {i} did not leave the depression: {:?}",
            r.units[i].motion.pos
        );
    }
    assert!(fixture::balanced(&r));
    std::fs::write(std::path::Path::new(&path).with_extension("csv"), csv).unwrap();
    println!("two walking lanes: height ranges {drop:?}; 420 ticks; balanced=true; save {path}");
}
