use super::*;
use modding::range::RangeUnitKind;

fn models() -> Vec<(RangeUnitKind,UnitBody)> {
    vec![(RangeUnitKind::Commander,voxel::unit::commander()),
        (RangeUnitKind::Walker04,voxel::unit::walker04()),
        (RangeUnitKind::Strider,voxel::unit::strider()),
        (RangeUnitKind::Spider,voxel::unit::spider())]
}
struct Ramp;

struct Level;
impl voxel::Generator for Level {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 { u16::from(y < 16) }
    fn batches_columns(&self)->bool {true}
    fn uniform_box(&self,min:[u32;3],max:[u32;3])->Option<u16> {
        if max[1]<=16 {Some(1)} else if min[1]>=16 {Some(0)} else {None}
    }
    fn column(&self,_:u32,_:u32,y0:u32,out:&mut[u16]) {
        out.fill(0);
        let end=(16u32.saturating_sub(y0) as usize).min(out.len());
        out[..end].fill(1);
    }
}

#[test]
fn healthy_multipeds_keep_world_anchors_until_liftoff() {
    for (kind,body,minimum) in [(RangeUnitKind::Spider,voxel::unit::spider(),4),
        (RangeUnitKind::Strider,voxel::unit::strider(),2)] {
        let world=World::new(2048,512,2048,65536,Box::new(Level));
        let mut m=Motion::new([1024,16,1024],kind,&body);
        m.enabled=true;
        let rubble=nav::vehicle::Rubble::new();let changed=nav::vehicle::Changed::default();
        let top=Fx::from_ratio(m.speed,TICKS);let mut forward=Fx::ZERO;
        let cube=Fx::from_ratio(i32::from(body.mm),i32::from(voxel::unit::WORLD_MM));
        let mut turned=0i32;
        for tick in 0..500 {
            let old=m.feet.clone();let at=m.pos;let head=m.drive.head;
            m.pilot=Some(match tick {0..=79=>[1,0],80..=109=>[0,0],110..=159=>[0,1],
                160..=199=>[-1,0],200..=259=>[1,-1],_=>[0,0]});
            m.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),
                &[],tick,98,&rubble,&changed);
            assert!(m.feet.iter().filter(|f|f.planted()).count()>=minimum,"{kind:?} support tick {tick}");
            assert!(m.pos[1].sub(at[1]).abs()<=cube,"{kind:?} root height snapped: tick {tick}");
            let root=m.frame(&body);
            for (i,(rig,foot)) in m.rigs.iter().zip(&m.feet).enumerate() {
                if !foot.planted() {continue;}
                let c=foot.contact.unwrap();let d=c.pos.sub(root.base);
                let local=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                let solved=rig.solve_contact(local).ankle;
                assert!(solved.sub(local).len_sq()<=Fx::from_ratio(1,20).mul(Fx::from_ratio(1,20)),
                    "{kind:?} unreachable stance: tick={tick} leg={i}");
                let ankle=root.base.add(root.x.scale(solved.x)).add(root.y.scale(solved.y)).add(root.z.scale(solved.z));
                let axes=c.axes();
                let gap=rig.sole.probes().iter().map(|p|ankle.add(axes[0].scale(p.x))
                    .add(axes[1].scale(p.y)).add(axes[2].scale(p.z)).y.sub(Fx::from_int(16))).min().unwrap();
                assert!(gap.abs()<=Fx::from_ratio(1,50),"{kind:?} sole gap {gap:?}: tick={tick} leg={i}");
            }
            for (i,(was,now)) in old.iter().zip(&m.feet).enumerate() {
                if was.planted() && now.planted() {
                    assert_eq!(was.contact,now.contact,"{kind:?} anchor moved: tick={tick} leg={i}");
                }
            }
            if (50..80).contains(&tick) {forward=forward.add(m.pos[0].sub(at[0]));}
            if (110..160).contains(&tick) {turned+=angle::diff(m.drive.head,head).abs();}
        }
        assert!(forward>=top.mul(Fx::from_int(30)).mul(Fx::from_ratio(90,100)),
            "{kind:?} forward progress {forward:?}");
        assert!(turned>=angle::FULL/12,"{kind:?} pivot stalled: {turned}");
        assert!(m.feet.iter().all(|f|f.planted()),"{kind:?} failed to settle");
    }
}

#[test]
fn strider_keeps_two_real_stance_feet_through_start_turn_reverse_and_stop() {
    let world = World::new(2048, 512, 2048, 65536, Box::new(Level));
    let body = voxel::unit::strider();
    let mut motion = Motion::new([1024, 16, 1024], RangeUnitKind::Strider, &body);
    motion.enabled = true;
    motion.speed = 84;
    let rubble = nav::vehicle::Rubble::new();
    let changed = nav::vehicle::Changed::default();
    let mut least = 4;
    let mut previous_height = motion.pos[1];
    for tick in 0..400 {
        motion.pilot = Some(match tick {
            0..=79 => [1, 0], 80..=109 => [0, 0], 110..=159 => [0, 1],
            160..=199 => [-1, 0], 200..=259 => [1, -1], _ => [0, 0],
        });
        motion.step(&world, &body, true, true, crate::mobility::Mobility::intact(true),
            &[], tick, 98, &rubble, &changed);
        let root = motion.frame(&body);
        assert!(motion.pos[1].sub(previous_height).abs() <= Fx::from_ratio(1, 2),
            "flat strider jumped at tick {tick}: {:?} -> {:?}",previous_height,motion.pos[1]);
        previous_height = motion.pos[1];
        let supported = motion.rigs.iter().zip(&motion.feet).filter(|(rig, foot)| {
            if !foot.planted() { return false; }
            let Some(contact) = foot.contact else { return false; };
            let d = contact.pos.sub(root.base);
            let target = V3::new(d.dot(root.x), d.dot(root.y), d.dot(root.z));
            let solved = rig.solve_contact(target).ankle;
            let ankle = root.base.add(root.x.scale(solved.x)).add(root.y.scale(solved.y)).add(root.z.scale(solved.z));
            let axes = contact.axes();
            let gap = rig.sole.probes().iter().map(|p|
                ankle.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z)).y
                    .sub(Fx::from_int(16))).min().unwrap();
            solved.sub(target).len_sq() <= Fx::from_ratio(1, 20).mul(Fx::from_ratio(1, 20))
                && gap.abs() <= Fx::from_ratio(1, 50)
        }).count();
        least = least.min(supported);
        assert!(supported >= 2, "strider has only {supported} real stance feet at tick {tick}; root={:?}", motion.pos);
    }
    println!("STRIDER_SUPPORT minimum={least}");
}
struct StriderHill;
impl StriderHill {
    fn top(x:i32)->i32 {
        16 + if x<1180 {(x-1100).clamp(0,80)*12/80}
            else if x<1220 {12} else {(1300-x).clamp(0,80)*12/80}
    }
}
impl voxel::Generator for StriderHill {
    fn material(&self,x:u32,y:u32,_:u32)->u16 {u16::from((y as i32)<Self::top(x as i32))}
}
#[test]
fn strider_crosses_a_hill_without_a_second_height_snap_or_unreachable_supports() {
    let world=World::new(2048,512,2048,65536,Box::new(StriderHill));
    let body=voxel::unit::strider();
    let mut motion=Motion::new([1024,16,1024],RangeUnitKind::Strider,&body);
    motion.enabled=true;motion.speed=84;
    let rubble=nav::vehicle::Rubble::new();
    let changed=nav::vehicle::Changed::default();
    let mut previous=motion.pos[1];let mut maximum=Fx::ZERO;
    let cube=Fx::from_ratio(i32::from(body.mm),i32::from(voxel::unit::WORLD_MM));
    for tick in 0..300 {
        motion.pilot=Some(if tick<260 {[1,0]} else {[0,0]});
        motion.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),
            &[],tick,98,&rubble,&changed);
        let dy=motion.pos[1].sub(previous).abs();previous=motion.pos[1];maximum=maximum.max(dy);
        assert!(dy<=cube,"strider snapped vertically on the hill at {tick}: {dy:?}");
        let root=motion.frame(&body);
        let supported=motion.rigs.iter().zip(&motion.feet).filter(|(rig,foot)| {
            if !foot.planted() {return false;}
            let Some(c)=foot.contact else {return false;};
            let d=c.pos.sub(root.base);
            let target=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
            let solved=rig.solve_contact(target).ankle;
            if solved.sub(target).len_sq()>Fx::from_ratio(1,20).mul(Fx::from_ratio(1,20)) {return false;}
            let ankle=root.base.add(root.x.scale(solved.x)).add(root.y.scale(solved.y)).add(root.z.scale(solved.z));
            let axes=c.axes();let mut closest=Fx::MAX;
            for p in rig.sole.probes() {
                let at=ankle.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z));
                let gap=at.y.sub(Fx::from_int(StriderHill::top(at.x.to_int_trunc())));
                if gap<Fx::from_ratio(-1,50) {return false;}
                closest=closest.min(gap.abs());
            }
            closest<=Fx::from_ratio(1,50)
        }).count();
        assert!(supported>=2,"strider lost actual sole support on the hill at {tick}: {supported}");
    }
    assert!(motion.pos[0]>Fx::from_int(1900),"support admission stalled the hill crossing");
    println!("STRIDER_HILL maximum_dy={maximum:?} end={:?}",motion.pos);
}

struct SpiderRail { height: u32 }
impl voxel::Generator for SpiderRail {
    fn material(&self,x:u32,y:u32,_:u32)->u16 {
        u16::from(y<16 || (800..808).contains(&x) && y<16+self.height)
    }
}
#[test]
fn spider_crosses_thin_rails_both_ways_without_stalling_or_losing_all_supports() {
    let body=voxel::unit::spider();
    let rubble=nav::vehicle::Rubble::new();
    let mut recovered=false;let mut tripod=false;
    for height in [0,8,20,28] {for reverse in [false,true] {
        let world=World::new(2048,512,2048,65536,Box::new(SpiderRail {height}));
        let (start,end)=if reverse {(1100,600)} else {(600,1100)};
        let mut m=Motion::new([start,16,1024],RangeUnitKind::Spider,&body);
        m.enabled=true;m.drive.head=if reverse {angle::HALF} else {0};
        assert!(m.move_to(&world,[end,1024],&body,&[],&rubble));
        let mut first=None;let mut blocked=0;let mut least=8;
        let mut last_x=m.pos[0];let mut stagnant=0;let mut longest_wait=0;let mut inactive=0;let mut arrival=None;let mut last_landing=None;
        m.trace_anchors(true);
        for tick in 0..520 {
            let before=m.feet.clone();
            m.clear_anchor_trace();
            m.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),
                &[],tick,98,&rubble,&nav::vehicle::Changed::default());
            if m.pos[0]!=Fx::from_int(start) && first.is_none() {first=Some(tick);}
            blocked+=usize::from(m.blocked);
            if m.hull.grounded() {least=least.min(m.feet.iter().filter(|f|f.planted()).count());}
            recovered|=m.anchor_trace().unwrap().events().iter().any(|e|e.reason==crate::gait::AnchorReason::SupportRecovery);
            let root=m.frame(&body);let mut supports=Vec::new();
            let eps=Fx::from_ratio(1,20);
            for (i,(rig,foot)) in m.rigs.iter().zip(&m.feet).enumerate().filter(|(_,(_,f))|f.planted()) {
                let c=foot.contact.unwrap();let d=c.pos.sub(root.base);
                let local=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                let solved=rig.solve_contact(local).ankle;
                assert!(solved.sub(local).len_sq()<=eps.mul(eps),"rail {height}/{reverse} unreachable planted foot {i} tick={tick} error={:?} root={:?} up={:?} local={local:?} hip={:?} contact={c:?} old_planted={} unready={}",solved.sub(local).len_sq().sqrt(),m.pos,m.hull.up,rig.hip,before[i].planted(),m.anchor_trace().unwrap().window_unready);
                let ankle=root.base.add(root.x.scale(solved.x)).add(root.y.scale(solved.y)).add(root.z.scale(solved.z));
                let axes=c.axes();let mut closest=Fx::MAX;
                for p in rig.sole.probes() {
                    let at=ankle.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z));
                    let floor=Fx::from_int(16+if (800..808).contains(&at.x.to_int_trunc()){height as i32}else{0});
                    let gap=at.y.sub(floor);
                    assert!(gap>=eps.neg(),"rail sole penetration {height}/{reverse} foot={i} tick={tick}");
                    closest=closest.min(gap.abs());
                }
                assert!(closest<=eps,"rail false planted flag {height}/{reverse} foot={i} tick={tick}");
                supports.push(c.pos);
            }
            if supports.len()==3 {
                tripod=true;
                let cross=|a:V3,b:V3|a.x.mul(b.z).sub(a.z.mul(b.x));
                let area=cross(supports[1].sub(supports[0]),supports[2].sub(supports[0]));
                assert!(area.abs()>eps,"degenerate executed tripod");
                for i in 0..3 {
                    let weight=cross(supports[i].sub(root.base),supports[(i+1)%3].sub(root.base));
                    assert!(weight.raw().signum()==area.raw().signum(),"executed root outside actual support triangle");
                }
            }
            last_landing=m.anchor_trace().unwrap().events().iter().rev().find(|e|matches!(e.reason,crate::gait::AnchorReason::LandingBlocked|crate::gait::AnchorReason::LandingRejected)).copied().or(last_landing);
            stagnant=if m.pos[0].sub(last_x).abs()<=Fx::from_ratio(1,1000) {stagnant+1}else{0};
            longest_wait=longest_wait.max(stagnant);last_x=m.pos[0];
            let feet_active=m.feet.iter().zip(&before).any(|(new,old)|
                new.planted()!=old.planted() || match (new.contact,old.contact) {
                    (Some(a),Some(b))=>a.pos.sub(b.pos).len_sq()>Fx::from_ratio(1,1000).mul(Fx::from_ratio(1,1000)),
                    (a,b)=>a!=b,
                });
            inactive=if stagnant>0 && !feet_active {inactive+1}else{0};
            assert!(inactive<40,"spider body and feet have stopped at {height}: tick={tick} pos={:?} drive={:?} up={:?} last_landing={last_landing:?}",m.pos,m.drive,m.hull.up);
            if m.destination.is_none() {arrival=Some(tick);break;}
        }
        println!("SPIDER_RAIL height={height} reverse={reverse} first={first:?} blocked={blocked} support_min={least} arrival={arrival:?} longest_root_wait={longest_wait} end={:?}",m.pos);
        assert!(first.is_some_and(|v|v<=2),"delayed response");
        assert!(m.destination.is_none(),"spider stalled at {height}: {:?}",m.pos);
        assert!(blocked<5,"unnecessary collision braking: {blocked}");
        assert!(least>=3,"recovery lost its real support triangle");
        if height==0 {assert!(least>=4,"flat gait lost its supporting cohort");}
    }}
    assert!(recovered && tripod,"course must exercise an actual guarded support handoff");
}
impl voxel::Generator for Ramp {
    fn material(&self,x:u32,y:u32,_:u32)->u16 {
        u16::from(y<(16+((x as i32-700).clamp(0,400)*3/4)) as u32)
    }
}
struct Stairs;
fn floor(x:i32)->i32 {
    let d=(x-700).clamp(0,300);
    16+if d<100 {d/20*3} else if d<200 {15} else {(300-d)/20*3}
}
impl voxel::Generator for Stairs {
    fn material(&self,x:u32,y:u32,_:u32)->u16 {u16::from((y as i32)<floor(x as i32))}
}

struct Cliff;
impl voxel::Generator for Cliff {
    fn material(&self,x:u32,y:u32,_:u32)->u16 {u16::from(y<16+if x>=800 {40} else {0})}
}
#[test]
fn even_multi_leg_walkers_refuse_a_four_metre_vertical_cliff() {
    let w=World::new(2048,512,2048,65536,Box::new(Cliff));
    for (kind,b) in models() {
        let m=Motion::new([500,16,1024],kind,&b);
        let mut p=nav::vehicle::Probe::new(&w);
        assert!((760..840).any(|x|p.pose([Fx::from_int(x),Fx::from_int(1024)],0,m.shape(&b)).refused()),
            "{kind:?} could treat a four metre vertical cliff as ordinary ground");
    }
}

struct ForestHill;
impl voxel::Generator for ForestHill {
    fn material(&self,x:u32,y:u32,z:u32)->u16 {
        let h=16+(x as i32-650).clamp(0,450)/4;
        if (y as i32)<h {return 1;}
        let dy=y as i32-h;
        let (rx,rz)=(x as i32%13-6,z as i32%13-6);
        if ((x/13)*9173^(z/13)*6151)%7==0 && dy<4 && rx*rx+rz*rz<9 {return 6;}
        let (tx,tz)=(x as i32%28-14,z as i32%28-14);
        if tx.abs()<=1 && tz.abs()<=1 && dy<20 {return 4;}
        if tx.abs()<=5 && tz.abs()<=5 && (8..24).contains(&dy) {return 5;}
        0
    }
}
#[test]
fn commander_reverses_through_forest_pebbles_and_a_gentle_hill() {
    let w=World::new(2048,512,2048,65536,Box::new(ForestHill));
    let b=voxel::unit::commander();
    for uphill in [false,true] {for order in [false,true] {
    let (start,end)=if uphill {(500,1200)} else {(1100,500)};
    let mut m=Motion::new([start,16+(start-650).clamp(0,450)/4,1024],RangeUnitKind::Commander,&b);
    m.drive.head=if uphill {angle::HALF} else {0};
    m.enabled=true;
    let rubble=nav::vehicle::Rubble::new();
    if order {assert!(m.move_to(&w,[end,1024],&b,&[],&rubble));} else {m.pilot=Some([-1,0]);}
    let mut first=None;let mut peak=Fx::ZERO;let mut blocked=0;
    let mut support_start=Vec::new();
    for tick in 0..700 {
        m.step(&w,&b,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&nav::vehicle::Changed::default());
        if tick<3 {
            let root=m.frame(&b);
            support_start.push((tick,m.pos,m.feet.iter().zip(&m.rigs).map(|(f,r)| {
                let c=f.contact.unwrap();let d=c.pos.sub(root.base);let p=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                (f.planted(),f.ready(),f.support.reason,f.support.supported_mask,r.solve_contact(p).ankle.sub(p).len_sq().sqrt())
            }).collect::<Vec<_>>()));
        }
        if m.pos[0]!=Fx::from_int(start) && first.is_none() {first=Some(tick);}
        peak=peak.max(m.drive.vel.abs());blocked+=usize::from(m.blocked);
    }
    println!("FOREST uphill={uphill} order={order} reverse first={first:?} peak={peak:?} speed={} blocked={blocked} pos={:?}",m.speed,m.pos);
    let mut probe=nav::vehicle::Probe::with_yield(&w,&m.yielding);
    for dx in [0,-1,-2] {
        let p=probe.pose([m.pos[0].add(Fx::from_int(dx)),m.pos[2]],m.drive.head,m.shape(&b));
        println!("FOREST_POSE dx={dx} pose={p:?} shape={:?}",m.shape(&b));
    }
    assert!(first.is_some_and(|t|t<=2),"initial support: {support_start:?}; final {:?}",m.feet.iter().map(|f|(f.planted(),f.ready(),f.support)).collect::<Vec<_>>());
    assert!((m.pos[0].sub(Fx::from_int(start))).abs()>Fx::from_int(300),"reverse movement stalled on forest hill");
    assert!(peak>=Fx::from_int(m.speed).mul(Fx::from_ratio(95,100)),"trees imposed a speed penalty on the walker");
    }}
}

#[test]
fn bipeds_refuse_a_grade_that_multi_leg_walkers_can_climb() {
    let w=World::new(2048,1024,2048,65536,Box::new(Ramp));
    for (kind,b) in models() {
        let m=Motion::new([500,16,1024],kind,&b);
        let mut p=nav::vehicle::Probe::new(&w);
        let pose=p.pose([Fx::from_int(800),Fx::from_int(1024)],0,m.shape(&b));
        let biped=matches!(kind,RangeUnitKind::Commander|RangeUnitKind::Walker04);
        println!("GRADE {kind:?} shape={:?} refused={} steep={} tilt={}",m.shape(&b),pose.refused(),pose.steep,pose.tilt);
        assert_eq!(pose.refused(),biped,"{kind:?}: 37-degree grade must distinguish leg layouts");
    }
}

#[test]
fn walkers_cross_small_stairs_both_ways_and_still_start_immediately() {
    let mut errors=Vec::new();
    let mut evidence=String::from("kind\treverse\tlast_tick\tmax_planted_ik_q32\tmax_stationary_ticks\n");
    let w=World::new(2048,512,2048,65536,Box::new(Stairs));
    let rubble=nav::vehicle::Rubble::new();
    for (kind,b) in models() {for reverse in [false,true] {
        let (start,end)=if reverse {(1200,550)} else {(550,1200)};
        let mut m=Motion::new([start,16,1024],kind,&b);
        m.drive.head=if reverse {angle::HALF} else {0};
        m.enabled=true;
        assert!(m.move_to(&w,[end,1024],&b,&[],&rubble));
        let mut first=None;let mut high=Fx::from_int(16);let mut error=Fx::ZERO;
        let mut last=0;let mut stationary=0;let mut longest=0;
        for tick in 0..1100 {
            let before=m.pos;
            m.step(&w,&b,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&nav::vehicle::Changed::default());
            last=tick;
            stationary=if m.destination.is_some() && m.pos[0]==before[0] && m.pos[2]==before[2] {stationary+1}else{0};
            longest=longest.max(stationary);
            if m.pos[0]!=Fx::from_int(start) && first.is_none() {first=Some(tick);}
            high=high.max(m.pos[1]);
            if matches!(kind,RangeUnitKind::Commander|RangeUnitKind::Walker04) && m.hull.grounded() {
                assert!(m.feet.iter().any(|f|f.planted()),"{kind:?} lost both supports on stairs at {tick}");
            }
            let root=m.frame(&b);
            for (rig,foot) in m.rigs.iter().zip(&m.feet) {
                if !foot.planted() {continue;}
                let Some(c)=foot.contact else {continue;};
                let d=c.pos.sub(root.base);
                let local=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                let solved=rig.solve_contact(local);
                let e=solved.ankle.sub(local).len_sq().sqrt();
                if e>error && e>Fx::from_int(1) {println!("STAIR_IK {kind:?} tick={tick} root={root:?} foot={foot:?}");}
                error=error.max(e);
            }
            if m.destination.is_none() {break;}
        }
        println!("STAIRS {kind:?} reverse={reverse} first={first:?} high={high:?} error={error:?} end={:?} goal={:?}",m.pos,m.destination);
        assert!(first.is_some_and(|t|t<=2),"{kind:?} input latency");
        assert!(m.destination.is_none(),"{kind:?} stuck on modest stairs");
        assert!(high>Fx::from_int(22),"{kind:?} did not climb");
        evidence.push_str(&format!("{kind:?}\t{reverse}\t{last}\t{}\t{longest}\n",error.raw()));
        assert!(longest<=80,"{kind:?} exceeded the real support-wait horizon: {longest}");
        if error>Fx::from_ratio(1,20) {
            errors.push(format!("{kind:?} reverse={reverse} loses planted support: {error:?}"));
        }
    }}
    if let Ok(path)=std::env::var("RTS_SUPPORT_CAPTURE_DIR") {
        let path=std::path::Path::new(&path);std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("stairs.tsv"),evidence).unwrap();
    }
    assert!(errors.is_empty(),"{errors:?}");
}

fn uneven_floor(x: i32) -> i32 {
    match x {
        620..=779 => 16 + ((x - 620) / 20).min(4) * 2,
        780..=819 => 24 - (x - 780) / 5,
        820..=899 => 16 - (x - 820) / 4,
        900..=979 => -4 + (x - 900) / 4,
        _ => 16,
    }
}
struct Uneven;
impl voxel::Generator for Uneven {
    fn material(&self, x: u32, y: u32, _: u32) -> u16 {
        u16::from((y as i32) < uneven_floor(x as i32) + 32)
    }
}

#[test]
fn walking_soles_remain_reachable_and_touch_uneven_ground_both_directions() {
    let w = World::new(2048, 512, 2048, 65536, Box::new(Uneven));
    let rubble = nav::vehicle::Rubble::new();
    for (kind, body) in models() { for reverse in [false, true] {
        let start = if reverse {1050} else {580};
        let end = if reverse {580} else {1050};
        let mut m = Motion::new([start, uneven_floor(start) + 32, 1024], kind, &body);
        m.enabled = true;
        m.drive.head = if reverse {angle::HALF} else {0};
        assert!(m.move_to(&w, [end, 1024], &body, &[], &rubble));
        let mut max_error = Fx::ZERO;
        let mut max_gap = Fx::ZERO;
        let mut min_gap = Fx::ZERO;
        let mut first = None;
        let mut stopped = None;
        let mut stop_low = Fx::MAX;
        let mut stop_high = Fx::MIN;
        for tick in 0..740 {
            m.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
                &[], tick, 98, &rubble, &nav::vehicle::Changed::default());
            if m.pos[0] != Fx::from_int(start) && first.is_none() {first = Some(tick);}
            let root = m.frame(&body);
            let mut best_gap = None;
            for (rig, foot) in m.rigs.iter().zip(&m.feet) {
                if !foot.planted() {continue;}
                let c = foot.contact.unwrap();
                let d = c.pos.sub(root.base);
                let local = V3::new(d.dot(root.x), d.dot(root.y), d.dot(root.z));
                let solved = rig.solve_contact(local).ankle;
                let error = solved.sub(local).len_sq().sqrt();
                max_error = max_error.max(error);
                let ankle = root.base.add(root.x.scale(solved.x)).add(root.y.scale(solved.y)).add(root.z.scale(solved.z));
                let axes = c.axes();
                let gap = rig.sole.probes().iter().map(|p| {
                    let at = ankle.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z));
                    at.y.sub(Fx::from_int(uneven_floor(at.x.to_int_trunc()) + 32))
                }).min().unwrap();
                best_gap = Some(best_gap.map_or(gap, |v: Fx| if gap.abs() < v.abs() {gap} else {v}));
                min_gap = min_gap.min(gap);
            }
            if m.hull.grounded() {
                if matches!(kind, RangeUnitKind::Commander | RangeUnitKind::Walker04) {
                    assert!(best_gap.is_some(), "{kind:?} both feet lifted on uneven ground");
                }
                if let Some(gap) = best_gap {max_gap = max_gap.max(gap.abs());}
            }
            if m.destination.is_none() {
                let since = *stopped.get_or_insert(tick);
                if tick - since >= 60 {
                    stop_low = stop_low.min(m.pos[1]);
                    stop_high = stop_high.max(m.pos[1]);
                }
                if tick - since >= 80 {break;}
            }
        }
        println!("UNEVEN {kind:?} reverse={reverse} first={first:?} error={max_error:?} support_gap={max_gap:?} penetration={min_gap:?} goal={:?}", m.destination);
        assert!(first.is_some_and(|t| t <= 2));
        assert!(m.destination.is_none(), "{kind:?} stalled on a shallow uneven route: {:?} {:?}",m.pos,m.feet);
        assert!(max_error <= Fx::from_ratio(1, 20), "{kind:?} unreachable stance {max_error:?}");
        assert!(max_gap <= Fx::from_ratio(1, 20), "{kind:?} floating support {max_gap:?}");
        assert!(min_gap >= Fx::from_ratio(-1, 20), "{kind:?} foot below terrain {min_gap:?}");
        assert!(m.feet.iter().all(|f| f.planted()), "{kind:?} did not plant after stopping: {:?}",m.feet);
        assert!(stop_high >= stop_low && stop_high.sub(stop_low) <= Fx::from_ratio(1, 2),
            "{kind:?} continued bouncing after stopping: low={stop_low:?} high={stop_high:?} feet={:?}",m.feet);
    }}
}

struct DeepFlat;
impl voxel::Generator for DeepFlat {
    fn material(&self,_:u32,y:u32,_:u32)->u16 {u16::from(y<48)}
}

#[test]
fn dirty_support_under_a_stopped_walker_is_released_then_movement_starts_immediately() {
    for (kind, body) in models() {
        let mut w = World::new(2048, 512, 2048, 65536, Box::new(DeepFlat));
        let rubble = nav::vehicle::Rubble::new();
        let mut m = Motion::new([1024, 48, 1024], kind, &body);
        m.enabled = true;
        for tick in 0..20 {
            m.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
                &[], tick, 98, &rubble, &nav::vehicle::Changed::default());
        }
        let original = m.feet[0].contact.unwrap();
        assert!(m.feet[0].planted());
        let axes = original.axes();
        let mut lo = [i32::MAX; 2]; let mut hi = [i32::MIN; 2];
        for p in m.rigs[0].sole.probes() {
            let at = original.pos.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z));
            for (a, v) in [at.x.to_int_trunc(), at.z.to_int_trunc()].into_iter().enumerate() {
                lo[a] = lo[a].min(v - 2); hi[a] = hi[a].max(v + 2);
            }
        }
        let mut changed = nav::vehicle::Changed::default();
        for z in lo[1]..=hi[1] { for x in lo[0]..=hi[0] {
            for y in 45..48 {w.queue_dig(x as u32, y, z as u32);}
            changed.column(x, z);
        }}
        // Walker04's occupied sole spans more than four chunks. One flush
        // intentionally defers edits; exhaust the fixture's actual excavation.
        for _ in 0..64 {if w.queued()==0 {break;} w.flush();}
        assert_eq!(w.queued(),0,"support excavation must finish before testing removal");
        m.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
            &[], 20, 98, &rubble, &changed);
        assert!(!m.feet[0].planted(), "{kind:?} kept a stale support after a dig: old={original:?} new={:?} evidence={:?}",m.feet[0].contact,m.feet[0].support);
        let start = m.pos[0];
        assert!(m.move_to(&w, [start.to_int_trunc() + 150, 1024], &body, &[], &rubble));
        let mut first = None;
        for tick in 21..241 {
            m.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
                &[], tick, 98, &rubble, &nav::vehicle::Changed::default());
            if m.pos[0] != start && first.is_none() {first = Some(tick - 21);}
            if m.destination.is_none() { break; }
        }
        assert!(first.is_some_and(|t| t <= 2), "{kind:?} waited for a foot before obeying move");
        assert!(m.destination.is_none(), "{kind:?} did not leave its local crater");
    }
}

#[test]
fn reclaimed_rubble_releases_parked_soles_and_the_recovery_restores() {
    for (kind, body) in models() {
        let w = World::new(2048, 512, 2048, 65536, Box::new(DeepFlat));
        let heap = nav::vehicle::Rubble::from_sorted((900..1150).flat_map(|x|
            (900..1150).map(move |z| ([x,z],50))).collect());
        let empty = nav::vehicle::Rubble::new();
        let mut m = Motion::new([1024,50,1024],kind,&body);
        m.enabled = true;
        for tick in 0..20 {
            m.step(&w,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&heap,&nav::vehicle::Changed::default());
        }
        assert!(m.feet.iter().all(|f| f.planted()));
        m.step(&w,&body,true,true,crate::mobility::Mobility::intact(true),&[],20,98,&empty,&nav::vehicle::Changed::everywhere());
        assert!(m.feet.iter().all(|f| !f.planted()),"{kind:?} stood on reclaimed rubble");
        let mut bytes=Blob::new();m.save(&mut bytes);
        let mut copy=Motion::new([1024,50,1024],kind,&body);
        assert!(copy.load(&mut Scan::new(&bytes.v)));
        for tick in 21..100 {
            for motion in [&mut m,&mut copy] {
                motion.step(&w,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&empty,&nav::vehicle::Changed::default());
            }
            let mut a=Blob::new();m.save(&mut a);let mut b=Blob::new();copy.save(&mut b);
            assert_eq!(a.v,b.v,"{kind:?} rubble recovery restore tick {tick}");
        }
        assert!(m.feet.iter().all(|f| f.planted()),"{kind:?} never found the exposed ground");
    }
}
#[test]
fn walkers_keep_moving_across_small_reclaim_and_a_new_crater() {
    for (kind,body) in models() {
        let mut w=World::new(2048,512,2048,65536,Box::new(DeepFlat));
        w.prepare_navigation([4,5]);
        let mut rubble=nav::vehicle::Rubble::from_sorted((700..1020).step_by(13).flat_map(|x|
            (900..1150).step_by(17).map(move |z|([x,z],50))).collect());
        let mut m=Motion::new([600,48,1024],kind,&body);
        m.enabled=true;
        assert!(m.move_to(&w,[1100,1024],&body,&[],&rubble));
        let mut started=None;let mut blocked=0;let mut edited=false;
        for tick in 0..800 {
            let mut changed=nav::vehicle::Changed::default();
            if tick==20 {
                // A 2 m deep bowl appears ahead through the real edit path.
                for z in 900..1150 {for x in 820..980 {
                    let depth=((x as i32-820).min(980-x as i32)/4).min(20).max(0);
                    for y in 48-depth..48 {w.queue_dig(x,y as u32,z);}
                    changed.column(x as i32,z as i32);
                }}
                w.flush();edited=true;
                rubble=nav::vehicle::Rubble::from_sorted((700..1020).step_by(13).flat_map(|x|
                    (900..1150).step_by(17).map(move |z| {
                        let depth=if (820..980).contains(&x) {(x-820).min(980-x)/4} else {0};
                        ([x,z],(50-depth.min(20)) as u16)
                    })).collect());
            }
            m.step(&w,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&changed);
            if m.pos[0]!=Fx::from_int(600) && started.is_none() {started=Some(tick);}
            blocked+=u32::from(m.blocked);
            if edited && m.destination.is_none() {break;}
        }
        println!("CRATER_RECLAIM {kind:?} detail={} first={started:?} blocked={blocked} goal={:?}",m.terrain_detail,m.destination);
        assert!(edited && started.is_some_and(|t|t<=2));
        assert!(m.destination.is_none(),"{kind:?} stalled in crater/reclaim field");
        assert!(blocked<20,"{kind:?} repeatedly refused harmless debris");
        assert!(m.terrain_detail>0,"stock walking body did not enable relative detail");
    }
}
