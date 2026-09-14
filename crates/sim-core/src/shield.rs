//! Предикат «точка внутри щита» (ADR-040).
//!
//! Сам предикат — сравнение квадратов, две-три инструкции. Вопрос бюджета
//! не в нём, а в **числе пар**: снаряд против щита. Наивно это произведение,
//! и оно растёт как квадрат.

/// Щит: сфера в мировых единицах.
#[derive(Clone, Copy, Debug)]
pub struct Shield {
    pub x: i32,
    pub z: i32,
    pub y: i32,
    pub r: i32,
    pub owner: u8,
    /// Заряд: щит с нулевым зарядом не ловит.
    pub charge: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub owner: u8,
}

/// Предикат. Ни корня, ни деления — сравнение квадратов (ADR-071).
#[inline(always)]
pub fn inside(s: &Shield, p: &Point) -> bool {
    if s.charge <= 0 || s.owner == p.owner {
        return false;
    }
    let (dx, dy, dz) = (p.x - s.x, p.y - s.y, p.z - s.z);
    // i64, потому что при радиусе в сотни метров квадрат вылезает за i32
    // на разности координат карты.
    let d2 = (dx as i64) * (dx as i64) + (dy as i64) * (dy as i64) + (dz as i64) * (dz as i64);
    d2 <= (s.r as i64) * (s.r as i64)
}

/// Полный перебор пар. Существует как эталон и как точка отсчёта.
pub fn brute(shields: &[Shield], points: &[Point], out: &mut Vec<(u32, u32)>) -> u64 {
    out.clear();
    let mut tested = 0u64;
    for (pi, p) in points.iter().enumerate() {
        for (si, s) in shields.iter().enumerate() {
            tested += 1;
            if inside(s, p) {
                out.push((pi as u32, si as u32));
            }
        }
    }
    out.sort_unstable();
    tested
}

/// Сетка по щитам: ведро равно наибольшему радиусу, запрос трогает 3×3.
///
/// Раскладка CSR и счётная сортировка — как у всех индексов проекта:
/// одно выделение, порядок внутри ведра по индексу щита.
pub struct ShieldIndex {
    cell: i32,
    w: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl ShieldIndex {
    pub fn new(world: i32, max_radius: i32, capacity: usize) -> ShieldIndex {
        let cell = max_radius.max(1);
        let w = (world / cell) as usize + 2;
        ShieldIndex {
            cell,
            w,
            starts: vec![0; w * w + 1],
            items: vec![0; capacity],
        }
    }

    #[inline(always)]
    fn bucket(&self, x: i32, z: i32) -> usize {
        let bx = (x / self.cell).clamp(0, self.w as i32 - 1) as usize;
        let bz = (z / self.cell).clamp(0, self.w as i32 - 1) as usize;
        bz * self.w + bx
    }

    pub fn rebuild(&mut self, shields: &[Shield]) {
        self.starts.iter_mut().for_each(|v| *v = 0);
        if self.items.len() < shields.len() {
            self.items.resize(shields.len(), 0);
        }
        for s in shields {
            let b = self.bucket(s.x, s.z);
            self.starts[b + 1] += 1;
        }
        for k in 1..self.starts.len() {
            self.starts[k] += self.starts[k - 1];
        }
        let mut cursor = self.starts.clone();
        for (i, s) in shields.iter().enumerate() {
            let b = self.bucket(s.x, s.z);
            self.items[cursor[b] as usize] = i as u32;
            cursor[b] += 1;
        }
    }

    /// Пары «точка, щит». Порядок — по точке, внутри точки по индексу щита,
    /// то есть не зависит от раскладки индекса.
    pub fn pairs(&self, shields: &[Shield], points: &[Point], out: &mut Vec<(u32, u32)>) -> u64 {
        out.clear();
        let mut tested = 0u64;
        for (pi, p) in points.iter().enumerate() {
            let bx = (p.x / self.cell).clamp(0, self.w as i32 - 1);
            let bz = (p.z / self.cell).clamp(0, self.w as i32 - 1);
            for gz in (bz - 1).max(0)..=(bz + 1).min(self.w as i32 - 1) {
                for gx in (bx - 1).max(0)..=(bx + 1).min(self.w as i32 - 1) {
                    let b = gz as usize * self.w + gx as usize;
                    let (lo, hi) = (self.starts[b] as usize, self.starts[b + 1] as usize);
                    for &si in &self.items[lo..hi] {
                        tested += 1;
                        if inside(&shields[si as usize], p) {
                            out.push((pi as u32, si));
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        tested
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::rng::Rng;

    fn scene(n_shields: usize, n_points: usize, world: i32, r: i32) -> (Vec<Shield>, Vec<Point>) {
        let mut rng = Rng::new(3, 3);
        let shields = (0..n_shields)
            .map(|_| Shield {
                x: rng.below(world as u32) as i32,
                z: rng.below(world as u32) as i32,
                y: rng.range(0, 40),
                r: rng.range(r / 2, r),
                owner: rng.below(2) as u8,
                charge: rng.range(0, 100),
            })
            .collect();
        let points = (0..n_points)
            .map(|_| Point {
                x: rng.below(world as u32) as i32,
                z: rng.below(world as u32) as i32,
                y: rng.range(0, 40),
                owner: rng.below(2) as u8,
            })
            .collect();
        (shields, points)
    }

    /// Индекс обязан находить ровно то же, что полный перебор.
    #[test]
    fn index_matches_brute() {
        for (ns, np, r) in [(20usize, 500usize, 60i32), (200, 2000, 40), (5, 100, 200)] {
            let (shields, points) = scene(ns, np, 2000, r);
            let mut a = Vec::new();
            let mut b = Vec::new();
            brute(&shields, &points, &mut a);
            let mut idx = ShieldIndex::new(2000, r, shields.len());
            idx.rebuild(&shields);
            idx.pairs(&shields, &points, &mut b);
            assert_eq!(a, b, "щитов {ns}, точек {np}, радиус {r}");
        }
    }

    /// Щит не ловит своих и не ловит с нулевым зарядом.
    #[test]
    fn shield_ignores_own_and_empty() {
        let s = Shield {
            x: 0,
            z: 0,
            y: 0,
            r: 100,
            owner: 1,
            charge: 10,
        };
        let mine = Point {
            x: 1,
            y: 1,
            z: 1,
            owner: 1,
        };
        let theirs = Point {
            x: 1,
            y: 1,
            z: 1,
            owner: 0,
        };
        assert!(!inside(&s, &mine));
        assert!(inside(&s, &theirs));
        let empty = Shield { charge: 0, ..s };
        assert!(!inside(&empty, &theirs));
    }

    /// Граница включительная и симметричная: точка ровно на радиусе внутри.
    #[test]
    fn boundary_is_inclusive() {
        let s = Shield {
            x: 100,
            z: 100,
            y: 0,
            r: 10,
            owner: 0,
            charge: 1,
        };
        for (dx, dy, dz) in [(10, 0, 0), (-10, 0, 0), (0, 10, 0), (0, 0, 10), (6, 8, 0)] {
            let p = Point {
                x: 100 + dx,
                y: dy,
                z: 100 + dz,
                owner: 1,
            };
            assert!(inside(&s, &p), "({dx},{dy},{dz}) снаружи");
        }
        let p = Point {
            x: 111,
            y: 0,
            z: 100,
            owner: 1,
        };
        assert!(!inside(&s, &p));
    }
}
