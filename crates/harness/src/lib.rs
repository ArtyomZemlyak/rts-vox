//! Интеграционный стенд: устойчивая копка через все подсистемы сразу.
//!
//! До сих пор каждая подсистема мерилась отдельно, а связь между ними
//! бралась моделью. Три допущения ждали именно этого замера:
//!
//! - «шестьдесят правок на чанк» — множитель к главному слагаемому сметы
//!   связности (`14-connectivity-bench.md`);
//! - «доля правок, переворачивающих непрозрачность нав-клетки» — множитель
//!   ко всей таблице видимости (`13-vision-bench.md`);
//! - «сто пятнадцать грязных чанков» — нагрузка, для которой считался
//!   ремеш (`12-meshing-bench.md`).
//!
//! Здесь они измеряются, а не назначаются.
//!
//! **Это стенд, а не архитектура.** Он держит мир целиком в памяти и
//! соединяет крейты, которые в игре соединены иначе: `sim-core` не знает
//! про мешинг (`CLAUDE.md`, границы слоёв).

#![forbid(unsafe_code)]

pub mod ray;

use fx::rng::Rng;
use mesh::bits::{bits_from_chunk, BitVolume};
use mesh::incr::IncMesher;
use mesh::{Chunk as MeshChunk, Opts, CS as MCS};
use topo::stage_a::Adjacency;
use topo::{Chunk as TopoChunk, Components, CS as TCS};

/// Мир из чанков мешинга, выложенных по горизонтали. По вертикали один слой:
/// 62 воксела при вокселе в метр — этого хватает и для забоя, и для рельефа.
pub struct World {
    pub nx: usize,
    pub nz: usize,
    pub chunks: Vec<MeshChunk>,
    pub bits: Vec<BitVolume>,
    pub meshers: Vec<IncMesher>,
    /// Правки текущего тика, сгруппированные по чанку.
    edits: Vec<Vec<(u8, u8, u8)>>,
    dirty: Vec<bool>,
    dirty_list: Vec<u32>,
    /// Непрозрачность нав-клетки: высота столба выше уровня глаз.
    /// Нав-клетка 2 м, воксель 1 м, значит клетка — четыре столба.
    pub nav_opaque: Vec<bool>,
    pub nav_w: usize,
    pub eye: usize,
    /// Штампы дедупликации нав-клеток. Линейный поиск по списку правок дал бы
    /// квадрат: при 1300 правках это 850 тысяч сравнений на тик.
    nav_stamp: Vec<u32>,
    nav_epoch: u32,
}

/// Сколько вокселей мира приходится на нав-клетку по стороне.
pub const NAV_STEP: usize = 2;

impl World {
    pub fn new(nx: usize, nz: usize, seed: u64) -> World {
        let mut chunks = Vec::with_capacity(nx * nz);
        for cz in 0..nz {
            for cx in 0..nx {
                chunks.push(make_terrain(seed, cx, cz));
            }
        }
        let bits: Vec<BitVolume> = chunks.iter().map(bits_from_chunk).collect();
        let mut meshers: Vec<IncMesher> = (0..nx * nz).map(|_| IncMesher::new()).collect();
        for (m, b) in meshers.iter_mut().zip(bits.iter()) {
            m.rebuild_all(b, Opts::default());
        }
        let nav_w = nx * MCS / NAV_STEP;
        let mut w = World {
            nx,
            nz,
            chunks,
            bits,
            meshers,
            edits: (0..nx * nz).map(|_| Vec::new()).collect(),
            dirty: vec![false; nx * nz],
            dirty_list: Vec::new(),
            nav_opaque: vec![false; nav_w * (nz * MCS / NAV_STEP)],
            nav_w,
            eye: 24,
            nav_stamp: vec![0; nav_w * (nz * MCS / NAV_STEP)],
            nav_epoch: 0,
        };
        w.rebuild_nav();
        w
    }

    pub fn voxels_x(&self) -> usize {
        self.nx * MCS
    }

    pub fn voxels_z(&self) -> usize {
        self.nz * MCS
    }

    /// Внутри мира ли координата. Знаковая версия для DDA.
    #[inline]
    pub fn inside(&self, x: i32, y: i32, z: i32) -> bool {
        x >= 0
            && y >= 0
            && z >= 0
            && (x as usize) < self.voxels_x()
            && (y as usize) < MCS
            && (z as usize) < self.voxels_z()
    }

    /// Твёрдость по знаковым координатам: за пределами мира пусто.
    #[inline]
    pub fn get_i(&self, x: i32, y: i32, z: i32) -> bool {
        if !self.inside(x, y, z) {
            return false;
        }
        self.get(x as usize, y as usize, z as usize)
    }

    /// Слово битового объёма, содержащее колонку по X для данных `y, z`.
    /// Именно оно позволяет проглотить пустую колонку одним тестом —
    /// и именно поэтому раскладка анизотропна.
    #[inline]
    pub fn column_word(&self, x: i32, y: i32, z: i32) -> Option<u64> {
        if !self.inside(x, y, z) {
            return None;
        }
        let (cx, cz) = (x as usize / MCS, z as usize / MCS);
        let lz = z as usize % MCS + 1;
        let ly = y as usize + 1;
        Some(self.bits[cz * self.nx + cx][ly * mesh::CSP + lz])
    }

    /// Твёрдость воксела в мировых координатах.
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> bool {
        if x >= self.voxels_x() || z >= self.voxels_z() || y >= MCS {
            return false;
        }
        let (cx, lx) = (x / MCS, x % MCS + 1);
        let (cz, lz) = (z / MCS, z % MCS + 1);
        self.chunks[cz * self.nx + cx].get(lx, y + 1, lz) != mesh::AIR
    }

    /// Снять воксель. Возвращает `true`, если он был твёрдым.
    ///
    /// Правка у границы чанка обязана попасть **и в паддинг соседа**, иначе
    /// его грани посчитаются неверно. Это не деталь реализации: без неё
    /// на швах появляются лишние квады.
    pub fn dig(&mut self, x: usize, y: usize, z: usize) -> bool {
        if !self.get(x, y, z) {
            return false;
        }
        let (cx, lx) = (x / MCS, x % MCS + 1);
        let (cz, lz) = (z / MCS, z % MCS + 1);
        let ci = cz * self.nx + cx;
        self.chunks[ci].set(lx, y + 1, lz, mesh::AIR);
        self.touch(ci, lx, y + 1, lz);

        // Паддинг соседей по X и Z.
        if lx == 1 && cx > 0 {
            let ni = cz * self.nx + (cx - 1);
            self.chunks[ni].set(MCS + 1, y + 1, lz, mesh::AIR);
            self.touch(ni, MCS + 1, y + 1, lz);
        }
        if lx == MCS && cx + 1 < self.nx {
            let ni = cz * self.nx + (cx + 1);
            self.chunks[ni].set(0, y + 1, lz, mesh::AIR);
            self.touch(ni, 0, y + 1, lz);
        }
        if lz == 1 && cz > 0 {
            let ni = (cz - 1) * self.nx + cx;
            self.chunks[ni].set(lx, y + 1, MCS + 1, mesh::AIR);
            self.touch(ni, lx, y + 1, MCS + 1);
        }
        if lz == MCS && cz + 1 < self.nz {
            let ni = (cz + 1) * self.nx + cx;
            self.chunks[ni].set(lx, y + 1, 0, mesh::AIR);
            self.touch(ni, lx, y + 1, 0);
        }
        true
    }

    /// Правка попадает и в массив материалов, и в **битовый объём** —
    /// сразу, а не пересборкой объёма из байтов.
    ///
    /// ADR-061 требует именно этого: объём обновляется списком правок.
    /// Пересборка объёма обходом 64³ байт стоит около ста микросекунд
    /// на чанк и в сто раз перекрывает сам инкрементальный ремеш.
    #[inline]
    fn touch(&mut self, ci: usize, lx: usize, ly: usize, lz: usize) {
        self.bits[ci][ly * mesh::CSP + lz] &= !(1u64 << lx);
        self.edits[ci].push((lx as u8, ly as u8, lz as u8));
        if !self.dirty[ci] {
            self.dirty[ci] = true;
            self.dirty_list.push(ci as u32);
        }
    }

    /// Применить накопленные правки: обновить битовые объёмы и пересобрать
    /// грязные слои. Возвращает (грязных чанков, всего правок).
    pub fn flush_mesh(&mut self) -> (usize, usize) {
        let mut total = 0;
        for k in 0..self.dirty_list.len() {
            let ci = self.dirty_list[k] as usize;
            total += self.edits[ci].len();
            let edits = std::mem::take(&mut self.edits[ci]);
            self.meshers[ci].apply_edits(&self.bits[ci], &edits, Opts::default());
            self.edits[ci] = edits;
            self.edits[ci].clear();
        }
        let n = self.dirty_list.len();
        for k in 0..self.dirty_list.len() {
            self.dirty[self.dirty_list[k] as usize] = false;
        }
        self.dirty_list.clear();
        (n, total)
    }

    pub fn quad_total(&self) -> usize {
        self.meshers.iter().map(|m| m.quad_count()).sum()
    }

    /// Непрозрачность нав-клетки: хоть один из четырёх столбов клетки
    /// перекрывает линию глаз.
    fn nav_cell_opaque(&self, cx: usize, cz: usize) -> bool {
        for dz in 0..NAV_STEP {
            for dx in 0..NAV_STEP {
                let (x, z) = (cx * NAV_STEP + dx, cz * NAV_STEP + dz);
                if self.get(x, self.eye, z) {
                    return true;
                }
            }
        }
        false
    }

    pub fn rebuild_nav(&mut self) {
        let nh = self.voxels_z() / NAV_STEP;
        for cz in 0..nh {
            for cx in 0..self.nav_w {
                self.nav_opaque[cz * self.nav_w + cx] = self.nav_cell_opaque(cx, cz);
            }
        }
    }

    /// Пересчитать непрозрачность для нав-клеток, накрытых правками, и
    /// вернуть, сколько из них **перевернулось**. Это тот самый множитель,
    /// который в замере видимости брался моделью.
    pub fn nav_flips(&mut self, dug: &[(usize, usize, usize)]) -> usize {
        self.nav_epoch = self.nav_epoch.wrapping_add(1);
        if self.nav_epoch == 0 {
            self.nav_stamp.iter_mut().for_each(|v| *v = 0);
            self.nav_epoch = 1;
        }
        let nh = self.voxels_z() / NAV_STEP;
        let mut flips = 0;
        for &(x, _y, z) in dug {
            let (cx, cz) = (x / NAV_STEP, z / NAV_STEP);
            if cx >= self.nav_w || cz >= nh {
                continue;
            }
            let ci = cz * self.nav_w + cx;
            if self.nav_stamp[ci] == self.nav_epoch {
                continue;
            }
            self.nav_stamp[ci] = self.nav_epoch;
            let now = self.nav_cell_opaque(cx, cz);
            if self.nav_opaque[ci] != now {
                self.nav_opaque[ci] = now;
                flips += 1;
            }
        }
        flips
    }
}

/// Рельеф: высота из целочисленного шума, как в генераторе.
fn make_terrain(seed: u64, cx: usize, cz: usize) -> MeshChunk {
    let mut c = MeshChunk::new();
    for lz in 0..=MCS + 1 {
        for lx in 0..=MCS + 1 {
            let (wx, wz) = (
                (cx * MCS + lx) as i32 - 1,
                (cz * MCS + lz) as i32 - 1,
            );
            let h = height(seed, wx, wz);
            for y in 1..=h.min(MCS) {
                c.set(lx, y, lz, 1);
            }
        }
    }
    c
}

fn height(seed: u64, x: i32, z: i32) -> usize {
    let hash = |a: i32, b: i32| -> i32 {
        let mut h = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((a as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
            .wrapping_add((b as i64 as u64).wrapping_mul(0x94D0_49BB_1331_11EB));
        h ^= h >> 31;
        h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        ((h >> 40) & 0xFFFF) as i32
    };
    let period = 48;
    let (x0, z0) = (x.div_euclid(period), z.div_euclid(period));
    let (fx, fz) = (x.rem_euclid(period), z.rem_euclid(period));
    let (h00, h10) = (hash(x0, z0), hash(x0 + 1, z0));
    let (h01, h11) = (hash(x0, z0 + 1), hash(x0 + 1, z0 + 1));
    let a = h00 + (h10 - h00) * fx / period;
    let b = h01 + (h11 - h01) * fx / period;
    let n = a + (b - a) * fz / period;
    (26 + n * 10 / 65536).clamp(6, MCS as i32 - 4) as usize
}

/// Копатель: стоит на поверхности и вгрызается вниз-вперёд.
#[derive(Clone, Copy)]
pub struct Digger {
    pub x: usize,
    pub y: usize,
    pub z: usize,
    pub dx: i8,
    pub dz: i8,
    /// Сколько тиков копал на месте.
    pub age: u32,
}

pub struct Dig {
    pub diggers: Vec<Digger>,
    pub radius: i32,
    rng: Rng,
}

impl Dig {
    pub fn new(w: &World, n: usize, radius: i32, seed: u64) -> Dig {
        let mut rng = Rng::new(seed, 0xD16);
        let mut diggers = Vec::with_capacity(n);
        while diggers.len() < n {
            let x = rng.below(w.voxels_x() as u32) as usize;
            let z = rng.below(w.voxels_z() as u32) as usize;
            // Ставим на поверхность.
            let mut y = MCS - 1;
            while y > 0 && !w.get(x, y, z) {
                y -= 1;
            }
            if y < 8 {
                continue;
            }
            let (dx, dz) = match rng.below(4) {
                0 => (1i8, 0i8),
                1 => (-1, 0),
                2 => (0, 1),
                _ => (0, -1),
            };
            diggers.push(Digger {
                x,
                y,
                z,
                dx,
                dz,
                age: 0,
            });
        }
        Dig {
            diggers,
            radius,
            rng,
        }
    }

    /// Один тик копки. Возвращает список снятых вокселей в мировых
    /// координатах — он же вход для связности и для навигации.
    pub fn step(&mut self, w: &mut World, out: &mut Vec<(usize, usize, usize)>) {
        out.clear();
        let r = self.radius;
        for i in 0..self.diggers.len() {
            let d = self.diggers[i];
            for dy in -r..=r {
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy + dz * dz > r * r {
                            continue;
                        }
                        let (x, y, z) = (
                            d.x as i64 + dx as i64,
                            d.y as i64 + dy as i64,
                            d.z as i64 + dz as i64,
                        );
                        if x < 0 || y < 1 || z < 0 {
                            continue;
                        }
                        let (x, y, z) = (x as usize, y as usize, z as usize);
                        if w.dig(x, y, z) {
                            out.push((x, y, z));
                        }
                    }
                }
            }
            // Продвижение: вниз-вперёд, с редким поворотом.
            let d = &mut self.diggers[i];
            d.age += 1;
            let step = if d.age % 3 == 0 { 1 } else { 0 };
            if step == 1 {
                let nx = d.x as i64 + d.dx as i64;
                let nz = d.z as i64 + d.dz as i64;
                if nx >= 0 && nz >= 0 && (nx as usize) < w.voxels_x() && (nz as usize) < w.voxels_z()
                {
                    d.x = nx as usize;
                    d.z = nz as usize;
                }
                if d.y > 2 && d.age % 6 == 0 {
                    d.y -= 1;
                }
            }
            if self.rng.below(64) == 0 {
                let (dx, dz) = match self.rng.below(4) {
                    0 => (1i8, 0i8),
                    1 => (-1, 0),
                    2 => (0, 1),
                    _ => (0, -1),
                };
                d.dx = dx;
                d.dz = dz;
            }
        }
    }
}

/// Связность: чанки `topo` 32³ поверх той же геометрии.
///
/// Размер чанка связности (32³, ADR-024) и размер среза мешинга (62³)
/// не совпадают — это разные подсистемы с разными единицами, и стенд
/// обязан считать грязные чанки для каждой отдельно.
pub struct TopoLayer {
    pub nx: usize,
    pub nz: usize,
    pub ny: usize,
    pub chunks: Vec<TopoChunk>,
    pub adj: Adjacency,
    pub comp: Components,
    dirty: Vec<bool>,
    dirty_list: Vec<u32>,
    /// Сколько правок прошло ступень A за последний тик.
    pub last_survivors: usize,
}

impl TopoLayer {
    pub fn new(w: &World) -> TopoLayer {
        let nx = w.voxels_x().div_ceil(TCS);
        let nz = w.voxels_z().div_ceil(TCS);
        let ny = MCS.div_ceil(TCS);
        let mut chunks = Vec::with_capacity(nx * ny * nz);
        for cy in 0..ny {
            for cz in 0..nz {
                for cx in 0..nx {
                    let mut c = TopoChunk::new();
                    for z in 0..TCS {
                        for y in 0..TCS {
                            for x in 0..TCS {
                                if w.get(cx * TCS + x, cy * TCS + y, cz * TCS + z) {
                                    c.set(x, y, z, true);
                                }
                            }
                        }
                    }
                    chunks.push(c);
                }
            }
        }
        let n = nx * ny * nz;
        TopoLayer {
            nx,
            nz,
            ny,
            chunks,
            adj: Adjacency::new(),
            comp: Components::new(),
            dirty: vec![false; n],
            dirty_list: Vec::new(),
            last_survivors: 0,
        }
    }

    #[inline]
    fn index(&self, cx: usize, cy: usize, cz: usize) -> usize {
        (cy * self.nz + cz) * self.nx + cx
    }

    /// Применить список снятых вокселей: ступень A на каждый, пометка
    /// грязных чанков. Возвращает (грязных чанков, выживших после A).
    pub fn apply(&mut self, dug: &[(usize, usize, usize)]) -> (usize, usize) {
        let mut survivors = 0usize;
        for &(x, y, z) in dug {
            let (cx, lx) = (x / TCS, x % TCS);
            let (cy, ly) = (y / TCS, y % TCS);
            let (cz, lz) = (z / TCS, z % TCS);
            if cx >= self.nx || cy >= self.ny || cz >= self.nz {
                continue;
            }
            let ci = self.index(cx, cy, cz);
            // Ступень A считается **до** снятия соседей по списку: она
            // корректна только по одному вокселю за раз (ADR-074).
            self.chunks[ci].set(lx, ly, lz, false);
            if !self.adj.cannot_split(self.adj.gather(&self.chunks[ci], lx, ly, lz)) {
                survivors += 1;
            }
            if !self.dirty[ci] {
                self.dirty[ci] = true;
                self.dirty_list.push(ci as u32);
            }
        }
        self.last_survivors = survivors;
        let n = self.dirty_list.len();
        (n, survivors)
    }

    /// Пересчитать компоненты грязных чанков. Возвращает, сколько чанков
    /// пересчитано.
    pub fn recompute(&mut self) -> usize {
        let n = self.dirty_list.len();
        for k in 0..self.dirty_list.len() {
            let ci = self.dirty_list[k] as usize;
            self.comp.build(&self.chunks[ci]);
            self.dirty[ci] = false;
        }
        self.dirty_list.clear();
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Стенд обязан копать: за сто тиков объём твёрдого должен заметно упасть.
    #[test]
    fn digging_actually_removes_matter() {
        let mut w = World::new(2, 2, 5);
        let before: usize = count_solid(&w);
        let mut dig = Dig::new(&w, 20, 2, 1);
        let mut out = Vec::new();
        for _ in 0..100 {
            dig.step(&mut w, &mut out);
            w.flush_mesh();
        }
        let after = count_solid(&w);
        // Устойчивая копка снимает **много меньше**, чем первый залп:
        // копатель продвигается на воксель раз в три тика и вгрызается
        // в основном в уже пустое. Двадцать копателей за сто тиков дают
        // порядка девяти тысяч вокселей, а не тридцати трёх на копателя
        // за тик.
        assert!(
            after + 5_000 < before,
            "снято всего {} вокселей",
            before - after
        );
    }

    fn count_solid(w: &World) -> usize {
        let mut n = 0;
        for z in 0..w.voxels_z() {
            for y in 0..MCS {
                for x in 0..w.voxels_x() {
                    if w.get(x, y, z) {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// **Швы.** Инкрементальный ремеш стенда обязан давать то же, что полная
    /// пересборка каждого чанка: если паддинг соседа не обновляется, на швах
    /// появляются лишние квады, и поймать это можно только так.
    #[test]
    fn seams_survive_digging() {
        let mut w = World::new(2, 2, 7);
        let mut dig = Dig::new(&w, 12, 2, 3);
        let mut out = Vec::new();
        for _ in 0..60 {
            dig.step(&mut w, &mut out);
            w.flush_mesh();
        }
        let mut full = mesh::Mesher::new();
        for (ci, c) in w.chunks.iter().enumerate() {
            let inc = w.meshers[ci].quad_count();
            let exact = full.mesh(c, Opts::default()).quads;
            assert_eq!(inc, exact, "чанк {ci}: инкремент {inc}, полная {exact}");
        }
    }

    /// Слой связности обязан видеть те же снятия, что и мир.
    #[test]
    fn topo_layer_follows_the_world() {
        let mut w = World::new(1, 1, 11);
        let mut t = TopoLayer::new(&w);
        let mut dig = Dig::new(&w, 5, 2, 2);
        let mut out = Vec::new();
        for _ in 0..30 {
            dig.step(&mut w, &mut out);
            w.flush_mesh();
            t.apply(&out);
            t.recompute();
        }
        for z in 0..TCS {
            for y in 0..TCS {
                for x in 0..TCS {
                    assert_eq!(
                        t.chunks[0].get(x, y, z),
                        w.get(x, y, z),
                        "расхождение в ({x},{y},{z})"
                    );
                }
            }
        }
    }
}
