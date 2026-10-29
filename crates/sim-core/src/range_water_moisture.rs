//! Absorption and phase bookkeeping share the exact liquid volume ledger.
use super::*;
use crate::heat::Heat;
impl Water {
    pub(crate) fn remaining_absorption(&self, p: Pos) -> u64 {
        (u64::from(self.amount(p)) * u64::from(self.tuning.evaporation_heat)).saturating_sub(
            u64::from(self.thermal_remainder.get(&p).copied().unwrap_or(0)),
        )
    }
    pub(crate) fn thermal_region(&self, lo: [i32; 3], hi: [i32; 3], limit: usize) -> Vec<Pos> {
        if self.cells.is_empty() || lo[0] > hi[0] || hi[0] < 0 {
            return Vec::new();
        }
        let x0 = lo[0].max(0) as u32;
        let x1 = hi[0].max(0) as u32;
        self.cells
            .range([x0, 0, 0]..=[x1, u32::MAX, u32::MAX])
            .take(self.tuning.checks_per_tick.min(512))
            .map(|(&k, _)| key(k))
            .filter(|p| {
                (0..3).all(|a| {
                    i64::from(p[a]) >= i64::from(lo[a]) && i64::from(p[a]) <= i64::from(hi[a])
                })
            })
            .take(limit)
            .collect()
    }
    /// Consume heat at an exposed water cell. The returned heat is absorbed;
    /// vapour stays in the same conserved volume ledger as liquid and moisture.
    pub(crate) fn absorb_heat(&mut self, p: Pos, heat: u32, size: Pos) -> u32 {
        let amount = self.amount(p);
        if amount == 0 || heat == 0 {
            return 0;
        }
        let previous = self.thermal_remainder.remove(&p).unwrap_or(0);
        let total = u64::from(previous) + u64::from(heat);
        let latent = u64::from(self.tuning.evaporation_heat);
        let n = u64::from(amount).min(total / latent) as u32;
        if n > 0 {
            self.set(p, amount - n);
            self.vapor += u64::from(n);
            self.record_vapor(p, n);
            self.wake_near(p, size);
        }
        if n < amount {
            let remainder = (total % latent) as u32;
            if remainder > 0 {
                self.thermal_remainder.insert(p, remainder);
            }
            heat
        } else {
            (u64::from(amount) * latent)
                .saturating_sub(u64::from(previous))
                .min(u64::from(heat)) as u32
        }
    }
    pub(super) fn take_moving_heat(&mut self, p: Pos, moved: u32) -> u32 {
        let amount = self.amount(p);
        let heat = self.thermal_remainder.remove(&p).unwrap_or(0);
        if heat == 0 || amount == 0 {
            return 0;
        }
        let share = if moved >= amount {
            heat
        } else {
            (u64::from(heat) * u64::from(moved) / u64::from(amount)) as u32
        };
        if heat > share {
            self.thermal_remainder.insert(p, heat - share);
        }
        share
    }
    pub(crate) fn has_thermal_remainder(&self) -> bool {
        !self.thermal_remainder.is_empty()
    }
    pub(crate) fn save_thermal_remainder(&self, b: &mut Blob) {
        b.u32(self.thermal_remainder.len() as u32);
        for (&p, &heat) in &self.thermal_remainder {
            for v in p {
                b.u32(v);
            }
            b.u32(heat);
        }
    }
    pub(crate) fn load_thermal_remainder(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        self.thermal_remainder.clear();
        let count = s.u32() as usize;
        if count > self.cells.len() || count > s.left() / 16 {
            return None;
        }
        let mut last = None;
        for _ in 0..count {
            let p = std::array::from_fn(|_| s.u32());
            let h = s.u32();
            if !inside(p, size)
                || self.amount(p) == 0
                || h == 0
                || h >= self.tuning.evaporation_heat
                || last.is_some_and(|q| q >= p)
            {
                return None;
            }
            self.thermal_remainder.insert(p, h);
            last = Some(p);
        }
        s.ok().then_some(())
    }
    fn record_vapor(&mut self, p: Pos, n: u32) {
        if let Some(last) = self.vapor_events.last_mut().filter(|e| e.0 == p) {
            last.1 = last.1.saturating_add(n);
        } else if self.vapor_events.len() < 32 {
            self.vapor_events.push((p, n));
        }
    }
    /// A bounded thermal flash over existing water, without scanning empty
    /// world volume. A solid wall blocks the ray; the per-event work limit
    /// bounds the approximation for large pools and explosions.
    pub(crate) fn absorb_radial_heat(&mut self, w: &World, at: [i32; 3], radius: i32, heat: u32) {
        if self.cells.is_empty() || radius <= 0 || heat == 0 {
            return;
        }
        let radius = i64::from(radius);
        let r2 = radius * radius;
        let x0 = (i64::from(at[0]) - radius).max(0) as u32;
        let x1 = (i64::from(at[0]) + radius)
            .min(i64::from(w.nx.saturating_sub(1)))
            .max(0) as u32;
        if x0 > x1 {
            return;
        }
        let candidates: Vec<_> = self
            .cells
            .range([x0, 0, 0]..=[x1, u32::MAX, u32::MAX])
            .take(self.tuning.checks_per_tick)
            .filter_map(|(&k, _)| {
                let p = key(k);
                let d = std::array::from_fn::<_, 3, _>(|a| i64::from(p[a]) - i64::from(at[a]));
                let d2 = d.iter().map(|v| v * v).sum::<i64>();
                (d2 < r2).then_some((p, d, d2))
            })
            .take(self.tuning.moisture_checks)
            .collect();
        let mut rays = self.tuning.moisture_checks.saturating_mul(8);
        for (p, d, d2) in candidates {
            let steps = d.iter().map(|v| v.abs()).max().unwrap_or(0) as usize;
            if steps.saturating_sub(1) > rays {
                continue;
            }
            let mut visible = true;
            for i in 1..steps {
                rays -= 1;
                let q = std::array::from_fn::<_, 3, _>(|a| {
                    i64::from(at[a]) + d[a] * i as i64 / steps as i64
                });
                if q.iter().any(|v| *v < 0)
                    || (0..3).any(|a| q[a] >= i64::from([w.nx, w.ny, w.nz][a]))
                    || w.solid(q[0] as u32, q[1] as u32, q[2] as u32)
                {
                    visible = false;
                    break;
                }
            }
            if visible {
                let dose = (u64::from(heat) * (r2 - d2) as u64 / r2 as u64) as u32;
                self.absorb_heat(p, dose, [w.nx, w.ny, w.nz]);
            }
        }
    }
    pub fn moisture(&self, p: Pos) -> u32 {
        self.wet.get(&p).map_or(0, |v| v.1)
    }
    /// Read-only wet cells for bounded diagnostic presentation.
    pub fn diagnostic_wet_cells(&self) -> impl Iterator<Item=(Pos,u16,u32)> + '_ {
        self.wet.iter().map(|(&p,&(mat,amount))|(p,mat,amount))
    }
    pub fn repose_bonus(&self, p: Pos) -> i32 {
        let Some(&(mat, n)) = self.wet.get(&p) else {
            return 0;
        };
        self.tuning
            .porous
            .iter()
            .find(|v| v.material == mat)
            .map_or(0, |v| {
                if n >= v.capacity / 2 {
                    v.repose_bonus
                } else {
                    0
                }
            })
    }
    pub(super) fn absorb(&mut self, w: &World, p: Pos, size: Pos) {
        for d in DIRS {
            if self.amount(p) == 0 {
                break;
            }
            let Some(q) = neighbour(p, d, size) else {
                continue;
            };
            let mat = w.material(q[0], q[1], q[2]);
            let Some(porous) = self.tuning.porous.iter().find(|v| v.material == mat) else {
                continue;
            };
            if !self.wet.contains_key(&q) && self.wet.len() >= self.tuning.max_cells {
                continue;
            }
            let old = self.moisture(q);
            let n = porous
                .capacity
                .saturating_sub(old)
                .min(porous.rate)
                .min(self.amount(p));
            if n == 0 {
                continue;
            }
            self.set(p, self.amount(p) - n);
            self.wet.insert(q, (mat, old + n));
            self.absorbed += u64::from(n);
            self.wet_changed.push(q);
            self.wake(p);
        }
    }
    /// Capillary equalisation reads only the beginning-of-phase moisture.
    /// At most six neighbours per scheduled source, never a scan of the map.
    /// New wet cells cannot propagate again until a later step.
    fn wick(&mut self, w: &World, sources: &[Pos]) -> usize {
        if self.tuning.capillary_units == 0 {
            return 0;
        }
        let mut transfers = Vec::new();
        let mut checks = 0;
        let size = [w.nx, w.ny, w.nz];
        for &p in sources {
            let Some(&(mat, amount)) = self.wet.get(&p) else {
                continue;
            };
            if w.material(p[0], p[1], p[2]) != mat {
                continue;
            }
            let Some(source) = self.tuning.porous.iter().find(|v| v.material == mat) else {
                continue;
            };
            let mut available = amount;
            for d in DIRS {
                checks += 1;
                let Some(q) = neighbour(p, d, size) else {
                    continue;
                };
                let target_mat = w.material(q[0], q[1], q[2]);
                let Some(target) = self.tuning.porous.iter().find(|v| v.material == target_mat)
                else {
                    continue;
                };
                let old = match self.wet.get(&q) {
                    Some(&(wet_mat, n)) if wet_mat == target_mat => n,
                    Some(_) => continue, // The removed material must release its own water first.
                    None => 0,
                };
                // Equal saturation, not equal volume: materials have different capacities.
                let excess = (u64::from(amount) * u64::from(target.capacity))
                    .saturating_sub(u64::from(old) * u64::from(source.capacity));
                let equalise = excess / (u64::from(source.capacity) + u64::from(target.capacity));
                let n = equalise
                    .min(u64::from(
                        source
                            .rate
                            .min(target.rate)
                            .min(self.tuning.capillary_units),
                    ))
                    .min(u64::from(available)) as u32;
                if n > 0 {
                    transfers.push((p, q, target_mat, target.capacity, n));
                    available -= n;
                }
            }
        }
        // Serial commit resolves competing destinations in stable source/face order.
        for (p, q, target_mat, capacity, n) in transfers {
            if !self.wet.contains_key(&q) && self.wet.len() >= self.tuning.max_cells {
                continue;
            }
            let Some(&(mat, amount)) = self.wet.get(&p) else {
                continue;
            };
            let old = self.moisture(q);
            let n = n.min(amount).min(capacity.saturating_sub(old));
            if n == 0 {
                continue;
            }
            if n == amount {
                self.wet.remove(&p);
            } else {
                self.wet.insert(p, (mat, amount - n));
            }
            self.wet.insert(q, (target_mat, old + n));
            self.wet_changed.extend([p, q]);
            // absorbed is unchanged: both cells remain in the same reservoir.
        }
        checks
    }
    pub(super) fn dry(&mut self, w: &World) {
        use std::ops::Bound::{Excluded, Unbounded};
        let start = self.wet_cursor;
        let positions: Vec<_> = self
            .wet
            .range((start.map_or(Unbounded, Excluded), Unbounded))
            .chain(
                self.wet
                    .iter()
                    .take_while(|(p, _)| start.is_some_and(|s| **p <= s)),
            )
            .take(self.tuning.moisture_checks)
            .map(|(&p, _)| p)
            .collect();
        self.wick(w, &positions);
        for p in positions {
            self.wet_cursor = Some(p);
            let Some(&(mat, amount)) = self.wet.get(&p) else {
                continue;
            };
            if w.material(p[0], p[1], p[2]) != mat {
                // Detached grains release pore water at their source. It is
                // not silently carried by a different material or destroyed.
                if self.amount(p) == 0 && self.cells.len() >= self.tuning.max_cells {
                    continue;
                }
                let n = amount.min(self.tuning.max_cell_units - self.amount(p));
                self.set(p, self.amount(p) + n);
                self.absorbed -= u64::from(n);
                if n == amount {
                    self.wet.remove(&p);
                } else {
                    self.wet.insert(p, (mat, amount - n));
                }
                self.wake_near(p, [w.nx, w.ny, w.nz]);
                self.wet_changed.push(p);
            } else {
                let exposed = DIRS.iter().any(|&d| {
                    neighbour(p, d, [w.nx, w.ny, w.nz])
                        .is_some_and(|q| !w.solid(q[0], q[1], q[2]) && self.amount(q) == 0)
                });
                let n = if exposed {
                    amount.min(self.tuning.drying_units)
                } else {
                    0
                };
                self.absorbed -= u64::from(n);
                self.vapor += u64::from(n);
                if n == amount {
                    self.wet.remove(&p);
                } else {
                    self.wet.insert(p, (mat, amount - n));
                }
                if n > 0 {
                    self.wet_changed.push(p);
                }
            }
        }
    }
    /// Bounded heat contact: pore water and neighbouring liquid spend real
    /// volume on cooling. Vapour remains counted; gas transport is separate.
    pub fn cool(&mut self, w: &World, heat: &mut Heat, mats: &Registry) {
        for p in heat.batch_after(self.thermal_cursor, self.tuning.moisture_checks) {
            self.thermal_cursor = Some(p);
            let mut left = heat.cell(p).heat / self.tuning.evaporation_heat;
            if left == 0 {
                continue;
            }
            let mut used = 0;
            if let Some(&(mat, n)) = self.wet.get(&p) {
                let take = n.min(left);
                if take > 0 {
                    self.record_vapor(p, take);
                }
                left -= take;
                used += take;
                self.absorbed -= u64::from(take);
                if take == n {
                    self.wet.remove(&p);
                } else {
                    self.wet.insert(p, (mat, n - take));
                }
                self.wet_changed.push(p);
            }
            for d in DIRS {
                let Some(q) = neighbour(p, d, [w.nx, w.ny, w.nz]) else {
                    continue;
                };
                let take = self.amount(q).min(left);
                if take == 0 {
                    continue;
                }
                self.set(q, self.amount(q) - take);
                self.wake_near(q, [w.nx, w.ny, w.nz]);
                self.record_vapor(q, take);
                used += take;
                left -= take;
            }
            if used > 0 {
                self.vapor += u64::from(used);
                let kindling = mats
                    .get(w.material(p[0], p[1], p[2]))
                    .map_or(0, |m| m.kindling);
                heat.quench(p, used * self.tuning.evaporation_heat, kindling);
            }
        }
    }
    pub(super) fn save_moisture(&self, b: &mut Blob) {
        b.u64(self.vapor);
        for cursor in [self.wet_cursor, self.thermal_cursor] {
            b.bool(cursor.is_some());
            if let Some(p) = cursor {
                for v in p {
                    b.u32(v);
                }
            }
        }
        b.u32(self.wet.len() as u32);
        for (&p, &(mat, n)) in &self.wet {
            for v in p {
                b.u32(v);
            }
            b.u16(mat);
            b.u32(n);
        }
    }
    pub(super) fn load_moisture(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        self.vapor = s.u64();
        for cursor in [&mut self.wet_cursor, &mut self.thermal_cursor] {
            if s.bool() {
                let p = std::array::from_fn(|_| s.u32());
                if !inside(p, size) {
                    return None;
                }
                *cursor = Some(p);
            }
        }
        let count = s.u32() as usize;
        if count > self.tuning.max_cells || count > s.left() / 18 {
            return None;
        }
        let mut last = None;
        for _ in 0..count {
            let p = std::array::from_fn(|_| s.u32());
            let mat = s.u16();
            let n = s.u32();
            let capacity = self
                .tuning
                .porous
                .iter()
                .find(|m| m.material == mat)?
                .capacity;
            if !inside(p, size) || n == 0 || n > capacity || last.is_some_and(|old| old >= p) {
                return None;
            }
            self.wet.insert(p, (mat, n));
            self.absorbed += u64::from(n);
            last = Some(p);
        }
        s.ok().then_some(())
    }
}

#[cfg(test)]
mod thermal_tests {
    use super::*;
    struct Empty;
    impl voxel::Generator for Empty {
        fn material(&self, _x: u32, _y: u32, _z: u32) -> u16 {
            0
        }
    }
    #[test]
    fn small_heat_accumulates_moves_with_water_and_survives_save() {
        let _w = World::new(32, 32, 32, 4096, Box::new(Empty));
        let mut a = Water::new();
        let p = [10, 10, 10];
        let q = [10, 9, 10];
        assert!(a.edit_volume(p, true));
        let total = a.stats().units;
        for _ in 0..a.tuning.evaporation_heat - 1 {
            assert_eq!(a.absorb_heat(p, 1, [32; 3]), 1);
        }
        assert_eq!(a.stats().vapor_units, 0);
        let mut bytes = Blob::new();
        a.save(&mut bytes);
        let mut b = Water::new();
        b.load(&mut Scan::new(&bytes.v), [32; 3]).unwrap();
        for f in [&mut a, &mut b] {
            let amount = f.amount(p);
            assert_eq!(f.transfer(p, q, amount, [32; 3]), amount);
            assert_eq!(f.absorb_heat(q, 1, [32; 3]), 1);
            assert_eq!(f.stats().vapor_units, 1);
            assert_eq!(f.stats().units, total);
        }
        let mut x = Blob::new();
        let mut y = Blob::new();
        a.save(&mut x);
        b.save(&mut y);
        assert_eq!(x.v, y.v);
    }
    #[test]
    fn radial_heat_caps_candidate_work_and_preserves_water() {
        let w = World::new(64, 64, 64, 4096, Box::new(Empty));
        let mut f = Water::new();
        for x in 20..36 {
            for z in 20..36 {
                for y in 20..22 {
                    assert!(f.edit_volume([x, y, z], true));
                }
            }
        }
        let total = f.stats().units;
        f.absorb_radial_heat(&w, [28, 20, 28], 16, 80);
        let v = f.stats().vapor_units;
        assert!(v > 0);
        assert!(v <= f.tuning.moisture_checks as u64 * (80 / u64::from(f.tuning.evaporation_heat)));
        assert_eq!(f.stats().units, total);
    }
}

#[cfg(test)]
#[path = "range_water_capillary_tests.rs"]
mod tests;
