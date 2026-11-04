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
fn spider_laser_tracking_settles_on_close_and_low_final_points_without_hunting() {
    for point in [[300, 100, 360], [450, 18, 400]] {
        let mut r = trial(&[(RangeUnitKind::Tank, [540, 16, 540])], vec![]);
        // Isolate head tracking from suspension motion and incoming damage.
        // The actual battle servos, beam, heat and pulse/reload still run.
        r.units[0].motion.enabled = false;
        r.units[1].motion.enabled = false;
        r.units[1].status.weapon = false;
        assert!(r.battle_order(0, Order::Ground(point), None));
        for _ in 0..350 {
            step(&mut r);
        }
        let settled = (r.battle.actors[0].turret.angle, r.units[0].turret_pitch);
        let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        assert!(replay.set_physics_workers(1));
        assert!(replay.set_navigation_workers(1));
        let mut live = 0;
        for _ in 0..120 {
            step(&mut r);
            step(&mut replay);
            let actor = &r.battle.actors[0];
            assert_eq!(
                (actor.turret.angle, r.units[0].turret_pitch),
                settled,
                "head hunted a stationary final point {point:?}"
            );
            assert_eq!((actor.turret.rate, actor.elevation.rate), (0, 0));
            assert_eq!(r.state_hash(), replay.state_hash());
            assert!(r.matter.economy.ledger.balanced());
            live += r.battle.lasers.len();
        }
        assert!(live > 0, "a stopped beam cannot prove stable live tracking");
    }
}

#[test]
fn spider_laser_tracking_pitch_depends_on_mount_geometry_not_previous_head_pose() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::Spider, [200, 16, 336]);
    for target in [
        [300, 100, 336],
        [450, 18, 336],
        [560, 100, 336],
        [400, 100, 500],
    ] {
        let pitch = spider_laser_pitch(&u, target);
        for previous in [-12000, -1000, 0, 1000, 12000] {
            u.turret_pitch = previous;
            u.update_pose(previous);
            assert_eq!(spider_laser_pitch(&u, target), pitch);
        }
        let yaw = fx::angle::atan2(i64::from(target[2] - 336), i64::from(target[0] - 200));
        u.update_pose(yaw);
        u.turret_pitch = pitch;
        let from = weapons::muzzles(&u, yaw)[0];
        let f = u.root_frame(usize::from(u.body.turret_y));
        let delta = phys::V3::new(
            Fx::from_int(target[0]).sub(from[0]),
            Fx::from_int(target[1]).sub(from[1]),
            Fx::from_int(target[2]).sub(from[2]),
        );
        // Independent geometric check: the actual muzzle axis reaches the
        // authored point within one world cell, including the barrel offset.
        assert!(delta.dot(f.x) > Fx::ZERO);
        assert!(delta.dot(f.y).abs() < Fx::ONE);
    }
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
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
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
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    let mut collateral = false;
    // A swept grazing hit can need a second thermal pulse at a different
    // fixed-step phase. Bound the trial by two authored pulses and cooldown.
    let tuning = motion::spider_tuning();
    let limit =
        modding::clock::stream_ticks(tuning.laser.burst_ticks * tuning.laser.interval_ticks * 2)
            + tuning.laser.reload_ticks
            + tuning.charge_ticks;
    for _ in 0..limit {
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
    assert!(!r.units[3].status.alive, "sweep did not reach final victim: removed={} heat_cells={} burst={} target={:?} yaw={} pitch={} pos={:?}", r.units[3].removed.len(), r.thermals.cells().count(), r.battle.actors[0].burst, r.battle.actors[0].target, r.battle.actors[0].turret.angle, r.units[0].turret_pitch, r.units[3].motion.pos);
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
        for _ in 0..if kind == RangeUnitKind::Spider {
            240
        } else {
            120
        } {
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
            death.is_some_and(|t| t
                <= if kind == RangeUnitKind::Spider {
                    180
                } else {
                    90
                }),
            "siege beam failed against {kind:?}"
        );
        assert!(r.thermals.melted > 0);
        assert!(r.units[0].removed.is_empty());
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn spider_live_beam_keeps_melting_armour_while_a_hot_crater_competes_for_slots() {
    let mut r = trial(
        &[(RangeUnitKind::Tank, [450, 16, 336])],
        vec![RangeBlock {
            min: [40, 20, 40],
            max: [72, 127, 72],
            material: 3,
        }],
    );
    r.matter.tuning.max_flying = 512;
    r.matter.legacy_melt_for_test();
    r.heat_tuning.melt_flow = [None; 3];
    assert!(r.battle_order(0, Order::Attack(1), None));
    beam(&mut r);
    let first = r.tick;
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    replay.matter.tuning.max_flying = 512; // authored tuning is not stored in saves
    replay.matter.legacy_melt_for_test();
    replay.heat_tuning.melt_flow = [None; 3];
    for _ in 0..120 {
        for sim in [&mut r, &mut replay] {
            for x in 40..72 {
                for z in 40..72 {
                    let y = sim.world.column_ceiling(x, z).saturating_sub(1);
                    sim.heat.add(&sim.world, [x, y, z], 100_000);
                }
            }
            step(sim);
            assert!(sim.matter.debris.fragment_pools(&sim.matter.tuning).flying <= 512);
            assert!(sim.matter.economy.ledger.balanced());
        }
        assert_eq!(r.state_hash(), replay.state_hash());
        if !r.units[1].status.alive {
            break;
        }
    }
    eprintln!(
        "contested siege beam: ticks={} alive={} melted={} terrain_melt={:?}",
        r.tick - first,
        r.units[1].status.alive,
        r.thermals.melted,
        r.matter.admission.terrain_melt
    );
    assert!(
        !r.units[1].status.alive,
        "live siege beam was starved by unrelated terrain melting"
    );
    assert!(r.thermals.melted > 0);
    assert!(r.matter.admission.terrain_melt[1] > 0);
}

#[test]
fn spider_direct_beam_damages_new_target_when_heat_field_is_full_of_hot_cells() {
    let mut r = trial(
        &[
            (RangeUnitKind::Tank, [450, 16, 336]),
            (RangeUnitKind::Tank, [540, 16, 540]),
        ],
        vec![],
    );
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.thermal_tuning.max_cells = 64;
    r.thermal_tuning.cells_per_tick = 64;
    r.thermal_tuning.cooling_milli = 0;
    r.thermal_tuning.conduction_milli = 0;
    let body = &r.units[2].body;
    let mut old = Vec::new();
    'outer: for x in 0..body.vol.nx {
        for y in 0..body.vol.ny {
            for z in 0..body.vol.nz {
                if body.vol.get(x as usize, y as usize, z as usize)
                    && body.part_at(x as usize, y as usize, z as usize) == voxel::Part::Armour
                {
                    old.push(thermal::Cell {
                        unit: 2,
                        part: 0,
                        at: [x, y, z],
                    });
                    if old.len() == 64 {
                        break 'outer;
                    }
                }
            }
        }
    }
    assert_eq!(old.len(), 64);
    for p in old {
        r.thermals.deposit(p, 300, &r.thermal_tuning);
    }
    assert_eq!(r.thermals.hot(), 64);
    for u in &mut r.units {
        u.motion.enabled = false;
    }
    assert!(r.battle_order(0, Order::Attack(1), None));
    beam(&mut r);
    for _ in 0..40 {
        step(&mut r);
    }
    assert!(
        r.thermals.melted > 0,
        "direct laser heat was rejected by a full field of hot cells"
    );
    assert!(
        r.thermals.hot() >= 64,
        "accepted heat must not evict earlier cells"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn laser_heated_steel_cover_reserves_capacity_before_unrelated_wave_damage() {
    let mut r = trial(
        &[(RangeUnitKind::Tank, [450, 16, 336])],
        vec![RangeBlock {
            min: [40, 20, 40],
            max: [44, 24, 44],
            material: 3,
        }],
    );
    r.matter.tuning.max_flying = 1;
    r.matter.legacy_melt_for_test();
    r.heat_tuning.melt_flow = [None; 3];
    r.matter.tuning.max_fragments = 1;
    let p = [41, 23, 41];
    r.heat.add(&r.world, p, 16_000);
    r.reserve_thermal_fragments();
    assert_eq!(r.matter.claim_room(), 0);
    r.matter.begin(1_000_000, 2);
    assert!(matches!(
        r.matter.ground(&mut r.world, [43, 23, 43], [42, 23, 42]),
        matter::ClaimResult::Deferred
    ));
    let before = r.matter.remaining_mj;
    r.heat.melting.push(p);
    r.matter.melt_ground(&mut r.world, &mut r.heat);
    assert_eq!(r.matter.debris.claims(), 1);
    assert_eq!(r.heat.melted_total, 1);
    assert_eq!(
        r.matter.remaining_mj, before,
        "thermal runoff has no blast cost"
    );
    r.world.flush();
    r.matter.flush(&r.world, true, 98, 0, &[], &[]);
    assert!(!r.world.solid(p[0], p[1], p[2]));
    assert!(r.world.solid(43, 23, 43));
    assert!(r.matter.economy.ledger.balanced());
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
    let mut c = r.config;
    c.energy = 1;
    r.explosion = Some(Explosion::new(&r.world, [280, 75, 336], &c, &r.units));
    let captured = r.explosion.as_ref().unwrap().turret_pitches.clone();
    assert!(captured.iter().any(|p| *p != 0));
    assert!(r.battle_order(0, Order::Ground([600, 95, 380]), None));
    for _ in 0..5 {
        step(&mut r);
    }
    let bytes = r.snapshot();
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 52);
    let mut replay = BlastRange::restore(r.config, &bytes).unwrap();
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
    assert!(BlastRange::restore(r.config, &truncated).is_none());
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
