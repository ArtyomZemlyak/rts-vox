//! Whole weapon configuration is a bounded value, independent of a UI preset.
//! Applied saves own both the constructor base and the current live setting.
use super::{battle::commands::Outcome, BlastRange};
use fx::blob::{Blob, Scan};
use modding::explosion::{
    Delivery, ExplosionConfig, ProjectileMotion, Ricochet, SoilCrater, Spall,
};

impl BlastRange {
    pub(super) fn configure_weapon(&mut self, mut config: ExplosionConfig) -> Outcome {
        if let Some(layout) = &self.layout {
            config.unit_ablation_resistance = layout.unit_resistance;
        }
        // Validate before mutation, including the base required for restoration.
        if config.validate().is_err() || self.weapon_initial.validate().is_err() {
            return Outcome::Rejected;
        }
        if config == self.config {
            return Outcome::Unchanged;
        }
        self.config = config;
        // Keep ownership after reset: callers need not remember either preset.
        self.weapon_configured = true;
        Outcome::Applied
    }
}

pub(super) fn save(c: &ExplosionConfig, b: &mut Blob) {
    if let Some(shape) = c.soil_crater {
        b.u8(130);
        for value in [
            shape.spread,
            shape.step,
            shape.layer,
            shape.deep_half_width,
            shape.middle_half_width,
            shape.middle_depth,
            shape.outer_depth,
            shape.taper_units,
        ] {
            b.u32(value);
        }
    }
    if let Some(work) = c.material_resistance {
        b.u8(129);
        for value in work {
            b.u32(value);
        }
    }
    // Self-describing extension: legacy configs still start with delivery
    // 0..=4, including old applied saves and queued configuration commands.
    if c.soil_depth_cost != 0 || c.soil_fracture_milli != 1000 {
        b.u8(128);
        b.u32(c.soil_depth_cost);
        b.u32(c.soil_fracture_milli);
    }
    b.u8(match c.delivery {
        Delivery::Blast => 0,
        Delivery::Kinetic => 1,
        Delivery::Laser => 2,
        Delivery::Flame => 3,
        Delivery::MachineGun => 4,
    });
    b.u8(match c.motion {
        ProjectileMotion::Ballistic => 0,
        ProjectileMotion::Direct => 1,
    });
    b.u32(c.charge_ticks);
    b.usize(c.half_cells);
    b.i32(c.cell_units);
    b.u32(c.energy);
    b.u32(c.material_energy_j);
    b.u32(c.air_cost);
    b.u32(c.soil_resistance);
    b.u32(c.concrete_resistance);
    b.u32(c.steel_resistance);
    b.u32(c.unit_ablation_resistance);
    for n in c.unit_role_resistance_milli {
        b.u32(n);
    }
    b.usize(c.wave_work_per_tick);
    b.usize(c.capture_cells_per_tick);
    b.usize(c.edits_per_tick);
    b.u32(c.fire_tick);
    b.i32(c.shell_speed);
    b.bool(c.ricochet.is_some());
    if let Some(r) = c.ricochet {
        b.u16(r.material);
        b.i32(r.min_speed);
        b.u32(r.max_normal_milli);
        b.u32(r.wear_milli);
        b.u32(r.speed_retention_milli);
        b.u32(r.max_preexisting_wear_milli);
    }
    b.bool(c.spall.is_some());
    if let Some(p) = c.spall {
        b.u8(p.radius);
        b.u32(p.work_milli);
    }
    b.i32(c.gravity);
    b.i32(c.gas_cell_units);
    b.u32(c.gas_display_mm);
    b.i32(c.gas_source_radius);
    b.i32(c.gas_temperature_k);
    b.i32(c.gas_fuel_milli);
    b.i32(c.heat_units);
    b.u32(c.heat_per_cell);
    b.i32(c.gas_pressure_milli);
    b.i32(c.reaction_milli);
    b.i32(c.cooling_milli);
    b.i32(c.buoyancy_milli);
    b.u32(c.fragment_box);
    b.u32(c.hull_impulse_milli);
}
fn boolean(s: &mut Scan) -> Option<bool> {
    match s.u8() {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}
fn size(s: &mut Scan) -> Option<usize> {
    usize::try_from(s.u64()).ok()
}

/// Decode wire structure only: invalid pending gameplay inputs must still reach
/// the executor and receive Rejected. Applied snapshot callers also validate.
pub(super) fn load(s: &mut Scan) -> Option<ExplosionConfig> {
    let mut first = s.u8();
    let soil_crater = if first == 130 {
        let shape = SoilCrater {
            spread: s.u32(),
            step: s.u32(),
            layer: s.u32(),
            deep_half_width: s.u32(),
            middle_half_width: s.u32(),
            middle_depth: s.u32(),
            outer_depth: s.u32(),
            taper_units: s.u32(),
        };
        first = s.u8();
        Some(shape)
    } else {
        None
    };
    let material_resistance = if first == 129 {
        let work = std::array::from_fn(|_| s.u32());
        first = s.u8();
        Some(work)
    } else {
        None
    };
    let (soil_depth_cost, soil_fracture_milli, delivery) = if first == 128 {
        (s.u32(), s.u32(), s.u8())
    } else {
        (0, 1000, first)
    };
    let c = ExplosionConfig {
        delivery: match delivery {
            0 => Delivery::Blast,
            1 => Delivery::Kinetic,
            2 => Delivery::Laser,
            3 => Delivery::Flame,
            4 => Delivery::MachineGun,
            _ => return None,
        },
        motion: match s.u8() {
            0 => ProjectileMotion::Ballistic,
            1 => ProjectileMotion::Direct,
            _ => return None,
        },
        charge_ticks: s.u32(),
        half_cells: size(s)?,
        cell_units: s.i32(),
        energy: s.u32(),
        material_energy_j: s.u32(),
        air_cost: s.u32(),
        soil_resistance: s.u32(),
        soil_depth_cost,
        soil_crater,
        soil_fracture_milli,
        concrete_resistance: s.u32(),
        steel_resistance: s.u32(),
        material_resistance,
        unit_ablation_resistance: s.u32(),
        unit_role_resistance_milli: std::array::from_fn(|_| s.u32()),
        wave_work_per_tick: size(s)?,
        capture_cells_per_tick: size(s)?,
        edits_per_tick: size(s)?,
        fire_tick: s.u32(),
        shell_speed: s.i32(),
        ricochet: if boolean(s)? {
            Some(Ricochet {
                material: s.u16(),
                min_speed: s.i32(),
                max_normal_milli: s.u32(),
                wear_milli: s.u32(),
                speed_retention_milli: s.u32(),
                max_preexisting_wear_milli: s.u32(),
            })
        } else {
            None
        },
        spall: if boolean(s)? {
            Some(Spall {
                radius: s.u8(),
                work_milli: s.u32(),
            })
        } else {
            None
        },
        gravity: s.i32(),
        gas_cell_units: s.i32(),
        gas_display_mm: s.u32(),
        gas_source_radius: s.i32(),
        gas_temperature_k: s.i32(),
        gas_fuel_milli: s.i32(),
        heat_units: s.i32(),
        heat_per_cell: s.u32(),
        gas_pressure_milli: s.i32(),
        reaction_milli: s.i32(),
        cooling_milli: s.i32(),
        buoyancy_milli: s.i32(),
        fragment_box: s.u32(),
        hull_impulse_milli: s.u32(),
    };
    s.ok().then_some(c)
}

#[cfg(test)]
#[path = "range_weapon_configuration_tests.rs"]
mod tests;
