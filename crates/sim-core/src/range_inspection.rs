//! Read-only diagnostics for one minimum world cell, shared by editor/tools.
use super::*;

#[derive(Clone, Debug)]
pub struct CellInspection {
    pub at: [u32; 3],
    pub material: u16,
    pub definition: Option<modding::MatDef>,
    pub heat: crate::heat::HeatCell,
    pub moisture: u32,
    pub moisture_capacity: u32,
    pub water: u32,
    pub water_capacity: u32,
    pub deformation_work: u32,
    pub pending_dig: bool,
    pub claimed: bool,
    pub frame: bool,
    pub repose_drop: Option<u32>,
    pub yield_pressure: Option<u32>,
    pub motion: Option<(Fx, Fx)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn minimum_cell_inspection_reads_column_motion_without_mutating_simulation() {
        let mut layout = modding::laboratory::experiments::stock()
            .into_iter()
            .find(|e| e.id == "sand-flow")
            .unwrap()
            .layout;
        layout.blocks = vec![modding::range::RangeBlock {
            min: [380, 32, 336],
            max: [383, 48, 339],
            material: 10,
        }];
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/explosion.ron"
        ))
        .unwrap();
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        sim.control();
        sim.step();
        let before = sim.state_hash();
        let low = sim.inspect_cell([381, 32, 337]).unwrap();
        let high = sim.inspect_cell([381, 47, 337]).unwrap();
        assert_eq!(low.material, 10);
        assert_eq!(low.definition.unwrap().id, 10);
        assert!(low.motion.is_some());
        assert_eq!(low.motion, high.motion);
        assert_eq!(low.repose_drop, Some(1));
        assert_eq!(sim.inspect_cell([381, 70, 337]).unwrap().material, 0);
        assert!(sim.inspect_cell([u32::MAX, 0, 0]).is_none());
        assert_eq!(sim.state_hash(), before);
    }
}
impl BlastRange {
    pub fn inspect_cell(&self, at: [u32; 3]) -> Option<CellInspection> {
        self.inspect_cell_with_motion(at, true)
    }
    /// Skip the potentially deep granular-column query for static overlays.
    pub fn inspect_cell_with_motion(&self, at: [u32; 3], motion: bool) -> Option<CellInspection> {
        if (0..3).any(|a| at[a] >= self.size[a]) {
            return None;
        }
        let material = self.world.material(at[0], at[1], at[2]);
        let granular = self.granular.tuning.material(material);
        Some(CellInspection {
            at,
            material,
            definition: self.matter.economy.mats.get(material).copied(),
            heat: self.heat.cell(at),
            moisture: self.water.moisture(at),
            moisture_capacity: self
                .water
                .tuning
                .porous
                .iter()
                .find(|m| m.material == material)
                .map_or(0, |m| m.capacity),
            water: self.water.amount(at),
            water_capacity: self.water.tuning.units_per_voxel,
            deformation_work: self
                .surface_work
                .get(&at.map(|v| v as i32))
                .copied()
                .unwrap_or(0),
            pending_dig: self.world.has_pending_dig(at[0], at[1], at[2]),
            claimed: self.matter.debris.claimed(at),
            frame: self.world.on_frame(at[0], at[1], at[2]),
            repose_drop: granular
                .map(|m| (m.repose_drop as i32 + self.water.repose_bonus(at)).max(1) as u32),
            yield_pressure: granular.map(|m| m.yield_pressure),
            motion: if motion {self.granular.motion_at(&self.world, at)} else {None},
        })
    }
}
