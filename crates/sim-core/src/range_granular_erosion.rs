//! Bounded bed-load approximation driven by actual local water transfers.
use super::*;
use crate::blast::water::Water;
const FACES: [[i32; 3]; 6] = [
    [0, -1, 0],
    [-1, 0, 0],
    [1, 0, 0],
    [0, 0, -1],
    [0, 0, 1],
    [0, 1, 0],
];
impl Flow {
    pub fn erode(
        &mut self,
        w: &mut World,
        water: &mut Water,
        tick: u32,
        blocked: impl Fn(Pos) -> bool,
    ) -> usize {
        let tuning = self.tuning.erosion;
        if tuning.moves_per_tick == 0 || water.flux.is_empty() {
            return 0;
        }
        let size = [w.nx, w.ny, w.nz];
        // Bounded sample of this tick's flux, rotated by authoritative tick.
        // No historical force queue or map-sized suspended sediment field.
        // Advance by a complete sample, rather than repeatedly inspecting all
        // but one record from the previous tick's prefix. Use a fixed width
        // integer so the authoritative schedule does not depend on pointer size.
        let count = water.flux.len().min(tuning.checks_per_tick);
        let start = ((u64::from(tick) * count as u64) % water.flux.len() as u64) as usize;
        let faces: Vec<_> = (0..count)
            .map(|i| water.flux[(start + i) % water.flux.len()])
            .collect();
        let mut changes = BTreeMap::new();
        let mut moved_to = BTreeSet::new();
        let mut moved = 0;
        for face in faces {
            if moved >= tuning.moves_per_tick {
                break;
            }
            if face.units < tuning.min_flow_units {
                continue;
            }
            let delta: [i32; 3] = std::array::from_fn(|a| face.to[a] as i32 - face.from[a] as i32);
            let target = if delta[1] == 0 {
                shift(face.to, delta, size)
            } else {
                Some(face.to)
            };
            let Some(target) = target else {
                continue;
            };
            if w.on_frame(target[0], target[1], target[2])
                || blocked(target)
                || w.has_pending_dig(target[0], target[1], target[2])
                || read(w, &changes, target) != 0
                || read(w, &changes, face.from) != 0
                || read(w, &changes, face.to) != 0
            {
                continue;
            }
            let pressure = u64::from(face.units) * u64::from(tuning.pressure_per_voxel)
                / u64::from(water.tuning.units_per_voxel);
            let best = FACES
                .iter()
                .filter_map(|&d| shift(face.to, d, size))
                .filter_map(|p| {
                    if p == target
                        || moved_to.contains(&p)
                        || w.on_frame(p[0], p[1], p[2])
                        || blocked(p)
                        || w.has_pending_dig(p[0], p[1], p[2])
                    {
                        return None;
                    }
                    let mat = read(w, &changes, p);
                    let material = self.tuning.material(mat)?;
                    (pressure >= u64::from(material.yield_pressure)
                        && water.saturation_milli(p, mat) >= tuning.saturation_milli
                        && water.can_exchange_grain(p, target))
                    .then_some((p, mat))
                })
                .min_by_key(|(p, _)| linear(*p, size));
            let Some((p, mat)) = best else {
                continue;
            };
            move_cell(&mut changes, p, target, mat);
            moved_to.insert(target);
            water.exchange_grain(p, target, size);
            self.wake(p, size);
            self.wake(target, size);
            self.moved += 1;
            moved += 1;
        }
        apply(w, changes);
        moved
    }
}
fn shift(p: Pos, d: [i32; 3], size: Pos) -> Option<Pos> {
    let q: [i64; 3] = std::array::from_fn(|a| i64::from(p[a]) + i64::from(d[a]));
    (0..3)
        .all(|a| q[a] > 0 && q[a] + 1 < i64::from(size[a]))
        .then(|| q.map(|v| v as u32))
}
