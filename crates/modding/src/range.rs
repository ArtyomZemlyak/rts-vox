//! Bounded, integer-only fixtures for the live weapons proving ground.
use serde::{Deserialize, Serialize};

pub const MAX_RANGE_BLOCKS: usize = 384;
/// Machines a range layout places, both sides together (ADR-487).
pub const MAX_RANGE_UNITS: usize = 64;
pub const MAX_RANGE_ZONES: usize = 16;

/// Size of an authored clearing used for remote battle deployment.
#[derive(Clone, Debug, Deserialize)]
pub struct RangeEditor {
    pub clearing_side: i32,
    pub clearing_height: i32,
    #[serde(default)]
    pub subsoil_units: i32,
}
impl RangeEditor {
    pub fn from_ron(s: &str) -> Result<Self, String> {
        let c: Self = ron::from_str(s).map_err(|e| e.to_string())?;
        if !(64..=256).contains(&c.clearing_side)
            || !(32..=160).contains(&c.clearing_height)
            || !(0..=256).contains(&c.subsoil_units)
            || c.subsoil_units % 16 != 0
        {
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
    /// Use the route already authored in the range course for this sector.
    #[serde(default)]
    pub patrols: bool,
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
    Spider,
    Artillery,
    TurretT1,
    TurretT2,
    Base,
    ArtilleryT2,
    ArtilleryTurretT2,
    ArtilleryTurretT3,
    NuclearSilo,
    Crawler,
    LightningTank,
    EidolonTank,
    Walker04,
    RazlomT3,
    GrebenStorage,
    KilFactoryT1,
    KilFactoryT2,
    KilFactoryT3,
    KilFactoryT4,
    SkobaDrone,
}

impl RangeUnitKind {
    /// Canonical order for kind-indexed capability tables.
    pub const ALL: [Self; 22] = [
        Self::Tank,
        Self::Strider,
        Self::Spider,
        Self::Artillery,
        Self::TurretT1,
        Self::TurretT2,
        Self::Base,
        Self::ArtilleryT2,
        Self::ArtilleryTurretT2,
        Self::ArtilleryTurretT3,
        Self::NuclearSilo,
        Self::Crawler,
        Self::LightningTank,
        Self::EidolonTank,
        Self::Walker04,
        Self::RazlomT3,
        Self::GrebenStorage,
        Self::KilFactoryT1,
        Self::KilFactoryT2,
        Self::KilFactoryT3,
        Self::KilFactoryT4,
        Self::SkobaDrone,
    ];
    pub const COUNT: usize = Self::ALL.len();
    pub const fn index(self) -> usize {
        match self {
            Self::Tank => 0,
            Self::Strider => 1,
            Self::Spider => 2,
            Self::Artillery => 3,
            Self::TurretT1 => 4,
            Self::TurretT2 => 5,
            Self::Base => 6,
            Self::ArtilleryT2 => 7,
            Self::ArtilleryTurretT2 => 8,
            Self::ArtilleryTurretT3 => 9,
            Self::NuclearSilo => 10,
            Self::Crawler => 11,
            Self::LightningTank => 12,
            Self::EidolonTank => 13,
            Self::Walker04 => 14,
            Self::RazlomT3 => 15,
            Self::GrebenStorage => 16,
            Self::KilFactoryT1 => 17,
            Self::KilFactoryT2 => 18,
            Self::KilFactoryT3 => 19,
            Self::KilFactoryT4 => 20,
            Self::SkobaDrone => 21,
        }
    }

    pub const fn emplacement(self) -> bool {
        matches!(
            self,
            Self::TurretT1
                | Self::TurretT2
                | Self::ArtilleryTurretT2
                | Self::ArtilleryTurretT3
                | Self::NuclearSilo
                | Self::Base
                | Self::RazlomT3
                | Self::GrebenStorage
                | Self::KilFactoryT1
                | Self::KilFactoryT2
                | Self::KilFactoryT3
                | Self::KilFactoryT4
        )
    }
    pub const fn canister(self) -> bool {
        matches!(self, Self::ArtilleryT2 | Self::ArtilleryTurretT2)
    }
    pub const fn artillery(self) -> bool {
        matches!(
            self,
            Self::Artillery
                | Self::ArtilleryT2
                | Self::ArtilleryTurretT2
                | Self::ArtilleryTurretT3
                | Self::RazlomT3
        )
    }
}

/// Authored ammunition for vehicles with a selectable main gun.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum RangeLoadout {
    Blast,
    Kinetic,
    Energy,
    Annihilation,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeUnitDef {
    #[serde(default)]
    pub allied: bool,
    pub kind: RangeUnitKind,
    pub at: [i32; 3],
    /// None keeps the kind's stock gun. Fixed weapons cannot be overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<RangeLoadout>,
}
impl RangeUnitDef {
    pub fn valid_loadout(&self) -> bool {
        self.weapon.is_none() || matches!(self.kind, RangeUnitKind::Tank | RangeUnitKind::Artillery)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeBlock {
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub material: u16,
}

/// An authored, resting, reclaimable fragment, in world-cell coordinates.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeDebris {
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub material: u16,
}
/// Smooth elliptical relief. Bounds describe the base and absolute summit height;
/// overlapping mounds take the maximum, rather than adding unbounded elevation.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeMound {
    pub min: [i32; 3],
    pub max: [i32; 3],
}
impl RangeMound {
    pub fn height(&self, x: i32, z: i32) -> Option<i32> {
        if x < self.min[0] || x >= self.max[0] || z < self.min[2] || z >= self.max[2] {
            return None;
        }
        // Fixed-point ellipse and squared falloff give a level foot and rounded crest.
        // All intermediates fit i64 even at the catalogue's maximum dimensions.
        const Q: i64 = 1024;
        let norm = |v: i32, axis: usize| {
            (2 * i64::from(v) - i64::from(self.min[axis]) - i64::from(self.max[axis])).abs() * Q
                / i64::from(self.max[axis] - self.min[axis])
        };
        let nx = norm(x, 0);
        let nz = norm(z, 2);
        let t = (Q * Q - nx * nx - nz * nz).max(0);
        Some(self.min[1] + (i64::from(self.max[1] - self.min[1]) * t * t / (Q * Q * Q * Q)) as i32)
    }
}
/// Ground-level track linking sectors. It follows the natural relief and
/// clears trees, so vehicles can use it without a raised collision slab.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeRoad {
    pub width: i32,
    pub points: Vec<[i32; 2]>,
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
pub struct RangeConstructionOrder {
    pub kind: RangeUnitKind,
    pub at: [i32; 3],
    #[serde(default = "default_construction_quota")]
    pub quota: u8,
}
fn default_construction_quota() -> u8 { 8 }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeProductionOrder {
    pub actor: usize,
    pub kind: RangeUnitKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeReady {
    #[serde(default)]
    pub store: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub construction: Vec<RangeConstructionOrder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub production: Vec<RangeProductionOrder>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeLayout {
    /// Battle actors are the placed roster; the test gun is parked off-scene.
    #[serde(default)]
    pub battle: bool,
    /// Passive hostile targets keep their positions and never acquire orders.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idle_enemies: bool,
    /// Side, height and side of the world this layout is laid out in,
    /// world units (ADR-315). A layout without one keeps the stock
    /// range, so that every preset written before the size moved into
    /// the data goes on meaning what it meant.
    #[serde(default = "stock_size")]
    pub size: [u32; 3],
    /// Absolute level of the flat ground, including destructible subsoil.
    #[serde(default = "stock_ground", skip_serializing_if = "is_stock_ground")]
    pub ground_level: i32,
    #[serde(default)]
    pub course: Option<RangeCourse>,
    #[serde(default = "stock_gun")]
    pub gun: [i32; 3],
    /// A model-free emitter uses `gun` as its exact origin and has no physical chassis.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gun_point: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub laboratory: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_overlap: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lab_devices: Vec<crate::lab_tools::Device>,
    #[serde(default)]
    pub gun_kind: RangeUnitKind,
    #[serde(default)]
    pub zones: Vec<RangeZone>,
    pub target: [i32; 3],
    /// Automatic acquisition radius in world units; manual fire is unchanged.
    #[serde(default = "default_auto_attack_range")]
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub auto_attack_range: i32,
    /// Target-plane fuse makes the with/without-cover comparison reproducible.
    pub airburst_on_target: bool,
    pub clear_min: [i32; 2],
    pub clear_max: [i32; 2],
    pub units: Vec<RangeUnitDef>,
    /// Optional absolute quarter turns; omitted entries keep the side's spawn heading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unit_yaws: Vec<u8>,
    /// Later blocks override earlier blocks, allowing cladding and cutouts.
    pub blocks: Vec<RangeBlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_debris: Vec<RangeDebris>,
    #[serde(default)]
    pub roads: Vec<RangeRoad>,
    /// Optional relief: absent/empty keeps the old layout text and terrain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounds: Vec<RangeMound>,
    #[serde(default)]
    pub shields: Vec<RangeShieldDef>,
    pub power_milli: Vec<u32>,
    pub unit_resistance: u32,
    /// Dig economy of this layout (ADR-672). A layout without one has no
    /// tanks and no dig orders, and reads and writes as it did before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub economy: Option<crate::range_economy::RangeEconomy>,
    /// Ready-to-play stock and jobs, initialized once at scene construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<RangeReady>,
}
fn stock_ground() -> i32 {
    16
}
fn is_stock_ground(v: &i32) -> bool {
    *v == stock_ground()
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
    crate::attack_ranges::stock().acquisition
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
    pub fn unit_yaw(&self, index: usize) -> u8 {
        self.unit_yaws.get(index).copied().unwrap_or_else(|| {
            if self.battle && !self.units[index].allied {
                2
            } else {
                0
            }
        })
    }
    /// Translate the complete authored scene once, preserving sky clearance.
    /// Saved editor layouts already carry their level, so reopening is idempotent.
    pub fn with_ground_level(mut self, level: i32) -> Result<Self, String> {
        self.validate()?;
        if !(16..=272).contains(&level) || (level - 16) % 16 != 0 {
            return Err("invalid range ground level".into());
        }
        let dy = level - self.ground_level;
        if dy == 0 {
            return Ok(self);
        }
        self.size[1] = u32::try_from(i64::from(self.size[1]) + i64::from(dy))
            .map_err(|_| "invalid translated range height")?;
        let shift = |p: &mut [i32; 3]| p[1] += dy;
        shift(&mut self.gun);
        shift(&mut self.target);
        for u in &mut self.units {
            shift(&mut u.at);
        }
        for device in &mut self.lab_devices {
            shift(&mut device.at);
            if let Some(c) = device.collector.as_mut() {
                shift(&mut c.dock);
                shift(&mut c.area);
                if let Some(p) = c.aim.as_mut() {
                    shift(p);
                }
            }
        }
        for b in &mut self.blocks {
            shift(&mut b.min);
            shift(&mut b.max);
        }
        for b in &mut self.initial_debris {
            shift(&mut b.min);
            shift(&mut b.max);
        }
        for m in &mut self.mounds {
            shift(&mut m.min);
            shift(&mut m.max);
        }
        for s in &mut self.shields {
            shift(&mut s.at);
        }
        for z in &mut self.zones {
            shift(&mut z.spawn);
            shift(&mut z.target);
            if let Some(s) = &mut z.scenario {
                for p in &mut s.shots {
                    shift(p);
                }
            }
        }
        if let Some(c) = &mut self.course {
            for r in &mut c.ramps {
                shift(&mut r.min);
                shift(&mut r.max);
            }
        }
        self.ground_level = level;
        self.validate()?;
        Ok(self)
    }
    pub fn to_ron(&self) -> Result<String, String> {
        self.validate()?;
        // The text is part of the authoritative snapshot/hash. RON's
        // default newline depends on the target OS; keep the established
        // Windows bytes on every platform (ADR-676).
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default().new_line("\r\n"))
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
            || !(16..=272).contains(&r.ground_level)
            || (r.ground_level - 16) % 16 != 0
            || r.ground_level >= size[1]
            || r.unit_resistance == 0
            || r.unit_resistance > 1_000_000
            || r.lab_devices.len() > crate::lab_tools::Tuning::stock().devices
            || (!r.laboratory && (!r.lab_devices.is_empty() || r.allow_overlap))
            || r.lab_devices.iter().any(|d| {
                d.turns > 3
                    || (0..3).any(|a| d.at[a] < 1 || d.at[a] >= size[a] - 1)
                    || (d.kind == crate::lab_tools::DeviceKind::Collector) != d.collector.is_some()
                    || d.collector.as_ref().is_some_and(|c| !c.valid(size))
            })
            || r.units.len() > MAX_RANGE_UNITS
            || r.unit_yaws.len() > r.units.len()
            || r.unit_yaws.iter().any(|yaw| *yaw > 3)
            || r.units.iter().any(|u| !u.valid_loadout())
            || !(1..=100_000).contains(&r.auto_attack_range)
            || r.blocks.len() > MAX_RANGE_BLOCKS
            || r.initial_debris.len() > 128
            || r.initial_debris.iter().enumerate().any(|(i, b)| {
                ![2, 3, 5, 6].contains(&b.material)
                    || b.min[1] != r.ground_level
                    || (0..3).any(|a| {
                        b.min[a] <= 0
                            || b.max[a] >= size[a]
                            || b.max[a] <= b.min[a]
                            || b.max[a] - b.min[a] > 4
                    })
                    || r.initial_debris[..i].iter().any(|other| {
                        (0..3).all(|a| b.min[a] < other.max[a] && other.min[a] < b.max[a])
                    })
            })
            || r.mounds.len() > 16
            || r.mounds
                .iter()
                .any(|m| (0..3).any(|a| m.min[a] < 0 || m.max[a] > size[a] || m.min[a] >= m.max[a]))
            || r.roads.len() > 16
            || r.roads.iter().any(|road| {
                !(16..=96).contains(&road.width)
                    || !(2..=24).contains(&road.points.len())
                    || road
                        .points
                        .iter()
                        .any(|p| p[0] < 0 || p[0] >= size[0] || p[1] < 0 || p[1] >= size[2])
                    || road.points.windows(2).any(|pair| pair[0] == pair[1])
            })
            || r.power_milli.is_empty()
            || r.power_milli.len() > 8
            || r.power_milli.iter().any(|p| !(100..=4000).contains(p))
            || (r.target[0] <= 160 && !r.gun_point)
            || (0..3).any(|a| r.target[a] < 0 || r.target[a] >= size[a])
            || (0..2).any(|a| {
                r.clear_min[a] < 0
                    || r.clear_max[a] > size[a * 2]
                    || r.clear_min[a] >= r.clear_max[a]
            })
            || (r
                .blocks
                .iter()
                .any(|b| b.material == crate::water::MATERIAL)
                && r.blocks
                    .iter()
                    .filter(|b| b.material == crate::water::MATERIAL)
                    .map(|b| {
                        (0..3)
                            .map(|a| u64::try_from(b.max[a] - b.min[a]).unwrap_or(u64::MAX))
                            .fold(1u64, |v, n| v.saturating_mul(n))
                    })
                    .fold(0u64, |v, n| v.saturating_add(n))
                    > crate::water::Tuning::stock().max_cells as u64)
            || r.blocks.iter().any(|b| {
                ![0, 1, 2, 3, 4, 5, 6, 8, 10, 11, 12].contains(&b.material)
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
            || r.ready.as_ref().is_some_and(|ready| {
                ready.construction.len() > 8
                    || ready.construction.iter().any(|order| {
                        !matches!(order.kind, RangeUnitKind::TurretT1 | RangeUnitKind::KilFactoryT1 | RangeUnitKind::GrebenStorage)
                            || !(1..=32).contains(&order.quota)
                            || (0..3).any(|a| order.at[a] < 0 || order.at[a] >= size[a])
                    })
                    || ready.production.iter().any(|order| {
                        order.actor >= r.units.len()
                            || r.economy.as_ref().is_none_or(|economy| !economy.produces(r.units[order.actor].kind, order.kind))
                    })
                    || (!ready.construction.is_empty() && r.economy.is_none())
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
        if let Some(e) = &r.economy {
            e.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rounded_mounds_are_bounded_symmetric_and_old_layouts_omit_them() {
        let old = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        assert!(old.mounds.is_empty());
        assert!(!old.to_ron().unwrap().contains("mounds"));
        let m = RangeMound {
            min: [0, 16, 0],
            max: [16384, 192, 16384],
        };
        assert_eq!(m.height(8192, 8192), Some(192));
        assert_eq!(m.height(0, 8192), Some(16));
        assert_eq!(m.height(-1, 8192), None);
        assert_eq!(m.height(16384, 8192), None);
        for x in (1..8192).step_by(31) {
            assert_eq!(m.height(x, 8192), m.height(16384 - x, 8192));
            assert_eq!(m.height(8192, x), m.height(x, 8192));
        }
        let mut layout = old;
        layout.mounds = vec![RangeMound {
            min: [80, 16, 80],
            max: [400, 120, 400],
        }];
        assert_eq!(
            RangeLayout::from_ron(&layout.to_ron().unwrap()).unwrap(),
            layout
        );
        layout.mounds[0].max[1] = layout.size[1] as i32 + 1;
        assert!(layout.validate().is_err());
        layout.mounds[0].max[1] = 120;
        layout.mounds[0].max[0] = layout.mounds[0].min[0];
        assert!(layout.validate().is_err());
        layout.mounds = vec![m; 17];
        assert!(layout.validate().is_err());
    }

    #[test]
    fn crawler_field_debris_survives_scene_roundtrip_and_ground_translation() {
        let original =
            RangeLayout::from_ron(include_str!("../../../data/range-crawler-field.ron")).unwrap();
        assert!(!original.initial_debris.is_empty());
        assert_eq!(
            RangeLayout::from_ron(&original.to_ron().unwrap()).unwrap(),
            original
        );
        let shifted = original.clone().with_ground_level(80).unwrap();
        assert_eq!(
            shifted.initial_debris[0].min[1],
            original.initial_debris[0].min[1] + 64
        );
        assert_eq!(shifted.with_ground_level(16).unwrap(), original);
        let mut invalid = original.clone();
        invalid
            .initial_debris
            .push(invalid.initial_debris[0].clone());
        assert!(
            invalid.validate().is_err(),
            "overlapping fragments must be rejected"
        );
        invalid = original;
        invalid.initial_debris[0].max[0] += 1;
        assert!(
            invalid.validate().is_err(),
            "oversized fragment must be rejected"
        );
    }

    #[test]
    fn snapshot_layout_text_is_canonical_across_input_line_endings() {
        let source = include_str!("../../../data/range-km.ron").replace("\r\n", "\n");
        let lf = RangeLayout::from_ron(&source).unwrap().to_ron().unwrap();
        let crlf = RangeLayout::from_ron(&source.replace('\n', "\r\n"))
            .unwrap()
            .to_ron()
            .unwrap();
        assert_eq!(lf, crlf);
        assert!(lf.contains("\r\n"));
        assert!(!lf.replace("\r\n", "").contains('\n'));
        assert_eq!(RangeLayout::from_ron(&lf).unwrap().to_ron().unwrap(), lf);
    }
    #[test]
    fn stock_scenarios_are_bounded_sequences_in_their_sectors() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        assert_eq!(
            layout.zones.iter().filter(|z| z.scenario.is_some()).count(),
            7
        );
        assert!(layout.zones[9].scenario.as_ref().unwrap().patrols);
        assert_eq!(layout.zones[11].scenario.as_ref().unwrap().shots.len(), 2);
        assert_eq!(layout.zones[12].scenario.as_ref().unwrap().shots.len(), 2);
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
        r = good.clone();
        r.blocks = vec![r.blocks[0].clone(); MAX_RANGE_BLOCKS + 1];
        assert!(r.validate().is_err());
        // Sixteen machines a side pair may place; a seventeenth is refused
        // by the catalogue before any placement is checked (ADR-466).
        r = good;
        r.units = vec![r.units[0].clone(); MAX_RANGE_UNITS];
        assert!(r.validate().is_ok());
        r.units.push(r.units[0].clone());
        assert!(r.validate().is_err());
    }
}

#[cfg(test)]
mod subsoil_tests {
    use super::*;
    #[test]
    fn complete_catalogue_translation_is_reversible_and_survives_editor_save() {
        let stock = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        let deep = stock.clone().with_ground_level(80).unwrap();
        assert_eq!(deep.size[1], stock.size[1] + 64);
        assert_eq!(deep.gun[1], stock.gun[1] + 64);
        assert_eq!(
            deep.zones[0].scenario.as_ref().unwrap().shots[0][1],
            stock.zones[0].scenario.as_ref().unwrap().shots[0][1] + 64
        );
        assert_eq!(
            deep,
            RangeLayout::from_ron(&deep.to_ron().unwrap()).unwrap()
        );
        assert_eq!(stock, deep.with_ground_level(16).unwrap());
        assert!(!stock.to_ron().unwrap().contains("ground_level"));
        assert!(stock.clone().with_ground_level(81).is_err());
    }
}
