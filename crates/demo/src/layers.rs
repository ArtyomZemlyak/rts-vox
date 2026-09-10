//! Мост от мира к подсистемам, которые живут на своих сетках.
//!
//! Размеры чанков **не совпадают, и это не недосмотр**: мешинг режет
//! мир на 62³ с паддингом соседей, связность — на 32³, навигация —
//! на клетки по два воксела. Это разные подсистемы с разными единицами,
//! и мост обязан считать грязные множества для каждой отдельно.
//!
//! **Слои ленивые.** Состояние инкрементального мешера — 726 КиБ
//! на чанк (ADR-075); держать его на всю карту нельзя, а на тронутые
//! чанки — можно. Нетронутый чанк не стоит ничего, ровно как
//! в самом мире.

use meshing::bits::BitVolume;
use meshing::incr::IncMesher;
use meshing::{Opts, CS as MCS, CSP};
use voxel::topo::stage_a::Adjacency;
use voxel::topo::Chunk as TopoChunk;
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
        let cs = voxel::topo::CS as u32;
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

    /// Прогнать правки тика через ступень A.
    /// Возвращает (грязных чанков, выживших правок).
    pub fn apply(&mut self, w: &World, edits: &[Edit]) -> (usize, usize) {
        let cs = voxel::topo::CS as u32;
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
            let (s, _) = voxel::topo::stage_a::filter(&self.adj, c, &self.local[i]);
            survived += s;
        }
        (self.dirty.len(), survived)
    }

    pub fn live_chunks(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_some()).count()
    }
}

/// Слой навигации: проходимость нав-клетки.
///
/// **Переворот считается на клетку, а не на правку**, и с дедупликацией
/// внутри тика: сотня правок в одном столбе переворачивает клетку один
/// раз или ноль. Считать по правкам — завысить в тридцать раз.
///
/// **Критерий — просвет над полом, а не твёрдое на фиксированной высоте.**
/// Первая редакция брала уровень глаз константой, и на рельефе с размахом
/// в сорок вокселей это дало ровно ноль переворотов: половина карты выше
/// такого уровня, половина ниже. Клетка проходима, если над её полом
/// есть просвет в рост юнита.
pub struct NavLayer {
    passable: Vec<bool>,
    stamp: Vec<u32>,
    epoch: u32,
    w: u32,
    h: u32,
    /// Рост юнита в вокселях: столько нужно свободного над полом.
    pub height: u32,
    step: u32,
}

impl NavLayer {
    pub fn new(w: &World, height: u32, step: u32) -> NavLayer {
        let (nw, nh) = (w.nx / step, w.nz / step);
        let mut l = NavLayer {
            passable: vec![false; (nw * nh) as usize],
            stamp: vec![0; (nw * nh) as usize],
            epoch: 0,
            w: nw,
            h: nh,
            height,
            step,
        };
        for cz in 0..nh {
            for cx in 0..nw {
                l.passable[(cz * nw + cx) as usize] = l.cell_passable(w, cx, cz);
            }
        }
        l
    }

    /// Проходима ли клетка: есть ли над её полом просвет в рост.
    ///
    /// Пол ищется сверху вниз — это верхняя поверхность колонки.
    /// Подземный ход под ней даёт **второй** проходимый уровень
    /// (ADR-039), и этой меркой он не виден: она поверхностная.
    fn cell_passable(&self, w: &World, cx: u32, cz: u32) -> bool {
        for dz in 0..self.step {
            for dx in 0..self.step {
                let (x, z) = (cx * self.step + dx, cz * self.step + dz);
                let mut y = w.ny;
                while y > 0 && !w.solid(x, y - 1, z) {
                    y -= 1;
                }
                if y == 0 {
                    continue;
                }
                let mut clear = 0;
                while clear < self.height && y + clear < w.ny && !w.solid(x, y + clear, z) {
                    clear += 1;
                }
                if clear >= self.height {
                    return true;
                }
            }
        }
        false
    }

    /// Возвращает (пересчитано клеток, перевернулось).
    ///
    /// Две величины, а не одна: работа и результат здесь расходятся
    /// на порядки. Копка вглубь пересчитывает десятки клеток за тик
    /// и не переворачивает ни одной — поверхностная проходимость
    /// не меняется от того, что под ней прокопали ход (ADR-039).
    pub fn apply(&mut self, w: &World, edits: &[Edit]) -> (usize, usize) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.iter_mut().for_each(|v| *v = 0);
            self.epoch = 1;
        }
        let (mut flips, mut seen) = (0, 0);
        for e in edits {
            let (cx, cz) = (e.x / self.step, e.z / self.step);
            if cx >= self.w || cz >= self.h {
                continue;
            }
            let ci = (cz * self.w + cx) as usize;
            // Штамп вместо перебора: без него сотня правок в одном столбе
            // пересчитывала бы клетку сотню раз (ADR-078).
            if self.stamp[ci] == self.epoch {
                continue;
            }
            self.stamp[ci] = self.epoch;
            seen += 1;
            let now = self.cell_passable(w, cx, cz);
            if self.passable[ci] != now {
                self.passable[ci] = now;
                flips += 1;
            }
        }
        (seen, flips)
    }

    pub fn passable_count(&self) -> usize {
        self.passable.iter().filter(|&&p| p).count()
    }
}
