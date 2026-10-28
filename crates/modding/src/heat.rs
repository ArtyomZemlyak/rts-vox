//! Heat and secondary combustion tuning for the range (ADR-277). The
//! balance lives in `data/heat.ron`; the model is `sim_core::heat`.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Transformation {
    pub from: u16,
    pub to: u16,
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct MeltFlow {
    pub solid: u16,
    pub liquid: u16,
    pub freeze: u32,
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct HeatTuning {
    /// Share of a cell's heat lost per tick, thousandths; never less than
    /// one unit, so every hot cell goes cold eventually.
    pub cooling_milli: u32,
    /// Ceiling on hot cells visited per tick; the rest wait their turn.
    pub cells_per_tick: usize,
    /// Ticks a lit voxel burns before it is consumed.
    pub burn_ticks: u16,
    #[serde(default)]
    pub transforms: [Option<Transformation>; 4],
    #[serde(default)]
    pub melt_flow: [Option<MeltFlow>; 3],
    #[serde(default)]
    pub melt_moves_per_tick: usize,
}
impl HeatTuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=1000).contains(&t.cooling_milli)
            || !(64..=65536).contains(&t.cells_per_tick)
            || !(1..=3600).contains(&t.burn_ticks)
            || t.transforms
                .iter()
                .flatten()
                .any(|p| p.from == 0 || p.to == 0 || p.from == p.to)
            || t.melt_moves_per_tick > 512
            || t.melt_flow
                .iter()
                .flatten()
                .any(|p| p.solid == 0 || p.liquid == 0 || p.solid == p.liquid || p.freeze == 0)
        {
            return Err("invalid heat limits".into());
        }
        Ok(t)
    }
}
