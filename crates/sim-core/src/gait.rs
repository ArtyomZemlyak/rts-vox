//! Authoritative walker feet on the range (ADR-279).
//!
//! ADR-278 gave a walker's legs a damage volume in the rest pose while the
//! picture kept swinging them (`render::gait`, ADR-234/249/251): a stance
//! foot held its world contact, a swing foot flew to a predicted touchdown
//! over a clearance arc, and none of it was state, so a bolt aimed at a
//! lifted shin passed through the place where it was drawn. The range now
//! runs those rules in `Fx` once per tick: every foot's contact is
//! simulation state, saved and hashed; the links are posed by the same
//! two-link solve; hits land on the leg where it is. The renderer
//! interpolates the range's contacts between ticks and keeps its own
//! planting only for bodies whose simulation has no feet (the batch).
//!
//! The hull's supports stay the chassis' nominal points (ADR-280): a lost
//! leg drops its support, a foot in the air keeps it.
//!
//! Nothing here is parallel: a walker's feet are stepped in slot order
//! by the range, one sqrt per link per tick.

use crate::damage::Frame;
use fx::{angle, blob, Fx};
use phys::V3;
use voxel::unit::{Segment, UnitBody};

const ZERO: Fx = Fx::ZERO;
const ONE: Fx = Fx::ONE;
const MILLI: Fx = Fx::from_ratio(1, 1000);
const MAX_CLEARANCE: Fx = Fx::from_int(1000);
const X: V3 = V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO);
const Z: V3 = V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE);

/// Gait numbers in world units and binary fractions of the cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gait {
    /// Hull path per full cycle.
    pub stride: Fx,
    /// Share of the cycle a foot stands, `0.05..=1`.
    pub duty: Fx,
    /// Swing height.
    pub lift: Fx,
    /// Phase offset per suspension support, binary angle.
    pub offset: [i32; 4],
}

impl Gait {
    /// From the chassis data: centimetres to world units through the world
    /// voxel edge, per cent to fractions, offsets to binary angles.
    pub fn of(g: &modding::tuning::Gait) -> Gait {
        let cm_per_unit = (i32::from(voxel::unit::WORLD_MM) / 10).max(1);
        Gait {
            stride: Fx::from_ratio(g.stride_cm, cm_per_unit),
            duty: Fx::from_ratio(g.duty_pct.clamp(5, 100), 100),
            lift: Fx::from_ratio(g.lift_cm, cm_per_unit),
            offset: g
                .offset_pct
                .map(|p| (i64::from(p.rem_euclid(100)) * i64::from(angle::FULL) / 100) as i32),
        }
    }
}

/// A unit vector, or the fallback for a vanishing one.
fn unit_or(v: V3, fallback: V3) -> V3 {
    let n = v.len_sq();
    if n <= Fx::from_ratio(1, 1_000_000) {
        fallback
    } else if n == ONE {
        // Bit-identical to scaling by ONE / sqrt(ONE).
        v
    } else {
        v.scale(ONE.div(n.sqrt()))
    }
}

/// Rotation about the vertical to a heading: `+X` goes to `(cos, 0, sin)`,
/// the hull's own convention (`damage::Frame::new`).
#[inline]
fn turned_cs(v: V3, head: i32, c: Fx, s: Fx) -> V3 {
    if head == 0 {
        return v;
    }
    V3::new(v.x.mul(c).sub(v.z.mul(s)), v.y, v.x.mul(s).add(v.z.mul(c)))
}

fn turned(v: V3, head: i32) -> V3 {
    if head == 0 {
        return v;
    }
    let (c, s) = (angle::cos(head), angle::sin(head));
    turned_cs(v, head, c, s)
}

/// Images of the axes under the shortest rotation taking unit `a` onto
/// unit `b` (Rodrigues: `w + v×w + v×(v×w)/(1+c)` with `v = a×b`,
/// `c = a·b`). Opposite vectors have no shortest arc and keep the axes.
fn rotation_arc(a: V3, b: V3) -> [V3; 3] {
    let v = a.cross(b);
    if v==V3::ZERO {return [X,V3::UP,Z];}
    let k = ONE.add(a.dot(b));
    if k <= Fx::from_ratio(1, 64) {
        return [X, V3::UP, Z];
    }
    let inv = ONE.div(k);
    let apply = |w: V3| {
        let vw = v.cross(w);
        w.add(vw).add(v.cross(vw).scale(inv))
    };
    [apply(X), apply(V3::UP), apply(Z)]
}

/// `m` applied to `w`, where `m` holds the images of the axes.
fn apply(m: &[V3; 3], w: V3) -> V3 {
    m[0].scale(w.x).add(m[1].scale(w.y)).add(m[2].scale(w.z))
}

/// Hull-local offset (world units along the hull's axes) to the world.
fn to_world(root: &Frame, v: V3) -> V3 {
    root.base
        .add(root.x.scale(v.x))
        .add(root.y.scale(v.y))
        .add(root.z.scale(v.z))
}

/// World point to a hull-local offset.
fn to_local(root: &Frame, w: V3) -> V3 {
    let d = w.sub(root.base);
    V3::new(d.dot(root.x), d.dot(root.y), d.dot(root.z))
}

/// Where a sole stands: the ankle point in the world, the sole's heading
/// and its normal in the heading frame (`x` along the heading, `z` across).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    pub pos: V3,
    pub head: i32,
    pub normal: V3,
}

impl Default for Contact {
    fn default() -> Self {
        Contact {
            pos: V3::ZERO,
            head: 0,
            normal: V3::UP,
        }
    }
}

impl Contact {
    /// World axes of the sole: the heading laid onto the sole plane, the
    /// normal, and across.
    #[cfg(test)]
    fn legacy20_axes(&self) -> [V3; 3] {
        let m = rotation_arc(V3::UP, self.normal);
        [
            turned(m[0], self.head),
            turned(m[1], self.head),
            turned(m[2], self.head),
        ]
    }

    pub fn axes(&self) -> [V3; 3] {
        if self.normal == V3::UP {
            if self.head == 0 {
                return [X, V3::UP, Z];
            }
            let (c, s) = (angle::cos(self.head), angle::sin(self.head));
            return [V3::new(c, ZERO, s), V3::UP, V3::new(ZERO.sub(s), ZERO, c)];
        }
        let m = rotation_arc(V3::UP, self.normal);
        if self.head == 0 {
            return m;
        }
        let (c, s) = (angle::cos(self.head), angle::sin(self.head));
        [
            turned_cs(m[0], self.head, c, s),
            turned_cs(m[1], self.head, c, s),
            turned_cs(m[2], self.head, c, s),
        ]
    }
    /// Between two contacts: the point on the segment, the heading along
    /// the shorter arc, the normal renormalised.
    pub fn lerp(self, o: Contact, t: Fx) -> Contact {
        // A committed sole is already fitted. Renormalizing its endpoint
        // changes the support by a few fixed-point bits at touchdown.
        if t<=ZERO {return self;}
        if t>=ONE {return o;}
        Contact {
            pos: self.pos.add(o.pos.sub(self.pos).scale(t)),
            head: self.head.wrapping_add(
                Fx::from_int(angle::diff(o.head, self.head))
                    .mul(t)
                    .to_int_trunc(),
            ),
            normal: unit_or(self.normal.add(o.normal.sub(self.normal).scale(t)), V3::UP),
        }
    }
    fn save(&self, b: &mut blob::Blob) {
        b.fx(self.pos.x);
        b.fx(self.pos.y);
        b.fx(self.pos.z);
        b.i32(self.head);
        b.fx(self.normal.x);
        b.fx(self.normal.y);
        b.fx(self.normal.z);
    }
    fn load(s: &mut blob::Scan) -> Option<Contact> {
        let pos = V3::new(s.fx(), s.fx(), s.fx());
        let head = s.i32();
        let normal = V3::new(s.fx(), s.fx(), s.fx());
        let n = normal.len_sq();
        let bounded = |v: Fx| v.abs() < Fx::from_int(1 << 20);
        if !s.ok()
            || n < Fx::from_ratio(1, 2)
            || n > Fx::from_int(2)
            || normal.y <= ZERO
            || !bounded(pos.x)
            || !bounded(pos.y)
            || !bounded(pos.z)
        {
            return None;
        }
        Some(Contact { pos, head, normal })
    }
}

/// Representative bottom cells of a foot, relative to the ankle in the
/// sole's own frame. Empty space between toes must not act as a support.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sole {
    probes: Vec<V3>,
}

impl Sole {
    /// A single probe this far below the ankle.
    pub fn point(depth: Fx) -> Sole {
        Sole {
            probes: vec![V3::new(ZERO, depth.neg(), ZERO)],
        }
    }

    /// Up to nine occupied bottom cells of the segment nearest to a 3×3
    /// grid over its footprint, the renderer's choice (ADR-251); ties go
    /// to the first cell in scan order.
    pub fn of(segment: &Segment) -> Sole {
        let b = &segment.body;
        let k = Fx::from_ratio(i32::from(b.mm), i32::from(voxel::unit::WORLD_MM));
        let a = &segment.attach;
        let pivot = V3::new(
            Fx::from_int(i32::from(a.pivot.0)),
            Fx::from_int(i32::from(a.pivot.1)),
            Fx::from_int(i32::from(a.pivot.2)),
        );
        let half = Fx::from_ratio(1, 2);
        let mut bottom = Vec::new();
        for z in 0..b.vol.nz as usize {
            for x in 0..b.vol.nx as usize {
                if let Some(y) = (0..b.vol.ny as usize).find(|&y| b.vol.get(x, y, z)) {
                    bottom.push(V3::new(
                        Fx::from_int(x as i32).add(half).sub(pivot.x).mul(k),
                        Fx::from_int(y as i32).sub(pivot.y).mul(k),
                        Fx::from_int(z as i32).add(half).sub(pivot.z).mul(k),
                    ));
                }
            }
        }
        let Some(first) = bottom.first().copied() else {
            return Sole::point(pivot.y.mul(k));
        };
        let (mut lo, mut hi) = (first, first);
        for p in &bottom {
            lo = V3::new(lo.x.min(p.x), lo.y, lo.z.min(p.z));
            hi = V3::new(hi.x.max(p.x), hi.y, hi.z.max(p.z));
        }
        let mut probes = Vec::new();
        for zi in 0..3 {
            for xi in 0..3 {
                let want = [
                    lo.x.add(hi.x.sub(lo.x).mul(Fx::from_ratio(xi, 2))),
                    lo.z.add(hi.z.sub(lo.z).mul(Fx::from_ratio(zi, 2))),
                ];
                let distance = |v: &V3| {
                    let (dx, dz) = (v.x.sub(want[0]), v.z.sub(want[1]));
                    dx.mul(dx).add(dz.mul(dz))
                };
                let mut best = first;
                let mut nearest = distance(&first);
                for p in &bottom[1..] {
                    let d = distance(p);
                    if d < nearest {
                        nearest = d;
                        best = *p;
                    }
                }
                if !probes.contains(&best) {
                    probes.push(best);
                }
            }
        }
        Sole { probes }
    }

    pub fn probes(&self) -> &[V3] {
        &self.probes
    }
    pub(crate) fn width(&self) -> Fx {
        let extent = |axis: fn(&V3)->Fx| {
            let lo=self.probes.iter().map(axis).min().unwrap_or(ZERO);
            let hi=self.probes.iter().map(axis).max().unwrap_or(ZERO);
            hi.sub(lo)
        };
        extent(|p|p.x).min(extent(|p|p.z))
    }

    /// Fit a plane over the sole at `at` with the given heading, then lift
    /// the ankle until every probe clears the terrain. The sole tilts on
    /// its own, whatever the hull does; a cliff cannot turn it vertical.
    #[cfg(test)]
    pub fn legacy20_contact(&self, at: V3, head: i32, floor: &dyn Fn(Fx, Fx) -> Fx) -> Contact {
        let n = Fx::from_int(self.probes.len() as i32);
        let mut heights = [ZERO; 9];
        let heights = &mut heights[..self.probes.len()];
        let mut mean = V3::ZERO;
        for (p, sample) in self.probes.iter().zip(heights.iter_mut()) {
            let w = at.add(turned(*p, head));
            let h = floor(w.x, w.z);
            *sample = h;
            mean = mean.add(V3::new(p.x, h, p.z));
        }
        let mean = mean.scale(ONE.div(n));
        let (mut xx, mut xz, mut zz, mut xy, mut zy) = (ZERO, ZERO, ZERO, ZERO, ZERO);
        for (p, h) in self.probes.iter().zip(heights.iter()) {
            let (x, z, y) = (p.x.sub(mean.x), p.z.sub(mean.z), h.sub(mean.y));
            xx = xx.add(x.mul(x));
            xz = xz.add(x.mul(z));
            zz = zz.add(z.mul(z));
            xy = xy.add(x.mul(y));
            zy = zy.add(z.mul(y));
        }
        let det = xx.mul(zz).sub(xz.mul(xz));
        let normal = if det > Fx::from_ratio(1, 1_000_000) {
            let mut sx = xy.mul(zz).sub(zy.mul(xz)).div(det);
            let mut sz = zy.mul(xx).sub(xy.mul(xz)).div(det);
            let l2 = sx.mul(sx).add(sz.mul(sz));
            if l2 > ONE {
                let l = l2.sqrt();
                sx = sx.div(l);
                sz = sz.div(l);
            }
            unit_or(V3::new(sx.neg(), ONE, sz.neg()), V3::UP)
        } else {
            V3::UP
        };
        Contact {
            pos: V3::new(at.x, if normal == V3::UP {
                // Rotation about UP preserves probe heights and sample XZ:
                // these are exactly the first pass's terrain queries.
                self.probes.iter().zip(heights.iter()).map(|(p, h)| h.sub(p.y))
                    .max().unwrap_or(at.y)
            } else { self.legacy20_height(at, head, normal, floor) }, at.z),
            head,
            normal,
        }
    }

    pub fn contact(&self, at: V3, head: i32, floor: &dyn Fn(Fx, Fx) -> Fx) -> Contact {
        self.contact_with_flat(at, head, floor).0
    }

    fn contact_with_flat(
        &self,
        at: V3,
        head: i32,
        floor: &dyn Fn(Fx, Fx) -> Fx,
    ) -> (Contact, Fx) {
        let (c, s) = if head == 0 {
            (ONE, ZERO)
        } else {
            (angle::cos(head), angle::sin(head))
        };
        let n = Fx::from_int(self.probes.len() as i32);
        let mut heights = [ZERO; 9];
        let heights = &mut heights[..self.probes.len()];
        let mut mean = V3::ZERO;
        for (p, sample) in self.probes.iter().zip(heights.iter_mut()) {
            let w = at.add(turned_cs(*p, head, c, s));
            let h = floor(w.x, w.z);
            *sample = h;
            mean = mean.add(V3::new(p.x, h, p.z));
        }
        let mean = mean.scale(ONE.div(n));
        let (mut xx, mut xz, mut zz, mut xy, mut zy) = (ZERO, ZERO, ZERO, ZERO, ZERO);
        for (p, h) in self.probes.iter().zip(heights.iter()) {
            let (x, z, y) = (p.x.sub(mean.x), p.z.sub(mean.z), h.sub(mean.y));
            xx = xx.add(x.mul(x));
            xz = xz.add(x.mul(z));
            zz = zz.add(z.mul(z));
            xy = xy.add(x.mul(y));
            zy = zy.add(z.mul(y));
        }
        let det = xx.mul(zz).sub(xz.mul(xz));
        let normal = if det > Fx::from_ratio(1, 1_000_000) {
            let mut sx = xy.mul(zz).sub(zy.mul(xz)).div(det);
            let mut sz = zy.mul(xx).sub(xy.mul(xz)).div(det);
            let l2 = sx.mul(sx).add(sz.mul(sz));
            if l2 > ONE {
                let l = l2.sqrt();
                sx = sx.div(l);
                sz = sz.div(l);
            }
            unit_or(V3::new(sx.neg(), ONE, sz.neg()), V3::UP)
        } else {
            V3::UP
        };
        let flat_y = self
            .probes
            .iter()
            .zip(heights.iter())
            .map(|(p, h)| h.sub(p.y))
            .max()
            .unwrap_or(at.y);
        let y = if normal == V3::UP {
            flat_y
        } else {
            self.height_cs(at, head, c, s, normal, floor)
        };
        (
            Contact {
                pos: V3::new(at.x, y, at.z),
                head,
                normal,
            },
            flat_y,
        )
    }

    /// Ankle height at which no probe of the tilted sole is below the terrain.
    #[cfg(test)]
    fn legacy20_height(&self, at: V3, head: i32, normal: V3, floor: &dyn Fn(Fx, Fx) -> Fx) -> Fx {
        if normal==V3::UP {
            // The original rotation is exactly the identity. Keep every
            // occupied probe at the identical XZ, without its matrix products.
            return self.probes.iter().map(|p| {
                let offset=turned(*p,head);
                floor(at.x.add(offset.x),at.z.add(offset.z)).sub(offset.y)
            }).max().unwrap_or(at.y);
        }
        let m = rotation_arc(V3::UP, normal);
        let mut best: Option<Fx> = None;
        for p in &self.probes {
            let offset = turned(apply(&m, *p), head);
            let h = floor(at.x.add(offset.x), at.z.add(offset.z)).sub(offset.y);
            best = Some(best.map_or(h, |b| b.max(h)));
        }
        best.unwrap_or(at.y)
    }

    fn height(&self, at: V3, head: i32, normal: V3, floor: &dyn Fn(Fx, Fx) -> Fx) -> Fx {
        let (c, s) = if head == 0 {
            (ONE, ZERO)
        } else {
            (angle::cos(head), angle::sin(head))
        };
        self.height_cs(at, head, c, s, normal, floor)
    }

    fn height_cs(
        &self,
        at: V3,
        head: i32,
        c: Fx,
        s: Fx,
        normal: V3,
        floor: &dyn Fn(Fx, Fx) -> Fx,
    ) -> Fx {
        if normal == V3::UP {
            return self
                .probes
                .iter()
                .map(|p| {
                    let offset = turned_cs(*p, head, c, s);
                    floor(at.x.add(offset.x), at.z.add(offset.z)).sub(offset.y)
                })
                .max()
                .unwrap_or(at.y);
        }
        let m = rotation_arc(V3::UP, normal);
        let mut best: Option<Fx> = None;
        for p in &self.probes {
            let offset = turned_cs(apply(&m, *p), head, c, s);
            let h = floor(at.x.add(offset.x), at.z.add(offset.z)).sub(offset.y);
            best = Some(best.map_or(h, |b| b.max(h)));
        }
        best.unwrap_or(at.y)
    }

    /// Terrain-sized lift from bounded, footprint-scaled samples. The live
    /// sole projection handles voxel edges; an edge near a zero-slope arc
    /// endpoint must not amplify the whole step through division by ~zero.
    pub fn clearance(
        &self,
        from: Contact,
        to: Contact,
        lift: Fx,
        floor: &dyn Fn(Fx, Fx) -> Fx,
    ) -> Fx {
        // A large sole spans many world cells. Features smaller than a
        // quarter of its width do not need individual arc samples; the
        // current footprint still checks actual terrain on every tick.
        let pitch=self.width().mul(Fx::from_ratio(1,4)).max(ONE);
        let d = to.pos.sub(from.pos).len_sq().sqrt().div(pitch);
        let whole = d.to_int_trunc();
        let samples = if Fx::from_int(whole) < d {
            whole + 1
        } else {
            whole
        }
        .clamp(if pitch>ONE {4} else {8}, if pitch>ONE {12} else {32});
        let mid_y = from.pos.y.add(to.pos.y).mul(Fx::from_ratio(1, 2));
        let climbing = to.pos.y > from.pos.y;
        let mut height = lift;
        for i in 1..samples {
            let q = Fx::from_ratio(i, samples);
            let eased = q.mul(q).mul(Fx::from_int(3).sub(q.mul(Fx::from_int(2))));
            let at = from.lerp(to, eased);
            // When climbing onto a ledge, the chord already supplies half the
            // step rise by midpoint; do not add that lower-half rise again on
            // top of the apex and launch the ankle above its knee.
            let base_y = if climbing { at.pos.y.max(mid_y) } else { at.pos.y };
            let need = self.height(at.pos, at.head, at.normal, floor).sub(base_y);
            height = height.max(need.add(lift));
        }
        height
    }
}

/// A leg computed once from the body: rest joints in hull-local world
/// units, link lengths, bend direction, segment indices and the sole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegRig {
    /// Cached non-overlapping ankle corridors, derived from neighbouring legs.
    pub lane: LegLane,
    pub hip: V3,
    pub knee: V3,
    pub ankle: V3,
    pub l1: Fx,
    pub l2: Fx,
    /// Authored knee pole, including its vertical component. Flattening it
    /// reverses the IK branch when a raised ankle crosses hip height.
    pub bend: V3,
    /// Rest hip-to-ankle axis, cached for transporting the authored hinge plane.
    pub rest_axis: V3,
    pub thigh: usize,
    pub shin: usize,
    pub knee_ball: Option<usize>,
    pub ankle_ball: Option<usize>,
    pub foot: Option<usize>,
    /// Which suspension support the leg answers to.
    pub support: usize,
    /// Individual phase relative to that support, in binary turns.
    pub phase_shift: i32,
    pub sole: Sole,
    retracted: ([V3; 6], usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegLane {
    pub min: [Fx;2],
    pub max: [Fx;2],
    pub head: i32,
}
impl Default for LegLane {
    fn default() -> Self {Self {min:[Fx::MIN;2],max:[Fx::MAX;2],head:0}}
}
impl LegRig {
    pub(crate) fn contact_reachable(&self,target:V3)->bool {
        let d=target.sub(self.hip).len_sq();
        let near=self.l1.sub(self.l2).abs().max(MILLI);
        let far=self.l1.add(self.l2).mul(Fx::from_ratio(999,1000));
        d>=near.mul(near) && d<=far.mul(far)
    }
    pub(crate) fn lane_target(&self,mut p:V3) -> V3 {
        p.x=p.x.max(self.lane.min[0]).min(self.lane.max[0]);
        p.z=p.z.max(self.lane.min[1]).min(self.lane.max[1]);
        p
    }
    /// A foothold must be reachable from the walker's rest plane by a
    /// continuous ramp or steps no taller than `max_step_cm`, with no
    /// unclimbable sheer wall standing between the hull and the sole.
    pub(crate) fn step_climbable(
        &self,
        root: &Frame,
        contact: Contact,
        floor: &dyn Fn(Fx, Fx) -> Fx,
        profile: modding::walking_terrain::Capability,
    ) -> bool {
        let rest_ankle_y = root.base.y.add(self.ankle.y);
        let max_step = Fx::from_ratio(
            profile.max_step_cm,
            (i32::from(voxel::unit::WORLD_MM) / 10).max(1),
        );
        if contact.pos.y.sub(rest_ankle_y) <= max_step {
            return true;
        }
        let axes = contact.axes();
        let min_probe_y = self
            .sole
            .probes
            .iter()
            .map(|p| {
                axes[0]
                    .y
                    .mul(p.x)
                    .add(axes[1].y.mul(p.y))
                    .add(axes[2].y.mul(p.z))
            })
            .min()
            .unwrap_or(ZERO);
        self.step_climbable_surface(
            root,
            contact.pos,
            contact.pos.y.add(min_probe_y),
            rest_ankle_y,
            max_step,
            floor,
        )
    }
    pub(crate) fn step_sole_climbable(
        &self,
        root: &Frame,
        contact: Contact,
        floor: &dyn Fn(Fx, Fx) -> Fx,
        profile: modding::walking_terrain::Capability,
    ) -> bool {
        if !self.step_climbable(root, contact, floor, profile) {
            return false;
        }
        let flat_y = self.sole.height(contact.pos, contact.head, V3::UP, floor);
        let raised = Contact {
            pos: V3::new(contact.pos.x, flat_y, contact.pos.z),
            normal: V3::UP,
            ..contact
        };
        self.step_climbable(root, raised, floor, profile)
    }
    pub(crate) fn step_climbable_to(
        &self,
        root: &Frame,
        at: V3,
        surface_y: Fx,
        floor: &dyn Fn(Fx, Fx) -> Fx,
        profile: modding::walking_terrain::Capability,
    ) -> bool {
        let rest_ankle_y = root.base.y.add(self.ankle.y);
        let max_step = Fx::from_ratio(
            profile.max_step_cm,
            (i32::from(voxel::unit::WORLD_MM) / 10).max(1),
        );
        let rest_sole_depth = self
            .sole
            .probes
            .iter()
            .map(|p| p.y.neg())
            .max()
            .unwrap_or(ZERO);
        if surface_y.sub(rest_ankle_y.sub(rest_sole_depth)) <= max_step {
            return true;
        }
        self.step_climbable_surface(root, at, surface_y, rest_ankle_y, max_step, floor)
    }
    fn step_climbable_surface(
        &self,
        root: &Frame,
        at: V3,
        surface_y: Fx,
        rest_ankle_y: Fx,
        max_step: Fx,
        floor: &dyn Fn(Fx, Fx) -> Fx,
    ) -> bool {
        let rest_sole_depth = self
            .sole
            .probes
            .iter()
            .map(|p| p.y.neg())
            .max()
            .unwrap_or(ZERO);
        let mut high = rest_ankle_y
            .sub(rest_sole_depth)
            .max(floor(root.base.x, root.base.z));
        if surface_y.sub(high) <= max_step {
            return true;
        }
        for k in 1..=4 {
            let t = Fx::from_ratio(k, 5);
            let sx = root.base.x.add(at.x.sub(root.base.x).mul(t));
            let sz = root.base.z.add(at.z.sub(root.base.z).mul(t));
            let h = floor(sx, sz);
            if h > high && h.sub(high) <= max_step {
                high = h;
            }
        }
        surface_y.sub(high) <= max_step
    }
    pub(crate) fn sole_clearance_checked(
        &self,
        root: &Frame,
        from: Contact,
        to: Contact,
        lift: Fx,
        floor: &dyn Fn(Fx, Fx) -> Fx,
        profile: Option<modding::walking_terrain::Capability>,
    ) -> Fx {
        let Some(profile) = profile else {
            return self.sole.clearance(from, to, lift, floor);
        };
        let pitch = self.sole.width().mul(Fx::from_ratio(1, 4)).max(ONE);
        let d = to.pos.sub(from.pos).len_sq().sqrt().div(pitch);
        let whole = d.to_int_trunc();
        let samples = if Fx::from_int(whole) < d {
            whole + 1
        } else {
            whole
        }
        .clamp(if pitch > ONE { 4 } else { 8 }, if pitch > ONE { 12 } else { 32 });
        let mid_y = from.pos.y.add(to.pos.y).mul(Fx::from_ratio(1, 2));
        let climbing = to.pos.y > from.pos.y;
        let mut height = lift;
        for i in 1..samples {
            let q = Fx::from_ratio(i, samples);
            let eased = q.mul(q).mul(Fx::from_int(3).sub(q.mul(Fx::from_int(2))));
            let at = from.lerp(to, eased);
            let sample_h = self.sole.height(at.pos, at.head, at.normal, floor);
            let raised = Contact {
                pos: V3::new(at.pos.x, sample_h, at.pos.z),
                ..at
            };
            if !self.step_sole_climbable(root, raised, floor, profile) {
                continue;
            }
            let base_y = if climbing { at.pos.y.max(mid_y) } else { at.pos.y };
            height = height.max(sample_h.sub(base_y).add(lift));
        }
        height
    }
    pub(crate) fn retracted_candidates(&self) -> ([V3; 6], usize) {
        self.retracted
    }
    fn compute_retracted_candidates(&self) -> ([V3; 6], usize) {
        let rest = V3::new(self.ankle.x.sub(self.hip.x), ZERO, self.ankle.z.sub(self.hip.z));
        let rest_shin = self.ankle.sub(self.knee);
        let min_shin_dot = rest_shin.len_sq().mul(Fx::from_ratio(1, 16));
        let lift_margin = self.l1.add(self.l2).mul(Fx::from_ratio(1, 6));
        let shin_forward = |p: V3| {
            let pose = self.solve_contact(p);
            pose.ankle.sub(pose.knee).dot(self.rest_axis) > Fx::from_ratio(1, 8)
        };
        let shin_unreversed = |p: V3| {
            let pose = self.solve_contact(p);
            let shin = pose.ankle.sub(pose.knee);
            shin.y < Fx::from_int(-2) && shin.dot(rest_shin) > min_shin_dot
        };
        let mut out = [self.ankle; 6];
        let mut len = 0;
        for (idx, (num, den)) in [(3, 4), (5, 8), (1, 2), (2, 5), (1, 4), (0, 1)]
            .into_iter()
            .enumerate()
        {
            let k = Fx::from_ratio(num, den);
            let cand = self.lane_target(V3::new(
                self.hip.x.add(rest.x.mul(k)),
                self.ankle.y,
                self.hip.z.add(rest.z.mul(k)),
            ));
            let lifted = V3::new(cand.x, cand.y.add(lift_margin), cand.z);
            let ok = if idx < 4 {
                shin_forward(cand) && shin_forward(lifted)
            } else {
                self.contact_reachable(lifted) && shin_unreversed(cand) && shin_unreversed(lifted)
            };
            if self.contact_reachable(cand) && ok {
                out[len] = cand;
                len += 1;
            }
        }
        (out, len)
    }
    pub(crate) fn retracted_neutral(
        &self,
        root: &Frame,
        head: i32,
        floor: &dyn Fn(Fx, Fx) -> Fx,
        profile: modding::walking_terrain::Capability,
    ) -> Contact {
        let neutral = self.sole.contact(
            to_world(root, self.lane_target(self.ankle)),
            head,
            floor,
        );
        if self.step_sole_climbable(root, neutral, floor, profile) {
            return neutral;
        }
        let (cands, len) = self.retracted_candidates();
        if let Some(retracted) = cands[..len]
            .iter()
            .map(|&local| self.sole.contact(to_world(root, local), head, floor))
            .find(|&c| self.step_sole_climbable(root, c, floor, profile))
        {
            let mut good = retracted;
            let mut bad = neutral;
            for _ in 0..8 {
                let middle = self.sole.contact(
                    good.pos.add(bad.pos).scale(Fx::from_ratio(1, 2)),
                    head,
                    floor,
                );
                if self.step_sole_climbable(root, middle, floor, profile) {
                    good = middle;
                } else {
                    bad = middle;
                }
            }
            return good;
        }
        if let Some(retracted) = cands[..len]
            .iter()
            .map(|&local| self.sole.contact(to_world(root, local), head, floor))
            .find(|&c| self.step_climbable(root, c, floor, profile))
        {
            let mut good = retracted;
            let mut bad = neutral;
            for _ in 0..8 {
                let middle = self.sole.contact(
                    good.pos.add(bad.pos).scale(Fx::from_ratio(1, 2)),
                    head,
                    floor,
                );
                if self.step_climbable(root, middle, floor, profile) {
                    good = middle;
                } else {
                    bad = middle;
                }
            }
            return good;
        }
        neutral
    }
}

/// Joints of a posed leg, hull-local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegPose {
    pub knee: V3,
    pub ankle: V3,
}

/// The legs of a body, in template order.
pub fn rigs_of(b: &UnitBody) -> Vec<LegRig> {
    let k = Fx::from_ratio(i32::from(b.mm), i32::from(voxel::unit::WORLD_MM));
    let centre = [
        Fx::from_ratio(i32::from(b.vol.nx), 2),
        ZERO,
        Fx::from_ratio(i32::from(b.vol.nz), 2),
    ];
    let at = |p: (i16, i16, i16)| {
        V3::new(
            Fx::from_int(i32::from(p.0)).sub(centre[0]).mul(k),
            Fx::from_int(i32::from(p.1)).sub(centre[1]).mul(k),
            Fx::from_int(i32::from(p.2)).sub(centre[2]).mul(k),
        )
    };
    let mut rigs: Vec<LegRig> = b.legs
        .iter()
        .map(|l| {
            let (hip, knee, ankle) = (at(l.hip), at(l.knee), at(l.ankle));
            let along = unit_or(ankle.sub(hip), V3::ZERO);
            let off = knee.sub(hip);
            let side = off.sub(along.scale(off.dot(along)));
            let bend = if side.len_sq() > Fx::from_ratio(1, 1_000_000) {
                unit_or(side, X)
            } else {
                unit_or(V3::new(hip.x, ZERO, hip.z), X)
            };
            LegRig {
                lane: LegLane::default(),
                hip,
                knee,
                ankle,
                l1: knee.sub(hip).len_sq().sqrt().max(MILLI),
                l2: ankle.sub(knee).len_sq().sqrt().max(MILLI),
                bend,
                rest_axis: along,
                thigh: l.thigh as usize,
                shin: l.shin as usize,
                knee_ball: l.knee_ball.map(usize::from),
                ankle_ball: l.ankle_ball.map(usize::from),
                foot: l.foot.map(usize::from),
                support: usize::from(l.support),
                phase_shift: (i64::from(l.phase_shift_pct) * i64::from(angle::FULL) / 100) as i32,
                sole: l
                    .foot
                    .and_then(|i| b.segments.get(usize::from(i)))
                    .map_or_else(|| Sole::point(ankle.y), Sole::of),
                retracted: ([ankle; 6], 0),
            }
        })
        .collect();
    // A foot occupies a volume, not just its ankle. Reserve each side of
    // the nearest rest-pose bisector once, before running any gait ticks.
    let extent=|r:&LegRig| {
        let mut out=[ZERO;2];
        if let Some(foot)=r.foot.and_then(|i| b.segments.get(i)) {
            let pivot=foot.attach.pivot;
            let unit=Fx::from_ratio(i32::from(foot.body.mm),i32::from(voxel::unit::WORLD_MM));
            for (i,(size,p)) in [(foot.body.vol.nx,pivot.0),(foot.body.vol.nz,pivot.2)].into_iter().enumerate() {
                out[i]=Fx::from_int(i32::from(p).max(i32::from(size)-i32::from(p))).mul(unit);
            }
        }
        out
    };
    let mut extents=rigs.iter().map(extent).collect::<Vec<_>>();
    for i in 0..rigs.len() {
        // Crowded rows of radial legs put the long toe outwards, rather
        // than along the row into the next foot's planting area.
        if rigs.iter().enumerate().any(|(j,r)| j!=i && r.ankle.z==rigs[i].ankle.z
            && r.ankle.x.sub(rigs[i].ankle.x).abs()<=extents[i][0].add(extents[j][0])) {
            rigs[i].lane.head=if rigs[i].ankle.z<ZERO {-angle::QUARTER} else {angle::QUARTER};
        }
    }
    for (r,e) in rigs.iter().zip(&mut extents) {if r.lane.head!=0 {e.swap(0,1);}}
    for i in 0..rigs.len() {for j in 0..rigs.len() {
        if i==j {continue;}
        let a=[rigs[i].ankle.x,rigs[i].ankle.z];
        let z=[rigs[j].ankle.x,rigs[j].ankle.z];
        let d=[z[0].sub(a[0]),z[1].sub(a[1])];
        let axis=usize::from(d[1].abs()>d[0].abs());
        let middle=a[axis].add(z[axis]).mul(Fx::from_ratio(1,2));
        let radius=extents[i][axis].min(d[axis].abs().mul(Fx::from_ratio(1,2)));
        if d[axis]>ZERO {rigs[i].lane.max[axis]=rigs[i].lane.max[axis].min(middle.sub(radius));}
        if d[axis]<ZERO {rigs[i].lane.min[axis]=rigs[i].lane.min[axis].max(middle.add(radius));}
    }}
    for rig in &mut rigs {
        rig.retracted = rig.compute_retracted_candidates();
    }
    rigs
}

/// A complete stance may not consume more than the available foot lane.
/// This is geometry, not a change to the drive speed or a per-tick search.
pub(crate) fn lane_stride(rigs:&[LegRig],duty:Fx) -> Option<Fx> {
    let mut room=None;
    for r in rigs {
        for (bound,finite) in [(r.ankle.x.sub(r.lane.min[0]),r.lane.min[0]!=Fx::MIN),
            (r.lane.max[0].sub(r.ankle.x),r.lane.max[0]!=Fx::MAX)] {
            if finite && bound>ZERO {room=Some(room.map_or(bound,|v:Fx|v.min(bound)));}
        }
    }
    room.map(|v|v.mul(Fx::from_int(2)).div(duty.max(MILLI)))
}

impl LegRig {
    /// Two-link inverse kinematics in the hull frame: the knee where the
    /// thigh's circle meets the shin's, on the bend side; an unreachable
    /// target is pulled to the leg's reach.
    pub fn solve(&self, target: V3) -> LegPose {
        self.solve_contact(self.lane_target(target))
    }

    /// A committed world contact is an IK target, not a new placement request.
    /// Lane limits schedule the next step; clamping here would drag a stance
    /// or an interpolated swing without changing its authoritative contact.
    pub fn solve_contact(&self, target: V3) -> LegPose {
        let (u,dist)=self.contact_axis(target);
        let a = self
            .l1
            .mul(self.l1)
            .sub(self.l2.mul(self.l2))
            .add(dist.mul(dist))
            .div(dist.mul(Fx::from_int(2)));
        let h = self.l1.mul(self.l1).sub(a.mul(a)).max(ZERO).sqrt();
        // Transport the authored plane with the leg axis. Projecting a fixed
        // pole alone switches sides when a lifted ankle passes that pole.
        let k=ONE.add(self.rest_axis.dot(u));
        let pole=if k>Fx::from_ratio(1,64) {
            self.bend.sub(self.rest_axis.add(u).scale(self.bend.dot(u).div(k)))
        } else {self.bend};
        let n = unit_or(pole.sub(u.scale(pole.dot(u))), V3::UP);
        LegPose {
            knee: self.hip.add(u.scale(a)).add(n.scale(h)),
            ankle: self.hip.add(u.scale(dist)),
        }
    }

    /// Exact endpoint of the same contact IK. Touchdown admission only needs
    /// this endpoint; physical segment frames still solve the complete knee.
    pub(crate) fn contact_ankle(&self,target:V3)->V3 {
        let (u,dist)=self.contact_axis(target);
        self.hip.add(u.scale(dist))
    }

    #[inline]
    fn contact_axis(&self,target:V3)->(V3,Fx) {
        let d = target.sub(self.hip);
        let len = d.len_sq().sqrt();
        let dist = len
            .max(self.l1.sub(self.l2).abs().max(MILLI))
            .min(self.l1.add(self.l2).mul(Fx::from_ratio(999, 1000)));
        let u = if len > MILLI {
            d.scale(ONE.div(len))
        } else {
            V3::new(ZERO, ONE.neg(), ZERO)
        };
        (u,dist)
    }
}

/// Unit lift with zero vertical slope at take-off and touchdown. The
/// clearance proof and the executed swing must use the identical profile.
fn swing_arc(q: Fx) -> Fx {
    let q = q.max(ZERO).min(ONE);
    let edge = q.mul(ONE.sub(q));
    edge.mul(edge).mul(Fx::from_int(16))
}

/// The hull as a foot sees it on a tick.
pub struct Stance<'a> {
    /// The suspension carries load: flight keeps the legs where they were.
    pub grounded: bool,
    pub root: &'a Frame,
    /// Hull heading, binary angle.
    pub head: i32,
    /// Step phase, binary angle.
    pub phase: i32,
    /// Share of the lift a slow walk uses, `0..=1`.
    pub lift_scale: Fx,
    /// World displacement forecast. Normally accepted motion; a support wait
    /// retains the requested motion so landing opens the next body step.
    pub linear: V3,
    /// Yaw forecast for the step, binary angle.
    pub angular: i32,
    /// Phase advance this tick, as a fraction of the cycle.
    pub cycles: Fx,
    pub gait: &'a Gait,
    /// Terrain height under a world point.
    pub floor: &'a dyn Fn(Fx, Fx) -> Fx,
}

/// Bounded diagnostic reasons for proposals to change a world anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorReason { Reset, LaneFit, Reproject, SideGuard, LandingBlocked, LandingRejected, SupportRecovery,
    SupportReady, SupportChanged, SupportMoving, SupportRemoved, SupportRejected, SupportAlternative,
    /// A future landing target is bounded; neither endpoint is a planted anchor.
    GoalEnvelope }

#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum Readiness { #[default] Pending, Ready, Rejected }
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum SupportReason { #[default] Unchecked, Supported, SourceChanged, PartialSole, Moving, Removed, Unsupported, Obstructed, Unreachable, Normal }
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub struct SupportEvidence {
    pub readiness:Readiness,
    pub reason:SupportReason,
    pub source:Option<nav::vehicle::support::SupportRef>,
    /// Ordered evidence includes every occupied probe, including mixed sources.
    pub signature:u64,
    pub supported_mask:u16,
    pub samples:u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorEvent {
    pub leg: usize,
    pub reason: AnchorReason,
    pub before: Option<Contact>,
    pub proposed: Contact,
    pub after: Option<Contact>,
    pub planted_before: bool,
    pub planted_after: bool,
}

/// Opt-in diagnostic scratch. It never enters a foot's saved/hashed state.
/// A full buffer drops events instead of allocating an unbounded transcript.
#[derive(Clone, Debug, Default)]
pub struct AnchorTrace {
    pub leg: usize,
    events: Vec<AnchorEvent>,
    pub dropped: u32,
    pub window_rejected: u32,
    pub window_limited: u32,
    pub window_unready: u32,
    pub window_reason:Option<&'static str>,
    /// Last requested motor input, observation only; absent for passive feet.
    pub motor_command:Option<(Fx,i32,Option<Fx>)>,
    /// Initial terrain/collision admission, captured only when tracing.
    pub motor_terrain:Option<[i64;9]>,
    /// First requested transition; alternatives do not erase its refusal.
    pub transition:Option<nav::vehicle::transition::TransitionProof>,
    /// Requested/support/executed cadence, root admission and contact alternative.
    pub motor_step:Option<[i64;5]>,
}
impl AnchorTrace {
    pub const LIMIT: usize = 32;
    pub fn events(&self) -> &[AnchorEvent] { &self.events }
    pub fn clear(&mut self) { self.events.clear(); self.dropped=0;self.window_rejected=0;self.window_limited=0;self.window_unready=0;self.window_reason=None;self.motor_command=None;self.motor_terrain=None;self.motor_step=None;self.transition=None; }
    pub(crate) fn window(&mut self,rejected:u32,limited:bool,unready:bool) {
        self.window_rejected=self.window_rejected.saturating_add(rejected);
        self.window_limited=self.window_limited.saturating_add(u32::from(limited));self.window_unready=self.window_unready.saturating_add(u32::from(unready));
    }
    pub(crate) fn append(&mut self,other:Self) {
        self.dropped=self.dropped.saturating_add(other.dropped);
        for e in other.events {
            self.leg=e.leg;
            self.record(e.reason,e.before,e.proposed,e.after,e.planted_before,e.planted_after);
        }
    }

    pub(crate) fn record(&mut self, reason: AnchorReason, before: Option<Contact>, proposed: Contact,
        after: Option<Contact>, planted_before: bool, planted_after: bool) {
        if self.events.len() < Self::LIMIT {
            self.events.push(AnchorEvent {leg:self.leg,reason,before,proposed,after,planted_before,planted_after});
        } else {self.dropped=self.dropped.saturating_add(1);}
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Foot {
    /// Where the ankle is after the last step; `None` before the first.
    pub contact: Option<Contact>,
    pub support:SupportEvidence,
    anchor: Option<Contact>,
    from: Contact,
    goal: Contact,
    /// Last geometrically/source-checked placement, not a Ready vote. Saved so
    /// restore observes exactly the same target-change events during recovery.
    placement_checked:Option<Contact>,
    /// Nonzero only for a failed placement, keyed by its forecast frame.
    placement_frame:u64,
    base: V3,
    swing: bool,
    clearance: Fx,
    progress: Fx,
    /// Fastest driven cadence in this swing. Braking the hull cannot strand
    /// an already raised foot; a new swing starts from the new drive pace.
    cadence: Fx,
    /// Hull-local ankle, heading offset and normal while airborne.
    local: Option<(V3, i32, V3)>,
}

const ALT_BUCKETS: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
struct AltKey {
    lane_head: i32,
    scale: Fx,
    head: i32,
    pos: V3,
    sign_neg: bool,
}

#[derive(Clone, Default)]
struct AltOffsetCache {
    fitted: Option<(Contact, Fx)>,
    fitted_cnt: Option<Option<usize>>,
    flat_cnt: Option<Option<usize>>,
    pass1_built: bool,
    pass1_exhausted: bool,
    pass1: Vec<(V3, Option<Fx>, Option<bool>)>,
}

#[derive(Clone)]
struct AltEntry {
    key: AltKey,
    geometry: Option<(Sole, i32)>,
    offsets: [AltOffsetCache; 7],
}

struct AltTable {
    slots: [[u16; 2]; ALT_BUCKETS],
    next_way: [u64; ALT_BUCKETS / 64],
    entries: Vec<AltEntry>,
}

/// Phase-local memoization of terrain-only `ready_alternative` candidates
/// within one immutable motor selection. Compact 1 KB slot directory avoids
/// allocating unused 1 KB `AltEntry` buckets on every walker tick.
pub(crate) struct AlternativeMemo {
    table: std::cell::OnceCell<std::cell::RefCell<AltTable>>,
}

impl Default for AlternativeMemo {
    fn default() -> Self {
        Self {
            table: std::cell::OnceCell::new(),
        }
    }
}

impl AlternativeMemo {
    fn hash_key(k: &AltKey) -> usize {
        use nav::vehicle::support::mix;
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for v in [k.scale, k.pos.x, k.pos.y, k.pos.z] {
            h = mix(h, v.raw() as u64);
        }
        h = mix(mix(h, k.lane_head as u32 as u64), k.head as u32 as u64);
        h = mix(h, u64::from(k.sign_neg));
        (h as usize) & (ALT_BUCKETS - 1)
    }
}

impl Foot {
    pub fn ready(&self)->bool {self.planted() && self.support.readiness==Readiness::Ready}

    /// Called on landing or a local source/terrain event, never as an army-wide
    /// voxel scan. Touching is geometric; Ready is a separate weight admission.
    pub(crate) fn assess_support(&mut self,rig:&LegRig,root:&Frame,head:i32,
        surface:&dyn nav::vehicle::support::Surface,floor:&dyn Fn(Fx,Fx)->Fx,
        profile:modding::walking_terrain::Capability,lift:Fx,revalidate:bool,trace:Option<&mut AnchorTrace>) {
        self.assess_support_memo(rig,root,head,surface,floor,profile,lift,revalidate,None,trace)
    }

    pub(crate) fn assess_support_memo(&mut self,rig:&LegRig,root:&Frame,head:i32,
        surface:&dyn nav::vehicle::support::Surface,floor:&dyn Fn(Fx,Fx)->Fx,
        profile:modding::walking_terrain::Capability,lift:Fx,revalidate:bool,
        alt_memo:Option<&AlternativeMemo>,mut trace:Option<&mut AnchorTrace>) {
        use nav::vehicle::support::{mix,Validity};
        if !self.planted() {return;}
        let Some(contact)=self.contact else {return;};
        let before=self.support;
        let mut reason=SupportReason::Supported;
        if revalidate {
            if let Some(source)=before.source {
                reason=match surface.validity(source) {
                    Validity::Moving=>SupportReason::Moving,Validity::Removed=>SupportReason::Removed,
                    Validity::Unsupported=>SupportReason::Unsupported,Validity::Supported=>reason,
                };
            }
        }
        let mut next=SupportEvidence {readiness:Readiness::Ready,reason,..Default::default()};
        let axes=contact.axes();let tolerance=Fx::from_ratio(profile.sole_gap_mm,i32::from(voxel::unit::WORLD_MM));
        let mut signature=0xcbf29ce484222325;
        let mut max_supported_y=Fx::MIN;
        let mut step_unclimbable=false;
        if matches!(reason,SupportReason::Supported) {
            for (i,p) in rig.sole.probes().iter().enumerate() {
                let at=contact.pos.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z));
                let sample=surface.sample(at.x,at.y,at.z);next.samples+=1;
                for v in [u64::from(sample.source.kind),sample.source.id,sample.source.content,sample.source.pose] {signature=mix(signature,v);}
                if sample.validity==Validity::Moving {reason=SupportReason::Moving;}
                let gap=at.y.sub(sample.height);
                if gap<tolerance.neg() && reason==SupportReason::Supported {reason=SupportReason::Obstructed;}
                if sample.validity==Validity::Supported && gap.abs()<=tolerance {
                    next.supported_mask|=1<<i;
                    max_supported_y=max_supported_y.max(sample.height);
                    if next.source.is_none() {next.source=Some(sample.source);}
                }
            }
            if contact.normal.y<Fx::from_ratio(profile.min_up_pct,100) && reason==SupportReason::Supported {reason=SupportReason::Normal;}
            if !revalidate && reason==SupportReason::Supported {
                if !rig.contact_reachable(to_local(root,contact.pos)) {
                    reason=SupportReason::Unreachable;
                } else if next.supported_mask!=0 && !rig.step_climbable_to(root,contact.pos,max_supported_y,floor,profile) {
                    reason=SupportReason::Unreachable;
                    step_unclimbable=true;
                }
            }
            if reason==SupportReason::Supported && (next.samples==0 || next.supported_mask.count_ones()*100<u32::from(next.samples)*profile.sole_ready_pct as u32) {
                reason=if next.supported_mask==0 {SupportReason::Unsupported}else{SupportReason::PartialSole};
            }
        } else {
            next.source=before.source;next.samples=before.samples;
        }
        next.signature=mix(signature,u64::from(next.supported_mask));
        if reason==SupportReason::Supported && before.source.is_some() && (before.source!=next.source || before.signature!=next.signature) {
            reason=SupportReason::SourceChanged;
        } else if reason==SupportReason::Supported && before.readiness==Readiness::Ready && before.reason==SupportReason::SourceChanged {
            reason=SupportReason::SourceChanged;
        }
        next.reason=reason;
        let accepted=matches!(reason,SupportReason::Supported|SupportReason::SourceChanged);
        if !accepted {
            next.readiness=Readiness::Rejected;
            let neutral=if step_unclimbable {
                rig.retracted_neutral(root,head.wrapping_add(rig.lane.head),floor,profile)
            } else {
                rig.sole.contact(to_world(root,rig.lane_target(rig.ankle)),head.wrapping_add(rig.lane.head),floor)
            };
            let alternative=if step_unclimbable || matches!(reason,SupportReason::PartialSole|SupportReason::Unsupported|SupportReason::Obstructed|SupportReason::Normal) {
                self.ready_alternative_memo(rig,root,head,contact,surface,floor,profile,alt_memo)
            }else{None};
            self.recover_to(alternative.unwrap_or(neutral));
            if let Some(goal)=alternative {
                self.clearance=rig.sole_clearance_checked(root,self.from,goal,lift,floor,Some(profile));
                if let Some(t)=trace.as_deref_mut(){t.record(AnchorReason::SupportAlternative,Some(contact),goal,self.contact,false,false);}
            }
        }
        self.support=next;
        if before!=next {if let Some(t)=trace.as_deref_mut() {
            let reason=match reason {SupportReason::Moving=>AnchorReason::SupportMoving,SupportReason::Removed=>AnchorReason::SupportRemoved,
                SupportReason::SourceChanged=>AnchorReason::SupportChanged,_ if accepted=>AnchorReason::SupportReady,_=>AnchorReason::SupportRejected};
            t.record(reason,Some(contact),contact,self.contact,true,self.planted());
        }}
    }

    /// Rejecting a tiny contact must not retry the same bad placement forever.
    /// Bounded geometry-derived candidates stay in this leg's lane; the
    /// current/from contact never moves. Ready is earned only at real touchdown.
    #[cfg(test)]
    fn ready_alternative(&self,rig:&LegRig,root:&Frame,head:i32,contact:Contact,
        surface:&dyn nav::vehicle::support::Surface,floor:&dyn Fn(Fx,Fx)->Fx,
        profile:modding::walking_terrain::Capability)->Option<Contact> {
        self.ready_alternative_memo(rig,root,head,contact,surface,floor,profile,None)
    }

    fn ready_alternative_memo(&self,rig:&LegRig,root:&Frame,head:i32,contact:Contact,
        surface:&dyn nav::vehicle::support::Surface,floor:&dyn Fn(Fx,Fx)->Fx,
        profile:modding::walking_terrain::Capability,alt_memo:Option<&AlternativeMemo>)->Option<Contact> {
        let forward=V3::new(angle::cos(head),ZERO,angle::sin(head));
        let sign_neg=self.goal.pos.sub(self.from.pos).dot(forward)<ZERO;
        let key=AltKey{lane_head:rig.lane.head,scale:root.scale,head,pos:contact.pos,sign_neg};
        let eval=|entry:&mut AltEntry| Self::eval_ready_alternative(entry,rig,root,head,contact,sign_neg,surface,floor,profile);
        if let Some(memo)=alt_memo {
            let bucket=AlternativeMemo::hash_key(&key);
            let cell=memo.table.get_or_init(|| {
                std::cell::RefCell::new(AltTable {
                    slots: [[u16::MAX; 2]; ALT_BUCKETS],
                    next_way: [0; ALT_BUCKETS / 64],
                    entries: Vec::with_capacity(16),
                })
            });
            let mut table=cell.borrow_mut();
            for way in 0..2 {
                let idx=table.slots[bucket][way];
                if idx!=u16::MAX {
                    let e=&table.entries[idx as usize];
                    if e.key==key
                        && e.geometry.as_ref().is_some_and(|(sole,lane_head)|sole==&rig.sole && *lane_head==rig.lane.head)
                    {
                        return eval(&mut table.entries[idx as usize]);
                    }
                }
            }
            let way=if table.slots[bucket][0]==u16::MAX {0} else if table.slots[bucket][1]==u16::MAX {1} else {
                let word=&mut table.next_way[bucket>>6];
                let bit=bucket&63;
                let w=((*word>>bit)&1) as usize;
                *word^=1u64<<bit;
                w
            };
            let new_entry=AltEntry{
                key,
                geometry:Some((rig.sole.clone(),rig.lane.head)),
                offsets:std::array::from_fn(|_|AltOffsetCache::default()),
            };
            let idx=if table.slots[bucket][way]!=u16::MAX {
                let idx=table.slots[bucket][way];
                table.entries[idx as usize]=new_entry;
                idx
            } else {
                let idx=table.entries.len() as u16;
                table.entries.push(new_entry);
                table.slots[bucket][way]=idx;
                idx
            };
            eval(&mut table.entries[idx as usize])
        } else {
            let mut local_entry=AltEntry{key,geometry:None,offsets:std::array::from_fn(|_|AltOffsetCache::default())};
            eval(&mut local_entry)
        }
    }

    #[cfg(test)]
    fn legacy20_ready_alternative(entry:&mut Legacy20Entry,rig:&LegRig,root:&Frame,head:i32,contact:Contact,
        sign_neg:bool,surface:&dyn nav::vehicle::support::Surface,floor:&dyn Fn(Fx,Fx)->Fx,
        profile:modding::walking_terrain::Capability)->Option<Contact> {
        let low=rig.sole.probes.iter().map(|p|p.x).min().unwrap_or(ZERO);
        let high=rig.sole.probes.iter().map(|p|p.x).max().unwrap_or(ZERO);
        let span=high.sub(low).max(ONE.div(root.scale));
        let forward=V3::new(angle::cos(head),ZERO,angle::sin(head));
        let across=V3::new(angle::sin(head).neg(),ZERO,angle::cos(head));
        let sign=if sign_neg {ONE.neg()}else{ONE};
        let far=rig.l1.add(rig.l2).mul(Fx::from_ratio(profile.stance_reach_pct,100));
        let min_up=Fx::from_ratio(profile.min_up_pct,100);
        // A least-squares slope over scattered stones can leave only one toe
        // touching. Try the horizontal supporting plane at the same location
        // before moving the requested placement within its own lane.
        let short=rig.sole.width().max(ONE.div(root.scale)).mul(Fx::from_ratio(1,3));
        let offsets=[V3::ZERO,forward.scale(span.mul(Fx::from_ratio(1,4)).mul(sign)),
            forward.scale(span.mul(Fx::from_ratio(1,4)).mul(sign.neg())),
            across.scale(short),across.scale(short.neg()),
            forward.scale(span.mul(Fx::from_ratio(2,3)).mul(sign)),
            forward.scale(span.mul(Fx::from_ratio(2,3)).mul(sign.neg()))];
        // Prefer a nearby ordinary placement to a steep three-point plane on
        // a pebble. Keep both passes finite and deterministic.
        for planes_only in [false,true] {
            let mut best:Option<(usize,Fx,Contact)>=None;
            for (idx,&offset) in offsets.iter().enumerate() {
                let cost=offset.len_sq();
                // A fully supported earlier candidate cannot lose to the same or
                // greater offset cost. Preserve the original first-wins tie rule;
                // partial soles still require the remaining complete candidates.
                if best.as_ref().is_some_and(|(count,old,_)|*count==rig.sole.probes.len() && cost>=*old) {continue;}
                let at=contact.pos.add(offset);
                let slot=&mut entry.offsets[idx];
                let fitted=*slot.fitted.get_or_insert_with(||rig.sole.legacy20_contact(at,head.wrapping_add(rig.lane.head),floor));
                if !planes_only {
                    let flat=*slot.flat.get_or_insert_with(|| if fitted.normal==V3::UP {fitted} else {
                        Contact{pos:V3::new(at.x,rig.sole.legacy20_height(at,fitted.head,V3::UP,floor),at.z),normal:V3::UP,..fitted}
                    });
                    for (pass_flat,goal) in [(false,fitted),(true,flat)] {
                        if pass_flat && fitted.normal==V3::UP {continue;}
                        if best.as_ref().is_some_and(|(count,old,_)|*count==rig.sole.probes.len() && cost>=*old) {continue;}
                        let local=to_local(root,goal.pos);
                        if !rig.contact_reachable(local) || local.sub(rig.hip).len_sq()>far.mul(far)
                            || rig.lane_target(local).sub(local).len_sq()>MILLI.mul(MILLI)
                            || goal.normal.y<min_up {continue;}
                        let cnt_slot=if pass_flat {&mut slot.flat_cnt} else {&mut slot.fitted_cnt};
                        let count=*cnt_slot.get_or_insert_with(||Self::support_count(rig,goal,surface,profile));
                        if let Some(count)=count {
                            if best.as_ref().is_none_or(|(old,d,_)|count>*old || count==*old && cost<*d) {best=Some((count,cost,goal));}
                        }
                    }
                    continue;
                }
                // A rigid sole on scattered stones rests on a supporting plane,
                // rather than the least-squares average through those stones.
                // Enumerate only the <=84 triples of the nine occupied probes.
                if !slot.pass1_built {
                    let probes=rig.sole.probes();let mut points=[V3::ZERO;9];
                    for (i,p) in probes.iter().enumerate() {
                        let world=at.add(turned(*p,fitted.head));
                        points[i]=V3::new(p.x,floor(world.x,world.z).sub(p.y),p.z);
                    }
                    if probes.len()>=3 && points[1..probes.len()].iter().any(|q|q.y!=points[0].y) {
                        let mut normals=[V3::UP;86];normals[0]=fitted.normal;let mut used=2;
                        for i in 0..probes.len() {for j in i+1..probes.len() {for k in j+1..probes.len() {
                            let mut normal=points[j].sub(points[i]).cross(points[k].sub(points[i]));
                            if normal.y<ZERO {normal=normal.scale(ONE.neg());}
                            if normal.y<=MILLI {continue;}
                            normal=unit_or(normal,V3::UP);
                            if normal.y<min_up {continue;}
                            if normals[..used].contains(&normal) {continue;}
                            normals[used]=normal;used+=1;
                            let goal=Contact {pos:V3::new(at.x,rig.sole.legacy20_height(at,fitted.head,normal,floor),at.z),normal,..fitted};
                            slot.pass1.push((goal,None));
                        }}}
                    }
                    slot.pass1_built=true;
                }
                for (goal,ready_slot) in &mut slot.pass1 {
                    let local=to_local(root,goal.pos);
                    if rig.contact_reachable(local) && local.sub(rig.hip).len_sq()<=far.mul(far)
                        && rig.lane_target(local).sub(local).len_sq()<=MILLI.mul(MILLI) {
                        let ready=*ready_slot.get_or_insert_with(||Self::ready_at(rig,*goal,surface,profile));
                        if ready {return Some(*goal);}
                    }
                }
            }
            if let Some((_,_,goal))=best {return Some(goal);}
        }
        None
    }

    fn eval_ready_alternative(entry:&mut AltEntry,rig:&LegRig,root:&Frame,head:i32,contact:Contact,
        sign_neg:bool,surface:&dyn nav::vehicle::support::Surface,floor:&dyn Fn(Fx,Fx)->Fx,
        profile:modding::walking_terrain::Capability)->Option<Contact> {
        let low=rig.sole.probes.iter().map(|p|p.x).min().unwrap_or(ZERO);
        let high=rig.sole.probes.iter().map(|p|p.x).max().unwrap_or(ZERO);
        let span=high.sub(low).max(ONE.div(root.scale));
        let (ch,sh)=(angle::cos(head),angle::sin(head));
        let forward=V3::new(ch,ZERO,sh);
        let across=V3::new(sh.neg(),ZERO,ch);
        let sign=if sign_neg {ONE.neg()}else{ONE};
        let far=rig.l1.add(rig.l2).mul(Fx::from_ratio(profile.stance_reach_pct,100));
        let far_sq=far.mul(far);
        let link_far=rig.l1.add(rig.l2).mul(Fx::from_ratio(999,1000));
        let link_far_sq=link_far.mul(link_far);
        let milli_sq=MILLI.mul(MILLI);
        let level_root=root.x.y==ZERO && root.z.y==ZERO;
        let world_hip=to_world(root,rig.hip);
        let world_reach_bound=far.min(link_far).add(Fx::from_ratio(1,100));
        let world_reach_bound_sq=world_reach_bound.mul(world_reach_bound);
        let xz_feasible=|at:V3|->bool {
            if level_root {
                let dx=at.x.sub(root.base.x);
                let dz=at.z.sub(root.base.z);
                let lx=dx.mul(root.x.x).add(dz.mul(root.x.z));
                let lz=dx.mul(root.z.x).add(dz.mul(root.z.z));
                let cx=lx.max(rig.lane.min[0]).min(rig.lane.max[0]);
                let cz=lz.max(rig.lane.min[1]).min(rig.lane.max[1]);
                let ex=cx.sub(lx);let ez=cz.sub(lz);
                if ex.mul(ex).add(ez.mul(ez))>milli_sq {return false;}
                let hx=lx.sub(rig.hip.x);let hz=lz.sub(rig.hip.z);
                let h_sq=hx.mul(hx).add(hz.mul(hz));
                h_sq<=far_sq && h_sq<=link_far_sq
            } else {
                let wx=at.x.sub(world_hip.x);
                let wz=at.z.sub(world_hip.z);
                wx.mul(wx).add(wz.mul(wz))<=world_reach_bound_sq
            }
        };
        let min_up=Fx::from_ratio(profile.min_up_pct,100);
        let short=rig.sole.width().max(ONE.div(root.scale)).mul(Fx::from_ratio(1,3));
        let offsets=[V3::ZERO,forward.scale(span.mul(Fx::from_ratio(1,4)).mul(sign)),
            forward.scale(span.mul(Fx::from_ratio(1,4)).mul(sign.neg())),
            across.scale(short),across.scale(short.neg()),
            forward.scale(span.mul(Fx::from_ratio(2,3)).mul(sign)),
            forward.scale(span.mul(Fx::from_ratio(2,3)).mul(sign.neg()))];
        for planes_only in [false,true] {
            let mut best:Option<(usize,Fx,Contact)>=None;
            for (idx,&offset) in offsets.iter().enumerate() {
                let cost=offset.len_sq();
                if best.as_ref().is_some_and(|(count,old,_)|*count==rig.sole.probes.len() && cost>=*old) {continue;}
                let at=contact.pos.add(offset);
                if !xz_feasible(at) {continue;}
                let slot=&mut entry.offsets[idx];
                if planes_only && slot.pass1_exhausted {continue;}
                let (fitted,flat_y)=*slot.fitted.get_or_insert_with(||
                    rig.sole.contact_with_flat(at,head.wrapping_add(rig.lane.head),floor));
                if !planes_only {
                    let flat=if fitted.normal==V3::UP {fitted} else {
                        Contact{pos:V3::new(at.x,flat_y,at.z),normal:V3::UP,..fitted}
                    };
                    for (pass_flat,goal) in [(false,fitted),(true,flat)] {
                        if pass_flat && fitted.normal==V3::UP {continue;}
                        if best.as_ref().is_some_and(|(count,old,_)|*count==rig.sole.probes.len() && cost>=*old) {continue;}
                        let cnt_slot=if pass_flat {&mut slot.flat_cnt} else {&mut slot.fitted_cnt};
                        if *cnt_slot==Some(None) || goal.normal.y<min_up {continue;}
                        let local=to_local(root,goal.pos);
                        if !rig.contact_reachable(local) || local.sub(rig.hip).len_sq()>far_sq
                            || rig.lane_target(local).sub(local).len_sq()>milli_sq
                            || !rig.step_climbable(root,goal,floor,profile) {continue;}
                        let count=*cnt_slot.get_or_insert_with(||Self::support_count(rig,goal,surface,profile));
                        if let Some(count)=count {
                            if best.as_ref().is_none_or(|(old,d,_)|count>*old || count==*old && cost<*d) {best=Some((count,cost,goal));}
                        }
                    }
                    continue;
                }
                if !slot.pass1_built {
                    let probes=rig.sole.probes();let mut points=[V3::ZERO;9];
                    let (fc,fs)=if fitted.head==0 {(ONE,ZERO)} else {(angle::cos(fitted.head),angle::sin(fitted.head))};
                    for (i,p) in probes.iter().enumerate() {
                        let world=at.add(turned_cs(*p,fitted.head,fc,fs));
                        points[i]=V3::new(p.x,floor(world.x,world.z).sub(p.y),p.z);
                    }
                    if probes.len()>=3 && points[1..probes.len()].iter().any(|q|q.y!=points[0].y) {
                        let mut normals=[V3::UP;86];normals[0]=fitted.normal;let mut used=2;
                        for i in 0..probes.len() {for j in i+1..probes.len() {for k in j+1..probes.len() {
                            let mut normal=points[j].sub(points[i]).cross(points[k].sub(points[i]));
                            if normal.y<ZERO {normal=normal.scale(ONE.neg());}
                            if normal.y<=MILLI {continue;}
                            normal=unit_or(normal,V3::UP);
                            if normal.y<min_up {continue;}
                            if normals[..used].contains(&normal) {continue;}
                            normals[used]=normal;used+=1;
                            slot.pass1.push((normal,None,None));
                        }}}
                    }
                    slot.pass1_built=true;
                    if slot.pass1.is_empty() {
                        slot.pass1_exhausted=true;
                        continue;
                    }
                }
                let mut all_rejected=true;
                for (normal,y_slot,ready_slot) in &mut slot.pass1 {
                    if *ready_slot==Some(false) {continue;}
                    let y=*y_slot.get_or_insert_with(||rig.sole.height(at,fitted.head,*normal,floor));
                    let goal=Contact{pos:V3::new(at.x,y,at.z),normal:*normal,..fitted};
                    let local=to_local(root,goal.pos);
                    if rig.contact_reachable(local) && local.sub(rig.hip).len_sq()<=far_sq
                        && rig.lane_target(local).sub(local).len_sq()<=milli_sq
                        && rig.step_climbable(root,goal,floor,profile) {
                        let ready=*ready_slot.get_or_insert_with(||Self::ready_at(rig,goal,surface,profile));
                        if ready {return Some(goal);}
                    } else {
                        all_rejected=false;
                    }
                }
                if all_rejected {slot.pass1_exhausted=true;}
            }
            if let Some((_,_,goal))=best {return Some(goal);}
        }
        None
    }

    fn ready_at(rig:&LegRig,goal:Contact,surface:&dyn nav::vehicle::support::Surface,
        profile:modding::walking_terrain::Capability)->bool {
        Self::support_count(rig,goal,surface,profile).is_some()
    }
    fn support_count(rig:&LegRig,goal:Contact,surface:&dyn nav::vehicle::support::Surface,
        profile:modding::walking_terrain::Capability)->Option<usize> {
        use nav::vehicle::support::Validity;
        if goal.normal.y<Fx::from_ratio(profile.min_up_pct,100) {return None;}
        let tolerance=Fx::from_ratio(profile.sole_gap_mm,i32::from(voxel::unit::WORLD_MM));
        let axes=goal.axes();let mut supported=0;
        for p in rig.sole.probes() {
            let at=goal.pos.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z));
            let sample=surface.sample(at.x,at.y,at.z);let gap=at.y.sub(sample.height);
            if sample.validity==Validity::Moving || gap<tolerance.neg() {return None;}
            supported+=usize::from(sample.validity==Validity::Supported && gap.abs()<=tolerance);
        }
        (supported*100>=rig.sole.probes.len()*profile.sole_ready_pct as usize).then_some(supported)
    }

    /// Geometry-only callers have no authority to admit weight. Motion calls
    /// assess_support after a newly planted contact before counting supports.
    fn pending_support(&mut self) {self.support.readiness=Readiness::Pending;self.support.reason=SupportReason::Unchecked;}
    fn swing_position(&self,q:Fx)->V3 {
        let eased=q.mul(q).mul(Fx::from_int(3).sub(q.mul(Fx::from_int(2))));
        let mut pos=self.from.pos.add(self.goal.pos.sub(self.from.pos).scale(eased));
        // A recovery can already start high after rejecting a moving support.
        // Its descent supplies half the midpoint height span; do not spend that
        // same height again as an extra upward arc. Live sole collision checks
        // still hold/raise the executed contact when an obstacle requires it.
        let credit=self.from.pos.y.sub(self.goal.pos.y).max(ZERO).mul(Fx::from_ratio(1,2));
        pos.y=pos.y.add(swing_arc(q).mul(self.clearance.sub(credit).max(ZERO)));
        pos
    }

    #[cfg(test)]
    fn swing_contact(&self,rig:&LegRig,q:Fx,floor:&dyn Fn(Fx,Fx)->Fx)->(Contact,bool) {
        self.swing_contact_checked(rig,q,floor,None)
    }

    fn swing_contact_checked(&self,rig:&LegRig,q:Fx,floor:&dyn Fn(Fx,Fx)->Fx,
        step_guard:Option<(&Frame,modding::walking_terrain::Capability)>)->(Contact,bool) {
        let eased=q.mul(q).mul(Fx::from_int(3).sub(q.mul(Fx::from_int(2))));
        let mut contact=self.from.lerp(self.goal,eased);
        contact.pos=self.swing_position(q);
        // Evaluate the executed sole, including its heading/normal and the
        // lift-before-riser hold, rather than the unfitted trajectory point.
        let mut height=rig.sole.height(contact.pos,contact.head,contact.normal,floor);
        let mut waiting=false;
        if contact.pos.y.add(MILLI)<height {
            if let Some(old)=self.contact {
                let held=V3::new(old.pos.x,contact.pos.y,old.pos.z);
                let under=rig.sole.height(held,contact.head,contact.normal,floor);
                if under<=contact.pos.y {contact.pos=held;height=under;waiting=true;}
            }
            if !waiting {
                if let Some((root,profile))=step_guard {
                    let raised=Contact{pos:V3::new(contact.pos.x,height,contact.pos.z),..contact};
                    if !rig.step_sole_climbable(root,raised,floor,profile) {
                        for fallback in self.contact.into_iter().chain(std::iter::once(self.from)) {
                            let held=V3::new(fallback.pos.x,contact.pos.y,fallback.pos.z);
                            let under=rig.sole.height(held,fallback.head,fallback.normal,floor);
                            let held_contact=Contact{pos:V3::new(held.x,contact.pos.y.max(under),held.z),..fallback};
                            if under<=contact.pos.y || rig.step_sole_climbable(root,held_contact,floor,profile) {
                                contact=held_contact;height=under;waiting=true;break;
                            }
                        }
                    }
                }
            }
        }
        contact.pos.y=contact.pos.y.max(height);
        (contact,waiting)
    }

    /// Most take-offs retain q=0. If accepted body motion already puts that
    /// point outside the links, admit the earliest reachable sample of the
    /// existing arc within this tick. This changes timing, never the origin.
    fn fit_takeoff(&mut self,rig:&LegRig,root:&Frame,limit:Fx,floor:&dyn Fn(Fx,Fx)->Fx,
        step_guard:Option<(&Frame,modding::walking_terrain::Capability)>) {
        let fits=|q|rig.contact_reachable(to_local(root,self.swing_contact_checked(rig,q,floor,step_guard).0.pos));
        if fits(ZERO) {return;}
        let limit=limit.min(Fx::from_ratio(1,2));
        let mut bad=ZERO;
        for i in 1..=8 {
            let mut good=limit.mul(Fx::from_ratio(i,8));
            if fits(good) {
                for _ in 0..8 {
                    let mid=bad.add(good).mul(Fx::from_ratio(1,2));
                    if fits(mid) {good=mid;} else {bad=mid;}
                }
                self.progress=good;
                return;
            }
            bad=good;
        }
    }
    fn recover_to(&mut self, goal: Contact) {
        self.invalidate_placement();
        self.from=self.contact.unwrap_or(goal);
        self.goal=goal;
        self.progress=ZERO;
        self.clearance=ZERO;
        self.cadence=self.cadence.max(Fx::from_ratio(1,80));
        self.swing=true;
    }
    /// Revalidate only a sole whose supporting columns changed. Losing ground
    /// starts an ordinary saved swing, rather than retaining a fictitious
    /// anchor or snapping the ankle down to the bottom of a new crater.
    #[cfg(test)]
    pub(crate) fn refresh_support(&mut self, rig: &LegRig, root: &Frame, head: i32,
        floor: &dyn Fn(Fx, Fx) -> Fx) {
        if !self.planted() { return; }
        let Some(old) = self.contact else { return; };
        let live = rig.sole.contact(old.pos, old.head, floor);
        if live.pos.y.sub(old.pos.y).abs() <= MILLI && live.normal == old.normal { return; }
        if live.pos.y >= old.pos.y {
            self.contact = Some(live);
            self.anchor = Some(live);
            self.local = Some((to_local(root, live.pos), angle::diff(live.head, head), live.normal));
        } else {
            self.recover_to(rig.sole.contact(to_world(root, rig.lane_target(rig.ankle)),
                head.wrapping_add(rig.lane.head), floor));
        }
    }

    pub(crate) fn needs_reset(&self,rig:&LegRig,root:&Frame)->bool {
        let reach=rig.l1.add(rig.l2);
        self.anchor.is_none() || self.base.sub(root.base).len_sq()>reach.mul(reach).mul(Fx::from_int(4))
    }
    /// Standing on its contact: on the ground and not lifted. A swing
    /// begins at the anchor, but is already an active swing.
    pub fn planted(&self) -> bool {
        self.anchor.is_some() && !self.swing
    }
    /// The executed outgoing half still has to be reachable at the admitted
    /// body pose. It carries no planted-support vote during this interval.
    pub(crate) fn departure_share()->Fx {Fx::from_ratio(1,2)}
    pub(crate) fn departing(&self)->bool {self.swing && self.progress<=Self::departure_share()}

    /// A buckled leg (ADR-288): it folds on its own foot. The foot stays on
    /// the ground, drawn in under the hip to `share_milli` thousandths of
    /// its rest reach, so the two-link solve bends the knee out; the leg
    /// neither steps nor carries.
    pub fn fold(
        &mut self,
        rig: &LegRig,
        root: &Frame,
        head: i32,
        share_milli: u32,
        floor: &dyn Fn(Fx, Fx) -> Fx,
    ) -> Contact {
        let k = Fx::from_ratio(share_milli.min(1000) as i32, 1000);
        let rest = rig.ankle.sub(rig.hip);
        let local = V3::new(
            rig.hip.x.add(rest.x.mul(k)),
            rig.ankle.y,
            rig.hip.z.add(rest.z.mul(k)),
        );
        let c = rig.sole.contact(to_world(root, local), head, floor);
        self.anchor = Some(c);
        self.pending_support();
        self.swing = false;
        self.progress = ZERO;
        self.local = None;
        self.contact = Some(c);
        c
    }

    /// Advance one tick and return the ankle's contact.
    pub fn step(&mut self, rig: &LegRig, s: &Stance<'_>) -> Contact {
        self.step_with_liftoff(rig, s, true)
    }

    /// Shared by sole placement, lift planning and motor admission.
    pub(crate) fn heading_limit()->i32 {angle::FULL/24}
    pub(crate) fn heading_deadline(&self,s:&Stance<'_>)->Fx {
        if !self.planted() || s.angular==0 {return Fx::MAX;}
        let Some(anchor)=self.anchor else {return ZERO;};
        let used=angle::diff(s.head,anchor.head)*s.angular.signum();
        Fx::from_ratio((Self::heading_limit()-used).max(0),s.angular.abs())
    }

    /// Leave before the moving hull exhausts the reserved corridor or reach.
    fn lift_urgent_at(&self,rig:&LegRig,s:&Stance<'_>,reserve:Fx,carrying:Fx)->bool {
        if s.cycles<=ZERO || self.swing {return false;}
        let Some(anchor)=self.anchor else {return false;};
        let time=ONE.sub(s.gait.duty).mul(carrying).div(s.cycles.max(Fx::from_ratio(1,80)));
        let future=to_local(s.root,anchor.pos.sub(s.linear.scale(time)));
        let yaw=(i128::from(s.angular)*i128::from(time.raw())>>32)
            .clamp(-i128::from(angle::HALF),i128::from(angle::HALF)) as i32;
        let future=turned(future,-yaw);
        let reach=rig.l1.add(rig.l2).mul(reserve);
        carrying<ONE && self.heading_deadline(s)<=time
            || rig.lane_target(future)!=future || future.sub(rig.hip).len_sq()>reach.mul(reach)
    }
    pub(crate) fn lift_deadline(&self,rig:&LegRig,s:&Stance<'_>,reserve:Fx)->Fx {
        let Some(anchor)=self.anchor else {return ZERO;};
        let now=to_local(s.root,anchor.pos);
        let next=turned(to_local(s.root,anchor.pos.sub(s.linear)),-s.angular);
        let mut deadline=Fx::MAX;
        let ticks=|distance:Fx,rate:Fx| Fx(((i128::from(distance.max(ZERO).raw())<<32)
            /i128::from(rate.raw())).min(i128::from(i64::MAX)) as i64);
        for (i,(at,after)) in [(now.x,next.x),(now.z,next.z)].into_iter().enumerate() {
            let delta=after.sub(at);
            if delta<ZERO && rig.lane.min[i]!=Fx::MIN {
                deadline=deadline.min(ticks(at.sub(rig.lane.min[i]),delta.neg()));
            } else if delta>ZERO && rig.lane.max[i]!=Fx::MAX {
                deadline=deadline.min(ticks(rig.lane.max[i].sub(at),delta));
            }
        }
        // An unbounded outer lane still has a link-reach deadline. Sorting
        // only by lane edges can keep the most stretched foot down while
        // admitting a less urgent neighbour, forcing the hull to crouch.
        let reach=rig.l1.add(rig.l2).mul(reserve);
        let offset=now.sub(rig.hip);
        let velocity=next.sub(now);
        let c=offset.len_sq().sub(reach.mul(reach));
        let a=velocity.len_sq();
        if a>MILLI.mul(MILLI) {
            let b=offset.dot(velocity);
            // A just-landed foot can be at the reserve boundary while the
            // hull is approaching it. Its deadline is the later exit, not
            // zero: otherwise index ties repeatedly lift the new pair and
            // starve the trailing pair after a descent.
            if c>=ZERO && b>=ZERO {return ZERO;}
            let discriminant=b.mul(b).sub(a.mul(c));
            if discriminant>=ZERO {
                deadline=deadline.min(discriminant.sqrt().sub(b).div(a).max(ZERO));
            }
        } else if c>=ZERO {return ZERO;}
        deadline
    }

    pub(crate) fn requests_step(&self,rig:&LegRig,s:&Stance<'_>,reserve:Fx)->bool {
        let phase=Fx::from_ratio(angle::norm(s.phase.wrapping_sub(
            s.gait.offset[rig.support.min(3)].wrapping_add(rig.phase_shift))),angle::FULL);
        self.planted() && s.cycles>ZERO &&
            (self.lift_urgent_at(rig,s,reserve,ONE) || phase>=s.gait.duty && self.progress<ONE)
    }

    /// Remaining driven time of the committed swing, in simulation ticks.
    /// An unresolved touchdown gets another tick, never an assumed support.
    fn finish_rate(&self,cycles:Fx)->Fx {
        self.cadence.max(cycles).max(Fx::from_ratio(1,80))
    }

    fn landing_forecast(&self,root:&Frame,linear:V3,angular:i32,cycles:Fx,gait:&Gait)->(Frame,Fx) {
        let time=ONE.sub(self.progress).mul(ONE.sub(gait.duty).max(MILLI)).div(self.finish_rate(cycles));
        let yaw=(i128::from(angular)*i128::from(time.raw())>>32)
            .clamp(-i128::from(angle::HALF),i128::from(angle::HALF)) as i32;
        let mut landing=*root;
        let travel=[linear.x,linear.z].map(|v|i128::from(v.raw())*i128::from(time.raw())>>32);
        let limit=i128::from(gait.stride.mul(Fx::from_int(2)).raw()).max(0);
        let widest=travel.iter().map(|v|v.abs()).max().unwrap_or(0);
        let travel=travel.map(|v|Fx(if widest>limit {v*limit/widest} else {v} as i64));
        landing.base=landing.base.add(V3::new(travel[0],ZERO,travel[1]));
        landing.x=turned(landing.x,yaw);landing.y=turned(landing.y,yaw);landing.z=turned(landing.z,yaw);
        (landing,time)
    }

    fn placement_stamp(root:&Frame,head:i32)->u64 {
        let mut hash=0xcbf29ce484222325;
        for v in [root.base,root.x,root.y,root.z] {for n in [v.x,v.y,v.z] {hash=nav::vehicle::support::mix(hash,n.raw() as u64);}}
        nav::vehicle::support::mix(nav::vehicle::support::mix(hash,root.scale.raw() as u64),head as u32 as u64).max(1)
    }

    pub(crate) fn placement(&self)->Option<Contact> {self.swing.then_some(self.goal)}
    pub(crate) fn invalidate_placement(&mut self) {self.placement_checked=None;self.placement_frame=0;}

    pub(crate) fn rebase(&mut self,root:&Frame,head:i32) {
        if let Some(c)=self.contact {
            self.local=Some((to_local(root,c.pos),angle::diff(c.head,head),c.normal));
            self.base=root.base;
        }
    }

    /// Re-project an arriving free contact against the final post-constraint
    /// hull frame. Planted anchors and real departing takeoffs stay untouched.
    pub(crate) fn clamp_arriving_reach(&mut self,rig:&LegRig,root:&Frame,head:i32,mut trace:Option<&mut AnchorTrace>) {
        if self.planted() {return;}
        let Some(mut c)=self.contact else {return;};
        let mut local=to_local(root,c.pos);
        let mut changed=false;
        if !self.departing() && !rig.contact_reachable(local) {
            local=rig.contact_ankle(local);
            changed=true;
        }
        if (!self.departing() || self.support.readiness!=Readiness::Ready)
            && local.y>rig.ankle.y
            && rig.contact_reachable(local)
        {
            let rest_shin=rig.ankle.sub(rig.knee);
            let shin_ok=|p:V3| {
                if !rig.contact_reachable(p) {return false;}
                let pose=rig.solve_contact(p);
                let shin=pose.ankle.sub(pose.knee);
                shin.y<ZERO && shin.dot(rest_shin)>MILLI
            };
            if !shin_ok(local) {
                let low=rig.contact_ankle(V3::new(local.x,rig.ankle.y,local.z));
                if shin_ok(low) {
                    let mut good=low;
                    let mut bad=local;
                    for _ in 0..8 {
                        let mid=good.add(bad).scale(Fx::from_ratio(1,2));
                        if shin_ok(mid) {good=mid;} else {bad=mid;}
                    }
                    local=good;
                    changed=true;
                }
            }
        }
        if changed {
            c.pos=to_world(root,local);
            if let Some(t)=trace.as_deref_mut() {t.record(AnchorReason::Reproject,self.contact,c,Some(c),false,false);}
            self.contact=Some(c);
            self.rebase(root,head);
        }
    }

    pub(crate) fn blocked_landing(&self,rig:&LegRig,stance:&Stance<'_>)->bool {
        self.next_landing(stance.gait,stance.cycles).is_some_and(|t|t<=ONE)
            && !rig.contact_reachable(to_local(stance.root,
                rig.sole.contact(self.goal.pos,self.goal.head,stance.floor).pos))
    }

    pub(crate) fn next_landing(&self,gait:&Gait,cycles:Fx)->Option<Fx> {
        self.swing.then(||ONE.sub(self.progress).mul(ONE.sub(gait.duty))
            .div(self.finish_rate(cycles)).max(ONE))
    }

    /// A body coordinator may defer a new lift without interrupting an
    /// already committed swing. The saved phase/progress remain authoritative;
    /// a waiting foot does not construct another landing/clearance path.
    pub(crate) fn step_with_liftoff(&mut self, rig: &LegRig, s: &Stance<'_>, allow_liftoff: bool) -> Contact {
        self.step_coordinated(rig,s,allow_liftoff,false)
    }
    /// A four-leg coordinator starts the diagonal partner in the same
    /// cohort, retaining normal trajectory, cadence and terrain checks.
    pub(crate) fn step_coordinated(&mut self,rig:&LegRig,s:&Stance<'_>,allow_liftoff:bool,paired:bool)->Contact {
        self.step_supported(rig,s,allow_liftoff,paired,false)
    }

    pub(crate) fn step_supported(&mut self,rig:&LegRig,s:&Stance<'_>,allow_liftoff:bool,paired:bool,lock_anchor:bool)->Contact {
        self.step_supported_traced(rig,s,allow_liftoff,paired,lock_anchor,None)
    }

    pub(crate) fn step_supported_traced(&mut self,rig:&LegRig,s:&Stance<'_>,allow_liftoff:bool,paired:bool,lock_anchor:bool,
        trace:Option<&mut AnchorTrace>)->Contact {
        let contact=self.step_supported_inner(rig,s,allow_liftoff,paired,lock_anchor,ONE,None,None,trace);
        if !self.planted() && self.support.readiness==Readiness::Ready {self.pending_support();}
        contact
    }
    #[cfg(test)]
    pub(crate) fn step_ready_traced(&mut self,rig:&LegRig,s:&Stance<'_>,allow_liftoff:bool,paired:bool,lock_anchor:bool,
        carrying:Fx,
        surface:&dyn nav::vehicle::support::Surface,profile:modding::walking_terrain::Capability,
        trace:Option<&mut AnchorTrace>)->Contact {
        self.step_ready_traced_memo(rig,s,allow_liftoff,paired,lock_anchor,carrying,surface,profile,None,trace)
    }
    pub(crate) fn step_ready_traced_memo(&mut self,rig:&LegRig,s:&Stance<'_>,allow_liftoff:bool,paired:bool,lock_anchor:bool,
        carrying:Fx,
        surface:&dyn nav::vehicle::support::Surface,profile:modding::walking_terrain::Capability,
        alt_memo:Option<&AlternativeMemo>,
        trace:Option<&mut AnchorTrace>)->Contact {
        let contact=self.step_supported_inner(rig,s,allow_liftoff,paired,lock_anchor,carrying,Some((surface,profile)),alt_memo,trace);
        if !self.planted() && self.support.readiness==Readiness::Ready {self.pending_support();}
        contact
    }
    fn step_supported_inner(&mut self,rig:&LegRig,s:&Stance<'_>,allow_liftoff:bool,paired:bool,lock_anchor:bool,
        carrying:Fx,
        support:Option<(&dyn nav::vehicle::support::Surface,modding::walking_terrain::Capability)>,
        alt_memo:Option<&AlternativeMemo>,
        mut trace:Option<&mut AnchorTrace>)->Contact {
        let root = s.root;
        if !s.grounded {
            // Keep the joints where they were relative to the falling hull
            // instead of reaching for ground that no longer carries it.
            let (pos, dhead, normal) = *self.local.get_or_insert((rig.ankle, 0, V3::UP));
            self.anchor = None;
            self.swing = false;
            self.base = root.base;
            let c = Contact {
                pos: to_world(root, pos),
                head: s.head.wrapping_add(dhead),
                normal,
            };
            self.contact = Some(c);
            return c;
        }
        let g = s.gait;
        let p = Fx::from_ratio(
            angle::norm(
                s.phase
                    .wrapping_sub(g.offset[rig.support.min(3)].wrapping_add(rig.phase_shift)),
            ),
            angle::FULL,
        );
        let requested_swing = p >= g.duty;
        let reach = rig.l1.add(rig.l2);
        if self.needs_reset(rig,root) {
            let rest = rig.sole.contact(to_world(root, rig.lane_target(rig.ankle)), s.head.wrapping_add(rig.lane.head), s.floor);
            if let Some(t)=trace.as_deref_mut() {t.record(AnchorReason::Reset,self.contact,rest,Some(rest),self.planted(),true);}
            self.anchor = Some(rest);
            self.pending_support();
            self.from = rest;
            self.goal = rest;
            self.swing = false;
            self.progress = ZERO;
            self.cadence = ZERO;
        }
        // Old snapshots may contain an idle zero-progress synthetic swing.
        // It never left its anchor and can start a real step without a pop.
        if self.swing && self.progress == ZERO && self.cadence == ZERO {
            self.swing = false;
        }
        let anchor = self.anchor.unwrap_or(self.from);
        // Start before the planted foot leaves its reachable, reserved area.
        // Waiting for the global phase would drag the sole along the lane
        // boundary, particularly after a fast start or a reversing turn.
        // The biped coordinator keeps the partner down during this swing.
        // This anchor carries only through the existing departing half, not
        // through the entire flight. The motor still proves actual departure
        // reach and the complete support handoff before committing its root.
        // A completed swing stays planted for the rest of the global swing
        // interval. Passing through stance rearms it for the next step.
        if !requested_swing && !self.swing { self.progress = ZERO; }
        let started = allow_liftoff && !self.swing && s.cycles>ZERO &&
            (paired || requested_swing && self.progress<ONE
                || self.lift_urgent_at(rig,s,support.map_or(Fx::from_ratio(98,100),|(_,p)|Fx::from_ratio(p.stance_reach_pct,100)),carrying));
        if started {
            self.from = anchor;
            self.progress = ZERO;
            self.cadence = s.cycles;
            self.swing = true;
            // Predict one complete swing plus half the next stance. A late
            // global phase cannot skip the beginning of this physical step.
            let frac = g.duty.mul(Fx::from_ratio(1, 2)).add(ONE.sub(g.duty));
            // Preserve the bounded impulse/yaw prediction for very slow
            // cadence: an unbounded shove lead can overflow squared reach.
            let lead = |v: i128| v * i128::from(frac.raw()) / i128::from(s.cycles.raw());
            let far = [s.linear.x, s.linear.y, s.linear.z].map(|c| lead(i128::from(c.raw())));
            let limit = i128::from(frac.mul(g.stride).mul(Fx::from_int(2)).raw()).max(0);
            let widest = far.iter().map(|c| c.abs()).max().unwrap_or(0);
            let ahead = far.map(|c| {
                Fx(if widest > limit {
                    c * limit / widest
                } else {
                    c
                } as i64)
            });
            let half = i128::from(angle::FULL / 2);
            let yaw = (lead(i128::from(s.angular) << 32) >> 32).clamp(-half, half) as i32;
            let ankle = to_world(root, rig.ankle).sub(root.base);
            let at = root
                .base
                .add(V3::new(ahead[0], ahead[1], ahead[2]))
                .add(turned(ankle, yaw));
            let until_landing=ONE.sub(g.duty);
            let travel=V3::new(s.linear.x,s.linear.y,s.linear.z).scale(until_landing.div(s.cycles));
            let landed=to_world(root,rig.lane_target(to_local(root,at.sub(travel)))).add(travel);
            self.goal = rig.sole.contact(landed, s.head.wrapping_add(rig.lane.head), s.floor);
            self.clearance = rig.sole_clearance_checked(
                root,
                self.from,
                self.goal,
                g.lift.mul(s.lift_scale.max(ZERO).min(ONE)),
                s.floor,
                support.map(|(_,profile)|profile),
            ).min(reach).min(MAX_CLEARANCE);
            // Terrain edited beyond the passable envelope must not make
            // the stored lift exceed the links or snapshot bounds. The
            // live sole check still handles the local edge.
        }
        let target = if self.swing {
            let span = ONE.sub(g.duty).max(MILLI);
            if !started {
                if s.cycles>ZERO {self.cadence=self.cadence.max(s.cycles);}
                // Positive crawl input must not disable the same finish bound
                // used by next_landing and by a stopped hull.
                let rate=self.finish_rate(s.cycles);
                self.progress = self.progress.add(rate.div(span)).min(ONE);
            }
            // A start/stop or reversing turn can invalidate the landing
            // predicted at liftoff. Refit it against the remaining flight,
            // rather than lowering the hull to reach a stale distant goal.
            let (landing,_)=self.landing_forecast(root,s.linear,s.angular,s.cycles,g);
            let reach=reach.mul(Fx::from_ratio(98,100));
            let step_profile=support.map(|(_,profile)|profile);
            let fits=|c:Contact| {
                let local=to_local(&landing,c.pos);
                rig.lane_target(local).sub(local).len_sq()<=MILLI.mul(MILLI) && rig.contact_reachable(local)
                    && local.sub(rig.hip).len_sq()<=reach.mul(reach)
                    && support.is_none_or(|(_,profile)|rig.step_sole_climbable(&landing,c,s.floor,profile))
            };
            let goal_step_rejected=support.is_some_and(|(_,profile)|!rig.step_sole_climbable(&landing,self.goal,s.floor,profile));
            if !fits(self.goal) {
                let neutral=rig.sole.contact(to_world(&landing,rig.lane_target(rig.ankle)),self.goal.head,s.floor);
                let bracket=if fits(neutral) {
                    Some((neutral,self.goal))
                } else if support.is_some_and(|(_,profile)|!rig.step_sole_climbable(&landing,neutral,s.floor,profile)) {
                    let (cands,len)=rig.retracted_candidates();
                    cands[..len]
                        .iter()
                        .map(|&local|rig.sole.contact(to_world(&landing,local),self.goal.head,s.floor))
                        .find(|&c|fits(c))
                        .map(|retracted|(retracted,neutral))
                } else {
                    None
                };
                if let Some((mut good,mut bad))=bracket {
                    for _ in 0..8 {
                        let middle=rig.sole.contact(good.pos.add(bad.pos).scale(Fx::from_ratio(1,2)),self.goal.head,s.floor);
                        if fits(middle) {good=middle;} else {bad=middle;}
                    }
                    self.goal=good;
                    if goal_step_rejected {
                        self.clearance=rig.sole_clearance_checked(&landing,self.from,self.goal,g.lift.mul(s.lift_scale.max(ZERO).min(ONE)),s.floor,step_profile).min(reach).min(MAX_CLEARANCE);
                    }
                }
            }
            // Placement is an event, not weight admission: forecast callers and
            // execution use this same landing frame, while Ready stays Pending
            // until the actual touchdown is rechecked against live sources.
            let failed_frame_changed=self.placement_frame!=0 && self.placement_frame!=Self::placement_stamp(&landing,s.head);
            if self.placement_checked!=Some(self.goal) || failed_frame_changed {
                if let Some((surface,profile))=support {
                    let coverage=Self::support_count(rig,self.goal,surface,profile);
                    if coverage.is_none() || !fits(self.goal) {
                        if let Some(goal)=self.ready_alternative_memo(rig,&landing,s.head,self.goal,surface,s.floor,profile,alt_memo) {
                            if coverage.is_none() || !fits(self.goal) || Self::support_count(rig,goal,surface,profile).is_some_and(|better|coverage.is_some_and(|n|better>n)) {
                                if let Some(t)=trace.as_deref_mut(){t.record(AnchorReason::SupportAlternative,self.contact,goal,self.contact,false,false);}
                                self.goal=goal;
                                self.clearance=rig.sole_clearance_checked(&landing,self.from,goal,g.lift.mul(s.lift_scale.max(ZERO).min(ONE)),s.floor,Some(profile)).min(reach).min(MAX_CLEARANCE);
                            }
                        }
                    }
                    self.placement_checked=Some(self.goal);
                    self.placement_frame=if fits(self.goal) && Self::ready_at(rig,self.goal,surface,profile) {0}else{Self::placement_stamp(&landing,s.head)};
                }
            }
            if lock_anchor {
                let goal_local=to_local(&landing,self.goal.pos);
                if goal_local.y<rig.ankle.y && !rig.contact_reachable(goal_local) {
                    let solved=rig.contact_ankle(rig.lane_target(goal_local));
                    if solved.y.sub(goal_local.y)>g.lift {
                        let before=self.goal;
                        self.goal.pos=to_world(&landing,solved);
                        if let Some(t)=trace.as_deref_mut(){t.record(AnchorReason::GoalEnvelope,Some(before),self.goal,Some(self.goal),false,false);}
                    }
                }
            }
            let step_guard=support.map(|(_,profile)|(root,profile));
            if started && lock_anchor {self.fit_takeoff(rig,root,s.cycles.div(span),s.floor,step_guard);}
            let q=self.progress;
            let (mut contact,waiting)=self.swing_contact_checked(rig,q,s.floor,step_guard);
            if q >= ONE && waiting {
                let mut goal=self.goal;
                goal.pos.y=rig.sole.height(goal.pos,goal.head,goal.normal,s.floor);
                if let Some((_,profile))=support {
                    if !rig.step_sole_climbable(root,goal,s.floor,profile) {
                        goal=rig.retracted_neutral(root,self.goal.head,s.floor,profile);
                    }
                    if !rig.step_sole_climbable(root,contact,s.floor,profile) {
                        let cap=self.from.pos.y.max(goal.pos.y).add(self.clearance);
                        contact.pos.y=contact.pos.y.min(cap);
                    }
                }
                if let Some(t)=trace.as_deref_mut() {t.record(AnchorReason::LandingBlocked,self.contact,goal,Some(contact),false,false);}
                self.contact=Some(contact);
                self.recover_to(goal);
                self.clearance=rig.sole_clearance_checked(root,self.from,self.goal,
                    g.lift.mul(s.lift_scale.max(ZERO).min(ONE)),s.floor,step_profile).min(reach).min(MAX_CLEARANCE);
            } else if q >= ONE {
                // Keep the selected sole plane. Re-running the least-squares
                // slope here would discard a source-checked horizontal landing.
                contact.pos.y=rig.sole.height(contact.pos,contact.head,contact.normal,s.floor);
                if let Some((_,profile))=support.filter(|(_,profile)|!rig.step_sole_climbable(root,contact,s.floor,*profile)) {
                    let foot_head=s.head.wrapping_add(rig.lane.head);
                    let mut retry_goal=rig.retracted_neutral(root,foot_head,s.floor,profile);
                    let safe_from=self.contact.filter(|c|rig.step_sole_climbable(root,*c,s.floor,profile)).unwrap_or(self.from);
                    let retry_local=to_local(root,retry_goal.pos);
                    if retry_goal.pos.y<safe_from.pos.y && !rig.contact_reachable(retry_local) {
                        retry_goal.pos=to_world(root,rig.contact_ankle(retry_local));
                    }
                    if let Some(t)=trace.as_deref_mut() {t.record(AnchorReason::LandingBlocked,self.contact,retry_goal,Some(safe_from),false,false);}
                    self.contact=Some(safe_from);
                    self.recover_to(retry_goal);
                    self.clearance=rig.sole_clearance_checked(root,self.from,self.goal,
                        g.lift.mul(s.lift_scale.max(ZERO).min(ONE)),s.floor,Some(profile)).min(reach).min(MAX_CLEARANCE);
                    contact=safe_from;
                } else {
                    let local = to_local(root, contact.pos);
                    // The terrain fit may change local X/Z when the hull leans.
                    // A lane-clamped solve would reject a reachable touchdown
                    // forever, starving the supporting partner of its next lift.
                    let solved = rig.contact_ankle(if lock_anchor {local} else {rig.lane_target(local)});
                    if solved.sub(local).len_sq() <= MILLI.mul(MILLI) {
                        self.goal = contact;
                        self.anchor = Some(contact);
                        self.pending_support();
                        self.swing = false;
                        self.cadence = ZERO;
                    } else {
                        // A deep hole can be below the links while other legs
                        // still carry the hull. Keep this foot unplanted and
                        // retry near its own neutral lane as the hull advances.
                        let proposed=contact;
                        contact.pos = to_world(root, solved);
                        if let Some(t)=trace.as_deref_mut() {t.record(AnchorReason::LandingRejected,self.contact,proposed,Some(contact),false,false);}
                        self.from = contact;
                        let foot_head = s.head.wrapping_add(rig.lane.head);
                        let mut retry_goal = if let Some((_,profile)) = support {
                            rig.retracted_neutral(root, foot_head, s.floor, profile)
                        } else {
                            rig.sole.contact(to_world(root, rig.lane_target(rig.ankle)), foot_head, s.floor)
                        };
                        let retry_local = to_local(root, retry_goal.pos);
                        if retry_goal.pos.y < contact.pos.y && !rig.contact_reachable(retry_local) {
                            retry_goal.pos = to_world(root, rig.contact_ankle(retry_local));
                        }
                        self.goal = retry_goal;
                        self.progress = ZERO;
                        self.clearance = ZERO;
                        self.cadence = self.cadence.max(Fx::from_ratio(1, 80));
                    }
                }
            } else if !self.departing() {
                // Once past the departing support-admission half, keep the
                // executed airborne contact inside the physical link reach so
                // hit volumes and riser holds match the rendered ankle.
                let local = to_local(root, contact.pos);
                if !rig.contact_reachable(local) {
                    contact.pos = to_world(root, rig.contact_ankle(local));
                }
            }
            contact
        } else { anchor };
        let local=to_local(root,target.pos);
        let safe=rig.lane_target(local);
        // Lane fitting belongs to placement. Reprojecting an admitted swing
        // here moves/rotates even its q=0 contact away from the saved take-off
        // point, breaking the clearance path. Legacy recovery retains fitting.
        let foot_head=s.head.wrapping_add(rig.lane.head);
        let active=rig.lane.min!=[Fx::MIN;2] || rig.lane.max!=[Fx::MAX;2];
        if active && !lock_anchor && !self.swing && (safe!=local || target.head!=foot_head) {
            let before=target;
            let planted=self.planted();
            let fitted=rig.sole.contact(if safe==local {target.pos}else{to_world(root,safe)},foot_head,s.floor);
            let ledge=rig.sole.width().mul(Fx::from_ratio(1,4)).max(ONE);
            let delta=fitted.pos.sub(target.pos);
            if delta.x!=ZERO || delta.z!=ZERO || fitted.pos.y.sub(target.pos.y).abs()>ledge {
                // The lane is a placement constraint, not permission to move
                // a planted world anchor. Leave that contact through a real
                // recovery swing even on a flat floor or a non-radial rig.
                self.contact=Some(target);
                self.recover_to(fitted);
                self.clearance=rig.sole.clearance(self.from,self.goal,
                    g.lift.mul(s.lift_scale.max(ZERO).min(ONE)),s.floor).min(reach).min(MAX_CLEARANCE);
            }
            // A requested heading alone belongs to the next landing. The
            // planted sole must not rotate with the hull in the meantime.
            if let Some(t)=trace.as_deref_mut() {t.record(AnchorReason::LaneFit,Some(before),fitted,Some(target),planted,self.planted());}
        }
        self.base = root.base;
        self.local = Some((
            to_local(root, target.pos),
            angle::diff(target.head, s.head),
            target.normal,
        ));
        self.contact = Some(target);
        target
    }

    /// Refit a planted contact that left its reachable foot lane, without
    /// advancing a step. A descending landing may need a nearer foothold
    /// while the bounded suspension drop catches up.
    #[cfg(test)]
    pub(crate) fn reproject_stance(&mut self, rig: &LegRig, root: &Frame,
        head: i32, floor: &dyn Fn(Fx, Fx) -> Fx, smooth: bool) {
        self.reproject_stance_traced(rig,root,head,floor,smooth,None)
    }
    pub(crate) fn reproject_stance_traced(&mut self, rig: &LegRig, root: &Frame,
        head: i32, floor: &dyn Fn(Fx, Fx) -> Fx, smooth: bool,mut trace:Option<&mut AnchorTrace>) {
        if !self.planted() {return;}
        let Some(mut c) = self.contact else {return;};
        let original=c;
        for _ in 0..3 {
            let local = to_local(root, c.pos);
            let mut safe = rig.lane_target(local);
            let d = safe.sub(rig.hip);
            let reach = rig.l1.add(rig.l2).mul(Fx::from_ratio(999, 1000));
            if d.len_sq() > reach.mul(reach) {
                let flat = d.x.mul(d.x).add(d.z.mul(d.z));
                if flat > MILLI.mul(MILLI) {
                    let radius = reach.mul(reach).sub(d.y.mul(d.y)).max(ZERO).sqrt();
                    let scale = radius.div(flat.sqrt()).min(ONE);
                    safe.x = rig.hip.x.add(d.x.mul(scale));
                    safe.z = rig.hip.z.add(d.z.mul(scale));
                    safe = rig.lane_target(safe);
                }
            }
            if safe.sub(local).len_sq() <= MILLI.mul(MILLI) {break;}
            let next = rig.sole.contact(to_world(root, safe), c.head, floor);
            let delta=next.pos.sub(c.pos);
            let span=rig.sole.width().mul(Fx::from_ratio(1,4)).max(ONE);
            if delta.x!=ZERO || delta.z!=ZERO || smooth && next.pos.y.add(span)<c.pos.y {
                self.recover_to(next);
                if let Some(t)=trace.as_deref_mut() {t.record(AnchorReason::Reproject,Some(c),next,self.contact,true,false);}
                return;
            }
            if let Some(t)=trace.as_deref_mut() {t.record(AnchorReason::Reproject,Some(c),next,Some(next),true,true);}
            c=next;
        }
        self.contact = Some(c);
        self.anchor = Some(c);
        if c!=original {self.pending_support();}
        self.local = Some((to_local(root, c.pos), angle::diff(c.head, head), c.normal));
        self.base = root.base;
    }

    /// Keep biped landing targets on their side of the hull. A locked stance
    /// and take-off point are immutable; legacy recovery may refit them.
    #[cfg(test)]
    pub(crate) fn keep_walker_side(&mut self, rig: &LegRig, root: &Frame,
        head: i32, spread: Fx, floor: &dyn Fn(Fx, Fx) -> Fx, lock_anchor: bool) {
        self.keep_walker_side_traced(rig,root,head,spread,floor,lock_anchor,false,None)
    }
    pub(crate) fn fit_walker_landing_side(&mut self, rig:&LegRig, s:&Stance<'_>, spread:Fx,
        guided:bool,trace:Option<&mut AnchorTrace>) {
        if !guided {
            self.keep_walker_side_traced(rig,s.root,s.head,spread,s.floor,true,false,trace);
            return;
        }
        if !self.swing {return;}
        let (landing,time)=self.landing_forecast(s.root,s.linear,s.angular,s.cycles,s.gait);
        let yaw=(i128::from(s.angular)*i128::from(time.raw())>>32)
            .clamp(-i128::from(angle::HALF),i128::from(angle::HALF)) as i32;
        let landing_head=angle::norm(s.head.wrapping_add(yaw).wrapping_add(rig.lane.head));
        if self.goal.head!=landing_head {
            // Rotation changes the actual sampled sole plane as well as the
            // ankle orientation. Keep the take-off contact immutable.
            self.goal=rig.sole.contact(self.goal.pos,landing_head,s.floor);
        }
        // Fit the future sole against the arriving torso, then retain the
        // current corridor as well. Fitting only to the current frame makes
        // the same landing turn inward as the torso approaches it.
        self.keep_walker_side_traced(rig,&landing,s.head.wrapping_add(yaw),spread,s.floor,true,false,None);
        self.keep_walker_side_traced(rig,s.root,s.head,spread,s.floor,true,false,trace);
    }

    pub(crate) fn keep_walker_side_traced(&mut self, rig: &LegRig, root: &Frame,
        head: i32, spread: Fx, floor: &dyn Fn(Fx, Fx) -> Fx, lock_anchor: bool,
        placement:bool,trace:Option<&mut AnchorTrace>) {
        if lock_anchor && self.planted() {return;}
        let sign = if rig.ankle.z < ZERO { -1 } else { 1 };
        // Scale with the authored body: Walker 04 is enlarged in-world, so
        // a fixed world-unit gap would be much narrower than either foot.
        let min_side = rig.ankle.z.abs().mul(spread);
        let max_yaw = Self::heading_limit();
        let fix = |mut c: Contact, planted: bool| {
            let mut local = to_local(root, c.pos);
            let original = local;
            let side = local.z.mul(Fx::from_int(sign));
            let relative_head = angle::diff(c.head, head);
            if side < min_side { local.z = min_side.mul(Fx::from_int(sign)); }
            // A sideways step plus torso yaw can carry a planted contact
            // beyond the two links' reach. Keep its height and side, sliding
            // its horizontal target onto the same reach used by the IK solve.
            let reach = rig.l1.add(rig.l2).mul(Fx::from_ratio(999, 1000));
            let d = local.sub(rig.hip);
            if d.len_sq() > reach.mul(reach) {
                let flat_sq = reach.mul(reach).sub(d.y.mul(d.y)).max(ZERO);
                let radius = flat_sq.sqrt();
                let z = d.z.max(radius.neg()).min(radius);
                let x = flat_sq.sub(z.mul(z)).max(ZERO).sqrt();
                local.z = rig.hip.z.add(z);
                local.x = rig.hip.x.add(d.x.max(x.neg()).min(x));
            }
            if local != original || relative_head.abs() > max_yaw {
                let pos = to_world(root, local);
                let limited = head.wrapping_add(relative_head.clamp(-max_yaw, max_yaw));
                let grounded = rig.sole.contact(pos, limited, floor);
                // A stance correction must end on the surface. Keeping the
                // old raised ankle here fabricated a planted foot in the air.
                // Swing contacts retain their lift along the clearance arc.
                c.pos = V3::new(pos.x, if planted {grounded.pos.y} else {pos.y.max(grounded.pos.y)}, pos.z);
                c.head = limited;
                c.normal = grounded.normal;
            }
            c
        };
        if placement {
            // No accepted stance precedes the first placement. Apply the
            // authored biped width before this contact starts carrying load.
            let before=self.contact;
            self.anchor=self.anchor.map(|c|fix(c,true));
            self.from=fix(self.from,true);self.goal=fix(self.goal,true);
            self.contact=self.contact.map(|c|fix(c,true));
            if let Some(c)=self.contact {
                self.local=Some((to_local(root,c.pos),angle::diff(c.head,head),c.normal));
                if let Some(t)=trace {t.record(AnchorReason::Reset,before,c,Some(c),false,true);}
            }
            return;
        }
        if lock_anchor {
            // The next placement may be refitted while an existing stance
            // and the take-off point retain their accepted world contact.
            self.goal = fix(self.goal, true);
            return;
        }
        if let Some(old)=self.contact.filter(|_|self.planted()) {
            let next=fix(old,true);
            if next!=old {
                self.recover_to(next);
                if let Some(t)=trace {t.record(AnchorReason::SideGuard,Some(old),next,self.contact,true,false);}
            }
        }
        // Side fitting selects a future foothold. Current contacts and the
        // saved take-off point remain the executed trajectory in every mode.
        self.goal = fix(self.goal, true);
        if let Some(c) = self.contact {
            self.local = Some((to_local(root, c.pos), angle::diff(c.head, head), c.normal));
        }
    }

    pub fn save(&self, b: &mut blob::Blob) {
        for c in [&self.contact, &self.anchor] {
            b.bool(c.is_some());
            if let Some(c) = c {
                c.save(b);
            }
        }
        self.from.save(b);
        self.goal.save(b);
        b.fx(self.base.x);
        b.fx(self.base.y);
        b.fx(self.base.z);
        b.bool(self.swing);
        b.fx(self.clearance);
        b.fx(self.progress);
        b.fx(self.cadence);
        b.bool(self.local.is_some());
        if let Some((pos, dhead, normal)) = &self.local {
            b.fx(pos.x);
            b.fx(pos.y);
            b.fx(pos.z);
            b.i32(*dhead);
            b.fx(normal.x);
            b.fx(normal.y);
            b.fx(normal.z);
        }
        let proof=self.support;
        b.u8(proof.readiness as u8);b.u8(proof.reason as u8);
        b.bool(proof.source.is_some());
        if let Some(source)=proof.source {b.u8(source.kind);b.u64(source.id);b.u64(source.content);b.u64(source.pose);}
        b.u64(proof.signature);b.u16(proof.supported_mask);b.u8(proof.samples);
        b.bool(self.placement_checked.is_some());
        if let Some(goal)=self.placement_checked {goal.save(b);}
        b.u64(self.placement_frame);
    }

    pub fn load(s: &mut blob::Scan) -> Option<Foot> {
        Self::load_version(s,true)
    }
    pub(crate) fn load_version(s:&mut blob::Scan,live_support:bool)->Option<Foot> {
        let contact = if s.bool() {
            Some(Contact::load(s)?)
        } else {
            None
        };
        let anchor = if s.bool() {
            Some(Contact::load(s)?)
        } else {
            None
        };
        let from = Contact::load(s)?;
        let goal = Contact::load(s)?;
        let base = V3::new(s.fx(), s.fx(), s.fx());
        let swing = s.bool();
        let clearance = s.fx();
        let progress = s.fx();
        let cadence = s.fx();
        let local = if s.bool() {
            let pos = V3::new(s.fx(), s.fx(), s.fx());
            let dhead = s.i32();
            let normal = V3::new(s.fx(), s.fx(), s.fx());
            Some((pos, dhead, normal))
        } else {
            None
        };
        let support=if live_support {
            let readiness=match s.u8(){0=>Readiness::Pending,1=>Readiness::Ready,2=>Readiness::Rejected,_=>return None};
            let reason=match s.u8(){0=>SupportReason::Unchecked,1=>SupportReason::Supported,2=>SupportReason::SourceChanged,
                3=>SupportReason::PartialSole,4=>SupportReason::Moving,5=>SupportReason::Removed,6=>SupportReason::Unsupported,
                7=>SupportReason::Obstructed,8=>SupportReason::Unreachable,9=>SupportReason::Normal,_=>return None};
            let source=if s.bool(){let source=nav::vehicle::support::SupportRef{kind:s.u8(),id:s.u64(),content:s.u64(),pose:s.u64()};
                if source.kind>2{return None;} Some(source)}else{None};
            let proof=SupportEvidence{readiness,reason,source,signature:s.u64(),supported_mask:s.u16(),samples:s.u8()};
            if proof.samples>9 || u32::from(proof.supported_mask)>=(1u32<<proof.samples)
                || readiness==Readiness::Ready && (source.is_none() || proof.supported_mask==0 || anchor.is_none() || swing) {return None;}
            proof
        } else {SupportEvidence::default()};
        let placement_checked=if live_support && s.bool(){Some(Contact::load(s)?)}else{None};
        let placement_frame=if live_support {s.u64()}else{0};
        if placement_checked.is_none() && placement_frame!=0 {return None;}
        if !s.ok()
            || clearance < ZERO
            || clearance > MAX_CLEARANCE
            || progress < ZERO
            || progress > ONE
            || cadence < ZERO
            || cadence > ONE
            || (swing && contact.is_none())
        {
            return None;
        }
        Some(Foot {
            contact,
            support,
            anchor,
            from,
            goal,
            placement_checked,
            placement_frame,
            base,
            swing,
            clearance,
            progress,
            cadence,
            local,
        })
    }
}

/// Pose the segments of a leg for an ankle contact, from this tick's rest
/// frames: each link turns from its rest direction by the shortest arc,
/// the balls ride the joints, the foot takes the sole's axes. Frames of
/// the leg's segments in `frames` are replaced; the others are untouched.
pub fn leg_frames(rig: &LegRig, root: &Frame, contact: &Contact, frames: &mut [Frame]) {
    let pose = rig.solve_contact(to_local(root, contact.pos));
    let hip = to_world(root, rig.hip);
    let knee = to_world(root, pose.knee);
    let ankle = to_world(root, pose.ankle);
    let knee_rest = to_world(root, rig.knee);
    let ankle_rest = to_world(root, rig.ankle);
    let turn = |rest: &Frame, from: V3, to: V3, at: V3| -> Frame {
        let quarter = Fx::from_ratio(1, 4);
        let m = if from.len_sq() > quarter && to.len_sq() > quarter {
            rotation_arc(unit_or(from, X), unit_or(to, X))
        } else {
            [X, V3::UP, Z]
        };
        Frame {
            base: at,
            x: apply(&m, rest.x),
            y: apply(&m, rest.y),
            z: apply(&m, rest.z),
            ..*rest
        }
    };
    if let Some(rest) = frames.get(rig.thigh) {
        let f = turn(rest, knee_rest.sub(hip), knee.sub(hip), hip);
        frames[rig.thigh] = f;
    }
    if let Some(rest) = frames.get(rig.shin) {
        let f = turn(rest, ankle_rest.sub(knee_rest), ankle.sub(knee), knee);
        frames[rig.shin] = f;
    }
    for (part, at) in [(rig.knee_ball, knee), (rig.ankle_ball, ankle)] {
        if let Some(i) = part {
            if let Some(f) = frames.get_mut(i) {
                f.base = at;
            }
        }
    }
    if let Some(i) = rig.foot {
        if let Some(f) = frames.get_mut(i) {
            let [x, y, z] = contact.axes();
            f.base = ankle;
            f.x = x;
            f.y = y;
            f.z = z;
        }
    }
}


// Frozen pre-20 scalar oracle: it deliberately has neither lazy planes nor xz rejection.
#[cfg(test)]
#[derive(Default)]
struct Legacy20Offset {
    fitted: Option<Contact>, flat: Option<Contact>,
    fitted_cnt: Option<Option<usize>>, flat_cnt: Option<Option<usize>>,
    pass1_built: bool, pass1: Vec<(Contact, Option<bool>)>,
}
#[cfg(test)]
struct Legacy20Entry { offsets: [Legacy20Offset; 7] }

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(v: f64) -> Fx {
        Fx::from_ratio((v * 10000.0).round() as i32, 10000)
    }
    fn v(x: f64, y: f64, z: f64) -> V3 {
        V3::new(fx(x), fx(y), fx(z))
    }
    fn near(a: V3, b: V3, tol: f64) -> bool {
        a.sub(b).len_sq().to_f64_lossy().sqrt() < tol
    }
    fn hull(base: V3, head: i32) -> Frame {
        let x = V3::new(angle::cos(head), ZERO, angle::sin(head));
        Frame {
            base,
            x,
            y: V3::UP,
            z: x.cross(V3::UP),
            scale: ONE,
            centre: [ZERO; 3],
        }
    }
    /// A leg like the strider's: hip high, knee out, foot under the hull.
    #[test]
    fn swing_endpoints_preserve_the_committed_contact_exactly() {
        let body=voxel::unit::strider();let rig=rigs_of(&body).remove(0);
        let floor=|x:Fx,z:Fx|x.mul(Fx::from_ratio(1,3)).add(z.mul(Fx::from_ratio(1,7)));
        let goal=rig.sole.contact(V3::ZERO,0,&floor);
        let from=Contact{pos:V3::ZERO,head:0,normal:V3::UP};
        assert_eq!(from.lerp(goal,ONE),goal);
        assert_eq!(goal.lerp(from,ZERO),goal);
    }

    fn rig() -> LegRig {
        let hip = v(-0.9, 2.7, -0.9);
        let knee = v(-1.3, 1.5, -1.3);
        let ankle = v(-1.1, 0.4, -1.1);
        LegRig {
            lane: LegLane::default(),
            hip,
            knee,
            ankle,
            l1: knee.sub(hip).len_sq().sqrt(),
            l2: ankle.sub(knee).len_sq().sqrt(),
            bend: unit_or(v(-1.0, 0.0, -1.0), X),
            rest_axis: unit_or(ankle.sub(hip),V3::UP),
            thigh: 0,
            shin: 1,
            knee_ball: None,
            ankle_ball: None,
            foot: None,
            support: 3,
            phase_shift: 0,
            sole: Sole::point(ankle.y),
            retracted: ([ankle; 6], 0),
        };
        let mut r = LegRig {
            lane: LegLane::default(),
            hip,
            knee,
            ankle,
            l1: knee.sub(hip).len_sq().sqrt(),
            l2: ankle.sub(knee).len_sq().sqrt(),
            bend: unit_or(v(-1.0, 0.0, -1.0), X),
            rest_axis: unit_or(ankle.sub(hip), V3::UP),
            thigh: 0,
            shin: 1,
            knee_ball: None,
            ankle_ball: None,
            foot: None,
            support: 3,
            phase_shift: 0,
            sole: Sole::point(ankle.y),
            retracted: ([ankle; 6], 0),
        };
        r.retracted = r.compute_retracted_candidates();
        r
    }
    fn gait() -> Gait {
        Gait {
            stride: fx(0.4),
            duty: fx(0.7),
            lift: fx(0.15),
            offset: [0; 4],
        }
    }
    fn flat(_: Fx, _: Fx) -> Fx {
        ZERO
    }

    struct LiveSurface {
        source:std::cell::Cell<nav::vehicle::support::SupportRef>,
        validity:std::cell::Cell<nav::vehicle::support::Validity>,
        partial:std::cell::Cell<bool>,
        centre:V3,
        queries:std::cell::Cell<usize>,
    }
    impl nav::vehicle::support::Surface for LiveSurface {
        fn sample(&self,x:Fx,_y:Fx,z:Fx)->nav::vehicle::support::Sample {
            self.queries.set(self.queries.get()+1);
            let supported=!self.partial.get() || x==self.centre.x && z==self.centre.z;
            nav::vehicle::support::Sample {height:if supported {ZERO}else{Fx::from_int(-10)},source:self.source.get(),validity:self.validity.get()}
        }
        fn validity(&self,old:nav::vehicle::support::SupportRef)->nav::vehicle::support::Validity {
            use nav::vehicle::support::Validity;
            if old.id!=self.source.get().id {Validity::Removed}
            else if old.pose!=self.source.get().pose {Validity::Moving}else{self.validity.get()}
        }
    }
    fn live_fixture()->(LegRig,Frame,Foot,LiveSurface,modding::walking_terrain::Capability) {
        let mut r=rig();let depth=r.ankle.y;
        r.sole=Sole {probes:(-1..=1).flat_map(|x|(-1..=1).map(move|z|V3::new(Fx::from_ratio(x,5),depth.neg(),Fx::from_ratio(z,5)))).collect()};
        let contact=Contact{pos:r.ankle,head:0,normal:V3::UP};
        let foot=Foot{contact:Some(contact),anchor:Some(contact),from:contact,goal:contact,..Default::default()};
        let surface=LiveSurface{source:std::cell::Cell::new(nav::vehicle::support::SupportRef{kind:1,id:7,content:1,pose:1}),
            validity:std::cell::Cell::new(nav::vehicle::support::Validity::Supported),partial:std::cell::Cell::new(false),centre:r.ankle,queries:std::cell::Cell::new(0)};
        let profile=modding::walking_terrain::Tuning::from_ron(include_str!("../../../data/walking-terrain.ron")).unwrap().biped;
        (r,hull(V3::ZERO,0),foot,surface,profile)
    }
    fn proof_roundtrip(foot:&Foot) {
        let mut bytes=blob::Blob::new();foot.save(&mut bytes);
        let copy=Foot::load(&mut blob::Scan::new(&bytes.v)).unwrap();assert_eq!(&copy,foot);
    }
    #[test]
    fn support_legacy_geometry_loads_pending_and_earns_fresh_ready() {
        let (rig,root,mut foot,surface,profile)=live_fixture();
        foot.assess_support(&rig,&root,0,&surface,&flat,profile,gait().lift,false,None);
        assert!(foot.ready());
        // The pre-80 wire contract, deliberately without live support fields.
        let mut old=blob::Blob::new();
        for c in [foot.contact,foot.anchor] {old.bool(c.is_some());if let Some(c)=c {c.save(&mut old);}}
        foot.from.save(&mut old);foot.goal.save(&mut old);
        for v in [foot.base.x,foot.base.y,foot.base.z] {old.fx(v);}
        old.bool(foot.swing);old.fx(foot.clearance);old.fx(foot.progress);old.fx(foot.cadence);
        old.bool(foot.local.is_some());
        if let Some((p,h,n))=foot.local {
            for v in [p.x,p.y,p.z] {old.fx(v);}old.i32(h);
            for v in [n.x,n.y,n.z] {old.fx(v);}
        }
        let mut copy=Foot::load_version(&mut blob::Scan::new(&old.v),false).unwrap();
        assert_eq!(copy.contact,foot.contact);assert!(copy.planted());assert!(!copy.ready());
        assert_eq!(copy.support,SupportEvidence::default());
        copy.assess_support(&rig,&root,0,&surface,&flat,profile,gait().lift,true,None);
        assert!(copy.ready());proof_roundtrip(&copy);
    }

    #[test]
    fn support_failed_placement_retries_on_forecast_pose_change_and_restores() {
        use nav::vehicle::support::Validity;
        let (rig,root,mut foot,surface,profile)=live_fixture();let g=gait();
        surface.validity.set(Validity::Unsupported);foot.recover_to(foot.contact.unwrap());
        let stance=Stance{grounded:true,root:&root,head:0,phase:0,lift_scale:ONE,linear:V3::ZERO,angular:0,cycles:ZERO,gait:&g,floor:&flat};
        foot.step_ready_traced(&rig,&stance,false,false,true,ONE,&surface,profile,None);
        let queries=surface.queries.get();assert!(queries>0);assert!(!foot.ready());
        foot.step_ready_traced(&rig,&stance,false,false,true,ONE,&surface,profile,None);
        assert_eq!(surface.queries.get(),queries,"unchanged failed placement repeated its source search");
        let mut bytes=blob::Blob::new();foot.save(&mut bytes);
        let mut copy=Foot::load(&mut blob::Scan::new(&bytes.v)).unwrap();assert_eq!(copy,foot);
        let mut moved=root;moved.base.x=moved.base.x.add(Fx::from_ratio(1,100));
        let stance=Stance{root:&moved,..stance};surface.validity.set(Validity::Supported);
        for f in [&mut foot,&mut copy] {f.step_ready_traced(&rig,&stance,false,false,true,ONE,&surface,profile,None);}
        assert!(surface.queries.get()>queries,"new forecast did not retry the failed placement");
        assert_eq!(copy,foot);assert!(!foot.ready(),"a placement forecast voted Ready before touchdown");
        for _ in 0..80 {
            for f in [&mut foot,&mut copy] {
                f.step_ready_traced(&rig,&stance,false,false,true,ONE,&surface,profile,None);
                if f.planted(){f.assess_support(&rig,&moved,0,&surface,&flat,profile,g.lift,false,None);}
            }
            assert_eq!(copy,foot);
            if foot.ready(){break;}
        }
        assert!(foot.ready());
    }

    #[test]
    fn support_a_carrier_waking_mid_swing_never_inherits_predicted_ready() {
        use nav::vehicle::support::Validity;
        let (rig,root,mut foot,surface,profile)=live_fixture();let g=gait();
        foot.assess_support(&rig,&root,0,&surface,&flat,profile,g.lift,false,None);
        foot.recover_to(foot.contact.unwrap());
        let stance=Stance{grounded:true,root:&root,head:0,phase:0,lift_scale:ONE,linear:V3::ZERO,angular:0,cycles:ZERO,gait:&g,floor:&flat};
        foot.step_ready_traced(&rig,&stance,false,false,true,ONE,&surface,profile,None);
        assert!(foot.placement_checked.is_some());assert!(!foot.ready());
        surface.validity.set(Validity::Moving);let mut rejected=false;
        for _ in 0..80 {
            foot.step_ready_traced(&rig,&stance,false,false,true,ONE,&surface,profile,None);
            if foot.planted(){foot.assess_support(&rig,&root,0,&surface,&flat,profile,g.lift,false,None);}
            assert!(!foot.ready());rejected|=foot.support.reason==SupportReason::Moving;
        }
        assert!(rejected);proof_roundtrip(&foot);
        surface.validity.set(Validity::Supported);
        for _ in 0..80 {
            foot.step_ready_traced(&rig,&stance,false,false,true,ONE,&surface,profile,None);
            if foot.planted(){foot.assess_support(&rig,&root,0,&surface,&flat,profile,g.lift,false,None);}
            if foot.ready(){break;}
        }
        assert!(foot.ready());proof_roundtrip(&foot);
    }
    #[test]
    fn support_ready_requires_occupied_sole_coverage_and_survives_restore() {
        let (rig,root,mut foot,surface,profile)=live_fixture();
        assert!(foot.planted());assert!(!foot.ready());proof_roundtrip(&foot);
        foot.assess_support(&rig,&root,0,&surface,&flat,profile,gait().lift,false,None);
        assert!(foot.ready());assert_eq!(foot.support.supported_mask.count_ones(),9);proof_roundtrip(&foot);
        surface.partial.set(true);let before=foot.contact;
        foot.assess_support(&rig,&root,0,&surface,&flat,profile,gait().lift,true,None);
        assert!(!foot.ready());assert!(!foot.planted());assert_eq!(foot.contact,before);
        assert_eq!(foot.support.reason,SupportReason::PartialSole);assert_eq!(foot.support.supported_mask.count_ones(),1);proof_roundtrip(&foot);
    }
    #[test]
    fn support_profile_admits_a_voxel_step_strip_without_inventing_full_coverage() {
        struct Strip {points:Vec<(Fx,Fx)>,source:nav::vehicle::support::SupportRef}
        impl nav::vehicle::support::Surface for Strip {
            fn sample(&self,x:Fx,_:Fx,z:Fx)->nav::vehicle::support::Sample {
                nav::vehicle::support::Sample {height:if self.points.contains(&(x,z)){ZERO}else{Fx::from_int(-10)},source:self.source,
                    validity:nav::vehicle::support::Validity::Supported}
            }
            fn validity(&self,_:nav::vehicle::support::SupportRef)->nav::vehicle::support::Validity {nav::vehicle::support::Validity::Supported}
        }
        for count in [2,3] {
            let (rig,root,mut foot,full,profile)=live_fixture();
            let points=(-1..=1).take(count).map(|z|(rig.ankle.x,rig.ankle.z.add(Fx::from_ratio(z,5)))).collect();
            let strip=Strip{points,source:full.source.get()};
            foot.assess_support(&rig,&root,0,&strip,&flat,profile,gait().lift,false,None);
            assert_eq!(foot.ready(),count==3);assert_eq!(foot.support.supported_mask.count_ones(),count as u32);
            assert_ne!(foot.support.supported_mask,511,"partial evidence must remain partial");proof_roundtrip(&foot);
        }
    }
    #[test]
    fn support_equal_height_revision_wake_and_removed_sources_are_explicit() {
        use nav::vehicle::support::Validity;
        for event in 0..4 {
            let (rig,root,mut foot,surface,profile)=live_fixture();
            foot.assess_support(&rig,&root,0,&surface,&flat,profile,gait().lift,false,None);assert!(foot.ready());
            let before=foot.contact;let mut source=surface.source.get();
            match event {0=>source.content+=1,1=>surface.validity.set(Validity::Moving),2=>source.id+=1,_=>source.pose+=1}
            surface.source.set(source);
            foot.assess_support(&rig,&root,0,&surface,&flat,profile,gait().lift,true,None);
            assert_eq!(foot.contact,before,"event {event} moved an anchor");
            if event==0 {
                assert!(foot.ready());assert_eq!(foot.support.reason,SupportReason::SourceChanged);
                assert_eq!(foot.support.source,Some(source));assert_eq!(surface.queries.get(),18);
            } else {
                assert!(!foot.ready());assert!(!foot.planted());
                assert_eq!(foot.support.reason,if event==2 {SupportReason::Removed}else{SupportReason::Moving});
            }
            proof_roundtrip(&foot);
        }
    }

    #[test]
    fn identity_transforms_preserve_full_sole_probe_coordinates() {
        let original_turn=|p:V3,head| {
            let (c,s)=(angle::cos(head),angle::sin(head));
            V3::new(p.x.mul(c).sub(p.z.mul(s)),p.y,p.x.mul(s).add(p.z.mul(c)))
        };
        let original_arc=|a:V3,b:V3| {
            let v=a.cross(b);let k=ONE.add(a.dot(b));
            if k<=Fx::from_ratio(1,64) {return [X,V3::UP,Z];}
            let inv=ONE.div(k);
            let image=|w:V3|{let vw=v.cross(w);w.add(vw).add(v.cross(vw).scale(inv))};
            [image(X),image(V3::UP),image(Z)]
        };
        for a in [X,V3::UP,Z,V3::UP.scale(ONE.neg())] {
            for b in [a,a.scale(Fx::from_int(2)),a.scale(ONE.neg()),X,V3::UP,Z] {
                assert_eq!(rotation_arc(a,b),original_arc(a,b));
            }
        }
        let terrain=|x:Fx,z:Fx|Fx::from_int(x.to_int_trunc().rem_euclid(3)+z.to_int_trunc().rem_euclid(5));
        for rig in rigs_of(&voxel::unit::spider()) {
            for head in [0,angle::FULL/4,5731] {
                for normal in [V3::UP,unit_or(V3::new(ONE,ONE,ONE),V3::UP)] {
                    let matrix=original_arc(V3::UP,normal);
                    let points:Vec<_>=rig.sole.probes.iter().map(|p| {
                        let offset=original_turn(apply(&matrix,*p),head);
                        (rig.ankle.x.add(offset.x),rig.ankle.z.add(offset.z),offset.y)
                    }).collect();
                    let expected=points.iter().map(|&(x,z,y)|terrain(x,z).sub(y)).max().unwrap();
                    let observed=std::cell::RefCell::new(Vec::new());
                    let floor=|x,z|{observed.borrow_mut().push((x,z));terrain(x,z)};
                    assert_eq!(rig.sole.height(rig.ankle,head,normal,&floor),expected);
                    assert_eq!(*observed.borrow(),points.iter().map(|&(x,z,_)|(x,z)).collect::<Vec<_>>());
                }
            }
        }
    }

    #[test]
    fn alternative_nominal_full_sole_checks_every_probe_once_and_never_admits_moving() {
        let (rig,root,foot,surface,profile)=live_fixture();
        let queries=std::cell::Cell::new(0);
        let floor=|_:Fx,_:Fx|{queries.set(queries.get()+1);ZERO};
        let contact=foot.contact.unwrap();
        let goal=foot.ready_alternative(&rig,&root,0,contact,&surface,&floor,profile).unwrap();
        assert_eq!(goal,contact,"retain the full-search first optimal contact");
        assert_eq!(surface.queries.get(),rig.sole.probes.len(),"every occupied probe must earn support");
        assert_eq!(queries.get(),rig.sole.probes.len(),"do not refit the identical horizontal sole or dominated offsets");
        assert!(!foot.ready(),"a proposal cannot grant Ready before actual touchdown");
        surface.validity.set(nav::vehicle::support::Validity::Moving);
        assert!(foot.ready_alternative(&rig,&root,0,contact,&surface,&floor,profile).is_none());
        surface.validity.set(nav::vehicle::support::Validity::Supported);
        surface.partial.set(true);
        assert!(foot.ready_alternative(&rig,&root,0,contact,&surface,&floor,profile).is_none(),"partial nominal support cannot take the full-sole shortcut");
        let memo=AlternativeMemo::default();
        assert!(foot.ready_alternative_memo(&rig,&root,0,contact,&surface,&floor,profile,Some(&memo)).is_none());
        let (floor_after_first,surf_after_first)=(queries.get(),surface.queries.get());
        let shifted=hull(V3::new(Fx::from_ratio(1,10),Fx::from_ratio(1,10),ZERO),0);
        assert!(foot.ready_alternative_memo(&rig,&shifted,0,contact,&surface,&floor,profile,Some(&memo)).is_none());
        assert_eq!((queries.get(),surface.queries.get()),(floor_after_first,surf_after_first),
            "memoized rejected alternatives must not repeat sole or surface queries across shifted landing frames");
    }

    #[test]
    fn alternative_memo_distinguishes_sole_geometry_and_lane_heading_at_shared_hip() {
        let (rig,root,foot,surface,profile)=live_fixture();
        let floor=|_:Fx,_:Fx|ZERO;let contact=foot.contact.unwrap();
        for different_sole in [true,false] {
            let mut other=rig.clone();
            if different_sole {for p in &mut other.sole.probes {p.y=p.y.sub(Fx::from_ratio(1,10));}}
            else {other.lane.head=angle::FULL/4;}
            let memo=AlternativeMemo::default();
            let first=foot.ready_alternative_memo(&rig,&root,0,contact,&surface,&floor,profile,Some(&memo));
            let expected=foot.ready_alternative_memo(&other,&root,0,contact,&surface,&floor,profile,None);
            assert!(expected.is_some());assert_ne!(expected,first,"fixture must distinguish the actual candidate");
            let actual=foot.ready_alternative_memo(&other,&root,0,contact,&surface,&floor,profile,Some(&memo));
            assert_eq!(actual,expected,"memo must not share terrain candidates across different soles/lanes");
        }
    }

    #[test]
    fn touchdown_endpoint_matches_full_ik_for_real_rigs_and_reach_limits() {
        for body in [voxel::unit::spider(),voxel::unit::strider(),voxel::unit::walker04(),voxel::unit::commander()] {
            let rigs=rigs_of(&body);assert!(!rigs.is_empty());
            for rig in rigs {
                let reach=rig.l1.add(rig.l2);
                let mut targets=vec![rig.hip,rig.ankle,rig.knee];
                for scale in [Fx::from_ratio(1,1000),Fx::from_ratio(1,2),ONE,Fx::from_int(2)] {
                    for direction in [V3::UP,V3::new(ONE,ZERO,ZERO),V3::new(ZERO,ONE.neg(),ONE)] {
                        targets.push(rig.hip.add(direction.scale(reach.mul(scale))));
                    }
                }
                for target in targets {
                    assert_eq!(rig.contact_ankle(target),rig.solve_contact(target).ankle);
                    assert_eq!(rig.contact_ankle(rig.lane_target(target)),rig.solve(target).ankle);
                }
            }
        }
    }

    #[test]
    fn real_spider_radial_lane_fit_never_moves_a_planted_world_anchor() {
        let body=voxel::unit::spider();
        let rig=rigs_of(&body).remove(1);
        assert_ne!(rig.lane.head,0,"exercise the shipped radial rig");
        let g=gait();let root=hull(V3::ZERO,0);let mut foot=Foot::default();
        let stance=Stance{grounded:true,root:&root,head:0,phase:0,lift_scale:ONE,
            linear:V3::ZERO,angular:0,cycles:ZERO,gait:&g,floor:&flat};
        let original=foot.step_with_liftoff(&rig,&stance,false);
        let mut trace=AnchorTrace::default();trace.leg=1;
        for head in [637,1274,1911,0,-637] {
            let root=hull(V3::ZERO,head);
            let stance=Stance{root:&root,head,..stance};
            let plain=foot.clone();let mut control=plain.clone();
            control.step_supported(&rig,&stance,false,false,false);
            foot.step_supported_traced(&rig,&stance,false,false,false,Some(&mut trace));
            assert_eq!(foot,control,"reason tracing changed physical foot state");
            foot.reproject_stance_traced(&rig,&root,head,&flat,true,Some(&mut trace));
            if head==637 {
                assert!(!foot.planted(),"lane transfer must be an explicit recovery");
                assert_eq!(foot.contact,Some(original),"recovery must depart from the actual old contact");
                assert!(rig.contact_reachable(to_local(&root,original.pos)),"do not retain an unreachable origin");
            }
            if foot.planted() {
                assert_eq!(foot.contact,Some(original),"silent radial stance refit: {:?}",trace.events());
            }
        }
        for _ in 0..64 {
            let mut fresh=Foot::default();fresh.step_with_liftoff(&rig,&stance,false);
            let root=hull(V3::ZERO,637);let turned=Stance{root:&root,head:637,..stance};
            let mut control=fresh.clone();control.step_supported(&rig,&turned,false,false,false);
            fresh.step_supported_traced(&rig,&turned,false,false,false,Some(&mut trace));
            assert_eq!(fresh,control,"a saturated trace changed physical state");
        }
        assert_eq!(trace.events().len(),AnchorTrace::LIMIT);
        assert!(trace.dropped>0,"exercise the fixed diagnostic bound");
    }

    #[test]
    fn real_spider_lift_through_hip_height_keeps_authored_knee_branch() {
        let rig=rigs_of(&voxel::unit::spider()).remove(6);
        // The live explosion continuation raises this free foot through the
        // hip plane. A horizontal pole sends the knee to the opposite side;
        // this is not a reach failure or a planted-contact adjustment.
        let target=V3::new(Fx::from_int(58),Fx::from_int(72),Fx::from_int(-70));
        let rest=rig.ankle.sub(rig.knee);
        let delta=target.sub(rig.hip);let distance=delta.len_sq().sqrt();
        let axis=delta.scale(ONE.div(distance));
        let a=rig.l1.mul(rig.l1).sub(rig.l2.mul(rig.l2)).add(distance.mul(distance)).div(distance.mul(Fx::from_int(2)));
        let h=rig.l1.mul(rig.l1).sub(a.mul(a)).max(ZERO).sqrt();
        let flat=unit_or(V3::new(rig.bend.x,ZERO,rig.bend.z),X);
        let old_knee=rig.hip.add(axis.scale(a)).add(unit_or(flat.sub(axis.scale(flat.dot(axis))),V3::UP).scale(h));
        assert!(target.sub(old_knee).dot(rest)<ZERO,"exercise the old branch reversal");
        let mut previous=None;
        for quarter in 180..=296 {
            let target=V3::new(target.x,Fx::from_ratio(quarter,4),target.z);
            let pose=rig.solve_contact(target);
            assert!(near(pose.ankle,target,0.001),"reachable swing contact moved");
            assert!(pose.ankle.sub(pose.knee).dot(rest)>ZERO,"lower link reversed at {target:?}");
            assert!(pose.ankle.y<pose.knee.y,"foot curled above its knee at {target:?}");
            if let Some(before)=previous {
                assert!(pose.knee.sub(before).len_sq()<ONE,"knee branch jumped at {target:?}");
            }
            previous=Some(pose.knee);
        }
    }

    #[test]
    fn a_high_moving_support_recovery_does_not_add_the_descent_as_a_second_lift() {
        let rig=rigs_of(&voxel::unit::spider()).remove(7);
        // Captured free-foot geometry after rejecting a moving support. The
        // former arc raised the ankle beyond the thigh's maximum height, so
        // changing only the drawn knee could never repair this trajectory.
        let from=Contact{pos:V3::new(Fx::from_int(64),Fx::from_int(80),Fx::from_int(65)),head:0,normal:V3::UP};
        let goal=Contact{pos:rig.ankle,head:0,normal:V3::UP};
        let mut foot=Foot::default();foot.contact=Some(from);foot.recover_to(goal);
        foot.support.readiness=Readiness::Rejected;foot.support.reason=SupportReason::Moving;
        foot.clearance=Fx::from_int(67);
        let q=Fx::from_ratio(3,10);let eased=q.mul(q).mul(Fx::from_int(3).sub(q.mul(Fx::from_int(2))));
        let old_y=from.pos.y.add(goal.pos.y.sub(from.pos.y).mul(eased)).add(swing_arc(q).mul(foot.clearance));
        assert!(old_y>rig.hip.y.add(rig.l1),"negative control must require an impossible downward shin");
        for i in 0..=64 {
            let target=foot.swing_position(Fx::from_ratio(i,64));
            let posed=rig.solve_contact(target);
            assert!(near(posed.ankle,target,0.001),"recovery exceeded the real links");
            assert!(posed.ankle.y<=posed.knee.y,"descending recovery curled its shin upward");
        }
        assert_eq!(foot.swing_position(ZERO),from.pos);assert_eq!(foot.swing_position(ONE),goal.pos);
        assert_eq!(foot.from,from);assert!(!foot.planted());assert!(!foot.ready());
        assert_eq!(foot.support.reason,SupportReason::Moving);
    }

    #[test]
    fn a_locked_liftoff_retains_its_origin_and_lifts_without_a_lane_snap() {
        let mut r=rig();r.lane.min[0]=r.ankle.x;r.lane.max[0]=r.ankle.x.add(ONE);
        let g=gait();let old=hull(V3::ZERO,0);let mut foot=Foot::default();
        let standing=Stance{grounded:true,root:&old,head:0,phase:0,lift_scale:ONE,
            linear:V3::ZERO,angular:0,cycles:ZERO,gait:&g,floor:&flat};
        let contact=foot.step_supported(&r,&standing,false,false,true);
        assert!(foot.planted());
        let head=angle::FULL/180;let root=hull(v(0.2,0.0,0.0),head);
        let walking=Stance{grounded:true,root:&root,head,phase:angle::FULL*4/5,lift_scale:ONE,
            linear:v(0.2,0.0,0.0),angular:head,cycles:fx(0.02),gait:&g,floor:&flat};
        foot.step_supported(&r,&walking,true,false,true);
        assert!(!foot.planted(),"a test that never starts a step proves no handoff");
        assert_eq!(foot.from,contact,"a new swing must retain its exact take-off origin");
        let lifted=foot.contact.unwrap();
        assert_eq!(lifted,contact,"a reachable take-off retains its exact initial contact");
        assert!(angle::diff(lifted.head,contact.head).abs()<head,
            "the sole rotates along its path instead of snapping to the torso");
        let local=to_local(&root,lifted.pos);
        assert_ne!(r.lane_target(local),local,"the departing sole may trail its moving lane while lifting");
    }

    #[test]
    fn an_overextended_takeoff_uses_the_earliest_reachable_arc_sample() {
        let mut r=rig();r.lane.min[0]=r.ankle.x;r.lane.max[0]=r.ankle.x.add(ONE);
        let g=gait();let old=hull(V3::ZERO,0);let mut foot=Foot::default();
        let standing=Stance{grounded:true,root:&old,head:0,phase:0,lift_scale:ONE,
            linear:V3::ZERO,angular:0,cycles:ZERO,gait:&g,floor:&flat};
        let origin=foot.step_supported(&r,&standing,false,false,true);
        let root=hull(v(1.0,0.0,0.0),0);
        assert!(!r.contact_reachable(to_local(&root,origin.pos)));
        let walking=Stance{grounded:true,root:&root,head:0,phase:angle::FULL*4/5,lift_scale:ONE,
            linear:v(1.0,0.0,0.0),angular:0,cycles:fx(0.2),gait:&g,floor:&flat};
        foot.step_supported(&r,&walking,true,false,true);
        assert_eq!(foot.from,origin);
        assert!(!foot.planted());
        assert!(foot.progress>ZERO && foot.progress<Fx::from_ratio(1,2));
        let c=foot.contact.unwrap();
        assert!(c.pos.y>origin.pos.y);
        assert!(r.contact_reachable(to_local(&root,c.pos)));
        let local=to_local(&root,c.pos);
        assert!(r.solve_contact(local).ankle.sub(local).len_sq()<=MILLI.mul(MILLI));
    }
    fn radians(r: f64) -> i32 {
        (r / std::f64::consts::TAU * f64::from(angle::FULL)).round() as i32
    }
    fn phase(cycles: f64) -> i32 {
        angle::norm((cycles * f64::from(angle::FULL)).round() as i32)
    }

    #[test]
    fn the_shortest_arc_takes_the_first_vector_onto_the_second() {
        let a = unit_or(v(0.2, 1.0, -0.3), V3::UP);
        let b = unit_or(v(-0.5, 0.6, 0.4), V3::UP);
        let m = rotation_arc(a, b);
        assert!(near(apply(&m, a), b, 1e-4));
        // Orthonormal: the images of the axes stay unit and perpendicular.
        for i in 0..3 {
            assert!((m[i].len_sq().to_f64_lossy() - 1.0).abs() < 1e-4);
            assert!(m[i].dot(m[(i + 1) % 3]).to_f64_lossy().abs() < 1e-4);
        }
        assert!(near(apply(&rotation_arc(a, a), X), X, 1e-6));
    }

    #[test]
    fn the_two_link_solve_reproduces_the_rest_knee_of_every_shipped_leg() {
        let mut checked = 0;
        for (_, b) in voxel::roster::walking() {
            for rig in rigs_of(&b) {
                let pose = rig.solve(rig.ankle);
                assert!(
                    near(pose.knee, rig.knee, 0.02),
                    "{:?} vs {:?}",
                    pose.knee,
                    rig.knee
                );
                assert!(near(pose.ankle, rig.ankle, 0.001));
                // A target beyond the reach is pulled onto it.
                let far = rig.solve(rig.hip.add(v(0.0, -90.0, 0.0)));
                let l = far.ankle.sub(rig.hip).len_sq().sqrt();
                let reach = rig.l1.add(rig.l2);
                assert!(l <= reach && l > reach.mul(fx(0.99)), "{l:?} of {reach:?}");
                checked += 1;
            }
        }
        assert!(checked >= 4);
    }

    #[test]
    fn shipped_soles_use_occupied_cells_and_reproduce_the_rest_height() {
        let mut checked = 0;
        for (_, b) in voxel::roster::walking() {
            for (leg, rig) in b.legs.iter().zip(rigs_of(&b)) {
                let Some(i) = leg.foot else {
                    continue;
                };
                let part = &b.segments[usize::from(i)];
                assert!((4..=9).contains(&rig.sole.probes().len()));
                let k = Fx::from_ratio(i32::from(part.body.mm), i32::from(voxel::unit::WORLD_MM));
                for p in rig.sole.probes() {
                    let q = [
                        p.x.div(k).add(Fx::from_int(i32::from(part.attach.pivot.0))),
                        p.y.div(k).add(Fx::from_int(i32::from(part.attach.pivot.1))),
                        p.z.div(k).add(Fx::from_int(i32::from(part.attach.pivot.2))),
                    ];
                    assert!(part.body.vol.get(
                        q[0].to_int_trunc() as usize,
                        q[1].add(Fx::from_ratio(1, 2)).to_int_trunc() as usize,
                        q[2].to_int_trunc() as usize
                    ));
                }
                let c = rig.sole.contact(V3::ZERO, 0, &flat);
                let want = Fx::from_int(i32::from(leg.ankle.1)).mul(k);
                assert!(
                    (c.pos.y.sub(want)).abs() < Fx::from_ratio(1, 1000),
                    "{:?}",
                    c
                );
                assert_eq!(c.normal, V3::UP);
                checked += 1;
            }
        }
        assert!(checked >= 4);
    }

    #[test]
    fn a_sloping_sole_matches_the_plane_without_penetration() {
        let sole = Sole {
            probes: [-0.4, 0.0, 0.4]
                .into_iter()
                .flat_map(|x| [-0.25, 0.0, 0.25].map(|z| v(x, -0.15, z)))
                .collect(),
        };
        let floor = |x: Fx, z: Fx| fx(0.3).mul(x).sub(fx(0.2).mul(z));
        for head in [0, radians(0.7), radians(2.8)] {
            let c = sole.contact(v(2.0, 0.0, 4.0), head, &floor);
            let [_, up, _] = c.axes();
            let want = unit_or(v(-0.3, 1.0, 0.2), V3::UP);
            assert!(near(up, want, 1e-3), "{up:?} vs {want:?}");
            let m = rotation_arc(V3::UP, c.normal);
            for p in sole.probes() {
                let w = c.pos.add(turned(apply(&m, *p), head));
                let gap = w.y.sub(floor(w.x, w.z)).to_f64_lossy();
                assert!(gap.abs() < 2e-3, "probe {w:?} off the plane by {gap}");
            }
        }
    }

    #[test]
    fn a_planted_foot_keeps_its_world_contact_during_turns() {
        let r = rig();
        let g = gait();
        let mut foot = Foot::default();
        let mut first = None;
        for k in 0..24 {
            let base = v(f64::from(k) * 0.004, 0.0, 0.0);
            let head = radians(f64::from(k) * 0.008);
            let root = hull(base, head);
            let s = Stance {
                grounded: true,
                root: &root,
                head,
                phase: phase(f64::from(k) * 0.02),
                lift_scale: ONE,
                linear: V3::ZERO,
                angular: 0,
                cycles: ZERO,
                gait: &g,
                floor: &flat,
            };
            let c = foot.step(&r, &s);
            let want = *first.get_or_insert(c);
            assert_eq!(c, want, "planted foot slid at tick {k}");
            assert_eq!(c.head, 0);
            assert!(foot.planted());
        }
    }

    #[test]
    fn lost_support_recovers_without_a_pop_or_an_unreachable_plant_and_restores() {
        let r = rig(); let g = gait(); let root = hull(V3::ZERO, 0);
        let mut foot = Foot::default();
        let stance = |root, floor| Stance { grounded: true, root, head: 0, phase: 0,
            lift_scale: ZERO, linear: V3::ZERO, angular: 0, cycles: ZERO, gait: &g, floor };
        let original = foot.step(&r, &stance(&root, &flat));
        let unchanged = foot.clone();
        foot.refresh_support(&r, &root, 0, &flat);
        assert_eq!(foot, unchanged, "an unchanged surface changed saved foot state");
        let deep = |_: Fx, _: Fx| Fx::from_int(-1);
        foot.refresh_support(&r, &root, 0, &deep);
        assert_eq!(foot.contact, Some(original), "dirty support teleported the ankle");
        assert!(!foot.planted(), "removed ground remained a support");
        let mut copy = None;
        let lowered = hull(v(0.0, -1.0, 0.0), 0);
        for tick in 0..100 {
            let root = if tick < 50 {&root} else {&lowered};
            foot.step(&r, &stance(root, &deep));
            if tick < 50 { assert!(!foot.planted(), "planted below the leg's reachable envelope"); }
            if let Some(other) = copy.as_mut() {
                let other: &mut Foot = other;
                other.step(&r, &stance(root, &deep));
                assert_eq!(&foot, other, "recovery restore tick {tick}");
            } else if tick == 31 {
                let mut b = blob::Blob::new(); foot.save(&mut b);
                copy = Some(Foot::load(&mut blob::Scan::new(&b.v)).unwrap());
            }
        }
        assert!(foot.planted(), "reachable ground was never recovered");
        assert!(foot.contact.unwrap().pos.y.sub(fx(-0.6)).abs() <= MILLI);
    }

    #[test]
    fn a_swinging_foot_clears_the_ground_and_replants_without_a_jump() {
        let r = rig();
        let g = gait();
        let mut foot = Foot::default();
        let mut last: Option<V3> = None;
        let mut highest = ZERO;
        let step = fx(0.002);
        for k in 0..110 {
            let root = hull(V3::new(step.mul(Fx::from_int(k)), ZERO, ZERO), 0);
            let s = Stance {
                grounded: true,
                root: &root,
                head: 0,
                phase: phase(f64::from(k) * 0.01),
                lift_scale: ONE,
                linear: V3::new(step, ZERO, ZERO),
                angular: 0,
                cycles: fx(0.01),
                gait: &g,
                floor: &flat,
            };
            let c = foot.step(&r, &s);
            highest = highest.max(c.pos.y.sub(r.ankle.y));
            if let Some(old) = last {
                assert!(near(old, c.pos, 0.035), "foot teleported at tick {k}");
            }
            last = Some(c.pos);
        }
        assert!(highest > g.lift.mul(fx(0.95)), "lift {highest:?}");
        assert!((last.unwrap().y.sub(r.ankle.y)).abs() < MILLI);
        assert!(foot.planted());
    }

    /// **A shoved walker aims its foot within reach** (ADR-451). The lead
    /// to touchdown is the drive's; the motion it scaled included the
    /// shove, and a walker barely stepping while pushed aimed its foot
    /// eighty thousand units away — where the square of the distance
    /// wraps and the root of it fails.
    #[test]
    fn a_shoved_walker_aims_its_foot_within_reach() {
        let r = rig();
        let g = gait();
        let mut foot = Foot::default();
        let root = hull(V3::ZERO, 0);
        let s = Stance {
            grounded: true,
            root: &root,
            head: 0,
            phase: phase(0.95),
            lift_scale: ONE,
            linear: V3::new(fx(2.0), ZERO, ZERO),
            angular: 900,
            cycles: Fx::from_ratio(1, 100_000),
            gait: &g,
            floor: &flat,
        };
        let c = foot.step(&r, &s);
        let reach = r.l1.add(r.l2).add(g.stride.mul(Fx::from_int(4)));
        let d = foot.goal.pos.sub(root.base);
        assert!(
            d.x.abs() <= reach && d.z.abs() <= reach,
            "the foot is aimed {:?} away",
            (d.x, d.z)
        );
        assert!(c.pos.y.abs() < Fx::from_int(1000));
    }

    #[test]
    fn a_stopped_hull_finishes_a_lifted_step_at_the_lift_off_pace() {
        let r = rig();
        let g = gait();
        let mut foot = Foot::default();
        let step = fx(0.004);
        let mut k = 0;
        let mut lifted = None;
        // Walk into the swing.
        while lifted.is_none() {
            let root = hull(V3::new(step.mul(Fx::from_int(k)), ZERO, ZERO), 0);
            let s = Stance {
                grounded: true,
                root: &root,
                head: 0,
                phase: phase(f64::from(k) * 0.02),
                lift_scale: ONE,
                linear: V3::new(step, ZERO, ZERO),
                angular: 0,
                cycles: fx(0.02),
                gait: &g,
                floor: &flat,
            };
            let c = foot.step(&r, &s);
            if c.pos.y.sub(r.ankle.y) > fx(0.02) {
                lifted = Some((k, root));
            }
            k += 1;
            assert!(k < 200, "never lifted");
        }
        let (at, root) = lifted.unwrap();
        // Stop: the phase and the hull stand still, the foot still lands.
        let mut landed = None;
        for n in 0..60 {
            let s = Stance {
                grounded: true,
                root: &root,
                head: 0,
                phase: phase(f64::from(at) * 0.02),
                lift_scale: ONE,
                linear: V3::ZERO,
                angular: 0,
                cycles: ZERO,
                gait: &g,
                floor: &flat,
            };
            let c = foot.step(&r, &s);
            if (c.pos.y.sub(r.ankle.y)).abs() < MILLI && landed.is_none() {
                landed = Some((n, c));
            }
        }
        let (n, c) = landed.expect("the lifted foot never landed");
        assert!(n > 0 && n < 40, "landed after {n} ticks");
        // The landing point is the goal frozen at lift-off, ahead of the hull.
        assert!(
            near(c.pos, foot.goal.pos, 1e-4) && c.head == foot.goal.head,
            "{c:?}"
        );
        assert!(c.pos.x > root.base.x.add(r.ankle.x), "{c:?}");
        assert!(foot.planted(), "the landed foot remained marked airborne after stopping");
        let still = foot.contact.unwrap();
        for _ in 0..10 {
            let s = Stance { grounded: true, root: &root, head: 0,
                phase: phase(f64::from(at) * 0.02), lift_scale: ONE, linear: V3::ZERO,
                angular: 0, cycles: ZERO, gait: &g, floor: &flat };
            assert!(near(foot.step(&r, &s).pos, still.pos, 1e-4));
            assert!(foot.planted());
        }
        let mut bytes = blob::Blob::new();
        foot.save(&mut bytes);
        let mut restored = Foot::load(&mut blob::Scan::new(&bytes.v)).expect("stopped foot restores");
        let resumed_root = hull(root.base.add(V3::new(step, ZERO, ZERO)), 0);
        let resumed = Stance { grounded: true, root: &resumed_root, head: 0,
            phase: phase((f64::from(at) + 1.0) * 0.02), lift_scale: ONE,
            linear: V3::new(step, ZERO, ZERO), angular: 0, cycles: fx(0.02),
            gait: &g, floor: &flat };
        let next = foot.step(&r, &resumed);
        assert_eq!(next, restored.step(&r, &resumed));
        assert!(near(still.pos, next.pos, 0.035),
            "foot jumps when motion resumes: {:?} -> {:?}", still.pos, next.pos);
        let mut previous = next;
        let mut moved_again = false;
        for j in 2..80 {
            let next_root = hull(root.base.add(V3::new(step.mul(Fx::from_int(j)), ZERO, ZERO)), 0);
            let s = Stance { grounded: true, root: &next_root, head: 0,
                phase: phase((f64::from(at) + f64::from(j)) * 0.02), lift_scale: ONE,
                linear: V3::new(step, ZERO, ZERO), angular: 0, cycles: fx(0.02),
                gait: &g, floor: &flat };
            let contact = foot.step(&r, &s);
            assert_eq!(contact, restored.step(&r, &s), "restored foot diverged at {j}");
            assert!(near(previous.pos, contact.pos, 0.035), "foot jumped on resumed tick {j}");
            moved_again |= !foot.planted();
            previous = contact;
        }
        assert!(moved_again, "the foot did not resume its gait");
    }

    #[test]
    fn a_buckled_leg_folds_on_its_foot_with_the_knee_out() {
        let r = rig();
        let mut foot = Foot::default();
        let root = hull(v(3.0, 0.0, -2.0), radians(0.4));
        let c = foot.fold(&r, &root, radians(0.4), 350, &flat);
        // On the ground, where the rest foot would stand at this height.
        assert!((c.pos.y.sub(r.ankle.y)).abs() < MILLI, "{c:?}");
        // Drawn in under the hip to a third of the rest reach.
        let hip = to_world(&root, r.hip);
        let rest = to_world(&root, r.ankle);
        let reach = |p: V3| {
            V3::new(p.x.sub(hip.x), ZERO, p.z.sub(hip.z))
                .len_sq()
                .sqrt()
        };
        let want = reach(rest).mul(fx(0.35));
        assert!(
            (reach(c.pos).sub(want)).abs() < fx(0.01),
            "{:?} vs {want:?}",
            reach(c.pos)
        );
        assert!(foot.planted());
        // The leg still reaches its foot: the solve puts the ankle there.
        let pose = r.solve(to_local(&root, c.pos));
        assert!(near(to_world(&root, pose.ankle), c.pos, 1e-3), "{pose:?}");
    }

    #[test]
    fn flight_keeps_the_legs_with_the_hull_and_landing_replants() {
        let r = rig();
        let g = gait();
        let mut foot = Foot::default();
        let ground = hull(V3::ZERO, 0);
        let s = Stance {
            grounded: true,
            root: &ground,
            head: 0,
            phase: 0,
            lift_scale: ONE,
            linear: V3::ZERO,
            angular: 0,
            cycles: ZERO,
            gait: &g,
            floor: &flat,
        };
        let planted = foot.step(&r, &s);
        let air = hull(v(0.0, 3.0, 0.0), radians(0.5));
        let s = Stance {
            grounded: false,
            root: &air,
            ..s
        };
        let flying = foot.step(&r, &s);
        assert!(near(
            flying.pos,
            to_world(&air, to_local(&ground, planted.pos)),
            1e-3
        ));
        assert!(!foot.planted());
        let s = Stance {
            grounded: true,
            root: &ground,
            ..s
        };
        assert_eq!(foot.step(&r, &s), planted);
    }
    #[test]
    fn starting_inside_swing_does_not_pop_the_foot_into_the_air() {
        let r = rig(); let g = gait(); let root = hull(V3::ZERO, 0);
        let mut foot = Foot::default();
        let mut s = Stance { grounded: true, root: &root, head: 0, phase: 0,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: ZERO,
            gait: &g, floor: &flat };
        let planted = foot.step(&r, &s);
        s.phase = angle::FULL * 9 / 10;
        s.cycles = Fx::from_ratio(1, 100);
        s.linear = V3::new(Fx::from_ratio(1, 250), ZERO, ZERO);
        let first = foot.step(&r, &s);
        assert_eq!(first, planted, "a phase offset skipped the beginning of the foot's swing");
        assert!(!foot.planted(), "lift-off must release the stance before moving the foot");
        let next = foot.step(&r, &s);
        assert!(next.pos.y > first.pos.y, "the active foot never lifted");
    }

    #[test]
    fn a_phase_wrap_cannot_instantly_land_an_unfinished_foot() {
        let r = rig(); let g = gait(); let root = hull(V3::ZERO, 0);
        let mut foot = Foot::default();
        let mut s = Stance { grounded: true, root: &root, head: 0, phase: 0,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: ZERO,
            gait: &g, floor: &flat };
        foot.step(&r, &s);
        s.cycles = Fx::from_ratio(1, 100);
        s.linear = V3::new(Fx::from_ratio(1, 250), ZERO, ZERO);
        s.phase = angle::FULL * 3 / 4;
        foot.step(&r, &s);
        s.phase += angle::FULL / 100;
        let previous = foot.step(&r, &s);
        s.phase = angle::FULL / 20;
        let next = foot.step(&r, &s);
        assert!(!foot.planted(), "the global phase teleported a raised foot onto its goal");
        assert!(next.pos.sub(previous.pos).len_sq() < g.lift.mul(g.lift), "foot jumped at wrap");
        let mut bytes = blob::Blob::new(); foot.save(&mut bytes);
        let mut copy = Foot::load(&mut blob::Scan::new(&bytes.v)).unwrap();
        for _ in 0..50 { assert_eq!(foot.step(&r, &s), copy.step(&r, &s)); }
        assert!(foot.planted(), "the wrap stranded the foot in mid-air");
    }

    #[test]
    fn swing_lifts_and_lands_with_smaller_vertical_steps_at_the_endpoints() {
        let r = rig(); let g = gait(); let root = hull(V3::ZERO, 0);
        let mut foot = Foot::default();
        let mut s = Stance { grounded: true, root: &root, head: 0, phase: 0,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: ZERO,
            gait: &g, floor: &flat };
        let rest = foot.step(&r, &s);
        s.phase = angle::FULL * 3 / 4;
        s.cycles = Fx::from_ratio(1, 100);
        let mut heights = vec![rest.pos.y];
        foot.step(&r, &s);
        for _ in 0..40 {
            s.phase = angle::norm(s.phase + angle::FULL / 100);
            heights.push(foot.step(&r, &s).pos.y);
            if foot.planted() { break; }
        }
        let deltas: Vec<_> = heights.windows(2).map(|v| v[1].sub(v[0])).collect();
        assert!(deltas.len() > 4);
        assert!(deltas[0] > ZERO && deltas[0] < deltas[1], "abrupt lift: {deltas:?}");
        let n = deltas.len();
        assert!(deltas[n-1].abs() < deltas[n-2].abs(), "abrupt landing: {deltas:?}");
        assert!(foot.planted());
    }

    #[test]
    fn a_raised_foot_clears_terrain_that_changes_during_its_swing() {
        let r = rig(); let g = gait(); let root = hull(V3::ZERO, 0);
        let height = std::cell::Cell::new(ZERO);
        let floor = |_: Fx, _: Fx| height.get();
        let mut foot = Foot::default();
        let mut s = Stance { grounded: true, root: &root, head: 0, phase: 0,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: ZERO,
            gait: &g, floor: &floor };
        foot.step(&r, &s);
        s.phase = angle::FULL * 3 / 4; s.cycles = Fx::from_ratio(1, 100);
        foot.step(&r, &s);
        height.set(g.lift.mul(Fx::from_int(3)));
        let c = foot.step(&r, &s);
        for p in r.sole.probes() {
            let [x, y, z] = c.axes();
            let world = c.pos.add(x.scale(p.x)).add(y.scale(p.y)).add(z.scale(p.z));
            assert!(world.y >= height.get(), "the swinging sole entered newly raised ground");
        }
    }

    #[test]
    fn a_crawl_started_swing_honours_its_forecast_landing_bound() {
        let r=rig();let g=gait();let root=hull(V3::ZERO,0);
        let mut foot=Foot::default();
        let mut s=Stance {grounded:true,root:&root,head:0,phase:0,lift_scale:ONE,
            linear:V3::ZERO,angular:0,cycles:ZERO,gait:&g,floor:&flat};
        foot.step(&r,&s);
        s.phase=angle::FULL*3/4;s.cycles=Fx::from_ratio(1,100_000);
        foot.step(&r,&s);assert!(!foot.planted());
        let time=foot.next_landing(&g,s.cycles).unwrap();
        let ticks=time.to_int_trunc()+i32::from(time.raw()&0xffff_ffff!=0);
        assert!(ticks<=40,"unbounded crawl landing forecast");
        for _ in 0..=ticks {foot.step(&r,&s);}
        assert!(foot.planted(),"positive crawl input left the foot hovering beyond its promised landing");
    }

    #[test]
    fn braking_to_a_crawl_finishes_the_raised_foot_at_its_committed_pace() {
        let r = rig(); let g = gait(); let root = hull(V3::ZERO, 0);
        let mut foot = Foot::default();
        let mut s = Stance { grounded: true, root: &root, head: 0, phase: 0,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: ZERO,
            gait: &g, floor: &flat };
        foot.step(&r, &s);
        s.phase = angle::FULL * 3 / 4; s.cycles = Fx::from_ratio(1, 100);
        foot.step(&r, &s);
        for _ in 0..8 { foot.step(&r, &s); }
        assert!(!foot.planted());
        s.cycles = Fx::from_ratio(1, 100_000);
        for _ in 0..30 { foot.step(&r, &s); }
        assert!(foot.planted(), "braking left a raised foot hovering at crawl cadence");
        let landed = foot.contact.unwrap();
        for _ in 0..10 { assert_eq!(foot.step(&r, &s), landed); }
    }

    #[test]
    fn raised_ground_at_liftoff_cannot_launch_the_foot_beyond_its_links_or_break_a_save() {
        let r = rig(); let g = gait(); let root = hull(V3::ZERO, 0);
        let height = std::cell::Cell::new(ZERO);
        let floor = |_: Fx, _: Fx| height.get();
        let mut foot = Foot::default();
        let mut s = Stance { grounded: true, root: &root, head: 0, phase: 0,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: ZERO,
            gait: &g, floor: &floor };
        foot.step(&r, &s);
        height.set(Fx::from_int(200));
        s.phase = angle::FULL * 3 / 4; s.cycles = Fx::from_ratio(1, 100);
        foot.step(&r, &s);
        assert!(foot.clearance <= r.l1.add(r.l2), "unreachable lift {:?}", foot.clearance);
        let mut bytes = blob::Blob::new(); foot.save(&mut bytes);
        assert!(Foot::load(&mut blob::Scan::new(&bytes.v)).is_some(), "an ordinary step made an unloadable snapshot");
    }

    #[test]
    fn a_small_ledge_does_not_amplify_the_entire_swing_to_leg_height() {
        let sole = Sole::point(ZERO);
        let floor = |x: Fx, _: Fx| if x > Fx::from_ratio(1, 100) {Fx::from_int(3)} else {ZERO};
        let from = sole.contact(V3::ZERO, 0, &floor);
        let to = sole.contact(V3::new(Fx::from_int(20), ZERO, ZERO), 0, &floor);
        let lift = Fx::from_int(4);
        let clearance = sole.clearance(from, to, lift, &floor);
        assert!(clearance <= Fx::from_int(7), "30 cm ledge amplified swing: {clearance:?}");
    }

    #[test]
    fn a_level_sole_reuses_samples_without_losing_a_high_centre_probe() {
        let sole = Sole { probes: (-1..=1).flat_map(|x| (-1..=1).map(move |z|
            V3::new(Fx::from_int(x), Fx::from_ratio(-1, 4), Fx::from_int(z)))).collect() };
        let calls = std::cell::Cell::new(0usize);
        let floor = |x: Fx, z: Fx| {
            calls.set(calls.get() + 1);
            if x == ZERO && z == ZERO { Fx::from_int(2) } else { ZERO }
        };
        let c = sole.contact(V3::ZERO, 0, &floor);
        assert_eq!(c.normal, V3::UP);
        assert_eq!(c.pos.y, Fx::from_ratio(9, 4));
        assert_eq!(calls.get(), 9, "the unchanged footprint was queried twice");
        let slope = |x: Fx, _: Fx| x.mul(Fx::from_ratio(1, 5));
        let c = sole.contact(V3::ZERO, 0, &slope);
        assert_ne!(c.normal, V3::UP);
        let [x, y, z] = c.axes();
        for p in sole.probes() {
            let world = c.pos.add(x.scale(p.x)).add(y.scale(p.y)).add(z.scale(p.z));
            assert!(world.y.add(Fx::from_ratio(1, 1000)) >= slope(world.x, world.z));
        }
    }

    #[test]
    fn climbing_onto_a_high_riser_does_not_double_count_step_rise_above_the_knee() {
        let body = voxel::unit::spider();
        let rig = rigs_of(&body).remove(1);
        let root = hull(V3::new(ZERO, Fx::from_ratio(1309, 100), ZERO), 0);
        let width = rig.sole.width();
        let edge = rig.ankle.x.add(width).add(ONE);
        let floor = |x: Fx, _: Fx| if x > edge { Fx::from_int(60) } else { Fx::from_int(16) };
        let from = rig.sole.contact(to_world(&root, V3::new(rig.ankle.x, rig.ankle.y, rig.ankle.z)), 0, &floor);
        let to = rig.sole.contact(to_world(&root, V3::new(rig.ankle.x.add(width.mul(Fx::from_int(4))), Fx::from_int(47), rig.ankle.z)), 0, &floor);
        assert_eq!(to.pos.y.sub(from.pos.y), Fx::from_int(44),
            "the full sole must start below and land above the ledge");
        let lift = Fx::from_int(10);
        let clearance = rig.sole.clearance(from, to, lift, &floor);
        let mut foot = Foot {
            contact: Some(from),
            anchor: None,
            from,
            goal: to,
            swing: true,
            clearance,
            progress: Fx::from_ratio(1, 2),
            cadence: Fx::from_ratio(1, 8),
            ..Foot::default()
        };
        let (mid, _) = foot.swing_contact(&rig, Fx::from_ratio(1, 2), &floor);
        assert!(mid.pos.y <= to.pos.y.add(lift).add(MILLI),
            "step-up apex {:?} overshot plateau+lift {:?}", mid.pos.y, to.pos.y.add(lift));
        let pose = rig.solve_contact(to_local(&root, mid.pos));
        assert!(pose.ankle.y < pose.knee.y,
            "mid-swing ankle {:?} rose above knee {:?}", pose.ankle.y, pose.knee.y);
        // Arriving half keeps an overstretched airborne contact inside physical link reach.
        foot.from = Contact { pos: to_world(&root, V3::new(rig.hip.x, Fx::from_int(-120), rig.hip.z)), head: 0, normal: V3::UP };
        foot.goal = to;
        foot.progress = Fx::from_ratio(3, 5);
        let g = Gait { stride: Fx::from_int(44), duty: Fx::from_ratio(3, 4), lift, offset: [0; 4] };
        let stance = Stance { grounded: true, root: &root, head: 0, phase: angle::FULL * 4 / 5,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: Fx::from_ratio(1, 20), gait: &g, floor: &floor };
        let stepped = foot.step(&rig, &stance);
        assert!(rig.contact_reachable(to_local(&root, stepped.pos)),
            "arriving airborne contact {:?} left physical reach", stepped.pos);
    }

    #[test]
    fn arriving_free_swing_matches_physical_reach_without_admitting_support() {
        let body = voxel::unit::spider();
        let rig = rigs_of(&body).remove(1);
        let root = hull(V3::new(ZERO, Fx::from_ratio(1309, 100), ZERO), 0);
        let floor = |_: Fx, _: Fx| Fx::from_int(16);
        let goal = rig.sole.contact(to_world(&root, rig.ankle), 0, &floor);
        let from = Contact { pos: goal.pos.add(V3::new(Fx::from_int(500), ZERO, ZERO)), ..goal };
        let gait = Gait { stride: Fx::from_int(44), duty: Fx::from_ratio(3, 4), lift: Fx::from_int(10), offset: [0; 4] };
        let stance = Stance { grounded: true, root: &root, head: 0, phase: 0,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: ZERO, gait: &gait, floor: &floor };
        for (progress, arriving) in [(Fx::from_ratio(3, 5), true), (Fx::from_ratio(1, 4), false)] {
            let mut foot = Foot { contact: Some(from), anchor: Some(from), base: root.base, from, goal, swing: true,
                progress, clearance: gait.lift, cadence: Fx::from_ratio(1, 80), ..Foot::default() };
            let next = progress.add(Fx::from_ratio(1, 80).div(ONE.sub(gait.duty)));
            let (raw, _) = foot.swing_contact(&rig, next, &floor);
            assert!(!rig.contact_reachable(to_local(&root, raw.pos)), "fixture must exceed reach");
            let contact = foot.step(&rig, &stance);
            if arriving {
                assert!(rig.contact_reachable(to_local(&root, contact.pos)), "executed arriving contact stayed beyond the links");
            } else {
                assert_eq!(contact, raw, "departing contact must remain visible to TransitionReason::Reach");
            }
            assert!(foot.swing && !foot.planted() && !foot.ready(),
                "an airborne reach correction cannot admit support");
            assert_eq!(foot.anchor, Some(from), "reach fitting cannot move the takeoff anchor");
        }
    }

    #[test]
    fn final_frame_projection_preserves_planted_and_departing_and_traces_only_free_contacts() {
        let (rig,root,original,_,_)=live_fixture();
        let far=rig.hip.add(V3::new(rig.l1.add(rig.l2).mul(Fx::from_int(2)),ZERO,ZERO));
        for (swing,progress) in [(false,ZERO),(true,Fx::from_ratio(1,4)),(true,Fx::from_ratio(3,4))] {
            let mut foot=original.clone();foot.swing=swing;foot.progress=progress;
            foot.contact=Some(Contact{pos:to_world(&root,far),..original.contact.unwrap()});
            let before=foot.clone();let mut trace=AnchorTrace::default();
            foot.clamp_arriving_reach(&rig,&root,0,Some(&mut trace));
            if swing && progress>Fx::from_ratio(1,2) {
                assert!(rig.contact_reachable(to_local(&root,foot.contact.unwrap().pos)));
                assert_eq!((foot.anchor,foot.from,foot.goal,foot.progress,foot.support),(before.anchor,before.from,before.goal,before.progress,before.support));
                assert!(!foot.ready());assert_eq!(trace.events().len(),1);
                let e=trace.events()[0];assert_eq!(e.reason,AnchorReason::Reproject);
                assert!(!e.planted_before && !e.planted_after);assert_eq!(e.before,before.contact);assert_eq!(e.after,foot.contact);
            } else {assert_eq!(foot,before);assert!(trace.events().is_empty());}
            proof_roundtrip(&foot);
        }
    }

    #[test]
    fn failed_touchdown_retry_only_bounds_unreachable_lower_targets_without_granting_ready() {
        let (rig,root,original,_,_)=live_fixture();
        let gait=Gait{stride:Fx::from_int(8),duty:Fx::from_ratio(3,4),lift:Fx::ONE,offset:[0;4]};
        for depth in [-100,100] {
            let floor=|_:Fx,_:Fx|Fx::from_int(depth);
            let from=original.contact.unwrap();let goal=Contact{pos:V3::new(from.pos.x,Fx::from_int(-100),from.pos.z),..from};
            let mut foot=Foot{contact:Some(from),anchor:Some(from),base:root.base,from,goal,swing:true,progress:Fx::from_ratio(99,100),cadence:Fx::from_ratio(1,10),..Foot::default()};
            let stance=Stance{grounded:true,root:&root,head:0,phase:0,lift_scale:ONE,linear:V3::ZERO,angular:0,cycles:ONE,gait:&gait,floor:&floor};
            let mut trace=AnchorTrace::default();foot.step_supported_traced(&rig,&stance,false,false,true,Some(&mut trace));
            assert!(trace.events().iter().any(|e|e.reason==AnchorReason::LandingRejected));
            assert!(foot.swing && foot.departing() && !foot.planted() && !foot.ready());assert_eq!(foot.anchor,Some(from));
            assert!(rig.contact_reachable(to_local(&root,foot.contact.unwrap().pos)));
            if depth<0 {assert!(rig.contact_reachable(to_local(&root,foot.goal.pos)),"retry over a pit must stay in physical reach");}
            else {assert!(!rig.contact_reachable(to_local(&root,foot.goal.pos)),"an upper obstruction cannot be hidden as a lower retry");}
            proof_roundtrip(&foot);
        }
    }

    #[test]
    fn locked_swing_over_deep_cliff_bounds_abyss_goal_without_moving_anchor_or_admitting_support() {
        let body = voxel::unit::spider();
        let rig = rigs_of(&body).remove(6);
        let root = hull(V3::new(ZERO, Fx::from_ratio(6609, 100), ZERO), 0);
        let ramp_floor = |_: Fx, _: Fx| Fx::from_int(66);
        let abyss_floor = |_: Fx, _: Fx| Fx::from_int(16);
        let from = rig.sole.contact(to_world(&root, rig.ankle), 0, &ramp_floor);
        let abyss_goal = rig.sole.contact(to_world(&root, rig.ankle), 0, &abyss_floor);
        assert!(!rig.contact_reachable(to_local(&root, abyss_goal.pos)));
        let gait = Gait { stride: Fx::from_int(44), duty: Fx::from_ratio(3, 4), lift: Fx::from_int(10), offset: [0; 4] };
        let stance = Stance { grounded: true, root: &root, head: 0, phase: 0,
            lift_scale: ONE, linear: V3::ZERO, angular: 0, cycles: Fx::from_ratio(1, 40), gait: &gait, floor: &abyss_floor };
        let mut foot = Foot { contact: Some(from), anchor: Some(from), base: root.base, from, goal: abyss_goal,
            swing: true, progress: Fx::from_ratio(38, 100), clearance: ZERO, cadence: Fx::from_ratio(1, 40), ..Foot::default() };
        let contact = foot.step_supported(&rig, &stance, true, false, true);
        assert!(foot.departing(), "fixture must stay in the departing half");
        assert!(rig.contact_reachable(to_local(&root, contact.pos)),
            "abyss goal pulled departing foot outside physical reach: {:?}", contact.pos);
        assert!(foot.swing && !foot.planted() && !foot.ready(),
            "cliff swing must not admit support");
        assert_eq!(foot.anchor, Some(from), "abyss goal bound must not alter takeoff anchor");
        assert_eq!(foot.from,from,"abyss goal bound must not replace the departing trajectory origin");
        proof_roundtrip(&foot);
    }



    #[test]
    fn abyss_goal_envelope_respects_lock_lift_direction_and_keeps_takeoff_evidence() {
        let body=voxel::unit::spider();let rig=rigs_of(&body).remove(6);
        let root=hull(V3::new(ZERO,Fx::from_ratio(6609,100),ZERO),0);
        let start_floor=|_:Fx,_:Fx|Fx::from_int(66);
        let from=rig.sole.contact(to_world(&root,rig.ankle),0,&start_floor);
        for (lock,y,lift,bounded) in [(true,16,10,true),(false,16,10,false),(true,16,1000,false),(true,116,10,false)] {
            let floor=|_:Fx,_:Fx|Fx::from_int(y);
            let goal=rig.sole.contact(to_world(&root,rig.ankle),0,&floor);
            let gait=Gait{stride:Fx::from_int(44),duty:Fx::from_ratio(3,4),lift:Fx::from_int(lift),offset:[0;4]};
            let stance=Stance{grounded:true,root:&root,head:0,phase:0,lift_scale:ONE,linear:V3::ZERO,angular:0,cycles:Fx::from_ratio(1,40),gait:&gait,floor:&floor};
            let mut foot=Foot{contact:Some(from),anchor:Some(from),base:root.base,from,goal,swing:true,progress:Fx::from_ratio(38,100),clearance:ZERO,cadence:Fx::from_ratio(1,40),..Foot::default()};
            let mut trace=AnchorTrace::default();let contact=foot.step_supported_traced(&rig,&stance,true,false,lock,Some(&mut trace));
            let events:Vec<_>=trace.events().iter().filter(|e|e.reason==AnchorReason::GoalEnvelope).collect();
            assert_eq!(!events.is_empty(),bounded,"lock={lock} y={y} lift={lift}");
            assert_eq!(foot.anchor,Some(from));assert_eq!(foot.from,from);
            assert!(foot.swing && foot.departing() && !foot.ready() && !foot.planted());
            if bounded {
                assert!(rig.contact_reachable(to_local(&root,contact.pos)));
                assert_eq!(events.len(),1);let e=events[0];assert_eq!(e.before,Some(goal));assert_eq!(e.after,Some(foot.goal));assert!(!e.planted_before && !e.planted_after);
            }
            proof_roundtrip(&foot);
        }
    }

    #[test]
    fn abyss_goal_envelope_does_not_hide_an_unreachable_actual_departing_origin() {
        let body=voxel::unit::spider();let rig=rigs_of(&body).remove(6);
        let root=hull(V3::new(ZERO,Fx::from_ratio(6609,100),ZERO),0);
        let start_floor=|_:Fx,_:Fx|Fx::from_int(66);let floor=|_:Fx,_:Fx|Fx::from_int(16);
        let mut from=rig.sole.contact(to_world(&root,rig.ankle),0,&start_floor);from.pos.x=from.pos.x.add(Fx::from_int(500));
        let goal=rig.sole.contact(to_world(&root,rig.ankle),0,&floor);
        let gait=Gait{stride:Fx::from_int(44),duty:Fx::from_ratio(3,4),lift:Fx::from_int(10),offset:[0;4]};
        let stance=Stance{grounded:true,root:&root,head:0,phase:0,lift_scale:ONE,linear:V3::ZERO,angular:0,cycles:Fx::from_ratio(1,40),gait:&gait,floor:&floor};
        let mut foot=Foot{contact:Some(from),anchor:Some(from),base:root.base,from,goal,swing:true,progress:Fx::from_ratio(38,100),clearance:ZERO,cadence:Fx::from_ratio(1,40),..Foot::default()};
        let contact=foot.step_supported(&rig,&stance,true,false,true);
        assert!(foot.departing());assert_eq!(foot.from,from);assert_eq!(foot.anchor,Some(from));
        assert!(!rig.contact_reachable(to_local(&root,from.pos)));
        assert!(!rig.contact_reachable(to_local(&root,contact.pos)),"actual departing error must remain visible to the motor's Reach check");
        assert!(!foot.ready() && !foot.planted());proof_roundtrip(&foot);
    }

    #[test]
    fn ready_alternative_memo_indexed_table_and_lazy_planes_match_unmemoized_with_zero_repeat_reads() {
        use nav::vehicle::support::{Sample, SupportRef, Surface, Validity};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        struct StoneSurface<'a> {
            samples: &'a AtomicUsize,
            armed: &'a AtomicBool,
            reject_all: bool,
        }
        impl StoneSurface<'_> {
            fn terrain_y(x: Fx, z: Fx) -> Fx {
                let ix = (x.raw() >> 32) as i32;
                let iz = (z.raw() >> 32) as i32;
                let bump = ((ix.wrapping_mul(17) ^ iz.wrapping_mul(31)).rem_euclid(5)) - 2;
                Fx::from_int(32 + bump)
            }
        }
        impl Surface for StoneSurface<'_> {
            fn sample(&self, x: Fx, _y: Fx, z: Fx) -> Sample {
                if self.armed.load(Ordering::Relaxed) {
                    self.samples.fetch_add(1, Ordering::Relaxed);
                }
                let h = Self::terrain_y(x, z);
                let ix = (x.raw() >> 32) as i32;
                let iz = (z.raw() >> 32) as i32;
                let valid = if self.reject_all || (ix + iz).rem_euclid(3) == 0 {
                    Validity::Unsupported
                } else {
                    Validity::Supported
                };
                Sample {
                    height: h,
                    source: SupportRef { kind: 0, id: 0, content: h.raw() as u64, pose: 0 },
                    validity: valid,
                }
            }
            fn validity(&self, _source: SupportRef) -> Validity {
                Validity::Supported
            }
        }

        let body = voxel::unit::spider();
        let rigs = rigs_of(&body);
        let profile = modding::walking_terrain::Capability {
            min_support_pct: 50,
            sole_ready_pct: 60,
            sole_gap_mm: 120,
            min_up_pct: 70,
            stance_reach_pct: 98,
            stance_drop_pct: 100,
            ..live_fixture().4
        };
        let floor_reads = AtomicUsize::new(0);
        let sample_reads = AtomicUsize::new(0);
        let armed = AtomicBool::new(false);
        let floor = |x: Fx, z: Fx| {
            if armed.load(Ordering::Relaxed) {
                floor_reads.fetch_add(1, Ordering::Relaxed);
            }
            StoneSurface::terrain_y(x, z)
        };

        for reject_all in [false, true] {
            let surface = StoneSurface { samples: &sample_reads, armed: &armed, reject_all };
            let memo = AlternativeMemo::default();
            for up in [
                V3::UP,
                unit_or(V3::new(Fx::from_ratio(18, 100), ONE, Fx::from_ratio(-14, 100)), V3::UP),
            ] {
                for head in [-511, 0, 733, angle::FULL / 6] {
                    let base = V3::new(Fx::from_int(128), Fx::from_int(32), Fx::from_int(128));
                    let root = Frame::new(&body, base, head, up);
                    for rig in &rigs {
                        let world_ankle = to_world(&root, rig.lane_target(rig.ankle));
                        let contact = rig.sole.contact(world_ankle, head.wrapping_add(rig.lane.head), &floor);
                        let foot = Foot {
                            from: Contact { pos: contact.pos.sub(V3::new(ONE, ZERO, ZERO)), ..contact },
                            goal: contact,
                            ..Foot::default()
                        };
                        let fast = foot.ready_alternative_memo(rig, &root, head, contact, &surface, &floor, profile, Some(&memo));
                        let exact = foot.ready_alternative_memo(rig, &root, head, contact, &surface, &floor, profile, None);
                        assert_eq!(fast, exact, "reject_all={reject_all} head={head} up={up:?}");
                        let mut legacy=Legacy20Entry{offsets:std::array::from_fn(|_|Legacy20Offset::default())};
                        assert_eq!(fast,Foot::legacy20_ready_alternative(&mut legacy,rig,&root,head,contact,false,&surface,&floor,profile),
                            "pre20 scalar: reject_all={reject_all} head={head} up={up:?}");
                    }
                }
            }
            if reject_all {
                let base2 = V3::new(Fx::from_ratio(12805, 100), Fx::from_int(32), Fx::from_ratio(12803, 100));
                let root2 = Frame::new(&body, base2, 0, V3::UP);
                let rig = &rigs[0];
                let base0 = V3::new(Fx::from_int(128), Fx::from_int(32), Fx::from_int(128));
                let root0 = Frame::new(&body, base0, 0, V3::UP);
                let contact = rig.sole.contact(to_world(&root0, rig.lane_target(rig.ankle)), rig.lane.head, &floor);
                let foot = Foot {
                    from: Contact { pos: contact.pos.sub(V3::new(ONE, ZERO, ZERO)), ..contact },
                    goal: contact,
                    ..Foot::default()
                };
                let _ = foot.ready_alternative_memo(rig, &root0, 0, contact, &surface, &floor, profile, Some(&memo));
                floor_reads.store(0, Ordering::Relaxed);
                sample_reads.store(0, Ordering::Relaxed);
                armed.store(true, Ordering::Relaxed);
                let again = foot.ready_alternative_memo(rig, &root2, 0, contact, &surface, &floor, profile, Some(&memo));
                armed.store(false, Ordering::Relaxed);
                assert_eq!(again, None);
                assert_eq!(floor_reads.load(Ordering::Relaxed), 0);
                assert_eq!(sample_reads.load(Ordering::Relaxed), 0);
            }
        }
    }
    #[test]
    fn indexed_alt_memo_matches_pre20_geometry_after_eviction_and_rig_changes() {
        let (base_rig,base_root,foot,surface,profile)=live_fixture();
        let memo=AlternativeMemo::default(); let mut accepted=0;
        // The same terrain key is reused after changing root, hip, ankle, sole and lane.
        for pass in 0..3 {
            for change in 0..9 {
                let mut rig=base_rig.clone();let mut root=base_root;
                match change {
                    1=>root.base.x=Fx::from_ratio(1,5),
                    2=>root.base.y=Fx::from_ratio(1,2),
                    3=>rig.hip.x=rig.hip.x.add(Fx::from_ratio(1,3)),
                    4=>rig.ankle.z=rig.ankle.z.sub(Fx::from_ratio(1,3)),
                    5=>rig.sole.probes[0].y=rig.sole.probes[0].y.sub(Fx::from_ratio(1,4)),
                    6=>rig.lane.head=733,
                    7=>{root.y=unit_or(v(0.18,1.0,-0.14),V3::UP);root.x=unit_or(X.sub(root.y.scale(X.dot(root.y))),X);root.z=root.x.cross(root.y);},
                    8=>root.base.x=Fx::from_int(20),_=>{}
                }
                let contact=foot.goal;
                let mut entry=Legacy20Entry{offsets:std::array::from_fn(|_|Legacy20Offset::default())};
                let expected=Foot::legacy20_ready_alternative(&mut entry,&rig,&root,0,contact,false,&surface,&flat,profile);
                let actual=foot.ready_alternative_memo(&rig,&root,0,contact,&surface,&flat,profile,Some(&memo));
                assert_eq!(actual,expected,"pass={pass} change={change}");accepted+=usize::from(actual.is_some());
            }
        }
        assert!(accepted>0,"oracle must exercise accepted placements");
        // Force round-robin replacement at the last bit of the last way word.
        let mut colliding=Vec::new();
        for i in 0..100_000 {
            let pos=foot.goal.pos.add(V3::new(Fx::from_ratio(i,10000),ZERO,ZERO));
            let key=AltKey{lane_head:base_rig.lane.head,scale:base_root.scale,head:0,pos,sign_neg:false};
            if AlternativeMemo::hash_key(&key)==ALT_BUCKETS-1 {colliding.push(pos);if colliding.len()==7 {break;}}
        }
        assert_eq!(colliding.len(),7);
        for &pos in colliding.iter().chain(colliding.iter().rev()) {
            let contact=Contact{pos,..foot.goal};
            let mut entry=Legacy20Entry{offsets:std::array::from_fn(|_|Legacy20Offset::default())};
            let expected=Foot::legacy20_ready_alternative(&mut entry,&base_rig,&base_root,0,contact,false,&surface,&flat,profile);
            assert_eq!(foot.ready_alternative_memo(&base_rig,&base_root,0,contact,&surface,&flat,profile,Some(&memo)),expected);
        }
        // Grow beyond the initial 16 entries, fill/evict throughout the directory,
        // then revisit the colliding keys after Vec relocation.
        for i in 0..1024 {
            let contact=Contact{pos:foot.goal.pos.add(V3::new(Fx::from_ratio(i,100),ZERO,ZERO)),..foot.goal};
            let mut entry=Legacy20Entry{offsets:std::array::from_fn(|_|Legacy20Offset::default())};
            let expected=Foot::legacy20_ready_alternative(&mut entry,&base_rig,&base_root,0,contact,false,&surface,&flat,profile);
            assert_eq!(foot.ready_alternative_memo(&base_rig,&base_root,0,contact,&surface,&flat,profile,Some(&memo)),expected);
        }
        let table=memo.table.get().unwrap().borrow();
        assert!(table.entries.len()>16 && table.entries.len()<=2*ALT_BUCKETS);
        for slots in &table.slots {for &idx in slots {assert!(idx==u16::MAX || usize::from(idx)<table.entries.len());}}
    }

    #[test]
    fn sole_fast_paths_match_pre20_fixed_point_contacts_and_axes() {
        let body=voxel::unit::spider();let rigs=rigs_of(&body);
        for head in [0,-733,511,angle::FULL,angle::FULL/3] {
            for normal in [V3::UP,unit_or(v(0.18,1.0,-0.14),V3::UP),unit_or(v(-0.7,1.0,0.3),V3::UP)] {
                let contact=Contact{pos:V3::ZERO,head,normal};assert_eq!(contact.axes(),contact.legacy20_axes());
            }
            for rig in &rigs {for mode in 0..3 {
                let floor=|x:Fx,z:Fx|match mode {0=>Fx::from_int(24),1=>x.mul(Fx::from_ratio(1,3)).add(z.mul(Fx::from_ratio(1,7))),_=>Fx::from_int(((x.raw()>>32) as i32 ^ (z.raw()>>32) as i32).rem_euclid(4))};
                let at=rig.ankle;
                assert_eq!(rig.sole.contact(at,head,&floor),rig.sole.legacy20_contact(at,head,&floor));
                for n in [V3::UP,unit_or(v(0.3,1.0,-0.2),V3::UP)] {assert_eq!(rig.sole.height(at,head,n,&floor),rig.sole.legacy20_height(at,head,n,&floor));}
            }}
        }
    }

    #[test]
    fn unclimbable_sheer_wall_refuses_perch_and_holds_swing_while_climbable_step_and_ramp_pass() {
        let body = voxel::unit::spider();
        let rigs = rigs_of(&body);
        let profile = modding::walking_terrain::Tuning::from_ron(include_str!("../../../data/walking-terrain.ron")).unwrap().spider;
        let root_level = Frame::new(
            &body,
            V3::new(Fx::from_int(1150), Fx::from_int(16), Fx::from_int(1024)),
            0,
            V3::UP,
        );
        let root_pitched = Frame::new(
            &body,
            V3::new(Fx::from_int(1150), Fx::from_int(16), Fx::from_int(1024)),
            0,
            unit_or(V3::new(Fx::from_ratio(-25, 100), ONE, ZERO), V3::UP),
        );
        let rig = rigs.iter().max_by_key(|r| r.ankle.x.raw()).unwrap();
        let foot_head = rig.lane.head;
        let at_ahead = to_world(&root_level, rig.lane_target(rig.ankle));
        let (fc, fs) = (angle::cos(foot_head), angle::sin(foot_head));
        let tip_dx = rig.sole.probes.iter().map(|p| turned_cs(*p, foot_head, fc, fs).x).max().unwrap_or(ZERO);
        let wall_x = at_ahead.x.add(tip_dx).sub(Fx::from_int(2));

        // 1. Climbable step (h = 28 == max_step) and gradual ramp (h = 50) both pass.
        let step28 = |x: Fx, _z: Fx| {
            if x >= wall_x { Fx::from_int(44) } else { Fx::from_int(16) }
        };
        let ramp50 = |x: Fx, _z: Fx| {
            if x <= Fx::from_int(1150) {
                Fx::from_int(16)
            } else if x >= Fx::from_int(1270) {
                Fx::from_int(66)
            } else {
                Fx::from_int(16).add(x.sub(Fx::from_int(1150)).mul(Fx::from_ratio(50, 120)))
            }
        };
        assert!(rig.step_climbable(&root_level, rig.sole.contact(at_ahead, foot_head, &step28), &step28, profile));
        assert!(rig.step_climbable(&root_level, rig.sole.contact(at_ahead, foot_head, &ramp50), &ramp50, profile));

        // 2. Unclimbable sheer wall (h = 44 > max_step) is refused even under nose-up pitch.
        let wall44 = |x: Fx, _z: Fx| {
            if x >= wall_x { Fx::from_int(60) } else { Fx::from_int(16) }
        };
        let perched = rig.sole.contact(at_ahead, foot_head, &wall44);
        assert!(perched.pos.y >= Fx::from_int(60));
        assert!(!rig.step_climbable(&root_level, perched, &wall44, profile));
        assert!(!rig.step_climbable(&root_pitched, perched, &wall44, profile));

        // 3. Retracted neutral lands cleanly on the flat ground in front of the wall with forward shin.
        let retracted = rig.retracted_neutral(&root_level, foot_head, &wall44, profile);
        assert!(rig.step_sole_climbable(&root_level, retracted, &wall44, profile));
        assert!(retracted.pos.y <= Fx::from_int(26), "expected ground sole height, got {:?}", retracted.pos);
        assert!(
            rig.sole.height(retracted.pos, retracted.head, retracted.normal, &wall44) <= Fx::from_int(26),
            "retracted sole probes must clear unclimbable wall face, got pos={:?}",
            retracted.pos,
        );
        let local_ret = to_local(&root_level, retracted.pos);
        let pose = rig.solve_contact(local_ret);
        assert!(pose.ankle.sub(pose.knee).dot(rig.rest_axis) > ZERO);

        // 4. Clearance and swing_contact_checked do not vault or teleport onto the wall roof.
        let pre_x = wall_x.sub(Fx::from_int(12));
        let from = rig.sole.contact(V3::new(pre_x, Fx::from_int(16), at_ahead.z.sub(Fx::from_int(16))), foot_head, &wall44);
        let graze_to = Contact { pos: V3::new(wall_x.add(Fx::from_int(2)), from.pos.y, at_ahead.z.add(Fx::from_int(16))), head: foot_head, normal: V3::UP };
        let lift = Fx::from_int(8);
        assert!(rig.sole.clearance(from, graze_to, lift, &wall44) > lift);
        let checked_clr = rig.sole_clearance_checked(&root_level, from, graze_to, lift, &wall44, Some(profile));
        assert_eq!(checked_clr, lift, "unclimbable wall graze must not inflate vertical arc");

        let foot = Foot {
            from,
            goal: Contact { pos: V3::new(wall_x.add(Fx::from_int(8)), from.pos.y, from.pos.z), head: foot_head + angle::FULL / 8, ..from },
            contact: Some(from),
            clearance: lift,
            swing: true,
            ..Foot::default()
        };
        let (held, waiting) = foot.swing_contact_checked(rig, Fx::from_ratio(1, 2), &wall44, Some((&root_level, profile)));
        assert!(waiting, "sole grazing unclimbable wall must hold pre-wall contact");
        assert!(held.pos.y < Fx::from_int(40), "sole must not teleport onto y=60 wall roof: {:?}", held.pos);
    }

    #[test]
    fn step_height_boundary_is_exact_and_independent_of_hull_pitch() {
        let body=voxel::unit::spider();let rig=rigs_of(&body).remove(0);
        let profile=modding::walking_terrain::Tuning::from_ron(include_str!("../../../data/walking-terrain.ron")).unwrap().spider;
        let base=V3::new(Fx::from_int(128),Fx::from_int(16),Fx::from_int(128));
        let depth=rig.sole.probes.iter().map(|p|p.y.neg()).max().unwrap();
        let low=base.y.add(rig.ankle.y).sub(depth);
        let limit=Fx::from_ratio(profile.max_step_cm,i32::from(voxel::unit::WORLD_MM)/10);
        let at=base.add(V3::new(Fx::from_int(50),ZERO,ZERO));
        for delta in [Fx(-1),ZERO,Fx(1)] {
            let high=low.add(limit).add(delta);
            let floor=|x:Fx,_z:Fx|if x>=base.x.add(Fx::from_int(25)){high}else{low};
            for up in [V3::UP,unit_or(v(-0.25,1.0,0.0),V3::UP),unit_or(v(0.25,1.0,0.0),V3::UP)] {
                let root=Frame::new(&body,base,0,up);
                assert_eq!(rig.step_climbable_to(&root,at,high,&floor,profile),delta<=ZERO,"delta={delta:?} up={up:?}");
                assert!(rig.step_climbable_to(&root,at,low.sub(Fx::from_int(100)),&floor,profile),"drop must keep its separate reach/Ready checks");
            }
        }
    }

    #[test]
    fn wall_finish_and_retry_restore_without_roof_ready_or_anchor_drift() {
        use nav::vehicle::support::{Sample,SupportRef,Surface,Validity};
        struct Ground<'a>(&'a dyn Fn(Fx,Fx)->Fx);
        impl Surface for Ground<'_> {
            fn sample(&self,x:Fx,_y:Fx,z:Fx)->Sample {
                let h=(self.0)(x,z);Sample{height:h,source:SupportRef{kind:0,id:0,content:h.raw() as u64,pose:0},validity:Validity::Supported}
            }
            fn validity(&self,_:SupportRef)->Validity {Validity::Supported}
        }
        let body=voxel::unit::spider();let rigs=rigs_of(&body);let rig=rigs.iter().max_by_key(|r|r.ankle.x).unwrap();
        let profile=modding::walking_terrain::Tuning::from_ron(include_str!("../../../data/walking-terrain.ron")).unwrap().spider;
        let root=Frame::new(&body,V3::new(Fx::from_int(1150),Fx::from_int(16),Fx::from_int(1024)),0,V3::UP);
        let at=to_world(&root,rig.lane_target(rig.ankle));let head=rig.lane.head;
        let tip=rig.sole.probes.iter().map(|p|turned(*p,head).x).max().unwrap();let edge=at.x.add(tip).sub(Fx::from_int(2));
        let floor=|x:Fx,_z:Fx|if x>=edge {Fx::from_int(60)}else{Fx::from_int(16)};
        let from=rig.retracted_neutral(&root,head,&floor,profile);
        let goal=rig.sole.contact(at,head.wrapping_add(angle::FULL/8),&floor);
        let mut foot=Foot{contact:Some(from),anchor:Some(from),from,goal,base:root.base,
            progress:ONE,swing:true,cadence:Fx::from_ratio(1,20),clearance:Fx::from_int(8),..Foot::default()};
        let mut bytes=blob::Blob::new();foot.save(&mut bytes);let mut copy=Foot::load(&mut blob::Scan::new(&bytes.v)).unwrap();
        let g=gait();let surface=Ground(&floor);
        let stance=Stance{grounded:true,root:&root,head:0,phase:0,lift_scale:ONE,linear:V3::ZERO,angular:0,cycles:Fx::from_ratio(1,20),gait:&g,floor:&floor};
        for _ in 0..12 {
            for f in [&mut foot,&mut copy] {
                let old_anchor=f.anchor;
                f.step_ready_traced(rig,&stance,false,false,true,ONE,&surface,profile,None);
                if f.planted() {f.assess_support(rig,&root,0,&surface,&floor,profile,g.lift,false,None);}
                if f.swing {assert_eq!(f.anchor,old_anchor,"retry cannot move the committed anchor");}
                let c=f.contact.unwrap();assert!(c.pos.y<Fx::from_int(40),"unclimbable roof contact {c:?}");
                if f.ready(){assert!(f.planted() && rig.step_sole_climbable(&root,c,&floor,profile));}
                proof_roundtrip(f);
            }
            assert_eq!(foot,copy);
        }
        assert!(
            foot.planted() && foot.ready(),
            "retracted goal clear of wall must settle into planted Ready contact instead of oscillating: {foot:?}",
        );
    }

    #[test]
    fn wall_toe_overlap_retracts_without_two_tick_progress_oscillation() {
        use nav::vehicle::support::{Sample, SupportRef, Surface, Validity};
        struct Ground<'a>(&'a dyn Fn(Fx, Fx) -> Fx);
        impl Surface for Ground<'_> {
            fn sample(&self, x: Fx, _y: Fx, z: Fx) -> Sample {
                let h = (self.0)(x, z);
                Sample {
                    height: h,
                    source: SupportRef { kind: 0, id: 0, content: h.raw() as u64, pose: 0 },
                    validity: Validity::Supported,
                }
            }
            fn validity(&self, _: SupportRef) -> Validity {
                Validity::Supported
            }
        }
        let body = voxel::unit::spider();
        let rigs = rigs_of(&body);
        let rig = rigs.iter().max_by_key(|r| r.ankle.x).unwrap();
        let profile = modding::walking_terrain::Tuning::from_ron(include_str!("../../../data/walking-terrain.ron"))
            .unwrap()
            .spider;
        let root = Frame::new(&body, V3::new(Fx::from_int(1150), Fx::from_int(16), Fx::from_int(1024)), 0, V3::UP);
        let head = rig.lane.head;
        let at = to_world(&root, rig.lane_target(rig.ankle));
        let corner = rig
            .sole
            .probes
            .iter()
            .map(|p| turned(*p, head))
            .max_by_key(|v| v.x.add(v.z))
            .unwrap();
        // Place a diagonal 44-high sheer wall so only the single outermost corner
        // probe crosses it: tilted Sole::contact pulls back below max_step
        // (step_climbable == true), while un-tilted flat_y hits y=60
        // (step_sole_climbable == false).
        let diag_edge = at.x.add(at.z).add(corner.x).add(corner.z).sub(Fx::from_ratio(1, 8));
        let floor = |x: Fx, z: Fx| if x.add(z) >= diag_edge { Fx::from_int(60) } else { Fx::from_int(16) };
        let toe_clip = rig.sole.contact(at, head, &floor);
        assert!(rig.step_climbable(&root, toe_clip, &floor, profile));
        assert!(!rig.step_sole_climbable(&root, toe_clip, &floor, profile));

        let from = rig.retracted_neutral(&root, head, &floor, profile);
        assert!(rig.step_sole_climbable(&root, from, &floor, profile));
        let mut foot = Foot {
            contact: Some(from),
            anchor: Some(from),
            from,
            goal: toe_clip,
            base: root.base,
            progress: ZERO,
            swing: true,
            cadence: Fx::from_ratio(1, 4),
            clearance: Fx::from_int(6),
            ..Foot::default()
        };
        let g = gait();
        let surface = Ground(&floor);
        let stance = Stance {
            grounded: true,
            root: &root,
            head: 0,
            phase: 0,
            lift_scale: ONE,
            linear: V3::ZERO,
            angular: 0,
            cycles: Fx::from_ratio(1, 4),
            gait: &g,
            floor: &floor,
        };
        let mut resets = 0;
        let mut prev_q = ZERO;
        for _ in 0..8 {
            foot.step_ready_traced(rig, &stance, false, false, true, ONE, &surface, profile, None);
            if foot.planted() {
                foot.assess_support(rig, &root, 0, &surface, &floor, profile, g.lift, false, None);
            }
            if foot.swing && foot.progress < prev_q {
                resets += 1;
            }
            prev_q = foot.progress;
        }
        assert_eq!(resets, 0, "toe-overlapping goal must retract before touchdown without progress resets");
        assert!(foot.planted() && foot.ready(), "foot must land and become Ready: {foot:?}");
    }

    #[test]
    fn close_sheer_wall_beyond_two_fifths_lane_retracts_to_inner_corridor_and_plants() {
        use nav::vehicle::support::{Sample, SupportRef, Surface, Validity};
        struct Ground<'a>(&'a dyn Fn(Fx, Fx) -> Fx);
        impl Surface for Ground<'_> {
            fn sample(&self, x: Fx, _y: Fx, z: Fx) -> Sample {
                let h = (self.0)(x, z);
                Sample {
                    height: h,
                    source: SupportRef { kind: 0, id: 0, content: h.raw() as u64, pose: 0 },
                    validity: Validity::Supported,
                }
            }
            fn validity(&self, _: SupportRef) -> Validity {
                Validity::Supported
            }
        }
        let body = voxel::unit::spider();
        let rigs = rigs_of(&body);
        let rig = &rigs[0];
        let profile = modding::walking_terrain::Tuning::from_ron(include_str!("../../../data/walking-terrain.ron"))
            .unwrap()
            .spider;
        // Exact scene-16 oblique geometry (hull at x=1121.5, z=999.25, head=24461)
        // facing an 8-unit-thick 44-high sheer wall at x in [1200, 1208]:
        // neutral (x~1212.8) straddles the far edge of the wall, and 2/5 still
        // clips x=1200, while inner lane candidates (1/4, 0/1) clear x<1200.
        let root = Frame::new(&body, V3::new(Fx::from_ratio(11215, 10), Fx::from_int(16), Fx::from_ratio(99925, 100)), 24461, V3::UP);
        let floor = |x: Fx, _z: Fx| {
            if x >= Fx::from_int(1200) && x <= Fx::from_int(1208) {
                Fx::from_int(60)
            } else {
                Fx::from_int(16)
            }
        };
        let foot_head = 24461i32.wrapping_add(rig.lane.head);
        let neutral = rig.sole.contact(to_world(&root, rig.lane_target(rig.ankle)), foot_head, &floor);
        assert!(
            !rig.step_sole_climbable(&root, neutral, &floor, profile),
            "thin sheer wall straddled by neutral must fail step_sole_climbable: {neutral:?}",
        );
        let retracted = rig.retracted_neutral(&root, foot_head, &floor, profile);
        assert!(
            rig.step_sole_climbable(&root, retracted, &floor, profile),
            "inner lane retraction must land in front of x=1200 clear of wall: {retracted:?}",
        );
        assert!(retracted.pos.x < Fx::from_int(1200), "retracted ankle must stay on near side of wall: {retracted:?}");

        let mut foot = Foot {
            contact: Some(retracted),
            anchor: Some(retracted),
            from: retracted,
            goal: neutral,
            base: root.base,
            progress: ZERO,
            swing: true,
            cadence: Fx::from_ratio(1, 3),
            clearance: Fx::from_int(8),
            ..Foot::default()
        };
        let g = gait();
        let surface = Ground(&floor);
        let stance = Stance {
            grounded: true,
            root: &root,
            head: 24461,
            phase: 0,
            lift_scale: ONE,
            linear: V3::ZERO,
            angular: 0,
            cycles: Fx::from_ratio(1, 3),
            gait: &g,
            floor: &floor,
        };
        for _ in 0..6 {
            foot.step_ready_traced(rig, &stance, false, false, true, ONE, &surface, profile, None);
            if foot.planted() {
                foot.assess_support(rig, &root, 24461, &surface, &floor, profile, g.lift, false, None);
            }
        }
        assert!(
            foot.planted() && foot.ready(),
            "rig 0 near thin sheer wall must retract to inner corridor and become Ready: {foot:?}",
        );
    }

}
