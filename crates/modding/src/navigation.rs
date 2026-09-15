use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Navigation {
    pub cell_units: i32,
    pub search_nodes: usize,
    pub recovery_ticks: u32,
    pub reverse_ticks: u32,
    pub reverse_speed_pct: i32,
    pub recovery_turn_deg: i32,
    /// Extra contact length the planner probes with, world units: its
    /// refusals are wider than the executor's by this much.
    pub plan_margin: i32,
    pub fell: Felling,
}
/// What a tracked hull pushes over instead of driving around.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Felling {
    /// Materials a tracked hull crushes; a walker still treats them as walls.
    pub materials: [u16; 2],
    /// Connected yielding matter within this distance of a cut cell is
    /// released and falls; farther crowns stay standing.
    pub radius: i32,
    /// Ceiling on released cells per tick; the rest waits, nothing is lost.
    pub cells_per_tick: u32,
    /// Planner price of one crushed footprint column (a straight move is 10).
    pub cost_per_column: u32,
    /// Ceiling of that price per planner node.
    pub cost_cap: u32,
    /// Speed while the hull is inside yielding matter.
    pub speed_pct: i32,
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
            || !(0..=16).contains(&n.plan_margin)
            || n.fell.materials.contains(&0)
            || !(1..=32).contains(&n.fell.radius)
            || !(1..=8192).contains(&n.fell.cells_per_tick)
            || n.fell.cost_per_column > 1000
            || n.fell.cost_cap > 1000
            || !(1..=100).contains(&n.fell.speed_pct)
        {
            return Err("invalid range navigation limits".into());
        }
        Ok(n)
    }
}
