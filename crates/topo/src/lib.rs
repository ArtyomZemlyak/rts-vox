//! Связность твёрдого: каскад фильтров ADR-055 в коде.
//!
//! Бюджет (`../../docs/10-budgets.md`) называет локальный пересчёт связности
//! внутри чанка слабым звеном сметы и требует мерить его первым: при 5 мкс
//! каскад стоит 0.1 мс, при 100 мкс — полмиллисекунды и доминирует.
//!
//! Чанк 32³ (ADR-024). Раскладка: колонка по X — один `u32`,
//! индексируется `z * 32 + y`. Тридцать два бита ровно в слово,
//! и это единственная причина, по которой чанк именно 32.

#![forbid(unsafe_code)]

pub mod scene;
pub mod stage_a;
pub mod stage_b;

/// Сторона чанка.
pub const CS: usize = 32;
/// Число колонок.
pub const COLS: usize = CS * CS;

/// Битовая занятость чанка: `cols[z * 32 + y]`, бит `x`.
#[derive(Clone, PartialEq, Eq)]
pub struct Chunk {
    pub cols: Vec<u32>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}

impl Chunk {
    pub fn new() -> Chunk {
        Chunk {
            cols: vec![0; COLS],
        }
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> bool {
        self.cols[z * CS + y] & (1u32 << x) != 0
    }

    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, z: usize, v: bool) {
        let i = z * CS + y;
        if v {
            self.cols[i] |= 1u32 << x;
        } else {
            self.cols[i] &= !(1u32 << x);
        }
    }

    pub fn solid_count(&self) -> u32 {
        self.cols.iter().map(|c| c.count_ones()).sum()
    }
}

// ---------------------------------------------------------------------------
// Локальные компоненты связности внутри чанка.
//
// По пробегам, а не по вокселям: колонка из тридцати двух бит распадается
// на считаные пробеги, и объединять надо их, а не 32 768 клеток.
// ---------------------------------------------------------------------------

/// Один пробег: сплошной отрезок твёрдого внутри колонки.
#[derive(Clone, Copy, Debug)]
pub struct Run {
    pub col: u16,
    /// Битовая маска пробега внутри колонки — пересечение проверяется одним И.
    pub mask: u32,
}

#[derive(Default)]
pub struct Components {
    pub runs: Vec<Run>,
    /// Union-find по пробегам.
    parent: Vec<u32>,
    /// Метка компоненты на пробег — **минимальный индекс пробега**,
    /// а не ранг: union by rank сделал бы метку зависимой от порядка слияний
    /// (ловушка, названная в ADR-055).
    pub label: Vec<u32>,
    pub count: usize,
}

impl Components {
    pub fn new() -> Components {
        Components::default()
    }

    fn clear(&mut self) {
        self.runs.clear();
        self.parent.clear();
        self.label.clear();
        self.count = 0;
    }

    #[inline(always)]
    fn find(&mut self, mut a: u32) -> u32 {
        while self.parent[a as usize] != a {
            // Сжатие пути в полшага: без рекурсии и без второго прохода.
            let g = self.parent[self.parent[a as usize] as usize];
            self.parent[a as usize] = g;
            a = g;
        }
        a
    }

    #[inline(always)]
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        // Корнем всегда становится меньший индекс. Это делает метку функцией
        // геометрии, а не порядка обхода.
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[hi as usize] = lo;
    }

    /// Разложить чанк на компоненты. Возвращает число компонент.
    pub fn build(&mut self, c: &Chunk) -> usize {
        self.clear();
        // Индекс первого пробега колонки, чтобы искать соседей без поиска.
        let mut col_start = vec![0u32; COLS + 1];

        for (ci, start) in col_start.iter_mut().enumerate().take(COLS) {
            *start = self.runs.len() as u32;
            let mut w = c.cols[ci];
            while w != 0 {
                let lo = w.trailing_zeros();
                let above = w >> lo;
                let len = above.trailing_ones();
                let mask = if len >= 32 {
                    u32::MAX
                } else {
                    ((1u32 << len) - 1) << lo
                };
                self.runs.push(Run {
                    col: ci as u16,
                    mask,
                });
                w &= !mask;
            }
        }
        col_start[COLS] = self.runs.len() as u32;

        self.parent = (0..self.runs.len() as u32).collect();

        // Соседние колонки: +Y и +Z. Каждая пара рассматривается один раз.
        for z in 0..CS {
            for y in 0..CS {
                let ci = z * CS + y;
                let (a0, a1) = (col_start[ci] as usize, col_start[ci + 1] as usize);
                if a0 == a1 {
                    continue;
                }
                for (dy, dz) in [(1usize, 0usize), (0, 1)] {
                    let (ny, nz) = (y + dy, z + dz);
                    if ny >= CS || nz >= CS {
                        continue;
                    }
                    let cj = nz * CS + ny;
                    let (b0, b1) = (col_start[cj] as usize, col_start[cj + 1] as usize);
                    if b0 == b1 {
                        continue;
                    }
                    for ai in a0..a1 {
                        let am = self.runs[ai].mask;
                        for bi in b0..b1 {
                            if am & self.runs[bi].mask != 0 {
                                self.union(ai as u32, bi as u32);
                            }
                        }
                    }
                }
            }
        }

        // Канонизация: метка — корень, корни перенумерованы по возрастанию.
        self.label = vec![u32::MAX; self.runs.len()];
        let mut next = 0u32;
        for i in 0..self.runs.len() {
            let r = self.find(i as u32);
            if self.label[r as usize] == u32::MAX {
                self.label[r as usize] = next;
                next += 1;
            }
            self.label[i] = self.label[r as usize];
        }
        self.count = next as usize;
        self.count
    }

    /// Компонента, содержащая воксель, или `None` если он пуст.
    pub fn component_at(&self, x: usize, y: usize, z: usize) -> Option<u32> {
        let ci = (z * CS + y) as u16;
        for (i, r) in self.runs.iter().enumerate() {
            if r.col == ci && r.mask & (1u32 << x) != 0 {
                return Some(self.label[i]);
            }
        }
        None
    }
}

/// Эталон: пометка компонент обходом по вокселям. Медленно и очевидно
/// правильно — существует только чтобы проверять быструю версию.
pub fn reference_labels(c: &Chunk) -> Vec<i32> {
    let mut lab = vec![-1i32; CS * CS * CS];
    let idx = |x: usize, y: usize, z: usize| (z * CS + y) * CS + x;
    let mut next = 0i32;
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    for z in 0..CS {
        for y in 0..CS {
            for x in 0..CS {
                if !c.get(x, y, z) || lab[idx(x, y, z)] >= 0 {
                    continue;
                }
                lab[idx(x, y, z)] = next;
                stack.push((x, y, z));
                while let Some((px, py, pz)) = stack.pop() {
                    let nbs = [
                        (px.wrapping_sub(1), py, pz),
                        (px + 1, py, pz),
                        (px, py.wrapping_sub(1), pz),
                        (px, py + 1, pz),
                        (px, py, pz.wrapping_sub(1)),
                        (px, py, pz + 1),
                    ];
                    for (nx, ny, nz) in nbs {
                        if nx >= CS || ny >= CS || nz >= CS {
                            continue;
                        }
                        if c.get(nx, ny, nz) && lab[idx(nx, ny, nz)] < 0 {
                            lab[idx(nx, ny, nz)] = next;
                            stack.push((nx, ny, nz));
                        }
                    }
                }
                next += 1;
            }
        }
    }
    lab
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{build, Scene};

    /// Быстрая пометка обязана давать **то же разбиение**, что обход
    /// по вокселям: те же клетки в одной компоненте, то же их число.
    #[test]
    fn matches_voxel_reference() {
        let mut comp = Components::new();
        for scene in Scene::ALL {
            for seed in 0..4u64 {
                let c = build(scene, seed);
                let n = comp.build(&c);
                let reference = reference_labels(&c);
                let ref_count = reference.iter().copied().filter(|&v| v >= 0).max().map_or(0, |m| m + 1) as usize;
                assert_eq!(
                    n,
                    ref_count,
                    "{} seed={seed}: {n} компонент против {ref_count}",
                    scene.name()
                );
                // Разбиение обязано совпасть: биекция меток.
                let mut map = vec![u32::MAX; ref_count.max(1)];
                for z in 0..CS {
                    for y in 0..CS {
                        for x in 0..CS {
                            if !c.get(x, y, z) {
                                continue;
                            }
                            let r = reference[(z * CS + y) * CS + x] as usize;
                            let f = comp.component_at(x, y, z).expect("твёрдый без компоненты");
                            if map[r] == u32::MAX {
                                map[r] = f;
                            } else {
                                assert_eq!(map[r], f, "{} ({x},{y},{z})", scene.name());
                            }
                        }
                    }
                }
            }
        }
    }

    /// Метка обязана быть функцией геометрии, а не порядка слияний:
    /// union by rank сделал бы её зависимой (ловушка ADR-055).
    #[test]
    fn labels_are_canonical() {
        let mut a = Components::new();
        let mut b = Components::new();
        for scene in Scene::ALL {
            let c = build(scene, 5);
            a.build(&c);
            // Второй проход на тех же буферах и на копии чанка.
            let c2 = c.clone();
            b.build(&c2);
            assert_eq!(a.label, b.label, "{}", scene.name());
        }
    }

    #[test]
    fn empty_and_full_chunks() {
        let mut comp = Components::new();
        assert_eq!(comp.build(&Chunk::new()), 0);
        let mut full = Chunk::new();
        full.cols.iter_mut().for_each(|c| *c = u32::MAX);
        assert_eq!(comp.build(&full), 1);
    }

    /// Две несвязанные плиты — ровно две компоненты.
    #[test]
    fn two_slabs_are_two_components() {
        let mut c = Chunk::new();
        for z in 0..CS {
            for y in 0..CS {
                c.cols[z * CS + y] = 0b11 | (0b11 << 10);
            }
        }
        let mut comp = Components::new();
        assert_eq!(comp.build(&c), 2);
    }
}
