//! Small surface-clutter resistance, independent of walls and drive orders.
use serde::Deserialize;
#[derive(Clone,Copy,Debug,Deserialize)]
pub struct Tuning {
    pub max_loss_pct:i32,
    pub reference_mass_kg:u64,
    pub blend_ticks:i32,
    pub track_detail_cm:i32,
}
impl Tuning {
    pub fn from_ron(text:&str)->Result<Self,String> {
        let t:Self=ron::from_str(text).map_err(|e|e.to_string())?;
        if !(0..=10).contains(&t.max_loss_pct) || !(1..=1_000_000_000).contains(&t.reference_mass_kg)
            || !(1..=200).contains(&t.blend_ticks) || !(0..=50).contains(&t.track_detail_cm) {
            return Err("invalid terrain resistance".into());
        }
        Ok(t)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surface_limits_are_explicit_and_bounded() {
        let text=include_str!("../../../data/terrain-motion.ron");
        assert!(Tuning::from_ron(text).is_ok());
        for field in ["max_loss_pct: 10,","reference_mass_kg: 60000,","blend_ticks: 20,","track_detail_cm: 50,"] {
            assert!(Tuning::from_ron(&text.replace(field,"")).is_err());
        }
        for (a,b) in [("max_loss_pct: 10","max_loss_pct: 11"),("reference_mass_kg: 60000","reference_mass_kg: 0"),
            ("blend_ticks: 20","blend_ticks: 0"),("track_detail_cm: 50","track_detail_cm: 51")] {
            assert!(Tuning::from_ron(&text.replace(a,b)).is_err());
        }
    }
}
