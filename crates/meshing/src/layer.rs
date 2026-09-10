//! Мост от мира к подсистеме: грязное множество и ленивое состояние.
//!
//! Размеры чанков у подсистем **не совпадают, и это не недосмотр**:
//! мешинг режет мир на 62³ с паддингом соседей, связность — на 32³,
//! навигация — на клетки по два воксела. Это разные подсистемы
//! с разными единицами, и каждая считает своё грязное множество.
//!
//! **Слой ленивый.** Нетронутый чанк не стоит ничего, ровно как
//! в самом мире.

use crate::bits::BitVolume;
use crate::incr::IncMesher;
use crate::{Opts, CS as MCS, CSP};
use voxel::{Edit, World};


/// Слой мешинга: битовый объём чанка плюс его инкрементальный мешер.
struct MeshChunk {
    bits: BitVolume,
    mesher: IncMesher,
}

pub struct MeshLayer {
    chunks: Vec<Option<Box<MeshChunk>>>,
    nx: u32,
    ny: u32,
    nz: u32,
    opts: Opts,
    /// Грязные чанки текущего тика; переиспользуется, чтобы не выделять.
    dirty: Vec<u32>,
    stamp: Vec<u32>,
    epoch: u32,
    /// Правки, сгруппированные по чанку, в локальных координатах.
    local: Vec<Vec<(u8, u8, u8)>>,
    /// Сколько раз чанк собирался с нуля. Первое касание чанка стоит
    /// полной пересборки, и на старте это даёт пик — см. D-7.
    pub full_rebuilds: u64,
}

impl MeshLayer {
    pub fn new(w: &World, opts: Opts) -> MeshLayer {
        let (nx, ny, nz) = (
            w.nx.div_ceil(MCS as u32),
            w.ny.div_ceil(MCS as u32),
            w.nz.div_ceil(MCS as u32),
        );
        let n = (nx * ny * nz) as usize;
        MeshLayer {
            chunks: (0..n).map(|_| None).collect(),
            nx,
            ny,
            nz,
            opts,
            dirty: Vec::new(),
            stamp: vec![u32::MAX; n],
            epoch: 0,
            local: (0..n).map(|_| Vec::new()).collect(),
            full_rebuilds: 0,
        }
    }

    #[inline]
    fn index(&self, cx: u32, cy: u32, cz: u32) -> usize {
        ((cz * self.ny + cy) * self.nx + cx) as usize
    }

    /// Собрать битовый объём чанка из мира, включая паддинг соседей.
    fn build_bits(w: &World, cx: u32, cy: u32, cz: u32) -> BitVolume {
        let mut b: BitVolume = [0u64; CSP * CSP];
        let (ox, oy, oz) = (
            cx * MCS as u32,
            cy * MCS as u32,
            cz * MCS as u32,
        );
        // Локальные 0 и 63 — паддинг: воксели соседнего чанка.
        for ly in 0..CSP {
            for lz in 0..CSP {
                let (wy, wz) = (
                    oy as i64 + ly as i64 - 1,
                    oz as i64 + lz as i64 - 1,
                );
                if wy < 0 || wz < 0 {
                    continue;
                }
                let mut word = 0u64;
                for lx in 0..CSP {
                    let wx = ox as i64 + lx as i64 - 1;
                    if wx < 0 {
                        continue;
                    }
                    if w.solid(wx as u32, wy as u32, wz as u32) {
                        word |= 1u64 << lx;
                    }
                }
                b[ly * CSP + lz] = word;
            }
        }
        b
    }

    /// Развести правки тика по чанкам мешинга. Правка на границе чанка
    /// попадает **и в соседний**: там она меняет паддинг, а значит
    /// и швы. Пропустить это — получить лишние квады на границах,
    /// и заметно это не сразу.
    fn scatter(&mut self, edits: &[Edit]) {
        self.epoch += 1;
        self.dirty.clear();
        for e in edits {
            let (cx, cy, cz) = (
                e.x / MCS as u32,
                e.y / MCS as u32,
                e.z / MCS as u32,
            );
            let (lx, ly, lz) = (
                e.x % MCS as u32 + 1,
                e.y % MCS as u32 + 1,
                e.z % MCS as u32 + 1,
            );
            self.touch(cx, cy, cz, lx as u8, ly as u8, lz as u8);
            // Соседи по каждой оси, если правка легла на первый
            // или последний полезный воксель.
            if lx == 1 && cx > 0 {
                self.touch(cx - 1, cy, cz, (MCS + 1) as u8, ly as u8, lz as u8);
            }
            if lx == MCS as u32 && cx + 1 < self.nx {
                self.touch(cx + 1, cy, cz, 0, ly as u8, lz as u8);
            }
            if ly == 1 && cy > 0 {
                self.touch(cx, cy - 1, cz, lx as u8, (MCS + 1) as u8, lz as u8);
            }
            if ly == MCS as u32 && cy + 1 < self.ny {
                self.touch(cx, cy + 1, cz, lx as u8, 0, lz as u8);
            }
            if lz == 1 && cz > 0 {
                self.touch(cx, cy, cz - 1, lx as u8, ly as u8, (MCS + 1) as u8);
            }
            if lz == MCS as u32 && cz + 1 < self.nz {
                self.touch(cx, cy, cz + 1, lx as u8, ly as u8, 0);
            }
        }
    }

    fn touch(&mut self, cx: u32, cy: u32, cz: u32, lx: u8, ly: u8, lz: u8) {
        if cx >= self.nx || cy >= self.ny || cz >= self.nz {
            return;
        }
        let i = self.index(cx, cy, cz);
        if self.stamp[i] != self.epoch {
            self.stamp[i] = self.epoch;
            self.local[i].clear();
            self.dirty.push(i as u32);
        }
        self.local[i].push((lx, ly, lz));
    }

    /// Применить правки тика. Возвращает число грязных чанков.
    pub fn apply(&mut self, w: &World, edits: &[Edit]) -> usize {
        self.scatter(edits);
        for k in 0..self.dirty.len() {
            let i = self.dirty[k] as usize;
            let (cx, cy, cz) = (
                (i as u32) % self.nx,
                ((i as u32) / self.nx) % self.ny,
                (i as u32) / (self.nx * self.ny),
            );
            if self.chunks[i].is_none() {
                // Первое касание: собрать объём и смешить целиком.
                let bits = Self::build_bits(w, cx, cy, cz);
                let mut mesher = IncMesher::new();
                mesher.rebuild_all(&bits, self.opts);
                self.chunks[i] = Some(Box::new(MeshChunk { bits, mesher }));
                self.full_rebuilds += 1;
                continue;
            }
            let c = self.chunks[i].as_mut().expect("чанк только что проверен");
            // Битовый объём обновляется правкой, а не пересборкой (ADR-061).
            for &(lx, ly, lz) in &self.local[i] {
                let solid = w.solid(
                    cx * MCS as u32 + lx as u32 - 1,
                    cy * MCS as u32 + ly as u32 - 1,
                    cz * MCS as u32 + lz as u32 - 1,
                );
                let word = &mut c.bits[ly as usize * CSP + lz as usize];
                if solid {
                    *word |= 1u64 << lx;
                } else {
                    *word &= !(1u64 << lx);
                }
            }
            let bits = c.bits;
            c.mesher.apply_edits(&bits, &self.local[i], self.opts);
        }
        self.dirty.len()
    }

    pub fn quad_total(&self) -> usize {
        self.chunks
            .iter()
            .filter_map(|c| c.as_ref())
            .map(|c| c.mesher.quad_count())
            .sum()
    }

    pub fn live_chunks(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_some()).count()
    }

    pub fn state_bytes(&self) -> usize {
        self.chunks
            .iter()
            .filter_map(|c| c.as_ref())
            .map(|c| c.mesher.state_bytes() + core::mem::size_of::<BitVolume>())
            .sum()
    }

    /// Пересобрать чанк с нуля — эталон для проверки швов.
    pub fn rebuilt_quad_count(&self, w: &World, i: usize) -> usize {
        let (cx, cy, cz) = (
            (i as u32) % self.nx,
            ((i as u32) / self.nx) % self.ny,
            (i as u32) / (self.nx * self.ny),
        );
        let bits = Self::build_bits(w, cx, cy, cz);
        let mut m = IncMesher::new();
        m.rebuild_all(&bits, self.opts);
        m.quad_count()
    }

    /// Число квадов конкретного чанка — для сверки швов.
    pub fn quad_at(&self, i: usize) -> usize {
        self.chunks[i].as_ref().map_or(0, |c| c.mesher.quad_count())
    }

    pub fn live_indices(&self) -> Vec<usize> {
        (0..self.chunks.len())
            .filter(|&i| self.chunks[i].is_some())
            .collect()
    }
}

