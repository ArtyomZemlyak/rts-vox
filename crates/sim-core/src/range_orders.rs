//! Common order validation and mutation. Adapters retain their phase and
//! route-replacement semantics; neither controller owns another physics path.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Request {
    pub actor: usize,
    pub order: Order,
    pub destination: Option<[i32; 2]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Replacement {
    Explicit,
    Maintain,
}

pub(super) enum Admission {
    Rejected,
    Unchanged,
    Apply,
}

/// Reads the current owner state before any mutation or route preparation.
pub(super) fn validate(
    range: &BlastRange,
    actors: &[Actor],
    request: Request,
    side: bool,
    replacement: Replacement,
) -> Admission {
    let Request {
        actor: id,
        order,
        destination,
    } = request;
    if range.allied(id) != side
        || !range.units.get(id).is_some_and(|u| u.status.alive)
        || id >= actors.len()
    {
        return Admission::Rejected;
    }
    if range.idle_enemy(id) && order != Order::Hold {
        return Admission::Rejected;
    }
    if range.units[id].kind == RangeUnitKind::NuclearSilo
        && (replacement == Replacement::Maintain
            || !matches!(order, Order::Ground(_) | Order::Hold | Order::Pilot { .. }))
    {
        return Admission::Rejected;
    }
    if let Order::Pilot {
        throttle,
        turn,
        aim,
        ..
    } = order
    {
        if !(-1..=1).contains(&throttle)
            || !(-1..=1).contains(&turn)
            || destination.is_some()
            || (0..3).any(|a| aim[a] < 0 || aim[a] >= range.size[a] as i32)
        {
            return Admission::Rejected;
        }
        if actors[id].order == order {
            return Admission::Unchanged;
        }
    }
    if let Order::Attack(target) = order {
        if range.allied(target) == side || !range.units.get(target).is_some_and(|u| u.status.alive)
        {
            return Admission::Rejected;
        }
    }
    if let Order::Support(target) = order {
        if target == id || destination.is_some()
            || range.units[id].kind != RangeUnitKind::LightningTank
            || !range.units.get(target).is_some_and(|u| u.kind == RangeUnitKind::LightningTank && u.status.alive)
            || range.allied(target) != side
            || matches!(actors[target].order, Order::Support(_))
            || actors.iter().any(|actor| actor.order == Order::Support(id))
        { return Admission::Rejected; }
    }
    if let Order::Ground(p) = order {
        if (0..3).any(|a| p[a] < 0 || p[a] >= range.size[a] as i32) {
            return Admission::Rejected;
        }
        if range.units[id].kind == RangeUnitKind::NuclearSilo {
            let pos = range.units[id].motion.pos;
            let dx = i64::from(p[0] - pos[0].to_int_trunc());
            let dz = i64::from(p[2] - pos[2].to_int_trunc());
            let reach = i64::from(range.battle.tuning.nuclear.range);
            if dx * dx + dz * dz > reach * reach || range.tick < actors[id].next {
                return Admission::Rejected;
            }
        }
    }
    // A face inside the map, for a kind with a tank; the approach is the
    // range's to derive, not the caller's to give (ADR-672).
    if let Order::Dig(p) | Order::Reclaim(p) = order {
        if (0..3).any(|a| p[a] < 0 || p[a] >= range.size[a] as i32)
            || destination.is_some()
            || !range.mining().is_some_and(|m| m.digs(range.units[id].kind))
        {
            return Admission::Rejected;
        }
    }
    // The rules AI maintains orders; it never replaces a dig (ADR-672).
    if replacement == Replacement::Maintain
        && matches!(actors[id].order, Order::Dig(_) | Order::Reclaim(_) | Order::Support(_) | Order::Pilot { .. })
    {
        return Admission::Unchanged;
    }
    if replacement == Replacement::Maintain && actors[id].order == order && order != Order::Move {
        return Admission::Unchanged;
    }
    if order == Order::Move {
        if range.units[id].kind.emplacement() {
            return Admission::Rejected;
        }
        let Some(p) = destination else {
            return Admission::Rejected;
        };
        if replacement == Replacement::Maintain && actors[id].order == Order::Move {
            let motion = &range.units[id].motion;
            let dx = i64::from(motion.pos[0].to_int_trunc() - p[0]);
            let dz = i64::from(motion.pos[2].to_int_trunc() - p[1]);
            let arrival = i64::from(motion.arrival.max(1));
            if motion.destination == Some(p)
                || (motion.destination.is_none() && dx * dx + dz * dz <= arrival * arrival)
            {
                return Admission::Unchanged;
            }
        }
    }
    Admission::Apply
}

pub(super) struct Executor<'a> {
    pub units: &'a mut [RangeUnit],
    pub actors: &'a mut [Actor],
    pub world: &'a World,
    pub rubble: &'a nav::vehicle::Rubble,
    pub tick: u32,
}

impl Executor<'_> {
    /// Only an admitted request reaches mutation. Neighbours are built in
    /// source order and a failed search retains its original failure effects.
    pub fn apply(&mut self, request: Request, replacement: Replacement) -> commands::Outcome {
        let Request {
            actor: id,
            order,
            destination,
        } = request;
        let digging = matches!(order, Order::Dig(_) | Order::Reclaim(_));
        if matches!(order, Order::Pilot { .. })
            && !matches!(self.actors[id].order, Order::Pilot { .. })
        {
            let u = &mut self.units[id];
            u.motion.stop();
            u.motion.route.clear();
            if !u.kind.emplacement() {
                u.motion.enabled = true;
            }
        }
        if order == Order::Move || digging && destination.is_some() {
            let others: Vec<_> = self
                .units
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != id)
                .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
                .collect();
            let u = &mut self.units[id];
            if replacement == Replacement::Explicit {
                u.motion.route.clear();
            }
            // A whole-map search begins with no work here; the range walks
            // it under its shared allowance from the next tick (ADR-680).
            u.motion.allow_coarse(Some(0));
            let accepted = u.motion.move_to(
                self.world,
                destination.expect("admitted move"),
                &u.body,
                &others,
                self.rubble,
            );
            u.motion.allow_coarse(None);
            if !accepted {
                return commands::Outcome::PathFailed;
            }
        } else if order == Order::Hold || digging || matches!(order, Order::Support(_)) {
            self.units[id].motion.stop();
        }
        commit(&mut self.actors[id], self.units[id].kind, order, self.tick);
        commands::Outcome::Applied
    }

    /// Formation admission defers search to the existing Motion budget.
    pub fn queue(&mut self, request: Request, fallback: [i32; 2]) -> commands::Outcome {
        let unit = &mut self.units[request.actor];
        if !(unit
            .motion
            .queue_move(self.world, request.destination.expect("admitted formation"))
            || unit.motion.queue_move(self.world, fallback))
        {
            return commands::Outcome::PathFailed;
        }
        commit(
            &mut self.actors[request.actor],
            unit.kind,
            Order::Move,
            self.tick,
        );
        commands::Outcome::Queued
    }
}

/// Shared final commit for immediate and budgeted formation moves. An active
/// siege pulse survives retargeting; Hold consumes it with the normal reload.
pub(super) fn commit(actor: &mut Actor, kind: RangeUnitKind, order: Order, tick: u32) {
    let continuing =
        matches!(actor.order, Order::Pilot { .. }) && matches!(order, Order::Pilot { .. });
    actor.order = order;
    if continuing {
        return;
    }
    actor.target = None;
    if !(kind == RangeUnitKind::Spider && actor.burst > 0 && order != Order::Hold) {
        stop_active_beam(actor, kind, tick);
        actor.charge = None;
        actor.burst = 0;
    }
    actor.pursuit = 0;
}
