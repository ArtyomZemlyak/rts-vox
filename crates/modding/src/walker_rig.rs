//! Mechanical limits of the heavy four-arm biped, separate from the strider.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Tuning {
    pub stride_cm: i32,
    pub duty_pct: i32,
    pub offset_pct: [i32; 4],
    pub lift_cm: i32,
    pub turn_step_degrees: i32,
    pub turn_deg_s: i32,
    pub arm_turn_deg_s: i32,
    #[serde(default = "arm_settle")]
    pub arm_settle_ticks: i32,
    pub arm_pitch_limit_degrees: i32,
    #[serde(default = "elbow_rest")]
    pub arm_elbow_rest_degrees: i32,
    #[serde(default = "elbow_swing")]
    pub arm_elbow_swing_degrees: i32,
    pub arm_fire_error_units: i32,
    #[serde(default = "support_x")]
    pub support_x_cm: i32,
    #[serde(default = "support_z")]
    pub support_z_cm: i32,
    #[serde(default = "max_tilt")]
    pub max_tilt_pct: i32,
    #[serde(default = "hull_inertia")]
    pub inertia: i32,
    #[serde(default = "settle_tilt")]
    pub settle_tilt_pct: i32,
    #[serde(default = "body_sway")]
    pub body_sway_pct: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn walker_elbow_limits_load_stock_and_legacy_and_reject_impossible_folds() {
        let stock = include_str!("../../../data/walker-rig.ron");
        let loaded = Tuning::from_ron(stock).unwrap();
        let legacy = stock
            .replace("    arm_settle_ticks: 6,", "")
            .replace("    arm_elbow_rest_degrees: 55,", "")
            .replace("    arm_elbow_swing_degrees: 20,", "")
            .replace("    support_x_cm: 240,", "")
            .replace("    support_z_cm: 200,", "")
            .replace("    max_tilt_pct: 99,", "")
            .replace("    inertia: 600,", "")
            .replace("    settle_tilt_pct: 15,", "")
            .replace("    body_sway_pct: 8,", "");
        let old = Tuning::from_ron(&legacy).unwrap();
        assert_eq!(loaded.arm_settle_ticks, old.arm_settle_ticks);
        assert_eq!(loaded.arm_elbow_rest_degrees, old.arm_elbow_rest_degrees);
        assert_eq!(loaded.arm_elbow_swing_degrees, old.arm_elbow_swing_degrees);
        assert_eq!(loaded.support_x_cm, old.support_x_cm);
        assert_eq!(loaded.support_z_cm, old.support_z_cm);
        assert_eq!(loaded.max_tilt_pct, old.max_tilt_pct);
        assert_eq!(loaded.inertia, old.inertia);
        assert_eq!(loaded.settle_tilt_pct, old.settle_tilt_pct);
        assert_eq!(loaded.body_sway_pct, old.body_sway_pct);
        for (key, bad) in [
            ("arm_settle_ticks: 6", "arm_settle_ticks: 0"),
            ("arm_elbow_rest_degrees: 55", "arm_elbow_rest_degrees: 90"),
            ("arm_elbow_swing_degrees: 20", "arm_elbow_swing_degrees: 40"),
            ("support_x_cm: 240", "support_x_cm: 10"),
            ("max_tilt_pct: 99", "max_tilt_pct: 70"),
            ("inertia: 600", "inertia: 50"),
            ("body_sway_pct: 8", "body_sway_pct: 60"),
        ] {
            assert!(Tuning::from_ron(&stock.replace(key, bad)).is_err());
        }
    }
}
fn arm_settle() -> i32 { 6 }
fn elbow_rest() -> i32 { 55 }
fn elbow_swing() -> i32 { 20 }
fn support_x() -> i32 { 240 }
fn support_z() -> i32 { 200 }
fn max_tilt() -> i32 { 99 }
fn hull_inertia() -> i32 { 600 }
fn settle_tilt() -> i32 { 15 }
fn body_sway() -> u8 { 8 }
impl Tuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(100..=1000).contains(&t.stride_cm)
            || !(50..=80).contains(&t.duty_pct)
            || t.offset_pct.iter().any(|v| !(0..100).contains(v))
            || !(10..=150).contains(&t.lift_cm)
            || !(10..=60).contains(&t.turn_step_degrees)
            || !(10..=90).contains(&t.turn_deg_s)
            || !(5..=120).contains(&t.arm_turn_deg_s)
            || !(2..=30).contains(&t.arm_settle_ticks)
            || !(5..=45).contains(&t.arm_pitch_limit_degrees)
            || !(20..=70).contains(&t.arm_elbow_rest_degrees)
            || !(0..=25).contains(&t.arm_elbow_swing_degrees)
            || t.arm_elbow_rest_degrees - t.arm_elbow_swing_degrees < 15
            || t.arm_elbow_rest_degrees + t.arm_elbow_swing_degrees > 85
            || !(1..=4).contains(&t.arm_fire_error_units)
            || !(50..=400).contains(&t.support_x_cm)
            || !(50..=400).contains(&t.support_z_cm)
            || !(80..=100).contains(&t.max_tilt_pct)
            || !(100..=5000).contains(&t.inertia)
            || !(0..=100).contains(&t.settle_tilt_pct)
            || t.body_sway_pct > 50
        {
            return Err("invalid walker rig limits".into());
        }
        Ok(t)
    }
}
