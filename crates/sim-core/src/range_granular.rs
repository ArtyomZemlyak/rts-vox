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
pub(super) struct Flow {
    pub tuning: Tuning,
    queue: VecDeque<Work>,
    queued: Positions,
    revisit: BTreeSet<Pos>,
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
            queued: Positions::default(),
            revisit: BTreeSet::new(),
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
    fn wake_tile(&mut self, tile: Pos) {
        if self.queued.insert(tile) {
            self.queue.push_back(Work { tile, cursor: 0 });
        } else {
            self.revisit.insert(tile);
        }
    }
    fn wake(&mut self, p: Pos, size: Pos) {
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
        // Deduplicate before adding neighbour patches; nuclear edits are dense.
        let patches: BTreeSet<_> = edits
            .iter()
            .map(|e| [e.x / SIDE, e.y / SIDE, e.z / SIDE])
            .collect();
        // Discover changed cells before their (mostly empty) halo. Otherwise
        // a distant edited column can wait several ticks just for activation.
        for &tile in &patches {
            self.wake_tile(tile);
        }
        for tile in patches {
            let lo = tile.map(|v| v.saturating_sub(1));
            let hi: Pos = std::array::from_fn(|a| (tile[a] + 1).min((size[a] - 1) / SIDE));
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        self.wake_tile([x, y, z]);
                    }
                }
            }
        }
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
        b.u32(self.revisit.len() as u32);
        for p in &self.revisit {
            for &v in p {
                b.u32(v);
            }
        }
    }
    pub fn load_legacy(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        self.active.clear();
        self.active_positions.clear();
        self.active_dirty = false;
        self.moved = s.u64();
        self.pushed = s.u64();
        self.queue.clear();
        self.queued.clear();
        self.revisit.clear();
        let count = s.u32() as usize;
        if count > s.left() / 16 {
            return None;
        }
        for _ in 0..count {
            let tile = std::array::from_fn(|_| s.u32());
            let cursor = s.u32();
            if (0..3).any(|a| tile[a] >= size[a].div_ceil(SIDE))
                || cursor >= CELLS
                || !self.queued.insert(tile)
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
            if !self.queued.contains(&p) || last.is_some_and(|prev| prev >= p) {
                return None;
            }
            self.revisit.insert(p);
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
