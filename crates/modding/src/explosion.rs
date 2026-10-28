//! Parameters of the blast range and its reusable explosion systems.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum ProjectileMotion {
    #[default]
    Ballistic,
    Direct,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Ricochet {
    pub material: u16,
    pub min_speed: i32,
    pub max_normal_milli: u32,
    pub wear_milli: u32,
    pub speed_retention_milli: u32,
    pub max_preexisting_wear_milli: u32,
}

/// Finite lateral work carried by fragments behind a penetrated surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Spall {
    pub radius: u8,
    pub work_milli: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SoilCrater {
    pub spread: u32,
    pub step: u32,
    pub layer: u32,
    pub deep_half_width: u32,
    pub middle_half_width: u32,
    pub middle_depth: u32,
    pub outer_depth: u32,
    pub taper_units: u32,
}

impl SoilCrater {
    pub fn valid(&self) -> bool {
        (1..=32).contains(&self.spread)
            && (1..=64).contains(&self.step)
            && (1..=64).contains(&self.layer)
            && self.deep_half_width >= self.step
            && self.middle_half_width >= self.deep_half_width
            && self.middle_half_width <= 4096
            && (1..=1_000_000).contains(&self.middle_depth)
            && (1..=self.middle_depth).contains(&self.outer_depth)
            && self.middle_depth % self.layer == 0
            && self.outer_depth % self.layer == 0
            && (1..=4096).contains(&self.taper_units)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
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
    /// Required: a forgotten field must not select annihilation (ADR-429).
    pub material_energy_j: u32,
    pub air_cost: u32,
    pub soil_resistance: u32,
    /// Extra soil work per world unit below the soil surface or buried impact.
    /// Zero retains the isotropic legacy preset; vegetation and body work
    /// are independent.
    #[serde(default)]
    pub soil_depth_cost: u32,
    /// Optional square terraces; absence retains the legacy nuclear bowl.
    #[serde(default)]
    pub soil_crater: Option<SoilCrater>,
    /// Soil fracture price for this weapon only; 1000 keeps material work.
    #[serde(default = "normal_soil_fracture")]
    pub soil_fracture_milli: u32,
    pub concrete_resistance: u32,
    pub steel_resistance: u32,
    /// Optional explicit work for terrain material IDs0..8. Absent retains
    /// the legacy soil/concrete/steel groups; other IDs retain steel work.
    #[serde(default)]
    pub material_resistance: Option<[u32; 9]>,
    /// Anti-matter ablation of the range unit; zero disables the witness unit.
    #[serde(default)]
    pub unit_ablation_resistance: u32,
    /// Armour, core, engine, weapon, sensor multipliers; 1000 = base work.
    pub unit_role_resistance_milli: [u32; 5],
    /// Frontier heap pops, and contacts visited, by one wave in one tick
    /// (ADR-681): each wave's own, not a share of the tick's. The tick's
    /// ceiling over all waves lives in the battle tuning.
    pub wave_work_per_tick: usize,
    /// Cells of the resistance grid one wave captures in one tick
    /// (ADR-311, ADR-681). The capture of a charge that reaches further
    /// costs more ticks, not a longer one; cells the inscribed sphere cuts
    /// are free and do not count.
    pub capture_cells_per_tick: usize,
    pub edits_per_tick: usize,
    pub fire_tick: u32,
    pub shell_speed: i32,
    #[serde(default)]
    pub ricochet: Option<Ricochet>,
    #[serde(default)]
    pub spall: Option<Spall>,
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
    pub heat_units: i32,
    pub heat_per_cell: u32,
    pub gas_pressure_milli: i32,
    pub reaction_milli: i32,
    pub cooling_milli: i32,
    pub buoyancy_milli: i32,
    /// Cells per side of the box this weapon's debris pieces are grouped
    /// in, `1..=4` (ADR-284).
    pub fragment_box: u32,
    /// Push on a hull, thousandths of a newton-second per unit of wave
    /// pressure the body absorbed (ADR-286, ADR-297): the pressure that
    /// dents and breaks the side is what throws the machine. Zero: the
    /// burst does not move hulls.
    pub hull_impulse_milli: u32,
}

impl ExplosionConfig {
    pub const MAX_MATERIAL_ENERGY_J: u32 = 4_000_000_000;

    /// Keep the pressure grid bounded in cell count, not in world radius.
    /// Above the former heavy-charge reference, equal-overpressure distances
    /// grow with the cube root of yield. Exact contacts refine every voxel.
    pub fn propagation_cell_units(&self) -> i32 {
        if self.energy <= 40_000 {
            return self.cell_units;
        }
        let target = u64::from(self.energy) * 4 * 4 * 4;
        let mut edge = self.cell_units;
        while (edge as u64).pow(3) * 40_000 < target {
            edge += 1;
        }
        edge
    }

    /// Scale the finite charge without narrowing an overflowing u64 product.
    /// Keep live tuning within the same bounds as data and restored configs.
    pub fn at_power(mut self, power_milli: u32) -> Self {
        self.energy =
            (u64::from(self.energy) * u64::from(power_milli) / 1000).clamp(1, 1_000_000) as u32;
        self.material_energy_j = (u64::from(self.material_energy_j) * u64::from(power_milli) / 1000)
            .min(u64::from(Self::MAX_MATERIAL_ENERGY_J)) as u32;
        self
    }
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
        c.validate()?;
        Ok(c)
    }

    /// Shared bounds for authored data, live commands and applied snapshots.
    pub fn validate(&self) -> Result<(), String> {
        let c = self;
        if !(1..=32).contains(&c.half_cells)
            || c.charge_ticks > c.fire_tick
            || !(1..=64).contains(&c.cell_units)
            || !(1..=1_000_000).contains(&c.energy)
            || c.material_energy_j > Self::MAX_MATERIAL_ENERGY_J
            || c.air_cost != 100
            || c.soil_depth_cost > 1_000_000
            || c.soil_crater.is_some_and(|shape| !shape.valid())
            || !(1000..=100_000).contains(&c.soil_fracture_milli)
            || c.unit_ablation_resistance > 1_000_000
            || c.material_resistance.is_some_and(|work| {
                work[0] != 0 || work[1..].iter().any(|v| !(1..=1_000_000).contains(v))
            })
            || [c.soil_resistance, c.concrete_resistance, c.steel_resistance]
                .iter()
                .any(|v| !(1..=1_000_000).contains(v))
            || c.unit_role_resistance_milli
                .iter()
                .any(|v| !(1..=10000).contains(v))
            || c.wave_work_per_tick == 0
            || c.wave_work_per_tick > 1 << 20
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
            || c.spall.as_ref().is_some_and(|s| {
                c.delivery != Delivery::Kinetic
                    || !(1..=4).contains(&s.radius)
                    || !(1..=900).contains(&s.work_milli)
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
        Ok(())
    }
}

/// Display only: the renderer's gas cube, not a simulation number.
fn display_cell() -> u32 {
    320
}
fn normal_soil_fracture() -> u32 {
    1000
}

#[cfg(test)]
mod display_tests {
    use super::*;
    #[test]
    fn square_crater_is_optional_and_shape_is_bounded() {
        let text = include_str!("../../../data/nuclear-warhead.ron");
        let stock = ExplosionConfig::from_ron(text).unwrap();
        assert!(stock.soil_crater.is_some());
        let old = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        assert!(old.soil_crater.is_none());
        for (from, to) in [
            ("spread: 7", "spread: 0"),
            ("step: 32", "step: 65"),
            ("layer: 4", "layer: 0"),
            ("middle_depth: 32", "middle_depth: 3"),
            ("outer_depth: 4", "outer_depth: 36"),
            ("taper_units: 256", "taper_units: 0"),
        ] {
            assert!(
                ExplosionConfig::from_ron(&text.replace(from, to)).is_err(),
                "{to}"
            );
        }
    }
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
    fn spall_is_optional_kinetic_only_and_has_a_bounded_radius_and_budget() {
        let text = include_str!("../../../data/kinetic-shell.ron");
        let c = ExplosionConfig::from_ron(text).unwrap();
        assert_eq!(c.spall.as_ref().unwrap().radius, 3);
        assert!(
            ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
                .unwrap()
                .spall
                .is_none()
        );
        for bad in [
            text.replace("radius: 3", "radius: 0"),
            text.replace("radius: 3", "radius: 5"),
            text.replace("work_milli: 750", "work_milli: 0"),
            text.replace("work_milli: 750", "work_milli: 1000"),
            text.replace("delivery: Kinetic", "delivery: Blast"),
        ] {
            assert!(ExplosionConfig::from_ron(&bad).is_err());
        }
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

#[cfg(test)]
mod gravity_tests {
    use super::*;

    #[test]
    fn heavy_siege_charge_scales_without_overflow_and_keeps_supported_bounds() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
        assert_eq!(c.energy, 400_000);
        assert_eq!(c.cell_units, 4);
        assert_eq!(c.propagation_cell_units(), 9);
        assert_eq!(c.material_energy_j, 4_000_000_000);
        for power in [1000, 2000, 4000, u32::MAX] {
            let scaled = c.at_power(power);
            scaled.validate().unwrap();
            assert_eq!(
                scaled.material_energy_j,
                ExplosionConfig::MAX_MATERIAL_ENERGY_J
            );
            assert!(scaled.energy >= c.energy);
            assert!(scaled.propagation_cell_units() >= c.propagation_cell_units());
        }
    }

    /// Одно тяготение на юнита, кусок и снаряд (ADR-173, ADR-658):
    /// пресеты оружия и огонь полигона повторяют число `data/sim.ron`.
    #[test]
    fn every_gravity_in_data_is_the_one_from_sim_ron() {
        let one = crate::tuning::Tuning::from_ron(include_str!("../../../data/sim.ron"))
            .expect("data/sim.ron")
            .gravity;
        let presets = [
            ("explosion", include_str!("../../../data/explosion.ron")),
            (
                "kinetic-shell",
                include_str!("../../../data/kinetic-shell.ron"),
            ),
            (
                "canister-shell",
                include_str!("../../../data/canister-shell.ron"),
            ),
            ("siege-shell", include_str!("../../../data/siege-shell.ron")),
            (
                "energy-cannon",
                include_str!("../../../data/energy-cannon.ron"),
            ),
            ("anni-cannon", include_str!("../../../data/anni-cannon.ron")),
            ("machine-gun", include_str!("../../../data/machine-gun.ron")),
            (
                "flamethrower",
                include_str!("../../../data/flamethrower.ron"),
            ),
            ("laser", include_str!("../../../data/laser.ron")),
        ];
        for (name, text) in presets {
            let c = ExplosionConfig::from_ron(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(c.gravity, one, "{name}.ron: тяготение не то, что в sim.ron");
        }
        let fire =
            crate::range_fire::Tuning::from_ron(include_str!("../../../data/range-fire.ron"))
                .expect("data/range-fire.ron");
        assert_eq!(
            fire.gravity, one,
            "range-fire.ron: тяготение не то, что в sim.ron"
        );
    }
}
