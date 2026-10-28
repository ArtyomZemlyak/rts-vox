//! Independent point queries check procedural column runs and precedence.
use super::*;
use modding::range::{RangeBlock, RangeRoad, RangeUnitDef, RangeZone};
use std::collections::BTreeSet;

#[test]
fn uniform_subsoil_proof_respects_interior_blocks_surfaces_and_foundations() {
    for level in [16, 80, 272] {
        let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
            .unwrap()
            .with_ground_level(level)
            .unwrap();
        l.battle = true;
        l.blocks.clear();
        l.units.clear();
        let min = [110, 1, 110];
        let max = [520, (level - 1) as u32, 520];
        let g = RangeTerrain::new(true, Some(l.clone()));
        assert_eq!(g.uniform_box(min, max), Some(1));
        for z in (min[2]..max[2]).step_by(31) {
            for x in (min[0]..max[0]).step_by(29) {
                for y in [min[1], max[1] - 1] {
                    assert_eq!(g.material(x, y, z), 1);
                }
            }
        }
        assert_eq!(g.uniform_box(min, [520, level as u32, 520]), None);
        for mat in [0, 2, 65535] {
            l.blocks = vec![RangeBlock {
                min: [300, 2, 300],
                max: [310, 4, 310],
                material: mat,
            }];
            let g = RangeTerrain::new(true, Some(l.clone()));
            assert_eq!(g.uniform_box(min, max), None, "interior tile mat={mat}");
            assert_eq!(g.uniform_box([110, 1, 110], [120, 4, 120]), Some(1));
            assert_eq!(g.material(301, 3, 301), mat);
        }
        l.blocks.clear();
        l.units = vec![RangeUnitDef {
            kind: RangeUnitKind::TurretT1,
            at: [350, level, 330],
            allied: true,
            weapon: None,
        }];
        let g = RangeTerrain::new(true, Some(l));
        assert_eq!(
            g.uniform_box(
                [345, (level - 2) as u32, 325],
                [355, (level - 1) as u32, 335]
            ),
            None
        );
    }
}

fn compare(gen: &RangeTerrain, points: &BTreeSet<(u32, u32)>) {
    for &(x, z) in points {
        let candidates = gen
            .material_candidates(
                [x.saturating_sub(31), 0, z.saturating_sub(31)],
                [x + 33, 300, z + 33],
            )
            .unwrap();
        for (y0, len) in [
            (0, 192),
            (0, 0),
            (0, 1),
            (1, 14),
            (14, 2),
            (15, 1),
            (16, 16),
            (31, 2),
            (60, 16),
            (191, 9),
            (192, 16),
        ] {
            let mut out = vec![65534; len];
            gen.column(x, z, y0, &mut out);
            for (i, &material) in out.iter().enumerate() {
                assert!(
                    candidates.contains(&material),
                    "missing material{material} at {x},{},{z}",
                    y0 + i as u32
                );
                assert_eq!(
                    material,
                    gen.material(x, y0 + i as u32, z),
                    "{x},{z}, y0={y0}, len={len}, i={i}"
                );
            }
        }
    }
}

fn add_box(points: &mut BTreeSet<(u32, u32)>, lo: [i32; 3], hi: [i32; 3], size: [u32; 3]) {
    for x in [
        lo[0] - 1,
        lo[0],
        lo[0] + 1,
        (lo[0] + hi[0]) / 2,
        hi[0] - 1,
        hi[0],
        hi[0] + 1,
    ] {
        for z in [
            lo[2] - 1,
            lo[2],
            lo[2] + 1,
            (lo[2] + hi[2]) / 2,
            hi[2] - 1,
            hi[2],
            hi[2] + 1,
        ] {
            if x >= 0 && z >= 0 && x < size[0] as i32 && z < size[2] as i32 {
                points.insert((x as u32, z as u32));
            }
        }
    }
}

#[test]
fn column_runs_match_points_across_every_stock_scenario() {
    let base = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    for id in 0..modding::range_skirmish::STOCK.len() {
        let layout = modding::range_skirmish::stock(id)
            .unwrap()
            .apply_to(base.clone())
            .unwrap();
        let mut points = BTreeSet::new();
        let gen = RangeTerrain::new(true, Some(layout.clone()));
        for unit in &layout.units {
            add_box(&mut points, unit.at, unit.at, layout.size);
        }
        for b in layout.blocks.iter().chain(gen.index.foundations.iter()) {
            add_box(&mut points, b.min, b.max, layout.size);
        }
        for zone in &layout.zones {
            add_box(
                &mut points,
                [zone.min[0], 0, zone.min[1]],
                [zone.max[0], 192, zone.max[1]],
                layout.size,
            );
        }
        if let Some(course) = &layout.course {
            add_box(
                &mut points,
                [course.min[0], 0, course.min[1]],
                [course.max[0], 192, course.max[1]],
                layout.size,
            );
            for ramp in &course.ramps {
                add_box(&mut points, ramp.min, ramp.max, layout.size);
            }
        }
        for mound in &layout.mounds {
            add_box(&mut points, mound.min, mound.max, layout.size);
        }
        compare(&gen, &points);
        assert!(points.len() > 100, "scenario{id} did not exercise geometry");
    }
}

#[test]
fn column_runs_keep_air_cut_material_ids_and_foundation_priority() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.units = vec![RangeUnitDef {
        kind: RangeUnitKind::TurretT1,
        at: [350, 16, 330],
        allied: true,
        weapon: None,
    }];
    layout.zones = vec![RangeZone {
        name: "test".into(),
        scenario: None,
        min: [300, 280],
        max: [390, 370],
        spawn: [350, 16, 330],
        target: [380, 16, 330],
    }];
    layout.roads = vec![RangeRoad {
        width: 12,
        points: vec![[310, 320], [385, 320]],
    }];
    layout.blocks = vec![
        RangeBlock {
            min: [330, 0, 300],
            max: [375, 64, 350],
            material: 300,
        },
        RangeBlock {
            min: [340, 8, 310],
            max: [365, 50, 340],
            material: 0,
        },
        RangeBlock {
            min: [345, 20, 320],
            max: [370, 40, 345],
            material: 2,
        },
        RangeBlock {
            min: [350, 31, 325],
            max: [371, 67, 346],
            material: 65535,
        },
    ];
    for course in [true, false] {
        let mut layout = layout.clone();
        if !course {
            layout.course = None;
        }
        let gen = RangeTerrain::new(true, Some(layout.clone()));
        let mut points = BTreeSet::new();
        for z in (275..375).step_by(3) {
            for x in (295..395).step_by(3) {
                points.insert((x, z));
            }
        }
        for b in layout.blocks.iter().chain(gen.index.foundations.iter()) {
            add_box(&mut points, b.min, b.max, layout.size);
        }
        compare(&gen, &points);
        assert_eq!(gen.material(350, 15, 330), foundation::tuning().material);
        assert_eq!(gen.material(341, 19, 311), 0);
        assert_eq!(gen.material(350, 31, 330), 65535);
    }
}

#[test]
fn column_runs_preserve_first_match_with_maximum_overlapping_blocks() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.blocks = (0..modding::range::MAX_RANGE_BLOCKS)
        .map(|i| RangeBlock {
            min: [380, if i % 2 == 0 { 0 } else { 8 }, 430],
            max: [420, if i % 2 == 0 { 8 } else { 192 }, 470],
            material: (i % 7) as u16,
        })
        .collect();
    let gen = RangeTerrain::new(true, Some(layout));
    // The front two half-columns hide every older block. Both a full
    // column and slices crossing their boundary retain exact precedence.
    compare(&gen, &BTreeSet::from([(400, 450)]));
    let mut out = [65534; 192];
    gen.column(400, 450, 0, &mut out);
    assert!(out[..8]
        .iter()
        .all(|&m| m == ((modding::range::MAX_RANGE_BLOCKS - 2) % 7) as u16));
    assert!(out[8..]
        .iter()
        .all(|&m| m == ((modding::range::MAX_RANGE_BLOCKS - 1) % 7) as u16));
}

#[test]
fn deep_range_columns_match_scalar_materials_and_preserve_authored_scene() {
    let old = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let raised = old.clone().with_ground_level(80).unwrap();
    assert_eq!(raised.clone().with_ground_level(80).unwrap(), raised);
    assert_eq!(raised.clone().with_ground_level(16).unwrap(), old);
    let low = RangeTerrain::new(true, Some(old));
    let high = RangeTerrain::new(true, Some(raised));
    for (x, z) in [
        (40, 40),
        (359, 312),
        (399, 336),
        (410, 450),
        (520, 510),
        (300, 560),
    ] {
        let mut col = [0u16; 256];
        high.column(x, z, 0, &mut col);
        for y in 0..256 {
            assert_eq!(
                col[y as usize],
                high.material(x, y, z),
                "column {x} {y} {z}"
            );
            if y < 64 {
                assert_eq!(col[y as usize], 1);
            } else {
                assert_eq!(
                    col[y as usize],
                    low.material(x, y - 64, z),
                    "translation {x} {y} {z}"
                );
            }
        }
    }
}

fn road_contains_reference(x: u32, z: u32, a: [i32; 2], b: [i32; 2], radius: i128) -> bool {
    let dx = (b[0] - a[0]) as i128;
    let dz = (b[1] - a[1]) as i128;
    let px = x as i128 - a[0] as i128;
    let pz = z as i128 - a[1] as i128;
    let len2 = dx * dx + dz * dz;
    let dot = (px * dx + pz * dz).clamp(0, len2);
    let ox = px * len2 - dx * dot;
    let oz = pz * len2 - dz * dot;
    ox * ox + oz * oz <= radius * radius * len2 * len2
}

#[test]
fn road_membership_matches_projection_at_endpoints_verges_and_degenerate_segments() {
    let mut rng = fx::rng::Rng::new(393, 809);
    for (a, b) in [
        ([100, 100], [100, 100]),
        ([10, 10], [200, 10]),
        ([200, 10], [10, 10]),
        ([0, 0], [0, 200]),
        ([-30, -70], [400, 280]),
        ([32000, 16000], [-32000, -16000]),
    ] {
        for radius in [-10i128, 0, 1, 3, 10, 100] {
            for x in 0..256 {
                for z in 0..256 {
                    assert_eq!(
                        road_contains(x, z, a, b, radius),
                        road_contains_reference(x, z, a, b, radius),
                        "x={x} z={z} a={a:?} b={b:?} radius={radius}"
                    );
                }
            }
        }
    }
    for _ in 0..100000 {
        let a = [
            rng.below(65536) as i32 - 32768,
            rng.below(65536) as i32 - 32768,
        ];
        let b = [
            rng.below(65536) as i32 - 32768,
            rng.below(65536) as i32 - 32768,
        ];
        let x = rng.below(65536);
        let z = rng.below(65536);
        let radius = i128::from(rng.below(1000));
        assert_eq!(
            road_contains(x, z, a, b, radius),
            road_contains_reference(x, z, a, b, radius)
        );
    }
}

#[test]
#[ignore = "manual exact road membership timing, run alone in release"]
fn measure_road_membership() {
    use std::{hint::black_box, time::Instant};
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    let mut inputs = Vec::new();
    for road in &layout.roads {
        for pair in road.points.windows(2) {
            let x0 = pair[0][0].min(pair[1][0]);
            let z0 = pair[0][1].min(pair[1][1]);
            let width = pair[0][0].abs_diff(pair[1][0]) + 129;
            let depth = pair[0][1].abs_diff(pair[1][1]) + 129;
            for i in 0..8192 {
                inputs.push((
                    (x0 - 64 + (i * 73 % width) as i32).max(0) as u32,
                    (z0 - 64 + (i * 131 % depth) as i32).max(0) as u32,
                    pair[0],
                    pair[1],
                    i128::from(road.width / 2),
                ));
            }
        }
    }
    assert!(!inputs.is_empty());
    let mut times = [Vec::new(), Vec::new()];
    for repeat in 0..13 {
        for optimized in if repeat % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            let mut count = 0;
            for &(x, z, a, b, r) in black_box(&inputs) {
                count += usize::from(if optimized {
                    road_contains(x, z, a, b, r)
                } else {
                    road_contains_reference(x, z, a, b, r)
                });
            }
            black_box(count);
            times[usize::from(optimized)].push(start.elapsed().as_nanos());
            println!("ROAD_SAMPLE optimized={optimized} count={count}");
        }
    }
    for (optimized, ns) in times.iter_mut().enumerate() {
        ns.sort_unstable();
        println!(
            "ROAD optimized={optimized} n={} median_ns={} min_ns={} max_ns={}",
            inputs.len(),
            ns[6],
            ns[0],
            ns[12]
        );
    }
}
