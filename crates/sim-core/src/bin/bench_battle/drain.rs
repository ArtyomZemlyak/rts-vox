//! Opt-in diagnostic: cancel new combat/move orders, retain all physical work.
//! Setup is forced here solely to suppress both AI and weapon production. This
//! is not a gameplay lifecycle command or evidence of ordinary scenario timing.
use sim_core::blast::{
    battle::{Order, Phase},
    BlastRange,
};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    time::Instant,
};

fn quiesce(sim: &mut BlastRange) {
    // Setup preserves the battle motion adapter while suppressing battle_orders.
    // Existing flight, waves, kinetic continuations, heat and collapse still run.
    sim.battle.phase = Phase::Setup;
    for (unit, actor) in sim.units.iter_mut().zip(&mut sim.battle.actors) {
        actor.order = Order::Hold;
        unit.motion.route.clear();
        unit.motion.stop();
    }
}

pub(super) fn run(sim: &mut BlastRange, ticks: u32, csv: Option<&str>) {
    sim.tick.checked_add(ticks).expect("drain tick overflow");
    let mut out = csv.map(|path| {
        BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .expect("new drain CSV"),
        )
    });
    if let Some(out) = &mut out {
        writeln!(out, "step,tick,total_us,felling,fragments,flying,claims,rounds,waves,pending_kinetic,arrivals,visits,retired,released,deferrals").unwrap();
    }
    let shots: Vec<_> = sim.battle.actors.iter().map(|a| a.fired).collect();
    println!("drain_fixture forced_setup=true orders=hold physics_preserved=true start_tick={} steps={ticks} start_felling={} start_fragments={}", sim.tick, sim.matter.felling_pending(), sim.matter.debris.fragments.len());
    quiesce(sim);
    let mut previous = sim.matter.admission.felling;
    let mut window = Vec::new();
    let mut phases = super::TickProfile::default();
    for step in 1..=ticks {
        let start = Instant::now();
        let mut last = start;
        sim.step_observed(|phase| {
            let now = Instant::now();
            phases.record_phase(phase, now.duration_since(last).as_micros());
            last = now;
        });
        window.push(start.elapsed().as_micros());
        assert!(
            sim.battle
                .actors
                .iter()
                .zip(&shots)
                .all(|(a, &fired)| a.fired == fired),
            "new fire during drain"
        );
        let work = sim.matter.admission.felling;
        let delta: [u64; 5] = std::array::from_fn(|i| work[i] - previous[i]);
        previous = work;
        if let Some(out) = &mut out {
            writeln!(
                out,
                "{step},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                sim.tick,
                window.last().unwrap(),
                sim.matter.felling_pending(),
                sim.matter.debris.fragments.len(),
                sim.matter
                    .debris
                    .fragments
                    .iter()
                    .filter(|f| !f.resting)
                    .count(),
                sim.matter.debris.claims(),
                sim.battle.rounds.len(),
                sim.battle.wave_progress().count(),
                sim.pending_kinetic_impacts(),
                delta[0],
                delta[1],
                delta[2],
                delta[3],
                delta[4]
            )
            .unwrap();
        }
        if step.is_multiple_of(300) || step == ticks {
            assert!(sim.matter.economy.ledger.balanced(), "drain ledger");
            let mean = window.iter().sum::<u128>() / window.len() as u128;
            window.sort_unstable();
            println!("drain_window step={step} tick={} mean_us={mean} p99_us={} max_us={} felling={} fragments={} flying={} rounds={} waves={} pending_kinetic={}", sim.tick, window[window.len()*99/100], window.last().unwrap(), sim.matter.felling_pending(), sim.matter.debris.fragments.len(), sim.matter.debris.fragments.iter().filter(|f| !f.resting).count(), sim.battle.rounds.len(), sim.battle.wave_progress().count(), sim.pending_kinetic_impacts());
            let n = window.len() as u128;
            println!("drain_phases step={step} unit=mean_us_per_tick observer=true rubble={} motion={} debris={} collapse={} kinetic_resume={} unit_fire={} heat_exchange={} accounting={} observed={}", phases.rubble_us/n, phases.motion_us/n, phases.debris_us/n, phases.collapse_us/n, phases.kinetic_resume_us/n, phases.unit_fire_us/n, phases.heat_exchange_us/n, phases.accounting_us/n, phases.observed_phases_us/n);
            window.clear();
            phases = super::TickProfile::default();
        }
    }
    if let Some(out) = &mut out {
        out.flush().unwrap();
    }
    println!(
        "drain_final tick={} hash={:016x} felling_profile={:?}",
        sim.tick,
        sim.state_hash(),
        sim.matter.felling_profile(&sim.world, &sim.navigation.fell)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn salvo() -> BlastRange {
        let mut layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron"))
                .unwrap();
        let preset = modding::range_skirmish::Skirmish::from_ron(include_str!(
            "../../../../../data/skirmish-canister.ron"
        ))
        .unwrap();
        layout.battle = true;
        layout.units = vec![preset.units[0].clone(), preset.units[3].clone()];
        layout.course = None;
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../../../data/explosion.ron"
        ))
        .unwrap();
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        sim.start_battle().unwrap();
        for _ in 0..160 {
            sim.step();
            if sim.battle.actors[0].fired > 0 {
                return sim;
            }
        }
        panic!("fixture never fired");
    }

    #[test]
    fn quiescence_preserves_physical_matter_and_existing_salvo() {
        let mut sim = salvo();
        let matter = |s: &BlastRange| {
            let mut out = fx::blob::Blob::new();
            s.matter.save(&mut out);
            s.world.save(&mut out);
            s.world.save_transfers(&mut out);
            out.v
        };
        let before = matter(&sim);
        let rounds = sim.battle.rounds.len();
        let waves = sim.battle.wave_progress().count();
        assert!(rounds > 0);
        quiesce(&mut sim);
        assert_eq!(before, matter(&sim));
        assert_eq!(rounds, sim.battle.rounds.len());
        assert_eq!(waves, sim.battle.wave_progress().count());
        assert!(sim.units.iter().all(|u| u.motion.destination.is_none()));
        assert!(sim.battle.actors.iter().all(|a| a.order == Order::Hold));
    }

    #[test]
    fn quiescent_physics_advances_and_replays_without_new_shots() {
        let mut sim = salvo();
        quiesce(&mut sim);
        let shots: Vec<_> = sim.battle.actors.iter().map(|a| a.fired).collect();
        let mut replica =
            BlastRange::restore(sim.config, &sim.snapshot()).expect("quiescent snapshot");
        let mut contacts = 0;
        for _ in 0..180 {
            sim.step();
            replica.step();
            contacts += sim.battle.impacts.len();
            assert_eq!(sim.state_hash(), replica.state_hash());
            assert!(sim
                .battle
                .actors
                .iter()
                .zip(&shots)
                .all(|(a, &fired)| a.fired == fired));
        }
        assert!(contacts > 0, "existing salvo did not reach the world");
        assert!(sim.matter.economy.ledger.balanced());
    }
}
