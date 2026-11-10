//! Travel alignment uses the same saved angular actuator as combat aiming.
use super::*;

pub(super) fn follow(unit: &mut RangeUnit, actor: &mut Actor, limits: crate::turret::Limits) {
    if unit.kind.emplacement() || !unit.status.alive || !unit.status.weapon { return; }
    let before=(actor.turret.angle,unit.turret_pitch);
    let tracking = if unit.kind == RangeUnitKind::Spider {motion::spider_tuning().laser_tracking} else {None};
    let yaw = tracking.map_or(limits,|t| crate::turret::Limits::of(t.yaw,crate::TICKS_PER_SECOND));
    let elevation = tracking.map_or(limits,|t| crate::turret::Limits::of(t.elevation,crate::TICKS_PER_SECOND));
    let pitch_limit = tracking.map_or(fx::angle::QUARTER,
        |t|t.elevation_limit_degrees*fx::angle::FULL/360);
    actor.turret.step(unit.motion.travel_heading(), yaw);
    actor.elevation.step(0, elevation);
    let pitch = fx::angle::diff(actor.elevation.angle, 0);
    unit.turret_pitch = pitch.clamp(-pitch_limit, pitch_limit);
    if pitch != unit.turret_pitch {
        actor.elevation.angle = fx::angle::norm(unit.turret_pitch); actor.elevation.rate = 0;
    }
    if unit.kind == RangeUnitKind::Walker04 && unit.body.segments.len() >= 26 {
        // Reuse each arm's bounded slew, rather than snapping its bearing to
        // a rest frame when the last enemy disappears.
        let heading = actor.turret.angle;
        let direction = phys::V3::new(fx::angle::cos(heading), Fx::ZERO, fx::angle::sin(heading));
        let targets = std::array::from_fn(|arm| {
            let gun = &unit.seg_frames[13 + arm * 4];
            let p = unit.pose.base.add(direction.scale(Fx::from_int(300)));
            [p.x.to_int_trunc(), gun.base.y.to_int_trunc(), p.z.to_int_trunc()]
        });
        if unit.arm_targets != Some(targets) {unit.set_arm_targets(targets, heading);}
        else if before!=(actor.turret.angle,unit.turret_pitch) {unit.update_pose(heading);}
    } else if unit.kind == RangeUnitKind::Commander && unit.arm_targets.is_some() {
        unit.relax_commander_arms(actor.turret.angle);
    } else if before!=(actor.turret.angle,unit.turret_pitch)
        || unit.turret_delta != fx::angle::diff(actor.turret.angle,unit.motion.drive.head) {
        unit.update_pose(actor.turret.angle);
    }
}

#[cfg(test)]
#[path = "range_idle_aim_tests.rs"]
mod tests;
