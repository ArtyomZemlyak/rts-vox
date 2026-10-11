//! A burst is instantaneous (ADR-681): one canister salvo and one ordinary
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
fn canister_salvo_destruction_lands_within_fifteen_ticks() {
    let mut r = salvo_range();
    let before = r.removed;
    land(&mut r, Weapon::Canister, [420, 330], 6);
    let ticks = ticks_to_rest(&mut r, 15);
    assert!(ticks.is_some(), "salvo still working after 15 ticks");
    assert!(r.removed > before, "the salvo removed no ground");
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn ordinary_shell_destruction_lands_within_fifteen_ticks() {
    let mut r = salvo_range();
    let before = r.removed;
    land(&mut r, Weapon::Blast, [420, 330], 1);
    let ticks = ticks_to_rest(&mut r, 15);
    assert!(ticks.is_some(), "shell still working after 15 ticks");
    assert!(r.removed > before, "the shell removed no ground");
    assert!(r.matter.economy.ledger.balanced());
}

/// The parallel parts of the wave phase (frontiers, chunk builds, matter
/// read ahead, captures) are functions of each wave alone: the same salvo
/// on one, two and four physics workers gives the same state every tick.
#[test]
fn salvo_state_is_independent_of_physics_workers() {
    let runs: Vec<Vec<u64>> = [1, 2, 4]
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
    assert_eq!(runs[0], runs[2], "four workers changed the salvo");
}

/// A salvo saved while its waves still capture and dig, restored and run
/// on, matches the run that never stopped.
#[test]
fn salvo_restored_mid_wave_matches_the_continuous_run() {
    let mut a = salvo_range();
    land(&mut a, Weapon::Canister, [420, 330], 6);
    for _ in 0..4 {
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

/// Measurement probe for analysis 251: per tick, live waves, heap pops,
/// contacts, claims, edits and wall time per phase. Timing lives here,
/// never in the simulation.
#[test]
#[ignore = "measurement probe: cargo test --release -- --ignored --nocapture"]
fn probe_salvo_timeline() {
    for (name, weapon, count) in [
        ("canister", Weapon::Canister, 6),
        ("shell", Weapon::Blast, 1),
    ] {
        let mut r = salvo_range();
        r.set_physics_workers(4);
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
            "tick,ms,waves_capturing,waves_applying,examined,pending,resumes,ground_applied,no_room,claim_limit,pending_cell,body_applied,body_deferred,pulverized,held,claims,queued_edits,applied_edits,fragments,flying,removed,chunks,phases"
        );
        let mut examined_before = 0usize;
        let mut ground_before = r.matter.admission.ground;
        let mut body_before = r.matter.admission.body;
        let mut pulverized_before = r.matter.admission.pulverized;
        let mut total = 0.0;
        let mut sums: std::collections::BTreeMap<&'static str, f64> = Default::default();
        for t in 0..400 {
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
                "{t},{ms:.2},{c},{a},{},{pending},{resumes},{},{},{},{},{},{},{},{:?},{},{},{},{},{},{},{},{}",
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
