//! Dig economy of the range (plan 245, slices 0, 1, 3, 4 and 6; ADR-672,
//! ADR-673, ADR-674, ADR-675). Hauling to the docks and the sides' stores —
//! [`haul`]; reclaim of resting debris and dead diggers' tanks —
//! [`reclaim`]; production and produced units — [`produce`].
//!
//! A unit whose kind has a tank takes `Order::Dig(face)`. Within reach of
//! its face it queues the ball of `dig_radius` around it, a cell at a time,
//! into the world's one edit queue, each edit tagged [`DIG`] with its slot.
//! The drain credits what an edit actually removed: the ground reveals the
//! cell's matter and transfers it into the digger's tank. Matter is never
//! made: a reveal is the world's own volume × density, and a credit is a
//! transfer between two containers. A commander's fabricator is a separate,
//! explicitly accounted source; it never changes terrain voxels.
//!
//! **Room is reserved before the edit is queued.** A cell goes into the
//! queue only while the tank's room, less what is already in flight for it,
//! holds that cell's matter; the first cell that does not fit makes the
//! tank full, and a full tank stops digging (Q-56 default). The reservation
//! is per cell, so no two diggers queue one voxel.
//!
//! **Digging has a sub-pool of its own** inside the shared edit ceiling:
//! all diggers together keep at most `dig_edits_in_flight` edits waiting,
//! so a dig never starves combat edits. When the sub-pool runs out the
//! rest wait for the next tick, with the first digger served rotated by
//! tick, and the tick is counted (ADR-114).
use super::{battle, BlastRange, RangeUnit};
use crate::matter::{Amount, ContainerId, Kind, Ledger};
use fx::blob::{Blob, Scan};
use modding::range_economy::{KindEconomy, RangeEconomy};
use std::collections::BTreeMap;
use std::sync::Arc;
use voxel::store::AIR;

#[path = "range_construction.rs"]
pub mod construction;
#[path = "range_haul.rs"]
pub mod haul;
#[path = "range_produce.rs"]
pub mod produce;
#[path = "range_reclaim.rs"]
pub mod reclaim;
#[cfg(test)]
#[path = "range_economy_tests.rs"]
mod tests;

/// Requester tag of a dig edit: this prefix with the digger's slot. Below
/// the compare-and-remove range (`0xfffe0000`), the fire tag and NOBODY.
pub const DIG: u32 = 0xfffd_0000;
const TAG: u32 = 0xffff_0000;

// All crawlers share this per-tick budget for tracing their cutting faces.
// The remaining work waits for a later tick, never bypassing solid cover.
const DRILL_RAY_STEPS: usize = 32_768;

/// Highest remaining ground under the entire circular cutting head. Using
/// only the centre column lets a wide head descend through solid ground.
fn bore_surface(world: &voxel::World, x: i32, z: i32, radius: i32, tip_y: i32) -> i32 {
    let mut top = 1u32;
    let r = radius.max(0);
    for dz in -r..=r {
        let row_z = z + dz;
        if row_z < 0 || row_z >= world.nz as i32 { continue; }
        let extent = ((r * r - dz * dz) as u32).isqrt() as i32;
        let min_x = (x - extent).max(0) as u32;
        let max_x = (x + extent + 1).clamp(0, world.nx as i32) as u32;
        if min_x < max_x {
            top = top.max(world.column_box_ceiling([min_x, row_z as u32], [max_x, row_z as u32 + 1]));
        }
        if top >= tip_y.max(1) as u32 { return tip_y.max(1); }
    }
    (top as i32).min(tip_y.max(1))
}

#[cfg(test)]
fn drill_front(
    world: &voxel::World,
    from: [i32; 3],
    target: [i32; 3],
    budget: &mut usize,
) -> Option<[i32; 3]> {
    if from.iter().all(|v| *v >= 0) && world.solid(from[0] as u32, from[1] as u32, from[2] as u32) {
        return Some(from);
    }
    let mut found = None;
    let mut exhausted = false;
    voxel::los::walk(
        (from[0], from[1], from[2]),
        (target[0], target[1], target[2]),
        |x, y, z| {
            if *budget == 0 {
                exhausted = true;
                return false;
            }
            *budget -= 1;
            if x >= 0 && y >= 0 && z >= 0 && world.solid(x as u32, y as u32, z as u32) {
                found = Some([x, y, z]);
                false
            } else {
                true
            }
        },
    );
    if exhausted {
        None
    } else {
        Some(found.unwrap_or(target))
    }
}

fn drill_path_clear(
    world: &voxel::World,
    from: [i32; 3],
    target: [u32; 3],
    budget: &mut usize,
) -> Option<bool> {
    let mut clear = true;
    let mut exhausted = false;
    voxel::los::walk(
        (from[0], from[1], from[2]),
        (target[0] as i32, target[1] as i32, target[2] as i32),
        |x, y, z| {
            if *budget == 0 {
                exhausted = true;
                return false;
            }
            *budget -= 1;
            if x >= 0 && y >= 0 && z >= 0 && world.solid(x as u32, y as u32, z as u32) {
                clear = false;
                false
            } else {
                true
            }
        },
    );
    if exhausted {
        None
    } else {
        Some(clear)
    }
}

/// The digger a dig edit was queued for.
pub fn dig_slot(by: u32) -> Option<usize> {
    (by & TAG == DIG).then_some((by & !TAG) as usize)
}

/// A voxel queued for a digger: what it held and the matter set aside for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reservation {
    slot: u16,
    material: u16,
    amount: Amount,
}

/// A digger at its face this tick: its kind's entry, the dig edits it has
/// left this tick and whether its tank is full.
#[derive(Clone, Copy, Debug)]
struct Digger {
    id: usize,
    face: [i32; 3],
    k: KindEconomy,
    rate: u32,
    full: bool,
    /// Nearby debris may be claimed, waiting for search, or still in work.
    reclaim_pending: bool,
    reclaim_only: bool,
}

/// Cosmetic samples of completed transfers. They are reset each tick and
/// intentionally excluded from simulation saves and accounting.
#[derive(Clone, Copy, Debug)]
pub struct ReclaimVisual {
    pub from: [i32; 3],
    pub crawler: usize,
    pub material: u16,
}

/// The active setting of the crawler's shared cutting head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrawlerToolMode { Drill, Reclaim }
const RECLAIM_VISUAL_LIMIT: usize = 32;
const CRAWLER_RECLAIM_REACH: u32 = 160;
/// Continue a salvage order across the nearby wreck field before unloading.
// World cells are 10 cm: continue within 32 m rather than chasing debris
// across the whole battlefield before ever delivering a partial load.
const CRAWLER_RECLAIM_CHAIN_REACH: i64 = 320;

/// Horizontal distance to a wreck's enclosing footprint, rather than its
/// centre. This permits a large collector to work from a reachable edge.
fn wreck_edge_distance2(point: [i32; 3], bounds: ([i32; 3], [i32; 3])) -> i64 {
    [0usize, 2].into_iter().map(|a| {
        let nearest = point[a].clamp(bounds.0[a], bounds.1[a]);
        i64::from(point[a] - nearest).pow(2)
    }).sum()
}

fn reclaim_hull_in_reach(units: &[RangeUnit], id: usize, face: [i32; 3], reach: u32) -> bool {
    let pos = units[id].motion.pos.map(fx::Fx::to_int_trunc);
    units.iter().enumerate().any(|(other, u)| {
        if other == id || u.status.alive || !reclaimable_hull(u) { return false; }
        let bounds = u.bounds();
        wreck_edge_distance2(face, bounds) <= i64::from(CRAWLER_RECLAIM_REACH).pow(2)
            && wreck_edge_distance2(pos, bounds) <= i64::from(reach).pow(2)
    })
}

fn reclaimable_hull(u: &RangeUnit) -> bool {
    u.body.solid_count() > 0 || u.body.segments.iter().any(|s| s.body.solid_count() > 0)
}

fn reclaim_piece_bounds(f: &crate::burst::Fragment) -> ([i32;3],[i32;3]) {
    let (lo,hi)=f.bounds();
    let at=[f.position.x,f.position.y,f.position.z];
    // Conservative integer box of the actual rotated, shape-preserving piece.
    (std::array::from_fn(|a|at[a].add(lo[a]).to_int_trunc()-1),
     std::array::from_fn(|a|at[a].add(hi[a]).to_int_trunc()+1))
}

fn reclaim_edge_approach(unit:&RangeUnit,(lo,hi):([i32;3],[i32;3]),reach:u32)->[i32;2] {
    let pos=unit.motion.pos.map(fx::Fx::to_int_trunc);
    let edge=[pos[0].clamp(lo[0],hi[0]),pos[2].clamp(lo[2],hi[2])];
    let dx=i64::from(edge[0])-i64::from(pos[0]);let dz=i64::from(edge[1])-i64::from(pos[2]);
    let dist=((dx*dx+dz*dz) as u64).isqrt().max(1) as i64;
    // Reach from the collector's centre leaves clearance for its long head.
    let back=i64::from(reach).saturating_sub(4).max(i64::from(reach)*3/4).min(dist);
    [edge[0]-(dx*back/dist) as i32,edge[1]-(dz*back/dist) as i32]
}

fn reclaim_piece_at_face(f: &crate::burst::Fragment,face:[i32;3])->bool {
    let at=[f.position.x,f.position.y,f.position.z].map(fx::Fx::to_int_trunc);
    f.resting && (0..3).map(|a|(i64::from(at[a])-i64::from(face[a])).pow(2)).sum::<i64>()
        <= i64::from(CRAWLER_RECLAIM_REACH).pow(2)
}

fn reclaim_piece_in_reach(fragments:&[crate::burst::Fragment],u:&RangeUnit,face:[i32;3],reach:u32)->bool {
    let pos=u.motion.pos.map(fx::Fx::to_int_trunc);
    fragments.iter().any(|f|reclaim_piece_at_face(f,face)
        && wreck_edge_distance2(pos,reclaim_piece_bounds(f))<=i64::from(reach).pow(2))
}

fn next_reclaim_target(
    units: &[RangeUnit], matter: &super::matter::Matter, id: usize,
    face: [i32; 3], room: Amount,
) -> Option<[i32; 3]> {
    let origin = units[id].motion.pos.map(fx::Fx::to_int_trunc);
    let distance = |a: [i32; 3], b: [i32; 3]| -> i64 {
        (0..3).map(|axis| (i64::from(a[axis]) - i64::from(b[axis])).pow(2)).sum()
    };
    let valid = |at| distance(face, at) > i64::from(CRAWLER_RECLAIM_REACH).pow(2)
        && distance(origin, at) <= CRAWLER_RECLAIM_CHAIN_REACH.pow(2);
    let pieces = matter.debris.fragments.iter().filter_map(|f| {
        let at = [f.position.x, f.position.y, f.position.z].map(fx::Fx::to_int_trunc);
        let held = matter.economy.ledger.held(f.held);
        (f.resting && held > 0 && held <= room && valid(at))
            .then_some(((distance(origin, at), 0u8, f.id), at))
    });
    let hulls = units.iter().enumerate().filter_map(|(other, u)| {
        let at = u.motion.pos.map(fx::Fx::to_int_trunc);
        (other != id && !u.status.alive && valid(at)
            && matter.economy.ledger.held(matter.body_container(other)) > 0
            && room >= matter.body_min_quantum(other) && reclaimable_hull(u))
            .then_some(((distance(origin, at), 1u8, other as u64), at))
    });
    pieces.chain(hulls).min_by_key(|(key, _)| *key).map(|(_, at)| at)
}

// Disassemble a dead hull directly into the hold. Geometry and the body
// ledger move together; no temporary fragment pool or loss of material.
fn reclaim_hulls(
    m: &mut Mining,
    units: &mut [RangeUnit],
    matter: &mut super::matter::Matter,
    diggers: &mut [Digger],
) {
    let Some(rc) = m.reclaim.as_ref() else {
        return;
    };
    let reach = i64::from(CRAWLER_RECLAIM_REACH.max(m.data.reclaim.as_ref().unwrap().reach));
    let _ = rc;
    for d in diggers.iter_mut().filter(|d| d.reclaim_only) {
        let target = units
            .iter()
            .enumerate()
            .filter(|(id, u)| {
                *id != d.id
                    && !u.status.alive
                    && matter.economy.ledger.held(matter.body_container(*id)) > 0
                    && reclaimable_hull(u)
            })
            .filter_map(|(id, u)| {
                let bounds = u.bounds();
                let dist = wreck_edge_distance2(d.face, bounds);
                let collector = units[d.id].motion.pos.map(fx::Fx::to_int_trunc);
                (dist <= reach * reach && wreck_edge_distance2(collector, bounds) <= reach * reach)
                    .then_some((dist, id))
            })
            .min();
        let Some((hull_distance, id)) = target else {
            continue;
        };
        // Respect the clicked point across both types, instead of always
        // consuming a nearby hull before a closer selected fragment.
        let available = matter.economy.ledger.room(m.tank(d.id).unwrap()).saturating_sub(m.inflight[d.id]);
        let closer_piece = matter
            .debris
            .fragments
            .iter()
            .filter(|f| f.resting)
            .any(|f| {
                let p = [f.position.x, f.position.y, f.position.z].map(fx::Fx::to_int_trunc);
                let dist = (0..3)
                    .map(|a| i64::from(p[a] - d.face[a]).pow(2))
                    .sum::<i64>();
                if dist > reach * reach || dist > hull_distance {
                    return false;
                }
                let held = matter.economy.ledger.held(f.held);
                held > 0 && held <= available
            });
        if closer_piece {
            continue;
        }
        d.reclaim_pending = true;
        d.rate = 0;
        let tank = m.tank(d.id).unwrap();
        let room = matter
            .economy
            .ledger
            .room(tank)
            .saturating_sub(m.inflight[d.id]);
        let limit = (room / matter.body_min_quantum(id)).min(1024) as usize;
        if limit == 0 {
            d.full = true;
            m.mark_full(d.id);
            continue;
        }
        let collector = units[d.id].motion.pos.map(fx::Fx::to_int_trunc);
        let u = &mut units[id];
        // Existing section/shed jobs own their cells until they complete.
        if u.section.is_some()
            || !u.seg_shed.is_empty()
            || u.seg_sections.iter().any(Option::is_some)
        {
            continue;
        }
        let mut cells = Vec::with_capacity(limit);
        for part in 0..=u.body.segments.len() {
            let (b, frame) = if part == 0 {
                (&u.body, u.pose)
            } else {
                (&u.body.segments[part - 1].body, u.seg_frames[part - 1])
            };
            let local = frame.local(phys::V3::new(
                fx::Fx::from_int(collector[0]), fx::Fx::from_int(collector[1]), fx::Fx::from_int(collector[2])));
            let nx = usize::from(b.vol.nx);
            let nz = usize::from(b.vol.nz);
            let dx = local[0].sub(fx::Fx::from_ratio(nx as i32, 2));
            let dz = local[2].sub(fx::Fx::from_ratio(nz as i32, 2));
            let x_major = dx.abs().mul(fx::Fx::from_int(nz as i32)) >= dz.abs().mul(fx::Fx::from_int(nx as i32));
            let (primary, secondary) = if x_major { (nx, nz) } else { (nz, nx) };
            'outer: for first in 0..primary {
                for second in 0..secondary {
                    let (x, z) = if x_major { (first, second) } else { (second, first) };
                    let x = if dx > fx::Fx::ZERO { nx - 1 - x } else { x };
                    let z = if dz > fx::Fx::ZERO { nz - 1 - z } else { z };
                    for y in 0..usize::from(b.vol.ny) {
                        if b.vol.get(x, y, z) { cells.push((part, [x, y, z])); }
                        if cells.len() == limit { break 'outer; }
                    }
                }
            }
            if cells.len() == limit { break; }
        }
        if cells.is_empty() {
            d.reclaim_pending = false;
            continue;
        }
        let mut amount = 0u64;
        cells.retain(|(part, _)| {
            let quantum = matter.body_quantum(id, *part);
            if quantum <= room.saturating_sub(amount) { amount += quantum; true } else { false }
        });
        if cells.is_empty() { d.full = true; m.mark_full(d.id); continue; }
        let count = cells.len();
        let samples = count
            .min(8)
            .min(RECLAIM_VISUAL_LIMIT.saturating_sub(m.reclaim_events.len()));
        for sample in 0..samples {
            let (part, cell) = cells[sample * count / samples];
            let from = if part == 0 {
                u.world_pos(cell)
            } else {
                let v = u.seg_frames[part - 1]
                    .world(cell.map(|v| fx::Fx::from_int(v as i32).add(fx::Fx::from_ratio(1, 2))));
                [v.x, v.y, v.z].map(fx::Fx::to_int_trunc)
            };
            m.reclaim_events.push(ReclaimVisual {
                from,
                crawler: d.id,
                material: 3,
            });
        }
        assert_eq!(
            matter
                .economy
                .ledger
                .transfer(matter.body_container(id), tank, amount),
            amount
        );
        for (part, cell) in cells {
            let key = cell.map(|v| v as u8);
            if part == 0 {
                assert!(u.body.remove(cell[0], cell[1], cell[2]));
                u.removed.push(key);
                u.dents.remove(&key);
                u.detached += 1;
            } else {
                assert!(u.body.segments[part - 1]
                    .body
                    .remove(cell[0], cell[1], cell[2]));
                u.seg_removed.push(((part - 1) as u8, key));
                u.seg_dents.remove(&((part - 1) as u8, key));
                u.seg_detached += 1;
            }
        }
        u.revision += 1;
        u.refresh_damage(id);
        if let Some(rc) = m.reclaim.as_mut() {
            rc.reclaimed += amount;
        }
    }
}

/// The economy's state on a range whose layout has an economy section.
pub struct Mining {
    data: RangeEconomy,
    /// Exact X spans, by configured radius. Derived geometry prepared once,
    /// never a cached answer about the mutable world's occupancy.
    balls: BTreeMap<u32, Arc<[[i32; 3]]>>,
    /// Tank of each unit slot; `None` for a kind that does not dig.
    tanks: Vec<Option<ContainerId>>,
    /// Voxels queued for digging and not yet drained, by cell.
    reserved: BTreeMap<[u32; 3], Reservation>,
    /// Matter reserved in flight per slot; derived from `reserved`.
    inflight: Vec<Amount>,
    /// Matter credited to tanks, voxels credited, dig edits drained with
    /// nothing to credit, ticks the sub-pool ran out with work waiting.
    pub mined: Amount,
    pub cells: u64,
    pub misses: u64,
    pub deferred: u64,
    /// Hauling and the sides' stores, with a bank section (ADR-673).
    haul: Option<haul::Haul>,
    /// Reclaim, with a reclaim section (ADR-674).
    reclaim: Option<reclaim::Reclaim>,
    reclaim_events: Vec<ReclaimVisual>,
    /// Production, with a production section (ADR-675).
    production: Option<produce::Production>,
    /// Typed drone batches and block construction, with stores.
    construction: Option<construction::Construction>,
}

impl Mining {
    /// Tanks are created after every other container of the scene, so the
    /// numbering of bodies and fuel is that of a layout without economy.
    /// With a bank section the enemy side's store follows the tanks;
    /// container 1, the allied store, only takes its cap.
    pub(super) fn new(
        data: RangeEconomy,
        units: &[RangeUnit],
        sides: Vec<bool>,
        ledger: &mut Ledger,
        allied_store: ContainerId,
    ) -> Self {
        let mut balls = BTreeMap::new();
        for k in &data.kinds {
            if k.tank > 0 {
                balls
                    .entry(k.dig_radius)
                    .or_insert_with(|| Arc::from(ball_spans(k.dig_radius as i32)));
            }
        }
        let tanks = units
            .iter()
            .map(|u| {
                data.kind(u.kind)
                    .filter(|k| k.tank > 0)
                    .map(|k| ledger.create(Kind::Hauler, 0, k.tank))
            })
            .collect();
        let construction = data
            .bank
            .is_some()
            .then(|| { let mut c = construction::Construction::new(units, &sides); c.set_rules(data.balance.clone()); c });
        let haul = data
            .bank
            .is_some()
            .then(|| haul::Haul::new(&data, units, sides, ledger, allied_store));
        let reclaim = data.reclaim.map(|r| reclaim::Reclaim::new(r, units.len()));
        let production = data.production.as_ref().map(produce::Production::new);
        Self {
            data,
            balls,
            tanks,
            reserved: BTreeMap::new(),
            inflight: vec![0; units.len()],
            mined: 0,
            cells: 0,
            misses: 0,
            deferred: 0,
            haul,
            reclaim,
            reclaim_events: Vec::new(),
            production,
            construction,
        }
    }
    /// A unit appended to the roster (ADR-675): its tank, if its kind digs,
    /// and an empty slot in every per-unit list.
    pub(super) fn push_unit(&mut self, allied: bool, tank: Option<ContainerId>) {
        self.tanks.push(tank);
        self.inflight.push(0);
        if let Some(h) = self.haul.as_mut() {
            h.push_unit(allied);
        }
        if let Some(rc) = self.reclaim.as_mut() {
            rc.push_unit();
        }
    }
    pub fn reclaim_visuals(&self) -> &[ReclaimVisual] {
        &self.reclaim_events
    }
    pub fn data(&self) -> &RangeEconomy {
        &self.data
    }
    /// Whether a unit of this kind digs.
    pub fn digs(&self, kind: modding::range::RangeUnitKind) -> bool {
        self.data.digs(kind)
    }
    pub fn tank(&self, id: usize) -> Option<ContainerId> {
        self.tanks.get(id).copied().flatten()
    }
    /// Dig edits waiting in the world's queue.
    pub fn in_flight(&self) -> usize {
        self.reserved.len()
    }
    /// Matter reserved for edits of this digger still in the queue.
    pub fn reserved(&self, id: usize) -> Amount {
        self.inflight.get(id).copied().unwrap_or(0)
    }
    pub(super) fn save(&self, b: &mut Blob) {
        b.u64(self.mined);
        b.u64(self.cells);
        b.u64(self.misses);
        b.u64(self.deferred);
        b.u32(self.reserved.len() as u32);
        for (cell, r) in &self.reserved {
            for v in cell {
                b.u32(*v);
            }
            b.u16(r.slot);
            b.u16(r.material);
            b.u64(r.amount);
        }
    }
}

/// Offsets of the ball of radius `r`, top layer first, then by z and x:
/// a fixed order, so ties go by linear index.
#[cfg(test)]
fn ball(r: i32) -> impl Iterator<Item = [i32; 3]> {
    (-r..=r).rev().flat_map(move |dy| {
        (-r..=r).flat_map(move |dz| {
            (-r..=r).filter_map(move |dx| {
                (dx * dx + dy * dy + dz * dz <= r * r).then_some([dx, dy, dz])
            })
        })
    })
}

/// `(dy, dz, half_width)` in the original ball's order. A radius is
/// prepared outside the tick and shared by every kind that uses it.
fn ball_spans(r: i32) -> Vec<[i32; 3]> {
    let mut spans = Vec::new();
    for dy in (-r..=r).rev() {
        for dz in -r..=r {
            let squared = r * r - dy * dy - dz * dz;
            if squared < 0 {
                continue;
            }
            let mut half = r;
            while half * half > squared {
                half -= 1;
            }
            spans.push([dy, dz, half]);
        }
    }
    spans
}

/// Read this row's current occupancy as at most two 64-bit words (the
/// largest diameter is 65). The returned iterator owns its bits so edits
/// may be queued during iteration. Bits are visited from low to high X;
/// frame cells are excluded exactly as in the original scalar walk.
fn occupied_row(
    world: &voxel::World,
    face: [i32; 3],
    [dy, dz, half]: [i32; 3],
) -> impl Iterator<Item = [u32; 3]> {
    let y = i64::from(face[1]) + i64::from(dy);
    let z = i64::from(face[2]) + i64::from(dz);
    let x = (i64::from(face[0]) - i64::from(half)).max(1);
    let end = (i64::from(face[0]) + i64::from(half) + 1).min(i64::from(world.nx) - 1);
    let mut bits = 0u128;
    if y > 0 && y < i64::from(world.ny) && z > 0 && z + 1 < i64::from(world.nz) && x < end {
        let n = (end - x) as u32;
        bits = u128::from(world.solid_bits(x as u32, y as u32, z as u32, n));
        if n > 64 {
            bits |= u128::from(world.solid_bits(x as u32 + 64, y as u32, z as u32, n - 64)) << 64;
        }
    }
    std::iter::from_fn(move || {
        if bits == 0 {
            return None;
        }
        let dx = bits.trailing_zeros();
        bits &= bits - 1;
        Some([x as u32 + dx, y as u32, z as u32])
    })
}


impl BlastRange {
    /// Read-only presentation of a head that can currently work. Movement,
    /// unloading, a full tank and loss of the drive all stop the rotor.
    pub fn crawler_tool_mode(&self, id: usize) -> Option<CrawlerToolMode> {
        let u = self.units.get(id)?;
        if u.kind != modding::range::RangeUnitKind::Crawler || !u.status.alive
            || !u.status.engine || !u.status.weapon { return None; }
        let mining = self.mining.as_ref()?;
        if mining.hauling(id) { return None; }
        let tank = mining.tank(id)?;
        if self.matter.economy.ledger.room(tank) <= mining.inflight[id] { return None; }
        let (face, mode) = match self.battle.actors.get(id)?.order {
            battle::Order::Dig(face) => (face, CrawlerToolMode::Drill),
            battle::Order::Reclaim(face) => (face, CrawlerToolMode::Reclaim),
            _ => return None,
        };
        let reach = mining.data.kind(u.kind)?.dig_reach;
        let reach = if mode==CrawlerToolMode::Reclaim {reach.max(CRAWLER_RECLAIM_REACH)}else{reach};
        if !within(u, face, reach)
            && !(mode == CrawlerToolMode::Reclaim && (reclaim_hull_in_reach(&self.units, id, face, reach)
                || reclaim_piece_in_reach(&self.matter.debris.fragments,u,face,reach))) {
            return None;
        }
        Some(mode)
    }
    /// Position of the extended vertical head, following the actual open
    /// column. The selected X/Z remains stable for the whole bore.
    pub fn crawler_bore_position(&self, id: usize) -> Option<[i32; 3]> {
        if self.crawler_tool_mode(id) != Some(CrawlerToolMode::Drill) { return None; }
        let battle::Order::Dig(face) = self.battle.actors.get(id)?.order else { return None; };
        if face[0] < 1 || face[2] < 1 || face[0] >= self.world.nx as i32 - 1 || face[2] >= self.world.nz as i32 - 1 { return None; }
        let tip = self.units.get(id)?.world_pos(voxel::unit::CRAWLER_BORE_TIP);
        let floor = bore_surface(&self.world, face[0], face[2], voxel::unit::CRAWLER_BORE_RADIUS, tip[1]);
        Some([face[0], floor, face[2]])
    }
    /// The physical head has reached the ordered column and may cut.
    pub fn crawler_bore_cutting(&self, id: usize) -> bool {
        self.crawler_tool_mode(id) == Some(CrawlerToolMode::Drill)
            && self.battle.actors.get(id).is_some_and(|a| a.pursuit != 0 && self.tick >= a.pursuit)
    }
    pub fn crawler_bore_depth(&self, id: usize) -> Option<i32> {
        let pos = self.crawler_bore_position(id)?;
        let tip = self.units.get(id)?.world_pos(voxel::unit::CRAWLER_BORE_TIP);
        Some((tip[1] - pos[1]).max(0))
    }
    pub fn mining(&self) -> Option<&Mining> {
        self.mining.as_ref()
    }
    /// Matter in this unit's tank; `None` for a unit without one.
    pub fn tank_held(&self, id: usize) -> Option<Amount> {
        let tank = self.mining.as_ref()?.tank(id)?;
        Some(self.matter.economy.ledger.held(tank))
    }
    pub(super) fn has_economy(&self) -> bool {
        self.mining.is_some()
    }
    /// Where a digger outside the reach of its face drives: the face's
    /// column. The dig phase stops it as soon as the face is in reach.
    pub(super) fn dig_approach(&self, id: usize, face: [i32; 3]) -> Option<[i32; 2]> {
        let u = &self.units[id];
        if u.kind.emplacement() || self.in_dig_reach(u, face) {
            return None;
        }
        if matches!(u.kind, modding::range::RangeUnitKind::Crawler) {
            let reach = self
                .mining
                .as_ref()
                .and_then(|m| m.data.kind(u.kind))
                .map_or(0, |k| k.dig_reach) as i64;
            let pos = u.motion.pos.map(fx::Fx::to_int_trunc);
            let dx = i64::from(face[0] - pos[0]);
            let dz = i64::from(face[2] - pos[2]);
            let dist = ((dx * dx + dz * dz) as u64).isqrt().max(1) as i64;
            let back = (reach * 3 / 4).min(dist);
            return Some([
                face[0] - (dx * back / dist) as i32,
                face[2] - (dz * back / dist) as i32,
            ]);
        }
        Some([face[0], face[2]])
    }
    pub(super) fn reclaim_approach(&self, id: usize, face: [i32; 3]) -> Option<[i32; 2]> {
        let unit = &self.units[id];
        if unit.kind != modding::range::RangeUnitKind::Crawler { return self.dig_approach(id, face); }
        let reach = self.mining.as_ref()?.data.kind(unit.kind)?.dig_reach.max(CRAWLER_RECLAIM_REACH);
        if within(unit, face, reach) || reclaim_hull_in_reach(&self.units, id, face, reach)
            || reclaim_piece_in_reach(&self.matter.debris.fragments,unit,face,reach) { return None; }
        // A fragment directly selected at the click has priority over hulls.
        if let Some(f)=self.matter.debris.fragments.iter().find(|f| f.resting && {
            let p = [f.position.x, f.position.y, f.position.z].map(fx::Fx::to_int_trunc);
            (0..3).map(|a| i64::from(p[a] - face[a]).pow(2)).sum::<i64>() <= 9
        }) {
            return Some(reclaim_edge_approach(unit,reclaim_piece_bounds(f),reach));
        }
        let pos = unit.motion.pos.map(fx::Fx::to_int_trunc);
        let bounds = self.units.iter().enumerate().filter(|(other, u)| *other != id && !u.status.alive && reclaimable_hull(u))
            .map(|(_, u)| u.bounds())
            .filter(|&bounds| wreck_edge_distance2(face, bounds) <= i64::from(CRAWLER_RECLAIM_REACH).pow(2))
            .min_by_key(|&bounds| (wreck_edge_distance2(face, bounds), wreck_edge_distance2(pos, bounds)));
        let bounds=bounds.or_else(||self.matter.debris.fragments.iter().filter(|f|reclaim_piece_at_face(f,face))
            .map(reclaim_piece_bounds).min_by_key(|&b|(wreck_edge_distance2(face,b),wreck_edge_distance2(pos,b))));
        bounds.map(|b|reclaim_edge_approach(unit,b,reach)).or_else(||self.dig_approach(id, face))
    }

    fn in_dig_reach(&self, u: &RangeUnit, face: [i32; 3]) -> bool {
        let Some(k) = self.mining.as_ref().and_then(|m| m.data.kind(u.kind)) else {
            return false;
        };
        within(u, face, k.dig_reach)
    }

    /// Queue this tick's digging, before the drain. A digger digs while it
    /// is alive, ordered to dig, within reach of its face and has room for
    /// the next cell; cells claimed by debris, already reserved, air and
    /// the world's frame are passed over.
    pub(super) fn dig_phase(&mut self) {
        if !matches!(self.battle.phase,battle::Phase::Setup|battle::Phase::Fighting) {return;}
        // A corpse can become a free chunk after accepting the order. The
        // old edge may disappear or move; reacquire a stopped approach using
        // the normal budgeted movement scheduler, without resetting live paths.
        let approaches:Vec<_>=self.battle.actors.iter().enumerate().filter_map(|(id,a)| {
            let battle::Order::Reclaim(face)=a.order else{return None;};
            let u=&self.units[id];
            (u.kind==modding::range::RangeUnitKind::Crawler && u.status.alive && u.status.engine
                && (u.motion.destination.is_none() || u.motion.blocked) && !self.hauling(id))
                .then(||self.reclaim_approach(id,face).map(|to|(id,to))).flatten()
        }).collect();
        for (id,to) in approaches {self.units[id].motion.queue_move(&self.world,to);}
        let Self {
            mining,
            units,
            battle,
            world,
            matter,
            tick,
            ..
        } = self;
        let Some(m) = mining.as_mut() else {
            return;
        };
        m.reclaim_events.clear();
        if !matches!(battle.phase, battle::Phase::Setup | battle::Phase::Fighting) {
            return;
        }
        let ledger = &matter.economy.ledger;
        let mut diggers = Vec::new();
        let edge_reachable: Vec<bool> = units.iter().enumerate().map(|(id, u)| {
            let Some(actor) = battle.actors.get(id) else { return false; };
            let battle::Order::Reclaim(face) = actor.order else { return false; };
            if u.kind != modding::range::RangeUnitKind::Crawler { return false; }
            let reach = m.data.kind(u.kind).map_or(0, |k| k.dig_reach).max(CRAWLER_RECLAIM_REACH);
            within(u,face,reach) || (reclaim_hull_in_reach(units, id, face, reach)
                || reclaim_piece_in_reach(&matter.debris.fragments,u,face,reach))
        }).collect();
        for (id, u) in units.iter_mut().enumerate() {
            let (Some(tank), Some(actor)) = (m.tank(id), battle.actors.get_mut(id)) else {
                continue;
            };
            let (face, reclaim_only) = match actor.order {
                battle::Order::Dig(face) => (face, false),
                battle::Order::Reclaim(face) => (face, true),
                _ => continue,
            };
            let k = m.data.kind(u.kind).expect("a tank has its kind's entry");
            // A hauler is away from its face, even when it drives past it.
            if !u.status.alive || m.hauling(id) || (!within(u, face, k.dig_reach) && !edge_reachable[id])
                || (u.kind == modding::range::RangeUnitKind::Crawler
                    && (!u.status.weapon || !u.status.engine)) {
                if u.kind == modding::range::RangeUnitKind::Crawler && !reclaim_only { actor.pursuit = 0; }
                continue;
            }
            // At the face: a digger driving to it stops here.
            if u.motion.destination.is_some() {
                u.motion.stop();
            }
            if ledger.room(tank) > m.inflight[id] {
                if u.kind == modding::range::RangeUnitKind::Crawler && !reclaim_only {
                    // Reuse the saved, otherwise idle pursuit deadline while
                    // Dig owns the actor: excavation cannot outrun the head's
                    // deployment and travel to the selected column.
                    if actor.pursuit == 0 {
                        let tip = u.world_pos(voxel::unit::CRAWLER_BORE_TIP);
                        let dx = i64::from(face[0] - tip[0]);
                        let dz = i64::from(face[2] - tip[2]);
                        let horizontal = ((dx * dx + dz * dz) as u64).isqrt();
                        let ground = if face[0] >= 0 && face[2] >= 0
                            && face[0] < world.nx as i32 && face[2] < world.nz as i32 {
                            world.column_ceiling(face[0] as u32, face[2] as u32) as i32
                        } else { tip[1] };
                        let depth = (tip[1] - ground).max(0) as u64;
                        let travel = (horizontal.max(depth) * crate::TICKS_PER_SECOND as u64).div_ceil(80) as u32;
                        actor.pursuit = tick.saturating_add(14.max(travel).max(1));
                    }
                    if *tick < actor.pursuit { continue; }
                }
                diggers.push(Digger {
                    id,
                    face,
                    k: *k,
                    rate: if reclaim_only {
                        2048
                    } else {
                        k.dig_edits_per_tick
                    },
                    full: false,
                    reclaim_pending: false,
                    reclaim_only,
                });
            } else {
                if u.kind == modding::range::RangeUnitKind::Crawler && !reclaim_only { actor.pursuit = 0; }
                m.mark_full(id);
            }
        }
        // At least one edit a tick stays with everything else.
        let pool =
            (m.data.dig_edits_in_flight as usize).min(world.max_edits_per_tick.saturating_sub(1));
        let mut left = pool.saturating_sub(m.reserved.len());
        // Debris at the face goes first, from the same rate and sub-pool
        // (ADR-674); a digger away from its face forgets its reclaim.
        let mut reclaimers: Vec<_> = diggers
            .iter()
            .copied()
            .filter(|d| {
                d.reclaim_only || d.k.kind != modding::range::RangeUnitKind::Crawler
            })
            .collect();
        reclaim_hulls(m, units, matter, &mut reclaimers);
        let hull_work: Vec<_> = reclaimers
            .iter()
            .filter(|d| d.reclaim_only && d.reclaim_pending)
            .map(|d| d.id)
            .collect();
        // Reclaim changes ownership of existing debris, not terrain edits.
        // Give the crusher its own budget without consuming the dig queue.
        let mut reclaim_left = if reclaimers.iter().any(|d| d.reclaim_only) {
            left.max(4096)
        } else {
            left
        };
        let mut starved =
            reclaim::reclaim(m, units, matter, *tick, &mut reclaimers, &mut reclaim_left);
        if !reclaimers.iter().any(|d| d.reclaim_only) {
            left = reclaim_left;
        }
        for d in &mut reclaimers {
            d.reclaim_pending |= hull_work.contains(&d.id);
        }
        for updated in reclaimers {
            if let Some(d) = diggers.iter_mut().find(|d| d.id == updated.id) {
                *d = updated;
            }
        }
        if diggers.is_empty() {
            return;
        }
        let ledger = &matter.economy.ledger;
        let n = diggers.len();
        let start = *tick as usize % n;
        let mut cursor = voxel::World::mat_cursor();
        let mut drill_steps = DRILL_RAY_STEPS;
        let mut chain_searches = m.data.reclaim.as_ref().map_or(0, |r| r.searches_per_tick);
        'diggers: for step in 0..n {
            let Digger {
                id,
                face,
                k,
                rate,
                full,
                reclaim_pending,
                reclaim_only,
            } = diggers[(start + step) % n];
            if reclaim_only {
                if !reclaim_pending && m.inflight[id] == 0 {
                    if k.kind == modding::range::RangeUnitKind::Crawler {
                        if *tick % crate::TICKS_PER_SECOND as u32 != id as u32 % crate::TICKS_PER_SECOND as u32 || chain_searches == 0 {
                            continue;
                        }
                        chain_searches -= 1;
                        let tank = m.tank(id).unwrap();
                        if let Some(next) = next_reclaim_target(units, matter, id, face, ledger.room(tank)) {
                            battle.actors[id].order = battle::Order::Reclaim(next);
                            let pos = units[id].motion.pos.map(fx::Fx::to_int_trunc);
                            let dx = i64::from(next[0] - pos[0]);
                            let dz = i64::from(next[2] - pos[2]);
                            let dist = ((dx * dx + dz * dz) as u64).isqrt().max(1) as i64;
                            let back = (i64::from(k.dig_reach) * 3 / 4).min(dist);
                            units[id].motion.queue_move(world, [next[0] - (dx * back / dist) as i32, next[2] - (dz * back / dist) as i32]);
                        } else if ledger.held(tank) > 0 {
                            m.mark_full(id);
                        }
                    } else if ledger.held(m.tank(id).unwrap()) > 0 {
                        m.mark_full(id);
                    }
                }
                continue;
            }
            if full || rate == 0 {
                continue;
            }
            let tank = m.tank(id).expect("digger has a tank");
            // Reclaim can still run above when the drill is damaged, but
            // solid terrain needs both its connected drive and its motor.
            if matches!(k.kind, modding::range::RangeUnitKind::Crawler)
                && (!units[id].status.weapon || !units[id].status.engine)
            {
                if !reclaim_pending && m.inflight[id] == 0 && ledger.held(tank) > 0 {
                    m.mark_full(id);
                }
                continue;
            }
            let mut room = ledger.room(tank) - m.inflight[id];
            let mut rate = rate;
            let mut face_pending = false;
            let drill_origin = match k.kind {
                modding::range::RangeUnitKind::Crawler => {
                    let mut tip = units[id].world_pos(voxel::unit::CRAWLER_BORE_TIP);
                    tip[0] = face[0];
                    tip[2] = face[2];
                    if tip[0] >= 1 && tip[2] >= 1 && tip[0] < world.nx as i32 - 1 && tip[2] < world.nz as i32 - 1 {
                        tip[1] = bore_surface(world, tip[0], tip[2], voxel::unit::CRAWLER_BORE_RADIUS, tip[1]);
                    }
                    Some(tip)
                }
                _ => None,
            };
            let cut_face = if let Some(origin) = drill_origin {
                // Bore vertically from the visible cutting head, following
                // the excavated column all the way down to the world floor.
                // Work across the highest remaining layer, not the deepest
                // centre column: the whole head must have clearance first.
                [origin[0], (origin[1] - 1).max(1), origin[2]]
            } else {
                face
            };
            if drill_origin.is_some() && cut_face[0] >= 0 && cut_face[1] >= 0 && cut_face[2] >= 0
                && (cut_face[0] as u32) < world.nx && (cut_face[1] as u32) < world.ny && (cut_face[2] as u32) < world.nz {
                let material = world.material(cut_face[0] as u32, cut_face[1] as u32, cut_face[2] as u32);
                if material != AIR {
                    let strength = matter.economy.mats.get(material).map_or(512, |m| m.strength);
                    let period = m.data.dig_periods.iter().find(|d| d.material == material).map_or_else(|| strength.div_ceil(128).clamp(1, 8), |d| d.period);
                    rate = rate.min(rate.saturating_mul(2) / period).max(1);
                }
            }
            let initial_rate = rate;
            if let Some(origin) = drill_origin {
                // Clear exposed cells of the highest shaft layer across the
                // real head footprint. Every candidate is the top of its
                // column, so it has a clear vertical approach; bounded work
                // and the normal edit, strength and ledger budgets still apply.
                let radius = voxel::unit::CRAWLER_BORE_RADIUS;
                let side = 2 * radius + 1;
                let cells = (side * side) as usize;
                let offset = (*tick as usize).wrapping_mul(271) % cells;
                for step in 0..cells {
                    let index = (step + offset) % cells;
                    let dx = index as i32 % side - radius;
                    let dz = index as i32 / side - radius;
                    if dx * dx + dz * dz > radius * radius { continue; }
                    let x = origin[0] + dx;
                    let z = origin[2] + dz;
                    if x <= 0 || z <= 0 || x >= world.nx as i32 - 1 || z >= world.nz as i32 - 1 { continue; }
                    let top = world.column_ceiling(x as u32, z as u32) as i32;
                    if top != origin[1] || top <= 1 { continue; }
                    let (x, y, z) = (x as u32, (top - 1) as u32, z as u32);
                    if m.reserved.contains_key(&[x, y, z]) || matter.debris.claimed([x, y, z]) {
                        face_pending = true;
                        continue;
                    }
                    let material = world.material_cached(&mut cursor, x, y, z);
                    let amount = matter.economy.per_voxel(material);
                    if material == AIR || amount == 0 { continue; }
                    let strength = matter.economy.mats.get(material).map_or(512, |m| m.strength);
                    let period = m.data.dig_periods.iter().find(|d| d.material == material).map_or_else(|| strength.div_ceil(128).clamp(1, 8), |d| d.period);
                    let phase = x.wrapping_mul(17).wrapping_add(y.wrapping_mul(31)).wrapping_add(z.wrapping_mul(47));
                    if tick.wrapping_add(phase) % period != 0 { face_pending = true; continue; }
                    if amount > room { m.mark_full(id); continue 'diggers; }
                    if left == 0 { starved = true; continue 'diggers; }
                    world.queue_dig_by(x, y, z, DIG | id as u32);
                    m.reserved.insert([x, y, z], Reservation { slot: id as u16, material, amount });
                    m.inflight[id] += amount;
                    room -= amount;
                    left -= 1;
                    rate -= 1;
                    if rate == 0 { continue 'diggers; }
                }
                if !face_pending && m.inflight[id] == 0 && ledger.held(tank) > 0 && origin[1] <= 1 {
                    m.mark_full(id);
                }
                continue;
            }
            let spans = Arc::clone(m.balls.get(&k.dig_radius).expect("dig ball is prepared"));
            // Visit the first solid voxel on the drill axis before scanning
            // the surrounding ball. Otherwise long rays to peripheral cells
            // can exhaust the shared ray budget before the actual face.
            let first = drill_origin.map(|_| [0, 0, 0]);
            // A blocked early row used to consume the complete ray budget on
            // every tick, starving reachable rows later in the ball forever.
            let start = if drill_origin.is_some() && !spans.is_empty() {
                (*tick as usize).wrapping_mul(31) % spans.len()
            } else { 0 };
            let rows = spans.iter().cycle().skip(start).take(spans.len()).copied();
            for span in first.into_iter().chain(rows) {
                for [x, y, z] in occupied_row(world, cut_face, span) {
                    if m.reserved.contains_key(&[x, y, z]) || matter.debris.claimed([x, y, z]) {
                        // Do not depart while another queued edit or debris
                        // may still make this face productive.
                        face_pending = true;
                        continue;
                    }
                    let material = world.material_cached(&mut cursor, x, y, z);
                    let amount = matter.economy.per_voxel(material);
                    if material == AIR || amount == 0 {
                        continue;
                    }
                    if matches!(k.kind, modding::range::RangeUnitKind::Crawler) {
                        // Fixed, bounded work approximation: harder material
                        // accepts fewer cuts over time. A spatial phase spreads
                        // work across ticks and needs no per-voxel save state.
                        let strength = matter
                            .economy
                            .mats
                            .get(material)
                            .map_or(512, |m| m.strength);
                        let period = m.data.dig_periods.iter().find(|d| d.material == material).map_or_else(|| strength.div_ceil(128).clamp(1, 8), |d| d.period);
                        let phase = x
                            .wrapping_mul(17)
                            .wrapping_add(y.wrapping_mul(31))
                            .wrapping_add(z.wrapping_mul(47));
                        if tick.wrapping_add(phase) % period != 0 {
                            face_pending = true;
                            continue;
                        }
                        if [x as i32, y as i32, z as i32] != cut_face {
                            match drill_path_clear(
                                world,
                                drill_origin.expect("crawler drill origin"),
                                [x, y, z],
                                &mut drill_steps,
                            ) {
                                Some(true) => {}
                                Some(false) => {
                                    face_pending = true;
                                    continue;
                                }
                                None => {
                                    starved = true;
                                    // Occluded cells can exhaust every ray
                                    // without producing a single edit. Move
                                    // the boring point instead of retrying
                                    // the same blocked surface indefinitely.
                                    break 'diggers;
                                }
                            }
                        }
                    }
                    // The next voxel does not fit: the tank is full.
                    if amount > room {
                        m.mark_full(id);
                        continue 'diggers;
                    }
                    if left == 0 {
                        starved = true;
                        break 'diggers;
                    }
                    world.queue_dig_by(x, y, z, DIG | id as u32);
                    m.reserved.insert(
                        [x, y, z],
                        Reservation {
                            slot: id as u16,
                            material,
                            amount,
                        },
                    );
                    m.inflight[id] += amount;
                    room -= amount;
                    left -= 1;
                    rate -= 1;
                    if rate == 0 {
                        continue 'diggers;
                    }
                }
            }
            // A small salvage job may end long before the tank is full. Send
            // the real crawler home once this face is exhausted, but never
            // while a reclaim search, claimed cell or dig edit is pending.
            if matches!(k.kind, modding::range::RangeUnitKind::Crawler)
                && !reclaim_pending
                && m.inflight[id] == 0
                && rate == initial_rate
            {
                // The selected shaft remains authoritative; only return
                // for unloading when there is no solid work left below it.
                if !face_pending && ledger.held(tank) > 0 && cut_face[1] <= 1 {
                    m.mark_full(id);
                }
            }
        }
        if starved {
            m.deferred += 1;
        }
    }

    /// Credit what this tick's drain removed for diggers, right after the
    /// drain. The cell's matter is revealed from the ground and transferred
    /// into the tank. A voxel already gone, one that holds another material
    /// than the one reserved, or one dug for a digger that has died
    /// credits nothing: the edit was nobody's, as a crater's.
    pub(super) fn credit_digs(&mut self) {
        let Some(m) = self.mining.as_mut() else {
            return;
        };
        if m.reserved.is_empty() {
            return;
        }
        let e = &mut self.matter.economy;
        for &(edit, previous) in self.world.completed() {
            let Some(slot) = dig_slot(edit.by) else {
                continue;
            };
            let r = m
                .reserved
                .remove(&[edit.x, edit.y, edit.z])
                .expect("a dig edit drains with its reservation");
            debug_assert_eq!(usize::from(r.slot), slot);
            m.inflight[slot] -= r.amount;
            if previous == AIR || previous != r.material || !self.units[slot].status.alive {
                m.misses += 1;
                continue;
            }
            let tank = m.tank(slot).expect("digger has a tank");
            // Volume × density at removal. A density tuned up while the edit
            // waited may not fit: the rest stays in the ground container.
            let amount = e.per_voxel(previous);
            e.ledger.reveal(e.ground, amount);
            m.mined += e.ledger.transfer(e.ground, tank, amount);
            m.cells += 1;
        }
    }

    /// Version 62 tail: the economy's state. Everything it refers to —
    /// units, world queue, ledger, orders — is already loaded.
    pub(super) fn load_mining(&mut self, s: &mut Scan) -> Option<()> {
        let m = self.mining.as_mut()?;
        m.mined = s.u64();
        m.cells = s.u64();
        m.misses = s.u64();
        m.deferred = s.u64();
        let n = s.u32() as usize;
        if n > m.data.dig_edits_in_flight as usize || n > s.left() / 24 {
            return None;
        }
        let ledger = &self.matter.economy.ledger;
        for (id, u) in self.units.iter().enumerate() {
            if let Some(tank) = m.tank(id) {
                let c = ledger.try_get(tank)?;
                let k = m.data.kind(u.kind)?;
                if !c.alive || c.kind != Kind::Hauler || c.cap != k.tank {
                    return None;
                }
            }
        }
        m.reserved.clear();
        m.inflight.iter_mut().for_each(|v| *v = 0);
        for _ in 0..n {
            let cell: [u32; 3] = std::array::from_fn(|_| s.u32());
            let r = Reservation {
                slot: s.u16(),
                material: s.u16(),
                amount: s.u64(),
            };
            let slot = usize::from(r.slot);
            let [x, y, z] = cell;
            if !self.world.inside(x, y, z)
                || self.world.on_frame(x, y, z)
                || m.tank(slot).is_none()
                || r.material == AIR
                || r.amount == 0
                || m.reserved.insert(cell, r).is_some()
            {
                return None;
            }
            m.inflight[slot] = m.inflight[slot].checked_add(r.amount)?;
        }
        for (id, reserved) in m.inflight.iter().enumerate() {
            if let Some(tank) = m.tank(id) {
                if *reserved > ledger.room(tank) {
                    return None;
                }
            }
        }
        // Every reservation has its edit waiting, and every dig edit waiting
        // has its reservation.
        let mut queued = 0;
        for e in self.world.queued_edits() {
            let Some(slot) = dig_slot(e.by) else {
                continue;
            };
            queued += 1;
            if e.mat != AIR
                || m.reserved
                    .get(&[e.x, e.y, e.z])
                    .map(|r| usize::from(r.slot))
                    != Some(slot)
            {
                return None;
            }
        }
        if queued != m.reserved.len() {
            return None;
        }
        for (id, a) in self.battle.actors.iter().enumerate() {
            if matches!(a.order, battle::Order::Dig(_) | battle::Order::Reclaim(_))
                && m.tank(id).is_none()
            {
                return None;
            }
        }
        s.ok().then_some(())
    }
}

/// Horizontal distance from the unit to its face within `reach`, compared
/// in squares.
fn within(u: &RangeUnit, face: [i32; 3], reach: u32) -> bool {
    let dx = i64::from(u.motion.pos[0].to_int_trunc() - face[0]);
    let dz = i64::from(u.motion.pos[2].to_int_trunc() - face[2]);
    dx * dx + dz * dz <= i64::from(reach) * i64::from(reach)
}
