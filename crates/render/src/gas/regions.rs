//! Bounded presentation state. Shot/reload state never owns a plume's lifetime.
use super::{GasParams, TerrainWindow, GAS_CELLS, GAS_N};
use bevy::prelude::*;
use std::collections::VecDeque;
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
    mask_window: TerrainWindow,
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
    /// Resize already visible cubes without restarting any field or clock.
    pub(super) fn resize_display(&mut self, old: f32, edge: f32) {
        for r in self.regions.iter_mut().flatten() {
            r.params.charge.z = edge * (r.params.charge.z / old);
            for d in &mut r.deposits {
                d.params.charge.z = edge * (d.params.charge.z / old);
            }
        }
        for (_, p) in &mut self.pending {
            p.charge.z = edge * (p.charge.z / old);
        }
    }
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
    pub fn advance(
        &mut self,
        clock: u32,
        words: &[u32],
        window: TerrainWindow,
        dirty: &[Range<usize>],
    ) {
        self.advance_impl(clock, words, window, dirty, false);
    }
    pub(super) fn advance_gpu(
        &mut self,
        clock: u32,
        words: &[u32],
        window: TerrainWindow,
        dirty: &[Range<usize>],
    ) {
        self.advance_impl(clock, words, window, dirty, true);
    }
    fn advance_impl(
        &mut self,
        clock: u32,
        words: &[u32],
        window: TerrainWindow,
        dirty: &[Range<usize>],
        gpu: bool,
    ) {
        self.clock = clock;
        for slot in &mut self.regions {
            if let Some(r) = slot {
                let done = r.applied.load(Ordering::Relaxed);
                r.deposits.retain(|d| d.serial > done);
                if r.opacity(clock) <= 0.0 {
                    *slot = None;
                } else if !dirty.is_empty() || window != r.mask_window {
                    if gpu {
                        if window != r.mask_window || touches_mask(r, window, dirty) {
                            r.mask_window = window;
                            r.mask_revision += 1;
                        }
                    } else {
                        refresh_mask(r, words, window, dirty);
                    }
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
                let mask = if gpu {
                    Vec::new()
                } else {
                    make_mask(words, window, p.origin)
                };
                self.regions[i] = Some(Region {
                    id: self.serial,
                    born: clock,
                    last: clock,
                    params: p,
                    deposits: VecDeque::new(),
                    applied: Arc::new(AtomicU64::new(0)),
                    mask: Arc::new(mask),
                    mask_revision: 1,
                    mask_window: window,
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

pub(super) fn surface_source(
    words: &[u32],
    window: TerrainWindow,
    point: Vec3,
    outward: Vec3,
    edge: i32,
) -> Vec3 {
    if edge <= 0 || words.is_empty() {
        return point;
    }
    let direction = outward.normalize_or_zero();
    // At most four field cells toward the incoming projectile (or upwards
    // for a burning surface). Never choose the far side of the obstacle.
    // The surrounding cube covers every possible alignment of a gas cell.
    for step in 0..=8 {
        let candidate = point + direction * (step as f32 * edge as f32 * 0.5);
        let base = candidate.floor().as_ivec3() - IVec3::splat(edge - 1);
        if solid(words, window, base, edge * 2 - 1) == 0 {
            return candidate;
        }
    }
    // Fully buried emissions remain blocked by the field's occupancy mask.
    point
}

fn solid(words: &[u32], window: TerrainWindow, base: IVec3, edge: i32) -> u32 {
    let lo = base.max(IVec3::from_array(window.origin));
    let hi = (base + IVec3::splat(edge))
        .min(IVec3::from_array(window.origin) + UVec3::from_array(window.dims).as_ivec3());
    for z in lo.z..hi.z {
        let depth = (z as usize % window.dims[2] as usize) * window.dims[1] as usize;
        for y in lo.y..hi.y {
            let row = window.dims[0] as usize * (depth + y as usize % window.dims[1] as usize);
            let mut x = lo.x;
            let mut column = x as usize % window.dims[0] as usize;
            while x < hi.x {
                let i = row + column;
                // Test up to eight packed cells at once, stopping at a word,
                // row or toroidal seam. The mask is exact even on odd widths.
                let count = (hi.x - x)
                    .min(8 - (i % 8) as i32)
                    .min(window.dims[0] as i32 - column as i32);
                let mask = (u32::MAX >> ((8 - count) * 4)) << ((i % 8) * 4);
                if words.get(i / 8).is_some_and(|w| w & mask != 0) {
                    return 1;
                }
                x += count;
                column += count as usize;
                if column == window.dims[0] as usize {
                    column = 0;
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
fn make_mask(words: &[u32], window: TerrainWindow, origin: Vec4) -> Vec<u32> {
    let mut mask = vec![0; GAS_CELLS];
    let edge = origin.w as i32;
    let corner = origin.truncate().as_ivec3();
    let lo = corner.max(IVec3::from_array(window.origin));
    let hi = (corner + IVec3::splat(GAS_N as i32 * edge))
        .min(IVec3::from_array(window.origin) + UVec3::from_array(window.dims).as_ivec3());
    // Scan contiguous packed terrain rows. Empty words skip eight voxels;
    // occupied rows populate a whole gas row, then stop once it is solid.
    // Gathering each gas cell separately re-read the same rows with strides.
    for gz in 0..GAS_N {
        for gy in 0..GAS_N {
            let target = &mut mask[GAS_N * (gy + GAS_N * gz)..GAS_N * (gy + GAS_N * gz + 1)];
            let z0 = corner.z + gz as i32 * edge;
            let y0 = corner.y + gy as i32 * edge;
            for z in z0.max(lo.z)..(z0 + edge).min(hi.z) {
                let depth = z as usize % window.dims[2] as usize * window.dims[1] as usize;
                for y in y0.max(lo.y)..(y0 + edge).min(hi.y) {
                    let row =
                        window.dims[0] as usize * (depth + y as usize % window.dims[1] as usize);
                    let mut x = lo.x;
                    let mut column = x as usize % window.dims[0] as usize;
                    while x < hi.x {
                        let i = row + column;
                        let count = (hi.x - x)
                            .min(8 - (i % 8) as i32)
                            .min(window.dims[0] as i32 - column as i32);
                        let mut packed = words.get(i / 8).copied().unwrap_or(0) >> ((i % 8) * 4);
                        packed &= u32::MAX >> ((8 - count) * 4);
                        while packed != 0 {
                            let lane = packed.trailing_zeros() / 4;
                            target[((x + lane as i32 - corner.x) / edge) as usize] = 1;
                            packed &= !(15 << (lane * 4));
                        }
                        x += count;
                        column += count as usize;
                        if column == window.dims[0] as usize {
                            column = 0;
                        }
                    }
                    if target.iter().all(|&v| v != 0) {
                        break;
                    }
                }
                if target.iter().all(|&v| v != 0) {
                    break;
                }
            }
        }
    }
    mask
}
// A conservative overlap test: stop at the first changed packed word
// intersecting the field. No per-cell temporary vector or terrain scan.
fn touches_mask(r: &Region, window: TerrainWindow, spans: &[Range<usize>]) -> bool {
    let lo = r.params.origin.truncate().as_ivec3();
    let hi = lo + IVec3::splat(GAS_N as i32 * r.params.origin.w as i32);
    if spans.iter().map(|s| s.len()).sum::<usize>() > GAS_CELLS / 8 {
        return true;
    }
    for span in spans {
        for word in span.clone() {
            let first = window.position(word * 8);
            let contiguous = word * 8 % window.dims[0] as usize + 8 <= window.dims[0] as usize
                && first[0] + 8 <= window.origin[0] + window.dims[0] as i32;
            if contiguous {
                if first[0] < hi.x
                    && first[0] + 8 > lo.x
                    && first[1] >= lo.y
                    && first[1] < hi.y
                    && first[2] >= lo.z
                    && first[2] < hi.z
                {
                    return true;
                }
            } else {
                for lane in 0..8 {
                    let index = word * 8 + lane;
                    if index >= window.dims.iter().map(|&n| n as usize).product() {
                        break;
                    }
                    let p = IVec3::from_array(window.position(index));
                    if p.cmpge(lo).all() && p.cmplt(hi).all() {
                        return true;
                    }
                }
            }
        }
    }
    false
}
fn refresh_mask(r: &mut Region, words: &[u32], window: TerrainWindow, spans: &[Range<usize>]) {
    // Bulk world uploads must not turn into a walk over hundreds of millions
    // of terrain cells for each small gas volume. Rebuild the bounded volume.
    if window != r.mask_window || spans.iter().map(|s| s.len()).sum::<usize>() > GAS_CELLS / 8 {
        r.mask = Arc::new(make_mask(words, window, r.params.origin));
        r.mask_window = window;
        r.mask_revision += 1;
        return;
    }
    let mut visited = vec![false; GAS_CELLS];
    let mut changed = Vec::new();
    let edge = r.params.origin.w as i32;
    let origin = r.params.origin.truncate().as_ivec3();
    for span in spans {
        for word in span.clone() {
            let first = window.position(word * 8);
            let contiguous = word * 8 % window.dims[0] as usize + 8 <= window.dims[0] as usize
                && first[0] + 8 <= window.origin[0] + window.dims[0] as i32;
            // A packed word can straddle both a world row and a gas cell.
            for i in word * 8..word * 8 + 8 {
                if i >= window.dims.iter().map(|&v| v as usize).product::<usize>() {
                    break;
                }
                let p = IVec3::from_array(if contiguous {
                    [first[0] + (i % 8) as i32, first[1], first[2]]
                } else {
                    window.position(i)
                }) - origin;
                let q = IVec3::new(
                    p.x.div_euclid(edge),
                    p.y.div_euclid(edge),
                    p.z.div_euclid(edge),
                );
                if q.cmpge(IVec3::ZERO).all() && q.cmplt(IVec3::splat(GAS_N as i32)).all() {
                    let index = q.x as usize + GAS_N * (q.y as usize + GAS_N * q.z as usize);
                    if !visited[index] {
                        visited[index] = true;
                        changed.push(index);
                    }
                }
            }
        }
    }
    if !changed.is_empty() {
        let mask = Arc::make_mut(&mut r.mask);
        for i in changed {
            mask[i] = solid(words, window, base(i, r.params.origin), edge);
        }
        r.mask_revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_mask_invalidation_covers_each_changed_voxel_across_wraps() {
        let window = TerrainWindow {
            origin: [79, 0, 71],
            dims: [97, 32, 96],
        };
        let words = vec![0u32; (97 * 32 * 96usize).div_ceil(8)];
        let mut a = Atmosphere::default();
        a.emit(shot(100.0, false));
        a.advance_gpu(0, &words, window, &[]);
        let r = a.regions[0].as_mut().unwrap();
        assert!(r.mask.is_empty(), "the GPU path must not build a CPU mask");
        for origin in [
            Vec4::new(70.0, 0.0, 60.0, 1.0),
            Vec4::new(89.0, 0.0, 87.0, 3.0),
        ] {
            r.params.origin = origin;
            let lo = origin.truncate().as_ivec3();
            let hi = lo + IVec3::splat(GAS_N as i32 * origin.w as i32);
            for start in (0..words.len() - 3).step_by(37) {
                let span = start..start + 3;
                let expected = (span.start * 8..span.end * 8).any(|i| {
                    let p = IVec3::from_array(window.position(i));
                    p.cmpge(lo).all() && p.cmplt(hi).all()
                });
                assert_eq!(touches_mask(r, window, &[span]), expected);
            }
        }
        let revision = r.mask_revision;
        a.advance_gpu(1, &words, window, &[]);
        assert_eq!(a.regions[0].as_ref().unwrap().mask_revision, revision);
        a.advance_gpu(
            2,
            &words,
            TerrainWindow {
                origin: [80, 0, 71],
                ..window
            },
            &[],
        );
        assert_eq!(a.regions[0].as_ref().unwrap().mask_revision, revision + 1);
    }
    #[test]
    fn packed_mask_matches_voxel_reference_across_window_and_word_seams() {
        let window = TerrainWindow {
            origin: [79, 0, 71],
            dims: [97, 32, 96],
        };
        let mut words = vec![0; (97 * 32 * 96usize).div_ceil(8)];
        for p in [[96, 12, 95], [97, 13, 96], [79, 0, 71], [175, 31, 166]] {
            let i = window.index(p).unwrap();
            words[i / 8] |= 15 << ((i % 8) * 4);
        }
        for origin in [
            Vec4::new(70.0, 0.0, 60.0, 3.0),
            Vec4::new(89.0, 0.0, 87.0, 5.0),
        ] {
            let mask = make_mask(&words, window, origin);
            for (i, &got) in mask.iter().enumerate() {
                let p = base(i, origin);
                let edge = origin.w as i32;
                let expected = (0..edge).any(|z| {
                    (0..edge).any(|y| {
                        (0..edge).any(|x| {
                            window
                                .index((p + IVec3::new(x, y, z)).to_array())
                                .is_some_and(|j| words[j / 8] >> ((j % 8) * 4) & 15 != 0)
                        })
                    })
                });
                assert_eq!(got, u32::from(expected), "cell {i}");
            }
        }
        let mut a = Atmosphere::default();
        a.emit(shot(100.0, false));
        a.advance(0, &words, window, &[]);
        let i = window.index([100, 18, 102]).unwrap();
        words[i / 8] |= 15 << ((i % 8) * 4);
        a.advance(2, &words, window, std::slice::from_ref(&(i / 8..i / 8 + 1)));
        let r = a.regions[0].as_ref().unwrap();
        assert_eq!(*r.mask, make_mask(&words, window, r.params.origin));
        let moved = TerrainWindow {
            origin: [370, 0, 359],
            ..window
        };
        a.advance(4, &words, moved, &[]);
        let r = a.regions[0].as_ref().unwrap();
        assert_eq!(*r.mask, make_mask(&words, moved, r.params.origin));
        assert!(
            !r.mask.contains(&1),
            "old world cells must leave the gas mask"
        );
    }
    fn shot(x: f32, energy: bool) -> GasParams {
        GasParams {
            source: Vec4::new(x, 20.0, 100.0, 15.0),
            weapon: Vec4::new(f32::from(energy), 0.0, 0.0, 0.0),
            ..GasParams::default()
        }
    }
    fn advance(a: &mut Atmosphere, tick: u32) {
        a.advance(tick, &[], TerrainWindow::default(), &[]);
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
        let window = TerrainWindow {
            origin: [0; 3],
            dims: dims.to_array(),
        };
        let mut words = vec![0; (dims.x * dims.y * dims.z).div_ceil(8) as usize];
        let mut a = Atmosphere::default();
        a.emit(shot(30.0, false));
        a.emit(shot(30.0, true));
        a.advance(0, &words, window, &[]);
        // Packed words straddle rows; both the old and new occupied location
        // must enter each region's mask, including negative region origins.
        let i = (29 + dims.x * (16 + dims.y * 40)) as usize;
        words[i / 8] |= 15 << (i % 8 * 4);
        a.advance(2, &words, window, std::slice::from_ref(&(i / 8..i / 8 + 1)));
        assert!(a.regions[0].as_ref().unwrap().mask.contains(&1));
        words[i / 8] = 0;
        let j = (78 + dims.x * (18 + dims.y * 51)) as usize;
        words[j / 8] |= 15 << (j % 8 * 4);
        a.advance(4, &words, window, &[i / 8..i / 8 + 1, j / 8..j / 8 + 1]);
        for r in a.regions.iter().flatten() {
            assert_eq!(*r.mask, make_mask(&words, window, r.params.origin));
        }
    }
}

#[cfg(test)]
mod surface_tests {
    use super::*;
    #[test]
    fn resizing_visible_and_queued_clouds_keeps_their_field_and_lifetime() {
        let window = TerrainWindow {
            origin: [0; 3],
            dims: [8; 3],
        };
        let mut p = GasParams::default();
        p.source = Vec4::new(40.0, 40.0, 40.0, 5.0);
        p.charge.z = 3.2;
        let mut a = Atmosphere::default();
        a.emit_at(p, 10);
        a.advance_gpu(10, &[0; 64], window, &[]);
        a.emit_at(p, 12);
        let before = a.regions[0].as_ref().unwrap().clone();
        for edge in [6.4, 7.2, 3.2] {
            a.resize_display(p.charge.z, edge);
            p.charge.z = edge;
            let r = a.regions[0].as_ref().unwrap();
            assert_eq!(
                (r.id, r.born, r.last, r.mask_revision),
                (before.id, before.born, before.last, before.mask_revision)
            );
            assert_eq!(r.params.origin, before.params.origin);
            assert_eq!(r.params.chemistry, before.params.chemistry);
            assert_eq!(r.params.charge.z, edge);
            assert_eq!(r.deposits[0].params.charge.z, edge);
            assert_eq!(a.pending[0].1.charge.z, edge);
            assert!(r.accepts(&p));
        }
    }
    #[test]
    fn surface_deposits_stay_in_incident_air_for_any_field_alignment() {
        let window = TerrainWindow {
            origin: [40, 0, 56],
            dims: [64, 64, 64],
        };
        for axis in 0..3 {
            let mut words = vec![0u32; window.words()];
            for z in window.span(2) {
                for y in window.span(1) {
                    for x in window.span(0) {
                        let q = [x, y, z];
                        if q[axis] == window.origin[axis] + 32 {
                            let i = window.index(q).unwrap();
                            words[i / 8] |= 3 << ((i % 8) * 4);
                        }
                    }
                }
            }
            for sign in [-1.0, 1.0] {
                let mut point = Vec3::from(window.origin.map(|v| v as f32)) + Vec3::splat(32.0);
                point[axis] += if sign > 0.0 { 0.99 } else { 0.0 };
                let mut outward = Vec3::ZERO;
                outward[axis] = sign;
                let result = surface_source(&words, window, point, outward, 5);
                assert!((result - point).dot(outward) > 0.0);
                assert!((result - point).length() <= 20.0);
                for dz in 0..5 {
                    for dy in 0..5 {
                        for dx in 0..5 {
                            let base = result.floor().as_ivec3() - IVec3::new(dx, dy, dz);
                            assert_eq!(
                                solid(&words, window, base, 5),
                                0,
                                "axis={axis} sign={sign}"
                            );
                        }
                    }
                }
            }
        }
        let words = vec![0; window.words()];
        let point = Vec3::new(55.5, 20.1, 90.2);
        assert_eq!(surface_source(&words, window, point, Vec3::X, 5), point);
        let words = vec![u32::MAX; window.words()];
        assert_eq!(surface_source(&words, window, point, Vec3::ZERO, 5), point);
    }
}
