//! Screen-space strategic identifiers for distant units. Presentation only.
use super::*;
use bevy::ui::FocusPolicy;
use workbench::EditorCamera;

#[derive(Resource)]
pub(super) struct Icons {
    camera: Entity,
    font: TextFont,
    entries: Vec<(Entity, Entity)>,
    construction: Vec<(Entity, [Entity; 10])>,
}

pub fn setup(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands.insert_resource(Icons {
        camera,
        font: font.clone(),
        entries: Vec::new(),
        construction: Vec::new(),
    });
}

// The value depends on projected scale, not camera altitude or map dimensions.
fn opacity(world_per_pixel: f32) -> f32 {
    let t = ((world_per_pixel - 0.45) / 0.5).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn designation(kind: RangeUnitKind) -> &'static str {
    use RangeUnitKind::*;
    match kind {
        Tank => "◆ 1",
        EidolonTank => "▰ 1",
        Strider => "✣ 2",
        Spider => "✣ 3",
        Artillery => "▲ 1",
        ArtilleryT2 => "▲ 2",
        TurretT1 => "⊕ 1",
        TurretT2 => "⊕ 2",
        ArtilleryTurretT2 => "▴ 2",
        ArtilleryTurretT3 => "▴ 3",
        Base => "▣",
        NuclearSilo => "☢",
        Crawler => "⚙",
        LightningTank => "ϟ 2",
        Walker04 => "✣ 4",
        RazlomT3 => "▴ 4",
        GrebenStorage => "▥",
        KilFactoryT1 => "⌂ 1",
        KilFactoryT2 => "⌂ 2",
        KilFactoryT3 => "⌂ 3",
        KilFactoryT4 => "⌂ 4",
        SkobaDrone => "⌁",
        Commander => "✣ К",
        PowerPlantT1 => "ϟ 1",
        PowerPlantT2 => "ϟ 2",
        PowerPlantT3 => "ϟ 3",
    }
}

fn spawn_icon(commands: &mut Commands, camera: Entity, font: &TextFont) -> (Entity, Entity) {
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: px(30),
                height: px(16),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(2)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::NONE),
            FocusPolicy::Pass,
            UiTargetCamera(camera),
        ))
        .id();
    let label = commands
        .spawn((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..font.clone()
            },
            TextColor(Color::WHITE),
            FocusPolicy::Pass,
        ))
        .id();
    commands.entity(root).add_child(label);
    (root, label)
}

fn filled_segments(delivered: u64, required: u64) -> usize {
    ((u128::from(delivered) * 10) / u128::from(required.max(1))).min(10) as usize
}

fn spawn_construction_bar(commands: &mut Commands, camera: Entity) -> (Entity, [Entity; 10]) {
    let root = commands.spawn((Node {
        position_type: PositionType::Absolute, height: px(12), width: px(77),
        padding: UiRect::all(px(2)), column_gap: px(1), border: UiRect::all(px(1)),
        display: Display::None, ..default()
    }, BackgroundColor(Color::srgba(0.025, 0.05, 0.075, 0.9)),
        BorderColor::all(Color::srgb(0.23, 0.42, 0.48)),
        FocusPolicy::Pass, UiTargetCamera(camera))).id();
    let segments = std::array::from_fn(|_| commands.spawn((Node {
        width: px(6), height: px(6), flex_shrink: 0.0, ..default()
    }, BackgroundColor(Color::srgb(0.12, 0.18, 0.20)),
        BorderColor::all(Color::NONE), FocusPolicy::Pass)).id());
    for segment in segments { commands.entity(root).add_child(segment); }
    (root, segments)
}

pub fn update(
    mut commands: Commands,
    scene: Res<Scene>,
    edit: Res<Workshop>,
    pilot: Option<Res<pilot::Pilot>>,
    lab: Option<Res<laboratory::Laboratory>>,
    mut icons: Option<ResMut<Icons>>,
    scale: Option<Res<UiScale>>,
    camera: Query<(&Camera, &Transform, &Projection), With<EditorCamera>>,
    mut nodes: Query<(&mut Node, &mut BackgroundColor, &mut BorderColor)>,
    mut labels: Query<(&mut Text, &mut TextColor)>,
) {
    let Some(mut icons) = icons.take() else {
        return;
    };
    let ui_scale = scale.as_ref().map_or(1., |s| s.0);
    let active =
        !pilot.as_ref().is_some_and(|p| p.active()) && !lab.as_ref().is_some_and(|l| l.active());
    let view = camera
        .single()
        .ok()
        .and_then(|(camera, transform, projection)| {
            let Projection::Orthographic(p) = projection else {
                return None;
            };
            let size = camera.logical_viewport_size()? / ui_scale;
            let height = match p.scaling_mode {
                bevy::camera::ScalingMode::FixedVertical { viewport_height } => {
                    viewport_height * p.scale
                }
                _ => p.area.height(),
            };
            if size.y <= 0.0 || !height.is_finite() || height <= 0.0 {
                return None;
            }
            Some((
                camera,
                GlobalTransform::from(*transform),
                size,
                opacity(height / size.y),
            ))
        });
    let visible = active && view.as_ref().is_some_and(|v| v.3 > 0.01);
    if visible {
        while icons.entries.len() < scene.sim.units.len() {
            let entry = spawn_icon(&mut commands, icons.camera, &icons.font);
            icons.entries.push(entry);
        }
    }
    // Bars are independent of strategic-icon zoom and use the same shared UI
    // camera. Reuse a small pool; completed or offscreen sites cost no draws.
    let construction_view = active && scene.sim.battle_mode();
    let projection = camera.single().ok().and_then(|(camera, transform, _)| {
        Some((camera, GlobalTransform::from(*transform), camera.logical_viewport_size()? / ui_scale))
    });
    let mut count = 0;
    if construction_view {
        for site in scene.sim.construction_progress_views() {
            let Some((camera, transform, viewport)) = projection.as_ref() else { break };
            let anchor = if let Some(id) = site.unit {
                let Some(unit) = scene.sim.units.get(id).filter(|u| u.status.alive) else { continue };
                let base = Vec3::from(unit.motion.pos.map(|v| v.to_f64_lossy() as f32));
                let height = unit.body.vol.ny as f32 * unit.body.mm as f32 / voxel::unit::WORLD_MM as f32;
                base + Vec3::Y * (height + 4.0)
            } else {
                Vec3::from(site.at.map(|v| v as f32)) + Vec3::Y * 18.0
            };
            let Ok(pixel) = camera.world_to_viewport(transform, anchor) else { continue };
            let pixel = pixel / ui_scale;
            if pixel.x < -40.0 || pixel.x > viewport.x + 40.0 || pixel.y < -50.0 || pixel.y > viewport.y + 20.0 { continue; }
            if count == icons.construction.len() {
                let entry = spawn_construction_bar(&mut commands, icons.camera);
                icons.construction.push(entry);
            }
            let (root, segments) = icons.construction[count];
            count += 1;
            if let Ok((mut node, _, _)) = nodes.get_mut(root) {
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                let left = px(pixel.x - 38.5);
                if node.left != left {
                    node.left = left;
                }
                let top = px(pixel.y - 43.0);
                if node.top != top {
                    node.top = top;
                }
            }
            let filled = filled_segments(site.delivered, site.required);
            let lit = if site.allied { Color::srgb(0.25, 0.90, 0.65) } else { Color::srgb(1.0, 0.46, 0.24) };
            for (i, segment) in segments.into_iter().enumerate() {
                if let Ok((_, mut background, _)) = nodes.get_mut(segment) {
                    let colour = if i < filled { lit } else { Color::srgb(0.12, 0.18, 0.20) };
                    if background.0 != colour { background.0 = colour; }
                }
            }
        }
    }
    for &(root, _) in icons.construction.iter().skip(count) {
        if let Ok((mut node, _, _)) = nodes.get_mut(root) {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
    }
    for (id, &(root, label)) in icons.entries.iter().enumerate() {
        let Ok((mut node, mut background, mut border)) = nodes.get_mut(root) else {
            continue;
        };
        let data = if visible {
            scene
                .sim
                .units
                .get(id)
                .filter(|u| u.status.alive)
                .and_then(|unit| {
                    let (camera, transform, viewport, alpha) = view.as_ref()?;
                    let base = Vec3::from(unit.motion.pos.map(|v| v.to_f64_lossy() as f32));
                    let height = unit.body.vol.ny as f32 * unit.body.mm as f32
                        / voxel::unit::WORLD_MM as f32;
                    let at = base + Vec3::Y * (height + 4.0);
                    let pixel = camera.world_to_viewport(transform, at).ok()? / ui_scale;
                    (pixel.x >= -22.0
                        && pixel.x <= viewport.x + 22.0
                        && pixel.y >= -24.0
                        && pixel.y <= viewport.y + 24.0)
                        .then_some((unit.kind, pixel, *alpha))
                })
        } else {
            None
        };
        let Some((kind, pixel, alpha)) = data else {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
        let left = px(pixel.x - 15.0);
        let top = px(pixel.y - 20.0);
        if node.left != left {
            node.left = left;
        }
        if node.top != top {
            node.top = top;
        }
        let selected = edit.selected_units.contains(&id)
            || (edit.selected_units.is_empty() && edit.controlled == Some(id));
        let allied = scene.sim.allied(id);
        let (r, g, b) = if selected {
            (1.0, 1.0, 1.0)
        } else if allied {
            (0.28, 0.80, 0.98)
        } else {
            (1.0, 0.34, 0.22)
        };
        let bg_alpha = if selected { 0.72 } else { 0.38 };
        let frame_alpha = if selected { 0.95 } else { 0.28 };
        let text_alpha = if selected { 1.0 } else { 0.80 };
        let bg = Color::srgba(0.02, 0.04, 0.065, bg_alpha * alpha);
        if background.0 != bg {
            background.0 = bg;
        }
        let frame = BorderColor::all(Color::srgba(r, g, b, frame_alpha * alpha));
        if *border != frame {
            *border = frame;
        }
        if let Ok((mut text, mut colour)) = labels.get_mut(label) {
            let name = designation(kind);
            if text.0 != name {
                text.0 = name.to_owned();
            }
            let ink = Color::srgba(r, g, b, text_alpha * alpha);
            if colour.0 != ink {
                colour.0 = ink;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn construction_segments_are_bounded_and_handle_zero_cost() {
        assert_eq!(filled_segments(0, 100), 0);
        assert_eq!(filled_segments(49, 100), 4);
        assert_eq!(filled_segments(100, 100), 10);
        assert_eq!(filled_segments(u64::MAX, 1), 10);
        assert_eq!(filled_segments(0, 0), 0);
    }
    #[test]
    fn icons_fade_with_zoom_and_identify_every_unit() {
        assert_eq!(opacity(0.3), 0.0);
        assert!(opacity(0.7) > 0.0 && opacity(0.7) < 1.0);
        assert_eq!(opacity(1.0), 1.0);
        for kind in RangeUnitKind::ALL {
            assert!(!designation(kind).is_empty());
        }
        assert_ne!(
            designation(RangeUnitKind::Spider),
            designation(RangeUnitKind::Tank)
        );
    }

    #[test]
    fn native_badges_and_construction_bars_follow_world_anchors_at_ui_scale_two() {
        let mut layout=RangeLayout::from_ron(include_str!("../../../../../data/fixture-drive.ron")).unwrap();
        layout.size=[1536,256,1536];layout.battle=true;layout.idle_enemies=true;
        layout.course=None;layout.ready=None;layout.shields.clear();layout.unit_yaws.clear();
        layout.gun=[64,16,64];layout.target=[1400,16,1400];layout.clear_min=[1,1];layout.clear_max=[1535,1535];
        layout.blocks=vec![modding::range::RangeBlock{min:[0;3],max:[1536,256,1536],material:0},
            modding::range::RangeBlock{min:[0;3],max:[1536,16,1536],material:1}];
        layout.units=vec![modding::range::RangeUnitDef{kind:RangeUnitKind::Tank,allied:true,at:[500,16,500],weapon:None},
            modding::range::RangeUnitDef{kind:RangeUnitKind::Tank,allied:false,at:[1400,16,1400],weapon:None}];
        layout.economy=Some(modding::range_economy::RangeEconomy::stock());
        let mut sim=BlastRange::with_layout(config(false,false),layout.clone()).unwrap();
        sim.set_physics_workers(1);sim.set_navigation_workers(1);sim.start_battle().unwrap();
        let edit=Workshop::new(Some(layout),false);
        let mut app = panel::hud::tests::app_with_scene(sim,edit);
        app.insert_resource(UiScale(1.));
        let window=app.world_mut().spawn(Window{resolution:(1920,1080).into(),..default()}).id();
        assert!(app.world_mut().resource_mut::<Scene>().sim
            .create_construction_site(RangeUnitKind::TurretT1,true,[900,16,900]).is_some());
        let ui_camera = app.world_mut().spawn(Camera2d).id();
        setup(&mut app.world_mut().commands(),ui_camera,&TextFont::default());
        let mut camera=Camera::default();
        camera.computed.target_info=Some(bevy::camera::RenderTargetInfo{physical_size:UVec2::new(1920,1080),scale_factor:1.});
        camera.computed.clip_from_view=Mat4::orthographic_rh(-2200.,2200.,-1237.5,1237.5,0.1,10000.);
        let transform=Transform::from_xyz(1550.,1500.,1950.).looking_at(Vec3::new(550.,100.,550.),Vec3::Y);
        let projection=Projection::Orthographic(OrthographicProjection{scaling_mode:bevy::camera::ScalingMode::FixedVertical{viewport_height:2475.},..OrthographicProjection::default_3d()});
        let camera_entity=app.world_mut().spawn((EditorCamera,camera,transform,projection)).id();
        app.add_systems(PostUpdate,update);
        for (width,height,ui_scale) in [(1920,1080,1.),(3840,2160,2.),(1920,1080,1.)] {
            app.world_mut().get_mut::<Window>(window).unwrap().resolution=(width,height).into();
            app.world_mut().resource_mut::<UiScale>().0=ui_scale;
            app.world_mut().get_mut::<Camera>(camera_entity).unwrap().computed.target_info=Some(bevy::camera::RenderTargetInfo{physical_size:UVec2::new(width,height),scale_factor:1.});
            app.update();app.update();
            assert_eq!(app.world().resource::<UiScale>().0,ui_scale,"exercise the real panel scale");
            let scene=app.world().resource::<Scene>();let unit=&scene.sim.units[0];
            let anchor=Vec3::from(unit.motion.pos.map(|v|v.to_f64_lossy() as f32))+Vec3::Y*(unit.body.vol.ny as f32*unit.body.mm as f32/voxel::unit::WORLD_MM as f32+4.);
            let camera=app.world().get::<Camera>(camera_entity).unwrap();let gt=GlobalTransform::from(transform);
            let expected=camera.world_to_viewport(&gt,anchor).unwrap()/ui_scale;
            let icons=app.world().resource::<Icons>();assert!(!icons.entries.is_empty());
            let node=app.world().get::<Node>(icons.entries[0].0).unwrap();
            assert_eq!(node.display,Display::Flex);
            assert_eq!((node.left,node.top),(px(expected.x-15.),px(expected.y-20.)),"unit badge at UI scale{ui_scale}");
            assert_eq!(icons.construction.len(),1);
            let site=scene.sim.construction_progress_views().next().unwrap();assert!(site.unit.is_none());
            let expected=camera.world_to_viewport(&gt,Vec3::from(site.at.map(|v|v as f32))+Vec3::Y*18.).unwrap()/ui_scale;
            let node=app.world().get::<Node>(icons.construction[0].0).unwrap();
            assert_eq!(node.display,Display::Flex);
            assert_eq!((node.left,node.top),(px(expected.x-38.5),px(expected.y-43.)),"construction bar at UI scale{ui_scale}");
            let (badge, bar) = (icons.entries[0].0, icons.construction[0].0);
            let badge_tick = app.world().entity(badge).get_ref::<Node>().unwrap().last_changed();
            let bar_tick = app.world().entity(bar).get_ref::<Node>().unwrap().last_changed();
            app.update();
            assert_eq!(app.world().entity(badge).get_ref::<Node>().unwrap().last_changed(), badge_tick,
                "stationary badge dirtied layout at UI scale {ui_scale}");
            assert_eq!(app.world().entity(bar).get_ref::<Node>().unwrap().last_changed(), bar_tick,
                "stationary construction bar dirtied layout at UI scale {ui_scale}");
        }
    }
}
