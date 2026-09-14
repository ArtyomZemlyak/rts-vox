//! Whole root components leave atomically after bounded extraction and staging.
//! Queued damage waits for the affected body; other bodies keep their FIFO turns.
use super::*;
use fx::blob::{Blob, Scan};
use voxel::topo::{components::UnitComponents, unit::UnitVol};

pub struct Separated {
    pub unit: u32,
    pub cells: Vec<Cell>,
    pub shape: crate::burst::chunk::Shape,
}
pub(crate) struct Job {
    pub unit: u32,
    generation: u32,
    scan: UnitComponents,
    cells: Vec<Cell>,
    cursor: usize,
    staged: Option<Arc<UnitBody>>,
    shape: crate::burst::chunk::Shape,
}
impl Job {
    pub(crate) fn new(unit: u32, generation: u32, mask: UnitVol) -> Self {
        Self {
            unit,
            generation,
            scan: UnitComponents::new(mask),
            cells: Vec::new(),
            cursor: 0,
            staged: None,
            shape: crate::burst::chunk::Shape::default(),
        }
    }

    /// Shared bounded extraction. The caller keeps the source unchanged until
    /// commit; staging is private and cannot remove unpaid material from it.
    pub(crate) fn advance(&mut self, source: &UnitBody, budget: &mut u32) {
        while *budget > 0 {
            if self.cells.is_empty() {
                let mut token = 1;
                let found = self.scan.advance(&mut token);
                *budget -= 1;
                if let Some(cells) = found.into_iter().next() {
                    self.cells = cells;
                    self.staged = Some(Arc::new(source.clone()));
                } else if self.scan.finished() {
                    break;
                }
            } else if self.cursor < self.cells.len() {
                *budget -= 1;
                let c = self.cells[self.cursor];
                let [x, y, z] = c.map(usize::from);
                assert!(Arc::make_mut(self.staged.as_mut().unwrap()).remove(x, y, z));
                self.shape.add(c, source.turret_y);
                self.cursor += 1;
            } else {
                break;
            }
        }
    }
    pub(crate) fn ready(&self) -> bool {
        !self.cells.is_empty() && self.cursor == self.cells.len()
    }
    pub(crate) fn finished(&self) -> bool {
        self.scan.finished() && self.cells.is_empty()
    }
    pub(crate) fn commit(&mut self) -> (Arc<UnitBody>, Separated) {
        assert!(self.ready());
        self.cursor = 0;
        (
            self.staged.take().unwrap(),
            Separated {
                unit: self.unit,
                cells: std::mem::take(&mut self.cells),
                shape: std::mem::take(&mut self.shape),
            },
        )
    }
    pub(crate) fn save_cut(&self, b: &mut Blob) {
        self.scan.save(b);
        b.usize(self.cursor);
        b.u32(self.cells.len() as u32);
        for c in &self.cells {
            for v in c {
                b.u8(*v);
            }
        }
    }
    pub(crate) fn load_cut(
        unit: u32,
        generation: u32,
        s: &mut Scan,
        source: &UnitBody,
        core: &mut CoreBufs,
    ) -> Option<Self> {
        let scan = UnitComponents::load(s)?;
        let cursor = s.usize();
        let count = s.list(3);
        let status = source.status(core);
        if !status.alive
            || scan.dims() != [source.vol.nx, source.vol.ny, source.vol.nz]
            || count > status.detached as usize
            || cursor > count
        {
            return None;
        }
        let mut uncommitted = source.vol.clone();
        for (w, r) in uncommitted.cols.iter_mut().zip(core.reached_cols()) {
            *w &= !r;
        }
        if !scan.contained_in(&uncommitted) {
            return None;
        }
        let mut j = Self {
            unit,
            generation,
            scan,
            cells: Vec::new(),
            cursor,
            staged: (count > 0).then(|| Arc::new(source.clone())),
            shape: crate::burst::chunk::Shape::default(),
        };
        let mut cells = BTreeSet::new();
        for i in 0..count {
            let c = [s.u8(), s.u8(), s.u8()];
            if !cells.insert(c)
                || c[0] >= source.vol.nx
                || c[1] >= source.vol.ny
                || c[2] >= source.vol.nz
                || !uncommitted.get(c[0] as usize, c[1] as usize, c[2] as usize)
                || j.scan.contains(c)
            {
                return None;
            }
            if i < cursor {
                Arc::make_mut(j.staged.as_mut().unwrap()).remove(
                    c[0] as usize,
                    c[1] as usize,
                    c[2] as usize,
                );
                j.shape.add(c, source.turret_y);
            }
            j.cells.push(c);
        }
        (s.ok() && j.scan.pending_count() + count == status.detached as usize && !j.finished())
            .then_some(j)
    }
}
impl Bodies {
    pub fn detach_pending(&self) -> usize {
        self.detach.len()
    }
    /// One job per affected slot. A staging copy stays invisible until its whole
    /// section has a physical fragment slot. No partially cut / unpaid section.
    pub fn step_detach(
        &mut self,
        work: u32,
        jobs: u32,
        mut room: u32,
        alive: &[bool],
    ) -> Vec<Separated> {
        let mut out = Vec::new();
        let n = self.detach.len().min(jobs as usize);
        let mut left = work;
        for k in 0..n {
            let mut job = self.detach.pop_front().unwrap();
            let Some(state) = self
                .slots
                .get_mut(job.unit as usize)
                .and_then(Option::as_mut)
            else {
                continue;
            };
            if state.generation != job.generation {
                continue;
            }
            if !alive.get(job.unit as usize).copied().unwrap_or(false) || !state.status.alive {
                state.separating = false;
                continue;
            }
            let source = state
                .own
                .as_ref()
                .unwrap_or(&self.templates[state.template as usize]);
            let mut budget = left.div_ceil((n - k) as u32);
            let start = budget;
            job.advance(source, &mut budget);
            left -= start - budget;
            if job.ready() && room > 0 {
                room -= 1;
                let (body, part) = job.commit();
                let count = part.cells.len() as u32;
                state.own = Some(body);
                for c in &part.cells {
                    state.dents.remove(c);
                }
                for c in &part.cells {
                    state
                        .shape
                        .remove(*c, self.templates[state.template as usize].turret_y);
                }
                state.removed.extend_from_slice(&part.cells);
                state.removed_mm3 +=
                    count as u64 * (self.templates[state.template as usize].mm as u64).pow(3);
                state.status.detached -= count;
                let t = &self.templates[state.template as usize];
                state.mobility = crate::mobility::Mobility::from_shapes(
                    &self.template_shapes[state.template as usize],
                    &state.shape,
                    [t.vol.nx, t.vol.ny, t.vol.nz],
                    state.mobility.engine,
                )
                .with_traction(state.integrity.drive);
                state.revision = state.revision.wrapping_add(count as u64);
                out.push(part);
            }
            if job.scan.finished() && job.cells.is_empty() {
                state.separating = false;
            } else {
                self.detach.push_back(job);
            }
        }
        out
    }
    pub(super) fn save_detach(&self, b: &mut Blob) {
        b.u32(self.detach.len() as u32);
        for j in &self.detach {
            b.u32(j.unit);
            b.u32(j.generation);
            j.save_cut(b);
        }
    }
    pub(super) fn load_detach(&mut self, s: &mut Scan) -> bool {
        self.detach.clear();
        let n = s.list(8);
        if n > self.slots.len() {
            return false;
        }
        let mut seen = BTreeSet::new();
        for _ in 0..n {
            let unit = s.u32();
            let generation = s.u32();
            let Some(state) = self.slots.get_mut(unit as usize).and_then(Option::as_mut) else {
                return false;
            };
            let source = state
                .own
                .as_ref()
                .unwrap_or(&self.templates[state.template as usize]);
            if !seen.insert(unit) || generation != state.generation || !state.status.alive {
                return false;
            }
            let Some(j) = Job::load_cut(unit, generation, s, source, &mut self.core) else {
                return false;
            };
            state.separating = true;
            self.detach.push_back(j);
        }
        s.ok()
    }
    pub(super) fn detach_hash(&self) -> u64 {
        let mut h = self.detach.len() as u64;
        let add = |h: &mut u64, v| *h = h.wrapping_mul(0x100000001b3).wrapping_add(v);
        for j in &self.detach {
            for v in [
                j.unit as u64,
                j.generation as u64,
                j.scan.hash(),
                j.cursor as u64,
                j.cells.len() as u64,
            ] {
                add(&mut h, v);
            }
            for c in &j.cells {
                for v in c {
                    add(&mut h, *v as u64);
                }
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Bodies {
        let mut body = UnitBody::new_scaled(80, 5, 5, 1, 100);
        for x in 0..4 {
            body.set(x, 2, 2, voxel::Part::Armour);
        }
        body.set(0, 2, 2, voxel::Part::Core);
        for z in 1..4 {
            for y in 1..4 {
                for x in 4..70 {
                    body.set(x, y, z, voxel::Part::Armour);
                }
            }
        }
        body.seat_core();
        let mut b = Bodies::new(vec![body]);
        b.spawn_at(0, 0);
        b.spawn_at(1, 0);
        b.queue_cells(0, b.generations[0], vec![[3, 2, 2]], 1000, 0, V3::ZERO, 16);
        assert_eq!(b.step(&[1; 5], 8, 8)[0].removed, [[3, 2, 2]]);
        assert_eq!(b.detach_pending(), 1);
        b
    }
    #[test]
    fn a_large_section_stays_whole_and_waits_for_one_fragment_slot() {
        let mut b = fixture();
        let count = b.body(0).unwrap().solid_count();
        for _ in 0..20 {
            assert!(b.step_detach(1000, 8, 0, &[true, true]).is_empty());
        }
        assert_eq!(b.body(0).unwrap().solid_count(), count);
        assert_eq!(b.state(0).unwrap().removed.len(), 1);
        let sections = b.step_detach(1, 8, 1, &[true, true]);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].cells.len(), 66 * 9);
        assert_eq!(b.body(0).unwrap().solid_count(), 3);
        assert_eq!(b.state(0).unwrap().status.detached, 0);
        assert!(b.state(0).unwrap().status.alive);
        assert_eq!(b.state(0).unwrap().removed_mm3, (66 * 9 + 1) * 1_000_000);
        assert_eq!(b.body(1).unwrap().solid_count(), count + 1);
    }
    #[test]
    fn extraction_and_staging_resume_and_do_not_block_another_body() {
        let mut a = fixture();
        a.queue_cells(0, a.generations[0], vec![[2, 2, 2]], 1000, 0, V3::ZERO, 16);
        a.queue_cells(1, a.generations[1], vec![[69, 3, 3]], 1000, 0, V3::ZERO, 16);
        assert_eq!(a.step(&[1; 5], 8, 2)[0].unit, 1);
        for cut in [10, 90, 250, 650] {
            a.step_detach(cut, 8, 0, &[true, true]);
            let mut bytes = Blob::new();
            a.save(&mut bytes);
            let mut b = Bodies::new(a.templates.iter().map(|t| (**t).clone()).collect());
            let mut reader = Scan::new(&bytes.v);
            assert!(b.load(&mut reader, 2, 16), "cut {cut}");
            assert_eq!(reader.left(), 0);
            assert_eq!(a.hash(), b.hash());
            for _ in 0..10 {
                let x = a.step_detach(17, 8, 0, &[true, true]);
                let y = b.step_detach(17, 8, 0, &[true, true]);
                assert!(x.is_empty() && y.is_empty());
                assert_eq!(a.hash(), b.hash());
            }
        }
        for _ in 0..100 {
            a.step_detach(100, 8, 1, &[true, true]);
            if a.detach_pending() == 0 {
                break;
            }
        }
        assert_eq!(a.detach_pending(), 0);
        assert_eq!(a.step(&[1; 5], 8, 2)[0].removed, [[2, 2, 2]]);
    }
    #[test]
    fn cancelled_or_reused_slots_keep_uncommitted_matter_in_the_body() {
        let mut b = fixture();
        b.step_detach(800, 8, 0, &[true, true]);
        let count = b.body(0).unwrap().solid_count();
        assert!(b.step_detach(800, 8, 1, &[false, true]).is_empty());
        assert_eq!(b.body(0).unwrap().solid_count(), count);
        assert_eq!(b.detach_pending(), 0);
        let mut b = fixture();
        b.spawn_at(0, 0);
        assert!(b.step_detach(800, 8, 1, &[true, true]).is_empty());
        assert_eq!(
            b.body(0).unwrap().solid_count(),
            b.templates[0].solid_count()
        );
    }
}
