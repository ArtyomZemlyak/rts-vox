//! Ordered, budgeted penetration through material in body-local coordinates.
//! Energy is millijoules, lengths are millimetres; grid resolution is storage.
use std::collections::BTreeMap;

use crate::UnitBody;
use fx::Fx;

pub type Cell = [u8; 3];

/// Persistent deformation work in surviving cells. This is not unit health:
/// a second shot must intersect the same material to use its previous damage.
pub type Dents = BTreeMap<Cell, u64>;

/// Kinetic energy in mJ from mass in grams and velocity in mm/s.
pub fn kinetic_mj(mass_g: u32, velocity_mm_s: [i64; 3]) -> u64 {
    let squared = velocity_mm_s.into_iter().fold(0u128, |n, v| {
        let a = v.unsigned_abs() as u128;
        n.saturating_add(a.saturating_mul(a))
    });
    squared
        .saturating_mul(mass_g as u128)
        .checked_div(2_000_000)
        .unwrap_or(0)
        .min(u64::MAX as u128) as u64
}

/// Ordered intersections with positive volume. Edge/corner ties advance all
/// coincident axes, so a zero-length grazing contact consumes no energy.
/// Every axis advances at most 255 times: work cannot scale with ray length.
pub fn ray_cells(size: [u8; 3], from: [Fx; 3], to: [Fx; 3]) -> Vec<Cell> {
    if size.contains(&0) {
        return Vec::new();
    }
    let d = std::array::from_fn::<_, 3, _>(|a| to[a].sub(from[a]));
    let (mut enter, mut leave) = (Fx::ZERO, Fx::ONE);
    for a in 0..3 {
        if d[a] == Fx::ZERO {
            if from[a] < Fx::ZERO || from[a] >= Fx::from_int(size[a] as i32) {
                return Vec::new();
            }
        } else {
            let t0 = Fx::ZERO.sub(from[a]).div(d[a]);
            let t1 = Fx::from_int(size[a] as i32).sub(from[a]).div(d[a]);
            enter = enter.max(t0.min(t1));
            leave = leave.min(t0.max(t1));
        }
    }
    if enter >= leave || d.iter().all(|v| *v == Fx::ZERO) {
        return Vec::new();
    }
    // Move a few Q32.32 ulps into the clipped interval to resolve the far face
    // of negative rays without rounding a coordinate outside the volume.
    let start = enter.add(Fx(8)).min(enter.add(leave).div(Fx::from_int(2)));
    let mut cell = std::array::from_fn::<_, 3, _>(|a| {
        from[a]
            .add(d[a].mul(start))
            .to_int_trunc()
            .clamp(0, size[a] as i32 - 1)
    });
    let mut next = [Fx::MAX; 3];
    let mut delta = [Fx::MAX; 3];
    let mut step = [0; 3];
    for a in 0..3 {
        if d[a] != Fx::ZERO {
            step[a] = if d[a] > Fx::ZERO { 1 } else { -1 };
            let edge = cell[a] + i32::from(step[a] > 0);
            next[a] = Fx::from_int(edge).sub(from[a]).div(d[a]);
            delta[a] = Fx::ONE.div(d[a].abs());
        }
    }
    let mut out = Vec::new();
    let mut t = enter;
    for _ in 0..(size.into_iter().map(|v| v as usize).sum::<usize>() + 3) {
        if (0..3).any(|a| cell[a] < 0 || cell[a] >= size[a] as i32) {
            break;
        }
        let end = next[0].min(next[1]).min(next[2]).min(leave);
        if end > t {
            out.push(cell.map(|v| v as u8));
        }
        if end >= leave {
            break;
        }
        // Same fixed-point parameter means the same physical edge.
        for a in 0..3 {
            if next[a] <= end {
                cell[a] += step[a];
                next[a] = next[a].add(delta[a]);
            }
        }
        t = end;
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Penetration {
    pub cells: Vec<Cell>,
    pub cursor: usize,
    pub remaining_mj: u64,
    pub initial_mj: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImpactStep {
    pub visited: u32,
    pub spent_mj: u64,
    pub removed: Vec<Cell>,
    pub dented: Option<Cell>,
}

impl Penetration {
    pub fn new(body: &UnitBody, from: [Fx; 3], to: [Fx; 3], energy_mj: u64) -> Self {
        Self {
            cells: ray_cells([body.vol.nx, body.vol.ny, body.vol.nz], from, to),
            cursor: 0,
            remaining_mj: energy_mj,
            initial_mj: energy_mj,
        }
    }
    pub fn done(&self) -> bool {
        self.remaining_mj == 0 || self.cursor == self.cells.len()
    }

    /// Work density indexed by `Part` (kJ/m³). Budget counts visits including
    /// air, so long pierced tunnels cannot bypass the per-tick work limit.
    pub fn step(
        &mut self,
        body: &mut UnitBody,
        dents: &mut Dents,
        work_kj_m3: &[u32; 5],
        budget: u32,
    ) -> ImpactStep {
        let mut out = ImpactStep::default();
        while !self.done() && out.visited < budget {
            let cell = self.cells[self.cursor];
            self.cursor += 1;
            out.visited += 1;
            let [x, y, z] = cell.map(|v| v as usize);
            if !body.vol.get(x, y, z) {
                continue;
            }
            let mm = body.mm as u64;
            let density = work_kj_m3[body.part_at(x, y, z) as usize] as u64;
            let cost = ((mm as u128).pow(3) * density as u128)
                .div_ceil(1000)
                .clamp(1, u64::MAX as u128) as u64;
            let old = dents.get(&cell).copied().unwrap_or(0).min(cost);
            let need = cost - old;
            let spent = need.min(self.remaining_mj);
            self.remaining_mj -= spent;
            out.spent_mj += spent;
            if spent == need {
                body.remove(x, y, z);
                dents.remove(&cell);
                out.removed.push(cell);
            } else {
                dents.insert(cell, old + spent);
                out.dented = Some(cell);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Part;

    fn p(v: [i32; 3]) -> [Fx; 3] {
        v.map(Fx::from_int)
    }
    fn slab(mm: u16) -> UnitBody {
        let n = (1000 / mm) as u8;
        let mut b = UnitBody::new_scaled(n, 3, 3, 1, mm);
        for x in 0..n as usize {
            b.set(x, 1, 1, Part::Armour);
        }
        b
    }
    #[test]
    fn kinetic_energy_uses_mass_and_the_square_of_speed() {
        assert_eq!(kinetic_mj(6000, [600000, 0, 0]), 1_080_000_000);
        assert_eq!(kinetic_mj(12000, [600000, 0, 0]), 2_160_000_000);
        assert_eq!(kinetic_mj(6000, [1200000, 0, 0]), 4_320_000_000);
        assert_eq!(kinetic_mj(1000, [3000, -4000, 0]), 12500);
    }
    #[test]
    fn a_fast_ray_visits_the_first_layer_in_either_direction() {
        let a = [
            Fx::from_int(-1000),
            Fx::from_ratio(3, 2),
            Fx::from_ratio(3, 2),
        ];
        let b = [Fx::from_int(1000), a[1], a[2]];
        let f = ray_cells([10, 3, 3], a, b);
        let r = ray_cells([10, 3, 3], b, a);
        assert_eq!(f, (0..10).map(|x| [x, 1, 1]).collect::<Vec<_>>());
        assert_eq!(f, r.into_iter().rev().collect::<Vec<_>>());
        assert!(ray_cells([10, 3, 3], p([-1, 3, 1]), p([20, 3, 1])).is_empty());
    }
    #[test]
    fn partial_work_stays_where_the_first_shot_dented_the_armour() {
        let mut body = slab(100);
        let mut dents = Dents::new();
        let mut a = Penetration::new(&body, p([-1, 1, 1]), p([20, 1, 1]), 600);
        let x = a.step(&mut body, &mut dents, &[1; 5], 100);
        assert!(x.removed.is_empty());
        assert_eq!(x.dented, Some([0, 1, 1]));
        let mut b = Penetration::new(&body, p([-1, 1, 1]), p([20, 1, 1]), 500);
        let x = b.step(&mut body, &mut dents, &[1; 5], 100);
        assert_eq!(x.removed, [[0, 1, 1]]);
        assert_eq!(dents.get(&[1, 1, 1]), Some(&100));
        assert_eq!(x.spent_mj, 500);
    }
    #[test]
    fn energy_and_work_budget_produce_the_same_spatial_result() {
        let run = |budget| {
            let mut b = slab(100);
            let mut d = Dents::new();
            let mut ray = Penetration::new(&b, p([-1, 1, 1]), p([20, 1, 1]), 4500);
            while !ray.done() {
                let r = ray.step(&mut b, &mut d, &[1; 5], budget);
                assert!(r.visited <= budget);
            }
            (b.vol.cols, d, ray.remaining_mj)
        };
        assert_eq!(run(1), run(100));
        let (cols, dents, energy) = run(2);
        assert_eq!(energy, 0);
        assert_eq!(dents.get(&[4, 1, 1]), Some(&500));
        assert_eq!(cols.iter().map(|c| c.count_ones()).sum::<u32>(), 6);
    }
    #[test]
    fn work_scales_with_material_volume_not_voxel_count() {
        for mm in [100, 200] {
            let mut b = slab(mm);
            let mut d = Dents::new();
            let cell_work = (mm as u64).pow(3) / 1000;
            let mut ray = Penetration::new(&b, p([-1, 1, 1]), p([20, 1, 1]), cell_work);
            let x = ray.step(&mut b, &mut d, &[1; 5], 100);
            assert_eq!(x.removed.len(), 1);
            assert_eq!(x.spent_mj, cell_work);
        }
    }
}
