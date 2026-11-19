//! Historical arm aim affects hit geometry, so preserve it across saves.
use super::{BlastRange, RangeUnitKind};
use fx::blob::{Blob, Scan};

impl BlastRange {
    pub(super) fn has_walker_poses(&self) -> bool {
        self.units
            .iter()
            .chain(std::iter::once(&self.gun))
            .any(|u| u.arm_targets.is_some())
    }
    pub(super) fn save_walker_poses(&self, b: &mut Blob) {
        let slots: Vec<_> = self
            .units
            .iter()
            .chain(std::iter::once(&self.gun))
            .enumerate()
            .filter_map(|(id, u)| u.arm_targets.map(|p| (id, p)))
            .collect();
        b.u32(slots.len() as u32);
        for (id, targets) in slots {
            b.u32(id as u32);
            for target in targets {
                for v in target {
                    b.i32(v);
                }
            }
        }
    }
    pub(super) fn load_walker_poses(&mut self, s: &mut Scan) -> Option<()> {
        let count = s.u32() as usize;
        if count == 0 || count > self.units.len() + 1 || count > s.left() / 52 {
            return None;
        }
        let mut last = None;
        for _ in 0..count {
            let id = s.u32() as usize;
            if id > self.units.len() || last.is_some_and(|prior| id <= prior) {
                return None;
            }
            last = Some(id);
            let targets: [[i32; 3]; 4] = std::array::from_fn(|_| std::array::from_fn(|_| s.i32()));
            let (unit, yaw) = if id == self.units.len() {
                (&mut self.gun, self.turret.angle)
            } else {
                (&mut self.units[id], self.battle.actors[id].turret.angle)
            };
            if !matches!(unit.kind, RangeUnitKind::Walker04 | RangeUnitKind::Commander)
                || unit.kind == RangeUnitKind::Commander && !matches!(targets[2], [0 | 1, 0, 0])
            {
                return None;
            }
            // Servo points are not world orders. During angular slew a
            // horizontal reach can rotate below a shallow map's Y extent.
            // Bound that reach by the world diagonal's L1 upper bound and
            // the data-defined idle reach, without changing the live pose.
            let commander_margin = self.size.iter().map(|v| i64::from(*v)).sum::<i64>()
                + i64::from(super::commander::tuning().rest_lookahead_cm);
            if targets.iter().any(|p| {
                (0..3).any(|a| {
                    let margin = if unit.kind == RangeUnitKind::Commander {
                        commander_margin
                    } else {
                        i64::from(self.size[a])
                    };
                    i64::from(p[a]) < -margin
                        || i64::from(p[a]) > i64::from(self.size[a]) + margin
                })
            }) {
                return None;
            }
            // The saved point already is the interpolated physical pose.
            unit.arm_targets = Some(targets);
            unit.update_pose(yaw);
        }
        s.ok().then_some(())
    }
}
