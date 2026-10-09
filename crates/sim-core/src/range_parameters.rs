//! On-demand parameter inspection, independent of the roster/geometry export.
use super::{BlastRange, RangeUnitKind};
use modding::{explosion::ExplosionConfig, tuning::Blast, Chassis, Mat, MatDef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionParameters {
    pub speed: i32,
    pub step: i32,
    pub chassis: Chassis,
}
impl MotionParameters {
    pub fn new(motion: &super::super::motion::Motion) -> Self {
        Self {
            speed: motion.speed,
            step: motion.step,
            chassis: motion.chassis,
        }
    }
}

/// First roster slot of each kind, including dead units, and only requested
/// materials. No paths, bodies, geometry, history or references are retained.
/// Missing kinds/materials remain missing; presentation chooses its defaults.
#[derive(Clone, Debug)]
pub struct Parameters {
    pub generation: u64,
    pub tick: u32,
    pub weapon: ExplosionConfig,
    pub gun: MotionParameters,
    pub motions: [Option<MotionParameters>; RangeUnitKind::COUNT],
    /// Walker speed and step height from the optional course definition.
    pub course: Option<[i32; 2]>,
    pub debris: Blast,
    pub materials: Vec<MatDef>,
}
impl Parameters {
    pub fn new(world: &BlastRange, generation: u64, materials: impl Iterator<Item = Mat>) -> Self {
        let mut view = Self {
            generation,
            tick: world.tick,
            weapon: world.config,
            gun: MotionParameters::new(&world.gun.motion),
            motions: [None; RangeUnitKind::COUNT],
            course: None,
            debris: world.matter.tuning,
            materials: Vec::new(),
        };
        view.update(world, generation, materials);
        view
    }

    /// Fresh even at the same tick: paused edits and rebuilds are observable.
    /// The caller bounds its request; a sparse registry is never scanned.
    pub fn update(
        &mut self,
        world: &BlastRange,
        generation: u64,
        materials: impl Iterator<Item = Mat>,
    ) {
        self.generation = generation;
        self.tick = world.tick;
        self.weapon.clone_from(&world.config);
        self.gun = MotionParameters::new(&world.gun.motion);
        self.motions.fill(None);
        for unit in &world.units {
            self.motions[unit.kind.index()]
                .get_or_insert_with(|| MotionParameters::new(&unit.motion));
        }
        self.course = world
            .layout
            .as_ref()
            .and_then(|l| l.course.as_ref())
            .map(|c| [c.walker_speed, c.step_height]);
        self.debris = world.matter.tuning;
        self.materials.clear();
        self.materials
            .extend(materials.filter_map(|id| world.matter.economy.mats.get(id).copied()));
        self.materials.sort_unstable_by_key(|m| m.id);
        self.materials.dedup_by_key(|m| m.id);
    }

    pub fn material(&self, id: Mat) -> Option<&MatDef> {
        self.materials
            .binary_search_by_key(&id, |m| m.id)
            .ok()
            .map(|i| &self.materials[i])
    }

    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.materials.len() * std::mem::size_of::<MatDef>()
    }
}
