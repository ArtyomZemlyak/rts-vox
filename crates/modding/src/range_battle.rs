//! Small, bounded skirmishes on the existing range.
use crate::explosion::Delivery;
use crate::range::{RangeUnitDef, RangeZone};
use serde::Deserialize;

/// Bounded salvo of ballistic explosive submunitions.
#[derive(Clone, Debug, Deserialize)]
pub struct Canister {
    pub pellets: usize,
    #[serde(default = "default_canister_loft")]
    pub flight_time_milli: i32,
    pub spread_milli: i32,
    #[serde(default = "default_canister_radius")]
    pub footprint_radius: i32,
    pub range: i32,
    pub reload_ticks: u32,
}
fn default_canister_radius() -> i32 {
    60
}
fn default_canister_loft() -> i32 {
    2000
}
impl Default for Canister {
    fn default() -> Self {
        Self {
            pellets: 4,
            flight_time_milli: 2000,
            spread_milli: 45,
            footprint_radius: 60,
            range: 1600,
            reload_ticks: 150,
        }
    }
}

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
    #[serde(default = "default_turret_t2_power")]
    pub turret_t2_power_milli: u32,
    #[serde(default)]
    pub turret_t2_reload_ticks: Option<u32>,
    pub tank_reload_ticks: u32,
    pub tank_range: i32,
    #[serde(default = "default_artillery_range")]
    pub artillery_range: i32,
    #[serde(default)]
    pub canister: Canister,
    pub kinetic_range: i32,
    pub energy_range: i32,
    pub anni_range: i32,
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
fn default_turret_t2_power() -> u32 {
    1000
}
fn default_artillery_range() -> i32 {
    1800
}
impl Tuning {
    pub fn from_ron(s: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(s).map_err(|e| e.to_string())?;
        if !(100..=4000).contains(&t.tank_power_milli)
            || !(100..=4000).contains(&t.turret_t2_power_milli)
            || t.turret_t2_reload_ticks
                .is_some_and(|n| !(1..=1800).contains(&n))
            || !(1..=1800).contains(&t.tank_reload_ticks)
            || !(100..=3000).contains(&t.tank_range)
            || !(100..=3000).contains(&t.artillery_range)
            || !(2..=24).contains(&t.canister.pellets)
            || t.canister.pellets > t.max_rounds
            || t.canister.pellets > t.max_waves
            || !(1500..=4000).contains(&t.canister.flight_time_milli)
            || !(5..=150).contains(&t.canister.spread_milli)
            || !(10..=150).contains(&t.canister.footprint_radius)
            || !(100..=3000).contains(&t.canister.range)
            || !(1..=1800).contains(&t.canister.reload_ticks)
            || !(100..=3000).contains(&t.kinetic_range)
            || !(100..=3000).contains(&t.energy_range)
            || !(100..=3000).contains(&t.anni_range)
            || !(1..=300).contains(&t.pursuit_ticks)
            || !(100..=950).contains(&t.standoff_milli)
            || !(0..=1000).contains(&t.firing_spacing)
            || !(2..=128).contains(&t.max_rounds)
            || !(1..=16).contains(&t.max_waves)
            || [t.tank_ammo.tank, t.tank_ammo.strider, t.tank_ammo.ground]
                .iter()
                .any(|d| !matches!(d, Delivery::Blast | Delivery::Kinetic))
            || t.units.len() > crate::range::MAX_RANGE_UNITS
            || t.units.iter().any(|u| !u.valid_loadout())
            || !t.units.iter().any(|u| u.allied)
            || !t.units.iter().any(|u| !u.allied)
        {
            return Err("invalid battle tuning".into());
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canister_defaults_and_budget_validation() {
        let shipped = include_str!("../../../data/range-battle.ron");
        let old = shipped
            .lines()
            .filter(|l| !l.contains("canister:"))
            .collect::<Vec<_>>()
            .join("\n");
        let t = Tuning::from_ron(&old).unwrap();
        assert_eq!(t.canister.pellets, 4);
        for invalid in [
            shipped.replace("pellets: 6", "pellets: 0"),
            shipped.replace("spread_milli: 35", "spread_milli: 500"),
            shipped.replace("reload_ticks: 150", "reload_ticks: 0"),
            shipped.replace("flight_time_milli: 2500", "flight_time_milli: 1499"),
            shipped.replace("flight_time_milli: 2500", "flight_time_milli: 4001"),
            shipped.replace("footprint_radius: 60", "footprint_radius: 0"),
            shipped.replace("footprint_radius: 60", "footprint_radius: 151"),
            shipped.replace("max_waves: 16", "max_waves: 5"),
            shipped.replace("max_waves: 16", "max_waves: 17"),
        ] {
            assert!(Tuning::from_ron(&invalid).is_err());
        }
    }

    #[test]
    fn artillery_range_has_a_legacy_default_and_rejects_invalid_values() {
        let shipped = include_str!("../../../data/range-battle.ron");
        let old = shipped.replace("artillery_range: 1800,", "");
        assert_eq!(Tuning::from_ron(&old).unwrap().artillery_range, 1800);
        let invalid = shipped.replace("artillery_range: 1800,", "artillery_range: 0,");
        assert!(Tuning::from_ron(&invalid).is_err());
    }
    #[test]
    fn turret_t2_power_has_legacy_default_and_finite_limits() {
        let shipped = include_str!("../../../data/range-battle.ron");
        let old = shipped
            .lines()
            .filter(|l| !l.contains("turret_t2_power_milli:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(Tuning::from_ron(&old).unwrap().turret_t2_power_milli, 1000);
        assert_eq!(
            Tuning::from_ron(shipped).unwrap().turret_t2_power_milli,
            2000
        );
        for n in [0, 99, 4001, u32::MAX] {
            let bad = shipped.replace(
                "turret_t2_power_milli: 2000",
                &format!("turret_t2_power_milli: {n}"),
            );
            assert!(Tuning::from_ron(&bad).is_err());
        }
    }
    #[test]
    fn turret_t2_reload_can_retain_legacy_tank_timing() {
        let shipped = include_str!("../../../data/range-battle.ron");
        let old = shipped
            .lines()
            .filter(|l| !l.contains("turret_t2_reload_ticks:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(Tuning::from_ron(&old).unwrap().turret_t2_reload_ticks, None);
        assert_eq!(
            Tuning::from_ron(shipped).unwrap().turret_t2_reload_ticks,
            Some(60)
        );
        for n in [0, 1801, u32::MAX] {
            let bad = shipped.replace(
                "turret_t2_reload_ticks: Some(60)",
                &format!("turret_t2_reload_ticks: Some({n})"),
            );
            assert!(Tuning::from_ron(&bad).is_err());
        }
    }
}
