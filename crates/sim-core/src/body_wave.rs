//! Frozen unit geometry in a bounded blast domain. Raster and damage cursors
//! consume the same deterministic work budgets as terrain; no per-voxel queue
//! is built in advance. Coarse resistance conservatively covers rotated cells.
use crate::damage::{Bodies, Frame};
use fx::{
    blob::{Blob, Scan},
    Fx,
};
use phys::V3;
use std::sync::Arc;
use voxel::UnitBody;

#[derive(Clone)]
struct Actor {
    id: u32,
    generation: u32,
    template: u16,
    body: Arc<UnitBody>,
    frame: Frame,
    layers: [u8; 2],
    lo: [i32; 3],
    hi: [i32; 3],
    shape_hash: u64,
}
#[derive(Default)]
pub struct Field {
    actors: Vec<Actor>,
    build_actor: usize,
    build_cell: usize,
    probe: Option<Probe>,
    hit_actor: usize,
    hit_cell: usize,
}
struct Probe {
    lo: [i32; 3],
    hi: [i32; 3],
    cursor: usize,
    resistance: u32,
}
fn arr(v: V3) -> [Fx; 3] {
    [v.x, v.y, v.z]
}
fn vec(a: [Fx; 3]) -> V3 {
    V3::new(a[0], a[1], a[2])
}
fn volume(lo: [i32; 3], hi: [i32; 3]) -> usize {
    (0..3).map(|a| (hi[a] - lo[a]).max(0) as usize).product()
}
fn point(i: usize, lo: [i32; 3], hi: [i32; 3]) -> [i32; 3] {
    let x = (hi[0] - lo[0]) as usize;
    let y = (hi[1] - lo[1]) as usize;
    [
        lo[0] + (i % x) as i32,
        lo[1] + (i / x % y) as i32,
        lo[2] + (i / (x * y)) as i32,
    ]
}
fn index(p: [i32; 3], n: usize) -> usize {
    p[0] as usize + n * (p[1] as usize + n * p[2] as usize)
}
fn local_box(a: &Actor, lo: [i32; 3], hi: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    let mut low = [Fx::MAX; 3];
    let mut high = [Fx::MIN; 3];
    for bits in 0..8 {
        let p = a.frame.local(vec(std::array::from_fn(|k| {
            Fx::from_int(if bits & (1 << k) == 0 { lo[k] } else { hi[k] })
        })));
        for k in 0..3 {
            low[k] = low[k].min(p[k]);
            high[k] = high[k].max(p[k]);
        }
    }
    let bounds_lo = [0, a.layers[0] as i32, 0];
    let bounds_hi = [
        a.body.vol.nx as i32,
        a.layers[1] as i32,
        a.body.vol.nz as i32,
    ];
    (
        std::array::from_fn(|k| low[k].to_int_trunc().clamp(bounds_lo[k], bounds_hi[k])),
        std::array::from_fn(|k| (high[k].to_int_trunc() + 1).clamp(bounds_lo[k], bounds_hi[k])),
    )
}
fn centre(a: &Actor, p: [i32; 3]) -> V3 {
    a.frame
        .world(p.map(|v| Fx::from_int(v).add(Fx::from_ratio(1, 2))))
}
fn coarse(p: V3, origin: [i32; 3], cell: i32) -> [i32; 3] {
    std::array::from_fn(|k| {
        arr(p)[k]
            .sub(Fx::from_int(origin[k]))
            .div(Fx::from_int(cell))
            .to_int_trunc()
    })
}
fn role(a: &Actor, p: [i32; 3]) -> usize {
    a.body.part_at(p[0] as usize, p[1] as usize, p[2] as usize) as usize
}
fn shape_hash(b: &UnitBody) -> u64 {
    b.vol
        .cols
        .iter()
        .fold(0u64, |h, v| h.wrapping_mul(0x100000001b3).wrapping_add(*v))
}

impl Field {
    pub fn capture(
        bodies: &Bodies,
        frames: &[Option<[Frame; 2]>],
        origin: [i32; 3],
        edge: i32,
    ) -> Self {
        let mut out = Self::default();
        let high = origin.map(|v| v + edge);
        for (id, frame) in frames.iter().enumerate() {
            let (Some(frames), Some(state)) = (frame, bodies.state(id as u32)) else {
                continue;
            };
            let Some(body) = bodies.shared_body(id as u32) else {
                continue;
            };
            let layers = if body.turret_y == 0 {
                [[0, body.vol.ny], [0, 0]]
            } else {
                [[0, body.turret_y], [body.turret_y, body.vol.ny]]
            };
            let hash = shape_hash(&body);
            for (frame, layers) in frames.iter().copied().zip(layers) {
                if layers[0] == layers[1] {
                    continue;
                }
                let mut a = Actor {
                    id: id as u32,
                    generation: state.generation,
                    template: state.template,
                    body: Arc::clone(&body),
                    frame,
                    layers,
                    lo: [0; 3],
                    hi: [0; 3],
                    shape_hash: hash,
                };
                (a.lo, a.hi) = local_box(&a, origin, high);
                if volume(a.lo, a.hi) > 0 {
                    out.actors.push(a);
                }
            }
        }
        out
    }
    pub fn empty(&self) -> bool {
        self.actors.is_empty()
    }
    pub fn finished(&self) -> bool {
        self.build_actor == self.actors.len()
    }
    // Bounded field phases borrow their grid, tuning and shared work budget separately.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        origin: [i32; 3],
        cell: i32,
        n: usize,
        resistance: &mut [u32],
        work: &[u32; 5],
        resistance_ppm: u32,
        budget: &mut u32,
    ) {
        let source = index([n as i32 / 2; 3], n);
        while *budget > 0 && self.build_actor < self.actors.len() {
            *budget -= 1;
            if let Some(probe) = self.probe.as_mut() {
                let p = point(probe.cursor, probe.lo, probe.hi);
                let i = index(p, n);
                // The deposition cell injects the pulse; it does not absorb its
                // own outward branch. Its material still receives damage below.
                if i != source {
                    resistance[i] = resistance[i].max(probe.resistance);
                }
                probe.cursor += 1;
                if probe.cursor == volume(probe.lo, probe.hi) {
                    self.probe = None;
                }
                continue;
            }
            let a = &self.actors[self.build_actor];
            if self.build_cell == volume(a.lo, a.hi) {
                self.build_actor += 1;
                self.build_cell = 0;
                continue;
            }
            let p = point(self.build_cell, a.lo, a.hi);
            self.build_cell += 1;
            if !a.body.vol.get(p[0] as usize, p[1] as usize, p[2] as usize) {
                continue;
            }
            let mut lo = [n as i32; 3];
            let mut hi = [-1; 3];
            for bits in 0..8 {
                let q = a.frame.world(std::array::from_fn(|k| {
                    Fx::from_int(p[k] + i32::from(bits & (1 << k) != 0))
                }));
                let q = coarse(q, origin, cell);
                for k in 0..3 {
                    lo[k] = lo[k].min(q[k]);
                    hi[k] = hi[k].max(q[k]);
                }
            }
            let lo = lo.map(|v| v.clamp(0, n as i32));
            let hi = hi.map(|v| (v + 1).clamp(0, n as i32));
            if volume(lo, hi) == 0 {
                continue;
            }
            let r =
                (work[role(a, p)] as u64 * resistance_ppm as u64 / 1_000_000).min(1_000_000) as u32;
            self.probe = Some(Probe {
                lo,
                hi,
                cursor: 0,
                resistance: r.max(1),
            });
        }
    }
    /// Finish all local voxel candidates for one settled wave cell. Returning
    /// false means a work/queue cap: the same hit resumes without double damage.
    // Bounded field phases borrow their grid, tuning and shared work budget separately.
    #[allow(clippy::too_many_arguments)]
    pub fn hit(
        &mut self,
        hit: phys::blast::Hit,
        origin: [i32; 3],
        cell: i32,
        n: usize,
        at: [i32; 3],
        energy: &mut u64,
        bodies: &mut Bodies,
        impact: &modding::tuning::Impact,
        resistance_ppm: u32,
        budget: &mut u32,
    ) -> bool {
        let low = std::array::from_fn(|k| origin[k] + hit.cell[k] * cell);
        let high = low.map(|v| v + cell);
        let mut cached_actor = usize::MAX;
        let mut bounds = ([0; 3], [0; 3]);
        while self.hit_actor < self.actors.len() {
            if *budget == 0 || bodies.pending.len() >= impact.max_pending as usize {
                return false;
            }
            *budget -= 1;
            let a = &self.actors[self.hit_actor];
            if cached_actor != self.hit_actor {
                bounds = local_box(a, low, high);
                cached_actor = self.hit_actor;
            }
            let (lo, hi) = bounds;
            if self.hit_cell == volume(lo, hi) {
                self.hit_actor += 1;
                self.hit_cell = 0;
                continue;
            }
            let p = point(self.hit_cell, lo, hi);
            self.hit_cell += 1;
            if *energy == 0 || !a.body.vol.get(p[0] as usize, p[1] as usize, p[2] as usize) {
                continue;
            }
            let world = centre(a, p);
            if coarse(world, origin, cell) != hit.cell {
                continue;
            }
            let r = (impact.work_kj_m3[role(a, p)] as u64 * resistance_ppm as u64 / 1_000_000)
                .min(1_000_000) as u32;
            if hit.pressure <= r {
                continue;
            }
            let work = impact.work_kj_m3[role(a, p)] as u64 * (a.body.mm as u64).pow(3) / 1000;
            let total = (work.max(1) * 100 / (100 - impact.motion_pct) as u64).min(*energy);
            let motion = total * impact.motion_pct as u64 / 100;
            let fracture = total - motion;
            let direction = vec(low.map(|v| Fx::from_int(v).add(Fx::from_ratio(cell, 2))))
                .sub(vec(at.map(Fx::from_int)));
            let accepted = bodies.queue_cells(
                a.id,
                a.generation,
                vec![p.map(|v| v as u8)],
                fracture,
                motion,
                direction,
                impact.max_pending,
            );
            debug_assert!(accepted);
            *energy -= total;
        }
        self.hit_actor = 0;
        self.hit_cell = 0;
        debug_assert!(hit.cell.iter().all(|v| *v >= 0 && *v < n as i32));
        true
    }
    pub fn valid_domain(
        &self,
        origin: [i32; 3],
        cell: i32,
        n: usize,
        hit: Option<phys::blast::Hit>,
    ) -> bool {
        let high = origin.map(|v| v + cell * n as i32);
        for a in &self.actors {
            if a.frame.scale != Fx::from_ratio(voxel::unit::WORLD_MM as i32, a.body.mm as i32)
                || (a.lo, a.hi) != local_box(a, origin, high)
            {
                return false;
            }
        }
        if self.hit_actor == self.actors.len() {
            return self.hit_cell == 0;
        }
        let Some(hit) = hit else {
            return self.hit_actor == 0 && self.hit_cell == 0;
        };
        let lo = std::array::from_fn(|k| origin[k] + hit.cell[k] * cell);
        let hi = lo.map(|v| v + cell);
        let (lo, hi) = local_box(&self.actors[self.hit_actor], lo, hi);
        self.hit_cell <= volume(lo, hi)
    }
    pub fn hash(&self) -> u64 {
        let mut b = Blob::new();
        for a in &self.actors {
            b.u32(a.id);
            b.u32(a.generation);
            b.u16(a.template);
            b.u64(a.shape_hash);
            save_frame(&mut b, a.frame);
            for v in a.layers {
                b.u8(v);
            }
        }
        for v in [
            self.build_actor,
            self.build_cell,
            self.hit_actor,
            self.hit_cell,
        ] {
            b.usize(v);
        }
        b.bool(self.probe.is_some());
        if let Some(p) = &self.probe {
            for v in p.lo.into_iter().chain(p.hi) {
                b.i32(v);
            }
            b.usize(p.cursor);
            b.u32(p.resistance);
        }
        b.v.iter().fold(0u64, |h, v| {
            h.wrapping_mul(0x100000001b3).wrapping_add(*v as u64)
        })
    }
    pub fn save(&self, b: &mut Blob) {
        b.usize(self.actors.len());
        for a in &self.actors {
            b.u32(a.id);
            b.u32(a.generation);
            b.u16(a.template);
            save_frame(b, a.frame);
            for v in a.layers {
                b.u8(v);
            }
            for v in a.lo.into_iter().chain(a.hi) {
                b.i32(v);
            }
            b.u64s(&a.body.vol.cols);
        }
        for v in [
            self.build_actor,
            self.build_cell,
            self.hit_actor,
            self.hit_cell,
        ] {
            b.usize(v);
        }
        b.bool(self.probe.is_some());
        if let Some(p) = &self.probe {
            for v in p.lo.into_iter().chain(p.hi) {
                b.i32(v);
            }
            b.usize(p.cursor);
            b.u32(p.resistance);
        }
    }
    pub fn load(s: &mut Scan, bodies: Option<&Bodies>, units: usize, n: usize) -> Option<Self> {
        let count = s.usize();
        if count > units * 2 {
            return None;
        }
        let mut actors = Vec::new();
        for _ in 0..count {
            let id = s.u32();
            let generation = s.u32();
            let template = s.u16();
            let frame = load_frame(s)?;
            let layers = [s.u8(), s.u8()];
            let lo = std::array::from_fn(|_| s.i32());
            let hi = std::array::from_fn(|_| s.i32());
            let source = bodies?.templates.get(template as usize)?;
            let words = s.u64s();
            if id as usize >= units
                || words.len() != source.vol.cols.len()
                || layers[0] >= layers[1]
                || layers[1] > source.vol.ny
            {
                return None;
            }
            let dims = [
                source.vol.nx as i32,
                source.vol.ny as i32,
                source.vol.nz as i32,
            ];
            if (0..3).any(|k| lo[k] < 0 || hi[k] > dims[k] || hi[k] <= lo[k]) {
                return None;
            }
            let mut body = Arc::clone(source);
            for (wi, (&original, &now)) in source.vol.cols.iter().zip(&words).enumerate() {
                if now & !original != 0 {
                    return None;
                }
                let mut removed = original & !now;
                while removed != 0 {
                    let x =
                        (wi % source.vol.words as usize) * 64 + removed.trailing_zeros() as usize;
                    let col = wi / source.vol.words as usize;
                    let y = col % source.vol.ny as usize;
                    let z = col / source.vol.ny as usize;
                    Arc::make_mut(&mut body).remove(x, y, z);
                    removed &= removed - 1;
                }
            }
            let shape_hash = shape_hash(&body);
            actors.push(Actor {
                id,
                generation,
                template,
                body,
                frame,
                layers,
                lo,
                hi,
                shape_hash,
            });
        }
        let build_actor = s.usize();
        let build_cell = s.usize();
        let hit_actor = s.usize();
        let hit_cell = s.usize();
        if build_actor > count
            || hit_actor > count
            || build_cell > actors.get(build_actor).map_or(0, |a| volume(a.lo, a.hi))
        {
            return None;
        }
        let probe = if s.bool() {
            let lo = std::array::from_fn(|_| s.i32());
            let hi = std::array::from_fn(|_| s.i32());
            let cursor = s.usize();
            let resistance = s.u32();
            if (0..3).any(|k| lo[k] < 0 || hi[k] > n as i32 || hi[k] <= lo[k])
                || cursor >= volume(lo, hi)
            {
                return None;
            }
            Some(Probe {
                lo,
                hi,
                cursor,
                resistance,
            })
        } else {
            None
        };
        s.ok().then_some(Self {
            actors,
            build_actor,
            build_cell,
            probe,
            hit_actor,
            hit_cell,
        })
    }
}
pub(crate) fn save_frame(b: &mut Blob, f: Frame) {
    for v in [f.base, f.x, f.y, f.z] {
        for x in arr(v) {
            b.i64(x.raw());
        }
    }
    b.i64(f.scale.raw());
    for x in f.centre {
        b.i64(x.raw());
    }
}
pub(crate) fn load_frame(s: &mut Scan) -> Option<Frame> {
    let mut v = || vec(std::array::from_fn(|_| Fx(s.i64())));
    let (base, x, y, z) = (v(), v(), v(), v());
    let scale = Fx(s.i64());
    let centre = std::array::from_fn(|_| Fx(s.i64()));
    let axes = [x, y, z];
    let eps = Fx::from_ratio(1, 100);
    if scale <= Fx::ZERO
        || scale > Fx::from_int(voxel::unit::WORLD_MM as i32)
        || axes.iter().any(|v| {
            arr(*v).iter().any(|c| c.abs() > Fx::ONE.add(eps))
                || v.len_sq().sub(Fx::ONE).abs() > eps
        })
        || x.dot(y).abs() > eps
        || y.dot(z).abs() > eps
        || z.dot(x).abs() > eps
        || x.cross(y).dot(z).sub(Fx::ONE).abs() > eps
    {
        return None;
    }
    Some(Frame {
        base,
        x,
        y,
        z,
        scale,
        centre,
    })
}
