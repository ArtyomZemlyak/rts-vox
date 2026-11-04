//! Authored emitters are independent of visual catalogue elements.
use crate::{explosion::ExplosionConfig, range::RangeLayout};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Source {
    pub name: String,
    pub at: [i32; 3],
    pub target: [i32; 3],
    /// Index of a placed library element, resolved to its first physical unit.
    #[serde(default)]
    pub binding: Option<usize>,
    pub config: ExplosionConfig,
    pub power_milli: u32,
    pub visual: u8,
    #[serde(default)]
    pub effect: Option<u8>,
    /// Local charge rather than a projectile emitted towards `target`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub detonate: bool,
    /// The scenario command driven by this editable source (legacy plans migrate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<usize>,
}
impl Source {
    pub fn validate(&self, layout: &RangeLayout, elements: usize) -> Result<(), String> {
        self.config.validate()?;
        if self.name.trim().is_empty()
            || self.name.chars().count() > 64
            || !(100..=100_000).contains(&self.power_milli)
            || self.visual > 2
            || self.effect.is_some_and(|i| i > 9)
            || self.input.is_some_and(|i| i >= 32)
            || self.binding.is_some_and(|i| i >= elements)
            || (self.detonate
                && (self.config.delivery != crate::explosion::Delivery::Blast
                    || self.binding.is_some()))
            || (0..3).any(|a| {
                self.at[a] < 0
                    || self.at[a] >= layout.size[a] as i32
                    || self.target[a] < 0
                    || self.target[a] >= layout.size[a] as i32
            })
            || self.at[0] < 32
            || self.at[0] >= layout.size[0] as i32 - 32
            || self.at[2] < 32
            || self.at[2] >= layout.size[2] as i32 - 32
        {
            return Err("Некорректный источник оружия: координаты, мощность или привязка".into());
        }
        Ok(())
    }
    pub fn resolved(&self) -> ExplosionConfig {
        if matches!(
            self.config.delivery,
            crate::explosion::Delivery::Blast | crate::explosion::Delivery::Kinetic
        ) {
            self.config.at_power(self.power_milli)
        } else {
            let mut c = self.config;
            c.energy = (u64::from(c.energy) * u64::from(self.power_milli) / 1000)
                .clamp(1, 1_000_000) as u32;
            c
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Workspace {
    pub sources: Vec<Source>,
    pub selected: usize,
    #[serde(default)]
    pub material: Vec<(u16, u8, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_material: Option<u16>,
    #[serde(default)]
    pub observation_ticks: Option<u32>,
    #[serde(default)]
    pub editor_step: Option<i32>,
    #[serde(default)]
    pub surface_snap: Option<bool>,
    #[serde(default)]
    pub allow_overlap: bool,
    #[serde(default)]
    pub devices: Vec<crate::lab_tools::Device>,
}
impl Workspace {
    pub fn validate(&self, layout: &RangeLayout, elements: usize) -> Result<(), String> {
        if self.devices.len() > crate::lab_tools::Tuning::stock().devices
            || self.sources.len() > 32
            || (!self.sources.is_empty() && self.selected >= self.sources.len())
            || self
                .observation_ticks
                .is_some_and(|v| !(1..=18000).contains(&v))
            || self.editor_step.is_some_and(|v| !(1..=16).contains(&v))
            || self
                .selected_material
                .is_some_and(|id| ![1, 2, 3, 4, 5, 6, 8, 10, 11, 12].contains(&id))
            || self.material.len() > 32
            || self.material.iter().any(|(id, field, value)| {
                ![1, 2, 3, 4, 5, 6, 8, 10, 11, 12].contains(id)
                    || *field > 2
                    || *value == 0
                    || *value > 1_000_000
            })
        {
            return Err("Некорректные настройки лаборатории".into());
        }
        for source in &self.sources {
            source.validate(layout, elements)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_material_is_optional_stable_and_validated() {
        let mut w = Workspace::default();
        let layout = super::super::Definition::stock().layout;
        let old = ron::to_string(&w).unwrap();
        assert!(!old.contains("selected_material"));
        assert_eq!(
            ron::from_str::<Workspace>(&old).unwrap().selected_material,
            None
        );
        w.selected_material = Some(5);
        w.validate(&layout, 0).unwrap();
        assert_eq!(
            ron::from_str::<Workspace>(&ron::to_string(&w).unwrap())
                .unwrap()
                .selected_material,
            Some(5)
        );
        w.selected_material = Some(999);
        assert!(w.validate(&layout, 0).is_err());
    }
    #[test]
    fn laboratory_local_charge_roundtrips_and_legacy_sources_keep_projectile_mode() {
        let layout = super::super::Definition::stock().layout;
        let mut source = Source {
            name: "Charge".into(),
            at: [128, 32, 128],
            target: [384, 32, 336],
            binding: None,
            config: crate::explosion::ExplosionConfig::from_ron(include_str!(
                "../../../data/explosion.ron"
            ))
            .unwrap(),
            power_milli: 1000,
            visual: 0,
            effect: Some(0),
            detonate: false,
            input: None,
        };
        let old = ron::to_string(&source).unwrap();
        assert!(!old.contains("detonate"));
        assert!(!ron::from_str::<Source>(&old).unwrap().detonate);
        source.detonate = true;
        source.validate(&layout, 1).unwrap();
        assert_eq!(
            ron::from_str::<Source>(&ron::to_string(&source).unwrap()).unwrap(),
            source
        );
        source.binding = Some(0);
        assert!(source.validate(&layout, 1).is_err());
        source.binding = None;
        source.config.delivery = crate::explosion::Delivery::Laser;
        assert!(source.validate(&layout, 1).is_err());
        assert_eq!(super::super::events::stock().len(), 9);
    }
}
