//! Тома юнитов. Размеры взяты из ADR-019: у мира воксель крупный, у тел
//! мелкий, отношение дизайн сузил к двум. При мировом вокселе в метр это
//! полметра у тел, то есть танк 6×3×3 м — это 12×6×6 вокселей.
//!
//! Q-9 (точные рёбра вокселей) остаётся за человеком, поэтому берётся
//! диапазон: от лёгкого юнита до крупного шагохода.

use crate::unit::UnitVol;
use fx::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnitShape {
    /// Лёгкий разведчик: 8×5×5.
    Scout,
    /// Танк: корпус, башня, гусеницы — 16×8×8.
    Tank,
    /// Шагоход: корпус на ногах, много тонких перемычек — 24×12×12.
    Walker,
    /// Крупная постройка: 32×20×20, плотная.
    Building,
}

impl UnitShape {
    pub fn name(self) -> &'static str {
        match self {
            UnitShape::Scout => "scout 8x5x5",
            UnitShape::Tank => "tank 16x8x8",
            UnitShape::Walker => "walker 24x12x12",
            UnitShape::Building => "building 32x20x20",
        }
    }

    pub const ALL: [UnitShape; 4] = [
        UnitShape::Scout,
        UnitShape::Tank,
        UnitShape::Walker,
        UnitShape::Building,
    ];

    /// Ядро задаётся формой, а не серединой габарита: у шагохода середина
    /// габарита приходится на воздух между ногами.
    pub fn core(self) -> (u8, u8, u8) {
        match self {
            UnitShape::Scout => (3, 2, 2),
            UnitShape::Tank => (7, 3, 3),
            UnitShape::Walker => (11, 5, 8),
            UnitShape::Building => (15, 9, 6),
        }
    }

    /// Куда целиться, чтобы отстрелить тонкую часть: гусеницу, ногу,
    /// надстройку. Пусто там, где отстреливать нечего.
    pub fn thin_part(self) -> Option<(i32, i32, i32)> {
        match self {
            UnitShape::Scout => None,
            UnitShape::Tank => Some((7, 1, 2)),
            UnitShape::Walker => Some((6, 3, 3)),
            UnitShape::Building => Some((15, 9, 17)),
        }
    }
}

fn box_fill(v: &mut UnitVol, x0: usize, x1: usize, y0: usize, y1: usize, z0: usize, z1: usize) {
    for z in z0..z1.min(v.nz as usize) {
        for y in y0..y1.min(v.ny as usize) {
            for x in x0..x1.min(v.nx as usize) {
                v.set(x, y, z, true);
            }
        }
    }
}

pub fn build_unit(shape: UnitShape, seed: u64) -> UnitVol {
    let mut r = Rng::new(seed, shape as u64);
    match shape {
        UnitShape::Scout => {
            let mut v = UnitVol::new(8, 5, 5);
            box_fill(&mut v, 1, 7, 1, 4, 1, 4);
            v
        }
        UnitShape::Tank => {
            let mut v = UnitVol::new(16, 8, 8);
            // Корпус.
            box_fill(&mut v, 3, 13, 1, 7, 2, 6);
            // Башня сверху.
            box_fill(&mut v, 6, 11, 2, 6, 6, 8);
            // Гусеницы по бокам, держатся на узких стойках.
            box_fill(&mut v, 2, 14, 0, 1, 1, 4);
            box_fill(&mut v, 2, 14, 7, 8, 1, 4);
            for x in (4..12).step_by(3) {
                v.set(x, 1, 2, true);
                v.set(x, 6, 2, true);
            }
            v
        }
        UnitShape::Walker => {
            let mut v = UnitVol::new(24, 12, 12);
            // Корпус наверху.
            box_fill(&mut v, 5, 19, 3, 9, 7, 11);
            // Четыре ноги — тонкие, по два вокселя в сечении.
            for (lx, ly) in [(6usize, 3usize), (6, 8), (16, 3), (16, 8)] {
                for z in 1..7 {
                    for dx in 0..2 {
                        for dy in 0..2 {
                            v.set(lx + dx, ly + dy, z, true);
                        }
                    }
                }
            }
            v
        }
        UnitShape::Building => {
            let mut v = UnitVol::new(32, 20, 20);
            box_fill(&mut v, 2, 30, 2, 18, 0, 16);
            // Надстройка.
            box_fill(&mut v, 8, 24, 6, 14, 16, 20);
            // Внутренние пустоты — коридоры.
            for _ in 0..6 {
                let (x0, y0, z0) = (
                    r.range(4, 24) as usize,
                    r.range(4, 12) as usize,
                    r.range(2, 12) as usize,
                );
                for z in z0..(z0 + 3).min(20) {
                    for y in y0..(y0 + 3).min(20) {
                        for x in x0..(x0 + 6).min(32) {
                            v.set(x, y, z, false);
                        }
                    }
                }
            }
            v
        }
    }
}

/// Попадание: снимает несколько соседних вокселей. Не идеальный куб —
/// форма выемки задаётся оружием (ADR-045), здесь взята кучка.
/// Возвращает список снятого: ступень A корректна только по одному вокселю
/// за раз, поэтому залп обязан применяться пошагово.
pub fn apply_hit_at(
    v: &mut UnitVol,
    cx: i32,
    cy: i32,
    cz: i32,
    radius: i32,
    out: &mut Vec<(usize, usize, usize)>,
) -> u32 {
    let (nx, ny, nz) = (v.nx as i32, v.ny as i32, v.nz as i32);
    let mut removed = 0;
    for dz in -radius..=radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy + dz * dz > radius * radius {
                    continue;
                }
                let (x, y, z) = (cx + dx, cy + dy, cz + dz);
                if x < 0 || y < 0 || z < 0 || x >= nx || y >= ny || z >= nz {
                    continue;
                }
                if v.get(x as usize, y as usize, z as usize) {
                    v.set(x as usize, y as usize, z as usize, false);
                    out.push((x as usize, y as usize, z as usize));
                    removed += 1;
                }
            }
        }
    }
    removed
}

pub fn apply_hit(v: &mut UnitVol, r: &mut Rng, radius: i32) -> u32 {
    let (nx, ny, nz) = (v.nx as i32, v.ny as i32, v.nz as i32);
    let (cx, cy, cz) = (
        r.range(0, nx - 1),
        r.range(0, ny - 1),
        r.range(0, nz - 1),
    );
    let mut removed = 0;
    for dz in -radius..=radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy + dz * dz > radius * radius {
                    continue;
                }
                let (x, y, z) = (cx + dx, cy + dy, cz + dz);
                if x < 0 || y < 0 || z < 0 || x >= nx || y >= ny || z >= nz {
                    continue;
                }
                if v.get(x as usize, y as usize, z as usize) {
                    v.set(x as usize, y as usize, z as usize, false);
                    removed += 1;
                }
            }
        }
    }
    removed
}
