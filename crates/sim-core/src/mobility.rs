//! Response of the installed machinery to actual remaining root material.
//! Intact type sizing stays in Economy; losing armour does not shrink the motor.
use crate::burst::chunk::Shape;
use crate::drive::Limits;
use fx::Fx;

/// How much lighter than whole a body may ride (ADR-305): the spring of
/// a hull stripped to a tenth of its cells is ten times as stiff taken to
/// its mass, and a fixed step of a thirtieth of a second does not hold it.
const RIDE_MASS_CAP: i32 = 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mobility {
    pub inverse_mass: Fx,
    pub inverse_yaw_inertia: Fx,
    /// Mean roll/pitch inertia response, matching the two-axis ride approximation.
    pub inverse_tilt_inertia: Fx,
    pub tilt_radius_ratio: Fx,
    /// Fraction of original engine cells still connected to the core.
    pub engine: Fx,
    /// Remaining transmission/load paths. Zero prevents powered movement.
    pub traction: Fx,
}

fn ratio(n: u64, d: u64) -> Fx {
    Fx((((n as u128) << 32) / d.max(1) as u128).min(i64::MAX as u128) as i64)
}

impl Mobility {
    pub fn after_damage(
        original: &Shape,
        remaining: &Shape,
        template: &voxel::UnitBody,
        body: &voxel::UnitBody,
        core: &voxel::topo::unit::CoreBufs,
        status: voxel::unit::Status,
    ) -> Self {
        let engine = if status.alive && status.engine {
            Self::engine_fraction(
                body.connected_subsystem_count(voxel::Part::Engine, core),
                template.subsystem_count(voxel::Part::Engine),
            )
        } else {
            Fx::ZERO
        };
        Self::from_shapes(
            original,
            remaining,
            [body.vol.nx, body.vol.ny, body.vol.nz],
            engine,
        )
    }

    pub fn intact(powered: bool) -> Self {
        Self {
            inverse_mass: Fx::ONE,
            inverse_yaw_inertia: Fx::ONE,
            inverse_tilt_inertia: Fx::ONE,
            tilt_radius_ratio: Fx::ONE,
            engine: if powered { Fx::ONE } else { Fx::ZERO },
            traction: Fx::ONE,
        }
    }

    pub fn from_shapes(original: &Shape, remaining: &Shape, dims: [u8; 3], engine: Fx) -> Self {
        let a = original.inertia(dims);
        let b = remaining.inertia(dims);
        let inverse_mass = ratio(original.count() as u64, remaining.count() as u64);
        let inverse_tilt_inertia = ratio(a[0] + a[2], b[0] + b[2]);
        Self {
            inverse_mass,
            inverse_yaw_inertia: ratio(a[1], b[1]),
            inverse_tilt_inertia,
            tilt_radius_ratio: inverse_mass.div(inverse_tilt_inertia),
            engine,
            traction: Fx::ONE,
        }
    }

    pub fn engine_fraction(connected: u32, original: u32) -> Fx {
        if original == 0 {
            Fx::ZERO
        } else {
            ratio(connected as u64, original as u64).min(Fx::ONE)
        }
    }

    pub fn limits(self, original: &Limits, braking: bool) -> Limits {
        let mut out = *original;
        out.accel = out
            .accel
            .mul(self.inverse_mass)
            .mul(self.engine)
            .mul(self.traction);
        out.brake = out.brake.mul(self.inverse_mass);
        // Motor failure removes active turning torque, not the mechanical brake.
        out.turn_acc = out.turn_acc.mul(self.inverse_yaw_inertia).mul(if braking {
            Fx::ONE
        } else {
            self.engine.mul(self.traction)
        });
        out
    }

    pub fn with_traction(mut self, traction: Fx) -> Self {
        self.traction = traction;
        self
    }

    /// The ride of a damaged body (ADR-305). Springs and dampers are the
    /// machine's own, so taken to its mass they grow as its cells go: a
    /// lighter body on the same suspension. The preload does not grow. It
    /// carries the weight, and weight taken to mass is gravity whatever
    /// is left, so scaling it pushed a wrecked hull up with many times
    /// its own weight and threw it into the air (P-30). The ratio is
    /// capped: the step is fixed, and a spring stiff enough for it walks
    /// away in one tick.
    pub fn ride(self, mut original: phys::Ride) -> phys::Ride {
        let lighter = self.inverse_mass.min(Fx::from_int(RIDE_MASS_CAP));
        original.stiffness = original.stiffness.mul(lighter);
        original.damping = original.damping.mul(lighter);
        // Ride stores I/M, so divide the mass response by the inertia response.
        original.inertia = original.inertia.mul(self.tilt_radius_ratio).max(Fx(1));
        original
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::Bodies;
    use crate::drive::{advance, Drive, Move, NO_WAY};
    use voxel::{Part, UnitBody};

    /// The nominal speed the ride runs at, world units per second.
    const SPEED: i32 = 90;

    /// One 9x3x9 armoured hull with a core and two engine cells, damaged
    /// through the same body queue the battle uses. No world around it:
    /// the drive below is the same composition every driven hull runs,
    /// the mobility of the body over the chassis' nominal limits.
    fn fixture() -> Bodies {
        let mut body = UnitBody::new_scaled(9, 3, 9, 1, 100);
        for x in 0..9 {
            for y in 0..3 {
                for z in 0..9 {
                    body.set(x, y, z, Part::Armour);
                }
            }
        }
        body.set(4, 1, 4, Part::Core);
        body.set(1, 1, 4, Part::Engine);
        body.set(2, 1, 4, Part::Engine);
        assert!(body.seat_core());
        let mut bodies = Bodies::new(vec![body]);
        bodies.spawn_at(0, 0);
        bodies
    }
    fn hit(b: &mut Bodies, cells: Vec<[u8; 3]>) {
        assert!(b.queue_cells(
            0,
            b.generations[0],
            cells,
            1_000_000,
            0,
            phys::V3::ZERO,
            4096
        ));
        b.step(&[1; 5], 16_384, 32);
    }
    fn response(b: &Bodies) -> Mobility {
        b.state(0).unwrap().mobility
    }
    /// One tick of a hull's drive: thrust only while the machinery is
    /// whole enough to give it, the top speed scaled by the engine left,
    /// and the brake always the hull's own.
    fn drive(m: Mobility, d: Drive, way: i32) -> (Drive, Move) {
        let nominal =
            crate::drive::Limits::of(&crate::test_tuning().tracked, crate::TICKS_PER_SECOND);
        let powered = m.engine > Fx::ZERO && m.traction > Fx::ZERO;
        let way = if powered { way } else { NO_WAY };
        let vmax = if powered {
            Fx::from_int(SPEED).mul(m.engine.mul(m.traction))
        } else {
            Fx::ZERO
        };
        advance(
            d,
            way,
            vmax,
            &m.limits(&nominal, way == NO_WAY),
            Fx::ONE,
            Fx::from_ratio(1, crate::TICKS_PER_SECOND),
        )
    }

    #[test]
    fn lost_armour_changes_actual_acceleration_with_the_same_installed_engine() {
        let whole = fixture();
        let mut light = fixture();
        let mut cut = Vec::new();
        for x in 7..9 {
            for y in 0..3 {
                for z in 0..9 {
                    cut.push([x, y, z]);
                }
            }
        }
        hit(&mut light, cut);
        let (whole, light) = (response(&whole), response(&light));
        assert_eq!(whole, Mobility::intact(true));
        assert_eq!(light.engine, Fx::ONE);
        assert!(light.inverse_mass > Fx::ONE);
        let (w, wm) = drive(whole, Drive::default(), 0);
        let (l, lm) = drive(light, Drive::default(), 0);
        assert!(l.vel > w.vel);
        assert!(lm.dx > wm.dx);
        // Losing peripheral material also changes angular acceleration.
        let (w, _) = drive(whole, w, fx::angle::QUARTER);
        let (l, _) = drive(light, l, fx::angle::QUARTER);
        assert!(l.yaw_rate > w.yaw_rate);
    }

    #[test]
    fn partial_engine_damage_reduces_thrust_and_failure_keeps_the_coasting_motion() {
        let whole = response(&fixture());
        let mut b = fixture();
        hit(&mut b, vec![[1, 1, 4]]);
        let damaged = response(&b);
        assert_eq!(damaged.engine, Fx::from_ratio(1, 2));
        let (w, _) = drive(whole, Drive::default(), 0);
        let (d, _) = drive(damaged, Drive::default(), 0);
        assert!(d.vel > w.vel.mul(Fx::from_ratio(1, 3)));
        assert!(d.vel < w.vel.mul(Fx::from_ratio(2, 3)));
        // The last engine cell goes while the hull runs: it coasts on its
        // brake, still moving forward, and stops.
        hit(&mut b, vec![[2, 1, 4]]);
        let failed = response(&b);
        assert_eq!(failed.engine, Fx::ZERO);
        let coasting = Drive {
            vel: Fx::from_int(60),
            ..d
        };
        let (mut d, mv) = drive(failed, coasting, 0);
        assert!(d.vel > Fx::ZERO && d.vel < Fx::from_int(60));
        assert!(mv.dx > Fx::ZERO);
        for _ in 0..120 {
            d = drive(failed, d, 0).0;
        }
        assert_eq!(d.vel, Fx::ZERO);
        b.spawn_at(0, 0);
        assert_eq!(response(&b), Mobility::intact(true));
    }

    #[test]
    fn disconnected_engine_cells_stop_providing_thrust_before_they_leave_the_body() {
        let mut body = UnitBody::new_scaled(7, 1, 1, 1, 100);
        for x in 0..7 {
            body.set(x, 0, 0, Part::Armour);
        }
        body.set(3, 0, 0, Part::Core);
        body.set(0, 0, 0, Part::Engine);
        body.set(6, 0, 0, Part::Engine);
        assert!(body.seat_core());
        let mut b = Bodies::new(vec![body]);
        b.spawn_at(0, 0);
        b.queue_cells(
            0,
            b.generations[0],
            vec![[1, 0, 0]],
            1000,
            0,
            phys::V3::ZERO,
            16,
        );
        b.step(&[1; 5], 16, 16);
        assert_eq!(b.state(0).unwrap().mobility.engine, Fx::from_ratio(1, 2));
        let before = b.state(0).unwrap().mobility;
        assert!(b.body(0).unwrap().vol.get(0, 0, 0));
        b.step_detach(100, 16, 0, &[true]);
        assert_eq!(b.state(0).unwrap().mobility, before);
        let parts = b.step_detach(100, 16, 1, &[true]);
        assert_eq!(parts.len(), 1);
        assert!(b.state(0).unwrap().mobility.inverse_mass > before.inverse_mass);
        assert_eq!(b.state(0).unwrap().mobility.engine, before.engine);
    }
    #[test]
    fn removing_outer_mass_changes_turning_more_than_the_same_mass_near_the_pivot() {
        let mut original = Shape::default();
        for x in 0..9 {
            for z in 0..9 {
                original.add([x, 0, z], 0);
            }
        }
        let mut middle = original.clone();
        middle.remove([4, 0, 4], 0);
        let mut outer = original.clone();
        outer.remove([8, 0, 8], 0);
        let a = Mobility::from_shapes(&original, &middle, [9, 1, 9], Fx::ONE);
        let b = Mobility::from_shapes(&original, &outer, [9, 1, 9], Fx::ONE);
        assert_eq!(a.inverse_mass, b.inverse_mass);
        assert!(b.inverse_yaw_inertia > a.inverse_yaw_inertia);
        assert!(a.inverse_mass > Fx::ONE);
    }
    #[test]
    fn incremental_statistics_match_a_fresh_scan_including_each_cubes_own_inertia() {
        let mut body = voxel::unit::tank();
        let dims = [body.vol.nx, body.vol.ny, body.vol.nz];
        let mut shape = Shape::from_body(&body);
        for x in (0..body.vol.nx).step_by(3) {
            for y in (0..body.vol.ny).step_by(2) {
                for z in (0..body.vol.nz).step_by(3) {
                    if body.remove(x as usize, y as usize, z as usize) {
                        shape.remove([x, y, z], body.turret_y);
                    }
                }
            }
        }
        assert_eq!(shape.inertia(dims), Shape::from_body(&body).inertia(dims));
        let mut cube = Shape::default();
        cube.add([0, 0, 0], 0);
        assert_eq!(cube.inertia([1, 1, 1]), [5, 2, 5]);
    }
}
