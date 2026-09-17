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
use modding::explosion::ExplosionConfig;
use modding::range::{RangeLayout, RangeUnitKind};
use render::gas::{GasCamera, GasInput, GasPlugin};
use render::geom::MeshBuf;
use render::occlusion::OcclusionPlugin;
use render::recording::Recording;
use render::{look, presentation};
use sim_core::blast::{BlastRange, GUN, MUZZLE, SIZE};
#[path = "blast/bodies.rs"]
mod bodies;
#[path = "blast/damage_art.rs"]
mod damage_art;
#[path = "blast/fragments.rs"]
mod fragments;
#[path = "blast/knobs.rs"]
mod knobs;
#[path = "blast/moving.rs"]
mod moving;
#[path = "blast/panel.rs"]
mod panel;
#[path = "blast/workbench.rs"]
mod workbench;
use damage_art::SkinBindings;
use std::collections::BTreeSet;
use std::sync::{atomic::Ordering, Arc};
use workbench::Workshop;

#[derive(Resource)]
struct Scene {
    sim: BlastRange,
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
struct Witness(usize, Transform, Option<usize>);
#[derive(Resource)]
struct WitnessArt {
    materials: [Handle<StandardMaterial>; look::FINISHES],
    pristine: [voxel::unit::UnitBody; 2],
    skins: [SkinBindings; 2],
    seen: Vec<Option<(usize, u64, RangeUnitKind)>>,
}
#[derive(Component)]
struct Gun {
    base: Transform,
    turret: bool,
}
#[derive(Component)]
struct Shell;
#[derive(Component)]
struct BlastLight;
#[derive(Component)]
struct MuzzleFlash;
#[derive(Component)]
struct Emitter;
#[derive(Component)]
struct Ejecta {
    velocity: Vec3,
    life: f32,
}
#[derive(Resource)]
struct Art {
    cube: Handle<Mesh>,
    rock: Handle<StandardMaterial>,
    ember: Handle<StandardMaterial>,
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
    c.gas_source_radius = c.gas_source_radius * (1000 + power as i32) / 2000;
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
fn range_sim(
    energy: bool,
    anni: bool,
    power: u32,
    layout: Option<&RangeLayout>,
    uncovered: bool,
    kind: Option<RangeUnitKind>,
) -> Result<BlastRange, String> {
    let c = scaled_config(energy, anni, power);
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
/// Radius of one burning voxel's flame, world units (P-22).
const FLAME_RADIUS: f32 = 2.4;
fn configure_gas(sim: &BlastRange, energy: bool, anni: bool, gas: &mut GasInput) {
    let c = &sim.config;
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
    gas.params.charge = Vec4::new(
        c.gas_fuel_milli as f32 / 1000.0,
        c.gas_pressure_milli as f32 / 1000.0,
        if energy { 1.7 } else { 3.2 },
        0.0,
    );
    gas.params.beam = (muzzle_vec(sim) + gun_rotation(sim) * Vec3::X * 2.0).extend(0.0);
    gas.params.energy = Vec4::ZERO;
    gas.params.source = Vec4::ZERO;
    gas.params.control.y = 0.0;
    gas.params.weapon.x = weapon_code(energy, anni);
    gas.params.weapon.z = if sim.layout.is_some() { 1.0 } else { 0.0 };
    gas.params.weapon.w = look::scene().smoke_extinction;
    gas.params.look.x = look::scene().ground_glow;
}
fn main() {
    let mut out = None;
    let mut all_frames = false;
    let mut finish = None;
    let mut energy = false;
    let mut anni = false;
    let mut repeat = false;
    let mut view = 0;
    let mut requested_view = None;
    let mut patrol = None;
    let mut close_view = None;
    let mut realtime = false;
    // The live tuning menu open from the first frame (ADR-298), for a
    // capture of it.
    let mut settings = false;
    let mut layout_file = None;
    let mut power = 1000;
    let mut uncovered = false;
    let mut edit = false;
    let mut drive = false;
    let mut demo = false;
    let mut go: Option<[i32; 2]> = None;
    let mut record_dir = None;
    let mut record_seconds = 10.0;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--edit" => edit = true,
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
                    _ => panic!("--view expects auto, overview, exposed, sheltered, gun or target"),
                });
            }
            "--patrol" => {
                i += 1;
                patrol = Some(args[i].parse().expect("patrol number"));
                assert!(patrol != Some(0), "--patrol counts routes from 1");
            }
            "--realtime" => realtime = true,
            "--settings" => settings = true,
            "--weapon" => {
                i += 1;
                (energy, anni) = match args.get(i).map(String::as_str) {
                    Some("energy") => (true, false),
                    Some("anni") => (true, true),
                    Some("shell") => (false, false),
                    _ => panic!("--weapon expects shell, energy or anni"),
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
    let layout = read_layout(layout_file.as_deref()).expect("range file");
    assert!(!edit || layout.is_some(), "--edit needs --range");
    let workshop = Workshop::new(layout, edit);
    let mut sim = range_sim(
        energy,
        anni,
        power,
        workshop.layout.as_ref(),
        uncovered,
        None,
    )
    .expect("valid range");
    if drive {
        sim.control();
        view = 1;
    }
    if let Some(p) = go {
        assert!(sim.order_move(p), "--move: no route to {p:?}");
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
        !(3..=5).contains(&view) || workshop.layout.is_some(),
        "--view sheltered/gun/target needs --range"
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
    let mut gas = GasInput::default();
    fill_terrain(&sim, &mut gas);
    let c = &sim.config;
    gas.params.origin = Vec4::new(
        if energy { 272.0 } else { 112.0 },
        0.0,
        216.0,
        c.gas_cell_units as f32,
    );
    gas.params.chemistry = Vec4::new(
        c.reaction_milli as f32 / 1000.0,
        c.cooling_milli as f32 / 1000.0,
        c.buoyancy_milli as f32 / 1000.0,
        c.gas_temperature_k as f32 / 1000.0,
    );
    gas.params.charge = Vec4::new(
        c.gas_fuel_milli as f32 / 1000.0,
        c.gas_pressure_milli as f32 / 1000.0,
        if energy { 1.7 } else { 3.2 },
        0.0,
    );
    gas.params.sun = (-Vec3::from(look::sun_dir())).extend(1.8);
    gas.params.sky = Vec4::new(0.075, 0.105, 0.145, 0.00065);
    gas.params.weapon.x = weapon_code(energy, anni);
    gas.params.weapon.w = look::scene().smoke_extinction;
    gas.params.look.x = look::scene().ground_glow;
    gas.params.beam = Vec4::new(
        MUZZLE[0] as f32 + 2.0,
        MUZZLE[1] as f32,
        MUZZLE[2] as f32,
        0.0,
    );
    configure_gas(&sim, energy, anni, &mut gas);
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "RTS VOX / BLAST RANGE — Space: pause · R: replay".into(),
                resolution:
                    bevy::window::WindowResolution::new(1600, 900).with_scale_factor_override(1.0),
                present_mode: if realtime {
                    PresentMode::AutoVsync
                } else {
                    PresentMode::AutoNoVsync
                },
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.12, 0.15, 0.19)))
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
        .insert_resource(panel::Panel::new(drive, demo))
        .insert_resource(knobs::Overrides::new(settings))
        .init_resource::<moving::Visual>()
        .init_resource::<workbench::ShieldFlashes>()
        .insert_resource(workshop)
        .insert_resource(recording)
        .insert_resource(gas)
        .add_plugins((GasPlugin, OcclusionPlugin))
        .add_systems(Startup, (setup, workbench::setup, panel::setup))
        .add_systems(
            Update,
            (
                panel::actions,
                knobs::actions,
                workbench::input,
                advance,
                orient_gun,
                update_witness,
                moving::animate,
                fragments::draw,
                bodies::draw,
                weapon_art,
                animate_energy,
                framing,
                workbench::camera,
                workbench::overlay,
                panel::refresh,
                knobs::refresh,
                panel::release_keys,
                record,
                capture,
            )
                .chain(),
        )
        .run();
}
fn surface_material(sim: &BlastRange, p: [i32; 3], m: u16) -> u32 {
    let work = sim.surface_work.get(&p).copied().unwrap_or(0);
    if work == 0 {
        return m as u32;
    }
    let cost = sim_core::blast::material_work(m, &sim.config).max(1);
    if m == 3 {
        9 + (work * 3 / cost).min(2)
    } else if m == 2 || m == 6 {
        12 + (work * 3 / cost).min(2)
    } else {
        m as u32
    }
}
fn fill_terrain(sim: &BlastRange, gas: &mut GasInput) {
    let [nx, ny, nz] = SIZE;
    let mut words = vec![0u32; (nx * ny * nz) as usize / 8];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let m = surface_material(
                    sim,
                    [x as i32, y as i32, z as i32],
                    sim.world.material(x, y, z),
                );
                let i = (x + nx * (y + ny * z)) as usize;
                words[i / 8] |= m << ((i % 8) * 4);
            }
        }
    }
    gas.terrain_words = Arc::new(words);
    // The unit remains PBR geometry. Its voxel body only supplies occupancy to
    // the terrain's shadow rays (reserved presentation material 15): the root
    // and every segment in the frames the simulation posed them in (ADR-282).
    let words = Arc::make_mut(&mut gas.terrain_words);
    for i in moving::cells(sim) {
        words[i / 8] = (words[i / 8] & !(15 << ((i % 8) * 4))) | (15 << ((i % 8) * 4));
    }
    gas.terrain_revision += 1;
    gas.terrain_dirty = std::iter::once(0..gas.terrain_words.len()).collect();
    gas.params.terrain = Vec4::new(nx as f32, ny as f32, nz as f32, 0.0);
}
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
    let words = Arc::make_mut(&mut gas.terrain_words);
    let mut dirty = BTreeSet::new();
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
        if (0..3).any(|a| p[a] < 0 || p[a] >= SIZE[a] as i32) {
            continue;
        }
        let i = (p[0] as u32 + SIZE[0] * (p[1] as u32 + SIZE[1] * p[2] as u32)) as usize;
        let shift = (i % 8) * 4;
        let m = if sim.units.iter().any(|u| u.solid(p)) {
            15
        } else {
            surface_material(sim, p, mat)
        };
        words[i / 8] = (words[i / 8] & !(15 << shift)) | (m << shift);
        dirty.insert(i / 8);
    }
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
// Small finish variation breaks up pristine toy-like plates. This affects only
// reflected colour, not occupancy, armour resistance or the damage ledger.
fn weather(buf: &mut MeshBuf, finish: usize) {
    for j in 0..buf.pos.len() {
        let p = Vec3::from(buf.pos[j]) - Vec3::from(buf.nrm[j]) * 0.08;
        let q = p.floor();
        let grain = (q.dot(Vec3::new(17.1, 7.7, 23.3)).sin() * 437.1)
            .fract()
            .abs();
        let patch = (p.x * 0.27 + (p.z * 0.32).sin() * 1.8 + p.y * 0.19).sin();
        let dust = (1.0 - p.y.max(0.0) / 18.0).clamp(0.0, 1.0);
        let shade = (0.69 + grain * 0.19 + patch * 0.11) * if finish == 5 { 0.52 } else { 1.0 };
        for a in 0..3 {
            buf.col[j][a] *= shade;
        }
        if finish == 0 {
            for (a, tint) in [0.05, 0.035, 0.017].into_iter().enumerate() {
                buf.col[j][a] = buf.col[j][a] * (1.0 - dust * 0.22) + tint * dust * 0.22;
            }
        }
        if finish == 2 {
            buf.col[j][0] *= 0.035;
            buf.col[j][1] *= 0.07;
            buf.col[j][2] *= 0.1;
        }
    }
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
        skins: [
            SkinBindings::new(&voxel::unit::tank()),
            SkinBindings::new(&voxel::unit::strider()),
        ],
        pristine: [voxel::unit::tank(), voxel::unit::strider()],
        seen: Vec::new(),
    });
    let body = voxel::unit::tank();
    let tint = [0.16, 0.25, 0.34];
    let scale = body.mm as f32 / voxel::unit::WORLD_MM as f32;
    let frames = render::model::segment_frames(&body, 1.0 / voxel::unit::WORLD_MM as f32);
    let mut add_body = |b: &voxel::unit::UnitBody,
                        centre: [f32; 3],
                        pos: Vec3,
                        rot: Quat,
                        rows: (usize, usize),
                        turret: bool| {
        let mut bufs = std::array::from_fn(|_| MeshBuf::default());
        render::model::body_to_mesh(
            b,
            tint,
            b.mm as f32 / voxel::unit::WORLD_MM as f32,
            rows,
            render::model::BEVEL,
            centre,
            &mut bufs,
        );
        for (k, buf) in bufs.iter_mut().enumerate() {
            if buf.idx.is_empty() {
                continue;
            }
            weather(buf, k);
            let base = Transform::from_translation(
                Vec3::new(GUN[0] as f32, GUN[1] as f32, GUN[2] as f32) + pos,
            )
            .with_rotation(rot);
            commands.spawn((
                Mesh3d(meshes.add(mesh(buf))),
                MeshMaterial3d(finishes[k].clone()),
                base,
                Gun { base, turret },
            ));
        }
    };
    add_body(
        &body,
        render::model::root_centre(&body),
        Vec3::ZERO,
        Quat::IDENTITY,
        (0, body.turret_y as usize),
        false,
    );
    add_body(
        &body,
        render::model::root_centre(&body),
        Vec3::ZERO,
        Quat::IDENTITY,
        (body.turret_y as usize, usize::MAX),
        true,
    );
    for (s, f) in body.segments.iter().zip(frames) {
        add_body(
            &s.body,
            render::model::pivot_centre(&s.attach),
            f.pos,
            f.rot,
            (0, usize::MAX),
            f.turret,
        );
    }
    println!(
        "gun: {} mm voxels, scale {scale}, {} segments",
        body.mm,
        body.segments.len()
    );
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    {
        let coil = materials.add(StandardMaterial {
            base_color: Color::srgb(0.05, 0.20, 0.38),
            emissive: LinearRgba::new(0.10, 0.38, 0.85, 1.0),
            metallic: 0.7,
            perceptual_roughness: 0.24,
            ..default()
        });
        // Two open emitter collars around the existing barrel, sharing recoil.
        for x in [-8.0, -3.0] {
            for (offset, size) in [
                (Vec3::new(x, 3.0, 0.0), Vec3::new(1.3, 0.7, 6.7)),
                (Vec3::new(x, -3.0, 0.0), Vec3::new(1.3, 0.7, 6.7)),
                (Vec3::new(x, 0.0, 3.0), Vec3::new(1.3, 5.5, 0.7)),
                (Vec3::new(x, 0.0, -3.0), Vec3::new(1.3, 5.5, 0.7)),
            ] {
                let base = Transform::from_translation(
                    Vec3::new(MUZZLE[0] as f32, MUZZLE[1] as f32, MUZZLE[2] as f32) + offset,
                )
                .with_scale(size);
                commands.spawn((
                    Mesh3d(cube.clone()),
                    MeshMaterial3d(coil.clone()),
                    base,
                    Emitter,
                    Gun { base, turret: true },
                ));
            }
        }
    }
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
    commands.insert_resource(Art { cube, rock, ember });
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
fn update_witness(
    mut commands: Commands,
    scene: Res<Scene>,
    mut art: ResMut<WitnessArt>,
    debris_art: Res<Art>,
    mut meshes: ResMut<Assets<Mesh>>,
    old: Query<(Entity, &Witness)>,
) {
    for (entity, w) in &old {
        if w.0 >= scene.sim.units.len() {
            commands.entity(entity).despawn();
        }
    }
    art.seen.resize(scene.sim.units.len(), None);
    for (id, unit) in scene.sim.units.iter().enumerate() {
        let removed = unit.removed.len();
        let revision = (removed, unit.revision, unit.kind);
        if art.seen[id] == Some(revision) {
            continue;
        }
        let from = art.seen[id].map_or(removed, |r| r.0).min(removed);
        for (entity, w) in &old {
            if w.0 == id {
                commands.entity(entity).despawn();
            }
        }
        let base = Vec3::from(unit.at.map(|v| v as f32));
        let centre = Vec3::from(render::model::root_centre(&unit.body));
        let kind = match unit.kind {
            RangeUnitKind::Tank => 0,
            RangeUnitKind::Strider => 1,
        };
        let pristine = &art.pristine[kind];
        let skins = &art.skins[kind];
        let lost = |v: Vec3| {
            let q = v.floor().as_ivec3();
            q.cmpge(IVec3::ZERO).all()
                && q.cmplt(IVec3::new(
                    pristine.vol.nx as i32,
                    pristine.vol.ny as i32,
                    pristine.vol.nz as i32,
                ))
                .all()
                && pristine.vol.get(q.x as usize, q.y as usize, q.z as usize)
                && !unit.body.vol.get(q.x as usize, q.y as usize, q.z as usize)
        };
        let mut draw = |body: &voxel::unit::UnitBody,
                        pivot: [f32; 3],
                        pos: Vec3,
                        rot: Quat,
                        segment: Option<usize>| {
            let mut bufs = std::array::from_fn(|_| MeshBuf::default());
            render::model::body_to_mesh(
                body,
                if id == 0 {
                    [0.38, 0.34, 0.25]
                } else {
                    [0.22, 0.32, 0.27]
                },
                1.0,
                (0, usize::MAX),
                0.35,
                pivot,
                &mut bufs,
            );
            for (k, buf) in bufs.iter_mut().enumerate() {
                if buf.idx.is_empty() {
                    continue;
                }
                weather(buf, k);
                if unit.revision > 0 {
                    for j in 0..buf.pos.len() {
                        let normal = Vec3::from(buf.nrm[j]);
                        let local = Vec3::from(buf.pos[j]) + Vec3::from(pivot) - normal * 0.08;
                        let cell = local.floor().as_ivec3();
                        let owner = if let Some(seg) = segment {
                            if cell.cmplt(IVec3::ZERO).any() {
                                None
                            } else {
                                skins.owner(
                                    pristine,
                                    seg,
                                    [cell.x as u8, cell.y as u8, cell.z as u8],
                                )
                            }
                        } else if cell.cmplt(IVec3::ZERO).any() {
                            None
                        } else {
                            Some([cell.x as u8, cell.y as u8, cell.z as u8])
                        };
                        let Some(owner) = owner else {
                            continue;
                        };
                        let p = Vec3::from(owner.map(|v| v as f32)) + Vec3::splat(0.5);
                        let work = unit.dents.get(&owner).copied().unwrap_or(0);
                        if work > 0 {
                            let cost = sim_core::blast::unit_work(
                                &unit.body,
                                owner.map(usize::from),
                                &scene.sim.config,
                            );
                            let severity = (work as f32 / cost as f32).clamp(0.0, 1.0);
                            let grain = (p.dot(Vec3::new(17.1, 7.7, 23.3)).sin() * 437.1)
                                .fract()
                                .abs();
                            let steel = Vec3::new(0.14, 0.155, 0.165) * (0.65 + grain * 0.7);
                            let burned = Vec3::new(0.026, 0.03, 0.035);
                            let col = steel.lerp(burned, severity * 0.85);
                            buf.col[j] = [col.x, col.y, col.z, 1.0];
                            // Sub-voxel deformation is presentation of the
                            // persisted work, never a second collision body.
                            buf.pos[j] =
                                (Vec3::from(buf.pos[j]) - normal * severity * 0.13).to_array();
                        }
                        let cut = lost(p + normal * 0.55);
                        let scorched = cut
                            || [
                                Vec3::X,
                                Vec3::NEG_X,
                                Vec3::Y,
                                Vec3::NEG_Y,
                                Vec3::Z,
                                Vec3::NEG_Z,
                            ]
                            .iter()
                            .any(|d| lost(p + *d * 1.4));
                        if scorched {
                            let grain = (p.dot(Vec3::new(17.1, 7.7, 23.3)).sin() * 437.1)
                                .fract()
                                .abs();
                            let c = if cut {
                                0.038 + grain * 0.065
                            } else {
                                0.025 + grain * 0.022
                            };
                            buf.col[j] = [c, c * 0.95, c * 0.87, 1.0];
                        }
                    }
                }
                commands.spawn((
                    Mesh3d(meshes.add(mesh(buf))),
                    MeshMaterial3d(art.materials[k].clone()),
                    Transform::from_translation(base + pos).with_rotation(rot),
                    Witness(
                        id,
                        Transform::from_translation(pos).with_rotation(rot),
                        segment,
                    ),
                ));
            }
        };
        draw(
            &unit.body,
            centre.to_array(),
            Vec3::ZERO,
            Quat::IDENTITY,
            None,
        );
        for (seg, (segment, frame)) in unit
            .body
            .segments
            .iter()
            .zip(render::model::segment_frames(&unit.body, 0.01))
            .enumerate()
        {
            if unit.seg_gone[seg] {
                continue;
            }
            let clipped = skins.clipped(pristine, &unit.body, seg);
            draw(
                &clipped,
                render::model::pivot_centre(&segment.attach),
                frame.pos,
                frame.rot,
                Some(seg),
            );
        }
        if let Some(at) = scene
            .sim
            .impact
            .filter(|_| scene.sim.config.material_energy_j == 0)
        {
            let hit = Vec3::from(at.map(|v| v as f32));
            for p in unit.removed[from..].iter().step_by(97).take(32) {
                let pos = Vec3::from(unit.world_pos(p.map(usize::from)).map(|v| v as f32));
                let seed = f32::from(p[0] ^ p[1] ^ p[2]) / 63.0;
                commands.spawn((
                    Mesh3d(debris_art.cube.clone()),
                    MeshMaterial3d(debris_art.rock.clone()),
                    Transform::from_translation(pos).with_scale(Vec3::new(1.0 + seed, 0.35, 1.5)),
                    Ejecta {
                        velocity: (pos - hit).normalize_or_zero() * (20.0 + seed * 24.0)
                            + Vec3::Y * 24.0,
                        life: 4.0,
                    },
                ));
            }
        }
        art.seen[id] = Some(revision);
        if removed > 0 {
            println!(
                "unit {id} ablated={} core={} engine={} weapon={} sensor={}",
                removed,
                unit.status.alive,
                unit.status.engine,
                unit.status.weapon,
                unit.status.sensor
            );
        }
    }
}

// Explicit ECS inputs and disjoint filters describe the visual update borrows.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn advance(
    mut commands: Commands,
    mut scene: ResMut<Scene>,
    mut workshop: ResMut<Workshop>,
    mut visual: ResMut<moving::Visual>,
    panel: Res<panel::Panel>,
    recording: Res<Recording>,
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
        let next = if let Some(sim) = authored.filter(|_| !changed) {
            Ok(sim)
        } else {
            range_sim(energy, anni, power, reloaded.as_ref(), uncovered, kind)
        };
        let Ok(sim) = next else {
            scene.notice = format!("Layout rejected: {}", next.err().unwrap());
            eprintln!("{}", scene.notice);
            return;
        };
        scene.notice.clear();
        if keys.just_pressed(KeyCode::F5) {
            *workshop = Workshop::new(reloaded, workshop.editing);
        }
        if keys.just_pressed(KeyCode::Enter) {
            workshop.editing = false;
        }
        scene.sim = sim;
        if panel.drive {
            scene.sim.control();
        }
        *visual = moving::Visual::default();
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
        fill_terrain(&scene.sim, &mut gas);
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
    if !recording.running() {
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
        scene.live_elapsed += time.delta_secs();
        if scene.acc < 1.0 / 30.0 {
            return;
        }
        (scene.acc * 30.0).floor().min(8.0) as u32
    } else {
        1
    };
    gas.terrain_dirty.clear();
    for _ in 0..steps {
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
        moving::before(&scene.sim, &mut visual);
        if panel.demo {
            if scene.sim.tick == 20 {
                scene.sim.order_move([235, 365]);
            }
            if scene.sim.tick == 220 {
                scene.sim.order_attack(2);
            }
        }
        let started = std::time::Instant::now();
        scene.sim.step();
        let us = started.elapsed().as_micros();
        scene.step_us = if scene.frame.is_multiple_of(30) {
            us
        } else {
            scene.step_us.max(us)
        };
        scene.frame += 1;
        apply_terrain(&scene.sim, &mut gas, &unit_start);
        moving::occupancy(&scene.sim, &mut gas, &mut visual);
        if before.is_none() {
            if let Some(at) = scene.sim.impact {
                gas.params.control.z = 1.0;
                gas.params.source = Vec4::new(
                    at[0] as f32 - 3.0,
                    at[1] as f32 + 7.0,
                    at[2] as f32,
                    scene.sim.config.gas_source_radius as f32,
                );
                let params = gas.params;
                gas.atmosphere.emit_at(params, scene.sim.impact_tick * 2);
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
            params.source = Vec4::new(p[0] as f32, p[1] as f32 + 2.0, p[2] as f32, 3.0);
            gas.atmosphere.emit_at(params, scene.sim.tick * 2);
        }
        let alight = scene.sim.heat.burning();
        if alight > 0 {
            scene.flame = (scene.flame + 1) % alight;
            if let Some(p) = scene.sim.heat.burning_cells().nth(scene.flame) {
                let mut params = gas.params;
                params.control.z = 1.0;
                // Hot, little pressure and half the fuel: a flame, not a burst.
                params.chemistry.w = (params.chemistry.w * 1.8).max(2.2);
                params.charge = Vec4::new(
                    gas.params.charge.x * 0.5,
                    gas.params.charge.y * 0.2,
                    1.1,
                    0.0,
                );
                params.source =
                    Vec4::new(p[0] as f32, p[1] as f32 + 1.5, p[2] as f32, FLAME_RADIUS);
                gas.atmosphere.emit_at(params, scene.sim.tick * 2);
            }
        }
        gas.advance(scene.sim.tick * 2);
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
            if !scene.energy && (0.0..0.1).contains(&recoil_age) {
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
        // Cosmetic ejecta are sampled from voxels actually removed this tick.
        // They collide with the current terrain, but never become extra matter.
        if let Some(at) = scene
            .sim
            .impact
            .filter(|_| scene.sim.config.material_energy_j == 0)
        {
            for (i, e) in scene
                .sim
                .world
                .edits()
                .iter()
                .enumerate()
                .step_by(41)
                .take(24)
            {
                if e.mat != 0 {
                    continue;
                }
                let pos = Vec3::new(e.x as f32, e.y as f32 + 1.0, e.z as f32);
                let seed = (e.x.wrapping_mul(73856093)
                    ^ e.y.wrapping_mul(19349663)
                    ^ e.z.wrapping_mul(83492791)) as f32
                    / 4294967296.0;
                let delta =
                    (pos - Vec3::new(at[0] as f32, at[1] as f32, at[2] as f32)).normalize_or_zero();
                let v = delta * (28.0 + seed * 40.0) + Vec3::Y * (30.0 + seed * 32.0);
                commands.spawn((
                    Mesh3d(art.cube.clone()),
                    MeshMaterial3d(if i % (if scene.energy { 17 } else { 3 }) == 0 {
                        art.ember.clone()
                    } else {
                        art.rock.clone()
                    }),
                    Transform::from_translation(pos).with_scale(Vec3::new(
                        0.6 + seed * 1.8,
                        0.5 + seed * 0.8,
                        0.8 + seed,
                    )),
                    Ejecta {
                        velocity: v,
                        life: 5.0,
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
    mut gun: Query<(&Gun, &mut Transform)>,
) {
    let pose = moving::visible(&scene.sim.gun, visual.previous.last(), &scene);
    let pivot = pose.pos;
    let origin = Vec3::from(GUN.map(|v| v as f32));
    // The turret turns about the hull's own up, not the world's (P-28): on
    // a tilted hull a level turret left a gap at the ring.
    let aim = render::realtime::hull_rotation(pose.up, visible_gun_yaw(&scene, &visual));
    let hull = render::realtime::hull_rotation(pose.up, pose.yaw);
    let age = (i64::from(scene.sim.tick) - i64::from(scene.sim.fire_tick)) as f32 / 30.0;
    let recoil = if (0.0..0.6).contains(&age) {
        (age * 14.0).sin() * (-age * 8.0).exp() * 3.0
    } else {
        0.0
    };
    for (g, mut tr) in &mut gun {
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
    mut coils: Query<&mut Visibility, With<Emitter>>,
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
    for mut v in &mut coils {
        *v = if scene.energy {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
fn animate_energy(
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
    let visual_tick = scene.sim.tick.saturating_sub(1) as f32 + fraction;
    gas.params.weapon.y = visual_tick / 30.0;
    let pose = moving::visible(&scene.sim.gun, visual.previous.last(), &scene);
    let emitter =
        pose.pos + Vec3::Y * 23.0 + visible_gun_rotation(&scene, &visual) * Vec3::X * 32.0;
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
    let gun = Vec3::from(GUN.map(|v| v as f32));
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

fn framing(
    scene: Res<Scene>,
    workshop: Res<Workshop>,
    visual: Res<moving::Visual>,
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
                let mut aim = Vec3::new((GUN[0] as f32 + centre.x) * 0.5 + 35.0, 34.0, centre.z);
                let mut offset = Vec3::new(-155.0, 250.0, 320.0);
                let aspect = camera
                    .logical_viewport_size()
                    .map_or(16.0 / 9.0, |v| v.x / v.y.max(1.0));
                let (fitted, mut height) = overview_frame(&scene.sim, aim, offset, aspect);
                aim = fitted;
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
                    // Ходовую мишень называет сама раскладка. Прежний
                    // `unwrap_or(2)` держался на порядке юнитов в
                    // range-cover.ron и на чужой раскладке снимал не то.
                    //
                    // Выбор редактора участвует только если выбрано **ходовое**
                    // тело: `Workshop::new` заранее ставит `Unit(0)` при
                    // `--edit`, и без этого условия `--edit --view target`
                    // всегда снимал неподвижный танк (ADR-268).
                    let patrol = |i: usize| {
                        layout
                            .course
                            .as_ref()
                            .is_some_and(|c| c.patrols.iter().any(|p| p.unit == i))
                    };
                    let id = scene
                        .sim
                        .attack
                        .or_else(|| match workshop.selected {
                            Some(workbench::Selection::Unit(i)) if patrol(i) => Some(i),
                            _ => None,
                        })
                        .or_else(|| {
                            Some(layout.course.as_ref()?.patrols.get(scene.patrol - 1)?.unit)
                        });
                    // Мишени может не быть вовсе: сохранённая раскладка
                    // допускает ноль тел. Тогда остаётся общий план, а не
                    // кроп в 150 единиц по пустому рельефу.
                    if let Some((i, u)) = id.and_then(|i| Some((i, scene.sim.units.get(i)?))) {
                        aim =
                            moving::visible(u, visual.previous.get(i), &scene).pos + Vec3::Y * 14.0;
                        height = 150.0;
                    }
                }
                // Reserve the sidebar's width in the fitted overview.
                let panel_fraction = camera.logical_viewport_size().map_or(0.2, |v| {
                    310.0 * (v.y / 860.0).clamp(0.65, 1.0) / v.x.max(640.0)
                });
                if scene.view == 1 {
                    height /= 1.0 - panel_fraction;
                }
                let right = Transform::from_translation(offset)
                    .looking_at(Vec3::ZERO, Vec3::Y)
                    .rotation
                    * Vec3::X;
                aim -= right * height * aspect * panel_fraction * 0.5;
                aim += Vec3::new(workshop.pan.x, 0.0, workshop.pan.y);
                height *= workshop.zoom;
                *transform = Transform::from_translation(aim + offset).looking_at(aim, Vec3::Y);
                if let Projection::Orthographic(p) = &mut *projection {
                    p.scaling_mode = bevy::camera::ScalingMode::FixedVertical {
                        viewport_height: height,
                    };
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
    if scene.paused || scene.last_capture == scene.frame {
        return;
    }
    let fire = scene.sim.fire_tick;
    let impact = if scene.sim.impact.is_some() {
        scene.sim.impact_tick
    } else {
        u32::MAX - 1000
    };
    let phases = [
        (30, "00-ready"),
        (fire.saturating_sub(10), "01-charge"),
        (fire + 2, "02-launch"),
        (fire + 4, "03-flight"),
        (impact + 1, "04-impact"),
        (impact + 8, "05-discharge"),
        (impact + 48, "06-dust"),
        (impact + 140, "07-wall-and-damage"),
        (impact + 280, "08-sheltered-unit"),
    ];
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
