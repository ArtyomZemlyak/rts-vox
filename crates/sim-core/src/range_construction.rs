use super::{battle, BlastRange, RangeUnit};
use crate::matter::{Amount, ContainerId, Kind};
use fx::blob::{Blob, Scan};
use fx::Fx;
use modding::range::{RangeUnitDef, RangeUnitKind, MAX_RANGE_UNITS};
use std::collections::{BTreeMap, VecDeque};

#[cfg(test)]
#[path = "range_construction_tests.rs"]
mod tests;

const BATCH: Amount = 332_800;
const FLY_SPEED: i32 = 8;
const STEEL: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leg {
    Idle,
    ToSource,
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
}

#[derive(Default)]
pub struct Construction {
    sites: Vec<Site>,
    drones: BTreeMap<u16, Drone>,
    next_batch: u64,
    pub delivered: Amount,
    pub trips: u64,
    pub lost: Amount,
}

impl Construction {
    pub fn new(units: &[RangeUnit], sides: &[bool]) -> Self {
        let mut c = Self::default();
        for (id, u) in units.iter().enumerate() {
            if u.kind == RangeUnitKind::SkobaDrone {
                c.drones.insert(id as u16, Drone {
                    home: nearest_home(units, sides, id) as u16,
                    site: None,
                    want: 0,
                    leg: Leg::Idle,
                    cargo: None,
                });
            }
        }
        c
    }

    pub fn push_unit(&mut self, id: usize, units: &[RangeUnit], sides: &[bool]) {
        if units[id].kind == RangeUnitKind::SkobaDrone {
            self.drones.insert(id as u16, Drone {
                home: nearest_home(units, sides, id) as u16,
                site: None,
                want: 0,
                leg: Leg::Idle,
                cargo: None,
            });
        }
    }

    pub fn sites(&self) -> Vec<SiteView> {
        self.sites.iter().enumerate().map(|(id, s)| SiteView {
            id,
            kind: s.kind,
            at: s.at,
            required: s.required,
            delivered: s.delivered,
            quota: s.quota,
            assigned: self.drones.values().filter(|d| d.site == Some(id as u16)).count(),
            complete: s.delivered == s.required,
        }).collect()
    }

    pub fn cargo(&self, id: usize) -> Option<Batch> {
        self.drones.get(&(id as u16))?.cargo
    }

    pub fn leg(&self, id: usize) -> Option<Leg> {
        Some(self.drones.get(&(id as u16))?.leg)
    }

    pub fn active(&self) -> bool {
        !self.sites.is_empty()
            || self.drones.values().any(|d| d.cargo.is_some() || d.site.is_some())
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
            for v in s.at { b.i32(v); }
            b.u64(s.required);
            b.u64(s.delivered);
            b.u64(s.reserved);
            b.u8(s.quota);
            b.bool(s.unit.is_some());
            if let Some(id) = s.unit { b.u16(id); }
        }
        b.u32(self.drones.len() as u32);
        for (&id, d) in &self.drones {
            b.u16(id);
            b.u16(d.home);
            b.bool(d.site.is_some());
            if let Some(site) = d.site { b.u16(site); }
            b.u64(d.want);
            b.u8(match d.leg { Leg::Idle => 0, Leg::ToSource => 1, Leg::ToSite => 2, Leg::Returning => 3 });
            b.bool(d.cargo.is_some());
            if let Some(cargo) = d.cargo {
                b.u64(cargo.id);
                b.u16(cargo.material);
                b.u32(cargo.container);
            }
        }
    }

    pub fn load(&mut self, s: &mut Scan, units: &[RangeUnit], ledger: &crate::matter::Ledger) -> Option<()> {
        self.next_batch = s.u64();
        self.delivered = s.u64();
        self.trips = s.u64();
        self.lost = s.u64();
        let n = s.u32() as usize;
        if n > 32 || n > s.left() / 32 { return None; }
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
            if !matches!(kind, RangeUnitKind::TurretT1 | RangeUnitKind::KilFactoryT1 | RangeUnitKind::GrebenStorage)
                || required == 0 || completed > required || reserved > required - completed
                || quota > 32 || unit.is_some_and(|id| {
                    units.get(usize::from(id)).is_none_or(|u| u.kind != kind || u.at != at)
                })
            {
                return None;
            }
            delivered = delivered.checked_add(completed)?;
            self.sites.push(Site {
                kind, allied, at, required, delivered: completed, reserved, quota, unit,
            });
        }
        if delivered != self.delivered { return None; }
        let n = s.u32() as usize;
        if n > units.len() || n > s.left() / 24 { return None; }
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
                _ => return None,
            };
            let cargo = if s.bool() {
                let cargo = Batch { id: s.u64(), material: s.u16(), container: s.u32() };
                if cargo.material != STEEL
                    || ledger.try_get(cargo.container).is_none_or(|c| !c.alive || c.kind != Kind::Body)
                    || !batches.insert(cargo.id)
                    || !containers.insert(cargo.container)
                {
                    return None;
                }
                last_batch = last_batch.max(cargo.id.saturating_add(1));
                Some(cargo)
            } else { None };
            if units.get(usize::from(id)).is_none_or(|u| u.kind != RangeUnitKind::SkobaDrone)
                || usize::from(home) >= units.len()
                || site.is_some_and(|site| usize::from(site) >= self.sites.len())
                || want > BATCH
                || cargo.is_some_and(|c| site.is_some() && ledger.held(c.container) != want)
                || self.drones.insert(id, Drone { home, site, want, leg, cargo }).is_some()
            {
                return None;
            }
        }
        if self.next_batch < last_batch || !self.active() || !s.ok() { return None; }
        Some(())
    }

    fn assign_idle(&mut self) {
        for site in 0..self.sites.len() {
            let quota = usize::from(self.sites[site].quota);
            if self.sites[site].delivered == self.sites[site].required {
                continue;
            }
            let mut assigned = self.drones.values().filter(|d| d.site == Some(site as u16)).count();
            if assigned >= quota { continue; }
            let idle: Vec<u16> = self.drones.iter()
                .filter(|(_, d)| d.site.is_none() && d.cargo.is_none())
                .map(|(&id, _)| id)
                .collect();
            for id in idle {
                if assigned == quota { break; }
                let s = &mut self.sites[site];
                let left = s.required - s.delivered - s.reserved;
                if left == 0 { break; }
                let want = left.min(BATCH);
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
        let Some(s) = self.sites.get_mut(site) else { return false; };
        s.quota = quota.min(32);
        let quota = usize::from(s.quota);
        let mut assigned = self.drones.values().filter(|d| d.site == Some(site as u16)).count();
        if assigned > quota {
            let ids: Vec<u16> = self.drones.iter().rev()
                .filter(|(_, d)| d.site == Some(site as u16))
                .map(|(&id, _)| id).collect();
            for id in ids {
                if assigned <= quota { break; }
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
        if !matches!(r.battle.phase, battle::Phase::Setup | battle::Phase::Fighting) {
            return;
        }
        self.assign_idle();
        let ids: Vec<u16> = self.drones.keys().copied().collect();
        for id in ids {
            self.step_drone(r, id);
        }
    }

    fn step_drone(&mut self, r: &mut BlastRange, id: u16) {
        let index = usize::from(id);
        let Some(mut d) = self.drones.get(&id).copied() else { return; };
        if index >= r.units.len() { return; }
        if !r.units[index].status.alive {
            set_skoba_pose(&mut r.units[index], index, false);
            if let Some(batch) = d.cargo.take() {
                let amount = r.matter.economy.ledger.held(batch.container);
                if let Some(site) = d.site.and_then(|s| self.sites.get_mut(usize::from(s))) {
                    site.reserved = site.reserved.saturating_sub(d.want);
                }
                r.drop_drone_cargo(index, batch.container);
                self.lost += amount;
            }
            self.drones.insert(id, d);
            return;
        }
        let allied = r.allied(index);
        if !r.units.get(usize::from(d.home)).is_some_and(|u| u.status.alive && r.allied(usize::from(d.home)) == allied) {
            let sides: Vec<bool> = (0..r.units.len()).map(|id| r.allied(id)).collect();
            d.home = nearest_home(&r.units, &sides, index) as u16;
        }
        match d.leg {
            Leg::Idle => {}
            Leg::ToSource => {
                set_skoba_pose(&mut r.units[index], index, false);
                let target = home_target(&r.units, usize::from(d.home));
                if !fly(&mut r.units[index], target) {
                    self.drones.insert(id, d);
                    return;
                }
                let source = if r.units[usize::from(d.home)].kind == RangeUnitKind::Crawler {
                    r.mining.as_ref().and_then(|m| m.tank(usize::from(d.home)))
                } else {
                    r.mining.as_ref().and_then(|m| m.haul()).map(|h| h.store(allied))
                };
                let Some(source) = source else {
                    self.drones.insert(id, d);
                    return;
                };
                let ledger = &mut r.matter.economy.ledger;
                let cargo = ledger.create(Kind::Body, 0, d.want);
                let moved = ledger.transfer(source, cargo, d.want);
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
                let batch = Batch { id: self.next_batch, material: STEEL, container: cargo };
                self.next_batch += 1;
                d.cargo = Some(batch);
                d.leg = Leg::ToSite;
                set_skoba_loaded(&mut r.units[index], index);
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
                if !fly(&mut r.units[index], [site.at[0], site.at[1] + 100, site.at[2]]) {
                    self.drones.insert(id, d);
                    return;
                }
                let Some(batch) = d.cargo else {
                    d.leg = Leg::Returning;
                    self.drones.insert(id, d);
                    return;
                };
                if batch.material != STEEL || !ensure_site_unit(r, &mut self.sites[usize::from(site_id)]) {
                    self.drones.insert(id, d);
                    return;
                }
                let unit = self.sites[usize::from(site_id)].unit.expect("site unit");
                let body = r.matter.body_container(usize::from(unit));
                let moved = r.matter.economy.ledger.transfer(batch.container, body, d.want);
                if moved > 0 {
                    let s = &mut self.sites[usize::from(site_id)];
                    s.reserved -= moved;
                    s.delivered += moved;
                    self.delivered += moved;
                    apply_site_progress(r, usize::from(s.unit.expect("site unit")), s.kind, s.at, s.required, s.delivered);
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
                        spawn_greben_crew(r, usize::from(s.unit.expect("storage unit")));
                    }
                }
            }
            Leg::Returning => {
                let target = home_target(&r.units, usize::from(d.home));
                if !fly(&mut r.units[index], target) {
                    self.drones.insert(id, d);
                    return;
                }
                if let Some(batch) = d.cargo.take() {
                    let store = r.mining.as_ref().and_then(|m| m.haul()).map(|h| h.store(allied));
                    if let Some(store) = store {
                        let amount = r.matter.economy.ledger.held(batch.container);
                        let moved = r.matter.economy.ledger.transfer(batch.container, store, amount);
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
            }
        }
        self.drones.insert(id, d);
    }
}

fn nearest_home(units: &[RangeUnit], sides: &[bool], id: usize) -> usize {
    units.iter().enumerate()
        .filter(|(home, u)| *home != id && u.status.alive && sides.get(*home) == sides.get(id)
            && matches!(u.kind, RangeUnitKind::GrebenStorage | RangeUnitKind::Crawler))
        .min_by_key(|(home, u)| {
            let a = u.motion.pos.map(Fx::to_int_trunc);
            let b = units[id].motion.pos.map(Fx::to_int_trunc);
            (
                usize::from(u.kind != RangeUnitKind::GrebenStorage),
                (0..3).map(|k| {
                    let d = i64::from(a[k] - b[k]);
                    d * d
                }).sum::<i64>(),
                *home,
            )
        })
        .map(|(home, _)| home)
        .unwrap_or(id)
}

fn home_target(units: &[RangeUnit], home: usize) -> [i32; 3] {
    let Some(u) = units.get(home) else { return [0, 100, 0]; };
    let at = u.motion.pos.map(Fx::to_int_trunc);
    if u.kind == RangeUnitKind::GrebenStorage {
        [at[0] + 800, at[1] + 140, at[2]]
    } else {
        [at[0], at[1] + 100, at[2]]
    }
}

fn set_skoba_loaded(u: &mut RangeUnit, id: usize) {
    if u.kind != RangeUnitKind::SkobaDrone || !u.status.alive || !u.removed.is_empty() {
        return;
    }
    let mut body = voxel::unit::skoba_with_cargo();
    body.ablation_milli = u.body.ablation_milli;
    body.fracture_milli = u.body.fracture_milli;
    u.body = body;
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
    let mut body = voxel::unit::skoba_drone(compact);
    body.ablation_milli = u.body.ablation_milli;
    body.fracture_milli = u.body.fracture_milli;
    u.body = body;
    let head = u.motion.drive.head;
    u.update_pose(head);
    u.refresh_damage(id);
    u.status.alive = alive;
    u.revision += 1;
}

fn fly(u: &mut RangeUnit, target: [i32; 3]) -> bool {
    let at = u.motion.pos.map(Fx::to_int_trunc);
    let delta = [target[0] - at[0], target[1] - at[1], target[2] - at[2]];
    let distance = delta.map(i32::abs).into_iter().max().unwrap_or(0);
    let next = if distance <= FLY_SPEED {
        target
    } else {
        std::array::from_fn(|a| at[a] + delta[a] * FLY_SPEED / distance)
    };
    if delta[0] != 0 || delta[2] != 0 {
        u.motion.drive.head = fx::angle::atan2(
            Fx::from_int(delta[2]).raw(),
            Fx::from_int(delta[0]).raw(),
        );
    }
    u.motion.pos = next.map(Fx::from_int);
    let head = u.motion.drive.head;
    u.update_pose(head);
    next == target
}

impl BlastRange {
    pub fn construction_sites(&self) -> Vec<SiteView> {
        self.mining.as_ref().and_then(|m| m.construction.as_ref()).map_or_else(Vec::new, |c| c.sites())
    }

    pub fn drone_cargo(&self, id: usize) -> Option<Batch> {
        self.mining.as_ref()?.construction.as_ref()?.cargo(id)
    }

    pub fn drone_leg(&self, id: usize) -> Option<Leg> {
        self.mining.as_ref()?.construction.as_ref()?.leg(id)
    }

    pub fn set_construction_quota(&mut self, site: usize, quota: u8) -> bool {
        self.mining.as_mut().and_then(|m| m.construction.as_mut()).is_some_and(|c| c.set_quota(site, quota))
    }

    pub fn create_construction_site(&mut self, kind: RangeUnitKind, allied: bool, at: [i32; 3]) -> Option<usize> {
        if !matches!(kind, RangeUnitKind::TurretT1 | RangeUnitKind::KilFactoryT1 | RangeUnitKind::GrebenStorage)
            || self.units.len() >= MAX_RANGE_UNITS
            || self.mining.as_ref().and_then(|m| m.construction.as_ref()).is_none()
        {
            return None;
        }
        let probe = RangeUnit::of_kind(kind, at);
        let (lo, hi) = probe.bounds();
        if (0..3).any(|a| lo[a] < 1 || hi[a] >= self.size[a] as i32 - 1)
            || self.units.iter().any(|u| {
                let (a, b) = u.bounds();
                (0..3).all(|k| lo[k] < b[k] && a[k] < hi[k])
            })
        {
            return None;
        }
        let required = self.matter.template_matter(&probe);
        let quota = if self.construction_sites().is_empty() { 8 } else { 4 };
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
        if !self.units.get(id).is_some_and(|u| u.kind == RangeUnitKind::Crawler && u.status.alive && self.allied(id))
            || self.units.len() + 4 > MAX_RANGE_UNITS
            || self.mining.as_ref().and_then(|m| m.construction.as_ref())
                .is_some_and(|c| c.drones.values().any(|d| usize::from(d.home) == id))
        {
            return false;
        }
        let Some(price) = self.production_cost(RangeUnitKind::SkobaDrone) else { return false; };
        let total = price.saturating_mul(4);
        let allied = self.allied(id);
        let Some(store) = self.mining.as_ref().and_then(|m| m.haul()).map(|h| h.store(allied)) else { return false; };
        if self.matter.economy.ledger.held(store) < total { return false; }
        let paid = self.matter.economy.ledger.create(Kind::Body, 0, total);
        assert_eq!(self.matter.economy.ledger.transfer(store, paid, total), total);
        let at = self.units[id].motion.pos.map(Fx::to_int_trunc);
        for offset in [-36, -12, 12, 36] {
            let blank = self.matter.economy.ledger.create(Kind::Body, 0, price);
            assert_eq!(self.matter.economy.ledger.transfer(paid, blank, price), price);
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
        let Some(mut c) = self.mining.as_mut().and_then(|m| m.construction.take()) else { return; };
        c.step(self);
        if let Some(m) = self.mining.as_mut() {
            m.construction = Some(c);
        }
    }

    pub(crate) fn has_construction(&self) -> bool {
        self.mining.as_ref().and_then(|m| m.construction.as_ref()).is_some_and(|c| c.active())
    }

    pub(crate) fn save_construction(&self, b: &mut Blob) {
        if let Some(c) = self.mining.as_ref().and_then(|m| m.construction.as_ref()) {
            c.save(b);
        }
    }

    pub(crate) fn load_construction(&mut self, s: &mut Scan) -> Option<()> {
        let c = self.mining.as_mut()?.construction.as_mut()?;
        c.load(s, &self.units, &self.matter.economy.ledger)
    }
}

fn spawn_greben_crew(r: &mut BlastRange, home: usize) {
    let at = r.units[home].motion.pos.map(Fx::to_int_trunc);
    let allied = r.allied(home);
    let direction = if at[0] + 900 < r.size[0] as i32 { 1 } else { -1 };
    for n in 0..8 {
        if r.units.len() >= MAX_RANGE_UNITS { break; }
        spawn_initial_unit(
            r,
            RangeUnitKind::SkobaDrone,
            allied,
            [at[0] + direction * 800, at[1] + 44, at[2] - 240 + n * 70],
        );
    }
    let crawler_near = r.units.iter().enumerate().any(|(id, u)| {
        id != home && u.kind == RangeUnitKind::Crawler && r.allied(id) == allied
            && (0..3).map(|a| {
                let p = u.motion.pos[a].to_int_trunc();
                let d = i64::from(p - at[a]);
                d * d
            }).sum::<i64>() <= 2000 * 2000
    });
    if !crawler_near && r.units.len() < MAX_RANGE_UNITS {
        spawn_initial_unit(
            r,
            RangeUnitKind::Crawler,
            allied,
            [at[0] + direction * 800, at[1], at[2] + 500],
        );
    }
}

fn spawn_initial_unit(r: &mut BlastRange, kind: RangeUnitKind, allied: bool, at: [i32; 3]) {
    let probe = RangeUnit::of_kind(kind, at);
    let cost = r.matter.template_matter(&probe);
    let body = r.matter.economy.ledger.create(Kind::Body, cost, cost);
    let tank = if kind == RangeUnitKind::Crawler {
        let cap = r.mining.as_ref().and_then(|m| m.data().kind(kind)).map_or(0, |k| k.tank);
        Some(r.matter.economy.ledger.create(Kind::Hauler, 0, cap))
    } else {
        None
    };
    r.append_unit(super::produce::Spawned {
        def: RangeUnitDef { allied, kind, at, weapon: None },
        body,
        tank,
    });
    r.note_construction_spawn();
}

fn ensure_site_unit(r: &mut BlastRange, site: &mut Site) -> bool {
    if let Some(id) = site.unit {
        return r.units.get(usize::from(id)).is_some_and(|u| u.status.alive);
    }
    if r.units.len() >= MAX_RANGE_UNITS { return false; }
    let body = r.matter.economy.ledger.create(Kind::Body, 0, site.required);
    let id = r.append_unit(super::produce::Spawned {
        def: RangeUnitDef { allied: site.allied, kind: site.kind, at: site.at, weapon: None },
        body,
        tank: None,
    });
    r.note_construction_spawn();
    site.unit = Some(id as u16);
    true
}

fn apply_site_progress(r: &mut BlastRange, unit: usize, kind: RangeUnitKind, at: [i32; 3], required: Amount, delivered: Amount) {
    let density = r.matter.source_density(STEEL) as Amount;
    if density == 0 { return; }
    let keep = (delivered / density).min((required / density).max(1)) as usize;
    let id = usize::from(unit);
    let revision = r.units[id].revision;
    let motion = r.units[id].motion.clone();
    let mut fresh = RangeUnit::placed(kind, r.allied(id), at, true);
    fresh.motion = motion;
    let cells = connected_cells(&fresh.body);
    for &cell in cells.iter().skip(keep) {
        fresh.body.remove(cell[0], cell[1], cell[2]);
        fresh.removed.push(cell.map(|v| v as u8));
    }
    let head = fresh.motion.drive.head;
    fresh.update_pose(head);
    fresh.refresh_damage(id);
    r.units[id] = fresh;
    r.units[id].revision = revision + 1;
}

fn connected_cells(body: &voxel::unit::UnitBody) -> Vec<[usize; 3]> {
    let size = [usize::from(body.vol.nx), usize::from(body.vol.ny), usize::from(body.vol.nz)];
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
