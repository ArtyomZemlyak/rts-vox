//! Common bounded formation assignment before ordinary command ingress.
use super::BlastRange;
use fx::Fx;

impl BlastRange {
    /// Allocate distinct, size-aware slots. Selection order has no influence.
    /// Explicit per-actor destinations remain ordinary replayable commands.
    pub fn battle_formation_destinations(
        &self,
        ids: &[usize],
        centre: [i32; 2],
    ) -> Vec<(usize, [i32; 2])> {
        // Walk the bounded roster, rather than an untrusted selection length.
        let mut selected = [false; modding::range::MAX_RANGE_UNITS];
        for &id in ids.iter().take(selected.len()) {
            if let Some(s) = selected.get_mut(id) {
                *s = true;
            }
        }
        let movers: Vec<_> = self
            .units
            .iter()
            .enumerate()
            .filter(|(id, u)| {
                selected.get(*id).copied().unwrap_or(false)
                    && u.status.alive
                    && !u.kind.emplacement()
                    && self.allied(*id)
            })
            .map(|(id, _)| id)
            .collect();
        let n = movers.len();
        if n == 0 {
            return Vec::new();
        }
        let size = self.size();
        let centre: [i32; 2] = std::array::from_fn(|a| centre[a].clamp(0, size[2 * a] as i32 - 1));
        let spacing = movers
            .iter()
            .map(|&id| {
                let o = super::motion::Obstacle::of(&self.units[id].motion, &self.units[id].body);
                let diameter = o.half[0]
                    .mul(o.half[0])
                    .add(o.half[1].mul(o.half[1]))
                    .sqrt()
                    .mul(Fx::from_int(2));
                diameter.to_int_trunc()
                    + 1
                    + 2 * self.navigation.cell_units
                    + 2 * self.navigation.plan_margin
            })
            .max()
            .unwrap();
        let cols = (1..=n).find(|c| c * c >= n).unwrap() as i32;
        let rows = (n as i32 + cols - 1) / cols;
        let average: [i32; 2] = std::array::from_fn(|a| {
            (movers
                .iter()
                .map(|&id| i64::from(self.units[id].motion.pos[2 * a].to_int_trunc()))
                .sum::<i64>()
                / n as i64) as i32
        });
        let forward = if (centre[0] - average[0]).abs() >= (centre[1] - average[1]).abs() {
            0
        } else {
            1
        };
        let lateral = 1 - forward;
        let mut span = [0; 2];
        span[lateral] = (cols - 1) * spacing;
        span[forward] = (rows - 1) * spacing;
        let margin = (spacing / 2).max(super::motion::EDGE);
        if (0..2).any(|a| span[a] + 2 * margin >= size[2 * a] as i32) {
            return Vec::new();
        }
        // Translate the whole grid at the edge; clamping each slot merges them.
        let origin: [i32; 2] = std::array::from_fn(|a| {
            (centre[a] - span[a] / 2).clamp(
                margin,
                (size[2 * a] as i32 - 1 - margin - span[a]).max(margin),
            )
        });
        let slots: Vec<[i32; 2]> = (0..n)
            .map(|i| {
                let mut slot = origin;
                slot[lateral] += (i as i32 % cols) * spacing;
                slot[forward] += (i as i32 / cols) * spacing;
                slot
            })
            .collect();
        let costs: Vec<Vec<i64>> = movers
            .iter()
            .map(|&id| {
                let p = self.units[id].motion.pos;
                slots
                    .iter()
                    .map(|s| {
                        let dx = i64::from(p[0].to_int_trunc()) - i64::from(s[0]);
                        let dz = i64::from(p[2].to_int_trunc()) - i64::from(s[1]);
                        dx * dx + dz * dz
                    })
                    .collect()
            })
            .collect();
        let assignment = assign(&costs);
        movers
            .into_iter()
            .enumerate()
            .map(|(i, id)| (id, slots[assignment[i]]))
            .collect()
    }
}

/// Hungarian assignment: O(n^3) once per order, n <= the bounded actor roster.
/// Integer costs, stable row/column ties. No searches and no per-tick state.
fn assign(cost: &[Vec<i64>]) -> Vec<usize> {
    let n = cost.len();
    let (mut u, mut v, mut p, mut way) = (
        vec![0i64; n + 1],
        vec![0i64; n + 1],
        vec![0usize; n + 1],
        vec![0usize; n + 1],
    );
    for i in 1..=n {
        p[0] = i;
        let mut col = 0;
        let mut best = vec![i64::MAX; n + 1];
        let mut used = vec![false; n + 1];
        loop {
            used[col] = true;
            let row = p[col];
            let (mut delta, mut next) = (i64::MAX, 0);
            for j in 1..=n {
                if used[j] {
                    continue;
                }
                let c = cost[row - 1][j - 1] - u[row] - v[j];
                if c < best[j] {
                    best[j] = c;
                    way[j] = col;
                }
                if best[j] < delta {
                    delta = best[j];
                    next = j;
                }
            }
            for j in 0..=n {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    best[j] -= delta;
                }
            }
            col = next;
            if p[col] == 0 {
                break;
            }
        }
        loop {
            let next = way[col];
            p[col] = p[next];
            col = next;
            if col == 0 {
                break;
            }
        }
    }
    let mut result = vec![0; n];
    for j in 1..=n {
        result[p[j] - 1] = j - 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assignment_minimizes_total_travel_with_stable_ties() {
        assert_eq!(
            assign(&[vec![100, 1, 3], vec![2, 90, 4], vec![3, 5, 80]]),
            vec![1, 2, 0]
        );
        assert_eq!(assign(&vec![vec![0; 4]; 4]), vec![0, 1, 2, 3]);
    }
}
