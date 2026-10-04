//! Bounded battle load, including real damage, concurrent waves and bookkeeping.
use sim_core::blast::battle::{Order, Phase, Weapon};
use sim_core::blast::BlastRange;
use std::io::Write;
use std::time::Instant;

#[path = "bench_battle/workers.rs"]
mod workers;

#[derive(Default)]
struct TickProfile {
    tick: u32,
    total_us: u128,
    motion_us: u128,
    route_us: u128,
    commit_us: u128,
    feet_us: u128,
    contact_us: u128,
    felling_us: u128,
    debris_us: u128,
    battle_ai_us: u128,
    battle_orders_us: u128,
    battle_pose_us: u128,
    battle_los_us: u128,
    battle_laser_us: u128,
    battle_beam_us: u128,
    battle_halo_us: u128,
    rubble_us: u128,
    heat_flush_us: u128,
    collapse_us: u128,
    collapse_work: sim_core::blast::CollapseWork,
    searching: usize,
    moving: usize,
    blocked: usize,
    fragments: usize,
    flying: usize,
    chunks: usize,
    felling: usize,
}

impl TickProfile {
    fn record_phase(&mut self, name: &str, us: u128) {
        match name {
            "motion" => self.motion_us = us,
            "debris" => self.debris_us = us,
            "battle_ai" => self.battle_ai_us = us,
            "orders_pose" => {
                self.battle_pose_us += us;
                self.battle_orders_us += us;
            }
            "orders_los" => {
                self.battle_los_us += us;
                self.battle_orders_us += us;
            }
            "orders_laser" => {
                self.battle_laser_us += us;
                self.battle_orders_us += us;
            }
            "orders_laser_setup" | "orders_laser_query" | "orders_laser_beam" => {
                self.battle_beam_us += us;
                self.battle_laser_us += us;
                self.battle_orders_us += us;
            }
            "orders_laser_halo" => {
                self.battle_halo_us += us;
                self.battle_laser_us += us;
                self.battle_orders_us += us;
            }
            name if name.starts_with("orders_") || name == "battle_orders" => {
                self.battle_orders_us += us
            }
            "rubble" => self.rubble_us = us,
            "heat_flush" => self.heat_flush_us = us,
            name if name == "collapse" || name.starts_with("collapse_") => self.collapse_us += us,
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if let Some(mode) = args.windows(2).find(|w| w[0] == "--collapse-stress") {
        collapse_stress(&mode[1]);
        return;
    }
    println!("physics_workers={}", rayon::current_num_threads());
    let assault = args.iter().any(|a| a == "--assault");
    let siege = args
        .windows(2)
        .find(|w| w[0] == "--siege")
        .map(|w| w[1].as_str());
    let replay = args.iter().any(|a| a == "--replay");
    let world_battle = assault || siege.is_some() || args.iter().any(|a| a == "--world");
    let scenario = args
        .windows(2)
        .find(|w| w[0] == "--scenario")
        .map(|w| w[1].as_str());
    assert!(
        !world_battle || scenario.is_none(),
        "--scenario and --world/assault/siege are exclusive"
    );
    let progress = args.iter().any(|a| a == "--progress");
    let trace = args.iter().any(|a| a == "--trace");
    let damage_trace = args.iter().any(|a| a == "--damage-trace");
    let profile_workers = args.iter().any(|a| a == "--profile-workers");
    let tick_csv = args
        .windows(2)
        .find(|w| w[0] == "--tick-csv")
        .map(|w| w[1].as_str());
    let workers = args
        .windows(2)
        .find(|w| w[0] == "--workers")
        .map(|w| w[1].parse::<usize>().expect("workers"));
    let order_trace_tick = args
        .windows(2)
        .find(|w| w[0] == "--order-trace-tick")
        .map(|w| w[1].parse::<usize>().expect("order trace tick"));
    let focus = args
        .windows(2)
        .find(|w| w[0] == "--focus-unit")
        .map(|w| w[1].parse::<usize>().expect("focus unit"));
    let focus_from = args
        .windows(2)
        .find(|w| w[0] == "--focus-from")
        .map_or(0, |w| w[1].parse::<u32>().expect("focus from"));
    let focus_every = args
        .windows(2)
        .find(|w| w[0] == "--focus-every")
        .map_or(30, |w| w[1].parse::<u32>().expect("focus every"));
    assert!(focus_every > 0);
    let expect_phase =
        args.windows(2)
            .find(|w| w[0] == "--expect-phase")
            .map(|w| match w[1].as_str() {
                "victory" => Phase::Victory,
                "defeat" => Phase::Defeat,
                "draw" => Phase::Draw,
                _ => panic!("--expect-phase: victory, defeat or draw"),
            });
    let ai_vs_ai = args.iter().any(|a| a == "--ai-vs-ai");
    let balance = args
        .windows(2)
        .find(|w| w[0] == "--balance")
        .map_or("even", |w| w[1].as_str());
    assert!(
        !ai_vs_ai || world_battle || scenario.is_some(),
        "--ai-vs-ai requires --world, --assault, --siege or --scenario"
    );
    let until_end = assault
        || siege.is_some()
        || expect_phase.is_some()
        || args.iter().any(|a| a == "--until-end");
    assert!(
        !assault || siege.is_none(),
        "--assault and --siege are exclusive"
    );
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-km.ron"))
            .unwrap();
    let t =
        modding::range_battle::Tuning::from_ron(include_str!("../../../../data/range-battle.ron"))
            .unwrap();
    layout.battle = true;
    if let Some(c) = &mut layout.course {
        c.patrols.clear();
    }
    // `--units N`: N/4 groups of the stock four; each pair of groups takes
    // a column, the stock one, one further out and one further in;
    // `--eight` is `--units 8`.
    if world_battle {
        assert!(
            !args.iter().any(|a| a == "--units" || a == "--eight"),
            "--world uses its authored roster"
        );
        let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
            "../../../../data/battle-world.ron"
        ))
        .unwrap();
        layout.units = preset.units;
        layout.blocks.extend(preset.blocks);
        let omitted: &[usize] = match balance {
            "even" => &[],
            // Keep the full authored battle and remove only a few mobile
            // reinforcements, so balance runs still exercise the large map.
            "blue-light" => &[1, 4, 5, 15, 19, 21],
            "red-light" => &[7, 10, 11, 37, 41, 45],
            "blue-front-light" => &[4, 5, 21],
            "red-front-light" => &[10, 11, 45],
            _ => {
                panic!("--balance: even, blue-light, red-light, blue-front-light, red-front-light")
            }
        };
        if !omitted.is_empty() {
            layout.units = layout
                .units
                .into_iter()
                .enumerate()
                .filter_map(|(id, unit)| (!omitted.contains(&id)).then_some(unit))
                .collect();
        }
        if assault {
            assert_eq!(balance, "even", "--assault uses its fixed roster");
            // Isolate a real cross-map route and base kill from the large
            // firefight. Keep authored positions and the unchanged terrain.
            layout.units = [0, 4, 6].map(|id| layout.units[id].clone()).into();
        } else if let Some(side) = siege {
            assert_eq!(balance, "even", "--siege uses its fixed roster");
            let ids = match side {
                "blue" => [0, 2, 6],
                "red" => [0, 6, 8],
                _ => panic!("--siege: blue or red"),
            };
            layout.units = ids.map(|id| layout.units[id].clone()).into();
        }
    } else if let Some(name) = scenario {
        assert_eq!(balance, "even", "--balance requires --world");
        assert!(
            !args.iter().any(|a| a == "--units" || a == "--eight"),
            "--scenario uses its authored roster"
        );
        let ron = match name {
            "spider" => include_str!("../../../../data/skirmish-spider.ron"),
            "turrets" => include_str!("../../../../data/skirmish-turrets.ron"),
            "pincer" => include_str!("../../../../data/skirmish-pincer.ron"),
            "breakthrough" => include_str!("../../../../data/skirmish-breakthrough.ron"),
            _ => panic!("--scenario: spider, turrets, pincer or breakthrough"),
        };
        let preset = modding::range_skirmish::Skirmish::from_ron(ron).unwrap();
        layout.gun = preset.zone.spawn;
        layout.target = preset.zone.target;
        layout.zones.push(preset.zone);
        layout.units = preset.units;
        layout.blocks.extend(preset.blocks);
    } else {
        assert_eq!(balance, "even", "--balance requires --world");
        layout.gun = t.zone.spawn;
        layout.target = t.zone.target;
        layout.zones.push(t.zone);
        layout.units = t.units;
        let units = args
            .windows(2)
            .find(|w| w[0] == "--units")
            .map(|w| w[1].parse::<usize>().expect("units"))
            .unwrap_or(if args.iter().any(|a| a == "--eight") {
                8
            } else {
                4
            });
        assert!(
            units % 4 == 0 && (4..=24.min(modding::range::MAX_RANGE_UNITS)).contains(&units),
            "units: 4, 8 .. the layout's limit, at most 24"
        );
        let stock = layout.units.clone();
        layout.units = (0..units / 4)
            .flat_map(|k| {
                let (dx, dz) = ([0, 70, -70][k / 2], 90 * (k as i32 % 2));
                stock.iter().cloned().map(move |mut u| {
                    u.at[0] += if u.allied { -dx } else { dx };
                    u.at[2] += dz;
                    u
                })
            })
            .collect();
    }
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let mut sim = BlastRange::with_layout(config, layout).unwrap_or_else(|e| panic!("layout: {e}"));
    if ai_vs_ai || siege.is_some() {
        assert!(sim.set_ai_vs_ai(true));
    }
    sim.start_battle().unwrap();
    if let Some(workers) = workers {
        assert!(sim.set_navigation_workers(workers), "workers: 1..=8");
    }
    if let Some(path) = args
        .windows(2)
        .find(|w| w[0] == "--restore-file")
        .map(|w| &w[1])
    {
        let bytes = std::fs::read(path).expect("read snapshot");
        let restored = restore_snapshot(sim.config.clone(), &bytes);
        println!(
            "restored tick={} hash={:016x}",
            restored.tick,
            restored.state_hash()
        );
        return;
    }
    if args.iter().any(|a| a == "--group-order-bench") {
        assert!(world_battle, "--group-order-bench requires --world");
        let ids: Vec<_> = sim
            .units
            .iter()
            .enumerate()
            .filter(|(i, u)| sim.allied(*i) && !u.kind.emplacement())
            .map(|(i, _)| i)
            .collect();
        let columns = (1..=ids.len()).find(|n| n * n >= ids.len()).unwrap() as i32;
        let rows = (ids.len() as i32 + columns - 1) / columns;
        let destinations: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(slot, &id)| {
                let (col, row) = (slot as i32 % columns, slot as i32 / columns);
                (
                    id,
                    [
                        5000 + (2 * col - columns + 1) * 45,
                        5000 + (2 * row - rows + 1) * 45,
                    ],
                )
            })
            .collect();
        let no_group = args.iter().any(|a| a == "--no-group");
        let now = Instant::now();
        let accepted = if no_group {
            0
        } else {
            sim.battle_group_move(&destinations, [5000, 5000])
        };
        println!(
            "group_move_us={} mobile={} accepted={accepted}",
            now.elapsed().as_micros(),
            ids.len()
        );
        let mut ticks = Vec::with_capacity(ids.len() + 1);
        let mut phases = std::collections::BTreeMap::<&str, Vec<u128>>::new();
        for _ in 0..=ids.len() {
            let now = Instant::now();
            let mut last = now;
            sim.step_observed(|phase| {
                let at = Instant::now();
                phases
                    .entry(phase)
                    .or_default()
                    .push(at.duration_since(last).as_micros());
                last = at;
            });
            ticks.push(now.elapsed().as_micros());
        }
        ticks.sort_unstable();
        println!(
            "group_first_ticks_us_median={} p95={} max={}",
            ticks[ticks.len() / 2],
            ticks[ticks.len() * 95 / 100],
            ticks[ticks.len() - 1]
        );
        let mut phase_p95: Vec<_> = phases
            .into_iter()
            .map(|(name, mut samples)| {
                samples.sort_unstable();
                (name, samples[samples.len() * 95 / 100])
            })
            .collect();
        phase_p95.sort_unstable_by_key(|&(_, us)| std::cmp::Reverse(us));
        println!("group_first_ticks_phase_p95_us={phase_p95:?}");
        return;
    }
    if assault {
        assert!(sim.set_battle_weapon(1, Some(Weapon::Annihilation)));
        assert!(sim.battle_order(1, Order::Attack(2), None));
    }
    let mut samples = Vec::new();
    let mut tick_profiles = Vec::new();
    let mut peak_rounds = 0;
    let mut assault_shots = 0;
    let mut assault_weapon = true;
    let mut phases = std::collections::BTreeMap::<&str, Vec<u128>>::new();
    let mut impact_bins = std::collections::BTreeMap::<(u8, i32, i32), u32>::new();
    let ticks = args
        .windows(2)
        .find(|w| w[0] == "--ticks")
        .map_or(900, |w| w[1].parse::<usize>().expect("ticks"));
    assert!(ticks > 0);
    let snapshot_tick = args
        .windows(2)
        .find(|w| w[0] == "--snapshot-tick")
        .map_or(300, |w| w[1].parse::<u32>().expect("snapshot tick"));
    assert!(
        !replay || snapshot_tick > 0,
        "snapshot tick must be positive"
    );
    assert!(
        !replay || ticks >= snapshot_tick as usize + 300,
        "replay requires 300 ticks after the snapshot"
    );
    let mut restored = None;
    let mut unit_time_windows = vec![vec![0u128; sim.units.len()]; ticks.div_ceil(300)];
    let mut unit_phase_windows = vec![vec![[0u128; 6]; sim.units.len()]; ticks.div_ceil(300)];
    let mut debris_phase_windows = vec![[0u128; 16]; ticks.div_ceil(300)];
    let mut settled_phase_windows = vec![[0u128; 6]; ticks.div_ceil(300)];
    let mut heat_phase_windows = vec![[0u128; 8]; ticks.div_ceil(300)];
    let mut local_hunt_windows = vec![[0usize; 2]; ticks.div_ceil(300)];
    let mut ground_windows = vec![(0u128, [0usize; 2]); ticks.div_ceil(300)];
    let mut unit_start = vec![Instant::now(); sim.units.len()];
    let mut unit_last = unit_start.clone();
    let worker_profile = profile_workers.then(|| workers::Profile::new(sim.units.len()));
    let mut worker_windows: Vec<workers::Window> = if profile_workers {
        (0..ticks.div_ceil(300))
            .map(|_| workers::Window::default())
            .collect()
    } else {
        Vec::new()
    };
    let mut admission_previous = sim.matter.admission;
    let mut damage_previous = [0u64; 2];
    let mut shots_previous = [0u64; 2];
    let mut impact_window = [0usize; 6];
    let mut laser_contacts = 0usize;
    for tick_index in 0..ticks {
        let start = Instant::now();
        let mut last = start;
        let mut detail_last = start;
        let mut motion_detail = [0u128; 4];
        let mut ground_us = 0;
        let mut feet_us = 0;
        let mut debris_detail = [0u128; 16];
        let mut settled_detail = [0u128; 6];
        let mut settled_last = None;
        let mut heat_detail = [0u128; 8];
        let mut profile = TickProfile::default();
        let unit_window = &mut unit_time_windows[tick_index / 300];
        let unit_phases = &mut unit_phase_windows[tick_index / 300];
        let local_hunts = &mut local_hunt_windows[tick_index / 300];
        let ground_counts = &mut ground_windows[tick_index / 300].1;
        sim.step_observed_tasks(
            |phase| {
                let now = Instant::now();
                let elapsed = now.duration_since(last).as_micros();
                last = now;
                phases.entry(phase).or_default().push(elapsed);
                if order_trace_tick == Some(tick_index + 1) && phase.starts_with("orders_") {
                    println!(
                        "order_trace tick={} phase={phase} us={elapsed}",
                        tick_index + 1
                    );
                }
                if tick_csv.is_some() {
                    profile.record_phase(phase, elapsed);
                }
            },
            |name| {
                if tick_csv.is_some() {
                    let now = Instant::now();
                    let settled_phase = match name {
                        "settled_clear" => Some(0),
                        "settled_boxes" => Some(1),
                        "settled_ranges" => Some(2),
                        "settled_queries" => Some(3),
                        "settled_candidates" => Some(4),
                        "settled_cells" => Some(5),
                        _ => None,
                    };
                    if let Some(phase) = settled_phase {
                        settled_detail[phase] = now
                            .duration_since(settled_last.unwrap_or(detail_last))
                            .as_micros();
                        settled_last = Some(now);
                        return;
                    }
                    let elapsed = now.duration_since(detail_last).as_micros();
                    detail_last = now;
                    match name {
                        "motion_begin" => {}
                        "motion_routes" => motion_detail[0] = elapsed,
                        "motion_ground" => ground_us = elapsed,
                        "motion_commit" => motion_detail[1] = elapsed,
                        "motion_feet" => feet_us = elapsed,
                        "motion_contacts" => motion_detail[2] = elapsed,
                        "motion_felling" => motion_detail[3] = elapsed,
                        "debris_begin" => {}
                        "debris_bodies" => debris_detail[0] = elapsed,
                        "debris_sections" => debris_detail[1] = elapsed,
                        "debris_hulls" => debris_detail[2] = elapsed,
                        "debris_confirm" => debris_detail[3] = elapsed,
                        "debris_settled" => debris_detail[4] = elapsed,
                        "debris_wake" => debris_detail[5] = elapsed,
                        "debris_translate" => debris_detail[6] = elapsed,
                        "debris_merge_keys" => debris_detail[7] = elapsed,
                        "debris_merge_groups" => debris_detail[8] = elapsed,
                        "debris_advance" => debris_detail[9] = elapsed,
                        "debris_compact" => debris_detail[10] = elapsed,
                        "debris_wreck_prune" => debris_detail[11] = elapsed,
                        "debris_wreck_append" => debris_detail[12] = elapsed,
                        "debris_retire" => debris_detail[13] = elapsed,
                        "debris_corpses" => debris_detail[14] = elapsed,
                        "debris_finish" => debris_detail[15] = elapsed,
                        "heat_begin" => {}
                        "heat_world" => heat_detail[0] = elapsed,
                        "heat_melt" => heat_detail[1] = elapsed,
                        "heat_units" => {
                            heat_detail[2] = elapsed + heat_detail[4..].iter().sum::<u128>()
                        }
                        "heat_edits" => heat_detail[3] = elapsed,
                        "unit_heat_conduction" => heat_detail[4] = elapsed,
                        "unit_heat_commit" => heat_detail[5] = elapsed,
                        "unit_heat_melt" => heat_detail[6] = elapsed,
                        "unit_heat_damage" => heat_detail[7] = elapsed,
                        _ => unreachable!(),
                    }
                }
            },
            |id, stage| {
                if tick_csv.is_some() {
                    if stage == "ground_accepted" || stage == "ground_rejected" {
                        ground_counts[usize::from(stage == "ground_rejected")] += 1;
                        return;
                    }
                    if stage == "local_hunt_accepted" || stage == "local_hunt_rejected" {
                        local_hunts[usize::from(stage == "local_hunt_rejected")] += 1;
                        return;
                    }
                    let now = Instant::now();
                    if stage == "begin" {
                        unit_start[id] = now;
                        unit_last[id] = now;
                    } else {
                        let elapsed = now.duration_since(unit_last[id]).as_micros();
                        unit_last[id] = now;
                        let phase = match stage {
                            "hunt" => 0,
                            "plan" | "plan_coarse" | "plan_fine" | "plan_target"
                            | "plan_prepare" | "plan_direct" => 1,
                            "terrain" => 2,
                            "ride" => 3,
                            "feet" => 4,
                            "unit_done" => 5,
                            _ => unreachable!(),
                        };
                        unit_phases[id][phase] += elapsed;
                        if stage == "unit_done" {
                            unit_window[id] += now.duration_since(unit_start[id]).as_micros();
                        }
                    }
                }
            },
            |id, event| {
                if let Some(profile) = &worker_profile {
                    profile.observe(id, event);
                }
            },
        );
        // Measure the authoritative tick, excluding optional replay and log I/O.
        let elapsed = start.elapsed().as_micros();
        if let Some(profile) = &worker_profile {
            profile.collect(&mut worker_windows[tick_index / 300]);
        }
        samples.push(elapsed);
        if tick_csv.is_some() {
            ground_windows[tick_index / 300].0 += ground_us;
            for (sum, value) in settled_phase_windows[tick_index / 300]
                .iter_mut()
                .zip(settled_detail)
            {
                *sum += value;
            }
            for (sum, value) in heat_phase_windows[tick_index / 300]
                .iter_mut()
                .zip(heat_detail)
            {
                *sum += value;
            }
            for (sum, value) in debris_phase_windows[tick_index / 300]
                .iter_mut()
                .zip(debris_detail)
            {
                *sum += value;
            }
            profile.tick = sim.tick;
            profile.collapse_work = sim.collapse_work;
            profile.total_us = elapsed;
            profile.feet_us = feet_us;
            [
                profile.route_us,
                profile.commit_us,
                profile.contact_us,
                profile.felling_us,
            ] = motion_detail;
            for unit in std::iter::once(&sim.gun).chain(sim.units.iter()) {
                profile.searching += usize::from(unit.motion.searching());
                profile.moving += usize::from(unit.motion.destination.is_some());
                profile.blocked += usize::from(unit.motion.blocked);
            }
            profile.fragments = sim.matter.debris.fragments.len();
            profile.felling = sim.matter.felling_pending();
            for fragment in &sim.matter.debris.fragments {
                profile.flying += usize::from(!fragment.resting);
                profile.chunks += usize::from(fragment.chunk.is_some());
            }
            tick_profiles.push(profile);
        }
        if replay && sim.tick == snapshot_tick {
            let bytes = sim.snapshot();
            println!(
                "snapshot tick={} bytes={} layout_bytes={}",
                sim.tick,
                bytes.len(),
                sim.layout.as_ref().unwrap().to_ron().unwrap().len()
            );
            if let Some(path) = args
                .windows(2)
                .find(|w| w[0] == "--snapshot-out")
                .map(|w| &w[1])
            {
                std::fs::write(path, &bytes).expect("write replay snapshot");
                println!("snapshot_file={path}");
            }
            let replica = restore_snapshot(sim.config.clone(), &bytes);
            assert_eq!(sim.state_hash(), replica.state_hash());
            restored = Some(replica);
        } else if let Some(replica) = &mut restored {
            if sim.tick <= snapshot_tick + 300 {
                replica.step();
                if sim.tick == snapshot_tick + 300 {
                    assert_eq!(sim.state_hash(), replica.state_hash(), "replay final tick");
                    if world_battle && !assault {
                        assert_eq!(
                            [sim.units[0].at[0], sim.units[0].at[2]],
                            [1530, 630],
                            "allied command base was pushed off its foundation"
                        );
                    }
                    println!("replay tick={} hash={:016x}", sim.tick, sim.state_hash());
                }
            }
        }
        peak_rounds = peak_rounds.max(sim.battle.rounds.len());
        if trace {
            for impact in &sim.battle.impacts {
                *impact_bins
                    .entry((impact.weapon as u8, impact.at[0] / 200, impact.at[2] / 200))
                    .or_default() += 1;
            }
        }
        if assault {
            let fired = sim.battle.actors[1].fired;
            if fired != assault_shots
                || !sim.battle.impacts.is_empty()
                || (assault_weapon && !sim.units[1].status.weapon)
            {
                println!(
                    "assault tick={} tank={:?} target={:?} fired={} impacts={:?} weapon={} damage={} base={:?}",
                    sim.tick,
                    sim.units[1].at,
                    sim.battle.actors[1].target,
                    fired,
                    sim.battle.impacts.iter().map(|i| i.at).collect::<Vec<_>>(),
                    sim.units[1].status.weapon,
                    sim.units[1].revision,
                    sim.battle_base_integrity()
                );
            }
            assault_shots = fired;
            assault_weapon = sim.units[1].status.weapon;
        }
        if world_battle && (progress || assault) && sim.tick % 3000 == 0 {
            println!(
                "progress tick={} phase={:?} alive={:?} bases={:?}",
                sim.tick,
                sim.battle.phase,
                sim.battle_counts(),
                sim.battle_base_integrity()
            );
        }
        if damage_trace {
            for impact in &sim.battle.impacts {
                impact_window[impact.weapon as usize] += 1;
            }
            laser_contacts += sim.battle.lasers.iter().filter(|trace| trace.hit).count();
            if sim.tick % 150 == 0 {
                let mut damage = [0u64; 2];
                let mut shots = [0u64; 2];
                let mut armed = [0usize; 2];
                for (id, u) in sim.units.iter().enumerate() {
                    let side = usize::from(!sim.allied(id));
                    damage[side] += u.revision;
                    shots[side] += u64::from(sim.battle.actors[id].fired);
                    armed[side] += usize::from(u.status.alive && u.status.weapon);
                }
                println!("damage_window tick={} phase={:?} revisions_delta={:?} fired_delta={:?} impact_weapons={:?} laser_contacts={laser_contacts} alive={:?} armed={armed:?} waves={:?} fragments={} chunks={} flying={} claims={} fragment_room={}",
                    sim.tick, sim.battle.phase,
                    [0, 1].map(|side| damage[side] - damage_previous[side]),
                    [0, 1].map(|side| shots[side] - shots_previous[side]),
                    impact_window, sim.battle_counts(), sim.battle.wave_counts(),
                    sim.matter.debris.fragments.len(),
                    sim.matter.debris.fragments.iter().filter(|f| f.chunk.is_some()).count(),
                    sim.matter.debris.fragments.iter().filter(|f| !f.resting).count(),
                    sim.matter.debris.claims(),
                    sim.matter.debris.fragment_room(&sim.matter.tuning));
                let admission = sim.matter.admission;
                let pool = sim.matter.debris.fragment_pools(&sim.matter.tuning);
                println!("damage_storage tick={} working={} parked={} flying={} working_cap={} parked_cap={} record_bytes={} section_bytes={}",
                    sim.tick, pool.working, pool.parked, pool.flying,
                    sim.matter.tuning.max_fragments, sim.matter.tuning.max_resting_fragments,
                    std::mem::size_of::<sim_core::burst::Fragment>(),
                    std::mem::size_of::<sim_core::burst::chunk::Chunk>());
                println!("damage_admission tick={} body_applied_unaffordable_deferred={:?} melt_applied_deferred={:?} room_scans={} room_items={}",
                    sim.tick,
                    [0, 1, 2].map(|i| admission.body[i] - admission_previous.body[i]),
                    [0, 1].map(|i| admission.melt[i] - admission_previous.melt[i]),
                    admission.room_scans - admission_previous.room_scans,
                    admission.room_items - admission_previous.room_items);
                admission_previous = admission;
                damage_previous = damage;
                shots_previous = shots;
                impact_window = [0; 6];
                laser_contacts = 0;
            }
        }
        if trace && sim.tick % 600 == 0 {
            println!(
                "trace tick={} phase={:?} bases={:?}",
                sim.tick,
                sim.battle.phase,
                sim.battle_base_integrity()
            );
            let mut hotspots: Vec<_> = std::mem::take(&mut impact_bins).into_iter().collect();
            hotspots.sort_by_key(|&(key, count)| (std::cmp::Reverse(count), key));
            println!(
                "  impact_hotspots={:?}",
                hotspots.into_iter().take(5).collect::<Vec<_>>()
            );
            for (id, u) in sim.units.iter().enumerate() {
                if u.kind.emplacement() || !u.status.alive {
                    continue;
                }
                let a = &sim.battle.actors[id];
                println!("  side={} unit={id} at=({}, {}) order={:?} target={:?} dest={:?} vel={} path={}/{} searching={} blocked={} recovery={:?} motor={} traction={} fired={} damage={}",
                    if sim.allied(id) { "blue" } else { "red" }, u.at[0], u.at[2], a.order, a.target,
                    u.motion.destination, u.motion.drive.vel.to_int_trunc(), u.motion.cursor,
                    u.motion.path.len(), u.motion.searching(), u.motion.blocked, u.motion.recovery_state(),
                    u.mobility.engine.to_f64_lossy(), u.mobility.traction.to_f64_lossy(), a.fired, u.revision);
            }
        }
        if let Some(id) = focus.filter(|&id| id < sim.units.len()) {
            if sim.tick >= focus_from && sim.tick % focus_every == 0 {
                let u = &sim.units[id];
                let a = &sim.battle.actors[id];
                println!("focus tick={} unit={} at={:?} alive={} weapon_ok={} order={:?} target={:?} ammo={:?} range={:?} fired={} damage={} motor={} traction={} dest={:?} path={}/{} searching={} blocked={} recovery={:?}",
                    sim.tick, id, u.at, u.status.alive, u.status.weapon, a.order, a.target,
                    sim.battle_weapon(id), sim.battle_attack_range(id), a.fired,
                    u.revision, u.mobility.engine.to_f64_lossy(), u.mobility.traction.to_f64_lossy(),
                    u.motion.destination, u.motion.cursor, u.motion.path.len(), u.motion.searching(),
                    u.motion.blocked, u.motion.recovery_state());
            }
        }
        if until_end && sim.battle.phase != sim_core::blast::battle::Phase::Fighting {
            break;
        }
    }
    if !assault && siege.is_none() {
        for side in [true, false] {
            assert!(
                sim.battle
                    .actors
                    .iter()
                    .enumerate()
                    .any(|(id, actor)| sim.allied(id) == side && actor.fired > 0),
                "a side never fired"
            );
            assert!(
                sim.units
                    .iter()
                    .enumerate()
                    .any(|(id, unit)| sim.allied(id) == side && unit.revision > 0),
                "a side never took spatial damage"
            );
        }
    }
    if let Some(side) = siege {
        let (expected, attacker, target) = if side == "blue" {
            (Phase::Victory, 1, 2)
        } else {
            (Phase::Defeat, 2, 0)
        };
        assert_eq!(sim.battle.phase, expected, "{side} siege did not finish");
        assert!(sim.battle.actors[attacker].fired > 0, "siege never fired");
        assert!(!sim.units[target].status.alive, "target base survived");
    }
    if let Some(expected) = expect_phase {
        assert_eq!(
            sim.battle.phase, expected,
            "battle did not reach expected phase"
        );
    }
    assert!(sim.matter.economy.ledger.balanced());
    if !world_battle {
        assert_eq!(
            sim.matter
                .economy
                .sunk(sim_core::matter::Sink::Annihilation),
            0
        );
    }
    if let Some(path) = tick_csv {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path).expect("tick CSV path"));
        writeln!(out, "tick,total_us,motion_us,route_us,commit_us,contact_us,felling_us,debris_us,battle_ai_us,battle_orders_us,battle_pose_us,battle_los_us,battle_laser_us,battle_beam_us,battle_halo_us,rubble_us,heat_flush_us,searching,moving,blocked,fragments,flying,chunks,felling,feet_us,collapse_us,collapse_queued,collapse_submitted,collapse_blocked_by,collapse_blocked_ticks,collapse_candidates,collapse_buffer_peak").unwrap();
        for p in &tick_profiles {
            writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                p.tick,
                p.total_us,
                p.motion_us,
                p.route_us,
                p.commit_us,
                p.contact_us,
                p.felling_us,
                p.debris_us,
                p.battle_ai_us,
                p.battle_orders_us,
                p.battle_pose_us,
                p.battle_los_us,
                p.battle_laser_us,
                p.battle_beam_us,
                p.battle_halo_us,
                p.rubble_us,
                p.heat_flush_us,
                p.searching,
                p.moving,
                p.blocked,
                p.fragments,
                p.flying,
                p.chunks,
                p.felling,
                p.feet_us,
                p.collapse_us,
                p.collapse_work.queued,
                p.collapse_work.submitted,
                p.collapse_work.blocked_by,
                p.collapse_work.blocked_ticks,
                p.collapse_work.gather_candidates,
                p.collapse_work.gather_buffer_peak
            )
            .unwrap();
        }
        out.flush().unwrap();
        for (index, window) in tick_profiles.chunks(300).enumerate() {
            let len = window.len() as u128;
            println!("collapse_window={}..{} mean_us={} queued_max={} submitted_max={} blocked_ticks_max={} candidates_max={} buffer_peak_max={}",
                window[0].tick, window[window.len()-1].tick,
                window.iter().map(|p| p.collapse_us).sum::<u128>() / len,
                window.iter().map(|p| p.collapse_work.queued).max().unwrap(),
                window.iter().map(|p| p.collapse_work.submitted).max().unwrap(),
                window.iter().map(|p| p.collapse_work.blocked_ticks).max().unwrap(),
                window.iter().map(|p| p.collapse_work.gather_candidates).max().unwrap(),
                window.iter().map(|p| p.collapse_work.gather_buffer_peak).max().unwrap());
            let mut times: Vec<_> = window.iter().map(|p| p.total_us).collect();
            times.sort_unstable();
            println!("tick_window={}..{} mean_us={} p99_us={} motion_us={} route_us={} commit_us={} contact_us={} felling_us={} debris_us={} battle_ai_us={} battle_orders_us={} battle_pose_us={} battle_los_us={} battle_laser_us={} battle_beam_us={} battle_halo_us={} searches_max={} flying_max={} fragments_max={} felling_max={}",
                window[0].tick, window[window.len()-1].tick,
                window.iter().map(|p| p.total_us).sum::<u128>() / len,
                times[times.len() * 99 / 100],
                window.iter().map(|p| p.motion_us).sum::<u128>() / len,
                window.iter().map(|p| p.route_us).sum::<u128>() / len,
                window.iter().map(|p| p.commit_us).sum::<u128>() / len,
                window.iter().map(|p| p.contact_us).sum::<u128>() / len,
                window.iter().map(|p| p.felling_us).sum::<u128>() / len,
                window.iter().map(|p| p.debris_us).sum::<u128>() / len,
                window.iter().map(|p| p.battle_ai_us).sum::<u128>() / len,
                window.iter().map(|p| p.battle_orders_us).sum::<u128>() / len,
                window.iter().map(|p| p.battle_pose_us).sum::<u128>() / len,
                window.iter().map(|p| p.battle_los_us).sum::<u128>() / len,
                window.iter().map(|p| p.battle_laser_us).sum::<u128>() / len,
                window.iter().map(|p| p.battle_beam_us).sum::<u128>() / len,
                window.iter().map(|p| p.battle_halo_us).sum::<u128>() / len,
                window.iter().map(|p| p.searching).max().unwrap(),
                window.iter().map(|p| p.flying).max().unwrap(),
                window.iter().map(|p| p.fragments).max().unwrap(),
                window.iter().map(|p| p.felling).max().unwrap());
            let mut units: Vec<_> = unit_time_windows[index]
                .iter()
                .copied()
                .enumerate()
                .collect();
            println!(
                "local_hunts_window={}..{} accepted={} rejected={}",
                window[0].tick,
                window[window.len() - 1].tick,
                local_hunt_windows[index][0],
                local_hunt_windows[index][1],
            );
            units.sort_unstable_by_key(|&(id, us)| (std::cmp::Reverse(us), id));
            println!(
                "motion_units_window={}..{} top_us_per_tick={:?}",
                window[0].tick,
                window[window.len() - 1].tick,
                units
                    .into_iter()
                    .take(8)
                    .map(|(id, us)| (id, us / len))
                    .collect::<Vec<_>>()
            );
            let mut ranked: Vec<_> = unit_time_windows[index]
                .iter()
                .copied()
                .enumerate()
                .collect();
            ranked.sort_unstable_by_key(|&(id, us)| (std::cmp::Reverse(us), id));
            println!(
                "motion_unit_phases_window={}..{} hunt_plan_terrain_ride_feet_other_us={:?}",
                window[0].tick,
                window[window.len() - 1].tick,
                ranked
                    .into_iter()
                    .take(3)
                    .map(|(id, _)| (id, unit_phase_windows[index][id].map(|us| us / len)))
                    .collect::<Vec<_>>()
            );
            println!(
                "debris_phases_window={}..{} bodies_sections_hulls_confirm_settled_wake_translate_keys_groups_merge_compact_prune_append_retire_corpses_finish_us={:?}",
                window[0].tick,
                window[window.len() - 1].tick,
                debris_phase_windows[index].map(|us| us / len)
            );
            println!(
                "heat_phases_window={}..{} world_melt_units_edits_unit_conduction_commit_melt_damage_us={:?}",
                window[0].tick,
                window[window.len() - 1].tick,
                heat_phase_windows[index].map(|us| us / len)
            );
            println!(
                "settled_phases_window={}..{} clear_boxes_ranges_queries_candidates_cells_us={:?}",
                window[0].tick,
                window[window.len() - 1].tick,
                settled_phase_windows[index].map(|us| us / len)
            );
            println!(
                "motion_ground_window={}..{} mean_us={} accepted={} rejected={}",
                window[0].tick,
                window[window.len() - 1].tick,
                ground_windows[index].0 / len,
                ground_windows[index].1[0],
                ground_windows[index].1[1]
            );
            println!(
                "motion_feet_window={}..{} mean_us={}",
                window[0].tick,
                window[window.len() - 1].tick,
                window.iter().map(|p| p.feet_us).sum::<u128>() / len,
            );
        }
        println!("tick_csv={path}");
        let (mut single, mut full, mut identity) = (0, 0, 0);
        for fragment in &sim.matter.debris.fragments {
            single += usize::from(fragment.shape.count_ones() == 1);
            let cells = u32::from(fragment.side.clamp(1, 4)).pow(3);
            let mask = if cells == 64 {
                u64::MAX
            } else {
                (1u64 << cells) - 1
            };
            full += usize::from(fragment.shape == mask);
            identity += usize::from(fragment.basis == sim_core::burst::IDENTITY);
        }
        println!("fragment_shapes single={single} full={full} identity={identity}");
    }
    if profile_workers {
        println!("navigation_task_profile=wall_time_not_cpu opt_in=true");
        for (index, window) in worker_windows.iter().enumerate() {
            window.report(index * 300 + 1, ((index + 1) * 300).min(ticks));
        }
    }
    samples.sort_unstable();
    let count = samples.len();
    println!("ticks={} units={} phase={:?} alive={:?} bases={:?} shots={:?} damage={:?} peak_rounds={} p50_us={} p95_us={} p99_us={} max_us={} hash={:016x}",sim.tick,sim.units.len(),sim.battle.phase,sim.battle_counts(),sim.battle_base_integrity(),sim.battle.actors.iter().map(|a|a.fired).collect::<Vec<_>>(),sim.units.iter().map(|u|u.revision).collect::<Vec<_>>(),peak_rounds,samples[count/2],samples[count*95/100],samples[count*99/100],samples[count-1],sim.state_hash());
    if args.iter().any(|arg| arg == "--felling-profile") {
        println!(
            "felling_profile={:?}",
            sim.matter.felling_profile(&sim.world, &sim.navigation.fell)
        );
    }
    for (id, u) in sim.units.iter().enumerate() {
        println!(
            "unit={id} alive={} engine={} weapon={} core_pct={} at={:?} order={:?} target={:?} destination={:?} path={} cursor={} next={:?} vel={} searching={} blocked={} floors={:?} last_plan={:?}",
            u.status.alive,
            u.status.engine,
            u.status.weapon,
            u.integrity.core.mul(fx::Fx::from_int(100)).to_int_trunc(),
            u.at,
            sim.battle.actors[id].order,
            sim.battle.actors[id].target,
            u.motion.destination,
            u.motion.path.len(),
            u.motion.cursor,
            u.motion.path.get(u.motion.cursor),
            u.motion.drive.vel.to_int_trunc(),
            u.motion.searching(),
            u.motion.blocked,
            u.motion.floors.map(fx::Fx::to_int_trunc),
            u.motion.last_plan
        );
    }
    for (name, mut values) in phases {
        values.sort_unstable();
        println!(
            "phase={name} mean_us={} p95_us={} p99_us={} max_us={}",
            values.iter().sum::<u128>() / values.len() as u128,
            values[values.len() * 95 / 100],
            values[values.len() * 99 / 100],
            values[values.len() - 1]
        );
    }
    if assault {
        assert_eq!(
            sim.battle.phase,
            Phase::Victory,
            "cross-map assault did not finish"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pursuit_subphases_remain_in_the_total_order_time() {
        let mut profile = TickProfile::default();
        for name in [
            "orders_retarget_coarse",
            "orders_retarget_target",
            "orders_retarget_prepare",
            "orders_retarget_direct",
            "orders_retarget_fine",
            "orders_approach",
            "orders_firing_point",
            "orders_flank",
            "orders_retarget_setup",
            "orders_retarget",
            "battle_orders",
        ] {
            profile.record_phase(name, 17);
        }
        assert_eq!(profile.battle_orders_us, 11 * 17);
        profile.record_phase("orders_laser_halo", 23);
        assert_eq!(profile.battle_orders_us, 11 * 17 + 23);
        assert_eq!(profile.battle_laser_us, 23);
        assert_eq!(profile.battle_halo_us, 23);
    }
}

fn restore_snapshot(config: modding::explosion::ExplosionConfig, bytes: &[u8]) -> BlastRange {
    let mut last = ("header", 0, None);
    BlastRange::restore_observed(config, bytes, |section, offset, unit| {
        last = (section, offset, unit);
    })
    .unwrap_or_else(|| {
        panic!(
            "world battle snapshot refused: section={} offset={} unit={:?}",
            last.0, last.1, last.2
        )
    })
}

/// Isolated complete-batch qualification: setup, serialization and log I/O are
/// outside the measured collapse calls. No weapon or timing semantics change.
fn collapse_stress(mode: &str) {
    use voxel::{bodies::Bodies, collapse::CollapseLayer, world::Edit, Generator, World};
    struct Terrain(bool);
    impl Generator for Terrain {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            if if self.0 {
                (32..288).contains(&x) && (32..48).contains(&y) && (32..288).contains(&z)
            } else {
                y < 48
            } {
                2
            } else {
                0
            }
        }
    }
    assert!(matches!(mode, "backlog" | "oversized"));
    let mut samples = Vec::new();
    for repeat in 0..3 {
        let mut world = World::new(
            320,
            96,
            320,
            4_000_000,
            Box::new(Terrain(mode == "oversized")),
        );
        // Fixture setup must really apply the entire requested edit batch;
        // the ordinary materialization cap otherwise leaves most digs pending.
        world.max_materialize_per_tick = 4_000_000;
        let mut queued = std::collections::BTreeMap::new();
        if mode == "oversized" {
            world.queue_dig(32, 32, 32);
        } else {
            'cells: for x in 32..288 {
                for y in 8..43 {
                    for z in 32..288 {
                        world.queue_dig(x, y, z);
                        if queued.len() == 2_286_933 {
                            queued.insert([x, y, z], Edit::dig(x, y, z));
                            break 'cells;
                        }
                        queued.insert([x, y, z], Edit::dig(x, y, z));
                    }
                }
            }
        }
        world.flush();
        queued.clear();
        for e in world.edits() {
            queued.insert([e.x, e.y, e.z], *e);
        }
        assert_eq!(world.queued(), 0, "fixture setup left source digs pending");
        let expected = if mode == "backlog" { 2_286_934 } else { 1 };
        assert_eq!(
            queued.len(),
            expected,
            "fixture must exercise the requested batch"
        );
        println!(
            "collapse_fixture mode={mode} queued={} edit_bytes={}",
            queued.len(),
            std::mem::size_of::<Edit>()
        );
        let mut collapse = CollapseLayer::new(&world);
        let mut bodies = Bodies::new(16, fx::Fx::from_int(100));
        collapse.set_max_body_cells(4096);
        let mut ticks = Vec::new();
        let mut peak = 0;
        let mut candidates = 0;
        for _ in 0..128 {
            let start = Instant::now();
            let st = collapse.apply_iter_observed(
                &mut world,
                queued.values().copied(),
                &mut bodies,
                |_| {},
            );
            ticks.push(start.elapsed().as_micros());
            peak = peak.max(st.gather_buffer_peak);
            candidates = candidates.max(st.gather_candidates);
            queued.clear();
            if !collapse.pending() {
                break;
            }
        }
        assert!(!collapse.pending(), "qualification did not drain");
        assert!(
            bodies.list.is_empty(),
            "both fixtures must retain their terrain"
        );
        let mut snapshot = fx::blob::Blob::new();
        world.save(&mut snapshot);
        world.save_transfers(&mut snapshot);
        collapse.save(&mut snapshot);
        bodies.save(&mut snapshot);
        let hash = snapshot.v.iter().fold(0xcbf29ce484222325u64, |h, v| {
            (h ^ u64::from(*v)).wrapping_mul(0x100000001b3)
        });
        println!("collapse_stress mode={mode} repeat={repeat} sum_us={} max_us={} calls={} candidates={candidates} buffer_peak={peak} hash={hash:016x}",
            ticks.iter().sum::<u128>(), ticks.iter().max().unwrap(), ticks.len());
        samples.extend(ticks);
    }
    samples.sort_unstable();
    println!(
        "collapse_stress_summary mode={mode} calls={} p50_us={} p95_us={} max_us={}",
        samples.len(),
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100],
        samples.last().unwrap()
    );
}
