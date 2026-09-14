use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Navigation {
    pub cell_units: i32,
    pub search_nodes: usize,
    pub recovery_ticks: u32,
    pub reverse_ticks: u32,
    pub reverse_speed_pct: i32,
    pub recovery_turn_deg: i32,
}
impl Navigation {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let n: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(2..=16).contains(&n.cell_units)
            || 640 % n.cell_units != 0
            || !(64..=51200).contains(&n.search_nodes)
            || !(1..=300).contains(&n.recovery_ticks)
            || !(1..=300).contains(&n.reverse_ticks)
            || !(1..=60).contains(&n.reverse_speed_pct)
            || !(0..=60).contains(&n.recovery_turn_deg)
        {
            return Err("invalid range navigation limits".into());
        }
        Ok(n)
    }
}
