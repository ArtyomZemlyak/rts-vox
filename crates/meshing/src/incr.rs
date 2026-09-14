//! Инкрементальный ремеш: пересобираются только грязные слои плоскостей.
//!
//! **Свойство, которое это делает возможным:** жадное слияние внутри одного
//! слоя плоскости граней не зависит ни от какого другого слоя. Квады слоя
//! `s` грани `f` — функция только от `planes[f][s][*]`. Значит слой можно
//! пересобрать точно, а не приблизительно, и результат обязан совпадать
//! с полной пересборкой побитово.
//!
//! Правка одного воксела трогает три колонки (по одной на ось), в каждой
//! меняет **около двух бит** маски граней, и потому пачкает порядка
//! двенадцати пар «грань, слой» из трёхсот семидесяти двух.
//!
//! Ограничение: разделение по материалам здесь не поддержано. Замер
//! (`12-meshing-bench.md`) показал, что оно стоит +1.0% квадов в худшем
//! случае, но инкрементальная форма для него сложнее, а выигрыш тот же.

use crate::bits::{expand_axes, BitVolume};
use crate::{Opts, Quad, CS, CSP};

const FACES: usize = 6;

/// Мешер с состоянием: держит маски и квады по слоям между вызовами.
pub struct IncMesher {
    cols: Vec<u64>,
    faces: Vec<u64>,
    planes: Vec<u64>,
    /// Квады, разложенные по паре «грань, слой»: `slice_quads[face * CSP + s]`.
    slice_quads: Vec<Vec<Quad>>,
    /// Грязные пары «грань, слой», битсетом.
    dirty: Vec<u64>,
    /// Штампы для дедупликации затронутых колонок без `HashSet`.
    col_stamp: Vec<u32>,
    epoch: u32,
    touched_cols: Vec<u32>,
    /// Сколько пар «грань, слой» пересобрано последним вызовом.
    pub last_dirty_slices: usize,
    /// Сколько колонок пересчитано последним вызовом.
    pub last_touched_cols: usize,
}

impl Default for IncMesher {
    fn default() -> Self {
        Self::new()
    }
}

impl IncMesher {
    pub fn new() -> IncMesher {
        IncMesher {
            cols: vec![0; 3 * CSP * CSP],
            faces: vec![0; FACES * CSP * CSP],
            planes: vec![0; FACES * CSP * CSP],
            slice_quads: (0..FACES * CSP).map(|_| Vec::with_capacity(64)).collect(),
            dirty: vec![0; FACES * CSP / 64 + 1],
            col_stamp: vec![0; 3 * CSP * CSP],
            epoch: 0,
            touched_cols: Vec::with_capacity(4096),
            last_dirty_slices: 0,
            last_touched_cols: 0,
        }
    }

    /// Сколько байт состояния держит мешер на один чанк. Это цена
    /// инкрементальности: без резидентных масок диффать нечего.
    pub fn state_bytes(&self) -> usize {
        self.cols.len() * 8
            + self.faces.len() * 8
            + self.planes.len() * 8
            + self.col_stamp.len() * 4
            + self.dirty.len() * 8
            + self
                .slice_quads
                .iter()
                .map(|v| v.capacity() * 8)
                .sum::<usize>()
    }

    pub fn quad_count(&self) -> usize {
        self.slice_quads.iter().map(|v| v.len()).sum()
    }

    pub fn collect_quads(&self) -> Vec<Quad> {
        let mut out = Vec::with_capacity(self.quad_count());
        self.append_quads(&mut out);
        out
    }

    /// Дописать квады в чужой вектор.
    ///
    /// То же, что `collect_quads`, но без выделения: потоковая сборка
    /// зовёт это тысячи раз за кадр, и новый вектор на каждый чанк
    /// мерил бы аллокатор.
    pub fn append_quads(&self, out: &mut Vec<Quad>) {
        out.reserve(self.quad_count());
        for v in self.slice_quads.iter() {
            out.extend_from_slice(v);
        }
    }

    #[inline(always)]
    fn mark_dirty(&mut self, face: usize, s: usize) {
        let i = face * CSP + s;
        self.dirty[i >> 6] |= 1u64 << (i & 63);
    }

    /// Полная пересборка. Делается один раз на чанк, дальше — правки.
    pub fn rebuild_all(&mut self, bits: &BitVolume, opt: Opts) {
        expand_axes(bits, &mut self.cols);
        self.faces.iter_mut().for_each(|v| *v = 0);
        self.planes.iter_mut().for_each(|v| *v = 0);
        for v in self.slice_quads.iter_mut() {
            v.clear();
        }

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
        for axis in 0..3 {
            for dir in 0..2 {
                let face = axis * 2 + dir;
                if opt.skip_bottom && face == 3 {
                    continue;
                }
                for k in 0..CSP * CSP {
                    let (a, b) = (k / CSP, k % CSP);
                    if a == 0 || a > CS || b == 0 || b > CS {
                        continue;
                    }
                    let mut w = self.faces[face * CSP * CSP + k] & SLICE_MASK;
                    while w != 0 {
                        let s = w.trailing_zeros() as usize;
                        w &= w - 1;
                        self.planes[(face * CSP + s) * CSP + a] |= 1u64 << b;
                    }
                }
            }
        }
        self.last_dirty_slices = 0;
        for face in 0..FACES {
            if opt.skip_bottom && face == 3 {
                continue;
            }
            for s in 1..=CS {
                self.merge_slice(face, s, opt);
                self.last_dirty_slices += 1;
            }
        }
        self.last_touched_cols = 3 * CSP * CSP;
    }

    /// Применить правки: пересчитать только затронутые колонки и пересобрать
    /// только грязные слои.
    ///
    /// `bits` — объём **после** правок; `edits` — координаты изменённых
    /// вокселей внутри чанка, в системе с паддингом (1..=CS).
    pub fn apply_edits(&mut self, bits: &BitVolume, edits: &[(u8, u8, u8)], opt: Opts) {
        self.dirty.iter_mut().for_each(|v| *v = 0);
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.col_stamp.iter_mut().for_each(|v| *v = 0);
            self.epoch = 1;
        }
        self.touched_cols.clear();

        // Правка воксела меняет по одному **биту** в каждой из трёх осевых
        // колонок. Пересобирать колонку из объёма незачем — достаточно
        // перевернуть бит; состояние берётся из объёма одним чтением.
        for &(x, y, z) in edits {
            let (x, y, z) = (x as usize, y as usize, z as usize);
            let solid = bits[y * CSP + z] & (1u64 << x) != 0;
            for (axis, k, bit) in [
                (0usize, z * CSP + y, x),
                (1, x * CSP + z, y),
                (2, y * CSP + x, z),
            ] {
                let gi = axis * CSP * CSP + k;
                if solid {
                    self.cols[gi] |= 1u64 << bit;
                } else {
                    self.cols[gi] &= !(1u64 << bit);
                }
                if self.col_stamp[gi] != self.epoch {
                    self.col_stamp[gi] = self.epoch;
                    self.touched_cols.push(gi as u32);
                }
            }
        }
        self.last_touched_cols = self.touched_cols.len();

        for i in 0..self.touched_cols.len() {
            let gi = self.touched_cols[i] as usize;
            let axis = gi / (CSP * CSP);
            let k = gi % (CSP * CSP);
            let new_col = self.cols[gi];
            let (a, b) = (k / CSP, k % CSP);
            let inside = a > 0 && a <= CS && b > 0 && b <= CS;
            for dir in 0..2 {
                let face = axis * 2 + dir;
                let new_face = if dir == 0 {
                    new_col & !(new_col >> 1)
                } else {
                    new_col & !(new_col << 1)
                };
                let fi = face * CSP * CSP + k;
                let changed = (self.faces[fi] ^ new_face) & SLICE_MASK;
                self.faces[fi] = new_face;
                if !inside || changed == 0 {
                    continue;
                }
                if opt.skip_bottom && face == 3 {
                    continue;
                }
                let mut w = changed;
                while w != 0 {
                    let sl = w.trailing_zeros() as usize;
                    w &= w - 1;
                    let pi = (face * CSP + sl) * CSP + a;
                    if new_face & (1u64 << sl) != 0 {
                        self.planes[pi] |= 1u64 << b;
                    } else {
                        self.planes[pi] &= !(1u64 << b);
                    }
                    self.mark_dirty(face, sl);
                }
            }
        }

        self.last_dirty_slices = 0;
        for w in 0..self.dirty.len() {
            let mut bitsw = self.dirty[w];
            while bitsw != 0 {
                let b = bitsw.trailing_zeros() as usize;
                bitsw &= bitsw - 1;
                let i = w * 64 + b;
                let (face, s) = (i / CSP, i % CSP);
                if s == 0 || s > CS {
                    continue;
                }
                self.merge_slice(face, s, opt);
                self.last_dirty_slices += 1;
            }
        }
    }

    /// Пересобрать один слой одной грани. Плоскость слоя расходуется:
    /// её приходится восстановить, потому что слияние снимает биты.
    fn merge_slice(&mut self, face: usize, s: usize, opt: Opts) {
        let out = &mut self.slice_quads[face * CSP + s];
        out.clear();
        let base = (face * CSP + s) * CSP;
        // Копия плоскости: слияние снимает биты, а хранить их надо.
        let mut work = [0u64; CSP];
        work[1..=CS].copy_from_slice(&self.planes[base + 1..=base + CS]);
        for row in 1..=CS {
            while work[row] != 0 {
                let bit = work[row].trailing_zeros() as usize;
                let run = (work[row] >> bit).trailing_ones() as usize;
                let h = run.min(opt.max_run);
                let mask = ((1u64 << h) - 1) << bit;
                let mut w = 1usize;
                while row + w <= CS && w < opt.max_run && (work[row + w] & mask) == mask {
                    work[row + w] &= !mask;
                    w += 1;
                }
                work[row] &= !mask;
                out.push(Quad {
                    face: face as u8,
                    slice: s as u8,
                    a: row as u8,
                    b: bit as u8,
                    w: w as u8,
                    h: h as u8,
                    mat: 1,
                });
            }
        }
    }
}

const SLICE_MASK: u64 = ((1u64 << CS) - 1) << 1;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::bits_from_chunk;
    use crate::scene::{build, Scene};
    use crate::{Chunk, Mesher, AIR};

    fn sorted(mut q: Vec<Quad>) -> Vec<Quad> {
        q.sort_by_key(|x| (x.face, x.slice, x.a, x.b, x.w, x.h));
        q
    }

    /// **Главное свойство.** Инкрементальный ремеш обязан давать **тот же
    /// набор квадов**, что полная пересборка, — не «примерно столько же».
    /// Иначе им нельзя пользоваться: расхождение это дыра в геометрии.
    #[test]
    fn incremental_equals_full_rebuild() {
        let mut r = fx::rng::Rng::new(17, 4);
        let mut inc = IncMesher::new();
        let mut full = Mesher::new();
        for scene in Scene::ALL {
            for burst in [1usize, 8, 60, 300] {
                let mut c = build(scene, 11);
                let mut b = bits_from_chunk(&c);
                inc.rebuild_all(&b, Opts::default());

                // Пачка правок, кучкой — как копает юнит.
                let (cx, cy, cz) = (
                    r.below(CS as u32) as usize + 1,
                    r.below(CS as u32) as usize + 1,
                    r.below(CS as u32) as usize + 1,
                );
                let mut edits = Vec::new();
                for _ in 0..burst {
                    let x = (cx + r.below(5) as usize).clamp(1, CS);
                    let y = (cy + r.below(5) as usize).clamp(1, CS);
                    let z = (cz + r.below(5) as usize).clamp(1, CS);
                    let want = r.below(4) == 0;
                    let cur = c.get(x, y, z) != AIR;
                    if cur == want {
                        continue;
                    }
                    c.set(x, y, z, if want { 1 } else { AIR });
                    edits.push((x as u8, y as u8, z as u8));
                }
                b = bits_from_chunk(&c);
                inc.apply_edits(&b, &edits, Opts::default());

                full.mesh(&c, Opts::default());
                assert_eq!(
                    sorted(inc.collect_quads()),
                    sorted(full.quads.clone()),
                    "{} burst={burst}: инкремент разошёлся с полной пересборкой",
                    scene.name()
                );
            }
        }
    }

    /// Правки, применённые по одной, обязаны давать то же, что применённые
    /// пачкой: очередь правок батчится за тик, и порядок не должен быть виден.
    #[test]
    fn batching_edits_is_invisible() {
        let mut r = fx::rng::Rng::new(23, 9);
        let mut a = IncMesher::new();
        let mut b_inc = IncMesher::new();
        for scene in [Scene::Cavern, Scene::CavernAndScatter, Scene::Smooth] {
            let mut c = build(scene, 5);
            let bits0 = bits_from_chunk(&c);
            a.rebuild_all(&bits0, Opts::default());
            b_inc.rebuild_all(&bits0, Opts::default());

            let mut edits = Vec::new();
            for _ in 0..40 {
                let (x, y, z) = (
                    r.below(CS as u32) as usize + 1,
                    r.below(CS as u32) as usize + 1,
                    r.below(CS as u32) as usize + 1,
                );
                if c.get(x, y, z) == AIR {
                    continue;
                }
                c.set(x, y, z, AIR);
                edits.push((x as u8, y as u8, z as u8));
            }
            let bits1 = bits_from_chunk(&c);
            a.apply_edits(&bits1, &edits, Opts::default());

            // По одной: состояние объёма надо давать промежуточное.
            let mut c2 = build(scene, 5);
            for &(x, y, z) in edits.iter() {
                c2.set(x as usize, y as usize, z as usize, AIR);
                let bi = bits_from_chunk(&c2);
                b_inc.apply_edits(&bi, &[(x, y, z)], Opts::default());
            }
            assert_eq!(
                sorted(a.collect_quads()),
                sorted(b_inc.collect_quads()),
                "{}: батч и поштучно разошлись",
                scene.name()
            );
        }
    }

    /// Пустой список правок ничего не меняет и ничего не пересобирает.
    #[test]
    fn no_edits_is_a_noop() {
        let c = build(Scene::Cavern, 3);
        let bits = bits_from_chunk(&c);
        let mut inc = IncMesher::new();
        inc.rebuild_all(&bits, Opts::default());
        let before = sorted(inc.collect_quads());
        inc.apply_edits(&bits, &[], Opts::default());
        assert_eq!(inc.last_dirty_slices, 0);
        assert_eq!(sorted(inc.collect_quads()), before);
    }

    /// Правка в пустоте, ничего не меняющая, не должна пачкать слои.
    #[test]
    fn touching_air_dirties_nothing() {
        let mut c = Chunk::new();
        for y in 1..=10 {
            for z in 1..=CS {
                for x in 1..=CS {
                    c.set(x, y, z, 1);
                }
            }
        }
        let bits = bits_from_chunk(&c);
        let mut inc = IncMesher::new();
        inc.rebuild_all(&bits, Opts::default());
        // Воксель высоко над поверхностью: был пуст, остался пуст.
        inc.apply_edits(&bits, &[(20, 40, 20)], Opts::default());
        assert_eq!(inc.last_dirty_slices, 0, "пустая правка испачкала слои");
    }
}
