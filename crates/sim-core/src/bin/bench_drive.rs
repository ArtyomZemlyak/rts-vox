//! CPU-only benchmark of the range's driving orders (P-17): no frames, no
//! GPU. First the planning time of a move order to each of several
//! points, then a drive over a fresh shell crater, timing every tick and
//! naming the slowest ones. `--targets x,z;x,z` picks the points,
//! `--crater x,z` where the shell lands, `--to x,z` where the tank then
//! drives, `--ticks n` how long, `--nodes n` and `--heuristic pct` the
//! planner's budget and weight instead of the data's.
use sim_core::blast::BlastRange;
use std::time::Instant;

fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1).cloned())
}
fn point(s: &str) -> [i32; 2] {
    let mut it = s
        .split(',')
        .map(|v| v.trim().parse::<i32>().expect("integer"));
    [it.next().expect("x"), it.next().expect("z")]
}
fn range(search: Option<(usize, u32)>, layout: &modding::range::RangeLayout) -> BlastRange {
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let mut scene = BlastRange::with_layout(config, layout.clone()).unwrap();
    scene.control();
    if let Some((nodes, pct)) = search {
        scene.gun.motion.set_search(nodes, pct);
    }
    scene
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let layout = arg(&args, "--range")
        .map(|p| std::fs::read_to_string(p).expect("range file"))
        .unwrap_or_else(|| include_str!("../../../../data/range-cover.ron").into());
    let layout = modding::range::RangeLayout::from_ron(&layout).expect("range layout");
    let ticks: usize = arg(&args, "--ticks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let targets =
        arg(&args, "--targets").unwrap_or_else(|| "235,365;128,470;200,336;340,336".into());
    let crater = point(&arg(&args, "--crater").unwrap_or_else(|| "200,336".into()));
    let to = point(&arg(&args, "--to").unwrap_or_else(|| "260,336".into()));
    let nodes: Option<usize> = arg(&args, "--nodes").and_then(|s| s.parse().ok());
    let pct: Option<u32> = arg(&args, "--heuristic").and_then(|s| s.parse().ok());
    let search =
        (nodes.is_some() || pct.is_some()).then(|| (nodes.unwrap_or(16000), pct.unwrap_or(100)));

    // Planning alone: the order's cost and whether a route came out.
    for t in targets.split(';').filter(|t| !t.is_empty()) {
        let target = point(t);
        let mut scene = range(search, &layout);
        let start = Instant::now();
        let ok = scene.order_move(target);
        let us = start.elapsed().as_micros();
        let mut slices = 0;
        let mut slice_us = 0;
        while scene.gun.motion.searching() && slices < 10000 {
            let started = Instant::now();
            scene.step();
            slice_us = slice_us.max(started.elapsed().as_micros());
            slices += 1;
        }
        assert!(!scene.gun.motion.searching(), "search never completed");
        println!(
            "plan to=({},{}) ok={ok} us={us} slices={slices} max_slice_us={slice_us} nodes={} poses={} scans={} samples={} path_len={} blocked={}",
            target[0],
            target[1],
            scene.gun.motion.last_plan[0],
            scene.gun.motion.last_plan[1],
            scene.gun.motion.last_probe[0],
            scene.gun.motion.last_probe[1],
            scene.gun.motion.path.len(),
            scene.gun.motion.blocked
        );
    }
    if args.iter().any(|a| a == "--plan-only") {
        return;
    }

    // The drive: a shell on the open ground, then the order across it.
    for with_crater in [false, true] {
        let mut scene = range(search, &layout);
        let mut samples: Vec<(u128, usize)> = Vec::new();
        let mut tick = 0usize;
        if with_crater {
            assert!(
                scene.order_ground_fire([crater[0], 16, crater[1]]),
                "the gun is not ready"
            );
            for _ in 0..150 {
                let start = Instant::now();
                scene.step();
                samples.push((start.elapsed().as_micros(), tick));
                tick += 1;
            }
            assert!(scene.impact.is_some(), "the shell never landed");
            assert!(scene.removed > 0, "no crater");
        }
        let plan = Instant::now();
        let ok = scene.order_move(to);
        let plan_us = plan.elapsed().as_micros();
        let mut trail = Vec::new();
        for _ in 0..ticks {
            let start = Instant::now();
            scene.step();
            samples.push((start.elapsed().as_micros(), tick));
            tick += 1;
            if tick.is_multiple_of(100) {
                trail.push(format!(
                    "{tick}:({},{})",
                    scene.gun.motion.pos[0].to_int_trunc(),
                    scene.gun.motion.pos[2].to_int_trunc()
                ));
            }
        }
        let mut top = samples.clone();
        top.sort_unstable_by(|a, b| b.cmp(a));
        let top: Vec<String> = top
            .iter()
            .take(8)
            .map(|(us, t)| format!("{t}:{us}"))
            .collect();
        let mut sorted: Vec<u128> = samples.iter().map(|s| s.0).collect();
        sorted.sort_unstable();
        let n = sorted.len();
        println!(
            "drive crater={with_crater} removed={} plan_ok={ok} plan_us={plan_us} path_len={} p50_us={} p99_us={} max_us={} slowest={} trail={} hash={:016x}",
            scene.removed,
            scene.gun.motion.path.len(),
            sorted[n / 2],
            sorted[n * 99 / 100],
            sorted[n - 1],
            top.join(","),
            trail.join(" "),
            scene.state_hash()
        );
    }
}
