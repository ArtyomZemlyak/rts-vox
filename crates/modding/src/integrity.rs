use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Integrity {
    pub core_volume_pct: i32,
    pub core_section_pct: i32,
    /// Share of core volume in a single connected cavity spanning opposite
    /// faces of the core that destroys the unit even if peripheral mass remains.
    #[serde(default = "no_bore_limit")]
    pub core_bore_pct: i32,
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
fn no_bore_limit() -> i32 {
    100
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
            rules.core_bore_pct,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_stock_integrity_rules_parse_and_validate_bounds() {
        let legacy = "(core_volume_pct: 60, core_section_pct: 30, engine_volume_pct: 20, gear_section_pct: 30, required_gears_pct: 75)";
        let parsed = Integrity::from_ron(legacy).expect("legacy RON must parse");
        assert_eq!(parsed.core_bore_pct, 100);

        let stock = Integrity::from_ron(include_str!("../../../data/unit-integrity.ron"))
            .expect("stock unit-integrity.ron must parse");
        assert_eq!(stock.core_bore_pct, 15);

        let bad = "(core_volume_pct: 60, core_section_pct: 30, core_bore_pct: 0, engine_volume_pct: 20, gear_section_pct: 30, required_gears_pct: 75)";
        assert!(Integrity::from_ron(bad).is_err());
    }
}
