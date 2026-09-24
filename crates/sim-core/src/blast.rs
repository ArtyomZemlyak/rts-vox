//! Material-aware explosion service and a deterministic single-gun range.
//! The renderer only reads this state. Gas consumes impact events and solid
//! geometry; no GPU result is allowed to alter the authoritative world.

use fx::Fx;
#[path = "range_matter.rs"]
pub mod matter;
#[path = "range_motion.rs"]
pub mod motion;
#[path = "range_placement.rs"]
pub mod placement;
#[path = "range_plate.rs"]
mod plate;
use crate::heat::Heat;
use crate::shield::{inside, Point, Shield};
use modding::explosion::{ExplosionConfig, ProjectileMotion};
use modding::range::{RangeLayout, RangeShieldDef, RangeUnitKind};
use motion::Motion;
use phys::blast::Wave;
use std::collections::BTreeMap;
use voxel::bodies::Bodies;
use voxel::collapse::CollapseLayer;
use voxel::topo::unit::CoreBufs;
use voxel::unit::{Status, UnitBody};
use voxel::{Generator, World};

#[cfg(test)]
#[path = "range_reset_tests.rs"]
mod reset_tests;
#[path = "range_structure.rs"]
mod structure;
#[cfg(test)]
#[path = "range_support_tests.rs"]
mod support_tests;

/// The range as it is without a layout to say otherwise. A layout
/// carries its own size (ADR-315), and the scene reads it from there.
pub const SIZE: [u32; 3] = modding::range::stock_size();
pub const FLOOR: i32 = 16;
pub const GUN: [i32; 3] = modding::range::stock_gun();
/// Muzzle of the vanilla 100 mm tank template relative to its base centre.
pub const MUZZLE: [i32; 3] = [GUN[0] + 30, GUN[1] + 23, GUN[2]];
pub const TARGET: [i32; 3] = [236, 24, 336];
pub const ENERGY_TARGET: [i32; 3] = [396, 24, 336];
pub const WITNESS: [i32; 3] = [377, FLOOR, 305];

/// A stationary 100 mm body. Damage is a bounded, ordered ablation ledger;
/// the pristine template minus this ledger completely specifies its state.
pub struct RangeUnit {
    pub kind: RangeUnitKind,
    pub motion: Motion,
    pub pose: crate::damage::Frame,
    pub turret_delta: i32,
    pub body: UnitBody,
    pub at: [i32; 3],
    pub removed: Vec<[u8; 3]>,
    /// Wave pressure the body took this tick (ADR-297); the range turns it
    /// into a push on the hull and clears it. A cell the wave breaks off
    /// counts too: the plate took the blow before it went. Transient.
    pub absorbed: u32,
    /// The last push: the burst, the point it went through, the pressure
    /// absorbed and the newton-seconds. Diagnostics, not state.
    pub last_push: Option<([i32; 3], [i32; 3], u32, u64)>,
    /// Removed cells transferred to persistent debris, excluded from the sink.
    pub detached: usize,
    /// Local work already spent deforming surviving body cells.
    pub dents: BTreeMap<[u8; 3], u32>,
    pub revision: u64,
    pub status: Status,
    pub mobility: crate::mobility::Mobility,
    pub integrity: crate::integrity::Report,
    original_shape: crate::burst::chunk::Shape,
    integrity_model: crate::integrity::Model,
    connectivity: CoreBufs,
    section: Option<crate::damage::SectionJob>,
    /// Rest-pose frames of the segments, refreshed with the pose (ADR-278).
    pub seg_frames: Vec<crate::damage::Frame>,
    /// Segment cells removed by damage or separation: index and cell. Saved.
    pub seg_removed: Vec<(u8, [u8; 3])>,
    seg_dents: BTreeMap<(u8, [u8; 3]), u32>,
    /// Segment cells handed to persistent debris rather than annihilated.
    pub seg_detached: usize,
    /// Segments that left the body whole; their volumes are emptied.
    pub seg_gone: Vec<bool>,
    /// Detached segments not yet handed to debris, in detachment order.
    seg_shed: Vec<u8>,
    seg_sections: Vec<Option<crate::damage::SectionJob>>,
    /// Root cells a segment hangs from, from the template; empty never sheds.
    seg_mounts: Vec<Vec<[u8; 3]>>,
}
/// Root cells within one voxel of each root-mounted segment's attachment
/// point: the mount it hangs from. A segment mounted on another segment
/// leaves with it, so its own list is empty.
fn mounts_of(body: &UnitBody) -> Vec<Vec<[u8; 3]>> {
    body.segments
        .iter()
        .map(|seg| {
            let a = &seg.attach;
            if a.parent != 0 {
                return Vec::new();
            }
            let mut cells = Vec::new();
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let p = [
                            i32::from(a.at.0) + dx,
                            i32::from(a.at.1) + dy,
                            i32::from(a.at.2) + dz,
                        ];
                        if p[0] < 0
                            || p[1] < 0
                            || p[2] < 0
                            || p[0] >= i32::from(body.vol.nx)
                            || p[1] >= i32::from(body.vol.ny)
                            || p[2] >= i32::from(body.vol.nz)
                        {
                            continue;
                        }
                        if body.vol.get(p[0] as usize, p[1] as usize, p[2] as usize) {
                            cells.push(p.map(|v| v as u8));
                        }
                    }
                }
            }
            cells
        })
        .collect()
}
/// A world point in a body's own cells through the given frame.
fn local_in(body: &UnitBody, p: [i32; 3], pose: &crate::damage::Frame) -> Option<[usize; 3]> {
    if (p[0] - pose.base.x.to_int_trunc()).abs() > 80
        || (p[2] - pose.base.z.to_int_trunc()).abs() > 80
    {
        return None;
    }
    let v = p.map(|v| Fx::from_int(v).add(Fx::from_ratio(1, 2)));
    let q = pose
        .local(phys::V3::new(v[0], v[1], v[2]))
        .map(|v| (v.raw() >> 32) as i32);
    let size = [body.vol.nx, body.vol.ny, body.vol.nz];
    (0..3)
        .all(|a| q[a] >= 0 && q[a] < size[a] as i32)
        .then_some(q.map(|v| v as usize))
}
/// Exact integer-point form of `Frame::local(p + 0.5)`. The integer part of
/// each world coordinate contributes `p * axis` without a Q32 multiply;
/// the fractional offset and frame translation are constant for this pose.
struct LocalMap {
    base_xz: [i32; 2],
    axes: [[i64; 3]; 3],
    offset: [i64; 3],
    scale: Fx,
    centre: [Fx; 3],
}
impl LocalMap {
    fn new(frame: &crate::damage::Frame) -> Self {
        let base = [frame.base.x.raw(), frame.base.y.raw(), frame.base.z.raw()];
        let bias = base.map(|v| (1i64 << 31).wrapping_sub(v));
        let axes =
            [frame.x, frame.y, frame.z].map(|axis| [axis.x.raw(), axis.y.raw(), axis.z.raw()]);
        let offset = axes.map(|axis| {
            (0..3).fold(0i64, |sum, a| {
                sum.wrapping_add(((i128::from(bias[a]) * i128::from(axis[a])) >> 32) as i64)
            })
        });
        Self {
            base_xz: [frame.base.x.to_int_trunc(), frame.base.z.to_int_trunc()],
            axes,
            offset,
            scale: frame.scale,
            centre: frame.centre,
        }
    }
    #[inline]
    fn local_in(&self, body: &UnitBody, p: [i32; 3]) -> Option<[usize; 3]> {
        if (p[0] - self.base_xz[0]).abs() > 80 || (p[2] - self.base_xz[1]).abs() > 80 {
            return None;
        }
        let q = std::array::from_fn::<_, 3, _>(|a| {
            let dot = (0..3).fold(self.offset[a], |sum, b| {
                sum.wrapping_add(i64::from(p[b]).wrapping_mul(self.axes[a][b]))
            });
            (Fx(dot).mul(self.scale).add(self.centre[a]).raw() >> 32) as i32
        });
        let size = [body.vol.nx, body.vol.ny, body.vol.nz];
        (0..3)
            .all(|a| q[a] >= 0 && q[a] < size[a] as i32)
            .then_some(q.map(|v| v as usize))
    }
}
/// Derived broad phase for a batch of voxel queries in one (possibly frozen)
/// pose. Damage is read live; only transforms and enclosing boxes are cached.
struct HitFrames {
    root: LocalMap,
    turret: crate::damage::Frame,
    turret_map: LocalMap,
    shared_root: bool,
    segments: Vec<LocalMap>,
    boxes: Vec<([i32; 3], [i32; 3])>,
}
impl HitFrames {
    fn new(
        u: &RangeUnit,
        root: crate::damage::Frame,
        segments: &[crate::damage::Frame],
        delta: i32,
    ) -> Self {
        let boxes = u
            .body
            .segments
            .iter()
            .zip(segments)
            .map(|(s, frame)| {
                let mut lo = [i32::MAX; 3];
                let mut hi = [i32::MIN; 3];
                let dims = [s.body.vol.nx, s.body.vol.ny, s.body.vol.nz];
                for k in 0..8 {
                    let p = frame.world(std::array::from_fn(|a| {
                        Fx::from_int(if k & (1 << a) == 0 { 0 } else { dims[a] as i32 })
                    }));
                    for (a, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                        lo[a] = lo[a].min(v.to_int_trunc() - 1);
                        hi[a] = hi[a].max(v.to_int_trunc() + 1);
                    }
                }
                (lo, hi)
            })
            .collect();
        let turret = crate::damage::turned(&root, delta, 0);
        Self {
            root: LocalMap::new(&root),
            turret,
            turret_map: LocalMap::new(&turret),
            shared_root: delta == 0,
            segments: segments.iter().map(LocalMap::new).collect(),
            boxes,
        }
    }
    fn hit(&self, u: &RangeUnit, p: [i32; 3]) -> Option<(usize, [usize; 3])> {
        let cell = self.root.local_in(&u.body, p);
        let cell = if self.shared_root {
            cell
        } else {
            cell.filter(|c| c[1] < usize::from(u.body.turret_y))
                .or_else(|| {
                    self.turret_map
                        .local_in(&u.body, p)
                        .filter(|c| c[1] >= usize::from(u.body.turret_y))
                })
        };
        if let Some(c) = cell {
            if u.body.vol.get(c[0], c[1], c[2]) {
                return Some((0, c));
            }
        }
        for (i, ((seg, frame), (lo, hi))) in u
            .body
            .segments
            .iter()
            .zip(&self.segments)
            .zip(&self.boxes)
            .enumerate()
        {
            if u.seg_gone.get(i).copied().unwrap_or(false)
                || (0..3).any(|a| p[a] < lo[a] || p[a] > hi[a])
            {
                continue;
            }
            if let Some(c) = frame.local_in(&seg.body, p) {
                if seg.body.vol.get(c[0], c[1], c[2]) {
                    return Some((i + 1, c));
                }
            }
        }
        None
    }
}
impl RangeUnit {
    pub fn barrel_connected(&self, index: usize) -> bool {
        self.body.weapon_paths().get(index).is_some_and(|path| {
            self.body
                .barrel_connected(path, self.connectivity.reached_cols())
        })
    }
    pub fn new(at: [i32; 3]) -> Self {
        Self::of_kind(RangeUnitKind::Tank, at)
    }
    pub fn of_kind(kind: RangeUnitKind, at: [i32; 3]) -> Self {
        // Stock range bodies are immutable templates until a shot damages an
        // instance. Copy their prepared voxel data instead of redrawing the
        // same shapes for every placement and battle reset.
        static TANK: std::sync::OnceLock<UnitBody> = std::sync::OnceLock::new();
        static STRIDER: std::sync::OnceLock<UnitBody> = std::sync::OnceLock::new();
        let body = match kind {
            RangeUnitKind::Tank => TANK.get_or_init(voxel::unit::tank).clone(),
            RangeUnitKind::Strider => STRIDER.get_or_init(voxel::unit::strider).clone(),
        };
        let mut connectivity = CoreBufs::new();
        let integrity_model = crate::integrity::Model::new(&body);
        let (status, integrity) = integrity_model.evaluate(&body, &mut connectivity);
        let motion = Motion::new(at, kind, &body);
        let pose = motion.frame(&body);
        let original_shape = crate::burst::chunk::Shape::from_body(&body);
        let seg_frames =
            crate::damage::segment_frames(&pose, &body, motion.drive.head, motion.drive.head);
        let seg_mounts = mounts_of(&body);
        let seg_gone = vec![false; body.segments.len()];
        let seg_sections = (0..body.segments.len()).map(|_| None).collect();
        Self {
            motion,
            pose,
            turret_delta: 0,
            kind,
            body,
            at,
            removed: Vec::new(),
            absorbed: 0,
            last_push: None,
            detached: 0,
            dents: BTreeMap::new(),
            revision: 0,
            status,
            mobility: crate::mobility::Mobility::intact(status.engine)
                .with_traction(integrity.drive),
            integrity,
            original_shape,
            integrity_model,
            connectivity,
            section: None,
            seg_frames,
            seg_removed: Vec::new(),
            seg_dents: BTreeMap::new(),
            seg_detached: 0,
            seg_gone,
            seg_shed: Vec::new(),
            seg_sections,
            seg_mounts,
        }
    }
    fn refresh_damage(&mut self, id: usize) {
        self.update_response();
        for (li, severed) in self
            .integrity_model
            .severed_legs(&self.body)
            .into_iter()
            .enumerate()
        {
            if severed {
                self.shed(self.body.legs[li].thigh as usize);
            }
        }
        // A leg that lost more than the buckling share of its links folds
        // and stays folded (ADR-281).
        for (li, buckled) in self
            .integrity_model
            .buckled(&self.body)
            .into_iter()
            .enumerate()
        {
            if buckled {
                if let Some(flag) = self.motion.legs_buckled.get_mut(li) {
                    *flag = true;
                }
            }
        }
        // A dead body's legs carry nothing: they all fold, and the hull
        // comes down on them (ADR-304).
        if !self.status.alive {
            self.motion.legs_buckled.fill(true);
        }
        // A segment whose mount in the root is gone leaves whole, and
        // everything hanging from it goes with it (ADR-278).
        for i in 0..self.body.segments.len() {
            if self.seg_gone[i]
                || self.seg_mounts[i].is_empty()
                || self.seg_mounts[i].iter().any(|m| {
                    let word = (usize::from(m[2]) * usize::from(self.body.vol.ny)
                        + usize::from(m[1]))
                        * usize::from(self.body.vol.words)
                        + usize::from(m[0]) / 64;
                    self.connectivity.reached_cols()[word] & (1u64 << (m[0] % 64)) != 0
                })
            {
                continue;
            }
            self.shed(i);
        }
        if self.status.detached > 0 && self.section.is_none() {
            let mut mask = self.body.vol.clone();
            for (w, r) in mask.cols.iter_mut().zip(self.connectivity.reached_cols()) {
                *w &= !r;
            }
            self.section = Some(crate::damage::SectionJob::new(id as u32, 0, mask));
        }
        self.refresh_segment_sections(id);
    }
    fn update_response(&mut self) {
        (self.status, self.integrity) = self
            .integrity_model
            .evaluate(&self.body, &mut self.connectivity);
        let engine = if self.status.alive && self.status.engine {
            self.integrity.engine
        } else {
            Fx::ZERO
        };
        self.mobility = crate::mobility::Mobility::from_shapes(
            &self.original_shape,
            &crate::burst::chunk::Shape::from_body(&self.body),
            [self.body.vol.nx, self.body.vol.ny, self.body.vol.nz],
            engine,
        )
        .with_traction(self.integrity.drive);
    }
    /// Mark a segment as detached, with its descendants and, for a leg
    /// part, the rest of the leg: the links are mounted on the root
    /// separately and only the kinematic description ties them. Debris
    /// takes them when the pool has room.
    fn shed(&mut self, seg: usize) {
        let mut stack = vec![seg];
        while let Some(i) = stack.pop() {
            if self.seg_gone[i] {
                continue;
            }
            self.seg_gone[i] = true;
            self.seg_shed.push(i as u8);
            for (j, other) in self.body.segments.iter().enumerate() {
                if other.attach.parent as usize == i + 1 && !self.seg_gone[j] {
                    stack.push(j);
                }
            }
            for (li, leg) in self.body.legs.iter().enumerate() {
                let parts = [
                    Some(leg.thigh),
                    Some(leg.shin),
                    leg.knee_ball,
                    leg.ankle_ball,
                    leg.foot,
                ];
                if parts.contains(&Some(i as u8)) {
                    // The leg is gone: it neither steps nor carries (ADR-280).
                    if let Some(lost) = self.motion.legs_lost.get_mut(li) {
                        *lost = true;
                    }
                    for part in parts.into_iter().flatten() {
                        if !self.seg_gone[part as usize] {
                            stack.push(part as usize);
                        }
                    }
                }
            }
        }
    }
    pub fn update_pose(&mut self, turret_yaw: i32) {
        self.turret_delta = fx::angle::diff(turret_yaw, self.motion.drive.head);
        self.at = self.motion.pos.map(|v| v.to_int_trunc());
        self.pose = self.motion.frame(&self.body);
        self.seg_frames = crate::damage::segment_frames(
            &self.pose,
            &self.body,
            turret_yaw,
            self.motion.drive.head,
        );
        // Legs stand where their feet are, not where the template rests
        // them (ADR-279).
        for (rig, foot) in self.motion.rigs().iter().zip(&self.motion.feet) {
            if let Some(c) = &foot.contact {
                crate::gait::leg_frames(rig, &self.pose, c, &mut self.seg_frames);
            }
        }
    }
    /// Where a world point lands on this body: `0` the root, `i + 1` the
    /// segment `i`, with the cell there. Segments are tested in the given
    /// rest-pose frames.
    pub fn hit_at(
        &self,
        p: [i32; 3],
        pose: &crate::damage::Frame,
        segs: &[crate::damage::Frame],
    ) -> Option<(usize, [usize; 3])> {
        self.hit_with_turret(p, pose, segs, self.turret_delta)
    }
    fn hit_with_turret(
        &self,
        p: [i32; 3],
        pose: &crate::damage::Frame,
        segs: &[crate::damage::Frame],
        delta: i32,
    ) -> Option<(usize, [usize; 3])> {
        if let Some(c) = self.root_local(p, pose, delta) {
            if self.body.vol.get(c[0], c[1], c[2]) {
                return Some((0, c));
            }
        }
        self.segment_hit(p, segs)
    }
    fn segment_hit(
        &self,
        p: [i32; 3],
        segs: &[crate::damage::Frame],
    ) -> Option<(usize, [usize; 3])> {
        for (i, (seg, f)) in self.body.segments.iter().zip(segs).enumerate() {
            if self.seg_gone.get(i).copied().unwrap_or(false) {
                continue;
            }
            if let Some(c) = local_in(&seg.body, p, f) {
                if seg.body.vol.get(c[0], c[1], c[2]) {
                    return Some((i + 1, c));
                }
            }
        }
        None
    }
    /// The body part under a world point in the current pose.
    pub fn hit(&self, p: [i32; 3]) -> Option<(usize, [usize; 3])> {
        if let Some(c) = self.local(p) {
            if self.body.vol.get(c[0], c[1], c[2]) {
                return Some((0, c));
            }
        }
        self.segment_hit(p, &self.seg_frames)
    }
    pub fn seg_dent(&self, seg: usize, cell: [usize; 3]) -> u32 {
        self.seg_dents
            .get(&(seg as u8, cell.map(|v| v as u8)))
            .copied()
            .unwrap_or(0)
    }
    pub fn segment_dents(&self) -> &BTreeMap<(u8, [u8; 3]), u32> {
        &self.seg_dents
    }
    pub fn segment_shed_pending(&self) -> &[u8] {
        &self.seg_shed
    }
    pub fn segment_mounts(&self, seg: usize) -> &[[u8; 3]] {
        &self.seg_mounts[seg]
    }
    pub fn world_pos(&self, p: [usize; 3]) -> [i32; 3] {
        let v = self
            .root_frame(p[1])
            .world(p.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
        [v.x, v.y, v.z].map(|v| v.to_int_trunc())
    }
    pub fn root_frame(&self, y: usize) -> crate::damage::Frame {
        if y >= usize::from(self.body.turret_y) {
            crate::damage::turned(&self.pose, self.turret_delta, 0)
        } else {
            self.pose
        }
    }
    fn root_local(
        &self,
        p: [i32; 3],
        pose: &crate::damage::Frame,
        delta: i32,
    ) -> Option<[usize; 3]> {
        if delta == 0 {
            return local_in(&self.body, p, pose);
        }
        if let Some(c) = local_in(&self.body, p, pose) {
            if c[1] < usize::from(self.body.turret_y) {
                return Some(c);
            }
        }
        local_in(&self.body, p, &crate::damage::turned(pose, delta, 0))
            .filter(|c| c[1] >= usize::from(self.body.turret_y))
    }
    pub fn local_at(&self, p: [i32; 3], pose: &crate::damage::Frame) -> Option<[usize; 3]> {
        self.root_local(p, pose, self.turret_delta)
    }
    pub fn local(&self, p: [i32; 3]) -> Option<[usize; 3]> {
        self.local_at(p, &self.pose)
    }
    pub fn solid(&self, p: [i32; 3]) -> bool {
        self.hit(p).is_some()
    }
    /// The world box every cell of the body and of its segments lies in,
    /// a unit of margin around it (ADR-296): a point outside it hits
    /// nothing, and the pose-mapped lookups need not be asked.
    pub fn bounds(&self) -> ([i32; 3], [i32; 3]) {
        self.bounds_for(Some(&self.pose), &self.seg_frames)
    }
    /// An enclosing box in the supplied frames, including a burst's frozen
    /// poses. Current motion must never move that burst's broad phase.
    fn bounds_for(
        &self,
        pose: Option<&crate::damage::Frame>,
        segments: &[crate::damage::Frame],
    ) -> ([i32; 3], [i32; 3]) {
        self.bounds_aimed(pose, segments, self.turret_delta)
    }
    fn bounds_aimed(
        &self,
        pose: Option<&crate::damage::Frame>,
        segments: &[crate::damage::Frame],
        delta: i32,
    ) -> ([i32; 3], [i32; 3]) {
        let mut lo = [i32::MAX; 3];
        let mut hi = [i32::MIN; 3];
        let mut take = |v: phys::V3| {
            let p = [v.x, v.y, v.z].map(|c| c.to_int_trunc());
            for a in 0..3 {
                lo[a] = lo[a].min(p[a] - 1);
                hi[a] = hi[a].max(p[a] + 1);
            }
        };
        let corners = |n: [i32; 3]| {
            (0..8).map(move |k| {
                [
                    if k & 1 != 0 { n[0] } else { 0 },
                    if k & 2 != 0 { n[1] } else { 0 },
                    if k & 4 != 0 { n[2] } else { 0 },
                ]
            })
        };
        let size = |b: &UnitBody| {
            [
                i32::from(b.vol.nx),
                i32::from(b.vol.ny),
                i32::from(b.vol.nz),
            ]
        };
        if let Some(pose) = pose {
            for l in corners(size(&self.body)) {
                take(pose.world(l.map(Fx::from_int)));
                if delta != 0 {
                    take(crate::damage::turned(pose, delta, 0).world(l.map(Fx::from_int)));
                }
            }
        } else {
            let o = self.world_pos([0; 3]);
            let n = size(&self.body);
            take(phys::V3::new(
                Fx::from_int(o[0]),
                Fx::from_int(o[1]),
                Fx::from_int(o[2]),
            ));
            take(phys::V3::new(
                Fx::from_int(o[0] + n[0]),
                Fx::from_int(o[1] + n[1]),
                Fx::from_int(o[2] + n[2]),
            ));
        }
        for (seg, frame) in self.body.segments.iter().zip(segments) {
            for l in corners(size(&seg.body)) {
                take(frame.world(l.map(Fx::from_int)));
            }
        }
        (lo, hi)
    }
    pub fn resistance(&self, p: [i32; 3], c: &ExplosionConfig) -> u32 {
        self.hit_resistance(self.hit(p), c)
    }
    fn hit_resistance(&self, hit: Option<(usize, [usize; 3])>, c: &ExplosionConfig) -> u32 {
        match hit {
            Some((0, cell)) => unit_work(&self.body, cell, c)
                .saturating_sub(self.dents.get(&cell.map(|v| v as u8)).copied().unwrap_or(0)),
            Some((part, cell)) => unit_work(&self.body.segments[part - 1].body, cell, c)
                .saturating_sub(self.seg_dent(part - 1, cell)),
            None => 0,
        }
    }
    #[cfg(test)]
    fn receive(&mut self, p: [i32; 3], pressure: u32, c: &ExplosionConfig) {
        let Some(cell) = self.local(p) else {
            return;
        };
        self.receive_cell(cell, pressure, c);
    }
    fn receive_cell(&mut self, cell: [usize; 3], pressure: u32, c: &ExplosionConfig) {
        let [x, y, z] = cell;
        if !self.body.vol.get(x, y, z) {
            return;
        }
        let key = cell.map(|v| v as u8);
        let need = unit_work(&self.body, cell, c)
            .saturating_sub(self.dents.get(&key).copied().unwrap_or(0));
        if pressure == 0 {
            return;
        }
        if pressure >= need {
            if self.body.remove(x, y, z) {
                self.removed.push(key);
                self.dents.remove(&key);
                self.revision += 1;
            }
        } else {
            let cell = key;
            *self.dents.entry(cell).or_default() += pressure;
            self.revision += 1;
        }
    }
    fn receive_seg_cell(
        &mut self,
        seg: usize,
        cell: [usize; 3],
        pressure: u32,
        c: &ExplosionConfig,
    ) {
        let [x, y, z] = cell;
        let key = (seg as u8, cell.map(|v| v as u8));
        let body = &mut self.body.segments[seg].body;
        if pressure == 0 || !body.vol.get(x, y, z) {
            return;
        }
        let need =
            unit_work(body, cell, c).saturating_sub(self.seg_dents.get(&key).copied().unwrap_or(0));
        if pressure >= need {
            if body.remove(x, y, z) {
                self.seg_removed.push(key);
                self.seg_dents.remove(&key);
                self.revision += 1;
            }
        } else {
            *self.seg_dents.entry(key).or_default() += pressure;
            self.revision += 1;
        }
    }
    /// Explicit matter sink for this anti-matter weapon, never particle count.
    pub fn annihilated_mm3(&self) -> u64 {
        (self.removed.len() - self.detached) as u64 * u64::from(self.body.mm).pow(3)
    }
}

pub fn unit_work(body: &UnitBody, p: [usize; 3], c: &ExplosionConfig) -> u32 {
    (u64::from(c.unit_ablation_resistance)
        * u64::from(c.unit_role_resistance_milli[body.part_at(p[0], p[1], p[2]) as usize])
        / 1000)
        .clamp(1, 1_000_000) as u32
}

pub type SurfaceWork = BTreeMap<[i32; 3], u32>;
pub fn material_work(m: u16, c: &ExplosionConfig) -> u32 {
    match m {
        0 => 0,
        1 | 4 | 5 | 7 | 8 => c.soil_resistance,
        2 | 6 => c.concrete_resistance,
        _ => c.steel_resistance,
    }
}

#[path = "range_index.rs"]
mod range_index;

pub struct RangeTerrain {
    index: range_index::Index,
    extended: bool,
    layout: Option<RangeLayout>,
}
/// The range as it was before it could be bigger: everything inside this
/// square keeps exactly the relief it had (ADR-320).
const STOCK_SIDE: i32 = 640;

/// Relief beyond the stock range, so that a bigger map is not a plain
/// (P-34). A hill sits in each cell of a lattice, placed, sized and
/// raised by the cell's own number, and fades to nothing at its own
/// radius like the hills of the stock range do. A hill whose reach could
/// touch the stock square is not raised at all, so the range as it was
/// is untouched to the voxel.
fn far_ridge(x: u32, z: u32) -> u32 {
    const CELL: i32 = 320;
    let mut ridge = 0;
    let (gx, gz) = ((x as i32) / CELL, (z as i32) / CELL);
    for ox in -1..=1 {
        for oz in -1..=1 {
            let (cx, cz) = (gx + ox, gz + oz);
            if cx < 0 || cz < 0 {
                continue;
            }
            let seed =
                (cx as u32).wrapping_mul(2_654_435_761) ^ ((cz as u32).wrapping_mul(2_246_822_519));
            if seed.is_multiple_of(5) {
                continue;
            }
            let radius = 96 + (seed >> 3) as i32 % 96;
            let hx = cx * CELL + (seed >> 11) as i32 % CELL;
            let hz = cz * CELL + (seed >> 19) as i32 % CELL;
            if hx - radius < STOCK_SIDE && hz - radius < STOCK_SIDE {
                continue;
            }
            let (dx, dz) = (x as i32 - hx, z as i32 - hz);
            let r2 = radius * radius;
            let height = 10 + (seed >> 27) as i32 % 22;
            ridge = ridge.max(((r2 - dx * dx - dz * dz).max(0) * height / r2) as u32);
        }
    }
    ridge
}

/// A flat firing lane surrounded by terraced woodland. Integer-only world data.
pub fn range_height(x: u32, z: u32) -> u32 {
    let mut ridge = 0;
    for (cx, cz, radius, height) in [
        (65, 65, 108, 25),
        (181, 36, 112, 32),
        (322, 100, 111, 29),
        (363, 319, 89, 18),
    ] {
        let dx = x as i32 - cx;
        let dz = z as i32 - (cz + 128);
        let r2 = radius * radius;
        ridge = ridge.max(((r2 - dx * dx - dz * dz).max(0) * height / r2) as u32);
    }
    // Blend the wooded hills into the level range without a straight escarpment.
    ridge = ridge * (302u32.saturating_sub(z) + x.saturating_sub(285)).min(48) / 48;
    ridge = ridge.max(far_ridge(x, z));
    let relief = if ridge > 0 {
        let a = (x / 48).wrapping_mul(17) ^ (z / 48).wrapping_mul(31);
        let b = (x / 48 + 1).wrapping_mul(17) ^ (z / 48).wrapping_mul(31);
        let c = (x / 48).wrapping_mul(17) ^ (z / 48 + 1).wrapping_mul(31);
        let d = (x / 48 + 1).wrapping_mul(17) ^ (z / 48 + 1).wrapping_mul(31);
        let u = x % 48;
        let v = z % 48;
        ((a % 7 * (48 - u) + b % 7 * u) * (48 - v) + (c % 7 * (48 - u) + d % 7 * u) * v) / (48 * 48)
    } else {
        0
    };
    FLOOR as u32 + ridge + relief
}
/// Break the flat test pads into broad, irregular earth/grass patches while
/// retaining a clear stone strip wherever a road crosses them.
fn range_pad_surface(x: u32, z: u32, road: bool) -> u16 {
    if road {
        return 6;
    }
    const CELL: u32 = 80;
    let (gx, gz, u, v) = (x / CELL, z / CELL, x % CELL, z % CELL);
    let sample = |a: u32, b: u32| {
        let n = a.wrapping_mul(73_856_093) ^ b.wrapping_mul(19_349_663);
        (n ^ n >> 11 ^ n >> 21) & 255
    };
    let blend = sample(gx, gz) * (CELL - u) * (CELL - v)
        + sample(gx + 1, gz) * u * (CELL - v)
        + sample(gx, gz + 1) * (CELL - u) * v
        + sample(gx + 1, gz + 1) * u * v;
    let fine = (x / 20).wrapping_mul(83_492_791) ^ (z / 20).wrapping_mul(53_422_681);
    if blend / (CELL * CELL) + fine % 17 < 102 {
        7
    } else {
        8
    }
}
/// Everything about a column of the range that does not depend on its
/// height (ADR-319). Worked out once a column instead of once a voxel:
/// building the world asks the generator for every voxel of it, and on a
/// range of a kilometre that is nineteen billion asks.
struct Column {
    shift: u32,
    /// Layout blocks this column runs through, latest first, as the
    /// per-voxel walk sees them (it takes the last that contains the
    /// voxel).
    blocks: Vec<(i32, i32, u16)>,
    /// Height of the obstacle course here, when the column is on it.
    course: Option<i32>,
    /// Ground height, with the pads levelled.
    ground: u32,
    road: bool,
    pad_surface: u16,
    lane: bool,
    wall: bool,
    target: bool,
    /// Trunk and crown of the tree whose cell this column belongs to:
    /// its base, its height and the offset of this column from it.
    tree: Option<(u32, u32, i32, i32)>,
    rock: Option<(u32, i32, i32)>,
}

/// The smallest contiguous edit after preserving equal ordered ends. This
/// also confines insertions and removals in the middle of an authored list:
/// later entries keep their override order and need no terrain rebuild.
fn changed_span<T: PartialEq>(
    old: &[T],
    next: &[T],
) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let prefix = old.iter().zip(next).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(next[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (prefix..old.len() - suffix, prefix..next.len() - suffix)
}

impl RangeTerrain {
    fn road_at(&self, x: u32, z: u32, margin: i32) -> bool {
        let Some(layout) = &self.layout else {
            return false;
        };
        self.index.at(x, z).roads.iter().any(|&i| {
            let road = &layout.roads[i];
            let radius = (road.width / 2 + margin) as i128;
            road.points.windows(2).any(|segment| {
                let a = segment[0];
                let b = segment[1];
                let dx = (b[0] - a[0]) as i128;
                let dz = (b[1] - a[1]) as i128;
                let px = x as i128 - a[0] as i128;
                let pz = z as i128 - a[1] as i128;
                let len2 = dx * dx + dz * dz;
                let dot = (px * dx + pz * dz).clamp(0, len2);
                let ox = px * len2 - dx * dot;
                let oz = pz * len2 - dz * dot;
                ox * ox + oz * oz <= radius * radius * len2 * len2
            })
        })
    }
    /// Conservative XZ domains of changes to authored terrain. Unit rosters,
    /// firing targets, route speeds and zone labels do not generate terrain.
    fn changed_areas(old: &RangeLayout, next: &RangeLayout) -> Vec<([i32; 2], [i32; 2])> {
        let mut areas = Vec::new();
        if old.clear_min != next.clear_min || old.clear_max != next.clear_max {
            // Trees are selected by their lattice centre outside the queried
            // column. One full tree cell covers both the clearing margin and
            // the crown reaching back across it.
            for r in [old, next] {
                areas.push((r.clear_min.map(|v| v - 28), r.clear_max.map(|v| v + 28)));
            }
        }
        let (old_blocks, next_blocks) = changed_span(&old.blocks, &next.blocks);
        for b in old.blocks[old_blocks]
            .iter()
            .chain(&next.blocks[next_blocks])
        {
            areas.push(([b.min[0], b.min[2]], [b.max[0], b.max[2]]));
        }
        let (old_roads, next_roads) = changed_span(&old.roads, &next.roads);
        for road in old.roads[old_roads].iter().chain(&next.roads[next_roads]) {
            for segment in road.points.windows(2) {
                let pad = road.width / 2 + 20;
                areas.push((
                    [
                        segment[0][0].min(segment[1][0]) - pad,
                        segment[0][1].min(segment[1][1]) - pad,
                    ],
                    [
                        segment[0][0].max(segment[1][0]) + pad + 1,
                        segment[0][1].max(segment[1][1]) + pad + 1,
                    ],
                ));
            }
        }
        for (a, b) in [(old, next), (next, old)] {
            for z in &a.zones {
                if !b.zones.iter().any(|v| z.min == v.min && z.max == v.max) {
                    areas.push((z.min, z.max));
                }
            }
        }
        if old.course.as_ref().map(|c| (c.min, c.max))
            != next.course.as_ref().map(|c| (c.min, c.max))
        {
            for c in old.course.iter().chain(&next.course) {
                areas.push((c.min, c.max));
            }
        }
        let old_ramps = old.course.as_ref().map_or(&[][..], |c| c.ramps.as_slice());
        let next_ramps = next.course.as_ref().map_or(&[][..], |c| c.ramps.as_slice());
        let (old_changed, next_changed) = changed_span(old_ramps, next_ramps);
        for r in old_ramps[old_changed]
            .iter()
            .chain(&next_ramps[next_changed])
        {
            areas.push(([r.min[0], r.min[2]], [r.max[0], r.max[2]]));
        }
        areas
    }
    fn new(extended: bool, layout: Option<RangeLayout>) -> Self {
        Self {
            index: range_index::Index::new(layout.as_ref()),
            extended,
            layout,
        }
    }
    fn in_zone(&self, x: u32, z: u32) -> bool {
        self.layout.as_ref().is_some_and(|r| {
            self.index
                .at(x, z)
                .zones
                .iter()
                .any(|&i| r.zones[i].contains([x as i32, z as i32], 0))
        })
    }
    fn column_of(&self, x: u32, z: u32) -> Column {
        let shift = if self.extended { 160 } else { 0 };
        let road = self.road_at(x, z, 0);
        let layout_pad = self.layout.as_ref().is_some_and(|r| {
            x as i32 >= r.clear_min[0]
                && (x as i32) < r.clear_max[0]
                && z as i32 >= r.clear_min[1]
                && (z as i32) < r.clear_max[1]
        });
        let blocks = self.layout.as_ref().map_or_else(Vec::new, |r| {
            self.index
                .at(x, z)
                .blocks
                .iter()
                .rev()
                .map(|&i| &r.blocks[i])
                .filter(|b| {
                    x as i32 >= b.min[0]
                        && (x as i32) < b.max[0]
                        && z as i32 >= b.min[2]
                        && (z as i32) < b.max[2]
                })
                .map(|b| (b.min[1], b.max[1], b.material))
                .collect()
        });
        let course = self
            .layout
            .as_ref()
            .and_then(|r| r.course.as_ref())
            .filter(|c| {
                (x as i32 >= c.min[0]
                    && (x as i32) < c.max[0]
                    && z as i32 >= c.min[1]
                    && (z as i32) < c.max[1])
                    || self.in_zone(x, z)
            })
            .map(|c| {
                let mut h = FLOOR;
                for r in self.index.at(x, z).ramps.iter().map(|&i| &c.ramps[i]) {
                    if (x as i32) >= r.min[0]
                        && (x as i32) < r.max[0]
                        && (z as i32) >= r.min[2]
                        && (z as i32) < r.max[2]
                    {
                        let v = if r.axis == 0 { x as i32 } else { z as i32 };
                        let n = r.max[r.axis] - r.min[r.axis];
                        let along = if r.reverse {
                            r.max[r.axis] - v
                        } else {
                            v - r.min[r.axis]
                        };
                        h = h.max(r.min[1] + along * (r.max[1] - r.min[1]) / n);
                    }
                }
                h
            })
            .or_else(|| self.in_zone(x, z).then_some(FLOOR));
        let pad_surface = course.map_or(8, |_| range_pad_surface(x, z, road));
        let unit_pad = self.extended && (337..415).contains(&x) && (282..322).contains(&z);
        let ground =
            if layout_pad || unit_pad || (self.extended && x < 448 && (298..387).contains(&z)) {
                FLOOR as u32
            } else {
                range_height(x, z)
            };
        let lane = (322..351).contains(&z) && x < 254 + shift;
        let wall = self.layout.is_none()
            && (256 + shift..264 + shift).contains(&x)
            && (291..383).contains(&z)
            && (x > 257 + shift || z % 12 < 3);
        let target = self.layout.is_none()
            && (232 + shift..250 + shift).contains(&x)
            && (321..352).contains(&z);
        let cx = x / 28;
        let cz = z / 28;
        let seed = cx.wrapping_mul(73856093) ^ cz.wrapping_mul(19349663);
        let tx = cx * 28 + 8 + seed % 11;
        let tz = cz * 28 + 8 + (seed >> 7) % 11;
        let layout_tree = self.layout.as_ref().is_some_and(|r| {
            tx as i32 >= r.clear_min[0] - 10
                && (tx as i32) < r.clear_max[0] + 10
                && tz as i32 >= r.clear_min[1] - 10
                && (tz as i32) < r.clear_max[1] + 10
        });
        let unit_trees = self.extended && (328..425).contains(&tx) && (273..332).contains(&tz);
        let tree = (!layout_tree
            && !unit_trees
            && !self.road_at(tx, tz, 10)
            && !(tx < 288 + shift && (298..=386).contains(&tz))
            && !seed.is_multiple_of(5))
        .then(|| {
            (
                range_height(tx, tz),
                19 + (seed >> 13) % 18,
                x as i32 - tx as i32,
                z as i32 - tz as i32,
            )
        });
        let rock =
            (!road && !layout_pad && !unit_pad && !(x < 254 + shift && (319..354).contains(&z)))
                .then(|| {
                    let stone = (x / 13).wrapping_mul(9173) ^ (z / 13).wrapping_mul(6151);
                    (stone, x as i32 % 13 - 6, z as i32 % 13 - 6)
                })
                .filter(|(stone, _, _)| stone.is_multiple_of(7));
        Column {
            shift,
            blocks,
            course,
            ground,
            road,
            pad_surface,
            lane,
            wall,
            target,
            tree,
            rock,
        }
    }
    /// The material at this height of a column already worked out. The
    /// same answer as [`RangeTerrain::material`], to the bit; the test
    /// `the_column_of_the_range_matches_its_voxels` holds them together.
    fn at(&self, c: &Column, x: u32, y: u32, z: u32) -> u16 {
        if let Some(&(_, _, mat)) = c
            .blocks
            .iter()
            .find(|(lo, hi, _)| (y as i32) >= *lo && (y as i32) < *hi)
        {
            return mat;
        }
        if let Some(h) = c.course {
            return if (y as i32) < h {
                if y as i32 == h - 1 {
                    c.pad_surface
                } else {
                    1
                }
            } else {
                0
            };
        }
        if y < c.ground {
            return if y + 1 == c.ground {
                if c.lane || c.road {
                    8
                } else {
                    7
                }
            } else {
                1
            };
        }
        // A massive rear wall channels the gas upward and sideways.
        // Ribbed revetment, with a broken parapet instead of a blank slab.
        if c.wall && y < 66 && (y < 55 || !(z / 9).is_multiple_of(3)) {
            return 3;
        }
        // Hollow concrete target, with a face struck by the shell.
        if c.target
            && y < 45
            && (x < 236 + c.shift || x >= 247 + c.shift || !(325..348).contains(&z) || y >= 41)
        {
            return 2;
        }
        // Voxel trees are actual matter: rays, shadows and gas see the same tree.
        if let Some((base, tall, dx, dz)) = c.tree {
            let dy = y as i32 - base as i32;
            if dy >= 0 && dy < tall as i32 {
                if dx.abs() <= 1 && dz.abs() <= 1 && dy < tall as i32 - 4 {
                    return 5;
                }
                let crown = tall as i32 - dy;
                let radius = (crown / 3 + (dy % 5) / 2).clamp(1, 9);
                if dy > 5 && dx * dx + dz * dz < radius * radius {
                    return 4;
                }
            }
        }
        // Small broken rocks on the shoulders, out of the firing lane. No
        // taller than the course step: a hull climbs a shoulder rock rather
        // than steering round it (ADR-275).
        if let Some((stone, dx, dz)) = c.rock {
            if y >= c.ground
                && y < c.ground + 2 + stone % 3
                && dx * dx + dz * dz < (5 - (y - c.ground).min(4)) as i32 * 3
            {
                return 6;
            }
        }
        0
    }
}

impl Generator for RangeTerrain {
    /// A whole column at once (ADR-114, ADR-319): what depends only on
    /// `(x, z)` is worked out once instead of once a voxel. Building a
    /// world asks for every voxel of it, and that is where the time goes.
    fn column(&self, x: u32, z: u32, y0: u32, out: &mut [u16]) {
        let c = self.column_of(x, z);
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.at(&c, x, y0 + i as u32, z);
        }
    }
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        let shift = if self.extended { 160 } else { 0 };
        let layout_pad = self.layout.as_ref().is_some_and(|r| {
            x as i32 >= r.clear_min[0]
                && (x as i32) < r.clear_max[0]
                && z as i32 >= r.clear_min[1]
                && (z as i32) < r.clear_max[1]
        });
        let road = self.road_at(x, z, 0);
        if let Some(r) = &self.layout {
            if let Some(b) = self
                .index
                .at(x, z)
                .blocks
                .iter()
                .rev()
                .map(|&i| &r.blocks[i])
                .find(|b| b.contains([x as i32, y as i32, z as i32]))
            {
                return b.material;
            }
        }
        if let Some(c) = self.layout.as_ref().and_then(|r| r.course.as_ref()) {
            if (x as i32 >= c.min[0]
                && (x as i32) < c.max[0]
                && z as i32 >= c.min[1]
                && (z as i32) < c.max[1])
                || self.in_zone(x, z)
            {
                let mut h = FLOOR;
                for r in self.index.at(x, z).ramps.iter().map(|&i| &c.ramps[i]) {
                    if (x as i32) >= r.min[0]
                        && (x as i32) < r.max[0]
                        && (z as i32) >= r.min[2]
                        && (z as i32) < r.max[2]
                    {
                        let v = if r.axis == 0 { x as i32 } else { z as i32 };
                        let n = r.max[r.axis] - r.min[r.axis];
                        let along = if r.reverse {
                            r.max[r.axis] - v
                        } else {
                            v - r.min[r.axis]
                        };
                        h = h.max(r.min[1] + along * (r.max[1] - r.min[1]) / n);
                    }
                }
                return if (y as i32) < h {
                    if y as i32 == h - 1 {
                        range_pad_surface(x, z, road)
                    } else {
                        1
                    }
                } else {
                    0
                };
            }
        }
        if self.in_zone(x, z) {
            return if y < FLOOR as u32 {
                if y + 1 == FLOOR as u32 {
                    range_pad_surface(x, z, road)
                } else {
                    1
                }
            } else {
                0
            };
        }
        let unit_pad = self.extended && (337..415).contains(&x) && (282..322).contains(&z);
        let h = if layout_pad || unit_pad || (self.extended && x < 448 && (298..387).contains(&z)) {
            FLOOR as u32
        } else {
            range_height(x, z)
        };
        if y < h {
            let lane = (322..351).contains(&z) && x < 254 + shift;
            return if y + 1 == h {
                if lane || road {
                    8
                } else {
                    7
                }
            } else {
                1
            };
        }
        // A massive rear wall channels the gas upward and sideways.
        if self.layout.is_none()
            && (256 + shift..264 + shift).contains(&x)
            && (291..383).contains(&z)
            && y < 66
        {
            // Ribbed revetment, with a broken parapet instead of a blank slab.
            if (y < 55 || !(z / 9).is_multiple_of(3)) && (x > 257 + shift || z % 12 < 3) {
                return 3;
            }
        }
        // Hollow concrete target, with a face struck by the shell.
        if self.layout.is_none()
            && (232 + shift..250 + shift).contains(&x)
            && (321..352).contains(&z)
            && y < 45
            && (x < 236 + shift || x >= 247 + shift || !(325..348).contains(&z) || y >= 41)
        {
            return 2;
        }
        // Voxel trees are actual matter: rays, shadows and gas see the same tree.
        // Leave the vehicle, firing lane and immediate target readable.
        let cx = x / 28;
        let cz = z / 28;
        let seed = cx.wrapping_mul(73856093) ^ cz.wrapping_mul(19349663);
        let tx = cx * 28 + 8 + seed % 11;
        let tz = cz * 28 + 8 + (seed >> 7) % 11;
        let layout_tree = self.layout.as_ref().is_some_and(|r| {
            tx as i32 >= r.clear_min[0] - 10
                && (tx as i32) < r.clear_max[0] + 10
                && tz as i32 >= r.clear_min[1] - 10
                && (tz as i32) < r.clear_max[1] + 10
        });
        let unit_trees = self.extended && (328..425).contains(&tx) && (273..332).contains(&tz);
        if !layout_tree
            && !unit_trees
            && !self.road_at(tx, tz, 10)
            && !(tx < 288 + shift && (298..=386).contains(&tz))
            && !seed.is_multiple_of(5)
        {
            let base = range_height(tx, tz);
            let tall = 19 + (seed >> 13) % 18;
            let dx = x as i32 - tx as i32;
            let dz = z as i32 - tz as i32;
            let dy = y as i32 - base as i32;
            if dy >= 0 && dy < tall as i32 {
                if dx.abs() <= 1 && dz.abs() <= 1 && dy < tall as i32 - 4 {
                    return 5;
                }
                let crown = tall as i32 - dy;
                let radius = (crown / 3 + (dy % 5) / 2).clamp(1, 9);
                if dy > 5 && dx * dx + dz * dz < radius * radius {
                    return 4;
                }
            }
        }
        // Small broken rocks on the shoulders, out of the firing lane. No
        // taller than the course step: a hull climbs a shoulder rock rather
        // than steering round it (ADR-275).
        if !road && !layout_pad && !unit_pad && !(x < 254 + shift && (319..354).contains(&z)) {
            let stone = (x / 13).wrapping_mul(9173) ^ (z / 13).wrapping_mul(6151);
            let dx = x as i32 % 13 - 6;
            let dz = z as i32 % 13 - 6;
            if stone.is_multiple_of(7)
                && y >= h
                && y < h + 2 + stone % 3
                && dx * dx + dz * dz < (5 - (y - h).min(4)) as i32 * 3
            {
                return 6;
            }
        }
        0
    }
}

pub struct Explosion {
    pub at: [i32; 3],
    origin: [i32; 3],
    cell: i32,
    pub wave: Wave,
    pub queued: usize,
    // Next subcell/actor in the first pending contact; committed work is never replayed.
    contact_cursor: usize,
    poses: Vec<crate::damage::Frame>,
    turret_deltas: Vec<i32>,
    /// Segment frames of each unit when the burst began (ADR-278).
    seg_poses: Vec<Vec<crate::damage::Frame>>,
    pending: std::collections::VecDeque<phys::blast::Hit>,
    /// Cells of the grid whose resistance is captured (ADR-311). The
    /// capture runs under a ceiling per tick and the wave waits for all
    /// of them, so a charge that reaches further costs more ticks rather
    /// than one longer tick.
    captured: usize,
}

impl Explosion {
    /// Capture material resistance once. Results do not depend on how many
    /// edits the world has drained when a later piece of the wave is visited.
    pub fn new(w: &World, at: [i32; 3], c: &ExplosionConfig, units: &[RangeUnit]) -> Self {
        Self::with_damage(w, at, c, units, &SurfaceWork::new())
    }
    /// The whole grid at once. The range captures it in slices instead
    /// (ADR-311); this is the same burst with every slice already run.
    pub fn with_damage(
        w: &World,
        at: [i32; 3],
        c: &ExplosionConfig,
        units: &[RangeUnit],
        dents: &SurfaceWork,
    ) -> Self {
        let mut e = Self::begin(at, c, units);
        e.capture(w, c, units, dents, usize::MAX);
        e
    }

    /// A burst with its grid still empty. Nothing may read the grid until
    /// [`Explosion::capture`] reports it whole.
    pub fn begin(at: [i32; 3], c: &ExplosionConfig, units: &[RangeUnit]) -> Self {
        // The grid is as wide as the charge can reach through the cheapest
        // matter — a step and the matter's own cost per cell — and never
        // narrower than the data's, up to the wave's ceiling of 65 cells a
        // side; and the wave stops at the sphere inscribed in it (ADR-303).
        // A crater that met the grid's cube came out square (P-21).
        let cheapest = c
            .air_cost
            .min(c.soil_resistance)
            .min(c.concrete_resistance)
            .min(c.steel_resistance)
            .max(1);
        let half = c
            .half_cells
            .max((c.energy / (100 + cheapest)) as usize + 2)
            .min(32);
        let n = half * 2 + 1;
        let h = half as i32 * c.cell_units;
        let origin = at.map(|v| v - h);
        Self {
            at,
            pending: Default::default(),
            contact_cursor: 0,
            poses: units.iter().map(|u| u.pose).collect(),
            turret_deltas: units.iter().map(|u| u.turret_delta).collect(),
            seg_poses: units.iter().map(|u| u.seg_frames.clone()).collect(),
            origin,
            cell: c.cell_units,
            wave: Wave::new(n, c.energy, vec![0; n * n * n]).expect("validated explosion"),
            queued: 0,
            captured: 0,
        }
    }

    /// The grid still has cells to capture.
    pub fn capturing(&self) -> bool {
        let n = self.wave.size();
        self.captured < n * n * n
    }

    /// Capture up to `budget` cells, resuming where the last slice
    /// stopped, and report whether the grid is whole. Cells the inscribed
    /// sphere cuts are free and do not count against the budget: they are
    /// out of the wave's reach and cost nothing to answer.
    pub fn capture(
        &mut self,
        w: &World,
        c: &ExplosionConfig,
        units: &[RangeUnit],
        dents: &SurfaceWork,
        budget: usize,
    ) -> bool {
        let n = self.wave.size();
        let cells = n * n * n;
        if self.captured >= cells {
            return true;
        }
        let half = n / 2;
        let origin = self.origin;
        let radius2 = (half * half) as i32;
        let queries: Vec<_> = units
            .iter()
            .map(|u| HitFrames::new(u, u.pose, &u.seg_frames, u.turret_delta))
            .collect();
        // Bodies are asked only where they can be (ADR-296): one box test
        // per body instead of a pose-mapped lookup for every voxel of the
        // domain, and dents are looked up only where there is material.
        let boxes: Vec<([i32; 3], [i32; 3])> = units.iter().map(|u| u.bounds()).collect();
        // Three questions used to be asked of every voxel of the domain,
        // and all three answer the same for a whole cell (ADR-310): which
        // bodies can reach into it, whether any dented voxel lies in it,
        // and which chunk holds it. Each is asked once a cell now. The
        // answers are the same to the bit — this is a hoist, not an
        // approximation — and at four charges the capture fell from 12.2
        // to 11.2 ms on a clean world and from 23.7 to 17.3 ms late in a
        // match, where the dent map has grown.
        let far = [0, 1, 2].map(|a| origin[a] + (n as i32 - 1) * c.cell_units + c.cell_units - 1);
        let mut dented = vec![false; cells];
        for q in dents
            .range([origin[0], i32::MIN, i32::MIN]..=[far[0], i32::MAX, i32::MAX])
            .map(|(q, _)| q)
        {
            let cell = [0, 1, 2].map(|a| (q[a] - origin[a]).div_euclid(c.cell_units));
            if (0..3).all(|a| cell[a] >= 0 && (cell[a] as usize) < n) {
                dented[(cell[2] as usize * n + cell[1] as usize) * n + cell[0] as usize] = true;
            }
        }
        // A sliced capture visits only part of the sphere this tick. Resolve
        // an authoritative column ceiling when its first reachable cell needs
        // terrain, then share it across heights in this call. An unvisited
        // column (or one beyond the sphere) costs no occupancy queries.
        let mut ceilings = vec![u32::MAX; n * n];
        let maximum = c
            .soil_resistance
            .max(c.concrete_resistance)
            .max(c.steel_resistance);
        let mut cursor = World::mat_cursor();
        let mut near: Vec<usize> = Vec::new();
        let mut spent = 0;
        let mut next = self.captured;
        while next < cells && spent < budget {
            let (x, y, z) = (next % n, (next / n) % n, next / (n * n));
            next += 1;
            let p = [
                origin[0] + x as i32 * c.cell_units,
                origin[1] + y as i32 * c.cell_units,
                origin[2] + z as i32 * c.cell_units,
            ];
            // Beyond the inscribed sphere the wave does not go: a reach
            // the grid cuts is cut the same in every direction.
            let (cx, cy, cz) = (
                x as i32 - half as i32,
                y as i32 - half as i32,
                z as i32 - half as i32,
            );
            if cx * cx + cy * cy + cz * cz > radius2 {
                self.wave.set_resistance(x + n * (y + n * z), u32::MAX);
                continue;
            }
            spent += 1;
            let end = [0, 1, 2].map(|a| p[a] + c.cell_units - 1);
            near.clear();
            for (i, (lo, hi)) in boxes.iter().enumerate() {
                if (0..3).all(|a| end[a] >= lo[a] && p[a] <= hi[a]) {
                    near.push(i);
                }
            }
            let look = dented[(z * n + y) * n + x];
            let mut r = 0;
            if p[1] < w.ny as i32 && end[1] >= 0 {
                let ceiling = &mut ceilings[z * n + x];
                if *ceiling == u32::MAX {
                    *ceiling = 0;
                    for dz in 0..c.cell_units {
                        for dx in 0..c.cell_units {
                            let wx = p[0] + dx;
                            let wz = p[2] + dz;
                            if wx >= 0 && wz >= 0 && wx < w.nx as i32 && wz < w.nz as i32 {
                                *ceiling = (*ceiling).max(w.column_ceiling(wx as u32, wz as u32));
                            }
                        }
                    }
                }
                if p[1] < *ceiling as i32 {
                    let x0 = p[0].max(0);
                    let x1 = (end[0] + 1).min(w.nx as i32);
                    'terrain: for wz in p[2].max(0)..(end[2] + 1).min(w.nz as i32) {
                        for wy in p[1].max(0)..(end[1] + 1).min(w.ny as i32) {
                            if x1 <= x0 {
                                continue;
                            }
                            let mut bits =
                                w.solid_bits(x0 as u32, wy as u32, wz as u32, (x1 - x0) as u32);
                            while bits != 0 {
                                let q = [x0 + bits.trailing_zeros() as i32, wy, wz];
                                bits &= bits - 1;
                                let work = material_work(material_cached(w, &mut cursor, q), c);
                                let dent = if look {
                                    dents.get(&q).copied().unwrap_or(0)
                                } else {
                                    0
                                };
                                r = r.max(work.saturating_sub(dent));
                                if r == maximum {
                                    break 'terrain;
                                }
                            }
                        }
                    }
                }
            }
            // Body resistance is independent of the terrain reduction. Ask
            // only the intersecting part of each box, including suspended parts.
            for &i in &near {
                let (lo, hi) = boxes[i];
                for wz in p[2].max(lo[2])..=end[2].min(hi[2]) {
                    for wy in p[1].max(lo[1])..=end[1].min(hi[1]) {
                        for wx in p[0].max(lo[0])..=end[0].min(hi[0]) {
                            r = r.max(
                                units[i].hit_resistance(queries[i].hit(&units[i], [wx, wy, wz]), c),
                            );
                        }
                    }
                }
            }
            self.wave.set_resistance(x + n * (y + n * z), r);
        }
        self.captured = next;
        !self.capturing()
    }

    /// The wave has cells still to reach or hits still to apply. The
    /// collapse layer waits for it (ADR-300), so that what it takes does
    /// not depend on how the wave is sliced across ticks.
    pub fn working(&self) -> bool {
        self.capturing() || !self.pending.is_empty() || !self.wave.finished()
    }
    // Separate authoritative outputs keep world edits and material claims explicit.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        w: &mut World,
        budget: usize,
        units: &mut [RangeUnit],
        dents: &mut SurfaceWork,
        changed: &mut Vec<[i32; 3]>,
        c: &ExplosionConfig,
        mut matter: Option<&mut matter::Matter>,
    ) {
        if self.pending.is_empty() {
            self.pending.extend(self.wave.advance_contacts(budget));
        }
        let edge = self.cell as usize;
        let actors = units.len() + 1;
        let queries: Vec<_> = units
            .iter()
            .enumerate()
            .map(|(i, u)| {
                HitFrames::new(u, self.poses[i], &self.seg_poses[i], self.turret_deltas[i])
            })
            .collect();
        let boxes: Vec<_> = units
            .iter()
            .enumerate()
            .map(|(i, u)| {
                u.bounds_aimed(
                    Some(&self.poses[i]),
                    &self.seg_poses[i],
                    self.turret_deltas[i],
                )
            })
            .collect();
        // For each coarse wave cell, skip actors whose complete frozen box
        // cannot intersect it. Keep the canonical contact cursor and actor
        // order: a deferred material claim resumes at exactly the same hit.
        let mut next_actor = vec![units.len(); actors];
        while let Some(hit) = self.pending.front().copied() {
            let p = std::array::from_fn::<_, 3, _>(|a| self.origin[a] + hit.cell[a] * self.cell);
            let mut next = units.len();
            for i in (0..units.len()).rev() {
                let (lo, hi) = boxes[i];
                if (0..3).all(|a| p[a] <= hi[a] && p[a] + self.cell > lo[a]) {
                    next = i;
                }
                next_actor[i] = next;
            }
            while self.contact_cursor < edge.pow(3) * actors {
                let subcell = self.contact_cursor / actors;
                let actor = next_actor[self.contact_cursor % actors];
                self.contact_cursor = subcell * actors + actor;
                let q = [
                    p[0] + (subcell % edge) as i32,
                    p[1] + (subcell / edge % edge) as i32,
                    p[2] + (subcell / (edge * edge)) as i32,
                ];
                // Advance only after successful application or an unaffordable
                // hit. Pool pressure retries this exact actor on the next tick.
                if let Some(u) = units.get_mut(actor) {
                    let (lo, hi) = boxes[actor];
                    if (0..3).any(|a| q[a] < lo[a] || q[a] > hi[a]) {
                        self.contact_cursor += 1;
                        continue;
                    }
                    match queries[actor].hit(u, q) {
                        Some((0, cell)) => {
                            // A cell already cut loose leaves with its whole
                            // section; the wave neither ablates it nor waits
                            // for it. Every other cell takes the impulse as
                            // it arrives.
                            let leaving = u
                                .section
                                .as_ref()
                                .is_some_and(|j| j.holds(cell.map(|v| v as u8)));
                            if !leaving {
                                let need = unit_work(&u.body, cell, c).saturating_sub(
                                    u.dents.get(&cell.map(|v| v as u8)).copied().unwrap_or(0),
                                );
                                let mut apply = true;
                                if hit.pressure >= need && hit.pressure > 0 {
                                    if let Some(m) = matter.as_deref_mut() {
                                        let frame = if cell[1] >= usize::from(u.body.turret_y) {
                                            queries[actor].turret
                                        } else {
                                            self.poses[actor]
                                        };
                                        match m.body(actor, u, cell, &frame, self.at) {
                                            matter::ClaimResult::Applied => u.detached += 1,
                                            matter::ClaimResult::Unaffordable => apply = false,
                                            matter::ClaimResult::Deferred => return,
                                        }
                                    }
                                }
                                if apply {
                                    u.receive_cell(cell, hit.pressure, c);
                                    u.absorbed = u.absorbed.saturating_add(hit.pressure);
                                }
                            }
                        }
                        Some((part, cell)) => {
                            if u.section_holds(part, cell.map(|v| v as u8)) {
                                self.contact_cursor += 1;
                                continue;
                            }
                            // A segment takes the impulse in its own frame.
                            let seg = part - 1;
                            let need = unit_work(&u.body.segments[seg].body, cell, c)
                                .saturating_sub(u.seg_dent(seg, cell));
                            let mut apply = true;
                            if hit.pressure >= need && hit.pressure > 0 {
                                if let Some(m) = matter.as_deref_mut() {
                                    match m.body_segment(
                                        actor,
                                        u,
                                        seg,
                                        cell,
                                        &self.seg_poses[actor][seg],
                                        self.at,
                                    ) {
                                        matter::ClaimResult::Applied => u.seg_detached += 1,
                                        matter::ClaimResult::Unaffordable => apply = false,
                                        matter::ClaimResult::Deferred => return,
                                    }
                                }
                            }
                            if apply {
                                u.receive_seg_cell(seg, cell, hit.pressure, c);
                                u.absorbed = u.absorbed.saturating_add(hit.pressure);
                            }
                        }
                        None => {}
                    }
                } else {
                    let m = material(w, q);
                    if m != 0 && hit.pressure > 0 {
                        let total = dents
                            .get(&q)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(hit.pressure);
                        let mut apply = true;
                        if total >= material_work(m, c) {
                            if let Some(m) = matter.as_deref_mut() {
                                match m.ground(w, q, self.at) {
                                    matter::ClaimResult::Applied => (),
                                    matter::ClaimResult::Unaffordable => apply = false,
                                    matter::ClaimResult::Deferred => return,
                                }
                            } else {
                                w.queue_edit(voxel::world::Edit::dig_if(
                                    q[0] as u32,
                                    q[1] as u32,
                                    q[2] as u32,
                                    m,
                                ));
                            }
                            if apply {
                                dents.remove(&q);
                                self.queued += 1;
                            }
                        } else {
                            dents.insert(q, total);
                        }
                        if apply {
                            changed.push(q);
                        }
                    }
                }
                self.contact_cursor += 1;
            }
            self.contact_cursor = 0;
            self.pending.pop_front();
        }
    }
}

/// The same material as [`material`], to the bit, with the chunk index
/// kept between neighbouring voxels (ADR-310).
fn material_cached(w: &World, cursor: &mut voxel::world::MatCursor, p: [i32; 3]) -> u16 {
    if p[0] < 0
        || p[1] < 0
        || p[2] < 0
        || p[0] >= w.nx as i32
        || p[1] >= w.ny as i32
        || p[2] >= w.nz as i32
    {
        return 0;
    }
    w.material_cached(cursor, p[0] as u32, p[1] as u32, p[2] as u32)
}

pub fn material(w: &World, p: [i32; 3]) -> u16 {
    if p[0] < 0
        || p[1] < 0
        || p[2] < 0
        || p[0] >= w.nx as i32
        || p[1] >= w.ny as i32
        || p[2] >= w.nz as i32
    {
        return 0;
    }
    w.material(p[0] as u32, p[1] as u32, p[2] as u32)
}

/// A dome on the range (ADR-276): geometry from the layout, charge in the
/// wave units of the weapons.
#[derive(Clone, Debug)]
pub struct RangeShield {
    pub at: [i32; 3],
    pub radius: i32,
    pub charge: u32,
    /// Ticks left before a collapsed dome stands again; zero while it stands.
    pub down: u32,
    pub hits: u32,
}
impl RangeShield {
    fn new(d: &RangeShieldDef, capacity: u32) -> Self {
        Self {
            at: d.at,
            radius: d.radius,
            charge: capacity,
            down: 0,
            hits: 0,
        }
    }
    /// The predicate's view of the dome: a collapsed one catches nothing.
    pub fn dome(&self) -> Shield {
        Shield {
            x: self.at[0],
            y: self.at[1],
            z: self.at[2],
            r: self.radius,
            owner: 0,
            charge: if self.down > 0 {
                0
            } else {
                self.charge.min(i32::MAX as u32) as i32
            },
        }
    }
    pub fn standing(&self) -> bool {
        self.down == 0 && self.charge > 0
    }
}
/// A round caught by a dome this tick, for the renderer's flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShieldHit {
    pub shield: usize,
    pub at: [i32; 3],
    pub drained: u32,
    pub collapsed: bool,
}
fn shot_point(p: [i32; 3]) -> Point {
    Point {
        x: p[0],
        y: p[1],
        z: p[2],
        owner: 1,
    }
}

#[path = "range_fire.rs"]
pub mod fire;
#[path = "range_heat_exchange.rs"]
pub mod heat_exchange;
#[path = "range_thermal.rs"]
pub mod thermal;
#[path = "range_weapons.rs"]
pub mod weapons;

#[path = "range_ai.rs"]
pub mod ai;
#[path = "range_battle.rs"]
pub mod battle;

pub struct BlastRange {
    pub battle: battle::Battle,
    pub combustion: fire::Combustion,
    pub heat_exchange: heat_exchange::Exchange,
    pub thermals: thermal::Thermals,
    pub thermal_tuning: modding::unit_heat::Tuning,
    pub streams: weapons::Streams,
    pub matter: matter::Matter,
    pub world: World,
    /// Side, height and side of this range in world units (ADR-315). The
    /// layout says it; without a layout it is the stock [`SIZE`].
    size: [u32; 3],
    pub config: ExplosionConfig,
    pub tick: u32,
    pub shell: Option<[Fx; 3]>,
    velocity: [Fx; 3],
    pub impact: Option<[i32; 3]>,
    pub impact_tick: u32,
    pub explosion: Option<Explosion>,
    pub removed: usize,
    pub units: Vec<RangeUnit>,
    pub layout: Option<RangeLayout>,
    // Immutable generator/roster selection, independent of the equipped weapon.
    extended: bool,
    witness: bool,
    pub removed_materials: [usize; 16],
    pub surface_work: SurfaceWork,
    /// Transient renderer notification; authoritative values live above.
    pub surface_changed: Vec<[i32; 3]>,
    pub fire_tick: u32,
    pub gun: RangeUnit,
    pub player: bool,
    pub patrols_running: bool,
    pub attack: Option<usize>,
    /// Automatically acquire a live range target; manual orders disable it.
    pub auto_attack: bool,
    pub aim: Option<[i32; 3]>,
    pub turret: crate::turret::Traverse,
    turret_limits: crate::turret::Limits,
    pub navigation: modding::navigation::Navigation,
    pub shields: Vec<RangeShield>,
    /// Transient renderer notification, like `surface_changed`.
    pub shield_hits: Vec<ShieldHit>,
    shield_tuning: modding::shield::ShieldTuning,
    /// Authoritative heat and fire (ADR-277).
    pub heat: Heat,
    heat_tuning: modding::heat::HeatTuning,
    /// Columns of resting debris and their tops, rebuilt each tick from
    /// the pieces (ADR-292); derived, not saved.
    pub rubble: nav::vehicle::Rubble,
    /// The batch's support graph over the range's world (ADR-299): what
    /// the tick's edits leave hanging is dug out and becomes pieces.
    collapse: CollapseLayer,
    /// Cohesive terrain the layer tore loose, falling whole until it
    /// lands and bakes back into the world (ADR-300). In the snapshot.
    pub bodies: Bodies,
    /// Cells that fell for want of support. In the snapshot and the hash.
    pub collapsed_cells: u64,
    /// No edit reached the world on the last drain (ADR-306): a route
    /// search in flight may keep what it scanned. In the snapshot.
    world_steady: bool,
    /// Edits the layer has not seen yet: it waits for the wave's claims
    /// to clear and for room in the piece pool, and takes them in a batch.
    /// The latest edit per cell. In the snapshot.
    collapse_backlog: BTreeMap<[u32; 3], voxel::world::Edit>,
    /// The data ceiling on one fallen body; the room in the pool caps it
    /// further tick by tick.
    collapse_cap: usize,
}

/// The refused source motion is already normal to the other hull. Subtract
/// that hull's own motion on the same line: a convoy closing slowly should
/// not shove like a head-on collision at the same engine speed.
fn relative_contact_velocity(
    incoming: phys::V3,
    source: [Fx; 3],
    target: [Fx; 3],
    target_velocity: phys::V3,
) -> phys::V3 {
    let line = phys::V3::new(target[0].sub(source[0]), Fx::ZERO, target[2].sub(source[2]));
    let distance2 = line.len_sq();
    if distance2 == Fx::ZERO {
        return phys::V3::ZERO;
    }
    let other_normal = line.scale(target_velocity.dot(line).div(distance2));
    let relative = incoming.sub(other_normal);
    if relative.dot(line) <= Fx::ZERO {
        phys::V3::ZERO
    } else {
        relative
    }
}

impl BlastRange {
    /// Known chunks, component nodes, chunks needing an anchor path and edges.
    pub fn support_graph_size(&self) -> [usize; 4] {
        self.collapse.graph_size()
    }
    pub fn target(&self) -> [i32; 3] {
        if let Some(id) = self.attack {
            if let Some(u) = self.units.get(id) {
                let mut p = u.world_pos([
                    u.body.core.0 as usize,
                    u.body.core.1 as usize,
                    u.body.core.2 as usize,
                ]);
                let dx = Fx::from_int(p[0]).sub(self.gun.motion.pos[0]);
                let dz = Fx::from_int(p[2]).sub(self.gun.motion.pos[2]);
                let time = if self.config.delivery == modding::explosion::Delivery::Laser {
                    Fx::ZERO
                } else {
                    dx.mul(dx)
                        .add(dz.mul(dz))
                        .sqrt()
                        .div(Fx::from_int(self.config.shell_speed))
                        .max(Fx::from_ratio(1, 1000))
                };
                let travel = u.motion.drive.vel.mul(time);
                p[0] += travel
                    .mul(fx::angle::cos(u.motion.drive.head))
                    .to_int_trunc();
                p[2] += travel
                    .mul(fx::angle::sin(u.motion.drive.head))
                    .to_int_trunc();
                return p;
            }
        }
        if let Some(p) = self.aim {
            return p;
        }
        self.layout.as_ref().map_or(
            if self.config.motion == ProjectileMotion::Direct {
                ENERGY_TARGET
            } else {
                TARGET
            },
            |r| r.target,
        )
    }

    pub fn aim_angle(&self) -> i32 {
        let target = self.target();
        let x = Fx::from_int(target[0]).sub(self.gun.motion.pos[0]);
        let z = Fx::from_int(target[2]).sub(self.gun.motion.pos[2]);
        fx::angle::atan2(z.raw(), x.raw())
    }
    /// Actual actuator direction, shared by muzzle, flight and presentation.
    pub fn heading(&self) -> [Fx; 3] {
        let x = fx::angle::cos(self.turret.angle);
        let z = fx::angle::sin(self.turret.angle);
        let length = x.mul(x).add(z.mul(z)).sqrt();
        if length == Fx::ZERO {
            return [Fx::ONE, Fx::ZERO, Fx::ZERO];
        }
        [x.div(length), Fx::ZERO, z.div(length)]
    }
    pub fn muzzle(&self) -> [Fx; 3] {
        if self.gun.kind == RangeUnitKind::Strider {
            let muzzles = self.stream_muzzles();
            if !muzzles.is_empty() {
                // Heavy rounds use one barrel per reload. Keep the launched
                // barrel selected through flight and impact so its charge,
                // projectile trail and flash all start at the same place.
                let fired = u32::from(self.shell.is_some() || self.impact.is_some());
                return muzzles[self.streams.serial.wrapping_sub(fired) as usize % muzzles.len()];
            }
        }
        let d = self.heading();
        [
            self.gun.motion.pos[0].add(d[0].mul(Fx::from_int(30))),
            self.gun.motion.pos[1].add(Fx::from_int(23)),
            self.gun.motion.pos[2].add(d[2].mul(Fx::from_int(30))),
        ]
    }
    pub fn new(config: ExplosionConfig) -> Self {
        Self::build(config, None)
    }
    pub fn with_layout(mut config: ExplosionConfig, layout: RangeLayout) -> Result<Self, String> {
        placement::Validator::default()
            .validate(&layout)
            .map_err(|e| e.to_string())?;
        config.unit_ablation_resistance = layout.unit_resistance;
        Ok(Self::build(config, Some(layout)))
    }

    /// Reuse the occupancy allocation, restoring only damaged or reauthored
    /// columns. Validation precedes mutation: errors preserve the old scene.
    /// On success the caller must replace/discard `self` immediately; its world
    /// has moved into the returned fresh experiment.
    pub fn rebuilt_with_layout(
        &mut self,
        config: ExplosionConfig,
        layout: RangeLayout,
        validator: &placement::Validator,
    ) -> Result<Self, String> {
        self.rebuilt_with_layout_observed(config, layout, validator, |_| {})
    }
    /// Same reset with phase markers for the manual interaction benchmark.
    pub fn rebuilt_with_layout_observed(
        &mut self,
        mut config: ExplosionConfig,
        layout: RangeLayout,
        validator: &placement::Validator,
        mut completed: impl FnMut(&'static str),
    ) -> Result<Self, String> {
        let terrain = validator.prepare(&layout).map_err(|e| e.to_string())?;
        completed("validate");
        config.unit_ablation_resistance = layout.unit_resistance;
        let ai_tuning = self.battle.ai.tuning.clone();
        let Some(old) = self.layout.as_ref().filter(|old| old.size == layout.size) else {
            let world = World::new(
                layout.size[0],
                layout.size[1],
                layout.size[2],
                config.edits_per_tick,
                Box::new(terrain),
            );
            completed("world");
            let witness = config.unit_ablation_resistance > 0;
            let mut next = Self::build_in_world_observed(
                config,
                Some(layout),
                true,
                witness,
                world,
                &mut completed,
            );
            next.configure_enemy_ai(ai_tuning)?;
            completed("ai");
            return Ok(next);
        };
        let areas = RangeTerrain::changed_areas(old, &layout);
        // Leave only a tiny placeholder in the scene being retired.
        let mut world = std::mem::replace(
            &mut self.world,
            World::new(1, 1, 1, 1, Box::new(RangeTerrain::new(false, None))),
        );
        world.reset_generator_regions(Box::new(terrain), &areas);
        completed("world");
        world.max_edits_per_tick = config.edits_per_tick;
        let witness = config.unit_ablation_resistance > 0;
        let mut next = Self::build_in_world_observed(
            config,
            Some(layout),
            true,
            witness,
            world,
            &mut completed,
        );
        next.configure_enemy_ai(ai_tuning)?;
        completed("ai");
        Ok(next)
    }
    pub fn same_authored_terrain(&self, layout: &RangeLayout) -> bool {
        self.layout.as_ref().is_some_and(|old| {
            old.size == layout.size && RangeTerrain::changed_areas(old, layout).is_empty()
        })
    }

    fn build(config: ExplosionConfig, layout: Option<RangeLayout>) -> Self {
        let extended = layout.is_some() || config.motion == ProjectileMotion::Direct;
        let witness = config.unit_ablation_resistance > 0;
        Self::build_scene(config, layout, extended, witness)
    }
    fn build_scene(
        config: ExplosionConfig,
        layout: Option<RangeLayout>,
        extended: bool,
        witness: bool,
    ) -> Self {
        let size = layout.as_ref().map_or(SIZE, |r| r.size);
        let world = World::new(
            size[0],
            size[1],
            size[2],
            config.edits_per_tick,
            Box::new(RangeTerrain::new(extended, layout.clone())),
        );
        Self::build_in_world(config, layout, extended, witness, world)
    }
    fn build_in_world(
        config: ExplosionConfig,
        layout: Option<RangeLayout>,
        extended: bool,
        witness: bool,
        world: World,
    ) -> Self {
        Self::build_in_world_observed(config, layout, extended, witness, world, &mut |_| {})
    }
    fn build_in_world_observed(
        config: ExplosionConfig,
        layout: Option<RangeLayout>,
        extended: bool,
        witness: bool,
        world: World,
        completed: &mut impl FnMut(&'static str),
    ) -> Self {
        let size = [world.nx, world.ny, world.nz];
        let stock =
            crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("stock tuning");
        let mut collapse = CollapseLayer::new(&world);
        collapse.set_max_body_cells(stock.max_body_cells as usize);
        completed("collapse");
        let bodies = Bodies::new(
            stock.max_bodies,
            Fx::from_ratio(
                config.gravity,
                crate::TICKS_PER_SECOND * crate::TICKS_PER_SECOND,
            ),
        );
        let mut units: Vec<RangeUnit> = if let Some(r) = &layout {
            r.units
                .iter()
                .map(|u| {
                    let mut actor = RangeUnit::of_kind(u.kind, u.at);
                    if r.battle && !u.allied {
                        actor.motion.drive.head = fx::angle::FULL / 2;
                        actor.update_pose(actor.motion.drive.head);
                    }
                    actor
                })
                .collect()
        } else {
            witness
                .then(|| RangeUnit::new(WITNESS))
                .into_iter()
                .collect()
        };
        let mut gun = RangeUnit::of_kind(
            layout.as_ref().map_or(RangeUnitKind::Tank, |r| r.gun_kind),
            layout.as_ref().map_or(GUN, |r| r.gun),
        );
        completed("units");
        if let Some(c) = layout.as_ref().and_then(|r| r.course.as_ref()) {
            for u in units.iter_mut().chain(std::iter::once(&mut gun)) {
                u.motion.speed = if u.kind == RangeUnitKind::Tank {
                    c.tank_speed
                } else {
                    c.walker_speed
                };
                u.motion.step = if u.kind == RangeUnitKind::Tank {
                    c.step_height
                } else {
                    c.walker_step_height
                };
                u.motion.arrival = c.arrival;
                u.motion.alignment = c.turn_alignment_pct;
                u.motion.replan = c.replan_ticks;
            }
            for p in &c.patrols {
                units[p.unit].motion.route = p.points.clone();
                units[p.unit].motion.enabled = true;
            }
        }
        let mut matter = matter::Matter::new(&units);
        let combustion = fire::Combustion::new(&units, &mut matter);
        let shield_tuning =
            modding::shield::ShieldTuning::from_ron(include_str!("../../../data/shield.ron"))
                .expect("shield tuning");
        let shields = layout.as_ref().map_or_else(Vec::new, |r| {
            r.shields
                .iter()
                .map(|d| RangeShield::new(d, shield_tuning.capacity))
                .collect()
        });
        let battle = battle::Battle::new(&units, config.unit_ablation_resistance);
        let mut result = Self {
            battle,
            combustion,
            streams: weapons::Streams::default(),
            heat_exchange: heat_exchange::Exchange::default(),
            thermals: thermal::Thermals::default(),
            thermal_tuning: modding::unit_heat::Tuning::from_ron(include_str!(
                "../../../data/unit-heat.ron"
            ))
            .expect("unit heat tuning"),
            matter,
            gun,
            player: false,
            patrols_running: true,
            attack: None,
            auto_attack: false,
            aim: None,
            world,
            size,
            fire_tick: config.fire_tick,
            config,
            tick: 0,
            shell: None,
            velocity: [Fx::ZERO; 3],
            impact: None,
            impact_tick: 0,
            explosion: None,
            removed: 0,
            units,
            layout,
            extended,
            witness,
            removed_materials: [0; 16],
            surface_work: SurfaceWork::new(),
            surface_changed: Vec::new(),
            turret: crate::turret::Traverse::new(0),
            turret_limits: crate::turret::Limits::of(
                modding::turret::Turret::from_ron(include_str!("../../../data/turret.ron"))
                    .expect("turret limits"),
                crate::TICKS_PER_SECOND,
            ),
            navigation: modding::navigation::Navigation::from_ron(include_str!(
                "../../../data/range-navigation.ron"
            ))
            .expect("navigation"),
            shields,
            shield_hits: Vec::new(),
            shield_tuning,
            heat: Heat::default(),
            heat_tuning: modding::heat::HeatTuning::from_ron(include_str!(
                "../../../data/heat.ron"
            ))
            .expect("heat tuning"),
            rubble: nav::vehicle::Rubble::new(),
            collapse,
            bodies,
            collapsed_cells: 0,
            world_steady: true,
            collapse_backlog: BTreeMap::new(),
            collapse_cap: stock.max_body_cells as usize,
        };
        result.turret = crate::turret::Traverse::new(result.aim_angle());
        completed("components");
        result
    }
    pub fn heat_tuning(&self) -> &modding::heat::HeatTuning {
        &self.heat_tuning
    }
    pub fn shield_tuning(&self) -> &modding::shield::ShieldTuning {
        &self.shield_tuning
    }
    /// The first dome whose surface lies between two swept points: a dome
    /// is a wall for a round crossing it either way (ADR-040).
    fn dome_crossed(&self, a: [i32; 3], b: [i32; 3]) -> Option<usize> {
        let (pa, pb) = (shot_point(a), shot_point(b));
        self.shields.iter().position(|s| {
            let d = s.dome();
            inside(&d, &pa) != inside(&d, &pb)
        })
    }
    /// The round is spent against the dome: its wave energy leaves the
    /// charge, and a dome drained to nothing collapses for the tuned outage.
    fn absorb(&mut self, i: usize, at: [i32; 3]) {
        self.absorb_work(i, at, self.config.energy);
    }
    fn absorb_work(&mut self, i: usize, at: [i32; 3], drain: u32) {
        let overload = self.shield_tuning.overload_ticks;
        let s = &mut self.shields[i];
        let drained = drain.min(s.charge);
        let collapsed = s.charge <= drain;
        s.charge -= drained;
        s.hits += 1;
        if collapsed {
            s.down = overload;
        }
        self.shield_hits.push(ShieldHit {
            shield: i,
            at,
            drained,
            collapsed,
        });
    }

    /// Complete authoritative snapshot of this test range. Visual gas is absent.
    /// Side, height and side of this range in world units (ADR-315).
    pub fn size(&self) -> [u32; 3] {
        self.size
    }
    pub fn snapshot(&self) -> Vec<u8> {
        let mut b = fx::blob::Blob::new();
        b.u32(38);
        b.bool(self.extended);
        b.bool(self.witness);
        b.u32(self.fire_tick);
        b.bool(self.layout.is_some());
        if let Some(r) = &self.layout {
            b.u8s(r.to_ron().expect("validated layout").as_bytes());
        }
        self.turret.save(&mut b);
        b.bool(self.player);
        b.bool(self.patrols_running);
        b.usize(self.attack.unwrap_or(usize::MAX));
        b.bool(self.auto_attack);
        b.bool(self.aim.is_some());
        if let Some(p) = self.aim {
            for v in p {
                b.i32(v);
            }
        }
        b.u32(self.tick);
        b.bool(self.shell.is_some());
        if let Some(p) = self.shell {
            for v in p {
                b.fx(v);
            }
        }
        for v in self.velocity {
            b.fx(v);
        }
        b.bool(self.impact.is_some());
        if let Some(at) = self.impact {
            for v in at {
                b.i32(v);
            }
        }
        b.u32(self.impact_tick);
        b.usize(self.removed);
        for n in self.removed_materials {
            b.usize(n);
        }
        b.u32(self.surface_work.len() as u32);
        for (p, work) in &self.surface_work {
            for v in p {
                b.i32(*v);
            }
            b.u32(*work);
        }
        b.u32(self.units.len() as u32 + 1);
        for u in self.units.iter().chain(std::iter::once(&self.gun)) {
            for v in u.at {
                b.i32(v);
            }
            u.motion.save(&mut b);
            b.u64(u.revision);
            b.u32(u.dents.len() as u32);
            for (p, work) in &u.dents {
                for v in p {
                    b.u8(*v);
                }
                b.u32(*work);
            }
            b.usize(u.removed.len());
            b.usize(u.detached);
            for p in &u.removed {
                for v in p {
                    b.u8(*v);
                }
            }
            b.bool(u.section.is_some());
            if let Some(j) = &u.section {
                j.save_cut(&mut b);
            }
            b.u32(u.seg_removed.len() as u32);
            for (seg, p) in &u.seg_removed {
                b.u8(*seg);
                for v in p {
                    b.u8(*v);
                }
            }
            b.u32(u.seg_dents.len() as u32);
            for ((seg, p), work) in &u.seg_dents {
                b.u8(*seg);
                for v in p {
                    b.u8(*v);
                }
                b.u32(*work);
            }
            for gone in &u.seg_gone {
                b.bool(*gone);
            }
            b.u32(u.seg_shed.len() as u32);
            for seg in &u.seg_shed {
                b.u8(*seg);
            }
            b.usize(u.seg_detached);
        }
        self.world.save(&mut b);
        self.matter.save(&mut b);
        b.bool(self.explosion.is_some());
        if let Some(e) = &self.explosion {
            e.save(&mut b);
        }
        b.u32(self.shields.len() as u32);
        for s in &self.shields {
            b.u32(s.charge);
            b.u32(s.down);
            b.u32(s.hits);
        }
        self.heat.save(&mut b);
        self.collapse.save(&mut b);
        b.u64(self.collapsed_cells);
        b.u32(self.collapse_backlog.len() as u32);
        for (p, e) in &self.collapse_backlog {
            for v in p {
                b.u32(*v);
            }
            b.u32(u32::from(e.mat));
        }
        self.bodies.save(&mut b);
        b.bool(self.world_steady);
        self.streams.save(&mut b);
        self.thermals.save(&mut b);
        self.heat_exchange.save(&mut b);
        self.combustion.save(&mut b);
        self.save_structure(&mut b);
        self.battle.save(&mut b);
        b.v
    }
    fn save_structure(&self, b: &mut fx::blob::Blob) {
        for u in &self.units {
            for job in &u.seg_sections {
                b.bool(job.is_some());
                if let Some(job) = job {
                    job.save_cut(b);
                }
            }
        }
    }

    pub fn restore(config: ExplosionConfig, bytes: &[u8]) -> Option<Self> {
        let mut s = fx::blob::Scan::new(bytes);
        let version = s.u32();
        if ![27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38].contains(&version) {
            return None;
        }
        let extended = s.bool();
        let witness = s.bool();
        let fire_tick = s.u32();
        let mut r = if s.bool() {
            let data = s.u8s();
            if data.len() > 65536 {
                return None;
            }
            Self::with_layout(
                config,
                RangeLayout::from_ron(std::str::from_utf8(&data).ok()?).ok()?,
            )
            .ok()?
        } else {
            Self::build_scene(config, None, extended, witness)
        };
        if r.extended != extended {
            return None;
        }
        r.witness = witness;
        r.turret = crate::turret::Traverse::load(&mut s, r.turret_limits)?;
        r.player = s.bool();
        r.patrols_running = s.bool();
        let attack = s.usize();
        if attack != usize::MAX {
            if attack >= r.units.len() {
                return None;
            }
            r.attack = Some(attack);
        }
        r.auto_attack = version >= 30 && s.bool();
        if s.bool() {
            r.aim = Some(std::array::from_fn(|_| s.i32()));
        }
        r.fire_tick = fire_tick;
        r.tick = s.u32();
        if s.bool() {
            r.shell = Some(std::array::from_fn(|_| s.fx()));
        }
        r.velocity = std::array::from_fn(|_| s.fx());
        if s.bool() {
            r.impact = Some(std::array::from_fn(|_| s.i32()));
        }
        r.impact_tick = s.u32();
        r.removed = s.usize();
        r.removed_materials = std::array::from_fn(|_| s.usize());
        let count = s.u32() as usize;
        if count > bytes.len() / 16 {
            return None;
        }
        for _ in 0..count {
            let p = std::array::from_fn(|_| s.i32());
            let work = s.u32();
            if work == 0
                || work > 1_000_000
                || (0..3).any(|a| p[a] < 0 || p[a] >= r.size[a] as i32)
                || r.surface_work.insert(p, work).is_some()
            {
                return None;
            }
        }
        if s.u32() as usize != r.units.len() + 1 {
            return None;
        }
        let gun_id = r.units.len();
        let turret_angle = r.turret.angle;
        for (id, u) in r
            .units
            .iter_mut()
            .chain(std::iter::once(&mut r.gun))
            .enumerate()
        {
            let at = std::array::from_fn(|_| s.i32());
            if !u.motion.load_version(&mut s, version >= 29) {
                return None;
            }
            let head = u.motion.drive.head;
            u.update_pose(if id == gun_id { turret_angle } else { head });
            if u.at != at {
                return None;
            }
            let revision = s.u64();
            let count = s.u32() as usize;
            if count > u.body.solid_count() as usize {
                return None;
            }
            for _ in 0..count {
                let p = [s.u8(), s.u8(), s.u8()];
                let work = s.u32();
                if p[0] >= u.body.vol.nx
                    || p[1] >= u.body.vol.ny
                    || p[2] >= u.body.vol.nz
                    || !u.body.vol.get(p[0] as usize, p[1] as usize, p[2] as usize)
                    || work == 0
                    || work >= 1_000_000
                    || u.dents.insert(p, work).is_some()
                {
                    return None;
                }
            }
            let count = s.usize();
            u.detached = s.usize();
            if u.detached > count {
                return None;
            }
            if count > u.body.solid_count() as usize {
                return None;
            }
            for _ in 0..count {
                let p = [s.u8(), s.u8(), s.u8()];
                let [x, y, z] = p.map(usize::from);
                if x >= u.body.vol.nx as usize
                    || y >= u.body.vol.ny as usize
                    || z >= u.body.vol.nz as usize
                    || !u.body.vol.get(x, y, z)
                    || u.dents.contains_key(&p)
                {
                    return None;
                }
                u.body.remove(x, y, z);
                u.removed.push(p);
            }
            u.revision = revision;
            u.update_response();
            if s.bool() {
                u.section = Some(crate::damage::SectionJob::load_cut(
                    id as u32,
                    0,
                    &mut s,
                    &u.body,
                    &mut u.connectivity,
                )?);
            }
            if (u.status.alive || version >= 35) && u.status.detached > 0 && u.section.is_none() {
                return None;
            }
            let count = s.u32() as usize;
            if count > bytes.len() / 4 {
                return None;
            }
            for _ in 0..count {
                let seg = s.u8() as usize;
                let p = [s.u8(), s.u8(), s.u8()];
                let [x, y, z] = p.map(usize::from);
                let segment = u.body.segments.get_mut(seg)?;
                let b = &mut segment.body;
                if x >= b.vol.nx as usize
                    || y >= b.vol.ny as usize
                    || z >= b.vol.nz as usize
                    || !b.vol.get(x, y, z)
                {
                    return None;
                }
                b.remove(x, y, z);
                u.seg_removed.push((seg as u8, p));
            }
            let count = s.u32() as usize;
            if count > bytes.len() / 8 {
                return None;
            }
            for _ in 0..count {
                let seg = s.u8() as usize;
                let p = [s.u8(), s.u8(), s.u8()];
                let work = s.u32();
                let segment = u.body.segments.get(seg)?;
                let [x, y, z] = p.map(usize::from);
                if x >= segment.body.vol.nx as usize
                    || y >= segment.body.vol.ny as usize
                    || z >= segment.body.vol.nz as usize
                    || !segment.body.vol.get(x, y, z)
                    || work == 0
                    || work >= 1_000_000
                    || u.seg_dents.insert((seg as u8, p), work).is_some()
                {
                    return None;
                }
            }
            for i in 0..u.body.segments.len() {
                u.seg_gone[i] = s.bool();
            }
            let count = s.u32() as usize;
            if count > u.body.segments.len() {
                return None;
            }
            for _ in 0..count {
                let seg = s.u8();
                if seg as usize >= u.body.segments.len() || !u.seg_gone[seg as usize] {
                    return None;
                }
                u.seg_shed.push(seg);
            }
            // A segment already handed to debris has no cells left; one
            // still waiting for pool room keeps them until its turn.
            for i in 0..u.body.segments.len() {
                if u.seg_gone[i] && !u.seg_shed.contains(&(i as u8)) {
                    let b = &mut u.body.segments[i].body;
                    for z in 0..b.vol.nz as usize {
                        for y in 0..b.vol.ny as usize {
                            for x in 0..b.vol.nx as usize {
                                b.remove(x, y, z);
                            }
                        }
                    }
                }
            }
            u.seg_detached = s.usize();
            // A lost leg is a shed thigh, and the motion's own record of it
            // must agree; a buckled leg is one whose links lost the buckling
            // share, unless it is gone altogether.
            let buckled = u.integrity_model.buckled(&u.body);
            for (li, leg) in u.body.legs.iter().enumerate() {
                let lost = u.seg_gone[leg.thigh as usize];
                if u.motion.legs_lost.get(li).copied() != Some(lost) {
                    return None;
                }
                if !lost && u.motion.legs_buckled.get(li).copied() != buckled.get(li).copied() {
                    return None;
                }
            }
        }
        if !r.world.load(&mut s) {
            return None;
        }
        r.matter.load(&mut s, &r.world, &r.units)?;
        if r.surface_work.iter().any(|(p, work)| {
            let m = material(&r.world, *p);
            m == 0 || *work >= 1_000_000
        }) {
            return None;
        }
        if s.bool() {
            r.explosion = Some(Explosion::load(&mut s, &r.units, r.size, version >= 36)?);
        }
        if s.u32() as usize != r.shields.len() {
            return None;
        }
        for sh in &mut r.shields {
            sh.charge = s.u32();
            sh.down = s.u32();
            sh.hits = s.u32();
            if sh.charge > r.shield_tuning.capacity || sh.down > r.shield_tuning.overload_ticks {
                return None;
            }
        }
        r.heat = Heat::load(&mut s, &r.world, &r.heat_tuning)?;
        if !r.collapse.load(&mut s) {
            return None;
        }
        r.collapsed_cells = s.u64();
        let backlog = s.u32();
        if backlog > 1 << 20 {
            return None;
        }
        for _ in 0..backlog {
            let (x, y, z) = (s.u32(), s.u32(), s.u32());
            let mat = s.u32();
            if mat > u32::from(u16::MAX) {
                return None;
            }
            let e = if mat == 0 {
                voxel::world::Edit::dig(x, y, z)
            } else {
                voxel::world::Edit::put(x, y, z, mat as u16)
            };
            r.collapse_backlog.insert([x, y, z], e);
        }
        if !r.bodies.load_version(&mut s, version >= 37) {
            return None;
        }
        r.world_steady = s.bool();
        if version >= 31 {
            r.streams.load(&mut s)?;
        }
        if version >= 32 {
            r.thermals.load(&mut s, &r.units, &r.thermal_tuning)?;
        }
        if version >= 33 {
            r.heat_exchange.load(&mut s, r.size, &r.units)?;
        }
        if version >= 34 {
            r.combustion.load(&mut s, &r.units, &r.matter, r.size)?;
        } else {
            if r.matter
                .economy
                .ledger
                .snapshot()
                .0
                .iter()
                .any(|c| c.kind == crate::matter::Kind::Fuel)
            {
                return None;
            }
            r.combustion = fire::Combustion::empty(r.units.len());
        }
        if version >= 35 {
            for (id, u) in r.units.iter_mut().enumerate() {
                for i in 0..u.seg_sections.len() {
                    if s.bool() {
                        let mask = u.segment_loose_mask(i);
                        u.seg_sections[i] = Some(crate::damage::SectionJob::load_domain(
                            id as u32,
                            0,
                            &mut s,
                            &u.body.segments[i].body,
                            &mask,
                        )?);
                    }
                }
            }
        } else {
            // Old snapshots could leave dead roots and severed limbs
            // suspended. Rebuild their structural work under today's rules.
            for (id, u) in r.units.iter_mut().enumerate() {
                u.refresh_damage(id);
            }
        }
        if version >= 36 {
            let allied: Vec<_> = r
                .layout
                .as_ref()
                .map(|l| l.units.iter().map(|u| u.allied).collect())
                .unwrap_or_default();
            r.battle.load(
                &mut s,
                &r.units,
                r.size,
                r.turret_limits,
                (version, r.tick, &allied),
            )?;
            if r.battle_mode() {
                for (u, a) in r.units.iter_mut().zip(&r.battle.actors) {
                    u.update_pose(a.turret.angle);
                }
            }
        }
        (s.ok() && s.left() == 0).then_some(r)
    }

    pub fn state_hash(&self) -> u64 {
        self.snapshot()
            .into_iter()
            .fold(0xcbf29ce484222325, |h, v| {
                (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
            })
    }

    /// Charge another round against the current, already damaged world.
    pub fn ready(&self) -> bool {
        !self.battle_mode()
            && self.gun.status.alive
            && self.gun.status.weapon
            && self.streams.ready(self.tick)
            && self.shell.is_none()
            && (self.fire_tick == u32::MAX || self.tick > self.fire_tick)
            && self
                .explosion
                .as_ref()
                .is_none_or(|e| e.wave.finished() && e.pending.is_empty())
            && self.world.queued() == 0
            && self.units.iter().all(|u| u.section.is_none())
    }
    pub fn refire(&mut self) -> bool {
        if !self.ready() {
            return false;
        }
        self.fire_tick = self.tick + self.config.charge_ticks + 1;
        self.impact = None;
        self.explosion = None;
        true
    }

    pub fn control(&mut self) {
        if !self.player && self.impact.is_none() && self.shell.is_none() {
            self.fire_tick = u32::MAX;
        }
        self.player = true;
    }
    pub fn order_move(&mut self, to: [i32; 2]) -> bool {
        self.set_auto_attack(false);
        self.control();
        self.attack = None;
        self.gun.motion.route.clear();
        self.gun.motion.move_to(
            &self.world,
            to,
            &self.gun.body,
            &self
                .units
                .iter()
                .map(|u| motion::Obstacle::of(&u.motion, &u.body))
                .collect::<Vec<_>>(),
            &self.rubble,
        )
    }
    pub fn order_attack(&mut self, id: usize) -> bool {
        if id >= self.units.len() || !self.units[id].status.alive {
            return false;
        }
        self.set_auto_attack(false);
        self.control();
        self.attack = Some(id);
        self.gun.motion.stop();
        true
    }
    /// All placed range units are targets. Team rules belong to the battle
    /// scenario; this mode never retargets a manual order (ADR-338).
    pub fn set_auto_attack(&mut self, enabled: bool) {
        if self.auto_attack && !enabled {
            self.gun.motion.stop();
        }
        if self.auto_attack || enabled {
            self.attack = None;
            // Cancel a charged but unfired automatic round. A flying round
            // and its explosion remain physical events when the mode ends.
            if self.shell.is_none() && self.fire_tick >= self.tick {
                self.fire_tick = u32::MAX;
            }
        }
        self.auto_attack = enabled && self.layout.is_some();
        if self.auto_attack {
            self.control();
            self.gun.motion.stop();
        }
    }
    fn reload_ticks(&self) -> u32 {
        self.layout
            .as_ref()
            .and_then(|r| r.course.as_ref())
            .map_or(0, |c| c.reload_ticks)
    }
    fn stream_tuning(&self) -> Option<&modding::range_weapon::Stream> {
        match self.config.delivery {
            modding::explosion::Delivery::Flame => Some(&self.streams.tuning.flame),
            modding::explosion::Delivery::MachineGun => Some(&self.streams.tuning.machine_gun),
            modding::explosion::Delivery::Laser => Some(&self.streams.tuning.laser),
            modding::explosion::Delivery::Blast | modding::explosion::Delivery::Kinetic => None,
        }
    }
    fn acquire_auto_target(&mut self) {
        if !self.auto_attack {
            return;
        }
        if !self.gun.status.alive || !self.gun.status.weapon {
            self.set_auto_attack(false);
            return;
        }
        let radius = self
            .layout
            .as_ref()
            .map_or(0, |r| i64::from(r.auto_attack_range));
        let from = self.gun.motion.pos.map(Fx::to_int_trunc);
        // Layout validation caps this scan at eight slots. Integer world
        // coordinates keep squared distance safe on future 10 km layouts.
        let distance = |id: usize| -> Option<i64> {
            let u = self.units.get(id)?;
            let p = u.motion.pos.map(Fx::to_int_trunc);
            let dx = i64::from(p[0]) - i64::from(from[0]);
            let dz = i64::from(p[2]) - i64::from(from[2]);
            let d = dx * dx + dz * dz;
            (u.status.alive && d <= radius * radius).then_some(d)
        };
        if self.attack.is_some_and(|id| distance(id).is_some()) {
            return;
        }
        let chosen = (0..self.units.len())
            .filter_map(|id| distance(id).map(|d| (d, id)))
            .min()
            .map(|(_, id)| id);
        self.attack = None;
        if self.stream_tuning().is_some() && self.gun.motion.destination.is_some() {
            self.gun.motion.stop();
        }
        if self.shell.is_none() && self.fire_tick >= self.tick {
            self.fire_tick = u32::MAX;
        }
        // A new target does not bypass reload or replace a live projectile.
        if self.ready()
            && (self.impact.is_none()
                || self.tick.saturating_sub(self.impact_tick) >= self.reload_ticks())
        {
            self.attack = chosen;
        }
    }
    /// A manually ordered stream weapon closes to a checked firing point.
    /// Keep the saved route while its destination is still useful; restarting
    /// the bounded planner each tick would strand the vehicle indefinitely.
    fn keep_stream_target_in_range(&mut self, id: usize) -> bool {
        let Some(stream) = self.stream_tuning() else {
            return true;
        };
        let reach = stream.range;
        let standoff = reach * stream.standoff_milli / 1000;
        let from = self.gun.motion.pos;
        let to = self.units[id].motion.pos;
        let dx = to[0].sub(from[0]);
        let dz = to[2].sub(from[2]);
        let distance = dx.mul(dx).add(dz.mul(dz)).sqrt();
        if distance <= Fx::from_int(reach) {
            if self.gun.motion.destination.is_some() {
                self.gun.motion.stop();
            }
            return true;
        }
        let approach = distance.sub(Fx::from_int(standoff)).div(distance);
        let dest = [
            from[0].add(dx.mul(approach)).to_int_trunc(),
            from[2].add(dz.mul(approach)).to_int_trunc(),
        ];
        let changed = self.gun.motion.destination.is_none_or(|old| {
            (old[0] - dest[0]).abs().max((old[1] - dest[1]).abs()) > reach - standoff
        });
        if changed {
            let others: Vec<_> = self
                .units
                .iter()
                .map(|u| motion::Obstacle::of(&u.motion, &u.body))
                .collect();
            self.gun
                .motion
                .move_to(&self.world, dest, &self.gun.body, &others, &self.rubble);
        }
        false
    }
    /// Отмена приказа: цель снята, машина останавливается.
    pub fn order_stop(&mut self) {
        self.set_auto_attack(false);
        self.attack = None;
        self.gun.motion.stop();
        self.streams.left = 0;
        if self.shell.is_none() {
            self.fire_tick = u32::MAX;
        }
    }
    /// Пуск и остановка маршрутов мишеней.
    pub fn toggle_patrols(&mut self) {
        self.patrols_running = !self.patrols_running;
    }
    /// Выстрел по точке земли вместо тела: прежняя цель снимается.
    /// Возвращает `false`, если орудие ещё занято.
    pub fn order_ground_fire(&mut self, at: [i32; 3]) -> bool {
        self.set_auto_attack(false);
        self.attack = None;
        self.aim = Some(at);
        self.refire()
    }
    /// Замена пресета оружия.
    ///
    /// Стойкость тел задаёт **раскладка**, а не пресет: она пришла из
    /// `range.ron` и переживает смену оружия. Правило живёт здесь, потому
    /// что забыть его на стороне вызывающего — значит молча обнулить
    /// стойкость мишеней при каждом переключении.
    pub fn reconfigure(&mut self, config: ExplosionConfig) {
        self.config = config;
        if let Some(r) = &self.layout {
            self.config.unit_ablation_resistance = r.unit_resistance;
        }
    }
    pub fn step(&mut self) {
        self.step_observed(|_| {});
    }
    /// Report completed phases to a read-only observer. The simulation never
    /// reads clocks or consumes a result from it; benches own their timers.
    pub fn step_observed(&mut self, mut completed: impl FnMut(&'static str)) {
        self.surface_changed.clear();
        self.shield_hits.clear();
        // Resting debris is floor for this tick's motion (ADR-292), except
        // a piece lying against a live hull's flank under that hull
        // (ADR-302).
        let hulls: Vec<crate::burst::Hull> = self
            .units
            .iter()
            .chain((!self.battle_mode()).then_some(&self.gun))
            .filter(|u| u.status.alive)
            .map(|u| {
                let o = motion::Obstacle::of(&u.motion, &u.body);
                crate::burst::Hull {
                    pos: o.pos,
                    head: o.head,
                    half: o.half,
                    height: Fx::from_ratio(
                        u.body.vol.ny as i32 * u.body.mm as i32,
                        voxel::unit::WORLD_MM as i32,
                    ),
                    vel: [Fx::ZERO; 3],
                    step: Fx::from_int(u.motion.step),
                }
            })
            .collect();
        self.rubble =
            self.matter
                .debris
                .rubble(&self.world, &self.navigation.fell.materials, &hulls);
        completed("rubble");
        // Domes recover before this tick's round flies: a collapsed one
        // counts its outage down, a standing one refills to capacity.
        for s in &mut self.shields {
            if s.down > 0 {
                s.down -= 1;
            } else {
                s.charge = s
                    .charge
                    .saturating_add(self.shield_tuning.recharge_per_tick)
                    .min(self.shield_tuning.capacity);
            }
        }
        // Commit motion in stable spatial order. Later actors see accepted
        // positions, so two approaching tanks cannot each consume the same
        // free gap. Physical contact must not depend on which unit was placed
        // into the first battle slot.
        let before: Vec<[Fx; 3]> = self.units.iter().map(|u| u.motion.pos).collect();
        let gun_before = self.gun.motion.pos;
        let mut positions: Vec<_> = self
            .units
            .iter()
            .map(|u| motion::Obstacle::of(&u.motion, &u.body))
            .collect();
        // Bound concurrent starts as well as each search's work (ADR-312,
        // ADR-326). Hulls over this ceiling retain their orders and ask
        // again next tick, in the same stable slot order.
        let battle_mode = self.battle_mode();
        let mut starts = self.navigation.searches_started_per_tick;
        let mut contacts = Vec::new();
        let mut motion_order: Vec<_> = (0..self.units.len()).collect();
        motion_order.sort_unstable_by_key(|&id| {
            let p = self.units[id].motion.pos;
            (p[0], p[2], id)
        });
        for id in motion_order {
            let u = &mut self.units[id];
            u.motion.allow_start(starts > 0);
            {
                let others: Vec<_> = positions
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != id)
                    .map(|(_, p)| *p)
                    .chain(
                        (!battle_mode)
                            .then(|| motion::Obstacle::of(&self.gun.motion, &self.gun.body)),
                    )
                    .collect();
                if self.patrols_running
                    && (!battle_mode || self.battle.phase == battle::Phase::Fighting)
                {
                    u.motion.step(
                        &self.world,
                        &u.body,
                        u.status.alive,
                        u.status.engine,
                        u.mobility,
                        &others,
                        self.tick,
                        self.config.gravity,
                        &self.rubble,
                        self.world_steady,
                    );
                } else {
                    u.motion.passive(
                        &self.world,
                        &self.rubble,
                        &u.body,
                        self.config.gravity,
                        u.mobility,
                        &others,
                    );
                }
                let head = if battle_mode {
                    self.battle.actors[id].turret.angle
                } else {
                    u.motion.drive.head
                };
                u.update_pose(head);
                positions[id] = motion::Obstacle::of(&u.motion, &u.body);
                if let Some((other, velocity)) = u.motion.take_contact() {
                    let target = if other < id { other } else { other + 1 };
                    contacts.push((id, target, velocity));
                }
            }
            starts = starts.saturating_sub(usize::from(u.motion.started_search()));
        }
        if !self.battle_mode() {
            self.gun.motion.allow_start(starts > 0);
            self.gun.motion.step(
                &self.world,
                &self.gun.body,
                self.gun.status.alive,
                self.gun.status.engine,
                self.gun.mobility,
                &positions,
                self.tick,
                self.config.gravity,
                &self.rubble,
                self.world_steady,
            );
            self.gun.update_pose(self.turret.angle);
            if let Some((target, velocity)) = self.gun.motion.take_contact() {
                contacts.push((self.units.len(), target, velocity));
            }
        }
        // Position acceptance above remains in stable slot order. Sample all
        // contact velocities before transferring any shove: a hull touched
        // twice in one tick must not make the second contact order-dependent.
        let contact_state: Vec<_> = if contacts.is_empty() {
            Vec::new()
        } else {
            (0..=self.units.len())
                .map(|id| {
                    let motion = if id == self.units.len() {
                        &self.gun.motion
                    } else {
                        &self.units[id].motion
                    };
                    let velocity = phys::V3::new(
                        fx::angle::cos(motion.drive.head).mul(motion.drive.vel),
                        Fx::ZERO,
                        fx::angle::sin(motion.drive.head).mul(motion.drive.vel),
                    )
                    .add(motion.shove);
                    (motion.pos, velocity)
                })
                .collect()
        };
        for (source, target, incoming) in contacts {
            let velocity = relative_contact_velocity(
                incoming,
                contact_state[source].0,
                contact_state[target].0,
                contact_state[target].1,
            );
            let mass = |id: usize| {
                if id == self.units.len() {
                    self.gun.motion.chassis.mass.max(1)
                } else {
                    self.units[id].motion.chassis.mass.max(1)
                }
            };
            let source_mass = mass(source);
            let target_mass = mass(target);
            let transferred = velocity.scale(Fx::from_ratio(
                source_mass,
                source_mass.saturating_add(target_mass),
            ));
            if target == self.units.len() {
                self.gun.motion.receive_contact(transferred);
            } else {
                self.units[target].motion.receive_contact(transferred);
            }
        }
        // Hulls cut yielding vegetation along their path; the crown that
        // hung from those cells follows over the next ticks.
        let cuts: Vec<(Vec<[u32; 3]>, [i32; 3])> = self
            .units
            .iter()
            .chain((!self.battle_mode()).then_some(&self.gun))
            .filter(|u| u.status.alive && !u.motion.cut.is_empty())
            .map(|u| (u.motion.cut.clone(), u.at))
            .collect();
        for (cut, at) in cuts {
            self.matter
                .fell(&mut self.world, &cut, at, &self.navigation.fell);
        }
        self.matter
            .step_felling(&mut self.world, &self.navigation.fell);
        if battle_mode {
            completed("motion");
        }
        if !battle_mode {
            self.acquire_auto_target();
            let target_angle = self.aim_angle();
            self.turret.step(target_angle, self.turret_limits);
            if let Some(id) = self.attack {
                if !self.units[id].status.alive {
                    self.attack = None;
                    if self.stream_tuning().is_some() && self.gun.motion.destination.is_some() {
                        self.gun.motion.stop();
                    }
                } else {
                    let in_range = self.keep_stream_target_in_range(id);
                    if !in_range && self.shell.is_none() && self.fire_tick >= self.tick {
                        self.fire_tick = u32::MAX;
                    }
                    let reload = self.reload_ticks();
                    if in_range
                        && (self.fire_tick == u32::MAX
                            || (self.impact.is_some() && self.tick >= self.impact_tick + reload))
                    {
                        self.refire();
                    }
                }
            }
            completed("motion");
            let dt = Fx::from_ratio(1, crate::TICKS_PER_SECOND);
            let gravity = match self.config.motion {
                ProjectileMotion::Ballistic => self.config.gravity,
                ProjectileMotion::Direct => 0,
            };
            if self.tick == self.fire_tick && !self.turret.aligned(target_angle, self.turret_limits)
            {
                // Keep the charged round until the actual barrel has traversed.
                self.fire_tick = self.fire_tick.saturating_add(1);
            }
            if self.tick == self.fire_tick
                && self.gun.status.alive
                && self.gun.status.weapon
                && matches!(
                    self.config.delivery,
                    modding::explosion::Delivery::Blast | modding::explosion::Delivery::Kinetic
                )
            {
                let from = self.muzzle();
                let target = self.target();
                let dx = Fx::from_int(target[0]).sub(from[0]);
                let dz = Fx::from_int(target[2]).sub(from[2]);
                let time = dx
                    .mul(dx)
                    .add(dz.mul(dz))
                    .sqrt()
                    .div(Fx::from_int(self.config.shell_speed))
                    .max(Fx::from_ratio(1, 1000));
                self.velocity = [
                    self.heading()[0].mul(Fx::from_int(self.config.shell_speed)),
                    Fx::from_int(target[1])
                        .sub(from[1])
                        .div(time)
                        .add(Fx::from_int(gravity).mul(time).mul(Fx::from_ratio(1, 2))),
                    self.heading()[2].mul(Fx::from_int(self.config.shell_speed)),
                ];
                self.shell = Some(from);
                if self.gun.kind == RangeUnitKind::Strider {
                    self.streams.serial = self.streams.serial.wrapping_add(1);
                }
            }
            self.step_streams();
            if let Some(p) = self.shell {
                self.velocity[1] = self.velocity[1].sub(Fx::from_int(gravity).mul(dt));
                let q = std::array::from_fn(|a| p[a].add(self.velocity[a].mul(dt)));
                // Sweep at sub-voxel intervals; never jump through a thin target.
                let span = (0..3)
                    .map(|a| q[a].sub(p[a]).abs().to_int_trunc())
                    .max()
                    .unwrap_or(0)
                    + 1;
                let mut impact = None;
                let mut caught = None;
                let mut prev = p.map(Fx::to_int_trunc);
                let mut last_free = p;
                for s in 1..=span * 2 {
                    let f = Fx::from_ratio(s, span * 2);
                    let sample = std::array::from_fn(|a| p[a].add(q[a].sub(p[a]).mul(f)));
                    let v = sample.map(Fx::to_int_trunc);
                    if let Some(i) = self.dome_crossed(prev, v) {
                        caught = Some((i, v));
                        break;
                    }
                    if self.layout.as_ref().is_some_and(|r| {
                        !self.player && r.airburst_on_target && v[0] >= r.target[0]
                    }) || material(&self.world, v) != 0
                        || self.units.iter().any(|u| u.solid(v))
                    {
                        impact = Some((v, prev, last_free));
                        break;
                    }
                    prev = v;
                    last_free = sample;
                }
                if let Some((i, at)) = caught {
                    // Spent against the dome: no impact, no crater, another
                    // order may follow at once.
                    self.absorb(i, at);
                    self.shell = None;
                    self.fire_tick = u32::MAX;
                } else if let Some((at, from, last_free)) = impact {
                    self.impact = Some(at);
                    self.impact_tick = self.tick;
                    if self.config.delivery == modding::explosion::Delivery::Kinetic
                        && self.kinetic_ricochet(at, from)
                    {
                        self.shell = Some(last_free);
                    } else if self.config.delivery == modding::explosion::Delivery::Kinetic {
                        self.kinetic_impact(at);
                        self.shell = None;
                    } else {
                        self.heat.deposit(
                            &self.world,
                            &self.matter.economy.mats,
                            at,
                            self.config.heat_units,
                            self.config.heat_per_cell,
                        );
                        self.matter
                            .begin(self.config.material_energy_j, self.config.fragment_box);
                        self.explosion = Some(Explosion::begin(at, &self.config, &self.units));
                        self.shell = None;
                    }
                } else if (0..3)
                    .any(|a| q[a] < Fx::ZERO || q[a] >= Fx::from_int(self.size[a] as i32))
                {
                    // A miss retires at the map boundary, allowing another order.
                    self.shell = None;
                    self.fire_tick = u32::MAX;
                } else {
                    self.shell = Some(q);
                }
            }
            completed("flight");
            {
                // The grid of resistance is captured under a ceiling per tick
                // (ADR-311): a charge that reaches further costs the burst
                // more ticks rather than one longer tick. The wave waits for
                // the whole grid, so how the capture is sliced never decides
                // what the wave finds.
                let Self {
                    explosion,
                    world,
                    config,
                    units,
                    surface_work,
                    ..
                } = self;
                if let Some(e) = explosion.as_mut() {
                    if e.capturing() {
                        e.capture(
                            world,
                            config,
                            units,
                            surface_work,
                            config.capture_cells_per_tick,
                        );
                    }
                }
            }
            completed("capture");
            if let Some(e) = self.explosion.as_mut().filter(|e| !e.capturing()) {
                let before: Vec<_> = self
                    .units
                    .iter()
                    .map(|u| (u.removed.len(), u.seg_removed.len()))
                    .collect();
                e.step(
                    &mut self.world,
                    self.config.wave_work_per_tick,
                    &mut self.units,
                    &mut self.surface_work,
                    &mut self.surface_changed,
                    &self.config,
                    (self.config.material_energy_j > 0).then_some(&mut self.matter),
                );
                for (id, (u, (n, m))) in self.units.iter_mut().zip(before).enumerate() {
                    if u.removed.len() != n || u.seg_removed.len() != m {
                        if self.config.material_energy_j == 0 {
                            self.matter
                                .annihilate_body(id, u.removed.len() - n + u.seg_removed.len() - m);
                        }
                        u.refresh_damage(id);
                    }
                }
                // The wave's pressure on a body pushes its hull (ADR-297): what
                // dents and breaks the side is what throws the machine, so a
                // hull behind cover or out of the wave is not pushed at all.
                // The push lands on the hull's near face at the burst's height
                // (ADR-302): sideways from a blast beside the hull, upward from
                // one under its belly.
                let milli = u64::from(self.config.hull_impulse_milli);
                let at = e.at;
                for u in &mut self.units {
                    if u.absorbed > 0 {
                        if u.status.alive {
                            let newton_s = u64::from(u.absorbed).saturating_mul(milli) / 1000;
                            let through = u.motion.impulse(&u.body, at, newton_s);
                            u.last_push = Some((at, through, u.absorbed, newton_s));
                        }
                        u.absorbed = 0;
                    }
                }
            }
        }
        self.battle_tick_observed(&mut completed);
        completed("wave");
        self.step_unit_fire();
        completed("unit_fire");
        self.exchange_heat();
        completed("heat_exchange");
        {
            // Fire consumes voxels through the same edit queue as the wave;
            // cells the wave has claimed wait for it.
            let Self {
                heat,
                world,
                matter,
                heat_tuning,
                ..
            } = self;
            heat.step(world, &matter.economy.mats, heat_tuning, |p| {
                matter.debris.claimed(p)
            });
            // Melted voxels run off as debris (ADR-290); one the pool
            // cannot take keeps its heat for the next tick.
            for p in heat.melting.clone() {
                if matter.melt(world, p) {
                    heat.forget(p);
                    heat.melted_total += 1;
                }
            }
        }
        self.step_unit_heat();
        self.world_steady = self.world.flush().applied == 0;
        completed("heat_flush");
        // Everything without support falls (ADR-299): the batch's collapse
        // layer finds what the edits left hanging and digs it out as
        // bodies. The cohesive cells of a body fall on as the body and
        // bake back where it lands (ADR-300); its loose cells crumble
        // into debris pieces at once, or sink under an annihilating
        // weapon. The layer waits for the wave to finish — a slow wave
        // would otherwise lose cells to bodies torn off a half-dug crater
        // — and for its claims to clear, so that it never takes a claimed
        // cell from under the wave. Cohesive bodies have their own pool;
        // spare particle slots must not decide whether a building can fall.
        for e in self.world.edits() {
            self.collapse_backlog.insert([e.x, e.y, e.z], *e);
        }
        let mut crumbs: Vec<([u32; 3], u16)> = Vec::new();
        let wave_working =
            self.explosion.as_ref().is_some_and(|e| e.working()) || self.battle.working();
        if (!self.collapse_backlog.is_empty() || self.collapse.pending())
            && !wave_working
            && self.matter.debris.claims() == 0
        {
            self.collapse.set_max_body_cells(self.collapse_cap);
            let edits: Vec<voxel::world::Edit> = self.collapse_backlog.values().copied().collect();
            self.collapse
                .apply_observed(&mut self.world, &edits, &mut self.bodies, &mut completed);
            self.collapse_backlog.clear();
            // Each cell goes by its own matter, not by the body it came
            // off in — else the wave's slicing would decide what crumbles.
            // A body torn off this tick keeps its cohesive cells, rebuilt
            // without the loose ones, which crumble; one of loose cells
            // alone is all crumbs. Bodies already falling pass through.
            let mats = &self.matter.economy.mats;
            let mut whole = Vec::new();
            for b in std::mem::take(&mut self.bodies.list) {
                if !self.bodies.spawned.contains(&b.id) {
                    whole.push(b);
                    continue;
                }
                self.collapsed_cells += b.voxels.len() as u64;
                let mut keep: Vec<([u32; 3], u16)> = Vec::new();
                let mut shed: Vec<([u32; 3], u16)> = Vec::new();
                for v in &b.voxels {
                    let p = [
                        (b.ox + i64::from(v.x)) as u32,
                        (b.oy + i64::from(v.y)) as u32,
                        (b.oz + i64::from(v.z)) as u32,
                    ];
                    if mats.get(v.mat).is_some_and(|d| !d.loose) {
                        keep.push((p, v.mat));
                    } else {
                        shed.push((p, v.mat));
                    }
                }
                if shed.is_empty() {
                    whole.push(b);
                    continue;
                }
                crumbs.append(&mut shed);
                if keep.is_empty() {
                    continue;
                }
                // The cells are still in the world: the layer's digs are
                // queued, not applied, so the rest of the body reads from it.
                let cells: Vec<(u32, u32, u32)> =
                    keep.iter().map(|(p, _)| (p[0], p[1], p[2])).collect();
                match voxel::bodies::Body::from_world_cells(&self.world, &cells) {
                    Some(mut rest) => {
                        rest.id = b.id;
                        whole.push(rest);
                    }
                    None => crumbs.append(&mut keep),
                }
            }
            self.bodies.list = whole;
            self.bodies.spawned.clear();
            if let Some(layout) = &self.layout {
                let before = self.bodies.list.len();
                plate::fracture(layout, &mut self.world, &mut self.bodies, &edits, |mat| {
                    self.matter
                        .economy
                        .mats
                        .get(mat)
                        .map(|m| (m.strength, m.density, m.bend_span))
                });
                self.collapsed_cells += self.bodies.list[before..]
                    .iter()
                    .map(|b| b.voxels.len() as u64)
                    .sum::<u64>();
                self.bodies.spawned.clear();
            }
        }
        if !crumbs.is_empty() {
            // Crumbled cells are removed matter now held by pieces, or
            // sunk; the layer's digs are nobody's and not counted below.
            let returned = if battle_mode || self.config.material_energy_j > 0 {
                self.matter.collapse(&mut self.world, &crumbs).1
            } else {
                self.matter.annihilate_cells(&crumbs);
                0
            };
            self.removed += crumbs.len().saturating_sub(returned);
            for (_, mat) in &crumbs {
                if let Some(n) = self.removed_materials.get_mut(usize::from(*mat)) {
                    *n += 1;
                }
            }
        }
        completed("collapse");
        // Whole bodies fall a voxel at a time and bake back where they
        // land, through the edit queue like everything else.
        self.bodies.step(&mut self.world);
        let work = self.matter.step_sections(&mut self.units);
        self.matter.shed_segments_budget(&mut self.units, work);
        // Live hulls catch falling pieces (ADR-293) and bulldoze the ones
        // they drive into (ADR-294); the dead lie as debris.
        let hulls: Vec<crate::burst::Hull> = self
            .units
            .iter()
            .zip(&before)
            .chain((!battle_mode).then_some((&self.gun, &gun_before)))
            .filter(|(u, _)| u.status.alive)
            .map(|(u, was)| {
                let o = motion::Obstacle::of(&u.motion, &u.body);
                crate::burst::Hull {
                    pos: o.pos,
                    head: o.head,
                    half: o.half,
                    height: Fx::from_ratio(
                        u.body.vol.ny as i32 * u.body.mm as i32,
                        voxel::unit::WORLD_MM as i32,
                    ),
                    vel: [0, 1, 2].map(|k| o.pos[k].sub(was[k])),
                    step: Fx::from_int(u.motion.step),
                }
            })
            .collect();
        self.matter.flush(
            &self.world,
            battle_mode || self.config.material_energy_j > 0,
            self.config.gravity,
            u64::from(self.tick),
            &hulls,
            &self.navigation.fell.materials,
        );
        completed("debris");
        for e in self.world.edits() {
            if e.mat == 0 {
                self.surface_work
                    .remove(&[e.x as i32, e.y as i32, e.z as i32]);
            }
        }
        // What left the world for good this tick: a cell dug and put back
        // within one drain (ADR-299) was not removed, and nobody's digs —
        // the collapse layer's — are counted where they crumble (ADR-300)
        // or not at all when the body bakes back.
        let mut dug: BTreeMap<[u32; 3], u16> = BTreeMap::new();
        for &(e, previous) in self.world.completed() {
            let p = [e.x, e.y, e.z];
            if e.mat == 0 {
                if previous != 0
                    && e.by != voxel::world::NOBODY
                    && e.expected_material().is_none_or(|x| x == previous)
                {
                    dug.insert(p, previous);
                }
            } else {
                dug.remove(&p);
            }
        }
        self.removed += dug.len();
        for &mat in dug.values() {
            if let Some(n) = self.removed_materials.get_mut(mat as usize) {
                *n += 1;
            }
        }
        self.tick += 1;
        completed("accounting");
    }
}

impl Explosion {
    fn save(&self, b: &mut fx::blob::Blob) {
        for v in self.at {
            b.i32(v);
        }
        for v in self.origin {
            b.i32(v);
        }
        b.i32(self.cell);
        b.usize(self.queued);
        b.usize(self.captured);
        self.wave.save(b);
        b.usize(self.contact_cursor);
        b.usize(self.pending.len());
        for hit in &self.pending {
            for v in hit.cell {
                b.i32(v);
            }
            b.u32(hit.pressure);
            b.bool(hit.broken);
        }
        b.u32(self.poses.len() as u32);
        for f in &self.poses {
            for v in [f.base, f.x, f.y, f.z] {
                b.fx(v.x);
                b.fx(v.y);
                b.fx(v.z);
            }
            b.fx(f.scale);
            for v in f.centre {
                b.fx(v);
            }
        }
        for frames in &self.seg_poses {
            b.u32(frames.len() as u32);
            for f in frames {
                for v in [f.base, f.x, f.y, f.z] {
                    b.fx(v.x);
                    b.fx(v.y);
                    b.fx(v.z);
                }
                b.fx(f.scale);
                for v in f.centre {
                    b.fx(v);
                }
            }
        }
        for d in &self.turret_deltas {
            b.i32(*d);
        }
    }
    fn load(
        s: &mut fx::blob::Scan,
        units: &[RangeUnit],
        size: [u32; 3],
        aimed: bool,
    ) -> Option<Self> {
        let at = std::array::from_fn(|_| s.i32());
        let origin = std::array::from_fn(|_| s.i32());
        let cell = s.i32();
        let queued = s.usize();
        let captured = s.usize();
        let wave = Wave::load(s)?;
        if captured > wave.size().pow(3) {
            return None;
        }
        let contact_cursor = s.usize();
        let n = s.usize();
        if n > 8192 {
            return None;
        }
        let mut pending = std::collections::VecDeque::new();
        for _ in 0..n {
            let hit = phys::blast::Hit {
                cell: std::array::from_fn(|_| s.i32()),
                pressure: s.u32(),
                broken: s.bool(),
            };
            if hit.cell.iter().any(|&v| v < 0 || v >= wave.size() as i32)
                || hit.pressure > 1_000_000
            {
                return None;
            }
            pending.push_back(hit);
        }
        if !(1..=4).contains(&cell)
            || contact_cursor >= (cell as usize).pow(3) * (units.len() + 1)
            || (pending.is_empty() && contact_cursor != 0)
            || (0..3).any(|a| {
                at[a] < 0
                    || at[a] >= size[a] as i32
                    || i64::from(origin[a])
                        != i64::from(at[a]) - (wave.size() / 2) as i64 * i64::from(cell)
            })
        {
            return None;
        }
        let n = s.u32() as usize;
        if n != units.len() {
            return None;
        }
        let mut poses = Vec::new();
        for u in units {
            let mut v = || phys::V3::new(s.fx(), s.fx(), s.fx());
            let (base, x, y, z) = (v(), v(), v(), v());
            let scale = s.fx();
            let centre = std::array::from_fn(|_| s.fx());
            if scale != u.pose.scale
                || centre != u.pose.centre
                || [base.x, base.y, base.z].iter().any(|v| {
                    *v < Fx::from_int(-384)
                        || *v > Fx::from_int(size.into_iter().max().unwrap_or(1024) as i32 + 384)
                })
                || [x, y, z].iter().any(|v| {
                    [v.x, v.y, v.z]
                        .iter()
                        .any(|c| *c < Fx::from_int(-2) || *c > Fx::from_int(2))
                })
            {
                return None;
            }
            poses.push(crate::damage::Frame {
                base,
                x,
                y,
                z,
                scale,
                centre,
            });
        }
        let mut seg_poses = Vec::new();
        for u in units {
            let n = s.u32() as usize;
            if n != u.body.segments.len() {
                return None;
            }
            let mut frames = Vec::with_capacity(n);
            for _ in 0..n {
                let mut v = || phys::V3::new(s.fx(), s.fx(), s.fx());
                let (base, x, y, z) = (v(), v(), v(), v());
                let scale = s.fx();
                let centre = std::array::from_fn(|_| s.fx());
                if scale <= Fx::ZERO
                    || [base.x, base.y, base.z].iter().any(|v| {
                        *v < Fx::from_int(-384)
                            || *v
                                > Fx::from_int(size.into_iter().max().unwrap_or(1024) as i32 + 384)
                    })
                    || [x, y, z].iter().any(|v| {
                        [v.x, v.y, v.z]
                            .iter()
                            .any(|c| *c < Fx::from_int(-2) || *c > Fx::from_int(2))
                    })
                {
                    return None;
                }
                frames.push(crate::damage::Frame {
                    base,
                    x,
                    y,
                    z,
                    scale,
                    centre,
                });
            }
            seg_poses.push(frames);
        }
        let turret_deltas = if aimed {
            (0..units.len()).map(|_| s.i32()).collect()
        } else {
            vec![0; units.len()]
        };
        if turret_deltas
            .iter()
            .any(|d| i64::from(*d).abs() > i64::from(fx::angle::FULL))
        {
            return None;
        }
        Some(Self {
            turret_deltas,
            poses,
            seg_poses,
            pending,
            contact_cursor,
            at,
            origin,
            cell,
            queued,
            wave,
            captured,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_local_map_matches_frame_at_integer_voxels() {
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let unit = RangeUnit::of_kind(kind, [128, 16, 336]);
            let frames = std::iter::once((&unit.body, unit.pose))
                .chain(std::iter::once((
                    &unit.body,
                    crate::damage::turned(&unit.pose, 2713, 0),
                )))
                .chain(
                    unit.body
                        .segments
                        .iter()
                        .zip(&unit.seg_frames)
                        .map(|(seg, frame)| (&seg.body, *frame)),
                );
            for (body, frame) in frames {
                let map = LocalMap::new(&frame);
                let centre = [
                    frame.base.x.to_int_trunc(),
                    frame.base.y.to_int_trunc(),
                    frame.base.z.to_int_trunc(),
                ];
                for dz in (-20..=20).step_by(2) {
                    for dy in (-10..=20).step_by(2) {
                        for dx in (-20..=20).step_by(2) {
                            let p = [centre[0] + dx, centre[1] + dy, centre[2] + dz];
                            assert_eq!(
                                map.local_in(body, p),
                                local_in(body, p, &frame),
                                "{kind:?} {p:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn contact_uses_closing_speed_between_two_moving_hulls() {
        let source = [Fx::ZERO; 3];
        let target = [Fx::from_int(20), Fx::ZERO, Fx::ZERO];
        let along = |speed| phys::V3::new(Fx::from_int(speed), Fx::ZERO, Fx::ZERO);
        let stopped = relative_contact_velocity(along(10), source, target, along(0));
        let convoy = relative_contact_velocity(along(10), source, target, along(8));
        let head_on = relative_contact_velocity(along(10), source, target, along(-8));
        let separating = relative_contact_velocity(along(10), source, target, along(12));
        assert_eq!(stopped, along(10));
        assert!(convoy.x > Fx::ZERO && convoy.x < stopped.x);
        assert!(head_on.x > stopped.x);
        assert_eq!(separating, phys::V3::ZERO);
    }
    #[test]
    fn cached_stock_templates_keep_damage_private_to_each_unit() {
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let mut first = RangeUnit::of_kind(kind, [200, 24, 200]);
            let second = RangeUnit::of_kind(kind, [240, 24, 200]);
            let cell = (0..first.body.vol.nz as usize)
                .flat_map(|z| (0..first.body.vol.ny as usize).map(move |y| (y, z)))
                .find_map(|(y, z)| {
                    (0..first.body.vol.nx as usize)
                        .find(|&x| first.body.vol.get(x, y, z))
                        .map(|x| (x, y, z))
                })
                .expect("stock unit has solid root cells");
            assert!(first.body.remove(cell.0, cell.1, cell.2));
            assert!(second.body.vol.get(cell.0, cell.1, cell.2));
            assert!(RangeUnit::of_kind(kind, [280, 24, 200])
                .body
                .vol
                .get(cell.0, cell.1, cell.2));
        }
    }
    fn auto_range() -> BlastRange {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.blocks.clear();
        layout.shields.clear();
        layout.clear_min = [64, 240];
        layout.clear_max = [480, 570];
        layout.course.as_mut().unwrap().patrols.clear();
        layout.course.as_mut().unwrap().ramps.clear();
        layout.units.truncate(3);
        for (u, at) in layout
            .units
            .iter_mut()
            .zip([[300, 16, 336], [128, 16, 508], [430, 16, 336]])
        {
            u.at = at;
        }
        layout.auto_attack_range = 200;
        let mut r = BlastRange::with_layout(c, layout).unwrap();
        r.control();
        r
    }
    #[test]
    fn automatic_targeting_has_stable_ties_range_and_reload() {
        let mut r = auto_range();
        r.set_auto_attack(true);
        r.acquire_auto_target();
        assert_eq!(r.attack, Some(0), "equal distances resolve by slot");
        r.units[0].status.alive = false;
        r.impact = Some([300, 20, 336]);
        r.impact_tick = r.tick;
        r.acquire_auto_target();
        assert!(r.attack.is_none(), "retarget bypassed reload");
        r.tick += r.reload_ticks();
        r.acquire_auto_target();
        assert_eq!(r.attack, Some(1));
        r.units[1].motion.pos[2] = Fx::from_int(900);
        r.acquire_auto_target();
        assert!(r.attack.is_none(), "selected an out-of-range target");
        assert!(r.auto_attack, "mode must wait for a new target");
        r.units[2].motion.pos[0] = Fx::from_int(310);
        r.acquire_auto_target();
        assert_eq!(r.attack, Some(2));
    }
    #[test]
    fn auto_attack_detects_then_closes_to_the_active_weapon_range() {
        let mut r = auto_range();
        r.layout.as_mut().unwrap().auto_attack_range = 1200;
        for unit in r.units.iter_mut().skip(1) {
            unit.status.alive = false;
        }
        // Dead hulls remain physical cover; move the third target away so
        // this test measures range and pursuit rather than occlusion.
        r.units[2].motion.pos[2] = Fx::from_int(550);
        let [x, _, z] = r.gun.motion.pos.map(Fx::to_int_trunc);
        r.units[0].motion.pos[0] = Fx::from_int(x + 315);
        r.units[0].motion.pos[2] = Fx::from_int(z);
        r.reconfigure(
            ExplosionConfig::from_ron(include_str!("../../../data/flamethrower.ron")).unwrap(),
        );
        r.set_auto_attack(true);
        r.acquire_auto_target();
        assert_eq!(r.attack, Some(0), "sensor did not see the target");
        r.step();
        assert!(r.gun.motion.destination.is_some(), "flame did not pursue");
        assert_eq!(r.streams.emitted(), 0, "flame fired beyond its range");
        let mut replay = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
        for _ in 0..300 {
            replay.step();
            if replay.units[0].revision > 0 {
                break;
            }
        }
        assert!(replay.streams.emitted() > 0);
        assert!(replay.units[0].revision > 0, "automatic approach never hit");
        r.set_auto_attack(false);
        assert!(r.gun.motion.destination.is_none(), "cancel kept the route");
        r.units[0].motion.pos[0] = Fx::from_int(x + 250);
        r.set_auto_attack(true);
        r.acquire_auto_target();
        assert_eq!(r.attack, Some(0));

        r.set_auto_attack(false);
        r.units[0].motion.pos[0] = Fx::from_int(x + 350);
        r.reconfigure(
            ExplosionConfig::from_ron(include_str!("../../../data/machine-gun.ron")).unwrap(),
        );
        r.set_auto_attack(true);
        r.acquire_auto_target();
        assert_eq!(r.attack, Some(0), "machine gun lost its longer reach");
        r.step();
        assert!(
            r.gun.motion.destination.is_none(),
            "machine gun pursued in range"
        );
    }
    #[test]
    fn manual_flame_attack_approaches_before_firing_and_reaches_the_target() {
        let config =
            ExplosionConfig::from_ron(include_str!("../../../data/flamethrower.ron")).unwrap();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.blocks.clear();
        layout.shields.clear();
        layout.course.as_mut().unwrap().patrols.clear();
        layout.course.as_mut().unwrap().ramps.clear();
        layout.clear_min = [64, 240];
        layout.clear_max = [480, 570];
        layout.units.truncate(1);
        layout.units[0].at = [443, 16, 336];
        let mut r = BlastRange::with_layout(config, layout).unwrap();
        r.control();
        let start = r.gun.motion.pos[0];
        assert!(r.order_attack(0));
        r.step();
        assert!(r.gun.motion.destination.is_some(), "no approach route");
        assert_eq!(r.streams.emitted(), 0, "fired beyond flame range");
        let mut abandoned = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
        abandoned.units[0].status.alive = false;
        abandoned.step();
        assert!(abandoned.attack.is_none());
        assert!(abandoned.gun.motion.destination.is_none());
        let mut moving = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
        let old_destination = moving.gun.motion.destination;
        moving.units[0].motion.pos[2] = Fx::from_int(546);
        moving.step();
        assert_ne!(moving.gun.motion.destination, old_destination);
        for _ in 0..300 {
            r.step();
            if r.units[0].revision > 0 {
                break;
            }
        }
        assert!(r.gun.motion.pos[0] > start, "tank did not approach");
        assert!(r.streams.emitted() > 0, "never fired from range");
        assert!(r.units[0].revision > 0, "flame never touched the target");
    }
    #[test]
    fn manual_orders_cancel_automatic_charge_and_mode() {
        for command in 0..4 {
            let mut r = auto_range();
            r.set_auto_attack(true);
            r.acquire_auto_target();
            assert!(r.refire());
            assert_ne!(r.fire_tick, u32::MAX);
            match command {
                0 => {
                    r.order_stop();
                }
                1 => {
                    r.order_move([200, 336]);
                }
                2 => {
                    assert!(r.order_attack(1));
                }
                _ => {
                    assert!(r.order_ground_fire([320, 16, 336]));
                }
            }
            assert!(!r.auto_attack);
            if command < 2 {
                assert!(r.attack.is_none());
                assert_eq!(r.fire_tick, u32::MAX, "cancelled charge still fires");
            }
            if command == 2 {
                assert_eq!(r.attack, Some(1));
            }
        }
        let mut r = auto_range();
        r.set_auto_attack(true);
        r.shell = Some([Fx::from_int(200), Fx::from_int(30), Fx::from_int(336)]);
        let shell = r.shell;
        r.set_auto_attack(false);
        assert_eq!(r.shell, shell, "turning off erased an airborne round");
    }
    #[test]
    fn automatic_fire_damages_a_target_and_resumes_bit_for_bit() {
        let mut a = auto_range();
        a.set_auto_attack(true);
        a.step();
        assert_eq!(a.attack, Some(0));
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert!(b.auto_attack);
        let mut impacted = false;
        for tick in 0..240 {
            a.step();
            b.step();
            impacted |= a.impact.is_some();
            assert_eq!(
                a.state_hash(),
                b.state_hash(),
                "automatic fire diverged at {tick}"
            );
        }
        assert!(impacted);
        assert!(
            !a.units[0].removed.is_empty(),
            "auto fire did not damage the acquired target"
        );
    }
    #[test]
    fn range_version_29_restores_with_auto_attack_off() {
        let mut r = auto_range();
        r.set_auto_attack(true);
        r.acquire_auto_target();
        r.matter = matter::Matter::new(&r.units);
        r.combustion = fire::Combustion::empty(r.units.len());
        let mut bytes = r.snapshot();
        let mut tail = fx::blob::Blob::new();
        r.streams.save(&mut tail);
        r.thermals.save(&mut tail);
        r.heat_exchange.save(&mut tail);
        r.combustion.save(&mut tail);
        r.save_structure(&mut tail);
        r.battle.save(&mut tail);
        bytes.truncate(bytes.len() - tail.v.len());
        let mut s = fx::blob::Scan::new(&bytes);
        s.u32();
        s.bool();
        s.bool();
        s.u32();
        assert!(s.bool());
        s.u8s();
        crate::turret::Traverse::load(&mut s, r.turret_limits).unwrap();
        s.bool();
        s.bool();
        s.usize();
        let offset = bytes.len() - s.left();
        bytes.remove(offset);
        bytes[..4].copy_from_slice(&29u32.to_le_bytes());
        let old = BlastRange::restore(r.config.clone(), &bytes).unwrap();
        assert!(!old.auto_attack);
        assert_eq!(old.attack, r.attack);
    }
    #[test]
    fn range_engine_damage_uses_the_shared_physical_mobility_response() {
        // Compare propulsion on flat ground; route search latency is tested
        // separately and must not turn this into two stationary hulls.
        struct Flat;
        impl Generator for Flat {
            fn material(&self, _x: u32, y: u32, _z: u32) -> u16 {
                u16::from(y < 16)
            }
        }
        let world = World::new(640, 64, 640, 100, Box::new(Flat));
        let rubble = nav::vehicle::Rubble::new();
        let mut intact = RangeUnit::new(GUN);
        let mut damaged = RangeUnit::new(GUN);
        for z in 0..30 {
            for y in 0..30 {
                for x in 0..16 {
                    if damaged.body.vol.get(x, y, z)
                        && damaged.body.part_at(x, y, z) == voxel::Part::Engine
                    {
                        damaged.body.remove(x, y, z);
                    }
                }
            }
        }
        damaged.update_response();
        assert!(damaged.status.alive && damaged.status.engine);
        assert!(
            damaged.mobility.engine > Fx::ZERO && damaged.mobility.engine < Fx::from_ratio(1, 2)
        );
        for u in [&mut intact, &mut damaged] {
            assert!(u.motion.move_to(&world, [205, 336], &u.body, &[], &rubble));
        }
        for tick in 0..30 {
            for u in [&mut intact, &mut damaged] {
                u.motion.step(
                    &world,
                    &u.body,
                    u.status.alive,
                    u.status.engine,
                    u.mobility,
                    &[],
                    tick,
                    config().gravity,
                    &rubble,
                    true,
                );
            }
        }
        assert!(damaged.motion.drive.vel < intact.motion.drive.vel);
        assert!(damaged.motion.pos[0] < intact.motion.pos[0]);
        assert!(
            damaged.motion.pos[0] > Fx::from_int(GUN[0]),
            "damaged engine still has some thrust"
        );
    }
    #[test]
    fn a_severed_barrel_stops_an_already_charged_range_shot() {
        let c = config();
        let mut r = BlastRange::new(c);
        for z in 0..30 {
            for y in 0..30 {
                r.gun.body.remove(54, y, z);
            }
        }
        r.gun.status = r.gun.body.status(&mut r.gun.connectivity);
        assert!(r.gun.status.alive && !r.gun.status.weapon);
        r.fire_tick = 0;
        r.step();
        assert!(r.shell.is_none() && r.impact.is_none());
        assert!(!r.refire());
    }

    #[test]
    fn a_track_break_brakes_the_range_tank_and_resumes_without_powered_turning() {
        let c = config();
        let mut a = BlastRange::new(c.clone());
        a.control();
        a.gun.motion.drive.vel = Fx::from_int(30);
        for y in 0..10 {
            for z in 1..7 {
                if a.gun.body.remove(30, y, z) {
                    a.gun.removed.push([30, y as u8, z as u8]);
                }
            }
        }
        a.gun.refresh_damage(a.units.len());
        assert!(a.gun.status.alive && a.gun.status.engine && a.gun.status.weapon);
        assert_eq!(a.gun.mobility.traction, Fx::ZERO);
        a.order_move([205, 336]);
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        let mut first = Fx::ZERO;
        for i in 0..120 {
            a.step();
            b.step();
            if i == 0 {
                first = a.gun.motion.drive.vel;
            }
            assert_eq!(a.state_hash(), b.state_hash());
            assert_eq!(a.gun.motion.drive.head, 0);
        }
        assert!(first > Fx::ZERO && first < Fx::from_int(30));
        assert_eq!(a.gun.motion.drive.vel, Fx::ZERO);
        assert!(a.gun.motion.pos[0] < Fx::from_int(205));
    }
    #[test]
    fn changing_aim_traverses_before_firing_and_resumes_mid_turn() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::new(c.clone());
        a.control();
        let start = a.muzzle();
        a.aim = Some([128, 30, 520]);
        assert_eq!(start, a.muzzle(), "new aim teleported the barrel");
        assert!(a.refire());
        for _ in 0..10 {
            a.step();
        }
        assert!(a.shell.is_none());
        assert!(a.turret.rate > 0 && a.turret.angle != 0);
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        let mut fired = false;
        for _ in 0..200 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            if a.shell.is_some() {
                assert!(a.turret.aligned(a.aim_angle(), a.turret_limits));
                let speed = a.velocity[0]
                    .mul(a.velocity[0])
                    .add(a.velocity[2].mul(a.velocity[2]))
                    .sqrt();
                assert!(
                    speed.sub(Fx::from_int(a.config.shell_speed)).abs() < Fx::from_ratio(1, 1000)
                );
                fired = true;
                break;
            }
        }
        assert!(fired);
    }

    #[test]
    fn range_sections_wait_atomically_resume_and_keep_mass_and_pose() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        a.patrols_running = false;
        let initial = a.units[0].body.solid_count();
        let u = &mut a.units[0];
        // Cut the protruding barrel while retaining the core and turret.
        for z in 0..30 {
            for y in 0..30 {
                if u.body.remove(54, y, z) {
                    u.removed.push([54, y as u8, z as u8]);
                }
            }
        }
        u.revision = u.removed.len() as u64;
        a.matter.annihilate_body(0, u.removed.len());
        u.refresh_damage(0);
        assert!(u.status.alive && u.status.detached > 8);
        let detached = u.status.detached;
        let after_cut = u.body.solid_count();
        let sink = a.matter.economy.ledger.sunk_total();
        for mut budget in [1, 40, 100] {
            let u = &mut a.units[0];
            u.section.as_mut().unwrap().advance(&u.body, &mut budget);
            let restored = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
            assert_eq!(a.state_hash(), restored.state_hash(), "partial extraction");
        }
        a.matter.tuning.max_flying = 0;
        for _ in 0..30 {
            a.matter.step_sections(&mut a.units);
        }
        assert_eq!(a.units[0].body.solid_count(), after_cut);
        assert!(a.matter.debris.fragments.is_empty());
        assert!(a.units[0].section.as_ref().unwrap().ready());
        let snapshot = a.snapshot();
        let mut b = BlastRange::restore(c, &snapshot).unwrap();
        a.matter.tuning.max_flying = b.matter.tuning.max_flying;
        assert_eq!(a.state_hash(), b.state_hash());
        let frame = a.units[0].pose;
        for _ in 0..40 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!(a.units[0].section.is_none());
        assert_eq!(a.units[0].body.solid_count(), after_cut - detached);
        assert_eq!(a.units[0].status.detached, 0);
        assert_eq!(a.matter.economy.ledger.sunk_total(), sink);
        assert!(a.matter.economy.ledger.balanced());
        let fragments = &a.matter.debris.fragments;
        assert!(
            fragments.iter().any(|f| f.count > 8),
            "barrel was reduced to crumbs"
        );
        assert_eq!(fragments.iter().map(|f| f.count).sum::<u32>(), detached);
        assert_eq!(
            fragments.iter().map(|f| f.mass_g).sum::<u64>(),
            u64::from(detached) * 7850
        );
        let mut cells = std::collections::BTreeSet::new();
        for f in fragments {
            let chunk = f.chunk.as_ref().unwrap();
            assert_eq!(chunk.cells.len(), f.count as usize);
            assert_eq!(chunk.frames[0].base, frame.base);
            for c in &chunk.cells {
                assert!(cells.insert(*c));
            }
        }
        assert_eq!(
            initial,
            a.units[0].body.solid_count() + a.units[0].removed.len() as u32
        );
        let restored = BlastRange::restore(b.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(
            a.state_hash(),
            restored.state_hash(),
            "saved whole fragment"
        );
    }
    /// The owner's second report: craters came out square at high power
    /// (P-21, ADR-303). A double charge of the energy cannon into a wide
    /// flat clearing wants a crater wider than the data's wave grid; the
    /// grid grows with the charge and the wave stops at the sphere in it,
    /// so the rim reaches as far along the axes as along the diagonals.
    #[test]
    fn a_high_charge_crater_stays_round_where_the_grid_would_cut_it() {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        c.energy *= 2;
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.clear_min = [150, 150];
        layout.clear_max = [550, 550];
        layout.units.clear();
        layout.target = [350, FLOOR - 1, 350];
        layout.blocks.clear();
        layout.airburst_on_target = false;
        if let Some(course) = &mut layout.course {
            course.patrols.clear();
        }
        let mut r = BlastRange::with_layout(c, layout).unwrap();
        for _ in 0..300 {
            r.step();
            if r.impact.is_some() {
                break;
            }
        }
        let at = r.impact.expect("the bolt never landed");
        let grid = r.explosion.as_ref().map(|e| e.wave.size()).unwrap_or(0);
        assert!(grid > 41, "the grid did not grow with the charge: {grid}");
        for _ in 0..40 {
            r.step();
        }
        let y = FLOOR - 1;
        let reach = |dx: i32, dz: i32| {
            (1..120)
                .take_while(|&k| material(&r.world, [at[0] + dx * k, y, at[2] + dz * k]) == 0)
                .count() as i64
        };
        let axes = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dz)| reach(dx, dz) * 1000);
        let diagonals = [(1, 1), (1, -1), (-1, 1), (-1, -1)].map(|(dx, dz)| reach(dx, dz) * 1414);
        let radii: Vec<i64> = axes.into_iter().chain(diagonals).collect();
        let (low, high) = (
            radii.iter().copied().min().unwrap(),
            radii.iter().copied().max().unwrap(),
        );
        eprintln!("grid {grid} radii (thousandths): axes {axes:?} diagonals {diagonals:?}");
        assert!(low >= 40_000, "no crater to speak of: {radii:?}");
        // Before ADR-303 the axes stopped at the grid's face, 41 units, and
        // the diagonals ran on to its corners, 58: a square.
        assert!(
            (high - low) * 100 <= high * 15,
            "the crater is not round: {radii:?}"
        );
    }
    /// The owner's report: craters came out square (ADR-274). A shell into
    /// flat soil must dig a crater whose rim reaches as far along the
    /// diagonals as along the axes.
    #[test]
    fn a_shell_crater_in_flat_soil_is_round() {
        let mut r = BlastRange::new(config());
        // Flat lane soil short of the concrete target and the steel revetment.
        r.aim = Some([200, FLOOR, 336]);
        for _ in 0..600 {
            r.step();
            if r.impact.is_some() && r.ready() {
                break;
            }
        }
        let at = r.impact.expect("the shell never landed");
        let column: Vec<u16> = (0..=FLOOR + 2)
            .map(|y| material(&r.world, [at[0], y, at[2]]))
            .collect();
        eprintln!("impact {at:?} removed {} column {column:?}", r.removed);
        let reach_at = |layer: i32, dx: i32, dz: i32| {
            (1..40)
                .take_while(|&k| material(&r.world, [at[0] + dx * k, layer, at[2] + dz * k]) == 0)
                .count() as i64
        };
        // The widest layer: the rim. Across the lane the rock-strewn shoulders
        // clip the bowl, so the lane axis and the four diagonals are compared;
        // the diagonals stay inside the lane at this radius.
        let layer = (at[1] - 8..=at[1].min(FLOOR - 1))
            .max_by_key(|&y| reach_at(y, 1, 0) + reach_at(y, -1, 0))
            .unwrap();
        let reach = |dx: i32, dz: i32| reach_at(layer, dx, dz);
        let axes = [(1, 0), (-1, 0)].map(|(dx, dz)| reach(dx, dz) * 1000);
        let diagonals = [(1, 1), (1, -1), (-1, 1), (-1, -1)].map(|(dx, dz)| reach(dx, dz) * 1414);
        let radii: Vec<i64> = axes.into_iter().chain(diagonals).collect();
        let (low, high) = (
            radii.iter().copied().min().unwrap(),
            radii.iter().copied().max().unwrap(),
        );
        eprintln!(
            "crater layer {layer} radii (thousandths): axes {axes:?} diagonals {diagonals:?}"
        );
        assert!(low >= 3000, "no crater to speak of: {radii:?}");
        // One diagonal cell is 1.41 units, so single rays quantise coarsely;
        // the means must agree closely, every ray roughly. Before ADR-274
        // the diagonals ran a quarter longer than the axes.
        let axis = axes.iter().sum::<i64>() / axes.len() as i64;
        let diagonal = diagonals.iter().sum::<i64>() / diagonals.len() as i64;
        assert!(
            (axis - diagonal).abs() * 100 <= axis * 12,
            "the crater is not round: axes {axis} diagonals {diagonal}"
        );
        assert!(high * 100 <= low * 125, "a ray is far off: {radii:?}");
    }
    #[test]
    fn heavy_shell_scars_steel_cover_without_hitting_the_sheltered_tank() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.course = None;
        layout.clear_min = [64, 64];
        layout.clear_max = [600, 600];
        layout.units.truncate(1);
        layout.units[0].at = [300, 16, 336];
        layout.target = [286, 20, 336];
        layout.blocks = vec![modding::range::RangeBlock {
            min: [260, 0, 290],
            max: [269, 64, 382],
            material: 3,
        }];
        let mut stock = config();
        let mut heavy = stock.clone();
        heavy.energy *= 4;
        heavy.material_energy_j *= 4;
        for (c, should_chip) in [(&mut stock, false), (&mut heavy, true)] {
            let mut sim = BlastRange::with_layout(c.clone(), layout.clone()).unwrap();
            for _ in 0..240 {
                sim.step();
            }
            assert!(sim.impact.is_some());
            assert_eq!(sim.removed > 0, should_chip);
            assert_eq!(
                sim.units[0].removed.len() + sim.units[0].seg_removed.len(),
                0
            );
            assert!(sim.units[0].status.alive);
            assert!(
                !sim.surface_work.is_empty(),
                "the exposed face has no scars"
            );
        }
    }
    /// Trees are matter a tracked hull pushes over (ADR-271): the cut is paid
    /// by the vehicle, the crown falls for free, and nothing is erased.
    /// The default range as the capture script drives it: the player's
    /// tank is sent from its start into the far woods with the patrols
    /// running. Its first turn happens beside two boulders that no planner
    /// sample sees at the heading the executor reaches; it must leave that
    /// spot on its own and arrive (ADR-275).
    #[test]
    fn the_player_tank_reaches_the_far_woods_from_the_start() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c, layout).unwrap();
        a.control();
        assert!(a.order_move([96, 540]));
        let mut still = 0;
        let mut longest_still = 0;
        let mut last = [a.gun.motion.pos[0], a.gun.motion.pos[2]];
        let mut arrived = None;
        for tick in 0..1500 {
            a.step();
            let m = &a.gun.motion;
            let here = [m.pos[0], m.pos[2]];
            if here == last {
                still += 1;
                longest_still = longest_still.max(still);
            } else {
                still = 0;
            }
            last = here;
            if m.destination.is_none() {
                arrived = Some(tick);
                break;
            }
        }
        eprintln!("arrived {arrived:?} longest stand {longest_still} ticks");
        assert!(
            arrived.is_some(),
            "stuck at {:?} for {longest_still} ticks",
            a.gun.motion.pos
        );
        // Ninety ticks of waiting for the next try (`replan_ticks`) and
        // the slices of the search that follows it (ADR-306).
        assert!(longest_still <= 120, "stood {longest_still} ticks");
    }
    /// One walker alone on the open pad: a body with legs to shoot at.
    fn walker_layout() -> RangeLayout {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.units = vec![modding::range::RangeUnitDef {
            allied: false,
            kind: RangeUnitKind::Strider,
            at: [300, FLOOR, 336],
        }];
        layout.blocks.clear();
        layout.airburst_on_target = false;
        if let Some(c) = &mut layout.course {
            c.patrols.clear();
        }
        layout
    }
    /// A walker ordered across the pad with the patrols running.
    fn walking_walker() -> BlastRange {
        let mut a = BlastRange::with_layout(config(), walker_layout()).unwrap();
        a.patrols_running = true;
        let u = &mut a.units[0];
        u.motion.enabled = true;
        u.motion.destination = Some([420, 336]);
        u.motion.path = vec![[420, 336]];
        a
    }
    fn near_v3(a: phys::V3, b: phys::V3, tol: Fx) -> bool {
        a.sub(b).len_sq() < tol.mul(tol)
    }
    /// A parked walker's feet plant at the template's rest ankles and its
    /// leg frames stay the rest frames (ADR-279).
    #[test]
    fn a_parked_walker_plants_its_feet_where_the_template_rests_them() {
        let mut a = BlastRange::with_layout(config(), walker_layout()).unwrap();
        a.patrols_running = true;
        let rest: Vec<_> = a.units[0].seg_frames.iter().map(|f| f.base).collect();
        assert_eq!(a.units[0].motion.feet.len(), 4);
        for _ in 0..30 {
            a.step();
        }
        let u = &a.units[0];
        assert!(u.motion.feet.iter().all(|f| f.planted()));
        for (leg, foot) in u.body.legs.iter().zip(&u.motion.feet) {
            let c = foot.contact.unwrap();
            let ankle = u.pose.world([
                Fx::from_int(i32::from(leg.ankle.0)),
                Fx::from_int(i32::from(leg.ankle.1)),
                Fx::from_int(i32::from(leg.ankle.2)),
            ]);
            assert!(
                near_v3(c.pos, ankle, Fx::from_ratio(1, 2)),
                "{c:?} vs {ankle:?}"
            );
            assert_eq!(c.normal, phys::V3::UP);
        }
        for (f, r) in u.seg_frames.iter().zip(&rest) {
            assert!(
                near_v3(f.base, *r, Fx::from_ratio(1, 2)),
                "{:?} vs {r:?}",
                f.base
            );
        }
    }
    /// A walking walker lifts its feet, and a point of a lifted shin maps
    /// to the shin through the stepped frames but not through the rest
    /// frames (ADR-279).
    #[test]
    fn feet_follow_the_walk_and_a_hit_finds_a_lifted_shin_where_it_is() {
        let mut a = walking_walker();
        let mut found = None;
        for _ in 0..600 {
            a.step();
            let u = &a.units[0];
            for (i, leg) in u.body.legs.iter().enumerate() {
                let Some(c) = u.motion.feet[i].contact else {
                    continue;
                };
                let floor = Fx::from_int(motion::floor(
                    &a.world,
                    c.pos.x.to_int_trunc(),
                    c.pos.z.to_int_trunc(),
                ));
                let rest = Fx::from_int(i32::from(leg.ankle.1));
                if c.pos.y.sub(floor).sub(rest) > Fx::from_int(2) {
                    found = Some(i);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let leg = found.expect("no foot lifted in 600 ticks");
        let u = &a.units[0];
        assert!(!u.motion.feet[leg].planted());
        let shin = u.body.legs[leg].shin as usize;
        let b = &u.body.segments[shin].body;
        let cell = [
            b.vol.nx as usize / 2,
            b.vol.ny as usize / 2,
            b.vol.nz as usize / 2,
        ];
        assert!(b.vol.get(cell[0], cell[1], cell[2]));
        let w = u.seg_frames[shin]
            .world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
        let p = [w.x.to_int_trunc(), w.y.to_int_trunc(), w.z.to_int_trunc()];
        let hit = u.hit_at(p, &u.pose, &u.seg_frames);
        assert_eq!(
            hit.map(|(part, _)| part),
            Some(shin + 1),
            "{hit:?} at {p:?}"
        );
        let rest = crate::damage::segment_frames(
            &u.pose,
            &u.body,
            u.motion.drive.head,
            u.motion.drive.head,
        );
        assert_ne!(
            u.hit_at(p, &u.pose, &rest),
            hit,
            "the lifted leg is not at rest"
        );
    }
    /// Feet in mid-step survive the snapshot: the restored range keeps the
    /// same contacts and frames and walks on tick for tick (ADR-279).
    #[test]
    fn feet_state_round_trips_through_the_snapshot_mid_step() {
        let mut a = walking_walker();
        for _ in 0..150 {
            a.step();
        }
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.units[0].motion.feet, b.units[0].motion.feet);
        for (x, y) in a.units[0].seg_frames.iter().zip(&b.units[0].seg_frames) {
            assert_eq!(x.base, y.base);
            assert_eq!(x.x, y.x);
        }
        assert_eq!(a.state_hash(), b.state_hash());
        for _ in 0..90 {
            a.step();
            b.step();
        }
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.units[0].motion.feet, b.units[0].motion.feet);
        assert!(
            a.units[0].motion.pos[0] > Fx::from_int(310),
            "the walker did not walk"
        );
    }
    /// Aim points on a walker parked at the pad, for captures: each leg's
    /// hip mount and outer thigh cell in world units. Also writes
    /// `out/leg-demo.ron`, the same walker with the gun laid on its first
    /// hip, for `capture-range.ps1 -RangePath out/leg-demo.ron`.
    #[test]
    #[ignore]
    fn print_walker_aim_points() {
        let a = BlastRange::with_layout(config(), walker_layout()).unwrap();
        let u = &a.units[0];
        let hip0 = u.body.legs[0].hip;
        let mut demo = walker_layout();
        demo.target = u.world_pos([hip0.0 as usize, hip0.1 as usize, hip0.2 as usize]);
        std::fs::create_dir_all("../../out").ok();
        std::fs::write("../../out/leg-demo.ron", demo.to_ron().unwrap()).expect("demo layout");
        for (i, leg) in u.body.legs.iter().enumerate() {
            let hip = u.world_pos([leg.hip.0 as usize, leg.hip.1 as usize, leg.hip.2 as usize]);
            let thigh = leg.thigh as usize;
            let b = &u.body.segments[thigh].body;
            let cell = [
                b.vol.nx as usize - 2,
                b.vol.ny as usize / 2,
                b.vol.nz as usize / 2,
            ];
            let w = u.seg_frames[thigh]
                .world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
            eprintln!(
                "leg {i}: hip {hip:?} mounts {} thigh cell at [{}, {}, {}]",
                u.segment_mounts(thigh).len(),
                w.x.to_int_trunc(),
                w.y.to_int_trunc(),
                w.z.to_int_trunc()
            );
        }
    }
    /// Writes `out/walk-demo.ron`: the walker on a short out-and-back
    /// patrol at the pad, the target laid on the shin of a leg that is in
    /// the air, on the gun's side of the body, at the tick the bolt lands
    /// (ADR-279). The route's first heading is chosen so that such a leg
    /// exists at the impact tick, then the aim is refined until the impact
    /// maps to that leg. For `capture-range.ps1 -RangePath out/walk-demo.ron
    /// -Weapon energy -Power 500 -View target`.
    #[test]
    #[ignore]
    fn print_walking_leg_demo() {
        // `-Power 500` of the capture script.
        let mut config =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        config.energy = config.energy * 500 / 1000;
        config.material_energy_j = (u64::from(config.material_energy_j) * 500 / 1000) as u32;
        config.gas_pressure_milli = (config.gas_pressure_milli * 500 / 1000).min(10000);
        config.gas_source_radius = config.gas_source_radius * 1500 / 2000;
        let start = [300, 336];
        let mut routes = Vec::new();
        for k in 0..8 {
            let a = fx::angle::FULL / 8 * k;
            routes.push([
                start[0] + Fx::from_int(48).mul(fx::angle::cos(a)).to_int_trunc(),
                start[1] + Fx::from_int(48).mul(fx::angle::sin(a)).to_int_trunc(),
            ]);
        }
        for far in routes {
            let mut demo = walker_layout();
            {
                let c = demo.course.as_mut().unwrap();
                c.min = [150, 250];
                c.max = [500, 450];
                c.ramps.clear();
                c.patrols = vec![modding::range::Patrol {
                    unit: 0,
                    points: vec![start, far, start],
                }];
            }
            demo.target = [start[0], 22, start[1]];
            for round in 0..8 {
                let mut a = BlastRange::with_layout(config.clone(), demo.clone()).unwrap();
                let mut impact = None;
                for _ in 0..600 {
                    a.step();
                    if let Some(p) = a.impact {
                        impact = Some((a.impact_tick, p));
                        break;
                    }
                }
                let (tick, p) = impact.expect("no impact");
                let u = &a.units[0];
                let ys: Vec<Fx> = u
                    .motion
                    .feet
                    .iter()
                    .map(|f| f.contact.map_or(Fx::ZERO, |c| c.pos.y))
                    .collect();
                let lowest = ys.iter().copied().fold(Fx::from_int(1000), Fx::min);
                // A leg in the air on the gun's side: nearer the gun than
                // the body's centre, so the bolt meets it first.
                let lifted = u.motion.feet.iter().enumerate().find(|(i, f)| {
                    ys[*i] > lowest.add(Fx::ONE)
                        && f.contact
                            .is_some_and(|c| c.pos.x < u.motion.pos[0].sub(Fx::from_int(3)))
                });
                let lifted = lifted.map(|(i, _)| i);
                let l = lifted.map(|i| u.body.legs[i]);
                // The wave takes the struck cells within the impact tick, so
                // the point is tested against the pristine template in the
                // frames of that tick.
                let pristine = voxel::unit::strider();
                let struck = l.is_some_and(|l| {
                    [
                        Some(l.thigh),
                        Some(l.shin),
                        l.knee_ball,
                        l.ankle_ball,
                        l.foot,
                    ]
                    .into_iter()
                    .flatten()
                    .any(|seg| {
                        let seg = seg as usize;
                        local_in(&pristine.segments[seg].body, p, &u.seg_frames[seg])
                            .is_some_and(|c| pristine.segments[seg].body.vol.get(c[0], c[1], c[2]))
                    })
                });
                let in_root = local_in(&pristine, p, &u.pose)
                    .is_some_and(|c| pristine.vol.get(c[0], c[1], c[2]));
                let shin_world = l.map(|l| {
                    let shin = l.shin as usize;
                    let b = &u.body.segments[shin].body;
                    let cell = [
                        b.vol.nx as usize / 2,
                        b.vol.ny as usize / 2,
                        b.vol.nz as usize / 2,
                    ];
                    u.seg_frames[shin]
                        .world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))))
                });
                for _ in 0..3 {
                    a.step();
                }
                let u = &a.units[0];
                let parts = l.map_or(Vec::new(), |l| {
                    [
                        Some(l.thigh),
                        Some(l.shin),
                        l.knee_ball,
                        l.ankle_ball,
                        l.foot,
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<u8>>()
                });
                let leg_cells = u
                    .seg_removed
                    .iter()
                    .filter(|(seg, _)| parts.contains(seg))
                    .count();
                eprintln!(
                    "route {far:?} round {round}: impact tick {tick} at {p:?} feet {:?} walker {:?} lifted {lifted:?} struck {struck} in root {in_root} leg cells {leg_cells} root {} other segments {}",
                    ys.iter().map(|y| y.to_int_trunc()).collect::<Vec<_>>(),
                    u.at,
                    u.removed.len(),
                    u.seg_removed.len() - leg_cells
                );
                let Some(w) = shin_world else {
                    break;
                };
                if struck && leg_cells > 0 {
                    std::fs::create_dir_all("../../out").ok();
                    std::fs::write("../../out/walk-demo.ron", demo.to_ron().unwrap())
                        .expect("demo layout");
                    eprintln!(
                        "written: route {far:?} target {:?} impact tick {tick}",
                        demo.target
                    );
                    return;
                }
                demo.target = [w.x.to_int_trunc(), w.y.to_int_trunc(), w.z.to_int_trunc()];
            }
        }
        panic!("no route put a lifted gun-side leg under the bolt");
    }
    /// Segment frames sit each pivot on its mount, chain through parents,
    /// and turn turret-mounted parts with the traverse (ADR-278).
    #[test]
    fn segment_frames_sit_on_their_mounts_and_turret_parts_turn_with_the_aim() {
        let body = voxel::unit::tank();
        let base = phys::V3::new(Fx::from_int(100), Fx::from_int(16), Fx::from_int(100));
        let root = crate::damage::Frame::new(&body, base, 0, phys::V3::UP);
        let fx3 = |v: (i16, i16, i16)| {
            [
                Fx::from_int(i32::from(v.0)),
                Fx::from_int(i32::from(v.1)),
                Fx::from_int(i32::from(v.2)),
            ]
        };
        let pivot = |v: (u8, u8, u8)| {
            [
                Fx::from_int(i32::from(v.0)),
                Fx::from_int(i32::from(v.1)),
                Fx::from_int(i32::from(v.2)),
            ]
        };
        let straight = crate::damage::segment_frames(&root, &body, 0, 0);
        let turned = crate::damage::segment_frames(&root, &body, fx::angle::FULL / 4, 0);
        assert_eq!(straight.len(), body.segments.len());
        let mut turret_parts = 0;
        for (i, seg) in body.segments.iter().enumerate() {
            let a = &seg.attach;
            let mount = if a.parent == 0 {
                (if a.turret {
                    crate::damage::turned(&root, 0, 0)
                } else {
                    root
                })
                .world(fx3(a.at))
            } else {
                straight[a.parent as usize - 1].world(fx3(a.at))
            };
            let here = straight[i].world(pivot(a.pivot));
            let d = here.sub(mount);
            assert!(
                d.len_sq() < Fx::from_ratio(1, 1000),
                "segment {i} left its mount"
            );
            let moved = turned[i].base.sub(straight[i].base);
            if a.turret {
                turret_parts += 1;
                // A quarter turn about the root: same distance, right angle.
                let (r0, r1) = (straight[i].base.sub(base), turned[i].base.sub(base));
                assert!(
                    r0.len_sq().sub(r1.len_sq()).abs() < Fx::from_ratio(1, 100),
                    "segment {i} changed its distance to the root"
                );
                let dot = r0.x.mul(r1.x).add(r0.z.mul(r1.z));
                assert!(
                    dot.abs() < Fx::from_ratio(1, 10),
                    "segment {i} did not turn a right angle"
                );
            } else if a.parent == 0 {
                assert_eq!(
                    moved.len_sq(),
                    Fx::ZERO,
                    "hull segment {i} moved with the turret"
                );
            }
        }
        assert!(turret_parts > 0, "the tank has no turret-mounted segment");
    }
    /// A bolt at a walker's thigh lands on the thigh: segment cells go, the
    /// impact maps to that segment, and the wave is priced by its material.
    #[test]
    fn a_bolt_at_a_walker_thigh_takes_thigh_cells() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::with_layout(c, walker_layout()).unwrap();
        a.control();
        let thigh = a.units[0].body.legs[0].thigh as usize;
        let cell = {
            let b = &a.units[0].body.segments[thigh].body;
            [
                b.vol.nx as usize - 2,
                b.vol.ny as usize / 2,
                b.vol.nz as usize / 2,
            ]
        };
        assert!(a.units[0].body.segments[thigh]
            .body
            .vol
            .get(cell[0], cell[1], cell[2]));
        let world = a.units[0].seg_frames[thigh]
            .world(cell.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
        let aim = [world.x, world.y, world.z].map(|v| v.to_int_trunc());
        assert_eq!(a.units[0].hit(aim).map(|(part, _)| part), Some(thigh + 1));
        a.aim = Some(aim);
        for _ in 0..120 {
            a.step();
        }
        assert!(a.refire());
        for _ in 0..200 {
            a.step();
            if a.impact.is_some() {
                break;
            }
        }
        let at = a.impact.expect("the bolt never landed");
        let u = &a.units[0];
        assert!(
            u.seg_removed.iter().any(|(seg, _)| *seg as usize == thigh),
            "no thigh cell was taken: impact {at:?}, removed {:?}",
            u.seg_removed
        );
        assert!(u.seg_removed.iter().all(|(seg, c)| {
            let [x, y, z] = c.map(usize::from);
            !u.body.segments[*seg as usize].body.vol.get(x, y, z)
        }));
        assert!(a.matter.economy.ledger.balanced());
    }
    /// A leg whose hip mount is shot away leaves as one piece: its cells
    /// become a debris chunk with the leg's mass, the ledger balances, and
    /// the body no longer has it.
    #[test]
    fn a_leg_whose_mount_is_gone_falls_off_whole() {
        let mut a = BlastRange::with_layout(config(), walker_layout()).unwrap();
        a.control();
        let leg = a.units[0].body.legs[0];
        let thigh = leg.thigh as usize;
        let shin = leg.shin as usize;
        let mounts = a.units[0].segment_mounts(thigh).to_vec();
        assert!(!mounts.is_empty(), "the thigh has no mount");
        let leg_cells: u32 = a.units[0]
            .body
            .segments
            .iter()
            .enumerate()
            .filter(|(i, s)| *i == thigh || s.attach.parent as usize == thigh + 1 || *i == shin)
            .map(|(_, s)| s.body.solid_count())
            .sum();
        let held = a.matter.economy.ledger.held(a.matter.body_container(0));
        // Shoot the mount away by hand, with the accounting a weapon would
        // do for the cells it removes.
        for m in &mounts {
            let [x, y, z] = m.map(usize::from);
            assert!(a.units[0].body.remove(x, y, z));
            a.units[0].removed.push(*m);
        }
        a.matter.annihilate_body(0, mounts.len());
        a.units[0].refresh_damage(0);
        let u = &a.units[0];
        assert!(
            u.seg_gone[thigh] && u.seg_gone[shin],
            "the leg stayed on: {:?}",
            u.seg_gone
        );
        assert!(!u.segment_shed_pending().is_empty());
        let fragments = a.matter.debris.fragments.len();
        a.matter.shed_segments(&mut a.units);
        let u = &a.units[0];
        assert!(
            u.segment_shed_pending().is_empty(),
            "shedding did not finish"
        );
        assert_eq!(u.body.segments[thigh].body.solid_count(), 0);
        assert_eq!(u.body.segments[shin].body.solid_count(), 0);
        let shed: u32 = a.matter.debris.fragments[fragments..]
            .iter()
            .map(|f| {
                assert!(f.chunk.is_some(), "a shed segment lost its shape");
                f.count
            })
            .sum();
        assert!(shed >= leg_cells, "shed {shed} of {leg_cells} leg cells");
        assert!(a.matter.economy.ledger.balanced());
        let amount = a.matter.economy.per_voxel(3);
        assert_eq!(
            a.matter.economy.ledger.held(a.matter.body_container(0)),
            held - (u64::from(shed) + mounts.len() as u64) * amount
        );
        let b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(b.units[0].seg_gone, a.units[0].seg_gone);
    }
    /// A leg that fell off carries nothing (ADR-280): the walker stands on
    /// three feet, sinks, and leans towards the missing corner.
    #[test]
    fn a_lost_leg_carries_nothing_and_the_hull_leans_over_it() {
        let mut a = BlastRange::with_layout(config(), walker_layout()).unwrap();
        a.control();
        for _ in 0..30 {
            a.step();
        }
        let level = a.units[0].motion.hull;
        assert!(a.units[0].motion.feet.iter().all(|f| f.planted()));
        let leg = a.units[0].body.legs[0];
        let thigh = leg.thigh as usize;
        let mounts = a.units[0].segment_mounts(thigh).to_vec();
        for m in &mounts {
            let [x, y, z] = m.map(usize::from);
            assert!(a.units[0].body.remove(x, y, z));
            a.units[0].removed.push(*m);
        }
        a.matter.annihilate_body(0, mounts.len());
        a.units[0].refresh_damage(0);
        assert!(a.units[0].seg_gone[thigh]);
        assert_eq!(a.units[0].motion.legs_lost, [true, false, false, false]);
        for _ in 0..90 {
            a.step();
        }
        let u = &a.units[0];
        let hull = u.motion.hull;
        // Leg 0 hangs at the hull's `-x, -z` corner in the hull frame; with
        // the hull heading `+x` those are world axes too. The corner sinks,
        // and the hull's normal leans towards the sunk corner.
        assert_eq!(u.motion.drive.head, 0);
        assert!(
            hull.y < level.y.sub(Fx::from_ratio(1, 10)),
            "did not sink: {hull:?}"
        );
        assert!(
            hull.up.x < Fx::from_ratio(-1, 200) && hull.up.z < Fx::from_ratio(-1, 200),
            "did not lean over the lost leg: {:?}",
            hull.up
        );
        assert!(hull.load > Fx::ZERO);
        let b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(b.units[0].motion.legs_lost, a.units[0].motion.legs_lost);
    }
    /// Severing a thin slice breaks a leg even when most of its metal
    /// remains. It leaves the skeleton and becomes independent debris.
    #[test]
    fn a_leg_cut_through_its_shin_detaches_though_most_of_it_stands() {
        let mut a = BlastRange::with_layout(config(), walker_layout()).unwrap();
        a.control();
        for _ in 0..30 {
            a.step();
        }
        let leg = a.units[0].body.legs[1];
        let shin = leg.shin as usize;
        let (nx, ny, nz) = {
            let b = &a.units[0].body.segments[shin].body;
            (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize)
        };
        let cut = nx / 2;
        let mut taken = 0;
        for z in 0..nz {
            for y in 0..ny {
                if a.units[0].body.segments[shin].body.remove(cut, y, z) {
                    a.units[0]
                        .seg_removed
                        .push((shin as u8, [cut as u8, y as u8, z as u8]));
                    taken += 1;
                }
            }
        }
        assert!(taken > 0 && taken < 60, "the cut took {taken} cells");
        a.matter.annihilate_body(0, taken);
        a.units[0].refresh_damage(0);
        let u = &a.units[0];
        assert!(u.seg_gone[shin], "severed shin stayed on the skeleton");
        assert_eq!(u.motion.legs_lost, [false, true, false, false]);
        assert!(u.status.alive);
    }
    /// A dead walker folds every leg and comes down on them (ADR-304): a
    /// treble bolt into its core kills it, and within three seconds its
    /// hull sits well below where it stood.
    #[test]
    fn a_dead_walker_folds_its_legs_and_comes_down() {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        c.energy *= 3;
        // No `control()`: it would silence the gun of a range nobody plays.
        let mut a = BlastRange::with_layout(c, walker_layout()).unwrap();
        let core = {
            let u = &a.units[0];
            let (x, y, z) = u.body.core;
            let half = usize::from(u.body.core_size) / 2;
            u.world_pos([
                usize::from(x) + half,
                usize::from(y) + half,
                usize::from(z) + half,
            ])
        };
        a.aim = Some(core);
        for _ in 0..30 {
            a.step();
        }
        let level = a.units[0].motion.hull.y;
        let floor = a.units[0].motion.pos[1];
        let mut dead = None;
        for _ in 0..400 {
            a.step();
            if !a.units[0].status.alive {
                dead = Some(a.tick);
                break;
            }
        }
        dead.expect("the bolt did not kill the walker");
        assert!(
            a.units[0].motion.legs_buckled.iter().all(|b| *b),
            "a dead walker's legs did not all fold: {:?}",
            a.units[0].motion.legs_buckled
        );
        for _ in 0..90 {
            a.step();
        }
        let u = &a.units[0];
        let drop = level.sub(u.motion.hull.y);
        assert!(
            drop > level.sub(floor).mul(Fx::from_ratio(2, 5)),
            "the hull did not come down: stood {level:?} over {floor:?}, now {:?}",
            u.motion.hull.y
        );
    }
    /// A leg whose thigh and shin lost more than half their cells buckles
    /// (ADR-281, ADR-288): it folds on its foot, carries nothing, the
    /// walker sinks and leans over it, and the snapshot keeps the fold.
    #[test]
    fn a_leg_that_lost_more_than_half_its_cells_buckles() {
        let mut a = BlastRange::with_layout(config(), walker_layout()).unwrap();
        a.control();
        for _ in 0..30 {
            a.step();
        }
        let level = a.units[0].motion.hull;
        let leg = a.units[0].body.legs[0];
        let thigh = leg.thigh as usize;
        let shin = leg.shin as usize;
        // Thin but continuous beams still join each link's endpoints.
        let mut taken = 0;
        for seg in [thigh, shin] {
            let cells: Vec<[usize; 3]> = {
                let b = &a.units[0].body.segments[seg].body;
                let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
                let mut cells = Vec::new();
                for z in 0..nz {
                    for y in 0..ny {
                        for x in 0..nx {
                            if b.vol.get(x, y, z) {
                                cells.push([x, y, z]);
                            }
                        }
                    }
                }
                cells
            };
            for c in &cells {
                if c[1] >= 2 || c[2] >= 2 {
                    assert!(a.units[0].body.segments[seg].body.remove(c[0], c[1], c[2]));
                    a.units[0].seg_removed.push((seg as u8, c.map(|v| v as u8)));
                    taken += 1;
                }
            }
        }
        a.matter.annihilate_body(0, taken);
        a.units[0].refresh_damage(0);
        let u = &a.units[0];
        assert!(!u.seg_gone[thigh], "the leg fell off instead of buckling");
        assert_eq!(u.motion.legs_buckled, [true, false, false, false]);
        assert_eq!(u.motion.legs_lost, [false; 4]);
        for _ in 0..90 {
            a.step();
        }
        let u = &a.units[0];
        let hull = u.motion.hull;
        assert!(
            hull.y < level.y.sub(Fx::from_ratio(1, 10)),
            "did not sink: {hull:?}"
        );
        assert!(
            hull.up.x < Fx::from_ratio(-1, 200) && hull.up.z < Fx::from_ratio(-1, 200),
            "did not lean over the buckled leg: {:?}",
            hull.up
        );
        let c = u.motion.feet[0].contact.unwrap();
        // Folded on its foot: on the ground, drawn in under the hip; both
        // measured from the hip's projection at ankle height.
        let under = u.pose.world([
            Fx::from_int(i32::from(leg.hip.0)),
            Fx::from_int(i32::from(leg.ankle.1)),
            Fx::from_int(i32::from(leg.hip.2)),
        ]);
        let rest = u.pose.world([
            Fx::from_int(i32::from(leg.ankle.0)),
            Fx::from_int(i32::from(leg.ankle.1)),
            Fx::from_int(i32::from(leg.ankle.2)),
        ]);
        let on_ground = Fx::from_int(FLOOR + i32::from(leg.ankle.1));
        assert!(
            (c.pos.y.sub(on_ground)).abs() < Fx::from_ratio(1, 2),
            "foot off the ground: {c:?}"
        );
        let flat =
            |p: phys::V3| phys::V3::new(p.x.sub(under.x), Fx::ZERO, p.z.sub(under.z)).len_sq();
        assert!(
            flat(c.pos) < flat(rest).div(Fx::from_int(4)),
            "not drawn in: {c:?}"
        );
        // The folded leg's frames follow the fold: the shin's base, the knee,
        // left its rest place.
        let rest = crate::damage::segment_frames(
            &u.pose,
            &u.body,
            u.motion.drive.head,
            u.motion.drive.head,
        );
        let moved = u.seg_frames[shin].base.sub(rest[shin].base).len_sq().sqrt();
        assert!(moved > Fx::ONE, "the shin stayed at rest: moved {moved:?}");
        let b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(
            b.units[0].motion.legs_buckled,
            a.units[0].motion.legs_buckled
        );
        assert!(a.matter.economy.ledger.balanced());
    }
    /// A shell's debris comes in pieces up to three cells a side, a bolt's
    /// within two (ADR-284): the weapon names the box.
    #[test]
    fn a_shell_throws_bigger_pieces_than_a_bolt() {
        let mut shell = BlastRange::new(config());
        assert_eq!(shell.config.fragment_box, 3);
        for _ in 0..400 {
            shell.step();
        }
        let pieces = &shell.matter.debris.fragments;
        assert!(pieces.iter().any(|f| f.side == 3), "no three-cell pieces");
        assert!(
            pieces.iter().any(|f| f.side == 3 && f.count > 8),
            "no piece bigger than a two-cell box could hold"
        );
        assert!(pieces.iter().all(|f| f.chunk.is_some()
            || f.side == 3
            || f.material == 3
            || (f.resting && f.side == 4)));
        let mut bolt = BlastRange::new(
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        );
        assert_eq!(bolt.config.fragment_box, 2);
        for _ in 0..400 {
            bolt.step();
        }
        assert!(bolt
            .matter
            .debris
            .fragments
            .iter()
            .all(|f| f.side == 2 || (f.resting && f.side == 4)));
        let b = BlastRange::restore(shell.config.clone(), &shell.snapshot()).unwrap();
        assert_eq!(shell.state_hash(), b.state_hash());
    }
    /// The annihilation bolt (ADR-285) takes matter whatever its strength
    /// and leaves no debris: everything it removes goes to the annihilation
    /// sink, and the ledger still balances.
    #[test]
    fn the_annihilation_bolt_leaves_no_debris_and_sinks_what_it_takes() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/anni-cannon.ron")).unwrap();
        assert_eq!(c.material_energy_j, 0);
        assert_eq!(c.heat_units, 0);
        assert_eq!(c.soil_resistance, c.steel_resistance);
        let mut a = BlastRange::new(c);
        for _ in 0..300 {
            a.step();
        }
        assert!(a.impact.is_some(), "the bolt never landed");
        assert!(a.removed > 0, "the bolt took nothing");
        assert!(
            a.matter
                .economy
                .ledger
                .sunk(crate::matter::Sink::Annihilation)
                > 0
        );
        assert!(
            a.matter.debris.fragments.iter().all(|f| f.chunk.is_some()),
            "an annihilating bolt threw loose pieces"
        );
        assert!(a.matter.economy.ledger.balanced());
    }
    /// A shell landing beside a parked tank on the range shoves it, hops
    /// it, and the snapshot mid-shove restores tick for tick (ADR-286).
    #[test]
    fn a_shell_beside_a_target_shoves_it_and_the_snapshot_keeps_the_shove() {
        let mut layout = walker_layout();
        layout.units = vec![modding::range::RangeUnitDef {
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [300, FLOOR, 336],
        }];
        layout.target = [286, 20, 336];
        let mut a = BlastRange::with_layout(config(), layout).unwrap();
        assert!(a.config.hull_impulse_milli > 0, "the shell must push");
        let start = a.units[0].motion.pos;
        let mut impact = None;
        for _ in 0..300 {
            a.step();
            if a.impact.is_some() {
                impact = Some(a.tick);
                break;
            }
        }
        impact.expect("no impact");
        // The push arrives with the wave, a few ticks after the impact,
        // and only once the wave has pressed on the body (ADR-297).
        let mut pushed = false;
        for _ in 0..8 {
            a.step();
            pushed |= a.units[0].motion.shove.len_sq() > Fx::ZERO;
        }
        assert!(pushed, "no shove from the wave");
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.units[0].motion.shove, b.units[0].motion.shove);
        let level = a.units[0].motion.hull.y;
        let mut hopped = Fx::ZERO;
        for _ in 0..90 {
            a.step();
            b.step();
            hopped = hopped.max(a.units[0].motion.hull.y.sub(level));
        }
        assert_eq!(a.state_hash(), b.state_hash());
        let u = &a.units[0];
        assert!(
            u.motion.pos[0].sub(start[0]) > Fx::ONE,
            "the tank did not move: {:?} from {start:?}",
            u.motion.pos
        );
        assert!(
            hopped > Fx::from_ratio(1, 10),
            "the tank did not hop: {hopped:?}"
        );
        assert_eq!(u.motion.shove, phys::V3::ZERO);
    }
    #[test]
    fn a_strong_shell_under_the_tanks_nose_pushes_it_out_and_it_lands() {
        let mut layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        layout.units = vec![modding::range::RangeUnitDef {
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [359, FLOOR, 312],
        }];
        layout.target = [329, FLOOR + 4, 312];
        layout.blocks.clear();
        layout.airburst_on_target = false;
        if let Some(course) = &mut layout.course {
            course.patrols.clear();
        }
        let mut charge = config();
        charge.energy *= 4;
        let mut a = BlastRange::with_layout(charge, layout).unwrap();
        for _ in 0..300 {
            a.step();
            if a.impact.is_some() {
                break;
            }
        }
        assert!(a.impact.is_some());
        let start = a.units[0].motion.pos;
        let level = a.units[0].motion.hull.y;
        let mut rise = Fx::ZERO;
        for _ in 0..150 {
            a.step();
            rise = rise.max(a.units[0].motion.hull.y.sub(level));
        }
        let u = &a.units[0];
        assert!(u.status.alive);
        assert!(u.motion.pos[0].sub(start[0]) > Fx::from_int(10));
        assert!(
            rise > Fx::from_ratio(1, 2) && rise < Fx::from_int(3),
            "rise {rise:?}"
        );
        assert!(u.motion.hull.y.sub(level).abs() < Fx::from_ratio(1, 5));
        assert_eq!(u.motion.shove, phys::V3::ZERO);
    }
    /// Steel heated to its melting point runs off as a piece of steel
    /// (ADR-290): the cell is gone, the debris holds its matter, the ledger
    /// balances, and the count survives the snapshot; soil never melts.
    #[test]
    fn heated_steel_melts_into_debris_and_soil_does_not() {
        let mut a = BlastRange::new(config());
        let steel = a.matter.economy.mats.get(3).expect("range steel");
        assert!(steel.melt > 0 && steel.kindling == 0);
        assert_eq!(a.matter.economy.mats.get(1).map(|d| d.melt), Some(0));
        // A steel cell of the target wall and a soil cell of the pad.
        let mut wall = None;
        for x in (TARGET[0] - 30)..(TARGET[0] + 30) {
            for y in FLOOR..(FLOOR + 30) {
                for z in (TARGET[2] - 30)..(TARGET[2] + 30) {
                    if material(&a.world, [x, y, z]) == 3 {
                        wall = Some([x as u32, y as u32, z as u32]);
                    }
                }
            }
        }
        let wall = wall.expect("no steel near the target");
        let mut soil = None;
        for x in (TARGET[0] - 120)..(TARGET[0] - 40) {
            for y in (FLOOR - 6)..FLOOR {
                for z in (TARGET[2] - 30)..(TARGET[2] + 30) {
                    if material(&a.world, [x, y, z]) == 1 {
                        soil = Some([x as u32, y as u32, z as u32]);
                    }
                }
            }
        }
        let soil = soil.expect("no soil near the target");
        a.heat.add(&a.world, wall, steel.melt);
        a.heat.add(&a.world, soil, 100_000);
        let pieces = a.matter.debris.fragments.len();
        for _ in 0..4 {
            a.step();
        }
        assert_eq!(
            material(&a.world, wall.map(|v| v as i32)),
            0,
            "the steel stood"
        );
        assert_eq!(
            material(&a.world, soil.map(|v| v as i32)),
            1,
            "the soil melted"
        );
        assert_eq!(a.heat.melted_total, 1);
        // The ground is not enumerated: it reveals the cell's matter and
        // hands it to the piece, which holds exactly one voxel of steel.
        let mut steel_pieces = 0;
        let mut steel_matter = 0;
        for f in &a.matter.debris.fragments[pieces..] {
            if f.material == 3 && f.chunk.is_none() {
                steel_pieces += 1;
                steel_matter += a.matter.economy.ledger.held(f.held);
            }
        }
        assert_eq!(steel_pieces, 1, "steel pieces");
        assert_eq!(steel_matter, a.matter.economy.per_voxel(3));
        assert!(a.matter.economy.ledger.balanced());
        let b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(b.heat.melted_total, 1);
    }
    /// A heap of resting debris under one belt is floor (ADR-292): the
    /// driven tank rides over it with its hull raised where a flat drive
    /// stays level, and still arrives.
    #[test]
    fn a_tank_rides_up_over_a_heap_of_debris() {
        fn drive(heap: bool) -> (Fx, bool) {
            let mut a = BlastRange::new(config());
            a.patrols_running = false;
            let goal = [GUN[0] + 60, GUN[2]];
            if heap {
                let belt = a.gun.motion.shape(&a.gun.body).support[1];
                let cells = 32;
                let held = a
                    .matter
                    .economy
                    .ledger
                    .create(crate::matter::Kind::Debris, 0, u64::MAX);
                let amount = a.matter.economy.per_voxel(2) * cells;
                let ground = a.matter.economy.ground;
                a.matter.economy.ledger.reveal(ground, amount);
                a.matter.economy.ledger.transfer(ground, held, amount);
                let mut f = crate::burst::Fragment {
                    id: 9000,
                    position: phys::V3::new(
                        Fx::from_int(GUN[0] + 30),
                        Fx::from_int(FLOOR),
                        Fx::from_int(GUN[2] + belt),
                    ),
                    velocity: phys::V3::ZERO,
                    material: 2,
                    paint: None,
                    mm: 100,
                    basis: [phys::V3::ZERO; 3],
                    mass_g: 2400 * cells,
                    count: cells as u32,
                    shape: 0,
                    side: 4,
                    chunk: None,
                    held,
                    resting: true,
                };
                crate::burst::heap(&mut f, &a.world);
                a.matter.debris.fragments.push(f);
                a.rubble = a
                    .matter
                    .debris
                    .rubble(&a.world, &a.navigation.fell.materials, &[]);
                assert_eq!(
                    a.rubble.get(&[GUN[0] + 30, GUN[2] + belt]),
                    Some(&(FLOOR as u16 + 2))
                );
            }
            let rubble = a.rubble.clone();
            assert!(a
                .gun
                .motion
                .move_to(&a.world, goal, &a.gun.body, &[], &rubble));
            let mut top = a.gun.motion.hull.y;
            for _ in 0..600 {
                a.step();
                top = top.max(a.gun.motion.hull.y);
                if a.gun.motion.destination.is_none() {
                    break;
                }
            }
            (top, a.gun.motion.destination.is_none())
        }
        let (flat, arrived) = drive(false);
        assert!(arrived, "the flat drive did not arrive");
        let (over, arrived) = drive(true);
        assert!(arrived, "the tank did not get past the heap");
        assert!(
            over.sub(flat) > Fx::from_ratio(1, 2),
            "hull top over the heap {over:?}, flat {flat:?}"
        );
    }
    /// A piece falling onto the parked tank rests on its roof (ADR-293),
    /// and when the tank drives off it falls to the ground (ADR-291).
    #[test]
    fn a_piece_falling_onto_the_tank_rests_on_its_roof_and_falls_when_it_drives_off() {
        let mut a = BlastRange::new(config());
        a.patrols_running = false;
        let height = Fx::from_ratio(
            a.gun.body.vol.ny as i32 * a.gun.body.mm as i32,
            voxel::unit::WORLD_MM as i32,
        );
        let held = a
            .matter
            .economy
            .ledger
            .create(crate::matter::Kind::Debris, 0, u64::MAX);
        let amount = a.matter.economy.per_voxel(2);
        let ground = a.matter.economy.ground;
        a.matter.economy.ledger.reveal(ground, amount);
        a.matter.economy.ledger.transfer(ground, held, amount);
        let pos = a.gun.motion.pos;
        let roof = pos[1].add(height);
        a.matter.debris.fragments.push(crate::burst::Fragment {
            id: 9001,
            position: phys::V3::new(pos[0], roof.add(Fx::from_int(6)), pos[2]),
            velocity: phys::V3::ZERO,
            material: 2,
            paint: None,
            mm: 100,
            basis: crate::burst::IDENTITY,
            mass_g: 2400,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: false,
        });
        let piece = |a: &BlastRange| {
            let f = a
                .matter
                .debris
                .fragments
                .iter()
                .find(|f| f.id == 9001)
                .expect("the piece is gone");
            (f.resting, f.position.y)
        };
        let mut rested = None;
        for _ in 0..300 {
            a.step();
            let (resting, y) = piece(&a);
            if resting {
                rested = Some(y);
                break;
            }
        }
        let y = rested.expect("the piece never rested");
        assert!(
            y > roof && y < roof.add(Fx::from_int(2)),
            "not on the roof: {y:?}, roof {roof:?}"
        );
        assert!(a.order_move([GUN[0] + 60, GUN[2]]));
        let mut low = y;
        for _ in 0..600 {
            a.step();
            low = low.min(piece(&a).1);
            if a.gun.motion.destination.is_none() {
                break;
            }
        }
        assert!(
            low < roof.sub(Fx::from_int(2)),
            "the piece never fell: {low:?}"
        );
    }
    /// A unit's box holds every cell the wave can hit, on a driving unit
    /// with its pose and on a parked one, segments included (ADR-296).
    #[test]
    fn a_units_box_holds_every_cell_the_wave_can_hit() {
        let a = BlastRange::new(config());
        for u in a.units.iter().chain(std::iter::once(&a.gun)) {
            let (lo, hi) = u.bounds();
            let mut hits = 0;
            for x in (lo[0] - 3)..=(hi[0] + 3) {
                for y in (lo[1] - 3)..=(hi[1] + 3) {
                    for z in (lo[2] - 3)..=(hi[2] + 3) {
                        if u.solid([x, y, z]) {
                            hits += 1;
                            assert!(
                                x >= lo[0]
                                    && x <= hi[0]
                                    && y >= lo[1]
                                    && y <= hi[1]
                                    && z >= lo[2]
                                    && z <= hi[2],
                                "cell {:?} outside the box {lo:?}..{hi:?}",
                                [x, y, z]
                            );
                        }
                    }
                }
            }
            assert!(hits > 0, "no cells found in the box");
        }
    }
    #[test]
    fn blast_bounds_hold_frozen_hits_after_the_vehicle_moves_away() {
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let mut unit = RangeUnit::of_kind(kind, [200, 24, 200]);
            for yaw in [0, fx::angle::FULL / 7] {
                unit.motion.drive.head = yaw;
                unit.update_pose(yaw);
                let pose = unit.pose;
                let segments = unit.seg_frames.clone();
                let (lo, hi) = unit.bounds_for(Some(&pose), &segments);
                unit.motion.pos[0] = unit.motion.pos[0].add(Fx::from_int(300));
                unit.update_pose(yaw);
                assert_ne!(
                    unit.bounds_for(Some(&unit.pose), &unit.seg_frames),
                    (lo, hi)
                );
                let mut hits = 0;
                for z in lo[2] - 2..=hi[2] + 2 {
                    for y in lo[1] - 2..=hi[1] + 2 {
                        for x in lo[0] - 2..=hi[0] + 2 {
                            let q = [x, y, z];
                            if unit.hit_at(q, &pose, &segments).is_some() {
                                hits += 1;
                                assert!((0..3).all(|a| q[a] >= lo[a] && q[a] <= hi[a]));
                            }
                        }
                    }
                }
                assert!(hits > 100, "the frozen pose must contain a real body");
            }
        }
    }
    #[test]
    fn batched_hit_frames_preserve_live_damage_in_a_frozen_turned_pose() {
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let mut u = RangeUnit::of_kind(kind, [200, 24, 200]);
            u.motion.drive.head = fx::angle::FULL / 7;
            u.motion.hull.up = phys::V3::new(Fx::from_ratio(1, 7), Fx::ONE, Fx::from_ratio(-1, 5));
            u.update_pose(-fx::angle::FULL / 9);
            let pose = u.pose;
            let segments = u.seg_frames.clone();
            let delta = u.turret_delta;
            let batch = HitFrames::new(&u, pose, &segments, delta);
            let (lo, hi) = u.bounds();
            for damaged in [false, true] {
                if damaged {
                    u.motion.pos[0] = u.motion.pos[0].add(Fx::from_int(300));
                    u.update_pose(fx::angle::FULL / 3);
                    for (i, col) in u.body.vol.cols.iter_mut().enumerate() {
                        if i % 3 == 0 {
                            *col = 0;
                        }
                    }
                    u.seg_gone[0] = true;
                }
                let mut hits = 0;
                for z in lo[2] - 1..=hi[2] + 1 {
                    for y in lo[1] - 1..=hi[1] + 1 {
                        for x in lo[0] - 1..=hi[0] + 1 {
                            let p = [x, y, z];
                            let reference = u.hit_with_turret(p, &pose, &segments, delta);
                            assert_eq!(batch.hit(&u, p), reference, "at {p:?}, damaged={damaged}");
                            hits += usize::from(reference.is_some());
                        }
                    }
                }
                assert!(hits > 100);
            }
        }
    }
    /// A block of four by four by four placed in the air, with a layer of
    /// `cap` on top when that is not air, applied by the range's own
    /// flush; the collapse layer sees those edits the same tick. Returns
    /// the range, the block's cells and the piece count before the block
    /// went in.
    fn a_block_in_the_air(mat: u16, cap: u16) -> (BlastRange, Vec<[i32; 3]>, usize) {
        let mut a = BlastRange::new(config());
        a.patrols_running = false;
        a.fire_tick = u32::MAX;
        let base = [GUN[0] - 40, FLOOR + 8, GUN[2] + 20];
        let mut cells = Vec::new();
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    let p = [base[0] + x, base[1] + y, base[2] + z];
                    a.world.queue_edit(voxel::world::Edit::put(
                        p[0] as u32,
                        p[1] as u32,
                        p[2] as u32,
                        mat,
                    ));
                    cells.push(p);
                }
            }
        }
        if cap != 0 {
            for x in 0..4 {
                for z in 0..4 {
                    a.world.queue_edit(voxel::world::Edit::put(
                        (base[0] + x) as u32,
                        (base[1] + 4) as u32,
                        (base[2] + z) as u32,
                        cap,
                    ));
                }
            }
        }
        let pieces_before = a.matter.debris.fragments.len();
        a.step();
        assert!(cells.iter().all(|p| material(&a.world, *p) == mat));
        (a, cells, pieces_before)
    }
    /// A block of concrete under a layer of soil left in the air falls
    /// whole (ADR-300): the collapse layer tears the lot off as one body,
    /// the soil crumbles off it into pieces, the concrete falls on as the
    /// body, the snapshot mid-fall carries it, and it bakes back on the
    /// ground with every cell, no piece of concrete made and only the
    /// soil counted as removed.
    #[test]
    fn an_unsupported_concrete_block_falls_whole_and_lands() {
        let (mut a, cells, pieces_before) = a_block_in_the_air(2, 1);
        assert_eq!(a.bodies.list.len(), 1, "no body torn loose");
        assert_eq!(
            a.bodies.list[0].voxels.len(),
            64,
            "the soil fell with the body"
        );
        assert_eq!(a.collapsed_cells, 80);
        assert_eq!(a.removed, 16, "the soil cap was not counted");
        let soil: u64 = a.matter.debris.fragments[pieces_before..]
            .iter()
            .filter(|f| f.material == 1)
            .map(|f| u64::from(f.count))
            .sum();
        assert_eq!(soil, 16, "the soil cap did not crumble into pieces");
        for _ in 0..3 {
            a.step();
        }
        assert!(
            cells.iter().all(|p| material(&a.world, *p) == 0),
            "the block stood"
        );
        assert_eq!(a.bodies.list.len(), 1, "the body landed at once");
        let b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(b.bodies.list.len(), 1);
        assert_eq!(b.collapsed_cells, 80);
        let mut landed = None;
        for _ in 0..200 {
            a.step();
            if a.bodies.list.is_empty() {
                landed = Some(a.tick);
                break;
            }
        }
        landed.expect("the body never landed");
        a.step();
        let mut back = 0;
        let mut top = 0;
        for x in 0..4 {
            for z in 0..4 {
                for y in 0..(FLOOR + 8) {
                    if material(&a.world, [GUN[0] - 40 + x, y, GUN[2] + 20 + z]) == 2 {
                        back += 1;
                        top = top.max(y);
                    }
                }
            }
        }
        assert_eq!(back, 64, "the body did not bake back whole");
        assert!(top < FLOOR + 8, "the body did not come down: top {top}");
        assert_eq!(
            a.matter.debris.fragments[pieces_before..]
                .iter()
                .filter(|f| f.material == 2)
                .count(),
            0,
            "concrete crumbled"
        );
        assert_eq!(a.collapsed_cells, 80);
        assert_eq!(a.removed, 16, "a body that came back counted as removed");
        assert!(a.matter.economy.ledger.balanced());
    }
    /// A block of soil left in the air crumbles (ADR-299): the layer digs
    /// it out, the range turns it into pieces of the same matter, they
    /// settle on the ground, the ledger balances, and the snapshot
    /// carries the support graph and the count.
    #[test]
    fn an_unsupported_soil_block_crumbles_into_pieces_of_its_own_matter() {
        let (mut a, cells, pieces_before) = a_block_in_the_air(1, 0);
        assert!(a.bodies.list.is_empty(), "soil fell as a body");
        let mut gone = None;
        for _ in 0..30 {
            a.step();
            if cells.iter().all(|p| material(&a.world, *p) == 0) {
                gone = Some(a.tick);
                break;
            }
        }
        gone.expect("the block never fell");
        assert_eq!(a.collapsed_cells, 64);
        assert_eq!(a.removed, 64);
        let held: u64 = a.matter.debris.fragments[pieces_before..]
            .iter()
            .filter(|f| f.material == 1)
            .map(|f| u64::from(f.count))
            .sum();
        assert_eq!(held, 64, "the pieces do not hold the block's cells");
        assert!(a.matter.economy.ledger.balanced());
        let b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(b.collapsed_cells, 64);
        for _ in 0..300 {
            a.step();
        }
        let soil: Vec<_> = a
            .matter
            .debris
            .fragments
            .iter()
            .filter(|f| f.material == 1 && f.chunk.is_none())
            .collect();
        assert!(
            !soil.is_empty() && soil.iter().all(|f| f.resting),
            "pieces still in the air"
        );
        assert!(
            soil.iter().all(|f| f.position.y < Fx::from_int(FLOOR + 8)),
            "the pieces did not come down"
        );
    }
    /// An order onto a parked unit or into the wall is served next to it
    /// at once (P-17, ADR-301): the goal moves to the nearest open cell
    /// instead of the search running the map dry.
    #[test]
    fn an_order_onto_a_parked_unit_or_into_the_wall_is_served_next_to_it() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c, layout).unwrap();
        assert!(a.order_move([340, 336]), "no route next to the parked tank");
        while a.gun.motion.searching() {
            a.step();
        }
        assert!(
            a.gun.motion.last_plan[0] < 2000,
            "the order cost {} nodes",
            a.gun.motion.last_plan[0]
        );
        assert!(a.order_move([400, 336]), "no route to the wall's foot");
        while a.gun.motion.searching() {
            a.step();
        }
        let end = *a.gun.motion.path.last().unwrap();
        assert!(end[0] < 392, "the route ends inside the wall: {end:?}");
    }
    /// A route too long for one tick is found over several (ADR-306): the
    /// order stands, the hull waits where it is, and a snapshot taken
    /// while the search walks restores into the same search — the restored
    /// range answers the same route on the same tick.
    #[test]
    fn a_long_route_is_searched_over_ticks_and_the_snapshot_keeps_the_search() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c, layout).unwrap();
        a.patrols_running = false;
        a.fire_tick = u32::MAX;
        let start = a.gun.motion.pos;
        assert!(a.order_move([128, 470]), "the order was refused");
        assert!(
            a.gun.motion.searching(),
            "the woods were found in one tick: {:?}",
            a.gun.motion.last_plan
        );
        assert!(a.gun.motion.path.is_empty());
        // Mid-search: a restored range carries the same frontier.
        // The legal direct forest path now needs only two slices. Save
        // immediately after its bounded first slice, before it finishes.
        assert!(a.gun.motion.searching(), "the search ended too early");
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        let mut answered = None;
        for i in 0..200 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash(), "diverged at slice {i}");
            if answered.is_none() && !a.gun.motion.searching() {
                answered = Some(i);
            }
        }
        let at = answered.expect("the search never ended");
        assert!(at > 0, "the search did not span ticks");
        assert!(!a.gun.motion.path.is_empty(), "no route to the woods");
        assert_eq!(a.gun.motion.path, b.gun.motion.path);
        assert!(
            a.gun.motion.pos[0].sub(start[0]).abs() > Fx::ONE
                || a.gun.motion.pos[2].sub(start[2]).abs() > Fx::ONE,
            "the hull never left: {:?}",
            a.gun.motion.pos
        );
    }
    /// Segment damage is state: a restored range matches tick for tick.
    #[test]
    fn segment_damage_is_saved_and_hashed() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::with_layout(c, walker_layout()).unwrap();
        a.control();
        let thigh = a.units[0].body.legs[0].thigh as usize;
        let world = a.units[0].seg_frames[thigh].world([
            Fx::from_ratio(21, 2),
            Fx::from_ratio(9, 2),
            Fx::from_ratio(9, 2),
        ]);
        a.aim = Some([world.x, world.y, world.z].map(|v| v.to_int_trunc()));
        for _ in 0..120 {
            a.step();
        }
        assert!(a.refire());
        for _ in 0..200 {
            a.step();
            if !a.units[0].seg_removed.is_empty() {
                break;
            }
        }
        assert!(
            !a.units[0].seg_removed.is_empty(),
            "no segment damage to save"
        );
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.units[0].seg_removed, b.units[0].seg_removed);
        for _ in 0..30 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
    }
    /// The first upper-crown foliage cell of the south-western woods, in
    /// cell order: the round from the gun crosses only the open lane to it.
    fn a_crown(a: &BlastRange) -> [i32; 3] {
        for z in 440..520u32 {
            for x in 80..180u32 {
                for y in (36..60u32).rev() {
                    if a.world.material(x, y, z) == 4 {
                        return [x as i32, y as i32, z as i32];
                    }
                }
            }
        }
        panic!("no crown in the far woods")
    }
    /// A burst in a crown lights it: the fire spreads through the tree,
    /// consumes it into the combustion sink with the ledger balanced, and
    /// dies out; soil and rock never burn (ADR-277).
    #[test]
    fn a_burst_in_a_crown_lights_the_tree_and_the_fire_burns_out_into_the_sink() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.airburst_on_target = false;
        let mut a = BlastRange::with_layout(config(), layout).unwrap();
        a.control();
        a.patrols_running = false;
        a.aim = Some(a_crown(&a));
        // Let the turret traverse to the crown before the round is charged.
        for _ in 0..120 {
            a.step();
        }
        assert!(a.refire());
        let mut lit = None;
        let mut peak = 0;
        let mut out = None;
        for tick in 0..3000 {
            a.step();
            let burning = a.heat.burning();
            peak = peak.max(burning);
            if burning > 0 && lit.is_none() {
                lit = Some(tick);
            }
            assert!(a.matter.economy.ledger.balanced(), "ledger at {tick}");
            if lit.is_some() && burning == 0 && a.heat.hot() == 0 {
                out = Some(tick);
                break;
            }
        }
        let sunk = a
            .matter
            .economy
            .ledger
            .sunk(crate::matter::Sink::Combustion);
        eprintln!(
            "lit {lit:?} out {out:?} peak {peak} burned {} foliage {} trunk {} sunk {sunk}",
            a.heat.burned_total, a.heat.burned_by_material[4], a.heat.burned_by_material[5]
        );
        let (lit, out) = (
            lit.expect("the burst lit nothing"),
            out.expect("the fire never died"),
        );
        // Longer than one burn: the fire moved through crowns the burst
        // never reached, and consumed far more than it lit.
        assert!(out - lit >= 120, "the fire only flashed: {lit}..{out}");
        assert!(peak >= 20 && a.heat.burned_total >= 200, "peak {peak}");
        assert!(sunk > 0);
        let wood = a.heat.burned_by_material[4] + a.heat.burned_by_material[5];
        assert_eq!(
            wood, a.heat.burned_total,
            "something other than wood burned"
        );
    }
    /// A bolt into bare ground lights nothing and leaves no hot cell:
    /// heat is kept on fuel alone.
    #[test]
    fn a_bolt_into_bare_ground_leaves_no_heat_and_no_fire() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::new(c);
        a.control();
        a.aim = Some([300, FLOOR, 336]);
        assert!(a.refire());
        for _ in 0..400 {
            a.step();
            assert_eq!(
                (a.heat.hot(), a.heat.burning()),
                (0, 0),
                "bare ground burned"
            );
        }
        assert!(a.impact.is_some(), "the bolt never landed");
    }
    /// Heat is state: a range restored mid-fire matches tick for tick, and
    /// one unit of heat more changes the hash.
    #[test]
    fn heat_and_fire_are_saved_and_hashed() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.airburst_on_target = false;
        let mut a = BlastRange::with_layout(config(), layout).unwrap();
        a.control();
        a.patrols_running = false;
        a.aim = Some(a_crown(&a));
        // Let the turret traverse to the crown before the round is charged.
        for _ in 0..120 {
            a.step();
        }
        assert!(a.refire());
        for _ in 0..400 {
            a.step();
            if a.heat.burning() > 0 {
                break;
            }
        }
        assert!(a.heat.burning() > 0, "nothing burns to save");
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        for _ in 0..60 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert_eq!(a.heat.hot(), b.heat.hot());
        let mut c = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        let (p, _) = c.heat.hottest().unwrap();
        c.heat.add(&c.world, p, 1);
        assert_ne!(a.state_hash(), c.state_hash());
    }
    fn shielded_layout() -> RangeLayout {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.airburst_on_target = false;
        layout.shields.push(RangeShieldDef {
            at: [layout.target[0], FLOOR, layout.target[2]],
            radius: 40,
        });
        layout
    }
    fn caught(a: &mut BlastRange) -> Option<ShieldHit> {
        let hits = a.shields[0].hits;
        for _ in 0..300 {
            a.step();
            if let Some(h) = a.shield_hits.first() {
                return Some(*h);
            }
            if a.shields[0].hits > hits || a.impact.is_some() {
                break;
            }
        }
        None
    }
    /// A dome over the target catches the round at its surface: no impact,
    /// no crater, and the weapon's wave energy leaves the charge (ADR-276).
    #[test]
    fn a_dome_stops_the_round_at_its_surface_and_pays_with_charge() {
        let mut a = BlastRange::with_layout(config(), shielded_layout()).unwrap();
        let capacity = a.shields[0].charge;
        let h = caught(&mut a).expect("the round never met the dome");
        assert!(
            a.impact.is_none() && a.explosion.is_none(),
            "the round exploded"
        );
        assert_eq!((a.removed, a.shell), (0, None));
        let s = &a.shields[0];
        assert_eq!(
            (s.hits, s.charge, s.down, h.collapsed, h.drained),
            (1, capacity - a.config.energy, 0, false, a.config.energy)
        );
        let d2: i64 = (0..3).map(|k| i64::from(h.at[k] - s.at[k]).pow(2)).sum();
        let r = i64::from(s.radius);
        assert!(
            (d2 - r * r).abs() <= 4 * r,
            "caught at {:?}, not on a dome of radius {r}",
            h.at
        );
        assert!(a.ready(), "the spent round left the gun busy");
    }
    /// Both weapons drain one dome by their own wave energy: the HE round
    /// leaves less than the energy bolt needs, so the bolt collapses the
    /// dome, a third round passes while it is down, and the charge climbs
    /// back at the tuned rate once it stands again.
    #[test]
    fn two_weapons_on_one_dome_drain_it_and_the_second_collapses_it() {
        let mut a = BlastRange::with_layout(config(), shielded_layout()).unwrap();
        a.control();
        let tuning = *a.shield_tuning();
        let he = config().energy;
        let bolt =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        assert!(
            he < tuning.capacity && he + bolt.energy > tuning.capacity,
            "shield.ron no longer holds one round of each weapon and not two"
        );
        assert!(a.refire());
        let first = caught(&mut a).expect("the HE round passed the dome");
        assert!(!first.collapsed);
        assert_eq!(a.shields[0].charge, tuning.capacity - he);
        let bolt_energy = bolt.energy;
        a.config = bolt;
        assert!(a.refire());
        let second = caught(&mut a).expect("the bolt passed the dome");
        let s = &a.shields[0];
        assert!(second.collapsed && s.down > 0 && s.charge == 0, "{s:?}");
        // The dome recharged a little while the bolt was on its way, but
        // never enough to hold it.
        assert_eq!(s.hits, 2);
        assert!(
            second.drained >= tuning.capacity - he && second.drained < bolt_energy,
            "drained {} of a dome recharging from {}",
            second.drained,
            tuning.capacity - he
        );
        assert!(a.impact.is_none());
        assert!(a.refire());
        assert!(
            caught(&mut a).is_none(),
            "the collapsed dome caught a round"
        );
        assert!(
            a.impact.is_some(),
            "the round neither hit the dome nor the ground"
        );
        assert_eq!(a.shields[0].hits, 2);
        while a.shields[0].down > 0 {
            a.step();
        }
        for _ in 0..20 {
            a.step();
        }
        assert_eq!(a.shields[0].charge, 20 * tuning.recharge_per_tick);
    }
    /// Dome state is saved and hashed: a range restored after a catch
    /// matches tick for tick, and a spent point of charge changes the hash.
    #[test]
    fn dome_charge_and_outage_are_saved_and_hashed() {
        let mut a = BlastRange::with_layout(config(), shielded_layout()).unwrap();
        assert!(caught(&mut a).is_some());
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        for _ in 0..60 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert_eq!(a.shields[0].charge, b.shields[0].charge);
        let mut c = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        c.shields[0].charge -= 1;
        assert_ne!(a.state_hash(), c.state_hash());
        let mut wide = shielded_layout();
        wide.shields[0].radius = 200;
        assert!(BlastRange::with_layout(config(), wide).is_err());
    }
    #[test]
    fn a_tank_pushes_through_the_forest_and_the_trees_fall_as_debris() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        a.patrols_running = false;
        assert!(
            a.order_move([128, 470]),
            "no route into the woods from {:?}",
            a.gun.motion.pos
        );
        let mut b: Option<BlastRange> = None;
        let mut cut_ticks = 0;
        let mut released = 0;
        let mut blocked_ticks = 0;
        let mut reversed = false;
        let mut travelled = Fx::ZERO;
        let mut arrived = None;
        for tick in 0..1200 {
            let before = a.gun.motion.pos;
            a.step();
            if let Some(r) = b.as_mut() {
                r.step();
                assert_eq!(a.state_hash(), r.state_hash(), "tick {tick}");
            }
            let m = &a.gun.motion;
            let dx = m.pos[0].sub(before[0]);
            let dz = m.pos[2].sub(before[2]);
            travelled = travelled.add(dx.mul(dx).add(dz.mul(dz)).sqrt());
            blocked_ticks += usize::from(m.blocked);
            reversed |= m.drive.vel < Fx::ZERO;
            if !m.cut.is_empty() {
                cut_ticks += 1;
                if b.is_none() {
                    b = Some(BlastRange::restore(c.clone(), &a.snapshot()).unwrap());
                }
            }
            released = released.max(a.matter.felling_pending());
            assert!(a.matter.economy.ledger.balanced());
            if m.destination.is_none() {
                arrived.get_or_insert(tick);
                if a.matter.felling_pending() == 0 {
                    break;
                }
            }
        }
        let wood = a.removed_materials[4] + a.removed_materials[5];
        eprintln!(
            "arrived {arrived:?} ticks {} travelled {} blocked {blocked_ticks} reversed {reversed} \
             cut_ticks {cut_ticks} frontier_max {released} foliage {} trunk {} spent_mj {} fragments {}",
            a.tick,
            travelled.to_int_trunc(),
            a.removed_materials[4],
            a.removed_materials[5],
            a.matter.spent_mj,
            a.matter.debris.fragments.len()
        );
        assert!(
            a.gun.motion.destination.is_none(),
            "stuck at {:?} blocked {}",
            a.gun.motion.pos,
            a.gun.motion.blocked
        );
        assert!(!reversed, "the hull backed off a tree");
        assert!(
            cut_ticks > 0 && released > 0,
            "cut {cut_ticks} released {released}"
        );
        assert!(wood > 200 && a.removed_materials[5] > 0, "felled {wood}");
        assert_eq!(a.matter.economy.ledger.sunk_total(), 0);
        let m = &a.matter;
        assert_eq!(
            m.debris
                .fragments
                .iter()
                .map(|f| u64::from(f.count))
                .sum::<u64>(),
            a.removed as u64
        );
        assert!(m.spent_mj <= m.supplied_mj && m.spent_mj > 0);
        // Nothing yielding is left standing inside the hull where it stopped.
        let shape = a.gun.motion.shape(&a.gun.body);
        let mut probe = nav::vehicle::Probe::with_yield(&a.world, &a.navigation.fell.materials);
        assert!(probe
            .crushed(
                [a.gun.motion.pos[0], a.gun.motion.pos[2]],
                a.gun.motion.drive.head,
                shape
            )
            .is_empty());
        assert_eq!(
            a.state_hash(),
            BlastRange::restore(c, &a.snapshot()).unwrap().state_hash()
        );
    }
    #[test]
    fn a_walker_fells_trees_on_the_live_range_and_replays() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        // Only this walker moves; neither gunfire nor a tracked patrol may
        // fell trees on its behalf.
        for u in &mut a.units {
            u.motion.enabled = false;
            u.motion.route.clear();
        }
        let felled_before = a.removed_materials[4] + a.removed_materials[5];
        let id = a
            .units
            .iter()
            .position(|u| u.kind == RangeUnitKind::Strider)
            .expect("a strider on the range");
        let body = a.units[id].body.clone();
        let others: Vec<_> = a
            .units
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != id)
            .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
            .collect();
        a.units[id].motion.route.clear();
        a.units[id]
            .motion
            .move_to(&a.world, [128, 470], &body, &others, &a.rubble);
        let mut cut = false;
        let mut replay: Option<BlastRange> = None;
        for _ in 0..1200 {
            a.step();
            if let Some(b) = replay.as_mut() {
                b.step();
                assert_eq!(a.state_hash(), b.state_hash());
            }
            cut |= !a.units[id].motion.cut.is_empty();
            if cut && replay.is_none() {
                replay = Some(BlastRange::restore(c.clone(), &a.snapshot()).unwrap());
            }
            if a.units[id].motion.destination.is_none() && a.matter.felling_pending() == 0 {
                break;
            }
        }
        assert!(cut && replay.is_some(), "walker never cut a tree");
        assert!(
            a.units[id].motion.destination.is_none(),
            "walker stuck at {:?}",
            a.units[id].motion.pos
        );
        assert!(a.removed_materials[4] + a.removed_materials[5] > felled_before);
    }
    #[test]
    fn a_waiting_section_does_not_freeze_damage_elsewhere_on_the_body() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        a.patrols_running = false;
        let u = &mut a.units[0];
        for z in 0..30 {
            for y in 0..30 {
                if u.body.remove(54, y, z) {
                    u.removed.push([54, y as u8, z as u8]);
                }
            }
        }
        u.revision = u.removed.len() as u64;
        a.matter.annihilate_body(0, u.removed.len());
        u.refresh_damage(0);
        let detached = u.status.detached;
        assert!(u.status.alive && detached > 8);
        a.matter.tuning.max_flying = 0;
        a.matter.step_sections(&mut a.units);
        assert!(a.units[0].section.as_ref().unwrap().ready());
        // A hull cell outside the pending section still takes the impulse.
        let u = &a.units[0];
        let job = u.section.as_ref().unwrap();
        let hull = (0..u.body.vol.nx as usize)
            .flat_map(|x| (0..u.body.vol.ny as usize).map(move |y| [x, y, 15]))
            .find(|&[x, y, z]| {
                u.body.vol.get(x, y, z)
                    && !job.holds([x as u8, y as u8, z as u8])
                    && u.body.part_at(x, y, z) == voxel::Part::Armour
            })
            .expect("an armour cell that stays with the hull");
        let after_cut = u.body.solid_count();
        a.units[0].receive_cell(hull, 1_000_000, &c);
        a.matter.annihilate_body(0, 1);
        a.units[0].refresh_damage(0);
        assert_eq!(a.units[0].body.solid_count(), after_cut - 1);
        assert!(a.units[0].status.alive && a.units[0].section.is_some());
        let restored = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), restored.state_hash());
        a.matter.tuning.max_flying = restored.matter.tuning.max_flying;
        for _ in 0..40 {
            a.step();
            if a.units[0].section.is_none() {
                break;
            }
        }
        let u = &a.units[0];
        assert!(u.section.is_none());
        assert!(
            !u.body.vol.get(hull[0], hull[1], hull[2]),
            "commit resurrected the hull cell"
        );
        assert_eq!(u.body.solid_count(), after_cut - 1 - detached);
        assert_eq!(u.status.detached, 0);
        assert_eq!(
            a.matter
                .debris
                .fragments
                .iter()
                .map(|f| f.count)
                .sum::<u32>(),
            detached
        );
        assert!(a.matter.economy.ledger.balanced());
        assert_eq!(
            a.state_hash(),
            BlastRange::restore(c, &a.snapshot()).unwrap().state_hash()
        );
    }
    /// The owner's report: a directed-energy hit on a moving tank kept eating
    /// it for almost three seconds after the flash. The pieces cut loose were
    /// leaving one per tick while the wave waited on them (ADR-270).
    #[test]
    fn damage_arrives_with_the_wave_while_sections_are_still_leaving() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        let mut b: Option<BlastRange> = None;
        // The opening round lands on the default target; `hit` is the impact
        // of the round that actually struck the moving tank.
        let mut current = None;
        let mut hit = None;
        let mut first = None;
        let mut last = None;
        let mut death = None;
        let mut carried = 0;
        let mut stalled = 0;
        let mut previous_examined = 0;
        let mut last_removed = 0;
        for tick in 0..600u32 {
            if tick == 20 {
                a.order_move([235, 365]);
            }
            if tick == 220 {
                assert!(a.order_attack(2));
            }
            // One round only: the reload would otherwise land a second hit.
            if hit.is_some_and(|t| tick == t + 50) {
                break;
            }
            if hit.is_some() {
                carried += usize::from(a.units[2].section.is_some());
            }
            a.step();
            if let Some(r) = b.as_mut() {
                r.step();
                assert_eq!(a.state_hash(), r.state_hash(), "tick {tick}");
            }
            if a.impact.is_some() && current != Some(a.impact_tick) {
                current = Some(a.impact_tick);
                previous_examined = 0;
            }
            let u = &a.units[2];
            if u.removed.len() > last_removed {
                if hit.is_none() {
                    hit = current;
                    first = Some(tick);
                }
                last = Some(tick);
                last_removed = u.removed.len();
            }
            if !u.status.alive {
                death.get_or_insert(tick);
            }
            let examined = a.explosion.as_ref().map_or(0, |e| e.wave.examined);
            let finished = a.explosion.as_ref().is_none_or(|e| e.wave.finished());
            if hit.is_some() && !finished && examined == previous_examined {
                stalled += 1;
            }
            previous_examined = examined;
            if b.is_none() && hit.is_some_and(|t| tick == t + 2) {
                b = Some(BlastRange::restore(c.clone(), &a.snapshot()).unwrap());
            }
            assert!(a.matter.economy.ledger.balanced());
        }
        let (impact, first, last) = (hit.unwrap(), first.unwrap(), last.unwrap());
        let u = &a.units[2];
        eprintln!(
            "impact {impact} first {first} death {death:?} last {last} stalled {stalled} \
             carried {carried} removed {} detached {} finished {}",
            u.removed.len(),
            u.detached,
            a.explosion.as_ref().is_none_or(|e| e.wave.finished())
        );
        assert!(b.is_some(), "the replay never forked");
        assert_eq!(stalled, 0, "the wave paused for debris preparation");
        assert_eq!(first, impact);
        // Eight ticks is 0.27 s: the wave budget in energy-cannon.ron sweeps
        // the whole body within that, and nothing else may add to it. Where
        // the round lands on the moving hull depends on the drive model, so
        // the core is not always reached; when it is, it dies inside the same
        // window, and the hit is always a real one.
        assert!(
            last - impact <= 8 && death.is_none_or(|d| d - impact <= 8),
            "impact {impact} first {first} death {death:?} last {last}: destruction lagged the flash"
        );
        assert!(
            u.removed.len() > 2000,
            "a glancing hit: {} cells",
            u.removed.len()
        );
        assert!(u.detached > 0 && u.annihilated_mm3() > 0);
        // Pieces cut loose leave in the tick that frees them; nothing lingers.
        assert!(carried <= 2, "sections waited {carried} ticks");
    }
    #[test]
    fn material_shell_transfers_ground_and_armour_with_finite_energy_and_exact_resume() {
        let c = config();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.target = [359, 28, 312];
        layout.airburst_on_target = false;
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        for _ in 0..300 {
            a.step();
            if a.units[0].detached > 0 {
                break;
            }
        }
        assert!(
            a.units[0].detached > 0,
            "body export path was not exercised"
        );
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..300 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            let m = &a.matter;
            assert!(m.economy.ledger.balanced());
            assert_eq!(m.economy.ledger.sunk_total(), 0);
            assert!(m.spent_mj <= m.supplied_mj);
            assert!(
                m.debris.motion_mj + m.debris.body_motion_mj
                    <= m.spent_mj * u64::from(m.tuning.motion_pct) / 100
            );
            let body = a.units.iter().map(|u| u.detached as u64).sum::<u64>();
            assert_eq!(
                m.debris
                    .fragments
                    .iter()
                    .map(|f| u64::from(f.count))
                    .sum::<u64>(),
                a.removed as u64 + body
            );
            let ground_mass = a
                .removed_materials
                .iter()
                .enumerate()
                .map(|(i, &n)| {
                    m.economy.mats.get(i as u16).map_or(0, |d| {
                        n as u64
                            * u64::from(d.density)
                            * u64::from(m.tuning.density_kg_m3_per_density)
                    })
                })
                .sum::<u64>();
            assert_eq!(
                m.debris.fragments.iter().map(|f| f.mass_g).sum::<u64>(),
                ground_mass + body * 7850
            );
        }
        assert!(
            a.matter.debris.fragments.iter().any(|f| f.resting),
            "ground={} body={} first={:?}",
            a.removed,
            a.units[0].detached,
            a.matter
                .debris
                .fragments
                .first()
                .map(|f| (f.position, f.velocity))
        );
        assert_eq!(a.units[0].annihilated_mm3(), 0);
        assert!(
            a.units[1].removed.is_empty(),
            "the wall must still protect the rear tank"
        );
    }
    #[test]
    fn small_fragment_pool_defers_the_wave_without_discarding_matter() {
        let mut r = BlastRange::new(config());
        r.matter.tuning.max_flying = 16;
        let mut stalled = false;
        for _ in 0..6000 {
            let before = r.removed;
            r.step();
            let m = &r.matter;
            assert!(m.debris.fragments.iter().filter(|f| !f.resting).count() <= 16);
            assert_eq!(
                m.debris
                    .fragments
                    .iter()
                    .map(|f| u64::from(f.count))
                    .sum::<u64>(),
                r.removed as u64
            );
            assert!(m.economy.ledger.balanced());
            stalled |=
                r.explosion.as_ref().is_some_and(|e| !e.pending.is_empty()) && r.removed == before;
            if r.impact.is_some() && r.ready() {
                break;
            }
        }
        assert!(
            stalled && r.ready() && r.removed > 100,
            "stalled={stalled} ready={} removed={} moving={} first={:?}",
            r.ready(),
            r.removed,
            r.matter
                .debris
                .fragments
                .iter()
                .filter(|f| !f.resting)
                .count(),
            r.matter
                .debris
                .fragments
                .first()
                .map(|f| (f.position, f.velocity))
        );
        let mut ordinary = BlastRange::new(config());
        for _ in 0..500 {
            ordinary.step();
        }
        assert_eq!(r.removed_materials, ordinary.removed_materials);
        assert_eq!(r.matter.spent_mj, ordinary.matter.spent_mj);
    }
    #[test]
    fn switching_to_annihilation_keeps_existing_physical_debris() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut r = BlastRange::with_layout(config(), layout).unwrap();
        for _ in 0..400 {
            r.step();
        }
        let count = r
            .matter
            .debris
            .fragments
            .iter()
            .map(|f| u64::from(f.count))
            .sum::<u64>();
        let detached_before = r
            .units
            .iter()
            .map(|u| (u.detached + u.seg_detached) as u64)
            .sum::<u64>();
        r.config =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        r.aim = Some([359, 28, 312]);
        assert!(r.refire());
        for _ in 0..200 {
            r.step();
        }
        // Whole pieces cut loose stay matter under any weapon: sections of
        // the root and shed segments alike.
        assert_eq!(
            r.matter
                .debris
                .fragments
                .iter()
                .map(|f| u64::from(f.count))
                .sum::<u64>(),
            count
                + r.units
                    .iter()
                    .map(|u| (u.detached + u.seg_detached) as u64)
                    .sum::<u64>()
                - detached_before
        );
        assert!(r.matter.economy.ledger.sunk_total() > 0 && r.matter.economy.ledger.balanced());
        assert_eq!(
            r.state_hash(),
            BlastRange::restore(r.config.clone(), &r.snapshot())
                .unwrap()
                .state_hash()
        );
    }
    /// Стойкость мишеней принадлежит раскладке, а не пресету оружия.
    ///
    /// `with_layout` ставит её при сборке; смена оружия заменяет
    /// конфигурацию целиком и обязана вернуть её на место. Раньше это
    /// помнило окно, и проверить это было нечем (ADR-269).
    #[test]
    fn reconfigure_keeps_the_layout_resistance_when_the_weapon_changes() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let wanted = layout.unit_resistance;
        let mut r = BlastRange::with_layout(
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
            layout,
        )
        .unwrap();
        assert_eq!(r.config.unit_ablation_resistance, wanted);
        let shell = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        // Проверка имеет смысл только пока пресет сам по себе даёт другое
        // число: иначе она пройдёт и без единой строки `reconfigure`.
        assert_ne!(shell.unit_ablation_resistance, wanted);
        r.reconfigure(shell);
        assert_eq!(r.config.unit_ablation_resistance, wanted);
    }
    #[test]
    fn course_keeps_the_walkers_higher_step_limit() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let course = layout.course.as_ref().unwrap();
        let r = BlastRange::with_layout(config(), layout.clone()).unwrap();
        assert_eq!(r.gun.motion.step, course.step_height);
        for (u, def) in r.units.iter().zip(&layout.units) {
            assert_eq!(
                u.motion.step,
                if def.kind == RangeUnitKind::Tank {
                    course.step_height
                } else {
                    course.walker_step_height
                }
            );
        }
        assert!(r.units.iter().any(|u| u.kind == RangeUnitKind::Strider));
    }
    #[test]
    fn patrols_complete_two_laps_and_movement_resumes_exactly() {
        let mut a = BlastRange::with_layout(
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap(),
        )
        .unwrap();
        a.control();
        let mut highest = [16; 2];
        let mut tilted = false;
        for _ in 0..9000 {
            a.step();
            for (i, high) in highest.iter_mut().enumerate() {
                *high = (*high).max(a.units[i + 2].at[1]);
            }
            tilted |= a.units[2].motion.hull.up.x.abs() > Fx::from_ratio(1, 100);
            if a.units[2..].iter().all(|u| u.motion.laps >= 2) {
                break;
            }
        }
        for u in &a.units[2..] {
            assert!(
                u.motion.laps >= 2,
                "kind {:?} at {:?} laps {} blocked {} waypoint {} cursor {}/{} head {} path {:?}",
                u.kind,
                u.at,
                u.motion.laps,
                u.motion.blocked,
                u.motion.waypoint,
                u.motion.cursor,
                u.motion.path.len(),
                u.motion.drive.head,
                u.motion.path
            );
        }
        assert!(
            highest[0] > 20 && highest[1] > 19 && tilted,
            "ramps {highest:?}"
        );
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        for _ in 0..100 {
            a.step();
            b.step();
        }
        assert_eq!(a.state_hash(), b.state_hash());
        assert!(a.order_move([245, 340]));
        for _ in 0..300 {
            a.step();
        }
        assert!(
            (a.gun.at[0] - 245).abs() < 8,
            "gun {:?} motion {:?}",
            a.gun.at,
            a.gun.motion
        );
        assert!(a.order_attack(2));
        let removed = a.units[2].removed.len();
        let mut impacted = false;
        for _ in 0..240 {
            a.step();
            impacted |= a.impact.is_some();
        }
        assert!(impacted, "attack never produced an impact");
        assert!(
            a.units[2].removed.len() > removed,
            "moving target must take spatial damage"
        );
    }
    #[test]
    fn off_axis_aim_rotates_muzzle_without_increasing_horizontal_speed_and_resumes() {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        c.fire_tick = 0;
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.blocks.clear();
        layout.units.clear();
        layout.course = None;
        layout.target = [450, 24, 380];
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        assert!(a.muzzle()[2] > Fx::from_int(MUZZLE[2]));
        assert!(a.muzzle()[0] < Fx::from_int(MUZZLE[0]));
        a.step();
        let speed = a.velocity[0]
            .mul(a.velocity[0])
            .add(a.velocity[2].mul(a.velocity[2]))
            .sqrt();
        assert!(speed.sub(Fx::from_int(c.shell_speed)).abs() < Fx::from_ratio(1, 1000));
        let bytes = a.snapshot();
        let mut b = BlastRange::restore(c, &bytes).unwrap();
        for _ in 0..150 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!(a.impact.is_some());
    }
    #[test]
    fn steel_receives_work_and_repeated_rounds_erode_its_front_without_immunity() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut r = BlastRange::with_layout(c.clone(), layout).unwrap();
        for _ in 0..250 {
            r.step();
        }
        let steel: Vec<_> = r
            .surface_work
            .iter()
            .filter(|(p, _)| material(&r.world, **p) == 3)
            .map(|(p, w)| (*p, *w))
            .collect();
        assert!(steel.len() > 100, "steel did not receive surface work");
        assert_eq!(r.removed_materials[3], 0);
        assert!(!r.units[0].dents.is_empty());
        assert!(r.units[1].dents.is_empty() && r.units[1].removed.is_empty());
        // Work is spatial and remains part of the snapshot, not a global HP bar.
        for (p, _) in &steel {
            assert!(p[0] < 400);
        }
        for _ in 0..6 {
            assert!(r.refire());
            assert!(!r.refire(), "a charging shot was overwritten");
            let bytes = r.snapshot();
            let mut resumed = BlastRange::restore(c.clone(), &bytes).unwrap();
            for _ in 0..160 {
                r.step();
                resumed.step();
            }
            assert_eq!(r.state_hash(), resumed.state_hash());
        }
        assert!(
            r.removed_materials[3] > 0,
            "steel remained immune to repeated work"
        );
        assert!(
            r.units[1].removed.is_empty(),
            "small front scars punched through the thick core"
        );
    }
    #[test]
    fn stronger_round_breaks_steel_and_work_stays_local_on_the_body() {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        c.energy *= 4;
        let mut r = BlastRange::with_layout(c.clone(), layout).unwrap();
        for _ in 0..300 {
            r.step();
        }
        assert!(r.removed_materials[3] > 0);
        let mut u = RangeUnit::new([100, 16, 100]);
        let mut chosen = None;
        for z in 0..u.body.vol.nz as usize {
            for y in 0..u.body.vol.ny as usize {
                for x in 0..u.body.vol.nx as usize {
                    if u.body.vol.get(x, y, z) && u.body.part_at(x, y, z) == voxel::Part::Armour {
                        chosen = Some([x, y, z]);
                    }
                }
            }
        }
        let p = chosen.unwrap();
        let q = u.world_pos(p);
        let cost = unit_work(&u.body, p, &c);
        u.receive(q, cost / 2, &c);
        assert!(u.solid(q));
        assert_eq!(u.dents.len(), 1);
        assert!(u.removed.is_empty());
        u.receive(q, cost - cost / 2, &c);
        assert!(!u.solid(q));
        assert_eq!(u.removed.len(), 1);
        assert!(u.dents.is_empty());
    }
    #[test]
    fn weapon_switch_restores_the_original_generator_and_roster() {
        let energy =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        for (initial, current) in [(config(), energy.clone()), (energy, config())] {
            let mut a = BlastRange::new(initial);
            for _ in 0..200 {
                a.step();
            }
            a.config = current;
            let b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
            assert_eq!(a.state_hash(), b.state_hash());
            assert_eq!(a.units.len(), b.units.len());
            for x in [234, 258, 340, 394, 418] {
                assert_eq!(a.world.material(x, 30, 336), b.world.material(x, 30, 336));
            }
        }
    }

    #[test]
    fn a_single_fragment_slot_progresses_and_resumes_inside_a_contact() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.target = [359, 28, 312];
        layout.airburst_on_target = false;
        let c = config();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.matter.tuning.max_flying = 1;
        for _ in 0..400 {
            a.step();
            if a.explosion.as_ref().is_some_and(|e| e.contact_cursor > 0) {
                break;
            }
        }
        assert!(a.explosion.as_ref().is_some_and(|e| e.contact_cursor > 0));
        assert_eq!(a.matter.debris.fragments.len(), 1);
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        b.matter.tuning.max_flying = 1;
        for _ in 0..300 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            assert!(a.matter.economy.ledger.balanced());
            assert!(
                a.matter
                    .debris
                    .fragments
                    .iter()
                    .filter(|f| !f.resting)
                    .count()
                    <= 1
            );
        }
        assert!(
            a.matter
                .debris
                .fragments
                .iter()
                .map(|f| f.count)
                .sum::<u32>()
                > 1
        );
    }

    #[test]
    fn failed_move_retries_after_obstacle_leaves_and_stop_cancels_it() {
        let mut a = BlastRange::new(config());
        let goal = [180, 336];
        let obstacle = motion::Obstacle {
            pos: [
                Fx::from_int(goal[0]),
                Fx::from_int(FLOOR),
                Fx::from_int(goal[1]),
            ],
            head: 0,
            half: [Fx::from_int(32); 2],
        };
        assert!(a
            .gun
            .motion
            .move_to(&a.world, goal, &a.gun.body, &[obstacle], &a.rubble));
        // The obstructed target ring is checked in bounded slices too.
        for tick in 0..20 {
            if !a.gun.motion.searching() {
                break;
            }
            a.gun.motion.step(
                &a.world,
                &a.gun.body,
                true,
                true,
                a.gun.mobility,
                &[obstacle],
                tick,
                a.config.gravity,
                &a.rubble,
                true,
            );
        }
        assert!(!a.gun.motion.searching());
        assert!(a.gun.motion.blocked);
        assert!(a.gun.motion.path.is_empty());
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        b.gun.motion.step(
            &b.world,
            &b.gun.body,
            true,
            true,
            b.gun.mobility,
            &[],
            b.gun.motion.replan + 20,
            b.config.gravity,
            &b.rubble,
            true,
        );
        assert!(!b.gun.motion.path.is_empty());
        a.gun.motion.stop();
        a.gun.motion.step(
            &a.world,
            &a.gun.body,
            true,
            true,
            a.gun.mobility,
            &[],
            a.gun.motion.replan,
            a.config.gravity,
            &a.rubble,
            true,
        );
        assert!(a.gun.motion.path.is_empty());
        assert_eq!(a.gun.motion.destination, None);
    }

    #[test]
    fn a_tick_that_begins_a_search_does_not_also_walk_it() {
        // Beginning a search is a slice of work by itself (ADR-314): the
        // goal is probed from all eight headings over a ring of cells and
        // the straight run to it is walked pose by pose. The frontier
        // waits for the next tick, so one tick never pays for both.
        let mut r = BlastRange::new(config());
        assert!(r.order_move([128, 470]), "the order was refused");
        assert!(
            r.gun.motion.searching(),
            "the goal answered at once; this test needs one that does not"
        );
        assert_eq!(
            r.gun.motion.frontier_nodes(),
            0,
            "the tick that began the search also walked its frontier"
        );
        for _ in 0..3 {
            r.step();
        }
        assert!(
            !r.gun.motion.searching() || r.gun.motion.frontier_nodes() > 0,
            "the frontier stood still after the tick that began it"
        );
    }
    #[test]
    fn a_tick_begins_no_more_searches_than_the_ceiling_allows() {
        // Beginning a search costs a hundred times a slice of one
        // (ADR-312), so a tick begins only so many and the hulls over the
        // ceiling keep their orders for the next tick. Every hull still
        // gets its route: the patrol drives on.
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        let mut r = BlastRange::with_layout(config(), layout).unwrap();
        r.control();
        assert_eq!(
            r.navigation.searches_started_per_tick, 1,
            "the data's ceiling"
        );
        let began = |r: &BlastRange| {
            r.units.iter().filter(|u| u.motion.started_search()).count()
                + usize::from(r.gun.motion.started_search())
        };
        let mut started = vec![false; r.units.len()];
        for _ in 0..400 {
            r.step();
            let n = began(&r);
            assert!(n <= 1, "{n} searches began in one tick");
            for (seen, u) in started.iter_mut().zip(&r.units) {
                *seen |= u.motion.started_search();
            }
        }
        for (id, u) in r.units.iter().enumerate() {
            if !u.motion.route.is_empty() {
                assert!(started[id], "patrol {id} was starved by the start ceiling");
            }
        }
        assert!(
            r.units.iter().any(|u| !u.motion.path.is_empty()),
            "no hull ever got a route"
        );
        // Without the ceiling the hulls pile into the same tick, which is
        // what made the dearest tick of the range.
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        let mut loose = BlastRange::with_layout(config(), layout).unwrap();
        loose.control();
        loose.navigation.searches_started_per_tick = usize::MAX;
        let mut worst = 0;
        for _ in 0..400 {
            loose.step();
            worst = worst.max(began(&loose));
        }
        assert!(worst > 1, "the ceiling was not what kept them apart");
    }
    #[test]
    fn a_layout_says_how_wide_its_world_is() {
        // The size of the range lives in its layout, not in the code
        // (ADR-315): the game is to have a kilometre, and the tests a
        // range small enough to build hundreds of times. A preset from
        // before keeps the stock range, and the planner's bounds follow
        // the world instead of the old six hundred and forty.
        let stock = BlastRange::new(config());
        assert_eq!(stock.size(), SIZE, "no layout, stock range");
        let mut layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        assert_eq!(
            layout.size, SIZE,
            "a preset without a size keeps the stock one"
        );
        layout.size = [1024, 192, 1024];
        let mut wide = BlastRange::with_layout(config(), layout).unwrap();
        assert_eq!(wide.size(), [1024, 192, 1024]);
        assert_eq!(
            [wide.world.nx, wide.world.ny, wide.world.nz],
            [1024, 192, 1024],
            "the world was built to the layout's size"
        );
        assert!(
            material(&wide.world, [900, 10, 900]) != 0,
            "the far corner of the wider world has no ground"
        );
        // An order out where the stock range has no room at all.
        let far = [900, 900];
        wide.gun.motion.move_to(
            &wide.world,
            far,
            &wide.gun.body,
            &[],
            &nav::vehicle::Rubble::new(),
        );
        assert_eq!(
            wide.gun.motion.destination,
            Some(far),
            "the planner's bounds did not follow the world"
        );
        let mut narrow = BlastRange::new(config());
        narrow.gun.motion.move_to(
            &narrow.world,
            far,
            &narrow.gun.body,
            &[],
            &nav::vehicle::Rubble::new(),
        );
        assert_eq!(
            narrow.gun.motion.destination, None,
            "the stock range accepted an order outside itself"
        );
    }
    #[test]
    fn the_column_of_the_range_matches_its_voxels() {
        // The generator answers a whole column at once (ADR-319), with
        // everything that depends only on (x, z) worked out once. The
        // answer has to be the same as the per-voxel walk to the bit,
        // or the world is built one way and read another. Checked on
        // both ranges the code makes: the bare one and the one a layout
        // lays out, with its blocks, its course and its clearings.
        let layouts = [
            None,
            Some(
                modding::range::RangeLayout::from_ron(include_str!(
                    "../../../data/range-cover.ron"
                ))
                .unwrap(),
            ),
        ];
        for layout in layouts {
            for extended in [false, true] {
                let gen = RangeTerrain::new(extended, layout.clone());
                let mut col = vec![0u16; SIZE[1] as usize];
                let mut solid = 0usize;
                let mut kinds = std::collections::BTreeSet::new();
                // Every seventh column: the walk is the reference, and
                // over four hundred thousand columns of it is a minute.
                for z in (0..SIZE[2]).step_by(7) {
                    for x in (0..SIZE[0]).step_by(7) {
                        gen.column(x, z, 0, &mut col);
                        for (y, &m) in col.iter().enumerate() {
                            assert_eq!(
                                m,
                                gen.material(x, y as u32, z),
                                "column {x},{z} at {y}, extended={extended}"
                            );
                            if m != 0 {
                                solid += 1;
                                kinds.insert(m);
                            }
                        }
                    }
                }
                assert!(solid > 100_000, "the range came out empty: {solid}");
                assert!(
                    kinds.len() >= 4,
                    "only {kinds:?} of the range's materials were seen"
                );
            }
        }
    }
    #[test]
    fn all_sector_blocks_cutouts_and_index_boundaries_match_authored_material() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        let gen = RangeTerrain::new(true, Some(layout.clone()));
        let mut columns = std::collections::BTreeSet::new();
        for b in &layout.blocks {
            for x in [
                b.min[0] - 1,
                b.min[0],
                (b.min[0] + b.max[0]) / 2,
                b.max[0] - 1,
                b.max[0],
            ] {
                for z in [
                    b.min[2] - 1,
                    b.min[2],
                    (b.min[2] + b.max[2]) / 2,
                    b.max[2] - 1,
                    b.max[2],
                ] {
                    columns.insert([x, z]);
                }
            }
        }
        for zone in &layout.zones {
            for x in (zone.min[0] - 1..=zone.max[0]).step_by(127) {
                for z in (zone.min[1] - 1..=zone.max[1]).step_by(127) {
                    columns.insert([x, z]);
                }
            }
        }
        for [x, z] in columns {
            let mut col = [0u16; 192];
            gen.column(x as u32, z as u32, 0, &mut col);
            for (y, &mat) in col.iter().enumerate() {
                assert_eq!(
                    mat,
                    gen.material(x as u32, y as u32, z as u32),
                    "{x},{y},{z}"
                );
                // Independent full walk verifies broad-phase candidates and last-writer order.
                if let Some(b) = layout
                    .blocks
                    .iter()
                    .rev()
                    .find(|b| b.contains([x, y as i32, z]))
                {
                    assert_eq!(mat, b.material, "authored block at {x},{y},{z}");
                }
            }
        }
    }
    #[test]
    fn roads_link_sectors_and_keep_the_driving_strip_free_of_trees() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        let gen = RangeTerrain::new(true, Some(layout));
        // These two midpoints lie between prepared sectors, where natural
        // hills and the procedural forest would otherwise fill the gap.
        for (x, z) in [(900, 420), (3900, 730)] {
            assert!(gen.road_at(x, z, 0));
            let ground = range_height(x, z);
            assert_eq!(gen.material(x, ground - 1, z), 8);
            for y in ground..ground + 48 {
                assert_eq!(gen.material(x, y, z), 0, "road blocked at {x},{y},{z}");
            }
        }
        assert!(!gen.road_at(900, 600, 0));
    }
    #[test]
    fn road_centrelines_do_not_run_through_authored_walls() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        for road in &layout.roads {
            for segment in road.points.windows(2) {
                let dx = segment[1][0] - segment[0][0];
                let dz = segment[1][1] - segment[0][1];
                let steps = (dx.abs().max(dz.abs()) + 7) / 8;
                for i in 0..=steps {
                    let x = segment[0][0] + dx * i / steps;
                    let z = segment[0][1] + dz * i / steps;
                    assert!(
                        !layout
                            .blocks
                            .iter()
                            .any(|b| b.material != 0 && b.contains([x, 40, z])),
                        "road hits a tall wall at {x},{z}"
                    );
                }
            }
        }
    }
    fn config() -> ExplosionConfig {
        ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
    }
    #[test]
    fn asking_a_cell_once_answers_as_if_every_voxel_had_been_asked() {
        // The capture used to ask three questions of every voxel of the
        // domain: which bodies can reach it, whether it is dented, and
        // which chunk holds it. All three answer the same for a whole
        // cell, and ADR-310 asks them once a cell. The answers have to be
        // the same to the bit, at the edges of the domain as much as in
        // the middle, so the per-voxel walk stands here as the reference.
        // A shot is fired first, so that the world is edited, the dents
        // are real and the bodies have moved off their start.
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        let mut r = BlastRange::with_layout(config(), layout).unwrap();
        r.control();
        r.refire();
        for _ in 0..200 {
            r.step();
        }
        assert!(!r.surface_work.is_empty(), "no dents to fall between cells");
        assert!(!r.units.is_empty(), "the cover range brings machines");
        let c = r.config.clone();
        let walk = |at: [i32; 3]| {
            let e = Explosion::with_damage(&r.world, at, &c, &r.units, &r.surface_work);
            let n = e.wave.size();
            let half = n / 2;
            let radius2 = (half * half) as i32;
            let origin = at.map(|v| v - half as i32 * c.cell_units);
            let boxes: Vec<([i32; 3], [i32; 3])> = r.units.iter().map(|u| u.bounds()).collect();
            let (mut solid, mut armoured) = (0usize, 0usize);
            for z in 0..n {
                for y in 0..n {
                    for x in 0..n {
                        let (cx, cy, cz) = (
                            x as i32 - half as i32,
                            y as i32 - half as i32,
                            z as i32 - half as i32,
                        );
                        let want = if cx * cx + cy * cy + cz * cz > radius2 {
                            u32::MAX
                        } else {
                            let p = [
                                origin[0] + x as i32 * c.cell_units,
                                origin[1] + y as i32 * c.cell_units,
                                origin[2] + z as i32 * c.cell_units,
                            ];
                            let mut want = 0;
                            for dz in 0..c.cell_units {
                                for dy in 0..c.cell_units {
                                    for dx in 0..c.cell_units {
                                        let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                                        let m = material(&r.world, q);
                                        for (u, (lo, hi)) in r.units.iter().zip(&boxes) {
                                            if (0..3).all(|a| q[a] >= lo[a] && q[a] <= hi[a]) {
                                                let hard = u.resistance(q, &c);
                                                armoured += usize::from(hard > 0);
                                                want = want.max(hard);
                                            }
                                        }
                                        let work = material_work(m, &c);
                                        if work > 0 {
                                            let dent = r.surface_work.get(&q).copied().unwrap_or(0);
                                            want = want.max(work.saturating_sub(dent));
                                        }
                                    }
                                }
                            }
                            want
                        };
                        if want > 0 && want < u32::MAX {
                            solid += 1;
                        }
                        assert_eq!(
                            e.wave.resistance_at([x, y, z]),
                            want,
                            "cell {x},{y},{z} of {n} around {at:?}"
                        );
                    }
                }
            }
            (solid, armoured)
        };
        // Once where the round landed: terrain, a dented crater and the
        // wall. Once over a machine, so that the bodies are in the domain
        // and the shortlist of bodies per cell is exercised.
        let (solid, _) = walk(r.impact.expect("the round has landed"));
        assert!(solid > 1000, "the domain held almost nothing: {solid}");
        let (lo, hi) = r.units[0].bounds();
        let (_, armoured) = walk([0, 1, 2].map(|a| (lo[a] + hi[a]) / 2));
        assert!(armoured > 0, "no body reached into the domain");
    }
    #[test]
    fn capture_clips_packed_rows_at_world_edges_for_every_cell_size() {
        struct Pattern;
        impl Generator for Pattern {
            fn material(&self, x: u32, y: u32, z: u32) -> u16 {
                if y < 8 + (x + z) % 5 {
                    ((x + 3 * y + z) % 9) as u16
                } else {
                    0
                }
            }
        }
        let w = World::new(64, 32, 64, 100, Box::new(Pattern));
        let dents = BTreeMap::from([([1, 1, 1], 777), ([62, 9, 63], 500)]);
        for edge in 1..=4 {
            let mut c = config();
            c.half_cells = 4;
            c.cell_units = edge;
            c.energy = 100;
            for at in [[0, 0, 0], [63, 10, 63]] {
                let e = Explosion::with_damage(&w, at, &c, &[], &dents);
                let n = e.wave.size();
                let half = (n / 2) as i32;
                for z in 0..n {
                    for y in 0..n {
                        for x in 0..n {
                            let q = [x as i32 - half, y as i32 - half, z as i32 - half];
                            if q.iter().map(|v| v * v).sum::<i32>() > half * half {
                                continue;
                            }
                            let base = [0, 1, 2].map(|a| at[a] + q[a] * edge);
                            let mut expected = 0;
                            for dz in 0..edge {
                                for dy in 0..edge {
                                    for dx in 0..edge {
                                        let p = [base[0] + dx, base[1] + dy, base[2] + dz];
                                        expected = expected.max(
                                            material_work(material(&w, p), &c).saturating_sub(
                                                dents.get(&p).copied().unwrap_or(0),
                                            ),
                                        );
                                    }
                                }
                            }
                            assert_eq!(e.wave.resistance_at([x, y, z]), expected);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn shell_impacts_destroys_target_and_leaves_the_barrier() {
        let mut r = BlastRange::new(config());
        for _ in 0..150 {
            r.step();
        }
        assert!(r.impact.is_some());
        assert!(r.removed > 100, "no real destruction: {}", r.removed);
        assert_eq!(
            material(&r.world, [258, 30, 336]),
            3,
            "resistant wall destroyed"
        );
        let at = r.impact.unwrap();
        assert_eq!(material(&r.world, at), 0, "impact did not open the target");
    }
    #[test]
    fn different_work_budgets_produce_the_same_crater() {
        let mut a = BlastRange::new(config());
        let mut c = config();
        c.wave_work_per_tick = 257;
        let mut b = BlastRange::new(c);
        for _ in 0..300 {
            a.step();
            b.step();
        }
        assert_eq!(a.impact, b.impact);
        assert_eq!(a.removed, b.removed);
        for z in 316..358 {
            for y in 0..50 {
                for x in 205..270 {
                    assert_eq!(a.world.material(x, y, z), b.world.material(x, y, z));
                }
            }
        }
    }
    #[test]
    fn a_capture_cut_into_slices_leaves_the_same_crater() {
        // The grid of resistance is captured under a ceiling per tick
        // (ADR-311), so a charge reaching further costs the burst more
        // ticks instead of one longer tick. The wave waits for the whole
        // grid, so what it then finds must not depend on how the capture
        // was sliced. A slice of a thousand cells cuts this burst's grid
        // into some thirty-four.
        let mut a = BlastRange::new(config());
        let mut c = config();
        c.capture_cells_per_tick = 1000;
        let mut b = BlastRange::new(c);
        let mut slices = 0;
        for _ in 0..400 {
            a.step();
            b.step();
            slices += usize::from(b.explosion.as_ref().is_some_and(|e| e.capturing()));
            assert!(
                !a.explosion.as_ref().is_some_and(|e| e.capturing()),
                "the stock ceiling captured this grid in one tick"
            );
        }
        assert!(
            slices > 20,
            "the ceiling never bit: {slices} ticks capturing"
        );
        assert_eq!(a.impact, b.impact);
        assert_eq!(a.removed, b.removed);
        for z in 316..358 {
            for y in 0..50 {
                for x in 205..270 {
                    assert_eq!(a.world.material(x, y, z), b.world.material(x, y, z));
                }
            }
        }
    }
    #[test]
    fn the_wave_waits_for_the_whole_grid() {
        // Nothing of the burst happens while the grid is still being
        // captured: no cell is dug and no pressure is dealt, so the
        // slicing cannot decide what the wave finds.
        let mut c = config();
        c.capture_cells_per_tick = 1000;
        let mut r = BlastRange::new(c);
        while r.impact.is_none() {
            r.step();
        }
        let mut waited = 0;
        while r.explosion.as_ref().is_some_and(|e| e.capturing()) {
            let walked = r.explosion.as_ref().map_or(1, |e| e.wave.examined);
            assert_eq!(walked, 0, "the wave walked before the grid was whole");
            waited += 1;
            r.step();
        }
        assert!(
            waited > 20,
            "the ceiling never bit: {waited} ticks capturing"
        );
        for _ in 0..80 {
            r.step();
        }
        assert!(
            r.explosion.as_ref().is_none_or(|e| e.wave.examined > 0),
            "the wave never ran"
        );
    }
    #[test]
    fn save_during_propagation_matches_uninterrupted_run() {
        let mut a = BlastRange::new(config());
        while a.impact.is_none() {
            a.step();
        }
        let mut b = BlastRange::restore(config(), &a.snapshot()).unwrap();
        for _ in 0..80 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
    }

    #[test]
    fn energy_bolt_flies_straight_hits_and_resumes() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::new(c.clone());
        for _ in 0..=c.fire_tick {
            a.step();
        }
        let velocity = a.velocity;
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..150 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            if a.shell.is_some() {
                assert_eq!(a.velocity, velocity, "energy bolt fell under gravity");
            }
        }
        assert_eq!(a.impact.unwrap()[0], 392, "bolt skipped the target face");
        assert!(a.removed > 100 && a.world.queued() == 0);
        assert_eq!(material(&a.world, [418, 30, 336]), 3);
        assert!(
            a.impact_tick - a.config.fire_tick <= 6,
            "combat shot is still slowed down"
        );
        let u = a.units.first().unwrap();
        assert!(u.removed.len() > 1000 && u.body.solid_count() > 10000);
        assert!(
            u.status.alive,
            "the witness should survive with visible flank damage"
        );
        let mm3 = u64::from(u.body.mm).pow(3);
        assert_eq!(
            (u.body.solid_count() as u64 + u.detached as u64) * mm3 + u.annihilated_mm3(),
            voxel::unit::tank().solid_count() as u64 * mm3
        );
    }

    #[test]
    fn witness_damage_resumes_mid_wave_and_is_budget_independent() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::new(c.clone());
        while a.units.first().unwrap().removed.is_empty() {
            a.step();
        }
        let mut b = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
        for _ in 0..90 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        let mut slow = c;
        slow.wave_work_per_tick = 257;
        let mut other = BlastRange::new(slow);
        for _ in 0..400 {
            let before = other.units.first().unwrap().removed.len();
            other.step();
            assert!(other.units.first().unwrap().removed.len() - before <= 257 * 8);
        }
        let x = &a.units.first().unwrap().body.vol;
        let y = &other.units.first().unwrap().body.vol;
        assert_eq!(x.cols, y.cols, "work slicing changed armour damage");
        assert_eq!(a.removed, other.removed);
    }

    #[test]
    fn resistant_wall_shields_the_witness() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::new(c);
        // Body begins behind the steel revetment; its near side is inside the
        // wave domain, so this exercises occlusion, not simply out-of-range.
        r.units = vec![RangeUnit::new([449, FLOOR, 336])];
        for _ in 0..250 {
            r.step();
        }
        assert!(
            r.units.first().unwrap().removed.is_empty(),
            "ablation ignored steel shielding"
        );
    }

    #[test]
    fn cover_chips_but_protects_the_rear_unit_and_open_control_damages_it() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout.clone()).unwrap();
        let mut open = layout;
        open.blocks.clear();
        let mut b = BlastRange::with_layout(c.clone(), open).unwrap();
        for _ in 0..300 {
            a.step();
            b.step();
        }
        assert_eq!(
            a.impact, b.impact,
            "cover control changed the point of detonation"
        );
        assert!(a.removed_materials[1] > 5000 && a.removed_materials[2] > 100);
        assert_eq!(
            a.removed_materials[3], 0,
            "standard shot destroyed the steel core"
        );
        assert!(a.units[0].removed.len() > 1000);
        assert!(a.units[1].removed.is_empty() && a.units[1].status.alive);
        assert!(
            b.units[1].removed.len() > 1000,
            "sheltered body was just outside the blast"
        );
        for y in 0..60 {
            for z in 282..390 {
                assert_eq!(material(&a.world, [400, y, z]), 3, "hole through the core");
            }
        }
    }

    #[test]
    fn cover_layout_and_every_body_resume_during_damage() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout.clone()).unwrap();
        while a.units[0].removed.is_empty() {
            a.step();
        }
        let mut b = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
        for _ in 0..50 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        let mut slow = c;
        slow.wave_work_per_tick = 257;
        let mut other = BlastRange::with_layout(slow, layout).unwrap();
        for _ in 0..400 {
            other.step();
        }
        assert_eq!(a.removed_materials, other.removed_materials);
        for (a, b) in a.units.iter().zip(&other.units) {
            assert_eq!(a.body.vol.cols, b.body.vol.cols);
        }
    }

    #[test]
    fn range_rejects_embedded_units_and_accepts_alternative_body() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.units[0].at = [400, FLOOR, 336];
        assert!(BlastRange::with_layout(c.clone(), layout.clone()).is_err());
        layout.units[0].at = [359, FLOOR, 312];
        layout.units[0].kind = RangeUnitKind::Strider;
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        for _ in 0..200 {
            a.step();
        }
        assert!(!a.units[0].removed.is_empty());
        assert_eq!(
            a.state_hash(),
            BlastRange::restore(c, &a.snapshot()).unwrap().state_hash()
        );
    }
}
