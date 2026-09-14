//! A bounded immutable region at the moment an event happens.
//! Pristine chunks keep the shared generator; changed chunks use COW storage.
use super::{Generator, World};
use crate::store::{Mat, Paletted, AIR, CS};
use std::sync::Arc;

// Blast domains are at most 65 coarse cells with an edge of four world units.
const MAX_EDGE: i32 = 260;
const MAX_CHUNKS: usize = 10 * 10 * 10;

#[derive(Clone)]
pub struct Snapshot {
    world_size: [u32; 3],
    origin: [u32; 3],
    dims: [u32; 3],
    chunks: Vec<Option<Arc<Paletted>>>,
    gen: Arc<dyn Generator + Send + Sync>,
}

impl Snapshot {
    /// Bounds are half-open. Captures the containing chunks without visiting
    /// their voxels; out-of-world samples remain air.
    pub fn capture(w: &World, lo: [i32; 3], hi: [i32; 3]) -> Result<Self, &'static str> {
        if (0..3).any(|a| hi[a] < lo[a] || hi[a] as i64 - lo[a] as i64 > MAX_EDGE as i64) {
            return Err("snapshot domain exceeds the bounded blast region");
        }
        let world_size = [w.nx, w.ny, w.nz];
        let low: [u32; 3] = std::array::from_fn(|a| lo[a].clamp(0, world_size[a] as i32) as u32);
        let high: [u32; 3] = std::array::from_fn(|a| hi[a].clamp(0, world_size[a] as i32) as u32);
        let origin = low.map(|v| v / CS as u32);
        let dims: [u32; 3] = std::array::from_fn(|a| {
            if high[a] <= low[a] {
                0
            } else {
                (high[a] - 1) / CS as u32 + 1 - origin[a]
            }
        });
        let mut chunks = Vec::with_capacity(dims.iter().map(|v| *v as usize).product());
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let ci =
                        (((origin[2] + z) * w.cy + origin[1] + y) * w.cx + origin[0] + x) as usize;
                    chunks.push(w.chunks[ci].clone());
                }
            }
        }
        Ok(Self {
            world_size,
            origin,
            dims,
            chunks,
            gen: Arc::clone(&w.gen),
        })
    }
    pub fn material(&self, p: [i32; 3]) -> Mat {
        if (0..3).any(|a| p[a] < 0 || p[a] as u32 >= self.world_size[a]) {
            return AIR;
        }
        let p = p.map(|v| v as u32);
        let c = p.map(|v| v / CS as u32);
        if (0..3).any(|a| c[a] < self.origin[a] || c[a] - self.origin[a] >= self.dims[a]) {
            return AIR;
        }
        let q: [u32; 3] = std::array::from_fn(|a| c[a] - self.origin[a]);
        let i = (q[0] + self.dims[0] * (q[1] + self.dims[1] * q[2])) as usize;
        match &self.chunks[i] {
            Some(chunk) => chunk.get(p[0] as usize % CS, p[1] as usize % CS, p[2] as usize % CS),
            None => self.gen.material(p[0], p[1], p[2]),
        }
    }
    pub fn hash(&self) -> u64 {
        let mut h = 0u64;
        for v in self
            .world_size
            .into_iter()
            .chain(self.origin)
            .chain(self.dims)
        {
            h = h.wrapping_mul(31).wrapping_add(v as u64);
        }
        for chunk in &self.chunks {
            h = h.wrapping_mul(0x100000001b3).wrapping_add(
                chunk
                    .as_ref()
                    .map_or(0, |p| p.content_hash().wrapping_add(1)),
            );
        }
        h
    }
    pub fn save(&self, b: &mut fx::blob::Blob) {
        for v in self
            .world_size
            .into_iter()
            .chain(self.origin)
            .chain(self.dims)
        {
            b.u32(v);
        }
        for chunk in &self.chunks {
            b.bool(chunk.is_some());
            let Some(chunk) = chunk else {
                continue;
            };
            let mut runs: Vec<(Mat, u16)> = Vec::new();
            for y in 0..CS {
                for z in 0..CS {
                    for x in 0..CS {
                        let m = chunk.get(x, y, z);
                        match runs.last_mut() {
                            Some((last, n)) if *last == m => *n += 1,
                            _ => runs.push((m, 1)),
                        }
                    }
                }
            }
            b.u32(runs.len() as u32);
            for (m, n) in runs {
                b.u16(m);
                b.u16(n);
            }
        }
    }
    /// The generator comes from the restored world, whose seed/parameters have
    /// already been checked by its caller. Old edited chunks come from the file.
    pub fn load(s: &mut fx::blob::Scan, world: &World) -> Option<Self> {
        let world_size = std::array::from_fn(|_| s.u32());
        let origin: [u32; 3] = std::array::from_fn(|_| s.u32());
        let dims: [u32; 3] = std::array::from_fn(|_| s.u32());
        if world_size != [world.nx, world.ny, world.nz]
            || (0..3).any(|a| {
                dims[a] > 10
                    || origin[a] as u64 + dims[a] as u64 > world_size[a].div_ceil(CS as u32) as u64
            })
        {
            return None;
        }
        let count = dims.iter().map(|v| *v as usize).product::<usize>();
        if count > MAX_CHUNKS {
            return None;
        }
        let mut chunks = Vec::with_capacity(count);
        for _ in 0..count {
            if !s.bool() {
                chunks.push(None);
                continue;
            }
            let n = s.list(4);
            if n > CS * CS * CS {
                return None;
            }
            let mut p = Paletted::new();
            let mut cursor = 0usize;
            for _ in 0..n {
                let mat = s.u16();
                let run = s.u16() as usize;
                if run == 0 || cursor + run > CS * CS * CS {
                    return None;
                }
                if mat != AIR {
                    for i in cursor..cursor + run {
                        p.set(i % CS, i / (CS * CS), i / CS % CS, mat);
                    }
                }
                cursor += run;
            }
            if cursor != CS * CS * CS {
                return None;
            }
            chunks.push(Some(Arc::new(p)));
        }
        s.ok().then(|| Self {
            world_size,
            origin,
            dims,
            chunks,
            gen: Arc::clone(&world.gen),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Slab;
    impl Generator for Slab {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < 12 {
                2
            } else {
                AIR
            }
        }
    }
    fn world() -> World {
        World::new(96, 64, 96, 10000, Box::new(Slab))
    }
    #[test]
    fn edits_after_capture_do_not_change_old_or_pristine_chunks_in_a_snapshot() {
        let mut w = world();
        w.queue_edit(super::super::Edit::put(20, 20, 20, 3));
        w.flush();
        let a = Snapshot::capture(&w, [-10, -10, -10], [80, 60, 80]).unwrap();
        let h = a.hash();
        w.queue_edit(super::super::Edit::dig(20, 20, 20));
        w.queue_edit(super::super::Edit::dig(60, 10, 60));
        w.flush();
        assert_eq!(a.material([20, 20, 20]), 3);
        assert_eq!(a.material([60, 10, 60]), 2);
        assert_eq!(w.material(20, 20, 20), 0);
        assert_eq!(w.material(60, 10, 60), 0);
        assert_eq!(a.hash(), h);
        assert_eq!(a.material([-1, 1, 1]), 0);
        assert_eq!(a.material([96, 1, 1]), 0);
    }
    #[test]
    fn a_saved_snapshot_retains_the_old_wall_when_the_current_world_has_a_hole() {
        let mut w = world();
        w.queue_edit(super::super::Edit::put(20, 20, 20, 3));
        w.flush();
        let a = Snapshot::capture(&w, [10; 3], [30; 3]).unwrap();
        w.queue_edit(super::super::Edit::dig(20, 20, 20));
        w.flush();
        let mut blob = fx::blob::Blob::new();
        a.save(&mut blob);
        let b = Snapshot::load(&mut fx::blob::Scan::new(&blob.v), &w).unwrap();
        assert_eq!(a.hash(), b.hash());
        assert_eq!(b.material([20, 20, 20]), 3);
        for end in [0, 36, blob.v.len() - 1] {
            assert!(Snapshot::load(&mut fx::blob::Scan::new(&blob.v[..end]), &w).is_none());
        }
    }
    #[test]
    fn oversized_capture_is_refused_before_allocating() {
        let w = world();
        assert!(Snapshot::capture(&w, [i32::MIN; 3], [i32::MAX; 3]).is_err());
    }
    #[test]
    fn content_hash_is_independent_of_palette_order_and_detects_a_single_edit() {
        let mut a = Paletted::new();
        let mut b = Paletted::new();
        a.set(1, 1, 1, 2);
        a.set(2, 1, 1, 3);
        b.set(2, 1, 1, 3);
        b.set(1, 1, 1, 2);
        assert_eq!(a.content_hash(), b.content_hash());
        b.set(1, 1, 1, AIR);
        assert_ne!(a.content_hash(), b.content_hash());
        b.set(1, 1, 1, 2);
        assert_eq!(a.content_hash(), b.content_hash());
    }
}
