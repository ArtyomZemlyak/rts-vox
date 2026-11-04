//! Four rigid articulated assemblies aim about their real shoulder joints.
use super::*;
use phys::V3;

impl RangeUnit {
    pub(super) fn aim_walker_arms(&mut self, turret_yaw: i32) {
        let Some(targets) = self
            .arm_targets
            .filter(|_| self.kind == RangeUnitKind::Walker04)
        else {
            return;
        };
        // The shoulder sockets ride on the same pitched crown as its root cells.
        // Legs keep the unpitched chassis/gait frames prepared by update_pose.
        let crown = self.aimed_frame(&self.pose, self.turret_delta, self.turret_pitch);
        let rest = crate::damage::segment_frames(&crown, &self.body, turret_yaw, turret_yaw);
        for (arm, target) in targets.into_iter().enumerate() {
            let first = 10 + arm * 4;
            let gun = rest[first + 3];
            let shoulder = rest[first].world(rest[first].centre);
            let muzzle = gun.world([Fx::from_int(4), Fx::from_int(2), Fx::from_int(2)]);
            let offset = muzzle.sub(shoulder);
            let d = V3::new(
                Fx::from_int(target[0]),
                Fx::from_int(target[1]),
                Fx::from_int(target[2]),
            )
            .sub(shoulder);
            // Solve bore azimuth/elevation including its transverse offset from
            // the shoulder. Longitudinal muzzle offset does not change aim.
            let side = offset.dot(gun.z);
            let height = offset.dot(gun.y);
            let horizontal = d.x.mul(d.x).add(d.z.mul(d.z));
            let forward = horizontal.sub(side.mul(side)).max(Fx::ZERO).sqrt();
            let yaw = fx::angle::diff(
                fx::angle::atan2(d.z.raw(), d.x.raw()),
                fx::angle::atan2(side.raw(), forward.raw()),
            );
            let along = forward
                .mul(forward)
                .add(d.y.mul(d.y))
                .sub(height.mul(height))
                .max(Fx::ZERO)
                .sqrt();
            let pitch = fx::angle::diff(
                fx::angle::atan2(d.y.raw(), forward.raw()),
                fx::angle::atan2(height.raw(), along.raw()),
            );
            let aim = crate::damage::Frame::new(
                &self.body.segments[first + 3].body,
                shoulder,
                yaw,
                V3::UP,
            );
            let aim = crate::damage::elevated(&aim, pitch, Fx::ZERO);
            let rotate = |v: V3| {
                aim.x
                    .scale(v.dot(gun.x))
                    .add(aim.y.scale(v.dot(gun.y)))
                    .add(aim.z.scale(v.dot(gun.z)))
            };
            // One rotation for the entire arm preserves every elbow/wrist
            // connection and leaves the shoulder fixed; no independent pivots.
            for index in first..first + 4 {
                let f = rest[index];
                self.seg_frames[index] = crate::damage::Frame {
                    base: shoulder.add(rotate(f.base.sub(shoulder))),
                    x: rotate(f.x),
                    y: rotate(f.y),
                    z: rotate(f.z),
                    ..f
                };
            }
        }
    }
}
