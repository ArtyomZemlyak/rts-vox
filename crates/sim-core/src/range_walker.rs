//! Independent bearings, shoulder links and articulated elbows share hit poses.
use super::*;
use phys::V3;

pub(super) fn tuning() -> &'static modding::walker_rig::Tuning {
    static TUNING: std::sync::OnceLock<modding::walker_rig::Tuning> = std::sync::OnceLock::new();
    TUNING.get_or_init(|| {
        modding::walker_rig::Tuning::from_ron(include_str!("../../../data/walker-rig.ron"))
            .expect("stock walker rig")
    })
}

#[cfg(test)]
#[path = "range_walker_elbow_tests.rs"]
mod elbow_tests;

impl RangeUnit {
    pub(super) fn aim_walker_arms(&mut self, turret_yaw: i32) {
        if self.kind == RangeUnitKind::Commander {
            self.aim_commander_arms(turret_yaw);
            return;
        }
        if self.kind == RangeUnitKind::Walker04 && self.body.segments.len() >= 30 {
            self.aim_independent_walker_mounts();
            return;
        }
        let Some(targets) = self.arm_targets.filter(|_| {
            matches!(
                self.kind,
                RangeUnitKind::Walker04 | RangeUnitKind::Commander
            )
        }) else {
            return;
        };
        // The shoulder sockets ride on the same pitched crown as its root cells.
        // Legs keep the unpitched chassis/gait frames prepared by update_pose.
        let crown = self.aimed_frame(&self.pose, self.turret_delta, self.turret_pitch);
        let rest = crate::damage::segment_frames(&crown, &self.body, turret_yaw, turret_yaw);
        for (arm, target) in targets.into_iter().enumerate() {
            // Both tools retain their interpolated pose while changing modes.
            // Slots 2 and 3 are metadata on the two-armed commander.
            if self.kind == RangeUnitKind::Commander && arm >= 2 {
                continue;
            }
            let first = 10 + arm * 4;
            let gun = rest[first + 3];
            let shoulder = rest[first].world(rest[first].centre);
            let tip = &self.body.segments[first + 3].body;
            let muzzle = gun.world([
                Fx::from_int(i32::from(tip.vol.nx)),
                Fx::from_ratio(i32::from(tip.vol.ny), 2),
                Fx::from_ratio(i32::from(tip.vol.nz), 2),
            ]);
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
            let mut yaw = fx::angle::diff(
                fx::angle::atan2(d.z.raw(), d.x.raw()),
                fx::angle::atan2(side.raw(), forward.raw()),
            );
            if self.kind == RangeUnitKind::Commander {
                let limit = fx::angle::FULL * 35 / 360;
                yaw = fx::angle::norm(
                    self.motion.drive.head
                        + fx::angle::diff(yaw, self.motion.drive.head).clamp(-limit, limit),
                );
            } else if self.kind == RangeUnitKind::Walker04 {
                // A narrow forward sector prevents sweeping across the torso;
                // the shortened, offset mounts retain room to aim at one target.
                let centre = turret_yaw;
                let limit = fx::angle::FULL * 20 / 360;
                yaw = fx::angle::norm(centre + fx::angle::diff(yaw, centre).clamp(-limit, limit));
            }
            let along = forward
                .mul(forward)
                .add(d.y.mul(d.y))
                .sub(height.mul(height))
                .max(Fx::ZERO)
                .sqrt();
            let mut pitch = fx::angle::diff(
                fx::angle::atan2(d.y.raw(), forward.raw()),
                fx::angle::atan2(height.raw(), along.raw()),
            );
            if self.kind == RangeUnitKind::Walker04 {
                pitch = pitch.clamp(-fx::angle::FULL * 25 / 360, fx::angle::FULL * 25 / 360);
            }
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

    fn aim_independent_walker_mounts(&mut self) {
        let rest = crate::damage::segment_frames(
            &self.pose,
            &self.body,
            self.motion.drive.head,
            self.motion.drive.head,
        );
        for arm in 0..4 {
            let first = 10 + arm * 4;
            let mount = 26 + arm;
            let centre = rest[mount].base;
            let gun = rest[first + 3];
            let muzzle = gun.world([
                Fx::from_int(i32::from(self.body.segments[first + 3].body.vol.nx)),
                gun.centre[1],
                gun.centre[2],
            ]);
            let target = self.arm_targets.map(|targets| {
                V3::new(
                    Fx::from_int(targets[arm][0]),
                    Fx::from_int(targets[arm][1]),
                    Fx::from_int(targets[arm][2]),
                )
            });
            let orbit = if let Some(target) = target {
                let d = target.sub(centre);
                let side = muzzle.sub(centre).dot(gun.z);
                let dx = d.dot(self.pose.x);
                let dz = d.dot(self.pose.z);
                let r2 = dx.mul(dx).add(dz.mul(dz)).max(Fx::ONE);
                let forward = r2.sub(side.mul(side)).max(Fx::ZERO).sqrt();
                let c = dx.mul(forward).add(dz.mul(side)).div(r2);
                let s = dz.mul(forward).sub(dx.mul(side)).div(r2);
                let x = self.pose.x.scale(c).add(self.pose.z.scale(s));
                let length = x.len_sq().sqrt();
                let x = if length > Fx::from_ratio(1, 1000) {
                    x.scale(Fx::ONE.div(length))
                } else {
                    self.pose.x
                };
                crate::damage::Frame {
                    x,
                    z: x.cross(self.pose.y),
                    ..self.pose
                }
            } else {
                crate::damage::turned(
                    &self.pose,
                    fx::angle::norm(self.motion.drive.head + arm as i32 * fx::angle::QUARTER),
                    self.motion.drive.head,
                )
            };
            let rotate = |v: V3| {
                orbit
                    .x
                    .scale(v.dot(self.pose.x))
                    .add(orbit.y.scale(v.dot(self.pose.y)))
                    .add(orbit.z.scale(v.dot(self.pose.z)))
            };
            for index in (first..first + 4).chain(std::iter::once(mount)) {
                let f = rest[index];
                self.seg_frames[index] = crate::damage::Frame {
                    base: centre.add(rotate(f.base.sub(centre))),
                    x: rotate(f.x),
                    y: rotate(f.y),
                    z: rotate(f.z),
                    ..f
                };
            }
            let shoulder = self.seg_frames[first].base;
            // The upper link bends about its shoulder; the elbow hub follows
            // it, while the forearm retains the bore's reference orientation.
            // All children stay attached at the same physical joint positions.
            let upper = self.seg_frames[first];
            let elbow = self.seg_frames[first + 1].base;
            let gun = self.seg_frames[first + 3];
            let link = elbow.sub(shoulder).len_sq().sqrt();
            let forearm = gun.base.sub(elbow).len_sq().sqrt();
            let reach = link.add(forearm).max(Fx::ONE);
            let rig = tuning();
            let rest_bend = rig.arm_elbow_rest_degrees * fx::angle::FULL / 360;
            let swing = rig.arm_elbow_swing_degrees * fx::angle::FULL / 360;
            let bend = target.map_or(rest_bend, |target| {
                let d = target.sub(shoulder);
                let forward = d.dot(gun.x).max(Fx::ONE);
                let height = d.dot(gun.y);
                let elevation = fx::angle::diff(fx::angle::atan2(height.raw(), forward.raw()), 0);
                let near = reach.div(d.len_sq().sqrt().max(reach));
                (rest_bend - elevation / 2 + Fx::from_int(swing).mul(near).to_int_trunc())
                    .clamp(rest_bend - swing, rest_bend + swing)
            });
            let (c, s) = (fx::angle::cos(bend), fx::angle::sin(bend));
            let bent = crate::damage::Frame {
                x: upper.x.scale(c).sub(upper.y.scale(s)),
                y: upper.y.scale(c).add(upper.x.scale(s)),
                ..upper
            };
            let fold = |v: V3| bent.x.scale(v.dot(upper.x))
                .add(bent.y.scale(v.dot(upper.y))).add(bent.z.scale(v.dot(upper.z)));
            let bent_elbow = shoulder.add(fold(elbow.sub(shoulder)));
            for index in first..first + 2 {
                let f = self.seg_frames[index];
                self.seg_frames[index] = crate::damage::Frame {
                    base: shoulder.add(fold(f.base.sub(shoulder))),
                    x: fold(f.x), y: fold(f.y), z: fold(f.z), ..f
                };
            }
            for index in first + 2..first + 4 {
                self.seg_frames[index].base = self.seg_frames[index].base.add(bent_elbow.sub(elbow));
            }
            let gun = self.seg_frames[first + 3];
            let (c, s) = target.map_or((Fx::ONE, Fx::ZERO), |target| {
                let d = target.sub(shoulder);
                let forward = d.dot(gun.x).max(Fx::ONE);
                let height = d.dot(gun.y);
                let limit = tuning().arm_pitch_limit_degrees * fx::angle::FULL / 360;
                let muzzle = gun.world([Fx::from_int(i32::from(
                    self.body.segments[first + 3].body.vol.nx)), gun.centre[1], gun.centre[2]]);
                let offset = muzzle.sub(shoulder).dot(gun.y);
                let along = forward.mul(forward).add(height.mul(height))
                    .sub(offset.mul(offset)).max(Fx::ONE).sqrt();
                let pitch = fx::angle::diff(fx::angle::atan2(height.raw(), forward.raw()),
                    fx::angle::atan2(offset.raw(), along.raw()));
                if pitch.abs() > limit {
                    let pitch = pitch.clamp(-limit, limit);
                    (fx::angle::cos(pitch), fx::angle::sin(pitch))
                } else {
                    let r2 = forward.mul(forward).add(height.mul(height)).max(Fx::ONE);
                    // Analytic sin/cos retains laser precision beyond the
                    // drive's coarse trigonometric lookup table.
                    (forward.mul(along).add(height.mul(offset)).div(r2),
                        height.mul(along).sub(forward.mul(offset)).div(r2))
                }
            });
            let aim = crate::damage::Frame {
                x: gun.x.scale(c).add(gun.y.scale(s)),
                y: gun.y.scale(c).sub(gun.x.scale(s)),
                ..gun
            };
            let raise = |v: V3| {
                aim.x
                    .scale(v.dot(gun.x))
                    .add(aim.y.scale(v.dot(gun.y)))
                    .add(aim.z.scale(v.dot(gun.z)))
            };
            // Only the arm tilts at its spherical shoulder. The bearing stays
            // horizontal and remains concentric with the fixed central spine.
            for index in first..first + 4 {
                let f = self.seg_frames[index];
                self.seg_frames[index] = crate::damage::Frame {
                    base: shoulder.add(raise(f.base.sub(shoulder))),
                    x: raise(f.x),
                    y: raise(f.y),
                    z: raise(f.z),
                    ..f
                };
            }
        }
    }
}
