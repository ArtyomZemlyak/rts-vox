//! Confirmed wave cuts release saved, bounded, unpowered vegetation debris.
use super::*;
use voxel::{Edit, Generator};

struct Grove;
impl Generator for Grove {
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        for center in [24, 88] {
            if (center..center + 2).contains(&x) && (24..26).contains(&z) && (16..40).contains(&y) {
                return 5;
            }
            if (center - 4..center + 6).contains(&x)
                && (20..30).contains(&z)
                && (40..44).contains(&y)
            {
                return 4;
            }
        }
        0
    }
}
fn world() -> World {
    World::new(128, 96, 64, 4096, Box::new(Grove))
}
fn policy() -> Felling {
    let mut f = modding::navigation::Navigation::from_ron(include_str!(
        "../../../data/range-navigation.ron"
    ))
    .unwrap()
    .fell;
    f.cells_per_tick = 32;
    f
}

#[test]
fn nuclear_trunk_cuts_group_and_resume_as_unpowered_crown_debris() {
    let mut w = world();
    let mut m = Matter::new(&[]);
    m.begin(1_000_000, 1);
    let f = policy();
    for y in 32..36 {
        for z in 24..26 {
            for x in 24..26 {
                assert!(matches!(
                    m.ground(&mut w, [x, y, z], [0, 16, 24]),
                    ClaimResult::Applied
                ));
            }
        }
    }
    w.flush();
    m.debris.confirm(w.completed(), &mut m.economy, &m.tuning);
    let paid = m.spent_mj;
    let supplied = m.supplied_mj;
    m.release_nuclear_crowns(&w, &f, &[([8, 0, 8], [64, 96, 64])]);
    assert_eq!(m.felling_pending(), 5, "one shared seed, not sixteen");
    m.tuning.max_flying = 0;
    m.step_felling(&mut w, &f);
    assert!(m.felling_pending() > 0, "full pool discarded the crown");
    let mut bytes = Blob::new();
    m.save(&mut bytes);
    let mut restored = Matter::new(&[]);
    restored.load(&mut Scan::new(&bytes.v), &w, &[]).unwrap();
    restored.tuning.max_flying = 4096;
    restored.tuning.max_fragments = 4096;
    restored.tuning.max_claims = 4096;
    for _ in 0..200 {
        let visits = restored.admission.felling[1];
        restored.step_felling(&mut w, &f);
        assert!(restored.admission.felling[1] - visits <= 32);
        w.flush();
        restored
            .debris
            .confirm(w.completed(), &mut restored.economy, &restored.tuning);
        assert!(restored.economy.ledger.balanced());
        if restored.felling_pending() == 0 {
            break;
        }
    }
    assert_eq!(restored.felling_pending(), 0);
    assert_eq!(restored.spent_mj, paid, "crown charged as another blast");
    assert_eq!(restored.supplied_mj, supplied);
    for z in 20..30 {
        for y in 40..44 {
            for x in 20..30 {
                assert_eq!(w.material(x, y, z), 0, "near crown stayed hanging");
                assert_eq!(w.material(x + 64, y, z), 4, "remote tree was touched");
            }
        }
    }
    assert!(restored
        .debris
        .fragments
        .iter()
        .any(|piece| { piece.material == 4 && piece.velocity == V3::ZERO && !piece.resting }));
}

#[test]
fn nuclear_crown_release_ignores_leaves_outside_cuts_and_repaired_stems() {
    let mut w = world();
    w.queue_edit(Edit::dig_if(24, 41, 24, 4));
    w.queue_edit(Edit::dig_if(88, 35, 24, 5));
    w.queue_edit(Edit::dig_if(24, 35, 24, 5));
    w.queue_edit(Edit::put(24, 35, 24, 5));
    w.flush();
    let mut m = Matter::new(&[]);
    m.release_nuclear_crowns(&w, &policy(), &[([8, 0, 8], [64, 96, 64])]);
    assert_eq!(m.felling_pending(), 0);
}
