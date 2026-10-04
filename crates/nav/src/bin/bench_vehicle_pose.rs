//! Reproducible broad-hull clearance workload, separate from battle FPS.
use fx::{angle, Fx};
use nav::vehicle::{Probe, Rubble, Shape};
use std::{hint::black_box, time::Instant};
use voxel::{Generator, World};

struct Grade;
impl Generator for Grade {
    fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
        u16::from(y < 10 + x / 16)
    }
}

fn main() {
    let world = World::new(384, 96, 384, 100, Box::new(Grade));
    world.prepare_navigation([4, 5]);
    let shape = Shape {
        half: [88, 80],
        support: [50, 60],
        clearance: Fx::from_int(38),
        step: 14,
        min_up: Fx::from_ratio(84, 100),
        height: 20,
        span: 12,
        ramp: Fx::from_ratio(1, 2),
    };
    // Warm and measured rounds rotate cases; rebuilding the per-pose probe
    // is included. No world generation, tile preparation or log I/O is timed.
    let cases = [
        ("bare", Rubble::new()),
        ("one_low_heap", Rubble::from([([192, 192], 50)])),
        ("one_high_heap", Rubble::from([([192, 192], 82)])),
        (
            "sparse_heaps",
            Rubble::from([([120, 160], 50), ([192, 192], 50), ([240, 220], 50)]),
        ),
    ];
    let mut times: [Vec<u128>; 4] = std::array::from_fn(|_| Vec::new());
    let mut reads = [[0u64; 3]; 4];
    let mut hashes = [0u64; 4];
    for round in 0..220usize {
        for offset in 0..cases.len() {
            let case = (round + offset) % cases.len();
            let start = Instant::now();
            let mut totals = [0u64; 3];
            let mut hash = 0xcbf29ce484222325u64;
            for i in 0..64 {
                let pos = [
                    Fx::from_int(190 + i % 5).add(Fx::from_ratio(i % 3, 3)),
                    Fx::from_int(190 + i % 7).sub(Fx::from_ratio(i % 5, 5)),
                ];
                let head = [0, 1234, angle::FULL / 8, angle::FULL / 3][i as usize % 4];
                let mut probe =
                    Probe::with_yield(black_box(&world), &[4, 5]).with_rubble(&cases[case].1);
                let pose = black_box(probe.pose(black_box(pos), black_box(head), black_box(shape)));
                for v in [
                    pose.height.raw(),
                    pose.floors[0].raw(),
                    pose.floors[1].raw(),
                    pose.floors[2].raw(),
                    pose.floors[3].raw(),
                    pose.blocked as i64,
                    i64::from(pose.steep),
                    i64::from(pose.tilt),
                    pose.crush as i64,
                ] {
                    hash = (hash ^ v as u64).wrapping_mul(0x100000001b3);
                }
                for (sum, count) in
                    totals
                        .iter_mut()
                        .zip([probe.columns, probe.scans, probe.samples])
                {
                    *sum += count;
                }
            }
            let elapsed = start.elapsed().as_nanos();
            if round >= 20 {
                times[case].push(elapsed);
                reads[case] = totals;
                hashes[case] = hash;
            }
        }
    }
    for i in 0..cases.len() {
        times[i].sort_unstable();
        println!("case={} poses=64 rounds={} mean_batch_ns={} p99_batch_ns={} max_batch_ns={} columns={} scans={} surfaces={} hash={:016x}", cases[i].0, times[i].len(), times[i].iter().sum::<u128>() / times[i].len() as u128, times[i][times[i].len() * 99 / 100], times[i][times[i].len()-1], reads[i][0], reads[i][1], reads[i][2], hashes[i]);
    }
}
