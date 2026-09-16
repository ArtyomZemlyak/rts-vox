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
}
fn no_buckling() -> i32 {
    100
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
        ]
        .iter()
        .any(|v| !(1..=100).contains(v))
        {
            return Err("integrity percentages must be in 1..=100".into());
        }
        Ok(rules)
    }
}
