//! Sparse moving grains, shared gravity and bounded swept cellular paths.
//! A grain is still terrain, never a rigid-body entity or a render particle.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Grain {
    material: u16,
    speed: Fx,
    fraction: Fx,
}
struct Advance {
    to: Pos,
    grain: Option<Grain>,
}
impl Flow {
    pub(super) fn advance_grains(
        &mut self,
        w: &mut World,
        blocked: &impl Fn(Pos) -> bool,
        repose_bonus: &impl Fn(Pos) -> i32,
    ) {
        self.scanned = 0;
        self.path_checks = 0;
        let size = [w.nx, w.ny, w.nz];
        let mut changes = BTreeMap::new();
        let mut visited = Positions::default();
        let mut remaining = self.tuning.moves_per_tick;
        let mut checks = self.tuning.motion.path_checks_per_tick;
        // Moving cells get an early turn, while reserving discovery capacity
        // for newly unsupported neighbours. Neither queue can starve the other.
        let reserve = (remaining / 4).max(1).min(remaining.saturating_sub(1));
        let scans = self.tuning.scans_per_tick;
        let active_scans = (scans - scans / 4).max(1);
        let turns = self.active.len().min(self.tuning.motion.active_per_tick);
        for _ in 0..turns {
            if remaining <= reserve || self.scanned >= active_scans || checks == 0 {
                break;
            }
            let (p, grain) = self.active.pop_front().unwrap();
            self.scanned += 1;
            // An earlier column step already carried this old queue entry.
            // Its position may now hold the new leader; keep that marker.
            if visited.contains(&p) {
                continue;
            }
            self.active_positions.remove(&p);
            visited.insert(p);
            if read(w, &changes, p) != grain.material {
                continue; // An external edit replaced or removed this cell.
            }
            self.advance_one(
                w,
                &mut changes,
                p,
                grain,
                blocked,
                repose_bonus,
                &mut checks,
                &mut remaining,
                &mut visited,
            );
        }
        let turns = self.queue.len();
        for _ in 0..turns {
            if self.scanned >= scans || remaining == 0 || checks == 0 {
                break;
            }
            let Some(mut work) = self.queue.pop_front() else {
                break;
            };
            while work.cursor < CELLS && self.scanned < scans && remaining > 0 && checks > 0 {
                let k = work.cursor;
                let p = [
                    work.tile[0] * SIDE + k % SIDE,
                    work.tile[1] * SIDE + k / SIDE % SIDE,
                    work.tile[2] * SIDE + k / (SIDE * SIDE),
                ];
                work.cursor += 1;
                self.scanned += 1;
                if !inside(p, size) || visited.contains(&p) || self.active_positions.contains(&p) {
                    continue;
                }
                let mat = read(w, &changes, p);
                if self.tuning.material(mat).is_none() {
                    continue;
                }
                if self.active.len() >= self.tuning.motion.active_cells {
                    self.revisit.insert(work.tile);
                    continue;
                }
                visited.insert(p);
                let grain = Grain {
                    material: mat,
                    speed: Fx::ZERO,
                    fraction: Fx::ZERO,
                };
                self.advance_one(
                    w,
                    &mut changes,
                    p,
                    grain,
                    blocked,
                    repose_bonus,
                    &mut checks,
                    &mut remaining,
                    &mut visited,
                );
            }
            if work.cursor < CELLS {
                self.queue.push_back(work);
            } else {
                self.queued.remove(&work.tile);
                if self.revisit.remove(&work.tile) {
                    self.wake_tile(work.tile);
                }
            }
        }
        self.path_checks = self.tuning.motion.path_checks_per_tick - checks;
        if self.active_dirty {
            // Columns can absorb older individual entries that have not had
            // their turn. Keep only the current owner, including on snapshot.
            let mut seen = Positions::default();
            let mut live = VecDeque::new();
            while let Some(entry) = self.active.pop_back() {
                if self.active_positions.contains(&entry.0) && seen.insert(entry.0) {
                    live.push_front(entry);
                }
            }
            self.active = live;
            self.active_dirty = false;
        }
        apply(w, changes);
    }
    #[allow(clippy::too_many_arguments)]
    fn advance_one(
        &mut self,
        w: &World,
        changes: &mut BTreeMap<Pos, u16>,
        p: Pos,
        grain: Grain,
        blocked: &impl Fn(Pos) -> bool,
        bonus: &impl Fn(Pos) -> i32,
        checks: &mut u32,
        remaining: &mut u32,
        visited: &mut Positions,
    ) {
        let size = [w.nx, w.ny, w.nz];
        if !blocked(p)
            && !w.has_pending_dig(p[0], p[1], p[2])
            && self.advance_column(w, changes, p, grain, blocked, checks, remaining, visited)
        {
            return;
        }
        let result = if blocked(p) || w.has_pending_dig(p[0], p[1], p[2]) {
            Advance {
                to: p,
                grain: Some(Grain {
                    speed: Fx::ZERO,
                    fraction: Fx::ZERO,
                    ..grain
                }),
            }
        } else {
            self.sweep(w, changes, p, grain, blocked, bonus, checks)
        };
        if result.to != p {
            // One conservative transfer for the whole swept path; intermediate
            // empty cells need collision reads, not edits/remeshing events.
            move_cell(changes, p, result.to, grain.material);
            *remaining -= 1;
            self.moved += 1;
            self.wake(p, size);
            self.wake(result.to, size);
            visited.insert(result.to);
        }
        if let Some(grain) = result.grain {
            self.active_positions.insert(result.to);
            self.active.push_back((result.to, grain));
        }
    }
    fn sweep(
        &self,
        w: &World,
        changes: &BTreeMap<Pos, u16>,
        p: Pos,
        mut grain: Grain,
        blocked: &impl Fn(Pos) -> bool,
        bonus: &impl Fn(Pos) -> i32,
        checks: &mut u32,
    ) -> Advance {
        let max_steps = self.tuning.motion.max_steps;
        grain.speed = grain
            .speed
            .add(self.gravity)
            .min(Fx::from_int(max_steps as i32));
        // Dry sand flows readily down an exposed slope. Accelerate its
        // supported lateral motion without changing free-fall gravity or the
        // behaviour of cohesive and wet soil. The destination still checks
        // each swept cell and the same shared work budgets apply.
        let supported = p[1] > 1 && {
            let below = [p[0], p[1] - 1, p[2]];
            read(w, changes, below) != 0
                || w.has_pending_dig(below[0], below[1], below[2])
                || blocked(below)
        };
        let travel = if grain.material == 10 && supported && bonus(p) <= 0 {
            grain.speed.add(grain.speed)
        } else {
            grain.speed
        };
        grain.fraction = grain.fraction.add(travel);
        let steps = (grain.fraction.to_int_trunc().max(0) as u32).min(max_steps);
        let drop = self.tuning.material(grain.material).unwrap().repose_drop as i32;
        let mut to = p;
        for i in 0..steps.max(1) {
            let repose = (drop + bonus(to)).max(1) as u32;
            match destination(w, changes, to, repose, blocked, checks) {
                Ok(Some(next)) if i < steps => {
                    to = next;
                    grain.fraction = grain.fraction.sub(Fx::ONE);
                }
                Ok(Some(_)) => break, // Subvoxel motion accumulates before the first transfer.
                Ok(None) => return Advance { to, grain: None }, // Supported: friction dissipates motion.
                Err(()) => {
                    grain.fraction = grain.fraction.min(Fx::ONE);
                    break;
                } // Resume without exceeding the shared path budget.
            }
        }
        // High-speed sand can accumulate more distance than one bounded
        // sweep can consume. Keep the remainder within the same limit that
        // the motion snapshot loader accepts.
        grain.fraction = grain.fraction.min(Fx::from_int(max_steps as i32 * 2));
        Advance {
            to,
            grain: Some(grain),
        }
    }
    pub fn has_motion(&self) -> bool {
        !self.active.is_empty()
    }
    pub fn motion_at(&self, w: &World, mut p: Pos) -> Option<(Fx, Fx)> {
        if self.active.is_empty() {
            return None;
        }
        loop {
            self.tuning.material(w.material(p[0], p[1], p[2]))?;
            if self.active_positions.contains(&p) {
                return self
                    .active
                    .iter()
                    .find(|(at, _)| *at == p)
                    .map(|(_, grain)| (grain.speed, grain.fraction));
            }
            if p[1] == 0 {
                return None;
            }
            p[1] -= 1;
        }
    }
    pub fn save_motion(&self, b: &mut Blob) {
        b.u32(self.active.len() as u32);
        for (p, grain) in &self.active {
            for &v in p {
                b.u32(v);
            }
            b.u16(grain.material);
            b.fx(grain.speed);
            b.fx(grain.fraction);
        }
    }
    pub fn load_motion(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        self.active.clear();
        self.active_positions.clear();
        let count = s.u32() as usize;
        if count > self.tuning.motion.active_cells || count > s.left() / 30 {
            return None;
        }
        let limit = Fx::from_int(self.tuning.motion.max_steps as i32);
        for _ in 0..count {
            let p = std::array::from_fn(|_| s.u32());
            let grain = Grain {
                material: s.u16(),
                speed: s.fx(),
                fraction: s.fx(),
            };
            if !inside(p, size)
                || self.tuning.material(grain.material).is_none()
                || grain.speed < Fx::ZERO
                || grain.speed > limit
                || grain.fraction < Fx::ZERO
                || grain.fraction > limit.add(limit)
                || !self.active_positions.insert(p)
            {
                return None;
            }
            self.active.push_back((p, grain));
        }
        s.ok().then_some(())
    }
    #[cfg(test)]
    pub fn save(&self, b: &mut Blob) {
        self.save_legacy(b);
        self.save_motion(b);
    }
    #[cfg(test)]
    pub fn load(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        self.load_legacy(s, size)?;
        self.load_motion(s, size)
    }
}

#[path = "range_granular_columns.rs"]
mod columns;

fn free(
    w: &World,
    changes: &BTreeMap<Pos, u16>,
    p: Pos,
    blocked: &impl Fn(Pos) -> bool,
    checks: &mut u32,
) -> Result<bool, ()> {
    if *checks == 0 {
        return Err(());
    }
    *checks -= 1;
    Ok(!w.on_frame(p[0], p[1], p[2])
        && read(w, changes, p) == 0
        && !w.has_pending_dig(p[0], p[1], p[2])
        && !blocked(p))
}
fn destination(
    w: &World,
    changes: &BTreeMap<Pos, u16>,
    p: Pos,
    drop: u32,
    blocked: &impl Fn(Pos) -> bool,
    checks: &mut u32,
) -> Result<Option<Pos>, ()> {
    if w.on_frame(p[0], p[1], p[2]) || p[1] <= 1 {
        return Ok(None);
    }
    let below = [p[0], p[1] - 1, p[2]];
    if free(w, changes, below, blocked, checks)? {
        return Ok(Some(below));
    }
    let size = [w.nx, w.ny, w.nz];
    let mut best = None;
    for d in DIRS {
        let side = [p[0] as i32 + d[0], p[1] as i32, p[2] as i32 + d[2]];
        if side.iter().any(|&v| v < 1) {
            continue;
        }
        let side = side.map(|v| v as u32);
        if !inside(side, size) || p[1] < drop + 1 {
            continue;
        }
        if !free(w, changes, side, blocked, checks)? {
            continue;
        }
        let mut open = true;
        for n in 1..=drop {
            if !free(w, changes, [side[0], side[1] - n, side[2]], blocked, checks)? {
                open = false;
                break;
            }
        }
        if !open {
            continue;
        }
        let q = [side[0], side[1] - 1, side[2]];
        if best.is_none_or(|old| linear(q, size) < linear(old, size)) {
            best = Some(q);
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Empty;
    impl voxel::Generator for Empty {
        fn material(&self, _: u32, _: u32, _: u32) -> u16 {
            0
        }
    }
    #[test]
    fn fast_supported_sand_never_produces_an_unloadable_motion_remainder() {
        let w = World::new(64, 64, 64, 1024, Box::new(Empty));
        let flow = Flow::new();
        let grain = Grain {
            material: 10,
            speed: Fx::from_int(8),
            fraction: Fx::from_int(16),
        };
        let mut checks = 1000;
        let result = flow.sweep(
            &w,
            &BTreeMap::new(),
            [30, 30, 30],
            grain,
            &|p| p == [30, 29, 30],
            &|_| 0,
            &mut checks,
        );
        let next = result.grain.expect("moving sand");
        assert!(next.fraction <= Fx::from_int(flow.tuning.motion.max_steps as i32 * 2));
    }
}
