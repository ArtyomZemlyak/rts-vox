use serde::Deserialize;
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Tuning {
    pub minimum_burial: i32,
    pub radius_per_depth: i32,
    pub radius: i32,
    pub side: i32,
    pub layers: u32,
    pub checks: usize,
    pub pieces: usize,
    pub jobs: usize,
    pub energy_milli: u32,
    pub speed: i32,
    pub edge_speed: i32,
    pub fracture_j_per_strength: u32,
}
impl Tuning {
    pub fn stock() -> Self {
        Self::from_ron(include_str!("../../../data/range-upheaval.ron")).expect("upheaval")
    }
    pub fn from_ron(s: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(s).map_err(|e| e.to_string())?;
        if !(2..=64).contains(&t.minimum_burial)
            || !(1..=8).contains(&t.radius_per_depth)
            || !(4..=128).contains(&t.radius)
            || !(2..=16).contains(&t.side)
            || t.side % 2 != 0
            || !(1..=16).contains(&t.layers)
            || !(256..=65536).contains(&t.checks)
            || t.checks < (t.side * t.side) as usize * t.layers as usize
            || !(1..=64).contains(&t.pieces)
            || !(1..=16).contains(&t.jobs)
            || !(1..=500).contains(&t.energy_milli)
            || !(1..=400).contains(&t.speed)
            || !(1..=t.speed).contains(&t.edge_speed)
            || !(1..=1000).contains(&t.fracture_j_per_strength)
        {
            return Err("invalid underground uplift limits".into());
        }
        Ok(t)
    }
}
