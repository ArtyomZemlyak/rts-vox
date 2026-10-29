//! World-space section plane; presentation only, never changes simulation cells.
use super::*;
use bevy::mesh::VertexAttributeValues;

#[derive(Component)]
pub struct ClippedUnitMesh {
    source: Handle<Mesh>,
    result: Handle<Mesh>,
    key: ([u32; 14],u8,u32),
    bounds: Option<bevy::camera::primitives::Aabb>,
    material: Option<Handle<StandardMaterial>>,
}

#[derive(Clone, Copy)]
struct Vertex {
    position: Vec3,
    normal: Vec3,
    color: Vec4,
    uv: Vec2,
    tangent: Vec4,
}
// Bevy's mesh allocator does not allocate a slab for an empty vertex buffer.
// A degenerate triangle is invisible but keeps the GPU allocation valid.
fn section_mesh(b:&MeshBuf)->Mesh {
    if !b.idx.is_empty() {let mut m=mesh(b);m.asset_usage|=RenderAssetUsages::MAIN_WORLD;return m;}
    let mut empty=MeshBuf::default();
    empty.pos=vec![[0.0;3];3];empty.nrm=vec![[0.0,1.0,0.0];3];empty.col=vec![[0.0;4];3];
    empty.uv=vec![[0.0;2];3];empty.tan=vec![[1.0,0.0,0.0,1.0];3];empty.idx=vec![0,1,2];
    let mut m=mesh(&empty);m.asset_usage|=RenderAssetUsages::MAIN_WORLD;m
}
impl Vertex {
    fn lerp(self, other:Self, t:f32)->Self {
        Self {position:self.position.lerp(other.position,t),normal:self.normal.lerp(other.normal,t).normalize_or_zero(),
            color:self.color.lerp(other.color,t),uv:self.uv.lerp(other.uv,t),tangent:self.tangent.lerp(other.tangent,t)}
    }
}
fn unit_cell_colour(sim:&BlastRange,unit:&sim_core::blast::RangeUnit,id:usize,p:[i32;3],mode:u8)->Option<[f32;4]> {
    let (part,cell)=unit.hit(p)?;
    Some(match mode {
        1=>{
            let heat=sim.thermals.heat(sim_core::blast::thermal::Cell{unit:id as u16,part:part as u8,at:cell.map(|v|v as u8)});
            let mm=if part==0 {unit.body.mm}else{unit.body.segments.get(part-1)?.body.mm};
            metrics::scale(sim_core::blast::thermal::temperature(heat,mm),1000)
        },
        3=>[0.58,0.58,0.62,1.0],
        6=>metrics::scale((unit.motion.drive.vel.raw().unsigned_abs().saturating_mul(10)/fx::ONE_RAW as u64).min(u64::from(u32::MAX)) as u32,500),
        7=>{let work=if part==0 {unit.dents.get(&cell.map(|v|v as u8)).copied().unwrap_or(0)}else{unit.seg_dent(part-1,cell)};metrics::scale(work,2000)},
        _=>metrics::UNKNOWN,
    })
}
// An entirely cold unit heat field is stable across ticks. Keep its cached
// mesh until heat appears; distinguish that transition even within one tick.
fn metric_epoch(mode:u8,tick:u32,hot:usize)->u32 {
    if mode==1 && hot==0 {0}
    else if matches!(mode,1|6|7) {tick.saturating_div(5).saturating_add(1)}
    else {0}
}
pub(super) fn clip_mesh(source:&Mesh, transform:&Transform, plane:Vec4, metric:Option<(&BlastRange,&sim_core::blast::RangeUnit,usize,u8)>)->Option<Mesh> {
    let VertexAttributeValues::Float32x3(positions)=source.attribute(Mesh::ATTRIBUTE_POSITION)? else {return None};
    let VertexAttributeValues::Float32x3(normals)=source.attribute(Mesh::ATTRIBUTE_NORMAL)? else {return None};
    let colors=match source.attribute(Mesh::ATTRIBUTE_COLOR) {Some(VertexAttributeValues::Float32x4(v))=>Some(v),_=>None};
    let uv=match source.attribute(Mesh::ATTRIBUTE_UV_0) {Some(VertexAttributeValues::Float32x2(v))=>Some(v),_=>None};
    let tangents=match source.attribute(Mesh::ATTRIBUTE_TANGENT) {Some(VertexAttributeValues::Float32x4(v))=>Some(v),_=>None};
    let indices:Vec<usize>=source.indices()?.iter().collect();
    let vtx=|i:usize|->Option<Vertex> {Some(Vertex {position:Vec3::from(*positions.get(i)?),normal:Vec3::from(*normals.get(i)?),
        color:colors.and_then(|v|v.get(i)).copied().map(Vec4::from).unwrap_or(Vec4::ONE),
        uv:uv.and_then(|v|v.get(i)).copied().map(Vec2::from).unwrap_or(Vec2::ZERO),
        tangent:tangents.and_then(|v|v.get(i)).copied().map(Vec4::from).unwrap_or(Vec4::X)})};
    let distance=|v:Vertex| plane.truncate().dot(transform.transform_point(v.position))+plane.w;
    let mut out=MeshBuf::default();
    let mut sampled=std::collections::BTreeMap::<[i32;3],[f32;4]>::new();
    for tri in indices.chunks_exact(3) {
        let Some(mut a)=vtx(tri[0]) else {return None};let Some(mut b)=vtx(tri[1]) else {return None};let Some(mut c)=vtx(tri[2]) else {return None};
        if let Some((sim,unit,id,mode))=metric {
            let centre=transform.transform_point((a.position+b.position+c.position)/3.0);
            let normal=(transform.rotation*(a.normal+b.normal+c.normal).normalize_or_zero()).normalize_or_zero();
            let cell=(centre-normal*0.2).floor().to_array().map(|x|x as i32);
            let colour=if mode==3 {[0.58,0.58,0.62,1.0]}
            else if mode==6 {metrics::scale((unit.motion.drive.vel.raw().unsigned_abs().saturating_mul(10)/fx::ONE_RAW as u64).min(u64::from(u32::MAX)) as u32,500)}
            else if !matches!(mode,1|7) {metrics::UNKNOWN}
            else if let Some(value)=sampled.get(&cell) {*value} else {
                let value=if sampled.len()>=8192 {metrics::UNKNOWN} else {
                    [0.2,0.5,1.0].into_iter().find_map(|offset| {
                        let p=(centre-normal*offset).floor().to_array().map(|x|x as i32);
                        unit_cell_colour(sim,unit,id,p,mode)
                    }).unwrap_or(metrics::UNKNOWN)
                };
                if sampled.len()<8192 {sampled.insert(cell,value);} value
            };
            a.color=Vec4::from(colour);b.color=a.color;c.color=a.color;
        }
        let input=[a,b,c];let mut clipped=[a;4];let mut count=0;
        for i in 0..3 {
            let a=input[i];let b=input[(i+1)%3];let da=distance(a);let db=distance(b);
            if da>=0.0 {clipped[count]=a;count+=1;}
            if (da>=0.0)!=(db>=0.0) {clipped[count]=a.lerp(b,(da/(da-db)).clamp(0.0,1.0));count+=1;}
        }
        if count<3 {continue;}
        let first=out.pos.len() as u32;
        for v in clipped.into_iter().take(count) {
            out.pos.push(v.position.to_array());out.nrm.push(v.normal.to_array());out.col.push(v.color.to_array());
            if uv.is_some() && tangents.is_some() {out.uv.push(v.uv.to_array());out.tan.push(v.tangent.to_array());}
        }
        let n=out.pos.len() as u32-first;
        for i in 1..n-1 {out.idx.extend([first,first+i,first+i+1]);}
    }
    Some(section_mesh(&out))
}

fn clip_key(t:&Transform,plane:Vec4)->[u32;14] {
    [t.translation.x,t.translation.y,t.translation.z,t.rotation.x,t.rotation.y,t.rotation.z,t.rotation.w,
     t.scale.x,t.scale.y,t.scale.z,plane.x,plane.y,plane.z,plane.w].map(f32::to_bits)
}
fn box_side(aabb:&bevy::camera::primitives::Aabb,t:&Transform,plane:Vec4)->i8 {
    let centre=Vec3::from(aabb.center);let half=Vec3::from(aabb.half_extents);
    let mut lo=f32::INFINITY;let mut hi=f32::NEG_INFINITY;
    for x in [-1.0,1.0] {for y in [-1.0,1.0] {for z in [-1.0,1.0] {
        let p=t.transform_point(centre+half*Vec3::new(x,y,z));
        let d=plane.truncate().dot(p)+plane.w;lo=lo.min(d);hi=hi.max(d);
    }}}
    if lo>=0.0 {1} else if hi<0.0 {-1} else {0}
}

fn burned_source(source:&mut Option<Handle<StandardMaterial>>,base:&[Handle<StandardMaterial>],burned:&[Handle<StandardMaterial>]) {
    if let Some(source)=source {
        if let Some(i)=base.iter().position(|m|m==source) {
            if let Some(replacement)=burned.get(i) {*source=replacement.clone();}
        }
    }
}

/// View-only unit meshes: section clipping and diagnostic colouring preserve
/// the source meshes and materials, and restore them when disabled.
pub fn clip_units(mut commands:Commands,lab:Res<Laboratory>,scene:Res<Scene>,mut meshes:ResMut<Assets<Mesh>>,
    mut materials:ResMut<Assets<StandardMaterial>>,mut metric_material:Local<Option<Handle<StandardMaterial>>>,
    art:Option<Res<WitnessArt>>,
    mut query:Query<(Entity,&Transform,&mut Mesh3d,Option<&mut MeshMaterial3d<StandardMaterial>>,
        Option<&bevy::camera::primitives::Aabb>,Option<&Witness>,Option<&Gun>,Option<&mut ClippedUnitMesh>),Or<(With<Witness>,With<Gun>)>>) {
    let plane=(lab.active() && lab.tools.section.enabled).then(||lab.tools.section.plane());
    let mode=if lab.active() {lab.tools.metric}else{0};
    let diagnostic=(mode>0).then(||metric_material.get_or_insert_with(||materials.add(StandardMaterial{base_color:Color::WHITE,unlit:true,..default()})).clone());
    for (entity,t,mut current,mut material,aabb,witness,gun,mut clipped) in &mut query {
        if let (Some(c),Some(w),Some(art))=(clipped.as_mut(),witness,art.as_ref()) {
            if scene.sim.units.get(w.0).is_some_and(|u|!u.status.alive) {
                burned_source(&mut c.material,&art.materials,&art.burned_materials);
            }
        }
        if plane.is_none() && mode==0 {
            if let Some(c)=clipped {
                current.0=c.source.clone();
                if let (Some(m),Some(source))=(material.as_mut(),c.material.as_ref()) {m.0=source.clone();}
                meshes.remove(c.result.id());commands.entity(entity).remove::<ClippedUnitMesh>();
            }
            continue;
        }
        let plane_value=plane.unwrap_or(Vec4::ZERO);
        let bounds=match clipped.as_ref() {Some(c)=>c.bounds.as_ref(),None=>aabb};
        let side=plane.map_or(1,|p|bounds.map_or(0,|a|box_side(a,t,p)));
        if side==1 && mode==0 {
            if let Some(c)=clipped {
                current.0=c.source.clone();
                if let (Some(m),Some(source))=(material.as_mut(),c.material.as_ref()) {m.0=source.clone();}
                meshes.remove(c.result.id());commands.entity(entity).remove::<ClippedUnitMesh>();
            }
            continue;
        }
        let unit=witness.and_then(|w|scene.sim.units.get(w.0).map(|u|(u,w.0)))
            .or_else(||gun.and_then(|_|(!scene.sim.point_gun()).then_some((&scene.sim.gun,scene.sim.units.len()))));
        let metric=unit.filter(|_|mode>0).map(|(u,id)|(&scene.sim,u,id,mode));
        let key=(if plane.is_some(){clip_key(t,plane_value)}else{[0;14]},mode,metric_epoch(mode,scene.sim.tick,scene.sim.thermals.hot()));
        if let Some(mut c)=clipped {
            if let Some(m)=material.as_mut() {
                let wanted=diagnostic.as_ref().or(c.material.as_ref());
                if let Some(wanted)=wanted {if m.0!=*wanted {m.0=wanted.clone();}}
            }
            if c.key==key {continue;}
            if let Some(result)=if side<0 {Some(section_mesh(&MeshBuf::default()))} else {meshes.get(&c.source).and_then(|m|clip_mesh(m,t,plane_value,metric))} {
                let _=meshes.insert(c.result.id(),result);c.key=key;
            }
        } else if let Some(result)=if side<0 {Some(section_mesh(&MeshBuf::default()))} else {meshes.get(&current.0).and_then(|m|clip_mesh(m,t,plane_value,metric))} {
            let source=current.0.clone();let result=meshes.add(result);current.0=result.clone();
            let original=material.as_ref().map(|m|m.0.clone());
            if let (Some(m),Some(diagnostic))=(material.as_mut(),diagnostic.as_ref()) {m.0=diagnostic.clone();}
            commands.entity(entity).insert(ClippedUnitMesh{source,result,key,bounds:aabb.cloned(),material:original});
        }
    }
}

#[derive(Component)]
pub struct UnitSectionCap;
#[derive(Default)]
pub struct CapCache {key:Option<(u64,u32,[i32;3],u8,bool,u8)>,mesh:Option<Handle<Mesh>>,entity:Option<Entity>,material:Option<Handle<StandardMaterial>>}

fn unit_cap_colour(unit:&sim_core::blast::RangeUnit,part:usize,local:[usize;3])->[f32;4] {
    use voxel::unit::Part;
    let body=if part==0 {Some(&unit.body)}else{unit.body.segments.get(part-1).map(|s|&s.body)};
    let role=body.map(|b|b.part_at(local[0],local[1],local[2])).unwrap_or(Part::Armour);
    let c=match role {
        Part::Armour=>[0.27,0.31,0.33],Part::Core=>[0.53,0.26,0.14],
        Part::Engine=>[0.38,0.27,0.16],Part::Weapon=>[0.27,0.35,0.40],Part::Sensor=>[0.25,0.39,0.31],
    };
    [c[0],c[1],c[2],1.0]
}

/// Sample actual occupied unit voxels on the fixed plane. This fills the cut
/// without modifying the unit or converting it into world terrain.
pub fn unit_caps(mut commands:Commands,lab:Res<Laboratory>,scene:Res<Scene>,mut meshes:ResMut<Assets<Mesh>>,
    mut materials:ResMut<Assets<StandardMaterial>>,mut cache:Local<CapCache>,mut visibility:Query<&mut Visibility,With<UnitSectionCap>>) {
    if !lab.active() || !lab.tools.section.enabled {
        if let Some(e)=cache.entity {if let Ok(mut v)=visibility.get_mut(e){*v=Visibility::Hidden;}}
        cache.key=None;return;
    }
    let s=&lab.tools.section;let key=(scene.generation,scene.sim.tick,s.point(),s.axis,s.positive,lab.tools.metric);
    if cache.key==Some(key) {return;}
    let axis=s.axis.min(2) as usize;let u=(axis+1)%3;let v=(axis+2)%3;let point=s.point();
    let mut out=MeshBuf::default();let mut budget=32768usize;
    for (id,unit) in scene.sim.units.iter().chain((!scene.sim.point_gun()).then_some(&scene.sim.gun)).enumerate() {
        let (lo,hi)=unit.bounds();
        let sample_axis=point[axis]-i32::from(!s.positive);
        if sample_axis<lo[axis] || sample_axis>hi[axis] {continue;}
        let area=(hi[u]-lo[u]+1).max(0) as usize*(hi[v]-lo[v]+1).max(0) as usize;
        if area>budget {continue;} budget-=area;
        for j in lo[v]..=hi[v] {for i in lo[u]..=hi[u] {
            let mut cell=[0;3];cell[axis]=sample_axis;cell[u]=i;cell[v]=j;
            let Some((part,local))=unit.hit(cell) else {continue;};
            let color=if lab.tools.metric>0 {unit_cell_colour(&scene.sim,unit,id,cell,lab.tools.metric).unwrap_or(metrics::UNKNOWN)}
                else {unit_cap_colour(unit,part,local)};
            let mut base=Vec3::from(cell.map(|x|x as f32));base[axis]=point[axis] as f32;
            let mut du=Vec3::ZERO;du[u]=1.0;let mut dv=Vec3::ZERO;dv[v]=1.0;
            let normal=-s.plane().truncate();base+=normal*0.01;
            let first=out.pos.len() as u32;
            for p in [base,base+du,base+du+dv,base+dv] {out.pos.push(p.to_array());out.nrm.push(normal.to_array());out.col.push(color);}
            out.idx.extend([first,first+1,first+2,first,first+2,first+3]);
        }}
    }
    let material=cache.material.get_or_insert_with(||materials.add(StandardMaterial{base_color:Color::WHITE,unlit:true,cull_mode:None,..default()})).clone();
    if let Some(h)=&cache.mesh {let _=meshes.insert(h.id(),section_mesh(&out));}
    else {let h=meshes.add(section_mesh(&out));let e=commands.spawn((UnitSectionCap,Mesh3d(h.clone()),MeshMaterial3d(material),Transform::default())).id();cache.mesh=Some(h);cache.entity=Some(e);}
    if let Some(e)=cache.entity {if let Ok(mut vis)=visibility.get_mut(e){*vis=Visibility::Visible;}}
    cache.key=Some(key);
}

pub struct State {
    pub enabled: bool,
    pub anchor: [i32; 3],
    pub offset: i32,
    pub axis: u8,
    pub positive: bool,
}

impl Default for State {
    fn default() -> Self {
        Self { enabled: false, anchor: [0; 3], offset: 0, axis: 0, positive: true }
    }
}

impl State {
    pub fn set_axis(&mut self, axis: u8) {
        self.anchor = self.point();
        self.offset = 0;
        self.axis = axis.min(2);
    }
    pub fn point(&self) -> [i32; 3] {
        let mut p = self.anchor;
        p[self.axis.min(2) as usize] = p[self.axis.min(2) as usize].saturating_add(self.offset);
        p
    }
    pub fn plane(&self) -> Vec4 {
        let a = self.axis.min(2) as usize;
        let sign = if self.positive { 1.0 } else { -1.0 };
        let mut n = Vec3::ZERO;
        n[a] = sign;
        n.extend(-sign * self.point()[a] as f32)
    }
}

pub(super) fn anchor(lab: &Laboratory, scene: &Scene) -> [i32; 3] {
    let selected_item=lab.constructor.selected.and_then(|i|lab.constructor.placed.get(i).map(|p|(i,p)));
    let selected_object=selected_item.map(|(i,p)| {
        if !p.element.units.is_empty() {
            let base=lab.constructor.base_layout(&lab.definition,lab.recipe).ok().map_or(0,|l|l.units.len());
            let index=base+lab.constructor.placed[..i].iter().map(|p|p.element.units.len()).sum::<usize>();
            if let Some(unit)=scene.sim.units.get(index) {
                let (lo,hi)=unit.bounds();
                return std::array::from_fn(|a|lo[a]+(hi[a]-lo[a])/2);
            }
        }
        p.at
    });
    let selected=if lab.ui.tab==14 {lab.tools.selected.or(selected_object)}else{selected_object.or(lab.tools.selected)};
    selected
        .or_else(|| scene.sim.layout.as_ref().map(|layout| layout.target))
        .unwrap_or_else(|| {
            let size = scene.sim.size();
            [
                size[0] as i32 / 2,
                scene.sim.ground_level(),
                size[2] as i32 / 2,
            ]
        })
}

pub fn toggle(lab: &mut Laboratory, scene: &Scene) {
    lab.tools.section.enabled = !lab.tools.section.enabled;
    if lab.tools.section.enabled && lab.tools.section.anchor == [0; 3] {
        focus(lab, scene);
    }
}

pub fn focus(lab: &mut Laboratory, scene: &Scene) {
    let point = anchor(lab, scene);
    lab.tools.section.anchor = point;
    lab.tools.section.offset = 0;
}

pub fn shift(lab: &mut Laboratory, delta: i32) {
    lab.tools.section.offset = lab
        .tools
        .section
        .offset
        .saturating_add(delta)
        .clamp(-100_000, 100_000);
}

/// Upload the fixed plane for both orthographic and live perspective views.
pub fn camera(
    lab: Option<Res<Laboratory>>,
    mut cameras: Query<(&Transform, &mut Projection), With<GasCamera>>,
    mut gas: ResMut<GasInput>,
    mut gizmos: Gizmos,
) {
    for (_, mut projection) in &mut cameras {
        if let Projection::Orthographic(p) = &mut *projection {
            p.near = 1.0;
        }
    }
    gas.params.section = Vec4::ZERO;
    if let Some(lab) = lab.as_ref().filter(|l| l.active() && l.tools.section.enabled) {
        let s = &lab.tools.section;
        gas.params.section = s.plane();
        let centre = Vec3::from(s.point().map(|v| v as f32));
        let a = s.axis.min(2) as usize;
        let mut u = Vec3::ZERO; u[(a + 1) % 3] = 1.0;
        let mut v = Vec3::ZERO; v[(a + 2) % 3] = 1.0;
        let colour = Color::srgba(1.0, 0.72, 0.12, 0.8);
        for i in -4..=4 {
            let d = i as f32 * 10.0;
            gizmos.line(centre + u*d - v*40.0, centre + u*d + v*40.0, colour);
            gizmos.line(centre + v*d - u*40.0, centre + v*d + u*40.0, colour);
        }
        gizmos.arrow(centre, centre + s.plane().truncate()*12.0, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_section_uses_one_colour_for_each_material_role_without_checkerboard() {
        let unit=sim_core::blast::RangeUnit::new([64,20,64]);
        let b=&unit.body;let mut samples=std::collections::BTreeMap::new();
        for x in 0..usize::from(b.vol.nx) {for y in 0..usize::from(b.vol.ny) {for z in 0..usize::from(b.vol.nz) {
            if !b.vol.get(x,y,z) {continue;}
            let role=b.part_at(x,y,z) as u8;let colour=unit_cap_colour(&unit,0,[x,y,z]);
            if let Some(previous)=samples.insert(role,colour) {assert_eq!(previous,colour);}
        }}}
        assert!(!samples.is_empty());
    }
    #[test]
    fn focus_prefers_selected_unit_centre_and_preserves_explicit_probe() {
        let mut layout=Definition::stock().layout;
        layout.size=[128,128,128];layout.clear_min=[1,1];layout.clear_max=[127,127];
        layout.gun=[40,24,64];layout.gun_point=true;layout.target=[64,16,64];layout.units.clear();
        let mut sim=BlastRange::with_layout(config(false,false),layout.clone()).unwrap();
        sim.units.push(sim_core::blast::RangeUnit::new([64,20,64]));
        let (lo,hi)=sim.units[0].bounds();let centre=std::array::from_fn(|a|lo[a]+(hi[a]-lo[a])/2);
        let app=super::super::integration_tests::app_with_base(panel::hud::tests::app_with_scene(sim,Workshop::new(Some(layout.clone()),false)));
        let scene=app.world().resource::<Scene>();
        let mut lab=Laboratory::new(true);lab.constructor.base=Some(layout);lab.constructor.stand=false;
        let tank=lab.constructor.catalog.iter().find(|e|e.id=="tank").unwrap().clone();
        lab.constructor.placed.push(modding::laboratory::catalog::Placed{element:tank,at:[64,20,64],turns:0});
        lab.constructor.selected=Some(0);lab.tools.selected=Some([10,10,10]);lab.ui.tab=0;
        assert_eq!(anchor(&lab,scene),centre);
        lab.ui.tab=14;assert_eq!(anchor(&lab,scene),[10,10,10]);
    }
    #[test]
    fn section_mesh_retains_cpu_data_for_later_plane_moves() {
        let mut b=MeshBuf::default();b.pos=vec![[0.0;3],[1.0,0.0,0.0],[0.0,1.0,0.0]];
        b.nrm=vec![[0.0,0.0,1.0];3];b.col=vec![[1.0;4];3];b.idx=vec![0,1,2];
        assert!(section_mesh(&b).asset_usage.contains(RenderAssetUsages::MAIN_WORLD));
        assert!(section_mesh(&MeshBuf::default()).asset_usage.contains(RenderAssetUsages::MAIN_WORLD));
    }
    #[test]
    fn destroyed_unit_restores_the_burned_material() {
        let mut assets=Assets::<StandardMaterial>::default();
        let original=assets.add(StandardMaterial::default());
        let burned=assets.add(StandardMaterial::default());
        let mut source=Some(original.clone());
        burned_source(&mut source,std::slice::from_ref(&original),std::slice::from_ref(&burned));
        assert_eq!(source,Some(burned.clone()));
        burned_source(&mut source,std::slice::from_ref(&original),std::slice::from_ref(&burned));
        assert_eq!(source,Some(burned));
    }
    #[test]
    fn cold_heat_mesh_stays_cached_and_heat_transitions_invalidate_it() {
        assert_eq!(metric_epoch(1, 10, 0), metric_epoch(1, 1000, 0));
        assert_ne!(metric_epoch(1, 10, 0), metric_epoch(1, 10, 1));
        assert_ne!(metric_epoch(1, 10, 1), metric_epoch(1, 10, 0));
        assert_eq!(metric_epoch(1, 10, 1), metric_epoch(1, 14, 1));
        assert_ne!(metric_epoch(1, 14, 1), metric_epoch(1, 15, 1));
    }

    #[test]
    fn unit_mesh_is_cut_in_world_space_and_preserves_attributes() {
        let mut b=MeshBuf::default();
        b.pos=vec![[-1.0,0.0,0.0],[1.0,0.0,0.0],[1.0,1.0,0.0]];
        b.nrm=vec![[0.0,0.0,1.0];3];b.col=vec![[0.3,0.4,0.5,1.0];3];
        b.uv=vec![[0.0,0.0],[1.0,0.0],[1.0,1.0]];b.tan=vec![[1.0,0.0,0.0,1.0];3];b.idx=vec![0,1,2];
        let source=mesh(&b);
        for positive in [true,false] {
            let plane=if positive {Vec4::new(1.0,0.0,0.0,-3.0)} else {Vec4::new(-1.0,0.0,0.0,3.0)};
            let t=Transform::from_xyz(3.0,0.0,0.0);
            let out=clip_mesh(&source,&t,plane,None).unwrap();
            let VertexAttributeValues::Float32x3(pos)=out.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else {panic!("positions")};
            assert!(!pos.is_empty());
            assert!(pos.iter().all(|p|plane.truncate().dot(t.transform_point(Vec3::from(*p)))+plane.w>=-0.0001));
            assert_eq!(out.attribute(Mesh::ATTRIBUTE_UV_0).unwrap().len(),pos.len());
            assert_eq!(out.attribute(Mesh::ATTRIBUTE_TANGENT).unwrap().len(),pos.len());
        }
        assert_eq!(source.count_vertices(),3);
    }

    #[test]
    fn unit_metric_reads_unit_cells_and_marks_unavailable_values_unknown() {
        let mut layout=Definition::stock().layout;
        layout.size=[128,128,128];layout.clear_min=[1,1];layout.clear_max=[127,127];
        layout.gun=[40,24,64];layout.gun_point=true;layout.target=[64,16,64];
        let mut sim=BlastRange::with_layout(config(false,false),layout).unwrap();
        let unit=sim_core::blast::RangeUnit::new([64,20,64]);
        let [nx,ny,nz]=[unit.body.vol.nx,unit.body.vol.ny,unit.body.vol.nz].map(usize::from);
        let p=(0..nx).flat_map(|x|(0..ny).flat_map(move|y|(0..nz).map(move|z|[x,y,z])))
            .find_map(|cell|unit.body.vol.get(cell[0],cell[1],cell[2]).then(||unit.world_pos(cell)).filter(|p|unit.hit(*p).is_some())).expect("occupied unit cell");
        assert_eq!(unit_cell_colour(&sim,&unit,0,p,1),Some(metrics::scale(0,1000)));
        let (part,cell)=unit.hit(p).unwrap();
        sim.thermals.deposit(sim_core::blast::thermal::Cell{unit:0,part:part as u8,at:cell.map(|v|v as u8)},1000,&sim.thermal_tuning);
        assert_eq!(unit_cell_colour(&sim,&unit,0,p,1),Some(metrics::scale(1000,1000)));
        assert_eq!(unit_cell_colour(&sim,&unit,0,p,4),Some(metrics::UNKNOWN));
        assert_eq!(unit_cell_colour(&sim,&unit,0,p,3),Some([0.58,0.58,0.62,1.0]));
    }

    #[test]
    fn unit_heat_colour_uses_cell_volume_not_raw_energy() {
        let mut layout=Definition::stock().layout;
        layout.size=[128,128,128];layout.clear_min=[1,1];layout.clear_max=[127,127];
        layout.gun=[40,24,64];layout.gun_point=true;layout.target=[64,16,64];
        let mut sim=BlastRange::with_layout(config(false,false),layout).unwrap();
        let unit=sim_core::blast::RangeUnit::of_kind(RangeUnitKind::Spider,[64,20,64]);
        assert!(unit.body.mm>voxel::unit::WORLD_MM);
        let [nx,ny,nz]=[unit.body.vol.nx,unit.body.vol.ny,unit.body.vol.nz].map(usize::from);
        let p=(0..nx).flat_map(|x|(0..ny).flat_map(move|y|(0..nz).map(move|z|[x,y,z])))
            .find_map(|cell|unit.body.vol.get(cell[0],cell[1],cell[2]).then(||unit.world_pos(cell)).filter(|p|unit.hit(*p).is_some_and(|(part,_)|part==0))).expect("occupied spider cell");
        let (part,cell)=unit.hit(p).unwrap();
        sim.thermals.deposit(sim_core::blast::thermal::Cell{unit:0,part:part as u8,at:cell.map(|v|v as u8)},1000,&sim.thermal_tuning);
        let expected=sim_core::blast::thermal::temperature(1000,unit.body.mm);
        assert_eq!(unit_cell_colour(&sim,&unit,0,p,1),Some(metrics::scale(expected,1000)));
        assert_ne!(unit_cell_colour(&sim,&unit,0,p,1),Some(metrics::scale(1000,1000)));
    }

    #[test]
    fn section_switch_restores_the_original_unit_mesh_without_simulation_edits() {
        let mut layout=Definition::stock().layout;
        layout.size=[128,128,128];layout.clear_min=[1,1];layout.clear_max=[127,127];
        layout.gun=[40,24,64];layout.gun_point=true;layout.target=[64,16,64];
        let sim=BlastRange::with_layout(config(false,false),layout.clone()).unwrap();
        let mut app=super::super::integration_tests::app_with_base(panel::hud::tests::app_with_scene(sim,Workshop::new(Some(layout.clone()),false)));
        app.world_mut().resource_mut::<Laboratory>().definition.layout=layout;
        app.init_resource::<Assets<Mesh>>().init_resource::<Assets<StandardMaterial>>().add_systems(Update,clip_units.after(super::super::actions));
        super::super::integration_tests::enter(&mut app);
        let before=app.world().resource::<Scene>().sim.state_hash();
        let mut b=MeshBuf::default();b.pos=vec![[-1.0,0.0,0.0],[1.0,0.0,0.0],[1.0,1.0,0.0]];
        b.nrm=vec![[0.0,0.0,1.0];3];b.col=vec![[1.0;4];3];b.idx=vec![0,1,2];
        let source=app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh(&b));
        let original_material=app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial::default());
        let entity=app.world_mut().spawn((Mesh3d(source.clone()),MeshMaterial3d(original_material.clone()),Transform::default(),Witness(0,Transform::default(),None,false))).id();
        {let mut lab=app.world_mut().resource_mut::<Laboratory>();lab.tools.section.enabled=true;lab.tools.section.anchor=[0,0,0];}
        app.update();
        assert_ne!(app.world().get::<Mesh3d>(entity).unwrap().0,source);
        app.world_mut().resource_mut::<Laboratory>().tools.section.positive=false;app.update();
        app.world_mut().resource_mut::<Laboratory>().tools.section.enabled=false;app.update();
        assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0,source);
        app.world_mut().resource_mut::<Laboratory>().tools.metric=3;app.update();
        assert_ne!(app.world().get::<MeshMaterial3d<StandardMaterial>>(entity).unwrap().0,original_material);
        app.world_mut().resource_mut::<Laboratory>().tools.metric=0;app.update();
        assert_eq!(app.world().get::<MeshMaterial3d<StandardMaterial>>(entity).unwrap().0,original_material);
        assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0,source);
        assert_eq!(app.world().resource::<Scene>().sim.state_hash(),before);
    }

    #[test]
    fn section_plane_uses_world_coordinates_and_flips_sides() {
        let mut state = State {
            enabled: true,
            anchor: [10, 20, 30], offset: 5, axis: 2, positive: true,
        };
        assert_eq!(state.point(), [10,20,35]);
        assert_eq!(state.plane(), Vec4::new(0.0,0.0,1.0,-35.0));
        state.positive = false;
        assert_eq!(state.plane(), Vec4::new(0.0,0.0,-1.0,35.0));
        state.set_axis(0);
        assert_eq!(state.point(), [10,20,35]);
        assert_eq!(state.plane(), Vec4::new(-1.0,0.0,0.0,10.0));
    }

    #[test]
    fn section_clips_rotated_geometry_on_every_axis_and_side() {
        let mut b=MeshBuf::default();
        b.pos=vec![[-2.0,-2.0,0.0],[2.0,-2.0,0.0],[0.0,2.0,0.0]];
        b.nrm=vec![[0.0,0.0,1.0];3];b.col=vec![[1.0;4];3];b.idx=vec![0,1,2];
        let source=mesh(&b);
        let t=Transform::from_xyz(10.0,20.0,30.0).with_rotation(Quat::from_euler(EulerRot::XYZ,0.4,0.7,0.2));
        for axis in 0..3 {for positive in [true,false] {
            let mut n=Vec3::ZERO;n[axis]=if positive {1.0}else{-1.0};
            let plane=n.extend(-n.dot(t.translation));
            let out=clip_mesh(&source,&t,plane,None).unwrap();
            let VertexAttributeValues::Float32x3(pos)=out.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else {panic!("positions")};
            assert!(pos.len()>=3);
            assert!(pos.iter().all(|p|n.dot(t.transform_point(Vec3::from(*p)))+plane.w>=-0.0001));
        }}
    }

    #[test]
    fn effect_mesh_is_cut_and_kept_editable_for_the_next_frame() {
        let mut b=MeshBuf::default();b.pos=vec![[-1.0,0.0,0.0],[1.0,0.0,0.0],[1.0,1.0,0.0]];
        b.nrm=vec![[0.0,0.0,1.0];3];b.col=vec![[1.0,0.5,0.0,0.5];3];b.idx=vec![0,1,2];
        let out=super::super::section_effect_mesh(mesh(&b),Some((0,0,true)));
        let VertexAttributeValues::Float32x3(pos)=out.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else {panic!("positions")};
        assert!(pos.iter().all(|p|p[0]>=0.0));
        assert!(out.asset_usage.contains(RenderAssetUsages::MAIN_WORLD));
        let VertexAttributeValues::Float32x4(col)=out.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() else {panic!("colours")};
        assert!(col.iter().all(|c|c[3]==0.5));
    }
}
