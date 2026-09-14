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

    fn widen(&mut self) {
        let old_bits = self.bits;
        let old = std::mem::take(&mut self.data);
        self.bits = 8;
        self.data = vec![0u32; CS * CS * CS / 4];
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
