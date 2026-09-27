//! Atomic grouping of actually removed body cells. No extra damage and no
//! welding across units, articulated parts, poses or subsystem boundaries.
use super::*;
use crate::burst::{fracture_box, shape_centre, shape_components};
use crate::damage::Frame;
use std::collections::BTreeMap;

type Key = (usize, usize, [u32; 3], u8, [i64; 16]);
#[derive(Default)]
pub(super) struct Batch(BTreeMap<Key, (Frame, Vec<(u32, usize)>)>);
impl Batch {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record(
        &mut self,
        index: usize,
        unit: usize,
        part: usize,
        cell: [usize; 3],
        frame: Frame,
        side: u8,
        tuning: &modding::tuning::Blast,
    ) {
        let p = cell.map(|v| v as u32);
        let (origin, side) = fracture_box(p, side, tuning);
        let s = u32::from(side);
        let bit = p[0] - origin[0] + s * (p[1] - origin[1]) + s * s * (p[2] - origin[2]);
        let mut pose = [0; 16];
        let mut n = 0;
        for v in [frame.base, frame.x, frame.y, frame.z] {
            for a in [v.x, v.y, v.z] {
                pose[n] = a.raw();
                n += 1;
            }
        }
        for a in frame.centre {
            pose[n] = a.raw();
            n += 1;
        }
        pose[n] = frame.scale.raw();
        self.0
            .entry((unit, part, origin, side, pose))
            .or_insert_with(|| (frame, Vec::new()))
            .1
            .push((bit, index));
    }
}

fn motion_mj(f: &Fragment) -> u64 {
    let mm_s = [f.velocity.x, f.velocity.y, f.velocity.z].map(|v| {
        (v.raw() as i128 * voxel::unit::WORLD_MM as i128 * crate::TICKS_PER_SECOND as i128
            / (1i128 << 32)) as i64
    });
    voxel::impact::kinetic_mj(f.mass_g.min(u32::MAX as u64) as u32, mm_s)
}

impl Matter {
    pub(crate) fn begin_body_batch(&mut self) {
        assert!(self.batch.is_none());
        self.batch = Some(Batch::default());
    }
    pub(crate) fn finish_body_batch(&mut self) {
        let Some(batch) = self.batch.take() else {
            return;
        };
        let mut removed = BTreeSet::new();
        for ((_, _, origin, side, _), (frame, cells)) in batch.0 {
            // Keep subsystem boundaries, but paint scratches do not sever steel.
            let mut roles = BTreeMap::<u8, (u64, Vec<(u32, usize)>)>::new();
            for (bit, index) in cells {
                let role = self.debris.fragments[index].paint.unwrap()[2];
                let group = roles.entry(role).or_default();
                group.0 |= 1 << bit;
                group.1.push((bit, index));
            }
            for (_, (whole, cells)) in roles {
                for shape in shape_components(whole, side) {
                    let indices: Vec<_> = cells
                        .iter()
                        .filter(|(bit, _)| shape & (1 << bit) != 0)
                        .map(|(_, index)| *index)
                        .collect();
                    if indices.len() < 2 {
                        continue;
                    }
                    let first = indices[0];
                    let held = self.debris.fragments[first].held;
                    let mut velocity = V3::ZERO;
                    let mut mass = 0;
                    let mut old_motion = 0;
                    for &i in &indices {
                        let f = &self.debris.fragments[i];
                        velocity = velocity.add(f.velocity);
                        mass += f.mass_g;
                        old_motion += motion_mj(f);
                        if i != first {
                            let container = f.held;
                            let amount = self.economy.ledger.held(container);
                            assert_eq!(
                                self.economy.ledger.transfer(container, held, amount),
                                amount
                            );
                            assert!(self.economy.ledger.retire_empty_debris(container));
                            removed.insert(i);
                        }
                    }
                    let centre = shape_centre(shape, side);
                    let local = [centre.x, centre.y, centre.z];
                    let f = &mut self.debris.fragments[first];
                    f.position = frame.world(std::array::from_fn(|a| {
                        Fx::from_int(origin[a] as i32)
                            .add(Fx::from_ratio(1, 2))
                            .add(local[a])
                    }));
                    // Equal-volume cells of one material: mean velocity preserves
                    // momentum and cannot increase kinetic energy.
                    f.velocity = velocity.scale(Fx::from_ratio(1, indices.len() as i32));
                    f.mass_g = mass;
                    f.count = shape.count_ones();
                    f.shape = shape;
                    f.side = side;
                    let dissipated = old_motion.saturating_sub(motion_mj(f));
                    self.debris.body_motion_mj =
                        self.debris.body_motion_mj.saturating_sub(dissipated);
                }
            }
        }
        if removed.is_empty() {
            return;
        }
        let mut i = 0;
        self.debris.fragments.retain(|_| {
            let keep = !removed.contains(&i);
            i += 1;
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn armour_batches_keep_matter_and_exact_connected_geometry() {
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/energy-cannon.ron"
        ))
        .unwrap();
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        let mut r = crate::blast::BlastRange::with_layout(config, layout).unwrap();
        let u = &r.units[0];
        let b = &u.body;
        let mut cells = Vec::new();
        for z in 0..usize::from(b.vol.nz) {
            for y in 0..usize::from(b.vol.ny) {
                for x in 0..usize::from(b.vol.nx) {
                    if b.vol.get(x, y, z) {
                        cells.push([x, y, z]);
                    }
                }
            }
        }
        cells.truncate(512);
        let m = &mut r.matter;
        m.begin(u32::MAX, 4);
        m.begin_body_batch();
        for &cell in &cells {
            assert!(matches!(
                m.body(0, u, cell, &u.root_frame(cell[1]), [200, 16, 330]),
                ClaimResult::Applied
            ));
        }
        let before = m.debris.fragments.len();
        let mass: u64 = m.debris.fragments.iter().map(|f| f.mass_g).sum();
        let held = m.economy.ledger.in_containers();
        m.finish_body_batch();
        let fragments = &m.debris.fragments;
        assert!(
            fragments.len() < before / 2,
            "{} fragments for {} cells",
            fragments.len(),
            before
        );
        assert!(fragments.iter().any(|f| f.count >= 16));
        assert!(fragments.iter().any(|f| f.count <= 4));
        assert_eq!(
            fragments.iter().map(|f| f.count as usize).sum::<usize>(),
            cells.len()
        );
        assert_eq!(fragments.iter().map(|f| f.mass_g).sum::<u64>(), mass);
        assert_eq!(m.economy.ledger.in_containers(), held);
        assert!(m.economy.ledger.balanced());
        let mut restored = BTreeSet::new();
        for f in fragments {
            assert_eq!(shape_components(f.shape, f.side), [f.shape]);
            let centre = shape_centre(f.shape, f.side);
            let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
            for bit in 0..u32::from(f.side).pow(3) {
                if f.shape & (1 << bit) == 0 {
                    continue;
                }
                let s = u32::from(f.side);
                let delta = [
                    Fx::from_int((bit % s) as i32).sub(centre.x),
                    Fx::from_int((bit / s % s) as i32).sub(centre.y),
                    Fx::from_int((bit / s / s) as i32).sub(centre.z),
                ];
                let mut world = f.position;
                for a in 0..3 {
                    world = world.add(f.basis[a].scale(delta[a].mul(scale)));
                }
                // Cells chosen first come from the lower hull in this fixture.
                let local = u
                    .root_frame(0)
                    .local(world)
                    .map(|v| v.to_int_trunc() as usize);
                assert!(restored.insert(local));
            }
        }
        assert_eq!(restored, cells.into_iter().collect());
    }
}
