//! Live debris controls and the early physical tuning prelude. The complete
//! Blast configuration is saved: its storage ceilings validate later sections.
use super::{battle::commands::Outcome, BlastRange};
use fx::blob::{Blob, Scan};
use modding::tuning::Blast;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DebrisField {
    DragPct = 0,
    BouncePct = 1,
    FrictionPct = 2,
    SleepMmS = 3,
    PushPct = 4,
    PushUpPct = 5,
    RestCheckTicks = 6,
    PileCell = 7,
    ScatterPct = 8,
    SpeedMinPct = 9,
    SmallPiecePct = 10,
    LiftPct = 11,
    MotionPct = 12,
    MaxFlying = 13,
}
impl DebrisField {
    pub const ALL: [Self; 14] = [
        Self::DragPct,
        Self::BouncePct,
        Self::FrictionPct,
        Self::SleepMmS,
        Self::PushPct,
        Self::PushUpPct,
        Self::RestCheckTicks,
        Self::PileCell,
        Self::ScatterPct,
        Self::SpeedMinPct,
        Self::SmallPiecePct,
        Self::LiftPct,
        Self::MotionPct,
        Self::MaxFlying,
    ];

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    fn slot(self, t: &mut Blast) -> &mut u32 {
        match self {
            Self::DragPct => &mut t.drag_pct,
            Self::BouncePct => &mut t.bounce_pct,
            Self::FrictionPct => &mut t.friction_pct,
            Self::SleepMmS => &mut t.sleep_mm_s,
            Self::PushPct => &mut t.push_pct,
            Self::PushUpPct => &mut t.push_up_pct,
            Self::RestCheckTicks => &mut t.rest_check_ticks,
            Self::PileCell => &mut t.pile_cell,
            Self::ScatterPct => &mut t.scatter_pct,
            Self::SpeedMinPct => &mut t.speed_min_pct,
            Self::SmallPiecePct => &mut t.small_piece_pct,
            Self::LiftPct => &mut t.lift_pct,
            Self::MotionPct => &mut t.motion_pct,
            Self::MaxFlying => &mut t.max_flying,
        }
    }
    fn normalized(self, value: u32, t: &Blast) -> u32 {
        match self {
            Self::DragPct
            | Self::FrictionPct
            | Self::PushUpPct
            | Self::ScatterPct
            | Self::SpeedMinPct
            | Self::SmallPiecePct
            | Self::LiftPct => value.min(100),
            Self::BouncePct | Self::MotionPct => value.min(99),
            Self::SleepMmS | Self::RestCheckTicks | Self::PileCell => value.max(1),
            Self::PushPct => value,
            // A zero working ceiling is valid in internal capacity tests.
            // Live settings cannot open a slot when the enclosing pool is shut.
            Self::MaxFlying => value.max(1).min(t.max_fragments),
        }
    }
}

// Explicit, stable snapshot order; never derive it from a struct's memory layout.
// Building Blast with every named field makes additions require a codec decision.
macro_rules! blast_codec {
    ($($field:ident),+ $(,)?) => {
        fn values(t: &Blast) -> [u32; 32] { [$(t.$field),+] }
        fn from_values(v: [u32; 32]) -> Blast {
            let [$($field),+] = v;
            Blast { $($field),+ }
        }
    };
}
blast_codec!(
    cell,
    samples_per_tick,
    wave_pops_per_tick,
    edits_per_tick,
    max_pending,
    max_claims,
    max_fragments,
    max_flying,
    max_resting_fragments,
    pile_cell,
    fragment_box,
    resistance_per_strength,
    fracture_kj_m3_per_strength,
    density_kg_m3_per_density,
    motion_pct,
    body_energy_pct,
    body_resistance_ppm,
    lift_pct,
    scatter_pct,
    speed_min_pct,
    small_piece_pct,
    max_speed_mm_s,
    drag_pct,
    bounce_pct,
    friction_pct,
    sleep_mm_s,
    rest_check_ticks,
    push_pct,
    push_up_pct,
    spin_deg_s,
    contact_deform_pct,
    contact_min_mm_s
);

impl BlastRange {
    /// No allocation or fragment traversal. Lowering admission does not erase
    /// existing matter; the ordinary kernel waits for capacity to become free.
    pub(super) fn tune_debris(&mut self, field: DebrisField, value: u32) -> Outcome {
        let value = field.normalized(value, &self.matter.tuning);
        let slot = field.slot(&mut self.matter.tuning);
        if *slot == value {
            return Outcome::Unchanged;
        }
        *slot = value;
        // Claim groups are keyed by the piece boxes this tuning draws.
        self.matter.debris.regroup(&self.matter.tuning);
        Outcome::Applied
    }

    pub(super) fn has_debris_tuning(&self) -> bool {
        self.matter.tuning != self.debris_defaults
    }

    pub(super) fn save_debris_tuning(&self, b: &mut Blob) {
        let current = values(&self.matter.tuning);
        let mask = current
            .iter()
            .zip(values(&self.debris_defaults))
            .enumerate()
            .fold(0u32, |mask, (index, (&value, base))| {
                mask | (u32::from(value != base) << index)
            });
        b.u32(mask);
        for (index, value) in current.into_iter().enumerate() {
            if mask & (1 << index) != 0 {
                b.u32(value);
            }
        }
    }

    pub(super) fn load_debris_tuning(&mut self, s: &mut Scan) -> Option<()> {
        let mask = s.u32();
        if mask == 0 || mask.count_ones() as usize > s.left() / 4 {
            return None;
        }
        let mut current = values(&self.debris_defaults);
        for (index, value) in current.iter_mut().enumerate() {
            if mask & (1 << index) == 0 {
                continue;
            }
            let saved = s.u32();
            if saved == *value {
                return None;
            }
            *value = saved;
        }
        let restored = from_values(current);
        // Divisors and box geometry must be usable by the next physical step.
        // Admission ceilings may legitimately be zero (a closed pool).
        if !s.ok()
            || restored.pile_cell == 0
            || restored.motion_pct >= 100
            || !(1..=4).contains(&restored.fragment_box)
        {
            return None;
        }
        self.matter.tuning = restored;
        Some(())
    }
}

#[cfg(test)]
#[path = "range_debris_tuning_tests.rs"]
mod tests;
