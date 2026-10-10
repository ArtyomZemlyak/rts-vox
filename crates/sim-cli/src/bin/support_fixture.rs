//! Two standing striders: a trench between the supports versus the loss of
//! all supports (range, ADR-671). The save holds the queued digs; the first
//! tick after loading removes the ground. The check runs on the restored
//! save. The range has no fall damage (stage H3, D-2): the second walker
//! falls and lands, its body unchanged.
#[path = "../fixture.rs"]
mod fixture;
use fx::Fx;
fn main() {
    let path = fixture::path("out/support-loss/fixture.bin");
    let mut r = fixture::range(include_str!("../../../../data/fixture-supports.ron"));
    // Stand: the feet take their first contacts.
    for _ in 0..30 {
        r.step();
    }
    for (i, u) in r.units.iter().enumerate() {
        let feet: Vec<_> = u
            .motion
            .feet
            .iter()
            .filter_map(|f| {
                f.contact
                    .map(|c| [c.pos.x, c.pos.z].map(|v| v.to_int_trunc()))
            })
            .collect();
        println!("walker {i} at {:?}, feet {feet:?}", u.motion.pos);
    }
    let initial: Vec<_> = r
        .units
        .iter()
        .map(|u| (u.motion.hull.y, u.body.solid_count()))
        .collect();
    // Ground is 1.6 m deep; the digs stop 0.2 m above the bottom of the map.
    let trench = (350..390).flat_map(|x| (330..340).map(move |z| (x, z)));
    let pit = (345..385).flat_map(|x| (425..465).map(move |z| (x, z)));
    let dug = fixture::dig(
        &mut r,
        trench
            .chain(pit)
            .flat_map(|(x, z)| (2..16).map(move |y| [x, y, z])),
    );
    let mut r = fixture::save(&path, &r);
    let mut csv = String::from("tick,unit,y_raw,hull_y_raw,hull_vy_raw,load_raw,planted,cells\n");
    let mut airborne = 0;
    for _ in 0..180 {
        r.step();
        for (i, u) in r.units.iter().enumerate() {
            let m = &u.motion;
            let planted = m.feet.iter().filter(|f| f.planted()).count();
            csv.push_str(&format!(
                "{},{i},{},{},{},{},{planted},{}\n",
                r.tick,
                m.pos[1].raw(),
                m.hull.y.raw(),
                m.hull.vy.raw(),
                m.hull.load.raw(),
                u.body.solid_count()
            ));
            if i == 0 {
                assert!(
                    m.hull.y.sub(initial[0].0).abs() < Fx::from_ratio(1, 100),
                    "the first walker sagged over its trench"
                );
                assert_eq!(u.body.solid_count(), initial[0].1);
            } else if m.hull.load == Fx::ZERO {
                airborne += 1;
            }
        }
    }
    let fell = initial[1].0.sub(r.units[1].motion.hull.y);
    println!(
        "dug {dug} cells; second walker airborne {airborne} ticks, fell {fell:?}, vy {:?}",
        r.units[1].motion.hull.vy
    );
    assert!(airborne > 10, "the second walker never left the ground");
    assert!(fell > Fx::from_int(10), "the second walker did not fall");
    assert!(
        r.units[1].motion.hull.vy.abs() < Fx::from_ratio(1, 10),
        "the second walker did not land"
    );
    assert!(fixture::balanced(&r));
    std::fs::write(std::path::Path::new(&path).with_extension("csv"), csv).unwrap();
    println!("support-loss: left held without damage; right airborne={airborne} ticks, fell {fell:?}; balanced=true; save {path}");
}
