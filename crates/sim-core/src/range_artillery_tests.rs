use super::artillery::{solve, velocity};
use super::*;
use crate::blast::{battle::Order, BlastRange};
use fx::angle;
use modding::range::{RangeLayout, RangeUnitDef};
use phys::V3;

#[path = "range_surface_blast_tests.rs"]
mod surface_blast_tests;

#[cfg(test)]
#[test]
fn artillery_impacts_remove_soil_in_manual_and_ai_combat() {
    for kind in [RangeUnitKind::Artillery, RangeUnitKind::ArtilleryT2] {
        for ai in [false, true] {
            let mut r = arena(kind);
            if ai {
                let l = r.layout.take().unwrap();
                r = BlastRange::with_layout(r.config, l).unwrap();
                assert!(r.set_ai_vs_ai(true));
                r.start_battle().unwrap();
                r.battle.ai.next_tick = r.tick;
            } else {
                assert!(r.battle_order(0, Order::Attack(1), None));
            }
            // This fixture pins partially spent wave snapshots. Stock first
            // salvo/AI crater opening is covered by the isolated tests below.
            super::tests::slow_blasts(&mut r);
            let mut soil = 0;
            let mut contacts = 0;
            let mut first = None;
            let mut replay = None;
            let mut holes = std::collections::BTreeSet::new();
            for _ in 0..600 {
                if ai {
                    r.step();
                } else {
                    step(&mut r);
                }
                if let Some(copy) = replay.as_mut() {
                    if ai {
                        BlastRange::step(copy);
                    } else {
                        step(copy);
                    }
                    assert_eq!(
                        r.state_hash(),
                        copy.state_hash(),
                        "{kind:?} ai={ai} damage replay"
                    );
                }
                for impact in &r.battle.impacts {
                    if matches!(impact.weapon, Weapon::Blast | Weapon::Canister) {
                        contacts += 1;
                        first.get_or_insert(impact.at);
                    }
                }
                soil += r
                    .world
                    .completed()
                    .iter()
                    .filter(|(edit, before)| {
                        let dug = edit.mat == 0 && matches!(before, 1 | 7 | 8);
                        if dug {
                            holes.insert([edit.x, edit.y, edit.z]);
                        }
                        dug
                    })
                    .count();
                if replay.is_none()
                    && r.matter.admission.ground[0] > 0
                    && r.battle.waves.iter().any(|w| {
                        w.wave.body_energy_mj.is_some_and(|left| {
                            left < u64::from(r.battle.config(w.weapon).material_energy_j)
                                * 1000
                                * u64::from(r.matter.tuning.body_energy_pct)
                                / 100
                        })
                    })
                {
                    let bytes = r.snapshot();
                    let mut copy =
                        BlastRange::restore(r.config, &bytes).expect("mid-crater snapshot");
                    super::tests::slow_blasts(&mut copy);
                    assert_eq!(bytes, copy.snapshot());
                    replay = Some(copy);
                }
            }
            eprintln!("{kind:?} ai={ai} fired={} contacts={contacts} first={first:?} soil={soil} ground={:?} waves={:?}", r.battle.actors[0].fired, r.matter.admission.ground, r.battle.wave_counts());
            assert!(r.battle.actors[0].fired > 0 && contacts > 0);
            assert!(
                soil > 0,
                "{kind:?} ai={ai} explosive impacts did not dig soil"
            );
            assert!(
                holes
                    .iter()
                    .any(|p| r.world.material(p[0], p[1], p[2]) == 0),
                "no actual crater remained"
            );
            assert!(
                replay.is_some(),
                "no saved partially spent body/terrain blast"
            );
            assert!(r.matter.economy.ledger.balanced());
        }
    }
}

#[cfg(test)]
#[test]
fn player_artillery_surface_blast_keeps_ground_energy_and_replays() {
    let mut l = arena(RangeUnitKind::Artillery).layout.unwrap();
    l.battle = false;
    l.gun_kind = RangeUnitKind::Artillery;
    l.gun = [170, 16, 336];
    l.target = [450, 24, 336];
    l.airburst_on_target = false;
    l.units.remove(0);
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    // A stock burst finishes within its impact tick (ADR-681); a slower
    // slice keeps this one live, so a snapshot holds its unspent energy.
    c.wave_work_per_tick = 256;
    let mut r = BlastRange::with_layout(c, l).unwrap();
    let mut replay = None;
    for _ in 0..600 {
        r.step();
        if let Some(copy) = replay.as_mut() {
            BlastRange::step(copy);
            assert_eq!(r.snapshot(), copy.snapshot());
        }
        if replay.is_none() && r.matter.admission.ground[0] > 0 && r.has_wave_energy() {
            let bytes = r.snapshot();
            assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 59);
            let copy = BlastRange::restore(c, &bytes).expect("player material blast snapshot");
            assert!(
                !copy.weapon_configured,
                "save promotion invented a weapon edit"
            );
            assert_eq!(bytes, copy.snapshot());
            for (offset, value) in [(10, 2), (bytes.len() - 9, 2)] {
                let mut corrupt = bytes.clone();
                corrupt[offset] = value;
                assert!(BlastRange::restore(c, &corrupt).is_none());
            }
            let mut corrupt = bytes.clone();
            let last = corrupt.len() - 8;
            corrupt[last..].copy_from_slice(&u64::MAX.to_le_bytes());
            assert!(
                BlastRange::restore(c, &corrupt).is_none(),
                "budget exceeded total energy"
            );
            replay = Some(copy);
        }
    }
    assert!(replay.is_some());
    assert!(r.matter.admission.body[0] > 0 && r.matter.admission.ground[0] > 0);
    assert!([1, 7, 8].iter().any(|&m| r.removed_materials[m] > 0));
    assert!(r.matter.spent_mj <= r.matter.supplied_mj);
    assert!(r.matter.economy.ledger.balanced());
}

#[cfg(test)]
#[test]
fn player_artillery_impact_digs_soil_with_stock_charge() {
    let mut l = arena(RangeUnitKind::Artillery).layout.unwrap();
    l.battle = false;
    l.gun_kind = RangeUnitKind::Artillery;
    l.gun = [170, 16, 336];
    l.target = [450, 15, 336];
    l.airburst_on_target = false;
    l.units.clear();
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    let mut soil = 0;
    for _ in 0..600 {
        r.step();
        soil += r
            .world
            .completed()
            .iter()
            .filter(|(edit, before)| edit.mat == 0 && matches!(before, 1 | 7 | 8))
            .count();
    }
    eprintln!(
        "player impact={:?} removed={} soil={soil} ground={:?}",
        r.impact, r.removed, r.matter.admission.ground
    );
    assert!(r.impact.is_some());
    assert!(soil > 0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn ai_artillery_crater_progresses_while_felling_waits_and_replays() {
    contended_ai_artillery_crater(RangeUnitKind::Artillery);
}

#[test]
fn ai_t2_artillery_crater_progresses_while_felling_waits_and_replays() {
    for kind in [RangeUnitKind::ArtilleryT2, RangeUnitKind::ArtilleryTurretT2] {
        contended_ai_artillery_crater(kind);
    }
}

fn contended_ai_artillery_crater(kind: RangeUnitKind) {
    use crate::blast::{battle::commands::Command, debris_tuning::DebrisField};
    let mut layout = arena(kind).layout.unwrap();
    layout.units[0].allied = false;
    layout.units[1].allied = true;
    layout.blocks.push(modding::range::RangeBlock {
        min: [250, 16, 250],
        max: [300, 64, 300],
        material: 5,
    });
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.execute_command(Command::TuneDebris {
        field: DebrisField::MaxFlying,
        value: 128,
    });
    // This fixture tests two queues competing with a partially spent wave.
    // Fewer metal pieces can let the stock T1 wave finish in the same tick
    // as its first soil edits, before the competing cut can be seeded.
    // Stock first-salvo progress is covered by the isolated tests below.
    super::tests::slow_blasts(&mut r);
    r.start_battle().unwrap();
    assert!(r.battle_order(1, Order::Hold, None));
    let mut soil = 0;
    let mut wood = 0;
    let mut surface = std::collections::BTreeSet::new();
    let mut replay = None;
    let mut seeded_felling = false;
    for _ in 0..600 {
        r.step();
        if let Some(copy) = replay.as_mut() {
            BlastRange::step(copy);
            assert_eq!(r.state_hash(), copy.state_hash(), "contended crater replay");
        }
        for &(edit, before) in r.world.completed() {
            if edit.mat != 0 {
                continue;
            }
            if matches!(before, 1 | 7 | 8) {
                soil += 1;
                if edit.y == 15 {
                    surface.insert([edit.x, edit.y, edit.z]);
                }
            } else if before == 5 {
                wood += 1;
            }
        }
        assert!(r.matter.debris.fragment_pools(&r.matter.tuning).flying <= 128);
        assert!(r.matter.economy.ledger.balanced());
        // Start a real competing tree cut while the crater is active. The
        // former tick-zero cut can now finish before the first shell arrives;
        // stale frontier entries must not be what makes this fixture contend.
        if !seeded_felling && soil > 0 && r.battle.has_material_work() {
            assert_eq!(r.world.material(260, 16, 260), 5);
            let cut = r.matter.fell(
                &mut r.world,
                &[[260, 16, 260]],
                [260, 16, 260],
                &r.navigation.fell,
            );
            assert!(cut == 1 || r.matter.felling_pending() > 0);
            seeded_felling = true;
        }
        if replay.is_none()
            && soil > 0
            && r.matter.felling_pending() > 0
            && r.battle.has_material_work()
        {
            let bytes = r.snapshot();
            let mut copy = BlastRange::restore(config, &bytes).expect("contended crater snapshot");
            super::tests::slow_blasts(&mut copy);
            assert_eq!(bytes, copy.snapshot());
            replay = Some(copy);
        }
    }
    eprintln!(
        "AI contested {kind:?} fired={} soil={soil} wood={wood} surface={} ground={:?} felling={} seeded={seeded_felling} replay={}",
        r.battle.actors[0].fired,
        surface.len(),
        r.matter.admission.ground,
        r.matter.felling_pending(),
        replay.is_some()
    );
    assert!(r.battle.actors[0].fired > 0);
    assert_eq!(r.battle.actors[1].fired, 0);
    assert!(soil > 0 && wood > 0, "both competing queues must progress");
    assert!(
        surface
            .iter()
            .any(|p| r.world.material(p[0], p[1], p[2]) == 0),
        "no surface crater"
    );
    assert!(replay.is_some());
    assert!(seeded_felling);
}

#[test]
fn isolated_first_artillery_salvo_opens_surface_soil() {
    for kind in [
        RangeUnitKind::Artillery,
        RangeUnitKind::ArtilleryT2,
        RangeUnitKind::ArtilleryTurretT2,
    ] {
        for target in [RangeUnitKind::Tank, RangeUnitKind::Base] {
            let mut layout = arena(kind).layout.unwrap();
            layout.units[0].allied = false;
            layout.units[1].allied = true;
            layout.units[1].kind = target;
            let config =
                ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
            let mut r = BlastRange::with_layout(config, layout).unwrap();
            r.start_battle().unwrap();
            assert!(r.battle_order(1, Order::Hold, None));
            let mut first = None;
            let mut soil = 0;
            let mut surface = std::collections::BTreeSet::new();
            for _ in 0..300 {
                if r.battle.actors[0].fired > 0 {
                    r.battle.actors[0].order = Order::Hold;
                    step(&mut r);
                } else {
                    r.step();
                }
                for impact in &r.battle.impacts {
                    first.get_or_insert((r.tick, impact.at));
                }
                for &(edit, before) in r.world.completed() {
                    if edit.mat == 0 && matches!(before, 1 | 7 | 8) {
                        soil += 1;
                        if edit.y == 15 {
                            surface.insert([edit.x, edit.y, edit.z]);
                        }
                    }
                }
            }
            assert_eq!(r.battle.actors[0].fired, 1, "only the first salvo may dig");
            eprintln!("isolated {kind:?} -> {target:?}: first={first:?} fired={} soil={soil} surface={} ground={:?} waves={:?}", r.battle.actors[0].fired, surface.len(), r.matter.admission.ground, r.battle.wave_counts());
            assert_eq!(r.battle.actors[1].fired, 0);
            assert!(first.is_some(), "{kind:?} -> {target:?}: no impact");
            assert!(
                surface
                    .iter()
                    .any(|p| r.world.material(p[0], p[1], p[2]) == 0),
                "{kind:?} -> {target:?}: first salvo left no surface crater"
            );
            assert!(r.matter.economy.ledger.balanced());
        }
    }
}

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
        RangeUnitKind::ArtilleryTurretT3,
    ] {
        let mut r = arena(kind);
        let weapon = if kind == RangeUnitKind::ArtilleryTurretT3 {
            Weapon::Siege
        } else if kind.canister() {
            Weapon::Canister
        } else {
            Weapon::Blast
        };
        let config = r.battle.config(weapon);
        let milli = super::loft(&r.battle.tuning, weapon);
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
        RangeUnitKind::ArtilleryTurretT3,
    ] {
        let mut r = arena(kind);
        assert!(r.battle_order(0, Order::Ground([450, 100, 400]), None));
        step(&mut r);
        assert_ne!(r.units[0].turret_pitch, 0);
        assert_eq!(r.battle.actors[0].fired, 0, "fired before elevation");
        let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
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
                let config = r.battle.config(p.weapon);
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
                replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
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
            let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
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
        let restored = BlastRange::restore(r.config, &old).unwrap();
        assert_eq!(restored.units[0].turret_pitch, 0);
        assert_eq!(
            restored.battle.actors[0].elevation,
            crate::turret::Traverse::new(0)
        );
        let mut invalid = r;
        invalid.units[0].turret_pitch = angle::QUARTER + 1;
        assert!(BlastRange::restore(invalid.config, &invalid.snapshot()).is_none());
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
        assert!(BlastRange::restore(r.config, &r.snapshot()).is_some());
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
            let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
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
