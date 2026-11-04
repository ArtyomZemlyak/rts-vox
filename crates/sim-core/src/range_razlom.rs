//! A bounded swept volume, paid through the same material claims as explosions.
use super::*;
use phys::V3;

impl BlastRange {
    pub(super) fn step_razlom_wave(
        &mut self,
        b: &mut Battle,
        mut r: Round,
        budget: &mut usize,
    ) -> Option<Round> {
        let tuning = &b.tuning.razlom_wave;
        let range = motion::spider_tuning().laser.range;
        let strength = r.distance.div(Fx::from_int(range)).clamp(Fx::ZERO, Fx::ONE);
        let speed = V3::new(r.velocity[0], Fx::ZERO, r.velocity[2]);
        let length = speed.len_sq().sqrt();
        if length <= Fx::ZERO || r.distance <= Fx::ZERO {
            return None;
        }
        let forward = speed.scale(Fx::ONE.div(length));
        let side = V3::new(forward.z.neg(), Fx::ZERO, forward.x);
        let distance = length
            .div(Fx::from_int(crate::TICKS_PER_SECOND))
            .min(r.distance);
        let start = V3::new(r.pos[0], r.pos[1], r.pos[2]);
        let end = start.add(forward.scale(distance));
        let width = Fx::from_int(tuning.half_width).mul(strength).max(Fx::ONE);
        let height = tuning.half_height;
        let corners = [
            start.add(side.scale(width)),
            start.sub(side.scale(width)),
            end.add(side.scale(width)),
            end.sub(side.scale(width)),
        ];
        let min = [
            corners.iter().map(|p| p.x.to_int_trunc()).min().unwrap() - 1,
            start.y.to_int_trunc() - height,
            corners.iter().map(|p| p.z.to_int_trunc()).min().unwrap() - 1,
        ];
        let max = [
            corners.iter().map(|p| p.x.to_int_trunc()).max().unwrap() + 1,
            start.y.to_int_trunc() + height,
            corners.iter().map(|p| p.z.to_int_trunc()).max().unwrap() + 1,
        ];
        let near: Vec<_> = self
            .units
            .iter()
            .enumerate()
            .filter_map(|(id, u)| {
                let (lo, hi) = u.bounds();
                (id != r.owner && (0..3).all(|a| min[a] < hi[a] && lo[a] <= max[a])).then_some(id)
            })
            .collect();
        let outside = (0..3).any(|a| min[a] < 0 || max[a] >= self.size[a] as i32);
        let terrain = outside
            || self
                .world
                .any_solid_box(min.map(|v| v as u32), max.map(|v| v as u32));
        // Empty flight uses the occupancy index and actor bounds; do not visit
        // thousands of empty cells for every travelling front.
        let mut cells = Vec::new();
        if terrain || !near.is_empty() || !self.shields.is_empty() {
            for z in min[2]..=max[2] {
                for x in min[0]..=max[0] {
                    let delta = V3::new(
                        Fx::from_int(x).add(Fx::from_ratio(1, 2)),
                        start.y,
                        Fx::from_int(z).add(Fx::from_ratio(1, 2)),
                    )
                    .sub(start);
                    let along = delta.dot(forward);
                    if along < Fx::ZERO || along > distance || delta.dot(side).abs() > width {
                        continue;
                    }
                    for y in min[1]..=max[1] {
                        cells.push([x, y, z]);
                        if cells.len() > *budget {
                            return Some(r);
                        }
                    }
                }
            }
        }
        *budget -= cells.len();
        let reference = self.matter.ground_price(2).max(1);
        let mut resistance = 0u64;
        let mut stopped = false;
        let mut deferred = false;
        let mut hit = false;
        let config = b.laser_config;
        let fragment_box = self.matter.fragment_box();
        let work = !cells.is_empty();
        let previous_energy = self.matter.remaining_mj;
        if work {
            self.matter.begin(
                tuning.fracture_j_per_tick,
                u32::from(self.matter.tuning.fragment_box),
            );
            self.matter.begin_body_batch();
        }
        for p in cells {
            if (0..3).any(|a| p[a] < 0 || p[a] >= self.size[a] as i32) {
                stopped = true;
                continue;
            }
            let behind =
                V3::new(Fx::from_int(p[0]), Fx::from_int(p[1]), Fx::from_int(p[2])).sub(forward);
            if let Some(shield) =
                self.dome_crossed([behind.x.to_int_trunc(), p[1], behind.z.to_int_trunc()], p)
            {
                self.absorb_work(shield, p, motion::spider_tuning().laser.work);
                stopped = true;
                hit = true;
                break;
            }
            if let Some((id, part, cell)) = near
                .iter()
                .find_map(|&id| self.units[id].hit(p).map(|(part, cell)| (id, part, cell)))
            {
                let u = &mut self.units[id];
                if u.section_holds(part, cell.map(|v| v as u8)) {
                    continue;
                }
                let frame = if part == 0 {
                    u.root_frame(cell[1])
                } else {
                    u.seg_frames[part - 1]
                };
                let need = u.hit_resistance(Some((part, cell)), &config);
                let body = if part == 0 {
                    &u.body
                } else {
                    &u.body.segments[part - 1].body
                };
                let (price, motion) = self.matter.body_price(body, cell);
                match self.matter.crush(id, u, part, cell, &frame) {
                    matter::ClaimResult::Applied => {
                        if part == 0 {
                            u.detached += 1;
                            u.receive_cell(cell, need, &config);
                        } else {
                            u.seg_detached += 1;
                            u.receive_seg_cell(part - 1, cell, need, &config);
                        }
                        resistance = resistance.saturating_add(price);
                        hit = true;
                    }
                    matter::ClaimResult::Deferred => {
                        deferred = true;
                        break;
                    }
                    matter::ClaimResult::Unaffordable
                        if price - motion <= u64::from(tuning.fracture_j_per_tick) * 1000 =>
                    {
                        deferred = true;
                        break;
                    }
                    _ => {
                        stopped = true;
                        break;
                    }
                }
            } else {
                let mat = material(&self.world, p);
                if mat == 0 {
                    continue;
                }
                let price = self.matter.ground_price(mat);
                match self.matter.ground_or_pulverize(
                    &mut self.world,
                    p,
                    mat,
                    self.units[r.owner].at,
                ) {
                    matter::ClaimResult::Applied => {
                        resistance = resistance.saturating_add(price);
                        hit = true;
                    }
                    matter::ClaimResult::Deferred => {
                        deferred = true;
                        break;
                    }
                    matter::ClaimResult::Unaffordable
                        if price <= u64::from(tuning.fracture_j_per_tick) * 1000 =>
                    {
                        deferred = true;
                        break;
                    }
                    _ => {
                        stopped = true;
                        break;
                    }
                }
            }
        }
        if work {
            self.matter.finish_body_batch();
            self.matter.fragment_size(u32::from(fragment_box));
            self.matter.remaining_mj = previous_energy;
        }
        let area =
            (width.mul(Fx::from_int(2)).to_int_trunc().max(1) as u64) * (2 * height + 1) as u64;
        let loss = Fx::from_int(tuning.attenuation_per_layer).mul(Fx::from_ratio(
            (resistance / reference).min(i32::MAX as u64) as i32,
            area.min(i32::MAX as u64) as i32,
        ));
        // A full fragment pool or a partly spent tick budget postpones the
        // front. Do not discard a paid shot or advance through untouched cover.
        let travelled = if deferred { Fx::ZERO } else { distance };
        let end = if deferred { start } else { end };
        r.distance = r.distance.sub(travelled).sub(loss).max(Fx::ZERO);
        if stopped {
            r.distance = Fx::ZERO;
        }
        for channel in -3..=3 {
            let offset = side.scale(width.mul(Fx::from_ratio(channel, 3)));
            let from = start.add(offset);
            let to = end.add(offset);
            let serial = self.streams.serial;
            self.streams.serial = serial.wrapping_add(1);
            b.razlom_lasers.push(weapons::WaveTrace {
                strength,
                trace: weapons::Trace {
                    from: [from.x, from.y, from.z],
                    to: [to.x, to.y, to.z],
                    hit,
                    serial,
                },
            });
        }
        r.pos = [end.x, end.y, end.z];
        (r.distance > Fx::ZERO).then_some(r)
    }
}
