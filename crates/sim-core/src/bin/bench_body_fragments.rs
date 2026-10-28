//! Identical removed armour and isolated debris physics, legacy vs panels.
//! Timing stays outside the deterministic simulation.
use fx::Fx;
use phys::V3;
use sim_core::{burst::Surroundings, Tuning, World};
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let ticks: u64 = args
        .windows(2)
        .find(|w| w[0] == "--ticks")
        .map_or(600, |w| w[1].parse().expect("ticks"));
    let stock = Tuning::from_ron(include_str!("../../../../data/sim.ron")).unwrap();
    assert!(ticks > 0);
    let spans: Vec<u32> = args.windows(2).find(|w| w[0] == "--spans-mm").map_or_else(
        || vec![0, stock.body_fragment_mm],
        |w| {
            w[1].split(',')
                .map(|s| s.parse().expect("fragment span in mm"))
                .collect()
        },
    );
    assert!(!spans.is_empty() && spans.iter().all(|&s| s <= u16::MAX as u32));
    for span in spans {
        let mut tuning = stock;
        tuning.body_fragment_mm = span;
        let mut world = World::new(7, 16, tuning);
        let kinds =
            modding::kinds::from_ron(include_str!("../../../../data/kinds.ron"), 30).unwrap();
        let mats =
            modding::data::materials_from_ron(include_str!("../../../../data/materials.ron"))
                .unwrap();
        world.attach_economy(kinds, mats);
        let unit = world.spawn_kind(60, 60, 0, 1).unwrap();
        world.units.y[unit as usize] = 80;
        world.units.refresh_live();
        let bodies = world.material_bodies.as_mut().unwrap();
        let body = bodies.body(unit).unwrap();
        let mut cells = Vec::new();
        for z in 0..body.vol.nz {
            for y in 0..body.vol.ny {
                for x in 0..body.vol.nx {
                    if body.vol.get(x as usize, y as usize, z as usize)
                        && body.part_at(x as usize, y as usize, z as usize) == voxel::Part::Armour
                        && cells.len() < 512
                    {
                        cells.push([x, y, z]);
                    }
                }
            }
        }
        let volume = u64::from(body.mm).pow(3) * cells.len() as u64;
        let fracture = u64::from(tuning.impact.work_kj_m3[0]) * volume / 1000;
        let generation = bodies.state(unit).unwrap().generation;
        assert!(bodies.queue_cells(
            unit,
            generation,
            cells.clone(),
            fracture,
            1_000_000,
            V3::UP,
            tuning.impact.max_pending
        ));
        let start = Instant::now();
        world.tick();
        let export_us = start.elapsed().as_micros();
        let pieces = &world.bursts.fragments;
        let count = pieces.len();
        let chunks = pieces.iter().filter(|f| f.chunk.is_some()).count();
        let max_cells = pieces.iter().map(|f| f.count).max().unwrap_or(0);
        let emitted: u32 = pieces.iter().map(|f| f.count).sum();
        assert_eq!(emitted as usize, cells.len());
        let mass: u64 = pieces.iter().map(|f| f.mass_g).sum();
        let matter: u64 = pieces
            .iter()
            .map(|f| world.econ.as_ref().unwrap().ledger.held(f.held))
            .sum();
        let ground = voxel::World::new(128, 128, 128, 100, Box::new(Flat));
        let economy = world.econ.as_mut().unwrap();
        let mut samples = Vec::new();
        for tick in 0..ticks {
            let start = Instant::now();
            world.bursts.step_fragments(
                &ground,
                economy,
                &tuning.blast,
                Fx::from_ratio(tuning.gravity, sim_core::TICKS_PER_SECOND.pow(2)),
                tick,
                Surroundings::NONE,
            );
            samples.push(start.elapsed().as_nanos());
        }
        assert!(economy.ledger.balanced());
        assert_eq!(
            world.bursts.fragments.iter().map(|f| f.mass_g).sum::<u64>(),
            mass
        );
        assert_eq!(
            world
                .bursts
                .fragments
                .iter()
                .map(|f| economy.ledger.held(f.held))
                .sum::<u64>(),
            matter
        );
        let sum: u128 = samples.iter().sum();
        let early = (ticks.min(120)) as usize;
        let early_mean = samples.iter().take(early).sum::<u128>() / early as u128;
        samples.sort_unstable();
        let flying = world.bursts.fragments.iter().filter(|f| !f.resting).count();
        println!("span_mm={span} removed={} emitted={emitted} pieces={count} chunks={chunks} max_cells={max_cells} mass_g={mass} matter={matter} export_tick_us={export_us} physics_ticks={ticks} physics_mean_ns={} physics_p99_ns={} first_120_mean_ns={early_mean} final_pieces={} final_flying={flying} balanced=true", cells.len(), sum / samples.len() as u128, samples[samples.len()*99/100], world.bursts.fragments.len());
    }
}
struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _x: u32, y: u32, _z: u32) -> u16 {
        u16::from(y < 1)
    }
}
