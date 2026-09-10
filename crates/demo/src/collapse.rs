//! Каскад отделения: правка → ступень A → ступень B → тело.
//!
//! Три фильтра ADR-055, из которых здесь два. **Ступени C нет**, и это
//! записано, а не умолчано: отвал, не поместившийся в чанк 32³, даёт
//! `Overflow` и остаётся в мире. Настоящий каскад спросил бы граф
//! компонент чанков; пока такой кусок просто не падает.
//!
//! Ступень A — битовый фильтр «эта правка заведомо ничего не отделит»,
//! пропускает единицы процентов (ADR-090). Ступень B — двухфронтовой
//! флуд со строгим чередованием: обход ограничен удвоенной **меньшей**
//! стороной, а не размером чанка.

use voxel::bodies::Bodies;
use voxel::topo::stage_a::Adjacency;
use voxel::topo::stage_b::{probe, FloodBufs, Verdict};
use voxel::topo::Chunk as TopoChunk;
use voxel::{Edit, World};

/// Потолок обхода ступени B. Больше — дороже; ADR-055 берёт его так,
/// чтобы худший случай оставался ограниченным.
const CAP: u32 = 4096;

#[derive(Clone, Copy, Default, Debug)]
pub struct CollapseStats {
    /// Правок, дошедших до ступени B.
    pub probed: usize,
    /// Из них признано отделением.
    pub detached: usize,
    /// Пробило потолок — ступень C сказала бы больше.
    pub overflow: usize,
    /// Вокселей ушло в тела.
    pub cells: usize,
}

pub struct CollapseLayer {
    adj: Adjacency,
    flood: FloodBufs,
    chunks: Vec<Option<Box<TopoChunk>>>,
    nx: u32,
    ny: u32,
    nz: u32,
    dirty: Vec<u32>,
    stamp: Vec<u32>,
    epoch: u32,
    local: Vec<Vec<(u8, u8, u8)>>,
    /// Буфер состава куска в мировых координатах.
    cells: Vec<(u32, u32, u32)>,
}

impl CollapseLayer {
    pub fn new(w: &World) -> CollapseLayer {
        let cs = voxel::topo::CS as u32;
        let (nx, ny, nz) = (w.nx.div_ceil(cs), w.ny.div_ceil(cs), w.nz.div_ceil(cs));
        let n = (nx * ny * nz) as usize;
        CollapseLayer {
            adj: Adjacency::new(),
            flood: FloodBufs::new(),
            chunks: (0..n).map(|_| None).collect(),
            nx,
            ny,
            nz,
            dirty: Vec::new(),
            stamp: vec![u32::MAX; n],
            epoch: 0,
            local: (0..n).map(|_| Vec::new()).collect(),
            cells: Vec::new(),
        }
    }

    fn build(w: &World, cx: u32, cy: u32, cz: u32) -> Box<TopoChunk> {
        let cs = voxel::topo::CS as u32;
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

    /// Прогнать правки тика через каскад и оторвать всё, что отвалилось.
    pub fn apply(&mut self, w: &mut World, edits: &[Edit], bodies: &mut Bodies) -> CollapseStats {
        let cs = voxel::topo::CS as u32;
        let mut st = CollapseStats::default();
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
            // Чанк вынимается из массива на время обработки: внутри
            // цикла он и читается, и меняется, а держать одновременно
            // общую и исключительную ссылку нельзя.
            let mut c = self.chunks[i].take().expect("chunk present");
            // Порядок правок внутри чанка задан дренажем очереди (FIFO),
            // поэтому и порядок отрывов детерминирован.
            for e in 0..self.local[i].len() {
                let (lx, ly, lz) = self.local[i][e];
                let m = self.adj.gather(&c, lx as usize, ly as usize, lz as usize);
                if self.adj.cannot_split(m) {
                    continue;
                }
                st.probed += 1;
                let (v, _) = probe(&c, &mut self.flood, lx as usize, ly as usize, lz as usize, CAP);
                match v {
                    Verdict::Connected => {}
                    Verdict::Overflow => st.overflow += 1,
                    Verdict::Detached { cells } => {
                        if cells == 0 {
                            continue;
                        }
                        self.cells.clear();
                        for &ci in self.flood.detached_cells() {
                            let (px, py, pz) = voxel::topo::stage_b::unpack_index(ci);
                            self.cells.push((
                                cx * cs + px as u32,
                                cy * cs + py as u32,
                                cz * cs + pz as u32,
                            ));
                        }
                        if bodies.detach(w, &self.cells) {
                            st.detached += 1;
                            st.cells += self.cells.len();
                            // Чанк изменился под нами: воксели куска ушли.
                            // Обновляем сразу, иначе следующая правка того
                            // же тика решала бы задачу по устаревшему
                            // состоянию.
                            for &(px, py, pz) in &self.cells {
                                c.set(
                                    (px - cx * cs) as usize,
                                    (py - cy * cs) as usize,
                                    (pz - cz * cs) as usize,
                                    false,
                                );
                            }
                        }
                    }
                }
            }
            self.chunks[i] = Some(c);
        }
        st
    }

    /// Пометить чанк как требующий пересборки: воксели тела вернулись
    /// в мир не правкой копки, а запеканием.
    pub fn invalidate_at(&mut self, x: u32, y: u32, z: u32) {
        let cs = voxel::topo::CS as u32;
        let (cx, cy, cz) = (x / cs, y / cs, z / cs);
        if cx >= self.nx || cy >= self.ny || cz >= self.nz {
            return;
        }
        let i = ((cz * self.ny + cy) * self.nx + cx) as usize;
        self.chunks[i] = None;
    }
}
