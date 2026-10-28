//! Short headless probes; these timings are CPU simulation cost, never FPS.
use modding::{explosion::ExplosionConfig, laboratory::experiments};
use sim_core::blast::{
    battle::commands::{Command, GunCommand},
    BlastRange,
};
use std::time::Instant;
fn main() {
    let capillary = !std::env::args().any(|a| a == "--no-capillary");
    println!("capillary={capillary}");
    for id in [
        "water-fall",
        "water-vessels",
        "water-float",
        "wetting",
        "water-erosion",
        "vitrification",
        "drill-crawler",
        "buried-dome",
    ] {
        let e = experiments::stock()
            .into_iter()
            .find(|e| e.id == id)
            .unwrap();
        let c = ExplosionConfig::from_ron(if e.weapon == 3 {
            include_str!("../../../../data/laser.ron")
        } else {
            include_str!("../../../../data/explosion.ron")
        })
        .unwrap();
        let mut sim = BlastRange::with_layout(c, e.layout).unwrap();
        sim.control();
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        if !capillary {
            sim.water.tuning.capillary_units = 0;
        }
        for input in e.plan.inputs {
            match input {
                experiments::Input::Fire => {
                    sim.execute_command(Command::Gun(GunCommand::Refire));
                }
                experiments::Input::Detonate { at } => {
                    sim.execute_command(Command::Gun(GunCommand::Detonate { at }));
                }
                _ => {}
            }
        }
        let mut peak_bodies = 0;
        let mut peak_wood = 0;
        if id == "water-float" {
            println!(
                "float_initial_solid={} material={}",
                sim.world.solid(376, 20, 330),
                sim.world.material(376, 20, 330)
            );
        }
        let mut samples = Vec::new();
        let mut fluid = Vec::new();
        for _ in 0..600 {
            let begin = Instant::now();
            let mut previous = begin;
            let mut water = 0;
            sim.step_observed(|phase| {
                let now = Instant::now();
                if phase == "water_flow" {
                    water = (now - previous).as_nanos();
                }
                previous = now;
            });
            peak_bodies = peak_bodies.max(sim.bodies.list.len());
            peak_wood = peak_wood.max(
                sim.bodies
                    .list
                    .iter()
                    .filter(|b| b.voxels.first().is_some_and(|v| v.mat == 5))
                    .count(),
            );
            samples.push(begin.elapsed().as_nanos());
            fluid.push(water);
        }
        samples.sort_unstable();
        fluid.sort_unstable();
        let w = sim.water.stats();
        let dug: u64 = sim
            .layout
            .as_ref()
            .unwrap()
            .lab_devices
            .iter()
            .map(|d| d.dug)
            .sum();
        println!("scene={id} ticks=600 mean_us={} p95_us={} max_us={} water_mean_us={} water_p95_us={} water_max_us={} water_conserved={} absorbed={} vapour={} melted={} device_dug={dug} bodies={} balanced={}",
            samples.iter().sum::<u128>()/600/1000,samples[569]/1000,samples[599]/1000,
            fluid.iter().sum::<u128>()/600/1000,fluid[569]/1000,fluid[599]/1000,w.units==w.initial_units,w.absorbed_units,w.vapor_units,sim.heat.melted_total,sim.bodies.list.len(),sim.matter.economy.ledger.balanced());
        if id == "water-erosion" {
            let holes = (384..388)
                .flat_map(|x| (326..346).map(move |z| (x, z)))
                .map(|(x, z)| {
                    (17..19)
                        .filter(|&y| sim.world.material(x, y, z) == 0)
                        .count()
                })
                .sum::<usize>();
            println!(
                "erosion_bank_holes={holes} granular_moved={}",
                sim.material_flow().0
            );
            assert!(holes > 0, "water-erosion did not reshape its bank");
        }
        if id == "water-float" {
            let floats: Vec<_> = sim
                .bodies
                .list
                .iter()
                .filter(|b| b.voxels.first().is_some_and(|v| v.mat == 5))
                .map(|b| b.oy)
                .collect();
            println!("float_peak_bodies={peak_bodies} float_peak_wood={peak_wood} collapsed={} final_float_heights={floats:?}",sim.collapsed_cells);
            assert!(!floats.is_empty(), "wood lost buoyancy in shared scene");
        }
        assert_eq!(w.units, w.initial_units);
        assert!(sim.matter.economy.ledger.balanced());
    }
}
