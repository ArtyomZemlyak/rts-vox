//! Dig into the tank on the range (plan 245, slice 1; ADR-672).
use super::super::battle::{commands, Order};
use super::super::BlastRange;
use modding::explosion::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef, RangeUnitKind};
use modding::range_economy::RangeEconomy;

/// Rock (`range-materials.ron`): one cell carries this much matter.
const ROCK: u64 = 260;
/// A buried slab of rock beside the three tanks of `fixture-drive.ron`.
const FACE: [i32; 3] = [280, 8, 330];
const FAR_FACE: [i32; 3] = [280, 8, 440];

fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
}

fn layout(tank: u64, rate: u32, pool: u32) -> RangeLayout {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.blocks.push(RangeBlock {
        min: [266, 1, 316],
        max: [296, 15, 456],
        material: 6,
    });
    let mut e = RangeEconomy::stock();
    // Slice 1: no stores and no reclaim, so a full tank stops and the
    // snapshot is 62.
    e.bank = None;
    e.reclaim = None;
    e.kinds.iter_mut().for_each(|k| k.store = 0);
    e.dig_edits_in_flight = pool;
    let k = e.kind_mut(RangeUnitKind::Tank).unwrap();
    k.tank = tank;
    k.dig_radius = 3;
    k.dig_reach = 120;
    k.dig_edits_per_tick = rate;
    l.economy = Some(e);
    l
}

fn range(l: RangeLayout) -> BlastRange {
    let mut r = BlastRange::with_layout(config(), l).unwrap();
    r.start_free_control().unwrap();
    r
}

fn ball_solid(r: &BlastRange, face: [i32; 3], radius: i32) -> usize {
    super::ball(radius)
        .filter(|d| {
            let p = [face[0] + d[0], face[1] + d[1], face[2] + d[2]].map(|v| v as u32);
            r.world.solid(p[0], p[1], p[2])
        })
        .count()
}

fn assert_books(r: &BlastRange) {
    let m = r.mining().unwrap();
    assert!(r.matter.economy.ledger.balanced());
    let tanks: u64 = (0..r.units.len()).filter_map(|id| r.tank_held(id)).sum();
    assert_eq!(tanks, m.mined);
    assert_eq!(m.mined, m.cells * ROCK);
    assert!(m.in_flight() <= m.data().dig_edits_in_flight as usize);
}

#[test]
fn digging_fills_the_tank_and_removes_rock_with_balanced_books_every_tick() {
    let mut r = range(layout(1_000_000, 16, 64));
    let ball = ball_solid(&r, FACE, 3);
    assert!(ball > 100, "the face is in rock: {ball}");
    assert_eq!(r.tank_held(0), Some(0));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    let removed = r.removed;
    let mut last = 0;
    for _ in 0..40 {
        r.step();
        assert_books(&r);
        let held = r.tank_held(0).unwrap();
        assert!(held >= last);
        last = held;
    }
    let m = r.mining().unwrap();
    assert_eq!(ball_solid(&r, FACE, 3), 0, "the whole ball is dug");
    assert_eq!(m.cells as usize, ball);
    assert_eq!(r.tank_held(0), Some(ball as u64 * ROCK));
    assert_eq!(r.removed - removed, ball, "the rock left the world");
    // Only the ordered digger dug.
    assert_eq!(r.tank_held(1), Some(0));
    assert_eq!(r.tank_held(2), Some(0));
    assert_eq!(m.deferred, 0);
    assert_eq!(r.battle.actors[0].order, Order::Dig(FACE));
}

#[test]
fn a_full_tank_stops_the_digging() {
    // Room for ten cells of rock and part of an eleventh.
    let mut r = range(layout(10 * ROCK + ROCK / 2, 4, 64));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    for _ in 0..10 {
        r.step();
        assert_books(&r);
    }
    let m = r.mining().unwrap();
    assert_eq!(m.cells, 10);
    assert_eq!(r.tank_held(0), Some(10 * ROCK));
    assert_eq!(m.in_flight(), 0);
    let left = ball_solid(&r, FACE, 3);
    assert!(left > 0, "the face is not dug out");
    let removed = r.removed;
    for _ in 0..10 {
        r.step();
    }
    assert_eq!(ball_solid(&r, FACE, 3), left, "a full tank digs nothing");
    assert_eq!(r.removed, removed);
    assert_eq!(r.tank_held(0), Some(10 * ROCK));
}

#[test]
fn dug_cells_are_visible_to_navigation_the_same_tick() {
    let mut l = layout(1_000_000, 64, 64);
    // A face at the surface, so the column's top is what goes.
    let face = [280, 15, 330];
    l.blocks.push(RangeBlock {
        min: [270, 1, 320],
        max: [290, 16, 340],
        material: 6,
    });
    let mut r = range(l);
    let (x, z) = (face[0] as u32, face[2] as u32);
    assert_eq!(r.world.column_ceiling(x, z), 16);
    assert!(r.battle_order(0, Order::Dig(face), None));
    r.step();
    // Queued, drained and credited within the one tick.
    assert!(r.tank_held(0).unwrap() > 0);
    assert_eq!(r.mining().unwrap().in_flight(), 0);
    assert!(!r.world.solid(x, 15, z));
    assert!(
        r.world.column_ceiling(x, z) < 16,
        "the hole is in the occupancy"
    );
    assert!(
        !r.world_changed.is_empty(),
        "the hole is in the route pages"
    );
}

#[test]
fn a_restore_mid_dig_continues_bit_for_bit() {
    let l = layout(1_000_000, 8, 64);
    // Other edits ahead in the queue keep digs in flight across the save.
    let prepare = |r: &mut BlastRange| r.world.max_edits_per_tick = 6;
    let mut a = range(l);
    prepare(&mut a);
    assert!(a.battle_order(0, Order::Dig(FACE), None));
    assert!(a.battle_order(2, Order::Dig(FAR_FACE), None));
    for x in 400..440 {
        a.world.queue_dig(x, 15, 600);
    }
    for _ in 0..3 {
        a.step();
    }
    assert!(
        a.mining().unwrap().in_flight() > 0,
        "digs wait in the queue"
    );
    let bytes = a.snapshot();
    assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 62);
    let mut b = BlastRange::restore(config(), &bytes).expect("restore mid-dig");
    prepare(&mut b);
    assert_eq!(b.snapshot(), bytes);
    // A reservation without its queued edit is refused.
    let mut forged = BlastRange::restore(config(), &bytes).unwrap();
    let m = forged.mining.as_mut().unwrap();
    let (_, r) = m.reserved.pop_first().unwrap();
    m.inflight[usize::from(r.slot)] -= r.amount;
    assert!(BlastRange::restore(config(), &forged.snapshot()).is_none());
    for tick in 0..40 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash(), "tick {tick}");
        assert_books(&b);
    }
    assert!(b.tank_held(0).unwrap() > 0 && b.tank_held(2).unwrap() > 0);
}

#[test]
fn a_spent_dig_sub_pool_defers_the_rest_by_rotation_without_stopping() {
    let mut r = range(layout(1_000_000, 4, 4));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(r.battle_order(2, Order::Dig(FAR_FACE), None));
    let mut served = [0u64; 2];
    for tick in 0..10u64 {
        let before = [r.tank_held(0).unwrap(), r.tank_held(2).unwrap()];
        r.step();
        assert_books(&r);
        let after = [r.tank_held(0).unwrap(), r.tank_held(2).unwrap()];
        // One digger takes the whole sub-pool, the other waits a tick.
        let got: Vec<_> = (0..2).map(|i| (after[i] - before[i]) / ROCK).collect();
        assert_eq!(got.iter().sum::<u64>(), 4, "tick {tick}: {got:?}");
        assert!(got.contains(&0), "tick {tick}: {got:?}");
        for i in 0..2 {
            served[i] += got[i];
        }
        assert_eq!(r.mining().unwrap().deferred, tick + 1);
    }
    assert_eq!(served, [20, 20], "the rotation serves both in turn");
}

#[test]
fn the_rules_ai_does_not_override_a_dig_order() {
    let mut l = layout(1_000_000, 4, 64);
    l.units.push(RangeUnitDef {
        kind: RangeUnitKind::Tank,
        allied: false,
        ..l.units[0].clone()
    });
    l.units.last_mut().unwrap().at = [600, 16, 600];
    let mut r = BlastRange::with_layout(config(), l).unwrap();
    assert!(r.set_ai_vs_ai(true));
    r.start_battle().unwrap();
    r.start_command_capture(4096).unwrap();
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(r.battle_order(1, Order::Hold, None));
    for _ in 0..60 {
        r.step();
    }
    assert_eq!(r.battle.actors[0].order, Order::Dig(FACE));
    assert_ne!(
        r.battle.actors[1].order,
        Order::Hold,
        "the allied rules AI does replace other orders"
    );
    assert!(r.tank_held(0).unwrap() > 0);
    let trace = r.finish_command_capture().unwrap();
    let rules: Vec<_> = trace
        .records
        .iter()
        .filter(|x| {
            x.producer == commands::Producer::AlliedRules
                && matches!(x.command, commands::Command::Order { actor: 0, .. })
        })
        .collect();
    assert!(!rules.is_empty());
    assert!(rules
        .iter()
        .all(|x| x.outcome == commands::Outcome::Unchanged));
}

#[test]
fn a_dig_order_needs_a_tank_a_face_in_the_map_and_no_destination() {
    let mut stock = layout(0, 0, 64);
    let k = stock
        .economy
        .as_mut()
        .unwrap()
        .kind_mut(RangeUnitKind::Tank)
        .unwrap();
    (k.dig_radius, k.dig_reach) = (0, 0);
    let mut r = range(stock);
    assert!(!r.battle_order(0, Order::Dig(FACE), None), "no kind digs");
    let mut r2 = range(layout(1_000, 4, 64));
    assert!(!r2.battle_order(0, Order::Dig([280, 8, 768]), None));
    assert!(!r2.battle_order(0, Order::Dig(FACE), Some([280, 330])));
    assert!(r2.battle_order(0, Order::Dig(FACE), None));
    // A layout without economy has no tanks and no dig.
    let plain = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    let mut p = range(plain);
    assert!(p.mining().is_none() && p.tank_held(0).is_none());
    assert!(!p.battle_order(0, Order::Dig(FACE), None));
    assert_ne!(
        u32::from_le_bytes(p.snapshot()[..4].try_into().unwrap()),
        62
    );
    r.step();
}

#[test]
fn a_digger_out_of_reach_drives_to_its_face_and_stops_there() {
    let mut l = layout(1_000_000, 8, 64);
    l.economy
        .as_mut()
        .unwrap()
        .kind_mut(RangeUnitKind::Tank)
        .unwrap()
        .dig_reach = 40;
    let mut r = range(l);
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(
        r.units[0].motion.destination.is_some(),
        "it drives to its face"
    );
    for _ in 0..300 {
        r.step();
        if r.tank_held(0).unwrap() > 0 {
            break;
        }
    }
    assert!(r.tank_held(0).unwrap() > 0, "it reached its face and dug");
    assert_eq!(
        r.units[0].motion.destination, None,
        "it stopped at the face"
    );
}
