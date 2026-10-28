//! Data-only laboratory recipes and library assets, using the range's physics.
use crate::range::{RangeBlock, RangeLayout};
use serde::{Deserialize, Serialize};
#[path = "laboratory_catalog.rs"]
pub mod catalog;
#[path = "laboratory_experiments.rs"]
pub mod experiments;
#[path = "laboratory_sources.rs"]
pub mod sources;
#[path = "laboratory_events.rs"]
pub mod events;

#[derive(Clone, Debug, Deserialize)]
pub struct Material {
    pub id: u16,
    pub name: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Form {
    pub name: String,
    pub size: [i32; 3],
    pub lift: i32,
    pub supports: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Definition {
    pub layout: RangeLayout,
    pub materials: Vec<Material>,
    pub forms: Vec<Form>,
    pub thickness: Vec<i32>,
    pub strength_milli: Vec<u32>,
    pub support_material: u16,
    pub support_width: i32,
    pub observation_ticks: u32,
    pub measurement_ticks: u32,
    pub editor_step: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Recipe {
    pub material: usize,
    pub form: usize,
    pub thickness: usize,
    pub strength: usize,
    pub power: usize,
    /// Index into the laboratory's stock weapon presets.
    pub effect: u8,
}
impl Default for Recipe {
    fn default() -> Self {
        Self {
            material: 2,
            form: 0,
            thickness: 1,
            strength: 1,
            power: 1,
            effect: 0,
        }
    }
}
impl Definition {
    pub fn stock() -> Self {
        Self::from_ron(include_str!("../../../data/material-lab.ron")).expect("material lab RON")
    }
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let d: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        d.layout.validate()?;
        if d.layout.battle
            || !d.layout.units.is_empty()
            || !d.layout.blocks.is_empty()
            || !(1..=16).contains(&d.materials.len())
            || !(1..=8).contains(&d.forms.len())
            || !(1..=8).contains(&d.thickness.len())
            || !(1..=8).contains(&d.strength_milli.len())
            || d.materials.iter().enumerate().any(|(i, m)| {
                m.name.trim().is_empty()
                    || ![1, 2, 3, 4, 5, 6, 8, 10, 11, 12].contains(&m.id)
                    || d.materials[..i].iter().any(|p| p.id == m.id)
            })
            || d.forms.iter().any(|f| {
                f.name.trim().is_empty()
                    || f.size.iter().any(|n| !(1..=64).contains(n))
                    || !(0..=48).contains(&f.lift)
            })
            || d.thickness.iter().any(|n| !(1..=16).contains(n))
            || d.strength_milli.iter().any(|n| !(100..=4000).contains(n))
            || ![2, 3, 6].contains(&d.support_material)
            || !(1..=16).contains(&d.support_width)
            || !(1..=1800).contains(&d.observation_ticks)
            || !(1..=60).contains(&d.measurement_ticks)
        {
            return Err("invalid material laboratory limits".into());
        }
        if !(1..=16).contains(&d.editor_step) {
            return Err("invalid laboratory editor step".into());
        }
        // Validate every authored shape at the largest available thickness.
        for form in 0..d.forms.len() {
            for thickness in 0..d.thickness.len() {
                let r = Recipe {
                    material: 0,
                    form,
                    thickness,
                    strength: 0,
                    power: 0,
                    effect: 0,
                };
                d.fixture(r)?.0.validate()?;
            }
        }
        Ok(d)
    }
    pub fn fixture(&self, r: Recipe) -> Result<(RangeLayout, RangeBlock), String> {
        let material = self
            .materials
            .get(r.material)
            .ok_or("unknown laboratory material")?;
        let form = self.forms.get(r.form).ok_or("unknown laboratory form")?;
        let thickness = *self.thickness.get(r.thickness).ok_or("unknown thickness")?;
        if r.effect > 9
            || r.strength >= self.strength_milli.len()
            || r.power >= self.layout.power_milli.len()
        {
            return Err("unknown laboratory recipe".into());
        }
        let mut layout = self.layout.clone();
        let mut size = form.size;
        if r.form == 0 {
            size[0] = thickness;
        }
        let min = [
            layout.target[0] - size[0] / 2,
            layout.ground_level + form.lift,
            layout.target[2] - size[2] / 2,
        ];
        let max = std::array::from_fn(|a| min[a] + size[a]);
        let specimen = RangeBlock {
            min,
            max,
            material: material.id,
        };
        if form.supports {
            if form.lift == 0 {
                return Err("supported specimen needs clearance".into());
            }
            for z in [min[2], max[2] - self.support_width] {
                layout.blocks.push(RangeBlock {
                    min: [min[0], layout.ground_level, z],
                    max: [max[0], min[1], z + self.support_width],
                    material: self.support_material,
                });
            }
        }
        layout.blocks.push(specimen.clone());
        layout.target = [min[0], (min[1] + max[1]) / 2, (min[2] + max[2]) / 2];
        if r.effect == 5 {
            let support = layout
                .blocks
                .first()
                .filter(|_| form.supports)
                .ok_or("Для проверки опоры выберите балку")?;
            layout.target = [
                support.min[0],
                (support.min[1] + support.max[1]) / 2,
                (support.min[2] + support.max[2]) / 2,
            ];
        }
        layout.validate()?;
        Ok((layout, specimen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recipes_are_small_valid_isolated_and_do_not_mutate_the_stock_layout() {
        let d = Definition::stock();
        for material in 0..d.materials.len() {
            for form in 0..d.forms.len() {
                for thickness in 0..d.thickness.len() {
                    let r = Recipe {
                        material,
                        form,
                        thickness,
                        ..Recipe::default()
                    };
                    let (l, s) = d.fixture(r).unwrap();
                    l.validate().unwrap();
                    let volume: i32 = (0..3).map(|a| s.max[a] - s.min[a]).product();
                    assert!(volume <= 16384);
                    assert!(l.units.is_empty() && !l.battle);
                }
            }
        }
        assert!(d.layout.blocks.is_empty());
        assert!(d
            .fixture(Recipe {
                effect: 5,
                ..Recipe::default()
            })
            .is_err());
        assert!(d
            .fixture(Recipe {
                form: 2,
                effect: 5,
                ..Recipe::default()
            })
            .is_ok());
        assert!(d
            .fixture(Recipe {
                material: usize::MAX,
                ..Recipe::default()
            })
            .is_err());
        assert!(Definition::from_ron(
            &include_str!("../../../data/material-lab.ron")
                .replace("observation_ticks: 300", "observation_ticks: 0")
        )
        .is_err());
    }
}
