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
