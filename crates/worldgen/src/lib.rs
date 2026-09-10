//! Генерация мира: **чистая функция от сида и координат**.
//!
//! Ни одного `f32`/`f64`: генератор пишет в состояние симуляции, значит
//! на него распространяется весь запрет `CLAUDE.md`. Шум целочисленный
//! и хеш-основанный, а не накопительный — значение ячейки не зависит
//! ни от порядка обхода, ни от того, сколько соседей уже посчитано.
//! Это то же условие, что у порядок-независимых ядер видеокарты
//! (`docs/13-gpu.md`): результат — функция множества входов.
//!
//! **Что генератор обязан дать по ADR-100:** несколько ресурсов, жилами,
//! богатство растёт с глубиной, на дне лава. Богатая руда — это отдельный
//! материал с большей плотностью, а не модификатор: ресурс считается
//! как объём на плотность, и второго числа на воксель не нужно.
//!
//! Все параметры — **данные** (`Params`), а не константы (`CLAUDE.md`,
//! «данные, не код»). Размер карты тоже данные (ADR-094).

#![forbid(unsafe_code)]

/// Номера материалов. Индекс задаётся **явно** и никогда не выводится
/// из порядка загрузки (ADR-028).
pub mod mat {
    pub type Mat = u16;
    pub const AIR: Mat = 0;
    pub const SOIL: Mat = 1;
    pub const ROCK: Mat = 2;
    pub const DEEP_ROCK: Mat = 3;
    pub const IRON_POOR: Mat = 16;
    pub const IRON: Mat = 17;
    pub const IRON_RICH: Mat = 18;
    pub const COPPER_POOR: Mat = 24;
    pub const COPPER: Mat = 25;
    pub const COPPER_RICH: Mat = 26;
    pub const LAVA: Mat = 40;
}

use mat::Mat;

/// Параметры генерации. Живут в данных; здесь только значения
/// по умолчанию, чтобы движок собирался до появления загрузчика.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub nx: u32,
    pub ny: u32,
    pub nz: u32,
    /// Базовая высота поверхности в вокселях.
    pub base_height: u32,
    /// Размах рельефа.
    pub relief: u32,
    /// Толщина почвы под поверхностью.
    pub soil: u32,
    /// Сколько вокселей от дна занимает лава.
    pub lava_depth: u32,
    /// Ниже этой доли глубины порода становится глубинной.
    pub deep_at: u32,
    /// Порог жилы: чем выше, тем реже руда. 0..=255.
    pub vein_threshold: u32,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            nx: 512,
            ny: 256,
            nz: 512,
            base_height: 140,
            relief: 40,
            soil: 4,
            lava_depth: 6,
            deep_at: 90,
            vein_threshold: 200,
        }
    }
}

/// Хеш координаты. Финализатор splitmix64: лавина полная, значение
/// не зависит ни от чего, кроме входа.
#[inline]
fn hash3(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    let mut h = seed;
    for v in [x, y, z] {
        h ^= v as u64;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 29;
    }
    h
}

/// Значение в узле решётки, 0..=65535.
#[inline]
fn lattice(seed: u64, x: i64, z: i64) -> u32 {
    (hash3(seed, x, 0, z) >> 32) as u32 & 0xFFFF
}

/// Целочисленный value-noise с билинейной интерполяцией.
///
/// Интерполяция линейная, а не сглаженная: сглаживание требует умножения
/// третьей степени, а разница на рельефе не видна под слоем эрозии,
/// которой всё равно нет. Возвращает 0..=65535.
fn value_noise(seed: u64, x: u32, z: u32, period: u32) -> u32 {
    let p = period.max(1) as i64;
    let (gx, gz) = (x as i64 / p, z as i64 / p);
    let (fx, fz) = ((x as i64 % p) as u64, (z as i64 % p) as u64);
    let (a, b) = (lattice(seed, gx, gz) as u64, lattice(seed, gx + 1, gz) as u64);
    let (c, d) = (
        lattice(seed, gx, gz + 1) as u64,
        lattice(seed, gx + 1, gz + 1) as u64,
    );
    let pu = p as u64;
    let top = a * (pu - fx) + b * fx;
    let bot = c * (pu - fx) + d * fx;
    ((top * (pu - fz) + bot * fz) / (pu * pu)) as u32
}

/// Трёхмерный вариант — для жил и пещер.
fn value_noise3(seed: u64, x: u32, y: u32, z: u32, period: u32) -> u32 {
    let p = period.max(1) as i64;
    let (gx, gy, gz) = (x as i64 / p, y as i64 / p, z as i64 / p);
    let (fx, fy, fz) = (
        (x as i64 % p) as u64,
        (y as i64 % p) as u64,
        (z as i64 % p) as u64,
    );
    let pu = p as u64;
    let n = |dx: i64, dy: i64, dz: i64| -> u64 {
        (hash3(seed, gx + dx, gy + dy, gz + dz) >> 32) & 0xFFFF
    };
    let lerp = |a: u64, b: u64, t: u64| a * (pu - t) + b * t;
    let x00 = lerp(n(0, 0, 0), n(1, 0, 0), fx);
    let x10 = lerp(n(0, 1, 0), n(1, 1, 0), fx);
    let x01 = lerp(n(0, 0, 1), n(1, 0, 1), fx);
    let x11 = lerp(n(0, 1, 1), n(1, 1, 1), fx);
    let y0 = (lerp(x00 / pu, x10 / pu, fy)) / pu;
    let y1 = (lerp(x01 / pu, x11 / pu, fy)) / pu;
    ((y0 * (pu - fz) + y1 * fz) / pu) as u32
}

/// Высота поверхности в колонке. Две октавы: крупный рельеф плюс мелкий.
pub fn surface_height(p: &Params, seed: u64, x: u32, z: u32) -> u32 {
    let big = value_noise(seed, x, z, 96) as u64;
    let small = value_noise(seed ^ 0x51ED, x, z, 24) as u64;
    let n = (big * 3 + small) / 4;
    let h = p.base_height as u64 + (n * p.relief as u64 * 2) / 65536 - p.relief as u64;
    h.clamp(8, p.ny as u64 - 8) as u32
}

/// Материал воксела. Чистая функция: ни состояния, ни порядка.
pub fn material(p: &Params, seed: u64, x: u32, y: u32, z: u32) -> Mat {
    if x >= p.nx || y >= p.ny || z >= p.nz {
        return mat::AIR;
    }
    if y < p.lava_depth {
        return mat::LAVA;
    }
    let h = surface_height(p, seed, x, z);
    if y >= h {
        return mat::AIR;
    }
    // Пещеры: пустоты в породе. Не у самой поверхности, иначе рельеф
    // превращается в решето и греди-мешинг перестаёт сливать.
    if y + 12 < h && value_noise3(seed ^ 0xCAFE, x, y, z, 20) > 46000 {
        return mat::AIR;
    }
    if y + p.soil >= h {
        return mat::SOIL;
    }

    // Жилы. Порог по трёхмерному шуму — так руда лежит связными телами,
    // а не рассыпается по вокселю.
    let depth_frac = ((h - y) * 100 / h.max(1)).min(100);
    let vein_iron = value_noise3(seed ^ 0x1401, x, y, z, 14) * 255 / 65536;
    let vein_copper = value_noise3(seed ^ 0x2402, x, y, z, 11) * 255 / 65536;
    // Богатство растёт с глубиной (ADR-100): три материала, а не
    // модификатор одного.
    let grade = |v: u32, poor: Mat, mid: Mat, rich: Mat| -> Option<Mat> {
        if v < p.vein_threshold {
            return None;
        }
        Some(if depth_frac >= p.deep_at {
            rich
        } else if depth_frac >= p.deep_at / 2 {
            mid
        } else {
            poor
        })
    };
    if let Some(m) = grade(vein_iron, mat::IRON_POOR, mat::IRON, mat::IRON_RICH) {
        return m;
    }
    if let Some(m) = grade(vein_copper, mat::COPPER_POOR, mat::COPPER, mat::COPPER_RICH) {
        return m;
    }
    if depth_frac >= p.deep_at {
        mat::DEEP_ROCK
    } else {
        mat::ROCK
    }
}

/// Хеш сгенерированного мира — для golden-файлов и кросс-платформенной
/// сверки. Только `wrapping_*`: та же ассоциативная пара, что у хеша тика.
pub fn world_hash(p: &Params, seed: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    // Полный обход карты 512³ это 134 млн вызовов; для хеша берётся
    // регулярная решётка — она ловит и сдвиг рельефа, и смену материала,
    // и стоит секунды, а не минуты.
    let step = 7;
    let mut z = 0;
    while z < p.nz {
        let mut y = 0;
        while y < p.ny {
            let mut x = 0;
            while x < p.nx {
                let m = material(p, seed, x, y, z) as u64;
                let k = ((z as u64) << 40) ^ ((y as u64) << 20) ^ x as u64;
                h = h.wrapping_add(m.wrapping_mul(31).wrapping_add(1).wrapping_mul(k | 1));
                x += step;
            }
            y += step;
        }
        z += step;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Генератор — чистая функция: тот же сид даёт тот же мир, и порядок
    /// обхода на это не влияет.
    #[test]
    fn same_seed_same_world() {
        let p = Params { nx: 64, ny: 64, nz: 64, ..Default::default() };
        assert_eq!(world_hash(&p, 42), world_hash(&p, 42));
        assert_ne!(world_hash(&p, 42), world_hash(&p, 43));
    }

    /// Обход в обратном порядке даёт те же материалы: значение ячейки
    /// не зависит ни от чего, кроме координат.
    #[test]
    fn material_is_order_independent() {
        let p = Params { nx: 48, ny: 48, nz: 48, ..Default::default() };
        let fwd: Vec<Mat> = (0..p.nz)
            .flat_map(|z| (0..p.ny).flat_map(move |y| (0..p.nx).map(move |x| (x, y, z))))
            .map(|(x, y, z)| material(&p, 7, x, y, z))
            .collect();
        let mut rev: Vec<Mat> = (0..p.nz)
            .rev()
            .flat_map(|z| {
                (0..p.ny)
                    .rev()
                    .flat_map(move |y| (0..p.nx).rev().map(move |x| (x, y, z)))
            })
            .map(|(x, y, z)| material(&p, 7, x, y, z))
            .collect();
        rev.reverse();
        assert_eq!(fwd, rev);
    }

    /// Дно — лава, верх — воздух, между ними порода. Иначе карта
    /// вырождается, и это надо ловить сразу, а не глазами.
    #[test]
    fn world_has_the_three_zones() {
        let p = Params::default();
        let (mut air, mut solid, mut lava, mut ore) = (0, 0, 0, 0);
        for z in (0..p.nz).step_by(17) {
            for x in (0..p.nx).step_by(17) {
                for y in (0..p.ny).step_by(3) {
                    match material(&p, 5, x, y, z) {
                        mat::AIR => air += 1,
                        mat::LAVA => lava += 1,
                        m if (mat::IRON_POOR..mat::LAVA).contains(&m) => ore += 1,
                        _ => solid += 1,
                    }
                }
            }
        }
        assert!(air > 0 && solid > 0 && lava > 0, "{air} {solid} {lava}");
        assert!(ore > 0, "руды нет вовсе");
        // Руда должна быть заметной, но не составлять породу целиком.
        let frac = ore as f64 / (solid + ore) as f64;
        assert!((0.01..0.45).contains(&frac), "доля руды {frac:.3}");
    }

    /// Богатство растёт с глубиной (ADR-100): у дна богатой руды больше,
    /// чем у поверхности.
    #[test]
    fn ore_gets_richer_with_depth() {
        let p = Params::default();
        let (mut rich_deep, mut rich_shallow) = (0, 0);
        for z in (0..p.nz).step_by(11) {
            for x in (0..p.nx).step_by(11) {
                let h = surface_height(&p, 5, x, z);
                for y in p.lava_depth..h {
                    let m = material(&p, 5, x, y, z);
                    let deep = (h - y) * 100 / h.max(1) >= p.deep_at;
                    if m == mat::IRON_RICH || m == mat::COPPER_RICH {
                        if deep {
                            rich_deep += 1;
                        } else {
                            rich_shallow += 1;
                        }
                    }
                }
            }
        }
        assert!(rich_deep > 0, "богатой руды нет на глубине");
        assert_eq!(rich_shallow, 0, "богатая руда встретилась у поверхности");
    }
}
