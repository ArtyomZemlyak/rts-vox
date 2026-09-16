//! Bounded, deterministic propagation of a blast's impulse envelope.
//!
//! A coarse cell pays for distance and for the material it crosses. A wall
//! therefore shields what is behind it, while air routes carry the pulse
//! around its edges. This is a gameplay approximation, not compressible CFD.
//! Work continues across ticks; the caller applies emitted hits through the
//! world's normal edit queue. The field never reads a partially edited world.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub cell: [i32; 3],
    pub pressure: u32,
    pub broken: bool,
}

#[derive(Clone)]
pub struct Wave {
    n: usize,
    resistance: Vec<u32>,
    pressure: Vec<u32>,
    settled: Vec<bool>,
    frontier: BinaryHeap<(u32, Reverse<usize>)>,
    pub examined: usize,
}

impl Wave {
    /// At most 65 cubed cells. Rejected sizes do not allocate memory.
    pub fn new(n: usize, energy: u32, resistance: Vec<u32>) -> Result<Self, &'static str> {
        if !(3..=65).contains(&n) || n.is_multiple_of(2) || resistance.len() != n * n * n {
            return Err("blast grid must be odd, 3..=65, with one resistance per cell");
        }
        if energy == 0 || energy > 1_000_000 {
            return Err("blast energy out of range");
        }
        let mut pressure = vec![0; resistance.len()];
        let c = n / 2;
        let centre = c + n * (c + n * c);
        pressure[centre] = energy;
        Ok(Self {
            n,
            resistance,
            pressure,
            settled: vec![false; n * n * n],
            frontier: BinaryHeap::from([(energy, Reverse(centre))]),
            examined: 0,
        })
    }

    pub fn size(&self) -> usize {
        self.n
    }

    pub fn finished(&self) -> bool {
        self.frontier.is_empty()
    }

    pub fn pressure_at(&self, p: [usize; 3]) -> u32 {
        self.pressure[p[0] + self.n * (p[1] + self.n * p[2])]
    }

    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.u32(self.n as u32);
        b.u32s(&self.resistance);
        b.u32s(&self.pressure);
        b.bools(&self.settled);
        b.usize(self.examined);
        // Heap storage order is an implementation detail. Persist priority order.
        let mut heap = self.frontier.clone();
        b.u32(heap.len() as u32);
        while let Some((e, Reverse(i))) = heap.pop() {
            b.u32(e);
            b.u32(i as u32);
        }
    }

    pub fn load(s: &mut fx::blob::Scan) -> Option<Self> {
        let n = s.u32() as usize;
        if !(3..=65).contains(&n) || n.is_multiple_of(2) {
            return None;
        }
        let resistance = s.u32s();
        let pressure = s.u32s();
        let settled = s.bools();
        let examined = s.usize();
        if resistance.len() != n * n * n
            || pressure.len() != n * n * n
            || settled.len() != n * n * n
        {
            return None;
        }
        let count = s.u32() as usize;
        if count > n * n * n * 26 {
            return None;
        }
        let mut frontier = BinaryHeap::new();
        for _ in 0..count {
            let e = s.u32();
            let i = s.u32() as usize;
            if i >= n * n * n || e > 1_000_000 {
                return None;
            }
            frontier.push((e, Reverse(i)));
        }
        s.ok().then_some(Self {
            n,
            resistance,
            pressure,
            settled,
            frontier,
            examined,
        })
    }

    /// The budget counts heap pops, including stale entries. Even adversarial
    /// geometry cannot hide unbounded work behind a small number of hits.
    pub fn advance(&mut self, budget: usize) -> Vec<Hit> {
        self.advance_inner(budget, false)
    }

    /// Include impacts stopped at an intact face. The receiver can retain
    /// sub-threshold deformation; sealed cells still transmit no pressure.
    /// Use this consistently for a wave, including after loading its frontier.
    pub fn advance_contacts(&mut self, budget: usize) -> Vec<Hit> {
        self.advance_inner(budget, true)
    }

    fn advance_inner(&mut self, budget: usize, contacts: bool) -> Vec<Hit> {
        let mut hits = Vec::new();
        let n = self.n as i32;
        for _ in 0..budget {
            let Some((energy, Reverse(i))) = self.frontier.pop() else {
                break;
            };
            self.examined += 1;
            if self.settled[i] || energy != self.pressure[i] {
                continue;
            }
            self.settled[i] = true;
            let p = [
                (i % self.n) as i32,
                (i / self.n % self.n) as i32,
                (i / (self.n * self.n)) as i32,
            ];
            let broken = self.resistance[i] > 0 && energy > self.resistance[i];
            hits.push(Hit {
                cell: p,
                pressure: energy,
                broken,
            });
            let left = energy.saturating_sub(self.resistance[i]);
            if left <= 100 {
                continue;
            }
            for dz in -1i32..=1 {
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let axes = dx.abs() + dy.abs() + dz.abs();
                        let cost = match axes {
                            1 => 100,
                            2 => 141,
                            3 => 173,
                            _ => continue,
                        };
                        let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                        if q.iter().any(|&v| v < 0 || v >= n) {
                            continue;
                        }
                        // Diagonals cannot leak through a sealed corner.
                        let mut blocked = false;
                        for (a, d) in [dx, dy, dz].into_iter().enumerate() {
                            if d == 0 {
                                continue;
                            }
                            let mut s = p;
                            s[a] += d;
                            let j = (s[0] + n * (s[1] + n * s[2])) as usize;
                            if self.resistance[j] >= left && (!contacts || axes > 1) {
                                blocked = true;
                            }
                        }
                        if blocked {
                            continue;
                        }
                        let j = (q[0] + n * (q[1] + n * q[2])) as usize;
                        // A cell's material is paid once when the wave leaves
                        // it, whatever the direction. A diagonal step is longer
                        // than an axis step, so the target's material is also
                        // charged for the extra length: otherwise soil is 18 %
                        // cheaper per metre along diagonals and every crater
                        // comes out square (ADR-274).
                        let extra = self.resistance[j].saturating_mul(cost - 100) / 100;
                        let next = left.saturating_sub(cost).saturating_sub(extra);
                        if next > self.pressure[j] && !self.settled[j] {
                            self.pressure[j] = next;
                            self.frontier.push((next, Reverse(j)));
                        }
                    }
                }
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn solve(mut w: Wave, budget: usize) -> Wave {
        while !w.finished() {
            w.advance(budget);
        }
        w
    }
    #[test]
    fn a_sealed_wall_shields_and_an_opening_transmits() {
        let n = 17;
        let mut r = vec![0; n * n * n];
        for z in 0..n {
            for y in 0..n {
                r[10 + n * (y + n * z)] = 10000;
            }
        }
        let a = solve(Wave::new(n, 1400, r.clone()).unwrap(), 100);
        assert_eq!(a.pressure_at([12, 8, 8]), 0);
        r[10 + n * (8 + n * 8)] = 0;
        let b = solve(Wave::new(n, 1400, r).unwrap(), 100);
        assert!(b.pressure_at([12, 8, 8]) > 0);
    }
    #[test]
    fn weak_material_breaks_and_spends_energy() {
        let n = 9;
        let mut r = vec![0; n * n * n];
        r[4 + n * (4 + n * 4)] = 200;
        let mut w = Wave::new(n, 500, r).unwrap();
        assert!(w.advance(1)[0].broken);
        assert_eq!(w.pressure_at([5, 4, 4]), 200);
    }
    #[test]
    fn tick_budget_and_snapshot_do_not_change_the_result() {
        let n = 17;
        let r = (0..n * n * n)
            .map(|i| if i % 13 == 0 { 80 } else { 0 })
            .collect();
        let mut w = Wave::new(n, 1500, r).unwrap();
        w.advance(31);
        let a = solve(w.clone(), 1);
        let b = solve(w, 4096);
        assert_eq!(a.pressure, b.pressure);
        assert_eq!(a.examined, b.examined);
    }
    #[test]
    fn contact_mode_reaches_a_hard_face_without_transmitting_through_it() {
        let n = 9;
        let mut resistance = vec![0; n * n * n];
        for z in 0..n {
            for y in 0..n {
                resistance[5 + n * (y + n * z)] = 10000;
            }
        }
        let mut w = Wave::new(n, 900, resistance).unwrap();
        let mut face = None;
        while !w.finished() {
            for h in w.advance_contacts(7) {
                if h.cell == [5, 4, 4] {
                    face = Some(h);
                }
                assert_ne!(h.cell, [6, 4, 4]);
            }
        }
        let h = face.expect("hard face must receive the stopped impulse");
        assert_eq!(h.pressure, 800);
        assert!(!h.broken);
        assert_eq!(w.pressure_at([6, 4, 4]), 0);
    }
    /// Uniform soil worked from the centre: the reach along an axis and
    /// along a floor diagonal must agree within a few percent, or every
    /// crater comes out square (ADR-274).
    #[test]
    fn soil_is_worked_isotropically_not_square() {
        let n = 41;
        let c = 20i32;
        let w = solve(Wave::new(n, 2300, vec![165; n * n * n]).unwrap(), 4096);
        let reach = |dx: i32, dz: i32| {
            (1..=c)
                .take_while(|&k| {
                    w.pressure_at([(c + dx * k) as usize, c as usize, (c + dz * k) as usize]) > 0
                })
                .count() as i64
        };
        let axis = reach(1, 0);
        let diagonal = reach(1, 1);
        assert!(axis >= 5, "the wave barely left the centre: {axis}");
        // Cells along each axis of the diagonal, times sqrt 2, in thousandths.
        let ratio = diagonal * 1414 / axis;
        assert!(
            (900..=1100).contains(&ratio),
            "axis {axis} cells, diagonal {diagonal}: ratio {ratio} of 1000"
        );
    }
    #[test]
    fn invalid_grids_are_rejected() {
        assert!(Wave::new(1000, 1, vec![]).is_err());
        assert!(Wave::new(4, 100, vec![0; 64]).is_err());
    }
    #[test]
    fn a_saved_frontier_resumes_without_repeating_hits() {
        let mut a = Wave::new(17, 1300, vec![0; 17 * 17 * 17]).unwrap();
        a.advance(117);
        let mut blob = fx::blob::Blob::new();
        a.save(&mut blob);
        let mut b = Wave::load(&mut fx::blob::Scan::new(&blob.v)).unwrap();
        while !a.finished() {
            assert_eq!(a.advance(53), b.advance(53));
        }
        assert!(b.finished());
    }
}
