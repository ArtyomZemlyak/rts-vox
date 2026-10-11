//! Reclaim at the face and the wreck of a dead digger's tank (plan 245,
//! slice 6; ADR-674). Reclaim is the dig order: there is no other.
//!
//! **What is taken.** A digger at its face takes resting debris pieces
//! whose centre lies within the section's `reach` of the face, and the tank
//! of a dead digger whose hull does. A flying piece is not taken, nor a
//! live hull: those are not debris. The nearest by squared distance to the
//! face goes first; ties go to pieces before tanks, then to the lower piece
//! id or slot.
//!
//! **A piece is taken whole.** Its container moves into the tank in one
//! transfer once the digger has spent as many dig edits on it as the piece
//! has cells — the same rate and the same sub-pool as rock, so reclaim is
//! no cheaper than digging the same volume (ADR-045). The edits may span
//! ticks; the piece is the digger's while it works it, and no other digger
//! takes it. A piece whose matter does not fit the tank's room makes the
//! tank full, as the next cell of rock does; one bigger than the whole tank
//! is not this digger's to take. Splitting a piece would change its shape,
//! count and mass, which the ledger checks against each other; whole
//! pieces keep both exact. The emptied piece leaves the world the same
//! tick: geometry, wreck entry and container ([`Bursts::retire_reclaimed`]).
//! What rested on it finds no support at its next rest check and falls
//! (ADR-291).
//!
//! **A dead digger's tank breaches.** At the end of the tick it dies in,
//! `breach_num / breach_den` of its cargo goes to ContainerBreach; the rest
//! stays in the tank, which lies with the hull and is reclaimed by the dig
//! order. The tank has no cells: one dig edit takes `cargo_per_edit` of it,
//! and it may be taken in part (Q-78).
//!
//! **Searches have a ceiling per tick.** A digger without work searches
//! only when something lies in the grid cells around its face — one with
//! nothing there spends no search. At most `searches_per_tick` search, the
//! first one served rotated by tick; the rest wait for the next tick with
//! the tick counted (ADR-114).
use super::{BlastRange, Digger, Mining, RangeUnit};
use crate::burst::Fragment;
use crate::matter::{Amount, ContainerId, Ledger, Sink};
use fx::blob::{Blob, Scan};
use modding::range_economy::RangeReclaim;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
#[path = "range_reclaim_tests.rs"]
mod tests;

/// What a digger works at its face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Work {
    /// A resting piece by its id, and the dig edits spent on it.
    Piece { id: u64, spent: u32 },
    /// The tank of the dead digger in this slot.
    Tank(u16),
}

/// Reclaim's state on a layout whose economy has a reclaim section.
pub(crate) struct Reclaim {
    data: RangeReclaim,
    /// Each slot's work; kept only while the digger stands at its face.
    work: Vec<Option<Work>>,
    /// Whether each slot's tank has breached. Between ticks these are
    /// exactly the dead diggers, so a load derives it and a save omits it.
    breached: Vec<bool>,
    /// Matter taken into tanks, pieces taken, dig edits spent on reclaim,
    /// searches, searches that found nothing to take, ticks the search
    /// ceiling left diggers waiting, cargo lost to ContainerBreach.
    pub reclaimed: Amount,
    pub pieces: u64,
    pub edits: u64,
    pub searches: u64,
    pub search_misses: u64,
    pub search_deferred: u64,
    pub lost: Amount,
}

impl Reclaim {
    pub(crate) fn new(data: RangeReclaim, units: usize) -> Self {
        Self {
            data,
            work: vec![None; units],
            breached: vec![false; units],
            reclaimed: 0,
            pieces: 0,
            edits: 0,
            searches: 0,
            search_misses: 0,
            search_deferred: 0,
            lost: 0,
        }
    }
    pub(crate) fn save(&self, b: &mut Blob) {
        for v in [
            self.reclaimed,
            self.pieces,
            self.edits,
            self.searches,
            self.search_misses,
            self.search_deferred,
            self.lost,
        ] {
            b.u64(v);
        }
        for w in &self.work {
            match *w {
                None => b.u8(0),
                Some(Work::Piece { id, spent }) => {
                    b.u8(1);
                    b.u64(id);
                    b.u32(spent);
                }
                Some(Work::Tank(slot)) => {
                    b.u8(2);
                    b.u16(slot);
                }
            }
        }
    }
    /// The piece this slot works and the edits spent on it.
    #[cfg(test)]
    pub(crate) fn piece_work(&self, id: usize) -> Option<(u64, u32)> {
        match self.work.get(id).copied().flatten() {
            Some(Work::Piece { id, spent }) => Some((id, spent)),
            _ => None,
        }
    }
}

impl Mining {
    pub(crate) fn reclaim(&self) -> Option<&Reclaim> {
        self.reclaim.as_ref()
    }
}

/// Something a digger may take, and where it lies.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// A resting piece by its index in the fragments.
    Piece(usize),
    /// A breached tank by its slot.
    Tank(usize),
}

/// Resting pieces and breached tanks by grid cell of the reach, and resting
/// pieces by id. Built once a tick, in fragment and slot order.
struct Near {
    reach: i64,
    cells: BTreeMap<[i32; 2], Vec<(Target, [i32; 3])>>,
    pieces: BTreeMap<u64, usize>,
}

fn point(p: [fx::Fx; 3]) -> [i32; 3] {
    p.map(|v| v.to_int_trunc())
}

fn distance2(a: [i32; 3], b: [i32; 3]) -> i64 {
    (0..3)
        .map(|k| {
            let d = i64::from(a[k]) - i64::from(b[k]);
            d * d
        })
        .sum()
}

impl Near {
    fn build(
        reach: u32,
        fragments: &[Fragment],
        units: &[RangeUnit],
        tanks: &[Option<ContainerId>],
        breached: &[bool],
        ledger: &Ledger,
    ) -> Self {
        let mut near = Self {
            reach: i64::from(reach),
            cells: BTreeMap::new(),
            pieces: BTreeMap::new(),
        };
        for (i, f) in fragments.iter().enumerate() {
            if f.resting {
                let p = point([f.position.x, f.position.y, f.position.z]);
                near.insert(Target::Piece(i), p);
                near.pieces.insert(f.id, i);
            }
        }
        for (slot, tank) in tanks.iter().enumerate() {
            if tank.is_some_and(|t| breached[slot] && ledger.held(t) > 0) {
                near.insert(Target::Tank(slot), point(units[slot].motion.pos));
            }
        }
        near
    }
    fn cell(&self, p: [i32; 3]) -> [i32; 2] {
        let r = self.reach as i32;
        [p[0].div_euclid(r), p[2].div_euclid(r)]
    }
    fn insert(&mut self, t: Target, p: [i32; 3]) {
        let c = self.cell(p);
        self.cells.entry(c).or_default().push((t, p));
    }
    /// The grid cells around the face, in a fixed order: every target
    /// within the reach lies in one of them.
    fn around(&self, face: [i32; 3]) -> impl Iterator<Item = &(Target, [i32; 3])> + '_ {
        let [cx, cz] = self.cell(face);
        (-1..=1)
            .flat_map(move |dz| (-1..=1).map(move |dx| [cx + dx, cz + dz]))
            .filter_map(|c| self.cells.get(&c))
            .flatten()
    }
    fn within(&self, face: [i32; 3], p: [i32; 3]) -> bool {
        distance2(face, p) <= self.reach * self.reach
    }
}

/// Reclaim this tick, before the rock and from the same rate and sub-pool:
/// searches under the ceiling, then the work of each digger at its face,
/// the first served rotated by tick as the rock is. Returns whether the
/// sub-pool ran out with work waiting.
pub(super) fn reclaim(
    m: &mut Mining,
    units: &[RangeUnit],
    matter: &mut super::super::matter::Matter,
    tick: u32,
    diggers: &mut [Digger],
    left: &mut usize,
) -> bool {
    let Mining {
        tanks,
        inflight,
        reclaim: state,
        haul,
        ..
    } = m;
    let Some(rc) = state.as_mut() else {
        return false;
    };
    // Work belongs to a digger at its face: one that left forgets it.
    let mut at_face = vec![false; rc.work.len()];
    for d in diggers.iter() {
        at_face[d.id] = true;
    }
    for (w, here) in rc.work.iter_mut().zip(&at_face) {
        if !here {
            *w = None;
        }
    }
    if diggers.is_empty() {
        return false;
    }
    let fragments = &matter.debris.fragments;
    let ledger = &matter.economy.ledger;
    let near = Near::build(rc.data.reach, fragments, units, tanks, &rc.breached, ledger);
    let cap = |id: usize| ledger.get(tanks[id].expect("digger has a tank")).cap;
    // Work still there, still in reach and still fitting the tank.
    for d in diggers.iter() {
        let valid = match rc.work[d.id] {
            None => continue,
            Some(Work::Piece { id, .. }) => near.pieces.get(&id).is_some_and(|&i| {
                let f = &fragments[i];
                near.within(d.face, point([f.position.x, f.position.y, f.position.z]))
                    && ledger.held(f.held) <= cap(d.id)
            }),
            Some(Work::Tank(slot)) => {
                let slot = usize::from(slot);
                rc.breached[slot]
                    && ledger.held(tanks[slot].expect("a breached slot has a tank")) > 0
                    && near.within(d.face, point(units[slot].motion.pos))
            }
        };
        if !valid {
            rc.work[d.id] = None;
        }
    }
    let mut taken_by: BTreeSet<u64> = rc
        .work
        .iter()
        .filter_map(|w| match w {
            Some(Work::Piece { id, .. }) => Some(*id),
            _ => None,
        })
        .collect();
    // A digger with nothing around its face spends no search.
    let need: Vec<usize> = (0..diggers.len())
        .filter(|&i| {
            rc.work[diggers[i].id].is_none() && near.around(diggers[i].face).next().is_some()
        })
        .collect();
    let n = need.len();
    if n > 0 {
        let take = (rc.data.searches_per_tick as usize).min(n);
        if take < n {
            rc.search_deferred += 1;
        }
        // In u64: the same start on every pointer width.
        let start = (u64::from(tick) * take as u64 % n as u64) as usize;
        for step in 0..take {
            let d = diggers[need[(start + step) % n]];
            rc.searches += 1;
            let mut best: Option<((i64, u8, u64), Work)> = None;
            for &(t, p) in near.around(d.face) {
                if !near.within(d.face, p) {
                    continue;
                }
                let d2 = distance2(d.face, p);
                let (key, work) = match t {
                    Target::Piece(i) => {
                        let f = &fragments[i];
                        let held = ledger.held(f.held);
                        if taken_by.contains(&f.id) || held == 0 || held > cap(d.id) {
                            continue;
                        }
                        ((d2, 0, f.id), Work::Piece { id: f.id, spent: 0 })
                    }
                    Target::Tank(slot) => ((d2, 1, slot as u64), Work::Tank(slot as u16)),
                };
                if best.is_none_or(|(b, _)| key < b) {
                    best = Some((key, work));
                }
            }
            match best {
                Some((_, work)) => {
                    if let Work::Piece { id, .. } = work {
                        taken_by.insert(id);
                    }
                    rc.work[d.id] = Some(work);
                }
                None => rc.search_misses += 1,
            }
        }
    }
    // The work, the first digger rotated by tick as the rock's.
    let n = diggers.len();
    let start = tick as usize % n;
    let mut taken: BTreeSet<ContainerId> = BTreeSet::new();
    let mut starved = false;
    let ledger = &mut matter.economy.ledger;
    let cpe = rc.data.cargo_per_edit;
    for step in 0..n {
        let d = &mut diggers[(start + step) % n];
        let Some(work) = rc.work[d.id] else {
            continue;
        };
        let tank = tanks[d.id].expect("digger has a tank");
        let room = ledger.room(tank).saturating_sub(inflight[d.id]);
        let full = match work {
            Work::Piece { id, .. } => ledger.held(fragments[near.pieces[&id]].held) > room,
            Work::Tank(_) => room == 0,
        };
        if full {
            // The next thing at the face does not fit: the tank is full.
            d.full = true;
            if let Some(h) = haul.as_mut() {
                h.mark_full(d.id);
            }
            continue;
        }
        if *left == 0 {
            starved = true;
            break;
        }
        let budget = (d.rate as usize).min(*left);
        let used = match work {
            Work::Piece { id, spent } => {
                let f = &fragments[near.pieces[&id]];
                let spend = (f.count.max(1).saturating_sub(spent) as usize).min(budget) as u32;
                let spent = spent + spend;
                if spent >= f.count.max(1) {
                    let need = ledger.held(f.held);
                    assert_eq!(ledger.transfer(f.held, tank, need), need);
                    taken.insert(f.held);
                    rc.reclaimed += need;
                    rc.pieces += 1;
                    rc.work[d.id] = None;
                } else {
                    rc.work[d.id] = Some(Work::Piece { id, spent });
                }
                spend
            }
            Work::Tank(slot) => {
                let wreck = tanks[usize::from(slot)].expect("a breached slot has a tank");
                let most = (budget as u64).saturating_mul(cpe);
                let want = ledger.held(wreck).min(room).min(most);
                let got = ledger.transfer(wreck, tank, want);
                rc.reclaimed += got;
                if ledger.held(wreck) == 0 {
                    rc.work[d.id] = None;
                }
                got.div_ceil(cpe) as u32
            }
        };
        d.rate -= used;
        *left -= used as usize;
        rc.edits += u64::from(used);
    }
    if !taken.is_empty() {
        let super::super::matter::Matter {
            debris, economy, ..
        } = matter;
        debris.retire_reclaimed(economy, &taken);
    }
    starved
}

impl BlastRange {
    /// Whether the range reclaims: the snapshot is then version 64
    /// (ADR-674).
    pub(crate) fn has_reclaim(&self) -> bool {
        self.mining.as_ref().is_some_and(|m| m.reclaim.is_some())
    }

    /// Breach the tank of every digger that died this tick, at its end:
    /// the data's share of the cargo goes to ContainerBreach, the rest
    /// stays in the tank, which lies with the hull for reclaim.
    pub(crate) fn breach_dead_tanks(&mut self) {
        let Some(m) = self.mining.as_mut() else {
            return;
        };
        let Some(rc) = m.reclaim.as_mut() else {
            return;
        };
        let ledger = &mut self.matter.economy.ledger;
        let (num, den) = (rc.data.breach_num, rc.data.breach_den);
        for (id, u) in self.units.iter().enumerate() {
            let Some(tank) = m.tanks[id] else {
                continue;
            };
            if u.status.alive || rc.breached[id] {
                continue;
            }
            rc.breached[id] = true;
            rc.work[id] = None;
            // In u128 and down: the loss never exceeds the cargo.
            let held = u128::from(ledger.held(tank));
            let loss = (held * u128::from(num) / u128::from(den)) as Amount;
            rc.lost += ledger.sink(tank, loss, Sink::ContainerBreach);
        }
    }

    /// Version 64 tail: the reclaim's state, after the haul's. Fragments,
    /// tanks, units and orders are already loaded.
    pub(crate) fn load_reclaim(&mut self, s: &mut Scan) -> Option<()> {
        let m = self.mining.as_mut()?;
        let rc = m.reclaim.as_mut()?;
        rc.reclaimed = s.u64();
        rc.pieces = s.u64();
        rc.edits = s.u64();
        rc.searches = s.u64();
        rc.search_misses = s.u64();
        rc.search_deferred = s.u64();
        rc.lost = s.u64();
        if rc.search_misses > rc.searches || rc.pieces > rc.edits {
            return None;
        }
        for (id, u) in self.units.iter().enumerate() {
            rc.breached[id] = m.tanks[id].is_some() && !u.status.alive;
        }
        let pieces: BTreeMap<u64, &Fragment> = self
            .matter
            .debris
            .fragments
            .iter()
            .filter(|f| f.resting)
            .map(|f| (f.id, f))
            .collect();
        let mut worked = BTreeSet::new();
        for id in 0..self.units.len() {
            rc.work[id] = match s.u8() {
                0 => None,
                1 => Some(Work::Piece {
                    id: s.u64(),
                    spent: s.u32(),
                }),
                2 => Some(Work::Tank(s.u16())),
                _ => return None,
            };
            let Some(work) = rc.work[id] else {
                continue;
            };
            // Only a living digger with a dig order works at its face.
            let digs = matches!(
                self.battle.actors.get(id).map(|a| a.order),
                Some(super::battle::Order::Dig(_))
            );
            if m.tanks[id].is_none() || !self.units[id].status.alive || !digs {
                return None;
            }
            let valid = match work {
                Work::Piece { id: piece, spent } => {
                    worked.insert(piece)
                        && pieces.get(&piece).is_some_and(|f| spent < f.count.max(1))
                }
                Work::Tank(slot) => rc.breached.get(usize::from(slot)) == Some(&true),
            };
            if !valid {
                return None;
            }
        }
        s.ok().then_some(())
    }
}
