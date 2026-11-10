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
    } else {
        v.scale(ONE.div(n.sqrt()))
    }
}

/// Rotation about the vertical to a heading: `+X` goes to `(cos, 0, sin)`,
/// the hull's own convention (`damage::Frame::new`).
fn turned(v: V3, head: i32) -> V3 {
    let (c, s) = (angle::cos(head), angle::sin(head));
    V3::new(v.x.mul(c).sub(v.z.mul(s)), v.y, v.x.mul(s).add(v.z.mul(c)))
}

/// Images of the axes under the shortest rotation taking unit `a` onto
/// unit `b` (Rodrigues: `w + v×w + v×(v×w)/(1+c)` with `v = a×b`,
/// `c = a·b`). Opposite vectors have no shortest arc and keep the axes.
fn rotation_arc(a: V3, b: V3) -> [V3; 3] {
    let v = a.cross(b);
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
    pub fn axes(&self) -> [V3; 3] {
        let m = rotation_arc(V3::UP, self.normal);
        [
            turned(m[0], self.head),
            turned(m[1], self.head),
            turned(m[2], self.head),
        ]
    }
    /// Between two contacts: the point on the segment, the heading along
    /// the shorter arc, the normal renormalised.
    pub fn lerp(self, o: Contact, t: Fx) -> Contact {
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

    /// Fit a plane over the sole at `at` with the given heading, then lift
    /// the ankle until every probe clears the terrain. The sole tilts on
    /// its own, whatever the hull does; a cliff cannot turn it vertical.
    pub fn contact(&self, at: V3, head: i32, floor: &dyn Fn(Fx, Fx) -> Fx) -> Contact {
        let n = Fx::from_int(self.probes.len() as i32);
        let mut heights = Vec::with_capacity(self.probes.len());
        let mut mean = V3::ZERO;
        for p in &self.probes {
            let w = at.add(turned(*p, head));
            let h = floor(w.x, w.z);
            heights.push(h);
            mean = mean.add(V3::new(p.x, h, p.z));
        }
        let mean = mean.scale(ONE.div(n));
        let (mut xx, mut xz, mut zz, mut xy, mut zy) = (ZERO, ZERO, ZERO, ZERO, ZERO);
        for (p, h) in self.probes.iter().zip(&heights) {
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
            pos: V3::new(at.x, self.height(at, head, normal, floor), at.z),
            head,
            normal,
        }
    }

    /// Ankle height at which no probe of the tilted sole is below the terrain.
    fn height(&self, at: V3, head: i32, normal: V3, floor: &dyn Fn(Fx, Fx) -> Fx) -> Fx {
        let m = rotation_arc(V3::UP, normal);
        let mut best: Option<Fx> = None;
        for p in &self.probes {
            let offset = turned(apply(&m, *p), head);
            let h = floor(at.x.add(offset.x), at.z.add(offset.z)).sub(offset.y);
            best = Some(best.map_or(h, |b| b.max(h)));
        }
        best.unwrap_or(at.y)
    }

    /// Arc height that clears the terrain between two contacts: eight to
    /// thirty-two route samples, each lifted over the sine arc, plus the
    /// gait's own lift.
    pub fn clearance(
        &self,
        from: Contact,
        to: Contact,
        lift: Fx,
        floor: &dyn Fn(Fx, Fx) -> Fx,
    ) -> Fx {
        let d = to.pos.sub(from.pos).len_sq().sqrt();
        let whole = d.to_int_trunc();
        let samples = if Fx::from_int(whole) < d {
            whole + 1
        } else {
            whole
        }
        .clamp(8, 32);
        let mut height = lift;
        for i in 1..samples {
            let q = Fx::from_ratio(i, samples);
            let eased = q.mul(q).mul(Fx::from_int(3).sub(q.mul(Fx::from_int(2))));
            let at = from.lerp(to, eased);
            let need = self.height(at.pos, at.head, at.normal, floor).sub(at.pos.y);
            let s = angle::sin(q.mul(Fx::from_int(angle::HALF)).to_int_trunc());
            if s > ZERO {
                height = height.max(need.div(s).add(lift));
            }
        }
        height
    }
}

/// A leg computed once from the body: rest joints in hull-local world
/// units, link lengths, bend direction, segment indices and the sole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegRig {
    pub hip: V3,
    pub knee: V3,
    pub ankle: V3,
    pub l1: Fx,
    pub l2: Fx,
    /// Where the knee bends: away from the hull, horizontally.
    pub bend: V3,
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
    b.legs
        .iter()
        .map(|l| {
            let (hip, knee, ankle) = (at(l.hip), at(l.knee), at(l.ankle));
            let along = unit_or(ankle.sub(hip), V3::ZERO);
            let off = knee.sub(hip);
            let side = off.sub(along.scale(off.dot(along)));
            let flat = V3::new(side.x, ZERO, side.z);
            let bend = if flat.len_sq() > Fx::from_ratio(1, 1_000_000) {
                unit_or(flat, X)
            } else {
                unit_or(V3::new(hip.x, ZERO, hip.z), X)
            };
            LegRig {
                hip,
                knee,
                ankle,
                l1: knee.sub(hip).len_sq().sqrt().max(MILLI),
                l2: ankle.sub(knee).len_sq().sqrt().max(MILLI),
                bend,
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
            }
        })
        .collect()
}

impl LegRig {
    /// Two-link inverse kinematics in the hull frame: the knee where the
    /// thigh's circle meets the shin's, on the bend side; an unreachable
    /// target is pulled to the leg's reach.
    pub fn solve(&self, target: V3) -> LegPose {
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
        let a = self
            .l1
            .mul(self.l1)
            .sub(self.l2.mul(self.l2))
            .add(dist.mul(dist))
            .div(dist.mul(Fx::from_int(2)));
        let h = self.l1.mul(self.l1).sub(a.mul(a)).max(ZERO).sqrt();
        let n = unit_or(self.bend.sub(u.scale(self.bend.dot(u))), V3::UP);
        LegPose {
            knee: self.hip.add(u.scale(a)).add(n.scale(h)),
            ankle: self.hip.add(u.scale(dist)),
        }
    }
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
    /// Accepted world displacement this tick.
    pub linear: V3,
    /// Yaw step this tick, binary angle.
    pub angular: i32,
    /// Phase advance this tick, as a fraction of the cycle.
    pub cycles: Fx,
    pub gait: &'a Gait,
    /// Terrain height under a world point.
    pub floor: &'a dyn Fn(Fx, Fx) -> Fx,
}

/// One foot's planting state: the contact it stands on, the swing it is
/// in, and where it was relative to the hull when the hull left the ground.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Foot {
    /// Where the ankle is after the last step; `None` before the first.
    pub contact: Option<Contact>,
    anchor: Option<Contact>,
    from: Contact,
    goal: Contact,
    base: V3,
    swing: bool,
    clearance: Fx,
    progress: Fx,
    /// Cycles per tick when the swing began: a stopped hull finishes the
    /// step at this pace.
    cadence: Fx,
    /// Hull-local ankle, heading offset and normal while airborne.
    local: Option<(V3, i32, V3)>,
}

impl Foot {
    /// Standing on its contact: on the ground and not lifted. A swing
    /// that has not progressed keeps the foot on its anchor.
    pub fn planted(&self) -> bool {
        self.anchor.is_some() && (!self.swing || self.progress == ZERO)
    }

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
        self.swing = false;
        self.progress = ZERO;
        self.local = None;
        self.contact = Some(c);
        c
    }

    /// Advance one tick and return the ankle's contact.
    pub fn step(&mut self, rig: &LegRig, s: &Stance<'_>) -> Contact {
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
        let swing = p >= g.duty;
        let reach = rig.l1.add(rig.l2);
        if self.anchor.is_none()
            || self.base.sub(root.base).len_sq() > reach.mul(reach).mul(Fx::from_int(4))
        {
            let rest = rig.sole.contact(to_world(root, rig.ankle), s.head, s.floor);
            self.anchor = Some(rest);
            self.from = rest;
            self.goal = rest;
            self.swing = false;
        }
        let anchor = self.anchor.unwrap_or(self.from);
        // A foot that already completed its swing while the hull was stopped
        // must not restart halfway through the same frozen swing interval.
        // Keep it planted until the phase returns to stance; the next swing
        // can then start normally without a jump to mid-air.
        let hold_resumed = swing && s.cycles > ZERO && self.cadence == ZERO
            && (self.swing || self.progress >= ONE);
        if swing && !hold_resumed && (!self.swing || (self.cadence == ZERO && s.cycles > ZERO)) {
            self.from = anchor;
            self.progress = ZERO;
            self.cadence = s.cycles;
            if s.cycles > ZERO {
                // Predict touchdown plus half of the next stance and freeze
                // it for the swing: terrain or velocity changes must not
                // move the landing point while the foot is in the air.
                //
                // The lead is the drive's — `frac` cycles at its phase rate —
                // and a walker barely stepping has thousands of ticks of it.
                // The motion it scales is everything that moved the hull,
                // a shove included: pushed while pivoting, a walker aimed its
                // foot thousands of units away, where the fixed-point square
                // of the reach wraps. The prediction goes no farther than
                // twice the drive's own path by then, and the turn no more
                // than half a turn; the products are taken whole (ADR-451).
                let frac = g.duty.mul(Fx::from_ratio(1, 2)).add(ONE.sub(p));
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
                self.goal = rig.sole.contact(at, s.head.wrapping_add(yaw), s.floor);
            }
            self.clearance = rig.sole.clearance(
                self.from,
                self.goal,
                g.lift.mul(s.lift_scale.max(ZERO).min(ONE)),
                s.floor,
            );
        }
        let target = if hold_resumed {
            self.progress = ZERO;
            anchor
        } else if swing {
            let span = ONE.sub(g.duty).max(MILLI);
            let q = if s.cycles > ZERO {
                self.progress
                    .max(p.sub(g.duty).div(span).max(ZERO).min(ONE))
            } else {
                // A stopped hull finishes a lifted step at the pace it
                // began with, neither frozen in the air nor dropped at once.
                let landing = if self.cadence > ZERO {
                    // A step started during acceleration can have an almost
                    // zero cadence. Once stopped, finish it in bounded time
                    // rather than suspending the foot for several seconds.
                    self.cadence.max(Fx::from_ratio(1, 80))
                } else { ZERO };
                self.progress.add(landing.div(span)).min(ONE)
            };
            self.progress = q;
            let eased = q.mul(q).mul(Fx::from_int(3).sub(q.mul(Fx::from_int(2))));
            let mut v = self.from.lerp(self.goal, eased);
            let arc = angle::sin(q.mul(Fx::from_int(angle::HALF)).to_int_trunc());
            v.pos = V3::new(v.pos.x, v.pos.y.add(arc.mul(self.clearance)), v.pos.z);
            v
        } else {
            if self.swing {
                self.anchor = Some(self.goal);
            }
            self.anchor.unwrap_or(anchor)
        };
        // When motion stops mid-step the frozen phase remains in the swing
        // interval. Once the foot has landed, commit that contact instead of
        // leaving it permanently marked as airborne until movement resumes.
        if swing && s.cycles == ZERO && self.progress >= ONE {
            self.anchor = Some(self.goal);
            self.swing = false;
            self.cadence = ZERO;
        } else {
            self.swing = swing;
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

    /// Keep a two-legged walker's contacts on their own side of its hull.
    /// A planted foot may slide when the chassis pivots faster than its next
    /// step; the saved anchors and swing endpoints must obey the same bound.
    pub(crate) fn keep_walker_side(&mut self, rig: &LegRig, root: &Frame,
        head: i32, spread: Fx, floor: &dyn Fn(Fx, Fx) -> Fx) {
        let sign = if rig.ankle.z < ZERO { -1 } else { 1 };
        // Scale with the authored body: Walker 04 is enlarged in-world, so
        // a fixed world-unit gap would be much narrower than either foot.
        let min_side = rig.ankle.z.abs().mul(spread);
        let max_yaw = angle::FULL / 24;
        let fix = |mut c: Contact| {
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
                c.pos = V3::new(pos.x, pos.y.max(grounded.pos.y), pos.z);
                c.head = limited;
                c.normal = grounded.normal;
            }
            c
        };
        self.anchor = self.anchor.map(fix);
        self.from = fix(self.from);
        self.goal = fix(self.goal);
        self.contact = self.contact.map(fix);
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
    }

    pub fn load(s: &mut blob::Scan) -> Option<Foot> {
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
        if !s.ok()
            || clearance < ZERO
            || clearance > Fx::from_int(1000)
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
            anchor,
            from,
            goal,
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
    let pose = rig.solve(to_local(root, contact.pos));
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
    fn rig() -> LegRig {
        let hip = v(-0.9, 2.7, -0.9);
        let knee = v(-1.3, 1.5, -1.3);
        let ankle = v(-1.1, 0.4, -1.1);
        LegRig {
            hip,
            knee,
            ankle,
            l1: knee.sub(hip).len_sq().sqrt(),
            l2: ankle.sub(knee).len_sq().sqrt(),
            bend: unit_or(v(-1.0, 0.0, -1.0), X),
            thigh: 0,
            shin: 1,
            knee_ball: None,
            ankle_ball: None,
            foot: None,
            support: 3,
            phase_shift: 0,
            sole: Sole::point(ankle.y),
        }
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
}
