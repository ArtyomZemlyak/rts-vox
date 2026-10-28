//! In free fall, contiguous vertical grains have no static internal support.
//! Translate them together until contact, then return to independent slumping.
use super::*;

impl Flow {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn advance_column(
        &mut self,
        w: &World,
        changes: &mut BTreeMap<Pos, u16>,
        p: Pos,
        grain: Grain,
        blocked: &impl Fn(Pos) -> bool,
        checks: &mut u32,
        remaining: &mut u32,
        visited: &mut Positions,
    ) -> bool {
        if p[1] <= 1 || w.on_frame(p[0], p[1], p[2]) {
            return false;
        }
        // Do not turn supported piles into rigid columns or move them sideways
        // as a group. This batching is exact only during straight free fall.
        let below = [p[0], p[1] - 1, p[2]];
        if !matches!(free(w, changes, below, blocked, checks), Ok(true)) {
            return false;
        }
        let mut cells = vec![(p, grain.material)];
        for y in p[1] + 1..w.ny - 1 {
            if *checks == 0 {
                return false;
            }
            *checks -= 1;
            let q = [p[0], y, p[2]];
            let mat = read(w, changes, q);
            if self.tuning.material(mat).is_none()
                || blocked(q)
                || w.has_pending_dig(q[0], q[1], q[2])
            {
                break;
            }
            cells.push((q, mat));
        }
        // A single grain uses the ordinary path, including downhill motion.
        if cells.len() == 1 {
            return false;
        }
        let mut next = grain;
        next.speed = next
            .speed
            .add(self.gravity)
            .min(Fx::from_int(self.tuning.motion.max_steps as i32));
        next.fraction = next.fraction.add(next.speed);
        let steps = (next.fraction.to_int_trunc().max(0) as u32).min(self.tuning.motion.max_steps);
        let mut distance = 0;
        let mut landed = false;
        for i in 0..steps {
            if i != 0 {
                let q = [p[0], p[1] - distance - 1, p[2]];
                match free(w, changes, q, blocked, checks) {
                    Ok(true) => (),
                    Ok(false) => {
                        landed = true;
                        break;
                    }
                    Err(()) => break,
                }
            }
            distance += 1;
            next.fraction = next.fraction.sub(Fx::ONE);
        }
        // Compute only the changed boundary/layer cells. The source was
        // read above and the swept space below it was checked empty. Internal
        // equal material therefore needs neither another world lookup nor an
        // intermediate BTreeMap entry, even for a long uniform column.
        let mut plan = BTreeMap::new();
        let shift = distance as usize;
        for &(q, mat) in cells.iter().take(shift) {
            plan.insert([q[0], q[1] - distance, q[2]], mat);
        }
        for (i, &(q, mat)) in cells.iter().enumerate() {
            let next_mat = cells.get(i + shift).map_or(0, |&(_, m)| m);
            if mat != next_mat {
                plan.insert(q, next_mat);
            }
        }
        // Same edit ceiling as individual transfers (two writes per transfer).
        // A uniform column sliding one cell needs just two boundary writes.
        let cost = plan.len().div_ceil(2) as u32;
        let commit = cost <= *remaining;
        let to = if commit {
            [p[0], p[1] - distance, p[2]]
        } else {
            p
        };
        for &(q, _) in &cells {
            visited.insert(q);
            if self.active_positions.remove(&q) {
                self.active_dirty = true;
            }
            if commit {
                let target = [q[0], q[1] - distance, q[2]];
                visited.insert(target);
                if self.active_positions.remove(&target) {
                    self.active_dirty = true;
                }
            }
        }
        if commit {
            *remaining -= cost;
            self.moved += u64::from(cost);
            let size = [w.nx, w.ny, w.nz];
            for (q, mat) in plan {
                changes.insert(q, mat);
                self.wake(q, size);
            }
            // Do not accumulate a catch-up jump when the path budget ran out.
            next.fraction = next.fraction.min(Fx::ONE);
        }
        if !commit || !landed {
            self.active_positions.insert(to);
            self.active
                .push_back((to, if commit { next } else { grain }));
        }
        true
    }
}
