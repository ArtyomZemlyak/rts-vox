//! Двоичный греди-мешинг чанка и сцены под нашу механику.
//!
//! Крейт существует ради замера, которого требует `../../docs/10-budgets.md`:
//! «число для рваной каверны (16 421) воспроизвести нельзя, потому что сцена
//! не задана». Здесь сцена задана кодом.
//!
//! Раскладка: чанк 62³, паддинг до 64³, потому что колонка ровно `u64`.
//! Паддинг обязателен — без соседей грани на границе чанка считаются неверно.
//! Сборка паддинга и построение масок **внутри таймера**: в опубликованных
//! 65 мкс их не было, и отсюда половина расхождения.

#![forbid(unsafe_code)]

pub mod bits;
pub mod incr;
pub mod layer;
pub mod scene;

/// Сторона чанка без паддинга.
pub const CS: usize = 62;
/// Сторона с паддингом.
pub const CSP: usize = 64;
/// Сколько индексов материала поддерживает мешер.
pub const MAX_MAT: usize = 8;

/// Индекс материала. `0` — пусто.
pub type Mat = u8;
pub const AIR: Mat = 0;

/// Чанк с паддингом. Раскладка `[y][z][x]` — колонки по X лежат подряд.
#[derive(Clone)]
pub struct Chunk {
    pub v: Vec<Mat>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}

impl Chunk {
    pub fn new() -> Chunk {
        Chunk {
            v: vec![AIR; CSP * CSP * CSP],
        }
    }

    #[inline(always)]
    pub fn idx(x: usize, y: usize, z: usize) -> usize {
        (y * CSP + z) * CSP + x
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Mat {
        self.v[Self::idx(x, y, z)]
    }

    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, z: usize, m: Mat) {
        self.v[Self::idx(x, y, z)] = m;
    }

    pub fn solid_count(&self) -> usize {
        let mut n = 0;
        for y in 1..=CS {
            for z in 1..=CS {
                for x in 1..=CS {
                    if self.get(x, y, z) != AIR {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    pub fn materials_present(&self) -> Vec<Mat> {
        let mut seen = [false; 256];
        for &m in &self.v {
            seen[m as usize] = true;
        }
        (1..=255u16)
            .filter(|&m| seen[m as usize])
            .map(|m| m as Mat)
            .collect()
    }
}

/// Квад. Держим поля, чтобы нельзя было случайно смерить пустой цикл.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quad {
    pub face: u8,
    pub slice: u8,
    pub a: u8,
    pub b: u8,
    pub w: u8,
    pub h: u8,
    pub mat: Mat,
}

const FACES: usize = 6;
const PLANE_LEN: usize = FACES * CSP * CSP;

/// Переиспользуемые буферы мешера. Аллокации внутри таймера — это замер
/// аллокатора, и на чанке они сравнимы с полезной работой.
pub struct Mesher {
    cols: Vec<u64>,
    faces: Vec<u64>,
    planes: Vec<u64>,
    pub quads: Vec<Quad>,
}

impl Default for Mesher {
    fn default() -> Self {
        Self::new()
    }
}

/// Координаты вокселя, которому принадлежит грань.
#[inline(always)]
fn owner(axis: usize, s: usize, a: usize, b: usize) -> (usize, usize, usize) {
    match axis {
        0 => (s, b, a), // колонка по X индексирована (z, y)
        1 => (a, s, b), // по Y — (x, z)
        _ => (b, a, s), // по Z — (y, x)
    }
}

impl Mesher {
    pub fn new() -> Mesher {
        Mesher {
            cols: vec![0; 3 * CSP * CSP],
            faces: vec![0; FACES * CSP * CSP],
            planes: vec![0; PLANE_LEN],
            quads: Vec::with_capacity(1 << 16),
        }
    }

    /// Обычный путь: маски строятся обходом массива материалов.
    pub fn mesh(&mut self, c: &Chunk, opt: Opts) -> Stats {
        self.cols.iter_mut().for_each(|v| *v = 0);
        for y in 0..CSP {
            for z in 0..CSP {
                let row = (y * CSP + z) * CSP;
                for x in 0..CSP {
                    if c.v[row + x] == AIR {
                        continue;
                    }
                    self.cols[z * CSP + y] |= 1u64 << x;
                    self.cols[CSP * CSP + x * CSP + z] |= 1u64 << y;
                    self.cols[2 * CSP * CSP + y * CSP + x] |= 1u64 << z;
                }
            }
        }
        self.finish(c, opt)
    }

    /// Быстрый путь: осевые маски раскладываются из готового битового объёма
    /// (ADR-061) транспозициями, без обхода байтов.
    pub fn mesh_from_bits(&mut self, bits: &bits::BitVolume, c: &Chunk, opt: Opts) -> Stats {
        bits::expand_axes(bits, &mut self.cols);
        self.finish(c, opt)
    }

    fn finish(&mut self, c: &Chunk, opt: Opts) -> Stats {
        self.quads.clear();
        self.faces.iter_mut().for_each(|v| *v = 0);

        // 2. Отсев скрытых граней — две инструкции на колонку.
        //    Это и есть вся суть двоичного мешинга.
        for axis in 0..3 {
            for k in 0..CSP * CSP {
                let col = self.cols[axis * CSP * CSP + k];
                if col == 0 {
                    continue;
                }
                self.faces[(axis * 2) * CSP * CSP + k] = col & !(col >> 1);
                self.faces[(axis * 2 + 1) * CSP * CSP + k] = col & !(col << 1);
            }
        }

        // 3. Слияние. По материалу за проход: слить грани разных материалов
        //    нельзя, а рисовать их одним квадом — тем более.
        let mats: Vec<Mat> = if opt.per_material {
            c.materials_present()
        } else {
            vec![1]
        };
        let mut total = 0usize;
        for &m in &mats {
            self.planes.iter_mut().for_each(|v| *v = 0);
            for axis in 0..3 {
                for dir in 0..2 {
                    let face = axis * 2 + dir;
                    if opt.skip_bottom && face == 3 {
                        continue; // −Y: камера не видит их никогда
                    }
                    for k in 0..CSP * CSP {
                        let mut bits = self.faces[face * CSP * CSP + k];
                        if bits == 0 {
                            continue;
                        }
                        let (a, b) = (k / CSP, k % CSP);
                        if a == 0 || a > CS || b == 0 || b > CS {
                            continue;
                        }
                        // Обрезаем паддинг по оси слоя одним И.
                        bits &= SLICE_MASK;
                        while bits != 0 {
                            let s = bits.trailing_zeros() as usize;
                            bits &= bits - 1;
                            if opt.per_material {
                                let (x, y, z) = owner(axis, s, a, b);
                                if c.v[Chunk::idx(x, y, z)] != m {
                                    continue;
                                }
                            }
                            self.planes[(face * CSP + s) * CSP + a] |= 1u64 << b;
                        }
                    }
                }
            }
            total += self.merge(m, opt);
        }

        Stats {
            quads: total,
            bytes: total * 8,
            materials: mats.len(),
        }
    }

    fn merge(&mut self, m: Mat, opt: Opts) -> usize {
        let mut merged = 0usize;
        for face in 0..FACES {
            for s in 1..=CS {
                let base = (face * CSP + s) * CSP;
                for row in 1..=CS {
                    while self.planes[base + row] != 0 {
                        let bit = self.planes[base + row].trailing_zeros() as usize;
                        let run = (self.planes[base + row] >> bit).trailing_ones() as usize;
                        let h = run.min(opt.max_run);
                        let mask = ((1u64 << h) - 1) << bit;
                        let mut w = 1usize;
                        while row + w <= CS
                            && w < opt.max_run
                            && (self.planes[base + row + w] & mask) == mask
                        {
                            self.planes[base + row + w] &= !mask;
                            w += 1;
                        }
                        self.planes[base + row] &= !mask;
                        merged += 1;
                        self.quads.push(Quad {
                            face: face as u8,
                            slice: s as u8,
                            a: row as u8,
                            b: bit as u8,
                            w: w as u8,
                            h: h as u8,
                            mat: m,
                        });
                    }
                }
            }
        }
        merged
    }
}

/// Биты 1..=62: паддинг по оси слоя.
const SLICE_MASK: u64 = ((1u64 << CS) - 1) << 1;

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// Не выпускать нижние грани по `−Y`.
    pub skip_bottom: bool,
    /// Потолок длины греди-пробега.
    pub max_run: usize,
    /// Слияние только внутри одного материала.
    pub per_material: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            skip_bottom: false,
            max_run: CS,
            per_material: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub quads: usize,
    pub bytes: usize,
    pub materials: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> Chunk {
        let mut c = Chunk::new();
        for y in 1..=CS {
            for z in 1..=CS {
                for x in 1..=CS {
                    if (x + y + z) % 2 == 0 {
                        c.set(x, y, z, 1);
                    }
                }
            }
        }
        c
    }

    /// Шахматка — абсолютный потолок: слить нельзя ничего, у каждого твёрдого
    /// вокселя все шесть граней открыты. Число известно точно и совпадает
    /// с записанным в `10-budgets.md`.
    #[test]
    fn checkerboard_is_the_known_ceiling() {
        let c = checker();
        let solid = c.solid_count();
        let mut m = Mesher::new();
        let st = m.mesh(&c, Opts::default());
        assert_eq!(st.quads, solid * 6);
        assert_eq!(st.quads, 714_984, "известное значение потолка");
    }

    #[test]
    fn full_chunk_is_six_quads() {
        let mut c = Chunk::new();
        for y in 1..=CS {
            for z in 1..=CS {
                for x in 1..=CS {
                    c.set(x, y, z, 1);
                }
            }
        }
        let mut m = Mesher::new();
        assert_eq!(m.mesh(&c, Opts::default()).quads, 6);
    }

    #[test]
    fn single_voxel_is_six_quads() {
        let mut c = Chunk::new();
        c.set(10, 10, 10, 1);
        let mut m = Mesher::new();
        assert_eq!(m.mesh(&c, Opts::default()).quads, 6);
    }

    /// Плита в один воксель толщиной: по квадру на каждую из шести сторон.
    #[test]
    fn flat_slab_is_six_quads() {
        let mut c = Chunk::new();
        for z in 1..=CS {
            for x in 1..=CS {
                c.set(x, 5, z, 1);
            }
        }
        let mut m = Mesher::new();
        assert_eq!(m.mesh(&c, Opts::default()).quads, 6);
    }

    /// Число квадов не должно зависеть от того, где стоит куб: раскладка
    /// осей у мешера несимметрична, и перепутанная ось вылезет именно так.
    #[test]
    fn axes_are_symmetric() {
        let mut m = Mesher::new();
        for (dx, dy, dz) in [(20usize, 3, 3), (3, 20, 3), (3, 3, 20)] {
            let mut c = Chunk::new();
            for y in 5..5 + dy {
                for z in 5..5 + dz {
                    for x in 5..5 + dx {
                        c.set(x, y, z, 1);
                    }
                }
            }
            assert_eq!(m.mesh(&c, Opts::default()).quads, 6, "брус {dx}x{dy}x{dz}");
        }
    }

    #[test]
    fn skip_bottom_removes_exactly_one_sixth_on_checkerboard() {
        let c = checker();
        let mut m = Mesher::new();
        let all = m.mesh(&c, Opts::default()).quads;
        let cut = m
            .mesh(
                &c,
                Opts {
                    skip_bottom: true,
                    ..Opts::default()
                },
            )
            .quads;
        assert_eq!(all - cut, all / 6);
    }

    /// Клэмп пробега не может уменьшить число квадов и не может увеличить
    /// его больше чем вчетверо на осевой геометрии.
    #[test]
    fn run_clamp_only_grows() {
        let c = crate::scene::build(crate::scene::Scene::Cavern, 42);
        let mut m = Mesher::new();
        let free = m.mesh(&c, Opts::default()).quads;
        for r in [32usize, 16, 8] {
            let clamped = m
                .mesh(
                    &c,
                    Opts {
                        max_run: r,
                        ..Opts::default()
                    },
                )
                .quads;
            assert!(clamped >= free, "клэмп {r} уменьшил число квадов");
        }
    }

    /// Разделение по материалам не может уменьшить число квадов.
    #[test]
    fn per_material_only_grows() {
        let c = crate::scene::build(crate::scene::Scene::Layered, 42);
        let mut m = Mesher::new();
        let one = m.mesh(&c, Opts::default()).quads;
        let many = m
            .mesh(
                &c,
                Opts {
                    per_material: true,
                    ..Opts::default()
                },
            )
            .quads;
        assert!(many >= one, "{many} < {one}");
    }

    /// Быстрый путь обязан давать **побитово то же самое**, иначе им нельзя
    /// пользоваться: расхождение мешера и битового объёма это свет сквозь
    /// стену, которой в меше ещё есть, а в объёме уже нет.
    #[test]
    fn from_bits_matches_byte_scan() {
        let mut m = Mesher::new();
        for scene in crate::scene::Scene::ALL {
            for &opt in &[
                Opts::default(),
                Opts { skip_bottom: true, ..Opts::default() },
                Opts { max_run: 16, ..Opts::default() },
                Opts { per_material: true, ..Opts::default() },
            ] {
                let c = crate::scene::build(scene, 7);
                let slow = m.mesh(&c, opt).quads;
                let mut qs = m.quads.clone();
                let bits = crate::bits::bits_from_chunk(&c);
                let fast = m.mesh_from_bits(&bits, &c, opt).quads;
                assert_eq!(slow, fast, "{} {:?}", scene.name(), opt);
                qs.sort_by_key(|q| (q.face, q.slice, q.a, q.b));
                let mut qf = m.quads.clone();
                qf.sort_by_key(|q| (q.face, q.slice, q.a, q.b));
                assert_eq!(qs, qf, "{} {:?}: квады разошлись", scene.name(), opt);
            }
        }
    }

    /// Мешер — чистая функция от чанка: повторные прогоны на тех же буферах
    /// обязаны совпадать.
    #[test]
    fn mesher_is_reusable() {
        let c = crate::scene::build(crate::scene::Scene::Cavern, 42);
        let mut m = Mesher::new();
        let a = m.mesh(&c, Opts::default()).quads;
        let b = m.mesh(&c, Opts::default()).quads;
        let d = m.mesh(&c, Opts::default()).quads;
        assert_eq!((a, b), (b, d));
    }
}
