//! Conservative free-flight advection of contiguous wet columns, in Fx.
//! Velocity is water state; FIFO turns only schedule bounded collision work.
use super::*;

#[derive(Clone, Copy, Default)]
pub(super) struct Fall {
    speed: Fx,
    fraction: Fx,
    tick: u64,
}
impl Fall {
    pub(super) fn at(tick: u64) -> Self {
        Self {
            tick,
            ..Self::default()
        }
    }
}

impl Water {
    /// Last integrated downward velocity of the connected free-fall column.
    /// Resting and horizontally flowing water do not have a velocity here.
    pub fn falling_speed_at(&self, mut p: Pos) -> Option<Fx> {
        for _ in 0..self.tuning.motion.column_cells {
            if self.amount(p) == 0 {
                return None;
            }
            if let Some(f) = self.falling.get(&p) {
                return Some(f.speed);
            }
            p[1] = p[1].checked_sub(1)?;
        }
        None
    }
    pub(super) fn advance_falling(
        &mut self,
        w: &World,
        obstacles: &ObstacleIndex,
        checks: &mut u32,
        airborne: &mut Positions,
        supported: &mut Positions,
    ) {
        use std::ops::Bound::{Excluded, Unbounded};
        let quota = (self.tuning.checks_per_tick / 2).max(1);
        let keys: Vec<_> = if let Some(cursor) = self.fall_cursor {
            self.falling
                .range((Excluded(cursor), Unbounded))
                .chain(self.falling.range(..=cursor))
                .take(quota)
                .map(|(&p, _)| p)
                .collect()
        } else {
            self.falling.keys().take(quota).copied().collect()
        };
        for p in keys {
            if *checks == 0 || self.transfers >= self.tuning.transfers_per_tick {
                break;
            }
            self.fall_cursor = Some(p);
            if self.amount(p) == 0 {
                self.falling.remove(&p);
                continue;
            }
            self.checks += 1;
            self.absorb(w, p, [w.nx, w.ny, w.nz]);
            if self.amount(p) > 0 && !Self::blocked(w, p, obstacles) {
                self.fall_column(w, p, obstacles, checks, airborne, supported);
            }
            self.wake(p);
        }
    }
    pub(super) fn fall_column(
        &mut self,
        w: &World,
        p: Pos,
        obstacles: &ObstacleIndex,
        checks: &mut u32,
        airborne: &mut Positions,
        supported: &mut Positions,
    ) -> bool {
        if supported.contains(&p) {
            return false;
        }
        let size = [w.nx, w.ny, w.nz];
        if *checks == 0 {
            self.wake(p);
            return true;
        }
        *checks -= 1;
        if neighbour(p, [0, -1, 0], size)
            .is_none_or(|q| Self::blocked(w, q, obstacles) || supported.contains(&q))
        {
            supported.insert(p);
            self.falling.remove(&p);
            return false;
        }
        let mut bottom = p;
        let mut path = Vec::new();
        // Find the bottom of this connected wet run. Pool columns are marked
        // together too, avoiding a repeated depth walk for every interior cell.
        loop {
            if *checks == 0 {
                self.wake(p);
                return true;
            }
            *checks -= 1;
            path.push(bottom);
            let Some(q) = neighbour(bottom, [0, -1, 0], size) else {
                for at in path {
                    supported.insert(at);
                    self.falling.remove(&at);
                }
                return false;
            };
            if Self::blocked(w, q, obstacles) || supported.contains(&q) {
                for at in path {
                    supported.insert(at);
                    self.falling.remove(&at);
                }
                return false;
            }
            if airborne.contains(&q) {
                self.wake(p);
                return true;
            }
            if self.amount(q) == 0 {
                break;
            }
            bottom = q;
            if path.len() >= self.tuning.motion.column_cells as usize {
                self.wake(p);
                return true;
            }
        }
        let mut column = Vec::new();
        let mut at = bottom;
        loop {
            if *checks == 0 {
                self.wake(p);
                return true;
            }
            *checks -= 1;
            let amount = self.amount(at);
            if amount == 0 {
                break;
            }
            if Self::blocked(w, at, obstacles) {
                self.wake(p);
                return true;
            }
            column.push((at, amount));
            if column.len() >= self.tuning.motion.column_cells as usize {
                break;
            }
            let Some(up) = neighbour(at, [0, 1, 0], size) else {
                break;
            };
            at = up;
        }
        let old = self.falling.get(&bottom).copied().unwrap_or(Fall {
            tick: self.contact_tick.saturating_sub(1),
            ..Fall::default()
        });
        let elapsed = self
            .contact_tick
            .saturating_sub(old.tick)
            .min(u64::from(w.ny)) as i32;
        let time = Fx::from_int(elapsed);
        // Exact constant-acceleration integral: dx=v*dt+g*dt²/2. Units here
        // are grid cells and fixed ticks; g=98/(20*20) for 100 mm cells.
        let distance = old
            .fraction
            .add(old.speed.mul(time))
            .add(self.gravity.mul(time).mul(time).div(Fx::from_int(2)));
        let speed = old.speed.add(self.gravity.mul(time));
        let requested = (distance.to_int_trunc().max(0) as u32).min(self.tuning.motion.max_steps);
        if *checks < requested {
            self.wake(p);
            return true;
        }
        let mut moved = 0;
        let mut landed = false;
        for depth in 1..=requested {
            *checks -= 1;
            let Some(y) = bottom[1].checked_sub(depth) else {
                landed = true;
                break;
            };
            let q = [bottom[0], y, bottom[2]];
            if !inside(q, size) || Self::blocked(w, q, obstacles) || self.amount(q) > 0 {
                landed = true;
                break;
            }
            moved = depth;
        }
        for &(q, _) in &column {
            airborne.insert(q);
            self.falling.remove(&q);
        }
        if moved > 0 {
            // Copy the finite column once. Overlapping source/destination
            // ranges are cleared before writing: no lost volume or compression.
            let heat: Vec<_> = column
                .iter()
                .map(|&(q, amount)| self.take_moving_heat(q, amount))
                .collect();
            for &(q, _) in &column {
                self.set(q, 0);
            }
            for (&(q, amount), thermal) in column.iter().zip(heat) {
                let to = [q[0], q[1] - moved, q[2]];
                self.set(to, amount);
                if thermal > 0 {
                    self.absorb_heat(to, thermal, size);
                }
                airborne.insert(to);
                self.transferred += u64::from(amount) * u64::from(moved);
                if moved == 1 {
                    self.flux.push(Flux {
                        from: q,
                        to,
                        units: amount,
                    });
                }
                self.wake_near(q, size);
                self.wake_near(to, size);
            }
            // One bounded coherent-volume operation, not one job per cube.
            self.transfers += 1;
        }
        let to = [bottom[0], bottom[1] - moved, bottom[2]];
        if !landed {
            self.falling.insert(
                to,
                Fall {
                    speed,
                    fraction: distance.sub(Fx::from_int(moved as i32)),
                    tick: self.contact_tick,
                },
            );
        }
        self.wake(to);
        true
    }
    pub fn has_motion(&self) -> bool {
        !self.falling.is_empty()
    }
    pub fn save_motion(&self, b: &mut Blob) {
        b.u32(self.falling.len() as u32);
        for (p, v) in &self.falling {
            for &x in p {
                b.u32(x);
            }
            b.fx(v.speed);
            b.fx(v.fraction);
            b.u64(v.tick);
        }
        b.bool(self.fall_cursor.is_some());
        if let Some(p) = self.fall_cursor {
            for x in p {
                b.u32(x);
            }
        }
    }
    pub fn load_motion(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        self.falling.clear();
        let count = s.u32() as usize;
        if count > self.cells.len() || count > s.left() / 36 {
            return None;
        }
        let mut last = None;
        let limit = Fx::from_int(size[1] as i32);
        for _ in 0..count {
            let p = std::array::from_fn(|_| s.u32());
            let v = Fall {
                speed: s.fx(),
                fraction: s.fx(),
                tick: s.u64(),
            };
            if !inside(p, size)
                || self.amount(p) == 0
                || last.is_some_and(|old| old >= p)
                || v.speed < Fx::ZERO
                || v.speed > limit
                || v.fraction < Fx::ZERO
                || v.fraction > limit
                || v.tick > self.contact_tick
            {
                return None;
            }
            self.falling.insert(p, v);
            last = Some(p);
        }
        self.fall_cursor = if s.bool() {
            let p = std::array::from_fn(|_| s.u32());
            if !inside(p, size) {
                return None;
            }
            Some(p)
        } else {
            None
        };
        s.ok().then_some(())
    }
    pub fn save(&self, b: &mut Blob) {
        self.save_legacy(b);
        self.save_motion(b);
        self.save_thermal_remainder(b);
    }
    pub fn load(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        self.load_legacy(s, size)?;
        self.load_motion(s, size)?;
        self.load_thermal_remainder(s, size)
    }
}
