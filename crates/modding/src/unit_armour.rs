//! Immutable mechanical properties of the range's steel/composite grades.
use crate::range::RangeUnitKind;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Grade {
    pub name: String,
    /// Local ablation work by role: armour, core, engine, weapon, sensor.
    pub ablation_milli: [u32; 5],
    /// Fracture energy per physical volume, in the same role order.
    pub fracture_milli: [u32; 5],
}
#[derive(Clone, Debug, Deserialize)]
pub struct Armour {
    pub grades: Vec<Grade>,
    /// Explicit kind assignment; never infer a tier from an enum ordinal.
    pub units: Vec<(RangeUnitKind, usize)>,
}
impl Armour {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let c: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if c.grades.is_empty()
            || c.grades.len() > RangeUnitKind::COUNT
            || c.grades.iter().any(|g| {
                g.name.trim().is_empty()
                    || g.ablation_milli.iter().any(|v| !(1..=20000).contains(v))
                    || g.fracture_milli.iter().any(|v| !(1..=100000).contains(v))
            })
            || c.units.len() != RangeUnitKind::COUNT
            || RangeUnitKind::ALL
                .iter()
                .any(|kind| c.units.iter().filter(|(k, _)| k == kind).count() != 1)
            || c.units.iter().any(|(_, grade)| *grade >= c.grades.len())
        {
            return Err("invalid unit armour grades or kind assignments".into());
        }
        Ok(c)
    }
    pub fn grade(&self, kind: RangeUnitKind) -> &Grade {
        &self.grades[self
            .units
            .iter()
            .find(|(k, _)| *k == kind)
            .expect("validated armour kind")
            .1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_grades_are_complete_and_reject_ambiguous_or_unbounded_data() {
        let text = include_str!("../../../data/unit-armour.ron");
        let c = Armour::from_ron(text).unwrap();
        assert_eq!(c.grade(RangeUnitKind::Tank).ablation_milli, [1000; 5]);
        assert!(
            c.grade(RangeUnitKind::Spider).fracture_milli[0]
                > c.grade(RangeUnitKind::Strider).fracture_milli[0]
        );
        for bad in [
            text.replace("(Spider, 3)", "(Tank, 3)"),
            text.replace("(Spider, 3)", "(Spider, 99)"),
            text.replace("8000", "100001"),
            text.replace("18000", "20001"),
        ] {
            assert!(Armour::from_ron(&bad).is_err());
        }
    }
}
