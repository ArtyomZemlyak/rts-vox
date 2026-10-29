//! Finite-volume water on the existing world grid; no particle count scales with drops.
use serde::Deserialize;
pub const MATERIAL: u16 = 11;

#[derive(Clone, Debug, Deserialize)]
pub struct Porous {
    pub material: u16,
    pub capacity: u32,
    pub rate: u32,
    pub repose_bonus: i32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    #[serde(default)]
    pub motion: Motion,
    pub units_per_voxel: u32,
    pub compression: u32,
    pub surface_tension: u32,
    pub viscosity_milli: u32,
    pub max_flow: u32,
    pub max_cell_units: u32,
    pub max_cells: usize,
    pub checks_per_tick: usize,
    pub transfers_per_tick: usize,
    pub displacement_reach: u32,
    pub obstacle_checks: usize,
    pub pressure_checks: usize,
    pub pressure_fill_milli: u32,
    pub pressure_packets: usize,
    pub pressure_interval: u32,
    pub body_samples: usize,
    pub body_depth: u32,
    pub buoyancy_cells: usize,
    pub drag_milli: u32,
    pub porous: Vec<Porous>,
    pub moisture_checks: usize,
    pub drying_units: u32,
    #[serde(default = "default_capillary_units")]
    pub capillary_units: u32,
    pub evaporation_heat: u32,
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Motion {
    pub checks_per_tick: u32,
    pub column_cells: u32,
    pub max_steps: u32,
}
impl Default for Motion {
    fn default()->Self {Self {checks_per_tick:32768,column_cells:256,max_steps:32}}
}
impl Tuning {
    pub fn stock() -> Self {
        Self::from_ron(include_str!("../../../data/range-water.ron")).expect("water tuning")
    }
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(16..=4096).contains(&t.units_per_voxel)
            || !(1024..=131072).contains(&t.motion.checks_per_tick)
            || !(1..=512).contains(&t.motion.column_cells)
            || !(1..=64).contains(&t.motion.max_steps)
            || !(1..=t.units_per_voxel / 8).contains(&t.compression)
            || !(1..=t.units_per_voxel / 4).contains(&t.surface_tension)
            || t.viscosity_milli >= 1000
            || t.drag_milli > 1000
            || !(1..=t.units_per_voxel * 4).contains(&t.max_flow)
            || !(t.units_per_voxel * 2..=1_000_000).contains(&t.max_cell_units)
            || !(64..=524288).contains(&t.max_cells)
            || !(1..=65536).contains(&t.checks_per_tick)
            || !(1..=8192).contains(&t.transfers_per_tick)
            || !(1..=16).contains(&t.displacement_reach)
            || !(1..=8192).contains(&t.obstacle_checks)
            || !(1..=65536).contains(&t.pressure_checks)
            || !(500..=1000).contains(&t.pressure_fill_milli)
            || !(1..=512).contains(&t.pressure_packets)
            || !(1..=60).contains(&t.pressure_interval)
            || !(1..=32).contains(&t.body_samples)
            || !(1..=64).contains(&t.body_depth)
            || !(1..=65536).contains(&t.buoyancy_cells)
            || !(1..=8192).contains(&t.moisture_checks)
            || t.drying_units > t.units_per_voxel
            || t.capillary_units > t.units_per_voxel
            || !(1..=10000).contains(&t.evaporation_heat)
            || t.porous.len() > 16
            || t.porous.iter().enumerate().any(|(i, p)| {
                p.material == 0
                    || p.capacity == 0
                    || p.capacity > t.units_per_voxel
                    || p.rate == 0
                    || p.rate > p.capacity
                    || !(-4..=4).contains(&p.repose_bonus)
                    || t.porous[..i].iter().any(|v| v.material == p.material)
            })
        {
            return Err("invalid water limits".into());
        }
        Ok(t)
    }
}

fn default_capillary_units() -> u32 {
    32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capillary_limits_are_optional_bounded_and_can_be_disabled() {
        let stock = include_str!("../../../data/range-water.ron");
        assert_eq!(Tuning::from_ron(stock).unwrap().capillary_units, 32);
        let old = stock.replace("capillary_units:32,", "");
        assert_eq!(Tuning::from_ron(&old).unwrap().capillary_units, 32);
        assert_eq!(
            Tuning::from_ron(&stock.replace("capillary_units:32", "capillary_units:0"))
                .unwrap()
                .capillary_units,
            0
        );
        assert!(
            Tuning::from_ron(&stock.replace("capillary_units:32", "capillary_units:4097")).is_err()
        );
    }
    #[test]
    fn water_motion_work_limits_are_bounded_and_old_definitions_get_defaults() {
        let stock=include_str!("../../../data/range-water.ron");
        for (a,b) in [("checks_per_tick:32768","checks_per_tick:0"),("column_cells:256","column_cells:513"),("max_steps:32","max_steps:0")] {
            assert!(Tuning::from_ron(&stock.replace(a,b)).is_err());
        }
        let old=stock.lines().filter(|line|!line.contains("motion:")).collect::<Vec<_>>().join("\n");
        assert_eq!(Tuning::from_ron(&old).unwrap().motion.max_steps,32);
    }
}
