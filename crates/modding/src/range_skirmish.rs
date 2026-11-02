//! Authored encounters use the same placement data as the range editor.
use crate::range::{RangeBlock, RangeLayout, RangeUnitDef, RangeUnitKind, RangeZone};
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
pub struct ScriptOrder {
    pub actor: usize,
    pub tick: u32,
    pub order: ScriptedOrder,
}
#[derive(Clone, Debug, Deserialize)]
pub enum ScriptedOrder {
    Attack(usize),
    Dig([i32; 3]),
    Reclaim([i32; 3]),
}
#[derive(Clone, Debug, Deserialize)]
pub struct Skirmish {
    pub title: String,
    pub briefing: String,
    pub zone: RangeZone,
    pub units: Vec<RangeUnitDef>,
    pub blocks: Vec<RangeBlock>,
    #[serde(default)]
    pub world_height: Option<u32>,
    #[serde(default)]
    pub initial_debris: Vec<crate::range::RangeDebris>,
    #[serde(default)]
    pub focus: Option<usize>,
    #[serde(default)]
    pub script: Vec<ScriptOrder>,
    #[serde(default)]
    pub idle_enemies: bool,
    #[serde(default)]
    pub mission: Option<Mission>,
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
            || s.script.iter().any(|order| order.actor >= s.units.len())
            || s.script.windows(2).any(|orders| orders[0].tick > orders[1].tick)
        {
            return Err("skirmish needs a briefing and two bounded armies".into());
        }
        if let Some(mission) = &s.mission {
            mission.validate(&s)?;
        }
        Ok(s)
    }
    /// Prepare authored terrain and actors through the ordinary layout loader.
    pub fn apply_to(&self, mut layout: RangeLayout) -> Result<RangeLayout, String> {
        let stock =
            crate::range_battle::Tuning::from_ron(include_str!("../../../data/range-battle.ron"))?;
        let zone = layout
            .zones
            .iter()
            .position(|z| z.name == stock.zone.name)
            .ok_or("No battle sector")?;
        if self.mission.is_some() {
            layout.blocks.retain(|b| {
                !(b.min[0] < self.zone.max[0]
                    && b.max[0] > self.zone.min[0]
                    && b.min[2] < self.zone.max[1]
                    && b.max[2] > self.zone.min[1])
            });
        }
        layout.battle = true;
        if self.script.iter().any(|order| matches!(order.order, ScriptedOrder::Dig(_) | ScriptedOrder::Reclaim(_))) {
            layout.economy.get_or_insert_with(crate::range_economy::RangeEconomy::stock);
        }
        if let Some(height) = self.world_height {
            layout.size[1] = height;
        }
        layout.idle_enemies = self.idle_enemies;
        layout.units = self.units.clone();
        layout.unit_yaws.clear();
        layout.gun = self.zone.spawn;
        layout.target = self.zone.target;
        layout.zones[zone] = self.zone.clone();
        layout.blocks.extend(self.blocks.clone());
        layout.initial_debris.extend(self.initial_debris.clone());
        if let Some(course) = &mut layout.course {
            course.patrols.clear();
        }
        layout.validate()?;
        Ok(layout)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Mission {
    pub focus: RangeUnitKind,
    #[serde(default)]
    pub protect: Vec<usize>,
    /// An authored mission can require a particular gun to remain usable.
    /// This is distinct from preserving its core or ending the SIM battle.
    #[serde(default)]
    pub protect_weapons: Vec<usize>,
    pub steps: Vec<MissionStep>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct MissionStep {
    pub text: String,
    pub goal: MissionGoal,
}
#[derive(Clone, Debug, Deserialize)]
pub enum MissionGoal {
    Reach {
        actors: Vec<usize>,
        count: usize,
        point: [i32; 2],
        radius: i32,
    },
    Destroy {
        actors: Vec<usize>,
    },
}
impl Mission {
    fn validate(&self, preset: &Skirmish) -> Result<(), String> {
        let valid_ids = |ids: &[usize], allied| {
            !ids.is_empty()
                && ids.len() <= preset.units.len()
                && ids.iter().enumerate().all(|(i, &id)| {
                    preset.units.get(id).is_some_and(|u| u.allied == allied)
                        && !ids[..i].contains(&id)
                })
        };
        if self.steps.is_empty()
            || self.steps.len() > 8
            || !preset
                .units
                .iter()
                .any(|u| u.allied && u.kind == self.focus)
            || (!self.protect.is_empty() && !valid_ids(&self.protect, true))
            || (!self.protect_weapons.is_empty()
                && (!valid_ids(&self.protect_weapons, true)
                    || self
                        .protect_weapons
                        .iter()
                        .any(|&id| preset.units[id].kind == RangeUnitKind::Base)))
            || preset.blocks.len() > crate::range::MAX_RANGE_BLOCKS
            || preset
                .units
                .iter()
                .any(|u| !preset.zone.contains([u.at[0], u.at[2]], 0))
            || self.steps.iter().any(|step| {
                step.text.trim().is_empty()
                    || step.text.chars().count() > 160
                    || match &step.goal {
                        MissionGoal::Reach {
                            actors,
                            count,
                            point,
                            radius,
                        } => {
                            !valid_ids(actors, true)
                                || *count == 0
                                || *count > actors.len()
                                || !(1..=1000).contains(radius)
                                || !preset.zone.contains(*point, *radius)
                                || actors.iter().any(|&id| preset.units[id].kind.emplacement())
                        }
                        MissionGoal::Destroy { actors } => !valid_ids(actors, false),
                    }
            })
        {
            return Err("invalid tactical mission objectives".into());
        }
        Ok(())
    }
}

/// Stable IDs keep the original six encounters available to tools and replays.
pub const STOCK: &[&str] = &[
    include_str!("../../../data/skirmish-breakthrough.ron"),
    include_str!("../../../data/skirmish-pincer.ron"),
    include_str!("../../../data/skirmish-spider.ron"),
    include_str!("../../../data/skirmish-turrets.ron"),
    include_str!("../../../data/skirmish-canister.ron"),
    include_str!("../../../data/skirmish-siege.ron"),
    include_str!("../../../data/mission-artillery.ron"),
    include_str!("../../../data/mission-tanks-t1.ron"),
    include_str!("../../../data/mission-tanks-t2.ron"),
    include_str!("../../../data/mission-spider.ron"),
    include_str!("../../../data/mission-strider.ron"),
    include_str!("../../../data/mission-canister-mobile.ron"),
    include_str!("../../../data/mission-bastion.ron"),
    include_str!("../../../data/mission-turret-t1.ron"),
    include_str!("../../../data/mission-turret-t2.ron"),
    include_str!("../../../data/mission-canister-defense.ron"),
    include_str!("../../../data/mission-siege-defense.ron"),
    include_str!("../../../data/mission-base.ron"),
    include_str!("../../../data/mission-nuclear.ron"),
    include_str!("../../../data/mission-lightning.ron"),
    include_str!("../../../data/mission-eidolon-tank.ron"),
    include_str!("../../../data/skirmish-walker04.ron"),
    include_str!("../../../data/skirmish-razlom.ron"),
    include_str!("../../../data/skirmish-staratel.ron"),
    include_str!("../../../data/skirmish-kil.ron"),
    include_str!("../../../data/skirmish-skoba.ron"),
];
pub fn stock(id: usize) -> Result<Skirmish, String> {
    Skirmish::from_ron(STOCK.get(id).ok_or("Unknown encounter")?)
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
    fn tactical_catalog_covers_every_kind_and_builds_real_bounded_layouts() {
        let stock_layout =
            RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        let mut focus = Vec::new();
        let mut titles = std::collections::BTreeSet::new();
        for (id, _) in STOCK.iter().enumerate() {
            let preset = stock(id).unwrap();
            assert!(titles.insert(preset.title.clone()));
            let layout = preset.apply_to(stock_layout.clone()).unwrap();
            assert_eq!(layout.units, preset.units);
            assert_eq!(layout.zones.len(), stock_layout.zones.len());
            if let Some(mission) = preset.mission {
                focus.push(mission.focus);
                for own in preset.units.iter().filter(|u| u.allied) {
                    for enemy in preset.units.iter().filter(|u| !u.allied) {
                        let dx = i64::from(own.at[0]) - i64::from(enemy.at[0]);
                        let dz = i64::from(own.at[2]) - i64::from(enemy.at[2]);
                        let ranges = crate::attack_ranges::stock();
                        let reach = |u: &RangeUnitDef| match u.kind {
                            RangeUnitKind::Base
                            | RangeUnitKind::Crawler
                            | RangeUnitKind::GrebenStorage
                            | RangeUnitKind::KilFactoryT1
                            | RangeUnitKind::KilFactoryT2
                            | RangeUnitKind::KilFactoryT3
                            | RangeUnitKind::KilFactoryT4
                            | RangeUnitKind::SkobaDrone => 0,
                            RangeUnitKind::Tank
                            | RangeUnitKind::LightningTank
                            | RangeUnitKind::EidolonTank
                            | RangeUnitKind::TurretT1
                            | RangeUnitKind::TurretT2 => ranges.energy,
                            RangeUnitKind::Strider => ranges.machine_gun,
                            RangeUnitKind::Spider | RangeUnitKind::Walker04 | RangeUnitKind::RazlomT3 => ranges.spider_laser,
                            RangeUnitKind::Artillery => ranges.artillery,
                            RangeUnitKind::ArtilleryT2 | RangeUnitKind::ArtilleryTurretT2 => {
                                ranges.canister
                            }
                            RangeUnitKind::ArtilleryTurretT3 => ranges.siege,
                            RangeUnitKind::NuclearSilo => 0, // Manual strategic launch never opens a mission automatically.
                        };
                        let enemy_reach = if preset.idle_enemies { 0 } else { reach(enemy) };
                        let max = reach(own).max(enemy_reach);
                        assert!(
                            dx * dx + dz * dz > i64::from(max) * i64::from(max),
                            "unsafe deployment {id}"
                        );
                    }
                }
            }
        }
        assert_eq!(focus.len(), 17);
        assert!(RangeUnitKind::ALL.iter().filter(|kind| !matches!(kind, RangeUnitKind::Crawler | RangeUnitKind::Walker04 | RangeUnitKind::RazlomT3 | RangeUnitKind::GrebenStorage | RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3 | RangeUnitKind::KilFactoryT4)).all(|kind| focus.contains(kind)));
        assert!(stock(STOCK.len()).is_err());
    }
    #[test]
    fn mission_references_counts_and_protection_are_validated() {
        let valid = include_str!("../../../data/mission-artillery.ron");
        for broken in [
            valid.replace("actors: [0, 1]", "actors: [0, 99]"),
            valid.replace("count: 1", "count: 3"),
            valid.replace("radius: 260", "radius: -1"),
            valid.replace("actors: [6]", "actors: [0]"),
            valid.replace("protect: []", "protect: [6]"),
            valid.replace("actors: [0, 1]", "actors: [0, 0]"),
        ] {
            assert!(Skirmish::from_ron(&broken).is_err());
        }
    }
    #[test]
    fn mission_weapon_protection_is_optional_and_references_allied_guns() {
        assert!(stock(6)
            .unwrap()
            .mission
            .unwrap()
            .protect_weapons
            .is_empty());
        let valid = include_str!("../../../data/mission-siege-defense.ron");
        assert_eq!(
            Skirmish::from_ron(valid)
                .unwrap()
                .mission
                .unwrap()
                .protect_weapons,
            vec![0]
        );
        for ids in ["[99]", "[4]", "[1]", "[0, 0]"] {
            assert!(Skirmish::from_ron(
                &valid.replace("protect_weapons: [0]", &format!("protect_weapons: {ids}"))
            )
            .is_err());
        }
    }
    #[test]
    fn world_armies_include_all_kinds_and_authored_ammunition() {
        let preset = WorldBattle::from_ron(include_str!("../../../data/battle-world.ron")).unwrap();
        for side in [true, false] {
            let army: Vec<_> = preset.units.iter().filter(|u| u.allied == side).collect();
            assert_eq!(army.len(), 31);
            for kind in RangeUnitKind::ALL
                .into_iter()
                .filter(|k| !matches!(k, RangeUnitKind::NuclearSilo | RangeUnitKind::Crawler | RangeUnitKind::LightningTank | RangeUnitKind::EidolonTank | RangeUnitKind::Walker04 | RangeUnitKind::RazlomT3 | RangeUnitKind::GrebenStorage | RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3 | RangeUnitKind::KilFactoryT4 | RangeUnitKind::SkobaDrone))
            {
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
