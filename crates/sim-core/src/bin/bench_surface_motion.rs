//! Real forest/pebble traversal, including felling, rubble and replay.
use fx::Fx;
use modding::range::{RangeLayout,RangeUnitDef,RangeUnitKind,RangeBlock};
use sim_core::blast::{BlastRange,battle::Order};
use std::{fs,io::{BufWriter,Write},time::Instant};
fn mm(v:Fx)->i64 {((i128::from(v.raw())*100)>>32) as i64}
fn main() {
    use RangeUnitKind::*;
    let args=std::env::args().collect::<Vec<_>>();
    let arg=|key:&str|args.windows(2).find(|a|a[0]==key).map(|a|a[1].as_str());
    let prefix=arg("--out").unwrap_or("out/surface598");
    let profile=args.iter().any(|a|a=="--profile");
    let cfg=modding::explosion::ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron")).unwrap();
    let mut csv=BufWriter::new(fs::File::create(format!("{prefix}.csv")).unwrap());
    writeln!(csv,"kind,mode,tick,x_mm,dx_mm,drive_mm_s,rubble").unwrap();
    let mut failures=Vec::new();
    for kind in [Tank,EidolonTank,Artillery,LightningTank,ArtilleryT2,Crawler,Commander,Walker04,Strider,Spider] {
        let name=format!("{kind:?}").to_lowercase();
        if arg("--kind").is_some_and(|s|s!=name) {continue;}
        for mode in ["bare","trees","rocks","mixed"] {
            if arg("--mode").is_some_and(|s|s!=mode) {continue;}
            let mut layout=RangeLayout::from_ron(include_str!("../../../../data/range-motion-yard.ron")).unwrap();
            layout.size=[2048,512,2048];layout.clear_max=[2048,2048];layout.blocks.clear();
            let course=layout.course.as_mut().unwrap();course.max=[2008,2008];course.ramps.clear();
            course.patrols.truncate(1);course.patrols[0].unit=0;course.patrols[0].points=vec![[1600,1024],[500,1024]];
            let mut roots=Vec::new();
            if mode=="trees" || mode=="mixed" {
                for x in (900..1300).step_by(32) {for z in (920..1130).step_by(24) {
                    layout.blocks.push(RangeBlock{min:[x,16,z],max:[x+2,36,z+2],material:5});
                    layout.blocks.push(RangeBlock{min:[x-3,28,z-3],max:[x+5,40,z+5],material:4});
                    roots.push([x as u32,16,z as u32]);
                }}
            }
            if mode=="rocks" || mode=="mixed" {
                for x in (880..1320).step_by(13) {for z in (920..1130).step_by(13) {
                    if ((x/13)*9173^(z/13)*6151)%7!=0 {continue;}
                    let height=2+(x+z)%3;
                    layout.blocks.push(RangeBlock{min:[x-2,16,z-2],max:[x+3,16+height,z+3],material:6});
                }}
            }
            layout.units=vec![RangeUnitDef{kind,at:[500,16,1024],allied:true,weapon:None}];
            let mut sim=BlastRange::with_layout(cfg,layout).unwrap();sim.set_navigation_workers(1);sim.set_physics_workers(1);
            sim.start_free_control().unwrap();
            assert!(sim.battle_order(0,Order::Pilot{throttle:1,turn:0,aim:[1800,32,1024],fire:false},None));
            if profile {fs::write(format!("{prefix}-{name}-{mode}-start.bin"),sim.snapshot()).unwrap();}
            let top=Fx::from_ratio(sim.units[0].motion.speed,sim_core::TICKS_PER_SECOND);
            let mut least=top;let mut peak_drop=Fx::ZERO;let mut previous=Fx::ZERO;
            let mut samples=0;let mut idle=0;let mut copy=None;let mut captured=false;let mut max_rubble=0;
            let mut scratch=fx::blob::Blob::new();let mut replay_scratch=fx::blob::Blob::new();
            let mut steps=Vec::new();let mut phases=std::collections::BTreeMap::<&str,u128>::new();
            let mut details=std::collections::BTreeMap::<&str,u128>::new();
            let (mut replay_us,mut hash_us,mut snapshot_us)=(0u128,0u128,0u128);
            let (mut max_fragments,mut max_front)=(0usize,0usize);
            let begin=Instant::now();
            for tick in 0..1800 {
                let old=sim.units[0].motion.pos[0];let step_start=Instant::now();
                if profile {
                    let mut phase_stamp=step_start;let mut detail_stamp=step_start;
                    sim.step_observed_detail(|name|{let now=Instant::now();*phases.entry(name).or_default()+=now.duration_since(phase_stamp).as_micros();phase_stamp=now;},
                        |name|{let now=Instant::now();*details.entry(name).or_default()+=now.duration_since(detail_stamp).as_micros();detail_stamp=now;}, |_, _| {});
                } else {sim.step();}
                steps.push(step_start.elapsed().as_micros());
                max_fragments=max_fragments.max(sim.matter.debris.fragments.len());max_front=max_front.max(sim.matter.felling_pending());
                if let Some(ref mut replay)=copy {
                    let replay:&mut BlastRange=replay;let started=Instant::now();replay.step();replay_us+=started.elapsed().as_micros();
                    let started=Instant::now();
                    let actual=sim.state_hash_with_scratch(&mut scratch);let restored=replay.state_hash_with_scratch(&mut replay_scratch);
                    hash_us+=started.elapsed().as_micros();
                    assert_eq!(actual,restored,"surface replay {name}/{mode}/{tick}");
                } else if mode=="mixed" && tick==50 {
                    let mut replay=BlastRange::restore(cfg,&sim.snapshot()).unwrap();replay.set_navigation_workers(2);replay.set_physics_workers(2);copy=Some(replay);
                }
                let motion=&sim.units[0].motion;let x=motion.pos[0];let dx=x.sub(old);
                max_rubble=max_rubble.max(sim.rubble.len());
                writeln!(csv,"{name},{mode},{tick},{},{},{},{}",mm(x),mm(dx),mm(motion.drive.vel),sim.rubble.len()).unwrap();
                if (700..1500).contains(&old.to_int_trunc()) {peak_drop=peak_drop.max(previous.sub(dx));}
                if (1000..1200).contains(&old.to_int_trunc()) {samples+=1;least=least.min(dx);}
                previous=dx;idle=if dx<=Fx::ZERO {idle+1} else {0};
                if !captured && x>=Fx::from_int(1100) {
                    let started=Instant::now();let bytes=sim.snapshot();snapshot_us+=started.elapsed().as_micros();
                    fs::write(format!("{prefix}-{name}-{mode}.bin"),bytes).unwrap();captured=true;
                }
                if idle>=40 || x>=Fx::from_int(1600) {break;}
            }
            let felled=roots.iter().filter(|&&[x,y,z]|sim.world.material(x,y,z)!=5).count();
            let end=sim.units[0].motion.pos[0];
            let ok=samples>0 && least.add(Fx::from_ratio(1,1000))>=top.mul(Fx::from_ratio(90,100))
                && peak_drop<=top.mul(Fx::from_ratio(1,100)).add(Fx::from_ratio(1,1000)) && end>=Fx::from_int(1600)
                && (roots.is_empty() || felled>0);
            println!("SURFACE {name}/{mode} least_pct={:?} peak_drop_pct={:?} end_mm={} samples={samples} felled={felled} max_rubble={max_rubble} ms={} ok={ok}",
                least.div(top).mul(Fx::from_int(100)),peak_drop.div(top).mul(Fx::from_int(100)),mm(end),begin.elapsed().as_millis());
            if !ok {failures.push(format!("{name}/{mode}"));}
            assert!(sim.matter.economy.ledger.balanced());
            if profile {
                steps.sort_unstable();let percentile=|p|steps[(steps.len()-1)*p/100];
                println!("SURFACE_PROFILE {name}/{mode} ticks={} step_us_p50={} p95={} p99={} max={} replay_us={replay_us} hash_us={hash_us} snapshot_us={snapshot_us} max_fragments={max_fragments} max_felling_pending={max_front}",
                    steps.len(),percentile(50),percentile(95),percentile(99),steps.last().unwrap());
                println!("SURFACE_PHASES {name}/{mode} {phases:?}");println!("SURFACE_DETAILS {name}/{mode} {details:?}");
            }
        }
    }
    csv.flush().unwrap();assert!(failures.is_empty(),"surface failures: {}",failures.join(", "));
}
