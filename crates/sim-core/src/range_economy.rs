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
//! transfer between two containers.
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
const DRILL_RAY_STEPS: usize = 8192;

fn drill_front(world: &voxel::World, from: [i32; 3], target: [i32; 3], budget: &mut usize) -> Option<[i32; 3]> {
    if from.iter().all(|v| *v >= 0) && world.solid(from[0] as u32, from[1] as u32, from[2] as u32) {
        return Some(from);
    }
    let mut found = None;
    let mut exhausted = false;
    voxel::los::walk((from[0], from[1], from[2]), (target[0], target[1], target[2]), |x, y, z| {
        if *budget == 0 { exhausted = true; return false; }
        *budget -= 1;
        if x >= 0 && y >= 0 && z >= 0 && world.solid(x as u32, y as u32, z as u32) {
            found = Some([x, y, z]);
            false
        } else { true }
    });
    if exhausted { None } else { Some(found.unwrap_or(target)) }
}

fn drill_path_clear(world: &voxel::World, from: [i32; 3], target: [u32; 3], budget: &mut usize) -> Option<bool> {
    let mut clear = true;
    let mut exhausted = false;
    voxel::los::walk((from[0], from[1], from[2]), (target[0] as i32, target[1] as i32, target[2] as i32), |x, y, z| {
        if *budget == 0 { exhausted = true; return false; }
        *budget -= 1;
        if x >= 0 && y >= 0 && z >= 0 && world.solid(x as u32, y as u32, z as u32) {
            clear = false;
            false
        } else { true }
    });
    if exhausted { None } else { Some(clear) }
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
    /// Production, with a production section (ADR-675).
    production: Option<produce::Production>,
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
            production,
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
        (!u.kind.emplacement() && !self.in_dig_reach(u, face)).then_some([face[0], face[2]])
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
        if !matches!(battle.phase, battle::Phase::Setup | battle::Phase::Fighting) {
            return;
        }
        let ledger = &matter.economy.ledger;
        let mut diggers = Vec::new();
        for (id, u) in units.iter_mut().enumerate() {
            let (Some(tank), Some(actor)) = (m.tank(id), battle.actors.get(id)) else {
                continue;
            };
            let battle::Order::Dig(face) = actor.order else {
                continue;
            };
            let k = m.data.kind(u.kind).expect("a tank has its kind's entry");
            // A hauler is away from its face, even when it drives past it.
            if !u.status.alive || m.hauling(id) || !within(u, face, k.dig_reach) {
                continue;
            }
            // At the face: a digger driving to it stops here.
            if u.motion.destination.is_some() {
                u.motion.stop();
            }
            if ledger.room(tank) > m.inflight[id] {
                diggers.push(Digger {
                    id,
                    face,
                    k: *k,
                    rate: k.dig_edits_per_tick,
                    full: false,
                    reclaim_pending: false,
                });
            } else {
                m.mark_full(id);
            }
        }
        // At least one edit a tick stays with everything else.
        let pool =
            (m.data.dig_edits_in_flight as usize).min(world.max_edits_per_tick.saturating_sub(1));
        let mut left = pool.saturating_sub(m.reserved.len());
        // Debris at the face goes first, from the same rate and sub-pool
        // (ADR-674); a digger away from its face forgets its reclaim.
        let mut starved = reclaim::reclaim(m, units, matter, *tick, &mut diggers, &mut left);
        if diggers.is_empty() {
            return;
        }
        let ledger = &matter.economy.ledger;
        let n = diggers.len();
        let start = *tick as usize % n;
        let mut cursor = voxel::World::mat_cursor();
        let mut drill_steps = DRILL_RAY_STEPS;
        'diggers: for step in 0..n {
            let Digger {
                id,
                face,
                k,
                rate,
                full,
                reclaim_pending,
            } = diggers[(start + step) % n];
            if full || rate == 0 {
                continue;
            }
            let tank = m.tank(id).expect("digger has a tank");
            // Reclaim can still run above when the drill is damaged, but
            // solid terrain needs both its connected drive and its motor.
            if k.kind == modding::range::RangeUnitKind::Crawler
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
            let drill_origin = (k.kind == modding::range::RangeUnitKind::Crawler)
                .then(|| units[id].world_pos(voxel::unit::CRAWLER_DRILL_TIP));
            let cut_face = if let Some(origin) = drill_origin {
                let Some(front) = drill_front(world, origin, face, &mut drill_steps) else {
                    starved = true;
                    continue;
                };
                front
            } else { face };
            let spans = Arc::clone(m.balls.get(&k.dig_radius).expect("dig ball is prepared"));
            for &span in spans.iter() {
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
                    if k.kind == modding::range::RangeUnitKind::Crawler {
                        // Fixed, bounded work approximation: harder material
                        // accepts fewer cuts over time. A spatial phase spreads
                        // work across ticks and needs no per-voxel save state.
                        let strength = matter.economy.mats.get(material).map_or(512, |m| m.strength);
                        let period = strength.div_ceil(16).clamp(1, 32);
                        let phase = x.wrapping_mul(17).wrapping_add(y.wrapping_mul(31)).wrapping_add(z.wrapping_mul(47));
                        if tick.wrapping_add(phase) % period != 0 {
                            face_pending = true;
                            continue;
                        }
                        match drill_path_clear(world, drill_origin.expect("crawler drill origin"), [x, y, z], &mut drill_steps) {
                            Some(true) => {},
                            Some(false) => { face_pending = true; continue; },
                            None => { starved = true; break 'diggers; },
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
            if k.kind == modding::range::RangeUnitKind::Crawler
                && !reclaim_pending && !face_pending && m.inflight[id] == 0
                && rate == k.dig_edits_per_tick && ledger.held(tank) > 0
            {
                m.mark_full(id);
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
            if matches!(a.order, battle::Order::Dig(_)) && m.tank(id).is_none() {
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
