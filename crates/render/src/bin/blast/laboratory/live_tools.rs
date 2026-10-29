//! Live pointer tools: the existing simulation keeps running, no scene rebuild.
use super::*;
#[cfg(test)]
#[path = "live_tools_tests.rs"]
mod tests;

pub fn allowed(lab: &Laboratory, a: LabAction) -> bool {
    !lab.tools.live
        || matches!(
            a,
            LabAction::LiveTools
                | LabAction::Frame
                | LabAction::SectionToggle
                | LabAction::SectionShift(_)
                | LabAction::SectionFocus
                | LabAction::SectionAxis(_)
                | LabAction::SectionSide
                | LabAction::Metric(_)
                | LabAction::MetricScale
                | LabAction::ReplayStart | LabAction::ReplayStop | LabAction::ReplaySeek(_) | LabAction::ReplayJump(_) | LabAction::ReplaySave | LabAction::ReplayOpen | LabAction::ReplayBrowse(_) | LabAction::ReplayPlay | LabAction::ReplayLeave | LabAction::Tab(31)
                | LabAction::Tab(30)
                | LabAction::LiveTool(_)
                | LabAction::LivePoint
                | LabAction::LivePower(_)
                | LabAction::LiveApply(_, _)
                | LabAction::LiveCapture
                | LabAction::LiveSpeed(_)
                | LabAction::LiveLook(_)
                | LabAction::LiveFov(_)
                | LabAction::LiveRelease
                | LabAction::BrushSize(_)
                | LabAction::BrushDistance(_)
                | LabAction::PickMaterial(_)
                | LabAction::Material(_)
                | LabAction::Tab(1 | 21)
                | LabAction::Pause
                | LabAction::Step
                | LabAction::Flying
                | LabAction::Help
                | LabAction::Back
        )
}
pub fn stop(lab: &mut Laboratory, scene: &mut Scene) {
    lab.tools.live_target = None;
    if lab.tools.emitting
        || scene.sim.streams.left != 0
        || (scene.sim.shell.is_none()
            && scene.sim.fire_tick != u32::MAX
            && scene.sim.fire_tick >= scene.sim.tick)
    {
        scene
            .sim
            .execute_command(Command::Gun(GunCommand::LabStream {
                origin: [0; 3],
                target: [0; 3],
                enabled: false,
            }));
        lab.tools.emitting = false;
    }
}
pub fn action(lab: &mut Laboratory, scene: &mut Scene, a: LabAction) {
    match a {
        LabAction::LiveTools => {
            stop(lab, scene);
            lab.tools.live = !lab.tools.live;
            lab.tools.live_tick = None;
            lab.tools.flight.captured = false;
            lab.tools.flight.request = lab.tools.live;
            lab.tools.flight.trigger_ready = false;
            lab.tools.flight.overview_return = None;
            lab.constructor.selected = None;
            lab.constructor.hover = None;
            lab.browser = false;
            if lab.tools.live {
                scene.sim.execute_command(Command::Gun(GunCommand::Control));
                lab.constructor.mode = constructor::Mode::Live;
                lab.ui.tab = 21;
                lab.tools.flying = true;
                // Editor overview can be above the world's simulation bounds.
                // Start the flying instrument inside the map, looking at the scene.
                let size = scene.sim.size();
                let inside = lab.tools.camera.is_some_and(|c| {
                    let p = c.translation;
                    p.is_finite()
                        && p.x >= 33.0
                        && p.x < size[0] as f32 - 33.0
                        && p.z >= 33.0
                        && p.z < size[2] as f32 - 33.0
                        && p.y >= 2.0
                        && p.y < size[1] as f32 - 2.0
                });
                if !inside {
                    let mut at = lab.constructor.aim.unwrap_or(scene.sim.target());
                    for k in 0..3 {
                        at[k] = at[k].clamp(
                            if k == 1 { 4 } else { 48 },
                            size[k] as i32 - if k == 1 { 32 } else { 48 },
                        );
                    }
                    let p = Vec3::from(at.map(|v| v as f32)) + Vec3::splat(0.5);
                    let camera = (p + Vec3::new(16.0, 20.0, 24.0)).clamp(
                        Vec3::new(33.0, 2.0, 33.0),
                        Vec3::new(
                            size[0] as f32 - 34.0,
                            size[1] as f32 - 2.0,
                            size[2] as f32 - 34.0,
                        ),
                    );
                    lab.tools.camera =
                        Some(Transform::from_translation(camera).looking_at(p, Vec3::Y));
                }
                scene.paused = false;
                scene.realtime = true;
                scene.acc = 0.0;
                scene.skip_delta = true;
                lab.step_requested = false;
                lab.observing = false;
                lab.deadline = None;
                lab.notice="От первого лица: мышь — обзор, ЛКМ — инструмент, Space/Ctrl — высота, Tab — меню".into();
            } else {
                lab.constructor.mode = constructor::Mode::Select;
                lab.ui.tab = 16;
                lab.notice = "Живые инструменты выключены; симуляция продолжает работать".into();
            }
        }
        LabAction::LiveTool(n) if n <= 7 => {
            stop(lab, scene);
            lab.tools.live_tool = n;
            lab.tools.live_tick = None;
            lab.notice = format!("{} · удерживайте ЛКМ в сцене", name(n));
        }
        LabAction::LivePoint => {
            stop(lab, scene);
            lab.tools.point = !lab.tools.point;
            lab.tools.live_tick = None;
        }
        LabAction::LivePower(d) => {
            stop(lab, scene);
            lab.tools.power_milli =
                (lab.tools.power_milli as i32 + d * 250).clamp(100, 4000) as u32;
            lab.tools.live_tick = None;
        }
        LabAction::LiveCapture => {
            if let Some(camera)=lab.tools.flight.overview_return.take() {lab.tools.camera=Some(camera);}
            lab.tools.flying = true;
            lab.tools.flight.request = true;
        }
        LabAction::LiveSpeed(d) => {
            lab.tools.flight.speed = (lab.tools.flight.speed * 1.25f32.powi(d)).clamp(5.0, 600.0);
        }
        LabAction::LiveLook(d) => {
            lab.tools.flight.sensitivity =
                (lab.tools.flight.sensitivity * 1.2f32.powi(d)).clamp(0.0004, 0.012);
        }
        LabAction::LiveFov(d) => {
            lab.tools.flight.fov = (lab.tools.flight.fov + d as f32 * 5.0).clamp(50.0, 100.0);
        }
        LabAction::LiveRelease => stop(lab, scene),
        LabAction::LiveApply(at, origin) if lab.tools.live && !scene.paused => {
            lab.tools.live_target = Some((at, origin));
        }
        _ => {}
    }
}
fn emit(lab: &mut Laboratory, scene: &mut Scene, at: [i32; 3], origin: [i32; 3]) {
    if lab.tools.live_tick == Some(scene.sim.tick) {
        return;
    }
    lab.tools.live_tick = Some(scene.sim.tick);
    let n = lab.tools.live_tool;
    if n <= 2 {
        let mat = if n == 2 {
            modding::water::MATERIAL
        } else {
            lab.constructor.material(&lab.definition, lab.recipe)
        };
        scene.sim.execute_command(Command::Sculpt {
            at,
            edge: lab.tools.edge,
            material: mat,
            erase: n == 0,
        });
    } else {
        let delivery = match n {
            3 => Delivery::Flame,
            4 => Delivery::Laser,
            5 => Delivery::MachineGun,
            6 => Delivery::Blast,
            _ => Delivery::Kinetic,
        };
        let mut config = weapon_config(false, false, lab.tools.power_milli, delivery);
        if let Some(layout) = &scene.sim.layout {
            config.unit_ablation_resistance = layout.unit_resistance;
        }
        if scene.sim.config != config {
            // Contacts read the active config. Finish old packets and
            // solid flight before changing their delivery or power.
            if !scene.sim.streams.rounds.is_empty()
                || scene.sim.shell.is_some()
                || scene.sim.fire_tick != u32::MAX && scene.sim.fire_tick >= scene.sim.tick
            {
                lab.notice =
                    "Снаряды долетают; выбранное оружие включится при удержании ЛКМ".into();
                return;
            }
            scene
                .sim
                .execute_command(Command::ConfigureWeapon { config });
        }
        let outcome = scene
            .sim
            .execute_command(Command::Gun(GunCommand::LabStream {
                origin,
                target: at,
                enabled: true,
            }));
        lab.tools.emitting |= outcome.accepted();
        scene.energy = false;
        scene.anni = false;
        if outcome == sim_core::blast::battle::commands::Outcome::Busy {
            lab.notice = "Пушка перезаряжается · удерживайте ЛКМ".into();
        } else if !outcome.accepted() {
            lab.notice = "Источник вне рабочей области. Подлетите ближе к центру карты".into();
        }
    }
}
pub fn name(n: u8) -> &'static str {
    match n {
        0 => "Копатель",
        1 => "Материал",
        2 => "Вода",
        3 => "Огонь",
        4 => "Лазер",
        5 => "Пулемёт",
        6 => "Пушка · фугас",
        _ => "Пушка · бронебой",
    }
}
pub fn text(lab: &Laboratory, paused: bool) -> String {
    format!("{} · {}\nТочка: {}\nКисть: {} см · дальность: {:.1} м\nМощность оружия: {:.2}×\nМатериал: {}\nПолёт: {:.1} м/с · Мышь: {:.2}× · FOV: {:.0}° (верт.)",
        name(lab.tools.live_tool),if paused {"ПАУЗА · Space — продолжить"}else{"СИМУЛЯЦИЯ ИДЁТ"},
        if lab.tools.point {"перед камерой / под курсором"}else{"поверхность под курсором"},
        lab.tools.edge*100/10,lab.tools.distance*0.1,lab.tools.power_milli as f32/1000.0,
        lab.constructor.material_name(&lab.definition,lab.recipe),lab.tools.flight.speed*0.1,lab.tools.flight.sensitivity/0.0025,lab.tools.flight.fov)
}
/// Runs even when the cursor leaves the viewport. No stuck fire over menus or on focus loss.
pub fn release(lab: &mut Laboratory, mouse: &ButtonInput<MouseButton>, blocked: bool) {
    if (lab.tools.emitting || lab.tools.live_target.is_some())
        && (blocked
            || !mouse.pressed(MouseButton::Left)
            || (!lab.tools.flight.captured && mouse.pressed(MouseButton::Right)))
        && !lab.constructor.pending.contains(&LabAction::LiveRelease)
    {
        lab.constructor.pending.push(LabAction::LiveRelease);
    }
}
/// Repeat the held tool once per completed fixed tick, including catch-up
/// ticks in the same render frame. Pointer sampling only updates the target.
pub fn tick(lab: &mut Laboratory, scene: &mut Scene) {
    if lab.tools.live && !scene.paused {
        if let Some((at, origin)) = lab.tools.live_target {
            emit(lab, scene, at, origin);
        }
    }
}
pub fn target(lab: &Laboratory, scene: &Scene, ray: Ray3d) -> Option<([i32; 3], [i32; 3])> {
    let n = lab.tools.live_tool;
    let distant = || {
        // Missing a surface must not disable the trigger when aiming at sky.
        let mut distance = scene.sim.gun_attack_range() as f32;
        for k in 0..3 {
            let d = ray.direction[k];
            if d.abs() > 1e-6 {
                let bound = if d > 0.0 {
                    scene.sim.size()[k] as f32 - 2.0
                } else {
                    1.0
                };
                distance = distance.min((bound - ray.origin[k]) / d);
            }
        }
        ray.get_point(distance.max(0.0))
            .floor()
            .to_array()
            .map(|v| v as i32)
    };
    let mut at = if lab.tools.point {
        ray.get_point(lab.tools.distance)
            .floor()
            .to_array()
            .map(|v| v as i32)
    } else {
        tools::liquid_pick(scene, ray).or_else(|| (n >= 3).then(distant))?
    };
    if !lab.tools.point && matches!(n, 1 | 2) {
        if let Some((_, axis)) = tools::entry(
            ray,
            Vec3::from(at.map(|v| v as f32)),
            Vec3::from(at.map(|v| v as f32)) + Vec3::ONE,
        ) {
            at[axis] += if ray.direction[axis] < 0.0 {
                1
            } else {
                -(lab.tools.edge as i32)
            };
        }
    }
    let edge = if n <= 2 { lab.tools.edge as i32 } else { 1 };
    if (0..3).any(|k| at[k] < 1 || at[k] + edge >= scene.sim.size()[k] as i32) {
        return None;
    }
    let origin = if n >= 3 && lab.tools.flying {
        let pose = lab.tools.camera?;
        first_person::muzzle(scene, pose)?
            .0
            .floor()
            .to_array()
            .map(|v| v as i32)
    } else {
        ray.get_point(1.0).floor().to_array().map(|v| v as i32)
    };
    if n >= 3
        && ((0..3).any(|k| origin[k] < 1 || origin[k] >= scene.sim.size()[k] as i32 - 1)
            || origin[0] < 32
            || origin[0] >= scene.sim.size()[0] as i32 - 32
            || origin[2] < 32
            || origin[2] >= scene.sim.size()[2] as i32 - 32
            || origin == at)
    {
        return None;
    }
    Some((at, origin))
}
pub fn pointer(
    lab: &mut Laboratory,
    scene: &Scene,
    ray: Ray3d,
    mouse: &ButtonInput<MouseButton>,
    gizmos: &mut Gizmos,
) -> bool {
    let Some((at, origin)) = target(lab, scene, ray) else {
        gizmos.cross(
            ray.get_point(lab.tools.distance),
            1.5,
            Color::srgb(1.0, 0.2, 0.1),
        );
        release(lab, mouse, true);
        return true;
    };
    let edge = if lab.tools.live_tool <= 2 {
        lab.tools.edge as f32
    } else {
        1.0
    };
    let p = Vec3::from(at.map(|v| v as f32)) + Vec3::splat(edge * 0.5);
    let colour = match lab.tools.live_tool {
        0 => Color::srgb(1.0, 0.45, 0.1),
        2 => Color::srgb(0.2, 0.6, 1.0),
        3 => Color::srgb(1.0, 0.2, 0.1),
        4 => Color::srgb(0.4, 1.0, 1.0),
        _ => theme::ACCENT,
    };
    gizmos.cube(
        Transform::from_translation(p).with_scale(Vec3::splat(edge)),
        colour,
    );
    if !lab.tools.flight.captured {
        gizmos.line(Vec3::from(origin.map(|v| v as f32)), p, colour);
    }
    sample(lab, scene, at, origin, mouse);
    true
}
/// Sample held input independently of rendering gizmos, then flush to the
/// fixed-tick source. Capturing the pointer alone must never pull the trigger.
pub(super) fn sample(
    lab: &mut Laboratory,
    scene: &Scene,
    at: [i32; 3],
    origin: [i32; 3],
    mouse: &ButtonInput<MouseButton>,
) {
    if mouse.pressed(MouseButton::Left)
        && (!lab.tools.flight.captured || lab.tools.flight.trigger_ready)
        && (lab.tools.flight.captured || !mouse.pressed(MouseButton::Right))
        && !scene.paused
        && !lab
            .constructor
            .pending
            .iter()
            .any(|a| matches!(a, LabAction::LiveApply(_, _)))
    {
        lab.constructor
            .pending
            .push(LabAction::LiveApply(at, origin));
    }
}
