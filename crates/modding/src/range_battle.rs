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
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
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
            range: crate::attack_ranges::stock().canister,
            reload_ticks: crate::clock::stream_ticks(150),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Siege {
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub range: i32,
    pub reload_ticks: u32,
    pub flight_time_milli: i32,
}
impl Default for Siege {
    fn default() -> Self {
        ron::from_str(include_str!("../../../data/range-siege.ron")).expect("siege tuning")
    }
}

/// Strategic payload keeps its finite energy in millijoules at execution.
#[derive(Clone, Debug, Deserialize)]
pub struct Nuclear {
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub range: i32,
    pub reload_ticks: u32,
    pub flight_time_milli: i32,
    pub material_energy_j: u64,
    pub body_energy_pct: u32,
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
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub tank_range: i32,
    #[serde(default = "default_artillery_range")]
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub artillery_range: i32,
    #[serde(default)]
    pub canister: Canister,
    #[serde(default)]
    pub siege: Siege,
    #[serde(default = "default_nuclear")]
    pub nuclear: Nuclear,
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub kinetic_range: i32,
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub energy_range: i32,
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub anni_range: i32,
    pub pursuit_ticks: u32,
    pub standoff_milli: i32,
    /// World units a pursuer keeps its firing position from the points
    /// its allies hold; zero takes the position along the line to the
    /// target whoever holds it (ADR-467).
    pub firing_spacing: i32,
    pub max_rounds: usize,
    pub max_waves: usize,
    /// Ceilings over all live waves together, per tick (ADR-681): grid
    /// cells captured, frontier heap pops, and contacts (a cell visited,
    /// and each voxel of matter in it). Each wave takes its preset's own
    /// budget in turn while a ceiling lasts; a wave past it waits for the
    /// next tick.
    #[serde(default = "default_wave_capture_ceiling")]
    pub wave_capture_ceiling: usize,
    #[serde(default = "default_wave_work_ceiling")]
    pub wave_work_ceiling: usize,
    #[serde(default = "default_wave_work_ceiling")]
    pub wave_contact_ceiling: usize,
    /// Slots of the flying-piece pool a wave's ground cells leave to the
    /// hulls it strikes (ADR-681): ground beyond the rest is pulverised.
    #[serde(default)]
    pub hull_piece_reserve: u32,
    /// A wave's ground cell with no slot left waits for one, as before
    /// ADR-681, instead of being pulverised at once.
    #[serde(default)]
    pub ground_waits_for_pool: bool,
    /// Untouched terrain chunks the tick's edit drain is about to change,
    /// built ahead on the physics pool (ADR-681); zero builds none and
    /// leaves them to the drain's own ceiling on materialisation.
    #[serde(default)]
    pub drain_chunk_ceiling: usize,
    pub tank_ammo: TankAmmo,
    pub zone: RangeZone,
    pub units: Vec<RangeUnitDef>,
}
fn default_nuclear() -> Nuclear {
    ron::from_str(include_str!("../../../data/range-nuclear.ron")).expect("nuclear tuning")
}
fn default_turret_t2_power() -> u32 {
    1000
}
fn default_wave_capture_ceiling() -> usize {
    40000
}
fn default_wave_work_ceiling() -> usize {
    4096
}
fn default_artillery_range() -> i32 {
    crate::attack_ranges::stock().artillery
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
            // Each side owns half of the wave ceiling (ADR-649).
            || t.canister.pellets > t.max_waves / 2
            || !(1500..=4000).contains(&t.canister.flight_time_milli)
            || !(5..=150).contains(&t.canister.spread_milli)
            || !(10..=150).contains(&t.canister.footprint_radius)
            || !(100..=3000).contains(&t.canister.range)
            || !(1..=1800).contains(&t.canister.reload_ticks)
            || !(100..=crate::attack_ranges::MAX_DISTANCE).contains(&t.siege.range)
            || !(1..=1800).contains(&t.siege.reload_ticks)
            || !(1000..=4000).contains(&t.siege.flight_time_milli)
            || !(100..=crate::attack_ranges::MAX_DISTANCE).contains(&t.nuclear.range)
            || !(1..=1800).contains(&t.nuclear.reload_ticks)
            || !(1000..=4000).contains(&t.nuclear.flight_time_milli)
            || !(1..=4_000_000_000_000).contains(&t.nuclear.material_energy_j)
            || t.nuclear.body_energy_pct > 100
            || !(100..=3000).contains(&t.kinetic_range)
            || !(100..=3000).contains(&t.energy_range)
            || !(100..=3000).contains(&t.anni_range)
            || !(1..=300).contains(&t.pursuit_ticks)
            || !(100..=950).contains(&t.standoff_milli)
            || !(0..=1000).contains(&t.firing_spacing)
            || !(2..=128).contains(&t.max_rounds)
            || !(1..=16).contains(&t.max_waves)
            || !(1..=1 << 24).contains(&t.wave_capture_ceiling)
            || !(1..=1 << 24).contains(&t.wave_work_ceiling)
            || !(1..=1 << 24).contains(&t.wave_contact_ceiling)
            || t.drain_chunk_ceiling > 4096
            || t.hull_piece_reserve > 1 << 20
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
    fn nuclear_payload_is_finite_and_rejects_invalid_budgets() {
        let source = include_str!("../../../data/range-battle.ron");
        let stock = Tuning::from_ron(source).unwrap();
        assert_eq!(stock.nuclear.material_energy_j, 20_000_000_000 * 100);
        assert_eq!(stock.nuclear.range, crate::attack_ranges::stock().nuclear);
        let nuke = include_str!("../../../data/range-nuclear.ron");
        let explicit = nuke
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        Tuning::from_ron(&source.replacen(
            "tank_power_milli:",
            &format!("nuclear: {explicit}, tank_power_milli:"),
            1,
        ))
        .unwrap();
        for bad in [
            nuke.replace("2000000000000", "0"),
            nuke.replace("2000000000000", "4000000000001"),
            nuke.replace("body_energy_pct: 25", "body_energy_pct: 101"),
            nuke.replace("reload_ticks: 1200", "reload_ticks: 1801"),
        ] {
            let plain = bad
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            let modified = source.replacen(
                "tank_power_milli:",
                &format!("nuclear: {plain}, tank_power_milli:"),
                1,
            );
            assert!(Tuning::from_ron(&modified).is_err());
        }
    }

    #[test]
    fn siege_defaults_and_authored_limits_are_validated() {
        let text = include_str!("../../../data/range-battle.ron");
        let t = Tuning::from_ron(text).unwrap();
        assert_eq!(t.siege.range, crate::attack_ranges::stock().siege);
        assert_eq!(t.siege.reload_ticks, crate::clock::HZ * 7);
        assert_eq!(t.siege.flight_time_milli, 1800);
        for fields in [
            "range: 0, reload_ticks: 210, flight_time_milli: 1800",
            "range: 2400, reload_ticks: 0, flight_time_milli: 1800",
            "range: 2400, reload_ticks: 210, flight_time_milli: 4001",
        ] {
            let bad = text.replacen('(', &format!("(siege: ({fields}),"), 1);
            assert!(Tuning::from_ron(&bad).is_err());
        }
    }

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
            shipped.replace("reload_ticks: 100", "reload_ticks: 0"),
            shipped.replace("flight_time_milli: 2500", "flight_time_milli: 1499"),
            shipped.replace("flight_time_milli: 2500", "flight_time_milli: 4001"),
            shipped.replace("footprint_radius: 60", "footprint_radius: 0"),
            shipped.replace("footprint_radius: 60", "footprint_radius: 151"),
            shipped.replace("max_waves: 16", "max_waves: 5"),
            // Half of 11 is 5: a side could never fire a six-pellet salvo.
            shipped.replace("max_waves: 16", "max_waves: 11"),
            shipped.replace("max_waves: 16", "max_waves: 17"),
        ] {
            assert!(Tuning::from_ron(&invalid).is_err());
        }
    }

    #[test]
    fn artillery_range_has_a_legacy_default_and_rejects_invalid_values() {
        let shipped = include_str!("../../../data/range-battle.ron");
        let old = shipped.replace("artillery_range: \"artillery\",", "");
        assert_eq!(Tuning::from_ron(&old).unwrap().artillery_range, 1800);
        let invalid = shipped.replace("artillery_range: \"artillery\",", "artillery_range: 0,");
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
            Some(crate::clock::HZ * 2)
        );
        for n in [0, 1801, u32::MAX] {
            let bad = shipped.replace(
                "turret_t2_reload_ticks: Some(40)",
                &format!("turret_t2_reload_ticks: Some({n})"),
            );
            assert!(Tuning::from_ron(&bad).is_err());
        }
    }
}
