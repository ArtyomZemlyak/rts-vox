//! A synchronous lifecycle boundary, separate from per-tick gameplay input.
//! The caller owns the returned retirement before releasing its old journal.
use super::*;
use crate::blast::placement::Validator;
use modding::range::RangeLayout;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResetError {
    Epoch,
    Exhausted,
    Allocation(&'static str),
    World(String),
}

/// The original receipt is retained unchanged. Cancelled means the reset
/// prevented execution, not that gameplay validation returned Rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetiredCommand {
    Completed(Receipt),
    Cancelled(Receipt),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetiredPlan<'a> {
    Completed(&'a PlanReceipt),
    Cancelled(&'a PlanReceipt),
}

/// Bounded by the old owner's shared retention limit. Acknowledged results
/// are absent. This handoff is not part of the new world's checkpoint; a
/// durable caller must persist it in its own lifecycle journal before drop.
#[must_use = "consume or persist the retired receipts before discarding the old generation"]
pub struct RetiredIngress {
    pub epoch: u64,
    pub next_epoch: u64,
    pub tick: u32,
    commands: Vec<Receipt>,
    plans: Vec<PlanReceipt>,
}
impl RetiredIngress {
    pub fn commands(&self) -> impl ExactSizeIterator<Item = RetiredCommand> + '_ {
        self.commands.iter().map(|receipt| {
            if receipt.outcome.is_some() {
                RetiredCommand::Completed(*receipt)
            } else {
                RetiredCommand::Cancelled(*receipt)
            }
        })
    }
    pub fn plans(&self) -> impl ExactSizeIterator<Item = RetiredPlan<'_>> + '_ {
        self.plans.iter().map(|receipt| {
            if receipt.state == PlanState::Delivered {
                RetiredPlan::Cancelled(receipt)
            } else {
                RetiredPlan::Completed(receipt)
            }
        })
    }
}

impl Inbox {
    pub(super) fn empty(epoch: u64, limits: Limits) -> Result<Self, &'static str> {
        if epoch == 0 || limits.retained == 0 || limits.producers == 0 || limits.per_tick == 0 {
            return Err("command runtime needs nonzero epoch and limits");
        }
        let mut next = Vec::new();
        next.try_reserve_exact(limits.producers)
            .map_err(|_| "producer allocation failed")?;
        next.resize(limits.producers, 0);
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(limits.retained)
            .map_err(|_| "receipt allocation failed")?;
        let mut plans = Vec::new();
        plans
            .try_reserve_exact(limits.retained)
            .map_err(|_| "plan receipt allocation failed")?;
        Ok(Self {
            epoch,
            limits,
            next,
            next_id: 0,
            entries,
            plans,
        })
    }
}

impl CommandRuntime {
    /// Commit a fresh experiment and generation together. Stale requests,
    /// epoch exhaustion, allocation and validation failures preserve both
    /// world and inbox. No callbacks observe the temporarily moved world.
    /// Reset is a boundary between ticks, not a budgeted command or AI frame.
    pub fn reset_world(
        &mut self,
        expected_epoch: u64,
        config: ExplosionConfig,
        layout: Option<RangeLayout>,
        validator: &Validator,
    ) -> Result<RetiredIngress, ResetError> {
        if expected_epoch != self.inbox.epoch {
            return Err(ResetError::Epoch);
        }
        let next_epoch = self
            .inbox
            .epoch
            .checked_add(1)
            .ok_or(ResetError::Exhausted)?;
        // Reserve all new ingress storage before the reset can move occupancy.
        let next = Inbox::empty(next_epoch, self.inbox.limits).map_err(ResetError::Allocation)?;
        let tick = self.world.tick;
        let mut world = self
            .world
            .reset_candidate(config, layout, validator)
            .map_err(ResetError::World)?;
        // Execution configuration belongs to the owner. Move the existing
        // idle pool, avoiding a fallible thread-pool creation after reset.
        world.navigation_workers = self.world.navigation_workers;
        world.navigation_pool = self.world.navigation_pool.take();
        self.world = world;
        let previous = std::mem::replace(&mut self.inbox, next);
        Ok(RetiredIngress {
            epoch: previous.epoch,
            next_epoch,
            tick,
            commands: previous.entries,
            plans: previous.plans,
        })
    }
}

#[cfg(test)]
#[path = "range_runtime_reset_tests.rs"]
mod tests;
