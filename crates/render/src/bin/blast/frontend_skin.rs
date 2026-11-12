//! Optical command deck: one baked backdrop, live native controls, no world work.
use super::*;
use bevy::asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::{FontWeight, LetterSpacing};

const INK: Color = Color::srgb(0.88, 0.93, 0.93);
const DIM: Color = Color::srgb(0.43, 0.59, 0.63);
const FRAME: Color = Color::srgba(0.51, 0.65, 0.68, 0.38);
const LIGHT: Color = Color::srgb(0.91, 0.93, 0.74);
const CLEAR: Color = Color::srgba(0.0, 0.0, 0.0, 0.0);

pub(super) const ITEMS: [(Action, &str, &str); 8] = [
    (Action::Resume, "ПРОДОЛЖИТЬ", "Вернуться к текущей сессии. Мир ждёт вашего приказа."),
    (Action::Start(Destination::Battle), "СХВАТКА", "Настройте сражение и начните бой с командиром."),
    (Action::Start(Destination::Range), "ПОЛИГОН", "Техника, оружие и готовые боевые испытания. Свободное управление."),
    (Action::Start(Destination::Laboratory), "ЛАБОРАТОРИЯ", "Материалы, физика и разрушения. Собирайте и записывайте свои эксперименты."),
    (Action::Settings, "НАСТРОЙКИ", "Изображение, управление и параметры отображения."),
    (Action::Help, "КАК ИГРАТЬ", "Управление, режимы и инструменты командования."),
    (Action::Exit, "ВЫХОД", "Завершить работу и выйти из игры."),
    (Action::Start(Destination::Spectate), "НАБЛЮДАТЬ / ИИ × ИИ", "Наблюдайте за боем двух армий под управлением ИИ."),
];

pub(super) fn step_focus(index: usize, direction: i32, session: bool) -> usize {
    let first = usize::from(!session);
    let count = ITEMS.len() - first;
    first + ((index.max(first) - first) as i32 + direction).rem_euclid(count as i32) as usize
}

pub(super) struct DeckAssets { font: TextFont, background: Handle<Image>, cross: Handle<Image>, arrow: Handle<Image> }
impl DeckAssets {
    pub(super) fn new(fonts: &mut Assets<Font>, images: &mut Assets<Image>) -> Self {
        let font = fonts.add(Font::from_bytes(include_bytes!("../../../../../data/ui/menu/Oswald.ttf").to_vec()));
        let background = images.add(Image::new(Extent3d {width:1672,height:941,depth_or_array_layers:1},
            TextureDimension::D2,include_bytes!("../../../../../data/ui/menu/perimeter.rgba").to_vec(),
            TextureFormat::Rgba8UnormSrgb,RenderAssetUsages::RENDER_WORLD));
        let icon=|bytes:&[u8]|Image::new(Extent3d{width:40,height:40,depth_or_array_layers:1},
            TextureDimension::D2,bytes.to_vec(),TextureFormat::Rgba8UnormSrgb,RenderAssetUsages::RENDER_WORLD);
        let cross=images.add(icon(include_bytes!("../../../../../data/ui/icons/plus.rgba")));
        let arrow=images.add(icon(include_bytes!("../../../../../data/ui/icons/chevron-right.rgba")));
        Self {font:TextFont{font:font.into(),weight:FontWeight::LIGHT,..default()},background,cross,arrow}
    }
}

#[derive(Component)] pub struct Backdrop;
#[derive(Component)] pub struct HomeRow(usize);
#[derive(Component)] pub struct HomeLabel(usize);
#[derive(Component)] pub struct HomeMarker(usize);
#[derive(Component)] pub struct Description;
#[derive(Component)] pub struct HomeArrow(usize);

fn label(p: &mut ChildSpawnerCommands, f: &TextFont, value: &str, size: f32, spacing: f32, color: Color) -> Entity {
    p.spawn((Text::new(value),TextFont{font_size:FontSize::Px(size),..f.clone()},
        LetterSpacing::Px(spacing),TextColor(color))).id()
}
fn positioned(left:f32, top:f32) -> Node {
    Node{position_type:PositionType::Absolute,left:px(left),top:px(top),..default()}
}

pub(super) fn backdrop(p: &mut ChildSpawnerCommands, assets: &DeckAssets) {
    p.spawn((Node{position_type:PositionType::Absolute,left:px(42),right:px(42),top:px(170),bottom:px(70),
        overflow:Overflow::clip(),..default()},Pickable::IGNORE)).with_children(|p|{
            p.spawn((Backdrop,ImageNode{image:assets.background.clone(),color:Color::srgb(0.68,0.78,0.82),..default()},
                Node{position_type:PositionType::Absolute,width:percent(100),height:percent(100),..default()},Pickable::IGNORE));
        });
}

fn row(p:&mut ChildSpawnerCommands,a:&DeckAssets,index:usize,primary:bool) {
    let (action,name,_)=ITEMS[index];
    p.spawn((Button,action,HomeRow(index),Node{
        width:if primary {px(580)}else{Val::Auto},height:px(if primary {62.}else{42.}),
        align_items:AlignItems::Center,padding:UiRect{left:px(if primary {48.}else{24.}),right:px(22.),..default()},
        ..default()},BackgroundColor(CLEAR),BackgroundGradient::default())).with_children(|p|{
            p.spawn((HomeMarker(index),Node{position_type:PositionType::Absolute,left:px(0),top:px(7),bottom:px(7),width:px(3),..default()},BackgroundColor(CLEAR)));
            p.spawn((HomeArrow(index),ImageNode{image:a.arrow.clone(),color:CLEAR,..default()},Node{
                position_type:PositionType::Absolute,left:px(if primary {15.}else{5.}),width:px(if primary {16.}else{12.}),height:px(if primary {16.}else{12.}),..default()},Pickable::IGNORE));
            let e=label(p,&TextFont{weight:FontWeight::NORMAL,..a.font.clone()},name,
                if primary {32.}else{17.},if primary {4.2}else{2.},INK);
            p.commands().entity(e).insert(HomeLabel(index));
        });
}

pub(super) fn home(p: &mut ChildSpawnerCommands, a: &DeckAssets, fallback: &TextFont) {
    let f=&a.font;
    p.spawn((Part(Page::Home),Node{position_type:PositionType::Absolute,left:px(42),right:px(42),
        top:px(170),bottom:px(70),border:UiRect::all(px(1)),..default()},BorderColor::all(FRAME)))
    .with_children(|p| {
        // Native frame affordances and shared icon atlas marks, not baked text.
        for (right,bottom) in [(false,false),(true,false),(false,true),(true,true)] {
            p.spawn((Node{position_type:PositionType::Absolute,
                left:if right {Val::Auto}else{px(-1.)},right:if right {px(-1.)}else{Val::Auto},
                top:if bottom {Val::Auto}else{px(-1.)},bottom:if bottom {px(-1.)}else{Val::Auto},
                width:px(24),height:px(24),border:UiRect{
                    left:px(if right {0.}else{2.}),right:px(if right {2.}else{0.}),
                    top:px(if bottom {0.}else{2.}),bottom:px(if bottom {2.}else{0.})},..default()},
                BorderColor::all(Color::srgba(0.48,0.73,0.77,0.7)),Pickable::IGNORE));
            p.spawn((ImageNode{image:a.cross.clone(),color:Color::srgba(0.73,0.84,0.85,0.6),..default()},Node{
                position_type:PositionType::Absolute,
                left:if right {Val::Auto}else{px(26.)},right:if right {px(26.)}else{Val::Auto},
                top:if bottom {Val::Auto}else{px(24.)},bottom:if bottom {px(24.)}else{Val::Auto},
                width:px(16),height:px(16),..default()},Pickable::IGNORE));
        }
        for i in 0..14 {
            p.spawn((Node{position_type:PositionType::Absolute,left:px(49),top:px(117.+i as f32*36.),
                width:px(if i%3==0 {9.}else{4.}),height:px(1),..default()},
                BackgroundColor(Color::srgba(0.43,0.64,0.69,0.45)),Pickable::IGNORE));
        }
        p.spawn(positioned(78.,24.)).with_children(|p|{label(p,f,"ГЛАВНОЕ МЕНЮ",30.,3.,INK);});
        p.spawn((Node{position_type:PositionType::Absolute,left:px(110),top:px(104),
            padding:UiRect::left(px(32)),border:UiRect::left(px(2)),flex_direction:FlexDirection::Column,row_gap:px(3),..default()},BorderColor::all(DIM)))
        .with_children(|p|{
            label(p,f,"ОПЕРАЦИОННЫЙ РАЙОН",14.,3.2,DIM);
            label(p,&TextFont{weight:FontWeight::EXTRA_LIGHT,..f.clone()},"С–07",60.,8.,INK);
            label(p,f,"ЛЕСНОЙ ПОЛИГОН  /  СЕКТОР ДОСТУПЕН",13.,2.2,DIM);
        });
        p.spawn(Node{position_type:PositionType::Absolute,left:px(112),top:px(265),
            flex_direction:FlexDirection::Column,row_gap:px(4),..default()}).with_children(|p|{
                for i in 0..5 {row(p,a,i,true);}
            });
        p.spawn(Node{position_type:PositionType::Absolute,left:px(160),top:px(626),width:px(590),..default()})
            .with_children(|p|{let e=label(p,f,"",18.,0.8,DIM);p.commands().entity(e).insert(Description);});
        p.spawn((Notice,Text::new(""),TextFont{font_size:FontSize::Px(15.),..fallback.clone()},
            TextColor(LIGHT),Node{position_type:PositionType::Absolute,left:px(160),bottom:px(132),max_width:px(680),..default()}));
        p.spawn(Node{position_type:PositionType::Absolute,left:px(78),bottom:px(38),
            flex_direction:FlexDirection::Column,row_gap:px(10),..default()}).with_children(|p|{
                label(p,f,"ИНТЕРФЕЙС ГОТОВ",13.,2.5,DIM);
                label(p,f,"СТРЕЛКИ — ВЫБОР     ENTER — ОТКРЫТЬ     F10 — ВЕРНУТЬСЯ",12.,1.4,INK);
            });
        p.spawn(Node{position_type:PositionType::Absolute,right:px(64),bottom:px(30),
            align_items:AlignItems::Center,column_gap:px(20),..default()}).with_children(|p|{
                row(p,a,5,false);row(p,a,6,false);row(p,a,7,false);
            });
    });
    p.spawn((Part(Page::Home),Node{position_type:PositionType::Absolute,left:px(78),top:px(14),
        align_items:AlignItems::Center,column_gap:px(42),..default()})).with_children(|p|{
            label(p,&TextFont{weight:FontWeight::EXTRA_LIGHT,..f.clone()},"ПЕРИМЕТР",92.,24.,INK);
            p.spawn((Node{height:px(90),width:px(1),..default()},BackgroundColor(FRAME)));
            p.spawn(Node{flex_direction:FlexDirection::Column,row_gap:px(9),..default()}).with_children(|p|{
                label(p,f,"СТРАТЕГИЯ ПРОСТРАНСТВЕННОГО КОНТРОЛЯ",14.,3.,DIM);
                label(p,f,"ОПТИЧЕСКИЙ СЛОЙ",30.,7.,INK);
            });
        });
    p.spawn((Part(Page::Home),Node{position_type:PositionType::Absolute,left:px(80),bottom:px(23),right:px(80),
        justify_content:JustifyContent::SpaceBetween,..default()})).with_children(|p|{
            label(p,f,"RTS VOX  /  КУБИЧЕСКИЙ МИР",12.,2.6,DIM);
            label(p,f,"ПОЛИГОН   ·   СРАЖЕНИЕ   ·   ЛАБОРАТОРИЯ",12.,2.,DIM);
        });
}

pub fn sync(menu:Res<Menu>,scale:Res<UiScale>,windows:Query<&Window>,
    mut background:Query<&mut Node,With<Backdrop>>,
    mut rows:Query<(&HomeRow,&Interaction,&mut BackgroundColor,&mut BackgroundGradient),Without<HomeMarker>>,
    mut markers:Query<(&HomeMarker,&mut BackgroundColor),Without<HomeRow>>,
    mut arrows:Query<(&HomeArrow,&mut ImageNode)>,
    mut labels:Query<(&HomeLabel,&mut TextColor)>,mut description:Query<&mut Text,With<Description>>) {
    if !menu.shown(){return;}
    let viewport=windows.single().map_or(Vec2::new(1920.,1080.),|w|Vec2::new(w.width(),w.height()))/scale.0;
    let framed=Vec2::new((viewport.x-84.).max(1.),(viewport.y-240.).max(1.));
    let width=framed.x.max(framed.y*1672./941.);let height=width*941./1672.;
    for mut n in &mut background {n.width=px(width);n.height=px(height);n.left=px((framed.x-width)*0.5);n.top=px((framed.y-height)*0.5);}
    for (row,interaction,mut color,mut gradient) in &mut rows {
        let selected=row.0==menu.focus && (row.0>0 || menu.has_session);
        color.0=if selected && *interaction==Interaction::Pressed {Color::srgba(0.72,0.82,0.81,0.07)}else{CLEAR};
        if selected && gradient.0.is_empty() {
            *gradient=BackgroundGradient::from(LinearGradient {
                angle:std::f32::consts::FRAC_PI_2,
                stops:vec![ColorStop::percent(Color::srgba(0.74,0.84,0.81,0.13),0.),
                    ColorStop::percent(Color::srgba(0.56,0.73,0.73,0.025),45.),ColorStop::percent(CLEAR,100.)],
                ..default()
            });
        } else if !selected && !gradient.0.is_empty() {gradient.0.clear();}
    }
    for (marker,mut color) in &mut markers {color.0=if marker.0==menu.focus {LIGHT}else{CLEAR};}
    for (arrow,mut image) in &mut arrows {image.color=if arrow.0==menu.focus {LIGHT}else{CLEAR};}
    for (label,mut color) in &mut labels {color.0=if label.0==0&&!menu.has_session {Color::srgb(0.25,0.33,0.35)}else{INK};}
    for mut text in &mut description {text.0=ITEMS[menu.focus].2.into();}
}
