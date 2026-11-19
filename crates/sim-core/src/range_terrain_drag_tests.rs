use super::*;
use modding::range::RangeUnitKind::*;

struct Grove { trees:bool, rocks:bool }
impl voxel::Generator for Grove {
    fn material(&self,x:u32,y:u32,z:u32)->u16 {
        if y<16 {return 1;}
        if !(800..1400).contains(&x) {return 0;}
        let (rx,rz)=(x as i32%17-8,z as i32%17-8);
        if self.rocks && y<20 && rx*rx+rz*rz<9 {return 6;}
        let (tx,tz)=(x as i32%29-14,z as i32%29-14);
        if self.trees && tx.abs()<=1 && tz.abs()<=1 && y<36 {return 5;}
        if self.trees && tx.abs()<=5 && tz.abs()<=5 && (24..40).contains(&y) {return 4;}
        0
    }
    fn batches_columns(&self)->bool {true}
    fn uniform_box(&self,min:[u32;3],max:[u32;3])->Option<u16> {
        if max[1]<=16 {Some(1)} else if min[1]>=40 || min[1]>=16
            && (max[0]<=800 || min[0]>=1400 || !self.trees && !self.rocks) {Some(0)} else {None}
    }
    fn column(&self,x:u32,z:u32,y0:u32,out:&mut[u16]) {
        out.fill(0);let end=y0+out.len() as u32;
        let mut fill=|lo:u32,hi:u32,material:u16| {
            let a=lo.max(y0);let b=hi.min(end);
            if a<b {out[(a-y0)as usize..(b-y0)as usize].fill(material);}
        };
        fill(0,16,1);
        if !(800..1400).contains(&x) {return;}
        let (tx,tz)=(x as i32%29-14,z as i32%29-14);
        if self.trees && tx.abs()<=5 && tz.abs()<=5 {fill(24,40,4);}
        if self.trees && tx.abs()<=1 && tz.abs()<=1 {fill(16,36,5);}
        let (rx,rz)=(x as i32%17-8,z as i32%17-8);
        if self.rocks && rx*rx+rz*rz<9 {fill(16,20,6);}
    }

}
fn check_ground_kind(kind:modding::range::RangeUnitKind) {
    let mut failures=Vec::new();
        for (name,trees,rocks) in [("bare",false,false),("trees",true,false),("rocks",false,true),("mixed",true,true)] {
            let world=World::new(4096,512,2048,65536,Box::new(Grove{trees,rocks}));
            let unit=super::super::RangeUnit::of_kind(kind,[500,16,1024]);
            let body=unit.body;let mut m=unit.motion;m.enabled=true;m.pilot=Some([1,0]);
            let rubble=nav::vehicle::Rubble::new();let changed=nav::vehicle::Changed::default();
            let top=Fx::from_ratio(m.speed,TICKS);let minimum=top.mul(Fx::from_ratio(90,100));
            let mut least=top;let mut peak_drop=Fx::ZERO;let mut previous=Fx::ZERO;
            let mut samples=0;let mut blocked=0;let mut idle=0;let mut brake_seen=false;
            for tick in 0..2400 {
                let old=m.pos[0];
                let before=m.feet.clone();
                m.trace_anchors(true);m.clear_anchor_trace();
                m.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&changed);
                let root=m.frame(&body);
                for (i,((rig,foot),was)) in m.rigs.iter().zip(&m.feet).zip(&before).enumerate() {
                    if !was.planted() || foot.planted() {continue;}
                    let c=foot.contact.unwrap();let d=c.pos.sub(root.base);
                    let local=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                    let error=rig.solve_contact(local).ankle.sub(local).len_sq().sqrt();
                    if error>Fx::from_ratio(1,20) {println!("DEPART before={was:?} after={foot:?} root={root:?}");}
                    assert!(error<=Fx::from_ratio(1,20),
                        "{kind:?}/{name} unreachable departing leg {i} tick={tick} x={old:?} error={error:?}");
                }
                let dx=m.pos[0].sub(old);
                if !brake_seen && previous.sub(dx)>top.mul(Fx::from_ratio(1,100)) && old>Fx::from_int(700) {
                    brake_seen=true;println!("BRAKE {kind:?}/{name} tick={tick} x={old:?} previous={previous:?} dx={dx:?} trace={:?} feet={:?}",m.anchor_trace(),m.feet.iter().map(|f|(f.planted(),f.support)).collect::<Vec<_>>());
                }
                if (700..1900).contains(&old.to_int_trunc()) {
                    blocked+=usize::from(m.blocked);
                    peak_drop=peak_drop.max(previous.sub(dx));
                }
                if (1000..1200).contains(&old.to_int_trunc()) {least=least.min(dx);samples+=1;}
                idle=if dx<=Fx::ZERO {idle+1} else {0};
                previous=dx;
                if idle>=20 {println!("STALL {kind:?}/{name} at={:?} feet={:?}",m.pos,m.feet);break;}
                if m.pos[0]>=Fx::from_int(2000) {break;}
            }
            println!("TERRAIN {kind:?} {name} least_pct={:?} peak_drop_pct={:?} samples={samples} blocked={blocked} end={:?}",
                least.div(top).mul(Fx::from_int(100)),peak_drop.div(top).mul(Fx::from_int(100)),m.pos[0]);
            if samples==0 || least.add(Fx::from_ratio(1,1000))<minimum || blocked>0
                || peak_drop>top.mul(Fx::from_ratio(1,100)).add(Fx::from_ratio(1,1000))
                || m.pos[0]<Fx::from_int(1900) {
                failures.push(format!("{kind:?}/{name}: least={least:?} minimum={minimum:?} drop={peak_drop:?} blocked={blocked} samples={samples} end={:?}",m.pos[0]));
            }
        }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}

macro_rules! ground_case {
    ($name:ident,$kind:ident)=>{#[test]fn $name(){check_ground_kind($kind);}};
}
ground_case!(tank,Tank);
ground_case!(eidolon,EidolonTank);
ground_case!(artillery,Artillery);
ground_case!(lightning,LightningTank);
ground_case!(artillery_t2,ArtilleryT2);
ground_case!(crawler,Crawler);
ground_case!(commander,Commander);
ground_case!(walker,Walker04);
ground_case!(strider,Strider);
ground_case!(spider,Spider);
#[test]
fn optimized_grove_columns_match_the_exact_generator() {
    use voxel::Generator;
    for trees in [false,true] {for rocks in [false,true] {
        let grove=Grove{trees,rocks};
        for x in [790,800,808,813,826,899,1000,1399,1400] {for z in 1000..1035 {
            let mut column=[0;64];grove.column(x,z,0,&mut column);
            for (y,&material) in column.iter().enumerate() {assert_eq!(material,grove.material(x,y as u32,z));}
        }}
    }}
}

#[test]
fn actual_body_mass_weakens_the_shared_surface_loss_and_smoothing_never_overrides_brakes() {
    let mut masses=Vec::new();
    for kind in [Tank,EidolonTank,Artillery,LightningTank,ArtilleryT2,Crawler,Commander,Walker04,Strider,Spider] {
        let unit=super::super::RangeUnit::of_kind(kind,[500,16,512]);let mut m=unit.motion;
        let mass=u128::from(unit.body.volume_mm3())*m.chassis.density as u128/1_000_000_000;
        let cap=Fx::from_int(m.speed);m.drive.vel=cap;
        let first=m.surface_speed(cap,true,true);
        assert!(first>=cap.mul(Fx::from_ratio(90,100)) && first<=cap);
        assert!(cap.sub(first)<=cap.mul(Fx::from_ratio(1,200)).add(Fx::from_ratio(1,10000)));
        assert_eq!(m.surface_speed(Fx::ZERO,true,true),Fx::ZERO);
        let brake_target=cap.mul(Fx::from_ratio(1,4));
        assert!(m.surface_speed(brake_target,true,true)<=brake_target);
        masses.push((mass,m.surface_loss));
        println!("SURFACE_MASS {kind:?} kg={mass} maximum_loss_pct={:?}",m.surface_loss.mul(Fx::from_int(100)));
    }
    masses.sort_unstable_by_key(|(mass,_)|*mass);
    for pair in masses.windows(2) {
        if pair[0].0<pair[1].0 {assert!(pair[0].1>pair[1].1,"mass did not weaken resistance: {pair:?}");}
    }
}

