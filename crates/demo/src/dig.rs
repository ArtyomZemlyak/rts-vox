//! Копатели: то, что превращает мир в изрытый.
//!
//! Модель проверена стендом и один раз оказалась неверной: **фаза
//! продвижения обязана быть разной у разных копателей** (ADR-090).
//! При одинаковой стартовой фазе стенд сам собой работает трёхтиковой
//! пачкой — два тика пустых, третий втрое толще, — и потиковые числа
//! занижены вдвое, а пиковые завышены.
//!
//! Забой **органический** (ADR-098): грызёт по вокселю, стены рваные.
//! Это дороже осевого в 5.37 раза по квадам, и это выбор владельца.

use fx::rng::Rng;
use voxel::World;

pub struct Digger {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    dx: i8,
    dz: i8,
    /// Тиков на месте. Продвижение по `age % 3`.
    age: u32,
}

pub struct Dig {
    pub diggers: Vec<Digger>,
    pub radius: i32,
    rng: Rng,
}

impl Dig {
    /// Копатели, сбитые в `clusters` забоев. Равномерно рассыпанные —
    /// это не копка, а фронт по всей площади: горячим становится весь
    /// мир по построению (ADR-024).
    pub fn clustered(
        w: &World,
        surface: impl Fn(u32, u32) -> u32,
        n: usize,
        radius: i32,
        clusters: usize,
        seed: u64,
    ) -> Dig {
        let mut rng = Rng::new(seed, 0xD16);
        let centres: Vec<(u32, u32)> = (0..clusters.max(1))
            .map(|_| (rng.below(w.nx), rng.below(w.nz)))
            .collect();
        let mut diggers = Vec::with_capacity(n);
        let mut guard = 0usize;
        while diggers.len() < n && guard < n * 200 {
            guard += 1;
            let (cx, cz) = centres[rng.below(centres.len() as u32) as usize];
            let x = (cx as i64 + rng.range(-24, 24) as i64).clamp(1, w.nx as i64 - 2) as u32;
            let z = (cz as i64 + rng.range(-24, 24) as i64).clamp(1, w.nz as i64 - 2) as u32;
            let y = surface(x, z);
            if y < 16 || y + 2 >= w.ny {
                continue;
            }
            let (dx, dz) = match rng.below(4) {
                0 => (1i8, 0i8),
                1 => (-1, 0),
                2 => (0, 1),
                _ => (0, -1),
            };
            diggers.push(Digger {
                x,
                y: y - 1,
                z,
                dx,
                dz,
                // Стартовая фаза разная — иначе стенд резонирует (ADR-090).
                age: rng.below(3),
            });
        }
        Dig { diggers, radius, rng }
    }

    /// Один тик копки: поставить правки в очередь мира.
    /// Дренаж и потолок — дело мира, а не копателя.
    pub fn step(&mut self, w: &mut World) {
        let r = self.radius;
        for i in 0..self.diggers.len() {
            let d = &self.diggers[i];
            let (dx0, dy0, dz0) = (d.x as i64, d.y as i64, d.z as i64);
            for dy in -r..=r {
                for dz in -r..=r {
                    for dx in -r..=r {
                        let d2 = dx * dx + dy * dy + dz * dz;
                        // Органический забой: дрожание радиуса от мировой
                        // позиции, а не от порядка обхода — иначе стена
                        // менялась бы при каждом проходе.
                        let h = (dx0 * 0x9E37)
                            ^ (dz0 * 0x85EB)
                            ^ ((dx as i64 + 8) * 0xC2B2)
                            ^ ((dy as i64 + 8) * 0x27D4)
                            ^ ((dz as i64 + 8) * 0x1656);
                        let jitter = (h.unsigned_abs() % 3) as i32;
                        if d2 > r * r + jitter - 1 {
                            continue;
                        }
                        let (x, y, z) = (dx0 + dx as i64, dy0 + dy as i64, dz0 + dz as i64);
                        if x < 0 || y < 1 || z < 0 {
                            continue;
                        }
                        let (x, y, z) = (x as u32, y as u32, z as u32);
                        if w.solid(x, y, z) {
                            w.queue_dig(x, y, z);
                        }
                    }
                }
            }
            let d = &mut self.diggers[i];
            d.age += 1;
            if d.age % 3 == 0 {
                let nx = d.x as i64 + d.dx as i64;
                let nz = d.z as i64 + d.dz as i64;
                if nx >= 1 && nz >= 1 && (nx as u32) < w.nx - 1 && (nz as u32) < w.nz - 1 {
                    d.x = nx as u32;
                    d.z = nz as u32;
                }
                if d.y > 3 && d.age % 6 == 0 {
                    d.y -= 1;
                }
            }
            if self.rng.below(64) == 0 {
                let (dx, dz) = match self.rng.below(4) {
                    0 => (1i8, 0i8),
                    1 => (-1, 0),
                    2 => (0, 1),
                    _ => (0, -1),
                };
                let d = &mut self.diggers[i];
                d.dx = dx;
                d.dz = dz;
            }
        }
    }
}
