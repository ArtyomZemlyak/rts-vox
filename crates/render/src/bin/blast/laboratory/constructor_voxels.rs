//! Scoped authored-cell editing. Each mouse stroke is one validated transaction.
use super::*;
use catalog::voxels::{self, Tool};
use std::collections::BTreeSet;
pub struct Session {
    index: usize,
    original: Placed,
    pub tool: Tool,
    undo: Vec<Placed>,
    redo: Vec<Placed>,
    stroke: Option<Stroke>,
    queued: bool,
}
struct Stroke {
    cells: BTreeSet<[i32; 3]>,
    tool: Tool,
    material: u16,
}
impl Constructor {
    pub fn voxel_editing(&self) -> bool {
        self.voxel_edit.is_some()
    }
    pub fn voxel_allowed(&self, a: LabAction) -> bool {
        !self.voxel_editing()
            || matches!(
                a,
                LabAction::BeginVoxelEdit
                    | LabAction::SectionToggle
                    | LabAction::SectionShift(_)
                    | LabAction::SectionFocus
                    | LabAction::SectionAxis(_)
                    | LabAction::SectionSide
                    | LabAction::Metric(_)
                    | LabAction::MetricScale
                    | LabAction::ReplayStart | LabAction::ReplayStop | LabAction::ReplaySeek(_) | LabAction::ReplayJump(_) | LabAction::ReplaySave | LabAction::ReplayOpen | LabAction::ReplayBrowse(_) | LabAction::ReplayPlay | LabAction::ReplayLeave | LabAction::Tab(31)
                    | LabAction::Tab(30)
                    | LabAction::VoxelTool(_)
                    | LabAction::VoxelStroke
                    | LabAction::VoxelFinish(_)
                    | LabAction::VoxelUndo
                    | LabAction::VoxelRedo
                    | LabAction::PickMaterial(_)
                    | LabAction::Material(_)
                    | LabAction::Tab(1 | 17)
                    | LabAction::Help
                    | LabAction::Frame
                    | LabAction::ProbeFocus
                    | LabAction::SavePart
                    | LabAction::SaveScene
            )
    }
    pub fn can_voxel_edit(&self) -> bool {
        self.selected
            .and_then(|i| self.placed.get(i))
            .is_some_and(|p| {
                p.element.blocks.iter().any(|b| b.material != 0)
                    && p.element.units.is_empty()
                    && p.element.shields.is_empty()
            })
    }
    pub fn voxel_can_undo(&self, redo: bool) -> bool {
        self.voxel_edit.as_ref().is_some_and(|s| {
            if redo {
                !s.redo.is_empty()
            } else {
                !s.undo.is_empty()
            }
        })
    }
    fn replace_voxel_item(
        &mut self,
        index: usize,
        item: Placed,
        d: &Definition,
        r: Recipe,
    ) -> Result<(), String> {
        let mut next = self.placed.clone();
        *next
            .get_mut(index)
            .ok_or("Редактируемая деталь отсутствует")? = item;
        self.commit(next, d, r)
    }
    pub(super) fn voxel_action(
        &mut self,
        a: LabAction,
        d: &Definition,
        r: Recipe,
    ) -> Result<(bool, String), String> {
        if a == LabAction::SavePart {
            let i = self.selected.ok_or("Выберите деталь")?;
            let mut element = self.placed[i].element.clone();
            std::fs::create_dir_all(LIBRARY).map_err(|e| e.to_string())?;
            let number = (1..=128)
                .find(|n| {
                    !std::path::Path::new(LIBRARY)
                        .join(format!("custom-{n:04}.ron"))
                        .exists()
                        && !self
                            .catalog
                            .iter()
                            .any(|e| e.id == format!("custom-{n:04}"))
                })
                .ok_or("Библиотека пользовательских деталей заполнена")?;
            element.id = format!("custom-{number:04}");
            element.name = format!("Моя деталь {number:04}");
            element.category = Category::Custom;
            let text = element.to_ron()?;
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(std::path::Path::new(LIBRARY).join(format!("{}.ron", element.id)))
                .map_err(|e| e.to_string())?;
            file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            self.catalog.push(element);
            return Ok((
                false,
                "Новая версия детали сохранена в «Мои элементы». Исходная карточка сохранена"
                    .into(),
            ));
        }
        if a == LabAction::BeginVoxelEdit {
            if self.voxel_editing() {
                return Ok((false, String::new()));
            }
            if !self.can_voxel_edit() {
                return Err(
                    "Выберите блочную деталь или постройку; геометрия юнита задаётся шаблоном"
                        .into(),
                );
            }
            let i = self.selected.unwrap();
            self.voxel_edit = Some(Session {
                index: i,
                original: self.placed[i].clone(),
                tool: Tool::Add,
                undo: Vec::new(),
                redo: Vec::new(),
                stroke: None,
                queued: false,
            });
            self.mode = Mode::Voxel;
            self.hover = None;
            return Ok((
                true,
                "Редактирование исходной детали по кубикам 10 см. Физика на паузе".into(),
            ));
        }
        if !self.voxel_editing() {
            return Ok((false, String::new()));
        }
        if let LabAction::VoxelFinish(keep) = a {
            let changed = if keep {
                self.voxel_action(LabAction::VoxelStroke, d, r)?.0
            } else {
                false
            };
            let session = self.voxel_edit.take().unwrap();
            if !keep {
                self.replace_voxel_item(session.index, session.original, d, r)?;
            }
            self.mode = Mode::Select;
            self.selected = Some(session.index);
            self.hover = None;
            return Ok((
                changed || !keep,
                if keep {
                    "Кубики сохранены в детали и сцене"
                } else {
                    "Изменения кубиков отменены"
                }
                .into(),
            ));
        }
        if let LabAction::VoxelTool(n) = a {
            let tool = match n {
                0 => Tool::Add,
                1 => Tool::Erase,
                2 => Tool::Paint,
                _ => return Err("Неизвестный инструмент".into()),
            };
            let changed = self.voxel_action(LabAction::VoxelStroke, d, r)?.0;
            self.voxel_edit.as_mut().unwrap().tool = tool;
            self.mode = Mode::Voxel;
            return Ok((changed, String::new()));
        }
        if a == LabAction::VoxelStroke {
            let session = self.voxel_edit.as_mut().unwrap();
            session.queued = false;
            let Some(stroke) = session.stroke.take() else {
                return Ok((false, String::new()));
            };
            if stroke.cells.is_empty() {
                return Ok((false, String::new()));
            }
            let i = session.index;
            let before = self.placed[i].clone();
            let after = voxels::edit_placed(&before, &stroke.cells, stroke.tool, stroke.material)?;
            if before == after {
                return Ok((false, String::new()));
            }
            self.replace_voxel_item(i, after, d, r)?;
            let session = self.voxel_edit.as_mut().unwrap();
            session.undo.push(before);
            if session.undo.len() > 32 {
                session.undo.remove(0);
            }
            session.redo.clear();
            return Ok((
                true,
                format!("Деталь изменена: {} кубиков в мазке", stroke.cells.len()),
            ));
        }
        if matches!(a, LabAction::VoxelUndo | LabAction::VoxelRedo) {
            self.voxel_edit.as_mut().unwrap().stroke = None;
            let session = self.voxel_edit.as_ref().unwrap();
            let i = session.index;
            let source = if a == LabAction::VoxelUndo {
                &session.undo
            } else {
                &session.redo
            };
            let Some(item) = source.last().cloned() else {
                return Ok((false, String::new()));
            };
            let current = self.placed[i].clone();
            self.replace_voxel_item(i, item, d, r)?;
            let session = self.voxel_edit.as_mut().unwrap();
            if a == LabAction::VoxelUndo {
                session.undo.pop();
                session.redo.push(current);
            } else {
                session.redo.pop();
                session.undo.push(current);
            }
            return Ok((true, "Мазок отменён / восстановлен".into()));
        }
        Ok((false, String::new()))
    }
    pub fn stage_voxel_cells(
        &mut self,
        cells: BTreeSet<[i32; 3]>,
        tool: Tool,
        material: u16,
    ) -> Result<(), String> {
        if cells.len() > voxels::STROKE_CELLS {
            return Err("Слишком длинный мазок".into());
        }
        let session = self
            .voxel_edit
            .as_mut()
            .ok_or("Редактирование детали не открыто")?;
        session.stroke = Some(Stroke {
            cells,
            tool,
            material,
        });
        Ok(())
    }
    pub fn voxel_text(&self, d: &Definition, r: Recipe) -> String {
        self.voxel_edit.as_ref().map_or_else(||"Выберите деталь и нажмите «Кубики 10 см»".into(),|s|format!("{}\nКуб 100 × 100 × 100 мм\nМатериал: {}\nРежим: {}\nУчастков: {} / {} · отмена: {}",self.placed[s.index].element.name,self.material_name(d,r),match s.tool{Tool::Add=>"добавление",Tool::Erase=>"удаление",Tool::Paint=>"замена материала"},self.placed[s.index].element.blocks.len(),catalog::MAX_ELEMENT_BLOCKS,s.undo.len()))
    }
}
pub fn release(lab: &mut Laboratory, mouse: &ButtonInput<MouseButton>) {
    let Some(s) = lab.constructor.voxel_edit.as_mut() else {
        return;
    };
    if mouse.just_pressed(MouseButton::Right) {
        s.stroke = None;
        return;
    }
    if !mouse.pressed(MouseButton::Left) && s.stroke.is_some() && !s.queued {
        s.queued = true;
        lab.constructor.pending.push(LabAction::VoxelStroke);
    }
}
pub fn pointer(
    lab: &mut Laboratory,
    scene: &Scene,
    ray: Ray3d,
    mouse: &ButtonInput<MouseButton>,
    gizmos: &mut Gizmos,
) {
    let Some(session) = lab.constructor.voxel_edit.as_ref() else {
        return;
    };
    let item = &lab.constructor.placed[session.index];
    let Some(hit) = tools::liquid_pick(scene, ray) else {
        return;
    };
    let owned = voxels::material(&item.element, voxels::local_cell(item, hit)) != 0;
    let mut at = hit;
    if session.tool == Tool::Add {
        if let Some((_, axis)) = tools::entry(
            ray,
            Vec3::from(hit.map(|v| v as f32)),
            Vec3::from(hit.map(|v| v as f32)) + Vec3::ONE,
        ) {
            at[axis] += if ray.direction[axis] < 0.0 { 1 } else { -1 };
        }
    }
    let local = voxels::local_cell(item, at);
    let old = voxels::material(&item.element, local);
    let in_world = (0..3).all(|a| at[a] > 0 && at[a] < scene.sim.size()[a] as i32 - 1);
    let valid = owned
        && in_world
        && local[0] >= -128
        && local[0] < 128
        && local[2] >= -128
        && local[2] < 128
        && local[1] >= -128
        && local[1] < 128
        && (session.tool != Tool::Add || old == 0)
        && (session.tool != Tool::Add
            || lab.constructor.workspace.allow_overlap
            || !scene
                .sim
                .world
                .solid(at[0] as u32, at[1] as u32, at[2] as u32));
    let colour = if !valid {
        Color::srgb(1.0, 0.2, 0.15)
    } else {
        match session.tool {
            Tool::Add => Color::srgb(0.2, 1.0, 0.5),
            Tool::Erase => Color::srgb(1.0, 0.45, 0.15),
            Tool::Paint => Color::srgb(0.3, 0.7, 1.0),
        }
    };
    gizmos.cube(
        Transform::from_translation(Vec3::from(at.map(|v| v as f32)) + Vec3::splat(0.5))
            .with_scale(Vec3::splat(1.01)),
        colour,
    );
    if valid && mouse.pressed(MouseButton::Left) && !mouse.pressed(MouseButton::Right) && !mouse.pressed(MouseButton::Middle) {
        let material = lab.constructor.material(&lab.definition, lab.recipe);
        let s = lab.constructor.voxel_edit.as_mut().unwrap();
        let stroke = s.stroke.get_or_insert_with(|| Stroke {
            cells: BTreeSet::new(),
            tool: s.tool,
            material,
        });
        if stroke.cells.len() < voxels::STROKE_CELLS {
            stroke.cells.insert(at);
        }
    }
    if let Some(stroke) = lab.constructor.voxel_edit.as_ref().unwrap().stroke.as_ref() {
        for &p in &stroke.cells {
            gizmos.cube(
                Transform::from_translation(Vec3::from(p.map(|v| v as f32)) + Vec3::splat(0.5))
                    .with_scale(Vec3::splat(1.01)),
                colour,
            );
        }
    }
}
