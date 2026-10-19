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
    if args.iter().any(|a| a == "--settle-fragments") {
        settle_fragments(&mut sim);
        if args.iter().any(|a| a == "--trace-wakeups") {
            trace_wakeups(&mut sim);
        }
        return;
    }
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
    if args.iter().any(|a| a == "--support-levels") {
        let levels: Vec<_> = hulls
            .iter()
            .map(|h| {
                if h.solid.is_some() {
                    (h.pos[1].raw() >> 32) + 1
                } else {
                    h.pos[1].add(h.height).raw() >> 32
                }
            })
            .collect();
        let mut hist = std::collections::BTreeMap::<usize, usize>::new();
        for f in sim.matter.debris.fragments.iter().filter(|f| f.resting) {
            let reach = f
                .position
                .y
                .add(f.bounds().0[1])
                .add(Fx::from_ratio(1, 2))
                .raw()
                >> 32;
            *hist
                .entry(levels.iter().filter(|&&y| y <= reach).count())
                .or_default() += 1;
        }
        println!("tick={} hash={before:016x} hulls={} roof_levels={levels:?} resting_eligibility={hist:?}",sim.tick,hulls.len());
        assert_eq!(sim.state_hash(), before);
        return;
    }
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

/// Stop firing, terrain edits and hull motion to distinguish a busy battle
/// from a debris pile that keeps moving without any external input.
fn settle_fragments(sim: &mut BlastRange) {
    let tuning = sim_core::Tuning::from_ron(include_str!("../../../../data/sim.ron")).unwrap();
    let gravity = sim_core::gravity_per_tick(&tuning);
    let mut elapsed = 0;
    let mut wakeups = sim.matter.debris.wakeups;
    for tick in 0..1800 {
        let start = Instant::now();
        sim.matter.debris.step_fragments(
            &sim.world,
            &mut sim.matter.economy,
            &sim.matter.tuning,
            gravity,
            u64::from(sim.tick + tick),
            sim_core::burst::Surroundings::NONE,
        );
        elapsed += start.elapsed().as_micros();
        if (tick + 1) % 300 == 0 {
            let debris = &sim.matter.debris;
            println!(
                "settle_tick={} fragments={} flying={} wakeups={} mean_us={} balanced={}",
                tick + 1,
                debris.fragments.len(),
                debris.fragments.iter().filter(|f| !f.resting).count(),
                debris.wakeups - wakeups,
                elapsed / 300,
                sim.matter.economy.balanced()
            );
            elapsed = 0;
            wakeups = debris.wakeups;
        }
    }
}

fn trace_wakeups(sim: &mut BlastRange) {
    let tuning = sim_core::Tuning::from_ron(include_str!("../../../../data/sim.ron")).unwrap();
    let gravity = sim_core::gravity_per_tick(&tuning);
    let mut blast = sim.matter.tuning;
    // Diagnostic only: one unique ID is due per probe, so a wake-and-sleep
    // within the same tick can be attributed without instrumenting the kernel.
    blast.rest_check_ticks = u32::MAX;
    let ids: Vec<_> = sim.matter.debris.fragments.iter().map(|f| f.id).collect();
    for id in ids {
        let before = sim.matter.debris.wakeups;
        let tick = u64::from(u32::MAX) - id % u64::from(u32::MAX);
        sim.matter.debris.step_fragments(
            &sim.world,
            &mut sim.matter.economy,
            &blast,
            gravity,
            tick,
            sim_core::burst::Surroundings::NONE,
        );
        if sim.matter.debris.wakeups > before {
            let f = sim
                .matter
                .debris
                .fragments
                .iter()
                .find(|f| f.id == id)
                .unwrap();
            println!("wake_loop id={id} pos={:?} velocity={:?} mm={} material={} shape={:016x} side={} basis={:?} bounds={:?}",
                f.position, f.velocity, f.mm, f.material, f.shape, f.side, f.basis, f.bounds());
        }
    }
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
