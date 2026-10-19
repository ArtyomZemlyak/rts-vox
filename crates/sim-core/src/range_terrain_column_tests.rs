//! Independent point queries check procedural column runs and precedence.
use super::*;
use modding::range::{RangeBlock, RangeRoad, RangeUnitDef, RangeZone};
use std::collections::BTreeSet;

fn compare(gen: &RangeTerrain, points: &BTreeSet<(u32, u32)>) {
    for &(x, z) in points {
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
