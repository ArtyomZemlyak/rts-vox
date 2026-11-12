//! Shared terrain requirements versus the previous exact tile reduction.
//! Diagnostic wall clocks stay outside the simulation library.
use std::{hint::black_box, time::Instant};
use voxel::{Generator, World};

struct Terrain(u32);
impl Generator for Terrain {
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        let top = match self.0 {
            0 | 4 => 8,
            1 => 8 + x / 16 + z / 32,
            2 => {
                if (128..256).contains(&x) && (128..256).contains(&z) {
                    3
                } else {
                    16
                }
            }
            _ => {
                if x / 32 % 2 == 0 {
                    8
                } else {
                    36
                }
            }
        };
        if y < top {
            2
        } else if self.0 == 4 && x % 9 == 0 && y < top + 12 {
            4
        } else {
            0
        }
    }
}
fn reference(
    w: &World,
    lo: [u32; 2],
    hi: [u32; 2],
    local: bool,
    row: &mut Vec<[u16; 2]>,
    limit: Option<u16>,
) -> u16 {
    let mut min = u16::MAX;
    let mut max = 0;
    let mut step = 0;
    let width = (hi[0] - lo[0]) as usize;
    if local {
        row.resize(width, [u16::MAX, 0]);
        row.fill([u16::MAX, 0]);
    }
    for z in lo[1]..hi[1] {
        let mut left = [u16::MAX, 0];
        for (i, [bottom, top]) in w
            .floor_tile_row(lo[0], z, width, [4, 5])
            .unwrap()
            .enumerate()
        {
            min = min.min(bottom);
            max = max.max(top);
            if local {
                let above = row[i];
                step = step
                    .max(top.saturating_sub(bottom.min(left[0]).min(above[0])))
                    .max(left[1].max(above[1]).saturating_sub(bottom));
                if limit.is_some_and(|cap| step > cap) {
                    return step;
                }
                row[i] = [bottom, top];
                left = [bottom, top];
            }
        }
        if !local && limit.is_some_and(|cap| max - min > cap) {
            return max - min;
        }
    }
    if local {
        step
    } else {
        max - min
    }
}
fn main() {
    for (kind, name) in ["flat", "grade", "crater", "cliffs", "forest"]
        .iter()
        .enumerate()
    {
        let w = World::new(384, 80, 384, 100000, Box::new(Terrain(kind as u32)));
        let init = Instant::now();
        w.prepare_navigation([4, 5]);
        let init_us = init.elapsed().as_micros();
        for local in [false, true] {
            let mut scratch = Vec::new();
            for limit in [2, 16, 32] {
                let mut ns = [0u128; 2];
                let mut pass = [0; 2];
                let mut fallbacks = 0;
                for round in 0..24 {
                    for which in [round % 2, 1 - round % 2] {
                        let start = Instant::now();
                        let mut count = 0;
                        let mut unclear = 0;
                        for i in 0..128u32 {
                            let lo = [1 + i % 17, 1 + (i * 7) % 17];
                            let hi = [lo[0] + 24, lo[1] + 24];
                            let ok = if which == 0 {
                                reference(&w, lo, hi, local, &mut scratch, Some(limit)) <= limit
                            } else {
                                w.prepared_floor_passage(lo, hi, [4, 5], local, i32::from(limit))
                                    .unwrap_or_else(|| {
                                        unclear += 1;
                                        reference(&w, lo, hi, local, &mut scratch, Some(limit))
                                            <= limit
                                    })
                            };
                            count += u32::from(black_box(ok));
                        }
                        if round >= 4 {
                            ns[which] += start.elapsed().as_nanos();
                            pass[which] = count;
                            if which == 1 {
                                fallbacks = unclear;
                            }
                        }
                    }
                }
                assert_eq!(pass[0], pass[1]);
                println!("case={name} local={local} step={limit} queries=128 rounds=20 prepare_us={init_us} reference_ns={} cached_ns={} passed={} fallbacks={fallbacks}",ns[0]/20,ns[1]/20,pass[0]);
            }
        }
    }
}
