//! Saved continuations for material contacts that yield a bounded wave queue.
use super::*;
use fx::blob::{Blob, Scan};

impl BlastRange {
    pub(super) fn has_contact_resumes(&self) -> bool {
        self.explosion
            .as_ref()
            .is_some_and(|e| !e.contact_resumes.is_empty())
            || self.battle.has_contact_resumes()
    }
    pub(super) fn save_contact_resumes(&self, b: &mut Blob) {
        if let Some(e) = &self.explosion {
            e.save_resumes(b);
        }
        self.battle.save_contact_resumes(b);
    }
    pub(super) fn load_contact_resumes(&mut self, s: &mut Scan) -> Option<()> {
        if let Some(e) = &mut self.explosion {
            e.load_resumes(s, &self.units)?;
        }
        self.battle.load_contact_resumes(s, &self.units)?;
        Some(())
    }
}

impl Explosion {
    pub(super) fn save_resumes(&self, b: &mut Blob) {
        b.usize(self.contact_resumes.len());
        for (cell, (contact, body)) in &self.contact_resumes {
            for v in cell {
                b.i32(*v);
            }
            b.usize(*contact);
            b.usize(*body);
        }
    }
    pub(super) fn load_resumes(&mut self, s: &mut Scan, units: &[RangeUnit]) -> Option<()> {
        let units = &units[..self.roster(units.len())];
        let count = s.usize();
        if count > self.pending.len().saturating_sub(1) || count > s.left() / 28 {
            return None;
        }
        let mut hits = BTreeMap::new();
        for hit in &self.pending {
            if hits.insert(hit.cell, hit).is_some() {
                return None;
            }
        }
        for _ in 0..count {
            let cell = std::array::from_fn(|_| s.i32());
            let contact = s.usize();
            let body = s.usize();
            let hit = hits.get(&cell)?;
            if self.pending.front().is_some_and(|h| h.cell == cell)
                || self.contact_resumes.insert(cell, (contact, body)).is_some()
            {
                return None;
            }
            self.validate_contact_cursor(Some(hit), contact, body, units)?;
        }
        s.ok().then_some(())
    }
}
