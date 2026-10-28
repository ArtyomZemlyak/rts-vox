//! Laboratory apparatus, authoritative and checkpointed in the authored layout.
//! Cutting uses material strength, the ordinary edit queue and real containers.
use super::{battle::commands::Outcome, BlastRange};
use crate::matter::{Kind, Sink};
use modding::lab_tools::DeviceKind;
use voxel::Edit;
const ERASE: u32 = 0xfffc_0000;
const DRILL: u32 = 0xfffb_0000;
impl BlastRange {
    pub(super) fn lab_stream(&mut self, origin: [i32; 3], target: [i32; 3], enabled: bool) -> Outcome {
        use modding::explosion::Delivery;
        if !self.layout.as_ref().is_some_and(|l| l.laboratory) || self.battle_mode() {
            return Outcome::Rejected;
        }
        if !enabled {
            let changed = self.streams.left != 0 || !self.streams.traces.is_empty() || self.fire_tick != u32::MAX;
            self.streams.left = 0;
            self.streams.traces.clear();
            self.fire_tick = u32::MAX;
            return if changed { Outcome::Applied } else { Outcome::Unchanged };
        }
        if (0..3).any(|a| origin[a] < 1 || origin[a] >= self.size[a] as i32 - 1
            || target[a] < 0 || target[a] >= self.size[a] as i32)
            || origin[0] < 32 || origin[0] >= self.size[0] as i32 - 32
            || origin[2] < 32 || origin[2] >= self.size[2] as i32 - 32
            || origin == target {
            return Outcome::Rejected;
        }
        let solid = matches!(self.config.delivery, Delivery::Blast | Delivery::Kinetic);
        let charging = solid && self.shell.is_none() && self.fire_tick != u32::MAX && self.tick <= self.fire_tick;
        // Streams can heat/cut while a previous blast is still settling. Solid
        // rounds retain ordinary bounded flight/wave admission and reload.
        if solid && !charging && (!self.shot_ready() || self.tick.checked_add(self.config.charge_ticks)
            .and_then(|t|t.checked_add(1)).is_none_or(|t| t==u32::MAX)) { return Outcome::Busy; }
        if self.shell.is_some() { return Outcome::Busy; }
        let layout = self.layout.as_mut().unwrap();
        layout.gun_point = true;
        layout.gun = origin;
        self.aim = Some(target);
        self.attack = None;
        self.player = true;
        if solid {
            if charging {return Outcome::Applied;}
            if !self.refire_inner() { return Outcome::Busy; }
            // Input is already sampled for this fixed tick. Preserve charge
            // duration; a zero-charge tool fires during the upcoming step.
            self.fire_tick -= 1;
            return Outcome::Applied;
        }
        self.fire_tick = u32::MAX;
        // Refresh an ordinary bounded burst. Existing packet, interval, heat and
        // collision limits still apply; no unbounded or immortal stream state.
        self.streams.left = match self.config.delivery {
            Delivery::Laser => self.streams.tuning.laser.burst_ticks,
            Delivery::MachineGun => self.streams.tuning.machine_gun.burst_ticks,
            _ => self.streams.tuning.flame.burst_ticks,
        };
        Outcome::Applied
    }
    pub(super) fn sculpt(
        &mut self,
        at: [i32; 3],
        edge: u32,
        material: u16,
        erase: bool,
    ) -> Outcome {
        if !self.layout.as_ref().is_some_and(|l| l.laboratory) {
            return Outcome::Rejected;
        }
        let t = self.lab_tools_tuning;
        if edge == 0
            || edge > t.brush_edge
            || u64::from(edge).pow(3) > t.brush_cells as u64
            || (!erase && self.matter.economy.mats.get(material).is_none())
            || (!erase && material == 0)
            || (0..3)
                .any(|a| at[a] < 1 || i64::from(at[a]) + i64::from(edge) >= i64::from(self.size[a]))
        {
            return Outcome::Rejected;
        }
        let mut count = 0;
        for z in at[2]..at[2] + edge as i32 {
            for y in at[1]..at[1] + edge as i32 {
                for x in at[0]..at[0] + edge as i32 {
                    let p = [x as u32, y as u32, z as u32];
                    if self.world.on_frame(p[0], p[1], p[2])
                        || self.matter.debris.claimed(p)
                        || self.world.has_pending_dig(p[0], p[1], p[2])
                    {
                        continue;
                    }
                    let old = self.world.material(p[0], p[1], p[2]);
                    if erase {
                        if self.water.edit_volume(p, false) {
                            count += 1;
                        }
                        if old != 0 {
                            let mut e = Edit::dig(p[0], p[1], p[2]);
                            e.by = ERASE;
                            self.world.queue_edit(e);
                            count += 1;
                        }
                    } else if old == 0 {
                        if material == modding::water::MATERIAL {
                            count += usize::from(self.water.edit_volume(p, true));
                        } else if !self.units.iter().any(|u| u.solid([x, y, z])) {
                            self.world.queue_edit(Edit::put(p[0], p[1], p[2], material));
                            count += 1;
                        }
                    }
                }
            }
        }
        if count == 0 {
            Outcome::Unchanged
        } else {
            Outcome::Queued
        }
    }
    pub(super) fn step_lab_devices(&mut self) {
        self.step_lab_collectors();
        let Some(layout) = self.layout.as_mut().filter(|l| l.laboratory) else {
            return;
        };
        let t = self.lab_tools_tuning;
        let count = layout.lab_devices.len();
        if count == 0 {
            return;
        }
        let first = self.tick as usize % count;
        let mut checks = t.device_checks;
        for turn in 0..count {
            let i = (first + turn) % count;
            let d = &mut layout.lab_devices[i];
            if d.kind==DeviceKind::Collector {continue;}
            if d.container.is_some_and(|id| {
                self.matter
                    .economy
                    .ledger
                    .try_get(id)
                    .is_none_or(|c| c.kind != Kind::Hauler || !c.alive)
            }) {
                d.container = None;
            }
            if d.container.is_none() {
                d.container = Some(self.matter.economy.ledger.create(Kind::Hauler, 0, u64::MAX));
            }
            d.work = d
                .work
                .saturating_add(t.device_work)
                .min(t.device_work.saturating_mul(t.movement_ticks));
            let axis = match d.kind {
                DeviceKind::Drill => [0, -1, 0],
                DeviceKind::Crawler => match d.turns {
                    0 => [1, 0, 0],
                    1 => [0, 0, 1],
                    2 => [-1, 0, 0],
                    _ => [0, 0, -1],
                },
                DeviceKind::Collector => unreachable!(),
            };
            let next: [i32; 3] = std::array::from_fn(|a| d.at[a] + axis[a]);
            let r = t.device_edge / 2;
            let mut obstructed = false;
            for z in -r..=r {
                for y in -r..=r {
                    for x in -r..=r {
                        if checks == 0 {
                            obstructed = true;
                            continue;
                        }
                        checks -= 1;
                        let p = [next[0] + x, next[1] + y, next[2] + z];
                        if (0..3).any(|a| p[a] <= 0 || p[a] >= self.size[a] as i32 - 1) {
                            obstructed = true;
                            continue;
                        }
                        let p = p.map(|v| v as u32);
                        let mat = self.world.material(p[0], p[1], p[2]);
                        if mat == 0 {
                            continue;
                        }
                        obstructed = true;
                        if self.world.on_frame(p[0], p[1], p[2])
                            || self.world.has_pending_dig(p[0], p[1], p[2])
                            || self.matter.debris.claimed(p)
                        {
                            continue;
                        }
                        let cost = self
                            .matter
                            .economy
                            .mats
                            .get(mat)
                            .map_or(u32::MAX, |m| m.strength);
                        if cost > d.work {
                            continue;
                        }
                        d.work -= cost;
                        let mut e = Edit::dig(p[0], p[1], p[2]);
                        e.by = DRILL | i as u32;
                        self.world.queue_edit(e);
                    }
                }
            }
            if !obstructed && self.tick % t.movement_ticks == 0 {
                if d.kind == DeviceKind::Drill
                    || self.world.solid(
                        next[0] as u32,
                        (next[1] - r - 1).max(0) as u32,
                        next[2] as u32,
                    )
                {
                    d.at = next;
                }
            }
        }
    }
    pub(super) fn credit_lab_edits(&mut self) {
        if !self.layout.as_ref().is_some_and(|l| l.laboratory) {
            return;
        }
        for &(e, mat) in self.world.completed() {
            if mat == 0 || e.mat != 0 {
                continue;
            }
            let tag = e.by & 0xffff_0000;
            if tag != ERASE && tag != DRILL {
                continue;
            }
            let economy = &mut self.matter.economy;
            let amount = economy.per_voxel(mat);
            economy.ledger.reveal(economy.ground, amount);
            if tag == ERASE {
                economy
                    .ledger
                    .sink(economy.ground, amount, Sink::Annihilation);
            } else if let Some(d) = self
                .layout
                .as_mut()
                .and_then(|l| l.lab_devices.get_mut((e.by & 0xffff) as usize))
            {
                if let Some(tank) = d.container {
                    economy.ledger.transfer(economy.ground, tank, amount);
                    d.dug += 1;
                }
            }
        }
    }
}
