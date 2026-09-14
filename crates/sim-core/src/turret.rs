//! Deterministic angular servo. Rate and acceleration are bounded; estimated
//! target motion lets the barrel follow a crossing vehicle without persistent
//! proportional lag. The actuator owns no projectile or renderer state.
use fx::{
    angle,
    blob::{Blob, Scan},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Traverse {
    pub angle: i32,
    pub rate: i32,
    previous_target: i32,
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub speed: i32,
    pub accel: i32,
    pub tolerance: i32,
}
impl Limits {
    pub fn of(t: modding::turret::Turret, hz: i32) -> Self {
        assert!(hz > 0);
        let convert = |n: i32, d: i64| ((i64::from(n) * i64::from(angle::FULL)) / d).max(1) as i32;
        Self {
            speed: convert(t.speed_deg_s, 360 * i64::from(hz)),
            accel: convert(t.accel_deg_s2, 360 * i64::from(hz).pow(2)),
            tolerance: convert(t.aim_millidegrees, 360_000),
        }
    }
}
impl Traverse {
    pub fn new(at: i32) -> Self {
        Self {
            angle: angle::norm(at),
            rate: 0,
            previous_target: angle::norm(at),
        }
    }
    pub fn step(&mut self, target: i32, l: Limits) {
        let following = angle::diff(target, self.previous_target).clamp(-l.speed, l.speed);
        // Compare against this tick's predicted position, otherwise feed-forward
        // plus correction leads a moving target by one whole tick.
        let err = angle::diff(target, angle::norm(self.angle.wrapping_add(following)));
        self.previous_target = angle::norm(target);
        let correction = ((i64::from(err) * i64::from(l.accel)) / i64::from(l.speed))
            .clamp(-i64::from(err.abs()), i64::from(err.abs())) as i32;
        let wanted = following
            .saturating_add(correction)
            .clamp(-l.speed, l.speed);
        self.rate += (wanted - self.rate).clamp(-l.accel, l.accel);
        self.rate = self.rate.clamp(-l.speed, l.speed);
        self.angle = angle::norm(self.angle.wrapping_add(self.rate));
    }
    pub fn aligned(&self, target: i32, l: Limits) -> bool {
        angle::diff(target, self.angle).abs() <= l.tolerance
    }
    pub fn save(&self, b: &mut Blob) {
        b.i32(self.angle);
        b.i32(self.rate);
        b.i32(self.previous_target);
    }
    pub fn load(s: &mut Scan, l: Limits) -> Option<Self> {
        let v = Self {
            angle: s.i32(),
            rate: s.i32(),
            previous_target: s.i32(),
        };
        (s.ok()
            && v.angle == angle::norm(v.angle)
            && v.previous_target == angle::norm(v.previous_target)
            && i64::from(v.rate).abs() <= i64::from(l.speed))
        .then_some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> Limits {
        Limits::of(
            modding::turret::Turret::from_ron(include_str!("../../../data/turret.ron")).unwrap(),
            30,
        )
    }
    #[test]
    fn traverse_accelerates_brakes_wraps_and_resumes_while_reversing() {
        let l = limits();
        let mut a = Traverse::new(angle::FULL - 100);
        for target in [angle::QUARTER, angle::FULL - angle::QUARTER, 100] {
            for tick in 0..250 {
                let before = a;
                a.step(target, l);
                assert!((a.rate - before.rate).abs() <= l.accel);
                assert!(angle::diff(a.angle, before.angle).abs() <= l.speed);
                if tick == 10 {
                    let mut b = Blob::new();
                    a.save(&mut b);
                    let mut restored = Traverse::load(&mut Scan::new(&b.v), l).unwrap();
                    let mut expected = a;
                    for _ in 0..35 {
                        restored.step(target, l);
                        expected.step(target, l);
                    }
                    assert_eq!(restored, expected);
                }
            }
            assert!(a.aligned(target, l));
            assert_eq!(a.rate, 0);
        }
    }
    #[test]
    fn a_crossing_target_can_be_followed_without_teleporting() {
        let l = limits();
        let mut a = Traverse::new(0);
        let mut target = 0;
        for _ in 0..300 {
            target = angle::norm(target + l.speed / 3);
            let old = a;
            a.step(target, l);
            assert!((a.rate - old.rate).abs() <= l.accel);
        }
        assert!(a.aligned(target, l));
    }
}
