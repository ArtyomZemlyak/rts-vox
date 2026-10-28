//! First-person presentation and canonical direct-control commands (ADR-785).
use super::*;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::window::{CursorGrabMode, CursorOptions};
use sim_core::blast::battle::{Order, Phase};

#[derive(Resource, Default)]
pub struct Pilot {
    pub actor: Option<usize>,
    generation: u64,
    yaw: f32,
    pitch: f32,
    head: i32,
    throttle: i8,
    turn: i8,
    fire: bool,
    pub consumed_escape: bool,
    saved_projection: Option<Projection>,
    requested: bool,
    trigger_ready: bool,
}
impl Pilot {
    pub(super) fn rebind_generation(&mut self, generation: u64) { self.generation = generation; }
    pub fn requested(requested: bool) -> Self {
        Self {
            requested,
            ..default()
        }
    }
    pub fn active(&self) -> bool {
        self.actor.is_some()
    }
    pub fn leave(&mut self, scene: &mut Scene) {
        if let Some(id) = self.actor.take() {
            if self.generation == scene.generation {
                scene.sim.battle_order(id, Order::Hold, None);
            }
        }
        self.throttle = 0;
        self.turn = 0;
        self.fire = false;
    }
    fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        )
    }
    fn eye(&self, scene: &Scene, visual: Option<&moving::Visual>) -> Option<Vec3> {
        let id = self.actor?;
        let u = scene.sim.units.get(id)?;
        let pose = moving::visible(u, visual.and_then(|v| v.previous.get(id)), scene);
        // Actual posed body envelope works for walkers, crawlers and structures.
        let roof = u.bounds().1[1] as f32 - u.motion.pos[1].to_f64_lossy() as f32;
        Some(pose.pos + Vec3::Y * (roof + 0.6))
    }
    /// Called at each fixed tick, including catch-up ticks. No direct pose writes.
    pub fn tick(&self, scene: &mut Scene) {
        let Some(id) = self.actor else { return };
        if self.generation != scene.generation {
            return;
        }
        // Entering the camera keeps setup quiet. Pulling its armed trigger
        // explicitly starts combat through the same lifecycle as the RTS UI.
        if self.fire && !scene.paused && scene.sim.battle.phase == Phase::Setup {
            if let Err(error) = scene.sim.start_battle() {
                scene.notice = error.into();
            }
        }
        let Some(eye) = self.eye(scene, None) else {
            return;
        };
        let direction = self.direction();
        let size = scene.sim.size();
        let from = scene.sim.units[id].motion.pos;
        let centre = Vec2::new(from[0].to_f64_lossy() as f32, from[2].to_f64_lossy() as f32);
        let range = scene
            .sim
            .battle_attack_range(id)
            .unwrap_or_else(|| size[0].max(size[2]) as i32) as f32;
        let horizontal = Vec2::new(direction.x, direction.z).length();
        // SIM gates explicit points by horizontal actor-centre distance.
        // Leave the maximum X/Z voxel-rounding error inside that same radius.
        let mut distance = if horizontal > 1e-6 {
            (range - std::f32::consts::SQRT_2).max(0.0) / horizontal
        } else {
            size.into_iter().max().unwrap_or(1) as f32
        };
        for a in 0..3 {
            if direction[a].abs() > 1e-6 {
                let face = if direction[a] > 0.0 {
                    size[a] as f32 - 1.0
                } else {
                    0.0
                };
                distance = distance.min(((face - eye[a]) / direction[a]).max(0.0));
            }
        }
        let end = eye + direction * distance;
        let endpoint = std::array::from_fn(|a| (end[a] as i32).clamp(0, size[a] as i32 - 1));
        // Cull units once by their ray / AABB; voxel traversal then visits only
        // intersecting bodies, rather than all actors at every terrain cell.
        let candidates: Vec<_> = scene
            .sim
            .units
            .iter()
            .enumerate()
            .filter(|(i, u)| *i != id && u.status.alive && ray_box(eye, direction, u.bounds()))
            .map(|(i, _)| i)
            .collect();
        let aim =
            workbench::visit_pick_ray(size, Ray3d::new(eye, Dir3::new_unchecked(direction)), |p| {
                let delta = Vec2::new(p[0] as f32, p[2] as f32) - centre;
                if delta.length_squared() > range * range {
                    return Some(endpoint);
                }
                (sim_core::blast::material(&scene.sim.world, p) != 0
                    || candidates.iter().any(|&i| scene.sim.units[i].solid(p)))
                .then_some(p)
            })
            .unwrap_or(endpoint);
        let order = Order::Pilot {
            throttle: self.throttle,
            turn: self.turn,
            aim,
            fire: self.fire && scene.sim.battle.phase == Phase::Fighting,
        };
        if scene
            .sim
            .battle
            .actors
            .get(id)
            .is_some_and(|a| a.order != order)
        {
            scene.sim.battle_order(id, order, None);
        }
    }
    pub fn camera(
        &mut self,
        scene: &Scene,
        visual: &moving::Visual,
        transform: &mut Transform,
        projection: &mut Projection,
        aspect: f32,
    ) -> bool {
        if let Some(eye) = self
            .eye(scene, Some(visual))
            .filter(|_| self.generation == scene.generation)
        {
            if self.saved_projection.is_none() {
                self.saved_projection = Some(projection.clone());
            }
            *projection = Projection::Perspective(PerspectiveProjection {
                fov: 75.0_f32.to_radians(),
                aspect_ratio: aspect,
                near: 0.15,
                far: (scene.sim.size()[0].max(scene.sim.size()[2]) as f32 * 2.0).max(2000.0),
                ..default()
            });
            *transform = Transform::from_translation(eye).looking_to(self.direction(), Vec3::Y);
            true
        } else {
            if let Some(saved) = self.saved_projection.take() {
                *projection = saved;
            }
            false
        }
    }
}
fn ray_box(eye: Vec3, dir: Vec3, (lo, hi): ([i32; 3], [i32; 3])) -> bool {
    let (mut near, mut far) = (0.0_f32, f32::INFINITY);
    for a in 0..3 {
        if dir[a].abs() < 1e-6 {
            if eye[a] < lo[a] as f32 || eye[a] >= hi[a] as f32 {
                return false;
            }
        } else {
            let x = (lo[a] as f32 - eye[a]) / dir[a];
            let y = (hi[a] as f32 - eye[a]) / dir[a];
            near = near.max(x.min(y));
            far = far.min(x.max(y));
        }
    }
    near <= far
}
pub fn input(
    mut pilot: ResMut<Pilot>,
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut windows: Query<(&Window, &mut CursorOptions)>,
    mut panel: ResMut<panel::Panel>,
) {
    pilot.consumed_escape = false;
    let Ok((window, mut cursor)) = windows.single_mut() else {
        return;
    };
    if pilot.active()
        && (keys.just_pressed(KeyCode::Escape)
            || keys.just_pressed(KeyCode::KeyV)
            || keys.just_pressed(KeyCode::KeyE)
            || keys.just_pressed(KeyCode::Home)
            || !window.focused
            || scene.generation != pilot.generation
            || !scene
                .sim
                .units
                .get(pilot.actor.unwrap())
                .is_some_and(|u| u.status.alive)
            || !matches!(scene.sim.battle.phase, Phase::Setup | Phase::Fighting))
    {
        pilot.consumed_escape = keys.just_pressed(KeyCode::Escape);
        pilot.leave(&mut scene);
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        return;
    }
    let requested = pilot.requested;
    if !pilot.active()
        && (keys.just_pressed(KeyCode::KeyV) || requested)
        && !edit.editing
        && window.focused
    {
        pilot.requested = false;
        let id = edit
            .controlled
            .filter(|&i| {
                scene.sim.units.get(i).is_some_and(|u| u.status.alive) && scene.sim.allied(i)
            })
            .or_else(|| {
                scene
                    .sim
                    .units
                    .iter()
                    .enumerate()
                    .find(|(i, u)| scene.sim.allied(*i) && u.status.alive)
                    .map(|(i, _)| i)
            });
        if !scene.sim.battle_mode() {
            scene.notice = "Для управления юнитами выберите бой или свободное управление".into();
            return;
        }
        if let Some(id) = id {
            if scene.sim.battle.phase == Phase::Setup && !scene.sim.patrols_running {
                if let Err(e) = scene.sim.start_free_control() {
                    scene.notice = e.into();
                    return;
                }
            }
            pilot.actor = Some(id);
            pilot.generation = scene.generation;
            pilot.head = scene.sim.units[id].motion.drive.head;
            pilot.yaw = pilot.head as f32 * std::f32::consts::TAU / fx::angle::FULL as f32;
            pilot.trigger_ready = !mouse.pressed(MouseButton::Left);
            pilot.pitch = -0.06;
            pilot.throttle = 0;
            pilot.turn = 0;
            pilot.fire = false;
            edit.controlled = Some(id);
            edit.selected = Some(workbench::Selection::Unit(id));
            panel.drawer = 0;
            panel.ground_fire = false;
            scene.paused = false;
            scene.repeat = false;
            scene.notice.clear();
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
            pilot.tick(&mut scene);
        }
    } else if let Some(id) = pilot.actor {
        let head = scene.sim.units[id].motion.drive.head;
        pilot.yaw += fx::angle::diff(head, pilot.head) as f32 * std::f32::consts::TAU
            / fx::angle::FULL as f32;
        pilot.head = head;
        pilot.yaw = (pilot.yaw + motion.delta.x * 0.0025).rem_euclid(std::f32::consts::TAU);
        pilot.pitch = (pilot.pitch - motion.delta.y * 0.0025).clamp(-1.4, 1.4);
        pilot.throttle =
            i8::from(keys.pressed(KeyCode::KeyW)) - i8::from(keys.pressed(KeyCode::KeyS));
        pilot.turn = i8::from(keys.pressed(KeyCode::KeyD)) - i8::from(keys.pressed(KeyCode::KeyA));
        pilot.trigger_ready |= !mouse.pressed(MouseButton::Left);
        pilot.fire = pilot.trigger_ready && mouse.pressed(MouseButton::Left);
        if scene.paused || keys.just_pressed(KeyCode::Space) {
            pilot.throttle = 0;
            pilot.turn = 0;
            pilot.fire = false;
            pilot.tick(&mut scene);
        }
    }
    // W has a range-preset shortcut; captured controls never reach it.
    if pilot.active() {
        keys.clear_just_pressed(KeyCode::KeyW);
    }
}
#[derive(Component)]
pub struct PilotText;
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands.spawn((
        PilotText,
        Text::new(""),
        font.clone(),
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: percent(32),
            right: percent(32),
            bottom: px(24),
            ..default()
        },
        bevy::ui::FocusPolicy::Pass,
        UiTargetCamera(camera),
        ZIndex(30),
    ));
    commands.spawn((
        PilotCrosshair,
        Text::new("+"),
        font.clone(),
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: percent(50),
            top: percent(50),
            ..default()
        },
        bevy::ui::FocusPolicy::Pass,
        UiTargetCamera(camera),
        ZIndex(30),
        Visibility::Hidden,
    ));
}
#[derive(Component)]
pub struct PilotCrosshair;
pub fn hud(
    pilot: Res<Pilot>,
    scene: Res<Scene>,
    mut text: Query<&mut Text, With<PilotText>>,
    mut cross: Query<&mut Visibility, With<PilotCrosshair>>,
    mut decks: Query<
        &mut Visibility,
        (
            Without<PilotCrosshair>,
            Or<(
                With<panel::hud::CommandDeck>,
                With<panel::hud::RosterStrip>,
                With<panel::hud::BriefingCard>,
                With<panel::hud::BattleControls>,
            )>,
        ),
    >,
) {
    for mut visibility in &mut decks {
        *visibility = if pilot.active() {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
    for mut text in &mut text {
        text.0 = pilot.actor.map_or_else(String::new, |id| format!("{} · {}\nW/S — ход · A/D — поворот · мышь — прицел\nЛКМ — огонь · V / Esc — общий вид", scene.sim.units.get(id).map(|u| format!("{:?}",u.kind)).unwrap_or_default(),
        if scene.sim.battle.phase == Phase::Setup { "ЛКМ — начать бой и открыть огонь" } else { "прямое управление" }));
    }
    for mut v in &mut cross {
        *v = if pilot.active() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        let mut app = panel::hud::tests::app();
        app.init_resource::<Pilot>()
            .init_resource::<moving::Visual>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .add_systems(Update, input.after(panel::actions));
        app.world_mut().resource_mut::<Workshop>().editing = false;
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            CursorOptions::default(),
        ));
        app
    }
    #[test]
    fn pilot_camera_all_kinds_stays_above_body_and_restores_projection() {
        let mut app = app();
        let mut scene = app.world_mut().resource_mut::<Scene>();
        for kind in RangeUnitKind::ALL {
            scene.sim.units[0] = sim_core::blast::RangeUnit::of_kind(kind, [170, 16, 336]);
            let mut pilot = Pilot {
                actor: Some(0),
                generation: scene.generation,
                ..default()
            };
            let mut projection = Projection::Orthographic(OrthographicProjection::default_3d());
            let mut transform = Transform::IDENTITY;
            let before = scene.sim.state_hash();
            assert!(pilot.camera(
                &scene,
                &moving::Visual::default(),
                &mut transform,
                &mut projection,
                16.0 / 9.0
            ));
            assert!(matches!(projection, Projection::Perspective(_)));
            assert!(transform.translation.is_finite());
            assert!(transform.translation.y > scene.sim.units[0].bounds().1[1] as f32);
            assert_eq!(before, scene.sim.state_hash());
            pilot.actor = None;
            assert!(!pilot.camera(
                &scene,
                &moving::Visual::default(),
                &mut transform,
                &mut projection,
                16.0 / 9.0
            ));
            assert!(matches!(projection, Projection::Orthographic(_)));
        }
    }
    #[test]
    fn pilot_v_enters_drives_and_escape_or_focus_loss_stops_and_unlocks() {
        for focus in [true, false] {
            let mut app = app();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyV);
            app.update();
            assert!(app.world().resource::<Pilot>().active());
            {
                let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
                keys.clear();
                keys.press(KeyCode::KeyW);
            }
            app.update();
            app.world_mut().resource_scope(|world, pilot: Mut<Pilot>| {
                let mut scene = world.resource_mut::<Scene>();
                pilot.tick(&mut scene);
                assert!(matches!(
                    scene.sim.battle.actors[pilot.actor.unwrap()].order,
                    Order::Pilot { throttle: 1, .. }
                ));
            });
            if focus {
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(KeyCode::Escape);
            } else {
                let world = app.world_mut();
                let mut windows = world.query::<&mut Window>();
                windows.single_mut(world).unwrap().focused = false;
            }
            app.update();
            assert!(!app.world().resource::<Pilot>().active());
            assert_eq!(
                app.world().resource::<Scene>().sim.battle.actors[0].order,
                Order::Hold
            );
            let world = app.world_mut();
            let mut cursors = world.query::<&CursorOptions>();
            assert!(matches!(
                cursors.single(world).unwrap().grab_mode,
                CursorGrabMode::None
            ));
        }
    }
    #[test]
    fn pilot_entry_keeps_other_free_control_orders_and_reset_or_death_releases() {
        for reset in [true, false] {
            let mut app = app();
            {
                let mut scene = app.world_mut().resource_mut::<Scene>();
                scene.sim.start_free_control().unwrap();
                assert!(scene.sim.battle_order(1, Order::Move, Some([3500, 2500])));
            }
            let before = app.world().resource::<Scene>().sim.units[1]
                .motion
                .destination;
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyV);
            app.update();
            assert!(app.world().resource::<Pilot>().active());
            assert_eq!(
                before,
                app.world().resource::<Scene>().sim.units[1]
                    .motion
                    .destination
            );
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
            {
                let mut scene = app.world_mut().resource_mut::<Scene>();
                if reset {
                    scene.generation += 1;
                } else {
                    scene.sim.units[0].status.alive = false;
                }
            }
            app.update();
            assert!(!app.world().resource::<Pilot>().active());
            let world = app.world_mut();
            let mut cursors = world.query::<&CursorOptions>();
            assert!(cursors.single(world).unwrap().visible);
        }
    }
    #[test]
    fn pilot_hud_hides_and_restores_rts_panels() {
        let mut app = panel::hud::tests::app();
        app.init_resource::<Pilot>().add_systems(PostUpdate, hud);
        let deck = app
            .world_mut()
            .spawn((panel::hud::CommandDeck, Node::default()))
            .id();
        app.world_mut().resource_mut::<Pilot>().actor = Some(0);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(deck).unwrap(),
            Visibility::Hidden
        );
        app.world_mut().resource_mut::<Pilot>().actor = None;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(deck).unwrap(),
            Visibility::Inherited
        );
        assert_ne!(
            app.world().get::<Node>(deck).unwrap().display,
            Display::None
        );
    }
    #[test]
    fn pilot_ray_box_rejects_behind_and_parallel_and_accepts_inside() {
        let bounds = ([10, 0, 0], [20, 10, 10]);
        assert!(ray_box(Vec3::new(0.0, 5.0, 5.0), Vec3::X, bounds));
        assert!(!ray_box(Vec3::new(30.0, 5.0, 5.0), Vec3::X, bounds));
        assert!(!ray_box(Vec3::new(15.0, 15.0, 5.0), Vec3::X, bounds));
        assert!(ray_box(Vec3::new(15.0, 5.0, 5.0), Vec3::X, bounds));
    }
    #[test]
    fn pilot_trigger_starts_setup_and_actually_fires_without_entry_click_leaking() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyV);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.update();
        app.world_mut().resource_scope(|world, pilot: Mut<Pilot>| {
            let mut scene = world.resource_mut::<Scene>();
            pilot.tick(&mut scene);
            assert_eq!(scene.sim.battle.phase, Phase::Setup);
            assert_eq!(scene.sim.battle.actors[pilot.actor.unwrap()].fired, 0);
        });
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        app.world_mut()
            .resource_scope(|world, mut pilot: Mut<Pilot>| {
                let mut scene = world.resource_mut::<Scene>();
                // A shallow ray meets distant terrain beyond weapon reach.
                pilot.pitch = -0.002;
                let id = pilot.actor.unwrap();
                scene.sim.battle.trace_fire(true);
                pilot.tick(&mut scene);
                assert_eq!(scene.sim.battle.phase, Phase::Fighting);
                assert!(matches!(
                    scene.sim.battle.actors[id].order,
                    Order::Pilot { fire: true, .. }
                ));
                for _ in 0..240 {
                    pilot.tick(&mut scene);
                    scene.sim.step();
                    if scene.sim.battle.actors[id].fired > 0 {
                        break;
                    }
                }
                assert!(
                    scene.sim.battle.actors[id].fired > 0,
                    "trigger never fired: gate={:?} order={:?}",
                    scene.sim.battle.fire_gate(id),
                    scene.sim.battle.actors[id].order
                );
            });
    }
    #[test]
    fn pilot_aim_keeps_sky_and_distant_terrain_inside_weapon_range() {
        let mut app = app();
        let mut scene = app.world_mut().resource_mut::<Scene>();
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let pos = scene.sim.units[0].motion.pos.map(|p| p.to_int_trunc());
            scene.sim.units[0] = sim_core::blast::RangeUnit::of_kind(kind, pos);
            let mut pilot = Pilot {
                actor: Some(0),
                generation: scene.generation,
                ..default()
            };
            for yaw in [0.0, 0.7, 3.0, 4.0] {
                for pitch in [-0.002, 0.0, 1.3] {
                    pilot.yaw = yaw;
                    pilot.pitch = pitch;
                    pilot.tick(&mut scene);
                    let Order::Pilot { aim, .. } = scene.sim.battle.actors[0].order else {
                        panic!("missing pilot order")
                    };
                    let p = scene.sim.units[0].motion.pos;
                    let dx = fx::Fx::from_int(aim[0]).sub(p[0]);
                    let dz = fx::Fx::from_int(aim[2]).sub(p[2]);
                    let range = fx::Fx::from_int(scene.sim.battle_attack_range(0).unwrap());
                    assert!(
                        dx.mul(dx).add(dz.mul(dz)) <= range.mul(range),
                        "{kind:?} yaw={yaw} pitch={pitch} aim={aim:?}"
                    );
                }
            }
        }
    }
}
