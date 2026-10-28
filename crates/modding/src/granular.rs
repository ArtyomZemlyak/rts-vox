//! Bounded cellular material flow. Geometry remains on the unchanged world grid.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Material {
    pub id: u16,
    /// Minimum empty drop beside a supported cell before it slumps.
    pub repose_drop: u32,
    /// Contact pressure needed to displace this material.
    pub yield_pressure: u32,
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Erosion {
    pub checks_per_tick: usize,
    pub moves_per_tick: usize,
    pub min_flow_units: u32,
    pub saturation_milli: u32,
    pub pressure_per_voxel: u32,
}
impl Default for Erosion {
    fn default() -> Self {
        Self {
            checks_per_tick: 128,
            moves_per_tick: 4,
            min_flow_units: 256,
            saturation_milli: 500,
            pressure_per_voxel: 4800,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    pub scans_per_tick: u32,
    pub moves_per_tick: u32,
    pub contacts_per_tick: u32,
    #[serde(default = "contact_moves")]
    pub contact_moves_per_tick: u32,
    pub push_reach: u32,
    pub materials: Vec<Material>,
    #[serde(default)]
    pub erosion: Erosion,
    #[serde(default)]
    pub motion: Motion,
    #[serde(default)]
    pub slope: Slope,
}
/// Compact cohesive bodies can descend a loose surface without becoming terrain
/// at their first contact. These limits apply to the whole phase, not each body.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Slope {
    pub checks_per_tick: u32,
    pub max_cells: usize,
    pub reach: u32,
    /// Maximum real sand cells displaced by one downhill step.
    #[serde(default = "scuff_moves")]
    pub scuff_moves_per_step: u32,
    /// Roll begins when the supported fraction falls below this threshold.
    #[serde(default = "roll_support")]
    pub roll_support_milli: u32,
    /// Binary turns per travelled cell at a one-cell radius.
    #[serde(default = "roll_turn")]
    pub roll_turn_per_cell: u32,
}
fn scuff_moves() -> u32 {
    8
}
fn roll_support() -> u32 {
    750
}
fn roll_turn() -> u32 {
    10430
}
impl Default for Slope {
    fn default() -> Self {
        Self {
            checks_per_tick: 32768,
            max_cells: 1024,
            reach: 4,
            scuff_moves_per_step: scuff_moves(),
            roll_support_milli: roll_support(),
            roll_turn_per_cell: roll_turn(),
        }
    }
}
fn contact_moves() -> u32 {
    512
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Motion {
    pub active_cells: usize,
    pub active_per_tick: usize,
    pub path_checks_per_tick: u32,
    pub max_steps: u32,
}
impl Default for Motion {
    fn default() -> Self {
        Self {
            active_cells: 8192,
            active_per_tick: 2048,
            path_checks_per_tick: 32768,
            max_steps: 8,
        }
    }
}
impl Tuning {
    pub fn stock() -> Self {
        Self::from_ron(include_str!("../../../data/range-granular.ron")).expect("granular tuning")
    }
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=65536).contains(&t.scans_per_tick)
            || !(1..=4096).contains(&t.moves_per_tick)
            || !(1..=4096).contains(&t.contacts_per_tick)
            || !(1..=4096).contains(&t.contact_moves_per_tick)
            || !(1..=16).contains(&t.push_reach)
            || !(1..=65536).contains(&t.motion.active_cells)
            || !(1..=4096).contains(&t.motion.active_per_tick)
            || !(128..=262144).contains(&t.motion.path_checks_per_tick)
            || !(1..=32).contains(&t.motion.max_steps)
            || !(1024..=65536).contains(&t.slope.checks_per_tick)
            || !(1..=1024).contains(&t.slope.max_cells)
            || !(1..=4).contains(&t.slope.reach)
            || !(1..=32).contains(&t.slope.scuff_moves_per_step)
            || !(1..=1000).contains(&t.slope.roll_support_milli)
            || !(1..=16384).contains(&t.slope.roll_turn_per_cell)
            || !(1..=4096).contains(&t.erosion.checks_per_tick)
            || t.erosion.moves_per_tick > 64
            || !(1..=65536).contains(&t.erosion.min_flow_units)
            || !(1..=1000).contains(&t.erosion.saturation_milli)
            || !(1..=1_000_000).contains(&t.erosion.pressure_per_voxel)
            || t.materials.is_empty()
            || t.materials.len() > 16
            || t.materials.iter().enumerate().any(|(i, m)| {
                m.id == 0
                    || !(1..=16).contains(&m.repose_drop)
                    || m.yield_pressure == 0
                    || t.materials[..i].iter().any(|p| p.id == m.id)
            })
        {
            return Err("invalid granular tuning".into());
        }
        Ok(t)
    }
    pub fn material(&self, id: u16) -> Option<Material> {
        self.materials.iter().find(|m| m.id == id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn granular_stock_has_distinct_sand_and_cohesive_soils_and_bounded_work() {
        let t = Tuning::stock();
        assert!(t.material(10).unwrap().repose_drop < t.material(1).unwrap().repose_drop);
        // Exercise the actual external validator, including duplicate IDs.
        let text = include_str!("../../../data/range-granular.ron")
            .replace("push_reach: 8", "push_reach: 0");
        assert!(Tuning::from_ron(&text).is_err());
        let text = include_str!("../../../data/range-granular.ron").replace("(id:1,", "(id:10,");
        assert!(Tuning::from_ron(&text).is_err());
    }
    #[test]
    fn granular_motion_limits_and_legacy_defaults_are_validated() {
        let text = include_str!("../../../data/range-granular.ron");
        for (from, to) in [
            ("active_cells:8192", "active_cells:0"),
            ("active_per_tick:2048", "active_per_tick:4097"),
            ("path_checks_per_tick:32768", "path_checks_per_tick:127"),
            ("max_steps:8", "max_steps:33"),
            ("contact_moves_per_tick:512", "contact_moves_per_tick:0"),
            (
                "checks_per_tick:32768,max_cells",
                "checks_per_tick:0,max_cells",
            ),
            ("max_cells:1024", "max_cells:1025"),
            ("reach:4", "reach:5"),
        ] {
            assert!(Tuning::from_ron(&text.replace(from, to)).is_err(), "{from}");
        }
        let old: String = text
            .lines()
            .filter(|line| !line.contains("motion:") && !line.contains("contact_moves_per_tick:"))
            .collect::<Vec<_>>()
            .join("\n");
        let old = Tuning::from_ron(&old).unwrap();
        assert_eq!(old.motion.max_steps, 8);
        assert_eq!(old.contact_moves_per_tick, 512);
    }
}

#[cfg(test)]
mod erosion_tests {
    use super::*;
    #[test]
    fn erosion_defaults_limits_and_disabled_mode_are_validated() {
        let text = include_str!("../../../data/range-granular.ron");
        assert_eq!(Tuning::from_ron(text).unwrap().erosion.moves_per_tick, 4);
        assert!(Tuning::from_ron(&text.replace("moves_per_tick:4", "moves_per_tick:0")).is_ok());
        for (from, to) in [
            ("checks_per_tick:128", "checks_per_tick:0"),
            ("moves_per_tick:4", "moves_per_tick:65"),
            ("saturation_milli:500", "saturation_milli:1001"),
        ] {
            assert!(Tuning::from_ron(&text.replace(from, to)).is_err());
        }
        let begin = text.find(" erosion:").unwrap();
        let end = begin + text[begin..].find("),").unwrap() + 2;
        let mut old = text.to_owned();
        old.replace_range(begin..end, "");
        assert_eq!(Tuning::from_ron(&old).unwrap().erosion.checks_per_tick, 128);
    }
}
