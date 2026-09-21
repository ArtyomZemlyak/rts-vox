//! Sparse unit heat tuning. Values use the range heat units, not kelvin.
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    pub cells_per_tick: usize,
    pub max_cells: usize,
    pub cooling_milli: u32,
    pub conduction_milli: u32,
    /// Failure heat per WORLD_MM cube, in Part order.
    pub softening: [u32; 5],
    pub smoke: u32,
    pub glow: u32,
    pub exchange_cells_per_tick: usize,
    /// Reach in 100 mm world cells; six axial probes per hot source.
    pub exchange_reach: u8,
    pub exchange_milli: u32,
}
impl Tuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(32..=4096).contains(&t.cells_per_tick)
            || !(t.cells_per_tick..=32768).contains(&t.max_cells)
            || t.cooling_milli > 1000
            || t.conduction_milli > 150
            || t.softening.iter().any(|n| !(1..=100000).contains(n))
            || !(1..=100000).contains(&t.smoke)
            || !(1..=100000).contains(&t.glow)
            || !(1..=512).contains(&t.exchange_cells_per_tick)
            || !(1..=8).contains(&t.exchange_reach)
            || t.exchange_milli > 100
        {
            return Err("invalid unit heat limits".into());
        }
        Ok(t)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thermal_data_bounds_diffusion_and_work() {
        let text = include_str!("../../../data/unit-heat.ron");
        let t = Tuning::from_ron(text).unwrap();
        assert!(t.softening[0] > t.softening[2]);
        assert!(
            Tuning::from_ron(&text.replace("conduction_milli: 8", "conduction_milli: 167"))
                .is_err()
        );
        assert!(Tuning::from_ron(&text.replace("max_cells: 16384", "max_cells: 1")).is_err());
        for (from, to) in [
            ("exchange_reach: 4", "exchange_reach: 9"),
            ("exchange_cells_per_tick: 64", "exchange_cells_per_tick: 0"),
            ("exchange_milli: 40", "exchange_milli: 101"),
        ] {
            assert!(Tuning::from_ron(&text.replace(from, to)).is_err());
        }
    }
}
