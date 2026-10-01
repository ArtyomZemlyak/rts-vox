//! Material blasts in the ordinary world. Immutable inputs, bounded work and
//! confirmed voxel removals. The wave is an impulse envelope; fracture and
//! fragment kinetic energy draw from a separate, finite joule budget.
use crate::{
    economy::{Economy, Wreck},
    matter, NO_TARGET,
};
use fx::{
    angle,
    blob::{Blob, Scan},
    Fx,
};
use modding::{tuning::Blast, Registry};
use phys::{
    blast::{Hit, Wave},
    V3,
};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use voxel::{
    store::{Mat, AIR},
    world::{snapshot::Snapshot, Edit},
};

#[path = "chunk.rs"]
pub mod chunk;
#[path = "corpse.rs"]
mod corpse;
#[path = "unit_debris.rs"]
mod unit_debris;

struct Job {
    at: [i32; 3],
    origin: [i32; 3],
    n: usize,
    cell: i32,
    input: Snapshot,
    cursor: usize,
    resistance: Vec<u32>,
    wave: Option<Wave>,
    hits: VecDeque<Hit>,
    subcell: usize,
    energy_mj: u64,
    body_hit_done: bool,
    body_energy_mj: u64,
    body_field: crate::body_wave::Field,
}

/// Fracture work and the reserved kinetic share, in millijoules.
pub(crate) fn fracture_cost_mj(strength: u32, mm: u16, t: &Blast) -> (u64, u64) {
    let fracture = u64::from(strength)
        .saturating_mul(u64::from(t.fracture_kj_m3_per_strength))
        .saturating_mul(u64::from(mm).pow(3))
        / 1000;
    let motion = fracture.saturating_mul(u64::from(t.motion_pct)) / u64::from(100 - t.motion_pct);
    (fracture.max(1).saturating_add(motion), motion)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distant_hull_rejection_preserves_rotated_contacts() {
        for head in (0..angle::FULL).step_by((angle::FULL / 16) as usize) {
            let hull = Hull {
                pos: [Fx::from_int(20), Fx::from_int(12), Fx::from_int(30)],
                head,
                half: [Fx::from_int(7), Fx::from_int(13)],
                height: Fx::from_int(8),
                vel: [Fx::ZERO; 3],
                step: Fx::ONE,
                solid: None,
            };
            let low = [Fx::from_int(-2); 3];
            let high = [Fx::from_int(2); 3];
            let reach = Fx::from_int(2);
            let (c, s) = (angle::cos(head), angle::sin(head));
            for x in (-20..=60).step_by(4) {
                for z in (-10..=70).step_by(4) {
                    let p = [Fx::from_int(x), Fx::from_int(16), Fx::from_int(z)];
                    let dx = p[0].sub(hull.pos[0]);
                    let dz = p[2].sub(hull.pos[2]);
                    let u = dx.mul(c).add(dz.mul(s));
                    let v = dz.mul(c).sub(dx.mul(s));
                    let expected =
                        u.abs() <= hull.half[0].add(reach) && v.abs() <= hull.half[1].add(reach);
                    assert_eq!(
                        hull_box_hit(p, low, high, &[hull], 0, true).is_some(),
                        expected,
                        "head={head} x={x} z={z}"
                    );
                    let to = [p[0].add(Fx::from_int(3)), p[1], p[2].sub(Fx::ONE)];
                    let extent = [hull.half[0].add(reach), hull.half[1].add(reach)];
                    let rotated = [
                        extent[0]
                            .mul(c.abs())
                            .add(extent[1].mul(s.abs()))
                            .add(Fx::ONE),
                        extent[0]
                            .mul(s.abs())
                            .add(extent[1].mul(c.abs()))
                            .add(Fx::ONE),
                    ];
                    let old_near = [(0, 0), (2, 1)].iter().all(|&(axis, i)| {
                        p[axis].min(to[axis]) <= hull.pos[axis].add(rotated[i])
                            && p[axis].max(to[axis]) >= hull.pos[axis].sub(rotated[i])
                    });
                    assert_eq!(hull_near(p, to, low, high, &[hull], 0).0, old_near);
                }
            }
        }
    }
    // ADR-479 implementation retained as the differential oracle.
    impl Bursts {
        fn reference_rubble(
            &self,
            world: &voxel::World,
            yielding: &[Mat],
            hulls: &[Hull],
        ) -> nav::vehicle::Rubble {
            // Floor is what something holds: the world, or a resting piece
            // under it — a pile on a rock piece is floor at the pile's top.
            // Only the world was asked, and a stacked piece was missing from
            // the map (ADR-458). A piece on a roof, or one whose support was
            // taken and whose turn to fall has not come, is not floor; one a
            // hull carries is not asked about the pieces (ADR-469).
            let mut floor = Vec::new();
            let mut stacked = Vec::new();
            for (i, f) in self.fragments.iter().enumerate() {
                if !f.resting || f.chunk.is_some() || yielding.contains(&f.material) {
                    continue;
                }
                if supported(f, world, &[], &Settled::default(), u32::MAX) {
                    floor.push(i);
                } else if !hulls
                    .iter()
                    .any(|h| h.carries(f.position.y.add(f.bounds().0[1]), f.position))
                {
                    stacked.push(i);
                }
            }
            if !stacked.is_empty() {
                let settled = Settled::around(&self.fragments, hulls, |i, _| {
                    stacked.binary_search(&i).is_ok()
                });
                floor.extend(
                    stacked
                        .into_iter()
                        .filter(|&i| supported(&self.fragments[i], world, &[], &settled, i as u32)),
                );
            }
            // A hull covers no column outside the box around its footprint:
            // a piece far from every hull asks none of them (ADR-465).
            let reach: Vec<([Fx; 2], [Fx; 2])> = hulls.iter().map(Hull::footprint_box).collect();
            let mut against: Vec<&Hull> = Vec::with_capacity(hulls.len());
            // The columns with their tops, gathered and sorted, and the map
            // built from them at once: thousands of single insertions into
            // the tree cost a millisecond a step on a battlefield (ADR-465).
            let mut columns: Vec<([i32; 2], u16)> = Vec::new();
            for f in floor.into_iter().map(|i| &self.fragments[i]) {
                let (low, high) = f.bounds();
                let p = arr(f.position);
                // Floor of the bottom corners, ceiling of the top: the columns
                // the box covers and the height it reaches.
                let lo = [p[0].add(low[0]), p[2].add(low[2])].map(|v| (v.raw() >> 32) as i32);
                let hi = [p[0].add(high[0]), p[2].add(high[2])].map(|v| (v.raw() >> 32) as i32);
                let top = -(p[1].add(high[1]).neg().raw() >> 32);
                let top = top.clamp(0, i64::from(u16::MAX)) as u16;
                against.clear();
                against.extend(hulls.iter().zip(&reach).filter_map(|(h, (b0, b1))| {
                    let near = (0..2).all(|a| {
                        Fx::from_int(lo[a]) <= b1[a] && Fx::from_int(hi[a]).add(Fx::ONE) >= b0[a]
                    });
                    (near && !h.covers_point([p[0], p[2]])).then_some(h)
                }));
                for z in lo[1].max(0)..=hi[1] {
                    for x in lo[0].max(0)..=hi[0] {
                        if against.iter().any(|h| h.covers(x, z)) {
                            continue;
                        }
                        columns.push(([x, z], top));
                    }
                }
            }
            // A column under several pieces holds the highest top.
            sort_columns(&mut columns);
            columns.dedup_by(|later, kept| {
                let same = later.0 == kept.0;
                if same {
                    kept.1 = kept.1.max(later.1);
                }
                same
            });
            nav::vehicle::Rubble::from_sorted(columns)
        }
    }
    #[test]
    fn rubble_reused_boxes_match_reference_after_support_and_hull_changes() {
        let (mut b, mut w) = (Bursts::default(), world());
        for i in 0..192u32 {
            let side = (i % 4 + 1) as u8;
            let full = u64::MAX >> (64 - u32::from(side).pow(3));
            let shape = if i % 3 == 0 {
                full & 0x9d37_a518_6fc2_0be1
            } else {
                full
            };
            let turn = (i % 7) as i32 * 2731;
            let (c, s) = (angle::cos(turn), angle::sin(turn));
            b.fragments.push(Fragment {
                id: u64::from(i) + 1,
                position: V3::new(
                    Fx::from_int(32 + (i % 8) as i32 * 4),
                    Fx::from_int(12 + (i / 64) as i32 * 4).add(Fx::from_ratio(i32::from(side), 2)),
                    Fx::from_int(32 + (i / 8 % 8) as i32 * 4),
                ),
                velocity: V3::ZERO,
                material: (i % 3 + 1) as Mat,
                paint: None,
                mm: 100,
                basis: [
                    V3::new(c, Fx::ZERO, s),
                    V3::UP,
                    V3::new(s.neg(), Fx::ZERO, c),
                ],
                mass_g: 1200,
                count: shape.count_ones(),
                shape,
                side,
                chunk: None,
                held: 0,
                resting: i % 5 != 0,
            });
        }
        let solid = |[x, y, z]: [i32; 3]| {
            (42..54).contains(&x) && (42..54).contains(&z) && (14..16).contains(&y)
        };
        for pass in 0..4 {
            let hull = Hull {
                pos: [
                    Fx::from_int(48 + pass * 3),
                    Fx::from_int(12),
                    Fx::from_int(48),
                ],
                head: pass * 2731,
                half: [Fx::from_int(6), Fx::from_int(6)],
                height: Fx::from_int(10),
                vel: [Fx::ZERO; 3],
                step: Fx::from_int(4),
                solid: (pass % 2 == 0).then_some(Solid(&solid)),
            };
            for hulls in [&[][..], std::slice::from_ref(&hull)] {
                for yielding in [&[][..], &[1][..], &[1, 2, 3][..]] {
                    assert_eq!(
                        b.rubble(&w, yielding, hulls),
                        b.reference_rubble(&w, yielding, hulls)
                    );
                }
            }
            // Queries after digging and fragment movement cannot reuse an
            // earlier result, even when no simulation tick has elapsed.
            for x in 32 + pass * 8..40 + pass * 8 {
                for z in 32..64 {
                    for y in 8..12 {
                        w.queue_edit(Edit::dig(x as u32, y, z));
                    }
                }
            }
            w.flush();
            for (i, f) in b.fragments.iter_mut().enumerate() {
                if i % 7 == 0 {
                    f.resting = !f.resting;
                    f.position.y = f.position.y.add(Fx::ONE);
                }
            }
        }
    }
    #[test]
    fn scattered_impulses_are_varied_replayable_and_never_add_energy() {
        let t = tuning().blast;
        let direction = V3::new(Fx::from_int(3), Fx::from_int(2), Fx::ONE);
        let mut velocities = BTreeSet::new();
        let mut speeds = BTreeSet::new();
        for id in 1..128 {
            let v = scattered_launch(id, direction, Fx::ONE, &t);
            assert_eq!(v, scattered_launch(id, direction, Fx::ONE, &t));
            assert!(v.len_sq() <= Fx::ONE);
            velocities.insert(arr(v).map(Fx::raw));
            speeds.insert(v.len_sq().raw());
        }
        assert!(velocities.len() > 120 && speeds.len() > 60);
        assert_eq!(scattered_launch(1, direction, Fx::ZERO, &t), V3::ZERO);
    }
    #[test]
    fn fracture_partition_contains_both_large_chunks_and_small_chips() {
        let t = tuning().blast;
        let mut groups = BTreeMap::<([u32; 3], u8), u64>::new();
        for z in 0..16 {
            for y in 0..8 {
                for x in 0..16 {
                    let p = [x, y, z];
                    let (origin, side) = fracture_box(p, 4, &t);
                    let s = u32::from(side);
                    let bit = x - origin[0] + s * (y - origin[1]) + s * s * (z - origin[2]);
                    let mask = groups.entry((origin, side)).or_default();
                    assert_eq!(*mask & (1 << bit), 0);
                    *mask |= 1 << bit;
                }
            }
        }
        assert!(groups.values().any(|m| m.count_ones() == 64));
        assert!(groups.values().any(|m| m.count_ones() == 8));
        assert_eq!(
            groups.values().map(|m| m.count_ones()).sum::<u32>(),
            16 * 8 * 16
        );
    }
    fn walked_shape_geometry(shape: u64, side: u8) -> (V3, ([Fx; 3], [Fx; 3])) {
        let mut sum = [0; 3];
        let mut lo = [i32::from(side) - 1; 3];
        let mut hi = [0; 3];
        for z in 0..side {
            for y in 0..side {
                for x in 0..side {
                    let i = u32::from(x)
                        + u32::from(side) * u32::from(y)
                        + u32::from(side).pow(2) * u32::from(z);
                    if shape & (1 << i) != 0 {
                        for (a, value) in [x, y, z].map(i32::from).into_iter().enumerate() {
                            sum[a] += value;
                            lo[a] = lo[a].min(value);
                            hi[a] = hi[a].max(value);
                        }
                    }
                }
            }
        }
        let centre = sum.map(|v| Fx::from_ratio(v, shape.count_ones().max(1) as i32));
        (
            v3(centre),
            (
                [0, 1, 2].map(|a| {
                    Fx::from_int(lo[a])
                        .sub(centre[a])
                        .sub(Fx::from_ratio(49, 100))
                }),
                [0, 1, 2].map(|a| {
                    Fx::from_int(hi[a])
                        .sub(centre[a])
                        .add(Fx::from_ratio(49, 100))
                }),
            ),
        )
    }
    #[test]
    fn bit_plane_geometry_matches_cell_walk_for_dense_sparse_and_empty_shapes() {
        for side in 1..=4 {
            let cells = u32::from(side).pow(3);
            let full = u64::MAX >> (64 - cells);
            let mut shapes = vec![0, full];
            for i in 0..cells {
                shapes.extend([1 << i, full ^ (1 << i)]);
            }
            if side <= 2 {
                shapes.extend(0..=full);
            } else {
                let mut bits = 0x9d37_a518_6fc2_0be1u64;
                for _ in 0..4096 {
                    bits ^= bits << 13;
                    bits ^= bits >> 7;
                    bits ^= bits << 17;
                    shapes.push(bits & full);
                }
            }
            for shape in shapes {
                let (centre, bounds) = walked_shape_geometry(shape, side);
                assert_eq!(
                    shape_centre(shape, side),
                    centre,
                    "side={side} shape={shape:x}"
                );
                assert_eq!(
                    shape_bounds(shape, side),
                    bounds,
                    "side={side} shape={shape:x}"
                );
            }
        }
    }
    #[test]
    fn settled_query_words_match_cells_across_negative_and_word_boundaries() {
        for bottom in [-129, -65, -64, -1, 0, 1, 63, 64, 127] {
            for height in [1, 2, 63, 64, 65, 131] {
                let mut words = SettledQueries::default();
                let mut cells = BTreeSet::new();
                let boxes = [
                    ([-2, bottom, 1], [0, bottom + height - 1, 2]),
                    ([-1, bottom - 2, 0], [1, bottom + 2, 1]),
                ];
                // Repeated and overlapping requests must retain every bit.
                for &(lo, hi) in boxes.iter().chain(boxes.iter().rev()) {
                    words.include(lo, hi);
                    for z in lo[2]..=hi[2] {
                        for y in lo[1]..=hi[1] {
                            for x in lo[0]..=hi[0] {
                                cells.insert([x, y, z]);
                            }
                        }
                    }
                }
                for z in -1..=3 {
                    for y in bottom - 3..=bottom + height.max(4) {
                        for x in -3..=2 {
                            let p = [x, y, z];
                            assert_eq!(words.contains(p), cells.contains(&p), "{p:?}");
                        }
                    }
                }
            }
        }
        assert!(!SettledQueries::default().contains([0; 3]));
    }
    #[test]
    fn settled_index_matches_unpacked_requests_and_preserves_piece_indices() {
        let t = tuning().blast;
        let hull = Hull {
            pos: [Fx::from_int(8), Fx::ZERO, Fx::from_int(4)],
            head: 2731,
            half: [Fx::from_int(2); 2],
            height: Fx::from_int(6),
            vel: [Fx::ZERO; 3],
            step: Fx::ONE,
            solid: None,
        };
        let mut fragments: Vec<_> = (0..48u32)
            .map(|i| Fragment {
                id: u64::from(i) + 1,
                position: V3::new(
                    Fx::from_ratio((i % 8) as i32 * 5 - 9, 2),
                    Fx::from_ratio((i / 8) as i32 * 5 - 1, 2),
                    Fx::from_int((i % 3) as i32 * 4),
                ),
                velocity: V3::ZERO,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1,
                count: 3,
                shape: 7,
                side: 2,
                chunk: None,
                held: i,
                resting: i % 3 != 0,
            })
            .collect();
        // Far sleepers and interleaved flying pieces leave gaps in the
        // indices held by the grid. Its box table must not compact them.
        fragments[1].position.x = Fx::from_int(1000);
        let mut index = Settled::default();
        let mut queries = SettledQueries::default();
        for tick in [0, 1, u64::from(t.rest_check_ticks)] {
            let world_boxes: Vec<_> = fragments
                .iter()
                .map(|f| {
                    let (lo, hi) = f.bounds();
                    let p = arr(f.position);
                    (
                        [0, 1, 2].map(|a| p[a].add(lo[a])),
                        [0, 1, 2].map(|a| p[a].add(hi[a])),
                    )
                })
                .collect();
            let mut want = BTreeSet::new();
            for (i, f) in fragments.iter().enumerate() {
                if f.resting && !due(f.id, tick, &t) {
                    continue;
                }
                let (lo, hi) = settled_cells(world_boxes[i].0, world_boxes[i].1);
                for z in lo[2] - 1..=hi[2] + 1 {
                    for y in lo[1] - 1..=hi[1] + 1 {
                        for x in lo[0] - 1..=hi[0] + 1 {
                            want.insert([x, y, z]);
                        }
                    }
                }
            }
            let mut expected = HashMap::<_, Vec<u32>>::new();
            for (i, f) in fragments.iter().enumerate() {
                if !f.resting || hull.carries(world_boxes[i].0[1], f.position) {
                    continue;
                }
                let (lo, hi) = settled_cells(world_boxes[i].0, world_boxes[i].1);
                for z in lo[2]..=hi[2] {
                    for y in lo[1]..=hi[1] {
                        for x in lo[0]..=hi[0] {
                            if want.contains(&[x, y, z]) {
                                expected.entry([x, y, z]).or_default().push(i as u32);
                            }
                        }
                    }
                }
            }
            Settled::of_into(&fragments, &[hull], tick, &t, &mut index, &mut queries);
            assert!(!expected.is_empty());
            assert_eq!(index.cells.len(), expected.len());
            for (cell, pieces) in &expected {
                assert_eq!(index.cells.get(cell), Some(pieces));
            }
            for (i, &(lo, hi)) in world_boxes.iter().enumerate() {
                for skip in [i as u32, u32::MAX] {
                    for from_above in [None, Some(lo[1]), Some(hi[1])] {
                        let (c0, c1) = settled_cells(lo, hi);
                        let hit = expected.iter().any(|(cell, pieces)| {
                            (0..3).all(|a| cell[a] >= c0[a] && cell[a] <= c1[a])
                                && pieces.iter().any(|&other| {
                                    let (qlo, qhi) = world_boxes[other as usize];
                                    other != skip
                                        && from_above.is_none_or(|bottom| qhi[1] <= bottom)
                                        && (0..3).all(|a| lo[a] < qhi[a] && qlo[a] < hi[a])
                                })
                        });
                        assert_eq!(index.hit(lo, hi, skip, from_above), hit);
                    }
                }
            }
        }
        for f in &mut fragments {
            f.resting = false;
        }
        Settled::of_into(&fragments, &[], 0, &t, &mut index, &mut queries);
        assert!(index.cells.is_empty() && index.boxes.is_empty());
        fragments[0].resting = true;
        let sleeping = Settled::of(&fragments[..1], &[], 0, &t);
        assert!(!due(fragments[0].id, 0, &t));
        assert!(sleeping.cells.is_empty() && sleeping.boxes.is_empty());
    }
    #[test]
    fn reduced_fragment_bounds_match_all_eight_rounded_corners() {
        for side in 1..=4 {
            let cells = box_cells(side);
            let mask = u64::MAX >> (64 - cells);
            let shapes: Vec<_> = if side == 2 {
                (1..=mask).collect()
            } else {
                vec![1, 1 << (cells - 1), mask, mask & 0x9d37_a518_6fc2_0be1]
            };
            for shape in shapes {
                let centre = arr(walked_shape_geometry(shape, side).0);
                let mut lo = [i32::from(side) - 1; 3];
                let mut hi = [0; 3];
                for i in 0..cells {
                    if shape & (1 << i) != 0 {
                        let p = shape_cell(i, side);
                        for a in 0..3 {
                            lo[a] = lo[a].min(p[a]);
                            hi[a] = hi[a].max(p[a]);
                        }
                    }
                }
                let lo = [0, 1, 2].map(|a| {
                    Fx::from_int(lo[a])
                        .sub(centre[a])
                        .sub(Fx::from_ratio(49, 100))
                });
                let hi = [0, 1, 2].map(|a| {
                    Fx::from_int(hi[a])
                        .sub(centre[a])
                        .add(Fx::from_ratio(49, 100))
                });
                for mm in [1, 63, 100, 500, u16::MAX] {
                    let scale = Fx::from_ratio(i32::from(mm), voxel::unit::WORLD_MM as i32);
                    for turn in [0, 1, 2731, angle::FULL / 4, -11739] {
                        let (c, s) = (angle::cos(turn), angle::sin(turn));
                        let (cp, sp) = (angle::cos(turn / 3), angle::sin(turn / 3));
                        let basis = [
                            V3::new(c, Fx::ZERO, s),
                            V3::UP,
                            V3::new(s.neg(), Fx::ZERO, c),
                        ]
                        .map(|v| {
                            V3::new(
                                v.x,
                                v.y.mul(cp).sub(v.z.mul(sp)),
                                v.y.mul(sp).add(v.z.mul(cp)),
                            )
                        });
                        let mut expected = ([Fx::MAX; 3], [Fx::MIN; 3]);
                        for corner in 0..8 {
                            let mut p = V3::ZERO;
                            for a in 0..3 {
                                let endpoint = if corner & (1 << a) == 0 { lo[a] } else { hi[a] };
                                p = p.add(basis[a].scale(endpoint.mul(scale)));
                            }
                            for (a, value) in arr(p).into_iter().enumerate() {
                                expected.0[a] = expected.0[a].min(value);
                                expected.1[a] = expected.1[a].max(value);
                            }
                        }
                        let fragment = Fragment {
                            id: 1,
                            position: V3::ZERO,
                            velocity: V3::ZERO,
                            material: 1,
                            paint: None,
                            mm,
                            basis,
                            mass_g: 1,
                            count: shape.count_ones(),
                            shape,
                            side,
                            chunk: None,
                            held: 0,
                            resting: false,
                        };
                        assert_eq!(
                            fragment.bounds(),
                            expected,
                            "side={side} shape={shape:x} mm={mm} turn={turn}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn empty_corner_of_a_fragment_does_not_glue_it_to_a_wall() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.queue_edit(Edit::put(49, 14, 48, 2));
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let start = centre([48, 13, 48]).add(shape_centre(7, 2));
        b.fragments.push(Fragment {
            id: 1,
            position: start,
            velocity: V3::new(Fx::from_int(-2), Fx::ZERO, Fx::ZERO),
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 3600,
            count: 3,
            shape: 7,
            side: 2,
            chunk: None,
            held,
            resting: false,
        });
        b.step_fragments(
            &w,
            &mut e,
            &t.blast,
            crate::gravity_per_tick(&t),
            0,
            Surroundings::NONE,
        );
        assert!(b.fragments[0].position.x < start.x.sub(Fx::ONE));
        assert!(!b.fragments[0].resting);
        for _ in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                0,
                Surroundings::NONE,
            );
        }
        assert!(b.fragments[0].resting);
        assert!(b.fragments[0].position.y < Fx::from_int(14));
    }

    /// A piece on a shelf stays put while the shelf stands, and when the
    /// shelf is dug away it wakes within the check period, falls to the
    /// ground and settles again, its wreck entry following (ADR-291).
    #[test]
    fn a_resting_piece_falls_again_when_the_ground_under_it_is_dug() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.queue_edit(Edit::put(48, 14, 48, 2));
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 16, 48]),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: false,
        });
        let mut tick = 0;
        while !b.fragments[0].resting {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            tick += 1;
            assert!(tick < 300, "never rested");
        }
        let shelf = b.fragments[0].position.y;
        assert!(
            shelf > Fx::from_int(15) && shelf < Fx::from_int(16),
            "not on the shelf: {shelf:?}"
        );
        assert_eq!(e.wrecks.iter().filter(|w| w.held == held).count(), 1);
        for _ in 0..t.blast.rest_check_ticks * 2 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            tick += 1;
        }
        assert!(b.fragments[0].resting);
        assert_eq!(b.fragments[0].position.y, shelf);
        w.queue_edit(Edit::dig(48, 14, 48));
        w.begin_tick();
        w.flush();
        assert!(!w.solid(48, 14, 48));
        let mut woke = false;
        for _ in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            tick += 1;
            woke |= !b.fragments[0].resting;
            if woke && b.fragments[0].resting {
                break;
            }
        }
        assert!(woke, "the piece never woke");
        assert!(b.fragments[0].resting, "the piece did not settle again");
        assert!(
            b.fragments[0].position.y < Fx::from_int(14),
            "still up: {:?}",
            b.fragments[0].position
        );
        let wrecks: Vec<_> = e.wrecks.iter().filter(|w| w.held == held).collect();
        assert_eq!(wrecks.len(), 1);
        assert!(wrecks[0].y < 14);
        assert!(e.balanced());
    }

    /// **A waking piece does not take a claim's flying slot** (ADR-443).
    /// A claim whose dig the edit ceiling deferred holds its slot into the
    /// next tick; a piece woken meanwhile took it, and the claim's piece
    /// then flew over the budget.
    #[test]
    fn a_waking_piece_waits_behind_a_claim() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let mut t = tuning();
        t.blast.max_flying = 1;
        w.queue_edit(Edit::put(48, 14, 48, 2));
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 16, 48]),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: false,
        });
        let g = crate::gravity_per_tick(&t);
        let mut tick = 0;
        while !b.fragments[0].resting {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, Surroundings::NONE);
            tick += 1;
            assert!(tick < 300, "never rested");
        }
        // A claim takes the one flying slot; the shelf goes.
        assert!(b.claim_voxel(&mut w, [20, 10, 20], 1, [20, 12, 20], 0, 1, &t.blast));
        w.queue_edit(Edit::dig(48, 14, 48));
        w.begin_tick();
        w.flush();
        for _ in 0..t.blast.rest_check_ticks * 2 {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, Surroundings::NONE);
            tick += 1;
            assert!(b.fragments[0].resting, "the piece took the claim's slot");
        }
        b.claims.clear();
        let mut woke = false;
        for _ in 0..t.blast.rest_check_ticks * 2 {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, Surroundings::NONE);
            tick += 1;
            woke |= !b.fragments[0].resting;
        }
        assert!(woke, "the piece never woke once the slot was free");
    }

    /// Two pieces settling in one pile cell become one upright heap on the
    /// floor with a cell for each of theirs, and the heap is floor in its
    /// columns (ADR-292).
    #[test]
    fn pieces_settling_together_become_an_upright_heap_that_is_floor() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        for (id, x) in [(1, 48), (2, 49)] {
            let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
            b.fragments.push(Fragment {
                id,
                position: centre([x, 16, 48]),
                velocity: V3::ZERO,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1200,
                count: 1,
                shape: 1,
                side: 1,
                chunk: None,
                held,
                resting: false,
            });
        }
        for tick in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(b.fragments.len(), 1, "the pieces did not merge");
        let f = &b.fragments[0];
        assert!(f.resting);
        assert_eq!((f.side, f.shape, f.count), (4, 0b11, 2));
        assert_eq!(f.basis, IDENTITY);
        // Its bottom sits on the ground at 12 and its top one cell up.
        assert_eq!(f.position.y, Fx::from_ratio(25, 2));
        let rubble = b.rubble(&w, &[], &[]);
        assert_eq!(rubble.get(&[48, 48]), Some(&13));
        assert!(
            b.rubble(&w, &[1], &[]).is_empty(),
            "a yielding heap is no floor"
        );
        assert_eq!(rubble.get(&[49, 48]), Some(&13));
        assert_eq!(rubble.get(&[51, 48]), None);
        assert!(e.balanced());
    }

    /// **A pile grows on what its piece rests on** (ADR-457). A merge
    /// dropped the pile onto the world under its centre: a soil piece
    /// lying on a rock piece sank into the rock when a second soil piece
    /// settled in its pile cell.
    #[test]
    fn a_pile_merged_on_a_resting_piece_stays_on_it() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        // A ledge of the world, four high, for the second soil piece.
        for y in 12..16 {
            w.queue_edit(Edit::put(54, y, 49, 2));
        }
        w.flush();
        let piece = |e: &mut Economy, id, position, material, shape, side, count| Fragment {
            id,
            position,
            velocity: V3::ZERO,
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200 * u64::from(count),
            count,
            shape,
            side,
            chunk: None,
            held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
            resting: true,
        };
        // A rock block on the ground, 48..52 by 12..16 by 48..52; a soil
        // piece on it; another on the ledge, in the same pile cell.
        let rock = V3::new(Fx::from_int(50), Fx::from_int(14), Fx::from_int(50));
        b.fragments.push(piece(&mut e, 1, rock, 2, u64::MAX, 4, 64));
        b.fragments
            .push(piece(&mut e, 2, centre([49, 16, 49]), 1, 1, 1, 1));
        b.fragments
            .push(piece(&mut e, 3, centre([54, 16, 49]), 1, 1, 1, 1));
        b.step_fragments(
            &w,
            &mut e,
            &t.blast,
            crate::gravity_per_tick(&t),
            0,
            Surroundings::NONE,
        );
        assert_eq!(b.fragments.len(), 2, "the soil pieces did not merge");
        let pile = &b.fragments[1];
        assert_eq!((pile.material, pile.count), (1, 2));
        assert!(pile.resting);
        let bottom = pile.position.y.add(pile.bounds().0[1]);
        assert!(
            bottom >= Fx::from_int(16) && bottom < Fx::from_ratio(33, 2),
            "the pile sank into the rock under it: bottom {bottom:?}"
        );
        assert!(e.balanced());
    }

    /// **A piece lying on a piece is floor at its top** (ADR-458). The
    /// rubble map took only what the world held, and a pile on a rock
    /// piece was missing: the hull met the rock and not the pile on it.
    /// A piece whose support is gone is still no floor.
    #[test]
    fn a_stacked_piece_is_floor_and_a_hovering_one_is_not() {
        let (mut b, mut e) = (Bursts::default(), economy());
        let w = world();
        let piece = |e: &mut Economy, id, position, material, shape, side| Fragment {
            id,
            position,
            velocity: V3::ZERO,
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape,
            side,
            chunk: None,
            held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
            resting: true,
        };
        // A rock block on the ground, 48..52 by 12..16 by 48..52, a soil
        // piece on it, and one left hanging at 20 over bare ground.
        let rock = V3::new(Fx::from_int(50), Fx::from_int(14), Fx::from_int(50));
        b.fragments.push(piece(&mut e, 1, rock, 2, u64::MAX, 4));
        b.fragments
            .push(piece(&mut e, 2, centre([49, 16, 49]), 1, 1, 1));
        b.fragments
            .push(piece(&mut e, 3, centre([60, 20, 60]), 1, 1, 1));
        let rubble = b.rubble(&w, &[], &[]);
        assert_eq!(rubble.get(&[50, 50]), Some(&16), "the rock is not floor");
        assert_eq!(
            rubble.get(&[49, 49]),
            Some(&17),
            "the stacked piece is not floor"
        );
        assert_eq!(rubble.get(&[60, 60]), None, "a hanging piece is floor");
    }

    /// **A piece on a roof holds nothing** (ADR-293, ADR-458): what falls
    /// on it lands on the roof. The test for a rider asked whether the
    /// piece's centre lay inside the hull's box, and a piece on a roof
    /// has it above: riders held what fell on them.
    #[test]
    fn a_piece_on_a_roof_holds_nothing() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let hull = Hull {
            pos: [Fx::from_int(48), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(6)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::ZERO,
            solid: None,
        };
        let piece = |e: &mut Economy, id, at: [u32; 3], material| Fragment {
            id,
            position: centre(at),
            velocity: V3::ZERO,
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
            resting: false,
        };
        // A rider on the roof at 15, and a piece of another matter
        // dropped onto it from above.
        b.fragments.push(piece(&mut e, 1, [48, 17, 48], 1));
        let around = Surroundings {
            hulls: &[hull],
            yielding: &[],
        };
        let g = crate::gravity_per_tick(&t);
        let mut tick = 0;
        while !b.fragments[0].resting {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, around);
            tick += 1;
            assert!(tick < 300, "the rider never rested");
        }
        b.fragments.push(piece(&mut e, 2, [48, 22, 48], 2));
        while !b.fragments[1].resting {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, around);
            tick += 1;
            assert!(tick < 300, "the second piece never rested");
        }
        let bottom = |f: &Fragment| f.position.y.add(f.bounds().0[1]);
        assert!(
            bottom(&b.fragments[1]) < bottom(&b.fragments[0]).add(Fx::from_ratio(1, 2)),
            "the rider held the piece: {:?} over {:?}",
            b.fragments[1].position,
            b.fragments[0].position
        );
    }

    /// **A piece meets a hull where it meets the hull's voxels**
    /// (ADR-469), not its box: dropped over the deck, it rests on the deck
    /// under the turret's height and rides there; over the turret, on the
    /// turret; thrown across above the deck, it passes the box's side and
    /// stops at the turret; and one lying under the belly still leaves the
    /// box by the nearest side (ADR-302).
    #[test]
    fn a_piece_meets_the_hulls_voxels_and_not_its_box() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        // A deck at 14..16 over the whole footprint, a turret at 16..22
        // over the middle, and the belly clear under the deck.
        let shape = |[x, y, z]: [i32; 3]| {
            let over = |lo, hi| (lo..hi).contains(&x) && (lo..hi).contains(&z);
            over(42, 54) && (14..16).contains(&y) || over(46, 50) && (16..22).contains(&y)
        };
        let hull = Hull {
            pos: [Fx::from_int(48), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(6)],
            height: Fx::from_int(10),
            vel: [Fx::ZERO; 3],
            step: Fx::ZERO,
            solid: Some(Solid(&shape)),
        };
        let around = Surroundings {
            hulls: &[hull],
            yielding: &[],
        };
        let g = crate::gravity_per_tick(&t);
        let mut tick = 0;
        let mut rest = |b: &mut Bursts, e: &mut Economy, at: [u32; 3], velocity: V3| {
            b.fragments.clear();
            b.fragments.push(Fragment {
                id: 1,
                position: centre(at),
                velocity,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1200,
                count: 1,
                shape: 1,
                side: 1,
                chunk: None,
                held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
                resting: false,
            });
            let start = tick;
            while !b.fragments[0].resting {
                b.step_fragments(&w, e, &t.blast, g, tick, around);
                tick += 1;
                assert!(tick - start < 300, "the piece from {at:?} never rested");
            }
            let f = &b.fragments[0];
            (f.position, f.position.y.add(f.bounds().0[1]))
        };
        let (at, bottom) = rest(&mut b, &mut e, [44, 30, 48], V3::ZERO);
        assert!(
            bottom >= Fx::from_int(16) && bottom < Fx::from_ratio(33, 2),
            "not on the deck: {at:?}"
        );
        assert!(hull.carries(bottom, at), "the deck does not carry it");
        let (at, bottom) = rest(&mut b, &mut e, [48, 30, 48], V3::ZERO);
        assert!(
            bottom >= Fx::from_int(22) && bottom < Fx::from_ratio(45, 2),
            "not on the turret: {at:?}"
        );
        // Across above the deck, from afar and from the box's very side,
        // where the piece's box lies in the hull's and its centre outside.
        for (from, speed) in [
            ([34, 18, 48], Fx::from_int(2)),
            ([41, 18, 48], Fx::from_ratio(1, 2)),
        ] {
            let across = V3::new(speed, Fx::ZERO, Fx::ZERO);
            let (at, bottom) = rest(&mut b, &mut e, from, across);
            assert!(
                at.x > Fx::from_int(42) && at.x < Fx::from_int(46),
                "did not stop between the box's side and the turret: {at:?}"
            );
            assert!(
                bottom >= Fx::from_int(16) && bottom < Fx::from_ratio(33, 2),
                "not on the deck: {at:?}"
            );
        }
        b.fragments[0].position = centre([44, 12, 48]);
        b.fragments[0].resting = false;
        b.step_fragments(&w, &mut e, &t.blast, g, tick, around);
        let at = b.fragments[0].position;
        assert!(at.x < Fx::from_int(42), "stayed under the belly: {at:?}");
        assert!(e.balanced());
    }

    /// A piece falling onto a live hull rests on its roof, and when the
    /// hull is gone from under it, it falls to the ground (ADR-293).
    #[test]
    fn a_piece_rests_on_a_hull_and_falls_when_the_hull_leaves() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 20, 48]),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: false,
        });
        let hull = Hull {
            pos: [Fx::from_int(48), Fx::from_int(12), Fx::from_int(48)],
            head: angle::FULL / 8,
            half: [Fx::from_int(3), Fx::from_int(2)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::ZERO,
            solid: None,
        };
        let mut tick = 0;
        while !b.fragments[0].resting {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings {
                    hulls: &[hull],
                    yielding: &[],
                },
            );
            tick += 1;
            assert!(tick < 300, "never rested");
        }
        let roof = b.fragments[0].position.y;
        assert!(
            roof > Fx::from_int(15) && roof < Fx::from_int(16),
            "not on the roof: {roof:?}"
        );
        assert!(
            b.rubble(&w, &[], &[]).is_empty(),
            "a piece on a roof is no heap"
        );
        let mut woke = false;
        for _ in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            tick += 1;
            woke |= !b.fragments[0].resting;
            if woke && b.fragments[0].resting {
                break;
            }
        }
        assert!(
            woke && b.fragments[0].resting,
            "the piece did not fall and settle"
        );
        assert!(
            b.fragments[0].position.y < Fx::from_int(13),
            "still up: {:?}",
            b.fragments[0].position
        );
        assert!(e.balanced());
    }

    /// A hull driving into a lying piece bulldozes it: the piece leaves
    /// ahead of the hull and settles again outside it (ADR-294).
    #[test]
    fn a_moving_hull_bulldozes_a_lying_piece_out_of_its_way() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 12, 48]),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: true,
        });
        let mut hull = Hull {
            pos: [Fx::from_int(40), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(3), Fx::from_int(2)],
            height: Fx::from_int(3),
            vel: [Fx::from_ratio(1, 2), Fx::ZERO, Fx::ZERO],
            step: Fx::ZERO,
            solid: None,
        };
        let mut tick = 0;
        let mut moved = false;
        for _ in 0..40 {
            hull.pos[0] = hull.pos[0].add(hull.vel[0]);
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings {
                    hulls: &[hull],
                    yielding: &[],
                },
            );
            tick += 1;
            moved |= !b.fragments[0].resting;
        }
        assert!(moved, "the hull drove through the piece");
        hull.vel = [Fx::ZERO; 3];
        for _ in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings {
                    hulls: &[hull],
                    yielding: &[],
                },
            );
            tick += 1;
            if b.fragments[0].resting {
                break;
            }
        }
        let f = &b.fragments[0];
        let front = hull.pos[0].add(hull.half[0]);
        assert!(f.resting, "the piece never settled again");
        assert!(
            f.position.x >= front.sub(Fx::ONE),
            "the piece stayed under the hull: {:?}, front {front:?}",
            f.position
        );
        assert!(
            f.position.y < Fx::from_int(13),
            "the piece did not come down: {:?}",
            f.position
        );
        let wrecks: Vec<_> = e.wrecks.iter().filter(|w| w.held == held).collect();
        assert_eq!(wrecks.len(), 1, "the pushed piece lost its wreck marker");
        assert_eq!(wrecks[0].x, f.position.x.to_int_trunc());
        assert!(e.balanced());
    }

    /// Cells the world let go of become pieces at zero speed; with no
    /// room in the pool the whole shape goes back into the world, so no
    /// matter is lost (ADR-299).
    #[test]
    fn unsupported_cells_become_pieces_or_go_back_when_the_pool_is_full() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let mut t = tuning().blast;
        w.flush();
        let cells: Vec<([u32; 3], Mat)> = (0..8u32)
            .map(|i| ([48 + i % 2, 20 + i / 2 % 2, 48 + i / 4], 1))
            .collect();
        let (made, returned) = b.pieces_from_cells(&mut w, &cells, &mut e, &t, 2);
        assert_eq!((made, returned), (1, 0));
        assert_eq!(b.fragments[0].count, 8);
        assert_eq!(b.fragments[0].velocity, V3::ZERO);
        assert_eq!(e.ledger.held(b.fragments[0].held), 8 * e.per_voxel(1));
        assert!(e.balanced());
        t.max_fragments = 1;
        let more: Vec<([u32; 3], Mat)> = vec![([60, 20, 60], 1)];
        let (made, returned) = b.pieces_from_cells(&mut w, &more, &mut e, &t, 2);
        assert_eq!((made, returned), (0, 1));
        w.begin_tick();
        w.flush();
        assert!(w.solid(60, 20, 60), "the cell did not go back");
        assert!(e.balanced());
    }

    struct Ground;
    impl voxel::world::Generator for Ground {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < 12 {
                1
            } else {
                AIR
            }
        }
    }
    fn tuning() -> crate::Tuning {
        crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap()
    }
    /// A flying piece tumbles (ADR-309): thrown, it turns as it goes and
    /// keeps the face it landed on; laid down with no speed, it never
    /// turns. Its axes stay square to one another whatever it did.
    #[test]
    fn a_thrown_piece_tumbles_and_one_laid_down_does_not() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let piece = |id, x: i32, velocity| Fragment {
            id,
            position: V3::new(
                Fx::from_int(x).add(Fx::from_ratio(1, 2)),
                Fx::from_int(20),
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
            ),
            velocity,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: false,
        };
        b.fragments
            .push(piece(1, 40, V3::new(Fx::from_int(2), Fx::ONE, Fx::ZERO)));
        b.fragments.push(piece(2, 56, V3::ZERO));
        for tick in 0..30 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        let thrown = &b.fragments[0];
        assert_ne!(thrown.basis, IDENTITY, "the thrown piece did not turn");
        for (i, row) in thrown.basis.iter().enumerate() {
            assert!(
                row.len_sq().sub(Fx::ONE).abs() < Fx::from_ratio(1, 100),
                "axis {i} is no longer of unit length: {:?}",
                row.len_sq()
            );
            let next = thrown.basis[(i + 1) % 3];
            let dot = row
                .x
                .mul(next.x)
                .add(row.y.mul(next.y))
                .add(row.z.mul(next.z));
            assert!(
                dot.abs() < Fx::from_ratio(1, 100),
                "axes {i} and {} are no longer square: {dot:?}",
                (i + 1) % 3
            );
        }
        // Dropped straight down with no speed of its own, it lands as it
        // was laid: falling alone is not tumbling.
        let still = &b.fragments[1];
        assert_eq!(still.basis[0], IDENTITY[0], "the still piece turned");
        // What it turned to when it stopped is what it keeps.
        let turned = thrown.basis;
        for tick in 30..90 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert!(
            b.fragments[0].resting,
            "still in the air after three seconds"
        );
        assert_ne!(b.fragments[0].basis, turned, "it stopped turning too early");
    }
    /// A piece falls onto one that landed before it and stands on it
    /// (ADR-308) instead of sinking through it to the ground. Two
    /// materials, so that the heap of a cell (ADR-292) does not gather
    /// them into one and hide the answer.
    #[test]
    fn a_piece_stands_on_the_piece_that_landed_before_it() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let piece = |id, material, y: i32, resting| Fragment {
            id,
            position: V3::new(
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
                Fx::from_int(y),
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
            ),
            velocity: V3::ZERO,
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 9600,
            count: 8,
            shape: 0xff,
            side: 2,
            chunk: None,
            held,
            resting,
        };
        // Ground is at 12: one piece of soil already lying on it, one of
        // concrete falling from six units up.
        b.fragments.push(piece(1, 1, 13, true));
        b.fragments.push(piece(2, 2, 19, false));
        for tick in 0..240 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(b.fragments.len(), 2, "the pieces merged");
        let (below, above) = (&b.fragments[0], &b.fragments[1]);
        assert!(above.resting && below.resting, "still falling");
        let (_, high) = below.bounds();
        let (low, _) = above.bounds();
        let top = below.position.y.add(high[1]);
        let bottom = above.position.y.add(low[1]);
        assert!(
            bottom >= top.sub(Fx::from_ratio(1, 2)),
            "the piece sank into the one below: bottom {bottom:?} against top {top:?}"
        );
        assert!(
            above.position.y > below.position.y,
            "the piece did not land on top: {:?} against {:?}",
            above.position.y,
            below.position.y
        );
    }
    /// **A fast piece stands on the piece below it too** (ADR-438). The
    /// settled index held only the cells beside a flying piece's box,
    /// and a piece thrown down at seven units a tick crossed the one
    /// lying below it within the tick without asking about it, and sank
    /// to the ground through it.
    #[test]
    fn a_fast_piece_stands_on_the_piece_below_it() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let piece = |id, material, y: i32, vy: i32, resting| Fragment {
            id,
            position: V3::new(
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
                Fx::from_int(y),
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
            ),
            velocity: V3::new(Fx::ZERO, Fx::from_int(vy), Fx::ZERO),
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 9600,
            count: 8,
            shape: 0xff,
            side: 2,
            chunk: None,
            held,
            resting,
        };
        b.fragments.push(piece(1, 1, 13, 0, true));
        b.fragments.push(piece(2, 2, 21, -7, false));
        for tick in 0..240 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        let (below, above) = (&b.fragments[0], &b.fragments[1]);
        assert!(above.resting && below.resting, "still falling");
        let (_, high) = below.bounds();
        let (low, _) = above.bounds();
        let top = below.position.y.add(high[1]);
        let bottom = above.position.y.add(low[1]);
        assert!(
            bottom >= top.sub(Fx::from_ratio(1, 2)),
            "the piece sank into the one below: bottom {bottom:?} against top {top:?}"
        );
    }
    #[test]
    fn overlapping_airborne_pieces_cannot_hold_each_other_up() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        for i in 0..2 {
            let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
            b.fragments.push(Fragment {
                id: i + 1,
                position: V3::new(
                    Fx::from_int(48),
                    Fx::from_int(30).add(Fx::from_ratio(i as i32, 4)),
                    Fx::from_int(48),
                ),
                velocity: V3::ZERO,
                material: i as u16 + 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 9600,
                count: 8,
                shape: 0xff,
                side: 2,
                chunk: None,
                held,
                resting: true,
            });
        }
        for tick in 0..180 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(b.fragments.len(), 2);
        assert!(
            b.fragments
                .iter()
                .all(|f| f.resting && f.position.y < Fx::from_int(16)),
            "mutual support left pieces in mid-air"
        );
    }
    /// A piece that came to rest against a live hull's flank is no floor
    /// under that hull (ADR-302): the columns of the hull's box it reaches
    /// stay out of the map, its own columns outside stay in, and a piece
    /// the hull stands over is floor.
    #[test]
    fn a_piece_against_a_flank_is_no_floor_under_the_hull() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        // Two by two by two, its bottom on the ground at 12.
        let piece = |id, x: i32, z: i32| Fragment {
            id,
            position: V3::new(
                Fx::from_int(x).add(Fx::from_ratio(1, 2)),
                Fx::from_int(13),
                Fx::from_int(z).add(Fx::from_ratio(1, 2)),
            ),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 9600,
            count: 8,
            shape: 0xff,
            side: 2,
            chunk: None,
            held,
            resting: true,
        };
        // Against the flank: its centre a column outside the box, its
        // edge reaching a column under it. Under the hull: centre inside.
        b.fragments.push(piece(1, 45, 48));
        b.fragments.push(piece(2, 50, 48));
        let hull = Hull {
            pos: [Fx::from_int(50), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(4), Fx::from_int(3)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::from_int(4),
            solid: None,
        };
        let free = b.rubble(&w, &[], &[]);
        assert!(
            free.contains_key(&[46, 48]),
            "no hull: every column is heap"
        );
        let map = b.rubble(&w, &[], &[hull]);
        assert!(
            !map.contains_key(&[46, 48]),
            "the flank piece lifts the hull"
        );
        assert!(
            map.contains_key(&[44, 48]),
            "the flank piece lost its own column"
        );
        assert!(
            map.contains_key(&[50, 48]),
            "the piece under the hull is no floor"
        );
    }
    /// A piece dug from under a hull at the map edge leaves the hull's box
    /// by the nearest side that keeps it on the map (ADR-428). By the
    /// nearest side it left the map, the edge held it there, and no save
    /// after it would load.
    #[test]
    fn a_piece_evicted_at_the_map_edge_stays_on_the_map() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: V3::new(Fx::from_ratio(1, 2), Fx::from_int(13), Fx::from_int(48)),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: false,
        });
        b.next_id = 1;
        // Its box reaches past the map edge at x = 0; the piece lies a
        // unit and a half from the -x side and further from every other.
        let hull = Hull {
            pos: [Fx::from_int(2), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(2), Fx::from_int(6)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::ZERO,
            solid: None,
        };
        b.step_fragments(
            &w,
            &mut e,
            &t.blast,
            crate::gravity_per_tick(&t),
            0,
            Surroundings {
                hulls: &[hull],
                yielding: &[],
            },
        );
        let x = b.fragments[0].position.x;
        assert!(x > Fx::from_int(4), "the piece left by the far side: {x:?}");
        let mut bytes = Blob::new();
        b.save(&mut bytes);
        assert!(Bursts::load(&mut Scan::new(&bytes.v), Some(&w), &t.blast).is_some());
    }
    fn economy() -> Economy {
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/materials.ron")).unwrap();
        Economy::new(modding::Kinds::new(), mats, 2, &tuning())
    }
    fn world() -> voxel::World {
        voxel::World::new(96, 64, 96, 4096, Box::new(Ground))
    }
    #[test]
    fn many_flying_pieces_match_with_one_and_four_workers() {
        let w = world();
        let t = tuning();
        let run = |workers| {
            let (mut pieces, mut economy) = (Bursts::default(), economy());
            for i in 0..1200 {
                let held = economy.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                pieces.fragments.push(Fragment {
                    id: i + 1,
                    position: V3::new(
                        Fx::from_int(12 + (i % 60) as i32),
                        Fx::from_int(36),
                        Fx::from_int(12 + (i / 60) as i32),
                    ),
                    velocity: V3::new(
                        Fx::from_ratio((i % 5) as i32 - 2, 4),
                        Fx::ZERO,
                        Fx::from_ratio((i % 7) as i32 - 3, 4),
                    ),
                    material: 1,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 1,
                    count: 1,
                    shape: 1,
                    side: 1,
                    chunk: None,
                    held,
                    resting: false,
                });
            }
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            pool.install(|| {
                for tick in 0..8 {
                    pieces.step_fragments(
                        &w,
                        &mut economy,
                        &t.blast,
                        crate::gravity_per_tick(&t),
                        tick,
                        Surroundings::NONE,
                    );
                }
            });
            let mut bytes = Blob::new();
            pieces.save(&mut bytes);
            bytes.v
        };
        assert_eq!(run(1), run(4));
    }
    #[test]
    fn simultaneous_support_losses_wake_in_piece_order_across_workers() {
        let w = world();
        let mut t = tuning();
        t.blast.max_flying = 7;
        t.blast.rest_check_ticks = 1;
        t.blast.push_pct = 0;
        let run = |workers| {
            let (mut pieces, mut economy) = (Bursts::default(), economy());
            for i in 0..1200 {
                let held = economy.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                pieces.fragments.push(Fragment {
                    id: i + 1,
                    position: V3::new(
                        Fx::from_int(8 + (i % 40) as i32 * 2),
                        Fx::from_int(36),
                        Fx::from_int(8 + (i / 40) as i32 * 2),
                    ),
                    velocity: V3::ZERO,
                    material: 1,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 1,
                    count: 1,
                    shape: 1,
                    side: 1,
                    chunk: None,
                    held,
                    resting: true,
                });
            }
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            pool.install(|| {
                pieces.step_fragments(
                    &w,
                    &mut economy,
                    &t.blast,
                    crate::gravity_per_tick(&t),
                    0,
                    Surroundings::NONE,
                );
            });
            let flying: Vec<_> = pieces
                .fragments
                .iter()
                .filter(|f| !f.resting)
                .map(|f| f.id)
                .collect();
            assert_eq!(flying, (1..=7).collect::<Vec<_>>());
            assert!(economy.balanced());
            let mut bytes = Blob::new();
            pieces.save(&mut bytes);
            bytes.v
        };
        assert_eq!(run(1), run(4));
    }
    #[test]
    fn parallel_pile_grouping_matches_ordered_merging_and_ledger() {
        const GROUPS: usize = 256;
        fn reference(pieces: &mut Bursts, world: &voxel::World, economy: &mut Economy, t: &Blast) {
            let mut piles: HashMap<([i32; 3], Mat, Option<[u8; 3]>, u16), usize> = HashMap::new();
            let mut merged = BTreeSet::new();
            for i in 0..pieces.fragments.len() {
                let f = &pieces.fragments[i];
                let key = (
                    arr(f.position).map(|v| v.to_int_trunc().div_euclid(t.pile_cell as i32)),
                    f.material,
                    f.paint,
                    f.mm,
                );
                if let Some(&j) = piles.get(&key) {
                    let (a, b) = pieces.fragments.split_at_mut(i);
                    let to = &mut a[j];
                    let from = &b[0];
                    economy.ledger.transfer(from.held, to.held, u64::MAX);
                    to.mass_g += from.mass_g;
                    to.count += from.count;
                    let bottom = to.position.y.add(to.bounds().0[1]);
                    restack(to);
                    match world_floor(to, world) {
                        Some(floor) if bottom.sub(floor) <= Fx::from_ratio(1, 2) => seat(to, floor),
                        _ => to.position.y = bottom.sub(to.bounds().0[1]),
                    }
                    merged.insert(from.held);
                } else {
                    piles.insert(key, i);
                }
            }
            pieces.fragments.retain(|f| !merged.contains(&f.held));
            economy.wrecks.retain(|w| !merged.contains(&w.held));
            for held in merged {
                assert!(economy.ledger.retire_empty_debris(held));
            }
        }
        let w = world();
        let mut t = tuning();
        t.blast.max_flying = 0;
        t.blast.push_pct = 0;
        let build = || {
            let (mut pieces, mut economy) = (Bursts::default(), economy());
            // Interleave donors so grouping cannot use adjacency; different
            // materials, paint and scale at one position must stay separate.
            for donor in 0..4 {
                for group in 0..GROUPS {
                    let count = [1, 2, 7, 70][donor];
                    let material = 1 + (group % 2) as u16;
                    let held = economy.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                    let amount = economy.per_voxel(material) * u64::from(count);
                    economy.ledger.reveal(economy.ground, amount);
                    economy.ledger.transfer(economy.ground, held, amount);
                    let position = V3::new(
                        Fx::from_int(4 + (group / 8 % 8) as i32 * 8),
                        Fx::from_ratio(if group < 64 { 25 } else { 65 }, 2),
                        Fx::from_int(4 + (group / 64) as i32 * 8),
                    );
                    pieces.fragments.push(Fragment {
                        id: (donor * GROUPS + group + 1) as u64,
                        position,
                        velocity: V3::ZERO,
                        material,
                        paint: Some([0, (group % 4) as u8, 0]),
                        mm: if group % 8 < 4 { 50 } else { 100 },
                        basis: IDENTITY,
                        mass_g: 1200 * u64::from(count),
                        count,
                        shape: 1,
                        side: 1,
                        chunk: None,
                        held,
                        resting: true,
                    });
                    economy.wrecks.push(Wreck {
                        held,
                        x: 0,
                        y: 0,
                        z: 0,
                    });
                }
            }
            pieces.next_id = (4 * GROUPS + 1) as u64;
            (pieces, economy)
        };
        let (mut expected, mut expected_economy) = build();
        reference(&mut expected, &w, &mut expected_economy, &t.blast);
        assert_eq!(expected.fragments.len(), GROUPS);
        let mut expected_bytes = Blob::new();
        expected.save(&mut expected_bytes);
        for workers in [1, 2, 4, 8] {
            let (mut pieces, mut economy) = build();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            pool.install(|| {
                pieces.step_fragments(
                    &w,
                    &mut economy,
                    &t.blast,
                    crate::gravity_per_tick(&t),
                    0,
                    Surroundings::NONE,
                )
            });
            let mut bytes = Blob::new();
            pieces.save(&mut bytes);
            assert_eq!(bytes.v, expected_bytes.v, "workers={workers}");
            assert_eq!(economy.hash(), expected_economy.hash());
            assert_eq!(economy.wrecks.len(), expected_economy.wrecks.len());
            assert!(economy
                .wrecks
                .iter()
                .zip(&expected_economy.wrecks)
                .all(|(a, b)| (a.held, a.x, a.y, a.z) == (b.held, b.x, b.y, b.z)));
            assert!(economy.balanced());
        }
    }
    #[test]
    fn settled_candidates_keep_cell_lists_in_fragment_order_across_workers() {
        let t = tuning();
        let fragments: Vec<_> = (0..1400)
            .map(|i| Fragment {
                id: i + 1,
                position: V3::new(
                    Fx::from_int(12 + (i % 40) as i32),
                    Fx::from_int(18),
                    Fx::from_int(12 + ((i % 1200) / 40) as i32),
                ),
                velocity: V3::ZERO,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1,
                count: 1,
                shape: 1,
                side: 1,
                chunk: None,
                held: i as u32,
                resting: i < 1200,
            })
            .collect();
        let run = |workers| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            pool.install(|| {
                let mut index = Settled::default();
                let mut queries = SettledQueries::default();
                Settled::of_into(&fragments, &[], 0, &t.blast, &mut index, &mut queries);
                (index.boxes, index.cells, queries.0)
            })
        };
        assert_eq!(run(1), run(4));
    }
    fn tick(b: &mut Bursts, w: &mut voxel::World, e: &mut Economy, t: &Blast) {
        b.advance(w, &e.mats, t);
        w.begin_tick();
        w.flush();
        b.confirm(w.completed(), e, t);
        assert!(e.balanced());
    }
    #[test]
    fn fracture_is_spatial_bounded_and_does_not_create_matter_or_kinetic_energy() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning().blast;
        assert!(b.enqueue(&w, [48, 13, 48], 16, 300_000, &t));
        for _ in 0..200 {
            tick(&mut b, &mut w, &mut e, &t);
            if b.pending() == 0 && b.claims() == 0 {
                break;
            }
        }
        assert_eq!(b.pending(), 0);
        assert_eq!(b.claims(), 0);
        assert!(b.removed > 0);
        assert!(b.spent_mj <= 300_000_000);
        assert!(b.motion_mj <= b.spent_mj * t.motion_pct as u64 / 100);
        let total: u64 = b.fragments.iter().map(|f| e.ledger.held(f.held)).sum();
        assert_eq!(total, b.removed * e.per_voxel(1));
        for f in &b.fragments {
            let p = arr(f.position).map(|v| v.to_int_trunc() as u32);
            assert!(!w.solid(p[0], p[1], p[2]));
            assert!(f.velocity.y > Fx::ZERO);
        }
        assert!(w.solid(48, 1, 48));
    }
    #[test]
    fn confirmed_noop_releases_claim_without_duplicating_a_removed_voxel() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning().blast;
        let p = [48, 11, 48];
        // Another command removes the same cell first, ahead of the blast claim.
        w.queue_dig(p[0], p[1], p[2]);
        b.claims.insert(
            p,
            Claim {
                at: [48, 13, 48],
                motion_mj: 999,
                side: 2,
            },
        );
        w.queue_edit(Edit::dig_if(p[0], p[1], p[2], 1));
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        assert_eq!(b.claims(), 0);
        assert!(b.fragments.is_empty());
        assert_eq!(b.removed, 0);
        assert!(e.balanced());
    }
    #[test]
    fn delayed_fracture_cannot_erase_a_new_stronger_material_for_the_old_cost() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning().blast;
        w.queue_edit(Edit::put(48, 11, 48, 3));
        b.claims.insert(
            [48, 11, 48],
            Claim {
                at: [48, 13, 48],
                motion_mj: 999,
                side: 2,
            },
        );
        w.queue_edit(Edit::dig_if(48, 11, 48, 1));
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        assert_eq!(w.material(48, 11, 48), 3);
        assert_eq!(b.claims(), 0);
        assert!(b.fragments.is_empty());
        assert!(e.balanced());
    }
    #[test]
    fn different_work_slices_produce_identical_fracture_and_matter() {
        let run = |samples, edits| {
            let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
            let mut t = tuning().blast;
            t.samples_per_tick = samples;
            t.wave_pops_per_tick = edits;
            t.edits_per_tick = edits;
            b.enqueue(&w, [48, 12, 48], 4, 20_000, &t);
            for _ in 0..5000 {
                tick(&mut b, &mut w, &mut e, &t);
                if b.pending() == 0 && b.claims() == 0 {
                    break;
                }
            }
            assert_eq!(b.pending(), 0);
            assert!(b.removed > 0);
            let mut material = Vec::new();
            for z in 40..56 {
                for y in 4..20 {
                    for x in 40..56 {
                        material.push(w.material(x, y, z));
                    }
                }
            }
            (
                material,
                b.removed,
                b.spent_mj,
                b.fragments.iter().map(|f| f.mass_g).sum::<u64>(),
            )
        };
        assert_eq!(run(7, 1), run(10000, 10000));
    }
    #[test]
    fn fast_cube_cannot_tunnel_through_a_wall_one_world_voxel_thick() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        for y in 12..40 {
            for z in 20..76 {
                w.queue_edit(Edit::put(52, y, z, 2));
            }
        }
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 17, 48]),
            velocity: V3::new(Fx::from_int(10), Fx::ZERO, Fx::ZERO),
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 2,
            chunk: None,
            held,
            resting: false,
        });
        for _ in 0..100 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                0,
                Surroundings::NONE,
            );
            assert!(b.fragments[0].position.x < Fx::from_int(52));
        }
    }
    #[test]
    fn grouping_keeps_every_cell_once_and_never_glues_disconnected_corners() {
        for shape in 1u64..=255 {
            let pieces = shape_components(shape, 2);
            let mut seen = 0u64;
            for piece in pieces {
                assert_eq!(seen & piece, 0);
                seen |= piece;
                assert_eq!(shape_components(piece, 2), [piece]);
                let (lo, hi) = shape_bounds(piece, 2);
                for a in 0..3 {
                    assert!(lo[a] < hi[a]);
                }
            }
            assert_eq!(seen, shape);
        }
        assert_eq!(shape_components(0b10000001, 2), [1, 128]);
    }
    /// A three-cell box groups the same way: face neighbours only, every
    /// cell once, and a row across the box is one piece (ADR-284).
    #[test]
    fn a_wider_box_groups_by_faces_and_keeps_every_cell_once() {
        // A full row along x at y = z = 0, and the far corner alone.
        let row = 0b111u64;
        let corner = 1u64 << 26;
        assert_eq!(shape_components(row | corner, 3), [row, corner]);
        // Cells 2 and 3 are x = 2, y = 0 and x = 0, y = 1: not neighbours.
        assert_eq!(shape_components(0b1100, 3), [0b100, 0b1000]);
        // A column along z: strides of nine.
        let column = 1 | (1 << 9) | (1 << 18);
        assert_eq!(shape_components(column, 3), [column]);
        let c = arr(shape_centre(column, 3));
        assert_eq!(c, [Fx::ZERO, Fx::ZERO, Fx::ONE]);
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..200 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let shape = seed & ((1u64 << 27) - 1);
            if shape == 0 {
                continue;
            }
            let mut seen = 0u64;
            for piece in shape_components(shape, 3) {
                assert_eq!(seen & piece, 0);
                seen |= piece;
                assert_eq!(shape_components(piece, 3), [piece]);
            }
            assert_eq!(seen, shape);
        }
        // Four cells a side uses every bit of the mask.
        let all = u64::MAX;
        assert_eq!(shape_components(all, 4), [all]);
        assert!(shape_bounds(all, 4).1[0] > Fx::from_int(1));
    }
    #[test]
    fn a_sealed_strong_wall_protects_soil_behind_it() {
        let mut w = world();
        let mut e = economy();
        let mut t = tuning().blast;
        e.mats
            .insert(modding::MatDef {
                id: 65,
                density: 780,
                strength: 100000,
                bend_span: 0,
                harmful: false,
                fuel: 0,
                kindling: 0,
                melt: 0,
                loose: false,
            })
            .unwrap();
        for z in 28..=68 {
            for y in 0..36 {
                w.queue_edit(Edit::put(52, y, z, 65));
            }
        }
        w.flush();
        let mut b = Bursts::default();
        t.samples_per_tick = 1_000_000;
        t.wave_pops_per_tick = 100_000;
        t.edits_per_tick = 100_000;
        b.enqueue(&w, [48, 13, 48], 16, 3_000_000, &t);
        for _ in 0..100 {
            tick(&mut b, &mut w, &mut e, &t);
        }
        assert!(b.removed > 0);
        assert_eq!(w.material(52, 11, 48), 65);
        assert_eq!(w.material(55, 11, 48), 1);
        assert_eq!(w.material(48, 11, 48), AIR);
    }
    fn targets() -> (
        crate::damage::Bodies,
        Vec<Option<[crate::damage::Frame; 2]>>,
    ) {
        let mut body = voxel::UnitBody::new_scaled(3, 3, 3, 1, 100);
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..3 {
                    body.set(x, y, z, voxel::Part::Armour);
                }
            }
        }
        body.set(1, 1, 1, voxel::Part::Core);
        assert!(body.seat_core());
        let frame =
            |p: [i32; 3]| crate::damage::Frame::new(&body, v3(p.map(Fx::from_int)), 0, V3::UP);
        let frames = vec![
            Some([frame([45, 12, 53]); 2]),
            Some([frame([56, 12, 48]); 2]),
        ];
        let mut bodies = crate::damage::Bodies::new(vec![body]);
        bodies.spawn_at(0, 0);
        bodies.spawn_at(1, 0);
        (bodies, frames)
    }
    #[test]
    fn the_same_wave_damages_an_exposed_body_but_is_stopped_by_its_cover() {
        let run = |wall: bool| {
            let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
            let (mut bodies, frames) = targets();
            let mut t = tuning();
            t.impact.work_kj_m3 = [20_000; 5];
            e.mats
                .insert(modding::MatDef {
                    id: 65,
                    density: 780,
                    strength: 100000,
                    bend_span: 0,
                    harmful: false,
                    fuel: 0,
                    kindling: 0,
                    melt: 0,
                    loose: false,
                })
                .unwrap();
            if wall {
                for z in 28..=68 {
                    for y in 0..40 {
                        w.queue_edit(Edit::put(52, y, z, 65));
                    }
                }
                w.flush();
            }
            assert!(b.enqueue(&w, [48, 15, 48], 16, 3_000_000, &t.blast));
            b.capture_last(&bodies, &frames, &t.blast);
            for _ in 0..2000 {
                b.advance_material(&mut w, &e.mats, Some(&mut bodies), &t);
                w.begin_tick();
                w.flush();
                b.confirm(w.completed(), &mut e, &t.blast);
                bodies.step(
                    &t.impact.work_kj_m3,
                    t.impact.cells_per_tick,
                    t.impact.jobs_per_tick,
                );
                if b.pending() == 0 && b.claims() == 0 && bodies.pending.is_empty() {
                    break;
                }
            }
            assert_eq!(b.pending(), 0);
            assert!(b.spent_mj <= 3_000_000_000);
            assert!(b.removed > 0);
            assert!(e.balanced());
            [
                bodies.state(0).unwrap().removed.len(),
                bodies.state(1).unwrap().removed.len(),
            ]
        };
        let covered = run(true);
        let open = run(false);
        assert!(covered[0] > 0, "exposed unit was untouched: {covered:?}");
        assert_eq!(covered[1], 0, "wave crossed sealed cover");
        assert!(
            open[0] > 0 && open[1] > 0,
            "without cover both bodies must be reached: {open:?}"
        );
    }
    /// A blast refused for want of room has no job; a late capture must
    /// leave the last job alone, whose wave may already hold its
    /// resistance buffer (ADR-287).
    #[test]
    fn a_refused_blast_does_not_recapture_the_last_job() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let (mut bodies, frames) = targets();
        let mut t = tuning();
        t.impact.work_kj_m3 = [20_000; 5];
        t.blast.max_pending = 2;
        // Both jobs sample within the first tick; their waves then take
        // many ticks, so the queue stays full.
        t.blast.samples_per_tick = 200_000;
        t.blast.wave_pops_per_tick = 500;
        assert!(b.enqueue(&w, [48, 15, 48], 16, 3_000_000_000, &t.blast));
        b.capture_last(&bodies, &frames, &t.blast);
        assert!(b.enqueue(&w, [40, 15, 48], 16, 3_000_000_000, &t.blast));
        let tick = |b: &mut Bursts,
                    w: &mut voxel::World,
                    bodies: &mut crate::damage::Bodies,
                    e: &mut Economy| {
            b.advance_material(w, &e.mats, Some(bodies), &t);
            w.begin_tick();
            w.flush();
            b.confirm(w.completed(), e, &t.blast);
            bodies.step(
                &t.impact.work_kj_m3,
                t.impact.cells_per_tick,
                t.impact.jobs_per_tick,
            );
        };
        tick(&mut b, &mut w, &mut bodies, &mut e);
        // Both jobs are still at work, the last with its wave holding the
        // resistance buffer; the queue is full.
        assert_eq!(b.pending(), 2);
        // Refused for want of room; the old caller captured anyway.
        assert!(!b.enqueue(&w, [56, 15, 48], 16, 3_000_000_000, &t.blast));
        b.capture_last(&bodies, &frames, &t.blast);
        for _ in 0..2000 {
            tick(&mut b, &mut w, &mut bodies, &mut e);
            if b.pending() == 0 && b.claims() == 0 && bodies.pending.is_empty() {
                break;
            }
        }
        assert_eq!(b.pending(), 0);
        assert!(e.balanced());
    }
    #[test]
    fn unit_wave_build_and_damage_cursors_resume_exactly() {
        let (mut b, mut w, e) = (Bursts::default(), world(), economy());
        let (mut bodies, frames) = targets();
        let mut t = tuning();
        t.blast.samples_per_tick = 4000;
        t.blast.edits_per_tick = 31;
        t.impact.work_kj_m3 = [20_000; 5];
        b.enqueue(&w, [48, 15, 48], 16, 3_000_000, &t.blast);
        b.capture_last(&bodies, &frames, &t.blast);
        for cut in [1, 15, 80, 130] {
            for _ in 0..cut {
                b.advance_material(&mut w, &e.mats, Some(&mut bodies), &t);
            }
            let mut bytes = Blob::new();
            b.save(&mut bytes);
            let mut back = Bursts::load_material(
                &mut Scan::new(&bytes.v),
                Some(&w),
                &t.blast,
                Some(&bodies),
                2,
                false,
            )
            .unwrap();
            assert_eq!(b.hash(), back.hash());
            let mut saved = Blob::new();
            bodies.save(&mut saved);
            let (mut other, _) = targets();
            assert!(other.load(&mut Scan::new(&saved.v), 2, t.impact.max_pending));
            for _ in 0..7 {
                b.advance_material(&mut w, &e.mats, Some(&mut bodies), &t);
                back.advance_material(&mut w, &e.mats, Some(&mut other), &t);
                assert_eq!(b.hash(), back.hash());
                assert_eq!(bodies.hash(), other.hash());
            }
        }
    }

    #[test]
    fn partial_build_wave_and_claims_resume_after_serialization() {
        for cut in [1, 5, 40, 80] {
            let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
            let mut t = tuning().blast;
            t.samples_per_tick = 3000;
            t.wave_pops_per_tick = 100;
            t.edits_per_tick = 13;
            b.enqueue(&w, [48, 13, 48], 8, 60_000, &t);
            for _ in 0..cut {
                tick(&mut b, &mut w, &mut e, &t);
            }
            let mut bytes = Blob::new();
            b.save(&mut bytes);
            let mut back = Bursts::load(&mut Scan::new(&bytes.v), Some(&w), &t).unwrap();
            assert_eq!(b.hash(), back.hash());
            for end in [0, 31, bytes.v.len() - 1] {
                assert!(Bursts::load(&mut Scan::new(&bytes.v[..end]), Some(&w), &t).is_none());
            }
            // Advance the independent pure field builders against identical terrain.
            // Claims/fragments remain pending here so neither side edits shared inputs.
            let mut wb = world();
            let mut saved = Blob::new();
            w.save(&mut saved);
            assert!(wb.load(&mut Scan::new(&saved.v)));
            for _ in 0..10 {
                b.advance(&mut w, &e.mats, &t);
                back.advance(&mut wb, &e.mats, &t);
                assert_eq!(b.hash(), back.hash());
            }
        }
    }
    #[test]
    fn all_flying_voxels_land_merge_locally_and_remain_reclaimable() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        b.enqueue(&w, [48, 13, 48], 8, 60_000, &t.blast);
        for _ in 0..100 {
            tick(&mut b, &mut w, &mut e, &t.blast);
        }
        let n = b.removed;
        for _ in 0..800 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                0,
                Surroundings::NONE,
            );
        }
        assert!(n > 0);
        assert!(b.fragments.iter().all(|f| f.resting));
        assert_eq!(b.fragments.iter().map(|f| f.count as u64).sum::<u64>(), n);
        assert_eq!(e.wrecks.len(), b.fragments.len());
        assert!(e.balanced());
        let tank = e.ledger.create(matter::Kind::Hauler, 0, u64::MAX);
        for f in &b.fragments {
            e.ledger.transfer(f.held, tank, u64::MAX);
        }
        b.sweep(&e);
        assert!(b.fragments.is_empty());
        assert_eq!(e.ledger.held(tank), n * e.per_voxel(1));
        assert!(e.balanced());
    }
}

#[derive(Clone, Copy)]
struct Claim {
    at: [i32; 3],
    motion_mj: u64,
    /// Cells per side of the box this cell's piece is grouped in (ADR-284).
    side: u8,
}

/// A live hull a piece can land on (ADR-293): its footprint box about
/// `pos` turned by `head`, standing `height` above `pos.y`; `vel` is how
/// far it moved this tick, and a moving hull bulldozes the pieces it
/// drives into (ADR-294).
#[derive(Clone, Copy, Debug)]
pub struct Hull<'a> {
    pub pos: [Fx; 3],
    pub head: i32,
    pub half: [Fx; 2],
    pub height: Fx,
    pub vel: [Fx; 3],
    /// The rise the hull climbs rather than pushes: a piece no taller
    /// than this above the hull's bottom is floor to it (ADR-292).
    pub step: Fx,
    /// The hull's own voxels, as it stands now — turned turret, lost
    /// cells and segments with it (ADR-469). A piece meets the hull only
    /// where it meets them; without them the box is the hull.
    pub solid: Option<Solid<'a>>,
}
/// A hull's voxels as world cells (ADR-469).
pub trait Voxels: Sync {
    /// Whether the world cell `p` lies in the hull's voxels.
    fn solid(&self, p: [i32; 3]) -> bool;
}
impl<F: Fn([i32; 3]) -> bool + Sync> Voxels for F {
    fn solid(&self, p: [i32; 3]) -> bool {
        self(p)
    }
}
/// A hull's voxels for the pieces (ADR-469).
#[derive(Clone, Copy)]
pub struct Solid<'a>(pub &'a dyn Voxels);
impl std::fmt::Debug for Solid<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Solid")
    }
}
/// The most cells a piece's box asks of a hull's voxels, wherever it
/// lies; a larger piece — a whole section — meets a hull by its box
/// (ADR-469).
const SOLID_SAMPLES: i64 = 64;
/// The most cells of the box a piece sweeps in a tick asked of a hull's
/// voxels at once; past it, each substep asks its own (ADR-469).
const SWEPT_SAMPLES: i64 = 512;
/// Whether a piece's box `low..high` meets hulls by their voxels
/// (ADR-469): nowhere does it cover more than `SOLID_SAMPLES` cells —
/// `floor(extent) + 2` along each axis at most.
fn by_voxels(low: [Fx; 3], high: [Fx; 3]) -> bool {
    (0..3).fold(1i64, |n, a| {
        n * (high[a].sub(low[a]).to_int_trunc() as i64 + 2)
    }) <= SOLID_SAMPLES
}
impl Hull<'_> {
    /// Whether the world box `lo..hi` shares a cell with the hull's voxels
    /// (ADR-469): the cells it covers, floor of each corner, as the
    /// world's are asked. Without the voxels, or past `cap` cells, it does.
    fn meets(&self, lo: [Fx; 3], hi: [Fx; 3], cap: i64) -> bool {
        let Some(Solid(voxels)) = self.solid else {
            return true;
        };
        let lo = lo.map(|v| (v.raw() >> 32) as i32);
        let hi = hi.map(|v| (v.raw() >> 32) as i32);
        let cells = (0..3).fold(1i64, |n, a| n * i64::from(hi[a] - lo[a] + 1));
        if cells > cap {
            return true;
        }
        (lo[1]..=hi[1])
            .any(|y| (lo[2]..=hi[2]).any(|z| (lo[0]..=hi[0]).any(|x| voxels.solid([x, y, z]))))
    }
    /// Whether a piece's box at `p`, lying within the hull's box, meets
    /// the hull (ADR-469): its voxels, or its box for a piece too large
    /// to ask them.
    fn meets_piece(&self, p: [Fx; 3], low: [Fx; 3], high: [Fx; 3]) -> bool {
        !by_voxels(low, high)
            || self.meets(
                std::array::from_fn(|a| p[a].add(low[a])),
                std::array::from_fn(|a| p[a].add(high[a])),
                i64::MAX,
            )
    }
    /// Whether a point of the ground plane lies under the hull's box.
    pub fn covers_point(&self, p: [Fx; 2]) -> bool {
        let (dx, dz) = (p[0].sub(self.pos[0]), p[1].sub(self.pos[2]));
        // The turned box lies within `half[0] + half[1]` of its centre
        // along both world axes: most points are farther.
        let round = self.half[0].add(self.half[1]);
        if dx.abs() > round || dz.abs() > round {
            return false;
        }
        let (c, s) = (fx::angle::cos(self.head), fx::angle::sin(self.head));
        let u = dx.mul(c).add(dz.mul(s));
        let v = dz.mul(c).sub(dx.mul(s));
        u.abs() <= self.half[0] && v.abs() <= self.half[1]
    }
    /// The world box around the hull's footprint, a unit wider than the
    /// turned rectangle for the rounding of its frame (ADR-465): no point
    /// outside it lies under the hull.
    pub fn footprint_box(&self) -> ([Fx; 2], [Fx; 2]) {
        let (c, s) = (
            fx::angle::cos(self.head).abs(),
            fx::angle::sin(self.head).abs(),
        );
        let [a, b] = self.half;
        let extent = [a.mul(c).add(b.mul(s)), a.mul(s).add(b.mul(c))].map(|e| e.add(Fx::ONE));
        (
            [self.pos[0].sub(extent[0]), self.pos[2].sub(extent[1])],
            [self.pos[0].add(extent[0]), self.pos[2].add(extent[1])],
        )
    }
    /// The top of the hull's lowest voxel in the world column under
    /// `centre` (ADR-469); the box's top without the voxels. `None` where
    /// the centre is not over the box or the column holds no voxel.
    fn lowest(&self, centre: V3) -> Option<Fx> {
        if !self.covers_point([centre.x, centre.z]) {
            return None;
        }
        let top = self.pos[1].add(self.height);
        let Some(Solid(voxels)) = self.solid else {
            return Some(top);
        };
        let [x, z] = [centre.x, centre.z].map(|v| (v.raw() >> 32) as i32);
        let base = (self.pos[1].raw() >> 32) as i32;
        (base..-(top.neg().raw() >> 32) as i32)
            .find(|&y| voxels.solid([x, y, z]))
            .map(|y| Fx::from_int(y + 1))
    }
    /// Whether a piece whose box bottom is `bottom` and centre `centre`
    /// rides on the hull (ADR-293, ADR-458): its centre over the box and
    /// its bottom no lower than the support probe's half unit under the
    /// roof — on the roof, or on what lies on it. The roof is the hull's
    /// voxels in the column under the centre (ADR-469): a piece on the
    /// rear deck rides there, and one under the belly does not.
    pub fn carries(&self, bottom: Fx, centre: V3) -> bool {
        let reach = bottom.add(Fx::from_ratio(1, 2));
        self.lowest(centre).is_some_and(|roof| roof <= reach)
    }
    /// Whether a piece whose box bottom is `bottom` and centre `centre`
    /// lies under the hull (ADR-302, ADR-469): its centre over the box
    /// and the hull's voxels in that column all above it — under the
    /// belly, or dug from under it.
    fn shelters(&self, bottom: Fx, centre: V3) -> bool {
        let reach = bottom.add(Fx::from_ratio(1, 2));
        self.lowest(centre).is_some_and(|roof| roof > reach)
    }
    /// Whether the column's centre lies under the hull's box.
    pub fn covers(&self, x: i32, z: i32) -> bool {
        self.covers_point([
            Fx::from_int(x).add(Fx::from_ratio(1, 2)),
            Fx::from_int(z).add(Fx::from_ratio(1, 2)),
        ])
    }
}

/// What the pieces meet besides the world this tick: the live hulls
/// (ADR-293) and the materials a hull crushes rather than climbs
/// (ADR-294). The batch has neither.
#[derive(Clone, Copy)]
pub struct Surroundings<'a> {
    pub hulls: &'a [Hull<'a>],
    pub yielding: &'a [Mat],
}
impl Surroundings<'static> {
    pub const NONE: Self = Self {
        hulls: &[],
        yielding: &[],
    };
}

/// A real removed world voxel, or a settled local pile of them. Coordinates and
/// velocity are in world units and units/tick. Matter stays in `held` until haul.
pub struct Fragment {
    pub id: u64,
    pub position: V3,
    pub velocity: V3,
    pub material: Mat,
    /// Owner, finish code and subsystem role for an actual piece of a unit.
    pub paint: Option<[u8; 3]>,
    pub mm: u16,
    pub basis: [V3; 3],
    pub mass_g: u64,
    pub count: u32,
    /// Occupied cells of the piece's box, bit `x + side*y + side*side*z`
    /// (ADR-284).
    pub shape: u64,
    /// Cells per side of that box, `1..=4`.
    pub side: u8,
    pub chunk: Option<chunk::Chunk>,
    pub held: u32,
    pub resting: bool,
}

#[derive(Default)]
pub struct Bursts {
    jobs: VecDeque<Job>,
    claims: BTreeMap<[u32; 3], Claim>,
    /// Reused only within a tick; neither cache affects save or ordering.
    settled_scratch: Settled,
    query_scratch: SettledQueries,
    pub fragments: Vec<Fragment>,
    pub corpses: BTreeMap<u32, Fragment>,
    pub(crate) next_id: u64,
    pub removed: u64,
    pub spent_mj: u64,
    pub motion_mj: u64,
    pub body_motion_mj: u64,
    pub body_removed: u64,
}

pub const IDENTITY: [V3; 3] = [
    V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
    V3::UP,
    V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
];
impl Fragment {
    /// Keep the centre over the map (ADR-428). A save refuses a piece
    /// whose centre lies off it, and one would get there: a hull at the
    /// map edge hangs its cells over it. The box may still cross the edge;
    /// the edge holds the piece where it lands.
    pub(crate) fn keep_on_map(&mut self, columns: [u32; 2]) {
        let last = |n: u32| Fx::from_int(n as i32).sub(Fx(1));
        self.position.x = self.position.x.clamp(Fx::ZERO, last(columns[0]));
        self.position.z = self.position.z.clamp(Fx::ZERO, last(columns[1]));
        self.position.y = self.position.y.max(Fx::ZERO);
    }
    /// The piece's box in its own turn, around its centre.
    pub fn bounds(&self) -> ([Fx; 3], [Fx; 3]) {
        // A section's box is already in world axes about its centre, as its
        // captured frames placed its cells; its basis is the turn since it
        // left, and the box turns with it (ADR-468). On the identity basis
        // a section starts with, the sums below give the box unchanged.
        let (lo, hi) = match &self.chunk {
            Some(chunk) => chunk.bounds,
            None => {
                let (lo, hi) = shape_bounds(self.shape, self.side);
                let scale = Fx::from_ratio(self.mm as i32, voxel::unit::WORLD_MM as i32);
                (lo.map(|v| v.mul(scale)), hi.map(|v| v.mul(scale)))
            }
        };
        let mut low = [Fx::ZERO; 3];
        let mut high = [Fx::ZERO; 3];
        // Each corner sum chooses one endpoint independently per local
        // axis. Reduce the already rounded endpoint products, then add in
        // the original axis order: no centre/extent formula with different
        // Q32 rounding, and no eight-corner transform.
        for axis in 0..3 {
            let a = arr(self.basis[axis].scale(lo[axis]));
            let b = arr(self.basis[axis].scale(hi[axis]));
            for component in 0..3 {
                low[component] = low[component].add(a[component].min(b[component]));
                high[component] = high[component].add(a[component].max(b[component]));
            }
        }
        (low, high)
    }
}
fn mix(h: &mut u64, v: u64) {
    *h = h.wrapping_mul(0x100000001b3).wrapping_add(v);
}
fn ijk(i: usize, n: usize) -> [i32; 3] {
    [(i % n) as i32, (i / n % n) as i32, (i / (n * n)) as i32]
}
fn arr(v: V3) -> [Fx; 3] {
    [v.x, v.y, v.z]
}
fn v3(a: [Fx; 3]) -> V3 {
    V3::new(a[0], a[1], a[2])
}
/// A coarse box chooses either its full extent or half-sized sub-boxes.
/// All cells agree on that partition; no overlapping or disconnected solids
/// are invented. Seeded locally, so replay needs no new RNG state.
pub(crate) fn fracture_box(p: [u32; 3], max_side: u8, t: &Blast) -> ([u32; 3], u8) {
    let max_side = u32::from(max_side.clamp(1, 4));
    let tile = p.map(|v| v / max_side);
    let mut seed = 0;
    for v in tile {
        mix(&mut seed, u64::from(v));
    }
    let mut rng = fx::rng::Rng::new(seed, 475);
    let side = if rng.below(100) < t.small_piece_pct {
        (max_side / 2).max(1)
    } else {
        max_side
    };
    let base = tile.map(|v| v * max_side);
    (
        std::array::from_fn(|a| base[a] + (p[a] - base[a]) / side * side),
        side as u8,
    )
}

/// Disorder redirects the finite impulse and only reduces its speed.
fn scattered_launch(id: u64, direction: V3, speed: Fx, t: &Blast) -> V3 {
    let span = direction
        .x
        .abs()
        .max(direction.y.abs())
        .max(direction.z.abs());
    if span == Fx::ZERO || speed == Fx::ZERO {
        return V3::ZERO;
    }
    let mut rng = fx::rng::Rng::new(id, 476);
    let spread = t.scatter_pct as i32;
    let jitter = v3(std::array::from_fn(|_| {
        span.mul(Fx::from_ratio(rng.range(-spread, spread), 100))
    }));
    let mut direction = direction.add(jitter);
    if direction.len_sq() == Fx::ZERO {
        direction = V3::UP;
    }
    let speed = speed.mul(Fx::from_ratio(rng.range(t.speed_min_pct as i32, 100), 100));
    direction.scale(speed.div(direction.len_sq().sqrt()))
}

/// Cells of a piece's box.
fn box_cells(side: u8) -> u32 {
    u32::from(side.clamp(1, 4)).pow(3)
}
/// The cell a bit of a piece's mask names, in box coordinates.
fn shape_cell(i: u32, side: u8) -> [i32; 3] {
    let s = i32::from(side.clamp(1, 4));
    let i = i as i32;
    [i % s, (i / s) % s, i / (s * s)]
}
/// Bit masks of the four possible coordinate planes of each shape axis.
/// Generated at compile time for all supported box sizes, including 3³.
const SHAPE_PLANES: [[[u64; 4]; 3]; 4] = {
    let mut sizes = [[[0; 4]; 3]; 4];
    let mut s = 1;
    while s <= 4 {
        let mut i = 0;
        while i < s * s * s {
            sizes[s - 1][0][i % s] |= 1 << i;
            sizes[s - 1][1][i / s % s] |= 1 << i;
            sizes[s - 1][2][i / (s * s)] |= 1 << i;
            i += 1;
        }
        s += 1;
    }
    sizes
};
fn shape_sums(shape: u64, planes: &[[u64; 4]; 3]) -> [i32; 3] {
    planes.map(|p| {
        (shape & p[1]).count_ones() as i32
            + 2 * (shape & p[2]).count_ones() as i32
            + 3 * (shape & p[3]).count_ones() as i32
    })
}
/// Centre of mass of a piece in its box, cell units.
pub fn shape_centre(shape: u64, side: u8) -> V3 {
    let planes = &SHAPE_PLANES[usize::from(side.clamp(1, 4)) - 1];
    let sum = shape_sums(shape, planes);
    v3(sum.map(|v| Fx::from_ratio(v, shape.count_ones().max(1) as i32)))
}
/// Face-connected components of a mask, each a piece of its own.
pub(crate) fn shape_components(mut left: u64, side: u8) -> Vec<u64> {
    let s = u32::from(side.clamp(1, 4));
    let strides = [1, s, s * s];
    let n = box_cells(side);
    let mut out = Vec::new();
    while left != 0 {
        let mut component = 1u64 << left.trailing_zeros();
        loop {
            let before = component;
            for i in 0..n {
                if component & (1 << i) != 0 {
                    let p = shape_cell(i, side);
                    for a in 0..3 {
                        if p[a] + 1 < s as i32 {
                            component |= left & (1 << (i + strides[a]));
                        }
                        if p[a] > 0 {
                            component |= left & (1 << (i - strides[a]));
                        }
                    }
                }
            }
            if before == component {
                break;
            }
        }
        left &= !component;
        out.push(component);
    }
    out
}
fn shape_bounds(shape: u64, side: u8) -> ([Fx; 3], [Fx; 3]) {
    let side = side.clamp(1, 4);
    let planes = &SHAPE_PLANES[usize::from(side) - 1];
    let sum = shape_sums(shape, planes);
    let lo = planes.map(|p| {
        p.iter()
            .position(|mask| shape & mask != 0)
            .unwrap_or(usize::from(side) - 1) as i32
    });
    let hi = planes.map(|p| p.iter().rposition(|mask| shape & mask != 0).unwrap_or(0) as i32);
    let centre = sum.map(|v| Fx::from_ratio(v, shape.count_ones().max(1) as i32));
    (
        std::array::from_fn(|a| {
            Fx::from_int(lo[a])
                .sub(centre[a])
                .sub(Fx::from_ratio(49, 100))
        }),
        std::array::from_fn(|a| {
            Fx::from_int(hi[a])
                .sub(centre[a])
                .add(Fx::from_ratio(49, 100))
        }),
    )
}
fn centre(p: [u32; 3]) -> V3 {
    v3(p.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))))
}
fn save_v(b: &mut Blob, v: V3) {
    for x in arr(v) {
        b.i64(x.raw());
    }
}
fn load_v(s: &mut Scan) -> V3 {
    v3(std::array::from_fn(|_| Fx(s.i64())))
}

impl Job {
    fn sample(&self, i: usize) -> ([i32; 3], usize) {
        let c3 = (self.cell * self.cell * self.cell) as usize;
        let coarse = i / c3;
        let a = ijk(coarse, self.n);
        let b = ijk(i % c3, self.cell as usize);
        (
            std::array::from_fn(|k| self.origin[k] + a[k] * self.cell + b[k]),
            coarse,
        )
    }
    fn samples(&self) -> usize {
        self.n.pow(3) * (self.cell as usize).pow(3)
    }
}

impl Bursts {
    /// Reserve one conditional terrain edit for an external spatial envelope.
    /// The caller pays fracture work; confirmation remains the only source of
    /// terrain fragments and uses the same bounded claim pool as ordinary combat.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn claim_voxel(
        &mut self,
        world: &mut voxel::World,
        p: [u32; 3],
        mat: Mat,
        at: [i32; 3],
        motion_mj: u64,
        side: u8,
        t: &Blast,
    ) -> bool {
        // The edit queue drops digs on the world's frame (ADR-034); a claim
        // there would never be confirmed and never released, and every
        // wait on claims would wait forever (ADR-420). Callers check it
        // first; this is the last line.
        if self.fragment_room(t) == 0
            || self.claims.len() >= t.max_claims as usize
            || self.claims.contains_key(&p)
            || !world.inside(p[0], p[1], p[2])
            || world.on_frame(p[0], p[1], p[2])
        {
            return false;
        }
        self.claims.insert(
            p,
            Claim {
                at,
                motion_mj,
                side,
            },
        );
        world.queue_edit(Edit::dig_if(p[0], p[1], p[2], mat));
        true
    }
    pub fn pending(&self) -> usize {
        self.jobs.len()
    }
    pub fn claims(&self) -> usize {
        self.claims.len()
    }
    pub fn claimed(&self, p: [u32; 3]) -> bool {
        self.claims.contains_key(&p)
    }
    pub fn fragment_room(&self, t: &Blast) -> u32 {
        let moving = self.fragments.iter().filter(|f| !f.resting).count();
        (t.max_flying as usize)
            .saturating_sub(moving + self.claims.len())
            .min(
                (t.max_fragments as usize).saturating_sub(self.fragments.len() + self.claims.len()),
            ) as u32
    }
    pub(crate) fn emit_body_fragment(
        &mut self,
        mut f: Fragment,
        direction: V3,
        energy_mj: u64,
        t: &Blast,
    ) {
        let norm = direction.len_sq();
        if norm > Fx::ZERO && energy_mj > 0 {
            let denom = f.mass_g.max(1) as u128
                * (voxel::unit::WORLD_MM as u128).pow(2)
                * (crate::TICKS_PER_SECOND as u128).pow(2);
            let speed =
                Fx((((energy_mj as u128 * 2_000_000) << 32) / denom).min(i64::MAX as u128) as i64)
                    .sqrt()
                    .min(Fx::from_ratio(
                        t.max_speed_mm_s as i32,
                        voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
                    ));
            f.velocity = scattered_launch(self.next_id + 1, direction, speed, t);
        }
        // Truncate toward zero for this diagnostic: rounding a negative axis
        // away from zero could report more kinetic energy than was reserved.
        let mm_s = arr(f.velocity).map(|v| {
            (v.raw() as i128 * voxel::unit::WORLD_MM as i128 * crate::TICKS_PER_SECOND as i128
                / (1i128 << 32)) as i64
        });
        self.body_motion_mj = self
            .body_motion_mj
            .saturating_add(voxel::impact::kinetic_mj(
                f.mass_g.min(u32::MAX as u64) as u32,
                mm_s,
            ));
        self.next_id += 1;
        f.id = self.next_id;
        self.body_removed += f.count as u64;
        self.fragments.push(f);
    }
    /// A whole section inherits the parent's motion. That energy was already
    /// present in the vehicle and is not counted as new blast kinetic energy.
    pub(crate) fn emit_section(&mut self, mut f: Fragment) {
        assert!(f.chunk.is_some());
        self.next_id += 1;
        f.id = self.next_id;
        self.body_removed += u64::from(f.count);
        self.fragments.push(f);
    }
    pub fn ready(&self, t: &Blast) -> bool {
        self.jobs.len() < t.max_pending as usize
    }
    pub fn enqueue(
        &mut self,
        world: &voxel::World,
        at: [i32; 3],
        radius: i32,
        energy_j: u32,
        t: &Blast,
    ) -> bool {
        if !self.ready(t) || energy_j == 0 || !(1..=128).contains(&radius) {
            return false;
        }
        if at[0] < 0
            || at[1] < 0
            || at[2] < 0
            || at[0] >= world.nx as i32
            || at[1] >= world.ny as i32
            || at[2] >= world.nz as i32
        {
            return false;
        }
        let cell = (t.cell as i32).max((radius + 31) / 32);
        let half = (radius + cell - 1) / cell;
        let n = (half * 2 + 1) as usize;
        let origin = at.map(|v| v - half * cell);
        let high = origin.map(|v| v + n as i32 * cell);
        let Ok(input) = Snapshot::capture(world, origin, high) else {
            return false;
        };
        self.jobs.push_back(Job {
            at,
            origin,
            n,
            cell,
            input,
            cursor: 0,
            resistance: vec![0; n.pow(3)],
            wave: None,
            hits: VecDeque::new(),
            subcell: 0,
            energy_mj: energy_j as u64 * 1000,
            body_hit_done: false,
            body_energy_mj: 0,
            body_field: crate::body_wave::Field::default(),
        });
        true
    }

    /// Ordered jobs share all budgets. A cap postpones work and never loses
    /// an explosion, a voxel, or a fragment. Only the normal edit queue mutates terrain.
    pub fn capture_last(
        &mut self,
        bodies: &crate::damage::Bodies,
        frames: &[Option<[crate::damage::Frame; 2]>],
        t: &Blast,
    ) {
        let Some(j) = self.jobs.back_mut() else {
            return;
        };
        // Only a job that has not started may take bodies: one that has
        // sampled, captured or handed its resistance to its wave keeps
        // what it has (ADR-287). The load stand caught a refused blast
        // recapturing the last job after its wave had emptied the buffer.
        if j.cursor > 0 || j.wave.is_some() || !j.body_field.empty() {
            return;
        }
        j.body_field =
            crate::body_wave::Field::capture(bodies, frames, j.origin, j.n as i32 * j.cell);
        if !j.body_field.empty() {
            j.body_energy_mj = j.energy_mj * t.body_energy_pct as u64 / 100;
            j.energy_mj -= j.body_energy_mj;
        }
    }
    pub fn advance(&mut self, world: &mut voxel::World, mats: &Registry, t: &Blast) {
        self.advance_inner(world, mats, t, None, None);
    }
    pub fn advance_material(
        &mut self,
        world: &mut voxel::World,
        mats: &Registry,
        bodies: Option<&mut crate::damage::Bodies>,
        t: &crate::Tuning,
    ) {
        self.advance_inner(world, mats, &t.blast, bodies, Some(&t.impact));
    }
    fn advance_inner(
        &mut self,
        world: &mut voxel::World,
        mats: &Registry,
        t: &Blast,
        mut bodies: Option<&mut crate::damage::Bodies>,
        impact: Option<&modding::tuning::Impact>,
    ) {
        let mut samples = t.samples_per_tick;
        let mut pops = t.wave_pops_per_tick;
        let mut edits = t.edits_per_tick;
        let moving = self.fragments.iter().filter(|f| !f.resting).count();
        let room = (t.max_flying as usize).saturating_sub(moving);
        for _ in 0..self.jobs.len() {
            let mut j = self.jobs.pop_front().unwrap();
            while j.cursor < j.samples() && samples > 0 {
                samples -= 1;
                let (p, c) = j.sample(j.cursor);
                let m = j.input.material(p);
                // Unknown solid materials are barriers, never silently air.
                let r = if m == AIR {
                    0
                } else {
                    mats.get(m).map_or(1_000_000, |d| {
                        d.strength.saturating_mul(t.resistance_per_strength)
                    })
                };
                j.resistance[c] = j.resistance[c].max(r);
                j.cursor += 1;
            }
            if j.cursor == j.samples() && !j.body_field.finished() {
                if let Some(impact) = impact {
                    j.body_field.build(
                        j.origin,
                        j.cell,
                        j.n,
                        &mut j.resistance,
                        &impact.work_kj_m3,
                        t.body_resistance_ppm,
                        &mut samples,
                    );
                }
            }
            if j.cursor == j.samples() && j.body_field.finished() && j.wave.is_none() {
                let amplitude = (j.n as u32 / 2) * 100;
                j.wave = Some(
                    Wave::new(j.n, amplitude.max(1), std::mem::take(&mut j.resistance)).unwrap(),
                );
            }
            if let Some(wave) = j.wave.as_mut() {
                if j.hits.is_empty() && pops > 0 {
                    let before = wave.examined;
                    j.hits.extend(wave.advance(pops as usize));
                    pops -= (wave.examined - before) as u32;
                }
                while let Some(hit) = j.hits.front().copied() {
                    if edits == 0 {
                        break;
                    }
                    if !j.body_hit_done {
                        if let (Some(bodies), Some(impact)) = (bodies.as_deref_mut(), impact) {
                            let before = j.body_energy_mj;
                            let done = j.body_field.hit(
                                hit,
                                j.origin,
                                j.cell,
                                j.n,
                                j.at,
                                &mut j.body_energy_mj,
                                bodies,
                                impact,
                                t.body_resistance_ppm,
                                &mut edits,
                            );
                            self.spent_mj = self.spent_mj.saturating_add(before - j.body_energy_mj);
                            if !done {
                                break;
                            }
                        }
                        j.body_hit_done = true;
                    }
                    if edits == 0 {
                        break;
                    }
                    // Air visits and rejected/duplicate candidates consume work too.
                    edits -= 1;
                    if !hit.broken || j.energy_mj == 0 {
                        j.hits.pop_front();
                        j.subcell = 0;
                        j.body_hit_done = false;
                        continue;
                    }
                    if self.claims.len() >= t.max_claims as usize
                        || self.claims.len() >= room
                        || self.claims.len() + self.fragments.len() >= t.max_fragments as usize
                    {
                        break;
                    }
                    let q = ijk(j.subcell, j.cell as usize);
                    let p: [i32; 3] =
                        std::array::from_fn(|a| j.origin[a] + hit.cell[a] * j.cell + q[a]);
                    j.subcell += 1;
                    if j.subcell == (j.cell as usize).pow(3) {
                        j.subcell = 0;
                        j.hits.pop_front();
                        j.body_hit_done = false;
                    }
                    if p[0] < 0
                        || p[1] < 0
                        || p[2] < 0
                        || p[0] >= world.nx as i32
                        || p[1] >= world.ny as i32
                        || p[2] >= world.nz as i32
                    {
                        continue;
                    }
                    let key = p.map(|v| v as u32);
                    // The frame takes no edit (ADR-034): a claim there would
                    // never clear, and the energy paid for it would buy
                    // nothing (ADR-420).
                    if self.claims.contains_key(&key)
                        || !world.solid(key[0], key[1], key[2])
                        || world.on_frame(key[0], key[1], key[2])
                    {
                        continue;
                    }
                    let material = world.material(key[0], key[1], key[2]);
                    // Snapshot determines reach; current material determines the work
                    // and prevents a cheap old soil cell destroying new steel for free.
                    let Some(d) = mats.get(material) else {
                        continue;
                    };
                    if d.density == 0 {
                        continue;
                    }
                    let (cost, motion) = fracture_cost_mj(d.strength, voxel::unit::WORLD_MM, t);
                    if cost > j.energy_mj {
                        continue;
                    }
                    j.energy_mj -= cost;
                    self.spent_mj = self.spent_mj.saturating_add(cost);
                    self.claims.insert(
                        key,
                        Claim {
                            at: j.at,
                            motion_mj: motion,
                            side: t.fragment_box as u8,
                        },
                    );
                    world.queue_edit(Edit::dig_if(key[0], key[1], key[2], material));
                }
            }
            let done = j.wave.as_ref().is_some_and(|w| w.finished()) && j.hits.is_empty();
            if !done {
                self.jobs.push_back(j);
            }
        }
    }

    /// A no-op consumes its reservation too. Only solid -> air creates debris.
    pub fn confirm(&mut self, completed: &[(Edit, Mat)], e: &mut Economy, t: &Blast) {
        type GroundGroup = ([u32; 3], Mat, [i32; 3], u8);
        let mut groups: BTreeMap<GroundGroup, (u64, u64)> = BTreeMap::new();
        for &(edit, old) in completed {
            let Some(expected) = edit.expected_material() else {
                continue;
            };
            let key = [edit.x, edit.y, edit.z];
            let Some(claim) = self.claims.remove(&key) else {
                continue;
            };
            if old == AIR || old != expected {
                continue;
            }
            let (origin, side) = fracture_box(key, claim.side, t);
            let side = u32::from(side);
            let sub = (key[0] - origin[0])
                + side * (key[1] - origin[1])
                + side * side * (key[2] - origin[2]);
            let group = groups
                .entry((origin, old, claim.at, side as u8))
                .or_default();
            group.0 |= 1u64 << sub;
            group.1 = group.1.saturating_add(claim.motion_mj);
        }
        for ((key, old, at, side), (whole, total_motion_mj)) in groups {
            for shape in shape_components(whole, side) {
                let count = shape.count_ones();
                let motion_mj = total_motion_mj * count as u64 / whole.count_ones() as u64;
                self.spawn(key, old, at, side, shape, motion_mj, e, t);
            }
        }
    }

    /// One piece: the `shape` cells of `old` in the `side`-box at `key`,
    /// thrown from `at` with `motion_mj` of kinetic energy, its matter
    /// revealed from the ground into a container of its own. `false` for
    /// a material the registry does not know.
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        key: [u32; 3],
        old: Mat,
        at: [i32; 3],
        side: u8,
        shape: u64,
        motion_mj: u64,
        e: &mut Economy,
        t: &Blast,
    ) -> bool {
        let count = shape.count_ones();
        let Some(d) = e.mats.get(old) else {
            return false;
        };
        let mass_g = (d.density as u64 * t.density_kg_m3_per_density as u64)
            .saturating_mul((voxel::unit::WORLD_MM as u64).pow(3))
            / 1_000_000
            * count as u64;
        let position = centre(key).add(shape_centre(shape, side));
        let mut direction = position.sub(v3(at.map(Fx::from_int)));
        // Ground reflection lifts ejecta. This redirects, without adding to,
        // the assigned kinetic energy; no renderer randomness enters physics.
        direction.y = direction
            .y
            .abs()
            .add(
                Fx::from_ratio(t.lift_pct as i32, 100)
                    .mul(direction.x.abs().add(direction.z.abs())),
            )
            .add(Fx::ONE);
        // speed² in (world units/tick)², computed before Fx to avoid overflow.
        let denom = mass_g.max(1) as u128
            * (voxel::unit::WORLD_MM as u128).pow(2)
            * (crate::TICKS_PER_SECOND as u128).pow(2);
        let v2_raw = ((motion_mj as u128 * 2_000_000) << 32) / denom;
        let speed = Fx(v2_raw.min(i64::MAX as u128) as i64)
            .sqrt()
            .min(Fx::from_ratio(
                t.max_speed_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            ));
        let mut velocity = scattered_launch(self.next_id + 1, direction, speed, t);
        // Ground reflection keeps ejecta above the surface after scatter too.
        velocity.y = velocity.y.abs();
        let mm_s = arr(velocity).map(|v| {
            v.mul(Fx::from_int(
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            ))
            .to_int_trunc() as i64
        });
        self.motion_mj = self.motion_mj.saturating_add(voxel::impact::kinetic_mj(
            mass_g.min(u32::MAX as u64) as u32,
            mm_s,
        ));
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let amount = e.per_voxel(old) * count as u64;
        e.ledger.reveal(e.ground, amount);
        e.ledger.transfer(e.ground, held, amount);
        self.next_id += 1;
        self.removed += count as u64;
        self.fragments.push(Fragment {
            id: self.next_id,
            position,
            velocity,
            material: old,
            paint: None,
            mm: voxel::unit::WORLD_MM,
            basis: IDENTITY,
            mass_g,
            count,
            shape,
            side,
            chunk: None,
            held,
            resting: false,
        });
        true
    }

    /// Cells the world let go of (ADR-299: terrain left without support)
    /// become pieces at zero speed, grouped by box and material like burst
    /// debris. A shape the pool has no room for goes back into the world
    /// cell by cell, so that no matter is lost; it will be found hanging
    /// again. Returns pieces made and cells put back.
    pub fn pieces_from_cells(
        &mut self,
        world: &mut voxel::World,
        cells: &[([u32; 3], Mat)],
        e: &mut Economy,
        t: &Blast,
        side: u8,
    ) -> (usize, usize) {
        let side = side.clamp(1, 4);
        let s = u32::from(side);
        let mut groups: BTreeMap<([u32; 3], Mat), u64> = BTreeMap::new();
        for &(p, m) in cells {
            if m == AIR {
                continue;
            }
            let origin = p.map(|v| v / s * s);
            let sub = (p[0] - origin[0]) + s * (p[1] - origin[1]) + s * s * (p[2] - origin[2]);
            *groups.entry((origin, m)).or_default() |= 1u64 << sub;
        }
        let (mut made, mut returned) = (0, 0);
        for ((key, m), whole) in groups {
            let at = key.map(|v| v as i32 + i32::from(side) / 2);
            for shape in shape_components(whole, side) {
                if self.fragment_room(t) > 0 && self.spawn(key, m, at, side, shape, 0, e, t) {
                    made += 1;
                    continue;
                }
                // No room for a piece: the cell stays in the world. Its
                // detach dig is a transfer and drains first; so does this
                // put-back, right behind it (ADR-431).
                for i in 0..box_cells(side) {
                    if shape & (1 << i) != 0 {
                        let c = shape_cell(i, side);
                        world.queue_transfer(Edit::put(
                            key[0] + c[0] as u32,
                            key[1] + c[1] as u32,
                            key[2] + c[2] as u32,
                            m,
                        ));
                        returned += 1;
                    }
                }
            }
        }
        (made, returned)
    }

    pub fn step_fragments(
        &mut self,
        world: &voxel::World,
        e: &mut Economy,
        t: &Blast,
        gravity: Fx,
        tick: u64,
        around: Surroundings<'_>,
    ) {
        self.step_fragments_observed(world, e, t, gravity, tick, around, |_| {});
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step_fragments_observed(
        &mut self,
        world: &voxel::World,
        e: &mut Economy,
        t: &Blast,
        gravity: Fx,
        tick: u64,
        around: Surroundings<'_>,
        mut completed: impl FnMut(&'static str),
    ) {
        let (hulls, yielding) = (around.hulls, around.yielding);
        let motion = FragmentMotion::new(t);
        // What has already landed is ground for what is still falling
        // (ADR-308), gathered once for the whole tick.
        let mut settled = std::mem::take(&mut self.settled_scratch);
        let mut queries = std::mem::take(&mut self.query_scratch);
        Settled::of_into(&self.fragments, hulls, tick, t, &mut settled, &mut queries);
        completed("debris_settled");
        type RestingPile = ([i32; 3], Mat, Option<[u8; 3]>, u16);
        let mut merged = BTreeSet::new();
        let mut stale_wrecks = BTreeSet::new();
        // Waking pieces count against the flying budget like new ones; a
        // piece without room waits for its next turn. A claim holds its
        // flying slot until its dig lands and it becomes a piece: a
        // waking piece does not take it (ADR-443).
        let mut room = (t.max_flying as usize).saturating_sub(
            self.fragments.iter().filter(|f| !f.resting).count() + self.claims.len(),
        );
        // These queries read the frozen surroundings and only this piece.
        // Compute candidates independently; the ordered pass below still
        // allocates flying slots and therefore resolves competing wakeups.
        let query = |(i, f): (usize, &Fragment)| {
            let mut push = None;
            if f.chunk.is_none() && t.push_pct > 0 && (!f.resting || room > 0) {
                let (low, high) = f.bounds();
                if let Some(h) = hull_hit(arr(f.position), low, high, hulls, 0) {
                    // What the hull climbs as floor it does not push: a
                    // piece of a yielding material, or one rising above
                    // the hull's step, is bulldozed; the rest is ridden.
                    let climbs = !yielding.contains(&f.material)
                        && f.position.y.add(high[1]) <= h.pos[1].add(h.step);
                    if h.vel != [Fx::ZERO; 3] && !climbs {
                        // A moving hull bulldozes the piece it drove into
                        // (ADR-294): the piece leaves ahead of it at the
                        // hull's pace and a little up.
                        let along = V3::new(h.vel[0], Fx::ZERO, h.vel[2]).scale(motion.push);
                        let lift = along.len_sq().sqrt().mul(motion.push_up);
                        push = Some(V3::new(
                            along.x,
                            h.vel[1].mul(motion.push).add(lift),
                            along.z,
                        ));
                    }
                }
            }
            let unsupported = room > 0
                && push.is_none()
                && f.resting
                && due(f.id, tick, t)
                && !supported(f, world, hulls, &settled, i as u32);
            (push, unsupported)
        };
        let wakeups: Vec<_> = if self.fragments.len() >= 1024 {
            self.fragments.par_iter().enumerate().map(query).collect()
        } else {
            self.fragments.iter().enumerate().map(query).collect()
        };
        for (f, (push, unsupported)) in self.fragments.iter_mut().zip(wakeups) {
            if let Some(velocity) = push.filter(|_| !f.resting || room > 0) {
                if f.resting {
                    f.resting = false;
                    room -= 1;
                    stale_wrecks.insert(f.held);
                }
                f.velocity = velocity;
            }
            if room > 0 && f.resting && unsupported {
                // The ground under it was taken (ADR-291): it falls again,
                // and its wreck entry goes until it settles anew.
                f.resting = false;
                room -= 1;
                stale_wrecks.insert(f.held);
            }
        }
        completed("debris_wake");
        // All support checks and flying-slot claims above retain fragment
        // order. Translation reads only the frozen world, hulls and settled
        // index; no fragment observes another fragment's new position until
        // the ordered pile/ledger pass below.
        let translate = |(i, f): (usize, &mut Fragment)| {
            advance_fragment(
                f, world, t, &motion, gravity, hulls, &settled, i as u32, true,
            )
            .then(|| {
                let p = arr(f.position).map(|v| v.to_int_trunc());
                Wreck {
                    held: f.held,
                    x: p[0],
                    y: p[1],
                    z: p[2],
                }
            })
        };
        let landed_slots: Vec<Option<Wreck>> = if self.fragments.len() >= 1024 {
            self.fragments
                .par_iter_mut()
                .enumerate()
                .map(translate)
                .collect()
        } else {
            self.fragments
                .iter_mut()
                .enumerate()
                .map(translate)
                .collect()
        };
        let landed: Vec<Wreck> = landed_slots.into_iter().flatten().collect();
        completed("debris_translate");
        // Equal keys always enter the same shard; source slots stay ordered
        // inside it. Each independent table therefore chooses the same first
        // pile owner as the original single table, without shared mutation.
        let mut shards: [Vec<(RestingPile, usize)>; 8] = std::array::from_fn(|_| Vec::new());
        let parallel = self.fragments.len() >= 1024;
        for i in 0..self.fragments.len() {
            if !self.fragments[i].resting || self.fragments[i].chunk.is_some() {
                continue;
            }
            let f = &self.fragments[i];
            let key = (
                arr(f.position).map(|v| v.to_int_trunc().div_euclid(t.pile_cell as i32)),
                f.material,
                f.paint,
                f.mm,
            );
            let hash = (key.0[0] as u32).wrapping_mul(0x9e3779b9).rotate_left(13)
                ^ (key.0[1] as u32).wrapping_mul(0xc2b2ae35)
                ^ (key.0[2] as u32).wrapping_mul(0x85ebca6b);
            let shard = if parallel { (hash >> 29) as usize } else { 0 };
            shards[shard].push((key, i));
        }
        completed("debris_merge_keys");
        let group = |entries: &Vec<(RestingPile, usize)>| {
            let mut piles: HashMap<RestingPile, usize> = HashMap::with_capacity(entries.len());
            let mut pairs = Vec::new();
            for &(key, donor) in entries {
                if let Some(&head) = piles.get(&key) {
                    pairs.push((head, donor));
                } else {
                    piles.insert(key, donor);
                }
            }
            pairs
        };
        let groups: Vec<_> = if parallel {
            shards.par_iter().map(group).collect()
        } else {
            shards[..1].iter().map(group).collect()
        };
        completed("debris_merge_groups");
        let mut transfers: Vec<_> = groups.into_iter().flatten().collect();
        transfers.sort_unstable_by_key(|&(_, donor)| donor);
        for (head, donor) in transfers {
            let (a, b) = self.fragments.split_at_mut(donor);
            let to = &mut a[head];
            let from = &b[0];
            e.ledger.transfer(from.held, to.held, u64::MAX);
            to.mass_g += from.mass_g;
            to.count += from.count;
            // Grow after every donor in original order. Seat on terrain, but
            // retain the bottom on a piece or roof so the pile does not sink
            // into its support (ADR-457).
            let bottom = to.position.y.add(to.bounds().0[1]);
            restack(to);
            match world_floor(to, world) {
                Some(floor) if bottom.sub(floor) <= Fx::from_ratio(1, 2) => seat(to, floor),
                _ => to.position.y = bottom.sub(to.bounds().0[1]),
            }
            merged.insert(from.held);
        }
        completed("debris_advance");
        if !merged.is_empty() {
            self.fragments.retain(|f| !merged.contains(&f.held));
            stale_wrecks.extend(merged.iter().copied());
        }
        // A waking piece can land again in this same tick. Remove its old
        // wreck once, then append the newly landed one in fragment order.
        if !stale_wrecks.is_empty() {
            e.wrecks.retain(|w| !stale_wrecks.contains(&w.held));
        }
        e.wrecks
            .extend(landed.into_iter().filter(|w| !merged.contains(&w.held)));
        for held in merged {
            e.ledger.retire_empty_debris(held);
        }
        for f in self.corpses.values_mut() {
            if f.resting
                && due(f.id, tick, t)
                && !supported(f, world, &[], &Settled::default(), u32::MAX)
            {
                f.resting = false;
            }
            // A dead hull lies as it fell: whole hulls do not tumble.
            advance_fragment(
                f,
                world,
                t,
                &motion,
                gravity,
                &[],
                &Settled::default(),
                u32::MAX,
                false,
            );
        }
        for w in &mut e.wrecks {
            if let Some(f) = self.corpses.get(&w.held) {
                let p = arr(f.position).map(|v| v.to_int_trunc());
                w.x = p[0];
                w.y = p[1];
                w.z = p[2];
            }
        }
        self.settled_scratch = settled;
        self.query_scratch = queries;
        completed("debris_finish");
    }

    /// The columns resting pieces stand in and the height of their tops,
    /// for the contacts, feet and planner of the range (ADR-292). Pieces
    /// of `yielding` materials are left out: a hull crushes through a
    /// felled crown as through a standing one, and so is a piece lying on
    /// a hull's roof rather than on the world (ADR-293). Derived from the
    /// pieces every tick; nothing is saved.
    /// A piece that came to rest against a live hull's flank — its centre
    /// outside the hull's box, its edge reaching under — is no floor under
    /// that hull (ADR-302): the columns of the box it covers stay out of
    /// the map, while a piece the hull stands over is floor as before.
    pub fn rubble(
        &self,
        world: &voxel::World,
        yielding: &[Mat],
        hulls: &[Hull],
    ) -> nav::vehicle::Rubble {
        // Floor is what something holds: the world, or a resting piece
        // under it — a pile on a rock piece is floor at the pile's top.
        // Only the world was asked, and a stacked piece was missing from
        // the map (ADR-458). A piece on a roof, or one whose support was
        // taken and whose turn to fall has not come, is not floor; one a
        // hull carries is not asked about the pieces (ADR-469).
        // Reuse local boxes only within this immutable query: support edits,
        // hull movement and fragment changes are visible on the next call.
        let mut bounds = vec![None; self.fragments.len()];
        let mut floor = Vec::new();
        let mut stacked = Vec::new();
        for (i, f) in self.fragments.iter().enumerate() {
            if !f.resting || f.chunk.is_some() || yielding.contains(&f.material) {
                continue;
            }
            let (low, high) = f.bounds();
            bounds[i] = Some((low, high));
            let mut p = arr(f.position);
            p[1] = p[1].sub(Fx::from_ratio(1, 2));
            if fragment_contact(f, world, p, low, high, [world.nx, world.ny, world.nz]) {
                floor.push(i);
            } else if !hulls
                .iter()
                .any(|h| h.carries(f.position.y.add(low[1]), f.position))
            {
                stacked.push(i);
            }
        }
        if !stacked.is_empty() {
            let settled = Settled::around_bounds(
                &self.fragments,
                hulls,
                |i, _| stacked.binary_search(&i).is_ok(),
                |i, f| bounds[i].unwrap_or_else(|| f.bounds()),
            );
            // These pieces already failed the world support test above.
            // No world or hull changes occur while this query is running.
            floor.extend(stacked.into_iter().filter(|&i| {
                let f = &self.fragments[i];
                let (low, high) = bounds[i].unwrap();
                let mut p = arr(f.position);
                p[1] = p[1].sub(Fx::from_ratio(1, 2));
                settled.hit(
                    [0, 1, 2].map(|a| p[a].add(low[a])),
                    [0, 1, 2].map(|a| p[a].add(high[a])),
                    i as u32,
                    Some(f.position.y.add(low[1])),
                )
            }));
        }
        // A hull covers no column outside the box around its footprint:
        // a piece far from every hull asks none of them (ADR-465).
        let reach: Vec<([Fx; 2], [Fx; 2])> = hulls.iter().map(Hull::footprint_box).collect();
        let mut against: Vec<&Hull> = Vec::with_capacity(hulls.len());
        // The columns with their tops, gathered and sorted, and the map
        // built from them at once: thousands of single insertions into
        // the tree cost a millisecond a step on a battlefield (ADR-465).
        let mut columns: Vec<([i32; 2], u16)> = Vec::new();
        for i in floor {
            let f = &self.fragments[i];
            let (low, high) = bounds[i].unwrap();
            let p = arr(f.position);
            // Floor of the bottom corners, ceiling of the top: the columns
            // the box covers and the height it reaches.
            let lo = [p[0].add(low[0]), p[2].add(low[2])].map(|v| (v.raw() >> 32) as i32);
            let hi = [p[0].add(high[0]), p[2].add(high[2])].map(|v| (v.raw() >> 32) as i32);
            let top = -(p[1].add(high[1]).neg().raw() >> 32);
            let top = top.clamp(0, i64::from(u16::MAX)) as u16;
            against.clear();
            against.extend(hulls.iter().zip(&reach).filter_map(|(h, (b0, b1))| {
                let near = (0..2).all(|a| {
                    Fx::from_int(lo[a]) <= b1[a] && Fx::from_int(hi[a]).add(Fx::ONE) >= b0[a]
                });
                (near && !h.covers_point([p[0], p[2]])).then_some(h)
            }));
            for z in lo[1].max(0)..=hi[1] {
                for x in lo[0].max(0)..=hi[0] {
                    if against.iter().any(|h| h.covers(x, z)) {
                        continue;
                    }
                    columns.push(([x, z], top));
                }
            }
        }
        // A column under several pieces holds the highest top.
        sort_columns(&mut columns);
        columns.dedup_by(|later, kept| {
            let same = later.0 == kept.0;
            if same {
                kept.1 = kept.1.max(later.1);
            }
            same
        });
        nav::vehicle::Rubble::from_sorted(columns)
    }
    /// Before empty wreck containers are released/reused, remove their geometry.
    pub fn sweep(&mut self, e: &Economy) {
        self.corpses.retain(|held, _| e.ledger.held(*held) > 0);
        self.fragments
            .retain(|f| !f.resting || e.ledger.held(f.held) > 0);
    }

    pub fn hash(&self) -> u64 {
        if self.next_id == 0 && self.jobs.is_empty() && self.claims.is_empty() && self.spent_mj == 0
        {
            return 0;
        }
        let mut h = 0;
        for v in [
            self.next_id,
            self.removed,
            self.spent_mj,
            self.motion_mj,
            self.body_motion_mj,
            self.body_removed,
            self.jobs.len() as u64,
        ] {
            mix(&mut h, v);
        }
        for j in &self.jobs {
            for v in j.at.into_iter().chain(j.origin) {
                mix(&mut h, v as u64);
            }
            for v in [
                j.n as u64,
                j.cell as u64,
                j.input.hash(),
                j.cursor as u64,
                j.subcell as u64,
                j.energy_mj,
                j.body_energy_mj,
                j.body_field.hash(),
                j.body_hit_done as u64,
                j.wave.as_ref().map_or(0, |w| w.examined as u64 + 1),
                j.hits.len() as u64,
            ] {
                mix(&mut h, v);
            }
            // Resistance and frontier are pure functions of this immutable input
            // and exact build/pop cursors. No full 65³ traversal on each hash tick.
            for hit in &j.hits {
                for v in hit.cell {
                    mix(&mut h, v as u64);
                }
                mix(&mut h, hit.pressure as u64);
                mix(&mut h, hit.broken as u64);
            }
        }
        mix(&mut h, self.claims.len() as u64);
        for (p, c) in &self.claims {
            for v in *p {
                mix(&mut h, v as u64);
            }
            for v in c.at {
                mix(&mut h, v as u64);
            }
            mix(&mut h, c.motion_mj);
            mix(&mut h, u64::from(c.side));
        }
        mix(&mut h, self.fragments.len() as u64);
        if !self.corpses.is_empty() {
            mix(&mut h, self.corpses.len() as u64);
        }
        for f in self.fragments.iter().chain(self.corpses.values()) {
            for v in [
                f.id,
                f.material as u64,
                f.mm as u64,
                f.paint.map_or(0, |p| {
                    1 + p[0] as u64 + ((p[1] as u64) << 8) + ((p[2] as u64) << 16)
                }),
                f.mass_g,
                f.count as u64,
                f.shape,
                u64::from(f.side),
                f.held as u64,
                f.resting as u64,
            ] {
                mix(&mut h, v);
            }
            if let Some(c) = &f.chunk {
                mix(&mut h, c.hash());
            }
            for v in arr(f.position)
                .into_iter()
                .chain(arr(f.velocity))
                .chain(f.basis.into_iter().flat_map(arr))
            {
                mix(&mut h, v.raw() as u64);
            }
        }
        h
    }

    pub fn save(&self, b: &mut Blob) {
        for v in [
            self.next_id,
            self.removed,
            self.spent_mj,
            self.motion_mj,
            self.body_motion_mj,
            self.body_removed,
        ] {
            b.u64(v);
        }
        b.usize(self.jobs.len());
        for j in &self.jobs {
            for v in j.at.into_iter().chain(j.origin) {
                b.i32(v);
            }
            b.usize(j.n);
            b.i32(j.cell);
            j.input.save(b);
            b.usize(j.cursor);
            b.u32s(&j.resistance);
            b.bool(j.wave.is_some());
            if let Some(w) = &j.wave {
                w.save(b);
            }
            b.usize(j.hits.len());
            for hit in &j.hits {
                for v in hit.cell {
                    b.i32(v);
                }
                b.u32(hit.pressure);
                b.bool(hit.broken);
            }
            b.usize(j.subcell);
            b.u64(j.energy_mj);
            b.u64(j.body_energy_mj);
            b.bool(j.body_hit_done);
            j.body_field.save(b);
        }
        b.usize(self.claims.len());
        for (p, c) in &self.claims {
            for v in *p {
                b.u32(v);
            }
            for v in c.at {
                b.i32(v);
            }
            b.u64(c.motion_mj);
            b.u8(c.side);
        }
        b.usize(self.fragments.len());
        b.usize(self.corpses.len());
        for f in self.fragments.iter().chain(self.corpses.values()) {
            b.u64(f.id);
            save_v(b, f.position);
            save_v(b, f.velocity);
            b.u16(f.material);
            b.u16(f.mm);
            b.bool(f.paint.is_some());
            if let Some(p) = f.paint {
                for v in p {
                    b.u8(v);
                }
            }
            for v in f.basis {
                save_v(b, v);
            }
            b.u64(f.mass_g);
            b.u32(f.count);
            b.u64(f.shape);
            b.u8(f.side);
            b.u32(f.held);
            b.bool(f.resting);
            b.bool(f.chunk.is_some());
            if let Some(c) = &f.chunk {
                c.save(b);
            }
        }
    }
    pub fn load(s: &mut Scan, world: Option<&voxel::World>, t: &Blast) -> Option<Self> {
        Self::load_material(s, world, t, None, 0, false)
    }
    /// `two_cell_masks`: the save predates ADR-284 and holds eight-bit
    /// masks without a box side; every piece is read as a two-cell box.
    pub fn load_material(
        s: &mut Scan,
        world: Option<&voxel::World>,
        t: &Blast,
        bodies: Option<&crate::damage::Bodies>,
        units: usize,
        two_cell_masks: bool,
    ) -> Option<Self> {
        let mut out = Self {
            next_id: s.u64(),
            removed: s.u64(),
            spent_mj: s.u64(),
            motion_mj: s.u64(),
            body_motion_mj: s.u64(),
            body_removed: s.u64(),
            ..Self::default()
        };
        let count = s.usize();
        if count > t.max_pending as usize {
            return None;
        }
        for _ in 0..count {
            let at = std::array::from_fn(|_| s.i32());
            let origin = std::array::from_fn(|_| s.i32());
            let n = s.usize();
            let cell = s.i32();
            if !(3..=65).contains(&n) || n.is_multiple_of(2) || !(1..=4).contains(&cell) {
                return None;
            }
            let w = world?;
            let limits = [w.nx, w.ny, w.nz];
            if (0..3).any(|k| {
                at[k] < 0
                    || at[k] as u32 >= limits[k]
                    || origin[k] as i64 != at[k] as i64 - (n as i64 / 2) * cell as i64
            }) {
                return None;
            }
            let input = Snapshot::load(s, world?)?;
            let cursor = s.usize();
            let resistance = s.u32s();
            let wave = if s.bool() { Some(Wave::load(s)?) } else { None };
            if cursor > n.pow(3) * (cell as usize).pow(3)
                || (wave.is_none() && resistance.len() != n.pow(3))
                || (wave.is_some()
                    && (cursor != n.pow(3) * (cell as usize).pow(3) || !resistance.is_empty()))
            {
                return None;
            }
            if wave.as_ref().is_some_and(|w| w.size() != n) {
                return None;
            }
            let count = s.usize();
            if count > n.pow(3) {
                return None;
            }
            let mut hits = VecDeque::new();
            for _ in 0..count {
                let cell = std::array::from_fn(|_| s.i32());
                let pressure = s.u32();
                let broken = s.bool();
                if cell.iter().any(|&v| v < 0 || v >= n as i32) || pressure > 1_000_000 {
                    return None;
                }
                hits.push_back(Hit {
                    cell,
                    pressure,
                    broken,
                });
            }
            let subcell = s.usize();
            let energy_mj = s.u64();
            let body_energy_mj = s.u64();
            let body_hit_done = s.bool();
            let body_field = crate::body_wave::Field::load(s, bodies, units, n)?;
            if subcell >= (cell as usize).pow(3)
                || !body_field.valid_domain(origin, cell, n, hits.front().copied())
                || (wave.is_some() && !body_field.finished())
            {
                return None;
            }
            out.jobs.push_back(Job {
                at,
                origin,
                n,
                cell,
                input,
                cursor,
                resistance,
                wave,
                hits,
                subcell,
                energy_mj,
                body_energy_mj,
                body_field,
                body_hit_done,
            });
        }
        let count = s.usize();
        if count > t.max_claims as usize {
            return None;
        }
        for _ in 0..count {
            let p: [u32; 3] = std::array::from_fn(|_| s.u32());
            let at = std::array::from_fn(|_| s.i32());
            let motion_mj = s.u64();
            let side = if two_cell_masks { 2 } else { s.u8() };
            let w = world?;
            if p[0] >= w.nx
                || p[1] >= w.ny
                || p[2] >= w.nz
                || !(1..=4).contains(&side)
                || out
                    .claims
                    .insert(
                        p,
                        Claim {
                            at,
                            motion_mj,
                            side,
                        },
                    )
                    .is_some()
            {
                return None;
            }
        }
        let count = s.usize();
        let corpses = s.usize();
        if count > t.max_fragments as usize || corpses > s.left() / 100 {
            return None;
        }
        let mut ids = BTreeSet::new();
        let mut helds = BTreeSet::new();
        for i in 0..count + corpses {
            let mut f = Fragment {
                id: s.u64(),
                position: load_v(s),
                velocity: load_v(s),
                material: s.u16(),
                mm: s.u16(),
                paint: if s.bool() {
                    Some([s.u8(), s.u8(), s.u8()])
                } else {
                    None
                },
                basis: std::array::from_fn(|_| load_v(s)),
                mass_g: s.u64(),
                count: s.u32(),
                shape: if two_cell_masks {
                    u64::from(s.u8())
                } else {
                    s.u64()
                },
                side: if two_cell_masks { 2 } else { s.u8() },
                chunk: None,
                held: s.u32(),
                resting: s.bool(),
            };
            if s.bool() {
                f.chunk = Some(chunk::Chunk::load(s, bodies, f.mm, f.count)?);
            }
            if f.id == 0
                || f.id > out.next_id
                || !ids.insert(f.id)
                || !helds.insert(f.held)
                || f.count == 0
                || f.shape == 0
                || !(1..=4).contains(&f.side)
                || (f.side < 4 && f.shape >> box_cells(f.side) != 0)
                // A pile that fell again (ADR-291) flies with more cells
                // than its shape shows; a piece never has fewer.
                || (f.chunk.is_none() && f.count < f.shape.count_ones())
                || (f.material == AIR && f.paint.is_none())
                || f.mm == 0
                || f.mm > 1000
                || f.paint.is_some_and(|p| p[0] >= 8 || p[1] > 6 || p[2] > 4)
                || f.basis
                    .iter()
                    .any(|v| v.len_sq().sub(Fx::ONE).abs() > Fx::from_ratio(1, 100))
                || f.held == NO_TARGET
            {
                return None;
            }
            let w = world?;
            let limits = [w.nx, w.ny, w.nz];
            let vmax = Fx::from_ratio(
                t.max_speed_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            );
            if (0..3).any(|k| {
                arr(f.position)[k] < Fx::ZERO
                    || (k != 1 && arr(f.position)[k] >= Fx::from_int(limits[k] as i32))
                    || (k == 1 && arr(f.position)[k] > Fx::from_int(i32::MAX / 2))
            }) || arr(f.velocity).iter().any(|v| v.abs() > vmax)
                || f.mass_g == 0
            {
                return None;
            }
            let is_corpse = f.chunk.as_ref().is_some_and(|c| c.body.is_some());
            if is_corpse != (i >= count) {
                return None;
            }
            if is_corpse {
                out.corpses.insert(f.held, f);
            } else {
                out.fragments.push(f);
            }
        }
        s.ok().then_some(out)
    }
}

/// A pile stood on the world under its centre: how tests lay one down.
#[cfg(test)]
pub(crate) fn heap(f: &mut Fragment, world: &voxel::World) {
    restack(f);
    if let Some(floor) = world_floor(f, world) {
        seat(f, floor);
    }
}
/// Stands a restacked pile with its bottom layer on `floor`.
fn seat(f: &mut Fragment, floor: Fx) {
    let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
    let centre = arr(shape_centre(f.shape, 4));
    f.position.y = floor.add(centre[1].add(Fx::from_ratio(1, 2)).mul(scale));
}
/// The top of the world under a piece's centre, looking down from its
/// height; `None` off the map.
fn world_floor(f: &Fragment, world: &voxel::World) -> Option<Fx> {
    let p = arr(f.position);
    let (x, z) = ((p[0].raw() >> 32) as i32, (p[2].raw() >> 32) as i32);
    if x < 0 || z < 0 || x as u32 >= world.nx || z as u32 >= world.nz {
        return None;
    }
    let mut y = (p[1].raw() >> 32).clamp(0, i64::from(world.ny) - 1) as i32;
    while y >= 0 {
        if world.solid(x as u32, y as u32, z as u32) {
            return Some(Fx::from_int(y + 1));
        }
        y -= 1;
    }
    Some(Fx::ZERO)
}
/// A pile with more than one piece in it is a heap (ADR-292): its cells
/// fill a four-cell box from the bottom layer up, upright, so that it has
/// the height its matter takes. Matter beyond the box stays in the
/// container; the box only saturates. Where it stands is the caller's
/// (ADR-457).
fn restack(f: &mut Fragment) {
    let n = f.count.min(box_cells(4));
    let mut shape = 0u64;
    let mut placed = 0;
    for y in 0..4 {
        for z in 0..4 {
            for x in 0..4 {
                if placed < n {
                    shape |= 1u64 << (x + 4 * y + 16 * z);
                    placed += 1;
                }
            }
        }
    }
    f.shape = shape;
    f.side = 4;
    f.basis = IDENTITY;
}
/// A resting piece looks under itself every `rest_check_ticks` ticks
/// (ADR-291), the pieces spread over the period by id so that no tick
/// checks them all.
fn due(id: u64, tick: u64, t: &Blast) -> bool {
    id.wrapping_add(tick)
        .is_multiple_of(u64::from(t.rest_check_ticks.max(1)))
}
/// Whether material or a live hull still holds the piece up: its box half
/// a unit lower, the most a settling step moves, meets the world or a hull.
fn supported(
    f: &Fragment,
    world: &voxel::World,
    hulls: &[Hull],
    settled: &Settled,
    skip: u32,
) -> bool {
    let (low, high) = f.bounds();
    let mut p = arr(f.position);
    p[1] = p[1].sub(Fx::from_ratio(1, 2));
    fragment_contact(f, world, p, low, high, [world.nx, world.ny, world.nz])
        || hull_hit(p, low, high, hulls, 0).is_some()
        || settled.hit(
            [0, 1, 2].map(|a| p[a].add(low[a])),
            [0, 1, 2].map(|a| p[a].add(high[a])),
            skip,
            Some(f.position.y.add(low[1])),
        )
}
/// Columns in ascending column order, stably (ADR-465): four passes of
/// a byte each, over Z and then X. The columns of the rubble map lie in
/// `0..65536` on both axes, as a map is no wider than 16384; a comparison
/// sort of a battlefield's fifteen thousand columns cost more than half a
/// millisecond a step.
fn sort_columns(columns: &mut Vec<([i32; 2], u16)>) {
    debug_assert!(columns
        .iter()
        .all(|(c, _)| c.iter().all(|v| (0..1 << 16).contains(v))));
    let mut out = vec![([0, 0], 0u16); columns.len()];
    for (axis, shift) in [(1, 0), (1, 8), (0, 0), (0, 8)] {
        let digit = |c: &([i32; 2], u16)| ((c.0[axis] >> shift) & 0xff) as usize;
        let mut start = [0usize; 257];
        for c in columns.iter() {
            start[digit(c) + 1] += 1;
        }
        for d in 0..256 {
            start[d + 1] += start[d];
        }
        for c in columns.iter() {
            let d = digit(c);
            out[start[d]] = *c;
            start[d] += 1;
        }
        std::mem::swap(columns, &mut out);
    }
}
/// The boxes of resting pieces, in the cells a flying piece may reach
/// (ADR-308). Built once a tick: a piece that landed is ground for the
/// next one, and without this they fall through one another.
#[derive(Default)]
pub(crate) struct Settled {
    /// Cell of `CELL` units to the indices of the pieces whose box covers
    /// it, in index order.
    cells: FxHashMap<[i32; 3], Vec<u32>>,
    /// Reuse the per-cell lists when this scratch index is rebuilt next tick.
    free_lists: Vec<Vec<u32>>,
    /// Frozen world boxes in original fragment order, including unindexed
    /// pieces. The cell lists use those indices throughout the tick, even
    /// when pieces wake or merge. Reuses the construction scratch buffer.
    boxes: Vec<([Fx; 3], [Fx; 3])>,
}
/// Side of a cell of the settled index, world units.
const SETTLED_CELL: i32 = 4;
/// Membership-only query mask. A word covers 64 vertical index cells;
/// insertion ORs a range instead of hashing every Y coordinate. No path
/// iterates the hash table: contact order comes from the fragment slice.
#[derive(Default)]
struct SettledQueries(FxHashMap<[i32; 3], u64>);
impl SettledQueries {
    fn include(&mut self, lo: [i32; 3], hi: [i32; 3]) {
        let first = lo[1].div_euclid(64);
        let last = hi[1].div_euclid(64);
        for z in lo[2]..=hi[2] {
            for word in first..=last {
                let low = if word == first {
                    lo[1].rem_euclid(64)
                } else {
                    0
                };
                let high = if word == last {
                    hi[1].rem_euclid(64)
                } else {
                    63
                };
                let mask = (u64::MAX << low) & (u64::MAX >> (63 - high));
                for x in lo[0]..=hi[0] {
                    *self.0.entry([x, word, z]).or_default() |= mask;
                }
            }
        }
    }
    fn contains(&self, [x, y, z]: [i32; 3]) -> bool {
        self.0
            .get(&[x, y.div_euclid(64), z])
            .is_some_and(|word| word & (1 << y.rem_euclid(64)) != 0)
    }
}
fn settled_cells(lo: [Fx; 3], hi: [Fx; 3]) -> ([i32; 3], [i32; 3]) {
    let cell = |v: Fx| v.to_int_trunc().div_euclid(SETTLED_CELL);
    (
        [0, 1, 2].map(|a| cell(lo[a])),
        [0, 1, 2].map(|a| cell(hi[a])),
    )
}
impl Settled {
    /// The resting pieces around those that will ask about them this tick:
    /// the flying ones, and the resting ones whose turn it is to look for
    /// their support (ADR-291). Pieces on a hull's roof (ADR-293) are left
    /// out: they ride, they are not ground.
    #[cfg(test)]
    fn of(fragments: &[Fragment], hulls: &[Hull], tick: u64, t: &Blast) -> Settled {
        Self::around(fragments, hulls, |_, f| !f.resting || due(f.id, tick, t))
    }
    fn of_into(
        fragments: &[Fragment],
        hulls: &[Hull],
        tick: u64,
        t: &Blast,
        out: &mut Settled,
        want: &mut SettledQueries,
    ) {
        Self::around_bounds_into(
            fragments,
            hulls,
            |_, f| !f.resting || due(f.id, tick, t),
            |_, f| f.bounds(),
            out,
            want,
        );
    }
    /// The resting pieces around those `asks` picks, by index and piece;
    /// a hull's riders left out as in [`Settled::of`].
    #[cfg(test)]
    fn around(
        fragments: &[Fragment],
        hulls: &[Hull],
        asks: impl Fn(usize, &Fragment) -> bool,
    ) -> Settled {
        Self::around_bounds(fragments, hulls, asks, |_, f| f.bounds())
    }
    /// A caller that already measured pieces can supply the same local
    /// boxes. They must describe this exact immutable fragment slice.
    fn around_bounds(
        fragments: &[Fragment],
        hulls: &[Hull],
        asks: impl Fn(usize, &Fragment) -> bool,
        bounds: impl Fn(usize, &Fragment) -> ([Fx; 3], [Fx; 3]) + Sync,
    ) -> Settled {
        let mut out = Settled::default();
        Self::around_bounds_into(
            fragments,
            hulls,
            asks,
            bounds,
            &mut out,
            &mut SettledQueries::default(),
        );
        out
    }
    fn around_bounds_into(
        fragments: &[Fragment],
        hulls: &[Hull],
        asks: impl Fn(usize, &Fragment) -> bool,
        bounds: impl Fn(usize, &Fragment) -> ([Fx; 3], [Fx; 3]) + Sync,
        out: &mut Settled,
        want: &mut SettledQueries,
    ) {
        for (_, mut list) in out.cells.drain() {
            list.clear();
            out.free_lists.push(list);
        }
        out.boxes.clear();
        want.0.clear();
        if !fragments.iter().any(|f| f.resting) {
            return;
        }
        // Only membership is queried. Fragment insertion and contact order
        // still follow the ordered fragment slice, not the hash table.
        out.boxes
            .resize(fragments.len(), ([Fx::ZERO; 3], [Fx::ZERO; 3]));
        let world_box = |(i, f): (usize, &Fragment)| {
            let (low, high) = bounds(i, f);
            let p = arr(f.position);
            let lo = [0, 1, 2].map(|a| p[a].add(low[a]));
            let hi = [0, 1, 2].map(|a| p[a].add(high[a]));
            (lo, hi)
        };
        if fragments.len() >= 1024 {
            out.boxes
                .par_iter_mut()
                .enumerate()
                .for_each(|(i, box_at)| *box_at = world_box((i, &fragments[i])));
        } else {
            for (i, box_at) in out.boxes.iter_mut().enumerate() {
                *box_at = world_box((i, &fragments[i]));
            }
        }
        for (i, f) in fragments.iter().enumerate() {
            if !asks(i, f) {
                continue;
            }
            let (lo, hi) = out.boxes[i];
            // Where the piece may get to this tick: its box swept by its
            // velocity, and a cell beyond for what gravity adds (ADR-438).
            // A cell beside the one it is in was all it asked about, and a
            // piece moves up to two and a half cells a tick: a fast one
            // fell through a settled piece outside the cells it asked for.
            let v = arr(f.velocity);
            let reach_lo = [0, 1, 2].map(|a| lo[a].add(v[a].min(Fx::ZERO)));
            let reach_hi = [0, 1, 2].map(|a| hi[a].add(v[a].max(Fx::ZERO)));
            let (c0, c1) = settled_cells(reach_lo, reach_hi);
            want.include(c0.map(|v| v - 1), c1.map(|v| v + 1));
        }
        if want.0.is_empty() {
            out.boxes.clear();
            return;
        }
        // Most resting pieces are nowhere near a live hull. Reject them
        // against a conservative box before doing the rotated voxel roof
        // query, without changing which hull can carry a piece.
        let hull_boxes: Vec<_> = hulls.iter().map(Hull::footprint_box).collect();
        let candidate = |(i, f): (usize, &Fragment)| {
            if !f.resting {
                return None;
            }
            let (lo, hi) = out.boxes[i];
            let (c0, c1) = settled_cells(lo, hi);
            let cells = || {
                (c0[2]..=c1[2]).flat_map(move |z| {
                    (c0[1]..=c1[1]).flat_map(move |y| (c0[0]..=c1[0]).map(move |x| [x, y, z]))
                })
            };
            if !cells().any(|c| want.contains(c)) {
                return None;
            }
            // The test asked whether the piece's centre lay inside a
            // hull's box, and the centre of a piece on a roof lies above
            // it: riders held what fell on them (ADR-458). Asked of the
            // pieces the queries reach alone: a hull's voxels answer it
            // (ADR-469).
            if hulls.iter().zip(&hull_boxes).any(|(h, (min, max))| {
                let x = f.position.x;
                let z = f.position.z;
                x >= min[0]
                    && x <= max[0]
                    && z >= min[1]
                    && z <= max[1]
                    && h.carries(lo[1], f.position)
            }) {
                return None;
            }
            Some((c0, c1))
        };
        let selected: Vec<_> = if fragments.len() >= 1024 {
            fragments.par_iter().enumerate().map(candidate).collect()
        } else {
            fragments.iter().enumerate().map(candidate).collect()
        };
        for (i, cells) in selected.into_iter().enumerate() {
            let Some((c0, c1)) = cells else {
                continue;
            };
            for z in c0[2]..=c1[2] {
                for y in c0[1]..=c1[1] {
                    for x in c0[0]..=c1[0] {
                        if want.contains([x, y, z]) {
                            let (cells, free_lists) = (&mut out.cells, &mut out.free_lists);
                            cells
                                .entry([x, y, z])
                                .or_insert_with(|| free_lists.pop().unwrap_or_default())
                                .push(i as u32);
                        }
                    }
                }
            }
        }
    }
    /// Whether the box `lo..hi` shares space with a resting piece other
    /// than `skip`. The pieces are boxes here, as they are to the world.
    fn hit(&self, lo: [Fx; 3], hi: [Fx; 3], skip: u32, from_above: Option<Fx>) -> bool {
        if self.cells.is_empty() {
            return false;
        }
        let (c0, c1) = settled_cells(lo, hi);
        for z in c0[2]..=c1[2] {
            for y in c0[1]..=c1[1] {
                for x in c0[0]..=c1[0] {
                    let Some(list) = self.cells.get(&[x, y, z]) else {
                        continue;
                    };
                    for i in list {
                        if *i == skip {
                            continue;
                        }
                        let (qlo, qhi) = self.boxes[*i as usize];
                        // Support is a top face reached from above, not
                        // an existing overlap. Otherwise two intersecting
                        // pieces can hold each other in mid-air forever.
                        if from_above.is_some_and(|bottom| qhi[1] > bottom) {
                            continue;
                        }
                        if (0..3).all(|a| lo[a] < qhi[a] && qlo[a] < hi[a]) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}
/// Whether a piece's box at `p` meets a live hull (ADR-293): its vertical
/// extent against the hull's height, its horizontal reach against the
/// hull's footprint in the hull's own frame. A turned piece counts its
/// longest horizontal reach in every direction.
fn hull_hit<'h, 's>(
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    hulls: &'h [Hull<'s>],
    skip: u64,
) -> Option<&'h Hull<'s>> {
    hull_box_hit(p, low, high, hulls, skip, true)
}
/// [`hull_hit`] by the hulls' boxes alone, or with their voxels when
/// `exact` (ADR-469).
fn hull_box_hit<'h, 's>(
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    hulls: &'h [Hull<'s>],
    skip: u64,
    exact: bool,
) -> Option<&'h Hull<'s>> {
    if hulls.is_empty() {
        return None;
    }
    let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
    let (bottom, top) = (p[1].add(low[1]), p[1].add(high[1]));
    hulls.iter().enumerate().find_map(|(i, h)| {
        if (i < 64 && skip & (1 << i) != 0) || !hull_box_overlap(p, reach, bottom, top, h) {
            return None;
        }
        (!exact || h.meets_piece(p, low, high)).then_some(h)
    })
}
#[inline]
fn hull_box_overlap(p: [Fx; 3], reach: Fx, bottom: Fx, top: Fx, h: &Hull<'_>) -> bool {
    if top < h.pos[1] || bottom > h.pos[1].add(h.height) {
        return false;
    }
    let dx = p[0].sub(h.pos[0]);
    let dz = p[2].sub(h.pos[2]);
    // A rotated rectangle cannot extend further than the sum of its
    // half-axes on either world axis. Most debris is far from a hull.
    let extent = h.half[0].add(h.half[1]).add(reach);
    if dx.abs() > extent || dz.abs() > extent {
        return false;
    }
    let (c, s) = (angle::cos(h.head), angle::sin(h.head));
    let u = dx.mul(c).add(dz.mul(s));
    let v = dz.mul(c).sub(dx.mul(s));
    u.abs() <= h.half[0].add(reach) && v.abs() <= h.half[1].add(reach)
}
/// Coefficients common to every piece in one fixed tick. Constructing them
/// once avoids thousands of identical Q32.32 integer divisions in a battle.
struct FragmentMotion {
    push: Fx,
    push_up: Fx,
    drag: Fx,
    max_speed: Fx,
    sleep: Fx,
    bounce: Fx,
    friction: Fx,
    spin_rate: Fx,
}
impl FragmentMotion {
    fn new(t: &Blast) -> Self {
        Self {
            push: Fx::from_ratio(t.push_pct as i32, 100),
            push_up: Fx::from_ratio(t.push_up_pct as i32, 100),
            drag: Fx::from_ratio(t.drag_pct as i32, 100),
            max_speed: Fx::from_ratio(
                t.max_speed_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            ),
            sleep: Fx::from_ratio(
                t.sleep_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            ),
            bounce: Fx::from_ratio(t.bounce_pct as i32, 100),
            friction: Fx::from_ratio(t.friction_pct as i32, 100),
            spin_rate: Fx::from_ratio(
                t.spin_deg_s as i32 * (fx::angle::FULL / 360),
                crate::TICKS_PER_SECOND,
            ),
        }
    }
}
/// Common translation/contact step for fragments and persistent dead hulls.
#[allow(clippy::too_many_arguments)]
fn advance_fragment(
    f: &mut Fragment,
    world: &voxel::World,
    t: &Blast,
    motion: &FragmentMotion,
    gravity: Fx,
    hulls: &[Hull],
    settled: &Settled,
    skip: u32,
    spin: bool,
) -> bool {
    if f.resting {
        return false;
    }
    let bounds = [world.nx, world.ny, world.nz];

    f.velocity.y = f.velocity.y.sub(gravity);
    f.velocity = f.velocity.scale(motion.drag);
    let vmax = motion.max_speed;
    if f.velocity.len_sq() > vmax.mul(vmax) {
        f.velocity = f.velocity.scale(vmax.div(f.velocity.len_sq().sqrt()));
    }
    // At most half a voxel per axis substep. Packed occupancy tests
    // cover the full box; the terrain volume has no ceiling.
    let steps = arr(f.velocity)
        .iter()
        .map(|v| v.abs().mul(Fx::from_int(2)).to_int_trunc() as usize + 1)
        .max()
        .unwrap();
    let delta = arr(f.velocity.scale(Fx::from_ratio(1, steps as i32)));
    let mut supported = false;
    let (low, high) = f.bounds();
    // A piece in a live hull's box and under its voxels — born there, from
    // a cell dug under the hull — leaves by the nearest side before it
    // flies (ADR-302, ADR-469): the blast throws it out from under the
    // hull rather than letting it pile up under the belly and lift the
    // hull off the ground. Hulls the piece so leaves, or already meets,
    // do not pen it in (ADR-294): it may leave them, and only entering
    // one is a contact.
    let start = arr(f.position);
    let (mut inside, mut under) = (0u64, 0u64);
    let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
    let (bottom, top) = (start[1].add(low[1]), start[1].add(high[1]));
    for (i, h) in hulls.iter().enumerate().take(64) {
        if !hull_box_overlap(start, reach, bottom, top, h) {
            continue;
        }
        if h.shelters(start[1].add(low[1]), f.position) {
            under |= 1 << i;
            inside |= 1 << i;
        } else if h.meets_piece(start, low, high) {
            inside |= 1 << i;
        }
    }
    for (i, h) in hulls.iter().enumerate().take(64) {
        if under & (1 << i) != 0 {
            evict(f, h, low, high, [bounds[0], bounds[2]]);
        }
    }
    // Most pieces in flight meet nothing in a tick (ADR-465): when the box
    // they sweep through all the substeps is clear of the world, of every
    // hull they do not already overlap and of the settled pieces, no
    // substep is refused, and where they get to is the substeps' sum —
    // the same bits, without asking each substep again.
    let from = arr(f.position);
    let to: [Fx; 3] =
        std::array::from_fn(|a| from[a].add(delta[a].mul(Fx::from_int(steps as i32))));
    let swept_lo: [Fx; 3] = std::array::from_fn(|a| from[a].min(to[a]).add(low[a]));
    let swept_hi: [Fx; 3] = std::array::from_fn(|a| from[a].max(to[a]).add(high[a]));
    let (near, apart) = hull_near(from, to, low, high, hulls, inside);
    if !box_contact(world, [Fx::ZERO; 3], swept_lo, swept_hi, bounds)
        && !near
        && !settled.hit(swept_lo, swept_hi, skip, None)
    {
        f.position = v3(to);
    } else {
        substeps(
            f,
            world,
            motion,
            hulls,
            settled,
            skip,
            steps,
            delta,
            apart,
            &mut supported,
        );
    }
    if spin {
        tumble(f, t, motion);
    }
    let sleep = motion.sleep;
    if supported && f.velocity.len_sq() <= sleep.mul(sleep) {
        f.velocity = V3::ZERO;
        f.resting = true;
        return true;
    }
    false
}

/// Whether a piece moving from `from` to `to` with its box `low..high`
/// may meet a live hull it does not already overlap (`skip`): the hull's
/// height against the swept vertical extent, and the world box around
/// the hull's footprint, widened by the piece's reach and a unit for the
/// rounding of the hull's own frame, against the centre's swept box; and
/// the swept box against the hull's voxels (ADR-469). A bound from
/// outside: [`hull_hit`] decides the contact itself. With it, `skip` and
/// the first 64 hulls that no substep can meet.
fn hull_near(
    from: [Fx; 3],
    to: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    hulls: &[Hull],
    skip: u64,
) -> (bool, u64) {
    let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
    let (bottom, top) = (
        from[1].min(to[1]).add(low[1]),
        from[1].max(to[1]).add(high[1]),
    );
    let exact = by_voxels(low, high);
    let swept_lo: [Fx; 3] = std::array::from_fn(|a| from[a].min(to[a]).add(low[a]));
    let swept_hi: [Fx; 3] = std::array::from_fn(|a| from[a].max(to[a]).add(high[a]));
    let horizontal_lo = [from[0].min(to[0]), from[2].min(to[2])];
    let horizontal_hi = [from[0].max(to[0]), from[2].max(to[2])];
    let (mut near, mut apart) = (false, skip);
    for (i, h) in hulls.iter().enumerate() {
        if i < 64 && skip & (1 << i) != 0 {
            continue;
        }
        // Superset of the rotated footprint on both world axes. The exact
        // rotated and voxel checks below still decide contact.
        let extent = h.half[0].add(h.half[1]).add(reach).add(reach).add(Fx::ONE);
        if top < h.pos[1]
            || bottom > h.pos[1].add(h.height)
            || horizontal_lo[0] > h.pos[0].add(extent)
            || horizontal_hi[0] < h.pos[0].sub(extent)
            || horizontal_lo[1] > h.pos[2].add(extent)
            || horizontal_hi[1] < h.pos[2].sub(extent)
        {
            if i < 64 {
                apart |= 1 << i;
            }
            continue;
        }
        let reaches = top >= h.pos[1] && bottom <= h.pos[1].add(h.height) && {
            let (c, s) = (angle::cos(h.head).abs(), angle::sin(h.head).abs());
            let (a, b) = (h.half[0].add(reach), h.half[1].add(reach));
            let extent = [a.mul(c).add(b.mul(s)), a.mul(s).add(b.mul(c))].map(|e| e.add(Fx::ONE));
            [(0, 0), (2, 1)].iter().all(|&(k, e)| {
                from[k].min(to[k]) <= h.pos[k].add(extent[e])
                    && from[k].max(to[k]) >= h.pos[k].sub(extent[e])
            })
        };
        if reaches && (!exact || h.meets(swept_lo, swept_hi, SWEPT_SAMPLES)) {
            near = true;
        } else if i < 64 {
            apart |= 1 << i;
        }
    }
    (near, apart)
}

/// The substeps of [`advance_fragment`] when the piece may meet
/// something: half a voxel at most per axis, each refused move bouncing
/// its axis and rubbing the others.
#[allow(clippy::too_many_arguments)]
fn substeps(
    f: &mut Fragment,
    world: &voxel::World,
    motion: &FragmentMotion,
    hulls: &[Hull],
    settled: &Settled,
    skip: u32,
    steps: usize,
    mut delta: [Fx; 3],
    inside: u64,
    supported: &mut bool,
) {
    let bounds = [world.nx, world.ny, world.nz];
    let (low, high) = f.bounds();
    for _ in 0..steps {
        for axis in 0..3 {
            if delta[axis] == Fx::ZERO {
                continue;
            }
            let mut p = arr(f.position);
            p[axis] = p[axis].add(delta[axis]);
            let blocked = fragment_contact(f, world, p, low, high, bounds)
                || hull_hit(p, low, high, hulls, inside).is_some()
                || settled.hit(
                    [0, 1, 2].map(|a| p[a].add(low[a])),
                    [0, 1, 2].map(|a| p[a].add(high[a])),
                    skip,
                    (axis == 1 && delta[axis] < Fx::ZERO).then_some(f.position.y.add(low[1])),
                );
            if blocked {
                *supported |= axis == 1 && delta[axis] < Fx::ZERO;
                delta[axis] = Fx::ZERO;
                let mut velocity = arr(f.velocity);
                velocity[axis] = velocity[axis].neg().mul(motion.bounce);
                for (k, value) in velocity.iter_mut().enumerate() {
                    if k != axis {
                        *value = value.mul(motion.friction);
                    }
                }
                f.velocity = v3(velocity);
            } else {
                f.position = v3(p);
            }
        }
    }
}

/// A flying piece tumbles (ADR-309): it turns about one of its own axes,
/// chosen by its number, as fast as it goes. The turn lives in the basis
/// the piece already carries, so nothing is added to what is saved, and
/// a piece that lands keeps the face it landed on. A piece dropped into
/// place with no speed does not turn at all.
fn tumble(f: &mut Fragment, t: &Blast, motion: &FragmentMotion) {
    if t.spin_deg_s == 0 {
        return;
    }
    // The spin comes from the throw, not from the fall: a piece dropped
    // straight down has nothing to turn it, and one that flies sideways or
    // bounces does.
    let speed = f
        .velocity
        .x
        .mul(f.velocity.x)
        .add(f.velocity.z.mul(f.velocity.z))
        .sqrt();
    // Degrees a second for each unit of speed a tick, into binary angle
    // a tick: `deg × FULL / 360 / ticks`.
    let mut rate = speed.mul(motion.spin_rate);
    // A section turns as fast as a piece whose surface moves as fast: the
    // rate of a piece a unit across, over the section's half-width in
    // units (ADR-468). A turret does not spin like a crumb.
    if let Some(chunk) = &f.chunk {
        let (lo, hi) = chunk.bounds;
        let half = (0..3)
            .map(|a| hi[a].sub(lo[a]))
            .max()
            .unwrap_or(Fx::ZERO)
            .div(Fx::from_int(2));
        rate = rate.div(half.max(Fx::ONE));
    }
    let mut rng = fx::rng::Rng::new(f.id, 477);
    rate = rate.mul(Fx::from_ratio(rng.range(t.speed_min_pct as i32, 100), 100));
    let turn = rate.to_int_trunc() * if rng.below(2) == 0 { -1 } else { 1 };
    if turn == 0 {
        return;
    }
    let axis = rng.below(3) as usize;
    let (c, s) = (fx::angle::cos(turn), fx::angle::sin(turn));
    let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
    for row in &mut f.basis {
        let v = arr(*row);
        let mut out = v;
        out[i] = v[i].mul(c).sub(v[j].mul(s));
        out[j] = v[i].mul(s).add(v[j].mul(c));
        *row = v3(out).renormalised();
    }
}
/// Move a piece out of a hull's box by the nearest side, in the hull's
/// own frame, keeping its height (ADR-302). At the map edge the nearest
/// side can lie off the map; the nearest one that keeps the centre on
/// it is taken instead (ADR-428).
fn evict(f: &mut Fragment, h: &Hull, low: [Fx; 3], high: [Fx; 3], columns: [u32; 2]) {
    let (c, s) = (fx::angle::cos(h.head), fx::angle::sin(h.head));
    let p = arr(f.position);
    let (dx, dz) = (p[0].sub(h.pos[0]), p[2].sub(h.pos[2]));
    let u = dx.mul(c).add(dz.mul(s));
    let v = dz.mul(c).sub(dx.mul(s));
    let reach = high[0].max(high[2]).max(low[0].neg()).max(low[2].neg());
    let clear = reach.add(Fx::from_ratio(1, 2));
    let out_u = h.half[0].add(clear);
    let out_v = h.half[1].add(clear);
    // The four sides, nearest first; a tie keeps this order.
    let mut sides = [
        (
            out_u.sub(u.abs()),
            [if u.raw() < 0 { out_u.neg() } else { out_u }, v],
        ),
        (
            out_v.sub(v.abs()),
            [u, if v.raw() < 0 { out_v.neg() } else { out_v }],
        ),
        (
            out_u.add(u.abs()),
            [if u.raw() < 0 { out_u } else { out_u.neg() }, v],
        ),
        (
            out_v.add(v.abs()),
            [u, if v.raw() < 0 { out_v } else { out_v.neg() }],
        ),
    ];
    sides.sort_by_key(|side| side.0);
    let at = |[nu, nv]: [Fx; 2]| {
        [
            h.pos[0].add(nu.mul(c)).sub(nv.mul(s)),
            h.pos[2].add(nu.mul(s)).add(nv.mul(c)),
        ]
    };
    let on_map =
        |q: [Fx; 2]| (0..2).all(|a| q[a] >= Fx::ZERO && q[a] < Fx::from_int(columns[a] as i32));
    let q = sides
        .iter()
        .map(|side| at(side.1))
        .find(|q| on_map(*q))
        .unwrap_or_else(|| at(sides[0].1));
    f.position = V3::new(q[0], p[1], q[1]);
    f.keep_on_map(columns);
}

fn box_contact(
    world: &voxel::World,
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    bounds: [u32; 3],
) -> bool {
    // Floor, including negative fractions at the map boundary.
    let lo: [i32; 3] = std::array::from_fn(|k| (p[k].add(low[k]).raw() >> 32) as i32);
    let hi: [i32; 3] = std::array::from_fn(|k| (p[k].add(high[k]).raw() >> 32) as i32);
    if lo[1] < 0
        || [0, 2]
            .iter()
            .any(|&k| lo[k] < 0 || hi[k] >= bounds[k] as i32)
    {
        return true;
    }
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1].min(bounds[1] as i32 - 1) {
            for x in (lo[0]..=hi[0]).step_by(64) {
                if world.solid_bits(x as u32, y as u32, z as u32, (hi[0] - x + 1).min(64) as u32)
                    != 0
                {
                    return true;
                }
            }
        }
    }
    false
}
fn fragment_contact(
    f: &Fragment,
    world: &voxel::World,
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    bounds: [u32; 3],
) -> bool {
    if !box_contact(world, p, low, high, bounds) {
        return false;
    }
    if f.chunk.is_some() || f.shape.count_ones() == 1 {
        return true;
    }
    // A connected 2³ mask need not be a cuboid. Its empty corners must not
    // collide with material that was deliberately left in the wall.
    let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
    let half = Fx::from_ratio(49, 100).mul(scale);
    let radius: [Fx; 3] = std::array::from_fn(|a| {
        arr(f.basis[0])[a]
            .abs()
            .add(arr(f.basis[1])[a].abs())
            .add(arr(f.basis[2])[a].abs())
            .mul(half)
    });
    let centre = arr(shape_centre(f.shape, f.side));
    for i in 0..box_cells(f.side) {
        if f.shape & (1 << i) != 0 {
            let q = shape_cell(i, f.side);
            let mut offset = V3::ZERO;
            for a in 0..3 {
                offset = offset.add(f.basis[a].scale(Fx::from_int(q[a]).sub(centre[a]).mul(scale)));
            }
            let at = arr(v3(p).add(offset));
            if box_contact(world, at, radius.map(Fx::neg), radius, bounds) {
                return true;
            }
        }
    }
    false
}
