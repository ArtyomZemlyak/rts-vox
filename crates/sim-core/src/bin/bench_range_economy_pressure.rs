//! Reclaim index and obstructed production through the common tick (review 247).
//! Authored diagnostic inputs, not additional scenario rules in the kernel.
use modding::explosion::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef, RangeUnitKind};
use modding::range_economy::{Producer, RangeEconomy};
use phys::V3;
use sim_core::blast::{
    battle::commands::{Command, Outcome},
    battle::Order,
    BlastRange, RangeUnit,
};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Reclaim,
    Blocked,
    Construct,
}

struct Args {
    case: Case,
    pieces: usize,
    ticks: u32,
    product: RangeUnitKind,
    control: bool,
    coverage: bool,
    replay: bool,
    csv: Option<String>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut a = Self {
            case: Case::Reclaim,
            pieces: 10000,
            ticks: 120,
            product: RangeUnitKind::Tank,
            control: false,
            coverage: false,
            replay: false,
            csv: None,
        };
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--control" => a.control = true,
                "--coverage" => a.coverage = true,
                "--replay" => a.replay = true,
                "--case" | "--pieces" | "--ticks" | "--product" | "--csv" => {
                    let v = args
                        .next()
                        .ok_or_else(|| format!("missing value for {flag}"))?;
                    match flag.as_str() {
                        "--case" => {
                            a.case = match v.as_str() {
                                "reclaim" => Case::Reclaim,
                                "blocked" => Case::Blocked,
                                "construct" => Case::Construct,
                                _ => return Err(format!("unknown case: {v}")),
                            }
                        }
                        "--pieces" => a.pieces = v.parse().map_err(|_| "invalid --pieces")?,
                        "--ticks" => a.ticks = v.parse().map_err(|_| "invalid --ticks")?,
                        "--product" => {
                            a.product = match v.as_str() {
                                "tank" => RangeUnitKind::Tank,
                                "strider" => RangeUnitKind::Strider,
                                _ => return Err(format!("unknown product: {v}")),
                            }
                        }
                        "--csv" => a.csv = Some(v),
                        _ => unreachable!(),
                    }
                }
                _ => return Err(format!("unknown argument: {flag}")),
            }
        }
        if a.pieces > 40000 || !(1..=10000).contains(&a.ticks) {
            return Err("require pieces <= 40000 and ticks in 1..=10000".into());
        }
        if a.coverage && (a.case != Case::Reclaim || a.pieces != 40000) {
            return Err("--coverage requires --case reclaim --pieces 40000".into());
        }
        Ok(a)
    }
}

fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron")).unwrap()
}

fn layout() -> RangeLayout {
    RangeLayout::from_ron(include_str!("../../../../data/fixture-drive.ron")).unwrap()
}

fn unit(kind: RangeUnitKind, at: [i32; 3]) -> RangeUnitDef {
    RangeUnitDef {
        kind,
        allied: true,
        at,
        weapon: None,
    }
}

fn start(l: RangeLayout) -> BlastRange {
    let mut r = BlastRange::with_layout(config(), l).expect("valid diagnostic inputs");
    assert!(r.set_physics_workers(4));
    assert!(r.set_navigation_workers(4));
    r.start_free_control().unwrap();
    assert!(r.battle_order(0, Order::Hold, None));
    r
}

fn reclaim(a: &Args) -> BlastRange {
    let mut l = layout();
    // Fixed world for every piece count. Separate 16-cell pile bins prevent
    // merging from turning a large-index experiment into a small one.
    l.size = [4096, 128, 4096];
    l.clear_max = [4032, 4032];
    l.units = (0..if a.coverage { 64 } else { 1 })
        .map(|i| {
            unit(
                RangeUnitKind::Tank,
                [100 + (i % 8) * 480, 16, 100 + (i / 8) * 480],
            )
        })
        .collect();
    let mut e = RangeEconomy::stock();
    e.bank = None;
    e.production = None;
    for k in &mut e.kinds {
        k.store = 0;
    }
    let k = e.kind_mut(RangeUnitKind::Tank).unwrap();
    k.tank = if a.coverage { 1 } else { 1 << 30 };
    k.dig_radius = 1;
    k.dig_reach = 1;
    k.dig_edits_per_tick = 1;
    e.reclaim.as_mut().unwrap().reach = if a.coverage { 1024 } else { 2 };
    e.reclaim.as_mut().unwrap().searches_per_tick = 8;
    l.economy = Some(e);
    let mut r = start(l);
    let density = u64::from(r.matter.economy.mats.get(6).unwrap().density);
    let mut cells = Vec::new();
    let boxes: Vec<_> = r.units.iter().map(|u| u.bounds()).collect();
    let mut i = 0;
    while cells.len() < a.pieces {
        let x = 256 + (i % 200) as i32 * 16;
        let z = 256 + (i / 200) as i32 * 16;
        i += 1;
        assert!(z < 4032);
        if a.coverage
            && boxes.iter().any(|(lo, hi)| {
                x >= lo[0] - 2 && x <= hi[0] + 2 && z >= lo[2] - 2 && z <= hi[2] + 2
            })
        {
            continue;
        }
        assert!(r.world.solid(x as u32, 15, z as u32));
        assert!(!r.world.solid(x as u32, 16, z as u32));
        cells.push(([x as u32, 16, z as u32], 6));
    }
    // Use the common debris constructor, including IDs, mass and Ledger
    // transfers. Each batch fits the unchanged flying pool; these initially
    // stationary cubes already have real support and enter the sleeping pool.
    for batch in cells.chunks(1000) {
        let m = &mut r.matter;
        let before = m.debris.fragments.len();
        assert_eq!(
            m.debris
                .pieces_from_cells(&mut r.world, batch, &mut m.economy, &m.tuning, 1),
            (batch.len(), 0)
        );
        for f in &mut m.debris.fragments[before..] {
            assert_eq!(f.mass_g, density * 10);
            assert_eq!(f.velocity, V3::ZERO);
            f.resting = true;
        }
    }
    if !a.control {
        for id in 0..r.units.len() {
            let at = r.units[id].at;
            assert!(r.battle_order(id, Order::Dig([at[0], 96, at[2]]), None));
        }
    }
    r
}

fn blocked(a: &Args) -> BlastRange {
    let mut l = layout();
    let at = [384, 16, 384];
    l.units = vec![unit(RangeUnitKind::Base, at)];
    let (lo, hi) = RangeUnit::of_kind(RangeUnitKind::Base, at).bounds();
    // Solid terrain around an undamaged producer, with a hole for its hull.
    // Even a finished, fully paid product has no legal spot outside that hull.
    let (x0, x1, z0, z1) = (lo[0] - 1, hi[0] + 1, lo[2] - 1, hi[2] + 1);
    for (min, max) in [
        ([64, 16, 64], [x0, 120, 704]),
        ([x1, 16, 64], [704, 120, 704]),
        ([x0, 16, 64], [x1, 120, z0]),
        ([x0, 16, z1], [x1, 120, 704]),
    ] {
        l.blocks.push(RangeBlock {
            min,
            max,
            material: 6,
        });
    }
    let mut e = RangeEconomy::stock();
    e.reclaim = None;
    e.bank.as_mut().unwrap().base_store = 1 << 40;
    e.kind_mut(a.product).unwrap().build_secs = 1;
    e.production.as_mut().unwrap().producers = vec![Producer {
        kind: RangeUnitKind::Base,
        produces: vec![a.product],
    }];
    l.economy = Some(e);
    let mut r = start(l);
    let cost = r.production_cost(a.product).unwrap();
    let store = r.matter.economy.banks[0].stock;
    let e = &mut r.matter.economy;
    e.ledger.reveal(e.ground, cost);
    assert_eq!(e.ledger.transfer(e.ground, store, cost), cost);
    if !a.control {
        assert_eq!(
            r.execute_command(Command::Produce {
                actor: 0,
                kind: a.product
            }),
            Outcome::Applied
        );
    }
    println!(
        "producer_bounds={lo:?}..{hi:?} product={:?} cost={cost}",
        a.product
    );
    r
}

fn check(r: &BlastRange, a: &Args, poses: &[V3]) {
    assert!(r.matter.economy.ledger.balanced());
    assert_eq!(r.units.len(), if a.coverage { 64 } else { 1 });
    for u in &r.units {
        assert!(u.status.alive);
        assert_eq!(u.motion.destination, None);
        assert_eq!(u.motion.pos[0].to_int_trunc(), u.at[0]);
        assert_eq!(u.motion.pos[2].to_int_trunc(), u.at[2]);
    }
    let m = r.mining().unwrap();
    assert_eq!((m.mined, m.cells, m.in_flight()), (0, 0, 0));
    match a.case {
        Case::Reclaim => {
            let rc = m.reclaim().unwrap();
            assert_eq!(rc.reclaimed, 0);
            assert_eq!(
                rc.searches,
                if a.coverage && !a.control {
                    u64::from(r.tick) * 8
                } else {
                    0
                }
            );
            assert_eq!(rc.search_misses, rc.searches);
            assert_eq!(r.matter.debris.fragments.len(), a.pieces);
            for (f, &pose) in r.matter.debris.fragments.iter().zip(poses) {
                assert!(f.resting && f.velocity == V3::ZERO);
                assert_eq!(f.position, pose);
                assert_eq!(
                    r.matter.economy.ledger.held(f.held),
                    r.matter.economy.per_voxel(6)
                );
            }
        }
        Case::Blocked => {
            let p = m.production().unwrap();
            assert_eq!((p.produced, p.stalled, p.capped), (0, 0, 0));
            assert!(r.spawned().is_empty());
            if a.control {
                assert_eq!(p.blocked, 0);
            } else {
                let cost = r.production_cost(a.product).unwrap();
                assert_eq!(r.production_paid(0), Some(cost));
                assert_eq!(r.side_store(true).unwrap().0, 0);
                assert_eq!(p.blocked, u64::from(r.tick - 29));
                assert_eq!(r.production_queue(0), Some(vec![a.product]));
            }
        }
        Case::Construct => unreachable!(),
    }
}

fn run(a: Args) {
    if a.case == Case::Construct {
        // The exact allied probe constructor used by free_spot. This is
        // a microbenchmark, not a simulation tick or a game profile.
        let at = [384, 16, 384];
        drop(RangeUnit::of_kind(a.product, at));
        let mut times = Vec::new();
        for _ in 0..a.ticks {
            let start = Instant::now();
            drop(std::hint::black_box(RangeUnit::of_kind(a.product, at)));
            times.push(start.elapsed().as_micros());
        }
        times.sort_unstable();
        println!(
            "constructor={:?} samples={} mean_us={:.3} p99_us={} max_us={}",
            a.product,
            a.ticks,
            times.iter().sum::<u128>() as f64 / f64::from(a.ticks),
            times[(times.len() * 99).div_ceil(100) - 1],
            times[times.len() - 1]
        );
        return;
    }
    let mut r = match a.case {
        Case::Reclaim => reclaim(&a),
        Case::Blocked => blocked(&a),
        Case::Construct => unreachable!(),
    };
    let poses: Vec<_> = r
        .matter
        .debris
        .fragments
        .iter()
        .map(|f| f.position)
        .collect();
    for _ in 0..30 {
        r.step();
    }
    check(&r, &a, &poses);
    let mut rows = String::from("tick,total_us,heat_flush_us,accounting_us\n");
    let mut times = Vec::new();
    for _ in 0..a.ticks {
        let start = Instant::now();
        let mut last = start;
        let (mut heat, mut accounting) = (0, 0);
        r.step_observed(|phase| {
            let now = Instant::now();
            let us = now.duration_since(last).as_micros();
            if phase == "heat_flush" {
                heat = us;
            }
            if phase == "accounting" {
                accounting = us;
            }
            last = now;
        });
        let us = start.elapsed().as_micros();
        times.push(us);
        rows.push_str(&format!("{},{us},{heat},{accounting}\n", r.tick));
        // Assertions run outside timing; the broad phase intervals include
        // other common work and are not exclusive subsystem stopwatches.
        check(&r, &a, &poses);
    }
    if let Some(path) = &a.csv {
        std::fs::write(path, rows).expect("write CSV");
    }
    times.sort_unstable();
    println!("case={:?} product={:?} pieces={} control={} coverage={} ticks={} workers={:?} global={} mean_us={:.3} p99_us={} max_us={} hash={:016x}",
        a.case, a.product, r.matter.debris.fragments.len(), a.control, a.coverage, a.ticks, r.worker_counts(), rayon::current_num_threads(),
        times.iter().sum::<u128>() as f64 / f64::from(a.ticks),
        times[(times.len() * 99).div_ceil(100) - 1], times[times.len() - 1], r.state_hash());
    if a.replay {
        let bytes = r.snapshot();
        let mut back = BlastRange::restore(config(), &bytes).expect("restore pressure case");
        assert_eq!(back.snapshot(), bytes);
        assert!(back.set_physics_workers(4));
        assert!(back.set_navigation_workers(4));
        for _ in 0..30 {
            r.step();
            back.step();
            assert_eq!(r.state_hash(), back.state_hash());
            check(&r, &a, &poses);
            check(&back, &a, &poses);
        }
        println!("replay=30 passed hash={:016x}", r.state_hash());
    }
}

fn main() {
    match Args::parse() {
        Ok(a) => run(a),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }
}
