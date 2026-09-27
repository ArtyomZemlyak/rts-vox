//! Authored encounters use the same placement data as the range editor.
use crate::range::{RangeBlock, RangeUnitDef, RangeZone};
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
pub struct Skirmish {
    pub title: String,
    pub briefing: String,
    pub zone: RangeZone,
    pub units: Vec<RangeUnitDef>,
    pub blocks: Vec<RangeBlock>,
}
impl Skirmish {
    pub fn from_ron(src: &str) -> Result<Self, String> {
        let s: Self = ron::from_str(src).map_err(|e| e.to_string())?;
        if s.title.is_empty()
            || s.briefing.is_empty()
            || s.units.len() > crate::range::MAX_RANGE_UNITS
            || !s.units.iter().any(|u| u.allied)
            || !s.units.iter().any(|u| !u.allied)
        {
            return Err("skirmish needs a briefing and two bounded armies".into());
        }
        Ok(s)
    }
}
