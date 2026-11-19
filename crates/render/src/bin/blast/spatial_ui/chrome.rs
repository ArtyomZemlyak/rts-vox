//! Pooled optical chrome. Decoration is fixed-size and never owns gameplay input.
use super::*;

/// Restyle the actual cached model render, preserving its silhouette and facets.
/// This runs once per catalogue image, never in the frame or simulation loop.
pub(super) fn hologram_thumbnail(mut image: Image) -> Image {
    let width = image.texture_descriptor.size.width as usize;
    let height = image.texture_descriptor.size.height as usize;
    let Some(data) = image.data.as_mut() else { return image };
    if width == 0 || height == 0 || data.len() != width * height * 4 { return image }
    let source = data.clone();
    let occupied: Vec<_> = source.chunks_exact(4).map(|p| p[3] != 0).collect();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if !occupied[i] { data[i * 4 + 3] = 0; continue }
            let edge = [(x.wrapping_sub(1),y),(x+1,y),(x,y.wrapping_sub(1)),(x,y+1)]
                .into_iter().any(|(nx,ny)| nx >= width || ny >= height || !occupied[ny*width+nx]);
            let l=(u16::from(source[i*4])+u16::from(source[i*4+1])+u16::from(source[i*4+2]))/3;
            let rgba = if edge { [180, 217, 236, source[i*4+3]] }
                else { [(l*3/5) as u8,(l*4/5) as u8,l as u8,source[i*4+3]] };
            data[i*4..i*4+4].copy_from_slice(&rgba);
        }
    }
    image.sampler = bevy::image::ImageSampler::nearest();
    image
}

fn row() -> Node {
    Node { flex_shrink:0., column_gap: px(4), row_gap: px(4), align_items: AlignItems::Center,
        flex_wrap: FlexWrap::Wrap, ..default() }
}

fn corners(p: &mut ChildSpawnerCommands) {
    for i in 0..4 {
        p.spawn((Node {
            position_type: PositionType::Absolute,
            left: if i % 2 == 0 { px(-1) } else { Val::Auto },
            right: if i % 2 == 1 { px(-1) } else { Val::Auto },
            top: if i < 2 { px(-1) } else { Val::Auto },
            bottom: if i >= 2 { px(-1) } else { Val::Auto },
            width: px(8), height: px(8),
            border: UiRect { left: px(if i % 2 == 0 { 1. } else { 0. }),
                right: px(if i % 2 == 1 { 1. } else { 0. }),
                top: px(if i < 2 { 1. } else { 0. }),
                bottom: px(if i >= 2 { 1. } else { 0. }) },
            ..default()
        }, BorderColor::all(INK.with_alpha(0.85)), Pickable::IGNORE, FocusPolicy::Pass));
    }
}

fn command(p: &mut ChildSpawnerCommands, font: &TextFont, name: &str,
    icon: &Handle<Image>, part: Part, action: Option<Action>) {
    let mut button = p.spawn((part, Button, panel::theme::Style::Optical, Node {
        width:if matches!(part,Part::CatalogToggle){px(88)}else{Val::Auto},
        flex_shrink:0.,
        height: px(28), padding: UiRect::axes(px(6), px(3)), column_gap: px(4),
        align_items: AlignItems::Center, border: UiRect::bottom(px(1)), ..default()
    }, BackgroundColor(Color::NONE), BorderColor::all(FRAME)));
    if let Some(a) = action { button.insert(a); }
    if matches!(part, Part::CatalogToggle | Part::ToolsToggle) {
        // Closing a shelf must not leave an invisible building-placement tool armed.
        button.insert(construction::Action::DismissPlacement);
    }
    button.with_children(|p| {
        p.spawn((ImageNode { image: icon.clone(), color: INK, ..default() },
            Node { width: px(14), height: px(14), ..default() }, Pickable::IGNORE));
        let e = p.spawn((Text::new(name), TextFont { font_size: FontSize::Px(13.),
            ..font.clone() }, TextColor(INK), LetterSpacing::Px(0.3))).id();
        if matches!(part, Part::CatalogToggle) { p.commands().entity(e).insert(Caption::CatalogToggle); }
    });
}

pub(super) fn build(commands: &mut Commands, camera: Entity, _fallback: &TextFont,
    fonts: &mut Assets<Font>, images: &mut Assets<Image>) {
    let faces = panel::theme::Fonts::load(fonts);
    let font = faces.optical;
    let readout = faces.data;
    let icons = [
        include_bytes!("../../../../../../data/ui/icons/move.rgba").as_slice(),
        include_bytes!("../../../../../../data/ui/icons/crosshair.rgba").as_slice(),
        include_bytes!("../../../../../../data/ui/icons/pause.rgba").as_slice(),
        include_bytes!("../../../../../../data/ui/icons/shield.rgba").as_slice(),
        include_bytes!("../../../../../../data/ui/icons/camera.rgba").as_slice(),
        include_bytes!("../../../../../../data/ui/icons/box.rgba").as_slice(),
        include_bytes!("../../../../../../data/ui/icons/hammer.rgba").as_slice(),
        include_bytes!("../../../../../../data/ui/icons/zap.rgba").as_slice(),
        include_bytes!("../../../../../../data/ui/icons/settings-2.rgba").as_slice(),
    ].map(|data| images.add(Image::new(
        bevy::render::render_resource::Extent3d { width: 40, height: 40, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2, data.to_vec(),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD)));
    for i in 0..4 {
        commands.spawn((Part::Corner(i), Node {
            position_type: PositionType::Absolute, width: px(10), height: px(10), display: Display::None,
            border: UiRect { left: px(if i % 2 == 0 { 1. } else { 0. }),
                right: px(if i % 2 == 1 { 1. } else { 0. }),
                top: px(if i < 2 { 1. } else { 0. }), bottom: px(if i >= 2 { 1. } else { 0. }) },
            ..default()
        }, BorderColor::all(CYAN.with_alpha(0.8)), Pickable::IGNORE, FocusPolicy::Pass,
            UiTargetCamera(camera), GlobalZIndex(11)));
    }
    for layer in 0..3 {
        for edge in 0..4 {
            commands.spawn((Part::FrameEdge(layer,edge),Node{position_type:PositionType::Absolute,display:Display::None,..default()},
                UiTransform::default(),BackgroundColor(CYAN),Pickable::IGNORE,FocusPolicy::Pass,UiTargetCamera(camera),GlobalZIndex(11)));
            commands.spawn((Part::Vertex(layer,edge),Node{position_type:PositionType::Absolute,display:Display::None,..default()},
                BackgroundColor(CYAN),BoxShadow::new(CYAN.with_alpha(0.35),px(0),px(0),px(1),px(6)),
                Pickable::IGNORE,FocusPolicy::Pass,UiTargetCamera(camera),GlobalZIndex(13)));
        }
    }
    for i in 0..6 {
        for part in [Part::Ray(i),Part::Signal(i)] {
            commands.spawn((part,Node{position_type:PositionType::Absolute,display:Display::None,..default()},UiTransform::default(),
                BackgroundColor(CYAN),Pickable::IGNORE,FocusPolicy::Pass,UiTargetCamera(camera),GlobalZIndex(10)));
        }
        if i<4 {commands.spawn((Part::Spoke(i),Node{position_type:PositionType::Absolute,display:Display::None,..default()},
            UiTransform::default(),BackgroundColor(CYAN),Pickable::IGNORE,FocusPolicy::Pass,UiTargetCamera(camera),GlobalZIndex(11)));}
    }
    for part in [Part::Emitter(0),Part::Emitter(1),Part::SourceSpot] {
        commands.spawn((part,Node{position_type:PositionType::Absolute,display:Display::None,border_radius:BorderRadius::all(percent(50)),..default()},
            BackgroundColor(CYAN),BoxShadow::new(CYAN.with_alpha(0.45),px(0),px(0),px(3),px(18)),
            Pickable::IGNORE,FocusPolicy::Pass,UiTargetCamera(camera),GlobalZIndex(11)));
    }
    commands.spawn((Part::Restore,Button,panel::PanelRoot,panel::theme::Style::Optical,RelativeCursorPosition::default(),
        Node{position_type:PositionType::Absolute,width:px(140),height:px(28),display:Display::None,padding:UiRect::all(px(4)),..default()},
        BackgroundColor(GLASS),UiTargetCamera(camera),GlobalZIndex(12)))
        .with_children(|p|{p.spawn((Text::new("Проекция +"),TextFont{font_size:FontSize::Px(14.),..font.clone()},TextColor(INK)));});
    commands.spawn((Part::Deck, panel::PanelRoot, panel::ScrollPanel, ScrollPosition::default(), RelativeCursorPosition::default(),
        UiTargetCamera(camera), GlobalZIndex(12), UiTransform::default(), super::super::optical_ui::ProjectionEnvelope::default(),
        Node { position_type: PositionType::Absolute, width: px(WIDTH),
            padding: UiRect::all(px(10)), row_gap: px(5), flex_direction: FlexDirection::Column,
            display: Display::None, ..default() }, BackgroundColor(GLASS), FocusPolicy::Block,
        BoxShadow::new(CYAN.with_alpha(0.10), px(0), px(0), px(1), px(7))))
        .with_children(|p| {
            corners(p);
            p.spawn((Part::FoldToggle,Button,panel::theme::Style::Optical,Node{position_type:PositionType::Absolute,
                right:px(3),top:px(3),width:px(20),height:px(20),..default()},BackgroundColor(Color::NONE)))
                .with_children(|p|{p.spawn((Text::new("−"),TextFont{font_size:FontSize::Px(16.),..font.clone()},TextColor(INK)));});
            // Sparse phosphor ticks: one fixed pool, no fullscreen pass or particle system.
            p.spawn((Node { position_type: PositionType::Absolute, left: px(10), right: px(10), top: px(0),
                column_gap: px(4), ..default() }, Pickable::IGNORE, FocusPolicy::Pass)).with_children(|p| {
                for i in 0..36 {
                    p.spawn((Node { width: px(2), height: px(1), ..default() },
                        BackgroundColor(CYAN.with_alpha(if i % 5 == 0 { 0.6 } else { 0.20 })),
                        Pickable::IGNORE, FocusPolicy::Pass));
                }
            });
            p.spawn((Part::Scan, Node { position_type: PositionType::Absolute, left: px(1), right: px(1),
                height: px(1), ..default() }, BackgroundColor(CYAN.with_alpha(0.06)),
                Pickable::IGNORE, FocusPolicy::Pass));
            label(p, &readout, Caption::Title, 23., INK);
            label(p, &readout, Caption::Status, 17., DIM);
            p.spawn(Node { column_gap: px(3), width: percent(100), height: px(3), ..default() })
                .with_children(|p| {
                    for i in 0..20 {
                        p.spawn((Part::Pip(i), Node { flex_grow: 1., height: px(3), ..default() }, BackgroundColor(CYAN)));
                    }
                });
            p.spawn(row()).with_children(|p| {
                for (name, action, icon) in [
                    ("Стоп", Action::Stop, 2), ("Атака", Action::Fire, 1),
                    ("Авто", Action::AutoAttack, 1), ("Радиус", Action::Ranges, 3),
                    ("1-е лицо", Action::FirstPerson, 4),
                    ("Ехать", Action::CrawlerMode(CrawlerMode::Move), 0),
                    ("Сбор", Action::CrawlerMode(CrawlerMode::Reclaim), 5),
                    ("Бурить", Action::CrawlerMode(CrawlerMode::Drill), 6),
                    ("Разгрузка", Action::CrawlerUnload, 5),
                    ("Саппорт", Action::ElectricSupport, 7), ("Пуск", Action::NuclearLaunch, 1),
                    ("Улучшить", Action::UpgradeKil, 6), ("Дроны", Action::UpgradeCrawler, 6),
                    ("По цели", Action::BattleWeapon(None), 1),
                    ("Фугас", Action::BattleWeapon(Some(BattleWeapon::Blast)), 1),
                    ("ББ", Action::BattleWeapon(Some(BattleWeapon::Kinetic)), 1),
                    ("Энергия", Action::BattleWeapon(Some(BattleWeapon::Energy)), 7),
                ] { command(p, &font, name, &icons[icon], Part::Command(action), Some(action)); }
                command(p, &font, "Ещё", &icons[8], Part::ToolsToggle, None);
                command(p, &font, "Строить", &icons[6], Part::CatalogToggle, None);
            });
        });
    // A separate pooled island: opening it never reflows command hit targets.
    commands.spawn((Part::CatalogShelf, panel::LayoutOnly, Pickable::IGNORE, FocusPolicy::Pass,
        UiTargetCamera(camera), GlobalZIndex(12), UiTransform::default(),
        Node{position_type:PositionType::Absolute,width:px(WIDTH),height:px(SHELF_HEIGHT),
            column_gap:px(8),row_gap:px(8),flex_wrap:FlexWrap::Wrap,display:Display::None,..default()}))
        .with_children(|p| {
            for slot in 0..4 {
                p.spawn((Part::Recipe(slot),Button,panel::PanelRoot,RelativeCursorPosition::default(),
                    construction::Action::Choose(slot),panel::theme::Style::Optical,optical_ui::Finish::optical_chip(),
                    Node{width:px(142),height:px(58),flex_grow:1.,flex_basis:px(132),
                        padding:UiRect::all(px(5)),column_gap:px(6),align_items:AlignItems::Center,
                        border:UiRect::bottom(px(1)),..default()},
                    BackgroundColor(GLASS.with_alpha(0.26)),BorderColor::all(FRAME)))
                    .with_children(|p| {
                        p.spawn((Visual::Recipe(slot),ImageNode::default(),Pickable::IGNORE,
                            Node{width:px(46),height:px(36),flex_shrink:0.,..default()}));
                        p.spawn((Caption::Name(slot),Text::new(""),TextFont{font_size:FontSize::Px(13.),..font.clone()},
                            TextColor(INK),Pickable::IGNORE));
                        p.spawn((Text::new(format!("{:02}",slot+1)),TextFont{font_size:FontSize::Px(11.),..readout.clone()},TextColor(DIM),
                            Node{position_type:PositionType::Absolute,right:px(5),top:px(3),..default()},Pickable::IGNORE,FocusPolicy::Pass));
                    });
            }
        });
    // A tooltip is an island, outside the command deck's scroll/clipping viewport.
    commands.spawn((Part::Tooltip,ui_motion::Target{role:ui_motion::Role::Tooltip,group:8,active:None},
        panel::PanelRoot,RelativeCursorPosition::default(),UiTargetCamera(camera),GlobalZIndex(35),UiTransform::default(),
        Node{position_type:PositionType::Absolute,width:px(WIDTH),padding:UiRect::all(px(9)),display:Display::None,..default()},
        BackgroundColor(GLASS),FocusPolicy::Block)).with_children(|p|{corners(p);label(p,&font,Caption::Tooltip,13.,INK);});
}
