//! Deterministic enemy decision rules. All weights are integer world units.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Tuning {
    pub decision_ticks: u32,
    pub focus_fire: bool,
    pub armed_bonus: i32,
    pub mobile_bonus: i32,
    pub switch_margin: i32,
}

impl Tuning {
    pub fn from_ron(src: &str) -> Result<Self, String> {
        let value: Self = ron::from_str(src).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=300).contains(&self.decision_ticks)
            || !(0..=10_000).contains(&self.armed_bonus)
            || !(0..=10_000).contains(&self.mobile_bonus)
            || !(0..=10_000).contains(&self.switch_margin)
        {
            return Err("invalid battle AI tuning".into());
        }
        Ok(())
    }
}

/// Tactical radii of the rule-based controller, world units. Read from the
/// same `battle-ai.ron`; unlike [`Tuning`] they are fixed data of the pack,
/// not runtime configuration, so they enter neither snapshots nor
/// checkpoints (analysis 240, B3). A missing field is a parse error.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Radii {
    /// Allies within this L1 distance count toward focus fire.
    pub support: i32,
    /// A defender is picked among armed movers this close to its base.
    pub guard_pick: i32,
    /// The defender engages hostiles this close to its base...
    pub defend: i32,
    /// ...and keeps a target it already holds out to this distance.
    pub defend_keep: i32,
    /// The defender's post, this far from its base toward the map centre.
    pub guard_post: i32,
    /// Siege units divert to armed threats within this L1 distance.
    pub siege_threat: i32,
}

impl Radii {
    pub fn from_ron(src: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct File {
            radii: Radii,
        }
        let value = ron::from_str::<File>(src).map_err(|e| e.to_string())?.radii;
        let all = [
            value.support,
            value.guard_pick,
            value.defend,
            value.defend_keep,
            value.guard_post,
            value.siege_threat,
        ];
        if all.iter().any(|r| !(1..=100_000).contains(r)) || value.defend_keep < value.defend {
            return Err("invalid battle AI radii".into());
        }
        Ok(value)
    }

    /// The stock pack's radii.
    pub fn stock() -> Self {
        Self::from_ron(include_str!("../../../data/battle-ai.ron")).expect("battle AI radii")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_radii_parse_and_tuning_ignores_them() {
        let src = include_str!("../../../data/battle-ai.ron");
        let r = Radii::from_ron(src).unwrap();
        assert_eq!((r.support, r.siege_threat), (600, 1200));
        assert!(Tuning::from_ron(src).is_ok());
        for broken in [
            src.replace("defend_keep: 2200", "defend_keep: 100"),
            src.replace("support: 600", "support: 0"),
            src.replace("guard_post: 300", "guard_post: -1"),
            src.replace("siege_threat: 1200", "siege_threat: 100001"),
        ] {
            assert!(Radii::from_ron(&broken).is_err(), "{broken}");
        }
    }
}
