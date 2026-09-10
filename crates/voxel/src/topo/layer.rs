//! Мост от мира к подсистеме: грязное множество и ленивое состояние.
//!
//! Размеры чанков у подсистем **не совпадают, и это не недосмотр**:
//! мешинг режет мир на 62³ с паддингом соседей, связность — на 32³,
//! навигация — на клетки по два воксела. Это разные подсистемы
//! с разными единицами, и каждая считает своё грязное множество.
//!
//! **Слой ленивый.** Нетронутый чанк не стоит ничего, ровно как
//! в самом мире.

use crate::topo::stage_a::Adjacency;
use crate::topo::Chunk as TopoChunk;
use crate::{Edit, World};

/// Слой связности: ступень A на правках тика.
///
/// Ступень A — дешёвый фильтр «эта правка заведомо не может ничего
/// отделить». Пропускает единицы процентов правок (ADR-090), и только
/// они идут в дорогую ступень B.
///
/// **Чанки держатся и обновляются правкой, а не пересобираются.**
/// Первая редакция собирала чанк 32³ из мира на каждый грязный чанк —
/// 32 768 чтений на чанк, около полутора миллионов на тик, и связность
/// стоила вдвое дороже, чем в стенде. Это ровно то, что запрещает
/// ADR-061: пересборка объёма обходом перекрывает ту работу, которую
/// обслуживает.
pub struct TopoLayer {
    adj: Adjacency,
    chunks: Vec<Option<Box<TopoChunk>>>,
    nx: u32,
    ny: u32,
    nz: u32,
    dirty: Vec<u32>,
    stamp: Vec<u32>,
    epoch: u32,
    local: Vec<Vec<(u8, u8, u8)>>,
}

impl TopoLayer {
    pub fn new(w: &World) -> TopoLayer {
        let cs = crate::topo::CS as u32;
        let (nx, ny, nz) = (w.nx.div_ceil(cs), w.ny.div_ceil(cs), w.nz.div_ceil(cs));
        let n = (nx * ny * nz) as usize;
        TopoLayer {
            adj: Adjacency::new(),
            chunks: (0..n).map(|_| None).collect(),
            nx,
            ny,
            nz,
            dirty: Vec::new(),
            stamp: vec![u32::MAX; n],
            epoch: 0,
            local: (0..n).map(|_| Vec::new()).collect(),
        }
    }

    fn build(w: &World, cx: u32, cy: u32, cz: u32) -> Box<TopoChunk> {
        let cs = crate::topo::CS as u32;
        let (ox, oy, oz) = (cx * cs, cy * cs, cz * cs);
        let mut c = Box::new(TopoChunk::new());
        for z in 0..cs {
            for y in 0..cs {
                for x in 0..cs {
                    if w.solid(ox + x, oy + y, oz + z) {
                        c.set(x as usize, y as usize, z as usize, true);
                    }
                }
            }
        }
        c
    }

    /// Прогнать правки тика через ступень A.
    /// Возвращает (грязных чанков, выживших правок).
    pub fn apply(&mut self, w: &World, edits: &[Edit]) -> (usize, usize) {
        let cs = crate::topo::CS as u32;
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == u32::MAX {
            self.stamp.iter_mut().for_each(|v| *v = 0);
            self.epoch = 1;
        }
        self.dirty.clear();
        for e in edits {
            let (cx, cy, cz) = (e.x / cs, e.y / cs, e.z / cs);
            if cx >= self.nx || cy >= self.ny || cz >= self.nz {
                continue;
            }
            let i = ((cz * self.ny + cy) * self.nx + cx) as usize;
            if self.stamp[i] != self.epoch {
                self.stamp[i] = self.epoch;
                self.local[i].clear();
                self.dirty.push(i as u32);
            }
            self.local[i].push(((e.x % cs) as u8, (e.y % cs) as u8, (e.z % cs) as u8));
        }

        let mut survived = 0;
        for k in 0..self.dirty.len() {
            let i = self.dirty[k] as usize;
            let (cx, cy, cz) = (
                (i as u32) % self.nx,
                ((i as u32) / self.nx) % self.ny,
                (i as u32) / (self.nx * self.ny),
            );
            if self.chunks[i].is_none() {
                self.chunks[i] = Some(Self::build(w, cx, cy, cz));
            } else {
                let c = self.chunks[i].as_mut().expect("chunk checked");
                for &(lx, ly, lz) in &self.local[i] {
                    let solid =
                        w.solid(cx * cs + lx as u32, cy * cs + ly as u32, cz * cs + lz as u32);
                    c.set(lx as usize, ly as usize, lz as usize, solid);
                }
            }
            let c = self.chunks[i].as_ref().expect("chunk present");
            // filter возвращает (выжившие, всего), а не наоборот.
            let (s, _) = crate::topo::stage_a::filter(&self.adj, c, &self.local[i]);
            survived += s;
        }
        (self.dirty.len(), survived)
    }

    pub fn live_chunks(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_some()).count()
    }
}

