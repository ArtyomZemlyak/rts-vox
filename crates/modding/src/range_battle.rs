//! Small, bounded skirmishes on the existing range.
use crate::range::{RangeUnitDef, RangeZone};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    pub tank_power_milli: u32,
    pub tank_reload_ticks: u32,
    pub tank_range: i32,
    pub pursuit_ticks: u32,
    pub standoff_milli: i32,
    pub max_rounds: usize,
    pub max_waves: usize,
    pub zone: RangeZone,
    pub units: Vec<RangeUnitDef>,
}
impl Tuning {
    pub fn from_ron(s: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(s).map_err(|e| e.to_string())?;
        if !(100..=4000).contains(&t.tank_power_milli)
            || !(1..=1800).contains(&t.tank_reload_ticks)
            || !(100..=3000).contains(&t.tank_range)
            || !(1..=300).contains(&t.pursuit_ticks)
            || !(100..=950).contains(&t.standoff_milli)
            || !(2..=128).contains(&t.max_rounds)
            || !(1..=8).contains(&t.max_waves)
            || t.units.len() > 8
            || !t.units.iter().any(|u| u.allied)
            || !t.units.iter().any(|u| !u.allied)
        {
            return Err("invalid battle tuning".into());
        }
        Ok(t)
    }
}
