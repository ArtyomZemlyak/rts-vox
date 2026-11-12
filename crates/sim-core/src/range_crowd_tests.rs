use super::*;
use modding::range::{RangeBlock, RangeUnitDef};

fn arena(count: usize, gate: bool) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let side = if count > 10 { 4096 } else { 1600 };
    let cols = if count > 10 { 8 } else { 4 };
    l.size = [side, 128, side];
    l.battle = true;
    l.clear_min = [0, 0];
    l.clear_max = [side as i32; 2];
    l.blocks.clear();
    let c = l.course.as_mut().unwrap();
    c.min = [40, 40];
    c.max = [side as i32 - 40; 2];
    c.ramps.clear();
    c.patrols.clear();
    l.units = (0..count)
        .map(|id| RangeUnitDef {
            allied: true,
            kind: RangeUnitKind::Tank,
            at: [
                260 + (id / cols) as i32 * 90,
                16,
                550 + (id % cols) as i32 * 90,
            ],
            weapon: None,
        })
        .collect();
    if gate {
        l.blocks = vec![
            RangeBlock {
                min: [760, 16, 0],
                max: [800, 64, 640],
                material: 2,
            },
            RangeBlock {
                min: [760, 16, 760],
                max: [800, 64, 1600],
                material: 2,
            },
        ];
    }
    let mut s = BlastRange::with_layout(config(), l).unwrap();
    s.start_free_control().unwrap();
    s
}
fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
}

#[test]
fn opposing_traffic_finishes_and_replays_across_navigation_workers() {
    let mut scene = arena(8, false);
    scene.set_navigation_workers(1);
    scene.set_physics_workers(1);
    for id in 0..8 {
        let from = [if id % 2 == 0 {360} else {1160}, 16, 400+(id/2) as i32*100];
        scene.units[id].motion.pos = from.map(Fx::from_int);
        scene.units[id].motion.drive.head = if id % 2 == 0 {0} else {fx::angle::HALF};
        let goal = [1520-from[0],from[2]];
        assert!(scene.units[id].motion.queue_move(&scene.world,goal));
    }
    let mut replica = None;
    let mut refused = 0;
    for tick in 0..900 {
        scene.step();
        assert_separate(&scene);
        refused += scene.units.iter().filter(|u|u.motion.blocked).count();
        if let Some(other) = replica.as_mut() {
            let other: &mut BlastRange = other;
            other.step();
            assert_eq!(scene.state_hash(),other.state_hash(),"traffic diverged at {tick}");
        } else if tick == 100 {
            let mut copy = BlastRange::restore(config(),&scene.snapshot()).unwrap();
            copy.set_navigation_workers(2); copy.set_physics_workers(1); replica = Some(copy);
        }
        if scene.units.iter().all(|u|u.motion.destination.is_none()) { break; }
    }
    assert!(replica.is_some() && scene.units.iter().all(|u|u.motion.destination.is_none()));
    assert_eq!(refused,0);
    assert_eq!(scene.snapshot(),replica.unwrap().snapshot());
}
fn assert_separate(s: &BlastRange) {
    for a in 0..s.units.len() {
        for b in a + 1..s.units.len() {
            let ua = &s.units[a];
            let ub = &s.units[b];
            let depth = motion::Obstacle::of(&ua.motion, &ua.body)
                .penetration(&motion::Obstacle::of(&ub.motion, &ub.body));
            assert!(
                depth <= Fx::from_int(4),
                "hull overlap {depth:?} exceeds 40 cm: {a}/{b} at {}",
                s.tick
            );
        }
    }
}
#[test]
fn small_group_advances_without_deep_hull_overlap() {
    let mut s = arena(3, false);
    s.set_navigation_workers(1);
    s.set_physics_workers(1);
    let destinations = s.battle_formation_destinations(&[0, 1, 2], [800, 700]);
    assert_eq!(s.battle_group_move(&destinations, [800, 700]), 3);
    let start: Vec<_> = s.units.iter().map(|u| u.motion.pos[0]).collect();
    let mut replay = None;
    for tick in 0..120 {
        s.step();
        assert_separate(&s);
        for u in &s.units {
            assert!(u.motion.drive.vel.abs() <= Fx::from_int(u.motion.speed));
        }
        if tick == 59 {
            let mut copy =
                BlastRange::restore(config(), &s.snapshot()).expect("group movement snapshot");
            copy.set_navigation_workers(1);
            copy.set_physics_workers(1);
            replay = Some(copy);
        } else if let Some(copy) = replay.as_mut() {
            copy.step();
            assert_eq!(
                s.units
                    .iter()
                    .map(|u| (u.motion.pos, u.motion.drive))
                    .collect::<Vec<_>>(),
                copy.units
                    .iter()
                    .map(|u| (u.motion.pos, u.motion.drive))
                    .collect::<Vec<_>>(),
                "group motion diverged after restoring at tick {tick}"
            );
        }
    }
    assert_eq!(s.state_hash(), replay.unwrap().state_hash());
    for (i, u) in s.units.iter().enumerate() {
        assert!(
            u.motion.pos[0] > start[i].add(Fx::from_int(40)),
            "unit {i} did not advance"
        );
    }
}
#[test]
fn ten_tanks_form_without_contact_and_replay_across_worker_counts() {
    let mut s = arena(10, false);
    s.set_navigation_workers(1);
    let ids: Vec<_> = (0..10).collect();
    let dest = s.battle_formation_destinations(&ids, [1100, 700]);
    assert_eq!(
        dest,
        s.battle_formation_destinations(
            &ids.iter()
                .rev()
                .copied()
                .chain([usize::MAX, 3, 3])
                .collect::<Vec<_>>(),
            [1100, 700]
        )
    );
    assert_eq!(s.battle_group_move(&dest, [1100, 700]), 10);
    for _ in 0..120 {
        s.step();
        assert_separate(&s);
    }
    let bytes = s.snapshot();
    let mut replicas: Vec<_> = [1, 4, 8]
        .into_iter()
        .map(|workers| {
            let mut r = BlastRange::restore(config(), &bytes).unwrap();
            r.set_navigation_workers(workers);
            r.set_physics_workers(workers);
            r
        })
        .collect();
    for tick in 120..900 {
        s.step();
        assert_separate(&s);
        for (id, u) in s.units.iter().enumerate() {
            assert!(!u.motion.blocked, "tank {id} blocked at {tick}");
            assert!(u.motion.drive.vel >= Fx::ZERO);
        }
        for r in &mut replicas {
            r.step();
            assert_eq!(
                s.units
                    .iter()
                    .map(|u| (
                        u.motion.pos,
                        u.motion.drive.head,
                        u.motion.drive.vel,
                        u.motion.destination
                    ))
                    .collect::<Vec<_>>(),
                r.units
                    .iter()
                    .map(|u| (
                        u.motion.pos,
                        u.motion.drive.head,
                        u.motion.drive.vel,
                        u.motion.destination
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }
    assert!(s.units.iter().all(|u| u.motion.destination.is_none()));
    for r in replicas {
        assert_eq!(s.state_hash(), r.state_hash());
        assert_eq!(s.snapshot(), r.snapshot());
    }
    assert!(s.matter.economy.ledger.balanced());
}
#[test]
fn sixty_four_tanks_complete_as_one_group_without_intersections() {
    let mut s = arena(64, false);
    let dest = s.battle_formation_destinations(&(0..64).collect::<Vec<_>>(), [2200, 1800]);
    assert_eq!(s.battle_group_move(&dest, [2200, 1800]), 64);
    let (mut blocked, mut reverse) = (0, 0);
    for _ in 0..2400 {
        s.step();
        assert_separate(&s);
        blocked += s.units.iter().filter(|u| u.motion.blocked).count();
        reverse += s
            .units
            .iter()
            .filter(|u| u.motion.drive.vel < Fx::ZERO)
            .count();
    }
    assert!(s.units.iter().all(|u| u.motion.destination.is_none()));
    assert!(
        blocked == 0 && reverse == 0,
        "crowd recovery inflated: {blocked}/{reverse}"
    );
}
#[test]
fn formation_edge_slots_are_distinct_size_aware_and_bounded() {
    let mut s = arena(10, false);
    s.units[3].kind = RangeUnitKind::TurretT1;
    s.units[4].status.alive = false;
    s.units[5].body = voxel::unit::spider();
    for centre in [[0, 0], [1599, 1599], [800, 800], [i32::MIN, i32::MAX]] {
        let dest = s.battle_formation_destinations(&(0..10).collect::<Vec<_>>(), centre);
        assert_eq!(dest.len(), 8);
        if centre == [800, 800] {
            let small: Vec<_> = dest.iter().filter(|(id, _)| *id != 5).collect();
            assert!(
                small
                    .iter()
                    .enumerate()
                    .any(|(i, (_, a))| small[..i].iter().any(|(_, b)| {
                        let dx = i64::from(a[0] - b[0]);
                        let dz = i64::from(a[1] - b[1]);
                        dx * dx + dz * dz < 240 * 240
                    })),
                "small units should not all reserve the spider's spacing"
            );
        }
        for (i, &(id, p)) in dest.iter().enumerate() {
            assert!(p.iter().all(|&v| (40..1560).contains(&v)));
            assert_ne!(id, 3);
            assert_ne!(id, 4);
            for &(other, q) in &dest[..i] {
                let diameter = |id: usize| {
                    let o = super::motion::Obstacle::of(&s.units[id].motion, &s.units[id].body);
                    let d = o.half[0]
                        .mul(o.half[0])
                        .add(o.half[1].mul(o.half[1]))
                        .sqrt()
                        .mul(fx::Fx::from_int(2))
                        .to_int_trunc()
                        + 1
                        + 2 * s.navigation.cell_units
                        + 2 * s.navigation.plan_margin;
                    (d + 1) & !1
                };
                let separation = i64::from((diameter(id) + diameter(other)) / 2);
                let dx = i64::from(p[0] - q[0]);
                let dz = i64::from(p[1] - q[1]);
                assert!(
                    dx * dx + dz * dz >= separation * separation,
                    "hull slots too close"
                );
            }
        }
    }
    assert!(s.battle_formation_destinations(&[], [800, 800]).is_empty());
    assert!(s
        .battle_formation_destinations(&[usize::MAX], [800, 800])
        .is_empty());
    let mut tiny = arena(64, false);
    tiny.size = [128, 128, 128];
    assert!(tiny
        .battle_formation_destinations(&(0..64).collect::<Vec<_>>(), [64, 64])
        .is_empty());
}
#[test]
fn gate_convoy_remains_collision_free_and_all_orders_finish() {
    let mut s = arena(10, true);
    s.set_navigation_workers(1);
    s.set_physics_workers(1);
    let dest: Vec<_> = s
        .units
        .iter()
        .enumerate()
        .map(|(id, u)| {
            (
                id,
                [
                    u.motion.pos[0].to_int_trunc() + 700,
                    u.motion.pos[2].to_int_trunc(),
                ],
            )
        })
        .collect();
    assert_eq!(s.battle_group_move(&dest, [1100, 700]), 10);
    let mut replica: Option<BlastRange> = None;
    let mut searching = 0;
    let mut finished = None;
    for tick in 0..1200 {
        s.step();
        assert_separate(&s);
        searching += s.units.iter().filter(|u| u.motion.searching()).count();
        for u in &s.units {
            assert!(u.motion.drive.vel.abs() <= Fx::from_int(u.motion.speed));
        }
        if let Some(other) = replica.as_mut() {
            other.step();
            assert_eq!(s.state_hash(), other.state_hash(), "gate replay at {tick}");
        } else if tick == 210 {
            let mut copy = BlastRange::restore(config(), &s.snapshot()).unwrap();
            copy.set_navigation_workers(2);
            copy.set_physics_workers(1);
            replica = Some(copy);
        }
        if s.units.iter().all(|u| u.motion.destination.is_none()) {
            finished = Some(tick + 1);
            break;
        }
    }
    assert!(finished.is_some(), "gate convoy stalled");
    assert!(searching < 1000, "local search churn: {searching}");
    assert_eq!(s.snapshot(), replica.unwrap().snapshot());
    eprintln!("gate convoy finished at {} ticks; searching {searching}", finished.unwrap());
    assert!(s.matter.economy.ledger.balanced());
}

#[test]
fn gate_search_admission_keeps_shared_work_and_immediate_response() {
    for starts in [1, 2] {
        let mut sim = arena(10, true);
        sim.set_navigation_workers(1); sim.set_physics_workers(1);
        sim.navigation.searches_started_per_tick = starts;
        let destinations: Vec<_> = sim.units.iter().enumerate().map(|(id, u)|
            (id, [u.motion.pos[0].to_int_trunc() + 700, u.motion.pos[2].to_int_trunc()])).collect();
        let initial: Vec<_> = sim.units.iter().map(|u| u.motion.pos).collect();
        assert_eq!(sim.battle_group_move(&destinations, [1100, 700]), 10);
        let mut searching = 0;
        let mut finished = None;
        let mut last_initial_route = 0;
        let mut got_route = [false; 10];
        for tick in 0..2000 {
            let work = sim.coarse_work;
            sim.step(); assert_separate(&sim);
            assert!(sim.coarse_work.wrapping_sub(work) <= sim.navigation.global_work_per_tick);
            searching += sim.units.iter().filter(|u| u.motion.searching()).count();
            for (id, u) in sim.units.iter().enumerate() {
                if tick == 3 { assert!(u.motion.pos[0] > initial[id][0], "response {id}"); }
                if !got_route[id] && !u.motion.path.is_empty() {
                    got_route[id] = true; last_initial_route = tick + 1;
                }
            }
            if sim.units.iter().all(|u| u.motion.destination.is_none()) { finished = Some(tick + 1); break; }
        }
        eprintln!("gate starts={starts}: finish={finished:?}, initial_routes={last_initial_route}, searching={searching}");
        assert!(finished.is_some()); assert!(got_route.iter().all(|v| *v));
    }
}

#[test]
fn single_commander_group_orders_reach_nearby_and_distant_points() {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.size = [1024, 256, 1024]; l.battle = true; l.gun_point = true; l.course = None;
    l.clear_min = [1, 1]; l.clear_max = [1023, 1023]; l.blocks.clear(); l.shields.clear();
    l.units = vec![RangeUnitDef { allied: true, kind: RangeUnitKind::Commander, at: [512, 16, 512], weapon: None }];
    let mut r = BlastRange::with_layout(config(), l).unwrap();
    r.set_physics_workers(1); r.set_navigation_workers(1); r.start_free_control().unwrap();
    for delta in [8, 16, 40, 160] {
        let start = r.units[0].motion.pos[0].to_int_trunc();
        let goal = [start + delta, r.units[0].motion.pos[2].to_int_trunc()];
        let slots = r.battle_formation_destinations(&[0], goal);
        assert_eq!(slots, vec![(0, goal)]);
        assert_eq!(r.battle_group_move(&slots, goal), 1);
        for _ in 0..600 { r.step(); if r.units[0].motion.destination.is_none() { break; } }
        let motion = &r.units[0].motion;
        assert!((motion.pos[0].to_int_trunc() - goal[0]).abs() <= motion.arrival + 1,
            "near move {delta} stalled at {:?}, dest {:?}, blocked {}", motion.pos, motion.destination, motion.blocked);
    }
}

#[test]
fn commander_group_order_bypasses_a_factory_across_navigation_workers() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.size = [2048, 384, 2048]; layout.battle = true; layout.gun_point = true; layout.course = None;
    layout.clear_min = [0, 0]; layout.clear_max = [2048, 2048]; layout.blocks.clear(); layout.shields.clear();
    layout.units = vec![
        RangeUnitDef { allied: true, kind: RangeUnitKind::Commander, at: [300, 16, 1024], weapon: None },
        RangeUnitDef { allied: true, kind: RangeUnitKind::KilFactoryT1, at: [850, 16, 1024], weapon: None },
    ];
    let mut serial = BlastRange::with_layout(config(), layout).unwrap();
    serial.set_physics_workers(1); serial.set_navigation_workers(1); serial.start_free_control().unwrap();
    let goal = [1550, 1024];
    let slots = serial.battle_formation_destinations(&[0], goal);
    assert_eq!(serial.battle_group_move(&slots, goal), 1);
    let mut parallel = BlastRange::restore(serial.config, &serial.snapshot()).unwrap();
    parallel.set_physics_workers(1); parallel.set_navigation_workers(2);
    for tick in 0..1800 {
        serial.step(); parallel.step();
        let a = &serial.units[0].motion; let b = &parallel.units[0].motion;
        assert_eq!((a.pos, a.drive, &a.path, a.cursor, a.blocked),
            (b.pos, b.drive, &b.path, b.cursor, b.blocked), "worker divergence at {tick}");
        let own = motion::Obstacle::of(a, &serial.units[0].body);
        let factory = motion::Obstacle::of(&serial.units[1].motion, &serial.units[1].body);
        assert!(own.penetration(&factory) <= Fx::from_int(4), "commander entered factory at {tick}");
        if tick == 3 { assert!(a.pos[0] > Fx::from_int(300), "group order waited for planning"); }
        if a.destination.is_none() { break; }
    }
    assert!(serial.units[0].motion.destination.is_none(), "commander did not bypass factory: {:?}", serial.units[0].motion.pos);
    assert_eq!(serial.state_hash(), parallel.state_hash());
}

#[test]
fn walking_group_advances_without_detouring_around_its_own_moving_members() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.size = [2048, 256, 2048]; layout.battle = true; layout.gun_point = true; layout.course = None;
    layout.clear_min = [0, 0]; layout.clear_max = [2048, 2048]; layout.blocks.clear(); layout.shields.clear();
    layout.units = (0..3).map(|id| RangeUnitDef { allied: true,
        kind: RangeUnitKind::Commander, at: [300, 16, 700 + id * 180], weapon: None }).collect();
    let mut sim = BlastRange::with_layout(config(), layout).unwrap();
    sim.set_physics_workers(1); sim.set_navigation_workers(1); sim.start_free_control().unwrap();
    let goal = [1200, 880];
    let slots = sim.battle_formation_destinations(&[0, 1, 2], goal);
    assert_eq!(sim.battle_group_move(&slots, goal), 3);
    let mut copy = None;
    let mut stopped = [0u32; 3];
    for tick in 0..1800 {
        sim.step(); assert_separate(&sim);
        if let Some(other) = copy.as_mut() {
            let other: &mut BlastRange = other;
            other.step();
            for id in 0..3 {
                let a = &sim.units[id].motion; let b = &other.units[id].motion;
                assert_eq!((a.pos, a.drive, &a.path, a.cursor, a.destination),
                    (b.pos, b.drive, &b.path, b.cursor, b.destination), "group diverged: {id}, tick {tick}");
            }
        } else if tick == 40 {
            let mut other = BlastRange::restore(sim.config, &sim.snapshot()).unwrap();
            other.set_physics_workers(1); other.set_navigation_workers(2); copy = Some(other);
        }
        for id in 0..3 {
            let m = &sim.units[id].motion;
            if tick == 3 { assert!(m.pos[0] > Fx::from_int(300), "member {id} did not start promptly"); }
            stopped[id] = if m.blocked { stopped[id] + 1 } else { 0 };
            assert!(stopped[id] < 40, "member {id} stuck in its own group at {tick}");
        }
        if sim.units.iter().all(|u| u.motion.destination.is_none()) { break; }
    }
    assert!(sim.units.iter().all(|u| u.motion.destination.is_none()), "walking group never arrived: {:?}",
        sim.units.iter().map(|u| (&u.motion.pos, &u.motion.path, u.motion.cursor,
            u.motion.drive, u.motion.destination, u.motion.recovery_state())).collect::<Vec<_>>());
    assert_eq!(sim.state_hash(), copy.unwrap().state_hash());
}
