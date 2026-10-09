//! Live material properties. Density applies to newly revealed terrain;
//! existing containers retain their intrinsic matter and physical mass.
use super::{battle::commands::Outcome, BlastRange};
use fx::blob::{Blob, Scan};
use modding::{Mat, MatDef, Registry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MaterialField {
    Strength = 0,
    Melt = 1,
    Density = 2,
}
impl MaterialField {
    pub const ALL: [Self; 3] = [Self::Strength, Self::Melt, Self::Density];

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }
}

pub(super) type DefaultValues = (Mat, [u32; 3]);

fn values(d: &MatDef) -> [u32; 3] {
    [d.strength, d.melt, d.density]
}
pub(super) fn defaults(registry: &Registry) -> Vec<DefaultValues> {
    registry.definitions().map(|d| (d.id, values(d))).collect()
}

impl BlastRange {
    /// Same normalization and registry validation as the former UI setter.
    /// No traversal of bodies, fragments, queues or the matter ledger.
    pub(super) fn tune_material(
        &mut self,
        material: Mat,
        field: MaterialField,
        value: u32,
    ) -> Outcome {
        // The world's material pack is fixed at construction; adding materials
        // later requires its own restore contract, not an unsaved live override.
        if self
            .material_defaults
            .binary_search_by_key(&material, |d| d.0)
            .is_err()
        {
            return Outcome::Rejected;
        }
        let registry = &mut self.matter.economy.mats;
        let Some(&before) = registry.get(material) else {
            return Outcome::Rejected;
        };
        let mut after = before;
        match field {
            MaterialField::Strength => after.strength = value.max(1),
            MaterialField::Melt => after.melt = value,
            MaterialField::Density => after.density = value.max(1),
        }
        if after == before {
            return Outcome::Unchanged;
        }
        if registry.replace(after).is_err() {
            return Outcome::Rejected;
        }
        if field == MaterialField::Density {
            // Persistent even after reset: mixed-density fragments can survive.
            self.matter.density_history = true;
        }
        Outcome::Applied
    }

    pub(super) fn has_material_tuning(&self) -> bool {
        self.material_defaults.iter().any(|&(id, base)| {
            self.matter
                .economy
                .mats
                .get(id)
                .is_some_and(|d| values(d) != base)
        })
    }

    pub(super) fn save_material_tuning(&self, b: &mut Blob) {
        let changed = || {
            self.material_defaults.iter().filter_map(|&(id, base)| {
                let current = values(self.matter.economy.mats.get(id)?);
                let mask = u8::from(current[0] != base[0])
                    | (u8::from(current[1] != base[1]) << 1)
                    | (u8::from(current[2] != base[2]) << 2);
                (mask != 0).then_some((id, mask, current))
            })
        };
        b.u32(changed().count() as u32);
        for (id, mask, current) in changed() {
            b.u16(id);
            b.u8(mask);
            for (index, value) in current.into_iter().enumerate() {
                if mask & (1 << index) != 0 {
                    b.u32(value);
                }
            }
        }
    }

    pub(super) fn load_material_tuning(&mut self, s: &mut Scan, density: bool) -> Option<()> {
        let count = s.u32() as usize;
        if (!density && count == 0) || count > self.material_defaults.len() || count > s.left() / 7
        {
            return None;
        }
        let mut previous = None;
        for _ in 0..count {
            let id = s.u16();
            if previous.is_some_and(|last| id <= last) {
                return None;
            }
            previous = Some(id);
            let index = self
                .material_defaults
                .binary_search_by_key(&id, |d| d.0)
                .ok()?;
            let mut current = self.material_defaults[index].1;
            let mask = s.u8();
            if mask == 0 || mask > if density { 7 } else { 3 } {
                return None;
            }
            for (index, value) in current.iter_mut().enumerate() {
                if mask & (1 << index) == 0 {
                    continue;
                }
                let saved = s.u32();
                if saved == *value {
                    return None;
                }
                *value = saved;
            }
            let mut definition = *self.matter.economy.mats.get(id)?;
            [definition.strength, definition.melt, definition.density] = current;
            self.matter.economy.mats.replace(definition).ok()?;
        }
        self.matter.density_history = density;
        s.ok().then_some(())
    }
}

#[cfg(test)]
#[path = "range_material_tuning_tests.rs"]
mod tests;
