//! Persistent render identities and exact pose reuse. Physics owns every piece.
use super::{Entity, Transform, Vec3};
use sim_core::{burst::Fragment, damage::V3};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    position: V3,
    basis: [V3; 3],
    mm: u16,
    shape: u64,
    section: bool,
}
impl Stamp {
    fn of(f: &Fragment) -> Self {
        Self {
            position: f.position,
            basis: f.basis,
            mm: f.mm,
            shape: f.shape,
            section: f.chunk.is_some(),
        }
    }
}

pub(super) struct Slot {
    pub entity: Entity,
    pub key: (u64, bool),
    pub seen: u64,
    mesh: Option<render::model::FragmentMeshKey>,
    stamp: Stamp,
    current: Transform,
    shown: Transform,
}
impl Slot {
    pub fn new(
        entity: Entity,
        key: (u64, bool),
        f: &Fragment,
        current: Transform,
        shown: Transform,
        seen: u64,
    ) -> Self {
        Self {
            entity,
            key,
            seen,
            mesh: mesh_key(f),
            stamp: Stamp::of(f),
            current,
            shown,
        }
    }
    pub fn matches(&self, key: (u64, bool), f: &Fragment) -> bool {
        self.key == key && self.mesh == mesh_key(f)
    }
    /// Recompute the quaternion/scale only when authoritative geometry moved.
    /// Previous positions and alpha remain live, including pause and restart.
    /// Equal pictures return no update, avoiding Bevy's Changed<Transform> work.
    pub fn sample(
        &mut self,
        f: &Fragment,
        previous: Option<Vec3>,
        alpha: f32,
        make: impl FnOnce() -> Transform,
    ) -> Option<Transform> {
        let stamp = Stamp::of(f);
        if self.stamp != stamp {
            self.current = make();
            self.stamp = stamp;
        }
        let next = interpolate(self.current, previous, alpha);
        if next == self.shown {
            None
        } else {
            self.shown = next;
            Some(next)
        }
    }
}
fn mesh_key(f: &Fragment) -> Option<render::model::FragmentMeshKey> {
    f.chunk
        .is_none()
        .then_some((f.material, f.shape, f.side, f.paint))
}
pub(super) fn interpolate(mut current: Transform, previous: Option<Vec3>, alpha: f32) -> Transform {
    current.translation = previous
        .unwrap_or(current.translation)
        .lerp(current.translation, alpha);
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::Fx;

    fn piece() -> Fragment {
        Fragment {
            id: 7,
            position: V3::new(Fx::from_int(8), Fx::ZERO, Fx::ZERO),
            velocity: V3::ZERO,
            material: 3,
            paint: None,
            mm: 100,
            basis: [
                V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
                V3::new(Fx::ZERO, Fx::ONE, Fx::ZERO),
                V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
            ],
            mass_g: 1,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: 0,
            resting: true,
        }
    }
    fn slot(f: &Fragment, current: Transform) -> Slot {
        Slot::new(Entity::from_bits(7), (0, false), f, current, current, 1)
    }
    #[test]
    fn sleeping_piece_skips_pose_rebuild_and_transform_updates() {
        let f = piece();
        let current = Transform::from_xyz(8.0, 0.0, 0.0);
        let mut s = slot(&f, current);
        for alpha in [0.0, 0.25, 0.75, 1.0] {
            for previous in [None, Some(current.translation)] {
                assert!(s
                    .sample(&f, previous, alpha, || panic!("unchanged pose rebuilt"))
                    .is_none());
            }
        }
    }
    #[test]
    fn flying_and_just_landed_piece_preserve_every_interpolated_frame() {
        let mut f = piece();
        let mut current = Transform::from_xyz(8.0, 0.0, 0.0);
        let mut s = slot(&f, current);
        for tick in 0..4 {
            let previous = Some(current.translation);
            f.position.x = f.position.x.add(Fx::from_int(3));
            f.resting = tick == 3;
            current.translation.x += 3.0;
            for (frame, alpha) in [0.0, 0.2, 0.6, 1.0].into_iter().enumerate() {
                let next = s.sample(&f, previous, alpha, || {
                    assert_eq!(frame, 0, "same-tick quaternion rebuilt");
                    current
                });
                let expected = interpolate(current, previous, alpha);
                assert_eq!(next.unwrap_or(s.shown), expected);
            }
        }
        assert!(s
            .sample(&f, None, 1.0, || panic!("resting rebuild"))
            .is_none());
    }
    #[test]
    fn same_tick_pose_rotation_and_scale_changes_are_not_lost() {
        let mut f = piece();
        let current = Transform::from_xyz(8.0, 0.0, 0.0);
        let mut s = slot(&f, current);
        for change in 0..4 {
            match change {
                0 => f.position.y = Fx::ONE,
                1 => f.basis.swap(0, 2),
                2 => f.mm = 200,
                _ => f.shape = 3,
            }
            let expected = Transform::from_xyz(8.0, change as f32 + 1.0, 0.0);
            assert_eq!(s.sample(&f, None, 1.0, || expected), Some(expected));
        }
    }
    #[test]
    fn extra_inventory_does_not_rebuild_a_saturated_pile_pose() {
        let mut f = piece();
        f.shape = u64::MAX;
        f.side = 4;
        f.count = 64;
        let mut s = slot(&f, Transform::default());
        f.count = 4096;
        f.mass_g *= 64;
        assert!(s.matches((0, false), &f));
        assert!(s
            .sample(&f, None, 1.0, || panic!("inventory is not geometry"))
            .is_none());
    }
    #[test]
    fn pause_and_previous_tick_changes_update_picture_without_rebuilding_pose() {
        let f = piece();
        let current = Transform::from_xyz(8.0, 0.0, 0.0);
        let mut s = slot(&f, current);
        for (previous, alpha) in [
            (Some(Vec3::ZERO), 0.25),
            (Some(Vec3::ZERO), 1.0),
            (Some(Vec3::splat(2.0)), 0.5),
            (None, 1.0),
        ] {
            let next = s.sample(&f, previous, alpha, || panic!("unchanged physics rebuilt"));
            assert_eq!(
                next.unwrap_or(s.shown),
                interpolate(current, previous, alpha)
            );
        }
    }
    #[test]
    fn reset_or_merge_reusing_an_id_rebuilds_changed_mesh_and_finish() {
        let f = piece();
        let s = slot(&f, Transform::default());
        assert!(s.matches((0, false), &f));
        for change in 0..4 {
            let mut next = piece();
            match change {
                0 => next.material = 2,
                1 => next.shape = 3,
                2 => next.side = 2,
                _ => next.paint = Some([0, 1, 2]),
            }
            assert!(!s.matches((0, false), &next));
        }
        assert!(!s.matches((123, false), &f));
        assert!(!s.matches((0, true), &f));
    }
}
