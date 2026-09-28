//! Enemy decisions are deterministic tick inputs. A model runner may inspect an
//! observation and submit a plan, but inference and wall clocks stay outside sim-core.
use super::*;
use fx::blob::{Blob, Scan};
use modding::battle_ai::Tuning;
use modding::range::RangeRoad;

pub const OBSERVATION_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitObservation {
    pub id: usize,
    pub allied: bool,
    pub alive: bool,
    pub armed: bool,
    pub mobile: bool,
    pub kind: RangeUnitKind,
    pub position: [i32; 2],
    pub ordered_target: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub version: u32,
    pub observed_tick: u32,
    pub decision_tick: u32,
    pub size: [u32; 3],
    pub units: Vec<UnitObservation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiOrder {
    Attack(usize),
    Move([i32; 2]),
    Hold,
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub actor: usize,
    pub order: AiOrder,
}

/// Pure decision boundary shared by rules and future external model adapters.
/// Returned decisions must still pass `submit_enemy_ai_plan` validation.
pub trait Policy {
    fn decide(&self, observation: &Observation) -> Vec<Decision>;
}

/// One short, authored-road waypoint toward a distant objective. Roads are
/// only a strategic corridor: Motion still checks the complete vehicle and
/// terrain before accepting any step. No simulation state or floating point
/// estimate is hidden in this choice.
pub(super) fn road_waypoint(
    roads: &[RangeRoad],
    from: [i32; 2],
    goal: [i32; 2],
) -> Option<[i32; 2]> {
    let mut nodes = Vec::<[i32; 2]>::new();
    let mut edges = Vec::<(usize, usize, i64)>::new();
    let mut segments = Vec::<(usize, usize, i32)>::new();
    let distance = |a: [i32; 2], b: [i32; 2]| -> i64 {
        i64::from((a[0] - b[0]).abs()) + i64::from((a[1] - b[1]).abs())
    };
    let squared = |a: [i32; 2], b: [i32; 2]| -> i64 {
        let dx = i64::from(a[0] - b[0]);
        let dz = i64::from(a[1] - b[1]);
        dx * dx + dz * dz
    };
    for road in roads {
        for pair in road.points.windows(2) {
            let mut ids = [0; 2];
            for (k, &point) in pair.iter().enumerate() {
                ids[k] = nodes.iter().position(|&p| p == point).unwrap_or_else(|| {
                    nodes.push(point);
                    nodes.len() - 1
                });
            }
            edges.push((ids[0], ids[1], distance(pair[0], pair[1])));
            segments.push((ids[0], ids[1], road.width));
        }
    }
    let end = (0..nodes.len()).min_by_key(|&i| (squared(nodes[i], goal), i))?;
    let mut costs = vec![i64::MAX; nodes.len()];
    let mut visited = vec![false; nodes.len()];
    costs[end] = 0;
    for _ in 0..nodes.len() {
        let Some(i) = (0..nodes.len())
            .filter(|&i| !visited[i])
            .min_by_key(|&i| (costs[i], i))
        else {
            break;
        };
        if costs[i] == i64::MAX {
            break;
        }
        visited[i] = true;
        for &(a, b, length) in &edges {
            let j = if a == i {
                b
            } else if b == i {
                a
            } else {
                continue;
            };
            costs[j] = costs[j].min(costs[i].saturating_add(length));
        }
    }
    let project = |a: [i32; 2], b: [i32; 2]| {
        let d = [i64::from(b[0] - a[0]), i64::from(b[1] - a[1])];
        let p = [i64::from(from[0] - a[0]), i64::from(from[1] - a[1])];
        let length = d[0] * d[0] + d[1] * d[1];
        if length == 0 {
            return a;
        }
        let dot = (p[0] * d[0] + p[1] * d[1]).clamp(0, length);
        [
            a[0] + (d[0] * dot / length) as i32,
            a[1] + (d[1] * dot / length) as i32,
        ]
    };
    let &(a, b, width) = segments.iter().min_by_key(|&&(a, b, _)| {
        let q = project(nodes[a], nodes[b]);
        (squared(q, from), a, b)
    })?;
    let q = project(nodes[a], nodes[b]);
    // Corners and large hulls can sit outside the narrow painted strip while
    // still following it. A smaller tolerance alternated between returning
    // to the centre line and advancing to the next junction forever.
    if squared(q, from) > i64::from(width.max(160)).pow(2) {
        return Some(q);
    }
    let end_cost = |i: usize| costs[i].saturating_add(distance(q, nodes[i]));
    let toward = if (end_cost(a), a) <= (end_cost(b), b) {
        a
    } else {
        b
    };
    // A hull need not drive through the exact centre of a junction. On a
    // switchback the authored point may sit against the steep inside edge;
    // accepting the outgoing road as soon as the hull is near the junction
    // lets the local planner pick a passable arc around it.
    let junction_reach = 160;
    let (start, next) = if squared(q, nodes[toward]) <= i64::from(junction_reach).pow(2) {
        let next = edges
            .iter()
            .filter_map(|&(a, b, length)| {
                let j = if a == toward {
                    b
                } else if b == toward {
                    a
                } else {
                    return None;
                };
                (costs[j] < costs[toward]).then_some((j, length))
            })
            .min_by_key(|&(j, length)| (costs[j].saturating_add(length), j))?
            .0;
        (nodes[toward], nodes[next])
    } else {
        (q, nodes[toward])
    };
    let d = [next[0] - start[0], next[1] - start[1]];
    let length = d[0].abs().max(d[1].abs()).max(1);
    // Long chords cut across the outside of bends and can ask the local
    // planner to climb terrain outside the cleared road. Keep the next
    // objective close enough to the authored strip.
    let step = length.min(240);
    Some([
        start[0] + d[0] * step / length,
        start[1] + d[1] * step / length,
    ])
}

pub struct Rules<'a> {
    pub tuning: &'a Tuning,
    /// Which team's orders this policy is producing. The observation keeps
    /// absolute allegiance so a model adapter can use the same input.
    pub controlled_allied: bool,
}

impl Rules<'_> {
    fn distance(a: &UnitObservation, b: &UnitObservation) -> i64 {
        (i64::from(a.position[0]) - i64::from(b.position[0])).abs()
            + (i64::from(a.position[1]) - i64::from(b.position[1])).abs()
    }

    fn score(
        &self,
        actor: &UnitObservation,
        target: &UnitObservation,
        allies: &[&UnitObservation],
    ) -> i64 {
        let support = if self.tuning.focus_fire {
            allies
                .iter()
                .filter(|u| {
                    u.id != actor.id
                        && u.ordered_target == Some(target.id)
                        && Self::distance(u, actor) <= 600
                })
                .count()
                .min(2) as i64
        } else {
            0
        };
        Self::distance(actor, target)
            - i64::from(self.tuning.armed_bonus) * i64::from(target.armed)
            - i64::from(self.tuning.mobile_bonus) * i64::from(target.mobile)
            - support * i64::from(self.tuning.switch_margin)
    }
}

impl Policy for Rules<'_> {
    fn decide(&self, observation: &Observation) -> Vec<Decision> {
        let allies: Vec<_> = observation
            .units
            .iter()
            .filter(|u| u.allied == self.controlled_allied && u.alive && (u.armed || u.mobile))
            .collect();
        let targets: Vec<_> = observation
            .units
            .iter()
            .filter(|u| u.allied != self.controlled_allied && u.alive)
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }
        allies
            .iter()
            .map(|u| Decision {
                actor: u.id,
                order: AiOrder::Attack({
                    // Mobile artillery is the siege arm. It advances on the
                    // command objective instead of chasing a distant scout,
                    // but engages armed defenders in its immediate sector.
                    let siege = (u.kind == RangeUnitKind::Artillery && u.armed)
                        .then(|| targets.iter().find(|v| v.kind == RangeUnitKind::Base))
                        .flatten();
                    if let Some(base) = siege {
                        let threat_radius = 1200
                            + if u.ordered_target == Some(base.id) {
                                0
                            } else {
                                self.tuning.switch_margin
                            };
                        if let Some(threat) = targets
                            .iter()
                            .filter(|v| {
                                v.kind != RangeUnitKind::Base
                                    && v.armed
                                    && Self::distance(u, v) <= i64::from(threat_radius)
                            })
                            .min_by_key(|v| (self.score(u, v, &allies), v.id))
                        {
                            threat.id
                        } else {
                            base.id
                        }
                    } else {
                        let best = targets
                            .iter()
                            .min_by_key(|v| (self.score(u, v, &allies), v.id))
                            .expect("nonempty targets");
                        targets
                            .iter()
                            .find(|v| Some(v.id) == u.ordered_target)
                            .filter(|old| {
                                self.score(u, old, &allies)
                                    <= self.score(u, best, &allies)
                                        + i64::from(self.tuning.switch_margin)
                            })
                            .unwrap_or(best)
                            .id
                    }
                }),
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub(super) struct Controller {
    pub tuning: Tuning,
    pub next_tick: u32,
    pub pending: Option<Vec<Decision>>,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            tuning: Tuning::from_ron(include_str!("../../../data/battle-ai.ron"))
                .expect("battle AI tuning"),
            next_tick: 0,
            pending: None,
        }
    }

    pub fn save(&self, b: &mut Blob) {
        b.u32(self.tuning.decision_ticks);
        b.bool(self.tuning.focus_fire);
        b.i32(self.tuning.armed_bonus);
        b.i32(self.tuning.mobile_bonus);
        b.i32(self.tuning.switch_margin);
        b.u32(self.next_tick);
        b.bool(self.pending.is_some());
        if let Some(plan) = &self.pending {
            b.usize(plan.len());
            for decision in plan {
                b.usize(decision.actor);
                match decision.order {
                    AiOrder::Attack(id) => {
                        b.u8(0);
                        b.usize(id);
                    }
                    AiOrder::Move(p) => {
                        b.u8(1);
                        b.i32(p[0]);
                        b.i32(p[1]);
                    }
                    AiOrder::Hold => b.u8(2),
                    AiOrder::Auto => b.u8(3),
                }
            }
        }
    }

    pub fn load(
        &mut self,
        s: &mut Scan,
        tick: u32,
        fighting: bool,
        size: [u32; 3],
        allied: &[bool],
    ) -> Option<()> {
        let tuning = Tuning {
            decision_ticks: s.u32(),
            focus_fire: s.bool(),
            armed_bonus: s.i32(),
            mobile_bonus: s.i32(),
            switch_margin: s.i32(),
        };
        tuning.validate().ok()?;
        let next_tick = s.u32();
        if fighting && (next_tick < tick || next_tick > tick.saturating_add(tuning.decision_ticks))
        {
            return None;
        }
        let pending = if s.bool() {
            if !fighting {
                return None;
            }
            let count = s.usize();
            if count > allied.len() {
                return None;
            }
            let mut result = Vec::with_capacity(count);
            for _ in 0..count {
                let actor = s.usize();
                if actor >= allied.len()
                    || allied[actor]
                    || result.last().is_some_and(|d: &Decision| d.actor >= actor)
                {
                    return None;
                }
                let order = match s.u8() {
                    0 => {
                        let target = s.usize();
                        if target >= allied.len() || !allied[target] {
                            return None;
                        }
                        AiOrder::Attack(target)
                    }
                    1 => {
                        let p = [s.i32(), s.i32()];
                        if p[0] < 0 || p[1] < 0 || p[0] >= size[0] as i32 || p[1] >= size[2] as i32
                        {
                            return None;
                        }
                        AiOrder::Move(p)
                    }
                    2 => AiOrder::Hold,
                    3 => AiOrder::Auto,
                    _ => return None,
                };
                result.push(Decision { actor, order });
            }
            Some(result)
        } else {
            None
        };
        self.tuning = tuning;
        self.next_tick = next_tick;
        self.pending = pending;
        s.ok().then_some(())
    }
}
