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
//! Колонка по X — **несколько слов** `u64`. Одного хватало на 64
//! воксела, то есть на шестнадцать метров при каноническом вокселе
//! 0.25 м; крупная машина при таком пределе получала воксель в метр
//! и выходила вчетверо грубее танка — не «большая», а «из кубиков
//! покрупнее» (ADR-179).

use crate::topo::stage_a::Adjacency;

/// Том юнита. Колонка по X занимает `words` слов подряд:
/// `cols[(z * ny + y) * words + x / 64]`, бит `x % 64`.
#[derive(Clone, PartialEq, Eq)]
pub struct UnitVol {
    pub nx: u8,
    pub ny: u8,
    pub nz: u8,
    /// Слов в колонке — `ceil(nx / 64)`, от одного до четырёх.
    ///
    /// Хранится, а не считается на каждом обращении: индекс колонки
    /// берётся в самом горячем цикле проекта.
    pub words: u8,
    pub cols: Vec<u64>,
}

/// Сколько слов может занять колонка. `nx` не больше 255, значит
/// четырёх хватает всегда; константа нужна рабочим колонкам флуда,
/// которые живут на стеке.
pub const MAX_WORDS: usize = 4;

impl UnitVol {
    pub fn new(nx: u8, ny: u8, nz: u8) -> UnitVol {
        assert!(nx > 0);
        let words = nx.div_ceil(64);
        debug_assert!(words as usize <= MAX_WORDS);
        UnitVol {
            nx,
            ny,
            nz,
            words,
            cols: vec![0; ny as usize * nz as usize * words as usize],
        }
    }

    #[inline(always)]
    fn base(&self, y: usize, z: usize) -> usize {
        (z * self.ny as usize + y) * self.words as usize
    }

    /// Колонка целиком — слайс слов, а не одно слово.
    #[inline(always)]
    pub fn col(&self, y: usize, z: usize) -> &[u64] {
        let b = self.base(y, z);
        &self.cols[b..b + self.words as usize]
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> bool {
        self.cols[self.base(y, z) + (x >> 6)] & (1u64 << (x & 63)) != 0
    }

    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, z: usize, v: bool) {
        let i = self.base(y, z) + (x >> 6);
        if v {
            self.cols[i] |= 1u64 << (x & 63);
        } else {
            self.cols[i] &= !(1u64 << (x & 63));
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
    reached: Vec<u64>,
    queue: Vec<u32>,
    head: usize,
    /// Слов в колонке у тома, который считается сейчас.
    words: usize,
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
            words: 1,
        }
    }

    fn reset(&mut self, cols: usize, words: usize) {
        self.words = words;
        self.reached.clear();
        self.reached.resize(cols * words, 0);
        self.queue.clear();
        self.head = 0;
    }

    /// Маски прикреплённого к ядру, по колонке на `[z * ny + y]`.
    ///
    /// Нужны тому, кто спрашивает не «сколько отвалилось», а «жива ли
    /// вот эта подсистема»: подсистема жива, пока хоть один её воксель
    /// **связан с ядром** (ADR-095), и ответ читается пересечением
    /// её маски с этой.
    pub fn reached_cols(&self) -> &[u64] {
        &self.reached
    }

    /// Слов в колонке. Нужно тому, кто читает `reached_cols` рядом
    /// со своими масками.
    pub fn words(&self) -> usize {
        self.words
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

/// Расширить маску пробега внутри колонки: заливка по X делается
/// сдвигами, а не по биту.
///
/// **Единица обязана переходить через границу слова.** Без переноса
/// тело разваливалось бы ровно на 64-м вокселе, и разваливалось бы
/// молча: связность просто не нашла бы дальнюю половину.
///
/// Идёт до неподвижной точки, а не фиксированным числом шагов:
/// порядок обхода фиксирован и последователен, поэтому результат
/// не зависит ни от чего, кроме входа.
#[inline(always)]
fn saturate<const W: usize>(dst: &mut [u64; W], solid: &[u64]) {
    loop {
        let mut changed = false;
        for i in 0..W {
            // Бит 63 младшего слова соседствует с битом 0 старшего.
            let up = if i > 0 { dst[i - 1] >> 63 } else { 0 };
            let dn = if i + 1 < W { dst[i + 1] << 63 } else { 0 };
            let g = (dst[i] | (dst[i] << 1) | up | (dst[i] >> 1) | dn) & solid[i];
            if g != dst[i] {
                dst[i] = g;
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
}

/// Что осталось прикреплённым к ядру.
///
/// Флуд идёт по колонкам: внутри колонки — сдвигами, между колонками —
/// пересечением масок. Стоимость линейна по числу **уцелевших** колонок,
/// а не по объёму тома.
///
/// **Ширина колонки — константа типа, а не число в переменной.**
/// С динамическим числом слов однословное тело — а это всё, что
/// короче шестнадцати метров, то есть почти всё — платило вдвое:
/// 0.642 мкс против 0.280 на танке. Развёртка по четырём вариантам
/// возвращает однословному случаю прежний код (ADR-179).
pub fn core_connectivity(
    v: &UnitVol,
    b: &mut CoreBufs,
    core: (u8, u8, u8),
    core_size: u8,
) -> CoreReport {
    match v.words {
        1 => flood::<1>(v, b, core, core_size),
        2 => flood::<2>(v, b, core, core_size),
        3 => flood::<3>(v, b, core, core_size),
        _ => flood::<4>(v, b, core, core_size),
    }
}

fn flood<const W: usize>(
    v: &UnitVol,
    b: &mut CoreBufs,
    core: (u8, u8, u8),
    core_size: u8,
) -> CoreReport {
    debug_assert_eq!(v.words as usize, W);
    let (ny, nz) = (v.ny as usize, v.nz as usize);
    b.reset(ny * nz, W);

    let total = v.solid_count();
    // Ядро — куб `core_size` от заданной точки как от МИНИМАЛЬНОГО угла
    // (`UnitBody::seat_core`), а не вокруг неё. Уцелело, если хотя бы
    // один его воксель твёрдый: ядро разрушено — юнит выведен из строя,
    // и связность считать уже незачем.
    let cs = core_size as usize;
    let (cx, cy, cz) = (core.0 as usize, core.1 as usize, core.2 as usize);
    let last = v.nx as usize - 1;
    let lo = cx.min(last);
    let hi = (cx + cs - 1).min(last);
    // Рабочие колонки на стеке: ширина известна типу, выделять нечего.
    let mut band = [0u64; W];
    let mut grown = [0u64; W];
    let mut mine = [0u64; W];
    for (i, slot) in band.iter_mut().enumerate() {
        let wb = i * 64;
        *slot = if hi < wb || lo >= wb + 64 {
            0
        } else {
            let a = lo.max(wb) - wb;
            let c = hi.min(wb + 63) - wb;
            if c - a == 63 {
                !0u64
            } else {
                ((1u64 << (c - a + 1)) - 1) << a
            }
        };
    }

    let mut core_mask_any = false;
    for dz in 0..cs {
        for dy in 0..cs {
            let (y, z) = (cy + dy, cz + dz);
            if y >= ny || z >= nz {
                continue;
            }
            let sb = (z * ny + y) * W;
            let mut any = false;
            for (i, g) in grown.iter_mut().enumerate() {
                *g = v.cols[sb + i] & band[i];
                any |= *g != 0;
            }
            if !any {
                continue;
            }
            core_mask_any = true;
            saturate(&mut grown, &v.cols[sb..sb + W]);
            let mut changed = false;
            for (i, g) in grown.iter().enumerate() {
                let after = b.reached[sb + i] | g;
                changed |= after != b.reached[sb + i];
                b.reached[sb + i] = after;
            }
            if changed {
                b.queue.push((z * ny + y) as u32);
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
        mine.copy_from_slice(&b.reached[ci * W..ci * W + W]);
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
            let sb = cj * W;
            // Пробеги соседа, которых касается уже достигнутое.
            let mut any = false;
            for (i, g) in grown.iter_mut().enumerate() {
                *g = v.cols[sb + i] & mine[i];
                any |= *g != 0;
            }
            if !any {
                continue;
            }
            saturate(&mut grown, &v.cols[sb..sb + W]);
            let mut changed = false;
            for (i, g) in grown.iter().enumerate() {
                let after = b.reached[sb + i] | g;
                changed |= after != b.reached[sb + i];
                b.reached[sb + i] = after;
            }
            if changed {
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

    /// Тело длиннее одного слова: связность обязана переходить через
    /// границу 64-го воксела.
    ///
    /// Ни одно тело набора до этого не было длиннее 56 вокселей,
    /// поэтому многословный путь не проверялся **ничем**: тесты
    /// проходили бы и без переноса между словами (ADR-179).
    ///
    /// Случай построен, а не выпрошен: брусок длиной 200, ядро
    /// в начале. Без переноса заливка встанет на 63-м вокселе,
    /// и три четверти бруска окажутся «отвалившимися».
    #[test]
    fn a_body_longer_than_one_word_stays_whole() {
        let mut v = UnitVol::new(200, 5, 5);
        assert_eq!(v.words, 4, "брусок обязан занять четыре слова");
        for z in 1..4 {
            for y in 1..4 {
                for x in 0..200 {
                    v.set(x, y, z, true);
                }
            }
        }
        let mut b = CoreBufs::new();
        let rep = core_connectivity(&v, &mut b, (0, 1, 1), 2);
        let (att, det) = core_connectivity_reference(&v, (0, 1, 1), 2);
        assert!(rep.core_alive);
        assert_eq!(rep.attached, att, "быстрый флуд разошёлся с эталоном");
        assert_eq!(rep.detached, det);
        assert_eq!(rep.detached, 0, "целый брусок не может отваливаться");
        assert_eq!(rep.attached, 9 * 200);
    }

    /// Разрез **внутри** дальнего слова отделяет ровно хвост.
    ///
    /// Парный к предыдущему: тот ловит недотянувшийся перенос, этот —
    /// протёкший. Перенос, который не смотрит на твёрдость соседнего
    /// слова, склеил бы половинки через разрез.
    #[test]
    fn a_cut_across_a_word_boundary_detaches_the_tail() {
        let mut v = UnitVol::new(200, 5, 5);
        for z in 1..4 {
            for y in 1..4 {
                for x in 0..200 {
                    // Разрез на 128-м вокселе — ровно на границе слов.
                    if x == 128 {
                        continue;
                    }
                    v.set(x, y, z, true);
                }
            }
        }
        let mut b = CoreBufs::new();
        let rep = core_connectivity(&v, &mut b, (0, 1, 1), 2);
        let (att, det) = core_connectivity_reference(&v, (0, 1, 1), 2);
        assert_eq!(rep.attached, att, "быстрый флуд разошёлся с эталоном");
        assert_eq!(rep.detached, det);
        assert_eq!(rep.attached, 9 * 128, "прикрепилась не голова бруска");
        assert_eq!(rep.detached, 9 * 71, "отвалился не хвост");
    }

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
