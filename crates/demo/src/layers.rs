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
/// Отсутствие пола: колонка либо пустая до дна, либо сплошная до неба.
pub const NO_FLOOR: u16 = u16::MAX;

pub struct NavLayer {
    /// Высота пола клетки. **Это и есть та величина, которую меняет
    /// копка** — булева проходимость её не видит.
    floor: Vec<u16>,
    /// Есть ли над полом просвет в рост.
    stand: Vec<bool>,
    stamp: Vec<u32>,
    epoch: u32,
    w: u32,
    h: u32,
    /// Рост юнита в вокселях: столько нужно свободного над полом.
    pub height: u32,
    /// Какой перепад юнит перешагивает. Больше — обрыв, ребра нет.
    pub max_step: u32,
    step: u32,
}

impl NavLayer {
    pub fn new(w: &World, height: u32, step: u32) -> NavLayer {
        let (nw, nh) = (w.nx / step, w.nz / step);
        let mut l = NavLayer {
            floor: vec![NO_FLOOR; (nw * nh) as usize],
            stand: vec![false; (nw * nh) as usize],
            stamp: vec![0; (nw * nh) as usize],
            epoch: 0,
            w: nw,
            h: nh,
            height,
            max_step: 1,
            step,
        };
        for cz in 0..nh {
            for cx in 0..nw {
                let (f, st) = l.probe(w, cx, cz);
                let i = (cz * nw + cx) as usize;
                l.floor[i] = f;
                l.stand[i] = st;
            }
        }
        l
    }

    /// Пол клетки и есть ли над ним просвет.
    ///
    /// **Пол ищется снизу вверх.** Поверхностная мерка берёт верх
    /// колонки и потому не видит прокопанный под грядой ход, а верх
    /// отвесной стены считает проходимым (ADR-113). Снизу вверх
    /// берётся нижний проходимый уровень — тот самый, который
    /// единственный по ADR-039.
    ///
    /// Клетка шире воксела, поэтому берётся **самая низкая** её
    /// подколонка с просветом: юнит стоит там, где может стоять.
    fn probe(&self, w: &World, cx: u32, cz: u32) -> (u16, bool) {
        let mut best = NO_FLOOR;
        for dz in 0..self.step {
            for dx in 0..self.step {
                let (x, z) = (cx * self.step + dx, cz * self.step + dz);
                let mut y = 1u32;
                while y < w.ny {
                    if w.solid(x, y - 1, z) && !w.solid(x, y, z) {
                        let mut clear = 0;
                        while clear < self.height && y + clear < w.ny && !w.solid(x, y + clear, z)
                        {
                            clear += 1;
                        }
                        if clear >= self.height {
                            if (y as u16) < best {
                                best = y as u16;
                            }
                            break;
                        }
                    }
                    y += 1;
                }
            }
        }
        (best, best != NO_FLOOR)
    }

    #[inline]
    fn idx(&self, cx: u32, cz: u32) -> usize {
        (cz * self.w + cx) as usize
    }

    pub fn floor_at(&self, cx: u32, cz: u32) -> u16 {
        self.floor[self.idx(cx, cz)]
    }

    pub fn standable(&self, cx: u32, cz: u32) -> bool {
        self.stand[self.idx(cx, cz)]
    }

    /// Есть ли ребро между соседними клетками.
    ///
    /// **Перепад — свойство ребра, а не клетки** (ADR-113). Локальный
    /// критерий не работает ни в какую сторону: строгий запрещает ходить
    /// вдоль любой стены, мягкий оставляет проходимой вершину отвесной
    /// гряды.
    pub fn edge(&self, a: (u32, u32), b: (u32, u32)) -> bool {
        let (ia, ib) = (self.idx(a.0, a.1), self.idx(b.0, b.1));
        if !self.stand[ia] || !self.stand[ib] {
            return false;
        }
        let (fa, fb) = (self.floor[ia] as i32, self.floor[ib] as i32);
        (fa - fb).unsigned_abs() <= self.max_step
    }

    /// Сколько рёбер выходит из клетки. Нужно, чтобы считать разрывы:
    /// именно они, а не булева проходимость, — то, что копка меняет.
    fn degree(&self, cx: u32, cz: u32) -> u32 {
        let mut d = 0;
        for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (nx, nz) = (cx as i32 + dx, cz as i32 + dz);
            if nx < 0 || nz < 0 || nx >= self.w as i32 || nz >= self.h as i32 {
                continue;
            }
            if self.edge((cx, cz), (nx as u32, nz as u32)) {
                d += 1;
            }
        }
        d
    }

    /// Возвращает `NavStats`.
    ///
    /// Три величины, а не одна: работа, изменение пола и изменение
    /// рёбер расходятся на порядки. Булева проходимость сама по себе
    /// не меняется почти никогда — над поверхностью всегда небо, —
    /// и сцена, мерившая только её, мерила ноль (ADR-117).
    pub fn apply(&mut self, w: &World, edits: &[Edit]) -> NavStats {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.iter_mut().for_each(|v| *v = 0);
            self.epoch = 1;
        }
        let mut st = NavStats::default();
        // Сначала собрать затронутые клетки: степень зависит от соседей,
        // поэтому «до» надо снять до любой записи.
        let mut touched: Vec<(u32, u32, u16, bool, u32)> = Vec::new();
        for e in edits {
            let (cx, cz) = (e.x / self.step, e.z / self.step);
            if cx >= self.w || cz >= self.h {
                continue;
            }
            let ci = self.idx(cx, cz);
            // Штамп вместо перебора: без него сотня правок в одном столбе
            // пересчитывала бы клетку сотню раз (ADR-078).
            if self.stamp[ci] == self.epoch {
                continue;
            }
            self.stamp[ci] = self.epoch;
            touched.push((cx, cz, self.floor[ci], self.stand[ci], self.degree(cx, cz)));
        }
        st.seen = touched.len();
        for &(cx, cz, _, _, _) in &touched {
            let (f, sd) = self.probe(w, cx, cz);
            let ci = self.idx(cx, cz);
            self.floor[ci] = f;
            self.stand[ci] = sd;
        }
        for &(cx, cz, f0, s0, d0) in &touched {
            let ci = self.idx(cx, cz);
            if self.stand[ci] != s0 {
                st.flips += 1;
            }
            if self.floor[ci] != f0 {
                st.floor_moved += 1;
                st.floor_drop += (f0.min(NO_FLOOR - 1) as i32 - self.floor[ci].min(NO_FLOOR - 1) as i32)
                    .max(0) as u64;
            }
            let d1 = self.degree(cx, cz);
            st.edges_lost += d0.saturating_sub(d1) as u64;
            st.edges_gained += d1.saturating_sub(d0) as u64;
        }
        st
    }

    /// Сколько уровней, на которых можно стоять, есть в подколонке.
    ///
    /// Величина существует затем, чтобы **сосчитать, насколько неверна
    /// одноуровневая навигация** (ADR-039). Выбор уровня в колонке —
    /// не деталь реализации: сверху вниз теряется прокопанный ход
    /// (ADR-113), снизу вверх выбирается пол пещеры под поверхностью.
    /// Оба ответа неверны там, где уровней больше одного, и вопрос
    /// в том, где таких колонок много.
    fn levels_in_column(&self, w: &World, x: u32, z: u32) -> u32 {
        let mut n = 0;
        let mut y = 1u32;
        while y < w.ny {
            if w.solid(x, y - 1, z) && !w.solid(x, y, z) {
                let mut clear = 0;
                while clear < self.height && y + clear < w.ny && !w.solid(x, y + clear, z) {
                    clear += 1;
                }
                if clear >= self.height {
                    n += 1;
                    y += self.height;
                    continue;
                }
            }
            y += 1;
        }
        n
    }

    /// (клеток с полом, из них многоуровневых, уровней всего).
    /// Полный обход — вызывается один раз, не в горячем пути.
    pub fn level_census(&self, w: &World) -> (usize, usize, u64) {
        let (mut with, mut multi, mut total) = (0usize, 0usize, 0u64);
        for cz in 0..self.h {
            for cx in 0..self.w {
                let mut best = 0;
                for dz in 0..self.step {
                    for dx in 0..self.step {
                        let n = self.levels_in_column(w, cx * self.step + dx, cz * self.step + dz);
                        best = best.max(n);
                        total += n as u64;
                    }
                }
                if best > 0 {
                    with += 1;
                }
                if best > 1 {
                    multi += 1;
                }
            }
        }
        (with, multi, total)
    }

    pub fn standable_count(&self) -> usize {
        self.stand.iter().filter(|&&p| p).count()
    }

    /// Рёбер во всём графе. Считается по два раза на ребро, поэтому
    /// делится пополам.
    pub fn edge_count(&self) -> usize {
        let mut n = 0usize;
        for cz in 0..self.h {
            for cx in 0..self.w {
                n += self.degree(cx, cz) as usize;
            }
        }
        n / 2
    }
}

/// Что дал тик навигации.
#[derive(Default, Clone, Copy)]
pub struct NavStats {
    /// Клеток пересчитано.
    pub seen: usize,
    /// У скольких изменилась сама возможность стоять.
    pub flips: usize,
    /// У скольких сдвинулся пол.
    pub floor_moved: usize,
    /// Суммарное опускание пола в вокселях.
    pub floor_drop: u64,
    /// Рёбер разорвано и появилось.
    pub edges_lost: u64,
    pub edges_gained: u64,
}
