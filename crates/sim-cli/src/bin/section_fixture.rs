//! Reproducible visual-QA save: a moving tank loses its turret as a whole
//! section (range, ADR-260, ADR-671). The layer under the turret receives a
//! diagnostic heat dose, not the stock weapon's. Melting, separation, the
//! section's flight and its landing happen in the range's own ticks after
//! the window loads the save; the check runs on the restored save.
#[path = "../fixture.rs"]
mod fixture;
fn main() {
    let path = fixture::path("out/whole-sections/fixture.bin");
    let mut r = fixture::range(include_str!("../../../../data/fixture-sections.ron"));
    r.step();
    fixture::order_move(&mut r, 0, [540, 384]);
    fixture::order_move(&mut r, 1, [470, 460]);
    for _ in 0..90 {
        r.step();
    }
    let plane = r.units[0].body.turret_y - 1;
    assert!(r.units[0].body.core.1 < plane);
    let cells = fixture::root_cells(&r, 0, |p| p[1] == plane);
    fixture::melt(&mut r, 0, 0, &cells);
    let at = r.units[0].motion.pos.map(|v| v.to_int_trunc());
    let mut r = fixture::save(&path, &r);
    println!(
        "fixture: moving tank 0, attachment layer {plane}, heated cells {}, position {at:?}, save {path}",
        cells.len()
    );
    for _ in 0..240 {
        r.step();
    }
    let (pieces, section) = fixture::root_sections(&r, 0);
    let u = &r.units[0];
    println!(
        "after 240 ticks: alive {}, melted {}, root sections {pieces} ({section} cells), left {} cells, position {:?}",
        u.status.alive,
        r.thermals.melted,
        u.body.solid_count(),
        u.motion.pos.map(|v| v.to_int_trunc())
    );
    assert!(u.status.alive, "the turret cut killed the hull");
    assert!(
        pieces > 0 && section > 1000,
        "the turret did not leave whole"
    );
    assert_eq!(
        fixture::root_sections(&r, 1).0,
        0,
        "the parked tank lost a part"
    );
    assert!(fixture::balanced(&r));
    println!("section fixture verified: balanced=true");
}
