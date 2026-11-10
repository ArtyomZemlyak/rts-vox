//! Bounded player order plans. A blueprint reserves space, never material.
use super::{
    battle::{
        commands::{Command, Outcome},
        Order, Phase,
    },
    BlastRange, RangeUnitKind,
};
use fx::blob::{Blob, Scan};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[cfg(test)]
#[path = "range_build_plans_tests.rs"]
mod tests;

pub(crate) const MAX_ORDERS: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedOrder {
    pub order: Order,
    pub destination: Option<[i32; 2]>,
}
#[derive(Default)]
pub(crate) struct Plans {
    pub queues: BTreeMap<u16, VecDeque<PlannedOrder>>,
    pub cancelled: BTreeSet<usize>,
}
impl Plans {
    pub fn active(&self) -> bool {
        !self.queues.is_empty() || !self.cancelled.is_empty()
    }
    pub fn save(&self, b: &mut Blob) {
        b.u32(self.cancelled.len() as u32);
        for &id in &self.cancelled {
            b.u16(id as u16);
        }
        b.u32(self.queues.len() as u32);
        for (&id, queue) in &self.queues {
            b.u16(id);
            b.u8(queue.len() as u8);
            for &entry in queue {
                save_order(b, entry);
            }
        }
    }
    pub fn load(s: &mut Scan) -> Option<Self> {
        let mut out = Self::default();
        let n = s.u32() as usize;
        if n > 512 || n > s.left() / 2 {
            return None;
        }
        for _ in 0..n {
            if !out.cancelled.insert(usize::from(s.u16())) {
                return None;
            }
        }
        let n = s.u32() as usize;
        if n > modding::range::MAX_RANGE_UNITS || n > s.left() / 4 {
            return None;
        }
        for _ in 0..n {
            let id = s.u16();
            let count = usize::from(s.u8());
            if count == 0 || count > MAX_ORDERS || count > s.left() / 2 {
                return None;
            }
            let mut queue = VecDeque::new();
            for _ in 0..count {
                queue.push_back(load_order(s)?);
            }
            if out.queues.insert(id, queue).is_some() {
                return None;
            }
        }
        s.ok().then_some(out)
    }
}
pub(crate) fn save_order(b: &mut Blob, p: PlannedOrder) {
    match p.order {
        Order::Move => b.u8(0),
        Order::Build(id) => {
            b.u8(1);
            b.usize(id);
        }
        Order::Attack(id) => {
            b.u8(2);
            b.usize(id);
        }
        Order::Repair(id) => {
            b.u8(3);
            b.usize(id);
        }
        Order::Dig(at) | Order::Reclaim(at) | Order::Ground(at) => {
            b.u8(match p.order {
                Order::Dig(_) => 4,
                Order::Reclaim(_) => 5,
                _ => 6,
            });
            for v in at {
                b.i32(v);
            }
        }
        Order::Hold => b.u8(7),
        Order::Auto => b.u8(8),
        Order::Rally(at) => {
            b.u8(9);
            for v in at {
                b.i32(v);
            }
        }
        Order::Support(id) => {
            b.u8(10);
            b.usize(id);
        }
        Order::Pilot {
            throttle,
            turn,
            aim,
            fire,
        } => {
            b.u8(11);
            b.u8(throttle.wrapping_add(1) as u8);
            b.u8(turn.wrapping_add(1) as u8);
            for v in aim {
                b.i32(v);
            }
            b.bool(fire);
        }
    }
    b.bool(p.destination.is_some());
    if let Some(at) = p.destination {
        for v in at {
            b.i32(v);
        }
    }
}
pub(crate) fn load_order(s: &mut Scan) -> Option<PlannedOrder> {
    let order = match s.u8() {
        0 => Order::Move,
        1 => Order::Build(s.usize()),
        2 => Order::Attack(s.usize()),
        3 => Order::Repair(s.usize()),
        4 => Order::Dig(std::array::from_fn(|_| s.i32())),
        5 => Order::Reclaim(std::array::from_fn(|_| s.i32())),
        6 => Order::Ground(std::array::from_fn(|_| s.i32())),
        7 => Order::Hold,
        8 => Order::Auto,
        9 => Order::Rally([s.i32(), s.i32()]),
        10 => Order::Support(s.usize()),
        11 => Order::Pilot {
            throttle: s.u8().wrapping_sub(1) as i8,
            turn: s.u8().wrapping_sub(1) as i8,
            aim: std::array::from_fn(|_| s.i32()),
            fire: match s.u8() {
                0 => false,
                1 => true,
                _ => return None,
            },
        },
        _ => return None,
    };
    let destination = match s.u8() {
        0 => None,
        1 => Some([s.i32(), s.i32()]),
        _ => return None,
    };
    s.ok().then_some(PlannedOrder { order, destination })
}
fn supported(order: Order) -> bool {
    matches!(
        order,
        Order::Move
            | Order::Build(_)
            | Order::Attack(_)
            | Order::Repair(_)
            | Order::Dig(_)
            | Order::Reclaim(_)
            | Order::Ground(_)
            | Order::Hold
            | Order::Auto
    )
}
impl BlastRange {
    pub fn greben_build_admitted(&self, actor: usize, site: usize) -> bool {
        self.units
            .get(actor)
            .is_some_and(|u| u.kind == RangeUnitKind::GrebenStorage)
            && self.valid_planned_order(
                actor,
                PlannedOrder {
                    order: Order::Build(site),
                    destination: None,
                },
            )
    }
    pub fn construction_cancelled(&self, site: usize) -> bool {
        self.build_plans.cancelled.contains(&site)
    }
    pub fn order_queue(&self, actor: usize) -> Vec<PlannedOrder> {
        if actor>=self.units.len() {return Vec::new();}
        if let Some(q) = self.build_plans.queues.get(&(actor as u16)) {
            return q.iter().copied().collect();
        }
        self.current_planned_order(actor).into_iter().collect()
    }
    fn current_planned_order(&self, actor: usize) -> Option<PlannedOrder> {
        let a = self.battle.actors.get(actor)?;
        if a.order == Order::Move && self.units[actor].motion.destination.is_none() {
            return None;
        }
        if !supported(a.order) || matches!(a.order, Order::Auto | Order::Hold) {
            return None;
        }
        Some(PlannedOrder {
            order: a.order,
            destination: if a.order == Order::Move {
                self.units[actor].motion.destination
            } else {
                None
            },
        })
    }
    fn valid_planned_order(&self, actor: usize, p: PlannedOrder) -> bool {
        if !supported(p.order)
            || !matches!(self.battle.phase, Phase::Setup | Phase::Fighting)
            || !self
                .units
                .get(actor)
                .is_some_and(|u| u.status.alive && self.allied(actor))
            || self.unit_under_construction(actor)
        {
            return false;
        }
        if (p.order == Order::Move) != p.destination.is_some() {
            return false;
        }
        if let Order::Build(site) = p.order {
            if self.units[actor].kind == RangeUnitKind::GrebenStorage {
                return p.destination.is_none()
                    && self
                        .construction_site_state(site)
                        .is_some_and(|s| s.allied && !s.cancelled && !s.destroyed && !s.complete);
            }
        }
        self.queued_order_admitted(actor, p.order, p.destination)
    }
    pub(crate) fn queued_input_admitted(
        &self,
        actor: usize,
        order: Order,
        destination: Option<[i32; 2]>,
    ) -> bool {
        let p = PlannedOrder { order, destination };
        if !self.valid_planned_order(actor, p) {
            return false;
        }
        let queue = self.order_queue(actor);
        queue.len() < MAX_ORDERS || queue.last() == Some(&p)
    }
    fn apply_planned_order(&mut self, actor: usize, p: PlannedOrder) -> Outcome {
        if self.units[actor].kind == RangeUnitKind::GrebenStorage {
            if let Order::Build(site) = p.order {
                return if self.set_construction_quota(site, 4) {
                    Outcome::Applied
                } else {
                    Outcome::Rejected
                };
            }
        }
        // Automatic continuation is not a second external input in a replay.
        let command = if p.order == Order::Move {
            let Some(destination) = p.destination else {
                return Outcome::Rejected;
            };
            Command::FormationMove {
                actor,
                destination,
                fallback: destination,
            }
        } else {
            Command::Order {
                actor,
                order: p.order,
                destination: p.destination,
            }
        };
        self.execute_order_continuation(command)
    }
    pub(crate) fn clear_order_queue(&mut self, actor: usize) {
        if let Some(q) = self.build_plans.queues.remove(&(actor as u16)) {
            if self
                .units
                .get(actor)
                .is_some_and(|u| u.kind == RangeUnitKind::GrebenStorage)
            {
                for entry in q {
                    if let Order::Build(site) = entry.order {
                        self.set_construction_quota(site, 0);
                    }
                }
            }
        }
    }
    pub(crate) fn append_order(&mut self, actor: usize, p: PlannedOrder) -> Outcome {
        if !self.valid_planned_order(actor, p) {
            return Outcome::Rejected;
        }
        let mut queue = self
            .build_plans
            .queues
            .remove(&(actor as u16))
            .unwrap_or_else(|| self.current_planned_order(actor).into_iter().collect());
        if queue.back() == Some(&p) {
            self.build_plans.queues.insert(actor as u16, queue);
            let changed = self.battle.autonomous;
            self.battle.autonomous = false;
            return if changed {
                Outcome::Applied
            } else {
                Outcome::Unchanged
            };
        }
        if queue.len() >= MAX_ORDERS {
            self.build_plans.queues.insert(actor as u16, queue);
            return Outcome::Rejected;
        }
        if queue.is_empty() && !self.apply_planned_order(actor, p).accepted() {
            return Outcome::Rejected;
        }
        queue.push_back(p);
        self.build_plans.queues.insert(actor as u16, queue);
        self.battle.autonomous = false;
        Outcome::Queued
    }
    pub(crate) fn ordered_construction_admitted(
        &self,
        actor: usize,
        kind: RangeUnitKind,
        at: [i32; 3],
        queued: bool,
    ) -> bool {
        if !self.battle_mode() || !self.units.get(actor).is_some_and(|u| {
            u.status.alive
                && self.allied(actor)
                && matches!(
                    u.kind,
                    RangeUnitKind::Commander
                        | RangeUnitKind::SkobaDrone
                        | RangeUnitKind::GrebenStorage
                )
        }) || !matches!(self.battle.phase, Phase::Setup | Phase::Fighting)
            || queued && self.order_queue(actor).len() >= MAX_ORDERS
        {
            return false;
        }
        if self.units[actor].kind == RangeUnitKind::Commander
            && self.commander_tool_tip(actor).is_none()
            || self.units[actor].kind == RangeUnitKind::SkobaDrone
                && self.drone_leg(actor).is_none()
        {
            return false;
        }
        self.can_construct(kind, true, at)
    }
    pub(crate) fn place_ordered_construction(
        &mut self,
        actor: usize,
        kind: RangeUnitKind,
        at: [i32; 3],
        queued: bool,
    ) -> Outcome {
        if !self.ordered_construction_admitted(actor, kind, at, queued) {
            return Outcome::Rejected;
        }
        let Some(site) = self.create_construction_site(kind, true, at) else {
            return Outcome::Rejected;
        };
        self.set_construction_quota(site, 0);
        let p = PlannedOrder {
            order: Order::Build(site),
            destination: None,
        };
        if queued {
            self.append_order(actor, p)
        } else {
            self.clear_order_queue(actor);
            let result = self.apply_planned_order(actor, p);
            if result.accepted() && self.units[actor].kind == RangeUnitKind::GrebenStorage {
                self.build_plans
                    .queues
                    .insert(actor as u16, [p].into_iter().collect());
            }
            if result.accepted() {
                self.battle.autonomous = false;
            }
            result
        }
    }
    pub(crate) fn cancel_construction_plan(&mut self, site: usize) -> Outcome {
        let Some(s) = self.construction_site_state(site) else {
            return Outcome::Rejected;
        };
        if !s.allied || s.delivered != 0 || s.complete || s.destroyed || s.cancelled {
            return Outcome::Rejected;
        }
        self.set_construction_quota(site, 0);
        self.build_plans.cancelled.insert(site);
        Outcome::Applied
    }
    pub(crate) fn cancel_last_planned_order(&mut self, actor: usize) -> Outcome {
        if !self
            .units
            .get(actor)
            .is_some_and(|u| u.status.alive && self.allied(actor))
        {
            return Outcome::Rejected;
        }
        let mut queue = self
            .build_plans
            .queues
            .remove(&(actor as u16))
            .unwrap_or_else(|| self.current_planned_order(actor).into_iter().collect());
        let Some(last) = queue.pop_back() else {
            return Outcome::Rejected;
        };
        let stop = queue.is_empty();
        if !stop {
            self.build_plans.queues.insert(actor as u16, queue);
        } else {
            self.apply_planned_order(
                actor,
                PlannedOrder {
                    order: Order::Hold,
                    destination: None,
                },
            );
        }
        if let Order::Build(site) = last.order {
            let others = self
                .battle
                .actors
                .iter()
                .enumerate()
                .any(|(id, a)| id != actor && a.order == Order::Build(site))
                || self
                    .build_plans
                    .queues
                    .values()
                    .any(|q| q.iter().any(|p| p.order == Order::Build(site)));
            if !others {
                self.cancel_construction_plan(site);
            }
        }
        self.battle.autonomous = false;
        Outcome::Applied
    }
    pub(crate) fn order_plan_phase(&mut self) {
        let ids: Vec<_> = self.build_plans.queues.keys().copied().collect();
        for id in ids {
            let actor = usize::from(id);
            if !self.units.get(actor).is_some_and(|u| u.status.alive) {
                self.clear_order_queue(actor);
                continue;
            }
            let p = self.build_plans.queues[&id][0];
            let done = match p.order {
                Order::Build(site) => self
                    .construction_site_state(site)
                    .is_none_or(|s| s.complete || s.cancelled || s.destroyed),
                Order::Move => {
                    self.units[actor].motion.destination.is_none() && !self.hauling(actor)
                }
                Order::Attack(target) => self.units.get(target).is_none_or(|u| !u.status.alive),
                Order::Repair(target) => !self.can_commander_repair(actor, target),
                _ => false,
            };
            if !done {
                continue;
            }
            let mut queue = self.build_plans.queues.remove(&id).unwrap();
            queue.pop_front();
            while let Some(&next) = queue.front() {
                if self.valid_planned_order(actor, next)
                    && self.apply_planned_order(actor, next).accepted()
                {
                    break;
                }
                queue.pop_front();
            }
            if !queue.is_empty() {
                self.build_plans.queues.insert(id, queue);
            }
        }
    }
    pub(crate) fn validate_build_plans(&self) -> Option<()> {
        if self.build_plans.cancelled.iter().any(|&site| {
            self.construction_site_state(site)
                .is_none_or(|s| s.delivered != 0)
        }) {
            return None;
        }
        for (&id, queue) in &self.build_plans.queues {
            let actor = usize::from(id);
            if actor >= self.units.len() || !self.allied(actor) {
                return None;
            }
            for p in queue {
                if !supported(p.order) {
                    return None;
                }
                match p.order {
                    Order::Build(site)
                        if self.construction_site_state(site).is_none_or(|s| !s.allied) =>
                    {
                        return None
                    }
                    Order::Attack(id) | Order::Repair(id) if id >= self.units.len() => return None,
                    Order::Dig(at) | Order::Reclaim(at) | Order::Ground(at)
                        if (0..3).any(|a| at[a] < 0 || at[a] >= self.size[a] as i32) =>
                    {
                        return None
                    }
                    _ => {}
                }
                if (p.order == Order::Move) != p.destination.is_some()
                    || p.destination.is_some_and(|at| {
                        at[0] < 0
                            || at[1] < 0
                            || at[0] >= self.size[0] as i32
                            || at[1] >= self.size[2] as i32
                    })
                {
                    return None;
                }
            }
        }
        Some(())
    }
}
