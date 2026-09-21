use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Navigation {
    pub cell_units: i32,
    pub search_nodes: usize,
    /// Search work in one tick: one frontier node or one swept pose.
    /// Target selection and connections share the same bounded allowance.
    #[serde(default = "whole_search")]
    pub search_nodes_per_tick: usize,
    /// Searches the whole range may begin in one tick. Hulls beyond the
    /// ceiling retain their orders; each admitted start also has a work cap.
    #[serde(default = "every_start")]
    pub searches_started_per_tick: usize,
    pub recovery_ticks: u32,
    pub escape_ticks: u32,
    pub escape_speed_pct: i32,
    pub recovery_turn_deg: i32,
    /// Extra contact length the planner probes with, world units: its
    /// refusals are wider than the executor's by this much.
    pub plan_margin: i32,
    /// Precision at a checked turn centre or intermediate route endpoint.
    /// Final order arrival keeps the course's ordinary arrival radius.
    #[serde(default = "corner_arrival")]
    pub corner_arrival_mm: i32,
    /// Weight of the planner's distance estimate, percent; 100 is plain A*.
    #[serde(default = "plain_heuristic")]
    pub heuristic_pct: u32,
    pub fell: Felling,
}
fn corner_arrival() -> i32 {
    50
}
fn plain_heuristic() -> u32 {
    100
}
/// Older data without a slice budget searches whole, as it used to.
fn whole_search() -> usize {
    usize::MAX
}
/// Older data begins every search the tick asks for, as it used to.
fn every_start() -> usize {
    usize::MAX
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
        if n.searches_started_per_tick == 0
            || !(2..=16).contains(&n.cell_units)
            || !(n.cell_units as u32).is_power_of_two()
            || !(64..=51200).contains(&n.search_nodes)
            || !(1..=300).contains(&n.recovery_ticks)
            || !(1..=300).contains(&n.escape_ticks)
            || !(1..=60).contains(&n.escape_speed_pct)
            || !(0..=60).contains(&n.recovery_turn_deg)
            || !(0..=16).contains(&n.plan_margin)
            || !(1..=500).contains(&n.corner_arrival_mm)
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
