use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Navigation {
    pub cell_units: i32,
    pub search_nodes: usize,
    /// Search work in one tick: one frontier node or one swept pose.
    /// Target selection and connections share the same bounded allowance.
    pub search_nodes_per_tick: usize,
    /// Aggregate continuation work across the range, shared by pending hunts.
    pub search_resume_budget: usize,
    /// An isolated hunt may borrow idle capacity up to this ceiling.
    pub search_resume_per_unit: usize,
    /// Searches the whole range may begin in one tick. Hulls beyond the
    /// ceiling retain their orders; each admitted start also has a work cap.
    pub searches_started_per_tick: usize,
    pub recovery_ticks: u32,
    /// Delay before replanning once another hull blocks an accepted route.
    pub contact_replan_ticks: u32,
    pub escape_ticks: u32,
    pub escape_speed_pct: i32,
    pub recovery_turn_deg: i32,
    /// Extra contact length the planner probes with, world units: its
    /// refusals are wider than the executor's by this much.
    pub plan_margin: i32,
    /// Precision at a checked turn centre or intermediate route endpoint.
    /// Final order arrival keeps the course's ordinary arrival radius.
    pub corner_arrival_mm: i32,
    /// Weight of the planner's distance estimate, percent; 100 is plain A*.
    pub heuristic_pct: u32,
    /// How far a hull may get from where its search began, driving on or
    /// braking, before the search starts over, world units (ADR-462).
    pub retarget_units: i32,
    pub fell: Felling,
}
/// What a hull pushes over instead of driving around.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Felling {
    /// Materials a tank or walker crushes while crossing vegetation.
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
            || !(1..=51200).contains(&n.search_nodes_per_tick)
            || !(1..=51200).contains(&n.search_resume_budget)
            || !(1..=n.search_resume_budget).contains(&n.search_resume_per_unit)
            || !(2..=16).contains(&n.cell_units)
            || !(n.cell_units as u32).is_power_of_two()
            || !(64..=51200).contains(&n.search_nodes)
            || !(1..=300).contains(&n.recovery_ticks)
            || !(1..=300).contains(&n.contact_replan_ticks)
            || !(1..=300).contains(&n.escape_ticks)
            || !(1..=60).contains(&n.escape_speed_pct)
            || !(0..=60).contains(&n.recovery_turn_deg)
            || !(0..=16).contains(&n.plan_margin)
            || !(1..=500).contains(&n.corner_arrival_mm)
            || !(0..=256).contains(&n.retarget_units)
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

#[cfg(test)]
mod tests {
    use super::*;
    /// Every number is in the file (ADR-429): one left out is an error,
    /// not a default in code. A missing slice budget used to lift the
    /// per-tick ceiling on search work altogether.
    #[test]
    fn a_navigation_file_missing_a_number_is_refused() {
        for field in [
            "search_nodes_per_tick: 96,",
            "search_resume_budget: 768,",
            "search_resume_per_unit: 192,",
            "searches_started_per_tick: 1,",
            "contact_replan_ticks: 2,",
            "corner_arrival_mm: 50,",
            "heuristic_pct: 200,",
            "retarget_units: 160,",
        ] {
            let cut = include_str!("../../../data/range-navigation.ron").replace(field, "");
            assert!(
                Navigation::from_ron(&cut).is_err(),
                "accepted without {field}"
            );
        }
        assert!(Navigation::from_ron(
            &include_str!("../../../data/range-navigation.ron")
                .replace("contact_replan_ticks: 2", "contact_replan_ticks: 0")
        )
        .is_err());
    }
}
