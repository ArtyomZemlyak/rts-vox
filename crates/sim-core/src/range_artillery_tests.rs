use super::artillery::{solve, velocity};
use super::*;
use crate::blast::{battle::Order, BlastRange};
use fx::angle;
use modding::range::{RangeLayout, RangeUnitDef};
use phys::V3;

fn arena(kind: RangeUnitKind) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.size = [640, 192, 640];
    l.blocks.clear();
    l.shields.clear();
    l.course = None;
    l.clear_min = [64, 64];
    l.clear_max = [600, 600];
    l.units = vec![
        RangeUnitDef {
            kind,
            allied: true,
            at: [170, 16, 336],
            weapon: None,
        },
        RangeUnitDef {
            kind: RangeUnitKind::Tank,
            allied: false,
            at: [450, 16, 336],
            weapon: None,
        },
    ];
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, l).unwrap();
    r.start_battle().unwrap();
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    for a in &mut r.battle.actors {
        a.order = Order::Hold;
    }
    r
}

fn step(r: &mut BlastRange) {
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.step();
}

fn angle_between(a: V3, b: V3) -> i32 {
    let cross = V3::new(
        a.y.mul(b.z).sub(a.z.mul(b.y)),
        a.z.mul(b.x).sub(a.x.mul(b.z)),
        a.x.mul(b.y).sub(a.y.mul(b.x)),
    );
    angle::atan2(cross.len_sq().sqrt().raw(), a.dot(b).raw())
}

#[test]
fn artillery_solution_matches_the_actual_muzzle_on_slopes_and_at_different_heights() {
    for kind in [
        RangeUnitKind::Artillery,
        RangeUnitKind::ArtilleryT2,
        RangeUnitKind::ArtilleryTurretT2,
    ] {
        let mut r = arena(kind);
        let weapon = if kind.canister() {
            Weapon::Canister
        } else {
            Weapon::Blast
        };
        let config = if kind.canister() {
            &r.battle.canister_config
        } else {
            &r.battle.shell_config
        };
        let milli = if kind.canister() {
            r.battle.tuning.canister.flight_time_milli
        } else {
            1000
        };
        for up in [
            V3::UP,
            V3::new(Fx::from_ratio(1, 4), Fx::ONE, Fx::from_ratio(-1, 5)),
        ] {
            let u = &mut r.units[0];
            u.pose = crate::damage::Frame::new(
                &u.body,
                V3::new(Fx::from_int(170), Fx::from_int(16), Fx::from_int(336)),
                angle::FULL / 7,
                up,
            );
            u.motion.drive.head = angle::FULL / 7;
            for target in [[450, 20, 336], [420, 110, 490], [100, 2, 120]] {
                let aim = solve(u, target, weapon, config, milli).unwrap();
                u.turret_delta = angle::diff(aim.yaw, u.motion.drive.head);
                u.turret_pitch = aim.pitch;
                let from = weapons::muzzles(u, aim.yaw)[0];
                let [x, y, z] = velocity(from, target, weapon, config, milli);
                let axis = u.root_frame(usize::from(u.body.turret_y)).x;
                let error = angle_between(axis, V3::new(x, y, z));
                assert!(
                    error <= r.turret_limits.tolerance,
                    "{kind:?} target={target:?} axis error={error}"
                );
                assert!(axis.dot(V3::new(x, y, z)) > Fx::ZERO);
            }
        }
    }
}

#[test]
fn artillery_elevates_before_launch_and_replays_during_the_slew_and_flight() {
    for kind in [
        RangeUnitKind::Artillery,
        RangeUnitKind::ArtilleryT2,
        RangeUnitKind::ArtilleryTurretT2,
    ] {
        let mut r = arena(kind);
        assert!(r.battle_order(0, Order::Ground([450, 100, 400]), None));
        step(&mut r);
        assert_ne!(r.units[0].turret_pitch, 0);
        assert_eq!(r.battle.actors[0].fired, 0, "fired before elevation");
        let mut replay = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
        let mut fired = false;
        for _ in 0..180 {
            let before = r.battle.actors[0].fired;
            let pitch = r.units[0].turret_pitch;
            step(&mut r);
            step(&mut replay);
            assert_eq!(r.state_hash(), replay.state_hash(), "{kind:?} slew replay");
            assert!(angle::diff(r.units[0].turret_pitch, pitch).abs() <= r.turret_limits.speed);
            if r.battle.actors[0].fired > before {
                // Flight has already applied one gravity tick; previous is
                // the muzzle and reconstructing the initial velocity removes it.
                let p = r.battle.rounds.iter().find(|p| p.owner == 0).unwrap();
                let config = if kind.canister() {
                    &r.battle.canister_config
                } else {
                    &r.battle.shell_config
                };
                let mut v = p.velocity;
                v[1] = v[1].add(Fx::from_ratio(config.gravity, crate::TICKS_PER_SECOND));
                let axis = r.units[0]
                    .root_frame(usize::from(r.units[0].body.turret_y))
                    .x;
                assert!(angle_between(axis, V3::new(v[0], v[1], v[2])) <= 12);
                let muzzle = weapons::muzzles(&r.units[0], r.battle.actors[0].turret.angle)[0];
                assert_eq!(p.previous, muzzle);
                fired = true;
                r.battle_order(0, Order::Hold, None);
                replay = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
                for _ in 0..12 {
                    step(&mut r);
                    step(&mut replay);
                    assert_eq!(r.state_hash(), replay.state_hash());
                }
                break;
            }
        }
        assert!(fired, "{kind:?} never completed elevation");
    }
}

#[test]
fn player_artillery_uses_the_same_barrel_solution_and_preserves_it_on_restore() {
    let mut l = arena(RangeUnitKind::Artillery).layout.unwrap();
    l.battle = false;
    l.gun_kind = RangeUnitKind::Artillery;
    l.gun = [170, 16, 336];
    l.target = [450, 100, 400];
    l.units.clear();
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    let mut fired = false;
    for _ in 0..180 {
        step(&mut r);
        if r.shell.is_some() {
            let mut v = r.velocity;
            v[1] = v[1].add(Fx::from_ratio(r.config.gravity, crate::TICKS_PER_SECOND));
            let axis = r.gun.root_frame(usize::from(r.gun.body.turret_y)).x;
            assert!(angle_between(axis, V3::new(v[0], v[1], v[2])) <= 12);
            let mut replay = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
            for _ in 0..20 {
                step(&mut r);
                step(&mut replay);
                assert_eq!(r.state_hash(), replay.state_hash());
            }
            fired = true;
            break;
        }
    }
    assert!(fired);
}

#[test]
fn legacy_artillery_without_elevation_restores_and_invalid_pitch_is_rejected() {
    for kind in [
        RangeUnitKind::Artillery,
        RangeUnitKind::ArtilleryT2,
        RangeUnitKind::ArtilleryTurretT2,
    ] {
        let r = arena(kind);
        let version = if kind.canister() { 51 } else { 48 };
        let mut tail = Blob::new();
        r.save_after(&mut tail, version);
        let mut old = r.snapshot();
        old.truncate(old.len() - tail.v.len());
        old[..4].copy_from_slice(&version.to_le_bytes());
        let restored = BlastRange::restore(r.config.clone(), &old).unwrap();
        assert_eq!(restored.units[0].turret_pitch, 0);
        assert_eq!(
            restored.battle.actors[0].elevation,
            crate::turret::Traverse::new(0)
        );
        let mut invalid = r;
        invalid.units[0].turret_pitch = angle::QUARTER + 1;
        assert!(BlastRange::restore(invalid.config.clone(), &invalid.snapshot()).is_none());
    }
}

#[test]
fn artillery_elevation_brakes_at_vertical_and_keeps_a_valid_saved_pose() {
    for sign in [-1, 1] {
        let mut r = arena(RangeUnitKind::ArtilleryT2);
        let pitch = sign * (angle::QUARTER - 1);
        r.units[0].turret_pitch = pitch;
        r.battle.actors[0].elevation = crate::turret::Traverse::new(pitch);
        r.battle.actors[0].elevation.rate = sign * r.turret_limits.speed;
        r.battle.actors[0].next = r.tick + 100;
        assert!(r.battle_order(0, Order::Ground([450, 100, 400]), None));
        step(&mut r);
        assert_eq!(r.units[0].turret_pitch, sign * angle::QUARTER);
        assert_eq!(r.battle.actors[0].elevation.rate, 0);
        assert!(BlastRange::restore(r.config.clone(), &r.snapshot()).is_some());
    }
}

#[test]
fn canister_leads_a_crossing_tank_for_its_actual_lofted_flight() {
    for kind in [RangeUnitKind::ArtilleryT2, RangeUnitKind::ArtilleryTurretT2] {
        for speed in [-45, 45] {
            let mut l = arena(kind).layout.unwrap();
            l.size = [2208, 192, 1600];
            l.clear_max = [2100, 1500];
            l.units[0].at[2] = 800;
            l.units[1].at = [1450, 16, 800];
            let c =
                ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
            let mut r = BlastRange::with_layout(c, l).unwrap();
            r.start_battle().unwrap();
            r.battle.actors[1].order = Order::Hold;
            r.units[1].motion.drive.head = angle::QUARTER;
            r.units[1].motion.drive.vel = Fx::from_int(speed);
            r.units[1].update_pose(angle::QUARTER);
            r.battle_order(0, Order::Attack(1), None);
            // Isolate the real aiming/charging/launch gates from acceleration
            // and route changes: the target has a known constant crossing speed.
            for _ in 0..180 {
                let mut b = std::mem::replace(&mut r.battle, Battle::new(&r.units, 1));
                r.battle_orders(&mut b, |_| {}, &|_, _| {});
                r.battle = b;
                if !r.battle.rounds.is_empty() {
                    break;
                }
                r.tick += 1;
            }
            let p = r.battle.rounds.first().expect("never launched");
            let target = aim_point(&r.units[1]);
            let time = Fx::from_int(target[0]).sub(p.pos[0]).div(p.velocity[0]);
            let shell_z = p.pos[2].add(p.velocity[2].mul(time));
            let target_z = Fx::from_int(target[2]).add(Fx::from_int(speed).mul(time));
            let miss = shell_z.sub(target_z).abs();
            let height = p.pos[1].add(p.velocity[1].mul(time)).sub(
                Fx::from_int(r.battle.canister_config.gravity)
                    .mul(time)
                    .mul(time.add(Fx::from_ratio(1, crate::TICKS_PER_SECOND)))
                    .mul(Fx::from_ratio(1, 2)),
            );
            eprintln!("{kind:?} speed={speed} launch={:?} v={:?} crossing_miss={miss:?} height={height:?} target={target:?} pitch={}", p.pos, p.velocity, r.units[0].turret_pitch);
            assert!(
                miss < Fx::from_int(8),
                "{kind:?} speed={speed} crossing miss={miss:?}"
            );
            // Keep only the unspread central round. Translate the target at
            // its known velocity; production sweep/contact must reach its body.
            r.battle.rounds.truncate(1);
            r.battle.actors[0].order = Order::Hold;
            r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
            let mut replay = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
            assert_eq!(r.state_hash(), replay.state_hash());
            let mut hit = false;
            for _ in 0..600 {
                for sim in [&mut r, &mut replay] {
                    sim.units[1].motion.pos[2] = sim.units[1].motion.pos[2]
                        .add(Fx::from_ratio(speed, crate::TICKS_PER_SECOND));
                    sim.units[1].update_pose(angle::QUARTER);
                    let mut b = std::mem::replace(&mut sim.battle, Battle::new(&sim.units, 1));
                    sim.battle_flight(&mut b);
                    sim.battle = b;
                    sim.tick += 1;
                    sim.battle.ai.next_tick = sim.tick + sim.battle.ai.tuning.decision_ticks;
                }
                assert_eq!(r.state_hash(), replay.state_hash(), "moving contact replay");
                hit = r.battle.impacts.iter().any(|i| r.units[1].solid(i.at));
                if hit || r.battle.rounds.is_empty() {
                    break;
                }
            }
            assert!(
                hit,
                "{kind:?} speed={speed} central round did not contact the moving body"
            );
        }
    }
}

#[test]
fn artillery_interception_matches_each_ammo_flight_time_without_range_overflow() {
    let r = arena(RangeUnitKind::ArtilleryT2);
    for (weapon, config, milli) in [
        (Weapon::Blast, &r.battle.shell_config, 1000),
        (
            Weapon::Canister,
            &r.battle.canister_config,
            r.battle.tuning.canister.flight_time_milli,
        ),
        (Weapon::Kinetic, &r.battle.kinetic_config, 1000),
        (Weapon::Energy, &r.battle.energy_config, 1000),
        (Weapon::Annihilation, &r.battle.anni_config, 1000),
    ] {
        for target in [[1450, 80, 800], [18000, 1000, 10000]] {
            let from = [Fx::from_int(170), Fx::from_int(40), Fx::from_int(800)];
            for motion in [
                [Fx::ZERO; 3],
                [Fx::from_int(70), Fx::ZERO, Fx::ZERO],
                [Fx::from_int(-70), Fx::ZERO, Fx::ZERO],
                [Fx::ZERO, Fx::ZERO, Fx::from_int(45)],
                [Fx::from_int(45), Fx::ZERO, Fx::from_int(-30)],
            ] {
                let point = artillery::intercept(from, target, motion, weapon, config, milli);
                let time = if weapon.energetic() {
                    let delta: [Fx; 3] =
                        std::array::from_fn(|k| Fx::from_int(point[k]).sub(from[k]));
                    delta
                        .iter()
                        .fold(Fx::ZERO, |n, d| n.add(d.mul(*d)))
                        .sqrt()
                        .div(Fx::from_int(config.shell_speed))
                } else {
                    artillery::flight_time(from, point, config.shell_speed, milli)
                };
                for k in 0..3 {
                    let future = Fx::from_int(target[k]).add(motion[k].mul(time));
                    assert!(
                        future.sub(Fx::from_int(point[k])).abs() < Fx::from_int(2),
                        "{weapon:?} target={target:?} motion={motion:?} point={point:?}"
                    );
                }
                if motion == [Fx::ZERO; 3] {
                    assert_eq!(point, target);
                }
            }
        }
    }
    let target = [1450, 80, 800];
    let from = [Fx::from_int(170), Fx::from_int(40), Fx::from_int(800)];
    // A retreating target faster than the front cannot be intercepted.
    for speed in [140, 200, 1000] {
        assert_eq!(
            artillery::intercept(
                from,
                target,
                [Fx::from_int(speed), Fx::ZERO, Fx::ZERO],
                Weapon::Canister,
                &r.battle.canister_config,
                r.battle.tuning.canister.flight_time_milli
            ),
            target
        );
    }
}
