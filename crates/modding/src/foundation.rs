//! Stock emplacement footings. Separate from scenario placement data.
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Foundation {
    pub material: u16,
    /// Buried depth and horizontal margin, in world units.
    pub depth: i32,
    pub margin: i32,
    /// Intact load columns required out of a 3x3 footing grid.
    pub supports: usize,
}
impl Foundation {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let f: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if f.material == 0
            || !(1..=64).contains(&f.depth)
            || !(0..=32).contains(&f.margin)
            || !(1..=9).contains(&f.supports)
        {
            return Err("invalid emplacement foundation".into());
        }
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_footing_and_invalid_limits() {
        let text = include_str!("../../../data/range-foundation.ron");
        assert!(Foundation::from_ron(text).is_ok());
        for (old, new) in [
            ("material: 2", "material: 0"),
            ("depth: 12", "depth: 65"),
            ("margin: 4", "margin: -1"),
            ("supports: 5", "supports: 10"),
        ] {
            assert!(Foundation::from_ron(&text.replace(old, new)).is_err());
        }
    }
}
