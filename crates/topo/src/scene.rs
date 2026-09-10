//! Чанки 32³ под нашу механику. Сцена задана кодом, а не описанием.

use crate::{Chunk, CS};
use fx::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    /// Сплошная порода. Одна компонента, худший случай для пометки по вокселям.
    Solid,
    /// Рельеф-heightmap: чанк с поверхностью внутри.
    Terrain,
    /// Рельеф с прокопанным ходом — наша копка.
    Tunnel,
    /// Рельеф с разбросанными попаданиями — наш бой.
    Scatter,
    /// Ход и обстрел вместе.
    TunnelAndScatter,
    /// Пещеристый мир: несколько несвязанных полостей.
    Cavey,
    /// Подкоп: горизонтальный разрез почти через весь чанк.
    Undercut,
    /// Обломки: много мелких несвязанных кусков.
    Debris,
}

impl Scene {
    pub fn name(self) -> &'static str {
        match self {
            Scene::Solid => "solid",
            Scene::Terrain => "terrain",
            Scene::Tunnel => "tunnel",
            Scene::Scatter => "scatter",
            Scene::TunnelAndScatter => "tunnel+scatter",
            Scene::Cavey => "cavey",
            Scene::Undercut => "undercut",
            Scene::Debris => "debris",
        }
    }

    pub const ALL: [Scene; 8] = [
        Scene::Solid,
        Scene::Terrain,
        Scene::Tunnel,
        Scene::Scatter,
        Scene::TunnelAndScatter,
        Scene::Cavey,
        Scene::Undercut,
        Scene::Debris,
    ];
}

fn hash2(seed: u64, a: i32, b: i32) -> i32 {
    let mut h = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((a as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add((b as i64 as u64).wrapping_mul(0x94D0_49BB_1331_11EB));
    h ^= h >> 31;
    h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((h >> 40) & 0xFFFF) as i32
}

/// Высота твёрдого в колонке `(y, z)`. Ось X — вертикаль, как в раскладке.
fn height(seed: u64, y: usize, z: usize) -> usize {
    let n = hash2(seed, y as i32 / 8, z as i32 / 8);
    let f = hash2(seed ^ 0x11, y as i32 / 3, z as i32 / 3) / 4;
    (14 + (n + f) * 12 / 65536).clamp(4, CS as i32 - 3) as usize
}

fn terrain(seed: u64) -> Chunk {
    let mut c = Chunk::new();
    for z in 0..CS {
        for y in 0..CS {
            let h = height(seed, y, z);
            c.cols[z * CS + y] = if h >= 32 { u32::MAX } else { (1u32 << h) - 1 };
        }
    }
    c
}

fn carve_tunnel(c: &mut Chunk, r: &mut Rng, seed: u64) {
    let mut y = (CS / 3) as i32;
    let mut z = (CS / 2) as i32;
    let mut x = height(seed, y as usize, z as usize) as i32 - 1;
    for _ in 0..CS * 3 {
        for dx in -1i32..=1 {
            for dz in -1i32..=1 {
                for dy in -1i32..=1 {
                    let (xx, yy, zz) = (x + dx, y + dy, z + dz);
                    if xx < 0 || yy < 0 || zz < 0 || xx >= CS as i32 || yy >= CS as i32 || zz >= CS as i32
                    {
                        continue;
                    }
                    c.set(xx as usize, yy as usize, zz as usize, false);
                }
            }
        }
        match r.below(6) {
            0 | 1 => x -= 1,
            2 => y += 1,
            3 => y -= 1,
            4 => z += 1,
            _ => z -= 1,
        }
        x = x.clamp(2, CS as i32 - 2);
        y = y.clamp(1, CS as i32 - 2);
        z = z.clamp(1, CS as i32 - 2);
    }
}

fn scatter(c: &mut Chunk, r: &mut Rng, seed: u64, hits: usize) {
    for _ in 0..hits {
        let y = r.below(CS as u32) as usize;
        let z = r.below(CS as u32) as usize;
        let h = height(seed, y, z);
        let x = h.saturating_sub(r.below(3) as usize).min(CS - 1);
        c.set(x, y, z, false);
        if r.below(2) == 0 && y + 1 < CS {
            c.set(x, y + 1, z, false);
        }
    }
}

pub fn build(scene: Scene, seed: u64) -> Chunk {
    let mut r = Rng::new(seed, scene as u64);
    match scene {
        Scene::Solid => {
            let mut c = Chunk::new();
            c.cols.iter_mut().for_each(|v| *v = u32::MAX);
            c
        }
        Scene::Terrain => terrain(seed),
        Scene::Tunnel => {
            let mut c = terrain(seed);
            carve_tunnel(&mut c, &mut r, seed);
            c
        }
        Scene::Scatter => {
            let mut c = terrain(seed);
            scatter(&mut c, &mut r, seed, 120);
            c
        }
        Scene::TunnelAndScatter => {
            let mut c = terrain(seed);
            carve_tunnel(&mut c, &mut r, seed);
            scatter(&mut c, &mut r, seed, 120);
            c
        }
        Scene::Cavey => {
            let mut c = Chunk::new();
            c.cols.iter_mut().for_each(|v| *v = u32::MAX);
            for _ in 0..7 {
                let (cx, cy, cz) = (
                    r.range(4, CS as i32 - 5),
                    r.range(4, CS as i32 - 5),
                    r.range(4, CS as i32 - 5),
                );
                let rad = r.range(3, 6);
                for x in 0..CS as i32 {
                    for y in 0..CS as i32 {
                        for z in 0..CS as i32 {
                            let (dx, dy, dz) = (x - cx, y - cy, z - cz);
                            if dx * dx + dy * dy + dz * dz <= rad * rad {
                                c.set(x as usize, y as usize, z as usize, false);
                            }
                        }
                    }
                }
            }
            c
        }
        Scene::Undercut => {
            // Сплошная порода с горизонтальным разрезом, не доходящим
            // до одной стенки: ступень A обязана это пропустить дальше.
            let mut c = Chunk::new();
            c.cols.iter_mut().for_each(|v| *v = u32::MAX);
            for z in 0..CS {
                for y in 0..CS - 2 {
                    c.set(16, y, z, false);
                }
            }
            c
        }
        Scene::Debris => {
            let mut c = Chunk::new();
            for _ in 0..40 {
                let (cx, cy, cz) = (
                    r.range(1, CS as i32 - 3),
                    r.range(1, CS as i32 - 3),
                    r.range(1, CS as i32 - 3),
                );
                let s = r.range(1, 2);
                for x in cx..=(cx + s).min(CS as i32 - 1) {
                    for y in cy..=(cy + s).min(CS as i32 - 1) {
                        for z in cz..=(cz + s).min(CS as i32 - 1) {
                            c.set(x as usize, y as usize, z as usize, true);
                        }
                    }
                }
            }
            c
        }
    }
}
