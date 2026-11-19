//! Full stock communicating-vessel settlement. Clocks and CSV stay outside SIM.
use modding::{explosion::ExplosionConfig, laboratory::experiments};
use sim_core::blast::BlastRange;
use std::{fs::File, io::Write, time::Instant};

#[derive(Clone, Debug)]
struct Levels {
    left: u64,
    right: u64,
    passage: u64,
    full_passage_cells: usize,
    left_head: u64,
    right_head: u64,
    min: u64,
    max: u64,
}
fn levels(sim: &BlastRange) -> Levels {
    // Authored stock fixture: two equal 17x36 interiors, floor at y16,
    // divider x383..385 and a two-cell-high submerged passage.
    let mut columns = [0u64; 17 * 36 * 2];
    let c = u64::from(sim.water.tuning.units_per_voxel);
    let mut heads = [16 * c; 17 * 36 * 2];
    let mut passage = 0;
    let mut full_passage_cells = 0;
    for (p, amount) in sim.water.cells() {
        if !(318..354).contains(&p[2]) || !(16..40).contains(&p[1]) {
            continue;
        }
        let x = match p[0] {
            366..=382 => (p[0] - 366) as usize,
            385..=401 => (p[0] - 385 + 17) as usize,
            383..=384 => {
                passage += u64::from(amount);
                full_passage_cells += usize::from(u64::from(amount) >= c);
                continue;
            }
            _ => continue,
        };
        let i = x * 36 + (p[2] - 318) as usize;
        columns[i] += u64::from(amount);
        // Same free-surface head as pressure projection; compressed interior
        // volume and sub-tension films are not extra geometric height.
        if amount >= sim.water.tuning.surface_tension {
            heads[i] = heads[i].max(u64::from(p[1]) * c + u64::from(amount).min(c));
        }
    }
    Levels {
        left: columns[..17 * 36].iter().sum(),
        right: columns[17 * 36..].iter().sum(),
        passage,
        full_passage_cells,
        left_head: heads[..17 * 36].iter().sum(),
        right_head: heads[17 * 36..].iter().sum(),
        min: *heads.iter().min().unwrap(),
        max: *heads.iter().max().unwrap(),
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let arg = |key: &str| args.windows(2).find(|w| w[0] == key).map(|w| w[1].as_str());
    let workers: usize = arg("--workers").unwrap_or("1").parse().unwrap();
    let max_ticks: u32 = arg("--ticks").unwrap_or("20000").parse().unwrap();
    let restore_at: u32 = arg("--restore-at").unwrap_or("0").parse().unwrap();
    let legacy = args.iter().any(|a| a == "--legacy-water");
    let sealed = args.iter().any(|a| a == "--sealed-floor");
    let mut experiment = experiments::stock()
        .into_iter()
        .find(|e| e.id == "water-vessels")
        .unwrap();
    if sealed {
        experiment.layout.blocks.push(modding::range::RangeBlock {
            min: [366, 15, 318],
            max: [402, 16, 354],
            material: 2,
        });
    }
    let config = ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron")).unwrap();
    let mut sim = BlastRange::with_layout(config, experiment.layout).unwrap();
    if legacy {
        sim.water.tuning.pressure_checks = 2048;
        sim.water.tuning.pressure_packets = 64;
        sim.water.tuning.pressure_interval = 4;
        sim.water.tuning.transfers_per_tick = 1024;
    }
    sim.control();
    sim.set_physics_workers(workers);
    sim.set_navigation_workers(workers);
    let initial = sim.water.stats().initial_units;
    // One final indivisible unit covers integer transfer rounding. Report the
    // authored deadband and this extra quantum separately, never as equal mass.
    let deadband = u64::from(sim.water.tuning.surface_tension);
    let tolerance = deadband + 1;
    let mut csv = arg("--csv").map(|p| File::create(p).unwrap());
    if let Some(file) = &mut csv {
        writeln!(
            file,
            "tick,left,right,passage,min_head,max_head,spread,stable_ticks,absorbed,vapour,water_units,hash"
        )
        .unwrap();
    }
    let mut stable = 0;
    let mut first_settled = None;
    let mut first_one_percent = None;
    let mut candidate: Option<Levels> = None;
    let mut times = Vec::new();
    let started = Instant::now();
    for tick in 1..=max_ticks {
        let start = Instant::now();
        sim.step();
        times.push(start.elapsed().as_nanos());
        assert_eq!(sim.water.stats().units, initial, "water lost at {tick}");
        assert!(sim.matter.economy.ledger.balanced());
        if tick == restore_at {
            let expected = sim.state_hash();
            sim = BlastRange::restore(config, &sim.snapshot()).expect("mid-settlement restore");
            sim.set_physics_workers(workers);
            sim.set_navigation_workers(workers);
            assert_eq!(sim.state_hash(), expected);
        }
        let n = levels(&sim);
        if first_one_percent.is_none() && n.right >= initial / 100 {
            first_one_percent = Some(tick);
        }
        let settled = n.right > initial / 4
            && n.left_head.abs_diff(n.right_head) <= 17 * 36 * tolerance
            && n.max - n.min <= tolerance
            && n.full_passage_cells == 2 * 2 * 36;
        if settled {
            if candidate.as_ref().is_some_and(|first| {
                first.left.abs_diff(n.left) > 17 * 36 * tolerance
                    || first.right.abs_diff(n.right) > 17 * 36 * tolerance
                    || first.min.abs_diff(n.min) > tolerance
                    || first.max.abs_diff(n.max) > tolerance
            }) {
                stable = 0;
                candidate = None;
            }
            if candidate.is_none() {
                candidate = Some(n.clone());
            }
            stable += 1;
        } else {
            stable = 0;
            candidate = None;
        }
        if stable == 1 && first_settled.is_none() {
            first_settled = Some(tick);
        }
        if tick % 20 == 0 || stable >= 120 || tick == max_ticks {
            if let Some(file) = &mut csv {
                writeln!(
                    file,
                    "{tick},{},{},{},{},{},{},{stable},{},{},{initial},{:016x}",
                    n.left,
                    n.right,
                    n.passage,
                    n.min,
                    n.max,
                    n.max - n.min,
                    sim.water.stats().absorbed_units,
                    sim.water.stats().vapor_units,
                    sim.state_hash()
                )
                .unwrap();
                file.flush().unwrap();
            }
        }
        if stable >= 120 || tick == max_ticks {
            times.sort_unstable();
            println!("vessels workers={workers} legacy={legacy} sealed={sealed} ticks={tick} simulated_ms={} reached_one_percent={first_one_percent:?} first_settled={first_settled:?} stable_ticks={stable} left={} right={} passage={} min_head={} max_head={} spread={} deadband={deadband} rounding_quantum=1 tolerance={tolerance} absorbed={} vapour={} water_units={initial} balanced=true hash={:016x} wall_ms={} mean_tick_us={} p50_us={} p95_us={} p99_us={}",u64::from(tick)*50,n.left,n.right,n.passage,n.min,n.max,n.max-n.min,sim.water.stats().absorbed_units,sim.water.stats().vapor_units,sim.state_hash(),started.elapsed().as_millis(),times.iter().sum::<u128>()/times.len() as u128/1000,times[(times.len()-1)/2]/1000,times[(times.len()-1)*95/100]/1000,times[(times.len()-1)*99/100]/1000);
            assert!(
                stable >= 120,
                "stock vessels did not fully settle within the measured window"
            );
            break;
        }
    }
}
