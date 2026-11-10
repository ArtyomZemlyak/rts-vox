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
    pub arm_pitch_limit_degrees: i32,
    #[serde(default = "elbow_rest")]
    pub arm_elbow_rest_degrees: i32,
    #[serde(default = "elbow_swing")]
    pub arm_elbow_swing_degrees: i32,
    pub arm_fire_error_units: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn walker_elbow_limits_load_stock_and_legacy_and_reject_impossible_folds() {
        let stock = include_str!("../../../data/walker-rig.ron");
        let loaded = Tuning::from_ron(stock).unwrap();
        let legacy = stock.replace("    arm_elbow_rest_degrees: 55,", "")
            .replace("    arm_elbow_swing_degrees: 20,", "");
        let old = Tuning::from_ron(&legacy).unwrap();
        assert_eq!(loaded.arm_elbow_rest_degrees, old.arm_elbow_rest_degrees);
        assert_eq!(loaded.arm_elbow_swing_degrees, old.arm_elbow_swing_degrees);
        for (key, bad) in [("arm_elbow_rest_degrees: 55", "arm_elbow_rest_degrees: 90"),
            ("arm_elbow_swing_degrees: 20", "arm_elbow_swing_degrees: 40")] {
            assert!(Tuning::from_ron(&stock.replace(key, bad)).is_err());
        }
    }
}
fn elbow_rest() -> i32 { 55 }
fn elbow_swing() -> i32 { 20 }
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
            || !(5..=45).contains(&t.arm_pitch_limit_degrees)
            || !(20..=70).contains(&t.arm_elbow_rest_degrees)
            || !(0..=25).contains(&t.arm_elbow_swing_degrees)
            || t.arm_elbow_rest_degrees - t.arm_elbow_swing_degrees < 15
            || t.arm_elbow_rest_degrees + t.arm_elbow_swing_degrees > 85
            || !(1..=4).contains(&t.arm_fire_error_units)
        {
            return Err("invalid walker rig limits".into());
        }
        Ok(t)
    }
}
