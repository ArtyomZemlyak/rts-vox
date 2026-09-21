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
    if std::env::args().any(|a| a == "--eight") {
        let more: Vec<_> = layout
            .units
            .iter()
            .cloned()
            .map(|mut u| {
                u.at[2] += 90;
                u
            })
            .collect();
        layout.units.extend(more);
    }
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let mut sim = BlastRange::with_layout(config, layout).unwrap();
    sim.start_battle().unwrap();
    let mut samples = Vec::new();
    let mut peak_rounds = 0;
    let mut phases = std::collections::BTreeMap::<&str, u128>::new();
    let args: Vec<_> = std::env::args().collect();
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
            let peak = phases.entry(phase).or_default();
            *peak = (*peak).max(elapsed);
        });
        samples.push(start.elapsed().as_micros());
        peak_rounds = peak_rounds.max(sim.battle.rounds.len());
    }
    assert!(
        sim.battle.actors.iter().all(|a| a.fired > 0),
        "an actor never fired"
    );
    assert!(
        sim.units.iter().all(|u| u.revision > 0),
        "an actor never took spatial damage"
    );
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
    for (name, us) in phases {
        println!("phase={name} max_us={us}");
    }
}
