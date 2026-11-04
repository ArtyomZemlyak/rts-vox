//! Hauling to the docks and the sides' stores (plan 245, slice 3; ADR-673).
//!
//! **A store per side.** Container 1 is the allied side's store; the enemy
//! side's is appended after the tanks. A store's cap is the bank's base
//! store plus the stores of the living docks of its side — a dock is a
//! unit kind with a store in the economy table. The tank does not raise
//! the cap (Q-56 default).
//!
//! **A full tank hauls.** When the dig phase finds that the next cell does
//! not fit and nothing is in flight for the digger, it turns to haul:
//! the nearest living dock of its side by squared distance, ties by slot,
//! then the regular motion scheduler drives it there, as a group move.
//! Within reach of the dock it unloads: the refining yield goes to the
//! store, the rest to RefiningLoss, and no more is taken than the store has
//! room for — what does not fit stays in the tank. An empty tank returns to
//! the face of its dig order.
//!
//! **Dock searches have a ceiling per tick.** Haulers without a dock queue
//! for a search; at most `dock_searches_per_tick` of them search, the first
//! one served rotated by tick, and the rest wait for the next tick with the
//! tick counted (ADR-114). A search that finds no dock leaves the hauler
//! waiting with its cargo and asks again on a later tick.
//!
//! **Caps follow the docks every tick.** A dock that died lowers its side's
//! cap; a store over its new cap loses the excess to OverCap. The store is
//! the side's one container and a dock holds only capacity, so a dock's
//! death breaches nothing of the store; its body goes to debris by the
//! range's body rules.
use super::{battle, BlastRange, Mining, RangeUnit};
use crate::matter::{Amount, ContainerId, Kind, Ledger};
use fx::blob::{Blob, Scan};
use modding::range_economy::{RangeBank, RangeEconomy};

#[cfg(test)]
#[path = "range_haul_tests.rs"]
mod tests;

/// Where a digger is in its round trip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Leg {
    /// At its face or driving to it.
    #[default]
    Digging,
    /// Hauling to this dock, or waiting for a dock search when `None`.
    Hauling(Option<u16>),
}

/// The haul's state on a layout whose economy has a bank section.
pub(crate) struct Haul {
    bank: RangeBank,
    /// Allied store (container 1), enemy store (appended).
    stores: [ContainerId; 2],
    /// Whether each unit slot is allied.
    sides: Vec<bool>,
    legs: Vec<Leg>,
    /// Diggers the dig phase found full this tick; the haul phase reads and
    /// clears them in the same tick, so they are never saved.
    full: Vec<bool>,
    /// Matter taken from tanks at unloading, of it refined into stores,
    /// dock searches, searches that found no dock, ticks the search ceiling
    /// left haulers waiting.
    pub unloaded: Amount,
    pub delivered: Amount,
    pub searches: u64,
    pub search_misses: u64,
    pub search_deferred: u64,
}

/// Each side's cap: the base store plus the stores of its living docks.
fn caps(data: &RangeEconomy, base: Amount, units: &[RangeUnit], sides: &[bool]) -> [Amount; 2] {
    let mut caps = [base; 2];
    for (u, allied) in units.iter().zip(sides) {
        if u.status.alive {
            let side = usize::from(!allied);
            caps[side] = caps[side].saturating_add(data.store(u.kind));
        }
    }
    caps
}

fn dock_position(u: &RangeUnit) -> [i32; 2] {
    let at = u.motion.pos.map(|v| v.to_int_trunc());
    if u.kind == modding::range::RangeUnitKind::GrebenStorage {
        let front = i32::from(u.body.vol.nx) * i32::from(u.body.mm) / 200;
        [at[0] - front, at[2] - 16]
    } else {
        [at[0], at[2]]
    }
}

/// Squared horizontal distance between two units.
fn distance2(a: &RangeUnit, b: &RangeUnit) -> i64 {
    let a = dock_position(a);
    let b = dock_position(b);
    let dx = i64::from(a[0] - b[0]);
    let dz = i64::from(a[1] - b[1]);
    dx * dx + dz * dz
}

impl Haul {
    pub(crate) fn new(
        data: &RangeEconomy,
        units: &[RangeUnit],
        sides: Vec<bool>,
        ledger: &mut Ledger,
        allied_store: ContainerId,
    ) -> Self {
        let bank = data.bank.expect("a haul has its bank section");
        let caps = caps(data, bank.base_store, units, &sides);
        // Container 1 is empty: only its cap changes.
        ledger.lower_cap(allied_store, caps[0]);
        let enemy = ledger.create(Kind::Depot, 0, caps[1]);
        Self {
            bank,
            stores: [allied_store, enemy],
            legs: vec![Leg::Digging; sides.len()],
            full: vec![false; sides.len()],
            sides,
            unloaded: 0,
            delivered: 0,
            searches: 0,
            search_misses: 0,
            search_deferred: 0,
        }
    }
    /// A unit appended to the roster (ADR-675).
    pub(crate) fn push_unit(&mut self, allied: bool) {
        self.sides.push(allied);
        self.legs.push(Leg::Digging);
        self.full.push(false);
    }
    /// The dig phase found no room for the next cell or piece.
    pub(crate) fn mark_full(&mut self, id: usize) {
        self.full[id] = true;
    }
    fn store_of(&self, id: usize) -> ContainerId {
        self.stores[usize::from(!self.sides[id])]
    }
    /// The side's store container, allied first.
    pub fn store(&self, allied: bool) -> ContainerId {
        self.stores[usize::from(!allied)]
    }
    /// Whether `dock` takes `id`'s cargo: alive, of its side, with a store.
    fn dock_ok(&self, data: &RangeEconomy, units: &[RangeUnit], id: usize, dock: usize) -> bool {
        units
            .get(dock)
            .is_some_and(|d| d.status.alive && data.store(d.kind) > 0)
            && self.sides[dock] == self.sides[id]
    }
    /// The nearest living dock of `id`'s side by squared distance; ties go
    /// to the lower slot.
    fn nearest_dock(&self, data: &RangeEconomy, units: &[RangeUnit], id: usize) -> Option<usize> {
        let mut best: Option<(i64, usize)> = None;
        for dock in 0..units.len() {
            if !self.dock_ok(data, units, id, dock) {
                continue;
            }
            let d = distance2(&units[id], &units[dock]);
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, dock));
            }
        }
        best.map(|(_, dock)| dock)
    }
    pub(crate) fn save(&self, b: &mut Blob) {
        b.u64(self.unloaded);
        b.u64(self.delivered);
        b.u64(self.searches);
        b.u64(self.search_misses);
        b.u64(self.search_deferred);
        for leg in &self.legs {
            match *leg {
                Leg::Digging => b.u8(0),
                Leg::Hauling(None) => b.u8(1),
                Leg::Hauling(Some(dock)) => {
                    b.u8(2);
                    b.u16(dock);
                }
            }
        }
    }
}

impl Mining {
    /// Whether this digger is away hauling.
    pub(crate) fn hauling(&self, id: usize) -> bool {
        self.haul
            .as_ref()
            .is_some_and(|h| matches!(h.legs[id], Leg::Hauling(_)))
    }
    /// The dig phase found no room for the next cell. Without a bank the
    /// full tank simply stops digging (ADR-672).
    pub(crate) fn mark_full(&mut self, id: usize) {
        if let Some(h) = self.haul.as_mut() {
            h.mark_full(id);
        }
    }
    /// A new order ends the round trip: the order decides where to go.
    pub(crate) fn reset_leg(&mut self, id: usize) {
        if let Some(leg) = self.haul.as_mut().and_then(|h| h.legs.get_mut(id)) {
            *leg = Leg::Digging;
        }
    }
    pub(crate) fn haul(&self) -> Option<&Haul> {
        self.haul.as_ref()
    }
}

/// Where a hauler unloads, what it unloads with and where it returns.
struct Trip {
    id: usize,
    dock: usize,
    tank: ContainerId,
    /// The face of its dig order and its dig reach.
    face: [i32; 3],
    dig_reach: u32,
    /// The search has just assigned the dock.
    fresh: bool,
}

impl BlastRange {
    /// Whether the range has stores and hauling: the snapshot is then
    /// version 63 (ADR-673).
    pub(crate) fn has_banks(&self) -> bool {
        self.mining.as_ref().is_some_and(|m| m.haul.is_some())
    }
    /// A side's store: matter held and its cap; `None` without a bank.
    pub fn side_store(&self, allied: bool) -> Option<(Amount, Amount)> {
        let h = self.mining.as_ref()?.haul.as_ref()?;
        let c = self.matter.economy.ledger.get(h.store(allied));
        Some((c.held, c.cap))
    }
    /// A unit's tank: matter held and its capacity; `None` without one.
    pub fn tank_fill(&self, id: usize) -> Option<(Amount, Amount)> {
        let tank = self.mining.as_ref()?.tank(id)?;
        let c = self.matter.economy.ledger.get(tank);
        Some((c.held, c.cap))
    }
    /// Whether this digger is away hauling to a dock.
    pub fn hauling(&self, id: usize) -> bool {
        self.mining.as_ref().is_some_and(|m| m.hauling(id))
    }

    /// Recount the caps, turn full diggers to haul, search docks under the
    /// ceiling and unload — right after the dig credit, so a tank filled
    /// by this tick's drain turns the same tick.
    pub(crate) fn haul_phase(&mut self) {
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
        let Mining {
            data,
            tanks,
            inflight,
            haul,
            ..
        } = m;
        let Some(h) = haul.as_mut() else {
            return;
        };
        let full = std::mem::replace(&mut h.full, vec![false; units.len()]);
        if !matches!(battle.phase, battle::Phase::Setup | battle::Phase::Fighting) {
            return;
        }
        let ledger = &mut matter.economy.ledger;
        // A dock that died lowers its side's cap; the excess is lost to
        // OverCap (07-economy, order on a store's destruction).
        let caps = caps(data, h.bank.base_store, units, &h.sides);
        for (store, cap) in h.stores.into_iter().zip(caps) {
            if ledger.get(store).cap != cap {
                ledger.lower_cap(store, cap);
            }
        }
        let mut need = Vec::new();
        let mut trips = Vec::new();
        for id in 0..units.len() {
            let Some(tank) = tanks[id] else {
                continue;
            };
            let face = match battle.actors.get(id).map(|a| a.order) {
                Some(battle::Order::Dig(face) | battle::Order::Reclaim(face))
                    if units[id].status.alive =>
                {
                    face
                }
                _ => {
                    h.legs[id] = Leg::Digging;
                    continue;
                }
            };
            if h.legs[id] == Leg::Digging {
                // Full means the next cell did not fit, and what was in
                // flight has landed.
                if !(full[id] && inflight[id] == 0 && ledger.held(tank) > 0) {
                    continue;
                }
                h.legs[id] = Leg::Hauling(None);
            }
            let dig_reach = data.kind(units[id].kind).map_or(0, |k| k.dig_reach);
            match h.legs[id] {
                Leg::Hauling(Some(dock)) if h.dock_ok(data, units, id, usize::from(dock)) => trips
                    .push(Trip {
                        id,
                        dock: usize::from(dock),
                        tank,
                        face,
                        dig_reach,
                        fresh: false,
                    }),
                Leg::Hauling(Some(_)) => {
                    // A dead or otherwise invalid dock must not leave its old
                    // route active while the hauler waits for a new search.
                    units[id].motion.stop();
                    h.legs[id] = Leg::Hauling(None);
                    need.push((id, tank, face, dig_reach));
                }
                _ => need.push((id, tank, face, dig_reach)),
            }
        }
        // Searches under the ceiling, the first served rotated by tick: a
        // hauler whose search finds nothing every tick does not lock out
        // the ones behind it. The rest wait a tick with their cargo.
        let n = need.len();
        if n > 0 {
            let take = (h.bank.dock_searches_per_tick as usize).min(n);
            if take < n {
                h.search_deferred += 1;
            }
            // In u64: the same start on every pointer width.
            let start = (u64::from(*tick) * take as u64 % n as u64) as usize;
            for step in 0..take {
                let (id, tank, face, dig_reach) = need[(start + step) % n];
                h.searches += 1;
                match h.nearest_dock(data, units, id) {
                    Some(dock) => {
                        h.legs[id] = Leg::Hauling(Some(dock as u16));
                        trips.push(Trip {
                            id,
                            dock,
                            tank,
                            face,
                            dig_reach,
                            fresh: true,
                        });
                    }
                    None => {
                        h.legs[id] = Leg::Hauling(None);
                        h.search_misses += 1;
                    }
                }
            }
        }
        // Unload in slot order: two haulers at one store share its room by
        // linear index, not by who searched first.
        trips.sort_unstable_by_key(|t| t.id);
        let (num, den) = (h.bank.refine_num, h.bank.refine_den);
        let reach = i64::from(h.bank.unload_reach);
        for t in trips {
            let target = dock_position(&units[t.dock]);
            let u = &mut units[t.id];
            let dx = i64::from(u.motion.pos[0].to_int_trunc() - target[0]);
            let dz = i64::from(u.motion.pos[2].to_int_trunc() - target[1]);
            if dx * dx + dz * dz > reach * reach {
                // Drive by the regular scheduler. A hauler that stopped
                // short (no route, a dock that moved) asks again.
                if !u.kind.emplacement() && (t.fresh || u.motion.destination.is_none()) {
                    u.motion.queue_move(world, target);
                }
                continue;
            }
            if u.motion.destination.is_some() {
                u.motion.stop();
            }
            // No more is taken than the store has room for: refining
            // books everything taken and not delivered as RefiningLoss,
            // so a full tank into a full store would vanish into the sink.
            let store = h.store_of(t.id);
            let room = u128::from(ledger.room(store));
            let fits = (room * u128::from(den) / u128::from(num)).min(u128::from(u64::MAX));
            let cargo = ledger.held(t.tank);
            let want = cargo.min(fits as u64);
            // Room that refines into nothing takes nothing: the cargo would
            // trickle into the sink a unit a tick. The last of a load
            // refines whole, losses and all, and empties the tank.
            if want > 0 && (want == cargo || u128::from(want) * u128::from(num) >= u128::from(den))
            {
                h.delivered += ledger.refine(t.tank, store, want, num, den);
                h.unloaded += want;
            }
            if ledger.held(t.tank) > 0 {
                // The store is full: the cargo waits at the dock.
                continue;
            }
            h.legs[t.id] = Leg::Digging;
            let back = [t.face[0], t.face[2]];
            if !u.kind.emplacement() && !super::within(u, t.face, t.dig_reach) {
                u.motion.queue_move(world, back);
            }
        }
    }

    /// Version 63 tail: the haul's state, after the dig's. The stores are
    /// containers of the ledger, already loaded with their caps.
    pub(crate) fn load_haul(&mut self, s: &mut Scan) -> Option<()> {
        let m = self.mining.as_mut()?;
        let h = m.haul.as_mut()?;
        h.unloaded = s.u64();
        h.delivered = s.u64();
        h.searches = s.u64();
        h.search_misses = s.u64();
        h.search_deferred = s.u64();
        if h.delivered > h.unloaded || h.search_misses > h.searches {
            return None;
        }
        let ledger = &self.matter.economy.ledger;
        if h.stores[0] != self.matter.economy.banks.first()?.stock {
            return None;
        }
        for store in h.stores {
            let c = ledger.try_get(store)?;
            if !c.alive || c.kind != Kind::Depot {
                return None;
            }
        }
        h.full.iter_mut().for_each(|f| *f = false);
        for id in 0..self.units.len() {
            h.legs[id] = match s.u8() {
                0 => Leg::Digging,
                1 => Leg::Hauling(None),
                2 => Leg::Hauling(Some(s.u16())),
                _ => return None,
            };
            if let Leg::Hauling(dock) = h.legs[id] {
                let dig = matches!(
                    self.battle.actors.get(id).map(|a| a.order),
                    Some(battle::Order::Dig(_) | battle::Order::Reclaim(_))
                );
                if m.tanks[id].is_none()
                    || !dig
                    || dock.is_some_and(|d| {
                        let d = usize::from(d);
                        d >= self.units.len()
                            || m.data.store(self.units[d].kind) == 0
                            || h.sides[d] != h.sides[id]
                    })
                {
                    return None;
                }
            }
        }
        s.ok().then_some(())
    }
}
