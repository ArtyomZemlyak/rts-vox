//! The battle motion phase must retain contact work after a hull leaves.
use super::battle::Order;
use super::*;
use modding::range::{RangeBlock, RangeUnitDef};

#[test]
fn battle_hull_cuts_finish_after_departure_and_snapshot_restore() {
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size = [768, 128, 768];
    l.clear_min = [40, 40];
    l.clear_max = [728, 728];
    l.course = None;
    l.shields.clear();
    l.blocks = vec![RangeBlock {
        min: [120, 16, 124],
        max: [136, 32, 132],
        material: 4,
    }];
    l.units = (0..6)
        .map(|i| RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: if i == 0 {
                [80, 16, 128]
            } else {
                [300 + 60 * i, 16, 500]
            },
            allied: i < 5,
            weapon: None,
        })
        .collect();
    let mut a = BlastRange::with_layout(config, l).unwrap();
    assert!(a.set_navigation_workers(4));
    a.start_battle().unwrap();
    a.matter.tuning.max_flying = 0; // Deterministic fully occupied pool fixture.
    assert!(a.battle_order(0, Order::Move, Some([250, 128])));
    for _ in 0..180 {
        a.step();
        if !a.units[0].motion.cut.is_empty() {
            break;
        }
    }
    let touched = a.units[0].motion.cut.clone();
    assert!(
        !touched.is_empty(),
        "battle motion never touched the vegetation"
    );
    assert!(
        a.matter.felling_pending() >= touched.len(),
        "battle dropped rejected contact cells"
    );
    assert!(touched
        .iter()
        .all(|p| a.world.material(p[0], p[1], p[2]) == 4));
    // Isolate departure: no subsequent overlap can resubmit these contacts.
    a.units[0].motion.stop();
    a.units[0].motion.enabled = false;
    a.units[0].motion.pos[0] = Fx::from_int(250);
    a.units[0].update_pose(0);
    let bytes = a.snapshot();
    let mut b = BlastRange::restore(config, &bytes).unwrap();
    assert_eq!(b.snapshot(), bytes);
    assert!(b.set_navigation_workers(1));
    for r in [&mut a, &mut b] {
        r.matter.tuning.max_flying = 4096;
    }
    for _ in 0..8 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        assert!(a.matter.economy.ledger.balanced());
    }
    assert!(
        touched
            .iter()
            .all(|p| a.world.material(p[0], p[1], p[2]) == 0),
        "departed tank left contacted plants standing"
    );
    assert!(a.removed_materials[4] >= touched.len());
}
