//! Read-only resistance capture; ordered slice budgets precede worker jobs.
use super::{material_cached, material_work, Explosion, HitFrames, RangeUnit, SurfaceWork};
use modding::explosion::ExplosionConfig;
use rayon::prelude::*;
use voxel::World;

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
    let half = n / 2;
    let origin = e.origin;
    let radius2 = (half * half) as i32;
    let far = [0, 1, 2].map(|a| origin[a] + n as i32 * c.cell_units - 1);
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
                    (
                        u,
                        lo,
                        hi,
                        pose,
                        segs,
                        HitFrames::new(u, pose, segs, delta, e.turret_pitches[i]),
                    )
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
        let cell = [0, 1, 2].map(|a| (q[a] - origin[a]).div_euclid(c.cell_units));
        if (0..3).all(|a| cell[a] >= 0 && (cell[a] as usize) < n) {
            dented[(cell[2] as usize * n + cell[1] as usize) * n + cell[0] as usize] = true;
        }
    }
    let maximum = c
        .soil_resistance
        .max(c.concrete_resistance)
        .max(c.steel_resistance);
    // Every body role uses this same monotone, clamped formula in
    // unit_work. Dents can only lower resistance. Once the reduction
    // reaches this bound, no remaining body sample can change it.
    let unit_maximum = (u64::from(c.unit_ablation_resistance)
        * u64::from(*c.unit_role_resistance_milli.iter().max().unwrap())
        / 1000)
        .clamp(1, 1_000_000) as u32;
    // Determine exactly the old slice boundary before dispatch. Outside
    // cells are free, but even those wait once the logical quota is spent.
    let (next, spent) = slice_end(e, budget);
    // The serial test oracle keeps the original per-cell path, so comparing
    // workers also checks row proofs, not merely parallel dispatch of them.
    // Production capture always enables these exact row reductions.
    let batch_rows = !cfg!(test) || parallel;
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
                let wx = origin[0] + x as i32 * c.cell_units;
                let wz = origin[2] + z as i32 * c.cell_units;
                for dz in 0..c.cell_units {
                    for dx in 0..c.cell_units {
                        let (x, z) = (wx + dx, wz + dz);
                        if x >= 0 && z >= 0 && x < w.nx as i32 && z < w.nz as i32 {
                            *cached = (*cached).max(w.column_ceiling(x as u32, z as u32));
                        }
                    }
                }
            }
            *cached
        };
        let mut cursor = World::mat_cursor();
        let mut parts = Vec::new();
        let mut offset = 0;
        while offset < output.len() {
            let index = start + offset;
            let (x, y, z) = (index % n, index / n % n, index / (n * n));
            let len = (n - x).min(output.len() - offset);
            let wy = origin[1] + y as i32 * c.cell_units;
            let (cy, cz) = (y as i32 - half as i32, z as i32 - half as i32);
            let transverse = cy * cy + cz * cz;
            // Exact constant rows: bedrock/outside the sphere, or verified
            // air above terrain and every frozen hull in this Z strip. Only
            // the spherical boundary is refined per cell. Quotas and the
            // captured linear prefix remain unchanged.
            if batch_rows && (wy + c.cell_units <= 0 || transverse > radius2) {
                output[offset..offset + len].fill(u32::MAX);
                offset += len;
                continue;
            }
            if batch_rows && x == 0 && len == n {
                let row = &mut row_ceilings[z - first_z];
                if *row == u32::MAX {
                    *row = (0..n).map(|x| ceiling(x, z, &mut ceilings)).max().unwrap();
                }
                let wz = origin[2] + z as i32 * c.cell_units;
                let body = queries.iter().any(|(_, lo, hi, ..)| {
                    wy <= hi[1]
                        && wy + c.cell_units > lo[1]
                        && wz <= hi[2]
                        && wz + c.cell_units > lo[2]
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
                    origin[0] + x as i32 * c.cell_units,
                    origin[1] + y as i32 * c.cell_units,
                    origin[2] + z as i32 * c.cell_units,
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
                let end = [0, 1, 2].map(|a| p[a] + c.cell_units - 1);
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
                        'terrain: for wz in p[2].max(0)..(end[2] + 1).min(w.nz as i32) {
                            for wy in p[1].max(0)..(end[1] + 1).min(w.ny as i32) {
                                if x1 <= x0 {
                                    continue;
                                }
                                let mut bits =
                                    w.solid_bits(x0 as u32, wy as u32, wz as u32, (x1 - x0) as u32);
                                while bits != 0 {
                                    let q = [x0 + bits.trailing_zeros() as i32, wy, wz];
                                    bits &= bits - 1;
                                    let work = material_work(material_cached(w, &mut cursor, q), c);
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
                // Body resistance is independent of the terrain reduction. Ask
                // only the intersecting part of each box, including suspended parts.
                'bodies: for (u, lo, hi, pose, segments, query) in &queries {
                    if r >= unit_maximum {
                        break;
                    }
                    if (0..3).any(|a| end[a] < lo[a] || p[a] > hi[a]) {
                        continue;
                    }
                    if !e.point_contacts {
                        // Same voxel-centre ownership as damage, including thin
                        // rotated plates missed by the old world-point samples.
                        query.visit_wave_cells(
                            u,
                            pose,
                            segments,
                            p,
                            c.cell_units,
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
        resistance
            .par_chunks_mut(n * n)
            .enumerate()
            .for_each(|(i, out)| {
                fill(e.captured + i * n * n, out);
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

    struct Ground;
    impl Generator for Ground {
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
        use super::super::{material, read_cell_into, subcell_offsets};
        let mut w = World::new(192, 64, 192, 100, Box::new(Ground));
        for edited in [false, true] {
            if edited {
                for x in [0, 31, 63, 104, 191] {
                    w.queue_edit(Edit::dig_if(x, 15, 96, 2));
                    w.queue_edit(Edit::put(x, 42, 96, 3));
                }
                w.flush();
            }
            for edge in 1..=4 {
                let offsets = subcell_offsets(edge);
                for x in [-3, -1, 0, 15, 31, 62, 63, 103, 190, 192] {
                    for y in [-3, -1, 0, 14, 15, 16, 38, 41, 63, 64] {
                        for z in [-1, 0, 31, 63, 95, 96, 190, 192] {
                            let p = [x, y, z];
                            let mut want = (0u64, vec![99]);
                            for (s, o) in offsets.iter().enumerate() {
                                let mat = material(&w, std::array::from_fn(|a| p[a] + o[a]));
                                if mat != 0 {
                                    want.0 |= 1 << s;
                                    want.1.push(mat);
                                }
                            }
                            let mut mats = vec![99];
                            let mask = read_cell_into(&w, p, edge, [0; 3], &offsets, &mut mats);
                            assert_eq!((mask, mats), want, "{edited} {edge} {p:?}");
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
            for (at, edge) in [([0, 4, 0], 1), ([100, 28, 96], 2), ([191, 62, 191], 4)] {
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
}
