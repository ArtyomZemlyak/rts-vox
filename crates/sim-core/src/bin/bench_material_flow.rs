//! Small CPU-only lab probe. No window, GPU, battle or focus changes.
use modding::{explosion::ExplosionConfig, laboratory::experiments};
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    let config = ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron")).unwrap();
    for id in ["sand-flow", "soil-slide", "rock-sand"] {
        let e = experiments::stock()
            .into_iter()
            .find(|e| e.id == id)
            .unwrap();
        let mut sim = BlastRange::with_layout(config, e.layout).unwrap();
        sim.control();
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        let (mut total, mut flow) = (Vec::new(), Vec::new());
        let mut scans = 0;
        let mut quiet = None;
        let mut checkpoints = Vec::new();
        for tick in 1..=600 {
            let start = Instant::now();
            let mut previous = start;
            let flow_time = std::sync::Mutex::new(0u128);
            sim.step_observed(|phase| {
                let now = Instant::now();
                if phase == "material_flow" {
                    *flow_time.lock().unwrap() = (now - previous).as_nanos();
                }
                previous = now;
            });
            total.push(start.elapsed().as_nanos());
            flow.push(*flow_time.lock().unwrap());
            scans = scans.max(sim.material_flow().3);
            if [20, 60, 120].contains(&tick) {
                checkpoints.push((tick, sim.material_flow().0));
            }
            if quiet.is_none() && sim.material_flow().2 == 0 && tick > 1 {
                quiet = Some(tick);
            }
        }
        let (moved, pushed, pending, _) = sim.material_flow();
        total.sort_unstable();
        flow.sort_unstable();
        println!("scene={id} ticks=600 first_quiet={quiet:?} movement={checkpoints:?} tick_mean_us={} tick_p95_us={} tick_max_us={} flow_mean_us={} flow_p95_us={} flow_max_us={} max_scans={scans} moved={moved} pushed={pushed} pending={pending} balanced={}",
            total.iter().sum::<u128>()/600/1000,total[569]/1000,total[599]/1000,
            flow.iter().sum::<u128>()/600/1000,flow[569]/1000,flow[599]/1000,sim.matter.economy.ledger.balanced());
    }
}
