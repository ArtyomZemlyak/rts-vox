use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Integrity {
    pub core_volume_pct: i32,
    pub core_section_pct: i32,
    pub engine_volume_pct: i32,
    pub gear_section_pct: i32,
    pub required_gears_pct: i32,
    /// A leg that lost more than this share of its link cells buckles
    /// (ADR-281). Older data without the field keeps legs whole until they
    /// fall off.
    #[serde(default = "no_buckling")]
    pub leg_buckle_pct: i32,
    /// Share of a buckled leg's rest reach its foot keeps from the hip,
    /// per cent (ADR-288): the leg folds on its foot rather than lifting.
    #[serde(default = "a_third")]
    pub leg_fold_pct: i32,
    /// Clearance of a walker with fewer than two legs carrying — dead, or
    /// with every leg buckled or lost — per cent of the ride's (ADR-304):
    /// the hull comes down onto its folded legs.
    #[serde(default = "a_quarter")]
    pub collapse_clearance_pct: i32,
}
fn no_buckling() -> i32 {
    100
}
fn a_third() -> i32 {
    35
}
fn a_quarter() -> i32 {
    25
}
impl Integrity {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let rules: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if [
            rules.core_volume_pct,
            rules.core_section_pct,
            rules.engine_volume_pct,
            rules.gear_section_pct,
            rules.required_gears_pct,
            rules.leg_buckle_pct,
            rules.leg_fold_pct,
        ]
        .iter()
        .any(|v| !(1..=100).contains(v))
        {
            return Err("integrity percentages must be in 1..=100".into());
        }
        Ok(rules)
    }
}
