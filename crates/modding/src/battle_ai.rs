//! Deterministic enemy decision rules. All weights are integer world units.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Tuning {
    pub decision_ticks: u32,
    pub focus_fire: bool,
    pub armed_bonus: i32,
    pub mobile_bonus: i32,
    pub switch_margin: i32,
}

impl Tuning {
    pub fn from_ron(src: &str) -> Result<Self, String> {
        let value: Self = ron::from_str(src).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=300).contains(&self.decision_ticks)
            || !(0..=10_000).contains(&self.armed_bonus)
            || !(0..=10_000).contains(&self.mobile_bonus)
            || !(0..=10_000).contains(&self.switch_margin)
        {
            return Err("invalid battle AI tuning".into());
        }
        Ok(())
    }
}
