//! Valid data must not raise the range's whole-map work ceiling (review 252).
use crate::blast::{battle::Order, BlastRange};
use fx::Fx;
use modding::explosion::ExplosionConfig;
use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};

const GOAL: [i32; 2] = [880, 880];

fn install_budget(r: &mut BlastRange) {
    let text = include_str!("../../../data/range-navigation.ron")
        .replace("global_cell_units: 32,", "global_cell_units: 128,")
        .replace(
            "global_work_per_tick: 2097152,",
            "global_work_per_tick: 1024,",
        )
        .replace(
            "global_work_per_search: 262144,",
            "global_work_per_search: 1024,",
        );
    let n = modding::navigation::Navigation::from_ron(&text).expect("legal navigation data");
    assert_eq!((n.global_cell_units, n.global_work_per_tick), (128, 1024));
    r.navigation = n.clone();
    for u in &mut r.units {
        u.motion.navigation = n.clone();
    }
}

fn range(kind: RangeUnitKind) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size = [1024, 128, 1024];
    l.clear_max = [960, 960];
    l.units = vec![RangeUnitDef {
        kind,
        allied: true,
        at: [280, 16, 280],
        weapon: None,
    }];
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, l).unwrap();
    install_budget(&mut r);
    r.start_free_control().unwrap();
    assert!(r.battle_order(0, Order::Move, Some(GOAL)));
    assert_eq!(r.units[0].motion.coarse_waited(), Some(0));
    assert_eq!(r.coarse_work, 0, "the order does no whole-map work");
    r
}

#[test]
fn a_legal_small_tick_budget_is_never_raised_to_the_oldest_tests_price() {
    let mut r = range(RangeUnitKind::Tank);
    assert!(r.units[0].motion.coarse_max_test() > r.navigation.global_work_per_tick);
    let mut answered = false;
    for _ in 0..16 {
        let before = r.coarse_work;
        r.step();
        let spent = r.coarse_work - before;
        assert!(
            spent <= r.navigation.global_work_per_tick,
            "tick {} spent {spent}, budget {}",
            r.tick,
            r.navigation.global_work_per_tick
        );
        if r.units[0].motion.coarse_waited().is_none() {
            answered = true;
            break;
        }
    }
    assert!(answered, "a small but affordable search starved");
    assert_eq!(r.units[0].motion.destination, Some(GOAL));
    assert!(!r.units[0].motion.path.is_empty());
}

#[test]
fn a_corridor_larger_than_a_full_tick_falls_back_without_stalling_or_losing_the_order() {
    let mut a = range(RangeUnitKind::Spider);
    let shape = a.units[0].motion.probe_shape(&a.units[0].body);
    let radius = shape.half[0].min(shape.half[1]) + 1;
    // First diagonal section: (280,280) -> (400,400), local grade reads
    // three times per tile. Even one such check cannot fit this legal tick.
    let side = ((400 + radius) / 8 - (280 - radius) / 8 + 1) as u64;
    assert!(
        side * side * 3 > a.navigation.global_work_per_tick,
        "shape half={:?}",
        shape.half
    );
    a.step();
    assert_eq!(a.coarse_work, 0, "an unaffordable atomic test is never run");
    assert_eq!(
        a.units[0].motion.coarse_waited(),
        None,
        "no permanently pending coarse search"
    );
    assert_eq!(a.units[0].motion.destination, Some(GOAL));
    assert!(
        a.units[0].motion.hunting() || !a.units[0].motion.path.is_empty(),
        "fine planner owns the order"
    );
    let bytes = a.snapshot();
    assert_ne!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 66);
    let mut b = BlastRange::restore(a.config, &bytes).unwrap();
    install_budget(&mut b);
    assert_eq!(b.snapshot(), bytes);
    for _ in 0..12 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
    }
}

/// The other hulls are listed only when a search hands its order to the
/// fine planner (review 252): a search walking on, or holding with no
/// share, never asks for them, and the hand-over given the list lazily
/// leaves the hull exactly as the list built up front did.
#[test]
fn the_obstacle_list_is_built_only_on_hand_over_and_changes_nothing() {
    use std::cell::Cell;
    let r = range(RangeUnitKind::Spider);
    let rubble = r.current_rubble();
    let unchanged = nav::vehicle::Changed::default();
    let others: Vec<_> = (0..3)
        .map(|k| super::Obstacle {
            pos: [
                Fx::from_int(330 + 40 * k),
                Fx::from_int(16),
                Fx::from_int(300),
            ],
            head: 0,
            half: [Fx::from_int(12), Fx::from_int(8)],
        })
        .collect();
    let asked = Cell::new(0u32);
    let lazy = || {
        asked.set(asked.get() + 1);
        others.clone()
    };
    let save = |m: &super::Motion| {
        let mut b = fx::blob::Blob::new();
        m.save(&mut b);
        b.v
    };
    let body = &r.units[0].body;
    let mut m = r.units[0].motion.clone();
    // No share, and a share below any test: the search holds, no list.
    for budget in [0, 1] {
        m.continue_coarse(
            &r.world, body, lazy, &rubble, r.tick, &unchanged, budget, true, false,
        );
        assert_eq!(asked.get(), 0, "budget {budget}");
        assert!(m.coarse_waited().is_some());
    }
    // A test dearer than a whole tick: the order passes to the fine
    // planner, the list is asked for once, and the hull ends as with the
    // list built up front.
    let budget = r.navigation.global_work_per_tick;
    assert!(m.coarse_max_test() > budget);
    let mut eager = m.clone();
    let list = others.clone();
    eager.continue_coarse(
        &r.world,
        body,
        move || list,
        &rubble,
        r.tick,
        &unchanged,
        budget,
        true,
        true,
    );
    m.continue_coarse(
        &r.world, body, lazy, &rubble, r.tick, &unchanged, budget, true, true,
    );
    assert_eq!(asked.get(), 1);
    assert!(m.coarse_waited().is_none() && m.searching() || !m.path.is_empty());
    assert_eq!(save(&eager), save(&m));
}
