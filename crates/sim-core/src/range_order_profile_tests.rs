use super::*;

fn scenario(workers: usize) -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = vec![
        modding::range::RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [170, 16, 336],
            allied: true,
        },
        modding::range::RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [450, 16, 336],
            allied: false,
        },
    ];
    let mut range = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap(),
        layout,
    )
    .unwrap();
    range.start_battle().unwrap();
    assert!(range.set_navigation_workers(workers));
    range.battle.ai.next_tick = u32::MAX;
    range.battle.actors[1].order = Order::Hold;
    std::sync::Arc::make_mut(&mut range.battle.tuning).tank_range = 100;
    assert!(range.set_battle_weapon(0, Some(Weapon::Blast)));
    range
}

#[test]
fn pursuit_observations_and_worker_counts_preserve_ordered_state() {
    let mut serial = scenario(1);
    let mut parallel = scenario(8);
    let mut phases = Vec::new();
    for tick in 0..24 {
        serial.step();
        parallel.step_observed(|phase| phases.push(phase));
        assert_eq!(parallel.state_hash(), serial.state_hash(), "tick={tick}");
    }
    for required in [
        "orders_approach",
        "orders_firing_point",
        "orders_flank",
        "orders_retarget_setup",
        "orders_retarget_target",
        "orders_retarget_fine",
        "orders_retarget",
    ] {
        assert!(phases.contains(&required), "missing {required}");
    }
    assert!(parallel.units[0].motion.destination.is_some());
}
