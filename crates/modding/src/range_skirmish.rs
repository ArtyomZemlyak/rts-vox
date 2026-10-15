//! Authored encounters use the same placement data as the range editor.
use crate::range::{RangeBlock, RangeUnitDef, RangeUnitKind, RangeZone};
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
pub struct Skirmish {
    pub title: String,
    pub briefing: String,
    pub zone: RangeZone,
    pub units: Vec<RangeUnitDef>,
    pub blocks: Vec<RangeBlock>,
}
impl Skirmish {
    pub fn from_ron(src: &str) -> Result<Self, String> {
        let s: Self = ron::from_str(src).map_err(|e| e.to_string())?;
        if s.title.is_empty()
            || s.briefing.is_empty()
            || s.units.len() > crate::range::MAX_RANGE_UNITS
            || s.units.iter().any(|u| !u.valid_loadout())
            || !s.units.iter().any(|u| u.allied)
            || !s.units.iter().any(|u| !u.allied)
        {
            return Err("skirmish needs a briefing and two bounded armies".into());
        }
        Ok(s)
    }
}

/// A map-wide battle keeps the authored terrain and uses the bases as its
/// victory objective. Positions are authored, so loading never invents an
/// army based on whatever units happened to be in the editor.
#[derive(Clone, Debug, Deserialize)]
pub struct WorldBattle {
    pub title: String,
    pub briefing: String,
    pub units: Vec<RangeUnitDef>,
    #[serde(default)]
    pub blocks: Vec<RangeBlock>,
}
impl WorldBattle {
    pub fn from_ron(src: &str) -> Result<Self, String> {
        let battle: Self = ron::from_str(src).map_err(|e| e.to_string())?;
        if battle.title.trim().is_empty()
            || battle.briefing.trim().is_empty()
            || battle.units.len() > crate::range::MAX_RANGE_UNITS
            || battle.units.iter().any(|u| !u.valid_loadout())
            || battle.blocks.len() > crate::range::MAX_RANGE_BLOCKS
            || [true, false].iter().any(|&side| {
                battle
                    .units
                    .iter()
                    .filter(|u| u.allied == side && u.kind == RangeUnitKind::Base)
                    .count()
                    != 1
                    || !battle
                        .units
                        .iter()
                        .any(|u| u.allied == side && u.kind != RangeUnitKind::Base)
            })
        {
            return Err("world battle needs one base and an army on each side".into());
        }
        Ok(battle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::range::{RangeLayout, RangeLoadout};

    #[test]
    fn world_armies_include_all_kinds_and_authored_ammunition() {
        let preset = WorldBattle::from_ron(include_str!("../../../data/battle-world.ron")).unwrap();
        for side in [true, false] {
            let army: Vec<_> = preset.units.iter().filter(|u| u.allied == side).collect();
            assert_eq!(army.len(), 31);
            for kind in RangeUnitKind::ALL {
                assert!(
                    army.iter().any(|u| u.kind == kind),
                    "missing {kind:?} on side {side}"
                );
            }
            assert_eq!(
                army.iter()
                    .filter(|u| u.kind == RangeUnitKind::ArtilleryT2)
                    .count(),
                1
            );
            assert!(army
                .iter()
                .all(|u| u.weapon != Some(RangeLoadout::Annihilation)));
            for weapon in [RangeLoadout::Blast, RangeLoadout::Energy] {
                assert!(army
                    .iter()
                    .any(|u| u.kind == RangeUnitKind::Tank && u.weapon == Some(weapon)));
            }
            assert!(army
                .iter()
                .any(|u| u.kind == RangeUnitKind::Tank && u.weapon.is_none()));
        }
        let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        layout.units = preset.units;
        let text = layout.to_ron().unwrap();
        assert_eq!(layout, RangeLayout::from_ron(&text).unwrap());
        let loadout = layout.units[4].clone();
        layout.units.remove(1);
        assert_eq!(
            layout.units[3], loadout,
            "deletion shifted ammunition to another unit"
        );
    }

    #[test]
    fn fixed_guns_reject_authored_overrides_in_every_preset_loader() {
        let world = include_str!("../../../data/battle-world.ron");
        let invalid = world.replacen("kind: Base,", "kind: Base, weapon: Some(Energy),", 1);
        assert!(WorldBattle::from_ron(&invalid).is_err());
        let duel = include_str!("../../../data/skirmish-canister.ron");
        let invalid = duel.replacen(
            "kind: ArtilleryT2,",
            "kind: ArtilleryT2, weapon: Some(Energy),",
            1,
        );
        assert!(Skirmish::from_ron(&invalid).is_err());
        let small = include_str!("../../../data/range-battle.ron");
        let invalid = small.replacen("kind: Strider,", "kind: Strider, weapon: Some(Energy),", 1);
        assert!(crate::range_battle::Tuning::from_ron(&invalid).is_err());
        let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        for kind in [
            RangeUnitKind::Strider,
            RangeUnitKind::Spider,
            RangeUnitKind::ArtilleryT2,
            RangeUnitKind::ArtilleryTurretT2,
            RangeUnitKind::ArtilleryTurretT3,
            RangeUnitKind::TurretT1,
            RangeUnitKind::TurretT2,
            RangeUnitKind::Base,
        ] {
            layout.units[0].kind = kind;
            layout.units[0].weapon = Some(RangeLoadout::Energy);
            assert!(layout.validate().is_err(), "allowed override on {kind:?}");
        }
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Artillery] {
            layout.units[0].kind = kind;
            for weapon in [
                RangeLoadout::Blast,
                RangeLoadout::Kinetic,
                RangeLoadout::Energy,
                RangeLoadout::Annihilation,
            ] {
                layout.units[0].weapon = Some(weapon);
                assert!(layout.validate().is_ok());
            }
        }
    }
}
