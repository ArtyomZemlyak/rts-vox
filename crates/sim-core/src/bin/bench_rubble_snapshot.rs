//! Exact rubble-map cost on a saved battle, outside the simulation clock.
use fx::Fx;
use sim_core::{
    blast::{motion::Obstacle, BlastRange},
    burst::{Hull, Solid},
};
use std::{hint::black_box, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let bytes = std::fs::read(args.get(1).expect("snapshot path")).unwrap();
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let mut sim = BlastRange::restore(config, &bytes).expect("battle snapshot");
    if args.iter().any(|a| a == "--profile-tick") {
        let ticks = args
            .windows(2)
            .find(|w| w[0] == "--profile-ticks")
            .map_or(64, |w| w[1].parse().expect("tick count"));
        profile_ticks(&mut sim, ticks);
        return;
    }
    let before = sim.state_hash();
    let voxels: Vec<_> = sim
        .units
        .iter()
        .filter(|u| u.status.alive)
        .map(|u| move |p| u.solid(p))
        .collect();
    let hulls: Vec<_> = sim
        .units
        .iter()
        .filter(|u| u.status.alive)
        .zip(&voxels)
        .map(|(u, solid)| {
            let o = Obstacle::of(&u.motion, &u.body);
            Hull {
                pos: o.pos,
                head: o.head,
                half: o.half,
                height: Fx::from_ratio(
                    u.body.vol.ny as i32 * u.body.mm as i32,
                    voxel::unit::WORLD_MM as i32,
                ),
                vel: [Fx::ZERO; 3],
                step: Fx::from_int(u.motion.step),
                solid: Some(Solid(solid)),
            }
        })
        .collect();
    let yielding = &sim.navigation.fell.materials;
    let mut expected = None;
    let mut times = Vec::new();
    let rounds = args.get(2).map_or(160, |n| n.parse::<usize>().unwrap());
    assert!(rounds > 0, "positive sample count");
    let mut checksum = 0u64;
    let mut columns = 0;
    for round in 0..rounds + 16 {
        let start = Instant::now();
        let rubble = black_box(sim.matter.debris.rubble(&sim.world, yielding, &hulls));
        let ns = start.elapsed().as_nanos();
        if let Some(expected) = &expected {
            assert_eq!(&rubble, expected);
        } else {
            columns = rubble.len();
            for (&[x, z], &top) in &rubble {
                for v in [x as u64, z as u64, u64::from(top)] {
                    checksum = checksum.wrapping_mul(31).wrapping_add(v);
                }
            }
            expected = Some(rubble);
        }
        if round >= 16 {
            times.push(ns);
        }
    }
    assert_eq!(sim.state_hash(), before);
    times.sort_unstable();
    println!("tick={} hash={before:016x} workers={} fragments={} hulls={} columns={columns} rounds={rounds} mean_ns={} p50_ns={} p95_ns={} p99_ns={} checksum={checksum:016x}", sim.tick, rayon::current_num_threads(), sim.matter.debris.fragments.len(), hulls.len(), times.iter().sum::<u128>() / rounds as u128, times[rounds / 2], times[rounds * 95 / 100], times[rounds * 99 / 100]);
}

fn profile_ticks(sim: &mut BlastRange, ticks: u128) {
    use std::{collections::BTreeMap, sync::Mutex};
    let starts = Mutex::new(BTreeMap::new());
    let work = Mutex::new(BTreeMap::<&str, (u128, u128, usize)>::new());
    let mut phases = BTreeMap::<&str, u128>::new();
    for _ in 0..ticks {
        let mut last = Instant::now();
        sim.step_observed_all_tasks(
            |phase| {
                let now = Instant::now();
                *phases.entry(phase).or_default() += now.duration_since(last).as_nanos();
                last = now;
            },
            |_| {},
            |_, _| {},
            |_, _| {},
            |_, event| {
                let worker = rayon::current_thread_index();
                let now = Instant::now();
                if let Some(family) = event.strip_suffix("_begin") {
                    assert!(starts
                        .lock()
                        .unwrap()
                        .insert((worker, family), now)
                        .is_none());
                } else if let Some(family) = event.strip_suffix("_end") {
                    let start = starts.lock().unwrap().remove(&(worker, family)).unwrap();
                    let ns = now.duration_since(start).as_nanos();
                    let mut work = work.lock().unwrap();
                    let value = work.entry(family).or_default();
                    value.0 += ns;
                    value.1 = value.1.max(ns);
                    value.2 += 1;
                } else {
                    unreachable!("physics event {event}");
                }
            },
        );
    }
    assert!(starts.into_inner().unwrap().is_empty());
    for (name, (ns, max, calls)) in work.into_inner().unwrap() {
        println!(
            "task={name} sum_ns_per_tick={} max_ns={max} calls={calls}",
            ns / ticks
        );
    }
    for (name, ns) in phases {
        println!("phase={name} mean_ns={}", ns / ticks);
    }
    println!(
        "profile_final_tick={} hash={:016x}",
        sim.tick,
        sim.state_hash()
    );
}
