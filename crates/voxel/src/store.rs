//! Хранение чанка: плотный массив против построчной упаковки (ADR-024).
//!
//! Три вопроса, помеченных в бюджетах и дорожной карте:
//! стоимость записи в row-packed против плотного, доля горячих чанков,
//! копирование чанков в память игрока (ADR-029).
//!
//! Чанк 32³ (ADR-024). Строка — 32 воксела вдоль X, ровно та единица,
//! которую читает и мешинг, и марш луча.

pub const CS: usize = 32;
pub const ROWS: usize = CS * CS;
/// Номер материала. **u16 по ADR-094**: при вокселе 0.5 м и помеченных
/// подсистемах вариантов заведомо больше 256, а мешер и так принимает
/// `u16`, поэтому расширяющего прохода не нужно.
///
/// Это номер **в палитре чанка**, а не индекс в данных: индекс 4- или
/// 8-битный, и от ширины `Mat` он не зависит. Ширина `Mat` меняет только
/// размер самой палитры — единицы байт на чанк.
pub type Mat = u16;
pub const AIR: Mat = 0;

/// Плотный чанк: байт на воксель. 32 КиБ.
#[derive(Clone)]
pub struct Dense {
    pub v: Vec<Mat>,
}

impl Default for Dense {
    fn default() -> Self {
        Self::new()
    }
}

impl Dense {
    pub fn new() -> Dense {
        Dense {
            v: vec![AIR; CS * CS * CS],
        }
    }

    #[inline(always)]
    pub fn idx(x: usize, y: usize, z: usize) -> usize {
        (y * CS + z) * CS + x
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Mat {
        self.v[Self::idx(x, y, z)]
    }

    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, z: usize, m: Mat) {
        self.v[Self::idx(x, y, z)] = m;
    }

    pub fn bytes(&self) -> usize {
        self.v.len()
    }
}

/// Одна упакованная строка: палитра плюс индексы по 0/1/2/4 бита.
///
/// Ноль бит — вся строка одного материала, самый частый случай в толще
/// породы и в воздухе. Именно он и даёт экономию.
#[derive(Clone, Default)]
struct Row {
    /// Материалы строки. Длина до 16; больше — строка разжимается в плотную.
    palette: Vec<Mat>,
    /// Упакованные индексы. Пусто при нуле бит.
    data: Vec<u32>,
    bits: u8,
}

impl Row {
    fn uniform(m: Mat) -> Row {
        Row {
            palette: vec![m],
            data: Vec::new(),
            bits: 0,
        }
    }

    #[inline(always)]
    fn get(&self, x: usize) -> Mat {
        if self.bits == 0 {
            return self.palette[0];
        }
        let per = 32 / self.bits as usize;
        let w = self.data[x / per];
        let sh = (x % per) * self.bits as usize;
        let mask = (1u32 << self.bits) - 1;
        self.palette[((w >> sh) & mask) as usize]
    }

    fn bytes(&self) -> usize {
        self.palette.len() * core::mem::size_of::<Mat>() + self.data.len() * 4
    }

    /// Записать. Возвращает `true`, если строку пришлось перепаковать —
    /// это и есть та цена, из-за которой упаковку подозревают.
    fn set(&mut self, x: usize, m: Mat) -> bool {
        let pi = self.palette.iter().position(|&p| p == m);
        let (pi, grew) = match pi {
            Some(i) => (i, false),
            None => {
                self.palette.push(m);
                (self.palette.len() - 1, true)
            }
        };
        let need = bits_for(self.palette.len());
        if need > self.bits {
            self.repack(need);
            self.write_index(x, pi);
            return true;
        }
        self.write_index(x, pi);
        grew
    }

    #[inline(always)]
    fn write_index(&mut self, x: usize, pi: usize) {
        if self.bits == 0 {
            return;
        }
        let per = 32 / self.bits as usize;
        let sh = (x % per) * self.bits as usize;
        let mask = (1u32 << self.bits) - 1;
        let w = &mut self.data[x / per];
        *w = (*w & !(mask << sh)) | ((pi as u32) << sh);
    }

    /// Сузить палитру до фактически использованных материалов.
    ///
    /// На записи палитра только растёт: проверять, не исчез ли материал
    /// из строки, значит читать всю строку на каждую запись. Поэтому
    /// сужение — отдельный редкий проход, и его цена меряется отдельно.
    /// Возвращает `true`, если что-то изменилось.
    fn compact(&mut self) -> bool {
        if self.bits == 0 {
            return false;
        }
        let mut used = [false; 16];
        for x in 0..CS {
            used[self.index_at(x)] = true;
        }
        let n = used.iter().filter(|&&u| u).count();
        if n == self.palette.len() {
            return false;
        }
        let mut remap = [0usize; 16];
        let mut fresh: Vec<Mat> = Vec::with_capacity(n);
        for (old, &u) in used.iter().enumerate() {
            if u {
                remap[old] = fresh.len();
                fresh.push(self.palette[old]);
            }
        }
        let old_idx: Vec<usize> = (0..CS).map(|x| remap[self.index_at(x)]).collect();
        self.palette = fresh;
        let bits = bits_for(self.palette.len());
        self.bits = bits;
        if bits == 0 {
            self.data.clear();
        } else {
            let per = 32 / bits as usize;
            self.data = vec![0u32; CS.div_ceil(per)];
            for (x, &pi) in old_idx.iter().enumerate() {
                self.write_index(x, pi);
            }
        }
        true
    }

    #[inline(always)]
    fn index_at(&self, x: usize) -> usize {
        if self.bits == 0 {
            return 0;
        }
        let per = 32 / self.bits as usize;
        let w = self.data[x / per];
        let sh = (x % per) * self.bits as usize;
        let mask = (1u32 << self.bits) - 1;
        ((w >> sh) & mask) as usize
    }

    fn repack(&mut self, bits: u8) {
        let old: Vec<usize> = (0..CS)
            .map(|x| {
                if self.bits == 0 {
                    0
                } else {
                    let per = 32 / self.bits as usize;
                    let w = self.data[x / per];
                    let sh = (x % per) * self.bits as usize;
                    let mask = (1u32 << self.bits) - 1;
                    ((w >> sh) & mask) as usize
                }
            })
            .collect();
        self.bits = bits;
        let per = 32 / bits as usize;
        self.data = vec![0u32; CS.div_ceil(per)];
        for (x, &pi) in old.iter().enumerate() {
            self.write_index(x, pi);
        }
    }
}

fn bits_for(n: usize) -> u8 {
    match n {
        0 | 1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        _ => 8,
    }
}

/// Чанк из упакованных строк.
#[derive(Clone)]
pub struct RowPacked {
    rows: Vec<Row>,
    /// Сколько раз строку пришлось перепаковать за всё время.
    pub repacks: u64,
}

impl Default for RowPacked {
    fn default() -> Self {
        Self::new()
    }
}

impl RowPacked {
    pub fn new() -> RowPacked {
        RowPacked {
            rows: (0..ROWS).map(|_| Row::uniform(AIR)).collect(),
            repacks: 0,
        }
    }

    #[inline(always)]
    fn row(y: usize, z: usize) -> usize {
        y * CS + z
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Mat {
        self.rows[Self::row(y, z)].get(x)
    }

    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, z: usize, m: Mat) {
        if self.rows[Self::row(y, z)].set(x, m) {
            self.repacks += 1;
        }
    }

    pub fn bytes(&self) -> usize {
        self.rows.iter().map(|r| r.bytes()).sum::<usize>() + self.rows.len() * 16
    }

    /// Доля строк, состоящих из одного материала. Это и есть источник
    /// экономии: такая строка не хранит индексов вовсе.
    pub fn uniform_rows(&self) -> usize {
        self.rows.iter().filter(|r| r.bits == 0).count()
    }

    /// Сузить палитры всех строк. Редкий проход: на записи палитра
    /// только растёт, потому что иначе каждая запись читала бы строку
    /// целиком.
    pub fn compact(&mut self) -> usize {
        let mut n = 0;
        for r in self.rows.iter_mut() {
            if r.compact() {
                n += 1;
            }
        }
        n
    }

    pub fn from_dense(d: &Dense) -> RowPacked {
        let mut p = RowPacked::new();
        for y in 0..CS {
            for z in 0..CS {
                for x in 0..CS {
                    let m = d.get(x, y, z);
                    if m != AIR {
                        p.set(x, y, z, m);
                    }
                }
            }
        }
        p.repacks = 0;
        p
    }
}

/// Палитра на **чанк**, индекс фиксированной ширины, один массив на всё.
///
/// Это классическая раскладка «палитра плюс битовый массив», и от построчной
/// она отличается тем, что не держит тысячу отдельных аллокаций. Ширина
/// индекса растёт по чанку целиком: 4 бита до шестнадцати материалов,
/// дальше 8 — и тогда это плотный массив плюс палитра.
#[derive(Clone)]
pub struct Paletted {
    palette: Vec<Mat>,
    data: Vec<u32>,
    bits: u8,
    /// Сколько раз чанк пришлось расширить.
    pub widenings: u64,
    /// Order-independent content fingerprint; palette insertion order is not state.
    content_hash: u64,
}

impl Default for Paletted {
    fn default() -> Self {
        Self::new()
    }
}

impl Paletted {
    /// A contiguous X span. Uniform packed words decode once for the whole
    /// run; mixed words retain the exact palette values and voxel order.
    pub(crate) fn copy_row(&self, x: usize, y: usize, z: usize, out: &mut [Mat]) {
        debug_assert!(x + out.len() <= CS && y < CS && z < CS);
        match self.bits {
            4 => self.copy_row_width::<4>(x, y, z, out),
            8 => self.copy_row_width::<8>(x, y, z, out),
            16 => self.copy_row_width::<16>(x, y, z, out),
            _ => unreachable!("unsupported palette index width"),
        }
    }

    /// Fixed widths turn packed-word division and remainder into shifts/masks.
    /// Dispatch once per row, including partial first and last words.
    fn copy_row_width<const BITS: usize>(&self, x: usize, y: usize, z: usize, out: &mut [Mat]) {
        let bits = BITS;
        let per = 32 / bits;
        let mask = (1u32 << bits) - 1;
        let mut done = 0;
        let first = Dense::idx(x, y, z);
        while done < out.len() {
            let i = first + done;
            let count = (per - i % per).min(out.len() - done);
            let word = self.data[i / per] >> (i % per * bits);
            let index = word & mask;
            let used = u32::MAX >> (32 - count * bits);
            let same = word & used == (index * (u32::MAX / mask)) & used;
            if same {
                out[done..done + count].fill(self.palette[index as usize]);
            } else {
                for k in 0..count {
                    out[done + k] = self.palette[((word >> (k * bits)) & mask) as usize];
                }
            }
            done += count;
        }
    }
    pub(crate) fn material_candidates(&self) -> &[Mat] {
        &self.palette
    }
    pub fn new() -> Paletted {
        Paletted {
            palette: vec![AIR],
            data: vec![0u32; CS * CS * CS / 8],
            bits: 4,
            widenings: 0,
            content_hash: 0,
        }
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Mat {
        let i = Dense::idx(x, y, z);
        let per = 32 / self.bits as usize;
        let w = self.data[i / per];
        let sh = (i % per) * self.bits as usize;
        let mask = (1u32 << self.bits) - 1;
        self.palette[((w >> sh) & mask) as usize]
    }

    /// Exact uniformity test over a nonempty chunk-local half-open box.
    /// Compare packed indices a word at a time, including partial end words.
    /// The first voxel supplies the index: no scan of a possibly large palette.
    pub(crate) fn box_is_material(&self, min: [usize; 3], max: [usize; 3], mat: Mat) -> bool {
        debug_assert!((0..3).all(|a| min[a] < max[a] && max[a] <= CS));
        let bits = self.bits as usize;
        let per = 32 / bits;
        let index_mask = (1u32 << bits) - 1;
        let first = Dense::idx(min[0], min[1], min[2]);
        let index = (self.data[first / per] >> (first % per * bits)) & index_mask;
        if self.palette[index as usize] != mat {
            return false;
        }
        let repeated = (u32::MAX / index_mask) * index;
        for y in min[1]..max[1] {
            for z in min[2]..max[2] {
                let start = Dense::idx(min[0], y, z);
                let end = start + max[0] - min[0];
                for word in start / per..=(end - 1) / per {
                    let lo = if word == start / per {
                        start % per * bits
                    } else {
                        0
                    };
                    let hi = if word == (end - 1) / per {
                        ((end - 1) % per + 1) * bits
                    } else {
                        32
                    };
                    let mask = (u32::MAX << lo) & (u32::MAX >> (32 - hi));
                    if (self.data[word] ^ repeated) & mask != 0 {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Exact certificate allowing holes in an otherwise single-material box.
    /// AIR always has palette index zero (the palette only grows from `new`).
    pub(crate) fn box_is_material_or_air(
        &self,
        min: [usize; 3],
        max: [usize; 3],
        mat: Mat,
    ) -> bool {
        debug_assert!((0..3).all(|a| min[a] < max[a] && max[a] <= CS));
        let bits = self.bits as usize;
        let per = 32 / bits;
        let index_mask = (1u32 << bits) - 1;
        let lanes = u32::MAX / index_mask;
        let nonzero = |mut v: u32| {
            v |= v >> 1;
            v |= v >> 2;
            if bits >= 8 {
                v |= v >> 4;
            }
            if bits >= 16 {
                v |= v >> 8;
            }
            v & lanes
        };
        // Derive the one allowed solid index from live data, without scanning
        // the palette. Foreign entries outside the box cannot weaken the proof.
        let mut repeated = None;
        for y in min[1]..max[1] {
            for z in min[2]..max[2] {
                let start = Dense::idx(min[0], y, z);
                let end = start + max[0] - min[0];
                for word in start / per..=(end - 1) / per {
                    let lo = if word == start / per {
                        start % per * bits
                    } else {
                        0
                    };
                    let hi = if word == (end - 1) / per {
                        ((end - 1) % per + 1) * bits
                    } else {
                        32
                    };
                    let mask = (u32::MAX << lo) & (u32::MAX >> (32 - hi));
                    let v = self.data[word] & mask;
                    if v == 0 {
                        continue;
                    }
                    let pattern = match repeated {
                        Some(v) => v,
                        None => {
                            let shift = v.trailing_zeros() as usize / bits * bits;
                            let index = (v >> shift) & index_mask;
                            if self.palette[index as usize] != mat {
                                return false;
                            }
                            let pattern = lanes * index;
                            repeated = Some(pattern);
                            pattern
                        }
                    };
                    let different = (v ^ pattern) & mask;
                    if different != 0 && nonzero(v) & nonzero(different) & mask != 0 {
                        return false;
                    }
                }
            }
        }
        true
    }

    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, z: usize, m: Mat) {
        let was = self.get(x, y, z);
        if was == m {
            return;
        }
        let index = Dense::idx(x, y, z);
        self.content_hash ^= Self::cell_hash(index, was) ^ Self::cell_hash(index, m);
        let pi = match self.palette.iter().position(|&p| p == m) {
            Some(k) => k,
            None => {
                self.palette.push(m);
                if self.palette.len() > (1usize << self.bits) {
                    self.widen();
                }
                self.palette.len() - 1
            }
        };
        let i = Dense::idx(x, y, z);
        let per = 32 / self.bits as usize;
        let sh = (i % per) * self.bits as usize;
        let mask = (1u32 << self.bits) - 1;
        let w = &mut self.data[i / per];
        *w = (*w & !(mask << sh)) | ((pi as u32) << sh);
    }

    /// Удвоить ширину индекса: четыре бита держат шестнадцать
    /// материалов, восемь — 256, шестнадцать — любой `Mat` (ADR-446).
    /// Палитра только растёт, и чанк, видевший 257 материалов, требует
    /// третьей ширины: на восьми битах 257-й индекс заходил в биты
    /// соседней клетки либо обнулялся в воздух.
    fn widen(&mut self) {
        let old_bits = self.bits;
        let old = std::mem::take(&mut self.data);
        self.bits = old_bits * 2;
        self.data = vec![0u32; CS * CS * CS * self.bits as usize / 32];
        let old_per = 32 / old_bits as usize;
        let old_mask = (1u32 << old_bits) - 1;
        for i in 0..CS * CS * CS {
            let v = (old[i / old_per] >> ((i % old_per) * old_bits as usize)) & old_mask;
            let per = 32 / self.bits as usize;
            let sh = (i % per) * self.bits as usize;
            let mask = (1u32 << self.bits) - 1;
            let w = &mut self.data[i / per];
            *w = (*w & !(mask << sh)) | (v << sh);
        }
        self.widenings += 1;
    }

    pub fn bytes(&self) -> usize {
        self.palette.len() + self.data.len() * 4
    }

    fn cell_hash(index: usize, material: Mat) -> u64 {
        if material == AIR {
            return 0;
        }
        let mut v = ((index as u64) << 16 | material as u64).wrapping_add(0x9e3779b97f4a7c15);
        v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
        v ^ (v >> 31)
    }
    pub fn content_hash(&self) -> u64 {
        self.content_hash
    }

    pub fn from_dense(d: &Dense) -> Paletted {
        let mut p = Paletted::new();
        for y in 0..CS {
            for z in 0..CS {
                for x in 0..CS {
                    let m = d.get(x, y, z);
                    if m != AIR {
                        p.set(x, y, z, m);
                    }
                }
            }
        }
        p.widenings = 0;
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::rng::Rng;

    #[inline(never)]
    fn copy_row_reference(p: &Paletted, x: usize, y: usize, z: usize, out: &mut [Mat]) {
        let bits = p.bits as usize;
        let per = 32 / bits;
        let mask = (1u32 << bits) - 1;
        let mut done = 0;
        let first = Dense::idx(x, y, z);
        while done < out.len() {
            let i = first + done;
            let count = (per - i % per).min(out.len() - done);
            let word = p.data[i / per] >> (i % per * bits);
            let index = word & mask;
            let used = u32::MAX >> (32 - count * bits);
            let same = word & used == (index * (u32::MAX / mask)) & used;
            if same {
                out[done..done + count].fill(p.palette[index as usize]);
            } else {
                for k in 0..count {
                    out[done + k] = p.palette[((word >> (k * bits)) & mask) as usize];
                }
            }
            done += count;
        }
    }

    #[test]
    fn packed_row_widths_match_dense_at_all_partial_words() {
        for palette_size in [2, 16, 17, 256, 257, 600] {
            let mut dense = Dense::new();
            for i in 0..CS * CS * CS {
                let mat = if i % 71 < 25 {
                    65535
                } else {
                    (i % palette_size) as Mat
                };
                dense.set(i % CS, i / (CS * CS), (i / CS) % CS, mat);
            }
            let p = Paletted::from_dense(&dense);
            for y in 0..CS {
                for z in [0, 1, 15, 31] {
                    for x in 0..=CS {
                        for len in 0..=CS - x {
                            let mut out = [12345; CS + 2];
                            p.copy_row(x, y, z, &mut out[1..1 + len]);
                            assert_eq!(out[0], 12345);
                            assert!(out[1 + len..].iter().all(|&m| m == 12345));
                            for i in 0..len {
                                assert_eq!(
                                    out[1 + i],
                                    dense.get(x + i, y, z),
                                    "palette={palette_size} x={x} y={y} z={z} len={len}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "manual packed-row timing, run alone in release"]
    fn measure_packed_rows() {
        use std::{hint::black_box, time::Instant};
        for palette_size in [8, 40, 600] {
            for mixed in [false, true] {
                let mut dense = Dense::new();
                for i in 0..CS * CS * CS {
                    let material = if mixed {
                        i % palette_size
                    } else {
                        i / CS % palette_size
                    };
                    dense.set(i % CS, i / (CS * CS), i / CS % CS, material as Mat);
                }
                let p = Paletted::from_dense(&dense);
                for width in [3, 9, 32] {
                    let mut times = [Vec::new(), Vec::new()];
                    let mut out = [0; CS];
                    for repeat in 0..9 {
                        for specialized in if repeat % 2 == 0 {
                            [false, true]
                        } else {
                            [true, false]
                        } {
                            let begin = Instant::now();
                            for i in 0..131072 {
                                let x = i % (CS + 1 - width);
                                let y = i / 32 % CS;
                                let z = i / 1024 % CS;
                                if specialized {
                                    black_box(&p).copy_row(x, y, z, black_box(&mut out[..width]));
                                } else {
                                    copy_row_reference(
                                        black_box(&p),
                                        x,
                                        y,
                                        z,
                                        black_box(&mut out[..width]),
                                    );
                                }
                                black_box(&out);
                            }
                            times[usize::from(specialized)]
                                .push(begin.elapsed().as_nanos() / 131072);
                        }
                    }
                    for (specialized, ns) in times.iter_mut().enumerate() {
                        ns.sort_unstable();
                        println!("PACKED palette={palette_size} bits={} mixed={mixed} width={width} specialized={specialized} median_ns={} min_ns={} max_ns={}", p.bits, ns[4], ns[0], ns[8]);
                    }
                }
            }
        }
    }
    #[test]
    fn packed_uniform_boxes_match_dense_with_every_index_width_and_partial_words() {
        for palette_size in [2, 17, 257, 600] {
            let mut d = Dense::new();
            d.v.fill(300);
            let mut p = Paletted::from_dense(&d);
            for i in 0..palette_size {
                let (x, y, z, mat) = (i % CS, 0, i / CS, 1000 + i as Mat);
                d.set(x, y, z, mat);
                p.set(x, y, z, mat);
            }
            for (q, mat) in [([0, 10, 11], 65535), ([31, 10, 11], 0), ([17; 3], 7)] {
                d.set(q[0], q[1], q[2], mat);
                p.set(q[0], q[1], q[2], mat);
            }
            assert_eq!(
                p.bits,
                if palette_size < 16 {
                    4
                } else if palette_size < 256 {
                    8
                } else {
                    16
                }
            );
            let boxes = [
                ([0; 3], [CS; 3]),
                ([1, 10, 11], [31, 11, 12]),
                ([0, 10, 11], [1, 11, 12]),
                ([31, 10, 11], [32, 11, 12]),
                ([1; 3], [17; 3]),
                ([1; 3], [18; 3]),
                ([17; 3], [18; 3]),
                ([18; 3], [32; 3]),
            ];
            for (min, max) in boxes {
                for mat in [0, 7, 300, 1000, 65535] {
                    let expected = (min[1]..max[1]).all(|y| {
                        (min[2]..max[2]).all(|z| (min[0]..max[0]).all(|x| d.get(x, y, z) == mat))
                    });
                    assert_eq!(
                        p.box_is_material(min, max, mat),
                        expected,
                        "palette={palette_size} min={min:?} max={max:?} mat={mat}"
                    );
                    let expected_with_air = (min[1]..max[1]).all(|y| {
                        (min[2]..max[2]).all(|z| {
                            (min[0]..max[0])
                                .all(|x| matches!(d.get(x, y, z), m if m == AIR || m == mat))
                        })
                    });
                    assert_eq!(p.box_is_material_or_air(min, max, mat), expected_with_air);
                }
            }
        }
    }

    #[test]
    fn packed_material_with_air_accepts_interleaved_holes_at_all_index_widths() {
        for entries in [1, 16, 256, 599] {
            let mut p = Paletted::new();
            for id in 1..=entries {
                p.set(0, 0, 0, id);
            }
            let mat = 65535;
            let mut d = Dense::new();
            for y in 0..CS {
                for z in 0..CS {
                    for x in 0..CS {
                        let m = if (x * 13 + y * 7 + z) % 5 < 2 {
                            AIR
                        } else {
                            mat
                        };
                        p.set(x, y, z, m);
                        d.set(x, y, z, m);
                    }
                }
            }
            for changed in [false, true] {
                if changed {
                    p.set(17, 13, 15, 1);
                    d.set(17, 13, 15, 1);
                }
                for x in 0..CS {
                    for width in [1, 2, 7, CS - x] {
                        let end = (x + width).min(CS);
                        for (lo, hi) in [([x, 0, 0], [end, CS, CS]), ([x, 13, 15], [end, 14, 16])] {
                            let expected = (lo[1]..hi[1]).all(|y| {
                                (lo[2]..hi[2]).all(|z| {
                                    (lo[0]..hi[0]).all(|x| [AIR, mat].contains(&d.get(x, y, z)))
                                })
                            });
                            assert_eq!(
                                p.box_is_material_or_air(lo, hi, mat),
                                expected,
                                "entries={entries} changed={changed} lo={lo:?} hi={hi:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    fn scene(seed: u64) -> Dense {
        let mut d = Dense::new();
        let mut r = Rng::new(seed, 1);
        for z in 0..CS {
            for x in 0..CS {
                let h = 12 + (r.below(8) as usize);
                for y in 0..h.min(CS) {
                    // Слои: дёрн, порода, руда вкраплениями.
                    let m = if y + 2 >= h {
                        1
                    } else if r.below(64) == 0 {
                        3
                    } else {
                        2
                    };
                    d.set(x, y, z, m);
                }
            }
        }
        d
    }

    /// Упакованный чанк обязан читаться так же, как плотный. Без этого
    /// сравнивать их по скорости бессмысленно.
    #[test]
    fn packed_reads_like_dense() {
        for seed in 0..4u64 {
            let d = scene(seed);
            let p = RowPacked::from_dense(&d);
            for y in 0..CS {
                for z in 0..CS {
                    for x in 0..CS {
                        assert_eq!(p.get(x, y, z), d.get(x, y, z), "({x},{y},{z}) seed={seed}");
                    }
                }
            }
        }
    }

    /// Запись переживает рост палитры и перепаковку.
    #[test]
    fn writes_survive_repacking() {
        let mut p = RowPacked::new();
        let mut d = Dense::new();
        let mut r = Rng::new(9, 9);
        for _ in 0..20_000 {
            let (x, y, z) = (
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
            );
            let m = r.below(6) as Mat;
            p.set(x, y, z, m);
            d.set(x, y, z, m);
        }
        for y in 0..CS {
            for z in 0..CS {
                for x in 0..CS {
                    assert_eq!(p.get(x, y, z), d.get(x, y, z), "({x},{y},{z})");
                }
            }
        }
        assert!(
            p.repacks > 0,
            "перепаковки не случилось — тест не проверил её"
        );
    }

    /// **Палитра на записи только растёт.** Строка, снова ставшая
    /// однородной, сама не сжимается: проверять это на каждую запись
    /// означало бы читать строку целиком. Сужает отдельный проход.
    #[test]
    fn palette_only_grows_until_compacted() {
        let mut p = RowPacked::new();
        for x in 0..CS {
            p.set(x, 5, 5, 2);
        }
        assert_eq!(
            p.uniform_rows(),
            ROWS - 1,
            "строка сжалась сама — значит запись делает лишнюю работу"
        );
        assert_eq!(p.compact(), 1);
        assert_eq!(p.uniform_rows(), ROWS, "проход уплотнения не сработал");
    }

    /// Палитра на чанк читается так же, как плотный массив, и переживает
    /// расширение индекса.
    #[test]
    fn paletted_matches_dense() {
        let mut r = fx::rng::Rng::new(12, 3);
        let mut d = Dense::new();
        let mut p = Paletted::new();
        // Больше шестнадцати материалов: расширение обязано сработать.
        for _ in 0..30_000 {
            let (x, y, z) = (
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
            );
            let m = r.below(30) as Mat;
            d.set(x, y, z, m);
            p.set(x, y, z, m);
        }
        assert!(
            p.widenings > 0,
            "расширение не случилось — тест его не проверил"
        );
        for y in 0..CS {
            for z in 0..CS {
                for x in 0..CS {
                    assert_eq!(p.get(x, y, z), d.get(x, y, z), "({x},{y},{z})");
                }
            }
        }
    }

    /// **Чанк держит больше 256 материалов** (ADR-446). `Mat` — шестнадцать
    /// бит, потому что вариантов больше 256 (ADR-094); индекс палитры
    /// останавливался на восьми.
    #[test]
    fn a_chunk_holds_more_than_256_materials() {
        let mut d = Dense::new();
        let mut p = Paletted::new();
        for i in 0..600usize {
            let (x, y, z) = (i % CS, (i / CS) % CS, i / (CS * CS));
            let m = (i as Mat % 300) + 1;
            d.set(x, y, z, m);
            p.set(x, y, z, m);
        }
        for y in 0..CS {
            for z in 0..CS {
                for x in 0..CS {
                    assert_eq!(p.get(x, y, z), d.get(x, y, z), "({x},{y},{z})");
                }
            }
        }
    }

    /// Уплотнение не меняет содержимого — только раскладку.
    #[test]
    fn compaction_preserves_content() {
        let mut r = fx::rng::Rng::new(4, 4);
        let mut d = Dense::new();
        for _ in 0..8000 {
            let (x, y, z) = (
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
            );
            d.set(x, y, z, r.below(5) as Mat);
        }
        let mut p = RowPacked::from_dense(&d);
        // Стираем половину: палитры остаются широкими.
        for _ in 0..8000 {
            let (x, y, z) = (
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
            );
            p.set(x, y, z, AIR);
            d.set(x, y, z, AIR);
        }
        let before = p.bytes();
        p.compact();
        for y in 0..CS {
            for z in 0..CS {
                for x in 0..CS {
                    assert_eq!(p.get(x, y, z), d.get(x, y, z), "({x},{y},{z})");
                }
            }
        }
        assert!(p.bytes() <= before, "уплотнение увеличило размер");
    }
}
