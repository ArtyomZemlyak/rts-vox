//! Bounded, finite lateral fracture around a penetrator's channel (ADR-474).
use super::*;
use std::collections::{BTreeSet, VecDeque};

impl BlastRange {
    pub(super) fn kinetic_spall(
        &mut self,
        seeds: &[(usize, usize, [usize; 3])],
        work: u32,
        radius: u8,
        at: [i32; 3],
        config: &ExplosionConfig,
    ) {
        let mut weights = BTreeMap::<(usize, usize, [usize; 3]), u32>::new();
        // Stress travels through material connected to the drilled channel;
        // it cannot jump a hollow, cross into another unit, or bypass cover.
        // The radius is data-bounded to four local voxels, at most 129 visits
        // per seed. Pose aliases do not multiply either range or energy.
        for &(id, part, origin) in seeds {
            let u = &self.units[id];
            let body = if part == 0 {
                &u.body
            } else {
                &u.body.segments[part - 1].body
            };
            let size = [body.vol.nx, body.vol.ny, body.vol.nz].map(usize::from);
            let mut seen = BTreeSet::from([origin]);
            let mut queue = VecDeque::from([(origin, 0u8)]);
            while let Some((cell, depth)) = queue.pop_front() {
                if depth == radius {
                    continue;
                }
                for axis in 0..3 {
                    for offset in [-1, 1] {
                        let n = cell[axis] as i32 + offset;
                        if n < 0 || n >= size[axis] as i32 {
                            continue;
                        }
                        let mut next = cell;
                        next[axis] = n as usize;
                        if !seen.insert(next)
                            || !body.vol.get(next[0], next[1], next[2])
                            || u.section_holds(part, next.map(|v| v as u8))
                        {
                            continue;
                        }
                        let weight = u32::from(radius - depth).pow(2);
                        weights
                            .entry((id, part, next))
                            .and_modify(|w| *w = (*w).max(weight))
                            .or_insert(weight);
                        queue.push_back((next, depth + 1));
                    }
                }
            }
        }
        let total: u64 = weights.values().map(|w| u64::from(*w)).sum();
        if total == 0 {
            return;
        }
        // Every share is paid from one finite reserve. Rounding dissipates
        // unused work; no full-strength spherical blast is created.
        for ((id, part, cell), weight) in weights {
            let dose = (u64::from(work) * u64::from(weight) / total) as u32;
            if dose == 0 {
                continue;
            }
            let u = &mut self.units[id];
            let need = if part == 0 {
                unit_work(&u.body, cell, config)
                    .saturating_sub(u.dents.get(&cell.map(|v| v as u8)).copied().unwrap_or(0))
            } else {
                unit_work(&u.body.segments[part - 1].body, cell, config)
                    .saturating_sub(u.seg_dent(part - 1, cell))
            };
            if dose >= need {
                let result = if part == 0 {
                    self.matter.body(id, u, cell, &u.root_frame(cell[1]), at)
                } else {
                    self.matter
                        .body_segment(id, u, part - 1, cell, &u.seg_frames[part - 1], at)
                };
                if !matches!(result, matter::ClaimResult::Applied) {
                    continue;
                }
                if part == 0 {
                    u.detached += 1;
                } else {
                    u.seg_detached += 1;
                }
            }
            if part == 0 {
                u.receive_cell(cell, dose, config);
            } else {
                u.receive_seg_cell(part - 1, cell, dose, config);
            }
        }
    }
}
