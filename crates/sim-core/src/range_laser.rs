//! Surface heating around the unobstructed laser segment (ADR-473).
use super::*;

impl BlastRange {
    fn laser_cell(&self, p: [i32; 3], bounds: &[([i32; 3], [i32; 3])]) -> Option<thermal::Cell> {
        bounds.iter().enumerate().find_map(|(id, (lo, hi))| {
            if (0..3).any(|a| p[a] < lo[a] || p[a] > hi[a]) {
                return None;
            }
            self.units[id].hit(p).map(|(part, at)| thermal::Cell {
                unit: id as u16,
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
        bounds: &[([i32; 3], [i32; 3])],
    ) -> bool {
        let delta: [i64; 3] = std::array::from_fn(|a| i64::from(target[a]) * 16 + 8 - source[a]);
        let steps = (delta.iter().map(|v| v.abs()).max().unwrap() + 3) / 4;
        let mut previous = source.map(|v| v.div_euclid(16) as i32);
        for i in 0..=steps.max(1) {
            let p = std::array::from_fn(|a| {
                (source[a] + delta[a] * i / steps.max(1)).div_euclid(16) as i32
            });
            if self.dome_crossed(previous, p).is_some() {
                return false;
            }
            let other = self.laser_cell(p, bounds);
            if other.is_some() {
                return other == cell;
            }
            if material(&self.world, p) != 0 {
                return cell.is_none() && p == target;
            }
            previous = p;
        }
        false
    }

    pub(super) fn laser_halo(
        &mut self,
        from: [Fx; 3],
        to: [Fx; 3],
        contact: Option<[i32; 3]>,
        tuning: &Stream,
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
        let bounds: Vec<_> = self.units.iter().map(RangeUnit::bounds).collect();
        let core_cell = contact.and_then(|p| self.laser_cell(p, &bounds));
        let mut doses = BTreeMap::<thermal::Cell, u32>::new();
        let r2 = i64::from(radius * 16).pow(2);
        let lo = (a[axis].min(b[axis]) / 16 - i64::from(radius)).max(0) as i32;
        let hi = (a[axis].max(b[axis]) / 16 + i64::from(radius)).min(i64::from(self.size[axis]) - 1)
            as i32;
        for along in lo..=hi {
            let axial =
                (i64::from(along) * 16 + 8).clamp(a[axis].min(b[axis]), a[axis].max(b[axis]));
            let centre: [i32; 3] = std::array::from_fn(|i| {
                ((a[i] + d[i] * (axial - a[axis]) / axial_length) / 16) as i32
            });
            // At most sqrt(3) times the radius on an axial slice, including
            // endpoint caps. Every world cell is visited exactly once.
            for x in (centre[u] - 2 * radius).max(0)
                ..=(centre[u] + 2 * radius).min(self.size[u] as i32 - 1)
            {
                for y in (centre[v] - 2 * radius).max(0)
                    ..=(centre[v] + 2 * radius).min(self.size[v] as i32 - 1)
                {
                    let mut p = [0; 3];
                    p[axis] = along;
                    p[u] = x;
                    p[v] = y;
                    if contact == Some(p) {
                        continue;
                    }
                    let q: [i64; 3] = std::array::from_fn(|i| i64::from(p[i]) * 16 + 8 - a[i]);
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
