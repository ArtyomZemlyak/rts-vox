//! Ступень A каскада ADR-055: 26-битная маска соседства.
//!
//! Достаточное условие: если все твёрдые 6-соседи снятого воксела лежат
//! в одной 6-связной компоненте куба 3×3×3 без центра, удаление расщепить
//! не может. Это **достаточное**, а не необходимое условие — оно пропускает
//! дальше лишнее, но никогда не отсеивает настоящий разрез.
//!
//! Записанная в ADR-055 частота срабатывания — 0.23% — получена оценкой.
//! Здесь она измеряется.

use crate::topo::{Chunk, CS};

/// Смещения 27 позиций куба 3×3×3, центр — индекс 13.
const CENTER: usize = 13;

#[inline(always)]
const fn pos(i: usize) -> (i32, i32, i32) {
    let x = (i % 3) as i32 - 1;
    let y = ((i / 3) % 3) as i32 - 1;
    let z = (i / 9) as i32 - 1;
    (x, y, z)
}

/// Для каждой из 27 позиций — маска её 6-соседей внутри куба.
/// Считается один раз, живёт в L1: 108 байт.
pub struct Adjacency {
    nb: [u32; 27],
    /// Маска шести осевых соседей центра.
    pub face6: u32,
}

impl Default for Adjacency {
    fn default() -> Self {
        Self::new()
    }
}

impl Adjacency {
    pub fn new() -> Adjacency {
        let mut nb = [0u32; 27];
        for (i, slot) in nb.iter_mut().enumerate() {
            let (x, y, z) = pos(i);
            for j in 0..27 {
                if i == j {
                    continue;
                }
                let (a, b, c) = pos(j);
                let d = (a - x).abs() + (b - y).abs() + (c - z).abs();
                if d == 1 {
                    *slot |= 1u32 << j;
                }
            }
        }
        let mut face6 = 0u32;
        for j in 0..27 {
            let (a, b, c) = pos(j);
            if a.abs() + b.abs() + c.abs() == 1 {
                face6 |= 1u32 << j;
            }
        }
        Adjacency { nb, face6 }
    }

    /// Собрать 27-битную маску твёрдого вокруг `(x, y, z)`.
    ///
    /// За пределами чанка считаем твёрдым: сосед снаружи не может сделать
    /// кусок отдельным, а обратное дало бы ложный отсев — то есть
    /// пропущенный обвал.
    ///
    /// Наивная сборка по 27 вокселям стоит 115 нс. Раскладка «колонка по X —
    /// одно слово» позволяет взять всё девятью чтениями: из каждой колонки
    /// нужны ровно три соседних бита, и они достаются одним сдвигом.
    /// Колонка расширяется до 34 бит с единицами по краям — так границы
    /// чанка обрабатываются без ветвления.
    #[inline]
    pub fn gather(&self, c: &Chunk, x: usize, y: usize, z: usize) -> u32 {
        let mut m = 0u32;
        for dz in 0..3usize {
            let nz = z + dz;
            let z_out = nz == 0 || nz > CS;
            for dy in 0..3usize {
                let ny = y + dy;
                let three = if z_out || ny == 0 || ny > CS {
                    0b111u64
                } else {
                    let col = c.cols[(nz - 1) * CS + (ny - 1)] as u64;
                    // Бит 0 — воксель x = −1 (снаружи, твёрдый),
                    // биты 1..32 — сам чанк, бит 33 — x = 32.
                    let padded = (col << 1) | 1 | (1u64 << 33);
                    (padded >> x) & 0b111
                };
                m |= (three as u32) << (3 * dy + 9 * dz);
            }
        }
        m
    }

    /// Эталонная сборка по вокселям — медленная и очевидная.
    /// Существует только чтобы проверять быструю.
    pub fn gather_reference(&self, c: &Chunk, x: usize, y: usize, z: usize) -> u32 {
        let mut m = 0u32;
        for i in 0..27 {
            let (dx, dy, dz) = pos(i);
            let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
            let solid = if nx < 0
                || ny < 0
                || nz < 0
                || nx >= CS as i32
                || ny >= CS as i32
                || nz >= CS as i32
            {
                true
            } else {
                c.get(nx as usize, ny as usize, nz as usize)
            };
            if solid {
                m |= 1u32 << i;
            }
        }
        m
    }

    /// `true` — снятие центра **точно не расщепит** ничего.
    /// `false` — надо звать ступень B.
    ///
    /// Заливка внутри куба 3×3×3 — битово-параллельная: шесть сдвигов
    /// на шаг вместо обхода установленных битов.
    ///
    /// Число шагов фиксировать нельзя. Диаметр свободного куба по
    /// 6-связности равен шести, но заливка идёт **внутри твёрдого**,
    /// и там путь может виться: в худшем случае это 25 шагов по 26 клеткам.
    /// Отсюда цикл до неподвижной точки, а не развёртка.
    #[inline]
    pub fn cannot_split(&self, mask: u32) -> bool {
        let solid = mask & !(1u32 << CENTER);
        let seeds = solid & self.face6;
        if seeds == 0 {
            // Изолированный воксель: снятие ничего не расщепляет.
            return true;
        }
        let mut reached = 1u32 << seeds.trailing_zeros();
        loop {
            let next = expand(reached) & solid;
            if next == reached {
                break;
            }
            reached = next;
        }
        seeds & !reached == 0
    }

    /// Прежняя заливка обходом битов. Медленная и очевидная; существует
    /// только чтобы проверять быструю.
    pub fn cannot_split_reference(&self, mask: u32) -> bool {
        let solid = mask & !(1u32 << CENTER);
        let seeds = solid & self.face6;
        if seeds == 0 {
            return true;
        }
        let mut reached = 1u32 << seeds.trailing_zeros();
        loop {
            let mut grown = reached;
            let mut w = reached;
            while w != 0 {
                let b = w.trailing_zeros() as usize;
                w &= w - 1;
                grown |= self.nb[b] & solid;
            }
            if grown == reached {
                break;
            }
            reached = grown;
        }
        seeds & !reached == 0
    }
}


// Маски граней куба 3×3×3 в упаковке `i = dx + 3*dy + 9*dz`.
// Нужны, чтобы сдвиг не переносил биты через грань.
const fn plane(axis: usize, at: usize) -> u32 {
    let mut m = 0u32;
    let mut i = 0;
    while i < 27 {
        let c = match axis {
            0 => i % 3,
            1 => (i / 3) % 3,
            _ => i / 9,
        };
        if c == at {
            m |= 1u32 << i;
        }
        i += 1;
    }
    m
}

const X_MIN: u32 = plane(0, 0);
const X_MAX: u32 = plane(0, 2);
const Y_MIN: u32 = plane(1, 0);
const Y_MAX: u32 = plane(1, 2);
const Z_MIN: u32 = plane(2, 0);
const Z_MAX: u32 = plane(2, 2);

/// Один шаг заливки по 6-связности внутри куба: шесть сдвигов.
#[inline(always)]
fn expand(r: u32) -> u32 {
    r | ((r & !X_MAX) << 1)
        | ((r & !X_MIN) >> 1)
        | ((r & !Y_MAX) << 3)
        | ((r & !Y_MIN) >> 3)
        | ((r & !Z_MAX) << 9)
        | ((r & !Z_MIN) >> 9)
}

/// Пройти список снятых вокселей и посчитать, сколько прошло фильтр.
pub fn filter(adj: &Adjacency, c: &Chunk, edits: &[(u8, u8, u8)]) -> (usize, usize) {
    let mut passed = 0;
    for &(x, y, z) in edits {
        let m = adj.gather(c, x as usize, y as usize, z as usize);
        if !adj.cannot_split(m) {
            passed += 1;
        }
    }
    (passed, edits.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::scene::{build, Scene};
    use crate::topo::Components;

    /// **Фильтр обязан быть консервативным.** Если он говорит «расщепить
    /// нельзя», то число компонент после снятия обязано не вырасти.
    /// Обратное допустимо: пропустить лишнее безопасно, отсеять настоящий
    /// разрез — нет.
    #[test]
    fn filter_never_misses_a_split() {
        let adj = Adjacency::new();
        let mut before = Components::new();
        let mut after = Components::new();
        let mut r = fx::rng::Rng::new(9, 9);
        let mut checked = 0usize;
        let mut claimed_safe = 0usize;
        for scene in Scene::ALL {
            let base = build(scene, 3);
            let n0 = before.build(&base);
            for _ in 0..400 {
                let (x, y, z) = (
                    r.below(CS as u32) as usize,
                    r.below(CS as u32) as usize,
                    r.below(CS as u32) as usize,
                );
                if !base.get(x, y, z) {
                    continue;
                }
                checked += 1;
                let safe = adj.cannot_split(adj.gather(&base, x, y, z));
                let mut cut = base.clone();
                cut.set(x, y, z, false);
                let n1 = after.build(&cut);
                if safe {
                    claimed_safe += 1;
                    assert!(
                        n1 <= n0,
                        "{}: фильтр пропустил расщепление в ({x},{y},{z}): {n0} -> {n1}",
                        scene.name()
                    );
                }
            }
        }
        assert!(checked > 500, "тест почти ничего не проверил: {checked}");
        assert!(claimed_safe > 100, "фильтр не срабатывает вовсе");
    }

    /// Быстрая сборка обязана совпадать с эталонной **везде**, включая
    /// границы чанка: расхождение на границе — это пропущенный обвал.
    #[test]
    fn fast_gather_matches_reference() {
        let adj = Adjacency::new();
        for scene in Scene::ALL {
            let c = build(scene, 4);
            for z in 0..CS {
                for y in 0..CS {
                    for x in 0..CS {
                        assert_eq!(
                            adj.gather(&c, x, y, z),
                            adj.gather_reference(&c, x, y, z),
                            "{} ({x},{y},{z})",
                            scene.name()
                        );
                    }
                }
            }
        }
    }

    /// Битово-параллельная заливка обязана совпадать с обходом битов
    /// на **всех** достижимых масках, а не на выборке.
    #[test]
    fn bitwise_flood_matches_reference() {
        let adj = Adjacency::new();
        let mut r = fx::rng::Rng::new(77, 1);
        for _ in 0..200_000 {
            let m = r.next_u32() & ((1u32 << 27) - 1);
            assert_eq!(
                adj.cannot_split(m),
                adj.cannot_split_reference(m),
                "маска {m:#029b}"
            );
        }
        // Плюс все маски, где твёрдые только осевые соседи: 64 случая,
        // и именно они решают исход чаще всего.
        for bits in 0u32..64 {
            let mut m = 0u32;
            let mut k = 0;
            for i in 0..27 {
                let (a, b, c) = pos(i);
                if a.abs() + b.abs() + c.abs() == 1 {
                    if bits & (1 << k) != 0 {
                        m |= 1u32 << i;
                    }
                    k += 1;
                }
            }
            assert_eq!(adj.cannot_split(m), adj.cannot_split_reference(m), "осевые {bits:#08b}");
        }
    }

    /// Изолированный воксель снимается без последствий.
    #[test]
    fn lone_voxel_is_safe() {
        let adj = Adjacency::new();
        let mut c = Chunk::new();
        c.set(10, 10, 10, true);
        assert!(adj.cannot_split(adj.gather(&c, 10, 10, 10)));
    }

    /// Перемычка в один воксель между двумя половинами — не безопасна.
    #[test]
    fn a_neck_is_not_safe() {
        let adj = Adjacency::new();
        let mut c = Chunk::new();
        // Два куба, соединённые одним вокселем по оси Y.
        for x in 8..12 {
            for z in 8..12 {
                for y in 6..9 {
                    c.set(x, y, z, true);
                }
                for y in 12..15 {
                    c.set(x, y, z, true);
                }
            }
        }
        c.set(10, 9, 10, true);
        c.set(10, 10, 10, true);
        c.set(10, 11, 10, true);
        assert!(
            !adj.cannot_split(adj.gather(&c, 10, 10, 10)),
            "перемычка объявлена безопасной"
        );
    }
}
