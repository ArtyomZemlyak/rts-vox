//! CPU-only measurement of how debris stands against debris (criterion 2
//! of the physics goal): a shell on open ground, then every tick the
//! pieces that share space with another piece, how deep, and what a grid
//! of buckets costs to find them. `--weapon`, `--power`, `--ticks`, and
//! `--shots n` to fire that many rounds into the same spot, so that the
//! later pieces have earlier ones to land on.
use sim_core::blast::{BlastRange, FLOOR};
use std::collections::BTreeMap;
use std::time::Instant;

fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let weapon = arg(&args, "--weapon").unwrap_or_else(|| "shell".into());
    let ron = match weapon.as_str() {
        "energy" => include_str!("../../../../data/energy-cannon.ron"),
        "shell" => include_str!("../../../../data/explosion.ron"),
        _ => panic!("--weapon expects energy or shell"),
    };
    let power: u32 = arg(&args, "--power")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let ticks: u32 = arg(&args, "--ticks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let shots: u32 = arg(&args, "--shots")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let mut config = modding::explosion::ExplosionConfig::from_ron(ron).unwrap();
    config.energy = config.energy * power / 1000;
    config.material_energy_j =
        (u64::from(config.material_energy_j) * u64::from(power) / 1000) as u32;
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron"))
            .unwrap();
    layout.units.clear();
    layout.target = [350, FLOOR - 1, 350];
    layout.clear_min = [150, 150];
    layout.clear_max = [550, 550];
    layout.blocks.clear();
    layout.airburst_on_target = false;
    if let Some(c) = &mut layout.course {
        c.patrols.clear();
    }
    let mut scene = BlastRange::with_layout(config, layout).unwrap();
    for _ in 0..300 {
        scene.step();
        if scene.impact.is_some() {
            break;
        }
    }
    let impact = scene.tick;
    let aim = scene.impact.unwrap();
    let mut fired = 1;
    let mut step_us = 0u128;
    // The deepest overlap of a pair and how many pairs share space, found
    // through a grid of buckets: the shape the real contact would take.
    let mut worst_pairs = 0usize;
    let mut worst_depth = 0.0f64;
    let mut worst_tick = 0u32;
    let mut cost_us = 0u128;
    let mut peak_flying = 0usize;
    let mut settled_pairs = 0usize;
    let mut settled_depth = 0.0f64;
    for i in 0..ticks {
        if fired < shots && i.is_multiple_of(90) && i > 0 {
            scene.aim = Some(aim);
            if scene.refire() {
                fired += 1;
            }
        }
        let ticking = Instant::now();
        scene.step();
        step_us += ticking.elapsed().as_micros();
        let started = Instant::now();
        let pieces = &scene.matter.debris.fragments;
        let flying = pieces.iter().filter(|f| !f.resting).count();
        peak_flying = peak_flying.max(flying);
        let mut buckets: BTreeMap<[i32; 3], Vec<usize>> = BTreeMap::new();
        let boxes: Vec<([f64; 3], [f64; 3])> = pieces
            .iter()
            .map(|f| {
                let (low, high) = f.bounds();
                let p = [f.position.x, f.position.y, f.position.z];
                let lo = [0, 1, 2].map(|a| p[a].add(low[a]).to_f64_lossy());
                let hi = [0, 1, 2].map(|a| p[a].add(high[a]).to_f64_lossy());
                (lo, hi)
            })
            .collect();
        for (i, (lo, hi)) in boxes.iter().enumerate() {
            let c0 = lo.map(|v| (v / 4.0).floor() as i32);
            let c1 = hi.map(|v| (v / 4.0).floor() as i32);
            for z in c0[2]..=c1[2] {
                for y in c0[1]..=c1[1] {
                    for x in c0[0]..=c1[0] {
                        buckets.entry([x, y, z]).or_default().push(i);
                    }
                }
            }
        }
        let mut pairs = std::collections::BTreeSet::new();
        for list in buckets.values() {
            for (k, &i) in list.iter().enumerate() {
                for &j in &list[k + 1..] {
                    let (a, b) = (&boxes[i.min(j)], &boxes[i.max(j)]);
                    let depth = (0..3)
                        .map(|a2| (a.1[a2] - b.0[a2]).min(b.1[a2] - a.0[a2]))
                        .fold(f64::MAX, f64::min);
                    if depth > 0.0 {
                        pairs.insert((i.min(j), i.max(j)));
                        if depth > worst_depth {
                            worst_depth = depth;
                        }
                    }
                }
            }
        }
        cost_us += started.elapsed().as_micros();
        if pairs.len() > worst_pairs {
            worst_pairs = pairs.len();
            worst_tick = i;
        }
        // What is left standing inside one another once everything has
        // landed is the measure that matters: pieces born together in the
        // same hole may separate, and do.
        if i + 1 == ticks {
            let both = |(a, b): &(usize, usize)| pieces[*a].resting && pieces[*b].resting;
            settled_pairs = pairs.iter().filter(|p| both(p)).count();
            settled_depth = pairs
                .iter()
                .filter(|p| both(p))
                .map(|(a, b)| {
                    let (x, y) = (&boxes[*a], &boxes[*b]);
                    (0..3)
                        .map(|k| (x.1[k] - y.0[k]).min(y.1[k] - x.0[k]))
                        .fold(f64::MAX, f64::min)
                })
                .fold(0.0, f64::max);
        }
    }
    let pieces = &scene.matter.debris.fragments;
    println!(
        "weapon={weapon} power={power} shots={fired} impact_tick={impact} tick_us={} pieces={} flying_peak={peak_flying} resting={} overlapping_pairs_peak={worst_pairs}@{worst_tick} deepest_overlap={worst_depth:.2} settled_pairs={settled_pairs} settled_depth={settled_depth:.2} grid_us_per_tick={}",
        step_us / u128::from(ticks.max(1)),
        pieces.len(),
        pieces.iter().filter(|f| f.resting).count(),
        cost_us / u128::from(ticks.max(1))
    );
}
