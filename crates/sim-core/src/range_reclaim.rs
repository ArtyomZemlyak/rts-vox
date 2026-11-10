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
//! **A dead producer's blank lies the same way** (ADR-675): the matter
//! paid into a producer's unfinished order breaches by the same share when
//! the producer dies, and the rest is reclaimed at its hull as a tank is.
//! A shot-down construction drone's confirmed batch also lies at its hull,
//! once, without a warehouse copy or a second container breach.
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
    /// The blank of the dead producer in this slot (ADR-675).
    Blank(u16),
}

/// Reclaim's state on a layout whose economy has a reclaim section.
pub struct Reclaim {
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
    /// A unit appended to the roster (ADR-675).
    pub(crate) fn push_unit(&mut self) {
        self.work.push(None);
        self.breached.push(false);
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
                Some(Work::Blank(slot)) => {
                    b.u8(3);
                    b.u16(slot);
                }
            }
        }
    }
    /// The piece this slot works and the edits spent on it.
    pub(crate) fn piece_work(&self, id: usize) -> Option<(u64, u32)> {
        match self.work.get(id).copied().flatten() {
            Some(Work::Piece { id, spent }) => Some((id, spent)),
            _ => None,
        }
    }
}

impl Mining {
    /// Read-only reclaim counters; work assignments remain internal.
    pub fn reclaim(&self) -> Option<&Reclaim> {
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
    /// A dead producer's blank by its slot.
    Blank(usize),
}

/// Resting pieces and breached tanks by grid cell of the reach, and resting
/// pieces by id. Built once a tick for the diggers' nearby cells, in
/// fragment and slot order. Remote heaps cannot be candidates or work.
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
    #[allow(clippy::too_many_arguments)]
    fn build(
        reach: u32,
        fragments: &[Fragment],
        units: &[RangeUnit],
        tanks: &[Option<ContainerId>],
        breached: &[bool],
        blanks: &[(usize, ContainerId)],
        ledger: &Ledger,
        diggers: &[Digger],
    ) -> Self {
        let mut near = Self {
            reach: i64::from(reach),
            cells: BTreeMap::new(),
            pieces: BTreeMap::new(),
        };
        // The same nine cells around every active face that around() reads.
        // Retain all broad-phase candidates, including outside the sphere:
        // they affect whether a miss spends a search and its rotation.
        let wanted: BTreeSet<_> = diggers
            .iter()
            .flat_map(|d| {
                let [cx, cz] = near.cell(d.face);
                (-1..=1).flat_map(move |dz| (-1..=1).map(move |dx| [cx + dx, cz + dz]))
            })
            .collect();
        let (lower, upper) = wanted
            .iter()
            .fold(([i32::MAX; 2], [i32::MIN; 2]), |(lo, hi), c| {
                (
                    std::array::from_fn(|k| lo[k].min(c[k])),
                    std::array::from_fn(|k| hi[k].max(c[k])),
                )
            });
        // A single face and a fully covered battlefield both form a rectangle.
        // In that exact case membership needs no tree lookup per fragment.
        let rectangular = (i64::from(upper[0]) - i64::from(lower[0]) + 1)
            * (i64::from(upper[1]) - i64::from(lower[1]) + 1)
            == wanted.len() as i64;
        let needed = |c: [i32; 2]| {
            if rectangular {
                (0..2).all(|k| lower[k] <= c[k] && c[k] <= upper[k])
            } else {
                wanted.contains(&c)
            }
        };
        for (i, f) in fragments.iter().enumerate() {
            if f.resting {
                let p = point([f.position.x, f.position.y, f.position.z]);
                if needed(near.cell(p)) {
                    near.insert(Target::Piece(i), p);
                    near.pieces.insert(f.id, i);
                }
            }
        }
        for (slot, tank) in tanks.iter().enumerate() {
            if tank.is_some_and(|t| breached[slot] && ledger.held(t) > 0) {
                let p = point(units[slot].motion.pos);
                if needed(near.cell(p)) {
                    near.insert(Target::Tank(slot), p);
                }
            }
        }
        for &(slot, blank) in blanks {
            if ledger.held(blank) > 0 {
                let p = point(units[slot].motion.pos);
                if needed(near.cell(p)) {
                    near.insert(Target::Blank(slot), p);
                }
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
    fn within(&self, face: [i32; 3], p: [i32; 3], reach: u32) -> bool {
        distance2(face, p) <= i64::from(reach).pow(2)
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
        reclaim_events,
        haul,
        production,
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
    let blanks = production.as_ref().map_or_else(Vec::new, |p| p.wrecks());
    let blank_of = |slot: u16| {
        blanks
            .iter()
            .find(|(s, _)| *s == usize::from(slot))
            .map(|&(_, c)| c)
    };
    let near = Near::build(
        rc.data
            .reach
            .max(if diggers.iter().any(|d| d.reclaim_only) {
                super::CRAWLER_RECLAIM_REACH
            } else {
                0
            }),
        fragments,
        units,
        tanks,
        &rc.breached,
        &blanks,
        ledger,
        diggers,
    );
    let cap = |id: usize| ledger.get(tanks[id].expect("digger has a tank")).cap;
    for d in diggers.iter_mut() {
        d.reclaim_pending = rc.work[d.id].is_some();
    }
    // Work still there, still in reach and still fitting the tank.
    let configured_reach = rc.data.reach;
    let reach = |d: &Digger| {
        if d.reclaim_only {
            configured_reach.max(super::CRAWLER_RECLAIM_REACH)
        } else {
            configured_reach
        }
    };
    for d in diggers.iter() {
        let valid = match rc.work[d.id] {
            None => continue,
            Some(Work::Piece { id, .. }) => near.pieces.get(&id).is_some_and(|&i| {
                let f = &fragments[i];
                near.within(
                    d.face,
                    point([f.position.x, f.position.y, f.position.z]),
                    reach(d),
                ) && ledger.held(f.held) <= cap(d.id)
            }),
            Some(Work::Tank(slot)) => {
                let slot = usize::from(slot);
                rc.breached[slot]
                    && ledger.held(tanks[slot].expect("a breached slot has a tank")) > 0
                    && near.within(d.face, point(units[slot].motion.pos), reach(d))
            }
            Some(Work::Blank(slot)) => blank_of(slot).is_some_and(|c| {
                ledger.held(c) > 0
                    && near.within(d.face, point(units[usize::from(slot)].motion.pos), reach(d))
            }),
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
        // A queued search is still work; only the bounded search below can
        // establish that the candidates are too distant, held or too large.
        for &i in &need {
            diggers[i].reclaim_pending = true;
        }
        let take = (rc.data.searches_per_tick as usize).min(n);
        if take < n {
            rc.search_deferred += 1;
        }
        // In u64: the same start on every pointer width.
        let start = (u64::from(tick) * take as u64 % n as u64) as usize;
        for step in 0..take {
            let index = need[(start + step) % n];
            let d = diggers[index];
            rc.searches += 1;
            let mut best: Option<((u8, i64, u8, u64), Work)> = None;
            for &(t, p) in near.around(d.face) {
                if !near.within(d.face, p, reach(&d)) {
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
                        let room = ledger.room(tanks[d.id].expect("digger tank")).saturating_sub(inflight[d.id]);
                        ((u8::from(held > room), d2, 0, f.id), Work::Piece { id: f.id, spent: 0 })
                    }
                    Target::Tank(slot) => ((0, d2, 1, slot as u64), Work::Tank(slot as u16)),
                    Target::Blank(slot) => ((0, d2, 2, slot as u64), Work::Blank(slot as u16)),
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
                None => {
                    rc.search_misses += 1;
                    diggers[index].reclaim_pending = false;
                }
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
            Work::Tank(_) | Work::Blank(_) => room == 0,
        };
        if full {
            if d.reclaim_only && d.k.kind == modding::range::RangeUnitKind::Crawler && room > 0 {
                // Search the neighbouring field for a smaller fitting piece
                // before treating a single oversized fragment as a full hold.
                rc.work[d.id] = None;
                d.reclaim_pending = false;
                continue;
            }
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
                // A shape-preserving wreck can take several ticks. Show work
                // immediately while its indivisible container is processed;
                // these presentation samples do not transfer or create matter.
                if spend > 0 && d.reclaim_only && reclaim_events.len() < super::RECLAIM_VISUAL_LIMIT {
                    let (lo,hi)=super::reclaim_piece_bounds(f);
                    let collector=point(units[d.id].motion.pos);
                    reclaim_events.push(super::ReclaimVisual {
                        from: std::array::from_fn(|a|collector[a].clamp(lo[a],hi[a])),
                        crawler: d.id,
                        material: f.material,
                    });
                }
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
            Work::Tank(_) | Work::Blank(_) => {
                let wreck = match work {
                    Work::Tank(slot) => {
                        tanks[usize::from(slot)].expect("a breached slot has a tank")
                    }
                    Work::Blank(slot) => blank_of(slot).expect("a worked blank lies"),
                    Work::Piece { .. } => unreachable!(),
                };
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

impl Reclaim {
    /// Slots whose work is a dead producer's blank.
    pub(crate) fn blank_work(&self) -> impl Iterator<Item = usize> + '_ {
        self.work.iter().filter_map(|w| match w {
            Some(Work::Blank(slot)) => Some(usize::from(*slot)),
            _ => None,
        })
    }
    /// The breach share of a container's matter: `breach_num / breach_den`,
    /// down, never more than it holds.
    pub(crate) fn breach_loss(&self, held: Amount) -> Amount {
        (u128::from(held) * u128::from(self.data.breach_num) / u128::from(self.data.breach_den))
            as Amount
    }
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
        let cargo_unit = self.matter.economy.per_voxel(2);
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
            if u.kind == modding::range::RangeUnitKind::Crawler {
                // Cargo is universal matter. Show a bounded part as real rock
                // fragments, conserving its ledger quantity. Excess and any
                // sub-voxel remainder stay reclaimable in the wreck tank.
                let material = 2;
                let unit = cargo_unit;
                let room = self.matter.debris.fragment_room(&self.matter.tuning).min(32);
                if unit > 0 {
                    let pieces = room.min((ledger.held(tank) / unit).min(u64::from(u32::MAX)) as u32);
                    let half_width = (i32::from(u.body.vol.nz) * i32::from(u.body.mm)
                        / i32::from(voxel::unit::WORLD_MM) / 2).max(1);
                    for n in 0..pieces {
                        // Divide small loads between both sides too, rather
                        // than hiding a single full piece inside the body.
                        let remaining = ledger.held(tank) / unit;
                        let count = remaining.div_ceil(u64::from(pieces - n)).min(64) as u32;
                        if count == 0 { break; }
                        let held = ledger.create(crate::matter::Kind::Debris, 0, u64::MAX);
                        let amount = unit * u64::from(count);
                        assert_eq!(ledger.transfer(tank, held, amount), amount);
                        let lane = ((n / 2) % 8) as i32 - 3;
                        let side = if n % 2 == 0 { -1 } else { 1 };
                        let row = (n / 16) as i32;
                        let base = phys::V3::new(u.motion.pos[0], u.motion.pos[1], u.motion.pos[2]);
                        let position = base
                            .add(u.pose.x.scale(fx::Fx::from_int(lane * 10)))
                            .add(u.pose.z.scale(fx::Fx::from_int(side * (half_width + 5 + row * 3))))
                            .add(phys::V3::new(fx::Fx::ZERO, fx::Fx::from_int(25 + row * 4), fx::Fx::ZERO));
                        let velocity = u.pose.z.scale(fx::Fx::from_ratio(side * (7 + row), 10))
                            .add(u.pose.x.scale(fx::Fx::from_ratio(lane, 20)))
                            .add(phys::V3::new(fx::Fx::ZERO, fx::Fx::from_ratio(4 + row, 5), fx::Fx::ZERO));
                        let density = self.matter.tuning.density_kg_m3_per_density as u64;
                        self.matter.debris.next_id += 1;
                        let mut fragment = Fragment {
                            id: self.matter.debris.next_id,
                            position, velocity,
                            material, paint: None, mm: voxel::unit::WORLD_MM,
                            basis: crate::burst::IDENTITY,
                            mass_g: unit * density * u64::from(count), count,
                            shape: if count == 64 { u64::MAX } else { (1u64 << count) - 1 },
                            side: 4, chunk: None, held, resting: false,
                        };
                        fragment.keep_on_map([self.size[0], self.size[2]]);
                        self.matter.debris.fragments.push(fragment);
                    }
                }
                continue;
            }
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
                // Checked against the production's blanks once they load.
                3 => Some(Work::Blank(s.u16())),
                _ => return None,
            };
            let Some(work) = rc.work[id] else {
                continue;
            };
            // Only a living digger with a dig order works at its face.
            let digs = matches!(
                self.battle.actors.get(id).map(|a| a.order),
                Some(super::battle::Order::Dig(_) | super::battle::Order::Reclaim(_))
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
                Work::Blank(slot) => usize::from(slot) < self.units.len(),
            };
            if !valid {
                return None;
            }
        }
        s.ok().then_some(())
    }
}
