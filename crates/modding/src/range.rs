//! Bounded, integer-only fixtures for the live weapons proving ground.
use serde::{Deserialize, Serialize};

pub const MAX_RANGE_BLOCKS: usize = 256;
pub const MAX_RANGE_ZONES: usize = 12;

/// Size of an authored clearing used for remote battle deployment.
#[derive(Clone, Debug, Deserialize)]
pub struct RangeEditor {
    pub clearing_side: i32,
    pub clearing_height: i32,
}
impl RangeEditor {
    pub fn from_ron(s: &str) -> Result<Self, String> {
        let c: Self = ron::from_str(s).map_err(|e| e.to_string())?;
        if !(64..=256).contains(&c.clearing_side) || !(32..=160).contains(&c.clearing_height) {
            return Err("invalid range editor clearing".into());
        }
        Ok(c)
    }
}

/// A prepared sector of the same world, with an authored firing position.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeZone {
    pub name: String,
    pub min: [i32; 2],
    pub max: [i32; 2],
    pub spawn: [i32; 3],
    pub target: [i32; 3],
    /// Optional one-click experiment prepared for this sector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<RangeScenario>,
}
impl RangeZone {
    pub fn contains(&self, p: [i32; 2], margin: i32) -> bool {
        (0..2).all(|a| p[a] >= self.min[a] + margin && p[a] < self.max[a] - margin)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum RangeScenarioWeapon {
    Shell,
    Energy,
    Annihilation,
    Kinetic,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeScenario {
    pub title: String,
    pub description: String,
    pub weapon: RangeScenarioWeapon,
    pub power_milli: u32,
    /// The first shot uses the sector's authored target; later shots keep
    /// damage in the same world so their effects can be compared.
    pub shots: Vec<[i32; 3]>,
}
pub const fn stock_gun() -> [i32; 3] {
    [128, 16, 336]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum RangeUnitKind {
    #[default]
    Tank,
    Strider,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeUnitDef {
    #[serde(default)]
    pub allied: bool,
    pub kind: RangeUnitKind,
    pub at: [i32; 3],
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeBlock {
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub material: u16,
}
impl RangeBlock {
    pub fn contains(&self, p: [i32; 3]) -> bool {
        (0..3).all(|a| p[a] >= self.min[a] && p[a] < self.max[a])
    }
}

/// A dome the range raises over a point of the ground (ADR-276). The
/// sphere continues under the surface unseen, so digging under a shield
/// works (ADR-040).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeShieldDef {
    pub at: [i32; 3],
    pub radius: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeLayout {
    /// Battle actors are the placed roster; the test gun is parked off-scene.
    #[serde(default)]
    pub battle: bool,
    /// Side, height and side of the world this layout is laid out in,
    /// world units (ADR-315). A layout without one keeps the stock
    /// range, so that every preset written before the size moved into
    /// the data goes on meaning what it meant.
    #[serde(default = "stock_size")]
    pub size: [u32; 3],
    #[serde(default)]
    pub course: Option<RangeCourse>,
    #[serde(default = "stock_gun")]
    pub gun: [i32; 3],
    #[serde(default)]
    pub gun_kind: RangeUnitKind,
    #[serde(default)]
    pub zones: Vec<RangeZone>,
    pub target: [i32; 3],
    /// Automatic acquisition radius in world units; manual fire is unchanged.
    #[serde(default = "default_auto_attack_range")]
    pub auto_attack_range: i32,
    /// Target-plane fuse makes the with/without-cover comparison reproducible.
    pub airburst_on_target: bool,
    pub clear_min: [i32; 2],
    pub clear_max: [i32; 2],
    pub units: Vec<RangeUnitDef>,
    /// Later blocks override earlier blocks, allowing cladding and cutouts.
    pub blocks: Vec<RangeBlock>,
    #[serde(default)]
    pub shields: Vec<RangeShieldDef>,
    pub power_milli: Vec<u32>,
    pub unit_resistance: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeRamp {
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub axis: usize,
    pub reverse: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Patrol {
    pub unit: usize,
    pub points: Vec<[i32; 2]>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeCourse {
    pub min: [i32; 2],
    pub max: [i32; 2],
    pub ramps: Vec<RangeRamp>,
    pub patrols: Vec<Patrol>,
    pub tank_speed: i32,
    pub walker_speed: i32,
    pub step_height: i32,
    #[serde(default = "default_walker_step_height")]
    pub walker_step_height: i32,
    #[serde(default = "default_walker_terrain_tilt_pct")]
    pub walker_terrain_tilt_pct: i32,
    #[serde(default = "default_tank_terrain_tilt_pct")]
    pub tank_terrain_tilt_pct: i32,
    #[serde(default = "default_tank_terrain_approach_pct")]
    pub tank_terrain_approach_pct: i32,
    #[serde(default = "default_walker_terrain_approach_pct")]
    pub walker_terrain_approach_pct: i32,
    pub arrival: i32,
    pub reload_ticks: u32,
    #[serde(default = "default_turn_alignment")]
    pub turn_alignment_pct: i32,
    #[serde(default = "default_replan")]
    pub replan_ticks: u32,
}
fn default_auto_attack_range() -> i32 {
    1200
}
fn default_turn_alignment() -> i32 {
    97
}
fn default_replan() -> u32 {
    90
}
fn default_walker_step_height() -> i32 {
    4
}
fn default_walker_terrain_tilt_pct() -> i32 {
    84
}
fn default_tank_terrain_tilt_pct() -> i32 {
    91
}
fn default_tank_terrain_approach_pct() -> i32 {
    60
}
fn default_walker_terrain_approach_pct() -> i32 {
    50
}
/// The range as it was before the size moved into the data.
pub const fn stock_size() -> [u32; 3] {
    [640, 192, 640]
}
impl RangeLayout {
    pub fn to_ron(&self) -> Result<String, String> {
        self.validate()?;
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
    }
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let r: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        r.validate()?;
        Ok(r)
    }
    pub fn validate(&self) -> Result<(), String> {
        let r = self;
        let size = r.size.map(|v| v as i32);
        // The side is a multiple of sixteen so that every allowed cell of
        // the planner divides it and chunks tile it whole; the ceiling is
        // what a dense volume of occupancy still fits in memory.
        if r.size
            .iter()
            .any(|v| !(128..=16384).contains(v) || v % 16 != 0)
            || r.unit_resistance == 0
            || r.unit_resistance > 1_000_000
            || r.units.len() > 8
            || !(1..=100_000).contains(&r.auto_attack_range)
            || r.blocks.len() > MAX_RANGE_BLOCKS
            || r.power_milli.is_empty()
            || r.power_milli.len() > 8
            || r.power_milli.iter().any(|p| !(100..=4000).contains(p))
            || r.target[0] <= 160
            || (0..3).any(|a| r.target[a] < 0 || r.target[a] >= size[a])
            || (0..2).any(|a| {
                r.clear_min[a] < 0
                    || r.clear_max[a] > size[a * 2]
                    || r.clear_min[a] >= r.clear_max[a]
            })
            || r.blocks.iter().any(|b| {
                ![0, 1, 2, 3, 4, 5, 6, 8].contains(&b.material)
                    || (0..3).any(|a| b.min[a] < 0 || b.max[a] > size[a] || b.min[a] >= b.max[a])
            })
            || std::iter::once(r.gun)
                .chain(r.units.iter().map(|u| u.at))
                .any(|at| {
                    at[0] < 32
                        || at[0] >= size[0] - 32
                        || at[1] < 0
                        || at[1] >= size[1]
                        || at[2] < 32
                        || at[2] >= size[2] - 32
                })
            || r.zones.len() > MAX_RANGE_ZONES
            || r.zones.iter().any(|z| {
                z.name.trim().is_empty()
                    || z.name.chars().count() > 48
                    || (0..2).any(|a| {
                        z.min[a] < 0
                            || z.max[a] > size[a * 2]
                            || z.min[a] >= z.max[a]
                            || z.max[a] - z.min[a] < 96
                    })
                    || !z.contains([z.spawn[0], z.spawn[2]], 32)
                    || !z.contains([z.target[0], z.target[2]], 0)
                    || [z.spawn, z.target]
                        .iter()
                        .any(|p| p[1] < 0 || p[1] >= size[1])
                    || z.scenario.as_ref().is_some_and(|s| {
                        s.title.trim().is_empty()
                            || s.title.chars().count() > 48
                            || s.description.trim().is_empty()
                            || s.description.chars().count() > 160
                            || !r.power_milli.contains(&s.power_milli)
                            || !(1..=4).contains(&s.shots.len())
                            || s.shots.first() != Some(&z.target)
                            || s.shots.iter().any(|p| {
                                !z.contains([p[0], p[2]], 0) || p[1] < 0 || p[1] >= size[1]
                            })
                    })
            })
            || r.shields.len() > 8
            || r.shields.iter().any(|s| {
                !crate::shield::SHIELD_RADIUS.contains(&s.radius)
                    || (0..3).any(|a| s.at[a] < 0 || s.at[a] >= size[a])
            })
        {
            return Err("range layout exceeds supported bounds".into());
        }
        if let Some(c) = &r.course {
            if c.patrols
                .iter()
                .enumerate()
                .any(|(i, p)| c.patrols[..i].iter().any(|q| q.unit == p.unit))
                || c.ramps.len() > 32
                || c.patrols.len() > 8
                || !(1..=100).contains(&c.tank_speed)
                || !(1..=100).contains(&c.walker_speed)
                || !(1..=8).contains(&c.step_height)
                || !(1..=16).contains(&c.walker_step_height)
                || !(50..=99).contains(&c.walker_terrain_tilt_pct)
                || !(50..=99).contains(&c.tank_terrain_tilt_pct)
                || !(1..=100).contains(&c.tank_terrain_approach_pct)
                || !(1..=100).contains(&c.walker_terrain_approach_pct)
                || !(1..=16).contains(&c.arrival)
                || c.reload_ticks > 1800
                || c.replan_ticks == 0
                || c.replan_ticks > 1800
                || !(1..=99).contains(&c.turn_alignment_pct)
                || (0..2).any(|a| c.min[a] < 0 || c.max[a] > size[a * 2] || c.min[a] >= c.max[a])
                || c.patrols.iter().any(|p| {
                    p.unit >= r.units.len()
                        || p.points.len() < 2
                        || p.points.len() > 32
                        || p.points.iter().any(|p| {
                            (0..2).any(|a| p[a] < c.min[a] + 32 || p[a] >= c.max[a] - 32)
                                && !r.zones.iter().any(|z| z.contains(*p, 32))
                        })
                })
                || c.ramps.iter().any(|b| {
                    ![0, 2].contains(&b.axis)
                        || (0..3)
                            .any(|a| b.min[a] < 0 || b.max[a] > size[a] || b.min[a] >= b.max[a])
                })
            {
                return Err("course exceeds supported bounds".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_scenarios_are_bounded_sequences_in_their_sectors() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        assert_eq!(
            layout.zones.iter().filter(|z| z.scenario.is_some()).count(),
            5
        );
        let mut invalid = layout.clone();
        invalid.zones[0].scenario.as_mut().unwrap().shots[0][0] += 1;
        assert!(invalid.validate().is_err());
        invalid = layout;
        invalid.zones[2]
            .scenario
            .as_mut()
            .unwrap()
            .shots
            .push([i32::MAX, 0, 0]);
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn older_course_defaults_keep_the_walkers_original_limits() {
        let old = include_str!("../../../data/range-cover.ron")
            .replace("walker_step_height: 8,", "")
            .replace("walker_terrain_tilt_pct: 70,", "")
            .replace("tank_terrain_tilt_pct: 88,", "")
            .replace("tank_terrain_approach_pct: 70,", "")
            .replace("walker_terrain_approach_pct: 90,", "");
        let course = RangeLayout::from_ron(&old).unwrap().course.unwrap();
        assert_eq!(course.walker_step_height, 4);
        assert_eq!(course.walker_terrain_tilt_pct, 84);
        assert_eq!(course.tank_terrain_tilt_pct, 91);
        assert_eq!(course.tank_terrain_approach_pct, 60);
        assert_eq!(course.walker_terrain_approach_pct, 50);
    }
    #[test]
    fn sector_bounds_and_catalog_limits_are_rejected_without_overflow() {
        let good = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        for (lo, hi) in [(i32::MAX, i32::MIN), (-1, 100), (100, 101), (100, 20000)] {
            let mut r = good.clone();
            r.zones[0].min[0] = lo;
            r.zones[0].max[0] = hi;
            assert!(r.validate().is_err());
        }
        let mut r = good.clone();
        r.zones = vec![r.zones[0].clone(); MAX_RANGE_ZONES + 1];
        assert!(r.validate().is_err());
        r = good;
        r.blocks = vec![r.blocks[0].clone(); MAX_RANGE_BLOCKS + 1];
        assert!(r.validate().is_err());
    }
}
