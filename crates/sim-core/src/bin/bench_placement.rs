//! Cached layout validation: map-independent work on the editor hot path.
use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};
use sim_core::blast::placement::Validator;
use std::time::Instant;

fn main() {
    let start = Instant::now();
    let validator = Validator::default();
    println!("template_us={}", start.elapsed().as_micros());
    for side in [640, 10000] {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron")).unwrap();
        layout.size = [side, 192, side];
        for at in [
            [550, 16, 550],
            [250, 16, 550],
            [550, 16, 505],
            [550, 16, 445],
        ] {
            layout.units.push(RangeUnitDef {
                kind: RangeUnitKind::Tank,
                at,
            });
        }
        let mut samples = Vec::new();
        for n in 0..1000 {
            layout.units[7].at[0] = 548 + n % 3 * 2;
            let start = Instant::now();
            validator.validate(&layout).unwrap();
            samples.push(start.elapsed().as_micros());
        }
        samples.sort_unstable();
        println!(
            "side={side} units={} samples={} p50_us={} p99_us={} max_us={}",
            layout.units.len(),
            samples.len(),
            samples[500],
            samples[990],
            samples[999]
        );
    }
}
