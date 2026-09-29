//! Bounded skirmish on the range's ordinary, fully damageable unit roster.
//! Commands never exchange slots, matter containers or projectile ownership.
use super::ai::{self, Policy};
use super::*;
use fx::blob::{Blob, Scan};
use modding::{explosion::Delivery, range_battle::Tuning};

/// Weapon identity survives selection changes, flight and delayed wave work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Weapon {
    MachineGun = 0,
    Blast = 1,
    Kinetic = 2,
    Energy = 3,
    Annihilation = 4,
    Laser = 5,
}
impl Weapon {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::MachineGun,
            1 => Self::Blast,
            2 => Self::Kinetic,
            3 => Self::Energy,
            4 => Self::Annihilation,
            5 => Self::Laser,
            _ => return None,
        })
    }
    fn ammo(delivery: Delivery) -> Self {
        if delivery == Delivery::Kinetic {
            Self::Kinetic
        } else {
            Self::Blast
        }
    }
    pub fn energetic(self) -> bool {
        matches!(self, Self::Energy | Self::Annihilation)
    }
    pub fn bursts(self) -> bool {
        matches!(self, Self::Blast | Self::Energy | Self::Annihilation)
    }
}

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
    /// None selects ammunition by target; explicit choices belong to this actor.
    pub weapon: Option<Weapon>,
}
pub struct Round {
    pub owner: usize,
    pub weapon: Weapon,
    pub pos: [Fx; 3],
    pub previous: [Fx; 3],
    velocity: [Fx; 3],
    distance: Fx,
}
impl Round {
    /// A high-explosive shell: its impact starts a wave, and waves are
    /// bounded with the shells still in flight.
    fn bursts(&self) -> bool {
        self.weapon.bursts()
    }
}
pub struct Burst {
    weapon: Weapon,
    wave: Explosion,
    energy: u64,
}
#[derive(Clone, Copy)]
pub struct Impact {
    pub at: [i32; 3],
    pub weapon: Weapon,
}
pub struct Battle {
    pub phase: Phase,
    pub actors: Vec<Actor>,
    pub rounds: Vec<Round>,
    waves: Vec<Burst>,
    pub impacts: Vec<Impact>,
    pub traces: Vec<weapons::Trace>,
    pub lasers: Vec<weapons::Trace>,
    pub laser_config: ExplosionConfig,
    pub tuning: std::sync::Arc<Tuning>,
    pub shell_config: ExplosionConfig,
    /// The tank's armour-piercing round (ADR-456).
    pub kinetic_config: ExplosionConfig,
    bullet_config: ExplosionConfig,
    pub energy_config: ExplosionConfig,
    pub anni_config: ExplosionConfig,
    pub(super) ai: ai::Controller,
    /// Both teams use the same deterministic decision clock in spectator mode.
    pub autonomous: bool,
}
fn attack_range(tuning: &Tuning, kind: RangeUnitKind, weapon: Weapon, machine_gun: i32) -> i32 {
    if kind == RangeUnitKind::Artillery && weapon == Weapon::Blast {
        return tuning.artillery_range;
    }
    match weapon {
        Weapon::Blast => tuning.tank_range,
        Weapon::Kinetic => tuning.kinetic_range,
        Weapon::Energy => tuning.energy_range,
        Weapon::Annihilation => tuning.anni_range,
        Weapon::MachineGun => machine_gun,
        Weapon::Laser => motion::spider_tuning().laser.range,
    }
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
        // The same tank's power drives either round.
        let mut kinetic_config =
            ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap();
        kinetic_config.energy = kinetic_config.energy * tuning.tank_power_milli / 1000;
        kinetic_config.material_energy_j = (u64::from(kinetic_config.material_energy_j)
            * u64::from(tuning.tank_power_milli)
            / 1000) as u32;
        kinetic_config.unit_ablation_resistance = resistance;
        let mut bullet_config =
            ExplosionConfig::from_ron(include_str!("../../../data/machine-gun.ron")).unwrap();
        bullet_config.unit_ablation_resistance = resistance;
        let energetic = |text| {
            let mut c = ExplosionConfig::from_ron(text).expect("energy weapon tuning");
            c.energy = c.energy * tuning.tank_power_milli / 1000;
            c.material_energy_j =
                (u64::from(c.material_energy_j) * u64::from(tuning.tank_power_milli) / 1000) as u32;
            c
        };
        let energy_config = energetic(include_str!("../../../data/energy-cannon.ron"));
        let anni_config = energetic(include_str!("../../../data/anni-cannon.ron"));
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
                    weapon: match u.kind {
                        RangeUnitKind::Tank => Some(Weapon::Kinetic),
                        RangeUnitKind::Artillery => Some(Weapon::Blast),
                        _ => None,
                    },
                })
                .collect(),
            rounds: Vec::new(),
            waves: Vec::new(),
            impacts: Vec::new(),
            traces: Vec::new(),
            lasers: Vec::new(),
            laser_config: ExplosionConfig::from_ron(include_str!("../../../data/laser.ron"))
                .expect("laser"),
            tuning,
            shell_config,
            kinetic_config,
            bullet_config,
            energy_config,
            anni_config,
            ai: ai::Controller::new(),
            autonomous: false,
        }
    }
    pub fn config(&self, weapon: Weapon) -> &ExplosionConfig {
        match weapon {
            Weapon::Blast => &self.shell_config,
            Weapon::Kinetic => &self.kinetic_config,
            Weapon::Energy => &self.energy_config,
            Weapon::Annihilation => &self.anni_config,
            Weapon::MachineGun => &self.bullet_config,
            Weapon::Laser => &self.laser_config,
        }
    }
    pub(super) fn working(&self) -> bool {
        !self.waves.is_empty()
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
            // Earlier snapshots wrote a bool here: 0 and 1 keep their
            // meaning, 2 is the armour-piercing round (ADR-456).
            b.u8(r.weapon as u8);
            for v in r.pos.into_iter().chain(r.velocity) {
                b.fx(v);
            }
            b.fx(r.distance);
        }
        b.usize(self.waves.len());
        for w in &self.waves {
            b.u8(w.weapon as u8);
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
            let weapon = Weapon::from_code(s.u8())?;
            if weapon == Weapon::Laser || (version < 43 && weapon.energetic()) {
                return None;
            }
            let pos = std::array::from_fn(|_| s.fx());
            let velocity = std::array::from_fn(|_| s.fx());
            let distance = s.fx();
            if owner >= units.len()
                || !super::restored_round(pos, velocity, size)
                || distance <= Fx::ZERO
                || distance > Fx::from_int(10000)
            {
                return None;
            }
            self.rounds.push(Round {
                owner,
                weapon,
                pos,
                previous: pos,
                velocity,
                distance,
            });
        }
        let n = s.usize();
        if n.saturating_add(self.rounds.iter().filter(|r| r.bursts()).count())
            > self.tuning.max_waves
        {
            return None;
        }
        for _ in 0..n {
            let weapon = if version >= 43 {
                Weapon::from_code(s.u8())?
            } else {
                Weapon::Blast
            };
            if !weapon.bursts() {
                return None;
            }
            let energy = s.u64();
            if energy > u64::from(self.config(weapon).material_energy_j) * 1000 {
                return None;
            }
            self.waves.push(Burst {
                weapon,
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
pub(super) fn aim_point(u: &RangeUnit) -> [i32; 3] {
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
    /// Effective loadout, shared by aiming, HUD and range indicators.
    pub fn battle_weapon(&self, id: usize) -> Option<Weapon> {
        let u = self.units.get(id)?;
        let a = self.battle.actors.get(id)?;
        if u.kind == RangeUnitKind::Base {
            return None;
        }
        if u.kind == RangeUnitKind::Spider {
            return Some(Weapon::Laser);
        }
        if u.kind == RangeUnitKind::Strider {
            return Some(Weapon::MachineGun);
        }
        if u.kind == RangeUnitKind::Artillery {
            return Some(a.weapon.unwrap_or(Weapon::Blast));
        }
        if u.kind == RangeUnitKind::TurretT1 {
            return Some(Weapon::Kinetic);
        }
        if u.kind == RangeUnitKind::TurretT2 {
            return Some(Weapon::Energy);
        }
        Some(a.weapon.unwrap_or_else(|| {
            let ammo = self.battle.tuning.tank_ammo;
            Weapon::ammo(match (a.order, a.target.and_then(|t| self.units.get(t))) {
                (Order::Ground(_), _) | (_, None) => ammo.ground,
                (_, Some(t))
                    if matches!(
                        t.kind,
                        RangeUnitKind::Tank | RangeUnitKind::Artillery | RangeUnitKind::Base
                    ) =>
                {
                    ammo.tank
                }
                _ => ammo.strider,
            })
        }))
    }
    /// Connected barrels in the actor's current posed weapon, for muzzle effects.
    pub fn battle_charge(&self, id: usize) -> Option<u32> {
        self.battle.actors.get(id)?.charge
    }
    pub fn battle_muzzles(&self, id: usize) -> Vec<[Fx; 3]> {
        match (self.units.get(id), self.battle.actors.get(id)) {
            (Some(unit), Some(actor)) => weapons::muzzles(unit, actor.turret.angle),
            _ => Vec::new(),
        }
    }
    pub fn battle_attack_range(&self, id: usize) -> Option<i32> {
        Some(attack_range(
            &self.battle.tuning,
            self.units.get(id)?.kind,
            self.battle_weapon(id)?,
            self.streams.tuning.machine_gun.range,
        ))
    }

    /// Switching ammunition keeps the current order and reload deadline.
    /// Rounds already in flight retain their original configuration.
    pub fn set_battle_weapon(&mut self, id: usize, weapon: Option<Weapon>) -> bool {
        if !self.battle_mode()
            || !self.allied(id)
            || self.units.get(id).is_none_or(|u| {
                !u.status.alive || !matches!(u.kind, RangeUnitKind::Tank | RangeUnitKind::Artillery)
            })
            || matches!(weapon, Some(Weapon::MachineGun | Weapon::Laser))
        {
            return false;
        }
        let Some(actor) = self.battle.actors.get_mut(id) else {
            return false;
        };
        if actor.weapon != weapon {
            actor.weapon = weapon;
            actor.charge = None;
        }
        true
    }
    pub fn enemy_ai_tuning(&self) -> &modding::battle_ai::Tuning {
        &self.battle.ai.tuning
    }
    /// Enable autonomous orders for both teams before the fight starts.
    pub fn set_ai_vs_ai(&mut self, enabled: bool) -> bool {
        if !self.battle_mode() || self.battle.phase != Phase::Setup {
            return false;
        }
        self.battle.autonomous = enabled;
        true
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
                    mobile: unit.status.engine && unit.motion.can_drive(unit.mobility),
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
            controlled_allied: false,
        }
        .decide(&observation);
        let allied_rules = battle.autonomous.then(|| {
            ai::Rules {
                tuning: &battle.ai.tuning,
                controlled_allied: true,
            }
            .decide(&observation)
        });
        let supplied = battle.ai.pending.take();
        for fallback in rules {
            let decision = supplied
                .as_ref()
                .and_then(|plan| plan.iter().find(|d| d.actor == fallback.actor))
                .copied()
                .unwrap_or(fallback);
            if !self.apply_ai_decision(battle, decision, false) {
                self.apply_ai_decision(battle, fallback, false);
            }
        }
        if let Some(rules) = allied_rules {
            for decision in rules {
                self.apply_ai_decision(battle, decision, true);
            }
        }
        battle.ai.next_tick = self.tick.saturating_add(battle.ai.tuning.decision_ticks);
    }
    fn apply_ai_decision(
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
            if !u
                .motion
                .move_to(&self.world, p, &u.body, &others, &self.rubble)
            {
                return false;
            }
        } else if order == Order::Hold {
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
            if !u.motion.move_to(&self.world, p, &u.body, &others, &rubble) {
                return false;
            }
        } else if order == Order::Hold {
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
    /// Accept a formation immediately. Route searches begin during motion
    /// ticks, under the existing global start budget, instead of multiplying
    /// one input-frame stall by the selected unit count.
    pub fn battle_group_move(
        &mut self,
        destinations: &[(usize, [i32; 2])],
        fallback: [i32; 2],
    ) -> usize {
        if !self.battle_mode() || self.battle.phase != Phase::Fighting {
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
                actor.charge = None;
                actor.burst = 0;
                actor.pursuit = 0;
                accepted += 1;
            }
        }
        accepted
    }
    /// A firing position for `id` on the arc through `natural` around
    /// `target`, a `spacing` from the points its allies hold — where they
    /// drive, or where they stand (ADR-467). Allies lined up with the
    /// target would all drive for the same point before it and jostle
    /// there. The position along the line is kept when it is free;
    /// otherwise the arc is walked a chord of `spacing` at a time, each
    /// count of chords on the positive turn first, up to six each way;
    /// with none free, the position along the line.
    fn firing_point(
        &self,
        id: usize,
        target: [i32; 2],
        natural: [i32; 2],
        spacing: i32,
    ) -> [i32; 2] {
        let side = self.allied(id);
        let held: Vec<[i32; 2]> = self
            .units
            .iter()
            .enumerate()
            .filter(|&(j, u)| j != id && u.status.alive && self.allied(j) == side)
            .map(|(_, u)| {
                u.motion.destination.unwrap_or([
                    u.motion.pos[0].to_int_trunc(),
                    u.motion.pos[2].to_int_trunc(),
                ])
            })
            .collect();
        let free = |p: [i32; 2]| {
            held.iter().all(|h| {
                let (x, z) = (i64::from(p[0] - h[0]), i64::from(p[1] - h[1]));
                x * x + z * z >= i64::from(spacing) * i64::from(spacing)
            })
        };
        let arm = [natural[0] - target[0], natural[1] - target[1]].map(Fx::from_int);
        let radius = arm[0].mul(arm[0]).add(arm[1].mul(arm[1])).sqrt();
        if spacing == 0 || free(natural) || radius < Fx::from_int(spacing) {
            return natural;
        }
        // The angle of a chord of `spacing`, in binary turns: its arc in
        // radians is about the spacing over the radius.
        let chord = Fx::from_int(spacing)
            .div(radius)
            .mul(Fx::from_ratio(fx::angle::FULL, 1))
            .div(Fx::from_ratio(314_159_265, 50_000_000))
            .to_int_trunc();
        let far = [self.size[0] as i32, self.size[2] as i32];
        for k in 1..=6 {
            for sign in [1, -1] {
                let turn = sign * k * chord;
                let (c, s) = (fx::angle::cos(turn), fx::angle::sin(turn));
                let p = [
                    Fx::from_int(target[0])
                        .add(arm[0].mul(c))
                        .sub(arm[1].mul(s))
                        .to_int_trunc(),
                    Fx::from_int(target[1])
                        .add(arm[0].mul(s))
                        .add(arm[1].mul(c))
                        .to_int_trunc(),
                ];
                if (0..2).all(|a| (0..far[a]).contains(&p[a])) && free(p) {
                    return p;
                }
            }
        }
        natural
    }
    /// Try short lateral firing positions when a hill hides a target that is
    /// already inside weapon range. This runs only on the pursuit cadence.
    fn visible_flank(
        &self,
        id: usize,
        target: [i32; 3],
        natural: [i32; 2],
        muzzle_y: i32,
        pursuit_ticks: u32,
        spacing: i32,
    ) -> [i32; 2] {
        let here = [
            self.units[id].motion.pos[0].to_int_trunc(),
            self.units[id].motion.pos[2].to_int_trunc(),
        ];
        let vx = Fx::from_int(target[0] - here[0]);
        let vz = Fx::from_int(target[2] - here[1]);
        let length = vx.mul(vx).add(vz.mul(vz)).sqrt().max(Fx::ONE);
        let side = [Fx::ZERO.sub(vz.div(length)), vx.div(length)];
        let start = if (self.tick / pursuit_ticks.max(1) + id as u32) & 1 == 0 {
            1
        } else {
            -1
        };
        let muzzle_above_floor = muzzle_y - motion::floor(&self.world, here[0], here[1]);
        let free = |p: [i32; 2]| {
            self.units.iter().enumerate().all(|(other, u)| {
                if other == id || !u.status.alive || self.allied(other) != self.allied(id) {
                    return true;
                }
                let held = u.motion.destination.unwrap_or([
                    u.motion.pos[0].to_int_trunc(),
                    u.motion.pos[2].to_int_trunc(),
                ]);
                let dx = i64::from(p[0] - held[0]);
                let dz = i64::from(p[1] - held[1]);
                dx * dx + dz * dz >= i64::from(spacing) * i64::from(spacing)
            })
        };
        let mut fallback = natural;
        for distance in [180, 320, 500] {
            for sign in [start, -start] {
                let offset = Fx::from_int(distance * sign);
                let candidate = [
                    Fx::from_int(natural[0])
                        .add(side[0].mul(offset))
                        .to_int_trunc(),
                    Fx::from_int(natural[1])
                        .add(side[1].mul(offset))
                        .to_int_trunc(),
                ];
                if candidate[0] < 16
                    || candidate[1] < 16
                    || candidate[0] >= self.size[0] as i32 - 16
                    || candidate[1] >= self.size[2] as i32 - 16
                    || !free(candidate)
                {
                    continue;
                }
                if fallback == natural {
                    fallback = candidate;
                }
                let y = motion::floor(&self.world, candidate[0], candidate[1]) + muzzle_above_floor;
                if voxel::los::line_of_sight(
                    &self.world,
                    (candidate[0], y, candidate[1]),
                    (target[0], target[1], target[2]),
                )
                .clear
                {
                    return candidate;
                }
            }
        }
        fallback
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
    /// Health of each command base, when both sides have one. A destroyed
    /// base remains in the layout and reports zero health.
    pub fn battle_base_integrity(&self) -> Option<[i32; 2]> {
        let layout = self.layout.as_ref()?;
        let mut health = [None, None];
        for (i, def) in layout.units.iter().enumerate() {
            if def.kind == RangeUnitKind::Base {
                let side = usize::from(!def.allied);
                let unit = self.units.get(i)?;
                let value = if unit.status.alive {
                    unit.integrity
                        .core
                        .mul(Fx::from_int(100))
                        .to_int_trunc()
                        .clamp(1, 100)
                } else {
                    0
                };
                health[side] = Some(health[side].unwrap_or(0).max(value));
            }
        }
        Some([health[0]?, health[1]?])
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
            lasers: Vec::new(),
            laser_config: self.battle.laser_config.clone(),
            tuning: self.battle.tuning.clone(),
            shell_config: self.battle.shell_config.clone(),
            kinetic_config: self.battle.kinetic_config.clone(),
            bullet_config: self.battle.bullet_config.clone(),
            energy_config: self.battle.energy_config.clone(),
            anni_config: self.battle.anni_config.clone(),
            ai: self.battle.ai.clone(),
            autonomous: self.battle.autonomous,
        };
        let mut battle = std::mem::replace(&mut self.battle, empty);
        battle.impacts.clear();
        battle.traces.clear();
        battle.lasers.clear();
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
        if battle.phase == Phase::Fighting {
            // A base is the objective itself: an unrelated round still in
            // flight must not postpone the outcome indefinitely.
            if let Some([own, enemy]) = self.battle_base_integrity() {
                // A disabled attacker and two distant garrisons cannot ever
                // reach either base. Keep nearby stationary gunfights alive,
                // but do not leave a whole-map AI battle in Fighting forever.
                let mobile_gun = self.units.iter().any(|u| {
                    u.status.alive
                        && u.status.engine
                        && u.status.weapon
                        && u.motion.can_drive(u.mobility)
                        && !u.kind.emplacement()
                });
                let stationary_target_in_range = !mobile_gun && {
                    self.units.iter().enumerate().any(|(id, u)| {
                        let weapon = match u.kind {
                            RangeUnitKind::Base => None,
                            RangeUnitKind::Spider => Some(Weapon::Laser),
                            RangeUnitKind::Strider => Some(Weapon::MachineGun),
                            RangeUnitKind::TurretT1 => Some(Weapon::Kinetic),
                            RangeUnitKind::TurretT2 => Some(Weapon::Energy),
                            RangeUnitKind::Artillery => {
                                Some(battle.actors[id].weapon.unwrap_or(Weapon::Blast))
                            }
                            // An immobilized tank may still switch ammunition.
                            RangeUnitKind::Tank => None,
                        };
                        let range = weapon.map_or_else(
                            || {
                                [
                                    Weapon::Blast,
                                    Weapon::Kinetic,
                                    Weapon::Energy,
                                    Weapon::Annihilation,
                                ]
                                .into_iter()
                                .map(|w| {
                                    attack_range(
                                        &battle.tuning,
                                        u.kind,
                                        w,
                                        self.streams.tuning.machine_gun.range,
                                    )
                                })
                                .max()
                                .unwrap_or(0)
                            },
                            |w| {
                                attack_range(
                                    &battle.tuning,
                                    u.kind,
                                    w,
                                    self.streams.tuning.machine_gun.range,
                                )
                            },
                        ) + 128;
                        u.status.alive
                            && u.status.weapon
                            && battle.actors[id].target.is_some_and(|other_id| {
                                self.units.get(other_id).is_some_and(|other| {
                                    other.status.alive
                                        && self.allied(id) != self.allied(other_id)
                                        && {
                                            let dx = i64::from(u.at[0] - other.at[0]);
                                            let dz = i64::from(u.at[2] - other.at[2]);
                                            dx * dx + dz * dz <= i64::from(range) * i64::from(range)
                                        }
                                })
                            })
                    })
                };
                battle.phase = match (own > 0, enemy > 0) {
                    (false, false) => Phase::Draw,
                    (false, true) => Phase::Defeat,
                    (true, false) => Phase::Victory,
                    (true, true)
                        if battle.rounds.is_empty()
                            && battle.waves.is_empty()
                            && !mobile_gun
                            && !stationary_target_in_range =>
                    {
                        Phase::Draw
                    }
                    (true, true) => Phase::Fighting,
                };
            } else if battle.rounds.is_empty() && battle.waves.is_empty() {
                battle.phase = match self.battle_counts() {
                    [0, 0] => Phase::Draw,
                    [0, _] => Phase::Defeat,
                    [_, 0] => Phase::Victory,
                    _ => Phase::Fighting,
                };
            }
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
                || (a.order == Order::Move
                    && self.units[id].motion.destination.is_none()
                    && !b.autonomous)
            {
                a.order = Order::Auto;
            }
            if a.order == Order::Hold {
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
            // A tank loads for what it aims at (ADR-456).
            let weapon = if kind == RangeUnitKind::Tank {
                let ammo = b.tuning.tank_ammo;
                let choice = match (a.order, target) {
                    (Order::Ground(_), _) | (_, None) => ammo.ground,
                    (_, Some(t))
                        if matches!(
                            self.units[t].kind,
                            RangeUnitKind::Tank | RangeUnitKind::Artillery | RangeUnitKind::Base
                        ) =>
                    {
                        ammo.tank
                    }
                    _ => ammo.strider,
                };
                a.weapon.unwrap_or(Weapon::ammo(choice))
            } else if kind == RangeUnitKind::Artillery {
                a.weapon.unwrap_or(Weapon::Blast)
            } else if kind == RangeUnitKind::TurretT1 {
                Weapon::Kinetic
            } else if kind == RangeUnitKind::TurretT2 {
                Weapon::Energy
            } else if kind == RangeUnitKind::Spider {
                Weapon::Laser
            } else {
                Weapon::MachineGun
            };
            let range = attack_range(&b.tuning, kind, weapon, stream.range);
            // Borrow individual config fields independently of the mutable actor.
            let config = match weapon {
                Weapon::Blast => &b.shell_config,
                Weapon::Kinetic => &b.kinetic_config,
                Weapon::Energy => &b.energy_config,
                Weapon::Annihilation => &b.anni_config,
                Weapon::MachineGun => &b.bullet_config,
                Weapon::Laser => &b.laser_config,
            };
            let mut p = if let Order::Ground(p) = a.order {
                p
            } else if let Some(other) = target {
                aim_point(&self.units[other])
            } else {
                continue;
            };
            if let Some(other) =
                target.filter(|_| weapon != Weapon::Laser && !matches!(a.order, Order::Ground(_)))
            {
                let u = &self.units[other];
                let time = distance(other).sqrt().div(Fx::from_int(config.shell_speed));
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
            // Traverse independently of the hull and before entering range,
            // so a moving unit does not wait to begin slewing its turret.
            let angle = fx::angle::atan2(dz.raw(), dx.raw());
            a.turret.step(angle, self.turret_limits);
            self.units[id].update_pose(a.turret.angle);
            let in_range =
                dx.mul(dx).add(dz.mul(dz)) <= Fx::from_int(range).mul(Fx::from_int(range));
            // Bullets and penetrators cannot help by firing into a hill.
            // Check the actual muzzle ray only after entering range; the
            // normal projectile sweep remains authoritative for a hit.
            let blocked_direct = in_range
                && matches!(weapon, Weapon::MachineGun | Weapon::Kinetic)
                && !kind.emplacement()
                && !matches!(a.order, Order::Ground(_))
                && weapons::muzzles(&self.units[id], a.turret.angle)
                    .first()
                    .is_some_and(|muzzle| {
                        !voxel::los::line_of_sight(
                            &self.world,
                            (
                                muzzle[0].to_int_trunc(),
                                muzzle[1].to_int_trunc(),
                                muzzle[2].to_int_trunc(),
                            ),
                            (p[0], p[1], p[2]),
                        )
                        .clear
                    });
            if !in_range || blocked_direct {
                a.charge = None;
                if kind.emplacement() {
                    continue;
                }
                // A move order owns its route. Track and fire at hostiles that
                // enter range, but never turn the move into an attack pursuit.
                if a.order == Order::Move {
                    continue;
                }
                if self.tick >= a.pursuit {
                    a.pursuit = self.tick + b.tuning.pursuit_ticks;
                    // A kilometre search may need several slices. Reissuing
                    // move_to here discarded its frontier every pursuit
                    // period and the distant target could never be reached.
                    if self.units[id].motion.searching() && !target_changed {
                        continue;
                    }
                    if b.autonomous && self.units[id].motion.recovering() && !target_changed {
                        continue;
                    }
                    let needs_detour = self.units[id].motion.blocked;
                    let length = dx.mul(dx).add(dz.mul(dz)).sqrt().max(Fx::ONE);
                    let road_goal = if b.autonomous
                        && target.is_some_and(|other| self.units[other].kind == RangeUnitKind::Base)
                        && length > Fx::from_int(range * 2)
                    {
                        self.layout.as_ref().and_then(|layout| {
                            ai::road_waypoint(
                                &layout.roads,
                                [from[0].to_int_trunc(), from[2].to_int_trunc()],
                                [p[0], p[2]],
                            )
                        })
                    } else {
                        None
                    };
                    // Follow a short road leg to arrival. Reissuing an order
                    // for a moving siege unit would throw away its frontier.
                    if road_goal.is_some()
                        && !target_changed
                        && !needs_detour
                        && self.units[id].motion.destination.is_some_and(|old| {
                            let here = [from[0].to_int_trunc(), from[2].to_int_trunc()];
                            let x = i64::from(here[0] - old[0]);
                            let z = i64::from(here[1] - old[1]);
                            x * x + z * z > 160 * 160
                        })
                    {
                        continue;
                    }
                    // Aim for a firing position before the target hull, never its occupied centre.
                    let standoff = if blocked_direct {
                        (range / 3).min(200)
                    } else {
                        range * b.tuning.standoff_milli / 1000
                    };
                    let approach = length.sub(Fx::from_int(standoff)).max(Fx::ZERO).div(length);
                    let dest = [
                        from[0].add(dx.mul(approach)).to_int_trunc(),
                        from[2].add(dz.mul(approach)).to_int_trunc(),
                    ];
                    let dest = self.firing_point(id, [p[0], p[2]], dest, b.tuning.firing_spacing);
                    let dest = if blocked_direct || needs_detour && road_goal.is_none() {
                        let muzzle_y = weapons::muzzles(&self.units[id], a.turret.angle)
                            .first()
                            .map_or(from[1].to_int_trunc(), |m| m[1].to_int_trunc());
                        self.visible_flank(
                            id,
                            p,
                            dest,
                            muzzle_y,
                            b.tuning.pursuit_ticks,
                            b.tuning.firing_spacing,
                        )
                    } else {
                        dest
                    };
                    // Keep a usable firing position when our own detour changes
                    // the approach angle. Comparing the new radial point with
                    // the old one replanned even against a stationary target.
                    // A moving target still invalidates it on leaving its reach.
                    let dest = road_goal.unwrap_or(dest);
                    let changed = if road_goal.is_some() {
                        target_changed
                            || needs_detour
                            || self.units[id].motion.destination != Some(dest)
                    } else {
                        target_changed
                            || needs_detour
                            || (blocked_direct && self.units[id].motion.destination.is_none())
                            || self.units[id].motion.destination.is_none_or(|old| {
                                let x = i64::from(p[0]) - i64::from(old[0]);
                                let z = i64::from(p[2]) - i64::from(old[1]);
                                let reach = if blocked_direct { 200 } else { range };
                                x * x + z * z > i64::from(reach) * i64::from(reach)
                            })
                    };
                    if changed {
                        let others: Vec<_> = self
                            .units
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| *i != id)
                            .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
                            .collect();
                        let u = &mut self.units[id];
                        // Under way, the hull drives on while the new
                        // route is searched (ADR-462).
                        u.motion
                            .retarget(&self.world, dest, &u.body, &others, &self.rubble);
                    }
                }
                continue;
            }
            if self.tick < a.next || !a.turret.aligned(angle, self.turret_limits) {
                continue;
            }
            if weapon == Weapon::Laser {
                let tuning = motion::spider_tuning();
                let ready = *a.charge.get_or_insert(self.tick + tuning.charge_ticks);
                if self.tick < ready {
                    continue;
                }
                let muzzles = weapons::muzzles(&self.units[id], a.turret.angle);
                if let Some(from) = muzzles.first().copied() {
                    b.lasers
                        .push(self.laser_ray(from, p, Some(id), &tuning.laser, config));
                    a.fired = a.fired.wrapping_add(1);
                    a.burst += 1;
                    if a.burst >= tuning.laser.burst_ticks {
                        a.burst = 0;
                        a.charge = None;
                        a.next = self.tick + tuning.laser.reload_ticks;
                    } else {
                        a.next = self.tick + tuning.laser.interval_ticks;
                    }
                }
                continue;
            }
            let shell = matches!(
                kind,
                RangeUnitKind::Tank
                    | RangeUnitKind::Artillery
                    | RangeUnitKind::TurretT1
                    | RangeUnitKind::TurretT2
            );
            if shell {
                let charge = *a.charge.get_or_insert(self.tick + config.charge_ticks);
                if self.tick < charge
                    || (weapon.bursts()
                        && b.waves.len() + b.rounds.iter().filter(|r| r.bursts()).count()
                            >= b.tuning.max_waves)
                {
                    continue;
                }
            }
            let muzzles = weapons::muzzles(&self.units[id], a.turret.angle);
            if muzzles.is_empty() || b.rounds.len() + muzzles.len() > b.tuning.max_rounds {
                continue;
            }
            let speed = config.shell_speed;
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
                let ballistic = shell && !weapon.energetic();
                let mut velocity = if weapon == Weapon::Kinetic {
                    weapons::kinetic_velocity(muzzle, p, config)
                } else if ballistic {
                    delta.map(|v| v.div(time))
                } else {
                    weapons::flight_velocity(delta, speed)
                };
                if ballistic && weapon != Weapon::Kinetic {
                    velocity[1] = velocity[1].add(
                        Fx::from_int(config.gravity)
                            .mul(time)
                            .mul(Fx::from_ratio(1, 2)),
                    );
                }
                b.rounds.push(Round {
                    owner: id,
                    weapon,
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
                let remaining = stream.burst_ticks.saturating_sub(a.burst);
                a.burst += stream.interval_ticks;
                a.next = self.tick
                    + if a.burst >= stream.burst_ticks {
                        a.burst = 0;
                        remaining.saturating_sub(1) + stream.reload_ticks
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
            let config = b.config(r.weapon).clone();
            let gravity = if r.weapon == Weapon::MachineGun || r.weapon.energetic() {
                0
            } else {
                config.gravity
            };
            r.velocity[1] = r.velocity[1].sub(Fx::from_int(gravity).mul(dt));
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
            let mut last_free = r.pos;
            let mut contact = r.pos;
            for i in 1..=steps {
                let q: [Fx; 3] =
                    std::array::from_fn(|k| r.pos[k].add(delta[k].mul(Fx::from_ratio(i, steps))));
                let p = q.map(Fx::to_int_trunc);
                if let Some(s) = self.dome_crossed(prev, p) {
                    shield = Some(s);
                    hit = Some(p);
                    contact = q;
                    break;
                }
                if material(&self.world, p) != 0
                    || near.iter().any(|&id| {
                        let (lo, hi) = boxes[id];
                        (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a]) && self.units[id].solid(p)
                    })
                {
                    hit = Some(p);
                    contact = q;
                    break;
                }
                prev = p;
                last_free = q;
            }
            let to = hit
                .map(|p| p.map(Fx::from_int))
                .unwrap_or_else(|| std::array::from_fn(|k| r.pos[k].add(delta[k])));
            if r.weapon == Weapon::MachineGun {
                b.traces.push(weapons::Trace {
                    from: r.pos,
                    to,
                    hit: hit.is_some(),
                    serial: r.owner as u32,
                });
            }
            if let Some(at) = hit {
                if let Some(i) = shield {
                    let work = if r.weapon == Weapon::MachineGun {
                        stream.work
                    } else {
                        config.energy
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
                } else if r.weapon == Weapon::Kinetic {
                    // A solid round glances off or spends its work along
                    // its line, as the single gun's (ADR-454, ADR-456).
                    b.impacts.push(Impact {
                        at,
                        weapon: r.weapon,
                    });
                    let mut velocity = r.velocity;
                    if self.ricochet(at, prev, &mut velocity, &b.kinetic_config) {
                        r.velocity = velocity;
                        r.pos = last_free;
                        r.distance = r.distance.sub(travel);
                        if r.distance > Fx::ZERO {
                            b.rounds.push(r);
                        }
                    } else {
                        self.kinetic_hit_at(at, contact, r.velocity, &b.kinetic_config);
                    }
                } else if r.weapon.bursts() {
                    let old = self.matter.remaining_mj;
                    self.matter
                        .begin(config.material_energy_j, config.fragment_box);
                    let energy = self.matter.remaining_mj;
                    self.matter.remaining_mj = old;
                    b.waves.push(Burst {
                        weapon: r.weapon,
                        wave: Explosion::begin(at, &config, &self.units),
                        energy,
                    });
                    self.heat.deposit(
                        &self.world,
                        &self.matter.economy.mats,
                        at,
                        config.heat_units,
                        config.heat_per_cell,
                    );
                    b.impacts.push(Impact {
                        at,
                        weapon: r.weapon,
                    });
                } else {
                    self.stream_contact(
                        at,
                        &stream,
                        false,
                        self.units[r.owner].at,
                        &b.bullet_config,
                    );
                    b.impacts.push(Impact {
                        at,
                        weapon: r.weapon,
                    });
                }
            } else {
                r.pos = to;
                r.distance = r.distance.sub(travel);
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
            let config = match w.weapon {
                Weapon::Energy => &b.energy_config,
                Weapon::Annihilation => &b.anni_config,
                _ => &b.shell_config,
            };
            if w.wave.capturing() {
                w.wave.capture(
                    &self.world,
                    config,
                    &self.units,
                    &self.surface_work,
                    config.capture_cells_per_tick / n,
                );
                completed("battle_capture");
            }
            if !w.wave.capturing() {
                let old = self.matter.remaining_mj;
                self.matter.remaining_mj = w.energy;
                self.matter.fragment_size(config.fragment_box);
                let before: Vec<_> = self
                    .units
                    .iter()
                    .map(|u| (u.removed.len(), u.seg_removed.len()))
                    .collect();
                w.wave.step(
                    &mut self.world,
                    config.wave_work_per_tick / n,
                    &mut self.units,
                    &mut self.surface_work,
                    &mut self.surface_changed,
                    config,
                    Some(&mut self.matter),
                );
                if config.material_energy_j == 0 {
                    for (id, (u, (root, seg))) in self.units.iter().zip(before).enumerate() {
                        self.matter.annihilate_body(
                            id,
                            u.removed.len() - root + u.seg_removed.len() - seg,
                        );
                    }
                }
                completed("battle_wave_step");
                w.energy = self.matter.remaining_mj;
                self.matter.remaining_mj = old;
                for u in &mut self.units {
                    if u.absorbed > 0 && u.status.alive {
                        u.motion.impulse(
                            &u.body,
                            w.wave.at,
                            u64::from(u.absorbed) * u64::from(config.hull_impulse_milli) / 1000,
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
