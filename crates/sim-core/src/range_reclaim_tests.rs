//! Reclaim at the face and the wreck of a dead digger's tank (plan 245,
//! slice 6; ADR-674).
use super::super::super::battle::Order;
use super::super::super::BlastRange;
use super::Work;
use crate::burst::{Fragment, IDENTITY};
use crate::matter::{Kind, Sink};
use fx::Fx;
use modding::explosion::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout, RangeUnitKind};
use modding::range_economy::RangeEconomy;
use phys::V3;

/// Rock and concrete (`range-materials.ron`): matter of one cell.
const ROCK: u64 = 260;
const CONCRETE: u64 = 240;
/// Between the fixture's first two tanks, clear of both hulls.
const SPOT: [i32; 3] = [280, 30, 322];
/// A buried slab of rock beside the tanks, as in the dig tests.
const FACE: [i32; 3] = [280, 8, 330];

fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
}

/// The fixture's three allied tanks dig, without stores, and reclaim
/// within `reach` of the face.
fn layout(tank: u64, rate: u32, reach: u32) -> RangeLayout {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    let mut e = RangeEconomy::stock();
    e.bank = None;
    e.kinds.iter_mut().for_each(|k| k.store = 0);
    e.dig_edits_in_flight = 64;
    let k = e.kind_mut(RangeUnitKind::Tank).unwrap();
    k.tank = tank;
    k.dig_radius = 1;
    k.dig_reach = 130;
    k.dig_edits_per_tick = rate;
    let r = e.reclaim.as_mut().unwrap();
    r.reach = reach;
    r.searches_per_tick = 8;
    r.breach_num = 1;
    r.breach_den = 4;
    r.cargo_per_edit = ROCK;
    l.economy = Some(e);
    l
}

fn range(l: RangeLayout) -> BlastRange {
    let mut r = BlastRange::with_layout(config(), l).unwrap();
    r.start_free_control().unwrap();
    r
}

/// Drop a whole box of `side` cells of `material` at `at`; returns its id
/// and container. The matter is revealed from the ground, as a blast's.
fn drop_piece(r: &mut BlastRange, material: u16, side: u8, at: [i32; 3]) -> (u64, u32) {
    let count = u32::from(side).pow(3);
    let e = &mut r.matter.economy;
    let amount = e.per_voxel(material) * u64::from(count);
    let held = e.ledger.create(Kind::Debris, 0, u64::MAX);
    let ground = e.ground;
    e.ledger.reveal(ground, amount);
    assert_eq!(e.ledger.transfer(ground, held, amount), amount);
    let d = &mut r.matter.debris;
    d.next_id += 1;
    let id = d.next_id;
    let half = Fx::from_ratio(1, 2);
    d.fragments.push(Fragment {
        id,
        position: V3::new(
            Fx::from_int(at[0]).add(half),
            Fx::from_int(at[1]),
            Fx::from_int(at[2]).add(half),
        ),
        velocity: V3::ZERO,
        material,
        paint: None,
        mm: voxel::unit::WORLD_MM,
        basis: IDENTITY,
        mass_g: 2400 * u64::from(count),
        count,
        shape: if count == 64 {
            u64::MAX
        } else {
            (1u64 << count) - 1
        },
        side,
        chunk: None,
        held,
        resting: false,
    });
    (id, held)
}

fn piece(r: &BlastRange, id: u64) -> Option<&Fragment> {
    r.matter.debris.fragments.iter().find(|f| f.id == id)
}

/// Where a piece lies: its centre, truncated.
fn at(f: &Fragment) -> [i32; 3] {
    [f.position.x, f.position.y, f.position.z].map(|v| v.to_int_trunc())
}

fn settle(r: &mut BlastRange, id: u64) -> [i32; 3] {
    for _ in 0..300 {
        r.step();
        if piece(r, id).is_some_and(|f| f.resting) {
            return at(piece(r, id).unwrap());
        }
    }
    panic!("the piece never rested");
}

fn tanks(r: &BlastRange) -> u64 {
    (0..r.units.len()).filter_map(|id| r.tank_held(id)).sum()
}

/// Matter is conserved and the tanks hold what was dug and reclaimed from
/// pieces, less what dead tanks lost.
fn assert_books(r: &BlastRange, from_pieces: u64) {
    let l = &r.matter.economy.ledger;
    assert!(l.balanced(), "imbalance {}", l.imbalance());
    let m = r.mining().unwrap();
    let rc = m.reclaim().unwrap();
    assert_eq!(tanks(r) + rc.lost, m.mined + from_pieces);
    assert_eq!(rc.lost, l.sunk(Sink::ContainerBreach));
    assert!(rc.search_misses <= rc.searches);
}

fn reclaim(r: &BlastRange) -> &super::Reclaim {
    r.mining().unwrap().reclaim().unwrap()
}

#[test]
fn a_resting_piece_at_the_face_is_reclaimed_into_the_tank_and_leaves_the_same_tick() {
    let mut r = range(layout(1_000_000, 16, 4));
    let (id, held) = drop_piece(&mut r, 6, 2, SPOT);
    let face = settle(&mut r, id);
    assert_eq!(r.matter.economy.ledger.held(held), 8 * ROCK);
    assert!(r.matter.economy.wrecks.iter().any(|w| w.held == held));
    assert!(r.current_rubble().get(&[face[0], face[2]]).is_some());
    let removed = r.removed;
    assert!(r.battle_order(0, Order::Dig(face), None));
    r.step();
    assert_books(&r, 8 * ROCK);
    // Eight cells at sixteen edits a tick: the piece goes in one tick.
    assert_eq!(r.tank_held(0), Some(8 * ROCK));
    assert!(piece(&r, id).is_none(), "the piece is gone");
    let rc = reclaim(&r);
    assert_eq!((rc.pieces, rc.edits, rc.reclaimed), (1, 8, 8 * ROCK));
    assert_eq!((rc.searches, rc.search_misses), (1, 0));
    // The same tick: no wreck entry, the container retired, no rubble.
    assert!(!r.matter.economy.wrecks.iter().any(|w| w.held == held));
    let c = r.matter.economy.ledger.get(held);
    assert!(!c.alive && c.held == 0);
    let rubble = r.current_rubble();
    assert!(
        rubble.get(&[face[0], face[2]]).is_none(),
        "the heap is gone"
    );
    assert_eq!(r.removed, removed, "the world was not dug");
    // Nothing more at the face: no search either.
    for _ in 0..5 {
        r.step();
        assert_books(&r, 8 * ROCK);
    }
    assert_eq!(reclaim(&r).searches, 1);
    assert_eq!(r.tank_held(0), Some(8 * ROCK));
}

#[test]
fn a_piece_away_from_the_face_or_the_digger_is_not_reclaimed() {
    let mut l = layout(1_000_000, 16, 4);
    l.economy
        .as_mut()
        .unwrap()
        .kind_mut(RangeUnitKind::Tank)
        .unwrap()
        .dig_reach = 40;
    let mut r = range(l);
    let (near, _) = drop_piece(&mut r, 6, 2, SPOT);
    let (far, _) = drop_piece(&mut r, 2, 2, [SPOT[0], SPOT[1], SPOT[2] + 12]);
    let face = settle(&mut r, near);
    settle(&mut r, far);
    // The face is out of the digger's reach: it drives, and takes nothing
    // from across the map on the way.
    assert!(r.battle_order(0, Order::Dig(face), None));
    for _ in 0..3 {
        r.step();
        assert_books(&r, 0);
    }
    assert_eq!(reclaim(&r).reclaimed, 0);
    assert_eq!(reclaim(&r).searches, 0);
    let pos = r.units[0].motion.pos.map(|v| v.to_int_trunc());
    assert!((pos[2] - face[2]).abs() > 40, "it arrived already: {pos:?}");
    for _ in 0..600 {
        r.step();
        assert_books(&r, reclaim(&r).reclaimed);
        if piece(&r, near).is_none() {
            break;
        }
    }
    assert!(piece(&r, near).is_none(), "the piece at the face was taken");
    // The other lies twelve cells off a face of reach four.
    for _ in 0..20 {
        r.step();
    }
    assert!(piece(&r, far).is_some_and(|f| f.resting));
    assert_eq!(r.tank_held(0), Some(8 * ROCK));
}

#[test]
fn a_live_hull_is_never_taken_and_spends_no_search() {
    let mut r = range(layout(1_000_000, 16, 8));
    let body = r.matter.body_container(1);
    let held = r.matter.economy.ledger.held(body);
    let face = r.units[1].motion.pos.map(|v| v.to_int_trunc());
    // A face in the air at the hull's centre.
    assert!(r.battle_order(0, Order::Dig(face), None));
    for _ in 0..10 {
        r.step();
        assert_books(&r, 0);
    }
    assert_eq!(r.matter.economy.ledger.held(body), held);
    let rc = reclaim(&r);
    assert_eq!((rc.searches, rc.reclaimed, rc.edits), (0, 0, 0));
}

#[test]
fn a_flying_piece_is_not_taken() {
    let mut r = range(layout(1_000_000, 16, 4));
    // The face hangs in the air on the piece's way down.
    let face = [SPOT[0], 40, SPOT[2]];
    assert!(r.battle_order(0, Order::Dig(face), None));
    let (id, _) = drop_piece(&mut r, 6, 2, [SPOT[0], 70, SPOT[2]]);
    let mut passed = false;
    for _ in 0..300 {
        r.step();
        assert_books(&r, 0);
        let f = piece(&r, id).expect("the flying piece was taken");
        let p = at(f);
        passed |= !f.resting && (p[1] - face[1]).abs() <= 2;
        if f.resting {
            break;
        }
    }
    assert!(passed, "the piece never flew past the face");
    assert!(piece(&r, id).unwrap().resting);
    assert_eq!(reclaim(&r).reclaimed, 0);
}

#[test]
fn two_diggers_keep_id_ties_and_partial_work_across_neighboring_cells_and_restore() {
    let mut r = range(layout(1_000_000, 2, 32));
    // First ID is in the higher grid cell: grid traversal must not win ties.
    let (first, _) = drop_piece(&mut r, 6, 2, [296, 30, 322]);
    let (second, _) = drop_piece(&mut r, 6, 2, [264, 30, 322]);
    let (remote, _) = drop_piece(&mut r, 6, 2, [600, 30, 600]);
    let a = settle(&mut r, first);
    let b = settle(&mut r, second);
    settle(&mut r, remote);
    assert_eq!((a[1], a[2]), (b[1], b[2]));
    let face = [(a[0] + b[0]) / 2, a[1], a[2]];
    for id in [0, 1] {
        assert!(r.battle_order(id, Order::Dig(face), None));
    }
    r.step();
    assert_eq!(reclaim(&r).piece_work(0), Some((first, 2)));
    assert_eq!(reclaim(&r).piece_work(1), Some((second, 2)));
    assert_eq!((reclaim(&r).searches, reclaim(&r).edits), (2, 4));
    let bytes = r.snapshot();
    let mut back = BlastRange::restore(config(), &bytes).unwrap();
    assert_eq!(back.snapshot(), bytes);
    for _ in 0..3 {
        r.step();
        back.step();
        assert_eq!(r.state_hash(), back.state_hash());
        assert_books(&r, reclaim(&r).reclaimed);
    }
    assert_eq!((reclaim(&r).pieces, reclaim(&r).reclaimed), (2, 16 * ROCK));
    assert_eq!(r.mining().unwrap().mined, 0);
    assert_eq!(r.tank_held(0), Some(8 * ROCK));
    assert_eq!(r.tank_held(1), Some(8 * ROCK));
    assert!(piece(&r, first).is_none() && piece(&r, second).is_none());
    assert!(piece(&r, remote).is_some_and(|f| f.resting));
}

#[test]
fn a_piece_in_neighboring_cells_but_outside_the_sphere_still_spends_a_miss() {
    let mut r = range(layout(1_000_000, 2, 4));
    let (id, _) = drop_piece(&mut r, 6, 2, SPOT);
    let mut face = settle(&mut r, id);
    face[1] = 100;
    assert!(r.battle_order(0, Order::Dig(face), None));
    for _ in 0..3 {
        r.step();
        assert_books(&r, 0);
    }
    assert_eq!((reclaim(&r).searches, reclaim(&r).search_misses), (3, 3));
    assert_eq!(reclaim(&r).reclaimed, 0);
    assert!(piece(&r, id).is_some_and(|f| f.resting));
}

#[test]
fn reclaim_spends_the_dig_rate_and_respects_the_tank_room() {
    // Sixty-four cells at five edits a tick: thirteen ticks of work.
    let mut r = range(layout(1_000_000, 5, 4));
    let (id, held) = drop_piece(&mut r, 6, 4, SPOT);
    let face = settle(&mut r, id);
    assert!(r.battle_order(0, Order::Dig(face), None));
    for tick in 1..=12 {
        r.step();
        assert_books(&r, 0);
        assert_eq!(r.tank_held(0), Some(0), "tick {tick}");
        assert_eq!(
            reclaim(&r).piece_work(0),
            Some((id, 5 * tick)),
            "tick {tick}"
        );
    }
    r.step();
    assert_books(&r, 64 * ROCK);
    assert_eq!(r.tank_held(0), Some(64 * ROCK));
    assert_eq!(reclaim(&r).edits, 64);
    assert!(piece(&r, id).is_none());
    assert!(!r.matter.economy.ledger.get(held).alive);

    // A tank with room for less than the piece is full: it takes nothing.
    let mut r = range(layout(10 * ROCK, 16, 4));
    let (id, _) = drop_piece(&mut r, 6, 2, SPOT);
    let face = settle(&mut r, id);
    let e = &mut r.matter.economy;
    let tank = r.mining.as_ref().unwrap().tank(0).unwrap();
    e.ledger.reveal(e.ground, 3 * ROCK);
    e.ledger.transfer(e.ground, tank, 3 * ROCK);
    assert!(r.battle_order(0, Order::Dig(face), None));
    for _ in 0..10 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
    }
    assert_eq!(r.tank_held(0), Some(3 * ROCK));
    assert!(piece(&r, id).is_some_and(|f| f.resting));
    assert_eq!(reclaim(&r).searches, 1, "the piece stays its work");
    assert_eq!(reclaim(&r).edits, 0);

    // One bigger than the whole tank is never this digger's.
    let mut r = range(layout(4 * ROCK, 16, 4));
    let (id, _) = drop_piece(&mut r, 6, 2, SPOT);
    let face = settle(&mut r, id);
    assert!(r.battle_order(0, Order::Dig(face), None));
    for _ in 0..3 {
        r.step();
    }
    let rc = reclaim(&r);
    assert_eq!((rc.searches, rc.search_misses, rc.edits), (3, 3, 0));
    assert!(piece(&r, id).is_some());
}

#[test]
fn a_spent_sub_pool_defers_reclaim_and_searches_wait_their_turn() {
    let mut l = layout(1_000_000, 4, 4);
    let e = l.economy.as_mut().unwrap();
    e.dig_edits_in_flight = 4;
    e.reclaim.as_mut().unwrap().searches_per_tick = 1;
    let mut r = range(l);
    let (a, _) = drop_piece(&mut r, 6, 2, SPOT);
    let (b, _) = drop_piece(&mut r, 2, 2, [SPOT[0], SPOT[1], SPOT[2] + 24]);
    let face_a = settle(&mut r, a);
    let face_b = settle(&mut r, b);
    assert!(r.battle_order(0, Order::Dig(face_a), None));
    assert!(r.battle_order(1, Order::Dig(face_b), None));
    r.step();
    // One search a tick: the other digger waits for the next.
    let rc = reclaim(&r);
    assert_eq!((rc.searches, rc.search_deferred), (1, 1));
    for _ in 0..10 {
        r.step();
        assert_books(&r, reclaim(&r).reclaimed);
    }
    assert!(piece(&r, a).is_none() && piece(&r, b).is_none());
    assert_eq!(r.tank_held(0), Some(8 * ROCK));
    assert_eq!(r.tank_held(1), Some(8 * CONCRETE));
    // Sixteen edits at four a tick, shared: the sub-pool ran out with
    // work waiting.
    assert!(r.mining().unwrap().deferred > 0);
    assert_eq!(reclaim(&r).edits, 16);
}

#[test]
fn a_piece_resting_on_a_reclaimed_one_falls() {
    let mut r = range(layout(1_000_000, 16, 2));
    let (below, _) = drop_piece(&mut r, 6, 4, SPOT);
    let face = settle(&mut r, below);
    let (above, _) = drop_piece(&mut r, 2, 2, [SPOT[0] + 1, 50, SPOT[2] + 1]);
    let rested = settle(&mut r, above);
    assert!(rested[1] > face[1] + 2, "it lies on the piece: {rested:?}");
    // Nothing fell in between and nothing merged.
    assert!(piece(&r, below).is_some_and(|f| f.resting));
    assert!(r.battle_order(0, Order::Dig(face), None));
    let mut gone = None;
    for tick in 0..40 {
        r.step();
        assert_books(&r, reclaim(&r).reclaimed);
        if gone.is_none() && piece(&r, below).is_none() {
            gone = Some(tick);
        }
        if let Some(t) = gone {
            if piece(&r, above).is_none_or(|f| !f.resting) {
                // Within the rest check of the piece (8 ticks, ADR-291).
                assert!(tick - t <= 8, "it hung {} ticks", tick - t);
                return;
            }
        }
    }
    panic!("the piece on the reclaimed one never fell: {gone:?}");
}

#[test]
fn a_restore_mid_reclaim_continues_bit_for_bit() {
    let mut a = range(layout(1_000_000, 3, 4));
    let (id, _) = drop_piece(&mut a, 6, 4, SPOT);
    let face = settle(&mut a, id);
    assert!(a.battle_order(0, Order::Dig(face), None));
    for _ in 0..5 {
        a.step();
    }
    assert_eq!(reclaim(&a).piece_work(0), Some((id, 15)));
    let bytes = a.snapshot();
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 64);
    let mut b = BlastRange::restore(config(), &bytes).expect("restore mid-reclaim");
    assert_eq!(b.snapshot(), bytes);
    // Work on a piece that is not there is refused, and so is work past
    // the piece's cells.
    for forged in [
        Work::Piece {
            id: id + 100,
            spent: 1,
        },
        Work::Piece { id, spent: 64 },
    ] {
        let mut f = BlastRange::restore(config(), &bytes).unwrap();
        let m = f.mining.as_mut().unwrap();
        m.reclaim.as_mut().unwrap().work[0] = Some(forged);
        assert!(BlastRange::restore(config(), &f.snapshot()).is_none());
    }
    for tick in 0..40 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash(), "tick {tick}");
        assert_books(&b, reclaim(&b).reclaimed);
    }
    assert_eq!(b.tank_held(0), Some(64 * ROCK));
    assert!(piece(&b, id).is_none());
}

#[test]
fn a_dead_diggers_cargo_breaches_and_the_rest_is_reclaimed_by_digging() {
    let mut l = layout(1_000_000, 16, 8);
    l.blocks.push(RangeBlock {
        min: [266, 1, 316],
        max: [296, 15, 456],
        material: 6,
    });
    let mut r = range(l);
    assert!(r.battle_order(0, Order::Dig(FACE), None));
    for _ in 0..4 {
        r.step();
        assert_books(&r, 0);
    }
    let cargo = r.tank_held(0).unwrap();
    assert!(cargo > 0);
    r.units[0].status.alive = false;
    r.step();
    assert_books(&r, 0);
    // A quarter of the cargo is lost to the breach; the rest lies with the
    // hull, in the tank.
    let lost = cargo / 4;
    assert_eq!(reclaim(&r).lost, lost);
    assert_eq!(r.tank_held(0), Some(cargo - lost));
    r.step();
    assert_eq!(reclaim(&r).lost, lost, "the breach is once");
    // A living digger digs at the hull and takes the rest.
    // A face in the air over the hull, within reach of it.
    let hull = r.units[0].motion.pos.map(|v| v.to_int_trunc());
    let hull = [hull[0], hull[1] + 4, hull[2]];
    assert!(r.battle_order(1, Order::Dig(hull), None));
    for _ in 0..30 {
        r.step();
        assert_books(&r, 0);
        if r.tank_held(0) == Some(0) {
            break;
        }
    }
    assert_eq!(r.tank_held(0), Some(0));
    assert_eq!(r.tank_held(1), Some(cargo - lost));
    let rc = reclaim(&r);
    assert_eq!(rc.reclaimed, cargo - lost);
    // A cell of rock an edit.
    assert_eq!(rc.edits, (cargo - lost).div_ceil(ROCK));
    assert_eq!(r.mining().unwrap().mined, cargo);
    // Without the reclaim section the cargo stays whole (ADR-673).
    let mut l = layout(1_000_000, 16, 8);
    l.economy.as_mut().unwrap().reclaim = None;
    l.blocks.push(RangeBlock {
        min: [266, 1, 316],
        max: [296, 15, 456],
        material: 6,
    });
    let mut p = range(l);
    assert!(p.battle_order(0, Order::Dig(FACE), None));
    for _ in 0..4 {
        p.step();
    }
    p.units[0].status.alive = false;
    p.step();
    assert_eq!(p.tank_held(0), Some(cargo));
    assert_eq!(p.matter.economy.ledger.sunk(Sink::ContainerBreach), 0);
    assert_eq!(
        u32::from_le_bytes(p.snapshot()[8..12].try_into().unwrap()),
        62
    );
}

#[test]
fn a_restored_dead_digger_is_breached_once() {
    let mut l = layout(1_000_000, 16, 8);
    l.blocks.push(RangeBlock {
        min: [266, 1, 316],
        max: [296, 15, 456],
        material: 6,
    });
    let mut a = range(l);
    assert!(a.battle_order(0, Order::Dig(FACE), None));
    for _ in 0..4 {
        a.step();
    }
    a.units[0].status.alive = false;
    a.step();
    let hull = a.units[0].motion.pos.map(|v| v.to_int_trunc());
    let hull = [hull[0], hull[1] + 4, hull[2]];
    assert!(a.battle_order(1, Order::Dig(hull), None));
    a.step();
    let bytes = a.snapshot();
    let mut b = BlastRange::restore(config(), &bytes).expect("restore");
    assert_eq!(b.snapshot(), bytes);
    for tick in 0..20 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash(), "tick {tick}");
        assert_books(&b, 0);
    }
    assert_eq!(reclaim(&a).lost, reclaim(&b).lost);
}

#[test]
fn with_banks_a_piece_that_does_not_fit_sends_the_tank_to_haul() {
    let mut l = layout(10 * ROCK, 16, 4);
    l.units.push(modding::range::RangeUnitDef {
        kind: RangeUnitKind::Base,
        allied: true,
        at: [520, 16, 260],
        weapon: None,
    });
    let stock = RangeEconomy::stock();
    let e = l.economy.as_mut().unwrap();
    e.bank = stock.bank;
    e.kind_mut(RangeUnitKind::Base).unwrap().store = 1_000_000;
    let mut r = range(l);
    let (id, _) = drop_piece(&mut r, 6, 2, SPOT);
    let face = settle(&mut r, id);
    let e = &mut r.matter.economy;
    let tank = r.mining.as_ref().unwrap().tank(0).unwrap();
    e.ledger.reveal(e.ground, 3 * ROCK);
    e.ledger.transfer(e.ground, tank, 3 * ROCK);
    assert!(r.battle_order(0, Order::Dig(face), None));
    r.step();
    assert!(r.hauling(0), "the piece filled the tank's room: it hauls");
    // Stores and reclaim: version 64 with the haul's section.
    let bytes = r.snapshot();
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 64);
    let mut b = BlastRange::restore(config(), &bytes).expect("restore with banks");
    assert_eq!(b.snapshot(), bytes);
    let mut came_back = false;
    for tick in 0..3000 {
        r.step();
        b.step();
        assert_eq!(r.state_hash(), b.state_hash(), "tick {tick}");
        assert!(b.matter.economy.ledger.balanced());
        if piece(&b, id).is_none() {
            came_back = true;
            break;
        }
    }
    assert!(came_back, "unloaded, it came back and took the piece");
    assert_eq!(b.tank_held(0), Some(8 * ROCK));
    assert!(b.side_store(true).unwrap().0 > 0);
}
