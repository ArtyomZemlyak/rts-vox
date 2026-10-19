//! Owned presentation data. Queues, controllers and references to the
//! authoritative world do not cross this boundary. Export is deliberately fresh:
//! commands and setup edits may change the world without advancing its tick.
use super::{
    battle::{Order, Phase, Weapon},
    BlastRange, RangeUnit, RangeUnitKind,
};
use fx::Fx;
use voxel::unit::Status;

#[path = "range_parameters.rs"]
mod parameters;
pub use parameters::{MotionParameters, Parameters};

#[path = "range_geometry_view.rs"]
pub mod geometry;

#[path = "range_terrain_view.rs"]
pub mod terrain;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Integrity {
    pub core: Fx,
    pub engine: Fx,
    pub drive: Fx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Slot identity is scoped by the containing View's generation.
    pub kind: RangeUnitKind,
    pub allied: bool,
    pub status: Status,
    pub integrity: Integrity,
    pub position: [Fx; 3],
    pub weapon: Option<Weapon>,
    /// None means no battle actor; Some(None) means automatic loadout.
    pub loadout: Option<Option<Weapon>>,
    pub attack_range: Option<i32>,
    pub order: Option<Order>,
    pub reload_until: Option<u32>,
    pub charge_until: Option<u32>,
    pub patrol_laps: Option<u32>,
    /// Matter in the unit's tank and its capacity; `None` without a tank
    /// (ADR-672, ADR-673).
    pub tank: Option<[u64; 2]>,
    /// The digger is away hauling to a dock.
    pub hauling: bool,
}
impl Unit {
    fn from_unit(unit: &RangeUnit, allied: bool) -> Self {
        Self {
            kind: unit.kind,
            allied,
            status: unit.status,
            integrity: Integrity {
                core: unit.integrity.core,
                engine: unit.integrity.engine,
                drive: unit.integrity.drive,
            },
            position: unit.motion.pos,
            weapon: None,
            loadout: None,
            attack_range: None,
            order: None,
            reload_until: None,
            charge_until: None,
            patrol_laps: (!unit.motion.route.is_empty()).then_some(unit.motion.laps),
            tank: None,
            hauling: false,
        }
    }
}

/// One coherent publication for roster, status and map markers. This is not a
/// save format or the complete geometry/terrain model required by the renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub generation: u64,
    pub tick: u32,
    pub size: [u32; 3],
    pub battle: bool,
    pub phase: Phase,
    pub autonomous: bool,
    pub auto_attack: bool,
    pub attack: Option<usize>,
    pub patrols_running: bool,
    pub delivery: modding::explosion::Delivery,
    pub counts: [usize; 2],
    pub base_integrity: Option<[i32; 2]>,
    pub ready: bool,
    pub gun: Unit,
    pub units: Vec<Unit>,
    /// Each side's store, allied first: matter held and its cap; `None`
    /// without a bank (ADR-673).
    pub stores: Option<[[u64; 2]; 2]>,
}
impl View {
    pub fn new(world: &BlastRange, generation: u64) -> Self {
        let mut view = Self {
            generation,
            tick: world.tick,
            size: world.size(),
            battle: world.battle_mode(),
            phase: world.battle.phase,
            autonomous: world.battle.autonomous,
            auto_attack: world.auto_attack,
            attack: world.attack,
            patrols_running: world.patrols_running,
            delivery: world.config.delivery,
            counts: [0; 2],
            base_integrity: None,
            ready: false,
            gun: Unit::from_unit(&world.gun, true),
            units: Vec::with_capacity(world.units.len()),
            stores: None,
        };
        view.update(world, generation);
        view
    }

    /// Reuses the roster allocation, but does not use tick as an invalidation
    /// key. The caller publishes only after this synchronous export completes.
    pub fn update(&mut self, world: &BlastRange, generation: u64) {
        self.generation = generation;
        self.tick = world.tick;
        self.size = world.size();
        self.battle = world.battle_mode();
        self.phase = world.battle.phase;
        self.autonomous = world.battle.autonomous;
        self.auto_attack = world.auto_attack;
        self.attack = world.attack;
        self.patrols_running = world.patrols_running;
        self.delivery = world.config.delivery;
        self.counts = [0; 2];
        self.base_integrity = world.battle_base_integrity();
        self.ready = world.ready();
        self.stores = world
            .side_store(true)
            .zip(world.side_store(false))
            .map(|(a, e)| [[a.0, a.1], [e.0, e.1]]);
        self.gun = Unit::from_unit(&world.gun, true);
        self.units.clear();
        for (id, unit) in world.units.iter().enumerate() {
            let mut item = Unit::from_unit(unit, world.allied(id));
            if item.status.alive {
                self.counts[usize::from(!item.allied)] += 1;
            }
            item.weapon = world.battle_weapon(id);
            item.loadout = world.battle.actors.get(id).map(|actor| actor.weapon);
            item.attack_range = world.battle_attack_range(id);
            item.order = world.battle.actors.get(id).map(|actor| actor.order);
            item.reload_until = world.battle_reload_until(id);
            item.charge_until = world.battle_charge(id);
            item.tank = world.tank_fill(id).map(|(held, cap)| [held, cap]);
            item.hauling = world.hauling(id);
            self.units.push(item);
        }
    }

    /// Logical owned payload, excluding allocator metadata and spare capacity.
    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.units.len() * std::mem::size_of::<Unit>()
    }

    pub fn selected_allies(&self, controlled: Option<usize>, selected: &[usize]) -> Vec<usize> {
        let valid = |id: usize| {
            self.units
                .get(id)
                .is_some_and(|u| u.allied && u.status.alive)
        };
        if selected.is_empty() {
            controlled.filter(|&id| valid(id)).into_iter().collect()
        } else {
            selected.iter().copied().filter(|&id| valid(id)).collect()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shield {
    pub radius: i32,
    pub charge: u32,
    pub down: u32,
}

/// On-demand inspection, separate from the cheap per-frame roster. Counting
/// resting debris visits retained fragments; callers request this only while
/// inspecting those statistics. No fragment geometry is copied or retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub generation: u64,
    pub tick: u32,
    pub shield_capacity: u32,
    pub shields: Vec<Shield>,
    pub fragments: usize,
    pub resting_fragments: usize,
    pub bodies: usize,
    pub stream_range: Option<i32>,
    pub stream_muzzles: usize,
}
impl Inspection {
    pub fn new(world: &BlastRange, generation: u64) -> Self {
        let mut view = Self {
            generation,
            tick: world.tick,
            shield_capacity: 0,
            shields: Vec::with_capacity(world.shields.len()),
            fragments: 0,
            resting_fragments: 0,
            bodies: 0,
            stream_range: None,
            stream_muzzles: 0,
        };
        view.update(world, generation);
        view
    }

    pub fn update(&mut self, world: &BlastRange, generation: u64) {
        use modding::explosion::Delivery;
        self.generation = generation;
        self.tick = world.tick;
        self.shield_capacity = world.shield_tuning().capacity.max(1);
        self.shields.clear();
        self.shields.extend(world.shields.iter().map(|s| Shield {
            radius: s.radius,
            charge: s.charge,
            down: s.down,
        }));
        self.fragments = world.matter.debris.fragments.len();
        self.resting_fragments = world
            .matter
            .debris
            .fragments
            .iter()
            .filter(|f| f.resting)
            .count();
        self.bodies = world.bodies.list.len();
        self.stream_range = match world.config.delivery {
            Delivery::Flame | Delivery::MachineGun | Delivery::Laser => {
                Some(world.gun_attack_range())
            }
            Delivery::Blast | Delivery::Kinetic => None,
        };
        self.stream_muzzles =
            if !world.battle_mode() && world.config.delivery == Delivery::MachineGun {
                world.stream_muzzles().len()
            } else {
                0
            };
    }

    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.shields.len() * std::mem::size_of::<Shield>()
    }
}

#[cfg(test)]
#[path = "range_read_model_tests.rs"]
mod tests;
