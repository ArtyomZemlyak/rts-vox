//! End-to-end move-order latency on the loaded kilometre range. Clock values
//! are measurements only and never enter simulation decisions.
use modding::range::{RangeLayout, RangeUnitKind};
use sim_core::blast::{motion::Motion, BlastRange};
use std::time::Instant;
const BATTLE_EDGE: i32 = 40;
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
    if args.iter().any(|a| a == "--fine-local") {
        let sliced = args.iter().any(|a| a == "--fine-sliced");
        let fine_budget = args
            .windows(2)
            .find(|w| w[0] == "--fine-budget")
            .map_or(r.navigation.search_nodes_per_tick, |w| {
                w[1].parse::<usize>().expect("fine budget")
            });
        assert!(fine_budget > 0);
        let mut cases = Vec::new();
        let offsets = [
            [192, 0],
            [-192, 0],
            [0, 192],
            [0, -192],
            [128, 128],
            [-128, -128],
        ];
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let body = if kind == RangeUnitKind::Tank {
                voxel::unit::tank()
            } else {
                voxel::unit::strider()
            };
            for &from in &points {
                let m = Motion::new([from[0], 16, from[1]], kind, &body);
                for offset in offsets {
                    let to = [from[0] + offset[0], from[1] + offset[1]];
                    if to.iter().any(|&v| !(16..9984).contains(&v)) {
                        continue;
                    }
                    let search = nav::vehicle::Search {
                        cell: r.navigation.cell_units,
                        min: [BATTLE_EDGE; 2],
                        max: [
                            r.world.nx as i32 - BATTLE_EDGE,
                            r.world.nz as i32 - BATTLE_EDGE,
                        ],
                        nodes: r.navigation.search_nodes,
                        crush_cost: r.navigation.fell.cost_per_column,
                        crush_cap: r.navigation.fell.cost_cap,
                        margin: r.navigation.plan_margin,
                        heuristic_pct: r.navigation.heuristic_pct,
                    };
                    let start = Instant::now();
                    let (result, slices, max_slice) = if sliced {
                        let mut ground = nav::vehicle::Probe::for_search(
                            &r.world,
                            &r.navigation.fell.materials,
                            Default::default(),
                        )
                        .with_rubble(&rubble);
                        let pending = nav::vehicle::Hunt::start_bounded(
                            &mut ground,
                            m.probe_shape(&body),
                            from.map(fx::Fx::from_int),
                            m.drive.head,
                            to,
                            search,
                            &|_, _| true,
                            fine_budget,
                        );
                        match pending {
                            nav::vehicle::Hunting::Done(result) => {
                                (result, 0, start.elapsed().as_micros())
                            }
                            nav::vehicle::Hunting::Open(mut hunt) => {
                                let mut max_slice = start.elapsed().as_micros();
                                let mut slices = 0;
                                let result = loop {
                                    let tick = Instant::now();
                                    let answer = hunt.advance_in(
                                        &r.world,
                                        &rubble,
                                        &|_, _| true,
                                        fine_budget,
                                    );
                                    max_slice = max_slice.max(tick.elapsed().as_micros());
                                    slices += 1;
                                    if let Some(result) = answer {
                                        break result;
                                    }
                                    assert!(slices < 50000, "fine search did not finish");
                                };
                                (result, slices, max_slice)
                            }
                        }
                    } else {
                        let result = nav::vehicle::plan(
                            nav::vehicle::Probe::around(
                                &r.world,
                                &r.navigation.fell.materials,
                                from.map(fx::Fx::from_int),
                                256,
                            )
                            .with_rubble(&rubble),
                            m.probe_shape(&body),
                            from.map(fx::Fx::from_int),
                            m.drive.head,
                            to,
                            search,
                            |_, _| true,
                        );
                        (result, 0, start.elapsed().as_micros())
                    };
                    cases.push((
                        start.elapsed().as_micros(),
                        max_slice,
                        slices,
                        kind,
                        from,
                        to,
                        result,
                    ));
                }
            }
        }
        cases.sort_by_key(|c| c.0);
        println!(
            "fine_cases={} complete={} p50_us={} p99_us={} max_us={} max_slice_us={} max_slices={}",
            cases.len(),
            cases
                .iter()
                .filter(|c| c.6.path.last() == Some(&c.5))
                .count(),
            cases[cases.len() / 2].0,
            cases[cases.len() * 99 / 100].0,
            cases.last().unwrap().0,
            cases.iter().map(|c| c.1).max().unwrap(),
            cases.iter().map(|c| c.2).max().unwrap()
        );
        for (us, max_slice, slices, kind, from, to, result) in cases.iter().rev().take(16) {
            println!("kind={kind:?} from={from:?} to={to:?} us={us} slices={slices} max_slice_us={max_slice} complete={} nodes={} poses={} scans={}",
                result.path.last() == Some(to), result.nodes, result.poses, result.scans);
        }
        return;
    }
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
                            local_grade: false,
                            restarts: 0,
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
