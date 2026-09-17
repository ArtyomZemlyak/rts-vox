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
    pub gravity: i32,
    pub gas_cell_units: i32,
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
            || !(1..=10000).contains(&c.gravity)
            || !(1..=8).contains(&c.gas_cell_units)
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
