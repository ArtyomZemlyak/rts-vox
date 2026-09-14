//! Parameters of the blast range and its reusable explosion systems.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum ProjectileMotion {
    #[default]
    Ballistic,
    Direct,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExplosionConfig {
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
    pub edits_per_tick: usize,
    pub fire_tick: u32,
    pub shell_speed: i32,
    pub gravity: i32,
    pub gas_cell_units: i32,
    pub gas_source_radius: i32,
    pub gas_temperature_k: i32,
    pub gas_fuel_milli: i32,
    pub gas_pressure_milli: i32,
    pub reaction_milli: i32,
    pub cooling_milli: i32,
    pub buoyancy_milli: i32,
}

fn unit_roles() -> [u32; 5] {
    [1000; 5]
}

impl ExplosionConfig {
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
            || c.edits_per_tick == 0
            || c.edits_per_tick > 65536
            || !(1..=10000).contains(&c.shell_speed)
            || !(1..=10000).contains(&c.gravity)
            || !(1..=8).contains(&c.gas_cell_units)
            || !(1..=128).contains(&c.gas_source_radius)
            || !(300..=6000).contains(&c.gas_temperature_k)
            || !(0..=10000).contains(&c.gas_fuel_milli)
            || !(0..=10000).contains(&c.gas_pressure_milli)
            || !(0..=10000).contains(&c.reaction_milli)
            || !(0..=10000).contains(&c.cooling_milli)
            || !(0..=10000).contains(&c.buoyancy_milli)
        {
            return Err("explosion parameters exceed supported bounds".into());
        }
        Ok(c)
    }
}
