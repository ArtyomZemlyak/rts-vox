//! Hauling to the docks and the sides' stores (plan 245, slice 3; ADR-673).
use super::super::super::battle::Order;
use super::super::super::BlastRange;
use super::Leg;
use crate::matter::Sink;
use modding::explosion::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef, RangeUnitKind};
use modding::range_economy::RangeEconomy;

/// Rock (`range-materials.ron`): one cell carries this much matter.
const ROCK: u64 = 260;
/// A buried slab of rock beside the three tanks of `fixture-drive.ron`.
const FACE: [i32; 3] = [280, 8, 330];
const FAR_FACE: [i32; 3] = [280, 8, 440];
/// Slot of the allied base, the dock, after the fixture's three tanks.
const DOCK: usize = 3;

fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
}

/// The fixture's three allied tanks dig; an allied base stands far enough
/// from the face that the hauler drives there and back.
fn layout(tank: u64, base_store: u64, store: u64) -> RangeLayout {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.blocks.push(RangeBlock {
        min: [266, 1, 316],
        max: [296, 15, 456],
        material: 6,
    });
    l.units.push(RangeUnitDef {
        kind: RangeUnitKind::Base,
        allied: true,
        at: [520, 16, 260],
        weapon: None,
    });
    let mut e = RangeEconomy::stock();
    let k = e.kind_mut(RangeUnitKind::Tank).unwrap();
    k.tank = tank;
    k.dig_radius = 3;
    k.dig_reach = 120;
    k.dig_edits_per_tick = 16;
    e.kind_mut(RangeUnitKind::Base).unwrap().store = store;
    let b = e.bank.as_mut().unwrap();
    b.base_store = base_store;
    b.refine_num = 7;
    b.refine_den = 10;
    b.unload_reach = 100;
    b.dock_searches_per_tick = 8;
    l.economy = Some(e);
    l
}

fn range(l: RangeLayout) -> BlastRange {
    let mut r = BlastRange::with_layout(config(), l).unwrap();
    r.start_free_control().unwrap();
    r
}

fn stores(r: &BlastRange) -> u64 {
    r.side_store(true).unwrap().0 + r.side_store(false).unwrap().0
}

/// Matter is conserved and every unit of it is where the books say: what
/// was dug is in the tanks, in the stores or in the two sinks hauling uses.
fn assert_books(r: &BlastRange) {
    let l = &r.matter.economy.ledger;
    assert!(l.balanced(), "imbalance {}", l.imbalance());
    let m = r.mining().unwrap();
    let h = m.haul().unwrap();
    let tanks: u64 = (0..r.units.len()).filter_map(|id| r.tank_held(id)).sum();
    assert_eq!(
        tanks + stores(r) + l.sunk(Sink::RefiningLoss) + l.sunk(Sink::OverCap),
        m.mined
    );
    assert_eq!(h.unloaded, h.delivered + l.sunk(Sink::RefiningLoss));
    for allied in [true, false] {
        let (held, cap) = r.side_store(allied).unwrap();
        assert!(held <= cap);
    }
}

/// Step until `done`, checking the books every tick.
fn run(r: &mut BlastRange, ticks: usize, done: impl Fn(&BlastRange) -> bool) -> bool {
    for _ in 0..ticks {
        r.step();
        assert_books(r);
        if done(r) {
            return true;
        }
    }
    false
}

#[test]
fn a_full_tank_is_hauled_to_the_dock_and_refined_into_the_store() {
    let tank = 20 * ROCK;
    let mut r = range(layout(tank, 50_000, 1_000_000));
    assert_eq!(r.side_store(true), Some((0, 1_050_000)));
    assert_eq!(r.side_store(false), Some((0, 50_000)));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(run(&mut r, 40, |r| r.hauling(0)), "the full tank hauls");
    assert_eq!(r.tank_held(0), Some(tank));
    assert!(r.units[0].motion.destination.is_some(), "it drives");
    let removed = r.removed;
    assert!(
        run(&mut r, 1500, |r| r.side_store(true).unwrap().0 > 0),
        "the hauler reached its dock"
    );
    // It drove into the unload reach of the base, 240 cells away.
    let at = r.units[0].motion.pos.map(|v| v.to_int_trunc());
    let (dx, dz) = (i64::from(at[0] - 520), i64::from(at[2] - 260));
    assert!(dx * dx + dz * dz <= 100 * 100, "{at:?}");
    assert!(at[0] >= 400, "{at:?}");
    // Refined at 7/10: the rest is booked to RefiningLoss.
    let l = &r.matter.economy.ledger;
    assert_eq!(r.side_store(true).unwrap().0, tank * 7 / 10);
    assert_eq!(l.sunk(Sink::RefiningLoss), tank - tank * 7 / 10);
    assert_eq!(r.tank_held(0), Some(0));
    assert!(!r.hauling(0));
    assert_eq!(r.removed, removed, "nothing was dug on the way");
    let h = r.mining().unwrap().haul().unwrap();
    assert_eq!((h.searches, h.search_misses, h.search_deferred), (1, 0, 0));
    // Back at the face of its order, it digs again.
    assert!(
        run(&mut r, 1500, |r| r.tank_held(0).unwrap() > 0),
        "it returned to its face"
    );
    assert_eq!(r.battle.actors[0].order, Order::Dig(FACE));
    assert!(r.removed > removed);
    let at = r.units[0].motion.pos.map(|v| v.to_int_trunc());
    let (dx, dz) = (i64::from(at[0] - FACE[0]), i64::from(at[2] - FACE[2]));
    assert!(dx * dx + dz * dz <= 120 * 120, "back in dig reach: {at:?}");
}

#[test]
fn a_full_store_leaves_the_cargo_in_the_tank() {
    let tank = 20 * ROCK;
    // Room for 1000: of the cargo only what refines into it is taken.
    let mut r = range(layout(tank, 400, 600));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(run(&mut r, 1500, |r| r.side_store(true).unwrap().0 > 0));
    let taken = 1000 * 10 / 7;
    assert_eq!(r.side_store(true).unwrap().0, taken * 7 / 10);
    assert_eq!(r.tank_held(0), Some(tank - taken));
    assert!(r.hauling(0), "the cargo waits at the dock");
    let loss = r.matter.economy.ledger.sunk(Sink::RefiningLoss);
    assert_eq!(loss, taken - taken * 7 / 10);
    // No trickle into the sink: the unit of room left refines into nothing,
    // so nothing more is taken.
    run(&mut r, 30, |_| false);
    assert_eq!(r.tank_held(0), Some(tank - taken));
    assert_eq!(r.matter.economy.ledger.sunk(Sink::RefiningLoss), loss);
    assert!(r.hauling(0));
}

#[test]
fn a_dock_raises_the_cap_and_its_death_lowers_it_with_over_cap() {
    let tank = 20 * ROCK;
    let mut l = layout(tank, 1000, 1_000_000);
    // An enemy dock raises only its own side's cap.
    l.units.push(RangeUnitDef {
        kind: RangeUnitKind::Base,
        allied: false,
        at: [600, 16, 620],
        weapon: None,
    });
    let plain = BlastRange::with_layout(config(), {
        let mut p = l.clone();
        p.economy = None;
        p
    })
    .unwrap();
    let mut r = range(l);
    // Container 1 is the allied store; the enemy's follows every other
    // container, so bodies, fuel and tanks keep their numbers.
    let h = r.mining().unwrap().haul().unwrap();
    let before = plain.matter.economy.ledger.snapshot().0.len();
    assert_eq!(h.store(true), 1);
    let tanks = (0..r.units.len()).filter(|&id| r.tank_held(id).is_some());
    assert_eq!(h.store(false) as usize, before + tanks.count());
    assert_eq!(r.side_store(true), Some((0, 1_001_000)));
    assert_eq!(r.side_store(false), Some((0, 1_001_000)));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(run(&mut r, 1500, |r| r.side_store(true).unwrap().0 > 0));
    let stock = r.side_store(true).unwrap().0;
    assert_eq!(stock, tank * 7 / 10);
    assert!(stock > 1000, "the stock is over the base store");
    // The dock dies: the cap falls back to the base store and the excess
    // goes to OverCap — the store is the side's, so nothing is breached.
    r.units[DOCK].status.alive = false;
    r.step();
    assert_books(&r);
    let l = &r.matter.economy.ledger;
    assert_eq!(r.side_store(true), Some((1000, 1000)));
    assert_eq!(l.sunk(Sink::OverCap), stock - 1000);
    assert_eq!(l.sunk(Sink::ContainerBreach), 0);
    assert_eq!(r.side_store(false), Some((0, 1_001_000)));
}

#[test]
fn a_failed_dock_search_defers_and_the_next_digger_still_searches() {
    let mut l = layout(20 * ROCK, 50_000, 1_000_000);
    // An enemy digger with no dock of its side searches in vain.
    l.units.push(RangeUnitDef {
        kind: RangeUnitKind::Tank,
        allied: false,
        at: [280, 16, 508],
        weapon: None,
    });
    l.units[2].at = [600, 16, 600];
    let enemy = l.units.len() - 1;
    l.economy
        .as_mut()
        .unwrap()
        .bank
        .as_mut()
        .unwrap()
        .dock_searches_per_tick = 1;
    let mut r = range(l);
    // The player commands one side; the enemy's order is set directly.
    r.battle.actors[enemy].order = Order::Dig(FAR_FACE);
    assert!(run(&mut r, 40, |r| r.hauling(enemy)));
    assert!(run(&mut r, 20, |r| r
        .mining()
        .unwrap()
        .haul()
        .unwrap()
        .search_misses
        >= 3));
    let cargo = r.tank_held(enemy).unwrap();
    assert!(cargo > 0);
    // The allied digger fills while the enemy searches every tick it may.
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(run(&mut r, 40, |r| r.hauling(0)));
    // With one search a tick the two alternate: the allied one finds its
    // dock within two ticks although the enemy's search fails each time.
    let deferred = r.mining().unwrap().haul().unwrap().search_deferred;
    assert!(run(&mut r, 2, |r| matches!(
        r.mining().unwrap().haul().unwrap().legs[0],
        Leg::Hauling(Some(_))
    )));
    let h = r.mining().unwrap().haul().unwrap();
    assert_eq!(h.legs[0], Leg::Hauling(Some(DOCK as u16)));
    assert!(h.search_deferred > deferred, "a search waited a tick");
    assert!(run(&mut r, 1500, |r| r.side_store(true).unwrap().0 > 0));
    let h = r.mining().unwrap().haul().unwrap();
    assert_eq!(h.legs[enemy], Leg::Hauling(None));
    assert_eq!(r.tank_held(enemy), Some(cargo), "the cargo waits");
    assert_eq!(r.side_store(false).unwrap().0, 0);
}

#[test]
fn a_restore_mid_haul_continues_bit_for_bit() {
    let mut a = range(layout(20 * ROCK, 50_000, 1_000_000));
    assert!(a.battle_order(0, Order::Dig(FACE), None));
    assert!(a.battle_order(2, Order::Dig(FAR_FACE), None));
    assert!(run(&mut a, 40, |r| r.hauling(0)));
    for _ in 0..10 {
        a.step();
    }
    assert!(a.hauling(0) && a.units[0].motion.destination.is_some());
    let bytes = a.snapshot();
    assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 63);
    let mut b = BlastRange::restore(config(), &bytes).expect("restore mid-haul");
    assert_eq!(b.snapshot(), bytes);
    // A haul to a unit that is no dock is refused.
    let mut forged = BlastRange::restore(config(), &bytes).unwrap();
    forged.mining.as_mut().unwrap().haul.as_mut().unwrap().legs[0] = Leg::Hauling(Some(1));
    assert!(BlastRange::restore(config(), &forged.snapshot()).is_none());
    for tick in 0..1500 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash(), "tick {tick}");
        assert_books(&b);
        if b.side_store(true).unwrap().0 > 0 && !b.hauling(0) {
            break;
        }
    }
    assert!(
        b.side_store(true).unwrap().0 > 0,
        "the restored hauler unloaded"
    );
}

#[test]
fn the_store_and_the_haul_are_in_the_state_hash() {
    let mut r = range(layout(20 * ROCK, 50_000, 1_000_000));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    // The scenario requires the haul's result, not that the phase ran.
    assert!(run(&mut r, 1500, |r| r.side_store(true).unwrap().0 > 0));
    let hash = r.state_hash();
    let bytes = r.snapshot();
    // One unit moved from the store back into the tank: same total.
    let mut moved = BlastRange::restore(config(), &bytes).unwrap();
    assert_eq!(moved.state_hash(), hash);
    let e = &mut moved.matter.economy;
    let store = moved.mining.as_ref().unwrap().haul().unwrap().store(true);
    let tank = moved.mining.as_ref().unwrap().tank(0).unwrap();
    assert_eq!(e.ledger.transfer(store, tank, 1), 1);
    assert!(e.ledger.balanced());
    assert_ne!(moved.state_hash(), hash, "the stock is in the hash");
    // A leg alone changes the hash too.
    let mut leg = BlastRange::restore(config(), &bytes).unwrap();
    leg.mining.as_mut().unwrap().haul.as_mut().unwrap().legs[0] = Leg::Hauling(None);
    assert_ne!(leg.state_hash(), hash, "the leg is in the hash");
}

#[test]
fn a_new_order_ends_the_haul() {
    let mut r = range(layout(20 * ROCK, 50_000, 1_000_000));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(run(&mut r, 40, |r| r.hauling(0)));
    assert!(r.battle_order(0, Order::Hold, None));
    assert!(!r.hauling(0));
    assert_eq!(r.units[0].motion.destination, None);
    run(&mut r, 5, |_| false);
    assert!(!r.hauling(0), "a unit not ordered to dig does not haul");
    assert_eq!(r.tank_held(0), Some(20 * ROCK), "the cargo stays");
}

#[test]
fn the_read_model_shows_stores_and_tanks() {
    let mut r = range(layout(20 * ROCK, 50_000, 1_000_000));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(run(&mut r, 40, |r| r.hauling(0)));
    let v = super::super::super::read_model::View::new(&r, 1);
    assert_eq!(v.stores, Some([[0, 1_050_000], [0, 50_000]]));
    assert_eq!(v.units[0].tank, Some([20 * ROCK, 20 * ROCK]));
    assert!(v.units[0].hauling && !v.units[1].hauling);
    assert_eq!(v.units[DOCK].tank, None);
    // A layout without economy shows neither.
    let plain = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    let p = BlastRange::with_layout(config(), plain).unwrap();
    let v = super::super::super::read_model::View::new(&p, 1);
    assert_eq!(v.stores, None);
    assert!(v.units.iter().all(|u| u.tank.is_none() && !u.hauling));
}
