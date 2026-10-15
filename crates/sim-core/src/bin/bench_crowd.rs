//! Reproducible common-motion diagnostics. No rendering or combat.
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef, RangeUnitKind};
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let value = |key: &str| {
        args.iter()
            .position(|s| s == key)
            .and_then(|i| args.get(i + 1))
    };
    let ticks: usize = value("--ticks").map(|v| v.parse().unwrap()).unwrap_or(1800);
    let workers: usize = value("--workers").map(|v| v.parse().unwrap()).unwrap_or(1);
    let count: usize = value("--count").map(|v| v.parse().unwrap()).unwrap_or(10);
    assert!((1..=64).contains(&count) && ticks > 0);
    for case in ["convoy", "formation", "column", "crossing", "gate"] {
        if value("--case").is_some_and(|v| v != case) {
            continue;
        }
        if count != 10 && !["convoy", "formation"].contains(&case) {
            continue;
        }
        let columns = if count > 10 { 8 } else { 4 };
        let side = if count > 10 { 4096 } else { 1600 };
        let centre = if count > 10 {
            [2200, 1800]
        } else {
            [1100, 700]
        };
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron")).unwrap();
        layout.size = [side as u32, 128, side as u32];
        layout.battle = true;
        layout.clear_min = [0, 0];
        layout.clear_max = [side, side];
        layout.blocks.clear();
        let course = layout.course.as_mut().unwrap();
        course.min = [40, 40];
        course.max = [side - 40, side - 40];
        course.ramps.clear();
        course.patrols.clear();
        layout.units = (0..count)
            .map(|id| RangeUnitDef {
                allied: true,
                kind: RangeUnitKind::Tank,
                at: if case == "column" {
                    [150 + id as i32 * 70, 16, 700]
                } else if case == "crossing" {
                    [
                        if id < 5 { 300 } else { 1200 },
                        16,
                        500 + (id % 5) as i32 * 60,
                    ]
                } else {
                    [
                        260 + (id / columns) as i32 * 90,
                        16,
                        550 + (id % columns) as i32 * 90,
                    ]
                },
                weapon: None,
            })
            .collect();
        if case == "gate" {
            layout.blocks = vec![
                RangeBlock {
                    min: [760, 16, 0],
                    max: [800, 64, 640],
                    material: 2,
                },
                RangeBlock {
                    min: [760, 16, 760],
                    max: [800, 64, 1600],
                    material: 2,
                },
            ];
        }
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../../data/explosion.ron"
        ))
        .unwrap();
        let mut scene = BlastRange::with_layout(config, layout).unwrap();
        scene.set_navigation_workers(workers);
        scene.start_free_control().unwrap();
        if case == "column" {
            scene.units[9].motion.speed = 20;
        }
        let mut destinations: Vec<_> = scene
            .units
            .iter()
            .enumerate()
            .map(|(id, u)| {
                (
                    id,
                    match case {
                        "convoy" | "column" | "gate" => [
                            u.motion.pos[0].to_int_trunc() + 700,
                            u.motion.pos[2].to_int_trunc(),
                        ],
                        "crossing" => [if id < 5 { 1200 } else { 300 }, 500 + (id % 5) as i32 * 60],
                        _ => [
                            1100 + (2 * (id % 4) as i32 - 3) * 45,
                            700 + (2 * (id / 4) as i32 - 2) * 45,
                        ],
                    },
                )
            })
            .collect();
        if case == "formation" && !args.iter().any(|a| a == "--legacy-formation") {
            destinations =
                scene.battle_formation_destinations(&(0..count).collect::<Vec<_>>(), centre);
        }
        assert_eq!(scene.battle_group_move(&destinations, centre), count);
        let mut motion = Vec::new();
        let mut total = Vec::new();
        let (mut blocked, mut reverse, mut searches) = (0usize, 0usize, 0usize);
        let mut arrivals = vec![0usize; count];
        for tick in 1..=ticks {
            let start = Instant::now();
            let mut phase = start;
            scene.step_observed(|name| {
                let now = Instant::now();
                if name == "motion" {
                    motion.push(now.duration_since(phase).as_nanos());
                }
                phase = now;
            });
            total.push(start.elapsed().as_nanos());
            for (id, unit) in scene.units.iter().enumerate() {
                let m = &unit.motion;
                blocked += usize::from(m.blocked);
                reverse += usize::from(m.drive.vel < fx::Fx::ZERO);
                searches += usize::from(m.searching());
                if arrivals[id] == 0 && m.destination.is_none() {
                    arrivals[id] = tick;
                }
            }
            if args.iter().any(|a| a == "--trace") && tick % 300 == 0 {
                println!(
                    "trace case={case} tick={tick} positions={:?}",
                    scene
                        .units
                        .iter()
                        .map(|u| (
                            u.motion.pos.map(fx::Fx::to_int_trunc),
                            u.motion.destination,
                            u.motion.recovery_state()
                        ))
                        .collect::<Vec<_>>()
                );
            }
        }
        motion.sort_unstable();
        total.sort_unstable();
        println!("crowd case={case} ticks={ticks} workers={workers} arrived={} last_arrival={} blocked_ticks={blocked} reverse_ticks={reverse} searching_ticks={searches} motion_mean_us={} motion_p99_us={} tick_mean_us={} tick_p99_us={} hash={:016x}",
            arrivals.iter().filter(|&&t|t>0).count(), arrivals.iter().max().unwrap(),
            motion.iter().sum::<u128>() / ticks as u128 / 1000, motion[ticks * 99 / 100] / 1000,
            total.iter().sum::<u128>() / ticks as u128 / 1000, total[ticks * 99 / 100] / 1000,scene.state_hash());
    }
}
