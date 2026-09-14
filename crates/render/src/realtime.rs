//! Fixed simulation ticks and interpolated, read-only presentation poses.
use crate::gait::{self, Pose};
use bevy::prelude::{Mat3, Quat, Resource, Vec3};

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
        let count = (self.debt / step).floor().min(cap as f64) as u32;
        self.debt -= count as f64 * step;
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
    pub fn capture(&mut self, world: &sim_core::World) {
        self.previous.clone_from(&self.current);
        let u = &world.units;
        self.previous.resize(u.len(), Pose::default());
        self.current.resize(u.len(), Pose::default());
        self.identities.resize(u.len(), (u16::MAX, u8::MAX));
        for i in 0..u.len() {
            let identity = (u.kind[i], u.owner[i]);
            if self.identities[i] != identity {
                self.previous[i].alive = false;
            }
            self.identities[i] = identity;
            let n = u.up[i];
            self.current[i] = Pose {
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
        }
        // A death and same-kind spawn can reuse a slot within one tick.
        for e in world.events() {
            if let sim_core::SimEvent::Death { unit, .. } = *e {
                if let Some(p) = self.previous.get_mut(unit as usize) {
                    p.alive = false;
                }
            }
        }
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
    #[test]
    fn poses_are_continuous_and_new_units_do_not_fly_from_old_slots() {
        let tuning = modding::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        let mut w = sim_core::World::new(2, 1, tuning);
        let id = w.units.spawn(10, 20, 0, 30, 300) as usize;
        w.units.refresh_live();
        let mut p = Poses::default();
        p.capture(&w);
        w.units.x[id] = 12;
        p.capture(&w);
        p.sample(0.5);
        assert_eq!(p.visible[id].pos.x, 11.0);
        w.units.owner[id] = 1;
        w.units.x[id] = 500;
        p.capture(&w);
        p.sample(0.1);
        assert_eq!(p.visible[id].pos.x, 500.0);
    }
    #[test]
    fn hitch_debt_is_paid_and_pause_keeps_the_fraction() {
        let mut c = TickClock::default();
        let dt = 1.0 / 30.0;
        assert_eq!(c.advance(20.0, 1, dt, 8), 0);
        assert_eq!(c.advance(0.71, 1, dt, 8), 8);
        let debt = c.debt;
        assert_eq!(c.advance(60.0, 0, dt, 8), 0);
        assert_eq!(c.debt, debt);
        let mut ticks = 8;
        while c.debt >= dt {
            ticks += c.advance(0.0, 1, dt, 8);
        }
        assert_eq!(ticks, 21);
        assert!((ticks as f64 * dt + c.debt - 0.71).abs() < 1e-10);
    }

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
            c.advance(0.0, 1, 1.0 / 30.0, 8);
            let mut ticks = 0;
            for _ in 0..fps * 10 {
                ticks += c.advance(1.0 / fps as f64, 1, 1.0 / 30.0, 8);
            }
            assert!((ticks as f64 / 30.0 + c.debt - 10.0).abs() < 1e-8);
        }
    }
}
