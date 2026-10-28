//! Fixed simulation ticks and interpolated, read-only presentation poses.
use crate::gait::{self, Pose};
use bevy::prelude::{Mat3, Quat, Resource, Vec3};

/// Most real time a window may owe the simulation, in ticks: one second at
/// the configured frequency. Debt beyond it is dropped, so a long overload slows the game
/// instead of being replayed later as a fast-forward burst (analysis 240, B5).
/// Presentation only: the ticks themselves are unchanged.
pub const MAX_DEBT_TICKS: u32 = sim_core::TICKS_PER_SECOND as u32;

#[derive(Default)]
pub struct TickClock {
    pub debt: f64,
    primed: bool,
}

impl TickClock {
    pub fn advance(&mut self, elapsed: f64, speed: u32, step: f64, cap: u32) -> u32 {
        // Asset and window creation time is not time spent playing.
        if !self.primed {
            self.primed = true;
            return 0;
        }
        if speed == 0 {
            return 0;
        }
        self.debt += elapsed.max(0.0) * speed as f64;
        let mut ticks = self.debt / step;
        let integer = ticks.round();
        if (ticks - integer).abs() <= f64::EPSILON * ticks.abs().max(1.0) * 8.0 {
            ticks = integer;
        }
        let count = ticks.floor().min(cap as f64) as u32;
        self.debt = (ticks - f64::from(count))
            .max(0.0)
            .min(f64::from(MAX_DEBT_TICKS))
            * step;
        count
    }
    pub fn fraction(&self, step: f64) -> f32 {
        (self.debt / step).clamp(0.0, 1.0) as f32
    }
}

#[derive(Resource, Default)]
pub struct Poses {
    previous: Vec<Pose>,
    current: Vec<Pose>,
    pub visible: Vec<Pose>,
    identities: Vec<(u16, u8)>,
}

impl Poses {
    pub fn motion(&self, unit: usize, dt: f32) -> gait::StepMotion {
        match (self.previous.get(unit), self.current.get(unit)) {
            (Some(a), Some(b)) => gait::StepMotion::between(a, b, dt),
            _ => gait::StepMotion::default(),
        }
    }
    /// Start a tick: the current poses become the previous ones, and
    /// `unit(i)` gives slot `i`'s identity (kind, owner) and simulated pose.
    ///
    /// A slot whose identity changed, or that is listed in `deaths`, is a
    /// new unit: it appears at its pose instead of flying in from the old
    /// occupant's. A death and a same-kind spawn can reuse a slot within one
    /// tick, so identity alone does not catch it.
    pub fn capture_with(
        &mut self,
        count: usize,
        mut unit: impl FnMut(usize) -> ((u16, u8), Pose),
        deaths: impl IntoIterator<Item = usize>,
    ) {
        self.previous.clone_from(&self.current);
        self.previous.resize(count, Pose::default());
        self.current.resize(count, Pose::default());
        self.identities.resize(count, (u16::MAX, u8::MAX));
        for i in 0..count {
            let (identity, pose) = unit(i);
            if self.identities[i] != identity {
                self.previous[i].alive = false;
            }
            self.identities[i] = identity;
            self.current[i] = pose;
        }
        for unit in deaths {
            if let Some(p) = self.previous.get_mut(unit) {
                p.alive = false;
            }
        }
    }
    /// The `play` window's adapter over the old kernel; it leaves together
    /// with that window's port to the range (ADR-662).
    pub fn capture(&mut self, world: &sim_core::World) {
        let u = &world.units;
        let deaths = world.events().iter().filter_map(|e| match *e {
            sim_core::SimEvent::Death { unit, .. } => Some(unit as usize),
            _ => None,
        });
        self.capture_with(
            u.len(),
            |i| {
                let n = u.up[i];
                let pose = Pose {
                    pos: Vec3::new(
                        u.x[i] as f32 + u.fx[i].to_f64_lossy() as f32,
                        u.body_y(i).to_f64_lossy() as f32,
                        u.z[i] as f32 + u.fz[i].to_f64_lossy() as f32,
                    ),
                    yaw: crate::yaw_of(u.head[i]),
                    up: Vec3::new(
                        n.x.to_f64_lossy() as f32,
                        n.y.to_f64_lossy() as f32,
                        n.z.to_f64_lossy() as f32,
                    ),
                    phase: gait::phase_of(u.gait[i]),
                    alive: u.alive[i],
                };
                ((u.kind[i], u.owner[i]), pose)
            },
            deaths,
        );
    }
    pub fn sample(&mut self, fraction: f32) {
        self.visible.clear();
        self.visible.extend(
            self.previous
                .iter()
                .zip(&self.current)
                .map(|(a, b)| gait::lerp(a, b, fraction)),
        );
    }
}

pub fn hull_rotation(up: Vec3, yaw: f32) -> Quat {
    let up = up.try_normalize().unwrap_or(Vec3::Y);
    let f = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let f = (f - up * f.dot(up)).try_normalize().unwrap_or(Vec3::X);
    Quat::from_mat3(&Mat3::from_cols(f, up, f.cross(up)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(x: f32) -> Pose {
        Pose {
            pos: Vec3::new(x, 0.0, 20.0),
            alive: true,
            ..Pose::default()
        }
    }
    #[test]
    fn poses_are_continuous_and_new_units_do_not_fly_from_old_slots() {
        let mut p = Poses::default();
        let tank = (0u16, 0u8);
        p.capture_with(1, |_| (tank, at(10.0)), []);
        p.capture_with(1, |_| (tank, at(12.0)), []);
        p.sample(0.5);
        assert_eq!(p.visible[0].pos.x, 11.0);
        // Another owner in the same slot is another unit.
        p.capture_with(1, |_| ((0, 1), at(500.0)), []);
        p.sample(0.1);
        assert_eq!(p.visible[0].pos.x, 500.0);
        // So is a same-kind spawn into a slot freed on this tick.
        p.capture_with(1, |_| ((0, 1), at(-40.0)), [0]);
        p.sample(0.1);
        assert_eq!(p.visible[0].pos.x, -40.0);
    }
    #[test]
    fn hitch_debt_is_paid_and_pause_keeps_the_fraction() {
        let mut c = TickClock::default();
        let dt = 1.0 / f64::from(sim_core::TICKS_PER_SECOND);
        assert_eq!(c.advance(20.0, 1, dt, 8), 0);
        assert_eq!(c.advance(0.71, 1, dt, 8), 8);
        let debt = c.debt;
        assert_eq!(c.advance(60.0, 0, dt, 8), 0);
        assert_eq!(c.debt, debt);
        let mut ticks = 8;
        while c.debt >= dt {
            ticks += c.advance(0.0, 1, dt, 8);
        }
        assert_eq!(ticks, (0.71 / dt).floor() as u32);
        assert!((ticks as f64 * dt + c.debt - 0.71).abs() < 1e-10);
    }

    /// Checks the `play` window's adapter over the old kernel and leaves
    /// with it (ADR-662).
    #[test]
    fn heave_is_interpolated_in_the_same_space_as_the_collision_origin() {
        let tuning = modding::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        let mut w = sim_core::World::new(2, 1, tuning);
        let i = w.units.spawn(10, 20, 0, 30, 300) as usize;
        w.units.refresh_live();
        w.units.y[i] = 40;
        w.units.heave[i] = fx::Fx::from_ratio(5, 4);
        let mut poses = Poses::default();
        poses.capture(&w);
        assert_eq!(
            poses.current[i].pos.y,
            w.units.body_y(i).to_f64_lossy() as f32
        );
        w.units.heave[i] = fx::Fx::from_ratio(9, 4);
        poses.capture(&w);
        poses.sample(0.5);
        assert_eq!(poses.visible[i].pos.y, 41.75);
        assert_eq!(
            w.units.y[i], 40,
            "presentation changed the navigation level"
        );
    }
    #[test]
    fn rendering_rate_does_not_change_simulation_time() {
        for fps in [20, 30, 60, 144] {
            let mut c = TickClock::default();
            c.advance(0.0, 1, 1.0 / f64::from(sim_core::TICKS_PER_SECOND), 8);
            let mut ticks = 0;
            for _ in 0..fps * 10 {
                ticks += c.advance(
                    1.0 / fps as f64,
                    1,
                    1.0 / f64::from(sim_core::TICKS_PER_SECOND),
                    8,
                );
            }
            assert!(
                (ticks as f64 / f64::from(sim_core::TICKS_PER_SECOND) + c.debt - 10.0).abs() < 1e-8
            );
        }
    }
}

#[cfg(test)]
mod debt_tests {
    use super::*;

    /// A long stall owes at most a second; the clock then runs at 1x again.
    #[test]
    fn a_long_stall_does_not_fast_forward_later() {
        let step = 1.0 / f64::from(sim_core::TICKS_PER_SECOND);
        let mut clock = TickClock::default();
        assert_eq!(clock.advance(0.0, 1, step, 8), 0);
        let mut total = clock.advance(10.0, 1, step, 8);
        assert_eq!(total, 8);
        for _ in 0..100 {
            total += clock.advance(0.0, 1, step, 8);
        }
        assert_eq!(
            total,
            8 + MAX_DEBT_TICKS,
            "the debt beyond one second was replayed"
        );
        assert_eq!(clock.advance(step, 1, step, 8), 1);
    }
}
