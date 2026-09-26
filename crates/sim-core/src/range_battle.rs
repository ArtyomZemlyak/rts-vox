//! Bounded skirmish on the range's ordinary, fully damageable unit roster.
//! Commands never exchange slots, matter containers or projectile ownership.
use super::ai::{self, Policy};
use super::*;
use fx::blob::{Blob, Scan};
use modding::range_battle::Tuning;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Setup,
    Fighting,
    Victory,
    Defeat,
    Draw,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Auto,
    Hold,
    Move,
    Attack(usize),
    Ground([i32; 3]),
}
pub struct Actor {
    pub turret: crate::turret::Traverse,
    pub order: Order,
    pub target: Option<usize>,
    next: u32,
    charge: Option<u32>,
    pursuit: u32,
    burst: u32,
    pub fired: u32,
}
pub struct Round {
    pub owner: usize,
    pub shell: bool,
    pub pos: [Fx; 3],
    pub previous: [Fx; 3],
    velocity: [Fx; 3],
    distance: Fx,
}
pub struct Burst {
    wave: Explosion,
    energy: u64,
}
#[derive(Clone, Copy)]
pub struct Impact {
    pub at: [i32; 3],
    pub shell: bool,
}
pub struct Battle {
    pub phase: Phase,
    pub actors: Vec<Actor>,
    pub rounds: Vec<Round>,
    waves: Vec<Burst>,
    pub impacts: Vec<Impact>,
    pub traces: Vec<weapons::Trace>,
    pub tuning: std::sync::Arc<Tuning>,
    pub shell_config: ExplosionConfig,
    bullet_config: ExplosionConfig,
    pub(super) ai: ai::Controller,
}
impl Battle {
    pub fn new(units: &[RangeUnit], resistance: u32) -> Self {
        let tuning = std::sync::Arc::new(
            Tuning::from_ron(include_str!("../../../data/range-battle.ron"))
                .expect("battle tuning"),
        );
        let mut shell_config =
            ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        shell_config.energy = shell_config.energy * tuning.tank_power_milli / 1000;
        shell_config.material_energy_j = (u64::from(shell_config.material_energy_j)
            * u64::from(tuning.tank_power_milli)
            / 1000) as u32;
        shell_config.unit_ablation_resistance = resistance;
        let mut bullet_config =
            ExplosionConfig::from_ron(include_str!("../../../data/machine-gun.ron")).unwrap();
        bullet_config.unit_ablation_resistance = resistance;
        Self {
            phase: Phase::Setup,
            actors: units
                .iter()
                .map(|u| Actor {
                    turret: crate::turret::Traverse::new(u.motion.drive.head),
                    order: Order::Auto,
                    target: None,
                    next: 0,
                    charge: None,
                    pursuit: 0,
                    burst: 0,
                    fired: 0,
                })
                .collect(),
            rounds: Vec::new(),
            waves: Vec::new(),
            impacts: Vec::new(),
            traces: Vec::new(),
            tuning,
            shell_config,
            bullet_config,
            ai: ai::Controller::new(),
        }
    }
    pub(super) fn working(&self) -> bool {
        !self.waves.is_empty()
    }
    fn range(&self, kind: RangeUnitKind, streams: &weapons::Streams) -> i32 {
        if kind == RangeUnitKind::Tank {
            self.tuning.tank_range
        } else {
            streams.tuning.machine_gun.range
        }
    }
    pub(super) fn save(&self, b: &mut Blob) {
        b.u8(match self.phase {
            Phase::Setup => 0,
            Phase::Fighting => 1,
            Phase::Victory => 2,
            Phase::Defeat => 3,
            Phase::Draw => 4,
        });
        for a in &self.actors {
            a.turret.save(b);
            match a.order {
                Order::Auto => b.u8(0),
                Order::Hold => b.u8(1),
                Order::Move => b.u8(2),
                Order::Attack(i) => {
                    b.u8(3);
                    b.usize(i);
                }
                Order::Ground(p) => {
                    b.u8(4);
                    for v in p {
                        b.i32(v);
                    }
                }
            }
            b.usize(a.target.unwrap_or(usize::MAX));
            b.u32(a.next);
            b.u32(a.charge.unwrap_or(u32::MAX));
            b.u32(a.pursuit);
            b.u32(a.burst);
            b.u32(a.fired);
        }
        b.usize(self.rounds.len());
        for r in &self.rounds {
            b.usize(r.owner);
            b.bool(r.shell);
            for v in r.pos.into_iter().chain(r.velocity) {
                b.fx(v);
            }
            b.fx(r.distance);
        }
        b.usize(self.waves.len());
        for w in &self.waves {
            b.u64(w.energy);
            w.wave.save(b);
        }
        self.ai.save(b);
    }
    pub(super) fn load(
        &mut self,
        s: &mut Scan,
        units: &[RangeUnit],
        size: [u32; 3],
        limits: crate::turret::Limits,
        migration: (u32, u32, &[bool]),
    ) -> Option<()> {
        let (version, tick, allied) = migration;
        self.phase = match s.u8() {
            0 => Phase::Setup,
            1 => Phase::Fighting,
            2 => Phase::Victory,
            3 => Phase::Defeat,
            4 => Phase::Draw,
            _ => return None,
        };
        for a in &mut self.actors {
            a.turret = crate::turret::Traverse::load(s, limits)?;
            a.order = match s.u8() {
                0 => Order::Auto,
                1 => Order::Hold,
                2 => Order::Move,
                3 => {
                    let i = s.usize();
                    if i >= units.len() {
                        return None;
                    }
                    Order::Attack(i)
                }
                4 => {
                    let p = std::array::from_fn(|_| s.i32());
                    if (0..3).any(|k| p[k] < 0 || p[k] >= size[k] as i32) {
                        return None;
                    }
                    Order::Ground(p)
                }
                _ => return None,
            };
            let i = s.usize();
            if i != usize::MAX && i >= units.len() {
                return None;
            }
            a.target = (i != usize::MAX).then_some(i);
            a.next = s.u32();
            let t = s.u32();
            a.charge = (t != u32::MAX).then_some(t);
            a.pursuit = s.u32();
            a.burst = s.u32();
            a.fired = s.u32();
            if a.burst > 300 {
                return None;
            }
        }
        let n = s.usize();
        if n > self.tuning.max_rounds {
            return None;
        }
        for _ in 0..n {
            let owner = s.usize();
            let shell = s.bool();
            let pos = std::array::from_fn(|_| s.fx());
            let velocity = std::array::from_fn(|_| s.fx());
            let distance = s.fx();
            // A lobbed shell flies above the world (`in_flight_bounds`); its
            // remaining flight, at most 10 000, bounds how high.
            let above = |k: usize| if k == 1 { 10_000 } else { 384 };
            if owner >= units.len()
                || (0..3).any(|k| {
                    pos[k] < Fx::from_int(-384) || pos[k] > Fx::from_int(size[k] as i32 + above(k))
                })
                || distance <= Fx::ZERO
                || distance > Fx::from_int(10000)
                || velocity.iter().any(|v| v.abs() > Fx::from_int(10000))
            {
                return None;
            }
            self.rounds.push(Round {
                owner,
                shell,
                pos,
                previous: pos,
                velocity,
                distance,
            });
        }
        let n = s.usize();
        if n.saturating_add(self.rounds.iter().filter(|r| r.shell).count()) > self.tuning.max_waves
        {
            return None;
        }
        for _ in 0..n {
            let energy = s.u64();
            if energy > u64::from(self.shell_config.material_energy_j) * 1000 {
                return None;
            }
            self.waves.push(Burst {
                energy,
                wave: Explosion::load(s, units, size, true)?,
            });
        }
        if version >= 38 {
            self.ai
                .load(s, tick, self.phase == Phase::Fighting, size, allied)?;
        } else {
            self.ai.next_tick = tick;
        }
        s.ok().then_some(())
    }
}
fn aim_point(u: &RangeUnit) -> [i32; 3] {
    let origin = [u.body.core.0, u.body.core.1, u.body.core.2].map(usize::from);
    let size = usize::from(u.body.core_size);
    let dims = [u.body.vol.nx, u.body.vol.ny, u.body.vol.nz].map(usize::from);
    let mut best = None;
    for z in origin[2]..(origin[2] + size).min(dims[2]) {
        for y in origin[1]..(origin[1] + size).min(dims[1]) {
            for x in origin[0]..(origin[0] + size).min(dims[0]) {
                if !u.body.vol.get(x, y, z) {
                    continue;
                }
                let p = [x, y, z];
                let d = (0..3)
                    .map(|a| ((2 * p[a]) as i32 - (2 * origin[a] + size - 1) as i32).pow(2))
                    .sum::<i32>();
                let key = (d, x + dims[0] * (y + dims[1] * z), p);
                if best.is_none_or(|old| key < old) {
                    best = Some(key);
                }
            }
        }
    }
    u.world_pos(best.map_or(origin, |(_, _, p)| p))
}
impl BlastRange {
    pub fn battle_mode(&self) -> bool {
        self.layout.as_ref().is_some_and(|l| l.battle)
    }
    pub fn enemy_ai_tuning(&self) -> &modding::battle_ai::Tuning {
        &self.battle.ai.tuning
    }
    /// Rules are data-driven and can be replaced while setting up a battle.
    /// Changing them during a fight starts a new decision cycle on this tick.
    pub fn configure_enemy_ai(&mut self, tuning: modding::battle_ai::Tuning) -> Result<(), String> {
        tuning.validate()?;
        self.battle.ai.tuning = tuning;
        self.battle.ai.next_tick = self.tick;
        self.battle.ai.pending = None;
        Ok(())
    }
    /// Snapshot for a rule engine or an asynchronous model runner. The caller
    /// must return a plan tagged with `decision_tick`; a late plan is rejected.
    pub fn enemy_ai_observation(&self) -> Option<ai::Observation> {
        if !self.battle_mode() || self.battle.phase != Phase::Fighting {
            return None;
        }
        Some(self.enemy_ai_observation_for(&self.battle))
    }
    fn enemy_ai_observation_for(&self, battle: &Battle) -> ai::Observation {
        ai::Observation {
            version: ai::OBSERVATION_VERSION,
            observed_tick: self.tick,
            decision_tick: battle.ai.next_tick,
            size: self.size,
            units: self
                .units
                .iter()
                .enumerate()
                .map(|(id, unit)| ai::UnitObservation {
                    id,
                    allied: self.allied(id),
                    alive: unit.status.alive,
                    armed: unit.status.weapon,
                    mobile: unit.status.engine,
                    kind: unit.kind,
                    position: [
                        unit.motion.pos[0].to_int_trunc(),
                        unit.motion.pos[2].to_int_trunc(),
                    ],
                    ordered_target: match battle.actors[id].order {
                        Order::Attack(target) => Some(target),
                        _ => None,
                    },
                })
                .collect(),
        }
    }
    /// Accept one complete, slot-ordered enemy plan for the scheduled tick.
    /// Validation happens before mutation; simulation falls back to rules if
    /// no valid plan is available when that tick arrives.
    pub fn submit_enemy_ai_plan(
        &mut self,
        decision_tick: u32,
        plan: Vec<ai::Decision>,
    ) -> Result<(), &'static str> {
        if !self.battle_mode() || self.battle.phase != Phase::Fighting {
            return Err("battle is not fighting");
        }
        if decision_tick != self.battle.ai.next_tick || decision_tick < self.tick {
            return Err("stale or unscheduled AI tick");
        }
        if self.battle.ai.pending.is_some() {
            return Err("AI plan already submitted");
        }
        let enemies: Vec<_> = self
            .units
            .iter()
            .enumerate()
            .filter(|(id, u)| !self.allied(*id) && u.status.alive)
            .map(|(id, _)| id)
            .collect();
        if plan.len() != enemies.len() {
            return Err("AI plan must cover every living enemy");
        }
        for (decision, id) in plan.iter().zip(enemies) {
            if decision.actor != id {
                return Err("AI plan actors must be in slot order");
            }
            match decision.order {
                ai::AiOrder::Attack(target)
                    if !self.allied(target)
                        || !self.units.get(target).is_some_and(|u| u.status.alive) =>
                {
                    return Err("AI attack needs a living allied target");
                }
                ai::AiOrder::Move(p)
                    if p[0] < 0
                        || p[1] < 0
                        || p[0] >= self.size[0] as i32
                        || p[1] >= self.size[2] as i32 =>
                {
                    return Err("AI destination outside map");
                }
                _ => {}
            }
        }
        self.battle.ai.pending = Some(plan);
        Ok(())
    }
    fn battle_ai_tick(&mut self, battle: &mut Battle) {
        if self.tick < battle.ai.next_tick {
            return;
        }
        let observation = self.enemy_ai_observation_for(battle);
        let rules = ai::Rules {
            tuning: &battle.ai.tuning,
        }
        .decide(&observation);
        let supplied = battle.ai.pending.take();
        for fallback in rules {
            let decision = supplied
                .as_ref()
                .and_then(|plan| plan.iter().find(|d| d.actor == fallback.actor))
                .copied()
                .unwrap_or(fallback);
            if !self.apply_enemy_ai_decision(battle, decision) {
                self.apply_enemy_ai_decision(battle, fallback);
            }
        }
        battle.ai.next_tick = self.tick.saturating_add(battle.ai.tuning.decision_ticks);
    }
    fn apply_enemy_ai_decision(&mut self, battle: &mut Battle, decision: ai::Decision) -> bool {
        let id = decision.actor;
        if self.allied(id) || !self.units.get(id).is_some_and(|u| u.status.alive) {
            return false;
        }
        let order = match decision.order {
            ai::AiOrder::Attack(target) => {
                if !self.allied(target) || !self.units.get(target).is_some_and(|u| u.status.alive) {
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
            if battle.actors[id].order == Order::Move
                && self.units[id].motion.destination == Some(p)
            {
                return true;
            }
            let others: Vec<_> = self
                .units
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != id)
                .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
                .collect();
            let u = &mut self.units[id];
            if !u
                .motion
                .move_to(&self.world, p, &u.body, &others, &self.rubble)
            {
                return false;
            }
        } else {
            self.units[id].motion.stop();
        }
        let actor = &mut battle.actors[id];
        actor.order = order;
        actor.target = None;
        actor.charge = None;
        actor.burst = 0;
        actor.pursuit = 0;
        true
    }
    pub fn allied(&self, id: usize) -> bool {
        self.layout
            .as_ref()
            .and_then(|l| l.units.get(id))
            .is_some_and(|u| u.allied)
    }
    pub fn start_battle(&mut self) -> Result<(), &'static str> {
        if !self.battle_mode() {
            return Err("Сначала включите режим боя");
        }
        if self.battle.phase != Phase::Setup {
            return Err("Бой уже начат; восстановите расстановку для нового боя");
        }
        if !self
            .units
            .iter()
            .enumerate()
            .any(|(i, u)| u.status.alive && self.allied(i))
            || !self
                .units
                .iter()
                .enumerate()
                .any(|(i, u)| u.status.alive && !self.allied(i))
        {
            return Err("Нужен хотя бы один свой и один вражеский юнит");
        }
        self.control();
        self.fire_tick = u32::MAX;
        self.patrols_running = true;
        for u in &mut self.units {
            u.motion.route.clear();
            u.motion.stop();
        }
        self.battle.phase = Phase::Fighting;
        self.battle.ai.next_tick = self.tick;
        self.battle.ai.pending = None;
        Ok(())
    }
    pub fn battle_order(&mut self, id: usize, order: Order, destination: Option<[i32; 2]>) -> bool {
        if !self.battle_mode()
            || self.battle.phase != Phase::Fighting
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
            if !u.motion.move_to(&self.world, p, &u.body, &others, &rubble) {
                return false;
            }
        } else {
            self.units[id].motion.stop();
        }
        let a = &mut self.battle.actors[id];
        a.order = order;
        a.target = None;
        a.charge = None;
        a.burst = 0;
        a.pursuit = 0;
        true
    }
    pub fn battle_counts(&self) -> [usize; 2] {
        let mut n = [0; 2];
        for (i, u) in self.units.iter().enumerate() {
            if u.status.alive {
                n[usize::from(!self.allied(i))] += 1;
            }
        }
        n
    }
    #[cfg(test)]
    pub(super) fn battle_tick(&mut self) {
        self.battle_tick_observed(|_| {});
    }
    pub(super) fn battle_tick_observed(&mut self, mut completed: impl FnMut(&'static str)) {
        if !self.battle_mode() {
            return;
        }
        // Move the controller out, never the actors. Damage IDs stay stable.
        let empty = Battle {
            phase: Phase::Setup,
            actors: Vec::new(),
            rounds: Vec::new(),
            waves: Vec::new(),
            impacts: Vec::new(),
            traces: Vec::new(),
            tuning: self.battle.tuning.clone(),
            shell_config: self.battle.shell_config.clone(),
            bullet_config: self.battle.bullet_config.clone(),
            ai: self.battle.ai.clone(),
        };
        let mut battle = std::mem::replace(&mut self.battle, empty);
        battle.impacts.clear();
        battle.traces.clear();
        if battle.phase == Phase::Fighting {
            self.battle_ai_tick(&mut battle);
            self.battle_orders(&mut battle);
        }
        completed("battle_orders");
        let revisions: Vec<_> = self.units.iter().map(|u| u.revision).collect();
        self.battle_flight(&mut battle);
        completed("battle_flight");
        self.battle_waves(&mut battle, &mut completed);
        completed("battle_waves");
        for (id, (u, old)) in self.units.iter_mut().zip(revisions).enumerate() {
            if u.revision != old {
                u.refresh_damage(id);
            }
        }
        completed("battle_damage");
        if battle.phase == Phase::Fighting && battle.rounds.is_empty() && battle.waves.is_empty() {
            let n = self.battle_counts();
            battle.phase = match n {
                [0, 0] => Phase::Draw,
                [0, _] => Phase::Defeat,
                [_, 0] => Phase::Victory,
                _ => Phase::Fighting,
            };
            if battle.phase != Phase::Fighting {
                for u in &mut self.units {
                    u.motion.stop();
                }
            }
        }
        self.battle = battle;
    }
    fn battle_orders(&mut self, b: &mut Battle) {
        let stream = self.streams.tuning.machine_gun.clone();
        for id in 0..self.units.len() {
            if !self.units[id].status.alive || !self.units[id].status.weapon {
                b.actors[id].charge = None;
                b.actors[id].target = None;
                continue;
            }
            let kind = self.units[id].kind;
            let range = b.range(kind, &self.streams);
            let from = self.units[id].motion.pos;
            let distance = |other: usize| {
                let p = self.units[other].motion.pos;
                let dx = p[0].sub(from[0]);
                let dz = p[2].sub(from[2]);
                dx.mul(dx).add(dz.mul(dz))
            };
            let hostile = |other: usize| {
                other != id
                    && self.units[other].status.alive
                    && self.allied(other) != self.allied(id)
            };
            let a = &mut b.actors[id];
            if matches!(a.order, Order::Attack(i) if !hostile(i))
                || (a.order == Order::Move && self.units[id].motion.destination.is_none())
            {
                a.order = Order::Auto;
            }
            if a.order == Order::Hold || a.order == Order::Move {
                a.target = None;
                continue;
            }
            let target = if let Order::Attack(i) = a.order {
                hostile(i).then_some(i)
            } else {
                (0..self.units.len())
                    .filter(|&i| hostile(i))
                    .min_by_key(|&i| (distance(i), i))
            };
            let target_changed = a.target != target;
            if target_changed {
                a.charge = None;
                a.pursuit = 0;
            }
            a.target = target;
            let mut p = if let Order::Ground(p) = a.order {
                p
            } else if let Some(other) = target {
                aim_point(&self.units[other])
            } else {
                continue;
            };
            if let Some(other) = target.filter(|_| !matches!(a.order, Order::Ground(_))) {
                let u = &self.units[other];
                let speed = if kind == RangeUnitKind::Tank {
                    b.shell_config.shell_speed
                } else {
                    b.bullet_config.shell_speed
                };
                let time = distance(other).sqrt().div(Fx::from_int(speed));
                let travel = u.motion.drive.vel.mul(time);
                p[0] += travel
                    .mul(fx::angle::cos(u.motion.drive.head))
                    .to_int_trunc();
                p[2] += travel
                    .mul(fx::angle::sin(u.motion.drive.head))
                    .to_int_trunc();
            }
            let dx = Fx::from_int(p[0]).sub(from[0]);
            let dz = Fx::from_int(p[2]).sub(from[2]);
            let in_range =
                dx.mul(dx).add(dz.mul(dz)) <= Fx::from_int(range).mul(Fx::from_int(range));
            if !in_range {
                a.charge = None;
                if self.tick >= a.pursuit {
                    a.pursuit = self.tick + b.tuning.pursuit_ticks;
                    // A kilometre search may need several slices. Reissuing
                    // move_to here discarded its frontier every pursuit
                    // period and the distant target could never be reached.
                    if self.units[id].motion.searching() && !target_changed {
                        continue;
                    }
                    let length = dx.mul(dx).add(dz.mul(dz)).sqrt().max(Fx::ONE);
                    // Aim for a firing position before the target hull, never its occupied centre.
                    let approach = length
                        .sub(Fx::from_ratio(range * b.tuning.standoff_milli, 1000))
                        .max(Fx::ZERO)
                        .div(length);
                    let dest = [
                        from[0].add(dx.mul(approach)).to_int_trunc(),
                        from[2].add(dz.mul(approach)).to_int_trunc(),
                    ];
                    let margin = range * (1000 - b.tuning.standoff_milli) / 1000;
                    let changed = target_changed
                        || self.units[id].motion.destination.is_none_or(|old| {
                            let x = i64::from(dest[0] - old[0]);
                            let z = i64::from(dest[1] - old[1]);
                            x * x + z * z > i64::from(margin) * i64::from(margin)
                        });
                    if changed {
                        let others: Vec<_> = self
                            .units
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| *i != id)
                            .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
                            .collect();
                        let u = &mut self.units[id];
                        u.motion
                            .move_to(&self.world, dest, &u.body, &others, &self.rubble);
                    }
                }
                continue;
            }
            self.units[id].motion.stop();
            let angle = fx::angle::atan2(dz.raw(), dx.raw());
            a.turret.step(angle, self.turret_limits);
            self.units[id].update_pose(a.turret.angle);
            if self.tick < a.next || !a.turret.aligned(angle, self.turret_limits) {
                continue;
            }
            let shell = kind == RangeUnitKind::Tank;
            if shell {
                let charge = *a
                    .charge
                    .get_or_insert(self.tick + b.shell_config.charge_ticks);
                if self.tick < charge
                    || b.waves.len() + b.rounds.iter().filter(|r| r.shell).count()
                        >= b.tuning.max_waves
                {
                    continue;
                }
            }
            let muzzles = weapons::muzzles(&self.units[id], a.turret.angle);
            if muzzles.is_empty() || b.rounds.len() + muzzles.len() > b.tuning.max_rounds {
                continue;
            }
            let speed = if shell {
                b.shell_config.shell_speed
            } else {
                b.bullet_config.shell_speed
            };
            for (barrel, muzzle) in muzzles
                .into_iter()
                .take(if shell { 1 } else { 2 })
                .enumerate()
            {
                let mut delta: [Fx; 3] = std::array::from_fn(|k| Fx::from_int(p[k]).sub(muzzle[k]));
                if !shell {
                    let serial = a.fired.wrapping_mul(2).wrapping_add(barrel as u32)
                        ^ (id as u32).wrapping_mul(65537);
                    delta = weapons::spread(
                        delta,
                        [
                            fx::angle::cos(a.turret.angle),
                            Fx::ZERO,
                            fx::angle::sin(a.turret.angle),
                        ],
                        serial,
                        stream.spread_milli,
                    );
                }
                let time = delta[0]
                    .mul(delta[0])
                    .add(delta[2].mul(delta[2]))
                    .sqrt()
                    .div(Fx::from_int(speed))
                    .max(Fx::from_ratio(1, crate::TICKS_PER_SECOND));
                let mut velocity = delta.map(|v| v.div(time));
                if shell {
                    velocity[1] = velocity[1].add(
                        Fx::from_int(b.shell_config.gravity)
                            .mul(time)
                            .mul(Fx::from_ratio(1, 2)),
                    );
                }
                b.rounds.push(Round {
                    owner: id,
                    shell,
                    pos: muzzle,
                    previous: muzzle,
                    velocity,
                    distance: Fx::from_int(range * 2),
                });
            }
            a.fired = a.fired.wrapping_add(1);
            a.charge = None;
            if shell {
                a.next = self.tick + b.tuning.tank_reload_ticks;
            } else {
                a.burst += stream.interval_ticks;
                a.next = self.tick
                    + if a.burst >= stream.burst_ticks {
                        a.burst = 0;
                        stream.reload_ticks
                    } else {
                        stream.interval_ticks
                    };
            }
        }
    }
    fn battle_flight(&mut self, b: &mut Battle) {
        let dt = Fx::from_ratio(1, crate::TICKS_PER_SECOND);
        let stream = self.streams.tuning.machine_gun.clone();
        let boxes: Vec<_> = self.units.iter().map(|u| u.bounds()).collect();
        for mut r in std::mem::take(&mut b.rounds) {
            r.previous = r.pos;
            if r.shell {
                r.velocity[1] = r.velocity[1].sub(Fx::from_int(b.shell_config.gravity).mul(dt));
            }
            let delta = r.velocity.map(|v| v.mul(dt));
            let from = r.pos.map(Fx::to_int_trunc);
            let to = std::array::from_fn::<_, 3, _>(|a| r.pos[a].add(delta[a]).to_int_trunc());
            // The sweep still samples at sub-voxel intervals and preserves
            // stable actor order. Empty portions of a round's flight need no
            // local body transforms (especially the paired machine guns).
            let near: Vec<_> = boxes
                .iter()
                .enumerate()
                .filter_map(|(id, (lo, hi))| {
                    (id != r.owner
                        && (0..3)
                            .all(|a| from[a].min(to[a]) <= hi[a] && from[a].max(to[a]) >= lo[a]))
                    .then_some(id)
                })
                .collect();
            let travel = delta.iter().fold(Fx::ZERO, |n, v| n.add(v.mul(*v))).sqrt();
            let steps = (delta
                .iter()
                .map(|v| v.abs().to_int_trunc())
                .max()
                .unwrap_or(0)
                + 1)
                * 2;
            let mut hit = None;
            let mut shield = None;
            let mut prev = r.pos.map(Fx::to_int_trunc);
            for i in 1..=steps {
                let p = std::array::from_fn(|k| {
                    r.pos[k]
                        .add(delta[k].mul(Fx::from_ratio(i, steps)))
                        .to_int_trunc()
                });
                if let Some(s) = self.dome_crossed(prev, p) {
                    shield = Some(s);
                    hit = Some(p);
                    break;
                }
                if material(&self.world, p) != 0
                    || near.iter().any(|&id| {
                        let (lo, hi) = boxes[id];
                        (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a]) && self.units[id].solid(p)
                    })
                {
                    hit = Some(p);
                    break;
                }
                prev = p;
            }
            let to = hit
                .map(|p| p.map(Fx::from_int))
                .unwrap_or_else(|| std::array::from_fn(|k| r.pos[k].add(delta[k])));
            if !r.shell {
                b.traces.push(weapons::Trace {
                    from: r.pos,
                    to,
                    hit: hit.is_some(),
                    serial: r.owner as u32,
                });
            }
            if let Some(at) = hit {
                if let Some(i) = shield {
                    let work = if r.shell {
                        b.shell_config.energy
                    } else {
                        stream.work
                    };
                    let drained = self.shields[i].charge.min(work);
                    self.shields[i].charge -= drained;
                    self.shields[i].hits += 1;
                    let collapsed = self.shields[i].charge == 0;
                    if collapsed {
                        self.shields[i].down = self.shield_tuning.overload_ticks;
                    }
                    self.shield_hits.push(ShieldHit {
                        shield: i,
                        at,
                        drained,
                        collapsed,
                    });
                } else if r.shell {
                    let old = self.matter.remaining_mj;
                    self.matter.begin(
                        b.shell_config.material_energy_j,
                        b.shell_config.fragment_box,
                    );
                    let energy = self.matter.remaining_mj;
                    self.matter.remaining_mj = old;
                    b.waves.push(Burst {
                        wave: Explosion::begin(at, &b.shell_config, &self.units),
                        energy,
                    });
                    self.heat.deposit(
                        &self.world,
                        &self.matter.economy.mats,
                        at,
                        b.shell_config.heat_units,
                        b.shell_config.heat_per_cell,
                    );
                    b.impacts.push(Impact { at, shell: true });
                } else {
                    self.stream_contact(
                        at,
                        &stream,
                        false,
                        self.units[r.owner].at,
                        &b.bullet_config,
                    );
                    b.impacts.push(Impact { at, shell: false });
                }
            } else {
                r.pos = to;
                r.distance = r.distance.sub(travel);
                let gravity = if r.shell { b.shell_config.gravity } else { 0 };
                if r.distance > Fx::ZERO
                    && super::in_flight_bounds(to, self.size, r.velocity[1], gravity)
                {
                    b.rounds.push(r);
                }
            }
        }
    }
    fn battle_waves(&mut self, b: &mut Battle, completed: &mut impl FnMut(&'static str)) {
        let n = b.waves.len().max(1);
        for w in &mut b.waves {
            if w.wave.capturing() {
                w.wave.capture(
                    &self.world,
                    &b.shell_config,
                    &self.units,
                    &self.surface_work,
                    b.shell_config.capture_cells_per_tick / n,
                );
                completed("battle_capture");
            }
            if !w.wave.capturing() {
                let old = self.matter.remaining_mj;
                self.matter.remaining_mj = w.energy;
                self.matter.fragment_size(b.shell_config.fragment_box);
                w.wave.step(
                    &mut self.world,
                    b.shell_config.wave_work_per_tick / n,
                    &mut self.units,
                    &mut self.surface_work,
                    &mut self.surface_changed,
                    &b.shell_config,
                    Some(&mut self.matter),
                );
                completed("battle_wave_step");
                w.energy = self.matter.remaining_mj;
                self.matter.remaining_mj = old;
                for u in &mut self.units {
                    if u.absorbed > 0 && u.status.alive {
                        u.motion.impulse(
                            &u.body,
                            w.wave.at,
                            u64::from(u.absorbed) * u64::from(b.shell_config.hull_impulse_milli)
                                / 1000,
                        );
                    }
                    u.absorbed = 0;
                }
            }
        }
        b.waves.retain(|w| w.wave.working());
    }
}

#[cfg(test)]
#[path = "range_battle_tests.rs"]
mod tests;
