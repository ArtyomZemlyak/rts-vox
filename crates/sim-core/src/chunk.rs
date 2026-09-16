//! Preserved geometry of a disconnected unit section. Coordinates remain in
//! its source template; the captured two frames freeze the aimed turret pose.
use super::*;
use crate::damage::{Bodies, Frame};
use std::sync::Arc;
use voxel::impact::{Cell, Dents};
use voxel::UnitBody;

pub struct Chunk {
    pub template: u16,
    pub owner: u8,
    /// A whole segment of the template rather than a piece of its root:
    /// cells and shape are in the segment's own volume (ADR-277).
    pub segment: Option<u8>,
    pub cells: Vec<Cell>,
    pub body: Option<Arc<UnitBody>>,
    pub dents: Dents,
    pub frames: [Frame; 2],
    pub centre: V3,
    pub bounds: ([Fx; 3], [Fx; 3]),
    fingerprint: u64,
}
/// Accumulated alongside staged cell removals, not a second full pass at commit.
#[derive(Clone)]
pub struct Shape {
    sum: [[u64; 3]; 2],
    square: [[u64; 3]; 2],
    count: [u32; 2],
    lo: [[u8; 3]; 2],
    hi: [[u8; 3]; 2],
    fingerprint: u64,
}
impl Default for Shape {
    fn default() -> Self {
        Self {
            sum: [[0; 3]; 2],
            square: [[0; 3]; 2],
            count: [0; 2],
            lo: [[255; 3]; 2],
            hi: [[0; 3]; 2],
            fingerprint: 0,
        }
    }
}
impl Shape {
    pub fn from_body(body: &UnitBody) -> Self {
        let mut out = Self::default();
        for z in 0..body.vol.nz {
            for y in 0..body.vol.ny {
                for x in 0..body.vol.nx {
                    if body.vol.get(x as usize, y as usize, z as usize) {
                        out.add([x, y, z], body.turret_y);
                    }
                }
            }
        }
        out
    }
    pub fn count(&self) -> u32 {
        self.count[0] + self.count[1]
    }
    /// Twelve times the voxel-volume inertia about the root attachment pivot.
    /// Cell centres use half-cell coordinates; +2 is each cube's own inertia.
    /// Density and mm^5 cancel when comparing an intact and damaged root.
    pub fn inertia(&self, dims: [u8; 3]) -> [u64; 3] {
        let centre = [dims[0] as u64, 0, dims[2] as u64];
        let mut squares = [0u64; 3];
        for layer in 0..2 {
            for a in 0..3 {
                let n = self.count[layer] as u64;
                let sum = 2 * self.sum[layer][a] + n;
                squares[a] +=
                    self.square[layer][a] + n * centre[a] * centre[a] - 2 * centre[a] * sum;
            }
        }
        std::array::from_fn(|a| {
            3 * (squares[(a + 1) % 3] + squares[(a + 2) % 3]) + 2 * self.count() as u64
        })
    }
    fn cell_hash(c: Cell) -> u64 {
        let mut h =
            0x9e3779b97f4a7c15u64 ^ (c[0] as u64 | (c[1] as u64) << 8 | (c[2] as u64) << 16);
        h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
        h ^ (h >> 31)
    }
    pub fn remove(&mut self, c: Cell, turret_y: u8) {
        let l = usize::from(turret_y > 0 && c[1] >= turret_y);
        self.count[l] -= 1;
        for (a, value) in c.into_iter().enumerate() {
            self.sum[l][a] -= value as u64;
            self.square[l][a] -= (2 * value as u64 + 1).pow(2);
        }
        self.fingerprint ^= Self::cell_hash(c);
        // Bounds remain conservative after holes; count removes an empty layer.
    }
    pub fn add(&mut self, c: Cell, turret_y: u8) {
        let l = usize::from(turret_y > 0 && c[1] >= turret_y);
        self.count[l] += 1;
        self.fingerprint ^= Self::cell_hash(c);
        for (a, value) in c.into_iter().enumerate() {
            self.sum[l][a] += value as u64;
            self.square[l][a] += (2 * value as u64 + 1).pow(2);
            self.lo[l][a] = self.lo[l][a].min(value);
            self.hi[l][a] = self.hi[l][a].max(value);
        }
    }
}
impl Chunk {
    pub fn new(
        template: u16,
        owner: u8,
        cells: Vec<Cell>,
        frames: [Frame; 2],
        shape: Shape,
    ) -> Self {
        let count = shape.count();
        assert!(count > 0);
        let mut centre = V3::ZERO;
        for (l, frame) in frames.iter().enumerate() {
            let n = shape.count[l];
            if n == 0 {
                continue;
            }
            let avg = shape.sum[l].map(|s| {
                Fx((((s * 2 + n as u64) as u128) << 31)
                    .checked_div(n as u128)
                    .unwrap() as i64)
            });
            centre = centre.add(
                frame
                    .world(avg)
                    .scale(Fx::from_ratio(n as i32, count as i32)),
            );
        }
        let mut low = [Fx::MAX; 3];
        let mut high = [Fx::MIN; 3];
        for (l, frame) in frames.iter().enumerate() {
            if shape.count[l] > 0 {
                for mask in 0..8 {
                    let p = frame
                        .world(std::array::from_fn(|a| {
                            Fx::from_int(if mask & (1 << a) == 0 {
                                shape.lo[l][a] as i32
                            } else {
                                shape.hi[l][a] as i32 + 1
                            })
                        }))
                        .sub(centre);
                    for (a, v) in arr(p).into_iter().enumerate() {
                        low[a] = low[a].min(v);
                        high[a] = high[a].max(v);
                    }
                }
            }
        }
        let mut fingerprint = shape.fingerprint;
        mix(&mut fingerprint, template as u64);
        mix(&mut fingerprint, owner as u64);
        mix(&mut fingerprint, count as u64);
        for f in frames {
            for v in [f.base, f.x, f.y, f.z]
                .into_iter()
                .flat_map(arr)
                .chain([f.scale])
                .chain(f.centre)
            {
                mix(&mut fingerprint, v.raw() as u64);
            }
        }
        Self {
            template,
            owner,
            segment: None,
            cells,
            body: None,
            dents: Dents::new(),
            frames,
            centre,
            bounds: (low, high),
            fingerprint,
        }
    }
    pub fn from_body(
        template: u16,
        owner: u8,
        body: Arc<UnitBody>,
        frames: [Frame; 2],
        shape: Shape,
    ) -> Self {
        let mut out = Self::new(template, owner, Vec::new(), frames, shape);
        mix(&mut out.fingerprint, 1);
        out.body = Some(body);
        out
    }
    /// Mark the chunk as segment `seg` of its template.
    pub fn in_segment(mut self, seg: u8) -> Self {
        mix(&mut self.fingerprint, 1000 + u64::from(seg));
        self.segment = Some(seg);
        self
    }
    pub fn with_dents(mut self, dents: Dents) -> Self {
        for (c, energy) in &dents {
            for v in c {
                mix(&mut self.fingerprint, *v as u64);
            }
            mix(&mut self.fingerprint, *energy);
        }
        self.dents = dents;
        self
    }
    pub fn hash(&self) -> u64 {
        self.fingerprint
    }
    pub(super) fn save(&self, b: &mut Blob) {
        b.u16(self.template);
        b.u8(self.owner);
        b.u8(self.segment.unwrap_or(u8::MAX));
        for f in self.frames {
            crate::body_wave::save_frame(b, f);
        }
        b.bool(self.body.is_some());
        if let Some(body) = &self.body {
            b.u64s(&body.vol.cols);
            b.u32(self.dents.len() as u32);
            for (c, e) in &self.dents {
                for v in c {
                    b.u8(*v);
                }
                b.u64(*e);
            }
            return;
        }
        b.u32(self.cells.len() as u32);
        for c in &self.cells {
            for v in c {
                b.u8(*v);
            }
        }
    }
    pub(super) fn load(s: &mut Scan, bodies: Option<&Bodies>, mm: u16, count: u32) -> Option<Self> {
        let template = s.u16();
        let owner = s.u8();
        let segment = s.u8();
        let segment = (segment != u8::MAX).then_some(segment);
        let frames = [
            crate::body_wave::load_frame(s)?,
            crate::body_wave::load_frame(s)?,
        ];
        let root = bodies?.templates.get(template as usize)?;
        // A segment chunk is checked against the segment's own volume,
        // about its pivot; a root chunk about the root's base centre.
        let (body, centre) = match segment {
            None => (
                Arc::clone(root),
                [
                    Fx::from_ratio(root.vol.nx as i32, 2),
                    Fx::ZERO,
                    Fx::from_ratio(root.vol.nz as i32, 2),
                ],
            ),
            Some(i) => {
                let seg = root.segments.get(usize::from(i))?;
                (
                    Arc::new(seg.body.clone()),
                    [
                        Fx::from_int(i32::from(seg.attach.pivot.0)),
                        Fx::from_int(i32::from(seg.attach.pivot.1)),
                        Fx::from_int(i32::from(seg.attach.pivot.2)),
                    ],
                )
            }
        };
        let body = &body;
        if count == 0
            || owner >= 8
            || mm != body.mm
            || frames.iter().any(|f| {
                f.scale != Fx::from_ratio(voxel::unit::WORLD_MM as i32, mm as i32)
                    || f.centre != centre
            })
        {
            return None;
        }
        if s.bool() {
            let cols = s.u64s();
            if cols.len() != body.vol.cols.len()
                || cols.iter().zip(&body.vol.cols).any(|(a, b)| a & !b != 0)
                || cols.iter().map(|v| v.count_ones()).sum::<u32>() != count
            {
                return None;
            }
            let mut geometry = Arc::clone(body);
            let mut shape = Shape::from_body(body);
            for (wi, (&was, &now)) in body.vol.cols.iter().zip(&cols).enumerate() {
                let mut gone = was & !now;
                while gone != 0 {
                    let row = wi / body.vol.words as usize;
                    let c = [
                        ((wi % body.vol.words as usize) * 64 + gone.trailing_zeros() as usize)
                            as u8,
                        (row % body.vol.ny as usize) as u8,
                        (row / body.vol.ny as usize) as u8,
                    ];
                    Arc::make_mut(&mut geometry).remove(
                        c[0] as usize,
                        c[1] as usize,
                        c[2] as usize,
                    );
                    shape.remove(c, body.turret_y);
                    gone &= gone - 1;
                }
            }
            let n = s.list(11);
            if n > count as usize {
                return None;
            }
            let mut dents = Dents::new();
            for _ in 0..n {
                let c = [s.u8(), s.u8(), s.u8()];
                let energy = s.u64();
                if c[0] >= body.vol.nx
                    || c[1] >= body.vol.ny
                    || c[2] >= body.vol.nz
                    || energy == 0
                    || !geometry
                        .vol
                        .get(c[0] as usize, c[1] as usize, c[2] as usize)
                    || dents.insert(c, energy).is_some()
                {
                    return None;
                }
            }
            return s.ok().then(|| {
                let chunk =
                    Self::from_body(template, owner, geometry, frames, shape).with_dents(dents);
                match segment {
                    Some(i) => chunk.in_segment(i),
                    None => chunk,
                }
            });
        }
        let n = s.list(3);
        if n == 0 || n != count as usize || n > body.solid_count() as usize {
            return None;
        }
        let mut seen = body.vol.clone();
        seen.cols.fill(0);
        let mut cells = Vec::with_capacity(n);
        let mut shape = Shape::default();
        for _ in 0..n {
            let c = [s.u8(), s.u8(), s.u8()];
            let [x, y, z] = c.map(|v| v as usize);
            if x >= body.vol.nx as usize
                || y >= body.vol.ny as usize
                || z >= body.vol.nz as usize
                || !body.vol.get(x, y, z)
                || seen.get(x, y, z)
            {
                return None;
            }
            seen.set(x, y, z, true);
            shape.add(c, body.turret_y);
            cells.push(c);
        }
        s.ok().then(|| match segment {
            Some(i) => Self::new(template, owner, cells, frames, shape).in_segment(i),
            None => Self::new(template, owner, cells, frames, shape),
        })
    }
}
