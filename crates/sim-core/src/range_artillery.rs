//! Barrel elevation and flight share one ballistic solution, in the hull frame.
use super::{battle::Weapon, weapons, ExplosionConfig, RangeUnit, RangeUnitKind};
use fx::{angle, Fx};
use phys::V3;

pub(super) struct Aim {
    pub yaw: i32,
    pub pitch: i32,
}

pub(super) fn elevates(kind: RangeUnitKind) -> bool {
    kind == RangeUnitKind::Spider || kind.artillery()
}

pub(super) fn valid_pitch(pitch: i32, kind: RangeUnitKind) -> bool {
    i64::from(pitch).abs() <= i64::from(angle::QUARTER) && (pitch == 0 || elevates(kind))
}

/// The central launch, before the canister's finite spread is added.
pub(super) fn velocity(
    from: [Fx; 3],
    target: [i32; 3],
    weapon: Weapon,
    config: &ExplosionConfig,
    flight_time_milli: i32,
) -> [Fx; 3] {
    if weapon == Weapon::Kinetic {
        return weapons::kinetic_velocity(from, target, config);
    }
    let delta: [Fx; 3] = std::array::from_fn(|k| Fx::from_int(target[k]).sub(from[k]));
    if weapon.energetic() {
        return weapons::flight_velocity(delta, config.shell_speed);
    }
    let time = flight_time(from, target, config.shell_speed, flight_time_milli);
    let mut v = delta.map(|d| d.div(time));
    // Flight integrates gravity before position. Include half a tick of lift.
    v[1] = v[1].add(
        Fx::from_int(config.gravity)
            .mul(time.add(Fx::from_ratio(1, crate::TICKS_PER_SECOND)))
            .mul(Fx::from_ratio(1, 2)),
    );
    v
}

pub(super) fn flight_time(from: [Fx; 3], target: [i32; 3], speed: i32, milli: i32) -> Fx {
    let x = Fx::from_int(target[0]).sub(from[0]);
    let z = Fx::from_int(target[2]).sub(from[2]);
    x.mul(x)
        .add(z.mul(z))
        .sqrt()
        .div(Fx::from_int(speed))
        .max(Fx::from_ratio(1, crate::TICKS_PER_SECOND))
        .mul(Fx::from_ratio(milli, 1000))
}

pub(super) fn solve(
    u: &RangeUnit,
    target: [i32; 3],
    weapon: Weapon,
    config: &ExplosionConfig,
    flight_time_milli: i32,
) -> Option<Aim> {
    let path = u
        .body
        .weapon_paths()
        .iter()
        .find(|p| u.body.barrel_connected(p, u.connectivity.reached_cols()))?;
    let cell = weapons::muzzle_cell(path);
    let mut aim = Aim {
        yaw: angle::norm(u.turret_delta + u.motion.drive.head),
        pitch: u.turret_pitch,
    };
    // The muzzle moves with elevation. A bounded fixed-point solve uses its
    // actual position, rather than aiming a trajectory from the hull centre.
    // Eight iterations are a numerical work bound, not a gameplay tuning.
    for _ in 0..8 {
        let f = u.aimed_frame(
            &u.pose,
            angle::diff(aim.yaw, u.motion.drive.head),
            aim.pitch,
        );
        let from = f.world(cell);
        let [x, y, z] = velocity(
            [from.x, from.y, from.z],
            target,
            weapon,
            config,
            flight_time_milli,
        );
        let v = V3::new(x, y, z);
        let local_x = v.dot(u.pose.x);
        let local_y = v.dot(u.pose.y);
        let local_z = v.dot(u.pose.z);
        let yaw = angle::norm(u.motion.drive.head + angle::atan2(local_z.raw(), local_x.raw()));
        let flat = local_x.mul(local_x).add(local_z.mul(local_z)).sqrt();
        let pitch = angle::diff(angle::atan2(local_y.raw(), flat.raw()), 0);
        let settled =
            angle::diff(yaw, aim.yaw).abs() <= 1 && angle::diff(pitch, aim.pitch).abs() <= 1;
        aim = Aim { yaw, pitch };
        if settled {
            break;
        }
    }
    Some(aim)
}

/// Firing tolerances must never make a shell visibly leave sideways/upward:
/// its central velocity follows the *current* physical barrel exactly.
pub(super) fn along_barrel(u: &RangeUnit, desired: [Fx; 3]) -> [Fx; 3] {
    let axis = u.root_frame(usize::from(u.body.turret_y)).x;
    let [x, y, z] = desired;
    let speed = V3::new(x, y, z).len_sq().sqrt();
    let v = axis.scale(speed.div(axis.len_sq().sqrt().max(Fx::ONE)));
    [v.x, v.y, v.z]
}
