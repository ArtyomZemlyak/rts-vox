//! Exact rectangle queries over sleeping-history columns, including index build.
use nav::vehicle::Rubble;
use std::{hint::black_box, time::Instant};

fn main() {
    for n in [64, 8192, 65536, 262144] {
        let columns: Vec<_> = (0..64)
            .flat_map(|x| {
                (0..n / 64).map(move |z| ([x - 32, z - 16], ((x * 17 + z * 31) % 97) as u16))
            })
            .collect();
        let mut builds = Vec::new();
        let mut queries = Vec::new();
        let mut checksum = 0u64;
        for round in 0..24 {
            let input = columns.clone();
            let start = Instant::now();
            let rubble = black_box(Rubble::from_sorted(black_box(input)));
            let build_ns = start.elapsed().as_nanos();
            let start = Instant::now();
            let mut sum = 0u64;
            for i in 0..1024 {
                let lo = [i % 96 - 48, i % 80 - 40];
                let hi = [lo[0] + i % 24, lo[1] + i % 32];
                sum = sum.wrapping_mul(31).wrapping_add(u64::from(
                    black_box(&rubble).upper_in(black_box(lo), black_box(hi)),
                ));
                sum = sum.wrapping_add(u64::from(
                    black_box(&rubble).touches(black_box(lo), black_box(hi)),
                ));
            }
            let query_ns = start.elapsed().as_nanos();
            checksum = black_box(sum);
            if round >= 4 {
                builds.push(build_ns);
                queries.push(query_ns);
            }
        }
        println!(
            "columns={n} queries=2048 rounds={} mean_build_ns={} mean_queries_ns={} checksum={checksum:016x}",
            queries.len(),
            builds.iter().sum::<u128>() / builds.len() as u128,
            queries.iter().sum::<u128>() / queries.len() as u128,
        );
    }
}
