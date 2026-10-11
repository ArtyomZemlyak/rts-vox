//! Synchronous command receipts and optional bounded observation. Capture is
//! derived: it cannot schedule commands, change physics or replace a save.
use super::Order;
pub use crate::blast::{
    debris_tuning::DebrisField, material_tuning::MaterialField, motion_tuning::MotionField,
};

#[path = "range_command_runtime.rs"]
pub mod runtime;

/// Transitions of an existing authored roster; rebuilding a world is separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BattleLifecycle {
    StartBattle,
    FreeControl,
    Autonomy { enabled: bool },
}

/// Manual range input. Automatic targeting uses internal operations instead
/// of feeding its own decisions back into the external command journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GunCommand {
    Control,
    Move { destination: [i32; 2] },
    Attack { target: usize },
    Stop,
    AutoAttack { enabled: bool },
    GroundFire { at: [i32; 3] },
    Refire,
    Aim { at: Option<[i32; 3]> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Gun(GunCommand),
    SetPatrols {
        enabled: bool,
    },
    ConfigureEnemyAi {
        tuning: modding::battle_ai::Tuning,
    },
    BattleLifecycle(BattleLifecycle),
    SetBattleWeapon {
        actor: usize,
        weapon: Option<super::Weapon>,
    },
    ConfigureWeapon {
        config: modding::explosion::ExplosionConfig,
    },
    TuneMaterial {
        material: modding::Mat,
        field: MaterialField,
        value: u32,
    },
    TuneDebris {
        field: DebrisField,
        value: u32,
    },
    TuneMotion {
        kind: crate::blast::RangeUnitKind,
        include_gun: bool,
        field: MotionField,
        value: i32,
    },
    Order {
        actor: usize,
        order: Order,
        destination: Option<[i32; 2]>,
    },
    FormationMove {
        actor: usize,
        destination: [i32; 2],
        fallback: [i32; 2],
    },
    /// Queue a unit of `kind` at producer `actor` (ADR-675).
    Produce {
        actor: usize,
        kind: crate::blast::RangeUnitKind,
    },
    /// Cancel the last order queued at producer `actor` (ADR-675).
    Cancel {
        actor: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Rejected,
    Unchanged,
    Applied,
    Queued,
    PathFailed,
    /// No round was charged. GroundFire still updates aim and cancels the
    /// previous automatic target, matching the synchronous control contract.
    Busy,
}
impl Outcome {
    pub fn admitted(self) -> bool {
        self != Self::Rejected
    }
    pub fn accepted(self) -> bool {
        matches!(self, Self::Unchanged | Self::Applied | Self::Queued)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Producer {
    Player = 0,
    EnemyRules = 1,
    EnemyPlan = 2,
    EnemyFallback = 3,
    AlliedRules = 4,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandPhase {
    BetweenTicks,
    AfterMotion,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub requested_tick: u32,
    pub assigned_tick: u32,
    pub phase: CommandPhase,
    pub producer: Producer,
    /// Sequence within this capture session and producer, not a network ID.
    pub sequence: u64,
    pub command: Command,
    pub outcome: Outcome,
}
#[derive(Debug)]
pub struct CommandTrace {
    pub start_tick: u32,
    pub end_tick: u32,
    pub records: Vec<Record>,
    pub omitted: u64,
}
impl CommandTrace {
    pub fn complete(&self) -> bool {
        self.omitted == 0
    }
}

pub(super) struct Capture {
    start_tick: u32,
    limit: usize,
    sequence: [u64; 5],
    records: Vec<Record>,
    omitted: u64,
}
impl Capture {
    pub fn new(tick: u32, limit: usize) -> Result<Self, &'static str> {
        if limit == 0 {
            return Err("command capture needs a positive record limit");
        }
        let mut records = Vec::new();
        records
            .try_reserve_exact(limit)
            .map_err(|_| "command capture allocation failed")?;
        Ok(Self {
            start_tick: tick,
            limit,
            sequence: [0; 5],
            records,
            omitted: 0,
        })
    }
    pub fn remaining(&self) -> usize {
        self.limit - self.records.len()
    }
    pub fn omit(&mut self, count: usize) {
        self.omitted = self.omitted.saturating_add(count as u64);
    }
    pub fn record(
        &mut self,
        requested_tick: u32,
        assigned_tick: u32,
        phase: CommandPhase,
        producer: Producer,
        command: Command,
        outcome: Outcome,
    ) {
        if self.remaining() == 0 {
            self.omit(1);
            return;
        }
        let sequence = &mut self.sequence[producer as usize];
        let Some(next) = sequence.checked_add(1) else {
            self.omit(1);
            return;
        };
        self.records.push(Record {
            requested_tick,
            assigned_tick,
            phase,
            producer,
            sequence: *sequence,
            command,
            outcome,
        });
        *sequence = next;
    }
    pub fn finish(self, tick: u32) -> CommandTrace {
        CommandTrace {
            start_tick: self.start_tick,
            end_tick: tick,
            records: self.records,
            omitted: self.omitted,
        }
    }
}
