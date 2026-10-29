//! Bounded cellular approximation of viscous, cooling terrain melt.
use super::*;
use voxel::Edit;

impl Heat {
    pub(crate) fn has_molten(&self) -> bool {
        !self.molten.is_empty()
    }
    #[cfg(test)]
    pub(crate) fn molten_cells(&self) -> impl Iterator<Item = [u32; 3]> + '_ {
        self.molten.iter().copied()
    }
    pub(crate) fn mark_molten(&mut self, p: [u32; 3]) {
        self.molten.insert(p);
    }

    /// Work is restricted to known molten cells and the thermal work budget.
    /// World material remains authoritative; the set is reconstructed on load.
    pub(crate) fn step_molten(
        &mut self,
        w: &mut World,
        t: &HeatTuning,
        tick: u32,
        blocked: impl Fn([u32; 3]) -> bool,
    ) {
        if self.molten.is_empty() {
            return;
        }
        let limit = t.cells_per_tick.min(self.molten.len());
        let cursor = self.cursor.unwrap_or([0; 3]);
        let batch: Vec<_> = self
            .molten
            .range(cursor..)
            .chain(self.molten.range(..cursor))
            .copied()
            .take(limit)
            .collect();
        let mut changes = BTreeMap::<[u32; 3], u16>::new();
        let mut moved = BTreeSet::new();
        let mut moves = 0usize;
        let divisor = self
            .molten
            .len()
            .div_ceil(t.melt_moves_per_tick.max(1))
            .max(2) as u64;
        for p in batch {
            if moved.contains(&p) {
                continue;
            }
            let mat = changes
                .get(&p)
                .copied()
                .unwrap_or_else(|| w.material(p[0], p[1], p[2]));
            let Some(flow) = t.melt_flow.iter().flatten().find(|f| f.liquid == mat) else {
                self.molten.remove(&p);
                continue;
            };
            if w.has_pending_dig(p[0], p[1], p[2]) {
                continue;
            }
            if self.cell(p).heat < flow.freeze {
                changes.insert(p, flow.solid);
                self.molten.remove(&p);
                if self.cell(p).heat == 0 {
                    self.cells.remove(&p);
                }
                continue;
            }
            if t.melt_moves_per_tick == 0 || moves >= t.melt_moves_per_tick {
                continue;
            }
            let h = u64::from(p[0]).wrapping_mul(73856093)
                ^ u64::from(p[1]).wrapping_mul(19349663)
                ^ u64::from(p[2]).wrapping_mul(83492791);
            if (h.wrapping_add(u64::from(tick))) % divisor != 0 {
                continue;
            }
            let empty = |q: [u32; 3], changes: &BTreeMap<[u32; 3], u16>| {
                inside(w, q)
                    && !w.on_frame(q[0], q[1], q[2])
                    && !w.has_pending_dig(q[0], q[1], q[2])
                    && !blocked(q)
                    && changes
                        .get(&q)
                        .copied()
                        .unwrap_or_else(|| w.material(q[0], q[1], q[2]))
                        == AIR
            };
            let down = [p[0], p[1].saturating_sub(1), p[2]];
            let mut dest = (p[1] > 0 && empty(down, &changes)).then_some(down);
            if dest.is_none() {
                // A supported lone drop has no horizontal momentum: it must
                // not random-walk across a flat floor until it freezes.
                // A column above supplies pressure for lateral spreading.
                let above=[p[0],p[1].saturating_add(1),p[2]];
                let pressurised=inside(w,above)
                    && changes.get(&above).copied().unwrap_or_else(||w.material(above[0],above[1],above[2]))==mat;
                let dirs = [[-1, 0], [1, 0], [0, -1], [0, 1]];
                let mut level=None;
                for i in 0..4 {
                    let d = dirs[(i + (h as usize + tick as usize) % 4) % 4];
                    let x = i64::from(p[0]) + i64::from(d[0]);
                    let z = i64::from(p[2]) + i64::from(d[1]);
                    if x < 0 || z < 0 {
                        continue;
                    }
                    let side = [x as u32, p[1], z as u32];
                    let diagonal = [side[0], side[1].saturating_sub(1), side[2]];
                    let downhill=side[1]>0 && empty(diagonal,&changes);
                    if (!downhill && !pressurised) || !empty(side,&changes) {continue;}
                    if downhill {
                        dest=Some(diagonal);
                        break;
                    }
                    if pressurised && level.is_none() {level=Some(side);}
                }
                if dest.is_none() {dest=level;}
            }
            let Some(q) = dest else { continue };
            changes.insert(p, AIR);
            changes.insert(q, mat);
            if let Some(cell) = self.cells.remove(&p) {
                self.cells.insert(q, cell);
            }
            self.molten.remove(&p);
            self.molten.insert(q);
            moved.insert(q);
            moves += 1;
        }
        for (p, mat) in changes {
            w.queue_transfer(Edit::put(p[0], p[1], p[2], mat));
        }
        w.flush_transfers();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::blob::{Blob, Scan};
    struct Floor;
    impl voxel::Generator for Floor {
        fn material(&self, _x: u32, y: u32, _z: u32) -> u16 {
            if y < 2 {
                1
            } else {
                0
            }
        }
    }
    fn world() -> World {
        World::new(24, 24, 24, 1024, Box::new(Floor))
    }
    fn prepare() -> (World, Heat, HeatTuning) {
        let mut w = world();
        w.queue_transfer(Edit::put(10, 10, 10, 13));
        w.flush_transfers();
        let mut h = Heat::default();
        h.add(&w, [10, 10, 10], 1000);
        h.mark_molten([10, 10, 10]);
        let t = HeatTuning::from_ron(include_str!("../../../data/heat.ron")).unwrap();
        (w, h, t)
    }
    fn mass(w: &World) -> usize {
        (1..23)
            .flat_map(|x| (1..23).flat_map(move |y| (1..23).map(move |z| [x, y, z])))
            .filter(|p| matches!(w.material(p[0], p[1], p[2]), 3 | 13))
            .count()
    }
    #[test]
    fn isolated_melt_does_not_drift_over_flat_ground_before_freezing() {
        let mats=modding::data::materials_from_ron(include_str!("../../../data/range-materials.ron")).unwrap();
        let t=HeatTuning::from_ron(include_str!("../../../data/heat.ron")).unwrap();
        for (liquid,solid) in [(13,3),(14,2),(16,12)] {
            let mut w=world();let p=[10,2,10];w.queue_transfer(Edit::put(p[0],p[1],p[2],liquid));w.flush_transfers();
            let mut h=Heat::default();h.add(&w,p,5000);h.mark_molten(p);
            for tick in 0..160 {h.step_molten(&mut w,&t,tick,|_|false);h.step(&mut w,&mats,&t,|_|false);}
            assert_eq!(w.material(p[0],p[1],p[2]),solid,"liquid {liquid} drifted before solidifying");
            assert!(h.molten.is_empty());
        }
    }
    #[test]
    fn molten_drop_descends_off_a_step_then_stays_on_flat_ground() {
        let mut w=world();let t=HeatTuning::from_ron(include_str!("../../../data/heat.ron")).unwrap();
        w.queue_transfer(Edit::put(10,2,10,2));w.queue_transfer(Edit::put(10,3,10,13));w.flush_transfers();
        let mut h=Heat::default();h.add(&w,[10,3,10],5000);h.mark_molten([10,3,10]);
        for tick in 0..20 {h.step_molten(&mut w,&t,tick,|_|false);}
        let positions:Vec<_>=h.molten_cells().collect();assert_eq!(positions.len(),1);
        let p=positions[0];assert_eq!(p[1],2,"melt must run off the higher support");
        assert_eq!(p[0].abs_diff(10)+p[2].abs_diff(10),1);
        for tick in 20..100 {h.step_molten(&mut w,&t,tick,|_|false);}
        assert_eq!(h.molten_cells().collect::<Vec<_>>(),vec![p],"the drop must settle once it reaches flat ground");
    }
    #[test]
    fn stacked_melt_spreads_under_its_own_weight_without_losing_material() {
        let mut w=world();let t=HeatTuning::from_ron(include_str!("../../../data/heat.ron")).unwrap();
        let mut h=Heat::default();
        for p in [[10,2,10],[10,3,10]] {w.queue_transfer(Edit::put(p[0],p[1],p[2],13));}
        w.flush_transfers();
        for p in [[10,2,10],[10,3,10]] {h.add(&w,p,5000);h.mark_molten(p);}
        for tick in 0..80 {h.step_molten(&mut w,&t,tick,|_|false);}
        let positions:Vec<_>=h.molten_cells().collect();
        assert_eq!(positions.len(),2);assert_eq!(mass(&w),2);
        assert!(positions.iter().all(|p|p[1]==2));
        assert!(positions.iter().any(|p|p[0]!=10||p[2]!=10),"a pile must still spread");
        assert!(positions.iter().all(|p|p[0].abs_diff(10)+p[2].abs_diff(10)<=1));
    }
    #[test]
    fn molten_steel_descends_cools_and_restores_deterministically() {
        let (mut a, mut heat, t) = prepare();
        let (mut b, _, _) = prepare();
        let mut saved = Blob::new();
        heat.save(&mut saved);
        let mut restored = Heat::load(&mut Scan::new(&saved.v), &b, &t).unwrap();
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/range-materials.ron"))
                .unwrap();
        let mut descended = false;
        for tick in 0..80 {
            for (w, h) in [(&mut a, &mut heat), (&mut b, &mut restored)] {
                h.step_molten(w, &t, tick, |p| p == [10, 9, 10]);
                h.step(w, &mats, &t, |_| false);
                assert_eq!(mass(w), 1, "material lost or duplicated");
            }
            assert_eq!(heat.cells, restored.cells);
            assert_eq!(heat.molten, restored.molten);
            for x in 1..23 {
                for y in 1..23 {
                    for z in 1..23 {
                        assert_eq!(a.material(x, y, z), b.material(x, y, z));
                        if a.material(x, y, z) == 13 && y < 10 {
                            descended = true;
                        }
                    }
                }
            }
            assert_eq!(a.material(10, 9, 10), 0, "blocked cell was occupied");
        }
        assert!(descended, "melt never flowed around obstacle");
        assert!(heat.molten.is_empty(), "melt never solidified");
        assert_eq!(mass(&a), 1);
    }
    #[test]
    fn dense_melt_respects_move_budget_without_losing_cells() {
        let mut w = World::new(48, 48, 48, 8192, Box::new(Floor));
        let mut h = Heat::default();
        let t = HeatTuning::from_ron(include_str!("../../../data/heat.ron")).unwrap();
        for x in 4..36 {
            for z in 4..36 {
                w.queue_transfer(Edit::put(x, 20, z, 13));
            }
        }
        w.flush_transfers();
        for x in 4..36 {
            for z in 4..36 {
                h.add(&w, [x, 20, z], 1000);
                h.mark_molten([x, 20, z]);
            }
        }
        for tick in 0..12 {
            let before = w.edits().len();
            h.step_molten(&mut w, &t, tick, |_| false);
            let changes = w.edits().len() - before;
            assert!(
                changes <= 2 * t.melt_moves_per_tick,
                "unbounded edits: {changes}"
            );
            assert_eq!(h.molten.len(), 1024);
            let world_count = (1..47)
                .flat_map(|x| (1..47).flat_map(move |y| (1..47).map(move |z| [x, y, z])))
                .filter(|p| w.material(p[0], p[1], p[2]) == 13)
                .count();
            assert_eq!(world_count, 1024);
        }
    }
}
