//! Mechanical limits of the commander's two different articulated tools.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Tuning {
    pub yaw_limit_degrees: i32,
    pub pitch_down_degrees: i32,
    pub pitch_up_degrees: i32,
    pub rest_pitch_degrees: i32,
    pub walk_swing_degrees: i32,
    pub elbow_degrees: i32,
    pub elbow_swing_degrees: i32,
    pub slew_degrees_s: i32,
    pub settle_ticks: i32,
    pub target_step_cm: i32,
    pub rest_lookahead_cm: i32,
}

impl Tuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(10..=45).contains(&t.yaw_limit_degrees)
            || !(30..=85).contains(&t.pitch_down_degrees)
            || !(10..=60).contains(&t.pitch_up_degrees)
            || !(-t.pitch_down_degrees..=-20).contains(&t.rest_pitch_degrees)
            || !(0..=12).contains(&t.walk_swing_degrees)
            || !(25..=65).contains(&t.elbow_degrees)
            || !(0..=25).contains(&t.elbow_swing_degrees)
            || t.elbow_degrees - t.elbow_swing_degrees < 15
            || t.elbow_degrees + t.elbow_swing_degrees > 85
            || !(10..=90).contains(&t.slew_degrees_s)
            || !(2..=30).contains(&t.settle_ticks)
            || !(1..=80).contains(&t.target_step_cm)
            || !(100..=1000).contains(&t.rest_lookahead_cm)
        {
            return Err("invalid commander arm limits".into());
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commander_rig_rejects_intersecting_folds_and_invalid_servos() {
        let stock = include_str!("../../../data/commander-rig.ron");
        Tuning::from_ron(stock).unwrap();
        for (valid, invalid) in [
            ("elbow_degrees: 50", "elbow_degrees: 80"),
            ("yaw_limit_degrees: 35", "yaw_limit_degrees: 90"),
            ("settle_ticks: 6", "settle_ticks: 0"),
            ("rest_pitch_degrees: -55", "rest_pitch_degrees: -90"),
        ] {
            assert!(Tuning::from_ron(&stock.replace(valid, invalid)).is_err());
        }
    }
}
