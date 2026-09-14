//! Связность твёрдого: каскад фильтров ADR-055 в коде.
//!
//! Бюджет (`../../docs/10-budgets.md`) называет локальный пересчёт связности
//! внутри чанка слабым звеном сметы и требует мерить его первым: при 5 мкс
//! каскад стоит 0.1 мс, при 100 мкс — полмиллисекунды и доминирует.
//!
//! Чанк 32³ (ADR-024). Раскладка: колонка по X — один `u32`,
//! индексируется `z * 32 + y`. Тридцать два бита ровно в слово,
//! и это единственная причина, по которой чанк именно 32.

pub mod components;
pub mod layer;
pub mod scene;
pub mod stage_a;
pub mod stage_b;
pub mod unit;
pub mod unit_scene;

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
    /// Индекс первого пробега колонки. Переиспользуется: разметка
    /// зовётся десятками раз за тик, и четыре килобайта на вызов —
    /// это выделение с обнулением на каждый чанк (ADR-129).
    col_start: Vec<u32>,
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

    /// Разложить на компоненты **пустоту**, а не твёрдое (ADR-057).
    ///
    /// Второй граф той же конструкции: инвертированные колонки. Вода живёт
    /// в компонентах пустоты, и осушение делает этот граф декрементальным —
    /// компоненты только дробятся.
    pub fn build_void(&mut self, c: &Chunk) -> usize {
        let inverted = Chunk {
            cols: c.cols.iter().map(|w| !w).collect(),
        };
        self.build(&inverted)
    }

    /// Разложить чанк на компоненты. Возвращает число компонент.
    pub fn build(&mut self, c: &Chunk) -> usize {
        self.clear();
        // Индекс первого пробега колонки, чтобы искать соседей без поиска.
        let mut col_start = std::mem::take(&mut self.col_start);
        col_start.clear();
        col_start.resize(COLS + 1, 0);

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

        self.parent.clear();
        self.parent.extend(0..self.runs.len() as u32);

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
        self.label.clear();
        self.label.resize(self.runs.len(), u32::MAX);
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
        self.col_start = col_start;
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

/// Потолок объёма окрестности для локальной проверки. Больше — дешевле
/// разметить чанк заново.
pub const SPLIT_PROBE_CAP: usize = 4096;

/// Буферы локальной проверки. Переиспользуются: она зовётся десятками
/// раз за тик.
pub struct SplitBufs {
    stamp: Vec<u32>,
    epoch: u32,
    shell: Vec<u32>,
    queue: Vec<u32>,
}

impl Default for SplitBufs {
    fn default() -> Self {
        Self::new()
    }
}

impl SplitBufs {
    pub fn new() -> SplitBufs {
        SplitBufs {
            stamp: vec![0; CS * CS * CS],
            epoch: 0,
            shell: Vec::new(),
            queue: Vec::new(),
        }
    }
}

#[inline]
fn cell_index(x: usize, y: usize, z: usize) -> usize {
    (z * CS + y) * CS + x
}

/// **Достаточное условие «эта пачка снятий ничего не расщепила».**
///
/// `c` — чанк уже **после** снятий, `removed` — что сняли.
///
/// Ступень A (ADR-055) отвечает на тот же вопрос про **одно** снятие
/// и на пачке неверна: два снятия, каждое из которых поодиночке
/// безопасно, вместе перерезают перемычку в два воксела (ADR-118).
/// Здесь вопрос задаётся пачке целиком.
///
/// Идея: взять **оболочку** выреза — твёрдые ячейки, касавшиеся снятого,
/// — и проверить, что все они связаны между собой **в окрестности
/// выреза**. Если да, любой путь, проходивший через снятое, имеет
/// местный обход, и расщепить снятие не могло. Условие достаточное,
/// но не необходимое: обход длиннее окрестности проверка не увидит
/// и честно скажет «не знаю» (ADR-131).
///
/// Возвращает `true` только когда расщепления **точно** не было.
pub fn batch_cannot_split(c: &Chunk, removed: &[(u8, u8, u8)], b: &mut SplitBufs) -> bool {
    if removed.is_empty() {
        return true;
    }
    // Окрестность: габарит снятого, расширенный на воксель.
    let (mut x0, mut y0, mut z0) = (CS, CS, CS);
    let (mut x1, mut y1, mut z1) = (0usize, 0usize, 0usize);
    for &(x, y, z) in removed {
        let (x, y, z) = (x as usize, y as usize, z as usize);
        x0 = x0.min(x);
        y0 = y0.min(y);
        z0 = z0.min(z);
        x1 = x1.max(x);
        y1 = y1.max(y);
        z1 = z1.max(z);
    }
    let (x0, y0, z0) = (
        x0.saturating_sub(1),
        y0.saturating_sub(1),
        z0.saturating_sub(1),
    );
    let (x1, y1, z1) = (
        (x1 + 1).min(CS - 1),
        (y1 + 1).min(CS - 1),
        (z1 + 1).min(CS - 1),
    );
    let vol = (x1 - x0 + 1) * (y1 - y0 + 1) * (z1 - z0 + 1);
    if vol > SPLIT_PROBE_CAP {
        return false;
    }

    b.epoch = b.epoch.wrapping_add(1);
    if b.epoch == 0 {
        b.stamp.iter_mut().for_each(|v| *v = 0);
        b.epoch = 1;
    }
    let ep = b.epoch;

    // Оболочка: твёрдое, касавшееся снятого.
    b.shell.clear();
    for &(x, y, z) in removed {
        let (x, y, z) = (x as usize, y as usize, z as usize);
        for (dx, dy, dz) in [
            (-1i32, 0i32, 0i32),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ] {
            let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
            if nx < 0 || ny < 0 || nz < 0 {
                continue;
            }
            let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
            if nx >= CS || ny >= CS || nz >= CS || !c.get(nx, ny, nz) {
                continue;
            }
            let i = cell_index(nx, ny, nz);
            if b.stamp[i] != ep {
                b.stamp[i] = ep;
                b.shell.push(i as u32);
            }
        }
    }
    if b.shell.len() <= 1 {
        // Ноль или одна ячейка оболочки: разделять нечего.
        return true;
    }

    // Заливка от первой ячейки оболочки, не выходя из окрестности.
    // Метки оболочки уже стоят, поэтому считаем, скольких из них
    // заливка коснулась.
    let ep2 = ep.wrapping_add(1);
    b.epoch = ep2;
    b.queue.clear();
    let start = b.shell[0];
    b.stamp[start as usize] = ep2;
    b.queue.push(start);
    let mut found = 1usize;
    let mut head = 0usize;
    while head < b.queue.len() {
        let i = b.queue[head] as usize;
        head += 1;
        let (x, y, z) = (i % CS, (i / CS) % CS, i / (CS * CS));
        for (dx, dy, dz) in [
            (-1i32, 0i32, 0i32),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ] {
            let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
            if nx < x0 as i32 || ny < y0 as i32 || nz < z0 as i32 {
                continue;
            }
            let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
            if nx > x1 || ny > y1 || nz > z1 || !c.get(nx, ny, nz) {
                continue;
            }
            let j = cell_index(nx, ny, nz);
            let st = b.stamp[j];
            if st == ep2 {
                continue;
            }
            if st == ep {
                found += 1;
            }
            b.stamp[j] = ep2;
            b.queue.push(j as u32);
        }
    }
    found == b.shell.len()
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
    use crate::topo::scene::{build, Scene};

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
                let ref_count = reference
                    .iter()
                    .copied()
                    .filter(|&v| v >= 0)
                    .max()
                    .map_or(0, |m| m + 1) as usize;
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

    /// Пустота и твёрдое — два графа одной конструкции. У полностью
    /// сплошного чанка пустоты нет, у пустого — одна компонента.
    #[test]
    fn void_is_the_complement() {
        let mut comp = Components::new();
        assert_eq!(
            comp.build_void(&Chunk::new()),
            1,
            "у пустого чанка нет пустоты"
        );
        let mut full = Chunk::new();
        full.cols.iter_mut().for_each(|c| *c = u32::MAX);
        assert_eq!(
            comp.build_void(&full),
            0,
            "у сплошного чанка нашлась пустота"
        );

        // Две несвязанные полости в сплошной породе — две компоненты пустоты.
        let mut c = full.clone();
        for (cx, cy, cz) in [(8usize, 8usize, 8usize), (24, 24, 24)] {
            for dz in 0..4 {
                for dy in 0..4 {
                    for dx in 0..4 {
                        c.set(cx + dx, cy + dy, cz + dz, false);
                    }
                }
            }
        }
        assert_eq!(comp.build_void(&c), 2, "полости слиплись");
    }

    /// Пустота, посчитанная по инверсии, обязана совпадать с эталоном
    /// на инвертированном чанке.
    #[test]
    fn void_matches_reference() {
        let mut comp = Components::new();
        for scene in Scene::ALL {
            let c = build(scene, 3);
            let n = comp.build_void(&c);
            let inv = Chunk {
                cols: c.cols.iter().map(|w| !w).collect(),
            };
            let reference = reference_labels(&inv);
            let want = reference
                .iter()
                .copied()
                .filter(|&v| v >= 0)
                .max()
                .map_or(0, |m| m + 1) as usize;
            assert_eq!(n, want, "{}", scene.name());
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

#[cfg(test)]
mod split_probe_tests {
    use super::*;
    use fx::rng::Rng;

    /// Разбиение чанка на компоненты как **множество множеств**:
    /// сравнивать номера нельзя, они перенумеровываются.
    fn partition(c: &Chunk) -> Vec<Vec<usize>> {
        let mut comp = Components::new();
        let n = comp.build(c);
        let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (ri, r) in comp.runs.iter().enumerate() {
            let lab = comp.label[ri] as usize;
            let col = r.col as usize;
            let (y, z) = (col % CS, col / CS);
            let mut bits = r.mask;
            while bits != 0 {
                let x = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                out[lab].push(cell_index(x, y, z));
            }
        }
        for v in out.iter_mut() {
            v.sort_unstable();
        }
        out.sort();
        out
    }

    /// **Проверка обязана быть достаточной**: сказала «не расщепило» —
    /// значит разбиение и вправду то же, что было, за вычетом снятого.
    ///
    /// Ловится это перебором: случайные чанки, случайные пачки снятий,
    /// и каждое «да» сверяется с полной разметкой до и после.
    #[test]
    fn a_positive_answer_is_always_right() {
        let mut b = SplitBufs::new();
        let mut said_yes = 0;
        let mut said_no = 0;
        for seed in 0..200u64 {
            let mut r = Rng::new(seed, 0x5D1);
            let mut c = Chunk::new();
            // Пористая порода: заполнение гуляет, иначе всё сплошное
            // и расщепить нечего.
            let fill = 40 + r.below(55);
            for z in 0..CS {
                for y in 0..CS {
                    for x in 0..CS {
                        if r.below(100) < fill {
                            c.set(x, y, z, true);
                        }
                    }
                }
            }
            let before = partition(&c);
            // Пачка снятий: шар случайного радиуса в случайном месте.
            let (cx, cy, cz) = (
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
                r.below(CS as u32) as usize,
            );
            let rad = 1 + r.below(3) as i32;
            let mut removed: Vec<(u8, u8, u8)> = Vec::new();
            for dz in -rad..=rad {
                for dy in -rad..=rad {
                    for dx in -rad..=rad {
                        if dx * dx + dy * dy + dz * dz > rad * rad {
                            continue;
                        }
                        let (x, y, z) = (cx as i32 + dx, cy as i32 + dy, cz as i32 + dz);
                        if x < 0 || y < 0 || z < 0 {
                            continue;
                        }
                        let (x, y, z) = (x as usize, y as usize, z as usize);
                        if x >= CS || y >= CS || z >= CS || !c.get(x, y, z) {
                            continue;
                        }
                        c.set(x, y, z, false);
                        removed.push((x as u8, y as u8, z as u8));
                    }
                }
            }
            if removed.is_empty() {
                continue;
            }
            let verdict = batch_cannot_split(&c, &removed, &mut b);
            if !verdict {
                said_no += 1;
                continue;
            }
            said_yes += 1;
            // Разбиение после снятий обязано быть тем же, что до,
            // за вычетом снятых ячеек.
            let after = partition(&c);
            let gone: Vec<usize> = removed
                .iter()
                .map(|&(x, y, z)| cell_index(x as usize, y as usize, z as usize))
                .collect();
            let mut expect: Vec<Vec<usize>> = before
                .iter()
                .map(|v| {
                    v.iter()
                        .copied()
                        .filter(|i| !gone.contains(i))
                        .collect::<Vec<_>>()
                })
                .filter(|v: &Vec<usize>| !v.is_empty())
                .collect();
            expect.sort();
            assert_eq!(
                after, expect,
                "сид {seed}: сказала «не расщепило», а расщепило"
            );
        }
        // Стенд обязан нагружать обе ветки, иначе он ничего не проверил.
        assert!(said_yes > 0, "положительных ответов не было вовсе");
        assert!(said_no > 0, "отрицательных ответов не было вовсе");
    }

    /// Перемычка в два воксела: два снятия, каждое из которых
    /// поодиночке безопасно, вместе разрезают. Проверка обязана
    /// сказать «не знаю».
    #[test]
    fn a_two_thick_neck_is_not_called_safe() {
        let mut b = SplitBufs::new();
        let mut c = Chunk::new();
        // Две плиты, соединённые перемычкой два на два.
        for z in 0..CS {
            for y in 0..CS {
                for x in 0..CS {
                    let slab = !(10..=21).contains(&x);
                    let neck =
                        (10..=21).contains(&x) && (15..17).contains(&y) && (15..17).contains(&z);
                    if slab || neck {
                        c.set(x, y, z, true);
                    }
                }
            }
        }
        let mut removed = Vec::new();
        for y in 15..17 {
            for z in 15..17 {
                c.set(16, y, z, false);
                removed.push((16u8, y as u8, z as u8));
            }
        }
        assert!(
            !batch_cannot_split(&c, &removed, &mut b),
            "перемычка названа безопасной"
        );
    }

    /// Вырез в сплошной породе ничего не расщепляет, и проверка
    /// обязана это видеть — иначе она бесполезна.
    #[test]
    fn a_carve_in_solid_rock_is_called_safe() {
        let mut b = SplitBufs::new();
        let mut c = Chunk::new();
        for z in 0..CS {
            for y in 0..CS {
                for x in 0..CS {
                    c.set(x, y, z, true);
                }
            }
        }
        let mut removed = Vec::new();
        for dz in -2i32..=2 {
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    if dx * dx + dy * dy + dz * dz > 4 {
                        continue;
                    }
                    let (x, y, z) = ((16 + dx) as usize, (16 + dy) as usize, (16 + dz) as usize);
                    c.set(x, y, z, false);
                    removed.push((x as u8, y as u8, z as u8));
                }
            }
        }
        assert!(
            batch_cannot_split(&c, &removed, &mut b),
            "вырез в сплошном назван опасным"
        );
    }
}
