//! Unit mesh inputs, exported independently of renderer and scenario identity.
//! Cheap metadata is refreshed every publication; bodies are copied on demand.
use super::{BlastRange, RangeUnit, RangeUnitKind};
use modding::explosion::ExplosionConfig;
use std::{collections::BTreeMap, num::NonZeroUsize};
use voxel::unit::UnitBody;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Revision {
    pub removed: usize,
    pub damage: u64,
    pub kind: RangeUnitKind,
    pub dead: bool,
    pub resistance: u32,
    pub role_resistance_milli: [u32; 5],
}
impl Revision {
    fn of(unit: &RangeUnit, config: &ExplosionConfig) -> Self {
        Self {
            removed: unit.removed.len(),
            damage: unit.revision,
            kind: unit.kind,
            dead: !unit.status.alive,
            resistance: config.unit_ablation_resistance,
            role_resistance_milli: config.unit_role_resistance_milli,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    pub revision: Revision,
    /// Authored placement used to install a mesh; animated poses are separate.
    pub anchor: [i32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemovedCell {
    pub unit: usize,
    pub cell: [u8; 3],
    pub position: [i32; 3],
}

/// Consumer policy, not physical work. Nonzero stride prevents invalid scans;
/// each unit exports at most per_unit records in its original removal order.
#[derive(Clone, Copy)]
pub struct Sampling {
    pub stride: NonZeroUsize,
    pub per_unit: usize,
}

#[derive(Clone, Copy)]
pub struct Cursor<'a> {
    pub generation: u64,
    pub counts: &'a [usize],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub generation: u64,
    pub tick: u32,
    pub units: Vec<Unit>,
    /// Cosmetic ejecta exist only for the legacy annihilation impact.
    pub impact: Option<[i32; 3]>,
    pub removed: Vec<RemovedCell>,
    config: Option<ExplosionConfig>,
}
impl View {
    /// A cursor from another generation starts at zero. Missing or oversized
    /// cursors start at zero or clamp to the current count respectively.
    pub fn update(
        &mut self,
        world: &BlastRange,
        generation: u64,
        consumed: Cursor<'_>,
        sampling: Sampling,
    ) {
        self.generation = generation;
        self.tick = world.tick;
        self.config = Some(world.config);
        self.impact = world.impact.filter(|_| world.config.material_energy_j == 0);
        self.units.clear();
        self.removed.clear();
        for (id, unit) in world.units.iter().enumerate() {
            self.units.push(Unit {
                revision: Revision::of(unit, &world.config),
                anchor: unit.at,
            });
            if self.impact.is_some() && sampling.per_unit != 0 {
                let from = if consumed.generation == generation {
                    consumed
                        .counts
                        .get(id)
                        .copied()
                        .unwrap_or(0)
                        .min(unit.removed.len())
                } else {
                    0
                };
                self.removed.extend(
                    unit.removed[from..]
                        .iter()
                        .step_by(sampling.stride.get())
                        .take(sampling.per_unit)
                        .map(|&cell| RemovedCell {
                            unit: id,
                            cell,
                            position: unit.world_pos(cell.map(usize::from)),
                        }),
                );
            }
        }
    }

    /// Capture only a requested body's mesh inputs. A stale publication,
    /// configuration, revision or slot fails before any body copy occurs.
    /// The owner/caller supplies generation; tick alone is not an epoch.
    pub fn mesh(&self, world: &BlastRange, generation: u64, id: usize) -> Option<Mesh> {
        if self.generation != generation
            || self.tick != world.tick
            || self.config != Some(world.config)
        {
            return None;
        }
        let metadata = self.units.get(id)?;
        let unit = world.units.get(id)?;
        if metadata.revision != Revision::of(unit, &world.config) || metadata.anchor != unit.at {
            return None;
        }
        Some(Mesh {
            generation,
            tick: world.tick,
            id,
            revision: metadata.revision,
            body: unit.body.clone(),
            dents: unit.dents.clone(),
            segment_dents: unit.segment_dents().clone(),
            seg_gone: unit.seg_gone.clone(),
            allied: world.battle_mode().then(|| world.allied(id)),
            config: world.config,
        })
    }

    /// Logical payload, excluding allocator metadata and spare capacity.
    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.units.len() * std::mem::size_of::<Unit>()
            + self.removed.len() * std::mem::size_of::<RemovedCell>()
    }
}

/// Owned root/segment geometry and damage ledgers for one mesh job. Shared
/// immutable body labels remain Arc-backed; mutable occupancy is copied.
#[derive(Clone)]
pub struct Mesh {
    pub generation: u64,
    pub tick: u32,
    pub id: usize,
    pub revision: Revision,
    pub body: UnitBody,
    pub dents: BTreeMap<[u8; 3], u32>,
    pub segment_dents: BTreeMap<(u8, [u8; 3]), u32>,
    pub seg_gone: Vec<bool>,
    pub allied: Option<bool>,
    pub config: ExplosionConfig,
}
impl Mesh {
    /// Logical private payload: excludes shared immutable labels, spare
    /// capacity and BTree node overhead. This is not process resident memory.
    pub fn private_payload_bytes(&self) -> usize {
        fn body_bytes(body: &UnitBody) -> usize {
            body.bytes_private()
                + body.legs.len() * std::mem::size_of::<voxel::unit::Leg>()
                + body.segments.len() * std::mem::size_of::<voxel::unit::Segment>()
                + body
                    .segments
                    .iter()
                    .map(|s| body_bytes(&s.body))
                    .sum::<usize>()
        }
        std::mem::size_of::<Self>()
            + body_bytes(&self.body)
            + self.dents.len() * std::mem::size_of::<([u8; 3], u32)>()
            + self.segment_dents.len() * std::mem::size_of::<((u8, [u8; 3]), u32)>()
            + self.seg_gone.len() * std::mem::size_of::<bool>()
    }
}

#[cfg(test)]
#[path = "range_geometry_view_tests.rs"]
mod tests;
