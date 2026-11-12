//! Native pose-query timing; does not measure rendered FPS.
use fx::Fx;
use nav::vehicle::{Probe, Shape};
use std::{hint::black_box, time::Instant};
use voxel::{Generator, World};

struct Flat;
impl Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 { if y < 16 { 2 } else { 0 } }
}
fn main() {
    let w = World::new(2048, 96, 2048, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    for half in [[26,18],[72,72],[200,200]] {
        let shape = Shape { half, support: half.map(|n| n * 3 / 4),
            clearance: Fx::from_int(3), step: 8, min_up: Fx::from_ratio(7,10),
            height: 60, span: 8, ramp: Fx::ONE };
        let mut probe = Probe::with_yield(&w, &[4,5]);
        let begin = Instant::now();
        let mut sum = 0i64;
        for i in 0..10000 {
            let p = [Fx::from_ratio(102400 + i%127,100),Fx::from_ratio(102400 + i%131,100)];
            let pose = black_box(probe.pose(black_box(p), i*137%65536, shape));
            assert_eq!(pose.blocked,0);
            assert!(!pose.steep && !pose.tilt);
            sum = sum.wrapping_add(pose.height.raw());
        }
        println!("pose half={half:?} count=10000 ns_per_query={} scans={} samples={} sum={sum}",
            begin.elapsed().as_nanos()/10000,probe.scans,probe.samples);
    }
}
