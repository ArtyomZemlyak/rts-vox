//! Shared event-driven UI motion. Stable controls, routed accents, bounded native strokes.
use super::*;
use bevy::ui::{FocusPolicy,UiGlobalTransform,LayoutConfig};
use std::collections::BTreeMap;
use std::time::Duration;
#[path="ui_motion/acceptance.rs"]pub(super) mod acceptance;
#[cfg(test)]
#[path="ui_motion/contract_tests.rs"]mod contract_tests;
const TRACKS:usize=512;
const EFFECTS:usize=16;
const PIECES:usize=12;
const FILE:&str="out/ui-motion.txt";
#[derive(Resource,Clone,Copy)]
pub(super) struct Settings {pub percent:u8}
impl Default for Settings {fn default()->Self{Self{percent:100}}}
impl Settings {
    pub fn load(value:Option<u8>)->Self {Self{percent:value.or_else(||std::fs::read_to_string(FILE).ok()?.trim().parse().ok()).unwrap_or(100).min(100)}}
    pub fn strength(self)->f32 {f32::from(self.percent)/100.}
    pub fn reduced(self)->bool {self.percent<=35}
}
#[derive(Component)]struct IntensityButton;
#[derive(Component)]pub(super) struct IntensityLabel;
pub fn settings_control(p:&mut ChildSpawnerCommands,font:&TextFont) {
    p.spawn((IntensityButton,Button,panel::theme::Style::Service,Node{padding:UiRect::axes(px(6),px(4)),border:UiRect::all(px(1)),..default()},
        BackgroundColor(panel::theme::service::BUTTON),BorderColor::all(panel::theme::service::FRAME)))
        .with_children(|p|{p.spawn((IntensityLabel,Text::new(""),TextFont{font_size:FontSize::Px(14.),..font.clone()},TextColor(panel::theme::service::TEXT)));});
}
fn input(mut settings:ResMut<Settings>,buttons:Query<&Interaction,(With<IntensityButton>,Changed<Interaction>)>,
    mut labels:Query<&mut Text,With<IntensityLabel>>,scene:Option<Res<Scene>>,mut last:Local<Option<u8>>) {
    if buttons.iter().any(|i|*i==Interaction::Pressed) {
        settings.percent=match settings.percent {66..=100=>35,1..=65=>0,_=>100};
        if scene.is_some_and(|s|s.out.is_none()) {let _=std::fs::write(FILE,format!("{}\n",settings.percent));}
    }
    if *last!=Some(settings.percent) || labels.iter().any(|t|t.0.is_empty()) {
        let label=format!("UI эффекты: {}% · сменить",settings.percent);
        for mut t in &mut labels {if t.0!=label {t.0.clone_from(&label);}}
        *last=Some(settings.percent);
    }
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum Role {Panel,Menu,Service,Tooltip,Tab,Button}
#[derive(Component,Clone,Copy)]
pub(super) struct Target {pub role:Role,pub group:u8,pub active:Option<bool>}
impl Target {pub const fn panel(role:Role)->Self {Self{role,group:0,active:None}}pub const fn tab(group:u8)->Self{Self{role:Role::Tab,group,active:None}}}
#[derive(Component)]struct Tracked;
#[derive(Clone,Copy,Debug,Default,PartialEq)]struct Box2 {at:Vec2,size:Vec2}
impl Box2 {fn centre(self)->Vec2{self.at+self.size*0.5}fn corners(self)->[Vec2;4]{[self.at,self.at+Vec2::new(self.size.x,0.),self.at+self.size,self.at+Vec2::new(0.,self.size.y)]}}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub(super) enum Kind {Open,Close,Hover,Unhover,Select,Change,Advance,Complete}
impl Kind {
    fn lifetime(self)->Duration {Duration::from_millis(match self{Self::Hover=>90,Self::Unhover=>60,Self::Select=>120,Self::Open=>240,Self::Close=>180,Self::Change=>180,Self::Advance=>200,Self::Complete=>280})}
    fn duration(self)->f32 {self.lifetime().as_secs_f32()}
}
#[derive(Clone,Copy)]struct Effect {owner:Entity,kind:Kind,from:Vec2,to:Vec2,rect:Box2,old:Option<Box2>,age:Duration,color:Color}
struct Track {role:Role,shown:bool,over:bool,active:bool,rect:Box2,base:Color,output:Color,opening:f32,optical:bool,service:bool}
#[derive(Clone,Copy,Debug,PartialEq)]pub(super) struct ProductionSample {pub actor:usize,pub head:Option<usize>,pub count:usize,pub progress:u64,pub exit:Option<(usize,u32)>}
fn production_change(old:ProductionSample,new:ProductionSample)->Option<Kind> {
    if new.exit.is_some() && new.exit.map(|x|x.0)!=old.exit.map(|x|x.0) {Some(Kind::Complete)}
    else if old.head!=new.head || old.count!=new.count {Some(Kind::Advance)}else{None}
}
#[derive(Resource)]pub(super) struct Registry {
    tracks:BTreeMap<Entity,Track>,effects:Vec<Effect>,groups:BTreeMap<u8,(Entity,Box2)>,
    pub(super) production:BTreeMap<usize,ProductionSample>,production_revision:Option<u64>,generation:Option<u64>,scale:f32,
    pressed:Option<Vec2>,hovered:Option<Vec2>,prepared:bool,pub completed:u64,pub advanced:u64,
    pub emitted:u64,pub visible_effects:usize,
}
impl Default for Registry {fn default()->Self{Self{tracks:BTreeMap::new(),effects:Vec::new(),groups:BTreeMap::new(),production:BTreeMap::new(),production_revision:None,generation:None,scale:1.,pressed:None,hovered:None,prepared:false,completed:0,advanced:0,emitted:0,visible_effects:0}}}
impl Registry {
    fn follow_layout(&mut self) {
        for effect in &mut self.effects {
            if matches!(effect.kind,Kind::Close|Kind::Advance|Kind::Complete){continue;}
            if let Some(t)=self.tracks.get(&effect.owner).filter(|t|t.shown) {effect.rect=t.rect;effect.to=t.rect.centre();}
        }
    }
    fn tick(&mut self,dt:Duration) {for e in &mut self.effects{e.age=e.age.saturating_add(dt);}self.effects.retain(|e|e.age<e.kind.lifetime());self.visible_effects=self.effects.len();}
    fn emit(&mut self,e:Effect) {
        self.effects.retain(|x|x.owner!=e.owner || match e.kind {Kind::Open|Kind::Close=>!matches!(x.kind,Kind::Open|Kind::Close),Kind::Hover|Kind::Unhover=>!matches!(x.kind,Kind::Hover|Kind::Unhover),Kind::Complete=>x.kind!=e.kind || x.to!=e.to,_=>x.kind!=e.kind});
        if self.effects.len()>=EFFECTS {if let Some(i)=self.effects.iter().position(|x|matches!(x.kind,Kind::Hover|Kind::Unhover)){self.effects.remove(i);}else{self.effects.remove(0);}}
        self.effects.push(e);self.emitted+=1;
    }
    fn queue(&mut self,owner:Entity,new:ProductionSample,destination:Option<Vec2>) {
        if let Some(old)=self.production.insert(new.actor,new) {
            if let Some(kind)=production_change(old,new) {
                if kind==Kind::Complete{self.completed+=1;}else{self.advanced+=1;}
                if let Some(t)=self.tracks.get(&owner).filter(|t|t.shown || kind==Kind::Complete) {
                    let rect=t.rect;let from=rect.at+Vec2::new(24.,rect.size.y-12.);let to=destination.unwrap_or(rect.at+Vec2::new(24.,34.));
                    self.emit(Effect{owner,kind,from,to,rect,old:None,age:Duration::ZERO,color:panel::theme::AMBER});
                }
            }
        }
    }
}
#[derive(Component)]struct Piece(usize,usize);
pub fn build(commands:&mut Commands,camera:Entity) {
    for slot in 0..EFFECTS {for i in 0..PIECES {commands.spawn((Piece(slot,i),Node{position_type:PositionType::Absolute,display:Display::None,..default()},
        UiTransform::default(),LayoutConfig{use_rounding:false},BackgroundColor(Color::NONE),Pickable::IGNORE,FocusPolicy::Pass,UiTargetCamera(camera),GlobalZIndex(300)));}}
}
#[derive(SystemSet,Debug,Hash,PartialEq,Eq,Clone,Copy)]
pub(super) enum MotionSet {Render}
pub struct MotionPlugin;
impl Plugin for MotionPlugin {
    fn build(&self,app:&mut App) {app.init_resource::<Settings>().init_resource::<Registry>().init_resource::<acceptance::Sequence>()
        .add_systems(Update,acceptance::drive.before(frontend::input).before(spatial_ui::input))
        .add_systems(Update,input.before(frontend::input))
        .add_systems(PostUpdate,(attach,prepare).chain().before(optical_ui::SignalSet::Animate))
        .add_systems(PostUpdate,(production,render.in_set(MotionSet::Render),acceptance::capture).chain().after(optical_ui::SignalSet::Animate).before(bevy::ui::UiSystems::Layout));}
}
fn attach(mut commands:Commands,registry:Res<Registry>,
    nodes:Query<(Entity,&Node,Option<&BackgroundColor>,Has<Button>,Has<panel::PanelRoot>,Has<laboratory::Root>,Has<knobs::SettingsRoot>,
        Has<construction::RecipeTooltip>,Option<&construction::Action>,Option<&laboratory::LabAction>,Option<&panel::Action>,Option<&Target>,Has<spatial_ui::Part>,Option<&ChildOf>),(Without<Tracked>,Without<Piece>,Without<panel::LayoutOnly>,Or<(With<Button>,With<panel::PanelRoot>,With<laboratory::Root>,With<knobs::SettingsRoot>,With<construction::RecipeTooltip>,With<Target>)>)>,ancestors:Query<(&Node,Option<&ChildOf>)>) {
    let mut targets=Vec::new();
    for (e,node,bg,button,panel,lab,settings,tooltip,c,l,a,explicit,spatial,parent) in &nodes {
        if !shown(node,parent,&ancestors){continue;}
        if spatial && !button && !explicit.is_some_and(|t|t.role==Role::Tooltip){continue;}
        let role=explicit.copied().or_else(||if spatial{Some(Target{group:8,..Target::panel(Role::Button)})}else if tooltip{Some(Target::panel(Role::Tooltip))}else if settings{Some(Target::panel(Role::Service))}
            else if panel || lab {bg.filter(|b|b.0.to_linear().alpha>0.).map(|_|Target::panel(if lab{Role::Service}else{Role::Panel}))}
            else if button {Some(match (c,l,a) {
                (Some(construction::Action::Tier(_)),_,_)=>Target::tab(1),(Some(construction::Action::Category(_)),_,_)=>Target::tab(2),
                (_,Some(laboratory::LabAction::Category(_)),_)=>Target::tab(3),(_,Some(laboratory::LabAction::Tab(_)),_)=>Target::tab(4),
                (_,Some(laboratory::LabAction::Tool(_)),_)=>Target::tab(5),(_,_,Some(panel::Action::Group(_)))=>Target::tab(6),_=>Target::panel(Role::Button)})}
            else{None});
        if let Some(role)=role {targets.push((e,role));}
    }
    targets.sort_by_key(|(e,t)|(if matches!(t.role,Role::Panel|Role::Menu|Role::Service|Role::Tooltip){0}else if t.role==Role::Tab{1}else{2},e.index()));
    let capacity=TRACKS.saturating_sub(registry.tracks.len());
    for (e,target) in targets.into_iter().take(capacity){commands.entity(e).insert((target,Tracked));}
}
fn shown(node:&Node,parent:Option<&ChildOf>,ancestors:&Query<(&Node,Option<&ChildOf>)>)->bool {
    if node.display==Display::None{return false;}let mut next=parent.map(ChildOf::parent);
    while let Some(e)=next {let Ok((n,p))=ancestors.get(e)else{break};if n.display==Display::None{return false;}next=p.map(ChildOf::parent);}true
}
fn selected(c:Color)->bool {[panel::theme::ACTIVE,panel::theme::PRESSED,panel::theme::OPTICAL.active,panel::theme::OPTICAL.pressed,panel::theme::SERVICE.active,panel::theme::SERVICE.pressed].contains(&c)}
fn rect(n:&ComputedNode,global:&UiGlobalTransform)->Option<Box2> {
    let inverse=n.inverse_scale_factor();let half=n.size()*0.5;
    let points=[-half,Vec2::new(half.x,-half.y),half,Vec2::new(-half.x,half.y)].map(|p|global.affine().transform_point2(p)*inverse);
    let lo=points.into_iter().fold(Vec2::splat(f32::INFINITY),Vec2::min);let hi=points.into_iter().fold(Vec2::splat(f32::NEG_INFINITY),Vec2::max);let size=hi-lo;
    (size.x>1. && size.y>1.).then_some(Box2{at:lo,size})
}
fn prepare(mut registry:ResMut<Registry>,time:Res<Time>,scale:Option<Res<UiScale>>,scene:Option<Res<Scene>>,
    mut nodes:Query<(Entity,&Target,&Node,&ComputedNode,&UiGlobalTransform,Option<&ChildOf>,Option<&Interaction>,Option<&mut BackgroundColor>,Option<&construction::Action>,Option<&laboratory::LabAction>,Option<&panel::Action>),With<Tracked>>,
    ancestors:Query<(&Node,Option<&ChildOf>)>,catalog:Option<Res<construction::Catalog>>,lab:Option<Res<laboratory::Laboratory>>,knobs:Option<Res<knobs::Overrides>>,mut commands:Commands) {
    // Prior effects age before this frame emits new ones. Keep Duration endpoints.
    registry.tick(time.delta().min(Duration::from_millis(100)));registry.prepared=true;
    let dt=time.delta_secs().min(0.1);let scale=scale.map_or(1.,|s|s.0);
    registry.tracks.retain(|e,_|nodes.contains(*e));
    if registry.scale!=scale {
        let ratio=registry.scale/scale;for t in registry.tracks.values_mut(){t.rect.at*=ratio;t.rect.size*=ratio;}
        for f in &mut registry.effects {f.from*=ratio;f.to*=ratio;f.rect.at*=ratio;f.rect.size*=ratio;if let Some(old)=f.old.as_mut(){old.at*=ratio;old.size*=ratio;}}
        for (_,r) in registry.groups.values_mut(){r.at*=ratio;r.size*=ratio;}registry.scale=scale;
        registry.pressed=registry.pressed.map(|p|p*ratio);registry.hovered=registry.hovered.map(|p|p*ratio);
    }
    if let Some(scene)=scene {if registry.generation.is_some_and(|g|g!=scene.generation){registry.effects.clear();registry.groups.clear();registry.production.clear();registry.production_revision=None;}registry.generation=Some(scene.generation);}
    let mut pending=Vec::new();
    for (entity,target,node,computed,global,parent,interaction,mut background,c,l,k) in &mut nodes {
        let is_shown=shown(node,parent,&ancestors);let measured=rect(computed,global);let i=interaction.copied().unwrap_or(Interaction::None);
        let over=is_shown && i!=Interaction::None;
        if let Some(r)=measured {if i==Interaction::Pressed && is_shown{registry.pressed=Some(r.centre());}if over{registry.hovered=Some(r.centre());}}
        let base=background.as_ref().map_or(Color::NONE,|b|b.0);
        let source=if target.role==Role::Tooltip{registry.hovered.or(registry.pressed)}else{registry.pressed};
        let t=registry.tracks.entry(entity).or_insert_with(||Track{role:target.role,shown:false,over:false,active:false,rect:measured.unwrap_or_default(),base,output:base,opening:1.,optical:target.group==8,service:matches!(target.role,Role::Service)||(3..=6).contains(&target.group)||(base.to_linear().red>0.4&&base.to_linear().green>0.4&&base.to_linear().blue>0.4)});
        if let Some(ref mut b)=background {if b.0!=t.output {t.base=b.0;}if b.0!=t.base{b.0=t.base;}}
        let state=c.and_then(|a|catalog.as_ref().and_then(|v|v.motion_selected(*a)))
            .or_else(||l.and_then(|a|lab.as_ref().map(|v|laboratory::motion_selected(v,*a))))
            .or_else(||match k{Some(panel::Action::Group(g))=>knobs.as_ref().map(|v|v.group==usize::from(*g)),_=>None});
        let active=target.active.or(state).unwrap_or_else(||selected(t.base));let visible=is_shown && (measured.is_some() || t.rect.size.x>1.);
        if let Some(r)=measured {if visible{if t.shown && (r.size-t.rect.size).length()>8. && !matches!(target.role,Role::Button|Role::Tab){pending.push((entity,Kind::Change,r,source,0,Some(t.rect)));}t.rect=r;}}
        let r=t.rect;
        if visible!=t.shown && !matches!(target.role,Role::Button|Role::Tab) {
            t.opening=if visible{0.}else{1.};pending.push((entity,if visible{Kind::Open}else{Kind::Close},r,source,0,None));
        }
        if over!=t.over {pending.push((entity,if over{Kind::Hover}else{Kind::Unhover},r,source,0,None));}
        if visible && active && !t.active {pending.push((entity,Kind::Select,r,source,target.group,None));}
        t.shown=visible;t.over=over;t.active=active;t.opening=(t.opening+dt/0.24).min(1.);
    }
    for (owner,kind,r,source,group,old) in pending {
        let previous=if group>0 {registry.groups.get(&group).map(|(_,r)|*r)}else{old};
        if group>0{registry.groups.insert(group,(owner,r));}
        let color=if kind==Kind::Select {if registry.tracks.get(&owner).is_some_and(|t|t.service){panel::theme::service::FOCUS}else{panel::theme::CYAN}}else if registry.tracks.get(&owner).is_some_and(|t|t.optical){panel::theme::CYAN}else if registry.tracks.get(&owner).is_some_and(|t|matches!(t.role,Role::Service|Role::Menu)||t.service){Color::srgb(0.22,0.58,0.66)}else{panel::theme::AMBER};
        let from=previous.map(Box2::centre).or(source).unwrap_or(r.at+Vec2::new(8.,8.));
        registry.emit(Effect{owner,kind,from,to:r.centre(),rect:r,old:previous,age:Duration::ZERO,color});
    }
    // Layout runs after motion. Rebind in-flight decoration to the next valid
    // computed bounds, including a tooltip's first measured text height.
    registry.follow_layout();
    let retired:Vec<_>=registry.tracks.iter().filter_map(|(&e,t)|(!t.shown && !registry.effects.iter().any(|fx|fx.owner==e)).then_some(e)).collect();
    for e in retired{registry.tracks.remove(&e);commands.entity(e).remove::<Tracked>();}
}
/// Canonical goal is published by the queue. Only the bar interpolates; numeric text is live.
#[derive(Component,Default)]pub(super) struct Progress {pub goal:f32,value:f32,start:f32,last:f32,age:f32,ready:bool}
impl Progress {
    fn step(&mut self,dt:f32)->f32 {
        if !self.ready || self.goal<self.last {self.value=self.goal;self.start=self.goal;self.age=0.08;self.ready=true;}
        else if self.goal!=self.last {self.start=self.value;self.age=0.;}
        self.age=(self.age+dt).min(0.08);self.last=self.goal;self.value=self.start+(self.goal-self.start)*smooth(self.age/0.08);
        self.value.min(self.goal).clamp(0.,100.)
    }
}
fn production(mut registry:ResMut<Registry>,scene:Option<Res<Scene>>,state:Option<Res<construction::production_hud::State>>,
    roots:Query<Entity,With<construction::production_hud::Root>>,cameras:Query<(&Camera,&Transform),With<workbench::EditorCamera>>,scale:Option<Res<UiScale>>) {
    let (Some(scene),Some(state),Some(root))=(scene,state,roots.iter().next())else{return;};
    if registry.production_revision==Some(state.motion_revision()){return;}registry.production_revision=Some(state.motion_revision());
    let mut samples:BTreeMap<_,_>=state.motion_snapshot().map(|(actor,head,count,progress)|(actor,(head,count,progress))).collect();
    // An emptied line vanishes from HUD publication in the very tick of its final exit.
    // Retain those producer IDs, not a world walk, to observe final completion/cancellation.
    for &actor in registry.production.keys(){samples.entry(actor).or_insert((None,0,0));}
    for (actor,(head,count,progress)) in samples {
        let exit=scene.sim.production_exit(actor);
        let destination=exit.and_then(|(id,_)|{let u=scene.sim.units.get(id)?;let (c,t)=cameras.single().ok()?;
            let p=u.motion.pos.map(|v|v.to_f64_lossy() as f32);c.world_to_viewport(&GlobalTransform::from(*t),Vec3::from(p)).ok().map(|p|p/scale.as_ref().map_or(1.,|s|s.0))});
        registry.queue(root,ProductionSample{actor,head,count,progress,exit},destination);
    }
}
fn stroke(node:&mut Mut<Node>,transform:&mut Mut<UiTransform>,a:Vec2,b:Vec2,width:f32) {
    let delta=b-a;let length=delta.length().max(0.1);let mid=(a+b)*0.5;
    // A fixed layout box lets native UI move the decoration without invalidating Taffy.
    if node.display!=Display::Flex || node.left!=px(0) || node.top!=px(0) || node.width!=px(1) || node.height!=px(1) {
        node.display=Display::Flex;node.left=px(0);node.top=px(0);node.width=px(1);node.height=px(1);
    }
    transform.set_if_neq(UiTransform{translation:Val2::new(px(mid.x-0.5),px(mid.y-0.5)),scale:Vec2::new(length,width),rotation:Rot2::radians(delta.y.atan2(delta.x))});
}
fn smooth(t:f32)->f32 {let t=t.clamp(0.,1.);t*t*(3.-2.*t)}
fn render(mut registry:ResMut<Registry>,time:Res<Time>,settings:Res<Settings>,
    mut pieces:Query<(&Piece,&mut Node,&mut UiTransform,&mut BackgroundColor)>,
    mut backgrounds:Query<(Entity,&mut BackgroundColor),(Without<Piece>,With<Tracked>)>,
    mut meters:Query<(&mut Progress,&mut Node),Without<Piece>>) {
    let dt=time.delta().min(Duration::from_millis(100));
    if !std::mem::take(&mut registry.prepared) {registry.tick(dt);}
    let strength=settings.strength();
    for (piece,mut n,mut transform,mut color) in &mut pieces {
        let Some(e)=registry.effects.get(piece.0)else{if n.display!=Display::None{n.display=Display::None;}continue;};
        if strength==0.{if n.display!=Display::None{n.display=Display::None;}continue;}
        let t=(e.age.as_secs_f32()/e.kind.duration()).clamp(0.,1.);let fade=(1.-t).min(0.7)*strength;
        let corners=e.rect.corners();let index=piece.1;let width=if matches!(e.kind,Kind::Hover|Kind::Unhover){1.}else{2.};
        if e.kind==Kind::Select && e.old.is_some() {
            let old=e.old.unwrap();let at=(old.at+Vec2::new(0.,old.size.y)).lerp(e.rect.at+Vec2::new(0.,e.rect.size.y),smooth(t));
            let length=old.size.x+(e.rect.size.x-old.size.x)*smooth(t);
            if index<8{let a=at+Vec2::X*(length*index as f32/8.);stroke(&mut n,&mut transform,a,a+Vec2::X*(length/8.-2.).max(1.),2.);}else if n.display!=Display::None{n.display=Display::None;}
        } else if matches!(e.kind,Kind::Advance|Kind::Complete) {
            if index<8 {let p=e.from.lerp(e.to,smooth((t+index as f32*0.035).min(1.)));stroke(&mut n,&mut transform,p,p+Vec2::X*4.,if e.kind==Kind::Complete{3.}else{2.});}
            else {let c=e.rect.at+Vec2::new(12.,12.);let v=Vec2::from_angle((index-8) as f32*std::f32::consts::FRAC_PI_2);stroke(&mut n,&mut transform,c+v*(3.+t*10.),c+v*(8.+t*12.),1.);}
        } else if index<8 {
            let edge=index/2;let a=corners[edge].lerp(corners[(edge+1)%4],(index%2) as f32*0.5);
            let b=corners[edge].lerp(corners[(edge+1)%4],(index%2+1) as f32*0.5);
            let progress=if matches!(e.kind,Kind::Close|Kind::Unhover){1.-smooth(t)}else{smooth((t*1.5-index as f32*0.065).clamp(0.,1.))};
            let origin=if matches!(e.kind,Kind::Hover|Kind::Unhover){a}else{e.from};
            stroke(&mut n,&mut transform,origin.lerp(a,progress),origin.lerp(b,progress),width);
        } else if index==8 && !matches!(e.kind,Kind::Hover|Kind::Unhover) {
            let destination=corners.into_iter().min_by(|a,b|a.distance_squared(e.from).total_cmp(&b.distance_squared(e.from))).unwrap();
            stroke(&mut n,&mut transform,e.from,e.from.lerp(destination,smooth((t*3.).min(1.))),1.);
        } else if n.display!=Display::None{n.display=Display::None;}
        let next=e.color.with_alpha(fade*if index==8{0.20}else{0.75});if color.0!=next{color.0=next;}
    }
    for (entity,mut b) in &mut backgrounds {
        if let Some(track)=registry.tracks.get_mut(&entity){
            let factor=if track.shown && !matches!(track.role,Role::Button|Role::Tab){1.-0.20*strength*(1.-smooth(track.opening))}else{1.};
            let next=track.base.with_alpha(track.base.to_linear().alpha*factor);
            if b.0!=next{b.0=next;}track.output=next;
        }
    }
    for (mut p,mut node) in &mut meters {
        let next=percent(p.step(dt.as_secs_f32()));if node.width!=next{node.width=next;}
    }
}

#[cfg(test)]mod tests {
    use super::*;
    #[test]fn moving_motion_stroke_keeps_layout_fixed_and_preserves_world_endpoints(){
        let mut app=App::new();app.init_resource::<Registry>().init_resource::<Time>().init_resource::<Settings>().add_systems(Update,render);
        let owner=app.world_mut().spawn_empty().id();let piece=app.world_mut().spawn((Piece(0,0),Node{position_type:PositionType::Absolute,display:Display::None,..default()},UiTransform::default(),BackgroundColor(Color::NONE))).id();
        app.world_mut().resource_mut::<Registry>().emit(Effect{owner,kind:Kind::Open,from:Vec2::new(12.,18.),to:Vec2::new(80.,60.),rect:Box2{at:Vec2::new(50.,30.),size:Vec2::new(80.,60.)},old:None,age:Duration::ZERO,color:Color::WHITE});
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(20));app.update();
        let changed=app.world().entity(piece).get_ref::<Node>().unwrap().last_changed();let first=*app.world().get::<UiTransform>(piece).unwrap();
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(20));app.update();
        assert_eq!(app.world().entity(piece).get_ref::<Node>().unwrap().last_changed(),changed,"Moving a visible stroke must not dirty layout");
        let node=app.world().get::<Node>(piece).unwrap();assert_eq!((node.left,node.top,node.width,node.height),(px(0),px(0),px(1),px(1)));
        let transform=app.world().get::<UiTransform>(piece).unwrap();assert_ne!(*transform,first);
        let e=&app.world().resource::<Registry>().effects[0];let t=e.age.as_secs_f32()/e.kind.duration();let progress=smooth((t*1.5).clamp(0.,1.));let a=e.from.lerp(e.rect.at,progress);let b=e.from.lerp(e.rect.at+Vec2::new(40.,0.),progress);
        for scale in [0.5,1.,1.75,2.] {
            let affine=transform.compute_affine(scale,Vec2::splat(scale),Vec2::splat(scale));let centre=Vec2::splat(0.5*scale);
            assert!((affine.transform_point2(Vec2::new(-0.5*scale,0.))+centre-a*scale).length()<0.001);
            assert!((affine.transform_point2(Vec2::new(0.5*scale,0.))+centre-b*scale).length()<0.001);
        }
    }
    #[test]fn dormant_motion_strokes_do_not_dirty_layout_but_wake_and_retire_immediately(){
        let mut app=App::new();app.init_resource::<Registry>().init_resource::<Time>().init_resource::<Settings>().add_systems(Update,render);
        let owner=app.world_mut().spawn_empty().id();
        let piece=app.world_mut().spawn((Piece(0,0),Node{display:Display::None,..default()},UiTransform::default(),BackgroundColor(Color::NONE))).id();
        app.update();let quiet=app.world().entity(piece).get_ref::<Node>().unwrap().last_changed();
        app.update();assert_eq!(app.world().entity(piece).get_ref::<Node>().unwrap().last_changed(),quiet);
        app.world_mut().resource_mut::<Registry>().emit(Effect{owner,kind:Kind::Open,from:Vec2::ZERO,to:Vec2::ONE,rect:Box2{at:Vec2::ZERO,size:Vec2::splat(40.)},old:None,age:Duration::ZERO,color:Color::WHITE});
        app.update();assert_eq!(app.world().get::<Node>(piece).unwrap().display,Display::Flex);
        app.world_mut().resource_mut::<Registry>().effects.clear();app.update();assert_eq!(app.world().get::<Node>(piece).unwrap().display,Display::None);
        let retired=app.world().entity(piece).get_ref::<Node>().unwrap().last_changed();app.update();
        assert_eq!(app.world().entity(piece).get_ref::<Node>().unwrap().last_changed(),retired);
    }
    #[test]fn opening_frame_follows_measured_layout_but_closed_frame_keeps_its_origin(){
        let mut r=Registry::default();let mut w=World::new();let owner=w.spawn_empty().id();let old=Box2{at:Vec2::new(50.,30.),size:Vec2::new(300.,20.)};let current=Box2{at:Vec2::new(50.,200.),size:Vec2::new(300.,60.)};
        r.tracks.insert(owner,Track{role:Role::Tooltip,shown:true,over:false,active:false,rect:current,base:Color::BLACK,output:Color::BLACK,opening:0.,optical:true,service:false});
        r.emit(Effect{owner,kind:Kind::Open,from:Vec2::ZERO,to:old.centre(),rect:old,old:None,age:Duration::ZERO,color:Color::WHITE});r.follow_layout();assert_eq!(r.effects[0].rect,current);
        r.effects[0].kind=Kind::Close;r.tracks.get_mut(&owner).unwrap().rect=old;r.follow_layout();assert_eq!(r.effects[0].rect,current);
    }
    #[test]fn hidden_service_controls_cannot_fill_the_visible_motion_registry(){
        let mut app=App::new();app.init_resource::<Registry>().add_systems(Update,attach);
        let root=app.world_mut().spawn(Node{display:Display::None,..default()}).id();
        for _ in 0..TRACKS+1 {app.world_mut().spawn((Button,ChildOf(root)));}
        let visible=app.world_mut().spawn(Button).id();app.update();
        assert!(app.world().get::<Tracked>(visible).is_some());
        let mut q=app.world_mut().query_filtered::<Entity,With<Tracked>>();assert_eq!(q.iter(app.world()).count(),1);
    }
    #[test]fn timings_have_no_decorative_command_delay(){
        assert!((0.06..=0.12).contains(&Kind::Hover.duration()));assert!((0.18..=0.32).contains(&Kind::Open.duration()));
        assert!((0.18..=0.32).contains(&Kind::Close.duration()));assert_eq!(panel::theme::HOVER_SECONDS,0.09);
    }
    #[test]fn completion_requires_a_new_real_exit_not_a_cancel_or_repeated_kind(){
        let old=ProductionSample{actor:3,head:Some(0),count:3,progress:70,exit:None};
        assert_eq!(production_change(old,ProductionSample{count:2,..old}),Some(Kind::Advance));
        let finished=ProductionSample{count:2,progress:0,exit:Some((12,40)),..old};
        assert_eq!(production_change(old,finished),Some(Kind::Complete));assert_eq!(production_change(finished,finished),None);
        assert_eq!(production_change(finished,ProductionSample{progress:20,..finished}),None);
        assert_eq!(production_change(finished,ProductionSample{exit:Some((12,99)),..finished}),None,"clearing the gate updates the same product marker");
        assert_eq!(production_change(finished,ProductionSample{head:None,count:0,exit:Some((13,110)),..finished}),Some(Kind::Complete));
    }
    #[test]fn final_completion_can_leave_the_hidden_queue_island(){
        let mut r=Registry::default();let mut w=World::new();let owner=w.spawn_empty().id();
        r.tracks.insert(owner,Track{role:Role::Panel,shown:false,over:false,active:false,rect:Box2{at:Vec2::new(100.,20.),size:Vec2::new(400.,80.)},base:Color::BLACK,output:Color::BLACK,opening:1.,optical:false,service:false});
        let old=ProductionSample{actor:2,head:Some(0),count:1,progress:95,exit:None};r.production.insert(2,old);
        r.queue(owner,ProductionSample{head:None,count:0,progress:0,exit:Some((8,20)),..old},Some(Vec2::new(600.,300.)));
        assert_eq!(r.completed,1);assert_eq!(r.effects.len(),1);assert_eq!(r.effects[0].to,Vec2::new(600.,300.));
        r.production.insert(3,ProductionSample{actor:3,..old});
        r.queue(owner,ProductionSample{actor:3,head:None,count:0,progress:0,exit:Some((9,20))},Some(Vec2::new(600.,500.)));
        assert_eq!(r.completed,2);assert_eq!(r.effects.len(),2,"simultaneous products keep distinct destinations");
    }
    #[test]fn progress_never_invents_work_and_resets_on_the_next_order(){
        let mut p=Progress::default();assert_eq!(p.step(0.02),0.);p.goal=50.;let first=p.step(0.02);assert!(first>0. && first<50.);
        for _ in 0..5{assert!(p.step(0.02)<=50.);}assert_eq!(p.value,50.);
        p.goal=0.;assert_eq!(p.step(0.02),0.);for _ in 0..20{assert_eq!(p.step(0.02),0.);}
    }
    #[test]
    fn reversing_one_owner_preserves_other_effects() {
        for (start, finish) in [(Kind::Open, Kind::Close), (Kind::Hover, Kind::Unhover)] {
            let mut registry = Registry::default();
            let mut world = World::new();
            let owner = world.spawn_empty().id();
            let other = world.spawn_empty().id();
            let effect = |owner, kind, age| Effect {
                owner, kind, from: Vec2::ZERO, to: Vec2::ONE,
                rect: Box2::default(), old: None, age, color: Color::WHITE,
            };
            registry.emit(effect(owner, start, Duration::from_millis(50)));
            // Use the same effect family on both owners: reversal must be local.
            registry.emit(effect(other, start, Duration::from_millis(40)));
            registry.emit(effect(owner, finish, Duration::ZERO));
            assert_eq!(registry.effects.len(), 2);
            let reversed = registry.effects.iter().find(|effect| effect.owner == owner).unwrap();
            assert_eq!(reversed.kind, finish);
            assert_eq!(reversed.age, Duration::ZERO);
            let untouched = registry.effects.iter().find(|effect| effect.owner == other).unwrap();
            assert_eq!(untouched.kind, start);
            assert_eq!(untouched.age, Duration::from_millis(40));
        }
    }
    #[test]fn effect_pool_is_bounded_and_settles_without_repeating_cycles(){
        let mut r=Registry::default();let mut world=World::new();
        for _ in 0..100{let owner=world.spawn_empty().id();r.emit(Effect{owner,kind:Kind::Open,from:Vec2::ZERO,to:Vec2::ONE,rect:Box2::default(),old:None,age:Duration::ZERO,color:Color::WHITE});assert!(r.effects.len()<=EFFECTS);}
        r.tick(Duration::from_millis(330));assert!(r.effects.is_empty());let count=r.emitted;for _ in 0..100{r.tick(Duration::from_millis(100));}assert_eq!(r.emitted,count);
    }
    #[test]fn native_segment_pool_has_no_input_or_text_nodes(){
        let mut app=App::new();let camera=app.world_mut().spawn_empty().id();build(&mut app.world_mut().commands(),camera);app.world_mut().flush();
        let mut pieces=app.world_mut().query::<(&Piece,Has<Button>,Has<Text>,&FocusPolicy,&LayoutConfig)>();let mut count=0;
        for (_,button,text,focus,layout) in pieces.iter(app.world()){count+=1;assert!(!button && !text);assert_eq!(*focus,FocusPolicy::Pass);assert!(!layout.use_rounding,"Fractional UI scales must preserve thin decorative segments");}assert_eq!(count,EFFECTS*PIECES);
    }
    #[test]fn reduced_effects_keep_native_text_geometry_and_state(){
        let mut app=App::new();app.init_resource::<Registry>().init_resource::<Time>().init_resource::<Settings>().add_systems(Update,(input,render).chain());
        let e=app.world_mut().spawn((Button,IntensityButton,Node{left:px(10),width:px(160),height:px(30),..default()},BackgroundColor(panel::theme::ACTIVE),
            Text::new("М108 / Э +68"),TextFont{font_size:FontSize::Px(14.),..default()},TextColor(Color::WHITE))).id();
        *app.world_mut().get_mut::<Interaction>(e).unwrap()=Interaction::Pressed;app.update();assert_eq!(app.world().resource::<Settings>().percent,35);
        let w=app.world();assert_eq!(w.get::<Text>(e).unwrap().0,"М108 / Э +68");assert_eq!(w.get::<Node>(e).unwrap().left,px(10));assert_eq!(w.get::<Node>(e).unwrap().width,px(160));
        assert_eq!(w.get::<TextFont>(e).unwrap().font_size,FontSize::Px(14.));assert_eq!(w.get::<BackgroundColor>(e).unwrap().0,panel::theme::ACTIVE);
    }
    #[test]fn all_motion_system_queries_initialize_without_aliasing(){
        use bevy::ecs::system::{IntoSystem,System};let mut w=World::new();
        IntoSystem::into_system(attach).initialize(&mut w);IntoSystem::into_system(prepare).initialize(&mut w);
        IntoSystem::into_system(production).initialize(&mut w);IntoSystem::into_system(render).initialize(&mut w);IntoSystem::into_system(input).initialize(&mut w);
    }
}
