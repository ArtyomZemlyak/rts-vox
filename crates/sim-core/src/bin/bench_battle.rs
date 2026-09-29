//! Bounded battle load, including real damage, concurrent waves and bookkeeping.
use sim_core::blast::battle::{Order, Phase, Weapon};
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let assault = args.iter().any(|a| a == "--assault");
    let siege = args
        .windows(2)
        .find(|w| w[0] == "--siege")
        .map(|w| w[1].as_str());
    let replay = args.iter().any(|a| a == "--replay");
    let world_battle = assault || siege.is_some() || args.iter().any(|a| a == "--world");
    let progress = args.iter().any(|a| a == "--progress");
    let trace = args.iter().any(|a| a == "--trace");
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
        !ai_vs_ai || world_battle,
        "--ai-vs-ai requires --world, --assault or --siege"
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
        layout.units = modding::range_skirmish::WorldBattle::from_ron(include_str!(
            "../../../../data/battle-world.ron"
        ))
        .unwrap()
        .units;
        let selected: &[usize] = match balance {
            "even" => &[],
            // Bases and the central contact remain fixed; the missing rear
            // reinforcement or forward escort changes the local force ratio.
            "blue-light" => &[0, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            "red-light" => &[0, 1, 2, 3, 4, 5, 6, 9, 10, 11],
            "blue-front-light" => &[0, 1, 2, 3, 4, 6, 7, 8, 9, 10, 11],
            "red-front-light" => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            _ => {
                panic!("--balance: even, blue-light, red-light, blue-front-light, red-front-light")
            }
        };
        if !selected.is_empty() {
            layout.units = selected
                .iter()
                .map(|&id| layout.units[id].clone())
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
    if assault {
        assert!(sim.set_battle_weapon(1, Some(Weapon::Annihilation)));
        assert!(sim.battle_order(1, Order::Attack(2), None));
    }
    let mut samples = Vec::new();
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
    for _ in 0..ticks {
        let start = Instant::now();
        let mut last = start;
        sim.step_observed(|phase| {
            let now = Instant::now();
            let elapsed = now.duration_since(last).as_micros();
            last = now;
            phases.entry(phase).or_default().push(elapsed);
        });
        // Measure the authoritative tick, excluding optional replay and log I/O.
        samples.push(start.elapsed().as_micros());
        if replay && sim.tick == snapshot_tick {
            let bytes = sim.snapshot();
            println!(
                "snapshot tick={} bytes={} layout_bytes={}",
                sim.tick,
                bytes.len(),
                sim.layout.as_ref().unwrap().to_ron().unwrap().len()
            );
            let replica = BlastRange::restore(sim.config.clone(), &bytes)
                .expect("world battle snapshot restores");
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
    samples.sort_unstable();
    let count = samples.len();
    println!("ticks={} units={} phase={:?} alive={:?} bases={:?} shots={:?} damage={:?} peak_rounds={} p50_us={} p95_us={} p99_us={} max_us={} hash={:016x}",sim.tick,sim.units.len(),sim.battle.phase,sim.battle_counts(),sim.battle_base_integrity(),sim.battle.actors.iter().map(|a|a.fired).collect::<Vec<_>>(),sim.units.iter().map(|u|u.revision).collect::<Vec<_>>(),peak_rounds,samples[count/2],samples[count*95/100],samples[count*99/100],samples[count-1],sim.state_hash());
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
