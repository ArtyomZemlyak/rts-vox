//! Reproducible visual-QA save of a destroyed hull (range, ADR-671). Columns
//! through the roof and the core of a moving tank receive a diagnostic heat
//! dose, not the stock weapon's; a parked tank stays intact for comparison.
//! Melting, death and the hull's fate happen in the range's own ticks after
//! the window loads the save; the check runs on the restored save.
#[path = "../fixture.rs"]
mod fixture;
use std::collections::BTreeSet;
fn main() {
    let path = fixture::path("out/dead-hulls/fixture.bin");
    let mut r = fixture::range(include_str!("../../../../data/fixture-corpses.ron"));
    r.step();
    fixture::order_move(&mut r, 0, [540, 384]);
    fixture::order_move(&mut r, 1, [470, 440]);
    for _ in 0..180 {
        r.step();
    }
    // Every column the core occupies, from the roof down: parallel rays.
    let body = &r.units[0].body;
    let core = body.core;
    let core_role = body.part_at(
        usize::from(core.0),
        usize::from(core.1),
        usize::from(core.2),
    );
    let slices: BTreeSet<_> = fixture::root_cells(&r, 0, |[x, y, z]| {
        body.part_at(usize::from(x), usize::from(y), usize::from(z)) == core_role
    })
    .into_iter()
    .map(|[x, _, z]| (x, z))
    .collect();
    let cells = fixture::root_cells(&r, 0, |[x, _, z]| slices.contains(&(x, z)));
    fixture::melt(&mut r, 0, 0, &cells);
    let at = r.units[0].motion.pos.map(|v| v.to_int_trunc());
    let mut r = fixture::save(&path, &r);
    println!(
        "core penetration fixture: {} columns, {} heated cells, position {at:?}, save {path}",
        slices.len(),
        cells.len()
    );
    for _ in 0..180 {
        r.step();
    }
    let (pieces, section) = fixture::root_sections(&r, 0);
    let u = &r.units[0];
    println!(
        "after 180 ticks: alive {}, melted {}, hull sections {pieces} ({section} cells), left {} cells, fragments {}",
        u.status.alive,
        r.thermals.melted,
        u.body.solid_count(),
        r.matter.debris.fragments.len()
    );
    assert!(!u.status.alive, "the hull survived the core dose");
    assert!(pieces > 0, "the dead hull left no section");
    assert!(r.units[1].status.alive);
    assert_eq!(
        fixture::root_sections(&r, 1).0,
        0,
        "the parked tank lost a part"
    );
    assert!(fixture::balanced(&r));
    println!("corpse fixture verified: balanced=true");
}
