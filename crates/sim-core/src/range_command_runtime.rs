//! Single-thread owner for bounded external range and battle orders. The physical kernel
//! and its AI phase are unchanged. This adapter has no mutable world accessor.
use super::{BattleLifecycle, Command, Outcome};
use crate::blast::{
    ai::{AiOrder, Decision},
    battle::Phase,
    BlastRange,
};
use modding::explosion::ExplosionConfig;

#[path = "range_runtime_reset.rs"]
mod reset;
pub use reset::{ResetError, RetiredCommand, RetiredIngress, RetiredPlan};

#[cfg(test)]
#[path = "range_ai_plan_delivery_tests.rs"]
mod plan_delivery_tests;

#[path = "range_command_checkpoint.rs"]
mod checkpoint;
pub use checkpoint::DecodeLimits;

#[path = "range_ai_frame.rs"]
mod ai_frame;
pub use ai_frame::AiFrame;
pub(crate) use ai_frame::AiStep;

#[cfg(test)]
#[path = "range_command_checkpoint_tests.rs"]
mod checkpoint_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Includes pending commands AND completed receipts awaiting acknowledgement.
    pub retained: usize,
    pub producers: usize,
    pub per_tick: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope {
    /// Caller-issued initial generation; reset advances it without wrapping.
    pub epoch: u64,
    /// Dense producer ID in 0..Limits::producers; sequences start at zero.
    pub producer: usize,
    pub sequence: u64,
    pub requested_tick: u32,
    pub command: Command,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandId {
    pub epoch: u64,
    pub ordinal: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub id: CommandId,
    pub envelope: Envelope,
    pub assigned_tick: u32,
    /// None confirms scheduling, Some confirms execution, including rejection.
    pub outcome: Option<Outcome>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Epoch,
    Producer,
    Sequence { expected: u64 },
    PastTick,
    Full,
    Exhausted,
    Plan(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEnvelope {
    pub epoch: u64,
    pub producer: usize,
    pub sequence: u64,
    /// Must match the existing AI scheduler; plans are never shifted by the
    /// player command budget or split across ticks.
    pub decision_tick: u32,
    pub decisions: Vec<Decision>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanAttempt {
    pub decision: Decision,
    pub outcome: Outcome,
    pub fallback: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanState {
    Delivered,
    Executed {
        tick: u32,
        attempts: Vec<PlanAttempt>,
    },
    Cancelled {
        tick: u32,
    },
    /// A successfully applied AI configuration discarded this pending plan.
    Reconfigured {
        tick: u32,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanReceipt {
    pub id: CommandId,
    pub envelope: PlanEnvelope,
    pub delivered_tick: u32,
    pub state: PlanState,
}

// Transient handoff from the AI phase to its owner. The authoritative result
// lives in Inbox after step; a pending checkpoint re-arms this bounded buffer.
pub(crate) struct PlanExecution {
    pub tick: Option<u32>,
    pub attempts: Vec<PlanAttempt>,
}
impl PlanExecution {
    fn new(decisions: usize) -> Result<Box<Self>, &'static str> {
        let mut attempts = Vec::new();
        attempts
            .try_reserve_exact(decisions.checked_mul(2).ok_or("plan size overflow")?)
            .map_err(|_| "plan result allocation failed")?;
        Ok(Box::new(Self {
            tick: None,
            attempts,
        }))
    }
}

#[derive(Clone)]
struct Inbox {
    epoch: u64,
    limits: Limits,
    next: Vec<u64>,
    next_id: u64,
    // Sorted by (assigned tick, acceptance ID); no future command blocks an
    // earlier due command. Acceptance order resolves ties at the same tick.
    entries: Vec<Receipt>,
    plans: Vec<PlanReceipt>,
}

/// Checkpoint retaining the world AND authoritative ingress. Its versioned
/// byte representation is separate from the physical world's save format.
/// Saving only world().snapshot() loses ingress.
#[derive(Clone)]
pub struct Checkpoint {
    world: Vec<u8>,
    inbox: Inbox,
}

pub struct CommandRuntime {
    world: BlastRange,
    inbox: Inbox,
}

impl CommandRuntime {
    /// Own a range, a prepared roster or an already started battle. Launch and control
    /// commands use the same bounded ingress; reset_world retires that ingress.
    /// Limits are explicit owner configuration, not scenario physics settings.
    pub fn new(world: BlastRange, epoch: u64, limits: Limits) -> Result<Self, &'static str> {
        if world.battle_mode() && !matches!(world.battle.phase, Phase::Setup | Phase::Fighting) {
            return Err("command runtime needs a prepared or started battle");
        }
        Ok(Self {
            world,
            inbox: Inbox::empty(epoch, limits)?,
        })
    }
    pub fn epoch(&self) -> u64 {
        self.inbox.epoch
    }
    pub fn world(&self) -> &BlastRange {
        &self.world
    }
    pub fn retained(&self) -> usize {
        self.inbox.entries.len() + self.inbox.plans.len()
    }
    pub fn receipt(&self, id: CommandId) -> Option<Receipt> {
        self.inbox
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .copied()
    }
    /// Remove a completed receipt only after the consumer has received it.
    /// Producer watermarks survive acknowledgement, so retries cannot re-execute.
    pub fn acknowledge(&mut self, id: CommandId) -> Option<Receipt> {
        let at = self
            .inbox
            .entries
            .iter()
            .position(|entry| entry.id == id && entry.outcome.is_some())?;
        Some(self.inbox.entries.remove(at))
    }
    pub fn plan_receipt(&self, id: CommandId) -> Option<&PlanReceipt> {
        self.inbox.plans.iter().find(|receipt| receipt.id == id)
    }
    pub fn acknowledge_plan(&mut self, id: CommandId) -> Option<PlanReceipt> {
        let at = self
            .inbox
            .plans
            .iter()
            .position(|receipt| receipt.id == id && receipt.state != PlanState::Delivered)?;
        Some(self.inbox.plans.remove(at))
    }
    /// Deliver one whole enemy plan atomically through the legacy validator.
    /// Scheduling acceptance is distinct from per-actor execution/fallback.
    /// Sequences, IDs and retention capacity are shared with player commands.
    pub fn submit_plan(&mut self, mut envelope: PlanEnvelope) -> Result<CommandId, AdmissionError> {
        use AdmissionError as E;
        if envelope.epoch != self.inbox.epoch {
            return Err(E::Epoch);
        }
        let Some(&expected) = self.inbox.next.get(envelope.producer) else {
            return Err(E::Producer);
        };
        if envelope.sequence != expected {
            return Err(E::Sequence { expected });
        }
        if self.retained() == self.inbox.limits.retained {
            return Err(E::Full);
        }
        let next_sequence = expected.checked_add(1).ok_or(E::Exhausted)?;
        let next_id = self.inbox.next_id.checked_add(1).ok_or(E::Exhausted)?;
        if envelope.decisions.len() > self.world.units.len() {
            return Err(E::Plan("AI plan must cover every living enemy"));
        }
        // All storage is reserved before mutating controller.pending. Each
        // selected decision can produce at most one attempt and one fallback.
        let execution = PlanExecution::new(envelope.decisions.len()).map_err(E::Plan)?;
        // Do not retain caller-controlled spare Vec capacity. A tiny plan
        // backed by a huge allocation must not bypass the owner's memory bound.
        let mut retained = Vec::new();
        retained
            .try_reserve_exact(envelope.decisions.len())
            .map_err(|_| E::Plan("plan retention allocation failed"))?;
        retained.extend_from_slice(&envelope.decisions);
        envelope.decisions = retained;
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(envelope.decisions.len())
            .map_err(|_| E::Plan("plan allocation failed"))?;
        pending.extend_from_slice(&envelope.decisions);
        self.world
            .submit_enemy_ai_plan(envelope.decision_tick, pending)
            .map_err(E::Plan)?;
        self.world.battle.plan_execution = Some(execution);
        let id = CommandId {
            epoch: self.inbox.epoch,
            ordinal: self.inbox.next_id,
        };
        let producer = envelope.producer;
        self.inbox.plans.push(PlanReceipt {
            id,
            envelope,
            delivered_tick: self.world.tick,
            state: PlanState::Delivered,
        });
        self.inbox.next[producer] = next_sequence;
        self.inbox.next_id = next_id;
        Ok(id)
    }
    /// Scheduling is distinct from gameplay validation at execution. Commands
    /// for dead actors/targets still produce a retained Rejected outcome.
    pub fn submit(&mut self, envelope: Envelope) -> Result<Receipt, AdmissionError> {
        use AdmissionError as E;
        let inbox = &mut self.inbox;
        if envelope.epoch != inbox.epoch {
            return Err(E::Epoch);
        }
        let Some(&expected) = inbox.next.get(envelope.producer) else {
            return Err(E::Producer);
        };
        if envelope.sequence != expected {
            return Err(E::Sequence { expected });
        }
        if envelope.requested_tick < self.world.tick {
            return Err(E::PastTick);
        }
        if inbox.entries.len() + inbox.plans.len() == inbox.limits.retained {
            return Err(E::Full);
        }
        let next_sequence = expected.checked_add(1).ok_or(E::Exhausted)?;
        let next_id = inbox.next_id.checked_add(1).ok_or(E::Exhausted)?;
        let mut assigned_tick = envelope.requested_tick;
        // One bounded pass over sorted reservations; no quadratic rescan of
        // a saturated sequence of ticks. Existing assignments never move.
        let mut reserved = 0;
        for entry in &inbox.entries {
            if entry.outcome.is_some() || entry.assigned_tick < assigned_tick {
                continue;
            }
            if entry.assigned_tick > assigned_tick {
                break;
            }
            reserved += 1;
            if reserved == inbox.limits.per_tick {
                assigned_tick = assigned_tick.checked_add(1).ok_or(E::Exhausted)?;
                reserved = 0;
            }
        }
        // The physical clock increments at the END of a step.
        if assigned_tick == u32::MAX {
            return Err(E::Exhausted);
        }
        let receipt = Receipt {
            id: CommandId {
                epoch: inbox.epoch,
                ordinal: inbox.next_id,
            },
            envelope,
            assigned_tick,
            outcome: None,
        };
        let at = inbox.entries.partition_point(|entry| {
            (entry.assigned_tick, entry.id.ordinal) < (assigned_tick, receipt.id.ordinal)
        });
        inbox.entries.insert(at, receipt);
        inbox.next[envelope.producer] = next_sequence;
        inbox.next_id = next_id;
        Ok(receipt)
    }
    /// Drain external orders before ANY tick work. AI still runs after motion.
    /// Admission reserves both the per-tick slot and its eventual receipt.
    pub fn step(&mut self) -> Result<(), &'static str> {
        if self.world.tick == u32::MAX {
            return Err("command runtime clock exhausted");
        }
        for entry in &mut self.inbox.entries {
            if entry.outcome.is_none() && entry.assigned_tick == self.world.tick {
                entry.outcome = Some(self.world.execute_command(entry.envelope.command));
                if matches!(entry.envelope.command, Command::ConfigureEnemyAi { .. })
                    && entry.outcome == Some(Outcome::Applied)
                    && self.world.battle.plan_execution.take().is_some()
                {
                    // Finalize before AfterMotion: the new Rules decision is
                    // not execution of the previously delivered external plan.
                    self.inbox
                        .plans
                        .iter_mut()
                        .rev()
                        .find(|receipt| receipt.state == PlanState::Delivered)
                        .expect("owned pending AI plan")
                        .state = PlanState::Reconfigured {
                        tick: self.world.tick,
                    };
                }
            }
        }
        self.world.step();
        if self
            .world
            .battle
            .plan_execution
            .as_ref()
            .is_some_and(|execution| execution.tick.is_some())
        {
            let execution = self.world.battle.plan_execution.take().unwrap();
            let receipt = self
                .inbox
                .plans
                .iter_mut()
                .rev()
                .find(|receipt| receipt.state == PlanState::Delivered)
                .unwrap();
            receipt.state = PlanState::Executed {
                tick: execution.tick.unwrap(),
                attempts: execution.attempts,
            };
        } else if self.world.battle.phase != Phase::Fighting
            && self.world.battle.plan_execution.take().is_some()
        {
            let receipt = self
                .inbox
                .plans
                .iter_mut()
                .rev()
                .find(|receipt| receipt.state == PlanState::Delivered)
                .unwrap();
            receipt.state = PlanState::Cancelled {
                tick: self.world.tick - 1,
            };
        }
        Ok(())
    }
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            world: self.world.snapshot(),
            inbox: self.inbox.clone(),
        }
    }
    pub fn restore(config: ExplosionConfig, checkpoint: Checkpoint) -> Option<Self> {
        let world = BlastRange::restore(config, &checkpoint.world)?;
        checkpoint.inbox.valid_for(&world).then_some(())?;
        // Clone need not preserve spare Vec capacity. Reserve before use,
        // rather than allocating when a restored inbox accepts a command.
        let mut inbox = checkpoint.inbox;
        inbox
            .entries
            .try_reserve_exact(inbox.limits.retained - inbox.entries.len())
            .ok()?;
        inbox
            .plans
            .try_reserve_exact(inbox.limits.retained - inbox.plans.len())
            .ok()?;
        let mut world = world;
        if let Some(receipt) = inbox
            .plans
            .iter()
            .find(|receipt| receipt.state == PlanState::Delivered)
        {
            if world.battle.ai.pending.as_ref()? != &receipt.envelope.decisions {
                return None;
            }
            world.battle.plan_execution =
                Some(PlanExecution::new(receipt.envelope.decisions.len()).ok()?);
        }
        Some(Self { world, inbox })
    }
    /// Physical hash plus all future-affecting ingress and retained receipts.
    /// Diagnostic call; not used by scheduling or the normal tick path.
    pub fn state_hash(&self) -> u64 {
        use fx::blob::Blob;
        let mut b = Blob::new();
        b.u64(self.world.state_hash());
        checkpoint::save_inbox(&mut b, &self.inbox);
        b.v.into_iter().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
    }
}

fn save_decision(b: &mut fx::blob::Blob, decision: Decision) {
    b.usize(decision.actor);
    match decision.order {
        AiOrder::Attack(target) => {
            b.u8(0);
            b.usize(target);
        }
        AiOrder::Move(at) => {
            b.u8(1);
            for x in at {
                b.i32(x);
            }
        }
        AiOrder::Hold => b.u8(2),
        AiOrder::Auto => b.u8(3),
    }
}

#[cfg(test)]
#[path = "range_ai_configuration_tests.rs"]
mod ai_configuration_tests;

#[cfg(test)]
#[path = "range_gun_command_tests.rs"]
mod gun_command_tests;
