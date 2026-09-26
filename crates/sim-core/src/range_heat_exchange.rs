//! Short-range contact heat across independently moving voxel volumes.
//! Six axial probes, exact local voxel traversal, old-field reads, finite heat.
use super::{
    thermal::{temperature, Cell},
    BlastRange,
};
use crate::damage::{Frame, V3};
use fx::{
    blob::{Blob, Scan},
    Fx,
};
use std::collections::{BTreeMap, BTreeSet};
use voxel::{
    impact::ray_cells,
    unit::{UnitBody, WORLD_MM},
};

const CAP: u32 = 1_000_000;
type Bounds = ([i32; 3], [i32; 3]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Site {
    World([u32; 3]),
    Body(Cell),
}

#[derive(Default)]
pub struct Exchange {
    world_cursor: Option<[u32; 3]>,
    body_cursor: Option<Cell>,
    pub transferred: u64,
    /// Transient diagnostics: source visits, including cold or removed entries.
    pub visited: usize,
}
impl Exchange {
    pub fn save(&self, b: &mut Blob) {
        b.bool(self.world_cursor.is_some());
        if let Some(p) = self.world_cursor {
            for v in p {
                b.u32(v);
            }
        }
        b.bool(self.body_cursor.is_some());
        if let Some(p) = self.body_cursor {
            p.save(b);
        }
        b.u64(self.transferred);
    }
    pub fn load(&mut self, s: &mut Scan, size: [u32; 3], units: &[super::RangeUnit]) -> Option<()> {
        self.world_cursor = if s.bool() {
            let p = [s.u32(), s.u32(), s.u32()];
            if (0..3).any(|a| p[a] >= size[a]) {
                return None;
            }
            Some(p)
        } else {
            None
        };
        self.body_cursor = if s.bool() {
            let p = Cell::load(s)?;
            let u = units.get(usize::from(p.unit))?;
            let b = if p.part == 0 {
                &u.body
            } else {
                &u.body.segments.get(usize::from(p.part - 1))?.body
            };
            let dims = [b.vol.nx, b.vol.ny, b.vol.nz];
            if (0..3).any(|a| p.at[a] >= dims[a]) {
                return None;
            }
            Some(p)
        } else {
            None
        };
        self.transferred = s.u64();
        s.ok().then_some(())
    }
}
impl Site {
    fn heat(self, r: &BlastRange) -> u32 {
        match self {
            Self::World(p) => r.heat.cell(p).heat,
            Self::Body(p) => r.thermals.heat(p),
        }
    }
    fn mm(self, r: &BlastRange) -> u16 {
        match self {
            Self::World(_) => WORLD_MM,
            Self::Body(p) => p.body(&r.units).map_or(WORLD_MM, |b| b.mm),
        }
    }
    fn centre(self, r: &BlastRange) -> Option<[Fx; 3]> {
        match self {
            Self::World(p) => (r.world.solid(p[0], p[1], p[2]) && !r.matter.debris.claimed(p))
                .then_some(p.map(|n| Fx::from_int(n as i32).add(Fx::from_ratio(1, 2)))),
            Self::Body(p) => {
                let u = &r.units[usize::from(p.unit)];
                if u.section_holds(usize::from(p.part), p.at) {
                    return None;
                }
                p.world(&r.units)
            }
        }
    }
    fn accepts(self, r: &BlastRange) -> bool {
        match self {
            Self::Body(_) => self.centre(r).is_some(),
            Self::World(p) => {
                !r.matter.debris.claimed(p)
                    && r.matter
                        .economy
                        .mats
                        .get(r.world.material(p[0], p[1], p[2]))
                        .is_some_and(|m| m.kindling > 0 || m.melt > 0)
            }
        }
    }
    fn separate(self, other: Self) -> bool {
        match (self, other) {
            (Self::World(_), Self::World(_)) => false,
            (Self::Body(a), Self::Body(b)) => a.unit != b.unit || a.part != b.part,
            _ => true,
        }
    }
}

/// Ray parameter at entry to a voxel; ray_cells already proves intersection.
fn entry(from: [Fx; 3], to: [Fx; 3], p: [u8; 3]) -> Fx {
    let mut t = Fx::ZERO;
    for a in 0..3 {
        let d = to[a].sub(from[a]);
        if d != Fx::ZERO {
            let face = i32::from(p[a]) + i32::from(d < Fx::ZERO);
            t = t.max(Fx::from_int(face).sub(from[a]).div(d));
        }
    }
    t
}
fn vec(p: [Fx; 3]) -> V3 {
    V3::new(p[0], p[1], p[2])
}
fn intersects(bounds: Bounds, from: [Fx; 3], to: [Fx; 3]) -> bool {
    (0..3).all(|a| {
        from[a].min(to[a]) <= Fx::from_int(bounds.1[a])
            && from[a].max(to[a]) >= Fx::from_int(bounds.0[a])
    })
}
impl BlastRange {
    /// Closest solid across all volumes, including opaque neighbours in the
    /// source volume. Never skip armour just because local diffusion handles it.
    fn first_contact(
        &self,
        source: Option<Site>,
        from: [Fx; 3],
        to: [Fx; 3],
        bounds: &[Bounds],
    ) -> Option<(Fx, Option<Site>)> {
        let mut best: Option<(Fx, Option<Site>)> = None;
        let mut take = |t: Fx, site: Option<Site>| {
            if (source.is_none() || site != source) && best.is_none_or(|old| (t, site) < old) {
                best = Some((t, site));
            }
        };
        // Trace only a tiny translated world box; no grid allocation or walk
        // proportional to map size, and the same DDA as body penetration.
        let origin = std::array::from_fn::<_, 3, _>(|a| from[a].min(to[a]).to_int_trunc());
        let size =
            std::array::from_fn(|a| (from[a].max(to[a]).to_int_trunc() - origin[a] + 1) as u8);
        let local = |p: [Fx; 3]| std::array::from_fn(|a| p[a].sub(Fx::from_int(origin[a])));
        let (a, b) = (local(from), local(to));
        for p in ray_cells(size, a, b) {
            let q = std::array::from_fn::<_, 3, _>(|a| origin[a] + i32::from(p[a]));
            if (0..3).any(|a| q[a] < 0 || q[a] >= self.size[a] as i32) {
                continue;
            }
            let q = q.map(|n| n as u32);
            if Some(Site::World(q)) != source && self.world.solid(q[0], q[1], q[2]) {
                take(entry(a, b, p), Some(Site::World(q)));
                break;
            }
        }
        for (id, u) in self
            .units
            .iter()
            .chain(std::iter::once(&self.gun))
            .enumerate()
        {
            if !intersects(bounds[id], from, to) {
                continue;
            }
            let mut volume = |body: &UnitBody, frame: &Frame, part: u8| {
                let (a, b) = (frame.local(vec(from)), frame.local(vec(to)));
                for at in ray_cells([body.vol.nx, body.vol.ny, body.vol.nz], a, b) {
                    let p = Cell {
                        unit: id as u16,
                        part,
                        at,
                    };
                    // The player's range gun is an opaque body, even though
                    // its damage/heat ledger is not part of the target list.
                    let site = (id < self.units.len()).then_some(Site::Body(p));
                    if (source.is_none() || site != source)
                        && body.vol.get(at[0] as usize, at[1] as usize, at[2] as usize)
                    {
                        take(entry(a, b, at), site);
                        break;
                    }
                }
            };
            volume(&u.body, &u.pose, 0);
            for (i, s) in u.body.segments.iter().enumerate() {
                if !u.seg_gone[i] {
                    volume(&s.body, &u.seg_frames[i], i as u8 + 1);
                }
            }
        }
        best
    }
    /// Collision for a small physical spill. Caller bounds travel per tick.
    pub(super) fn fuel_collision(&self, from: [Fx; 3], to: [Fx; 3]) -> Option<Fx> {
        let bounds: Vec<_> = self
            .units
            .iter()
            .chain(std::iter::once(&self.gun))
            .map(|u| u.bounds())
            .collect();
        self.first_contact(None, from, to, &bounds).map(|(t, _)| t)
    }
    /// Chemical heat touches the first surface, including its own compartment.
    /// Heat with no receiver (or no field capacity) dissipates into the air.
    pub(super) fn deposit_fire_heat(&mut self, from: [Fx; 3], reach: u8, heat: u32) -> u32 {
        let bounds: Vec<_> = self
            .units
            .iter()
            .chain(std::iter::once(&self.gun))
            .map(|u| u.bounds())
            .collect();
        let mut contacts = BTreeSet::new();
        for a in 0..3 {
            for sign in [-1, 1] {
                let mut to = from;
                to[a] = to[a].add(Fx::from_int(sign * i32::from(reach)));
                if let Some((_, Some(site))) = self.first_contact(None, from, to, &bounds) {
                    if site.accepts(self) {
                        contacts.insert(site);
                    }
                }
            }
        }
        let mut accepted = 0;
        let count = contacts.len() as u32;
        for (i, site) in contacts.into_iter().enumerate() {
            let share = heat / count + u32::from((i as u32) < heat % count);
            let before = site.heat(self);
            match site {
                Site::World(p) => self.heat.add(&self.world, p, share),
                Site::Body(p) => self.thermals.deposit(p, share, &self.thermal_tuning),
            }
            accepted += site.heat(self) - before;
        }
        accepted
    }
    pub(super) fn exchange_heat(&mut self) {
        self.heat_exchange.visited = 0;
        let t = &self.thermal_tuning;
        if t.exchange_milli == 0 || (self.heat.hot() == 0 && self.thermals.hot() == 0) {
            return;
        }
        let world = self
            .heat
            .batch_after(self.heat_exchange.world_cursor, t.exchange_cells_per_tick);
        let body = self
            .thermals
            .batch_after(self.heat_exchange.body_cursor, t.exchange_cells_per_tick);
        self.heat_exchange.world_cursor = world.last().copied();
        self.heat_exchange.body_cursor = body.last().copied();
        self.heat_exchange.visited = world.len() + body.len();
        let bounds: Vec<_> = self
            .units
            .iter()
            .chain(std::iter::once(&self.gun))
            .map(|u| u.bounds())
            .collect();
        let mut outgoing = BTreeMap::<Site, u32>::new();
        let mut incoming = BTreeMap::<Site, u32>::new();
        let mut new = BTreeSet::new();
        let mut pairs = BTreeSet::new();
        for source in world
            .into_iter()
            .map(Site::World)
            .chain(body.into_iter().map(Site::Body))
        {
            let heat = source.heat(self);
            if u64::from(heat) * u64::from(t.exchange_milli) < 1000 {
                continue;
            }
            let Some(from) = source.centre(self) else {
                continue;
            };
            for axis in 0..3 {
                for sign in [-1, 1] {
                    let mut to = from;
                    to[axis] = to[axis].add(Fx::from_int(sign * i32::from(t.exchange_reach)));
                    let Some(dest) = self
                        .first_contact(Some(source), from, to, &bounds)
                        .and_then(|(_, p)| p)
                    else {
                        continue;
                    };
                    if !source.separate(dest)
                        || !dest.accepts(self)
                        || !pairs.insert((source, dest))
                    {
                        continue;
                    }
                    let hot = temperature(heat, source.mm(self));
                    let cold = temperature(dest.heat(self), dest.mm(self));
                    // Smaller physical volume bounds equilibration for unequal grids.
                    let volume = u128::from(source.mm(self).min(dest.mm(self))).pow(3);
                    let share = (u128::from(hot.saturating_sub(cold))
                        * u128::from(t.exchange_milli)
                        * volume
                        / (1000 * u128::from(WORLD_MM).pow(3)))
                    .min(u128::from(CAP)) as u32;
                    let available =
                        heat.saturating_sub(outgoing.get(&source).copied().unwrap_or(0));
                    let room = CAP
                        .saturating_sub(dest.heat(self))
                        .saturating_sub(incoming.get(&dest).copied().unwrap_or(0));
                    let share = share.min(available).min(room);
                    if share == 0 {
                        continue;
                    }
                    if let Site::Body(p) = dest {
                        if self.thermals.heat(p) == 0 && !new.contains(&p) {
                            if self.thermals.hot() + new.len() >= t.max_cells {
                                continue;
                            }
                            new.insert(p);
                        }
                    }
                    *outgoing.entry(source).or_default() += share;
                    *incoming.entry(dest).or_default() += share;
                }
            }
        }
        // Commit only after every probe has read the beginning of this phase.
        for (p, h) in outgoing {
            match p {
                Site::World(p) => self.heat.take_heat(p, h),
                Site::Body(p) => self.thermals.take_heat(p, h),
            }
            self.heat_exchange.transferred += u64::from(h);
        }
        for (p, h) in incoming {
            match p {
                Site::World(p) => self.heat.add(&self.world, p, h),
                Site::Body(p) => self.thermals.deposit(p, h, &self.thermal_tuning),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::RangeUnit;
    use super::*;
    use voxel::unit::Part;

    // Tiny separated volumes isolate exchange from conduction and ablation.
    fn cube(at: [i32; 3]) -> RangeUnit {
        let mut u = RangeUnit::new(at);
        u.body = UnitBody::new(1, 1, 1, 1);
        u.body.set(0, 0, 0, Part::Armour);
        u.seg_frames.clear();
        u.seg_gone.clear();
        u.pose = Frame::new(
            &u.body,
            V3::new(
                Fx::from_int(at[0]).add(Fx::from_ratio(1, 2)),
                Fx::from_int(at[1]),
                Fx::from_int(at[2]).add(Fx::from_ratio(1, 2)),
            ),
            0,
            V3::UP,
        );
        u
    }
    fn fixture(cubes: &[[i32; 3]], blocks: &[([i32; 3], u16)]) -> BlastRange {
        let c = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/flamethrower.ron"
        ))
        .unwrap();
        let mut layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        layout.course = None;
        layout.units.clear();
        layout.clear_min = [64, 64];
        layout.clear_max = [600, 600];
        layout.blocks = blocks
            .iter()
            .map(|(p, material)| modding::range::RangeBlock {
                min: *p,
                max: p.map(|n| n + 1),
                material: *material,
            })
            .collect();
        let mut r = BlastRange::with_layout(c, layout).unwrap();
        r.control();
        r.units = cubes.iter().map(|p| cube(*p)).collect();
        r
    }
    fn cell(unit: u16) -> Cell {
        Cell {
            unit,
            part: 0,
            at: [0; 3],
        }
    }
    fn total(r: &BlastRange) -> u64 {
        r.thermals.cells().map(|(_, h)| u64::from(h)).sum::<u64>()
            + r.heat
                .batch_after(None, usize::MAX)
                .into_iter()
                .map(|p| u64::from(r.heat.cell(p).heat))
                .sum::<u64>()
    }
    #[test]
    fn hot_armour_lights_timber_and_hot_world_heats_body_without_creating_energy() {
        let wood = [102, 32, 100];
        let mut r = fixture(&[[100, 32, 100]], &[(wood, 5)]);
        r.thermals.deposit(cell(0), 4000, &r.thermal_tuning);
        r.exchange_heat();
        assert_eq!(total(&r), 4000);
        assert_eq!(r.heat.cell(wood.map(|n| n as u32)).heat, 160);
        r.heat
            .step(&mut r.world, &r.matter.economy.mats, &r.heat_tuning, |_| {
                false
            });
        assert!(r.heat.cell(wood.map(|n| n as u32)).burn > 0);

        let mut r = fixture(&[[100, 32, 100]], &[(wood, 5)]);
        r.heat.add(&r.world, wood.map(|n| n as u32), 600);
        r.exchange_heat();
        assert_eq!(total(&r), 600);
        assert_eq!(r.thermals.heat(cell(0)), 24);
        assert_eq!(r.heat_exchange.transferred, 24);
    }
    #[test]
    fn steel_receives_heat_and_opaque_stone_blocks_it_from_timber() {
        for material in [3, 6] {
            let mut r = fixture(
                &[[100, 32, 100]],
                &[([101, 32, 100], material), ([102, 32, 100], 5)],
            );
            r.thermals.deposit(cell(0), 800, &r.thermal_tuning);
            r.exchange_heat();
            assert_eq!(r.heat.cell([102, 32, 100]).heat, 0);
            assert_eq!(
                r.heat.cell([101, 32, 100]).heat,
                if material == 3 { 32 } else { 0 }
            );
            assert_eq!(total(&r), 800);
        }
    }
    #[test]
    fn first_body_shields_the_next_and_gains_do_not_cascade_in_the_same_phase() {
        let mut r = fixture(&[[100, 32, 100], [102, 32, 100], [104, 32, 100]], &[]);
        r.thermals.deposit(cell(0), 800, &r.thermal_tuning);
        r.exchange_heat();
        assert_eq!(r.thermals.heat(cell(1)), 32);
        assert_eq!(r.thermals.heat(cell(2)), 0);
        r.exchange_heat();
        assert!(r.thermals.heat(cell(2)) > 0);
        assert_eq!(total(&r), 800);
        r.units[0].body.remove(0, 0, 0);
        let before = r.thermals.heat(cell(0));
        r.exchange_heat();
        assert_eq!(
            r.thermals.heat(cell(0)),
            before,
            "removed geometry must not emit queued heat"
        );
    }
    #[test]
    fn rotated_moving_segment_exchanges_only_at_its_current_pose() {
        let mut r = fixture(&[[100, 32, 100]], &[]);
        let mut seg = voxel::unit::tank().segments.remove(0);
        seg.body = cube([102, 32, 100]).body;
        let mut frame = cube([102, 32, 100]).pose;
        frame = Frame::new(&seg.body, frame.base, fx::angle::FULL / 8, V3::UP);
        r.units[0].body.segments.push(seg);
        r.units[0].seg_gone.push(false);
        r.units[0].seg_frames.push(frame);
        r.thermals.deposit(cell(0), 800, &r.thermal_tuning);
        let p = Cell { part: 1, ..cell(0) };
        r.exchange_heat();
        assert_eq!(r.thermals.heat(p), 32);
        assert_eq!(total(&r), 800);
        r.units[0].seg_frames[0].base.x = Fx::from_int(140);
        r.exchange_heat();
        assert_eq!(r.thermals.heat(p), 32);
        r.units[0].seg_frames[0] = frame;
        r.units[0].seg_gone[0] = true;
        r.exchange_heat();
        assert_eq!(r.thermals.heat(p), 32);
    }
    #[test]
    fn player_gun_is_opaque_even_without_target_heat_storage() {
        let mut r = fixture(&[[100, 32, 100], [103, 32, 100]], &[]);
        r.gun = cube([102, 32, 100]);
        r.thermals.deposit(cell(0), 800, &r.thermal_tuning);
        r.exchange_heat();
        assert_eq!(r.thermals.heat(cell(1)), 0);
        assert_eq!(total(&r), 800);
    }
    #[test]
    fn full_body_field_defers_exchange_without_losing_source_heat() {
        let mut r = fixture(&[[100, 32, 100], [102, 32, 100]], &[]);
        r.thermal_tuning.max_cells = 1;
        r.thermals.deposit(cell(0), 800, &r.thermal_tuning);
        r.exchange_heat();
        assert_eq!(r.thermals.heat(cell(0)), 800);
        assert_eq!(r.thermals.hot(), 1);
        assert_eq!(r.thermals.rejected_heat, 0);
        r.thermal_tuning.max_cells = 2;
        r.exchange_heat();
        assert_eq!(r.thermals.heat(cell(1)), 32);
        assert_eq!(total(&r), 800);
    }
    #[test]
    fn source_budget_wraps_and_cold_scenes_do_no_work() {
        let mut r = fixture(
            &[[100, 32, 100], [100, 32, 110]],
            &[([102, 32, 100], 5), ([102, 32, 110], 5)],
        );
        r.exchange_heat();
        assert_eq!(r.heat_exchange.visited, 0);
        r.thermal_tuning.exchange_cells_per_tick = 1;
        for id in 0..2 {
            r.thermals.deposit(cell(id), 800, &r.thermal_tuning);
        }
        r.exchange_heat();
        assert_eq!(r.heat_exchange.visited, 1);
        assert!(r.heat.cell([102, 32, 100]).heat > 0);
        assert_eq!(r.heat.cell([102, 32, 110]).heat, 0);
        r.exchange_heat();
        assert_eq!(r.heat_exchange.visited, 2);
        assert!(r.heat.cell([102, 32, 110]).heat > 0);
        r.exchange_heat();
        assert!(r.thermals.heat(cell(0)) < r.thermals.heat(cell(1)));
        assert_eq!(total(&r), 1600);
    }
    #[test]
    fn snapshot_preserves_active_cursors_and_version_32_starts_exchange_cleanly() {
        let c = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/flamethrower.ron"
        ))
        .unwrap();
        let mut a = BlastRange::new(c);
        a.control();
        let b = &a.units[0].body;
        let p = (0..b.vol.nz)
            .flat_map(|z| (0..b.vol.ny).flat_map(move |y| (0..b.vol.nx).map(move |x| [x, y, z])))
            .find(|p| b.vol.get(p[0] as usize, p[1] as usize, p[2] as usize))
            .unwrap();
        a.thermals
            .deposit(Cell { at: p, ..cell(0) }, 120, &a.thermal_tuning);
        a.heat.add(&a.world, [64, 8, 64], 120);
        a.exchange_heat();
        assert!(a.heat_exchange.world_cursor.is_some());
        assert!(a.heat_exchange.body_cursor.is_some());
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        for _ in 0..35 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        a.matter = super::super::matter::Matter::new(&a.units);
        a.combustion = super::super::fire::Combustion::empty(a.units.len());
        let mut data = a.snapshot();
        let mut tail = Blob::new();
        a.save_after(&mut tail, 32);
        data.truncate(data.len() - tail.v.len());
        data[..4].copy_from_slice(&32u32.to_le_bytes());
        let b = BlastRange::restore(a.config.clone(), &data).unwrap();
        assert_eq!(
            b.thermals.cells().collect::<Vec<_>>(),
            a.thermals.cells().collect::<Vec<_>>()
        );
        assert_eq!(b.heat_exchange.transferred, 0);
        assert!(b.heat_exchange.body_cursor.is_none());
    }
}
