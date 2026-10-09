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
    if let Order::Attack(target) = order {
        if range.allied(target) == side || !range.units.get(target).is_some_and(|u| u.status.alive)
        {
            return Admission::Rejected;
        }
    }
    if let Order::Ground(p) = order {
        if (0..3).any(|a| p[a] < 0 || p[a] >= range.size[a] as i32) {
            return Admission::Rejected;
        }
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
        if order == Order::Move {
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
            if !u.motion.move_to(
                self.world,
                destination.expect("admitted move"),
                &u.body,
                &others,
                self.rubble,
            ) {
                return commands::Outcome::PathFailed;
            }
        } else if order == Order::Hold {
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
    actor.order = order;
    actor.target = None;
    if !(kind == RangeUnitKind::Spider && actor.burst > 0 && order != Order::Hold) {
        stop_spider_beam(actor, kind, tick);
        actor.charge = None;
        actor.burst = 0;
    }
    actor.pursuit = 0;
}
