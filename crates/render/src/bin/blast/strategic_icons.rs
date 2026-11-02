//! Screen-space strategic identifiers for distant units. Presentation only.
use super::*;
use bevy::ui::FocusPolicy;
use workbench::EditorCamera;

#[derive(Resource)]
pub(super) struct Icons {
    camera: Entity,
    font: TextFont,
    entries: Vec<(Entity, Entity)>,
}

pub fn setup(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands.insert_resource(Icons { camera, font: font.clone(), entries: Vec::new() });
}

// The value depends on projected scale, not camera altitude or map dimensions.
fn opacity(world_per_pixel: f32) -> f32 {
    let t = ((world_per_pixel - 0.45) / 0.5).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn designation(kind: RangeUnitKind) -> &'static str {
    use RangeUnitKind::*;
    match kind {
        Tank => "◆ 1", EidolonTank => "▰ 1", Strider => "✣ 2",
        Spider => "✣ 3", Artillery => "▲ 1", ArtilleryT2 => "▲ 2",
        TurretT1 => "⊕ 1", TurretT2 => "⊕ 2", ArtilleryTurretT2 => "▴ 2",
        ArtilleryTurretT3 => "▴ 3", Base => "▣", NuclearSilo => "☢",
        Crawler => "⚙", LightningTank => "ϟ 2",
        Walker04 => "✣ 4", RazlomT3 => "▴ 4", GrebenStorage => "▥",
        KilFactoryT1 => "⌂ 1", KilFactoryT2 => "⌂ 2", KilFactoryT3 => "⌂ 3", KilFactoryT4 => "⌂ 4",
        SkobaDrone => "⌁",
    }
}

fn spawn_icon(commands: &mut Commands, camera: Entity, font: &TextFont) -> (Entity, Entity) {
    let root = commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: px(44), height: px(24),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border: UiRect::all(px(2)),
            border_radius: BorderRadius::all(px(3)),
            display: Display::None,
            ..default()
        },
        BackgroundColor(Color::NONE), BorderColor::all(Color::NONE),
        FocusPolicy::Pass, UiTargetCamera(camera),
    )).id();
    let label = commands.spawn((
        Text::new(""),
        TextFont { font_size: FontSize::Px(12.0), ..font.clone() },
        TextColor(Color::WHITE), FocusPolicy::Pass,
    )).id();
    commands.entity(root).add_child(label);
    (root, label)
}

pub fn update(
    mut commands: Commands,
    scene: Res<Scene>,
    edit: Res<Workshop>,
    pilot: Option<Res<pilot::Pilot>>,
    lab: Option<Res<laboratory::Laboratory>>,
    mut icons: Option<ResMut<Icons>>,
    camera: Query<(&Camera, &Transform, &Projection), With<EditorCamera>>,
    mut nodes: Query<(&mut Node, &mut BackgroundColor, &mut BorderColor)>,
    mut labels: Query<(&mut Text, &mut TextColor)>,
) {
    let Some(mut icons) = icons.take() else { return; };
    let active = !pilot.as_ref().is_some_and(|p| p.active())
        && !lab.as_ref().is_some_and(|l| l.active());
    let view = camera.single().ok().and_then(|(camera, transform, projection)| {
        let Projection::Orthographic(p) = projection else { return None; };
        let size = camera.logical_viewport_size()?;
        let height = match p.scaling_mode {
            bevy::camera::ScalingMode::FixedVertical { viewport_height } => viewport_height * p.scale,
            _ => p.area.height(),
        };
        if size.y <= 0.0 || !height.is_finite() || height <= 0.0 { return None; }
        Some((camera, GlobalTransform::from(*transform), size, opacity(height / size.y)))
    });
    let visible = active && view.as_ref().is_some_and(|v| v.3 > 0.01);
    if visible {
        while icons.entries.len() < scene.sim.units.len() {
            let entry = spawn_icon(&mut commands, icons.camera, &icons.font);
            icons.entries.push(entry);
        }
    }
    for (id, &(root, label)) in icons.entries.iter().enumerate() {
        let Ok((mut node, mut background, mut border)) = nodes.get_mut(root) else { continue; };
        let data = if visible {
            scene.sim.units.get(id).filter(|u| u.status.alive).and_then(|unit| {
                let (camera, transform, viewport, alpha) = view.as_ref()?;
                let base = Vec3::from(unit.motion.pos.map(|v| v.to_f64_lossy() as f32));
                let height = unit.body.vol.ny as f32 * unit.body.mm as f32 / voxel::unit::WORLD_MM as f32;
                let at = base + Vec3::Y * (height + 4.0);
                let pixel = camera.world_to_viewport(transform, at).ok()?;
                (pixel.x >= -22.0 && pixel.x <= viewport.x + 22.0 && pixel.y >= -24.0 && pixel.y <= viewport.y + 24.0)
                    .then_some((unit.kind, pixel, *alpha))
            })
        } else { None };
        let Some((kind, pixel, alpha)) = data else {
            if node.display != Display::None { node.display = Display::None; }
            continue;
        };
        if node.display != Display::Flex { node.display = Display::Flex; }
        let left = px(pixel.x - 22.0);
        let top = px(pixel.y - 28.0);
        if node.left != left { node.left = left; }
        if node.top != top { node.top = top; }
        let selected = edit.selected_units.contains(&id)
            || (edit.selected_units.is_empty() && edit.controlled == Some(id));
        let allied = scene.sim.allied(id);
        let (r, g, b) = if selected { (1.0, 1.0, 1.0) }
            else if allied { (0.20, 0.78, 1.0) } else { (1.0, 0.31, 0.18) };
        let bg = Color::srgba(0.025, 0.05, 0.075, 0.86 * alpha);
        if background.0 != bg { background.0 = bg; }
        let frame = BorderColor::all(Color::srgba(r, g, b, 0.95 * alpha));
        if *border != frame { *border = frame; }
        if let Ok((mut text, mut colour)) = labels.get_mut(label) {
            let name = designation(kind);
            if text.0 != name { text.0 = name.to_owned(); }
            let ink = Color::srgba(r, g, b, alpha);
            if colour.0 != ink { colour.0 = ink; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn icons_fade_with_zoom_and_identify_every_unit() {
        assert_eq!(opacity(0.3), 0.0);
        assert!(opacity(0.7) > 0.0 && opacity(0.7) < 1.0);
        assert_eq!(opacity(1.0), 1.0);
        for kind in RangeUnitKind::ALL { assert!(!designation(kind).is_empty()); }
        assert_ne!(designation(RangeUnitKind::Spider), designation(RangeUnitKind::Tank));
    }
}
