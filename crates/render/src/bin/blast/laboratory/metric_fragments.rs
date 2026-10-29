//! Bounded metric overlays on actual fragment geometry, without SIM writes.
use super::*;
use render::geom::MeshBuf;
use std::collections::BTreeMap;
use sim_core::burst::Fragment;

const MAX:usize=64;
const SCAN:usize=512;
const BUILDS:usize=4;
#[derive(Component)]
pub struct Overlay;
struct Slot {entity:Entity,mesh:Handle<Mesh>,index:usize,shape:(u64,u8,u16,u64,u16,Option<[u8;3]>),colour:[u8;3]}
#[derive(Default)]
pub struct Cache {generation:u64,cursor:usize,slots:BTreeMap<u64,Slot>,materials:BTreeMap<[u8;3],Handle<StandardMaterial>>}
fn shape(f:&Fragment)->(u64,u8,u16,u64,u16,Option<[u8;3]>) {(f.shape,f.side,f.mm,f.chunk.as_ref().map_or(0,|c|c.hash()),f.material,f.paint)}
fn colour(f:&Fragment,sim:&BlastRange,mode:u8)->[f32;4] {
    let def=sim.matter.economy.mats.get(f.material);
    match mode {
        3=>if matches!(f.material,11|13|14|16) {[0.1,0.45,0.95,1.0]} else if def.is_some_and(|d|d.loose) {[0.95,0.72,0.1,1.0]} else {[0.58,0.58,0.62,1.0]},
        4=>def.map_or(metrics::UNKNOWN,|d|metrics::scale(d.strength,500)),
        5=>def.map_or(metrics::UNKNOWN,|d|metrics::scale(d.density,1000)),
        6=>{let speed=Vec3::from([f.velocity.x,f.velocity.y,f.velocity.z].map(|v|v.raw() as f32/fx::ONE_RAW as f32)).length();metrics::scale((speed*200.0).min(u32::MAX as f32) as u32,500)},
        10=>{let id=u32::from(f.material);[0.18+(id*43%70) as f32/100.0,0.18+(id*67%70) as f32/100.0,0.18+(id*29%70) as f32/100.0,1.0]},
        _=>metrics::UNKNOWN,
    }
}
fn quantize(c:[f32;4])->[u8;3] {std::array::from_fn(|i|(c[i].clamp(0.0,1.0)*31.0).round() as u8)}
fn material(k:[u8;3],cache:&mut Cache,materials:&mut Assets<StandardMaterial>)->Handle<StandardMaterial> {
    cache.materials.entry(k).or_insert_with(||materials.add(StandardMaterial{base_color:Color::linear_rgb(k[0] as f32/31.0,k[1] as f32/31.0,k[2] as f32/31.0),unlit:true,cull_mode:None,..default()})).clone()
}
fn geometry(scene:&Scene,f:&Fragment)->MeshBuf {
    let mut out=MeshBuf::default();
    for source in fragments::picking::surface(scene,f) {
        let base=out.pos.len() as u32;
        out.col.extend(std::iter::repeat([1.0;4]).take(source.pos.len()));
        out.pos.extend(source.pos);out.nrm.extend(source.nrm);
        out.idx.extend(source.idx.into_iter().map(|v|v+base));
    }
    out
}
fn pose(f:&Fragment,scene:&Scene,visual:&moving::Visual)->Transform {
    let mut t=fragments::picking::pose(f);
    let alpha=if scene.realtime&&!scene.paused {(scene.acc*sim_core::TICKS_PER_SECOND as f32).clamp(0.0,1.0)} else {1.0};
    if !f.resting {t.translation=visual.fragments.get(&f.id).copied().unwrap_or(t.translation).lerp(t.translation,alpha);}
    // A sub-pixel depth separation keeps the overlay from z-fighting.
    t.scale*=1.001;
    t
}
#[allow(clippy::too_many_arguments)]
pub fn draw(mut commands:Commands,lab:Res<Laboratory>,scene:Res<Scene>,visual:Res<moving::Visual>,cameras:Query<&Transform,(With<GasCamera>,Without<Overlay>)>,
    mut cache:Local<Cache>,mut meshes:ResMut<Assets<Mesh>>,mut materials:ResMut<Assets<StandardMaterial>>,mut old:Query<&mut Transform,With<Overlay>>) {
    let enabled=lab.active()&&lab.tools.metric>0;
    if !enabled || cache.generation!=scene.generation {
        for (_,slot) in std::mem::take(&mut cache.slots) {commands.entity(slot.entity).despawn();meshes.remove(slot.mesh.id());}
        cache.generation=scene.generation;cache.cursor=0;
        if !enabled {return;}
    }
    let anchor=if lab.tools.live {lab.tools.live_target.map(|(at,_)|at).or_else(||cameras.iter().next().map(|t|(t.translation+*t.forward()*lab.tools.distance).floor().to_array().map(|v|v as i32))).unwrap_or([0;3])}
        else if lab.tools.section.enabled {lab.tools.section.point()}
        else {section::anchor(&lab,&scene)};
    let centre=Vec3::from(anchor.map(|v|v as f32));let radius=SIDE_RADIUS*f32::from(lab.tools.metric_scale.max(1));
    let fragments=&scene.sim.matter.debris.fragments;
    let stale:Vec<_>=cache.slots.iter().filter_map(|(&id,s)| {
        let f=fragments.get(s.index).filter(|f|f.id==id&&shape(f)==s.shape);
        f.filter(|f|(fragments::pos([f.position.x,f.position.y,f.position.z])-centre).abs().max_element()<=radius).is_none().then_some(id)
    }).collect();
    for id in stale {if let Some(slot)=cache.slots.remove(&id){commands.entity(slot.entity).despawn();meshes.remove(slot.mesh.id());}}
    let ids:Vec<_>=cache.slots.keys().copied().collect();
    for id in ids {
        let index=cache.slots[&id].index;let f=&fragments[index];
        let k=quantize(colour(f,&scene.sim,lab.tools.metric));
        let mat=material(k,&mut cache,&mut materials);
        let slot=cache.slots.get_mut(&id).unwrap();
        if let Ok(mut t)=old.get_mut(slot.entity) {*t=pose(f,&scene,&visual);}
        if slot.colour!=k {commands.entity(slot.entity).insert(MeshMaterial3d(mat));slot.colour=k;}
    }
    let n=fragments.len();if n==0 {cache.cursor=0;return;}
    let mut built=0;
    for _ in 0..SCAN.min(n) {
        if cache.slots.len()>=MAX || built>=BUILDS {break;}
        let i=cache.cursor%n;cache.cursor=(i+1)%n;let f=&fragments[i];
        if cache.slots.contains_key(&f.id)||(fragments::pos([f.position.x,f.position.y,f.position.z])-centre).abs().max_element()>radius {continue;}
        let b=geometry(&scene,f);if b.pos.is_empty(){continue;}
        let k=quantize(colour(f,&scene.sim,lab.tools.metric));let mat=material(k,&mut cache,&mut materials);
        let handle=meshes.add(mesh(&b));let entity=commands.spawn((Overlay,Mesh3d(handle.clone()),MeshMaterial3d(mat),pose(f,&scene,&visual))).id();
        if scene.out.is_some() && std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("metric-fragment") {println!("METRIC_FRAGMENT_OVERLAY id={} colour={:?}",f.id,k);}
        cache.slots.insert(f.id,Slot{entity,mesh:handle,index:i,shape:shape(f),colour:k});built+=1;
    }
}
const SIDE_RADIUS:f32=28.0;

#[cfg(test)]
mod tests {use super::*;
    #[test] fn fragment_overlay_queries_do_not_conflict() {
        use bevy::ecs::system::{IntoSystem,System};
        let mut world=bevy::prelude::World::new();
        let mut system=IntoSystem::into_system(draw);
        system.initialize(&mut world);
    }
    #[test] fn colour_quantization_preserves_order_and_bounds() {
        assert_eq!(quantize([0.0,0.5,1.0,1.0]),[0,16,31]);
        assert_eq!(quantize([-1.0,2.0,0.0,1.0]),[0,31,0]);
    }
    #[test] fn steel_fragment_uses_its_own_material_and_speed_without_fake_heat() {
        use sim_core::damage::V3;use fx::Fx;
        let cfg=modding::explosion::ExplosionConfig::from_ron(include_str!("../../../../../../data/explosion.ron")).unwrap();
        let mut layout=modding::range::RangeLayout::from_ron(include_str!("../../../../../../data/range-cover.ron")).unwrap();
        layout.blocks.clear();layout.units.clear();layout.shields.clear();layout.course=None;
        let sim=BlastRange::with_layout(cfg,layout).unwrap();
        let f=Fragment{id:7,position:V3::ZERO,velocity:V3::new(Fx::ONE,Fx::ZERO,Fx::ZERO),material:3,paint:None,mm:100,
            basis:[V3::new(Fx::ONE,Fx::ZERO,Fx::ZERO),V3::new(Fx::ZERO,Fx::ONE,Fx::ZERO),V3::new(Fx::ZERO,Fx::ZERO,Fx::ONE)],mass_g:1,count:1,shape:1,side:1,chunk:None,held:0,resting:false};
        let before=sim.state_hash();
        assert_eq!(colour(&f,&sim,1),metrics::UNKNOWN);
        assert_eq!(colour(&f,&sim,4),metrics::scale(500,500));
        assert_eq!(colour(&f,&sim,6),metrics::scale(200,500));
        assert_eq!(sim.state_hash(),before);
    }
}
