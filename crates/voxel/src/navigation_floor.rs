//! Shared requirements of the live surface, rather than a mask per unit.
//! A block summarizes 4x4 of the existing 8x8-column floor tiles. Its grade
//! includes each tile's joins to its left/upper neighbour. Outer bounds prove
//! permission; strictly contained bounds prove refusal; ambiguous edges fall
//! back to the exact corridor, without changing its answer.
use super::{Mat, World, FILL_THREADS};

const SIDE: u32 = 4;
pub(super) struct Block {
    key: [Mat; 2],
    bounds: [u16; 2],
    grade: u16,
    flat: Option<u16>,
    level: Option<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edit, Generator};
    struct Terrain;
    struct LevelAndForest;
    impl Generator for LevelAndForest {
        fn material(&self, x:u32, y:u32, z:u32)->Mat {
            if y < if x>=128 && z>=64 {8} else {16} {2}
            else if x<64 && x%13==0 && y<24 {4} else {0}
        }
    }
    fn level_reference(w:&World, lo:[i32;2], hi:[i32;2], key:[Mat;2], bare:bool)->Option<u16> {
        if lo.iter().any(|&n|n<0) || hi[0]>=w.nx as i32 || hi[1]>=w.nz as i32
            || (0..2).any(|a|lo[a]>hi[a]) {return None;}
        let mut level=None;
        for z in lo[1] as u32/8..=hi[1] as u32/8 {
            for x in lo[0] as u32/8..=hi[0] as u32/8 {
                let (profile,bounds,clear,_)=w.floor_tiles[(x+w.nx.div_ceil(8)*z) as usize].get()?;
                if *profile!=key {return None;}
                let h=if bare {(*clear)?} else {if bounds[0]!=bounds[1] {return None;} bounds[0]};
                if level.is_some_and(|old|old!=h) {return None;}
                level=Some(h);
            }
        }
        level
    }
    #[test]
    fn level_blocks_match_exact_tiles_at_edges_under_forest_and_after_edits() {
        let mut w=World::new(145,48,137,10000,Box::new(LevelAndForest));
        let check=|w:&World| {
            for x in [0,7,31,32,63,64,119,143] {
                for z in [0,7,31,32,63,64,129,135] {
                    for span in [0,1,8,31,32,70] {
                        let lo=[x,z]; let hi=[(x+span).min(144),(z+span).min(136)];
                        for key in [[4,5],[2,5],[0,0]] {
                            assert_eq!(w.prepared_flat_area(lo,hi,key),level_reference(w,lo,hi,key,true),"bare {lo:?} {hi:?}");
                            assert_eq!(w.prepared_level_floor_area(lo,hi,key),level_reference(w,lo,hi,key,false),"floor {lo:?} {hi:?}");
                        }
                    }
                }
            }
        };
        check(&w);
        w.prepare_navigation([4,5]); check(&w);
        assert_eq!(w.prepared_level_floor_area([0,0],[31,31],[4,5]),Some(16));
        assert_eq!(w.prepared_flat_area([0,0],[31,31],[4,5]),None);
        assert_eq!(w.prepared_flat_area([64,0],[95,31],[4,5]),Some(16));
        for (x,z) in [(31,31),(32,32),(63,64),(144,136)] {
            for (y,material) in [(20,4),(20,0),(16,2),(16,0)] {
                w.queue_edit(Edit::put(x,y,z,material));w.flush();check(&w);
                w.prepare_navigation([4,5]);check(&w);
            }
        }
        let mixed=World::new(145,48,137,10000,Box::new(LevelAndForest));
        mixed.floor_tile(0,0,[2,5]);
        mixed.prepare_navigation([4,5]);
        check(&mixed);
    }
    impl Generator for Terrain {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            let top = if (32..64).contains(&x) && (24..72).contains(&z) {
                3 + x / 16
            } else {
                12 + x / 16 + z / 24
            };
            if y < top {
                2
            } else if x % 7 == 0 && y < top + 8 {
                4
            } else {
                0
            }
        }
    }
    fn reference(w: &World, lo: [u32; 2], hi: [u32; 2], key: [Mat; 2], grade: bool) -> u16 {
        let mut bounds = [u16::MAX, 0];
        let mut required = 0;
        for z in lo[1]..hi[1] {
            for x in lo[0]..hi[0] {
                let h = w.floor_tile(x, z, key).unwrap();
                bounds[0] = bounds[0].min(h[0]);
                bounds[1] = bounds[1].max(h[1]);
                required = required.max(h[1] - h[0]);
                if x > lo[0] {
                    required = required.max(join(h, w.floor_tile(x - 1, z, key).unwrap()));
                }
                if z > lo[1] {
                    required = required.max(join(h, w.floor_tile(x, z - 1, key).unwrap()));
                }
            }
        }
        if grade {
            required
        } else {
            bounds[1] - bounds[0]
        }
    }
    fn compare(w: &World) {
        for key in [[4, 5], [2, 5], [0, 0]] {
            for z in 0..w.nz.div_ceil(8) {
                if key != [4, 5] && ![0, 3, w.nz.div_ceil(8) - 1].contains(&z) {
                    continue;
                }
                for x in 0..w.nx.div_ceil(8) {
                    if key != [4, 5] && ![0, 4, w.nx.div_ceil(8) - 1].contains(&x) {
                        continue;
                    }
                    for span in [1, 2, 4, 9, 20] {
                        let hi = [
                            (x + span).min(w.nx.div_ceil(8)),
                            (z + span + 1).min(w.nz.div_ceil(8)),
                        ];
                        for grade in [false, true] {
                            let expected = reference(w, [x, z], hi, key, grade);
                            for step in [-1, 0, 1, 4, 8, 32, i32::MAX] {
                                if let Some(answer) =
                                    w.prepared_floor_passage([x, z], hi, key, grade, step)
                                {
                                    assert_eq!(
                                        answer,
                                        i32::from(expected) <= step,
                                        "certificate {x},{z} {hi:?} {key:?} {grade} {step}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn capability_requirements_match_all_tile_joins_and_profiles() {
        let w = World::new(145, 48, 137, 10000, Box::new(Terrain));
        compare(&w); // cold then warm, includes partial edge blocks
        w.prepare_navigation([4, 5]);
        compare(&w);
        assert_eq!(
            w.prepared_floor_passage([1, 1], [1, 2], [4, 5], false, 4),
            None
        );
        assert_eq!(
            w.prepared_floor_passage([0, 0], [u32::MAX, 2], [4, 5], true, 4),
            None
        );
    }
    #[test]
    fn edits_invalidate_own_and_dependent_join_blocks_in_the_same_flush() {
        let mut w = World::new(145, 48, 137, 10000, Box::new(Terrain));
        w.prepare_navigation([4, 5]);
        let check = |w: &World| {
            for lo in [[0, 0], [3, 3], [4, 4], [0, 4], [4, 0], [7, 7]] {
                for grade in [false, true] {
                    let hi = [16, 16];
                    let expected = reference(w, lo, hi, [4, 5], grade);
                    for step in [0, 4, 12, 40] {
                        if let Some(answer) = w.prepared_floor_passage(lo, hi, [4, 5], grade, step)
                        {
                            assert_eq!(answer, i32::from(expected) <= step);
                        }
                    }
                }
            }
        };
        for (x, z) in [(31, 31), (32, 32), (63, 63), (95, 64), (72, 95)] {
            w.queue_edit(Edit::put(x, 40, z, 3));
            w.flush();
            check(&w);
            w.queue_edit(Edit::put(x, 40, z, 4));
            w.flush();
            check(&w); // same ceiling, new profile
            w.queue_edit(Edit::dig(x, 40, z));
            w.flush();
            check(&w);
        }
        compare(&w);
    }
    #[test]
    fn requirement_cache_does_not_change_snapshots_and_follows_restore() {
        let mut w = World::new(145, 48, 137, 10000, Box::new(Terrain));
        let mut before = fx::blob::Blob::new();
        w.save(&mut before);
        w.prepare_navigation([4, 5]);
        compare(&w);
        let mut warm = fx::blob::Blob::new();
        w.save(&mut warm);
        assert_eq!(before.v, warm.v);
        w.queue_edit(Edit::put(31, 40, 31, 3));
        w.flush();
        compare(&w);
        assert!(w.load(&mut fx::blob::Scan::new(&before.v)));
        compare(&w);
        w.queue_edit(Edit::put(63, 40, 63, 3));
        w.flush();
        compare(&w);
        w.restore_pristine();
        compare(&w);
    }
    #[test]
    fn capability_is_a_requirement_not_a_single_unit_tier() {
        struct Ramp;
        impl Generator for Ramp {
            fn material(&self, x: u32, y: u32, _z: u32) -> Mat {
                u16::from(y < 6 + x / 8)
            }
        }
        let w = World::new(160, 48, 160, 10000, Box::new(Ramp));
        w.prepare_navigation([4, 5]);
        let broad = reference(&w, [1, 1], [18, 18], [4, 5], false);
        let local = reference(&w, [1, 1], [18, 18], [4, 5], true);
        assert_eq!(
            w.prepared_floor_passage([1, 1], [18, 18], [4, 5], false, 2),
            Some(false)
        );
        assert_eq!(
            w.prepared_floor_passage([1, 1], [18, 18], [4, 5], true, 2),
            Some(true)
        );
        assert_eq!((broad, local), (16, 1));
        assert!(local <= 2 && broad > 2); // legs can follow this ramp, low track profile cannot
    }
    #[test]
    fn certificates_never_scan_cold_tiles_or_keep_a_stale_permission() {
        struct Flat;
        impl Generator for Flat {
            fn material(&self, _: u32, y: u32, _: u32) -> Mat {
                u16::from(y < 8)
            }
        }
        let mut w = World::new(160, 64, 160, 1000, Box::new(Flat));
        let lo = [1, 1];
        let hi = [18, 18];
        assert_eq!(w.prepared_floor_passage(lo, hi, [4, 5], true, 4), None);
        assert!(w.floor_tiles.iter().all(|tile| tile.get().is_none()));
        w.prepare_navigation([4, 5]);
        for local in [false, true] {
            assert_eq!(
                w.prepared_floor_passage(lo, hi, [4, 5], local, 4),
                Some(true)
            );
        }
        w.queue_edit(Edit::put(63, 40, 63, 3));
        w.flush();
        for local in [false, true] {
            assert_ne!(
                w.prepared_floor_passage(lo, hi, [4, 5], local, 4),
                Some(true)
            );
        }
        assert_eq!(w.prepared_floor_tile(7, 7, [4, 5]), None);
        w.floor_tile(7, 7, [4, 5]);
        for local in [false, true] {
            assert_eq!(
                w.prepared_floor_passage(lo, hi, [4, 5], local, 4),
                Some(false)
            );
        }
        w.queue_edit(Edit::dig(63, 40, 63));
        w.flush();
        assert_eq!(w.prepared_floor_passage(lo, hi, [4, 5], false, 4), None);
        w.floor_tile(7, 7, [4, 5]);
        assert_eq!(
            w.prepared_floor_passage(lo, hi, [4, 5], false, 4),
            Some(true)
        );
    }
}

fn join(a: [u16; 2], b: [u16; 2]) -> u16 {
    a[1].saturating_sub(b[0]).max(b[1].saturating_sub(a[0]))
}

impl World {
    /// Three-way surface classification. Some(true) proves passage for this
    /// step profile; Some(false) proves refusal; None asks for the exact tile
    /// test. Outer blocks can prove clearance, contained blocks can prove
    /// blockage. Partial boundaries never become false obstacles.
    pub fn prepared_floor_passage(
        &self,
        lo: [u32; 2],
        hi: [u32; 2],
        key: [Mat; 2],
        local_grade: bool,
        step: i32,
    ) -> Option<bool> {
        if lo[0] >= hi[0]
            || lo[1] >= hi[1]
            || hi[0] > self.nx.div_ceil(8)
            || hi[1] > self.nz.div_ceil(8)
        {
            return None;
        }
        if step < 0 {
            return Some(false);
        }
        let mut outer = [u16::MAX, 0];
        let mut inner = [u16::MAX, 0];
        let mut upper = 0;
        for z in lo[1] / SIDE..=(hi[1] - 1) / SIDE {
            for x in lo[0] / SIDE..=(hi[0] - 1) / SIDE {
                let (bounds, grade) = self.prepared_requirements_block(x, z, key)?;
                outer[0] = outer[0].min(bounds[0]);
                outer[1] = outer[1].max(bounds[1]);
                upper = upper.max(grade);
                if x * SIDE >= lo[0]
                    && z * SIDE >= lo[1]
                    && (x + 1) * SIDE <= hi[0]
                    && (z + 1) * SIDE <= hi[1]
                {
                    inner[0] = inner[0].min(bounds[0]);
                    inner[1] = inner[1].max(bounds[1]);
                    if (!local_grade && i32::from(inner[1] - inner[0]) > step)
                        || (local_grade
                            && x * SIDE > lo[0]
                            && z * SIDE > lo[1]
                            && i32::from(grade) > step)
                    {
                        return Some(false);
                    }
                }
            }
        }
        (i32::from(if local_grade {
            upper
        } else {
            outer[1] - outer[0]
        }) <= step)
            .then_some(true)
    }
    fn prepared_requirements_block(
        &self,
        x: u32,
        z: u32,
        key: [Mat; 2],
    ) -> Option<([u16; 2], u16)> {
        let i = (x + self.nx.div_ceil(32) * z) as usize;
        if let Some(b) = self.floor_blocks[i].get() {
            return (b.key == key).then_some((b.bounds, b.grade));
        }
        // A mutation invalidates the block at once. Rebuild only from already
        // refreshed child tiles: this fast path never starts extra voxel scans
        // beyond the metered corridor. The exact fallback warms needed tiles.
        let mut bounds = [u16::MAX, 0];
        let mut grade = 0;
        for tz in z * SIDE..((z + 1) * SIDE).min(self.nz.div_ceil(8)) {
            for tx in x * SIDE..((x + 1) * SIDE).min(self.nx.div_ceil(8)) {
                let h = self.prepared_floor_tile(tx, tz, key)?;
                bounds[0] = bounds[0].min(h[0]);
                bounds[1] = bounds[1].max(h[1]);
                grade = grade.max(h[1] - h[0]);
                if tx > 0 {
                    grade = grade.max(join(h, self.prepared_floor_tile(tx - 1, tz, key)?));
                }
                if tz > 0 {
                    grade = grade.max(join(h, self.prepared_floor_tile(tx, tz - 1, key)?));
                }
            }
        }
        let flat = self.prepared_block_level(x, z, key, true);
        let level = self.prepared_block_level(x, z, key, false);
        let b = self.floor_blocks[i].get_or_init(|| Block { key, bounds, grade, flat, level });
        (b.key == key).then_some((b.bounds, b.grade))
    }
    fn floor_block(&self, x: u32, z: u32, key: [Mat; 2]) -> Block {
        let mut bounds = [u16::MAX, 0];
        let mut grade = 0;
        for z in z * SIDE..((z + 1) * SIDE).min(self.nz.div_ceil(8)) {
            for x in x * SIDE..((x + 1) * SIDE).min(self.nx.div_ceil(8)) {
                let h = self.floor_tile(x, z, key).unwrap();
                bounds[0] = bounds[0].min(h[0]);
                bounds[1] = bounds[1].max(h[1]);
                grade = grade.max(self.floor_join(x, z, h, key));
            }
        }
        Block { key, bounds, grade, flat: self.prepared_block_level(x, z, key, true),
            level: self.prepared_block_level(x, z, key, false) }
    }
    fn prepared_block_level(&self, x: u32, z: u32, key: [Mat; 2], bare: bool) -> Option<u16> {
        let mut level = None;
        for tz in z * SIDE..((z + 1) * SIDE).min(self.nz.div_ceil(8)) {
            for tx in x * SIDE..((x + 1) * SIDE).min(self.nx.div_ceil(8)) {
                let i = (tx + self.nx.div_ceil(8) * tz) as usize;
                let (profile, bounds, clear, _) = self.floor_tiles[i].get()?;
                if *profile != key { return None; }
                let h = if bare { (*clear)? }
                    else { if bounds[0] != bounds[1] { return None; } bounds[0] };
                if level.is_some_and(|old| old != h) { return None; }
                level = Some(h);
            }
        }
        level
    }
    pub(super) fn prepared_level_block(&self, x: u32, z: u32, key: [Mat; 2], bare: bool) -> Option<u16> {
        let b = self.floor_blocks[(x + self.nx.div_ceil(32) * z) as usize].get()?;
        if b.key != key { return None; }
        if bare { b.flat } else { b.level }
    }
    fn floor_join(&self, x: u32, z: u32, h: [u16; 2], key: [Mat; 2]) -> u16 {
        let mut required = h[1] - h[0];
        if x > 0 {
            required = required.max(join(h, self.floor_tile(x - 1, z, key).unwrap()));
        }
        if z > 0 {
            required = required.max(join(h, self.floor_tile(x, z - 1, key).unwrap()));
        }
        required
    }
    fn requirements_block(&self, x: u32, z: u32, key: [Mat; 2]) -> ([u16; 2], u16) {
        let i = (x + self.nx.div_ceil(32) * z) as usize;
        let b = self.floor_blocks[i].get_or_init(|| self.floor_block(x, z, key));
        if b.key == key {
            (b.bounds, b.grade)
        } else {
            let b = self.floor_block(x, z, key);
            (b.bounds, b.grade)
        }
    }
    pub(super) fn prepare_floor_requirements(&self, key: [Mat; 2]) {
        let rows = self.nz.div_ceil(32);
        let band = rows.div_ceil(FILL_THREADS as u32).max(1);
        std::thread::scope(|scope| {
            for first in (0..rows).step_by(band as usize) {
                scope.spawn(move || {
                    for z in first..(first + band).min(rows) {
                        for x in 0..self.nx.div_ceil(32) {
                            self.requirements_block(x, z, key);
                        }
                    }
                });
            }
        });
    }
    pub(super) fn forget_floor_requirements(&mut self, x: u32, z: u32) {
        let width = self.nx.div_ceil(32);
        let bx = x / 32;
        let bz = z / 32;
        self.floor_blocks[(bx + width * bz) as usize].take();
        // Only the right/bottom blocks can include a changed external join.
        // Diagonal blocks share no axial edge with this tile.
        if x % 32 >= 24 && bx + 1 < width {
            self.floor_blocks[(bx + 1 + width * bz) as usize].take();
        }
        if z % 32 >= 24 && bz + 1 < self.nz.div_ceil(32) {
            self.floor_blocks[(bx + width * (bz + 1)) as usize].take();
        }
    }
}
