use super::*;
use modding::range::RangeUnitKind::*;

#[test]
fn tracks_share_the_same_pace_at_spawn_and_course_reload() {
    for (kind,speed) in [(Tank,120),(EidolonTank,120),(Artillery,120),
        (LightningTank,109),(ArtilleryT2,109),(Crawler,98)] {
        let mut u=super::super::RangeUnit::of_kind(kind,[512,16,512]);
        assert_eq!(u.motion.speed,speed,"{kind:?}");
        assert_eq!(u.motion.chassis.thrust,stock_packs().sim.tracked.thrust*9);
        assert_eq!(u.motion.chassis.brake,stock_packs().sim.tracked.brake*9);
        u.apply_course(&stock_packs().course);
        assert_eq!(u.motion.speed,speed,"course {kind:?}");
        let mut custom=stock_packs().course.clone(); custom.tank_speed=20;
        u.apply_course(&custom);
        assert_eq!(u.motion.speed,match kind {LightningTank|ArtilleryT2=>54,Crawler=>48,_=>60});
        assert!(u.motion.speed<speed);
        // Applying a course never compounds the installed acceleration.
        assert_eq!(u.motion.chassis.thrust,stock_packs().sim.tracked.thrust*9);
    }
    for (kind,speed) in [(Commander,25),(Strider,84),(Walker04,75),(Spider,24),(TurretT1,0)] {
        let mut u=super::super::RangeUnit::of_kind(kind,[512,16,512]);
        assert_eq!(u.motion.speed,speed,"{kind:?}");
        u.apply_course(&stock_packs().course);
        assert_eq!(u.motion.speed,speed,"course {kind:?}");
    }
}
fn drive_to_top(chassis:&modding::Chassis,factor:Fx,speed:i32)->(u32,Drive) {
    let mut d=Drive::default(); let limits=Limits::of(chassis,TICKS);
    for tick in 1..=2000 {
        d=drive::advance(d,0,Fx::from_int(speed),&limits,factor,Fx::from_ratio(1,TICKS)).0;
        if d.vel>=Fx::from_int(speed) {return (tick,d);}
    }
    panic!("never reached {speed}");
}
#[test]
fn new_top_speed_is_reached_in_about_a_third_of_the_old_time_with_real_body_mass() {
    for kind in [Tank,EidolonTank,Artillery,LightningTank,ArtilleryT2,Crawler] {
        let u=super::super::RangeUnit::of_kind(kind,[512,16,512]); let m=&u.motion;
        let old=stock_packs().sim.tracked;
        let (old_ticks,_)=drive_to_top(&old,m.drive_scale,stock_packs().course.tank_speed);
        let (ticks,mut d)=drive_to_top(&m.chassis,m.drive_scale,m.speed);
        println!("PACE {kind:?} speed={} old_ticks={old_ticks} new_ticks={ticks}",m.speed);
        assert!(ticks<=(old_ticks+2)/3+1,"{kind:?} acceleration to the new top speed");
        let limits=Limits::of(&m.chassis,TICKS); let mut travel=Fx::ZERO;
        for _ in 0..100 {
            let (next,step)=drive::advance(d,drive::NO_WAY,Fx::ZERO,&limits,m.drive_scale,Fx::from_ratio(1,TICKS));
            travel=travel.add(step.dx.abs());d=next;
            if d.vel==Fx::ZERO {break;}
        }
        assert_eq!(d.vel,Fx::ZERO,"{kind:?} brakes");
        assert!(travel<Fx::from_int(m.speed),"{kind:?} failed to stop within one second's travel");
    }
}
struct Wall;
impl voxel::Generator for Wall {
    fn material(&self,x:u32,y:u32,z:u32)->u16 {
        u16::from(y<16 || x==850 && y<100 && (400..625).contains(&z))
    }
}
#[test]
fn fast_tracks_still_sweep_a_one_voxel_wall_and_stop_on_a_move_goal() {
    let w=World::new(2048,256,2048,65536,Box::new(Wall));
    let rubble=nav::vehicle::Rubble::new();
    for kind in [Tank,EidolonTank,Artillery,LightningTank,ArtilleryT2,Crawler] {
        let u=super::super::RangeUnit::of_kind(kind,[500,16,512]);
        let mut m=u.motion; m.enabled=true;m.pilot=Some([1,0]);
        let shape=m.shape(&u.body);
        for tick in 0..180 {
            m.step(&w,&u.body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&nav::vehicle::Changed::default());
            assert!(m.pos[0]<Fx::from_int(850-shape.half[0]),"{kind:?} tunneled through the wall");
        }
        let mut m=Motion::new([500,16,1000],kind,&u.body);m.enabled=true;
        assert!(m.move_to(&w,[800,1000],&u.body,&[],&rubble));
        for tick in 0..500 {
            m.step(&w,&u.body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&nav::vehicle::Changed::default());
            if m.destination.is_none() && m.drive.vel==Fx::ZERO {break;}
        }
        assert!(m.destination.is_none(),"{kind:?} never arrived");
        assert!((m.pos[0].sub(Fx::from_int(800))).abs()<Fx::from_int(10),"{kind:?} overshot the goal");
    }
}

#[test]
fn fast_custom_courses_restore_at_top_speed_and_still_reject_excess_velocity() {
    struct Flat;
    impl voxel::Generator for Flat {fn material(&self,_:u32,y:u32,_:u32)->u16 {u16::from(y<16)}}
    let w=World::new(4096,256,2048,65536,Box::new(Flat));
    let mut c=stock_packs().course.clone();c.tank_speed=100;
    let rubble=nav::vehicle::Rubble::new();
    for kind in [Tank,EidolonTank,Artillery,LightningTank,ArtilleryT2,Crawler] {
        let mut u=super::super::RangeUnit::of_kind(kind,[500,16,1000]);u.apply_course(&c);
        u.motion.enabled=true;u.motion.pilot=Some([1,0]);
        for tick in 0..60 {u.motion.step(&w,&u.body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&Default::default());}
        assert!(u.motion.drive.vel>Fx::from_int(200),"{kind:?} did not exercise the old snapshot boundary");
        let mut saved=Blob::new();u.motion.save(&mut saved);
        let mut copy=super::super::RangeUnit::of_kind(kind,[500,16,1000]);copy.apply_course(&c);
        assert!(copy.motion.load(&mut Scan::new(&saved.v)),"{kind:?} rejected its legitimate fast snapshot");
        for tick in 60..90 {
            u.motion.pilot=Some([1,0]);copy.motion.pilot=Some([1,0]);
            for m in [&mut u.motion,&mut copy.motion] {m.step(&w,&u.body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&Default::default());}
            let mut a=Blob::new();let mut b=Blob::new();u.motion.save(&mut a);copy.motion.save(&mut b);assert_eq!(a.v,b.v);
        }
        u.motion.drive.vel=Fx::from_int(u.motion.speed).mul(Fx::from_ratio(u.motion.chassis.descend.max(100),100)).add(Fx::ONE);
        let mut bad=Blob::new();u.motion.save(&mut bad);
        assert!(!copy.motion.load(&mut Scan::new(&bad.v)));
    }
}
