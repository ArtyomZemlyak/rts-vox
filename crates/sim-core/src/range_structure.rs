//! Structural attachment is independent of subsystem health and drive orders.
use super::*;
use voxel::topo::unit::{core_connectivity, UnitVol};

impl RangeUnit {
    pub(super) fn segment_loose_mask(&self, i: usize) -> UnitVol {
        let seg = &self.body.segments[i];
        let mut mask = seg.body.vol.clone();
        if self.seg_gone[i] {
            return mask;
        }
        // A finite weld around the authored pivot, clamped for pivots just
        // outside the volume (the sole's ankle is above its top surface).
        let pivot = [seg.attach.pivot.0, seg.attach.pivot.1, seg.attach.pivot.2];
        let dims = [mask.nx, mask.ny, mask.nz];
        let seed: [u8; 3] = std::array::from_fn(|a| pivot[a].min(dims[a] - 1).saturating_sub(1));
        let mut scratch = CoreBufs::new();
        core_connectivity(&mask, &mut scratch, (seed[0], seed[1], seed[2]), 3);
        for (w, r) in mask.cols.iter_mut().zip(scratch.reached_cols()) {
            *w &= !r;
        }
        mask
    }
    pub(super) fn refresh_segment_sections(&mut self, id: usize) {
        for i in 0..self.seg_sections.len() {
            if self.seg_sections[i].is_some() {
                continue;
            }
            let mask = self.segment_loose_mask(i);
            if mask.solid_count() > 0 {
                self.seg_sections[i] = Some(crate::damage::SectionJob::new(id as u32, 0, mask));
            }
        }
    }
    pub(super) fn section_holds(&self, part: usize, cell: [u8; 3]) -> bool {
        let job = if part == 0 {
            self.section.as_ref()
        } else {
            self.seg_sections.get(part - 1).and_then(Option::as_ref)
        };
        job.is_some_and(|j| j.holds(cell))
    }
}
