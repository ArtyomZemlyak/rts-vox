//! Approaching traffic must resolve locally, without waiting for a new route.
use super::*;
use modding::range::RangeUnitKind;

struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        u16::from(y < 16)
    }
}

#[test]
fn gently_converging_stopped_walkers_do_not_wait_for_each_other() {
    let body = voxel::unit::commander();
    let mut a = Motion::new([503, 16, 717], RangeUnitKind::Commander, &body);
    let mut b = Motion::new([502, 16, 858], RangeUnitKind::Commander, &body);
    for (m, goal) in [(&mut a, [1088, 768]), (&mut b, [1312, 768])] {
        m.enabled = true;
        m.destination = Some(goal);
        m.path = vec![goal];
        m.drive.head = angle::atan2(i64::from(goal[1] - m.pos[2].to_int_trunc()),
            i64::from(goal[0] - m.pos[0].to_int_trunc()));
    }
    for group in [[&a, &b], [&b, &a]] {
        let traffic: Vec<_> = group.iter().map(|m| Traffic::of(m, &body, true)).collect();
        let speeds: Vec<_> = group.iter().enumerate().map(|(id, m)|
            m.traffic_hint(&body, id, &traffic, crate::mobility::Mobility::intact(true)).unwrap().speed).collect();
        assert!(speeds.iter().any(|s| *s > Fx::ZERO), "mutual following: {speeds:?}");
        assert!(speeds.iter().any(|s| *s == Fx::ZERO), "converging hulls lost yielding: {speeds:?}");
    }
}

#[test]
fn opposing_units_pass_without_search_or_contact_stops() {
    let w = World::new(1536, 192, 1536, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    for kind in [
        RangeUnitKind::Tank,
        RangeUnitKind::Commander,
        RangeUnitKind::Crawler,
        RangeUnitKind::Strider,
        RangeUnitKind::Spider,
        RangeUnitKind::Walker04,
        RangeUnitKind::Artillery,
        RangeUnitKind::ArtilleryT2,
        RangeUnitKind::LightningTank,
        RangeUnitKind::EidolonTank,
    ] {
        let a = super::super::RangeUnit::of_kind(kind, [360, 16, 700]);
        let b = super::super::RangeUnit::of_kind(kind, [1160, 16, 700]);
        let body = a.body;
        let mut group = [a.motion, b.motion];
        group[1].drive.head = angle::FULL / 2;
        for (m, goal) in group.iter_mut().zip([[1160, 700], [360, 700]]) {
            m.queue_move(&w, goal);
            m.path = vec![goal];
        }
        let mut blocked = 0;
        let mut finish = None;
        for tick in 0..1400 {
            let traffic: Vec<_> = group.iter().map(|m| Traffic::of(m, &body, true)).collect();
            let hints: Vec<_> = group
                .iter()
                .enumerate()
                .map(|(id, m)| {
                    m.traffic_hint(&body, id, &traffic, crate::mobility::Mobility::intact(true))
                })
                .collect();
            for id in 0..2 {
                let others = [Obstacle::of(&group[1 - id], &body)];
                let m = &mut group[id];
                m.set_traffic_hint(hints[id]);
                m.allow_start(false);
                m.step(
                    &w,
                    &body,
                    true,
                    true,
                    crate::mobility::Mobility::intact(true),
                    &others,
                    tick,
                    98,
                    &nav::vehicle::Rubble::new(),
                    &Default::default(),
                );
                blocked += u32::from(m.blocked);
                assert!(m.drive.vel.abs() <= Fx::from_int(m.speed));
            }
            assert!(
                Obstacle::of(&group[0], &body).penetration(&Obstacle::of(&group[1], &body))
                    <= HULL_CONTACT_SLOP
            );
            if group.iter().all(|m| m.destination.is_none()) {
                finish = Some(tick);
                break;
            }
        }
        eprintln!(
            "opposing {kind:?}: finish {finish:?}, refused {blocked}, positions {:?}, half {:?}",
            group.iter().map(|m| m.pos).collect::<Vec<_>>(),
            Obstacle::of(&group[0], &body).half
        );
        assert!(
            finish.is_some(),
            "{kind:?}: approaching units remained locked"
        );
        assert!(blocked <= 4, "{kind:?}: {blocked} contact refusals");
    }
}

fn pair_step(w: &World, pair: &mut [(Motion, UnitBody); 2], tick: u32, walls: &[Obstacle]) -> u32 {
    let traffic: Vec<_> = pair.iter().map(|(m, b)| Traffic::of(m, b, true)).collect();
    let hints: Vec<_> = pair
        .iter()
        .enumerate()
        .map(|(id, (m, b))| {
            m.traffic_hint(b, id, &traffic, crate::mobility::Mobility::intact(true))
        })
        .collect();
    let mut refused = 0;
    for id in 0..2 {
        let mut others = vec![Obstacle::of(&pair[1 - id].0, &pair[1 - id].1)];
        others.extend_from_slice(walls);
        let (m, b) = &mut pair[id];
        m.set_traffic_hint(hints[id]);
        m.allow_start(false);
        m.step(
            w,
            b,
            true,
            true,
            crate::mobility::Mobility::intact(true),
            &others,
            tick,
            98,
            &nav::vehicle::Rubble::new(),
            &Default::default(),
        );
        refused += u32::from(m.blocked);
        assert!(m.drive.vel.abs() <= Fx::from_int(m.speed));
        for o in &others {
            assert!(Obstacle::of(m, b).penetration(o) <= HULL_CONTACT_SLOP);
        }
    }
    refused
}

fn saved(m: &Motion) -> Vec<u8> {
    let mut b = Blob::new();
    m.save(&mut b);
    b.v
}

#[test]
fn mixed_and_rotated_passing_continues_identically_after_restore() {
    let w = World::new(1536, 192, 1536, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    for (kinds, starts, head) in [
        (
            [RangeUnitKind::Tank, RangeUnitKind::Commander],
            [[760, 360], [760, 1160]],
            angle::QUARTER,
        ),
        (
            [RangeUnitKind::Crawler, RangeUnitKind::Tank],
            [[350, 350], [1180, 1180]],
            angle::FULL / 8,
        ),
        (
            [RangeUnitKind::Commander, RangeUnitKind::Walker04],
            [[360, 700], [1160, 700]],
            0,
        ),
    ] {
        let mut pair = std::array::from_fn(|i| {
            let u = super::super::RangeUnit::of_kind(kinds[i], [starts[i][0], 16, starts[i][1]]);
            (u.motion, u.body)
        });
        for i in 0..2 {
            pair[i].0.drive.head = angle::norm(head + i as i32 * angle::HALF);
            pair[i].0.queue_move(&w, starts[1 - i]);
            pair[i].0.path = vec![starts[1 - i]];
        }
        let mut copy = None;
        let mut refused = 0;
        for tick in 0..1400 {
            let contact = pair_step(&w, &mut pair, tick, &[]);
            refused += contact;
            if let Some(replica) = copy.as_mut() {
                pair_step(&w, replica, tick, &[]);
                for i in 0..2 {
                    assert_eq!(
                        saved(&pair[i].0),
                        saved(&replica[i].0),
                        "{kinds:?}: tick {tick}"
                    );
                }
            } else if tick == 180 {
                let mut replica = pair.clone();
                for i in 0..2 {
                    assert!(replica[i].0.load(&mut Scan::new(&saved(&pair[i].0))));
                }
                copy = Some(replica);
            }
            if pair.iter().all(|(m, _)| m.destination.is_none()) {
                break;
            }
        }
        eprintln!(
            "mixed {kinds:?}: refused {refused}, positions {:?}",
            pair.iter().map(|(m, _)| m.pos).collect::<Vec<_>>()
        );
        assert!(
            copy.is_some() && pair.iter().all(|(m, _)| m.destination.is_none()),
            "{kinds:?}: did not finish"
        );
        assert!(refused <= 4, "{kinds:?}: {refused} contact refusals");
    }
}

#[test]
fn local_passing_cannot_enter_a_wall_or_use_an_elevated_neighbour() {
    let w = World::new(1536, 192, 1536, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    let mut pair = std::array::from_fn(|i| {
        let u =
            super::super::RangeUnit::of_kind(RangeUnitKind::Tank, [600 + i as i32 * 120, 16, 700]);
        (u.motion, u.body)
    });
    for i in 0..2 {
        pair[i].0.drive.head = i as i32 * angle::HALF;
        let goal = [1000 - i as i32 * 600, 700];
        pair[i].0.queue_move(&w, goal);
        pair[i].0.path = vec![goal];
    }
    let traffic: Vec<_> = pair.iter().map(|(m, b)| Traffic::of(m, b, true)).collect();
    let hint = pair[0]
        .0
        .traffic_hint(
            &pair[0].1,
            0,
            &traffic,
            crate::mobility::Mobility::intact(true),
        )
        .unwrap();
    assert!(hint.way.is_some(), "fixture missed approaching traffic");
    let mut air = traffic;
    air[1].obstacle.pos[1] = Fx::from_int(500);
    assert!(pair[0]
        .0
        .traffic_hint(&pair[0].1, 0, &air, crate::mobility::Mobility::intact(true))
        .unwrap()
        .way
        .is_none());
    let wall = Obstacle {
        moving: false,
        height: Fx::ZERO,
        head: 0,
        pos: [Fx::from_int(760), Fx::from_int(16), Fx::from_int(655)],
        half: [Fx::from_int(400), Fx::from_int(20)],
    };
    for tick in 0..160 {
        pair_step(&w, &mut pair, tick, &[wall]);
    }
}

#[test]
fn queued_orders_share_traffic_before_the_route_search_is_admitted() {
    let w = World::new(1536, 192, 1536, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    let mut pair = std::array::from_fn(|i| {
        let u =
            super::super::RangeUnit::of_kind(RangeUnitKind::Tank, [360 + i as i32 * 800, 16, 700]);
        (u.motion, u.body)
    });
    for i in 0..2 {
        pair[i].0.drive.head = i as i32 * angle::HALF;
        pair[i].0.queue_move(&w, [1160 - i as i32 * 800, 700]);
        assert!(pair[i].0.path.is_empty());
    }
    let mut refused = 0;
    for tick in 0..700 {
        refused += pair_step(&w, &mut pair, tick, &[]);
        assert!(pair.iter().all(|(m, _)| !m.started_search()));
        if pair.iter().all(|(m, _)| m.destination.is_none()) {
            break;
        }
    }
    assert!(
        pair.iter().all(|(m, _)| m.destination.is_none()),
        "queued traffic waited for routing"
    );
    assert_eq!(refused, 0);
}
