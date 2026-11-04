//! Read-only resistance capture; ordered slice budgets precede worker jobs.
use super::{
    material_cached, terrain_candidates, terrain_work, Explosion, HitFrames, RangeUnit, SurfaceWork,
};
use modding::explosion::ExplosionConfig;
use rayon::prelude::*;
use voxel::World;

/// Refine only regions whose conservative material bound can beat the best
/// live witness. Large wave cells stay coarse over homogeneous terrain; thin
/// walls, edited material and cavities still reach the exact column reader.
fn terrain_peak(
    w: &World,
    lo: [u32; 3],
    hi: [u32; 3],
    plane: i32,
    c: &ExplosionConfig,
    best: &mut u32,
    cursor: &mut voxel::world::MatCursor,
) {
    let top = hi[1].min(w.column_box_ceiling([lo[0], lo[2]], [hi[0], hi[2]]));
    if top <= lo[1] {
        return;
    }
    let hi = [hi[0], top, hi[2]];
    let bound = w.material_candidates(lo, hi).map(|ids| {
        ids.into_iter()
            .map(|m| terrain_work(m, lo[1] as i32, plane, c))
            .max()
            .unwrap_or(0)
    });
    if bound.is_some_and(|v| v <= *best) {
        return;
    }
    if let Some(mat) = w.uniform_box(lo, hi) {
        *best = (*best).max(terrain_work(mat, lo[1] as i32, plane, c));
        return;
    }
    // Both the deepest layer and the visible surface can attain the bound.
    for z in [lo[2], (lo[2] + hi[2] - 1) / 2, hi[2] - 1] {
        for x in [lo[0], (lo[0] + hi[0] - 1) / 2, hi[0] - 1] {
            let end = top.min(w.column_ceiling(x, z));
            if end <= lo[1] {
                continue;
            }
            for y in [lo[1], end - 1] {
                let mat = w.material_cached(cursor, x, y, z);
                *best = (*best).max(terrain_work(mat, y as i32, plane, c));
            }
            if bound.is_some_and(|v| v <= *best) {
                return;
            }
        }
    }
    let axis = if hi[0] - lo[0] >= hi[2] - lo[2] { 0 } else { 2 };
    if hi[axis] - lo[axis] > 8 {
        let mid = (lo[axis] + hi[axis]) / 2;
        let mut end = hi;
        end[axis] = mid;
        terrain_peak(w, lo, end, plane, c, best, cursor);
        if !bound.is_some_and(|v| v <= *best) {
            let mut start = lo;
            start[axis] = mid;
            terrain_peak(w, start, hi, plane, c, best, cursor);
        }
        return;
    }
    let mut column = [0; 64];
    for z in lo[2]..hi[2] {
        for x in lo[0]..hi[0] {
            let end = top.min(w.column_ceiling(x, z));
            if end <= lo[1] {
                continue;
            }
            let count = (end - lo[1]) as usize;
            w.column(x, z, lo[1], &mut column[..count]);
            for (dy, &mat) in column[..count].iter().enumerate() {
                *best = (*best).max(terrain_work(mat, lo[1] as i32 + dy as i32, plane, c));
                if bound.is_some_and(|v| v <= *best) {
                    return;
                }
            }
        }
    }
}

/// The cell index a capture slice of `budget` ends at: cells outside the
/// inscribed sphere are free, but even those wait once the quota is spent.
/// Also how many of its cells the sphere holds.
fn slice_end(e: &Explosion, budget: usize) -> (usize, usize) {
    let n = e.wave.size();
    let half = n / 2;
    let radius2 = (half * half) as i32;
    let mut next = e.captured;
    let mut spent = 0;
    while next < n * n * n && spent < budget {
        let q = [next % n, (next / n) % n, next / (n * n)].map(|v| v as i32 - half as i32);
        spent += usize::from(q.iter().map(|v| v * v).sum::<i32>() <= radius2);
        next += 1;
    }
    (next, spent)
}

pub(super) fn capture(
    e: &mut Explosion,
    w: &World,
    c: &ExplosionConfig,
    units: &[RangeUnit],
    dents: &SurfaceWork,
    budget: usize,
    parallel: bool,
) -> bool {
    capture_inner::<true>(e, w, c, units, dents, budget, parallel, true)
}

// The previous ground reader is instantiated only by the test probe.
fn capture_inner<const COLUMNS: bool>(
    e: &mut Explosion,
    w: &World,
    c: &ExplosionConfig,
    units: &[RangeUnit],
    dents: &SurfaceWork,
    budget: usize,
    parallel: bool,
    split_short_slices: bool,
) -> bool {
    let n = e.wave.size();
    let cells = n * n * n;
    if e.captured >= cells {
        return true;
    }
    // A queued wave can legitimately get no share on this tick. It must
    // wait without rebuilding body frames or allocating a domain-sized map.
    if budget == 0 {
        return false;
    }
    e.prepare_soil_plane(w, c);
    // Fix the logical slice before building its immutable geometry cache.
    let (next, spent) = slice_end(e, budget);
    let half = n / 2;
    let origin = e.origin;
    let radius2 = (half * half) as i32;
    let far = [0, 1, 2].map(|a| origin[a] + n as i32 * e.cell - 1);
    // Bodies are captured where the wave will meet them: in the poses
    // frozen when the burst began, as `step` hits them. A capture cut
    // into slices would otherwise read a moving hull's armour where it
    // has driven to, while the wave strikes its frozen cells as bare
    // (ADR-445).
    let queries: Vec<_> = units
        .iter()
        .enumerate()
        .filter_map(|(i, u)| {
            let (pose, segs, delta) = (e.poses[i], &e.seg_poses[i], e.turret_deltas[i]);
            let (lo, hi) = u.bounds_aimed(Some(&pose), segs, delta, e.turret_pitches[i]);
            (0..3)
                .all(|a| far[a] >= lo[a] && origin[a] <= hi[a])
                .then(|| {
                    let frames = HitFrames::new(u, pose, segs, delta, e.turret_pitches[i]);
                    let terms = (parallel && spent >= 1024 && !e.point_contacts)
                        .then(|| frames.wave_terms(u, &pose, segs));
                    (u, lo, hi, pose, segs, frames, terms)
                })
        })
        .collect();
    // Bodies are asked only where they can be (ADR-296): one box test
    // per body instead of a pose-mapped lookup for every voxel of the
    // domain, and dents are looked up only where there is material.
    // Three questions used to be asked of every voxel of the domain,
    // and all three answer the same for a whole cell (ADR-310): which
    // bodies can reach into it, whether any dented voxel lies in it,
    // and which chunk holds it. Each is asked once a cell now. The
    // answers are the same to the bit — this is a hoist, not an
    // approximation — and at four charges the capture fell from 12.2
    // to 11.2 ms on a clean world and from 23.7 to 17.3 ms late in a
    // match, where the dent map has grown.
    let mut dented = vec![false; cells];
    for q in dents
        .range([origin[0], i32::MIN, i32::MIN]..=[far[0], i32::MAX, i32::MAX])
        .map(|(q, _)| q)
    {
        let cell = [0, 1, 2].map(|a| (q[a] - origin[a]).div_euclid(e.cell));
        if (0..3).all(|a| cell[a] >= 0 && (cell[a] as usize) < n) {
            dented[(cell[2] as usize * n + cell[1] as usize) * n + cell[0] as usize] = true;
        }
    }
    // Every body role uses this same monotone, clamped formula in
    // unit_work. Dents can only lower resistance. Once the reduction
    // reaches this bound, no remaining body sample can change it.
    let unit_maximum = queries
        .iter()
        .map(|(u, ..)| {
            std::iter::once(&u.body)
                .chain(u.body.segments.iter().map(|s| &s.body))
                .map(|body| super::maximum_unit_work(body, c))
                .max()
                .unwrap()
        })
        .max()
        .unwrap_or(1);
    // Determine exactly the old slice boundary before dispatch. Outside
    // cells are free, but even those wait once the logical quota is spent.
    // The serial test oracle keeps the original per-cell path, so comparing
    // workers also checks row proofs, not merely parallel dispatch of them.
    // Production capture always enables these exact row reductions.
    let batch_rows = !cfg!(test) || parallel;
    let batch_ground =
        COLUMNS && batch_rows && (5..=64).contains(&e.cell) && w.batches_material_columns();
    let fill = |start: usize, output: &mut [u32]| {
        // A parallel chunk visits at most two Z planes. Share column
        // ceilings across heights locally; no cache crosses a worker.
        let first_z = start / (n * n);
        let last_z = (start + output.len() - 1) / (n * n);
        let mut ceilings = vec![u32::MAX; n * (last_z - first_z + 1)];
        let mut row_ceilings = vec![u32::MAX; last_z - first_z + 1];
        let ceiling = |x: usize, z: usize, ceilings: &mut [u32]| {
            let cached = &mut ceilings[(z - first_z) * n + x];
            if *cached == u32::MAX {
                *cached = 0;
                let wx = origin[0] + x as i32 * e.cell;
                let wz = origin[2] + z as i32 * e.cell;
                *cached = w.column_box_ceiling(
                    [wx.max(0) as u32, wz.max(0) as u32],
                    [(wx + e.cell).max(0) as u32, (wz + e.cell).max(0) as u32],
                );
            }
            *cached
        };
        let mut cursor = World::mat_cursor();
        let mut column = [0u16; 64];
        let mut parts = Vec::new();
        let mut offset = 0;
        while offset < output.len() {
            let index = start + offset;
            let (x, y, z) = (index % n, index / n % n, index / (n * n));
            let len = (n - x).min(output.len() - offset);
            let wy = origin[1] + y as i32 * e.cell;
            // The deepest included voxel bounds this row's confinement.
            // Uniform soil can still prove its maximum with one sample.
            let maximum = if c.material_resistance.is_some() {
                (0..9)
                    .map(|mat| terrain_work(mat, wy.max(0), e.soil_y(), c))
                    .max()
                    .unwrap_or(0)
                    .max(c.steel_resistance)
            } else {
                terrain_work(1, wy.max(0), e.soil_y(), c)
                    .max(c.soil_resistance)
                    .max(c.concrete_resistance)
                    .max(c.steel_resistance)
            };
            let (cy, cz) = (y as i32 - half as i32, z as i32 - half as i32);
            let transverse = cy * cy + cz * cz;
            // Exact constant rows: bedrock/outside the sphere, or verified
            // air above terrain and every frozen hull in this Z strip. Only
            // the spherical boundary is refined per cell. Quotas and the
            // captured linear prefix remain unchanged.
            if batch_rows && (wy + e.cell <= 0 || transverse > radius2) {
                output[offset..offset + len].fill(u32::MAX);
                offset += len;
                continue;
            }
            if batch_rows && x == 0 && len == n {
                let row = &mut row_ceilings[z - first_z];
                if *row == u32::MAX {
                    *row = (0..n).map(|x| ceiling(x, z, &mut ceilings)).max().unwrap();
                }
                let wz = origin[2] + z as i32 * e.cell;
                let body = queries.iter().any(|(_, lo, hi, ..)| {
                    wy <= hi[1] && wy + e.cell > lo[1] && wz <= hi[2] && wz + e.cell > lo[2]
                });
                if wy >= *row as i32 && !body {
                    for (x, resistance) in output[offset..offset + len].iter_mut().enumerate() {
                        let cx = x as i32 - half as i32;
                        *resistance = if cx * cx + transverse > radius2 {
                            u32::MAX
                        } else {
                            0
                        };
                    }
                    offset += len;
                    continue;
                }
            }
            for (local, resistance) in output[offset..offset + len].iter_mut().enumerate() {
                let offset = offset + local;
                let index = start + offset;
                let (x, y, z) = (index % n, (index / n) % n, index / (n * n));
                let p = [
                    origin[0] + x as i32 * e.cell,
                    origin[1] + y as i32 * e.cell,
                    origin[2] + z as i32 * e.cell,
                ];
                // Beyond the inscribed sphere the wave does not go: a reach
                // the grid cuts is cut the same in every direction.
                let (cx, cy, cz) = (
                    x as i32 - half as i32,
                    y as i32 - half as i32,
                    z as i32 - half as i32,
                );
                if cx * cx + cy * cy + cz * cz > radius2 {
                    *resistance = u32::MAX;
                    continue;
                }
                let end = [0, 1, 2].map(|a| p[a] + e.cell - 1);
                // Under the world is bedrock: the wave does not run beneath the
                // floor as through air and come up into the ground from below
                // (ADR-681).
                if end[1] < 0 {
                    *resistance = u32::MAX;
                    continue;
                }
                let look = dented[(z * n + y) * n + x];
                let mut r = 0;
                if p[1] < w.ny as i32 && end[1] >= 0 {
                    if p[1] < ceiling(x, z, &mut ceilings) as i32 {
                        let x0 = p[0].max(0);
                        let x1 = (end[0] + 1).min(w.nx as i32);
                        let y0 = p[1].max(0);
                        let z0 = p[2].max(0);
                        let z1 = (end[2] + 1).min(w.nz as i32);
                        if batch_ground && x0 < x1 && z0 < z1 {
                            let maximum = if !look {
                                terrain_candidates(
                                    w,
                                    [x0, y0, z0],
                                    [x1, (end[1] + 1).min(w.ny as i32), z1],
                                )
                                .map_or(maximum, |ids| {
                                    ids.into_iter()
                                        .map(|m| terrain_work(m, y0, e.soil_y(), c))
                                        .max()
                                        .unwrap_or(0)
                                })
                            } else {
                                maximum
                            };
                            // A saturated first sample already proves the
                            // terrain maximum, without preparing a column.
                            // Otherwise exact max is independent of traversal
                            // order: share generator work along each column.
                            let q = [x0, y0, z0];
                            let dent = if look {
                                dents.get(&q).copied().unwrap_or(0)
                            } else {
                                0
                            };
                            r = terrain_work(
                                material_cached(w, &mut cursor, q),
                                q[1],
                                e.soil_y(),
                                c,
                            )
                            .saturating_sub(dent);
                            // Tall solid surfaces often supply the exact max
                            // (rock, walls). A bounded set of live witnesses can
                            // prove it before preparing every ground column.
                            if r < maximum && !look {
                                'witnesses: for iz in 0..4 {
                                    for ix in 0..4 {
                                        let wx = x0 + (x1 - x0 - 1) * ix / 3;
                                        let wz = z0 + (z1 - z0 - 1) * iz / 3;
                                        let top = (end[1] + 1)
                                            .min(w.ny as i32)
                                            .min(w.column_ceiling(wx as u32, wz as u32) as i32);
                                        if top <= y0 {
                                            continue;
                                        }
                                        let q = [wx, top - 1, wz];
                                        r = r.max(terrain_work(
                                            material_cached(w, &mut cursor, q),
                                            q[1],
                                            e.soil_y(),
                                            c,
                                        ));
                                        if r == maximum {
                                            break 'witnesses;
                                        }
                                    }
                                }
                            }
                            if r < maximum && !look && e.cell >= 16 {
                                terrain_peak(
                                    w,
                                    [x0 as u32, y0 as u32, z0 as u32],
                                    [x1 as u32, (end[1] + 1).min(w.ny as i32) as u32, z1 as u32],
                                    e.soil_y(),
                                    c,
                                    &mut r,
                                    &mut cursor,
                                );
                            } else if r < maximum {
                                'columns: for wz in z0..z1 {
                                    for wx in x0..x1 {
                                        let y1 = (end[1] + 1)
                                            .min(w.ny as i32)
                                            .min(w.column_ceiling(wx as u32, wz as u32) as i32);
                                        if y1 <= y0 {
                                            continue;
                                        }
                                        w.column(
                                            wx as u32,
                                            wz as u32,
                                            y0 as u32,
                                            &mut column[..(y1 - y0) as usize],
                                        );
                                        for (dy, &mat) in
                                            column[..(y1 - y0) as usize].iter().enumerate()
                                        {
                                            if mat == 0 {
                                                continue;
                                            }
                                            let q = [wx, y0 + dy as i32, wz];
                                            let dent = if look {
                                                dents.get(&q).copied().unwrap_or(0)
                                            } else {
                                                0
                                            };
                                            r = r.max(
                                                terrain_work(mat, q[1], e.soil_y(), c)
                                                    .saturating_sub(dent),
                                            );
                                            if r == maximum {
                                                break 'columns;
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            'terrain: for wz in z0..z1 {
                                for wy in p[1].max(0)..(end[1] + 1).min(w.ny as i32) {
                                    if x1 <= x0 {
                                        continue;
                                    }
                                    let mut bits = w.solid_bits(
                                        x0 as u32,
                                        wy as u32,
                                        wz as u32,
                                        (x1 - x0) as u32,
                                    );
                                    while bits != 0 {
                                        let q = [x0 + bits.trailing_zeros() as i32, wy, wz];
                                        bits &= bits - 1;
                                        let work = terrain_work(
                                            material_cached(w, &mut cursor, q),
                                            q[1],
                                            e.soil_y(),
                                            c,
                                        );
                                        let dent = if look {
                                            dents.get(&q).copied().unwrap_or(0)
                                        } else {
                                            0
                                        };
                                        r = r.max(work.saturating_sub(dent));
                                        if r == maximum {
                                            break 'terrain;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Body resistance is independent of the terrain reduction. Ask
                // only the intersecting part of each box, including suspended parts.
                'bodies: for (u, lo, hi, pose, segments, query, terms) in &queries {
                    if r >= unit_maximum {
                        break;
                    }
                    if (0..3).any(|a| end[a] < lo[a] || p[a] > hi[a]) {
                        continue;
                    }
                    if !e.point_contacts {
                        // Same voxel-centre ownership as damage, including thin
                        // rotated plates missed by the old world-point samples.
                        query.visit_wave_cells_with_terms(
                            u,
                            pose,
                            segments,
                            p,
                            e.cell,
                            terms.as_ref(),
                            |(part, cell)| {
                                let occupied = if part == 0 {
                                    u.body.vol.get(cell[0], cell[1], cell[2])
                                } else {
                                    !u.seg_gone[part - 1]
                                        && u.body.segments[part - 1]
                                            .body
                                            .vol
                                            .get(cell[0], cell[1], cell[2])
                                };
                                if occupied {
                                    r = r.max(u.hit_resistance(Some((part, cell)), c));
                                }
                                r < unit_maximum
                            },
                        );
                        if r >= unit_maximum {
                            break 'bodies;
                        }
                        continue;
                    }
                    query.parts_in(u, p, end, &mut parts);
                    for wz in p[2].max(lo[2])..=end[2].min(hi[2]) {
                        for wy in p[1].max(lo[1])..=end[1].min(hi[1]) {
                            for wx in p[0].max(lo[0])..=end[0].min(hi[0]) {
                                r = r.max(u.hit_resistance(
                                    query.hit_parts(u, [wx, wy, wz], parts.iter().copied()),
                                    c,
                                ));
                                if r >= unit_maximum {
                                    break 'bodies;
                                }
                            }
                        }
                    }
                }
                *resistance = r;
            }
            offset += len;
        }
    };
    let mut resistance = vec![0; next - e.captured];
    // Large reads use the existing physics pool. Indexed, disjoint chunks
    // retain cell order; a single-thread pool and small slices stay local.
    if parallel && spent >= 1024 && rayon::current_num_threads() > 1 {
        let plane = n * n;
        let workers = rayon::current_num_threads();
        // Split short prefixes on whole rows: a fixed one-plane task leaves
        // most workers idle when the capture quota covers less than a plane.
        // Keep larger prefixes plane-sized to reuse the local height caches.
        let chunk = if split_short_slices && resistance.len() < plane * workers {
            resistance.len().div_ceil(workers).max(512).div_ceil(n) * n
        } else {
            plane
        }
        .min(plane);
        resistance
            .par_chunks_mut(chunk)
            .enumerate()
            .for_each(|(i, out)| {
                fill(e.captured + i * chunk, out);
            });
    } else if !resistance.is_empty() {
        fill(e.captured, &mut resistance);
    }
    for (i, r) in resistance.into_iter().enumerate() {
        e.wave.set_resistance(e.captured + i, r);
    }
    e.captured = next;
    !e.capturing()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::{blob::Blob, Fx};
    use modding::range::RangeUnitKind;
    use voxel::{Edit, Generator};

    #[test]
    fn explicit_material_work_above_legacy_bounds_matches_scalar_capture() {
        struct Terrain(bool);
        impl Generator for Terrain {
            fn material(&self, x: u32, y: u32, z: u32) -> u16 {
                if y < 40 {
                    if (x + z) % 7 == 0 {
                        4
                    } else {
                        3
                    }
                } else {
                    0
                }
            }
            fn batches_columns(&self) -> bool {
                true
            }
            fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<u16>> {
                self.0.then(|| vec![0, 3, 4])
            }
        }
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
        c.half_cells = 2;
        c.energy = 100;
        c.soil_depth_cost = 0;
        c.material_resistance.as_mut().unwrap()[4] = 5000;
        for (edge, bounded) in [(16, false), (64, false), (16, true), (64, true)] {
            let w = World::new(160, 96, 160, 100, Box::new(Terrain(bounded)));
            c.cell_units = edge;
            let mut serial = Explosion::begin([80, 40, 80], &c, &[]);
            let mut actual = Explosion::begin([80, 40, 80], &c, &[]);
            for budget in [1, 7, 29, usize::MAX] {
                capture(&mut serial, &w, &c, &[], &SurfaceWork::new(), budget, false);
                capture(&mut actual, &w, &c, &[], &SurfaceWork::new(), budget, true);
                let mut a = Blob::new();
                let mut b = Blob::new();
                serial.save(&mut a);
                actual.save(&mut b);
                assert_eq!(a.v, b.v, "edge={edge} budget={budget}");
            }
            // Independent known maximum: every occupied row contains rare
            // foliage5000 among steel900. An old900 bound must not stop at
            // the first steel voxel, even when generator IDs are unknown.
            assert_eq!(actual.wave.resistance_at([2, 1, 2]), 5000);
        }
    }

    #[test]
    fn regional_peak_matches_dense_voxels_with_thin_walls_holes_and_live_edits() {
        struct Terrain;
        impl Generator for Terrain {
            fn material(&self, x: u32, y: u32, z: u32) -> u16 {
                if y < 12 {
                    1
                } else if x == 51 && (17..35).contains(&y) && (33..83).contains(&z) {
                    3
                } else if y == 36 && (32..91).contains(&x) && (21..71).contains(&z) {
                    2
                } else if (x + z) % 11 == 0 && y < 29 {
                    5
                } else {
                    0
                }
            }
            fn material_candidates(&self, min: [u32; 3], max: [u32; 3]) -> Option<Vec<u16>> {
                let mut ids = vec![0, 1, 5];
                if min[0] <= 51
                    && max[0] > 51
                    && min[1] < 35
                    && max[1] > 17
                    && min[2] < 83
                    && max[2] > 33
                {
                    ids.push(3);
                }
                if min[1] <= 36
                    && max[1] > 36
                    && min[0] < 91
                    && max[0] > 32
                    && min[2] < 71
                    && max[2] > 21
                {
                    ids.push(2);
                }
                Some(ids)
            }
            fn uniform_box(&self, _: [u32; 3], max: [u32; 3]) -> Option<u16> {
                (max[1] <= 12).then_some(1)
            }
        }
        let mut w = World::new(128, 64, 128, 100, Box::new(Terrain));
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
        for edited in [false, true] {
            if edited {
                for (p, mat) in [
                    ([51, 23, 45], 0),
                    ([40, 9, 60], 0),
                    ([17, 46, 27], 65535),
                    ([80, 4, 50], 2),
                ] {
                    w.queue_edit(Edit::put(p[0], p[1], p[2], mat));
                }
                w.flush();
            }
            for depth in [0, 40, 400] {
                c.soil_depth_cost = depth;
                for k in 0..80u32 {
                    let lo = [(k * 13) % 64, (k * 7) % 48, (k * 19) % 64];
                    let hi = [
                        (lo[0] + 8 + k % 57).min(w.nx),
                        (lo[1] + 1 + k % 64).min(w.ny),
                        (lo[2] + 8 + k % 57).min(w.nz),
                    ];
                    let mut expected = 0;
                    for z in lo[2]..hi[2] {
                        for y in lo[1]..hi[1] {
                            for x in lo[0]..hi[0] {
                                expected = expected.max(terrain_work(
                                    w.material(x, y, z),
                                    y as i32,
                                    32,
                                    &c,
                                ));
                            }
                        }
                    }
                    let mut actual = 0;
                    terrain_peak(&w, lo, hi, 32, &c, &mut actual, &mut World::mat_cursor());
                    assert_eq!(
                        actual, expected,
                        "edited={edited} depth={depth} lo={lo:?} hi={hi:?}"
                    );
                }
            }
        }
    }

    struct Ground;
    impl Generator for Ground {
        fn batches_columns(&self) -> bool {
            true // Exercise the column path at coarse edges as well.
        }
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            if y < 16 {
                2
            } else if (104..108).contains(&x) && (80..112).contains(&z) && y < 40 {
                3
            } else {
                0
            }
        }
    }

    #[test]
    fn contact_rows_match_voxel_oracle_at_edges_and_after_edits() {
        use super::super::{material, read_cell_into, read_occupancy, subcell_offsets};
        let mut w = World::new(192, 64, 192, 100, Box::new(Ground));
        for edited in [false, true] {
            if edited {
                for x in [0, 31, 63, 104, 191] {
                    w.queue_edit(Edit::dig_if(x, 15, 96, 2));
                    w.queue_edit(Edit::put(x, 42, 96, 3));
                }
                w.flush();
            }
            for edge in [1, 2, 3, 4, 5, 8, 9, 12, 16, 64] {
                let offsets = subcell_offsets(edge);
                for x in [-3, -1, 0, 15, 31, 62, 63, 103, 190, 192] {
                    for y in [-3, -1, 0, 14, 15, 16, 38, 41, 63, 64] {
                        for z in [-1, 0, 31, 63, 95, 96, 190, 192] {
                            let p = [x, y, z];
                            let mut want = (Vec::new(), vec![99]);
                            for (s, o) in offsets.iter().enumerate() {
                                let mat = material(&w, std::array::from_fn(|a| p[a] + o[a]));
                                if mat != 0 {
                                    want.0.push(s);
                                    want.1.push(mat);
                                }
                            }
                            assert_eq!(
                                read_occupancy(&w, p, edge, [0; 3])
                                    .indices_from(0)
                                    .collect::<Vec<_>>(),
                                want.0
                            );
                            let mut mats = vec![99];
                            let mask = read_cell_into(&w, p, edge, [0; 3], &offsets, &mut mats);
                            assert_eq!(
                                (mask.indices_from(0).collect::<Vec<_>>(), mats),
                                want,
                                "{edited} {edge} {p:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn parallel_slices_preserve_quota_frozen_poses_and_live_edits() {
        let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        c.energy = 100;
        c.half_cells = 8;
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for (at, edge) in [
                ([0, 4, 0], 1),
                ([100, 28, 96], 2),
                ([100, 28, 96], 3),
                ([191, 62, 191], 4),
                ([0, 4, 0], 5),
                ([100, 28, 96], 9),
                ([191, 62, 191], 16),
                ([100, 28, 96], 64),
            ] {
                c.cell_units = edge;
                let mut w = World::new(192, 64, 192, 100, Box::new(Ground));
                let mut units = vec![
                    RangeUnit::of_kind(RangeUnitKind::Tank, [96, 16, 96]),
                    RangeUnit::of_kind(RangeUnitKind::Strider, [104, 16, 96]),
                ];
                units[0].turret_delta = 4927;
                units[0].update_pose(2731);
                units[1].update_pose(-5147);
                let mut dents = SurfaceWork::from([([104, 20, 96], 379)]);
                let mut serial = Explosion::begin(at, &c, &units);
                let mut actual = Explosion::begin(at, &c, &units);
                for (slice, budget) in [0, 1, 17, 1023, 1024, 31, usize::MAX, 0]
                    .into_iter()
                    .enumerate()
                {
                    if slice == 4 {
                        // Geometry, live armour work and turret pose change
                        // between slices; the burst retains its start frames.
                        for y in 16..32 {
                            w.queue_edit(Edit::dig_if(104, y, 96, 3));
                        }
                        w.flush();
                        dents.insert([105, 25, 97], 380);
                        units[0].motion.pos[0] = units[0].motion.pos[0].add(Fx::from_int(40));
                        units[0].turret_delta += 3017;
                        units[0].dents.insert([10, 10, 10], 91);
                        units[0].update_pose(7133);
                        units[1].seg_gone[0] = true;
                    }
                    let start = serial.captured;
                    let want = capture(&mut serial, &w, &c, &units, &dents, budget, false);
                    let got = pool.install(|| actual.capture(&w, &c, &units, &dents, budget));
                    assert_eq!(want, got);
                    let mut before = Blob::new();
                    let mut after = Blob::new();
                    serial.save(&mut before);
                    actual.save(&mut after);
                    assert_eq!(
                        before.v, after.v,
                        "workers={workers}, edge={edge}, slice={slice}"
                    );
                    let n = actual.wave.size();
                    let half = (n / 2) as i32;
                    let inside = |i: usize| {
                        [i % n, (i / n) % n, i / (n * n)]
                            .into_iter()
                            .map(|v| (v as i32 - half).pow(2))
                            .sum::<i32>()
                            <= half * half
                    };
                    let charged = (start..actual.captured).filter(|&i| inside(i)).count();
                    assert!(charged <= budget);
                    if actual.captured < n.pow(3) {
                        assert_eq!(charged, budget);
                        if budget > 0 {
                            assert!(inside(actual.captured - 1));
                        }
                    }
                }
                assert!(!actual.capturing());
            }
        }
    }

    #[test]
    fn short_capture_slices_match_plane_tasks_with_multiple_workers() {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
        c.half_cells = 32;
        c.cell_units = 2;
        let w = World::new(192, 64, 192, 100, Box::new(Ground));
        let units = vec![RangeUnit::of_kind(RangeUnitKind::Tank, [96, 16, 96])];
        let dents = SurfaceWork::from([([96, 15, 96], 100)]);
        for workers in [2, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for budget in [1024, 1536] {
                let mut reference = Explosion::begin([96, 28, 96], &c, &units);
                let n = reference.wave.size();
                reference.captured = n * n * (n / 2) + n * (n / 2 - 8);
                let mut actual = Explosion::begin([96, 28, 96], &c, &units);
                actual.captured = reference.captured;
                let start = std::time::Instant::now();
                let before = pool.install(|| {
                    capture_inner::<true>(
                        &mut reference,
                        &w,
                        &c,
                        &units,
                        &dents,
                        budget,
                        true,
                        false,
                    )
                });
                let baseline = start.elapsed();
                let start = std::time::Instant::now();
                let after = pool.install(|| {
                    capture_inner::<true>(&mut actual, &w, &c, &units, &dents, budget, true, true)
                });
                eprintln!(
                    "capture workers={workers} budget={budget} plane_us={} split_us={}",
                    baseline.as_micros(),
                    start.elapsed().as_micros()
                );
                let (mut a, mut b) = (Blob::new(), Blob::new());
                reference.save(&mut a);
                actual.save(&mut b);
                assert_eq!(before, after);
                assert_eq!(a.v, b.v, "workers={workers} budget={budget}");
            }
        }
    }

    #[test]
    #[ignore = "manual exact coarse capture comparison; run alone in release"]
    fn measure_coarse_column_capture() {
        use std::{hint::black_box, time::Instant};
        let w = World::new(
            640,
            192,
            640,
            100,
            Box::new(super::super::RangeTerrain::new(true, None)),
        );
        let c = ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let dents = SurfaceWork::from([([320, 16, 320], 1_000_000), ([321, 24, 320], 713)]);
        println!("CAPTURE_PROBE budget,columns,median_us,max_us,captured");
        for budget in [4096, 65536, usize::MAX] {
            let mut expected = Vec::new();
            for columns in [false, true] {
                let mut times = Vec::new();
                let mut count = 0;
                for rep in 0..8 {
                    let mut e = Explosion::begin([320, 24, 320], &c, &[]);
                    let start = Instant::now();
                    pool.install(|| {
                        if columns {
                            capture_inner::<true>(&mut e, &w, &c, &[], &dents, budget, true, true)
                        } else {
                            capture_inner::<false>(&mut e, &w, &c, &[], &dents, budget, true, true)
                        }
                    });
                    let elapsed = start.elapsed().as_nanos();
                    if rep > 0 {
                        times.push(elapsed);
                    }
                    let mut saved = Blob::new();
                    e.save(&mut saved);
                    if expected.is_empty() {
                        expected = saved.v.clone();
                    }
                    assert_eq!(saved.v, expected, "budget={budget} columns={columns}");
                    count = e.captured;
                    black_box(&e);
                }
                times.sort_unstable();
                println!(
                    "CAPTURE_PROBE {budget},{columns},{:.3},{:.3},{count}",
                    times[3] as f64 / 1000.0,
                    times[6] as f64 / 1000.0
                );
            }
        }
    }
}
