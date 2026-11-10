//! Production and produced units (plan 245, slice 4; ADR-675).
use super::super::super::battle::commands::{Command, Outcome};
use super::super::super::battle::Order;
use super::super::super::BlastRange;
use crate::matter::{Amount, Sink};
use modding::explosion::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef, RangeUnitKind, MAX_RANGE_UNITS};
use modding::range_economy::{Producer, RangeEconomy};

/// Slots of the fixture's three allied tanks, then the bases this file adds.
const BASE: usize = 3;
const SECOND_BASE: usize = 4;
/// One game second at the fixed clock: the stock build time of the tests.
const BUILD: u32 = crate::TICKS_PER_SECOND as u32;
/// A buried slab of rock beside the fixture's tanks, as in the dig tests.
const FACE: [i32; 3] = [280, 8, 330];

#[test]
fn scenario_starting_crews_are_symmetric_and_not_duplicated() {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.size = [5000, 1024, 5000];
    l.economy = Some(RangeEconomy::stock());
    l.units = [true, false].into_iter().map(|allied| RangeUnitDef {
        kind: RangeUnitKind::GrebenStorage, allied,
        at: if allied { [1000, 16, 1000] } else { [4800, 16, 4800] }, weapon: None,
    }).collect();
    super::starting_crews(&mut l);
    for side in [true, false] {
        assert_eq!(l.units.iter().filter(|u| u.allied == side && u.kind == RangeUnitKind::SkobaDrone).count(), 8);
        assert_eq!(l.units.iter().filter(|u| u.allied == side && u.kind == RangeUnitKind::Crawler).count(), 1);
    }
    assert!(l.units.iter().all(|u| (0..3).all(|axis| u.at[axis] >= 0 && u.at[axis] < l.size[axis] as i32)));
    let mut bounds = Vec::new();
    for u in l.units.iter().skip(2) {
        let unit = super::RangeUnit::placed(u.kind, u.allied, u.at, true);
        let (lo, hi) = unit.bounds();
        assert!((0..3).all(|axis| lo[axis] >= 0 && hi[axis] <= l.size[axis] as i32), "{:?} {:?}: {lo:?}..{hi:?}", u.kind, u.at);
        assert!(bounds.iter().all(|(a, b): &([i32; 3], [i32; 3])| (0..3).any(|axis| hi[axis] <= a[axis] || b[axis] <= lo[axis])), "overlapping starting crew {:?} {:?}", u.kind, u.at);
        bounds.push((lo, hi));
    }
    let count = l.units.len();
    super::starting_crews(&mut l);
    assert_eq!(l.units.len(), count);
}

#[test]
fn saved_enemy_storage_without_crew_restores_its_original_roster() {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.idle_enemies = true;
    l.size = [1024, 256, 1024];
    l.clear_min = [64, 64];
    l.clear_max = [960, 960];
    l.blocks = vec![
        RangeBlock { min: [0, 0, 0], max: [1024, 256, 1024], material: 0 },
        RangeBlock { min: [0, 0, 0], max: [1024, 16, 1024], material: 1 },
    ];
    l.shields.clear();
    l.course = None;
    l.economy = Some(RangeEconomy::stock());
    l.units = vec![RangeUnitDef { kind: RangeUnitKind::GrebenStorage, allied: false,
        at: [500, 16, 500], weapon: None }];
    // This was a valid roster before enemy crews became symmetrical.
    let r = BlastRange::with_saved_layout(config(), l).unwrap();
    assert_eq!(r.units.len(), 1);
    let restored = BlastRange::restore(r.config, &r.snapshot()).expect("legacy roster restores");
    assert_eq!(restored.units.len(), 1);
    assert_eq!(restored.state_hash(), r.state_hash());
}

fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
}

fn base(allied: bool, at: [i32; 3]) -> RangeUnitDef {
    RangeUnitDef {
        kind: RangeUnitKind::Base,
        allied,
        at,
        weapon: None,
    }
}

/// The economy of the tests: a base builds tanks and striders in a game
/// second each, tanks dig, stores are wide enough for any test.
fn economy() -> RangeEconomy {
    let mut e = RangeEconomy::stock();
    e.balance=None; e.dig_periods.clear();
    for k in &mut e.kinds { k.body_density=0; }
    e.reclaim = None;
    let k = e.kind_mut(RangeUnitKind::Tank).unwrap();
    k.tank = 40 * 260;
    k.dig_radius = 3;
    k.dig_reach = 120;
    k.dig_edits_per_tick = 16;
    k.build_secs = 1;
    e.kind_mut(RangeUnitKind::Strider).unwrap().build_secs = 1;
    e.bank.as_mut().unwrap().base_store = 1 << 40;
    let p = e.production.as_mut().unwrap();
    p.queue = 3;
    p.producers = vec![Producer {
        kind: RangeUnitKind::Base,
        produces: vec![RangeUnitKind::Tank, RangeUnitKind::Strider],
    }];
    e
}

/// The fixture's three allied tanks, an allied base and the economy.
fn layout() -> RangeLayout {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.units.push(base(true, [520, 16, 260]));
    l.economy = Some(economy());
    l
}

fn range(l: RangeLayout) -> BlastRange {
    let mut r = BlastRange::with_layout(config(), l).unwrap();
    r.start_free_control().unwrap();
    r
}

/// Matter revealed from the ground into a side's store, as hauling would.
fn seed(r: &mut BlastRange, allied: bool, amount: Amount) {
    let store = r.mining().unwrap().haul().unwrap().store(allied);
    let e = &mut r.matter.economy;
    e.ledger.reveal(e.ground, amount);
    assert_eq!(e.ledger.transfer(e.ground, store, amount), amount);
}

fn stores(r: &BlastRange) -> Amount {
    r.side_store(true).unwrap().0 + r.side_store(false).unwrap().0
}

/// What production holds: the lines' blanks and the produced bodies.
fn held(r: &BlastRange) -> (Amount, Amount) {
    let l = &r.matter.economy.ledger;
    let blanks = r
        .production_lines()
        .into_iter()
        .map(|id| r.production_paid(id).unwrap())
        .sum();
    let bodies = r.spawned().iter().map(|s| l.held(s.body)).sum();
    (blanks, bodies)
}

/// The books balance, and every unit of matter seeded is in a store, a
/// blank, a produced body or the sinks production uses.
fn assert_books(r: &BlastRange, seeded: Amount) {
    let l = &r.matter.economy.ledger;
    assert!(l.balanced(), "imbalance {}", l.imbalance());
    let (blanks, bodies) = held(r);
    assert_eq!(
        stores(r) + blanks + bodies + l.sunk(Sink::OverCap) + l.sunk(Sink::ContainerBreach),
        seeded
    );
    let p = r.mining().unwrap().production().unwrap();
    assert_eq!(p.paid, blanks + bodies + p.refunded + p.overflow + p.lost);
    assert_eq!(p.produced as usize, r.spawned().len());
}

fn run(
    r: &mut BlastRange,
    seeded: Amount,
    ticks: usize,
    done: impl Fn(&BlastRange) -> bool,
) -> bool {
    for _ in 0..ticks {
        r.step();
        assert_books(r, seeded);
        if done(r) {
            return true;
        }
    }
    false
}

fn version(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[8..12].try_into().unwrap())
}

#[test]
fn a_producer_turns_stock_and_time_into_a_unit() {
    let mut r = range(layout());
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    // The cost is the tank's template matter: the body of an authored tank.
    let l = &r.matter.economy.ledger;
    assert_eq!(cost, l.held(r.matter.body_of(0).unwrap()));
    let seeded = 3 * cost;
    seed(&mut r, true, seeded);
    let before = r.units.len();
    // Nothing produced yet: the snapshot is the banks'.
    assert_eq!(version(&r.snapshot()), 63);
    assert_eq!(
        r.execute_command(Command::Produce {
            actor: BASE,
            kind: RangeUnitKind::Tank,
        }),
        Outcome::Applied
    );
    assert_eq!(r.production_queue(BASE), Some(vec![RangeUnitKind::Tank]));
    // Round the accumulated share: no repeated rounding surcharge per tick.
    for tick in 1..BUILD {
        r.step();
        assert_books(&r, seeded);
        let cumulative = (cost * u64::from(tick)).div_ceil(u64::from(BUILD));
        assert_eq!(r.production_paid(BASE), Some(cumulative));
        assert_eq!(r.units.len(), before, "tick {tick}");
    }
    r.step();
    assert_books(&r, seeded);
    // Paid and stood: the tank joins the roster at the end of the tick.
    assert_eq!(r.units.len(), before + 1);
    let id = before;
    assert_eq!(r.units[id].kind, RangeUnitKind::Tank);
    assert!(r.units[id].status.alive);
    assert!(r.allied(id));
    assert_eq!(r.battle.actors.len(), r.units.len());
    assert_eq!(r.battle.actors[id].order, Order::Auto);
    let s = &r.spawned()[0];
    assert_eq!(r.unit_def(id), Some(&s.def));
    assert_eq!(r.roster().count(), r.units.len());
    assert_eq!(r.matter.economy.ledger.held(s.body), cost);
    assert_eq!(r.side_store(true).unwrap().0, seeded - cost);
    // The tank digs: it brought an empty tank of its kind.
    assert_eq!(r.tank_fill(id), Some((0, 40 * 260)));
    // The line is gone with its blank.
    assert_eq!(r.production_queue(BASE), None);
    assert!(r.production_lines().is_empty());
    // It stands clear of the base and of every other hull.
    let (lo, hi) = r.units[id].bounds();
    for (other, u) in r.units.iter().enumerate().filter(|(o, _)| *o != id) {
        let (olo, ohi) = u.bounds();
        assert!(
            (0..3).any(|k| lo[k] >= ohi[k] || olo[k] >= hi[k]),
            "{id} overlaps {other}"
        );
    }
    let p = r.mining().unwrap().production().unwrap();
    assert_eq!((p.produced, p.stalled, p.capped, p.blocked), (1, 0, 0, 0));
    // The read model sees it on its side.
    let view = super::super::super::read_model::View::new(&r, 1);
    assert_eq!(view.units.len(), r.units.len());
    assert!(view.units[id].allied);
    assert_eq!(view.counts[0], 5);
    // The snapshot is production's.
    assert_eq!(version(&r.snapshot()), 65);
}

#[test]
fn kil_production_progress_and_exit_use_its_front_gate() {
    let mut l = modding::laboratory::Definition::stock().layout;
    l.size = [1536, 256, 1536];
    l.clear_max = [1535, 1535];
    l.battle = true;
    l.units = vec![RangeUnitDef { kind: RangeUnitKind::KilFactoryT1, allied: true, at: [800, 16, 800], weapon: None }];
    let mut economy = economy();
    // This test isolates gate movement, without a storage network.
    economy.production.as_mut().unwrap().supply_radius = 0;
    economy.production.as_mut().unwrap().pipeline_cost_per_cell = 0;
    economy.production.as_mut().unwrap().producers = vec![Producer {
        kind: RangeUnitKind::KilFactoryT1,
        produces: vec![RangeUnitKind::Tank],
    }];
    l.economy = Some(economy);
    let mut r = range(l);
    for yaw in 0..4 {
        let head = yaw * (fx::angle::FULL / 4);
        r.units[0].motion.drive.head = head;
        r.units[0].update_pose(head);
        let at = r.free_spot(0, RangeUnitKind::Tank, true).expect("clear gate");
        let offset = [at[0] - 800, at[2] - 800];
        match yaw {
            0 => assert!(offset[0] < 0 && offset[1] == 0),
            1 => assert!(offset[1] < 0 && offset[0] == 0),
            2 => assert!(offset[0] > 0 && offset[1] == 0),
            _ => assert!(offset[1] > 0 && offset[0] == 0),
        }
    }
    r.units[0].motion.drive.head = 0;
    r.units[0].update_pose(0);
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    seed(&mut r, true, cost * 2);
    assert_eq!(r.enqueue_product(0, RangeUnitKind::Tank, true), Outcome::Applied);
    assert_eq!(r.production_progress(0), Some((0, BUILD, 0, cost)));
    for tick in 1..=BUILD {
        r.tick = tick;
        r.production_phase();
        if tick < BUILD {
            let (elapsed, total, paid, expected) = r.production_progress(0).unwrap();
            assert_eq!((elapsed, total, expected), (tick, BUILD, cost));
            assert!(paid > 0 && paid <= cost);
        }
    }
    let (product, born) = r.production_exit(0).expect("exit cue");
    assert_eq!(born, BUILD);
    assert_eq!(r.production_queue(0), None);
    let at = r.units[product].at;
    assert_eq!(at, [640, 16, 800], "new unit must start inside the actual gate");
    assert_eq!(r.units[product].motion.drive.head, fx::angle::FULL / 2);
    assert_eq!(r.battle.actors[product].turret.angle, fx::angle::FULL / 2);
    assert_eq!(r.factory_exiting_unit(0), Some(product));
    assert!(matches!(r.battle.actors[product].order, Order::Pilot { throttle: 1, turn: 0, fire: false, .. }));
    assert!(r.free_spot(0, RangeUnitKind::Tank, true).is_none(), "a tank standing in the corridor must block later production");
    let launch = r.battle.actors[product].order;
    r.battle.actors[product].order = Order::Hold;
    assert!(r.free_spot(0, RangeUnitKind::Tank, true).is_none(), "manual stop inside the gate must still block the next product");
    r.battle.actors[product].order = launch;
    assert_eq!(r.enqueue_product(0, RangeUnitKind::Tank, true), Outcome::Applied);
    for _ in 0..BUILD + 2 { r.tick += 1; r.production_phase(); }
    assert_eq!(r.units.len(), 2, "second tank waits behind the blocked gate");
    assert_eq!(r.production_progress(0), Some((BUILD, BUILD, cost, cost)));
    r.units[product].motion.pos[2] = fx::Fx::from_int(1200);
    r.units[product].update_pose(fx::angle::FULL / 2);
    r.finish_factory_launches();
    r.tick += 1;
    r.production_phase();
    assert_eq!(r.units.len(), 3, "production resumes after the corridor clears");
    let bytes = r.snapshot();
    let restored = BlastRange::restore(config(), &bytes).expect("production snapshot restores");
    assert_eq!(restored.state_hash(), r.state_hash());
}

#[test]
fn kil_tank_drives_out_front_first_at_every_factory_rotation() {
    for yaw in 0..4 {
        let mut l = modding::laboratory::Definition::stock().layout;
        l.size = [1536, 256, 1536]; l.clear_max = [1535, 1535]; l.battle = true;
        l.units = vec![RangeUnitDef { kind: RangeUnitKind::KilFactoryT1, allied: true, at: [800, 16, 800], weapon: None }];
        l.unit_yaws = vec![yaw];
        let mut e = economy();
        e.production.as_mut().unwrap().supply_radius = 0;
        e.production.as_mut().unwrap().pipeline_cost_per_cell = 0;
        e.production.as_mut().unwrap().producers = vec![Producer { kind: RangeUnitKind::KilFactoryT1, produces: vec![RangeUnitKind::Tank] }];
        l.economy = Some(e);
        let mut r = range(l);
        r.set_physics_workers(1); r.set_navigation_workers(1);
        let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
        seed(&mut r, true, cost);
        assert_eq!(r.enqueue_product(0, RangeUnitKind::Tank, true), Outcome::Applied);
        for _ in 0..BUILD { r.production_phase(); }
        let (id, _) = r.production_exit(0).unwrap();
        let head = (i32::from(yaw) * fx::angle::FULL / 4 + fx::angle::HALF) % fx::angle::FULL;
        assert_eq!(r.units[id].motion.drive.head, head);
        assert_eq!(r.battle.actors[id].turret.angle, head);
        let before = r.units[id].motion.pos;
        for _ in 0..25 { r.step(); }
        let p = r.units[id].motion.pos;
        let along = p[0].sub(before[0]).mul(fx::angle::cos(head))
            .add(p[2].sub(before[2]).mul(fx::angle::sin(head)));
        assert!(along > fx::Fx::ZERO, "yaw {yaw}: no forward motion: {:?}", r.units[id].at);
        assert!(fx::angle::diff(r.battle.actors[id].turret.angle, r.units[id].motion.drive.head).abs() < fx::angle::FULL / 180);
    }
}

#[test]
fn kil_product_exits_under_its_own_power_and_restores_in_gate() {
    let mut l = modding::laboratory::Definition::stock().layout;
    l.size = [1536, 256, 1536]; l.clear_max = [1535, 1535]; l.battle = true;
    l.units = vec![RangeUnitDef { kind: RangeUnitKind::KilFactoryT1, allied: true, at: [800, 16, 800], weapon: None }];
    let mut e = economy();
    e.production.as_mut().unwrap().supply_radius = 0;
    e.production.as_mut().unwrap().pipeline_cost_per_cell = 0;
    e.production.as_mut().unwrap().producers = vec![Producer { kind: RangeUnitKind::KilFactoryT1, produces: vec![RangeUnitKind::Tank] }];
    l.economy = Some(e);
    let mut r = range(l);
    r.set_physics_workers(1); r.set_navigation_workers(1);
    assert!(r.set_factory_rally(0, [1100, 1100]));
    assert!(!r.set_factory_rally(0, [0, 0]));
    assert_eq!(r.factory_rally(0), Some([1100, 1100]));
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    seed(&mut r, true, cost);
    assert_eq!(r.enqueue_product(0, RangeUnitKind::Tank, true), Outcome::Applied);
    for _ in 0..BUILD { r.production_phase(); }
    let (id, _) = r.production_exit(0).unwrap();
    let initial = r.units[id].motion.pos;
    let speed = r.units[id].motion.speed;
    let mut moved = false;
    for _ in 0..15 {
        let before = r.units[id].motion.pos;
        r.step();
        let after = r.units[id].motion.pos;
        let dx = after[0].sub(before[0]);
        let dz = after[2].sub(before[2]);
        assert!(dx.mul(dx).add(dz.mul(dz)).sqrt() <= fx::Fx::from_ratio(speed + 2, crate::TICKS_PER_SECOND), "product exceeded its speed");
        assert!(after[0] <= before[0], "product moved backwards");
        assert!(fx::angle::diff(r.battle.actors[id].turret.angle, r.units[id].motion.drive.head).abs() < fx::angle::FULL / 180);
        moved |= after[0] < initial[0];
    }
    assert!(moved, "product never moved inside gate: pos={:?}, blocked={}, pilot={:?}", r.units[id].at, r.units[id].motion.blocked, r.units[id].motion.pilot);
    assert!(r.set_factory_rally(0, [1200, 1100]));
    let mut saved = BlastRange::restore(r.config, &r.snapshot()).expect("factory exit restores");
    assert_eq!(saved.factory_rally(0), Some([1200, 1100]));
    for _ in 0..400 {
        r.step(); saved.step();
        assert_eq!(r.state_hash(), saved.state_hash());
        if r.factory_exiting_unit(0).is_none() { break; }
    }
    assert!(r.factory_exiting_unit(0).is_none(), "product remained in gate: pos {:?}, blocked {}", r.units[id].at, r.units[id].motion.blocked);
    assert!(r.units[id].at[0] < initial[0].to_int_trunc() - 100);
    assert_eq!(r.units[id].motion.destination, Some([1200, 1100]), "latest rally must be assigned only after clearing the gate");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn factory_pipeline_cost_radius_damage_and_snapshot_are_accounted_for() {
    let mut l = modding::laboratory::Definition::stock().layout;
    l.size = [1536, 256, 1536];
    l.clear_max = [1535, 1535];
    l.battle = true;
    l.units = vec![
        RangeUnitDef { kind: RangeUnitKind::KilFactoryT1, allied: true, at: [1000, 16, 800], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Base, allied: true, at: [200, 16, 800], weapon: None },
    ];
    let mut e = economy();
    let p = e.production.as_mut().unwrap();
    p.supply_radius = 1000;
    p.pipeline_cost_per_cell = 1000;
    p.producers = vec![Producer { kind: RangeUnitKind::KilFactoryT1, produces: vec![RangeUnitKind::Tank] }];
    l.economy = Some(e);
    let mut r = range(l);
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    let quote = r.factory_supply_quote(0).unwrap();
    assert_eq!(quote, (1, 800, 800_000));
    assert_eq!(r.supply_quote_at([700, 16, 800], true).unwrap().2, 500_000);
    assert!(r.supply_quote_at([1000, 16, 800], false).is_none(), "enemy cannot use this storage");
    r.production_phase();
    assert!(!r.factory_supply_connected(0), "a connection must not be free");
    r.units[1].motion.pos[2] = fx::Fx::from_int(1520);
    r.units[1].update_pose(0);
    assert!(r.factory_supply_quote(0).is_none(), "storage outside the radius cannot supply");
    r.units[1].motion.pos[2] = fx::Fx::from_int(800);
    r.units[1].update_pose(0);
    seed(&mut r, true, cost * 2 + quote.2);
    let before = r.side_store(true).unwrap().0;
    r.production_phase();
    assert!(r.factory_supply_connected(0));
    assert_eq!(r.side_store(true).unwrap().0, before - quote.2);
    r.production_phase();
    assert_eq!(r.side_store(true).unwrap().0, before - quote.2, "the same connection is paid only once");
    assert!(r.matter.economy.ledger.balanced());
    let bytes = r.snapshot();
    let restored = BlastRange::restore(config(), &bytes).expect("pipeline must survive saving");
    assert!(restored.factory_supply_connected(0));
    assert_eq!(restored.snapshot(), bytes);
    assert_eq!(restored.state_hash(), r.state_hash());
    assert_eq!(r.enqueue_product(0, RangeUnitKind::Tank, true), Outcome::Applied);
    r.production_phase();
    let paid = r.production_paid(0).unwrap();
    assert!(paid > 0);
    r.units[1].status.alive = false;
    assert!(!r.factory_supply_connected(0));
    r.production_phase();
    assert_eq!(r.production_paid(0), Some(paid), "a destroyed storage cannot supply the factory");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn an_empty_store_stalls_production() {
    let mut r = range(layout());
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    let half = cost / 2;
    seed(&mut r, true, half);
    assert_eq!(
        r.execute_command(Command::Produce {
            actor: BASE,
            kind: RangeUnitKind::Tank,
        }),
        Outcome::Applied
    );
    let before = r.units.len();
    assert!(!run(&mut r, half, 2 * BUILD as usize, |r| r.units.len() > before));
    // It paid what the store had and waits for the rest, counted.
    assert_eq!(r.production_paid(BASE), Some(half));
    assert_eq!(r.side_store(true).unwrap().0, 0);
    let p = r.mining().unwrap().production().unwrap();
    assert!(p.stalled > 0);
    assert_eq!(p.produced, 0);
    // Matter arrives: the line pays on and the tank is out.
    seed(&mut r, true, cost);
    assert!(run(&mut r, half + cost, BUILD as usize, |r| r.units.len()
        > before));
    assert_eq!(r.side_store(true).unwrap().0, half);
}

#[test]
fn cancelling_an_order_returns_the_matter() {
    let mut r = range(layout());
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    let seeded = 2 * cost;
    seed(&mut r, true, seeded);
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
        assert_eq!(
            r.execute_command(Command::Produce { actor: BASE, kind }),
            Outcome::Applied
        );
    }
    run(&mut r, seeded, 10, |_| false);
    let paid = r.production_paid(BASE).unwrap();
    assert!(paid > 0);
    // The last order goes first; the front keeps its blank.
    assert_eq!(
        r.execute_command(Command::Cancel { actor: BASE }),
        Outcome::Applied
    );
    assert_eq!(r.production_queue(BASE), Some(vec![RangeUnitKind::Tank]));
    assert_eq!(r.production_paid(BASE), Some(paid));
    // The only order: the blank goes back to the store, all of it.
    assert_eq!(
        r.execute_command(Command::Cancel { actor: BASE }),
        Outcome::Applied
    );
    assert_books(&r, seeded);
    assert_eq!(r.production_queue(BASE), None);
    assert_eq!(r.side_store(true).unwrap().0, seeded);
    let l = &r.matter.economy.ledger;
    assert_eq!(l.sunk(Sink::OverCap), 0);
    // Nothing to cancel.
    assert_eq!(
        r.execute_command(Command::Cancel { actor: BASE }),
        Outcome::Rejected
    );
    // A refund that no longer fits the store goes to OverCap.
    assert_eq!(
        r.execute_command(Command::Produce {
            actor: BASE,
            kind: RangeUnitKind::Tank,
        }),
        Outcome::Applied
    );
    run(&mut r, seeded, 5, |_| false);
    let paid = r.production_paid(BASE).unwrap();
    let (held, cap) = r.side_store(true).unwrap();
    seed(&mut r, true, cap - held);
    let seeded = seeded + cap - held;
    assert_eq!(
        r.execute_command(Command::Cancel { actor: BASE }),
        Outcome::Applied
    );
    assert_books(&r, seeded);
    let l = &r.matter.economy.ledger;
    assert_eq!(l.sunk(Sink::OverCap), paid);
    let p = r.mining().unwrap().production().unwrap();
    assert_eq!(p.overflow, paid);
}

#[test]
fn orders_are_validated() {
    let mut r = range(layout());
    let produce = |actor, kind| Command::Produce { actor, kind };
    // A tank produces nothing; a base does not produce a spider.
    for c in [
        produce(0, RangeUnitKind::Tank),
        produce(BASE, RangeUnitKind::Spider),
        produce(99, RangeUnitKind::Tank),
    ] {
        assert_eq!(r.execute_command(c), Outcome::Rejected);
    }
    // The queue holds three.
    for _ in 0..3 {
        assert_eq!(
            r.execute_command(produce(BASE, RangeUnitKind::Tank)),
            Outcome::Applied
        );
    }
    assert_eq!(
        r.execute_command(produce(BASE, RangeUnitKind::Tank)),
        Outcome::Rejected
    );
    // An enemy producer is not the player's.
    let mut l = layout();
    l.units.push(base(false, [520, 16, 600]));
    let mut r = range(l);
    assert_eq!(
        r.execute_command(produce(SECOND_BASE, RangeUnitKind::Tank)),
        Outcome::Rejected
    );
    // A dead producer orders nothing.
    r.units[BASE].status.alive = false;
    assert_eq!(
        r.execute_command(produce(BASE, RangeUnitKind::Tank)),
        Outcome::Rejected
    );
    // Without a production section nobody produces.
    let mut l = layout();
    l.economy.as_mut().unwrap().production = None;
    let mut r = range(l);
    assert_eq!(
        r.execute_command(produce(BASE, RangeUnitKind::Tank)),
        Outcome::Rejected
    );
}

#[test]
fn lines_are_kept_in_slot_order() {
    let mut l = layout();
    l.units.push(base(true, [520, 16, 508]));
    let mut r = range(l);
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    let share = cost.div_ceil(u64::from(BUILD));
    // The higher slot orders first.
    for actor in [SECOND_BASE, BASE] {
        assert_eq!(
            r.execute_command(Command::Produce {
                actor,
                kind: RangeUnitKind::Tank,
            }),
            Outcome::Applied
        );
    }
    assert_eq!(r.production_lines(), vec![BASE, SECOND_BASE]);
    // One share in the store: the lower slot pays, the higher stalls.
    seed(&mut r, true, share);
    r.step();
    assert_books(&r, share);
    assert_eq!(r.production_paid(BASE), Some(share));
    assert_eq!(r.production_paid(SECOND_BASE), Some(0));
    let p = r.mining().unwrap().production().unwrap();
    assert_eq!(p.stalled, 1);
}

/// Leave exactly one roster slot for the two competing production lines.
fn crowded() -> RangeLayout {
    let mut l = layout();
    l.size = [1600, 192, 1600];
    l.clear_min = [1, 1];
    l.clear_max = [1599, 1599];
    l.units.clear();
    for row in 0..(MAX_RANGE_UNITS - 3).div_ceil(16) {
        for col in 0..16 {
            if l.units.len() == MAX_RANGE_UNITS - 3 {
                break;
            }
            l.units.push(RangeUnitDef {
                kind: RangeUnitKind::Tank,
                allied: true,
                at: [100 + 80 * col as i32, 16, 100 + 60 * row as i32],
                weapon: None,
            });
        }
    }
    l.units.push(base(true, [200, 16, 1000]));
    l.units.push(base(true, [900, 16, 1000]));
    l
}

#[test]
fn at_the_roster_ceiling_a_finished_unit_waits_in_its_line() {
    let mut r = range(crowded());
    assert_eq!(r.units.len(), MAX_RANGE_UNITS - 1);
    let (first, second) = (MAX_RANGE_UNITS - 3, MAX_RANGE_UNITS - 2);
    let cost = r.production_cost(RangeUnitKind::Strider).unwrap();
    let seeded = 2 * cost;
    seed(&mut r, true, seeded);
    for actor in [first, second] {
        assert_eq!(
            r.execute_command(Command::Produce {
                actor,
                kind: RangeUnitKind::Strider,
            }),
            Outcome::Applied
        );
    }
    assert!(run(&mut r, seeded, BUILD as usize, |r| r.units.len()
        == MAX_RANGE_UNITS));
    // Both finished on one tick: the lower slot took the last place, the
    // higher waits paid in full, counted every tick it waits.
    assert_eq!(r.spawned().len(), 1);
    assert_eq!(r.production_lines(), vec![second]);
    assert_eq!(r.production_paid(second), Some(cost));
    let capped = r.mining().unwrap().production().unwrap().capped;
    assert_eq!(capped, 1);
    run(&mut r, seeded, 5, |_| false);
    let p = r.mining().unwrap().production().unwrap();
    assert_eq!(p.capped, 6);
    assert_eq!(r.units.len(), MAX_RANGE_UNITS);
    // A dead unit frees no slot: slots are never reused.
    r.units[0].status.alive = false;
    run(&mut r, seeded, 2, |_| false);
    assert_eq!(r.units.len(), MAX_RANGE_UNITS);
    assert_eq!(r.production_paid(second), Some(cost));
    // Cancelling the waiting unit refunds it whole.
    assert_eq!(
        r.execute_command(Command::Cancel { actor: second }),
        Outcome::Applied
    );
    assert_books(&r, seeded);
    assert_eq!(r.side_store(true).unwrap().0, cost);
}

#[test]
fn reserved_foundations_keep_their_space_and_last_roster_slot() {
    let mut r = range(crowded());
    let first = MAX_RANGE_UNITS - 3;
    let at = r.free_spot(first, RangeUnitKind::Tank, true).unwrap();
    assert!(r
        .create_construction_site(RangeUnitKind::TurretT1, true, at)
        .is_some());
    assert_ne!(r.free_spot(first, RangeUnitKind::Tank, true), Some(at));
    assert!(r
        .place_sandbox_unit(RangeUnitKind::Tank, true, at, 0)
        .is_err());
    assert_eq!(r.occupied_unit_slots(), MAX_RANGE_UNITS);
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    seed(&mut r, true, cost);
    assert!(r
        .execute_command(Command::Produce {
            actor: first,
            kind: RangeUnitKind::Tank
        })
        .accepted());
    for _ in 0..BUILD {
        r.production_phase();
    }
    assert_eq!(r.units.len(), MAX_RANGE_UNITS - 1);
    assert_eq!(r.production_paid(first), Some(cost));
    assert!(r.mining().unwrap().production().unwrap().capped > 0);
    let copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert_eq!(r.state_hash(), copy.state_hash());
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn every_sandbox_kind_after_production_keeps_its_side_pose_and_snapshot() {
    let mut l = layout();
    // Exercise the same post-production path one kind at a time. A map-wide
    // dense occupancy and its restored copy add memory, not test coverage.
    l.size = [2048, 1024, 2048];
    l.clear_min = [1, 1];
    l.clear_max = [2047, 2047];
    l.units = vec![base(true, [400, 16, 400])];
    l.blocks = vec![
        RangeBlock {
            min: [0; 3],
            max: [2048, 1024, 2048],
            material: 0,
        },
        RangeBlock {
            min: [0; 3],
            max: [2048, 16, 2048],
            material: 1,
        },
    ];
    for (i, kind) in RangeUnitKind::ALL.into_iter().enumerate() {
        let mut r = range(l.clone());
        let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
        seed(&mut r, true, cost);
        assert!(r
            .execute_command(Command::Produce {
                actor: 0,
                kind: RangeUnitKind::Tank
            })
            .accepted());
        for _ in 0..BUILD {
            r.production_phase();
        }
        assert_eq!(r.spawned().len(), 1);
        let tick = r.tick;
        let at = [1300, 16, 1300];
        let allied = i % 2 == 0;
        let id = r
            .place_sandbox_unit(kind, allied, at, (i % 4) as u8)
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        assert_eq!(r.units[id].kind, kind);
        assert_eq!(r.allied(id), allied);
        assert_eq!(r.tick, tick);
        assert!(r.matter.economy.ledger.balanced());
        let mut section = ("", 0, None);
        let copy =
            BlastRange::restore_observed(r.config, &r.snapshot(), |a, b, c| section = (a, b, c))
                .unwrap_or_else(|| panic!("{kind:?}: sandbox decoder stopped at {section:?}"));
        assert_eq!(r.state_hash(), copy.state_hash(), "{kind:?}");
    }
}

#[test]
fn a_wave_alive_while_a_unit_spawns_ignores_it_until_the_next_one() {
    let mut r = range(layout());
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    seed(&mut r, true, cost);
    assert_eq!(
        r.execute_command(Command::Produce {
            actor: BASE,
            kind: RangeUnitKind::Tank,
        }),
        Outcome::Applied
    );
    // A slow wave beside the base: its grid captures over many ticks, so
    // it is alive when the tank comes out.
    let units = r.units.len();
    r.battle.set_test_capture(400);
    r.battle.push_test_wave(&r.units, [600, 20, 330]);
    assert!(run(&mut r, cost, BUILD as usize, |r| r.units.len() > units));
    assert_eq!(r.battle.wave_rosters(r.units.len()), vec![units]);
    // A restore here: the saved wave knows fewer units than the roster.
    let bytes = r.snapshot();
    assert_eq!(version(&bytes), 65);
    let mut b = BlastRange::restore(config(), &bytes).expect("restore under a wave");
    // The capture budget is the test's, not the snapshot's.
    b.battle.set_test_capture(400);
    assert_eq!(b.snapshot(), bytes);
    for tick in 0..60 {
        r.step();
        b.step();
        assert_eq!(r.state_hash(), b.state_hash(), "tick {tick}");
        if !r.battle.wave_rosters(r.units.len()).is_empty() {
            assert_eq!(r.battle.wave_rosters(r.units.len()), vec![units]);
        }
    }
    // A wave that begins after the tank knows it.
    r.battle.push_test_wave(&r.units, [600, 20, 330]);
    assert_eq!(
        *r.battle.wave_rosters(r.units.len()).last().unwrap(),
        units + 1
    );
    r.step();
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn a_produced_unit_takes_orders_and_is_in_the_hash() {
    let mut l = layout();
    l.blocks.push(RangeBlock {
        min: [266, 1, 316],
        max: [296, 15, 456],
        material: 6,
    });
    let mut r = range(l);
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    seed(&mut r, true, cost);
    assert_eq!(
        r.execute_command(Command::Produce {
            actor: BASE,
            kind: RangeUnitKind::Tank,
        }),
        Outcome::Applied
    );
    let id = r.units.len();
    assert!(run(&mut r, cost, BUILD as usize, |r| r.units.len() > id));
    // It moves where it is ordered.
    let from = r.units[id].motion.pos.map(|v| v.to_int_trunc());
    let hash = r.state_hash();
    assert!(r.battle_order(id, Order::Move, Some([from[0], from[2] - 70])));
    r.step();
    assert_ne!(r.state_hash(), hash);
    for _ in 0..400 {
        r.step();
        if r.units[id].motion.destination.is_none() {
            break;
        }
    }
    let at = r.units[id].motion.pos.map(|v| v.to_int_trunc());
    assert!(at[2] < from[2] - 30, "{from:?} -> {at:?}");
    // It digs into its own tank.
    assert!(r.battle_order(id, Order::Dig(FACE), None));
    for _ in 0..600 {
        r.step();
        if r.tank_held(id).unwrap() > 0 {
            break;
        }
    }
    assert!(r.tank_held(id).unwrap() > 0, "the produced digger dug");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn a_restore_mid_production_and_after_a_spawn_continues_bit_for_bit() {
    let mut a = range(layout());
    let cost = a.production_cost(RangeUnitKind::Tank).unwrap();
    let seeded = 3 * cost;
    seed(&mut a, true, seeded);
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
        assert_eq!(
            a.execute_command(Command::Produce { actor: BASE, kind }),
            Outcome::Applied
        );
    }
    for _ in 0..10 {
        a.step();
    }
    // Mid-production, nothing spawned yet.
    let bytes = a.snapshot();
    assert_eq!(version(&bytes), 65);
    let mut b = BlastRange::restore(config(), &bytes).expect("restore mid-production");
    assert_eq!(b.snapshot(), bytes);
    let units = a.units.len();
    for tick in 0..(2 * BUILD + 5) {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash(), "tick {tick}");
        assert_books(&b, seeded);
    }
    assert_eq!(b.units.len(), units + 2);
    assert!(b.production_lines().is_empty());
    // After both spawned: the produced units restore with their slots.
    let bytes = a.snapshot();
    let mut c = BlastRange::restore(config(), &bytes).expect("restore after spawns");
    assert_eq!(c.snapshot(), bytes);
    assert_eq!(c.spawned(), a.spawned());
    let id = units + 1;
    assert_eq!(c.units[id].kind, RangeUnitKind::Strider);
    for r in [&mut a, &mut c] {
        assert!(r.battle_order(id, Order::Move, Some([400, 300])));
    }
    for tick in 0..40 {
        a.step();
        c.step();
        assert_eq!(a.state_hash(), c.state_hash(), "tick {tick}");
    }
    // Forged: a produced body that is another unit's, a produced count
    // that is not the roster's, a blank aliasing a body.
    let mut f = BlastRange::restore(config(), &bytes).unwrap();
    f.spawned[0].body = f.matter.body_of(0).unwrap();
    assert!(BlastRange::restore(config(), &f.snapshot()).is_none());
    let mut f = BlastRange::restore(config(), &bytes).unwrap();
    f.production_mut().produced += 1;
    assert!(BlastRange::restore(config(), &f.snapshot()).is_none());
    let mut f = BlastRange::restore(config(), &bytes).unwrap();
    assert_eq!(
        f.execute_command(Command::Produce {
            actor: BASE,
            kind: RangeUnitKind::Tank,
        }),
        Outcome::Applied
    );
    f.production_mut().lines[0].blank = f.matter.body_of(1).unwrap();
    assert!(BlastRange::restore(config(), &f.snapshot()).is_none());
}

#[test]
fn a_dead_producers_blank_breaches_and_is_reclaimed_by_digging() {
    let mut l = layout();
    let e = l.economy.as_mut().unwrap();
    e.reclaim = RangeEconomy::stock().reclaim;
    e.reclaim.as_mut().unwrap().reach = 60;
    // Room for the whole blank: its dock is the dead base.
    e.kind_mut(RangeUnitKind::Tank).unwrap().tank = 1 << 32;
    let mut r = range(l);
    let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
    seed(&mut r, true, cost);
    assert_eq!(
        r.execute_command(Command::Produce {
            actor: BASE,
            kind: RangeUnitKind::Tank,
        }),
        Outcome::Applied
    );
    for _ in 0..10 {
        r.step();
    }
    let paid = r.production_paid(BASE).unwrap();
    r.units[BASE].status.alive = false;
    r.step();
    // A quarter of the blank is lost; the rest lies with the hull.
    let lost = paid / 4;
    let p = r.mining().unwrap().production().unwrap();
    assert_eq!(p.lost, lost);
    assert!(r.production_lines().is_empty());
    let wrecks = p.wrecks();
    assert_eq!(wrecks.len(), 1);
    let (slot, blank) = wrecks[0];
    assert_eq!(slot, BASE);
    let l = &r.matter.economy.ledger;
    assert_eq!(l.held(blank), paid - lost);
    assert_eq!(l.sunk(Sink::ContainerBreach), lost);
    // The snapshot keeps the wreck. The test killed the base by its flag,
    // so a restore derives it alive from its intact body, and a wreck at a
    // living producer is refused.
    let bytes = r.snapshot();
    assert_eq!(version(&bytes), 65);
    let mut stage = "";
    let b = BlastRange::restore_observed(config(), &bytes, |s, _, _| stage = s);
    assert!(b.is_none());
    assert_eq!(stage, "production");
    // A digger at the hull takes it.
    let hull = r.units[BASE].motion.pos.map(|v| v.to_int_trunc());
    assert!(r.battle_order(0, Order::Dig([hull[0], hull[1] + 4, hull[2]]), None));
    // The blank grows with the actual tank template; allow enough bounded
    // edit ticks for all of it, plus time to approach the wreck.
    let work_ticks = (paid - lost).div_ceil(16 * 260) as usize;
    for _ in 0..work_ticks + 1500 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
        if r.matter.economy.ledger.held(blank) == 0 {
            break;
        }
    }
    assert_eq!(
        r.matter.economy.ledger.held(blank),
        0,
        "pos={:?} destination={:?} blocked={} tank={:?} hauling={} order={:?}",
        r.units[0].motion.pos.map(|v| v.to_int_trunc()),
        r.units[0].motion.destination,
        r.units[0].motion.blocked,
        r.tank_held(0),
        r.hauling(0),
        r.battle.actors[0].order
    );
    assert_eq!(r.tank_held(0), Some(paid - lost));
}

#[test]
fn the_rules_ai_treats_a_produced_unit_like_an_authored_one() {
    // An enemy base produces a tank; the same tank authored at the same
    // spot, in the same slot, gets the same orders from the rules AI.
    let mut l = layout();
    l.units.push(base(false, [520, 16, 600]));
    let mut a = BlastRange::with_layout(config(), l.clone()).unwrap();
    let cost = a.production_cost(RangeUnitKind::Tank).unwrap();
    seed(&mut a, false, cost);
    assert_eq!(
        a.enqueue_product(SECOND_BASE, RangeUnitKind::Tank, false),
        Outcome::Applied
    );
    let id = a.units.len();
    for _ in 0..BUILD {
        a.step();
    }
    assert_eq!(a.units.len(), id + 1);
    assert!(!a.allied(id));
    let mut authored = l.clone();
    authored.units.push(a.spawned()[0].def.clone());
    let mut b = BlastRange::with_layout(config(), authored).unwrap();
    for _ in 0..BUILD {
        b.step();
    }
    a.start_battle().unwrap();
    b.start_battle().unwrap();
    // The AI observes it as it observes the authored one.
    let observed = |r: &BlastRange| {
        let o = r.enemy_ai_observation().unwrap();
        o.units
            .into_iter()
            .map(|u| {
                (
                    u.id, u.allied, u.alive, u.armed, u.mobile, u.kind, u.position,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(observed(&a), observed(&b));
    // The first decision orders both the same.
    let mut decided = false;
    for _ in 0..60 {
        a.step();
        b.step();
        assert_eq!(a.battle.actors[id].order, b.battle.actors[id].order);
        if a.battle.actors[id].order != Order::Auto {
            decided = true;
            break;
        }
    }
    assert!(decided, "the rules AI ordered the produced tank");
}

#[test]
fn a_paid_blocked_product_sees_an_opening_in_the_same_tick_after_restore() {
    use super::super::super::RangeUnit;
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
        for allied in [true, false] {
            let at = [384, 16, 384];
            let mut l = layout();
            l.units = vec![base(allied, at)];
            // The first free spot is fixed before adding any obstruction.
            let clear = range(l.clone());
            let chosen = clear.free_spot(0, kind, allied).unwrap();
            drop(clear);
            let (lo, hi) = RangeUnit::of_kind(RangeUnitKind::Base, at).bounds();
            let (x0, x1, z0, z1) = (lo[0] - 1, hi[0] + 1, lo[2] - 1, hi[2] + 1);
            // A supported one-cell layer catches tracks/feet at every
            // candidate, but leaves the undamaged producer's box clear.
            for (min, max) in [
                ([64, 16, 64], [x0, 17, 704]),
                ([x1, 16, 64], [704, 17, 704]),
                ([x0, 16, 64], [x1, 17, z0]),
                ([x0, 16, z1], [x1, 17, 704]),
            ] {
                l.blocks.push(RangeBlock {
                    min,
                    max,
                    material: 6,
                });
            }
            let mut a = range(l);
            let cost = a.production_cost(kind).unwrap();
            seed(&mut a, allied, cost);
            assert_eq!(a.enqueue_product(0, kind, allied), Outcome::Applied);
            run(&mut a, cost, BUILD as usize + 2, |_| false);
            assert_eq!(a.units.len(), 1, "{kind:?} allied={allied}");
            assert_eq!(a.production_paid(0), Some(cost));
            assert_eq!(a.production_mut().blocked, 3);
            let bytes = a.snapshot();
            let mut b = BlastRange::restore(config(), &bytes).expect("blocked product restores");
            assert_eq!(b.snapshot(), bytes);
            for _ in 0..3 {
                a.step();
                b.step();
                assert_eq!(a.state_hash(), b.state_hash());
                assert_books(&a, cost);
                assert_books(&b, cost);
            }
            assert_eq!(a.production_mut().blocked, 6);
            // Clear only the actual feet/tracks at the formerly first spot.
            // Both the restored and uninterrupted line must see this drain
            // immediately, without a stale cached "blocked" result.
            let probe = RangeUnit::placed(kind, allied, chosen, true);
            let (plo, phi) = probe.bounds();
            let mut opened = 0;
            for z in plo[2]..phi[2] {
                for x in plo[0]..phi[0] {
                    if probe.solid([x, 16, z]) && a.world.solid(x as u32, 16, z as u32) {
                        a.world.queue_dig(x as u32, 16, z as u32);
                        b.world.queue_dig(x as u32, 16, z as u32);
                        opened += 1;
                    }
                }
            }
            assert!(opened > 0 && opened <= a.world.max_edits_per_tick);
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            assert_eq!(a.units.len(), 2, "{kind:?} allied={allied}");
            assert_eq!(a.spawned()[0].def.at, chosen);
            assert_eq!(a.spawned()[0].def.allied, allied);
            assert_eq!(a.production_queue(0), None);
            assert_eq!(a.production_mut().blocked, 6);
            assert_books(&a, cost);
            assert_books(&b, cost);
            let bytes = a.snapshot();
            assert_eq!(
                BlastRange::restore(config(), &bytes).unwrap().snapshot(),
                bytes
            );
        }
    }
}
