//! Empty-face digging cost through the common range tick (review 246).
//! Diagnostic inputs only: no scenario-specific simulation rules.
use modding::explosion::ExplosionConfig;
use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind, MAX_RANGE_UNITS};
use modding::range_economy::{Producer, RangeEconomy, MAX_DIG_RADIUS};
use sim_core::blast::{battle::Order, BlastRange, RangeUnit};
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--catalog") {
        catalog();
        return;
    }
    let number = |flag: &str, default| {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map_or(default, |w| w[1].parse::<u32>().expect(flag))
    };
    let count = number("--units", 64) as usize;
    let radius = number("--radius", 32);
    let ticks = number("--ticks", 120);
    assert!((1..=MAX_RANGE_UNITS).contains(&count));
    assert!((1..=MAX_DIG_RADIUS).contains(&radius));
    assert!(ticks > 0);
    let control = args.iter().any(|a| a == "--control");
    let replay = args.iter().any(|a| a == "--replay");
    let csv = args.windows(2).find(|w| w[0] == "--csv");
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
    kind.dig_edits_per_tick = 1;
    layout.economy = Some(economy);
    let mut sim = BlastRange::with_layout(config, layout).expect("valid diagnostic layout");
    assert!(sim.set_physics_workers(4));
    assert!(sim.set_navigation_workers(4));
    sim.start_free_control().unwrap();
    let touched = sim.world.touched_chunks();
    for id in 0..count {
        let u = &sim.units[id];
        let face = [u.at[0], 96, u.at[2]];
        // The entire ball is above the clearing's ground. No edit, routing,
        // hauling or reclaim should be needed, even at the largest radius.
        assert!(!sim.world.solid(face[0] as u32, 64, face[2] as u32));
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
    let mut text = String::from("tick,total_us,heat_flush_us\n");
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
        text.push_str(&format!("{},{us},{dig_phase_us}\n", sim.tick));
        assert!(sim.matter.economy.ledger.balanced());
        let mining = sim.mining().unwrap();
        assert_eq!((mining.mined, mining.cells, mining.in_flight()), (0, 0, 0));
        assert_eq!(sim.world.touched_chunks(), touched);
    }
    for u in &sim.units {
        assert!(u.status.alive);
        let dx = u.motion.pos[0].to_int_trunc() - u.at[0];
        let dz = u.motion.pos[2].to_int_trunc() - u.at[2];
        assert!(dx * dx + dz * dz <= 1, "digger left its face");
    }
    if let Some(csv) = csv {
        std::fs::write(&csv[1], text).expect("write diagnostic CSV");
    }
    times.sort_unstable();
    println!(
        "units={count} radius={radius} control={control} ticks={ticks} workers={:?} global={} mean_us={:.3} p99_us={} max_us={} hash={:016x} mined=0",
        sim.worker_counts(),
        rayon::current_num_threads(),
        times.iter().sum::<u128>() as f64 / f64::from(ticks),
        times[((times.len() * 99).div_ceil(100) - 1).min(times.len() - 1)],
        times[times.len() - 1],
        sim.state_hash(),
    );
    if replay {
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
        assert_eq!(cost, cells * density, "implemented cell-count price");
        println!(
            "{kind:?},{},{cells},{cost},{},{denominator}",
            u.body.mm,
            volume * density
        );
    }
    assert!(sim.matter.economy.ledger.balanced());
}
