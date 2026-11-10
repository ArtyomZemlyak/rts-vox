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
use fx::Fx;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub source: usize,
    pub container: ContainerId,
}

fn is_factory(kind: RangeUnitKind) -> bool {
    matches!(kind, RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3 | RangeUnitKind::KilFactoryT4)
}

fn next_factory(kind: RangeUnitKind) -> Option<RangeUnitKind> {
    match kind {
        RangeUnitKind::KilFactoryT1=>Some(RangeUnitKind::KilFactoryT2),
        RangeUnitKind::KilFactoryT2=>Some(RangeUnitKind::KilFactoryT3),
        RangeUnitKind::KilFactoryT3=>Some(RangeUnitKind::KilFactoryT4), _=>None,
    }
}
fn throughput_ticks(ticks: u32, percent: u32) -> u32 {
    (u64::from(ticks) * 100).div_ceil(u64::from(percent)).max(1) as u32
}
fn line_terms(p: &Production, line: &Line, units: &[RangeUnit], matter: &super::super::matter::Matter,
    rules: &modding::range_economy::Balance) -> (Amount,u32) {
    order_terms(p, usize::from(line.slot), *line.queue.front().expect("nonempty production line"), units, matter, rules)
}
fn order_terms(p: &Production, id: usize, kind: RangeUnitKind, units: &[RangeUnit],
    matter: &super::super::matter::Matter, rules: &modding::range_economy::Balance) -> (Amount,u32) {
    if rules.timed_upgrades && next_factory(units[id].kind)==Some(kind) {
        let full=p.costs[kind.index()];
        let cost=full.saturating_sub(matter.economy.ledger.held(matter.body_container(id)));
        let ticks=(u128::from(cost)*crate::TICKS_PER_SECOND as u128)
            .div_ceil(u128::from(rules.print_per_second)).clamp(1,u128::from(u32::MAX)) as u32;
        (cost,ticks)
    } else {
        let ticks = throughput_ticks(p.build[kind.index()],rules.factory_speed(units[id].kind));
        (p.costs[kind.index()],ticks)
    }
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
    /// Recent launch cues for the renderer; excluded from authoritative saves.
    exits: BTreeMap<usize, (usize, u32)>,
    pub(crate) pipes: BTreeMap<usize, Pipeline>,
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
                // Factory upgrades replace the same slot and never use a
                // product exit mask. Avoid constructing unused geometry.
                if !is_factory(kind) { placement.prepare_product(kind); }
            }
        }
        Self {
            queue: data.queue,
            lines: Vec::new(),
            wrecks: BTreeMap::new(),
            costs: [0; RangeUnitKind::COUNT],
            build: [0; RangeUnitKind::COUNT],
            exits: BTreeMap::new(),
            pipes: BTreeMap::new(),
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
        !self.pipes.is_empty()
            || !self.lines.is_empty()
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
            if k.build_secs == 0 || !self.data.production.as_ref().is_some_and(|p|
                p.producers.iter().any(|p|p.produces.contains(&k.kind))) {
                continue;
            }
            let i = k.kind.index();
            p.costs[i] = matter.kind_matter(k.kind);
            p.build[i] = k.build_secs.saturating_mul(crate::TICKS_PER_SECOND as u32);
            if is_factory(k.kind) && self.data.balance.as_ref().is_some_and(|b|b.timed_upgrades) {
                let rules=self.data.balance.as_ref().unwrap();
                let full=(u128::from(p.costs[i])*crate::TICKS_PER_SECOND as u128)
                    .div_ceil(u128::from(rules.print_per_second)).clamp(1,u128::from(u32::MAX)) as u32;
                p.build[i]=p.build[i].max(full);
            }
        }
    }
}

pub fn starting_crews(layout: &mut modding::range::RangeLayout) {
    if !layout.battle || layout.economy.is_none() {
        return;
    }
    let stores: Vec<(usize, RangeUnitDef, u8)> = layout
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.kind == RangeUnitKind::GrebenStorage)
        .map(|(id, u)| (id, u.clone(), layout.unit_yaw(id)))
        .collect();
    for (_, store, yaw) in stores {
        let mut station = RangeUnit::placed(store.kind, store.allied, store.at, layout.battle);
        station.motion.drive.head = i32::from(yaw) * fx::angle::FULL / 4;
        station.update_pose(station.motion.drive.head);
        // Crews may belong to either side, including a station near the far
        // map edge. Keep the whole starting formation inside the map.
        let xmax = (layout.size[0] as i32 - 1).max(0);
        let ymax = (layout.size[1] as i32 - 1).max(0);
        let zmax = (layout.size[2] as i32 - 1).max(0);
        let xmargin = 100.min(xmax / 2);
        let zmargin = 320.min(zmax / 2);
        let edge = 100.min(zmax / 2);
        let direction = if store.at[0] + 900 < xmax { 1 } else { -1 };
        let crew_x = (store.at[0] + direction * 800).clamp(xmargin, xmax - xmargin);
        let crew_z = store.at[2].clamp(zmargin, zmax - zmargin);
        let near = |u: &RangeUnitDef, kind, reach: i64| {
            u.allied == store.allied
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
        for _ in drones..8 {
            if layout.units.len() >= MAX_RANGE_UNITS {
                break;
            }
            layout.units.push(RangeUnitDef {
                allied: store.allied,
                kind: RangeUnitKind::SkobaDrone,
                at: {
                    let id = layout.units.len();
                    let mut point = station.world_pos(voxel::unit::GREBEN_DRONE_PADS[id % 8]);
                    for axis in 0..3 { point[axis] = point[axis].clamp(1, layout.size[axis] as i32 - 2); }
                    point
                },
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
                allied: store.allied,
                kind: RangeUnitKind::Crawler,
                at: [(crew_x - direction * 200).clamp(xmargin, xmax - xmargin),
                    store.at[1].clamp(0, ymax), (crew_z + 500).clamp(edge, zmax - edge)],
                weapon: None,
            });
        }
    }
}

impl BlastRange {
    /// Explicit sandbox authoring: add initial matter without rebuilding or
    /// healing the running scene. Combat UI must use Produce/construction.
    pub fn place_sandbox_unit(
        &mut self,
        kind: RangeUnitKind,
        allied: bool,
        at: [i32; 3],
        yaw: u8,
    ) -> Result<usize, &'static str> {
        if self.layout.is_none() || self.occupied_unit_slots() >= MAX_RANGE_UNITS {
            return Err("Достигнут лимит юнитов или нет карты");
        }
        // Runtime economic units must keep their existing ledger container IDs.
        // Re-authoring the layout would reconstruct those IDs in a different
        // order on restore, after the economy has already been initialised.
        let economic_dynamic = self.mining.as_ref().is_some_and(|m| m.production.is_some());
        let authored = self.spawned.is_empty() && !economic_dynamic;
        if !authored
            && self
                .mining
                .as_ref()
                .and_then(|m| m.production.as_ref())
                .is_none()
        {
            return Err("Сцена не поддерживает добавление техники");
        }
        Validator::default()
            .runtime(self, kind, at, yaw)
            .map_err(|_| "Место занято или объект выходит за карту")?;
        let probe = RangeUnit::of_kind(kind, at);
        let cost = self.matter.template_matter(&probe);
        let body = self.matter.economy.ledger.create(Kind::Body, cost, cost);
        let cap = self
            .mining
            .as_ref()
            .and_then(|m| m.data.kind(kind))
            .map_or(0, |k| k.tank);
        let tank = (cap > 0).then(|| self.matter.economy.ledger.create(Kind::Hauler, 0, cap));
        let def = RangeUnitDef {
            kind,
            allied,
            at,
            weapon: None,
        };
        if authored {
            let layout = self.layout.as_mut().expect("checked layout");
            while layout.unit_yaws.len() < layout.units.len() {
                let yaw = layout.unit_yaw(layout.unit_yaws.len());
                layout.unit_yaws.push(yaw);
            }
            layout.units.push(def.clone());
            layout.unit_yaws.push(yaw % 4);
        }
        let id = self.append_unit(Spawned { def, body, tank });
        if authored {
            self.spawned.pop();
        } else {
            self.note_construction_spawn();
        }
        let head = i32::from(yaw % 4) * fx::angle::FULL / 4;
        self.units[id].motion.drive.head = head;
        self.battle.actors[id].turret = crate::turret::Traverse::new(head);
        self.units[id].update_pose(head);
        Ok(id)
    }
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
    pub(super) fn queue_greben_crew(&mut self, home: usize, owned_drones: usize) {
        if !self.units.get(home).is_some_and(|u|u.status.alive && u.kind==RangeUnitKind::GrebenStorage) {return;}
        let side=self.allied(home);
        let queued=self.production_queue(home).unwrap_or_default();
        let nearby_crawler=self.units.iter().enumerate().any(|(id,u)|u.kind==RangeUnitKind::Crawler
            && u.status.alive && self.allied(id)==side && [0,2].into_iter().map(|a| {
                let d=i64::from(u.at[a])-i64::from(self.units[home].at[a]); d*d
            }).sum::<i64>()<=i64::from(self.supply_radius().max(self.economic_rules().power_link_reach)).pow(2));
        // Mining comes first: a station without income must not spend its
        // whole initial reserve on builders and then starve its own crawler.
        if !nearby_crawler && !queued.contains(&RangeUnitKind::Crawler) {
            self.enqueue_product(home,RangeUnitKind::Crawler,side);
        }
        let target=usize::from(self.economic_rules().greben_crew_drones);
        let count=owned_drones+queued.iter().filter(|&&k|k==RangeUnitKind::SkobaDrone).count();
        for _ in count..target { self.enqueue_product(home,RangeUnitKind::SkobaDrone,side); }
    }
    /// Nominal power of a product, including a selectable order not yet queued.
    fn product_power(&self,actor:usize,kind:Option<RangeUnitKind>)->u64 {
        let Some(unit)=self.units.get(actor) else {return 0;};
        let rules=self.economic_rules();
        if kind.is_some() && rules.timed_upgrades && next_factory(unit.kind)==kind {return rules.print_power;}
        match unit.kind {
            RangeUnitKind::KilFactoryT1=>rules.factory_power[0],RangeUnitKind::KilFactoryT2=>rules.factory_power[1],
            RangeUnitKind::KilFactoryT3=>rules.factory_power[2],RangeUnitKind::KilFactoryT4=>rules.factory_power[3],
            RangeUnitKind::GrebenStorage if kind==Some(RangeUnitKind::Crawler)=>rules.factory_power[0],
            RangeUnitKind::GrebenStorage=>rules.drone_power,_=>0,
        }
    }
    pub fn factory_upgrading(&self, id: usize) -> bool {
        self.economic_rules().timed_upgrades && self.units.get(id).is_some_and(|u|
            self.production_queue(id).and_then(|q| q.first().copied())==next_factory(u.kind)
                && next_factory(u.kind).is_some())
    }
    /// Nominal demand of an active line; zero for a historical unpowered preset.
    pub fn production_power(&self, actor: usize) -> u64 {
        self.product_power(actor,self.production_queue(actor).and_then(|q|q.first().copied()))
    }
    pub(super) fn production_power_requests(&self) -> Vec<u64> {
        let mut requests = vec![0; self.units.len()];
        let Some(m) = self.mining.as_ref() else { return requests; };
        let Some(p) = m.production.as_ref() else { return requests; };
        for line in &p.lines {
            let id=usize::from(line.slot);
            if !self.units[id].status.alive || self.unit_under_construction(id) { continue; }
            if line.queue.is_empty() { continue; }
            let paid=self.matter.economy.ledger.held(line.blank);
            let (price,ticks)=line_terms(p,line,&self.units,&self.matter,self.economic_rules());
            let ticks=ticks.max(1);
            if paid >= price && line.ticks >= ticks { continue; }
            let store_ready=m.haul().is_some_and(|h| self.matter.economy.ledger.held(h.store(self.allied(id)))>0)
                && (!is_factory(self.units[id].kind) || self.supply_radius()==0 || self.factory_supply_connected(id));
            let commander_ready=self.units.iter().enumerate().any(|(other,u)| {
                if !u.status.alive || u.kind!=RangeUnitKind::Commander || self.allied(other)!=self.allied(id)
                    || self.tank_held(other).unwrap_or(0)==0 { return false; }
                let a=u.motion.pos; let b=self.units[id].motion.pos;
                let dx=i64::from(a[0].to_int_trunc())-i64::from(b[0].to_int_trunc());
                let dz=i64::from(a[2].to_int_trunc())-i64::from(b[2].to_int_trunc());
                dx*dx+dz*dz<=400_i64.pow(2)
            });
            let needed=(u128::from(price)*u128::from((line.ticks+1).min(ticks))).div_ceil(u128::from(ticks)) as u64;
            if paid>=needed || store_ready || commander_ready { requests[id]=self.production_power(id); }
        }
        requests
    }
    /// Elapsed and required build ticks, paid and required matter for the front order.
    pub fn production_progress(&self, id: usize) -> Option<(u32, u32, Amount, Amount)> {
        let p = self.mining.as_ref()?.production.as_ref()?;
        let line = p.line(id)?;
        line.queue.front()?;
        let (price,ticks)=line_terms(p,line,&self.units,&self.matter,self.economic_rules());
        Some((line.ticks, ticks, self.matter.economy.ledger.held(line.blank), price))
    }
    /// A user-assigned destination, kept with the factory's saved order.
    pub fn factory_rally(&self, id: usize) -> Option<[i32; 2]> {
        if !self.units.get(id).is_some_and(|u| is_factory(u.kind) && u.status.alive) { return None; }
        match self.battle.actors.get(id)?.order { battle::Order::Rally(p) => Some(p), _ => None }
    }
    pub fn set_factory_rally(&mut self, id: usize, point: [i32; 2]) -> bool {
        self.battle_order(id, battle::Order::Rally(point), None)
    }
    /// Most recent completed product of a factory and its simulation tick.
    pub fn production_exit(&self, id: usize) -> Option<(usize, u32)> {
        self.mining.as_ref()?.production.as_ref()?.exits.get(&id).copied()
    }
    /// An automatically launched product uses an ordinary pilot order while
    /// in the gate. The marker is saved with the actor, so egress survives a
    /// save/load without relying on the renderer's transient exit cue.
    pub fn factory_exiting_unit(&self, factory: usize) -> Option<usize> {
        let f = self.units.get(factory)?;
        if !is_factory(f.kind) { return None; }
        let p = f.motion.pos.map(Fx::to_int_trunc);
        self.units.iter().enumerate().skip(self.authored_units()).find_map(|(id, u)| {
            if !u.status.alive || self.allied(id) != self.allied(factory) { return None; }
            match self.battle.actors.get(id)?.order {
                battle::Order::Pilot { throttle: 1, turn: 0, aim, fire: false }
                    if aim == [p[0], 0, p[2]] => Some(id),
                _ => None,
            }
        })
    }
    pub(crate) fn factory_launch_owner(&self, id: usize) -> Option<usize> {
        if id < self.authored_units() { return None; }
        self.factory_launch_for_order(id, self.battle.actors.get(id)?.order)
    }
    pub(crate) fn factory_launch_for_order(&self, id: usize, order: battle::Order) -> Option<usize> {
        if id < self.authored_units() { return None; }
        let battle::Order::Pilot { throttle: 1, turn: 0, aim, fire: false } = order else { return None; };
        if aim[1] != 0 { return None; }
        self.units.iter().enumerate().find_map(|(factory, f)| {
            let p = f.motion.pos.map(Fx::to_int_trunc);
            (is_factory(f.kind) && self.allied(id) == self.allied(factory)
                && p[0] == aim[0] && p[2] == aim[2]).then_some(factory)
        })
    }
    pub(crate) fn finish_factory_launches(&mut self) {
        let done: Vec<_> = (0..self.units.len()).filter_map(|id| {
            let factory = self.factory_launch_owner(id)?;
            let (a, b) = self.units[id].bounds();
            let (c, d) = self.units[factory].bounds();
            let quarter = fx::angle::FULL / 4;
            let yaw = self.units[factory].motion.drive.head.rem_euclid(fx::angle::FULL) / quarter;
            // Keep driving straight until the entire hull has cleared the
            // gate corridor, not merely the factory's bounding box. Nearby
            // units still block the drive; only the originating factory is
            // omitted from the collision set during this short departure.
            let clear = match yaw {
                0 => c[0] - b[0] >= 100 || a[2] >= d[2] || b[2] <= c[2],
                1 => c[2] - b[2] >= 100 || a[0] >= d[0] || b[0] <= c[0],
                2 => a[0] - d[0] >= 100 || a[2] >= d[2] || b[2] <= c[2],
                _ => a[2] - d[2] >= 100 || a[0] >= d[0] || b[0] <= c[0],
            };
            clear.then_some((factory, id))
        }).collect();
        for (factory, id) in done {
            let u = &self.units[id];
            let head = u.motion.drive.head;
            let p = u.motion.pos.map(Fx::to_int_trunc);
            let goal = self.factory_rally(factory).unwrap_or_else(|| [
                p[0] + fx::angle::cos(head).mul(Fx::from_int(120)).to_int_trunc(),
                p[2] + fx::angle::sin(head).mul(Fx::from_int(120)).to_int_trunc()]);
            // Leave room for the hull at the map edge; an unreachable point
            // makes the just-produced unit stop in front of the gate.
            let mx = 80.min((self.size[0] as i32 - 1) / 2);
            let mz = 80.min((self.size[2] as i32 - 1) / 2);
            let goal = if self.factory_rally(factory).is_some() { goal } else {
                [goal[0].clamp(mx, self.size[0] as i32 - 1 - mx),
                 goal[1].clamp(mz, self.size[2] as i32 - 1 - mz)]
            };
            self.units[id].motion.queue_move(&self.world, goal);
            self.battle.actors[id].order = battle::Order::Move;
            let tick = self.tick;
            self.production_mut().exits.insert(factory, (id, tick));
        }
    }

    /// Closest completed allied storage, distance and one-off connection cost.
    pub fn factory_supply_quote(&self, id: usize) -> Option<(usize, u32, Amount)> {
        if !self.units.get(id).is_some_and(|u| is_factory(u.kind)) { return None; }
        let pos = self.units[id].motion.pos.map(|v| v.to_int_trunc());
        self.supply_quote_at(pos, self.allied(id))
    }
    pub fn supply_radius(&self) -> u32 {
        self.mining.as_ref().and_then(|m| m.data().production.as_ref()).map_or(0, |p| p.supply_radius)
    }
    pub fn supply_quote_at(&self, pos: [i32; 3], allied: bool) -> Option<(usize, u32, Amount)> {
        let p = self.mining.as_ref()?.data().production.as_ref()?;
        if p.supply_radius == 0 { return None; }
        let (d, source) = self.units.iter().enumerate().filter(|(other, u)| {
            u.status.alive && self.allied(*other) == allied
                && matches!(u.kind, RangeUnitKind::GrebenStorage | RangeUnitKind::Base)
                && !self.unit_under_construction(*other)
        }).filter_map(|(other, u)| {
            let at = u.motion.pos.map(|v| v.to_int_trunc());
            let dx = i64::from(at[0]) - i64::from(pos[0]);
            let dz = i64::from(at[2]) - i64::from(pos[2]);
            let d = (dx * dx + dz * dz) as u64;
            (d <= u64::from(p.supply_radius).pow(2)).then_some((d, other))
        }).min()?;
        let distance = d.isqrt() as u32;
        Some((source, distance, u64::from(distance).saturating_mul(p.pipeline_cost_per_cell)))
    }
    pub fn factory_supply_connected(&self, id: usize) -> bool {
        let Some((source, _, cost)) = self.factory_supply_quote(id) else { return false; };
        self.mining.as_ref().and_then(|m| m.production.as_ref()).and_then(|p| p.pipes.get(&id))
            .is_some_and(|pipe| pipe.source == source && self.matter.economy.ledger.held(pipe.container) >= cost)
    }
    pub(crate) fn has_pipes(&self) -> bool {
        self.mining.as_ref().and_then(|m| m.production.as_ref()).is_some_and(|p| !p.pipes.is_empty())
    }
    pub(crate) fn save_pipes(&self, b: &mut Blob) {
        let pipes = &self.mining.as_ref().unwrap().production.as_ref().unwrap().pipes;
        b.u32(pipes.len() as u32);
        for (&id, pipe) in pipes { b.u16(id as u16); b.u16(pipe.source as u16); b.u32(pipe.container); }
    }
    pub(crate) fn load_pipes(&mut self, entries: &[(u16, u16, u32)]) -> Option<()> {
        let mut containers = BTreeSet::new();
        let sides: Vec<_> = (0..self.units.len()).map(|id| self.allied(id)).collect();
        let p = self.mining.as_mut()?.production.as_mut()?;
        for &(id, source, container) in entries {
            let (id, source) = (usize::from(id), usize::from(source));
            let factory = self.units.get(id)?;
            let storage = self.units.get(source)?;
            if !is_factory(factory.kind) || !matches!(storage.kind, RangeUnitKind::GrebenStorage | RangeUnitKind::Base)
                || sides[id] != sides[source]
                || !containers.insert(container)
                || self.matter.economy.ledger.try_get(container).is_none_or(|c| !c.alive || c.kind != Kind::Pipeline)
            { return None; }
            p.pipes.insert(id, Pipeline { source, container });
        }
        Some(())
    }
    /// Cargo left at a dead producer or shot-down drone, before reclaim finds it.
    pub fn wreck_cargo(&self, id: usize) -> Option<Amount> {
        let p = self.mining.as_ref()?.production.as_ref()?;
        let container = p.wrecks.get(&(id as u16))?;
        Some(self.matter.economy.ledger.held(*container))
    }
    /// Paid matter, full-power working ticks and power for a selectable order.
    pub fn production_quote(&self, actor: usize, kind: RangeUnitKind) -> Option<(Amount,u32,u64)> {
        let m=self.mining.as_ref()?; let p=m.production.as_ref()?;
        let unit=self.units.get(actor)?;
        if !m.data().produces(unit.kind,kind) { return None; }
        let (cost,ticks)=order_terms(p,actor,kind,&self.units,&self.matter,self.economic_rules());
        let power=self.product_power(actor,Some(kind));
        Some((cost,ticks,power))
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
        self.matter.push_body(s.body, &u);
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
        if !self
            .units
            .get(id)
            .is_some_and(|u| u.status.alive && self.allied(id))
            || self.unit_under_construction(id)
        {
            return false;
        }
        let next = match self.units.get(id).map(|u| u.kind) {
            Some(RangeUnitKind::KilFactoryT1) => RangeUnitKind::KilFactoryT2,
            Some(RangeUnitKind::KilFactoryT2) => RangeUnitKind::KilFactoryT3,
            Some(RangeUnitKind::KilFactoryT3) => RangeUnitKind::KilFactoryT4,
            _ => return false,
        };
        if self.economic_rules().timed_upgrades {
            if self.production_queue(id).is_some_and(|q| !q.is_empty()) { return false; }
            return self.enqueue_product(id,next,true)==battle::commands::Outcome::Applied;
        }
        let at = self.units[id].at;
        let motion = self.units[id].motion.clone();
        let replacement = RangeUnit::of_kind(next, at);
        let yaw = (motion.drive.head.rem_euclid(fx::angle::FULL) / (fx::angle::FULL / 4)) as u8;
        if Validator::default()
            .runtime_except(self, next, at, yaw, Some(id))
            .is_err()
        {
            return false;
        }
        let price = self.matter.template_matter(&replacement);
        let body = self.matter.body_container(id);
        let held = self.matter.economy.ledger.held(body);
        let Some(needed) = price.checked_sub(held) else {
            return false;
        };
        let Some(store) = self
            .mining
            .as_ref()
            .and_then(|m| m.haul())
            .map(|h| h.store(true))
        else {
            return false;
        };
        if self.matter.economy.ledger.held(store) < needed {
            return false;
        }
        self.matter.economy.ledger.lower_cap(body, price);
        assert_eq!(
            self.matter.economy.ledger.transfer(store, body, needed),
            needed
        );
        self.install_factory_upgrade(id, next, replacement);
        true
    }
    fn install_factory_upgrade(&mut self, id: usize, next: RangeUnitKind, mut replacement: RangeUnit) {
        let motion = self.units[id].motion.clone();
        let head = motion.drive.head;
        replacement.motion = motion;
        replacement.update_pose(head);
        self.matter.replace_body_tables(id, &replacement);
        self.units[id] = replacement;
        if let Some(def) = self
            .layout
            .as_mut()
            .and_then(|layout| layout.units.get_mut(id))
        {
            def.kind = next;
        }
        let authored = self.layout.as_ref().map_or(0, |l| l.units.len());
        if let Some(slot) = id
            .checked_sub(authored)
            .and_then(|slot| self.spawned.get_mut(slot))
        {
            slot.def.kind = next;
        }
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
            || self.unit_under_construction(actor)
            || self.allied(actor) != side
            || !self.units.get(actor).is_some_and(|u| u.status.alive)
        {
            return Outcome::Rejected;
        }
        if self.factory_upgrading(actor) { return Outcome::Rejected; }
        let producer = self.units[actor].kind;
        if self.economic_rules().timed_upgrades && is_factory(producer) && is_factory(kind) && (next_factory(producer)!=Some(kind)
            || self.production_queue(actor).is_some_and(|q| !q.is_empty())) { return Outcome::Rejected; }
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

    /// AI may reprioritise an unfinished order only when all paid matter
    /// fits back into the side's store and/or the nearby commander's tank.
    pub(crate) fn cancel_product_preserving_matter(
        &mut self, actor: usize, commander: usize, side: bool,
    ) -> bool {
        if !self.production_open() || self.allied(actor) != side || self.allied(commander) != side
            || !self.units.get(actor).is_some_and(|u| u.status.alive)
            || !self.units.get(commander).is_some_and(|u| u.kind == RangeUnitKind::Commander && u.status.alive)
        { return false; }
        let a = self.units[actor].motion.pos;
        let b = self.units[commander].motion.pos;
        let dx = i64::from(a[0].to_int_trunc() - b[0].to_int_trunc());
        let dz = i64::from(a[2].to_int_trunc() - b[2].to_int_trunc());
        let Some(m) = self.mining.as_mut() else { return false; };
        let (Some(p), Some(h), Some(tank)) = (m.production.as_mut(), m.haul.as_ref(), m.tanks.get(commander).copied().flatten()) else { return false; };
        let Some(line) = p.line(actor) else { return false; };
        if line.queue.len() != 1 { return false; }
        let blank = line.blank;
        let store = h.store(side);
        let ledger = &mut self.matter.economy.ledger;
        let near = dx * dx + dz * dz <= 400_i64.pow(2);
        let tank_room = if near { ledger.room(tank) } else { 0 };
        if ledger.held(blank) > ledger.room(store).saturating_add(tank_room) { return false; }
        if near {
            let returned = ledger.transfer(blank, tank, ledger.held(blank));
            p.refunded += returned;
        }
        self.cancel_product(actor, side).accepted()
    }

    /// Production at the end of the tick, after every other phase: dead
    /// producers' lines become wrecks, the lines pay and stand in slot
    /// order, and finished units join the roster.
    pub(crate) fn production_phase(&mut self) {
        if self.economic_power.as_ref().is_none_or(|(tick, _)| *tick != self.tick) {
            self.economic_power = Some((self.tick, self.construction_power_networks()));
        }
        let powered: Vec<_> = (0..self.units.len()).map(|id| self.economic_power_tick(id)).collect();
        let open = self.production_open();
        let sides: Vec<bool> = (0..self.units.len()).map(|id| self.allied(id)).collect();
        let supply = self.mining.as_ref().and_then(|m| m.data().production.as_ref())
            .map(|p| (p.supply_radius, p.pipeline_cost_per_cell)).unwrap_or((0, 0));
        let mut sources: BTreeMap<(bool, i32, i32), Vec<(usize, [i32; 3])>> = BTreeMap::new();
        if supply.0 > 0 {
            let cell = supply.0 as i32;
            for (id, u) in self.units.iter().enumerate() {
                if u.status.alive && matches!(u.kind, RangeUnitKind::GrebenStorage | RangeUnitKind::Base)
                    && !self.unit_under_construction(id) {
                    sources.entry((sides[id], u.at[0].div_euclid(cell), u.at[2].div_euclid(cell)))
                        .or_default().push((id, u.at));
                }
            }
        }
        let quotes: Vec<_> = if supply.0 == 0 { Vec::new() } else {
            self.units.iter().enumerate().filter(|(id, u)| u.status.alive && is_factory(u.kind)
                && !self.unit_under_construction(*id)).filter_map(|(id, u)| {
                let cell = supply.0 as i32;
                let x = u.at[0].div_euclid(cell);
                let z = u.at[2].div_euclid(cell);
                let (d, source) = (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (dx, dz)))
                    .filter_map(|(dx, dz)| sources.get(&(sides[id], x + dx, z + dz)))
                    .flatten().filter_map(|(source, at)| {
                        let dx = i64::from(at[0]) - i64::from(u.at[0]);
                        let dz = i64::from(at[2]) - i64::from(u.at[2]);
                        let d = (dx * dx + dz * dz) as u64;
                        (d <= u64::from(supply.0).pow(2)).then_some((d, *source))
                    }).min()?;
                Some((id, source, d.isqrt().saturating_mul(supply.1)))
            }).collect()
        };
        let terms: BTreeMap<_,_>=self.mining.as_ref().and_then(|m| m.production.as_ref())
            .map(|p| p.lines.iter().map(|line| (line.slot,line_terms(p,line,&self.units,&self.matter,self.economic_rules()))).collect())
            .unwrap_or_default();
        let Some(m) = self.mining.as_mut() else {
            return;
        };
        let Mining {
            production,
            haul,
            reclaim,
            tanks,
            ..
        } = m;
        let Some(p) = production.as_mut() else {
            return;
        };
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
        // Connections are paid once on completion, and are abstract ledger
        // containers, not simulated terrain or per-tick delivery entities.
        for &(id, source, cost) in &quotes {
            let store = h.store(sides[id]);
            let existing = p.pipes.get(&id).copied();
            let paid = existing.map_or(0, |pipe| ledger.held(pipe.container));
            let missing = cost.saturating_sub(paid);
            if ledger.held(store) < missing { continue; }
            let pipe = existing.unwrap_or_else(|| Pipeline {
                source, container: ledger.create(Kind::Pipeline, 0, Amount::MAX)
            });
            assert_eq!(ledger.transfer(store, pipe.container, missing), missing);
            p.pipes.insert(id, Pipeline { source, ..pipe });
        }
        if p.lines.is_empty() { return; }
        let mut finished = Vec::new();
        for line in &mut p.lines {
            let front = *line.queue.front().expect("a line has an order");
            let (cost, build) = terms[&line.slot];
            let paid = ledger.held(line.blank);
            // A completed product can wait for an obstructed gate. It no
            // longer requests work power, so retry deployment independently
            // of the power gate or it can remain finished forever.
            if paid >= cost && line.ticks >= build {
                finished.push((usize::from(line.slot), front, line.blank));
                continue;
            }
            if !powered[usize::from(line.slot)] { p.stalled += 1; continue; }
            let next_tick = line.ticks.saturating_add(1).min(build);
            let threshold = (u128::from(cost)*u128::from(next_tick)).div_ceil(u128::from(build.max(1))) as u64;
            let want = threshold.saturating_sub(paid);
            if want > 0 {
                let store = h.store(sides[usize::from(line.slot)]);
                let connected = supply.0 == 0 || !is_factory(self.units[usize::from(line.slot)].kind)
                    || quotes.binary_search_by_key(&usize::from(line.slot), |(id, _, _)| *id).ok().and_then(|index| quotes.get(index)).is_some_and(|(_, source, cost)| {
                        p.pipes.get(&usize::from(line.slot)).is_some_and(|pipe| pipe.source == *source && ledger.held(pipe.container) >= *cost)
                    });
                let mut moved = if connected { ledger.transfer(store, line.blank, want) } else { 0 };
                if moved < want {
                    let factory = usize::from(line.slot);
                    let from = self.units[factory].motion.pos;
                    // Nearby commanders can feed their own universal stock
                    // directly into an allied factory. No matter is created.
                    let source = self.units.iter().enumerate().filter_map(|(id, u)| {
                        if u.kind != RangeUnitKind::Commander || !u.status.alive || sides[id] != sides[factory] {
                            return None;
                        }
                        let tank = tanks.get(id).copied().flatten()?;
                        let p = u.motion.pos;
                        let dx = i64::from(p[0].to_int_trunc() - from[0].to_int_trunc());
                        let dz = i64::from(p[2].to_int_trunc() - from[2].to_int_trunc());
                        let d = dx * dx + dz * dz;
                        (d <= 400_i64.pow(2) && ledger.held(tank) > 0).then_some((d, id, tank))
                    }).min_by_key(|&(d, id, _)| (d, id));
                    if let Some((_, _, tank)) = source {
                        moved += ledger.transfer(tank, line.blank, want - moved);
                    }
                }
                p.paid += moved;
                if moved < want {
                    p.stalled += 1;
                }
            }
            if ledger.held(line.blank) >= threshold { line.ticks = next_tick; }
            if ledger.held(line.blank) == cost && line.ticks >= build {
                finished.push((usize::from(line.slot), front, line.blank));
            }
        }
        for (slot, kind, blank) in finished {
            let allied = sides[slot];
            if self.economic_rules().timed_upgrades && next_factory(self.units[slot].kind)==Some(kind) {
                let at=self.units[slot].at;
                let yaw=(self.units[slot].motion.drive.head.rem_euclid(fx::angle::FULL)/(fx::angle::FULL/4)) as u8;
                if Validator::default().runtime_except(self,kind,at,yaw,Some(slot)).is_err() {
                    self.production_mut().blocked+=1; continue;
                }
                let replacement=RangeUnit::of_kind(kind,at);
                let body=self.matter.body_container(slot);
                let amount=self.matter.economy.ledger.held(blank);
                let full=self.matter.template_matter(&replacement);
                self.matter.economy.ledger.lower_cap(body,full);
                assert_eq!(self.matter.economy.ledger.transfer(blank,body,amount),amount);
                self.install_factory_upgrade(slot,kind,replacement);
                let p=self.production_mut();
                let index=p.lines.binary_search_by_key(&slot,|l| usize::from(l.slot)).unwrap();
                p.lines.remove(index);
                assert!(self.matter.economy.ledger.retire_empty(blank));
                continue;
            }
            if self.occupied_unit_slots() >= MAX_RANGE_UNITS {
                self.production_mut().capped += 1;
                continue;
            }
            let Some(at) = self.free_spot(slot, kind, allied) else {
                self.production_mut().blocked += 1;
                continue;
            };
            let factory = is_factory(self.units[slot].kind) && !kind.emplacement();
            // Larger products cannot pass through a narrow factory opening;
            // keep their existing safe exterior deployment rather than move
            // an oversized body through the walls.
            let physical_exit = if factory {
                let f = &self.units[slot];
                let quarter = fx::angle::FULL / 4;
                let yaw = (f.motion.drive.head + fx::angle::FULL / 2).rem_euclid(fx::angle::FULL) / quarter;
                let mut probe = RangeUnit::placed(kind, allied, at, self.battle_mode());
                probe.motion.drive.head = yaw * quarter;
                probe.update_pose(probe.motion.drive.head);
                let (lo, hi) = probe.bounds();
                let width = if yaw % 2 == 0 { hi[2] - lo[2] } else { hi[0] - lo[0] };
                let height = hi[1] - lo[1];
                let (gate_width, gate_height) = match f.kind {
                    RangeUnitKind::KilFactoryT1 => (80, 56), RangeUnitKind::KilFactoryT2 => (96, 80),
                    RangeUnitKind::KilFactoryT3 => (144, 112), _ => (224, 176),
                };
                width <= gate_width && height <= gate_height
            } else { false };
            let spawn_at = if physical_exit {
                let f = &self.units[slot];
                let pos = f.motion.pos.map(Fx::to_int_trunc);
                [pos[0] - f.pose.x.x.mul(Fx::from_int(160)).to_int_trunc(), pos[1],
                 pos[2] - f.pose.x.z.mul(Fx::from_int(160)).to_int_trunc()]
            } else { at };
            let product = self.spawn(kind, allied, spawn_at, blank);
            if matches!(self.units[slot].kind, RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3 | RangeUnitKind::KilFactoryT4) {
                let quarter = fx::angle::FULL / 4;
                let head = (self.units[slot].motion.drive.head + fx::angle::FULL / 2).rem_euclid(fx::angle::FULL) / quarter * quarter;
                self.units[product].motion.drive.head = head;
                self.battle.actors[product].turret = crate::turret::Traverse::new(head);
                self.units[product].update_pose(head);
                if physical_exit {
                    self.units[product].motion.enabled = true;
                    let p = self.units[slot].motion.pos.map(Fx::to_int_trunc);
                    self.battle.actors[product].order = battle::Order::Pilot {
                        throttle: 1, turn: 0, aim: [p[0], 0, p[2]], fire: false,
                    };
                } else if factory {
                    let p = self.units[product].motion.pos.map(Fx::to_int_trunc);
                    let goal = self.factory_rally(slot).unwrap_or_else(|| [
                        p[0] + fx::angle::cos(head).mul(Fx::from_int(120)).to_int_trunc(),
                        p[2] + fx::angle::sin(head).mul(Fx::from_int(120)).to_int_trunc()]);
                    let goal = [goal[0].clamp(1, self.size[0] as i32 - 2), goal[1].clamp(1, self.size[2] as i32 - 2)];
                    self.battle_order(product, battle::Order::Move, Some(goal));
                }
            }
            if self.units[slot].kind==RangeUnitKind::GrebenStorage && kind==RangeUnitKind::SkobaDrone {
                self.receive_produced_drone(product,slot);
            }
            let tick = self.tick;
            let p = self.production_mut();
            p.exits.insert(slot, (product, tick));
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
    pub(crate) fn free_spot(&self, producer: usize, kind: RangeUnitKind, allied: bool) -> Option<[i32; 3]> {
        let battle = self.layout.as_ref().is_some_and(|l| l.battle);
        let p = &self.units[producer];
        let c = p.motion.pos.map(|v| v.to_int_trunc());
        let factory = matches!(p.kind, RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3 | RangeUnitKind::KilFactoryT4);
        if factory && self.factory_exiting_unit(producer).is_some() { return None; }
        if factory {
            // Do not place a new product on an interrupted or manually
            // controlled unit still physically inside the gate.
            let (width, height) = match p.kind {
                RangeUnitKind::KilFactoryT1 => (80, 56), RangeUnitKind::KilFactoryT2 => (96, 80),
                RangeUnitKind::KilFactoryT3 => (144, 112), _ => (224, 176),
            };
            let direction = p.motion.drive.head.rem_euclid(fx::angle::FULL) / (fx::angle::FULL / 4);
            let (mut lo, mut hi) = ([c[0] - width / 2, c[1], c[2] - width / 2],
                [c[0] + width / 2, c[1] + height + 16, c[2] + width / 2]);
            match direction {
                0 => { lo[0] = c[0] - 320; hi[0] = c[0] - 110; }
                1 => { lo[2] = c[2] - 320; hi[2] = c[2] - 110; }
                2 => { lo[0] = c[0] + 110; hi[0] = c[0] + 320; }
                _ => { lo[2] = c[2] + 110; hi[2] = c[2] + 320; }
            }
            if self.units.iter().enumerate().any(|(other, unit)| {
                if other == producer { return false; }
                let (a, b) = unit.bounds();
                (0..3).all(|axis| a[axis] < hi[axis] && lo[axis] < b[axis])
            }) { return None; }
        }
        let yaw = if factory {
            ((p.motion.drive.head + fx::angle::FULL / 2).rem_euclid(fx::angle::FULL) / (fx::angle::FULL / 4)) as u8
        } else if battle && !allied { 2 } else { 0 };
        let mut probe = RangeUnit::placed(kind, allied, c, battle);
        probe.motion.drive.head = i32::from(yaw) * (fx::angle::FULL / 4);
        probe.update_pose(probe.motion.drive.head);
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
        let fx = p.pose.x.x.raw();
        let fz = p.pose.x.z.raw();
        if factory {
            grid.retain(|g| {
                let [x, z] = g.2;
                if fx.abs() >= fz.abs() { z == 0 && i64::from(x) * fx < 0 } else { x == 0 && i64::from(z) * fz < 0 }
            });
        }
        grid.sort_unstable_by_key(|g| {
            let [x, z] = g.2;
            let gate_lane = if fx.abs() >= fz.abs() { z == 0 && i64::from(x) * fx < 0 } else { x == 0 && i64::from(z) * fz < 0 };
            (if factory && gate_lane { 0 } else { 1 }, g.0, g.1)
        });
        let boxes: Vec<([i32; 3], [i32; 3])> = self
            .units
            .iter()
            .chain((!self.battle_mode()).then_some(&self.gun))
            .map(|u| u.bounds())
            .chain(self.reserved_construction_bounds())
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
            if factory {
                // A free destination beyond a blocker is not a free exit.
                // Reserve the straight gate corridor through to the whole hull.
                let mut ca = a;
                let mut cb = b;
                // Hull bounds include the contact plane; do not treat the
                // ground supporting the tracks as an obstacle in the lane.
                ca[1] = ca[1].max(c[1] + 1);
                if fx.abs() >= fz.abs() {
                    if fx > 0 { cb[0] = plo[0]; } else { ca[0] = phi[0]; }
                } else if fz > 0 { cb[2] = plo[2]; } else { ca[2] = phi[2]; }
                if (0..3).all(|axis| ca[axis] < cb[axis]) {
                    for (index, (olo, ohi)) in boxes.iter().enumerate() {
                        if index != producer && (0..3).all(|k| ca[k] < ohi[k] && olo[k] < cb[k]) {
                            continue 'spots;
                        }
                    }
                    if self.world.any_solid_box(ca.map(|v| v as u32), cb.map(|v| (v - 1) as u32)) {
                        continue 'spots;
                    }
                }
            }
            let at = [c[0] + off[0], c[1], c[2] + off[1]];
            if placement.product_intersects(kind, yaw, &self.world, at, (a, b)) {
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
            if data.kind(kind).is_none()
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
            if id >= units {return None;}
            let rules=data.balance.as_ref().unwrap_or_else(|| modding::range_economy::Balance::legacy());
            let upgrading=rules.timed_upgrades && next_factory(kinds[id])==Some(queue[0]);
            let max_ticks=if upgrading {p.build[front]} else {throughput_ticks(p.build[front],rules.factory_speed(kinds[id]))};
            if p.lines.last().is_some_and(|l| l.slot >= slot)
                || !queue.iter().all(|k| data.produces(kinds[id], *k))
                || !blank_ok(blank)
                || ledger.held(blank) > p.costs[front]
                || ticks > max_ticks
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
        for pipe in p.pipes.values() {
            if !owned.insert(pipe.container) { return None; }
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
