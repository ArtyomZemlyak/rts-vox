//! Пирамида занятости (ADR-061) и марш луча по ней.
//!
//! ADR-063 держит бюджет кадра на двух числах: сколько итераций марша стоит
//! затенение и сколько солнце. Оба получены расчётом. Здесь они меряются.
//!
//! **Пирамида — своя структура, а не чанк мешинга.** Чанк мешинга это 62
//! полезных воксела плюс паддинг соседей, то есть его локальные координаты
//! не совпадают с мировыми по модулю степени двойки. Брик 4³ и тайл 16³
//! обязаны быть выровнены по мировой сетке, иначе пропуск уводит луч
//! не туда. Проверено: попытка построить пирамиду поверх паддинговых
//! чанков даёт лучи сквозь стену.
//!
//! **Арифметика в `f32`.** Это рендер: расхождения тут безопасны
//! (`CLAUDE.md`), и в шейдере всё равно будет `f32`. Лучи разрешения
//! попаданий — другая подсистема и живут в `Fx` (`ray.rs`).
//!
//! Ветвление ×4, как в ADR-061: воксель, брик 4³, тайл 16³, блок 64³.

use crate::World;

/// Сторона блока верхнего уровня.
pub const BLK: usize = 64;

/// Пирамида мира: блоки 64³, выложенные по мировой сетке без паддинга.
pub struct Pyramid {
    pub bx: usize,
    pub by: usize,
    pub bz: usize,
    /// L0: бит на воксель. `l0[blk][y * 64 + z]`, бит `x` — координаты
    /// внутри блока.
    l0: Vec<Vec<u64>>,
    /// L1: бит на брик 4³. `l1[blk][by * 16 + bz]`, бит `bx`.
    l1: Vec<[u16; 16 * 16]>,
    /// L2: бит на тайл 16³, один `u64` на блок.
    l2: Vec<u64>,
}

impl Pyramid {
    pub fn build(w: &World) -> Pyramid {
        let bx = w.voxels_x().div_ceil(BLK);
        let by = mesh::CS.div_ceil(BLK);
        let bz = w.voxels_z().div_ceil(BLK);
        let n = bx * by * bz;
        let mut l0 = vec![vec![0u64; BLK * BLK]; n];

        for z in 0..w.voxels_z() {
            for y in 0..mesh::CS {
                for x in 0..w.voxels_x() {
                    if !w.get(x, y, z) {
                        continue;
                    }
                    let b = (y / BLK * bz + z / BLK) * bx + x / BLK;
                    l0[b][(y % BLK) * BLK + (z % BLK)] |= 1u64 << (x % BLK);
                }
            }
        }

        let mut l1 = Vec::with_capacity(n);
        let mut l2 = Vec::with_capacity(n);
        for block in l0.iter() {
            let mut a = [0u16; 16 * 16];
            for cy in 0..16usize {
                for cz in 0..16usize {
                    let mut word = 0u16;
                    for dy in 0..4usize {
                        for dz in 0..4usize {
                            let col = block[(cy * 4 + dy) * BLK + (cz * 4 + dz)];
                            for cx in 0..16usize {
                                if col & (0xFu64 << (cx * 4)) != 0 {
                                    word |= 1 << cx;
                                }
                            }
                        }
                    }
                    a[cy * 16 + cz] = word;
                }
            }
            let mut t = 0u64;
            for ty in 0..4usize {
                for tz in 0..4usize {
                    for tx in 0..4usize {
                        let mut any = false;
                        'outer: for dy in 0..4usize {
                            for dz in 0..4usize {
                                if a[(ty * 4 + dy) * 16 + (tz * 4 + dz)] & (0xFu16 << (tx * 4)) != 0
                                {
                                    any = true;
                                    break 'outer;
                                }
                            }
                        }
                        if any {
                            t |= 1u64 << (ty * 16 + tz * 4 + tx);
                        }
                    }
                }
            }
            l1.push(a);
            l2.push(t);
        }

        Pyramid { bx, by, bz, l0, l1, l2 }
    }

    #[inline(always)]
    fn block_of(&self, x: usize, y: usize, z: usize) -> usize {
        (y / BLK * self.bz + z / BLK) * self.bx + x / BLK
    }

    #[inline(always)]
    pub fn solid(&self, x: usize, y: usize, z: usize) -> bool {
        let b = self.block_of(x, y, z);
        self.l0[b][(y % BLK) * BLK + (z % BLK)] & (1u64 << (x % BLK)) != 0
    }

    #[inline(always)]
    fn brick_any(&self, x: usize, y: usize, z: usize) -> bool {
        let b = self.block_of(x, y, z);
        let (cx, cy, cz) = ((x % BLK) / 4, (y % BLK) / 4, (z % BLK) / 4);
        self.l1[b][cy * 16 + cz] & (1u16 << cx) != 0
    }

    #[inline(always)]
    fn tile_any(&self, x: usize, y: usize, z: usize) -> bool {
        let b = self.block_of(x, y, z);
        let (tx, ty, tz) = ((x % BLK) / 16, (y % BLK) / 16, (z % BLK) / 16);
        self.l2[b] & (1u64 << (ty * 16 + tz * 4 + tx)) != 0
    }

    /// Байты уровней. L0 здесь для сравнения — в игре он и так есть.
    pub fn bytes_l0(&self) -> usize {
        self.l0.len() * BLK * BLK * 8
    }

    pub fn bytes_upper(&self) -> usize {
        self.l1.len() * 16 * 16 * 2 + self.l2.len() * 8
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MarchStats {
    pub hit: bool,
    /// Итераций цикла — то, чем ADR-063 меряет бюджет кадра.
    pub iters: u32,
    pub skips_l2: u32,
    pub skips_l1: u32,
}

struct Ray {
    o: [f32; 3],
    d: [f32; 3],
    inv: [f32; 3],
}

impl Ray {
    fn new(o: [f32; 3], d: [f32; 3]) -> Ray {
        Ray {
            o,
            d,
            inv: [
                if d[0] != 0.0 { 1.0 / d[0] } else { f32::INFINITY },
                if d[1] != 0.0 { 1.0 / d[1] } else { f32::INFINITY },
                if d[2] != 0.0 { 1.0 / d[2] } else { f32::INFINITY },
            ],
        }
    }

    /// Параметр выхода из выровненной ячейки размера `size`, в которой
    /// луч находится в момент `t`.
    #[inline(always)]
    fn exit(&self, t: f32, size: f32) -> f32 {
        let mut best = f32::INFINITY;
        for a in 0..3 {
            if self.d[a] == 0.0 {
                continue;
            }
            let pos = self.o[a] + self.d[a] * t;
            let base = (pos / size).floor() * size;
            let plane = if self.d[a] > 0.0 { base + size } else { base };
            let tt = (plane - self.o[a]) * self.inv[a];
            if tt > t && tt < best {
                best = tt;
            }
        }
        best
    }
}

/// Марш по вокселям, без пирамиды. Точка отсчёта — **точный** обход,
/// а не шаг фиксированной длины: фиксированный шаг пропускает воксели
/// на диагоналях и сравнивать с ним нечестно.
pub fn march_flat(w: &World, o: [f32; 3], d: [f32; 3], max_dist: f32) -> MarchStats {
    let r = Ray::new(o, d);
    let mut st = MarchStats::default();
    let mut t = 0.0f32;
    while t < max_dist {
        st.iters += 1;
        let (x, y, z) = (
            (r.o[0] + r.d[0] * t).floor() as i32,
            (r.o[1] + r.d[1] * t).floor() as i32,
            (r.o[2] + r.d[2] * t).floor() as i32,
        );
        if !w.inside(x, y, z) {
            return st;
        }
        if w.get_i(x, y, z) {
            st.hit = true;
            return st;
        }
        let nt = r.exit(t, 1.0);
        if !nt.is_finite() {
            return st;
        }
        t = nt + 1e-4;
    }
    st
}

/// Марш по пирамиде: пустая крупная ячейка проглатывается одним шагом.
pub fn march_pyramid(p: &Pyramid, w: &World, o: [f32; 3], d: [f32; 3], max_dist: f32) -> MarchStats {
    let r = Ray::new(o, d);
    let mut st = MarchStats::default();
    let mut t = 0.0f32;
    while t < max_dist {
        st.iters += 1;
        let (x, y, z) = (
            (r.o[0] + r.d[0] * t).floor() as i32,
            (r.o[1] + r.d[1] * t).floor() as i32,
            (r.o[2] + r.d[2] * t).floor() as i32,
        );
        if !w.inside(x, y, z) {
            return st;
        }
        let (ux, uy, uz) = (x as usize, y as usize, z as usize);

        if !p.tile_any(ux, uy, uz) {
            st.skips_l2 += 1;
            let nt = r.exit(t, 16.0);
            if !nt.is_finite() {
                return st;
            }
            t = nt + 1e-4;
            continue;
        }
        if !p.brick_any(ux, uy, uz) {
            st.skips_l1 += 1;
            let nt = r.exit(t, 4.0);
            if !nt.is_finite() {
                return st;
            }
            t = nt + 1e-4;
            continue;
        }
        if p.solid(ux, uy, uz) {
            st.hit = true;
            return st;
        }
        let nt = r.exit(t, 1.0);
        if !nt.is_finite() {
            return st;
        }
        t = nt + 1e-4;
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L0 пирамиды обязан совпадать с миром: пирамида строится отдельно,
    /// и рассинхрон здесь означал бы, что марш смотрит в другой мир.
    #[test]
    fn l0_matches_world() {
        let w = World::new(2, 2, 17);
        let p = Pyramid::build(&w);
        for z in 0..w.voxels_z() {
            for y in 0..mesh::CS {
                for x in 0..w.voxels_x() {
                    assert_eq!(p.solid(x, y, z), w.get(x, y, z), "({x},{y},{z})");
                }
            }
        }
    }

    /// Верхние уровни консервативны: если бит не стоит, внутри пусто.
    /// Обратное — ложное «занято» — безопасно и допустимо.
    #[test]
    fn upper_levels_are_conservative() {
        let w = World::new(2, 2, 3);
        let p = Pyramid::build(&w);
        for z in 0..w.voxels_z() {
            for y in 0..mesh::CS {
                for x in 0..w.voxels_x() {
                    if w.get(x, y, z) {
                        assert!(p.brick_any(x, y, z), "брик пуст, а воксель твёрдый");
                        assert!(p.tile_any(x, y, z), "тайл пуст, а воксель твёрдый");
                    }
                }
            }
        }
    }

    /// **Пирамида обязана давать тот же вердикт, что марш по вокселям.**
    /// Расхождение в сторону «не попал» — это луч сквозь стену.
    #[test]
    fn pyramid_agrees_with_flat() {
        let w = World::new(2, 2, 17);
        let p = Pyramid::build(&w);
        let mut r = fx::rng::Rng::new(5, 5);
        let mut checked = 0;
        let mut hits = 0;
        for _ in 0..6000 {
            let o = [
                r.below(w.voxels_x() as u32) as f32 + 0.5,
                r.range(2, mesh::CS as i32 - 2) as f32 + 0.5,
                r.below(w.voxels_z() as u32) as f32 + 0.5,
            ];
            let (dx, dy, dz) = (
                r.range(-100, 100) as f32 / 100.0,
                r.range(-100, 100) as f32 / 100.0,
                r.range(-100, 100) as f32 / 100.0,
            );
            let len = (dx * dx + dy * dy + dz * dz).sqrt();
            if len < 0.2 {
                continue;
            }
            let d = [dx / len, dy / len, dz / len];
            checked += 1;
            let a = march_flat(&w, o, d, 80.0);
            let b = march_pyramid(&p, &w, o, d, 80.0);
            hits += a.hit as usize;
            assert_eq!(a.hit, b.hit, "вердикт разошёлся из {o:?} в {d:?}");
        }
        assert!(checked > 5000);
        assert!(hits > 1000, "почти ничего не попало — сцена подозрительна");
    }
}
