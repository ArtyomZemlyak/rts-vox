//! Original handlers retained only as a differential test oracle.
use super::*;

impl BlastRange {
    pub(super) fn legacy_ai(
        &mut self,
        battle: &mut Battle,
        decision: ai::Decision,
        side: bool,
    ) -> bool {
        let id = decision.actor;
        if self.allied(id) != side || !self.units.get(id).is_some_and(|u| u.status.alive) {
            return false;
        }
        let order = match decision.order {
            ai::AiOrder::Attack(target) => {
                if self.allied(target) == side
                    || !self.units.get(target).is_some_and(|u| u.status.alive)
                {
                    return false;
                }
                Order::Attack(target)
            }
            ai::AiOrder::Move(_) => Order::Move,
            ai::AiOrder::Hold => Order::Hold,
            ai::AiOrder::Auto => Order::Auto,
        };
        if battle.actors[id].order == order && !matches!(decision.order, ai::AiOrder::Move(_)) {
            return true;
        }
        if let ai::AiOrder::Move(p) = decision.order {
            if self.units[id].kind.emplacement() {
                return false;
            }
            if battle.actors[id].order == Order::Move {
                let motion = &self.units[id].motion;
                let dx = i64::from(motion.pos[0].to_int_trunc() - p[0]);
                let dz = i64::from(motion.pos[2].to_int_trunc() - p[1]);
                let arrival = i64::from(motion.arrival.max(1));
                if motion.destination == Some(p)
                    || (motion.destination.is_none() && dx * dx + dz * dz <= arrival * arrival)
                {
                    return true;
                }
            }
            let others: Vec<_> = self
                .units
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != id)
                .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
                .collect();
            let u = &mut self.units[id];
            // The whole-map search is walked by the range (ADR-678).
            u.motion.allow_coarse(Some(0));
            let accepted = u
                .motion
                .move_to(&self.world, p, &u.body, &others, &self.rubble);
            u.motion.allow_coarse(None);
            if !accepted {
                return false;
            }
        } else if order == Order::Hold {
            self.units[id].motion.stop();
        }
        let actor = &mut battle.actors[id];
        actor.order = order;
        actor.target = None;
        if !(self.units[id].kind == RangeUnitKind::Spider
            && actor.burst > 0
            && order != Order::Hold)
        {
            stop_spider_beam(actor, self.units[id].kind, self.tick);
            actor.charge = None;
            actor.burst = 0;
        }
        actor.pursuit = 0;
        true
    }
    pub(super) fn legacy_order(
        &mut self,
        id: usize,
        order: Order,
        destination: Option<[i32; 2]>,
    ) -> bool {
        if !self.battle_mode()
            || !(self.battle.phase == Phase::Fighting
                || self.battle.phase == Phase::Setup && matches!(order, Order::Move | Order::Hold))
            || !self.allied(id)
            || !self.units.get(id).is_some_and(|u| u.status.alive)
        {
            return false;
        }
        if let Order::Attack(other) = order {
            if self.allied(other) || !self.units.get(other).is_some_and(|u| u.status.alive) {
                return false;
            }
        }
        if let Order::Ground(p) = order {
            if (0..3).any(|a| p[a] < 0 || p[a] >= self.size[a] as i32) {
                return false;
            }
        }
        if order == Order::Move {
            if self.units[id].kind.emplacement() {
                return false;
            }
            let Some(p) = destination else {
                return false;
            };
            let others: Vec<_> = self
                .units
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != id)
                .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
                .collect();
            // Between ticks: the rubble of the present state (ADR-426).
            let rubble = self.current_rubble();
            let u = &mut self.units[id];
            u.motion.route.clear();
            // The whole-map search is walked by the range (ADR-678).
            u.motion.allow_coarse(Some(0));
            let accepted = u.motion.move_to(&self.world, p, &u.body, &others, &rubble);
            u.motion.allow_coarse(None);
            if !accepted {
                return false;
            }
        } else if order == Order::Hold {
            self.units[id].motion.stop();
        }
        let a = &mut self.battle.actors[id];
        a.order = order;
        a.target = None;
        if !(self.units[id].kind == RangeUnitKind::Spider && a.burst > 0 && order != Order::Hold) {
            stop_spider_beam(a, self.units[id].kind, self.tick);
            a.charge = None;
            a.burst = 0;
        }
        a.pursuit = 0;
        true
    }
    pub(super) fn legacy_group(
        &mut self,
        destinations: &[(usize, [i32; 2])],
        fallback: [i32; 2],
    ) -> usize {
        if !self.battle_mode() || !self.battle_movement_active() {
            return 0;
        }
        let mut accepted = 0;
        for &(id, target) in destinations {
            if !self.allied(id)
                || !self.units.get(id).is_some_and(|u| u.status.alive)
                || self.units[id].kind.emplacement()
            {
                continue;
            }
            let unit = &mut self.units[id];
            let moved = unit.motion.queue_move(&self.world, target)
                || unit.motion.queue_move(&self.world, fallback);
            if moved {
                let actor = &mut self.battle.actors[id];
                actor.order = Order::Move;
                actor.target = None;
                if self.units[id].kind != RangeUnitKind::Spider || actor.burst == 0 {
                    actor.charge = None;
                    actor.burst = 0;
                }
                actor.pursuit = 0;
                accepted += 1;
            }
        }
        accepted
    }
}
