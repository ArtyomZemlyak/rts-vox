//! Live tuning of the range (ADR-298): a second panel of knobs the owner
//! turns while the range runs, saved to and loaded from
//! `out/range-tuning.txt`. Simulation numbers go through the range's own
//! control surface — the weapon preset, the units' chassis, the debris
//! tuning, the material registry — and picture numbers through the
//! look's overrides; the window never writes into the simulation's state
//! behind its back.
use super::panel::Action;
use super::*;
use bevy::post_process::bloom::Bloom;
use modding::tuning::Chassis;
use sim_core::blast::motion::Motion;
use std::collections::BTreeMap;

/// Where the saved knobs live; a plain `key value` per line.
pub const FILE: &str = "out/range-tuning.txt";
/// Rows a group may show at once; the tank and walker groups fill them.
pub const SLOTS: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Weapon,
    Tank,
    Walker,
    Debris,
    Density,
    Strength,
    Picture,
}
impl Group {
    pub const ALL: [Group; 7] = [
        Group::Weapon,
        Group::Tank,
        Group::Walker,
        Group::Debris,
        Group::Density,
        Group::Strength,
        Group::Picture,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Group::Weapon => "Оружие",
            Group::Tank => "Танк",
            Group::Walker => "Шагоход",
            Group::Debris => "Обломки",
            Group::Density => "Плотность",
            Group::Strength => "Прочность",
            Group::Picture => "Картинка",
        }
    }
}

pub struct Knob {
    pub key: &'static str,
    pub group: Group,
    pub label: &'static str,
    pub step: f64,
    pub min: f64,
    pub max: f64,
}
const fn k(
    key: &'static str,
    group: Group,
    label: &'static str,
    step: f64,
    min: f64,
    max: f64,
) -> Knob {
    Knob {
        key,
        group,
        label,
        step,
        min,
        max,
    }
}
macro_rules! chassis_knobs {
    ($prefix:literal, $group:expr) => {
        [
            k(
                concat!($prefix, ".speed"),
                $group,
                "Скорость",
                2.0,
                1.0,
                200.0,
            ),
            k(
                concat!($prefix, ".step"),
                $group,
                "Шаг подъёма",
                1.0,
                1.0,
                8.0,
            ),
            k(
                concat!($prefix, ".stiffness"),
                $group,
                "Жёсткость подвески",
                1.0,
                1.0,
                200.0,
            ),
            k(
                concat!($prefix, ".damping"),
                $group,
                "Демпфер подвески",
                1.0,
                0.0,
                100.0,
            ),
            k(
                concat!($prefix, ".travel_cm"),
                $group,
                "Ход подвески, см",
                2.0,
                2.0,
                200.0,
            ),
            k(
                concat!($prefix, ".clearance_cm"),
                $group,
                "Клиренс, см",
                5.0,
                5.0,
                400.0,
            ),
            k(
                concat!($prefix, ".inertia"),
                $group,
                "Инерция корпуса",
                5.0,
                1.0,
                2000.0,
            ),
            k(
                concat!($prefix, ".ang_damp"),
                $group,
                "Угловое трение",
                1.0,
                0.0,
                50.0,
            ),
            k(
                concat!($prefix, ".max_tilt_pct"),
                $group,
                "Предел завала, %",
                1.0,
                50.0,
                100.0,
            ),
            k(
                concat!($prefix, ".contact_cm"),
                $group,
                "Длина контакта, см",
                10.0,
                0.0,
                1000.0,
            ),
            k(
                concat!($prefix, ".approach_pct"),
                $group,
                "Угол наезда, %",
                5.0,
                1.0,
                300.0,
            ),
            k(
                concat!($prefix, ".settle_pct"),
                $group,
                "Поправка за тик, %",
                5.0,
                1.0,
                100.0,
            ),
            k(
                concat!($prefix, ".settle_tilt_pct"),
                $group,
                "Доля наклона, %",
                5.0,
                0.0,
                100.0,
            ),
            k(
                concat!($prefix, ".thrust"),
                $group,
                "Тяга",
                4.0,
                1.0,
                1000.0,
            ),
            k(
                concat!($prefix, ".brake"),
                $group,
                "Тормоз",
                4.0,
                1.0,
                1000.0,
            ),
            k(
                concat!($prefix, ".turn"),
                $group,
                "Поворот",
                4.0,
                1.0,
                1000.0,
            ),
            k(
                concat!($prefix, ".climb"),
                $group,
                "Подъём, %",
                5.0,
                0.0,
                300.0,
            ),
            k(
                concat!($prefix, ".descend"),
                $group,
                "Спуск, %",
                5.0,
                0.0,
                300.0,
            ),
        ]
    };
}
const TANK: [Knob; 18] = chassis_knobs!("tank", Group::Tank);
const WALKER: [Knob; 18] = chassis_knobs!("walker", Group::Walker);
const WEAPON: [Knob; 12] = [
    k(
        "weapon.energy",
        Group::Weapon,
        "Сила волны",
        100.0,
        100.0,
        1_000_000.0,
    ),
    k(
        "weapon.material_energy_j",
        Group::Weapon,
        "Энергия на обломки, Дж",
        1_000_000.0,
        0.0,
        4_000_000_000.0,
    ),
    k(
        "weapon.hull_impulse_milli",
        Group::Weapon,
        "Толчок корпуса",
        10.0,
        0.0,
        100_000.0,
    ),
    k(
        "weapon.soil_resistance",
        Group::Weapon,
        "Сопротивление грунта",
        10.0,
        1.0,
        1_000_000.0,
    ),
    k(
        "weapon.concrete_resistance",
        Group::Weapon,
        "Сопротивление бетона",
        10.0,
        1.0,
        1_000_000.0,
    ),
    k(
        "weapon.steel_resistance",
        Group::Weapon,
        "Сопротивление стали",
        10.0,
        1.0,
        1_000_000.0,
    ),
    k(
        "weapon.armour_milli",
        Group::Weapon,
        "Броня машин, тысячные",
        50.0,
        1.0,
        100_000.0,
    ),
    k(
        "weapon.fragment_box",
        Group::Weapon,
        "Куски, ячеек в стороне",
        1.0,
        1.0,
        4.0,
    ),
    k(
        "weapon.heat_units",
        Group::Weapon,
        "Радиус нагрева",
        1.0,
        0.0,
        64.0,
    ),
    k(
        "weapon.heat_per_cell",
        Group::Weapon,
        "Тепло на ячейку",
        10.0,
        0.0,
        100_000.0,
    ),
    k(
        "weapon.gas_source_radius",
        Group::Weapon,
        "Радиус дыма",
        1.0,
        1.0,
        60.0,
    ),
    k(
        "weapon.wave_work_per_tick",
        Group::Weapon,
        "Работа волны за тик",
        512.0,
        64.0,
        65_536.0,
    ),
];
const DEBRIS: [Knob; 11] = [
    k(
        "debris.drag_pct",
        Group::Debris,
        "Воздух, % скорости за тик",
        1.0,
        0.0,
        100.0,
    ),
    k(
        "debris.bounce_pct",
        Group::Debris,
        "Отскок, %",
        1.0,
        0.0,
        99.0,
    ),
    k(
        "debris.friction_pct",
        Group::Debris,
        "Трение, %",
        1.0,
        0.0,
        100.0,
    ),
    k(
        "debris.sleep_mm_s",
        Group::Debris,
        "Порог покоя, мм/с",
        50.0,
        1.0,
        5000.0,
    ),
    k(
        "debris.push_pct",
        Group::Debris,
        "Толчок корпусом, %",
        10.0,
        0.0,
        1000.0,
    ),
    k(
        "debris.push_up_pct",
        Group::Debris,
        "Подъём толчка, %",
        5.0,
        0.0,
        100.0,
    ),
    k(
        "debris.rest_check_ticks",
        Group::Debris,
        "Проверка опоры, тиков",
        1.0,
        1.0,
        600.0,
    ),
    k(
        "debris.pile_cell",
        Group::Debris,
        "Ячейка кучи",
        1.0,
        1.0,
        32.0,
    ),
    k(
        "debris.lift_pct",
        Group::Debris,
        "Подброс осколков, %",
        5.0,
        0.0,
        100.0,
    ),
    k(
        "debris.motion_pct",
        Group::Debris,
        "Доля энергии в полёт, %",
        1.0,
        0.0,
        99.0,
    ),
    k(
        "debris.max_flying",
        Group::Debris,
        "Летящих кусков, не больше",
        256.0,
        1.0,
        32_768.0,
    ),
];
const MATERIAL_NAMES: [&str; 8] = [
    "Грунт",
    "Бетон",
    "Сталь",
    "Крона",
    "Ствол",
    "Камень",
    "Дёрн",
    "Площадка",
];
macro_rules! material_knobs {
    ($field:literal, $group:expr, $step:expr, $max:expr) => {
        [
            k(
                concat!($field, ".1"),
                $group,
                MATERIAL_NAMES[0],
                $step,
                1.0,
                $max,
            ),
            k(
                concat!($field, ".2"),
                $group,
                MATERIAL_NAMES[1],
                $step,
                1.0,
                $max,
            ),
            k(
                concat!($field, ".3"),
                $group,
                MATERIAL_NAMES[2],
                $step,
                1.0,
                $max,
            ),
            k(
                concat!($field, ".4"),
                $group,
                MATERIAL_NAMES[3],
                $step,
                1.0,
                $max,
            ),
            k(
                concat!($field, ".5"),
                $group,
                MATERIAL_NAMES[4],
                $step,
                1.0,
                $max,
            ),
            k(
                concat!($field, ".6"),
                $group,
                MATERIAL_NAMES[5],
                $step,
                1.0,
                $max,
            ),
            k(
                concat!($field, ".7"),
                $group,
                MATERIAL_NAMES[6],
                $step,
                1.0,
                $max,
            ),
            k(
                concat!($field, ".8"),
                $group,
                MATERIAL_NAMES[7],
                $step,
                1.0,
                $max,
            ),
        ]
    };
}
const DENSITY: [Knob; 8] = material_knobs!("density", Group::Density, 10.0, 5000.0);
const STRENGTH: [Knob; 8] = material_knobs!("strength", Group::Strength, 5.0, 5000.0);
const MELT: [Knob; 2] = [
    k(
        "melt.2",
        Group::Strength,
        "Плавление бетона",
        20.0,
        0.0,
        100_000.0,
    ),
    k(
        "melt.3",
        Group::Strength,
        "Плавление стали",
        20.0,
        0.0,
        100_000.0,
    ),
];
const PICTURE: [Knob; 9] = [
    k(
        "picture.smoke_extinction",
        Group::Picture,
        "Плотность дыма",
        0.05,
        0.05,
        3.0,
    ),
    k(
        "picture.ground_glow",
        Group::Picture,
        "Свечение прогретой земли",
        0.1,
        0.0,
        3.0,
    ),
    k(
        "picture.bloom",
        Group::Picture,
        "Свечение (bloom)",
        0.05,
        0.0,
        1.0,
    ),
    k(
        "picture.ambient_brightness",
        Group::Picture,
        "Заполняющий свет",
        5.0,
        0.0,
        400.0,
    ),
    k(
        "picture.blast_size",
        Group::Picture,
        "Клуб взрыва, радиусов",
        0.1,
        0.1,
        5.0,
    ),
    k(
        "picture.blast_puffs",
        Group::Picture,
        "Клубов взрыва",
        1.0,
        0.0,
        24.0,
    ),
    k(
        "picture.spark_size",
        Group::Picture,
        "Искра, вокселей",
        0.1,
        0.1,
        5.0,
    ),
    k(
        "picture.smoke_size",
        Group::Picture,
        "Дым подбитой, вокселей",
        1.0,
        1.0,
        60.0,
    ),
    k(
        "picture.dust_alpha",
        Group::Picture,
        "Плотность пыли",
        0.05,
        0.0,
        1.0,
    ),
];

/// Every knob, in the order the groups show them.
pub fn knobs() -> impl Iterator<Item = &'static Knob> {
    WEAPON
        .iter()
        .chain(TANK.iter())
        .chain(WALKER.iter())
        .chain(DEBRIS.iter())
        .chain(DENSITY.iter())
        .chain(STRENGTH.iter())
        .chain(MELT.iter())
        .chain(PICTURE.iter())
}
fn knob(key: &str) -> Option<&'static Knob> {
    knobs().find(|k| k.key == key)
}

/// The turned knobs and the panel's state.
#[derive(Resource, Default)]
pub struct Overrides {
    pub values: BTreeMap<&'static str, f64>,
    /// What every knob read before anything was turned: the reset target.
    base: BTreeMap<&'static str, f64>,
    pub open: bool,
    pub group: usize,
    started: bool,
    last_tick: u32,
    pub notice: String,
    /// The row whose number is being typed, and the digits so far.
    pub editing: Option<(usize, String)>,
}

impl Overrides {
    pub fn new(open: bool) -> Self {
        Self {
            open,
            ..Default::default()
        }
    }
}

/// The weapon preset with the turned weapon knobs on top of the power
/// scaling: the knob shows the value in force, and the value it sets is
/// the one in force whatever the power button says.
pub fn effective_config(energy: bool, anni: bool, power: u32, o: &Overrides) -> ExplosionConfig {
    let mut c = scaled_config(energy, anni, power);
    for (key, v) in &o.values {
        if let Some(f) = key.strip_prefix("weapon.") {
            set_weapon(&mut c, f, *v);
        }
    }
    c
}
fn u(v: f64) -> u32 {
    v.round().clamp(0.0, u32::MAX as f64) as u32
}
fn i(v: f64) -> i32 {
    v.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}
fn set_weapon(c: &mut ExplosionConfig, f: &str, v: f64) {
    match f {
        "energy" => c.energy = u(v).clamp(1, 1_000_000),
        "material_energy_j" => c.material_energy_j = u(v),
        "hull_impulse_milli" => c.hull_impulse_milli = u(v).min(100_000),
        "soil_resistance" => c.soil_resistance = u(v).max(1),
        "concrete_resistance" => c.concrete_resistance = u(v).max(1),
        "steel_resistance" => c.steel_resistance = u(v).max(1),
        "armour_milli" => c.unit_role_resistance_milli[0] = u(v).max(1),
        "fragment_box" => c.fragment_box = u(v).clamp(1, 4),
        "heat_units" => c.heat_units = i(v).max(0),
        "heat_per_cell" => c.heat_per_cell = u(v),
        "gas_source_radius" => c.gas_source_radius = i(v).max(1),
        "wave_work_per_tick" => c.wave_work_per_tick = u(v).max(64) as usize,
        _ => {}
    }
}
fn get_weapon(c: &ExplosionConfig, f: &str) -> Option<f64> {
    Some(match f {
        "energy" => f64::from(c.energy),
        "material_energy_j" => f64::from(c.material_energy_j),
        "hull_impulse_milli" => f64::from(c.hull_impulse_milli),
        "soil_resistance" => f64::from(c.soil_resistance),
        "concrete_resistance" => f64::from(c.concrete_resistance),
        "steel_resistance" => f64::from(c.steel_resistance),
        "armour_milli" => f64::from(c.unit_role_resistance_milli[0]),
        "fragment_box" => f64::from(c.fragment_box),
        "heat_units" => f64::from(c.heat_units),
        "heat_per_cell" => f64::from(c.heat_per_cell),
        "gas_source_radius" => f64::from(c.gas_source_radius),
        "wave_work_per_tick" => c.wave_work_per_tick as f64,
        _ => return None,
    })
}
fn chassis_field<'a>(c: &'a mut Chassis, f: &str) -> Option<&'a mut i32> {
    Some(match f {
        "stiffness" => &mut c.stiffness,
        "damping" => &mut c.damping,
        "travel_cm" => &mut c.travel_cm,
        "clearance_cm" => &mut c.clearance_cm,
        "inertia" => &mut c.inertia,
        "ang_damp" => &mut c.ang_damp,
        "max_tilt_pct" => &mut c.max_tilt_pct,
        "contact_cm" => &mut c.contact_cm,
        "approach_pct" => &mut c.approach_pct,
        "settle_pct" => &mut c.settle_pct,
        "settle_tilt_pct" => &mut c.settle_tilt_pct,
        "thrust" => &mut c.thrust,
        "brake" => &mut c.brake,
        "turn" => &mut c.turn,
        "climb" => &mut c.climb,
        "descend" => &mut c.descend,
        _ => return None,
    })
}
fn stock_chassis(kind: RangeUnitKind) -> Chassis {
    let t = modding::tuning::Tuning::from_ron(include_str!("../../../../../data/sim.ron"))
        .expect("sim tuning");
    if kind == RangeUnitKind::Tank {
        t.tracked
    } else {
        t.walking
    }
}
fn kind_of(key: &str) -> Option<(RangeUnitKind, &str)> {
    if let Some(f) = key.strip_prefix("tank.") {
        Some((RangeUnitKind::Tank, f))
    } else {
        key.strip_prefix("walker.")
            .map(|f| (RangeUnitKind::Strider, f))
    }
}
fn motion_get(m: &Motion, f: &str) -> Option<f64> {
    match f {
        "speed" => Some(f64::from(m.speed)),
        "step" => Some(f64::from(m.step)),
        _ => {
            let mut c = m.chassis;
            chassis_field(&mut c, f).map(|v| f64::from(*v))
        }
    }
}
fn motion_set(m: &mut Motion, f: &str, v: f64) {
    match f {
        "speed" => m.speed = i(v).max(1),
        "step" => m.step = i(v).clamp(1, 8),
        _ => {
            if let Some(slot) = chassis_field(&mut m.chassis, f) {
                *slot = i(v);
            }
        }
    }
}
fn debris_get(t: &modding::tuning::Blast, f: &str) -> Option<f64> {
    Some(match f {
        "drag_pct" => f64::from(t.drag_pct),
        "bounce_pct" => f64::from(t.bounce_pct),
        "friction_pct" => f64::from(t.friction_pct),
        "sleep_mm_s" => f64::from(t.sleep_mm_s),
        "push_pct" => f64::from(t.push_pct),
        "push_up_pct" => f64::from(t.push_up_pct),
        "rest_check_ticks" => f64::from(t.rest_check_ticks),
        "pile_cell" => f64::from(t.pile_cell),
        "lift_pct" => f64::from(t.lift_pct),
        "motion_pct" => f64::from(t.motion_pct),
        "max_flying" => f64::from(t.max_flying),
        _ => return None,
    })
}
fn debris_set(t: &mut modding::tuning::Blast, f: &str, v: f64) {
    match f {
        "drag_pct" => t.drag_pct = u(v).min(100),
        "bounce_pct" => t.bounce_pct = u(v).min(99),
        "friction_pct" => t.friction_pct = u(v).min(100),
        "sleep_mm_s" => t.sleep_mm_s = u(v).max(1),
        "push_pct" => t.push_pct = u(v),
        "push_up_pct" => t.push_up_pct = u(v).min(100),
        "rest_check_ticks" => t.rest_check_ticks = u(v).max(1),
        "pile_cell" => t.pile_cell = u(v).max(1),
        "lift_pct" => t.lift_pct = u(v).min(100),
        "motion_pct" => t.motion_pct = u(v).min(99),
        "max_flying" => t.max_flying = u(v).clamp(1, t.max_fragments),
        _ => {}
    }
}
fn material_key(key: &str) -> Option<(&str, u16)> {
    let (field, id) = key.split_once('.')?;
    if !matches!(field, "density" | "strength" | "melt") {
        return None;
    }
    Some((field, id.parse().ok()?))
}

/// What the picture reads besides the look: the gas overlay's own knobs.
fn picture_get(f: &str, gas: &GasInput) -> Option<f64> {
    let s = look::scene();
    let d = look::dust();
    Some(match f {
        "smoke_extinction" => f64::from(s.smoke_extinction),
        "ground_glow" => f64::from(gas.params.look.x),
        "bloom" => f64::from(s.bloom),
        "ambient_brightness" => f64::from(s.ambient_brightness),
        "blast_size" => f64::from(d.blast_size.1),
        "blast_puffs" => d.blast_puffs as f64,
        "spark_size" => f64::from(d.spark_size),
        "smoke_size" => f64::from(d.smoke_size.1),
        "dust_alpha" => f64::from(d.dust_alpha),
        _ => return None,
    })
}

/// The value a knob has in force right now.
pub fn read(key: &str, scene: &Scene, gas: &GasInput) -> Option<f64> {
    if let Some(f) = key.strip_prefix("weapon.") {
        return get_weapon(&scene.sim.config, f);
    }
    if let Some((kind, f)) = kind_of(key) {
        let live = if kind == RangeUnitKind::Tank {
            Some(&scene.sim.gun.motion)
        } else {
            scene
                .sim
                .units
                .iter()
                .find(|u| u.kind == kind)
                .map(|u| &u.motion)
        };
        return match live {
            Some(m) => motion_get(m, f),
            None => {
                let course = scene.sim.layout.as_ref().and_then(|l| l.course.as_ref());
                match f {
                    "speed" => course.map(|c| f64::from(c.walker_speed)),
                    "step" => course.map(|c| f64::from(c.step_height)),
                    _ => {
                        let mut c = stock_chassis(kind);
                        chassis_field(&mut c, f).map(|v| f64::from(*v))
                    }
                }
            }
        };
    }
    if let Some(f) = key.strip_prefix("debris.") {
        return debris_get(&scene.sim.matter.tuning, f);
    }
    if let Some((field, id)) = material_key(key) {
        let d = scene.sim.matter.economy.mats.get(id)?;
        return Some(f64::from(match field {
            "density" => d.density,
            "strength" => d.strength,
            _ => d.melt,
        }));
    }
    if let Some(f) = key.strip_prefix("picture.") {
        return picture_get(f, gas);
    }
    None
}

/// Everything a knob may reach when it is turned.
pub struct Reach<'a, 'w, 's, 'b> {
    pub scene: &'a mut Scene,
    pub gas: &'a mut GasInput,
    pub ambient: &'a mut GlobalAmbientLight,
    pub bloom: &'a mut Query<'w, 's, &'b mut Bloom>,
}

/// Put a value in force. Weapon knobs rebuild the preset from the power
/// scaling and every weapon override, so `o` must already hold the value.
pub fn apply(key: &str, v: f64, r: &mut Reach, o: &Overrides) {
    if key.starts_with("weapon.") {
        let cfg = effective_config(r.scene.energy, r.scene.anni, r.scene.power, o);
        r.scene.sim.reconfigure(cfg);
        let origin = r.gas.params.origin;
        configure_gas(&r.scene.sim, r.scene.energy, r.scene.anni, r.gas);
        r.gas.params.origin = origin;
        return;
    }
    if let Some((kind, f)) = kind_of(key) {
        if kind == RangeUnitKind::Tank {
            motion_set(&mut r.scene.sim.gun.motion, f, v);
        }
        for u in r.scene.sim.units.iter_mut().filter(|u| u.kind == kind) {
            motion_set(&mut u.motion, f, v);
        }
        return;
    }
    if let Some(f) = key.strip_prefix("debris.") {
        debris_set(&mut r.scene.sim.matter.tuning, f, v);
        return;
    }
    if let Some((field, id)) = material_key(key) {
        let mats = &mut r.scene.sim.matter.economy.mats;
        if let Some(d) = mats.get(id) {
            let mut d = *d;
            match field {
                "density" => d.density = u(v).max(1),
                "strength" => d.strength = u(v).max(1),
                _ => d.melt = u(v),
            }
            let _ = mats.replace(d);
        }
        return;
    }
    if let Some(f) = key.strip_prefix("picture.") {
        let mut s = look::scene();
        let mut d = look::dust();
        let x = v as f32;
        match f {
            "smoke_extinction" => {
                s.smoke_extinction = x;
                r.gas.params.weapon.w = x;
            }
            "ground_glow" => r.gas.params.look.x = x,
            "bloom" => {
                s.bloom = x;
                for mut b in r.bloom.iter_mut() {
                    b.intensity = x;
                }
            }
            "ambient_brightness" => {
                s.ambient_brightness = x;
                r.ambient.brightness = x;
            }
            "blast_size" => d.blast_size.1 = x,
            "blast_puffs" => d.blast_puffs = v.round().max(0.0) as usize,
            "spark_size" => d.spark_size = x,
            "smoke_size" => d.smoke_size.1 = x,
            "dust_alpha" => d.dust_alpha = x,
            _ => {}
        }
        look::override_scene(Some(s));
        look::override_dust(Some(d));
    }
}

fn save(o: &Overrides) -> Result<String, String> {
    let text: String = o
        .values
        .iter()
        .map(|(key, v)| format!("{key} {v}\n"))
        .collect();
    std::fs::create_dir_all("out").map_err(|e| e.to_string())?;
    std::fs::write(FILE, text).map_err(|e| e.to_string())?;
    Ok(format!("Записано: {FILE} ({} значений)", o.values.len()))
}
fn load() -> Result<Vec<(&'static Knob, f64)>, String> {
    let text = std::fs::read_to_string(FILE).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Some(knob), Ok(v)) = (knob(key), value.parse::<f64>()) else {
            continue;
        };
        out.push((knob, v.clamp(knob.min, knob.max)));
    }
    Ok(out)
}

// ---------------------------------------------------------------- the panel

#[derive(Component)]
pub struct SettingsRoot;
#[derive(Component)]
pub struct Slot(pub usize);
#[derive(Component)]
pub struct SlotLabel(pub usize);
#[derive(Component)]
pub struct SlotValue(pub usize);
#[derive(Component)]
pub struct KnobNotice;

/// The settings panel to the right of the main one, hidden until asked.
pub fn build(commands: &mut Commands, camera: Entity, text_font: &TextFont) {
    let button = |label: &str, action: Action, width: Val| {
        (
            Button,
            action,
            Node {
                width,
                height: px(26),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                padding: UiRect::horizontal(px(3)),
                border_radius: BorderRadius::all(px(5)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.075, 0.13, 0.15)),
            children![(
                Text::new(label),
                text_font.clone(),
                TextColor(Color::srgb(0.82, 0.9, 0.91)),
            )],
        )
    };
    commands
        .spawn((
            SettingsRoot,
            super::panel::PanelRoot,
            bevy::ui::RelativeCursorPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                left: px(310),
                top: px(16),
                width: px(330),
                padding: UiRect::all(px(12)),
                row_gap: px(4),
                flex_direction: FlexDirection::Column,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(10)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.025, 0.045, 0.06, 0.96)),
            BorderColor::all(Color::srgb(0.17, 0.29, 0.32)),
            UiTargetCamera(camera),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("НАСТРОЙКИ"),
                TextFont {
                    font_size: FontSize::Px(19.0),
                    ..text_font.clone()
                },
                TextColor(Color::srgb(0.83, 0.94, 0.93)),
            ));
            p.spawn((
                Text::new(
                    "Щелчок по числу — ввод с клавиатуры, Enter; − и + на шаг, с Shift на десять",
                ),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..text_font.clone()
                },
                TextColor(Color::srgb(0.48, 0.65, 0.68)),
            ));
            for row in [&Group::ALL[..4], &Group::ALL[4..]] {
                p.spawn(Node {
                    width: percent(100),
                    column_gap: px(4),
                    ..default()
                })
                .with_children(|r| {
                    for g in row {
                        let index = Group::ALL.iter().position(|x| x == g).unwrap_or(0);
                        r.spawn(button(
                            g.title(),
                            Action::Group(index as u8),
                            percent(100.0 / row.len() as f32),
                        ));
                    }
                });
            }
            for slot in 0..SLOTS {
                p.spawn((
                    Slot(slot),
                    Node {
                        width: percent(100),
                        column_gap: px(4),
                        align_items: AlignItems::Center,
                        display: Display::None,
                        ..default()
                    },
                ))
                .with_children(|r| {
                    r.spawn((
                        SlotLabel(slot),
                        Text::new(""),
                        TextFont {
                            font_size: FontSize::Px(13.0),
                            ..text_font.clone()
                        },
                        TextColor(Color::srgb(0.72, 0.84, 0.85)),
                        Node {
                            flex_grow: 1.0,
                            ..default()
                        },
                    ));
                    r.spawn((
                        Button,
                        Action::EditKnob(slot as u8),
                        SlotValue(slot),
                        Text::new(""),
                        TextFont {
                            font_size: FontSize::Px(13.0),
                            ..text_font.clone()
                        },
                        TextColor(Color::srgb(0.9, 0.96, 0.95)),
                        Node {
                            width: px(78),
                            justify_content: JustifyContent::FlexEnd,
                            padding: UiRect::horizontal(px(4)),
                            border_radius: BorderRadius::all(px(4)),
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                    ));
                    r.spawn(button("−", Action::Knob(slot as u8, false), px(28)));
                    r.spawn(button("+", Action::Knob(slot as u8, true), px(28)));
                });
            }
            p.spawn(Node {
                width: percent(100),
                column_gap: px(4),
                margin: UiRect::top(px(6)),
                ..default()
            })
            .with_children(|r| {
                r.spawn(button("Сохранить", Action::SaveKnobs, percent(33.0)));
                r.spawn(button("Загрузить", Action::LoadKnobs, percent(33.0)));
                r.spawn(button("Сбросить", Action::ResetKnobs, percent(33.0)));
            });
            p.spawn((
                KnobNotice,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..text_font.clone()
                },
                TextColor(Color::srgb(0.61, 0.77, 0.78)),
            ));
        });
}

/// The character a key types into a number, if any.
fn typed(key: KeyCode) -> Option<char> {
    Some(match key {
        KeyCode::Digit0 | KeyCode::Numpad0 => '0',
        KeyCode::Digit1 | KeyCode::Numpad1 => '1',
        KeyCode::Digit2 | KeyCode::Numpad2 => '2',
        KeyCode::Digit3 | KeyCode::Numpad3 => '3',
        KeyCode::Digit4 | KeyCode::Numpad4 => '4',
        KeyCode::Digit5 | KeyCode::Numpad5 => '5',
        KeyCode::Digit6 | KeyCode::Numpad6 => '6',
        KeyCode::Digit7 | KeyCode::Numpad7 => '7',
        KeyCode::Digit8 | KeyCode::Numpad8 => '8',
        KeyCode::Digit9 | KeyCode::Numpad9 => '9',
        KeyCode::Period | KeyCode::Comma | KeyCode::NumpadDecimal => '.',
        KeyCode::Minus | KeyCode::NumpadSubtract => '-',
        _ => return None,
    })
}
fn shown(o: &Overrides) -> Vec<&'static Knob> {
    let group = Group::ALL[o.group.min(Group::ALL.len() - 1)];
    knobs().filter(|k| k.group == group).take(SLOTS).collect()
}
fn format_value(knob: &Knob, v: f64) -> String {
    if knob.step >= 1.0 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.2}")
    }
}

/// The knob presses: a step per press, ten with Shift; the file buttons;
/// on the first frame the base values and the saved file; after a rebuild
/// of the range, every turned knob again.
#[allow(clippy::too_many_arguments)]
pub fn actions(
    mut o: ResMut<Overrides>,
    mut scene: ResMut<Scene>,
    mut gas: ResMut<GasInput>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut bloom: Query<&mut Bloom>,
    interactions: Query<(&Action, &Interaction), Changed<Interaction>>,
    keys: Res<ButtonInput<KeyCode>>,
    edit: Res<Workshop>,
) {
    if edit.layout.is_none() {
        return;
    }
    let mut reach = Reach {
        scene: &mut scene,
        gas: &mut gas,
        ambient: &mut ambient,
        bloom: &mut bloom,
    };
    if !o.started {
        o.started = true;
        for knob in knobs() {
            if let Some(v) = read(knob.key, reach.scene, reach.gas) {
                o.base.insert(knob.key, v);
            }
        }
        if std::path::Path::new(FILE).is_file() {
            match load() {
                Ok(list) => {
                    for (knob, v) in &list {
                        o.values.insert(knob.key, *v);
                    }
                    for (knob, v) in &list {
                        apply(knob.key, *v, &mut reach, &o);
                    }
                    o.notice = format!("Загружено при старте: {} значений", list.len());
                }
                Err(e) => o.notice = format!("Не прочитан {FILE}: {e}"),
            }
        }
    }
    if reach.scene.sim.tick < o.last_tick {
        // The range was rebuilt from the layout: its units and debris
        // tuning are stock again, and the turned knobs go back on.
        let again: Vec<(&'static str, f64)> = o
            .values
            .iter()
            .filter(|(k, _)| !k.starts_with("weapon."))
            .map(|(k, v)| (*k, *v))
            .collect();
        for (key, v) in again {
            apply(key, v, &mut reach, &o);
        }
    }
    o.last_tick = reach.scene.sim.tick;
    // A number typed for the row under edit: digits, a point, a leading
    // minus, Backspace; Enter puts it in force, Escape drops it.
    if let Some((slot, mut buffer)) = o.editing.take() {
        let mut keep = true;
        for key in keys.get_just_pressed() {
            match key {
                KeyCode::Enter | KeyCode::NumpadEnter => {
                    keep = false;
                    let knob = shown(&o).get(slot).copied();
                    match (knob, buffer.trim().parse::<f64>()) {
                        (Some(knob), Ok(v)) => {
                            let v = v.clamp(knob.min, knob.max);
                            o.values.insert(knob.key, v);
                            apply(knob.key, v, &mut reach, &o);
                            o.notice = format!("{} = {}", knob.label, format_value(knob, v));
                        }
                        (Some(knob), Err(_)) if !buffer.trim().is_empty() => {
                            o.notice = format!("{}: не число «{buffer}»", knob.label);
                        }
                        _ => {}
                    }
                }
                KeyCode::Escape => keep = false,
                KeyCode::Backspace => {
                    buffer.pop();
                }
                key => {
                    if let Some(c) = typed(*key) {
                        if c != '-' || buffer.is_empty() {
                            buffer.push(c);
                        }
                    }
                }
            }
        }
        if keep {
            o.editing = Some((slot, buffer));
        }
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    for (&action, interaction) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            Action::Settings => o.open = !o.open,
            Action::EditKnob(slot) => {
                let slot = usize::from(slot);
                o.editing = match o.editing.take() {
                    Some((s, _)) if s == slot => None,
                    _ if slot < shown(&o).len() => Some((slot, String::new())),
                    _ => None,
                };
            }
            Action::Group(g) => o.group = usize::from(g).min(Group::ALL.len() - 1),
            Action::Knob(slot, up) => {
                let Some(knob) = shown(&o).get(usize::from(slot)).copied() else {
                    continue;
                };
                let Some(current) = read(knob.key, reach.scene, reach.gas) else {
                    continue;
                };
                let step = knob.step * if shift { 10.0 } else { 1.0 };
                let v = (current + if up { step } else { -step }).clamp(knob.min, knob.max);
                o.values.insert(knob.key, v);
                apply(knob.key, v, &mut reach, &o);
                o.notice = format!("{} = {}", knob.label, format_value(knob, v));
            }
            Action::SaveKnobs => {
                o.notice = match save(&o) {
                    Ok(m) | Err(m) => m,
                };
            }
            Action::LoadKnobs => match load() {
                Ok(list) => {
                    for (knob, v) in &list {
                        o.values.insert(knob.key, *v);
                    }
                    for (knob, v) in &list {
                        apply(knob.key, *v, &mut reach, &o);
                    }
                    o.notice = format!("Загружено: {} значений из {FILE}", list.len());
                }
                Err(e) => o.notice = format!("Не прочитан {FILE}: {e}"),
            },
            Action::ResetKnobs => {
                let keys: Vec<&'static str> = o.values.keys().copied().collect();
                o.values.clear();
                for key in keys {
                    if let Some(v) = o.base.get(key).copied() {
                        apply(key, v, &mut reach, &o);
                    }
                }
                // Weapon knobs come back with the preset itself.
                apply("weapon.", 0.0, &mut reach, &o);
                o.notice = "Сброшено к исходным".into();
            }
            _ => {}
        }
    }
}

/// The panel's picture: hidden or shown, the active group, the rows of its
/// knobs with a mark on the turned ones, and the notice.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn refresh(
    scene: Res<Scene>,
    gas: Res<GasInput>,
    o: Res<Overrides>,
    mut roots: Query<&mut Node, With<SettingsRoot>>,
    mut slots: Query<(&Slot, &mut Node), Without<SettingsRoot>>,
    mut labels: Query<(&SlotLabel, &mut Text)>,
    mut values: Query<(&SlotValue, &mut Text), Without<SlotLabel>>,
    mut buttons: Query<(&Action, &Interaction, &mut BackgroundColor)>,
    mut notice: Query<&mut Text, (With<KnobNotice>, Without<SlotLabel>, Without<SlotValue>)>,
) {
    for mut n in &mut roots {
        n.display = if o.open { Display::Flex } else { Display::None };
    }
    if !o.open {
        return;
    }
    let list = shown(&o);
    for (slot, mut n) in &mut slots {
        n.display = if slot.0 < list.len() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (slot, mut t) in &mut labels {
        if let Some(knob) = list.get(slot.0) {
            let mark = if o.values.contains_key(knob.key) {
                " •"
            } else {
                ""
            };
            *t = Text::new(format!("{}{mark}", knob.label));
        }
    }
    for (slot, mut t) in &mut values {
        if let Some(knob) = list.get(slot.0) {
            let typed = o
                .editing
                .as_ref()
                .filter(|(s, _)| *s == slot.0)
                .map(|(_, buffer)| format!("{buffer}_"));
            let v = read(knob.key, &scene, &gas);
            *t =
                Text::new(typed.unwrap_or_else(|| {
                    v.map_or_else(|| "—".to_string(), |v| format_value(knob, v))
                }));
        }
    }
    for (a, interaction, mut color) in &mut buttons {
        let active = match *a {
            Action::Settings => o.open,
            Action::Group(g) => usize::from(g) == o.group,
            Action::EditKnob(slot) => {
                let editing = o
                    .editing
                    .as_ref()
                    .is_some_and(|(s, _)| *s == usize::from(slot));
                color.0 = if editing {
                    Color::srgb(0.12, 0.34, 0.36)
                } else if *interaction == Interaction::Hovered {
                    Color::srgb(0.1, 0.2, 0.22)
                } else {
                    Color::NONE
                };
                continue;
            }
            _ => continue,
        };
        color.0 = if *interaction == Interaction::Hovered {
            Color::srgb(0.16, 0.28, 0.3)
        } else if active {
            Color::srgb(0.09, 0.31, 0.32)
        } else {
            Color::srgb(0.075, 0.13, 0.15)
        };
    }
    for mut t in &mut notice {
        *t = Text::new(o.notice.clone());
    }
}
