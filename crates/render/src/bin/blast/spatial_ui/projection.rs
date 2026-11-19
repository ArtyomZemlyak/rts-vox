//! One bounded projector rig. Only decorative planes move; controls keep their hit boxes.
use super::*;

type Key = (u64, usize);
#[derive(Default)]
pub(super) struct Motion {
    pub key: Option<Key>,
    pub aperture: f32,
    pub energy: f32,
    pub clock: f32,
    pub last: Option<Rig>,
    tools: bool,
    pub work:bool,
    size_from:Vec2,size_goal:Vec2,size_age:f32,
}
impl Motion {
    pub fn step(&mut self, requested: Option<Key>, tools: bool, dt: f32) {
        let dt = dt.clamp(0., 0.1);
        self.clock += dt;
        self.energy = (self.energy - dt * 1.8).max(0.);
        if self.key.is_none() && requested.is_none() { self.aperture=0.;return; }
        if self.key.is_none() && requested.is_some() {
            self.key = requested;
            self.energy = 1.;
        }
        if self.key == requested {
            self.aperture = (self.aperture + dt / panel::theme::TRANSITION_SECONDS).min(1.);
            if self.tools != tools { self.energy = 0.7; }
        } else {
            self.aperture = (self.aperture - dt / panel::theme::REVEAL_SECONDS).max(0.);
            self.energy = self.energy.max(self.aperture * 0.6);
            if self.aperture == 0. {
                self.key = requested;
                self.last = None;
                self.energy = if requested.is_some() { 1. } else { 0. };
            }
        }
        self.tools = tools;
    }
    pub fn publish(&mut self,rig:Rig,dt:f32) {
        if self.last.is_none() {self.size_from=rig.size;self.size_goal=rig.size;self.size_age=0.18;}
        if self.size_goal!=rig.size {self.size_from=self.last.map_or(rig.size,|r|r.size);self.size_goal=rig.size;self.size_age=0.;}
        self.size_age=(self.size_age+dt).min(0.18);let t=self.size_age/0.18;let t=t*t*(3.-2.*t);
        self.last=Some(Rig{size:self.size_from.lerp(self.size_goal,t),..rig});
    }
    pub fn readable(&self, requested: Option<Key>) -> bool {
        requested.is_some() && self.key == requested && self.aperture >= 0.82
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Rig {
    pub source: Vec2,
    pub position: Vec2,
    pub size: Vec2,
    pub camera: Vec2,
}
impl Rig {
    pub fn planes(self, aperture: f32, _clock: f32) -> [[Vec2; 4]; 3] {
        let a = aperture * aperture * (3. - 2. * aperture);
        let corners = [self.position, self.position + Vec2::new(self.size.x, 0.),
            self.position + self.size, self.position + Vec2::new(0., self.size.y)];
        std::array::from_fn(|layer| {
            let depth = [0.78, 0.90, 1.][layer];
            // Rear planes respond only to viewing angle. Ink never moves.
            let parallax = self.camera * 5. * (1. - depth) * 3.;
            corners.map(|p| self.source.lerp(p, depth * a) + parallax * a)
        })
    }
    pub fn endpoint(self, ray: usize, aperture: f32, clock: f32) -> Vec2 {
        let p = self.planes(aperture, clock)[2];
        match ray { 0..=3 => p[ray], 4 => (p[0]+p[1])*0.5, _ => (p[2]+p[3])*0.5 }
    }
}

pub(super) fn width(viewport: Vec2) -> f32 { if viewport.x < 1200. { 256. } else { WIDTH } }
fn overlap(a: Vec2, size: Vec2, b: Vec2, other: Vec2) -> bool {
    a.x < b.x+other.x && a.x+size.x > b.x && a.y < b.y+other.y && a.y+size.y > b.y
}
/// Reject opaque overlap instead of clamping a panel back onto the selected hull.
pub(super) fn place(at: Vec2, half: Vec2, viewport: Vec2, size: Vec2, queue_height: f32) -> Option<Vec2> {
    place_avoiding(at,half,viewport,size,queue_height,&[])
}
pub(super) fn clears(at:Vec2,size:Vec2,objects:&[(Vec2,Vec2)])->bool {objects.iter().all(|(p,s)|!overlap(at,size,*p,*s))}
pub(super) fn place_avoiding(at: Vec2, half: Vec2, viewport: Vec2, size: Vec2, queue_height: f32,objects:&[(Vec2,Vec2)]) -> Option<Vec2> {
    let bottom = viewport.y - construction::dock_height(viewport) - 120.;
    let candidates = [
        Vec2::new(at.x+half.x+24., at.y-size.y*0.45),
        Vec2::new(at.x-half.x-24.-size.x, at.y-size.y*0.45),
        Vec2::new(at.x-size.x*0.5, at.y-half.y-24.-size.y),
        Vec2::new(at.x-size.x*0.5, at.y+half.y+24.),
        Vec2::new(16.,104.), Vec2::new(viewport.x-size.x-16.,queue_height+16.),
        Vec2::new(16.,bottom-size.y), Vec2::new(viewport.x-size.x-16.,bottom-size.y),
    ];
    candidates.into_iter().map(|p| Vec2::new(p.x.clamp(16.,(viewport.x-size.x-16.).max(16.)),
        p.y.clamp(104.,(bottom-size.y).max(104.))))
        .find(|p| allowed(*p,at,half,viewport,size,queue_height) && clears(*p,size,objects))
}
pub(super) fn allowed(p:Vec2,at:Vec2,half:Vec2,viewport:Vec2,size:Vec2,queue_height:f32)->bool {
    p.x>=16. && p.x+size.x<=viewport.x-16. && p.y>=104.
        && p.y+size.y<=viewport.y-construction::dock_height(viewport)-120.
        && !overlap(p,size,at-half-Vec2::splat(12.),half*2.+Vec2::splat(24.))
        && !overlap(p,size,viewport*0.5-Vec2::splat(44.),Vec2::splat(88.))
        && !overlap(p,size,Vec2::new(viewport.x-460.,0.),Vec2::new(460.,queue_height))
}

fn line(node: &mut Node, transform: Option<&mut UiTransform>, a: Vec2, b: Vec2, thickness: f32) {
    let d=b-a;let length=d.length().max(0.1);let mid=(a+b)*0.5;
    node.display=Display::Flex;node.width=px(length);node.height=px(thickness);
    node.left=px(mid.x-length*0.5);node.top=px(mid.y-thickness*0.5);
    if let Some(t)=transform {t.rotation=Rot2::radians(d.y.atan2(d.x));}
}
pub(super) fn decorate(part: Part, node: &mut Node, transform: Option<&mut UiTransform>,
    background: Option<&mut BackgroundColor>, rig: Rig, motion: &Motion, strength:f32) -> bool {
    let planes=rig.planes(motion.aperture,motion.clock);
    let active=motion.aperture>0.001 || matches!(part,Part::Emitter(_)|Part::SourceSpot);
    let flare=motion.energy;
    let alpha=match part {
        Part::FrameEdge(layer,edge) => {
            line(node,transform,planes[layer][edge],planes[layer][(edge+1)%4],if layer==2 {1.5}else{1.});
            ([0.24,0.36,0.66][layer]+flare*0.25)*motion.aperture
        }
        Part::Spoke(i) => {
            line(node,transform,planes[0][i],planes[2][i],1.);
            (0.25+flare*0.28)*motion.aperture
        }
        Part::Ray(i) => {
            line(node,transform,rig.source,rig.endpoint(i,motion.aperture,motion.clock),1.);
            (if i<4 {0.14}else{0.07})+flare*0.20
        }
        Part::Signal(i) => {
            if motion.energy<=0.05 && !motion.work {node.display=Display::None;return true;}
            let phase=(motion.clock/if flare>0.15 {1.1}else{4.8}+i as f32/6.).fract();
            let end=rig.endpoint(i,motion.aperture,motion.clock);
            let p=rig.source.lerp(end,phase);
            let direction=(end-rig.source).normalize_or_zero();
            line(node,transform,p-direction*(5.+flare*10.),p,if flare>0.15 {2.}else{1.});
            (0.22+flare*0.7)*(core::f32::consts::PI*phase).sin()
        }
        Part::Vertex(layer,i) => {
            let p=planes[layer][i];let size=if layer==2 {4.}else{2.};
            node.display=Display::Flex;node.left=px(p.x-size*0.5);node.top=px(p.y-size*0.5);
            node.width=px(size);node.height=px(size);(0.45+flare*0.5)*motion.aperture
        }
        Part::Emitter(i) => {
            let size=if i==0 {5.+flare*3.}else{18.+flare*18.};
            node.display=Display::Flex;node.left=px(rig.source.x-size*0.5);node.top=px(rig.source.y-size*0.5);
            node.width=px(size);node.height=px(size*if i==0 {1.}else{0.5});
            if i==0 {0.8+flare*0.2}else{0.12+flare*0.22}
        }
        Part::SourceSpot => {
            let size=34.+flare*30.;node.display=Display::Flex;
            node.left=px(rig.source.x-size*0.5);node.top=px(rig.source.y-size*0.20);
            node.width=px(size);node.height=px(size*0.4);0.025+flare*0.08
        }
        _ => return false,
    };
    if let Some(bg)=background { bg.0=CYAN.with_alpha((alpha*strength).clamp(0.,1.)); }
    if !active {node.display=Display::None;}
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn another_actor_rejects_the_preferred_projection_slot(){
        let at=Vec2::new(800.,400.);let half=Vec2::new(70.,60.);let view=Vec2::new(1920.,1080.);let size=Vec2::new(328.,145.);
        let first=place(at,half,view,size,120.).unwrap();let objects=[(first,size)];
        let next=place_avoiding(at,half,view,size,120.,&objects).unwrap();assert!(clears(next,size,&objects));assert_ne!(next,first);
        assert_eq!(place_avoiding(at,half,view,size,120.,&[(Vec2::ZERO,view)]),None);
    }
    #[test]
    fn opening_settles_and_exit_returns_to_the_same_source_before_retarget() {
        let mut m=Motion::default();
        m.step(Some((1,2)),false,0.06);assert!(m.aperture>0. && !m.readable(Some((1,2))));
        for _ in 0..20 {m.step(Some((1,2)),false,0.05);}
        assert!(m.readable(Some((1,2))));assert_eq!(m.aperture,1.);assert_eq!(m.energy,0.);
        m.step(Some((1,3)),false,0.05);assert_eq!(m.key,Some((1,2)));assert!(!m.readable(Some((1,3))));
        for _ in 0..4 {m.step(Some((1,3)),false,0.05);}
        assert_eq!(m.key,Some((1,3)));assert!(m.aperture<0.5);
        for _ in 0..10 {m.step(None,false,0.05);}
        assert_eq!(m.key,None);assert_eq!(m.aperture,0.);
    }
    #[test]
    fn perspective_planes_converge_to_the_hull_and_only_rear_frames_have_parallax() {
        let r=Rig{source:Vec2::new(400.,400.),position:Vec2::new(600.,140.),size:Vec2::new(328.,120.),camera:Vec2::new(0.3,-0.5)};
        let closed=r.planes(0.,0.);assert!(closed.iter().flatten().all(|p|*p==r.source));
        let a=r.planes(1.,0.);let b=r.planes(1.,2.);
        assert_eq!(a[2],b[2]);assert_eq!(a[0],b[0],"idle decoration does not cycle");
        let moved=Rig{camera:r.camera+Vec2::splat(0.2),..r}.planes(1.,2.);assert_ne!(a[0],moved[0]);assert_eq!(a[2],moved[2]);
        assert_eq!(a[2][0],r.position);assert_eq!(a[2][2],r.position+r.size);
        assert!((a[0][0]-r.source).length()<(a[1][0]-r.source).length());
    }
    #[test]
    fn opaque_controls_avoid_hull_aim_queue_and_static_dock_or_decline() {
        for viewport in [Vec2::new(960.,540.),Vec2::new(1600.,900.),Vec2::new(3840.,2160.)] {
            for at in [viewport*0.5,Vec2::new(100.,200.),viewport-Vec2::new(100.,200.)] {
                let half=Vec2::new(60.,50.);let size=Vec2::new(width(viewport),112.);
                if let Some(p)=place(at,half,viewport,size,140.) {
                    assert!(!overlap(p,size,at-half-Vec2::splat(12.),half*2.+Vec2::splat(24.)));
                    assert!(!overlap(p,size,viewport*0.5-Vec2::splat(44.),Vec2::splat(88.)));
                    assert!(p.y+size.y<=viewport.y-construction::dock_height(viewport)-120.);
                }
            }
        }
        assert!(place(Vec2::new(480.,270.),Vec2::new(500.,300.),Vec2::new(960.,540.),Vec2::new(232.,120.),140.).is_none());
    }
}
