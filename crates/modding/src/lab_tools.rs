//! Authoring tools use the same world/edit budget as simulation damage.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum DeviceKind {
    Drill,
    Crawler,
    Collector,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Device {
    pub kind: DeviceKind,
    pub at: [i32; 3],
    pub turns: u8,
    #[serde(default)]
    pub work: u32,
    #[serde(default)]
    pub dug: u64,
    #[serde(default)]
    pub container: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collector: Option<Collector>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Collector {
    pub dock: [i32; 3],
    pub area: [i32; 3],
    pub capacity: u64,
    pub depot: Option<u32>,
    pub target: Option<u64>,
    pub aim: Option<[i32; 3]>,
    pub spent: u64,
    pub returning: bool,
    pub picked: u64,
    pub delivered: u64,
    pub pieces: u64,
    pub trips: u64,
    pub cursor: usize,
    pub searched: usize,
    pub route: Vec<[i32; 2]>,
    pub waypoint: usize,
    pub leg_start: [i32; 2],
    pub leg_step: u32,
    pub plan_tick: u32,
    pub rejected: Option<(u64, u32)>,
}
impl Default for Collector {
    fn default() -> Self {
        Self {
            dock: [0; 3],
            area: [0; 3],
            capacity: Tuning::stock().collector_capacity,
            depot: None,
            target: None,
            aim: None,
            spent: 0,
            returning: false,
            picked: 0,
            delivered: 0,
            pieces: 0,
            trips: 0,
            cursor: 0,
            searched: 0,
            route: Vec::new(),
            waypoint: 0,
            leg_start: [0; 2],
            leg_step: 0,
            plan_tick: 0,
            rejected: None,
        }
    }
}
impl Collector {
    pub fn at(at: [i32; 3]) -> Self {
        Self {
            dock: at,
            area: at,
            ..Self::default()
        }
    }
    pub fn valid(&self, size: [i32; 3]) -> bool {
        self.capacity > 0
            && self.capacity <= 1_000_000_000
            && self.route.len() <= 4096
            && self.waypoint <= self.route.len()
            && self.leg_step <= size[0].max(size[2]) as u32
            && self
                .aim
                .is_none_or(|p| (0..3).all(|a| p[a] >= 0 && p[a] < size[a]))
            && [self.dock, self.area]
                .iter()
                .all(|p| (0..3).all(|a| p[a] > 0 && p[a] < size[a] - 1))
            && self
                .route
                .iter()
                .all(|p| p[0] > 0 && p[0] < size[0] - 1 && p[1] > 0 && p[1] < size[2] - 1)
    }
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Tuning {
    pub brush_edge: u32,
    pub brush_cells: usize,
    pub devices: usize,
    pub device_edge: i32,
    pub device_work: u32,
    pub device_checks: usize,
    pub movement_ticks: u32,
    pub collector_capacity: u64,
    pub collector_depot: u64,
    pub collector_radius: i32,
    pub collector_reach: i32,
    pub collector_half: i32,
    pub collector_height: i32,
    pub collector_step: i32,
    pub collector_work: u32,
    pub collector_scans: usize,
    pub collector_plans: usize,
    pub collector_nodes: usize,
    pub collector_move_ticks: u32,
    pub collector_retry_ticks: u32,
    pub collector_unload: u64,
    pub collector_refine_num: u64,
    pub collector_refine_den: u64,
}
impl Tuning {
    pub fn stock() -> Self {
        Self::from_ron(include_str!("../../../data/lab-tools.ron")).expect("lab tools")
    }
    pub fn from_ron(s: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(s).map_err(|e| e.to_string())?;
        if !(1..=16).contains(&t.brush_edge)
            || !(1..=4096).contains(&t.brush_cells)
            || !(1..=32).contains(&t.devices)
            || !(1..=7).contains(&t.device_edge)
            || t.device_edge % 2 == 0
            || !(1..=1_000_000).contains(&t.device_work)
            || !(1..=4096).contains(&t.device_checks)
            || !(1..=120).contains(&t.movement_ticks)
            || t.collector_capacity == 0
            || t.collector_depot == 0
            || !(8..=512).contains(&t.collector_radius)
            || !(1..=64).contains(&t.collector_reach)
            || !(1..=12).contains(&t.collector_half)
            || !(1..=32).contains(&t.collector_height)
            || !(0..=8).contains(&t.collector_step)
            || !(1..=100_000).contains(&t.collector_work)
            || !(1..=4096).contains(&t.collector_scans)
            || !(1..=8).contains(&t.collector_plans)
            || !(32..=4096).contains(&t.collector_nodes)
            || !(1..=120).contains(&t.collector_move_ticks)
            || !(1..=1200).contains(&t.collector_retry_ticks)
            || t.collector_unload == 0
            || t.collector_refine_num == 0
            || t.collector_refine_num > t.collector_refine_den
            || t.collector_refine_den > 1000
        {
            return Err("invalid laboratory tool limits".into());
        }
        Ok(t)
    }
}
