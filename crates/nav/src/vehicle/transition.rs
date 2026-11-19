//! One admission result, refined from corridor bounds to live contact evidence.
//! Geometry is cheap and source resolution is explicit: a route is never Ready.
use super::{support::{Sample, Validity}, Pose, Probe, Shape};
use fx::{angle, Fx};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level { Coarse, Fine, Motor }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Drive, StepUp, CrossGap, DropDown }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Bounds, Obstacle, Tilt, Normal, RequiredLift, DropLimit,
    Moving, Unsupported, Removed, LandingRejected, WindowUnready,
    Reach, Lane, ReachLane, VerticalStep, Diagonal, NoHandoff, NoBrake, SpeedBound, TickBound, DepartingSpan, DepartingHeight, StanceSpan, StanceHeight, Budget,
}

impl Reason {
    pub fn as_str(self)->&'static str {match self {
        Self::Bounds=>"bounds",Self::Obstacle=>"obstacle",Self::Tilt=>"tilt",Self::Normal=>"normal",
        Self::RequiredLift=>"required_lift",Self::DropLimit=>"drop_limit",Self::Moving=>"moving",
        Self::Unsupported=>"unsupported",Self::Removed=>"removed",Self::LandingRejected=>"landing_rejected",
        Self::WindowUnready=>"window_unready",Self::Reach=>"current_reach",Self::Lane=>"current_lane",
        Self::ReachLane=>"future_reach_lane",Self::VerticalStep=>"future_vertical_step",Self::Diagonal=>"future_diagonal",
        Self::NoHandoff=>"window_no_handoff",Self::NoBrake=>"window_no_brake",Self::SpeedBound=>"window_speed_bound",
        Self::TickBound=>"window_tick_bound",Self::DepartingSpan=>"future_departing_span",Self::DepartingHeight=>"future_departing_height",
        Self::StanceSpan=>"future_stance_span",Self::StanceHeight=>"future_stance_height",Self::Budget=>"budget",
    }}
}

/// Explicit caller policy. Passive falling is distinct from a planned landing;
/// it does not turn a moving fragment into a floor or grant a foot Ready.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub level: Level,
    pub lift: Fx,
    pub drop: Option<Fx>,
    pub grade_exit: bool,
    pub grade_slack: Option<Fx>,
    pub overlap_exit: bool,
}
impl Policy {
    #[inline]
    pub fn fine(shape: Shape, grade_exit: bool) -> Self {
        Self {level:Level::Fine, lift:Fx::from_int(shape.step),
            drop:Some(Fx::from_int(shape.step)), grade_exit,
            grade_slack: Some(shape.grade_slack), overlap_exit:false}
    }
    #[inline]
    pub fn motor(shape: Shape, grade_exit: bool, overlap_exit: bool) -> Self {
        Self {level:Level::Motor, overlap_exit, ..Self::fine(shape,grade_exit)}
    }
    #[inline]
    pub fn passive(shape:Shape,grade_exit:bool,overlap_exit:bool)->Self {
        Self {drop:None,..Self::motor(shape,grade_exit,overlap_exit)}
    }
    #[inline]
    pub fn unbounded_grade_exit(mut self) -> Self {
        self.grade_slack = None;
        self
    }
}

/// Actual sole evidence supplied by the owner; footprint probes cannot invent
/// these votes. Signature retains mixed-source coverage from occupied probes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoleEvidence {
    pub leg:usize,
    pub at:[Fx;3],
    pub probe_at:[Fx;3],
    pub source:super::support::SupportRef,
    pub signature:u64,
    pub supported_mask:u16,
    pub samples:u8,
    pub planted:bool,
    pub ready:bool,
    pub validity:Validity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionProof {
    pub level: Level,
    pub kind: Kind,
    pub from: [Fx; 2],
    pub to: [Fx; 2],
    pub requested_to:[Fx;2],
    pub heads: [i32;2],
    /// Installed span, including the same 2*k <= span bridge rule as Probe.
    pub span: Fx,
    pub required_lift: Fx,
    pub drop: Fx,
    pub lift_limit:Fx,
    pub drop_limit:Option<Fx>,
    pub from_floors:[Fx;4],
    pub to_floors:[Fx;4],
    pub from_height: Fx,
    pub to_height: Fx,
    /// Live surface levels at queried witnesses: footprint for planning/belts,
    /// occupied sole points for walking motors. Empty means geometry only.
    pub sources: Vec<Sample>,
    pub soles:Vec<SoleEvidence>,
    pub first_refusal: Option<Reason>,
    pub sources_checked: bool,
    pub checked_samples:u32,
    pub failure_at:Option<[Fx;2]>,
}
impl TransitionProof {
    #[inline]
    pub fn geometry(from:[Fx;2],to:[Fx;2],old:Pose,next:Pose,shape:Shape,p:Policy)->Self {
        let delta=next.height.sub(old.height);
        let lift=delta.max(Fx::ZERO);let drop=delta.neg().max(Fx::ZERO);
        let grade_refused = next.steep
            && (!p.grade_exit || p.grade_slack.is_some_and(|slack| next.excess > old.excess.add(slack)));
        let reason=if next.blocked!=0 && !p.overlap_exit {Some(Reason::Obstacle)}
            else if next.tilt {Some(Reason::Tilt)}
            else if grade_refused {Some(Reason::Normal)}
            else if lift>p.lift {Some(Reason::RequiredLift)}
            else if p.drop.is_some_and(|max|drop>max) {Some(Reason::DropLimit)}else{None};
        Self {level:p.level,kind:if drop>Fx::ZERO {Kind::DropDown}else if lift>Fx::ZERO {Kind::StepUp}else{Kind::Drive},
            from,to,requested_to:to,heads:[0,0],span:Fx::from_int(shape.span.max(0)),required_lift:lift,drop,
            lift_limit:p.lift,drop_limit:p.drop,from_floors:old.floors,to_floors:next.floors,from_height:old.height,to_height:next.height,sources:Vec::new(),soles:Vec::new(),first_refusal:reason,sources_checked:false,checked_samples:0,failure_at:reason.map(|_|to)}
    }
    #[inline]
    pub fn geometry_accepted(old: Pose, next: Pose, p: Policy) -> bool {
        if next.blocked != 0 && !p.overlap_exit {
            return false;
        }
        if next.tilt {
            return false;
        }
        if next.steep
            && (!p.grade_exit || p.grade_slack.is_some_and(|slack| next.excess > old.excess.add(slack)))
        {
            return false;
        }
        let delta = next.height.sub(old.height);
        let lift = delta.max(Fx::ZERO);
        if lift > p.lift {
            return false;
        }
        let drop = delta.neg().max(Fx::ZERO);
        !p.drop.is_some_and(|max| drop > max)
    }
    #[inline]
    pub fn corridor_accepted(shape: Shape, low: i32, high: i32) -> bool {
        Fx::from_int(high).sub(Fx::from_int(low)).max(Fx::ZERO) <= Fx::from_int(shape.step)
    }
    #[inline]
    pub fn accepted(&self)->bool {self.first_refusal.is_none()}
    /// Preserve the first failure in the caller's ordered checks.
    pub fn refuse(&mut self,reason:Reason) {if self.first_refusal.is_none() {self.first_refusal=Some(reason);self.failure_at=Some(self.to);}}
    /// A conservative coarse interval cannot name individual contact sources.
    /// Its refinement obligation is explicit, including prepared fast paths.
    #[inline]
    pub fn corridor(from:[Fx;2],to:[Fx;2],shape:Shape,low:i32,high:i32)->Self {
        let mut p=Self::geometry(from,to,Pose {height:Fx::from_int(low),floors:[Fx::ZERO;4],blocked:0,steep:false,excess:Fx::ZERO,tilt:false,crush:0},
            Pose {height:Fx::from_int(high),floors:[Fx::ZERO;4],blocked:0,steep:false,excess:Fx::ZERO,tilt:false,crush:0},shape,Policy {drop:None,..Policy::fine(shape,false)});
        p.level=Level::Coarse;p
    }
}

impl Probe<'_> {
    /// Refine a geometry result against the same live source catalogue. Eight
    /// bounded rays, never all cells of a body. Used by diagnostics or a caller
    /// that needs source admission; plain route geometry keeps its fast path.
    pub fn resolve_transition_sources(&mut self,proof:&mut TransitionProof,heads:[i32;2],shape:Shape) {
        if proof.sources_checked {return;}
        proof.heads=heads;
        let mut gap=false;
        for ((at,head),floors) in [proof.from,proof.to].into_iter().zip(heads).zip([proof.from_floors,proof.to_floors]) {
            let (c,s)=(angle::cos(head),angle::sin(head));
            for (i,(x,z)) in [(-1,-1),(1,-1),(-1,1),(1,1)].into_iter().enumerate() {
                let u=Fx::from_int(x*shape.support[0]);let v=Fx::from_int(z*shape.support[1]);
                let px=at[0].add(c.mul(u)).sub(s.mul(v));let pz=at[1].add(s.mul(u)).add(c.mul(v));
                let sample=self.support_sample_at(px,pz,Some(floors[i]));
                // Name the actual two-sided bridge witnesses. A low ray does
                // not become a fabricated high source, nor is span doubled.
                let needs_bridge=sample.height.add(Fx::from_int(shape.step.max(0)))<floors[i];
                if needs_bridge && shape.span>=2 && sample.validity==Validity::Supported {
                    for k in 1..=(shape.span/2).min(64) {
                        let d=Fx::from_int(k);
                        let a=self.support_sample_at(px.sub(c.mul(d)),pz.sub(s.mul(d)),Some(floors[i]));
                        let b=self.support_sample_at(px.add(c.mul(d)),pz.add(s.mul(d)),Some(floors[i]));
                        if a.validity==Validity::Supported && b.validity==Validity::Supported
                            && a.height.min(b.height).add(Fx::from_ratio(1,1000))>=floors[i] {
                            proof.sources.extend([a,b]);gap=true;break;
                        }
                    }
                }
                match sample.validity {
                    Validity::Moving=>proof.refuse(Reason::Moving),
                    Validity::Unsupported=>{gap=true;proof.refuse(Reason::Unsupported);},
                    Validity::Removed=>proof.refuse(Reason::Removed),Validity::Supported=>{},
                }
                proof.sources.push(sample);
            }
        }
        proof.sources_checked=true;
        if gap && proof.span>Fx::ZERO && proof.kind==Kind::Drive {proof.kind=Kind::CrossGap;}
    }
    /// Bounded swept explanation of the same turn/run used by Connection.
    /// Exhaustion refuses; it never accepts an unchecked suffix. Sources are
    /// query-local evidence and are deliberately not stored as a permission.
    pub fn transition_proof(&mut self,from:[Fx;2],head:i32,to:[Fx;2],yaw:i32,shape:Shape,policy:Policy)->TransitionProof {
        let mut old=self.pose(from,head,shape);
        let mut proof=TransitionProof::geometry(from,from,old,old,shape,Policy {grade_exit:true,..policy});
        self.resolve_transition_sources(&mut proof,[head,head],shape);
        proof.checked_samples=1;proof.requested_to=to;
        if !proof.accepted() {return proof;}
        let delta=[to[0].sub(from[0]),to[1].sub(from[1])];
        let turns=super::connection::turn_samples(angle::diff(yaw,head),shape);
        let steps=(delta[0].abs().max(delta[1].abs()).to_int_trunc()+1).max(1);
        let mut previous=from;let mut previous_head=head;
        for cursor in 1..=turns.saturating_add(steps) {
            if proof.checked_samples>=80 {proof.refuse(Reason::Budget);break;}
            let (at,h)=if cursor<=turns {(from,angle::norm(head+angle::diff(yaw,head)*cursor/turns))}
                else {let f=Fx::from_ratio(cursor-turns,steps);([from[0].add(delta[0].mul(f)),from[1].add(delta[1].mul(f))],yaw)};
            let next=self.pose(at,h,shape);
            let mut part=TransitionProof::geometry(previous,at,old,next,shape,Policy {grade_exit:policy.grade_exit && old.steep,..policy});
            self.resolve_transition_sources(&mut part,[previous_head,h],shape);
            proof.checked_samples+=1;proof.to=at;proof.to_height=next.height;proof.to_floors=next.floors;proof.heads[1]=h;
            proof.required_lift=proof.required_lift.max(part.required_lift);proof.drop=proof.drop.max(part.drop);
            if part.kind!=Kind::Drive {proof.kind=part.kind;}
            proof.sources.extend(part.sources);
            if let Some(reason)=part.first_refusal {proof.refuse(reason);break;}
            old=next;previous=at;previous_head=h;
        }
        proof
    }
}

#[cfg(test)]
#[path="transition_tests.rs"]
mod tests;
