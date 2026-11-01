//! Surface detonations enter open air without bypassing the struck material.
use super::{material, BlastRange, Explosion, ExplosionConfig, HitFrames, RangeUnit};
use voxel::World;

impl BlastRange {
    /// Detonate an authored charge in a free-control scenario. The same
    /// wave and damage budgets as a normal impact apply; never replace an
    /// unfinished wave or let a scenario charge escape the map.
    pub fn scenario_charge(&mut self, at: [i32; 3], power_milli: u32) -> bool {
        if !self.battle_mode() || !self.shot_ready()
            || self.explosion.as_ref().is_some_and(|e| e.capturing())
            || (0..3).any(|a| at[a] < 0 || at[a] >= self.size[a] as i32)
        {
            return false;
        }
        let config = self.config;
        self.config = config.at_power(power_milli);
        self.begin_charge_impact(at, at);
        self.config = config;
        true
    }
    /// Shared by projectile contact and explicitly placed charges. Capture, wave
    /// work and collapse remain subject to the ordinary per-tick engine budgets.
    pub(super) fn begin_charge_impact(&mut self, at: [i32; 3], from: [i32; 3]) {
        self.impact = Some(at);
        self.impact_tick = self.tick;
        self.heat.deposit(
            &self.world,
            &self.matter.economy.mats,
            at,
            self.config.heat_units,
            self.config.heat_per_cell,
        );
        self.water.absorb_radial_heat(&self.world,at,self.config.heat_units,self.config.heat_per_cell);
        self.matter
            .begin(self.config.material_energy_j, self.config.fragment_box);
        let reserved=self.upheaval.reserve(&self.world,at,&self.config,self.matter.remaining_mj);
        self.matter.remaining_mj-=reserved;
        let mut wave_config=self.config;
        wave_config.material_energy_j=wave_config.material_energy_j.saturating_sub((reserved/1000) as u32);
        self.explosion = Some(Explosion::begin_impact(
            &self.world,
            at,
            from,
            &wave_config,
            &self.units,
            self.matter.tuning.body_energy_pct,
        ));
        self.shell = None;
    }
}

impl Explosion {
    pub(super) fn begin_impact(
        world: &World,
        at: [i32; 3],
        from: [i32; 3],
        config: &ExplosionConfig,
        units: &[RangeUnit],
        body_energy_pct: u32,
    ) -> Self {
        let mut e = Self::begin_material(at, config, units, body_energy_pct);
        if at == from || (material(world, at) == 0 && !units.iter().any(|u| u.solid(at))) {
            return e;
        }
        let direction = [0, 1, 2].map(|a| (from[a] - at[a]).signum());
        let mut previous = [0; 3];
        // At most one coarse-cell edge along the verified incoming air path.
        // A partly occupied coarse cell is not air: try the next free point,
        // but never step through matter or outside the adjacent neighbourhood.
        for step in 0..=e.cell {
            let p = [0, 1, 2].map(|a| from[a] + direction[a] * step);
            if p[0] < 0
                || p[1] < 0
                || p[2] < 0
                || p[0] >= world.nx as i32
                || p[1] >= world.ny as i32
                || p[2] >= world.nz as i32
                || material(world, p) != 0
                || units.iter().any(|u| u.solid(p))
            {
                return e;
            }
            let offset = [0, 1, 2].map(|a| (p[a] - at[a]).div_euclid(e.cell));
            if offset.iter().any(|d| !(-1..=1).contains(d)) {
                return e;
            }
            if offset == previous {
                continue;
            }
            previous = offset;
            let low = [0, 1, 2].map(|a| at[a] + offset[a] * e.cell);
            if clear_cell(world, units, &e, low) {
                e.wave.seed_exterior(offset);
                break;
            }
        }
        e
    }
}

fn clear_cell(world: &World, units: &[RangeUnit], e: &Explosion, low: [i32; 3]) -> bool {
    let high = low.map(|v| v + e.cell - 1);
    for z in low[2]..=high[2] {
        for y in low[1]..=high[1] {
            for x in low[0]..=high[0] {
                if material(world, [x, y, z]) != 0 {
                    return false;
                }
            }
        }
    }
    // Use the same frozen voxel-centre ownership as resistance capture,
    // including thin rotated plates that world-point sampling can miss.
    for (i, u) in units.iter().enumerate() {
        let pose = e.poses[i];
        let segs = &e.seg_poses[i];
        let delta = e.turret_deltas[i];
        let pitch = e.turret_pitches[i];
        let (lo, hi) = u.bounds_aimed(Some(&pose), segs, delta, pitch);
        if (0..3).any(|a| high[a] < lo[a] || low[a] > hi[a]) {
            continue;
        }
        let query = HitFrames::new(u, pose, segs, delta, pitch);
        let mut clear = true;
        query.visit_wave_cells(u, &pose, segs, low, e.cell, |(part, cell)| {
            let occupied = if part == 0 {
                u.body.vol.get(cell[0], cell[1], cell[2])
            } else {
                !u.seg_gone[part - 1]
                    && u.body.segments[part - 1]
                        .body
                        .vol
                        .get(cell[0], cell[1], cell[2])
            };
            clear &= !occupied;
            clear
        });
        if !clear {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[path = "range_blast_source_tests.rs"]
mod tests;
