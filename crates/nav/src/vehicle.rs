//! Oriented vehicle clearance and routes against the current voxel surface.
//! The same footprint query serves planning and accepted movement. This
//! surface adapter does not replace the multi-level navigation graph.
//!
//! Yielding materials (a tree over a tracked hull) are not obstacles: the
//! floor is the terrain beneath them, and the pose reports how many footprint
//! columns hold such matter inside the hull height, so the caller can crush
//! it and the planner can price the detour.
use fx::{angle, Fx};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap},
};
use voxel::{
    store::{Mat, AIR},
    World,
};

#[derive(Clone, Copy)]
pub struct Shape {
    pub half: [i32; 2],
    pub support: [i32; 2],
    pub clearance: Fx,
    pub step: i32,
    pub min_up: Fx,
    /// Hull height above its floor, in world units. Yielding matter below
    /// the hull top is crushed; matter above it is passed under.
    pub height: i32,
    /// Contact length of a support along the heading, both ways, in world
    /// units. A track bridges a gap shorter than this; zero is a point.
    pub span: i32,
    /// Approach slope: a rise seen `k` units along the span counts as
    /// `rise - ramp * k`, so the hull climbs a step over its run-up instead
    /// of snapping onto it, and settles off a ledge the same way.
    pub ramp: Fx,
}
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub height: Fx,
    pub floors: [Fx; 4],
    /// Footprint columns whose ground stands above the hull belly.
    pub blocked: usize,
    /// A support's contact sees a rise its belt cannot grip.
    pub steep: bool,
    /// The hull would lean past its tilt limit.
    pub tilt: bool,
    /// Footprint columns holding yielding matter inside the hull height.
    pub crush: usize,
}
impl Pose {
    /// Whether a hull may not stand here at all.
    pub fn refused(&self) -> bool {
        self.blocked != 0 || self.steep || self.tilt
    }
}
/// Columns where resting debris stands and the height of its top, in
/// world units (ADR-292): an overlay the probe adds to the world's floor,
/// so that contacts, feet and the planner all stand on the same heaps.
pub type Rubble = BTreeMap<[i32; 2], u16>;
/// Query-local height cache. No cached result survives a world edit.
///
/// The cache is a window of columns: the whole map for a route search, a
/// few hull lengths around the vehicle for the per-tick pose checks, so
/// that clearing it every tick costs kilobytes rather than megabytes.
/// The column window of a probe, taken out and put back so that a search
/// spanning ticks scans each column once rather than once a slice (P-29).
/// It holds answers about the world and is therefore only as good as the
/// world it was filled from: a search whose world changed drops it.
#[derive(Clone, Default)]
pub struct Columns {
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
    heights: Vec<u16>,
    yields: Vec<[u16; 2]>,
}
pub struct Probe<'a> {
    world: &'a World,
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
    heights: Vec<u16>,
    /// Lowest and highest yielding cell above the floor; `[MAX, 0]` is none.
    /// Empty when nothing yields.
    yields: Vec<[u16; 2]>,
    yielding: Vec<Mat>,
    rubble: Option<&'a Rubble>,
    /// Columns scanned, columns answered and surface samples taken since
    /// this probe was made: the price of a search, for benches (P-29).
    pub scans: u64,
    pub columns: u64,
    pub samples: u64,
}
impl<'a> Probe<'a> {
    pub fn new(world: &'a World) -> Self {
        Self::with_yield(world, &[])
    }
    /// `yielding` materials never count as floor or obstacle.
    pub fn with_yield(world: &'a World, yielding: &[Mat]) -> Self {
        Self::window(world, yielding, [0, 0], [world.nx as i32, world.nz as i32])
    }
    /// A cache covering `radius` columns around `centre`. Columns outside
    /// it are still answered, just recomputed on every call.
    pub fn around(world: &'a World, yielding: &[Mat], centre: [Fx; 2], radius: i32) -> Self {
        let c = centre.map(Fx::to_int_trunc);
        Self::window(
            world,
            yielding,
            [(c[0] - radius).max(0), (c[1] - radius).max(0)],
            [
                (c[0] + radius + 1).min(world.nx as i32),
                (c[1] + radius + 1).min(world.nz as i32),
            ],
        )
    }
    fn window(world: &'a World, yielding: &[Mat], lo: [i32; 2], hi: [i32; 2]) -> Self {
        let w = (hi[0] - lo[0]).max(0) as usize;
        let h = (hi[1] - lo[1]).max(0) as usize;
        Self {
            world,
            x0: lo[0],
            z0: lo[1],
            w,
            h,
            heights: vec![u16::MAX; w * h],
            yields: if yielding.is_empty() {
                Vec::new()
            } else {
                vec![[u16::MAX, 0]; w * h]
            },
            yielding: yielding.to_vec(),
            rubble: None,
            scans: 0,
            columns: 0,
            samples: 0,
        }
    }
    /// Resting debris counts as floor in its columns (ADR-292).
    pub fn with_rubble(mut self, rubble: &'a Rubble) -> Self {
        self.rubble = Some(rubble);
        self
    }
    /// Take back a window filled earlier, if it covers the same ground
    /// (P-29); a window of another shape is dropped rather than adapted.
    pub fn with_columns(mut self, c: Columns) -> Self {
        if c.x0 == self.x0
            && c.z0 == self.z0
            && c.w == self.w
            && c.h == self.h
            && c.heights.len() == self.heights.len()
            && c.yields.len() == self.yields.len()
        {
            self.heights = c.heights;
            self.yields = c.yields;
        }
        self
    }
    /// Give up the filled window; the probe keeps an empty one of the same
    /// shape and answers as a fresh probe would.
    pub fn take_columns(&mut self) -> Columns {
        let blank = if self.yields.is_empty() {
            Vec::new()
        } else {
            vec![[u16::MAX, 0]; self.w * self.h]
        };
        Columns {
            x0: self.x0,
            z0: self.z0,
            w: self.w,
            h: self.h,
            heights: std::mem::replace(&mut self.heights, vec![u16::MAX; self.w * self.h]),
            yields: std::mem::replace(&mut self.yields, blank),
        }
    }
    pub fn yields(&self, m: Mat) -> bool {
        self.yielding.contains(&m)
    }
    fn scan(&self, x: i32, z: i32) -> (u16, [u16; 2]) {
        let mut floor = 0;
        let mut span = [u16::MAX, 0];
        // The walk starts at the column's own ceiling, not at the sky
        // (ADR-313): above it there is nothing, and on the range that was
        // a hundred and seventy-four empty cells every time.
        for y in (0..self.world.column_ceiling(x as u32, z as u32)).rev() {
            if !self.world.solid(x as u32, y, z as u32) {
                continue;
            }
            if !self.yielding.is_empty() && self.yields(self.world.material(x as u32, y, z as u32))
            {
                span = [span[0].min(y as u16), span[1].max(y as u16)];
                continue;
            }
            floor = y as u16 + 1;
            break;
        }
        // A resting heap of debris is floor in its columns (ADR-292).
        if let Some(top) = self.rubble.and_then(|r| r.get(&[x, z])) {
            floor = floor.max(*top);
        }
        (floor, span)
    }
    /// Floor and yielding span of a column; `None` outside the world.
    fn column(&mut self, x: i32, z: i32) -> Option<(u16, [u16; 2])> {
        self.columns += 1;
        if x < 0 || z < 0 || x >= self.world.nx as i32 || z >= self.world.nz as i32 {
            return None;
        }
        let (dx, dz) = (x - self.x0, z - self.z0);
        if dx < 0 || dz < 0 || dx as usize >= self.w || dz as usize >= self.h {
            return Some(self.scan(x, z));
        }
        let i = dx as usize + self.w * dz as usize;
        if self.heights[i] == u16::MAX {
            self.scans += 1;
            let (floor, span) = self.scan(x, z);
            self.heights[i] = floor;
            if !self.yields.is_empty() {
                self.yields[i] = span;
            }
        }
        let span = if self.yields.is_empty() {
            [u16::MAX, 0]
        } else {
            self.yields[i]
        };
        Some((self.heights[i], span))
    }
    pub fn floor(&mut self, x: i32, z: i32) -> i32 {
        match self.column(x, z) {
            Some((floor, _)) => floor as i32,
            None => self.world.ny as i32,
        }
    }
    /// Lowest yielding cell of the column, if any lies above its floor.
    fn yield_low(&mut self, x: i32, z: i32) -> Option<i32> {
        let (_, span) = self.column(x, z)?;
        (span[0] != u16::MAX).then_some(span[0] as i32)
    }
    /// Every yielding cell inside the hull at this pose, in column order.
    /// This is the cut a tracked hull makes when it stands here.
    pub fn crushed(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> Vec<[u32; 3]> {
        let mut out = Vec::new();
        if self.yielding.is_empty() {
            return out;
        }
        let (c, sin) = (angle::cos(head), angle::sin(head));
        let (lo, hi) = self.extent(pos, head, s);
        for z in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                if !self.inside(pos, c, sin, s, x, z) {
                    continue;
                }
                let Some((floor, [low, high])) = self.column(x, z) else {
                    continue;
                };
                let top = (floor as i32 + s.height).min(self.world.ny as i32);
                if low == u16::MAX {
                    continue;
                }
                for y in low as i32..=(high as i32).min(top - 1) {
                    let m = self.world.material(x as u32, y as u32, z as u32);
                    if m != AIR && self.yields(m) {
                        out.push([x as u32, y as u32, z as u32]);
                    }
                }
            }
        }
        out
    }
    fn extent(&self, pos: [Fx; 2], head: i32, s: Shape) -> ([i32; 2], [i32; 2]) {
        let (c, sin) = (angle::cos(head), angle::sin(head));
        let ext = [
            c.abs()
                .mul(Fx::from_int(s.half[0]))
                .add(sin.abs().mul(Fx::from_int(s.half[1]))),
            sin.abs()
                .mul(Fx::from_int(s.half[0]))
                .add(c.abs().mul(Fx::from_int(s.half[1]))),
        ];
        (
            [
                pos[0].sub(ext[0]).to_int_trunc() - 1,
                pos[1].sub(ext[1]).to_int_trunc() - 1,
            ],
            [
                pos[0].add(ext[0]).to_int_trunc() + 1,
                pos[1].add(ext[1]).to_int_trunc() + 1,
            ],
        )
    }
    // Include a half-cell halo so a one-voxel rock cannot hide between
    // the four nominal supports or between diagonally sampled cells.
    fn inside(&self, pos: [Fx; 2], c: Fx, sin: Fx, s: Shape, x: i32, z: i32) -> bool {
        let dx = Fx::from_int(x).add(Fx::from_ratio(1, 2)).sub(pos[0]);
        let dz = Fx::from_int(z).add(Fx::from_ratio(1, 2)).sub(pos[1]);
        let u = dx.mul(c).add(dz.mul(sin));
        let v = dz.mul(c).sub(dx.mul(sin));
        u.abs() <= Fx::from_int(s.half[0]).add(Fx::from_ratio(1, 2))
            && v.abs() <= Fx::from_int(s.half[1]).add(Fx::from_ratio(1, 2))
    }
    fn surface(&mut self, p: [Fx; 2]) -> Fx {
        self.samples += 1;
        // A support has a finite contact patch. Blend the four neighbouring
        // columns instead of snapping its load at each integer boundary.
        // Hull obstacle checks below still use the exact voxel surface.
        let x = p[0].to_int_trunc();
        let z = p[1].to_int_trunc();
        let u = p[0].sub(Fx::from_int(x));
        let v = p[1].sub(Fx::from_int(z));
        let h00 = Fx::from_int(self.floor(x, z));
        let h10 = Fx::from_int(self.floor(x + 1, z));
        let h01 = Fx::from_int(self.floor(x, z + 1));
        let h11 = Fx::from_int(self.floor(x + 1, z + 1));
        let a = h00.add(h10.sub(h00).mul(u));
        let b = h01.add(h11.sub(h01).mul(u));
        a.add(b.sub(a).mul(v))
    }
    /// Support height at `p`, and whether the ground along its contact
    /// span rises steeper than the hull may climb.
    ///
    /// Three things a belt does that a point does not: it lies across a
    /// gap on the ground either side of it, it rides up a rise over the
    /// run-up rather than at the instant of contact, and it cannot grip a
    /// grade steeper than the tilt limit however level the hull sits.
    fn contact(&mut self, p: [Fx; 2], radius: Fx, dir: [Fx; 2], s: Shape, grade: Fx) -> (Fx, bool) {
        let base = self.surface(p);
        let mut height = base;
        let (nx, nz) = (
            Fx::from_int(self.world.nx as i32 - 1),
            Fx::from_int(self.world.nz as i32 - 1),
        );
        let mut samples = Vec::with_capacity(2 * s.span.max(0) as usize);
        let mut level = base;
        // A point support sees no grade: a belt shorter than two units has
        // no line to sample.
        for k in 1..=(if s.span >= 2 { s.span } else { 0 }) {
            let d = Fx::from_int(k);
            let mut pair = [None; 2];
            for (i, sign) in [-1, 1].into_iter().enumerate() {
                let q = [
                    p[0].add(dir[0].mul(d).mul(Fx::from_int(sign))),
                    p[1].add(dir[1].mul(d).mul(Fx::from_int(sign))),
                ];
                // The map edge is not a cliff; the hull check keeps it out.
                if q[0] < Fx::ZERO || q[1] < Fx::ZERO || q[0] >= nx || q[1] >= nz {
                    continue;
                }
                let there = self.surface(q);
                pair[i] = Some(there);
                samples.push((d, there));
            }
            // Bridging: the belt between neighbouring wheels lies on the
            // ground either side of a hollow narrower than half the contact.
            // A wider hollow is followed, the wheel drops into it.
            if let (Some(a), Some(b), true) = (pair[0], pair[1], 2 * k <= s.span) {
                level = level.max(a.min(b));
            }
        }
        height = height.max(level);
        let mut steep = false;
        let step = Fx::from_int(s.step);
        // The contact patch: nearby ground counts with the same approach
        // slope as ground ahead, or a corner sample would lift the support
        // onto a ledge at once and undo the run-up. A step under the edge
        // of the patch, higher than the step limit within half the patch,
        // is refused like one under the belt: a boulder under one track
        // edge is not ridden by the corner and refused by the belt a
        // degree later. A slope across the patch is left to the tilt limit.
        let half = radius.div(Fx::from_int(2));
        for (x, z) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let corner = self.surface([
                p[0].add(radius.mul(Fx::from_int(x))),
                p[1].add(radius.mul(Fx::from_int(z))),
            ]);
            height = height.max(corner.sub(s.ramp.mul(radius)));
            let mid = self.surface([
                p[0].add(half.mul(Fx::from_int(x))),
                p[1].add(half.mul(Fx::from_int(z))),
            ]);
            if s.span >= 2 && corner.sub(mid) > step {
                steep = true;
            }
        }
        for (d, there) in samples {
            height = height.max(there.sub(s.ramp.mul(d)));
            // One obstacle up to the step height is climbed; a longer rise
            // must stay within the grade the belt can hold.
            if there.sub(level) > step.max(grade.mul(d)) {
                steep = true;
            }
        }
        (height, steep)
    }
    pub fn pose(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> Pose {
        let (c, sin) = (angle::cos(head), angle::sin(head));
        let point = |x: i32, z: i32| {
            [
                pos[0]
                    .add(c.mul(Fx::from_int(x)))
                    .sub(sin.mul(Fx::from_int(z))),
                pos[1]
                    .add(sin.mul(Fx::from_int(x)))
                    .add(c.mul(Fx::from_int(z))),
            ]
        };
        let radius = Fx::from_int(
            (s.half[0] - s.support[0])
                .min(s.half[1] - s.support[1])
                .min(s.step)
                .max(1),
        );
        // tan of the tilt limit: the steepest grade the contact may climb.
        let m = s.min_up.max(Fx::from_ratio(1, 100));
        let grade = Fx::ONE.sub(m.mul(m)).max(Fx::ZERO).sqrt().div(m);
        let mut steep = false;
        let mut floors = [(-1, -1), (1, -1), (-1, 1), (1, 1)].map(|(x, z)| {
            let p = point(x * s.support[0], z * s.support[1]);
            let (h, too_steep) = self.contact(p, radius, [c, sin], s, grade);
            steep |= too_steep;
            h
        });
        // A belt rests on a hump between its wheels: ground above the line
        // joining the two supports of a side lifts both of them, so the hull
        // rides over a summit instead of grounding its belly on it.
        if s.span >= 2 {
            // The lift fades in over this many units as a hump enters the
            // belt at one wheel and out as it leaves at the other, so the
            // hull rides onto and off a summit instead of jumping.
            let fade = (s.span / 2).max(1);
            for (rear, front, z) in [(0, 1, -1), (2, 3, 1)] {
                let mut lift = Fx::ZERO;
                let mut top = Fx::ZERO;
                let mut u = -s.support[0] + 1;
                while u < s.support[0] {
                    let g = self.surface(point(u, z * s.support[1]));
                    let t =
                        Fx::from_int(u + s.support[0]).div(Fx::from_int(2 * s.support[0].max(1)));
                    let line = floors[rear].add(floors[front].sub(floors[rear]).mul(t));
                    let w =
                        Fx::from_int((s.support[0] - u.abs()).min(fade)).div(Fx::from_int(fade));
                    let excess = g.sub(line).mul(w);
                    if excess > lift {
                        lift = excess;
                        top = g;
                    }
                    u += 1;
                }
                if lift > Fx::ZERO {
                    // Both wheels rise together, and neither above the hump.
                    for i in [rear, front] {
                        floors[i] = floors[i].max(floors[i].add(lift).min(top));
                    }
                }
            }
        }
        let height = floors
            .iter()
            .fold(Fx::ZERO, |a, b| a.add(*b))
            .div(Fx::from_int(4));
        let pitch = floors[1]
            .add(floors[3])
            .sub(floors[0])
            .sub(floors[2])
            .div(Fx::from_int(4 * s.support[0].max(1)));
        let roll = floors[2]
            .add(floors[3])
            .sub(floors[0])
            .sub(floors[1])
            .div(Fx::from_int(4 * s.support[1].max(1)));
        let tilt = s
            .min_up
            .mul(s.min_up)
            .mul(Fx::ONE.add(pitch.mul(pitch)).add(roll.mul(roll)))
            > Fx::ONE;
        let mut blocked = 0;
        let (lo, hi) = self.extent(pos, head, s);
        let mut crush = 0;
        for z in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                if !self.inside(pos, c, sin, s, x, z) {
                    continue;
                }
                let dx = Fx::from_int(x).add(Fx::from_ratio(1, 2)).sub(pos[0]);
                let dz = Fx::from_int(z).add(Fx::from_ratio(1, 2)).sub(pos[1]);
                let u = dx.mul(c).add(dz.mul(sin));
                let v = dz.mul(c).sub(dx.mul(sin));
                let ground = Fx::from_int(self.floor(x, z));
                let local = height.add(pitch.mul(u)).add(roll.mul(v));
                let belly = local.add(s.clearance.max(Fx::from_int(s.step)));
                if ground > belly {
                    blocked += 1;
                }
                if let Some(low) = self.yield_low(x, z) {
                    if Fx::from_int(low) < local.add(Fx::from_int(s.height)) {
                        crush += 1;
                    }
                }
            }
        }
        Pose {
            height,
            floors,
            blocked,
            steep,
            tilt,
            crush,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Search {
    pub cell: i32,
    pub min: [i32; 2],
    pub max: [i32; 2],
    pub nodes: usize,
    /// Planner price of one crushed footprint column; a straight move costs 10.
    pub crush_cost: u32,
    /// Ceiling of that price per node, so a dense crown is a detour of a few
    /// cells, not an impassable field that exhausts the search.
    pub crush_cap: u32,
    /// Extra contact length the planner probes with. Its "too steep" zone
    /// is that much wider than the executor's, so a hull following the plan
    /// never meets a refusal the plan did not see between two samples.
    pub margin: i32,
    /// Weight of the distance-to-goal estimate, percent: 100 is plain A*;
    /// more trades the shortest route for far fewer expansions where every
    /// step carries a crush price and the estimate falls short of it.
    pub heuristic_pct: u32,
}
const DIR: [[i32; 2]; 8] = [
    [1, 0],
    [1, 1],
    [0, 1],
    [-1, 1],
    [-1, 0],
    [-1, -1],
    [0, -1],
    [1, -1],
];
/// Whether the straight move is open, and how many yielding columns it crushes.
fn connects(
    ground: &mut Probe<'_>,
    shape: Shape,
    from: [Fx; 2],
    head: i32,
    to: [Fx; 2],
    clear: &impl Fn([Fx; 2], i32) -> bool,
) -> Option<usize> {
    let delta = [to[0].sub(from[0]), to[1].sub(from[1])];
    let yaw = if delta == [Fx::ZERO; 2] {
        head
    } else {
        angle::atan2(delta[1].raw(), delta[0].raw())
    };
    let turn = angle::diff(yaw, head);
    let turns = (turn.abs() / (angle::FULL / 16) + 1).max(1);
    let mut old = ground.pose(from, head, shape);
    let mut crush = 0;
    for i in 1..=turns {
        let h = angle::norm(head + turn * i / turns);
        let next = ground.pose(from, h, shape);
        if next.refused()
            || next.height.sub(old.height).abs() > Fx::from_int(shape.step)
            || !clear(from, h)
        {
            return None;
        }
        crush = crush.max(next.crush);
        old = next;
    }
    let steps = (delta[0].abs().max(delta[1].abs()).to_int_trunc() + 1).max(1);
    for i in 1..=steps {
        let f = Fx::from_ratio(i, steps);
        let p = [from[0].add(delta[0].mul(f)), from[1].add(delta[1].mul(f))];
        let next = ground.pose(p, yaw, shape);
        if next.refused()
            || next.height.sub(old.height).abs() > Fx::from_int(shape.step)
            || !clear(p, yaw)
        {
            return None;
        }
        crush += next.crush;
        old = next;
    }
    Some(crush)
}
/// The outcome of a route search.
pub struct Route {
    /// Turn points to the goal; empty when there is none.
    pub path: Vec<[i32; 2]>,
    /// No move at all was open from the start pose: the hull stands where
    /// the search cannot begin, and only moving will change that.
    pub stranded: bool,
    /// Nodes the search expanded and poses it probed: the price of the
    /// order, for benches and budgets.
    pub nodes: u32,
    pub poses: u32,
    /// Columns scanned and surface samples taken (P-29).
    pub scans: u64,
    pub samples: u64,
    /// The open set ran dry before the budget: no route exists in the
    /// world as probed, and only a change of the world can make one.
    pub exhausted: bool,
}
/// How far outside the straight line from hull to goal a search's window
/// of columns reaches, world units (ADR-317). A detour wider than this is
/// still found; its columns are simply read afresh instead of once.
const WINDOW_MARGIN: i32 = 256;

/// Where a search's window of columns sits and how far it reaches: around
/// the middle of the run, wide enough for the run and a detour. A window
/// over the whole map costs 600 MB on a range of a kilometre, and one is
/// built for every order (ADR-317).
pub fn search_window(from: [Fx; 2], to: [i32; 2]) -> ([Fx; 2], i32) {
    let goal = to.map(Fx::from_int);
    let centre = [0, 1].map(|a| from[a].add(goal[a]).div(Fx::from_int(2)));
    let reach = [0, 1]
        .map(|a| goal[a].sub(from[a]).abs().to_int_trunc())
        .into_iter()
        .max()
        .unwrap_or(0);
    (centre, reach / 2 + WINDOW_MARGIN)
}

/// An oriented A*: translations keep heading, turns reserve their swept
/// footprint. It cannot choose a narrow corridor and then rotate through it.
pub fn route(
    ground: Probe<'_>,
    shape: Shape,
    from: [Fx; 2],
    head: i32,
    to: [i32; 2],
    cfg: Search,
    clear: impl Fn([Fx; 2], i32) -> bool,
) -> Vec<[i32; 2]> {
    plan(ground, shape, from, head, to, cfg, clear).path
}
/// [`route`], also reporting a start the search could not leave. The whole
/// search runs in this one call; [`Hunt`] spreads the same work over
/// several (P-29).
pub fn plan(
    mut ground: Probe<'_>,
    shape: Shape,
    from: [Fx; 2],
    head: i32,
    to: [i32; 2],
    cfg: Search,
    clear: impl Fn([Fx; 2], i32) -> bool,
) -> Route {
    match Hunt::start(&mut ground, shape, from, head, to, cfg, &clear) {
        Hunting::Done(route) => route,
        Hunting::Open(mut hunt) => hunt
            .advance(&mut ground, &clear, cfg.nodes)
            .expect("a slice of the whole allowance finishes the search"),
    }
}

/// What a route search answered, or the search itself to carry on with.
pub enum Hunting {
    /// Answered without walking the graph at all.
    Done(Route),
    /// A search begun, with its frontier open. Boxed: a search in flight
    /// carries the whole reached set, an answer carries a handful of turn
    /// points.
    Open(Box<Hunt>),
}

/// A route search in flight (P-29).
///
/// The A* of [`plan`] keeps its frontier and everything it has reached
/// between slices, so a search too long for one tick is spread over
/// several instead of stopping one. The poses it has probed are a memo of
/// a pure function of the world, and they are not part of the search's
/// answer: a search resumed with an empty memo reaches the same answer,
/// as long as the world has not changed under it. A search whose world
/// changed is dropped and begun again, never resumed.
#[derive(Clone)]
pub struct Hunt {
    shape: Shape,
    cfg: Search,
    from: [Fx; 2],
    /// The goal as the search snapped it, not as it was asked.
    to: [i32; 2],
    /// Materials the probe of each slice treats as yielding.
    yielding: Vec<Mat>,
    /// What the slices before this one scanned. Answers about the world,
    /// so only as good as the world they came from: [`Hunt::forget_world`]
    /// drops them when it changes, and the search itself carries on.
    columns: Columns,
    n: usize,
    rows: usize,
    anchor: [i32; 2],
    initial: usize,
    end: usize,
    /// What the search knows about the cells it has reached, by cell and
    /// heading: the cost to get there, where it came from, and the pose it
    /// probed. Held by the cells reached rather than by every cell of the
    /// map (ADR-316): a route search reaches thousands, and a map of a
    /// kilometre has twelve million, which at the poses alone is most of a
    /// gigabyte for every order given.
    distance: BTreeMap<usize, u32>,
    previous: BTreeMap<usize, usize>,
    poses: BTreeMap<usize, Pose>,
    open: BinaryHeap<Reverse<(u32, u32, usize)>>,
    /// Pops taken against the allowance, expansions made, poses probed.
    spent: usize,
    nodes: u32,
    probed: u32,
    scans: u64,
    samples: u64,
    left_start: bool,
}

impl std::fmt::Debug for Hunt {
    /// The reached set is thousands of cells; a search prints as what it
    /// is looking for and how far it has got.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Hunt {{ to: {:?}, nodes: {}, open: {} }}",
            self.to,
            self.nodes,
            self.open.len()
        )
    }
}
impl Hunt {
    /// Begin a search: check the request, move the goal to a cell the hull
    /// may stand on, and try the straight run. Any of the three may answer
    /// outright, and then no graph is walked.
    pub fn start(
        ground: &mut Probe<'_>,
        shape: Shape,
        from: [Fx; 2],
        head: i32,
        to: [i32; 2],
        cfg: Search,
        clear: &impl Fn([Fx; 2], i32) -> bool,
    ) -> Hunting {
        let none = |stranded, exhausted, nodes, poses| {
            Hunting::Done(Route {
                path: vec![],
                stranded,
                nodes,
                poses,
                exhausted,
                scans: 0,
                samples: 0,
            })
        };
        let shape = Shape {
            span: if shape.span >= 2 {
                shape.span + cfg.margin.max(0)
            } else {
                shape.span
            },
            ..shape
        };
        if cfg.cell <= 0
            || cfg.nodes == 0
            || (0..2).any(|a| {
                to[a] < cfg.min[a]
                    || to[a] >= cfg.max[a]
                    || from[a] < Fx::ZERO
                    || from[a] >= Fx::from_int([ground.world.nx, ground.world.nz][a] as i32)
            })
            || ground.world.nx as i32 % cfg.cell != 0
            || ground.world.nz as i32 % cfg.cell != 0
        {
            return none(false, false, 0, 0);
        }
        // The goal must be a pose the hull may take, clear of the other
        // hulls: a point inside a parked hull, on a slope or against a wall
        // is moved to the nearest cell within four that is, else there is
        // no route at all — and the search does not spend its budget
        // finding that out.
        let mut probed = 0u32;
        let mut goal = None;
        'ring: for r in 0..=4i32 {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    let p = [to[0] + dx * cfg.cell, to[1] + dz * cfg.cell];
                    if (0..2).any(|a| p[a] < cfg.min[a] || p[a] >= cfg.max[a]) {
                        continue;
                    }
                    let q = p.map(Fx::from_int);
                    let open = (0..8).any(|d| {
                        let yaw = d * angle::FULL / 8;
                        probed += 1;
                        !ground.pose(q, yaw, shape).refused() && clear(q, yaw)
                    });
                    if open {
                        goal = Some(p);
                        break 'ring;
                    }
                }
            }
        }
        let Some(to) = goal else {
            return none(false, true, 0, probed);
        };
        // A straight run through yielding matter is legal but not free: let
        // the search look for cheaper open ground first.
        if connects(ground, shape, from, head, to.map(Fx::from_int), clear) == Some(0) {
            return Hunting::Done(Route {
                path: vec![to],
                stranded: false,
                nodes: 0,
                poses: probed,
                exhausted: false,
                scans: ground.scans,
                samples: ground.samples,
            });
        }
        let n = (ground.world.nx as i32).div_euclid(cfg.cell) as usize;
        let rows = (ground.world.nz as i32).div_euclid(cfg.cell) as usize;
        let start = from[0].to_int_trunc().div_euclid(cfg.cell) as usize
            + n * from[1].to_int_trunc().div_euclid(cfg.cell) as usize;
        let anchor = [
            from[0].to_int_trunc().rem_euclid(cfg.cell),
            from[1].to_int_trunc().rem_euclid(cfg.cell),
        ];
        let end = (to[0] - anchor[0]).div_euclid(cfg.cell) as usize
            + n * (to[1] - anchor[1]).div_euclid(cfg.cell) as usize;
        let start_dir = (angle::norm(head + angle::FULL / 16) / (angle::FULL / 8)) as usize % 8;
        let initial = start * 8 + start_dir;
        let mut distance = BTreeMap::new();
        let previous = BTreeMap::new();
        let poses = BTreeMap::new();
        let mut open = BinaryHeap::new();
        distance.insert(initial, 0);
        open.push(Reverse((0, 0, initial)));
        Hunting::Open(Box::new(Hunt {
            shape,
            cfg,
            from,
            to,
            yielding: ground.yielding.clone(),
            columns: Columns::default(),
            n,
            rows,
            anchor,
            initial,
            end,
            distance,
            previous,
            poses,
            open,
            spent: 0,
            nodes: 0,
            probed,
            scans: ground.scans,
            samples: ground.samples,
            left_start: false,
        }))
    }

    /// What the search has paid to reach this cell and heading; the most
    /// a `u32` holds when it has not reached it.
    fn cost(&self, i: usize) -> u32 {
        self.distance.get(&i).copied().unwrap_or(u32::MAX)
    }
    /// The goal as the search snapped it.
    pub fn goal(&self) -> [i32; 2] {
        self.to
    }
    /// Frontier still open and nodes expanded so far.
    pub fn nodes(&self) -> u32 {
        self.nodes
    }
    /// One slice of a search that spans ticks (P-29): a probe over the
    /// window the earlier slices filled, `budget` of the frontier, and the
    /// window kept for the next slice.
    pub fn advance_in(
        &mut self,
        world: &World,
        rubble: &Rubble,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        budget: usize,
    ) -> Option<Route> {
        let carried = std::mem::take(&mut self.columns);
        let (centre, radius) = search_window(self.from, self.to);
        let mut ground = Probe::around(world, &self.yielding, centre, radius)
            .with_rubble(rubble)
            .with_columns(carried);
        self.advance(&mut ground, clear, budget)
    }
    /// The world changed under the search: everything scanned or probed in
    /// the world as it was goes, the search itself stays. Its answers are
    /// a pure function of the world, so a search that forgets them finds
    /// what it would have found without them (P-29).
    pub fn forget_world(&mut self) {
        self.columns = Columns::default();
        self.poses.clear();
    }
    /// The search as bytes: the cells it reached, the frontier in priority
    /// order, and the request. What it scanned and probed is left out
    /// twice over — the window and the memo because they are answers about
    /// the world that a resumed search asks again, the counts of them
    /// because they count those asks and a resumed search asks fewer.
    /// A search restored from these bytes finds the same route (P-29).
    pub fn save(&self, b: &mut fx::blob::Blob) {
        for v in [
            self.shape.half[0],
            self.shape.half[1],
            self.shape.support[0],
            self.shape.support[1],
            self.shape.step,
            self.shape.height,
            self.shape.span,
        ] {
            b.i32(v);
        }
        for v in [self.shape.clearance, self.shape.min_up, self.shape.ramp] {
            b.fx(v);
        }
        for v in [
            self.cfg.cell,
            self.cfg.min[0],
            self.cfg.min[1],
            self.cfg.max[0],
            self.cfg.max[1],
            self.cfg.margin,
        ] {
            b.i32(v);
        }
        b.usize(self.cfg.nodes);
        for v in [
            self.cfg.crush_cost,
            self.cfg.crush_cap,
            self.cfg.heuristic_pct,
        ] {
            b.u32(v);
        }
        for v in self.from {
            b.fx(v);
        }
        for v in [self.to[0], self.to[1], self.anchor[0], self.anchor[1]] {
            b.i32(v);
        }
        for v in [self.n, self.rows, self.initial, self.end, self.spent] {
            b.usize(v);
        }
        b.u32(self.nodes);
        b.bool(self.left_start);
        // Only the cells the search reached; the rest is its default.
        // A map keeps them in the order of the cell, which is the order
        // the dense array was written in before (ADR-316).
        b.u32(self.distance.len() as u32);
        for (&i, &d) in &self.distance {
            b.u32(i as u32);
            b.u32(d);
            b.u32(self.previous.get(&i).copied().unwrap_or(usize::MAX) as u32);
        }
        // Heap storage order is an implementation detail. Persist priority
        // order, as the blast wave does with its own frontier.
        let mut open: Vec<(u32, u32, usize)> = self.open.iter().map(|Reverse(v)| *v).collect();
        open.sort_unstable();
        b.u32(open.len() as u32);
        for (f, cost, i) in open {
            b.u32(f);
            b.u32(cost);
            b.u32(i as u32);
        }
    }
    /// A search read back. `None` when the bytes do not describe one.
    pub fn load(s: &mut fx::blob::Scan, yielding: &[Mat]) -> Option<Hunt> {
        let mut i32s = [0i32; 7];
        for v in &mut i32s {
            *v = s.i32();
        }
        let shape = Shape {
            half: [i32s[0], i32s[1]],
            support: [i32s[2], i32s[3]],
            step: i32s[4],
            height: i32s[5],
            span: i32s[6],
            clearance: s.fx(),
            min_up: s.fx(),
            ramp: s.fx(),
        };
        let mut c = [0i32; 6];
        for v in &mut c {
            *v = s.i32();
        }
        let cfg = Search {
            cell: c[0],
            min: [c[1], c[2]],
            max: [c[3], c[4]],
            margin: c[5],
            nodes: s.usize(),
            crush_cost: s.u32(),
            crush_cap: s.u32(),
            heuristic_pct: s.u32(),
        };
        let from = [s.fx(), s.fx()];
        let to = [s.i32(), s.i32()];
        let anchor = [s.i32(), s.i32()];
        let (n, rows, initial, end, spent) =
            (s.usize(), s.usize(), s.usize(), s.usize(), s.usize());
        let nodes = s.u32();
        let left_start = s.bool();
        let cells = n.checked_mul(rows)?.checked_mul(8)?;
        if !s.ok() || cells == 0 || cells > 1 << 22 || initial >= cells {
            return None;
        }
        let mut hunt = Hunt {
            shape,
            cfg,
            from,
            to,
            yielding: yielding.to_vec(),
            columns: Columns::default(),
            n,
            rows,
            anchor,
            initial,
            end,
            distance: BTreeMap::new(),
            previous: BTreeMap::new(),
            poses: BTreeMap::new(),
            open: BinaryHeap::new(),
            spent,
            nodes,
            probed: 0,
            scans: 0,
            samples: 0,
            left_start,
        };
        let reached = s.u32() as usize;
        if reached > cells {
            return None;
        }
        for _ in 0..reached {
            let i = s.u32() as usize;
            let (d, p) = (s.u32(), s.u32());
            if i >= cells {
                return None;
            }
            hunt.distance.insert(i, d);
            if p != u32::MAX {
                hunt.previous.insert(i, p as usize);
            }
        }
        let open = s.u32() as usize;
        if open > cells {
            return None;
        }
        for _ in 0..open {
            let (f, cost, i) = (s.u32(), s.u32(), s.u32() as usize);
            if i >= cells {
                return None;
            }
            hunt.open.push(Reverse((f, cost, i)));
        }
        s.ok().then_some(hunt)
    }

    fn point(&self, i: usize) -> [Fx; 2] {
        [
            Fx::from_int((i % self.n) as i32 * self.cfg.cell + self.anchor[0]),
            Fx::from_int((i / self.n) as i32 * self.cfg.cell + self.anchor[1]),
        ]
    }

    /// Walk at most `budget` of the frontier. `None` while the search goes
    /// on, the answer once it is over: a route found, a graph run dry, or
    /// the whole allowance of `cfg.nodes` spent.
    pub fn advance(
        &mut self,
        ground: &mut Probe<'_>,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        budget: usize,
    ) -> Option<Route> {
        let (scans, samples) = (ground.scans, ground.samples);
        let mut found = None;
        let mut exhausted = false;
        let mut slice = 0usize;
        while self.spent < self.cfg.nodes && slice < budget {
            self.spent += 1;
            slice += 1;
            let Some(Reverse((_, cost, i))) = self.open.pop() else {
                exhausted = true;
                break;
            };
            if cost != self.cost(i) {
                continue;
            }
            self.nodes += 1;
            let at = self.point(i / 8);
            let direction = i % 8;
            let yaw = direction as i32 * angle::FULL / 8;
            let pose = match self.poses.get(&i) {
                Some(p) => *p,
                None => {
                    self.probed += 1;
                    let p = ground.pose(at, yaw, self.shape);
                    self.poses.insert(i, p);
                    p
                }
            };
            if ((at[0].to_int_trunc() - self.to[0])
                .abs()
                .max((at[1].to_int_trunc() - self.to[1]).abs())
                <= self.cfg.cell * 4
                || i / 8 == self.end)
                && !pose.refused()
                && clear(at, yaw)
                && connects(
                    ground,
                    self.shape,
                    at,
                    yaw,
                    self.to.map(Fx::from_int),
                    clear,
                )
                .is_some()
            {
                found = Some(i);
                break;
            }
            for turn in [-1, 0, 1] {
                let d = (direction as i32 + turn).rem_euclid(8) as usize;
                let next_yaw = d as i32 * angle::FULL / 8;
                let cell = if turn == 0 {
                    let x = (i / 8 % self.n) as i32 + DIR[d][0];
                    let z = (i / 8 / self.n) as i32 + DIR[d][1];
                    if x < 0 || z < 0 || x >= self.n as i32 || z >= self.rows as i32 {
                        continue;
                    }
                    x as usize + self.n * z as usize
                } else {
                    i / 8
                };
                let q = self.point(cell);
                if (0..2).any(|a| {
                    q[a] < Fx::from_int(self.cfg.min[a]) || q[a] >= Fx::from_int(self.cfg.max[a])
                }) {
                    continue;
                }
                let j = cell * 8 + d;
                let next = match self.poses.get(&j) {
                    Some(p) => *p,
                    None => {
                        self.probed += 1;
                        let p = ground.pose(q, next_yaw, self.shape);
                        self.poses.insert(j, p);
                        p
                    }
                };
                let next_cost =
                    cost + if turn == 0 {
                        if d.is_multiple_of(2) {
                            10
                        } else {
                            14
                        }
                    } else {
                        6
                    } + (next.crush as u32)
                        .saturating_mul(self.cfg.crush_cost)
                        .min(self.cfg.crush_cap);
                if next_cost >= self.cost(j) {
                    continue;
                }
                if next.refused()
                    || !clear(q, next_yaw)
                    || next.height.sub(pose.height).abs() > Fx::from_int(self.shape.step)
                {
                    continue;
                }
                let mid = [
                    at[0].add(q[0]).div(Fx::from_int(2)),
                    at[1].add(q[1]).div(Fx::from_int(2)),
                ];
                let mid_yaw = angle::norm(yaw + angle::diff(next_yaw, yaw) / 2);
                self.probed += 1;
                if ground.pose(mid, mid_yaw, self.shape).refused() || !clear(mid, mid_yaw) {
                    continue;
                }
                self.distance.insert(j, next_cost);
                self.previous.insert(j, i);
                self.left_start = true;
                let heuristic = q[0]
                    .sub(Fx::from_int(self.to[0]))
                    .abs()
                    .max(q[1].sub(Fx::from_int(self.to[1])).abs())
                    .to_int_trunc()
                    / self.cfg.cell
                    * self.cfg.heuristic_pct.max(1) as i32
                    / 10;
                self.open
                    .push(Reverse((next_cost + heuristic as u32, next_cost, j)));
            }
        }
        self.scans += ground.scans.saturating_sub(scans);
        self.samples += ground.samples.saturating_sub(samples);
        // What this slice scanned goes to the next one (P-29).
        self.columns = ground.take_columns();
        match found {
            Some(i) => Some(self.route_through(i)),
            None if exhausted || self.spent >= self.cfg.nodes => Some(Route {
                path: vec![],
                stranded: !self.left_start,
                nodes: self.nodes,
                poses: self.probed,
                exhausted,
                scans: self.scans,
                samples: self.samples,
            }),
            None => None,
        }
    }

    /// The turn points from the start to the node the search stopped on,
    /// with the collinear ones between them dropped.
    fn route_through(&self, mut i: usize) -> Route {
        let mut all = vec![];
        while i != self.initial {
            let p = self.point(i / 8).map(Fx::to_int_trunc);
            if all.last() != Some(&p) {
                all.push(p);
            }
            i = self.previous.get(&i).copied().unwrap_or(usize::MAX);
        }
        all.reverse();
        if all.last() != Some(&self.to) {
            all.push(self.to);
        }
        // Remove collinear grid points while retaining the exact turn centres.
        let mut out = vec![];
        let mut before = self.from.map(Fx::to_int_trunc);
        for (i, p) in all.iter().enumerate() {
            let keep = i + 1 == all.len() || {
                let q = all[i + 1];
                (p[0] - before[0]) * (q[1] - p[1]) != (p[1] - before[1]) * (q[0] - p[0])
            };
            if keep {
                out.push(*p);
            }
            before = *p;
        }
        Route {
            path: out,
            stranded: false,
            nodes: self.nodes,
            poses: self.probed,
            exhausted: false,
            scans: self.scans,
            samples: self.samples,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestGround {
        rock: bool,
        corridor: bool,
        hill: bool,
    }
    impl voxel::Generator for TestGround {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            let height = if self.hill {
                16 + (12 - (x as i32 - 128).abs() / 6).max(0) as u32
            } else {
                16
            };
            if y < height
                || (self.rock && x == 90 && z == 72 && y < 30)
                || (self.corridor && (58..=82).contains(&x) && !(57..84).contains(&z) && y < 55)
            {
                1
            } else {
                0
            }
        }
    }
    fn world(rock: bool, corridor: bool, hill: bool) -> World {
        World::new(
            256,
            64,
            256,
            100,
            Box::new(TestGround {
                rock,
                corridor,
                hill,
            }),
        )
    }
    fn tank() -> Shape {
        Shape {
            half: [30, 12],
            support: [25, 10],
            clearance: Fx::from_ratio(45, 10),
            step: 4,
            min_up: Fx::from_ratio(91, 100),
            height: 30,
            span: 25,
            ramp: Fx::from_ratio(3, 5),
        }
    }
    /// Flat soil with a one-metre ditch across x = 118..128 and a 30 cm
    /// ledge at x = 180.
    struct Cut;
    impl voxel::Generator for Cut {
        fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
            let h = if (118..128).contains(&x) {
                10
            } else if x >= 180 {
                19
            } else {
                16
            };
            u16::from(y < h)
        }
    }
    /// A resting heap of debris raises the probe's floor in its columns
    /// only, and a hull whose step is lower than the heap is refused over
    /// it while a hull that steps higher is not (ADR-292).
    #[test]
    fn rubble_raises_the_floor_of_its_columns_and_blocks_a_hull_below_its_step() {
        let w = world(false, false, false);
        let mut rubble = Rubble::new();
        rubble.insert([100, 100], 19);
        let mut p = Probe::new(&w).with_rubble(&rubble);
        assert_eq!(p.floor(100, 100), 19);
        assert_eq!(p.floor(101, 100), 16);
        assert_eq!(Probe::new(&w).floor(100, 100), 16);
        let over = [Fx::from_ratio(201, 2), Fx::from_ratio(201, 2)];
        let mut low = tank();
        low.step = 2;
        low.clearance = Fx::ONE;
        assert!(p.pose(over, 0, low).blocked > 0);
        let mut high = tank();
        high.step = 4;
        assert_eq!(p.pose(over, 0, high).blocked, 0);
    }
    #[test]
    fn a_track_bridges_a_narrow_ditch_and_climbs_a_ledge_over_its_run_up() {
        let w = World::new(256, 64, 256, 100, Box::new(Cut));
        let mut p = Probe::new(&w);
        let mut point = tank();
        point.span = 0;
        let over = [Fx::from_int(123), Fx::from_int(100)];
        let front_over = [Fx::from_int(98), Fx::from_int(100)];
        // A point support drops into the ditch; the track spans it.
        assert!(p.pose(front_over, 0, point).height < Fx::from_int(16));
        assert_eq!(p.pose(front_over, 0, tank()).height, Fx::from_int(16));
        assert_eq!(p.pose(over, 0, tank()).height, Fx::from_int(16));
        // The ledge lifts the front supports gradually, three units over five.
        let mut last = Fx::from_int(16);
        for x in 140..=160 {
            let h = p
                .pose([Fx::from_int(x), Fx::from_int(100)], 0, tank())
                .height;
            assert!(h >= last && h.sub(last) <= Fx::from_ratio(61, 100), "x {x}");
            last = h;
        }
        assert!(last > Fx::from_int(16));
        // A point support takes the step within its own patch: two units
        // of travel lift the hull more than the belt's whole run-up does.
        let before = p
            .pose([Fx::from_int(154), Fx::from_int(100)], 0, point)
            .height;
        let after = p
            .pose([Fx::from_int(156), Fx::from_int(100)], 0, point)
            .height;
        assert!(after.sub(before) > Fx::from_ratio(1, 2));
    }
    #[test]
    fn a_grade_steeper_than_the_tilt_limit_blocks_even_a_level_hull() {
        struct Rim;
        impl voxel::Generator for Rim {
            fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
                // A 45-degree bank ten units long between two flats.
                let h = 16 + (x as i32 - 120).clamp(0, 10);
                u16::from((y as i32) < h)
            }
        }
        let w = World::new(256, 64, 256, 100, Box::new(Rim));
        let mut p = Probe::new(&w);
        // Front supports two units short of the bank: the chord over the
        // wheelbase is level, the belt ahead sees 45 degrees, and 45 is
        // more than the 24-degree limit.
        let at = [Fx::from_int(93), Fx::from_int(100)];
        assert!(p.pose(at, 0, tank()).steep, "the bank was accepted");
        let mut gentle = tank();
        gentle.min_up = Fx::from_ratio(1, 2);
        assert!(!p.pose(at, 0, gentle).refused());
        let mut point = tank();
        point.span = 0;
        assert!(!p.pose(at, 0, point).refused(), "a point sees no grade");
    }
    /// Flat soil with a 60 cm boulder, seven columns square.
    struct Boulder;
    impl voxel::Generator for Boulder {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            let h = if (150..157).contains(&x) && (120..127).contains(&z) {
                22
            } else {
                16
            };
            u16::from(y < h)
        }
    }
    #[test]
    fn a_boulder_under_a_contact_corner_is_refused_like_one_under_the_belt() {
        let w = World::new(256, 64, 256, 100, Box::new(Boulder));
        let mut p = Probe::new(&w);
        // The rear-right support stands at (148, 118): its belt line along
        // x misses the boulder, its diagonal corner sample two units away
        // lands on it. Six units is more than the step; refused.
        let on = p.pose([Fx::from_int(173), Fx::from_int(108)], 0, tank());
        assert!(on.steep, "the corner rode the boulder: {:?}", on.floors);
        // Four units further from the boulder the same pose is open.
        let off = p.pose([Fx::from_int(173), Fx::from_int(104)], 0, tank());
        assert!(!off.refused(), "{:?}", off.floors);
        assert_eq!(off.floors, [Fx::from_int(16); 4]);
    }
    fn search() -> Search {
        Search {
            cell: 8,
            min: [32; 2],
            max: [224; 2],
            nodes: 6400,
            crush_cost: 1,
            crush_cap: 20,
            margin: 2,
            heuristic_pct: 100,
        }
    }
    /// A soil floor with a wall of trees (trunk 7, crown 8) across x = 120..126.
    struct Forest;
    impl voxel::Generator for Forest {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            if y < 16 {
                return 1;
            }
            let trunk = (120..126).contains(&x) && z % 9 < 3 && y < 40;
            let crown = (116..130).contains(&x) && (24..46).contains(&y);
            if trunk {
                7
            } else if crown {
                8
            } else {
                0
            }
        }
    }
    #[test]
    fn yielding_trees_are_crushed_by_a_tracked_hull_and_avoided_by_others() {
        let w = World::new(256, 64, 256, 100, Box::new(Forest));
        let at = [Fx::from_int(123), Fx::from_int(100)];
        let mut hard = Probe::new(&w);
        assert!(hard.pose(at, 0, tank()).refused());
        assert_eq!(hard.floor(123, 100), 46, "crown top is the hard floor");
        assert!(hard.crushed(at, 0, tank()).is_empty());
        let mut soft = Probe::with_yield(&w, &[7, 8]);
        let pose = soft.pose(at, 0, tank());
        assert!(!pose.refused());
        assert_eq!(soft.floor(123, 100), 16, "trees are not floor");
        assert!(pose.crush > 0);
        let cut = soft.crushed(at, 0, tank());
        assert!(cut.iter().all(|c| c[1] < 16 + 30), "cut above the hull top");
        assert!(cut.iter().any(|c| w.material(c[0], c[1], c[2]) == 7));
        assert!(cut.iter().any(|c| w.material(c[0], c[1], c[2]) == 8));
        assert!(
            cut.windows(2)
                .all(|p| (p[0][2], p[0][0], p[0][1]) < (p[1][2], p[1][0], p[1][1])),
            "cut order is not column order"
        );
        let open = route(
            Probe::with_yield(&w, &[7, 8]),
            tank(),
            [Fx::from_int(60), Fx::from_int(100)],
            0,
            [200, 100],
            search(),
            |_, _| true,
        );
        assert_eq!(open.last(), Some(&[200, 100]), "no route through the trees");
        let walker = route(
            Probe::new(&w),
            tank(),
            [Fx::from_int(60), Fx::from_int(100)],
            0,
            [200, 100],
            search(),
            |_, _| true,
        );
        assert!(walker.is_empty(), "a hard hull passed through the wall");
    }
    #[test]
    fn narrow_opening_uses_width_and_forbids_a_turn_inside_it() {
        let w = world(false, true, false);
        let mut p = Probe::new(&w);
        let at = [Fx::from_int(70); 2];
        assert!(!p.pose(at, 0, tank()).refused());
        assert!(p.pose(at, angle::FULL / 8, tank()).refused());
        let route = route(
            Probe::new(&w),
            tank(),
            [Fx::from_int(40), Fx::from_int(70)],
            0,
            [200, 70],
            search(),
            |_, _| true,
        );
        assert!(!route.is_empty());
        assert_eq!(route.last(), Some(&[200, 70]));
    }
    #[test]
    fn a_single_voxel_obstacle_between_supports_blocks_the_full_hull() {
        let mut w = world(true, false, false);
        let mut p = Probe::new(&w);
        let at = [Fx::from_int(80), Fx::from_int(70)];
        let pose = p.pose(at, 0, tank());
        assert_eq!(pose.floors, [Fx::from_int(16); 4]);
        assert!(pose.blocked > 0);
        for y in 16..30 {
            w.queue_dig(90, y, 72);
        }
        w.flush();
        assert!(!Probe::new(&w).pose(at, 0, tank()).refused());
    }
    /// A goal inside a parked hull is served next to it (P-17): the goal
    /// moves to the nearest open cell instead of the search running the
    /// whole map dry to find that the point itself is taken.
    #[test]
    fn a_goal_inside_a_parked_hull_is_served_next_to_it() {
        let w = world(false, false, false);
        let parked = |p: [Fx; 2], _: i32| {
            (p[0].to_int_trunc() - 200).abs() > 20 || (p[1].to_int_trunc() - 70).abs() > 20
        };
        let r = plan(
            Probe::new(&w),
            tank(),
            [Fx::from_int(40), Fx::from_int(70)],
            0,
            [200, 70],
            search(),
            parked,
        );
        assert!(!r.exhausted && !r.path.is_empty());
        let end = *r.path.last().unwrap();
        assert!(
            parked(end.map(Fx::from_int), 0),
            "the route ends inside the hull: {end:?}"
        );
        assert!(
            (end[0] - 200).abs() <= 32 && (end[1] - 70).abs() <= 32,
            "far from the goal: {end:?}"
        );
        assert!(r.nodes < 400, "the snap cost a search: {} nodes", r.nodes);
    }
    /// A goal cut off from the hull ends the search when the open set runs
    /// dry, well under the budget, and says so (P-17).
    #[test]
    fn a_cut_off_goal_ends_the_search_when_the_open_set_runs_dry() {
        let w = world(false, false, false);
        let fence = |p: [Fx; 2], _: i32| !(100..110).contains(&p[0].to_int_trunc());
        let r = plan(
            Probe::new(&w),
            tank(),
            [Fx::from_int(40), Fx::from_int(70)],
            0,
            [200, 70],
            search(),
            fence,
        );
        assert!(r.path.is_empty() && r.exhausted);
        assert!(r.nodes > 0 && (r.nodes as usize) < search().nodes);
    }
    #[test]
    fn rounded_crest_and_gradual_slopes_fit_the_vehicle_clearance() {
        let w = world(false, false, true);
        let mut p = Probe::new(&w);
        for x in 90..168 {
            let at = [Fx::from_int(x), Fx::from_int(128)];
            assert!(!p.pose(at, 0, tank()).refused(), "crest at {x}");
            assert!(
                !p.pose(at, angle::FULL / 2, tank()).refused(),
                "reverse crest at {x}"
            );
        }
    }
}
