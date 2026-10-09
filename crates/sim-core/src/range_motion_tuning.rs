//! Integer-only live chassis edits. Defaults are derived from the constructed
//! layout; sparse snapshot deltas preserve the legacy bytes of untuned worlds.
use super::{battle::commands::Outcome, BlastRange, Motion, RangeUnitKind};
use fx::blob::{Blob, Scan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MotionField {
    Speed = 0,
    Step = 1,
    Stiffness = 2,
    Damping = 3,
    TravelCm = 4,
    ClearanceCm = 5,
    Inertia = 6,
    AngularDamping = 7,
    MaxTiltPct = 8,
    ContactCm = 9,
    ApproachPct = 10,
    SettlePct = 11,
    SettleTiltPct = 12,
    Thrust = 13,
    Brake = 14,
    Turn = 15,
    Climb = 16,
    Descend = 17,
}
impl MotionField {
    pub const ALL: [Self; 18] = [
        Self::Speed,
        Self::Step,
        Self::Stiffness,
        Self::Damping,
        Self::TravelCm,
        Self::ClearanceCm,
        Self::Inertia,
        Self::AngularDamping,
        Self::MaxTiltPct,
        Self::ContactCm,
        Self::ApproachPct,
        Self::SettlePct,
        Self::SettleTiltPct,
        Self::Thrust,
        Self::Brake,
        Self::Turn,
        Self::Climb,
        Self::Descend,
    ];

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    fn slot(self, m: &mut Motion) -> &mut i32 {
        match self {
            Self::Speed => &mut m.speed,
            Self::Step => &mut m.step,
            Self::Stiffness => &mut m.chassis.stiffness,
            Self::Damping => &mut m.chassis.damping,
            Self::TravelCm => &mut m.chassis.travel_cm,
            Self::ClearanceCm => &mut m.chassis.clearance_cm,
            Self::Inertia => &mut m.chassis.inertia,
            Self::AngularDamping => &mut m.chassis.ang_damp,
            Self::MaxTiltPct => &mut m.chassis.max_tilt_pct,
            Self::ContactCm => &mut m.chassis.contact_cm,
            Self::ApproachPct => &mut m.chassis.approach_pct,
            Self::SettlePct => &mut m.chassis.settle_pct,
            Self::SettleTiltPct => &mut m.chassis.settle_tilt_pct,
            Self::Thrust => &mut m.chassis.thrust,
            Self::Brake => &mut m.chassis.brake,
            Self::Turn => &mut m.chassis.turn,
            Self::Climb => &mut m.chassis.climb,
            Self::Descend => &mut m.chassis.descend,
        }
    }

    fn normalized(self, value: i32) -> i32 {
        match self {
            Self::Speed => value.max(1),
            Self::Step => value.clamp(1, 8),
            _ => value,
        }
    }
}

pub(super) type Values = [i32; MotionField::ALL.len()];
pub(super) fn values(m: &Motion) -> Values {
    let c = &m.chassis;
    [
        m.speed,
        m.step,
        c.stiffness,
        c.damping,
        c.travel_cm,
        c.clearance_cm,
        c.inertia,
        c.ang_damp,
        c.max_tilt_pct,
        c.contact_cm,
        c.approach_pct,
        c.settle_pct,
        c.settle_tilt_pct,
        c.thrust,
        c.brake,
        c.turn,
        c.climb,
        c.descend,
    ]
}

impl BlastRange {
    /// One atomic, synchronous group edit, in roster order. Both sides and
    /// dead slots are included, matching the original tuning surface. The gun
    /// is a separate explicitly requested target, regardless of its kind.
    pub(super) fn tune_motion(
        &mut self,
        kind: RangeUnitKind,
        include_gun: bool,
        field: MotionField,
        value: i32,
    ) -> Outcome {
        let value = field.normalized(value);
        let mut found = false;
        let mut changed = false;
        for u in self
            .units
            .iter_mut()
            .filter(|u| u.kind == kind)
            .chain(include_gun.then_some(&mut self.gun))
        {
            found = true;
            let slot = field.slot(&mut u.motion);
            changed |= *slot != value;
            *slot = value;
        }
        if changed {
            Outcome::Applied
        } else if found {
            Outcome::Unchanged
        } else {
            Outcome::Rejected
        }
    }

    pub(super) fn has_motion_tuning(&self) -> bool {
        self.units
            .iter()
            .chain(std::iter::once(&self.gun))
            .zip(&self.motion_defaults)
            .any(|(u, base)| values(&u.motion) != *base)
    }

    pub(super) fn save_motion_tuning(&self, b: &mut Blob) {
        let changed = || {
            self.units
                .iter()
                .chain(std::iter::once(&self.gun))
                .zip(&self.motion_defaults)
                .enumerate()
                .filter_map(|(id, (u, base))| {
                    let current = values(&u.motion);
                    let mask = current
                        .iter()
                        .zip(base)
                        .enumerate()
                        .fold(0u32, |mask, (field, (a, b))| {
                            mask | (u32::from(a != b) << field)
                        });
                    (mask != 0).then_some((id, mask, current))
                })
        };
        b.u32(changed().count() as u32);
        for (id, mask, current) in changed() {
            b.u32(id as u32);
            b.u32(mask);
            for (field, value) in current.into_iter().enumerate() {
                if mask & (1 << field) != 0 {
                    b.i32(value);
                }
            }
        }
    }

    pub(super) fn load_motion_tuning(&mut self, s: &mut Scan) -> Option<()> {
        let count = s.u32() as usize;
        if count == 0 || count > self.units.len() + 1 || count > s.left() / 12 {
            return None;
        }
        let mut previous = None;
        for _ in 0..count {
            let id = s.u32() as usize;
            let mask = s.u32();
            if previous.is_some_and(|p| p >= id)
                || id > self.units.len()
                || mask == 0
                || mask >> MotionField::ALL.len() != 0
            {
                return None;
            }
            previous = Some(id);
            let base = self.motion_defaults.get(id)?;
            let u = if id == self.units.len() {
                &mut self.gun
            } else {
                &mut self.units[id]
            };
            for (index, field) in MotionField::ALL.into_iter().enumerate() {
                if mask & (1 << index) == 0 {
                    continue;
                }
                let value = s.i32();
                if value == base[index] {
                    return None;
                }
                *field.slot(&mut u.motion) = value;
            }
        }
        s.ok().then_some(())
    }
}

#[cfg(test)]
#[path = "range_motion_tuning_tests.rs"]
mod tests;
