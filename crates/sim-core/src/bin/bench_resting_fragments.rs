//! Small, deterministic saved workloads for stationary-debris render profiling.
//! Construction and clocks stay outside SIM; all matter enters normal containers.
use sim_core::{
    blast::{
        battle::commands::{Command, GunCommand},
        BlastRange,
    },
    burst::IDENTITY,
};
use std::time::Instant;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let path = args.get(1).expect("OUTPUT.snapshot");
    let count: usize = args.get(2).map_or(8192, |s| s.parse().unwrap());
    assert!(count <= 32768);
    let columns = if count > 8192 { 192 } else { 96 };
    let side = if count > 8192 { 1024 } else { 512 };
    let mut l = modding::laboratory::Definition::stock().layout;
    l.size = [side, 128, side];
    l.clear_min = [1, 1];
    l.clear_max = [side as i32 - 1, side as i32 - 1];
    l.units.clear();
    l.unit_yaws.clear();
    l.blocks.clear();
    l.shields.clear();
    l.gun_point = true;
    l.laboratory = true;
    l.gun = [32, 32, 32];
    l.target = [side as i32 / 2, 16, side as i32 / 2];
    let c = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.execute_command(Command::Gun(GunCommand::Control));
    for start in (0..count).step_by(512) {
        let mut cells = Vec::new();
        for i in start..(start + 512).min(count) {
            let at = [
                64 + (i % columns) as u32 * 4,
                16,
                64 + (i / columns) as u32 * 4,
            ];
            let shape = [1u8, 3, 7, 0x17, 0x33, 0x7f, 0xff][i % 7];
            for bit in 0..8 {
                if shape & (1 << bit) != 0 {
                    cells.push(([at[0] + bit % 2, at[1] + bit / 2 % 2, at[2] + bit / 4], 2));
                }
            }
        }
        // These are detached world cells; admission, density and resource stock
        // use precisely the same ordinary debris path as a terrain collapse.
        let first = r.matter.debris.fragments.len();
        let (made, returned) = r.matter.debris.pieces_from_cells(
            &mut r.world,
            &cells,
            &mut r.matter.economy,
            &r.matter.tuning,
            2,
        );
        assert_eq!((made, returned), ((start + 512).min(count) - start, 0));
        for f in &mut r.matter.debris.fragments[first..] {
            f.resting = true;
            f.basis = IDENTITY;
        }
    }
    assert!(r.matter.economy.ledger.balanced());
    let mut timings = Vec::new();
    for _ in 0..40 {
        let start = Instant::now();
        r.step();
        timings.push(start.elapsed().as_micros());
    }
    assert_eq!(r.matter.debris.fragments.len(), count);
    assert!(r.matter.debris.fragments.iter().all(|f| f.resting));
    timings.sort_unstable();
    let bytes = r.snapshot();
    let back = BlastRange::restore(r.config, &bytes).expect("fixture must restore");
    assert_eq!(back.state_hash(), r.state_hash());
    std::fs::write(path, bytes).unwrap();
    println!("resting_fixture pieces={count} hash={:016x} sim_mean_us={} sim_p95_us={} sim_max_us={} snapshot={path}",r.state_hash(),timings.iter().sum::<u128>()/timings.len() as u128,timings[timings.len()*95/100],timings[timings.len()-1]);
}
