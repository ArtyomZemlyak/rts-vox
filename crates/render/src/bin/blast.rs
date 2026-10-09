//! One siege gun, one real shell, a destructible target and an obstructed gas plume.
//! `blast --capture out/blast --frames 330` writes reproducible PNG frames.
//! `--weapon energy` selects the long range and seven energy stages.
//! `--shots out/blast` writes key stages. Space pauses, R restarts, Esc exits.
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::DirectionalLightShadowMap;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use modding::explosion::{Delivery, ExplosionConfig};
use modding::range::{RangeLayout, RangeUnitKind};
use render::gas::{GasCamera, GasInput, GasPlugin, TerrainWindow};
use render::geom::MeshBuf;
use render::model::weather;
use render::occlusion::OcclusionPlugin;
use render::recording::Recording;
use render::{look, presentation};
use sim_core::blast::{BlastRange, GUN};
#[path = "blast/bodies.rs"]
mod bodies;
#[path = "blast/compute.rs"]
mod compute;
#[path = "blast/damage_art.rs"]
mod damage_art;
#[path = "blast/fire.rs"]
mod fire;
#[path = "blast/fragments.rs"]
mod fragments;
#[path = "blast/gun_art.rs"]
mod gun_art;
#[cfg(test)]
#[path = "blast/interaction_tests.rs"]
mod interaction_tests;
#[path = "blast/knobs.rs"]
mod knobs;
#[path = "blast/moving.rs"]
mod moving;
#[path = "blast/overview.rs"]
mod overview;
#[path = "blast/panel.rs"]
mod panel;
#[path = "blast/profile.rs"]
mod profile;
#[path = "blast/streaming.rs"]
mod streaming;
#[path = "blast/ui_snapshot.rs"]
mod ui_snapshot;
#[path = "blast/witness.rs"]
mod witness;
#[path = "blast/workbench.rs"]
mod workbench;
#[path = "blast/wreck.rs"]
mod wreck;
use damage_art::SkinBindings;
use std::collections::BTreeSet;
use std::sync::{atomic::Ordering, Arc};
use workbench::Workshop;

#[derive(Resource)]
struct Scene {
    sim: BlastRange,
    generation: u64,
    energy: bool,
    /// The annihilation bolt: an energy weapon for the picture, its own
    /// preset for the simulation (ADR-285).
    anni: bool,
    paused: bool,
    frame: u32,
    finish: u32,
    out: Option<String>,
    all_frames: bool,
    wait: u32,
    captured: usize,
    acc: f32,
    repeat: bool,
    view: u8,
    /// Deterministic zoom and pan path for the native camera stability capture.
    camera_sweep: bool,
    /// Номер маршрута для `--view target`, считая с единицы.
    patrol: usize,
    previous_shell: Option<Vec3>,
    fps: f32,
    /// The slowest simulation tick of the last thirty frames, microseconds.
    step_us: u128,
    /// Which burning voxel gets this tick's flame (P-22): the picture
    /// walks the fire rather than lighting all of it at once.
    flame: usize,
    realtime: bool,
    live_elapsed: f32,
    last_capture: u32,
    layout_file: Option<String>,
    power: u32,
    uncovered: bool,
    unit_override: Option<RangeUnitKind>,
    skip_delta: bool,
    notice: String,
}
#[derive(Component)]
struct Witness(usize, Transform, Option<usize>, bool);
#[derive(Resource)]
struct WitnessArt {
    materials: [Handle<StandardMaterial>; look::FINISHES],
    burned_materials: [Handle<StandardMaterial>; look::FINISHES],
    pristine: Arc<[voxel::unit::UnitBody; 9]>,
    skins: Arc<[SkinBindings; 9]>,
    seen: Vec<Option<witness::Revision>>,
    pending: Vec<Option<witness::Pending>>,
    emitted: Vec<usize>,
    geometry: sim_core::blast::read_model::geometry::View,
    next_mesh: usize,
    generation: u64,
}
#[derive(Component)]
struct Gun {
    base: Transform,
    turret: bool,
    segment: Option<usize>,
}
#[derive(Component)]
struct Shell;
#[derive(Component)]
struct BlastLight;
#[derive(Component)]
struct MuzzleFlash;
#[derive(Component)]
struct Emitter(usize);
#[derive(Component)]
struct Ejecta {
    velocity: Vec3,
    life: f32,
}
const EJECTA_CAP: usize = 256;
#[derive(Resource)]
struct Art {
    cube: Handle<Mesh>,
    rock: Handle<StandardMaterial>,
    ember: Handle<StandardMaterial>,
    coil: Handle<StandardMaterial>,
}

fn config(energy: bool, anni: bool) -> ExplosionConfig {
    ExplosionConfig::from_ron(if anni {
        include_str!("../../../../data/anni-cannon.ron")
    } else if energy {
        include_str!("../../../../data/energy-cannon.ron")
    } else {
        include_str!("../../../../data/explosion.ron")
    })
    .expect("weapon RON")
}
fn scaled_config(energy: bool, anni: bool, power: u32) -> ExplosionConfig {
    let mut c = config(energy, anni);
    c.energy = c.energy * power / 1000;
    c.material_energy_j = (u64::from(c.material_energy_j) * u64::from(power) / 1000) as u32;
    c.gas_pressure_milli = (c.gas_pressure_milli * power as i32 / 1000).min(10000);
    c.gas_source_radius = c.gas_radius_at_power(power);
    c
}
fn weapon_config(energy: bool, anni: bool, power: u32, delivery: Delivery) -> ExplosionConfig {
    if delivery == Delivery::Blast {
        return scaled_config(energy, anni, power);
    }
    let mut c = ExplosionConfig::from_ron(match delivery {
        Delivery::Flame => include_str!("../../../../data/flamethrower.ron"),
        Delivery::MachineGun => include_str!("../../../../data/machine-gun.ron"),
        Delivery::Kinetic => include_str!("../../../../data/kinetic-shell.ron"),
        Delivery::Laser => include_str!("../../../../data/laser.ron"),
        Delivery::Blast => unreachable!(),
    })
    .expect("weapon RON");
    c.energy = if delivery == Delivery::Kinetic {
        (u64::from(c.energy) * u64::from(power) / 1000) as u32
    } else {
        power
    };
    if delivery == Delivery::Kinetic {
        c.material_energy_j = (u64::from(c.material_energy_j) * u64::from(power) / 1000) as u32;
    }
    c
}
/// What the gas shader reads as the weapon: shell 0, energy bolt 1, the
/// annihilation bolt 2 (ADR-285), which the shader treats as an energy bolt
/// that also bends the picture.
fn weapon_code(energy: bool, anni: bool) -> f32 {
    if anni {
        2.0
    } else if energy {
        1.0
    } else {
        0.0
    }
}
fn read_layout(file: Option<&str>) -> Result<Option<RangeLayout>, String> {
    file.map(|file| {
        RangeLayout::from_ron(&std::fs::read_to_string(file).map_err(|e| e.to_string())?)
    })
    .transpose()
}
impl Scene {
    fn reset_world(
        &mut self,
        config: ExplosionConfig,
        layout: Option<RangeLayout>,
        validator: &sim_core::blast::placement::Validator,
    ) -> Result<(), String> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| "Scene generation exhausted".to_string())?;
        self.sim.reset_world(config, layout, validator)?;
        self.generation = generation;
        Ok(())
    }
}

fn range_sim(
    c: ExplosionConfig,
    layout: Option<&RangeLayout>,
    uncovered: bool,
    kind: Option<RangeUnitKind>,
) -> Result<BlastRange, String> {
    if let Some(layout) = layout {
        let mut layout = layout.clone();
        if uncovered {
            layout.blocks.clear();
        }
        if let Some(kind) = kind {
            for u in &mut layout.units {
                u.kind = kind;
            }
        }
        BlastRange::with_layout(c, layout)
    } else {
        Ok(BlastRange::new(c))
    }
}
fn gun_rotation(sim: &BlastRange) -> Quat {
    let d = sim.heading();
    Quat::from_rotation_y(-(d[2].to_f64_lossy() as f32).atan2(d[0].to_f64_lossy() as f32))
}
/// The turret's yaw this frame, in the picture's convention, interpolated
/// through the tick like the hull's own heading.
fn visible_gun_yaw(scene: &Scene, visual: &moving::Visual) -> f32 {
    let old = visual.turret.unwrap_or(scene.sim.turret.angle);
    let fraction = if scene.realtime && !scene.paused {
        (scene.acc * 30.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let turn = old as f32 + fx::angle::diff(scene.sim.turret.angle, old) as f32 * fraction;
    -turn * std::f32::consts::TAU / fx::angle::FULL as f32
}
fn visible_gun_rotation(scene: &Scene, visual: &moving::Visual) -> Quat {
    Quat::from_rotation_y(visible_gun_yaw(scene, visual))
}
fn muzzle_vec(sim: &BlastRange) -> Vec3 {
    Vec3::from(sim.muzzle().map(|v| v.to_f64_lossy() as f32))
}
fn visible_muzzle(scene: &Scene, visual: &moving::Visual) -> Vec3 {
    let current = moving::pose(&scene.sim.gun);
    let visible = moving::visible(&scene.sim.gun, visual.previous.last(), scene);
    let current_yaw =
        -scene.sim.turret.angle as f32 * std::f32::consts::TAU / fx::angle::FULL as f32;
    let current_rotation = render::realtime::hull_rotation(current.up, current_yaw);
    let visible_rotation =
        render::realtime::hull_rotation(visible.up, visible_gun_yaw(scene, visual));
    visible.pos
        + visible_rotation * (current_rotation.inverse() * (muzzle_vec(&scene.sim) - current.pos))
}
/// Radius of one burning voxel's flame, world units (P-22).
const FLAME_RADIUS: f32 = 2.4;
fn configure_gas(sim: &BlastRange, energy: bool, anni: bool, gas: &mut GasInput) {
    let c = if sim.battle_mode() {
        &sim.battle.shell_config
    } else {
        &sim.config
    };
    let target = sim.layout.as_ref().map_or(
        if energy {
            sim_core::blast::ENERGY_TARGET
        } else {
            sim_core::blast::TARGET
        },
        |r| r.target,
    );
    gas.params.origin = Vec4::new(
        target[0] as f32 - 120.0,
        0.0,
        target[2] as f32 - 120.0,
        c.gas_cell_units as f32,
    );
    gas.params.chemistry = Vec4::new(
        c.reaction_milli as f32 / 1000.0,
        c.cooling_milli as f32 / 1000.0,
        c.buoyancy_milli as f32 / 1000.0,
        c.gas_temperature_k as f32 / 1000.0,
    );
    gas.set_display_edge(c.gas_display_mm as f32 / voxel::unit::WORLD_MM as f32);
    gas.params.charge = Vec4::new(
        c.gas_fuel_milli as f32 / 1000.0,
        c.gas_pressure_milli as f32 / 1000.0,
        c.gas_display_mm as f32 / voxel::unit::WORLD_MM as f32,
        0.0,
    );
    gas.params.beam = (muzzle_vec(sim) + gun_rotation(sim) * Vec3::X * 2.0).extend(0.0);
    gas.params.energy = Vec4::ZERO;
    gas.params.source = Vec4::ZERO;
    gas.params.control.y = 0.0;
    gas.params.weapon.x = if sim.battle_mode() {
        0.0
    } else {
        weapon_code(energy, anni)
    };
    gas.params.weapon.z = if sim.layout.is_some() { 1.0 } else { 0.0 };
    gas.params.weapon.w = look::scene().smoke_extinction;
    gas.params.look.x = look::scene().ground_glow;
}
fn main() {
    let mut out = None;
    let mut all_frames = false;
    let mut finish = None;
    let mut delivery = Delivery::Blast;
    let mut gun_kind = None;
    let mut energy = false;
    let mut anni = false;
    let mut repeat = false;
    let mut view = 0;
    let mut camera_sweep = false;
    let mut zone = None;
    let mut requested_view = None;
    let mut patrol = None;
    let mut close_view = None;
    let mut realtime = false;
    // The live tuning menu open from the first frame (ADR-298), for a
    // capture of it.
    let mut settings = false;
    let mut stock_tuning = false;
    let mut layout_file = None;
    let mut power = 1000;
    let mut uncovered = false;
    let mut edit = false;
    let mut drive = false;
    let mut demo = false;
    let mut battle_preset = false;
    let mut battle_weapon = None;
    let mut battle_eight = false;
    let mut world_battle = false;
    let mut ai_vs_ai = false;
    let mut skirmish = None;
    let mut hud_drawer = 0;
    let mut camera_zoom = 1.0;
    let mut projectile_markers = true;
    let mut window_size = [1600, 900];
    let mut ai_rules_file = None;
    let mut attack_unit: Option<usize> = None;
    let mut go: Option<[i32; 2]> = None;
    let mut record_dir = None;
    let mut record_seconds = 10.0;
    let mut profile_output = None;
    let mut profile_seconds = 20.0;
    let mut profile_clouds = 0;
    let mut profile_gpu = false;
    let mut profile_render = false;
    let mut profile_render_tasks = false;
    let mut profile_render_spans = None;
    let mut profile_actions = false;
    let mut profile_camera_tour = false;
    let mut profile_core = false;
    let mut profile_trigger_ms = None;
    let mut navigation_workers = None;
    let mut compute_workers = None;
    let mut gas_cell_mm = None;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--profile" => {
                i += 1;
                profile_output = Some(args[i].clone());
            }
            "--profile-gpu" => profile_gpu = true,
            "--profile-render" => profile_render = true,
            "--profile-render-tasks" => profile_render_tasks = true,
            "--profile-render-spans" => {
                i += 1;
                let spans = args
                    .get(i)
                    .expect("--profile-render-spans 1..=1000000")
                    .parse::<usize>()
                    .expect("--profile-render-spans 1..=1000000");
                assert!((1..=render::timeline::MAX_SPANS).contains(&spans));
                profile_render_spans = Some(spans);
            }
            "--profile-actions" => profile_actions = true,
            "--profile-camera-tour" => profile_camera_tour = true,
            "--profile-core" => profile_core = true,
            "--profile-trigger-ms" => {
                i += 1;
                let ms = args
                    .get(i)
                    .expect("--profile-trigger-ms 1..=10000")
                    .parse::<u32>()
                    .expect("--profile-trigger-ms 1..=10000");
                assert!((1..=10000).contains(&ms), "--profile-trigger-ms 1..=10000");
                profile_trigger_ms = Some(ms);
            }
            "--compute-workers" => {
                i += 1;
                let workers = args
                    .get(i)
                    .expect("--compute-workers 1..=16")
                    .parse::<usize>()
                    .expect("--compute-workers 1..=16");
                assert!((1..=16).contains(&workers), "--compute-workers 1..=16");
                compute_workers = Some(workers);
            }
            "--nav-workers" => {
                i += 1;
                let workers = args
                    .get(i)
                    .expect("--nav-workers 1..=8")
                    .parse::<usize>()
                    .expect("--nav-workers 1..=8");
                assert!((1..=8).contains(&workers), "--nav-workers 1..=8");
                navigation_workers = Some(workers);
            }
            "--camera-sweep" => camera_sweep = true,
            "--stock-tuning" => stock_tuning = true,
            "--gas-cell-mm" => {
                i += 1;
                let mm = args[i].parse::<u32>().expect("visible gas cube edge in mm");
                assert!((100..=1600).contains(&mm));
                gas_cell_mm = Some(mm);
            }
            "--profile-clouds" => {
                i += 1;
                profile_clouds = args[i].parse::<usize>().expect("cloud count");
                assert!(profile_clouds <= 8);
            }
            "--profile-seconds" => {
                i += 1;
                profile_seconds = args[i].parse().expect("profile seconds");
            }
            "--window" => {
                i += 1;
                let (w, h) = args[i].split_once(',').expect("--window width,height");
                window_size = [w.parse().expect("width"), h.parse().expect("height")];
                assert!(window_size[0] >= 960 && window_size[1] >= 540);
            }
            "--battle" => battle_preset = true,
            "--world-battle" => world_battle = true,
            "--ai-vs-ai" => ai_vs_ai = true,
            "--battle-weapon" => {
                i += 1;
                battle_weapon = Some(match args[i].as_str() {
                    "energy" => sim_core::blast::battle::Weapon::Energy,
                    "anni" => sim_core::blast::battle::Weapon::Annihilation,
                    _ => panic!("--battle-weapon energy|anni"),
                });
            }
            "--skirmish" => {
                i += 1;
                skirmish = Some(args[i].parse::<u8>().expect("skirmish 0..=4"));
            }
            "--no-projectile-markers" => projectile_markers = false,
            "--zoom" => {
                i += 1;
                camera_zoom = args[i].parse::<f32>().expect("camera zoom factor");
                assert!(
                    camera_zoom.is_finite() && (0.3..=32.0).contains(&camera_zoom),
                    "--zoom 0.3..=32"
                );
            }
            "--hud-drawer" => {
                i += 1;
                hud_drawer = args[i].parse::<u8>().expect("drawer number");
            }
            "--ai-rules" => {
                i += 1;
                ai_rules_file = Some(args[i].clone());
            }
            "--battle-eight" => {
                battle_preset = true;
                battle_eight = true;
            }
            "--edit" => edit = true,
            "--attack-unit" => {
                i += 1;
                attack_unit = Some(
                    args[i]
                        .parse::<usize>()
                        .expect("unit number (1-based)")
                        .checked_sub(1)
                        .expect("unit numbers start at 1"),
                );
                drive = true;
            }
            "--drive" => drive = true,
            // Order the player's tank to a point at start, for scripted
            // captures such as a run through the woods (ADR-271).
            "--move" => {
                i += 1;
                let (x, z) = args[i].split_once(',').expect("--move x,z");
                go = Some([x.trim().parse().expect("x"), z.trim().parse().expect("z")]);
                drive = true;
            }
            "--drive-demo" => {
                drive = true;
                demo = true;
            }
            "--record" => {
                i += 1;
                record_dir = Some(args[i].clone());
            }
            "--record-seconds" => {
                i += 1;
                record_seconds = args[i].parse().expect("seconds");
            }
            "--zone" => {
                i += 1;
                zone = Some(args[i].parse::<usize>().expect("zone number (1-based)"));
            }
            "--range" => {
                i += 1;
                layout_file = Some(args[i].clone());
            }
            "--power" => {
                i += 1;
                power = args[i].parse().expect("power in thousandths");
                assert!((100..=4000).contains(&power));
            }
            "--open" => uncovered = true,
            "--loop" => repeat = true,
            "--close" => close_view = Some(2),
            "--view" => {
                i += 1;
                requested_view = Some(match args.get(i).map(String::as_str) {
                    Some("auto") => 0,
                    Some("overview") => 1,
                    Some("exposed") => 2,
                    Some("sheltered") => 3,
                    Some("gun") => 4,
                    Some("target") => 5,
                    Some("map") => 6,
                    _ => panic!(
                        "--view expects auto, overview, exposed, sheltered, gun, target or map"
                    ),
                });
            }
            "--patrol" => {
                i += 1;
                patrol = Some(args[i].parse().expect("patrol number"));
                assert!(patrol != Some(0), "--patrol counts routes from 1");
            }
            "--realtime" => realtime = true,
            "--settings" => settings = true,
            "--chassis" => {
                i += 1;
                gun_kind = Some(match args[i].as_str() {
                    "tank" => RangeUnitKind::Tank,
                    "walker" => RangeUnitKind::Strider,
                    "spider" => RangeUnitKind::Spider,
                    "artillery" => RangeUnitKind::Artillery,
                    "t1" => RangeUnitKind::TurretT1,
                    "t2" => RangeUnitKind::TurretT2,
                    _ => panic!("--chassis tank|walker|spider|artillery|t1|t2"),
                });
            }
            "--weapon" => {
                i += 1;
                (energy, anni) = match args.get(i).map(String::as_str) {
                    Some("energy") => {
                        delivery = Delivery::Blast;
                        (true, false)
                    }
                    Some("anni") => {
                        delivery = Delivery::Blast;
                        (true, true)
                    }
                    Some("shell") => {
                        delivery = Delivery::Blast;
                        (false, false)
                    }
                    Some("kinetic") => {
                        delivery = Delivery::Kinetic;
                        (false, false)
                    }
                    Some("laser") => {
                        delivery = Delivery::Laser;
                        (false, false)
                    }
                    Some("flame") => {
                        delivery = Delivery::Flame;
                        (false, false)
                    }
                    Some("machine-gun") => {
                        delivery = Delivery::MachineGun;
                        (false, false)
                    }
                    _ => panic!(
                        "--weapon expects shell, kinetic, energy, anni, laser, flame or machine-gun"
                    ),
                };
            }
            "--capture" | "--shots" => {
                all_frames = args[i] == "--capture";
                i += 1;
                out = Some(args.get(i).expect("output path").clone());
            }
            "--frames" => {
                i += 1;
                finish = Some(args[i].parse().expect("frame count"));
            }
            _ => panic!("unknown argument {}", args[i]),
        };
        i += 1;
    }
    if let Some(p) = &out {
        std::fs::create_dir_all(p).expect("capture directory");
    }
    // `--edit` включает паузу (`paused: edit`), а шаг симуляции выходит по
    // `scene.paused` — запись писала бы неподвижный кадр сколько угодно секунд
    // и завершалась кодом 0 с валидным timeline (ADR-269). Редактор живёт
    // в интерактивном запуске, где паузу снимает человек.
    assert!(
        record_dir.is_none() || (out.is_none() && !repeat && !edit),
        "--record needs one realtime session, without --capture/--shots/--loop/--edit"
    );
    let recording = Recording::new(record_dir, record_seconds).expect("record setup");
    realtime |= out.is_none();
    assert!(
        !(realtime && all_frames),
        "--realtime supports --shots; use offline capture for a gap-free movie"
    );
    let mut layout = read_layout(layout_file.as_deref()).expect("range file");
    if let Some(id) = zone {
        let r = layout.as_mut().expect("--zone needs --range");
        let z = r
            .zones
            .get(id.checked_sub(1).expect("zones start at 1"))
            .expect("zone in layout");
        r.gun = z.spawn;
        r.target = z.target;
        view = 1;
    }
    assert!(!edit || layout.is_some(), "--edit needs --range");
    if let Some(kind) = gun_kind {
        layout.as_mut().expect("--chassis needs --range").gun_kind = kind;
    }
    let mut workshop = Workshop::new(layout, edit);
    if battle_preset {
        workshop
            .battle_preset(config(false, false))
            .expect("battle preset needs the kilometre range");
        workshop.pending = None;
        if battle_eight {
            let layout = workshop.layout.as_mut().expect("battle layout");
            let more = layout
                .units
                .iter()
                .cloned()
                .map(|mut u| {
                    u.at[2] += 90;
                    u
                })
                .collect::<Vec<_>>();
            layout.units.extend(more);
        }
        workshop.editing = edit;
        view = 1;
        repeat = false;
        energy = false;
        anni = false;
        drive = true;
    }
    let mut battle_briefing = String::new();
    if world_battle {
        battle_briefing = workshop
            .battle_world(config(false, false))
            .expect("world battle needs the kilometre range");
        workshop.pending = None;
        workshop.editing = edit;
        battle_preset = true;
        drive = true;
        energy = false;
        anni = false;
        view = 1;
    }
    if let Some(id) = skirmish {
        battle_briefing = workshop
            .skirmish(id, config(false, false))
            .expect("valid skirmish");
        workshop.pending = None;
        workshop.editing = edit;
        battle_preset = true;
        drive = true;
        energy = false;
        anni = false;
        view = 1;
    }
    let mut hud = panel::Panel::new(drive, demo);
    hud.briefing = battle_briefing;
    hud.battle_name = if world_battle {
        if ai_vs_ai {
            "Война за базы · ИИ × ИИ".into()
        } else {
            "Война за базы".into()
        }
    } else if skirmish.is_some() {
        workshop
            .sector()
            .map_or_else(String::new, |z| z.name.clone())
    } else if battle_preset {
        "Бой 2×2 / расстановка".into()
    } else if workshop.layout.as_ref().is_some_and(|layout| layout.battle) {
        workshop
            .sector()
            .map_or_else(|| "Свой бой".into(), |zone| zone.name.clone())
    } else {
        String::new()
    };
    hud.drawer = hud_drawer;
    hud.projectile_markers = projectile_markers;
    workshop.zoom = camera_zoom;
    let mut sim = range_sim(
        weapon_config(energy, anni, power, delivery),
        workshop.layout.as_ref(),
        uncovered,
        None,
    )
    .expect("valid range");
    if ai_vs_ai {
        assert!(world_battle, "--ai-vs-ai requires --world-battle");
        assert!(sim.set_ai_vs_ai(true));
    }
    if let Some(file) = ai_rules_file {
        let source = std::fs::read_to_string(&file).expect("--ai-rules file");
        let tuning = modding::battle_ai::Tuning::from_ron(&source).expect("--ai-rules RON");
        sim.configure_enemy_ai(tuning).expect("valid AI rules");
    }
    if let Some(weapon) = battle_weapon {
        assert!(
            sim.battle_mode(),
            "--battle-weapon requires a battle layout"
        );
        for id in 0..sim.units.len() {
            if sim.allied(id) && sim.units[id].kind == RangeUnitKind::Tank {
                assert!(sim.set_battle_weapon(id, Some(weapon)));
            }
        }
    }
    if battle_preset && !edit {
        sim.start_battle().expect("two battle teams");
    }
    if let Some(mm) = gas_cell_mm {
        sim.config.gas_display_mm = mm;
    }
    if drive {
        sim.control();
        if !world_battle {
            view = 1;
        }
    }
    if let Some(p) = go {
        assert!(sim.order_move(p), "--move: no route to {p:?}");
    }
    if let Some(id) = attack_unit {
        assert!(
            !battle_preset && id < sim.units.len(),
            "--attack-unit needs a range target"
        );
        assert!(
            sim.order_attack(id),
            "--attack-unit: target cannot be attacked"
        );
    }
    if edit {
        repeat = false;
        view = 1;
    }
    // Один ярус приоритета на оба написания камеры: `--close` — то же, что
    // `--view exposed`, и оба сильнее камеры режима. Порядок ключей в строке
    // ничего не решает; при обоих побеждает явный `--view` (ADR-268).
    view = requested_view.or(close_view).unwrap_or(view);
    // Укрытый танк, орудие и мишень существуют только внутри разложенного
    // полигона. Без `--range` ключ разбирался и молча ничего не делал —
    // запись уходила с автоматической камеры и кодом возврата 0 (ADR-268).
    assert!(
        !(3..=6).contains(&view) || workshop.layout.is_some(),
        "--view sheltered/gun/target/map needs --range"
    );
    // Раскладка держит до восьми маршрутов, а ключ `--view target` сам по себе
    // называет только первый. `--patrol` называет остальные — и отказывает
    // сразу, а не снимает молча чужую мишень (ADR-269).
    if let Some(n) = patrol {
        let routes = workshop
            .layout
            .as_ref()
            .and_then(|l| l.course.as_ref())
            .map_or(0, |c| c.patrols.len());
        assert!(
            n <= routes,
            "--patrol {n} needs a range with at least {n} route(s), found {routes}"
        );
    }
    let patrol = patrol.unwrap_or(1);
    let finish = finish.unwrap_or(if energy { 420 } else { 330 });
    assert!(
        profile_clouds == 0 || profile_output.is_some(),
        "--profile-clouds requires --profile"
    );
    assert!(
        !profile_gpu || profile_output.is_some(),
        "--profile-gpu requires --profile"
    );
    assert!(
        !profile_render || profile_output.is_some(),
        "--profile-render requires --profile"
    );
    assert!(
        profile_render_spans.is_none() || profile_render,
        "--profile-render-spans requires --profile-render"
    );
    assert!(
        !profile_render_tasks || (profile_render && cfg!(feature = "render-tasks")),
        "--profile-render-tasks requires --profile-render and a build with --features render-tasks"
    );
    let mut profiler = profile::Profile::new(profile_output.clone(), profile_seconds);
    if profile_output.is_some() {
        println!("PROFILE compute_override={compute_workers:?}");
        println!(
            "PROFILE render_tasks_build={}",
            cfg!(feature = "render-tasks")
        );
    }
    profiler.navigation_workers = navigation_workers;
    profiler.configure_navigation(&mut sim);
    profiler.clouds = profile_clouds;
    profiler.gpu = profile_gpu;
    assert!(
        !profile_core || profile_output.is_some(),
        "--profile-core requires --profile"
    );
    profiler.core = profile_core;
    if let Some(ms) = profile_trigger_ms {
        profiler.configure_trigger(ms);
    }
    assert!(
        !profile_actions || profile_output.is_some(),
        "--profile-actions requires --profile"
    );
    profiler.actions = profile_actions;
    assert!(
        !profile_camera_tour || (profile_output.is_some() && !camera_sweep),
        "--profile-camera-tour requires --profile and excludes --camera-sweep"
    );
    profiler.camera_tour = profile_camera_tour;
    let mut gas = GasInput::default();
    gas.timings
        .enabled
        .store(profile_output.is_some(), Ordering::Relaxed);
    fill_terrain(&sim, &mut gas);
    gas.params.sun = (-Vec3::from(look::sun_dir())).extend(1.8);
    gas.params.sky = Vec4::new(0.075, 0.105, 0.145, 0.00065);
    configure_gas(&sim, energy, anni, &mut gas);
    let mut app = App::new();
    #[cfg(feature = "render-tasks")]
    if profile_render_tasks {
        app.insert_resource(render::timeline::tasks::Tasks::new(
            profile::render_sample,
            profile_seconds,
        ));
    }
    let log_plugin = bevy::log::LogPlugin::default();
    #[cfg(feature = "render-tasks")]
    let log_plugin = bevy::log::LogPlugin {
        custom_layer: render::timeline::tasks::layer,
        ..log_plugin
    };
    app.add_plugins(
        DefaultPlugins
            .set(compute::plugin(compute_workers))
            .set(log_plugin)
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "RTS VOX / BLAST RANGE — Space: pause · R: replay".into(),
                    resolution: bevy::window::WindowResolution::new(window_size[0], window_size[1])
                        .with_scale_factor_override(1.0),
                    present_mode: if realtime && profile_output.is_none() {
                        PresentMode::AutoVsync
                    } else {
                        PresentMode::AutoNoVsync
                    },
                    ..default()
                }),
                ..default()
            }),
    );
    if profile_render {
        app.add_plugins(render::timeline::TimelinePlugin {
            sample: profile::render_sample,
            gas_details: true,
            span_limit: profile_render_spans
                .unwrap_or_else(|| render::timeline::span_limit_for_seconds(profile_seconds)),
        });
    }
    if profile_gpu {
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
    }
    app.insert_resource(ClearColor(Color::srgb(0.12, 0.15, 0.19)))
        .insert_resource(DirectionalLightShadowMap {
            size: look::scene().shadow_map,
        })
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.65, 0.78, 1.0),
            brightness: look::scene().ambient_brightness,
            ..default()
        })
        .insert_resource(Scene {
            sim,
            generation: 0,
            energy,
            anni,
            paused: edit,
            frame: 0,
            finish,
            out,
            all_frames,
            wait: 0,
            captured: 0,
            acc: 0.0,
            repeat,
            view,
            camera_sweep,
            patrol,
            previous_shell: None,
            fps: 60.0,
            step_us: 0,
            flame: 0,
            realtime,
            live_elapsed: 0.0,
            last_capture: 0,
            layout_file,
            power,
            uncovered,
            unit_override: None,
            skip_delta: true,
            notice: String::new(),
        })
        .insert_resource(hud)
        .insert_resource(knobs::Overrides::new(settings, stock_tuning))
        .init_resource::<moving::Visual>()
        .init_resource::<fire::Effects>()
        .init_resource::<workbench::ShieldFlashes>()
        .insert_resource(workshop)
        .insert_resource(recording)
        .insert_resource(profiler)
        .insert_resource(gas)
        .init_resource::<streaming::Streaming>()
        .init_resource::<overview::Distant>()
        .init_resource::<ui_snapshot::UiSnapshot>()
        .add_plugins((GasPlugin, OcclusionPlugin))
        .add_systems(Startup, (setup, workbench::setup, panel::setup))
        .add_systems(
            Update,
            (
                profile::begin,
                profile::actions,
                (
                    panel::actions,
                    panel::minimap::input,
                    panel::scroll,
                    knobs::actions,
                    workbench::input,
                    advance,
                    panel::scenario_progress,
                    panel::minimap::update_terrain,
                    profile::stamp,
                    gun_art::refresh,
                    orient_gun,
                    witness::update,
                    profile::stamp,
                    moving::animate,
                    fragments::draw,
                    bodies::draw,
                    profile::stamp,
                )
                    .chain(),
                (
                    framing,
                    fire::draw,
                    weapon_art,
                    animate_energy,
                    overview::sync,
                    overview::atmosphere,
                    streaming::camera,
                    workbench::camera,
                    workbench::ghost::update,
                    workbench::overlay,
                    (
                        ui_snapshot::publish,
                        panel::refresh,
                        panel::minimap::refresh_markers,
                        panel::minimap::update_view,
                        panel::hud::refresh,
                        knobs::refresh,
                    )
                        .chain(),
                    panel::hud::refresh_selection_box,
                    panel::release_keys,
                    record,
                    capture,
                    profile::render_context,
                    profile::end,
                )
                    .chain(),
            )
                .chain(),
        )
        .run();
}
fn surface_material(sim: &BlastRange, p: [i32; 3], m: u16) -> u32 {
    let work = sim.surface_work.get(&p).copied().unwrap_or(0);
    if work == 0 {
        return u32::from(m);
    }
    let cost = sim_core::blast::material_work(m, &sim.config).max(1);
    surface_tint(m, work, cost)
}
fn surface_tint(m: u16, work: u32, cost: u32) -> u32 {
    if work == 0 {
        return m as u32;
    }
    let level = (u64::from(work) * 3 / u64::from(cost.max(1))).min(2) as u32;
    if m == 3 {
        9 + level
    } else if m == 2 || m == 6 {
        12 + level
    } else {
        m as u32
    }
}
/// The window of terrain the picture keeps, in world units (ADR-318).
/// Wide enough for the whole reach of the ray march, which stops after
/// about a thousand voxels: a narrower window would cut the view short
/// of what the shader can already draw.
const TERRAIN_WINDOW: [u32; 3] = [1280, 192, 1280];
/// The window's corner moves in steps of this many voxels.
const WINDOW_STEP: i32 = 8;

/// Initial terrain window before the first camera update.
fn focus(sim: &BlastRange) -> [i32; 3] {
    let p = sim.gun.motion.pos;
    [p[0], p[1], p[2]].map(|v| v.to_int_trunc())
}
fn wanted_window(sim: &BlastRange) -> TerrainWindow {
    TerrainWindow::around(sim.size(), focus(sim), TERRAIN_WINDOW, WINDOW_STEP)
}

fn fill_terrain(sim: &BlastRange, gas: &mut GasInput) {
    let window = wanted_window(sim);
    gas.terrain_words = Arc::new(vec![0u32; window.words()]);
    gas.terrain_column_top = vec![0; (window.dims[0] * window.dims[2]) as usize];
    write_window(sim, gas, window, None);
}

/// Write every voxel of `window` that `had` did not already hold, and
/// put the moving bodies back on top. Without an earlier window that is
/// the whole volume; with one it is only the slabs that came into view.
fn write_window(
    sim: &BlastRange,
    gas: &mut GasInput,
    window: TerrainWindow,
    had: Option<TerrainWindow>,
) {
    write_window_for(sim, 0, gas, window, had);
}
fn write_window_for(
    sim: &BlastRange,
    generation: u64,
    gas: &mut GasInput,
    window: TerrainWindow,
    had: Option<TerrainWindow>,
) {
    write_window_in_for(
        sim,
        generation,
        gas,
        window,
        had,
        had.and_then(|_| bevy::tasks::ComputeTaskPool::try_get().map(|p| &**p)),
    );
}

#[cfg(test)]
fn write_window_in(
    sim: &BlastRange,
    gas: &mut GasInput,
    window: TerrainWindow,
    had: Option<TerrainWindow>,
    pool: Option<&bevy::tasks::TaskPool>,
) {
    write_window_in_for(sim, 0, gas, window, had, pool);
}
fn write_window_in_for(
    sim: &BlastRange,
    generation: u64,
    gas: &mut GasInput,
    window: TerrainWindow,
    had: Option<TerrainWindow>,
    pool: Option<&bevy::tasks::TaskPool>,
) {
    let words = Arc::make_mut(&mut gas.terrain_words);
    let tops = &mut gas.terrain_column_top;
    tops.resize((window.dims[0] * window.dims[2]) as usize, 0);
    // Above a column's ceiling there is nothing to ask about (ADR-313):
    // the volume is 192 cells tall and the ground stands at twenty to
    // sixty, so four fifths of every column is sky and costs a nibble
    // rather than a material.
    let mut highest = if had.is_some() {
        gas.params.terrain_origin.w as i32
    } else {
        0
    };
    let mut dirty: Vec<std::ops::Range<usize>> = Vec::new();
    let mut write = |xs: std::ops::Range<i32>, zs: std::ops::Range<i32>, highest: &mut i32| {
        let (top, patches) = streaming::write_slab(
            sim,
            generation,
            window,
            words,
            tops,
            xs.clone(),
            zs.clone(),
            had.is_some(),
            pool,
        );
        *highest = (*highest).max(top);
        dirty.extend(patches);
    };
    match had {
        None => write(window.span(0), window.span(2), &mut highest),
        Some(had) => {
            // At most one slab an axis comes into view. The slab along x
            // spans the new window's depth; the one along z covers only
            // the part of it the first has not already written.
            let fresh = |new: std::ops::Range<i32>, old: std::ops::Range<i32>| {
                if new.start >= old.end || new.end <= old.start {
                    new
                } else if new.start < old.start {
                    new.start..old.start
                } else {
                    old.end..new.end
                }
            };
            let (nx, nz) = (window.span(0), window.span(2));
            let (ox, oz) = (had.span(0), had.span(2));
            let sx = fresh(nx.clone(), ox.clone());
            let sz = fresh(nz.clone(), oz);
            if !sx.is_empty() {
                write(sx, nz, &mut highest);
            }
            if !sz.is_empty() {
                let rest = nx.start.max(ox.start)..nx.end.min(ox.end);
                if !rest.is_empty() {
                    write(rest, sz, &mut highest);
                }
            }
        }
    }
    // The unit remains PBR geometry. Its voxel body only supplies occupancy to
    // the terrain's shadow rays (reserved presentation material 15): the root
    // and every segment in the frames the simulation posed them in (ADR-282).
    if had.is_none() {
        for i in moving::cells(sim, &window) {
            words[i / 8] = (words[i / 8] & !(15 << ((i % 8) * 4))) | (15 << ((i % 8) * 4));
        }
    }
    gas.window = window;
    gas.terrain_revision += 1;
    // Occupancy restores moving bodies after a shift; only the newly exposed
    // rows are uploaded, not the 157 MB volume.
    if had.is_none() {
        gas.terrain_dirty = std::iter::once(0..gas.terrain_words.len()).collect();
    } else {
        gas.terrain_dirty.extend(dirty);
    }
    let d = window.dims;
    gas.params.terrain = Vec4::new(d[0] as f32, d[1] as f32, d[2] as f32, 0.0);
    let o = window.origin;
    // The `w` of the corner is the top of everything the window holds
    // (ADR-321): the march above it meets nothing. A slab that came into
    // view can only raise it; a dig can only lower it, and a stale top is
    // merely a march that skips a little less.
    let ceiling = highest.max(sim.gun.at[1] + 64).min(window.span(1).end) as f32;
    gas.params.terrain_origin = Vec4::new(o[0] as f32, o[1] as f32, o[2] as f32, ceiling);
}

/// Move the window when the camera has left the middle of it, writing only
/// what came into view (ADR-318).
#[cfg(test)]
fn recentre_terrain(sim: &BlastRange, gas: &mut GasInput, focus: [i32; 3]) -> bool {
    let had = gas.window;
    let want = TerrainWindow::around(sim.size(), focus, TERRAIN_WINDOW, WINDOW_STEP);
    if want != had {
        write_window(sim, gas, want, Some(had));
        return true;
    }
    false
}
/// Publish authoritative terrain and surface damage to the visible window.
fn apply_terrain(sim: &BlastRange, gas: &mut GasInput, unit_start: &[usize]) {
    let unit_edits: Vec<_> = sim
        .units
        .iter()
        .enumerate()
        .flat_map(|(id, u)| {
            u.removed[unit_start[id]..]
                .iter()
                .map(|p| u.world_pos(p.map(usize::from)))
        })
        .collect();
    if sim.world.edits().is_empty() && unit_edits.is_empty() && sim.surface_changed.is_empty() {
        return;
    }
    // Distant columns must also receive edits outside the detailed window.
    if gas.params.distant.z > 0.0 {
        let step = gas.params.distant.z as u32;
        for e in sim.world.edits() {
            gas.distant_edits.insert([e.x / step, e.z / step]);
        }
        for p in &sim.surface_changed {
            if p[0] >= 0 && p[2] >= 0 {
                gas.distant_edits
                    .insert([p[0] as u32 / step, p[2] as u32 / step]);
            }
        }
    }
    // Most crater edits are nowhere near a unit. Its enclosing box also
    // includes the turned turret and articulated segments, so rejecting here
    // preserves every actual voxel hit without transforming empty space.
    let unit_boxes: Vec<_> = sim.units.iter().map(|u| u.bounds()).collect();
    let window = gas.window;
    let words = Arc::make_mut(&mut gas.terrain_words);
    let mut dirty = Vec::new();
    let edits = sim
        .world
        .edits()
        .iter()
        .map(|e| ([e.x as i32, e.y as i32, e.z as i32], e.mat))
        .chain(
            sim.surface_changed
                .iter()
                .map(|p| (*p, sim_core::blast::material(&sim.world, *p))),
        )
        .chain(unit_edits.iter().map(|p| {
            let q = *p;
            (q, sim_core::blast::material(&sim.world, q))
        }));
    for (p, mat) in edits {
        let Some(i) = window.index(p) else {
            continue;
        };
        let shift = (i % 8) * 4;
        let m = if sim
            .units
            .iter()
            .zip(&unit_boxes)
            .any(|(u, (lo, hi))| (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a]) && u.solid(p))
        {
            15
        } else {
            surface_material(sim, p, mat)
        };
        if m != 0 {
            gas.params.terrain_origin.w = gas.params.terrain_origin.w.max((p[1] + 1) as f32);
            let slot = (p[0] as u32 % window.dims[0]
                + window.dims[0] * (p[2] as u32 % window.dims[2])) as usize;
            gas.terrain_column_top[slot] = gas.terrain_column_top[slot].max((p[1] + 1) as u16);
        }
        words[i / 8] = (words[i / 8] & !(15 << shift)) | (m << shift);
        dirty.push(i / 8);
    }
    // Craters touch the same packed word repeatedly; batch ordering avoids
    // maintaining a tree for every edit while preserving sorted dirty spans.
    dirty.sort_unstable();
    dirty.dedup();
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for i in dirty {
        if let Some(r) = spans.last_mut() {
            if i <= r.end + 16 {
                r.end = i + 1;
                continue;
            }
        }
        spans.push(i..i + 1);
    }
    gas.terrain_dirty.extend(spans);
    gas.terrain_revision += 1;
}
fn mesh(b: &MeshBuf) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.pos.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, b.col.clone())
    .with_inserted_indices(Indices::U32(b.idx.clone()));
    if b.uv.len() == b.pos.len() && !b.uv.is_empty() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, b.uv.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, b.tan.clone());
    }
    mesh
}
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut mediums: ResMut<Assets<ScatteringMedium>>,
    scene: Res<Scene>,
) {
    let finishes = render::unit_surface::materials(&mut images, &mut materials);
    commands.insert_resource(WitnessArt {
        materials: finishes.clone(),
        burned_materials: wreck::materials(&finishes, &mut materials),
        skins: Arc::new([
            SkinBindings::new(&voxel::unit::tank()),
            SkinBindings::new(&voxel::unit::strider()),
            SkinBindings::new(&voxel::unit::spider()),
            SkinBindings::new(&voxel::unit::artillery()),
            SkinBindings::new(&voxel::unit::turret_t1()),
            SkinBindings::new(&voxel::unit::turret_t2()),
            SkinBindings::new(&voxel::unit::command_base()),
            SkinBindings::new(&voxel::unit::artillery_t2()),
            SkinBindings::new(&voxel::unit::artillery_turret_t2()),
        ]),
        pristine: Arc::new([
            voxel::unit::tank(),
            voxel::unit::strider(),
            voxel::unit::spider(),
            voxel::unit::artillery(),
            voxel::unit::turret_t1(),
            voxel::unit::turret_t2(),
            voxel::unit::command_base(),
            voxel::unit::artillery_t2(),
            voxel::unit::artillery_turret_t2(),
        ]),
        seen: Vec::new(),
        pending: Vec::new(),
        emitted: Vec::new(),
        geometry: default(),
        next_mesh: 0,
        generation: scene.generation,
    });
    gun_art::spawn(&mut commands, &mut meshes, &finishes, &scene);
    commands.insert_resource(gun_art::Chassis(scene.sim.gun.kind));
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let coil = materials.add(StandardMaterial {
        base_color: Color::srgb(0.05, 0.20, 0.38),
        emissive: LinearRgba::new(0.10, 0.38, 0.85, 1.0),
        metallic: 0.7,
        perceptual_roughness: 0.24,
        ..default()
    });
    gun_art::emitters(&mut commands, &scene, &cube, &coil);
    let ember = materials.add(StandardMaterial {
        base_color: if scene.energy {
            Color::srgb(0.15, 0.5, 1.0)
        } else {
            Color::srgb(1.0, 0.6, 0.12)
        },
        emissive: if scene.energy {
            LinearRgba::new(3.0, 4.5, 7.0, 1.0)
        } else {
            LinearRgba::new(32.0, 6.4, 0.2, 1.0)
        },
        ..default()
    });
    let rock = materials.add(StandardMaterial {
        base_color: Color::srgb(0.32, 0.29, 0.24),
        perceptual_roughness: 1.0,
        ..default()
    });
    commands.spawn((
        Mesh3d(cube.clone()),
        MeshMaterial3d(ember.clone()),
        Transform::from_xyz(0.0, -1000.0, 0.0).with_scale(Vec3::new(3.0, 1.4, 1.4)),
        Shell,
        NoFrustumCulling,
    ));
    commands.spawn((
        Mesh3d(cube.clone()),
        MeshMaterial3d(ember.clone()),
        Transform::from_xyz(0.0, -1000.0, 0.0),
        MuzzleFlash,
        NoFrustumCulling,
    ));
    commands.insert_resource(Art {
        cube,
        rock,
        ember,
        coil,
    });
    presentation::daylight(&mut commands, &mut mediums, sim_core::blast::FLOOR as f32);
    commands.spawn((
        PointLight {
            color: if scene.energy {
                Color::srgb(0.60, 0.78, 1.0)
            } else {
                Color::srgb(1.0, 0.4, 0.08)
            },
            intensity: 0.0,
            range: 240.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(232.0, 34.0, 336.0),
        BlastLight,
    ));
    let camera = commands
        .spawn((
            Camera3d::default(),
            GasCamera,
            Msaa::Off,
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                    viewport_height: 160.0,
                },
                near: 1.0,
                far: 1200.0,
                ..OrthographicProjection::default_3d()
            }),
            Transform::from_xyz(48.0, 240.0, 593.0)
                .looking_at(Vec3::new(207.0, 34.0, 336.0), Vec3::Y),
        ))
        .id();
    presentation::camera(&mut commands, camera, 366.0);
    // One live-range grade for both weapons. Changing the projectile must not
    // silently replace the established camera stack with default grading.
    commands
        .entity(camera)
        .insert((
            bevy::core_pipeline::tonemapping::Tonemapping::AgX,
            presentation::grading(look::live_range_grade()),
            render::occlusion::Occlusion {
                radius: 4.0,
                strength: 0.32,
                bias: 0.06,
                power: 1.2,
            },
        ))
        .remove::<bevy::post_process::dof::DepthOfField>();
}

/// Rebuild only after authoritative geometry changes, including its exposed cuts.
// Explicit ECS inputs and disjoint filters describe the visual update borrows.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn advance(
    mut commands: Commands,
    mut scene: ResMut<Scene>,
    mut workshop: ResMut<Workshop>,
    mut visual: ResMut<moving::Visual>,
    panel: Res<panel::Panel>,
    (recording, mut profile, mut fire_fx, mut streaming, _main_thread): (
        Res<Recording>,
        ResMut<profile::Profile>,
        ResMut<fire::Effects>,
        ResMut<streaming::Streaming>,
        // Keep the synchronous tick coordinator out of the shared compute
        // executor used by pipelined render preparation. Physics/navigation
        // still fan out into their own persistent pools before ordered join.
        bevy::ecs::system::NonSendMarker,
    ),
    mut gas: ResMut<GasInput>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    art: Res<Art>,
    mut gun: Query<(&Gun, &mut Transform), (Without<Shell>, Without<Ejecta>, Without<BlastLight>)>,
    mut shell: Query<
        &mut Transform,
        (
            With<Shell>,
            Without<Gun>,
            Without<Ejecta>,
            Without<BlastLight>,
        ),
    >,
    mut light: Query<
        (&mut PointLight, &mut Transform),
        (
            With<BlastLight>,
            Without<Gun>,
            Without<Shell>,
            Without<Ejecta>,
        ),
    >,
    mut debris: Query<
        (Entity, &mut Transform, &mut Ejecta),
        (Without<Shell>, Without<Gun>, Without<BlastLight>),
    >,
    mut flash: Query<
        &mut Transform,
        (
            With<MuzzleFlash>,
            Without<Gun>,
            Without<Shell>,
            Without<Ejecta>,
            Without<BlastLight>,
        ),
    >,
    mut exit: MessageWriter<AppExit>,
) {
    gas.begin_frame();
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
    if !workshop.editing && keys.just_pressed(KeyCode::KeyF) && scene.sim.refire() {
        scene.paused = false;
        scene.notice.clear();
        println!(
            "REFIRE tick={} fire={}",
            scene.sim.tick, scene.sim.fire_tick
        );
    }
    if !workshop.editing && keys.just_pressed(KeyCode::Space) {
        scene.paused = !scene.paused;
    }
    if !workshop.editing && keys.just_pressed(KeyCode::Digit1) {
        scene.view = 1;
    }
    if !workshop.editing && keys.just_pressed(KeyCode::Digit2) {
        scene.view = 2;
    }
    if !workshop.editing && keys.just_pressed(KeyCode::Digit3) {
        scene.view = 3;
    }
    if !workshop.editing && keys.just_pressed(KeyCode::KeyC) {
        scene.view = 0;
    }
    let mut energy = scene.energy;
    let mut anni = scene.anni;
    let delivery = if keys.just_pressed(KeyCode::KeyW) {
        Delivery::Blast
    } else {
        scene.sim.config.delivery
    };
    let mut power = scene.power;
    let mut uncovered = scene.uncovered;
    let mut kind = scene.unit_override;
    let mut changed = keys.just_pressed(KeyCode::F5);
    if keys.just_pressed(KeyCode::KeyW) {
        // Shell, energy bolt, annihilation bolt, and round again.
        (energy, anni) = match (energy, anni) {
            (false, _) => (true, false),
            (true, false) => (true, true),
            (true, true) => (false, false),
        };
        changed = true;
    }
    if keys.just_pressed(KeyCode::KeyP) {
        if let Some(r) = &scene.sim.layout {
            let i = r.power_milli.iter().position(|p| *p == power).unwrap_or(0);
            power = r.power_milli[(i + 1) % r.power_milli.len()];
            changed = true;
        }
    }
    if !workshop.editing && scene.layout_file.is_some() && keys.just_pressed(KeyCode::KeyB) {
        uncovered = !uncovered;
        changed = true;
    }
    if !workshop.editing && scene.layout_file.is_some() && keys.just_pressed(KeyCode::KeyT) {
        kind = Some(if kind == Some(RangeUnitKind::Strider) {
            RangeUnitKind::Tank
        } else {
            RangeUnitKind::Strider
        });
        changed = true;
    }
    if changed
        || workshop.pending.is_some()
        || keys.just_pressed(KeyCode::KeyR)
        || keys.just_pressed(KeyCode::Enter)
        || (scene.repeat && !scene.paused && scene.sim.tick >= scene.finish && scene.out.is_none())
    {
        if scene.generation == u64::MAX {
            scene.notice = "Scene generation exhausted".into();
            return;
        }
        let reloaded = if keys.just_pressed(KeyCode::F5) {
            match read_layout(scene.layout_file.as_deref()) {
                Ok(r) => r,
                Err(e) => {
                    scene.notice = e;
                    return;
                }
            }
        } else {
            workshop.layout.clone()
        };
        let authored = workshop.pending.take();
        if authored.is_some() || workshop.editing {
            uncovered = false;
            kind = None;
        }
        let mut keep_terrain = false;
        let mut rebuild = |config, layout: RangeLayout| {
            keep_terrain = !streaming.loading()
                && scene.sim.same_authored_terrain(&layout)
                && scene.sim.world.touched_chunks() == 0
                && scene.sim.surface_work.is_empty();
            scene.reset_world(config, Some(layout), &workshop.validator)
        };
        let next = if let Some((config, layout)) = authored.filter(|_| !changed) {
            rebuild(config, layout)
        } else if let Some(mut layout) = reloaded.clone() {
            if uncovered {
                layout.blocks.clear();
            }
            if let Some(kind) = kind {
                for u in &mut layout.units {
                    u.kind = kind;
                }
            }
            rebuild(weapon_config(energy, anni, power, delivery), layout)
        } else {
            scene.reset_world(
                weapon_config(energy, anni, power, delivery),
                None,
                &workshop.validator,
            )
        };
        let Ok(()) = next else {
            scene.notice = format!("Layout rejected: {}", next.err().unwrap());
            eprintln!("{}", scene.notice);
            return;
        };
        profile.configure_navigation(&mut scene.sim);
        scene.notice.clear();
        if keys.just_pressed(KeyCode::F5) {
            *workshop = Workshop::new(reloaded, workshop.editing);
        }
        if keys.just_pressed(KeyCode::Enter) {
            workshop.editing = false;
        }
        fire_fx.clear();
        if panel.drive {
            scene.sim.control();
        }
        moving::reset(&mut visual);
        scene.energy = energy;
        scene.anni = anni;
        scene.power = power;
        scene.uncovered = uncovered;
        scene.unit_override = kind;
        scene.frame = 0;
        scene.wait = 0;
        scene.acc = 0.0;
        scene.paused = workshop.editing;
        scene.previous_shell = None;
        scene.live_elapsed = 0.0;
        scene.last_capture = 0;
        scene.skip_delta = true;
        gas.params.control.z = 0.0;
        gas.atmosphere.clear();
        configure_gas(&scene.sim, scene.energy, scene.anni, &mut gas);
        if keep_terrain {
            moving::occupancy(&scene.sim, &mut gas, &mut visual);
        } else {
            streaming.invalidate();
        }
        let rotation = gun_rotation(&scene.sim);
        let pivot = Vec3::from(GUN.map(|v| v as f32));
        for (g, mut tr) in &mut gun {
            *tr = g.base;
            tr.translation = pivot + rotation * (g.base.translation - pivot);
            tr.rotation = rotation * g.base.rotation;
        }
        for mut tr in &mut shell {
            tr.translation = Vec3::new(0.0, -1000.0, 0.0);
        }
        for mut tr in &mut flash {
            tr.translation = Vec3::new(0.0, -1000.0, 0.0);
        }
        for (e, _, _) in &debris {
            commands.entity(e).despawn();
        }
        return;
    }
    if !gas.ready.load(Ordering::Relaxed) {
        return;
    }
    if streaming.loading()
        || (scene.out.is_some() && gas.params.distant.z > 0.0 && gas.params.distant.w < 0.5)
    {
        scene.skip_delta = true;
        return;
    }
    if !recording.running() || !profile.running() {
        scene.skip_delta = false;
        return;
    }
    if scene.paused || (scene.out.is_some() && scene.frame >= scene.finish) {
        return;
    }
    if scene.skip_delta {
        scene.skip_delta = false;
        return;
    }
    let steps = if scene.realtime {
        // Retain the fractional remainder and run every elapsed fixed tick.
        // At low FPS the old single-step path silently slowed the whole scene.
        scene.acc += time.delta_secs();
        // Bounded debt (analysis 240, B5): a long overload slows the scene
        // rather than replaying the backlog as a later fast-forward.
        scene.acc = scene
            .acc
            .min(render::realtime::MAX_DEBT_TICKS as f32 / 30.0);
        scene.live_elapsed += time.delta_secs();
        if scene.acc < 1.0 / 30.0 {
            return;
        }
        (scene.acc * 30.0).floor().min(8.0) as u32
    } else {
        1
    };
    // A long tick must not be multiplied by an eight-tick catch-up burst.
    // Debt is subtracted only for completed work and stays for later frames.
    let batch_started = std::time::Instant::now();
    for batch_step in 0..steps {
        if scene.realtime
            && batch_step > 0
            && batch_started.elapsed() >= std::time::Duration::from_millis(4)
        {
            break;
        }
        if scene.realtime {
            scene.acc -= 1.0 / 30.0;
        }
        let before = scene.sim.impact;
        let unit_start: Vec<_> = scene.sim.units.iter().map(|u| u.removed.len()).collect();
        scene.previous_shell = scene
            .sim
            .shell
            .map(|v| Vec3::from(v.map(|f| f.raw() as f32 / 4294967296.0)));
        if scene.sim.tick == scene.sim.fire_tick {
            scene.previous_shell = Some(muzzle_vec(&scene.sim));
        }
        moving::before(&scene.sim, &mut visual, &gas.window);
        fire_fx.before(&scene.sim);
        if panel.demo {
            if scene.sim.tick == 20 {
                scene.sim.order_move([235, 365]);
            }
            if scene.sim.tick == 220 {
                scene.sim.order_attack(2);
            }
        }
        let started = std::time::Instant::now();
        let generation = scene.generation;
        profile.step(&mut scene.sim, generation);
        profile.record_tick(started.elapsed());
        for impact in scene
            .sim
            .battle
            .impacts
            .iter()
            .filter(|i| i.weapon.bursts())
        {
            let mut params = gas.params;
            let c = scene.sim.battle.config(impact.weapon);
            params.control.z = 1.0;
            params.control.y = 0.0;
            params.weapon.x = match impact.weapon {
                sim_core::blast::battle::Weapon::Energy => 1.0,
                sim_core::blast::battle::Weapon::Annihilation => 2.0,
                _ => 0.0,
            };
            params.chemistry = Vec4::new(
                c.reaction_milli as f32 / 1000.0,
                c.cooling_milli as f32 / 1000.0,
                c.buoyancy_milli as f32 / 1000.0,
                c.gas_temperature_k as f32 / 1000.0,
            );
            params.charge = Vec4::new(
                c.gas_fuel_milli as f32 / 1000.0,
                c.gas_pressure_milli as f32 / 1000.0,
                c.gas_display_mm as f32 / voxel::unit::WORLD_MM as f32,
                0.0,
            );
            params.energy = Vec3::from(impact.at.map(|v| v as f32)).extend(0.0);
            params.beam = Vec4::ZERO;
            params.source = gas
                .surface_source(Vec3::from(impact.at.map(|v| v as f32)), Vec3::Y)
                .extend(c.gas_source_radius as f32);
            gas.atmosphere.emit_at(params, scene.sim.tick * 2);
        }
        fire_fx.capture(&scene.sim);
        let us = started.elapsed().as_micros();
        profile.measure(0, started.elapsed());
        scene.step_us = if scene.frame.is_multiple_of(30) {
            us
        } else {
            scene.step_us.max(us)
        };
        scene.frame += 1;
        let terrain_started = std::time::Instant::now();
        let patch_start = gas.terrain_dirty.len();
        apply_terrain(&scene.sim, &mut gas, &unit_start);
        let occupancy_started = std::time::Instant::now();
        moving::occupancy(&scene.sim, &mut gas, &mut visual);
        profile.measure(3, occupancy_started.elapsed());
        let observe_started = profile.camera_tour.then(std::time::Instant::now);
        streaming.observe(
            &scene.sim,
            scene.generation,
            &gas.terrain_dirty[patch_start..],
        );
        if let Some(start) = observe_started {
            profile.measure_camera(0, start.elapsed());
        }
        profile.measure(1, terrain_started.elapsed());
        if before.is_none() {
            if let Some(at) = scene.sim.impact {
                // Solid shot contacts use their own bounded spark/spall layer.
                // An empty cold gas field still adds explosion lighting/soot,
                // so do not deposit one for a non-explosive projectile.
                if scene.sim.config.delivery != Delivery::Kinetic {
                    gas.params.control.z = 1.0;
                    let point = Vec3::from(at.map(|v| v as f32));
                    let outward = scene.previous_shell.map_or(Vec3::Y, |p| p - point);
                    gas.params.source = gas
                        .surface_source(point, outward)
                        .extend(scene.sim.config.gas_source_radius as f32);
                    let params = gas.params;
                    gas.atmosphere.emit_at(params, scene.sim.impact_tick * 2);
                }
                println!("IMPACT tick={} at={at:?}", scene.sim.impact_tick);
            }
        }
        if scene.sim.impact.is_some() {
            gas.params.control.y = (scene.sim.tick - scene.sim.impact_tick) as f32 * 2.0;
        }
        // Fire is decided by the authoritative heat (ADR-277); the image
        // follows it. One small plume a tick where a voxel lights, and one
        // where a voxel keeps burning, walking the fire voxel by voxel so
        // a tree alight shows a flame and not a single puff (P-22). The
        // flame's own chemistry is hotter and its plume smaller than the
        // weapon's, so every flame shares one region of gas.
        if let Some(p) = scene.sim.heat.ignited.first() {
            let mut params = gas.params;
            params.control.z = 1.0;
            params.weapon.x = 0.0;
            params.chemistry.w = 2.7;
            params.charge.x = 0.6;
            params.charge.y = 0.06;
            params.source = gas
                .surface_source(Vec3::from(p.map(|v| v as f32)), Vec3::Y)
                .extend(3.0);
            gas.atmosphere.emit_at(params, scene.sim.tick * 2);
        }
        let alight = scene.sim.heat.burning();
        if alight > 0 {
            scene.flame = (scene.flame + 1) % alight;
            if let Some(p) = scene.sim.heat.burning_cells().nth(scene.flame) {
                let mut params = gas.params;
                params.control.z = 1.0;
                params.weapon.x = 0.0;
                // Hot, little pressure and half the fuel: a flame, not a burst.
                params.chemistry.w = (params.chemistry.w * 1.8).max(2.2);
                params.charge = Vec4::new(
                    gas.params.charge.x * 0.5,
                    gas.params.charge.y * 0.2,
                    gas.params.charge.z * 0.5,
                    0.0,
                );
                params.source = gas
                    .surface_source(Vec3::from(p.map(|v| v as f32)), Vec3::Y)
                    .extend(FLAME_RADIUS);
                gas.atmosphere.emit_at(params, scene.sim.tick * 2);
            }
        }
        let gas_started = std::time::Instant::now();
        gas.advance(scene.sim.tick * 2);
        profile.measure(2, gas_started.elapsed());
        let age = if scene.sim.impact.is_some() {
            (scene.sim.tick - scene.sim.impact_tick) as f32 / 30.0
        } else {
            100.0
        };
        let charge_start = scene
            .sim
            .fire_tick
            .saturating_sub(scene.sim.config.charge_ticks);
        let charging = if scene.sim.tick <= scene.sim.fire_tick {
            (scene.sim.tick.saturating_sub(charge_start) as f32
                / scene.sim.config.charge_ticks.max(1) as f32)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        gas.params.weapon.y = scene.sim.tick as f32 / 30.0;
        gas.params.beam.w = charging;
        gas.params.energy.w = 0.0;
        if let Some(v) = scene.sim.shell {
            gas.params.energy = Vec4::new(
                v[0].raw() as f32 / 4294967296.0,
                v[1].raw() as f32 / 4294967296.0,
                v[2].raw() as f32 / 4294967296.0,
                2.8,
            );
        } else if let Some(at) = scene.sim.impact {
            gas.params.energy = Vec4::new(at[0] as f32, at[1] as f32, at[2] as f32, 0.0);
        }
        for (mut l, mut t) in &mut light {
            l.color = if scene.energy {
                Color::srgb(0.60, 0.78, 1.0)
            } else {
                Color::srgb(1.0, 0.4, 0.08)
            };
            l.intensity = 280_000_000.0 * (-age * 1.8).exp();
            if scene.sim.config.delivery == Delivery::Kinetic {
                l.color = Color::srgb(1.0, 0.82, 0.55);
                l.intensity = 6_000_000.0 * (-age * 28.0).exp();
            }
            if scene.energy {
                l.intensity = if scene.sim.impact.is_some() {
                    150_000_000.0 * (-age * 5.0).exp()
                } else {
                    25_000_000.0
                        * (charging * charging + if scene.sim.shell.is_some() { 1.0 } else { 0.0 })
                };
                t.translation = if scene.sim.shell.is_some() {
                    gas.params.energy.truncate()
                } else {
                    gas.params.beam.truncate()
                };
            }
            if let Some(at) = scene.sim.impact {
                t.translation = Vec3::new(at[0] as f32 - 4.0, at[1] as f32 + 12.0, at[2] as f32);
            }
            // PBR shadow maps do not contain raymarched world geometry. The
            // range composite instead traces the wall before adding this light.
            if scene.sim.layout.is_some() {
                l.intensity = 0.0;
            }
        }
        for mut tr in &mut shell {
            tr.translation = if scene.energy {
                Vec3::new(0.0, -1000.0, 0.0)
            } else {
                scene
                    .sim
                    .shell
                    .map(|p| {
                        Vec3::new(
                            p[0].raw() as f32 / 4294967296.0,
                            p[1].raw() as f32 / 4294967296.0,
                            p[2].raw() as f32 / 4294967296.0,
                        )
                    })
                    .unwrap_or(Vec3::new(0.0, -1000.0, 0.0))
            };
        }
        let recoil_age = (i64::from(scene.sim.tick) - i64::from(scene.sim.fire_tick)) as f32 / 30.0;
        let recoil = if (0.0..0.6).contains(&recoil_age) {
            (recoil_age * 14.0).sin() * (-recoil_age * 8.0).exp() * 3.0
        } else {
            0.0
        };
        for mut tr in &mut flash {
            if matches!(
                scene.sim.config.delivery,
                Delivery::Blast | Delivery::Kinetic
            ) && !scene.energy
                && (0.0..0.1).contains(&recoil_age)
            {
                tr.translation =
                    muzzle_vec(&scene.sim) + gun_rotation(&scene.sim) * Vec3::X * (2.0 - recoil);
                tr.rotation = gun_rotation(&scene.sim);
                tr.scale = Vec3::new(5.0, 2.4, 2.4) * (1.0 - recoil_age * 7.0);
            } else {
                tr.translation = Vec3::new(0.0, -1000.0, 0.0);
            }
        }
        let rotation = gun_rotation(&scene.sim);
        let pivot = Vec3::from(GUN.map(|v| v as f32));
        for (g, mut tr) in &mut gun {
            *tr = g.base;
            tr.translation = pivot + rotation * (g.base.translation - pivot - Vec3::X * recoil);
            tr.rotation = rotation * g.base.rotation;
        }
        // Small cosmetic chips are sampled only from actual terrain edits.
        // Larger conserved pieces remain owned by the fragment simulation.
        if let Some(at) = scene.sim.impact {
            let room = EJECTA_CAP.saturating_sub(debris.iter_mut().count());
            let per_tick = if scene.sim.config.material_energy_j == 0 {
                16
            } else {
                8
            };
            let edits = scene.sim.world.edits();
            for e in edits
                .iter()
                .step_by((edits.len() / per_tick).max(1))
                .take(room.min(per_tick))
            {
                if e.mat != 0 {
                    continue;
                }
                let pos = Vec3::new(e.x as f32, e.y as f32 + 1.0, e.z as f32);
                let hash = e.x.wrapping_mul(73856093)
                    ^ e.y.wrapping_mul(19349663)
                    ^ e.z.wrapping_mul(83492791);
                let seed = hash as f32 / u32::MAX as f32;
                let delta =
                    (pos - Vec3::new(at[0] as f32, at[1] as f32, at[2] as f32)).normalize_or_zero();
                let edge = if hash.is_multiple_of(13) {
                    2.4 + seed * 1.8
                } else if hash.is_multiple_of(3) {
                    1.1 + seed * 1.3
                } else {
                    0.35 + seed * 0.8
                };
                let speed = 1.0 / (1.0 + edge * 0.18);
                let v = (delta * (33.0 + seed * 54.0) + Vec3::Y * (27.0 + seed * 35.0)) * speed;
                commands.spawn((
                    Mesh3d(art.cube.clone()),
                    MeshMaterial3d(if hash.is_multiple_of(if scene.energy { 17 } else { 11 }) {
                        art.ember.clone()
                    } else {
                        art.rock.clone()
                    }),
                    Transform::from_translation(pos).with_scale(Vec3::new(
                        edge,
                        edge * (0.65 + seed * 0.5),
                        edge * (0.75 + (1.0 - seed) * 0.5),
                    )),
                    Ejecta {
                        velocity: v,
                        life: 2.4 + edge * 0.55,
                    },
                ));
            }
        }
        for (e, mut t, mut b) in &mut debris {
            b.life -= 1.0 / 30.0;
            if b.life <= 0.0 {
                commands.entity(e).despawn();
                continue;
            }
            b.velocity.y -= scene.sim.config.gravity as f32 / 30.0;
            let old = t.translation;
            let new = old + b.velocity / 30.0;
            let mut hit = false;
            for i in 1..=8 {
                let v = old.lerp(new, i as f32 / 8.0);
                if sim_core::blast::material(&scene.sim.world, [v.x as i32, v.y as i32, v.z as i32])
                    != 0
                {
                    hit = true;
                    break;
                }
            }
            if hit {
                b.velocity *= Vec3::new(0.45, -0.26, 0.45);
                b.life = b.life.min(1.2);
            } else {
                t.translation = new;
                t.rotate_local_x(0.11);
                t.rotate_local_z(0.07);
            }
        }
        if scene.frame.is_multiple_of(30) {
            println!(
                "frame={} blast_age={age:.2}s removed={} pending_edits={} bodies={} tick_ms={:.1}",
                scene.frame,
                scene.sim.removed,
                scene.sim.world.queued(),
                scene.sim.bodies.list.len(),
                scene.step_us as f64 / 1000.0
            );
        }
    }
}

fn orient_gun(
    scene: Res<Scene>,
    visual: Res<moving::Visual>,
    mut gun: Query<(&Gun, &mut Transform, &mut Visibility, Option<&Emitter>)>,
) {
    let pose = moving::visible(&scene.sim.gun, visual.previous.last(), &scene);
    let pivot = pose.pos;
    let origin = Vec3::from(GUN.map(|v| v as f32));
    // The turret turns about the hull's own up, not the world's (P-28): on
    // a tilted hull a level turret left a gap at the ring.
    let aim = render::realtime::hull_rotation(pose.up, visible_gun_yaw(&scene, &visual));
    let hull = render::realtime::hull_rotation(pose.up, pose.yaw);
    let segments = moving::gun_segments(&scene, &visual, pose);
    let age = (i64::from(scene.sim.tick) - i64::from(scene.sim.fire_tick)) as f32 / 30.0;
    let recoil = if (0.0..0.6).contains(&age) {
        (age * 14.0).sin() * (-age * 8.0).exp() * 3.0
    } else {
        0.0
    };
    for (g, mut tr, mut visible, emitter) in &mut gun {
        let connected = emitter.is_none_or(|e| scene.sim.gun.barrel_connected(e.0));
        *visible =
            if scene.sim.battle_mode() || (emitter.is_some() && (!scene.energy || !connected)) {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
        if let Some(seg) = g.segment {
            if let Some(local) = segments.get(seg) {
                *tr = *local;
                continue;
            }
        }
        let rotation = if g.turret { aim } else { hull };
        *tr = g.base;
        tr.translation = pivot
            + rotation
                * (g.base.translation - origin - Vec3::X * if g.turret { recoil } else { 0.0 });
        tr.rotation = rotation * g.base.rotation;
    }
}

fn weapon_art(
    scene: Res<Scene>,
    art: Res<Art>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut last: Local<Option<bool>>,
) {
    if *last == Some(scene.energy) {
        return;
    }
    *last = Some(scene.energy);
    if let Some(mut m) = materials.get_mut(&art.ember) {
        m.base_color = if scene.energy {
            Color::srgb(0.15, 0.5, 1.0)
        } else {
            Color::srgb(1.0, 0.6, 0.12)
        };
        m.emissive = if scene.energy {
            LinearRgba::new(3.0, 4.5, 7.0, 1.0)
        } else {
            LinearRgba::new(32.0, 6.4, 0.2, 1.0)
        };
    }
}
fn animate_energy(
    effects: Res<fire::Effects>,
    mut scene: ResMut<Scene>,
    visual: Res<moving::Visual>,
    workshop: Res<Workshop>,
    mut gas: ResMut<GasInput>,
    time: Res<Time<Real>>,
    mut windows: Query<&mut Window>,
) {
    if !gas.ready.load(Ordering::Relaxed) {
        return;
    }
    scene.fps = 1.0 / ((1.0 / scene.fps) * 0.95 + time.delta_secs().max(0.0001) * 0.05);
    // Interpolation is presentation only; collision and ablation remain 30 Hz.
    let fraction = if scene.realtime {
        (scene.acc * 30.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    effects.battle_energy(&scene.sim, fraction, &mut gas.params);
    let visual_tick = scene.sim.tick.saturating_sub(1) as f32 + fraction;
    gas.params.weapon.y = visual_tick / 30.0;
    let emitter =
        visible_muzzle(&scene, &visual) + visible_gun_rotation(&scene, &visual) * Vec3::X * 2.0;
    gas.params.beam = emitter.extend(gas.params.beam.w);
    if let (Some(a), Some(b)) = (scene.previous_shell, scene.sim.shell) {
        let next = Vec3::from(b.map(|v| v.raw() as f32 / 4294967296.0));
        gas.params.energy = a.lerp(next, fraction).extend(2.8);
    }
    for mut w in &mut windows {
        w.title = format!(
            "RTS VOX / FOREST RANGE | {} | {:.0} FPS",
            if workshop.editing {
                "EDITOR"
            } else if scene.paused {
                "PAUSED"
            } else {
                "LIVE 1x"
            },
            scene.fps
        );
    }
}

fn overview_frame(sim: &BlastRange, aim: Vec3, offset: Vec3, aspect: f32) -> (Vec3, f32) {
    let rotation = Transform::from_translation(offset)
        .looking_at(Vec3::ZERO, Vec3::Y)
        .rotation;
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    let mut include = |lo: Vec3, hi: Vec3| {
        for i in 0..8 {
            let p = Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            ) - aim;
            let q = Vec2::new(right.dot(p), up.dot(p));
            min = min.min(q);
            max = max.max(q);
        }
    };
    let gun = Vec3::from(sim.gun.at.map(|v| v as f32));
    include(
        gun - Vec3::new(45.0, 2.0, 30.0),
        gun + Vec3::new(45.0, 50.0, 30.0),
    );
    for (i, u) in sim.units.iter().enumerate() {
        let base = sim
            .layout
            .as_ref()
            .and_then(|l| l.units.get(i))
            .map_or(u.at, |d| d.at);
        let size = Vec3::new(
            u.body.vol.nx as f32,
            u.body.vol.ny as f32,
            u.body.vol.nz as f32,
        );
        let lo = Vec3::from(base.map(|v| v as f32)) - Vec3::new(size.x * 0.5, 0.0, size.z * 0.5);
        include(lo, lo + size);
    }
    if let Some(layout) = &sim.layout {
        if let Some(c) = &layout.course {
            include(
                Vec3::new(c.min[0] as f32, 16.0, c.min[1] as f32),
                Vec3::new(c.max[0] as f32, 55.0, c.max[1] as f32),
            );
        }
        for b in &layout.blocks {
            include(
                Vec3::from(b.min.map(|v| v as f32)),
                Vec3::from(b.max.map(|v| v as f32)),
            );
        }
        let target = Vec3::from(layout.target.map(|v| v as f32));
        include(target, target);
    }
    let aspect = aspect.max(0.1);
    let half = Vec2::new(120.0 * aspect, 120.0) - Vec2::splat(8.0);
    let centre = (min + max) * 0.5;
    let shift = Vec2::new(
        if min.x < -half.x || max.x > half.x {
            centre.x
        } else {
            0.0
        },
        if min.y < -half.y || max.y > half.y {
            centre.y
        } else {
            0.0
        },
    );
    let extent = (min - shift).abs().max((max - shift).abs()) + Vec2::splat(8.0);
    (
        aim + right * shift.x + up * shift.y,
        240.0f32.max(extent.y * 2.0).max(extent.x * 2.0 / aspect),
    )
}

/// Fit only the selected sector, never all objects across a kilometre.
fn sector_frame(z: &modding::range::RangeZone, offset: Vec3, aspect: f32) -> (Vec3, f32) {
    let aim = Vec3::new(
        (z.min[0] + z.max[0]) as f32 * 0.5,
        34.0,
        (z.min[1] + z.max[1]) as f32 * 0.5,
    );
    let rotation = Transform::from_translation(offset)
        .looking_at(Vec3::ZERO, Vec3::Y)
        .rotation;
    let mut extent = Vec2::ZERO;
    for x in [z.min[0], z.max[0]] {
        for y in [0, 110] {
            for zz in [z.min[1], z.max[1]] {
                let p = Vec3::new(x as f32, y as f32, zz as f32) - aim;
                extent = extent
                    .max(Vec2::new((rotation * Vec3::X).dot(p), (rotation * Vec3::Y).dot(p)).abs());
            }
        }
    }
    (
        aim,
        (extent.y * 2.0).max(extent.x * 2.0 / aspect.max(0.1)) + 24.0,
    )
}

fn whole_map_frame(size: [u32; 3], offset: Vec3, aspect: f32) -> (Vec3, f32) {
    let aim = Vec3::new(size[0] as f32 * 0.5, 34.0, size[2] as f32 * 0.5);
    let rotation = Transform::from_translation(offset)
        .looking_at(Vec3::ZERO, Vec3::Y)
        .rotation;
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let mut extent = Vec2::ZERO;
    for x in [0.0, size[0] as f32] {
        for y in [0.0, size[1] as f32] {
            for z in [0.0, size[2] as f32] {
                let p = Vec3::new(x, y, z) - aim;
                extent = extent.max(Vec2::new(right.dot(p), up.dot(p)).abs());
            }
        }
    }
    (
        aim,
        (extent.y * 2.0).max(extent.x * 2.0 / aspect.max(0.1)) + 80.0,
    )
}

fn orbit_offset(offset: Vec3, orbit: f32, pitch: f32) -> Vec3 {
    let radius = offset.length();
    let angle = (offset.y.atan2(Vec2::new(offset.x, offset.z).length()) + pitch)
        .clamp(20.0f32.to_radians(), 80.0f32.to_radians());
    let horizontal = Quat::from_rotation_y(orbit) * Vec3::new(offset.x, 0.0, offset.z).normalize();
    horizontal * (radius * angle.cos()) + Vec3::Y * (radius * angle.sin())
}

fn window_height_limit(offset: Vec3, aspect: f32, size: [u32; 3]) -> f32 {
    let rotation = Transform::from_translation(offset)
        .looking_at(Vec3::ZERO, Vec3::Y)
        .rotation;
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let forward = rotation * Vec3::NEG_Z;
    let ground_up = up - forward * (up.y / forward.y);
    let extent = right.abs() * aspect + ground_up.abs();
    let dims = [
        size[0].min(TERRAIN_WINDOW[0]),
        size[2].min(TERRAIN_WINDOW[2]),
    ];
    ((dims[0] as f32 - 16.0) / extent.x.max(0.01)).min((dims[1] as f32 - 16.0) / extent.z.max(0.01))
}

fn framing_far(base: f32, eye_offset: Vec3, height: f32, terrain_height: u32) -> f32 {
    // Zoom pulls the eye back even on legacy small layouts. A fixed 1200
    // plane can then clip the terrain and every depth-tested projectile.
    base.max(eye_offset.length() + height + terrain_height as f32)
}

fn framing(
    scene: Res<Scene>,
    workshop: Res<Workshop>,
    visual: Res<moving::Visual>,
    stream: Res<streaming::Streaming>,
    profile: Res<profile::Profile>,
    time: Res<Time<Real>>,
    mut camera: Query<(&mut Transform, &mut Projection, &Camera), With<GasCamera>>,
) {
    let age = if scene.sim.impact.is_some() {
        (scene.sim.tick - scene.sim.impact_tick) as f32 / 30.0
    } else {
        0.0
    };
    // Ease back after the fireball, keeping the rising column and gun in frame.
    let u = ((age - 1.3) / 3.0).clamp(0.0, 1.0);
    let pullback = u * u * (3.0 - 2.0 * u);
    let aim_x = if scene.energy { 285.0 } else { 207.0 };
    let rise = if scene.energy { 20.0 } else { 34.0 };
    for (mut transform, mut projection, camera) in &mut camera {
        *transform = Transform::from_xyz(aim_x - 159.0, 240.0 + pullback * rise, 593.0)
            .looking_at(Vec3::new(aim_x, 34.0 + pullback * rise, 336.0), Vec3::Y);
        if let Projection::Orthographic(p) = &mut *projection {
            p.scaling_mode = bevy::camera::ScalingMode::FixedVertical {
                viewport_height: if scene.energy {
                    220.0 + pullback * 20.0
                } else {
                    160.0 + pullback * 45.0
                },
            };
        }
        if scene.energy || scene.sim.layout.is_some() {
            let u = ((age - 2.5) / 3.0).clamp(0.0, 1.0);
            let close = match scene.view {
                1 => 0.0,
                2 => 1.0,
                _ => u * u * (3.0 - 2.0 * u),
            };
            let unit_at = scene.sim.units.first().map_or([377, 16, 305], |u| u.at);
            let aim = Vec3::new(285.0, 34.0, 336.0).lerp(
                Vec3::new(unit_at[0] as f32, 29.0, unit_at[2] as f32 + 9.0),
                close,
            );
            let offset =
                Vec3::new(-159.0, 206.0, 257.0).lerp(Vec3::new(-200.0, 160.0, 245.0), close);
            *transform = Transform::from_translation(aim + offset).looking_at(aim, Vec3::Y);
            if let Projection::Orthographic(p) = &mut *projection {
                p.scaling_mode = bevy::camera::ScalingMode::FixedVertical {
                    viewport_height: 220.0 - close * 133.0,
                };
            }
            if let Some(layout) = &scene.sim.layout {
                let centre = Vec3::from(layout.target.map(|v| v as f32));
                let mut aim = Vec3::new(
                    (layout.gun[0] as f32 + centre.x) * 0.5 + 35.0,
                    34.0,
                    centre.z,
                );
                let mut offset = Vec3::new(-155.0, 250.0, 320.0);
                let aspect = camera
                    .logical_viewport_size()
                    .map_or(16.0 / 9.0, |v| v.x / v.y.max(1.0));
                if workshop.sector().is_some() {
                    offset.y = 420.0;
                }
                offset = orbit_offset(offset, workshop.orbit, workshop.pitch);
                let (fitted, mut height) = workshop.sector().map_or_else(
                    || overview_frame(&scene.sim, aim, offset, aspect),
                    |z| sector_frame(z, offset, aspect),
                );
                aim = fitted;
                if scene.view == 6 {
                    (aim, height) = whole_map_frame(scene.sim.size(), offset, aspect);
                }
                if scene.view == 2 || scene.view == 3 {
                    let id = if scene.view == 3 { 1 } else { 0 };
                    aim = scene
                        .sim
                        .units
                        .get(id)
                        .map_or(centre, |u| Vec3::from(u.at.map(|v| v as f32)))
                        + Vec3::Y * 14.0;
                    offset = Vec3::new(if id == 1 { 190.0 } else { -190.0 }, 170.0, 245.0);
                    height = 100.0;
                } else if scene.view == 0 {
                    let v = ((age - 2.1) / 2.4).clamp(0.0, 1.0);
                    let zoom = v * v * (3.0 - 2.0 * v);
                    let v = ((age - 6.2) / 2.5).clamp(0.0, 1.0);
                    let orbit = v * v * (3.0 - 2.0 * v);
                    aim = aim.lerp(centre + Vec3::new(6.0, 7.0, -4.0), zoom);
                    offset = offset.lerp(Vec3::new(-170.0 + orbit * 360.0, 200.0, 285.0), zoom);
                    height -= zoom * 88.0;
                }
                if scene.view == 4 {
                    // Камера обязана смотреть туда, где тело **нарисовано**:
                    // `animate` рисует по `visible`, интерполируя между тиками,
                    // и прицел по `pose` отставал бы на кадр (ADR-268).
                    aim = moving::visible(
                        &scene.sim.gun,
                        visual.previous.get(scene.sim.units.len()),
                        &scene,
                    )
                    .pos + Vec3::Y * 14.0;
                    height = 150.0;
                }
                if scene.view == 5 {
                    // Explicit attack wins. Otherwise inspect a target in the
                    // selected sector; old layouts retain their named patrol.
                    let id = workshop.target_unit(scene.sim.attack, scene.patrol);
                    // Мишени может не быть вовсе: сохранённая раскладка
                    // допускает ноль тел. Тогда остаётся общий план, а не
                    // кроп в 150 единиц по пустому рельефу.
                    if let Some((i, u)) = id.and_then(|i| Some((i, scene.sim.units.get(i)?))) {
                        aim =
                            moving::visible(u, visual.previous.get(i), &scene).pos + Vec3::Y * 14.0;
                        height = 150.0;
                    }
                }
                // Reserve the top bar and command deck; the map stays centred
                // in the usable field instead of beside the old left sidebar.
                let viewport = camera
                    .logical_viewport_size()
                    .unwrap_or(Vec2::new(1600.0, 900.0));
                let ui_scale = panel::hud::scale(viewport);
                let panel_fraction = 310.0 * ui_scale / viewport.y.max(1.0);
                if scene.view == 1 || scene.view == 6 {
                    height /= 1.0 - panel_fraction;
                }
                let screen_up = Transform::from_translation(offset)
                    .looking_at(Vec3::ZERO, Vec3::Y)
                    .rotation
                    * Vec3::Y;
                let local_limit = window_height_limit(offset, aspect, scene.sim.size());
                let (zoom, pan) = if scene.camera_sweep {
                    let frame = scene.frame as f32;
                    let zoom = if frame < 80.0 {
                        1.0 + frame / 80.0 * 5.0
                    } else if frame < 160.0 {
                        6.0 - (frame - 80.0) / 80.0 * 5.0
                    } else {
                        1.0
                    };
                    (
                        zoom,
                        Vec2::new((frame - 160.0).clamp(0.0, 140.0) * 10.0, 0.0),
                    )
                } else {
                    (workshop.zoom, workshop.pan)
                };
                height = if scene.view == 1 {
                    // The authored sector is wider than the detailed GPU
                    // window. Its default overview used to stop at this
                    // limit; keep that sharp local view until the player
                    // actually zooms out, then progress toward the map.
                    (height * zoom).min(local_limit) + local_limit * (zoom - 1.0).max(0.0)
                } else {
                    height * zoom
                };
                if scene.view == 1 && height > local_limit {
                    let (whole_aim, whole_height) =
                        whole_map_frame(scene.sim.size(), offset, aspect);
                    let full = whole_height / (1.0 - panel_fraction);
                    let progress =
                        ((height - local_limit) / (full - local_limit).max(1.0)).clamp(0.0, 1.0);
                    aim = aim.lerp(whole_aim, progress);
                }
                let full_height =
                    whole_map_frame(scene.sim.size(), offset, aspect).1 / (1.0 - panel_fraction);
                height = height.min(full_height * 1.1);
                if let Some(tour) = profile.camera_sample(time.elapsed_secs_f64(), scene.sim.size())
                {
                    aim = Vec3::new(tour.focus.x, 34.0, tour.focus.y);
                    height = tour.height.min(full_height * 1.1);
                }
                aim -= screen_up * height * (238.0 - 72.0) * ui_scale / viewport.y.max(1.0) * 0.5;
                aim += Vec3::new(pan.x, 0.0, pan.y);
                // Continuous at the local-window limit: the old branch jumped
                // the eye hundreds of units, abruptly changing atmospheric depth.
                let eye_offset =
                    offset.normalize() * (offset.length() + (height - local_limit).max(0.0) * 1.15);
                *transform = Transform::from_translation(aim + eye_offset).looking_at(aim, Vec3::Y);
                if let Projection::Orthographic(p) = &mut *projection {
                    p.scaling_mode = bevy::camera::ScalingMode::FixedVertical {
                        viewport_height: height,
                    };
                    // The kilometre overview needs a distant plane, but
                    // using it in the local battle wastes depth precision
                    // on tank panels and thin terrain faces while panning.
                    let base = if streaming::map_scale(scene.sim.size(), height, stream.map_scale) {
                        30_000.0
                    } else {
                        1200.0
                    };
                    p.far = framing_far(base, eye_offset, height, scene.sim.size()[1]);
                }
            }
        }
    }
}

fn record(
    mut commands: Commands,
    scene: Res<Scene>,
    gas: Res<GasInput>,
    time: Res<Time<Real>>,
    mut recording: ResMut<Recording>,
    mut exit: MessageWriter<AppExit>,
) {
    if !gas.ready.load(Ordering::Relaxed) {
        return;
    }
    if let Some(done) = recording.capture(
        &mut commands,
        time.elapsed_secs_f64(),
        u64::from(scene.sim.tick),
        scene.live_elapsed as f64,
    ) {
        exit.write(done);
    }
}

fn capture(
    mut commands: Commands,
    mut scene: ResMut<Scene>,
    gas: Res<GasInput>,
    mut exit: MessageWriter<AppExit>,
) {
    if !gas.ready.load(Ordering::Relaxed) {
        return;
    }
    let Some(dir) = scene.out.clone() else {
        return;
    };
    // Editor captures have no simulation ticks. Save the actual paused UI
    // once and give the asynchronous screenshot writer time to finish.
    if scene.paused {
        if scene.captured == 0 {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(format!("{dir}/0000.png")));
            scene.captured = 1;
        } else {
            scene.wait += 1;
            if scene.wait == 35 {
                println!("CAPTURE complete: {} images", scene.captured);
                exit.write(AppExit::Success);
            }
        }
        return;
    }
    if scene.frame >= scene.finish {
        scene.wait += 1;
        if scene.wait == 35 {
            println!(
                "CLOCK realtime={} wall={:.3}s sim={:.3}s remainder={:.4}s",
                scene.realtime,
                scene.live_elapsed,
                scene.sim.tick as f32 / 30.0,
                scene.acc
            );
            println!(
                "CAPTURE complete: {} images, {} removed voxels",
                scene.captured, scene.sim.removed
            );
            exit.write(AppExit::Success);
        }
        return;
    }
    if scene.last_capture == scene.frame {
        return;
    }
    let fire = if matches!(
        scene.sim.config.delivery,
        Delivery::Blast | Delivery::Kinetic
    ) {
        scene.sim.fire_tick
    } else {
        scene.sim.config.fire_tick
    };
    let impact = if scene.sim.impact.is_some() {
        scene.sim.impact_tick
    } else {
        u32::MAX - 1000
    };
    let phases = if scene.sim.battle_mode() {
        [
            (30, "00-ready"),
            (60, "01-aim"),
            (90, "02-flight"),
            (120, "03-contact"),
            (180, "04-exchange"),
            (240, "05-damage"),
            (360, "06-battle"),
            (540, "07-survivors"),
            (720, "08-aftermath"),
        ]
    } else if !matches!(
        scene.sim.config.delivery,
        Delivery::Blast | Delivery::Kinetic
    ) {
        [
            (30, "00-ready"),
            (fire.saturating_sub(1), "01-aim"),
            (fire.saturating_add(2), "02-launch"),
            (fire.saturating_add(10), "03-flight"),
            (fire.saturating_add(20), "04-contact"),
            (fire.saturating_add(38), "05-stream"),
            (fire.saturating_add(72), "06-smoke"),
            (fire.saturating_add(100), "07-damage"),
            (fire.saturating_add(140), "08-after"),
        ]
    } else {
        [
            (30, "00-ready"),
            (fire.saturating_sub(10), "01-charge"),
            (fire.saturating_add(2), "02-launch"),
            (fire.saturating_add(4), "03-flight"),
            (impact + 1, "04-impact"),
            (impact + 8, "05-discharge"),
            (impact + 48, "06-dust"),
            (impact + 140, "07-wall-and-damage"),
            (impact + 280, "08-sheltered-unit"),
        ]
    };
    let stage = phases
        .iter()
        .find(|(tick, _)| *tick > scene.last_capture && *tick <= scene.frame)
        .map(|(_, name)| *name);
    scene.last_capture = scene.frame;
    if scene.all_frames {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(format!("{dir}/{:04}.png", scene.frame)));
        scene.captured += 1;
    } else if let Some(name) = stage {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(format!("{dir}/{name}.png")));
        scene.captured += 1;
    }
}
