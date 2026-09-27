//! Small, bounded skirmishes on the existing range.
use crate::explosion::Delivery;
use crate::range::{RangeUnitDef, RangeZone};
use serde::Deserialize;

/// Which round a tank loads for what it aims at (ADR-456): `Blast` is the
/// high-explosive shell, `Kinetic` the armour-piercing one.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct TankAmmo {
    pub tank: Delivery,
    pub strider: Delivery,
    /// A point on the ground, and a tank with no target.
    pub ground: Delivery,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    pub tank_power_milli: u32,
    pub tank_reload_ticks: u32,
    pub tank_range: i32,
    pub pursuit_ticks: u32,
    pub standoff_milli: i32,
    /// World units a pursuer keeps its firing position from the points
    /// its allies hold; zero takes the position along the line to the
    /// target whoever holds it (ADR-467).
    pub firing_spacing: i32,
    pub max_rounds: usize,
    pub max_waves: usize,
    pub tank_ammo: TankAmmo,
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
            || !(0..=1000).contains(&t.firing_spacing)
            || !(2..=128).contains(&t.max_rounds)
            || !(1..=8).contains(&t.max_waves)
            || [t.tank_ammo.tank, t.tank_ammo.strider, t.tank_ammo.ground]
                .iter()
                .any(|d| !matches!(d, Delivery::Blast | Delivery::Kinetic))
            || t.units.len() > crate::range::MAX_RANGE_UNITS
            || !t.units.iter().any(|u| u.allied)
            || !t.units.iter().any(|u| !u.allied)
        {
            return Err("invalid battle tuning".into());
        }
        Ok(t)
    }
}
