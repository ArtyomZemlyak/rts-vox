//! Surface heating around the unobstructed laser segment (ADR-473).
use super::*;

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
    fn laser_cell(
        &self,
        p: [i32; 3],
        bounds: &[(usize, ([i32; 3], [i32; 3]))],
    ) -> Option<thermal::Cell> {
        bounds.iter().find_map(|(id, (lo, hi))| {
            if (0..3).any(|a| p[a] < lo[a] || p[a] > hi[a]) {
                return None;
            }
            self.units[*id].hit(p).map(|(part, at)| thermal::Cell {
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
        bounds: &[(usize, ([i32; 3], [i32; 3]))],
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
            previous = p;
        }
        false
    }

    #[cfg(test)]
    pub(super) fn laser_halo(
        &mut self,
        from: [Fx; 3],
        to: [Fx; 3],
        contact: Option<[i32; 3]>,
        tuning: &Stream,
    ) {
        self.laser_halo_owned(from, to, contact, tuning, None);
    }
    pub(super) fn laser_halo_owned(
        &mut self,
        from: [Fx; 3],
        to: [Fx; 3],
        contact: Option<[i32; 3]>,
        tuning: &Stream,
        owner: Option<usize>,
    ) {
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
                let box_at = u.bounds();
                (0..3)
                    .all(|i| box_at.0[i] <= region_hi[i] && box_at.1[i] >= region_lo[i])
                    .then_some((id, box_at))
            })
            .collect();
        // Above both the terrain column and every nearby unit box there is
        // no heatable matter. Clip vertical loops before evaluating the
        // exact distance to the beam for each empty-air candidate.
        let unit_top = bounds
            .iter()
            .map(|(_, (_, hi))| hi[1] + 1)
            .max()
            .unwrap_or(0);
        let core_cell = contact.and_then(|p| self.laser_cell(p, &bounds));
        let mut doses = BTreeMap::<thermal::Cell, u32>::new();
        let r2 = i64::from(radius * 16).pow(2);
        // Reject the many cells in the enclosing square that cannot lie in
        // the round halo before the three integer divisions below. The exact
        // dose still uses the original sixteenth-cell projection. Two extra
        // sixteenths cover its < sqrt(3) quantization error, so this test can
        // only reject cells whose exact dose is zero.
        let lo = (a[axis].min(b[axis]) / 16 - i64::from(radius)).max(0) as i32;
        let hi = (a[axis].max(b[axis]) / 16 + i64::from(radius)).min(i64::from(self.size[axis]) - 1)
            as i32;
        for along in lo..=hi {
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
                    let top =
                        (self.world.column_ceiling(x as u32, along as u32) as i32).max(unit_top);
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
                        doses
                            .entry(c)
                            .and_modify(|h| *h = (*h).max(heat))
                            .or_insert(heat);
                    } else {
                        self.heat.add(&self.world, p.map(|n| n as u32), heat);
                    }
                }
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
    use super::outside_halo;

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
