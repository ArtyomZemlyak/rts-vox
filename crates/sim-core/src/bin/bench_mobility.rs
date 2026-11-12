//! Movement-only yard and mixed junction; never measures rendered FPS.
use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let value = |key: &str| args.windows(2).find(|w| w[0] == key).map(|w| w[1].as_str());
    let case = value("--case").unwrap_or("yard");
    let ticks: usize = value("--ticks").unwrap_or("3000").parse().unwrap();
    assert!(ticks > 0);
    let source = value("--layout").map(|p| std::fs::read_to_string(p).unwrap());
    let mut layout = RangeLayout::from_ron(
        source
            .as_deref()
            .unwrap_or(include_str!("../../../../data/range-motion-yard.ron")),
    )
    .unwrap();
    let mut orders: Vec<_> = layout
        .course
        .as_ref()
        .unwrap()
        .patrols
        .iter()
        .map(|p| (p.unit, p.points[0]))
        .collect();
    if case == "junction" {
        let kinds = [
            RangeUnitKind::Tank,
            RangeUnitKind::Crawler,
            RangeUnitKind::Commander,
            RangeUnitKind::Walker04,
        ];
        let starts = [
            [350, 16, 850],
            [1250, 16, 850],
            [800, 16, 350],
            [800, 16, 1350],
        ];
        layout.units = kinds
            .into_iter()
            .zip(starts)
            .map(|(kind, at)| RangeUnitDef {
                kind,
                at,
                allied: true,
                weapon: None,
            })
            .collect();
        orders = vec![
            (0, [1250, 850]),
            (1, [350, 850]),
            (2, [800, 1350]),
            (3, [800, 350]),
        ];
        layout.course.as_mut().unwrap().patrols.clear();
    } else {
        assert_eq!(case, "yard");
    }
    let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let mut sim = BlastRange::with_layout(cfg, layout).unwrap();
    sim.set_navigation_workers(1);
    sim.set_physics_workers(1);
    sim.start_free_control().unwrap();
    for &(id, goal) in &orders {
        let m = &mut sim.units[id].motion;
        m.drive.head = fx::angle::atan2(
            i64::from(goal[1] - m.pos[2].to_int_trunc()),
            i64::from(goal[0] - m.pos[0].to_int_trunc()),
        );
    }
    assert_eq!(sim.battle_group_move(&orders, [800, 850]), orders.len());
    let prefix = value("--snapshot-prefix");
    let capture: usize = value("--snapshot-tick").unwrap_or("300").parse().unwrap();
    if let Some(p) = prefix {
        std::fs::write(format!("{p}-start.bin"), sim.snapshot()).unwrap();
    }
    let start: Vec<_> = orders
        .iter()
        .map(|&(id, _)| sim.units[id].motion.pos)
        .collect();
    let mut first = vec![0; orders.len()];
    let mut finished = first.clone();
    let mut blocked = first.clone();
    let mut stopped = first.clone();
    let mut longest = first.clone();
    let mut times = Vec::new();
    let mut replica = None;
    for tick in 1..=ticks {
        let begin = Instant::now();
        sim.step();
        times.push(begin.elapsed().as_nanos());
        if args.iter().any(|a| a == "--replay") {
            if let Some(copy) = replica.as_mut() {
                let copy: &mut BlastRange = copy;
                copy.step();
                assert_eq!(sim.state_hash(), copy.state_hash(), "replay at {tick}");
            } else if tick == 120 {
                let mut copy = BlastRange::restore(cfg, &sim.snapshot()).unwrap();
                copy.set_physics_workers(1);
                copy.set_navigation_workers(2);
                replica = Some(copy);
            }
        }
        for (slot, &(id, _)) in orders.iter().enumerate() {
            let m = &sim.units[id].motion;
            if first[slot] == 0 && (m.pos[0] != start[slot][0] || m.pos[2] != start[slot][2]) {
                first[slot] = tick;
            }
            if finished[slot] == 0 && m.destination.is_none() {
                finished[slot] = tick;
            }
            blocked[slot] += usize::from(m.blocked);
            stopped[slot] = if m.destination.is_some() && m.drive.vel.abs() < fx::Fx::ONE {
                stopped[slot] + 1
            } else {
                0
            };
            longest[slot] = longest[slot].max(stopped[slot]);
        }
        if tick == capture {
            if let Some(p) = prefix {
                std::fs::write(format!("{p}-{tick}.bin"), sim.snapshot()).unwrap();
            }
        }
        if args.iter().any(|a| a == "--trace") && tick % 300 == 0 {
            for &(id, _) in &orders {
                let m = &sim.units[id].motion;
                println!("trace case={case} tick={tick} id={id} pos={:?} path={:?} cursor={} recovery={:?} searching={} nodes={} head={} speed={:?} partial={}",m.pos,m.path,m.cursor,m.recovery_state(),m.searching(),m.frontier_nodes(),m.drive.head,m.drive.vel,m.partial_path);
            }
        }
    }
    times.sort_unstable();
    for (slot, &(id, _)) in orders.iter().enumerate() {
        println!(
            "actor case={case} id={id} kind={:?} first={} arrival={} blocked={} longest_stopped={}",
            sim.units[id].kind, first[slot], finished[slot], blocked[slot], longest[slot]
        );
    }
    println!("mobility case={case} ticks={ticks} arrived={} count={} last_arrival={} blocked={} mean_tick_us={} p99_tick_us={} hash={:016x}",finished.iter().filter(|&&v|v>0).count(),orders.len(),finished.iter().max().unwrap(),blocked.iter().sum::<usize>(),times.iter().sum::<u128>()/ticks as u128/1000,times[ticks*99/100]/1000,sim.state_hash());
}
