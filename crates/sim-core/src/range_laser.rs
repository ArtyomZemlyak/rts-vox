//! Surface heating around the unobstructed laser segment (ADR-473).
use super::*;

type Bounds = ([i32; 3], [i32; 3]);
type HaloObservations<'a, F> = (&'a [Bounds], &'a F);
type HaloBody = (usize, Bounds, Option<HitFrames>);

#[derive(Default)]
struct HaloDeposits {
    ground: Vec<([u32; 3], u32)>,
    units: BTreeMap<thermal::Cell, u32>,
}

#[inline]
fn outside_halo(q: [i64; 3], d: [i64; 3], length2: i64, radius: i32) -> bool {
    // The exact projection rounds each component by < 1 sixteenth. A margin
    // of 2 covers the resulting three-dimensional displacement.
    // The range's maximum map dimensions keep these squared sixteenth-cell
    // coordinates within i64. Only the final products need i128; widening
    // each input and squaring it there was costly for every halo candidate.
    let limit = i64::from(radius) * 16 + 2;
    let limit2 = limit * limit;
    let q2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2];
    let dot = q[0] * d[0] + q[1] * d[1] + q[2] * d[2];
    if dot <= 0 {
        q2 >= limit2
    } else if dot >= length2 {
        let end = [0, 1, 2].map(|i| q[i] - d[i]);
        end[0] * end[0] + end[1] * end[1] + end[2] * end[2] >= limit2
    } else {
        i128::from(q2) * i128::from(length2) - i128::from(dot) * i128::from(dot)
            >= i128::from(limit2) * i128::from(length2)
    }
}

impl BlastRange {
    fn laser_cell(&self, p: [i32; 3], bounds: &[HaloBody]) -> Option<thermal::Cell> {
        bounds.iter().find_map(|(id, (lo, hi), frames)| {
            if (0..3).any(|a| p[a] < lo[a] || p[a] > hi[a]) {
                return None;
            }
            let u = &self.units[*id];
            frames
                .as_ref()
                .map_or_else(|| u.hit(p), |f| f.hit(u, p))
                .map(|(part, at)| thermal::Cell {
                    unit: *id as u16,
                    part: part as u8,
                    at: at.map(|v| v as u8),
                })
        })
    }

    // A short radial visibility ray stops on the first surface or shield.
    // Several world samples can address the same rotated armour voxel.
    fn laser_visible(
        &self,
        source: [i64; 3],
        target: [i32; 3],
        cell: Option<thermal::Cell>,
        bounds: &[HaloBody],
    ) -> bool {
        let delta: [i64; 3] = std::array::from_fn(|a| i64::from(target[a]) * 16 + 8 - source[a]);
        let steps = (delta.iter().map(|v| v.abs()).max().unwrap() + 3) / 4;
        let mut previous = source.map(|v| v.div_euclid(16) as i32);
        for i in 0..=steps.max(1) {
            let p = std::array::from_fn(|a| {
                (source[a] + delta[a] * i / steps.max(1)).div_euclid(16) as i32
            });
            // Quarter-cell steps often land in the same voxel. Its shield,
            // body and terrain cannot change during this visibility query.
            if i > 0 && p == previous {
                continue;
            }
            if self.dome_crossed(previous, p).is_some() {
                return false;
            }
            let other = self.laser_cell(p, bounds);
            if other.is_some() {
                return other == cell;
            }
            if self.world.solid(p[0] as u32, p[1] as u32, p[2] as u32) {
                return cell.is_none() && p == target;
            }
            if p==target && cell.is_none() && p.iter().all(|v|*v>=0)
                && self.water.amount(p.map(|v|v as u32))>0 {return true;}
            previous = p;
        }
        false
    }

    #[cfg(test)]
    pub(crate) fn laser_halo(
        &mut self,
        from: [Fx; 3],
        to: [Fx; 3],
        contact: Option<[i32; 3]>,
        tuning: &Stream,
    ) {
        self.laser_halo_owned(from, to, contact, tuning, None);
    }
    #[cfg(test)]
    pub(super) fn laser_halo_owned(
        &mut self,
        from: [Fx; 3],
        to: [Fx; 3],
        contact: Option<[i32; 3]>,
        tuning: &Stream,
        owner: Option<usize>,
    ) {
        self.laser_halo_in([from, to], contact, tuning, owner, None, (true, &|_, _| {}));
    }
    pub(super) fn laser_halo_with_boxes(
        &mut self,
        from: [Fx; 3],
        to: [Fx; 3],
        contact: Option<[i32; 3]>,
        tuning: &Stream,
        owner: Option<usize>,
        observations: HaloObservations<'_, impl Fn(usize, &'static str) + Sync>,
    ) {
        let (boxes, task) = observations;
        self.laser_halo_in(
            [from, to],
            contact,
            tuning,
            owner,
            Some(boxes),
            (true, task),
        );
    }
    fn laser_halo_in(
        &mut self,
        beam: [[Fx; 3]; 2],
        contact: Option<[i32; 3]>,
        tuning: &Stream,
        owner: Option<usize>,
        boxes: Option<&[Bounds]>,
        observations: (bool, &(impl Fn(usize, &'static str) + Sync)),
    ) {
        let (parallel, task) = observations;
        let _total = PhysicsTask::new(task, "physics_laser_halo_begin", "physics_laser_halo_end");
        let [from, to] = beam;
        let radius = tuning.heat_radius;
        if radius == 0 || tuning.heat == 0 {
            return;
        }
        // Sixteenth-cell geometry, integer projection and a dominant-axis
        // sweep: work grows with beam length, never its enclosing box volume.
        let a = from.map(|v| i64::from(v.mul(Fx::from_int(16)).to_int_trunc()));
        let b = to.map(|v| i64::from(v.mul(Fx::from_int(16)).to_int_trunc()));
        let d: [i64; 3] = std::array::from_fn(|i| b[i] - a[i]);
        // A muzzle immediately beside a wall still heats a contact patch.
        let length2 = d.iter().map(|v| v * v).sum::<i64>().max(1);
        let axis = (0..3).max_by_key(|&i| d[i].abs()).unwrap();
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let axial_length = if d[axis] == 0 { 1 } else { d[axis] };
        // Only bodies inside the enclosing halo box can address any of its
        // surface cells or obstruct a short radial visibility ray.
        let margin = 2 * radius + 2;
        let region_lo = [0, 1, 2].map(|i| {
            (a[i].min(b[i]).div_euclid(16) as i32 - margin).min(contact.map_or(i32::MAX, |p| p[i]))
        });
        let region_hi = [0, 1, 2].map(|i| {
            (a[i].max(b[i]).div_euclid(16) as i32 + margin).max(contact.map_or(i32::MIN, |p| p[i]))
        });
        let bounds: Vec<_> = self
            .units
            .iter()
            .enumerate()
            .filter_map(|(id, u)| {
                if Some(id) == owner {
                    return None;
                }
                let box_at = boxes.map_or_else(|| u.bounds(), |boxes| boxes[id]);
                if !(0..3).all(|i| box_at.0[i] <= region_hi[i] && box_at.1[i] >= region_lo[i]) {
                    return None;
                }
                // Geometry is immutable until all radial rays have deposited
                // their heat. Reuse transforms/segment boxes for this halo;
                // the serial test oracle retains the original direct hit.
                let frames = (!cfg!(test) || parallel).then(|| {
                    HitFrames::new(u, u.pose, &u.seg_frames, u.turret_delta, u.turret_pitch)
                });
                Some((id, box_at, frames))
            })
            .collect();
        // Above both the terrain column and every nearby unit box there is
        // no heatable matter. Clip vertical loops before evaluating the
        // exact distance to the beam for each empty-air candidate.
        let unit_top = bounds
            .iter()
            .map(|(_, (_, hi), _)| hi[1] + 1)
            .max()
            .unwrap_or(0);
        let core_cell = contact.and_then(|p| self.laser_cell(p, &bounds));
        let r2 = i64::from(radius * 16).pow(2);
        // Reject the many cells in the enclosing square that cannot lie in
        // the round halo before the three integer divisions below. The exact
        // dose still uses the original sixteenth-cell projection. Two extra
        // sixteenths cover its < sqrt(3) quantization error, so this test can
        // only reject cells whose exact dose is zero.
        let lo = (a[axis].min(b[axis]) / 16 - i64::from(radius)).max(0) as i32;
        let hi = (a[axis].max(b[axis]) / 16 + i64::from(radius)).min(i64::from(self.size[axis]) - 1)
            as i32;
        // Each slice reads the same immutable geometry, shields and material
        // registry. Heat does not alter them until the later thermal phase.
        let collect = |start: i32, end: i32| {
            let _task = PhysicsTask::new(
                task,
                "physics_laser_halo_read_begin",
                "physics_laser_halo_read_end",
            );
            let mut deposits = HaloDeposits::default();
            for along in start..=end {
                let axial =
                    (i64::from(along) * 16 + 8).clamp(a[axis].min(b[axis]), a[axis].max(b[axis]));
                let centre: [i32; 3] = std::array::from_fn(|i| {
                    ((a[i] + d[i] * (axial - a[axis]) / axial_length) / 16) as i32
                });
                let x_lo = (centre[u] - 2 * radius).max(0);
                let mut x_hi = (centre[u] + 2 * radius).min(self.size[u] as i32 - 1);
                let y_lo = (centre[v] - 2 * radius).max(0);
                let y_hi = (centre[v] + 2 * radius).min(self.size[v] as i32 - 1);
                // For an X-dominant beam, the outer transverse coordinate is
                // height. A single maximum over this slice rejects only air.
                if axis == 0 {
                    let top = (y_lo..=y_hi)
                        .map(|z| self.world.column_ceiling(along as u32, z as u32) as i32)
                        .max()
                        .unwrap_or(0)
                        .max(unit_top);
                    x_hi = x_hi.min(top - 1);
                }
                // At most sqrt(3) times the radius on an axial slice, including
                // endpoint caps. Every world cell is visited exactly once.
                for x in x_lo..=x_hi {
                    let mut column_y_hi = y_hi;
                    if axis == 2 {
                        let top = (self.world.column_ceiling(x as u32, along as u32) as i32)
                            .max(unit_top);
                        column_y_hi = column_y_hi.min(top - 1);
                    }
                    for y in y_lo..=column_y_hi {
                        let mut p = [0; 3];
                        p[axis] = along;
                        p[u] = x;
                        p[v] = y;
                        if contact == Some(p) {
                            continue;
                        }
                        let q: [i64; 3] = std::array::from_fn(|i| i64::from(p[i]) * 16 + 8 - a[i]);
                        if outside_halo(q, d, length2, radius) {
                            continue;
                        }
                        // A dose can only reach actual unit or terrain matter.
                        // Test that before the three exact projection divisions;
                        // the vast majority of halo candidates are empty air.
                        let cell = self.laser_cell(p, &bounds);
                        if cell.is_some() && cell == core_cell {
                            continue;
                        }
                        if let Some(c) = cell {
                            if self.units[c.unit as usize].section_holds(c.part as usize, c.at) {
                                continue;
                            }
                        } else {
                            if p[1] as u32 >= self.world.column_ceiling(p[0] as u32, p[2] as u32) {
                                continue;
                            }
                            if !self.world.solid(p[0] as u32, p[1] as u32, p[2] as u32) {
                                continue;
                            }
                            let mat = material(&self.world, p);
                            if !self
                                .matter
                                .economy
                                .mats
                                .get(mat)
                                .is_some_and(|m| m.melt > 0 || m.kindling > 0)
                            {
                                continue;
                            }
                        }
                        let dot = (0..3).map(|i| q[i] * d[i]).sum::<i64>().clamp(0, length2);
                        let source: [i64; 3] = std::array::from_fn(|i| a[i] + d[i] * dot / length2);
                        let distance2 = (0..3)
                            .map(|i| (i64::from(p[i]) * 16 + 8 - source[i]).pow(2))
                            .sum::<i64>();
                        if distance2 >= r2 {
                            continue;
                        }
                        let heat = (i64::from(tuning.heat) * (r2 - distance2) / r2) as u32;
                        if heat == 0 {
                            continue;
                        }
                        if !self.laser_visible(source, p, cell, &bounds) {
                            continue;
                        }
                        if let Some(c) = cell {
                            deposits
                                .units
                                .entry(c)
                                .and_modify(|h| *h = (*h).max(heat))
                                .or_insert(heat);
                        } else {
                            deposits.ground.push((p.map(|n| n as u32), heat));
                        }
                    }
                }
            }
            deposits
        };
        let batches = if parallel && hi - lo >= 127 {
            let count = (hi - lo + 1) as usize;
            // Indexed collection preserves axial traversal order; no thermal
            // cap, admission or saturation decision is made on a worker.
            (0..count.div_ceil(32))
                .into_par_iter()
                .map(|chunk| {
                    let start = lo + (chunk * 32) as i32;
                    collect(start, (start + 31).min(hi))
                })
                .collect::<Vec<_>>()
        } else {
            vec![collect(lo, hi)]
        };
        let _merge = PhysicsTask::new(
            task,
            "physics_laser_halo_commit_begin",
            "physics_laser_halo_commit_end",
        );
        let mut doses = BTreeMap::<thermal::Cell, u32>::new();
        for batch in batches {
            for (p, heat) in batch.ground {
                self.heat.add(&self.world, p, heat);
            }
            for (cell, heat) in batch.units {
                doses
                    .entry(cell)
                    .and_modify(|h| *h = (*h).max(heat))
                    .or_insert(heat);
            }
        }
        // Water is a sparse, separate volume. Visit a bounded selection from
        // the halo's box rather than expanding the solid-terrain scan into air.
        for p in self.water.thermal_region(region_lo,region_hi,64) {
            if contact==Some(p.map(|v|v as i32)) {continue;}
            let q: [i64;3]=std::array::from_fn(|i|i64::from(p[i])*16+8-a[i]);
            if outside_halo(q,d,length2,radius) {continue;}
            let dot=(0..3).map(|i|q[i]*d[i]).sum::<i64>().clamp(0,length2);
            let source:[i64;3]=std::array::from_fn(|i|a[i]+d[i]*dot/length2);
            let distance2=(0..3).map(|i|(i64::from(p[i])*16+8-source[i]).pow(2)).sum::<i64>();
            if distance2>=r2 {continue;}
            let dose=(i64::from(tuning.heat)*(r2-distance2)/r2) as u32;
            if dose>0 && self.laser_visible(source,p.map(|v|v as i32),None,&bounds) {
                self.water.absorb_heat(p,dose,self.size);
            }
        }
        // The firing gun is separate from `units`: neither its hull nor its
        // connected barrels enter this thermal field. Two barrels may heat
        // the same victim, but duplicate samples of one barrel never stack.
        for (cell, heat) in doses {
            self.thermals.deposit(cell, heat, &self.thermal_tuning);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pooled_halo_keeps_heat_and_admission_order_across_workers() {
        let config = ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.course = None;
        layout.shields.clear();
        layout.units.truncate(1);
        layout.units[0].at = [285, 16, 336];
        layout.blocks = [
            ([200, 100, 338], [201, 101, 340]),
            ([210, 100, 339], [211, 101, 340]),
            ([340, 100, 338], [341, 101, 340]),
            ([338, 100, 200], [340, 101, 201]),
            ([339, 100, 210], [340, 101, 211]),
        ]
        .map(|(min, max)| modding::range::RangeBlock {
            min,
            max,
            material: 3,
        })
        .to_vec();
        let initial = BlastRange::with_layout(config, layout).unwrap();
        let armour = initial.units[0].world_pos([30, 11, 0]);
        let centre = |p: [i32; 3]| p.map(|v| Fx::from_int(v).add(Fx::from_ratio(1, 2)));
        let cases = [
            ([180, 100, 336], [500, 100, 336], None),
            ([500, 100, 336], [180, 100, 336], Some([200, 100, 338])),
            ([336, 100, 180], [336, 100, 500], None),
            ([200, 20, 336], [200, 220, 336], None),
            ([180, 100, 180], [500, 100, 500], None),
            ([500, 100, 500], [180, 100, 180], None),
            ([200, 100, 336], [200, 100, 336], None),
            (
                [armour[0] - 150, armour[1], armour[2] - 2],
                [armour[0] + 150, armour[1], armour[2] - 2],
                None,
            ),
        ];
        let pools: Vec<_> = [1, 2, 4, 8]
            .into_iter()
            .map(|workers| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .build()
                    .unwrap()
            })
            .collect();
        for shielded in [false, true] {
            let mut layout = initial.layout.as_ref().unwrap().clone();
            if shielded {
                layout.shields.push(modding::range::RangeShieldDef {
                    at: [285, 100, 336],
                    radius: 40,
                });
            }
            let seed = BlastRange::with_layout(initial.config, layout).unwrap();
            let snapshot = seed.snapshot();
            for (case, (from, to, contact)) in cases.into_iter().enumerate() {
                for owner in [None, Some(0)] {
                    let mut serial = BlastRange::restore(seed.config, &snapshot).unwrap();
                    // Tuning is authored rather than stored in the snapshot.
                    // Exercise the cap that makes unit deposit order significant.
                    serial.thermal_tuning.max_cells = 7;
                    let mut tuning = serial.streams.tuning.laser.clone();
                    tuning.heat = 900_000; // repeated deposits exercise heat saturation
                    for _ in 0..2 {
                        serial.laser_halo_in(
                            [centre(from), centre(to)],
                            contact,
                            &tuning,
                            owner,
                            None,
                            (false, &|_, _| {}),
                        );
                    }
                    if !shielded && case == 0 {
                        assert!(serial.heat.hot() > 0, "fixture did not heat terrain");
                    }
                    if case == 7 && owner.is_none() {
                        assert!(serial.thermals.hot() > 0, "fixture did not heat armour");
                        assert!(
                            serial.thermals.rejected_heat > 0,
                            "fixture did not reach the admission cap"
                        );
                    }
                    for pool in &pools {
                        let mut parallel = BlastRange::restore(seed.config, &snapshot).unwrap();
                        parallel.thermal_tuning.max_cells = 7;
                        let trace = crate::task_trace::Trace::default();
                        pool.install(|| {
                            let boxes = laser_boxes_observed(&parallel.units, &|slot, event| {
                                trace.record(slot, event)
                            });
                            assert_eq!(
                                boxes,
                                parallel
                                    .units
                                    .iter()
                                    .map(|u| u.bounds())
                                    .collect::<Vec<_>>()
                            );
                            for _ in 0..2 {
                                parallel.laser_halo_in(
                                    [centre(from), centre(to)],
                                    contact,
                                    &tuning,
                                    owner,
                                    Some(&boxes),
                                    (true, &|slot, event| trace.record(slot, event)),
                                );
                            }
                        });
                        trace.balanced(&[
                            "physics_laser_boxes",
                            "physics_laser_halo",
                            "physics_laser_halo_read",
                            "physics_laser_halo_commit",
                        ]);
                        assert_eq!(
                            parallel.state_hash(),
                            serial.state_hash(),
                            "case={case} shield={shielded} owner={owner:?} workers={}",
                            pool.current_num_threads()
                        );
                        assert_eq!(parallel.snapshot(), serial.snapshot());
                    }
                }
            }
        }
    }

    #[test]
    fn coarse_halo_rejection_never_drops_an_exactly_heated_cell() {
        for d in [
            [0, 0, 0],
            [160, 0, 0],
            [64, 192, 48],
            [-96, 160, -80],
            [240, -64, 96],
        ] {
            let length2 = d.iter().map(|v| v * v).sum::<i64>().max(1);
            for radius in 1..=5 {
                for x in (-120..=120).step_by(16) {
                    for y in (-120..=120).step_by(16) {
                        for z in (-120..=120).step_by(16) {
                            let q = [x, y, z];
                            if !outside_halo(q, d, length2, radius) {
                                continue;
                            }
                            let dot = (0..3).map(|i| q[i] * d[i]).sum::<i64>().clamp(0, length2);
                            let exact2 = (0..3)
                                .map(|i| (q[i] - d[i] * dot / length2).pow(2))
                                .sum::<i64>();
                            assert!(
                                exact2 >= i64::from(radius * 16).pow(2),
                                "q={q:?} d={d:?} radius={radius}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn coarse_halo_integer_width_matches_wide_projection_at_map_extents() {
        for d in [[262_128, 0, 0], [-262_128, 262_128, 0], [131_072; 3]] {
            let length2 = d.iter().map(|v| v * v).sum::<i64>();
            for q in [
                [0, 0, 0],
                [262_128, 262_128, 262_128],
                [-262_128, 262_128, -262_128],
                [131_072, 16, 16],
            ] {
                let q2 = q.iter().map(|&v| i128::from(v).pow(2)).sum::<i128>();
                let dot = (0..3)
                    .map(|i| i128::from(q[i]) * i128::from(d[i]))
                    .sum::<i128>();
                for radius in 1..=8 {
                    let limit2 = i128::from(radius * 16 + 2).pow(2);
                    let expected = if dot <= 0 {
                        q2 >= limit2
                    } else if dot >= i128::from(length2) {
                        (0..3).map(|i| i128::from(q[i] - d[i]).pow(2)).sum::<i128>() >= limit2
                    } else {
                        q2 * i128::from(length2) - dot * dot >= limit2 * i128::from(length2)
                    };
                    assert_eq!(outside_halo(q, d, length2, radius), expected);
                }
            }
        }
    }
}
