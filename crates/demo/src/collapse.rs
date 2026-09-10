//! Каскад отделения: правки тика → компоненты чанка → тела.
//!
//! **Тик — атом.** Вопрос «что отвалилось» задаётся по состоянию конца
//! тика, а не по каждой правке отдельно. Это не оформительская разница:
//! пока каскад спрашивал по правке, толстый вырез за один тик проходил
//! мимо него целиком (ADR-118). Ступени A и B (ADR-055) отвечают на
//! вопрос «расщепило ли снятие ЭТОГО воксела» и потому годятся только
//! там, где снятие одно; на пачке они дают неверный ответ в обе
//! стороны — либо не находят отвала вовсе, либо отрывают кусок вместе
//! с породой, которую в том же тике выкопали.
//!
//! Ответ на пачку — разметка связных компонент чанка (5–10 мкс на
//! чанк 32³) и проверка заякоренности: компонента держится, если хоть
//! одна её ячейка стоит на грани чанка, за которой твёрдое, либо лежит
//! на коренной породе.
//!
//! **Ступени C по-прежнему нет** (Q-66), и здесь это видно точно:
//! компонента, касающаяся грани, считается заякоренной, хотя то, за
//! что она держится, может само висеть в воздухе. Такие компоненты
//! считаются отдельно — `assumed_anchored`.

use voxel::bodies::Bodies;
use voxel::topo::{Chunk as TopoChunk, Components, CS};
use voxel::{Edit, World};

/// Потолок на размер одного отвала в вокселях. Больше — не тело,
/// а событие другого масштаба, и оно оставляется в мире.
const MAX_BODY_CELLS: usize = 4096;

#[derive(Clone, Copy, Default, Debug)]
pub struct CollapseStats {
    /// Компонент рассмотрено.
    pub probed: usize,
    /// Из них признано отделением.
    pub detached: usize,
    /// Компонент сочтено заякоренными **только** по касанию грани чанка.
    /// Ступень C сказала бы точнее (Q-66).
    pub assumed_anchored: usize,
    /// Компонент отвалилось, но не влезло в потолок тела.
    pub too_big: usize,
    /// Вокселей ушло в тела.
    pub cells: usize,
}

pub struct CollapseLayer {
    comps: Components,
    chunks: Vec<Option<Box<TopoChunk>>>,
    nx: u32,
    ny: u32,
    nz: u32,
    dirty: Vec<u32>,
    stamp: Vec<u32>,
    epoch: u32,
    /// Правки тика по чанкам: местные координаты и твёрдость после.
    local: Vec<Vec<(u8, u8, u8, bool)>>,
    /// Заякоренность на метку компоненты (метка — индекс пробега).
    anchored: Vec<bool>,
    /// Буфер состава куска в мировых координатах.
    cells: Vec<(u32, u32, u32)>,
}

impl CollapseLayer {
    pub fn new(w: &World) -> CollapseLayer {
        let cs = CS as u32;
        let (nx, ny, nz) = (w.nx.div_ceil(cs), w.ny.div_ceil(cs), w.nz.div_ceil(cs));
        let n = (nx * ny * nz) as usize;
        CollapseLayer {
            comps: Components::new(),
            chunks: (0..n).map(|_| None).collect(),
            nx,
            ny,
            nz,
            dirty: Vec::new(),
            stamp: vec![u32::MAX; n],
            epoch: 0,
            local: (0..n).map(|_| Vec::new()).collect(),
            anchored: Vec::new(),
            cells: Vec::new(),
        }
    }

    fn build(w: &World, cx: u32, cy: u32, cz: u32) -> Box<TopoChunk> {
        let cs = CS as u32;
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
        let cs = CS as u32;
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
            self.local[i].push((
                (e.x % cs) as u8,
                (e.y % cs) as u8,
                (e.z % cs) as u8,
                e.mat != voxel::store::AIR,
            ));
        }

        for k in 0..self.dirty.len() {
            let i = self.dirty[k] as usize;
            let (cx, cy, cz) = (
                (i as u32) % self.nx,
                ((i as u32) / self.nx) % self.ny,
                (i as u32) / (self.nx * self.ny),
            );
            // Засыпка материал добавляет, а добавление не расщепляет:
            // чанк, в котором за тик ничего не сняли, можно не трогать.
            if !self.local[i].iter().any(|&(_, _, _, solid)| !solid) {
                continue;
            }
            if self.chunks[i].is_none() {
                self.chunks[i] = Some(Self::build(w, cx, cy, cz));
            } else {
                let c = self.chunks[i].as_mut().expect("chunk checked");
                for &(lx, ly, lz, solid) in &self.local[i] {
                    c.set(lx as usize, ly as usize, lz as usize, solid);
                }
            }
            let mut c = self.chunks[i].take().expect("chunk present");
            st.probed += self.detach_loose(w, &mut c, (cx, cy, cz), bodies, &mut st);
            self.chunks[i] = Some(c);
        }
        st
    }

    /// Разметить компоненты чанка, найти незаякоренные и оторвать их.
    /// Возвращает число рассмотренных компонент.
    fn detach_loose(
        &mut self,
        w: &mut World,
        c: &mut TopoChunk,
        (cx, cy, cz): (u32, u32, u32),
        bodies: &mut Bodies,
        st: &mut CollapseStats,
    ) -> usize {
        let cs = CS as u32;
        let (ox, oy, oz) = (cx * cs, cy * cs, cz * cs);
        let n = self.comps.build(c);
        if n == 0 {
            return 0;
        }
        // Метки компонент перенумерованы 0..n самим `build`, поэтому
        // индексируем ими, а не индексами пробегов.
        self.anchored.clear();
        self.anchored.resize(n, false);
        // Заякоренность проверяется по пробегам, а не по вокселям:
        // граней шесть, и пробег знает, на какой из них он лежит.
        let mut face_only = vec![false; n];
        for (ri, r) in self.comps.runs.iter().enumerate() {
            let lab = self.comps.label[ri] as usize;
            let col = r.col as usize;
            let (ly, lz) = (col % CS, col / CS);
            let (wy, wz) = (oy + ly as u32, oz + lz as u32);
            let m = r.mask;
            let mut bedrock = false;
            let mut face = false;
            if m & 1 != 0 && ox > 0 && w.solid(ox - 1, wy, wz) {
                face = true;
            }
            if m & (1 << 31) != 0 && w.solid(ox + cs, wy, wz) {
                face = true;
            }
            if ly == 0 || ly == CS - 1 || lz == 0 || lz == CS - 1 {
                let mut bits = m;
                while bits != 0 {
                    let x = bits.trailing_zeros();
                    bits &= bits - 1;
                    let wx = ox + x;
                    if ly == 0 {
                        if wy == 0 {
                            bedrock = true;
                        } else if w.solid(wx, wy - 1, wz) {
                            face = true;
                        }
                    }
                    if ly == CS - 1 && w.solid(wx, wy + 1, wz) {
                        face = true;
                    }
                    if lz == 0 && wz > 0 && w.solid(wx, wy, wz - 1) {
                        face = true;
                    }
                    if lz == CS - 1 && w.solid(wx, wy, wz + 1) {
                        face = true;
                    }
                    if bedrock && face {
                        break;
                    }
                }
            }
            if bedrock {
                self.anchored[lab] = true;
                face_only[lab] = false;
            } else if face && !self.anchored[lab] {
                self.anchored[lab] = true;
                face_only[lab] = true;
            }
        }

        // Собрать состав незаякоренных компонент. Порядок меток задан
        // самим `build` — по первому появлению пробега, то есть по
        // колонкам: детерминирован.
        let mut loose: Vec<u32> = Vec::new();
        for lab in 0..n {
            if self.anchored[lab] {
                if face_only[lab] {
                    st.assumed_anchored += 1;
                }
                continue;
            }
            loose.push(lab as u32);
        }

        for lab in loose {
            self.cells.clear();
            for (ri, r) in self.comps.runs.iter().enumerate() {
                if self.comps.label[ri] != lab {
                    continue;
                }
                let col = r.col as usize;
                let (ly, lz) = (col % CS, col / CS);
                let mut bits = r.mask;
                while bits != 0 {
                    let x = bits.trailing_zeros();
                    bits &= bits - 1;
                    self.cells.push((ox + x, oy + ly as u32, oz + lz as u32));
                }
            }
            if self.cells.is_empty() {
                continue;
            }
            if self.cells.len() > MAX_BODY_CELLS {
                st.too_big += 1;
                continue;
            }
            if bodies.detach(w, &self.cells) {
                st.detached += 1;
                st.cells += self.cells.len();
                for &(px, py, pz) in &self.cells {
                    c.set((px - ox) as usize, (py - oy) as usize, (pz - oz) as usize, false);
                }
            }
        }
        n
    }

    /// Пометить чанк как требующий пересборки: воксели тела вернулись
    /// в мир не правкой копки, а запеканием.
    pub fn invalidate_at(&mut self, x: u32, y: u32, z: u32) {
        let cs = CS as u32;
        let (cx, cy, cz) = (x / cs, y / cs, z / cs);
        if cx >= self.nx || cy >= self.ny || cz >= self.nz {
            return;
        }
        let i = ((cz * self.ny + cy) * self.nx + cx) as usize;
        self.chunks[i] = None;
    }
}
