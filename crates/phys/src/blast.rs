//! Bounded, deterministic propagation of a blast's impulse envelope.
//!
//! A coarse cell pays for distance and for the material it crosses. A wall
//! therefore shields what is behind it, while air routes carry the pulse
//! around its edges. This is a gameplay approximation, not compressible CFD.
//! Work continues across ticks; the caller applies emitted hits through the
//! world's normal edit queue. The field never reads a partially edited world.

/// One frontier entry: pressure in the high word, the cell's index
/// inverted in the low one, so the greatest entry is the highest pressure
/// and, among equal pressures, the lowest index. The same order as the
/// pair `(pressure, Reverse(index))`, compared as one integer.
fn key(pressure: u32, index: usize) -> u64 {
    (u64::from(pressure) << 32) | u64::from(u32::MAX - index as u32)
}
fn unkey(k: u64) -> (u32, usize) {
    ((k >> 32) as u32, (u32::MAX - k as u32) as usize)
}

/// Pressure units per frontier bucket, as a shift.
const BUCKET_SHIFT: u32 = 6;

/// The frontier, popped greatest entry first (ADR-681). Entries wait in
/// buckets of 64 pressure units; the highest bucket is sorted once when
/// its turn comes and drained from the top. A step costs at least 100, so
/// a cell drained from a bucket only ever pushes into lower buckets, and
/// the pop order is exactly a binary heap's. A push into the bucket being
/// drained, or above it, is placed in order there all the same.
#[derive(Clone, Default)]
struct Frontier {
    buckets: Vec<Vec<u64>>,
    /// No bucket above this one holds an entry.
    top: usize,
    /// The bucket being drained, ascending: the next pop is its last.
    current: Vec<u64>,
    current_bucket: usize,
    len: usize,
}

impl Frontier {
    fn push(&mut self, k: u64) {
        self.len += 1;
        let b = (k >> (32 + BUCKET_SHIFT)) as usize;
        if !self.current.is_empty() && b >= self.current_bucket {
            let at = self.current.partition_point(|&x| x < k);
            self.current.insert(at, k);
            return;
        }
        if b >= self.buckets.len() {
            self.buckets.resize_with(b + 1, Vec::new);
        }
        self.buckets[b].push(k);
        self.top = self.top.max(b);
    }
    fn pop(&mut self) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        while self.current.is_empty() {
            let b = self.top;
            if self.buckets[b].is_empty() {
                self.top -= 1;
                continue;
            }
            std::mem::swap(&mut self.current, &mut self.buckets[b]);
            self.current.sort_unstable();
            self.current_bucket = b;
        }
        self.len -= 1;
        self.current.pop()
    }
    fn len(&self) -> usize {
        self.len
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Every entry, greatest first: the order the pops would take.
    fn ordered(&self) -> Vec<u64> {
        let mut all: Vec<u64> = self
            .buckets
            .iter()
            .flatten()
            .chain(&self.current)
            .copied()
            .collect();
        all.sort_unstable_by(|a, b| b.cmp(a));
        all
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub cell: [i32; 3],
    pub pressure: u32,
    pub broken: bool,
}

/// The settled flag, kept in the top bit of a cell's pressure word. The
/// pressure itself never exceeds the energy ceiling of one million.
const SETTLED: u32 = 1 << 31;

/// A cell's resistance and pressure side by side: the frontier reads both
/// of every neighbour, and one cache line then serves both (ADR-681).
#[derive(Clone, Copy, Default)]
struct Cell {
    resistance: u32,
    /// Pressure, with [`SETTLED`] once the frontier has left the cell.
    /// A settled cell's word exceeds every pressure, so `next > state`
    /// alone says "higher than the cell's pressure and not settled".
    state: u32,
}

#[derive(Clone)]
pub struct Wave {
    n: usize,
    cells: Vec<Cell>,
    frontier: Frontier,
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
        let mut cells: Vec<Cell> = resistance
            .into_iter()
            .map(|resistance| Cell {
                resistance,
                state: 0,
            })
            .collect();
        let c = n / 2;
        let centre = c + n * (c + n * c);
        cells[centre].state = energy;
        let mut frontier = Frontier::default();
        frontier.push(key(energy, centre));
        Ok(Self {
            n,
            cells,
            frontier,
            examined: 0,
        })
    }

    /// A surface detonation also enters verified exterior air on its incoming
    /// side. The contact cell keeps its pressure and resistance, so an intact
    /// wall still shields its far side. Call only before propagation, after
    /// checking that this adjacent cell and the path to it contain no matter.
    /// The existing frontier persists the source without additional state.
    pub fn seed_exterior(&mut self, direction: [i32; 3]) -> bool {
        if self.examined != 0 || direction.iter().any(|d| !(-1..=1).contains(d)) {
            return false;
        }
        let axes = direction.iter().map(|d| d.abs()).sum::<i32>();
        let Some(cost) = step_cost(axes) else {
            return false;
        };
        let c = self.n / 2;
        let centre = c + self.n * (c + self.n * c);
        let q = direction.map(|d| (c as i32 + d) as usize);
        let index = q[0] + self.n * (q[1] + self.n * q[2]);
        let pressure = (self.cells[centre].state & !SETTLED).saturating_sub(cost);
        let target = self.cells[index];
        if target.resistance != 0 || pressure <= target.state & !SETTLED {
            return false;
        }
        self.cells[index].state = pressure;
        self.frontier.push(key(pressure, index));
        true
    }

    pub fn size(&self) -> usize {
        self.n
    }

    pub fn finished(&self) -> bool {
        self.frontier.is_empty()
    }

    pub fn pressure_at(&self, p: [usize; 3]) -> u32 {
        self.cells[p[0] + self.n * (p[1] + self.n * p[2])].state & !SETTLED
    }

    /// What the cell costs to break. Read by the test that holds the
    /// capture to the answer a per-voxel walk would give (ADR-310).
    pub fn resistance_at(&self, p: [usize; 3]) -> u32 {
        self.cells[p[0] + self.n * (p[1] + self.n * p[2])].resistance
    }

    /// What one cell costs to break, as the capture finds it. The grid is
    /// filled in slices under a ceiling per tick and the wave waits for
    /// the whole of it (ADR-311), so this is only ever written before the
    /// wave has taken a step.
    pub fn set_resistance(&mut self, i: usize, r: u32) {
        debug_assert_eq!(self.examined, 0, "the wave has already walked the grid");
        self.cells[i].resistance = r;
    }

    fn pressures(&self) -> Vec<u32> {
        self.cells.iter().map(|c| c.state & !SETTLED).collect()
    }
    fn settled(&self) -> Vec<bool> {
        self.cells.iter().map(|c| c.state & SETTLED != 0).collect()
    }

    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.u32(self.n as u32);
        let resistance: Vec<u32> = self.cells.iter().map(|c| c.resistance).collect();
        b.u32s(&resistance);
        b.u32s(&self.pressures());
        b.bools(&self.settled());
        b.usize(self.examined);
        // Storage order is an implementation detail. Persist priority order.
        let entries = self.frontier.ordered();
        b.u32(entries.len() as u32);
        for (e, i) in entries.into_iter().map(unkey) {
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
            || pressure.iter().any(|&p| p > 1_000_000)
        {
            return None;
        }
        let cells = resistance
            .into_iter()
            .zip(pressure)
            .zip(settled)
            .map(|((resistance, pressure), settled)| Cell {
                resistance,
                state: pressure | if settled { SETTLED } else { 0 },
            })
            .collect();
        let count = s.u32() as usize;
        if count > n * n * n * 26 {
            return None;
        }
        let mut frontier = Frontier::default();
        for _ in 0..count {
            let e = s.u32();
            let i = s.u32() as usize;
            if i >= n * n * n || e > 1_000_000 {
                return None;
            }
            frontier.push(key(e, i));
        }
        s.ok().then_some(Self {
            n,
            cells,
            frontier,
            examined,
        })
    }

    /// Entries waiting in the frontier, stale ones included.
    pub fn frontier_len(&self) -> usize {
        self.frontier.len()
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
        let n = self.n;
        let plane = n * n;
        // The 26 steps with their costs, hoisted out of the frontier loop.
        // The frontier orders by key alone, so the order in which a cell's
        // neighbours are pushed changes nothing.
        // Each step names the faces it crosses as bits, `a * 2 + side`:
        // it needs each of them inside the grid, and any of them sealed
        // seals a diagonal.
        let mut steps = [(0isize, 0u32, 0u8, false); 26];
        let mut k = 0;
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let axes = dx.abs() + dy.abs() + dz.abs();
                    let Some(cost) = step_cost(axes) else {
                        continue;
                    };
                    let offset =
                        dx as isize + (dy as isize + dz as isize * n as isize) * n as isize;
                    let faces = [dx, dy, dz]
                        .into_iter()
                        .enumerate()
                        .filter(|&(_, d)| d != 0)
                        .fold(0u8, |m, (a, d)| m | 1 << (a * 2 + usize::from(d > 0)));
                    steps[k] = (offset, cost, faces, axes > 1);
                    k += 1;
                }
            }
        }
        let axis_offset = [1, n, plane];
        for _ in 0..budget {
            let Some((energy, i)) = self.frontier.pop().map(unkey) else {
                break;
            };
            self.examined += 1;
            let cell = self.cells[i];
            // A settled cell's word is never an energy; nor is a stale one.
            if energy != cell.state {
                continue;
            }
            self.cells[i].state |= SETTLED;
            let p = [i % n, i / n % n, i / plane];
            let broken = cell.resistance > 0 && energy > cell.resistance;
            hits.push(Hit {
                cell: p.map(|v| v as i32),
                pressure: energy,
                broken,
            });
            let left = energy.saturating_sub(cell.resistance);
            if left <= 100 {
                continue;
            }
            // Whether the face neighbour on each side of each axis seals
            // against what is left, read once rather than once per
            // diagonal. `None` lies outside the grid, as does every step
            // that moves onto that side.
            let mut inside = 0u8;
            let mut seal = 0u8;
            for a in 0..3 {
                if p[a] > 0 {
                    inside |= 1 << (a * 2);
                    seal |= u8::from(self.cells[i - axis_offset[a]].resistance >= left) << (a * 2);
                }
                if p[a] + 1 < n {
                    inside |= 1 << (a * 2 + 1);
                    seal |=
                        u8::from(self.cells[i + axis_offset[a]].resistance >= left) << (a * 2 + 1);
                }
            }
            for &(offset, cost, faces, diagonal) in &steps {
                if faces & !inside != 0 {
                    continue;
                }
                // Diagonals cannot leak through a sealed corner.
                if faces & seal != 0 && (!contacts || diagonal) {
                    continue;
                }
                let j = (i as isize + offset) as usize;
                let target = self.cells[j];
                // A cell's material is paid once when the wave leaves
                // it, whatever the direction. A diagonal step is longer
                // than an axis step, so the target's material is also
                // charged for the extra length: otherwise soil is 18 %
                // cheaper per metre along diagonals and every crater
                // comes out square (ADR-274).
                let extra = target.resistance.saturating_mul(cost - 100) / 100;
                let next = left.saturating_sub(cost).saturating_sub(extra);
                if next > target.state {
                    self.cells[j].state = next;
                    self.frontier.push(key(next, j));
                }
            }
        }
        hits
    }

    /// The loop as first written, over a binary heap and separate arrays,
    /// kept as the reference the bucketed one must match hit for hit.
    #[cfg(test)]
    fn advance_reference(reference: &mut Reference, budget: usize, contacts: bool) -> Vec<Hit> {
        use std::cmp::Reverse;
        let Reference {
            n,
            resistance,
            pressure,
            settled,
            frontier,
            examined,
        } = reference;
        let n = *n as i32;
        let mut hits = Vec::new();
        for _ in 0..budget {
            let Some((energy, Reverse(i))) = frontier.pop() else {
                break;
            };
            *examined += 1;
            if settled[i] || energy != pressure[i] {
                continue;
            }
            settled[i] = true;
            let p = [i as i32 % n, i as i32 / n % n, i as i32 / (n * n)];
            let broken = resistance[i] > 0 && energy > resistance[i];
            hits.push(Hit {
                cell: p,
                pressure: energy,
                broken,
            });
            let left = energy.saturating_sub(resistance[i]);
            if left <= 100 {
                continue;
            }
            for dz in -1i32..=1 {
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let axes = dx.abs() + dy.abs() + dz.abs();
                        let Some(cost) = step_cost(axes) else {
                            continue;
                        };
                        let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                        if q.iter().any(|&v| v < 0 || v >= n) {
                            continue;
                        }
                        let mut blocked = false;
                        for (a, d) in [dx, dy, dz].into_iter().enumerate() {
                            if d == 0 {
                                continue;
                            }
                            let mut s = p;
                            s[a] += d;
                            let j = (s[0] + n * (s[1] + n * s[2])) as usize;
                            if resistance[j] >= left && (!contacts || axes > 1) {
                                blocked = true;
                            }
                        }
                        if blocked {
                            continue;
                        }
                        let j = (q[0] + n * (q[1] + n * q[2])) as usize;
                        let extra = resistance[j].saturating_mul(cost - 100) / 100;
                        let next = left.saturating_sub(cost).saturating_sub(extra);
                        if next > pressure[j] && !settled[j] {
                            pressure[j] = next;
                            frontier.push((next, Reverse(j)));
                        }
                    }
                }
            }
        }
        hits
    }
}

/// The wave as first stored, for the reference loop.
#[cfg(test)]
struct Reference {
    n: usize,
    resistance: Vec<u32>,
    pressure: Vec<u32>,
    settled: Vec<bool>,
    frontier: std::collections::BinaryHeap<(u32, std::cmp::Reverse<usize>)>,
    examined: usize,
}

#[cfg(test)]
impl Reference {
    fn of(w: &Wave) -> Self {
        Self {
            n: w.n,
            resistance: w.cells.iter().map(|c| c.resistance).collect(),
            pressure: w.pressures(),
            settled: w.settled(),
            frontier: w
                .frontier
                .ordered()
                .into_iter()
                .map(|k| {
                    let (e, i) = unkey(k);
                    (e, std::cmp::Reverse(i))
                })
                .collect(),
            examined: w.examined,
        }
    }
}

fn step_cost(axes: i32) -> Option<u32> {
    match axes {
        1 => Some(100),
        2 => Some(141),
        3 => Some(173),
        _ => None,
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
        assert_eq!(a.pressures(), b.pressures());
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
    fn a_surface_pulse_enters_air_and_preserves_steel_shielding_after_reload() {
        let n = 9;
        for (energy, thickness) in [(3500, 1), (9200, 3)] {
            let mut resistance = vec![0; n * n * n];
            for z in 0..n {
                for y in 0..n {
                    for x in 4..4 + thickness {
                        resistance[x + n * (y + n * z)] = 6000;
                    }
                }
            }
            let mut original = Wave::new(n, energy, resistance).unwrap();
            assert!(original.seed_exterior([-1, 0, 0]));
            assert!(!original.seed_exterior([-1, 0, 0]));
            let mut blob = fx::blob::Blob::new();
            original.save(&mut blob);
            let restored = Wave::load(&mut fx::blob::Scan::new(&blob.v)).unwrap();
            let mut contact = original.clone();
            let hits = contact.advance_contacts(1);
            assert_eq!(hits[0].cell, [4, 4, 4]);
            assert_eq!(hits[0].broken, energy > 6000);
            assert!(!contact.seed_exterior([1, 0, 0]));
            let single = solve(original, 1);
            let batch = solve(restored, 4096);
            assert_eq!(single.pressures(), batch.pressures());
            assert_eq!(single.examined, batch.examined);
            assert_eq!(single.pressure_at([3, 4, 4]), energy - 100);
            assert!(single.pressure_at([0, 4, 4]) > 0);
            assert_eq!(single.pressure_at([4 + thickness, 4, 4]), 0);
        }
    }

    /// The bucketed, packed loop takes the same hits, in the same order, with the
    /// same pressures and heap pops as the loop as first written, on
    /// grids mixing air, soil and sealing walls, both modes, edge seeds.
    #[test]
    fn hoisted_steps_match_the_reference_loop() {
        let mut seed = 0x9e37_79b9_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for n in [3, 5, 9, 17] {
            for case in 0..6 {
                let resistance: Vec<u32> = (0..n * n * n)
                    .map(|_| match next() % 8 {
                        0..=3 => 0,
                        4 | 5 => 165,
                        6 => 380,
                        _ => 6000 + case * 500,
                    })
                    .collect();
                let energy = 600 + next() % 9000;
                for contacts in [false, true] {
                    let mut a = Wave::new(n, energy, resistance.clone()).unwrap();
                    let direction = [next() % 3, next() % 3, next() % 3].map(|v| v as i32 - 1);
                    a.seed_exterior(direction);
                    let mut b = Reference::of(&a);
                    while !a.finished() {
                        let budget = 1 + next() as usize % 37;
                        assert_eq!(
                            a.advance_inner(budget, contacts),
                            Wave::advance_reference(&mut b, budget, contacts)
                        );
                        assert_eq!(a.frontier_len(), b.frontier.len());
                    }
                    assert!(b.frontier.is_empty());
                    assert_eq!(a.pressures(), b.pressure);
                    assert_eq!(a.settled(), b.settled);
                    assert_eq!(a.examined, b.examined);
                }
            }
        }
    }

    /// Measurement for analysis 251: a canister-sized field, soil below
    /// the centre and air above, against the reference loop.
    #[test]
    #[ignore = "measurement: cargo test --release -p phys -- --ignored --nocapture"]
    fn frontier_cost_on_a_canister_field() {
        let n = 65;
        let resistance: Vec<u32> = (0..n * n * n)
            .map(|i| if i / n % n < n / 2 { 165 } else { 0 })
            .collect();
        let wave = Wave::new(n, 20000, resistance).unwrap();
        let mut reference = Reference::of(&wave);
        let start = std::time::Instant::now();
        let mut hits = 0;
        while !reference.frontier.is_empty() {
            hits += Wave::advance_reference(&mut reference, 4096, true).len();
        }
        let old = start.elapsed();
        let mut wave = wave;
        let start = std::time::Instant::now();
        while !wave.finished() {
            wave.advance_contacts(4096);
        }
        let new = start.elapsed();
        println!(
            "pops {} hits {hits}: reference {:.1} ms, bucketed {:.1} ms",
            wave.examined,
            old.as_secs_f64() * 1e3,
            new.as_secs_f64() * 1e3
        );
        assert_eq!(wave.examined, reference.examined);
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
