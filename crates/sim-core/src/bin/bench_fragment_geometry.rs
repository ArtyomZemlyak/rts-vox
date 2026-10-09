//! Exact fragment geometry query cost; clocks stay outside the simulation.
use fx::{angle, Fx};
use phys::V3;
use sim_core::burst::{shape_centre, Fragment};
use std::{hint::black_box, time::Instant};

fn main() {
    for side in 1..=4u8 {
        let full = u64::MAX >> (64 - u32::from(side).pow(3));
        let pieces: Vec<_> = (0..1024u64)
            .map(|i| {
                let shape = if i % 4 == 0 {
                    full
                } else {
                    (i.wrapping_mul(0x9d37_a518_6fc2_0be1) & full).max(1)
                };
                let head = (i % 16) as i32 * 2731;
                let (c, s) = (angle::cos(head), angle::sin(head));
                Fragment {
                    id: i,
                    position: V3::ZERO,
                    velocity: V3::ZERO,
                    material: 1,
                    paint: None,
                    mm: 100,
                    basis: [
                        V3::new(c, Fx::ZERO, s),
                        V3::UP,
                        V3::new(s.neg(), Fx::ZERO, c),
                    ],
                    mass_g: 1,
                    count: shape.count_ones(),
                    shape,
                    side,
                    chunk: None,
                    held: 0,
                    resting: true,
                }
            })
            .collect();
        for bounds in [false, true] {
            let mut timings = Vec::new();
            let mut checksum = 0u64;
            for round in 0..16 {
                let start = Instant::now();
                let mut sum = 0u64;
                for _ in 0..128 {
                    for f in &pieces {
                        let f = black_box(f);
                        let values = if bounds {
                            let (lo, hi) = f.bounds();
                            [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]]
                        } else {
                            let c = shape_centre(f.shape, f.side);
                            [c.x, c.y, c.z, Fx::ZERO, Fx::ZERO, Fx::ZERO]
                        };
                        for v in black_box(values) {
                            sum = sum.wrapping_mul(31).wrapping_add(v.raw() as u64);
                        }
                    }
                }
                let ns = start.elapsed().as_nanos();
                checksum = black_box(sum);
                if round >= 4 {
                    timings.push(ns);
                }
            }
            println!("side={side} bounds={bounds} queries=131072 rounds={} mean_ns={} checksum={checksum:016x}", timings.len(), timings.iter().sum::<u128>() / timings.len() as u128);
        }
    }
}
