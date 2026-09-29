//! Placement tools for the existing forest range. Edits replace a validated
//! pristine fixture; damage never leaks back into the authored layout.
#[path = "workbench/ghost.rs"]
pub mod ghost;
use super::*;
use bevy::camera::visibility::RenderLayers;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use modding::range::{RangeBlock, RangeShieldDef, RangeUnitDef};

fn world_battle_blocks() -> Result<Vec<RangeBlock>, String> {
    Ok(modding::range_skirmish::WorldBattle::from_ron(include_str!(
        "../../../../../data/battle-world.ron"
    ))?
    .blocks)
}

fn remove_world_battle_blocks(layout: &mut RangeLayout, blocks: &[RangeBlock]) {
    layout.blocks.retain(|block| !blocks.contains(block));
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Tool {
    #[default]
    Select,
    Tank,
    Strider,
    Spider,
    Artillery,
    TurretT1,
    TurretT2,
    Base,
    Concrete,
    Steel,
    Earth,
    Clear,
    Aim,
    Shield,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Selection {
    Unit(usize),
    Block(usize),
    Shield(usize),
}
/// Rounds caught by domes, kept for a few frames after their tick.
#[derive(Resource, Default)]
pub struct ShieldFlashes {
    seen: u32,
    live: Vec<ShieldFlash>,
}
struct ShieldFlash {
    at: Vec3,
    centre: Vec3,
    tick: u32,
    collapsed: bool,
}
#[derive(Component)]
pub struct EditorCamera;

pub fn setup(
    mut commands: Commands,
    edit: Res<Workshop>,
    mut config: ResMut<GizmoConfigStore>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if edit.layout.is_none() {
        return;
    }
    ghost::setup(&mut commands, &mut meshes, &mut materials);
    // The gas composite follows the world pass and would cover ordinary
    // gizmos. Draw interaction handles with a second, transparent camera.
    let (style, _) = config.config_mut::<DefaultGizmoConfigGroup>();
    style.render_layers = RenderLayers::layer(1);
    style.line.width = 2.0;
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            output_mode: bevy::camera::CameraOutputMode::Write {
                blend_state: Some(
                    bevy::render::render_resource::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                ),
                clear_color: ClearColorConfig::None,
            },
            is_active: true,
            ..default()
        },
        RenderLayers::layer(1),
        EditorCamera,
        Msaa::Off,
        Projection::Orthographic(OrthographicProjection::default_3d()),
        Transform::default(),
    ));
}
// Explicit disjoint filters let Bevy verify the mutable camera borrows.
#[allow(clippy::type_complexity)]
pub fn camera(
    edit: Res<Workshop>,
    main: Query<(&Transform, &Projection), (With<GasCamera>, Without<EditorCamera>)>,
    mut overlay: Query<
        (&mut Transform, &mut Projection, &mut Camera),
        (With<EditorCamera>, Without<GasCamera>),
    >,
) {
    let Ok((t, p)) = main.single() else { return };
    for (mut ot, mut op, mut c) in &mut overlay {
        *ot = *t;
        *op = p.clone();
        c.is_active = edit.layout.is_some();
    }
}

#[derive(Resource)]
pub struct Workshop {
    pub layout: Option<RangeLayout>,
    pub editing: bool,
    pub allied: bool,
    pub controlled: Option<usize>,
    pub selected_units: Vec<usize>,
    pub tool: Tool,
    pub selected: Option<Selection>,
    pub pan: Vec2,
    pub zoom: f32,
    pub zone: usize,
    pub orbit: f32,
    pub pitch: f32,
    rotate: bool,
    history: Vec<RangeLayout>,
    redo: Vec<RangeLayout>,
    hover: Option<[i32; 3]>,
    preview: Option<Preview>,
    pub(super) validator: sim_core::blast::placement::Validator,
    previous_cursor: Option<Vec2>,
    pub drag_start: Option<Vec2>,
    pub drag_current: Option<Vec2>,
    pub pending: Option<(ExplosionConfig, RangeLayout)>,
}
impl Workshop {
    pub fn new(layout: Option<RangeLayout>, editing: bool) -> Self {
        let selected = if editing
            && layout
                .as_ref()
                .is_some_and(|l| l.zones.is_empty() && !l.units.is_empty())
        {
            Some(Selection::Unit(0))
        } else {
            None
        };
        let zone = layout
            .as_ref()
            .and_then(|r| r.zones.iter().position(|z| z.spawn == r.gun))
            .unwrap_or(0);
        Self {
            layout,
            editing,
            allied: true,
            controlled: None,
            selected_units: Vec::new(),
            zone,
            orbit: 0.0,
            pitch: 0.0,
            tool: Tool::Select,
            selected,
            pan: Vec2::ZERO,
            zoom: 1.0,
            rotate: false,
            history: Vec::new(),
            redo: Vec::new(),
            hover: None,
            preview: None,
            validator: default(),
            previous_cursor: None,
            drag_start: None,
            drag_current: None,
            pending: None,
        }
    }
    pub fn battle_selected_ids(&self, sim: &BlastRange) -> Vec<usize> {
        let ids: Vec<_> = if self.selected_units.is_empty() {
            self.controlled.into_iter().collect()
        } else {
            self.selected_units.clone()
        };
        ids.into_iter()
            .filter(|&id| sim.units.get(id).is_some_and(|u| u.status.alive) && sim.allied(id))
            .collect()
    }
    pub fn select_battle_group(&mut self, sim: &BlastRange, ids: &[usize], additive: bool) {
        if additive && self.selected_units.is_empty() {
            if let Some(id) = self
                .controlled
                .filter(|&i| sim.units.get(i).is_some_and(|u| u.status.alive) && sim.allied(i))
            {
                self.selected_units.push(id);
            }
        }
        if !additive {
            self.selected_units.clear();
        }
        for &id in ids {
            if sim.units.get(id).is_some_and(|u| u.status.alive)
                && sim.allied(id)
                && !self.selected_units.contains(&id)
            {
                self.selected_units.push(id);
            }
        }
        self.selected_units.sort_unstable();
        self.controlled = self
            .controlled
            .filter(|id| self.selected_units.contains(id))
            .or_else(|| self.selected_units.first().copied());
        self.selected = self.controlled.map(Selection::Unit);
    }
    pub fn select_battle_unit(&mut self, sim: &BlastRange, id: usize, additive: bool) {
        if additive && self.selected_units.contains(&id) {
            self.selected_units.retain(|&i| i != id);
            self.controlled = self.selected_units.first().copied();
            self.selected = self.controlled.map(Selection::Unit);
        } else {
            self.select_battle_group(sim, &[id], additive);
            self.controlled = Some(id).filter(|&i| self.selected_units.contains(&i));
            self.selected = self.controlled.map(Selection::Unit);
        }
    }
    pub fn battle_preset(&mut self, config: ExplosionConfig) -> Result<(), String> {
        let tuning = modding::range_battle::Tuning::from_ron(include_str!(
            "../../../../../data/range-battle.ron"
        ))?;
        let mut layout = self.layout.clone().ok_or("No range layout")?;
        remove_world_battle_blocks(&mut layout, &world_battle_blocks()?);
        layout.battle = true;
        layout.units = tuning.units;
        layout.gun = tuning.zone.spawn;
        layout.target = tuning.zone.target;
        if let Some(c) = &mut layout.course {
            c.patrols.clear();
        }
        let zone = if let Some(i) = layout.zones.iter().position(|z| z.name == tuning.zone.name) {
            i
        } else {
            layout.zones.push(tuning.zone);
            layout.zones.len() - 1
        };
        self.commit(layout, config)?;
        self.zone = zone;
        self.editing = true;
        self.selected = None;
        self.controlled = Some(0);
        self.selected_units = vec![0];
        self.pan = Vec2::ZERO;
        self.zoom = 0.85;
        self.orbit = 0.0;
        self.pitch = 0.0;
        Ok(())
    }
    pub fn skirmish(&mut self, id: u8, config: ExplosionConfig) -> Result<String, String> {
        let preset = modding::range_skirmish::Skirmish::from_ron(match id {
            0 => include_str!("../../../../../data/skirmish-breakthrough.ron"),
            1 => include_str!("../../../../../data/skirmish-pincer.ron"),
            2 => include_str!("../../../../../data/skirmish-spider.ron"),
            3 => include_str!("../../../../../data/skirmish-turrets.ron"),
            _ => return Err("Неизвестный сценарий боя".into()),
        })?;
        let mut layout = RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron"))?;
        layout.battle = true;
        layout.units = preset.units;
        layout.gun = preset.zone.spawn;
        layout.target = preset.zone.target;
        layout.blocks.extend(preset.blocks);
        // Replace the existing flat battle sector, keeping one coherent map.
        let stock = modding::range_battle::Tuning::from_ron(include_str!(
            "../../../../../data/range-battle.ron"
        ))?;
        let zone = layout
            .zones
            .iter()
            .position(|z| z.name == stock.zone.name)
            .ok_or("Нет боевого участка")?;
        layout.zones[zone] = preset.zone;
        if let Some(course) = &mut layout.course {
            course.patrols.clear();
        }
        self.commit(layout, config)?;
        self.zone = zone;
        self.editing = true;
        self.selected = Some(Selection::Unit(0));
        self.controlled = Some(0);
        self.selected_units = vec![0];
        self.pan = Vec2::ZERO;
        self.zoom = if id == 3 { 1.35 } else { 0.85 };
        self.orbit = 0.0;
        self.pitch = 0.0;
        Ok(preset.briefing)
    }
    pub fn battle_world(&mut self, config: ExplosionConfig) -> Result<String, String> {
        let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
            "../../../../../data/battle-world.ron"
        ))?;
        let mut layout = self.layout.clone().ok_or("No range layout")?;
        remove_world_battle_blocks(&mut layout, &preset.blocks);
        layout.battle = true;
        layout.units = preset.units;
        layout.blocks.extend(preset.blocks);
        if let Some(course) = &mut layout.course {
            course.patrols.clear();
        }
        let centre = [layout.size[0] as i32 / 2, layout.size[2] as i32 / 2];
        let focus = layout
            .units
            .iter()
            .filter(|u| u.allied && !u.kind.emplacement())
            .min_by_key(|u| (u.at[0] - centre[0]).abs() + (u.at[2] - centre[1]).abs())
            .map(|u| [u.at[0], u.at[2]]);
        let focus_zone = focus.and_then(|at| layout.zones.iter().position(|z| z.contains(at, 0)));
        let controlled = layout
            .units
            .iter()
            .position(|u| u.allied && !u.kind.emplacement());
        self.commit(layout, config)?;
        self.editing = true;
        self.allied = true;
        self.controlled = controlled;
        self.selected = self.controlled.map(Selection::Unit);
        self.selected_units = self.controlled.into_iter().collect();
        if let Some(zone) = focus_zone {
            self.zone = zone;
        }
        self.pan = Vec2::ZERO;
        self.zoom = 1.0;
        self.orbit = 0.0;
        self.pitch = 0.0;
        Ok(preset.briefing)
    }
    pub fn range_mode(&mut self, config: ExplosionConfig) -> Result<(), String> {
        let mut layout = self.layout.clone().ok_or("No range layout")?;
        remove_world_battle_blocks(&mut layout, &world_battle_blocks()?);
        layout.battle = false;
        self.commit(layout, config)?;
        self.controlled = None;
        self.selected_units.clear();
        self.selected = None;
        self.editing = false;
        Ok(())
    }
    pub fn side(&mut self, allied: bool, config: ExplosionConfig) -> Result<(), String> {
        self.allied = allied;
        self.preview = None;
        if let Some(mut layout) = self.layout.clone() {
            layout.battle = true;
            if let Some(Selection::Unit(i)) = self.selected {
                if let Some(u) = layout.units.get_mut(i) {
                    u.allied = allied;
                }
            }
            self.commit(layout, config)?;
        }
        Ok(())
    }
    pub fn sector(&self) -> Option<&modding::range::RangeZone> {
        self.layout.as_ref()?.zones.get(self.zone)
    }
    pub fn change_zone(&mut self, step: i32) {
        let count = self.layout.as_ref().map_or(0, |r| r.zones.len());
        if count == 0 {
            return;
        }
        self.zone = (self.zone as i32 + step).rem_euclid(count as i32) as usize;
        self.selected = None;
        self.pan = Vec2::ZERO;
        self.zoom = 1.0;
        self.orbit = 0.0;
        self.pitch = 0.0;
        self.hover = None;
        self.preview = None;
    }
    pub fn target_unit(&self, attack: Option<usize>, patrol_number: usize) -> Option<usize> {
        let r = self.layout.as_ref()?;
        if let Some(i) = attack.filter(|&i| i < r.units.len()) {
            return Some(i);
        }
        let local = |i: usize| {
            r.units.get(i).is_some_and(|u| {
                self.sector()
                    .is_none_or(|z| z.contains([u.at[0], u.at[2]], 0))
            })
        };
        let patrols = r.course.as_ref().map_or(&[][..], |c| c.patrols.as_slice());
        if let Some(Selection::Unit(i)) = self.selected {
            if local(i) && (self.sector().is_some() || patrols.iter().any(|p| p.unit == i)) {
                return Some(i);
            }
        }
        patrols
            .get(patrol_number.saturating_sub(1))
            .filter(|p| local(p.unit))
            .or_else(|| patrols.iter().find(|p| local(p.unit)))
            .map(|p| p.unit)
            .or_else(|| {
                self.sector()
                    .and_then(|_| (0..r.units.len()).find(|&i| local(i)))
            })
    }
    pub fn pan_by(&mut self, delta: Vec2) {
        let Some(layout) = &self.layout else { return };
        let centre =
            self.sector()
                .map_or(Vec2::new(layout.gun[0] as f32, layout.gun[2] as f32), |z| {
                    Vec2::new(
                        (z.min[0] + z.max[0]) as f32 * 0.5,
                        (z.min[1] + z.max[1]) as f32 * 0.5,
                    )
                });
        self.pan = (centre + self.pan + delta).clamp(
            Vec2::ZERO,
            Vec2::new(layout.size[0] as f32, layout.size[2] as f32),
        ) - centre;
    }
    /// Starting a new experiment is explicit: restore the world and use the
    /// selected sector's authored shooter and target. Browsing never mutates it.
    pub fn restart_here(&mut self, config: ExplosionConfig) -> Result<(), String> {
        let Some(zone) = self.sector() else {
            return Ok(());
        };
        let mut next = self.layout.clone().unwrap();
        next.gun = zone.spawn;
        next.target = zone.target;
        self.commit(next, config)?;
        self.selected = None;
        self.pan = Vec2::ZERO;
        self.zoom = 1.0;
        Ok(())
    }
    /// Atomic edit: validate the real body occupancy before touching history.
    pub(super) fn commit(
        &mut self,
        next: RangeLayout,
        config: ExplosionConfig,
    ) -> Result<(), String> {
        self.validator.validate(&next).map_err(|e| e.to_string())?;
        self.preview = None;
        if let Some(old) = self.layout.replace(next) {
            if self.history.len() == 32 {
                self.history.remove(0);
            }
            self.history.push(old);
        }
        self.redo.clear();
        self.pending = Some((config, self.layout.as_ref().unwrap().clone()));
        Ok(())
    }
    fn undo(&mut self, config: ExplosionConfig, redo: bool) -> Result<(), String> {
        let stack = if redo {
            &mut self.redo
        } else {
            &mut self.history
        };
        let Some(next) = stack.last().cloned() else {
            return Ok(());
        };
        self.validator.validate(&next).map_err(|e| e.to_string())?;
        self.preview = None;
        stack.pop();
        if let Some(old) = self.layout.replace(next) {
            if redo {
                self.history.push(old);
            } else {
                self.redo.push(old);
            }
        }
        self.selected = None;
        self.pending = Some((config, self.layout.as_ref().unwrap().clone()));
        Ok(())
    }
    fn preview(&mut self, at: [i32; 3], radius: i32) {
        let key = PreviewKey {
            at,
            tool: self.tool,
            selected: self.selected,
            rotate: self.rotate,
            radius,
            allied: self.allied,
        };
        if self.preview.as_ref().is_some_and(|p| p.key == key) {
            return;
        }
        let Some(layout) = &self.layout else { return };
        let mut next = layout.clone();
        let selected = match self.tool {
            Tool::Select => {
                let Some(s) = self.selected else {
                    self.preview = None;
                    return;
                };
                relocate(&mut next, s, at);
                s
            }
            Tool::Tank
            | Tool::Strider
            | Tool::Spider
            | Tool::Artillery
            | Tool::TurretT1
            | Tool::TurretT2
            | Tool::Base => {
                next.units.push(RangeUnitDef {
                    allied: self.allied,
                    kind: if self.tool == Tool::Tank {
                        RangeUnitKind::Tank
                    } else if self.tool == Tool::Spider {
                        RangeUnitKind::Spider
                    } else if self.tool == Tool::Artillery {
                        RangeUnitKind::Artillery
                    } else if self.tool == Tool::TurretT1 {
                        RangeUnitKind::TurretT1
                    } else if self.tool == Tool::TurretT2 {
                        RangeUnitKind::TurretT2
                    } else if self.tool == Tool::Base {
                        RangeUnitKind::Base
                    } else {
                        RangeUnitKind::Strider
                    },
                    at,
                });
                Selection::Unit(next.units.len() - 1)
            }
            Tool::Shield => {
                next.shields.push(RangeShieldDef { at, radius });
                Selection::Shield(next.shields.len() - 1)
            }
            Tool::Aim => {
                self.preview = None;
                return;
            }
            tool => {
                next.blocks.push(block(tool, at, self.rotate));
                Selection::Block(next.blocks.len() - 1)
            }
        };
        let inside = |lo: [i32; 2], hi: [i32; 2]| {
            at[0] >= lo[0] && at[0] < hi[0] && at[2] >= lo[1] && at[2] < hi[1]
        };
        let issue = if !next.battle
            && !inside(next.clear_min, next.clear_max)
            && !next.course.as_ref().is_some_and(|c| inside(c.min, c.max))
            && !next.zones.iter().any(|z| inside(z.min, z.max))
        {
            Some("Выберите место на подготовленной площадке".into())
        } else {
            // Authored layers may intersect within their own group. Moving
            // that group or adding a wall must not swallow another object.
            let collides = if let Selection::Block(i) = selected {
                let moved = if self.tool == Tool::Select {
                    group(layout, i)
                } else {
                    vec![i]
                };
                moved.iter().any(|j| {
                    next.blocks
                        .iter()
                        .enumerate()
                        .any(|(k, b)| !moved.contains(&k) && overlap(&next.blocks[*j], b))
                })
            } else {
                false
            };
            if collides {
                Some("Место занято другим блоком".into())
            } else {
                self.validator.validate(&next).err().map(|e| {
                    use sim_core::blast::placement::Rejection;
                    match e {
                        Rejection::Layout(_) => "Превышен лимит объектов или границы расстановки",
                        Rejection::Outside => "Часть корпуса выходит за границу карты",
                        Rejection::Terrain => "Корпус пересекается с грунтом или блоком",
                        Rejection::Units => "Место занято юнитом",
                    }
                    .into()
                })
            }
        };
        self.preview = Some(Preview {
            key,
            next,
            selected,
            issue,
        });
    }
    pub fn placement_notice(&self) -> &str {
        if !self.editing || self.hover.is_none() {
            return "";
        }
        self.preview
            .as_ref()
            .map_or("", |p| p.issue.as_deref().unwrap_or("Можно разместить"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PreviewKey {
    at: [i32; 3],
    tool: Tool,
    selected: Option<Selection>,
    rotate: bool,
    radius: i32,
    allied: bool,
}
struct Preview {
    key: PreviewKey,
    next: RangeLayout,
    selected: Selection,
    issue: Option<String>,
}

fn snap(v: f32) -> i32 {
    (v / 4.0).round() as i32 * 4
}
fn block(tool: Tool, at: [i32; 3], rotate: bool) -> RangeBlock {
    if tool == Tool::Clear {
        static CLEARING: std::sync::OnceLock<modding::range::RangeEditor> =
            std::sync::OnceLock::new();
        let cfg = CLEARING.get_or_init(|| {
            modding::range::RangeEditor::from_ron(include_str!(
                "../../../../../data/range-editor.ron"
            ))
            .expect("range editor clearing")
        });
        let half = cfg.clearing_side / 2;
        let floor = if at[0] >= 0 && at[2] >= 0 {
            sim_core::blast::range_height(at[0] as u32, at[2] as u32) as i32
        } else {
            at[1]
        };
        return RangeBlock {
            min: [at[0] - half, floor, at[2] - half],
            max: [
                at[0] + cfg.clearing_side - half,
                floor + cfg.clearing_height,
                at[2] + cfg.clearing_side - half,
            ],
            material: 0,
        };
    }
    let (material, mut size) = match tool {
        Tool::Concrete => (2, [12, 44, 60]),
        Tool::Steel => (3, [16, 44, 60]),
        _ => (1, [28, 28, 60]),
    };
    if rotate {
        size.swap(0, 2);
    }
    let min = [at[0] - size[0] / 2, 0, at[2] - size[2] / 2];
    RangeBlock {
        min,
        max: std::array::from_fn(|a| min[a] + size[a]),
        material,
    }
}
fn overlap(a: &RangeBlock, b: &RangeBlock) -> bool {
    (0..3).all(|i| a.min[i] < b.max[i] && b.min[i] < a.max[i])
}
/// Connected overlapping layers move/delete together (the stock wall's jacket
/// and steel core). Separate, touching test walls remain independent.
fn group(layout: &RangeLayout, id: usize) -> Vec<usize> {
    let mut ids = vec![id];
    loop {
        let before = ids.len();
        for j in 0..layout.blocks.len() {
            if !ids.contains(&j)
                && ids
                    .iter()
                    .any(|i| overlap(&layout.blocks[*i], &layout.blocks[j]))
            {
                ids.push(j);
            }
        }
        if before == ids.len() {
            break;
        }
    }
    ids
}
fn relocate(layout: &mut RangeLayout, selected: Selection, at: [i32; 3]) {
    match selected {
        Selection::Unit(i) => layout.units[i].at = at,
        Selection::Shield(i) => layout.shields[i].at = at,
        Selection::Block(i) => {
            let b = &layout.blocks[i];
            let dx = at[0] - (b.min[0] + b.max[0]) / 2;
            let dz = at[2] - (b.min[2] + b.max[2]) / 2;
            let dy = if b.material == 0 && at[0] >= 0 && at[2] >= 0 {
                sim_core::blast::range_height(at[0] as u32, at[2] as u32) as i32 - b.min[1]
            } else {
                0
            };
            for j in group(layout, i) {
                let b = &mut layout.blocks[j];
                b.min[0] += dx;
                b.max[0] += dx;
                if b.material == 0 {
                    b.min[1] += dy;
                    b.max[1] += dy;
                }
                b.min[2] += dz;
                b.max[2] += dz;
            }
        }
    }
}
fn remove(layout: &mut RangeLayout, selected: Selection) {
    match selected {
        Selection::Unit(i) => {
            layout.units.remove(i);
            if let Some(c) = &mut layout.course {
                c.patrols.retain(|p| p.unit != i);
                for p in &mut c.patrols {
                    if p.unit > i {
                        p.unit -= 1;
                    }
                }
            }
        }
        Selection::Block(i) => {
            let mut ids = group(layout, i);
            ids.sort_unstable();
            for j in ids.into_iter().rev() {
                layout.blocks.remove(j);
            }
        }
        Selection::Shield(i) => {
            layout.shields.remove(i);
        }
    }
}

/// Presentation ray only. Half-cell stepping is bounded by the 640-cell map.
/// The authoritative shot uses its separate swept integer collision.
fn pick(sim: &BlastRange, ray: Ray3d) -> Option<([i32; 3], Option<Selection>)> {
    for step in 0..2600 {
        let v = ray.origin + *ray.direction * (step as f32 * 0.5);
        let p = v.floor().as_ivec3().to_array();
        if (0..3).any(|a| p[a] < 0 || p[a] >= sim.size()[a] as i32) {
            continue;
        }
        if let Some(i) = sim.units.iter().position(|u| u.solid(p)) {
            return Some((p, Some(Selection::Unit(i))));
        }
        if sim_core::blast::material(&sim.world, p) != 0 {
            let b = sim
                .layout
                .as_ref()
                .and_then(|l| {
                    l.blocks.iter().rposition(|b| {
                        b.contains(p)
                            || (b.material == 0
                                && p[0] >= b.min[0]
                                && p[0] < b.max[0]
                                && p[2] >= b.min[2]
                                && p[2] < b.max[2]
                                && p[1] + 1 == b.min[1])
                    })
                })
                .map(Selection::Block);
            // Open ground under a dome picks the dome.
            let dome = sim
                .shields
                .iter()
                .position(|s| {
                    let (dx, dz) = (p[0] - s.at[0], p[2] - s.at[2]);
                    dx * dx + dz * dz <= s.radius * s.radius
                })
                .map(Selection::Shield);
            return Some((p, b.or(dome)));
        }
    }
    None
}

pub fn issue_battle_group_order(
    sim: &mut BlastRange,
    ids: &[usize],
    order: sim_core::blast::battle::Order,
    destination: Option<[i32; 2]>,
) -> usize {
    use sim_core::blast::battle::Order;
    if order != Order::Move {
        return ids
            .iter()
            .filter(|&&id| sim.battle_order(id, order, destination))
            .count();
    }
    let Some(centre) = destination else { return 0 };
    let movers: Vec<_> = ids
        .iter()
        .copied()
        .filter(|&id| sim.units.get(id).is_some_and(|u| !u.kind.emplacement()))
        .collect();
    let columns = (1..=movers.len())
        .find(|n| n * n >= movers.len())
        .unwrap_or(1) as i32;
    let rows = (movers.len() as i32 + columns - 1) / columns;
    let size = sim.size();
    let destinations: Vec<_> = movers
        .into_iter()
        .enumerate()
        .map(|(slot, id)| {
            let (col, row) = (slot as i32 % columns, slot as i32 / columns);
            (
                id,
                [
                    (centre[0] + (2 * col - columns + 1) * 45).clamp(0, size[0] as i32 - 1),
                    (centre[1] + (2 * row - rows + 1) * 45).clamp(0, size[2] as i32 - 1),
                ],
            )
        })
        .collect();
    sim.battle_group_move(&destinations, centre)
}

fn battle_units_in_screen_rect(
    sim: &BlastRange,
    camera: &Camera,
    transform: &GlobalTransform,
    start: Vec2,
    end: Vec2,
) -> Vec<usize> {
    let lo = start.min(end);
    let hi = start.max(end);
    sim.units
        .iter()
        .enumerate()
        .filter(|(i, u)| sim.allied(*i) && u.status.alive)
        .filter_map(|(i, u)| {
            let at = Vec3::from(u.motion.pos.map(|v| v.to_f64_lossy() as f32));
            camera
                .world_to_viewport(transform, at)
                .ok()
                .and_then(|point| {
                    (point.x >= lo.x && point.x <= hi.x && point.y >= lo.y && point.y <= hi.y)
                        .then_some(i)
                })
        })
        .collect()
}

// Bevy injects each resource independently into this system.
#[allow(clippy::too_many_arguments)]
pub fn input(
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    panel: Option<Res<panel::Panel>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<GasCamera>>,
    overrides: Option<Res<knobs::Overrides>>,
    streaming: Option<Res<streaming::Streaming>>,
) {
    if edit.layout.is_none() {
        wheel.clear();
        return;
    }
    let stock = knobs::Overrides::default();
    let overrides = overrides.as_deref().unwrap_or(&stock);
    if keys.just_pressed(KeyCode::KeyE) {
        edit.editing = !edit.editing;
        scene.paused = true;
        scene.repeat = false;
        scene.view = 1;
        scene.notice.clear();
        if edit.editing {
            edit.selected = None;
            edit.pending = Some((
                scene.sim.config.clone(),
                edit.layout.as_ref().unwrap().clone(),
            ));
        }
    }
    if keys.just_pressed(KeyCode::Home) {
        edit.pan = Vec2::ZERO;
        edit.zoom = 1.0;
        scene.view = 1;
    }
    let blocked = panel.as_ref().is_some_and(|p| p.blocked);
    for ev in wheel.read() {
        if blocked {
            continue;
        }
        let y = ev.y
            * if ev.unit == MouseScrollUnit::Pixel {
                0.01
            } else {
                1.0
            };
        edit.zoom = (edit.zoom * (-y * 0.08).exp()).clamp(0.3, 32.0);
    }
    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, gt)) = cameras.single() else {
        return;
    };
    if let (Some(a), Some(b)) = (edit.previous_cursor, cursor) {
        if !blocked && mouse.pressed(MouseButton::Middle) {
            if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
                edit.orbit = (edit.orbit + (b.x - a.x) * 0.006).rem_euclid(std::f32::consts::TAU);
                edit.pitch = (edit.pitch + (a.y - b.y) * 0.006).clamp(-0.6, 0.6);
            } else if let (Ok(ra), Ok(rb)) = (
                camera.viewport_to_world(gt, a),
                camera.viewport_to_world(gt, b),
            ) {
                let d = ra.origin - rb.origin;
                edit.pan_by(Vec2::new(d.x, d.z));
            }
        }
    }
    edit.previous_cursor = cursor;
    // Camera controls remain responsive, but picking the old displayed map
    // during a staged replacement must not place/fire into another scene.
    if streaming.as_ref().is_some_and(|s| s.loading()) {
        edit.hover = None;
        edit.preview = None;
        edit.drag_start = None;
        edit.drag_current = None;
        return;
    }
    if scene.sim.battle_mode() {
        edit.selected_units.retain(|&id| {
            scene.sim.units.get(id).is_some_and(|u| u.status.alive) && scene.sim.allied(id)
        });
        if edit.controlled.is_some_and(|id| {
            !scene.sim.units.get(id).is_some_and(|u| u.status.alive) || !scene.sim.allied(id)
        }) {
            edit.controlled = edit.selected_units.first().copied();
            edit.selected = edit.controlled.map(Selection::Unit);
        }
    }
    if !edit.editing {
        edit.hover = None;
        let battle = scene.sim.battle_mode();
        let fire = panel.as_ref().is_some_and(|p| p.ground_fire);
        let additive = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        if battle && !fire {
            if mouse.just_pressed(MouseButton::Left) && !blocked {
                edit.drag_start = cursor;
                edit.drag_current = cursor;
            }
            if edit.drag_start.is_some() && mouse.pressed(MouseButton::Left) {
                edit.drag_current = cursor.or(edit.drag_current);
            }
            if mouse.just_released(MouseButton::Left) {
                let start = edit.drag_start.take();
                let end = cursor.or(edit.drag_current.take());
                if let (Some(start), Some(end)) = (start, end) {
                    if start.distance_squared(end) >= 64.0 {
                        let ids = battle_units_in_screen_rect(&scene.sim, camera, gt, start, end);
                        edit.select_battle_group(&scene.sim, &ids, additive);
                        return;
                    }
                    if !blocked {
                        if let Ok(ray) = camera.viewport_to_world(gt, end) {
                            match pick(&scene.sim, ray).and_then(|(_, s)| s) {
                                Some(Selection::Unit(i))
                                    if scene.sim.allied(i) && scene.sim.units[i].status.alive =>
                                {
                                    edit.select_battle_unit(&scene.sim, i, additive);
                                }
                                Some(enemy @ Selection::Unit(_)) => edit.selected = Some(enemy),
                                _ if !additive => edit.select_battle_group(&scene.sim, &[], false),
                                _ => {}
                            }
                        }
                    }
                }
            }
        } else {
            edit.drag_start = None;
            edit.drag_current = None;
        }
        if !blocked {
            if let Some(c) = cursor {
                if let Ok(ray) = camera.viewport_to_world(gt, c) {
                    if let Some((hit, selection)) = pick(&scene.sim, ray) {
                        if !battle && mouse.just_pressed(MouseButton::Left) {
                            edit.selected = selection;
                            if let Some(Selection::Unit(i)) = selection {
                                if scene.sim.allied(i) && scene.sim.units[i].status.alive {
                                    edit.controlled = Some(i);
                                }
                            }
                        }
                        if mouse.just_pressed(MouseButton::Right)
                            || (fire && mouse.just_pressed(MouseButton::Left))
                        {
                            if battle {
                                let ids = edit.battle_selected_ids(&scene.sim);
                                if !ids.is_empty() {
                                    use sim_core::blast::battle::Order;
                                    let order = if fire {
                                        Order::Ground(hit)
                                    } else if let Some(Selection::Unit(enemy)) = selection {
                                        if scene.sim.allied(enemy) {
                                            if !scene.sim.units[enemy].status.alive {
                                                return;
                                            }
                                            Order::Move
                                        } else {
                                            Order::Attack(enemy)
                                        }
                                    } else {
                                        Order::Move
                                    };
                                    let eligible = if order == Order::Move {
                                        ids.iter()
                                            .filter(|&&id| {
                                                scene
                                                    .sim
                                                    .units
                                                    .get(id)
                                                    .is_some_and(|u| !u.kind.emplacement())
                                            })
                                            .count()
                                    } else {
                                        ids.len()
                                    };
                                    let accepted = issue_battle_group_order(
                                        &mut scene.sim,
                                        &ids,
                                        order,
                                        Some([hit[0], hit[2]]),
                                    );
                                    if eligible == 0 && order == Order::Move {
                                        scene.notice = "В выделении нет подвижных юнитов".into();
                                    } else if accepted == eligible && accepted > 0 {
                                        scene.notice.clear();
                                    } else if accepted > 0 {
                                        scene.notice = format!(
                                            "Приказ получили {accepted} из {eligible} юнитов"
                                        );
                                    } else {
                                        scene.notice = "Приказ недоступен: проверьте фазу боя и состояние отряда".into();
                                    }
                                } else {
                                    scene.notice =
                                        "Сначала выделите своих юнитов левой кнопкой".into();
                                }
                                return;
                            }
                            scene.sim.control();
                            scene.paused = false;
                            if fire {
                                if !scene.sim.order_ground_fire(hit) {
                                    scene.notice = "Орудие ещё занято".into();
                                }
                            } else if let Some(Selection::Unit(id)) = selection {
                                scene.sim.order_attack(id);
                                edit.selected = selection;
                            } else if !scene.sim.order_move([hit[0], hit[2]]) {
                                scene.notice = "Нет проезда к этой точке".into();
                            }
                        }
                    }
                }
            }
        }
        return;
    }
    edit.drag_start = None;
    edit.drag_current = None;
    for (key, tool) in [
        (KeyCode::Digit1, Tool::Select),
        (KeyCode::Digit2, Tool::Tank),
        (KeyCode::Digit3, Tool::Strider),
        (KeyCode::Digit4, Tool::Concrete),
        (KeyCode::Digit5, Tool::Steel),
        (KeyCode::Digit6, Tool::Earth),
        (KeyCode::Digit7, Tool::Aim),
        (KeyCode::Digit8, Tool::Shield),
    ] {
        if keys.just_pressed(key) {
            edit.tool = tool;
            scene.notice.clear();
        }
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        edit.rotate = !edit.rotate;
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let cfg = knobs::effective_config(
        scene.energy,
        scene.anni,
        scene.power,
        scene.sim.config.delivery,
        overrides,
    );
    if ctrl && (keys.just_pressed(KeyCode::KeyZ) || keys.just_pressed(KeyCode::KeyY)) {
        let redo = keys.just_pressed(KeyCode::KeyY);
        if let Err(e) = edit.undo(cfg, redo) {
            scene.notice = e;
        }
        return;
    }
    if keys.just_pressed(KeyCode::F6) {
        let result = edit.layout.as_ref().unwrap().to_ron().and_then(|text| {
            std::fs::create_dir_all("out")
                .and_then(|_| std::fs::write("out/range-layout.ron", text))
                .map_err(|e| e.to_string())
        });
        scene.notice = match result {
            Ok(()) => {
                scene.layout_file = Some("out/range-layout.ron".into());
                "Saved: out/range-layout.ron".into()
            }
            Err(e) => format!("Save failed: {e}"),
        };
    }
    let mut next = edit.layout.as_ref().unwrap().clone();
    if keys.just_pressed(KeyCode::Delete) {
        if let Some(s) = edit.selected {
            remove(&mut next, s);
            edit.selected = None;
        } else {
            return;
        }
    } else {
        if blocked {
            edit.hover = None;
            return;
        }
        let Some(c) = cursor else {
            edit.hover = None;
            return;
        };
        let Ok(ray) = camera.viewport_to_world(gt, c) else {
            return;
        };
        // Placement uses the existing flat test pad, not the top of a wall.
        let t = (sim_core::blast::FLOOR as f32 - ray.origin.y) / ray.direction.y;
        let p = ray.origin + *ray.direction * t;
        let point = pick(&scene.sim, ray).map_or(p, |(hit, _)| Vec3::from(hit.map(|v| v as f32)));
        let x = snap(point.x);
        let z = snap(point.z);
        let at = [x, sim_core::blast::motion::floor(&scene.sim.world, x, z), z];
        edit.hover = Some(at);
        let left = mouse.just_pressed(MouseButton::Left);
        let right = mouse.just_pressed(MouseButton::Right);
        if right && edit.tool != Tool::Select {
            edit.tool = Tool::Select;
            edit.preview = None;
            return;
        }
        if left && edit.tool == Tool::Select {
            edit.selected = pick(&scene.sim, ray).and_then(|(_, s)| s);
            edit.preview = None;
            return;
        }
        edit.preview(at, scene.sim.shield_tuning().default_radius);
        if !left && !right {
            return;
        }
        if left && edit.tool == Tool::Aim {
            let Some((hit, _)) = pick(&scene.sim, ray) else {
                return;
            };
            next.target = hit;
            next.airburst_on_target = false;
        } else if let Some(preview) = &edit.preview {
            if preview.issue.is_some() {
                return;
            }
            // The exact checked proposal under the cursor is committed.
            next = preview.next.clone();
        } else {
            return;
        }
    }

    // Keep additions in the prepared pad: outside terrain is intentionally
    // forested and uneven. Real unit-volume validation catches every overlap.
    let result = edit.commit(next, cfg);
    scene.notice = match result {
        Ok(()) => String::new(),
        Err(e) => format!("Placement rejected: {e}"),
    };
}

/// Prefer a same-sector route; otherwise use its clear perimeter rather than
/// copying a route from the original firing pad on the other side of the map.
pub fn sector_patrol(layout: &RangeLayout, id: usize) -> Option<Vec<[i32; 2]>> {
    let unit = layout.units.get(id)?;
    let zone = layout
        .zones
        .iter()
        .find(|z| z.contains([unit.at[0], unit.at[2]], 0))?;
    if let Some(p) = layout.course.as_ref()?.patrols.iter().find(|p| {
        layout.units[p.unit].kind == unit.kind && p.points.iter().all(|p| zone.contains(*p, 32))
    }) {
        return Some(p.points.clone());
    }
    // Leave room for the stock body and its turn at the prepared boundary.
    let lo = zone.min.map(|v| v + 48);
    let hi = zone.max.map(|v| v - 48);
    Some(vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]])
}

/// A dashed line laid along the ground between two ground points, every
/// other segment drawn: a route as the hull will drive it (P-25).
fn ground_dash(
    giz: &mut Gizmos,
    sim: &BlastRange,
    a: [i32; 2],
    b: [i32; 2],
    lift: f32,
    colour: Color,
) {
    let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) / 6).max(1);
    let point = |t: i32| {
        let x = a[0] + (b[0] - a[0]) * t / n;
        let z = a[1] + (b[1] - a[1]) * t / n;
        Vec3::new(
            x as f32,
            sim_core::blast::motion::floor(&sim.world, x, z) as f32 + lift,
            z as f32,
        )
    };
    for k in (0..n).step_by(2) {
        giz.line(point(k), point(k + 1), colour);
    }
}
/// The range marker follows dug ground but does not climb tree crowns or
/// walls. It is a distance guide; solid cover still blocks a shot.
fn ground_range(giz: &mut Gizmos, sim: &BlastRange, at: [i32; 2], radius: i32, colour: Color) {
    let mut previous = None;
    for i in 0..=64 {
        let angle = i as f32 * std::f32::consts::TAU / 64.0;
        let x = (at[0] as f32 + angle.cos() * radius as f32).round() as i32;
        let z = (at[1] as f32 + angle.sin() * radius as f32).round() as i32;
        let next = if x >= 0 && z >= 0 && x < sim.size()[0] as i32 && z < sim.size()[2] as i32 {
            let natural = sim_core::blast::range_height(x as u32, z as u32) as i32;
            let floor = sim_core::blast::motion::floor(&sim.world, x, z).min(natural);
            Some(Vec3::new(x as f32, floor as f32 + 0.7, z as f32))
        } else {
            None
        };
        if let (Some(a), Some(b)) = (previous, next) {
            giz.line(a, b, colour);
        }
        previous = next;
    }
}
fn bounds(giz: &mut Gizmos, min: Vec3, max: Vec3, color: Color) {
    giz.cube(
        Transform::from_translation((min + max) * 0.5).with_scale(max - min),
        color,
    );
}
fn shield_field(giz: &mut Gizmos, centre: Vec3, radius: f32, charge: f32, tick: u32, eye: Vec3) {
    let facing = (eye - centre).normalize_or_zero();
    let phase = tick as f32 * 0.055;
    for (band, (u, v)) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)]
        .into_iter()
        .enumerate()
    {
        let mut previous = None;
        for i in 0..=72 {
            let angle = i as f32 * std::f32::consts::TAU / 72.0;
            let ripple = 1.0
                + 0.012
                    * (angle * 7.0 + phase + band as f32).sin()
                    * (angle * 11.0 - phase * 0.7).sin();
            let point = centre + (u * angle.cos() + v * angle.sin()) * radius * ripple;
            if let Some(prev) = previous {
                if (i + band * 13) % 19 != 0 {
                    let front = ((point - centre).dot(facing) * 0.5 / radius + 0.5).clamp(0.0, 1.0);
                    let flow = 0.75 + 0.25 * (angle * 5.0 + phase + band as f32).sin();
                    let alpha = (0.08 + front * (0.18 + charge * 0.35)) * flow;
                    giz.line(prev, point, Color::srgba(0.32, 0.73, 1.0, alpha));
                }
            }
            previous = Some(point);
        }
    }
}
pub fn overlay(
    mut giz: Gizmos,
    scene: Res<Scene>,
    edit: Res<Workshop>,
    art: Res<WitnessArt>,
    panel: Res<panel::Panel>,
    mut flashes: ResMut<ShieldFlashes>,
    camera: Query<&Transform, With<EditorCamera>>,
) {
    let flat = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    if scene.sim.battle_mode() {
        for (i, u) in scene.sim.units.iter().enumerate() {
            if !u.status.alive {
                continue;
            }
            let p = Vec3::from(u.motion.pos.map(|v| v.to_f64_lossy() as f32));
            let color = if scene.sim.allied(i) {
                Color::srgb(0.15, 0.8, 1.0)
            } else {
                Color::srgb(1.0, 0.3, 0.16)
            };
            let radius = (u.body.vol.nx.max(u.body.vol.nz) as f32 * u.body.mm as f32
                / voxel::unit::WORLD_MM as f32
                * 0.6)
                .max(24.0);
            giz.circle(Isometry3d::new(p + Vec3::Y * 0.8, flat), radius, color);
            if panel.ranges {
                let reach = scene.sim.battle_attack_range(i).unwrap_or(0);
                ground_range(
                    &mut giz,
                    &scene.sim,
                    [u.at[0], u.at[2]],
                    reach,
                    if scene.sim.allied(i) {
                        Color::srgba(0.48, 0.9, 1.0, 0.38)
                    } else {
                        Color::srgba(1.0, 0.48, 0.25, 0.38)
                    },
                );
            }
            if edit.selected_units.contains(&i)
                || (edit.selected_units.is_empty() && edit.controlled == Some(i))
            {
                giz.circle(
                    Isometry3d::new(p + Vec3::Y, flat),
                    radius + 3.0,
                    Color::WHITE,
                );
            }
            if edit.controlled == Some(i) {
                let mut from = [u.at[0], u.at[2]];
                for &to in u.motion.path.iter().skip(u.motion.cursor) {
                    ground_dash(&mut giz, &scene.sim, from, to, 0.8, color);
                    from = to;
                }
            }
        }
    } else if scene.sim.gun.status.alive && panel.ranges {
        // Stream rounds have a hard flight limit. Show that same limit in
        // the solo range; ballistic shells have no single ground radius.
        let reach = match scene.sim.config.delivery {
            Delivery::Flame => Some(scene.sim.streams.tuning.flame.range),
            Delivery::MachineGun => Some(scene.sim.streams.tuning.machine_gun.range),
            Delivery::Laser => Some(scene.sim.streams.tuning.laser.range),
            Delivery::Kinetic => None,
            Delivery::Blast => None,
        };
        if let Some(reach) = reach {
            ground_range(
                &mut giz,
                &scene.sim,
                [scene.sim.gun.at[0], scene.sim.gun.at[2]],
                reach,
                Color::srgba(0.48, 0.9, 1.0, 0.38),
            );
        }
    }
    // The charge keeps the idle field subdued; contacts travel along the
    // actual dome surface instead of appearing as a free-floating flash.
    let capacity = scene.sim.shield_tuning().capacity.max(1) as f32;
    let eye = camera
        .single()
        .map_or(Vec3::ZERO, |camera| camera.translation);
    for s in &scene.sim.shields {
        let c = Vec3::from(s.at.map(|v| v as f32));
        let (colour, radius) = if s.down > 0 {
            (Color::srgba(0.9, 0.35, 0.25, 0.4), s.radius as f32 * 0.3)
        } else {
            let k = s.charge as f32 / capacity;
            (
                Color::srgba(0.35 + 0.5 * (1.0 - k), 0.8, 1.0, 0.2 + 0.6 * k),
                s.radius as f32,
            )
        };
        if s.down > 0 {
            giz.sphere(Isometry3d::from_translation(c), radius, colour)
                .resolution(24);
        } else {
            shield_field(
                &mut giz,
                c,
                radius,
                s.charge as f32 / capacity,
                scene.sim.tick,
                eye,
            );
        }
        giz.circle(Isometry3d::new(c + Vec3::Y * 0.5, flat), radius, colour);
    }
    if flashes.seen != scene.sim.tick {
        if scene.sim.tick < flashes.seen {
            flashes.live.clear();
        }
        flashes.seen = scene.sim.tick;
        for h in &scene.sim.shield_hits {
            if let Some(shield) = scene.sim.shields.get(h.shield) {
                flashes.live.push(ShieldFlash {
                    at: Vec3::from(h.at.map(|v| v as f32)),
                    centre: Vec3::from(shield.at.map(|v| v as f32)),
                    tick: scene.sim.tick,
                    collapsed: h.collapsed,
                });
            }
        }
    }
    for flash in &flashes.live {
        let age = scene.sim.tick.saturating_sub(flash.tick);
        let life = if flash.collapsed { 18 } else { 12 };
        if age >= life {
            continue;
        }
        let t = 1.0 - age as f32 / life as f32;
        let normal = (flash.at - flash.centre).normalize_or(Vec3::Y);
        let orientation = Quat::from_rotation_arc(Vec3::Z, normal);
        let glow = if flash.collapsed {
            Color::srgba(1.0, 0.47, 0.23, 0.85 * t)
        } else {
            Color::srgba(0.53, 0.88, 1.0, 0.88 * t)
        };
        giz.sphere(
            Isometry3d::from_translation(flash.at),
            2.0 + 2.5 * (1.0 - t),
            Color::srgba(1.0, 0.96, 0.8, t),
        );
        let wave = 4.0 + (1.0 - t) * 34.0;
        giz.circle(Isometry3d::new(flash.at, orientation), wave, glow);
        giz.circle(
            Isometry3d::new(flash.at, orientation),
            wave * 0.68,
            Color::srgba(0.55, 0.88, 1.0, 0.35 * t),
        );
    }
    flashes.live.retain(|flash| {
        scene.sim.tick.saturating_sub(flash.tick) < if flash.collapsed { 18 } else { 12 }
    });
    if !edit.editing && !scene.sim.battle_mode() {
        let p = Vec3::from(scene.sim.gun.motion.pos.map(|v| v.to_f64_lossy() as f32));
        giz.circle(
            Isometry3d::new(p + Vec3::Y * 0.5, flat),
            23.0,
            Color::srgb(0.22, 0.85, 0.68),
        );
        // The way the hull will go (P-25): from where it stands through
        // the turn points it has left, a ring at each and at the goal.
        let m = &scene.sim.gun.motion;
        let teal = Color::srgba(0.22, 0.85, 0.68, 0.8);
        let mut from = [m.pos[0].to_int_trunc(), m.pos[2].to_int_trunc()];
        for goal in m.path.iter().skip(m.cursor) {
            ground_dash(&mut giz, &scene.sim, from, *goal, 0.6, teal);
            from = *goal;
        }
        for (i, goal) in m.path.iter().enumerate().skip(m.cursor) {
            let p = Vec3::new(
                goal[0] as f32,
                sim_core::blast::motion::floor(&scene.sim.world, goal[0], goal[1]) as f32 + 0.7,
                goal[1] as f32,
            );
            let last = i + 1 == m.path.len();
            giz.circle(
                Isometry3d::new(p, flat),
                if last { 5.0 } else { 2.0 },
                if last {
                    Color::srgb(0.22, 0.85, 0.68)
                } else {
                    teal
                },
            );
        }
        if let Some(i) = scene.sim.attack {
            if let Some(u) = scene.sim.units.get(i) {
                let p = Vec3::from(u.at.map(|v| v as f32));
                giz.circle(
                    Isometry3d::new(p + Vec3::Y, flat),
                    24.0,
                    Color::srgb(1.0, 0.4, 0.22),
                );
            }
        }
    }
    if panel.routes {
        if let Some(c) = scene.sim.layout.as_ref().and_then(|l| l.course.as_ref()) {
            for (j, patrol) in c.patrols.iter().enumerate() {
                for i in 0..patrol.points.len() {
                    let a = patrol.points[i];
                    let b = patrol.points[(i + 1) % patrol.points.len()];
                    ground_dash(
                        &mut giz,
                        &scene.sim,
                        a,
                        b,
                        0.5,
                        if j == 0 {
                            Color::srgba(0.9, 0.68, 0.3, 0.65)
                        } else {
                            Color::srgba(0.36, 0.75, 0.9, 0.65)
                        },
                    );
                }
            }
        }
    }
    if !edit.editing {
        return;
    }

    let Some(layout) = &edit.layout else { return };
    let cyan = Color::srgb(0.3, 0.85, 0.9);
    let (lo, hi) = edit
        .sector()
        .map_or((layout.clear_min, layout.clear_max), |z| (z.min, z.max));
    let corners = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        giz.line(
            Vec3::new(a[0] as f32, 16.1, a[1] as f32),
            Vec3::new(b[0] as f32, 16.1, b[1] as f32),
            Color::srgba(0.3, 0.65, 0.7, 0.35),
        );
    }
    let flat = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    let at = Vec3::from(layout.target.map(|v| v as f32));
    giz.circle(
        Isometry3d::new(at + Vec3::Y * 0.7, flat),
        5.0,
        Color::srgb(1.0, 0.63, 0.25),
    );
    giz.line(
        at - Vec3::X * 8.0,
        at + Vec3::X * 8.0,
        Color::srgb(1.0, 0.63, 0.25),
    );
    if let Some(s) = edit.selected {
        match s {
            Selection::Unit(i) => {
                if let Some(u) = scene.sim.units.get(i) {
                    let lo = Vec3::from(u.world_pos([0; 3]).map(|v| v as f32));
                    bounds(
                        &mut giz,
                        lo,
                        lo + Vec3::new(
                            u.body.vol.nx as f32,
                            u.body.vol.ny as f32,
                            u.body.vol.nz as f32,
                        ),
                        cyan,
                    );
                }
            }
            Selection::Block(i) => {
                if i < layout.blocks.len() {
                    for j in group(layout, i) {
                        let b = &layout.blocks[j];
                        bounds(
                            &mut giz,
                            Vec3::from(b.min.map(|v| v as f32)),
                            Vec3::from(b.max.map(|v| v as f32)),
                            cyan,
                        );
                    }
                }
            }
            Selection::Shield(i) => {
                if let Some(s) = layout.shields.get(i) {
                    giz.sphere(
                        Isometry3d::from_translation(Vec3::from(s.at.map(|v| v as f32))),
                        s.radius as f32,
                        cyan,
                    );
                }
            }
        }
    }
    if edit.hover.is_some() {
        if let Some(preview) = &edit.preview {
            let c = if preview.issue.is_none() {
                Color::srgb(0.22, 1.0, 0.42)
            } else {
                Color::srgb(1.0, 0.18, 0.12)
            };
            match preview.selected {
                Selection::Block(i) => {
                    let ids = if edit.tool == Tool::Select {
                        group(layout, i)
                    } else {
                        vec![i]
                    };
                    for j in ids {
                        let b = &preview.next.blocks[j];
                        bounds(
                            &mut giz,
                            Vec3::from(b.min.map(|v| v as f32)),
                            Vec3::from(b.max.map(|v| v as f32)),
                            c,
                        );
                    }
                }
                Selection::Unit(i) => {
                    let u = &preview.next.units[i];
                    let body = &art.pristine[u.kind.index()];
                    let size =
                        Vec3::new(body.vol.nx as f32, body.vol.ny as f32, body.vol.nz as f32);
                    let lo = Vec3::from(u.at.map(|v| v as f32))
                        - Vec3::new(size.x * 0.5, 0.0, size.z * 0.5);
                    bounds(&mut giz, lo, lo + size, c);
                }
                Selection::Shield(i) => {
                    let s = &preview.next.shields[i];
                    giz.sphere(
                        Isometry3d::from_translation(Vec3::from(s.at.map(|v| v as f32))),
                        s.radius as f32,
                        c,
                    );
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn battle_preset_preserves_map_and_editor_sides_roundtrip() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
        let blocks = layout.blocks.clone();
        let mut edit = Workshop::new(Some(layout), false);
        edit.battle_preset(config(false, false)).unwrap();
        let layout = edit.layout.as_ref().unwrap();
        assert_eq!(layout.blocks, blocks);
        assert_eq!(layout.size, [10000, 192, 10000]);
        assert_eq!(layout.units.iter().filter(|u| u.allied).count(), 2);
        assert_eq!(layout.units.iter().filter(|u| !u.allied).count(), 2);
        assert_eq!(
            RangeLayout::from_ron(&layout.to_ron().unwrap()).unwrap(),
            *layout
        );
        let (pending_config, layout) = edit.pending.take().unwrap();
        let mut sim = BlastRange::with_layout(pending_config, layout).unwrap();
        assert_eq!(sim.battle.phase, sim_core::blast::battle::Phase::Setup);
        sim.start_battle().unwrap();
        edit.selected = Some(Selection::Unit(2));
        edit.side(true, config(false, false)).unwrap();
        assert!(edit.layout.as_ref().unwrap().units[2].allied);
        assert!(edit.pending.as_ref().unwrap().1.battle);
        edit.range_mode(config(false, false)).unwrap();
        assert!(!edit.layout.as_ref().unwrap().battle);
        assert!(!edit.pending.as_ref().unwrap().1.battle);
        assert_eq!(edit.layout.as_ref().unwrap().units.len(), 4);
        assert!(edit.controlled.is_none());
    }
    #[test]
    fn world_battle_places_both_sides_beyond_prepared_sectors() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
        let config = config(false, false);
        let far = [7800, 16, 4000];
        assert!(!layout.zones.iter().any(|z| z.contains([far[0], far[2]], 0)));
        let blocks = layout.blocks.clone();
        let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
            "../../../../../data/battle-world.ron"
        ))
        .unwrap();
        let mut probe = layout.clone();
        probe.battle = true;
        probe.units.clear();
        probe.blocks.extend(preset.blocks.clone());
        if let Some(course) = &mut probe.course {
            course.patrols.clear();
        }
        probe.units = preset.units.clone();
        sim_core::blast::placement::Validator::default()
            .validate(&probe)
            .expect("whole-map battle placement");
        for allied in [true, false] {
            let side: Vec<_> = preset.units.iter().filter(|u| u.allied == allied).collect();
            assert_eq!(side.len(), 30);
            for kind in [
                RangeUnitKind::Base,
                RangeUnitKind::Tank,
                RangeUnitKind::Strider,
                RangeUnitKind::Spider,
                RangeUnitKind::Artillery,
                RangeUnitKind::TurretT1,
                RangeUnitKind::TurretT2,
            ] {
                assert!(side.iter().any(|u| u.kind == kind), "missing {kind:?}");
            }
        }
        let mut edit = Workshop::new(Some(layout), false);
        let briefing = edit.battle_world(config.clone()).unwrap();
        assert!(briefing.contains("баз"));
        assert!(edit.editing);
        assert!(edit.layout.as_ref().unwrap().battle);
        assert!(edit.sector().unwrap().contains([4850, 5580], 0));
        assert_eq!(edit.layout.as_ref().unwrap().units.len(), 60);
        assert_eq!(edit.controlled, Some(1));
        assert_eq!(
            edit.layout
                .as_ref()
                .unwrap()
                .units
                .iter()
                .filter(|u| u.kind == RangeUnitKind::Base)
                .count(),
            2
        );
        assert!(edit.layout.as_ref().unwrap().blocks.starts_with(&blocks));
        assert_eq!(
            edit.layout.as_ref().unwrap().blocks.len(),
            blocks.len() + preset.blocks.len()
        );
        edit.tool = Tool::Steel;
        edit.preview(far, 60);
        let preview = edit.preview.as_ref().unwrap();
        assert_eq!(preview.issue, None, "{:?}", preview.issue);
        edit.commit(preview.next.clone(), config.clone()).unwrap();
        let remote_unit = [7860, sim_core::blast::range_height(7860, 4000) as i32, 4000];
        edit.tool = Tool::Clear;
        edit.preview(remote_unit, 60);
        let preview = edit.preview.as_ref().unwrap();
        assert_eq!(preview.issue, None, "{:?}", preview.issue);
        edit.commit(preview.next.clone(), config.clone()).unwrap();
        edit.tool = Tool::Tank;
        edit.preview(remote_unit, 60);
        let preview = edit.preview.as_ref().unwrap();
        assert_eq!(preview.issue, None, "{:?}", preview.issue);
        edit.commit(edit.preview.as_ref().unwrap().next.clone(), config.clone())
            .unwrap();
        assert!(!edit
            .layout
            .as_ref()
            .unwrap()
            .zones
            .iter()
            .any(|z| z.contains([remote_unit[0], remote_unit[2]], 0)));
        for (allied, at) in [(true, [128, 16, 336]), (false, [1480, 16, 460])] {
            edit.allied = allied;
            edit.preview(at, 60);
            let preview = edit.preview.as_ref().unwrap();
            assert_eq!(preview.issue, None, "{:?}", preview.issue);
            edit.commit(preview.next.clone(), config.clone()).unwrap();
        }
        let (_, placed) = edit.pending.take().unwrap();
        assert_eq!(placed.units.len(), 63);
        assert!(placed.units[60].allied && placed.units[61].allied && !placed.units[62].allied);
        assert_eq!(
            RangeLayout::from_ron(&placed.to_ron().unwrap()).unwrap(),
            placed
        );
        // Keep this long movement check at its original 15-unit size. The
        // full 60-unit build and combat start are exercised by the HUD test.
        let mut sim_layout = placed;
        sim_layout.units = sim_layout
            .units
            .into_iter()
            .enumerate()
            .filter_map(|(i, u)| (i < 12 || i >= 60).then_some(u))
            .collect();
        let mut sim = BlastRange::with_layout(config, sim_layout).unwrap();
        let ground = remote_unit[1] as u32;
        assert!(sim
            .world
            .solid(remote_unit[0] as u32, ground - 1, remote_unit[2] as u32));
        assert!(!sim
            .world
            .solid(remote_unit[0] as u32, ground, remote_unit[2] as u32));
        sim.start_battle().unwrap();
        assert_eq!(sim.battle.phase, sim_core::blast::battle::Phase::Fighting);
        sim.step();
        assert_eq!(sim.battle_base_integrity(), Some([100, 100]));
        assert!(sim.battle.actors[12].target.is_some());
        assert!(sim.battle.actors[14].target.is_some());
        assert!(sim.battle_order(4, sim_core::blast::battle::Order::Move, Some([5050, 5800])));
        let front = sim.units[4].at;
        for _ in 0..180 {
            sim.step();
        }
        assert!(sim.units[2].at[1] < 30, "artillery climbed onto a roof");
        assert!(
            sim.units[4].at[0] > front[0] + 40 || sim.units[4].at[2] > front[2] + 40,
            "front tank stalled on the road: {:?} -> {:?}",
            front,
            sim.units[4].at
        );
        edit.range_mode(sim.config.clone()).unwrap();
        assert_eq!(edit.layout.as_ref().unwrap().blocks, blocks);
    }
    #[test]
    fn preview_and_commit_agree_on_terrain_and_unit_collisions() {
        let (mut w, c) = fixture();
        let before = w.layout.clone();
        w.tool = Tool::Strider;
        w.preview([448, 16, 384], 60);
        let p = w.preview.as_ref().unwrap();
        assert_eq!(p.issue, None);
        let ptr = p.next.blocks.as_ptr();
        w.preview([448, 16, 384], 60);
        assert_eq!(
            w.preview.as_ref().unwrap().next.blocks.as_ptr(),
            ptr,
            "unchanged hover rebuilt the proposal"
        );
        for at in [[400, 16, 336], [439, 16, 336]] {
            w.preview(at, 60);
            let p = w.preview.as_ref().unwrap();
            assert!(p.issue.is_some());
            assert!(w.commit(p.next.clone(), c.clone()).is_err());
            assert_eq!(w.layout, before);
            assert!(w.history.is_empty());
            assert!(w.pending.is_none());
        }
        w.preview([448, 16, 384], 60);
        w.commit(w.preview.as_ref().unwrap().next.clone(), c)
            .unwrap();
        assert!(
            w.preview.is_none(),
            "edit did not invalidate the hover cache"
        );
    }
    #[test]
    fn course_placement_rotation_and_layered_moves_have_checked_previews() {
        let (mut w, _) = fixture();
        w.tool = Tool::Concrete;
        w.preview([550, 16, 570], 60);
        let p = w.preview.as_ref().unwrap();
        assert_eq!(p.issue, None, "course outside the stock pad must be green");
        let old = p.next.blocks.last().unwrap().clone();
        w.rotate = true;
        w.preview([550, 16, 570], 60);
        assert_ne!(w.preview.as_ref().unwrap().next.blocks.last(), Some(&old));
        w.preview([400, 16, 336], 60);
        assert!(
            w.preview.as_ref().unwrap().issue.is_some(),
            "new block swallowed the wall"
        );
        w.tool = Tool::Select;
        w.selected = Some(Selection::Block(1));
        w.preview([400, 16, 340], 60);
        let p = w.preview.as_ref().unwrap();
        assert_eq!(p.issue, None);
        assert_eq!(p.next.blocks[0].min[2], 286);
        assert_eq!(p.next.blocks[1].min[2], 286);
        w.preview([100, 16, 100], 60);
        assert!(w.preview.as_ref().unwrap().issue.is_some());
    }
    #[test]
    fn object_limit_is_visible_before_clicking() {
        let (mut w, _) = fixture();
        let layout = w.layout.as_mut().unwrap();
        layout.shields = vec![
            RangeShieldDef {
                at: [448, 16, 384],
                radius: 60
            };
            8
        ];
        w.tool = Tool::Shield;
        w.preview([450, 16, 380], 60);
        assert!(w.preview.as_ref().unwrap().issue.is_some());
    }
    #[test]
    fn overview_keeps_the_shooter_when_aim_moves_beyond_the_wall() {
        use bevy::camera::{CameraProjection, RenderTargetInfo};
        let (w, c) = fixture();
        let mut layout = w.layout.unwrap();
        layout.target = [480, 24, 380];
        let sim = BlastRange::with_layout(c, layout).unwrap();
        let aim = Vec3::new(339.0, 34.0, 380.0);
        let offset = Vec3::new(-155.0, 250.0, 320.0);
        let (fitted, height) = overview_frame(&sim, aim, offset, 16.0 / 9.0);

        let half = Vec2::new(height * 16.0 / 9.0, height) * 0.5;
        let projection = OrthographicProjection {
            area: Rect::from_corners(-half, half),
            near: 1.0,
            far: 1200.0,
            ..OrthographicProjection::default_3d()
        };
        let mut camera = Camera::default();
        camera.computed.clip_from_view = projection.get_clip_from_view();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(1600, 900),
            scale_factor: 1.0,
        });
        let gt = GlobalTransform::from(
            Transform::from_translation(fitted + offset).looking_at(fitted, Vec3::Y),
        );
        for x in [83.0, 173.0] {
            for y in [14.0, 66.0] {
                for z in [306.0, 366.0] {
                    let p = camera.world_to_viewport(&gt, Vec3::new(x, y, z)).unwrap();
                    assert!(
                        p.x > 0.0 && p.x < 1600.0 && p.y > 0.0 && p.y < 900.0,
                        "cropped: {p}"
                    );
                }
            }
        }
    }
    #[test]
    fn browsing_sectors_preserves_the_experiment_and_restart_moves_the_shooter() {
        let (mut w, config) = fixture();
        let zone = modding::range::RangeZone {
            name: "Test sector".into(),
            min: [80, 260],
            max: [620, 625],
            spawn: [200, 16, 336],
            target: [392, 24, 336],
            scenario: None,
        };
        w.layout.as_mut().unwrap().zones = vec![zone.clone(), zone];
        let before = w.layout.clone();
        w.pan = Vec2::splat(50.0);
        w.change_zone(-1);
        assert_eq!(w.zone, 1);
        assert_eq!(w.layout, before);
        assert!(w.pending.is_none());
        assert_eq!(w.pan, Vec2::ZERO);
        w.pan_by(Vec2::splat(20000.0));
        assert!(w.pan.x <= 640.0 && w.pan.y <= 640.0);
        w.restart_here(config).unwrap();
        assert_eq!(w.pending.as_ref().unwrap().1.gun, [200, 16, 336]);
        assert_eq!(w.layout.as_ref().unwrap().gun, [200, 16, 336]);
        assert_eq!(w.history.len(), 1);
    }
    #[test]
    fn distant_sector_framing_is_local_and_contains_its_structures_when_rotated() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
        let zone = layout
            .zones
            .iter()
            .find(|z| z.min[0] > 8000 && z.min[1] > 8000)
            .unwrap();
        for orbit in [0.0, 1.2, 2.6, 4.8] {
            let offset = Quat::from_rotation_y(orbit) * Vec3::new(-155.0, 420.0, 320.0);
            let (aim, height) = sector_frame(zone, offset, 16.0 / 9.0);
            assert!(aim.x > 8000.0 && aim.z > 8000.0);
            assert!(height < 1300.0, "fit included distant units: {height}");
            let rotation = Transform::from_translation(offset)
                .looking_at(Vec3::ZERO, Vec3::Y)
                .rotation;
            for b in &layout.blocks {
                if !zone.contains([b.min[0], b.min[2]], 0) {
                    continue;
                }
                for p in [b.min, b.max] {
                    let delta = Vec3::from(p.map(|v| v as f32)) - aim;
                    assert!((rotation * Vec3::X).dot(delta).abs() < height * 16.0 / 9.0 * 0.5);
                    assert!((rotation * Vec3::Y).dot(delta).abs() < height * 0.5);
                }
            }
        }
    }
    #[test]
    fn a_remote_target_and_its_new_route_stay_in_the_selected_sector() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
        let mut w = Workshop::new(Some(layout.clone()), false);
        assert_eq!(w.target_unit(None, 1), Some(2));
        w.change_zone(4);
        assert_eq!(w.target_unit(None, 1), Some(4));
        let points = sector_patrol(&layout, 4).unwrap();
        assert!(points.iter().all(|p| layout.zones[4].contains(*p, 32)));
        w.change_zone(3);
        assert_eq!(
            w.target_unit(None, 1),
            None,
            "empty far sector must not jump to the old patrol"
        );
        assert_eq!(w.target_unit(Some(4), 1), Some(4), "explicit attack wins");
    }
    #[test]
    fn spider_skirmish_preserves_the_map_and_places_both_armies() {
        let (mut workshop, config) = fixture();
        let briefing = workshop.skirmish(2, config).unwrap();
        assert!(!briefing.is_empty());
        let layout = workshop.layout.as_ref().unwrap();
        assert!(layout.battle);
        assert_eq!(layout.units[0].kind, RangeUnitKind::Spider);
        assert!(layout.units[0].allied);
        assert_eq!(layout.units.len(), 5);
        assert!(layout.zones.len() > 5);
        assert_eq!(workshop.controlled, Some(0));
        assert!(workshop.pending.is_some());
    }

    #[test]
    fn turret_skirmish_loads_both_emplacements_and_mobile_artillery() {
        let (mut workshop, config) = fixture();
        let briefing = workshop.skirmish(3, config.clone()).unwrap();
        assert!(briefing.contains("T1") && briefing.contains("T2"));
        let layout = workshop.layout.as_ref().unwrap();
        assert!(layout.battle);
        assert_eq!(layout.units.len(), 6);
        assert!(layout
            .units
            .iter()
            .any(|u| u.kind == RangeUnitKind::Artillery && u.allied));
        assert!(layout
            .units
            .iter()
            .any(|u| u.kind == RangeUnitKind::TurretT1 && !u.allied));
        assert!(layout
            .units
            .iter()
            .any(|u| u.kind == RangeUnitKind::TurretT2 && !u.allied));
        let mut sim = BlastRange::with_layout(config, layout.clone()).unwrap();
        sim.start_battle().unwrap();
        for _ in 0..240 {
            sim.step();
        }
        assert!(sim.battle.actors[1].fired > 0, "artillery never fired");
        assert!(
            sim.battle.actors[4].fired > 0,
            "T2 energy cannon never fired"
        );
    }

    fn fixture() -> (Workshop, ExplosionConfig) {
        (
            Workshop::new(
                Some(
                    RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron"))
                        .unwrap(),
                ),
                true,
            ),
            scaled_config(true, false, 1000),
        )
    }
    #[test]
    fn battle_drag_rectangle_selects_only_visible_allies_inside_it() {
        use bevy::camera::{CameraProjection, RenderTargetInfo};
        let (workshop, config) = fixture();
        let mut layout = workshop.layout.unwrap();
        layout.battle = true;
        for (id, unit) in layout.units.iter_mut().enumerate() {
            unit.allied = id < 2;
        }
        let sim = BlastRange::with_layout(config, layout).unwrap();
        let projection = OrthographicProjection {
            area: Rect::from_corners(Vec2::new(-320.0, -240.0), Vec2::new(320.0, 240.0)),
            near: 1.0,
            far: 1200.0,
            ..OrthographicProjection::default_3d()
        };
        let mut camera = Camera::default();
        camera.computed.clip_from_view = projection.get_clip_from_view();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(1600, 900),
            scale_factor: 1.0,
        });
        let transform = GlobalTransform::from(
            Transform::from_xyz(400.0, 300.0, 400.0)
                .looking_at(Vec3::new(400.0, 0.0, 400.0), Vec3::NEG_Z),
        );
        let points: Vec<_> = sim.units[..2]
            .iter()
            .map(|u| {
                camera
                    .world_to_viewport(
                        &transform,
                        Vec3::from(u.motion.pos.map(|v| v.to_f64_lossy() as f32)),
                    )
                    .unwrap()
            })
            .collect();
        let start = points[0].min(points[1]) - Vec2::splat(8.0);
        let end = points[0].max(points[1]) + Vec2::splat(8.0);
        assert_eq!(
            battle_units_in_screen_rect(&sim, &camera, &transform, start, end),
            [0, 1]
        );
    }
    #[test]
    fn rejected_overlap_preserves_layout_history_and_pending() {
        let (mut w, c) = fixture();
        let before = w.layout.clone().unwrap();
        let mut next = before.clone();
        next.units[0].at = [400, 16, 336];
        assert!(w.commit(next, c).is_err());
        assert_eq!(w.layout.as_ref(), Some(&before));
        assert!(w.history.is_empty());
        assert!(w.pending.is_none());
    }
    #[test]
    fn moving_layered_cover_undo_redo_and_serializing_preserve_layers() {
        let (mut w, c) = fixture();
        let before = w.layout.clone().unwrap();
        let mut next = before.clone();
        relocate(&mut next, Selection::Block(0), [400, 16, 340]);
        assert_eq!(next.blocks[0].min[2], 286);
        assert_eq!(next.blocks[1].min[2], 286);
        w.commit(next.clone(), c.clone()).unwrap();
        w.undo(c.clone(), false).unwrap();
        assert_eq!(w.layout.as_ref(), Some(&before));
        w.undo(c, true).unwrap();
        assert_eq!(w.layout.as_ref(), Some(&next));
        assert_eq!(
            RangeLayout::from_ron(&next.to_ron().unwrap()).unwrap(),
            next
        );
        remove(&mut next, Selection::Block(1));
        assert!(next.blocks.is_empty());
    }
    #[test]
    fn bounded_placements_and_click_pick_use_actual_occupancy() {
        let (mut w, c) = fixture();
        let mut next = w.layout.clone().unwrap();
        next.units.push(RangeUnitDef {
            allied: false,
            kind: RangeUnitKind::Strider,
            at: [448, 16, 385],
        });
        w.commit(next, c).unwrap();
        let (config, layout) = w.pending.take().unwrap();
        let sim = BlastRange::with_layout(config, layout).unwrap();
        let hit = pick(
            &sim,
            Ray3d::new(Vec3::new(439.0, 180.0, 336.0), Dir3::NEG_Y),
        )
        .unwrap();
        assert_eq!(hit.1, Some(Selection::Unit(1)));
        let mut too_many = w.layout.clone().unwrap();
        while too_many.units.len() <= 8 {
            too_many.units.push(too_many.units[0].clone());
        }
        assert!(w
            .commit(too_many, scaled_config(true, false, 1000))
            .is_err());
    }

    #[test]
    fn mouse_and_keyboard_system_places_a_walker_and_undoes_without_a_window_backend() {
        use bevy::camera::{CameraProjection, RenderTargetInfo};
        let (workshop, c) = fixture();
        let original = workshop.layout.clone().unwrap();
        let sim = BlastRange::with_layout(c, original.clone()).unwrap();
        let scene = Scene {
            generation: 0,
            sim,
            energy: true,
            anni: false,
            paused: true,
            frame: 0,
            finish: 420,
            out: None,
            all_frames: false,
            wait: 0,
            captured: 0,
            acc: 0.0,
            repeat: false,
            view: 1,
            camera_sweep: false,
            patrol: 1,
            previous_shell: None,
            fps: 60.0,
            step_us: 0,
            flame: 0,
            realtime: true,
            live_elapsed: 0.0,
            last_capture: 0,
            layout_file: Some("data/range-cover.ron".into()),
            power: 1000,
            uncovered: false,
            unit_override: None,
            skip_delta: true,
            notice: String::new(),
        };
        let mut app = App::new();
        app.insert_resource(scene)
            .insert_resource(workshop)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<MouseWheel>()
            .add_systems(Update, input);
        let projection = OrthographicProjection {
            area: Rect::from_corners(Vec2::new(-320.0, -240.0), Vec2::new(320.0, 240.0)),
            near: 1.0,
            far: 1200.0,
            ..OrthographicProjection::default_3d()
        };
        let mut camera = Camera::default();
        camera.computed.clip_from_view = projection.get_clip_from_view();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(1600, 900),
            scale_factor: 1.0,
        });
        let gt = GlobalTransform::from(
            Transform::from_xyz(320.0, 300.0, 336.0)
                .looking_at(Vec3::new(320.0, 0.0, 336.0), Vec3::NEG_Z),
        );
        let screen = camera
            .world_to_viewport(&gt, Vec3::new(448.0, 16.0, 384.0))
            .unwrap();
        let mut window = Window {
            resolution: bevy::window::WindowResolution::new(1600, 900)
                .with_scale_factor_override(1.0),
            ..default()
        };
        window.set_cursor_position(Some(screen));
        app.world_mut().spawn(window);
        app.world_mut().spawn((camera, gt, GasCamera));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Digit3);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        let w = app.world().resource::<Workshop>();
        assert_eq!(
            w.layout.as_ref().unwrap().units.len(),
            original.units.len() + 1,
            "{}",
            app.world().resource::<Scene>().notice
        );
        assert_eq!(
            w.layout.as_ref().unwrap().units[original.units.len()].kind,
            RangeUnitKind::Strider
        );
        assert_eq!(
            w.layout.as_ref().unwrap().units[original.units.len()].at,
            [448, 16, 384]
        );
        assert!(w.pending.is_some());
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        *keys = default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::KeyZ);
        app.update();
        assert_eq!(
            app.world().resource::<Workshop>().layout.as_ref(),
            Some(&original)
        );
        // The same input system accepts RTS commands with no keyboard keys.
        app.insert_resource(panel::Panel::new(true, false))
            .insert_resource(GasInput::default());
        app.add_systems(
            Update,
            (
                panel::actions.before(input),
                panel::release_keys.after(input),
            ),
        );
        *app.world_mut().resource_mut::<ButtonInput<KeyCode>>() = default();
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        app.world_mut().resource_mut::<Workshop>().pending = None;
        app.world_mut()
            .spawn((panel::Action::Play, Interaction::Pressed));
        app.update();
        assert!(!app.world().resource::<Workshop>().editing);
        assert!(app.world().resource::<Scene>().sim.player);
        assert_eq!(app.world().resource::<Scene>().sim.fire_tick, u32::MAX);
        let screen = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<(&Camera, &GlobalTransform), With<GasCamera>>();
            let (c, g) = q.single(world).unwrap();
            c.world_to_viewport(g, Vec3::new(245.0, 16.0, 340.0))
                .unwrap()
        };
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world)
                .unwrap()
                .set_cursor_position(Some(screen));
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        {
            let mut scene = app.world_mut().resource_mut::<Scene>();
            for _ in 0..350 {
                scene.sim.step();
            }
            assert!(
                (scene.sim.gun.at[0] - 245).abs() < 8,
                "mouse move {:?}",
                scene.sim.gun.at
            );
        }
        let screen = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<(&Camera, &GlobalTransform), With<GasCamera>>();
            let (c, g) = q.single(world).unwrap();
            c.world_to_viewport(g, Vec3::new(359.0, 40.0, 312.0))
                .unwrap()
        };
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world)
                .unwrap()
                .set_cursor_position(Some(screen));
        }
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        assert_eq!(app.world().resource::<Scene>().sim.attack, Some(0));
        // A click on the panel executes its action and cannot issue a ground order.
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world)
                .unwrap()
                .set_cursor_position(Some(Vec2::new(40.0, 40.0)));
        }
        app.world_mut()
            .spawn((panel::Action::Stop, Interaction::Pressed));
        app.update();
        assert!(app.world().resource::<Scene>().sim.attack.is_none());
        // Auto attack is a mouse-only command and Stop cancels its mode.
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        let auto = app
            .world_mut()
            .spawn((panel::Action::AutoAttack, Interaction::Pressed))
            .id();
        app.update();
        assert!(app.world().resource::<Scene>().sim.auto_attack);
        app.world_mut().entity_mut(auto).insert(Interaction::None);
        app.world_mut()
            .spawn((panel::Action::Stop, Interaction::Pressed));
        app.update();
        assert!(!app.world().resource::<Scene>().sim.auto_attack);
        assert!(app
            .world()
            .resource::<Scene>()
            .sim
            .gun
            .motion
            .path
            .is_empty());
        app.world_mut()
            .spawn((panel::Action::Scenario(2), Interaction::Pressed));
        app.update();
        let edit = app.world().resource::<Workshop>();
        let (config, layout) = edit.pending.as_ref().unwrap_or_else(|| {
            panic!(
                "scenario did not rebuild: {}",
                app.world().resource::<Scene>().notice
            )
        });
        assert_eq!(edit.zone, 2);
        assert_eq!(layout.gun, layout.zones[2].spawn);
        assert_eq!(layout.target, layout.zones[2].target);
        assert_eq!(config.delivery, Delivery::Blast);
        assert!(layout.course.as_ref().unwrap().patrols.is_empty());
        assert_eq!(app.world().resource::<Scene>().power, 1000);
        assert!(!app.world().resource::<panel::Panel>().drive);
        assert!(app
            .world()
            .resource::<panel::Panel>()
            .scenario_run
            .is_some());
        app.world_mut()
            .spawn((panel::Action::Scenario(9), Interaction::Pressed));
        app.update();
        let edit = app.world().resource::<Workshop>();
        let (config, layout) = edit
            .pending
            .as_ref()
            .expect("forest scenario did not rebuild");
        assert_eq!(config.delivery, Delivery::Kinetic);
        let patrols = &layout.course.as_ref().unwrap().patrols;
        assert_eq!(patrols.len(), 1);
        assert_eq!(patrols[0].unit, 7);

        // A real pointer drag in battle selects two allies, never enemies.
        let mut battle_layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        battle_layout.battle = true;
        for (id, unit) in battle_layout.units.iter_mut().enumerate() {
            unit.allied = id < 2;
        }
        let cfg = app.world().resource::<Scene>().sim.config.clone();
        let mut battle = BlastRange::with_layout(cfg, battle_layout.clone()).unwrap();
        battle.start_battle().unwrap();
        app.world_mut().resource_mut::<Scene>().sim = battle;
        {
            let mut edit = app.world_mut().resource_mut::<Workshop>();
            edit.layout = Some(battle_layout);
            edit.editing = false;
            edit.controlled = None;
            edit.selected = None;
            edit.selected_units.clear();
        }
        let (start, end) = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<(&Camera, &GlobalTransform), With<GasCamera>>();
            let (camera, transform) = q.single(world).unwrap();
            let a = camera
                .world_to_viewport(transform, Vec3::new(359.0, 16.0, 312.0))
                .unwrap();
            let b = camera
                .world_to_viewport(transform, Vec3::new(439.0, 16.0, 336.0))
                .unwrap();
            (a.min(b) - Vec2::splat(8.0), a.max(b) + Vec2::splat(8.0))
        };
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world)
                .unwrap()
                .set_cursor_position(Some(start));
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert!(app.world().resource::<Workshop>().drag_start.is_some());
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world).unwrap().set_cursor_position(Some(end));
        }
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.update();
        assert_eq!(app.world().resource::<Workshop>().selected_units, [0, 1]);
        let enemy = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<(&Camera, &GlobalTransform), With<GasCamera>>();
            let (camera, transform) = q.single(world).unwrap();
            camera
                .world_to_viewport(transform, Vec3::new(300.0, 20.0, 445.0))
                .unwrap()
        };
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world)
                .unwrap()
                .set_cursor_position(Some(enemy));
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        assert!(app.world().resource::<Scene>().sim.battle.actors[..2]
            .iter()
            .all(|actor| actor.order == sim_core::blast::battle::Order::Attack(2)));
    }
}
