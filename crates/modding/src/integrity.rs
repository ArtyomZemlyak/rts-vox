use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Integrity {
    pub core_volume_pct: i32,
    pub core_section_pct: i32,
    pub engine_volume_pct: i32,
    pub gear_section_pct: i32,
    pub required_gears_pct: i32,
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
        ]
        .iter()
        .any(|v| !(1..=100).contains(v))
        {
            return Err("integrity percentages must be in 1..=100".into());
        }
        Ok(rules)
    }
}
