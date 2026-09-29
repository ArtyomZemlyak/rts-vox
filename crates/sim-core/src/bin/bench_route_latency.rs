//! End-to-end move-order latency on the loaded kilometre range. Clock values
//! are measurements only and never enter simulation decisions.
use modding::range::{RangeLayout, RangeUnitKind};
use sim_core::blast::{motion::Motion, BlastRange};
use std::time::Instant;
fn main() {
    let layout = RangeLayout::from_ron(include_str!("../../../../data/range-km.ron")).unwrap();
    let points: Vec<_> = layout
        .zones
        .iter()
        .map(|z| [z.spawn[0], z.spawn[2]])
        .collect();
    let setup = Instant::now();
    let r = BlastRange::with_layout(
        modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../../data/explosion.ron"
        ))
        .unwrap(),
        layout,
    )
    .unwrap();
    println!(
        "map_ready_ms={} extent={}x{}",
        setup.elapsed().as_millis(),
        r.world.nx,
        r.world.nz
    );
    let args: Vec<String> = std::env::args().collect();
    let cell = args
        .windows(2)
        .find(|w| w[0] == "--cell")
        .map(|w| w[1].parse::<i32>().unwrap());
    let rubble_columns = args
        .windows(2)
        .find(|w| w[0] == "--rubble-columns")
        .map_or(0, |w| w[1].parse::<usize>().unwrap());
    // Zero-height entries stress debris indexing without changing whether
    // terrain is passable, so the same 312 routes remain comparable.
    let mut columns = Vec::with_capacity(rubble_columns);
    for x in (32..r.world.nx as i32 - 32).step_by(80) {
        for z in (32..r.world.nz as i32 - 32).step_by(80) {
            if columns.len() == rubble_columns {
                break;
            }
            columns.push(([x, z], 0));
        }
        if columns.len() == rubble_columns {
            break;
        }
    }
    assert_eq!(columns.len(), rubble_columns);
    let rubble = nav::vehicle::Rubble::from_sorted(columns);
    let mut cases = Vec::new();
    let mut incomplete = 0;
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
        let body = if kind == RangeUnitKind::Tank {
            voxel::unit::tank()
        } else {
            voxel::unit::strider()
        };
        for &from in &points {
            for &to in &points {
                if from == to {
                    continue;
                }
                let mut m = Motion::new([from[0], 16, from[1]], kind, &body);
                let start = Instant::now();
                let (complete, nodes) = if let Some(cell) = cell {
                    let result = nav::vehicle::coarse::plan(
                        &r.world,
                        &rubble,
                        m.probe_shape(&body),
                        from.map(fx::Fx::from_int),
                        to,
                        nav::vehicle::coarse::Config {
                            cell,
                            nodes: r.navigation.global_nodes,
                            yielding: r.navigation.fell.materials,
                        },
                    );
                    (
                        result.as_ref().is_some_and(|p| p.path.last() == Some(&to)),
                        result.map_or(0, |p| p.nodes),
                    )
                } else {
                    m.move_to(&r.world, to, &body, &[], &rubble);
                    (
                        !m.searching() && !m.partial_path && m.path.last() == Some(&to),
                        m.last_plan[0],
                    )
                };
                let us = start.elapsed().as_micros();
                incomplete += usize::from(!complete);
                cases.push((us, format!("{kind:?}"), from, to, complete, nodes));
            }
        }
    }
    cases.sort_by_key(|r| r.0);
    println!(
        "cases={} complete={} fallback={} p50_us={} p99_us={} max_us={}",
        cases.len(),
        cases.len() - incomplete,
        incomplete,
        cases[cases.len() / 2].0,
        cases[cases.len() * 99 / 100].0,
        cases.last().unwrap().0
    );
    for (us, kind, from, to, complete, nodes) in cases.iter().rev().take(16) {
        println!("kind={kind} from={from:?} to={to:?} us={us} complete={complete} nodes={nodes}");
    }
    if std::env::args().any(|a| a == "--assert-10ms") {
        assert_eq!(
            incomplete, 0,
            "some loaded-map commands still require a deferred search"
        );
        assert!(
            cases.last().unwrap().0 <= 10000,
            "move order exceeded 10 ms"
        );
    }
}
