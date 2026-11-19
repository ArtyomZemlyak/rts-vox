//! T3 uses the shared ballistic, foundation, admission and matter services.
use super::*;

fn step_without_ai(r: &mut BlastRange) {
    r.battle.ai.next_tick = r.tick + r.battle.ai.tuning.decision_ticks;
    r.step();
}

#[test]
fn wave_task_observers_are_balanced_and_do_not_change_combat() {
    let mut plain = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    plain.start_battle().unwrap();
    plain.battle.actors[1].order = Order::Hold;
    assert!(plain.battle_order(0, Order::Ground([580, 15, 336]), None));
    let mut observed = BlastRange::restore(plain.config, &plain.snapshot()).unwrap();
    let counts = std::sync::Mutex::new(std::collections::BTreeMap::<&str, usize>::new());
    for _ in 0..240 {
        plain.battle.ai.next_tick = u32::MAX;
        observed.battle.ai.next_tick = u32::MAX;
        plain.step();
        observed.step_observed_all_tasks(
            |_| {},
            |_| {},
            |_, _| {},
            |_, _| {},
            |stage, event| {
                if event.starts_with("physics_wave_") {
                    assert_eq!(stage, 6);
                    *counts.lock().unwrap().entry(event).or_default() += 1;
                }
            },
        );
        assert_eq!(plain.state_hash(), observed.state_hash());
    }
    let counts = counts.into_inner().unwrap();
    for family in [
        "capture",
        "heap",
        "filter",
        "read_ground",
        "body_map",
        "apply",
    ] {
        let begin = format!("physics_wave_{family}_begin");
        let end = format!("physics_wave_{family}_end");
        let n = counts.get(begin.as_str()).copied().unwrap_or(0);
        assert!(n > 0, "unobserved family: {family}");
        assert_eq!(Some(&n), counts.get(end.as_str()), "unbalanced {family}");
    }
    assert!(plain.matter.economy.ledger.balanced());
}

#[test]
fn siege_single_shell_digs_soil_and_replays_flight_and_live_wave() {
    let mut r = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert_eq!(r.battle_weapon(0), Some(Weapon::Siege));
    assert_eq!(
        r.battle_attack_range(0),
        Some(modding::attack_ranges::stock().siege)
    );
    assert!(!r.battle_order(0, Order::Move, Some([220, 336])));
    assert!(!r.set_battle_weapon(0, Some(Weapon::Blast)));
    assert!(!r.set_battle_weapon(1, Some(Weapon::Siege)));
    let start = r.units[0].motion.pos;
    // Keep the shooter outside the now larger, physically damaging blast.
    assert!(r.battle_order(0, Order::Ground([580, 15, 336]), None));
    for _ in 0..180 {
        step_without_ai(&mut r);
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert_eq!(r.battle.actors[0].fired, 1);
    assert_eq!(
        r.battle.rounds.len(),
        1,
        "T3 must not produce a canister fan"
    );
    assert_eq!(r.battle.rounds[0].weapon, Weapon::Siege);
    assert_eq!(
        r.battle.actors[0].next,
        r.tick - 1 + r.battle.tuning.siege.reload_ticks
    );
    assert!(r.battle_order(0, Order::Hold, None));
    let mut replay = BlastRange::restore(r.config, &r.snapshot()).expect("T3 flight snapshot");
    let mut wave_replay = None;
    let mut soil = 0;
    let mut impacts = 0;
    let mut peak = 0;
    for _ in 0..240 {
        step_without_ai(&mut r);
        step_without_ai(&mut replay);
        assert_eq!(r.state_hash(), replay.state_hash());
        if let Some(copy) = wave_replay.as_mut() {
            step_without_ai(copy);
            assert_eq!(r.state_hash(), copy.state_hash());
        } else if !r.battle.waves.is_empty() {
            wave_replay = Some(BlastRange::restore(r.config, &r.snapshot()).expect("T3 live wave"));
        }
        for p in &r.battle.rounds {
            peak = peak.max(p.pos[1].to_int_trunc());
        }
        impacts += r
            .battle
            .impacts
            .iter()
            .filter(|p| p.weapon == Weapon::Siege)
            .count();
        soil += r
            .world
            .completed()
            .iter()
            .filter(|(edit, before)| edit.mat == 0 && matches!(before, 1 | 7 | 8))
            .count();
        assert_eq!(
            r.units[0].motion.pos, start,
            "foundation moved while firing"
        );
    }
    assert_eq!(impacts, 1);
    assert!(peak > 50, "no elevated ballistic arc");
    assert!(
        soil > 0 && wave_replay.is_some(),
        "no real terrain destruction"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn siege_full_range_survives_high_altitude_snapshot() {
    let mut l = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank])
        .layout
        .unwrap();
    let target_x = l.units[0].at[0] + modding::attack_ranges::stock().siege;
    l.size[0] = ((target_x + 300 + 15) / 16 * 16) as u32;
    l.clear_max[0] = target_x + 200;
    l.units[1].at[0] = target_x;
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Ground([target_x, 15, 336]), None));
    let mut replay = None;
    let mut hit = false;
    let flight_ticks = modding::attack_ranges::stock().siege
        * r.battle.tuning.siege.flight_time_milli
        * crate::TICKS_PER_SECOND
        / (r.battle.siege_config.shell_speed * 1000);
    for _ in 0..flight_ticks + 180 {
        step_without_ai(&mut r);
        if r.battle.actors[0].fired > 0 {
            r.battle.actors[0].order = Order::Hold;
        }
        if let Some(copy) = replay.as_mut() {
            step_without_ai(copy);
            assert_eq!(r.state_hash(), copy.state_hash());
        } else if r.battle.rounds.iter().any(|p| p.pos[1] > Fx::from_int(500)) {
            replay = Some(BlastRange::restore(r.config, &r.snapshot()).expect("high T3 shell"));
        }
        hit |= r
            .battle
            .impacts
            .iter()
            .any(|p| p.weapon == Weapon::Siege && (p.at[0] - target_x).abs() < 50);
        if hit {
            break;
        }
    }
    assert!(
        replay.is_some() && hit,
        "T3 did not reach its advertised range"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn siege_stationary_ai_uses_its_actual_weapon_reach() {
    let mut layout = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank])
        .layout
        .unwrap();
    // Target the added half of the weapon's reach, not a nearby tank that
    // would also be hittable with the former balance.
    let target_x = layout.units[0].at[0] + modding::attack_ranges::stock().siege * 4 / 5;
    layout.size[0] = ((target_x + 300 + 15) / 16 * 16) as u32;
    layout.clear_max[0] = target_x + 200;
    layout.units[1].at[0] = target_x;
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, layout).unwrap();
    assert!(r.set_ai_vs_ai(true));
    r.start_battle().unwrap();
    let obs = r.enemy_ai_observation().unwrap();
    assert!(!obs.units[0].mobile && !obs.units[0].can_reposition);
    assert!(obs.units[0]
        .attack_ranges
        .iter()
        .all(|&v| v == modding::attack_ranges::stock().siege));
    for _ in 0..240 {
        r.step();
        if r.battle.actors[0].fired > 0 {
            break;
        }
    }
    assert!(r.battle.actors[0].fired > 0, "AI never fired T3");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn siege_large_tilted_barrel_remains_hittable_beyond_old_eight_metre_cutoff() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::ArtilleryTurretT3, [200, 24, 200]);
    u.motion.hull.up = phys::V3::new(Fx::from_ratio(1, 4), Fx::ONE, Fx::ZERO);
    u.update_pose(0);
    let p = u.world_pos([111, 10, 28]);
    assert!(
        (p[0] - 200).abs() > 80,
        "fixture must expose old clipping: {p:?}"
    );
    assert!(u.solid(p), "large tilted barrel became non-colliding");
    let map =
        crate::blast::HitFrames::new(&u, u.pose, &u.seg_frames, u.turret_delta, u.turret_pitch);
    assert!(map.hit(&u, p).is_some(), "blast capture clipped the barrel");
}

#[test]
fn siege_trunnion_elevates_only_the_barrel_and_uses_its_real_muzzle() {
    let mut u = RangeUnit::of_kind(RangeUnitKind::ArtilleryTurretT3, [200, 16, 200]);
    assert!(u.articulated_siege() && u.barrel_connected(0));
    let deck = u.root_frame(14);
    let bearing = u.seg_frames[0].base;
    let flat = weapons::muzzles(&u, 0)[0];
    u.turret_pitch = fx::angle::FULL / 8;
    u.update_pose(0);
    assert_eq!(u.root_frame(14), deck, "charge chambers tilted with the barrel");
    assert_eq!(u.seg_frames[0].base, bearing, "trunnion translated");
    let muzzle = weapons::muzzles(&u, 0)[0];
    assert!(muzzle[1] > flat[1].add(Fx::from_int(20)));
    let physical = u.seg_frames[0].world(weapons::muzzle_cell(u.weapon_path(0).unwrap()));
    assert_eq!(muzzle, [physical.x, physical.y, physical.z]);
    let b = &u.body.segments[0].body;
    let mut hits = 0;
    for x in 25..35 { for y in 1..6 { for z in 3..13 {
        if !b.vol.get(x, y, z) { continue; }
        let p = u.seg_frames[0].world([x, y, z].map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
        if u.hit([p.x, p.y, p.z].map(|v|v.to_int_trunc())).is_some_and(|(part, _)|part == 1) { hits += 1; }
    }}}
    assert!(hits > 100, "elevated barrel is only a picture");
}

#[test]
fn siege_trunnion_severed_barrel_cannot_fire_and_damaged_pose_restores() {
    let mut r = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    r.start_battle().unwrap();
    r.battle.actors[0].turret.angle = fx::angle::FULL / 7;
    r.battle.actors[0].elevation.angle = fx::angle::FULL / 10;
    let u = &mut r.units[0];
    u.turret_pitch = fx::angle::FULL / 10;
    u.update_pose(fx::angle::FULL / 7);
    for y in 0..7 { for z in 0..16 {
        if u.body.segments[0].body.remove(20, y, z) { u.seg_removed.push((0, [20, y as u8, z as u8])); }
    }}
    assert!(!u.barrel_connected(0));
    assert!(weapons::muzzles(u, u.motion.drive.head + u.turret_delta).is_empty());
    r.matter.annihilate_removed(0, &r.units[0], 0, 0);
    r.units[0].update_response();
    assert!(!r.units[0].status.weapon && r.units[0].status.alive);
    let mut phase = String::new();
    let copy = BlastRange::restore_observed(r.config, &r.snapshot(), |p, offset, id| phase = format!("{p} {offset} {id:?}"))
        .unwrap_or_else(|| panic!("hinged damaged siege snapshot: {phase}"));
    assert_eq!(r.state_hash(), copy.state_hash());
    assert_eq!(r.units[0].seg_frames, copy.units[0].seg_frames);
    assert!(!copy.units[0].barrel_connected(0));
    assert!(!copy.units[0].status.weapon);
}

#[test]
fn siege_legacy_monolithic_snapshot_retains_its_voxel_ownership() {
    let mut r = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    let at = r.units[0].at;
    r.units[0] = RangeUnit::of_kind_model(RangeUnitKind::ArtilleryTurretT3, at, false);
    r.matter = crate::blast::matter::Matter::new(&r.units);
    r.combustion = crate::blast::fire::Combustion::new(&r.units, &mut r.matter);
    let mut phase = String::new();
    let copy = BlastRange::restore_observed(r.config, &r.snapshot(), |p, offset, id| phase = format!("{p} {offset} {id:?}"))
        .unwrap_or_else(|| panic!("legacy siege snapshot: {phase}"));
    assert!(!copy.units[0].articulated_siege());
    assert_eq!(r.state_hash(), copy.state_hash());
    if let Some(folder) = std::env::var_os("RTS_ARTILLERY_EVIDENCE") {
        r.start_battle().unwrap();
        r.battle.actors[1].order = Order::Hold;
        assert!(r.battle_order(0, Order::Ground([410, 16, 336]), None));
        for _ in 0..180 {
            if r.battle.actors[0].charge.is_some_and(|deadline| r.tick + 2 >= deadline) { break; }
            step_without_ai(&mut r);
        }
        assert!(r.battle.actors[0].charge.is_some());
        std::fs::write(std::path::Path::new(&folder).join("legacy.bin"), r.snapshot()).unwrap();
    }
}

#[test]
fn siege_native_charge_and_actual_shot_evidence() {
    let Some(folder) = std::env::var_os("RTS_ARTILLERY_EVIDENCE") else { return; };
    let folder = std::path::PathBuf::from(folder);
    std::fs::create_dir_all(&folder).unwrap();
    for (name, yaw) in [("rear", 0), ("left", fx::angle::FULL / 3), ("right", -fx::angle::FULL / 3)] {
        let mut layout = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]).layout.unwrap();
        layout.units[0].at = [320, 16, 320];
        layout.units[1].at = [560, 16, 560];
        let target = [320 + fx::angle::cos(yaw).mul(Fx::from_int(240)).to_int_trunc(),
            16, 320 + fx::angle::sin(yaw).mul(Fx::from_int(240)).to_int_trunc()];
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(c, layout).unwrap();
        r.start_battle().unwrap();
        r.battle.actors[1].order = Order::Hold;
        assert!(r.battle_order(0, Order::Ground(target), None));
        for _ in 0..240 {
            if r.battle.actors[0].charge.is_some_and(|deadline| r.tick + 2 >= deadline) { break; }
            step_without_ai(&mut r);
        }
        assert!(r.battle.actors[0].charge.is_some() && r.battle.actors[0].fired == 0);
        let copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        assert_eq!(copy.state_hash(), r.state_hash());
        std::fs::write(folder.join(format!("charge-{name}.bin")), r.snapshot()).unwrap();
        println!("SIEGE_EVIDENCE view={name} tick={} pitch={} head_pitch_y={} barrel_pitch_y={}",
            r.tick, r.units[0].turret_pitch, r.units[0].root_frame(14).x.y.raw(), r.units[0].seg_frames[0].x.y.raw());
        if name == "rear" {
            for _ in 0..12 { step_without_ai(&mut r); if r.battle.actors[0].fired > 0 { break; } }
            assert_eq!(r.battle.actors[0].fired, 1);
            assert!(r.battle.rounds[0].velocity[1] > Fx::ZERO);
            std::fs::write(folder.join("shot.bin"), r.snapshot()).unwrap();
        }
    }
}

#[test]
fn siege_trunnion_actual_laser_damage_is_conserved_and_replays() {
    let mut r = arena_of([RangeUnitKind::ArtilleryTurretT3, RangeUnitKind::Tank]);
    r.units[0].turret_pitch = fx::angle::FULL / 10;
    r.units[0].update_pose(0);
    let f = r.units[0].seg_frames[0];
    let p = f.world([Fx::from_int(25), Fx::from_int(3), Fx::from_int(8)]);
    let source = p.add(f.z.scale(Fx::from_int(40)));
    let target = p.sub(f.z.scale(Fx::from_int(40)));
    let tuning = crate::blast::motion::spider_tuning().laser.clone();
    let config = r.config;
    for _ in 0..20 {
        let trace = r.laser_ray([source.x, source.y, source.z], [target.x, target.y, target.z].map(Fx::to_int_trunc),
            None, &tuning, &config, (&mut |_| {}, &|_, _| {}));
        assert!(trace.hit);
    }
    assert!(r.thermals.hot() > 0);
    r.battle.actors[0].elevation.angle = fx::angle::FULL / 10;
    r.units[0].turret_pitch = fx::angle::FULL / 10;
    r.units[0].update_pose(0);
    let mut phase = String::new();
    let mut copy = BlastRange::restore_observed(r.config, &r.snapshot(), |p, offset, id|phase=format!("{p} {offset} {id:?}"))
        .unwrap_or_else(||panic!("laser-contact siege snapshot: {phase}"));
    for _ in 0..20 {
        step_without_ai(&mut r); step_without_ai(&mut copy);
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    assert!(!r.units[0].seg_removed.is_empty(), "raised barrel did not receive thermal damage");
    assert!(r.matter.economy.ledger.balanced());
    if let Some(folder) = std::env::var_os("RTS_ARTILLERY_EVIDENCE") {
        std::fs::write(std::path::Path::new(&folder).join("damage.bin"), r.snapshot()).unwrap();
    }
}
