//! Bounded battle load, including real damage, concurrent waves and bookkeeping.
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-km.ron"))
            .unwrap();
    let t =
        modding::range_battle::Tuning::from_ron(include_str!("../../../../data/range-battle.ron"))
            .unwrap();
    layout.battle = true;
    layout.gun = t.zone.spawn;
    layout.target = t.zone.target;
    layout.zones.push(t.zone);
    layout.units = t.units;
    if let Some(c) = &mut layout.course {
        c.patrols.clear();
    }
    // `--units N`: N/4 groups of the stock four; each pair of groups takes
    // a column, the stock one, one further out and one further in;
    // `--eight` is `--units 8`.
    let args: Vec<_> = std::env::args().collect();
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
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let mut sim = BlastRange::with_layout(config, layout).unwrap_or_else(|e| panic!("layout: {e}"));
    sim.start_battle().unwrap();
    let mut samples = Vec::new();
    let mut peak_rounds = 0;
    let mut phases = std::collections::BTreeMap::<&str, Vec<u128>>::new();
    let ticks = args
        .windows(2)
        .find(|w| w[0] == "--ticks")
        .map_or(900, |w| w[1].parse::<usize>().expect("ticks"));
    assert!(ticks > 0);
    for _ in 0..ticks {
        let start = Instant::now();
        let mut last = start;
        sim.step_observed(|phase| {
            let now = Instant::now();
            let elapsed = now.duration_since(last).as_micros();
            last = now;
            phases.entry(phase).or_default().push(elapsed);
        });
        samples.push(start.elapsed().as_micros());
        peak_rounds = peak_rounds.max(sim.battle.rounds.len());
    }
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
    assert!(sim.matter.economy.ledger.balanced());
    assert_eq!(
        sim.matter
            .economy
            .sunk(sim_core::matter::Sink::Annihilation),
        0
    );
    samples.sort_unstable();
    println!("ticks={} units={} phase={:?} alive={:?} shots={:?} damage={:?} peak_rounds={} p50_us={} p95_us={} p99_us={} max_us={} hash={:016x}",sim.tick,sim.units.len(),sim.battle.phase,sim.battle_counts(),sim.battle.actors.iter().map(|a|a.fired).collect::<Vec<_>>(),sim.units.iter().map(|u|u.revision).collect::<Vec<_>>(),peak_rounds,samples[ticks/2],samples[ticks*95/100],samples[ticks*99/100],samples[ticks-1],sim.state_hash());
    for (id, u) in sim.units.iter().enumerate() {
        println!(
            "unit={id} alive={} engine={} weapon={} core_pct={} at={:?}",
            u.status.alive,
            u.status.engine,
            u.status.weapon,
            u.integrity.core.mul(fx::Fx::from_int(100)).to_int_trunc(),
            u.at
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
}
