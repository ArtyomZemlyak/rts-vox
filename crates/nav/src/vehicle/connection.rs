//! A swept turn and straight run, resumable at each exact pose sample.
use super::*;
use fx::blob::{Blob, Scan};

#[derive(Clone)]
pub(super) struct Connection {
    from: [Fx; 2],
    to: [Fx; 2],
    head: i32,
    yaw: i32,
    cursor: i32,
    height: Fx,
    crush: usize,
}
impl Connection {
    pub fn new(from: [Fx; 2], head: i32, to: [Fx; 2]) -> Self {
        let delta = [to[0].sub(from[0]), to[1].sub(from[1])];
        let yaw = if delta == [Fx::ZERO; 2] {
            head
        } else {
            angle::atan2(delta[1].raw(), delta[0].raw())
        };
        Self::sweep(from, head, to, yaw)
    }
    pub fn sweep(from: [Fx; 2], head: i32, to: [Fx; 2], yaw: i32) -> Self {
        Self {
            from,
            to,
            head,
            yaw,
            cursor: 0,
            height: Fx::ZERO,
            crush: 0,
        }
    }
    /// The columns the samples checked so far may have read, as a box;
    /// `None` before the first sample (ADR-427). A change outside it
    /// leaves what the run has checked as it was.
    pub fn checked(&self, shape: Shape) -> Option<([i32; 2], [i32; 2])> {
        if self.cursor == 0 {
            return None;
        }
        let delta = [self.to[0].sub(self.from[0]), self.to[1].sub(self.from[1])];
        let turns = turn_samples(angle::diff(self.yaw, self.head), shape);
        let steps = (delta[0].abs().max(delta[1].abs()).to_int_trunc() + 1).max(1);
        let last = self.cursor - 1;
        let at = if last <= turns {
            self.from
        } else {
            let f = Fx::from_ratio(last - turns, steps);
            [
                self.from[0].add(delta[0].mul(f)),
                self.from[1].add(delta[1].mul(f)),
            ]
        };
        let r = reach(shape);
        Some((
            [0, 1].map(|a| self.from[a].min(at[a]).to_int_trunc() - r),
            [0, 1].map(|a| self.from[a].max(at[a]).to_int_trunc() + 1 + r),
        ))
    }
    pub fn restart(&mut self) {
        self.cursor = 0;
        self.height = Fx::ZERO;
        self.crush = 0;
    }
    pub(super) fn samples(&self, shape: Shape, budget: usize) -> Vec<([Fx; 2], i32)> {
        // Let the owning search establish that the connection has a useful
        // legal prefix before speculating across later samples.
        if self.cursor < 16 {
            return Vec::new();
        }
        self.sample_range(shape, budget)
    }
    pub(super) fn start_batch(&self, shape: Shape, budget: usize) -> Option<StartSweep> {
        if budget < 16 {
            return None;
        }
        let samples = self.sample_range(shape, budget);
        (samples.len() >= 16).then_some(StartSweep {
            shape,
            samples,
            height: self.height,
            initial: self.cursor == 0,
        })
    }
    fn sample_range(&self, shape: Shape, budget: usize) -> Vec<([Fx; 2], i32)> {
        let delta = [self.to[0].sub(self.from[0]), self.to[1].sub(self.from[1])];
        let turns = turn_samples(angle::diff(self.yaw, self.head), shape);
        let steps = (delta[0].abs().max(delta[1].abs()).to_int_trunc() + 1).max(1);
        let last = (turns + steps).min(self.cursor.saturating_add(budget.min(32) as i32 - 1));
        (self.cursor..=last)
            .map(|cursor| self.sample(cursor, turns, steps))
            .collect()
    }
    fn sample(&self, cursor: i32, turns: i32, steps: i32) -> ([Fx; 2], i32) {
        if cursor == 0 {
            (self.from, self.head)
        } else if cursor <= turns {
            (
                self.from,
                angle::norm(self.head + angle::diff(self.yaw, self.head) * cursor / turns),
            )
        } else {
            let delta = [self.to[0].sub(self.from[0]), self.to[1].sub(self.from[1])];
            let f = Fx::from_ratio(cursor - turns, steps);
            (
                [
                    self.from[0].add(delta[0].mul(f)),
                    self.from[1].add(delta[1].mul(f)),
                ],
                self.yaw,
            )
        }
    }
    pub(super) fn prefix_open(&self, shape: Shape, poses: &[Pose]) -> bool {
        let mut height = self.height;
        for pose in poses {
            if pose.refused() || pose.height.sub(height).abs() > Fx::from_int(shape.step) {
                return false;
            }
            height = pose.height;
        }
        true
    }
    /// Outer None: allowance spent. Inner None: blocked. Some count: open.
    pub fn advance(
        &mut self,
        ground: &mut Probe<'_>,
        shape: Shape,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        allowance: &mut usize,
    ) -> Option<Option<usize>> {
        self.advance_with_endpoints(ground, shape, clear, allowance, None, None)
    }
    /// The search has already probed both nodes of an edge. Reuse their exact
    /// poses while charging the same logical queries for bounded progress.
    pub fn advance_with_endpoints(
        &mut self,
        ground: &mut Probe<'_>,
        shape: Shape,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        allowance: &mut usize,
        origin_pose: Option<Pose>,
        target_pose: Option<Pose>,
    ) -> Option<Option<usize>> {
        let delta = [self.to[0].sub(self.from[0]), self.to[1].sub(self.from[1])];
        let yaw = self.yaw;
        let turn = angle::diff(yaw, self.head);
        // Bound the furthest footprint corner's arc between probes by one
        // world voxel. Seven is a conservative integer upper bound for 2*pi.
        let turns = turn_samples(turn, shape);
        let steps = (delta[0].abs().max(delta[1].abs()).to_int_trunc() + 1).max(1);
        while *allowance > 0 {
            *allowance -= 1;
            if self.cursor == 0 {
                self.height = origin_pose.map_or_else(
                    || ground.route_pose(self.from, self.head, shape).height,
                    |pose| pose.height,
                );
            } else {
                let (p, h) = self.sample(self.cursor, turns, steps);
                if !clear(p, h) {
                    return Some(None);
                }
                // A zero-angle turn's first sample is exactly the source
                // pose again. Reuse its full clearance and crush answer.
                let next = if self.cursor == turns + steps {
                    target_pose.unwrap_or_else(|| ground.route_pose(p, h, shape))
                } else if turn == 0 && self.cursor == 1 {
                    origin_pose.unwrap_or_else(|| ground.route_pose(p, h, shape))
                } else {
                    ground.route_pose(p, h, shape)
                };
                if next.refused() || next.height.sub(self.height).abs() > Fx::from_int(shape.step) {
                    return Some(None);
                }
                self.crush = if self.cursor <= turns {
                    self.crush.max(next.crush)
                } else {
                    self.crush + next.crush
                };
                self.height = next.height;
            }
            self.cursor += 1;
            if self.cursor > turns + steps {
                return Some(Some(self.crush));
            }
        }
        None
    }
    pub fn save(&self, b: &mut Blob) {
        for p in [self.from, self.to] {
            for v in p {
                b.fx(v);
            }
        }
        b.i32(self.head);
        b.i32(self.yaw);
        b.i32(self.cursor);
        b.fx(self.height);
        b.usize(self.crush);
    }
    pub fn load(s: &mut Scan, swept: bool) -> Option<Self> {
        let mut result = Self {
            from: [s.fx(), s.fx()],
            to: [s.fx(), s.fx()],
            head: s.i32(),
            yaw: if swept { s.i32() } else { 0 },
            cursor: s.i32(),
            height: s.fx(),
            crush: s.usize(),
        };
        if !swept {
            // Old snapshots sampled turns more sparsely. Restart this finite
            // connection so an unchecked part cannot be accepted on upgrade.
            result = Self::new(result.from, result.head, result.to);
        }
        (s.ok()
            && (0..=200_000).contains(&result.cursor)
            && (0..angle::FULL).contains(&result.head)
            && (0..angle::FULL).contains(&result.yaw))
        .then_some(result)
    }
}

pub(super) fn turn_samples(turn: i32, shape: Shape) -> i32 {
    ((i64::from(turn.abs()) * i64::from(shape.half[0] + shape.half[1]) * 7
        + i64::from(angle::FULL)
        - 1)
        / i64::from(angle::FULL))
    .max(1) as i32
}
