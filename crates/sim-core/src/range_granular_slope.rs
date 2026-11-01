//! Bounded downhill contact and rolling for compact bodies on loose terrain.
//! This is a cellular approximation, not a general rigid-body solver.
use super::*;
use voxel::bodies::Body;

impl Flow {
    /// Conservative maximum for one complete slope query, including the
    /// swept hull, rotation and sand shearing.
    pub(in crate::blast) fn slope_query_budget(&self, b: &Body, occupied: usize) -> u32 {
        let t = self.tuning.slope;
        u32::from(b.sx) * u32::from(b.sz)
            + b.voxels.len() as u32 * 4
            + (b.voxels.len() + occupied) as u32 * 4 * t.reach * 2
            + 2 * (occupied as u32 + 2)
            + t.scuff_moves_per_step * 8
    }
    pub fn slope_candidate(&self, b: &Body) -> bool {
        !b.resting && (b.tilt_axis == 0 || b.tilt_axis >= 3)
            && b.fallen > 0
            && b.vy >= Fx::ZERO
            && b.riders.is_empty()
            && b.voxels.len() <= self.tuning.slope.max_cells
            && u32::from(b.height()) <= 2 * u32::from(b.sx.min(b.sz))
            && b.voxels
                .iter()
                .all(|v| self.tuning.material(v.mat).is_none())
    }
    /// Four footprint corners distinguish a glancing slope contact from a
    /// straight impact into a flat bed. Do not excavate a pocket before the
    /// tangential contact solver has had a chance to carry the body downhill.
    pub(super) fn glancing_landing(
        &self,
        b: &Body,
        w: &World,
        reach: u32,
        checks: &mut u32,
    ) -> bool {
        if !self.slope_candidate(b) {
            return false;
        }
        if b.tilt_axis >= 3 {
            // The dedicated rolling contact handles the support and shearing.
            return true;
        }
        let sx = usize::from(b.sx);
        let sz = usize::from(b.sz);
        let mut nearest = reach + 1;
        let mut farthest = 0;
        let mut loose = true;
        for column in [0, sx - 1, (sz - 1) * sx, sx * sz - 1] {
            let Some([x, y, z]) = b.floor_contact(column) else {
                continue;
            };
            if x < 1 || z < 1 || y < 2 {
                return false;
            }
            let mut depth = reach + 1;
            let mut mat = 0;
            for n in 1..=reach.min((y - 1) as u32) {
                if *checks == 0 {
                    return false;
                }
                *checks -= 1;
                mat = w.material(x as u32, (y - i64::from(n)) as u32, z as u32);
                if mat != 0 {
                    depth = n;
                    break;
                }
            }
            let sand = self
                .tuning
                .material(mat)
                .is_some_and(|m| m.repose_drop == 1);
            if depth < nearest {
                nearest = depth;
                loose = sand;
            } else if depth == nearest {
                loose &= sand;
            }
            farthest = farthest.max(depth);
        }
        nearest <= reach && farthest > nearest && loose
    }
    /// `true` keeps the body dynamic for this tick. Only a completely clear
    /// swept pose with a lower potential height can move it; level ground rests.
    /// The caller supplies conservative bounds of other bodies and unit hulls.
    pub fn slide_body(
        &mut self,
        b: &mut Body,
        w: &mut World,
        checks: &mut u32,
        occupied: &[(u64, [i64; 3], [i64; 3])],
    ) -> bool {
        let t = self.tuning.slope;
        if !self.slope_candidate(b) {
            return false;
        }
        let area = usize::from(b.sx) * usize::from(b.sz);
        // Even an adversarial body cannot restart a query larger than a full
        // phase budget forever. Larger footprints use ordinary vertical motion.
        let worst = self.slope_query_budget(b, occupied.len());
        if worst > t.checks_per_tick {
            return false;
        }
        if *checks < worst {
            return true;
        }
        let floor = if b.tilt_axis >= 3 {
            let mut columns = BTreeMap::<(i64, i64), i64>::new();
            for &v in &b.voxels {
                *checks -= 1;
                let (x, y, z) = b.world_of(v);
                columns
                    .entry((x, z))
                    .and_modify(|h| *h = (*h).min(y))
                    .or_insert(y);
            }
            columns
                .into_iter()
                .map(|((x, z), y)| [x, y, z])
                .collect::<Vec<_>>()
        } else {
            (0..area)
                .filter_map(|i| b.floor_contact(i))
                .collect::<Vec<_>>()
        };
        let mut supported = 0u32;
        for &[x, y, z] in &floor {
            if x < 1 || y < 2 || z < 1 || x + 1 >= i64::from(w.nx) || z + 1 >= i64::from(w.nz) {
                return false;
            }
            *checks -= 1;
            let mat = w.material(x as u32, (y - 1) as u32, z as u32);
            if mat == 0 {
                continue;
            }
            // Cohesive/wet soils need a larger yield than this shallow slip.
            if self.tuning.material(mat).is_none_or(|m| m.repose_drop != 1) {
                return false;
            }
            supported += 1;
        }
        if supported == 0 {
            return false;
        }
        let origin = [b.ox, b.oy, b.oz];
        let mut best: Option<([i64; 3], u32)> = None;
        for d in DIRS {
            for distance in 1..=t.reach {
                let at = [
                    origin[0] + i64::from(d[0]) * i64::from(distance),
                    origin[1],
                    origin[2] + i64::from(d[2]) * i64::from(distance),
                ];
                // Every intermediate horizontal pose and the downward pose are
                // checked: no tunnelling through a lip, a unit or another rock.
                if !clear(b, w, at, checks, occupied) {
                    break;
                }
                let down = [at[0], at[1] - 1, at[2]];
                if clear(b, w, down, checks, occupied) {
                    let key = (distance, down[2], down[1], down[0]);
                    if best.is_none_or(|(p, n)| key < (n, p[2], p[1], p[0])) {
                        best = Some((down, distance));
                    }
                    break;
                }
            }
        }
        let Some((to, distance)) = best else {
            return false;
        };
        // Project gravity onto the local stair slope and dissipate the normal
        // impact speed. The existing saved Fx velocity/fraction carries motion;
        // no wall-clock or extra per-grain entity enters the simulation.
        b.vy =
            b.vy.min(Fx::ONE)
                .add(self.gravity.div(Fx::from_int(distance as i32 + 1)))
                .min(Fx::ONE);
        b.fy = b.fy.add(b.vy);
        if b.fy >= Fx::ONE {
            self.scuff(b, w, to, checks, occupied);
            b.ox = to[0];
            b.oy = to[1];
            b.oz = to[2];
            b.fallen += 1;
            b.fy = b.fy.sub(Fx::ONE);
            if u64::from(supported) * 1000 < floor.len() as u64 * u64::from(t.roll_support_milli) {
                self.roll(b, w, origin, to, checks, occupied);
            }
        }
        true
    }
    fn roll(
        &self,
        b: &mut Body,
        w: &World,
        from: [i64; 3],
        to: [i64; 3],
        checks: &mut u32,
        occupied: &[(u64, [i64; 3], [i64; 3])],
    ) {
        let (axis, direction, radius) = if to[0] != from[0] {
            (
                4,
                -(to[0] - from[0]).signum() as i32,
                (b.sx.max(b.height()) / 2).max(1),
            )
        } else {
            (
                3,
                (to[2] - from[2]).signum() as i32,
                (b.sz.max(b.height()) / 2).max(1),
            )
        };
        if b.tilt_axis != 0 && b.tilt_axis != axis {
            return;
        }
        let need = (b.voxels.len() as u32 + occupied.len() as u32) * 2;
        if *checks < need {
            return;
        }
        let previous = (
            b.tilt_axis,
            b.tilt_angle,
            b.tilt_speed,
            b.pivot_x,
            b.pivot_z,
        );
        b.tilt_axis = axis;
        b.pivot_x = 0;
        b.pivot_z = 0;
        let step = (self.tuning.slope.roll_turn_per_cell / u32::from(radius)).max(1) as i32;
        b.tilt_speed = direction * step;
        // Check the intermediate angle too: a rotating corner must not pass
        // through a wall just because its final orientation is clear.
        for delta in [step / 2, step - step / 2] {
            b.tilt_angle = (b.tilt_angle + direction * delta).rem_euclid(65536);
            if !clear(b, w, to, checks, occupied) {
                (
                    b.tilt_axis,
                    b.tilt_angle,
                    b.tilt_speed,
                    b.pivot_x,
                    b.pivot_z,
                ) = previous;
                break;
            }
        }
    }
    /// Shear a bounded chain along each edge of the track. A displaced cell
    /// pushes its loose neighbours into actual free space; ordinary granular
    /// gravity continues the disturbance after this contact.
    pub(in crate::blast) fn scuff(
        &mut self,
        b: &Body,
        w: &mut World,
        to: [i64; 3],
        checks: &mut u32,
        occupied: &[(u64, [i64; 3], [i64; 3])],
    ) {
        let along_x = to[0] != b.ox;
        let mut floor = BTreeMap::<(i64, i64), i64>::new();
        let mut body_lo = [i64::MAX; 3];
        let mut body_hi = [i64::MIN; 3];
        for &v in &b.voxels {
            if *checks == 0 {
                return;
            }
            *checks -= 1;
            let (x, y, z) = b.world_of(v);
            for (a, p) in [x, y, z].into_iter().enumerate() {
                body_lo[a] = body_lo[a].min(p);
                body_hi[a] = body_hi[a].max(p + 1);
            }
            floor
                .entry((x, z))
                .and_modify(|h| *h = (*h).min(y))
                .or_insert(y);
        }
        let mut points: Vec<_> = floor.into_iter().map(|((x, z), y)| [x, y, z]).collect();
        let mut changes = BTreeMap::new();
        let mut remaining = self.tuning.slope.scuff_moves_per_step;
        let size = [w.nx, w.ny, w.nz];
        for side in [0, 1] {
            if remaining == 0 {
                break;
            }
            // Start at the trailing corner, then move outward from the track.
            points.sort_by_key(|p| {
                let rear = if along_x {
                    p[0] * (to[0] - b.ox).signum()
                } else {
                    p[2] * (to[2] - b.oz).signum()
                };
                let edge = if along_x { p[2] } else { p[0] };
                (rear, if side == 0 { edge } else { -edge }, p[1])
            });
            for foot in &points {
                if *checks == 0 {
                    break;
                }
                let start = [foot[0], foot[1] - 1, foot[2]];
                if start.iter().any(|&v| v < 1)
                    || start[0] + 1 >= i64::from(size[0])
                    || start[1] + 1 >= i64::from(size[1])
                    || start[2] + 1 >= i64::from(size[2])
                {
                    continue;
                }
                let p = start.map(|v| v as u32);
                *checks -= 1;
                let mat = read(w, &changes, p);
                if self.tuning.material(mat).is_none_or(|m| m.repose_drop != 1)
                    || w.has_pending_dig(p[0], p[1], p[2])
                {
                    continue;
                }
                let delta = if along_x {
                    [0, 0, if side == 0 { -1 } else { 1 }]
                } else {
                    [if side == 0 { -1 } else { 1 }, 0, 0]
                };
                let mut chain = vec![p];
                for n in 1..=self.tuning.push_reach.min(remaining) {
                    if *checks == 0 {
                        break;
                    }
                    let pos: [i64; 3] =
                        std::array::from_fn(|a| start[a] + i64::from(delta[a]) * i64::from(n));
                    if pos.iter().any(|&v| v < 1)
                        || pos[0] + 1 >= i64::from(size[0])
                        || pos[1] + 1 >= i64::from(size[1])
                        || pos[2] + 1 >= i64::from(size[2])
                    {
                        break;
                    }
                    let q = pos.map(|v| v as u32);
                    *checks -= 1;
                    if w.has_pending_dig(q[0], q[1], q[2]) {
                        break;
                    }
                    if (0..3).all(|a| pos[a] >= body_lo[a] && pos[a] < body_hi[a]) {
                        break;
                    }
                    let mut blocked = false;
                    for &(id, lo, hi) in occupied {
                        if *checks == 0 {
                            blocked = true;
                            break;
                        }
                        *checks -= 1;
                        if id != b.id && (0..3).all(|a| pos[a] >= lo[a] && pos[a] < hi[a]) {
                            blocked = true;
                            break;
                        }
                    }
                    if blocked {
                        break;
                    }
                    let next = read(w, &changes, q);
                    chain.push(q);
                    if next == 0 {
                        for pair in chain.windows(2).rev() {
                            let m = read(w, &changes, pair[0]);
                            move_cell(&mut changes, pair[0], pair[1], m);
                            self.wake(pair[0], size);
                            self.wake(pair[1], size);
                            self.pushed += 1;
                            remaining -= 1;
                        }
                        break;
                    }
                    if self
                        .tuning
                        .material(next)
                        .is_none_or(|m| m.repose_drop != 1)
                    {
                        break;
                    }
                }
                if chain.last().is_some_and(|q| read(w, &changes, *q) != 0)
                    && chain.len() > 1
                    && read(w, &changes, p) == 0
                {
                    break;
                }
            }
        }
        if !changes.is_empty() {
            apply(w, changes);
        }
    }
}

fn clear(
    b: &Body,
    w: &World,
    at: [i64; 3],
    checks: &mut u32,
    occupied: &[(u64, [i64; 3], [i64; 3])],
) -> bool {
    if b.tilt_axis >= 3 {
        let delta = [at[0] - b.ox, at[1] - b.oy, at[2] - b.oz];
        let mut lo = [i64::MAX; 3];
        let mut hi = [i64::MIN; 3];
        for &v in &b.voxels {
            *checks -= 1;
            let (x, y, z) = b.world_of(v);
            let p = [x + delta[0], y + delta[1], z + delta[2]];
            if p.iter().any(|&v| v < 1)
                || p[0] + 1 >= i64::from(w.nx)
                || p[1] + 1 >= i64::from(w.ny)
                || p[2] + 1 >= i64::from(w.nz)
            {
                return false;
            }
            let q = p.map(|v| v as u32);
            if w.solid(q[0], q[1], q[2]) || w.has_pending_dig(q[0], q[1], q[2]) {
                return false;
            }
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a] + 1);
            }
        }
        for &(id, other_lo, other_hi) in occupied {
            *checks -= 1;
            if id != b.id && (0..3).all(|a| lo[a] < other_hi[a] && hi[a] > other_lo[a]) {
                return false;
            }
        }
        return true;
    }
    let hi = [
        at[0] + i64::from(b.sx),
        at[1] + i64::from(b.height()),
        at[2] + i64::from(b.sz),
    ];
    if at.iter().any(|&v| v < 1)
        || hi[0] >= i64::from(w.nx)
        || hi[1] >= i64::from(w.ny)
        || hi[2] >= i64::from(w.nz)
    {
        return false;
    }
    for &(id, lo, other_hi) in occupied {
        *checks -= 1;
        if id != b.id && (0..3).all(|a| at[a] < other_hi[a] && hi[a] > lo[a]) {
            return false;
        }
    }
    for v in &b.voxels {
        *checks -= 1;
        let p = [
            at[0] + i64::from(v.x),
            at[1] + i64::from(v.y),
            at[2] + i64::from(v.z),
        ]
        .map(|v| v as u32);
        if w.solid(p[0], p[1], p[2]) || w.has_pending_dig(p[0], p[1], p[2]) {
            return false;
        }
    }
    true
}
