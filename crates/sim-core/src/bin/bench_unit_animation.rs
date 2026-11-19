//! Small, bounded arm-servo diagnostic; timings are SIM pose updates, not FPS.
use fx::{angle, Fx};
use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};
use sim_core::blast::BlastRange;
use std::{
    fs::File,
    io::{BufWriter, Write},
    time::Instant,
};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let prefix = args
        .windows(2)
        .find(|v| v[0] == "--out")
        .map_or("out/unit567", |v| v[1].as_str());
    let mut csv = BufWriter::new(File::create(format!("{prefix}.csv")).unwrap());
    writeln!(csv, "kind,tick,arm,yaw,pitch,elbow_x,elbow_y,elbow_z").unwrap();
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    for kind in [RangeUnitKind::Commander, RangeUnitKind::Walker04] {
        let name = format!("{kind:?}").to_lowercase();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../data/range-motion-yard.ron")).unwrap();
        layout.course = None;
        layout.units = vec![RangeUnitDef {
            kind,
            at: [600, 16, 600],
            allied: true,
            weapon: None,
        }];
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        sim.set_navigation_workers(1);
        sim.set_physics_workers(1);
        sim.start_free_control().unwrap();
        let mut times = Vec::new();
        let mut replay: Option<BlastRange> = None;
        for tick in 0..600 {
            let target = match tick {
                0..=149 => [1600, 200, 600],
                150..=299 => [1500, 200, 1150],
                300..=449 => [850, 400, 600],
                _ => [1800, 400, 600],
            };
            let mut goals = [target; 4];
            if kind == RangeUnitKind::Commander {
                goals[2] = [i32::from(tick >= 300), 0, 0];
                goals[3] = [0; 3];
            }
            let begin = Instant::now();
            sim.units[0].set_arm_targets(goals, 0);
            times.push(begin.elapsed().as_nanos());
            if let Some(copy) = replay.as_mut() {
                copy.units[0].set_arm_targets(goals, 0);
                assert_eq!(
                    sim.state_hash(),
                    copy.state_hash(),
                    "{name} replay at {tick}"
                );
                assert_eq!(sim.units[0].seg_frames, copy.units[0].seg_frames);
            } else if tick == 170 {
                replay = Some(BlastRange::restore(config, &sim.snapshot()).unwrap());
            }
            let u = &sim.units[0];
            for arm in 0..if kind == RangeUnitKind::Commander {
                2
            } else {
                4
            } {
                let f = u.seg_frames[13 + arm * 4];
                let elbow = u.seg_frames[11 + arm * 4].base;
                let yaw = angle::atan2(f.x.z.raw(), f.x.x.raw());
                let pitch = angle::atan2(
                    f.x.y.raw(),
                    f.x.x.mul(f.x.x).add(f.x.z.mul(f.x.z)).sqrt().raw(),
                );
                writeln!(
                    csv,
                    "{name},{tick},{arm},{yaw},{pitch},{},{},{}",
                    elbow.x.raw(),
                    elbow.y.raw(),
                    elbow.z.raw()
                )
                .unwrap();
            }
            if [149, 170, 299, 449, 450, 599].contains(&tick) {
                std::fs::write(format!("{prefix}-{name}-{tick}.bin"), sim.snapshot()).unwrap();
            }
        }
        times.sort_unstable();
        println!(
            "unit={name} updates=600 mean_us={} p95_us={} p99_us={} max_us={} hash={:016x}",
            times.iter().sum::<u128>() / 600 / 1000,
            times[570] / 1000,
            times[594] / 1000,
            times[599] / 1000,
            sim.state_hash()
        );
        if kind == RangeUnitKind::Commander {
            for (label, speed, turn, phase) in [
                ("forward", 1, 0, angle::QUARTER),
                ("backward", -1, 0, angle::QUARTER),
                ("pivot", 0, 1, 0),
            ] {
                let u = &mut sim.units[0];
                u.arm_targets = None;
                u.motion.drive.vel = Fx::from_int(u.motion.speed * speed);
                u.motion.phase = phase;
                u.motion.drive.yaw_rate =
                    turn * u.motion.chassis.turn * angle::FULL / (360 * sim_core::TICKS_PER_SECOND);
                u.update_pose(0);
                let saved = sim.snapshot();
                let copy = BlastRange::restore(config, &saved).unwrap();
                assert_eq!(sim.state_hash(), copy.state_hash());
                assert_eq!(sim.units[0].seg_frames, copy.units[0].seg_frames);
                std::fs::write(format!("{prefix}-commander-{label}.bin"), saved).unwrap();
            }
        }
    }
    csv.flush().unwrap();
}
