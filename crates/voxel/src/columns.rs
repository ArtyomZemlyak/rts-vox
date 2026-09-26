//! Immutable column reader for background presentation jobs. Shares edited
//! chunks and the generator, without copying occupancy or simulation queues.
use super::{Generator, World};
use crate::store::{Mat, Paletted, AIR, CS};
use std::{collections::BTreeMap, sync::Arc};

pub struct Columns {
    size: [u32; 3],
    cx: u32,
    cy: u32,
    chunks: BTreeMap<usize, Arc<Paletted>>,
    generator: Arc<dyn Generator + Send + Sync>,
}

impl Columns {
    pub fn capture(world: &World) -> Self {
        Self {
            size: [world.nx, world.ny, world.nz],
            cx: world.cx,
            cy: world.cy,
            chunks: world
                .chunks
                .iter()
                .enumerate()
                .filter_map(|(i, c)| c.as_ref().map(|c| (i, Arc::clone(c))))
                .collect(),
            generator: Arc::clone(&world.gen),
        }
    }

    pub fn column(&self, x: u32, z: u32, out: &mut [Mat]) {
        out.fill(AIR);
        if x >= self.size[0] || z >= self.size[2] {
            return;
        }
        let n = out.len().min(self.size[1] as usize);
        self.generator.column(x, z, 0, &mut out[..n]);
        for y in (0..n).step_by(CS) {
            let i = ((z / CS as u32 * self.cy + y as u32 / CS as u32) * self.cx + x / CS as u32)
                as usize;
            if let Some(chunk) = self.chunks.get(&i) {
                for (dy, dst) in out[y..(y + CS).min(n)].iter_mut().enumerate() {
                    *dst = chunk.get(x as usize % CS, dy, z as usize % CS);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Ground;
    impl Generator for Ground {
        fn material(&self, _: u32, y: u32, _: u32) -> Mat {
            if y < 12 {
                7
            } else {
                AIR
            }
        }
    }
    #[test]
    fn background_columns_retain_edits_at_capture_time() {
        let mut world = World::new(64, 64, 64, 10000, Box::new(Ground));
        world.queue_edit(super::super::Edit::put(20, 40, 20, 3));
        world.flush();
        let columns = Columns::capture(&world);
        world.queue_edit(super::super::Edit::dig(20, 40, 20));
        world.flush();
        let mut old = [0; 64];
        columns.column(20, 20, &mut old);
        assert_eq!(old[40], 3);
        assert_eq!(old[11], 7);
        assert_eq!(world.material(20, 40, 20), 0);
        columns.column(64, 20, &mut old);
        assert!(old.iter().all(|m| *m == AIR));
    }
}
