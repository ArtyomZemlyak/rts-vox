//! Bounded native finishes. Text is never sampled, displaced or split.
use super::*;
/// A newly published image needs one render extraction before it is a UI texture.
/// Keep that frame transparent instead of drawing Bevy's white fallback quad.
pub(super) fn preview_image(image:&mut ImageNode,handle:Handle<Image>) {
    if image.image!=handle {image.image=handle;image.color=Color::NONE;}
    else {image.color=if image.image==Handle::default(){Color::NONE}else{Color::WHITE};}
}
use bevy::asset::uuid_handle;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::ui_render::{ui_material::UiMaterial,UiMaterialPlugin};
const SHADER:Handle<Shader>=uuid_handle!("84c11b52-201b-4648-a864-a46d3d3613ad");
const MAX_SURFACES:usize=96;
#[path="optical_ui/acceptance.rs"]
pub(super) mod acceptance;
#[derive(Component,Clone,Copy,Debug,PartialEq)]
pub(super) struct Finish {pub kind:u8,pub variant:u8,pub events:bool,pub glitch:bool}
impl Finish {
    pub fn pixel_filter(variant:u8)->Self {Self{variant:8+variant%3,..Self::pixel(variant)}}
    pub const fn pixel(variant:u8)->Self {Self{kind:0,variant,events:true,glitch:false}}
    pub const fn data()->Self {Self{kind:1,variant:1,events:false,glitch:false}}
    pub const fn crt()->Self {Self{kind:3,variant:2,events:true,glitch:true}}
    fn optical()->Self {Self{kind:2,variant:0,events:true,glitch:true}}
    pub fn optical_chip()->Self {Self{variant:1,glitch:false,..Self::optical()}}
    fn fits(self,size:Vec2)->bool {size.x>0. && size.y>0. && size.x<=1100. && size.y<=700. && size.x*size.y<=480_000.
        && (self.kind!=3 || (size.x<=480. && size.y<=80.))}
}
#[derive(Asset,TypePath,AsBindGroup,Debug,Clone,PartialEq)]
pub struct OpticalGlass {
    #[uniform(0)] tint:Vec4,
    // time, aperture, kind, variant
    #[uniform(0)] phase:Vec4,
    #[uniform(0)] source:Vec4,
    // active, hover, flash, decorative split in pixels
    #[uniform(0)] signal:Vec4,
    // sync offset, event noise, enabled, seed
    #[uniform(0)] detail:Vec4,
    #[uniform(0)] motion:Vec4,
}
#[derive(Component,Default)]
pub(super) struct ProjectionEnvelope {pub source:Vec2,pub aperture:f32,pub energy:f32}
impl UiMaterial for OpticalGlass {fn fragment_shader()->ShaderRef {SHADER.into()}}
#[derive(SystemSet,Debug,Hash,PartialEq,Eq,Clone,Copy)]
pub(super) enum SignalSet {Animate}
pub struct OpticalUiPlugin;
impl Plugin for OpticalUiPlugin {
    fn build(&self,app:&mut App) {app.init_resource::<Audit>().init_resource::<acceptance::Sequence>().add_plugins(UiMaterialPlugin::<OpticalGlass>::default()).add_systems(Startup,shader)
        .add_systems(Update,acceptance::drive.before(frontend::input))
        .add_systems(PostUpdate,(attach,animate.in_set(SignalSet::Animate)).chain().before(bevy::ui::UiSystems::Layout))
        .add_systems(PostUpdate,acceptance::capture.after(ui_motion::MotionSet::Render).before(bevy::ui::UiSystems::Layout));}
}
fn shader(mut shaders:ResMut<Assets<Shader>>) {shaders.insert(SHADER.id(),Shader::from_wgsl(include_str!("optical_ui.wgsl"),"embedded/optical_ui.wgsl")).expect("signal shader");}
#[derive(Clone,Copy)]struct Event {age:f32,serial:u32}
impl Default for Event {fn default()->Self {Self{age:10.,serial:0}}}
impl Event {
    fn trigger(&mut self) {self.age=0.;self.serial=self.serial.wrapping_add(1);}
    fn sample(self,glitch:bool,seed:u32)->Vec3 {
        let flash=(1.-self.age/panel::theme::REVEAL_SECONDS).clamp(0.,1.).powi(2);
        let rare=glitch && self.serial.wrapping_add(seed).is_multiple_of(3);
        let split=if glitch && (0.025..0.09).contains(&self.age){1.25*flash}else{0.};
        let sync=if rare && (0.035..0.075).contains(&self.age){2.*flash}else{0.};
        Vec3::new(flash,split,sync)
    }
}
#[derive(Component)]struct Surface {finish:Option<Finish>,shown:bool,age:f32,event:Event,active:bool,interaction:Interaction,energy:f32,seed:u32,handle:Handle<OpticalGlass>,painted:bool}
impl Surface {fn new(finish:Option<Finish>,seed:u32)->Self {Self{finish,shown:false,age:0.,event:default(),active:false,interaction:Interaction::None,energy:0.,seed,handle:default(),painted:false}}}
#[derive(Resource,Default)]
pub(super) struct Audit {pub allocated:usize,pub drawn:usize,pub kinds:[usize;4],pub flash:f32,pub split:f32,pub sync:f32,pub area:f32}
#[derive(Component,Default)]struct HoverLight(f32);
#[derive(Component)]pub struct Rail;
fn attach(mut commands:Commands,mut materials:ResMut<Assets<OpticalGlass>>,
    panels:Query<(Entity,&BackgroundColor,Option<&spatial_ui::Part>,Has<Rail>,Has<laboratory::Root>,Option<&panel::theme::Style>,Option<&Finish>,Has<panel::LayoutOnly>,Has<frontend::Part>),
        (Or<(With<panel::PanelRoot>,With<laboratory::Root>,With<frontend::Part>,With<Finish>)>,Without<Surface>)>,
    buttons:Query<Entity,(With<Button>,Without<HoverLight>,Without<BoxShadow>)>) {
    // Explicit category finishes and world projections get the fixed budget first.
    let mut pending:Vec<_>=panels.iter().collect();pending.sort_by_key(|p|(if p.6.is_some(){0}else if p.2.is_some(){1}else{2},p.0.index()));
    for (entity,background,part,rail,lab,style,explicit,layout,frontend) in pending {
        let color=background.0.to_linear();
        let style=style.copied().unwrap_or(if part.is_some(){panel::theme::Style::Optical}
            else if lab || frontend || color.red>0.4 && color.green>0.4 && color.blue>0.4 {panel::theme::Style::Service}else{panel::theme::Style::Pixel});
        let finish=explicit.copied().or_else(||match style {
            panel::theme::Style::Optical=>Some(Finish::optical()),
            panel::theme::Style::Pixel=>Some(if rail{Finish::data()}else{Finish{events:false,..Finish::pixel(0)}}),_=>None,
        }).filter(|_|!layout && (explicit.is_some() || color.alpha>0.));
        let seed=entity.index_u32();
        if finish.is_none() || materials.len()>=MAX_SURFACES {commands.entity(entity).insert(Surface::new(None,seed));continue;}
        let f=finish.unwrap();let glass=materials.add(OpticalGlass{tint:Vec4::new(color.red,color.green,color.blue,color.alpha),phase:Vec4::new(0.,1.,f32::from(f.kind),f32::from(f.variant)),source:Vec4::ZERO,signal:Vec4::ZERO,detail:Vec4::ZERO,motion:Vec4::ONE});
        let mut surface=Surface::new(Some(f),seed);surface.handle=glass;commands.entity(entity).insert(surface);
        // Existing pixel backgrounds remain the authoritative native state colours.
        if f.kind==2 {commands.entity(entity).insert(BackgroundColor(Color::NONE));}
    }
    for e in &buttons {commands.entity(e).insert((HoverLight::default(),BoxShadow::default()));}
}
fn active_color(c:Color)->bool {[panel::theme::ACTIVE,panel::theme::PRESSED,panel::theme::OPTICAL.active,panel::theme::OPTICAL.pressed].contains(&c)}
fn animate(mut commands:Commands,time:Res<Time>,mut materials:ResMut<Assets<OpticalGlass>>,
    mut panels:Query<(Entity,&mut Surface,&Node,Option<&ChildOf>,Option<&ProjectionEnvelope>,Option<&Interaction>,&BackgroundColor,Option<&ComputedNode>)>,
    ancestors:Query<(&Node,Option<&ChildOf>)>,mut buttons:Query<(&Interaction,&mut HoverLight,&mut BoxShadow,Option<&panel::theme::Style>)>,
    menu:Option<Res<frontend::Menu>>,mut page:Local<Option<u8>>,mut audit:ResMut<Audit>,settings:Option<Res<ui_motion::Settings>>) {
    let strength=settings.as_ref().map_or(1.,|s|s.strength());
    let reduced=settings.as_ref().is_some_and(|s|s.reduced());
    let dt=time.delta_secs().min(0.1);let key=menu.as_ref().map(|m|m.signal_key());let page_event=*page!=key;*page=key;
    *audit=Audit{allocated:materials.len(),..default()};
    for (entity,mut s,node,parent,envelope,interaction,background,computed) in &mut panels {
        let Some(f)=s.finish else{continue};let mut shown=node.display!=Display::None;let mut parent=parent.map(ChildOf::parent);
        while shown {let Some(e)=parent else{break};let Ok((n,next))=ancestors.get(e)else{break};shown=n.display!=Display::None;parent=next.map(ChildOf::parent);}
        let size=computed.map_or(Vec2::ZERO,|n|n.size()*n.inverse_scale_factor());
        let i=interaction.copied().unwrap_or(Interaction::None);let active=active_color(background.0);let energy=envelope.map_or(0.,|e|e.energy);
        let changed=(!s.shown && shown) || (!s.active && active) || (i!=s.interaction && i!=Interaction::None) || energy>s.energy+0.2 || (f.kind==3 && page_event);
        s.event.age+=dt;if shown && changed && f.events{s.event.trigger();}
        s.age=if shown{if s.shown{s.age+dt}else{0.}}else{0.};s.shown=shown;s.active=active;s.interaction=i;s.energy=energy;
        let event=s.event.sample(f.glitch && !reduced,s.seed)*strength;
        // A settled CRT insert is exactly transparent, so needs no draw at all.
        let paint=shown && f.fits(size) && (strength>0. || f.kind==2) && (f.kind!=3 || event.x>0.);
        if paint!=s.painted {
            if paint {commands.entity(entity).insert(MaterialNode(s.handle.clone()));}
            else {commands.entity(entity).remove::<MaterialNode<OpticalGlass>>();}
            s.painted=paint;
        }
        // Hidden/disabled finishes have no MaterialNode to consume uniforms.
        // Preserve their event state above, and publish fresh values on wake.
        if !paint {continue;}
        if let Some(current)=materials.get(&s.handle) {
            let mut m=current.clone();
            if paint {audit.drawn+=1;audit.kinds[usize::from(f.kind)]+=1;audit.flash=audit.flash.max(event.x);audit.split=audit.split.max(event.y);audit.sync=audit.sync.max(event.z);audit.area=audit.area.max(size.x*size.y);}
            m.motion.x=strength;
            // The shader reads time only for event noise, multiplied by flash.
            // A settled material has the same pixels without a new GPU upload.
            if event.x>0.{m.phase.x=time.elapsed_secs()%120.;}m.phase.y=1.;
            m.signal=Vec4::new(if active{1.}else{0.},if i!=Interaction::None{1.}else{0.},event.x,event.y);
            m.detail=Vec4::new(event.z,event.x,if shown && f.fits(size){1.}else{0.},(s.seed%127) as f32);
            if f.kind==2 {m.phase.y=(s.age/panel::theme::REVEAL_SECONDS).clamp(0.,1.);
                if let Some(e)=envelope{m.phase.y=e.aperture;m.source=Vec4::new(e.source.x,e.source.y,e.energy,1.);}}
            if &m!=current {*materials.get_mut(&s.handle).expect("live optical material")=m;}
        }
    }
    for (i,mut light,mut shadow,style) in &mut buttons {
        if style.copied()!=Some(panel::theme::Style::Optical){continue;}
        let target=match i{Interaction::None=>0.,Interaction::Hovered=>1.,Interaction::Pressed=>0.65};
        light.0+=(target-light.0)*(dt/if *i==Interaction::Pressed{panel::theme::PRESS_SECONDS}else{panel::theme::HOVER_SECONDS}).min(1.);
        let next=BoxShadow::new(panel::theme::CYAN.with_alpha(light.0*0.23*strength),px(0),px(0),px(1),px(9));
        if *shadow!=next{*shadow=next;}
    }
}
/// Four-by-four native marks form a fixed pool beside category labels.
pub(super) fn pixel_glyph(p:&mut ChildSpawnerCommands,code:u8) {
    let mask=[0x6996u16,0x44e4,0xe8e2,0xe6e6,0xaae2,0xf99f,0x6ff6,0x6f99][usize::from(code)%8];
    p.spawn((Node{width:px(14),height:px(14),flex_shrink:0.,..default()},Pickable::IGNORE,bevy::ui::FocusPolicy::Pass)).with_children(|p|{
        for bit in 0..16 {if mask&(1<<bit)!=0{p.spawn((Node{position_type:PositionType::Absolute,left:px(1+(bit%4)*3),top:px(1+(bit/4)*3),width:px(2),height:px(2),..default()},BackgroundColor(panel::theme::CYAN.with_alpha(0.70)),Pickable::IGNORE,bevy::ui::FocusPolicy::Pass));}}
    });
}
/// Cached actual geometry, with warm edges, quantised facets and a transparent canvas.
#[cfg(test)]
pub(super) fn pixel_thumbnail(mut image:Image)->Image {
    let w=image.texture_descriptor.size.width as usize;let h=image.texture_descriptor.size.height as usize;
    let Some(data)=image.data.as_mut()else{return image};if w==0 || h==0 || data.len()!=w*h*4{return image;}
    let src=data.clone();for y in 0..h{for x in 0..w{let k=(y*w+x)*4;if src[k+3]==0{continue;}
        let edge=x==0 || y==0 || x+1==w || y+1==h || [k-4,k+4,k-w*4,k+w*4].into_iter().any(|n|src[n+3]==0);
        let l=(u16::from(src[k])+u16::from(src[k+1])+u16::from(src[k+2]))/3;let amber=edge || ((x/2+y/2)%7==0 && l>80);let level=(l/32*32+24).min(240) as u8;
        let rgb=if amber{[level.saturating_add(28),(u16::from(level)*3/4) as u8,(u16::from(level)*2/7) as u8]}
            else{[(u16::from(level)*4/5) as u8,level,level.saturating_add(8)]};
        data[k..k+3].copy_from_slice(&rgb);if !edge && (x+y)%4==0{data[k+3]=(u16::from(src[k+3])*4/5) as u8;}
    }}image.sampler=bevy::image::ImageSampler::nearest();image
}
#[cfg(test)]mod tests {
    use super::*;
    #[test]fn settled_optical_button_keeps_shadow_unchanged_and_hover_still_responds(){
        let mut app=App::new();app.init_resource::<Assets<OpticalGlass>>().init_resource::<Time>().init_resource::<Audit>().add_systems(Update,animate);
        let e=app.world_mut().spawn((Interaction::None,HoverLight::default(),BoxShadow::default(),panel::theme::Style::Optical)).id();
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(20));app.update();
        let quiet=app.world().entity(e).get_ref::<BoxShadow>().unwrap().last_changed();app.update();
        assert_eq!(app.world().entity(e).get_ref::<BoxShadow>().unwrap().last_changed(),quiet);
        *app.world_mut().get_mut::<Interaction>(e).unwrap()=Interaction::Hovered;app.update();
        assert_ne!(app.world().entity(e).get_ref::<BoxShadow>().unwrap().last_changed(),quiet);
        assert!(app.world().get::<HoverLight>(e).unwrap().0>0.);
    }
    #[test]fn hidden_optical_surfaces_preserve_gpu_uniforms_while_event_time_advances(){
        let mut app=App::new();app.init_resource::<Assets<OpticalGlass>>().init_resource::<Time>().init_resource::<Audit>().add_systems(Update,(attach,animate).chain());
        let entity=app.world_mut().spawn((Finish::pixel(1),Node{display:Display::None,..default()},BackgroundColor(panel::theme::ACTIVE))).id();
        app.update();let handle=app.world().get::<Surface>(entity).unwrap().handle.clone();
        let before=app.world().resource::<Assets<OpticalGlass>>().get(&handle).unwrap().clone();
        for _ in 0..8 {app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(50));app.update();}
        assert_eq!(app.world().resource::<Assets<OpticalGlass>>().get(&handle).unwrap(),&before);
        assert!(app.world().get::<Surface>(entity).unwrap().event.age>10.);
        assert!(app.world().get::<MaterialNode<OpticalGlass>>(entity).is_none());
    }
    #[test]fn pending_preview_never_flashes_a_white_fallback_quad(){
        let mut images=Assets::<Image>::default();let handle=images.add(Image::default());let mut image=ImageNode::default();
        preview_image(&mut image,handle.clone());assert_eq!(image.color,Color::NONE);
        preview_image(&mut image,handle);assert_eq!(image.color,Color::WHITE);
        preview_image(&mut image,Handle::default());assert_eq!(image.color,Color::NONE);
        preview_image(&mut image,Handle::default());assert_eq!(image.color,Color::NONE);
    }
    #[test]fn real_material_keeps_native_text_colours_and_hit_geometry_unchanged(){
        let mut app=App::new();app.init_resource::<Assets<OpticalGlass>>().init_resource::<Time>().init_resource::<Audit>()
            .add_systems(Update,(attach,animate).chain());
        let e=app.world_mut().spawn((Finish::pixel(1),Node{left:px(24),top:px(48),width:px(180),height:px(32),..default()},
            BackgroundColor(panel::theme::ACTIVE),Text::new("М108 / Э +68"),TextFont{font_size:FontSize::Px(14.),..default()},TextColor(Color::WHITE))).id();
        app.update();assert_eq!(app.world().get::<BackgroundColor>(e).unwrap().0,panel::theme::ACTIVE);
        assert_eq!(app.world().get::<Text>(e).unwrap().0,"М108 / Э +68");
        let n=app.world().get::<Node>(e).unwrap();assert_eq!((n.left,n.top,n.width,n.height),(px(24),px(48),px(180),px(32)));
        assert_eq!(app.world().get::<TextFont>(e).unwrap().font_size,FontSize::Px(14.));assert_eq!(app.world().get::<TextColor>(e).unwrap().0,Color::WHITE);
        for _ in 0..6 {app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(50));app.update();}
        let s=app.world().get::<Surface>(e).unwrap();let m=app.world().resource::<Assets<OpticalGlass>>().get(&s.handle).unwrap();
        assert_eq!(m.signal.z,0.);assert_eq!(m.signal.w,0.);assert_eq!(m.detail.x,0.);
    }
    #[test]fn actual_surface_pool_stops_at_the_budget_without_removing_native_controls(){
        let mut app=App::new();app.init_resource::<Assets<OpticalGlass>>().add_systems(Update,attach);
        for _ in 0..MAX_SURFACES+4 {app.world_mut().spawn((Finish::pixel(0),Node::default(),BackgroundColor(panel::theme::BUTTON)));}
        app.update();assert_eq!(app.world().resource::<Assets<OpticalGlass>>().len(),MAX_SURFACES);
        assert_eq!(app.world_mut().query::<&BackgroundColor>().iter(app.world()).filter(|b|b.0==panel::theme::BUTTON).count(),MAX_SURFACES+4);
    }
    #[test]fn events_end_exactly_and_glitches_are_rare_bounded_and_decorative(){let mut e=Event::default();assert_eq!(e.sample(true,0),Vec3::ZERO);
        for n in 0..9{e.trigger();e.age=0.05;let s=e.sample(true,0);assert!(s.y<=1.25 && s.z<=2.);assert_eq!(s.z>0.,(n+1)%3==0);e.age=panel::theme::REVEAL_SECONDS;assert_eq!(e.sample(true,0),Vec3::ZERO);}
        e.trigger();e.age=0.05;let s=e.sample(false,0);assert_eq!(s.y,0.);assert_eq!(s.z,0.);}
    #[test]fn finish_budget_rejects_fullscreen_and_crt_outside_a_local_strip(){assert!(!Finish::pixel(0).fits(Vec2::new(3840.,2160.)));assert!(!Finish::crt().fits(Vec2::new(900.,60.)));assert!(Finish::crt().fits(Vec2::new(360.,48.)));assert!(Finish::data().fits(Vec2::new(440.,160.)));assert_eq!(MAX_SURFACES,96);}
    #[test]fn native_material_queries_initialize_without_aliasing(){use bevy::ecs::system::{IntoSystem,System};IntoSystem::into_system(attach).initialize(&mut World::new());IntoSystem::into_system(animate).initialize(&mut World::new());}
    #[test]fn phosphor_icons_preserve_transparent_canvas_and_black_geometry(){use bevy::render::render_resource::{Extent3d,TextureDimension,TextureFormat};
        let src=Image::new(Extent3d{width:2,height:2,depth_or_array_layers:1},TextureDimension::D2,vec![0,0,0,255,5,5,5,0,100,120,140,255,200,200,200,255],TextureFormat::Rgba8UnormSrgb,bevy::asset::RenderAssetUsages::default());
        let result=pixel_thumbnail(src);let data=result.data.unwrap();assert_eq!(data[3],255);assert_eq!(data[7],0);assert_eq!(data.chunks_exact(4).filter(|p|p[3]>0).count(),3);assert!(data[0]>data[2]);}
}
