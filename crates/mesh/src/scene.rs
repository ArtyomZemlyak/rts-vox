//! Сцены чанка. Смысл файла — в том, что порог бенчмарка обязан нести
//! параметры сцены рядом с числом (`../../docs/10-budgets.md`). Здесь
//! параметры — это код, и число воспроизводимо по сиду.

use crate::{Chunk, Mat, AIR, CS};
use fx::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    /// Гладкий рельеф-heightmap. Опорный случай всей опубликованной литературы.
    Smooth,
    /// Он же, но со слоями материалов: дёрн, порода, руда.
    Layered,
    /// Гладкий рельеф со связной каверной — наша копка.
    Cavern,
    /// Слоистый рельеф со связной каверной: подкоп **вскрывает слои**,
    /// и на стенках хода оказываются три материала сразу. Это единственная
    /// сцена, где разделение по материалам чего-то стоит.
    LayeredCavern,
    /// Гладкий рельеф с разбросанными одиночными снятиями — наш бой (ADR-035).
    Scatter,
    /// Каверна плюс обстрел: копка и бой одновременно.
    CavernAndScatter,
    /// Сфера. Есть в опубликованных замерах, держим для сверки.
    Sphere,
    /// Сплошная порода с процентом случайных снятий.
    Random5,
    Random30,
    /// Шахматка — абсолютный потолок.
    Checker,
}

impl Scene {
    pub fn name(self) -> &'static str {
        match self {
            Scene::Smooth => "smooth",
            Scene::Layered => "layered",
            Scene::Cavern => "cavern",
            Scene::LayeredCavern => "layered-cavern",
            Scene::Scatter => "scatter",
            Scene::CavernAndScatter => "cavern+scatter",
            Scene::Sphere => "sphere",
            Scene::Random5 => "random-5%",
            Scene::Random30 => "random-30%",
            Scene::Checker => "checker",
        }
    }

    pub const ALL: [Scene; 10] = [
        Scene::Smooth,
        Scene::Layered,
        Scene::Cavern,
        Scene::LayeredCavern,
        Scene::Scatter,
        Scene::CavernAndScatter,
        Scene::Sphere,
        Scene::Random5,
        Scene::Random30,
        Scene::Checker,
    ];
}

/// Значение-шум с билинейной интерполяцией. Целочисленный, детерминированный,
/// без плавающей точки: это же будет и в генераторе мира.
fn value_noise(seed: u64, x: i32, z: i32, period: i32) -> i32 {
    let hash = |a: i32, b: i32| -> i32 {
        let mut h = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((a as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
            .wrapping_add((b as i64 as u64).wrapping_mul(0x94D0_49BB_1331_11EB));
        h ^= h >> 31;
        h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        ((h >> 40) & 0xFFFF) as i32
    };
    let (x0, z0) = (x.div_euclid(period), z.div_euclid(period));
    let (fx, fz) = (x.rem_euclid(period), z.rem_euclid(period));
    let (h00, h10) = (hash(x0, z0), hash(x0 + 1, z0));
    let (h01, h11) = (hash(x0, z0 + 1), hash(x0 + 1, z0 + 1));
    let a = h00 + (h10 - h00) * fx / period;
    let b = h01 + (h11 - h01) * fx / period;
    a + (b - a) * fz / period
}

fn height_at(seed: u64, x: usize, z: usize) -> usize {
    let (xi, zi) = (x as i32, z as i32);
    let n = value_noise(seed, xi, zi, 48) / 2 + value_noise(seed ^ 0x55, xi, zi, 16) / 4;
    (20 + n * 24 / 65536).clamp(4, CS as i32 - 4) as usize
}

fn terrain(seed: u64, layered: bool) -> Chunk {
    let mut c = Chunk::new();
    for z in 0..=CS + 1 {
        for x in 0..=CS + 1 {
            let h = height_at(seed, x, z);
            for y in 1..=h.min(CS) {
                let m: Mat = if !layered {
                    1
                } else if y + 2 >= h {
                    1 // дёрн
                } else if y * 3 < h {
                    3 // руда глубже
                } else {
                    2 // порода
                };
                c.set(x, y, z, m);
            }
        }
    }
    c
}

/// Связная каверна: ход, прокопанный от поверхности вниз и вбок.
fn carve_cavern(c: &mut Chunk, r: &mut Rng, seed: u64) {
    let mut x = (CS / 3) as i32;
    let mut z = (CS / 2) as i32;
    let mut y = height_at(seed, x as usize, z as usize) as i32;
    let radius = 2i32;
    for step in 0..CS * 3 {
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dy * dy + dz * dz > radius * radius {
                        continue;
                    }
                    let (xx, yy, zz) = (x + dx, y + dy, z + dz);
                    if xx < 1 || yy < 1 || zz < 1 {
                        continue;
                    }
                    let (xx, yy, zz) = (xx as usize, yy as usize, zz as usize);
                    if xx > CS || yy > CS || zz > CS {
                        continue;
                    }
                    c.set(xx, yy, zz, AIR);
                }
            }
        }
        // Вниз чаще, чем вбок: это шахта, а не пещера.
        match r.below(6) {
            0 | 1 => y -= 1,
            2 => x += 1,
            3 => x -= 1,
            4 => z += 1,
            _ => z -= 1,
        }
        x = x.clamp(2, CS as i32 - 2);
        z = z.clamp(2, CS as i32 - 2);
        y = y.clamp(3, CS as i32 - 1);
        if step % 40 == 39 {
            // Отвод вбок: штрек.
            x += if r.below(2) == 0 { 6 } else { -6 };
            x = x.clamp(2, CS as i32 - 2);
        }
    }
}

/// Разбросанные одиночные снятия по поверхности — попадания.
fn scatter_hits(c: &mut Chunk, r: &mut Rng, seed: u64, hits: usize) {
    for _ in 0..hits {
        let x = r.below(CS as u32) as usize + 1;
        let z = r.below(CS as u32) as usize + 1;
        let h = height_at(seed, x, z);
        let depth = r.below(3) as usize;
        let y = h.saturating_sub(depth).max(1);
        // Попадание снимает воксель и пару соседей — это не идеальный куб.
        c.set(x, y, z, AIR);
        if r.below(2) == 0 && x < CS {
            c.set(x + 1, y, z, AIR);
        }
        if r.below(2) == 0 && z < CS {
            c.set(x, y, z + 1, AIR);
        }
    }
}

pub fn build(scene: Scene, seed: u64) -> Chunk {
    let mut r = Rng::new(seed, scene as u64);
    match scene {
        Scene::Smooth => terrain(seed, false),
        Scene::Layered => terrain(seed, true),
        Scene::Cavern => {
            let mut c = terrain(seed, false);
            carve_cavern(&mut c, &mut r, seed);
            c
        }
        Scene::LayeredCavern => {
            let mut c = terrain(seed, true);
            carve_cavern(&mut c, &mut r, seed);
            c
        }
        Scene::Scatter => {
            let mut c = terrain(seed, false);
            scatter_hits(&mut c, &mut r, seed, 400);
            c
        }
        Scene::CavernAndScatter => {
            let mut c = terrain(seed, false);
            carve_cavern(&mut c, &mut r, seed);
            scatter_hits(&mut c, &mut r, seed, 400);
            c
        }
        Scene::Sphere => {
            let mut c = Chunk::new();
            let rad = (CS / 2 - 1) as i32;
            let ctr = (CS / 2 + 1) as i32;
            for y in 1..=CS {
                for z in 1..=CS {
                    for x in 1..=CS {
                        let (dx, dy, dz) = (x as i32 - ctr, y as i32 - ctr, z as i32 - ctr);
                        if dx * dx + dy * dy + dz * dz <= rad * rad {
                            c.set(x, y, z, 1);
                        }
                    }
                }
            }
            c
        }
        Scene::Random5 | Scene::Random30 => {
            let pct = if scene == Scene::Random5 { 5 } else { 30 };
            let mut c = Chunk::new();
            for y in 1..=CS {
                for z in 1..=CS {
                    for x in 1..=CS {
                        if r.below(100) >= pct {
                            c.set(x, y, z, 1);
                        }
                    }
                }
            }
            c
        }
        Scene::Checker => {
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
    }
}
