//! Two-link arms. The same joint frames drive rendering, hits and emitters.
use super::*;
use crate::damage::Frame;
use phys::V3;

pub(super) fn tuning() -> &'static modding::commander_rig::Tuning {
    static TUNING: OnceLock<modding::commander_rig::Tuning> = OnceLock::new();
    TUNING.get_or_init(|| modding::commander_rig::Tuning::from_ron(
        include_str!("../../../data/commander-rig.ron")).expect("stock commander rig"))
}

fn degrees(value: i32) -> i32 { value * fx::angle::FULL / 360 }
fn point(p: [i32; 3]) -> V3 {
    V3::new(Fx::from_int(p[0]), Fx::from_int(p[1]), Fx::from_int(p[2]))
}
fn coords(p: V3) -> [i32; 3] {
    [p.x.to_int_trunc(), p.y.to_int_trunc(), p.z.to_int_trunc()]
}

impl RangeUnit {
    /// Read the actual articulated emitter, not its requested aim point.
    pub(crate) fn commander_arm_ready(&self, arm: usize, target: [i32; 3]) -> bool {
        if self.kind != RangeUnitKind::Commander || !self.status.alive || arm >= 2 { return false; }
        let first = 10 + arm * 4;
        if self.seg_gone.get(first..first+4).is_none_or(|parts|parts.iter().any(|gone|*gone)) { return false; }
        let Some(frame) = self.seg_frames.get(first+3) else { return false; };
        let tip = &self.body.segments[first+3].body;
        if tip.solid_count()==0 { return false; }
        let from = frame.world([Fx::from_int(i32::from(tip.vol.nx)),
            Fx::from_ratio(i32::from(tip.vol.ny),2),Fx::from_ratio(i32::from(tip.vol.nz),2)]);
        let delta = point(target).sub(from);
        let forward = frame.x.dot(delta);
        let cosine = fx::angle::cos(degrees(tuning().work_ready_degrees));
        forward > Fx::ZERO && forward.mul(forward) >= delta.len_sq().mul(cosine.mul(cosine))
    }

    pub(super) fn commander_rest_targets(&self, turret_yaw: i32) -> [[i32; 3]; 2] {
        let crown = self.aimed_frame(&self.pose, fx::angle::diff(turret_yaw, self.motion.drive.head), self.turret_pitch);
        let speed = self.motion.drive.vel.abs().div(Fx::from_int(self.motion.speed.max(1))).min(Fx::ONE);
        let swing = fx::angle::sin(self.motion.phase).mul(speed)
            .mul(Fx::from_int(degrees(tuning().walk_swing_degrees))).to_int_trunc();
        std::array::from_fn(|arm| {
            let pitch = degrees(tuning().rest_pitch_degrees) + if arm == 0 { swing } else { -swing };
            let direction = self.pose.x.scale(fx::angle::cos(pitch))
                .add(self.pose.y.scale(fx::angle::sin(pitch)));
            let at = self.body.segments[10 + arm * 4].attach.at;
            coords(crown.world([Fx::from_int(i32::from(at.0)), Fx::from_int(i32::from(at.1)), Fx::from_int(i32::from(at.2))])
                .add(direction.scale(Fx::from_int(tuning().rest_lookahead_cm))))
        })
    }

    /// Bound angular travel even for a close target crossing the shoulder.
    /// Targets remain the existing saved servo state; pose rebuilds never
    /// advance the mechanism, so drawing/hit queries cannot speed it up.
    pub(super) fn commander_limit_slew(&self, arm: usize, prior: [i32; 3], next: [i32; 3], turret_yaw: i32) -> [i32; 3] {
        let crown = self.aimed_frame(&self.pose, fx::angle::diff(turret_yaw, self.motion.drive.head), self.turret_pitch);
        let at = self.body.segments[10 + arm * 4].attach.at;
        let origin = crown.world([Fx::from_int(i32::from(at.0)), Fx::from_int(i32::from(at.1)), Fx::from_int(i32::from(at.2))]);
        let before = point(prior).sub(origin);
        let after = point(next).sub(origin);
        let angles = |d: V3| {
            let horizontal = d.x.mul(d.x).add(d.z.mul(d.z)).sqrt();
            (fx::angle::atan2(d.z.raw(), d.x.raw()), fx::angle::atan2(d.y.raw(), horizontal.raw()))
        };
        let (by, bp) = angles(before);
        let (ay, ap) = angles(after);
        let limit = degrees(tuning().slew_degrees_s) / crate::TICKS_PER_SECOND;
        let dy = fx::angle::diff(ay, by);
        let dp = fx::angle::diff(ap, bp);
        if dy.abs() <= limit && dp.abs() <= limit { return next; }
        let yaw = fx::angle::norm(by + dy.clamp(-limit, limit));
        let pitch = bp + dp.clamp(-limit, limit);
        let radius = after.len_sq().sqrt();
        let horizontal = radius.mul(fx::angle::cos(pitch));
        coords(origin.add(V3::new(horizontal.mul(fx::angle::cos(yaw)),
            radius.mul(fx::angle::sin(pitch)), horizontal.mul(fx::angle::sin(yaw)))))
    }

    pub(super) fn aim_commander_arms(&mut self, turret_yaw: i32) {
        let rig = tuning();
        let crown = self.aimed_frame(&self.pose, self.turret_delta, self.turret_pitch);
        let mut rest = crate::damage::segment_frames(&crown, &self.body, turret_yaw, turret_yaw);
        let targets = self.arm_targets.map(|p| [p[0], p[1]])
            .unwrap_or_else(|| self.commander_rest_targets(turret_yaw));
        for arm in 0..2 {
            let first = 10 + arm * 4;
            let shoulder = rest[first].base;
            let target = point(targets[arm]);
            let gun = rest[first + 3];
            let d = target.sub(shoulder);
            let horizontal = d.x.mul(d.x).add(d.z.mul(d.z)).sqrt();
            let elevation = fx::angle::diff(fx::angle::atan2(d.y.raw(), horizontal.raw()), 0);
            // A lowered forearm leaves the upper link nearly vertical. Raising
            // the tool opens a real elbow instead of swinging one rigid bar.
            let bend = (degrees(rig.elbow_degrees) + elevation / 2)
                .clamp(degrees(rig.elbow_degrees - rig.elbow_swing_degrees),
                    degrees(rig.elbow_degrees + rig.elbow_swing_degrees));
            let upper = rest[first];
            let current = fx::angle::atan2(upper.x.dot(gun.y).raw(), upper.x.dot(gun.x).raw());
            let turn = -bend - fx::angle::diff(current, 0);
            let (c, s) = (fx::angle::cos(turn), fx::angle::sin(turn));
            let fold = |v: V3| {
                let x = v.dot(gun.x); let y = v.dot(gun.y);
                gun.x.scale(x.mul(c).sub(y.mul(s)))
                    .add(gun.y.scale(x.mul(s).add(y.mul(c))))
                    .add(gun.z.scale(v.dot(gun.z)))
            };
            let elbow = rest[first + 1].base;
            let bent_elbow = shoulder.add(fold(elbow.sub(shoulder)));
            for index in first..first + 2 {
                let f = rest[index];
                rest[index] = Frame { base: shoulder.add(fold(f.base.sub(shoulder))),
                    x: fold(f.x), y: fold(f.y), z: fold(f.z), ..f };
            }
            for index in first + 2..first + 4 {
                rest[index].base = rest[index].base.add(bent_elbow.sub(elbow));
            }

            // Aim the now bent chain, accounting for the changed muzzle
            // offset. Shoulder yaw/pitch limits keep tools out of the torso.
            let gun = rest[first + 3];
            let tip = &self.body.segments[first + 3].body;
            let muzzle = gun.world([Fx::from_int(i32::from(tip.vol.nx)),
                Fx::from_ratio(i32::from(tip.vol.ny), 2), Fx::from_ratio(i32::from(tip.vol.nz), 2)]);
            let offset = muzzle.sub(shoulder);
            let side = offset.dot(gun.z);
            let height = offset.dot(gun.y);
            let forward = horizontal.mul(horizontal).sub(side.mul(side)).max(Fx::ZERO).sqrt();
            let yaw = fx::angle::diff(fx::angle::atan2(d.z.raw(), d.x.raw()), fx::angle::atan2(side.raw(), forward.raw()));
            let yaw = fx::angle::norm(self.motion.drive.head
                + fx::angle::diff(yaw, self.motion.drive.head)
                    .clamp(-degrees(rig.yaw_limit_degrees), degrees(rig.yaw_limit_degrees)));
            let along = forward.mul(forward).add(d.y.mul(d.y)).sub(height.mul(height)).max(Fx::ZERO).sqrt();
            let pitch = fx::angle::diff(fx::angle::atan2(d.y.raw(), forward.raw()), fx::angle::atan2(height.raw(), along.raw()))
                .clamp(-degrees(rig.pitch_down_degrees), degrees(rig.pitch_up_degrees));
            let aim = crate::damage::elevated(&Frame::new(tip, shoulder, yaw, V3::UP), pitch, Fx::ZERO);
            let rotate = |v: V3| aim.x.scale(v.dot(gun.x))
                .add(aim.y.scale(v.dot(gun.y))).add(aim.z.scale(v.dot(gun.z)));
            for index in first..first + 4 {
                let f = rest[index];
                self.seg_frames[index] = Frame { base: shoulder.add(rotate(f.base.sub(shoulder))),
                    x: rotate(f.x), y: rotate(f.y), z: rotate(f.z), ..f };
            }
        }
    }
}

#[cfg(test)]
#[path = "range_commander_arm_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "range_commander_work_tests.rs"]
mod work_tests;
