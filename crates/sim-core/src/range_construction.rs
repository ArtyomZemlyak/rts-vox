use super::{battle, BlastRange, RangeUnit};
use crate::matter::{Amount, ContainerId, Kind};
use fx::blob::{Blob, Scan};
use fx::Fx;
use modding::range::{RangeUnitDef, RangeUnitKind, MAX_RANGE_UNITS};
use std::collections::{BTreeMap, VecDeque};

#[cfg(test)]
#[path = "range_construction_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "range_economy_followup_tests.rs"]
mod economy_followup_tests;

// Legacy aliases remain for historical fixtures. Runtime reads captured RON.
#[cfg(test)] const COMMANDER_PRINT_RATE: Amount = 43_175;
#[cfg(test)] const COMMANDER_MATTER_PER_SECOND: Amount = 172_700;
#[cfg(test)] const COMMANDER_BUILD_DEMAND: u64 = 1_000;
use modding::range_economy::Balance;

fn per_second_rate(rate: Amount, tick: u32) -> Amount {
    let hz = crate::TICKS_PER_SECOND as u64;
    let t = u128::from(tick);
    (((t + 1) * u128::from(rate) / u128::from(hz)) - (t * u128::from(rate) / u128::from(hz))) as Amount
}

pub fn energy_generation(kind: RangeUnitKind) -> u64 { Balance::stock().generation(kind) }
fn active_generation(unit: &RangeUnit, rules: &Balance) -> u64 {
    if matches!(unit.kind, RangeUnitKind::PowerPlantT1 | RangeUnitKind::PowerPlantT2 | RangeUnitKind::PowerPlantT3)
        && !unit.status.engine { 0 } else { rules.generation(unit.kind) }
}
pub fn commander_build_energy_estimate(matter: Amount) -> u64 {
    let rules = Balance::stock();
    (u128::from(matter) * u128::from(rules.print_power) * crate::TICKS_PER_SECOND as u128)
        .div_ceil(u128::from(rules.print_per_second)).min(u128::from(u64::MAX)) as u64
}
// Local supply keeps a small working buffer in the commander's tank; it
// does not teleport a side's complete stock into a mobile unit.
pub const COMMANDER_SUPPLY_REACH: i64 = 400;


/// Distribute sub-voxel work over simulation ticks. Flooring each tick on
/// its own would permanently strand the last cell during an energy deficit.
fn powered_work(work: Amount, generation: u64, demand: u64, density: Amount, tick: u32) -> Amount {
    if demand == 0 || density == 0 { return 0; }
    let numerator = u128::from(work) * u128::from(generation.min(demand));
    let denominator = u128::from(demand) * u128::from(density);
    let at = u128::from(tick);
    let cells = (at + 1) * numerator / denominator - at * numerator / denominator;
    (cells as Amount).saturating_mul(density)
}
const FLY_SPEED: i32 = 8;
const FLY_COMPACT_SPEED: i32 = 12;
const STEEL: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leg {
    Idle,
    ToSource,
    /// Loading paid material at the station pad; remaining simulation ticks.
    Loading(u8),
    ToSite,
    Returning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    pub id: u64,
    pub material: u16,
    pub container: ContainerId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Drone {
    home: u16,
    site: Option<u16>,
    want: Amount,
    leg: Leg,
    cargo: Option<Batch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Site {
    kind: RangeUnitKind,
    allied: bool,
    at: [i32; 3],
    required: Amount,
    delivered: Amount,
    reserved: Amount,
    quota: u8,
    unit: Option<u16>,
}

fn site_quantum(site: &Site) -> Amount {
    // Stock parts of one construction template share their voxel scale and
    // structural grade. Delivery is intrinsic material, not 785-IU tokens.
    site.required / (construction_geometry(site.kind).len() as u64).max(1)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteView {
    pub id: usize,
    pub kind: RangeUnitKind,
    pub at: [i32; 3],
    pub required: Amount,
    pub delivered: Amount,
    pub quota: u8,
    pub assigned: usize,
    pub complete: bool,
    pub allied:bool,
    pub unit:Option<usize>,
    pub cancelled:bool,
    pub destroyed:bool,
}

/// Lightweight presentation data for unfinished sites, without drone counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressView {
    pub allied: bool,
    pub at: [i32; 3],
    pub unit: Option<usize>,
    pub required: Amount,
    pub delivered: Amount,
}

/// A visual cue for a paid printing step. Not part of saved simulation state.
#[derive(Clone, Copy, Debug)]
pub struct PrintTrace {
    pub actor: usize,
    pub impact: [Fx; 3],
}

#[derive(Default)]
pub struct Construction {
    sites: Vec<Site>,
    drones: BTreeMap<u16, Drone>,
    next_batch: u64,
    pub delivered: Amount,
    pub trips: u64,
    pub lost: Amount,
    print_traces: Vec<PrintTrace>,
    rules: Option<Balance>,
}

impl Construction {
    fn home_point(&self, r: &BlastRange, home: u16, id: u16) -> [i32;3] {
        let pad=if r.economic_rules().queued_crews {
            self.drones.range(..id).filter(|(&other,d)| d.home==home && r.units[usize::from(other)].status.alive).count() as u16
        } else { id };
        drone_home_point(&r.units,usize::from(home),pad,r.size)
    }
    fn rules(&self) -> &Balance { self.rules.as_ref().unwrap_or_else(|| Balance::legacy()) }
    pub(super) fn set_rules(&mut self, rules: Option<Balance>) { self.rules = rules; }
    pub fn new(units: &[RangeUnit], sides: &[bool]) -> Self {
        let mut c = Self::default();
        for (id, u) in units.iter().enumerate() {
            if u.kind == RangeUnitKind::SkobaDrone {
                c.drones.insert(
                    id as u16,
                    Drone {
                        home: nearest_home(units, sides, id) as u16,
                        site: None,
                        want: 0,
                        leg: Leg::Idle,
                        cargo: None,
                    },
                );
            }
        }
        c
    }

    pub fn push_unit(&mut self, id: usize, units: &[RangeUnit], sides: &[bool]) {
        if units[id].kind == RangeUnitKind::SkobaDrone {
            self.drones.insert(
                id as u16,
                Drone {
                    home: nearest_home(units, sides, id) as u16,
                    site: None,
                    want: 0,
                    leg: Leg::Idle,
                    cargo: None,
                },
            );
        }
    }

    fn register_new_units(&mut self, r: &BlastRange, first: usize) {
        if !r.units[first..].iter().any(|u| u.kind == RangeUnitKind::SkobaDrone) { return; }
        let sides: Vec<_> = (0..r.units.len()).map(|id| r.allied(id)).collect();
        for id in first..r.units.len() {
            self.push_unit(id, &r.units, &sides);
        }
    }

    pub fn sites(&self) -> Vec<SiteView> {
        self.sites
            .iter()
            .enumerate()
            .map(|(id, s)| SiteView {
                id,
                kind: s.kind,
                at: s.at,
                required: s.required,
                delivered: s.delivered,
                quota: s.quota,
                assigned: self
                    .drones
                    .values()
                    .filter(|d| d.site == Some(id as u16))
                    .count(),
                complete: s.delivered == s.required,
                allied:s.allied,unit:s.unit.map(usize::from),cancelled:false,destroyed:false,
            })
            .collect()
    }

    pub fn cargo(&self, id: usize) -> Option<Batch> {
        self.drones.get(&(id as u16))?.cargo
    }

    pub fn leg(&self, id: usize) -> Option<Leg> {
        Some(self.drones.get(&(id as u16))?.leg)
    }

    pub fn active(&self) -> bool {
        !self.sites.is_empty()
            || self
                .drones
                .values()
                .any(|d| d.cargo.is_some() || d.site.is_some())
            || self.delivered > 0
            || self.trips > 0
            || self.lost > 0
    }

    pub fn save(&self, b: &mut Blob) {
        b.u64(self.next_batch);
        b.u64(self.delivered);
        b.u64(self.trips);
        b.u64(self.lost);
        b.u32(self.sites.len() as u32);
        for s in &self.sites {
            b.u8(s.kind.index() as u8);
            b.bool(s.allied);
            for v in s.at {
                b.i32(v);
            }
            b.u64(s.required);
            b.u64(s.delivered);
            b.u64(s.reserved);
            b.u8(s.quota);
            b.bool(s.unit.is_some());
            if let Some(id) = s.unit {
                b.u16(id);
            }
        }
        b.u32(self.drones.len() as u32);
        for (&id, d) in &self.drones {
            b.u16(id);
            b.u16(d.home);
            b.bool(d.site.is_some());
            if let Some(site) = d.site {
                b.u16(site);
            }
            b.u64(d.want);
            b.u8(match d.leg {
                Leg::Idle => 0,
                Leg::ToSource => 1,
                Leg::ToSite => 2,
                Leg::Returning => 3,
                Leg::Loading(ticks) => 3 + ticks,
            });
            b.bool(d.cargo.is_some());
            if let Some(cargo) = d.cargo {
                b.u64(cargo.id);
                b.u16(cargo.material);
                b.u32(cargo.container);
            }
        }
    }

    pub fn load(
        &mut self,
        s: &mut Scan,
        units: &[RangeUnit],
        ledger: &crate::matter::Ledger,
    ) -> Option<()> {
        self.next_batch = s.u64();
        self.delivered = s.u64();
        self.trips = s.u64();
        self.lost = s.u64();
        let n = s.u32() as usize;
        if n > 512 || n > s.left() / 32 {
            return None;
        }
        self.sites.clear();
        let mut delivered = 0u64;
        for _ in 0..n {
            let kind = *RangeUnitKind::ALL.get(usize::from(s.u8()))?;
            let allied = s.bool();
            let at = std::array::from_fn(|_| s.i32());
            let required = s.u64();
            let completed = s.u64();
            let reserved = s.u64();
            let quota = s.u8();
            let unit = if s.bool() { Some(s.u16()) } else { None };
            if !kind.emplacement()
                || required == 0
                || completed > required
                || reserved > required - completed
                || quota > 32
                || unit.is_some_and(|id| {
                    units.get(usize::from(id)).is_none_or(|u| {
                        let upgraded_factory = completed == required
                            && matches!(
                                kind,
                                RangeUnitKind::KilFactoryT1
                                    | RangeUnitKind::KilFactoryT2
                                    | RangeUnitKind::KilFactoryT3
                            )
                            && matches!(
                                u.kind,
                                RangeUnitKind::KilFactoryT2
                                    | RangeUnitKind::KilFactoryT3
                                    | RangeUnitKind::KilFactoryT4
                            )
                            && u.kind.index() > kind.index();
                        (u.kind != kind && !upgraded_factory) || u.at != at
                    })
                })
            {
                return None;
            }
            delivered = delivered.checked_add(completed)?;
            self.sites.push(Site {
                kind,
                allied,
                at,
                required,
                delivered: completed,
                reserved,
                quota,
                unit,
            });
        }
        if delivered != self.delivered {
            return None;
        }
        let n = s.u32() as usize;
        // An unloaded drone takes 15 bytes; cargo and an assigned site add fields.
        if n > units.len() || n > s.left() / 15 {
            return None;
        }
        self.drones.clear();
        let mut batches = std::collections::BTreeSet::new();
        let mut containers = std::collections::BTreeSet::new();
        let mut last_batch = 0;
        for _ in 0..n {
            let id = s.u16();
            let home = s.u16();
            let site = if s.bool() { Some(s.u16()) } else { None };
            let want = s.u64();
            let leg = match s.u8() {
                0 => Leg::Idle,
                1 => Leg::ToSource,
                2 => Leg::ToSite,
                3 => Leg::Returning,
                ticks @ 4..=255 => Leg::Loading(ticks - 3),
            };
            let cargo = if s.bool() {
                let cargo = Batch {
                    id: s.u64(),
                    material: s.u16(),
                    container: s.u32(),
                };
                if cargo.material != STEEL
                    || ledger
                        .try_get(cargo.container)
                        .is_none_or(|c| !c.alive || c.kind != Kind::Body)
                    || !batches.insert(cargo.id)
                    || !containers.insert(cargo.container)
                {
                    return None;
                }
                last_batch = last_batch.max(cargo.id.saturating_add(1));
                Some(cargo)
            } else {
                None
            };
            if units
                .get(usize::from(id))
                .is_none_or(|u| u.kind != RangeUnitKind::SkobaDrone)
                || usize::from(home) >= units.len()
                || site.is_some_and(|site| usize::from(site) >= self.sites.len())
                || want > self.rules().drone_batch
                || cargo.is_some_and(|c| site.is_some() && ledger.held(c.container) != want)
                || self
                    .drones
                    .insert(
                        id,
                        Drone {
                            home,
                            site,
                            want,
                            leg,
                            cargo,
                        },
                    )
                    .is_some()
            {
                return None;
            }
        }
        if self.next_batch < last_batch || !self.active() || !s.ok() {
            return None;
        }
        Some(())
    }

    /// Keep a tick of work available for commanders, even if empty drones
    /// have already reserved the whole site while waiting for material.
    fn commander_allowance(&self, r: &BlastRange, site: usize) -> Amount {
        if r.construction_cancelled(site) {return 0;}
        let s = &self.sites[site];
        let reach = r.mining.as_ref().and_then(|m| m.data().kind(RangeUnitKind::Commander))
            .map_or(0, |k| i64::from(k.dig_reach));
        let density = site_quantum(s).max(1);
        r.battle.actors.iter().enumerate().filter_map(|(id, actor)| {
            if actor.order != battle::Order::Build(site) || r.allied(id) != s.allied
                || !r.units.get(id).is_some_and(|u| u.kind == RangeUnitKind::Commander && u.status.alive)
                || r.commander_tool_tip(id).is_none() { return None; }
            let pos = r.units[id].motion.pos.map(Fx::to_int_trunc);
            let distance: i64 = (0..3).map(|a| { let d = i64::from(pos[a]) - i64::from(s.at[a]); d * d }).sum();
            (distance <= reach * reach).then(|| per_second_rate(r.economic_rules().print_per_second, r.tick).min(r.tank_held(id).unwrap_or(0)) / density * density)
        }).sum()
    }

    fn assign_idle(&mut self, r: &BlastRange) {
        for site in 0..self.sites.len() {
            let allowance = self.commander_allowance(r, site);
            let quota = usize::from(self.sites[site].quota);
            if r.construction_cancelled(site) || self.sites[site].delivered == self.sites[site].required
                || self.sites[site]
                    .unit
                    .is_some_and(|id| !r.units.get(usize::from(id)).is_some_and(|u| u.status.alive))
            {
                continue;
            }
            let mut assigned = self
                .drones
                .values()
                .filter(|d| d.site == Some(site as u16))
                .count();
            let mut idle: Vec<u16> = self
                .drones
                .iter()
                .filter(|(id, d)| {
                    d.site.is_none()
                        && d.cargo.is_none()
                        && r.units
                            .get(usize::from(**id))
                            .is_some_and(|u| u.status.alive)
                        && r.allied(usize::from(**id)) == self.sites[site].allied
                        && match r.battle.actors.get(usize::from(**id)).map(|a| a.order) {
                            Some(battle::Order::Build(target)) => target == site,
                            Some(battle::Order::Auto) => assigned < quota,
                            _ => false,
                        }
                })
                .map(|(&id, _)| id)
                .collect();
            // Explicit player orders take precedence over spare automatic
            // capacity, with unit slot as the deterministic tie-breaker.
            idle.sort_by_key(|&id| (!r.battle.actors.get(usize::from(id)).is_some_and(|a| a.order == battle::Order::Build(site)), id));
            let mut explicit_left = idle.iter().filter(|&&id| r.battle.actors.get(usize::from(id))
                .is_some_and(|a| a.order == battle::Order::Build(site))).count();
            for id in idle {
                let explicit = r.battle.actors.get(usize::from(id)).is_some_and(|a| a.order == battle::Order::Build(site));
                if !explicit && assigned >= quota { continue; }
                let s = &mut self.sites[site];
                let left = (s.required - s.delivered - s.reserved).saturating_sub(allowance);
                if left == 0 {
                    break;
                }
                let density = site_quantum(s).max(1);
                let batch = (r.economic_rules().drone_batch / density).max(1) * density;
                // An explicit group order is shared across all selected
                // drones, even when the entire site fits in a few big loads.
                let want = if explicit && explicit_left > 1 {
                    let cells = left / density;
                    let share = cells.div_ceil(explicit_left as u64).max(1) * density;
                    explicit_left -= 1;
                    left.min(batch).min(share)
                } else {
                    if explicit { explicit_left = explicit_left.saturating_sub(1); }
                    left.min(batch)
                };
                s.reserved += want;
                let d = self.drones.get_mut(&id).expect("idle drone");
                d.site = Some(site as u16);
                d.want = want;
                d.leg = Leg::ToSource;
                assigned += 1;
            }
        }
    }

    pub fn set_quota(&mut self, site: usize, quota: u8) -> bool {
        let Some(s) = self.sites.get_mut(site) else {
            return false;
        };
        s.quota = quota.min(32);
        let quota = usize::from(s.quota);
        let mut assigned = self
            .drones
            .values()
            .filter(|d| d.site == Some(site as u16))
            .count();
        if assigned > quota {
            let ids: Vec<u16> = self
                .drones
                .iter()
                .rev()
                .filter(|(_, d)| d.site == Some(site as u16))
                .map(|(&id, _)| id)
                .collect();
            for id in ids {
                if assigned <= quota {
                    break;
                }
                let d = self.drones.get_mut(&id).expect("assigned drone");
                if d.cargo.is_none() {
                    self.sites[site].reserved -= d.want;
                    d.site = None;
                    d.want = 0;
                    d.leg = Leg::Idle;
                } else {
                    self.sites[site].reserved -= d.want;
                    d.site = None;
                    d.want = 0;
                    d.leg = Leg::Returning;
                }
                assigned -= 1;
            }
        }
        true
    }

    fn step(&mut self, r: &mut BlastRange) {
        if !matches!(
            r.battle.phase,
            battle::Phase::Setup | battle::Phase::Fighting
        ) {
            return;
        }
        // A newly issued explicit order releases an old reservation. Loaded
        // cargo first returns to its source; it is never discarded or copied.
        for (&id, drone) in &mut self.drones {
            let order = r.battle.actors.get(usize::from(id)).map(|a| a.order);
            let cancel = drone.site.is_some_and(|site| r.construction_cancelled(usize::from(site)) || match order {
                Some(battle::Order::Build(target)) => target != usize::from(site),
                Some(battle::Order::Move | battle::Order::Hold | battle::Order::Pilot { .. }) => true,
                _ => self.sites[usize::from(site)].quota == 0,
            });
            if cancel {
                let site = &mut self.sites[usize::from(drone.site.take().unwrap())];
                site.reserved = site.reserved.saturating_sub(drone.want);
                drone.want = 0;
                drone.leg = if drone.cargo.is_some() { Leg::Returning } else { Leg::Idle };
            }
        }
        for site in 0..self.sites.len() {
            let allowance = self.commander_allowance(r, site);
            if allowance == 0 { continue; }
            for d in self.drones.values_mut().rev() {
                let s = &mut self.sites[site];
                if s.required.saturating_sub(s.delivered).saturating_sub(s.reserved) >= allowance { break; }
                if d.site == Some(site as u16) && d.cargo.is_none() {
                    s.reserved = s.reserved.saturating_sub(d.want);
                    d.site = None;
                    d.want = 0;
                    d.leg = Leg::Idle;
                }
            }
        }
        self.assign_idle(r);
        let ids: Vec<u16> = self.drones.keys().copied().collect();
        for id in ids {
            self.step_drone(r, id);
        }
        for site in self.sites.iter().filter(|s| s.delivered < s.required) {
            if let Some(id) = site.unit.map(usize::from) {
                let body = r.matter.body_container(id);
                let intact = r.matter.economy.ledger.held(body) >= site.delivered;
                let u = &mut r.units[id];
                if intact && u.status.alive {
                    // A foundation supports its unfinished scaffold. Once
                    // damaged or complete, ordinary body physics owns it.
                    let pos = site.at.map(Fx::from_int);
                    if u.motion.pos != pos {
                        u.motion.pos = pos;
                        u.update_pose(u.motion.drive.head);
                        u.revision += 1;
                    }
                }
                u.status.engine = false;
                u.status.weapon = false;
                u.status.sensor = false;
            }
        }
    }

    fn step_drone(&mut self, r: &mut BlastRange, id: u16) {
        let index = usize::from(id);
        let Some(mut d) = self.drones.get(&id).copied() else {
            return;
        };
        if index >= r.units.len() {
            return;
        }
        if !r.units[index].status.alive {
            set_skoba_pose(&mut r.units[index], index, false);
            if let Some(site) = d
                .site
                .take()
                .and_then(|s| self.sites.get_mut(usize::from(s)))
            {
                site.reserved = site.reserved.saturating_sub(d.want);
            }
            d.want = 0;
            d.leg = Leg::Idle;
            if let Some(batch) = d.cargo.take() {
                let amount = r.matter.economy.ledger.held(batch.container);
                r.drop_drone_cargo(index, batch.container);
                self.lost += amount;
            }
            self.drones.insert(id, d);
            return;
        }
        // Stop is an explicit player order. Keep a carried batch safely on
        // the drone until another order resumes its return or delivery.
        if r.battle.actors.get(index).is_some_and(|actor| matches!(actor.order, battle::Order::Hold | battle::Order::Move | battle::Order::Pilot { .. })) {
            let moving = r.units[index].motion.destination.is_some()
                || r.units[index].motion.drive.vel != Fx::ZERO
                || r.units[index].motion.pilot.is_some_and(|pilot| pilot[0] != 0 || pilot[1] != 0);
            if d.cargo.is_none() && moving { set_skoba_pose(&mut r.units[index], index, true); }
            self.drones.insert(id, d);
            return;
        }
        // Manual control and its braking tail own flight until the craft stops.
        // Keep the delivery and cargo intact so automatic work can resume.
        if r.units[index].motion.pilot.is_some()
            || r.units[index].motion.drive.vel != Fx::ZERO
            || r.units[index].motion.drive.yaw_rate != 0
        {
            self.drones.insert(id, d);
            return;
        }
        let allied = r.allied(index);
        if !r.units.get(usize::from(d.home)).is_some_and(|u| {
            u.status.alive
                && matches!(
                    u.kind,
                    RangeUnitKind::GrebenStorage | RangeUnitKind::Crawler
                )
                && r.allied(usize::from(d.home)) == allied
        }) {
            let sides: Vec<bool> = (0..r.units.len()).map(|id| r.allied(id)).collect();
            d.home = nearest_home(&r.units, &sides, index) as u16;
        }
        if d.site.is_some_and(|id| {
            self.sites[usize::from(id)].unit.is_some_and(|unit| {
                !r.units
                    .get(usize::from(unit))
                    .is_some_and(|u| u.status.alive)
            })
        }) {
            let site = &mut self.sites[usize::from(d.site.take().unwrap())];
            site.reserved = site.reserved.saturating_sub(d.want);
            d.want = 0;
            d.leg = if d.cargo.is_some() {
                Leg::Returning
            } else {
                Leg::Idle
            };
        }
        // The self slot denotes no available home. Keep cargo/reservations
        // and recheck next tick, rather than chase our own moving y + 100.
        // A loaded delivery to a still-valid site can finish without a home.
        if usize::from(d.home) == index && matches!(d.leg, Leg::ToSource | Leg::Returning) {
            self.drones.insert(id, d);
            return;
        }
        if d.leg != Leg::Idle && !r.economic_power_tick(index) {
            self.drones.insert(id, d);
            return;
        }
        match d.leg {
            Leg::Idle => {
                if usize::from(d.home) != index && r.units.get(usize::from(d.home)).is_some_and(|u| u.kind == RangeUnitKind::GrebenStorage) {
                    let target = self.home_point(r,d.home,id);
                    if r.units[index].motion.pos.map(Fx::to_int_trunc) == target {
                        park_skoba(&mut r.units, index, usize::from(d.home));
                    }
                }
            }
            Leg::ToSource => {
                let target = self.home_point(r,d.home,id);
                if r.units[index].motion.pos.map(Fx::to_int_trunc) != target {
                    set_skoba_pose(&mut r.units[index], index, true);
                }
                let greben = r.units[usize::from(d.home)].kind == RangeUnitKind::GrebenStorage;
                if !approach_home(&mut r.units[index], target, r.size, greben) {
                    self.drones.insert(id, d);
                    return;
                }
                park_skoba(&mut r.units, index, usize::from(d.home));
                let source = if r.units[usize::from(d.home)].kind == RangeUnitKind::Crawler {
                    r.mining.as_ref().and_then(|m| m.tank(usize::from(d.home)))
                } else {
                    r.mining
                        .as_ref()
                        .and_then(|m| m.haul())
                        .map(|h| h.store(allied))
                };
                let Some(source) = source else {
                    self.drones.insert(id, d);
                    return;
                };
                let density = d.site.map_or(1, |site| site_quantum(&self.sites[usize::from(site)]).max(1));
                let ledger = &mut r.matter.economy.ledger;
                let cargo = ledger.create(Kind::Body, 0, d.want);
                let available = ledger.held(source).min(d.want) / density * density;
                let moved = ledger.transfer(source, cargo, available);
                if moved == 0 {
                    assert!(ledger.retire_empty(cargo));
                    self.drones.insert(id, d);
                    return;
                }
                if moved < d.want {
                    if let Some(site) = d.site.and_then(|s| self.sites.get_mut(usize::from(s))) {
                        site.reserved -= d.want - moved;
                    }
                    d.want = moved;
                }
                let batch = Batch {
                    id: self.next_batch,
                    material: STEEL,
                    container: cargo,
                };
                self.next_batch += 1;
                d.cargo = Some(batch);
                d.leg = Leg::Loading(r.economic_rules().drone_service_ticks as u8);
            }
            Leg::Loading(ticks) => {
                if d.cargo.is_none() { d.leg = Leg::Idle; }
                else if ticks > 1 { d.leg = Leg::Loading(ticks - 1); }
                else {
                    set_skoba_loaded(&mut r.units[index], index);
                    d.leg = Leg::ToSite;
                }
            }
            Leg::ToSite => {
                let Some(site_id) = d.site else {
                    d.leg = Leg::Returning;
                    self.drones.insert(id, d);
                    return;
                };
                let Some(site) = self.sites.get(usize::from(site_id)).cloned() else {
                    d.leg = Leg::Returning;
                    self.drones.insert(id, d);
                    return;
                };
                if r.units.get(usize::from(d.home)).is_some_and(|home| home.kind == RangeUnitKind::GrebenStorage && home.status.alive) {
                    let pad = self.home_point(r,d.home,id);
                    let pos = r.units[index].motion.pos.map(Fx::to_int_trunc);
                    if (pos[0] - pad[0]).abs() <= 2 && (pos[2] - pad[2]).abs() <= 2 && pos[1] < pad[1] + 40 {
                        fly(&mut r.units[index], [pad[0], (pad[1] + 40).min(r.size[1] as i32 - 2), pad[2]]);
                        self.drones.insert(id, d);
                        return;
                    }
                }
                if !fly(
                    &mut r.units[index],
                    drone_work_point([site.at[0], site.at[1] + 100, site.at[2]], id, r.size),
                ) {
                    self.drones.insert(id, d);
                    return;
                }
                let Some(batch) = d.cargo else {
                    d.leg = Leg::Returning;
                    self.drones.insert(id, d);
                    return;
                };
                if batch.material != STEEL
                    || !ensure_site_unit(r, &mut self.sites[usize::from(site_id)])
                {
                    self.drones.insert(id, d);
                    return;
                }
                let unit = self.sites[usize::from(site_id)].unit.expect("site unit");
                let body = r.matter.body_container(usize::from(unit));
                let moved = r
                    .matter
                    .economy
                    .ledger
                    .transfer(batch.container, body, d.want);
                if moved > 0 {
                    let s = &mut self.sites[usize::from(site_id)];
                    s.reserved -= moved;
                    s.delivered += moved;
                    self.delivered += moved;
                    apply_site_progress(
                        r,
                        usize::from(s.unit.expect("site unit")),
                        s.kind,
                        s.at,
                        s.required,
                        s.delivered - moved,
                        s.delivered,
                    );
                    if moved < d.want {
                        s.reserved = s.reserved.saturating_sub(d.want - moved);
                    }
                    d.want = 0;
                    d.site = None;
                    d.cargo = None;
                    d.leg = Leg::Returning;
                    self.trips += 1;
                    assert!(r.matter.economy.ledger.retire_empty(batch.container));
                    set_skoba_pose(&mut r.units[index], index, true);
                    if s.kind == RangeUnitKind::GrebenStorage && s.delivered == s.required {
                        let home = usize::from(s.unit.expect("storage unit"));
                        let reserved = self.sites.iter().filter(|s| s.unit.is_none()).count();
                        let owned=self.drones.iter().filter(|(&id,d)| usize::from(d.home)==home && r.units[usize::from(id)].status.alive).count();
                        spawn_greben_crew(r, home, reserved, owned);
                    }
                }
            }
            Leg::Returning => {
                if d.cargo.is_none() { set_skoba_pose(&mut r.units[index], index, true); }
                let target = self.home_point(r,d.home,id);
                let greben = r.units[usize::from(d.home)].kind == RangeUnitKind::GrebenStorage;
                if !approach_home(&mut r.units[index], target, r.size, greben) {
                    self.drones.insert(id, d);
                    return;
                }
                if let Some(batch) = d.cargo.take() {
                    let store = r
                        .mining
                        .as_ref()
                        .and_then(|m| m.haul())
                        .map(|h| h.store(allied));
                    if let Some(store) = store {
                        let amount = r.matter.economy.ledger.held(batch.container);
                        let moved =
                            r.matter
                                .economy
                                .ledger
                                .transfer(batch.container, store, amount);
                        if moved < amount {
                            d.cargo = Some(batch);
                        } else {
                            assert!(r.matter.economy.ledger.retire_empty(batch.container));
                            d.leg = Leg::Idle;
                        }
                    } else {
                        d.cargo = Some(batch);
                    }
                } else {
                    d.leg = Leg::Idle;
                }
                if d.leg == Leg::Idle { park_skoba(&mut r.units, index, usize::from(d.home)); }
            }
        }
        self.drones.insert(id, d);
    }
}

fn nearest_home(units: &[RangeUnit], sides: &[bool], id: usize) -> usize {
    units
        .iter()
        .enumerate()
        .filter(|(home, u)| {
            *home != id
                && u.status.alive
                && sides.get(*home) == sides.get(id)
                && matches!(
                    u.kind,
                    RangeUnitKind::GrebenStorage | RangeUnitKind::Crawler
                )
        })
        .min_by_key(|(home, u)| {
            let a = u.motion.pos.map(Fx::to_int_trunc);
            let b = units[id].motion.pos.map(Fx::to_int_trunc);
            (
                usize::from(u.kind != RangeUnitKind::GrebenStorage),
                (0..3)
                    .map(|k| {
                        let d = i64::from(a[k] - b[k]);
                        d * d
                    })
                    .sum::<i64>(),
                *home,
            )
        })
        .map(|(home, _)| home)
        .unwrap_or(id)
}

fn home_target(units: &[RangeUnit], home: usize) -> [i32; 3] {
    let Some(u) = units.get(home) else {
        return [0, 100, 0];
    };
    let at = u.motion.pos.map(Fx::to_int_trunc);
    if u.kind == RangeUnitKind::GrebenStorage {
        let entrance = u.world_pos([0, 8, 20]);
        [entrance[0], entrance[1] + 20, entrance[2]]
    } else {
        [at[0], at[1] + 100, at[2]]
    }
}

fn drone_home_point(units: &[RangeUnit], home: usize, id: u16, size: [u32; 3]) -> [i32; 3] {
    let mut point = if let Some(station) = units.get(home).filter(|u| u.kind == RangeUnitKind::GrebenStorage) {
        station.world_pos(voxel::unit::GREBEN_DRONE_PADS[usize::from(id % 8)])
    } else {
        return drone_work_point(home_target(units, home), id, size);
    };
    for axis in 0..3 { point[axis] = point[axis].clamp(1, size[axis].saturating_sub(2) as i32); }
    point
}

/// Keep simultaneous flights in distinct lanes at the pickup and scaffold.
fn drone_work_point(mut target: [i32; 3], id: u16, size: [u32; 3]) -> [i32; 3] {
    let slot = i32::from(id % 8);
    target[0] += ((slot % 4) * 2 - 3) * 24;
    target[2] += if slot < 4 { -42 } else { 42 };
    target[1] += (slot / 4) * 20;
    for axis in 0..3 { target[axis] = target[axis].clamp(1, size[axis].saturating_sub(2) as i32); }
    target
}

fn set_skoba_loaded(u: &mut RangeUnit, id: usize) {
    if u.kind != RangeUnitKind::SkobaDrone || !u.status.alive || !u.removed.is_empty() {
        return;
    }
    u.restore_skoba_form(2);
    let head = u.motion.drive.head;
    u.update_pose(head);
    u.refresh_damage(id);
    u.revision += 1;
}

fn set_skoba_pose(u: &mut RangeUnit, id: usize, compact: bool) {
    if u.kind != RangeUnitKind::SkobaDrone || !u.removed.is_empty() {
        return;
    }
    let is_compact = u.body.vol.get(64, 16, 26) && !u.body.vol.get(64, 16, 5);
    if compact == is_compact && !u.body.vol.get(70, 4, 31) {
        return;
    }
    let alive = u.status.alive;
    u.restore_skoba_form(u8::from(compact));
    let head = u.motion.drive.head;
    u.update_pose(head);
    u.refresh_damage(id);
    u.status.alive = alive;
    u.revision += 1;
}

impl RangeUnit {
    pub(in crate::blast) fn restore_skoba_form(&mut self, form: u8) {
        let mut body = match form {
            0 => voxel::unit::skoba(),
            1 => voxel::unit::skoba_drone(true),
            2 => voxel::unit::skoba_with_cargo(),
            _ => unreachable!("validated Skoba form"),
        };
        body.ablation_milli = self.body.ablation_milli;
        body.fracture_milli = self.body.fracture_milli;
        self.body = body;
        self.skoba_form = form;
    }
}

/// First reach a safe altitude, then approach above the pad, then descend.
/// The stages derive from position, so saves need no extra flight state.
fn approach_home(u: &mut RangeUnit, target: [i32; 3], size: [u32; 3], greben: bool) -> bool {
    if !greben { return fly(u, target); }
    let pos = u.motion.pos.map(Fx::to_int_trunc);
    let above = (target[1] + 40).min(size[1] as i32 - 2);
    let away = (pos[0] - target[0]).abs() > 2 || (pos[2] - target[2]).abs() > 2;
    if away && pos[1] < above {
        fly(u, [pos[0], above, pos[2]]);
        return false;
    }
    if away {
        fly(u, [target[0], above, target[2]]);
        return false;
    }
    fly(u, target)
}

/// A landed drone adopts the station orientation and opens its frame.
fn park_skoba(units: &mut [RangeUnit], id: usize, home: usize) {
    let heading = units[home].motion.drive.head;
    set_skoba_pose(&mut units[id], id, false);
    if units[id].motion.drive.head != heading {
        units[id].motion.drive.head = heading;
        units[id].update_pose(heading);
    }
}

fn fly(u: &mut RangeUnit, target: [i32; 3]) -> bool {
    let at = u.motion.pos.map(Fx::to_int_trunc);
    let delta = [target[0] - at[0], target[1] - at[1], target[2] - at[2]];
    let distance = delta.map(i32::abs).into_iter().max().unwrap_or(0);
    let speed = if u.skoba_form == 1 { FLY_COMPACT_SPEED } else { FLY_SPEED };
    let next = if distance <= speed {
        target
    } else {
        std::array::from_fn(|a| at[a] + delta[a] * speed / distance)
    };
    if delta[0] != 0 || delta[2] != 0 {
        u.motion.drive.head =
            fx::angle::atan2(Fx::from_int(delta[2]).raw(), Fx::from_int(delta[0]).raw());
    }
    u.motion.pos = next.map(Fx::from_int);
    let head = u.motion.drive.head;
    u.update_pose(head);
    next == target
}

impl BlastRange {
    /// Refill active builders from a nearby finished allied storage. Matter
    /// passes through the normal tank and ledger, so paid cells and repairs
    /// retain exactly the same ownership as locally gathered material.
    fn commander_supply_phase(&mut self) {
        let Some(mining) = self.mining.as_ref() else { return; };
        let Some(haul) = mining.haul() else { return; };
        let mut transfers = Vec::new();
        for (id, unit) in self.units.iter().enumerate() {
            if unit.kind != RangeUnitKind::Commander || !unit.status.alive { continue; }
            let working=match self.battle.actors.get(id).map(|a|a.order) {
                Some(battle::Order::Build(site))=>self.can_commander_build(id,site),
                Some(battle::Order::Repair(target))=>self.can_commander_repair(id,target), _=>false,
            };
            if !working {continue;}
            let Some(tank) = mining.tank(id) else { continue; };
            let held = self.matter.economy.ledger.held(tank);
            let want = self.economic_rules().print_per_second.saturating_mul(2).saturating_sub(held)
                .min(self.matter.economy.ledger.room(tank).saturating_sub(mining.reserved(id)));
            if want == 0 { continue; }
            let side = self.allied(id);
            let source = haul.store(side);
            if self.matter.economy.ledger.held(source) == 0 { continue; }
            let pos = unit.motion.pos;
            let near = self.units.iter().enumerate().any(|(other, storage)| {
                if !storage.status.alive || self.allied(other) != side
                    || !matches!(storage.kind, RangeUnitKind::Base | RangeUnitKind::GrebenStorage)
                    || self.unit_under_construction(other) { return false; }
                let dx = i64::from(pos[0].to_int_trunc() - storage.motion.pos[0].to_int_trunc());
                let dz = i64::from(pos[2].to_int_trunc() - storage.motion.pos[2].to_int_trunc());
                dx * dx + dz * dz <= COMMANDER_SUPPLY_REACH * COMMANDER_SUPPLY_REACH
            });
            if near { transfers.push((source, tank, want)); }
        }
        for (source, tank, want) in transfers {
            self.matter.economy.ledger.transfer(source, tank, want);
        }
    }

    /// A commander can bootstrap without a wreck or a mining order. This is
    /// an explicit ledger source and respects tank space reserved by digs.
    pub(crate) fn commander_matter_phase(&mut self) {
        if self.battle.phase != battle::Phase::Fighting { return; }
        let Some(mining) = self.mining.as_ref() else { return; };
        let rate = per_second_rate(self.economic_rules().commander_matter_per_second, self.tick);
        for (id, u) in self.units.iter().enumerate() {
            if u.kind != RangeUnitKind::Commander || !u.status.alive || self.unit_under_construction(id) { continue; }
            let Some(tank) = mining.tank(id) else { continue; };
            let room = self.matter.economy.ledger.room(tank).saturating_sub(mining.reserved(id));
            self.matter.economy.ledger.generate(tank, rate.min(room));
        }
    }
    /// Production and requested construction power for a side, per tick.
    /// Only alive, finished generators and builders currently able to work
    /// participate. Recomputed from authoritative state, so saves need no
    /// additional mutable accounting and both sides remain independent.
    pub fn construction_energy(&self, allied: bool) -> (u64, u64) {
        let generation = self.units.iter().enumerate().filter(|(id, u)| u.status.alive
            && self.allied(*id) == allied && !self.unit_under_construction(*id))
            .map(|(_, u)| active_generation(u, self.economic_rules())).sum();
        let demand = self.economic_requests().iter().enumerate().filter(|(id, _)| self.allied(*id) == allied)
            .map(|(_, &p)| p).sum();
        (generation, demand)
    }
    /// Local electrical islands, indexed by unit slot. Sources of the same
    /// side connect when they are within reach; other units attach to the
    /// nearest source. A mobile commander is also its own small generator.
    /// The graph is derived and bounded by the roster (at most 128 units).
    pub fn construction_power_networks(&self) -> Vec<Option<(u64, u64)>> {
        self.construction_power_with_requests(&self.economic_requests())
    }
    fn construction_power_with_requests(&self, requests: &[u64]) -> Vec<Option<(u64, u64)>> {
        let n = self.units.len();
        let mut roots: Vec<usize> = (0..n).collect();
        let active = |id: usize| self.units[id].status.alive && !self.unit_under_construction(id);
        let dist2 = |a: usize, b: usize| -> i64 {
            let a = self.units[a].motion.pos;
            let b = self.units[b].motion.pos;
            [0, 2].into_iter().map(|axis| {
                let d = i64::from(a[axis].to_int_trunc()) - i64::from(b[axis].to_int_trunc());
                d * d
            }).sum()
        };
        let mut sources = Vec::new();
        for id in 0..n {
            if active(id) && active_generation(&self.units[id], self.economic_rules()) > 0 { sources.push(id); }
        }
        for (i, &a) in sources.iter().enumerate() {
            for &b in &sources[..i] {
                if self.allied(a) == self.allied(b) && dist2(a, b) <= i64::from(self.economic_rules().power_link_reach).pow(2) {
                    let (mut x, mut y) = (a, b);
                    while roots[x] != x { x = roots[x]; }
                    while roots[y] != y { y = roots[y]; }
                    if x != y { roots[x] = y; }
                }
            }
        }
        for id in 0..n {
            if !active(id) || active_generation(&self.units[id], self.economic_rules()) > 0 { continue; }
            if let Some((_, source)) = sources.iter().copied().filter(|&source| self.allied(id) == self.allied(source))
                .filter_map(|source| { let d = dist2(id, source); (d <= i64::from(self.economic_rules().power_link_reach).pow(2)).then_some((d, source)) }).min() {
                roots[id] = source;
            }
        }
        let mut assigned = vec![None; n];
        for id in 0..n {
            if !active(id) { continue; }
            let mut root = id;
            while roots[root] != root { root = roots[root]; }
            if active_generation(&self.units[root], self.economic_rules()) > 0 { assigned[id] = Some(root); }
        }
        // Drone thrusters and the carried printer are supplied by the home
        // station. Flying beyond the cable radius must not strand the craft.
        if let Some(c) = self.mining.as_ref().and_then(|m| m.construction.as_ref()) {
            for (&id, drone) in &c.drones {
                let home=usize::from(drone.home);
                if home<n && self.units[home].status.alive && self.allied(usize::from(id))==self.allied(home) {
                    assigned[usize::from(id)] = assigned[home];
                }
            }
        }
        let mut totals = vec![(0u64, 0u64); n];
        for id in 0..n {
            if let Some(root) = assigned[id] {
                totals[root].0 = totals[root].0.saturating_add(active_generation(&self.units[id], self.economic_rules()));
            }
        }
        for (id, &demand) in requests.iter().enumerate() {
            if let Some(root) = assigned[id] { totals[root].1 = totals[root].1.saturating_add(demand); }
        }
        assigned.into_iter().map(|root| root.map(|root| totals[root])).collect()
    }
    pub fn construction_energy_estimate(&self, matter: Amount) -> u64 {
        let rules=self.economic_rules();
        (u128::from(matter)*u128::from(rules.print_power)*crate::TICKS_PER_SECOND as u128)
            .div_ceil(u128::from(rules.print_per_second)).min(u128::from(u64::MAX)) as u64
    }
    pub fn economic_rules(&self) -> &Balance {
        self.mining.as_ref().and_then(|m| m.data().balance.as_ref()).unwrap_or_else(|| Balance::legacy())
    }
    pub fn unit_energy_generation(&self, id: usize) -> u64 {
        self.units.get(id).filter(|u| u.status.alive).map_or(0, |u| active_generation(u, self.economic_rules()))
    }
    fn economic_requests(&self) -> Vec<u64> {
        let n = self.units.len();
        let mut requests = self.production_power_requests();
        for id in 0..n {
            if !self.units[id].status.alive || self.unit_under_construction(id) { continue; }
            let u = &self.units[id];
            if u.kind != RangeUnitKind::Commander || self.tank_held(id).unwrap_or(0) == 0 { continue; }
            let Some(battle::Order::Build(site)) = self.battle.actors.get(id).map(|a| a.order) else { continue; };
            if !self.can_commander_build(id, site) { continue; }
            let Some(s) = self.mining.as_ref().and_then(|m| m.construction.as_ref()).and_then(|c| c.sites.get(site)) else { continue; };
            let pos = u.motion.pos.map(Fx::to_int_trunc);
            let reach = self.mining.as_ref().and_then(|m| m.data().kind(RangeUnitKind::Commander)).map_or(0, |k| i64::from(k.dig_reach));
            let distance: i64 = (0..3).map(|a| { let d = i64::from(pos[a]) - i64::from(s.at[a]); d * d }).sum();
            if distance > reach * reach { continue; }
            let facing = fx::angle::atan2(Fx::from_int(s.at[2] - pos[2]).raw(), Fx::from_int(s.at[0] - pos[0]).raw());
            if fx::angle::diff(facing, u.motion.drive.head).abs() > fx::angle::FULL * 20 / 360 { continue; }
            if !u.commander_arm_ready(1,s.at) { continue; }
            let reclaimable: Amount = self.mining.as_ref().and_then(|m| m.construction.as_ref())
                .map_or(0, |c| c.drones.values().filter(|d| d.site == Some(site as u16) && d.cargo.is_none()).map(|d| d.want).sum());
            let free = s.required.saturating_sub(s.delivered).saturating_sub(s.reserved).saturating_add(reclaimable);
            let density = site_quantum(s).max(1);
            let print_rate = per_second_rate(self.economic_rules().print_per_second, self.tick);
            let work = print_rate.min(self.tank_held(id).unwrap_or(0)).min(free);
            if work > 0 && self.tank_held(id).unwrap_or(0) >= density && free >= density {
                let demand = ((u128::from(work) * u128::from(self.economic_rules().print_power) + u128::from(print_rate) - 1) / u128::from(print_rate)) as u64;
                requests[id] = demand;
            }
        }
        let rules = self.economic_rules();
        for id in 0..n {
            if !self.units[id].status.alive || self.unit_under_construction(id) { continue; }
            if let Some(battle::Order::Repair(target)) = self.battle.actors.get(id).map(|a| a.order) {
                if self.can_commander_repair(id, target) && self.tank_held(id).unwrap_or(0) >= self.matter.body_min_quantum(target) {
                    let at = self.units[target].motion.pos.map(Fx::to_int_trunc);
                    if self.commander_tool_in_reach(id, at) { requests[id] = rules.repair_power; }
                }
            }
        }
        if let Some(c) = self.mining.as_ref().and_then(|m| m.construction.as_ref()) {
            for (&id, drone) in &c.drones {
                let id = usize::from(id);
                if self.units[id].status.alive && drone.leg != Leg::Idle { requests[id] = rules.drone_power; }
            }
        }
        requests
    }
    fn commander_tool_in_reach(&self, id: usize, at: [i32; 3]) -> bool {
        let pos = self.units[id].motion.pos.map(Fx::to_int_trunc);
        let reach = self.mining.as_ref().and_then(|m| m.data().kind(RangeUnitKind::Commander)).map_or(0, |k| i64::from(k.dig_reach));
        let distance: i64 = (0..3).map(|axis| { let d=i64::from(at[axis])-i64::from(pos[axis]); d*d }).sum();
        let angle = fx::angle::atan2(Fx::from_int(at[2]-pos[2]).raw(), Fx::from_int(at[0]-pos[0]).raw());
        distance <= reach*reach && fx::angle::diff(angle, self.units[id].motion.drive.head).abs() <= fx::angle::FULL*20/360
            && self.units[id].commander_arm_ready(1,at)
    }
    pub(crate) fn economic_power_tick(&self, id: usize) -> bool {
        let nominal = if self.units.get(id).is_some_and(|u|u.kind==RangeUnitKind::SkobaDrone) {
            self.economic_rules().drone_power
        } else { self.production_power(id) };
        // A configured zero-power line is free; it must not wait forever on
        // a zero aggregate demand (including historical custom producers).
        if nominal == 0 { return true; }
        let (generation, demand) = self.economic_power.as_ref().filter(|(tick, _)| *tick == self.tick)
            .and_then(|(_, budget)| budget.get(id)).copied().flatten().unwrap_or((0, 0));
        if demand == 0 { return false; }
        let t = u128::from(self.tick);
        let n = u128::from(generation.min(demand));
        (t+1)*n/u128::from(demand) > t*n/u128::from(demand)
    }
    pub(super) fn receive_produced_drone(&mut self, drone: usize, home: usize) {
        if let Some(d)=self.mining.as_mut().and_then(|m|m.construction.as_mut()).and_then(|c|c.drones.get_mut(&(drone as u16))) {
            d.home=home as u16; d.leg=Leg::Returning;
            self.battle.actors[drone].order=battle::Order::Auto;
        }
    }
    /// Liquid material actually accessible to a commander's tool, not a remote side total.
    pub fn commander_accessible_matter(&self,id:usize)->Amount {
        let own=self.tank_held(id).unwrap_or(0);
        let Some(u)=self.units.get(id).filter(|u|u.kind==RangeUnitKind::Commander && u.status.alive) else {return own;};
        let near=self.units.iter().enumerate().any(|(other,s)| s.status.alive && self.allied(other)==self.allied(id)
            && matches!(s.kind,RangeUnitKind::Base|RangeUnitKind::GrebenStorage) && !self.unit_under_construction(other)
            && [0,2].into_iter().map(|a|{let d=i64::from(s.motion.pos[a].to_int_trunc())-i64::from(u.motion.pos[a].to_int_trunc());d*d}).sum::<i64>()<=COMMANDER_SUPPLY_REACH.pow(2));
        own.saturating_add(if near {self.side_store(self.allied(id)).map_or(0,|s|s.0)} else {0})
    }
    /// Site associated with an already materialised, unfinished structure.
    pub fn unit_construction_site(&self, unit: usize) -> Option<usize> {
        let unit = u16::try_from(unit).ok()?;
        self.mining.as_ref()?.construction.as_ref()?.sites.iter().position(|s| {
            s.unit == Some(unit) && s.delivered < s.required
        })
    }

    pub fn can_commander_repair(&self, commander: usize, target: usize) -> bool {
        if commander == target || !self.units.get(commander).is_some_and(|u| {
            u.kind == RangeUnitKind::Commander && u.status.alive
        }) || self.commander_tool_tip(commander).is_none() || self.allied(commander) != self.allied(target)
            || self.unit_under_construction(target) || self.factory_upgrading(target)
        { return false; }
        self.units.get(target).is_some_and(|u| u.status.alive &&
            (!u.removed.is_empty() || u.seg_removed.iter().any(|(s, _)| !u.seg_gone[usize::from(*s)])))
    }

    /// Pay for a bounded number of original cells from the commander's tank.
    /// Detached segments and in-flight fracture jobs stay under damage control.
    fn commander_repair_step(&mut self, commander: usize, target: usize) -> Amount {

        const SCAN_PER_TICK: usize = 512;
        if !self.can_commander_repair(commander, target) { return 0; }
        let Some(source) = self.mining.as_ref().and_then(|m| m.tank(commander)) else { return 0; };
        let body_id = self.matter.body_container(target);
        let density = self.matter.body_min_quantum(target);
        if density == 0 { return 0; }
        let (generation, demand) = self.economic_power.as_ref().and_then(|(_, p)| p.get(commander)).copied().flatten().unwrap_or((0, 0));
        let rules = self.economic_rules();
        let budget = if rules.repair_power == 0 { 32 * density } else {
            powered_work(per_second_rate(rules.repair_per_second, self.tick), generation, demand, density, self.tick)
        };
        let limit = (budget.min(self.matter.economy.ledger.held(source)) / density)
            .min(self.matter.economy.ledger.room(body_id) / density)
            .min(SCAN_PER_TICK as u64) as usize;
        if limit == 0 { return 0; }
        let u = &self.units[target];
        if u.section.is_some() || u.seg_sections.iter().any(Option::is_some) { return 0; }
        let mut root = Vec::new();
        let n = u.removed.len();
        if n > 0 {
            let start = (u64::from(self.tick) * SCAN_PER_TICK as u64 % n as u64) as usize;
            let reached = u.connectivity.reached_cols();
            let size = [usize::from(u.body.vol.nx), usize::from(u.body.vol.ny), usize::from(u.body.vol.nz)];
            for j in 0..n.min(SCAN_PER_TICK) {
                let i = (start + j) % n;
                let p = u.removed[i].map(usize::from);
                let linked = [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)].into_iter().any(|(axis, sign)| {
                    let mut q = p;
                    let v = p[axis] as i32 + sign;
                    if v < 0 || v >= size[axis] as i32 { return false; }
                    q[axis] = v as usize;
                    let word = (q[2] * size[1] + q[1]) * usize::from(u.body.vol.words) + q[0] / 64;
                    reached[word] & (1u64 << (q[0] % 64)) != 0
                });
                if linked { root.push(i); if root.len() == limit { break; } }
            }
        }
        let mut segments = Vec::new();
        if root.len() < limit {
            let n = u.seg_removed.len();
            let start = if n == 0 { 0 } else { (u64::from(self.tick) * SCAN_PER_TICK as u64 % n as u64) as usize };
            for j in 0..n.min(SCAN_PER_TICK) {
                let i = (start + j) % n;
                let (seg, p) = &u.seg_removed[i];
                let body = &u.body.segments[usize::from(*seg)].body;
                let p = p.map(usize::from);
                let size = [usize::from(body.vol.nx), usize::from(body.vol.ny), usize::from(body.vol.nz)];
                let linked = [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)].into_iter().any(|(axis, sign)| {
                    let mut q = p;
                    let v = p[axis] as i32 + sign;
                    if v < 0 || v >= size[axis] as i32 { return false; }
                    q[axis] = v as usize;
                    body.vol.get(q[0], q[1], q[2])
                });
                if !u.seg_gone[usize::from(*seg)] && linked {
                    segments.push(i);
                    if root.len() + segments.len() == limit { break; }
                }
            }
        }
        let mut paid = 0;
        let available = budget.min(self.matter.economy.ledger.held(source)).min(self.matter.economy.ledger.room(body_id));
        root.retain(|_| {
            let quantum = self.matter.body_quantum(target, 0);
            if quantum <= available.saturating_sub(paid) { paid += quantum; true } else { false }
        });
        segments.retain(|&index| {
            let part = usize::from(self.units[target].seg_removed[index].0) + 1;
            let quantum = self.matter.body_quantum(target, part);
            if quantum <= available.saturating_sub(paid) { paid += quantum; true } else { false }
        });
        let count = root.len() + segments.len();
        if count == 0 { return 0; }
        let (frame, cell) = if let Some(&i) = root.last() {
            let cell = u.removed[i];
            (u.root_frame(usize::from(cell[1])), cell)
        } else {
            let (segment, cell) = u.seg_removed[*segments.last().unwrap()];
            (u.seg_frames[usize::from(segment)], cell)
        };
        let impact = frame.world(cell.map(|v| Fx::from_int(i32::from(v)).add(Fx::from_ratio(1, 2))));
        if self.matter.economy.ledger.transfer(source, body_id, paid) != paid { return 0; }
        let u = &mut self.units[target];
        for &i in &root {
            let p = u.removed[i].map(usize::from);
            let part = u.body.part_at(p[0], p[1], p[2]);
            u.body.set(p[0], p[1], p[2], part);
        }
        for &i in &segments {
            let (s, p) = u.seg_removed[i];
            let p = p.map(usize::from);
            let body = &mut u.body.segments[usize::from(s)].body;
            let part = body.part_at(p[0], p[1], p[2]);
            body.set(p[0], p[1], p[2], part);
        }
        root.sort_unstable();
        segments.sort_unstable();
        let mut i = 0;
        u.removed.retain(|_| { let keep = root.binary_search(&i).is_err(); i += 1; keep });
        let mut i = 0;
        u.seg_removed.retain(|_| { let keep = segments.binary_search(&i).is_err(); i += 1; keep });
        u.detached = u.detached.min(u.removed.len());
        u.seg_detached = u.seg_detached.min(u.seg_removed.len());
        u.refresh_damage(target);
        u.revision += 1;
        if let Some(c) = self.mining.as_mut().and_then(|m| m.construction.as_mut()) {
            c.print_traces.push(PrintTrace { actor: commander, impact: [impact.x, impact.y, impact.z] });
        }
        paid
    }
    pub fn can_commander_build(&self, commander: usize, site: usize) -> bool {
        !self.construction_cancelled(site) && self.units.get(commander).is_some_and(|u| u.kind == RangeUnitKind::Commander && u.status.alive)
            && self.commander_tool_tip(commander).is_some()
            && self.mining.as_ref().and_then(|m| m.construction.as_ref())
                .and_then(|c| c.sites.get(site))
                .is_some_and(|s| s.allied == self.allied(commander)
                    && s.delivered < s.required
                    && s.unit.is_none_or(|id| self.units.get(usize::from(id)).is_some_and(|u| u.status.alive)))
    }
    pub fn can_drone_build(&self, drone: usize, site: usize) -> bool {
        !self.construction_cancelled(site) && self.units.get(drone).is_some_and(|u| u.kind == RangeUnitKind::SkobaDrone && u.status.alive)
            && self.mining.as_ref().and_then(|m| m.construction.as_ref())
                .is_some_and(|c| c.drones.contains_key(&(drone as u16)) && c.sites.get(site)
                    .is_some_and(|s| s.allied == self.allied(drone) && s.delivered < s.required
                        && s.unit.is_none_or(|id| self.units.get(usize::from(id)).is_some_and(|u| u.status.alive))))
    }
    pub(crate) fn valid_commander_site(&self, commander: usize, site: usize) -> bool {
        self.units.get(commander).is_some_and(|u| matches!(u.kind, RangeUnitKind::Commander | RangeUnitKind::SkobaDrone))
            && self.mining.as_ref().and_then(|m| m.construction.as_ref())
                .and_then(|c| c.sites.get(site))
                .is_some_and(|s| s.allied == self.allied(commander))
    }
    /// One direct fabrication step, paid from the commander's own universal
    /// matter container. This primitive does not schedule orders or draw a
    /// beam; callers must provide a deterministic rate and reach.
    pub(crate) fn commander_print_step(
        &mut self,
        commander: usize,
        site: usize,
        reach: i32,
        rate: Amount,
    ) -> Amount {
        if self.construction_cancelled(site) || reach < 0
            || !matches!(
                self.battle.phase,
                battle::Phase::Setup | battle::Phase::Fighting
            )
            || !self
                .units
                .get(commander)
                .is_some_and(|u| u.kind == RangeUnitKind::Commander && u.status.alive)
        {
            return 0;
        }
        let Some(source) = self.mining.as_ref().and_then(|m| m.tank(commander)) else {
            return 0;
        };
        let Some(mut construction) = self.mining.as_mut().and_then(|m| m.construction.take())
        else {
            return 0;
        };
        let moved = (|| {
            let s = construction.sites.get(site)?;
            if s.allied != self.allied(commander)
                || s.unit
                    .is_some_and(|id| !self.units[usize::from(id)].status.alive)
            {
                return None;
            }
            let pos = self.units[commander].motion.pos.map(Fx::to_int_trunc);
            let distance: i64 = (0..3)
                .map(|a| {
                    let d = i64::from(pos[a]) - i64::from(s.at[a]);
                    d * d
                })
                .sum();
            if distance > i64::from(reach) * i64::from(reach) {
                return None;
            }
            let density = site_quantum(s).max(1);
            let available = s
                .required
                .saturating_sub(s.delivered)
                .saturating_sub(s.reserved)
                .min(rate)
                .min(self.matter.economy.ledger.held(source))
                / density
                * density;
            if available == 0 {
                return None;
            }
            let s = &mut construction.sites[site];
            if !ensure_site_unit(self, s) {
                return None;
            }
            let id = usize::from(s.unit?);
            let body = self.matter.body_container(id);
            let paid = self.matter.economy.ledger.transfer(source, body, available);
            let previous = s.delivered;
            s.delivered += paid;
            construction.delivered += paid;
            apply_site_progress(self, id, s.kind, s.at, s.required, previous, s.delivered);
            Some(paid)
        })()
        .unwrap_or(0);
        self.mining
            .as_mut()
            .expect("construction owner")
            .construction = Some(construction);
        if moved > 0 {
            let completed = self.mining.as_ref().and_then(|m| m.construction.as_ref()).and_then(|c| {
                let s = c.sites.get(site)?;
                (s.kind == RangeUnitKind::GrebenStorage && s.delivered == s.required)
                    .then(|| (usize::from(s.unit.expect("finished storage has a body")),
                        c.sites.iter().filter(|s| s.unit.is_none()).count()))
            });
            if let Some((home, reserved)) = completed {
                let owned=self.mining.as_ref().and_then(|m|m.construction.as_ref()).map_or(0,|c|c.drones.iter().filter(|(&id,d)| usize::from(d.home)==home && self.units[usize::from(id)].status.alive).count());
                spawn_greben_crew(self, home, reserved, owned);
            }
        }
        moved
    }

    /// Unmaterialised foundations already own a roster slot and a footprint.
    /// Production and sandbox insertion must respect both reservations.
    pub(in super::super) fn reserved_construction_bounds(
        &self,
    ) -> impl Iterator<Item = ([i32; 3], [i32; 3])> + '_ {
        self.mining
            .as_ref()
            .and_then(|m| m.construction.as_ref())
            .into_iter()
            .flat_map(|c| c.sites.iter().enumerate())
            .filter(|(id,s)| s.unit.is_none() && !self.construction_cancelled(*id))
            .map(|(_,s)| {
                super::super::placement::Validator::default().runtime_bounds(
                    s.kind,
                    s.at,
                    if self.battle_mode() && !s.allied {
                        2
                    } else {
                        0
                    },
                )
            })
    }

    pub(super) fn occupied_unit_slots(&self) -> usize {
        self.units.len() + self.reserved_construction_bounds().count()
    }

    pub fn construction_cost(&self, kind: RangeUnitKind) -> Option<Amount> {
        kind.emplacement().then(|| self.matter.kind_matter(kind))
    }

    pub fn unit_under_construction(&self, id: usize) -> bool {
        self.mining
            .as_ref()
            .and_then(|m| m.construction.as_ref())
            .is_some_and(|c| {
                c.sites
                    .iter()
                    .any(|s| s.unit == Some(id as u16) && s.delivered < s.required)
            })
    }
    pub fn construction_sites(&self) -> Vec<SiteView> {
        let mut sites=self.mining
            .as_ref()
            .and_then(|m| m.construction.as_ref())
            .map_or_else(Vec::new, |c| c.sites());
        for s in &mut sites {s.cancelled=self.construction_cancelled(s.id);s.destroyed=s.unit.is_some_and(|id|!self.units[id].status.alive);}
        sites
    }
    pub(in super::super) fn construction_history_count(&self)->usize {self.mining.as_ref().and_then(|m|m.construction.as_ref()).map_or(0,|c|c.sites.len())}
    pub fn construction_site_state(&self,id:usize)->Option<SiteView> {
        let s=self.mining.as_ref()?.construction.as_ref()?.sites.get(id)?;
        Some(SiteView{id,kind:s.kind,at:s.at,required:s.required,delivered:s.delivered,quota:s.quota,assigned:0,
            complete:s.delivered==s.required,allied:s.allied,unit:s.unit.map(usize::from),cancelled:self.construction_cancelled(id),
            destroyed:s.unit.is_some_and(|id|!self.units[usize::from(id)].status.alive)})
    }
    pub fn construction_site_at(&self,point:[i32;3])->Option<usize> {
        self.construction_sites().into_iter().find_map(|s| {
            if !s.allied || s.complete || s.cancelled || s.destroyed || s.unit.is_some() {return None;}
            let (lo,hi)=super::super::placement::Validator::default().runtime_bounds(s.kind,s.at,0);
            ([0,2].into_iter().all(|a|point[a]>=lo[a] && point[a]<=hi[a])).then_some(s.id)
        })
    }

    /// Unfinished construction sites for presentation, without building a
    /// per-site drone census every rendered frame.
    pub fn construction_progress_views(&self) -> impl Iterator<Item = ProgressView> + '_ {
        self.mining.as_ref().and_then(|m| m.construction.as_ref())
            .into_iter().flat_map(|c| c.sites.iter().enumerate())
            .filter(|(id,s)| s.delivered < s.required && !self.construction_cancelled(*id) && s.unit.is_none_or(|id|self.units[usize::from(id)].status.alive))
            .map(|(_,s)| ProgressView { allied: s.allied, at: s.at,
                unit: s.unit.map(usize::from), required: s.required, delivered: s.delivered })
    }

    pub fn construction_print_traces(&self) -> &[PrintTrace] {
        self.mining.as_ref().and_then(|m| m.construction.as_ref())
            .map_or(&[], |c| c.print_traces.as_slice())
    }

    fn construction_print_impact(&self, site: usize) -> Option<[Fx; 3]> {
        let s = self.mining.as_ref()?.construction.as_ref()?.sites.get(site)?;
        let unit = self.units.get(usize::from(s.unit?))?;
        let cells = construction_geometry(s.kind);
        let count = ((cells.len() as u128 * u128::from(s.delivered)) / u128::from(s.required.max(1))) as usize;
        let (segment, cell) = *cells.get(count.checked_sub(1)?)?;
        let frame = match segment {
            Some(index) => *unit.seg_frames.get(index)?,
            None => unit.root_frame(cell[1]),
        };
        let position = frame.world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
        Some([position.x, position.y, position.z])
    }

    pub fn drone_cargo(&self, id: usize) -> Option<Batch> {
        self.mining.as_ref()?.construction.as_ref()?.cargo(id)
    }

    pub fn drone_leg(&self, id: usize) -> Option<Leg> {
        self.mining.as_ref()?.construction.as_ref()?.leg(id)
    }

    pub fn set_construction_quota(&mut self, site: usize, quota: u8) -> bool {
        self.mining
            .as_mut()
            .and_then(|m| m.construction.as_mut())
            .is_some_and(|c| c.set_quota(site, quota))
    }

    pub fn can_construct(&self, kind: RangeUnitKind, allied: bool, at: [i32; 3]) -> bool {
        let Some(c) = self.mining.as_ref().and_then(|m| m.construction.as_ref()) else {
            return false;
        };
        if kind == RangeUnitKind::Base && self.economic_rules().unique_base
            && (self.units.iter().enumerate().any(|(id,u)| u.kind == kind && u.status.alive && self.allied(id)==allied)
                || c.sites.iter().enumerate().any(|(id,s)| s.kind==kind && s.allied==allied && s.unit.is_none() && !self.construction_cancelled(id))) { return false; }
        if !kind.emplacement()
            || self.units.len() + c.sites.iter().enumerate().filter(|(id,s)| s.unit.is_none() && !self.construction_cancelled(*id)).count()
                >= MAX_RANGE_UNITS
            || c.sites.len() >= 512
            || c.sites.iter().enumerate().filter(|(id,s)|s.delivered<s.required && !self.construction_cancelled(*id) && s.unit.is_none_or(|id|self.units[usize::from(id)].status.alive)).count()>=32
        {
            return false;
        }
        let validator = super::super::placement::Validator::default();
        let yaw = if self.battle_mode() && !allied { 2 } else { 0 };
        if validator.runtime(self, kind, at, yaw).is_err() {
            return false;
        }
        let (lo, hi) = validator.runtime_bounds(kind, at, yaw);
        !c.sites.iter().enumerate().filter(|(id,s)| s.unit.is_none() && !self.construction_cancelled(*id)).any(|(_,s)| {
            let yaw = if self.battle_mode() && !s.allied {
                2
            } else {
                0
            };
            let (a, b) = validator.runtime_bounds(s.kind, s.at, yaw);
            (0..3).all(|k| lo[k] < b[k] && a[k] < hi[k])
        })
    }

    pub fn create_construction_site(
        &mut self,
        kind: RangeUnitKind,
        allied: bool,
        at: [i32; 3],
    ) -> Option<usize> {
        if !self.can_construct(kind, allied, at) {
            return None;
        }
        let required = self.matter.kind_matter(kind);
        let quota = if self.construction_sites().is_empty() {
            8
        } else {
            4
        };
        let m = self.mining.as_mut()?;
        let c = m.construction.as_mut()?;
        c.sites.push(Site {
            kind,
            allied,
            at,
            required,
            delivered: 0,
            reserved: 0,
            quota,
            unit: None,
        });
        Some(c.sites.len() - 1)
    }

    pub fn upgrade_crawler_drones(&mut self, id: usize) -> bool {
        if !self
            .units
            .get(id)
            .is_some_and(|u| u.kind == RangeUnitKind::Crawler && u.status.alive && self.allied(id))
            || self.occupied_unit_slots() + 4 > MAX_RANGE_UNITS
            || self
                .mining
                .as_ref()
                .and_then(|m| m.construction.as_ref())
                .is_some_and(|c| c.drones.values().any(|d| usize::from(d.home) == id))
        {
            return false;
        }
        let Some(price) = self.production_cost(RangeUnitKind::SkobaDrone) else {
            return false;
        };
        let total = price.saturating_mul(4);
        let allied = self.allied(id);
        let Some(store) = self
            .mining
            .as_ref()
            .and_then(|m| m.haul())
            .map(|h| h.store(allied))
        else {
            return false;
        };
        if self.matter.economy.ledger.held(store) < total {
            return false;
        }
        let paid = self.matter.economy.ledger.create(Kind::Body, 0, total);
        assert_eq!(
            self.matter.economy.ledger.transfer(store, paid, total),
            total
        );
        let at = self.units[id].motion.pos.map(Fx::to_int_trunc);
        for offset in [-36, -12, 12, 36] {
            let blank = self.matter.economy.ledger.create(Kind::Body, 0, price);
            assert_eq!(
                self.matter.economy.ledger.transfer(paid, blank, price),
                price
            );
            let drone = self.spawn(
                RangeUnitKind::SkobaDrone,
                true,
                [at[0] + 24, at[1] + 64, at[2] + offset],
                blank,
            );
            self.note_construction_spawn();
            set_skoba_pose(&mut self.units[drone], drone, true);
            if let Some(c) = self.mining.as_mut().and_then(|m| m.construction.as_mut()) {
                if let Some(d) = c.drones.get_mut(&(drone as u16)) {
                    d.home = id as u16;
                    d.leg = Leg::Idle;
                }
            }
        }
        assert!(self.matter.economy.ledger.retire_empty(paid));
        true
    }

    pub(crate) fn construction_phase(&mut self) {
        self.commander_supply_phase();
        let work_requests = self.economic_requests();
        self.economic_power = Some((self.tick, self.construction_power_with_requests(&work_requests)));
        let Some(mut c) = self.mining.as_mut().and_then(|m| m.construction.take()) else {
            return;
        };
        c.print_traces.clear();
        let previous_units = self.units.len();
        c.step(self);
        // Completion can append crew while Construction is temporarily
        // detached from Mining, so append_unit cannot register those drones.
        c.register_new_units(self, previous_units);
        if let Some(m) = self.mining.as_mut() {
            m.construction = Some(c);
        }
        let power = self.economic_power.as_ref().unwrap().1.clone();
        let orders: Vec<_> = self.battle.actors.iter().enumerate().filter_map(|(id, a)| {
            if let battle::Order::Build(site) = a.order { Some((id, site)) } else { None }
        }).collect();
        for (id, site) in orders {
            if self.units[id].kind == RangeUnitKind::SkobaDrone {
                if !self.can_drone_build(id, site) { self.battle.actors[id].order = battle::Order::Auto; }
                continue;
            }
            if !self.can_commander_build(id, site) {
                self.battle.actors[id].order = battle::Order::Hold;
                self.units[id].motion.stop();
                let yaw = self.battle.actors[id].turret.angle;
                self.units[id].relax_commander_arms(yaw);
                continue;
            }
            let reach = self.mining.as_ref().and_then(|m| m.data().kind(RangeUnitKind::Commander))
                .map_or(0, |k| k.dig_reach) as i32;
            let target = self.mining.as_ref().and_then(|m| m.construction.as_ref())
                .expect("validated construction").sites[site].at;
            let pos = self.units[id].motion.pos.map(Fx::to_int_trunc);
            let dx = i64::from(target[0] - pos[0]);
            let dz = i64::from(target[2] - pos[2]);
            let dy = i64::from(target[1] - pos[1]);
            if dx * dx + dy * dy + dz * dz > i64::from(reach) * i64::from(reach) {
                if self.units[id].motion.destination.is_none() && reach > 0 {
                    let dist = ((dx * dx + dz * dz) as u64).isqrt().max(1) as i64;
                    let back = (i64::from(reach) * 3 / 4).min(dist);
                    self.units[id].motion.queue_move(&self.world, [target[0] - (dx * back / dist) as i32, target[2] - (dz * back / dist) as i32]);
                }
            } else {
                if self.units[id].motion.destination.is_some() { self.units[id].motion.stop(); }
                let facing = self.units[id].commander_face(target);
                let yaw = self.units[id].motion.drive.head;
                self.battle.actors[id].turret = crate::turret::Traverse::new(yaw);
                // The power budget was frozen before moving the arm. A tool
                // becoming ready now starts paid work on the following tick.
                self.units[id].set_arm_targets([target, target, [1, 0, 0], [0, 0, 0]], yaw);
                let (generation, demand) = power.get(id).copied().flatten().unwrap_or((0, 0));
                let s = &self.mining.as_ref().unwrap().construction.as_ref().unwrap().sites[site];
                let density = site_quantum(s).max(1);
                let work = per_second_rate(self.economic_rules().print_per_second, self.tick).min(self.tank_held(id).unwrap_or(0))
                    .min(s.required.saturating_sub(s.delivered).saturating_sub(s.reserved));
                let rate = powered_work(work, generation, demand, density, self.tick);
                if facing && work_requests.get(id).is_some_and(|demand|*demand>0)
                    && self.units[id].commander_arm_ready(1,target)
                    && self.commander_print_step(id, site, reach, rate) > 0 {
                    if let Some(impact) = self.construction_print_impact(site) {
                        if let Some(c) = self.mining.as_mut().and_then(|m| m.construction.as_mut()) {
                            c.print_traces.push(PrintTrace { actor: id, impact });
                        }
                    }
                }
            }
        }
        let repairs: Vec<_> = self.battle.actors.iter().enumerate().filter_map(|(id, a)| {
            if let battle::Order::Repair(target) = a.order { Some((id, target)) } else { None }
        }).collect();
        for (id, target) in repairs {
            if !self.can_commander_repair(id, target) {
                self.battle.actors[id].order = battle::Order::Hold;
                self.units[id].motion.stop();
                let yaw = self.battle.actors[id].turret.angle;
                self.units[id].relax_commander_arms(yaw);
                continue;
            }
            let reach = self.mining.as_ref().and_then(|m| m.data().kind(RangeUnitKind::Commander))
                .map_or(0, |k| k.dig_reach) as i32;
            let at = self.units[target].motion.pos.map(Fx::to_int_trunc);
            let pos = self.units[id].motion.pos.map(Fx::to_int_trunc);
            let dx = i64::from(at[0] - pos[0]);
            let dy = i64::from(at[1] - pos[1]);
            let dz = i64::from(at[2] - pos[2]);
            if dx * dx + dy * dy + dz * dz > i64::from(reach) * i64::from(reach) {
                if self.units[id].motion.destination.is_none() && reach > 0 {
                    let dist = ((dx * dx + dz * dz) as u64).isqrt().max(1) as i64;
                    let back = (i64::from(reach) * 3 / 4).min(dist);
                    self.units[id].motion.queue_move(&self.world, [at[0] - (dx * back / dist) as i32, at[2] - (dz * back / dist) as i32]);
                }
            } else {
                if self.units[id].motion.destination.is_some() { self.units[id].motion.stop(); }
                let facing = self.units[id].commander_face(at);
                let yaw = self.units[id].motion.drive.head;
                self.battle.actors[id].turret = crate::turret::Traverse::new(yaw);
                self.units[id].set_arm_targets([at, at, [1, 0, 0], [0, 0, 0]], yaw);
                if facing && work_requests.get(id).is_some_and(|demand|*demand>0)
                    && self.units[id].commander_arm_ready(1,at) {
                    self.commander_repair_step(id, target);
                }
            }
        }
    }

    pub(crate) fn has_construction(&self) -> bool {
        self.mining
            .as_ref()
            .and_then(|m| m.construction.as_ref())
            .is_some_and(|c| c.active())
    }

    pub(crate) fn save_construction(&self, b: &mut Blob) {
        if let Some(c) = self.mining.as_ref().and_then(|m| m.construction.as_ref()) {
            c.save(b);
        }
    }

    pub(crate) fn load_construction(&mut self, s: &mut Scan, legacy_forms: bool) -> Option<()> {
        let c = self.mining.as_mut()?.construction.as_mut()?;
        c.load(s, &self.units, &self.matter.economy.ledger)?;
        if legacy_forms {
            // Older snapshots omitted the form. Undamaged loaded/returning
            // craft have an unambiguous form; damaged historical poses do not.
            let forms: Vec<_> = c
                .drones
                .iter()
                .filter_map(|(&id, d)| {
                    let u = &self.units[usize::from(id)];
                    (u.status.alive && u.removed.is_empty()).then_some((
                        id,
                        if d.cargo.is_some() {
                            2
                        } else if d.leg == Leg::Returning {
                            1
                        } else {
                            0
                        },
                    ))
                })
                .collect();
            for (id, form) in forms.into_iter().filter(|(_, form)| *form != 0) {
                let u = &mut self.units[usize::from(id)];
                u.restore_skoba_form(form);
                u.update_pose(u.motion.drive.head);
                u.refresh_damage(usize::from(id));
            }
        }
        // An unfinished core is paid and grown incrementally. Its low fill
        // ratio is construction progress, not a weapon destroying the core.
        let unfinished: Vec<_> = self
            .construction_sites()
            .into_iter()
            .filter(|s| !s.complete)
            .collect();
        for site in unfinished {
            if let Some(id) = self
                .units
                .iter()
                .position(|u| u.kind == site.kind && u.at == site.at)
            {
                let body = self.matter.body_container(id);
                if self.matter.economy.ledger.held(body) >= site.delivered {
                    unfinished_status(&mut self.units[id]);
                } else {
                    self.units[id].status.weapon = false;
                    self.units[id].status.engine = false;
                    self.units[id].status.sensor = false;
                }
            }
        }
        Some(())
    }
}

fn spawn_greben_crew(r: &mut BlastRange, home: usize, reserved: usize, owned: usize) {
    if r.battle_mode() && r.economic_rules().queued_crews {
        r.queue_greben_crew(home, owned);
        return;
    }
    let at = r.units[home].motion.pos.map(Fx::to_int_trunc);
    let allied = r.allied(home);
    let direction = if at[0] + 900 < r.size[0] as i32 {
        1
    } else {
        -1
    };
    let xmax = (r.size[0] as i32 - 1).max(0);
    let ymax = (r.size[1] as i32 - 1).max(0);
    let zmax = (r.size[2] as i32 - 1).max(0);
    let xmargin = 100.min(xmax / 2);
    let edge = 100.min(zmax / 2);
    let drones_near = r
        .units
        .iter()
        .enumerate()
        .filter(|(id, u)| {
            *id != home
                && u.kind == RangeUnitKind::SkobaDrone
                && r.allied(*id) == allied
                && (0..3)
                    .map(|a| {
                        let p = u.motion.pos[a].to_int_trunc();
                        let d = i64::from(p - at[a]);
                        d * d
                    })
                    .sum::<i64>()
                    <= 2000 * 2000
        })
        .count();
    for _ in drones_near..8 {
        if r.units.len() + reserved >= MAX_RANGE_UNITS {
            break;
        }
        if !spawn_paid_crew_unit(
            r,
            RangeUnitKind::SkobaDrone,
            allied,
            drone_home_point(&r.units, home, r.units.len() as u16, r.size),
        ) { break; }
        let drone = r.units.len() - 1;
        park_skoba(&mut r.units, drone, home);
    }
    let crawler_near = r.units.iter().enumerate().any(|(id, u)| {
        id != home
            && u.kind == RangeUnitKind::Crawler
            && r.allied(id) == allied
            && (0..3)
                .map(|a| {
                    let p = u.motion.pos[a].to_int_trunc();
                    let d = i64::from(p - at[a]);
                    d * d
                })
                .sum::<i64>()
                <= 2000 * 2000
    });
    if !crawler_near && r.units.len() + reserved < MAX_RANGE_UNITS {
        spawn_paid_crew_unit(
            r,
            RangeUnitKind::Crawler,
            allied,
            [(at[0] + direction * 800).clamp(xmargin, xmax - xmargin),
                at[1].clamp(0, ymax), (at[2] + 500).clamp(edge, zmax - edge)],
        );
    }
}

/// Crew generated by a newly completed station is real production, not free
/// matter. Authored scenario crews are created separately at scenario start.
fn spawn_paid_crew_unit(r: &mut BlastRange, kind: RangeUnitKind, allied: bool, at: [i32; 3]) -> bool {
    let cost = r.matter.kind_matter(kind);
    let Some(source) = r.mining.as_ref().and_then(|m| m.haul()).map(|h| h.store(allied)) else { return false; };
    let ledger = &mut r.matter.economy.ledger;
    if ledger.held(source) < cost { return false; }
    let body = ledger.create(Kind::Body, 0, cost);
    assert_eq!(ledger.transfer(source, body, cost), cost);
    let tank = if kind == RangeUnitKind::Crawler {
        let cap = r
            .mining
            .as_ref()
            .and_then(|m| m.data().kind(kind))
            .map_or(0, |k| k.tank);
        Some(r.matter.economy.ledger.create(Kind::Hauler, 0, cap))
    } else {
        None
    };
    r.append_unit(super::produce::Spawned {
        def: RangeUnitDef {
            allied,
            kind,
            at,
            weapon: None,
        },
        body,
        tank,
    });
    r.note_construction_spawn();
    true
}

fn ensure_site_unit(r: &mut BlastRange, site: &mut Site) -> bool {
    if let Some(id) = site.unit {
        return r.units.get(usize::from(id)).is_some_and(|u| u.status.alive);
    }
    if r.units.len() >= MAX_RANGE_UNITS {
        return false;
    }
    let body = r.matter.economy.ledger.create(Kind::Body, 0, site.required);
    let id = r.append_unit(super::produce::Spawned {
        def: RangeUnitDef {
            allied: site.allied,
            kind: site.kind,
            at: site.at,
            weapon: None,
        },
        body,
        tank: None,
    });
    r.note_construction_spawn();
    site.unit = Some(id as u16);
    true
}

fn apply_site_progress(
    r: &mut BlastRange,
    unit: usize,
    kind: RangeUnitKind,
    _at: [i32; 3],
    required: Amount,
    previous: Amount,
    delivered: Amount,
) {
    let id = usize::from(unit);
    let cells = construction_geometry(kind);
    let share = |amount: Amount| {
        ((cells.len() as u128 * u128::from(amount)) / u128::from(required.max(1))) as usize
    };
    let keep = share(delivered).min(cells.len());
    let u = &mut r.units[id];
    if previous == 0 {
        // The first delivery starts with a pristine spawned body. Cells not
        // paid for yet remain absent and are recorded for snapshot restore.
        for &(segment, cell) in cells.iter().skip(keep) {
            if let Some(segment) = segment {
                u.body.segments[segment]
                    .body
                    .remove(cell[0], cell[1], cell[2]);
                u.seg_removed.push((segment as u8, cell.map(|v| v as u8)));
            } else {
                u.body.remove(cell[0], cell[1], cell[2]);
                u.removed.push(cell.map(|v| v as u8));
            }
        }
    } else {
        // Add only the newly paid interval. Rebuilding the whole body here
        // would repair combat damage and erase dents/pending separation.
        let start = share(previous);
        for &(segment, cell) in cells.iter().take(keep).skip(start) {
            let body = if let Some(segment) = segment {
                &mut u.body.segments[segment].body
            } else {
                &mut u.body
            };
            let part = body.part_at(cell[0], cell[1], cell[2]);
            body.set(cell[0], cell[1], cell[2], part);
        }
        u.removed.retain(|p| {
            !u.body
                .vol
                .get(usize::from(p[0]), usize::from(p[1]), usize::from(p[2]))
        });
        u.seg_removed.retain(|(segment, p)| {
            !u.body.segments[usize::from(*segment)].body.vol.get(
                usize::from(p[0]),
                usize::from(p[1]),
                usize::from(p[2]),
            )
        });
    }
    let head = u.motion.drive.head;
    u.update_pose(head);
    if delivered < required {
        u.update_response();
        unfinished_status(u);
    } else {
        u.refresh_damage(id);
    }
    u.revision += 1;
}

fn unfinished_status(u: &mut RangeUnit) {
    u.status.alive = true;
    u.status.engine = false;
    u.status.weapon = false;
    u.status.sensor = false;
}

fn construction_geometry(kind: RangeUnitKind) -> &'static [(Option<usize>, [usize; 3])] {
    type Cell = (Option<usize>, [usize; 3]);
    static CELLS: [std::sync::OnceLock<Vec<Cell>>; RangeUnitKind::COUNT] =
        [const { std::sync::OnceLock::new() }; RangeUnitKind::COUNT];
    CELLS[kind.index()].get_or_init(|| {
        let unit = RangeUnit::of_kind(kind, [0; 3]);
        let root = construction_cells(kind);
        let seen: std::collections::BTreeSet<_> = root.iter().copied().collect();
        let mut cells: Vec<Cell> = root.iter().map(|p| (None, *p)).collect();
        for (segment, body) in std::iter::once((None, &unit.body)).chain(
            unit.body
                .segments
                .iter()
                .enumerate()
                .map(|(id, s)| (Some(id), &s.body)),
        ) {
            for z in 0..usize::from(body.vol.nz) {
                for y in 0..usize::from(body.vol.ny) {
                    for x in 0..usize::from(body.vol.nx) {
                        let p = [x, y, z];
                        if body.vol.get(x, y, z) && (segment.is_some() || !seen.contains(&p)) {
                            cells.push((segment, p));
                        }
                    }
                }
            }
        }
        cells
    })
}

fn construction_cells(kind: RangeUnitKind) -> &'static [[usize; 3]] {
    // Authored local geometry and BFS order do not depend on the site/side.
    // Keep them outside the delivery loop; no cache contents enter snapshots.
    static CELLS: [std::sync::OnceLock<Vec<[usize; 3]>>; RangeUnitKind::COUNT] =
        [const { std::sync::OnceLock::new() }; RangeUnitKind::COUNT];
    CELLS[kind.index()].get_or_init(|| connected_cells(&RangeUnit::of_kind(kind, [0; 3]).body))
}

fn connected_cells(body: &voxel::unit::UnitBody) -> Vec<[usize; 3]> {
    let size = [
        usize::from(body.vol.nx),
        usize::from(body.vol.ny),
        usize::from(body.vol.nz),
    ];
    let mut queue = VecDeque::new();
    let mut seen = std::collections::BTreeSet::new();
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                if body.vol.get(x, y, z) && body.part_at(x, y, z) == voxel::unit::Part::Core {
                    queue.push_back([x, y, z]);
                    seen.insert([x, y, z]);
                }
            }
        }
    }
    let mut out = Vec::new();
    while let Some(p) = queue.pop_front() {
        out.push(p);
        for (axis, sign) in [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)] {
            let mut q = p;
            q[axis] = q[axis].checked_add_signed(sign).unwrap_or(size[axis]);
            if q[axis] < size[axis] && body.vol.get(q[0], q[1], q[2]) && seen.insert(q) {
                queue.push_back(q);
            }
        }
    }
    out
}
