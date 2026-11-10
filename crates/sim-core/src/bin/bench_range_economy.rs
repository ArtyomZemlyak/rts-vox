//! Empty-face digging cost through the common range tick (review 246).
//! Diagnostic inputs only: no scenario-specific simulation rules.
use modding::explosion::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef, RangeUnitKind, MAX_RANGE_UNITS};
use modding::range_economy::{Producer, RangeEconomy, MAX_DIG_POOL, MAX_DIG_RADIUS};
use sim_core::blast::{battle::Order, BlastRange, RangeUnit};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Empty,
    Rock,
    Deep,
}

struct Args {
    count: usize,
    radius: u32,
    ticks: u32,
    rate: u32,
    pool: Option<u32>,
    case: Case,
    control: bool,
    replay: bool,
    trace: bool,
    catalog: bool,
    csv: Option<String>,
    snapshot: Option<String>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut a = Self {
            count: 64,
            radius: 32,
            ticks: 120,
            rate: 1,
            pool: None,
            case: Case::Empty,
            control: false,
            replay: false,
            trace: false,
            catalog: false,
            csv: None,
            snapshot: None,
        };
        let mut args = std::env::args().skip(1);
        let mut flags = 0;
        while let Some(flag) = args.next() {
            flags += 1;
            match flag.as_str() {
                "--control" => a.control = true,
                "--replay" => a.replay = true,
                "--trace" => a.trace = true,
                "--catalog" => a.catalog = true,
                "--units" | "--radius" | "--ticks" | "--rate" | "--pool" | "--case" | "--csv"
                | "--snapshot" => {
                    let v = args
                        .next()
                        .ok_or_else(|| format!("missing value for {flag}"))?;
                    match flag.as_str() {
                        "--units" => a.count = v.parse().map_err(|_| "invalid --units")?,
                        "--radius" => a.radius = v.parse().map_err(|_| "invalid --radius")?,
                        "--ticks" => a.ticks = v.parse().map_err(|_| "invalid --ticks")?,
                        "--rate" => a.rate = v.parse().map_err(|_| "invalid --rate")?,
                        "--pool" => a.pool = Some(v.parse().map_err(|_| "invalid --pool")?),
                        "--case" => {
                            a.case = match v.as_str() {
                                "empty" => Case::Empty,
                                "rock" => Case::Rock,
                                "deep" => Case::Deep,
                                _ => return Err(format!("unknown case: {v}")),
                            }
                        }
                        "--csv" => a.csv = Some(v),
                        "--snapshot" => a.snapshot = Some(v),
                        _ => unreachable!(),
                    }
                }
                _ => return Err(format!("unknown argument: {flag}")),
            }
        }
        if !(1..=MAX_RANGE_UNITS).contains(&a.count)
            || !(1..=MAX_DIG_RADIUS).contains(&a.radius)
            || !(1..=10000).contains(&a.ticks)
            || !(1..=a.pool.unwrap_or(RangeEconomy::stock().dig_edits_in_flight)).contains(&a.rate)
            || a.pool.is_some_and(|p| !(1..=MAX_DIG_POOL).contains(&p))
        {
            return Err("require units 1..=64, radius 1..=32, ticks 1..=10000 and 1 <= rate <= pool <= MAX_DIG_POOL".into());
        }
        if a.catalog && flags != 1 {
            return Err("--catalog cannot be combined with simulation arguments".into());
        }
        Ok(a)
    }
}

fn main() {
    let a = Args::parse().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    if a.catalog {
        catalog();
        return;
    }
    let (count, radius, ticks, control) = (a.count, a.radius, a.ticks, a.control);
    let config = ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron"))
        .expect("stock weapon");
    let mut layout = RangeLayout::from_ron(include_str!("../../../../data/fixture-drive.ron"))
        .expect("fixture layout");
    layout.units = (0..count)
        .map(|id| RangeUnitDef {
            kind: RangeUnitKind::Tank,
            allied: true,
            at: [100 + (id % 8) as i32 * 80, 16, 100 + (id / 8) as i32 * 80],
            weapon: None,
        })
        .collect();
    let mut economy = RangeEconomy::stock();
    if let Some(pool) = a.pool {
        economy.dig_edits_in_flight = pool;
    }
    economy.bank = None;
    economy.reclaim = None;
    economy.production = None;
    for kind in &mut economy.kinds {
        kind.store = 0;
    }
    let kind = economy.kind_mut(RangeUnitKind::Tank).unwrap();
    kind.tank = 1 << 30;
    kind.dig_radius = radius;
    kind.dig_reach = 1;
    kind.dig_edits_per_tick = a.rate;
    let pool = economy.dig_edits_in_flight;
    layout.economy = Some(economy);
    if a.case != Case::Empty {
        // Supported buried rock; the top stays at the clearing's original
        // height. Deep puts most of the sphere in air before the first rock.
        layout.blocks.push(RangeBlock {
            min: [64, 1, 64],
            max: [704, 16, 704],
            material: 6,
        });
    }
    let mut sim = BlastRange::with_layout(config, layout).expect("valid diagnostic layout");
    assert!(sim.set_physics_workers(4));
    assert!(sim.set_navigation_workers(4));
    sim.start_free_control().unwrap();
    let touched = sim.world.touched_chunks();
    for id in 0..count {
        let u = &sim.units[id];
        let face = [
            u.at[0],
            match a.case {
                Case::Empty => 96,
                Case::Rock => 8,
                Case::Deep => 16 + radius as i32 - 8,
            },
            u.at[2],
        ];
        // The entire ball is above the clearing's ground. No edit, routing,
        // hauling or reclaim should be needed, even at the largest radius.
        if a.case == Case::Empty {
            assert!(!sim.world.solid(face[0] as u32, 64, face[2] as u32));
        }
        assert!(sim.battle_order(
            id,
            if control {
                Order::Hold
            } else {
                Order::Dig(face)
            },
            None,
        ));
    }
    let mut times = Vec::with_capacity(ticks as usize);
    let mut text = String::from(
        "tick,total_us,heat_flush_us,hash,mined,cells,inflight,deferred,misses,removed\n",
    );
    // Identical warm-up for both modes; excluded from the statistics.
    for _ in 0..30 {
        sim.step();
    }
    for u in &sim.units {
        assert!(u.status.alive);
        assert_eq!(u.motion.destination, None);
        let dx = u.motion.pos[0].to_int_trunc() - u.at[0];
        let dz = u.motion.pos[2].to_int_trunc() - u.at[2];
        assert!(dx * dx + dz * dz <= 1, "digger left its face");
    }
    for _ in 0..ticks {
        let start = Instant::now();
        let mut last = start;
        let mut dig_phase_us = 0;
        sim.step_observed(|phase| {
            let now = Instant::now();
            if phase == "heat_flush" {
                // Includes the drain, dig credits and hauling, not just dig.
                dig_phase_us = now.duration_since(last).as_micros();
            }
            last = now;
        });
        let us = start.elapsed().as_micros();
        times.push(us);
        assert!(sim.matter.economy.ledger.balanced());
        let mining = sim.mining().unwrap();
        if control || a.case == Case::Empty {
            assert_eq!((mining.mined, mining.cells, mining.in_flight()), (0, 0, 0));
            assert_eq!(sim.world.touched_chunks(), touched);
        } else {
            assert!(mining.mined > 0 && mining.cells > 0, "actual digging");
        }
        let hash = if a.trace { sim.state_hash() } else { 0 };
        text.push_str(&format!(
            "{},{us},{dig_phase_us},{hash:016x},{},{},{},{},{},{}\n",
            sim.tick,
            mining.mined,
            mining.cells,
            mining.in_flight(),
            mining.deferred,
            mining.misses,
            sim.removed
        ));
    }
    for u in &sim.units {
        assert!(u.status.alive);
        let dx = u.motion.pos[0].to_int_trunc() - u.at[0];
        let dz = u.motion.pos[2].to_int_trunc() - u.at[2];
        assert!(dx * dx + dz * dz <= 1, "digger left its face");
    }
    if let Some(csv) = a.csv {
        std::fs::write(csv, text).expect("write diagnostic CSV");
    }
    if let Some(path) = a.snapshot {
        std::fs::write(path, sim.snapshot()).expect("write diagnostic snapshot");
    }
    times.sort_unstable();
    println!(
        "case={:?} units={count} radius={radius} rate={} pool={pool} control={control} ticks={ticks} workers={:?} global={} mean_us={:.3} p99_us={} max_us={} hash={:016x} mined={} cells={} deferred={}",
        a.case,
        a.rate,
        sim.worker_counts(),
        rayon::current_num_threads(),
        times.iter().sum::<u128>() as f64 / f64::from(ticks),
        times[((times.len() * 99).div_ceil(100) - 1).min(times.len() - 1)],
        times[times.len() - 1],
        sim.state_hash(),
        sim.mining().unwrap().mined,
        sim.mining().unwrap().cells,
        sim.mining().unwrap().deferred,
    );
    if a.replay {
        let bytes = sim.snapshot();
        let mut back = BlastRange::restore(config, &bytes).expect("diagnostic snapshot restores");
        assert_eq!(back.snapshot(), bytes);
        assert!(back.set_physics_workers(4));
        assert!(back.set_navigation_workers(4));
        for _ in 0..30 {
            sim.step();
            back.step();
            assert_eq!(sim.state_hash(), back.state_hash());
            assert!(back.matter.economy.ledger.balanced());
        }
        println!("replay=30 passed hash={:016x}", sim.state_hash());
    }
}

/// Actual production prices versus physical volume, without changing balance.
/// Both quantities are printed as integers; no rounding policy is invented.
fn catalog() {
    let config = ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron")).unwrap();
    let mut layout =
        RangeLayout::from_ron(include_str!("../../../../data/fixture-drive.ron")).unwrap();
    let mut economy = RangeEconomy::stock();
    for kind in &mut economy.kinds {
        kind.build_secs = 1;
    }
    economy.production.as_mut().unwrap().producers = vec![Producer {
        kind: RangeUnitKind::Base,
        produces: RangeUnitKind::ALL.to_vec(),
    }];
    layout.economy = Some(economy);
    let sim = BlastRange::with_layout(config, layout).unwrap();
    let density = u64::from(sim.matter.economy.mats.get(3).unwrap().density);
    let denominator = u64::from(voxel::unit::WORLD_MM).pow(3);
    println!("kind,root_mm,cells,production_cost,volume_cost_numerator,volume_cost_denominator");
    for kind in RangeUnitKind::ALL {
        let u = RangeUnit::of_kind(kind, [0; 3]);
        let mut cells = u64::from(u.body.solid_count());
        let mut volume = cells * u64::from(u.body.mm).pow(3);
        for segment in &u.body.segments {
            let n = u64::from(segment.body.solid_count());
            cells += n;
            volume += n * u64::from(segment.body.mm).pow(3);
        }
        let cost = sim
            .production_cost(kind)
            .expect("catalog enables every product");
        let grade=RangeEconomy::stock().kind(kind).unwrap().body_density;
        let expected: u64=std::iter::once(&u.body).chain(u.body.segments.iter().map(|s| &s.body))
            .map(|b| u64::from(b.solid_count()) * (u64::from(grade)*u64::from(b.mm).pow(3)).div_ceil(denominator)).sum();
        assert_eq!(cost, expected, "implemented physical-volume price");
        println!(
            "{kind:?},{},{cells},{cost},{},{denominator}",
            u.body.mm,
            volume * density
        );
    }
    assert!(sim.matter.economy.ledger.balanced());
}
