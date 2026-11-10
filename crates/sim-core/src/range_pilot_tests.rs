use super::*;
use command_tests::{arena, order, same};

fn pilot(throttle: i8, turn: i8, fire: bool) -> Order {
    Order::Pilot {
        throttle,
        turn,
        aim: [410, 22, 336],
        fire,
    }
}

#[test]
fn skoba_pilot_flies_turns_brakes_and_replays_above_ground() {
    let mut a = arena(RangeUnitKind::SkobaDrone);
    a.start_free_control().unwrap();
    let y = a.units[0].motion.pos[1];
    let start = a.units[0].motion.pos[0];
    a.battle_order(0, pilot(1, 1, false), None);
    for _ in 0..10 {
        a.step();
    }
    assert!(a.units[0].motion.pos[0] > start);
    assert_ne!(a.units[0].motion.drive.head, 0);
    assert_eq!(a.units[0].motion.pos[1], y);
    let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
    a.set_physics_workers(1);
    b.set_physics_workers(2);
    for _ in 0..12 {
        a.step();
        b.step();
        same(&a, &b);
    }
    a.battle_order(0, Order::Hold, None);
    for _ in 0..60 {
        a.step();
    }
    assert_eq!(a.units[0].motion.drive.vel, Fx::ZERO);
    assert_eq!(a.units[0].motion.drive.yaw_rate, 0);
    assert_eq!(a.units[0].motion.pos[1], y);
}

#[test]
fn skoba_pilot_cannot_cross_a_thin_wall_or_another_hull() {
    for terrain in [false, true] {
        let mut r = arena(RangeUnitKind::SkobaDrone);
        r.units[1].motion.pos[0] = Fx::from_int(270);
        r.units[1].update_pose(0);
        if terrain {
            for y in 16..70 {
                for z in 285..385 {
                    r.world.queue_transfer(voxel::Edit::put(240, y, z, 2));
                }
            }
            r.world.flush_transfers();
        }
        r.start_free_control().unwrap();
        r.battle_order(0, pilot(1, 0, false), None);
        for _ in 0..100 {
            r.step();
        }
        assert!(r.units[0].motion.pos[0] > Fx::from_int(170));
        assert!(r.units[0].motion.pos[0] < Fx::from_int(if terrain { 209 } else { 207 }));
        assert!(r.units[0].motion.blocked);
    }
}
#[test]
fn pilot_all_kinds_move_or_hold_foundation_and_resume_snapshots() {
    for kind in RangeUnitKind::ALL {
        let mut sim = arena(kind);
        sim.start_free_control().unwrap();
        assert!(
            sim.execute_command(order(0, pilot(1, 0, false), None))
                .accepted(),
            "{kind:?}"
        );
        let before = sim.units[0].motion.pos;
        for _ in 0..45 {
            sim.step();
        }
        if kind.emplacement() {
            assert_eq!(before[0], sim.units[0].motion.pos[0], "{kind:?}");
            assert_eq!(before[2], sim.units[0].motion.pos[2], "{kind:?}");
        } else {
            assert!(
                sim.units[0].motion.pos[0] > before[0].add(Fx::ONE),
                "{kind:?}"
            );
        }
        let mut copy = BlastRange::restore(sim.config, &sim.snapshot()).expect("pilot snapshot");
        for _ in 0..12 {
            sim.step();
            copy.step();
            same(&sim, &copy);
        }
        assert!(sim.execute_command(order(0, Order::Hold, None)).accepted());
        for _ in 0..60 {
            sim.step();
        }
        assert_eq!(sim.units[0].motion.drive.vel, Fx::ZERO, "{kind:?}");
    }
}
#[test]
fn pilot_reverse_steering_and_release_retain_chassis_limits() {
    for kind in [
        RangeUnitKind::Tank,
        RangeUnitKind::Strider,
        RangeUnitKind::Spider,
        RangeUnitKind::Artillery,
        RangeUnitKind::ArtilleryT2,
    ] {
        let mut sim = arena(kind);
        sim.start_free_control().unwrap();
        let before = sim.units[0].motion.pos;
        assert!(sim
            .execute_command(order(0, pilot(-1, 0, false), None))
            .accepted());
        for _ in 0..30 {
            sim.step();
        }
        assert!(sim.units[0].motion.pos[0] < before[0], "reverse {kind:?}");
        let head = sim.units[0].motion.drive.head;
        assert!(sim
            .execute_command(order(0, pilot(1, 1, false), None))
            .accepted());
        for _ in 0..30 {
            sim.step();
            assert!(sim.units[0].motion.drive.vel.abs() <= Fx::from_int(sim.units[0].motion.speed));
        }
        assert_ne!(head, sim.units[0].motion.drive.head, "steering {kind:?}");
        assert!(sim.units[0].motion.destination.is_none());
        assert!(!sim.units[0].motion.searching());
    }
}
#[test]
fn pilot_rejects_bad_axes_enemy_and_setup_fire_without_mutating_world() {
    let mut sim = arena(RangeUnitKind::Tank);
    for request in [
        order(0, pilot(2, 0, false), None),
        order(0, pilot(0, -2, false), None),
        order(1, pilot(1, 0, false), None),
        order(0, pilot(0, 0, true), None),
        order(0, pilot(0, 0, false), Some([200, 336])),
        order(
            0,
            Order::Pilot {
                throttle: 0,
                turn: 0,
                aim: [-1, 0, 0],
                fire: false,
            },
            None,
        ),
    ] {
        let before = sim.snapshot();
        assert_eq!(sim.execute_command(request), commands::Outcome::Rejected);
        assert_eq!(before, sim.snapshot());
    }
}
#[test]
fn pilot_tracking_without_fire_does_not_shoot_and_out_of_range_never_pursues() {
    for kind in RangeUnitKind::ALL {
        let mut sim = arena(kind);
        sim.layout.as_mut().unwrap().idle_enemies = true;
        sim.start_battle().unwrap();
        assert!(!sim.execute_command(order(1, Order::Hold, None)).accepted()); // enemy is never player-owned
        sim.battle.actors[1].order = Order::Hold;
        assert!(sim
            .execute_command(order(0, pilot(0, 0, false), None))
            .accepted());
        for _ in 0..45 {
            sim.step();
        }
        assert_eq!(sim.battle.actors[0].fired, 0, "{kind:?}");
        assert!(sim.units[0].motion.destination.is_none());
        let far = Order::Pilot {
            throttle: 0,
            turn: 0,
            aim: [639, 190, 639],
            fire: true,
        };
        assert!(
            sim.execute_command(order(0, far, None)).accepted(),
            "far rejected {kind:?} phase={:?} size={:?}",
            sim.battle.phase,
            sim.size()
        );
        for _ in 0..8 {
            sim.step();
        }
        assert!(
            sim.units[0].motion.destination.is_none(),
            "pursuit {kind:?}"
        );
    }
}
#[test]
fn pilot_live_fire_and_aim_updates_keep_charge_then_release_cancels_it() {
    for kind in [
        RangeUnitKind::Tank,
        RangeUnitKind::Spider,
        RangeUnitKind::TurretT1,
    ] {
        let mut sim = arena(kind);
        sim.layout.as_mut().unwrap().idle_enemies = true;
        sim.start_battle().unwrap();
        sim.battle.actors[1].order = Order::Hold;
        assert!(sim
            .execute_command(order(0, pilot(0, 0, true), None))
            .accepted());
        sim.battle.trace_fire(true);
        for _ in 0..200 {
            let mut aim = [410, 22, 336];
            // Move the sight during acquisition, then let the physical
            // traverse settle; aiming tolerance still gates the shot.
            if sim.tick < 30 {
                aim[2] += ((sim.tick / 5) % 2) as i32;
            }
            let charge = sim.battle.actors[0].charge;
            sim.execute_command(order(
                0,
                Order::Pilot {
                    throttle: 0,
                    turn: 0,
                    aim,
                    fire: true,
                },
                None,
            ));
            assert_eq!(
                charge, sim.battle.actors[0].charge,
                "aim update reset charge"
            );
            sim.step();
            if sim.battle.actors[0].fired > 0 {
                break;
            }
        }
        assert!(sim.battle.actors[0].fired > 0, "{kind:?} never fired gate={:?} turret={:?} phase={:?} charge={:?} status={:?} order={:?}", sim.battle.fire_gate(0),sim.battle.actors[0].turret,sim.battle.phase,sim.battle.actors[0].charge,sim.units[0].status,sim.battle.actors[0].order);
        sim.execute_command(order(0, pilot(0, 0, false), None));
        sim.step();
        assert!(sim.battle.actors[0].charge.is_none(), "{kind:?}");
    }
}

#[test]
fn pilot_workers_match_through_turning_reversing_and_fresh_wall_contact() {
    let mut a = arena(RangeUnitKind::Tank);
    a.start_free_control().unwrap();
    let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
    a.set_physics_workers(1);
    b.set_physics_workers(4);
    a.set_navigation_workers(1);
    b.set_navigation_workers(4);
    for tick in 0..150 {
        if tick == 10 {
            for sim in [&mut a, &mut b] {
                for x in 212..216 {
                    for y in 16..56 {
                        for z in 290..380 {
                            sim.world.queue_edit(voxel::Edit::put(x, y, z, 2));
                        }
                    }
                }
                while sim.world.queued() > 0 {
                    sim.world.flush();
                }
            }
        }
        let request = order(
            0,
            pilot(
                if tick < 90 { 1 } else { -1 },
                if tick >= 110 { 1 } else { 0 },
                false,
            ),
            None,
        );
        assert!(a.execute_command(request).accepted());
        assert!(b.execute_command(request).accepted());
        a.step();
        b.step();
        same(&a, &b);
        if (30..90).contains(&tick) {
            assert!(a.units[0].motion.pos[0] < Fx::from_int(212));
        }
    }
    assert!(a.units[0].motion.destination.is_none());
}

#[test]
fn compact_skoba_is_faster_in_manual_flight() {
    let mut open = arena(RangeUnitKind::SkobaDrone);
    let mut compact = arena(RangeUnitKind::SkobaDrone);
    open.start_free_control().unwrap();
    compact.start_free_control().unwrap();
    compact.units[0].restore_skoba_form(1);
    compact.units[0].update_pose(0);
    assert!(open.battle_order(0, pilot(1, 0, false), None));
    assert!(compact.battle_order(0, pilot(1, 0, false), None));
    for _ in 0..30 { open.step(); compact.step(); }
    assert!(compact.units[0].motion.pos[0] > open.units[0].motion.pos[0], "open {:?} {:?}, compact {:?} {:?}", open.units[0].motion.pos, open.units[0].motion.drive.vel, compact.units[0].motion.pos, compact.units[0].motion.drive.vel);
}
