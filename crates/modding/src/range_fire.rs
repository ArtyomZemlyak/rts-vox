//! Finite combustible contents of range engine compartments.
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Reservoir {
    pub at: [u8; 3],
    pub fuel_ml: u32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    pub material: u16,
    pub tank: Reservoir,
    pub walker: Reservoir,
    pub spider: Reservoir,
    pub burn_ml_per_tick: u32,
    pub heat_per_ml: u32,
    pub ignition: u32,
    pub ignition_radius: u8,
    pub reach: u8,
    pub fade_ticks: u32,
    pub gravity: i32,
    pub max_speed: i32,
}
impl Tuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=1000).contains(&t.burn_ml_per_tick)
            || !(1..=100).contains(&t.heat_per_ml)
            || !(1..=100000).contains(&t.ignition)
            || t.ignition_radius > 3
            || !(1..=16).contains(&t.reach)
            || !(1..=900).contains(&t.fade_ticks)
            || !(1..=300).contains(&t.gravity)
            || !(1..=240).contains(&t.max_speed)
            || [&t.tank, &t.walker, &t.spider].iter().any(|r| {
                !(1..=1000000).contains(&r.fuel_ml) || !r.fuel_ml.is_multiple_of(t.burn_ml_per_tick)
            })
        {
            return Err("invalid range fire limits".into());
        }
        Ok(t)
    }
    pub fn reservoir(&self, kind: crate::range::RangeUnitKind) -> &Reservoir {
        match kind {
            crate::range::RangeUnitKind::Tank
            | crate::range::RangeUnitKind::LightningTank
            | crate::range::RangeUnitKind::EidolonTank
            | crate::range::RangeUnitKind::Crawler
            | crate::range::RangeUnitKind::Artillery
            | crate::range::RangeUnitKind::ArtilleryT2 => &self.tank,
            crate::range::RangeUnitKind::Strider | crate::range::RangeUnitKind::Walker04 | crate::range::RangeUnitKind::SkobaDrone => &self.walker,
            crate::range::RangeUnitKind::Spider => &self.spider,
            crate::range::RangeUnitKind::TurretT1
            | crate::range::RangeUnitKind::ArtilleryTurretT2
            | crate::range::RangeUnitKind::ArtilleryTurretT3
            | crate::range::RangeUnitKind::RazlomT3
            | crate::range::RangeUnitKind::GrebenStorage
            | crate::range::RangeUnitKind::KilFactoryT1
            | crate::range::RangeUnitKind::KilFactoryT2
            | crate::range::RangeUnitKind::KilFactoryT3
            | crate::range::RangeUnitKind::KilFactoryT4
            | crate::range::RangeUnitKind::NuclearSilo
            | crate::range::RangeUnitKind::TurretT2
            | crate::range::RangeUnitKind::Base => &self.tank,
        }
    }
    /// Whole economic quantities: no fractional matter disappears on burning.
    pub fn validate_density(&self, density: u32) -> Result<(), String> {
        if density == 0
            || !(u64::from(self.burn_ml_per_tick) * u64::from(density)).is_multiple_of(1000)
        {
            return Err("fuel burn rate must represent whole matter quantities".into());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_fuel_has_bounded_exact_consumption() {
        let text = include_str!("../../../data/range-fire.ron");
        let t = Tuning::from_ron(text).unwrap();
        t.validate_density(80).unwrap();
        assert!(t.validate_density(1).is_err());
        for (a, b) in [
            ("reach: 8", "reach: 255"),
            ("ignition_radius: 2", "ignition_radius: 4"),
            ("burn_ml_per_tick: 50", "burn_ml_per_tick: 0"),
            ("fuel_ml: 80000", "fuel_ml: 80001"),
        ] {
            assert!(Tuning::from_ron(&text.replace(a, b)).is_err());
        }
    }
}
