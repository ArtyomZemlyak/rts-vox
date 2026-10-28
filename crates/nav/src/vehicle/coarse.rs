//! Whole-map terrain routes. The coarse corridor guides movement; live
//! oriented collision probes still authorize every step and local replanning.
//!
//! A search is a value that walks on under a work allowance (ADR-680): a
//! caller may run it to the end in one call, or a slice per tick, and on a
//! world that does not change both give the same route. Work is counted in
//! terrain tiles a corridor test reads, not in nodes: one node of a wide
//! hull costs many tiles, and a node count bounded nothing in time.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub cell: i32,
    pub nodes: usize,
    pub yielding: [Mat; 2],
    /// Wide articulated legs may climb a gradual grade across their broad
    /// footprint; a tracked belt still needs the strict combined height.
    pub local_grade: bool,
    /// How many times a search in flight starts its check of a found route,
    /// or the whole search, over because the world changed under it
    /// (ADR-680). A search run in one call never restarts.
    pub restarts: u32,
}

pub fn plan(
    world: &World,
    rubble: &Rubble,
    shape: Shape,
    from: [Fx; 2],
    to: [i32; 2],
    cfg: Config,
) -> Option<Route> {
    plan_explained(world, rubble, shape, from, to, cfg).ok()
}

/// Why the whole-map planner gave no route. Diagnostics only: the planner
/// itself treats every refusal alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Cell too small or no node allowance.
    Config,
    /// Start or goal off the lattice.
    OffLattice,
    /// The corridor from the start to its node, or from the goal's node
    /// to the goal, is not clear.
    StartCorridor,
    GoalCorridor,
    /// No corridor leaves the start, but the lattice node `at` near it has
    /// clear standing room: the route begins with the oriented planner
    /// leading the hull there (ADR-691).
    Entry {
        at: [i32; 2],
    },
    /// One frontier ran dry: that side is enclosed for this shape.
    /// 0 is the start's side, 1 the goal's.
    Enclosed {
        side: usize,
        expanded: usize,
    },
    /// The node allowance ran out.
    Budget,
    /// The route has more turns than a route may hold.
    TooLong,
}

/// [`plan`], with the reason for a refusal: a [`Search`] run to its end
/// in one call.
pub fn plan_explained(
    world: &World,
    rubble: &Rubble,
    shape: Shape,
    from: [Fx; 2],
    to: [i32; 2],
    cfg: Config,
) -> Result<Route, Refusal> {
    let mut search = Search::new([world.nx, world.nz], shape, from, to, cfg)?;
    search
        .advance(world, rubble, u64::MAX)
        .0
        .expect("an unbounded allowance finishes the search")
}

/// The corridor test of the search and its price.
struct Ground<'a> {
    world: &'a World,
    debris: Arc<DebrisTiles>,
    yielding: [Mat; 2],
    radius: i32,
    step: i32,
    local_grade: bool,
    /// Previous row of this corridor only; never retained across world views.
    row: Vec<[u16; 2]>,
}
impl<'a> Ground<'a> {
    fn new(world: &'a World, rubble: &Rubble, shape: Shape, cfg: Config) -> Self {
        Self {
            world,
            debris: rubble.coarse_tiles([world.nx, world.nz]),
            yielding: cfg.yielding,
            // A broad travel corridor, not an oriented turning/clearance
            // proof. Short maneuvers and actual movement keep the detailed
            // footprint probe.
            radius: shape.half[0].min(shape.half[1]) + 1,
            step: shape.step,
            local_grade: cfg.local_grade,
            row: Vec::new(),
        }
    }
    #[cfg(test)]
    fn tile(&self, x: i32, z: i32) -> [u16; 2] {
        let [bottom, top] = self
            .world
            .floor_tile(x as u32, z as u32, self.yielding)
            .unwrap();
        [bottom, top.max(self.debris.top(x as usize, z as usize))]
    }
    /// The columns a corridor test between `a` and `b` reads, or `None`
    /// when its box leaves the usable map and the test refuses unread.
    fn bounds(&self, a: [i32; 2], b: [i32; 2]) -> Option<([i32; 2], [i32; 2])> {
        let lo = [0, 1].map(|k| a[k].min(b[k]) - self.radius);
        let hi = [0, 1].map(|k| a[k].max(b[k]) + self.radius);
        (lo[0] >= 1
            && lo[1] >= 1
            && hi[0] < self.world.nx as i32 - 1
            && hi[1] < self.world.nz as i32 - 1)
            .then_some((lo, hi))
    }
    /// Logical work: the original tile/neighbor allowance, independent of
    /// reused reads and early refusals; one for a test refused unread.
    fn cost(&self, a: [i32; 2], b: [i32; 2]) -> u64 {
        self.bounds(a, b).map_or(1, |(lo, hi)| {
            let tiles = [0, 1].map(|k| (hi[k] / 8 - lo[k] / 8 + 1) as u64);
            tiles[0] * tiles[1] * if self.local_grade { 3 } else { 1 }
        })
    }
    fn clear(&mut self, a: [i32; 2], b: [i32; 2]) -> bool {
        let Some((lo, hi)) = self.bounds(a, b) else {
            return false;
        };
        if self.local_grade {
            let width = (hi[0] / 8 - lo[0] / 8 + 1) as usize;
            // Missing upper/left neighbours have neutral extrema. Reset on
            // every rectangle: an earlier refusal may have left a partial row.
            self.row.resize(width, [u16::MAX, 0]);
            self.row.fill([u16::MAX, 0]);
            for z in lo[1] / 8..=hi[1] / 8 {
                let mut left = [u16::MAX, 0];
                let tiles = self
                    .world
                    .floor_tile_row(lo[0] as u32 / 8, z as u32, width, self.yielding)
                    .unwrap();
                for (column, [bottom, top]) in tiles.enumerate() {
                    let x = lo[0] / 8 + column as i32;
                    let top = top.max(self.debris.top(x as usize, z as usize));
                    let above = self.row[column];
                    // Previously visited neighbours have already passed their
                    // own height checks. Only this tile and the cross terms
                    // remain: top - neighbour bottom, neighbour top - bottom.
                    // Do not compare the left neighbour with the upper one;
                    // they need not be mutually traversable. Signed height
                    // differences also keep extreme step values overflow-free.
                    if i32::from(top) - i32::from(bottom.min(left[0]).min(above[0])) > self.step
                        || i32::from(left[1].max(above[1])) - i32::from(bottom) > self.step
                    {
                        return false;
                    }
                    self.row[column] = [bottom, top];
                    left = [bottom, top];
                }
            }
            true
        } else {
            let mut low = u16::MAX;
            let mut high = 0;
            for z in lo[1] / 8..=hi[1] / 8 {
                let width = (hi[0] / 8 - lo[0] / 8 + 1) as usize;
                let tiles = self
                    .world
                    .floor_tile_row(lo[0] as u32 / 8, z as u32, width, self.yielding)
                    .unwrap();
                for (column, [bottom, top]) in tiles.enumerate() {
                    let x = lo[0] / 8 + column as i32;
                    let top = top.max(self.debris.top(x as usize, z as usize));
                    low = low.min(bottom);
                    high = high.max(top);
                }
                // Bounds only widen. Check once per contiguous row so a
                // clear corridor retains its branch-free inner reduction.
                if i32::from(high - low) > self.step {
                    return false;
                }
            }
            i32::from(high - low) <= self.step
        }
    }
}

/// The work a call may still spend. Work never exceeds it: a call whose
/// allowance is below [`Search::max_test`] may make no progress at all.
struct Meter {
    budget: u64,
    spent: u64,
}
impl Meter {
    fn take(&mut self, cost: u64) -> bool {
        if self.spent.saturating_add(cost) <= self.budget {
            self.spent = self.spent.saturating_add(cost);
            true
        } else {
            false
        }
    }
}

/// One call's view: the ground, the meter, and the region read so far.
struct Walk<'a> {
    ground: Ground<'a>,
    meter: Meter,
    explored: Option<[[i32; 2]; 2]>,
}
impl Walk<'_> {
    /// A metered corridor test; `None` when the allowance is spent.
    fn test(&mut self, a: [i32; 2], b: [i32; 2]) -> Option<bool> {
        self.test_then(a, b, 0)
    }
    /// A metered test priced together with `then`, the work that follows
    /// it whatever it answers (ADR-691); both or neither are paid.
    fn test_then(&mut self, a: [i32; 2], b: [i32; 2], then: u64) -> Option<bool> {
        if !self.meter.take(self.ground.cost(a, b).saturating_add(then)) {
            return None;
        }
        let [lo, hi] = read_box(a, b, self.ground.radius);
        self.explored = Some(match self.explored {
            None => [lo, hi],
            Some([l, h]) => [
                [l[0].min(lo[0]), l[1].min(lo[1])],
                [h[0].max(hi[0]), h[1].max(hi[1])],
            ],
        });
        Some(self.ground.clear(a, b))
    }
}
/// Columns of the whole tiles a test between `a` and `b` reads.
fn read_box(a: [i32; 2], b: [i32; 2], radius: i32) -> [[i32; 2]; 2] {
    let lo = [0, 1].map(|k| ((a[k].min(b[k]) - radius).max(0) / 8) * 8);
    let hi = [0, 1].map(|k| ((a[k].max(b[k]) + radius).max(0) / 8) * 8 + 7);
    [lo, hi]
}

/// Lattice nodes whose dense entry costs one unit of work to build
/// (ADR-691): writing and faulting in a node's distances costs at most
/// about a sixteenth of reading one terrain tile (analysis 261).
pub const LATTICE_NODES_PER_UNIT: u64 = 16;

type Open = BinaryHeap<Reverse<(u32, u32, usize)>>;
const FAR: u32 = u32::MAX;

/// A node whose eight edges are being tested; `dir` is the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Expanding {
    side: usize,
    node: usize,
    cost: u32,
    dir: usize,
}
/// Distance and predecessor of a reached node, per frontier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reached {
    distance: [u32; 2],
    previous: [u32; 2],
}
const UNREACHED: Reached = Reached {
    distance: [FAR; 2],
    previous: [FAR; 2],
};
/// Every lattice node's distances, dense as the one-call planner had
/// them: a map of the reached nodes only was measurably slower
/// (analysis 248).
#[derive(Clone)]
struct Grid(Vec<Reached>);
impl Grid {
    fn new(cells: usize) -> Self {
        Self(vec![UNREACHED; cells])
    }
    fn get(&self, i: usize) -> Reached {
        self.0[i]
    }
    fn set(&mut self, i: usize, r: Reached) {
        self.0[i] = r;
    }
}
/// The two frontiers meeting under one shared cap.
#[derive(Clone)]
struct Meet {
    reached: Grid,
    open: [Open; 2],
    expanded: usize,
    node: Option<Expanding>,
}
/// One frontier from the start toward the goal, for a partial route.
#[derive(Clone)]
struct Approach {
    refusal: Refusal,
    allowance: usize,
    reached: Grid,
    open: Open,
    best: (u32, usize),
    expanded: usize,
    node: Option<Expanding>,
}
/// A found chain of nodes, before its first leg is joined.
#[derive(Clone)]
struct Found {
    path: Vec<[i32; 2]>,
    partial: bool,
    nodes: u32,
}
/// A found route checked again, leg by leg, against the world as it is
/// now, because the world changed where the search had read.
#[derive(Clone)]
struct Recheck {
    route: Route,
    proof: Vec<[[i32; 2]; 2]>,
    next: usize,
}
#[derive(Clone)]
enum Stage {
    /// The straight corridor in sections; `step` is the next section.
    Straight {
        step: i32,
        last: [i32; 2],
    },
    /// Lattice nodes near the start, nearest first; `next` is the next one.
    Snap {
        candidates: Vec<usize>,
        next: usize,
    },
    /// The corridor from the goal's node to the goal.
    Goal,
    Meet(Box<Meet>),
    Approach(Box<Approach>),
    Found(Box<Found>),
    Recheck(Box<Recheck>),
}
impl Stage {
    fn tag(&self) -> u8 {
        match self {
            Stage::Straight { .. } => 0,
            Stage::Snap { .. } => 1,
            Stage::Goal => 2,
            Stage::Meet(_) => 3,
            Stage::Approach(_) => 4,
            Stage::Found(_) => 5,
            Stage::Recheck(_) => 6,
        }
    }
}
enum Step {
    Continue,
    Pause,
    Done(Result<Route, Refusal>),
}

/// A whole-map search in flight (ADR-680). Its answer is a function of
/// the request and the world each test read: sliced over ticks on an
/// unchanged world it equals the answer of one call. When the world
/// changes where it has read, the route it finds is tested again, leg by
/// leg, before it is handed out; a leg no longer clear starts the search
/// over, up to `Config::restarts` times, after which the route stands as
/// found and the hull's live collision check governs it.
#[derive(Clone)]
pub struct Search {
    shape: Shape,
    cfg: Config,
    // Derived from cfg on construction/load; absent from the saved frontier.
    cell_shift: Option<u32>,
    from: [Fx; 2],
    to: [i32; 2],
    origin: [i32; 2],
    n: usize,
    rows: usize,
    nearest: usize,
    end: usize,
    start: usize,
    stage: Stage,
    explored: Option<[[i32; 2]; 2]>,
    stale: bool,
    restarts: u32,
    spent: u64,
}
impl std::fmt::Debug for Search {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Search")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("stage", &self.stage.tag())
            .field("restarts", &self.restarts)
            .field("spent", &self.spent)
            .finish()
    }
}
impl Search {
    /// A search from `from` to `to` on a world of `size` columns; refused
    /// at once for a bad configuration or an end off the lattice.
    pub fn new(
        size: [u32; 2],
        shape: Shape,
        from: [Fx; 2],
        to: [i32; 2],
        cfg: Config,
    ) -> Result<Self, Refusal> {
        if cfg.cell < 8 || cfg.nodes == 0 {
            return Err(Refusal::Config);
        }
        let n = (size[0] as i32).div_euclid(cfg.cell) as usize;
        let rows = (size[1] as i32).div_euclid(cfg.cell) as usize;
        let origin = from.map(Fx::to_int_trunc);
        let mut search = Self {
            shape,
            cfg,
            cell_shift: (cfg.cell as u32)
                .is_power_of_two()
                .then(|| (cfg.cell as u32).trailing_zeros()),
            from,
            to,
            origin,
            n,
            rows,
            nearest: 0,
            end: 0,
            start: 0,
            stage: Stage::Straight {
                step: 1,
                last: origin,
            },
            explored: None,
            stale: false,
            restarts: 0,
            spent: 0,
        };
        search.nearest = search
            .index(origin.map(|v| v + cfg.cell / 2))
            .ok_or(Refusal::OffLattice)?;
        search.end = search
            .index(to.map(|v| v + cfg.cell / 2))
            .ok_or(Refusal::OffLattice)?;
        search.start = search.nearest;
        Ok(search)
    }
    /// Where the hull stood when the search was asked for.
    pub fn origin(&self) -> [Fx; 2] {
        self.from
    }
    /// A bounded, already checked start-side prefix while the full search
    /// continues. No ground queries or frontier mutations. Live collision
    /// remains authoritative; a stale search must finish its route recheck.
    pub fn prefix_route(&self) -> Option<Route> {
        if self.stale {
            return None;
        }
        let mut path = match &self.stage {
            Stage::Straight { last, .. } => vec![*last],
            Stage::Meet(m) => self.frontier_prefix(&m.reached, &m.open[0])?,
            Stage::Approach(a) => self.frontier_prefix(&a.reached, &a.open)?,
            _ => return None,
        };
        let end = *path.last()?;
        if (0..2).all(|a| (end[a] - self.origin[a]).abs() < 2 * self.cfg.cell) {
            return None;
        }
        if path.first() == Some(&self.origin) {
            path.remove(0);
        }
        Some(Route {
            path: compact(self.origin, path),
            partial: true,
            stranded: false,
            nodes: 0,
            poses: 0,
            scans: 0,
            samples: 0,
            exhausted: false,
        })
    }
    fn frontier_prefix(&self, reached: &Grid, open: &Open) -> Option<Vec<[i32; 2]>> {
        let &Reverse((_, cost, mut node)) = open.peek()?;
        if reached.get(node).distance[0] != cost {
            return None;
        }
        let mut path = Vec::new();
        // Deriving an optional preview must not introduce a graph-sized scan.
        for _ in 0..64 {
            path.push(self.point(node));
            if node == self.start {
                path.reverse();
                return Some(path);
            }
            node = reached.get(node).previous[0] as usize;
        }
        None
    }
    pub fn goal(&self) -> [i32; 2] {
        self.to
    }
    /// Work spent so far, in tiles read and frontier pops.
    pub fn spent(&self) -> u64 {
        self.spent
    }
    /// The most work one corridor test of this search can cost, with the
    /// lattice a test may lead to: an allowance of at least this much
    /// always makes progress. The widest test spans under four cells along
    /// an axis (joining the start to the second node of its route).
    pub fn max_test(&self) -> u64 {
        let radius = self.shape.half[0].min(self.shape.half[1]) + 1;
        let tiles = ((4 * self.cfg.cell + 2 * radius) / 8 + 2) as u64;
        // The goal test also pays the lattice it leads to (ADR-691).
        (tiles * tiles * if self.cfg.local_grade { 3 } else { 1 })
            .saturating_add(self.lattice_cost())
    }
    /// Work of building one dense lattice of reached nodes (ADR-691):
    /// one unit per [`LATTICE_NODES_PER_UNIT`] nodes, at least one.
    pub fn lattice_cost(&self) -> u64 {
        ((self.n * self.rows) as u64)
            .div_ceil(LATTICE_NODES_PER_UNIT)
            .max(1)
    }
    /// Times the search or its route check started over.
    pub fn restarts(&self) -> u32 {
        self.restarts
    }
    // One shared lattice for both directions and all units on the map.
    fn anchor(&self) -> [i32; 2] {
        [self.cfg.cell / 2; 2]
    }
    fn point(&self, i: usize) -> [i32; 2] {
        let anchor = self.anchor();
        [
            (i % self.n) as i32 * self.cfg.cell + anchor[0],
            (i / self.n) as i32 * self.cfg.cell + anchor[1],
        ]
    }
    fn index(&self, p: [i32; 2]) -> Option<usize> {
        let anchor = self.anchor();
        let c = [0, 1].map(|a| (p[a] - anchor[a]).div_euclid(self.cfg.cell));
        self.cell_index(c)
    }
    fn cell_index(&self, c: [i32; 2]) -> Option<usize> {
        (c[0] >= 0 && c[1] >= 0 && c[0] < self.n as i32 && c[1] < self.rows as i32)
            .then(|| c[0] as usize + self.n * c[1] as usize)
    }
    fn estimate(&self, p: [i32; 2], goal: [i32; 2]) -> u32 {
        let d = [
            (p[0] - goal[0]).unsigned_abs(),
            (p[1] - goal[1]).unsigned_abs(),
        ];
        let cost = (10 * d[0].max(d[1]) + 4 * d[0].min(d[1])) * 2;
        match self.cell_shift {
            Some(shift) => cost >> shift,
            None => cost / self.cfg.cell as u32,
        }
    }
    fn heuristic(&self, i: usize, side: usize) -> u32 {
        let goal = if side == 0 { self.to } else { self.origin };
        self.estimate(self.point(i), goal)
    }
    /// Sections of the straight corridor: each check covers the whole
    /// swept box between its endpoints, and comparing the heights of the
    /// entire line would mistake a gradual hill for a cliff.
    fn sections(&self) -> i32 {
        let delta = [self.to[0] - self.origin[0], self.to[1] - self.origin[1]];
        delta[0]
            .unsigned_abs()
            .max(delta[1].unsigned_abs())
            .div_ceil(self.cfg.cell as u32)
            .max(1) as i32
    }
    fn section(&self, step: i32) -> [i32; 2] {
        let steps = self.sections();
        let delta = [self.to[0] - self.origin[0], self.to[1] - self.origin[1]];
        [0, 1].map(|axis| {
            self.origin[axis] + (i64::from(delta[axis]) * i64::from(step) / i64::from(steps)) as i32
        })
    }
    /// Note what changed in the world since the last slice. A change where
    /// the search has read marks its answer for a recheck; a change on a
    /// leg already rechecked sends the recheck back to that leg.
    pub fn note(&mut self, changed: &Changed) {
        if changed.is_empty() {
            return;
        }
        let radius = self.shape.half[0].min(self.shape.half[1]) + 1;
        if let Stage::Recheck(r) = &mut self.stage {
            let back = r.proof[..r.next].iter().position(|leg| {
                let [lo, hi] = read_box(leg[0], leg[1], radius);
                changed.touches(lo, hi)
            });
            if let Some(k) = back {
                if self.restarts < self.cfg.restarts {
                    self.restarts += 1;
                    r.next = k;
                }
            }
            return;
        }
        if self
            .explored
            .is_some_and(|[lo, hi]| changed.touches(lo, hi))
        {
            self.stale = true;
        }
    }
    /// Walk on with at most `budget` work. The answer once the search ends,
    /// and the work this call spent.
    pub fn advance(
        &mut self,
        world: &World,
        rubble: &Rubble,
        budget: u64,
    ) -> (Option<Result<Route, Refusal>>, u64) {
        let (answer, _, spent) = self.advance_mode(world, rubble, budget, false);
        (answer, spent)
    }
    /// As advance, with one optional checked shortcut into a start prefix.
    /// It pays from the same allowance and avoids turning back to a snapped
    /// grid centre when the next forward cell is directly reachable.
    pub fn advance_with_prefix(
        &mut self,
        world: &World,
        rubble: &Rubble,
        budget: u64,
    ) -> (Option<Result<Route, Refusal>>, Option<Route>, u64) {
        self.advance_mode(world, rubble, budget, true)
    }
    fn advance_mode(
        &mut self,
        world: &World,
        rubble: &Rubble,
        budget: u64,
        preview: bool,
    ) -> (Option<Result<Route, Refusal>>, Option<Route>, u64) {
        let mut walk = Walk {
            ground: Ground::new(world, rubble, self.shape, self.cfg),
            meter: Meter { budget, spent: 0 },
            explored: self.explored,
        };
        let mut prefix = if preview { self.prefix_route() } else { None };
        if let Some(route) = &mut prefix {
            if route.path.len() > 1 {
                let [first, next] = [route.path[0], route.path[1]];
                // A compacted run may be arbitrarily long. Join only its
                // first grid step, retaining the bounded atomic test price.
                let join = [0, 1].map(|a| {
                    first[a]
                        + (next[a] - first[a]).signum()
                            * self.cfg.cell.min((next[a] - first[a]).abs())
                });
                match walk.test(self.origin, join) {
                    Some(true) if join == next => {
                        route.path.remove(0);
                    }
                    Some(true) => route.path[0] = join,
                    Some(false) => {} // The checked snapped entry is necessary.
                    None => prefix = None,
                }
            }
        }
        let out = loop {
            match self.step(&mut walk) {
                Step::Continue => {}
                Step::Pause => break None,
                Step::Done(answer) => break Some(answer),
            }
        };
        self.explored = walk.explored;
        self.spent = self.spent.saturating_add(walk.meter.spent);
        // A straight prefix needs no extra joining test; it can be issued
        // in the first slice in which it becomes available.
        if preview && prefix.is_none() && matches!(self.stage, Stage::Straight { .. }) {
            prefix = self.prefix_route();
        }
        (out, prefix, walk.meter.spent)
    }
    fn restart(&mut self) {
        self.restarts += 1;
        self.start = self.nearest;
        self.explored = None;
        self.stale = false;
        self.stage = Stage::Straight {
            step: 1,
            last: self.origin,
        };
    }
    /// A found route, rechecked first when the world changed under it.
    fn conclude(&mut self, route: Route, proof: Vec<[[i32; 2]; 2]>) -> Step {
        if !self.stale {
            return Step::Done(Ok(route));
        }
        self.stale = false;
        self.stage = Stage::Recheck(Box::new(Recheck {
            route,
            proof,
            next: 0,
        }));
        Step::Continue
    }
    fn step(&mut self, walk: &mut Walk<'_>) -> Step {
        let stage = std::mem::replace(&mut self.stage, Stage::Goal);
        match stage {
            Stage::Straight { mut step, mut last } => {
                let steps = self.sections();
                while step <= steps {
                    let next = self.section(step);
                    match walk.test(last, next) {
                        None => {
                            self.stage = Stage::Straight { step, last };
                            return Step::Pause;
                        }
                        Some(false) => {
                            self.stage = self.snap();
                            return Step::Continue;
                        }
                        Some(true) => {}
                    }
                    last = next;
                    step += 1;
                }
                let mut proof = Vec::with_capacity(steps as usize);
                let mut last = self.origin;
                for step in 1..=steps {
                    let next = self.section(step);
                    proof.push([last, next]);
                    last = next;
                }
                self.conclude(
                    Route {
                        path: vec![self.to],
                        partial: false,
                        stranded: false,
                        nodes: 0,
                        poses: 0,
                        scans: 0,
                        samples: 0,
                        exhausted: false,
                    },
                    proof,
                )
            }
            Stage::Snap {
                candidates,
                mut next,
            } => {
                while let Some(&i) = candidates.get(next) {
                    match walk.test(self.origin, self.point(i)) {
                        None => {
                            self.stage = Stage::Snap { candidates, next };
                            return Step::Pause;
                        }
                        Some(true) => {
                            self.start = i;
                            self.stage = Stage::Goal;
                            return Step::Continue;
                        }
                        Some(false) => next += 1,
                    }
                }
                // No corridor leaves the start. Every one of them includes
                // the box around the hull itself, so an obstacle the hull
                // already stands beside refuses them all. A node with clear
                // standing room is an entry the oriented planner can lead
                // the hull to (ADR-691): `next` walks the same candidates a
                // second time, nearest the goal first, ties by distance
                // from the hull.
                let mut toward: Vec<(u32, usize, usize)> = candidates
                    .iter()
                    .enumerate()
                    .map(|(k, &i)| (self.estimate(self.point(i), self.to), k, i))
                    .collect();
                toward.sort_unstable();
                while let Some(&(_, _, i)) = next
                    .checked_sub(candidates.len())
                    .and_then(|k| toward.get(k))
                {
                    let at = self.point(i);
                    match walk.test(at, at) {
                        None => {
                            self.stage = Stage::Snap { candidates, next };
                            return Step::Pause;
                        }
                        Some(true) => return Step::Done(Err(Refusal::Entry { at })),
                        Some(false) => next += 1,
                    }
                }
                Step::Done(Err(Refusal::StartCorridor))
            }
            // Either answer builds a dense lattice: the two frontiers, or
            // the approach after a closed goal corridor (ADR-691).
            Stage::Goal => match walk.test_then(self.point(self.end), self.to, self.lattice_cost())
            {
                None => {
                    self.stage = Stage::Goal;
                    Step::Pause
                }
                Some(false) => {
                    self.stage = self.approach(Refusal::GoalCorridor, 0);
                    Step::Continue
                }
                Some(true) => {
                    let mut meet = Meet {
                        reached: Grid::new(self.n * self.rows),
                        open: [Open::new(), Open::new()],
                        expanded: 0,
                        node: None,
                    };
                    for (side, root) in [self.start, self.end].into_iter().enumerate() {
                        let mut r = meet.reached.get(root);
                        r.distance[side] = 0;
                        meet.reached.set(root, r);
                        meet.open[side].push(Reverse((self.heuristic(root, side), 0, root)));
                    }
                    self.stage = Stage::Meet(Box::new(meet));
                    Step::Continue
                }
            },
            Stage::Meet(mut meet) => {
                let step = self.meet(walk, &mut meet);
                if matches!(step, Step::Pause) {
                    self.stage = Stage::Meet(meet);
                }
                step
            }
            Stage::Approach(mut approach) => {
                let step = self.walk_approach(walk, &mut approach);
                if matches!(step, Step::Pause) {
                    self.stage = Stage::Approach(approach);
                }
                step
            }
            Stage::Found(found) => {
                // Join the first grid leg directly where possible, without
                // a needless backwards move to the snapped start node.
                let joined = if found.path.len() > 1 {
                    match walk.test(self.origin, found.path[1]) {
                        None => {
                            self.stage = Stage::Found(found);
                            return Step::Pause;
                        }
                        Some(clear) => clear,
                    }
                } else {
                    false
                };
                let Found {
                    mut path,
                    partial,
                    nodes,
                } = *found;
                if joined || path.first() == Some(&self.origin) {
                    path.remove(0);
                }
                let mut proof = Vec::with_capacity(path.len());
                let mut last = self.origin;
                for &p in &path {
                    proof.push([last, p]);
                    last = p;
                }
                let compact = compact(self.origin, path);
                if compact.len() > 6400 {
                    return Step::Done(Err(Refusal::TooLong));
                }
                self.conclude(
                    Route {
                        path: compact,
                        partial,
                        stranded: false,
                        nodes,
                        poses: 0,
                        scans: 0,
                        samples: 0,
                        exhausted: false,
                    },
                    proof,
                )
            }
            Stage::Recheck(mut recheck) => {
                while let Some(&[a, b]) = recheck.proof.get(recheck.next) {
                    match walk.test(a, b) {
                        None => {
                            self.stage = Stage::Recheck(recheck);
                            return Step::Pause;
                        }
                        Some(true) => recheck.next += 1,
                        Some(false) if self.restarts < self.cfg.restarts => {
                            self.restart();
                            return Step::Continue;
                        }
                        // Out of restarts: the route stands as found, and
                        // the hull's live collision check governs it.
                        Some(false) => break,
                    }
                }
                Step::Done(Ok(recheck.route))
            }
        }
    }
    /// A hull in a cleared pad may have no clear corridor to its nearest
    /// node: take the nearest node within two cells that has one, by
    /// distance and then index.
    fn snap(&self) -> Stage {
        let c = [
            (self.nearest % self.n) as i32,
            (self.nearest / self.n) as i32,
        ];
        let mut candidates: Vec<(i64, usize)> = (-2..=2)
            .flat_map(|dz| (-2..=2).map(move |dx| [c[0] + dx, c[1] + dz]))
            .filter(|q| q[0] >= 0 && q[1] >= 0 && q[0] < self.n as i32 && q[1] < self.rows as i32)
            .map(|q| {
                let i = q[0] as usize + self.n * q[1] as usize;
                let at = self.point(i);
                let d = [0, 1].map(|a| i64::from(at[a] - self.origin[a]));
                (d[0] * d[0] + d[1] * d[1], i)
            })
            .collect();
        candidates.sort_unstable();
        Stage::Snap {
            candidates: candidates.into_iter().map(|(_, i)| i).collect(),
            next: 0,
        }
    }
    // Go as close as the corridor allows (analysis 240, C5). A goal whose
    // node has no clear corridor to it, or whose side of the lattice is
    // enclosed for this shape, used to refuse the whole route, and the hull
    // stood while the budgeted fine search crossed the map. Now a one-sided
    // search from the start returns a partial route to the reached node
    // nearest the goal, ties by index: the hull drives there, and the fine
    // search covers the rest. A node not a full cell nearer the goal than
    // the hull itself is no progress, and the refusal stands: the snapped
    // start node may lie behind the hull. `heuristic` counts twenty per
    // cell along an axis. The search shares the plan's node allowance with
    // any two-sided search that ran before it.
    fn approach(&self, refusal: Refusal, spent: usize) -> Stage {
        let mut reached = Grid::new(self.n * self.rows);
        let mut root = UNREACHED;
        root.distance[0] = 0;
        reached.set(self.start, root);
        let mut open = Open::new();
        open.push(Reverse((self.heuristic(self.start, 0), 0u32, self.start)));
        Stage::Approach(Box::new(Approach {
            refusal,
            allowance: self.cfg.nodes.saturating_sub(spent),
            reached,
            open,
            best: (self.heuristic(self.start, 0), self.start),
            expanded: 0,
            node: None,
        }))
    }
    /// Test the edges of a node from `e.dir` on, relaxing its frontier.
    /// `false` when the allowance ran out; `e.dir` then names the edge.
    fn expand(
        &self,
        walk: &mut Walk<'_>,
        reached: &mut Grid,
        open: &mut Open,
        e: &mut Expanding,
    ) -> bool {
        let cell = [(e.node % self.n) as i32, (e.node / self.n) as i32];
        let at = cell.map(|c| c * self.cfg.cell + self.cfg.cell / 2);
        let goal = if e.side == 0 { self.to } else { self.origin };
        while e.dir < DIR.len() {
            let d = DIR[e.dir];
            let q = [at[0] + d[0] * self.cfg.cell, at[1] + d[1] * self.cfg.cell];
            // q is exactly one lattice step away. Avoid dividing its world
            // coordinates back to cells, and retain the full boundary check.
            if let Some(j) = self.cell_index([cell[0] + d[0], cell[1] + d[1]]) {
                let next = e.cost + if d[0] != 0 && d[1] != 0 { 14 } else { 10 };
                let old = reached.get(j);
                if next < old.distance[e.side] {
                    match walk.test(at, q) {
                        None => return false,
                        Some(false) => {}
                        Some(true) => {
                            let mut r = old;
                            r.distance[e.side] = next;
                            r.previous[e.side] = e.node as u32;
                            reached.set(j, r);
                            // q is point(j), already computed for the corridor.
                            open.push(Reverse((next + self.estimate(q, goal), next, j)));
                        }
                    }
                }
            }
            e.dir += 1;
        }
        true
    }
    fn meet(&mut self, walk: &mut Walk<'_>, m: &mut Meet) -> Step {
        loop {
            if let Some(mut e) = m.node {
                let done = self.expand(walk, &mut m.reached, &mut m.open[e.side], &mut e);
                m.node = (!done).then_some(e);
                if !done {
                    return Step::Pause;
                }
            }
            // Alternate the two frontiers under one shared deterministic
            // cap. A remote obstacle cannot monopolize a one-sided greedy
            // search. A dry frontier hands over to the approach, whose
            // lattice is paid with this pop (ADR-691).
            let side = m.expanded % 2;
            let lattice = if m.open[side].is_empty() {
                self.lattice_cost()
            } else {
                0
            };
            if !walk.meter.take(1 + lattice) {
                return Step::Pause;
            }
            let Some(Reverse((_, cost, i))) = m.open[side].pop() else {
                // Either frontier running dry means the two never meet;
                // which one ran dry first only says which side is smaller.
                let expanded = m.expanded;
                self.stage = self.approach(Refusal::Enclosed { side, expanded }, expanded);
                return Step::Continue;
            };
            let r = m.reached.get(i);
            if cost != r.distance[side] {
                continue;
            }
            if r.distance[1 - side] != FAR {
                let at = |j: usize| m.reached.get(j);
                let mut path = Vec::new();
                let mut j = i;
                while j != self.start {
                    path.push(self.point(j));
                    j = at(j).previous[0] as usize;
                }
                path.push(self.point(self.start));
                path.reverse();
                j = i;
                while j != self.end {
                    j = at(j).previous[1] as usize;
                    path.push(self.point(j));
                }
                if path.last() != Some(&self.to) {
                    path.push(self.to);
                }
                self.stage = Stage::Found(Box::new(Found {
                    path,
                    partial: false,
                    nodes: m.expanded as u32,
                }));
                return Step::Continue;
            }
            if m.expanded >= self.cfg.nodes {
                return Step::Done(Err(Refusal::Budget));
            }
            m.expanded += 1;
            m.node = Some(Expanding {
                side,
                node: i,
                cost,
                dir: 0,
            });
        }
    }
    fn walk_approach(&mut self, walk: &mut Walk<'_>, a: &mut Approach) -> Step {
        loop {
            if let Some(mut e) = a.node {
                let done = self.expand(walk, &mut a.reached, &mut a.open, &mut e);
                a.node = (!done).then_some(e);
                if !done {
                    return Step::Pause;
                }
            }
            if !walk.meter.take(1) {
                return Step::Pause;
            }
            let Some(Reverse((_, cost, i))) = a.open.pop() else {
                break;
            };
            let r = a.reached.get(i);
            if cost != r.distance[0] {
                continue;
            }
            a.best = a.best.min((self.heuristic(i, 0), i));
            if a.expanded >= a.allowance {
                break;
            }
            a.expanded += 1;
            a.node = Some(Expanding {
                side: 0,
                node: i,
                cost,
                dir: 0,
            });
        }
        let from_hull = self.estimate(self.origin, self.to);
        if a.best.1 == self.start || a.best.0 + 20 > from_hull {
            return Step::Done(Err(a.refusal));
        }
        let mut path = Vec::new();
        let mut j = a.best.1;
        while j != self.start {
            path.push(self.point(j));
            j = a.reached.get(j).previous[0] as usize;
        }
        path.push(self.point(self.start));
        path.reverse();
        self.stage = Stage::Found(Box::new(Found {
            path,
            partial: true,
            nodes: a.expanded as u32,
        }));
        Step::Continue
    }
}

/// Merge straight runs of a joined chain.
fn compact(origin: [i32; 2], path: Vec<[i32; 2]>) -> Vec<[i32; 2]> {
    let mut compact: Vec<[i32; 2]> = Vec::new();
    let mut last = origin;
    for (k, &p) in path.iter().enumerate() {
        if let Some(&next) = path.get(k + 1) {
            let a = [p[0] - last[0], p[1] - last[1]];
            let b = [next[0] - p[0], next[1] - p[1]];
            if i64::from(a[0]) * i64::from(b[1]) == i64::from(a[1]) * i64::from(b[0])
                && i64::from(a[0]) * i64::from(b[0]) + i64::from(a[1]) * i64::from(b[1]) > 0
            {
                continue;
            }
        }
        compact.push(p);
        last = p;
    }
    compact
}

/// Only the nodes a frontier reached, in node order.
fn save_reached(b: &mut fx::blob::Blob, reached: &Grid) {
    let entries: Vec<(usize, &Reached)> = reached
        .0
        .iter()
        .enumerate()
        .filter(|(_, r)| **r != UNREACHED)
        .collect();
    b.u32(entries.len() as u32);
    for (i, r) in entries {
        b.u32(i as u32);
        for v in r.distance.into_iter().chain(r.previous) {
            b.u32(v);
        }
    }
}
fn load_reached(s: &mut fx::blob::Scan, cells: usize) -> Option<Grid> {
    let count = s.u32() as usize;
    if count > cells || count > s.left() / 20 {
        return None;
    }
    let mut reached = Grid::new(cells);
    let mut last = None;
    for _ in 0..count {
        let i = s.u32() as usize;
        let r = Reached {
            distance: [s.u32(), s.u32()],
            previous: [s.u32(), s.u32()],
        };
        if i >= cells
            || last.is_some_and(|l| i <= l)
            || r == UNREACHED
            || r.previous.iter().any(|&p| p != FAR && p as usize >= cells)
        {
            return None;
        }
        last = Some(i);
        reached.set(i, r);
    }
    Some(reached)
}
// Heap storage order is an implementation detail; entries are distinct, so
// the order they leave in is a function of the set. Persist priority order.
fn save_open(b: &mut fx::blob::Blob, open: &Open) {
    let mut entries: Vec<(u32, u32, usize)> = open.iter().map(|Reverse(v)| *v).collect();
    entries.sort_unstable();
    b.u32(entries.len() as u32);
    for (f, cost, i) in entries {
        b.u32(f);
        b.u32(cost);
        b.u32(i as u32);
    }
}
fn load_open(s: &mut fx::blob::Scan, cells: usize) -> Option<Open> {
    let count = s.u32() as usize;
    if count > s.left() / 12 {
        return None;
    }
    let mut open = Open::with_capacity(count);
    for _ in 0..count {
        let entry = (s.u32(), s.u32(), s.u32() as usize);
        if entry.2 >= cells {
            return None;
        }
        open.push(Reverse(entry));
    }
    Some(open)
}
fn save_node(b: &mut fx::blob::Blob, node: Option<Expanding>) {
    b.bool(node.is_some());
    if let Some(e) = node {
        b.u8(e.side as u8);
        b.usize(e.node);
        b.u32(e.cost);
        b.u8(e.dir as u8);
    }
}
fn load_node(s: &mut fx::blob::Scan, cells: usize) -> Option<Option<Expanding>> {
    if !s.bool() {
        return Some(None);
    }
    let e = Expanding {
        side: s.u8() as usize,
        node: s.usize(),
        cost: s.u32(),
        dir: s.u8() as usize,
    };
    (e.side < 2 && e.node < cells && e.dir <= DIR.len()).then_some(Some(e))
}
fn save_points(b: &mut fx::blob::Blob, points: &[[i32; 2]]) {
    b.u32(points.len() as u32);
    for p in points {
        b.i32(p[0]);
        b.i32(p[1]);
    }
}
fn load_points(s: &mut fx::blob::Scan, size: [u32; 2], most: usize) -> Option<Vec<[i32; 2]>> {
    let count = s.u32() as usize;
    if count > most || count > s.left() / 8 {
        return None;
    }
    let points: Vec<[i32; 2]> = (0..count).map(|_| [s.i32(), s.i32()]).collect();
    points
        .iter()
        .all(|p| (0..2).all(|a| (0..size[a] as i32).contains(&p[a])))
        .then_some(points)
}
impl Search {
    /// The search as bytes: the request, how far it got, and what its
    /// frontiers reached. The work it spent is a count of reads, not
    /// state, and is left out. A search read back from these bytes walks
    /// on exactly as the saved one would.
    pub fn save(&self, b: &mut fx::blob::Blob) {
        let s = self.shape;
        for v in [
            s.half[0],
            s.half[1],
            s.support[0],
            s.support[1],
            s.step,
            s.height,
            s.span,
        ] {
            b.i32(v);
        }
        for v in [s.clearance, s.min_up, s.ramp] {
            b.fx(v);
        }
        b.i32(self.cfg.cell);
        b.usize(self.cfg.nodes);
        b.u16(self.cfg.yielding[0]);
        b.u16(self.cfg.yielding[1]);
        b.bool(self.cfg.local_grade);
        b.u32(self.cfg.restarts);
        b.fx(self.from[0]);
        b.fx(self.from[1]);
        b.i32(self.to[0]);
        b.i32(self.to[1]);
        b.usize(self.start);
        b.bool(self.stale);
        b.u32(self.restarts);
        b.bool(self.explored.is_some());
        if let Some([lo, hi]) = self.explored {
            for v in [lo[0], lo[1], hi[0], hi[1]] {
                b.i32(v);
            }
        }
        b.u8(self.stage.tag());
        match &self.stage {
            Stage::Straight { step, last } => {
                b.i32(*step);
                b.i32(last[0]);
                b.i32(last[1]);
            }
            Stage::Snap { candidates, next } => {
                b.u32(candidates.len() as u32);
                for &i in candidates {
                    b.usize(i);
                }
                b.usize(*next);
            }
            Stage::Goal => {}
            Stage::Meet(m) => {
                save_reached(b, &m.reached);
                save_open(b, &m.open[0]);
                save_open(b, &m.open[1]);
                b.usize(m.expanded);
                save_node(b, m.node);
            }
            Stage::Approach(a) => {
                match a.refusal {
                    Refusal::Enclosed { side, expanded } => {
                        b.u8(1);
                        b.u8(side as u8);
                        b.usize(expanded);
                    }
                    _ => b.u8(0),
                }
                b.usize(a.allowance);
                save_reached(b, &a.reached);
                save_open(b, &a.open);
                b.u32(a.best.0);
                b.usize(a.best.1);
                b.usize(a.expanded);
                save_node(b, a.node);
            }
            Stage::Found(f) => {
                save_points(b, &f.path);
                b.bool(f.partial);
                b.u32(f.nodes);
            }
            Stage::Recheck(r) => {
                save_points(b, &r.route.path);
                b.bool(r.route.partial);
                b.u32(r.route.nodes);
                b.u32(r.proof.len() as u32);
                for leg in &r.proof {
                    for p in leg {
                        b.i32(p[0]);
                        b.i32(p[1]);
                    }
                }
                b.usize(r.next);
            }
        }
    }
    /// A search read back for a world of `size` columns. `None` when the
    /// bytes do not describe one.
    pub fn load(s: &mut fx::blob::Scan, size: [u32; 2]) -> Option<Self> {
        let i = [
            s.i32(),
            s.i32(),
            s.i32(),
            s.i32(),
            s.i32(),
            s.i32(),
            s.i32(),
        ];
        let shape = Shape {
            half: [i[0], i[1]],
            support: [i[2], i[3]],
            step: i[4],
            height: i[5],
            span: i[6],
            clearance: s.fx(),
            min_up: s.fx(),
            ramp: s.fx(),
        };
        let cfg = Config {
            cell: s.i32(),
            nodes: s.usize(),
            yielding: [s.u16(), s.u16()],
            local_grade: s.bool(),
            restarts: s.u32(),
        };
        let from = [s.fx(), s.fx()];
        let to = [s.i32(), s.i32()];
        if !s.ok()
            || shape.half.iter().any(|&h| !(0..=4096).contains(&h))
            || !(8..=4096).contains(&cfg.cell)
            || cfg.nodes > 1 << 24
            || from
                .iter()
                .zip(size)
                .any(|(&v, n)| v < Fx::ZERO || v >= Fx::from_int(n as i32))
        {
            return None;
        }
        let mut search = Self::new(size, shape, from, to, cfg).ok()?;
        let cells = search.n * search.rows;
        search.start = s.usize();
        search.stale = s.bool();
        search.restarts = s.u32();
        search.explored = if s.bool() {
            let [a, b, c, d] = [s.i32(), s.i32(), s.i32(), s.i32()];
            if a > c || b > d {
                return None;
            }
            Some([[a, b], [c, d]])
        } else {
            None
        };
        if !s.ok() || search.start >= cells || search.restarts > cfg.restarts {
            return None;
        }
        search.stage = match s.u8() {
            0 => {
                let step = s.i32();
                let last = [s.i32(), s.i32()];
                if !(1..=search.sections() + 1).contains(&step) {
                    return None;
                }
                Stage::Straight { step, last }
            }
            1 => {
                let count = s.u32() as usize;
                if count > 25 {
                    return None;
                }
                let candidates: Vec<usize> = (0..count).map(|_| s.usize()).collect();
                let next = s.usize();
                // Past the candidates, `next` walks them again for
                // standing room (ADR-691).
                if next > 2 * count || candidates.iter().any(|&i| i >= cells) {
                    return None;
                }
                Stage::Snap { candidates, next }
            }
            2 => Stage::Goal,
            3 => {
                let reached = load_reached(s, cells)?;
                let open = [load_open(s, cells)?, load_open(s, cells)?];
                let expanded = s.usize();
                let node = load_node(s, cells)?;
                if expanded > cfg.nodes {
                    return None;
                }
                Stage::Meet(Box::new(Meet {
                    reached,
                    open,
                    expanded,
                    node,
                }))
            }
            4 => {
                let refusal = match s.u8() {
                    0 => Refusal::GoalCorridor,
                    1 => Refusal::Enclosed {
                        side: s.u8() as usize,
                        expanded: s.usize(),
                    },
                    _ => return None,
                };
                let allowance = s.usize();
                let reached = load_reached(s, cells)?;
                let open = load_open(s, cells)?;
                let best = (s.u32(), s.usize());
                let expanded = s.usize();
                let node = load_node(s, cells)?;
                if allowance > cfg.nodes
                    || expanded > allowance
                    || best.1 >= cells
                    || matches!(refusal, Refusal::Enclosed { side, .. } if side > 1)
                {
                    return None;
                }
                Stage::Approach(Box::new(Approach {
                    refusal,
                    allowance,
                    reached,
                    open,
                    best,
                    expanded,
                    node,
                }))
            }
            5 => Stage::Found(Box::new(Found {
                path: load_points(s, size, cells + 2)?,
                partial: s.bool(),
                nodes: s.u32(),
            })),
            6 => {
                let path = load_points(s, size, 6400)?;
                let partial = s.bool();
                let nodes = s.u32();
                let count = s.u32() as usize;
                if count > cells + 2 || count > s.left() / 16 {
                    return None;
                }
                let proof: Vec<[[i32; 2]; 2]> = (0..count)
                    .map(|_| [[s.i32(), s.i32()], [s.i32(), s.i32()]])
                    .collect();
                let next = s.usize();
                if next > count {
                    return None;
                }
                Stage::Recheck(Box::new(Recheck {
                    route: Route {
                        path,
                        partial,
                        stranded: false,
                        nodes,
                        poses: 0,
                        scans: 0,
                        samples: 0,
                        exhausted: false,
                    },
                    proof,
                    next,
                }))
            }
            _ => return None,
        };
        s.ok().then_some(search)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_neighbors_match_world_round_trips_at_partial_edges() {
        for size in [[8, 8], [33, 17], [257, 131], [1025, 513], [4096, 1024]] {
            for cell in [8, 9, 15, 16, 31, 32, 77, 128] {
                let cfg = Config { cell, ..config() };
                let center = [cell / 2; 2];
                let search = Search::new(size, shape(), center.map(Fx::from_int), center, cfg);
                if size.into_iter().any(|n| n < cell as u32) {
                    assert!(matches!(search, Err(Refusal::OffLattice)));
                    continue;
                }
                let search = search.unwrap();
                for node in 0..search.n * search.rows {
                    let at = search.point(node);
                    let c = [(node % search.n) as i32, (node / search.n) as i32];
                    for d in DIR {
                        let q = [at[0] + d[0] * cell, at[1] + d[1] * cell];
                        let direct = search.cell_index([c[0] + d[0], c[1] + d[1]]);
                        assert_eq!(
                            direct,
                            search.index(q),
                            "size={size:?} cell={cell} node={node} d={d:?}"
                        );
                        if let Some(next) = direct {
                            assert_eq!(q, search.point(next));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn heuristic_shift_preserves_exact_quotients_for_all_cell_profiles() {
        for cell in (8..=256).chain([511, 512, 1023, 1024, 4095, 4096]) {
            let search = Search::new(
                [16384, 16384],
                shape(),
                [4096; 2].map(Fx::from_int),
                [8192; 2],
                Config { cell, ..config() },
            )
            .unwrap();
            let mut state = 0x6d2b79f5u32;
            for _ in 0..256 {
                let mut coordinate = || {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    (state % 32769) as i32 - 16384
                };
                let p = [coordinate(), coordinate()];
                let goal = [coordinate(), coordinate()];
                let d = [0, 1].map(|a| (p[a] - goal[a]).unsigned_abs());
                let expected = (10 * d[0].max(d[1]) + 4 * d[0].min(d[1])) * 2 / cell as u32;
                assert_eq!(
                    search.estimate(p, goal),
                    expected,
                    "cell={cell} {p:?}->{goal:?}"
                );
            }
        }
    }

    // Independent full rectangle scan: no rolling row or early refusal.
    fn reference_clear(g: &Ground<'_>, a: [i32; 2], b: [i32; 2]) -> bool {
        let Some((lo, hi)) = g.bounds(a, b) else {
            return false;
        };
        let mut clear = true;
        let (mut low, mut high) = (u16::MAX, 0);
        for z in lo[1] / 8..=hi[1] / 8 {
            for x in lo[0] / 8..=hi[0] / 8 {
                let [bottom, top] = g.tile(x, z);
                low = low.min(bottom);
                high = high.max(top);
                if g.local_grade {
                    clear &= i32::from(top) - i32::from(bottom) <= g.step;
                    for (nx, nz) in [(x - 1, z), (x, z - 1)] {
                        if nx >= lo[0] / 8 && nz >= lo[1] / 8 {
                            let [nb, nt] = g.tile(nx, nz);
                            clear &= i32::from(top.max(nt)) - i32::from(bottom.min(nb)) <= g.step;
                        }
                    }
                }
            }
        }
        if g.local_grade {
            clear
        } else {
            i32::from(high - low) <= g.step
        }
    }

    #[test]
    fn corridor_rows_match_full_scan_after_refusals_craters_and_debris_changes() {
        struct Terraces;
        impl voxel::Generator for Terraces {
            fn material(&self, x: u32, y: u32, z: u32) -> Mat {
                let floor = 6 + x / 32 + if z >= 128 { 12 } else { 0 };
                if y < floor {
                    2
                } else if (40..88).contains(&x) && y < floor + 8 {
                    4
                } else {
                    AIR
                }
            }
        }
        let mut world = World::new(256, 48, 192, 100000, Box::new(Terraces));
        let mut rubble = Rubble::new();
        let mut accepted = [0; 2];
        let mut refused = [0; 2];
        for stage in 0..3 {
            if stage > 0 {
                for z in 60..76 {
                    for x in 108..124 {
                        for y in 0..16 {
                            world.queue_edit(voxel::Edit::put(x, y, z, AIR));
                        }
                    }
                }
                while world.queued() > 0 {
                    world.flush();
                }
                if stage == 1 {
                    rubble.insert([96, 80], 35);
                } else {
                    rubble.remove(&[96, 80]);
                }
            }
            for local_grade in [false, true] {
                for half in [[2, 3], [10, 5], [30, 30]] {
                    let mut ground = Ground::new(
                        &world,
                        &rubble,
                        Shape { half, ..shape() },
                        Config {
                            local_grade,
                            ..config()
                        },
                    );
                    // Different widths, reverse directions and early refusals
                    // reuse the same scratch; stale row tails must not matter.
                    for k in 0..600 {
                        let a = [(k * 73 % 256) - 2, (k * 41 % 192) - 2];
                        let b = [a[0] + k * 13 % 81 - 40, a[1] + k * 17 % 81 - 40];
                        let expected = reference_clear(&ground, a, b);
                        assert_eq!(
                            ground.clear(a, b),
                            expected,
                            "stage={stage} local={local_grade} half={half:?} {a:?}->{b:?}"
                        );
                        assert_eq!(ground.clear(b, a), expected);
                        // Changing the allowed rise exercises neutral borders
                        // and the full i32 domain without step arithmetic.
                        for step in [i32::MIN, -1, 0, 1, 8, i32::MAX] {
                            ground.step = step;
                            assert_eq!(ground.clear(a, b), reference_clear(&ground, a, b));
                        }
                        ground.step = shape().step;
                        if expected {
                            accepted[usize::from(local_grade)] += 1;
                        } else {
                            refused[usize::from(local_grade)] += 1;
                        }
                    }
                }
            }
        }
        assert!(accepted.into_iter().all(|n| n > 100));
        assert!(refused.into_iter().all(|n| n > 100));
    }
    struct Floor;
    impl voxel::Generator for Floor {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < 6 {
                2
            } else {
                AIR
            }
        }
    }
    fn shape() -> Shape {
        Shape {
            half: [10, 5],
            support: [8, 4],
            clearance: Fx::from_int(4),
            step: 4,
            min_up: Fx::from_ratio(9, 10),
            height: 12,
            span: 8,
            ramp: Fx::from_ratio(3, 5),
        }
    }
    fn config() -> Config {
        Config {
            cell: 16,
            nodes: 8192,
            yielding: [4, 5],
            local_grade: false,
            restarts: 2,
        }
    }
    #[test]
    fn pending_prefixes_are_checked_bounded_and_do_not_change_the_frontier() {
        let mut world = World::new(2048, 32, 768, 65536, Box::new(Floor));
        for z in 1..480 {
            for y in 6..26 {
                world.queue_edit(voxel::Edit::put(640, y, z, 3));
            }
        }
        world.flush();
        world.prepare_navigation([4, 5]);
        let rubble = Rubble::new();
        let from = [128, 128].map(Fx::from_int);
        let mut search = Search::new([2048, 768], shape(), from, [1800, 128], config()).unwrap();
        let mut seen = [false; 2];
        for slice in 0..1000 {
            let budget = if slice < 8 { 32 } else { 512 };
            let (answer, spent) = search.advance(&world, &rubble, budget);
            assert!(spent <= budget);
            if answer.is_some() {
                break;
            }
            let mut before = fx::blob::Blob::new();
            search.save(&mut before);
            if let Some(prefix) = search.prefix_route() {
                seen[usize::from(matches!(search.stage, Stage::Meet(_) | Stage::Approach(_)))] =
                    true;
                assert!(prefix.partial && !prefix.exhausted);
                assert!(prefix.path.len() <= 64);
                let mut ground = Ground::new(&world, &rubble, shape(), config());
                let mut last = from.map(Fx::to_int_trunc);
                for next in prefix.path {
                    assert!(
                        ground.clear(last, next),
                        "unchecked prefix {last:?}->{next:?}"
                    );
                    last = next;
                }
                let mut stale = search.clone();
                let mut changed = Changed::default();
                changed.column(128, 128);
                stale.note(&changed);
                assert!(stale.prefix_route().is_none(), "stale route was published");
            }
            let mut after = fx::blob::Blob::new();
            search.save(&mut after);
            assert_eq!(before.v, after.v, "preview mutated the search");
        }
        assert_eq!(
            seen,
            [true, true],
            "straight and graph previews must be exercised"
        );
    }
    #[test]
    fn a_prefix_joins_forward_without_returning_to_the_snapped_start() {
        let world = World::new(2048, 32, 768, 65536, Box::new(Floor));
        world.prepare_navigation([4, 5]);
        let rubble = Rubble::new();
        let from = [143, 135];
        let mut search = Search::new(
            [2048, 768],
            shape(),
            from.map(Fx::from_int),
            [1800, 135],
            config(),
        )
        .unwrap();
        // A paused frontier whose snapped root is seven units behind the
        // hull. Its four forward edges are all clear on this flat ground.
        let mut reached = Grid::new(search.n * search.rows);
        let root = search.start;
        for step in 0..=4 {
            let mut r = UNREACHED;
            r.distance[0] = 10 * step;
            if step > 0 {
                r.previous[0] = (root + step as usize - 1) as u32;
            }
            reached.set(root + step as usize, r);
        }
        let node = root + 4;
        let mut open = Open::new();
        open.push(Reverse((search.heuristic(node, 0) + 40, 40, node)));
        search.stage = Stage::Meet(Box::new(Meet {
            reached,
            open: [open, Open::new()],
            expanded: 0,
            node: None,
        }));
        assert!(search.prefix_route().unwrap().path[0][0] < from[0]);
        let join = search.point(root + 1);
        let budget = Ground::new(&world, &rubble, shape(), config()).cost(from, join);
        let (answer, prefix, spent) = search.advance_with_prefix(&world, &rubble, budget);
        assert!(answer.is_none());
        assert_eq!(
            spent, budget,
            "preview must pay without lifting the ceiling"
        );
        let prefix = prefix.unwrap();
        assert_eq!(prefix.path[0], join);
        assert!(Ground::new(&world, &rubble, shape(), config()).clear(from, join));
    }
    #[test]
    fn complete_route_crosses_a_kilometre_and_sweeps_around_a_thin_wall() {
        let mut world = World::new(10000, 32, 320, 100000, Box::new(Floor));
        for y in 6..26 {
            for z in 1..200 {
                world.queue_edit(voxel::Edit::put(4800, y, z, 3));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let from = [128, 128].map(Fx::from_int);
        let to = [9800, 128];
        assert!(
            plan(
                &world,
                &Rubble::new(),
                shape(),
                from,
                to,
                Config {
                    nodes: 1,
                    ..config()
                }
            )
            .is_none(),
            "budget must not return a complete route"
        );
        let cold = plan(&world, &Rubble::default(), shape(), from, to, config()).unwrap();
        world.prepare_navigation([4, 5]);
        let warm = plan(&world, &Rubble::default(), shape(), from, to, config()).unwrap();
        assert_eq!(cold.path, warm.path);
        assert_eq!(cold.nodes, warm.nodes);
        assert_eq!(warm.path.last(), Some(&to));
        assert!(!warm.partial);
        let mut ground = Probe::for_search(&world, &[4, 5], Columns::default());
        let mut at = from;
        let mut head = 0;
        for &p in &warm.path {
            let q = p.map(Fx::from_int);
            assert!(
                super::super::connects(&mut ground, shape(), at, head, q, &|_, _| true).is_some(),
                "invalid leg {at:?} -> {q:?}"
            );
            head = angle::atan2(q[1].sub(at[1]).raw(), q[0].sub(at[0]).raw());
            at = q;
        }
    }
    /// A route that goes no farther than `x`: refused, or partial and
    /// wholly on the near side (analysis 240, C5: a blocked goal gives a
    /// partial route as close as the corridor allows, never one across).
    fn stops_before(route: Option<Route>, x: i32) -> bool {
        route.is_none_or(|r| r.partial && r.path.iter().all(|p| p[0] < x))
    }
    /// A partial route that ends before `x`: the goal approached, not refused.
    fn approaches(route: Option<Route>, x: i32) -> bool {
        route.is_some_and(|r| r.partial && r.path.iter().all(|p| p[0] < x))
    }
    fn complete(route: Option<Route>) -> bool {
        route.is_some_and(|r| !r.partial)
    }
    #[test]
    fn tile_aligned_cliffs_and_live_debris_do_not_become_flat_corridors() {
        let mut world = World::new(512, 32, 256, 100000, Box::new(Floor));
        let from = [64, 128].map(Fx::from_int);
        let to = [448, 128];
        for x in 248..264 {
            for z in 0..256 {
                for y in 6..26 {
                    world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        assert!(approaches(
            plan(&world, &Rubble::new(), shape(), from, to, config()),
            248
        ));
        assert!(approaches(
            plan(
                &world,
                &Rubble::new(),
                shape(),
                from,
                to,
                Config {
                    local_grade: true,
                    ..config()
                }
            ),
            248
        ));
        world.restore_pristine();
        assert!(complete(plan(
            &world,
            &Rubble::new(),
            shape(),
            from,
            to,
            config()
        )));
        let rubble = Rubble::from_sorted((0..256).map(|z| ([256, z], 26)).collect());
        assert!(approaches(
            plan(&world, &rubble, shape(), from, to, config()),
            256
        ));
        assert!(complete(plan(
            &world,
            &Rubble::new(),
            shape(),
            from,
            to,
            config()
        )));
    }
    #[test]
    fn articulated_hull_can_route_up_a_gradual_hill() {
        struct Hill;
        impl voxel::Generator for Hill {
            fn material(&self, x: u32, y: u32, _z: u32) -> Mat {
                if y < 6 + x / 8 {
                    2
                } else {
                    AIR
                }
            }
        }
        let world = World::new(512, 96, 256, 100000, Box::new(Hill));
        let broad = Shape {
            half: [50, 50],
            support: [25, 25],
            step: 4,
            ..shape()
        };
        let from = [80, 128].map(Fx::from_int);
        let to = [432, 128];
        assert!(plan(
            &world,
            &Rubble::new(),
            broad,
            from,
            to,
            Config {
                nodes: 1,
                ..config()
            }
        )
        .is_none());
        let route = plan(
            &world,
            &Rubble::new(),
            broad,
            from,
            to,
            Config {
                nodes: 1,
                local_grade: true,
                ..config()
            },
        )
        .expect("a gradual grade is not one giant step for articulated legs");
        assert_eq!(route.path, vec![to]);
    }
    #[test]
    fn sparse_pages_refuse_fresh_debris_on_a_large_map() {
        let world = World::new(20000, 32, 10000, 100000, Box::new(Floor));
        let from = [64, 128].map(Fx::from_int);
        let to = [448, 128];
        let cfg = Config {
            nodes: 1,
            ..config()
        };
        assert_eq!(
            plan(&world, &Rubble::new(), shape(), from, to, cfg)
                .unwrap()
                .path,
            vec![to]
        );
        let rubble = Rubble::from_sorted((0..256).map(|z| ([256, z], 26)).collect());
        assert!(
            plan(&world, &rubble, shape(), from, to, cfg).is_none(),
            "large-map sparse debris was ignored"
        );
    }
    #[test]
    fn sealed_wall_forest_and_opened_gap_have_distinct_answers() {
        let mut world = World::new(512, 32, 256, 100000, Box::new(Floor));
        let from = [64, 128].map(Fx::from_int);
        let to = [448, 128];
        let r = Rubble::default();
        for y in 6..26 {
            for z in 0..256 {
                world.queue_edit(voxel::Edit::put(256, y, z, 4));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        assert!(
            complete(plan(&world, &r, shape(), from, to, config())),
            "forest is crushable"
        );
        for y in 6..26 {
            for z in 0..256 {
                world.queue_edit(voxel::Edit::put(256, y, z, 3));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        assert!(
            stops_before(plan(&world, &r, shape(), from, to, config()), 256),
            "sealed wall crossed"
        );
        // As close as the corridor allows: up to the wall, not refused.
        let near = plan(&world, &r, shape(), from, to, config()).expect("partial route");
        assert!(near.partial);
        assert!(
            near.path.last().is_some_and(|p| p[0] >= 200),
            "{:?}",
            near.path
        );
        for y in 6..26 {
            for z in 96..160 {
                world.queue_edit(voxel::Edit::dig(256, y, z));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        assert!(
            complete(plan(&world, &r, shape(), from, to, config())),
            "fresh hole not seen"
        );
    }
    /// **The cached debris tiles are the tiles a fresh build gives**, on
    /// a dense and a paged map, after every insert and removal, and a route
    /// over cached tiles equals one over a cold rubble (analysis 240, B6).
    #[test]
    fn cached_debris_tiles_follow_every_rubble_change() {
        for size in [[1024u32, 1024], [12_000, 12_000]] {
            let mut rubble = Rubble::default();
            let mut columns: Vec<([i32; 2], u16)> = Vec::new();
            let mut seed = 0x9e37_79b9u32;
            for step in 0..60 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let at = [(seed >> 8) as i32 % 900, (seed >> 20) as i32 % 900];
                if step % 4 == 3 && !columns.is_empty() {
                    let (gone, _) = columns.remove(columns.len() / 2);
                    rubble.remove(&gone);
                } else {
                    let top = 6 + (seed % 9) as u16;
                    columns.retain(|(c, _)| *c != at);
                    columns.push((at, top));
                    columns.sort_unstable();
                    rubble.insert(at, top);
                }
                let cached = rubble.coarse_tiles(size);
                let fresh = DebrisTiles::build(size, &columns);
                for &(at, _) in &columns {
                    let (x, z) = ((at[0] / 8) as usize, (at[1] / 8) as usize);
                    assert_eq!(cached.top(x, z), fresh.top(x, z), "{size:?} step {step}");
                }
                assert_eq!(cached.top(120, 120), fresh.top(120, 120));
            }
        }
        let mut world = World::new(1024, 32, 1024, 100000, Box::new(Floor));
        while world.queued() > 0 {
            world.flush();
        }
        let rubble: Rubble = (0..40).map(|k| ([300 + k, 500], 20)).collect();
        let (from, to) = ([100, 500].map(Fx::from_int), [900, 500]);
        let cold = plan(&world, &rubble.clone(), shape(), from, to, config()).unwrap();
        let warm = plan(&world, &rubble, shape(), from, to, config()).unwrap();
        let again = plan(&world, &rubble, shape(), from, to, config()).unwrap();
        assert_eq!(cold.path, warm.path);
        assert_eq!(warm.path, again.path);
        assert_eq!(cold.nodes, again.nodes);
        assert_ne!(
            cold.path,
            plan(&world, &Rubble::new(), shape(), from, to, config())
                .unwrap()
                .path,
            "the debris wall must shape the route, or this proves nothing"
        );
    }
    /// A pillar in the corridor from the stand to its nearest lattice
    /// node no longer refuses the route: the start snaps to the nearest
    /// node within two cells with a clear corridor (ADR-654). Origin
    /// (100, 128) has nearest node (104, 136); its corridor box spans x
    /// 94..110, z 122..142 and holds the pillar at x 104..110, z 138..142.
    /// Node (104, 120), as near, has a box ending at z 134.
    #[test]
    fn the_start_snaps_past_a_pillar_by_its_nearest_node() {
        let mut world = World::new(512, 32, 512, 100000, Box::new(Floor));
        for x in 104..110 {
            for z in 138..142 {
                for y in 6..26 {
                    world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let from = [100, 128].map(Fx::from_int);
        let route = plan_explained(&world, &Rubble::new(), shape(), from, [100, 400], config());
        assert!(
            route.as_ref().is_ok_and(|r| !r.partial),
            "{:?}",
            route.map(|r| r.path)
        );
    }
    /// A goal sealed in a room gives a partial route up to the room from
    /// afar, and a refusal from beside the room: a node not a full cell
    /// nearer the goal than the start is no progress (ADR-654).
    #[test]
    fn a_sealed_goal_is_approached_from_afar_and_refused_from_beside_it() {
        let mut world = World::new(512, 32, 256, 100000, Box::new(Floor));
        for y in 6..26 {
            for k in 260..=340 {
                for (x, z) in [(k, 88), (k, 168), (260, k - 172), (340, k - 172)] {
                    world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let r = Rubble::new();
        let goal = [300, 128];
        let far = plan_explained(
            &world,
            &r,
            shape(),
            [64, 128].map(Fx::from_int),
            goal,
            config(),
        );
        let far = far.expect("partial route from afar");
        assert!(far.partial);
        assert!(far.path.iter().all(|p| p[0] < 260), "{:?}", far.path);
        assert!(
            far.path.last().is_some_and(|p| p[0] >= 230),
            "{:?}",
            far.path
        );
        let beside = plan_explained(
            &world,
            &r,
            shape(),
            [244, 128].map(Fx::from_int),
            goal,
            config(),
        );
        assert!(beside.is_err(), "{:?}", beside.map(|r| r.path));
    }
    /// Run a search a slice of `budget` at a time, reading it back from
    /// its bytes after every slice: the answer and the work of each call.
    fn sliced(
        world: &World,
        rubble: &Rubble,
        shape: Shape,
        from: [Fx; 2],
        to: [i32; 2],
        cfg: Config,
        budget: u64,
    ) -> (Result<Route, Refusal>, Vec<u64>) {
        let size = [world.nx, world.nz];
        let mut search = match Search::new(size, shape, from, to, cfg) {
            Ok(search) => search,
            Err(refusal) => return (Err(refusal), Vec::new()),
        };
        let mut calls = Vec::new();
        loop {
            let (answer, spent) = search.advance(world, rubble, budget);
            calls.push(spent);
            if let Some(answer) = answer {
                return (answer, calls);
            }
            let mut b = fx::blob::Blob::new();
            search.save(&mut b);
            let mut scan = fx::blob::Scan::new(&b.v);
            let back = Search::load(&mut scan, size).expect("search reads back");
            assert_eq!(scan.left(), 0);
            let mut again = fx::blob::Blob::new();
            back.save(&mut again);
            assert_eq!(b.v, again.v, "bytes of a read-back search differ");
            search = back;
        }
    }
    fn same(a: &Result<Route, Refusal>, b: &Result<Route, Refusal>) -> bool {
        match (a, b) {
            (Ok(a), Ok(b)) => a.path == b.path && a.partial == b.partial && a.nodes == b.nodes,
            (Err(a), Err(b)) => a == b,
            _ => false,
        }
    }
    /// **A search walked a slice per tick finds what one call finds**, on
    /// an unchanged world (ADR-680): routes around a wall, through a gap,
    /// partial up to a sealed room, refusals, for a belt and for a walker
    /// grading locally. Each slice respects its allowance — the largest
    /// single test of these shapes is far below every budget tried — and
    /// a search read back from its bytes between slices walks on the same.
    #[test]
    fn a_sliced_search_finds_the_route_of_one_call() {
        let mut world = World::new(1024, 32, 512, 100000, Box::new(Floor));
        for y in 6..26 {
            for z in 0..512u32 {
                if !(200..260).contains(&z) {
                    world.queue_edit(voxel::Edit::put(400, y, z, 3));
                }
            }
            for k in 700..=780 {
                for (x, z) in [(k, 300), (k, 380), (700, k - 400), (780, k - 400)] {
                    world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let rubble: Rubble = (0..40).map(|k| ([300 + k, 100], 20)).collect();
        let broad = Shape {
            half: [30, 30],
            support: [20, 20],
            ..shape()
        };
        let mut checked = 0;
        for (shape, local_grade) in [(shape(), false), (broad, true)] {
            for (from, to) in [
                ([100, 100], [900, 120]),
                ([100, 450], [740, 340]),
                ([120, 230], [900, 230]),
                ([300, 60], [300, 470]),
                ([740, 340], [100, 100]),
            ] {
                let cfg = Config {
                    local_grade,
                    ..config()
                };
                let from = from.map(Fx::from_int);
                let whole = plan_explained(&world, &rubble, shape, from, to, cfg);
                for budget in [512u64, 2048, 50_000] {
                    let (answer, calls) = sliced(&world, &rubble, shape, from, to, cfg, budget);
                    assert!(
                        same(&whole, &answer),
                        "{from:?} -> {to:?} at {budget}: {:?} vs {:?}",
                        whole.as_ref().map(|r| &r.path),
                        answer.as_ref().map(|r| &r.path)
                    );
                    assert!(
                        calls.iter().all(|&c| c <= budget),
                        "{calls:?} over {budget}"
                    );
                    if budget == 512 && calls.len() > 2 {
                        checked += 1;
                    }
                }
            }
        }
        assert!(
            checked >= 4,
            "too few searches spanned several slices: {checked}"
        );
    }
    /// A slice that spends nothing walks nothing, and the work of all
    /// slices adds up to the work of one call.
    #[test]
    fn slices_add_up_to_the_work_of_one_call() {
        let mut world = World::new(1024, 32, 512, 100000, Box::new(Floor));
        for y in 6..26 {
            for z in 0..400u32 {
                world.queue_edit(voxel::Edit::put(500, y, z, 3));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let r = Rubble::new();
        let (from, to) = ([100, 100].map(Fx::from_int), [900, 100]);
        let mut whole = Search::new([1024, 512], shape(), from, to, config()).unwrap();
        let (answer, total) = whole.advance(&world, &r, u64::MAX);
        assert!(answer.is_some_and(|a| a.is_ok_and(|r| !r.partial)));
        let mut search = Search::new([1024, 512], shape(), from, to, config()).unwrap();
        let (none, spent) = search.advance(&world, &r, 0);
        assert!(none.is_none() && spent == 0);
        let (_, calls) = sliced(&world, &r, shape(), from, to, config(), 1000);
        assert_eq!(calls.iter().sum::<u64>(), total);
        assert!(calls.len() as u64 >= total / 1000);
    }
    /// **A hull pinned by an obstacle beside it gets a lattice entry**
    /// (ADR-691). A wall clips the corner of the box around the hull
    /// itself, so every corridor from where it stands is refused, though
    /// the field is open. The search names the node with clear standing
    /// room nearest the goal, the route from it is whole, and a hull
    /// with no such node near it is refused as before. Sliced and read
    /// back between slices, the second walk of the candidates answers the
    /// same.
    #[test]
    fn a_hull_beside_an_obstacle_is_led_to_a_lattice_entry() {
        let mut world = World::new(1024, 32, 512, 100000, Box::new(Floor));
        for y in 6..26 {
            for x in 325..=328 {
                for z in 190..=222 {
                    world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let r = Rubble::new();
        let broad = Shape {
            half: [30, 30],
            support: [20, 20],
            ..shape()
        };
        let from = [300, 250].map(Fx::from_int);
        let to = [900, 250];
        let answer = plan_explained(&world, &r, broad, from, to, config());
        let Err(Refusal::Entry { at }) = answer else {
            panic!("{answer:?}");
        };
        // Clear standing room, within the two snap cells, toward the goal.
        let box_clear = |p: [i32; 2]| {
            let (lo, hi) = (p.map(|v| v - 31), p.map(|v| v + 31));
            !(lo[0] <= 328 && hi[0] >= 325 && lo[1] <= 222 && hi[1] >= 190)
        };
        assert!(box_clear(at), "{at:?}");
        assert!((at[0] - 300).abs() <= 40 && (at[1] - 250).abs() <= 40);
        let onward = plan_explained(&world, &r, broad, at.map(Fx::from_int), to, config());
        assert!(onward.is_ok_and(|r| !r.partial), "no route on from {at:?}");
        let (sliced_answer, _) = sliced(&world, &r, broad, from, to, config(), 512);
        assert!(same(&answer, &sliced_answer));
        // Boxed in on all sides: no node has room, the refusal stands.
        let mut world = World::new(1024, 32, 512, 100000, Box::new(Floor));
        for y in 6..26 {
            // Narrower than the hull's box: no node near has room.
            for k in 220..=280u32 {
                for (x, z) in [(k, 220), (k, 280), (220, k), (280, k)] {
                    world.queue_edit(voxel::Edit::put(x, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let pinned = plan_explained(
            &world,
            &r,
            broad,
            [250, 250].map(Fx::from_int),
            to,
            config(),
        );
        assert_eq!(pinned.map(|r| r.path), Err(Refusal::StartCorridor));
    }
    /// **The dense lattice is paid in the slice that builds it** (ADR-691):
    /// a slice of exactly the dearest test either stays before the
    /// frontiers or enters them having spent at least the lattice, and
    /// an approach after a dry frontier is paid the same way. Walked so,
    /// the search still answers what one call answers.
    #[test]
    fn the_lattice_is_paid_before_it_is_built() {
        let mut world = World::new(1024, 32, 512, 100000, Box::new(Floor));
        for y in 6..26 {
            // A closed pocket around the start: its frontier runs dry.
            for k in 0..=300u32 {
                world.queue_edit(voxel::Edit::put(200, y, k, 3));
                world.queue_edit(voxel::Edit::put(k.min(200), y, 300, 3));
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let r = Rubble::new();
        let (from, to) = ([100, 100].map(Fx::from_int), [900, 100]);
        let cfg = config();
        let whole = plan_explained(&world, &r, shape(), from, to, cfg);
        let mut search = Search::new([1024, 512], shape(), from, to, cfg).unwrap();
        let lattice = search.lattice_cost();
        assert_eq!(lattice, (64 * 32u64).div_ceil(LATTICE_NODES_PER_UNIT));
        assert!(search.max_test() > lattice);
        let budget = search.max_test();
        let mut built = 0;
        let answer = loop {
            let before = search.stage.tag();
            let (answer, spent) = search.advance(&world, &r, budget);
            assert!(spent <= budget);
            let after = search.stage.tag();
            // Entering the frontiers or the approach builds one lattice.
            if before != after && matches!(after, 3 | 4) {
                assert!(spent >= lattice, "{before}->{after} spent {spent}");
                built += 1;
            }
            if let Some(answer) = answer {
                break answer;
            }
        };
        assert!(built >= 2, "frontiers and approach: {built}");
        assert!(same(&whole, &answer));
    }
    /// **A world change under a search in flight** (ADR-680). The gap the
    /// search is routing through is sealed between two slices: the found
    /// route fails its recheck, the search starts over on the world as it
    /// now is, and answers what one call on that world answers. Without
    /// the change reported the stale route through the gap would stand.
    /// A change away from everything the search read leaves it alone.
    #[test]
    fn a_route_through_a_gap_sealed_mid_search_is_not_handed_out() {
        let mut world = World::new(512, 32, 256, 100000, Box::new(Floor));
        for y in 6..26 {
            for z in 0..256u32 {
                if !(96..160).contains(&z) {
                    world.queue_edit(voxel::Edit::put(256, y, z, 3));
                }
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let r = Rubble::new();
        let (from, to) = ([64, 40].map(Fx::from_int), [448, 40]);
        let open = plan_explained(&world, &r, shape(), from, to, config()).unwrap();
        assert!(!open.partial && open.path.iter().any(|p| (96..160).contains(&p[1])));
        // Walk each search to just short of its answer: it has read the
        // gap and found its way through it.
        let mut whole = Search::new([512, 256], shape(), from, to, config()).unwrap();
        let total = whole.advance(&world, &r, u64::MAX).1;
        let begin = |world: &World| {
            let mut search = Search::new([512, 256], shape(), from, to, config()).unwrap();
            assert!(search.advance(world, &r, total - 1).0.is_none());
            search
        };
        let mut far = begin(&world);
        let mut stale = begin(&world);
        let mut noted = begin(&world);
        let mut changed = Changed::default();
        for y in 6..26 {
            for z in 96..160u32 {
                world.queue_edit(voxel::Edit::put(256, y, z, 3));
                changed.column(256, z as i32);
            }
        }
        while world.queued() > 0 {
            world.flush();
        }
        let sealed = plan_explained(&world, &r, shape(), from, to, config());
        assert!(
            sealed.as_ref().is_ok_and(|s| s.partial),
            "{:?}",
            sealed.map(|s| s.path)
        );
        noted.note(&changed);
        let answer = noted.advance(&world, &r, u64::MAX).0.unwrap();
        assert!(same(&answer, &sealed), "{:?}", answer.map(|a| a.path));
        assert_eq!(noted.restarts(), 1);
        // Not told: the route through the gap stands as found.
        let blind = stale.advance(&world, &r, u64::MAX).0.unwrap().unwrap();
        assert!(!blind.partial, "the fixture must route through the gap");
        // A change nowhere near: no recheck, no restart.
        let mut away = Changed::default();
        away.column(500, 250);
        far.note(&away);
        assert!(far.advance(&world, &r, u64::MAX).0.unwrap().unwrap().path == blind.path);
        assert_eq!(far.restarts(), 0);
    }
}

const PAGE: usize = 32;

/// Highest debris top per 8×8 tile, for the coarse corridor test. Built
/// from the rubble columns once per rubble state (analysis 240, B6): every
/// route used to rebuild it, three megabytes on a kilometre map.
#[derive(Clone, Debug, Default)]
pub struct DebrisTiles {
    width: usize,
    dense: Vec<u16>,
    pages: Vec<Option<Box<[u16; PAGE * PAGE]>>>,
    page_width: usize,
}

impl DebrisTiles {
    pub(crate) fn build(size: [u32; 2], columns: &[([i32; 2], u16)]) -> Self {
        let tiles = [
            (size[0] as usize).div_ceil(8),
            (size[1] as usize).div_ceil(8),
        ];
        // A kilometre map needs only about 3 MiB for direct tile lookups. Keep
        // that fast path, but cap its size: a 10 km map would otherwise allocate
        // hundreds of MiB even with only a few debris columns.
        let dense = !columns.is_empty() && tiles[0] * tiles[1] <= 2_000_000;
        let page_width = tiles[0].div_ceil(PAGE);
        let mut out = Self {
            width: tiles[0],
            dense: if dense {
                vec![0u16; tiles[0] * tiles[1]]
            } else {
                Vec::new()
            },
            pages: if columns.is_empty() || dense {
                Vec::new()
            } else {
                vec![None; page_width * tiles[1].div_ceil(PAGE)]
            },
            page_width,
        };
        for &(p, top) in columns {
            let x = p[0].div_euclid(8);
            let z = p[1].div_euclid(8);
            if x < 0 || z < 0 || x as usize >= tiles[0] || z as usize >= tiles[1] {
                continue;
            }
            let (x, z) = (x as usize, z as usize);
            let pile = if dense {
                &mut out.dense[z * tiles[0] + x]
            } else {
                let page = &mut out.pages[z / PAGE * page_width + x / PAGE];
                let page = page.get_or_insert_with(|| Box::new([0; PAGE * PAGE]));
                &mut page[z % PAGE * PAGE + x % PAGE]
            };
            *pile = (*pile).max(top);
        }
        out
    }
    /// Highest debris top on the tile, zero with none.
    pub(crate) fn top(&self, x: usize, z: usize) -> u16 {
        if !self.dense.is_empty() {
            self.dense[z * self.width + x]
        } else if !self.pages.is_empty() {
            self.pages[z / PAGE * self.page_width + x / PAGE]
                .as_ref()
                .map_or(0, |page| page[z % PAGE * PAGE + x % PAGE])
        } else {
            0
        }
    }
}
