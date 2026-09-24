//! CPU-only live-range benchmark; no frames, captures or GPU readback.
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    // `--weapon shell` measures the heavier debris of the shell (ADR-296).
    let args: Vec<String> = std::env::args().collect();
    let value = |key: &str| args.windows(2).find(|w| w[0] == key).map(|w| &w[1]);
    let ticks = value("--ticks").map_or(1800, |v| v.parse::<usize>().expect("ticks"));
    assert!(ticks > 0);
    let single = args.iter().any(|a| a == "--single-shot");
    let drive_to = value("--move").map(|v| {
        let (x, z) = v.split_once(',').expect("--move x,z");
        [
            x.trim().parse().expect("move x"),
            z.trim().parse().expect("move z"),
        ]
    });
    assert!(
        !single || drive_to.is_none(),
        "choose --move or --single-shot"
    );
    let uncovered = args.iter().any(|a| a == "--open");
    let layout_ron = value("--range").map(|p| std::fs::read_to_string(p).expect("range file"));
    let thermal_stress = args.iter().any(|a| a == "--thermal-stress");
    let fire_stress = args.iter().any(|a| a == "--fire-stress");
    let weapon = args
        .iter()
        .position(|a| a == "--weapon")
        .and_then(|i| args.get(i + 1).cloned())
        .unwrap_or_else(|| "energy".to_string());
    let ron = match weapon.as_str() {
        "energy" => include_str!("../../../../data/energy-cannon.ron"),
        "shell" => include_str!("../../../../data/explosion.ron"),
        "anni" => include_str!("../../../../data/anni-cannon.ron"),
        "flame" => include_str!("../../../../data/flamethrower.ron"),
        "machine-gun" => include_str!("../../../../data/machine-gun.ron"),
        _ => panic!("--weapon expects energy, shell, anni, flame or machine-gun"),
    };
    // Power scales the charge as the window's buttons do (P-31).
    let power: u32 = args
        .iter()
        .position(|a| a == "--power")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let modes: &[bool] = if drive_to.is_some() {
        &[false]
    } else if single {
        &[true]
    } else {
        &[false, true]
    };
    for &firing in modes {
        let mut config = modding::explosion::ExplosionConfig::from_ron(ron).unwrap();
        config.energy = config.energy * power / 1000;
        config.material_energy_j =
            (u64::from(config.material_energy_j) * u64::from(power) / 1000) as u32;
        let mut layout = modding::range::RangeLayout::from_ron(
            layout_ron
                .as_deref()
                .unwrap_or(include_str!("../../../../data/range-cover.ron")),
        )
        .unwrap();
        if let Some(zone) = value("--zone") {
            let index = zone
                .parse::<usize>()
                .expect("--zone: positive sector number");
            let sector = layout
                .zones
                .get(index.checked_sub(1).expect("zones start at 1"))
                .expect("--zone: sector in range layout");
            layout.gun = sector.spawn;
            layout.target = sector.target;
        }
        if uncovered {
            layout.blocks.clear();
        }
        if weapon == "machine-gun" {
            layout.gun_kind = modding::range::RangeUnitKind::Strider;
        }
        let mut scene = BlastRange::with_layout(config, layout).unwrap();
        if scene.config.delivery != modding::explosion::Delivery::Blast {
            scene.aim = Some(scene.units[0].world_pos([30, 18, 16]));
        }
        if !single {
            scene.control();
        }
        if let Some(goal) = drive_to {
            assert!(scene.order_move(goal), "--move: no route");
        }
        // Saturate the sparse field with real surviving armour cells. This
        // measures the budget ceiling, independent of a lucky flame contact.
        if thermal_stress {
            'units: for (id, u) in scene.units.iter().enumerate() {
                let b = &u.body;
                for z in 0..b.vol.nz {
                    for y in 0..b.vol.ny {
                        for x in 0..b.vol.nx {
                            if b.vol.get(x as usize, y as usize, z as usize) {
                                scene.thermals.deposit(
                                    sim_core::blast::thermal::Cell {
                                        unit: id as u16,
                                        part: 0,
                                        at: [x, y, z],
                                    },
                                    80,
                                    &scene.thermal_tuning,
                                );
                                if scene.thermals.hot() == scene.thermal_tuning.max_cells {
                                    break 'units;
                                }
                            }
                        }
                    }
                }
            }
            println!("thermal_stress_seeded={}", scene.thermals.hot());
        }
        if fire_stress {
            for (id, u) in scene
                .units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.motion.route.is_empty())
            {
                let at = scene.combustion.tuning.reservoir(u.kind).at;
                scene.thermals.deposit(
                    sim_core::blast::thermal::Cell {
                        unit: id as u16,
                        part: 0,
                        at,
                    },
                    scene.combustion.tuning.ignition,
                    &scene.thermal_tuning,
                );
            }
        }
        let mut samples = Vec::new();
        let mut shots = 0;
        let mut hottest = 0;
        let mut phases = std::collections::BTreeMap::<&str, (u128, usize)>::new();
        let mut totals = std::collections::BTreeMap::<&str, Vec<u128>>::new();
        for tick in 0..ticks {
            if !single && firing && tick % 180 == 0 && scene.refire() {
                shots += 1;
            }
            let start = Instant::now();
            let mut mark = start;
            scene.step_observed(|name| {
                let now = Instant::now();
                let elapsed = now.duration_since(mark).as_micros();
                mark = now;
                totals.entry(name).or_default().push(elapsed);
                let peak = phases.entry(name).or_default();
                if elapsed > peak.0 {
                    *peak = (elapsed, tick);
                }
            });
            samples.push(start.elapsed().as_micros());
            hottest = hottest.max(scene.thermals.hot());
        }
        // The slowest ticks by number: a spike names its moment.
        println!(
            "support_graph={:?} gun={:?}",
            scene.support_graph_size(),
            scene.gun.at
        );
        let mut top: Vec<(u128, usize)> = samples.iter().copied().zip(0..).collect();
        top.sort_unstable_by(|a, b| b.cmp(a));
        let top: Vec<String> = top
            .iter()
            .take(5)
            .map(|(us, tick)| format!("{tick}:{us}"))
            .collect();
        println!(
            "weapon={weapon} power={power} firing={firing} slowest_ticks_us={}",
            top.join(",")
        );
        for (name, (us, tick)) in phases {
            let v = totals.get_mut(name).unwrap();
            v.sort_unstable();
            println!(
                "phase={name} firing={firing} p50_us={} p99_us={} max_us={us} tick={tick}",
                v[v.len() / 2],
                v[v.len() * 99 / 100]
            );
        }
        println!(
            "thermal_peak_cells={hottest} melted={} rejected_heat={} exchanged_heat={}",
            scene.thermals.melted, scene.thermals.rejected_heat, scene.heat_exchange.transferred
        );
        println!(
            "fuel_consumed={} generated_heat={} dissipated_heat={} active_fires={}",
            scene.combustion.consumed,
            scene.combustion.generated_heat,
            scene.combustion.dissipated_heat,
            scene.combustion.sources(&scene.units, &scene.matter).len()
        );
        if fire_stress {
            assert!(
                scene.combustion.consumed > 0,
                "fire benchmark did not burn real fuel"
            );
        }
        samples.sort_unstable();
        let laps: Vec<_> = scene
            .units
            .iter()
            .filter(|u| !u.motion.route.is_empty())
            .map(|u| u.motion.laps)
            .collect();
        if ticks >= 1800 && !single {
            assert!(laps.iter().any(|n| *n > 0), "no completed patrols");
        }
        if single {
            assert!(
                scene.impact.is_some() && scene.removed > 0,
                "single-shot benchmark did no destruction"
            );
            shots = 1;
        }
        if firing {
            assert!(
                (scene.removed > 0 || scene.units.iter().any(|u| u.revision > 0))
                    && (single || shots > 1),
                "no real repeated destruction"
            );
        }
        println!("firing={firing} ticks={} units={} shots={shots} removed={} laps={laps:?} p50_us={} p99_us={} max_us={} hash={:016x}",
            samples.len(), scene.units.len()+1, scene.removed, samples[samples.len()/2], samples[samples.len()*99/100], samples[samples.len()-1], scene.state_hash());
    }
}
