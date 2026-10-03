//! Render slots follow the physical array. Normal compaction is a linear merge;
//! imported/reordered IDs use a compatibility map only while aligning.
use super::{pos, slots::Slot, Entity, HashMap, Mat3, Quat, Transform, Vec3};
use sim_core::{blast::RangeUnit, burst::Fragment};

#[derive(Clone, Copy)]
pub(super) enum Prepared {
    Existing(Option<Transform>),
    Replace((u64, bool), Transform),
}
pub(super) struct Entry {
    id: u64,
    pub slot: Option<Slot>,
    pub prepared: Prepared,
}
impl Entry {
    fn new(id: u64) -> Self {
        Self {
            id,
            slot: None,
            prepared: Prepared::Existing(None),
        }
    }
    pub fn prepare(
        &mut self,
        f: &Fragment,
        units: &[RangeUnit],
        previous: Option<Vec3>,
        alpha: f32,
        exists: impl Fn(Entity) -> bool,
    ) {
        debug_assert_eq!(self.id, f.id);
        let key = f.chunk.as_ref().map_or((0, false), |c| {
            (c.hash(), !units[c.template as usize].status.alive)
        });
        if let Some(slot) = &mut self.slot {
            if slot.matches(key, f) && exists(slot.entity) {
                self.prepared =
                    Prepared::Existing(slot.sample(f, previous, alpha, || transform(f)));
                return;
            }
        }
        self.prepared = Prepared::Replace(key, transform(f));
    }
}
fn transform(f: &Fragment) -> Transform {
    let current = pos([f.position.x, f.position.y, f.position.z]);
    let basis = f.basis.map(|v| pos([v.x, v.y, v.z]));
    let rotation = Quat::from_mat3(&Mat3::from_cols(basis[0], basis[1], basis[2])).normalize();
    Transform::from_translation(current)
        .with_rotation(rotation)
        .with_scale(Vec3::splat(render::model::fragment_scale(f)))
}
pub(super) struct Retired {
    pub id: u64,
    pub entity: Entity,
}
#[derive(Default)]
pub(super) struct Slots {
    pub entries: Vec<Entry>,
    scratch: Vec<Entry>,
    pub retired: Vec<Retired>,
}
impl Slots {
    pub fn prepare(
        &mut self,
        pieces: &[Fragment],
        units: &[RangeUnit],
        previous: &HashMap<u64, Vec3>,
        alpha: f32,
        exists: impl Fn(Entity) -> bool + Sync,
        pool: Option<&bevy::tasks::TaskPool>,
    ) {
        let prepare = |entries: &mut [Entry], pieces: &[Fragment]| {
            for (entry, f) in entries.iter_mut().zip(pieces) {
                entry.prepare(f, units, previous.get(&f.id).copied(), alpha, &exists);
            }
        };
        if let Some(pool) = pool.filter(|_| pieces.len() >= 2048) {
            pool.scope(|scope| {
                for (entries, pieces) in self.entries.chunks_mut(2048).zip(pieces.chunks(2048)) {
                    let prepare = &prepare;
                    scope.spawn(async move { prepare(entries, pieces) });
                }
            });
        } else {
            prepare(&mut self.entries, pieces);
        }
    }
    fn retire(&mut self, entry: Entry) {
        if let Some(slot) = entry.slot {
            self.retired.push(Retired {
                id: entry.id,
                entity: slot.entity,
            });
        }
    }
    pub fn align(&mut self, pieces: &[Fragment]) {
        if self.entries.len() == pieces.len()
            && self.entries.iter().zip(pieces).all(|(s, f)| s.id == f.id)
        {
            return;
        }
        self.scratch.clear();
        let ordered = pieces.windows(2).all(|w| w[0].id < w[1].id)
            && self.entries.windows(2).all(|w| w[0].id < w[1].id);
        // Move the allocation out so retirement can fill a separate buffer.
        let mut old = std::mem::take(&mut self.entries);
        if ordered {
            let mut iter = old.drain(..).peekable();
            for f in pieces {
                while iter.peek().is_some_and(|s| s.id < f.id) {
                    self.retire(iter.next().unwrap());
                }
                let entry = if iter.peek().is_some_and(|s| s.id == f.id) {
                    iter.next().unwrap()
                } else {
                    Entry::new(f.id)
                };
                self.scratch.push(entry);
            }
            for entry in iter {
                self.retire(entry);
            }
        } else {
            let mut lookup: HashMap<_, _> = old.drain(..).map(|s| (s.id, s)).collect();
            for f in pieces {
                self.scratch
                    .push(lookup.remove(&f.id).unwrap_or_else(|| Entry::new(f.id)));
            }
            for (_, entry) in lookup {
                self.retire(entry);
            }
        }
        self.entries = std::mem::replace(&mut self.scratch, old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::Fx;
    use sim_core::damage::V3;
    fn piece(id: u64) -> Fragment {
        Fragment {
            id,
            position: V3::new(Fx::from_int(id as i32), Fx::ZERO, Fx::ZERO),
            velocity: V3::ZERO,
            material: 3,
            paint: None,
            mm: 100,
            basis: [
                V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
                V3::UP,
                V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
            ],
            mass_g: 1,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: 0,
            resting: true,
        }
    }
    fn install(slots: &mut Slots, pieces: &[Fragment]) {
        for (entry, f) in slots.entries.iter_mut().zip(pieces) {
            let tr = transform(f);
            entry.slot = Some(Slot::new(Entity::from_bits(f.id), (0, false), f, tr, tr));
        }
    }
    #[test]
    fn ordered_compaction_and_unordered_import_preserve_identities() {
        let mut slots = Slots::default();
        let pieces: Vec<_> = [7, 9, 12].into_iter().map(piece).collect();
        slots.align(&pieces);
        install(&mut slots, &pieces);
        for ids in [vec![7, 12, 19], vec![19, 7, 12], vec![12, 19], vec![]] {
            let pieces: Vec<_> = ids.into_iter().map(piece).collect();
            slots.align(&pieces);
            assert_eq!(slots.entries.len(), pieces.len());
            for (entry, f) in slots.entries.iter().zip(&pieces) {
                assert_eq!(entry.id, f.id);
                if let Some(slot) = &entry.slot {
                    assert_eq!(slot.entity, Entity::from_bits(f.id));
                }
            }
            for retired in slots.retired.drain(..) {
                assert!(!pieces.iter().any(|f| f.id == retired.id));
            }
            install(&mut slots, &pieces);
        }
    }
    #[test]
    fn parallel_preparation_matches_serial_across_wake_pause_and_missing_entities() {
        let pool = bevy::tasks::TaskPoolBuilder::new().num_threads(4).build();
        let mut pieces: Vec<_> = (1..=8193).map(piece).collect();
        let mut serial = Slots::default();
        let mut parallel = Slots::default();
        serial.align(&pieces);
        parallel.align(&pieces);
        install(&mut serial, &pieces);
        install(&mut parallel, &pieces);
        let previous: HashMap<_, _> = pieces.iter().map(|f| (f.id, Vec3::ZERO)).collect();
        for round in 0..4 {
            for f in &mut pieces {
                if f.id % 3 == 0 {
                    f.position.y = Fx::from_int(round);
                    f.resting = false;
                    f.mm = 100 + round as u16 * 10;
                }
            }
            for alpha in [0.0, 0.25, 0.75, 1.0] {
                // Emulate external entity removal and a changed mesh on reset.
                let exists = |e: Entity| e.to_bits() % 11 != 0;
                serial.prepare(&pieces, &[], &previous, alpha, exists, None);
                parallel.prepare(&pieces, &[], &previous, alpha, exists, Some(&pool));
                for (a, b) in serial.entries.iter().zip(&parallel.entries) {
                    match (a.prepared, b.prepared) {
                        (Prepared::Existing(a), Prepared::Existing(b)) => assert_eq!(a, b),
                        (Prepared::Replace(ka, a), Prepared::Replace(kb, b)) => {
                            assert_eq!(ka, kb);
                            assert_eq!(a, b);
                        }
                        _ => panic!("serial/parallel lifecycle mismatch"),
                    }
                }
            }
        }
    }
}
