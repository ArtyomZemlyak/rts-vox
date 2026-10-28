//! Model-free weapon authoring; firing still goes through the shared range gun.
use super::*;
use modding::laboratory::sources::Source;

pub fn ensure(lab: &mut Laboratory) -> Result<(), String> {
    scenario::materialize(lab)?;
    if !lab.constructor.workspace.sources.is_empty() || lab.constructor.experiment.is_some() {
        return Ok(());
    }
    let layout = lab.constructor.layout(&lab.definition, lab.recipe)?;
    let source = Source {
        name: "Источник 1".into(),
        at: [layout.gun[0], layout.gun[1] + 32, layout.gun[2]],
        target: layout.target,
        binding: None,
        config: lab.preset_config(),
        power_milli: 1000,
        visual: if lab.recipe.effect == 6 {
            2
        } else if lab.recipe.effect == 2 {
            1
        } else {
            0
        },
        effect: lab
            .constructor
            .weapon
            .is_none()
            .then_some(lab.recipe.effect),
        detonate: false,
        input: None,
    };
    let mut source = source;
    if let Some((_, visual)) = lab.constructor.weapon {
        source.visual = visual;
    }
    source.validate(&layout, lab.constructor.placed.len())?;
    lab.constructor.workspace.sources.push(source);
    lab.constructor.revision += 1;
    Ok(())
}
pub fn active(lab: &Laboratory) -> Option<&Source> {
    lab.constructor
        .workspace
        .sources
        .get(lab.constructor.workspace.selected)
}
pub fn ensure_weapon(lab:&mut Laboratory)->Result<(),String> {
    ensure(lab)?;
    if lab.constructor.workspace.sources.is_empty() {
        if lab.constructor.experiment.as_ref().is_some_and(|p|p.inputs.len()>=32) {return Err("Не более 32 команд в сценарии".into());}
        if lab.constructor.base.as_ref().is_some_and(|b|b.battle) {return Err("В боевой сцене используйте штатное оружие участника в командах сценария".into());}
        let plan=lab.constructor.experiment.take();
        let result=ensure(lab);
        lab.constructor.experiment=plan;
        result?;scenario::attach_selected(lab)?;
    }
    Ok(())
}
pub fn active_mut(lab: &mut Laboratory) -> Option<&mut Source> {
    let i = lab.constructor.workspace.selected;
    lab.constructor.workspace.sources.get_mut(i)
}
pub fn readout(lab: &Laboratory) -> String {
    let Some(s) = active(lab) else {
        return String::new();
    };
    let name = s.effect.map_or("Своё оружие", |i| {
        [
            "Фугас",
            "Бронебой",
            "Энергопушка",
            "Лазер",
            "Огнемёт",
            "Удар по опоре",
            "Анипушка",
            "Пулемёт",
            "Ядерный заряд",
            "Осадный снаряд",
        ][i as usize]
    });
    format!(
        "{} · {} · {:.3}×\nЭнергия: {} · наблюдение: {} тиков",
        s.name,
        name,
        s.power_milli as f64 / 1000.0,
        s.resolved().energy,
        lab.constructor
            .workspace
            .observation_ticks
            .unwrap_or(lab.definition.observation_ticks)
    )
}
pub fn actor(lab: &Laboratory, source: &Source, sim: &BlastRange) -> Result<Option<usize>, String> {
    let Some(binding) = source.binding else {
        return Ok(None);
    };
    let c = &lab.constructor;
    let item = c
        .placed
        .get(binding)
        .filter(|p| !p.element.units.is_empty())
        .ok_or("В привязанном элементе нет юнита")?;
    let base_units = c.base.as_ref().map_or(0, |l| l.units.len());
    let id = base_units
        + c.placed[..binding]
            .iter()
            .map(|p| p.element.units.len())
            .sum::<usize>();
    let unit = sim
        .units
        .get(id)
        .filter(|u| u.status.alive && u.status.weapon)
        .ok_or("Привязанный юнит уничтожен или потерял оружие")?;
    let _ = (item, unit);
    Ok(Some(id))
}
/// Same position for the scene marker, inspector readout and click selection.
pub fn position(lab: &Laboratory, source: &Source, sim: &BlastRange) -> [i32; 3] {
    actor(lab, source, sim)
        .ok()
        .flatten()
        .and_then(|id| sim.battle_muzzles(id).first().copied())
        .map_or(source.at, |p| p.map(fx::Fx::to_int_trunc))
}
/// Constant screen-space hit target: zooming out does not make markers unclickable.
pub fn pick(
    lab: &Laboratory,
    sim: &BlastRange,
    camera: &Camera,
    transform: &GlobalTransform,
    cursor: Vec2,
) -> Option<usize> {
    lab.constructor
        .workspace
        .sources
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let p = Vec3::from(position(lab, s, sim).map(|v| v as f32));
            let screen = camera.world_to_viewport(transform, p).ok()?;
            let distance = screen.distance_squared(cursor);
            (distance <= 14.0 * 14.0).then_some((i, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
        .map(|(i, _)| i)
}
pub fn arm(lab: &Laboratory, scene: &mut Scene) -> Result<(), String> {
    let Some(source) = active(lab) else {
        return Ok(());
    };
    source.validate(
        scene.sim.layout.as_ref().ok_or("Нет карты")?,
        lab.constructor.placed.len(),
    )?;
    source.resolved().validate()?;
    // A scenario source is authoring data. Its next launch reads these exact
    // values; selecting/editing it must not replace an ongoing scenario shot.
    if lab.constructor.experiment.is_some() { return Ok(()); }
    if !scene.sim.ready() {
        return Err("Дождитесь завершения воздействия перед изменением источника".into());
    }
    let at = if let Some(id) = actor(lab, source, &scene.sim)? {
        scene
            .sim
            .battle_muzzles(id)
            .first()
            .copied()
            .ok_or("У юнита нет действующего ствола")?
            .map(fx::Fx::to_int_trunc)
    } else {
        source.at
    };
    if !scene
        .sim
        .execute_command(Command::Gun(GunCommand::Origin { at }))
        .admitted()
    {
        return Err("Движок отклонил положение источника".into());
    }
    if !scene
        .sim
        .execute_command(Command::ConfigureWeapon {
            config: source.resolved(),
        })
        .admitted()
    {
        return Err("Движок отклонил параметры оружия".into());
    }
    scene.sim.execute_command(Command::Gun(GunCommand::Aim {
        at: Some(source.target),
    }));
    scene.power = source.power_milli;
    scene.energy = source.visual != 0;
    scene.anni = source.visual == 2;
    Ok(())
}
pub fn action(lab: &mut Laboratory, action: LabAction, scene: &mut Scene) -> Result<bool, String> {
    ensure(lab)?;
    if action==LabAction::AddSource && lab.constructor.workspace.sources.is_empty() {
        ensure_weapon(lab)?;lab.ui.tab=10;return Ok(true);
    }
    let before = lab.constructor.workspace.clone();
    let old_plan=lab.constructor.experiment.clone();
    let old_aim = lab.constructor.aim;
    let old_mode = lab.constructor.mode;
    match action {
        LabAction::Source(i) => {
            if i >= before.sources.len() {
                return Err("Источник отсутствует".into());
            }
            lab.constructor.workspace.selected = i;
            lab.constructor.aim = None;
            lab.constructor.mode = constructor::Mode::Select;
            lab.ui.tab=10;
        }
        LabAction::AddSource => {
            if before.sources.len() >= 32 {
                return Err("В сцене не более 32 источников".into());
            }
            if lab.constructor.experiment.as_ref().is_some_and(|p|p.inputs.len()>=32) {return Err("Не более 32 команд в сценарии".into());}
            let mut s = active(lab).ok_or("Выберите свободное оружие")?.clone();
            s.input=None;
            s.name = format!("Источник {}", before.sources.len() + 1);
            s.binding = None;
            s.at[2] = (s.at[2] + 32).min(scene.sim.size()[2] as i32 - 33);
            lab.constructor.workspace.sources.push(s);
            lab.constructor.workspace.selected = before.sources.len();
            scenario::attach_selected(lab)?;
        }
        LabAction::RemoveSource => {
            if before.sources.len() <= 1 && lab.constructor.experiment.is_none() {
                return Err("Оставьте хотя бы один источник".into());
            }
            if let Some(input)=before.sources[before.selected].input {scenario::remove_input(lab,input);}
            else {lab.constructor.workspace.sources.remove(before.selected);}
            lab.constructor.workspace.selected=before.selected.min(lab.constructor.workspace.sources.len().saturating_sub(1));
        }
        LabAction::BindSource(bind) => {
            let detached = if !bind {
                active(lab)
                    .and_then(|s| actor(lab, s, &scene.sim).ok().flatten())
                    .and_then(|id| scene.sim.battle_muzzles(id).first().copied())
                    .map(|p| p.map(fx::Fx::to_int_trunc))
            } else {
                None
            };
            let index = if bind {
                Some(
                    lab.constructor
                        .selected
                        .filter(|i| !lab.constructor.placed[*i].element.units.is_empty())
                        .ok_or("Выберите юнит в составе сцены")?,
                )
            } else {
                None
            };
            let source = active_mut(lab).ok_or("Нет источника")?;
            source.binding = index;
            if bind { source.detonate = false; }
            if let Some(at) = detached {
                source.at = at;
            }
        }
        LabAction::SourcePosition => {
            lab.constructor.mode = constructor::Mode::Source;
            lab.constructor.elevation=0;
            lab.ui.tab = 10;
            return Ok(false);
        }
        LabAction::SourceAt(at) => {
            let source = active_mut(lab).ok_or("Нет источника")?;
            source.at = at;
            source.binding = None;
            lab.constructor.mode = constructor::Mode::Select;
        }
        _ => return Ok(false),
    }
    scenario::sync_sources(lab);
    if let Err(e) = arm(lab, scene) {
        lab.constructor.workspace = before;
        lab.constructor.experiment=old_plan;
        lab.constructor.aim = old_aim;
        lab.constructor.mode = old_mode;
        return Err(e);
    }
    lab.constructor.revision += 1;
    lab.notice = if lab.constructor.experiment.is_some() {"Источник настроен; «Пуск» запускает команды сценария по порядку"}else{"Источник настроен; «Пуск» запускает выбранный источник"}.into();
    Ok(false)
}

pub fn draw(lab: &Laboratory, scene: &Scene, gizmos: &mut Gizmos) {
    for (i, s) in lab.constructor.workspace.sources.iter().enumerate() {
        let p = position(lab, s, &scene.sim);
        let p = Vec3::from(p.map(|v| v as f32));
        let colour = if i == lab.constructor.workspace.selected {
            Color::srgb(1.0, 0.75, 0.25)
        } else {
            Color::srgb(0.35, 0.8, 1.0)
        };
        gizmos.cube(
            Transform::from_translation(p).with_scale(Vec3::splat(5.0)),
            colour,
        );
        if s.detonate && p.y < scene.sim.ground_level() as f32 {
            gizmos.line(p,p.with_y(scene.sim.ground_level() as f32+0.5),colour.with_alpha(0.6));
            gizmos.cross(p.with_y(scene.sim.ground_level() as f32+0.5),3.0,colour);
        }
        if i == lab.constructor.workspace.selected && !s.detonate {
            gizmos.line(
                p,
                Vec3::from(s.target.map(|v| v as f32)),
                colour.with_alpha(0.5),
            );
            gizmos.cross(Vec3::from(s.target.map(|v| v as f32)), 4.0, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::{CameraProjection, RenderTargetInfo};

    #[test]
    fn laboratory_source_markers_select_the_same_emitter_at_different_zoom_levels() {
        let mut app = super::super::integration_tests::app();
        super::super::integration_tests::enter(&mut app);
        super::super::integration_tests::click(&mut app, LabAction::AddSource);
        super::super::integration_tests::click(&mut app, LabAction::SourceAt([448, 48, 448]));
        let scene = app.world().resource::<Scene>();
        let lab = app.world().resource::<Laboratory>();
        let before = scene.sim.snapshot();
        let point = Vec3::new(448.0, 48.0, 448.0);
        let transform = GlobalTransform::from(
            Transform::from_translation(point + Vec3::Y * 700.0).looking_at(point, Vec3::Z),
        );
        for height in [128.0, 512.0, 2048.0] {
            let half = Vec2::new(height * 1920.0 / 1080.0, height) * 0.5;
            let p = OrthographicProjection {
                area: Rect::from_corners(-half, half),
                ..OrthographicProjection::default_3d()
            };
            let mut camera = Camera::default();
            camera.computed.target_info = Some(RenderTargetInfo {
                physical_size: UVec2::new(1920, 1080),
                scale_factor: 1.0,
            });
            camera.computed.clip_from_view = p.get_clip_from_view();
            let cursor = camera.world_to_viewport(&transform, point).unwrap();
            assert_eq!(pick(lab, &scene.sim, &camera, &transform, cursor), Some(1));
            assert_eq!(
                pick(
                    lab,
                    &scene.sim,
                    &camera,
                    &transform,
                    cursor + Vec2::X * 13.0
                ),
                Some(1)
            );
            assert_eq!(
                pick(
                    lab,
                    &scene.sim,
                    &camera,
                    &transform,
                    cursor + Vec2::X * 40.0
                ),
                None
            );
        }
        assert_eq!(
            scene.sim.snapshot(),
            before,
            "hover/picking cannot mutate the simulation"
        );
    }
}
