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
            .unwrap_or(if case == "gate" {
                include_str!("../../../../data/range-motion-gate.ron")
            } else { include_str!("../../../../data/range-motion-yard.ron") }),
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
    if case == "gate" {
        // Group commands are not patrols (the authored patrol list is bounded
        // separately). Issue the same ten one-shot orders as the gate fixture.
        orders = layout.units.iter().enumerate()
            .map(|(id,u)| (id,[u.at[0]+700,u.at[2]])).collect();
    }
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
        assert!(["yard", "gate"].contains(&case));
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
    let group_goal = if case == "gate" { [1100, 700] } else { [800, 850] };
    assert_eq!(sim.battle_group_move(&orders, group_goal), orders.len());
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
    if let Some(p) = prefix {
        std::fs::write(format!("{p}-end.bin"), sim.snapshot()).unwrap();
    }
    if case == "gate" {
        assert!(finished.iter().all(|&t| t>0), "gate group did not finish within {ticks} ticks");
        for &(id,goal) in &orders {
            let m=&sim.units[id].motion;
            let dx=m.pos[0].sub(fx::Fx::from_int(goal[0]));
            let dz=m.pos[2].sub(fx::Fx::from_int(goal[1]));
            let arrival=fx::Fx::from_int(m.arrival+1)
                .add(fx::Fx::from_ratio(m.speed,sim_core::TICKS_PER_SECOND));
            assert!(m.destination.is_none() && dx.mul(dx).add(dz.mul(dz))<=arrival.mul(arrival),
                "gate actor {id} stopped short of its actual goal");
        }
        assert!(sim.matter.economy.ledger.balanced());
    }
}
