//! World-anchored control deck. One pooled panel; no simulation entities or HUD per unit.
use super::*;
use bevy::text::LetterSpacing;
use bevy::ui::{FocusPolicy, RelativeCursorPosition};
use panel::{Action, CrawlerMode};
use sim_core::blast::battle::Weapon as BattleWeapon;
use workbench::EditorCamera;

const WIDTH: f32 = 500.;
const INK: Color = Color::srgb(0.86, 0.94, 0.96);
const CYAN: Color = Color::srgb(0.35, 0.86, 1.0);
const AMBER: Color = Color::srgb(1., 0.77, 0.32);
const FRAME: Color = Color::srgba(0.48, 0.74, 0.81, 0.55);
const GLASS: Color = Color::srgba(0.018, 0.037, 0.048, 0.91);

#[derive(Resource)]
pub(super) struct State {
    pub enabled: bool,
    pub visible: bool,
    pinned: Option<(usize, Vec2)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            enabled: true,
            visible: false,
            pinned: None,
        }
    }
}
#[derive(Component)]
pub(super) struct Toggle;
#[derive(Component, Clone, Copy)]
pub(super) enum Part {
    Deck,
    Production,
    Recipe(usize),
    Queue(usize),
    Command(Action),
    Corner(usize),
    Leader,
    Filter,
    Pip(usize),
    QueuePip(usize),
    Greben,
}
#[derive(Component)]
pub(super) enum Caption {
    Title,
    Status,
    Heading,
    Hint,
    Name(usize),
    Cost(usize),
    Queue,
    Toggle,
}
#[derive(Component)]
pub(super) enum Visual {
    Recipe(usize),
    Queue(usize),
}

pub fn input(
    mut state: ResMut<State>,
    buttons: Query<&Interaction, (With<Toggle>, Changed<Interaction>)>,
    keys: Res<ButtonInput<KeyCode>>,
    menu: Res<frontend::Menu>,
    lab: Res<laboratory::Laboratory>,
    pilot: Res<pilot::Pilot>,
) {
    if menu.shown() || lab.active() || pilot.active() {
        return;
    }
    if keys.just_pressed(KeyCode::F8) || buttons.iter().any(|i| *i == Interaction::Pressed) {
        state.enabled = !state.enabled;
        state.pinned = None;
    }
}
pub fn entry(p: &mut ChildSpawnerCommands, font: &TextFont) {
    p.spawn((
        Toggle,
        Button,
        Node {
            height: px(30),
            padding: UiRect::axes(px(9), px(4)),
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(GLASS),
        BorderColor::all(FRAME),
    ))
    .with_children(|p| {
        p.spawn((
            Caption::Toggle,
            Text::new("У объекта · F8"),
            TextFont {
                font_size: FontSize::Px(13.),
                ..font.clone()
            },
            TextColor(INK),
        ));
    });
}
fn label(p: &mut ChildSpawnerCommands, font: &TextFont, caption: Caption, size: f32, color: Color) {
    p.spawn((
        caption,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(size),
            ..font.clone()
        },
        TextColor(color),
        LetterSpacing::Px(0.6),
    ));
}
fn catalog_button(
    p: &mut ChildSpawnerCommands,
    font: &TextFont,
    name: &str,
    action: construction::Action,
) {
    p.spawn((
        Part::Filter,
        Button,
        action,
        Node {
            height: px(28),
            padding: UiRect::axes(px(9), px(3)),
            border: UiRect::bottom(px(1)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        BorderColor::all(FRAME),
    ))
    .with_children(|p| {
        p.spawn((
            Text::new(name),
            TextFont {
                font_size: FontSize::Px(13.),
                ..font.clone()
            },
            TextColor(INK),
        ));
    });
}
pub fn build(
    commands: &mut Commands,
    camera: Entity,
    fallback: &TextFont,
    fonts: &mut Assets<Font>,
    images: &mut Assets<Image>,
) {
    let font = TextFont {
        font: fonts
            .add(Font::from_bytes(
                include_bytes!("../../../../../data/ui/menu/Oswald.ttf").to_vec(),
            ))
            .into(),
        ..fallback.clone()
    };
    let load = |data: &[u8]| {
        Image::new(
            bevy::render::render_resource::Extent3d {
                width: 40,
                height: 40,
                depth_or_array_layers: 1,
            },
            bevy::render::render_resource::TextureDimension::D2,
            data.to_vec(),
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
        )
    };
    let icons = [
        include_bytes!("../../../../../data/ui/icons/move.rgba").as_slice(),
        include_bytes!("../../../../../data/ui/icons/crosshair.rgba").as_slice(),
        include_bytes!("../../../../../data/ui/icons/pause.rgba").as_slice(),
        include_bytes!("../../../../../data/ui/icons/shield.rgba").as_slice(),
        include_bytes!("../../../../../data/ui/icons/camera.rgba").as_slice(),
        include_bytes!("../../../../../data/ui/icons/box.rgba").as_slice(),
        include_bytes!("../../../../../data/ui/icons/hammer.rgba").as_slice(),
        include_bytes!("../../../../../data/ui/icons/zap.rgba").as_slice(),
    ]
    .map(|b| images.add(load(b)));
    for i in 0..4 {
        commands.spawn((
            Part::Corner(i),
            Node {
                position_type: PositionType::Absolute,
                width: px(13),
                height: px(13),
                display: Display::None,
                border: UiRect {
                    left: px(if i % 2 == 0 { 2. } else { 0. }),
                    right: px(if i % 2 == 1 { 2. } else { 0. }),
                    top: px(if i < 2 { 2. } else { 0. }),
                    bottom: px(if i >= 2 { 2. } else { 0. }),
                },
                ..default()
            },
            BorderColor::all(CYAN),
            Pickable::IGNORE,
            FocusPolicy::Pass,
            UiTargetCamera(camera),
            GlobalZIndex(11),
        ));
    }
    commands.spawn((
        Part::Leader,
        Node {
            position_type: PositionType::Absolute,
            height: px(1),
            display: Display::None,
            ..default()
        },
        UiTransform::default(),
        BackgroundColor(FRAME),
        Pickable::IGNORE,
        FocusPolicy::Pass,
        UiTargetCamera(camera),
        GlobalZIndex(11),
    ));
    commands
        .spawn((
            Part::Deck,
            panel::PanelRoot,
            RelativeCursorPosition::default(),
            UiTargetCamera(camera),
            GlobalZIndex(12),
            UiTransform::default(),
            Node {
                position_type: PositionType::Absolute,
                width: px(WIDTH),
                padding: UiRect::all(px(14)),
                row_gap: px(8),
                flex_direction: FlexDirection::Column,
                border: UiRect::all(px(1)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(GLASS),
            BorderColor::all(FRAME),
            FocusPolicy::Block,
        ))
        .with_children(|p| {
            label(p, &font, Caption::Title, 21., INK);
            label(
                p,
                &font,
                Caption::Status,
                13.,
                Color::srgb(0.63, 0.77, 0.81),
            );
            p.spawn(Node {
                column_gap: px(3),
                height: px(4),
                ..default()
            })
            .with_children(|p| {
                for i in 0..20 {
                    p.spawn((
                        Part::Pip(i),
                        Node {
                            width: px(20),
                            height: px(4),
                            ..default()
                        },
                        BackgroundColor(CYAN),
                    ));
                }
            });
            p.spawn(Node {
                column_gap: px(5),
                row_gap: px(5),
                flex_wrap: FlexWrap::Wrap,
                ..default()
            })
            .with_children(|p| {
                for (name, action, icon) in [
                    ("Стоп", Action::Stop, 2),
                    ("Атаковать", Action::Fire, 1),
                    ("Авто", Action::AutoAttack, 1),
                    ("Радиусы", Action::Ranges, 3),
                    ("1-е лицо", Action::FirstPerson, 4),
                    ("Ехать", Action::CrawlerMode(CrawlerMode::Move), 0),
                    ("Сбор", Action::CrawlerMode(CrawlerMode::Reclaim), 5),
                    ("Бурить", Action::CrawlerMode(CrawlerMode::Drill), 6),
                    ("Разгрузить", Action::CrawlerUnload, 5),
                    ("Саппорт", Action::ElectricSupport, 7),
                    ("Пуск", Action::NuclearLaunch, 1),
                    ("Улучшить", Action::UpgradeKil, 6),
                    ("Дроны", Action::UpgradeCrawler, 6),
                    ("По цели", Action::BattleWeapon(None), 1),
                    ("Фугас", Action::BattleWeapon(Some(BattleWeapon::Blast)), 1),
                    ("ББ", Action::BattleWeapon(Some(BattleWeapon::Kinetic)), 1),
                    (
                        "Энергия",
                        Action::BattleWeapon(Some(BattleWeapon::Energy)),
                        7,
                    ),
                ] {
                    p.spawn((
                        Part::Command(action),
                        Button,
                        action,
                        Node {
                            height: px(34),
                            padding: UiRect::axes(px(7), px(4)),
                            column_gap: px(5),
                            align_items: AlignItems::Center,
                            border: UiRect::bottom(px(1)),
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                        BorderColor::all(FRAME),
                    ))
                    .with_children(|p| {
                        p.spawn((
                            ImageNode {
                                image: icons[icon].clone(),
                                color: INK,
                                ..default()
                            },
                            Node {
                                width: px(18),
                                height: px(18),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                        p.spawn((
                            Text::new(name),
                            TextFont {
                                font_size: FontSize::Px(13.),
                                ..font.clone()
                            },
                            TextColor(INK),
                        ));
                    });
                }
            });
            p.spawn((
                Part::Production,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(7),
                    ..default()
                },
            ))
            .with_children(|p| {
                label(p, &font, Caption::Heading, 14., INK);
                p.spawn(Node {
                    column_gap: px(3),
                    ..default()
                })
                .with_children(|p| {
                    for (t, n) in [(0, "Все"), (1, "T1"), (2, "T2"), (3, "T3"), (4, "T4")] {
                        catalog_button(p, &font, n, construction::Action::Tier(t));
                    }
                    catalog_button(p, &font, "‹", construction::Action::Page(-1));
                    catalog_button(p, &font, "›", construction::Action::Page(1));
                    catalog_button(p, &font, "Отмена", construction::Action::Cancel);
                    p.spawn((Part::Greben, Node::default())).with_children(|p| {
                        catalog_button(p, &font, "Режим", construction::Action::GrebenMode);
                    });
                });
                p.spawn(Node {
                    column_gap: px(7),
                    ..default()
                })
                .with_children(|p| {
                    for i in 0..4 {
                        p.spawn((
                            Part::Recipe(i),
                            Button,
                            construction::Action::Choose(i),
                            Node {
                                width: px(112),
                                height: px(125),
                                padding: UiRect::all(px(5)),
                                flex_direction: FlexDirection::Column,
                                align_items: AlignItems::Center,
                                border: UiRect::all(px(1)),
                                ..default()
                            },
                            BackgroundColor(Color::NONE),
                            BorderColor::all(FRAME),
                        ))
                        .with_children(|p| {
                            p.spawn((
                                Visual::Recipe(i),
                                ImageNode::default(),
                                Node {
                                    width: px(88),
                                    height: px(57),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                            label(p, &font, Caption::Name(i), 13., INK);
                            label(p, &font, Caption::Cost(i), 11., AMBER);
                        });
                    }
                });
                label(p, &font, Caption::Queue, 13., INK);
                p.spawn(Node {
                    column_gap: px(4),
                    ..default()
                })
                .with_children(|p| {
                    for i in 0..6 {
                        p.spawn((
                            Part::Queue(i),
                            Node {
                                width: px(48),
                                height: px(38),
                                border: UiRect::all(px(1)),
                                ..default()
                            },
                            BorderColor::all(FRAME),
                        ))
                        .with_children(|p| {
                            p.spawn((
                                Visual::Queue(i),
                                ImageNode::default(),
                                Node {
                                    width: px(46),
                                    height: px(36),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                        });
                    }
                });
                p.spawn(Node {
                    column_gap: px(2),
                    height: px(4),
                    ..default()
                })
                .with_children(|p| {
                    for i in 0..20 {
                        p.spawn((
                            Part::QueuePip(i),
                            Node {
                                width: px(21),
                                height: px(4),
                                ..default()
                            },
                            BackgroundColor(FRAME),
                        ));
                    }
                });
                label(p, &font, Caption::Hint, 12., Color::srgb(0.63, 0.77, 0.81));
            });
        });
}
/// Cache local occupied bounds once per template/scene, never scan voxels per frame.
#[derive(Default)]
pub(super) struct BoundsCache(std::collections::BTreeMap<(u64, usize), Vec<([i32; 3], [i32; 3])>>);
fn occupied_bounds(body: &voxel::unit::UnitBody) -> ([i32; 3], [i32; 3]) {
    let mut lo = [i32::MAX; 3];
    let mut hi = [0; 3];
    for z in 0..usize::from(body.vol.nz) {
        for x in 0..usize::from(body.vol.nx) {
            for y in 0..usize::from(body.vol.ny) {
                if body.vol.get(x, y, z) {
                    for (a, v) in [x, y, z].into_iter().enumerate() {
                        lo[a] = lo[a].min(v as i32);
                        hi[a] = hi[a].max(v as i32 + 1);
                    }
                }
            }
        }
    }
    if lo[0] == i32::MAX {
        ([0; 3], [1; 3])
    } else {
        (lo, hi)
    }
}
/// Try the open side first, then the opposite side, and keep controls on screen.
fn panel_position(anchor: Vec2, half: Vec2, viewport: Vec2, size: Vec2) -> Vec2 {
    let gap = 22.;
    let top = if viewport.y < 760. { 164. } else { 82. };
    let mut x = anchor.x + half.x + gap;
    if x + size.x > viewport.x - 16. {
        x = anchor.x - half.x - gap - size.x;
    }
    Vec2::new(
        x.clamp(16., (viewport.x - size.x - 16.).max(16.)),
        (anchor.y - size.y * 0.4).clamp(top, (viewport.y - size.y - 24.).max(top)),
    )
}
fn command_visible(action: Action, kind: RangeUnitKind, armed: bool, fighting: bool) -> bool {
    match action {
        Action::CrawlerMode(_) | Action::CrawlerUnload | Action::UpgradeCrawler => {
            kind == RangeUnitKind::Crawler
        }
        Action::ElectricSupport => kind == RangeUnitKind::LightningTank,
        Action::NuclearLaunch => kind == RangeUnitKind::NuclearSilo && fighting,
        Action::UpgradeKil => matches!(
            kind,
            RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3
        ),
        Action::Fire | Action::AutoAttack => {
            armed && kind != RangeUnitKind::NuclearSilo && fighting
        }
        Action::BattleWeapon(_) => matches!(kind, RangeUnitKind::Tank | RangeUnitKind::Artillery),
        _ => true,
    }
}
pub fn update(
    (mut state, scene, snapshot): (ResMut<State>, Res<Scene>, Res<ui_snapshot::UiSnapshot>),
    (edit, panel): (Res<Workshop>, Res<panel::Panel>),
    (catalog, menu, lab, pilot, scale): (
        Res<construction::Catalog>,
        Res<frontend::Menu>,
        Res<laboratory::Laboratory>,
        Res<pilot::Pilot>,
        Res<UiScale>,
    ),
    cameras: Query<(&Camera, &Transform), With<EditorCamera>>,
    roots: Query<&RelativeCursorPosition, With<Part>>,
    mut parts: Query<(
        &Part,
        &mut Node,
        Option<&mut UiTransform>,
        Option<&mut BackgroundColor>,
        Option<&mut BorderColor>,
        Option<&ComputedNode>,
        Option<&Interaction>,
    )>,
    mut legacy: Query<
        (
            &mut Node,
            Option<&panel::hud::CommandDeck>,
            Option<&construction::Root>,
            Option<&panel::hud::RosterStrip>,
            Option<&panel::minimap::MiniRoot>,
        ),
        (
            Or<(
                With<panel::hud::CommandDeck>,
                With<construction::Root>,
                With<panel::hud::RosterStrip>,
                With<panel::minimap::MiniRoot>,
            )>,
            Without<Part>,
        ),
    >,
    mut texts: Query<(&Caption, &mut Text)>,
    mut visuals: Query<(&Visual, &mut ImageNode)>,
    mut giz: Gizmos,
    mut bounds: Local<BoundsCache>,
) {
    bounds
        .0
        .retain(|(generation, _), _| *generation == scene.generation);
    let active = state.enabled
        && !menu.shown()
        && !lab.active()
        && !pilot.active()
        && !edit.editing
        && !panel.free_placement
        && scene.sim.battle_mode();
    let id = edit.controlled.filter(|&id| {
        scene
            .sim
            .units
            .get(id)
            .is_some_and(|u| u.status.alive && scene.sim.allied(id))
    });
    let projection = active.then_some(id).flatten().and_then(|id| {
        let u = &scene.sim.units[id];
        let (camera, transform) = cameras.single().ok()?;
        let viewport = camera.logical_viewport_size()? / scale.0;
        let gt = GlobalTransform::from(*transform);
        let boxes = bounds
            .0
            .entry((scene.generation, u.kind.index()))
            .or_insert_with(|| {
                std::iter::once(&u.body)
                    .chain(u.body.segments.iter().map(|s| &s.body))
                    .map(occupied_bounds)
                    .collect()
            });
        let mut screen_lo = Vec2::splat(f32::INFINITY);
        let mut screen_hi = Vec2::splat(f32::NEG_INFINITY);
        let mut world_lo = Vec3::splat(f32::INFINITY);
        let mut world_hi = Vec3::splat(f32::NEG_INFINITY);
        for (index, (lo, hi)) in boxes.iter().enumerate() {
            if index > 0 && u.seg_gone.get(index - 1).copied().unwrap_or(false) {
                continue;
            }
            let frame = if index == 0 {
                &u.pose
            } else {
                u.seg_frames.get(index - 1)?
            };
            for k in 0..8 {
                let local = std::array::from_fn(|a| {
                    fx::Fx::from_int(if k & (1 << a) == 0 { lo[a] } else { hi[a] })
                });
                let point = frame.world(local);
                let p = Vec3::new(
                    point.x.to_f64_lossy() as f32,
                    point.y.to_f64_lossy() as f32,
                    point.z.to_f64_lossy() as f32,
                );
                world_lo = world_lo.min(p);
                world_hi = world_hi.max(p);
                if let Ok(pixel) = camera.world_to_viewport(&gt, p) {
                    screen_lo = screen_lo.min(pixel / scale.0);
                    screen_hi = screen_hi.max(pixel / scale.0);
                }
            }
        }
        if !screen_lo.is_finite() || !screen_hi.is_finite() {
            return None;
        }
        let at = (screen_lo + screen_hi) * 0.5;
        if at.x < 0. || at.x > viewport.x || at.y < 70. || at.y > viewport.y {
            return None;
        }
        let half = ((screen_hi - screen_lo) * 0.5).max(Vec2::splat(18.));
        let centre = (world_lo + world_hi) * 0.5;
        Some((
            id,
            at,
            half,
            viewport,
            Vec3::new(centre.x, world_lo.y, centre.z),
            world_hi - world_lo,
        ))
    });
    state.visible = projection.is_some();
    let details = projection
        .as_ref()
        .and_then(|p| construction::spatial_catalog(&catalog, &scene.sim, scene.generation, p.0));
    let mut position = Vec2::ZERO;
    let mut panel_size = Vec2::new(WIDTH, 200.);
    let mut zoom = 1.;
    if let Some((id, at, half, viewport, _, _)) = projection {
        zoom = (viewport.y / 1080.).clamp(0.8, 2.);
        let has_catalog = details.is_some();
        // Layout size is from the previous finished UI pass; first-frame bound
        // reserves the maximum deck so appearing controls never leave the window.
        panel_size.y = parts
            .iter()
            .find_map(|(p, _, _, _, _, n, _)| {
                matches!(p, Part::Deck)
                    .then(|| n.map(|n| n.size().y * n.inverse_scale_factor()))
                    .flatten()
            })
            .filter(|h| *h > 50.)
            .unwrap_or(if has_catalog { 465. } else { 230. });
        let top = if viewport.y < 760. { 164. } else { 82. };
        zoom = zoom.min(((viewport.y - top - 24.) / panel_size.y).max(0.6));
        position = panel_position(at, half, viewport, panel_size * zoom);
        let hovered = roots.iter().any(|r| r.cursor_over());
        if hovered {
            if let Some((old_id, p)) = state.pinned {
                if old_id == id {
                    position = Vec2::new(
                        p.x.clamp(16., (viewport.x - WIDTH * zoom - 16.).max(16.)),
                        p.y.clamp(82., (viewport.y - panel_size.y * zoom - 24.).max(82.)),
                    );
                }
            }
        }
        state.pinned = Some((id, position));
    } else {
        state.pinned = None;
    }
    for (mut node, deck, build, strip, map) in &mut legacy {
        if projection.is_some() {
            if deck.is_some() || strip.is_some() || build.is_some() && details.is_some() {
                node.display = Display::None;
            }
            if map.is_some() {
                node.bottom = px(24);
            }
        }
    }
    let selected = projection
        .map(|p| p.0)
        .and_then(|id| snapshot.view.as_ref().and_then(|v| v.units.get(id)));
    let integrity = selected.map_or(0, |u| {
        u.integrity
            .core
            .mul(fx::Fx::from_int(100))
            .to_int_trunc()
            .clamp(0, 100) as usize
    });
    for (part, mut node, transform, mut background, border, _, interaction) in &mut parts {
        if let Some((id, at, half, _, base, size)) = projection {
            let unit = &scene.sim.units[id];
            match *part {
                Part::Deck => {
                    node.display = Display::Flex;
                    node.left = px(position.x + (zoom - 1.) * WIDTH * 0.5);
                    node.top = px(position.y + (zoom - 1.) * panel_size.y * 0.5);
                    if let Some(mut t) = transform {
                        t.scale = Vec2::splat(zoom);
                    }
                }
                Part::Corner(i) => {
                    node.display = Display::Flex;
                    node.left = px(at.x
                        + if i % 2 == 0 {
                            -half.x - 6.
                        } else {
                            half.x - 7.
                        });
                    node.top = px(at.y + if i < 2 { -half.y - 6. } else { half.y - 7. });
                }
                Part::Leader => {
                    let end = Vec2::new(
                        if position.x > at.x {
                            position.x
                        } else {
                            position.x + WIDTH * zoom
                        },
                        position.y + 26. * zoom,
                    );
                    let start = at
                        + Vec2::new(
                            if end.x > at.x {
                                half.x + 7.
                            } else {
                                -half.x - 7.
                            },
                            -half.y * 0.3,
                        );
                    let delta = end - start;
                    let mid = (start + end) * 0.5;
                    node.display = Display::Flex;
                    node.width = px(delta.length());
                    node.left = px(mid.x - delta.length() * 0.5);
                    node.top = px(mid.y);
                    if let Some(mut t) = transform {
                        t.rotation = Rot2::radians(delta.y.atan2(delta.x));
                    }
                }
                Part::Filter => {}
                Part::Production => {
                    node.display = if details.is_some() {
                        Display::Flex
                    } else {
                        Display::None
                    }
                }
                Part::Greben => {
                    node.display = if details.as_ref().is_some_and(|d| d.greben) {
                        Display::Flex
                    } else {
                        Display::None
                    }
                }
                Part::Recipe(i) => {
                    node.display = if details.as_ref().is_some_and(|d| i < d.recipes.len()) {
                        Display::Flex
                    } else {
                        Display::None
                    }
                }
                Part::Queue(i) => {
                    node.display = if details.as_ref().is_some_and(|d| i < d.queue.len()) {
                        Display::Flex
                    } else {
                        Display::None
                    }
                }
                Part::Command(a) => {
                    node.display = if command_visible(
                        a,
                        unit.kind,
                        unit.status.weapon,
                        scene.sim.battle.phase == sim_core::blast::battle::Phase::Fighting,
                    ) {
                        Display::Flex
                    } else {
                        Display::None
                    }
                }
                Part::Pip(i) => {
                    if let Some(ref mut bg) = background {
                        bg.0 = if i * 5 < integrity { CYAN } else { FRAME };
                    }
                }
                Part::QueuePip(i) => {
                    if let Some(ref mut bg) = background {
                        bg.0 = if details
                            .as_ref()
                            .is_some_and(|d| (i as u64) * 5 < d.progress)
                        {
                            AMBER
                        } else {
                            FRAME
                        };
                    }
                }
            }
            if matches!(part, Part::Recipe(_) | Part::Command(_) | Part::Filter) {
                let active = background
                    .as_ref()
                    .is_some_and(|b| b.0 == panel::theme::ACTIVE)
                    || matches!(part,Part::Command(Action::CrawlerMode(m)) if *m==panel.crawler_mode)
                    || matches!(part,Part::Command(Action::Ranges) if panel.ranges)
                    || matches!(part,Part::Command(Action::Fire|Action::NuclearLaunch) if panel.ground_fire);
                let hover = interaction.is_some_and(|i| *i != Interaction::None);
                if let Some(mut b) = border {
                    *b = BorderColor::all(if hover || active { AMBER } else { FRAME });
                }
                if let Some(ref mut b) = background {
                    b.0 = if hover || active {
                        Color::srgba(0.27, 0.34, 0.32, 0.55)
                    } else {
                        Color::NONE
                    };
                }
            }
            // Drawing is emitted once, not once per UI entity.
            if matches!(part, Part::Deck) {
                let y = base.y + 1.;
                let extent = Vec2::new(size.x, size.z) * 0.55;
                let leg = extent.min_element() * 0.35;
                for x in [-1., 1.] {
                    for z in [-1., 1.] {
                        let c = Vec3::new(base.x + x * extent.x, y, base.z + z * extent.y);
                        giz.line(c, c - Vec3::X * x * leg, CYAN);
                        giz.line(c, c - Vec3::Z * z * leg, CYAN);
                        giz.line(c, c + Vec3::Y * 5., CYAN);
                    }
                }
            }
        } else if matches!(part, Part::Deck | Part::Corner(_) | Part::Leader) {
            node.display = Display::None;
        }
    }
    for (caption, mut text) in &mut texts {
        let value = match caption {
            Caption::Toggle => {
                if state.enabled {
                    "У объекта · F8".into()
                } else {
                    "Панель HUD · F8".into()
                }
            }
            Caption::Title => projection.map_or(String::new(), |(id, _, _, _, _, _)| {
                format!(
                    "{} · {:02} / T{}",
                    construction::name(scene.sim.units[id].kind).to_uppercase(),
                    id + 1,
                    construction::tier(scene.sim.units[id].kind)
                )
            }),
            Caption::Status => projection.map_or(String::new(), |(id, _, _, _, _, _)| {
                let group = edit.battle_selected_ids(&scene.sim).len();
                let cargo = snapshot
                    .crawler_cargo
                    .get(id)
                    .copied()
                    .flatten()
                    .map(|(held, cap, _, _, _, _, mode)| {
                        format!(
                            "М {} / {} · {mode}",
                            economy_display::mass(held),
                            economy_display::mass(cap)
                        )
                    })
                    .or_else(|| {
                        snapshot
                            .base_cargo
                            .get(id)
                            .copied()
                            .flatten()
                            .map(|(held, cap)| {
                                format!(
                                    "М {} / {}",
                                    economy_display::mass(held),
                                    economy_display::mass(cap)
                                )
                            })
                    })
                    .unwrap_or_else(|| format!("Ядро {integrity}% · выбрано {group}"));
                let power = snapshot
                    .energy_local
                    .get(id)
                    .copied()
                    .flatten()
                    .map(|(have, need)| {
                        format!(
                            " · лок. сеть +{} / −{}",
                            economy_display::energy(have),
                            economy_display::energy(need)
                        )
                    })
                    .unwrap_or_default();
                format!("{cargo}{power}")
            }),
            Caption::Heading => details
                .as_ref()
                .map_or(String::new(), |d| d.heading.clone()),
            Caption::Hint => {
                if !scene.notice.is_empty() {
                    scene.notice.chars().take(140).collect()
                } else {
                    details.as_ref().map_or(String::new(), |d| d.hint.clone())
                }
            }
            Caption::Name(i) => details
                .as_ref()
                .and_then(|d| d.recipes.get(*i))
                .map_or(String::new(), |r| construction::name(r.kind).into()),
            Caption::Cost(i) => details
                .as_ref()
                .and_then(|d| d.recipes.get(*i))
                .map_or(String::new(), |r| r.cost.clone()),
            Caption::Queue => details.as_ref().map_or(String::new(), |d| {
                if d.queue.is_empty() {
                    "ОЧЕРЕДЬ · пусто".into()
                } else {
                    format!(
                        "СБОРКА {}% · {} заказов · {}{}",
                        d.progress,
                        d.queue.len(),
                        construction::name(d.queue[0].0),
                        if d.queue.len() > 6 {
                            " · далее…"
                        } else {
                            ""
                        }
                    )
                }
            }),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    for (visual, mut image) in &mut visuals {
        image.image = match visual {
            Visual::Recipe(i) => details
                .as_ref()
                .and_then(|d| d.recipes.get(*i))
                .map(|r| r.image.clone()),
            Visual::Queue(i) => details
                .as_ref()
                .and_then(|d| d.queue.get(*i))
                .map(|q| q.1.clone()),
        }
        .unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spatial_panel_uses_opposite_side_and_stays_inside_viewport() {
        let viewport = Vec2::new(1920., 1080.);
        let size = Vec2::new(WIDTH, 460.);
        let left = panel_position(Vec2::new(300., 500.), Vec2::splat(40.), viewport, size);
        assert!(left.x > 340.);
        let right = panel_position(Vec2::new(1700., 950.), Vec2::splat(40.), viewport, size);
        assert!(right.x + size.x < 1660.);
        assert!(right.y + size.y <= 1056.);
        let small = panel_position(
            Vec2::new(400., 350.),
            Vec2::splat(80.),
            Vec2::new(960., 540.),
            Vec2::new(WIDTH, 320.),
        );
        assert!(small.x >= 16. && small.x + size.x <= 944.);
    }
    #[test]
    fn spatial_mode_toggle_preserves_selection_and_ignores_main_menu() {
        let mut app = App::new();
        app.init_resource::<State>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(frontend::Menu::new(false))
            .insert_resource(laboratory::Laboratory::new(false))
            .init_resource::<pilot::Pilot>()
            .add_systems(Update, input);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F8);
        app.update();
        assert!(!app.world().resource::<State>().enabled);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F8);
        app.update();
        assert!(app.world().resource::<State>().enabled);
        app.insert_resource(frontend::Menu::new(true));
        app.update();
        assert!(app.world().resource::<State>().enabled);
    }
    #[test]
    fn spatial_recipe_buttons_use_the_selected_factory_and_block_map_input() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../data/fixture-drive.ron")).unwrap();
        layout.size = [1536, 256, 1536];
        layout.gun = [64, 16, 64];
        layout.target = [1400, 16, 1400];
        layout.clear_min = [1, 1];
        layout.clear_max = [1535, 1535];
        layout.idle_enemies = true;
        layout.units = vec![
            modding::range::RangeUnitDef {
                kind: RangeUnitKind::KilFactoryT1,
                allied: true,
                at: [550, 16, 550],
                weapon: None,
            },
            modding::range::RangeUnitDef {
                kind: RangeUnitKind::Tank,
                allied: false,
                at: [1400, 16, 1400],
                weapon: None,
            },
        ];
        layout.blocks = vec![
            modding::range::RangeBlock {
                min: [0; 3],
                max: [1536, 256, 1536],
                material: 0,
            },
            modding::range::RangeBlock {
                min: [0; 3],
                max: [1536, 16, 1536],
                material: 1,
            },
        ];
        layout.economy = Some(modding::range_economy::RangeEconomy::stock());
        layout.ready = None;
        let mut sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
        sim.set_navigation_workers(1);
        sim.set_physics_workers(1);
        sim.start_battle().unwrap();
        let mut edit = Workshop::new(Some(layout), false);
        edit.controlled = Some(0);
        edit.selected_units = vec![0];
        let mut app = panel::hud::tests::app_with_scene(sim, edit);
        app.init_resource::<construction::Catalog>()
            .insert_resource(laboratory::Laboratory::new(false))
            .init_resource::<pilot::Pilot>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, construction::input.after(panel::actions));
        app.world_mut().resource_mut::<panel::Panel>().sandbox = false;
        let camera = app.world_mut().spawn(Camera2d).id();
        let mut fonts = Assets::<Font>::default();
        let mut images = Assets::<Image>::default();
        build(
            &mut app.world_mut().commands(),
            camera,
            &TextFont::default(),
            &mut fonts,
            &mut images,
        );
        app.world_mut().flush();
        app.update();
        let first = app
            .world_mut()
            .query::<(Entity, &Part)>()
            .iter(app.world())
            .find_map(|(e, p)| matches!(p, Part::Recipe(0)).then_some(e))
            .unwrap();
        let root = app
            .world_mut()
            .query::<(Entity, &Part)>()
            .iter(app.world())
            .find_map(|(e, p)| matches!(p, Part::Deck).then_some(e))
            .unwrap();
        *app.world_mut()
            .get_mut::<RelativeCursorPosition>(root)
            .unwrap() = RelativeCursorPosition {
            cursor_over: true,
            normalized: Some(Vec2::splat(0.5)),
        };
        *app.world_mut().get_mut::<Interaction>(first).unwrap() = Interaction::Pressed;
        app.update();
        assert!(
            app.world().resource::<panel::Panel>().blocked,
            "world panel must consume pointer input"
        );
        assert_eq!(
            app.world().resource::<Scene>().sim.production_queue(0),
            Some(vec![RangeUnitKind::Tank]),
            "world recipe uses the existing actor queue"
        );
    }
    #[test]
    fn spatial_commands_match_capabilities() {
        assert!(command_visible(
            Action::CrawlerUnload,
            RangeUnitKind::Crawler,
            false,
            true
        ));
        assert!(!command_visible(
            Action::CrawlerUnload,
            RangeUnitKind::Tank,
            true,
            true
        ));
        assert!(!command_visible(
            Action::Fire,
            RangeUnitKind::NuclearSilo,
            true,
            true
        ));
        assert!(!command_visible(
            Action::NuclearLaunch,
            RangeUnitKind::NuclearSilo,
            true,
            false
        ));
        assert!(!command_visible(
            Action::BattleWeapon(None),
            RangeUnitKind::Commander,
            true,
            true
        ));
    }
}
