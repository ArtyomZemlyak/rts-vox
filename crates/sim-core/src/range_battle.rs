//! Bounded skirmish on the range's ordinary, fully damageable unit roster.
//! Commands never exchange slots, matter containers or projectile ownership.
use super::ai::{self, Policy};
use super::*;
use fx::blob::{Blob, Scan};
use modding::{explosion::Delivery, range_battle::Tuning};

#[cfg(test)]
#[path = "range_artillery_tests.rs"]
mod artillery_tests;

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
    Canister = 6,
}
impl Weapon {
    pub const COUNT: usize = 7;
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::MachineGun,
            1 => Self::Blast,
            2 => Self::Kinetic,
            3 => Self::Energy,
            4 => Self::Annihilation,
            5 => Self::Laser,
            6 => Self::Canister,
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
        matches!(
            self,
            Self::Blast | Self::Energy | Self::Annihilation | Self::Canister
        )
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
/// Opt-in observations of actual fire gates, never input to simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FireGate {
    NoTarget,
    Disabled,
    Hold,
    Approach,
    Reload,
    Aiming,
    Charging,
    WaveCapacity,
    RoundCapacity,
    SolidCapacity,
    NoMuzzle,
    Beam,
    Fired,
    BurstQueue,
}
impl FireGate {
    pub const COUNT: usize = 14;
    pub const NAMES: [&'static str; Self::COUNT] = [
        "no_target",
        "disabled",
        "hold",
        "approach",
        "reload",
        "aiming",
        "charging",
        "wave_capacity",
        "round_capacity",
        "solid_capacity",
        "no_muzzle",
        "beam_path",
        "fired",
        "burst_queue",
    ];
}
fn record_gate(trace: &mut Option<Vec<FireGate>>, id: usize, gate: FireGate) {
    if let Some(trace) = trace {
        trace[id] = gate;
    }
}
/// Read-only progress of each canonical wave; heap pops are not contact counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaveProgress {
    pub weapon: Weapon,
    pub at: [i32; 3],
    pub capturing: bool,
    pub frontier_finished: bool,
    pub heap_examined: usize,
    pub pending: usize,
    pub queued_continuations: usize,
    pub contact_cursor: usize,
    pub body_cursor: usize,
    pub energy_mj: u64,
}
pub struct Actor {
    pub turret: crate::turret::Traverse,
    pub elevation: crate::turret::Traverse,
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
// Only geometry/readiness are frozen in the unit pass. Trajectories are built
// after admission, so a saturated pool does not repeatedly construct pellets.
struct ProjectileLaunch {
    owner: usize,
    kind: RangeUnitKind,
    weapon: Weapon,
    point: [i32; 3],
    range: i32,
    angle: i32,
    shell: bool,
    count: usize,
    muzzles: Vec<[Fx; 3]>,
    artillery_velocity: Option<[Fx; 3]>,
    ready: u32,
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
/// Divide a phase quota without losing its remainder. `turn` is the offset
/// in the tick's rotated service order, not an index into reordered state.
fn wave_share(budget: usize, count: usize, turn: usize) -> usize {
    debug_assert!(count > 0 && turn < count);
    budget / count + usize::from(turn < budget % count)
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
    pub canister_config: ExplosionConfig,
    pub energy_config: ExplosionConfig,
    pub anni_config: ExplosionConfig,
    pub(super) ai: ai::Controller,
    /// Both teams use the same deterministic decision clock in spectator mode.
    pub autonomous: bool,
    // Derived diagnostics: absent in hashes/snapshots, never read by combat.
    fire_trace: Option<Vec<FireGate>>,
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
        Weapon::Canister => tuning.canister.range,
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
        let mut canister_config = energetic(include_str!("../../../data/canister-shell.ron"));
        canister_config.unit_ablation_resistance = resistance;
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
                    elevation: crate::turret::Traverse::new(0),
                    fired: 0,
                    weapon: match u.kind {
                        RangeUnitKind::Tank => Some(Weapon::Kinetic),
                        RangeUnitKind::Artillery => Some(Weapon::Blast),
                        RangeUnitKind::ArtilleryT2 | RangeUnitKind::ArtilleryTurretT2 => {
                            Some(Weapon::Canister)
                        }
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
            canister_config,
            energy_config,
            anni_config,
            ai: ai::Controller::new(),
            autonomous: false,
            fire_trace: None,
        }
    }
    pub fn trace_fire(&mut self, enabled: bool) {
        self.fire_trace = enabled.then(|| vec![FireGate::NoTarget; self.actors.len()]);
    }
    pub fn fire_gate(&self, id: usize) -> Option<FireGate> {
        if self.phase != Phase::Fighting {
            return None;
        }
        self.fire_trace.as_ref()?.get(id).copied()
    }
    pub fn wave_progress(&self) -> impl Iterator<Item = WaveProgress> + '_ {
        self.waves.iter().map(|w| WaveProgress {
            weapon: w.weapon,
            at: w.wave.at,
            capturing: w.wave.capturing(),
            frontier_finished: w.wave.wave.finished(),
            heap_examined: w.wave.wave.examined,
            pending: w.wave.pending.len(),
            queued_continuations: w.wave.contact_resumes.len(),
            contact_cursor: w.wave.contact_cursor,
            body_cursor: w.wave.body_cursor,
            energy_mj: w.energy,
        })
    }
    /// Layout validation has already checked compatibility with each unit kind.
    pub(super) fn apply_loadouts(&mut self, defs: &[modding::range::RangeUnitDef]) {
        use modding::range::RangeLoadout;
        for (actor, def) in self.actors.iter_mut().zip(defs) {
            if let Some(weapon) = def.weapon {
                actor.weapon = Some(match weapon {
                    RangeLoadout::Blast => Weapon::Blast,
                    RangeLoadout::Kinetic => Weapon::Kinetic,
                    RangeLoadout::Energy => Weapon::Energy,
                    RangeLoadout::Annihilation => Weapon::Annihilation,
                });
            }
        }
    }
    pub fn config(&self, weapon: Weapon) -> &ExplosionConfig {
        match weapon {
            Weapon::Blast => &self.shell_config,
            Weapon::Kinetic => &self.kinetic_config,
            Weapon::Canister => &self.canister_config,
            Weapon::Energy => &self.energy_config,
            Weapon::Annihilation => &self.anni_config,
            Weapon::MachineGun => &self.bullet_config,
            Weapon::Laser => &self.laser_config,
        }
    }
    pub(super) fn shot_config(&self, weapon: Weapon, kind: RangeUnitKind) -> ExplosionConfig {
        let mut config = self.config(weapon).clone();
        if kind == RangeUnitKind::TurretT2 && weapon == Weapon::Energy {
            config.energy = (u64::from(config.energy)
                * u64::from(self.tuning.turret_t2_power_milli)
                / 1000) as u32;
        }
        config
    }
    /// Read-only diagnostic counts: capturing resistance, applying pressure.
    pub fn wave_counts(&self) -> [usize; 2] {
        let capturing = self.waves.iter().filter(|w| w.wave.capturing()).count();
        [capturing, self.waves.len() - capturing]
    }
    pub(super) fn working(&self) -> bool {
        !self.waves.is_empty()
    }
    pub(super) fn save_wave_contacts(&self, b: &mut Blob) {
        for burst in &self.waves {
            burst.wave.save_contacts(b);
        }
    }
    pub(super) fn has_contact_resumes(&self) -> bool {
        self.waves
            .iter()
            .any(|w| !w.wave.contact_resumes.is_empty())
    }
    pub(super) fn save_contact_resumes(&self, b: &mut Blob) {
        for w in &self.waves {
            w.wave.save_resumes(b);
        }
    }
    pub(super) fn load_contact_resumes(&mut self, s: &mut Scan, units: &[RangeUnit]) -> Option<()> {
        for w in &mut self.waves {
            w.wave.load_resumes(s, units)?;
        }
        Some(())
    }
    #[cfg(test)]
    pub(super) fn load_wave_contacts(&mut self, s: &mut Scan, units: &[RangeUnit]) -> Option<()> {
        self.load_wave_contacts_deferred(s, units, false)
    }
    pub(super) fn load_wave_contacts_deferred(
        &mut self,
        s: &mut Scan,
        units: &[RangeUnit],
        defer: bool,
    ) -> Option<()> {
        for burst in &mut self.waves {
            burst.wave.load_contacts(s, units, defer)?;
        }
        Some(())
    }
    pub(super) fn save_laser_tracking(&self, b: &mut Blob) {
        for a in &self.actors {
            a.elevation.save(b);
        }
        for w in &self.waves {
            for p in &w.wave.turret_pitches {
                b.i32(*p);
            }
        }
    }
    pub(super) fn load_laser_tracking(&mut self, s: &mut Scan, units: &[RangeUnit]) -> Option<()> {
        let tuning = motion::spider_tuning();
        for (a, u) in self.actors.iter_mut().zip(units) {
            let limits = crate::turret::Limits::of(
                tuning
                    .laser_tracking
                    .filter(|_| u.kind == RangeUnitKind::Spider)
                    .map_or(
                        modding::turret::Turret::from_ron(include_str!("../../../data/turret.ron"))
                            .expect("turret tuning"),
                        |t| t.elevation,
                    ),
                crate::TICKS_PER_SECOND,
            );
            a.elevation = crate::turret::Traverse::load(s, limits)?;
            if (!artillery::elevates(u.kind) && a.elevation != crate::turret::Traverse::new(0))
                || (artillery::elevates(u.kind)
                    && fx::angle::diff(a.elevation.angle, 0) != u.turret_pitch)
            {
                return None;
            }
        }
        for w in &mut self.waves {
            for (p, u) in w.wave.turret_pitches.iter_mut().zip(units) {
                *p = s.i32();
                if !artillery::valid_pitch(*p, u.kind) {
                    return None;
                }
            }
            w.wave.validate_contacts(units)?;
        }
        s.ok().then_some(())
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
            if weapon == Weapon::Laser
                || (version < 43 && weapon.energetic())
                || (version < 51 && weapon == Weapon::Canister)
            {
                return None;
            }
            let pos = std::array::from_fn(|_| s.fx());
            let velocity = std::array::from_fn(|_| s.fx());
            let distance = s.fx();
            if owner >= units.len()
                || (weapon == Weapon::Canister && !units[owner].kind.canister())
                || !super::restored_round(pos, velocity, size)
                || distance <= Fx::ZERO
                || distance
                    > Fx::from_int(if weapon == Weapon::Canister {
                        100000
                    } else {
                        10000
                    })
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
/// Cancellation consumes the current pulse; retargeting never calls this.
fn stop_spider_beam(a: &mut Actor, kind: RangeUnitKind, tick: u32) {
    if kind == RangeUnitKind::Spider && a.burst > 0 {
        a.next = a
            .next
            .max(tick.saturating_add(motion::spider_tuning().laser.reload_ticks));
        a.burst = 0;
        a.charge = None;
    }
}
/// Smooth opening aim, derived from the already saved active beam sample count.
/// The ray still owns contact/cover/heat; this only supplies its current aim.
fn spider_laser_aim(
    world: &World,
    from: [Fx; 3],
    target: [i32; 3],
    samples: u32,
    tuning: &modding::spider::Tuning,
) -> [i32; 3] {
    let Some(sweep) = tuning.laser_sweep else {
        return target;
    };
    let elapsed = samples.saturating_mul(tuning.laser.interval_ticks);
    if elapsed >= sweep.ticks {
        return target;
    }
    let dx = Fx::from_int(target[0]).sub(from[0]);
    let dz = Fx::from_int(target[2]).sub(from[2]);
    let distance_sq = dx.mul(dx).add(dz.mul(dz));
    if distance_sq <= Fx::ONE {
        return target;
    }
    // Never place the opening point behind the muzzle when the enemy is close.
    let pull = Fx::from_int(sweep.lead_units)
        .div(distance_sq.sqrt())
        .min(Fx::from_ratio(1, 2));
    let x = Fx::from_int(target[0]).sub(dx.mul(pull)).to_int_trunc();
    let z = Fx::from_int(target[2]).sub(dz.mul(pull)).to_int_trunc();
    // This exact ceiling is refreshed by edits in the same simulation tick.
    let y = world.column_ceiling(x as u32, z as u32).saturating_sub(1) as i32;
    let start = [x, y, z];
    let t = Fx::from_ratio(elapsed as i32, sweep.ticks as i32);
    let blend = t.mul(t).mul(Fx::from_int(3).sub(t.mul(Fx::from_int(2))));
    std::array::from_fn(|a| {
        Fx::from_int(start[a])
            .add(Fx::from_int(target[a] - start[a]).mul(blend))
            .to_int_trunc()
    })
}

/// A unit attack needs a useful firing position, rather than burning a forest
/// indefinitely. Meltable cover may still be breached in place; point orders
/// deliberately keep their direct ray. Walk in firing direction, since a
/// symmetric visibility query would classify cover starting at the enemy.
fn laser_requires_flank(
    world: &World,
    mats: &modding::Registry,
    from: [Fx; 3],
    target: [i32; 3],
) -> bool {
    let p = from.map(Fx::to_int_trunc);
    let mut blocked = false;
    voxel::los::walk(
        (p[0], p[1], p[2]),
        (target[0], target[1], target[2]),
        |x, y, z| {
            if x >= 0 && y >= 0 && z >= 0 && world.solid(x as u32, y as u32, z as u32) {
                let mat = world.material(x as u32, y as u32, z as u32);
                blocked = mats.get(mat).is_none_or(|m| m.melt == 0);
                false
            } else {
                true
            }
        },
    );
    blocked
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
    /// Setup accepts explicit movement without running combat or AI.
    pub fn battle_movement_active(&self) -> bool {
        matches!(self.battle.phase, Phase::Setup | Phase::Fighting)
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
        if u.kind.canister() {
            return Some(Weapon::Canister);
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
                        RangeUnitKind::Tank
                            | RangeUnitKind::Artillery
                            | RangeUnitKind::ArtilleryT2
                            | RangeUnitKind::Base
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
            || matches!(
                weapon,
                Some(Weapon::MachineGun | Weapon::Laser | Weapon::Canister)
            )
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
        if self.units.len() >= 16 && self.navigation_workers > 1 && self.navigation_pool.is_none() {
            self.set_navigation_workers(self.navigation_workers);
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
    /// Drive the prepared roster without starting AI, combat or victory checks.
    pub fn start_free_control(&mut self) -> Result<(), &'static str> {
        if !self.battle_mode() || self.battle.phase != Phase::Setup {
            return Err("Свободное управление доступно до начала боя");
        }
        self.control();
        self.fire_tick = u32::MAX;
        self.patrols_running = true;
        for u in &mut self.units {
            u.motion.route.clear();
            u.motion.stop();
        }
        if self.units.len() >= 16 && self.navigation_workers > 1 && self.navigation_pool.is_none() {
            self.set_navigation_workers(self.navigation_workers);
        }
        Ok(())
    }
    pub fn battle_order(&mut self, id: usize, order: Order, destination: Option<[i32; 2]>) -> bool {
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
            if !u.motion.move_to(&self.world, p, &u.body, &others, &rubble) {
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
    /// Accept a formation immediately. Route searches begin during motion
    /// ticks, under the existing global start budget, instead of multiplying
    /// one input-frame stall by the selected unit count.
    pub fn battle_group_move(
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
    /// Project the current muzzle height onto a candidate firing position.
    /// The same terrain test validates a held detour and selects a new flank.
    fn firing_line_clear(
        &self,
        id: usize,
        target: [i32; 3],
        point: [i32; 2],
        muzzle_y: i32,
    ) -> bool {
        let here = self.units[id].motion.pos;
        let above =
            muzzle_y - motion::floor(&self.world, here[0].to_int_trunc(), here[2].to_int_trunc());
        let y = motion::floor(&self.world, point[0], point[1]) + above;
        voxel::los::line_of_sight(
            &self.world,
            (point[0], y, point[1]),
            (target[0], target[1], target[2]),
        )
        .clear
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
                if self.firing_line_clear(id, target, candidate, muzzle_y) {
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
        self.battle_tick_observed(|_| {}, &|_, _| {});
    }
    pub(super) fn battle_tick_observed(
        &mut self,
        mut completed: impl FnMut(&'static str),
        physics: &(impl Fn(usize, &'static str) + Sync),
    ) {
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
            canister_config: self.battle.canister_config.clone(),
            energy_config: self.battle.energy_config.clone(),
            anni_config: self.battle.anni_config.clone(),
            ai: self.battle.ai.clone(),
            autonomous: self.battle.autonomous,
            fire_trace: None,
        };
        let mut battle = std::mem::replace(&mut self.battle, empty);
        battle.impacts.clear();
        battle.traces.clear();
        battle.lasers.clear();
        // A surviving siege head carries its current pulse to the next target.
        for (id, actor) in battle.actors.iter_mut().enumerate() {
            if !self.units[id].status.alive || !self.units[id].status.weapon {
                stop_spider_beam(actor, self.units[id].kind, self.tick);
                actor.target = None;
                actor.charge = None;
                actor.burst = 0;
            } else if actor
                .target
                .is_some_and(|target| !self.units[target].status.alive)
            {
                actor.target = None;
                if self.units[id].kind != RangeUnitKind::Spider || actor.burst == 0 {
                    actor.charge = None;
                    actor.burst = 0;
                }
                if matches!(actor.order, Order::Attack(_)) {
                    actor.order = Order::Auto;
                }
            }
        }
        if battle.phase == Phase::Fighting {
            self.battle_ai_tick(&mut battle);
            completed("battle_ai");
            self.battle_orders(&mut battle, &mut completed, physics);
        }
        completed("battle_orders");
        // New live beam deposits must reserve admission before wave/crater work.
        self.reserve_thermal_fragments();
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
                            RangeUnitKind::ArtilleryT2 | RangeUnitKind::ArtilleryTurretT2 => {
                                Some(Weapon::Canister)
                            }
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
    fn battle_orders(
        &mut self,
        b: &mut Battle,
        mut completed: impl FnMut(&'static str),
        physics: &(impl Fn(usize, &'static str) + Sync),
    ) {
        let stream = self.streams.tuning.machine_gun.clone();
        if let Some(trace) = &mut b.fire_trace {
            trace.fill(FireGate::NoTarget);
        }
        let mut bursts = Vec::new();
        for id in 0..self.units.len() {
            if !self.units[id].status.alive || !self.units[id].status.weapon {
                record_gate(&mut b.fire_trace, id, FireGate::Disabled);
                b.actors[id].charge = None;
                b.actors[id].burst = 0;
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
                record_gate(&mut b.fire_trace, id, FireGate::Hold);
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
                if kind != RangeUnitKind::Spider || a.burst == 0 {
                    a.charge = None;
                }
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
                            RangeUnitKind::Tank
                                | RangeUnitKind::Artillery
                                | RangeUnitKind::ArtilleryT2
                                | RangeUnitKind::Base
                        ) =>
                    {
                        ammo.tank
                    }
                    _ => ammo.strider,
                };
                a.weapon.unwrap_or(Weapon::ammo(choice))
            } else if kind.canister() {
                Weapon::Canister
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
                Weapon::Canister => &b.canister_config,
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
                stop_spider_beam(a, kind, self.tick);
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
            let artillery_aim = kind
                .artillery()
                .then(|| {
                    artillery::solve(
                        &self.units[id],
                        p,
                        weapon,
                        config,
                        if weapon == Weapon::Canister {
                            b.tuning.canister.flight_time_milli
                        } else {
                            1000
                        },
                    )
                })
                .flatten();
            let angle = artillery_aim
                .as_ref()
                .map_or_else(|| fx::angle::atan2(dz.raw(), dx.raw()), |aim| aim.yaw);
            let tracking = (kind == RangeUnitKind::Spider)
                .then(|| motion::spider_tuning().laser_tracking)
                .flatten();
            let yaw_limits = tracking.map_or(self.turret_limits, |t| {
                crate::turret::Limits::of(t.yaw, crate::TICKS_PER_SECOND)
            });
            a.turret.step(angle, yaw_limits);
            if let Some(aim) = &artillery_aim {
                a.elevation.step(aim.pitch, self.turret_limits);
                let pitch = fx::angle::diff(a.elevation.angle, 0);
                let bounded = pitch.clamp(-fx::angle::QUARTER, fx::angle::QUARTER);
                if pitch != bounded {
                    a.elevation.rate = 0;
                    a.elevation.angle = fx::angle::norm(bounded);
                }
                self.units[id].turret_pitch = bounded;
            }
            completed("orders_misc");
            self.units[id].update_pose(a.turret.angle);
            completed("orders_pose");
            let in_range =
                dx.mul(dx).add(dz.mul(dz)) <= Fx::from_int(range).mul(Fx::from_int(range));
            // Direct rounds need a clear line. Siege lasers can melt hard
            // cover, but should change position instead of indefinitely
            // burning foliage or firing into terrain that cannot melt.
            // Explicit point orders retain their ordinary physical contacts.
            completed("orders_misc");
            let blocked_direct = in_range
                && matches!(weapon, Weapon::MachineGun | Weapon::Kinetic | Weapon::Laser)
                && !kind.emplacement()
                && !matches!(a.order, Order::Ground(_))
                && weapons::muzzles(&self.units[id], a.turret.angle)
                    .first()
                    .is_some_and(|muzzle| {
                        if weapon == Weapon::Laser {
                            return laser_requires_flank(
                                &self.world,
                                &self.matter.economy.mats,
                                *muzzle,
                                p,
                            );
                        }
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
            completed("orders_los");
            if !in_range || blocked_direct {
                record_gate(&mut b.fire_trace, id, FireGate::Approach);
                stop_spider_beam(a, kind, self.tick);
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
                    let standoff = if blocked_direct && weapon != Weapon::Laser {
                        (range / 3).min(200)
                    } else {
                        range * b.tuning.standoff_milli / 1000
                    };
                    let approach = length.sub(Fx::from_int(standoff)).max(Fx::ZERO).div(length);
                    let dest = [
                        from[0].add(dx.mul(approach)).to_int_trunc(),
                        from[2].add(dz.mul(approach)).to_int_trunc(),
                    ];
                    completed("orders_approach");
                    let dest = self.firing_point(id, [p[0], p[2]], dest, b.tuning.firing_spacing);
                    completed("orders_firing_point");
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
                    completed("orders_flank");
                    // Keep a usable firing position when our own detour changes
                    // the approach angle. Comparing the new radial point with
                    // the old one replanned even against a stationary target.
                    // A moving target still invalidates it on leaving its reach.
                    let dest = road_goal.unwrap_or(dest);
                    // A checked local prefix is a committed leg of the same
                    // firing approach. Our changing angle must not replace it
                    // with a nearby flank before its endpoint is reached.
                    let motion = &self.units[id].motion;
                    let following_prefix = blocked_direct
                        && motion.partial_path
                        && motion.cursor < motion.path.len()
                        && motion.destination.is_some_and(|old| {
                            let x = i64::from(p[0]) - i64::from(old[0]);
                            let z = i64::from(p[2]) - i64::from(old[1]);
                            x * x + z * z <= i64::from(range) * i64::from(range)
                        });
                    let changed = if road_goal.is_some() {
                        target_changed
                            || needs_detour
                            || self.units[id].motion.destination != Some(dest)
                    } else if following_prefix && !target_changed && !needs_detour {
                        false
                    } else {
                        target_changed
                            || needs_detour
                            || (blocked_direct && self.units[id].motion.destination.is_none())
                            || self.units[id].motion.destination.is_none_or(|old| {
                                let x = i64::from(p[0]) - i64::from(old[0]);
                                let z = i64::from(p[2]) - i64::from(old[1]);
                                // The fallback may still have no firing line.
                                // Only a visible lateral point can use the full
                                // weapon reach rather than the radial standoff.
                                let reach = if blocked_direct && weapon != Weapon::Laser {
                                    let muzzle_y =
                                        weapons::muzzles(&self.units[id], a.turret.angle)
                                            .first()
                                            .map_or(from[1].to_int_trunc(), |m| {
                                                m[1].to_int_trunc()
                                            });
                                    if self.firing_line_clear(id, p, old, muzzle_y) {
                                        range
                                    } else {
                                        200
                                    }
                                } else {
                                    range
                                };
                                x * x + z * z > i64::from(reach) * i64::from(reach)
                            })
                    };
                    // Reissuing the identical firing point cannot improve a
                    // live route or its sight line. It only restarts work from
                    // a moved origin and can send the hull back into a pocket.
                    // A block or a different target still needs a fresh search.
                    if changed
                        && (needs_detour
                            || target_changed
                            || self.units[id].motion.destination != Some(dest))
                    {
                        completed("orders_retarget_setup");
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
                        u.motion.retarget_observed(
                            &self.world,
                            dest,
                            &u.body,
                            &others,
                            &self.rubble,
                            |phase| {
                                completed(match phase {
                                    "plan_coarse" => "orders_retarget_coarse",
                                    "plan_target" => "orders_retarget_target",
                                    "plan_prepare" => "orders_retarget_prepare",
                                    "plan_direct" => "orders_retarget_direct",
                                    "plan_fine" => "orders_retarget_fine",
                                    _ => unreachable!("unknown pursuit route phase"),
                                });
                            },
                        );
                        completed("orders_retarget");
                    }
                }
                continue;
            }
            let laser_aim = if weapon == Weapon::Laser {
                let tuning = motion::spider_tuning();
                let muzzle = weapons::muzzles(&self.units[id], a.turret.angle)
                    .first()
                    .copied()
                    .unwrap_or(from);
                let desired = if matches!(a.order, Order::Ground(_)) {
                    p
                } else {
                    spider_laser_aim(&self.world, muzzle, p, a.burst, tuning)
                };
                if let Some(t) = tracking {
                    let frame = crate::damage::turned(
                        &self.units[id].pose,
                        a.turret.angle,
                        self.units[id].motion.drive.head,
                    );
                    let delta = phys::V3::new(
                        Fx::from_int(desired[0]).sub(muzzle[0]),
                        Fx::from_int(desired[1]).sub(muzzle[1]),
                        Fx::from_int(desired[2]).sub(muzzle[2]),
                    );
                    let y = delta.dot(frame.y);
                    let horizontal = delta.len_sq().sub(y.mul(y)).max(Fx::ONE).sqrt();
                    let limit = t.elevation_limit_degrees * fx::angle::FULL / 360;
                    let pitch = fx::angle::diff(fx::angle::atan2(y.raw(), horizontal.raw()), 0)
                        .clamp(-limit, limit);
                    a.elevation.step(
                        pitch,
                        crate::turret::Limits::of(t.elevation, crate::TICKS_PER_SECOND),
                    );
                    let bounded = fx::angle::diff(a.elevation.angle, 0).clamp(-limit, limit);
                    if bounded != fx::angle::diff(a.elevation.angle, 0) {
                        a.elevation.rate = 0;
                    }
                    a.elevation.angle = fx::angle::norm(bounded);
                    self.units[id].turret_pitch = fx::angle::diff(a.elevation.angle, 0);
                }
                desired
            } else {
                p
            };
            if self.tick < a.next {
                record_gate(&mut b.fire_trace, id, FireGate::Reload);
                continue;
            }
            if tracking.is_none()
                && (!a.turret.aligned(angle, yaw_limits)
                    || artillery_aim
                        .as_ref()
                        .is_some_and(|aim| !a.elevation.aligned(aim.pitch, self.turret_limits)))
            {
                record_gate(&mut b.fire_trace, id, FireGate::Aiming);
                continue;
            }
            if weapon == Weapon::Laser {
                record_gate(&mut b.fire_trace, id, FireGate::Beam);
                let tuning = motion::spider_tuning();
                if let Some(t) = tracking {
                    if a.burst == 0
                        && fx::angle::diff(angle, a.turret.angle).abs()
                            > (i64::from(t.fire_millidegrees) * i64::from(fx::angle::FULL) / 360000)
                                as i32
                    {
                        continue;
                    }
                }
                let ready = *a.charge.get_or_insert(self.tick + tuning.charge_ticks);
                if self.tick < ready {
                    continue;
                }
                let muzzles = weapons::muzzles(&self.units[id], a.turret.angle);
                if let Some(from) = muzzles.first().copied() {
                    completed("orders_misc");
                    let p = if tracking.is_some() {
                        let f =
                            self.units[id].root_frame(usize::from(self.units[id].body.turret_y));
                        let end = phys::V3::new(from[0], from[1], from[2])
                            .add(f.x.scale(Fx::from_int(tuning.laser.range)));
                        [end.x, end.y, end.z].map(Fx::to_int_trunc)
                    } else {
                        laser_aim
                    };
                    completed("orders_laser_sweep");
                    let trace = self.laser_ray(
                        from,
                        p,
                        Some(id),
                        &tuning.laser,
                        config,
                        (&mut completed, physics),
                    );
                    completed("orders_laser");
                    b.lasers.push(trace);
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
                    | RangeUnitKind::ArtilleryT2
                    | RangeUnitKind::ArtilleryTurretT2
                    | RangeUnitKind::TurretT1
                    | RangeUnitKind::TurretT2
            );
            let count = if weapon == Weapon::Canister {
                b.tuning.canister.pellets
            } else if shell {
                1
            } else {
                weapons::muzzles(&self.units[id], a.turret.angle)
                    .len()
                    .min(2)
            };
            if shell {
                let charge = *a.charge.get_or_insert(self.tick + config.charge_ticks);
                if self.tick < charge {
                    record_gate(&mut b.fire_trace, id, FireGate::Charging);
                    continue;
                }
            }
            let muzzles = weapons::muzzles(&self.units[id], a.turret.angle);
            let pending_full = matches!(weapon, Weapon::Kinetic | Weapon::MachineGun)
                && !self.solid_flight_room(&b.rounds, count);
            if muzzles.is_empty() {
                record_gate(&mut b.fire_trace, id, FireGate::NoMuzzle);
                continue;
            }
            if !weapon.bursts() && b.rounds.len() + count > b.tuning.max_rounds {
                record_gate(&mut b.fire_trace, id, FireGate::RoundCapacity);
                continue;
            }
            if pending_full {
                record_gate(&mut b.fire_trace, id, FireGate::SolidCapacity);
                continue;
            }
            let shot = ProjectileLaunch {
                owner: id,
                kind,
                weapon,
                point: p,
                range,
                angle: a.turret.angle,
                shell,
                count,
                artillery_velocity: kind.artillery().then(|| {
                    artillery::along_barrel(
                        &self.units[id],
                        artillery::velocity(
                            muzzles[0],
                            p,
                            weapon,
                            config,
                            if weapon == Weapon::Canister {
                                b.tuning.canister.flight_time_milli
                            } else {
                                1000
                            },
                        ),
                    )
                }),
                muzzles,
                ready: a.charge.unwrap_or(self.tick),
            };
            if weapon.bursts() {
                bursts.push(shot);
            } else {
                Self::launch_projectiles(self.tick, b, shot, &stream);
            }
        }
        // Existing charge deadlines are FIFO tickets, with an explicit owner
        // tie. Eligibility is rebuilt from live orders/aim/range every tick.
        bursts.sort_unstable_by_key(|shot| (shot.ready, shot.owner));
        let mut reserved = b.waves.len() + b.rounds.iter().filter(|r| r.bursts()).count();
        let mut blocked = false;
        for shot in bursts {
            let gate = if blocked {
                Some(FireGate::BurstQueue)
            } else if reserved + shot.count > b.tuning.max_waves {
                Some(FireGate::WaveCapacity)
            } else if b.rounds.len() + shot.count > b.tuning.max_rounds {
                Some(FireGate::RoundCapacity)
            } else {
                None
            };
            if let Some(gate) = gate {
                record_gate(&mut b.fire_trace, shot.owner, gate);
                // Do not refill freed slots with younger small bursts while an
                // older full salvo accumulates room. Solids and beams continue.
                blocked = true;
            } else {
                reserved += shot.count;
                Self::launch_projectiles(self.tick, b, shot, &stream);
            }
        }
        completed("orders_projectile_commit");
    }
    fn launch_projectiles(
        tick: u32,
        b: &mut Battle,
        shot: ProjectileLaunch,
        stream: &modding::range_weapon::Stream,
    ) {
        let ProjectileLaunch {
            owner: id,
            kind,
            weapon,
            point: p,
            range,
            angle: turret_angle,
            shell,
            count,
            muzzles,
            artillery_velocity,
            ..
        } = shot;
        let config = match weapon {
            Weapon::Blast => &b.shell_config,
            Weapon::Kinetic => &b.kinetic_config,
            Weapon::Canister => &b.canister_config,
            Weapon::Energy => &b.energy_config,
            Weapon::Annihilation => &b.anni_config,
            Weapon::MachineGun => &b.bullet_config,
            Weapon::Laser => unreachable!("beam is not a projectile"),
        };
        let a = &mut b.actors[id];
        record_gate(&mut b.fire_trace, id, FireGate::Fired);
        let speed = config.shell_speed;
        for barrel in 0..count {
            let muzzle = muzzles[if shell { 0 } else { barrel }];
            let mut delta: [Fx; 3] = std::array::from_fn(|k| Fx::from_int(p[k]).sub(muzzle[k]));
            if weapon == Weapon::Canister && barrel > 0 {
                // One central round and a rotated ring. A finite authored
                // footprint spreads the impacts themselves, rather than
                // sending every pellet into the same frontal armour voxel.
                let angle = (barrel - 1) as i32 * fx::angle::FULL / (count - 1) as i32;
                let angle = angle
                    .wrapping_add(turret_angle)
                    .wrapping_add(i32::from((a.fired as u16).wrapping_mul(7919)));
                let radius = Fx::from_int(b.tuning.canister.footprint_radius);
                delta[0] = delta[0].add(fx::angle::cos(angle).mul(radius));
                delta[2] = delta[2].add(fx::angle::sin(angle).mul(radius));
            }
            if !shell {
                let serial = a.fired.wrapping_mul(2).wrapping_add(barrel as u32)
                    ^ (id as u32).wrapping_mul(65537);
                delta = weapons::spread(
                    delta,
                    [
                        fx::angle::cos(turret_angle),
                        Fx::ZERO,
                        fx::angle::sin(turret_angle),
                    ],
                    serial,
                    stream.spread_milli,
                );
            }
            let mut time = delta[0]
                .mul(delta[0])
                .add(delta[2].mul(delta[2]))
                .sqrt()
                .div(Fx::from_int(speed))
                .max(Fx::from_ratio(1, crate::TICKS_PER_SECOND));
            if weapon == Weapon::Canister {
                time = time.mul(Fx::from_ratio(b.tuning.canister.flight_time_milli, 1000));
            }
            let ballistic = shell && !weapon.energetic();
            let mut velocity = if let Some(central) = artillery_velocity {
                if weapon == Weapon::Canister && barrel > 0 {
                    // Each offset impact has its own flight time and lift.
                    // Adding offsets to a single central parabola would
                    // scatter contacts when the ground/target has a height.
                    let mut v = delta.map(|d| d.div(time));
                    v[1] = v[1].add(
                        Fx::from_int(config.gravity)
                            .mul(time.add(Fx::from_ratio(1, crate::TICKS_PER_SECOND)))
                            .mul(Fx::from_ratio(1, 2)),
                    );
                    v
                } else {
                    central
                }
            } else if weapon == Weapon::Kinetic {
                weapons::kinetic_velocity(muzzle, p, config)
            } else if ballistic {
                delta.map(|v| v.div(time))
            } else {
                weapons::flight_velocity(delta, speed)
            };
            if weapon == Weapon::Canister && (artillery_velocity.is_none() || barrel > 0) {
                velocity = weapons::spread(
                    velocity,
                    [
                        fx::angle::cos(turret_angle),
                        Fx::ZERO,
                        fx::angle::sin(turret_angle),
                    ],
                    a.fired.wrapping_mul(257).wrapping_add(barrel as u32)
                        ^ (id as u32).wrapping_mul(65537),
                    b.tuning.canister.spread_milli,
                );
            }
            if ballistic && weapon != Weapon::Kinetic && artillery_velocity.is_none() {
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
                distance: if weapon == Weapon::Canister {
                    // A lofted arc travels farther than its horizontal range.
                    Fx::from_int(range * 2)
                        .add(Fx::from_int(config.gravity).mul(time).mul(time))
                        .min(Fx::from_int(100000))
                } else {
                    Fx::from_int(range * 2)
                },
            });
        }
        a.fired = a.fired.wrapping_add(1);
        a.charge = None;
        if shell {
            a.next = tick
                + if weapon == Weapon::Canister {
                    b.tuning.canister.reload_ticks
                } else if kind == RangeUnitKind::TurretT2 {
                    b.tuning
                        .turret_t2_reload_ticks
                        .unwrap_or(b.tuning.tank_reload_ticks)
                } else {
                    b.tuning.tank_reload_ticks
                };
        } else {
            let remaining = stream.burst_ticks.saturating_sub(a.burst);
            a.burst += stream.interval_ticks;
            a.next = tick
                + if a.burst >= stream.burst_ticks {
                    a.burst = 0;
                    remaining.saturating_sub(1) + stream.reload_ticks
                } else {
                    stream.interval_ticks
                };
        }
    }
    fn battle_flight(&mut self, b: &mut Battle) {
        let dt = Fx::from_ratio(1, crate::TICKS_PER_SECOND);
        let stream = self.streams.tuning.machine_gun.clone();
        let boxes: Vec<_> = self.units.iter().map(|u| u.bounds()).collect();
        for mut r in std::mem::take(&mut b.rounds) {
            r.previous = r.pos;
            // Stable owner kind survives its destruction; already flying rounds
            // retain their emplacement identity, including after restore.
            let config = b.shot_config(r.weapon, self.units[r.owner].kind);
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
                    if self.ricochet(at, prev, &mut velocity, &config) {
                        r.velocity = velocity;
                        r.pos = last_free;
                        r.distance = r.distance.sub(travel);
                        if r.distance > Fx::ZERO {
                            b.rounds.push(r);
                        }
                    } else {
                        self.kinetic_hit_at(at, contact, r.velocity, &config);
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
        // A growing old crater must not consume every newly freed fragment
        // slot before a newer hit reaches its hull. Rotate service, not the
        // canonical wave array: snapshots and stable wave identity keep their
        // order, and the saved tick determines the next turn after loading.
        let first = self.tick as usize % n;
        // No physics or claim confirmation runs between these atomic batches.
        // Component commits consume exactly their reservations; terrain claims
        // keep one slot per conditional edit. Recount at the next phase/tick.
        let mut room = None;
        for offset in 0..b.waves.len() {
            let w = &mut b.waves[(first + offset) % n];
            let config = match w.weapon {
                Weapon::Energy => &b.energy_config,
                Weapon::Annihilation => &b.anni_config,
                Weapon::Canister => &b.canister_config,
                _ => &b.shell_config,
            };
            if w.wave.capturing() {
                w.wave.capture(
                    &self.world,
                    config,
                    &self.units,
                    &self.surface_work,
                    wave_share(config.capture_cells_per_tick, n, offset),
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
                let room = room.get_or_insert_with(|| self.matter.claim_room());
                w.wave.step_with_room::<true>(
                    &mut self.world,
                    wave_share(config.wave_work_per_tick, n, offset),
                    &mut self.units,
                    &mut self.surface_work,
                    &mut self.surface_changed,
                    config,
                    Some(&mut self.matter),
                    Some(room),
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
#[path = "range_wave_admission_tests.rs"]
mod wave_admission_tests;

#[cfg(test)]
#[path = "range_wave_budget_tests.rs"]
mod wave_budget_tests;

#[cfg(test)]
#[path = "range_battle_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "range_order_profile_tests.rs"]
mod order_profile_tests;

#[cfg(test)]
#[path = "range_laser_tracking_tests.rs"]
mod tracking_tests;

#[cfg(test)]
#[path = "range_burst_admission_tests.rs"]
mod burst_admission_tests;
