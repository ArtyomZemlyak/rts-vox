//! Qualification through actual battle servos, muzzles and thermal contacts.
use super::*;
use modding::range::{RangeBlock, RangeUnitDef};

fn trial(targets: &[(RangeUnitKind, [i32; 3])], blocks: Vec<RangeBlock>) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.size = [640, 128, 640];
    l.course = None;
    l.clear_min = [16, 16];
    l.clear_max = [624, 624];
    l.blocks = blocks;
    l.shields.clear();
    l.battle = true;
    l.units = vec![RangeUnitDef {
        kind: RangeUnitKind::Spider,
        allied: true,
        at: [200, 16, 336],
        weapon: None,
    }];
    l.units
        .extend(targets.iter().map(|&(kind, at)| RangeUnitDef {
            kind,
            allied: false,
            at,
            weapon: None,
        }));
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.start_battle().unwrap();
    for a in &mut r.battle.actors[1..] {
        a.order = Order::Hold;
    }
    r
}
fn step(r: &mut BlastRange) {
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.step();
}
fn beam(r: &mut BlastRange) {
    for _ in 0..360 {
        step(r);
        if !r.battle.lasers.is_empty() {
            return;
        }
    }
    panic!("siege beam never started");
}
#[test]
fn spider_laser_tracking_starts_before_alignment_and_ray_follows_the_real_barrel() {
    let mut r = trial(&[(RangeUnitKind::Tank, [200, 16, 560])], vec![]);
    assert!(r.battle_order(0, Order::Attack(1), None));
    beam(&mut r);
    let error = fx::angle::diff(fx::angle::QUARTER, r.battle.actors[0].turret.angle).abs();
    assert!(error > fx::angle::FULL / 36, "beam waited for alignment");
    let trace = r.battle.lasers[0];
    let d = phys::V3::new(
        trace.to[0].sub(trace.from[0]),
        trace.to[1].sub(trace.from[1]),
        trace.to[2].sub(trace.from[2]),
    );
    let f = r.units[0].root_frame(usize::from(r.units[0].body.turret_y));
    let dot = d.dot(f.x);
    assert!(dot > Fx::ZERO);
    assert!(
        dot.mul(dot) >= d.len_sq().mul(Fx::from_ratio(9999, 10000)),
        "ray left barrel axis"
    );
    assert_ne!(r.units[0].turret_pitch, 0);
    let mut replay = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    for _ in 0..120 {
        let was = r.battle.actors[0].turret;
        step(&mut r);
        step(&mut replay);
        let limits = crate::turret::Limits::of(
            motion::spider_tuning().laser_tracking.unwrap().yaw,
            crate::TICKS_PER_SECOND,
        );
        assert!(fx::angle::diff(r.battle.actors[0].turret.angle, was.angle).abs() <= limits.speed);
        assert!((r.battle.actors[0].turret.rate - was.rate).abs() <= limits.accel);
        assert_eq!(r.state_hash(), replay.state_hash(), "mid-transfer replay");
    }
    assert!(r.matter.economy.ledger.balanced());
}
#[test]
fn spider_laser_tracking_live_retarget_burns_an_unselected_unit_between_targets() {
    let mut r = trial(
        &[
            (RangeUnitKind::Tank, [440, 16, 336]),
            (RangeUnitKind::Tank, [460, 16, 431]),
            (RangeUnitKind::Tank, [420, 16, 536]),
        ],
        vec![],
    );
    assert!(r.battle_order(0, Order::Attack(1), None));
    beam(&mut r);
    for _ in 0..150 {
        if !r.units[1].status.alive {
            break;
        }
        step(&mut r);
    }
    assert!(!r.units[1].status.alive, "initial victim survived");
    let burst = r.battle.actors[0].burst;
    let yaw = r.battle.actors[0].turret.angle;
    assert!(burst > 0);
    assert!(r.battle_order(0, Order::Attack(3), None));
    assert_eq!(r.battle.actors[0].burst, burst, "command restarted pulse");
    step(&mut r);
    assert_eq!(
        r.battle.lasers.len(),
        1,
        "beam switched off during retarget"
    );
    assert_eq!(r.battle.actors[0].burst, burst + 1);
    assert!(fx::angle::diff(r.battle.actors[0].turret.angle, yaw).abs() < fx::angle::FULL / 360);
    let mut replay = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    let mut collateral = false;
    for _ in 0..210 {
        let active = r.battle.actors[0].burst;
        step(&mut r);
        step(&mut replay);
        assert_eq!(r.state_hash(), replay.state_hash());
        collateral |= !r.units[2].removed.is_empty();
        if r.units[3].status.alive
            && active > 0
            && active + 1 < motion::spider_tuning().laser.burst_ticks
        {
            assert_eq!(r.battle.lasers.len(), 1, "live pulse interrupted");
        }
        if !r.units[3].status.alive {
            break;
        }
    }
    eprintln!(
        "live sweep: burst={burst} collateral={collateral} final_alive={} tick={}",
        r.units[3].status.alive, r.tick
    );
    assert!(collateral, "unselected intermediate tank took no damage");
    assert!(!r.units[3].status.alive, "sweep did not reach final victim");
    assert!(!r.thermals.cells().any(|(c, _)| c.unit == 0));
    assert!(r.matter.economy.ledger.balanced());
}
#[test]
fn spider_laser_tracking_breaches_multiple_steel_and_concrete_thicknesses() {
    for material in [2, 3] {
        for thickness in [2, 8, 20, 30] {
            let mut r = trial(
                &[(RangeUnitKind::Tank, [540, 16, 480])],
                vec![RangeBlock {
                    min: [390, 0, 320],
                    max: [390 + thickness, 128, 352],
                    material,
                }],
            );
            assert!(r.battle_order(0, Order::Ground([600, 100, 336]), None));
            beam(&mut r);
            let first = r.tick;
            let mut breached = None;
            for _ in 0..thickness + 30 {
                if r.battle
                    .lasers
                    .first()
                    .is_some_and(|t| t.to[0].to_int_trunc() >= 390 + thickness)
                {
                    breached = Some(r.tick - first);
                    break;
                }
                step(&mut r);
            }
            eprintln!("siege plate: material={material} thickness_dm={thickness} breach_ticks={breached:?} melted={}", r.heat.melted_total);
            assert!(
                breached.is_some_and(|t| t <= thickness as u32 + 15),
                "slow breach"
            );
            assert!(
                r.heat.melted_total > 0,
                "plate vanished without thermal melting"
            );
            assert!(r.units[0].removed.is_empty(), "emitter damaged itself");
            assert!(r.matter.economy.ledger.balanced());
        }
    }
}
#[test]
fn spider_laser_tracking_quickly_melts_the_actual_armour_of_every_combat_class() {
    for kind in [
        RangeUnitKind::Tank,
        RangeUnitKind::Strider,
        RangeUnitKind::Artillery,
        RangeUnitKind::ArtilleryT2,
        RangeUnitKind::TurretT1,
        RangeUnitKind::TurretT2,
        RangeUnitKind::Spider,
    ] {
        let mut r = trial(&[(kind, [450, 16, 336])], vec![]);
        assert!(r.battle_order(0, Order::Attack(1), None));
        beam(&mut r);
        let first = r.tick;
        let mut death = None;
        for _ in 0..if kind == RangeUnitKind::Spider { 240 } else { 120 } {
            if !r.units[1].status.alive {
                death = Some(r.tick - first);
                break;
            }
            step(&mut r);
        }
        eprintln!(
            "siege victim: {kind:?} death_ticks={death:?} core={:?} melted={}",
            r.units[1].integrity.core, r.thermals.melted
        );
        assert!(
            death.is_some_and(|t| t <= if kind == RangeUnitKind::Spider { 180 } else { 90 }),
            "siege beam failed against {kind:?}"
        );
        assert!(r.thermals.melted > 0);
        assert!(r.units[0].removed.is_empty());
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn spider_laser_tracking_group_move_preserves_fire_and_hold_consumes_the_pulse() {
    let mut r = trial(&[(RangeUnitKind::Tank, [440, 16, 336])], vec![]);
    assert!(r.battle_order(0, Order::Attack(1), None));
    beam(&mut r);
    let burst = r.battle.actors[0].burst;
    assert_eq!(r.battle_group_move(&[(0, [250, 336])], [250, 336]), 1);
    assert_eq!(r.battle.actors[0].burst, burst);
    step(&mut r);
    assert_eq!(r.battle.lasers.len(), 1);
    assert!(r.battle_order(0, Order::Hold, None));
    assert_eq!(r.battle.actors[0].burst, 0);
    let next = r.battle.actors[0].next;
    assert!(next >= r.tick + motion::spider_tuning().laser.reload_ticks);
    assert!(r.battle_order(0, Order::Attack(1), None));
    while r.tick + 1 < next {
        step(&mut r);
        assert!(r.battle.lasers.is_empty());
    }
}

#[test]
fn spider_laser_tracking_frozen_wave_pose_survives_aim_changes_and_snapshot_corruption_is_rejected()
{
    let mut r = trial(&[(RangeUnitKind::Tank, [440, 16, 336])], vec![]);
    assert!(r.battle_order(0, Order::Attack(1), None));
    beam(&mut r);
    let mut c = r.config.clone();
    c.energy = 1;
    r.explosion = Some(Explosion::new(&r.world, [280, 75, 336], &c, &r.units));
    let captured = r.explosion.as_ref().unwrap().turret_pitches.clone();
    assert!(captured.iter().any(|p| *p != 0));
    assert!(r.battle_order(0, Order::Ground([600, 95, 380]), None));
    for _ in 0..5 {
        step(&mut r);
    }
    let bytes = r.snapshot();
    assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 52);
    let mut replay = BlastRange::restore(r.config.clone(), &bytes).unwrap();
    assert_eq!(r.state_hash(), replay.state_hash());
    if let Some(e) = &replay.explosion {
        assert_eq!(e.turret_pitches, captured);
    }
    for _ in 0..40 {
        step(&mut r);
        step(&mut replay);
        assert_eq!(r.state_hash(), replay.state_hash());
    }
    let mut truncated = bytes;
    truncated.pop();
    assert!(BlastRange::restore(r.config.clone(), &truncated).is_none());
}

#[test]
fn spider_laser_tracking_ignites_wood_and_foliage_instead_of_melting_them() {
    for material in [4, 5] {
        let mut r = trial(
            &[(RangeUnitKind::Tank, [540, 16, 480])],
            vec![RangeBlock {
                min: [390, 0, 320],
                max: [398, 128, 352],
                material,
            }],
        );
        assert!(r.battle_order(0, Order::Ground([600, 100, 336]), None));
        beam(&mut r);
        for _ in 0..15 {
            step(&mut r);
        }
        assert!(r.heat.burning() > 0 || r.heat.burned_by_material[material as usize] > 0);
        assert_eq!(r.heat.melted_total, 0);
        assert!(r.matter.economy.ledger.balanced());
    }
}
