use super::*;
struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _:u32,y:u32,_:u32)->u16 {u16::from(y<16)}
}
struct Barrier {height:u32}
impl voxel::Generator for Barrier {
    fn material(&self,x:u32,y:u32,_:u32)->u16 {
        u16::from(y<16 || (800..808).contains(&x) && y<16+self.height)
    }
}
#[test]
fn separate_feet_step_over_a_low_barrier_without_a_fictitious_track_belt() {
    let s=Shape {continuous: false, detail:2,half:[88,80],support:[50,60],clearance:Fx::from_int(38),
        step:28,min_up:Fx::from_ratio(70,100),height:80,span:10,reach:0,grade_slack:Fx::ZERO,ramp:Fx::ONE};
    let at=[Fx::from_int(804),Fx::from_int(1024)];
    let w=World::new(2048,256,2048,65536,Box::new(Barrier {height:28}));
    let mut p=Probe::new(&w);
    let pose=p.pose(at,0,s);
    assert!(!pose.refused(),"low barrier under an elevated torso: {pose:?}");
    assert_eq!(p.support_floors(at,0,s),[Fx::from_int(16);4],"the empty gap between legs acted like a track");
    let tall=World::new(2048,256,2048,65536,Box::new(Barrier {height:60}));
    assert!(Probe::new(&tall).pose(at,0,s).blocked>0,"a tall wall disappeared from the belly test");
}
fn shape(detail:i32)->Shape {
    Shape {continuous: false, detail,half:[24,24],support:[4,4],clearance:Fx::from_int(20),
        step:6,min_up:Fx::from_ratio(82,100),height:60,span:18,reach:0,grade_slack:Fx::ZERO,ramp:Fx::from_ratio(65,100)}
}
#[test]
fn small_debris_is_filtered_but_wide_heaps_and_walls_remain_real() {
    let w=World::new(256,128,256,65536,Box::new(Flat));
    let rubble=Rubble::from_sorted(vec![([102,102],18)]);
    let at=[Fx::from_int(102);2];
    let mut p=Probe::new(&w).with_rubble(&rubble);
    assert_eq!(p.surface(at),Fx::from_int(18));
    assert_eq!(p.terrain_surface(at,shape(2)),Fx::from_int(16));
    // Actual soles and the exact belly still see the same piece.
    assert_eq!(p.floor(102,102),18);
    let heap=Rubble::from_sorted((80..125).flat_map(|x|(80..125).map(move|z|([x,z],80))).collect());
    let mut p=Probe::new(&w).with_rubble(&heap);
    assert!(p.terrain_surface(at,shape(2))>Fx::from_int(70));
    assert!(p.pose([Fx::from_int(72),Fx::from_int(102)],0,shape(2)).refused());
    let wall=Rubble::from_sorted((75..130).map(|z|([95,z],100)).collect());
    let mut p=Probe::new(&w).with_rubble(&wall);
    assert!(p.pose([Fx::from_int(90),Fx::from_int(102)],0,shape(2)).refused(),
        "the exact belly lost a thin tall wall between coarse samples");
}
struct Crater;
impl voxel::Generator for Crater {
    fn material(&self,x:u32,y:u32,z:u32)->u16 {
        let r=(x as i32-128).abs().max((z as i32-128).abs());
        let h=16-(40-r).max(0)/4;
        u16::from((y as i32)<h)
    }
}
#[test]
fn a_wide_crater_retains_depth_and_coarse_queries_need_far_fewer_columns() {
    let w=World::new(256,128,256,65536,Box::new(Crater));
    let mut exact=Probe::new(&w);let mut coarse=Probe::new(&w);
    for i in 0..200 {
        let at=[Fx::from_ratio(10500+i,100),Fx::from_ratio(12800+i%19,100)];
        let a=exact.support_floors(at,0,shape(0));
        let b=coarse.support_floors(at,0,shape(2));
        for (x,y) in a.into_iter().zip(b) {assert!(x.sub(y).abs()<=Fx::from_int(2));}
    }
    println!("WALK_DETAIL columns exact={} coarse={}",exact.columns,coarse.columns);
    assert!(coarse.columns*2<exact.columns,"coarse grid increased terrain reads");
    assert!(coarse.terrain_surface([Fx::from_int(128);2],shape(2))<Fx::from_int(8));
}
#[test]
fn changed_ground_and_rubble_are_seen_after_reusing_raw_column_pages() {
    let mut w=World::new(256,128,256,65536,Box::new(Flat));
    let at=[Fx::from_int(102);2];
    let mut old=Probe::for_search(&w,&[],Columns::default());
    assert_eq!(old.terrain_surface(at,shape(2)),Fx::from_int(16));
    let columns=old.take_columns();drop(old);
    for z in 95..111 {for x in 95..111 {for y in 12..16 {w.queue_dig(x,y,z);}}}
    w.flush();
    let mut changed=Changed::default();changed.column(102,102);
    let mut fresh=Probe::for_local(&w,&[],columns,&changed);
    assert_eq!(fresh.terrain_surface(at,shape(2)),Fx::from_int(12));
    let rubble=Rubble::from_sorted((95..111).flat_map(|x|(95..111).map(move|z|([x,z],20))).collect());
    let columns=fresh.take_columns();drop(fresh);
    let mut fresh=Probe::for_local(&w,&[],columns,&changed).with_rubble(&rubble);
    assert_eq!(fresh.terrain_surface(at,shape(2)),Fx::from_int(20));
}

#[test]
fn cold_and_batched_walk_probes_report_cost_and_keep_the_same_answers() {
    let w=World::new(256,128,256,65536,Box::new(Crater));
    for cold in [false,true] {
        let mut totals=Vec::new();
        for detail in [0,2] {
            let mut p=Probe::for_search(&w,&[],Columns::default());
            let begin=std::time::Instant::now();let mut columns=0;
            for i in 0..500 {
                if cold && i>0 {
                    let carry=p.take_columns();
                    p=Probe::for_local(&w,&[],carry,&Changed::default());
                }
                p.support_floors([Fx::from_ratio(10500+i%200,100),Fx::from_int(128)],0,shape(detail));
                if cold {columns+=p.columns;}
            }
            if !cold {columns=p.columns;}
            let micros=begin.elapsed().as_micros();
            println!("WALK_COST cold={cold} detail={detail} queries=500 columns={columns} micros={micros}");
            totals.push(columns);
        }
        assert!(totals[1]<totals[0],"walking grid increased column reads cold={cold}");
    }
}

#[test]
fn a_pending_fine_route_restores_the_owners_derived_detail() {
    let w=World::new(256,128,256,65536,Box::new(Crater));
    let mut p=Probe::for_search(&w,&[],Columns::default());
    let cfg=Search {cell:4,min:[30,30],max:[226,226],margin:0,nodes:4096,
        crush_cost:0,crush_cap:0,heuristic_pct:100};
    let Hunting::Open(mut a)=Hunt::begin(&mut p,shape(2),[Fx::from_int(60),Fx::from_int(128)],0,[190,128],cfg)
        else {panic!("expected a pending route");};
    let mut blob=fx::blob::Blob::new();a.save(&mut blob);
    let mut b=Hunt::load_with_detail(&mut fx::blob::Scan::new(&blob.v),&[],2).unwrap();
    assert_eq!(b.shape.detail,2);
    let rubble=Rubble::default();
    for _ in 0..1000 {
        let x=a.advance_in(&w,&rubble,&|_,_|true,8);
        let y=b.advance_in(&w,&rubble,&|_,_|true,8);
        assert_eq!(x.is_some(),y.is_some());
        if let (Some(x),Some(y))=(x,y) {assert_eq!(x.path,y.path);assert!(!x.path.is_empty());return;}
        let mut x=fx::blob::Blob::new();a.save(&mut x);
        let mut y=fx::blob::Blob::new();b.save(&mut y);assert_eq!(x.v,y.v);
    }
    panic!("route never finished");
}

struct BeltRidge;
impl voxel::Generator for BeltRidge {
    fn material(&self,x:u32,y:u32,_:u32)->u16 {
        u16::from(y<16 || (792..816).contains(&x) && y<20)
    }
}
#[test]
fn tracked_surface_detail_keeps_belt_support_between_the_wheels() {
    let world=World::new(2048,128,2048,65536,Box::new(BeltRidge));
    let at=[Fx::from_int(804),Fx::from_int(1024)];
    let mut s=Shape {continuous:false,detail:5,half:[64,24],support:[50,12],clearance:Fx::from_ratio(9,2),
        step:5,min_up:Fx::from_ratio(88,100),height:40,span:4,reach:0,grade_slack:Fx::ZERO,ramp:Fx::from_ratio(70,100)};
    let mut probe=Probe::new(&world);
    assert_eq!(probe.support_floors(at,0,s),[Fx::from_int(16);4]);
    s.continuous=true;
    let floors=probe.support_floors(at,0,s);
    assert!(floors.iter().all(|h|*h>Fx::from_int(19)),"belt lost its ridge: {floors:?}");
}
