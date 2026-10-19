//! Bounded spatial partition and exact six-connected metal fragments.
//! Callers supply one owner, material, articulated frame and subsystem.
use super::{chunk, shape_centre, Fragment, IDENTITY};
use crate::damage::Frame;
use fx::Fx;
use std::collections::BTreeMap;
use voxel::impact::Cell;

/// Align large boxes with every legacy box. Regrouping can join reserved
/// components, but must never split one and require an unreserved pool slot.
pub(crate) fn span_cells(mm: u16, span_mm: u32, quantum: u8) -> u32 {
    let quantum = u32::from(quantum.clamp(1, 4));
    (span_mm / u32::from(mm).max(1) / quantum)
        .max(1)
        .min(256 / quantum)
        * quantum
}

pub(crate) fn components(
    cells: impl IntoIterator<Item = (Cell, usize)>,
    span: u32,
) -> Vec<Vec<(Cell, usize)>> {
    let mut boxes = BTreeMap::<[u32; 3], Vec<(Cell, usize)>>::new();
    for (cell, index) in cells {
        boxes
            .entry(cell.map(|v| u32::from(v) / span))
            .or_default()
            .push((cell, index));
    }
    let mut out = Vec::new();
    for (_, mut cells) in boxes {
        // Same lexicographic seed order as the former inner BTreeMap.
        cells.sort_unstable_by_key(|&(cell, _)| cell);
        assert!(cells.windows(2).all(|p| p[0].0 != p[1].0));
        let lo: Cell = std::array::from_fn(|a| cells.iter().map(|c| c.0[a]).min().unwrap());
        let hi: Cell = std::array::from_fn(|a| cells.iter().map(|c| c.0[a]).max().unwrap());
        let size: [usize; 3] = std::array::from_fn(|a| usize::from(hi[a]) + 1 - usize::from(lo[a]));
        let volume = size.iter().product::<usize>();
        // Scratch is bounded to 512 KiB and at most eight slots per input.
        // Sparse/large domains retain the tree; no 256^3 table for two cells.
        if volume <= 65_536 && volume <= cells.len() * 8 {
            let strides = [size[1] * size[2], size[2], 1];
            let offset = |c: Cell| {
                (0..3)
                    .map(|a| usize::from(c[a] - lo[a]) * strides[a])
                    .sum::<usize>()
            };
            let mut left = vec![0usize; volume];
            for (i, &(cell, _)) in cells.iter().enumerate() {
                left[offset(cell)] = i + 1;
            }
            for &seed in &cells {
                let slot = offset(seed.0);
                if left[slot] == 0 {
                    continue;
                }
                left[slot] = 0;
                let mut piece = vec![seed];
                let mut cursor = 0;
                while cursor < piece.len() {
                    let cell = piece[cursor].0;
                    cursor += 1;
                    let slot = offset(cell);
                    for axis in 0..3 {
                        // Preserve the original X-/X+/Y-/Y+/Z-/Z+ walk.
                        for next in [
                            (cell[axis] > lo[axis]).then(|| slot - strides[axis]),
                            (cell[axis] < hi[axis]).then(|| slot + strides[axis]),
                        ]
                        .into_iter()
                        .flatten()
                        {
                            let index = std::mem::take(&mut left[next]);
                            if index != 0 {
                                piece.push(cells[index - 1]);
                            }
                        }
                    }
                }
                piece.sort_unstable_by_key(|&(_, index)| index);
                out.push(piece);
            }
        } else {
            let mut left: BTreeMap<_, _> = cells.into_iter().collect();
            while let Some(seed) = left.pop_first() {
                let mut piece = vec![seed];
                let mut cursor = 0;
                while cursor < piece.len() {
                    let cell = piece[cursor].0;
                    cursor += 1;
                    for axis in 0..3 {
                        for neighbour in [cell[axis].checked_sub(1), cell[axis].checked_add(1)]
                            .into_iter()
                            .flatten()
                        {
                            let mut next = cell;
                            next[axis] = neighbour;
                            if let Some(index) = left.remove(&next) {
                                piece.push((next, index));
                            }
                        }
                    }
                }
                piece.sort_unstable_by_key(|&(_, index)| index);
                out.push(piece);
            }
        }
    }
    out
}

/// Compact masks stay cheap; larger connected panels reuse the existing
/// exact section geometry, rendering and snapshot codec.
pub(crate) fn set_geometry(
    f: &mut Fragment,
    template: u16,
    segment: Option<u8>,
    frame: Frame,
    cells: Vec<Cell>,
    turret_y: u8,
    preserve_finishes: bool,
) {
    let lo: [u8; 3] = std::array::from_fn(|a| cells.iter().map(|c| c[a]).min().unwrap());
    let hi: [u8; 3] = std::array::from_fn(|a| cells.iter().map(|c| c[a]).max().unwrap());
    let side = (0..3)
        .map(|a| u32::from(hi[a]) + 1 - u32::from(lo[a]))
        .max()
        .unwrap();
    f.count = cells.len() as u32;
    if side <= 4 && !preserve_finishes {
        let mut shape = 0;
        for c in cells {
            let p = std::array::from_fn::<_, 3, _>(|a| u32::from(c[a] - lo[a]));
            shape |= 1u64 << (p[0] + side * p[1] + side * side * p[2]);
        }
        let centre = shape_centre(shape, side as u8);
        let local = [centre.x, centre.y, centre.z];
        f.position = frame.world(std::array::from_fn(|a| {
            Fx::from_int(i32::from(lo[a]))
                .add(Fx::from_ratio(1, 2))
                .add(local[a])
        }));
        f.shape = shape;
        f.side = side as u8;
        f.basis = [frame.x, frame.y, frame.z];
    } else {
        let mut shape = chunk::Shape::default();
        for &cell in &cells {
            shape.add(cell, turret_y);
        }
        let mut chunk = chunk::Chunk::new(template, f.paint.unwrap()[0], cells, [frame; 2], shape);
        if let Some(segment) = segment {
            chunk = chunk.in_segment(segment);
        }
        f.position = chunk.centre;
        f.basis = IDENTITY;
        f.shape = 1;
        f.side = 2;
        f.chunk = Some(Box::new(chunk));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn oracle(
        cells: impl IntoIterator<Item = (Cell, usize)>,
        span: u32,
    ) -> Vec<Vec<(Cell, usize)>> {
        let mut boxes = BTreeMap::<[u32; 3], BTreeMap<Cell, usize>>::new();
        for (cell, index) in cells {
            let tile = cell.map(|v| u32::from(v) / span);
            assert!(boxes.entry(tile).or_default().insert(cell, index).is_none());
        }
        let mut out = Vec::new();
        for (_, mut left) in boxes {
            while let Some(seed) = left.pop_first() {
                let mut piece = vec![seed];
                let mut cursor = 0;
                while cursor < piece.len() {
                    let cell = piece[cursor].0;
                    cursor += 1;
                    for axis in 0..3 {
                        for neighbour in [cell[axis].checked_sub(1), cell[axis].checked_add(1)]
                            .into_iter()
                            .flatten()
                        {
                            let mut next = cell;
                            next[axis] = neighbour;
                            if let Some(index) = left.remove(&next) {
                                piece.push((next, index));
                            }
                        }
                    }
                }
                // Stable donor order also fixes rounding and surviving IDs.
                piece.sort_unstable_by_key(|(_, index)| *index);
                out.push(piece);
            }
        }
        out
    }

    #[test]
    fn adaptive_components_match_tree_including_piece_and_donor_order() {
        use fx::rng::Rng;
        for span in [1, 2, 3, 4, 9, 16, 32, 64, 128, 256] {
            for shape in 0..6 {
                let mut cells = Vec::new();
                let mut rng = Rng::new(span as u64, shape);
                for x in 0..32u8 {
                    for y in 0..16u8 {
                        for z in 0..32u8 {
                            let keep = match shape {
                                0 => true,
                                1 => y == 0 || x == 0,
                                2 => (x / 4 + y / 4 + z / 4) % 2 == 0,
                                3 => rng.below(3) != 0,
                                4 => rng.below(23) == 0,
                                _ => x % 2 == 0 && y % 2 == 0 && z % 2 == 0,
                            };
                            if keep {
                                cells.push(([x, y, z], cells.len() * 3 + 7));
                            }
                        }
                    }
                }
                // u8 borders, sparse large boxes and disconnected seeds.
                cells.extend([
                    ([255, 255, 255], usize::MAX),
                    ([254, 255, 255], 1),
                    ([0, 255, 0], 3),
                ]);
                for order in 0..3 {
                    if order == 1 {
                        cells.reverse();
                    }
                    if order == 2 {
                        cells.rotate_left(11);
                    }
                    assert_eq!(
                        components(cells.clone(), span),
                        oracle(cells.clone(), span),
                        "span={span}, shape={shape}, order={order}"
                    );
                }
            }
        }
        assert_eq!(components([], 16), oracle([], 16));
        assert_eq!(
            components([([255; 3], 19)], 256),
            oracle([([255; 3], 19)], 256)
        );
    }

    #[test]
    #[should_panic]
    fn adaptive_components_reject_duplicate_cells() {
        components([([1, 2, 3], 0), ([1, 2, 3], 7)], 16);
    }

    #[test]
    #[ignore = "manual exact body grouping A/B; run alone in release"]
    fn measure_adaptive_components() {
        use std::{hint::black_box, time::Instant};
        for (edge, sparse, span) in [
            (1, false, 16),
            (2, false, 16),
            (4, false, 16),
            (8, false, 16),
            (16, false, 16),
            (32, false, 16),
            (32, true, 256),
        ] {
            let mut cells = Vec::new();
            for x in 0..edge {
                for y in 0..edge {
                    for z in 0..edge {
                        if !sparse || (u32::from(x) + u32::from(y) * 3 + u32::from(z) * 7) % 13 == 0
                        {
                            cells.push(([x, y, z], cells.len()));
                        }
                    }
                }
            }
            let expected = oracle(cells.clone(), span);
            for fast in [false, true] {
                let mut times = Vec::new();
                for rep in 0..8 {
                    let input = cells.clone();
                    let start = Instant::now();
                    let actual = if fast {
                        components(input, span)
                    } else {
                        oracle(input, span)
                    };
                    let us = start.elapsed().as_nanos();
                    if rep > 0 {
                        times.push(us);
                    }
                    assert_eq!(actual, expected);
                    black_box(actual);
                }
                times.sort_unstable();
                println!("BODY_GROUP edge={edge} sparse={sparse} span={span} cells={} fast={fast} median_ns={} max_ns={}",cells.len(),times[3],times[6]);
            }
        }
    }

    #[test]
    fn panels_keep_holes_and_only_face_connected_matter_within_the_span() {
        let mut cells = Vec::new();
        for z in 0..16 {
            for x in 0..32 {
                if !(6..10).contains(&x) || !(6..10).contains(&z) {
                    cells.push([x, 3, z]);
                }
            }
        }
        cells.extend([[0, 4, 16], [255, 255, 255]]);
        let pieces = components(cells.iter().copied().enumerate().map(|(i, c)| (c, i)), 16);
        assert_eq!(pieces.len(), 4);
        assert_eq!(pieces.iter().map(Vec::len).sum::<usize>(), cells.len());
        assert_eq!(pieces[0].len(), 240);
        assert_eq!(pieces[1].len(), 1);
        assert_eq!(pieces[2].len(), 256);
        let mut restored: Vec<_> = pieces.into_iter().flatten().map(|(c, _)| c).collect();
        restored.sort_unstable();
        cells.sort_unstable();
        assert_eq!(restored, cells);
    }
    #[test]
    fn large_partitions_never_split_a_reserved_legacy_box_at_any_scale() {
        for mm in [50, 100, 250, 400, 2000] {
            for quantum in 1..=4 {
                let span = span_cells(mm, 1600, quantum);
                assert_eq!(span % u32::from(quantum), 0);
                for p in 0..256u32 {
                    let origin = p / u32::from(quantum) * u32::from(quantum);
                    assert_eq!(p / span, origin / span);
                }
            }
        }
    }
}
