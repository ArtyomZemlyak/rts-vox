//! Production and produced units (plan 245, slice 4; ADR-675).
//!
//! **A producer is a unit kind with a list.** `produces` of the economy
//! table names what a living unit of the kind builds; `build_secs` of the
//! product is its time. The cost is not a number of the data: a unit costs
//! the cells of its template times the density of body material, exactly
//! the matter an intact body of the kind holds.
//!
//! **A line pays as it stands.** Each producer with orders has one line:
//! its queue of kinds and a blank, a container of the matter paid for the
//! front order. Every tick, in producer slot order, a line moves
//! `cost / build ticks` (rounded up) from its side's store into its blank,
//! and stands one more tick. A store that cannot pay the share stalls the
//! line — counted, and the line pays on a later tick; it never stops the
//! other lines. The unit is finished when it is both paid for and stood.
//!
//! **A finished unit joins the roster at the end of the tick.** After every
//! other phase, the line's unit is appended to every per-unit list of the
//! range at a free spot beside its producer, and the blank's matter moves
//! into its body container: a transfer, so nothing is made or lost.
//! Slots are never reused and never reordered: the command journal and
//! the snapshot address units by slot. At the roster ceiling
//! ([`MAX_RANGE_UNITS`]) or with no free spot the finished unit waits in
//! its line, counted, and tries again next tick.
//!
//! **Cancel refunds.** Cancelling drops the last order; when it was the
//! only one, the blank goes back to the store, and what no longer fits
//! the store's cap goes to OverCap.
//!
//! **A dead producer's blank is a wreck.** At the end of the tick its
//! producer dies, the line goes; with a reclaim section the blank loses
//! the breach share to ContainerBreach, and the rest lies with the hull,
//! reclaimed by the dig order as a dead digger's tank is.
use super::super::placement::Validator;
use super::super::{battle, motion_tuning, BlastRange, RangeUnit};
use super::Mining;
use crate::matter::{Amount, ContainerId, Kind, Sink};
use fx::blob::{Blob, Scan};
use modding::range::{RangeUnitDef, RangeUnitKind, MAX_RANGE_UNITS};
use modding::range_economy::RangeProduction;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[cfg(test)]
#[path = "range_produce_tests.rs"]
mod tests;

/// A produced unit: where it was placed, its side and the containers it
/// brought. With the authored layout it is the roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawned {
    pub def: RangeUnitDef,
    pub body: ContainerId,
    pub tank: Option<ContainerId>,
}

/// One producer's orders and the matter paid for the front one.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    slot: u16,
    queue: VecDeque<RangeUnitKind>,
    blank: ContainerId,
    /// Ticks the front order has stood, up to its build ticks.
    ticks: u32,
}

/// Production's state on a layout whose economy has a production section.
pub struct Production {
    /// Orders one producer holds.
    queue: u32,
    /// Sorted by producer slot: ties between producers drawing on one
    /// store go by linear index, not by who ordered first.
    lines: Vec<Line>,
    /// Dead producers' blanks and shot-down drones' cargo by slot: they lie with the hull.
    wrecks: BTreeMap<u16, ContainerId>,
    /// Derived from the data and the body templates; never saved.
    costs: [Amount; RangeUnitKind::COUNT],
    build: [u32; RangeUnitKind::COUNT],
    /// Exact occupied cells shared with layout validation. Prepared for
    /// producible kinds at scene construction, never built in a tick/saved.
    placement: Validator,
    /// Matter paid into blanks, units produced, matter refunded to stores,
    /// refunds over the cap lost to OverCap, line-ticks the store could not
    /// pay the share, line-ticks a finished unit waited at the roster
    /// ceiling, line-ticks it found no free spot, blank matter lost to
    /// ContainerBreach.
    pub paid: Amount,
    pub produced: u64,
    pub refunded: Amount,
    pub overflow: Amount,
    pub stalled: u64,
    pub capped: u64,
    pub blocked: u64,
    pub lost: Amount,
}

impl Production {
    pub(crate) fn new(data: &RangeProduction) -> Self {
        let placement = Validator::default();
        for producer in &data.producers {
            for &kind in &producer.produces {
                placement.prepare_product(kind);
            }
        }
        Self {
            queue: data.queue,
            lines: Vec::new(),
            wrecks: BTreeMap::new(),
            costs: [0; RangeUnitKind::COUNT],
            build: [0; RangeUnitKind::COUNT],
            placement,
            paid: 0,
            produced: 0,
            refunded: 0,
            overflow: 0,
            stalled: 0,
            capped: 0,
            blocked: 0,
            lost: 0,
        }
    }
    /// Dead producers' blanks: slot and container, in slot order.
    pub(crate) fn wrecks(&self) -> Vec<(usize, ContainerId)> {
        self.wrecks
            .iter()
            .map(|(&slot, &c)| (usize::from(slot), c))
            .collect()
    }
    fn line(&self, slot: usize) -> Option<&Line> {
        self.lines
            .binary_search_by_key(&slot, |l| usize::from(l.slot))
            .ok()
            .map(|i| &self.lines[i])
    }
    /// Whether anything of production differs from a range that never
    /// produced: the snapshot is then version 65.
    fn active(&self) -> bool {
        !self.lines.is_empty()
            || !self.wrecks.is_empty()
            || [
                self.paid,
                self.produced,
                self.refunded,
                self.overflow,
                self.stalled,
                self.capped,
                self.blocked,
                self.lost,
            ]
            .iter()
            .any(|v| *v != 0)
    }
    fn save(&self, b: &mut Blob) {
        for v in [
            self.paid,
            self.produced,
            self.refunded,
            self.overflow,
            self.stalled,
            self.capped,
            self.blocked,
            self.lost,
        ] {
            b.u64(v);
        }
        b.u32(self.lines.len() as u32);
        for l in &self.lines {
            b.u16(l.slot);
            b.u32(l.blank);
            b.u32(l.ticks);
            b.u8(l.queue.len() as u8);
            for k in &l.queue {
                b.u8(k.index() as u8);
            }
        }
        b.u32(self.wrecks.len() as u32);
        for (&slot, &c) in &self.wrecks {
            b.u16(slot);
            b.u32(c);
        }
    }
}

impl Mining {
    /// Read-only production counters; lines and their blanks remain internal.
    pub fn production(&self) -> Option<&Production> {
        self.production.as_ref()
    }
    /// Cost and build ticks of every product, from the templates and the
    /// data, once at construction.
    pub(in super::super) fn price_products(&mut self, matter: &super::super::matter::Matter) {
        let Some(p) = self.production.as_mut() else {
            return;
        };
        for k in &self.data.kinds {
            if k.build_secs == 0 {
                continue;
            }
            let i = k.kind.index();
            p.costs[i] = matter.template_matter(&RangeUnit::of_kind(k.kind, [0; 3]));
            p.build[i] = k.build_secs.saturating_mul(crate::TICKS_PER_SECOND as u32);
        }
    }
}

pub fn starting_crews(layout: &mut modding::range::RangeLayout) {
    if !layout.battle || layout.economy.is_none() {
        return;
    }
    let stores: Vec<RangeUnitDef> = layout
        .units
        .iter()
        .filter(|u| u.allied && u.kind == RangeUnitKind::GrebenStorage)
        .cloned()
        .collect();
    for store in stores {
        let near = |u: &RangeUnitDef, kind, reach: i64| {
            u.allied
                && u.kind == kind
                && (0..3)
                    .map(|a| {
                        let d = i64::from(u.at[a] - store.at[a]);
                        d * d
                    })
                    .sum::<i64>()
                    <= reach * reach
        };
        let drones = layout
            .units
            .iter()
            .filter(|u| near(u, RangeUnitKind::SkobaDrone, 1400))
            .count();
        for n in drones..8 {
            if layout.units.len() >= MAX_RANGE_UNITS {
                break;
            }
            layout.units.push(RangeUnitDef {
                allied: true,
                kind: RangeUnitKind::SkobaDrone,
                at: [store.at[0] + 800, 60, store.at[2] - 240 + n as i32 * 70],
                weapon: None,
            });
        }
        if !layout
            .units
            .iter()
            .any(|u| near(u, RangeUnitKind::Crawler, 2000))
            && layout.units.len() < MAX_RANGE_UNITS
        {
            layout.units.push(RangeUnitDef {
                allied: true,
                kind: RangeUnitKind::Crawler,
                at: [store.at[0] + 800, store.at[1], store.at[2] + 500],
                weapon: None,
            });
        }
    }
}

impl BlastRange {
    /// The roster's definitions in slot order: the layout's units, then
    /// the produced ones. The authored layout is never changed by
    /// production, so a snapshot carries it as it was written.
    pub fn roster(&self) -> impl Iterator<Item = &RangeUnitDef> + '_ {
        self.layout
            .iter()
            .flat_map(|l| l.units.iter())
            .chain(self.spawned.iter().map(|s| &s.def))
    }
    /// The definition of the unit in this slot, authored or produced.
    pub fn unit_def(&self, id: usize) -> Option<&RangeUnitDef> {
        let authored = self.layout.as_ref().map_or(0, |l| l.units.len());
        match id.checked_sub(authored) {
            None => self.layout.as_ref().and_then(|l| l.units.get(id)),
            Some(i) => self.spawned.get(i).map(|s| &s.def),
        }
    }
    /// Units produced during the run, in slot order after the layout's.
    pub fn spawned(&self) -> &[Spawned] {
        &self.spawned
    }
    /// Units the layout placed: the slots before every produced one.
    pub(crate) fn authored_units(&self) -> usize {
        self.units.len() - self.spawned.len()
    }
    /// Orders queued at this producer, front first; `None` without a line.
    pub fn production_queue(&self, id: usize) -> Option<Vec<RangeUnitKind>> {
        let p = self.mining.as_ref()?.production.as_ref()?;
        Some(p.line(id)?.queue.iter().copied().collect())
    }
    /// Matter paid for this producer's front order.
    pub fn production_paid(&self, id: usize) -> Option<Amount> {
        let p = self.mining.as_ref()?.production.as_ref()?;
        Some(self.matter.economy.ledger.held(p.line(id)?.blank))
    }
    /// Cargo left at a dead producer or shot-down drone, before reclaim finds it.
    pub fn wreck_cargo(&self, id: usize) -> Option<Amount> {
        let p = self.mining.as_ref()?.production.as_ref()?;
        let container = p.wrecks.get(&(id as u16))?;
        Some(self.matter.economy.ledger.held(*container))
    }
    /// What one unit of this kind costs, matter units; `None` for a kind
    /// nobody produces.
    pub fn production_cost(&self, kind: RangeUnitKind) -> Option<Amount> {
        let p = self.mining.as_ref()?.production.as_ref()?;
        Some(p.costs[kind.index()]).filter(|c| *c > 0)
    }
    /// Producer slots with lines, in slot order.
    pub fn production_lines(&self) -> Vec<usize> {
        self.mining
            .as_ref()
            .and_then(|m| m.production.as_ref())
            .map_or_else(Vec::new, |p| {
                p.lines.iter().map(|l| usize::from(l.slot)).collect()
            })
    }
    /// Whether the snapshot carries production: version 65 (ADR-675).
    pub(crate) fn has_production(&self) -> bool {
        self.mining
            .as_ref()
            .and_then(|m| m.production.as_ref())
            .is_some_and(Production::active)
    }

    /// Append a unit to every per-unit list, at the end: the one place the
    /// roster grows, at runtime and when a snapshot is restored.
    pub(super) fn append_unit(&mut self, s: Spawned) -> usize {
        let id = self.units.len();
        let battle = self.layout.as_ref().is_some_and(|l| l.battle);
        let mut u = RangeUnit::placed(s.def.kind, s.def.allied, s.def.at, battle);
        if let Some(c) = self.layout.as_ref().and_then(|l| l.course.as_ref()) {
            u.apply_course(c);
        }
        // The gun's defaults stay last.
        self.motion_defaults
            .insert(id, motion_tuning::values(&u.motion));
        self.battle.push_actor(&u);
        self.matter.push_body(s.body);
        self.combustion.push_unit();
        if let Some(m) = self.mining.as_mut() {
            m.push_unit(s.def.allied, s.tank);
        }
        self.units.push(u);
        self.spawned.push(s);
        let sides: Vec<bool> = (0..=id).map(|id| self.allied(id)).collect();
        if let Some(c) = self.mining.as_mut().and_then(|m| m.construction.as_mut()) {
            c.push_unit(id, &self.units, &sides);
        }
        id
    }

    /// Produce a unit from a paid blank: its body container takes exactly
    /// the blank's cost, a digging kind gets an empty tank, and the unit
    /// joins the roster. Called only at the end of a tick.
    pub(crate) fn spawn(
        &mut self,
        kind: RangeUnitKind,
        allied: bool,
        at: [i32; 3],
        blank: ContainerId,
    ) -> usize {
        let m = self.mining.as_ref().expect("production has an economy");
        let cost = m.production.as_ref().expect("a line has production").costs[kind.index()];
        let tank = m.data.kind(kind).map_or(0, |k| k.tank);
        let ledger = &mut self.matter.economy.ledger;
        let body = ledger.create(Kind::Body, 0, cost);
        assert_eq!(ledger.transfer(blank, body, cost), cost, "a paid blank");
        let tank = (tank > 0).then(|| ledger.create(Kind::Hauler, 0, tank));
        self.append_unit(Spawned {
            def: RangeUnitDef {
                allied,
                kind,
                at,
                weapon: None,
            },
            body,
            tank,
        })
    }

    pub fn upgrade_kil(&mut self, id: usize) -> bool {
        let next = match self.units.get(id).map(|u| u.kind) {
            Some(RangeUnitKind::KilFactoryT1) => RangeUnitKind::KilFactoryT2,
            Some(RangeUnitKind::KilFactoryT2) => RangeUnitKind::KilFactoryT3,
            Some(RangeUnitKind::KilFactoryT3) => RangeUnitKind::KilFactoryT4,
            _ => return false,
        };
        let at = self.units[id].at;
        let motion = self.units[id].motion.clone();
        let mut replacement = RangeUnit::of_kind(next, at);
        let head = motion.drive.head;
        replacement.motion = motion;
        replacement.update_pose(head);
        self.units[id] = replacement;
        if let Some(def) = self
            .layout
            .as_mut()
            .and_then(|layout| layout.units.get_mut(id))
        {
            def.kind = next;
        }
        true
    }

    fn production_open(&self) -> bool {
        self.battle_mode()
            && matches!(
                self.battle.phase,
                battle::Phase::Setup | battle::Phase::Fighting
            )
    }
    /// Queue `kind` at producer `actor` of `side`: a living unit whose kind
    /// produces it, with room in its queue. The first order opens the line
    /// and its empty blank.
    pub(crate) fn enqueue_product(
        &mut self,
        actor: usize,
        kind: RangeUnitKind,
        side: bool,
    ) -> battle::commands::Outcome {
        use battle::commands::Outcome;
        if !self.production_open()
            || self.allied(actor) != side
            || !self.units.get(actor).is_some_and(|u| u.status.alive)
        {
            return Outcome::Rejected;
        }
        let producer = self.units[actor].kind;
        let Some(m) = self.mining.as_mut() else {
            return Outcome::Rejected;
        };
        if !m.data.produces(producer, kind) {
            return Outcome::Rejected;
        }
        let Some(p) = m.production.as_mut() else {
            return Outcome::Rejected;
        };
        let at = p
            .lines
            .binary_search_by_key(&actor, |l| usize::from(l.slot));
        if at.is_ok_and(|i| p.lines[i].queue.len() >= p.queue as usize) {
            return Outcome::Rejected;
        }
        let i = match at {
            Ok(i) => i,
            Err(i) => {
                let blank = self
                    .matter
                    .economy
                    .ledger
                    .create(Kind::Body, 0, Amount::MAX);
                p.lines.insert(
                    i,
                    Line {
                        slot: actor as u16,
                        queue: VecDeque::new(),
                        blank,
                        ticks: 0,
                    },
                );
                i
            }
        };
        p.lines[i].queue.push_back(kind);
        Outcome::Applied
    }
    /// Cancel the last order at producer `actor` of `side`. Cancelling the
    /// only order returns the blank to the store; what exceeds its cap goes
    /// to OverCap, as the World kernel's `drop_line`.
    pub(crate) fn cancel_product(&mut self, actor: usize, side: bool) -> battle::commands::Outcome {
        use battle::commands::Outcome;
        if !self.production_open()
            || self.allied(actor) != side
            || !self.units.get(actor).is_some_and(|u| u.status.alive)
        {
            return Outcome::Rejected;
        }
        let Some(m) = self.mining.as_mut() else {
            return Outcome::Rejected;
        };
        let (Some(p), Some(h)) = (m.production.as_mut(), m.haul.as_ref()) else {
            return Outcome::Rejected;
        };
        let Ok(i) = p
            .lines
            .binary_search_by_key(&actor, |l| usize::from(l.slot))
        else {
            return Outcome::Rejected;
        };
        let line = &mut p.lines[i];
        line.queue.pop_back();
        if line.queue.is_empty() {
            let blank = line.blank;
            // An ordered remove: the lines stay sorted by slot.
            p.lines.remove(i);
            let ledger = &mut self.matter.economy.ledger;
            let store = h.store(side);
            p.refunded += ledger.transfer(blank, store, ledger.held(blank));
            let rest = ledger.held(blank);
            p.overflow += ledger.sink(blank, rest, Sink::OverCap);
            assert!(ledger.retire_empty(blank), "the refunded blank is empty");
        }
        Outcome::Applied
    }

    /// Production at the end of the tick, after every other phase: dead
    /// producers' lines become wrecks, the lines pay and stand in slot
    /// order, and finished units join the roster.
    pub(crate) fn production_phase(&mut self) {
        let open = self.production_open();
        let sides: Vec<bool> = (0..self.units.len()).map(|id| self.allied(id)).collect();
        let Some(m) = self.mining.as_mut() else {
            return;
        };
        let Mining {
            production,
            haul,
            reclaim,
            ..
        } = m;
        let Some(p) = production.as_mut() else {
            return;
        };
        if p.lines.is_empty() {
            return;
        }
        let ledger = &mut self.matter.economy.ledger;
        // A dead producer's blank shares its fate (07-economy, «Петля»,
        // п. 6): the breach share is lost, the rest lies with the hull.
        let dead: Vec<usize> = (0..p.lines.len())
            .filter(|&i| !self.units[usize::from(p.lines[i].slot)].status.alive)
            .collect();
        for &i in dead.iter().rev() {
            let line = p.lines.remove(i);
            if let Some(rc) = reclaim.as_ref() {
                let loss = rc.breach_loss(ledger.held(line.blank));
                p.lost += ledger.sink(line.blank, loss, Sink::ContainerBreach);
            }
            if ledger.held(line.blank) == 0 {
                ledger.retire_empty(line.blank);
            } else {
                p.wrecks.insert(line.slot, line.blank);
            }
        }
        if !open {
            return;
        }
        let Some(h) = haul.as_ref() else {
            return;
        };
        let mut finished = Vec::new();
        for line in &mut p.lines {
            let front = *line.queue.front().expect("a line has an order");
            let (cost, build) = (p.costs[front.index()], p.build[front.index()]);
            let paid = ledger.held(line.blank);
            let want = cost.div_ceil(u64::from(build.max(1))).min(cost - paid);
            if want > 0 {
                let store = h.store(sides[usize::from(line.slot)]);
                let moved = ledger.transfer(store, line.blank, want);
                p.paid += moved;
                if moved < want {
                    p.stalled += 1;
                }
            }
            line.ticks = line.ticks.saturating_add(1).min(build);
            if ledger.held(line.blank) == cost && line.ticks >= build {
                finished.push((usize::from(line.slot), front, line.blank));
            }
        }
        for (slot, kind, blank) in finished {
            let allied = sides[slot];
            if self.units.len() >= MAX_RANGE_UNITS {
                self.production_mut().capped += 1;
                continue;
            }
            let Some(at) = self.free_spot(slot, kind, allied) else {
                self.production_mut().blocked += 1;
                continue;
            };
            self.spawn(kind, allied, at, blank);
            let p = self.production_mut();
            p.produced += 1;
            let i = p
                .lines
                .binary_search_by_key(&slot, |l| usize::from(l.slot))
                .expect("a finished line");
            let line = &mut p.lines[i];
            line.queue.pop_front();
            line.ticks = 0;
            if line.queue.is_empty() {
                p.lines.remove(i);
                assert!(
                    self.matter.economy.ledger.retire_empty(blank),
                    "a spent blank is empty"
                );
            }
        }
    }
    fn production_mut(&mut self) -> &mut Production {
        self.mining
            .as_mut()
            .and_then(|m| m.production.as_mut())
            .expect("production")
    }

    pub(crate) fn note_construction_spawn(&mut self) {
        self.production_mut().produced += 1;
    }

    pub(crate) fn drop_drone_cargo(&mut self, id: usize, container: ContainerId) {
        self.production_mut().wrecks.insert(id as u16, container);
    }

    /// A free spot beside the producer for a unit of `kind`: positions on
    /// a grid of the unit's own box around the producer, nearest first,
    /// ties by the grid's linear index. A spot is free when its box lies
    /// inside the world off the frame, clear of every unit's box (living
    /// or dead, and of the gun outside battle) and its body cells clear of
    /// solid ground.
    fn free_spot(&self, producer: usize, kind: RangeUnitKind, allied: bool) -> Option<[i32; 3]> {
        let battle = self.layout.as_ref().is_some_and(|l| l.battle);
        let p = &self.units[producer];
        let c = p.motion.pos.map(|v| v.to_int_trunc());
        let probe = RangeUnit::placed(kind, allied, c, battle);
        let (lo, hi) = probe.bounds();
        let (plo, phi) = p.bounds();
        let step = [(hi[0] - lo[0]).max(1), (hi[2] - lo[2]).max(1)];
        // Rings enough to clear the producer's box, and two beyond it.
        let reach = ((phi[0] - plo[0]) / step[0]).max((phi[2] - plo[2]) / step[1]) + 2;
        let side = 2 * reach + 1;
        let mut grid: Vec<(i64, i32, [i32; 2])> = Vec::new();
        for gz in -reach..=reach {
            for gx in -reach..=reach {
                if gx == 0 && gz == 0 {
                    continue;
                }
                let off = [gx * step[0], gz * step[1]];
                let d = i64::from(off[0]).pow(2) + i64::from(off[1]).pow(2);
                grid.push((d, (gz + reach) * side + gx + reach, off));
            }
        }
        grid.sort_unstable_by_key(|g| (g.0, g.1));
        let boxes: Vec<([i32; 3], [i32; 3])> = self
            .units
            .iter()
            .chain((!self.battle_mode()).then_some(&self.gun))
            .map(|u| u.bounds())
            .collect();
        let size = self.size.map(|v| v as i32);
        let placement = &self.mining.as_ref()?.production.as_ref()?.placement;
        'spots: for (_, _, off) in grid {
            let shift = [off[0], 0, off[1]];
            let a: [i32; 3] = std::array::from_fn(|k| lo[k] + shift[k]);
            let b: [i32; 3] = std::array::from_fn(|k| hi[k] + shift[k]);
            if a[0] < 1
                || a[2] < 1
                || a[1] < 1
                || b[0] >= size[0] - 1
                || b[2] >= size[2] - 1
                || b[1] >= size[1]
            {
                continue;
            }
            for (olo, ohi) in &boxes {
                if (0..3).all(|k| a[k] < ohi[k] && olo[k] < b[k]) {
                    continue 'spots;
                }
            }
            let at = [c[0] + off[0], c[1], c[2] + off[1]];
            if placement.product_intersects(kind, battle && !allied, &self.world, at, (a, b)) {
                continue;
            }
            return Some(at);
        }
        None
    }

    /// Version 65: the produced units, right after the layout, so every
    /// per-unit section that follows covers them.
    pub(crate) fn save_spawned(&self, b: &mut Blob) {
        b.u32(self.spawned.len() as u32);
        for s in &self.spawned {
            b.u8(s.def.kind.index() as u8);
            b.bool(s.def.allied);
            for v in s.def.at {
                b.i32(v);
            }
            b.u32(s.body);
            b.bool(s.tank.is_some());
            if let Some(t) = s.tank {
                b.u32(t);
            }
        }
    }
    /// Append the produced units of a version-65 snapshot before anything
    /// per unit loads. Their containers are checked when the ledger is.
    pub(crate) fn load_spawned(&mut self, s: &mut Scan) -> Option<()> {
        let n = s.u32() as usize;
        let data = self.mining.as_ref()?.data.clone();
        if n > MAX_RANGE_UNITS.saturating_sub(self.units.len())
            || !self.battle_mode()
            || (n > 0 && !data.any_producer())
        {
            return None;
        }
        for _ in 0..n {
            let kind = *RangeUnitKind::ALL.get(usize::from(s.u8()))?;
            let allied = s.bool();
            let at: [i32; 3] = std::array::from_fn(|_| s.i32());
            let body = s.u32();
            let tank = if s.bool() { Some(s.u32()) } else { None };
            // Something produces it, and it has a tank exactly if it digs.
            if !(data.produced(kind)
                || matches!(
                    kind,
                    RangeUnitKind::TurretT1
                        | RangeUnitKind::KilFactoryT1
                        | RangeUnitKind::GrebenStorage
                        | RangeUnitKind::Crawler
                ))
                || tank.is_some() != data.digs(kind)
                || (0..3).any(|a| at[a] < 0 || at[a] >= self.size[a] as i32)
                || !s.ok()
            {
                return None;
            }
            self.append_unit(Spawned {
                def: RangeUnitDef {
                    allied,
                    kind,
                    at,
                    weapon: None,
                },
                body,
                tank,
            });
        }
        s.ok().then_some(())
    }
    pub(crate) fn save_production(&self, b: &mut Blob) {
        if let Some(p) = self.mining.as_ref().and_then(|m| m.production.as_ref()) {
            p.save(b);
        }
    }
    /// Version 65 tail: production, after the reclaim's. Units, orders and
    /// the ledger are loaded.
    pub(crate) fn load_production(&mut self, s: &mut Scan, construction: bool) -> Option<()> {
        let units = self.units.len();
        let spawned = self.spawned.len();
        let alive: Vec<bool> = self.units.iter().map(|u| u.status.alive).collect();
        let kinds: Vec<RangeUnitKind> = self.units.iter().map(|u| u.kind).collect();
        let m = self.mining.as_mut()?;
        let data = &m.data;
        let p = m.production.as_mut()?;
        p.paid = s.u64();
        p.produced = s.u64();
        p.refunded = s.u64();
        p.overflow = s.u64();
        p.stalled = s.u64();
        p.capped = s.u64();
        p.blocked = s.u64();
        p.lost = s.u64();
        if p.produced != spawned as u64
            || p.refunded.checked_add(p.overflow)? > p.paid
            || p.lost > p.paid
        {
            return None;
        }
        let ledger = &self.matter.economy.ledger;
        let blank_ok = |id: ContainerId| {
            ledger
                .try_get(id)
                .is_some_and(|c| c.alive && c.kind == Kind::Body)
        };
        let n = s.u32() as usize;
        if n > units {
            return None;
        }
        p.lines.clear();
        for _ in 0..n {
            let slot = s.u16();
            let blank = s.u32();
            let ticks = s.u32();
            let len = usize::from(s.u8());
            if len == 0 || len > p.queue as usize {
                return None;
            }
            let mut queue = VecDeque::with_capacity(len);
            for _ in 0..len {
                queue.push_back(*RangeUnitKind::ALL.get(usize::from(s.u8()))?);
            }
            let id = usize::from(slot);
            let front = queue[0].index();
            if id >= units
                || p.lines.last().is_some_and(|l| l.slot >= slot)
                || !queue.iter().all(|k| data.produces(kinds[id], *k))
                || !blank_ok(blank)
                || ledger.held(blank) > p.costs[front]
                || ticks > p.build[front]
            {
                return None;
            }
            p.lines.push(Line {
                slot,
                queue,
                blank,
                ticks,
            });
        }
        let n = s.u32() as usize;
        if n > units {
            return None;
        }
        p.wrecks.clear();
        for _ in 0..n {
            let slot = s.u16();
            let blank = s.u32();
            let id = usize::from(slot);
            if id >= units
                || alive[id]
                || (data.products(kinds[id]).is_empty() && kinds[id] != RangeUnitKind::SkobaDrone)
                || p.wrecks
                    .last_key_value()
                    .is_some_and(|(&last, _)| last >= slot)
                || !blank_ok(blank)
            {
                return None;
            }
            p.wrecks.insert(slot, blank);
        }
        // A wreck lies at a dead producer, a line at a living one; a
        // container belongs to one owner only.
        let mut owned: BTreeSet<ContainerId> = BTreeSet::new();
        let bodies = (0..units).map(|id| self.matter.body_of(id));
        for c in bodies.flatten().chain(m.tanks.iter().flatten().copied()) {
            if !owned.insert(c) {
                return None;
            }
        }
        for c in p
            .lines
            .iter()
            .map(|l| l.blank)
            .chain(p.wrecks.values().copied())
        {
            if !owned.insert(c) {
                return None;
            }
        }
        if p.lines.iter().any(|l| p.wrecks.contains_key(&l.slot)) || (!p.active() && !construction)
        {
            return None;
        }
        s.ok().then_some(())
    }
    /// Reclaim works only blanks that lie: checked after production loads,
    /// and against none when the snapshot has no production.
    pub(crate) fn check_blank_work(&self) -> Option<()> {
        let Some(m) = self.mining.as_ref() else {
            return Some(());
        };
        let Some(rc) = m.reclaim.as_ref() else {
            return Some(());
        };
        let wrecks = m.production.as_ref().map_or_else(Vec::new, |p| p.wrecks());
        rc.blank_work()
            .all(|slot| wrecks.iter().any(|(s, _)| *s == slot))
            .then_some(())
    }
}
