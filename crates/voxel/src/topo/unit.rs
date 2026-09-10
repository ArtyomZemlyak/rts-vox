//! Связность частей юнита с ядром (ADR-016).
//!
//! ADR-016 прямо требует «отдельной строки в бюджетах и скелетного бенчмарка
//! ещё на этапе 0 — иначе цена решения выяснится только на этапе 6».
//! Строка в `../../docs/10-budgets.md` до сих пор пустая.
//!
//! Задача **не та же**, что связность мира. Там нужно разбиение на компоненты;
//! здесь нужен один вопрос: что осталось прикреплённым к ядру. Значит флуд
//! от ядра, а не пометка всего тома, и цена — размер **уцелевшего**, а не
//! размер тома.
//!
//! Раскладка та же, что у чанка: колонка по X — один `u32`, но размеры тома
//! произвольные до 32.

use crate::topo::stage_a::Adjacency;

/// Том юнита. `cols[z * ny + y]`, бит `x`.
#[derive(Clone, PartialEq, Eq)]
pub struct UnitVol {
    pub nx: u8,
    pub ny: u8,
    pub nz: u8,
    pub cols: Vec<u32>,
}

impl UnitVol {
    pub fn new(nx: u8, ny: u8, nz: u8) -> UnitVol {
        assert!(nx <= 32 && nx > 0);
        UnitVol {
            nx,
            ny,
            nz,
            cols: vec![0; ny as usize * nz as usize],
        }
    }

    #[inline(always)]
    pub fn col(&self, y: usize, z: usize) -> u32 {
        self.cols[z * self.ny as usize + y]
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> bool {
        self.col(y, z) & (1u32 << x) != 0
    }

    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, z: usize, v: bool) {
        let i = z * self.ny as usize + y;
        if v {
            self.cols[i] |= 1u32 << x;
        } else {
            self.cols[i] &= !(1u32 << x);
        }
    }

    pub fn solid_count(&self) -> u32 {
        self.cols.iter().map(|c| c.count_ones()).sum()
    }

    pub fn voxels(&self) -> usize {
        self.nx as usize * self.ny as usize * self.nz as usize
    }
}

/// Переиспользуемые буферы флуда. Событий — сотни за тик.
pub struct CoreBufs {
    /// Достигнутая часть каждой колонки. Не «посещено», а именно маска:
    /// колонка обрабатывается пробегами, а не по одному биту.
    reached: Vec<u32>,
    queue: Vec<u32>,
    head: usize,
}

impl Default for CoreBufs {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreBufs {
    pub fn new() -> CoreBufs {
        CoreBufs {
            reached: Vec::new(),
            queue: Vec::with_capacity(1024),
            head: 0,
        }
    }

    fn reset(&mut self, n: usize) {
        self.reached.clear();
        self.reached.resize(n, 0);
        self.queue.clear();
        self.head = 0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreReport {
    /// Сколько твёрдых вокселей осталось прикреплено к ядру.
    pub attached: u32,
    /// Сколько отвалилось.
    pub detached: u32,
    /// Сколько колонок пришлось тронуть — это и есть цена.
    pub cols_touched: u32,
    /// Ядро уцелело.
    pub core_alive: bool,
}

/// Расширить маску пробега вверх и вниз внутри колонки: заливка по X
/// делается сдвигами, а не по биту.
#[inline(always)]
fn saturate(seed: u32, solid: u32) -> u32 {
    // Классический приём: прибавление к маске «протаскивает» единицы
    // по сплошному участку. Делаем в обе стороны и пересекаем с solid.
    let mut r = seed;
    loop {
        let grown = (r | (r << 1) | (r >> 1)) & solid;
        if grown == r {
            return r;
        }
        r = grown;
    }
}

/// Что осталось прикреплённым к ядру.
///
/// Флуд идёт по колонкам: внутри колонки — сдвигами, между колонками —
/// пересечением масок. Стоимость линейна по числу **уцелевших** колонок,
/// а не по объёму тома.
pub fn core_connectivity(
    v: &UnitVol,
    b: &mut CoreBufs,
    core: (u8, u8, u8),
    core_size: u8,
) -> CoreReport {
    let (ny, nz) = (v.ny as usize, v.nz as usize);
    b.reset(ny * nz);

    let total = v.solid_count();
    // Ядро — куб `core_size` вокруг заданной точки. Уцелело, если хотя бы
    // один его воксель твёрдый: ядро разрушено — юнит выведен из строя,
    // и связность считать уже незачем.
    let cs = core_size as usize;
    let (cx, cy, cz) = (core.0 as usize, core.1 as usize, core.2 as usize);
    let mut core_mask_any = false;
    for dz in 0..cs {
        for dy in 0..cs {
            let (y, z) = (cy + dy, cz + dz);
            if y >= ny || z >= nz {
                continue;
            }
            let solid = v.cols[z * ny + y];
            let lo = cx.min(31);
            let hi = (cx + cs - 1).min(31);
            let band = if hi >= 31 {
                !0u32 << lo
            } else {
                ((1u32 << (hi - lo + 1)) - 1) << lo
            };
            let seed = solid & band;
            if seed != 0 {
                core_mask_any = true;
                let filled = saturate(seed, solid);
                if b.reached[z * ny + y] != filled {
                    b.reached[z * ny + y] |= filled;
                    b.queue.push((z * ny + y) as u32);
                }
            }
        }
    }
    if !core_mask_any {
        return CoreReport {
            attached: 0,
            detached: total,
            cols_touched: 0,
            core_alive: false,
        };
    }

    let mut cols_touched = 0u32;
    while b.head < b.queue.len() {
        let ci = b.queue[b.head] as usize;
        b.head += 1;
        cols_touched += 1;
        let mine = b.reached[ci];
        let (y, z) = (ci % ny, ci / ny);
        let nbs = [
            (y.wrapping_sub(1), z),
            (y + 1, z),
            (y, z.wrapping_sub(1)),
            (y, z + 1),
        ];
        for (ay, az) in nbs {
            if ay >= ny || az >= nz {
                continue;
            }
            let cj = az * ny + ay;
            let solid = v.cols[cj];
            // Пробеги соседа, которых касается уже достигнутое.
            let seed = solid & mine;
            if seed == 0 {
                continue;
            }
            let filled = saturate(seed, solid);
            let before = b.reached[cj];
            let after = before | filled;
            if after != before {
                b.reached[cj] = after;
                b.queue.push(cj as u32);
            }
        }
    }

    let attached: u32 = b.reached.iter().map(|m| m.count_ones()).sum();
    CoreReport {
        attached,
        detached: total - attached,
        cols_touched,
        core_alive: true,
    }
}

/// Эталон: флуд по вокселям. Медленный и очевидный.
pub fn core_connectivity_reference(v: &UnitVol, core: (u8, u8, u8), core_size: u8) -> (u32, u32) {
    let (nx, ny, nz) = (v.nx as usize, v.ny as usize, v.nz as usize);
    let idx = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    let mut seen = vec![false; nx * ny * nz];
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    let cs = core_size as usize;
    for dz in 0..cs {
        for dy in 0..cs {
            for dx in 0..cs {
                let (x, y, z) = (
                    core.0 as usize + dx,
                    core.1 as usize + dy,
                    core.2 as usize + dz,
                );
                if x >= nx || y >= ny || z >= nz || !v.get(x, y, z) || seen[idx(x, y, z)] {
                    continue;
                }
                seen[idx(x, y, z)] = true;
                stack.push((x, y, z));
            }
        }
    }
    let mut attached = 0u32;
    while let Some((px, py, pz)) = stack.pop() {
        attached += 1;
        let nbs = [
            (px.wrapping_sub(1), py, pz),
            (px + 1, py, pz),
            (px, py.wrapping_sub(1), pz),
            (px, py + 1, pz),
            (px, py, pz.wrapping_sub(1)),
            (px, py, pz + 1),
        ];
        for (ax, ay, az) in nbs {
            if ax >= nx || ay >= ny || az >= nz || !v.get(ax, ay, az) || seen[idx(ax, ay, az)] {
                continue;
            }
            seen[idx(ax, ay, az)] = true;
            stack.push((ax, ay, az));
        }
    }
    (attached, v.solid_count() - attached)
}

/// Ступень A для тома юнита: может ли снятие этого воксела что-то отделить.
/// Та же 26-битная маска, что у мира, — с тем же условием консервативности,
/// но **за пределами тома пусто**, а не твёрдо: снаружи юнита ничего нет.
pub fn cannot_detach(adj: &Adjacency, v: &UnitVol, x: usize, y: usize, z: usize) -> bool {
    let (nx, ny, nz) = (v.nx as usize, v.ny as usize, v.nz as usize);
    let mut m = 0u32;
    for dz in 0..3usize {
        for dy in 0..3usize {
            for dx in 0..3usize {
                let (ax, ay, az) = (x + dx, y + dy, z + dz);
                if ax == 0 || ay == 0 || az == 0 {
                    continue;
                }
                let (ax, ay, az) = (ax - 1, ay - 1, az - 1);
                if ax >= nx || ay >= ny || az >= nz || !v.get(ax, ay, az) {
                    continue;
                }
                m |= 1u32 << (dx + 3 * dy + 9 * dz);
            }
        }
    }
    adj.cannot_split(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::unit_scene::{build_unit, UnitShape};

    /// Быстрый флуд обязан совпадать с обходом по вокселям — и по числу
    /// прикреплённого, и по числу отвалившегося.
    #[test]
    fn matches_voxel_reference() {
        let mut b = CoreBufs::new();
        let mut r = fx::rng::Rng::new(31, 7);
        for shape in UnitShape::ALL {
            for seed in 0..3u64 {
                let mut v = build_unit(shape, seed);
                let core = shape.core();
                for hits in [0usize, 3, 12, 40] {
                    let mut dmg = v.clone();
                    for _ in 0..hits {
                        let (x, y, z) = (
                            r.below(dmg.nx as u32) as usize,
                            r.below(dmg.ny as u32) as usize,
                            r.below(dmg.nz as u32) as usize,
                        );
                        dmg.set(x, y, z, false);
                    }
                    let fast = core_connectivity(&dmg, &mut b, core, 2);
                    let (att, det) = core_connectivity_reference(&dmg, core, 2);
                    assert_eq!(
                        (fast.attached, fast.detached),
                        (att, det),
                        "{} seed={seed} hits={hits}",
                        shape.name()
                    );
                }
                v.set(0, 0, 0, false);
            }
        }
    }


    /// Разрушенное ядро — отдельный ответ, а не «всё отвалилось молча».
    #[test]
    fn destroyed_core_is_reported() {
        let mut v = build_unit(UnitShape::Tank, 1);
        let core = UnitShape::Tank.core();
        for dz in 0..2 {
            for dy in 0..2 {
                for dx in 0..2 {
                    v.set(
                        core.0 as usize + dx,
                        core.1 as usize + dy,
                        core.2 as usize + dz,
                        false,
                    );
                }
            }
        }
        let mut b = CoreBufs::new();
        let rep = core_connectivity(&v, &mut b, core, 2);
        assert!(!rep.core_alive);
        assert_eq!(rep.attached, 0);
    }

    /// Отстреленная гусеница обязана быть посчитана как отвалившаяся.
    #[test]
    fn severed_part_is_detached() {
        let mut v = UnitVol::new(16, 12, 8);
        // Корпус.
        for z in 2..6 {
            for y in 3..9 {
                for x in 4..10 {
                    v.set(x, y, z, true);
                }
            }
        }
        // Выступ, держащийся на одной клетке.
        for z in 2..6 {
            for y in 10..12 {
                for x in 4..10 {
                    v.set(x, y, z, true);
                }
            }
        }
        for z in 2..6 {
            v.set(7, 9, z, true);
        }
        let core = (6u8, 5u8, 3u8);
        let mut b = CoreBufs::new();
        let whole = core_connectivity(&v, &mut b, core, 2);
        assert_eq!(whole.detached, 0, "целый юнит уже разваливается");

        for z in 2..6 {
            v.set(7, 9, z, false);
        }
        let cut = core_connectivity(&v, &mut b, core, 2);
        assert!(cut.detached > 0, "отстреленная часть не отвалилась");
        assert_eq!(cut.attached + cut.detached, v.solid_count());
    }

    /// Колонку можно поставить в очередь **заново**, когда до неё дотянулись
    /// с другой стороны: это нужно для корректности, но означает, что цена
    /// не равна числу колонок. Здесь пришпилена граница повторов — без неё
    /// сильно фрагментированный том мог бы стоить непредсказуемо дорого.
    #[test]
    fn requeue_is_bounded() {
        let mut b = CoreBufs::new();
        let mut r = fx::rng::Rng::new(5, 5);
        let mut worst = 0.0f64;
        for shape in UnitShape::ALL {
            for damage in [0usize, 1, 2, 4] {
                let mut v = build_unit(shape, 4);
                let core = shape.core();
                let take = v.solid_count() as usize * damage / 8;
                for _ in 0..take {
                    let (x, y, z) = (
                        r.below(v.nx as u32) as usize,
                        r.below(v.ny as u32) as usize,
                        r.below(v.nz as u32) as usize,
                    );
                    v.set(x, y, z, false);
                }
                let rep = core_connectivity(&v, &mut b, core, 2);
                if !rep.core_alive {
                    continue;
                }
                let nonempty = v.cols.iter().filter(|&&c| c != 0).count();
                if nonempty == 0 {
                    continue;
                }
                let ratio = rep.cols_touched as f64 / nonempty as f64;
                worst = worst.max(ratio);
                assert!(
                    ratio <= 4.0,
                    "{} damage={damage}: {} постановок в очередь на {nonempty} колонок",
                    shape.name(),
                    rep.cols_touched
                );
            }
        }
        assert!(worst > 0.2, "тест ничего не измерил");
    }

    /// Прикреплённого не может быть больше, чем твёрдого, и сумма обязана
    /// сходиться при любом повреждении.
    #[test]
    fn attached_plus_detached_is_the_whole() {
        let mut b = CoreBufs::new();
        let mut r = fx::rng::Rng::new(11, 2);
        for shape in UnitShape::ALL {
            for damage in 0..6usize {
                let mut v = build_unit(shape, 2);
                let core = shape.core();
                for _ in 0..(damage * 20) {
                    let (x, y, z) = (
                        r.below(v.nx as u32) as usize,
                        r.below(v.ny as u32) as usize,
                        r.below(v.nz as u32) as usize,
                    );
                    v.set(x, y, z, false);
                }
                let rep = core_connectivity(&v, &mut b, core, 2);
                assert_eq!(
                    rep.attached + rep.detached,
                    v.solid_count(),
                    "{} damage={damage}",
                    shape.name()
                );
            }
        }
    }
}
