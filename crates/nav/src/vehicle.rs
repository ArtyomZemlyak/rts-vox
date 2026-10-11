//! Oriented vehicle clearance and routes against the current voxel surface.
//! The same footprint query serves planning and accepted movement. This
//! surface adapter does not replace the multi-level navigation graph.
//!
//! Yielding materials (a tree over a tracked hull) are not obstacles: the
//! floor is the terrain beneath them, and the pose reports how many footprint
//! columns hold such matter inside the hull height, so the caller can crush
//! it and the planner can price the detour.
pub mod coarse;
mod connection;
mod rubble_index;
use connection::Connection;
use fx::{angle, Fx};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    sync::Arc,
};
use voxel::{
    store::{Mat, AIR},
    World,
};

/// A footprint and its swept sample poses, prepared independently.
pub type SweepSamples = (Shape, Vec<([Fx; 2], i32)>);

#[derive(Clone, Copy, PartialEq, Eq)]
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
///
/// Columns in ascending order, read by binary search (ADR-465). The map
/// is rebuilt from the pieces every step, and a battlefield holds some
/// fifteen thousand columns: built into a tree they cost more than half
/// a millisecond a step, built from sorted columns at once almost none.
/// A derived bounds/max tree rejects distant columns in rectangle queries;
/// point lookup, iteration and equality use only the canonical sorted slice.
///
/// The coarse planner's debris tile map is derived too, built on first use
/// and shared by every route planned until the rubble changes
/// (analysis 240, B6).
#[derive(Clone, Debug, Default)]
pub struct Rubble(
    Vec<([i32; 2], u16)>,
    rubble_index::Index,
    std::sync::OnceLock<([u32; 2], std::sync::Arc<coarse::DebrisTiles>)>,
);
impl Rubble {
    pub fn new() -> Self {
        Self::default()
    }
    /// From columns in strictly ascending order, as a sorted and deduplicated
    /// gather gives them.
    pub fn from_sorted(columns: Vec<([i32; 2], u16)>) -> Self {
        debug_assert!(columns.windows(2).all(|p| p[0].0 < p[1].0));
        let mut index = rubble_index::Index::default();
        index.rebuild(&columns);
        Self(columns, index, Default::default())
    }
    pub fn get(&self, at: &[i32; 2]) -> Option<&u16> {
        self.0
            .binary_search_by(|(c, _)| c.cmp(at))
            .ok()
            .map(|i| &self.0[i].1)
    }
    pub fn contains_key(&self, at: &[i32; 2]) -> bool {
        self.get(at).is_some()
    }
    /// Set the top of a column, returning the one it replaced.
    pub fn insert(&mut self, at: [i32; 2], top: u16) -> Option<u16> {
        let old = self.insert_column(at, top);
        self.1.rebuild(&self.0);
        self.2 = Default::default();
        old
    }
    fn insert_column(&mut self, at: [i32; 2], top: u16) -> Option<u16> {
        match self.0.binary_search_by(|(c, _)| c.cmp(&at)) {
            Ok(i) => Some(std::mem::replace(&mut self.0[i].1, top)),
            Err(i) => {
                self.0.insert(i, (at, top));
                None
            }
        }
    }
    /// Clear a column, returning its top.
    pub fn remove(&mut self, at: &[i32; 2]) -> Option<u16> {
        let i = self.0.binary_search_by(|(c, _)| c.cmp(at)).ok()?;
        let old = self.0.remove(i).1;
        self.1.rebuild(&self.0);
        self.2 = Default::default();
        Some(old)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Whether resting debris can affect a box of terrain columns. The
    /// columns are sorted by X then Z; a dense X strip elsewhere along Z
    /// must not force a wide hull into an exact footprint scan.
    pub fn touches(&self, lo: [i32; 2], hi: [i32; 2]) -> bool {
        if (0..2).any(|a| lo[a] > hi[a]) {
            return false;
        }
        if !self.1.is_empty() {
            return self.1.touches(&self.0, lo, hi);
        }
        let first = self.0.partition_point(|(at, _)| at[0] < lo[0]);
        let rest = &self.0[first..];
        let count = rest.partition_point(|(at, _)| at[0] <= hi[0]);
        rest[..count]
            .iter()
            .any(|(at, _)| at[1] >= lo[1] && at[1] <= hi[1])
    }
    /// Exact upper bound of debris inside a terrain box. A pile below the
    /// lowest belly cannot invalidate a bare-terrain clearance proof.
    pub fn upper_in(&self, lo: [i32; 2], hi: [i32; 2]) -> u16 {
        if (0..2).any(|a| lo[a] > hi[a]) {
            return 0;
        }
        if !self.1.is_empty() {
            return self.1.upper(&self.0, lo, hi, &mut || {});
        }
        let first = self.0.partition_point(|(at, _)| at[0] < lo[0]);
        let rest = &self.0[first..];
        let count = rest.partition_point(|(at, _)| at[0] <= hi[0]);
        rest[..count]
            .iter()
            .filter(|(at, _)| at[1] >= lo[1] && at[1] <= hi[1])
            .map(|(_, top)| *top)
            .max()
            .unwrap_or(0)
    }
    /// Columns and tops in ascending column order.
    pub fn iter(&self) -> impl Iterator<Item = (&[i32; 2], &u16)> {
        self.0.iter().map(|(c, t)| (c, t))
    }
    /// The coarse planner's debris tiles for a map of `size` columns:
    /// built once per rubble state and size, a pure function of both.
    pub(crate) fn coarse_tiles(&self, size: [u32; 2]) -> std::sync::Arc<coarse::DebrisTiles> {
        if let Some((built, tiles)) = self.2.get() {
            if *built == size {
                return tiles.clone();
            }
            return std::sync::Arc::new(coarse::DebrisTiles::build(size, &self.0));
        }
        self.2
            .get_or_init(|| {
                (
                    size,
                    std::sync::Arc::new(coarse::DebrisTiles::build(size, &self.0)),
                )
            })
            .1
            .clone()
    }
}
/// A later entry for a column replaces an earlier one, as insertion does.
impl FromIterator<([i32; 2], u16)> for Rubble {
    fn from_iter<I: IntoIterator<Item = ([i32; 2], u16)>>(items: I) -> Self {
        let mut r = Self::new();
        for (at, top) in items {
            r.insert_column(at, top);
        }
        r.1.rebuild(&r.0);
        r
    }
}
impl<const N: usize> From<[([i32; 2], u16); N]> for Rubble {
    fn from(items: [([i32; 2], u16); N]) -> Self {
        items.into_iter().collect()
    }
}
impl<'a> IntoIterator for &'a Rubble {
    type Item = (&'a [i32; 2], &'a u16);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, ([i32; 2], u16)>,
        fn(&'a ([i32; 2], u16)) -> (&'a [i32; 2], &'a u16),
    >;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(c, t)| (c, t))
    }
}
/// Columns that changed under a pending search, by cache page (ADR-427):
/// the pages a drain's applied edits touched and those where the rubble
/// map differs from the last step's. A search keeps what it cached
/// elsewhere and restarts only a check whose checked part lies on one of
/// them. `all` stands for a change nobody located.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Changed {
    all: bool,
    pages: BTreeSet<[i32; 2]>,
}
impl Changed {
    /// A change somewhere on the map, place unknown.
    pub fn everywhere() -> Self {
        Self {
            all: true,
            pages: BTreeSet::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        !self.all && self.pages.is_empty()
    }
    /// The page holding column `(x, z)` changed.
    pub fn column(&mut self, x: i32, z: i32) {
        self.pages.insert(page_of(x, z));
    }
    /// The pages holding these columns changed. A run of columns on one
    /// page (a drain's edits) asks the set once.
    pub fn columns(&mut self, columns: impl IntoIterator<Item = [i32; 2]>) {
        let mut last = None;
        for [x, z] in columns {
            let page = page_of(x, z);
            if last != Some(page) {
                self.pages.insert(page);
                last = Some(page);
            }
        }
    }
    pub fn merge(&mut self, other: &Changed) {
        self.all |= other.all;
        self.pages.extend(other.pages.iter().copied());
    }
    fn page(&self, key: [i32; 2]) -> bool {
        self.all || self.pages.contains(&key)
    }
    /// Whether a change meets the box of columns `lo..=hi`.
    pub fn touches(&self, lo: [i32; 2], hi: [i32; 2]) -> bool {
        if self.all {
            return true;
        }
        let (a, b) = (page_of(lo[0], lo[1]), page_of(hi[0], hi[1]));
        self.pages
            .range([a[0], i32::MIN]..=[b[0], i32::MAX])
            .any(|p| (a[1]..=b[1]).contains(&p[1]))
    }
    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.bool(self.all);
        b.u32(self.pages.len() as u32);
        for p in &self.pages {
            b.i32(p[0]);
            b.i32(p[1]);
        }
    }
    /// `size` is the world in columns; a page off it is refused.
    pub fn load(s: &mut fx::blob::Scan, size: [u32; 2]) -> Option<Self> {
        let all = s.bool();
        let across = pages_across(size);
        let count = s.u32() as usize;
        if count > across[0] as usize * across[1] as usize {
            return None;
        }
        let mut pages = BTreeSet::new();
        for _ in 0..count {
            let p = [s.i32(), s.i32()];
            if !(0..2).all(|a| (0..across[a]).contains(&p[a])) || !pages.insert(p) {
                return None;
            }
        }
        s.ok().then_some(Self { all, pages })
    }
}
/// Prints of a rubble map by cache page (ADR-427): FNV-1a over the page's
/// columns in key order. Two maps whose prints differ on a page differ
/// there; equal prints are equal columns but for a collision, which reads
/// the same in every run. A few words a page, small enough for a snapshot,
/// where the map itself is not.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct RubblePrint(BTreeMap<[i32; 2], u64>);
impl RubblePrint {
    /// Columns off the `size` of the world are left out: no probe reads
    /// them.
    ///
    /// The map runs in column order, so the pages of one band of
    /// `PAGE_SIDE` columns along X fill together: their prints are kept
    /// by the band's page along Z and handed on when the band ends, in
    /// page order, each page hashing its columns in the map's order as
    /// before (ADR-465). A lookup in a tree per column cost a third of a
    /// millisecond a step on a battlefield.
    pub fn of(rubble: &Rubble, size: [u32; 2]) -> Self {
        const SEED: u64 = 0xcbf2_9ce4_8422_2325;
        let mut band = vec![SEED; pages_across(size)[1].max(0) as usize];
        let mut touched: Vec<usize> = Vec::new();
        let mut pages: Vec<([i32; 2], u64)> = Vec::new();
        let mut current = None;
        let mut hand_on = |bx: i32, band: &mut [u64], touched: &mut Vec<usize>| {
            touched.sort_unstable();
            for &bz in touched.iter() {
                pages.push(([bx, bz as i32], std::mem::replace(&mut band[bz], SEED)));
            }
            touched.clear();
        };
        for (&[x, z], &top) in rubble {
            if x < 0 || z < 0 || x >= size[0] as i32 || z >= size[1] as i32 {
                continue;
            }
            let [bx, bz] = page_of(x, z);
            if current != Some(bx) {
                if let Some(done) = current {
                    hand_on(done, &mut band, &mut touched);
                }
                current = Some(bx);
            }
            let bz = bz as usize;
            if band[bz] == SEED && !touched.contains(&bz) {
                touched.push(bz);
            }
            let h = &mut band[bz];
            for byte in x
                .to_le_bytes()
                .into_iter()
                .chain(z.to_le_bytes())
                .chain(top.to_le_bytes())
            {
                *h = (*h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        if let Some(done) = current {
            hand_on(done, &mut band, &mut touched);
        }
        Self(pages.into_iter().collect())
    }
    /// Mark in `changed` every page where `self` and `next` differ.
    pub fn changes(&self, next: &Self, changed: &mut Changed) {
        for (key, print) in &self.0 {
            if next.0.get(key) != Some(print) {
                changed.pages.insert(*key);
            }
        }
        for key in next.0.keys() {
            if !self.0.contains_key(key) {
                changed.pages.insert(*key);
            }
        }
    }
    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.u32(self.0.len() as u32);
        for (p, print) in &self.0 {
            b.i32(p[0]);
            b.i32(p[1]);
            b.u64(*print);
        }
    }
    pub fn load(s: &mut fx::blob::Scan, size: [u32; 2]) -> Option<Self> {
        let across = pages_across(size);
        let count = s.u32() as usize;
        if count > across[0] as usize * across[1] as usize {
            return None;
        }
        let mut pages = BTreeMap::new();
        for _ in 0..count {
            let p = [s.i32(), s.i32()];
            let print = s.u64();
            if !(0..2).all(|a| (0..across[a]).contains(&p[a])) || pages.insert(p, print).is_some() {
                return None;
            }
        }
        s.ok().then_some(Self(pages))
    }
}
fn page_of(x: i32, z: i32) -> [i32; 2] {
    [x, z].map(|v| v.div_euclid(PAGE_SIDE as i32))
}
fn pages_across(size: [u32; 2]) -> [i32; 2] {
    size.map(|n| n.div_ceil(PAGE_SIDE as u32).min(i32::MAX as u32) as i32)
}
/// How far from a pose's position the columns it reads can lie, per axis:
/// the footprint or the supports, whichever reaches further, the contact
/// run along the heading, the corner patch (no wider than the step) and
/// the neighbour a surface sample blends in.
fn reach(s: Shape) -> i32 {
    s.half[0].max(s.support[0]) + s.half[1].max(s.support[1]) + s.span.max(0) + s.step.max(0) + 2
}
/// Cached column answers for a suspended search. Pages are bounded and moved
/// into the next probe; none survives a change on it or enters a save.
#[derive(Clone, Debug, Default)]
pub struct Columns {
    pages: Vec<Option<Arc<ColumnPage>>>,
    replacement: Vec<u8>,
}
/// Transient exact geometry, with worker reads accounted separately from
/// logical search work. No answers or cache pages enter a saved search.
#[derive(Default)]
pub struct PreparedSweep {
    pub poses: Vec<(([Fx; 2], i32), Pose)>,
    pub columns: Option<Columns>,
    pub reads: [u64; 2],
}
/// Exact terrain samples of a freshly started direct connection. Its initial
/// query establishes height; later poses must also satisfy clearance and step.
pub struct StartSweep {
    pub shape: Shape,
    pub samples: Vec<([Fx; 2], i32)>,
    height: Fx,
    initial: bool,
}
pub type PrepareStart<'a> = dyn FnMut(StartSweep, &mut Probe<'_>) -> PreparedSweep + 'a;
impl StartSweep {
    pub fn prefix_open(&self, poses: &[Pose]) -> bool {
        let mut height = self.height;
        let mut steep = None;
        for (i, pose) in poses.iter().enumerate() {
            if !(i == 0 && self.initial)
                && (pose.blocked != 0
                    || pose.tilt
                    || pose.steep && steep == Some(false)
                    || pose.height.sub(height).abs() > Fx::from_int(self.shape.step))
            {
                return false;
            }
            height = pose.height;
            steep = Some(pose.steep);
        }
        true
    }
}
impl Columns {
    /// Drop the pages a change touched; the rest still answer for the
    /// world as it is (ADR-427).
    fn forget(&mut self, changed: &Changed) {
        for slot in &mut self.pages {
            if slot.as_ref().is_some_and(|p| changed.page(p.key)) {
                *slot = None;
            }
        }
    }
}
// Four-way set-associative pages bound a search cache independently of map or route size.
// Eviction only repeats a pure query, never changes the search's work allowance.
const PAGE_SIDE: usize = 32;
const PAGE_SLOTS: usize = 128;
#[derive(Clone, Debug)]
struct ColumnPage {
    key: [i32; 2],
    values: Vec<(u16, [u16; 2])>,
}
pub struct Probe<'a> {
    world: &'a World,
    pages: Vec<Option<Arc<ColumnPage>>>,
    replacement: Vec<u8>,
    last_page: usize,
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
    heights: Vec<u16>,
    /// Lowest and highest yielding cell above the floor; `[MAX, 0]` is none.
    /// Empty when nothing yields.
    yields: Vec<[u16; 2]>,
    /// Hump interpolation for the current support length and fade. A probe
    /// may check many poses of one hull; these weights do not depend on pose.
    hump_weights: Vec<(Fx, Fx)>,
    hump_key: (i32, i32),
    /// Grade depends on the hull's tilt limit, not its pose or the terrain.
    grade_cache: Option<(Fx, Fx)>,
    prepared_routes: BTreeMap<(Fx, Fx, i32), Pose>,
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
            pages: Vec::new(),
            replacement: Vec::new(),
            last_page: 0,
            x0: lo[0],
            z0: lo[1],
            w,
            h,
            // A prepared flat pose needs no columns at all. Allocate the
            // dense cache only if an exact column query actually follows.
            heights: Vec::new(),
            yields: Vec::new(),
            hump_weights: Vec::new(),
            hump_key: (0, 0),
            grade_cache: None,
            prepared_routes: BTreeMap::new(),
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
    /// Bounded cache for long searches, moved between slices without clearing
    /// or allocating a dense window over the remaining distance.
    pub fn for_search(world: &'a World, yielding: &[Mat], mut columns: Columns) -> Self {
        if columns.pages.is_empty() {
            columns.pages = (0..PAGE_SLOTS).map(|_| None).collect();
            columns.replacement = vec![0; PAGE_SLOTS / 4];
        }
        let mut probe = Self::window(world, yielding, [0; 2], [0; 2]);
        probe.pages = columns.pages;
        probe.replacement = columns.replacement;
        probe
    }
    /// Reuse exact local column answers across motion ticks. Both world
    /// edits and rubble changes invalidate their affected pages first.
    pub fn for_local(
        world: &'a World,
        yielding: &[Mat],
        mut columns: Columns,
        changed: &Changed,
    ) -> Self {
        columns.forget(changed);
        Self::for_search(world, yielding, columns)
    }
    /// Transfer search pages without allocating or clearing a replacement.
    pub fn take_columns(&mut self) -> Columns {
        Columns {
            pages: std::mem::take(&mut self.pages),
            replacement: std::mem::take(&mut self.replacement),
        }
    }
    /// Independent scratch sharing only immutable, copy-on-write page answers.
    pub fn fork_route(&self) -> Probe<'a> {
        let columns = Columns {
            pages: self.pages.clone(),
            replacement: self.replacement.clone(),
        };
        let mut fork = Self::for_search(self.world, &self.yielding, columns);
        fork.rubble = self.rubble;
        fork
    }
    pub fn yields(&self, m: Mat) -> bool {
        self.yielding.contains(&m)
    }
    /// Whether cached column answers use this exact yielding-material set.
    pub fn matches_yielding(&self, materials: &[Mat]) -> bool {
        self.yielding == materials
    }
    fn scan(&self, x: i32, z: i32) -> (u16, [u16; 2]) {
        let mut floor = 0;
        let mut span = [u16::MAX, 0];
        let ceiling = self.world.column_ceiling(x as u32, z as u32);
        // The resident 8x8 navigation tile gives the exact floor of every
        // column when its lower and upper bounds agree. If this column's
        // ceiling is that floor, it cannot hide yielding matter above it.
        if let [a, b] = self.yielding.as_slice() {
            if self
                .world
                .prepared_floor_tile(x as u32 / 8, z as u32 / 8, [*a, *b])
                .is_some_and(|bounds| bounds[0] == bounds[1] && u32::from(bounds[0]) == ceiling)
            {
                floor = ceiling as u16;
                if let Some(top) = self.rubble.and_then(|r| r.get(&[x, z])) {
                    floor = floor.max(*top);
                }
                return (floor, span);
            }
        }
        // The walk starts at the column's own ceiling, not at the sky
        // (ADR-313): above it there is nothing, and on the range that was
        // a hundred and seventy-four empty cells every time.
        for y in (0..ceiling).rev() {
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
        if !self.pages.is_empty() {
            let key = [x / PAGE_SIDE as i32, z / PAGE_SIDE as i32];
            let slot = if self.pages[self.last_page]
                .as_ref()
                .is_some_and(|p| p.key == key)
            {
                self.last_page
            } else {
                let bucket = ((key[0] as usize).wrapping_mul(73_856_093)
                    ^ (key[1] as usize).wrapping_mul(19_349_663))
                    % (PAGE_SLOTS / 4);
                let base = bucket * 4;
                let slot = (base..base + 4)
                    .find(|&i| self.pages[i].as_ref().is_some_and(|p| p.key == key))
                    .unwrap_or_else(|| {
                        let i = base + usize::from(self.replacement[bucket]);
                        self.replacement[bucket] = (self.replacement[bucket] + 1) % 4;
                        if let Some(page) = self.pages[i].as_mut().and_then(Arc::get_mut) {
                            page.key = key;
                            page.values.fill((u16::MAX, [u16::MAX, 0]));
                        } else {
                            self.pages[i] = Some(Arc::new(ColumnPage {
                                key,
                                values: vec![(u16::MAX, [u16::MAX, 0]); PAGE_SIDE * PAGE_SIDE],
                            }));
                        }
                        i
                    });
                self.last_page = slot;
                slot
            };
            let index = x as usize % PAGE_SIDE + (z as usize % PAGE_SIDE) * PAGE_SIDE;
            let value = self.pages[slot].as_ref().unwrap().values[index];
            if value.0 != u16::MAX {
                return Some(value);
            }
            self.scans += 1;
            let value = self.scan(x, z);
            Arc::make_mut(self.pages[slot].as_mut().unwrap()).values[index] = value;
            return Some(value);
        }
        let (dx, dz) = (x - self.x0, z - self.z0);
        if dx < 0 || dz < 0 || dx as usize >= self.w || dz as usize >= self.h {
            return Some(self.scan(x, z));
        }
        if self.heights.is_empty() {
            self.heights = vec![u16::MAX; self.w * self.h];
            if !self.yielding.is_empty() {
                self.yields = vec![[u16::MAX, 0]; self.w * self.h];
            }
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
    /// Every yielding cell inside the hull at this pose, in column order.
    /// This is the cut a tracked hull makes when it stands here.
    pub fn crushed(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> Vec<[u32; 3]> {
        self.crushed_limit(pos, head, s, usize::MAX)
    }
    /// Ordered cut prefix for a bounded fragment pool. Frame cells do not
    /// count toward `limit`: the pool cannot claim them, but the caller still
    /// receives them in their original positions before the prefix ends.
    pub fn crushed_limit(
        &mut self,
        pos: [Fx; 2],
        head: i32,
        s: Shape,
        limit: usize,
    ) -> Vec<[u32; 3]> {
        let mut out = Vec::new();
        if self.yielding.is_empty() || limit == 0 {
            return out;
        }
        let mut claimable = 0usize;
        let (c, sin) = (angle::cos(head), angle::sin(head));
        let (lo, hi) = self.extent(pos, head, s);
        for z in lo[1]..=hi[1] {
            for (x, _, _) in Self::row(pos, c, sin, s, lo[0], hi[0], z) {
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
                        let p = [x as u32, y as u32, z as u32];
                        if limit != usize::MAX && !self.world.on_frame(p[0], p[1], p[2]) {
                            if claimable == limit {
                                return out;
                            }
                            claimable += 1;
                        }
                        out.push(p);
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
    #[cfg(test)]
    fn inside(&self, pos: [Fx; 2], c: Fx, sin: Fx, s: Shape, x: i32, z: i32) -> bool {
        let dx = Fx::from_int(x).add(Fx::from_ratio(1, 2)).sub(pos[0]);
        let dz = Fx::from_int(z).add(Fx::from_ratio(1, 2)).sub(pos[1]);
        let u = dx.mul(c).add(dz.mul(sin));
        let v = dz.mul(c).sub(dx.mul(sin));
        u.abs() <= Fx::from_int(s.half[0]).add(Fx::from_ratio(1, 2))
            && v.abs() <= Fx::from_int(s.half[1]).add(Fx::from_ratio(1, 2))
    }
    /// Moving one whole world cell adds exactly `c` / `-sin` to the
    /// fixed-point local coordinates, including the original rounding.
    /// Reuse those coordinates for clearance and avoid transforming twice.
    fn row(
        pos: [Fx; 2],
        c: Fx,
        sin: Fx,
        s: Shape,
        lo: i32,
        hi: i32,
        z: i32,
    ) -> impl Iterator<Item = (i32, Fx, Fx)> {
        let half = Fx::from_ratio(1, 2);
        let dx = Fx::from_int(lo).add(half).sub(pos[0]);
        let dz = Fx::from_int(z).add(half).sub(pos[1]);
        let mut u = dx.mul(c).add(dz.mul(sin));
        let mut v = dz.mul(c).sub(dx.mul(sin));
        let bounds = s.half.map(|n| Fx::from_int(n).add(half));
        (lo..=hi).filter_map(move |x| {
            let local = (u, v);
            u = u.add(c);
            v = v.sub(sin);
            (local.0.abs() <= bounds[0] && local.1.abs() <= bounds[1])
                .then_some((x, local.0, local.1))
        })
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
        // A zero interpolation weight makes that neighbour irrelevant.
        // Grid-aligned supports are common in the search; keep the exact
        // arithmetic for fractional positions without reading unused rows.
        let a = if u == Fx::ZERO {
            h00
        } else {
            let h10 = Fx::from_int(self.floor(x + 1, z));
            h00.add(h10.sub(h00).mul(u))
        };
        if v == Fx::ZERO {
            return a;
        }
        let h01 = Fx::from_int(self.floor(x, z + 1));
        let b = if u == Fx::ZERO {
            h01
        } else {
            let h11 = Fx::from_int(self.floor(x + 1, z + 1));
            h01.add(h11.sub(h01).mul(u))
        };
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
        // Only two reductions are needed after the bridging level is known:
        // the highest ramp-adjusted support and the minimum level that would
        // make every sample climbable. No per-contact sample allocation.
        let mut required_level = Fx::MIN;
        let mut level = base;
        let step = Fx::from_int(s.step);
        // A point support sees no grade: a belt shorter than two units has
        // no line to sample.
        for k in 1..=(if s.span >= 2 { s.span } else { 0 }) {
            let d = Fx::from_int(k);
            let ramp_drop = s.ramp.mul(d);
            let climb = step.max(grade.mul(d));
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
                height = height.max(there.sub(ramp_drop));
                required_level = required_level.max(there.sub(climb));
            }
            // Bridging: the belt between neighbouring wheels lies on the
            // ground either side of a hollow narrower than half the contact.
            // A wider hollow is followed, the wheel drops into it.
            if let (Some(a), Some(b), true) = (pair[0], pair[1], 2 * k <= s.span) {
                level = level.max(a.min(b));
            }
        }
        height = height.max(level);
        let mut steep = required_level > level;
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
        (height, steep)
    }
    pub fn support_pose(&mut self, pos: [Fx; 2], head: i32, mut s: Shape) -> Pose {
        // Contact belongs to the footprint. Looking beyond the track's
        // end makes a neighbouring wall lift even a parked vehicle.
        // Route clearance still looks ahead over the full approach span.
        s.span = s.span.min((s.half[0] - s.support[0]).max(0));
        self.pose(pos, head, s)
    }
    /// Suspension only needs the four contact floors, not another full hull
    /// clearance scan. Movement and planning still use the complete pose.
    pub fn support_floors(&mut self, pos: [Fx; 2], head: i32, mut s: Shape) -> [Fx; 4] {
        s.span = s.span.min((s.half[0] - s.support[0]).max(0));
        if let Some(pose) = self.flat_pose(pos, s) {
            return pose.floors;
        }
        self.contacts(pos, head, s).0
    }
    fn contacts(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> ([Fx; 4], bool) {
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
        let grade = match self.grade_cache {
            Some((key, grade)) if key == m => grade,
            _ => {
                let grade = Fx::ONE.sub(m.mul(m)).max(Fx::ZERO).sqrt().div(m);
                self.grade_cache = Some((m, grade));
                grade
            }
        };
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
            let support = s.support[0];
            if self.hump_key != (support, fade) {
                self.hump_weights.clear();
                for u in -support + 1..support {
                    let t = Fx::from_int(u + support).div(Fx::from_int(2 * support.max(1)));
                    let w = Fx::from_int((support - u.abs()).min(fade)).div(Fx::from_int(fade));
                    self.hump_weights.push((t, w));
                }
                self.hump_key = (support, fade);
            }
            for (rear, front, z) in [(0, 1, -1), (2, 3, 1)] {
                let mut lift = Fx::ZERO;
                let mut top = Fx::ZERO;
                let mut u = -s.support[0] + 1;
                while u < s.support[0] {
                    let (t, w) = self.hump_weights[(u + support - 1) as usize];
                    let g = self.surface(point(u, z * s.support[1]));
                    let line = floors[rear].add(floors[front].sub(floors[rear]).mul(t));
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
        (floors, steep)
    }
    pub fn pose(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> Pose {
        if let Some(pose) = self.flat_pose(pos, s) {
            return pose;
        }
        self.probe_pose(pos, head, s, false)
    }
    /// A recovery step may retain existing belly contacts while crossing a
    /// fraction of a column, but must not acquire a new blocked column.
    /// Exact subset check; a matching count alone cannot establish this.
    pub fn keeps_belly_contacts(
        &mut self,
        from: ([Fx; 2], i32, Pose),
        to: ([Fx; 2], i32, Pose),
        s: Shape,
    ) -> bool {
        if to.2.blocked == 0 {
            return true;
        }
        if from.2.blocked == 0 || to.2.blocked > from.2.blocked {
            return false;
        }
        let plane = |p: Pose| {
            [
                p.height,
                p.floors[1]
                    .add(p.floors[3])
                    .sub(p.floors[0])
                    .sub(p.floors[2])
                    .div(Fx::from_int(4 * s.support[0].max(1))),
                p.floors[2]
                    .add(p.floors[3])
                    .sub(p.floors[0])
                    .sub(p.floors[1])
                    .div(Fx::from_int(4 * s.support[1].max(1))),
            ]
        };
        let (source, target) = (plane(from.2), plane(to.2));
        let clearance = s.clearance.max(Fx::from_int(s.step));
        let belly =
            |p: [Fx; 3], u: Fx, v: Fx| p[0].add(p[1].mul(u)).add(p[2].mul(v)).add(clearance);
        let (c0, s0) = (angle::cos(from.1), angle::sin(from.1));
        let (c1, s1) = (angle::cos(to.1), angle::sin(to.1));
        let (lo, hi) = self.extent(to.0, to.1, s);
        for z in lo[1]..=hi[1] {
            for (x, u, v) in Self::row(to.0, c1, s1, s, lo[0], hi[0], z) {
                let floor = Fx::from_int(self.floor(x, z));
                if floor <= belly(target, u, v) {
                    continue;
                }
                let Some((_, u, v)) = Self::row(from.0, c0, s0, s, x, x, z).next() else {
                    return false;
                };
                if floor <= belly(source, u, v) {
                    return false;
                }
            }
        }
        true
    }
    /// Planning only consumes the full depth/crush counts of legal poses.
    /// An illegal pose may stop at the first proof of refusal. Execution
    /// still uses `pose`: decreasing penetration must retain exact counts.
    fn route_pose(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> Pose {
        self.route_pose_mode(pos, head, s, false)
    }
    fn route_pose_mode(&mut self, pos: [Fx; 2], head: i32, s: Shape, leave_steep: bool) -> Pose {
        if let Some(pose) = self.prepared_routes.get(&(pos[0], pos[1], head)) {
            // Ordinary speculative poses may stop before scanning the belly
            // on a steep contact. An exit must validate that clearance too.
            if !leave_steep || !pose.steep || pose.tilt || pose.blocked != 0 {
                return *pose;
            }
        }
        if let Some(pose) = self.flat_pose(pos, s) {
            return pose;
        }
        self.probe_pose_cost(pos, head, s, true, true, leave_steep)
    }
    fn contact_steep(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> bool {
        self.flat_pose(pos, s)
            .map_or_else(|| self.contacts(pos, head, s).1, |p| p.steep)
    }
    /// Immutable-world geometry for one speculative sweep batch. Logical
    /// progress and blocker callbacks remain with the owning search.
    pub fn route_samples(&mut self, s: Shape, samples: &[([Fx; 2], i32)]) -> Vec<Pose> {
        samples
            .iter()
            .map(|&(at, head)| self.route_pose(at, head, s))
            .collect()
    }
    /// Ordered speculative prefix, stopping at the first failed connection
    /// proof. The caller still commits samples with its original work budget;
    /// geometry beyond a known refusal would only be discarded.
    pub fn route_prefix(
        &mut self,
        s: Shape,
        samples: &[([Fx; 2], i32)],
        open: impl Fn(&[Pose]) -> bool,
    ) -> Vec<Pose> {
        let mut poses = Vec::with_capacity(samples.len());
        for &(at, head) in samples {
            poses.push(self.route_pose(at, head, s));
            if !open(&poses) {
                break;
            }
        }
        poses
    }
    /// The prepared tiles prove the complete support and belly footprint is
    /// one bare height. The proof is the same for driving and route search.
    fn flat_pose(&self, pos: [Fx; 2], s: Shape) -> Option<Pose> {
        // A prepared tile box can prove that every contact sample and every
        // footprint column sees the same bare floor. Do not repeat hundreds
        // of contact and belly probes for a turn on level, empty ground.
        if s.ramp >= Fx::ZERO && s.step >= 0 && s.clearance >= Fx::ZERO && s.min_up.abs() <= Fx::ONE
        {
            if let [a, b] = self.yielding.as_slice() {
                let c = pos.map(Fx::to_int_trunc);
                let r = reach(s);
                let lo = [c[0] - r, c[1] - r];
                let hi = [c[0] + r, c[1] + r];
                if !self.rubble.is_some_and(|rubble| rubble.touches(lo, hi)) {
                    if let Some(h) = self.world.prepared_flat_area(lo, hi, [*a, *b]) {
                        let h = Fx::from_int(i32::from(h));
                        return Some(Pose {
                            height: h,
                            floors: [h; 4],
                            blocked: 0,
                            steep: false,
                            tilt: false,
                            crush: 0,
                        });
                    }
                }
            }
        }
        None
    }
    /// Goal-ring probes consume only refusal, never vegetation cost. Contact
    /// floors remain exact; a prepared terrain bound can omit crush counting.
    fn goal_refused(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> bool {
        if let Some(pose) = self.flat_pose(pos, s) {
            return pose.refused();
        }
        self.probe_pose_cost(pos, head, s, true, false, false)
            .refused()
    }
    fn probe_pose(&mut self, pos: [Fx; 2], head: i32, s: Shape, stop_on_refusal: bool) -> Pose {
        self.probe_pose_cost(pos, head, s, stop_on_refusal, true, false)
    }
    fn probe_pose_cost(
        &mut self,
        pos: [Fx; 2],
        head: i32,
        s: Shape,
        stop_on_refusal: bool,
        count_crush: bool,
        leave_steep: bool,
    ) -> Pose {
        let (floors, steep) = self.contacts(pos, head, s);
        let (c, sin) = (angle::cos(head), angle::sin(head));
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
        let mut result = Pose {
            height,
            floors,
            blocked: 0,
            steep,
            tilt,
            crush: 0,
        };
        if stop_on_refusal && (tilt || steep && !leave_steep) {
            return result;
        }
        let (lo, hi) = self.extent(pos, head, s);
        let clearance = s.clearance.max(Fx::from_int(s.step));
        // Every local coordinate of the rotated footprint lies inside these
        // bounds. Its minimum belly is at the corner opposing each tilt.
        let u = Fx::from_int(s.half[0]).add(Fx::from_ratio(1, 2));
        let v = Fx::from_int(s.half[1]).add(Fx::from_ratio(1, 2));
        let u = if pitch < Fx::ZERO { u } else { Fx::ZERO.sub(u) };
        let v = if roll < Fx::ZERO { v } else { Fx::ZERO.sub(v) };
        let min_belly = height.add(pitch.mul(u)).add(roll.mul(v)).add(clearance);
        let no_rubble = !self.rubble.is_some_and(|rubble| rubble.touches(lo, hi));
        let profile = match self.yielding.as_slice() {
            [a, b] => Some([*a, *b]),
            _ => None,
        };
        let upper = |lo, hi, profile| {
            if count_crush {
                self.world.prepared_bare_upper(lo, hi, profile)
            } else {
                self.world.prepared_floor_upper(lo, hi, profile)
            }
        };
        if let Some(profile) = profile {
            if let Some(top) = upper(lo, hi, profile) {
                if Fx::from_int(i32::from(top)) <= min_belly
                    && (no_rubble
                        || self.rubble.is_none_or(|r| {
                            Fx::from_int(i32::from(r.upper_in(lo, hi))) <= min_belly
                        }))
                {
                    return result;
                }
            }
        }
        // One tree or damaged tile must not force a wide articulated hull
        // to read every otherwise bare column. Tile proofs may omit only
        // columns with neither yielding matter nor rubble, whose highest
        // floor is below even the lowest point of the hull's belly.
        let clear_tiles = if let Some(profile) = profile.filter(|_| {
            lo[0] >= 0
                && lo[1] >= 0
                && hi[0] < self.world.nx as i32
                && hi[1] < self.world.nz as i32
                && i64::from(hi[0] - lo[0] + 1) * i64::from(hi[1] - lo[1] + 1) >= 4096
        }) {
            let first = [lo[0] / 8, lo[1] / 8];
            let last = [hi[0] / 8, hi[1] / 8];
            let width = (last[0] - first[0] + 1) as usize;
            let mut clear = Vec::with_capacity(width * (last[1] - first[1] + 1) as usize);
            for z in first[1]..=last[1] {
                for x in first[0]..=last[0] {
                    let a = [x * 8, z * 8];
                    let b = [
                        (x * 8 + 7).min(self.world.nx as i32 - 1),
                        (z * 8 + 7).min(self.world.nz as i32 - 1),
                    ];
                    // A heap under one part of a broad hull only invalidates
                    // that tile's proof, not every bare tile of the footprint.
                    let top = upper(a, b, profile);
                    clear.push(
                        top.is_some_and(|h| Fx::from_int(i32::from(h)) <= min_belly)
                            && (no_rubble
                                || self.rubble.is_none_or(|r| {
                                    Fx::from_int(i32::from(r.upper_in(a, b))) <= min_belly
                                })),
                    );
                }
            }
            Some((first, width, clear))
        } else {
            None
        };
        // A level hull has one belly plane. Flat ground dominates range
        // searches; avoid two fixed-point products per footprint column.
        let level = pitch == Fx::ZERO && roll == Fx::ZERO;
        let hull_height = Fx::from_int(s.height);
        for z in lo[1]..=hi[1] {
            // A proved clear tile needs no footprint transforms either.
            // Start each remaining run at its exact local coordinate; moving
            // by whole columns adds an integral multiple of c / -sin, with
            // the same fixed-point rounding as the unsplit row.
            let (first, last) = clear_tiles
                .as_ref()
                .map_or((0, 0), |(first, _, _)| (first[0], hi[0] / 8));
            let runs = (first..=last).filter_map(|tile| {
                if let Some((first, width, clear)) = &clear_tiles {
                    let i = (z / 8 - first[1]) as usize * width + (tile - first[0]) as usize;
                    (!clear[i]).then_some((lo[0].max(tile * 8), hi[0].min(tile * 8 + 7)))
                } else {
                    Some((lo[0], hi[0]))
                }
            });
            for (x, u, v) in runs.flat_map(|(a, b)| Self::row(pos, c, sin, s, a, b, z)) {
                // Floor and yielding matter come from the same column answer.
                // A second lookup used to repeat the page/cache query for
                // every cell of every route pose, even without any trees.
                let (floor, low) = self
                    .column(x, z)
                    .map(|(floor, [low, _])| (i32::from(floor), low))
                    .unwrap_or((self.world.ny as i32, u16::MAX));
                let ground = Fx::from_int(floor);
                let local = if level {
                    height
                } else {
                    height.add(pitch.mul(u)).add(roll.mul(v))
                };
                let belly = local.add(clearance);
                if ground > belly {
                    result.blocked += 1;
                    if stop_on_refusal {
                        return result;
                    }
                }
                if count_crush
                    && low != u16::MAX
                    && Fx::from_int(i32::from(low)) < local.add(hull_height)
                {
                    result.crush += 1;
                }
            }
        }
        result
    }
}

#[derive(Clone, Copy)]
pub struct Search {
    pub cell: i32,
    pub min: [i32; 2],
    pub max: [i32; 2],
    /// Total graph work: frontier pops, endpoint queries and swept edge poses.
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
#[cfg(test)]
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
    let turns = connection::turn_samples(turn, shape);
    let mut old = ground.pose(from, head, shape);
    let mut crush = 0;
    for i in 1..=turns {
        let h = angle::norm(head + turn * i / turns);
        let next = ground.pose(from, h, shape);
        if next.blocked != 0
            || next.tilt
            || next.steep && !old.steep
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
        if next.blocked != 0
            || next.tilt
            || next.steep && !old.steep
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
#[derive(Clone, Debug)]
pub struct Route {
    /// Turn points to the goal; empty when there is none.
    pub path: Vec<[i32; 2]>,
    /// The budget ended before the goal. This path stops on the live frontier;
    /// the caller must keep its goal and plan again from that endpoint.
    pub partial: bool,
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
    let answer = plan(ground, shape, from, head, to, cfg, clear);
    if answer.partial {
        Vec::new()
    } else {
        answer.path
    }
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
            .advance(&mut ground, &clear, usize::MAX)
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
/// as long as the world has not changed under it. A change invalidates
/// the cached probes on its pages and restarts an unfinished connection
/// that checked them. The answer is a route from the start pose: a caller
/// whose position or heading changed either discards the search itself or
/// takes the route up from where it has got to (ADR-462).
#[derive(Clone)]
pub struct Hunt {
    shape: Shape,
    cfg: Search,
    from: [Fx; 2],
    head: i32,
    /// The goal as the search snapped it, not as it was asked.
    to: [i32; 2],
    /// Materials the probe of each slice treats as yielding.
    yielding: Vec<Mat>,
    /// What the slices before this one scanned. Answers about the world,
    /// so only as good as the world they came from: [`Hunt::forget`] drops
    /// the changed pages, and the search itself carries on.
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
    distance: fx::lookup::Lookup<usize, u32>,
    previous: fx::lookup::Lookup<usize, usize>,
    poses: fx::lookup::Lookup<usize, Pose>,
    open: BinaryHeap<Reverse<(u32, u32, usize)>>,
    /// Graph work spent, expansions made, poses probed.
    spent: usize,
    nodes: u32,
    probed: u32,
    scans: u64,
    samples: u64,
    left_start: bool,
    prelude: Option<Prelude>,
    connection: Option<(usize, u32, Connection)>,
    expansion: Option<Expansion>,
}
#[derive(Clone)]
struct Expansion {
    node: usize,
    cost: u32,
    edge: u8,
    pending: Option<(usize, u32, Connection)>,
}
impl Expansion {
    fn new(node: usize, cost: u32) -> Self {
        Self {
            node,
            cost,
            edge: 0,
            pending: None,
        }
    }
}
#[derive(Clone)]
struct Prelude {
    head: i32,
    goal_cursor: usize,
    straight: Option<Connection>,
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
    /// Whole prelude for synchronous callers; the live range uses the same
    /// work through `start_bounded` with its deterministic per-tick allowance.
    pub fn start(
        ground: &mut Probe<'_>,
        shape: Shape,
        from: [Fx; 2],
        head: i32,
        to: [i32; 2],
        cfg: Search,
        clear: &impl Fn([Fx; 2], i32) -> bool,
    ) -> Hunting {
        Self::start_bounded(ground, shape, from, head, to, cfg, clear, usize::MAX)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn start_bounded(
        ground: &mut Probe<'_>,
        shape: Shape,
        from: [Fx; 2],
        head: i32,
        to: [i32; 2],
        cfg: Search,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        budget: usize,
    ) -> Hunting {
        Self::start_bounded_sweep(ground, shape, from, head, to, cfg, clear, budget, None)
    }
    /// Optional pure geometry preparation. Logical goal, direct-connection and
    /// frontier work still runs in the original order under the same budget.
    #[allow(clippy::too_many_arguments)]
    pub fn start_bounded_sweep(
        ground: &mut Probe<'_>,
        shape: Shape,
        from: [Fx; 2],
        head: i32,
        to: [i32; 2],
        cfg: Search,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        budget: usize,
        prepare: Option<&mut PrepareStart<'_>>,
    ) -> Hunting {
        Self::start_bounded_sweep_observed(
            ground,
            shape,
            from,
            head,
            to,
            cfg,
            clear,
            budget,
            prepare,
            |_| {},
        )
    }
    /// Pure boundaries of a fresh search; clocks belong to the observer.
    #[allow(clippy::too_many_arguments)]
    pub fn start_bounded_sweep_observed(
        ground: &mut Probe<'_>,
        shape: Shape,
        from: [Fx; 2],
        head: i32,
        to: [i32; 2],
        cfg: Search,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        budget: usize,
        prepare: Option<&mut PrepareStart<'_>>,
        completed: impl FnMut(&'static str),
    ) -> Hunting {
        let none = |stranded, exhausted, nodes, poses| {
            Hunting::Done(Route {
                path: vec![],
                partial: false,
                stranded,
                nodes,
                poses,
                exhausted,
                scans: 0,
                samples: 0,
            })
        };
        // Recovery uses exactly the executor's contacts. Padding a grade
        // can call a physically flat source steep and permit an illegal entry.
        let cfg = Search {
            margin: if cfg.margin > 0 && ground.contact_steep(from, head, shape) {
                0
            } else {
                cfg.margin
            },
            ..cfg
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
        let n = (ground.world.nx as i32).div_euclid(cfg.cell) as usize;
        let rows = (ground.world.nz as i32).div_euclid(cfg.cell) as usize;
        let start = from[0].to_int_trunc().div_euclid(cfg.cell) as usize
            + n * from[1].to_int_trunc().div_euclid(cfg.cell) as usize;
        let anchor = [
            from[0].to_int_trunc().rem_euclid(cfg.cell),
            from[1].to_int_trunc().rem_euclid(cfg.cell),
        ];
        let start_dir = (angle::norm(head + angle::FULL / 16) / (angle::FULL / 8)) as usize % 8;
        let initial = start * 8 + start_dir;
        let mut distance = fx::lookup::Lookup::new();
        let previous = fx::lookup::Lookup::new();
        let poses = fx::lookup::Lookup::new();
        let mut open = BinaryHeap::new();
        distance.insert(initial, 0);
        open.push(Reverse((0, 0, initial)));
        let mut hunt = Hunt {
            shape,
            cfg,
            from,
            head,
            to,
            yielding: ground.yielding.clone(),
            columns: Columns::default(),
            n,
            rows,
            anchor,
            initial,
            // The requested point can still be outside the hull's legal
            // ground. Index only the pose chosen by the bounded target probe.
            end: 0,
            distance,
            previous,
            poses,
            open,
            spent: 0,
            nodes: 0,
            probed: 0,
            scans: 0,
            samples: 0,
            left_start: false,
            prelude: Some(Prelude {
                head,
                goal_cursor: 0,
                straight: None,
            }),
            connection: None,
            expansion: None,
        };
        let result = hunt.prepare_sweep(ground, clear, &mut budget.max(1), prepare, completed);
        hunt.scans = ground.scans;
        hunt.samples = ground.samples;
        if let Some(mut route) = result {
            route.scans = hunt.scans;
            route.samples = hunt.samples;
            Hunting::Done(route)
        } else {
            hunt.columns = ground.take_columns();
            Hunting::Open(Box::new(hunt))
        }
    }
    /// Probe the target ring in the original order, then the exact swept run.
    fn prepare(
        &mut self,
        ground: &mut Probe<'_>,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        allowance: &mut usize,
    ) -> Option<Route> {
        self.prepare_sweep(ground, clear, allowance, None, |_| {})
    }
    // `start_batch` advances the run, so it must not be called when there is
    // no preparer; `Option::zip` would evaluate it eagerly.
    #[allow(clippy::manual_option_zip)]
    fn prepare_sweep(
        &mut self,
        ground: &mut Probe<'_>,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        allowance: &mut usize,
        prepare: Option<&mut PrepareStart<'_>>,
        mut completed: impl FnMut(&'static str),
    ) -> Option<Route> {
        let mut pending = self.prelude.take()?;
        if pending.straight.is_none() {
            let mut candidates = Vec::with_capacity(81);
            for r in 0..=4i32 {
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx.abs() == r || dz.abs() == r {
                            candidates.push([
                                self.to[0] + dx * self.cfg.cell,
                                self.to[1] + dz * self.cfg.cell,
                            ]);
                        }
                    }
                }
            }
            while pending.goal_cursor < candidates.len() * 8 && *allowance > 0 {
                let p = candidates[pending.goal_cursor / 8];
                let yaw = (pending.goal_cursor % 8) as i32 * angle::FULL / 8;
                pending.goal_cursor += 1;
                if (0..2).any(|a| p[a] < self.cfg.min[a] || p[a] >= self.cfg.max[a]) {
                    continue;
                }
                *allowance -= 1;
                self.probed += 1;
                let q = p.map(Fx::from_int);
                if clear(q, yaw) && !ground.goal_refused(q, yaw, self.shape) {
                    self.to = p;
                    let cell = [self.n, self.rows].map(|n| n as i32 - 1);
                    let end = [0, 1].map(|a| {
                        (p[a] - self.anchor[a])
                            .div_euclid(self.cfg.cell)
                            .clamp(0, cell[a]) as usize
                    });
                    self.end = end[0] + self.n * end[1];
                    pending.straight = Some(Connection::new(self.from, pending.head, q));
                    break;
                }
            }
            completed("plan_target");
            if pending.straight.is_none() && pending.goal_cursor == candidates.len() * 8 {
                return Some(self.empty(false, true));
            }
        }
        if let Some(run) = &mut pending.straight {
            if let Some((prepare, batch)) = prepare.and_then(|prepare| {
                run.start_batch(self.shape, *allowance)
                    .map(|batch| (prepare, batch))
            }) {
                let prepared = prepare(batch, ground);
                if let Some(columns) = prepared.columns {
                    ground.pages = columns.pages;
                    ground.replacement = columns.replacement;
                    ground.last_page = 0;
                }
                ground.prepared_routes.extend(
                    prepared
                        .poses
                        .into_iter()
                        .map(|((p, h), pose)| ((p[0], p[1], h), pose)),
                );
                ground.scans = ground.scans.wrapping_add(prepared.reads[0]);
                ground.samples = ground.samples.wrapping_add(prepared.reads[1]);
            }
            completed("plan_prepare");
            let before = *allowance;
            let answer = run.advance_with_endpoints(
                ground,
                self.shape,
                clear,
                allowance,
                None,
                None,
                self.cfg.margin <= 0,
            );
            completed("plan_direct");
            self.probed += (before - *allowance) as u32;
            match answer {
                Some(Some(_)) => {
                    return Some(Route {
                        path: vec![self.to],
                        ..self.empty(false, false)
                    })
                }
                // A fully swept direct route is usable even through yielding
                // trees. Do not discard it for an optional cheaper detour.
                Some(None) => return None,
                None => {}
            }
        }
        self.prelude = Some(pending);
        None
    }
    fn empty(&self, stranded: bool, exhausted: bool) -> Route {
        Route {
            path: vec![],
            partial: false,
            stranded,
            exhausted,
            nodes: self.nodes,
            poses: self.probed,
            scans: self.scans,
            samples: self.samples,
        }
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
    /// Swept edges were checked from this exact position and heading. A
    /// displaced caller must request a new search rather than reuse them.
    pub fn starts_at(&self, from: [Fx; 2], head: i32) -> bool {
        self.from == from && self.head == head
    }
    /// Where the search began.
    pub fn origin(&self) -> [Fx; 2] {
        self.from
    }
    /// One slice of a search that spans ticks: reuse its bounded pages and
    /// spend `budget` on target poses, swept poses or frontier nodes.
    pub fn advance_in(
        &mut self,
        world: &World,
        rubble: &Rubble,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        budget: usize,
    ) -> Option<Route> {
        self.advance_in_prepared(world, rubble, clear, budget, PreparedSweep::default())
    }
    /// Exact poses ahead of the currently suspended swept connection. A caller
    /// may prepare them independently; branch decisions still consume them in
    /// original order and pay the original logical allowance.
    pub fn sweep_samples(&self, budget: usize) -> Option<SweepSamples> {
        let run = self.pending_connection()?;
        let samples = run.samples(self.shape, budget);
        (samples.len() >= 16).then_some((self.shape, samples))
    }
    fn pending_connection(&self) -> Option<&Connection> {
        self.prelude
            .as_ref()
            .and_then(|p| p.straight.as_ref())
            .or_else(|| self.connection.as_ref().map(|(_, _, run)| run))
            .or_else(|| {
                self.expansion
                    .as_ref()
                    .and_then(|e| e.pending.as_ref().map(|(_, _, run)| run))
            })
    }
    /// A short exact prefix avoids launching workers for an immediate refusal.
    /// This only decides whether to speculate; the search still commits it.
    pub fn sweep_prefix_open(&self, poses: &[Pose]) -> bool {
        self.pending_connection()
            .is_some_and(|run| run.prefix_open(self.shape, poses))
    }
    /// Worker-local scratch over the search's already validated columns.
    /// A worker can extend/evict its copy without mutating another worker.
    pub fn sweep_probe<'a>(&self, world: &'a World, rubble: &'a Rubble) -> Probe<'a> {
        Probe::for_search(world, &self.yielding, self.columns.clone()).with_rubble(rubble)
    }
    /// Prepared answers must use this search's shape/yielding profile and the
    /// exact world/rubble being advanced. They are discarded after this slice.
    pub fn advance_in_prepared(
        &mut self,
        world: &World,
        rubble: &Rubble,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        budget: usize,
        prepared: PreparedSweep,
    ) -> Option<Route> {
        let carried = std::mem::take(&mut self.columns);
        let mut ground =
            Probe::for_search(world, &self.yielding, prepared.columns.unwrap_or(carried))
                .with_rubble(rubble);
        ground.prepared_routes.extend(
            prepared
                .poses
                .iter()
                .map(|&((p, h), pose)| ((p[0], p[1], h), pose)),
        );
        self.scans = self.scans.wrapping_add(prepared.reads[0]);
        self.samples = self.samples.wrapping_add(prepared.reads[1]);
        let result = self.advance(&mut ground, clear, budget);
        self.columns = ground.take_columns();
        result
    }
    /// The world or the rubble changed under the search (ADR-306,
    /// ADR-427). What it scanned or probed on the changed pages goes; a
    /// check whose checked part lies on one starts over, and so does the
    /// goal ring if a candidate it refused stood there. A change elsewhere
    /// leaves the search as it was: a long check finishes while a battle
    /// digs the other side of the map. The answers are a pure function of
    /// the world, so a search that forgets them finds what it would have
    /// found without them (P-29).
    pub fn forget(&mut self, changed: &Changed) {
        if changed.is_empty() {
            return;
        }
        self.columns.forget(changed);
        let r = reach(self.shape);
        // The same point as `Hunt::point`, including the sub-voxel start.
        let (n, anchor, cell) = (self.n, self.anchor, self.cfg.cell);
        let (start, from) = (self.initial / 8, self.from.map(Fx::to_int_trunc));
        self.poses.remove_where(|j| {
            let i = j / 8;
            let p = if i == start {
                from
            } else {
                [
                    (i % n) as i32 * cell + anchor[0],
                    (i / n) as i32 * cell + anchor[1],
                ]
            };
            changed.touches([p[0] - r, p[1] - r], [p[0] + 1 + r, p[1] + 1 + r])
        });
        let shape = self.shape;
        let touched = |run: &Connection| {
            run.checked(shape)
                .is_some_and(|(lo, hi)| changed.touches(lo, hi))
        };
        let ring = 4 * self.cfg.cell + r;
        let to = self.to;
        if let Some(pending) = &mut self.prelude {
            if let Some(run) = &mut pending.straight {
                if touched(run) {
                    run.restart();
                }
            } else if pending.goal_cursor > 0
                && changed.touches([to[0] - ring, to[1] - ring], [to[0] + ring, to[1] + ring])
            {
                pending.goal_cursor = 0;
            }
        }
        if let Some((_, _, run)) = &mut self.connection {
            if touched(run) {
                run.restart();
            }
        }
        if let Some(e) = &mut self.expansion {
            if let Some((_, _, run)) = &mut e.pending {
                if touched(run) {
                    run.restart();
                }
            }
        }
    }
    /// The search as bytes: the cells it reached, the frontier in priority
    /// order, and the request. What it scanned and probed is left out
    /// twice over — the window and the memo because they are answers about
    /// the world that a resumed search asks again, the counts of them
    /// because they count those asks and a resumed search asks fewer.
    /// A search restored from these bytes finds the same route (P-29).
    pub fn save(&self, b: &mut fx::blob::Blob) {
        // Negative half-size cannot occur in the legacy layout.
        b.i32(-2);
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
        let reached = self.distance.sorted_entries();
        for (&i, &d) in reached {
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
        b.bool(self.prelude.is_some());
        if let Some(p) = &self.prelude {
            b.i32(p.head);
            b.usize(p.goal_cursor);
            b.bool(p.straight.is_some());
            if let Some(run) = &p.straight {
                run.save(b);
            }
        }
        b.bool(self.connection.is_some());
        if let Some((node, cost, run)) = &self.connection {
            b.usize(*node);
            b.u32(*cost);
            run.save(b);
        }
        b.bool(self.expansion.is_some());
        if let Some(e) = &self.expansion {
            b.usize(e.node);
            b.u32(e.cost);
            b.u8(e.edge);
            b.bool(e.pending.is_some());
            if let Some((node, cost, run)) = &e.pending {
                b.usize(*node);
                b.u32(*cost);
                run.save(b);
            }
        }
        b.i32(self.head);
    }
    /// A search read back. `None` when the bytes do not describe one.
    pub fn load(s: &mut fx::blob::Scan, yielding: &[Mat]) -> Option<Hunt> {
        let mut i32s = [0i32; 7];
        let first = s.i32();
        let swept = first == -2;
        let resumable = first == -1 || swept;
        i32s[0] = if resumable { s.i32() } else { first };
        for v in &mut i32s[1..] {
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
        if !s.ok()
            || cells == 0
            || cells > 1 << 29
            || initial >= cells
            || end >= cells / 8
            || cfg.cell <= 0
            || cfg.nodes == 0
            || spent > cfg.nodes
        {
            return None;
        }
        let mut hunt = Hunt {
            shape,
            cfg,
            from,
            head: (initial % 8) as i32 * angle::FULL / 8,
            to,
            yielding: yielding.to_vec(),
            columns: Columns::default(),
            n,
            rows,
            anchor,
            initial,
            end,
            distance: fx::lookup::Lookup::new(),
            previous: fx::lookup::Lookup::new(),
            poses: fx::lookup::Lookup::new(),
            open: BinaryHeap::new(),
            spent,
            nodes,
            probed: 0,
            scans: 0,
            samples: 0,
            left_start,
            prelude: None,
            connection: None,
            expansion: None,
        };
        let reached = s.u32() as usize;
        if reached > cells || reached > s.left() / 12 {
            return None;
        }
        for _ in 0..reached {
            let i = s.u32() as usize;
            let (d, p) = (s.u32(), s.u32());
            if i >= cells || (p != u32::MAX && p as usize >= cells) {
                return None;
            }
            hunt.distance.insert(i, d);
            if p != u32::MAX {
                hunt.previous.insert(i, p as usize);
            }
        }
        // Every reached cell leads back to the start (ADR-432): a chain
        // with a gap or a loop sent the route walk off the grid or round
        // it forever, where a bad snapshot must simply not load.
        let mut rooted = BTreeSet::from([initial]);
        for i in hunt.distance.sorted_keys() {
            let mut chain = Vec::new();
            let mut j = i;
            while !rooted.contains(&j) {
                if chain.len() >= hunt.distance.len() {
                    return None;
                }
                chain.push(j);
                j = *hunt.previous.get(&j)?;
            }
            rooted.extend(chain);
        }
        let open = s.u32() as usize;
        if open > cells || open > s.left() / 12 {
            return None;
        }
        for _ in 0..open {
            let (f, cost, i) = (s.u32(), s.u32(), s.u32() as usize);
            if i >= cells {
                return None;
            }
            hunt.open.push(Reverse((f, cost, i)));
        }
        if resumable {
            if s.bool() {
                let head = s.i32();
                let goal_cursor = s.usize();
                if !(0..angle::FULL).contains(&head) || goal_cursor > 648 {
                    return None;
                }
                let straight = if s.bool() {
                    Some(Connection::load(s, swept)?)
                } else {
                    None
                };
                hunt.prelude = Some(Prelude {
                    head,
                    goal_cursor,
                    straight,
                });
            }
            if s.bool() {
                let node = s.usize();
                let cost = s.u32();
                if node >= cells {
                    return None;
                }
                hunt.connection = Some((node, cost, Connection::load(s, swept)?));
            }
        }
        if swept && s.bool() {
            let node = s.usize();
            let cost = s.u32();
            let edge = s.u8();
            if node >= cells || edge > 3 {
                return None;
            }
            let pending = if s.bool() {
                let j = s.usize();
                let next_cost = s.u32();
                if j >= cells {
                    return None;
                }
                Some((j, next_cost, Connection::load(s, true)?))
            } else {
                None
            };
            hunt.expansion = Some(Expansion {
                node,
                cost,
                edge,
                pending,
            });
        }
        if swept {
            hunt.head = s.i32();
        } else if let Some(p) = &hunt.prelude {
            hunt.head = p.head;
        }
        (s.ok() && (0..angle::FULL).contains(&hunt.head)).then_some(hunt)
    }

    fn heading(&self, node: usize) -> i32 {
        if node == self.initial {
            self.head
        } else {
            (node % 8) as i32 * angle::FULL / 8
        }
    }

    fn point(&self, i: usize) -> [Fx; 2] {
        // The first swept turn starts at the real sub-voxel position, not
        // its truncated grid corner. Rounding can invent a collision.
        if i == self.initial / 8 {
            return self.from;
        }
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
        let mut allowance = budget;
        if let Some(mut route) = self.prepare(ground, clear, &mut allowance) {
            self.scans += ground.scans.saturating_sub(scans);
            self.samples += ground.samples.saturating_sub(samples);
            route.scans = self.scans;
            route.samples = self.samples;
            return Some(route);
        }
        while self.prelude.is_none() && allowance > 0 {
            if self.spent >= self.cfg.nodes && self.connection.is_none() {
                // The reached source of a pending edge remains a frontier
                // candidate; the unchecked destination never enters a prefix.
                break;
            }
            if self.expansion.is_some() {
                let mut edge_allowance = allowance.min(self.cfg.nodes - self.spent);
                let before = edge_allowance;
                self.expand(ground, clear, &mut edge_allowance);
                let used = before - edge_allowance;
                allowance -= used;
                self.spent += used;
                continue;
            }
            if let Some((node, cost, run)) = &mut self.connection {
                let before = allowance;
                let answer = run.advance_with_endpoints(
                    ground,
                    self.shape,
                    clear,
                    &mut allowance,
                    None,
                    None,
                    self.cfg.margin <= 0,
                );
                self.probed += (before - allowance) as u32;
                match answer {
                    None => break,
                    Some(Some(_)) => {
                        found = Some(*node);
                        break;
                    }
                    Some(None) => {
                        self.expansion = Some(Expansion::new(*node, *cost));
                        self.connection = None;
                        continue;
                    }
                }
            }
            if self.spent >= self.cfg.nodes {
                break;
            }
            self.spent += 1;
            allowance -= 1;
            let Some(Reverse((_, cost, i))) = self.open.pop() else {
                exhausted = true;
                break;
            };
            if cost != self.cost(i) {
                continue;
            }
            self.nodes += 1;
            let at = self.point(i / 8);
            let yaw = self.heading(i);
            if (at[0].to_int_trunc() - self.to[0])
                .abs()
                .max((at[1].to_int_trunc() - self.to[1]).abs())
                <= self.cfg.cell * 4
                || i / 8 == self.end
            {
                self.connection =
                    Some((i, cost, Connection::new(at, yaw, self.to.map(Fx::from_int))));
            } else {
                self.expansion = Some(Expansion::new(i, cost));
            }
        }
        self.scans += ground.scans.saturating_sub(scans);
        self.samples += ground.samples.saturating_sub(samples);
        match found {
            Some(i) => Some(self.route_through(i, false)),
            None if exhausted || (self.spent >= self.cfg.nodes && self.connection.is_none()) => {
                // A dry frontier is a proven refusal. A spent budget is not:
                // retain useful, checked progress instead of discarding it.
                if !exhausted {
                    if let Some(node) = self.prefix_endpoint() {
                        return Some(self.route_through(node, true));
                    }
                }
                Some(self.empty(!self.left_start, exhausted))
            }
            None => None,
        }
    }

    /// Finish at most this tick's allowance of exact pose probes. In
    /// particular, an edge never pays for a whole turn in one tick.
    fn expand(
        &mut self,
        ground: &mut Probe<'_>,
        clear: &impl Fn([Fx; 2], i32) -> bool,
        allowance: &mut usize,
    ) {
        let mut e = self.expansion.take().unwrap();
        let direction = e.node % 8;
        let at = self.point(e.node / 8);
        let yaw = self.heading(e.node);
        // Pure validation of the reached source, including the real heading
        // at the initial node. Derived cache only; it adds no graph state.
        let source = if let Some(pose) = self.poses.get(&e.node) {
            *pose
        } else {
            let pose = ground.pose(at, yaw, self.shape);
            self.poses.insert(e.node, pose);
            pose
        };
        let leave_steep = source.steep && self.cfg.margin <= 0;
        while e.edge < 3 && *allowance > 0 {
            if e.pending.is_none() {
                let turn = i32::from(e.edge) - 1;
                let d = (direction as i32 + turn).rem_euclid(8) as usize;
                let cell = if turn == 0 {
                    let x = (e.node / 8 % self.n) as i32 + DIR[d][0];
                    let z = (e.node / 8 / self.n) as i32 + DIR[d][1];
                    if x < 0 || z < 0 || x >= self.n as i32 || z >= self.rows as i32 {
                        e.edge += 1;
                        continue;
                    }
                    x as usize + self.n * z as usize
                } else {
                    e.node / 8
                };
                let q = self.point(cell);
                if (0..2).any(|a| {
                    q[a] < Fx::from_int(self.cfg.min[a]) || q[a] >= Fx::from_int(self.cfg.max[a])
                }) {
                    e.edge += 1;
                    continue;
                }
                let j = cell * 8 + d;
                let base = e.cost
                    + if turn == 0 {
                        if d.is_multiple_of(2) {
                            10
                        } else {
                            14
                        }
                    } else {
                        6
                    };
                if base >= self.cost(j) {
                    e.edge += 1;
                    continue;
                }
                let next_yaw = d as i32 * angle::FULL / 8;
                // Charge the logical query even on a memo hit: restored
                // searches have empty caches but must finish on the same tick.
                *allowance -= 1;
                if !clear(q, next_yaw) {
                    e.edge += 1;
                    continue;
                }
                let cached = self.poses.get(&j).copied();
                let pose = match cached {
                    Some(p) if !leave_steep || !p.steep || p.tilt || p.blocked != 0 => p,
                    _ => {
                        self.probed += 1;
                        let p = ground.route_pose_mode(q, next_yaw, self.shape, leave_steep);
                        self.poses.insert(j, p);
                        p
                    }
                };
                let next_cost = base
                    + (pose.crush as u32)
                        .saturating_mul(self.cfg.crush_cost)
                        .min(self.cfg.crush_cap);
                if next_cost >= self.cost(j)
                    || pose.blocked != 0
                    || pose.tilt
                    || pose.steep && !leave_steep
                {
                    e.edge += 1;
                    continue;
                }
                e.pending = Some((j, next_cost, Connection::sweep(at, yaw, q, next_yaw)));
            }
            let (j, next_cost, run) = e.pending.as_mut().unwrap();
            let before = *allowance;
            let origin_pose = self.poses.get(&e.node).copied();
            let target_pose = self.poses.get(j).copied();
            let answer = run.advance_with_endpoints(
                ground,
                self.shape,
                clear,
                allowance,
                origin_pose,
                target_pose,
                self.cfg.margin <= 0,
            );
            self.probed += (before - *allowance) as u32;
            match answer {
                None => break,
                Some(Some(_)) => {
                    let (j, next_cost) = (*j, *next_cost);
                    self.distance.insert(j, next_cost);
                    self.previous.insert(j, e.node);
                    self.left_start |= j / 8 != self.initial / 8;
                    let q = self.point(j / 8);
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
                Some(None) => {}
            }
            e.pending = None;
            e.edge += 1;
        }
        if e.edge < 3 {
            self.expansion = Some(e);
        }
    }

    fn progress(&self, node: usize, cost: u32) -> (u64, u32, usize) {
        let p = self.point(node / 8).map(Fx::to_int_trunc);
        let d = [0, 1].map(|a| (p[a] - self.to[a]).unsigned_abs() as u64);
        (d[0] * d[0] + d[1] * d[1], cost, node)
    }

    /// End a bounded prefix on the live frontier, not in a closer pocket
    /// whose exits the search has already exhausted. All predecessors are
    /// checked; ties use distance, path cost and stable node index. A detour
    /// may initially lead away from the goal, so progress is not monotonic.
    fn prefix_endpoint(&self) -> Option<usize> {
        self.open
            .iter()
            .map(|Reverse((_, cost, node))| (*node, *cost))
            .chain(self.expansion.iter().map(|e| (e.node, e.cost)))
            .filter(|&(node, cost)| node / 8 != self.initial / 8 && cost == self.cost(node))
            .map(|(node, cost)| self.progress(node, cost))
            .min()
            .map(|(_, _, node)| node)
    }

    /// The checked turn points to this node; only a completed final connection
    /// may append the actual goal. Partial paths never bridge unchecked space.
    fn route_through(&self, mut i: usize, partial: bool) -> Route {
        let mut all = vec![];
        while i != self.initial {
            let p = self.point(i / 8).map(Fx::to_int_trunc);
            if i / 8 != self.initial / 8 && all.last() != Some(&p) {
                all.push(p);
            }
            i = self.previous.get(&i).copied().unwrap_or(usize::MAX);
        }
        all.reverse();
        if !partial && all.last() != Some(&self.to) {
            all.push(self.to);
        }
        // Remove collinear grid points while retaining the exact turn
        // centres. A reversal is a turn too, though its cross product is
        // zero like a straight line's: the hull drives on past it and
        // pivots there, where the search found room to (ADR-432).
        let mut out = vec![];
        let mut before = self.from.map(Fx::to_int_trunc);
        for (i, p) in all.iter().enumerate() {
            let keep = i + 1 == all.len() || {
                let q = all[i + 1];
                let a = [p[0] - before[0], p[1] - before[1]];
                let b = [q[0] - p[0], q[1] - p[1]];
                a[0] * b[1] != a[1] * b[0] || a[0] * b[0] + a[1] * b[1] < 0
            };
            if keep {
                out.push(*p);
            }
            before = *p;
        }
        Route {
            path: out,
            partial,
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
    #[test]
    fn fractional_escape_keeps_existing_contacts_but_cannot_exchange_them_for_a_wall() {
        let w = world(false, false, false);
        let mut rubble = Rubble::new();
        rubble.insert([80, 100], 30);
        rubble.insert([86, 100], 30);
        let mut probe = Probe::new(&w).with_rubble(&rubble);
        let shape = Shape {
            half: [4, 4],
            support: [4, 4],
            clearance: Fx::from_int(2),
            step: 2,
            min_up: Fx::from_ratio(4, 5),
            height: 8,
            span: 0,
            ramp: Fx::ZERO,
        };
        let from = [80, 100].map(Fx::from_int);
        let old = probe.pose(from, 0, shape);
        assert_eq!(old.blocked, 1);
        for fraction in [Fx(1), Fx::from_ratio(1, 4), Fx::from_ratio(1, 2)] {
            let to = [from[0].add(fraction), from[1]];
            let next = probe.pose(to, 0, shape);
            assert_eq!(next.blocked, old.blocked);
            assert!(probe.keeps_belly_contacts((from, 0, old), (to, 0, next), shape));
        }
        let to = [86, 100].map(Fx::from_int);
        let next = probe.pose(to, 0, shape);
        assert_eq!(
            next.blocked, old.blocked,
            "equal counts can hide a new wall"
        );
        assert!(!probe.keeps_belly_contacts((from, 0, old), (to, 0, next), shape));
    }
    impl Probe<'_> {
        fn reference_contact(
            &mut self,
            p: [Fx; 2],
            radius: Fx,
            dir: [Fx; 2],
            s: Shape,
            grade: Fx,
        ) -> (Fx, bool) {
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
    }
    #[test]
    fn contact_reductions_match_two_pass_support_on_rough_ground() {
        let w = world(true, true, true);
        let mut actual = Probe::new(&w);
        let mut reference = Probe::new(&w);
        for span in [0, 1, 2, 7, 25] {
            for step in [0, 1, 4, 8] {
                let shape = Shape {
                    span,
                    step,
                    ..tank()
                };
                for at in [[1, 1], [60, 70], [90, 72], [128, 110], [254, 254]] {
                    for fraction in [Fx::ZERO, Fx(1), Fx::from_ratio(2, 3)] {
                        let p = at.map(|n| Fx::from_int(n).add(fraction));
                        for head in [0, angle::FULL / 7, angle::FULL / 4] {
                            for grade in [Fx::ZERO, Fx::from_ratio(3, 4)] {
                                let dir = [angle::cos(head), angle::sin(head)];
                                let got = actual.contact(p, Fx::from_int(2), dir, shape, grade);
                                let want = reference.reference_contact(
                                    p,
                                    Fx::from_int(2),
                                    dir,
                                    shape,
                                    grade,
                                );
                                assert_eq!(
                                    got, want,
                                    "span={span} step={step} p={p:?} head={head} grade={grade:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(actual.samples, reference.samples);
        assert_eq!(actual.columns, reference.columns);
    }

    #[test]
    fn cached_grade_tracks_changed_tilt_limits() {
        let w = world(true, true, true);
        let mut reused = Probe::new(&w);
        for at in [[90, 72], [125, 100], [80, 90]] {
            for limit in [91, 70, 100, 85, 0, 91] {
                let shape = Shape {
                    min_up: Fx::from_ratio(limit, 100),
                    ..tank()
                };
                let pos = at.map(Fx::from_int);
                assert_eq!(
                    reused.contacts(pos, 0, shape),
                    Probe::new(&w).contacts(pos, 0, shape)
                );
            }
        }
    }
    #[test]
    fn aligned_supports_skip_only_zero_weight_surface_neighbours() {
        let w = world(true, true, true);
        let rubble = Rubble::from([([128, 72], 40)]);
        let mut full = Probe::new(&w).with_rubble(&rubble);
        let mut fast = Probe::new(&w).with_rubble(&rubble);
        for x in [-1, 0, 58, 82, 90, 127, 128, 255, 256] {
            for z in [-1, 0, 56, 72, 83, 255, 256] {
                for dx in [Fx::ZERO, Fx(1), Fx::from_ratio(1, 3)] {
                    for dz in [Fx::ZERO, Fx(1), Fx::from_ratio(9, 10)] {
                        let p = [Fx::from_int(x).add(dx), Fx::from_int(z).add(dz)];
                        let x = p[0].to_int_trunc();
                        let z = p[1].to_int_trunc();
                        let u = p[0].sub(Fx::from_int(x));
                        let v = p[1].sub(Fx::from_int(z));
                        let h00 = Fx::from_int(full.floor(x, z));
                        let h10 = Fx::from_int(full.floor(x + 1, z));
                        let h01 = Fx::from_int(full.floor(x, z + 1));
                        let h11 = Fx::from_int(full.floor(x + 1, z + 1));
                        let a = h00.add(h10.sub(h00).mul(u));
                        let b = h01.add(h11.sub(h01).mul(u));
                        let before = fast.columns;
                        assert_eq!(fast.surface(p), a.add(b.sub(a).mul(v)));
                        let needed =
                            (1 + u64::from(u != Fx::ZERO)) * (1 + u64::from(v != Fx::ZERO));
                        assert_eq!(fast.columns - before, needed);
                    }
                }
            }
        }
        assert!(fast.columns < full.columns);
    }
    #[test]
    fn route_refusals_match_full_clearance_and_keep_legal_pose_counts() {
        let worlds = [
            world(false, false, false),
            world(true, true, false),
            world(false, false, true),
            World::new(256, 64, 256, 100, Box::new(Forest)),
        ];
        let rubble = Rubble::from([([148, 110], 38), ([149, 110], 38)]);
        let mut legal = 0;
        let mut refused = 0;
        let mut crushed = 0;
        let mut saved_columns = 0;
        for w in &worlds {
            let mut full = Probe::with_yield(w, &[7, 8]).with_rubble(&rubble);
            let mut fast = Probe::with_yield(w, &[7, 8]).with_rubble(&rubble);
            for shape in [
                tank(),
                Shape {
                    span: 0,
                    step: 8,
                    min_up: Fx::from_ratio(65, 100),
                    ..tank()
                },
            ] {
                for x in [24, 72, 110, 123, 148, 208, 248] {
                    for z in [24, 72, 110, 123, 148, 208, 248] {
                        for head in (0..angle::FULL).step_by(8192) {
                            let at = [
                                Fx::from_int(x).add(Fx::from_ratio(1, 3)),
                                Fx::from_int(z).add(Fx::from_ratio(17, 19)),
                            ];
                            let expected = full.pose(at, head, shape);
                            let actual = fast.route_pose(at, head, shape);
                            assert_eq!(actual.refused(), expected.refused());
                            assert_eq!(actual.height, expected.height);
                            assert_eq!(actual.floors, expected.floors);
                            assert_eq!(actual.steep, expected.steep);
                            assert_eq!(actual.tilt, expected.tilt);
                            if expected.refused() {
                                refused += 1;
                                assert!(actual.blocked <= expected.blocked);
                                assert!(actual.crush <= expected.crush);
                            } else {
                                legal += 1;
                                crushed += usize::from(expected.crush > 0);
                                assert_eq!(actual.blocked, expected.blocked);
                                assert_eq!(actual.crush, expected.crush);
                            }
                        }
                    }
                }
            }
            assert!(fast.columns < full.columns);
            saved_columns += full.columns - fast.columns;
        }
        assert!(legal > 100 && refused > 100 && crushed > 0);
        assert!(saved_columns > 100_000);
    }
    #[test]
    fn hull_refusal_spends_a_query_without_scanning_the_rejected_pose() {
        let w = world(false, false, false);
        let from = [60, 100].map(Fx::from_int);
        let to = [200, 100].map(Fx::from_int);
        let mut run = Connection::new(from, 0, to);
        let mut ground = Probe::new(&w);
        // One query establishes the initial height. The next checks the
        // hull before asking terrain; it still consumes its logical work.
        assert!(run
            .advance(&mut ground, tank(), &|_, _| false, &mut 1)
            .is_none());
        let samples = ground.samples;
        let columns = ground.columns;
        let mut budget = 1;
        assert_eq!(
            run.advance(&mut ground, tank(), &|_, _| false, &mut budget),
            Some(None)
        );
        assert_eq!(budget, 0);
        assert_eq!(ground.samples, samples);
        assert_eq!(ground.columns, columns);
        // A new attempt after the hull leaves must probe the open ground.
        run.restart();
        let mut budget = usize::MAX;
        assert!(run
            .advance(&mut ground, tank(), &|_, _| true, &mut budget)
            .unwrap()
            .is_some());
        assert!(ground.samples > samples);
    }
    #[test]
    fn footprint_rows_match_individual_fixed_point_transforms() {
        let w = world(false, false, false);
        let probe = Probe::new(&w);
        for base in [-100, 0, 91, 99999] {
            for half in [[20, 10], [1, 1], [0, 0], [3, 34]] {
                let s = Shape { half, ..tank() };
                let pos = [
                    Fx::from_int(base).add(Fx::from_ratio(1, 3)),
                    Fx::from_int(base).sub(Fx::from_ratio(7, 13)),
                ];
                for head in (0..angle::FULL).step_by(257).chain([
                    0,
                    1,
                    angle::FULL / 4 - 1,
                    angle::FULL / 4,
                    angle::FULL / 2,
                ]) {
                    let (c, sin) = (angle::cos(head), angle::sin(head));
                    let (lo, hi) = probe.extent(pos, head, s);
                    for z in lo[1]..=hi[1] {
                        let expected: Vec<_> = (lo[0]..=hi[0])
                            .filter(|&x| probe.inside(pos, c, sin, s, x, z))
                            .map(|x| {
                                let dx = Fx::from_int(x).add(Fx::from_ratio(1, 2)).sub(pos[0]);
                                let dz = Fx::from_int(z).add(Fx::from_ratio(1, 2)).sub(pos[1]);
                                (x, dx.mul(c).add(dz.mul(sin)), dz.mul(c).sub(dx.mul(sin)))
                            })
                            .collect();
                        assert_eq!(
                            Probe::row(pos, c, sin, s, lo[0], hi[0], z).collect::<Vec<_>>(),
                            expected,
                            "base={base} half={half:?} head={head} row={z}"
                        );
                        let split: Vec<_> = (lo[0].div_euclid(8)..=hi[0].div_euclid(8))
                            .flat_map(|tile| {
                                Probe::row(
                                    pos,
                                    c,
                                    sin,
                                    s,
                                    lo[0].max(tile * 8),
                                    hi[0].min(tile * 8 + 7),
                                    z,
                                )
                            })
                            .collect();
                        assert_eq!(split, expected, "split row changed fixed-point coordinates");
                    }
                }
            }
        }
    }
    #[test]
    fn swept_edges_reject_obstacles_between_old_midpoint_samples() {
        let w = world(false, false, false);
        for turn in [false, true] {
            let from = [80, 100].map(Fx::from_int);
            let to = if turn {
                from
            } else {
                [88, 100].map(Fx::from_int)
            };
            let yaw = if turn { angle::FULL / 8 } else { 0 };
            let clear = |p: [Fx; 2], h: i32| {
                if turn {
                    !(500..900).contains(&h)
                } else {
                    p[0] < Fx::from_int(82) || p[0] > Fx::from_int(83)
                }
            };
            let mut run = Connection::sweep(from, 0, to, yaw);
            let mut ground = Probe::new(&w);
            let mut refused = false;
            for _ in 0..100 {
                let result = run.advance(&mut ground, tank(), &clear, &mut 1);
                if let Some(result) = result {
                    assert_eq!(result, None);
                    refused = true;
                    break;
                }
                let mut b = fx::blob::Blob::new();
                run.save(&mut b);
                run = Connection::load(&mut fx::blob::Scan::new(&b.v), true).unwrap();
            }
            assert!(refused, "missed obstacle during turn={turn}");
        }
    }

    #[test]
    fn target_at_world_corner_is_probed_before_it_is_indexed() {
        let w = world(false, false, false);
        let cfg = Search {
            min: [0; 2],
            ..search()
        };
        let mut ground = Probe::new(&w);
        let Hunting::Open(mut hunt) = Hunt::start_bounded(
            &mut ground,
            tank(),
            [51, 73].map(Fx::from_int),
            0,
            [0, 0],
            cfg,
            &|_, _| true,
            1,
        ) else {
            panic!("the corner target requires further pose checks");
        };
        let mut bytes = fx::blob::Blob::new();
        hunt.save(&mut bytes);
        *hunt = Hunt::load(&mut fx::blob::Scan::new(&bytes.v), &[]).unwrap();
        let answer = hunt
            .advance_in(&w, &Rubble::new(), &|_, _| true, usize::MAX)
            .unwrap();
        assert_ne!(answer.path.last(), Some(&[0, 0]));
    }
    #[test]
    fn sliced_connections_match_the_original_swept_poses() {
        for (rock, corridor, hill) in [
            (false, false, false),
            (true, true, false),
            (false, false, true),
        ] {
            let w = world(rock, corridor, hill);
            for (from, head, to) in [
                ([45, 72], 0, [210, 72]),
                ([150, 128], angle::FULL / 3, [65, 100]),
            ] {
                let from = from.map(Fx::from_int);
                let to = to.map(Fx::from_int);
                let mut reference = Probe::new(&w);
                let expected = connects(&mut reference, tank(), from, head, to, &|_, _| true);
                for budget in [1, 7, 96] {
                    let mut ground = Probe::for_search(&w, &[], Columns::default());
                    let mut run = Connection::new(from, head, to);
                    let actual = loop {
                        let before = ground.samples;
                        let mut left = budget;
                        let answer = run.advance(&mut ground, tank(), &|_, _| true, &mut left);
                        assert!(ground.samples - before < (budget * 1000) as u64);
                        let mut b = fx::blob::Blob::new();
                        run.save(&mut b);
                        run = Connection::load(&mut fx::blob::Scan::new(&b.v), true).unwrap();
                        if let Some(answer) = answer {
                            break answer;
                        }
                    };
                    assert_eq!(actual, expected);
                    // Resuming just before a steep refusal reconstructs the
                    // previous contacts; it never redoes a full footprint.
                    assert!(ground.samples >= reference.samples);
                    assert!(ground.samples <= reference.samples * 2);
                }
            }
        }
    }
    #[test]
    fn an_edge_reuses_probed_endpoints_without_changing_clearance_or_budget() {
        let w = world(false, false, true);
        let from = [116, 72].map(Fx::from_int);
        let to = [124, 72].map(Fx::from_int);
        let shape = tank();
        let mut endpoints = Probe::new(&w);
        let start = endpoints.route_pose(from, 0, shape);
        let end = endpoints.route_pose(to, 0, shape);
        let mut plain = Probe::new(&w);
        let mut cached = Probe::new(&w);
        let mut a = Connection::sweep(from, 0, to, 0);
        let mut b = a.clone();
        let mut left_a = usize::MAX;
        let mut left_b = usize::MAX;
        let expected = a.advance(&mut plain, shape, &|_, _| true, &mut left_a);
        let actual = b.advance_with_endpoints(
            &mut cached,
            shape,
            &|_, _| true,
            &mut left_b,
            Some(start),
            Some(end),
            true,
        );
        assert_eq!(actual, expected);
        assert_eq!(left_b, left_a);
        let mut saved_a = fx::blob::Blob::new();
        let mut saved_b = fx::blob::Blob::new();
        a.save(&mut saved_a);
        b.save(&mut saved_b);
        assert_eq!(saved_b.v, saved_a.v);
        assert!(cached.samples < plain.samples);
    }
    #[test]
    fn bounded_prelude_and_finish_restore_on_the_same_slice() {
        let w = world(false, false, true);
        let from = [50, 72].map(Fx::from_int);
        for to in [[220, 72], [128, 128]] {
            let expected = plan(Probe::new(&w), tank(), from, 0, to, search(), |_, _| true);
            let mut probe = Probe::for_search(&w, &[], Columns::default());
            let Hunting::Open(mut live) =
                Hunt::start_bounded(&mut probe, tank(), from, 0, to, search(), &|_, _| true, 1)
            else {
                panic!("one pose cannot finish a long route");
            };
            let mut slices = 0;
            loop {
                let mut bytes = fx::blob::Blob::new();
                live.save(&mut bytes);
                let mut loaded = Hunt::load(&mut fx::blob::Scan::new(&bytes.v), &[]).unwrap();
                let a = live.advance_in(&w, &Rubble::new(), &|_, _| true, 7);
                let b = loaded.advance_in(&w, &Rubble::new(), &|_, _| true, 7);
                assert_eq!(a.is_some(), b.is_some());
                let mut lhs = fx::blob::Blob::new();
                let mut rhs = fx::blob::Blob::new();
                live.save(&mut lhs);
                loaded.save(&mut rhs);
                assert_eq!(lhs.v, rhs.v, "different search progress after load");
                slices += 1;
                assert!(slices < 5000);
                if let Some(answer) = a {
                    assert_eq!(answer.path, expected.path);
                    assert_eq!(answer.path, b.unwrap().path);
                    break;
                }
            }
            assert!(slices > 1);
        }
    }
    #[test]
    fn initial_sweep_prefix_uses_origin_only_to_establish_height() {
        let pose = |height, blocked| Pose {
            height: Fx::from_int(height),
            floors: [Fx::from_int(height); 4],
            blocked,
            steep: false,
            tilt: false,
            crush: 0,
        };
        let mut batch = StartSweep {
            shape: tank(),
            samples: Vec::new(),
            height: Fx::ZERO,
            initial: true,
        };
        assert!(batch.prefix_open(&[pose(20, 1), pose(24, 0)]));
        assert!(!batch.prefix_open(&[pose(20, 1), pose(25, 0)]));
        assert!(!batch.prefix_open(&[pose(20, 0), pose(20, 1)]));
        batch.initial = false;
        batch.height = Fx::from_int(20);
        assert!(!batch.prefix_open(&[pose(20, 1)]));
        assert!(!batch.prefix_open(&[pose(25, 0)]));
        assert!(batch.prefix_open(&[pose(24, 0)]));
    }
    #[test]
    fn prepared_initial_sweeps_preserve_frontiers_budgets_and_callbacks() {
        let w = world(false, false, true);
        let mut batches = 0;
        for budget in [0, 1, 7, 17, 32, 96] {
            for to in [[220, 72], [128, 128], [8, 8]] {
                for blocked in [false, true] {
                    let from = [50, 72].map(Fx::from_int);
                    let mut a = Probe::for_search(&w, &[], Columns::default());
                    let mut b = Probe::for_search(&w, &[], Columns::default());
                    let calls_a = std::cell::RefCell::new(Vec::new());
                    let calls_b = std::cell::RefCell::new(Vec::new());
                    let clear_a = |p, h| {
                        calls_a.borrow_mut().push((p, h));
                        !blocked
                    };
                    let clear_b = |p, h| {
                        calls_b.borrow_mut().push((p, h));
                        !blocked
                    };
                    let mut prepare = |batch: StartSweep, probe: &mut Probe<'_>| {
                        batches += 1;
                        let answers = probe.route_samples(batch.shape, &batch.samples);
                        PreparedSweep {
                            poses: batch.samples.into_iter().zip(answers).collect(),
                            ..Default::default()
                        }
                    };
                    let x = Hunt::start_bounded(
                        &mut a,
                        tank(),
                        from,
                        1234,
                        to,
                        search(),
                        &clear_a,
                        budget,
                    );
                    let mut stages = Vec::new();
                    let y = Hunt::start_bounded_sweep_observed(
                        &mut b,
                        tank(),
                        from,
                        1234,
                        to,
                        search(),
                        &clear_b,
                        budget,
                        Some(&mut prepare),
                        |stage| stages.push(stage),
                    );
                    if (0..2).any(|i| to[i] < search().min[i] || to[i] >= search().max[i]) {
                        assert!(stages.is_empty());
                    } else {
                        assert!(
                            stages == ["plan_target"]
                                || stages == ["plan_target", "plan_prepare", "plan_direct"]
                        );
                    }
                    assert_eq!(calls_a.into_inner(), calls_b.into_inner());
                    match (x, y) {
                        (Hunting::Done(a), Hunting::Done(b)) => {
                            assert_eq!(
                                (a.path, a.nodes, a.poses, a.partial, a.stranded, a.exhausted),
                                (b.path, b.nodes, b.poses, b.partial, b.stranded, b.exhausted)
                            );
                        }
                        (Hunting::Open(mut a), Hunting::Open(mut b)) => {
                            for _ in 0..12 {
                                let mut sa = fx::blob::Blob::new();
                                let mut sb = fx::blob::Blob::new();
                                a.save(&mut sa);
                                b.save(&mut sb);
                                assert_eq!(sa.v, sb.v);
                                let ra = a.advance_in(&w, &Rubble::new(), &|_, _| !blocked, 7);
                                let rb = b.advance_in(&w, &Rubble::new(), &|_, _| !blocked, 7);
                                assert_eq!(ra.is_some(), rb.is_some());
                                if let Some(a) = ra {
                                    let b = rb.unwrap();
                                    assert_eq!(
                                        (
                                            a.path,
                                            a.nodes,
                                            a.poses,
                                            a.partial,
                                            a.stranded,
                                            a.exhausted
                                        ),
                                        (
                                            b.path,
                                            b.nodes,
                                            b.poses,
                                            b.partial,
                                            b.stranded,
                                            b.exhausted
                                        )
                                    );
                                    break;
                                }
                            }
                        }
                        _ => panic!("preparation changed initial search progress"),
                    }
                }
            }
        }
        assert!(batches > 0);
    }
    #[test]
    fn prepared_sweeps_preserve_search_slices_edits_and_blocker_callbacks() {
        let mut batches = 0;
        for budget in [16, 31, 64] {
            let mut w = world(false, false, true);
            let rubble = Rubble::from([([149, 72], 30)]);
            let mut probe = Probe::for_search(&w, &[], Columns::default()).with_rubble(&rubble);
            let Hunting::Open(mut serial) = Hunt::start_bounded(
                &mut probe,
                tank(),
                [50, 72].map(Fx::from_int),
                1234,
                [220, 72],
                search(),
                &|_, _| true,
                1,
            ) else {
                panic!("bounded prelude must stay open");
            };
            let mut prepared = serial.clone();
            drop(probe);
            for tick in 0..3000 {
                if tick == 1 {
                    w.queue_edit(voxel::Edit::put(70, 20, 72, 4));
                    w.flush();
                    let mut changed = Changed::default();
                    changed.column(70, 72);
                    serial.forget(&changed);
                    prepared.forget(&changed);
                }
                let a = std::cell::RefCell::new(Vec::new());
                let b = std::cell::RefCell::new(Vec::new());
                let (answers, columns) = prepared
                    .sweep_samples(budget)
                    .map(|(shape, samples)| {
                        batches += 1;
                        let mut probe = prepared.sweep_probe(&w, &rubble);
                        let mut poses = probe.route_samples(shape, &samples[..4]);
                        if prepared.sweep_prefix_open(&poses) {
                            let mut fork = probe.fork_route();
                            poses.extend(fork.route_samples(shape, &samples[4..]));
                            (
                                samples.into_iter().zip(poses).collect::<Vec<_>>(),
                                Some(fork.take_columns()),
                            )
                        } else {
                            (
                                samples[..4].iter().copied().zip(poses).collect::<Vec<_>>(),
                                Some(probe.take_columns()),
                            )
                        }
                    })
                    .unwrap_or_default();
                let x = serial.advance_in(
                    &w,
                    &rubble,
                    &|p, h| {
                        a.borrow_mut().push((p, h));
                        p[0] < Fx::from_int(210)
                    },
                    budget,
                );
                let y = prepared.advance_in_prepared(
                    &w,
                    &rubble,
                    &|p, h| {
                        b.borrow_mut().push((p, h));
                        p[0] < Fx::from_int(210)
                    },
                    budget,
                    PreparedSweep {
                        poses: answers,
                        columns,
                        reads: [0; 2],
                    },
                );
                assert_eq!(*a.borrow(), *b.borrow(), "budget={budget} tick={tick}");
                let mut left = fx::blob::Blob::new();
                let mut right = fx::blob::Blob::new();
                serial.save(&mut left);
                prepared.save(&mut right);
                assert_eq!(left.v, right.v, "budget={budget} tick={tick}");
                assert_eq!(x.is_some(), y.is_some());
                if let Some(x) = x {
                    let y = y.unwrap();
                    assert_eq!(
                        (x.path, x.partial, x.stranded, x.nodes, x.poses, x.exhausted),
                        (y.path, y.partial, y.stranded, y.nodes, y.poses, y.exhausted)
                    );
                    break;
                }
                assert!(tick < 2999, "search did not finish");
            }
        }
        assert!(batches > 0);
    }
    #[test]
    fn bounded_column_cache_survives_eviction_transfer_and_world_edits() {
        let mut w = World::new(
            1024,
            64,
            1024,
            100,
            Box::new(TestGround {
                rock: true,
                corridor: false,
                hill: true,
            }),
        );
        let mut cached = Probe::for_search(&w, &[], Columns::default());
        let mut reference = Probe::new(&w);
        for x in (0..1024).step_by(13) {
            for z in (0..1024).step_by(17) {
                assert_eq!(cached.column(x, z), reference.column(x, z));
            }
        }
        assert_eq!(cached.pages.len(), PAGE_SLOTS);
        let columns = cached.take_columns();
        assert!(cached.pages.is_empty());
        let mut resumed = Probe::for_search(&w, &[], columns);
        for x in (0..256).rev() {
            assert_eq!(resumed.floor(x, 72), reference.floor(x, 72));
        }
        drop(resumed);
        drop(cached);
        drop(reference);
        w.queue_dig(90, 29, 72);
        w.flush();
        let mut fresh = Probe::for_search(&w, &[], Columns::default());
        assert_eq!(fresh.floor(90, 72), Probe::new(&w).floor(90, 72));
    }
    #[test]
    fn forked_column_pages_preserve_answers_across_evictions_and_edits() {
        let mut w = World::new(
            1024,
            64,
            1024,
            100,
            Box::new(TestGround {
                rock: true,
                corridor: false,
                hill: true,
            }),
        );
        let mut root = Probe::for_search(&w, &[], Columns::default());
        for x in (0..1024).step_by(17) {
            assert_eq!(root.floor(x, 100), Probe::new(&w).floor(x, 100));
        }
        root.floor(100, 100);
        let columns = root.take_columns();
        drop(root);
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..4)
                .map(|offset| {
                    let columns = columns.clone();
                    let world = &w;
                    scope.spawn(move || {
                        let mut fork = Probe::for_search(world, &[], columns);
                        let mut plain = Probe::new(world);
                        for x in (offset..1024).step_by(7) {
                            for z in (0..1024).step_by(23) {
                                assert_eq!(fork.column(x, z), plain.column(x, z));
                            }
                        }
                    })
                })
                .collect();
            for job in jobs {
                job.join().unwrap();
            }
        });
        let mut resumed = Probe::for_search(&w, &[], columns);
        assert_eq!(resumed.floor(100, 100), Probe::new(&w).floor(100, 100));
        let columns = resumed.take_columns();
        drop(resumed);
        w.queue_edit(voxel::Edit::put(100, 40, 100, 4));
        w.flush();
        let mut changed = Changed::default();
        changed.column(100, 100);
        let mut resumed = Probe::for_local(&w, &[], columns, &changed);
        assert_eq!(resumed.floor(100, 100), 41);
        assert_eq!(resumed.column(100, 100), Probe::new(&w).column(100, 100));
    }
    #[test]
    fn local_column_cache_forgets_world_and_rubble_changes() {
        let mut w = world(false, false, false);
        let mut probe = Probe::for_local(&w, &[], Columns::default(), &Changed::default());
        assert_eq!(probe.floor(100, 100), 16);
        let columns = probe.take_columns();
        drop(probe);

        w.queue_dig(100, 15, 100);
        w.flush();
        let mut changed = Changed::default();
        changed.column(100, 100);
        let mut probe = Probe::for_local(&w, &[], columns, &changed);
        assert_eq!(probe.floor(100, 100), 15);
        let columns = probe.take_columns();
        drop(probe);

        let rubble = Rubble::from([([100, 100], 40)]);
        let mut probe = Probe::for_local(&w, &[], columns, &changed).with_rubble(&rubble);
        assert_eq!(probe.floor(100, 100), 40);
    }
    #[test]
    fn old_frontiers_and_kilometre_search_dimensions_load() {
        let w = world(false, false, false);
        let mut ground = Probe::new(&w);
        let Hunting::Open(mut hunt) = Hunt::start(
            &mut ground,
            tank(),
            [50, 72].map(Fx::from_int),
            0,
            [210, 72],
            search(),
            &|p, _| p[0] < Fx::from_int(100) || p[0] > Fx::from_int(120),
        ) else {
            panic!("the direct road crosses a blocker");
        };
        assert!(hunt.prelude.is_none() && hunt.connection.is_none());
        let mut bytes = fx::blob::Blob::new();
        hunt.save(&mut bytes);
        // Legacy searches had no marker, pending flags or exact start heading.
        let old = &bytes.v[4..bytes.v.len() - 7];
        let loaded = Hunt::load(&mut fx::blob::Scan::new(old), &[]).unwrap();
        let mut check = fx::blob::Blob::new();
        loaded.save(&mut check);
        assert_eq!(bytes.v, check.v);
        hunt.n = 1250;
        hunt.rows = 1250;
        let mut large = fx::blob::Blob::new();
        hunt.save(&mut large);
        assert!(Hunt::load(&mut fx::blob::Scan::new(&large.v), &[]).is_some());
    }
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
    #[test]
    fn bounded_crush_preserves_column_order_and_full_query() {
        let mut w = world(false, false, false);
        for x in 110..145 {
            w.queue_edit(voxel::Edit::put(x, 16, 128, 4));
        }
        w.flush();
        let at = [Fx::from_int(128); 2];
        let mut probe = Probe::with_yield(&w, &[4, 5]);
        let full = probe.crushed(at, 0, tank());
        assert!(full.len() > 10);
        let prefix = probe.crushed_limit(at, 0, tank(), 10);
        assert_eq!(prefix, full[..10]);
        assert!(probe.crushed_limit(at, 0, tank(), 0).is_empty());
        assert_eq!(probe.crushed_limit(at, 0, tank(), usize::MAX), full);
    }
    #[test]
    fn prepared_flat_pose_matches_full_contacts_and_invalidates_on_edit() {
        let mut w = world(false, false, false);
        w.prepare_navigation([4, 5]);
        let s = tank();
        let at = [Fx::from_int(128), Fx::from_int(128)];
        assert_eq!(
            w.prepared_flat_area([128 - reach(s); 2], [128 + reach(s); 2], [4, 5]),
            Some(16)
        );
        let mut shortcut = Probe::with_yield(&w, &[4, 5]);
        shortcut.route_pose(at, 0, s);
        assert_eq!(shortcut.samples, 0, "flat pose still scanned contacts");
        let mut driven = Probe::with_yield(&w, &[4, 5]);
        driven.pose(at, 0, s);
        driven.support_floors(at, 0, s);
        assert_eq!(driven.samples, 0, "flat drive still scanned contacts");
        assert!(
            driven.heights.is_empty(),
            "flat drive allocated a column cache"
        );
        let compare = |w: &World, rubble: &Rubble| {
            for pos in [at, [Fx::from_int(126), Fx::from_int(130)]] {
                for head in [0, angle::FULL / 8, angle::FULL / 4, angle::FULL * 3 / 8] {
                    let mut fast = Probe::with_yield(w, &[4, 5]).with_rubble(rubble);
                    let mut full = Probe::with_yield(w, &[4, 5]).with_rubble(rubble);
                    let a = fast.route_pose(pos, head, s);
                    let b = full.probe_pose(pos, head, s, true);
                    assert_eq!(
                        (a.height, a.floors, a.blocked, a.steep, a.tilt, a.crush),
                        (b.height, b.floors, b.blocked, b.steep, b.tilt, b.crush)
                    );
                    let mut driven = Probe::with_yield(w, &[4, 5]).with_rubble(rubble);
                    let mut driven_full = Probe::with_yield(w, &[4, 5]).with_rubble(rubble);
                    let a = driven.pose(pos, head, s);
                    let b = driven_full.probe_pose(pos, head, s, false);
                    assert_eq!(
                        (a.height, a.floors, a.blocked, a.steep, a.tilt, a.crush),
                        (b.height, b.floors, b.blocked, b.steep, b.tilt, b.crush)
                    );
                }
            }
        };
        compare(&w, &Rubble::new());
        let mut distant_rubble = Rubble::new();
        distant_rubble.insert([20, 20], 32);
        let mut shortcut = Probe::with_yield(&w, &[4, 5]).with_rubble(&distant_rubble);
        shortcut.route_pose(at, 0, s);
        assert_eq!(shortcut.samples, 0, "distant rubble disabled the shortcut");
        compare(&w, &distant_rubble);
        let mut nearby_rubble = Rubble::new();
        nearby_rubble.insert([128, 128], 32);
        compare(&w, &nearby_rubble);
        w.queue_edit(voxel::Edit::put(128, 30, 128, 3));
        w.flush();
        assert_eq!(
            w.prepared_flat_area([128 - reach(s); 2], [128 + reach(s); 2], [4, 5]),
            None
        );
        compare(&w, &Rubble::new());
        let mut rubble = Rubble::new();
        rubble.insert([128, 128], 32);
        compare(&w, &rubble);
    }
    #[test]
    fn prepared_bare_grade_skips_a_wide_hulls_exact_footprint() {
        struct Grade;
        impl voxel::Generator for Grade {
            fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
                if y < 10 + x / 16 {
                    1
                } else {
                    0
                }
            }
        }
        let mut w = World::new(384, 96, 384, 100, Box::new(Grade));
        w.prepare_navigation([4, 5]);
        let s = Shape {
            half: [88, 80],
            support: [50, 60],
            clearance: Fx::from_int(38),
            step: 14,
            min_up: Fx::from_ratio(84, 100),
            span: 12,
            ramp: Fx::from_ratio(1, 2),
            ..tank()
        };
        let pos = [Fx::from_int(192); 2];
        assert_eq!(w.prepared_flat_area([100; 2], [284; 2], [4, 5]), None);
        assert!(w.prepared_bare_upper([100; 2], [284; 2], [4, 5]).is_some());
        let distant: Vec<_> = (100..140)
            .flat_map(|x| (0..30).map(move |z| ([x, z], 50)))
            .collect();
        let mut rubble = Rubble::from_sorted(distant);
        assert!(!rubble.touches([100, 100], [284, 284]));
        for head in [0, angle::FULL / 8] {
            let mut fast = Probe::with_yield(&w, &[4, 5]).with_rubble(&rubble);
            let mut exact = Probe::with_yield(&w, &[5, 4]).with_rubble(&rubble);
            let a = fast.route_pose(pos, head, s);
            let b = exact.route_pose(pos, head, s);
            assert_eq!(
                (a.height, a.floors, a.blocked, a.steep, a.tilt, a.crush),
                (b.height, b.floors, b.blocked, b.steep, b.tilt, b.crush)
            );
            assert!(
                fast.columns < 2000,
                "wide grade still scanned its footprint: head={head}, fast={}, exact={}",
                fast.columns,
                exact.columns
            );
            assert!(exact.columns > 10_000);
        }
        w.queue_edit(voxel::Edit::put(192, 40, 192, 4));
        w.flush();
        w.prepare_navigation([4, 5]);
        assert_eq!(w.prepared_bare_upper([100; 2], [284; 2], [4, 5]), None);
        let mut fast = Probe::with_yield(&w, &[4, 5]).with_rubble(&rubble);
        let mut exact = Probe::with_yield(&w, &[5, 4]).with_rubble(&rubble);
        let a = fast.route_pose(pos, 0, s);
        let b = exact.route_pose(pos, 0, s);
        assert_eq!(
            (a.height, a.floors, a.blocked, a.steep, a.tilt, a.crush),
            (b.height, b.floors, b.blocked, b.steep, b.tilt, b.crush)
        );
        assert!(fast.columns < 4000, "one tree scanned the whole hull");
        assert!(exact.columns > 10_000);
        rubble.insert([192, 192], 50);
        assert!(rubble.touches([100, 100], [284, 284]));
        let mut near = Probe::with_yield(&w, &[4, 5]).with_rubble(&rubble);
        let mut exact = Probe::with_yield(&w, &[5, 4]).with_rubble(&rubble);
        let a = near.pose(pos, 0, s);
        let b = exact.pose(pos, 0, s);
        assert_eq!(
            (a.height, a.floors, a.blocked, a.steep, a.tilt, a.crush),
            (b.height, b.floors, b.blocked, b.steep, b.tilt, b.crush)
        );
        assert!(
            near.columns < 4000,
            "one rubble tile scanned the whole hull: {}",
            near.columns
        );
        assert!(exact.columns > 10_000);
        for top in [16, 50, 82] {
            for at in [[108, 180], [192, 192], [250, 220]] {
                let rubble = Rubble::from([(at, top)]);
                for head in [0, 1234, angle::FULL / 8, angle::FULL / 3] {
                    let mut fast = Probe::with_yield(&w, &[4, 5]).with_rubble(&rubble);
                    let mut exact = Probe::with_yield(&w, &[5, 4]).with_rubble(&rubble);
                    let p = [
                        pos[0].add(Fx::from_ratio(1, 3)),
                        pos[1].sub(Fx::from_ratio(1, 5)),
                    ];
                    let a = fast.pose(p, head, s);
                    let b = exact.pose(p, head, s);
                    assert_eq!(
                        (a.height, a.floors, a.blocked, a.steep, a.tilt, a.crush),
                        (b.height, b.floors, b.blocked, b.steep, b.tilt, b.crush),
                        "top={top} at={at:?} head={head}"
                    );
                }
            }
        }
        w.queue_edit(voxel::Edit::put(192, 60, 192, 2));
        w.flush();
        assert_eq!(w.prepared_bare_upper([192, 192], [199, 199], [4, 5]), None);
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
    #[test]
    fn hump_weights_match_fixed_point_division_and_refresh_with_shape() {
        let w = world(true, true, true);
        let pos = [
            Fx::from_int(126).add(Fx::from_ratio(1, 3)),
            Fx::from_int(110).add(Fx::from_ratio(2, 5)),
        ];
        let mut reused = Probe::around(&w, &[], pos, 96);
        for (span, support) in [(25, 25), (8, 9), (14, 25), (2, 9), (25, 25)] {
            let shape = Shape {
                span,
                support: [support, 8],
                ..tank()
            };
            let fade = (span / 2).max(1);
            for head in [0, angle::FULL / 7, angle::FULL / 4] {
                let actual = reused.contacts(pos, head, shape);
                let expected = Probe::around(&w, &[], pos, 96).contacts(pos, head, shape);
                assert_eq!(actual, expected);
                assert_eq!(reused.hump_key, (support, fade));
                for u in -support + 1..support {
                    assert_eq!(
                        reused.hump_weights[(u + support - 1) as usize],
                        (
                            Fx::from_int(u + support).div(Fx::from_int(2 * support.max(1))),
                            Fx::from_int((support - u.abs()).min(fade)).div(Fx::from_int(fade)),
                        )
                    );
                }
            }
        }
    }
    #[test]
    fn suspension_floors_match_full_poses_without_scanning_the_belly() {
        let terrains = [
            world(true, true, true),
            World::new(256, 64, 256, 100, Box::new(Cut)),
            World::new(256, 64, 256, 100, Box::new(Forest)),
        ];
        let rubble = Rubble::from([([100, 100], 26), ([126, 72], 36)]);
        for w in &terrains {
            for yielding in [&[][..], &[7, 8][..]] {
                for span in [0, 1, 25] {
                    let mut shape = tank();
                    shape.span = span;
                    for at in [[100, 100], [126, 72], [180, 100], [2, 2], [254, 254]] {
                        let pos = at.map(|v| Fx::from_int(v).add(Fx::from_ratio(1, 3)));
                        for head in [0, angle::FULL / 7, angle::FULL / 4] {
                            let mut full = Probe::around(w, yielding, pos, 96).with_rubble(&rubble);
                            let mut contacts =
                                Probe::around(w, yielding, pos, 96).with_rubble(&rubble);
                            assert_eq!(
                                contacts.support_floors(pos, head, shape),
                                full.support_pose(pos, head, shape).floors,
                                "at={at:?}, head={head}, span={span}, yielding={yielding:?}"
                            );
                            assert!(contacts.columns < full.columns);
                            assert!(contacts.scans <= full.scans);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn suspension_observes_a_support_removed_before_the_next_tick() {
        let mut w = world(false, false, false);
        let pos = [Fx::from_int(100); 2];
        let before = Probe::new(&w).support_floors(pos, 0, tank());
        for z in 85..96 {
            for x in 69..82 {
                for y in 10..16 {
                    w.queue_dig(x, y, z);
                }
            }
        }
        while w.queued() > 0 {
            w.flush();
        }
        let after = Probe::new(&w).support_floors(pos, 0, tank());
        assert!(after[0] < before[0], "a removed support kept its old floor");
        assert_eq!(after, Probe::new(&w).support_pose(pos, 0, tank()).floors);
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
    /// Changes are found by cache page, and both the pages and the
    /// rubble's prints survive a save; a page off the map is refused
    /// (ADR-427).
    #[test]
    fn a_change_is_found_by_its_page_and_survives_a_save() {
        let mut c = Changed::default();
        assert!(c.is_empty());
        c.column(40, 70);
        assert!(c.touches([32, 64], [32, 64]));
        assert!(c.touches([0, 0], [100, 100]));
        assert!(!c.touches([0, 0], [31, 200]));
        assert!(!c.touches([64, 0], [200, 200]));
        assert!(Changed::everywhere().touches([0, 0], [0, 0]));
        let mut b = fx::blob::Blob::new();
        c.save(&mut b);
        let back = Changed::load(&mut fx::blob::Scan::new(&b.v), [128, 128]);
        assert_eq!(back, Some(c.clone()));
        assert!(Changed::load(&mut fx::blob::Scan::new(&b.v), [32, 32]).is_none());

        let old = Rubble::from([([40, 70], 20), ([100, 5], 3)]);
        let mut new = old.clone();
        new.insert([41, 70], 18);
        new.remove(&[100, 5]);
        // Off the map: no probe reads it, no page changes.
        new.insert([500, 5], 9);
        let (before, after) = (
            RubblePrint::of(&old, [128, 128]),
            RubblePrint::of(&new, [128, 128]),
        );
        let mut changed = Changed::default();
        before.changes(&after, &mut changed);
        assert_eq!(changed.pages, BTreeSet::from([[1, 2], [3, 0]]));
        let mut none = Changed::default();
        after.changes(&after.clone(), &mut none);
        assert!(none.is_empty());
        let mut b = fx::blob::Blob::new();
        after.save(&mut b);
        let back = RubblePrint::load(&mut fx::blob::Scan::new(&b.v), [128, 128]);
        assert_eq!(back, Some(after));
        assert!(RubblePrint::load(&mut fx::blob::Scan::new(&b.v), [40, 40]).is_none());
    }
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
    struct Rim;
    impl voxel::Generator for Rim {
        fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
            // A 45-degree bank ten units long between two flats.
            let h = 16 + (x as i32 - 120).clamp(0, 10);
            u16::from((y as i32) < h)
        }
    }
    #[test]
    fn a_grade_steeper_than_the_tilt_limit_blocks_even_a_level_hull() {
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
    #[test]
    fn a_sweep_can_leave_a_steep_contact_but_cannot_enter_one() {
        let w = World::new(256, 64, 256, 100, Box::new(Rim));
        for (from, to, head, open) in [
            ([93, 100], [70, 100], angle::FULL / 2, true),
            ([70, 100], [93, 100], 0, false),
            ([93, 100], [130, 100], 0, true), // already on the grade
        ] {
            let from = from.map(Fx::from_int);
            let to = to.map(Fx::from_int);
            let expected = connects(&mut Probe::new(&w), tank(), from, head, to, &|_, _| true);
            assert_eq!(expected.is_some(), open, "from={from:?}, to={to:?}");
            for budget in [1, 7, 96] {
                let mut run = Connection::new(from, head, to);
                let mut probe = Probe::new(&w);
                let mut finished = false;
                for _ in 0..200 {
                    let mut left = budget;
                    let answer = run.advance(&mut probe, tank(), &|_, _| true, &mut left);
                    let mut bytes = fx::blob::Blob::new();
                    run.save(&mut bytes);
                    run = Connection::load(&mut fx::blob::Scan::new(&bytes.v), true).unwrap();
                    if let Some(answer) = answer {
                        assert_eq!(answer, expected);
                        finished = true;
                        break;
                    }
                }
                assert!(finished);
            }
        }
        // Exiting a grade does not excuse a blocked belly or a live obstacle.
        let from = [93, 100].map(Fx::from_int);
        let to = [70, 100].map(Fx::from_int);
        assert!(connects(
            &mut Probe::new(&w),
            tank(),
            from,
            angle::FULL / 2,
            to,
            &|p, _| p[0] > Fx::from_int(86)
        )
        .is_none());
        let mut rubble = Rubble::new();
        rubble.insert([80, 100], 26);
        assert!(connects(
            &mut Probe::new(&w).with_rubble(&rubble),
            tank(),
            from,
            angle::FULL / 2,
            to,
            &|_, _| true
        )
        .is_none());
    }

    #[test]
    fn a_padded_contact_does_not_let_a_flat_start_enter_a_steep_grade() {
        struct TallRim;
        impl voxel::Generator for TallRim {
            fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
                u16::from((y as i32) < 16 + (x as i32 - 120).clamp(0, 30))
            }
        }
        let w = World::new(256, 64, 256, 100, Box::new(TallRim));
        let mut probe = Probe::new(&w);
        let padded = Shape {
            span: tank().span + search().margin,
            ..tank()
        };
        let from = (60..100)
            .map(|x| [x, 100].map(Fx::from_int))
            .find(|&at| !probe.pose(at, 0, tank()).steep && probe.pose(at, 0, padded).steep)
            .expect("margin must reach the bank before the actual belt");
        assert!(
            probe.pose(from, 0, padded).steep,
            "padding must reach the bank"
        );
        let result = plan(
            Probe::new(&w),
            tank(),
            from,
            0,
            [200, 100],
            search(),
            |_, _| true,
        );
        assert_ne!(
            result.path.last(),
            Some(&[200, 100]),
            "false recovery crossed the bank"
        );
        let Hunting::Open(hunt) = Hunt::start_bounded(
            &mut Probe::new(&w),
            tank(),
            from,
            0,
            [200, 100],
            search(),
            &|_, _| true,
            1,
        ) else {
            panic!("bounded target prelude should stay open");
        };
        assert_eq!(hunt.cfg.margin, search().margin);
        assert_eq!(hunt.shape.span, padded.span);
    }

    #[test]
    fn steep_start_searches_match_reloaded_and_prepared_slices() {
        let w = World::new(256, 64, 256, 100, Box::new(Rim));
        let rubble = Rubble::new();
        let clear = |p: [Fx; 2], _: i32| {
            !(p[0] > Fx::from_int(75)
                && p[0] < Fx::from_int(80)
                && p[1] > Fx::from_int(90)
                && p[1] < Fx::from_int(110))
        };
        let from = [93, 100].map(Fx::from_int);
        let head = angle::FULL / 2;
        let mut batches = 0;
        for budget in [1, 7, 32, 96] {
            let Hunting::Open(mut live) = Hunt::start_bounded(
                &mut Probe::new(&w),
                tank(),
                from,
                head,
                [40, 100],
                search(),
                &clear,
                1,
            ) else {
                panic!("the detour must remain pending");
            };
            assert_eq!(
                live.shape.span,
                tank().span,
                "escape must use the executor's exact grade contacts"
            );
            for tick in 0..10000 {
                let mut bytes = fx::blob::Blob::new();
                live.save(&mut bytes);
                let mut restored = Hunt::load(&mut fx::blob::Scan::new(&bytes.v), &[]).unwrap();
                let prepared = restored
                    .sweep_samples(budget)
                    .map(|(shape, samples)| {
                        batches += 1;
                        let mut probe = restored.sweep_probe(&w, &rubble);
                        let poses = probe.route_samples(shape, &samples);
                        PreparedSweep {
                            poses: samples.into_iter().zip(poses).collect(),
                            columns: Some(probe.take_columns()),
                            reads: [0; 2],
                        }
                    })
                    .unwrap_or_default();
                let before = live.spent;
                let result = live.advance_in(&w, &rubble, &clear, budget);
                assert!(live.spent - before <= budget);
                let other = restored.advance_in_prepared(&w, &rubble, &clear, budget, prepared);
                assert_eq!(result.is_some(), other.is_some());
                let mut a = fx::blob::Blob::new();
                let mut b = fx::blob::Blob::new();
                live.save(&mut a);
                restored.save(&mut b);
                assert_eq!(a.v, b.v, "budget={budget}, tick={tick}");
                if let Some(result) = result {
                    let other = other.unwrap();
                    assert_eq!(
                        (&result.path, result.partial, result.stranded),
                        (&other.path, other.partial, other.stranded)
                    );
                    assert!(!result.path.is_empty() && !result.stranded);
                    let mut probe = Probe::new(&w);
                    let mut at = from;
                    let mut yaw = head;
                    for point in &result.path {
                        let to = point.map(Fx::from_int);
                        assert!(connects(&mut probe, tank(), at, yaw, to, &clear).is_some());
                        yaw = angle::atan2(to[1].sub(at[1]).raw(), to[0].sub(at[0]).raw());
                        at = to;
                    }
                    assert!(!probe.pose(at, yaw, tank()).refused());
                    break;
                }
                assert!(tick < 9999);
            }
        }
        assert!(batches > 0);
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
    fn flat_tile_floor_shortcut_matches_columns_after_edits_and_rubble() {
        let mut w = World::new(256, 64, 256, 100, Box::new(Forest));
        w.prepare_navigation([7, 8]);
        let mut rubble = Rubble::new();
        rubble.insert([113, 98], 21);
        let check = |w: &World, rubble: &Rubble| {
            let probe = Probe::with_yield(w, &[7, 8]).with_rubble(rubble);
            for z in 96..105 {
                for x in 112..133 {
                    let mut expected = (0, [u16::MAX, 0]);
                    for y in (0..w.column_ceiling(x, z)).rev() {
                        if !w.solid(x, y, z) {
                            continue;
                        }
                        if [7, 8].contains(&w.material(x, y, z)) {
                            expected.1[0] = expected.1[0].min(y as u16);
                            expected.1[1] = expected.1[1].max(y as u16);
                        } else {
                            expected.0 = y as u16 + 1;
                            break;
                        }
                    }
                    expected.0 = expected
                        .0
                        .max(rubble.get(&[x as i32, z as i32]).copied().unwrap_or(0));
                    assert_eq!(probe.scan(x as i32, z as i32), expected, "column {x},{z}");
                }
            }
        };
        check(&w, &rubble);
        w.queue_edit(voxel::Edit::put(113, 27, 98, 3));
        w.flush();
        check(&w, &rubble);
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
    fn partial_path_does_not_end_in_an_already_explored_dead_end() {
        let w = world(false, false, false);
        // A narrow east-facing pocket points toward the goal, but the
        // usable detour starts before it. A hull cannot pivot in the pocket.
        let clear = |p: [Fx; 2], h: i32| {
            let x = p[0].to_int_trunc();
            let z = p[1].to_int_trunc();
            !(x > 80 && x < 180 && (94..=106).contains(&z)) || (x < 120 && h == 0)
        };
        let cfg = Search {
            nodes: 512,
            ..search()
        };
        let answer = plan(
            Probe::new(&w),
            tank(),
            [60, 100].map(Fx::from_int),
            0,
            [220, 100],
            cfg,
            clear,
        );
        assert!(
            answer.partial,
            "expected a bounded prefix: {:?}",
            answer.path
        );
        let end = *answer.path.last().unwrap();
        assert!(
            !(end[0] > 80 && end[0] < 120 && (94..=106).contains(&end[1])),
            "prefix enters an explored dead end: {:?}",
            answer.path
        );
    }
    #[test]
    fn bounded_detour_can_start_by_moving_farther_from_the_goal() {
        let w = world(false, false, false);
        let clear = |p: [Fx; 2], _: i32| {
            !(p[0] > Fx::from_int(120)
                && p[0] < Fx::from_int(192)
                && p[1] > Fx::from_int(64)
                && p[1] < Fx::from_int(136))
        };
        let cfg = Search {
            nodes: 512,
            ..search()
        };
        let goal = [220, 100];
        let mut at = [120, 100].map(Fx::from_int);
        let mut head = 0;
        for prefixes in 0..16 {
            let answer = plan(Probe::new(&w), tank(), at, head, goal, cfg, clear);
            assert!(!answer.path.is_empty(), "detour stalled at {at:?}");
            if prefixes == 0 {
                assert!(answer.partial);
                let end = *answer.path.last().unwrap();
                assert!(
                    (end[0] - goal[0]).pow(2) + (end[1] - goal[1]).pow(2) > 100 * 100,
                    "fixture must require moving away first: {:?}",
                    answer.path
                );
            }
            let mut probe = Probe::new(&w);
            let shape = Shape {
                span: tank().span + cfg.margin,
                ..tank()
            };
            for point in &answer.path {
                let to = point.map(Fx::from_int);
                assert!(connects(&mut probe, shape, at, head, to, &clear).is_some());
                head = angle::atan2(to[1].sub(at[1]).raw(), to[0].sub(at[0]).raw());
                at = to;
            }
            if !answer.partial {
                assert_eq!(at, goal.map(Fx::from_int));
                assert!(prefixes > 0);
                return;
            }
        }
        panic!("detour did not finish: {at:?}");
    }
    #[test]
    fn spent_budget_returns_checked_progress_and_restores_the_same_endpoint() {
        let w = World::new(256, 64, 256, 100, Box::new(Forest));
        let clear = |p: [Fx; 2], _: i32| {
            !((110..130).contains(&p[0].to_int_trunc()) && (70..130).contains(&p[1].to_int_trunc()))
        };
        let cfg = Search {
            nodes: 512,
            ..search()
        };
        let from = [60, 100].map(Fx::from_int);
        let to = [200, 100];
        let reference = plan(
            Probe::with_yield(&w, &[7, 8]),
            tank(),
            from,
            0,
            to,
            cfg,
            clear,
        );
        assert!(reference.partial && !reference.exhausted && !reference.path.is_empty());
        let endpoint = *reference.path.last().unwrap();
        assert_ne!(endpoint, to, "unchecked goal appended to a partial path");
        assert!(endpoint[0] > 60 && endpoint[0] < 200);
        let mut probe = Probe::for_search(&w, &[7, 8], Columns::default());
        let Hunting::Open(mut live) =
            Hunt::start_bounded(&mut probe, tank(), from, 0, to, cfg, &clear, 7)
        else {
            panic!("the search should be suspended");
        };
        let mut slices = 0;
        let actual = loop {
            let mut bytes = fx::blob::Blob::new();
            live.save(&mut bytes);
            let mut loaded = Hunt::load(&mut fx::blob::Scan::new(&bytes.v), &[7, 8]).unwrap();
            let a = live.advance_in(&w, &Rubble::new(), &clear, 7);
            let b = loaded.advance_in(&w, &Rubble::new(), &clear, 7);
            assert_eq!(a.is_some(), b.is_some());
            slices += 1;
            assert!(slices < 1000);
            if let Some(a) = a {
                let b = b.unwrap();
                assert_eq!(a.path, b.path);
                assert_eq!(a.partial, b.partial);
                break a;
            }
        };
        assert!(actual.partial);
        assert_eq!(actual.path, reference.path);
        let mut at = from;
        let mut head = 0;
        let shape = Shape {
            span: tank().span + cfg.margin,
            ..tank()
        };
        for p in actual.path {
            let to = p.map(Fx::from_int);
            assert!(connects(&mut probe, shape, at, head, to, &clear).is_some());
            head = angle::atan2(to[1].sub(at[1]).raw(), to[0].sub(at[0]).raw());
            at = to;
        }
        // A caller using the legacy path-only API never mistakes a partial
        // route for arrival at its requested destination.
        assert!(route(
            Probe::with_yield(&w, &[7, 8]),
            tank(),
            from,
            0,
            to,
            cfg,
            clear
        )
        .is_empty());
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
    /// **A snapshot of a search whose cells do not lead back to its start,
    /// or that spent more than its budget, does not load** (ADR-432).
    /// The route walk followed such a chain off the grid or round a loop
    /// forever, and the slice budget underflowed.
    #[test]
    fn a_corrupt_search_snapshot_is_refused() {
        let w = world(false, true, false);
        let mut ground = Probe::for_search(&w, &[], Columns::default());
        let Hunting::Open(hunt) = Hunt::start_bounded(
            &mut ground,
            tank(),
            [Fx::from_int(70); 2],
            0,
            [200, 200],
            search(),
            &|_, _| true,
            24,
        ) else {
            panic!("the search answered at once");
        };
        let mut hunt = *hunt;
        while hunt.distance.len() < 4 {
            assert!(hunt
                .advance_in(&w, &Rubble::new(), &|_, _| true, 24)
                .is_none());
        }
        let bytes = |h: &Hunt| {
            let mut b = fx::blob::Blob::new();
            h.save(&mut b);
            b.v
        };
        assert!(Hunt::load(&mut fx::blob::Scan::new(&bytes(&hunt)), &[]).is_some());
        let reached: Vec<usize> = hunt
            .distance
            .sorted_keys()
            .into_iter()
            .filter(|&i| i != hunt.initial)
            .collect();
        let mut gap = hunt.clone();
        gap.previous.remove(&reached[0]);
        assert!(Hunt::load(&mut fx::blob::Scan::new(&bytes(&gap)), &[]).is_none());
        let mut ring = hunt.clone();
        ring.previous.insert(reached[0], reached[1]);
        ring.previous.insert(reached[1], reached[0]);
        assert!(Hunt::load(&mut fx::blob::Scan::new(&bytes(&ring)), &[]).is_none());
        let mut spent = hunt.clone();
        spent.spent = spent.cfg.nodes + 1;
        assert!(Hunt::load(&mut fx::blob::Scan::new(&bytes(&spent)), &[]).is_none());
    }
    /// **A route that drives out of a gap and back keeps its pivot**
    /// (ADR-432). The hull faces east inside a gap too narrow to turn in,
    /// and the goal lies west: the search drives it out, pivots and comes
    /// back. Every point lies on one line, and a reversal has a zero cross
    /// product like a straight run, so the simplification dropped the
    /// pivot and asked for a turn inside the gap the planner had refused.
    #[test]
    fn a_route_that_turns_back_keeps_its_pivot() {
        let w = world(false, true, false);
        let path = route(
            Probe::new(&w),
            tank(),
            [Fx::from_int(70); 2],
            0,
            [40, 70],
            search(),
            |_, _| true,
        );
        assert!(!path.is_empty(), "no route out of the gap and back");
        assert_eq!(path.last(), Some(&[40, 70]));
        assert!(
            path.iter().any(|p| p[0] > 82),
            "the route turns about inside the gap: {path:?}"
        );
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
            Search {
                nodes: 1_000_000,
                ..search()
            },
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

#[cfg(test)]
#[path = "vehicle_clearance_tests.rs"]
mod clearance_tests;
