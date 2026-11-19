//! Event-driven, conservative cellular flow; no particle per grain or map scan.
//! Repose/cohesion and impact yield are approximations, not continuum mechanics.
use super::membership::Positions;
use fx::{
    blob::{Blob, Scan},
    Fx,
};
use modding::{granular::Tuning, Registry};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use voxel::{bodies::Bodies, Edit, World};

// Scheduling patch, not a change of voxel size. Every moved cell stays exact.
#[path = "range_granular_erosion.rs"]
mod erosion;
#[path = "range_granular_motion.rs"]
mod motion;
#[path = "range_granular_slope.rs"]
mod slope;
use motion::Grain;
const SIDE: u32 = 4;
const CELLS: u32 = SIDE * SIDE * SIDE;
const DIRS: [[i32; 3]; 4] = [[-1, 0, 0], [1, 0, 0], [0, 0, -1], [0, 0, 1]];
type Pos = [u32; 3];
#[derive(Clone, Copy, Debug)]
struct Work {
    tile: Pos,
    cursor: u32,
}
struct TileSlice {
    ty: usize,
    tz6: usize,
    revisit_count: usize,
    /// Packed `(queued_bits, revisit_bits)` at `y * tz6 + z6`, bit `b = z & 63`.
    words: Vec<(u64, u64)>,
}
#[derive(Default)]
pub(super) struct TileQueueSet {
    slices: Vec<Option<TileSlice>>,
    default_ty: usize,
    default_tz6: usize,
    queued_len: usize,
    revisit_len: usize,
}
impl TileQueueSet {
    pub fn ensure_size(&mut self, size: Pos) {
        let tx = size[0].div_ceil(SIDE) as usize;
        let ty = size[1].div_ceil(SIDE).max(1) as usize;
        let tz6 = size[2].div_ceil(SIDE).div_ceil(64).max(1) as usize;
        if tx > self.slices.len() {
            self.slices.resize_with(tx, || None);
        }
        self.default_ty = self.default_ty.max(ty);
        self.default_tz6 = self.default_tz6.max(tz6);
    }
    #[inline]
    fn slice_mut(&mut self, p: Pos) -> (&mut TileSlice, usize) {
        let x = p[0] as usize;
        let y = p[1] as usize;
        let z6 = (p[2] as usize) >> 6;
        if x >= self.slices.len() {
            self.slices.resize_with(x + 1, || None);
        }
        let d_ty = self.default_ty.max(y + 1);
        let d_tz6 = self.default_tz6.max(z6 + 1);
        let slice = self.slices[x].get_or_insert_with(|| TileSlice {
            ty: d_ty,
            tz6: d_tz6,
            revisit_count: 0,
            words: vec![(0, 0); d_ty * d_tz6],
        });
        if y >= slice.ty || z6 >= slice.tz6 {
            let new_ty = slice.ty.max(y + 1);
            let new_tz6 = slice.tz6.max(z6 + 1);
            let mut new_words = vec![(0, 0); new_ty * new_tz6];
            for oy in 0..slice.ty {
                let src = oy * slice.tz6;
                let dst = oy * new_tz6;
                new_words[dst..dst + slice.tz6].copy_from_slice(&slice.words[src..src + slice.tz6]);
            }
            slice.ty = new_ty;
            slice.tz6 = new_tz6;
            slice.words = new_words;
        }
        let idx = y * slice.tz6 + z6;
        (slice, idx)
    }
    #[inline]
    pub fn wake_or_revisit(&mut self, p: Pos) -> bool {
        let bit = 1u64 << (p[2] & 63);
        let (slice, idx) = self.slice_mut(p);
        let word = &mut slice.words[idx];
        if word.0 & bit == 0 {
            word.0 |= bit;
            self.queued_len += 1;
            true
        } else {
            if word.1 & bit == 0 {
                word.1 |= bit;
                slice.revisit_count += 1;
                self.revisit_len += 1;
            }
            false
        }
    }
    pub fn insert_queued(&mut self, p: Pos) -> bool {
        let bit = 1u64 << (p[2] & 63);
        let (slice, idx) = self.slice_mut(p);
        let word = &mut slice.words[idx];
        if word.0 & bit == 0 {
            word.0 |= bit;
            self.queued_len += 1;
            true
        } else {
            false
        }
    }
    pub fn contains_queued(&self, p: &Pos) -> bool {
        let x = p[0] as usize;
        let y = p[1] as usize;
        let z6 = (p[2] as usize) >> 6;
        let Some(Some(slice)) = self.slices.get(x) else {
            return false;
        };
        if y >= slice.ty || z6 >= slice.tz6 {
            return false;
        }
        slice.words[y * slice.tz6 + z6].0 & (1u64 << (p[2] & 63)) != 0
    }
    pub fn insert_revisit(&mut self, p: Pos) {
        let bit = 1u64 << (p[2] & 63);
        let (slice, idx) = self.slice_mut(p);
        let word = &mut slice.words[idx];
        if word.1 & bit == 0 {
            word.1 |= bit;
            slice.revisit_count += 1;
            self.revisit_len += 1;
        }
    }
    #[inline]
    pub fn finish_tile(&mut self, p: Pos) -> bool {
        let x = p[0] as usize;
        let y = p[1] as usize;
        let z6 = (p[2] as usize) >> 6;
        let bit = 1u64 << (p[2] & 63);
        let Some(Some(slice)) = self.slices.get_mut(x) else {
            return false;
        };
        if y >= slice.ty || z6 >= slice.tz6 {
            return false;
        }
        let word = &mut slice.words[y * slice.tz6 + z6];
        if word.1 & bit != 0 {
            word.1 &= !bit;
            slice.revisit_count -= 1;
            self.revisit_len -= 1;
            debug_assert!(word.0 & bit != 0);
            true
        } else {
            if word.0 & bit != 0 {
                word.0 &= !bit;
                self.queued_len -= 1;
            }
            false
        }
    }
    pub fn revisit_len(&self) -> usize {
        self.revisit_len
    }
    pub fn clear(&mut self) {
        self.slices.clear();
        self.default_ty = 0;
        self.default_tz6 = 0;
        self.queued_len = 0;
        self.revisit_len = 0;
    }
    pub fn for_each_revisit(&self, mut f: impl FnMut(Pos)) {
        for (x, maybe_slice) in self.slices.iter().enumerate() {
            let Some(slice) = maybe_slice.as_ref().filter(|s| s.revisit_count > 0) else {
                continue;
            };
            for y in 0..slice.ty {
                let row = y * slice.tz6;
                for z6 in 0..slice.tz6 {
                    let mut w = slice.words[row + z6].1;
                    let z_base = (z6 as u32) << 6;
                    while w != 0 {
                        let b = w.trailing_zeros();
                        w &= w - 1;
                        f([x as u32, y as u32, z_base + b]);
                    }
                }
            }
        }
    }
}
pub(super) struct Flow {
    pub tuning: Tuning,
    queue: VecDeque<Work>,
    tiles: TileQueueSet,
    patch_scratch: Vec<Pos>,
    pub moved: u64,
    pub pushed: u64,
    pub scanned: u32,
    active: VecDeque<(Pos, Grain)>,
    active_positions: Positions,
    active_dirty: bool,
    gravity: Fx,
    pub path_checks: u32,
}
impl Flow {
    pub fn new() -> Self {
        let physics =
            crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("stock physics");
        Self {
            tuning: Tuning::stock(),
            queue: VecDeque::new(),
            tiles: TileQueueSet::default(),
            patch_scratch: Vec::new(),
            moved: 0,
            pushed: 0,
            scanned: 0,
            active: VecDeque::new(),
            active_positions: Positions::default(),
            active_dirty: false,
            gravity: crate::gravity_per_tick(&physics),
            path_checks: 0,
        }
    }
    pub fn pending(&self) -> usize {
        self.queue.len() + self.active.len()
    }
    #[inline]
    fn wake_tile(&mut self, tile: Pos) {
        if self.tiles.wake_or_revisit(tile) {
            self.queue.push_back(Work { tile, cursor: 0 });
        }
    }
    fn wake(&mut self, p: Pos, size: Pos) {
        self.tiles.ensure_size(size);
        let lo = p.map(|v| v.saturating_sub(1) / SIDE);
        let hi: Pos = std::array::from_fn(|a| (p[a] + 1).min(size[a] - 1) / SIDE);
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    self.wake_tile([x, y, z]);
                }
            }
        }
    }
    pub fn seed(&mut self, layout: Option<&modding::range::RangeLayout>) {
        let Some(layout) = layout else {
            return;
        };
        self.tiles.ensure_size(layout.size);
        for b in &layout.blocks {
            if self.tuning.material(b.material).is_none() {
                continue;
            }
            for z in b.min[2] as u32 / SIDE..=(b.max[2] as u32 - 1) / SIDE {
                for y in b.min[1] as u32 / SIDE..=(b.max[1] as u32 - 1) / SIDE {
                    for x in b.min[0] as u32 / SIDE..=(b.max[0] as u32 - 1) / SIDE {
                        self.wake_tile([x, y, z]);
                    }
                }
            }
        }
    }
    pub fn observe(&mut self, edits: &[Edit], size: Pos) {
        if edits.is_empty() {
            return;
        }
        self.tiles.ensure_size(size);
        // Deduplicate in canonical lexicographical Pos order before adding
        // neighbour patches; nuclear edits are dense.
        let mut patches = std::mem::take(&mut self.patch_scratch);
        patches.clear();
        patches.extend(edits.iter().map(|e| [e.x / SIDE, e.y / SIDE, e.z / SIDE]));
        patches.sort_unstable();
        patches.dedup();
        // Discover changed cells before their (mostly empty) halo. Otherwise
        // a distant edited column can wait several ticks just for activation.
        for &tile in &patches {
            self.wake_tile(tile);
        }
        let max_tile: Pos = std::array::from_fn(|a| (size[a] - 1) / SIDE);
        for &tile in &patches {
            let lo = tile.map(|v| v.saturating_sub(1));
            let hi: Pos = std::array::from_fn(|a| (tile[a] + 1).min(max_tile[a]));
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        self.wake_tile([x, y, z]);
                    }
                }
            }
        }
        patches.clear();
        self.patch_scratch = patches;
    }
    #[cfg(test)]
    pub fn step(&mut self, w: &mut World, blocked: impl Fn(Pos) -> bool) {
        self.step_moist(w, blocked, |_| 0);
    }
    pub fn step_moist(
        &mut self,
        w: &mut World,
        blocked: impl Fn(Pos) -> bool,
        repose_bonus: impl Fn(Pos) -> i32,
    ) {
        self.advance_grains(w, &blocked, &repose_bonus);
    }
    /// A falling cohesive body's pressure displaces neighbouring loose cells.
    /// A bounded push chain preserves all material instead of deleting a crater.
    pub fn impact(
        &mut self,
        w: &mut World,
        bodies: &Bodies,
        mats: &Registry,
        tick: u32,
        blocked: impl Fn(Pos) -> bool,
    ) {
        let size = [w.nx, w.ny, w.nz];
        let mut changes = BTreeMap::new();
        let mut contacts = self.tuning.contacts_per_tick;
        let mut remaining = self
            .tuning
            .contact_moves_per_tick
            .min(self.tuning.moves_per_tick);
        for b in &bodies.list {
            if b.fresh || b.resting || b.fallen == 0 || b.vy < Fx::ZERO || b.awaiting_removal(w) {
                continue;
            }
            let area = usize::from(b.sx) * usize::from(b.sz);
            let reach = b.fy.add(b.vy).add(self.gravity).to_int_trunc().max(1) as u32;
            if self.glancing_landing(b, w, reach.min(self.tuning.push_reach), &mut contacts) {
                continue;
            }
            let density = b
                .voxels
                .first()
                .and_then(|v| mats.get(v.mat))
                .map_or(0, |m| m.density);
            let pressure = u64::from(density)
                * u64::from(b.fallen)
                * b.voxels.len().div_ceil(area.max(1)) as u64;
            for offset in 0..area {
                if contacts == 0 || remaining == 0 {
                    break;
                }
                let Some(q) = b.floor_contact((tick as usize + offset) % area) else {
                    continue;
                };
                if q.iter().any(|&v| v < 1) {
                    continue;
                }
                // A fast body can land and bake within its next step. Sample
                // that swept floor now, rather than missing its sand contact
                // whenever it starts this tick more than one cell above it.
                let mut contact = None;
                for depth in 1..=reach.min(self.tuning.push_reach).min(q[1] as u32) {
                    if contacts == 0 {
                        break;
                    }
                    contacts -= 1;
                    let p = [q[0] as u32, q[1] as u32 - depth, q[2] as u32];
                    if !inside(p, size) || blocked(p) || w.has_pending_dig(p[0], p[1], p[2]) {
                        break;
                    }
                    if read(w, &changes, p) != 0 {
                        contact = Some(p);
                        break;
                    }
                }
                let Some(p) = contact else { continue };
                if !inside(p, size) || blocked(p) || w.has_pending_dig(p[0], p[1], p[2]) {
                    continue;
                }
                let Some(m) = self.tuning.material(read(w, &changes, p)) else {
                    continue;
                };
                if pressure < u64::from(m.yield_pressure) {
                    continue;
                }
                let mut best: Option<Vec<Pos>> = None;
                for d in DIRS {
                    let mut chain = vec![p];
                    for distance in 1..=self.tuning.push_reach {
                        let q = [
                            p[0] as i32 + d[0] * distance as i32,
                            p[1] as i32,
                            p[2] as i32 + d[2] * distance as i32,
                        ];
                        if q.iter().any(|&v| v < 1) {
                            break;
                        }
                        let q = q.map(|v| v as u32);
                        if !inside(q, size)
                            || w.on_frame(q[0], q[1], q[2])
                            || blocked(q)
                            || w.has_pending_dig(q[0], q[1], q[2])
                        {
                            break;
                        }
                        let id = read(w, &changes, q);
                        // Never refill a footprint column already cleared by
                        // an earlier contact of this same falling body.
                        let under_body = (q[0] as i64) >= b.ox
                            && (q[0] as i64) < b.ox + i64::from(b.sx)
                            && (q[2] as i64) >= b.oz
                            && (q[2] as i64) < b.oz + i64::from(b.sz);
                        if id == 0 && under_body {
                            continue;
                        }
                        chain.push(q);
                        if id == 0 {
                            if (chain.len() - 1) as u32 <= remaining
                                && best.as_ref().is_none_or(|old| {
                                    (chain.len(), linear(q, size))
                                        < (old.len(), linear(*old.last().unwrap(), size))
                                })
                            {
                                best = Some(chain);
                            }
                            break;
                        }
                        if self
                            .tuning
                            .material(id)
                            .is_none_or(|m| pressure < u64::from(m.yield_pressure))
                        {
                            break;
                        }
                    }
                }
                if let Some(chain) = best {
                    for pair in chain.windows(2).rev() {
                        let mat = read(w, &changes, pair[0]);
                        move_cell(&mut changes, pair[0], pair[1], mat);
                        remaining -= 1;
                        self.pushed += 1;
                        self.wake(pair[0], size);
                        self.wake(pair[1], size);
                    }
                }
            }
        }
        apply(w, changes);
    }
    pub fn save_legacy(&self, b: &mut Blob) {
        b.u64(self.moved);
        b.u64(self.pushed);
        b.u32(self.queue.len() as u32);
        for work in &self.queue {
            for v in work.tile {
                b.u32(v);
            }
            b.u32(work.cursor);
        }
        b.u32(self.tiles.revisit_len() as u32);
        self.tiles.for_each_revisit(|p| {
            for v in p {
                b.u32(v);
            }
        });
    }
    pub fn load_legacy(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        self.active.clear();
        self.active_positions.clear();
        self.active_dirty = false;
        self.moved = s.u64();
        self.pushed = s.u64();
        self.queue.clear();
        self.tiles.clear();
        self.tiles.ensure_size(size);
        let count = s.u32() as usize;
        if count > s.left() / 16 {
            return None;
        }
        for _ in 0..count {
            let tile = std::array::from_fn(|_| s.u32());
            let cursor = s.u32();
            if (0..3).any(|a| tile[a] >= size[a].div_ceil(SIDE))
                || cursor >= CELLS
                || !self.tiles.insert_queued(tile)
            {
                return None;
            }
            self.queue.push_back(Work { tile, cursor });
        }
        let count = s.u32() as usize;
        if count > self.queue.len() || count > s.left() / 12 {
            return None;
        }
        let mut last = None;
        for _ in 0..count {
            let p = std::array::from_fn(|_| s.u32());
            if !self.tiles.contains_queued(&p) || last.is_some_and(|prev| prev >= p) {
                return None;
            }
            self.tiles.insert_revisit(p);
            last = Some(p);
        }
        s.ok().then_some(())
    }
    pub fn has_state(&self) -> bool {
        !self.queue.is_empty() || !self.active.is_empty() || self.moved != 0 || self.pushed != 0
    }
}
fn inside(p: Pos, size: Pos) -> bool {
    (0..3).all(|a| p[a] < size[a])
}
fn linear(p: Pos, size: Pos) -> u64 {
    (u64::from(p[2]) * u64::from(size[1]) + u64::from(p[1])) * u64::from(size[0]) + u64::from(p[0])
}
fn read(w: &World, changes: &BTreeMap<Pos, u16>, p: Pos) -> u16 {
    changes
        .get(&p)
        .copied()
        .unwrap_or_else(|| w.material(p[0], p[1], p[2]))
}
fn move_cell(changes: &mut BTreeMap<Pos, u16>, p: Pos, to: Pos, mat: u16) {
    changes.insert(p, 0);
    changes.insert(to, mat);
}
fn apply(w: &mut World, changes: BTreeMap<Pos, u16>) {
    // All destinations were checked against this phase's overlay. Puts/digs
    // drain together, after mining credit; moving unrevealed ground is no sink.
    for (p, mat) in changes {
        w.queue_transfer(Edit::put(p[0], p[1], p[2], mat));
    }
    w.flush_transfers();
}
