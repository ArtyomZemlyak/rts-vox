//! Parameters of the blast range and its reusable explosion systems.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum ProjectileMotion {
    #[default]
    Ballistic,
    Direct,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Delivery {
    #[default]
    Blast,
    Kinetic,
    Laser,
    Flame,
    MachineGun,
}

/// Optional material-specific deflection for a solid projectile. All
/// percentages are thousandths, matching other weapon tuning values.
#[derive(Clone, Debug, Deserialize)]
pub struct Ricochet {
    pub material: u16,
    pub min_speed: i32,
    pub max_normal_milli: u32,
    pub wear_milli: u32,
    pub speed_retention_milli: u32,
    pub max_preexisting_wear_milli: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExplosionConfig {
    #[serde(default)]
    pub delivery: Delivery,
    #[serde(default)]
    pub motion: ProjectileMotion,
    #[serde(default)]
    pub charge_ticks: u32,
    pub half_cells: usize,
    pub cell_units: i32,
    pub energy: u32,
    /// Zero is explicit annihilation; otherwise fracture and fragment motion
    /// share this finite budget, independently of the propagation envelope.
    #[serde(default)]
    pub material_energy_j: u32,
    pub air_cost: u32,
    pub soil_resistance: u32,
    pub concrete_resistance: u32,
    pub steel_resistance: u32,
    /// Anti-matter ablation of the range unit; zero disables the witness unit.
    #[serde(default)]
    pub unit_ablation_resistance: u32,
    /// Armour, core, engine, weapon, sensor multipliers; 1000 = base work.
    #[serde(default = "unit_roles")]
    pub unit_role_resistance_milli: [u32; 5],
    pub wave_work_per_tick: usize,
    /// Cells of the resistance grid captured in one tick (ADR-311). The
    /// capture of a charge that reaches further costs more ticks, not a
    /// longer one; cells the inscribed sphere cuts are free and do not
    /// count. Presets from before the ceiling keep the whole grid of the
    /// stock weapons in one tick.
    #[serde(default = "capture_cells")]
    pub capture_cells_per_tick: usize,
    pub edits_per_tick: usize,
    pub fire_tick: u32,
    pub shell_speed: i32,
    #[serde(default)]
    pub ricochet: Option<Ricochet>,
    pub gravity: i32,
    pub gas_cell_units: i32,
    /// Visible gas cube edge, in millimetres. Independent of the solver grid.
    #[serde(default = "display_cell")]
    pub gas_display_mm: u32,
    pub gas_source_radius: i32,
    pub gas_temperature_k: i32,
    pub gas_fuel_milli: i32,
    /// Authoritative heat left by the burst (ADR-277): every solid cell
    /// within this many units of the impact receives `heat_per_cell`.
    #[serde(default)]
    pub heat_units: i32,
    #[serde(default)]
    pub heat_per_cell: u32,
    pub gas_pressure_milli: i32,
    pub reaction_milli: i32,
    pub cooling_milli: i32,
    pub buoyancy_milli: i32,
    /// Cells per side of the box this weapon's debris pieces are grouped
    /// in, `1..=4` (ADR-284). Older presets keep the two-cell box.
    #[serde(default = "two_cells")]
    pub fragment_box: u32,
    /// Push on a hull, thousandths of a newton-second per unit of wave
    /// pressure the body absorbed (ADR-286, ADR-297): the pressure that
    /// dents and breaks the side is what throws the machine. Zero: the
    /// burst does not move hulls.
    #[serde(default)]
    pub hull_impulse_milli: u32,
}

fn display_cell() -> u32 {
    320
}
fn two_cells() -> u32 {
    2
}
fn capture_cells() -> usize {
    40000
}
fn unit_roles() -> [u32; 5] {
    [1000; 5]
}

impl ExplosionConfig {
    /// Visual source volume follows charge strength, so its radius scales
    /// with the cube root rather than filling the fixed gas window at 4x.
    pub fn gas_radius_at_power(&self, power_milli: u32) -> i32 {
        let radius = self.gas_source_radius.clamp(1, 128) as u128;
        let volume = radius * radius * radius * u128::from(power_milli);
        // Find the first integral radius whose volume reaches the scaled
        // source. Compare against the half-radius cube to round exactly.
        let mut low = 1u128;
        let mut high = 128u128;
        while low < high {
            let mid = (low + high) / 2;
            if mid * mid * mid * 1000 >= volume {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        let halfway = 2 * low - 1;
        let rounded = if volume * 8 >= halfway * halfway * halfway * 1000 {
            low
        } else {
            low.saturating_sub(1)
        };
        rounded.clamp(1, 128) as i32
    }

    pub fn from_ron(text: &str) -> Result<Self, String> {
        let c: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=32).contains(&c.half_cells)
            || c.charge_ticks > c.fire_tick
            || !(1..=4).contains(&c.cell_units)
            || !(1..=1_000_000).contains(&c.energy)
            || c.material_energy_j > 1_000_000_000
            || c.air_cost != 100
            || c.unit_ablation_resistance > 1_000_000
            || [c.soil_resistance, c.concrete_resistance, c.steel_resistance]
                .iter()
                .any(|v| !(1..=1_000_000).contains(v))
            || c.unit_role_resistance_milli
                .iter()
                .any(|v| !(1..=10000).contains(v))
            || c.wave_work_per_tick == 0
            || c.wave_work_per_tick > 8192
            || c.capture_cells_per_tick == 0
            || c.capture_cells_per_tick > 1_000_000
            || c.edits_per_tick == 0
            || c.edits_per_tick > 65536
            || !(1..=10000).contains(&c.shell_speed)
            || c.ricochet.as_ref().is_some_and(|r| {
                c.delivery != Delivery::Kinetic
                    || !(1..=15).contains(&r.material)
                    || !(1..=10000).contains(&r.min_speed)
                    || !(1..=999).contains(&r.max_normal_milli)
                    || !(1..=999).contains(&r.wear_milli)
                    || !(1..=999).contains(&r.speed_retention_milli)
                    || r.max_preexisting_wear_milli > 1000
            })
            || !(1..=10000).contains(&c.gravity)
            || !(1..=8).contains(&c.gas_cell_units)
            || !(100..=1600).contains(&c.gas_display_mm)
            || !(1..=128).contains(&c.gas_source_radius)
            || !(300..=6000).contains(&c.gas_temperature_k)
            || !(0..=10000).contains(&c.gas_fuel_milli)
            || !(0..=10000).contains(&c.gas_pressure_milli)
            || !(0..=10000).contains(&c.reaction_milli)
            || !(0..=10000).contains(&c.cooling_milli)
            || !(0..=10000).contains(&c.buoyancy_milli)
            || !(0..=32).contains(&c.heat_units)
            || c.heat_per_cell > 100_000
            || !(1..=4).contains(&c.fragment_box)
            || c.hull_impulse_milli > 100_000
        {
            return Err("explosion parameters exceed supported bounds".into());
        }
        Ok(c)
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;
    #[test]
    fn display_scale_is_optional_bounded_and_independent_of_solver_grid() {
        let text = include_str!("../../../data/explosion.ron");
        let base = ExplosionConfig::from_ron(text).unwrap();
        let old = text.replace("gas_display_mm: 640,", "");
        let old = ExplosionConfig::from_ron(&old).unwrap();
        assert_eq!(old.gas_display_mm, 320);
        assert_eq!(old.gas_cell_units, base.gas_cell_units);
        for mm in [0, 99, 1601, u32::MAX] {
            assert!(ExplosionConfig::from_ron(
                &text.replace("gas_display_mm: 640", &format!("gas_display_mm: {mm}"))
            )
            .is_err());
        }
    }

    #[test]
    fn visual_gas_radius_scales_with_charge_volume() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        assert_eq!(c.gas_source_radius, 32);
        assert_eq!(
            [500, 1000, 2000, 4000].map(|power| c.gas_radius_at_power(power)),
            [25, 32, 40, 51]
        );
    }

    #[test]
    fn visual_gas_radius_rounds_at_half_cells_and_stays_bounded() {
        let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        c.gas_source_radius = 8;
        assert_eq!(c.gas_radius_at_power(125), 4);
        assert_eq!(c.gas_radius_at_power(177), 4);
        assert_eq!(c.gas_radius_at_power(178), 5);
        assert_eq!(c.gas_radius_at_power(0), 1);
        assert_eq!(c.gas_radius_at_power(u32::MAX), 128);
    }

    #[test]
    fn ricochet_tuning_is_optional_and_rejects_invalid_energy_or_angle() {
        let solid = include_str!("../../../data/kinetic-shell.ron");
        let c = ExplosionConfig::from_ron(solid).unwrap();
        assert_eq!(c.ricochet.as_ref().unwrap().material, 3);
        assert!(
            ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
                .unwrap()
                .ricochet
                .is_none()
        );
        for bad in [
            solid.replace("max_normal_milli: 400", "max_normal_milli: 0"),
            solid.replace("speed_retention_milli: 600", "speed_retention_milli: 1000"),
            solid.replace("material: 3", "material: 16"),
        ] {
            assert!(ExplosionConfig::from_ron(&bad).is_err());
        }
    }
}
