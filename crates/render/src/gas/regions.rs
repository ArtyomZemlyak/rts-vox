//! Bounded presentation state. Shot/reload state never owns a plume's lifetime.
use super::{GasParams, GAS_CELLS, GAS_N};
use bevy::prelude::*;
use std::collections::{BTreeSet, VecDeque};
use std::ops::Range;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub const REGIONS: usize = 8;
const PENDING: usize = 32;
const DEPOSITS: usize = 16;
const LIFETIME: u32 = 24 * 60;
const FADE: u32 = 3 * 60;

#[derive(Clone)]
pub(super) struct Deposit {
    pub serial: u64,
    pub step: u32,
    pub params: GasParams,
}
#[derive(Clone)]
pub struct Region {
    pub(super) id: u64,
    pub(super) born: u32,
    pub(super) last: u32,
    pub(super) params: GasParams,
    pub(super) deposits: VecDeque<Deposit>,
    pub(super) applied: Arc<AtomicU64>,
    pub(super) mask: Arc<Vec<u32>>,
    pub(super) mask_revision: u64,
    retire: Option<u32>,
}
impl Region {
    pub(super) fn opacity(&self, now: u32) -> f32 {
        let end = (self.last + LIFETIME).min(self.retire.unwrap_or(u32::MAX));
        ((end.saturating_sub(now)) as f32 / FADE as f32).min(1.0)
    }
    fn accepts(&self, p: &GasParams) -> bool {
        let margin = (p.source.w * 1.5).max(self.params.origin.w * 2.0);
        let v = p.source.truncate() - self.params.origin.truncate();
        self.retire.is_none()
            && self.params.weapon.x == p.weapon.x
            && self.params.chemistry == p.chemistry
            && self.params.charge == p.charge
            && self.params.origin.w == p.origin.w
            && self.deposits.len() < DEPOSITS
            && v.x.min(v.z) > margin
            && v.x.max(v.z) < GAS_N as f32 * self.params.origin.w - margin
            && v.y >= 0.0
            && v.y < GAS_N as f32 * self.params.origin.w - margin
    }
}

#[derive(Clone, Default)]
pub struct Atmosphere {
    pub(super) regions: [Option<Region>; REGIONS],
    pub(super) clock: u32,
    serial: u64,
    pending: VecDeque<(u32, GasParams)>,
    /// Presentation overload only: never removes an already visible effect.
    pub omitted: u64,
}
impl Atmosphere {
    pub fn clear(&mut self) {
        self.regions.fill(None);
        self.pending.clear();
        self.clock = 0;
        // IDs survive a scene reset, so a reused slot cannot reuse old GPU gas.
    }
    pub fn emit(&mut self, params: GasParams) {
        self.emit_at(params, self.clock);
    }
    pub fn emit_at(&mut self, params: GasParams, clock: u32) {
        if self.pending.len() < PENDING {
            self.pending.push_back((clock, params));
        } else {
            self.omitted += 1;
        }
    }
    pub fn advance(&mut self, clock: u32, words: &[u32], dims: UVec3, dirty: &[Range<usize>]) {
        self.clock = clock;
        for slot in &mut self.regions {
            if let Some(r) = slot {
                let done = r.applied.load(Ordering::Relaxed);
                r.deposits.retain(|d| d.serial > done);
                if r.opacity(clock) <= 0.0 {
                    *slot = None;
                } else if !dirty.is_empty() {
                    refresh_mask(r, words, dims, dirty);
                }
            }
        }
        while let Some(&(at, mut p)) = self.pending.front() {
            let nearby = self
                .regions
                .iter()
                .position(|r| r.as_ref().is_some_and(|r| r.accepts(&p)));
            let free = self.regions.iter().position(Option::is_none);
            let Some(i) = nearby.or(free) else {
                // Under pressure retire one region smoothly, keeping the new
                // deposits queued. Never teleport a live volume to a new hit.
                if !self.regions.iter().flatten().any(|r| r.retire.is_some()) {
                    let r = self
                        .regions
                        .iter_mut()
                        .flatten()
                        .min_by_key(|r| (r.last, r.id))
                        .unwrap();
                    r.retire = Some(clock + FADE);
                }
                break;
            };
            self.pending.pop_front();
            self.serial += 1;
            if nearby.is_none() {
                p.origin.x = (p.source.x - GAS_N as f32 * p.origin.w * 0.5).floor();
                p.origin.z = (p.source.z - GAS_N as f32 * p.origin.w * 0.5).floor();
                p.origin.y = (p.source.y - GAS_N as f32 * p.origin.w * 0.15)
                    .floor()
                    .max(0.0);
                let mask = make_mask(words, dims, p.origin);
                self.regions[i] = Some(Region {
                    id: self.serial,
                    born: clock,
                    last: clock,
                    params: p,
                    deposits: VecDeque::new(),
                    applied: Arc::new(AtomicU64::new(0)),
                    mask: Arc::new(mask),
                    mask_revision: 1,
                    retire: None,
                });
            }
            let r = self.regions[i].as_mut().unwrap();
            p.origin = r.params.origin;
            r.params = p;
            r.last = clock;
            r.deposits.push_back(Deposit {
                serial: self.serial,
                step: at.saturating_sub(r.born),
                params: p,
            });
        }
    }
}

fn solid(words: &[u32], dims: UVec3, base: IVec3, edge: i32) -> u32 {
    for z in base.z.max(0)..(base.z + edge).min(dims.z as i32) {
        for y in base.y.max(0)..(base.y + edge).min(dims.y as i32) {
            let row = dims.x as usize * (y as usize + dims.y as usize * z as usize);
            for x in base.x.max(0)..(base.x + edge).min(dims.x as i32) {
                let i = row + x as usize;
                if words
                    .get(i / 8)
                    .is_some_and(|w| (w >> (i % 8 * 4)) & 15 != 0)
                {
                    return 1;
                }
            }
        }
    }
    0
}
fn base(i: usize, origin: Vec4) -> IVec3 {
    origin.truncate().as_ivec3()
        + IVec3::new(
            (i % GAS_N) as i32,
            (i / GAS_N % GAS_N) as i32,
            (i / GAS_N / GAS_N) as i32,
        ) * origin.w as i32
}
fn make_mask(words: &[u32], dims: UVec3, origin: Vec4) -> Vec<u32> {
    (0..GAS_CELLS)
        .map(|i| solid(words, dims, base(i, origin), origin.w as i32))
        .collect()
}
fn refresh_mask(r: &mut Region, words: &[u32], dims: UVec3, spans: &[Range<usize>]) {
    let mut changed = BTreeSet::new();
    let edge = r.params.origin.w as i32;
    let origin = r.params.origin.truncate().as_ivec3();
    for span in spans {
        for word in span.clone() {
            // A packed word can straddle both a world row and a gas cell.
            for i in word * 8..word * 8 + 8 {
                let p = IVec3::new(
                    (i % dims.x as usize) as i32,
                    (i / dims.x as usize % dims.y as usize) as i32,
                    (i / dims.x as usize / dims.y as usize) as i32,
                ) - origin;
                let q = IVec3::new(
                    p.x.div_euclid(edge),
                    p.y.div_euclid(edge),
                    p.z.div_euclid(edge),
                );
                if q.cmpge(IVec3::ZERO).all() && q.cmplt(IVec3::splat(GAS_N as i32)).all() {
                    changed.insert(q.x as usize + GAS_N * (q.y as usize + GAS_N * q.z as usize));
                }
            }
        }
    }
    if !changed.is_empty() {
        let mask = Arc::make_mut(&mut r.mask);
        for i in changed {
            mask[i] = solid(words, dims, base(i, r.params.origin), edge);
        }
        r.mask_revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shot(x: f32, energy: bool) -> GasParams {
        GasParams {
            source: Vec4::new(x, 20.0, 100.0, 15.0),
            weapon: Vec4::new(f32::from(energy), 0.0, 0.0, 0.0),
            ..GasParams::default()
        }
    }
    fn advance(a: &mut Atmosphere, tick: u32) {
        a.advance(tick, &[], UVec3::new(640, 192, 640), &[]);
    }
    #[test]
    fn refire_keeps_old_region_and_mixed_weapons_keep_their_colour() {
        let mut a = Atmosphere::default();
        a.emit(shot(150.0, false));
        advance(&mut a, 0);
        let id = a.regions[0].as_ref().unwrap().id;
        advance(&mut a, 120);
        a.emit(shot(160.0, false));
        a.emit(shot(160.0, true));
        a.emit(shot(450.0, false));
        advance(&mut a, 122);
        let r = a.regions[0].as_ref().unwrap();
        assert_eq!(r.id, id);
        assert_eq!(r.born, 0);
        assert_eq!(r.deposits.len(), 2);
        assert_eq!(r.deposits[1].step, 120);
        assert_eq!(a.regions.iter().flatten().count(), 3);
        assert_eq!(r.params.weapon.x, 0.0);
        assert_eq!(a.regions[1].as_ref().unwrap().params.weapon.x, 1.0);
        let old_id = a.serial;
        a.clear();
        a.emit(shot(150.0, false));
        advance(&mut a, 0);
        assert!(a.regions[0].as_ref().unwrap().id > old_id);
    }
    #[test]
    fn overload_fades_before_recycling_and_upload_ack_does_not_kill_a_plume() {
        let mut a = Atmosphere::default();
        for i in 0..REGIONS {
            a.emit(shot(i as f32 * 300.0, false));
        }
        advance(&mut a, 0);
        let r = a.regions[0].as_ref().unwrap();
        r.applied.store(r.deposits[0].serial, Ordering::Relaxed);
        a.emit(shot(4000.0, false));
        advance(&mut a, 60);
        let r = a.regions[0].as_ref().unwrap();
        assert!(r.deposits.is_empty());
        assert_eq!(r.opacity(60), 1.0);
        assert_eq!(a.pending.len(), 1);
        advance(&mut a, 60 + FADE / 2);
        assert_eq!(a.regions[0].as_ref().unwrap().opacity(a.clock), 0.5);
        advance(&mut a, 60 + FADE);
        assert!(a.pending.is_empty());
        assert_eq!(a.regions[0].as_ref().unwrap().params.source.x, 4000.0);
        advance(&mut a, 60 + FADE + LIFETIME);
        assert!(a.regions.iter().all(Option::is_none));
    }
    #[test]
    fn every_region_observes_wall_removal_and_moving_occupancy() {
        let dims = UVec3::new(97, 32, 96);
        let mut words = vec![0; (dims.x * dims.y * dims.z).div_ceil(8) as usize];
        let mut a = Atmosphere::default();
        a.emit(shot(30.0, false));
        a.emit(shot(30.0, true));
        a.advance(0, &words, dims, &[]);
        // Packed words straddle rows; both the old and new occupied location
        // must enter each region's mask, including negative region origins.
        let i = (29 + dims.x * (16 + dims.y * 40)) as usize;
        words[i / 8] |= 15 << (i % 8 * 4);
        a.advance(2, &words, dims, std::slice::from_ref(&(i / 8..i / 8 + 1)));
        assert!(a.regions[0].as_ref().unwrap().mask.contains(&1));
        words[i / 8] = 0;
        let j = (78 + dims.x * (18 + dims.y * 51)) as usize;
        words[j / 8] |= 15 << (j % 8 * 4);
        a.advance(4, &words, dims, &[i / 8..i / 8 + 1, j / 8..j / 8 + 1]);
        for r in a.regions.iter().flatten() {
            assert_eq!(*r.mask, make_mask(&words, dims, r.params.origin));
        }
    }
}
