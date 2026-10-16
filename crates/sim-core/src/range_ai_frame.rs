//! One-step policy boundary. The caller owns the journal and must reserve its
//! sink before recording. No future tape is hidden in world/checkpoint state.
use super::*;
use crate::blast::battle::{orders, Order};
use fx::blob::{Blob, Scan};

#[cfg(test)]
#[path = "range_ai_frame_tests.rs"]
mod tests;

type Inputs = (Vec<Decision>, Option<Vec<Decision>>);

pub(crate) struct AiStep {
    pub replay: bool,
    pub input: Option<Inputs>,
    pub output: Option<Inputs>,
}

/// Rules inputs actually computed at AfterMotion, including enemy fallbacks
/// and autonomous allied decisions. External plans remain authoritative Inbox
/// inputs: restore/deliver them, and player commands/acks, before this step.
/// Private fields prevent accidental use as a general order submission API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiFrame {
    tick: u32,
    before_hash: u64,
    enemy: Vec<Decision>,
    allied: Option<Vec<Decision>>,
}

impl CommandRuntime {
    // Diagnostic preflight follows exactly the commands drained by step(). A
    // queued launch can make this tick the first AI deadline. Nothing here
    // mutates the world, so a missing frame still rejects atomically.
    fn ai_due(&self) -> Option<bool> {
        if !self.world.battle_mode() {
            return None;
        }
        let mut phase = self.world.battle.phase;
        let mut autonomous = self.world.battle.autonomous;
        let mut next_tick = self.world.battle.ai.next_tick;
        for entry in &self.inbox.entries {
            if entry.assigned_tick != self.world.tick || entry.outcome.is_some() {
                continue;
            }
            match entry.envelope.command {
                Command::ConfigureEnemyAi { tuning } if tuning.validate().is_ok() => {
                    next_tick = self.world.tick;
                }
                Command::BattleLifecycle(BattleLifecycle::Autonomy { enabled })
                    if phase == Phase::Setup =>
                {
                    autonomous = enabled
                }
                Command::BattleLifecycle(BattleLifecycle::StartBattle)
                    if phase == Phase::Setup && self.world.battle_start_rejection().is_none() =>
                {
                    phase = Phase::Fighting;
                    next_tick = self.world.tick;
                }
                Command::Order {
                    actor,
                    order,
                    destination,
                } => {
                    if !matches!(
                        self.world.manual_order_admission(
                            phase,
                            orders::Request {
                                actor,
                                order,
                                destination
                            }
                        ),
                        orders::Admission::Rejected
                    ) {
                        autonomous = false;
                    }
                }
                Command::FormationMove {
                    actor, destination, ..
                } => {
                    if !matches!(
                        self.world.manual_order_admission(
                            phase,
                            orders::Request {
                                actor,
                                order: Order::Move,
                                destination: Some(destination)
                            }
                        ),
                        orders::Admission::Rejected
                    ) {
                        autonomous = false;
                    }
                }
                _ => {}
            }
        }
        (phase == Phase::Fighting && self.world.tick >= next_tick).then_some(autonomous)
    }

    /// Record at most one phase; each actor contributes at most one decision.
    /// Hashing is explicit diagnostic work, only on AI deadlines. Ordinary
    /// step() neither records nor hashes. The caller bounds journal retention.
    pub fn step_record_ai(&mut self) -> Result<Option<AiFrame>, &'static str> {
        if self.ai_due().is_none() {
            self.step()?;
            return Ok(None);
        }
        if self.world.tick == u32::MAX {
            return Err("command runtime clock exhausted");
        }
        let tick = self.world.tick;
        let before_hash = self.state_hash();
        self.world.battle.ai_step = Some(Box::new(AiStep {
            replay: false,
            input: None,
            output: None,
        }));
        self.step()?;
        let (enemy, allied) = self.world.battle.ai_step.take().unwrap().output.unwrap();
        Ok(Some(AiFrame {
            tick,
            before_hash,
            enemy,
            allied,
        }))
    }

    /// Replay one step without policy execution. Missing/extra/wrong-world
    /// frames are rejected BEFORE commands or physics mutate the world.
    /// None explicitly means this step has no AI deadline; it never means
    /// "use Rules". Both capture and replay clear their transient hook on return.
    pub fn step_replay_ai(&mut self, frame: Option<AiFrame>) -> Result<(), &'static str> {
        if self.world.tick == u32::MAX {
            return Err("command runtime clock exhausted");
        }
        let due = self.ai_due();
        if due.is_some() != frame.is_some() {
            return Err("AI replay deadline mismatch");
        }
        if let Some(frame) = frame {
            if frame.tick != self.world.tick
                || frame.before_hash != self.state_hash()
                || Some(frame.allied.is_some()) != due
                || !frame.valid_for(&self.world)
            {
                return Err("AI replay frame mismatch");
            }
            self.world.battle.ai_step = Some(Box::new(AiStep {
                replay: true,
                input: Some((frame.enemy, frame.allied)),
                output: None,
            }));
            self.step()?;
            let step = self.world.battle.ai_step.take().unwrap();
            // The exact phase must consume the input once, even when empty.
            assert!(step.input.is_none() && step.output.is_some());
        } else {
            self.step()?;
        }
        Ok(())
    }
}

impl AiFrame {
    pub fn tick(&self) -> u32 {
        self.tick
    }
    pub fn decisions(&self) -> usize {
        self.enemy.len() + self.allied.as_ref().map_or(0, Vec::len)
    }
    fn valid_for(&self, world: &BlastRange) -> bool {
        let valid = |decisions: &[Decision], allied: bool| {
            decisions.windows(2).all(|w| w[0].actor < w[1].actor)
                && decisions.iter().all(|d| {
                    d.actor < world.units.len()
                        && world.allied(d.actor) == allied
                        && match d.order {
                            AiOrder::Attack(target) => {
                                target < world.units.len() && world.allied(target) != allied
                            }
                            _ => true,
                        }
                })
        };
        self.decisions() <= world.units.len()
            && valid(&self.enemy, false)
            && self.allied.as_ref().is_none_or(|v| valid(v, true))
    }

    /// Separate little-endian wire v1. No I/O. This checksum detects damaged
    /// local journal records; it does not authenticate decisions or policy.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Blob::new();
        b.u64(u64::from_le_bytes(*b"RTSAIF01"));
        b.u32(1);
        b.u32(self.tick);
        b.u64(self.before_hash);
        b.usize(self.enemy.len());
        for &d in &self.enemy {
            save_decision(&mut b, d);
        }
        b.bool(self.allied.is_some());
        if let Some(allied) = &self.allied {
            b.usize(allied.len());
            for &d in allied {
                save_decision(&mut b, d);
            }
        }
        b.u64(checksum(&b.v));
        b.v
    }

    /// The aggregate actor ceiling bounds allocations before any Vec reserve.
    /// Replay additionally validates frame/owner/sides. No physical save parse.
    pub fn from_bytes(bytes: &[u8], max_decisions: usize) -> Option<Self> {
        let end = bytes.len().checked_sub(8)?;
        let payload = &bytes[..end];
        if u64::from_le_bytes(bytes[end..].try_into().ok()?) != checksum(payload) {
            return None;
        }
        let mut s = Scan::new(payload);
        if s.u64() != u64::from_le_bytes(*b"RTSAIF01") || s.u32() != 1 {
            return None;
        }
        let tick = s.u32();
        let before_hash = s.u64();
        let enemy = read_decisions(&mut s, max_decisions)?;
        let allied = match s.u8() {
            0 => None,
            1 => Some(read_decisions(
                &mut s,
                max_decisions.checked_sub(enemy.len())?,
            )?),
            _ => return None,
        };
        (s.ok() && s.left() == 0).then_some(Self {
            tick,
            before_hash,
            enemy,
            allied,
        })
    }
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
fn read_decisions(s: &mut Scan<'_>, limit: usize) -> Option<Vec<Decision>> {
    let count = usize::try_from(s.u64()).ok()?;
    if !s.ok() || count > limit || count > s.left() / 9 {
        return None;
    }
    let mut v = Vec::new();
    v.try_reserve_exact(count).ok()?;
    for _ in 0..count {
        let actor = usize::try_from(s.u64()).ok()?;
        let order = match s.u8() {
            0 => AiOrder::Attack(usize::try_from(s.u64()).ok()?),
            1 => AiOrder::Move([s.i32(), s.i32()]),
            2 => AiOrder::Hold,
            3 => AiOrder::Auto,
            _ => return None,
        };
        if !s.ok() {
            return None;
        }
        v.push(Decision { actor, order });
    }
    Some(v)
}
