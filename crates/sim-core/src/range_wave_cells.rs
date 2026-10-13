//! Each body voxel belongs to the wave cell containing its world-space centre.
//! Sampling world-grid centres into a rotated body leaves periodic columns
//! untouched. Iterate a conservative local box instead, then test ownership.
use super::{HitFrames, RangeUnit};
use crate::damage::Frame;
use fx::Fx;
use phys::V3;
use voxel::UnitBody;

pub(super) type Contact = (usize, [usize; 3]);

/// The conservative box of local cells `cells` walks for the wave cell at
/// `low`: the image of the world cube under the inverse frame, clipped to
/// the body's layers.
fn candidate_box(
    body: &UnitBody,
    frame: &Frame,
    layers: [usize; 2],
    low: [i32; 3],
    edge: i32,
) -> ([i32; 3], [i32; 3]) {
    let half = Fx::from_ratio(edge, 2);
    let p = low.map(|v| Fx::from_int(v).add(half));
    let centre = frame.local(V3::new(p[0], p[1], p[2]));
    // Image of the world cube's half extents under the inverse rotation.
    // This is the same conservative box as eight corner transforms. The
    // tiny fixed-point pad covers operation-order rounding at boundaries.
    let radius = [frame.x, frame.y, frame.z].map(|v| {
        v.x.abs()
            .add(v.y.abs())
            .add(v.z.abs())
            .mul(half)
            .mul(frame.scale)
            .add(Fx(64))
    });
    let mut lo = [0, 1, 2].map(|a| (centre[a].sub(radius[a]).raw() >> 32) as i32);
    let mut hi = [0, 1, 2].map(|a| (centre[a].add(radius[a]).raw() >> 32) as i32 + 1);
    let min = [0, layers[0] as i32, 0];
    let max = [body.vol.nx as i32, layers[1] as i32, body.vol.nz as i32];
    for a in 0..3 {
        lo[a] = lo[a].max(min[a]);
        hi[a] = hi[a].min(max[a]);
    }
    (lo, hi)
}

/// The world term of local axis `a` for the local cells `from..to`: a
/// voxel's world centre is the frame's base plus one term per local axis,
/// each a function of that axis's coordinate alone. The sum is wrapping,
/// so adding tabled terms in any grouping gives the same bits as the
/// transform term by term.
fn axis_terms(frame: &Frame, a: usize, from: i32, to: i32) -> Vec<V3> {
    let axis = [frame.x, frame.y, frame.z][a];
    (from..to)
        .map(|c| {
            let local = Fx::from_int(c)
                .add(Fx::from_ratio(1, 2))
                .sub(frame.centre[a]);
            let v = if frame.scale == Fx::ONE {
                local
            } else {
                local.div(frame.scale)
            };
            axis.scale(v)
        })
        .collect()
}

/// Exact transformed axis terms, shared by all cells of one capture slice.
/// These are geometry only: live occupancy is still read by the visitor.
pub(super) struct WaveTerms {
    root: [Vec<V3>; 3],
    turret: Option<[Vec<V3>; 3]>,
    segments: Vec<[Vec<V3>; 3]>,
}

fn body_terms(body: &UnitBody, frame: &Frame) -> [Vec<V3>; 3] {
    let size = [body.vol.nx, body.vol.ny, body.vol.nz];
    std::array::from_fn(|a| axis_terms(frame, a, 0, i32::from(size[a])))
}

#[allow(clippy::too_many_arguments)]
fn cells(
    body: &UnitBody,
    frame: &Frame,
    layers: [usize; 2],
    low: [i32; 3],
    edge: i32,
    part: usize,
    terms: Option<&[Vec<V3>; 3]>,
    visit: &mut impl FnMut(Contact) -> bool,
) -> bool {
    let high = low.map(|v| v + edge);
    let (lo, hi) = candidate_box(body, frame, layers, low, edge);
    if (0..3).any(|a| lo[a] >= hi[a]) {
        return true;
    }
    let owned = terms
        .is_none()
        .then(|| std::array::from_fn::<_, 3, _>(|a| axis_terms(frame, a, lo[a], hi[a])));
    let sliced: [&[V3]; 3] = std::array::from_fn(|a| match terms {
        Some(terms) => &terms[a][lo[a] as usize..hi[a] as usize],
        None => &owned.as_ref().unwrap()[a],
    });
    let [tx, ty, tz] = sliced;
    let (wlo, whi) = (low.map(Fx::from_int), high.map(Fx::from_int));
    for (z, dz) in (lo[2]..hi[2]).zip(tz) {
        let plane = frame.base.add(*dz);
        for (y, dy) in (lo[1]..hi[1]).zip(ty) {
            let row = plane.add(*dy);
            for (x, dx) in (lo[0]..hi[0]).zip(tx) {
                let p = [x, y, z];
                let w = row.add(*dx);
                if w.x >= wlo[0]
                    && w.x < whi[0]
                    && w.y >= wlo[1]
                    && w.y < whi[1]
                    && w.z >= wlo[2]
                    && w.z < whi[2]
                {
                    // Include empty/gone cells too: the cursor must remain
                    // stable when material claims remove cells between ticks.
                    if !visit((part, p.map(|v| v as usize))) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

impl HitFrames {
    pub(super) fn wave_terms(
        &self,
        unit: &RangeUnit,
        root: &Frame,
        segments: &[Frame],
    ) -> WaveTerms {
        WaveTerms {
            root: body_terms(&unit.body, root),
            turret: (!self.shared_root).then(|| body_terms(&unit.body, &self.turret)),
            segments: unit
                .body
                .segments
                .iter()
                .zip(segments)
                .map(|(segment, frame)| body_terms(&segment.body, frame))
                .collect(),
        }
    }

    pub(super) fn wave_cells(
        &self,
        unit: &RangeUnit,
        root: &Frame,
        segments: &[Frame],
        low: [i32; 3],
        edge: i32,
        out: &mut Vec<Contact>,
    ) {
        out.clear();
        self.visit_wave_cells(unit, root, segments, low, edge, |cell| {
            out.push(cell);
            true
        });
    }

    pub(super) fn visit_wave_cells(
        &self,
        unit: &RangeUnit,
        root: &Frame,
        segments: &[Frame],
        low: [i32; 3],
        edge: i32,
        visit: impl FnMut(Contact) -> bool,
    ) {
        self.visit_wave_cells_with_terms(unit, root, segments, low, edge, None, visit);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit_wave_cells_with_terms(
        &self,
        unit: &RangeUnit,
        root: &Frame,
        segments: &[Frame],
        low: [i32; 3],
        edge: i32,
        terms: Option<&WaveTerms>,
        mut visit: impl FnMut(Contact) -> bool,
    ) {
        let body = &unit.body;
        let split = usize::from(body.turret_y);
        let end = usize::from(body.vol.ny);
        if self.shared_root {
            if !cells(
                body,
                root,
                [0, end],
                low,
                edge,
                0,
                terms.map(|t| &t.root),
                &mut visit,
            ) {
                return;
            }
        } else {
            if !cells(
                body,
                root,
                [0, split],
                low,
                edge,
                0,
                terms.map(|t| &t.root),
                &mut visit,
            ) {
                return;
            }
            if !cells(
                body,
                &self.turret,
                [split, end],
                low,
                edge,
                0,
                terms.and_then(|t| t.turret.as_ref()),
                &mut visit,
            ) {
                return;
            }
        }
        for (i, (segment, frame)) in body.segments.iter().zip(segments).enumerate() {
            let (lo, hi) = self.boxes[i];
            if (0..3).all(|a| low[a] <= hi[a] && low[a] + edge > lo[a])
                && !cells(
                    &segment.body,
                    frame,
                    [0, usize::from(segment.body.vol.ny)],
                    low,
                    edge,
                    i + 1,
                    terms.map(|t| &t.segments[i]),
                    &mut visit,
                )
            {
                return;
            }
        }
    }
}

/// Every cell of one hull by the wave cell that owns it, as `wave_cells`
/// lists them, for a whole field at once (ADR-681): each body cell's world
/// centre is found once, instead of once per wave cell whose conservative
/// box reaches it. Entries are (wave cell index, packed contact), sorted
/// by wave cell and, within one, in `wave_cells` order. Valid for the
/// frozen poses and the body sizes it was built at.
///
/// A packed contact is its layer (0 the root below the turret, 1 the
/// turret, 2 + k segment k), then z, y and x: within one wave cell the
/// walk order of `wave_cells` is the packed order, so one unstable sort of
/// (cell, contact) keys gives it.
pub(super) struct BodyMap {
    pub(super) sizes: Vec<u16>,
    entries: Vec<u64>,
}

impl BodyMap {
    /// The entries of wave cell `index`, as a range of `entries`.
    pub(super) fn range(&self, index: u32) -> (usize, usize) {
        let index = u64::from(index);
        let start = self.entries.partition_point(|e| e >> 32 < index);
        let end = self.entries.partition_point(|e| e >> 32 <= index);
        (start, end - start)
    }
    pub(super) fn contact(&self, k: usize) -> Contact {
        let v = self.entries[k] as u32;
        let layer = (v >> 24) as usize;
        (
            layer.saturating_sub(1),
            [
                v as usize & 0xff,
                (v >> 8) as usize & 0xff,
                (v >> 16) as usize & 0xff,
            ],
        )
    }
}

/// A field's grid: the corner of its first cell, its cell edge and cells
/// per side.
#[derive(Clone, Copy)]
pub(super) struct Grid {
    pub(super) origin: [i32; 3],
    pub(super) edge: i32,
    pub(super) n: usize,
}

/// An owning wave cell, its conservative box of local cells, and whether
/// the part may meet it at all.
type OwnerBox = ([i32; 3], [i32; 3], [i32; 3], bool);

impl Grid {
    pub(super) fn index(&self, cell: [i32; 3]) -> u32 {
        ((cell[2] as usize * self.n + cell[1] as usize) * self.n + cell[0] as usize) as u32
    }
}

/// Bucket's input is already in packed-contact order (layer, z, y, x).
/// Stable scatter by owning cell therefore gives the same total order as
/// sorting the full key. Restrict the histogram to dense, bounded spans;
/// unusual input orders and sparse fields retain the comparison sort.
fn order_entries(entries: &mut Vec<u64>) {
    if entries.len() < 1024 {
        entries.sort_unstable();
        return;
    }
    let (mut first, mut last) = (u32::MAX, 0);
    let mut previous = 0;
    for &entry in entries.iter() {
        let packed = entry as u32;
        if packed < previous {
            entries.sort_unstable();
            return;
        }
        previous = packed;
        let cell = (entry >> 32) as u32;
        first = first.min(cell);
        last = last.max(cell);
    }
    let span = u64::from(last) - u64::from(first) + 1;
    if span > entries.len() as u64 {
        entries.sort_unstable();
        return;
    }
    let key = |entry: u64| ((entry >> 32) as u32 - first) as usize;
    let mut positions = vec![0usize; span as usize];
    for &entry in entries.iter() {
        positions[key(entry)] += 1;
    }
    let mut offset = 0;
    for position in &mut positions {
        let count = *position;
        *position = offset;
        offset += count;
    }
    let mut ordered = vec![0; entries.len()];
    for &entry in entries.iter() {
        let position = &mut positions[key(entry)];
        ordered[*position] = entry;
        *position += 1;
    }
    *entries = ordered;
}

/// Append the cells of one layer of one part, each under the wave cell
/// owning it, if that cell's conservative box reaches it (as `cells`
/// requires) and `reaches` admits the cell.
#[allow(clippy::too_many_arguments)]
fn bucket(
    body: &UnitBody,
    frame: &Frame,
    layers: [usize; 2],
    layer: usize,
    grid: Grid,
    reaches: impl Fn([i32; 3]) -> bool,
    out: &mut Vec<u64>,
) {
    let min = [0, layers[0] as i32, 0];
    let max = [body.vol.nx as i32, layers[1] as i32, body.vol.nz as i32];
    if (0..3).any(|a| min[a] >= max[a]) {
        return;
    }
    let term = |a: usize| axis_terms(frame, a, min[a], max[a]);
    let (tx, ty, tz) = (term(0), term(1), term(2));
    let span = i64::from(grid.edge) << 32;
    // Signed arithmetic shift is Euclidean division by a positive power
    // of two, including negative positions and exact cell boundaries.
    let shift = (grid.edge > 0 && (grid.edge as u32).is_power_of_two())
        .then(|| 32 + grid.edge.trailing_zeros());
    let origin = grid.origin.map(|v| i64::from(v) << 32);
    // The box of the last owning cell asked: neighbours share it.
    let mut last: Option<OwnerBox> = None;
    for (z, dz) in (min[2]..max[2]).zip(&tz) {
        let plane = frame.base.add(*dz);
        for (y, dy) in (min[1]..max[1]).zip(&ty) {
            let row = plane.add(*dy);
            for (x, dx) in (min[0]..max[0]).zip(&tx) {
                let w = row.add(*dx);
                let w = [w.x.raw(), w.y.raw(), w.z.raw()];
                let cell: [i32; 3] = std::array::from_fn(|a| {
                    let delta = w[a] - origin[a];
                    match shift {
                        Some(shift) => (delta >> shift) as i32,
                        None => delta.div_euclid(span) as i32,
                    }
                });
                if cell.iter().any(|&c| c < 0 || c as usize >= grid.n) {
                    continue;
                }
                let (lo, hi, admitted) = match last {
                    Some((c, lo, hi, admitted)) if c == cell => (lo, hi, admitted),
                    _ => {
                        let low: [i32; 3] =
                            std::array::from_fn(|a| grid.origin[a] + cell[a] * grid.edge);
                        let (lo, hi) = candidate_box(body, frame, layers, low, grid.edge);
                        let admitted = reaches(low);
                        last = Some((cell, lo, hi, admitted));
                        (lo, hi, admitted)
                    }
                };
                let p = [x, y, z];
                if admitted && (0..3).all(|a| p[a] >= lo[a] && p[a] < hi[a]) {
                    let packed =
                        (layer as u32) << 24 | (z as u32) << 16 | (y as u32) << 8 | x as u32;
                    out.push(u64::from(grid.index(cell)) << 32 | u64::from(packed));
                }
            }
        }
    }
}

impl HitFrames {
    /// The map of `unit`'s cells over the field `grid` (see [`BodyMap`]);
    /// `None` for a hull with more parts than a packed contact holds.
    pub(super) fn body_map(
        &self,
        unit: &RangeUnit,
        root: &Frame,
        segments: &[Frame],
        grid: Grid,
        sizes: Vec<u16>,
    ) -> Option<BodyMap> {
        let body = &unit.body;
        if body.segments.len() >= 254 {
            return None;
        }
        let split = usize::from(body.turret_y);
        let end = usize::from(body.vol.ny);
        let mut entries = Vec::new();
        let any = |_: [i32; 3]| true;
        if self.shared_root {
            bucket(body, root, [0, end], 0, grid, any, &mut entries);
        } else {
            bucket(body, root, [0, split], 0, grid, any, &mut entries);
            bucket(body, &self.turret, [split, end], 1, grid, any, &mut entries);
        }
        for (i, (segment, frame)) in body.segments.iter().zip(segments).enumerate() {
            let (lo, hi) = self.boxes[i];
            let edge = grid.edge;
            bucket(
                &segment.body,
                frame,
                [0, usize::from(segment.body.vol.ny)],
                i + 2,
                grid,
                |low| (0..3).all(|a| low[a] <= hi[a] && low[a] + edge > lo[a]),
                &mut entries,
            );
        }
        // Distinct keys; within a wave cell, packed order is walk order.
        order_entries(&mut entries);
        Some(BodyMap { sizes, entries })
    }
}

#[cfg(test)]
mod tests {
    use super::order_entries;

    #[test]
    fn contact_order_matches_full_sort_for_dense_sparse_and_unusual_inputs() {
        for count in [0, 1, 1023, 1024, 8192] {
            for (base, stride, reversed) in [
                (0u32, 1u32, false),
                (u32::MAX - 100, 1, false),
                (0, 10000, false),
                (0, 1, true),
            ] {
                let mut entries: Vec<_> = (0..count)
                    .map(|i| {
                        let cell = base + ((i * 97) % 100) as u32 * stride;
                        let packed = if reversed { count - i } else { i } as u32 / 2;
                        u64::from(cell) << 32 | u64::from(packed)
                    })
                    .collect();
                let mut expected = entries.clone();
                expected.sort_unstable();
                order_entries(&mut entries);
                assert_eq!(entries, expected, "{count} {base} {stride} {reversed}");
            }
        }
    }
}
