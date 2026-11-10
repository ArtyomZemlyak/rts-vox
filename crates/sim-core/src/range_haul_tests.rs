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

#[test]
fn compact_greben_dock_tracks_its_opening_and_rotation() {
    let mut u = super::RangeUnit::of_kind(RangeUnitKind::GrebenStorage, [800, 16, 800]);
    for heading in [0, fx::angle::QUARTER, fx::angle::HALF, 3 * fx::angle::QUARTER] {
        u.motion.drive.head = heading;
        u.update_pose(heading);
        let mouth = u.world_pos([0, 8, 20]);
        assert_eq!(super::dock_position(&u), [mouth[0], mouth[2]]);
        assert_eq!(u.body.mm, 200);
        let bays = super::greben_bays(&u);
        assert_eq!(u.body.segments.len(), 2);
        for (index, (bay, z)) in bays.into_iter().zip(voxel::unit::GREBEN_CRAWLER_SOCKET_Z).enumerate() {
            let mouth = u.world_pos([0, 8, z]);
            assert_eq!(bay, [
                mouth[0] - u.pose.x.x.mul(fx::Fx::from_int(108)).to_int_trunc(),
                mouth[2] - u.pose.x.z.mul(fx::Fx::from_int(108)).to_int_trunc(),
            ]);
            let portal = &u.body.segments[index].body;
            let frame = u.seg_frames[index];
            let entry = frame.world([fx::Fx::ZERO, fx::Fx::ZERO, fx::Fx::from_int(22)]);
            let relative = fx::Fx::from_int(mouth[0]).sub(entry.x).mul(u.pose.x.x)
                .add(fx::Fx::from_int(mouth[2]).sub(entry.z).mul(u.pose.x.z));
            assert!(relative.sub(fx::Fx::from_int(64)).abs() <= fx::Fx::ONE,
                "portal entrance did not rotate with the dock");
            assert!(!portal.vol.get(4, 20, 22), "loading lane must remain open");
            assert!(portal.vol.get(1, 34, 22), "portal must have an overhead frame");
        }
        assert_eq!(super::crawler_bay(&u, [bays[0][0], 16, bays[0][1]]), bays[0]);
        assert_eq!(super::crawler_bay(&u, [bays[1][0], 16, bays[1][1]]), bays[1]);
    }
}

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
    // Slice 3: no reclaim, so the snapshot is 63.
    e.reclaim = None;
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

#[test]
fn crawler_must_reach_greben_bay_before_unloading() {
    let mut l = layout(100_000, 50_000, 1_000_000);
    l.size[1] = 512;
    l.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: true, at: [200, 16, 400], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::GrebenStorage, allied: true, at: [500, 16, 400], weapon: None },
    ];
    l.economy = Some(RangeEconomy::stock());
    let mut r = range(l);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    let face = [200, 16, 400];
    assert!(r.battle_order(0, Order::Reclaim(face), None));
    let tank = r.mining().unwrap().tank(0).unwrap();
    let e = &mut r.matter.economy;
    e.ledger.reveal(e.ground, 10_000);
    assert_eq!(e.ledger.transfer(e.ground, tank, 10_000), 10_000);
    r.mining.as_mut().unwrap().mark_full(0);
    r.haul_phase();
    let (dock, bay) = r.crawler_unloading_bay(0).expect("assigned Greben bay");
    assert_eq!(dock, 1);
    assert_eq!(r.tank_held(0), Some(10_000));
    let before = r.side_store(true).unwrap().0;
    for _ in 0..1600 {
        r.step();
        if r.side_store(true).unwrap().0 > before { break; }
    }
    assert!(r.side_store(true).unwrap().0 > before, "crawler must actually reach and unload at the bay; position {:?}, bay {:?}", r.units[0].motion.pos.map(|v| v.to_int_trunc()), bay);
    let p = r.units[0].motion.pos.map(|v| v.to_int_trunc());
    let dx = i64::from(p[0] - bay[0]);
    let dz = i64::from(p[2] - bay[1]);
    assert!(dx * dx + dz * dz <= super::CRAWLER_DOCK_REACH * super::CRAWLER_DOCK_REACH);
    assert!(fx::angle::diff(r.units[dock].motion.drive.head, r.units[0].motion.drive.head).abs()
        <= super::CRAWLER_DOCK_ANGLE, "crawler unloaded without facing its socket");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn greben_unloads_large_load_over_multiple_ticks_without_losing_matter() {
    let mut l = layout(100_000, 50_000, 1_000_000);
    l.size[1] = 512;
    l.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: true, at: [200, 16, 400], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::GrebenStorage, allied: true, at: [500, 16, 400], weapon: None },
    ];
    l.economy = Some(RangeEconomy::stock());
    let mut r = range(l);
    assert!(r.battle_order(0, Order::Reclaim([200, 16, 400]), None));
    let tank = r.mining().unwrap().tank(0).unwrap();
    let cargo = r.matter.economy.ledger.get(tank).cap / 2;
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, cargo);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, cargo), cargo);
    r.mining.as_mut().unwrap().mark_full(0);
    r.haul_phase();
    let (dock, bay) = r.crawler_unloading_bay(0).unwrap();
    r.units[0].motion.pos = [bay[0], 16, bay[1]].map(fx::Fx::from_int);
    let heading = r.units[dock].motion.drive.head;
    r.units[0].motion.drive.head = heading;
    r.units[0].update_pose(heading);
    r.haul_phase();
    assert!(r.tank_held(0).unwrap() > 0 && r.tank_held(0).unwrap() < cargo);
    for _ in 0..50 { r.haul_phase(); }
    assert_eq!(r.tank_held(0), Some(0));
    assert!(!r.hauling(0), "crawler stayed in its hauling leg");
    assert_eq!(r.units[0].motion.destination, Some([200, 400]), "crawler did not resume the collection order");
    assert!(matches!(r.battle.actors[0].order, Order::Reclaim([200, 16, 400])));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_turns_at_the_socket_before_unloading_and_restores() {
    let mut l = layout(100_000, 50_000, 1_000_000);
    l.size[1] = 512;
    l.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: true, at: [200, 16, 400], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::GrebenStorage, allied: true, at: [500, 16, 400], weapon: None },
    ];
    l.economy = Some(RangeEconomy::stock());
    let mut r = range(l);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.units[1].motion.drive.head = fx::angle::QUARTER;
    r.units[1].update_pose(fx::angle::QUARTER);
    assert!(r.battle_order(0, Order::Reclaim([200, 16, 400]), None));
    let tank = r.mining().unwrap().tank(0).unwrap();
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, 10_000);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, 10_000), 10_000);
    r.mining.as_mut().unwrap().mark_full(0);
    r.haul_phase();
    let (dock, bay) = r.crawler_unloading_bay(0).unwrap();
    r.units[0].motion.pos = [bay[0], 16, bay[1]].map(fx::Fx::from_int);
    r.units[0].motion.drive.head = fx::angle::HALF;
    r.units[0].update_pose(fx::angle::HALF);
    let before = r.side_store(true).unwrap().0;
    r.haul_phase();
    assert_eq!(r.side_store(true).unwrap().0, before, "sideways crawler must not unload");
    assert!(r.crawler_docking_pilot(0).is_some());
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("dock approach restores");
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(r.crawler_docking_pilot(0), restored.crawler_docking_pilot(0));
    assert!(r.units[0].motion.can_drive(r.units[0].mobility), "dock chassis disabled: {:?}", r.units[0].mobility);
    {
        let mut probe = nav::vehicle::Probe::with_yield(&r.world, &[4, 5]);
        let motion = &r.units[0].motion;
        let at = [motion.pos[0], motion.pos[2]];
        let pose = probe.pose(at, motion.drive.head - 1, motion.probe_shape(&r.units[0].body));
        assert!(!pose.refused(), "dock terrain refuses a tiny turn: {pose:?}; gun {:?}", r.gun.motion.pos);
    }
    for turn_tick in 0..600 {
        r.step();
        if turn_tick == 0 {
            assert!(r.units[0].motion.drive.head != fx::angle::HALF,
                "dock turn did not start: pilot {:?}, chassis {:?}, status engine {}, enabled {}, patrols {}, phase {:?}",
                r.units[0].motion.pilot, r.units[0].mobility, r.units[0].status.engine, r.units[0].motion.enabled,
                r.patrols_running, r.battle.phase);
        }
        if r.side_store(true).unwrap().0 > before { break; }
    }
    assert!(r.side_store(true).unwrap().0 > before, "crawler failed to align: heading {}, target {}, pilot {:?}, pos {:?}, bay {:?}, enabled {}, blocked {}, penetration {:?}, own {:?}, dock {:?}",
        r.units[0].motion.drive.head, r.units[dock].motion.drive.head,
        r.crawler_docking_pilot(0), r.units[0].at, bay, r.units[0].motion.enabled, r.units[0].motion.blocked,
        super::super::super::motion::Obstacle::of_unit(&r.units[0]).penetration(&super::super::super::motion::Obstacle::of_unit(&r.units[dock])),
        (r.units[0].motion.pos, super::super::super::motion::Obstacle::of_unit(&r.units[0]).half),
        (r.units[dock].motion.pos, super::super::super::motion::Obstacle::of_unit(&r.units[dock]).half));
    assert!(fx::angle::diff(r.units[dock].motion.drive.head, r.units[0].motion.drive.head).abs()
        <= super::CRAWLER_DOCK_ANGLE);
    assert!(r.matter.economy.ledger.balanced());
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
fn destroyed_dock_cancels_the_old_route_without_losing_cargo() {
    let tank = 20 * ROCK;
    let mut r = range(layout(tank, 50_000, 1_000_000));
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(run(&mut r, 40, |r| r.hauling(0)));
    assert!(r.units[0].motion.destination.is_some());
    let held = r.tank_held(0);
    r.units[DOCK].status.alive = false;
    r.haul_phase();
    assert_eq!(
        r.units[0].motion.destination, None,
        "still driving to destroyed dock"
    );
    assert_eq!(r.tank_held(0), held);
    assert!(r.hauling(0));
    assert!(matches!(
        r.mining().unwrap().haul().unwrap().legs[0],
        Leg::Hauling(None)
    ));
    assert_books(&r);
}

#[test]
fn destroyed_dock_redirects_the_cargo_to_another_live_base() {
    let mut l = layout(20 * ROCK, 50_000, 1_000_000);
    l.units.push(RangeUnitDef {
        kind: RangeUnitKind::Base,
        allied: true,
        at: [520, 16, 450],
        weapon: None,
    });
    let mut r = range(l);
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    assert!(run(&mut r, 40, |r| r.hauling(0)));
    assert!(matches!(
        r.mining().unwrap().haul().unwrap().legs[0],
        Leg::Hauling(Some(3))
    ));
    let held = r.tank_held(0);
    r.units[DOCK].status.alive = false;
    r.haul_phase();
    assert!(matches!(
        r.mining().unwrap().haul().unwrap().legs[0],
        Leg::Hauling(Some(4))
    ));
    assert_eq!(r.units[0].motion.destination, Some([520, 450]));
    assert_eq!(r.tank_held(0), held);
    assert_books(&r);
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
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 63);
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

#[test]
fn manual_unload_accepts_partial_load_and_selected_greben_then_resumes_work() {
    use super::super::super::battle::commands::{Command, Outcome};
    let mut l = layout(100_000, 50_000, 1_000_000);
    l.size[1] = 512;
    l.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: true, at: [200, 16, 400], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::GrebenStorage, allied: true, at: [500, 16, 400], weapon: None },
    ];
    l.economy = Some(RangeEconomy::stock());
    let mut r = range(l);
    assert_eq!(r.execute_command(Command::UnloadCrawler { actor: 0, dock: None }), Outcome::Rejected);
    assert!(r.battle_order(0, Order::Reclaim([200, 16, 400]), None));
    let tank = r.mining().unwrap().tank(0).unwrap();
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, 10_000);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, 10_000), 10_000);
    assert_eq!(r.execute_command(Command::UnloadCrawler { actor: 0, dock: Some(usize::MAX) }), Outcome::Rejected);
    assert_eq!(r.execute_command(Command::UnloadCrawler { actor: 0, dock: Some(1) }), Outcome::Applied);
    let (dock, bay) = r.crawler_unloading_bay(0).unwrap();
    assert_eq!(dock, 1);
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("manual trip restores");
    assert_eq!(restored.crawler_unloading_bay(0), Some((dock, bay)));
    r.units[0].motion.pos = [bay[0], 16, bay[1]].map(fx::Fx::from_int);
    let heading = r.units[dock].motion.drive.head;
    r.units[0].motion.drive.head = heading;
    r.units[0].update_pose(heading);
    for _ in 0..50 { r.haul_phase(); }
    assert_eq!(r.tank_held(0), Some(0));
    assert!(!r.hauling(0));
    assert_eq!(r.battle.actors[0].order, Order::Reclaim([200, 16, 400]));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn one_off_unload_drives_to_dock_and_stays_there_without_starting_reclaim() {
    use super::super::super::battle::commands::{Command, Outcome};
    let mut l = layout(100_000, 50_000, 1_000_000);
    l.size[1] = 512;
    l.units = vec![
        RangeUnitDef { kind: RangeUnitKind::Crawler, allied: true, at: [200, 16, 400], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::GrebenStorage, allied: true, at: [500, 16, 400], weapon: None },
    ];
    l.economy = Some(RangeEconomy::stock());
    let mut r = range(l);
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    let tank = r.mining().unwrap().tank(0).unwrap();
    let ground = r.matter.economy.ground;
    r.matter.economy.ledger.reveal(ground, 10_000);
    assert_eq!(r.matter.economy.ledger.transfer(ground, tank, 10_000), 10_000);
    assert_eq!(r.execute_command(Command::UnloadCrawler { actor: 0, dock: Some(1) }), Outcome::Applied);
    assert_eq!(r.battle.actors[0].order, Order::Hold);
    let (_, bay) = r.crawler_unloading_bay(0).expect("one-off trip has a stable bay");
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("one-off trip restores");
    assert_eq!(restored.crawler_unloading_bay(0), Some((1, bay)));
    for _ in 0..1600 {
        r.step();
        if r.tank_held(0) == Some(0) { break; }
    }
    assert_eq!(r.tank_held(0), Some(0), "crawler failed to drive and unload at {bay:?}; pos {:?}", r.units[0].motion.pos);
    assert_eq!(r.battle.actors[0].order, Order::Hold);
    assert!(!r.hauling(0));
    assert!(r.units[0].motion.destination.is_none());
    assert!(r.matter.economy.ledger.balanced());
}
