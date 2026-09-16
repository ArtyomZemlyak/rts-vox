//! CPU-only live-range benchmark; no frames, captures or GPU readback.
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    // `--weapon shell` measures the heavier debris of the shell (ADR-296).
    let args: Vec<String> = std::env::args().collect();
    let weapon = args
        .iter()
        .position(|a| a == "--weapon")
        .and_then(|i| args.get(i + 1).cloned())
        .unwrap_or_else(|| "energy".to_string());
    let ron = match weapon.as_str() {
        "energy" => include_str!("../../../../data/energy-cannon.ron"),
        "shell" => include_str!("../../../../data/explosion.ron"),
        "anni" => include_str!("../../../../data/anni-cannon.ron"),
        _ => panic!("--weapon expects energy, shell or anni"),
    };
    for firing in [false, true] {
        let config = modding::explosion::ExplosionConfig::from_ron(ron).unwrap();
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
        // The slowest ticks by number: a spike names its moment.
        let mut top: Vec<(u128, usize)> = samples.iter().copied().zip(0..).collect();
        top.sort_unstable_by(|a, b| b.cmp(a));
        let top: Vec<String> = top
            .iter()
            .take(5)
            .map(|(us, tick)| format!("{tick}:{us}"))
            .collect();
        println!(
            "weapon={weapon} firing={firing} slowest_ticks_us={}",
            top.join(",")
        );
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
