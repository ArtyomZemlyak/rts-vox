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
        }
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
        let mut cursor = self.starts.clone();
        for &i in alive {
            let b = self.bucket_of(xs[i as usize], zs[i as usize]);
            self.items[cursor[b] as usize] = i;
            cursor[b] += 1;
        }
    }

    /// Соседи в 3×3 вёдрах вокруг точки. Порядок обхода фиксирован:
    /// по ведру, внутри ведра по индексу сущности.
    #[inline]
    pub fn for_each_near(&self, x: i32, z: i32, mut f: impl FnMut(u32)) {
        let bx = (x / self.cell).clamp(0, self.w as i32 - 1);
        let bz = (z / self.cell).clamp(0, self.h as i32 - 1);
        for gz in (bz - 1).max(0)..=(bz + 1).min(self.h as i32 - 1) {
            for gx in (bx - 1).max(0)..=(bx + 1).min(self.w as i32 - 1) {
                let b = gz as usize * self.w + gx as usize;
                let (lo, hi) = (self.starts[b] as usize, self.starts[b + 1] as usize);
                for &j in &self.items[lo..hi] {
                    f(j);
                }
            }
        }
    }

    /// Сколько сущностей осмотрит запрос — цена, а не результат.
    #[inline]
    pub fn near_count(&self, x: i32, z: i32) -> usize {
        let mut n = 0;
        self.for_each_near(x, z, |_| n += 1);
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
