//! Authored experiments contain geometry and inputs, never their own physics.
use crate::range::RangeLayout;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Phenomenon {
    Blast,
    Nuclear,
    Penetration,
    RicochetSpall,
    HeatCooling,
    Combustion,
    Melt,
    HeatExchange,
    SupportCollapse,
    GravityImpact,
    Collision,
    Shield,
    Terrain,
    Vegetation,
    BodyDamage,
    Fuel,
    Debris,
    Navigation,
    Excavation,
    Canister,
    GranularFlow,
    SoilSlide,
    MaterialContact,
    WaterFlow,
    HydrostaticPressure,
    Buoyancy,
    Wetting,
    Vitrification,
    UndergroundUplift,
    Drilling,
    HydraulicErosion,
}
impl Phenomenon {
    pub const ALL: [Self; 31] = [
        Self::Blast,
        Self::Nuclear,
        Self::Penetration,
        Self::RicochetSpall,
        Self::HeatCooling,
        Self::Combustion,
        Self::Melt,
        Self::HeatExchange,
        Self::SupportCollapse,
        Self::GravityImpact,
        Self::Collision,
        Self::Shield,
        Self::Terrain,
        Self::Vegetation,
        Self::BodyDamage,
        Self::Fuel,
        Self::Debris,
        Self::Navigation,
        Self::Excavation,
        Self::Canister,
        Self::GranularFlow,
        Self::SoilSlide,
        Self::MaterialContact,
        Self::WaterFlow,
        Self::HydrostaticPressure,
        Self::Buoyancy,
        Self::Wetting,
        Self::Vitrification,
        Self::UndergroundUplift,
        Self::Drilling,
        Self::HydraulicErosion,
    ];
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Input {
    Fire,
    Detonate {
        at: [i32; 3],
    },
    FreeControl,
    StartBattle,
    Move {
        actor: usize,
        to: [i32; 2],
    },
    Dig {
        actor: usize,
        at: [i32; 3],
    },
    Ground {
        actor: usize,
        at: [i32; 3],
    },
    Pilot {
        actor: usize,
        throttle: i8,
        turn: i8,
        aim: [i32; 3],
        fire: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Plan {
    pub name: String,
    pub description: String,
    pub duration_ticks: u32,
    pub inputs: Vec<Input>,
}
impl Plan {
    pub fn validate_sources(
        &self,
        layout: &RangeLayout,
        sources: &[super::sources::Source],
    ) -> Result<(), String> {
        self.validate(layout)?;
        let mut linked = std::collections::BTreeSet::new();
        for source in sources {
            if let Some(i) = source.input {
                if !linked.insert(i)
                    || !self.inputs.get(i).is_some_and(|input| match input {
                        Input::Fire => !source.detonate,
                        Input::Detonate { at } => source.detonate && *at == source.at,
                        _ => false,
                    })
                    || layout.battle
                {
                    return Err("Некорректная связь источника с командой сценария".into());
                }
            }
        }
        Ok(())
    }
    pub fn validate(&self, layout: &RangeLayout) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > 96
            || self.description.len() > 2048
            || !(1..=1800).contains(&self.duration_ticks)
            || self.inputs.len() > 32
        {
            return Err("invalid laboratory experiment plan".into());
        }
        let inside = |p: [i32; 3]| (0..3).all(|a| p[a] >= 0 && p[a] < layout.size[a] as i32);
        let mut control = false;
        let mut fighting = false;
        for input in &self.inputs {
            let valid = match input {
                Input::Fire => true,
                Input::Detonate { at } => layout.gun_point && !layout.battle && inside(*at),
                Input::FreeControl => {
                    control = true;
                    layout.battle
                }
                Input::StartBattle => {
                    fighting = true;
                    control && layout.battle
                }
                Input::Move { actor, to } => {
                    control
                        && *actor < layout.units.len()
                        && inside([to[0], layout.ground_level, to[1]])
                }
                Input::Dig { actor, at } => {
                    control
                        && *actor < layout.units.len()
                        && inside(*at)
                        && layout.economy.as_ref().is_some_and(|e| {
                            e.kind(layout.units[*actor].kind)
                                .is_some_and(|k| k.tank > 0)
                        })
                }
                Input::Ground { actor, at } => {
                    control && fighting && *actor < layout.units.len() && inside(*at)
                }
                Input::Pilot {
                    actor,
                    throttle,
                    turn,
                    aim,
                    ..
                } => {
                    control
                        && *actor < layout.units.len()
                        && (-1..=1).contains(throttle)
                        && (-1..=1).contains(turn)
                        && inside(*aim)
                }
            };
            if !valid {
                return Err("invalid laboratory experiment input".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Experiment {
    pub id: String,
    pub phenomena: Vec<Phenomenon>,
    pub weapon: u8,
    pub layout: RangeLayout,
    pub plan: Plan,
}
impl Experiment {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || self.weapon > 9
            || self.phenomena.is_empty()
            || self.phenomena.len() > Phenomenon::ALL.len()
            || self.layout.size.iter().any(|n| *n > 1024)
        {
            return Err("invalid laboratory experiment".into());
        }
        super::catalog::compose(&self.layout, &[])?;
        self.plan.validate(&self.layout)
    }
}
pub fn stock() -> Vec<Experiment> {
    let experiments: Vec<Experiment> =
        ron::from_str(include_str!("../../../data/lab-experiments.ron"))
            .expect("lab experiments RON");
    for (i, e) in experiments.iter().enumerate() {
        e.validate().expect("valid laboratory experiment");
        assert!(!experiments[..i].iter().any(|a| a.id == e.id));
    }
    experiments
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_experiments_cover_the_supported_effects_with_bounded_core_inputs() {
        let cases = stock();
        for effect in Phenomenon::ALL {
            assert!(
                cases.iter().any(|e| e.phenomena.contains(&effect)),
                "{effect:?}"
            );
        }
        let mut bad = cases[0].clone();
        bad.plan.inputs = vec![Input::Dig {
            actor: usize::MAX,
            at: [0; 3],
        }];
        assert!(bad.validate().is_err());
    }
}
