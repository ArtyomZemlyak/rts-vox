//! Barrel elevation and flight share one ballistic solution, in the hull frame.
use super::{battle::Weapon, weapons, ExplosionConfig, RangeUnit, RangeUnitKind};
use fx::{angle, Fx};
use phys::V3;

#[derive(Clone, Copy)]
pub(super) struct Aim {
    pub yaw: i32,
    pub pitch: i32,
    pub target: [i32; 3],
}

pub(super) fn elevates(kind: RangeUnitKind) -> bool {
    kind == RangeUnitKind::Spider || kind.artillery()
}

pub(super) fn valid_pitch(pitch: i32, kind: RangeUnitKind) -> bool {
    i64::from(pitch).abs() <= i64::from(angle::QUARTER) && (pitch == 0 || elevates(kind))
}

/// A broad traverse tolerance must not launch on another physical direction.
/// Long, high arcs amplify even a fraction of a degree into a hull-sized miss.
pub(super) fn same_direction(a: i32, b: i32) -> bool {
    angle::sin(a) == angle::sin(b) && angle::cos(a) == angle::cos(b)
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
    solve_moving(u, target, [Fx::ZERO; 3], weapon, config, flight_time_milli)
}

/// Constant-velocity interception. Normalize before forming the quadratic:
/// squaring distance * speed would overflow Fx even inside the map limits.
pub(super) fn intercept(
    from: [Fx; 3],
    target: [i32; 3],
    motion: [Fx; 3],
    weapon: Weapon,
    config: &ExplosionConfig,
    flight_time_milli: i32,
) -> [i32; 3] {
    if motion == [Fx::ZERO; 3] {
        return target;
    }
    let multiplier = if weapon.energetic() || weapon == Weapon::Kinetic {
        Fx::ONE
    } else {
        Fx::from_ratio(flight_time_milli, 1000)
    };
    let speed = Fx::from_int(config.shell_speed).div(multiplier);
    let mut delta: [Fx; 3] = std::array::from_fn(|k| Fx::from_int(target[k]).sub(from[k]));
    let mut travel = motion;
    if !weapon.energetic() {
        delta[1] = Fx::ZERO;
        travel[1] = Fx::ZERO;
    }
    let extent = delta.iter().map(|d| d.abs()).max().unwrap().max(Fx::ONE);
    let relative = delta.map(|d| d.div(extent));
    let length = relative
        .iter()
        .fold(Fx::ZERO, |n, d| n.add(d.mul(*d)))
        .sqrt();
    let time = if length == Fx::ZERO {
        Fx::ZERO
    } else {
        let direction = relative.map(|d| d.div(length));
        let drift = travel.map(|v| v.div(speed));
        let radial = (0..3).fold(Fx::ZERO, |n, k| n.add(direction[k].mul(drift[k])));
        let drift_sq = drift.iter().fold(Fx::ZERO, |n, v| n.add(v.mul(*v)));
        let discriminant = radial.mul(radial).add(Fx::ONE.sub(drift_sq));
        if discriminant < Fx::ZERO {
            return target;
        }
        let closing = discriminant.sqrt().sub(radial);
        if closing <= Fx::ZERO {
            return target;
        }
        let base = extent.mul(length).div(speed);
        let raw = (i128::from(base.raw()) << fx::FRAC_BITS) / i128::from(closing.raw());
        let Ok(raw) = i64::try_from(raw) else {
            return target;
        };
        let time = Fx(raw);
        // No solution beyond Fx's safe distance domain. This is a numeric
        // bound, not an attack-range or balance adjustment.
        if time > Fx::from_int(Fx::MAX_SAFE_COORD).div(speed) {
            return target;
        }
        time
    };
    // Match the ballistic launch's minimum flight interval at close range.
    let time = if weapon.energetic() {
        time
    } else {
        time.max(Fx::from_ratio(1, crate::TICKS_PER_SECOND).mul(multiplier))
    };
    std::array::from_fn(|k| {
        Fx::from_int(target[k])
            .add(motion[k].mul(time))
            .to_int_trunc()
    })
}

pub(super) fn solve_moving(
    u: &RangeUnit,
    target: [i32; 3],
    motion: [Fx; 3],
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
        target,
    };
    let mut best: Option<(Fx, Aim)> = None;
    // The muzzle moves with elevation. A bounded fixed-point solve uses its
    // actual position, rather than aiming a trajectory from the hull centre.
    // The longer T3 barrel moves its muzzle farther while turning, so it
    // needs more iterations for nearby targets; retain a fixed work bound.
    let iterations = if u.kind == RangeUnitKind::ArtilleryTurretT3 {
        16
    } else {
        8
    };
    for _ in 0..iterations {
        let f = u.aimed_frame(
            &u.pose,
            angle::diff(aim.yaw, u.motion.drive.head),
            aim.pitch,
        );
        let from = f.world(cell);
        let point = intercept(
            [from.x, from.y, from.z],
            target,
            motion,
            weapon,
            config,
            flight_time_milli,
        );
        let [x, y, z] = velocity(
            [from.x, from.y, from.z],
            point,
            weapon,
            config,
            flight_time_milli,
        );
        let v = V3::new(x, y, z);
        // Muzzle motion plus angular quantization can form a two-cell cycle.
        // Retain the closest physical direction actually visited, rather
        // than commanding the last opposite cell indefinitely.
        let alignment =
            f.x.dot(v)
                .div(f.x.len_sq().sqrt())
                .div(v.len_sq().sqrt().max(Fx::ONE));
        let candidate = Aim {
            target: point,
            ..aim
        };
        if best.is_none_or(|(score, old)| {
            alignment > score
                || (alignment == score && (candidate.yaw, candidate.pitch) < (old.yaw, old.pitch))
        }) {
            best = Some((alignment, candidate));
        }
        let local_x = v.dot(u.pose.x);
        let local_y = v.dot(u.pose.y);
        let local_z = v.dot(u.pose.z);
        let yaw = angle::norm(u.motion.drive.head + angle::atan2(local_z.raw(), local_x.raw()));
        let flat = local_x.mul(local_x).add(local_z.mul(local_z)).sqrt();
        let pitch = angle::diff(angle::atan2(local_y.raw(), flat.raw()), 0);
        let settled =
            angle::diff(yaw, aim.yaw).abs() <= 1 && angle::diff(pitch, aim.pitch).abs() <= 1;
        // Damping prevents a long muzzle from overshooting back and forth
        // around a close target as the barrel's pivot moves during the solve.
        let (yaw, pitch) = if u.kind == RangeUnitKind::ArtilleryTurretT3 && !settled {
            (
                angle::norm(aim.yaw + angle::diff(yaw, aim.yaw) / 2),
                aim.pitch + angle::diff(pitch, aim.pitch) / 2,
            )
        } else {
            (yaw, pitch)
        };
        aim = Aim {
            yaw,
            pitch,
            target: point,
        };
        if settled {
            break;
        }
    }
    aim = best?.1;
    // Traverse's tiny residual must not straddle a sine-table boundary.
    // Command the centre of the same representable physical direction.
    aim.yaw = angle::atan2(angle::sin(aim.yaw).raw(), angle::cos(aim.yaw).raw());
    aim.pitch = angle::diff(
        angle::atan2(angle::sin(aim.pitch).raw(), angle::cos(aim.pitch).raw()),
        0,
    );
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
