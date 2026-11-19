//! Real motor/foot regression yard: thin barriers, direct paths and move orders.
use fx::Fx;
use modding::range::RangeUnitKind;
use sim_core::{blast::motion::Motion,mobility::Mobility};
struct Yard {height:u32}
impl voxel::Generator for Yard {
 fn material(&self,x:u32,y:u32,_:u32)->u16 {
  u16::from(y<16 || (800..808).contains(&x) && y<16+self.height)
 }
}
fn main() {
 let args:Vec<_>=std::env::args().collect();
 let only=args.windows(2).find(|v|v[0]=="--height").map(|v|v[1].parse::<u32>().unwrap());
 let body=voxel::unit::spider();
 for height in [0,8,14,20,28,44] {for direct in [true,false] {
  if only.is_some_and(|v|v!=height) || only.is_some() && direct {continue;}
  let world=voxel::World::new(2048,512,2048,65536,Box::new(Yard {height}));
  let rubble=nav::vehicle::Rubble::new();let changed=nav::vehicle::Changed::default();
  let mut m=Motion::new([600,16,1024],RangeUnitKind::Spider,&body);m.enabled=true;
  let accepted=if direct {m.destination=Some([1100,1024]);m.path=vec![[1100,1024]];m.cursor=0;true} else {m.move_to(&world,[1100,1024],&body,&[],&rubble)};
  let mut blocked=0;let mut first=None;let mut zero=0;let mut min=8;let mut max_step=Fx::ZERO;let mut max_y=m.pos[1];
  let mut previous=vec![None;8];let mut old_planted=[false;8];let mut peaks=0;let mut stance_delta=Fx::ZERO;
  for tick in 0..560 {
   m.step(&world,&body,true,true,Mobility::intact(true),&[],tick,98,&rubble,&changed);
   if first.is_none() && m.pos[0]!=Fx::from_int(600) {first=Some(tick);}
   blocked+=u32::from(m.blocked);
   max_y=max_y.max(m.pos[1]);let root=m.frame(&body);
   let n=m.feet.iter().filter(|f|f.planted()).count(); min=min.min(n);zero+=u32::from(n==0);
   for (i,((old,foot),rig)) in previous.iter_mut().zip(&m.feet).zip(m.rigs()).enumerate() {
    let stored=foot.contact.unwrap().pos;let d=stored.sub(root.base);
    let local=phys::V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
    let solved=rig.solve_contact(local).ankle;
    let c=root.base.add(root.x.scale(solved.x)).add(root.y.scale(solved.y)).add(root.z.scale(solved.z));
    if let Some(p)=*old {let p:phys::V3=p;let delta=c.sub(p).len_sq().sqrt();max_step=max_step.max(delta);
     if old_planted[i] && foot.planted() {stance_delta=stance_delta.max(delta);}
     if only.is_some() && (delta>Fx::from_int(10) || old_planted[i] && foot.planted() && delta>Fx::from_ratio(3,20)) && peaks<12 {
      println!("foot_peak tick={tick} leg={i} delta={delta:?} planted={}->{} from={p:?} to={c:?} raw={stored:?} root={:?}",old_planted[i],foot.planted(),m.pos);peaks+=1;
     }
    }
    *old=Some(c);
    old_planted[i]=foot.planted();
   }
   if m.pos[0]>=Fx::from_int(1090) || !direct && m.destination.is_none() {break;}
  }
  let pose=nav::vehicle::Probe::new(&world).pose([m.pos[0],m.pos[2]],m.drive.head,m.probe_shape(&body));
  println!("spider_barrier height={height} direct={direct} accepted={accepted} first={first:?} x={} blocked={blocked} planted_min={min} zero={zero} foot_delta_mm={} stance_delta_mm={} max_root_y={} step={} steep={} tilt={} belly={} gait={:?}",m.pos[0].to_int_trunc(),max_step.raw()*100/(1i64<<32),stance_delta.raw()*100/(1i64<<32),max_y.to_int_trunc(),m.step,pose.steep,pose.tilt,pose.blocked,m.chassis.gait);
 }}
}
