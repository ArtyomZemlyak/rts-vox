//! Response of the installed machinery to actual remaining root material.
//! Intact type sizing stays in Economy; losing armour does not shrink the motor.
use crate::burst::chunk::Shape;
use crate::drive::Limits;
use fx::Fx;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mobility {
    pub inverse_mass: Fx,
    pub inverse_yaw_inertia: Fx,
    /// Mean roll/pitch inertia response, matching the two-axis ride approximation.
    pub inverse_tilt_inertia: Fx,
    pub tilt_radius_ratio: Fx,
    /// Fraction of original engine cells still connected to the core.
    pub engine: Fx,
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
        out.accel = out.accel.mul(self.inverse_mass).mul(self.engine);
        out.brake = out.brake.mul(self.inverse_mass);
        // Motor failure removes active turning torque, not the mechanical brake.
        out.turn_acc = out.turn_acc.mul(self.inverse_yaw_inertia).mul(if braking {
            Fx::ONE
        } else {
            self.engine
        });
        out
    }

    pub fn ride(self, mut original: phys::Ride) -> phys::Ride {
        original.stiffness = original.stiffness.mul(self.inverse_mass);
        original.damping = original.damping.mul(self.inverse_mass);
        original.preload = original.preload.mul(self.inverse_mass);
        // Ride stores I/M, so divide the mass response by the inertia response.
        original.inertia = original.inertia.mul(self.tilt_radius_ratio).max(Fx(1));
        original
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::Bodies;
    use crate::{Tuning, World};
    use voxel::{Part, UnitBody};

    fn fixture() -> World {
        let mut t = Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        t.impact.work_kj_m3 = [1; 5];
        t.gravity = 0;
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
        let mut w = World::new(42, 16, t);
        w.units.spawn(300, 300, 0, 90, 99);
        w.units.refresh_live();
        let mut bodies = Bodies::new(vec![body]);
        bodies.spawn_at(0, 0);
        w.material_bodies = Some(bodies);
        w.delta.resize(w.units.len(), crate::drive::Want::default());
        w.delta[0].way = 0;
        w
    }
    fn hit(w: &mut World, cells: Vec<[u8; 3]>) {
        let b = w.material_bodies.as_mut().unwrap();
        assert!(b.queue_cells(
            0,
            b.generations[0],
            cells,
            1_000_000,
            0,
            phys::V3::ZERO,
            w.tuning.impact.max_pending
        ));
        w.apply_material_damage();
    }

    #[test]
    fn lost_armour_changes_actual_acceleration_with_the_same_installed_engine() {
        let mut whole = fixture();
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
        let response = light
            .material_bodies
            .as_ref()
            .unwrap()
            .state(0)
            .unwrap()
            .mobility;
        assert_eq!(response.engine, Fx::ONE);
        assert!(response.inverse_mass > Fx::ONE);
        whole.apply_deltas();
        light.apply_deltas();
        assert!(light.units.vel[0] > whole.units.vel[0]);
        assert!(light.units.fx[0] > whole.units.fx[0]);
        // Losing peripheral material also changes angular acceleration.
        whole.delta[0].way = fx::angle::QUARTER;
        light.delta[0].way = fx::angle::QUARTER;
        whole.apply_deltas();
        light.apply_deltas();
        assert!(light.units.yaw_rate[0] > whole.units.yaw_rate[0]);
    }

    #[test]
    fn partial_engine_damage_reduces_thrust_and_failure_keeps_the_coasting_motion() {
        let mut whole = fixture();
        let mut damaged = fixture();
        hit(&mut damaged, vec![[1, 1, 4]]);
        assert_eq!(
            damaged
                .material_bodies
                .as_ref()
                .unwrap()
                .state(0)
                .unwrap()
                .mobility
                .engine,
            Fx::from_ratio(1, 2)
        );
        whole.apply_deltas();
        damaged.apply_deltas();
        assert!(damaged.units.vel[0] > whole.units.vel[0].mul(Fx::from_ratio(1, 3)));
        assert!(damaged.units.vel[0] < whole.units.vel[0].mul(Fx::from_ratio(2, 3)));
        damaged.units.vel[0] = Fx::from_int(60);
        let x = damaged.units.x[0];
        hit(&mut damaged, vec![[2, 1, 4]]);
        damaged.apply_deltas();
        assert!(damaged.units.vel[0] > Fx::ZERO && damaged.units.vel[0] < Fx::from_int(60));
        assert!(damaged.units.x[0] > x);
        for _ in 0..120 {
            damaged.apply_deltas();
        }
        assert_eq!(damaged.units.vel[0], Fx::ZERO);
        let b = damaged.material_bodies.as_mut().unwrap();
        b.spawn_at(0, 0);
        assert_eq!(b.state(0).unwrap().mobility, Mobility::intact(true));
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
