//! Each body voxel belongs to the wave cell containing its world-space centre.
//! Sampling world-grid centres into a rotated body leaves periodic columns
//! untouched. Iterate a conservative local box instead, then test ownership.
use super::{HitFrames, RangeUnit};
use crate::damage::Frame;
use fx::Fx;
use phys::V3;
use voxel::UnitBody;

pub(super) type Contact = (usize, [usize; 3]);

fn cells(
    body: &UnitBody,
    frame: &Frame,
    layers: [usize; 2],
    low: [i32; 3],
    edge: i32,
    part: usize,
    visit: &mut impl FnMut(Contact) -> bool,
) -> bool {
    let high = low.map(|v| v + edge);
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
    for z in lo[2]..hi[2] {
        for y in lo[1]..hi[1] {
            for x in lo[0]..hi[0] {
                let p = [x, y, z];
                let local = [0, 1, 2].map(|a| {
                    Fx::from_int(p[a])
                        .add(Fx::from_ratio(1, 2))
                        .sub(frame.centre[a])
                });
                let v = if frame.scale == Fx::ONE {
                    local
                } else {
                    local.map(|v| v.div(frame.scale))
                };
                let w = frame
                    .base
                    .add(frame.x.scale(v[0]))
                    .add(frame.y.scale(v[1]))
                    .add(frame.z.scale(v[2]));
                if [w.x, w.y, w.z]
                    .into_iter()
                    .enumerate()
                    .all(|(a, v)| v >= Fx::from_int(low[a]) && v < Fx::from_int(high[a]))
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
        mut visit: impl FnMut(Contact) -> bool,
    ) {
        let body = &unit.body;
        let split = usize::from(body.turret_y);
        let end = usize::from(body.vol.ny);
        if self.shared_root {
            if !cells(body, root, [0, end], low, edge, 0, &mut visit) {
                return;
            }
        } else {
            if !cells(body, root, [0, split], low, edge, 0, &mut visit) {
                return;
            }
            if !cells(body, &self.turret, [split, end], low, edge, 0, &mut visit) {
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
                    &mut visit,
                )
            {
                return;
            }
        }
    }
}
