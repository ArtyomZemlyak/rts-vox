//! Bounded skirmish on the range's ordinary, fully damageable unit roster.
//! Commands never exchange slots, matter containers or projectile ownership.
use super::ai::{self, Policy};
use super::*;
use fx::blob::{Blob, Scan};
use modding::{explosion::Delivery, range_battle::Tuning};

#[path = "range_orders.rs"]
mod orders;
#[path = "range_razlom.rs"]
mod razlom;
#[path = "range_idle_aim.rs"]
mod idle_aim;

#[path = "range_commands.rs"]
pub mod commands;

#[cfg(test)]
#[path = "range_command_tests.rs"]
mod command_tests;

#[cfg(test)]
#[path = "range_pilot_tests.rs"]
mod pilot_tests;

#[cfg(test)]
#[path = "range_order_handler_tests.rs"]
mod order_handler_tests;

#[cfg(test)]
#[path = "range_artillery_tests.rs"]
mod artillery_tests;

#[cfg(test)]
#[path = "range_unit_felling_tests.rs"]
mod unit_felling_tests;

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
    Siege = 7,
    Nuclear = 8,
    Lightning = 9,
}
impl Weapon {
    pub const COUNT: usize = 10;
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::MachineGun,
            1 => Self::Blast,
            2 => Self::Kinetic,
            3 => Self::Energy,
            4 => Self::Annihilation,
            5 => Self::Laser,
            6 => Self::Canister,
            7 => Self::Siege,
            8 => Self::Nuclear,
            9 => Self::Lightning,
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
            Self::Blast
                | Self::Energy
                | Self::Annihilation
                | Self::Canister
                | Self::Siege
                | Self::Nuclear
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
    /// A factory's persistent destination for subsequently produced units.
    Rally([i32; 2]),
    Attack(usize),
    Ground([i32; 3]),
    /// Dig the ball around this face into the unit's tank (ADR-672). The
    /// unit holds its fire, and the rules AI leaves the order alone.
    Dig([i32; 3]),
    /// Collect wreckage without drilling the ground.
    Reclaim([i32; 3]),
    /// Directly fabricate an existing allied construction site.
    Build(usize),
    /// Restore damaged cells of a living allied unit using the commander's matter.
    Repair(usize),
    /// Transfer charge to another allied electric tank.
    Support(usize),
    /// Persistent direct control; axes are -1, 0, 1. Uses ordinary hull physics.
    Pilot {
        throttle: i8,
        turn: i8,
        aim: [i32; 3],
        fire: bool,
    },
}
impl Order {
    pub fn point(self) -> Option<[i32; 3]> {
        match self {
            Self::Ground(p) | Self::Pilot { aim: p, .. } => Some(p),
            _ => None,
        }
    }
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
    // Also the deployment deadline while a crawler has an active Dig order.
    pub(super) pursuit: u32,
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
    /// The side that fired it (`allied`), for the per-side half of the
    /// wave ceiling (ADR-649). `None` only for a wave restored from a
    /// snapshot older than version 60: it counts against the whole
    /// ceiling and against neither half.
    pub(super) side: Option<bool>,
}
#[path = "range_wave_energy.rs"]
mod energy;
/// Divide a phase quota without losing its remainder. `turn` is the offset
/// in the tick's rotated service order, not an index into reordered state.
#[derive(Clone, Copy)]
pub struct Impact {
    pub at: [i32; 3],
    /// Last free sweep sample, for presentation on the incident side.
    /// Transient event only: not part of snapshots or authoritative hashes.
    pub from: [i32; 3],
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
    pub walker_lasers: Vec<weapons::Trace>,
    pub razlom_lasers: Vec<weapons::WaveTrace>,
    pub lightning: Vec<weapons::Trace>,
    pub laser_config: ExplosionConfig,
    pub tuning: std::sync::Arc<Tuning>,
    pub shell_config: ExplosionConfig,
    /// The tank's armour-piercing round (ADR-456).
    pub kinetic_config: ExplosionConfig,
    bullet_config: ExplosionConfig,
    pub canister_config: ExplosionConfig,
    pub siege_config: ExplosionConfig,
    pub nuclear_config: ExplosionConfig,
    pub energy_config: ExplosionConfig,
    pub anni_config: ExplosionConfig,
    pub(super) ai: ai::Controller,
    /// Both teams use the same deterministic decision clock in spectator mode.
    pub autonomous: bool,
    // Derived diagnostics: absent in hashes/snapshots, never read by combat.
    fire_trace: Option<Vec<FireGate>>,
    command_capture: Option<Box<commands::Capture>>,
    // Transient phase handoff; the command owner retains the authoritative
    // receipt and re-arms this buffer when restoring its pending checkpoint.
    plan_execution: Option<Box<commands::runtime::PlanExecution>>,
    pub(crate) ai_step: Option<Box<commands::runtime::AiStep>>,
    /// Waves the last tick's ceilings held back: capturing, advancing
    /// (ADR-681). A diagnostic, absent in hashes and snapshots.
    pub wave_held: [usize; 2],
}
pub(super) fn attack_range(
    tuning: &Tuning,
    kind: RangeUnitKind,
    weapon: Weapon,
    machine_gun: i32,
) -> i32 {
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
        Weapon::Siege => tuning.siege.range,
        Weapon::Nuclear => tuning.nuclear.range,
        Weapon::Laser => motion::spider_tuning().laser.range,
        Weapon::Lightning => lightning_tuning().range,
    }
}

fn lightning_tuning() -> &'static modding::range_weapon::Stream {
    static TUNING: std::sync::OnceLock<modding::range_weapon::Stream> = std::sync::OnceLock::new();
    TUNING.get_or_init(|| {
        modding::range_weapon::Tuning::from_ron(include_str!("../../../data/range-weapons.ron"))
            .expect("range weapon tuning")
            .lightning
            .expect("stock lightning tuning")
    })
}
fn supported_lightning(helpers: u32) -> modding::range_weapon::Stream {
    let mut stream = lightning_tuning().clone();
    let factor = 1 + helpers.min(4);
    stream.range = stream.range.saturating_mul(factor as i32);
    stream.work = stream.work.saturating_mul(factor);
    stream.heat = stream.heat.saturating_mul(factor);
    stream.fracture_j = stream.fracture_j.saturating_mul(factor);
    stream.reload_ticks = (stream.reload_ticks / factor).max(1);
    stream
}

fn loft(t: &Tuning, weapon: Weapon) -> i32 {
    match weapon {
        Weapon::Canister => t.canister.flight_time_milli,
        Weapon::Siege => t.siege.flight_time_milli,
        Weapon::Nuclear => t.nuclear.flight_time_milli,
        _ => 1000,
    }
}

/// One loadout rule for combat, presentation and controller observations.
pub(super) fn effective_weapon(
    kind: RangeUnitKind,
    explicit: Option<Weapon>,
    ground: bool,
    target: Option<RangeUnitKind>,
    tuning: &Tuning,
) -> Option<Weapon> {
    if kind == RangeUnitKind::NuclearSilo {
        return Some(Weapon::Nuclear);
    }
    if kind == RangeUnitKind::Commander {
        return Some(Weapon::Energy);
    }
    if matches!(
        kind,
        RangeUnitKind::Base
            | RangeUnitKind::PowerPlantT1
            | RangeUnitKind::PowerPlantT2
            | RangeUnitKind::PowerPlantT3
            | RangeUnitKind::Crawler
            | RangeUnitKind::GrebenStorage
            | RangeUnitKind::KilFactoryT1
            | RangeUnitKind::KilFactoryT2
            | RangeUnitKind::KilFactoryT3
            | RangeUnitKind::KilFactoryT4
            | RangeUnitKind::SkobaDrone
    ) {
        return None;
    }
    if kind == RangeUnitKind::Walker04 || kind == RangeUnitKind::RazlomT3 {
        return Some(Weapon::Laser);
    }
    if kind == RangeUnitKind::LightningTank {
        return Some(Weapon::Lightning);
    }
    if kind == RangeUnitKind::EidolonTank {
        return Some(Weapon::Kinetic);
    }
    if kind == RangeUnitKind::Spider {
        return Some(Weapon::Laser);
    }
    if kind == RangeUnitKind::Strider {
        return Some(Weapon::MachineGun);
    }
    if kind == RangeUnitKind::ArtilleryTurretT3 {
        return Some(Weapon::Siege);
    }
    if kind.canister() {
        return Some(Weapon::Canister);
    }
    if kind == RangeUnitKind::Artillery {
        return Some(explicit.unwrap_or(Weapon::Blast));
    }
    if kind == RangeUnitKind::TurretT1 {
        return Some(Weapon::Kinetic);
    }
    if kind == RangeUnitKind::TurretT2 {
        return Some(Weapon::Energy);
    }
    let ammo = tuning.tank_ammo;
    Some(explicit.unwrap_or_else(|| {
        Weapon::ammo(match (ground, target) {
            (true, _) | (_, None) => ammo.ground,
            (
                _,
                Some(
                    RangeUnitKind::Tank
                    | RangeUnitKind::EidolonTank
                    | RangeUnitKind::Artillery
                    | RangeUnitKind::ArtilleryT2
                    | RangeUnitKind::Base,
                ),
            ) => ammo.tank,
            _ => ammo.strider,
        })
    }))
}

impl Actor {
    /// A fresh actor for a unit: automatic order, the kind's stock gun.
    /// A produced unit gets the same (ADR-675).
    pub(super) fn of(u: &RangeUnit) -> Self {
        Actor {
            turret: crate::turret::Traverse::new(u.motion.drive.head),
            order: if u.kind == RangeUnitKind::NuclearSilo {
                Order::Hold
            } else {
                Order::Auto
            },
            target: None,
            next: 0,
            charge: None,
            pursuit: 0,
            burst: 0,
            elevation: crate::turret::Traverse::new(0),
            fired: 0,
            weapon: match u.kind {
                RangeUnitKind::ArtilleryTurretT3 => Some(Weapon::Siege),
                RangeUnitKind::NuclearSilo => Some(Weapon::Nuclear),
                RangeUnitKind::Tank => Some(Weapon::Kinetic),
                RangeUnitKind::Artillery => Some(Weapon::Blast),
                RangeUnitKind::ArtilleryT2 | RangeUnitKind::ArtilleryTurretT2 => {
                    Some(Weapon::Canister)
                }
                _ => None,
            },
        }
    }
}

impl Battle {
    /// A blast wave of the shell at `at` over the present roster. Tests
    /// of a roster growing under a live wave (ADR-675).
    #[cfg(test)]
    pub(crate) fn push_test_wave(&mut self, units: &[RangeUnit], at: [i32; 3]) {
        self.waves.push(Burst {
            side: None,
            weapon: Weapon::Blast,
            wave: Explosion::begin(at, &self.shell_config, units),
            energy: u64::from(self.shell_config.material_energy_j) * 1000,
        });
    }
    /// The shell's capture budget per tick; not in the snapshot.
    #[cfg(test)]
    pub(crate) fn set_test_capture(&mut self, capture: usize) {
        self.shell_config.capture_cells_per_tick = capture;
    }
    /// Each live wave's roster: the units it began with.
    #[cfg(test)]
    pub(crate) fn wave_rosters(&self, units: usize) -> Vec<usize> {
        self.waves.iter().map(|w| w.wave.roster(units)).collect()
    }
    /// Append a produced unit's actor, and its fire trace when tracing.
    pub(super) fn push_actor(&mut self, u: &RangeUnit) {
        self.actors.push(Actor::of(u));
        if let Some(trace) = self.fire_trace.as_mut() {
            trace.push(FireGate::NoTarget);
        }
    }
    pub fn new(units: &[RangeUnit], resistance: u32) -> Self {
        let tuning = std::sync::Arc::new(
            Tuning::from_ron(include_str!("../../../data/range-battle.ron"))
                .expect("battle tuning"),
        );
        let mut shell_config =
            ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        shell_config = shell_config.at_power(tuning.tank_power_milli);
        shell_config.unit_ablation_resistance = resistance;
        // The same tank's power drives either round.
        let mut kinetic_config =
            ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap();
        kinetic_config = kinetic_config.at_power(tuning.tank_power_milli);
        kinetic_config.unit_ablation_resistance = resistance;
        let mut bullet_config =
            ExplosionConfig::from_ron(include_str!("../../../data/machine-gun.ron")).unwrap();
        bullet_config.unit_ablation_resistance = resistance;
        let energetic = |text| {
            ExplosionConfig::from_ron(text)
                .expect("energy weapon tuning")
                .at_power(tuning.tank_power_milli)
        };
        let energy_config = energetic(include_str!("../../../data/energy-cannon.ron"));
        let anni_config = energetic(include_str!("../../../data/anni-cannon.ron"));
        let mut canister_config = energetic(include_str!("../../../data/canister-shell.ron"));
        canister_config.unit_ablation_resistance = resistance;
        let mut siege_config = energetic(include_str!("../../../data/siege-shell.ron"));
        siege_config.unit_ablation_resistance = resistance;
        let nuclear_config = energetic(include_str!("../../../data/nuclear-warhead.ron"));
        Self {
            phase: Phase::Setup,
            actors: units.iter().map(Actor::of).collect(),
            rounds: Vec::new(),
            waves: Vec::new(),
            impacts: Vec::new(),
            traces: Vec::new(),
            lasers: Vec::new(),
            walker_lasers: Vec::new(),
            razlom_lasers: Vec::new(),
            lightning: Vec::new(),
            laser_config: ExplosionConfig::from_ron(include_str!("../../../data/laser.ron"))
                .expect("laser"),
            tuning,
            shell_config,
            kinetic_config,
            bullet_config,
            canister_config,
            siege_config,
            nuclear_config,
            energy_config,
            anni_config,
            ai: ai::Controller::new(),
            autonomous: false,
            fire_trace: None,
            command_capture: None,
            plan_execution: None,
            ai_step: None,
            wave_held: [0; 2],
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
            Weapon::Siege => &self.siege_config,
            Weapon::Nuclear => &self.nuclear_config,
            Weapon::Energy => &self.energy_config,
            Weapon::Annihilation => &self.anni_config,
            Weapon::MachineGun => &self.bullet_config,
            Weapon::Laser | Weapon::Lightning => &self.laser_config,
        }
    }
    fn material_energy_mj(&self, weapon: Weapon) -> u64 {
        if weapon == Weapon::Nuclear {
            self.tuning.nuclear.material_energy_j * 1000
        } else {
            u64::from(self.config(weapon).material_energy_j) * 1000
        }
    }
    pub(super) fn shot_config(&self, weapon: Weapon, kind: RangeUnitKind) -> ExplosionConfig {
        let mut config = *self.config(weapon);
        if kind == RangeUnitKind::EidolonTank && weapon == Weapon::Kinetic {
            config = config.at_power(500);
        }
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
    /// Whether a live wave knows its side (ADR-649): snapshot version 60.
    pub(super) fn has_wave_sides(&self) -> bool {
        self.waves.iter().any(|w| w.side.is_some())
    }
    pub(super) fn save_wave_sides(&self, b: &mut Blob) {
        for w in &self.waves {
            b.u8(match w.side {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            });
        }
    }
    pub(super) fn load_wave_sides(&mut self, s: &mut Scan) -> Option<()> {
        for w in &mut self.waves {
            w.side = match s.u8() {
                0 => None,
                1 => Some(true),
                2 => Some(false),
                _ => return None,
            };
        }
        s.ok().then_some(())
    }
    /// Field boxes of the live waves, in storage order.
    pub(super) fn domains(&self) -> impl Iterator<Item = ([i32; 3], [i32; 3])> + '_ {
        self.waves.iter().map(|w| w.wave.domain())
    }
    pub(super) fn nuclear_domains(&self) -> impl Iterator<Item = ([i32; 3], [i32; 3])> + '_ {
        self.waves
            .iter()
            .filter(|w| w.weapon == Weapon::Nuclear)
            .map(|w| w.wave.domain())
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
            let n = burst.wave.roster(units.len());
            burst.wave.load_contacts(s, &units[..n], defer)?;
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
                Order::Rally(p) => { b.u8(11); for v in p { b.i32(v); } }
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
                Order::Dig(p) => {
                    b.u8(5);
                    for v in p {
                        b.i32(v);
                    }
                }
                Order::Reclaim(p) => {
                    b.u8(7);
                    for v in p {
                        b.i32(v);
                    }
                }
                Order::Build(site) => {
                    b.u8(9);
                    b.usize(site);
                }
                Order::Repair(target) => {
                    b.u8(10);
                    b.usize(target);
                }
                Order::Support(id) => {
                    b.u8(8);
                    b.usize(id);
                }
                Order::Pilot {
                    throttle,
                    turn,
                    aim,
                    fire,
                } => {
                    b.u8(6);
                    b.u8((throttle + 1) as u8);
                    b.u8((turn + 1) as u8);
                    for v in aim {
                        b.i32(v);
                    }
                    b.bool(fire);
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
        migration: (u32, u32, &[bool], usize),
    ) -> Option<()> {
        let (version, tick, allied, authored) = migration;
        self.phase = match s.u8() {
            0 => Phase::Setup,
            1 => Phase::Fighting,
            2 => Phase::Victory,
            3 => Phase::Defeat,
            4 => Phase::Draw,
            _ => return None,
        };
        for (id, a) in self.actors.iter_mut().enumerate() {
            a.turret = crate::turret::Traverse::load(s, limits)?;
            a.order = match s.u8() {
                0 => Order::Auto,
                1 => Order::Hold,
                2 => Order::Move,
                11 if matches!(units[id].kind, RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3 | RangeUnitKind::KilFactoryT4) => {
                    let p = [s.i32(), s.i32()];
                    if p[0] < 40 || p[1] < 40 || p[0] >= size[0] as i32 - 40 || p[1] >= size[2] as i32 - 40 { return None; }
                    Order::Rally(p)
                }
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
                // Only a layout with an economy digs; its snapshots are 62.
                5 if version >= 62 => {
                    let p = std::array::from_fn(|_| s.i32());
                    if (0..3).any(|k| p[k] < 0 || p[k] >= size[k] as i32) {
                        return None;
                    }
                    Order::Dig(p)
                }
                7 if version >= 62 => {
                    let p = std::array::from_fn(|_| s.i32());
                    if (0..3).any(|k| p[k] < 0 || p[k] >= size[k] as i32) {
                        return None;
                    }
                    Order::Reclaim(p)
                }
                8 => {
                    let target = s.usize();
                    if target >= units.len()
                        || target == id
                        || units[id].kind != RangeUnitKind::LightningTank
                        || units[target].kind != RangeUnitKind::LightningTank
                        || allied[id] != allied[target]
                    {
                        return None;
                    }
                    Order::Support(target)
                }
                9 if version >= 65 && matches!(units[id].kind, RangeUnitKind::Commander | RangeUnitKind::SkobaDrone) => {
                    let site = s.usize();
                    if site >= modding::range::MAX_RANGE_UNITS {
                        return None;
                    }
                    Order::Build(site)
                }
                10 if version >= 62 && units[id].kind == RangeUnitKind::Commander => {
                    let target = s.usize();
                    if target >= units.len() || target == id || allied[target] != allied[id] {
                        return None;
                    }
                    Order::Repair(target)
                }
                6 => {
                    let throttle = s.u8();
                    let turn = s.u8();
                    let aim = std::array::from_fn(|_| s.i32());
                    let fire = s.bool();
                    if throttle > 2
                        || turn > 2
                        || (0..3).any(|k| aim[k] < 0 || aim[k] >= size[k] as i32)
                    {
                        return None;
                    }
                    Order::Pilot {
                        throttle: throttle as i8 - 1,
                        turn: turn as i8 - 1,
                        aim,
                        fire,
                    }
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
        // Admission limits differ for autonomous battles; its flag follows
        // this section in the snapshot. Bound allocation by encoded bytes.
        if n > s.left() / 65 {
            return None;
        }
        for _ in 0..n {
            let owner = s.usize();
            let weapon = Weapon::from_code(s.u8())?;
            if (weapon == Weapon::Lightning
                || (weapon == Weapon::Laser
                    && (owner >= units.len() || units[owner].kind != RangeUnitKind::RazlomT3)))
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
                || (weapon == Weapon::Nuclear && units[owner].kind != RangeUnitKind::NuclearSilo)
                || (weapon == Weapon::Siege
                    && units[owner].kind != RangeUnitKind::ArtilleryTurretT3)
                || !super::restored_round(pos, velocity, size)
                || distance <= Fx::ZERO
                || distance
                    > Fx::from_int(
                        if matches!(weapon, Weapon::Canister | Weapon::Siege | Weapon::Nuclear) {
                            100000
                        } else {
                            10000
                        },
                    )
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
        if n > s.left() / 8 {
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
            if energy > self.material_energy_mj(weapon) {
                return None;
            }
            let mut wave = Explosion::load(s, units, authored, size, true)?;
            wave.nuclear_ground = weapon == Weapon::Nuclear;
            self.waves.push(Burst {
                weapon,
                energy,
                wave,
                side: None,
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
fn stop_active_beam(a: &mut Actor, kind: RangeUnitKind, tick: u32) {
    if matches!(kind, RangeUnitKind::Spider | RangeUnitKind::LightningTank) && a.burst > 0 {
        let reload = if kind == RangeUnitKind::LightningTank {
            lightning_tuning().reload_ticks
        } else {
            motion::spider_tuning().laser.reload_ticks
        };
        a.next = a.next.max(tick.saturating_add(reload));
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

/// Solve elevation about the fixed mount, not the previous pitched muzzle.
/// At the target azimuth the barrel line has a fixed normal offset `height`:
/// -forward * sin(pitch) + up * cos(pitch) = height.
/// Its longitudinal muzzle offset does not change the required angle.
fn spider_laser_pitch(u: &RangeUnit, target: [i32; 3]) -> i32 {
    let frame = &u.pose;
    let mount = Fx::from_int(i32::from(u.body.turret_y));
    let pivot = frame.base.add(frame.y.scale(mount.div(frame.scale)));
    let delta = phys::V3::new(
        Fx::from_int(target[0]),
        Fx::from_int(target[1]),
        Fx::from_int(target[2]),
    )
    .sub(pivot);
    let up = delta.dot(frame.y);
    // Keep azimuth and elevation independent during a live transfer. Using
    // the projection onto the current barrel heading would command a near
    // vertical pitch while the yaw passes 90 degrees from the new target.
    let horizontal_sq = delta.len_sq().sub(up.mul(up)).max(Fx::ZERO);
    let forward = horizontal_sq.sqrt();
    let height = u.body.weapon_paths().first().map_or(Fx::ZERO, |path| {
        weapons::muzzle_cell(path)[1].sub(mount).div(frame.scale)
    });
    let along = horizontal_sq
        .add(up.mul(up))
        .sub(height.mul(height))
        .max(Fx::ZERO)
        .sqrt();
    fx::angle::diff(
        fx::angle::atan2(up.raw(), forward.raw()),
        fx::angle::atan2(height.raw(), along.raw()),
    )
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
    /// Economy controller uses the same validation and movement executor as
    /// player orders, without changing the side's AI ownership.
    pub(crate) fn economy_ai_order(&mut self, id: usize, order: Order, side: bool) -> bool {
        let request = orders::Request { actor: id, order, destination: None };
        match orders::validate(self, &self.battle.actors, request, side, orders::Replacement::Explicit) {
            orders::Admission::Rejected => false,
            orders::Admission::Unchanged => true,
            orders::Admission::Apply => {
                let request = match order {
                    Order::Dig(face) => orders::Request { destination: self.dig_approach(id, face), ..request },
                    Order::Reclaim(face) => orders::Request { destination: self.reclaim_approach(id, face), ..request },
                    _ => request,
                };
                let outcome = orders::Executor {
                    units: &mut self.units,
                    actors: &mut self.battle.actors,
                    world: &self.world,
                    rubble: &self.rubble,
                    tick: self.tick,
                }.apply(request, orders::Replacement::Explicit);
                if outcome.accepted() {
                    if let Some(m) = self.mining.as_mut() { m.reset_leg(id); }
                }
                outcome.accepted()
            }
        }
    }
    pub(crate) fn economy_ai_move(&mut self, id: usize, destination: [i32; 2], side: bool) -> bool {
        let request = orders::Request { actor: id, order: Order::Move, destination: Some(destination) };
        if !matches!(orders::validate(self, &self.battle.actors, request, side, orders::Replacement::Explicit), orders::Admission::Apply) { return false; }
        orders::Executor { units: &mut self.units, actors: &mut self.battle.actors,
            world: &self.world, rubble: &self.rubble, tick: self.tick }
            .apply(request, orders::Replacement::Explicit).accepted()
    }
    /// The reference scenario (ADR-662): the authored world battle on the
    /// kilometre range, both sides under rules AI, already started. The
    /// golden test and `sim-cli` hash exactly this, so the scenario is
    /// built here and nowhere else (ADR-157). Files are read by the caller.
    pub fn world_battle(
        config: ExplosionConfig,
        mut layout: RangeLayout,
        preset: modding::range_skirmish::WorldBattle,
    ) -> Result<Self, String> {
        layout.battle = true;
        layout.units = preset.units;
        layout.blocks.extend(preset.blocks);
        if let Some(course) = &mut layout.course {
            course.patrols.clear();
        }
        let mut sim = Self::with_layout(config, layout)?;
        if !sim.set_ai_vs_ai(true) {
            return Err("world battle: rules AI refused".into());
        }
        sim.start_battle()?;
        Ok(sim)
    }
    pub fn battle_mode(&self) -> bool {
        self.layout.as_ref().is_some_and(|l| l.battle)
    }
    pub fn point_gun(&self) -> bool {
        self.layout.as_ref().is_some_and(|l| l.gun_point)
    }
    pub fn physical_gun(&self) -> bool {
        !self.battle_mode() && !self.point_gun()
    }
    /// Setup accepts explicit movement without running combat or AI.
    pub fn battle_movement_active(&self) -> bool {
        matches!(self.battle.phase, Phase::Setup | Phase::Fighting)
    }
    /// Effective loadout, shared by aiming, HUD and range indicators.
    pub fn battle_weapon(&self, id: usize) -> Option<Weapon> {
        let u = self.units.get(id)?;
        let a = self.battle.actors.get(id)?;
        effective_weapon(
            u.kind,
            a.weapon,
            a.order.point().is_some(),
            a.target.and_then(|id| self.units.get(id)).map(|u| u.kind),
            &self.battle.tuning,
        )
    }
    /// Connected barrels in the actor's current posed weapon, for muzzle effects.
    pub fn battle_charge(&self, id: usize) -> Option<u32> {
        self.battle.actors.get(id)?.charge
    }
    pub fn battle_reload_until(&self, id: usize) -> Option<u32> {
        self.battle.actors.get(id).map(|a| a.next)
    }
    pub fn battle_muzzles(&self, id: usize) -> Vec<[Fx; 3]> {
        match (self.units.get(id), self.battle.actors.get(id)) {
            (Some(unit), Some(actor)) => weapons::muzzles(unit, actor.turret.angle),
            _ => Vec::new(),
        }
    }
    /// Tip of the commander's articulated fabrication tool.
    pub fn commander_tool_tip(&self, id: usize) -> Option<[Fx; 3]> {
        let u = self.units.get(id)?;
        if u.kind != RangeUnitKind::Commander || !u.status.alive
            || u.seg_gone.get(14..18)?.iter().any(|gone| *gone)
        {
            return None;
        }
        let body = &u.body.segments.get(17)?.body;
        if body.solid_count() == 0 { return None; }
        let p = u.seg_frames.get(17)?.world([
            Fx::from_int(i32::from(body.vol.nx)),
            Fx::from_ratio(i32::from(body.vol.ny), 2),
            Fx::from_ratio(i32::from(body.vol.nz), 2),
        ]);
        Some([p.x, p.y, p.z])
    }
    pub fn battle_attack_range(&self, id: usize) -> Option<i32> {
        let weapon = self.battle_weapon(id)?;
        if weapon == Weapon::Lightning {
            return Some(supported_lightning(self.electric_support_count(id)).range);
        }
        Some(attack_range(
            &self.battle.tuning,
            self.units.get(id)?.kind,
            weapon,
            self.streams.tuning.machine_gun.range,
        ))
    }
    /// Active, in-range allied links; the visual readout uses the same rules
    /// as the combat pass, and never grants the bonus by itself.
    pub fn electric_support_count(&self, id: usize) -> u32 {
        let Some(target) = self.units.get(id) else {
            return 0;
        };
        if target.kind != RangeUnitKind::LightningTank
            || !target.status.alive
            || !target.status.weapon
        {
            return 0;
        }
        self.battle
            .actors
            .iter()
            .enumerate()
            .filter(|(other, actor)| {
                let u = &self.units[*other];
                if actor.order != Order::Support(id)
                    || !u.status.alive
                    || !u.status.weapon
                    || !u.status.engine
                    || self.allied(*other) != self.allied(id)
                {
                    return false;
                }
                let dx =
                    i64::from(u.motion.pos[0].to_int_trunc() - target.motion.pos[0].to_int_trunc());
                let dz =
                    i64::from(u.motion.pos[2].to_int_trunc() - target.motion.pos[2].to_int_trunc());
                dx * dx + dz * dz <= 160i64.pow(2)
            })
            .count()
            .min(4) as u32
    }

    /// Switching ammunition keeps the current order and reload deadline.
    /// Rounds already in flight retain their original configuration.
    pub fn set_battle_weapon(&mut self, id: usize, weapon: Option<Weapon>) -> bool {
        self.execute_command(commands::Command::SetBattleWeapon { actor: id, weapon })
            .accepted()
    }
    fn apply_battle_weapon(&mut self, id: usize, weapon: Option<Weapon>) -> commands::Outcome {
        if !self.battle_mode()
            || !self.allied(id)
            || self.units.get(id).is_none_or(|u| {
                !u.status.alive || !matches!(u.kind, RangeUnitKind::Tank | RangeUnitKind::Artillery)
            })
            || matches!(
                weapon,
                Some(
                    Weapon::MachineGun
                        | Weapon::Laser
                        | Weapon::Lightning
                        | Weapon::Canister
                        | Weapon::Siege
                        | Weapon::Nuclear
                )
            )
        {
            return commands::Outcome::Rejected;
        }
        let Some(actor) = self.battle.actors.get_mut(id) else {
            return commands::Outcome::Rejected;
        };
        if actor.weapon == weapon {
            return commands::Outcome::Unchanged;
        }
        actor.weapon = weapon;
        actor.charge = None;
        commands::Outcome::Applied
    }
    pub(super) fn idle_enemy(&self, id: usize) -> bool {
        self.layout.as_ref().is_some_and(|l| l.idle_enemies) && !self.allied(id)
    }
    pub fn enemy_ai_tuning(&self) -> &modding::battle_ai::Tuning {
        &self.battle.ai.tuning
    }
    /// Tactical radii of the pack (analysis 240, B3); a plan producer
    /// outside the simulation reads the same ones.
    pub fn enemy_ai_radii(&self) -> &modding::battle_ai::Radii {
        &self.battle.ai.radii
    }
    /// Enable autonomous orders for both teams before the fight starts.
    pub fn set_ai_vs_ai(&mut self, enabled: bool) -> bool {
        self.execute_battle_lifecycle(commands::BattleLifecycle::Autonomy { enabled })
            .is_ok()
    }
    /// Rules are data-driven and can be replaced while setting up a battle.
    /// Changing them during a fight starts a new decision cycle on this tick.
    pub fn configure_enemy_ai(&mut self, tuning: modding::battle_ai::Tuning) -> Result<(), String> {
        self.configure_enemy_ai_command(tuning).map(|_| ())
    }
    fn configure_enemy_ai_command(
        &mut self,
        tuning: modding::battle_ai::Tuning,
    ) -> Result<commands::Outcome, String> {
        let result = tuning.validate().map(|()| {
            self.battle.ai.tuning = tuning;
            // Equal tuning still restarts the current decision cycle and
            // invalidates a supplied plan, matching the existing API.
            self.battle.ai.next_tick = self.tick;
            self.battle.ai.pending = None;
            commands::Outcome::Applied
        });
        self.record_player_command(
            commands::Command::ConfigureEnemyAi { tuning },
            result
                .as_ref()
                .copied()
                .unwrap_or(commands::Outcome::Rejected),
        );
        result
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
                    attack_ranges: std::array::from_fn(|target| {
                        if !unit.status.weapon {
                            return 0;
                        }
                        let target = RangeUnitKind::ALL[target];
                        effective_weapon(
                            unit.kind,
                            battle.actors[id].weapon,
                            false,
                            Some(target),
                            &battle.tuning,
                        )
                        .map_or(0, |weapon| {
                            attack_range(
                                &battle.tuning,
                                unit.kind,
                                weapon,
                                self.streams.tuning.machine_gun.range,
                            )
                        })
                    }),
                    id,
                    allied: self.allied(id),
                    alive: unit.status.alive,
                    armed: unit.status.weapon
                        && unit.kind != RangeUnitKind::NuclearSilo
                        && effective_weapon(unit.kind, battle.actors[id].weapon, false, None, &battle.tuning).is_some()
                        && !self.idle_enemy(id),
                    mobile: unit.status.engine
                        && unit.motion.can_drive(unit.mobility)
                        && !self.idle_enemy(id),
                    can_reposition: !self.idle_enemy(id)
                        && unit.status.engine
                        && !unit.kind.emplacement()
                        && unit.motion.can_reposition(unit.mobility),
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
        // Replay supplies both sides from the SAME recorded AfterMotion phase.
        // It never falls back to policy, including rejected external plans.
        let replay = battle.ai_step.as_mut().and_then(|step| step.input.take());
        let (rules, allied_rules) = if let Some(input) = replay {
            input
        } else {
            assert!(!battle.ai_step.as_ref().is_some_and(|step| step.replay));
            let observation = self.enemy_ai_observation_for(battle);
            let rules = ai::Rules {
                tuning: &battle.ai.tuning,
                radii: &battle.ai.radii,
                controlled_allied: false,
            }
            .decide(&observation);
            let allied = battle.autonomous.then(|| {
                ai::Rules {
                    tuning: &battle.ai.tuning,
                    radii: &battle.ai.radii,
                    controlled_allied: true,
                }
                .decide(&observation)
            });
            (rules, allied)
        };
        let supplied = battle.ai.pending.take();
        if supplied.is_some() {
            if let Some(execution) = &mut battle.plan_execution {
                execution.tick = Some(self.tick);
            }
        }
        for fallback in rules.iter().copied() {
            let chosen = supplied
                .as_ref()
                .and_then(|plan| plan.iter().find(|d| d.actor == fallback.actor))
                .copied();
            let producer = if chosen.is_some() {
                commands::Producer::EnemyPlan
            } else {
                commands::Producer::EnemyRules
            };
            let outcome =
                self.apply_ai_decision_from(battle, chosen.unwrap_or(fallback), false, producer);
            if let Some(decision) = chosen {
                if let Some(execution) = &mut battle.plan_execution {
                    execution.attempts.push(commands::runtime::PlanAttempt {
                        decision,
                        outcome,
                        fallback: false,
                    });
                }
            }
            if !outcome.accepted() {
                let outcome = self.apply_ai_decision_from(
                    battle,
                    fallback,
                    false,
                    commands::Producer::EnemyFallback,
                );
                if chosen.is_some() {
                    if let Some(execution) = &mut battle.plan_execution {
                        execution.attempts.push(commands::runtime::PlanAttempt {
                            decision: fallback,
                            outcome,
                            fallback: true,
                        });
                    }
                }
            }
        }
        if let Some(rules) = &allied_rules {
            for decision in rules.iter().copied() {
                self.apply_ai_decision_from(
                    battle,
                    decision,
                    true,
                    commands::Producer::AlliedRules,
                );
            }
        }
        battle.ai.next_tick = self.tick.saturating_add(battle.ai.tuning.decision_ticks);
        if let Some(step) = &mut battle.ai_step {
            step.output = Some((rules, allied_rules));
        }
    }
    #[cfg(test)]
    fn apply_ai_decision(
        &mut self,
        battle: &mut Battle,
        decision: ai::Decision,
        side: bool,
    ) -> bool {
        self.apply_ai_decision_from(
            battle,
            decision,
            side,
            if side {
                commands::Producer::AlliedRules
            } else {
                commands::Producer::EnemyRules
            },
        )
        .accepted()
    }
    fn apply_ai_decision_from(
        &mut self,
        battle: &mut Battle,
        decision: ai::Decision,
        side: bool,
        producer: commands::Producer,
    ) -> commands::Outcome {
        let (order, destination) = match decision.order {
            ai::AiOrder::Attack(target) => (Order::Attack(target), None),
            ai::AiOrder::Move(p) => (Order::Move, Some(p)),
            ai::AiOrder::Hold => (Order::Hold, None),
            ai::AiOrder::Auto => (Order::Auto, None),
        };
        let request = orders::Request {
            actor: decision.actor,
            order,
            destination,
        };
        let outcome = match orders::validate(
            self,
            &battle.actors,
            request,
            side,
            orders::Replacement::Maintain,
        ) {
            orders::Admission::Rejected => commands::Outcome::Rejected,
            orders::Admission::Unchanged => commands::Outcome::Unchanged,
            orders::Admission::Apply => orders::Executor {
                units: &mut self.units,
                actors: &mut battle.actors,
                world: &self.world,
                rubble: &self.rubble,
                tick: self.tick,
            }
            .apply(request, orders::Replacement::Maintain),
        };
        if let Some(capture) = &mut battle.command_capture {
            capture.record(
                battle.ai.next_tick,
                self.tick,
                commands::CommandPhase::AfterMotion,
                producer,
                commands::Command::Order {
                    actor: decision.actor,
                    order,
                    destination,
                },
                outcome,
            );
        }
        outcome
    }
    /// The unit's side: an authored unit's from the layout, a produced
    /// one's from its spawn record (ADR-675).
    pub fn allied(&self, id: usize) -> bool {
        self.unit_def(id).is_some_and(|u| u.allied)
    }
    pub fn start_battle(&mut self) -> Result<(), &'static str> {
        self.execute_battle_lifecycle(commands::BattleLifecycle::StartBattle)
            .map(|_| ())
    }
    /// The same typed executor as CommandRuntime, with the existing UI reason.
    pub fn execute_battle_lifecycle(
        &mut self,
        command: commands::BattleLifecycle,
    ) -> Result<commands::Outcome, &'static str> {
        use commands::{BattleLifecycle, Command, Outcome};
        let result = match command {
            BattleLifecycle::StartBattle => self.apply_start_battle().map(|()| Outcome::Applied),
            BattleLifecycle::FreeControl => self.apply_free_control().map(|()| Outcome::Applied),
            BattleLifecycle::Autonomy { enabled } => {
                if !self.battle_mode() || self.battle.phase != Phase::Setup {
                    Err("Режим ИИ для обеих сторон задаётся до начала боя")
                } else if self.battle.autonomous == enabled {
                    Ok(Outcome::Unchanged)
                } else {
                    self.battle.autonomous = enabled;
                    Ok(Outcome::Applied)
                }
            }
        };
        self.record_player_command(
            Command::BattleLifecycle(command),
            result.unwrap_or(Outcome::Rejected),
        );
        result
    }
    /// Shared preflight for execution and diagnostic replay of a queued start.
    fn battle_start_rejection(&self) -> Option<&'static str> {
        if !self.battle_mode() {
            return Some("Сначала включите режим боя");
        }
        if self.battle.phase != Phase::Setup {
            return Some("Бой уже начат; восстановите расстановку для нового боя");
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
            return Some("Нужен хотя бы один свой и один вражеский юнит");
        }
        None
    }
    fn apply_start_battle(&mut self) -> Result<(), &'static str> {
        if let Some(error) = self.battle_start_rejection() {
            return Err(error);
        }
        if self.units.len() >= 16 && self.navigation_workers > 1 && self.navigation_pool.is_none() {
            self.set_navigation_workers(self.navigation_workers);
        }
        self.control_inner();
        self.fire_tick = u32::MAX;
        self.patrols_running = true;
        for u in &mut self.units {
            u.motion.route.clear();
            u.motion.stop();
        }
        for id in 0..self.units.len() {
            if self.idle_enemy(id) {
                self.battle.actors[id].order = Order::Hold;
            }
        }
        self.battle.phase = Phase::Fighting;
        self.battle.ai.next_tick = self.tick;
        self.battle.ai.pending = None;
        Ok(())
    }
    /// Drive the prepared roster without starting AI, combat or victory checks.
    pub fn start_free_control(&mut self) -> Result<(), &'static str> {
        self.execute_battle_lifecycle(commands::BattleLifecycle::FreeControl)
            .map(|_| ())
    }
    fn apply_free_control(&mut self) -> Result<(), &'static str> {
        if !self.battle_mode() || self.battle.phase != Phase::Setup {
            return Err("Свободное управление доступно до начала боя");
        }
        self.control_inner();
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
        self.execute_battle_command(commands::Command::Order {
            actor: id,
            order,
            destination,
        })
        .accepted()
    }
    /// Opt-in bounded diagnostics. Capture IDs are scoped to this session and
    /// are not authoritative acknowledgements or saved replay input.
    pub fn start_battle_command_capture(&mut self, max_records: usize) -> Result<(), &'static str> {
        if !self.battle_mode() {
            return Err("command capture needs battle mode");
        }
        self.start_command_capture(max_records)
    }
    /// Bounded external input diagnostics for either a range or a battle.
    pub fn start_command_capture(&mut self, max_records: usize) -> Result<(), &'static str> {
        if self.battle.command_capture.is_some() {
            return Err("command capture already active");
        }
        self.battle.command_capture =
            Some(Box::new(commands::Capture::new(self.tick, max_records)?));
        Ok(())
    }
    pub fn finish_battle_command_capture(&mut self) -> Option<commands::CommandTrace> {
        self.finish_command_capture()
    }
    pub fn finish_command_capture(&mut self) -> Option<commands::CommandTrace> {
        self.battle
            .command_capture
            .take()
            .map(|capture| capture.finish(self.tick))
    }
    /// Synchronous player ingress; formation search remains a continuation.
    pub fn execute_battle_command(&mut self, command: commands::Command) -> commands::Outcome {
        self.execute_command(command)
    }
    pub(super) fn queued_order_admitted(&self,actor:usize,order:Order,destination:Option<[i32;2]>)->bool {
        !matches!(self.manual_order_admission(self.battle.phase,orders::Request{actor,order,destination}),orders::Admission::Rejected)
    }
    pub(super) fn execute_order_continuation(&mut self,command:commands::Command)->commands::Outcome {
        let capture=self.battle.command_capture.take();
        let result=self.execute_command(command);
        self.battle.command_capture=capture;result
    }
    fn manual_order_admission(&self, phase: Phase, request: orders::Request) -> orders::Admission {
        if !self.battle_mode()
            || !(phase == Phase::Fighting
                || phase == Phase::Setup
                    && matches!(
                        request.order,
                        Order::Move
                            | Order::Rally(_)
                            | Order::Hold
                            | Order::Dig(_)
                            | Order::Reclaim(_)
                            | Order::Build(_)
                            | Order::Repair(_)
                            | Order::Support(_)
                            | Order::Pilot { fire: false, .. }
                    ))
        {
            orders::Admission::Rejected
        } else {
            orders::validate(
                self,
                &self.battle.actors,
                request,
                true,
                orders::Replacement::Explicit,
            )
        }
    }
    /// Shared synchronous ingress. Setup tuning and battle orders retain their
    /// own admission rules; external owners schedule this same operation.
    pub fn execute_command(&mut self, command: commands::Command) -> commands::Outcome {
        let (id, order, destination, fallback) = match command {
            commands::Command::Order{actor,order:Order::Build(site),destination:None} if self.greben_build_admitted(actor,site)=>{
                self.clear_order_queue(actor);
                let outcome=self.append_order(actor,super::build_plans::PlannedOrder{order:Order::Build(site),destination:None});
                self.record_player_command(command,outcome);return outcome;
            }
            commands::Command::QueueOrder{actor,order,destination}=>{
                let outcome=self.append_order(actor,super::build_plans::PlannedOrder{order,destination});
                self.record_player_command(command,outcome);return outcome;
            }
            commands::Command::PlaceConstruction{actor,kind,at,queued}=>{
                let outcome=self.place_ordered_construction(actor,kind,at,queued);
                self.record_player_command(command,outcome);return outcome;
            }
            commands::Command::CancelConstruction{site}=>{
                let outcome=self.cancel_construction_plan(site);self.record_player_command(command,outcome);return outcome;
            }
            commands::Command::CancelLastOrder{actor}=>{
                let outcome=self.cancel_last_planned_order(actor);self.record_player_command(command,outcome);return outcome;
            }
            commands::Command::Sculpt {
                at,
                edge,
                material,
                erase,
            } => {
                let outcome = self.sculpt(at, edge, material, erase);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::Gun(input) => {
                let outcome = self.apply_gun_command(input);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::SetPatrols { enabled } => {
                let outcome = if self.patrols_running == enabled {
                    commands::Outcome::Unchanged
                } else {
                    self.patrols_running = enabled;
                    commands::Outcome::Applied
                };
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::ConfigureEnemyAi { tuning } => {
                return self
                    .configure_enemy_ai_command(tuning)
                    .unwrap_or(commands::Outcome::Rejected);
            }
            commands::Command::BattleLifecycle(lifecycle) => {
                return self
                    .execute_battle_lifecycle(lifecycle)
                    .unwrap_or(commands::Outcome::Rejected);
            }
            commands::Command::SetBattleWeapon { actor, weapon } => {
                let outcome = self.apply_battle_weapon(actor, weapon);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::ConfigureWeapon { config } => {
                let outcome = self.configure_weapon(config);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::TuneMaterial {
                material,
                field,
                value,
            } => {
                let outcome = self.tune_material(material, field, value);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::TuneDebris { field, value } => {
                let outcome = self.tune_debris(field, value);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::TuneMotion {
                kind,
                include_gun,
                field,
                value,
            } => {
                let outcome = self.tune_motion(kind, include_gun, field, value);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::UnloadCrawler { actor, dock } => {
                let outcome = self.order_crawler_unload(actor, dock);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::Produce { actor, kind } => {
                let outcome = self.enqueue_product(actor, kind, true);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::Cancel { actor } => {
                let outcome = self.cancel_product(actor, true);
                self.record_player_command(command, outcome);
                return outcome;
            }
            commands::Command::Order {
                actor,
                order,
                destination,
            } => (actor, order, destination, None),
            commands::Command::FormationMove {
                actor,
                destination,
                fallback,
            } => (actor, Order::Move, Some(destination), Some(fallback)),
        };
        let request = orders::Request {
            actor: id,
            order,
            destination,
        };
        let mut outcome = match self.manual_order_admission(self.battle.phase, request) {
            orders::Admission::Rejected => commands::Outcome::Rejected,
            orders::Admission::Unchanged => commands::Outcome::Unchanged,
            orders::Admission::Apply => {
                // A digger out of reach drives to its face; the route
                // is derived, never part of the command (ADR-672).
                let request = match order {
                    Order::Dig(face) => orders::Request { destination: self.dig_approach(id, face), ..request },
                    Order::Reclaim(face) => orders::Request { destination: self.reclaim_approach(id, face), ..request },
                    _ => request,
                };
                // Between ticks, a manual Move uses the current physical rubble.
                let routed = order == Order::Move
                    || matches!(order, Order::Dig(_) | Order::Reclaim(_))
                        && request.destination.is_some();
                let current = (routed && fallback.is_none()).then(|| self.current_rubble());
                let mut executor = orders::Executor {
                    units: &mut self.units,
                    actors: &mut self.battle.actors,
                    world: &self.world,
                    rubble: current.as_ref().unwrap_or(&self.rubble),
                    tick: self.tick,
                };
                if let Some(fallback) = fallback {
                    executor.queue(request, fallback)
                } else {
                    executor.apply(request, orders::Replacement::Explicit)
                }
            }
        };
        // An admitted human order takes control from the allied rules AI.
        // Enemy decisions continue; rejected commands never change ownership.
        // Persist the existing team flag so save/replay need no new state.
        // An admitted but unreachable Move also expresses manual ownership.
        if outcome.admitted() && self.battle.autonomous {
            self.battle.autonomous = false;
            if outcome == commands::Outcome::Unchanged {
                outcome = commands::Outcome::Applied;
            }
        }
        // A new order ends a haul: it decides where to go (ADR-673).
        if outcome.admitted() {
            let had_queue=self.build_plans.queues.contains_key(&(id as u16));
            self.clear_order_queue(id);
            if had_queue && outcome==commands::Outcome::Unchanged {outcome=commands::Outcome::Applied;}
        }
        if matches!(
            outcome,
            commands::Outcome::Applied | commands::Outcome::Queued
        ) {
            if let Some(m) = self.mining.as_mut() {
                m.reset_leg(id);
            }
        }
        self.record_player_command(command, outcome);
        outcome
    }
    fn record_player_command(&mut self, command: commands::Command, outcome: commands::Outcome) {
        if let Some(capture) = &mut self.battle.command_capture {
            capture.record(
                self.tick,
                self.tick,
                commands::CommandPhase::BetweenTicks,
                commands::Producer::Player,
                command,
                outcome,
            );
        }
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
            // A rejected group has no physical effects. Record only the
            // bounded prefix, then count omissions without walking the rest.
            if let Some(capture) = &self.battle.command_capture {
                let count = destinations.len().min(capture.remaining());
                for &(actor, destination) in &destinations[..count] {
                    self.execute_battle_command(commands::Command::FormationMove {
                        actor,
                        destination,
                        fallback,
                    });
                }
                self.battle
                    .command_capture
                    .as_mut()
                    .unwrap()
                    .omit(destinations.len() - count);
            }
            return 0;
        }
        let mut accepted = 0;
        for &(id, target) in destinations {
            accepted += usize::from(
                self.execute_battle_command(commands::Command::FormationMove {
                    actor: id,
                    destination: target,
                    fallback,
                })
                .accepted(),
            );
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
        self.layout.as_ref()?;
        let mut health = [None, None];
        for (i, def) in self.roster().enumerate() {
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
            walker_lasers: Vec::new(),
            razlom_lasers: Vec::new(),
            lightning: Vec::new(),
            laser_config: self.battle.laser_config,
            tuning: self.battle.tuning.clone(),
            shell_config: self.battle.shell_config,
            kinetic_config: self.battle.kinetic_config,
            bullet_config: self.battle.bullet_config,
            canister_config: self.battle.canister_config,
            siege_config: self.battle.siege_config,
            nuclear_config: self.battle.nuclear_config,
            energy_config: self.battle.energy_config,
            anni_config: self.battle.anni_config,
            ai: self.battle.ai.clone(),
            autonomous: self.battle.autonomous,
            fire_trace: None,
            command_capture: None,
            plan_execution: None,
            ai_step: None,
            wave_held: [0; 2],
        };
        let mut battle = std::mem::replace(&mut self.battle, empty);
        battle.impacts.clear();
        battle.traces.clear();
        battle.lasers.clear();
        battle.walker_lasers.clear();
        battle.razlom_lasers.clear();
        battle.lightning.clear();
        // A surviving siege head carries its current pulse to the next target.
        for (id, actor) in battle.actors.iter_mut().enumerate() {
            if !self.units[id].status.alive || !self.units[id].status.weapon {
                stop_active_beam(actor, self.units[id].kind, self.tick);
                actor.target = None;
                actor.charge = None;
                actor.burst = 0;
            } else if actor
                .target
                .is_some_and(|target| !self.units[target].status.alive)
            {
                actor.target = None;
                if self.units[id].kind != RangeUnitKind::Spider || actor.burst == 0 {
                    if self.units[id].kind == RangeUnitKind::LightningTank {
                        stop_active_beam(actor, self.units[id].kind, self.tick);
                    }
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
        }
        if battle.phase == Phase::Fighting
            || battle.phase == Phase::Setup
                && battle
                    .actors
                    .iter()
                    .any(|a| matches!(a.order, Order::Pilot { .. } | Order::Support(_)))
        {
            self.battle_orders(&mut battle, &mut completed, physics);
        } else if battle.phase == Phase::Setup {
            for (unit, actor) in self.units.iter_mut().zip(&mut battle.actors) {
                if !matches!(actor.order, Order::Build(_) | Order::Repair(_) | Order::Dig(_) | Order::Reclaim(_)) {
                    idle_aim::follow(unit, actor, self.turret_limits);
                }
            }
        }
        completed("battle_orders");
        // New live beam deposits must reserve admission before wave/crater work.
        self.reserve_thermal_fragments();
        let revisions: Vec<_> = self.units.iter().map(|u| u.revision).collect();
        self.battle_flight(&mut battle);
        completed("battle_flight");
        self.matter.reserve_damage(
            battle.has_material_work() || self.pending_kinetic_impacts() > 0,
            u64::from(self.tick),
        );
        self.reserve_thermal_fragments();
        self.battle_waves(&mut battle, &mut completed, physics);
        completed("battle_waves");
        for (id, (u, old)) in self.units.iter_mut().zip(revisions).enumerate() {
            if u.revision != old {
                u.refresh_damage(id);
            }
        }
        completed("battle_damage");
        if battle.phase == Phase::Fighting && !self.layout.as_ref().is_some_and(|layout| layout.endless_battle) {
            // Commander-start scenarios use their authored commanders as the
            // objectives even after factories and other units are constructed.
            let commanders = self.layout.as_ref().and_then(|layout| {
                let own = layout.units.iter().position(|u| u.allied && u.kind == RangeUnitKind::Commander)?;
                let enemy = layout.units.iter().position(|u| !u.allied && u.kind == RangeUnitKind::Commander)?;
                Some([self.units.get(own)?.status.alive, self.units.get(enemy)?.status.alive])
            });
            // A base is the objective itself: an unrelated round still in
            // flight must not postpone the outcome indefinitely.
            if let Some([own, enemy]) = commanders {
                battle.phase = match (own, enemy) {
                    (false, false) => Phase::Draw,
                    (false, true) => Phase::Defeat,
                    (true, false) => Phase::Victory,
                    (true, true) => Phase::Fighting,
                };
            } else if let Some([own, enemy]) = self.battle_base_integrity() {
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
                            RangeUnitKind::Base
                            | RangeUnitKind::PowerPlantT1
                            | RangeUnitKind::PowerPlantT2
                            | RangeUnitKind::PowerPlantT3
                            | RangeUnitKind::GrebenStorage
                            | RangeUnitKind::KilFactoryT1
                            | RangeUnitKind::KilFactoryT2
                            | RangeUnitKind::KilFactoryT3
                            | RangeUnitKind::KilFactoryT4
                            | RangeUnitKind::SkobaDrone => None,
                            RangeUnitKind::Spider => Some(Weapon::Laser),
                            RangeUnitKind::LightningTank => Some(Weapon::Lightning),
                            RangeUnitKind::EidolonTank => Some(Weapon::Kinetic),
                            RangeUnitKind::Strider => Some(Weapon::MachineGun),
                            RangeUnitKind::TurretT1 => Some(Weapon::Kinetic),
                            RangeUnitKind::TurretT2 => Some(Weapon::Energy),
                            RangeUnitKind::Commander => Some(Weapon::Energy),
                            RangeUnitKind::ArtilleryTurretT3 => Some(Weapon::Siege),
                            RangeUnitKind::NuclearSilo => Some(Weapon::Nuclear),
                            RangeUnitKind::ArtilleryT2 | RangeUnitKind::ArtilleryTurretT2 => {
                                Some(Weapon::Canister)
                            }
                            RangeUnitKind::Artillery => {
                                Some(battle.actors[id].weapon.unwrap_or(Weapon::Blast))
                            }
                            RangeUnitKind::Walker04 | RangeUnitKind::RazlomT3 => {
                                Some(Weapon::Laser)
                            }
                            // An immobilized tank may still switch ammunition.
                            RangeUnitKind::Tank | RangeUnitKind::Crawler => None,
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
                            && !stationary_target_in_range
                            && !battle.actors.iter().enumerate().any(|(id, a)| {
                                self.units[id].status.alive
                                    && matches!(a.order, Order::Pilot { .. })
                            }) =>
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
        // Terminal battles cannot execute a future plan. Retaining it made
        // their physical snapshots fail Controller::load's fighting guard.
        if battle.phase != Phase::Fighting {
            battle.ai.pending = None;
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
        const LINK_RANGE: i32 = 160;
        let mut support = vec![0u32; self.units.len()];
        for (id, actor) in b.actors.iter().enumerate() {
            let Order::Support(target) = actor.order else {
                continue;
            };
            if target >= self.units.len() || target == id {
                continue;
            }
            let a = &self.units[id];
            let t = &self.units[target];
            if !a.status.alive
                || !a.status.weapon
                || !a.status.engine
                || !t.status.alive
                || !t.status.weapon
                || self.allied(id) != self.allied(target)
            {
                continue;
            }
            let dx = i64::from(a.motion.pos[0].to_int_trunc() - t.motion.pos[0].to_int_trunc());
            let dz = i64::from(a.motion.pos[2].to_int_trunc() - t.motion.pos[2].to_int_trunc());
            if dx * dx + dz * dz <= i64::from(LINK_RANGE).pow(2) {
                support[target] = (support[target] + 1).min(4);
            }
        }
        if let Some(trace) = &mut b.fire_trace {
            trace.fill(FireGate::NoTarget);
        }
        let mut bursts = Vec::new();
        for id in 0..self.units.len() {
            if b.phase == Phase::Setup
                && !matches!(b.actors[id].order, Order::Pilot { .. } | Order::Support(_))
            {
                if !matches!(b.actors[id].order, Order::Build(_) | Order::Repair(_) | Order::Dig(_) | Order::Reclaim(_)) {
                    idle_aim::follow(&mut self.units[id], &mut b.actors[id], self.turret_limits);
                }
                continue;
            }
            if !self.units[id].status.alive || !self.units[id].status.weapon {
                record_gate(&mut b.fire_trace, id, FireGate::Disabled);
                b.actors[id].charge = None;
                b.actors[id].burst = 0;
                b.actors[id].target = None;
                continue;
            }
            let kind = self.units[id].kind;
            if self.factory_launch_for_order(id, b.actors[id].order).is_some() {
                // The marker is not a firing target. Keep the visible turret
                // aligned with the chassis throughout the physical exit.
                let head = self.units[id].motion.drive.head;
                b.actors[id].turret = crate::turret::Traverse::new(head);
                self.units[id].update_pose(head);
                b.actors[id].target = None;
                continue;
            }
            if let Order::Support(target) = b.actors[id].order {
                let valid = target < self.units.len()
                    && target != id
                    && self.units[target].status.alive
                    && self.units[target].kind == RangeUnitKind::LightningTank
                    && self.allied(id) == self.allied(target);
                if !valid {
                    b.actors[id].order = Order::Hold;
                    self.units[id].motion.stop();
                    continue;
                }
                let from = self.units[id].motion.pos.map(Fx::to_int_trunc);
                let to = self.units[target].motion.pos.map(Fx::to_int_trunc);
                let dx = i64::from(to[0] - from[0]);
                let dz = i64::from(to[2] - from[2]);
                let distance = ((dx * dx + dz * dz) as u64).isqrt().max(1) as i64;
                if distance > i64::from(LINK_RANGE) {
                    idle_aim::follow(&mut self.units[id], &mut b.actors[id], self.turret_limits);
                    if self.tick % 20 == 0 || self.units[id].motion.destination.is_none() {
                        let back = 120i64.min(distance);
                        self.units[id].motion.queue_move(
                            &self.world,
                            [
                                to[0] - (dx * back / distance) as i32,
                                to[2] - (dz * back / distance) as i32,
                            ],
                        );
                    }
                } else {
                    if self.units[id].motion.destination.is_some() {
                        self.units[id].motion.stop();
                    }
                    if self.units[id].status.engine
                        && self.units[target].status.weapon
                        && self.tick % 2 == 0
                    {
                        let angle = fx::angle::atan2(
                            Fx::from_int(to[2] - from[2]).raw(),
                            Fx::from_int(to[0] - from[0]).raw(),
                        );
                        b.actors[id].turret.step(angle, self.turret_limits);
                        self.units[id].update_pose(b.actors[id].turret.angle);
                        let muzzle = weapons::muzzles(&self.units[id], b.actors[id].turret.angle)
                            .first()
                            .copied()
                            .unwrap_or_else(|| aim_point(&self.units[id]).map(Fx::from_int));
                        b.lightning.push(weapons::Trace {
                            from: muzzle,
                            to: aim_point(&self.units[target]).map(Fx::from_int),
                            hit: true,
                            serial: self.tick.wrapping_mul(31).wrapping_add(id as u32),
                        });
                    }
                }
                stop_active_beam(&mut b.actors[id], kind, self.tick);
                b.actors[id].target = None;
                continue;
            }
            // Each arm bearing traverses independently. A shared pitched
            // crown would move all sockets even when only one arm is aiming.
            if kind == RangeUnitKind::Walker04 {
                b.actors[id].elevation = crate::turret::Traverse::new(0);
                self.units[id].turret_pitch = 0;
                self.units[id].update_pose(b.actors[id].turret.angle);
            }
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
            if kind == RangeUnitKind::NuclearSilo && a.order.point().is_none() {
                record_gate(&mut b.fire_trace, id, FireGate::Hold);
                a.target = None;
                a.charge = None;
                continue;
            }
            if matches!(a.order, Order::Attack(i) if !hostile(i))
                || (a.order == Order::Move
                    && self.units[id].motion.destination.is_none()
                    && !b.autonomous)
            {
                a.order = Order::Auto;
            }
            if matches!(a.order, Order::Hold | Order::Rally(_) | Order::Dig(_) | Order::Reclaim(_) | Order::Build(_) | Order::Repair(_)) {
                if !matches!(a.order, Order::Build(_) | Order::Repair(_) | Order::Dig(_) | Order::Reclaim(_)) {
                    idle_aim::follow(&mut self.units[id], a, self.turret_limits);
                }
                if matches!(a.order, Order::Dig(_) | Order::Reclaim(_)) {
                    self.units[id].relax_commander_arms(a.turret.angle);
                }
                record_gate(&mut b.fire_trace, id, FireGate::Hold);
                if kind == RangeUnitKind::LightningTank {
                    stop_active_beam(a, kind, self.tick);
                }
                a.target = None;
                continue;
            }
            let target =
                if kind == RangeUnitKind::NuclearSilo || matches!(a.order, Order::Pilot { .. }) {
                    None // A manual point cannot inherit moving automatic targets.
                } else if let Order::Attack(i) = a.order {
                    hostile(i).then_some(i)
                } else {
                    (0..self.units.len())
                        .filter(|&i| hostile(i))
                        .min_by_key(|&i| (distance(i), i))
                };
            let target_distance = target.map(distance);
            let target_changed = a.target != target;
            if target_changed {
                if kind != RangeUnitKind::Spider || a.burst == 0 {
                    a.charge = None;
                }
                a.pursuit = 0;
            }
            a.target = target;
            // A tank loads for what it aims at (ADR-456).
            let Some(weapon) = effective_weapon(
                kind,
                a.weapon,
                a.order.point().is_some(),
                target.map(|id| self.units[id].kind),
                &b.tuning,
            ) else {
                record_gate(&mut b.fire_trace, id, FireGate::Hold);
                a.target = None;
                idle_aim::follow(&mut self.units[id], a, self.turret_limits);
                continue;
            };
            let mut range = attack_range(&b.tuning, kind, weapon, stream.range);
            if weapon == Weapon::Lightning {
                range = supported_lightning(support[id]).range;
            }
            // Borrow individual config fields independently of the mutable actor.
            let config = match weapon {
                Weapon::Blast => &b.shell_config,
                Weapon::Kinetic => &b.kinetic_config,
                Weapon::Canister => &b.canister_config,
                Weapon::Siege => &b.siege_config,
                Weapon::Nuclear => &b.nuclear_config,
                Weapon::Energy => &b.energy_config,
                Weapon::Annihilation => &b.anni_config,
                Weapon::MachineGun => &b.bullet_config,
                Weapon::Laser | Weapon::Lightning => &b.laser_config,
            };
            let mut p = if let Some(p) = a.order.point() {
                p
            } else if let Some(other) = target {
                aim_point(&self.units[other])
            } else {
                stop_active_beam(a, kind, self.tick);
                idle_aim::follow(&mut self.units[id], a, self.turret_limits);
                continue;
            };
            let mut target_velocity = [Fx::ZERO; 3];
            if let Some(other) = target.filter(|_| {
                !matches!(weapon, Weapon::Laser | Weapon::Lightning) && a.order.point().is_none()
            }) {
                let u = &self.units[other];
                let velocity = u.motion.velocity();
                target_velocity = [velocity.x, velocity.y, velocity.z];
                if !kind.artillery() {
                    let time = distance(other).sqrt().div(Fx::from_int(config.shell_speed));
                    p[0] += velocity.x.mul(time).to_int_trunc();
                    p[2] += velocity.z.mul(time).to_int_trunc();
                }
            }
            // Traverse independently of the hull and before entering range,
            // so a moving unit does not wait to begin slewing its turret.
            let artillery_aim = kind
                .artillery()
                .then(|| {
                    artillery::solve_moving(
                        &self.units[id],
                        p,
                        target_velocity,
                        weapon,
                        config,
                        loft(&b.tuning, weapon),
                    )
                })
                .flatten();
            if let Some(aim) = &artillery_aim {
                p = aim.target;
            }
            let dx = Fx::from_int(p[0]).sub(from[0]);
            let dz = Fx::from_int(p[2]).sub(from[2]);
            let angle = artillery_aim
                .as_ref()
                .map_or_else(|| fx::angle::atan2(dz.raw(), dx.raw()), |aim| aim.yaw);
            let tactical_distance = if a.order.point().is_some() {
                dx.mul(dx).add(dz.mul(dz))
            } else {
                target_distance.unwrap_or_else(|| dx.mul(dx).add(dz.mul(dz)))
            };
            let in_range = tactical_distance <= Fx::from_int(range).mul(Fx::from_int(range));
            let commander_facing = kind != RangeUnitKind::Commander || !in_range
                || self.units[id].commander_face(p);
            let tracking = (kind == RangeUnitKind::Spider)
                .then(|| motion::spider_tuning().laser_tracking)
                .flatten();
            let yaw_limits = tracking.map_or(self.turret_limits, |t| {
                crate::turret::Limits::of(t.yaw, crate::TICKS_PER_SECOND)
            });
            let travel_aim = !kind.emplacement() && !in_range
                && !matches!(a.order, Order::Attack(_) | Order::Pilot { .. })
                && a.order.point().is_none() && a.charge.is_none() && a.burst == 0;
            let mut walker_goals = None;
            if travel_aim {
                idle_aim::follow(&mut self.units[id], a, self.turret_limits);
            } else {
                if kind == RangeUnitKind::NuclearSilo {
                    // A silo launches freely into its ballistic arc; no traversing barrel.
                    a.turret.angle = angle;
                    a.turret.rate = 0;
                } else {
                    let desired = if kind == RangeUnitKind::Commander && !in_range {
                        self.units[id].motion.drive.head
                    } else { angle };
                    a.turret.step(desired, yaw_limits);
                }
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
                if kind == RangeUnitKind::Commander {
                    let aim = if in_range { p } else {
                        let head = self.units[id].motion.drive.head;
                        [from[0].add(fx::angle::cos(head).mul(Fx::from_int(300))).to_int_trunc(),
                         from[1].to_int_trunc() + 60,
                         from[2].add(fx::angle::sin(head).mul(Fx::from_int(300))).to_int_trunc()]
                    };
                    self.units[id].set_arm_targets([aim, aim, [0, 0, 0], [0, 0, 0]], a.turret.angle);
                } else if kind == RangeUnitKind::Walker04 {
                    // Begin slewing all four arms while approaching and charging,
                    // not only on the tick after the charge has completed.
                    let mut targets = [p; 4];
                    if !matches!(a.order, Order::Attack(_)) && a.order.point().is_none() {
                        let alternatives: Vec<[i32; 3]> = self.units.iter().enumerate()
                            .filter(|(other, unit)| unit.status.alive && Some(*other) != target
                                && self.allied(*other) != self.allied(id)
                                && { let dx = unit.motion.pos[0].sub(from[0]); let dz = unit.motion.pos[2].sub(from[2]);
                                    dx.mul(dx).add(dz.mul(dz)) <= Fx::from_int(range).mul(Fx::from_int(range)) })
                            .map(|(_, unit)| aim_point(unit)).collect();
                        for (arm, target) in targets.iter_mut().enumerate().skip(1) {
                            if !alternatives.is_empty() { *target = alternatives[(arm - 1) % alternatives.len()]; }
                        }
                    }
                    walker_goals = Some(targets);
                    self.units[id].set_arm_targets(targets, a.turret.angle);
                } else {
                    self.units[id].update_pose(a.turret.angle);
                }
            }
            completed("orders_pose");
            // Acquisition/AI/UI use horizontal actor-centre distance, not
            // a selected body cell or the projectile's predicted intercept.
            // Direct rounds need a clear line. Siege lasers can melt hard
            // cover, but should change position instead of indefinitely
            // burning foliage or firing into terrain that cannot melt.
            // Explicit point orders retain their ordinary physical contacts.
            completed("orders_misc");
            let blocked_direct = in_range
                && matches!(
                    weapon,
                    Weapon::MachineGun | Weapon::Kinetic | Weapon::Energy | Weapon::Laser | Weapon::Lightning
                )
                && !kind.emplacement()
                && a.order.point().is_none()
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
                stop_active_beam(a, kind, self.tick);
                a.charge = None;
                if kind.emplacement() {
                    continue;
                }
                // The player's commander defends from its position by default.
                // Only an explicit attack may turn a distant target into pursuit;
                // AI-controlled commanders retain their ordinary behaviour.
                if kind == RangeUnitKind::Commander && self.allied(id)
                    && !b.autonomous && a.order == Order::Auto {
                    continue;
                }
                // A move order owns its route. Track and fire at hostiles that
                // enter range, but never turn the move into an attack pursuit.
                if a.order == Order::Move || matches!(a.order, Order::Pilot { .. }) {
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
                    // A live route to a still usable firing point owns its
                    // destination. A brief blocked step asks navigation to
                    // repair that route, not to alternate the flank again.
                    let retained_point = self.units[id].motion.destination.filter(|old| {
                        if target_changed || road_goal.is_some() {
                            return false;
                        }
                        let x = i64::from(old[0]) - i64::from(p[0]);
                        let z = i64::from(old[1]) - i64::from(p[2]);
                        if x * x + z * z > i64::from(range) * i64::from(range) {
                            return false;
                        }
                        let muzzle_y = weapons::muzzles(&self.units[id], a.turret.angle)
                            .first()
                            .map_or(from[1].to_int_trunc(), |m| m[1].to_int_trunc());
                        self.firing_line_clear(id, p, *old, muzzle_y)
                    });
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
                    let dest = if let Some(old) = retained_point {
                        old
                    } else if blocked_direct || needs_detour && road_goal.is_none() {
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
                        // A fine search starts here while what the motion
                        // phase left of the tick's start allowance holds a
                        // whole start slice; past it, it is set up unwalked
                        // and the next tick's start shares walk it, reading
                        // the hulls near it then (ADR-691).
                        let slice = self.navigation.search_nodes_per_tick;
                        let walk = self.fine_start_left >= slice;
                        let others: Vec<_> = if walk {
                            self.units
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| *i != id)
                                .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let u = &mut self.units[id];
                        // Under way, the hull drives on while the new
                        // route is searched (ADR-462). A whole-map search
                        // begins with no work in the orders phase; the
                        // next tick's continuation walks it under the
                        // shared allowance (ADR-680).
                        u.motion.allow_coarse(Some(0));
                        u.motion.defer_fine(!walk);
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
                                    "plan_direct" | "plan_reverse" | "plan_hull_detour" => "orders_retarget_direct",
                                    "plan_fine" => "orders_retarget_fine",
                                    _ => unreachable!("unknown pursuit route phase"),
                                });
                            },
                        );
                        self.units[id].motion.allow_coarse(None);
                        if self.units[id].motion.walked_start() {
                            self.fine_start_left -= slice;
                        }
                        self.units[id].motion.defer_fine(false);
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
                let desired = if a.order.point().is_some() {
                    p
                } else {
                    spider_laser_aim(&self.world, muzzle, p, a.burst, tuning)
                };
                if let Some(t) = tracking {
                    let limit = t.elevation_limit_degrees * fx::angle::FULL / 360;
                    let pitch = spider_laser_pitch(&self.units[id], desired).clamp(-limit, limit);
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
            if matches!(a.order, Order::Pilot { fire: false, .. }) {
                record_gate(&mut b.fire_trace, id, FireGate::Hold);
                stop_active_beam(a, kind, self.tick);
                a.charge = None;
                continue;
            }

            if self.tick < a.next {
                record_gate(&mut b.fire_trace, id, FireGate::Reload);
                continue;
            }
            if kind != RangeUnitKind::Walker04
                && tracking.is_none()
                && (!a.turret.aligned(angle, yaw_limits)
                    || !commander_facing
                    || (kind == RangeUnitKind::Commander && self.units[id].arm_targets.is_some_and(|targets| {
                        (0..3).any(|axis| (i64::from(targets[0][axis]) - i64::from(p[axis])).abs() > 20)
                    }))
                    || artillery_aim.as_ref().is_some_and(|aim| {
                        !a.elevation.aligned(aim.pitch, self.turret_limits)
                            || !artillery::same_direction(a.turret.angle, aim.yaw)
                            || !artillery::same_direction(self.units[id].turret_pitch, aim.pitch)
                    }))
            {
                record_gate(&mut b.fire_trace, id, FireGate::Aiming);
                continue;
            }
            if weapon == Weapon::Lightning {
                record_gate(&mut b.fire_trace, id, FireGate::Beam);
                if let Some(from) = weapons::muzzles(&self.units[id], a.turret.angle)
                    .first()
                    .copied()
                {
                    let electric = supported_lightning(support[id]);
                    let trace = self.laser_ray(
                        from,
                        p,
                        Some(id),
                        &electric,
                        config,
                        (&mut completed, physics),
                    );
                    b.lightning.push(trace);
                    // At most three secondary contacts, on four ticks per
                    // burst. Every arc uses ordinary occlusion and shields.
                    if trace.hit && a.burst % 4 == 0 {
                        let mut end = trace.to;
                        let at = end.map(Fx::to_int_trunc);
                        let mut source = self.units.iter().enumerate().find_map(|(other, unit)| {
                            (other != id
                                && unit.status.alive
                                && self.allied(other) != self.allied(id)
                                && unit.solid(at))
                            .then_some(other)
                        });
                        let mut visited = vec![id];
                        let mut secondary = electric.clone();
                        // Secondary contacts reach 32 m; the primary weapon
                        // range remains independently authored in the tuning.
                        secondary.range = 320;
                        secondary.heat = electric.heat / 2;
                        secondary.work = electric.work / 2;
                        for _ in 0..3 {
                            let Some(previous) = source else {
                                break;
                            };
                            visited.push(previous);
                            let origin = end.map(Fx::to_int_trunc);
                            let next = self
                                .units
                                .iter()
                                .enumerate()
                                .filter(|(other, unit)| {
                                    unit.status.alive
                                        && !visited.contains(other)
                                        && self.allied(*other) != self.allied(id)
                                })
                                .filter_map(|(other, unit)| {
                                    let target = aim_point(unit);
                                    let d = (0..3)
                                        .map(|axis| {
                                            let v = i64::from(target[axis] - origin[axis]);
                                            v * v
                                        })
                                        .sum::<i64>();
                                    (d <= i64::from(secondary.range).pow(2))
                                        .then_some((d, other, target))
                                })
                                .min_by_key(|(distance, other, _)| (*distance, *other));
                            let Some((_, _, target)) = next else {
                                break;
                            };
                            let arc = self.laser_ray(
                                end,
                                target,
                                Some(previous),
                                &secondary,
                                config,
                                (&mut completed, physics),
                            );
                            end = arc.to;
                            b.lightning.push(arc);
                            if !arc.hit {
                                break;
                            }
                            let at = end.map(Fx::to_int_trunc);
                            source = self.units.iter().enumerate().find_map(|(other, unit)| {
                                (unit.status.alive
                                    && !visited.contains(&other)
                                    && self.allied(other) != self.allied(id)
                                    && unit.solid(at))
                                .then_some(other)
                            });
                        }
                    }
                    a.fired = a.fired.wrapping_add(1);
                    a.burst += 1;
                    if a.burst >= electric.burst_ticks {
                        a.burst = 0;
                        a.next = self.tick.saturating_add(electric.reload_ticks);
                    } else {
                        a.next = self.tick.saturating_add(1);
                    }
                }
                continue;
            }
            if kind == RangeUnitKind::RazlomT3 {
                record_gate(&mut b.fire_trace, id, FireGate::Beam);
                let tuning = motion::spider_tuning();
                let ready = *a.charge.get_or_insert(self.tick + tuning.charge_ticks);
                if self.tick < ready {
                    continue;
                }
                let muzzles = weapons::razlom_muzzles(&self.units[id], a.turret.angle);
                let Some(centre) = muzzles.get(3).copied() else {
                    record_gate(&mut b.fire_trace, id, FireGate::NoMuzzle);
                    continue;
                };
                let centre = [centre[0], Fx::from_int(p[1]), centre[2]];
                let direction = [
                    Fx::from_int(p[0]).sub(centre[0]),
                    Fx::ZERO,
                    Fx::from_int(p[2]).sub(centre[2]),
                ];
                let length = direction
                    .iter()
                    .fold(Fx::ZERO, |sum, v| sum.add(v.mul(*v)))
                    .sqrt()
                    .max(Fx::ONE);
                let velocity =
                    direction.map(|v| v.div(length).mul(Fx::from_int(b.tuning.razlom_wave.speed)));
                completed("orders_misc");
                b.rounds.push(Round {
                    owner: id,
                    weapon: Weapon::Laser,
                    pos: centre,
                    previous: centre,
                    velocity,
                    distance: Fx::from_int(range),
                });
                a.fired = a.fired.wrapping_add(1);
                a.burst = 0;
                a.charge = None;
                a.next = self.tick + b.tuning.razlom_wave.reload_ticks;
                continue;
            }
            if kind == RangeUnitKind::Walker04 {
                record_gate(&mut b.fire_trace, id, FireGate::Beam);
                let tuning = motion::spider_tuning();
                let ready = *a.charge.get_or_insert(self.tick + tuning.charge_ticks);
                if self.tick < ready {
                    continue;
                }
                let muzzles = weapons::walker_muzzles(&self.units[id]);
                if muzzles.is_empty() {
                    record_gate(&mut b.fire_trace, id, FireGate::NoMuzzle);
                    continue;
                }
                completed("orders_misc");
                let mut walker_laser = tuning.laser.clone();
                walker_laser.work = (walker_laser.work / 2).max(1);
                walker_laser.heat = (walker_laser.heat / 2).max(1);
                walker_laser.fracture_j = (walker_laser.fracture_j / 2).max(1);
                let targets = walker_goals.expect("walker goals are chosen before firing");
                let current = self.units[id].arm_targets.expect("walker arms are aimed before firing");
                let mut fired = false;
                for (arm, from) in muzzles {
                    let target = targets[arm];
                    if (0..3).any(|axis| (i64::from(current[arm][axis]) - i64::from(target[axis])).abs()
                        > i64::from(super::walker::tuning().arm_fire_error_units)) {
                        continue;
                    }
                    let axis = self.units[id].seg_frames[13 + arm * 4].x;
                    let delta = phys::V3::new(Fx::from_int(target[0]).sub(from[0]),
                        Fx::from_int(target[1]).sub(from[1]), Fx::from_int(target[2]).sub(from[2]));
                    let length = delta.len_sq().sqrt();
                    if length <= Fx::ONE || axis.dot(delta).div(length) < Fx::from_ratio(998, 1000) {
                        continue;
                    }
                    // Never send a ray into terrain while an arm is still slewing
                    // or its independently chosen target is behind the ground.
                    if target[0] < 0 || target[2] < 0 || target[0] >= self.world.nx as i32
                        || target[2] >= self.world.nz as i32
                        || target[1] < self.world.column_ceiling(target[0] as u32, target[2] as u32) as i32
                        || !voxel::los::line_of_sight(&self.world,
                            (from[0].to_int_trunc(), from[1].to_int_trunc(), from[2].to_int_trunc()),
                            (target[0], target[1], target[2])).clear {
                        continue;
                    }
                    let length = length.add(Fx::from_int(4)).min(Fx::from_int(walker_laser.range));
                    let mut arm_laser = walker_laser.clone();
                    arm_laser.range = length.to_int_trunc().max(1);
                    let end = [from[0].add(axis.x.mul(length)).to_int_trunc(),
                        from[1].add(axis.y.mul(length)).to_int_trunc(),
                        from[2].add(axis.z.mul(length)).to_int_trunc()];
                    let trace = self.laser_ray(
                        from,
                        end,
                        Some(id),
                        &arm_laser,
                        config,
                        (&mut completed, physics),
                    );
                    b.walker_lasers.push(trace);
                    fired = true;
                }
                if !fired {
                    record_gate(&mut b.fire_trace, id, FireGate::Aiming);
                    continue;
                }
                a.fired = a.fired.wrapping_add(1);
                a.burst += 1;
                if a.burst >= tuning.laser.burst_ticks {
                    a.burst = 0;
                    a.charge = None;
                    a.next = self.tick + tuning.laser.reload_ticks;
                } else {
                    a.next = self.tick + tuning.laser.interval_ticks.max(1);
                }
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
                    let samples = if tuning.laser.interval_ticks == 1 {
                        modding::clock::stream_samples(self.tick)
                    } else {
                        1
                    }
                    .min(tuning.laser.burst_ticks.saturating_sub(a.burst).max(1));
                    // Ablation is nonlinear: a second sample must see the
                    // hole the first made, rather than waste double work on
                    // the same surface. Publish only the last visual trace.
                    let mut last = None;
                    for _ in 0..samples {
                        last = Some(self.laser_ray(
                            from,
                            p,
                            Some(id),
                            &tuning.laser,
                            config,
                            (&mut completed, physics),
                        ));
                    }
                    completed("orders_laser");
                    b.lasers.push(last.expect("at least one laser sample"));
                    a.fired = a.fired.wrapping_add(samples);
                    a.burst += samples;
                    if a.burst >= tuning.laser.burst_ticks {
                        a.burst = 0;
                        a.charge = None;
                        a.next = self.tick + tuning.laser.reload_ticks;
                    } else {
                        let next = modding::clock::stream_ticks(
                            a.burst.saturating_mul(tuning.laser.interval_ticks),
                        );
                        let previous = modding::clock::stream_ticks(
                            a.burst
                                .saturating_sub(1)
                                .saturating_mul(tuning.laser.interval_ticks),
                        );
                        a.next = self.tick + next.saturating_sub(previous).max(1);
                    }
                }
                continue;
            }
            let shell = matches!(
                kind,
                RangeUnitKind::Tank
                    | RangeUnitKind::EidolonTank
                    | RangeUnitKind::Artillery
                    | RangeUnitKind::ArtilleryT2
                    | RangeUnitKind::ArtilleryTurretT2
                    | RangeUnitKind::ArtilleryTurretT3
                    | RangeUnitKind::NuclearSilo
                    | RangeUnitKind::TurretT1
                    | RangeUnitKind::TurretT2
            );
            let muzzles = weapons::muzzles(&self.units[id], a.turret.angle);
            let count = if weapon == Weapon::Canister {
                b.tuning.canister.pellets
            } else if kind == RangeUnitKind::EidolonTank {
                muzzles.len().min(2)
            } else if shell {
                1
            } else {
                muzzles.len().min(2)
            };
            if shell {
                let charge = *a.charge.get_or_insert(self.tick + config.charge_ticks);
                if self.tick < charge {
                    record_gate(&mut b.fire_trace, id, FireGate::Charging);
                    continue;
                }
            }
            let pending_full = matches!(weapon, Weapon::Kinetic | Weapon::MachineGun)
                && !self.solid_flight_room(&b.rounds, count);
            if muzzles.is_empty() {
                record_gate(&mut b.fire_trace, id, FireGate::NoMuzzle);
                continue;
            }
            if !b.autonomous && !weapon.bursts() && b.rounds.len() + count > b.tuning.max_rounds {
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
                artillery_velocity: if kind == RangeUnitKind::NuclearSilo {
                    Some(artillery::velocity(
                        muzzles[0],
                        p,
                        weapon,
                        config,
                        loft(&b.tuning, weapon),
                    ))
                } else {
                    kind.artillery().then(|| {
                        artillery::along_barrel(
                            &self.units[id],
                            artillery::velocity(
                                muzzles[0],
                                p,
                                weapon,
                                config,
                                loft(&b.tuning, weapon),
                            ),
                        )
                    })
                },
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
        // In a battle each side owns half of the wave ceiling (ADR-649).
        // The ceiling protects the tick, and a shared one decided the fight:
        // two salvos of one side held it for as long as their waves lived,
        // and the other side's canister never fired. A side's salvo queues
        // only behind its own older ones.
        let battle_sides = self.battle_mode();
        let side_of = |owner: usize| usize::from(battle_sides && !self.allied(owner));
        let half = b.tuning.max_waves / 2;
        let mut by_side = [0usize; 2];
        if battle_sides {
            for w in &b.waves {
                if let Some(allied) = w.side {
                    by_side[usize::from(!allied)] += 1;
                }
            }
            for r in b.rounds.iter().filter(|r| r.bursts()) {
                by_side[side_of(r.owner)] += 1;
            }
        }
        let mut blocked = [false; 2];
        for shot in bursts {
            let side = side_of(shot.owner);
            let gate = if b.autonomous {
                None
            } else if blocked[side] {
                Some(FireGate::BurstQueue)
            } else if reserved + shot.count > b.tuning.max_waves
                || (battle_sides && by_side[side] + shot.count > half)
            {
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
                blocked[side] = true;
            } else {
                reserved += shot.count;
                by_side[side] += shot.count;
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
            Weapon::Siege => &b.siege_config,
            Weapon::Nuclear => &b.nuclear_config,
            Weapon::Energy => &b.energy_config,
            Weapon::Annihilation => &b.anni_config,
            Weapon::MachineGun => &b.bullet_config,
            Weapon::Laser | Weapon::Lightning => unreachable!("beam is not a projectile"),
        };
        let a = &mut b.actors[id];
        record_gate(&mut b.fire_trace, id, FireGate::Fired);
        let speed = config.shell_speed;
        for barrel in 0..count {
            let muzzle = muzzles[if shell && kind != RangeUnitKind::EidolonTank {
                0
            } else {
                barrel
            }];
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
            time = time.mul(Fx::from_ratio(loft(&b.tuning, weapon), 1000));
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
                distance: if matches!(weapon, Weapon::Canister | Weapon::Siege | Weapon::Nuclear) {
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
        if weapon == Weapon::Nuclear {
            a.order = Order::Hold;
            a.target = None;
        }
        a.charge = None;
        if shell {
            a.next = tick
                + if weapon == Weapon::Canister {
                    b.tuning.canister.reload_ticks
                } else if weapon == Weapon::Nuclear {
                    b.tuning.nuclear.reload_ticks
                } else if weapon == Weapon::Siege {
                    b.tuning.siege.reload_ticks
                } else if kind == RangeUnitKind::EidolonTank {
                    b.tuning
                        .eidolon_reload_ticks
                        .unwrap_or(b.tuning.tank_reload_ticks)
                } else if kind == RangeUnitKind::TurretT2 {
                    b.tuning
                        .turret_t2_reload_ticks
                        .unwrap_or(b.tuning.tank_reload_ticks)
                } else {
                    b.tuning.tank_reload_ticks
                };
        } else {
            let remaining = stream.burst_ticks.saturating_sub(a.burst);
            let previous = a.burst;
            a.burst += stream.interval_ticks;
            a.next = tick
                + if a.burst >= stream.burst_ticks {
                    a.burst = 0;
                    modding::clock::stream_ticks(remaining.saturating_sub(1)) + stream.reload_ticks
                } else {
                    modding::clock::stream_ticks(a.burst)
                        .saturating_sub(modding::clock::stream_ticks(previous))
                        .max(1)
                };
        }
    }
    fn battle_flight(&mut self, b: &mut Battle) {
        let dt = Fx::from_ratio(1, crate::TICKS_PER_SECOND);
        let stream = self.streams.tuning.machine_gun.clone();
        let boxes: Vec<_> = self.units.iter().map(|u| u.bounds()).collect();
        let mut razlom_work = b.tuning.razlom_wave.cells_per_tick;
        for mut r in std::mem::take(&mut b.rounds) {
            if r.weapon == Weapon::Laser && self.units[r.owner].kind == RangeUnitKind::RazlomT3 {
                if let Some(round) = self.step_razlom_wave(b, r, &mut razlom_work) {
                    b.rounds.push(round);
                }
                continue;
            }
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
                        from: prev,
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
                    let mut energy = b.material_energy_mj(r.weapon);
                    self.matter.begin_mj(energy, config.fragment_box);
                    self.matter.remaining_mj = old;
                    let reserved = self.upheaval.reserve(&self.world, at, &config, energy);
                    energy -= reserved;
                    let mut wave_config = config;
                    wave_config.material_energy_j = wave_config
                        .material_energy_j
                        .saturating_sub((reserved / 1000) as u32);
                    let side = (r.owner < self.units.len()).then(|| self.allied(r.owner));
                    let mut wave = Explosion::begin_impact(
                        &self.world,
                        at,
                        prev,
                        &wave_config,
                        &self.units,
                        self.matter.tuning.body_energy_pct,
                    );
                    if r.weapon == Weapon::Nuclear {
                        wave.nuclear_ground = true;
                        wave.material_budget(energy, b.tuning.nuclear.body_energy_pct);
                    }
                    b.waves.push(Burst {
                        side,
                        weapon: r.weapon,
                        wave,
                        energy,
                    });
                    self.heat.deposit(
                        &self.world,
                        &self.matter.economy.mats,
                        at,
                        config.heat_units,
                        config.heat_per_cell,
                    );
                    self.water.absorb_radial_heat(
                        &self.world,
                        at,
                        config.heat_units,
                        config.heat_per_cell,
                    );
                    b.impacts.push(Impact {
                        at,
                        from: prev,
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
                        from: prev,
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
    fn battle_waves(
        &mut self,
        b: &mut Battle,
        completed: &mut impl FnMut(&'static str),
        physics: &(impl Fn(usize, &'static str) + Sync),
    ) {
        use rayon::prelude::*;
        let autonomous = b.autonomous;
        let n = b.waves.len().max(1);
        // A growing old crater must not consume every newly freed fragment
        // slot before a newer hit reaches its hull. Rotate service, not the
        // canonical wave array: snapshots and stable wave identity keep their
        // order, and the saved tick determines the next turn after loading.
        let first = self.tick as usize % n;
        let order: Vec<usize> = (0..b.waves.len()).map(|k| (first + k) % n).collect();
        let Battle {
            waves,
            tuning,
            energy_config,
            anni_config,
            canister_config,
            siege_config,
            nuclear_config,
            shell_config,
            wave_held,
            ..
        } = b;
        let config_of = |weapon: Weapon| match weapon {
            Weapon::Energy => &*energy_config,
            Weapon::Annihilation => &*anni_config,
            Weapon::Canister => &*canister_config,
            Weapon::Siege => &*siege_config,
            Weapon::Nuclear => &*nuclear_config,
            _ => &*shell_config,
        };
        // Each wave captures and advances by its own preset's budget, in
        // turn, while the tick's ceilings over all waves last (ADR-681).
        // A wave past a ceiling waits for the next tick.
        *wave_held = [0; 2];
        let mut capture_left = tuning.wave_capture_ceiling;
        let mut work_left = tuning.wave_work_ceiling;
        let mut contact_left = tuning.wave_contact_ceiling;
        if autonomous {
            // Every admitted AI-battle wave gets its authored share. Backlog
            // may cost a frame, but a full wave queue cannot starve new hits.
            let capture = waves
                .iter()
                .filter(|w| w.wave.capturing())
                .fold(0usize, |n, w| {
                    n.saturating_add(config_of(w.weapon).capture_cells_per_tick)
                });
            let work = waves
                .iter()
                .filter(|w| !w.wave.capturing())
                .fold(0usize, |n, w| {
                    n.saturating_add(config_of(w.weapon).wave_work_per_tick)
                });
            capture_left = capture_left.max(capture);
            work_left = work_left.max(work);
            contact_left = contact_left.max(work);
        }
        let mut slices = vec![0; waves.len()];
        for &i in &order {
            let w = &waves[i];
            if w.wave.capturing() {
                let budget = config_of(w.weapon).capture_cells_per_tick.min(capture_left);
                capture_left -= budget;
                wave_held[0] += usize::from(budget == 0);
                slices[i] = budget;
            }
        }
        // Each wave captures its own grid from the same world, hulls and
        // dents, by the slice handed out above: the waves capture side by
        // side, each also in parallel within its slice (ADR-311).
        if waves.iter().any(|w| w.wave.capturing()) {
            let (world, units, dents) = (&self.world, &self.units, &self.surface_work);
            waves
                .par_iter_mut()
                .zip(slices.par_iter())
                .for_each(|(w, &slice)| {
                    if w.wave.capturing() {
                        physics(6, "physics_wave_capture_begin");
                        w.wave
                            .capture(world, config_of(w.weapon), units, dents, slice);
                        physics(6, "physics_wave_capture_end");
                    }
                });
            completed("battle_capture");
        }
        let mut pops = vec![0; waves.len()];
        let mut budgets = vec![0; waves.len()];
        for &i in &order {
            let w = &waves[i];
            if !w.wave.capturing() {
                let own = config_of(w.weapon).wave_work_per_tick;
                pops[i] = own.min(work_left);
                work_left -= pops[i];
                budgets[i] = own.min(contact_left);
                contact_left -= budgets[i];
                wave_held[1] += usize::from(pops[i] == 0 || budgets[i] == 0);
            }
        }
        // Each frontier reads its own captured grid alone: the waves advance
        // theirs in parallel, every one by its own budget, and the contacts
        // below meet the same hits in the same order as when each wave
        // advanced just before its own contacts. Then each wave reads ahead
        // the matter under the cells its contacts visit, in the same task:
        // the world's voxels do not change while the waves step (ADR-681),
        // and a wave with a short frontier reads while another advances.
        {
            let (world, matter, units) = (&self.world, &self.matter, &self.units);
            waves
                .par_iter_mut()
                .zip(pops.par_iter().zip(budgets.par_iter()))
                .for_each(|(w, (&pop, &budget))| {
                    let observe = |event| physics(6, event);
                    w.wave
                        .advance_frontier_observed(pop, world, units, &observe);
                    let spent = Some((matter, config_of(w.weapon), w.energy));
                    w.wave
                        .read_contacts_observed(world, budget, units, spent, &observe)
                });
        }
        completed("battle_frontier");
        // No physics or claim confirmation runs between these atomic batches.
        // Component commits consume exactly their reservations; terrain claims
        // hold one slot per connected component of their group (ADR-659).
        // Recount at the next phase/tick.
        self.matter.hull_reserve = tuning.hull_piece_reserve;
        self.matter.ground_waits = tuning.ground_waits_for_pool;
        let mut room = None;
        for &i in &order {
            let w = &mut waves[i];
            if w.wave.capturing() {
                continue;
            }
            let config = config_of(w.weapon);
            let old = self.matter.remaining_mj;
            self.matter.remaining_mj = w.energy;
            self.matter.fragment_size(config.fragment_box);
            // Only annihilation consumes removed-cell deltas. Ordinary
            // waves need neither this full roster walk nor its allocation.
            let before = (config.material_energy_j == 0).then(|| {
                self.units
                    .iter()
                    .map(|u| (u.removed.len(), u.seg_removed.len()))
                    .collect::<Vec<_>>()
            });
            let room = room.get_or_insert_with(|| self.matter.claim_room());
            physics(6, "physics_wave_apply_begin");
            w.wave.step_with_room_observed::<true>(
                &mut self.world,
                budgets[i],
                &mut self.units,
                &mut self.surface_work,
                &mut self.surface_changed,
                config,
                Some(&mut self.matter),
                Some(room),
                &|event| physics(6, event),
            );
            physics(6, "physics_wave_apply_end");
            if let Some(before) = before {
                for (id, (u, (root, seg))) in self.units.iter().zip(before).enumerate() {
                    self.matter
                        .annihilate_removed(id, u, root, seg);
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
        waves.retain(|w| w.wave.working());
    }
}

#[cfg(test)]
#[path = "range_wave_admission_tests.rs"]
mod wave_admission_tests;

#[cfg(test)]
#[path = "range_wave_budget_tests.rs"]
mod wave_budget_tests;

#[cfg(test)]
#[path = "range_foundation_tests.rs"]
mod foundation_tests;
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

#[cfg(test)]
#[path = "range_battle_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "range_coarse_continuation_tests.rs"]
mod coarse_continuation_tests;

#[cfg(test)]
#[path = "range_instant_blast_tests.rs"]
mod instant_blast_tests;

#[cfg(test)]
#[path = "range_attack_tests.rs"]
mod attack_tests;
