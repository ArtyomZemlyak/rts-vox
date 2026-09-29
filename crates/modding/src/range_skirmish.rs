//! Authored encounters use the same placement data as the range editor.
use crate::range::{RangeBlock, RangeUnitDef, RangeUnitKind, RangeZone};
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

/// A map-wide battle keeps the authored terrain and uses the bases as its
/// victory objective. Positions are authored, so loading never invents an
/// army based on whatever units happened to be in the editor.
#[derive(Clone, Debug, Deserialize)]
pub struct WorldBattle {
    pub title: String,
    pub briefing: String,
    pub units: Vec<RangeUnitDef>,
    #[serde(default)]
    pub blocks: Vec<RangeBlock>,
}
impl WorldBattle {
    pub fn from_ron(src: &str) -> Result<Self, String> {
        let battle: Self = ron::from_str(src).map_err(|e| e.to_string())?;
        if battle.title.trim().is_empty()
            || battle.briefing.trim().is_empty()
            || battle.units.len() > crate::range::MAX_RANGE_UNITS
            || battle.blocks.len() > crate::range::MAX_RANGE_BLOCKS
            || [true, false].iter().any(|&side| {
                battle
                    .units
                    .iter()
                    .filter(|u| u.allied == side && u.kind == RangeUnitKind::Base)
                    .count()
                    != 1
                    || !battle
                        .units
                        .iter()
                        .any(|u| u.allied == side && u.kind != RangeUnitKind::Base)
            })
        {
            return Err("world battle needs one base and an army on each side".into());
        }
        Ok(battle)
    }
}
