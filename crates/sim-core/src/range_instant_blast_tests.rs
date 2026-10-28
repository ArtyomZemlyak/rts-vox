//! A burst is fast (ADR-697): one canister salvo and one ordinary
//! shell finish their destruction, applied edits included, within a bound
//! of ticks on a reference layout.
use super::*;

/// Flat ground with the canister skirmish's two targets near the impact.
fn salvo_range() -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [
        (RangeUnitKind::ArtilleryT2, [140, 16, 320], true),
        (RangeUnitKind::Strider, [400, 16, 300], false),
        (RangeUnitKind::Tank, [450, 16, 360], false),
    ]
    .into_iter()
    .map(|(kind, at, allied)| modding::range::RangeUnitDef {
        weapon: None,
        kind,
        at,
        allied,
    })
    .collect();
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap(),
        layout,
    )
    .unwrap();
    r.start_battle().unwrap();
    for a in &mut r.battle.actors {
        a.order = Order::Hold;
        a.next = u32::MAX;
    }
    r
}

fn ground(r: &BlastRange, x: i32, z: i32) -> i32 {
    (0..r.size[1] as i32)
        .rev()
        .find(|&y| material(&r.world, [x, y, z]) != 0)
        .unwrap()
        + 1
}

/// Rounds one tick above the ground: a central one and a ring, as a
/// salvo's footprint lays them (`footprint_radius`).
fn land(r: &mut BlastRange, weapon: Weapon, centre: [i32; 2], count: usize) {
    let radius = r.battle.tuning.canister.footprint_radius;
    for k in 0..count {
        let [x, z] = if k == 0 {
            centre
        } else {
            let angle = (k - 1) as i32 * fx::angle::FULL / (count - 1) as i32;
            [
                centre[0]
                    + fx::angle::cos(angle)
                        .mul(Fx::from_int(radius))
                        .to_int_trunc(),
                centre[1]
                    + fx::angle::sin(angle)
                        .mul(Fx::from_int(radius))
                        .to_int_trunc(),
            ]
        };
        let y = ground(r, x, z) + 2;
        let pos = [Fx::from_int(x), Fx::from_int(y), Fx::from_int(z)];
        r.battle.rounds.push(Round {
            owner: 0,
            weapon,
            pos,
            previous: pos,
            velocity: [Fx::ZERO, Fx::from_int(-240), Fx::ZERO],
            distance: Fx::from_int(1000),
        });
    }
}

/// Ticks from the landing until no wave is live, the edit queue is empty
/// and no ground claim waits for its drain.
fn ticks_to_rest(r: &mut BlastRange, limit: u32) -> Option<u32> {
    r.step();
    assert!(r.battle.rounds.is_empty(), "rounds did not land");
    for t in 1..=limit {
        let [c, a] = r.battle.wave_counts();
        if c + a == 0 && r.world.queued() == 0 && r.matter.debris.claims() == 0 {
            return Some(t);
        }
        r.step();
    }
    None
}

#[test]
fn canister_salvo_destruction_lands_within_five_ticks() {
    let mut r = salvo_range();
    let before = r.removed;
    land(&mut r, Weapon::Canister, [420, 330], 6);
    let ticks = ticks_to_rest(&mut r, 5);
    assert!(ticks.is_some(), "salvo still working after 5 ticks");
    assert!(r.removed > before, "the salvo removed no ground");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn ordinary_shell_destruction_lands_within_five_ticks() {
    let mut r = salvo_range();
    let before = r.removed;
    land(&mut r, Weapon::Blast, [420, 330], 1);
    let ticks = ticks_to_rest(&mut r, 5);
    assert!(ticks.is_some(), "shell still working after 5 ticks");
    assert!(r.removed > before, "the shell removed no ground");
    assert!(r.matter.economy.ledger.balanced());
}

/// One authored firing event (a stream's complete burst), on the same flat
/// soil. Canister here is one submunition; the six-round salvo is separate.
fn soil_presets() -> [(&'static str, &'static str); 9] {
    [
        ("shell", include_str!("../../../data/explosion.ron")),
        (
            "canister_round",
            include_str!("../../../data/canister-shell.ron"),
        ),
        ("kinetic", include_str!("../../../data/kinetic-shell.ron")),
        ("energy", include_str!("../../../data/energy-cannon.ron")),
        ("energy_t2", include_str!("../../../data/energy-cannon.ron")),
        (
            "annihilation",
            include_str!("../../../data/anni-cannon.ron"),
        ),
        ("machine_gun", include_str!("../../../data/machine-gun.ron")),
        ("laser", include_str!("../../../data/laser.ron")),
        ("flame", include_str!("../../../data/flamethrower.ron")),
    ]
}

fn soil_config(name: &str, text: &str) -> ExplosionConfig {
    if name == "energy_t2" {
        salvo_range()
            .battle
            .shot_config(Weapon::Energy, RangeUnitKind::TurretT2)
    } else {
        ExplosionConfig::from_ron(text).unwrap()
    }
}

fn soil_trial(config: ExplosionConfig) -> BlastRange {
    let mut layout = salvo_range().layout.take().unwrap();
    layout.battle = false;
    layout.airburst_on_target = false;
    layout.units.clear();
    layout.target = [420, 15, 330];
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(4);
    r.control();
    r.aim = Some([420, 15, 330]);
    assert!(r.refire());
    r
}

fn soil_removed(r: &BlastRange) -> usize {
    r.removed_materials[1] + r.removed_materials[7] + r.removed_materials[8]
}

fn single_blast_latency(
    config: ExplosionConfig,
    mut step: impl FnMut(&mut BlastRange),
) -> (u32, usize) {
    let mut r = soil_trial(config);
    let mut impact = None;
    for t in 0..240 {
        step(&mut r);
        assert!(r.matter.economy.ledger.balanced());
        if r.impact.is_some() {
            let start = *impact.get_or_insert(t);
            if r.explosion.as_ref().is_some_and(|e| !e.working())
                && r.world.queued() == 0
                && r.matter.debris.claims() == 0
            {
                return (t - start + 1, soil_removed(&r));
            }
        }
    }
    panic!("blast did not finish");
}

#[test]
fn explosive_presets_at_one_two_and_four_power_finish_within_five_ticks() {
    for (name, text) in soil_presets().into_iter().filter(|(name, _)| {
        matches!(
            *name,
            "shell" | "canister_round" | "energy" | "annihilation"
        )
    }) {
        for power in [1, 2, 4] {
            let mut c = soil_config(name, text);
            c.energy *= power;
            c.material_energy_j *= power;
            let (ticks, removed) = single_blast_latency(c, BlastRange::step);
            assert!(removed > 0, "{name} x{power}");
            assert!(ticks <= 5, "{name} x{power}: {ticks} ticks");
        }
    }
}

#[test]
fn siege_shell_terrain_destruction_lands_within_five_ticks() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
    let (ticks, removed) = single_blast_latency(c, BlastRange::step);
    assert!(removed > 0, "T3 did not dig soil");
    assert!(ticks <= 5, "T3 destruction took {ticks} ticks");
    eprintln!("T3 terrain completion: ticks={ticks}, removed_soil={removed}");
}

#[test]
#[ignore = "latency and peak CPU measurement for blast pacing"]
fn probe_explosive_preset_latency() {
    for (name, text) in soil_presets().into_iter().filter(|(name, _)| {
        matches!(
            *name,
            "shell" | "canister_round" | "energy" | "annihilation"
        )
    }) {
        for power in [1, 2, 4] {
            let mut c = soil_config(name, text);
            c.energy *= power;
            c.material_energy_j *= power;
            if std::env::var_os("PROBE_OLD_PACE").is_some() {
                let material = c.material_energy_j > 0;
                c.wave_work_per_tick = if material { 32768 } else { 4096 };
                c.capture_cells_per_tick = if material { 49152 } else { 40000 };
                c.edits_per_tick = if material { 65536 } else { 8192 };
            }
            let mut peak = 0.0f64;
            let mut total = 0.0;
            let (ticks, removed) = single_blast_latency(c, |r| {
                let start = std::time::Instant::now();
                r.step();
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                peak = peak.max(ms);
                total += ms;
            });
            println!("{name} power={power} ticks={ticks} soil={removed} peak_ms={peak:.3} total_ms={total:.3}");
        }
    }
}

/// Empty cells of the fixture's original 16-cell soil slab, and the
/// deepest opening from its surface. Settled debris can refill a hole.
fn crater(r: &BlastRange) -> (usize, usize) {
    let mut empty = 0;
    let mut depth = 0;
    for z in 210..450 {
        for x in 300..540 {
            let mut opening = 0;
            for y in (0..16).rev() {
                if !r.world.solid(x, y, z) {
                    empty += 1;
                    if opening == 15 - y {
                        opening += 1;
                    }
                }
            }
            depth = depth.max(opening as usize);
        }
    }
    (empty, depth)
}

fn former_soil(r: &mut BlastRange) {
    for (mat, strength) in [(1, 30), (7, 30), (8, 40)] {
        r.tune_material(
            mat,
            super::super::material_tuning::MaterialField::Strength,
            strength,
        );
    }
}

#[test]
fn stock_weapons_soil_balance_and_replay() {
    for (name, text) in soil_presets() {
        let config = soil_config(name, text);
        let mut now = soil_trial(config);
        let mut before = soil_trial(config);
        former_soil(&mut before);
        let mut replay = None;
        for t in 0..240 {
            now.step();
            before.step();
            assert!(now.matter.economy.ledger.balanced(), "{name}");
            assert!(before.matter.economy.ledger.balanced(), "{name} reference");
            if let Some(copy) = replay.as_mut() {
                BlastRange::step(copy);
                assert_eq!(now.state_hash(), copy.state_hash(), "{name} tick={t}");
            } else if t == config.charge_ticks + 10 {
                replay = Some(BlastRange::restore(config, &now.snapshot()).unwrap());
            }
        }
        let (new, old) = (soil_removed(&now), soil_removed(&before));
        let (new_crater, old_crater) = (crater(&now), crater(&before));
        eprintln!("{name}: removed={old}->{new} empty_depth={old_crater:?}->{new_crater:?}");
        match name {
            "shell" | "canister_round" => {
                assert!(new > 0 && new * 3 < old, "{name}: {new} vs {old}");
                assert!(
                    new_crater.0 * 2 < old_crater.0,
                    "{name}: net crater did not shrink"
                );
            }
            "kinetic" | "machine_gun" => assert!(new > 0 && new <= old, "{name}: {new} vs {old}"),
            "laser" | "flame" => assert_eq!((new, old), (0, 0), "{name}"),
            _ => assert_eq!(new, old, "{name}: annihilation has no fracture budget"),
        }
    }
}

#[test]
fn canister_salvo_digs_less_soil_and_still_finishes() {
    let mut now = salvo_range();
    let mut before = salvo_range();
    former_soil(&mut before);
    for r in [&mut now, &mut before] {
        land(r, Weapon::Canister, [420, 330], 6);
        assert!(ticks_to_rest(r, 15).is_some());
        assert!(r.matter.economy.ledger.balanced());
    }
    assert!(soil_removed(&now) > 0);
    assert!(soil_removed(&now) * 3 < soil_removed(&before));
}

#[test]
#[ignore = "terrain balance and timing measurement"]
fn probe_all_weapon_soil() {
    for (name, text) in soil_presets() {
        let config = soil_config(name, text);
        let mut r = soil_trial(config);
        let start = std::time::Instant::now();
        for _ in 0..240 {
            r.step();
            assert!(r.matter.economy.ledger.balanced(), "{name}");
        }
        let soil = r.removed_materials[1] + r.removed_materials[7] + r.removed_materials[8];
        println!(
            "{name}: soil={soil} fragments={} pulverized={} ms={:.3} hash={:016x}",
            r.matter.debris.fragments.len(),
            r.matter.admission.pulverized,
            start.elapsed().as_secs_f64() * 1000.0,
            r.state_hash()
        );
    }
    let mut r = salvo_range();
    r.set_physics_workers(4);
    land(&mut r, Weapon::Canister, [420, 330], 6);
    let start = std::time::Instant::now();
    for _ in 0..240 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
    }
    println!(
        "canister_salvo: soil={} fragments={} pulverized={} ms={:.3} hash={:016x}",
        r.removed_materials[1] + r.removed_materials[7] + r.removed_materials[8],
        r.matter.debris.fragments.len(),
        r.matter.admission.pulverized,
        start.elapsed().as_secs_f64() * 1000.0,
        r.state_hash()
    );
}

/// The parallel parts of the wave phase (frontiers, chunk builds, matter
/// and body maps read ahead, captures side by side, sorts in runs) are
/// functions of each wave alone or have one answer: the same salvo on one
/// to four physics workers gives the same state every tick.
#[test]
fn salvo_state_is_independent_of_physics_workers() {
    let runs: Vec<Vec<u64>> = [1, 2, 3, 4]
        .into_iter()
        .map(|workers| {
            let mut r = salvo_range();
            assert!(r.set_physics_workers(workers));
            land(&mut r, Weapon::Canister, [420, 330], 6);
            (0..14)
                .map(|_| {
                    r.step();
                    r.state_hash()
                })
                .collect()
        })
        .collect();
    assert_eq!(runs[0], runs[1], "two workers changed the salvo");
    assert_eq!(runs[0], runs[2], "three workers changed the salvo");
    assert_eq!(runs[0], runs[3], "four workers changed the salvo");
}

/// A salvo saved while its waves still capture and dig, restored and run
/// on, matches the run that never stopped.
#[test]
fn salvo_restored_mid_wave_matches_the_continuous_run() {
    let mut a = salvo_range();
    land(&mut a, Weapon::Canister, [420, 330], 6);
    for _ in 0..1 {
        a.step();
    }
    let [capturing, applying] = a.battle.wave_counts();
    assert!(capturing + applying > 0, "no live wave to save");
    let mut b = BlastRange::restore(a.config, &a.snapshot()).expect("restore");
    assert_eq!(a.state_hash(), b.state_hash());
    for tick in 0..12 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash(), "tick {tick} after restore");
    }
    let [c, w] = a.battle.wave_counts();
    assert_eq!(c + w, 0, "the salvo did not finish after the restore");
}

/// The body map lists, for every wave cell of a field, exactly the cells
/// `wave_cells` lists for it, in the same order: on turned and pitched
/// hulls with segments, for each cell edge in use.
#[test]
fn body_map_lists_what_wave_cells_lists() {
    use crate::blast::wave_cells::Grid;
    let r = salvo_range();
    for (k, u) in r.units.iter().enumerate() {
        for (yaw, delta, pitch) in [(0, 0, 0), (5000, 9000, 0), (11000, -3000, 2500)] {
            let mut pose = crate::damage::turned(&u.pose, yaw, 0);
            let mut segs: Vec<_> = u
                .seg_frames
                .iter()
                .map(|f| crate::damage::turned(f, yaw, 0))
                .collect();
            if pitch != 0 {
                // Negative coordinates exercise floor division and the
                // half-open ownership boundary on every axis.
                let offset = phys::V3::new(Fx::from_int(512), Fx::from_int(64), Fx::from_int(512));
                pose.base = pose.base.sub(offset);
                for frame in &mut segs {
                    frame.base = frame.base.sub(offset);
                }
            }
            let frames = HitFrames::new(u, pose, &segs, delta, pitch);
            let terms = frames.wave_terms(u, &pose, &segs);
            let (lo, hi) = u.bounds_aimed(Some(&pose), &segs, delta, pitch);
            for edge in [1, 2, 3, 4, 9, 64] {
                let n = ((0..3).map(|a| hi[a] - lo[a]).max().unwrap() / edge + 4) as usize;
                let origin = lo.map(|v| v - edge * 2 + k as i32 % edge);
                let grid = Grid { origin, edge, n };
                let map = frames
                    .body_map(u, &pose, &segs, grid, body_sizes(u))
                    .expect("map");
                let mut listed = Vec::new();
                let mut total = 0;
                for i in 0..n * n * n {
                    let cell = [i % n, i / n % n, i / (n * n)].map(|v| v as i32);
                    let low = std::array::from_fn(|a| origin[a] + cell[a] * edge);
                    frames.wave_cells(u, &pose, &segs, low, edge, &mut listed);
                    let mut cached = Vec::new();
                    frames.visit_wave_cells_with_terms(
                        u,
                        &pose,
                        &segs,
                        low,
                        edge,
                        Some(&terms),
                        |contact| {
                            cached.push(contact);
                            true
                        },
                    );
                    assert_eq!(
                        cached, listed,
                        "cached terms: unit {k} yaw {yaw} edge {edge} cell {cell:?}"
                    );
                    if !listed.is_empty() {
                        cached.clear();
                        let limit = 1 + i % listed.len().min(4);
                        frames.visit_wave_cells_with_terms(
                            u,
                            &pose,
                            &segs,
                            low,
                            edge,
                            Some(&terms),
                            |contact| {
                                cached.push(contact);
                                cached.len() < limit
                            },
                        );
                        assert_eq!(cached, listed[..limit], "visitor stop prefix");
                    }
                    let (start, len) = map.range(grid.index(cell));
                    let mapped: Vec<_> = (start..start + len).map(|k| map.contact(k)).collect();
                    assert_eq!(
                        mapped, listed,
                        "unit {k} yaw {yaw} edge {edge} cell {cell:?}"
                    );
                    total += len;
                }
                assert!(total > 0, "unit {k}: the field holds no body cell");
            }
        }
    }
}

#[test]
#[ignore = "measurement probe for exact wave geometry"]
fn probe_wave_body_maps() {
    use crate::blast::wave_cells::Grid;
    use std::{hint::black_box, time::Instant};
    let r = salvo_range();
    for (id, u) in r.units.iter().enumerate() {
        let yaw = 5000;
        let pose = crate::damage::turned(&u.pose, yaw, 0);
        let segs: Vec<_> = u
            .seg_frames
            .iter()
            .map(|f| crate::damage::turned(f, yaw, 0))
            .collect();
        let frames = HitFrames::new(u, pose, &segs, 9000, 2500);
        let (lo, hi) = u.bounds_aimed(Some(&pose), &segs, 9000, 2500);
        for edge in [1, 2, 3, 4] {
            let n = ((0..3).map(|a| hi[a] - lo[a]).max().unwrap() / edge + 4) as usize;
            let grid = Grid {
                origin: lo.map(|v| v - 2 * edge),
                edge,
                n,
            };
            let mut times = Vec::new();
            let mut checksum = 0u64;
            let mut contacts = 0;
            for round in 0..12 {
                let start = Instant::now();
                let map = black_box(
                    frames
                        .body_map(u, &pose, &segs, grid, body_sizes(u))
                        .unwrap(),
                );
                let ns = start.elapsed().as_nanos();
                if round >= 4 {
                    times.push(ns);
                }
                if round == 0 {
                    for cell in 0..n * n * n {
                        let (start, len) = map.range(cell as u32);
                        contacts += len;
                        for k in start..start + len {
                            let (part, at) = map.contact(k);
                            for value in [cell, part, at[0], at[1], at[2]] {
                                checksum = checksum.wrapping_mul(31).wrapping_add(value as u64);
                            }
                        }
                    }
                }
            }
            times.sort_unstable();
            println!("body_map unit={id} edge={edge} n={n} contacts={contacts} mean_ns={} p50_ns={} checksum={checksum:016x}",times.iter().sum::<u128>()/times.len() as u128,times[times.len()/2]);
        }
    }
}

/// Measurement probe for analysis 251: per tick, live waves, heap pops,
/// contacts, claims, edits and wall time per phase. Timing lives here,
/// never in the simulation.
#[test]
#[ignore = "measurement probe: cargo test --release -- --ignored --nocapture"]
fn probe_salvo_timeline() {
    // `PROBE_TICKS=n` stops each run after n ticks and `PROBE_REPEAT=k`
    // runs the canister salvo k times: a profiler then sees the salvo's
    // own ticks rather than the pieces settling for seconds after it.
    let env = |name: &str, default: usize| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let (limit, repeat) = (env("PROBE_TICKS", 400), env("PROBE_REPEAT", 0));
    let runs: Vec<(&str, Weapon, usize)> = if repeat > 0 {
        vec![("canister", Weapon::Canister, 6); repeat]
    } else {
        vec![
            ("canister", Weapon::Canister, 6),
            ("shell", Weapon::Blast, 1),
        ]
    };
    for (name, weapon, count) in runs {
        let mut r = salvo_range();
        r.set_physics_workers(4);
        // Version 59+ also saves the constructor's weapon configuration.
        // Keep it equal in old-pace A/B runs, not just the battle's grants.
        r.config.wave_work_per_tick = env("PROBE_WORK", r.config.wave_work_per_tick);
        r.config.capture_cells_per_tick = env("PROBE_CAPTURE", r.config.capture_cells_per_tick);
        r.weapon_initial = r.config;
        for c in [&mut r.battle.canister_config, &mut r.battle.shell_config] {
            c.wave_work_per_tick = env("PROBE_WORK", c.wave_work_per_tick);
            c.capture_cells_per_tick = env("PROBE_CAPTURE", c.capture_cells_per_tick);
        }
        let tuning = std::sync::Arc::make_mut(&mut r.battle.tuning);
        tuning.wave_capture_ceiling = env("PROBE_CAPTURE_CEILING", tuning.wave_capture_ceiling);
        tuning.wave_work_ceiling = env("PROBE_WORK_CEILING", tuning.wave_work_ceiling);
        tuning.wave_contact_ceiling = env("PROBE_CONTACT_CEILING", tuning.wave_contact_ceiling);
        if std::env::var_os("PROBE_UNBOUND").is_some() {
            for c in [&mut r.battle.canister_config, &mut r.battle.shell_config] {
                c.wave_work_per_tick = 1 << 30;
                c.capture_cells_per_tick = 1 << 30;
            }
            r.world.max_edits_per_tick = 1 << 30;
            r.matter.tuning.max_claims = 1 << 30;
            r.matter.tuning.max_flying = 1 << 30;
            r.matter.tuning.max_fragments = 1 << 30;
        }
        land(&mut r, weapon, [420, 330], count);
        println!("== {name}");
        {
            let start = std::time::Instant::now();
            let mut built = 0;
            for x in 0..4 {
                for z in 0..4 {
                    let ci = r.world.chunk_of(320 + x * 32, 8, 256 + z * 32).unwrap();
                    built += r.world.build_chunk(ci).bytes();
                }
            }
            println!(
                "build_chunk 16 ground chunks: {:.2} ms ({built} bytes)",
                start.elapsed().as_secs_f64() * 1e3
            );
        }
        println!(
            "tick,ms,waves_capturing,waves_applying,examined,pending,resumes,ground_applied,no_room,claim_limit,pending_cell,body_applied,body_deferred,pulverized,held,claims,queued_edits,applied_edits,fragments,flying,removed,chunks,hash,phases"
        );
        let mut examined_before = 0usize;
        let mut ground_before = r.matter.admission.ground;
        let mut body_before = r.matter.admission.body;
        let mut pulverized_before = r.matter.admission.pulverized;
        let mut total = 0.0;
        let mut sums: std::collections::BTreeMap<&'static str, f64> = Default::default();
        for t in 0..limit {
            let mut phases: Vec<(&'static str, f64)> = Vec::new();
            let start = std::time::Instant::now();
            let mut last = start;
            r.step_observed(|p| {
                let now = std::time::Instant::now();
                phases.push((p, (now - last).as_secs_f64() * 1e3));
                last = now;
            });
            let ms = start.elapsed().as_secs_f64() * 1e3;
            total += ms;
            let progress: Vec<_> = r.battle.wave_progress().collect();
            let examined: usize = progress.iter().map(|p| p.heap_examined).sum();
            let pending: usize = progress.iter().map(|p| p.pending).sum();
            let resumes: usize = progress.iter().map(|p| p.queued_continuations).sum();
            let g = r.matter.admission.ground;
            let b = r.matter.admission.body;
            let [c, a] = r.battle.wave_counts();
            let pools = r.matter.debris.fragment_pools(&r.matter.tuning);
            for (p, ms) in &phases {
                *sums.entry(p).or_default() += ms;
            }
            let mut tick_sums: std::collections::BTreeMap<&'static str, f64> = Default::default();
            for (p, ms) in &phases {
                *tick_sums.entry(p).or_default() += ms;
            }
            let mut top: Vec<_> = tick_sums.into_iter().collect();
            top.sort_by(|x, y| y.1.total_cmp(&x.1));
            let top: Vec<String> = top
                .iter()
                .take(9)
                .map(|(p, ms)| format!("{p}={ms:.1}"))
                .collect();
            println!(
                "{t},{ms:.2},{c},{a},{},{pending},{resumes},{},{},{},{},{},{},{},{:?},{},{},{},{},{},{},{},{:016x},{}",
                examined.saturating_sub(examined_before),
                g[0] - ground_before[0],
                g[2] - ground_before[2],
                g[3] - ground_before[3],
                g[4] - ground_before[4],
                b[0] - body_before[0],
                b[2] - body_before[2],
                r.matter.admission.pulverized - pulverized_before,
                r.battle.wave_held,
                r.matter.debris.claims(),
                r.world.queued(),
                r.world.edits().len(),
                r.matter.debris.fragments.len(),
                pools.flying,
                r.removed,
                r.world.materialized,
                r.state_hash(),
                top.join(" ")
            );
            examined_before = if c + a == 0 { 0 } else { examined };
            ground_before = g;
            body_before = b;
            pulverized_before = r.matter.admission.pulverized;
            if t > 3
                && c + a == 0
                && r.world.queued() == 0
                && r.matter.debris.claims() == 0
                && pools.flying == 0
            {
                break;
            }
        }
        println!("{name} total_ms={total:.1}");
        let mut sums: Vec<_> = sums.into_iter().collect();
        sums.sort_by(|x, y| y.1.total_cmp(&x.1));
        for (p, ms) in sums.iter().take(14) {
            println!("{name} phase {p} {ms:.1}");
        }
    }
}

fn previous_ground_strength(r: &mut BlastRange) {
    for (material, value) in [(1, 120), (7, 120), (8, 160)] {
        r.tune_material(
            material,
            super::super::material_tuning::MaterialField::Strength,
            value,
        );
    }
}

fn deep_soil_trial(config: ExplosionConfig, level: i32) -> BlastRange {
    let mut layout = salvo_range().layout.take().unwrap();
    layout.battle = false;
    layout.airburst_on_target = false;
    layout.units.clear();
    layout.target = [420, 15, 330];
    let layout = layout.with_ground_level(level).unwrap();
    let target = layout.target;
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.set_physics_workers(4);
    r.control();
    r.aim = Some(target);
    assert!(r.refire());
    r
}

fn soil_shape(r: &BlastRange) -> (usize, u32) {
    let level = r.ground_level() as u32;
    let mut depth = 0;
    for z in 190..470 {
        for x in 280..560 {
            let mut y = level;
            while y > 0 && !r.world.solid(x, y - 1, z) {
                y -= 1;
            }
            depth = depth.max(level - y);
        }
    }
    (soil_removed(r), depth)
}

#[test]
fn shallow_soil_profiles_reduce_depth_and_volume_on_deep_ground_and_replay() {
    for text in [
        include_str!("../../../data/explosion.ron"),
        include_str!("../../../data/canister-shell.ron"),
        include_str!("../../../data/energy-cannon.ron"),
        include_str!("../../../data/siege-shell.ron"),
    ] {
        let config = ExplosionConfig::from_ron(text).unwrap();
        assert!(config.soil_depth_cost > 0);
        let mut legacy = config;
        legacy.soil_depth_cost = 0;
        legacy.soil_fracture_milli = 1000;
        let mut now = deep_soil_trial(config, 80);
        let mut before = deep_soil_trial(legacy, 80);
        previous_ground_strength(&mut before);
        let mut replay = None;
        for _ in 0..100 {
            now.step();
            before.step();
            assert!(now.matter.economy.ledger.balanced());
            if let Some(copy) = replay.as_mut() {
                BlastRange::step(copy);
                assert_eq!(now.state_hash(), copy.state_hash());
            } else if now.impact.is_some() {
                replay = Some(BlastRange::restore(config, &now.snapshot()).unwrap());
            }
            if now.impact.is_some()
                && now.explosion.as_ref().is_some_and(|e| !e.working())
                && before.explosion.as_ref().is_some_and(|e| !e.working())
                && now.world.queued() == 0
                && before.world.queued() == 0
            {
                break;
            }
        }
        let old = soil_shape(&before);
        let new = soil_shape(&now);
        eprintln!(
            "soil profile energy={} old={old:?} new={new:?}",
            config.energy
        );
        assert!(replay.is_some());
        assert!(new.0 > 0 && new.0 < old.0, "soil volume did not shrink");
        assert!(
            new.1 > 0 && new.1 < old.1,
            "crater did not become shallower"
        );
        assert!(new.1 * 2 <= old.1, "crater depth reduction is too small");
    }
}

#[test]
#[ignore = "manual isolated before/after depth and CPU comparison"]
fn probe_deep_soil_profiles() {
    for (name, text) in [
        ("shell", include_str!("../../../data/explosion.ron")),
        ("canister", include_str!("../../../data/canister-shell.ron")),
        ("energy", include_str!("../../../data/energy-cannon.ron")),
        ("siege", include_str!("../../../data/siege-shell.ron")),
    ] {
        for level in [16, 80] {
            for profile in [false, true] {
                let mut c = ExplosionConfig::from_ron(text).unwrap();
                if !profile {
                    c.soil_depth_cost = 0;
                    c.soil_fracture_milli = 1000;
                }
                let mut r = deep_soil_trial(c, level);
                if !profile {
                    previous_ground_strength(&mut r);
                }
                let mut peak = 0.0f64;
                let mut total = 0.0;
                let mut impact_ticks = 0;
                for _ in 0..120 {
                    let start = std::time::Instant::now();
                    let mut phases = Vec::new();
                    let mut last = start;
                    let jobs = std::sync::Mutex::new(Vec::new());
                    r.step_observed_all_tasks(
                        |p| {
                            let t = std::time::Instant::now();
                            phases.push((p, (t - last).as_secs_f64() * 1000.0));
                            last = t;
                        },
                        |_| {},
                        |_, _| {},
                        |_, _| {},
                        |stage, event| {
                            if stage == 6 {
                                jobs.lock()
                                    .unwrap()
                                    .push((event, std::time::Instant::now()));
                            }
                        },
                    );
                    let ms = start.elapsed().as_secs_f64() * 1000.0;
                    if ms > 40.0 {
                        let mut open = std::collections::BTreeMap::new();
                        let mut scopes = Vec::new();
                        for (event, t) in jobs.into_inner().unwrap() {
                            if let Some(name) = event.strip_suffix("_begin") {
                                open.insert(name, t);
                            }
                            if let Some(name) = event.strip_suffix("_end") {
                                if let Some(start) = open.remove(name) {
                                    scopes.push((name, (t - start).as_secs_f64() * 1000.0));
                                }
                            }
                        }
                        scopes.sort_by(|a, b| b.1.total_cmp(&a.1));
                        println!(
                            "SCOPES {name} level={level} profile={profile} {:?}",
                            &scopes[..scopes.len().min(8)]
                        );
                        phases.sort_by(|a, b| b.1.total_cmp(&a.1));
                        println!(
                            "SLOW {name} level={level} profile={profile} tick={} phases={:?}",
                            r.tick,
                            &phases[..phases.len().min(5)]
                        );
                    }
                    total += ms;
                    peak = peak.max(ms);
                    if r.impact.is_some() {
                        impact_ticks += 1;
                        if r.explosion.as_ref().is_some_and(|e| !e.working())
                            && r.world.queued() == 0
                        {
                            break;
                        }
                    }
                }
                println!("GROUND {name} level={level} profile={profile} shape={:?} impact_ticks={impact_ticks} peak_ms={peak:.3} total_ms={total:.3}",soil_shape(&r));
            }
        }
    }
}

#[test]
fn soil_work_profile_preserves_other_materials_hulls_and_ground_properties() {
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let body = voxel::unit::tank();
    let mut old = c;
    old.soil_depth_cost = 0;
    old.soil_fracture_milli = 1000;
    for mat in [0, 2, 3, 4, 5, 6] {
        assert_eq!(
            super::super::terrain_work(mat, 0, 16, &c),
            super::super::terrain_work(mat, 0, 16, &old)
        );
        assert_eq!(super::super::soil_price_milli(mat, &c), 1000);
    }
    assert_eq!(
        super::super::unit_work(&body, [10, 10, 10], &c),
        super::super::unit_work(&body, [10, 10, 10], &old)
    );
    let m = super::super::matter::Matter::new(&[]);
    for mat in [1, 7, 8] {
        assert_eq!(
            super::super::blast_ground_price(&m, &c, mat),
            4 * m.ground_price(mat)
        );
    }
    c.soil_depth_cost = u32::MAX;
    assert!(c.validate().is_err());
}

#[test]
fn confined_soil_cell_skip_matches_contacts_without_hiding_other_materials() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut w = World::new(64, 128, 64, 100, Box::new(FlatSoil));
    let mut e = Explosion::begin([32, 79, 32], &c, &[]);
    e.soil_plane_y = Some(79);
    let mut dents = SurfaceWork::new();
    let mut changed = Vec::new();
    assert!(super::super::silent_soil(&c, 100, &[1, 7, 8], 60, 79));
    assert!(!super::super::silent_soil(&c, 100, &[1, 2, 8], 60, 79));
    for y in 58..=60 {
        for mat in [1, 7, 8] {
            assert!(e.ground_contact_known(
                &mut w,
                [32, y, 32],
                mat,
                100,
                &mut dents,
                &mut changed,
                &c,
                None
            ));
        }
    }
    assert!(dents.is_empty() && changed.is_empty() && w.queued() == 0);
}
struct FlatSoil;
impl voxel::Generator for FlatSoil {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        u16::from(y < 80)
    }
}
