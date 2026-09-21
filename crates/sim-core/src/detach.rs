//! Whole root components leave atomically after bounded extraction and staging.
//! The job owns only the cells of its domain; damage elsewhere on the same body
//! does not wait for it. As many ready sections leave per tick as the shared
//! budget and the fragment pool allow.
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
    shape: crate::burst::chunk::Shape,
    /// Cells reserved for whole sections. Only the job removes them; every
    /// other cell of the body stays open to ordinary damage meanwhile.
    domain: UnitVol,
}
impl Job {
    pub(crate) fn new(unit: u32, generation: u32, mask: UnitVol) -> Self {
        Self {
            unit,
            generation,
            domain: mask.clone(),
            scan: UnitComponents::new(mask),
            cells: Vec::new(),
            cursor: 0,
            shape: crate::burst::chunk::Shape::default(),
        }
    }

    /// Shared bounded extraction. Staging only accumulates the section's
    /// statistics; the cut itself is taken from the source as it is at commit,
    /// so damage outside the domain applied meanwhile is never undone.
    pub(crate) fn advance(&mut self, source: &UnitBody, budget: &mut u32) {
        while *budget > 0 {
            if self.cells.is_empty() {
                let mut token = 1;
                let found = self.scan.advance(&mut token);
                *budget -= 1;
                if let Some(cells) = found.into_iter().next() {
                    self.cells = cells;
                } else if self.scan.finished() {
                    break;
                }
            } else if self.cursor < self.cells.len() {
                *budget -= 1;
                self.shape.add(self.cells[self.cursor], source.turret_y);
                self.cursor += 1;
            } else {
                break;
            }
        }
    }
    /// True for a cell that a pending whole section will carry away.
    pub(crate) fn holds(&self, c: Cell) -> bool {
        self.domain.get(c[0] as usize, c[1] as usize, c[2] as usize)
    }
    pub(crate) fn ready(&self) -> bool {
        !self.cells.is_empty() && self.cursor == self.cells.len()
    }
    pub(crate) fn finished(&self) -> bool {
        self.scan.finished() && self.cells.is_empty()
    }
    /// Cut the ready section out of the current source. Domain cells are
    /// untouched by anyone else, so every one of them is still present.
    pub(crate) fn commit(&mut self, source: &UnitBody) -> (Arc<UnitBody>, Separated) {
        assert!(self.ready());
        self.cursor = 0;
        let mut body = source.clone();
        for c in &self.cells {
            let [x, y, z] = c.map(usize::from);
            assert!(body.remove(x, y, z));
            self.domain.set(x, y, z, false);
        }
        (
            Arc::new(body),
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
        source.status(core);
        let mut allowed = source.vol.clone();
        for (w, r) in allowed.cols.iter_mut().zip(core.reached_cols()) {
            *w &= !r;
        }
        Self::load_domain(unit, generation, s, source, &allowed)
    }
    pub(crate) fn load_domain(
        unit: u32,
        generation: u32,
        s: &mut Scan,
        source: &UnitBody,
        uncommitted: &UnitVol,
    ) -> Option<Self> {
        let scan = UnitComponents::load(s)?;
        let cursor = s.usize();
        let count = s.list(3);
        let detached = uncommitted.solid_count() as usize;
        if scan.dims() != [source.vol.nx, source.vol.ny, source.vol.nz]
            || count > detached
            || cursor > count
        {
            return None;
        }
        if !scan.contained_in(uncommitted) {
            return None;
        }
        let domain = scan.mask();
        let mut j = Self {
            unit,
            generation,
            scan,
            cells: Vec::new(),
            cursor,
            shape: crate::burst::chunk::Shape::default(),
            domain,
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
            j.domain
                .set(c[0] as usize, c[1] as usize, c[2] as usize, true);
            if i < cursor {
                j.shape.add(c, source.turret_y);
            }
            j.cells.push(c);
        }
        // Damage outside the domain may have cut more cells loose since the
        // job began; those wait for the next job, so the count is a floor.
        (s.ok() && j.scan.pending_count() + count <= detached && !j.finished()).then_some(j)
    }
}
impl Bodies {
    pub fn detach_pending(&self) -> usize {
        self.detach.len()
    }
    /// One job per affected slot. A section becomes visible only whole and only
    /// with a physical fragment slot; within one tick a job keeps cutting
    /// further ready sections while its work share and the room last.
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
            let mut budget = left.div_ceil((n - k) as u32);
            let start = budget;
            loop {
                let source = state
                    .own
                    .as_ref()
                    .unwrap_or(&self.templates[state.template as usize]);
                job.advance(source, &mut budget);
                if !job.ready() || room == 0 {
                    break;
                }
                room -= 1;
                let (body, part) = job.commit(source);
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
                if budget == 0 {
                    break;
                }
            }
            left -= start - budget;
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
    fn shattered() -> Bodies {
        // A spine holding the core, plus three islands that the first cut frees.
        let mut body = UnitBody::new_scaled(80, 5, 5, 1, 100);
        for x in 0..4 {
            body.set(x, 2, 2, voxel::Part::Armour);
        }
        body.set(0, 2, 2, voxel::Part::Core);
        for x0 in [10, 20, 30] {
            for x in x0..x0 + 3 {
                body.set(x, 2, 2, voxel::Part::Armour);
            }
        }
        for x in 4..10 {
            body.set(x, 2, 2, voxel::Part::Armour);
        }
        body.seat_core();
        let mut b = Bodies::new(vec![body]);
        b.spawn_at(0, 0);
        b.queue_cells(0, b.generations[0], vec![[3, 2, 2]], 1000, 0, V3::ZERO, 16);
        assert_eq!(b.step(&[1; 5], 8, 8)[0].removed, [[3, 2, 2]]);
        assert_eq!(b.detach_pending(), 1);
        b
    }
    #[test]
    fn a_shattered_body_sheds_every_ready_section_within_one_tick() {
        let mut b = shattered();
        let sections = b.step_detach(1000, 8, 8, &[true]);
        assert_eq!(sections.len(), 3, "one tick, one job, all free pieces");
        assert_eq!(
            sections.iter().map(|s| s.cells.len()).collect::<Vec<_>>(),
            [9, 3, 3]
        );
        assert_eq!(b.body(0).unwrap().solid_count(), 3);
        assert_eq!(b.state(0).unwrap().status.detached, 0);
        assert_eq!(b.detach_pending(), 0);
        assert_eq!(b.state(0).unwrap().removed.len(), 1 + 9 + 3 + 3);
        // Room is still the ceiling: one slot, one section, the rest wait whole.
        let mut b = shattered();
        assert_eq!(b.step_detach(1000, 8, 1, &[true]).len(), 1);
        assert_eq!(b.detach_pending(), 1);
        assert_eq!(b.step_detach(1000, 8, 0, &[true]).len(), 0);
        assert_eq!(b.step_detach(1000, 8, 8, &[true]).len(), 2);
        assert_eq!(b.body(0).unwrap().solid_count(), 3);
    }
    #[test]
    fn a_section_is_cut_from_the_source_as_it_is_at_commit() {
        let b = shattered();
        let mut body = b.body(0).unwrap().clone();
        let mut mask = body.vol.clone();
        for (w, r) in mask.cols.iter_mut().zip(b.core.reached_cols()) {
            *w &= !r;
        }
        let mut job = Job::new(0, 0, mask);
        job.advance(&body, &mut 1000);
        assert!(job.ready());
        assert!(job.holds([10, 2, 2]) && !job.holds([1, 2, 2]));
        // Ordinary damage outside the domain lands while the section waits.
        assert!(body.remove(1, 2, 2));
        let (cut, part) = job.commit(&body);
        assert_eq!(part.cells.len(), 9);
        assert!(!cut.vol.get(1, 2, 2), "staged copy resurrected damage");
        assert!(part.cells.iter().all(|c| !cut.vol.get(c[0] as usize, 2, 2)));
        assert!(!job.holds([10, 2, 2]) && job.holds([20, 2, 2]));
        assert_eq!(cut.solid_count(), body.solid_count() - 9);
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
