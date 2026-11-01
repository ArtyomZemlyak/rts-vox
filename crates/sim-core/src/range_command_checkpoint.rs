use super::*;
use fx::blob::{Blob, Scan};

const MAGIC: u64 = u64::from_le_bytes(*b"RTSCMD01");
const VERSION: u32 = 1;

/// Caller-owned ceilings for loading local checkpoints, not gameplay quotas.
/// The embedded physical snapshot uses the existing trusted-save loader.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    pub world_bytes: usize,
    pub runtime: Limits,
    pub plan_decisions: usize,
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

impl Checkpoint {
    /// Explicit little-endian wire format. No I/O, policy execution or tick
    /// work; acknowledged receipts are intentionally absent from this save.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Keep the large world copy at exact final capacity. Appending even
        // one metadata field to an exactly full world Vec could double it.
        let mut metadata = Blob::new();
        save_inbox(&mut metadata, &self.inbox);
        let length = self
            .world
            .len()
            .checked_add(metadata.len())
            .and_then(|n| n.checked_add(28))
            .expect("checkpoint size overflow");
        let mut b = Blob {
            v: Vec::with_capacity(length),
        };
        b.u64(MAGIC);
        b.u32(VERSION);
        b.usize(self.world.len());
        b.v.extend_from_slice(&self.world);
        b.v.extend_from_slice(&metadata.v);
        b.u64(checksum(&b.v));
        b.v
    }

    /// Check wire structure, checksum and explicit allocation ceilings before
    /// constructing a checkpoint. Restore also validates ingress against world.
    /// The checksum detects damage; it is not transport authentication.
    pub fn from_bytes(bytes: &[u8], limits: DecodeLimits) -> Option<Self> {
        let end = bytes.len().checked_sub(8)?;
        let payload = &bytes[..end];
        if u64::from_le_bytes(bytes[end..].try_into().ok()?) != checksum(payload) {
            return None;
        }
        let mut header = Reader(Scan::new(payload));
        if header.0.u64() != MAGIC || header.0.u32() != VERSION {
            return None;
        }
        let length = header.size()?;
        if !header.0.ok() || length > limits.world_bytes {
            return None;
        }
        let world_end = 20usize.checked_add(length)?;
        let world = payload.get(20..world_end)?;
        let mut r = Reader(Scan::new(payload.get(world_end..)?));
        let epoch = r.0.u64();
        let retained = r.size()?;
        let producers = r.size()?;
        let per_tick = r.size()?;
        if epoch == 0
            || retained == 0
            || producers == 0
            || per_tick == 0
            || retained > limits.runtime.retained
            || producers > limits.runtime.producers
            || per_tick > limits.runtime.per_tick
        {
            return None;
        }
        let next_id = r.0.u64();
        let mut next = r.vector(producers, 8)?;
        for _ in 0..producers {
            next.push(r.0.u64());
        }
        let count = r.count(retained)?;
        // Header/outcome 49 + tag/target/subtag 10 (lifecycle/ammunition).
        let mut entries = r.vector(count, 59)?;
        for _ in 0..count {
            let id = r.id()?;
            let epoch = r.0.u64();
            let producer = r.size()?;
            let sequence = r.0.u64();
            let requested_tick = r.0.u32();
            let assigned_tick = r.0.u32();
            let outcome = match r.0.u8() {
                0 => None,
                tag @ 1..=6 => Some(outcome(tag - 1)?),
                _ => return None,
            };
            let command = r.command()?;
            entries.push(Receipt {
                id,
                envelope: Envelope {
                    epoch,
                    producer,
                    sequence,
                    requested_tick,
                    command,
                },
                assigned_tick,
                outcome,
            });
        }
        let count = r.count(retained.checked_sub(entries.len())?)?;
        let mut plans = r.vector(count, 57)?;
        for _ in 0..count {
            let id = r.id()?;
            let epoch = r.0.u64();
            let producer = r.size()?;
            let sequence = r.0.u64();
            let decision_tick = r.0.u32();
            let delivered_tick = r.0.u32();
            let count = r.count(limits.plan_decisions)?;
            let mut decisions = r.vector(count, 9)?;
            for _ in 0..count {
                decisions.push(r.decision()?);
            }
            let state = match r.0.u8() {
                0 => PlanState::Delivered,
                1 => {
                    let tick = r.0.u32();
                    let count = r.count(decisions.len().checked_mul(2)?)?;
                    let mut attempts = r.vector(count, 11)?;
                    for _ in 0..count {
                        attempts.push(PlanAttempt {
                            decision: r.decision()?,
                            outcome: outcome(r.0.u8())?,
                            fallback: r.boolean()?,
                        });
                    }
                    PlanState::Executed { tick, attempts }
                }
                2 => PlanState::Cancelled { tick: r.0.u32() },
                3 => PlanState::Reconfigured { tick: r.0.u32() },
                _ => return None,
            };
            plans.push(PlanReceipt {
                id,
                envelope: PlanEnvelope {
                    epoch,
                    producer,
                    sequence,
                    decision_tick,
                    decisions,
                },
                delivered_tick,
                state,
            });
        }
        let inbox = Inbox {
            epoch,
            limits: Limits {
                retained,
                producers,
                per_tick,
            },
            next,
            next_id,
            entries,
            plans,
        };
        if !r.0.ok() || r.0.left() != 0 || !inbox.valid() {
            return None;
        }
        let mut copy = Vec::new();
        copy.try_reserve_exact(world.len()).ok()?;
        copy.extend_from_slice(world);
        Some(Self { world: copy, inbox })
    }
}

fn outcome(tag: u8) -> Option<Outcome> {
    Some(match tag {
        0 => Outcome::Rejected,
        1 => Outcome::Unchanged,
        2 => Outcome::Applied,
        3 => Outcome::Queued,
        4 => Outcome::PathFailed,
        5 => Outcome::Busy,
        _ => return None,
    })
}

struct Reader<'a>(Scan<'a>);
impl Reader<'_> {
    fn size(&mut self) -> Option<usize> {
        usize::try_from(self.0.u64()).ok().filter(|_| self.0.ok())
    }
    fn count(&mut self, limit: usize) -> Option<usize> {
        self.size().filter(|&n| n <= limit)
    }
    fn vector<T>(&self, count: usize, minimum_bytes: usize) -> Option<Vec<T>> {
        if !self.0.ok() || count > self.0.left() / minimum_bytes {
            return None;
        }
        let mut v = Vec::new();
        v.try_reserve_exact(count).ok()?;
        Some(v)
    }
    fn boolean(&mut self) -> Option<bool> {
        match self.0.u8() {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn id(&mut self) -> Option<CommandId> {
        let id = CommandId {
            epoch: self.0.u64(),
            ordinal: self.0.u64(),
        };
        self.0.ok().then_some(id)
    }
    fn decision(&mut self) -> Option<Decision> {
        let actor = self.size()?;
        let order = match self.0.u8() {
            0 => AiOrder::Attack(self.size()?),
            1 => AiOrder::Move([self.0.i32(), self.0.i32()]),
            2 => AiOrder::Hold,
            3 => AiOrder::Auto,
            _ => return None,
        };
        self.0.ok().then_some(Decision { actor, order })
    }
    fn command(&mut self) -> Option<Command> {
        use super::super::Order;
        let tag = self.0.u8();
        let actor = self.size()?;
        let command = match tag {
            9 if actor == 0 => Command::Gun(match self.0.u8() {
                0 => super::super::GunCommand::Control,
                1 => super::super::GunCommand::Move {
                    destination: [self.0.i32(), self.0.i32()],
                },
                2 => super::super::GunCommand::Attack {
                    target: self.size()?,
                },
                3 => super::super::GunCommand::Stop,
                4 => super::super::GunCommand::AutoAttack {
                    enabled: self.boolean()?,
                },
                5 => super::super::GunCommand::GroundFire {
                    at: std::array::from_fn(|_| self.0.i32()),
                },
                6 => super::super::GunCommand::Refire,
                8 => super::super::GunCommand::Origin { at: std::array::from_fn(|_| self.0.i32()) },
                9 => super::super::GunCommand::Detonate { at: std::array::from_fn(|_| self.0.i32()) },
                10 => super::super::GunCommand::LabStream { origin: std::array::from_fn(|_| self.0.i32()), target: std::array::from_fn(|_| self.0.i32()), enabled: self.boolean()? },
                7 => super::super::GunCommand::Aim {
                    at: if self.boolean()? {
                        Some(std::array::from_fn(|_| self.0.i32()))
                    } else {
                        None
                    },
                },
                _ => return None,
            }),
            10 if actor == 0 => Command::SetPatrols {
                enabled: self.boolean()?,
            },
            8 if actor == 0 => Command::ConfigureEnemyAi {
                tuning: modding::battle_ai::Tuning {
                    decision_ticks: self.0.u32(),
                    focus_fire: self.boolean()?,
                    armed_bonus: self.0.i32(),
                    mobile_bonus: self.0.i32(),
                    switch_margin: self.0.i32(),
                },
            },
            6 if actor == 0 => Command::BattleLifecycle(match self.0.u8() {
                0 => super::BattleLifecycle::StartBattle,
                1 => super::BattleLifecycle::FreeControl,
                2 => super::BattleLifecycle::Autonomy {
                    enabled: self.boolean()?,
                },
                _ => return None,
            }),
            7 => Command::SetBattleWeapon {
                actor,
                weapon: match self.0.u8() {
                    0 => None,
                    code @ 1..=8 => Some(crate::blast::battle::Weapon::from_code(code - 1)?),
                    _ => return None,
                },
            },
            0 => {
                let order = match self.0.u8() {
                    0 => Order::Auto,
                    1 => Order::Hold,
                    2 => Order::Move,
                    3 => Order::Attack(self.size()?),
                    4 => Order::Ground([self.0.i32(), self.0.i32(), self.0.i32()]),
                    5 => Order::Dig([self.0.i32(), self.0.i32(), self.0.i32()]),
                    7 => Order::Reclaim([self.0.i32(), self.0.i32(), self.0.i32()]),
                    8 => Order::Support(self.size()?),
                    6 => {
                        let throttle = self.0.u8();
                        let turn = self.0.u8();
                        Order::Pilot {
                            throttle: throttle.wrapping_sub(1) as i8,
                            turn: turn.wrapping_sub(1) as i8,
                            aim: [self.0.i32(), self.0.i32(), self.0.i32()],
                            fire: self.boolean()?,
                        }
                    }
                    _ => return None,
                };
                let destination = self.boolean()?.then(|| [self.0.i32(), self.0.i32()]);
                Command::Order {
                    actor,
                    order,
                    destination,
                }
            }
            1 => Command::FormationMove {
                actor,
                destination: [self.0.i32(), self.0.i32()],
                fallback: [self.0.i32(), self.0.i32()],
            },
            5 if actor == 0 => Command::ConfigureWeapon {
                config: crate::blast::weapon_configuration::load(&mut self.0)?,
            },
            4 => Command::TuneMaterial {
                material: u16::try_from(actor).ok()?,
                field: super::super::MaterialField::from_code(self.0.u8())?,
                value: self.0.u32(),
            },
            3 if actor == 0 => Command::TuneDebris {
                field: super::super::DebrisField::from_code(self.0.u8())?,
                value: self.0.u32(),
            },
            11 => Command::Produce {
                actor,
                kind: *crate::blast::RangeUnitKind::ALL.get(usize::from(self.0.u8()))?,
            },
            12 => Command::Cancel { actor },
            13 if actor==0 => Command::Sculpt{at:std::array::from_fn(|_|self.0.i32()),edge:self.0.u32(),material:self.0.u16(),erase:self.boolean()?},
            2 => Command::TuneMotion {
                kind: *crate::blast::RangeUnitKind::ALL.get(actor)?,
                include_gun: self.boolean()?,
                field: super::super::MotionField::from_code(self.0.u8())?,
                value: self.0.i32(),
            },
            _ => return None,
        };
        self.0.ok().then_some(command)
    }
}

impl Inbox {
    fn valid(&self) -> bool {
        if self.epoch == 0
            || self.limits.retained == 0
            || self.limits.producers == 0
            || self.limits.per_tick == 0
            || self.next.len() != self.limits.producers
            || self
                .entries
                .len()
                .checked_add(self.plans.len())
                .is_none_or(|n| n > self.limits.retained)
            || self
                .next
                .iter()
                .try_fold(0u64, |sum, &n| sum.checked_add(n))
                != Some(self.next_id)
        {
            return false;
        }
        let mut ids = std::collections::BTreeMap::new();
        let mut add = |id: CommandId, epoch, producer: usize, sequence| {
            id.epoch == self.epoch
                && epoch == self.epoch
                && id.ordinal < self.next_id
                && self.next.get(producer).is_some_and(|&next| sequence < next)
                && id.ordinal >= sequence
                && ids.insert(id.ordinal, (producer, sequence)).is_none()
        };
        for entry in &self.entries {
            if !add(
                entry.id,
                entry.envelope.epoch,
                entry.envelope.producer,
                entry.envelope.sequence,
            ) || entry.assigned_tick < entry.envelope.requested_tick
                || entry.assigned_tick == u32::MAX
            {
                return false;
            }
        }
        for plan in &self.plans {
            if !add(
                plan.id,
                plan.envelope.epoch,
                plan.envelope.producer,
                plan.envelope.sequence,
            ) || plan.delivered_tick > plan.envelope.decision_tick
            {
                return false;
            }
        }
        let mut previous = vec![None; self.limits.producers];
        for (_, (producer, sequence)) in ids {
            if previous[producer].is_some_and(|old| old >= sequence) {
                return false;
            }
            previous[producer] = Some(sequence);
        }
        if self.entries.windows(2).any(|pair| {
            (pair[0].assigned_tick, pair[0].id.ordinal)
                >= (pair[1].assigned_tick, pair[1].id.ordinal)
        }) {
            return false;
        }
        let mut tick = None;
        let mut count = 0;
        for entry in &self.entries {
            if tick != Some(entry.assigned_tick) {
                tick = Some(entry.assigned_tick);
                count = 0;
            }
            count += 1;
            if count > self.limits.per_tick {
                return false;
            }
        }
        if self.plans.windows(2).any(|pair| {
            pair[0].id.ordinal >= pair[1].id.ordinal
                || pair[0].delivered_tick > pair[1].delivered_tick
                || match pair[0].state {
                    PlanState::Delivered => true,
                    PlanState::Executed { tick, .. } | PlanState::Cancelled { tick } => {
                        pair[1].delivered_tick <= tick
                            || pair[0].envelope.decision_tick >= pair[1].envelope.decision_tick
                    }
                    // A restart can bring the next deadline forward, or leave
                    // it unchanged. Delivery still follows the cancelling step.
                    PlanState::Reconfigured { tick } => pair[1].delivered_tick <= tick,
                }
        }) {
            return false;
        }
        self.plans
            .iter()
            .filter(|p| p.state == PlanState::Delivered)
            .count()
            <= 1
            && self
                .plans
                .iter()
                .rev()
                .skip(1)
                .all(|p| p.state != PlanState::Delivered)
    }

    pub(super) fn valid_for(&self, world: &BlastRange) -> bool {
        if !self.valid() || !world.battle_mode() && !self.plans.is_empty() {
            return false;
        }
        for entry in &self.entries {
            if entry.outcome.is_some() != (entry.assigned_tick < world.tick) {
                return false;
            }
        }
        let valid_decision = |d: &Decision| {
            d.actor < world.units.len()
                && !world.allied(d.actor)
                && match d.order {
                    AiOrder::Attack(target) => target < world.units.len() && world.allied(target),
                    AiOrder::Move(p) => {
                        p[0] >= 0
                            && p[1] >= 0
                            && p[0] < world.size[0] as i32
                            && p[1] < world.size[2] as i32
                    }
                    AiOrder::Hold | AiOrder::Auto => true,
                }
        };
        for plan in &self.plans {
            let input = &plan.envelope;
            if plan.delivered_tick > world.tick
                || input.decisions.len() > world.units.len()
                || !input.decisions.iter().all(&valid_decision)
                || input
                    .decisions
                    .windows(2)
                    .any(|pair| pair[0].actor >= pair[1].actor)
            {
                return false;
            }
            match &plan.state {
                PlanState::Delivered => {
                    if world.battle.phase != Phase::Fighting
                        || input.decision_tick != world.battle.ai.next_tick
                        || input.decision_tick < world.tick
                        || world.battle.ai.pending.as_ref() != Some(&input.decisions)
                    {
                        return false;
                    }
                }
                PlanState::Cancelled { tick } => {
                    if world.battle.phase == Phase::Fighting
                        || *tick < plan.delivered_tick
                        || *tick >= input.decision_tick
                        || *tick >= world.tick
                    {
                        return false;
                    }
                }
                PlanState::Reconfigured { tick } => {
                    if *tick < plan.delivered_tick
                        || *tick > input.decision_tick
                        || *tick >= world.tick
                    {
                        return false;
                    }
                }
                PlanState::Executed { tick, attempts } => {
                    if *tick != input.decision_tick
                        || *tick >= world.tick
                        || attempts.len() > input.decisions.len().saturating_mul(2)
                        || attempts.iter().any(|a| a.outcome == Outcome::Busy)
                    {
                        return false;
                    }
                    let mut at = 0;
                    let mut previous = None;
                    while at < attempts.len() {
                        let selected = &attempts[at];
                        if selected.fallback
                            || !input.decisions.contains(&selected.decision)
                            || previous.is_some_and(|actor| actor >= selected.decision.actor)
                        {
                            return false;
                        }
                        previous = Some(selected.decision.actor);
                        at += 1;
                        if !selected.outcome.accepted() {
                            let Some(fallback) = attempts.get(at) else {
                                return false;
                            };
                            // A recorded rules Move can itself fail map/path
                            // validation. Do not invent a new admission gate
                            // for the fallback that actually ran.
                            let valid = matches!(fallback.decision.order, AiOrder::Move(_))
                                || valid_decision(&fallback.decision);
                            if !fallback.fallback
                                || fallback.decision.actor != selected.decision.actor
                                || !valid
                            {
                                return false;
                            }
                            at += 1;
                        }
                    }
                }
            }
        }
        true
    }
}

pub(super) fn save_inbox(b: &mut Blob, inbox: &Inbox) {
    b.u64(inbox.epoch);
    b.usize(inbox.limits.retained);
    b.usize(inbox.limits.producers);
    b.usize(inbox.limits.per_tick);
    b.u64(inbox.next_id);
    for &next in &inbox.next {
        b.u64(next);
    }
    b.usize(inbox.entries.len());
    for entry in &inbox.entries {
        b.u64(entry.id.epoch);
        b.u64(entry.id.ordinal);
        b.u64(entry.envelope.epoch);
        b.usize(entry.envelope.producer);
        b.u64(entry.envelope.sequence);
        b.u32(entry.envelope.requested_tick);
        b.u32(entry.assigned_tick);
        b.u8(match entry.outcome {
            None => 0,
            Some(Outcome::Rejected) => 1,
            Some(Outcome::Unchanged) => 2,
            Some(Outcome::Applied) => 3,
            Some(Outcome::Queued) => 4,
            Some(Outcome::PathFailed) => 5,
            Some(Outcome::Busy) => 6,
        });
        save_recorded_command(b, entry.envelope.command);
    }
    b.usize(inbox.plans.len());
    for receipt in &inbox.plans {
        b.u64(receipt.id.epoch);
        b.u64(receipt.id.ordinal);
        b.u64(receipt.envelope.epoch);
        b.usize(receipt.envelope.producer);
        b.u64(receipt.envelope.sequence);
        b.u32(receipt.envelope.decision_tick);
        b.u32(receipt.delivered_tick);
        b.usize(receipt.envelope.decisions.len());
        for &decision in &receipt.envelope.decisions {
            save_decision(b, decision);
        }
        match &receipt.state {
            PlanState::Delivered => b.u8(0),
            PlanState::Executed { tick, attempts } => {
                b.u8(1);
                b.u32(*tick);
                b.usize(attempts.len());
                for attempt in attempts {
                    save_decision(b, attempt.decision);
                    b.u8(match attempt.outcome {
                        Outcome::Rejected => 0,
                        Outcome::Unchanged => 1,
                        Outcome::Applied => 2,
                        Outcome::Queued => 3,
                        Outcome::PathFailed => 4,
                        Outcome::Busy => 5,
                    });
                    b.bool(attempt.fallback);
                }
            }
            PlanState::Cancelled { tick } => {
                b.u8(2);
                b.u32(*tick);
            }
            PlanState::Reconfigured { tick } => {
                b.u8(3);
                b.u32(*tick);
            }
        }
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::blast::{battle::commands::MotionField, RangeUnitKind};

    #[test]
    fn gun_command_wire_checks_reserved_target_tags_booleans_and_every_truncation() {
        use super::super::super::GunCommand as G;
        let mut pairs = vec![
            (Command::Gun(G::Control), vec![9, 0]),
            (Command::Gun(G::Stop), vec![9, 3]),
            (Command::Gun(G::Refire), vec![9, 6]),
            (Command::Gun(G::AutoAttack { enabled: true }), vec![9, 4, 1]),
            (Command::Gun(G::Aim { at: None }), vec![9, 7, 0]),
            (Command::SetPatrols { enabled: false }, vec![10, 0]),
        ];
        let mut movement = vec![9, 1];
        let mut origin = vec![9, 8];
        for x in [192i32,48,336] {origin.extend(x.to_le_bytes());}
        pairs.push((Command::Gun(G::Origin{at:[192,48,336]}),origin));
        for x in [-1i32, 365] {
            movement.extend(x.to_le_bytes());
        }
        pairs.push((
            Command::Gun(G::Move {
                destination: [-1, 365],
            }),
            movement,
        ));
        let mut attack = vec![9, 2];
        attack.extend(17u64.to_le_bytes());
        pairs.push((Command::Gun(G::Attack { target: 17 }), attack));
        for input in [
            G::GroundFire {
                at: [i32::MAX, -384, 0],
            },
            G::Aim {
                at: Some([i32::MAX, -384, 0]),
            },
        ] {
            let mut payload = if matches!(input, G::GroundFire { .. }) {
                vec![9, 5]
            } else {
                vec![9, 7, 1]
            };
            for x in [i32::MAX, -384, 0] {
                payload.extend(x.to_le_bytes());
            }
            pairs.push((Command::Gun(input), payload));
        }
        for (input,tag) in [(G::Origin {at:[32,48,64]},8),(G::Detonate {at:[32,48,64]},9)] {
            let mut payload=vec![9,tag];
            for x in [32i32,48,64] {payload.extend(x.to_le_bytes());}
            pairs.push((Command::Gun(input),payload));
        }
        for enabled in [true,false] {
            let mut payload=vec![9,10];
            for x in [32i32,48,64,60,30,60] {payload.extend(x.to_le_bytes());}
            payload.push(u8::from(enabled));
            pairs.push((Command::Gun(G::LabStream{origin:[32,48,64],target:[60,30,60],enabled}),payload));
        }
        for (command, payload) in pairs {
            let mut bytes = vec![payload[0]];
            bytes.extend(0u64.to_le_bytes());
            bytes.extend_from_slice(&payload[1..]);
            assert_eq!(Reader(Scan::new(&bytes)).command(), Some(command));
            for end in 0..bytes.len() {
                assert!(Reader(Scan::new(&bytes[..end])).command().is_none());
            }
            for (at, value) in [(0, 255), (1, 1), (9, 255)] {
                let mut bad = bytes.clone();
                bad[at] = value;
                assert!(Reader(Scan::new(&bad)).command().is_none());
            }
            if matches!(command, Command::Gun(G::AutoAttack { .. } | G::Aim { .. })) {
                bytes[10] = 2;
                assert!(Reader(Scan::new(&bytes)).command().is_none());
            }
            if matches!(command,Command::Gun(G::LabStream{..})) {
                *bytes.last_mut().unwrap()=2;
                assert!(Reader(Scan::new(&bytes)).command().is_none());
            }
        }
    }

    #[test]
    fn ai_configuration_wire_preserves_values_and_rejects_structure_errors() {
        let tuning = modding::battle_ai::Tuning {
            decision_ticks: 0,
            focus_fire: false,
            armed_bonus: -1,
            mobile_bonus: 23,
            switch_margin: 31,
        };
        let mut b = Blob::new();
        b.u8(8);
        b.usize(0);
        b.u32(tuning.decision_ticks);
        b.bool(tuning.focus_fire);
        b.i32(tuning.armed_bonus);
        b.i32(tuning.mobile_bonus);
        b.i32(tuning.switch_margin);
        assert_eq!(b.len(), 26);
        // Invalid gameplay values are retained for Rejected at execution.
        assert_eq!(
            Reader(Scan::new(&b.v)).command(),
            Some(Command::ConfigureEnemyAi { tuning })
        );
        for (at, value) in [(0, 255), (1, 1), (13, 2)] {
            let mut bad = b.v.clone();
            bad[at] = value;
            assert!(Reader(Scan::new(&bad)).command().is_none());
        }
        for end in 0..b.len() {
            assert!(Reader(Scan::new(&b.v[..end])).command().is_none());
        }
    }

    #[test]
    fn material_command_wire_rejects_wide_ids_unknown_fields_and_truncation() {
        use crate::blast::battle::commands::MaterialField;
        let mut b = Blob::new();
        b.u8(4);
        b.usize(3);
        b.u8(MaterialField::Melt as u8);
        b.u32(17);
        assert_eq!(
            Reader(Scan::new(&b.v)).command(),
            Some(Command::TuneMaterial {
                material: 3,
                field: MaterialField::Melt,
                value: 17
            })
        );
        for (at, value) in [(3, 1), (9, 3)] {
            let mut bad = b.v.clone();
            bad[at] = value;
            assert!(Reader(Scan::new(&bad)).command().is_none());
        }
        for end in 0..b.len() {
            assert!(Reader(Scan::new(&b.v[..end])).command().is_none());
        }
    }

    #[test]
    fn debris_command_wire_rejects_reserved_target_unknown_field_and_truncation() {
        use crate::blast::battle::commands::DebrisField;
        let mut b = Blob::new();
        b.u8(3);
        b.usize(0);
        b.u8(DebrisField::DragPct as u8);
        b.u32(17);
        assert_eq!(
            Reader(Scan::new(&b.v)).command(),
            Some(Command::TuneDebris {
                field: DebrisField::DragPct,
                value: 17
            })
        );
        for (at, value) in [(1, 1), (9, 14)] {
            let mut bad = b.v.clone();
            bad[at] = value;
            assert!(Reader(Scan::new(&bad)).command().is_none());
        }
        for len in 0..b.len() {
            assert!(Reader(Scan::new(&b.v[..len])).command().is_none());
        }
    }
    #[test]
    fn motion_command_wire_rejects_unknown_kind_boolean_and_field() {
        let mut b = Blob::new();
        b.u8(2);
        b.usize(RangeUnitKind::Tank.index());
        b.bool(true);
        b.u8(MotionField::Speed as u8);
        b.i32(-7);
        assert_eq!(
            Reader(Scan::new(&b.v)).command(),
            Some(Command::TuneMotion {
                kind: RangeUnitKind::Tank,
                include_gun: true,
                field: MotionField::Speed,
                value: -7,
            })
        );
        for (at, value) in [(1, 255), (9, 2), (10, 18)] {
            let mut bad = b.v.clone();
            bad[at] = value;
            assert!(Reader(Scan::new(&bad)).command().is_none());
        }
        for len in 0..b.len() {
            assert!(Reader(Scan::new(&b.v[..len])).command().is_none());
        }
    }
}


pub(super) fn save_recorded_command(b: &mut Blob, command: Command) {
        match command {
            Command::Gun(command) => {
                use super::super::GunCommand;
                b.u8(9);
                b.usize(0);
                match command {
                    GunCommand::Control => b.u8(0),
                    GunCommand::Move { destination } => {
                        b.u8(1);
                        for x in destination {
                            b.i32(x);
                        }
                    }
                    GunCommand::Attack { target } => {
                        b.u8(2);
                        b.usize(target);
                    }
                    GunCommand::Stop => b.u8(3),
                    GunCommand::AutoAttack { enabled } => {
                        b.u8(4);
                        b.bool(enabled);
                    }
                    GunCommand::GroundFire { at } => {
                        b.u8(5);
                        for x in at {
                            b.i32(x);
                        }
                    }
                    GunCommand::Refire => b.u8(6),
                    GunCommand::Origin { at } => { b.u8(8); for x in at { b.i32(x); } }
                    GunCommand::Detonate { at } => { b.u8(9); for x in at { b.i32(x); } }
                    GunCommand::LabStream { origin, target, enabled } => { b.u8(10); for x in origin.into_iter().chain(target) { b.i32(x); } b.bool(enabled); }
                    GunCommand::Aim { at } => {
                        b.u8(7);
                        b.bool(at.is_some());
                        if let Some(at) = at {
                            for x in at {
                                b.i32(x);
                            }
                        }
                    }
                }
            }
            Command::SetPatrols { enabled } => {
                b.u8(10);
                b.usize(0);
                b.bool(enabled);
            }
            Command::ConfigureEnemyAi { tuning } => {
                b.u8(8);
                b.usize(0);
                b.u32(tuning.decision_ticks);
                b.bool(tuning.focus_fire);
                b.i32(tuning.armed_bonus);
                b.i32(tuning.mobile_bonus);
                b.i32(tuning.switch_margin);
            }
            Command::BattleLifecycle(command) => {
                b.u8(6);
                b.usize(0);
                match command {
                    super::BattleLifecycle::StartBattle => b.u8(0),
                    super::BattleLifecycle::FreeControl => b.u8(1),
                    super::BattleLifecycle::Autonomy { enabled } => {
                        b.u8(2);
                        b.bool(enabled);
                    }
                }
            }
            Command::SetBattleWeapon { actor, weapon } => {
                b.u8(7);
                b.usize(actor);
                b.u8(weapon.map_or(0, |weapon| weapon as u8 + 1));
            }
            Command::ConfigureWeapon { config } => {
                b.u8(5);
                b.usize(0);
                crate::blast::weapon_configuration::save(&config, b);
            }
            Command::TuneMaterial {
                material,
                field,
                value,
            } => {
                b.u8(4);
                b.usize(usize::from(material));
                b.u8(field as u8);
                b.u32(value);
            }
            Command::TuneDebris { field, value } => {
                b.u8(3);
                b.usize(0); // Reserved target: this command addresses the world.
                b.u8(field as u8);
                b.u32(value);
            }
            Command::TuneMotion {
                kind,
                include_gun,
                field,
                value,
            } => {
                b.u8(2);
                b.usize(kind.index());
                b.bool(include_gun);
                b.u8(field as u8);
                b.i32(value);
            }
            Command::Order {
                actor,
                order,
                destination,
            } => {
                b.u8(0);
                b.usize(actor);
                use super::super::Order;
                match order {
                    Order::Auto => b.u8(0),
                    Order::Hold => b.u8(1),
                    Order::Move => b.u8(2),
                    Order::Attack(target) => {
                        b.u8(3);
                        b.usize(target);
                    }
                    Order::Ground(at) => {
                        b.u8(4);
                        for x in at {
                            b.i32(x);
                        }
                    }
                    Order::Dig(at) => {
                        b.u8(5);
                        for x in at {
                            b.i32(x);
                        }
                    }
                    Order::Reclaim(at) => {
                        b.u8(7);
                        for x in at { b.i32(x); }
                    }
                    Order::Support(target) => { b.u8(8); b.usize(target); }
                    Order::Pilot {
                        throttle,
                        turn,
                        aim,
                        fire,
                    } => {
                        b.u8(6);
                        b.u8(throttle.wrapping_add(1) as u8);
                        b.u8(turn.wrapping_add(1) as u8);
                        for x in aim {
                            b.i32(x);
                        }
                        b.bool(fire);
                    }
                }
                b.bool(destination.is_some());
                if let Some(at) = destination {
                    for x in at {
                        b.i32(x);
                    }
                }
            }
            Command::FormationMove {
                actor,
                destination,
                fallback,
            } => {
                b.u8(1);
                b.usize(actor);
                for x in destination.into_iter().chain(fallback) {
                    b.i32(x);
                }
            }
            Command::Produce { actor, kind } => {
                b.u8(11);
                b.usize(actor);
                b.u8(kind.index() as u8);
            }
            Command::Sculpt{at,edge,material,erase}=> {
                b.u8(13);b.usize(0);for v in at {b.i32(v);}b.u32(edge);b.u16(material);b.bool(erase);
            }
            Command::Cancel { actor } => {
                b.u8(12);
                b.usize(actor);
            }
        }
}

pub(super) fn read_recorded_command(bytes: &[u8]) -> Option<(Command, usize)> {
    let mut reader = Reader(Scan::new(bytes));
    let command = reader.command()?;
    let used = bytes.len().checked_sub(reader.0.left())?;
    reader.0.ok().then_some((command, used))
}
