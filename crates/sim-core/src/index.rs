//! Равномерная сетка для запросов «кто рядом». Раскладка CSR, счётная
//! сортировка: одно выделение вместо тысяч, порядок внутри ведра — по
//! индексу сущности, то есть детерминирован.
//!
//! `HashMap` здесь был бы естественным выбором и запрещён (`CLAUDE.md`):
//! порядок итерации обязан быть стабильным.

/// Сторона ведра в мировых единицах. Берётся равной наибольшему радиусу
/// запроса, тогда запрос трогает ровно 3×3 ведра.
pub struct Grid {
    pub cell: i32,
    pub w: usize,
    pub h: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
    /// Курсор раскладки. Поле, а не `starts.clone()` на каждый тик:
    /// клон — это выделение кучи размером со всю сетку, каждый тик,
    /// дважды (ADR-189).
    cursor: Vec<u32>,
}

impl Grid {
    pub fn new(world: i32, cell: i32, capacity: usize) -> Grid {
        let w = (world / cell).max(1) as usize + 1;
        Grid {
            cell,
            w,
            h: w,
            starts: vec![0; w * w + 1],
            items: vec![0; capacity],
            cursor: vec![0; w * w + 1],
        }
    }

    /// Перевыставить сторону мира.
    ///
    /// Сетка создаётся вместе с миром, а настоящий размер мира
    /// приходит позже, вместе с землёй. До этого она была размером
    /// с `WORLD` — две тысячи единиц, — каким бы ни был мир на самом
    /// деле, и каждый тик обнуляла вёдра мира, которого нет (ADR-189).
    ///
    /// Обнуление лишнего не меняет ответов: `bucket_of` клампит
    /// координату, а юнит за пределы земли не выходит. Меняется
    /// только цена.
    pub fn resize_world(&mut self, world: i32) {
        let w = (world / self.cell).max(1) as usize + 1;
        if w == self.w {
            return;
        }
        self.w = w;
        self.h = w;
        self.starts.clear();
        self.starts.resize(w * w + 1, 0);
        self.cursor.clear();
        self.cursor.resize(w * w + 1, 0);
    }

    #[inline(always)]
    pub fn bucket_of(&self, x: i32, z: i32) -> usize {
        let bx = (x / self.cell).clamp(0, self.w as i32 - 1) as usize;
        let bz = (z / self.cell).clamp(0, self.h as i32 - 1) as usize;
        bz * self.w + bx
    }

    /// Пересборка. `alive` перечисляет живые индексы в возрастающем порядке.
    pub fn rebuild(&mut self, xs: &[i32], zs: &[i32], alive: &[u32]) {
        self.starts.iter_mut().for_each(|v| *v = 0);
        if self.items.len() < alive.len() {
            self.items.resize(alive.len(), 0);
        }
        for &i in alive {
            let b = self.bucket_of(xs[i as usize], zs[i as usize]);
            self.starts[b + 1] += 1;
        }
        for k in 1..self.starts.len() {
            self.starts[k] += self.starts[k - 1];
        }
        // Курсор поверх starts: второй проход раскладывает по вёдрам.
        self.cursor.copy_from_slice(&self.starts);
        for &i in alive {
            let b = self.bucket_of(xs[i as usize], zs[i as usize]);
            self.items[self.cursor[b] as usize] = i;
            self.cursor[b] += 1;
        }
    }

    /// Соседи в 3×3 вёдрах вокруг точки. Порядок обхода фиксирован:
    /// по ведру, внутри ведра по индексу сущности.
    #[inline]
    pub fn for_each_near(&self, x: i32, z: i32, mut f: impl FnMut(u32)) {
        self.for_each_bucket_near(x, z, |b| {
            for &j in self.bucket_items(b) {
                f(j);
            }
        });
    }

    pub(crate) fn bucket_items(&self, b: usize) -> &[u32] {
        &self.items[self.starts[b] as usize..self.starts[b + 1] as usize]
    }

    #[inline]
    pub(crate) fn for_each_bucket_near(&self, x: i32, z: i32, mut f: impl FnMut(usize)) {
        let bx = (x / self.cell).clamp(0, self.w as i32 - 1);
        let bz = (z / self.cell).clamp(0, self.h as i32 - 1);
        for gz in (bz - 1).max(0)..=(bz + 1).min(self.h as i32 - 1) {
            for gx in (bx - 1).max(0)..=(bx + 1).min(self.w as i32 - 1) {
                let b = gz as usize * self.w + gx as usize;
                f(b);
            }
        }
    }

    /// Superset of every 3x3 query along a monotone swept segment. The
    /// caller still checks membership at each sample; corner buckets may
    /// never be visited by the exact walk. Clamped edge buckets are retained.
    pub(crate) fn for_each_bucket_swept(&self, a: [i32; 2], b: [i32; 2], mut f: impl FnMut(usize)) {
        let a = self.bucket_of(a[0], a[1]);
        let b = self.bucket_of(b[0], b[1]);
        let (ax, az, bx, bz) = (a % self.w, a / self.w, b % self.w, b / self.w);
        for z in az.min(bz).saturating_sub(1)..=(az.max(bz) + 1).min(self.h - 1) {
            for x in ax.min(bx).saturating_sub(1)..=(ax.max(bx) + 1).min(self.w - 1) {
                f(z * self.w + x);
            }
        }
    }

    /// Candidate centres in a swept box expanded by the largest body radius.
    /// Buckets are visited once; exact geometry and ties are the caller's job.
    pub fn for_each_rect(&self, lo: [i32; 2], hi: [i32; 2], mut f: impl FnMut(u32)) {
        let x0 = (lo[0] / self.cell).clamp(0, self.w as i32 - 1) as usize;
        let x1 = (hi[0] / self.cell).clamp(0, self.w as i32 - 1) as usize;
        let z0 = (lo[1] / self.cell).clamp(0, self.h as i32 - 1) as usize;
        let z1 = (hi[1] / self.cell).clamp(0, self.h as i32 - 1) as usize;
        for z in z0..=z1 {
            for x in x0..=x1 {
                let b = z * self.w + x;
                for k in self.starts[b]..self.starts[b + 1] {
                    f(self.items[k as usize]);
                }
            }
        }
    }

    /// Сколько сущностей осмотрит запрос — цена, а не результат.
    #[inline]
    pub fn near_count(&self, x: i32, z: i32) -> usize {
        let mut n = 0;
        self.for_each_bucket_near(x, z, |b| n += self.bucket_items(b).len());
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_everyone_within_a_bucket_ring() {
        let n = 400usize;
        let mut xs = Vec::new();
        let mut zs = Vec::new();
        let mut r = fx::rng::Rng::new(4, 4);
        for _ in 0..n {
            xs.push(r.below(1000) as i32);
            zs.push(r.below(1000) as i32);
        }
        let alive: Vec<u32> = (0..n as u32).collect();
        let mut g = Grid::new(1000, 32, n);
        g.rebuild(&xs, &zs, &alive);

        for _ in 0..100 {
            let (qx, qz) = (r.below(1000) as i32, r.below(1000) as i32);
            let mut got = Vec::new();
            g.for_each_near(qx, qz, |j| got.push(j));
            got.sort_unstable();
            // Все, кто ближе стороны ведра, обязаны найтись.
            for i in 0..n {
                let (dx, dz) = ((xs[i] - qx).abs(), (zs[i] - qz).abs());
                if dx <= 32 && dz <= 32 {
                    assert!(
                        got.binary_search(&(i as u32)).is_ok(),
                        "не найден {i} на расстоянии ({dx},{dz})"
                    );
                }
            }
        }
    }

    /// Порядок обхода обязан быть функцией индексов, а не порядка вставки.
    #[test]
    fn order_is_stable() {
        let xs: Vec<i32> = (0..64).map(|i| (i * 7) % 200).collect();
        let zs: Vec<i32> = (0..64).map(|i| (i * 13) % 200).collect();
        let mut g = Grid::new(200, 32, 64);

        let fwd: Vec<u32> = (0..64u32).collect();
        g.rebuild(&xs, &zs, &fwd);
        let mut a = Vec::new();
        g.for_each_near(100, 100, |j| a.push(j));

        g.rebuild(&xs, &zs, &fwd);
        let mut b = Vec::new();
        g.for_each_near(100, 100, |j| b.push(j));
        assert_eq!(a, b);
    }
}
