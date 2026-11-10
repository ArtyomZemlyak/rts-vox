//! Manual hover flight shares drive state and never asks the ground planner.
use super::*;

type Bounds = ([i32; 3], [i32; 3]);

impl Motion {
    pub(in crate::blast) fn step_pilot_flight(
        &mut self,
        world: &World,
        body: &UnitBody,
        powered: bool,
        compact: bool,
        response: crate::mobility::Mobility,
        blockers: impl Iterator<Item = Bounds>,
    ) {
        self.cut.clear();
        self.clear_contact();
        // Tracks do not support a hovercraft. Engine damage still removes thrust.
        let response = response.with_traction(Fx::ONE);
        let [throttle, turn] = self.pilot.filter(|_| powered).unwrap_or([0, 0]);
        let want = if throttle == 0 && turn == 0 {
            drive::NO_WAY
        } else {
            angle::norm(self.drive.head + i32::from(turn) * Limits::of(&self.chassis, TICKS).turn)
        };
        let limits = response.limits(&Limits::of(&self.chassis, TICKS), want == drive::NO_WAY);
        let speed = Fx::from_int(self.speed)
            .mul(if compact { Fx::from_ratio(3, 2) } else { Fx::ONE })
            .mul(response.engine)
            .mul(Fx::from_int(i32::from(throttle)));
        let (next, delta) = drive::advance(
            self.drive,
            want,
            speed,
            &limits,
            self.drive_scale,
            Fx::from_ratio(1, TICKS),
        );
        // Find the real occupied local box, including cargo and damaged parts.
        let mut low = [i32::MAX; 3];
        let mut high = [i32::MIN; 3];
        for (row, words) in body
            .vol
            .cols
            .chunks_exact(usize::from(body.vol.words))
            .enumerate()
        {
            for (word, &bits) in words.iter().enumerate().filter(|(_, bits)| **bits != 0) {
                let a = [
                    (word * 64) as i32 + bits.trailing_zeros() as i32,
                    (row % usize::from(body.vol.ny)) as i32,
                    (row / usize::from(body.vol.ny)) as i32,
                ];
                let b = [
                    (word * 64 + 64) as i32 - bits.leading_zeros() as i32,
                    a[1] + 1,
                    a[2] + 1,
                ];
                for axis in 0..3 {
                    low[axis] = low[axis].min(a[axis]);
                    high[axis] = high[axis].max(b[axis]);
                }
            }
        }
        if low[0] == i32::MAX {
            return;
        }
        let bounds = |position: [Fx; 3], heading| {
            let frame = crate::damage::Frame::new(
                body,
                V3::new(position[0], position[1], position[2]),
                heading,
                V3::UP,
            );
            let mut lo = [i32::MAX; 3];
            let mut hi = [i32::MIN; 3];
            for corner in 0..8 {
                let p = frame.world(std::array::from_fn(|axis| {
                    Fx::from_int(if corner & (1 << axis) == 0 {
                        low[axis]
                    } else {
                        high[axis]
                    })
                }));
                for (axis, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                    lo[axis] = lo[axis].min((v.raw() >> 32) as i32);
                    hi[axis] = hi[axis].max(((v.raw() + (1i64 << 32) - 1) >> 32) as i32);
                }
            }
            (lo, hi)
        };
        let distance = delta.dx.abs().max(delta.dz.abs());
        let radius = i64::from(body.vol.nx.max(body.vol.nz)) * i64::from(body.mm) / 100;
        let yaw = angle::diff(next.head, self.drive.head);
        let turn_span = radius * i64::from(yaw.abs()) * 7 / i64::from(angle::FULL);
        let steps = (i64::from(distance.to_int_trunc()) + turn_span + 1).max(1);
        // Bound work for extreme edited speeds instead of tunnelling through walls.
        let blockers: Vec<_> = blockers.collect();
        let allowed = steps <= 128
            && (1..=steps).all(|step| {
                let t = Fx::from_ratio(step as i32, steps as i32);
                let p = [
                    self.pos[0].add(delta.dx.mul(t)),
                    self.pos[1],
                    self.pos[2].add(delta.dz.mul(t)),
                ];
                let (lo, hi) = bounds(
                    p,
                    angle::norm(self.drive.head + (i64::from(yaw) * step / steps) as i32),
                );
                let size = [world.nx, world.ny, world.nz];
                (0..3).all(|a| lo[a] >= 0 && hi[a] <= size[a] as i32)
                    && !world.any_solid_box(lo.map(|v| v as u32), hi.map(|v| (v - 1) as u32))
                    && !blockers
                        .iter()
                        .any(|(a, b)| (0..3).all(|axis| lo[axis] < b[axis] && a[axis] < hi[axis]))
            });
        self.blocked = !allowed;
        if allowed {
            self.contact_velocity =
                V3::new(delta.dx, Fx::ZERO, delta.dz).scale(Fx::from_int(TICKS));
            self.drive = next;
            self.pos[0] = self.pos[0].add(delta.dx);
            self.pos[2] = self.pos[2].add(delta.dz);
        } else {
            self.drive.vel = Fx::ZERO;
            self.drive.yaw_rate = 0;
        }
    }
}
