//! One stock balance table; named references resolve while loading packs.
//! Numeric fields remain accepted for older packs and explicit custom tuning.
use serde::{de, Deserialize, Deserializer};
use std::sync::OnceLock;

/// Technical loading ceiling, shared with long-range ballistic tuning.
pub const MAX_DISTANCE: i32 = 6000;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttackRanges {
    pub flame: i32,
    pub tank_blast: i32,
    pub kinetic: i32,
    pub energy: i32,
    pub machine_gun: i32,
    pub laser: i32,
    pub spider_laser: i32,
    pub artillery: i32,
    pub canister: i32,
    pub siege: i32,
    #[serde(default = "default_nuclear")]
    pub nuclear: i32,
    pub acquisition: i32,
}
fn default_nuclear() -> i32 {
    #[derive(Deserialize)]
    struct Nuclear {
        nuclear: i32,
    }
    ron::from_str::<Nuclear>(include_str!("../../../data/attack-ranges.ron"))
        .expect("stock nuclear reach")
        .nuclear
}
impl AttackRanges {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let value: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if value
            .values()
            .iter()
            .any(|n| !(100..=MAX_DISTANCE).contains(n))
        {
            return Err("attack distances outside bounded limits".into());
        }
        Ok(value)
    }
    fn values(&self) -> [i32; 12] {
        [
            self.flame,
            self.tank_blast,
            self.kinetic,
            self.energy,
            self.machine_gun,
            self.laser,
            self.spider_laser,
            self.artillery,
            self.canister,
            self.siege,
            self.nuclear,
            self.acquisition,
        ]
    }
    pub fn named(&self, name: &str) -> Option<i32> {
        Some(match name {
            "flame" => self.flame,
            "tank_blast" => self.tank_blast,
            "kinetic" => self.kinetic,
            "energy" => self.energy,
            "machine_gun" => self.machine_gun,
            "laser" => self.laser,
            "spider_laser" => self.spider_laser,
            "artillery" => self.artillery,
            "canister" => self.canister,
            "siege" => self.siege,
            "nuclear" => self.nuclear,
            "acquisition" => self.acquisition,
            _ => return None,
        })
    }
}
pub fn stock() -> &'static AttackRanges {
    static TABLE: OnceLock<AttackRanges> = OnceLock::new();
    TABLE.get_or_init(|| {
        AttackRanges::from_ron(include_str!("../../../data/attack-ranges.ron"))
            .expect("stock attack distances")
    })
}

/// A name reads the common table; a number is an explicit legacy/mod override.
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    struct Reach;
    impl<'de> de::Visitor<'de> for Reach {
        type Value = i32;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an attack-distance name or an integer override")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<i32, E> {
            stock()
                .named(v)
                .ok_or_else(|| E::custom(format!("unknown attack distance: {v}")))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i32, E> {
            i32::try_from(v).map_err(E::custom)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i32, E> {
            i32::try_from(v).map_err(E::custom)
        }
    }
    d.deserialize_any(Reach)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_packs_reference_the_table_instead_of_copying_attack_numbers() {
        for source in [
            include_str!("../../../data/range-battle.ron"),
            include_str!("../../../data/range-weapons.ron"),
            include_str!("../../../data/spider.ron"),
            include_str!("../../../data/range-siege.ron"),
            include_str!("../../../data/weapons.ron"),
            include_str!("../../../data/range-cover.ron"),
            include_str!("../../../data/range-km.ron"),
        ] {
            let data = source
                .lines()
                .map(|line| line.split("//").next().unwrap())
                .collect::<Vec<_>>()
                .join("\n");
            let refs: Vec<_> = data.split("range:").skip(1).collect();
            assert!(!refs.is_empty());
            for reference in refs {
                let value = reference.trim_start();
                assert!(value.starts_with('"'), "copied attack distance: {value}");
                let name = value[1..].split('"').next().unwrap();
                assert!(stock().named(name).is_some(), "unknown distance: {name}");
            }
        }
        let battle =
            crate::range_battle::Tuning::from_ron(include_str!("../../../data/range-battle.ron"))
                .unwrap();
        let streams =
            crate::range_weapon::Tuning::from_ron(include_str!("../../../data/range-weapons.ron"))
                .unwrap();
        let spider =
            crate::spider::Tuning::from_ron(include_str!("../../../data/spider.ron")).unwrap();
        let weapons = crate::weapons::from_ron(include_str!("../../../data/weapons.ron")).unwrap();
        assert_eq!(battle.canister.range, stock().canister);
        assert_eq!(battle.siege.range, stock().siege);
        assert_eq!(spider.laser.range, stock().spider_laser);
        assert_eq!(weapons.get(3).unwrap().range, battle.kinetic_range);
        assert_eq!(weapons.get(4).unwrap().range, streams.machine_gun.range);
    }
    #[derive(Deserialize)]
    struct Pack {
        #[serde(deserialize_with = "deserialize")]
        range: i32,
    }
    #[test]
    fn names_and_legacy_integers_resolve_without_silent_fallback() {
        assert_eq!(
            ron::from_str::<Pack>("(range: \"kinetic\")").unwrap().range,
            stock().kinetic
        );
        assert_eq!(ron::from_str::<Pack>("(range: 917)").unwrap().range, 917);
        for bad in ["(range: \"typo\")", "(range: 1.5)", "(range: 4294967296)"] {
            assert!(ron::from_str::<Pack>(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn the_table_is_bounded_and_covers_the_stock_role_hierarchy() {
        let t = stock();
        assert!(t.flame < t.machine_gun);
        assert!(t.machine_gun < t.artillery);
        assert!(t.spider_laser < t.canister);
        assert!(t.artillery < t.canister && t.canister < t.siege);
        assert!(t.acquisition >= *t.values()[..t.values().len() - 1].iter().max().unwrap());
        let source = include_str!("../../../data/attack-ranges.ron");
        for bad in [
            source.replace("flame: 300", "flame: 0"),
            source.replace("siege: 5600", &format!("siege: {}", MAX_DISTANCE + 1)),
        ] {
            assert!(AttackRanges::from_ron(&bad).is_err());
        }
    }
}
