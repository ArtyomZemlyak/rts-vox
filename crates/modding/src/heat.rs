//! Heat and secondary combustion tuning for the range (ADR-276). The
//! balance lives in `data/heat.ron`; the model is `sim_core::heat`.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct HeatTuning {
    /// Share of a cell's heat lost per tick, thousandths; never less than
    /// one unit, so every hot cell goes cold eventually.
    pub cooling_milli: u32,
    /// Ceiling on hot cells visited per tick; the rest wait their turn.
    pub cells_per_tick: usize,
    /// Ticks a lit voxel burns before it is consumed.
    pub burn_ticks: u16,
}
impl HeatTuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=1000).contains(&t.cooling_milli)
            || !(64..=65536).contains(&t.cells_per_tick)
            || !(1..=3600).contains(&t.burn_ticks)
        {
            return Err("invalid heat limits".into());
        }
        Ok(t)
    }
}
