//! Phase-local hull index build and first-contact queries, including misses.
use fx::Fx;
use sim_core::burst::{Hull, HullQueries};
use std::{hint::black_box, time::Instant};

fn main() {
    for n in [8usize, 60, 256, 1024] {
        let hulls: Vec<_> = (0..n)
            .map(|i| Hull {
                pos: [
                    Fx::from_int((i % 32) as i32 * 128),
                    Fx::from_int(12),
                    Fx::from_int((i / 32) as i32 * 128),
                ],
                head: i as i32 * 913,
                half: [Fx::from_int(24), Fx::from_int(18)],
                height: Fx::from_int(30),
                vel: [Fx::from_int(i as i32 + 1), Fx::ZERO, Fx::ZERO],
                step: Fx::from_int(4),
                solid: None,
            })
            .collect();
        let mut builds = Vec::new();
        let mut queries = Vec::new();
        let mut checksum = 0u64;
        for round in 0..24 {
            let start = Instant::now();
            let index = black_box(HullQueries::new(black_box(&hulls)));
            let build_ns = start.elapsed().as_nanos();
            let start = Instant::now();
            let mut sum = 0u64;
            for i in 0..4096usize {
                let h = &hulls[i % n];
                let p = [
                    h.pos[0].add(Fx::from_int((i % 3) as i32 * 32)),
                    Fx::from_int(18),
                    h.pos[2].add(Fx::from_ratio((i % 17) as i32, 5)),
                ];
                let hit = black_box(&index).hit(
                    black_box(p),
                    [Fx::from_int(-2); 3],
                    [Fx::from_int(2); 3],
                    0,
                );
                sum = sum
                    .wrapping_mul(31)
                    .wrapping_add(hit.map_or(0, |h| h.vel[0].raw() as u64));
            }
            let query_ns = start.elapsed().as_nanos();
            checksum = black_box(sum);
            if round >= 4 {
                builds.push(build_ns);
                queries.push(query_ns);
            }
        }
        println!("hulls={n} queries=4096 rounds={} mean_build_ns={} mean_queries_ns={} checksum={checksum:016x}", queries.len(), builds.iter().sum::<u128>() / builds.len() as u128, queries.iter().sum::<u128>() / queries.len() as u128);
    }
}
