//! CPU-only live-range benchmark; no frames, captures or GPU readback.
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    for firing in [false, true] {
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../../data/energy-cannon.ron"
        ))
        .unwrap();
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron"))
                .unwrap();
        let mut scene = BlastRange::with_layout(config, layout).unwrap();
        scene.control();
        let mut samples = Vec::new();
        let mut shots = 0;
        for tick in 0..1800 {
            if firing && tick % 180 == 0 && scene.refire() {
                shots += 1;
            }
            let start = Instant::now();
            scene.step();
            samples.push(start.elapsed().as_micros());
        }
        samples.sort_unstable();
        let laps: Vec<_> = scene
            .units
            .iter()
            .filter(|u| !u.motion.route.is_empty())
            .map(|u| u.motion.laps)
            .collect();
        assert!(laps.iter().any(|n| *n > 0), "no completed patrols");
        if firing {
            assert!(
                scene.removed > 0 && shots > 1,
                "no real repeated destruction"
            );
        }
        println!("firing={firing} ticks={} units={} shots={shots} removed={} laps={laps:?} p50_us={} p99_us={} max_us={} hash={:016x}",
            samples.len(), scene.units.len()+1, scene.removed, samples[samples.len()/2], samples[samples.len()*99/100], samples[samples.len()-1], scene.state_hash());
    }
}
