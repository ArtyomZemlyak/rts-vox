//! Whole-map searches walked over ticks under the range's shared work
//! allowance (ADR-680).
use super::*;
use modding::range::{RangeBlock, RangeUnitDef};

/// Five allied tanks on the west of a steel wall with one gap, an enemy
/// that holds far to the east, and no rules AI: every route is an order.
fn walled() -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.size = [1024, 192, 768];
    l.shields.clear();
    l.course = None;
    l.clear_min = [64, 64];
    l.clear_max = [960, 704];
    l.blocks = vec![RangeBlock {
        min: [480, 0, 0],
        max: [496, 60, 520],
        material: 3,
    }];
    l.units = (0..5)
        .map(|k| RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [150, 16, 120 + 90 * k],
            allied: true,
        })
        .chain(std::iter::once(RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [900, 16, 680],
            allied: false,
        }))
        .collect();
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        l,
    )
    .unwrap();
    short_reach(&mut r);
    r.start_battle().unwrap();
    r.battle.actors[5].order = Order::Hold;
    hold_ai(&mut r);
    r
}
/// Nobody reaches anybody: the battle outlasts every search. Tuning is
/// data, not snapshot state: a restored range is given it again.
fn short_reach(r: &mut BlastRange) {
    let tuning = std::sync::Arc::make_mut(&mut r.battle.tuning);
    tuning.tank_range = 100;
    tuning.kinetic_range = 100;
}
/// Keep the rules AI from deciding: a snapshot accepts only a decision
/// tick within its period.
fn hold_ai(r: &mut BlastRange) {
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
}
fn step(r: &mut BlastRange) {
    hold_ai(r);
    r.step();
}
fn goal(k: usize) -> [i32; 2] {
    [880, 100 + 60 * k as i32]
}
/// A small allowance: every route here takes several ticks.
fn starve(r: &mut BlastRange) {
    r.navigation.global_work_per_tick = 8192;
    r.navigation.global_work_per_search = 2048;
}
fn order_all(r: &mut BlastRange) {
    for k in 0..5 {
        assert!(r.battle_order(k, Order::Move, Some(goal(k))));
    }
}
/// The route one call of the planner gives each tank where it stands, and
/// the work of that call.
fn one_call(r: &BlastRange) -> Vec<(Vec<[i32; 2]>, u64)> {
    let rubble = r.current_rubble();
    (0..5)
        .map(|k| {
            let m = &r.units[k].motion;
            let mut search = nav::vehicle::coarse::Search::new(
                [r.world.nx, r.world.nz],
                m.probe_shape(&r.units[k].body),
                [m.pos[0], m.pos[2]],
                goal(k),
                m.coarse_config(),
            )
            .unwrap();
            let (answer, work) = search.advance(&r.world, &rubble, u64::MAX);
            (answer.unwrap().expect("a route around the wall").path, work)
        })
        .collect()
}

/// **Orders are answered over ticks, with the routes of one call**, and no
/// tick spends more whole-map work than the allowance. Every tank gets
/// its route — none starves behind the others — and the searches that
/// waited longest are served first, ties by slot, and what one search
/// leaves goes to the next: the last answer comes about when the summed
/// work of all five allows. The order itself does no work: the range
/// walks it from the next tick.
#[test]
fn budgeted_orders_get_the_routes_of_one_call_without_starving() {
    let mut r = walled();
    starve(&mut r);
    let expected = one_call(&r);
    assert!(
        expected.iter().any(|(p, _)| p.len() > 1),
        "the wall must shape a route: {expected:?}"
    );
    let total: u64 = expected.iter().map(|(_, w)| w).sum();
    let work = r.coarse_work;
    order_all(&mut r);
    assert_eq!(r.coarse_work, work, "an order must not search");
    assert!((0..5).all(|k| r.units[k].motion.coarse_waited() == Some(0)));
    let mut answered = [None; 5];
    for tick in 1..=400u32 {
        let before = r.coarse_work;
        step(&mut r);
        assert!(
            r.coarse_work - before <= r.navigation.global_work_per_tick,
            "tick {tick} spent {}",
            r.coarse_work - before
        );
        for k in 0..5 {
            if answered[k].is_none() && r.units[k].motion.coarse_waited().is_none() {
                answered[k] = Some(tick);
                let m = &r.units[k].motion;
                assert_eq!(
                    m.path,
                    expected[k].0,
                    "tank {k} tick {tick} phase={:?} order={:?} hunting={} last_plan={:?} dest={:?} blocked={} pos={:?}",
                    r.battle.phase,
                    r.battle.actors[k].order,
                    m.searching(),
                    m.last_plan,
                    m.destination,
                    m.blocked,
                    m.pos
                );
            }
        }
        if answered.iter().all(Option::is_some) {
            break;
        }
    }
    let answered = answered.map(|t| t.expect("a tank starved"));
    let last = *answered.iter().max().unwrap();
    assert!(last >= 3, "the fixture must span ticks: {answered:?}");
    // Work conserving: no tick idles while a search waits. One extra tick
    // for the slack a test larger than what is left leaves at a tick end.
    let allowance = r.navigation.global_work_per_tick;
    assert!(
        u64::from(last) <= total.div_ceil(allowance) + 2,
        "{answered:?} for {total} work"
    );
    // An unstarved range gives the same routes at once.
    let mut fast = walled();
    order_all(&mut fast);
    step(&mut fast);
    for (k, (path, _)) in expected.iter().enumerate() {
        assert_eq!(&fast.units[k].motion.path, path, "tank {k}");
    }
}

/// **A new order cancels the search in flight**: its work stops, the new
/// search starts from nothing, and the route is the new order's.
#[test]
fn a_new_order_cancels_the_search_in_flight() {
    let mut r = walled();
    starve(&mut r);
    order_all(&mut r);
    step(&mut r);
    step(&mut r);
    let m = &r.units[0].motion;
    assert!(m.coarse_waited() == Some(2) && m.coarse_work().unwrap() > 0);
    let other = [700, 650];
    assert!(r.battle_order(0, Order::Move, Some(other)));
    let m = &r.units[0].motion;
    assert_eq!(m.coarse_waited(), Some(0));
    assert_eq!(m.coarse_work(), Some(0));
    assert_eq!(m.destination, Some(other));
    for _ in 0..400 {
        step(&mut r);
        if !r.units[0].motion.searching() {
            break;
        }
    }
    assert_eq!(r.units[0].motion.path.last(), Some(&other));
    // A stop drops it as well.
    assert!(r.battle_order(1, Order::Move, Some(goal(1))));
    assert!(r.units[1].motion.searching());
    assert!(r.battle_order(1, Order::Hold, None));
    assert!(r.units[1].motion.coarse_waited().is_none());
}

/// **A range saved mid-search restores to the same run** (ADR-680): the
/// snapshot wraps its format in version 66 only while a search is in
/// flight, reads back to the same bytes, and the restored range hashes
/// the same as the continuing one on every tick, through the answers.
#[test]
fn a_range_restored_mid_search_steps_as_the_continuing_one() {
    let mut r = walled();
    starve(&mut r);
    let plain = r.snapshot();
    assert_ne!(u32::from_le_bytes(plain[..4].try_into().unwrap()), 66);
    order_all(&mut r);
    for _ in 0..3 {
        step(&mut r);
    }
    assert!((0..5).any(|k| r.units[k].motion.coarse_waited().is_some()));
    let bytes = r.snapshot();
    assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 66);
    let mut back = BlastRange::restore(r.config, &bytes).expect("mid-search snapshot");
    assert_eq!(back.snapshot(), bytes);
    // The allowance and the reach are data: the restored range is given
    // the same.
    short_reach(&mut back);
    back.navigation.global_work_per_tick = r.navigation.global_work_per_tick;
    back.navigation.global_work_per_search = r.navigation.global_work_per_search;
    let mut searching = true;
    for tick in 0..120 {
        step(&mut r);
        step(&mut back);
        assert_eq!(
            r.state_hash(),
            back.state_hash(),
            "tick {tick} after restore"
        );
        let now = (0..5).any(|k| r.units[k].motion.coarse_waited().is_some());
        if searching && !now {
            // The last answer: the snapshot falls back to its plain version.
            assert_ne!(
                u32::from_le_bytes(r.snapshot()[..4].try_into().unwrap()),
                65
            );
        }
        searching = now;
    }
    assert!(!searching, "searches never ended");
    assert!(
        (0..5).all(|k| r.units[k].at[0] > 150),
        "the tanks never set out"
    );
    // A version-66 header with no search in it is not a snapshot.
    let mut forged = plain.clone();
    forged.splice(0..0, 66u32.to_le_bytes());
    forged.extend(0u32.to_le_bytes());
    assert!(BlastRange::restore(r.config, &forged).is_none());
}

/// **A search a tick can't finish holds the order, not the hull**: a hull
/// already on a route keeps driving it while its new search walks.
#[test]
fn a_hull_on_a_route_drives_on_while_its_new_search_walks() {
    let mut r = walled();
    order_all(&mut r);
    for _ in 0..30 {
        step(&mut r);
    }
    let m = &r.units[0].motion;
    assert!(!m.path.is_empty() && m.cursor < m.path.len());
    starve(&mut r);
    // A pursuit retarget keeps the current route while the search runs
    // (ADR-462); the orders phase gives it no work (ADR-680).
    let others: Vec<_> = (1..r.units.len())
        .map(|i| motion::Obstacle::of(&r.units[i].motion, &r.units[i].body))
        .collect();
    let rubble = r.current_rubble();
    let u = &mut r.units[0];
    u.motion.allow_coarse(Some(0));
    assert!(u
        .motion
        .retarget(&r.world, [880, 600], &u.body, &others, &rubble));
    u.motion.allow_coarse(None);
    assert!(u.motion.coarse_waited() == Some(0) && !u.motion.path.is_empty());
    let at = u.motion.pos;
    step(&mut r);
    assert_ne!(r.units[0].motion.pos, at, "the hull stopped for its search");
}
