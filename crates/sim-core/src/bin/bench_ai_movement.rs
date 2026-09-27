//! Enemy movement latency and CPU cost on the same wall-detour fixtures as
//! the battle regressions. Run in release; timing never feeds the simulation.
use modding::{
    explosion::ExplosionConfig,
    range::{RangeLayout, RangeUnitDef, RangeUnitKind},
};
use sim_core::blast::{battle::Order, BlastRange};
use std::time::Instant;

fn main() {
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron")).unwrap();
        layout.battle = true;
        layout.size = [640, 192, 640];
        layout.blocks.clear();
        layout.shields.clear();
        layout.course = None;
        layout.clear_min = [64, 64];
        layout.clear_max = [600, 600];
        layout.units = vec![
            RangeUnitDef {
                kind: RangeUnitKind::Tank,
                at: [170, 16, 336],
                allied: true,
            },
            RangeUnitDef {
                kind,
                at: [450, 16, 336],
                allied: false,
            },
        ];
        let config =
            ExplosionConfig::from_ron(include_str!("../../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(config, layout).unwrap();
        std::sync::Arc::make_mut(&mut r.battle.tuning).kinetic_range = 100;
        r.streams.tuning.machine_gun.range = 100;
        for x in 300..310 {
            for y in 16..90 {
                for z in 260..410 {
                    r.world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while r.world.queued() > 0 {
            r.world.flush();
        }
        r.start_battle().unwrap();
        assert!(r.battle_order(0, Order::Hold, None));
        let origin = r.units[1].motion.pos;
        let mut first_motion = None;
        let mut first_path = None;
        let mut first_shot = None;
        let mut blocked = 0;
        let mut searching = 0;
        let mut samples = Vec::new();
        for tick in 0..1800 {
            let start = Instant::now();
            r.step();
            samples.push(start.elapsed().as_micros());
            let m = &r.units[1].motion;
            if !m.path.is_empty() {
                first_path.get_or_insert(tick);
            }
            if m.pos[0] != origin[0] || m.pos[2] != origin[2] {
                first_motion.get_or_insert(tick);
            }
            blocked += usize::from(m.blocked);
            searching += usize::from(m.searching());
            if r.battle.actors[1].fired > 0 {
                first_shot = Some(tick);
                break;
            }
        }
        assert!(first_shot.is_some(), "{kind:?} failed to engage");
        samples.sort_unstable();
        let n = samples.len();
        println!("kind={kind:?} first_path={first_path:?} first_motion={first_motion:?} first_shot={first_shot:?} searching_ticks={searching} blocked_ticks={blocked} total_us={} p50_us={} p99_us={} max_us={} hash={:016x}",
            samples.iter().sum::<u128>(), samples[n/2], samples[n*99/100], samples[n-1], r.state_hash());
    }
}
