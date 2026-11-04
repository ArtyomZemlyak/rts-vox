//! Authored catalogue/workspace, independent of the damaged live scene.
use super::*;
use modding::laboratory::catalog::{self, Category, Element, Placed, WeaponAsset};

#[path = "constructor_material.rs"]
mod material;
#[path = "constructor_voxels.rs"]
pub(super) mod voxels;
pub const LIBRARY: &str = "mods/laboratory/elements";
pub const PROJECT: &str = "mods/laboratory/scenes/latest.ron";
pub const WEAPONS: &str = "mods/laboratory/weapons";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Select,
    Brush,
    Erase,
    Inspect,
    Drill,
    Crawler,
    Collector,
    Source,
    Event,
    Place,
    Part,
    Voxel,
    Live,
    Aim,
    Move,
}
struct Draft {
    experiment: Option<modding::laboratory::experiments::Plan>,
    workspace: modding::laboratory::sources::Workspace,
    placed: Vec<Placed>,
    stand: bool,
    base: Option<RangeLayout>,
    selected: Option<usize>,
    aim: Option<[i32; 3]>,
    recipe: Recipe,
    weapon: Option<(ExplosionConfig, u8)>,
    strength: Option<(u16, u32)>,
}
pub struct Constructor {
    pub workspace: modding::laboratory::sources::Workspace,
    pub experiment: Option<modding::laboratory::experiments::Plan>,
    pub catalog: Vec<Element>,
    pub choice: usize,
    pub placed: Vec<Placed>,
    pub selected: Option<usize>,
    pub mode: Mode,
    pub stand: bool,
    pub base: Option<RangeLayout>,
    pub aim: Option<[i32; 3]>,
    pub weapon: Option<(ExplosionConfig, u8)>,
    pub strength: Option<(u16, u32)>,
    pub part_size: [i32; 3],
    pub pending: Vec<LabAction>,
    pub hover: Option<[i32; 3]>,
    pub revision: u64,
    pub turns: u8,
    pub elevation: i32,
    pub preview: Option<Preview>,
    #[cfg(test)]
    pub preview_builds: usize,
    preview_key: Option<(
        [i32; 3],
        u64,
        usize,
        u8,
        [i32; 3],
        Recipe,
        Option<usize>,
        u8,
    )>,
    snap_shape: Option<(
        (u64, usize, u8, [i32; 3], Recipe, Option<usize>, Mode),
        (Vec3, Vec3),
    )>,
    draft: Option<Draft>,
    scenario_return: Option<Draft>,
    pub weapons: Vec<WeaponAsset>,
    pub variant: Option<usize>,
    pub(super) voxel_edit: Option<voxels::Session>,
    validator: sim_core::blast::placement::Validator,
}
impl Constructor {
    pub fn new() -> Self {
        let mut catalog = catalog::stock();
        if let Ok(files) = std::fs::read_dir(LIBRARY) {
            let mut paths: Vec<_> = files
                .filter_map(Result::ok)
                .map(|f| f.path())
                .filter(|p| p.extension().is_some_and(|s| s == "ron"))
                .take(128)
                .collect();
            paths.sort();
            for path in paths {
                if let Ok(text) = read_bounded(&path, 1 << 20) {
                    if let Ok(e) = Element::from_ron(&text) {
                        if !catalog.iter().any(|other| other.id == e.id) {
                            catalog.push(e);
                        }
                    }
                }
            }
        }
        let mut weapons = Vec::new();
        if let Ok(files) = std::fs::read_dir(WEAPONS) {
            let mut paths: Vec<_> = files
                .filter_map(Result::ok)
                .map(|f| f.path())
                .filter(|p| p.extension().is_some_and(|s| s == "ron"))
                .take(128)
                .collect();
            paths.sort();
            for path in paths {
                if let Ok(text) = read_bounded(&path, 65536) {
                    if let Ok(w) = WeaponAsset::from_ron(&text) {
                        weapons.push(w);
                    }
                }
            }
        }
        Self {
            workspace: Default::default(),
            experiment: None,
            catalog,
            choice: 0,
            placed: Vec::new(),
            selected: None,
            mode: Mode::Select,
            stand: true,
            base: None,
            aim: None,
            weapon: None,
            strength: None,
            part_size: [16, 16, 16],
            pending: Vec::new(),
            hover: None,
            revision: 0,
            turns: 0,
            elevation: 0,
            preview: None,
            #[cfg(test)]
            preview_builds: 0,
            preview_key: None,
            snap_shape: None,
            draft: None,
            scenario_return: None,
            weapons,
            variant: None,
            voxel_edit: None,
            validator: sim_core::blast::placement::Validator::default(),
        }
    }
    pub fn creating(&self) -> bool {
        self.draft.is_some()
    }
    pub fn base_layout(&self, d: &Definition, r: Recipe) -> Result<RangeLayout, String> {
        let mut layout = if let Some(base) = &self.base {
            base.clone()
        } else if self.stand {
            d.fixture(r)?.0
        } else {
            d.layout.clone()
        };
        if let Some(source) = self.workspace.sources.get(self.workspace.selected) {
            layout.gun_point = true;
            layout.gun = source.at;
            layout.target = source.target;
        }
        layout.laboratory = true;
        layout.allow_overlap = self.workspace.allow_overlap;
        layout.lab_devices = self.workspace.devices.clone();
        Ok(layout)
    }
    pub fn remember_scene(&mut self, recipe: Recipe) {
        if self.scenario_return.is_some() {
            return;
        }
        self.scenario_return = Some(Draft {
            experiment: self.experiment.clone(),
            workspace: self.workspace.clone(),
            placed: self.placed.clone(),
            stand: self.stand,
            base: self.base.clone(),
            selected: self.selected,
            aim: self.aim,
            recipe,
            weapon: self.weapon,
            strength: self.strength,
        });
    }
    pub fn can_return(&self) -> bool {
        self.scenario_return.is_some() && !self.creating()
    }
    pub fn return_scene(&mut self, recipe: &mut Recipe) -> Result<(), String> {
        if !self.can_return() {
            return Err("Сначала завершите создание элемента".into());
        }
        self.draft = self.scenario_return.take();
        self.restore_draft(recipe);
        Ok(())
    }
    pub fn layout(&self, d: &Definition, r: Recipe) -> Result<RangeLayout, String> {
        let mut layout = catalog::compose(&self.base_layout(d, r)?, &self.placed)?;
        if let Some(source) = self.workspace.sources.get(self.workspace.selected) {
            layout.gun_point = true;
            layout.gun = source.at;
            layout.target = source.target;
        }
        if let Some(aim) = self.aim {
            layout.target = aim;
        }
        self.validator
            .validate(&layout)
            .map_err(|e| e.to_string())?;
        Ok(layout)
    }
    fn commit(&mut self, items: Vec<Placed>, d: &Definition, r: Recipe) -> Result<(), String> {
        self.check_items(&items, d, r)?;
        self.placed = items;
        self.revision += 1;
        Ok(())
    }
    pub fn part(&self, d: &Definition, r: Recipe) -> Element {
        let [x, y, z] = self.part_size;
        Element {
            id: "draft-block".into(),
            name: "Деталь".into(),
            category: Category::Custom,
            blocks: vec![RangeBlock {
                min: [-x / 2, 0, -z / 2],
                max: [x - x / 2, y, z - z / 2],
                material: self.material(d, r),
            }],
            units: Vec::new(),
            unit_yaws: Vec::new(),
            shields: Vec::new(),
        }
    }
    fn restore_draft(&mut self, recipe: &mut Recipe) {
        if let Some(d) = self.draft.take() {
            self.workspace = d.workspace;
            self.placed = d.placed;
            self.stand = d.stand;
            self.base = d.base;
            self.selected = d.selected;
            self.aim = d.aim;
            *recipe = d.recipe;
            self.weapon = d.weapon;
            self.strength = d.strength;
            self.experiment = d.experiment;
        }
        self.mode = Mode::Select;
        self.turns = 0;
        self.elevation = 0;
        self.hover = None;
        self.preview = None;
        self.preview_key = None;
        self.revision += 1;
    }
    /// Returns true only when authored geometry changed and needs a fresh SIM.
    pub fn action(
        &mut self,
        a: LabAction,
        d: &Definition,
        r: &mut Recipe,
    ) -> Result<(bool, String), String> {
        if !self.voxel_allowed(a) {
            return Err("Завершите редактирование кубиков кнопкой «Готово» или «Отмена»".into());
        }
        match a {
            LabAction::BeginVoxelEdit
            | LabAction::VoxelTool(_)
            | LabAction::VoxelStroke
            | LabAction::VoxelFinish(_)
            | LabAction::VoxelUndo
            | LabAction::VoxelRedo
            | LabAction::SavePart => return self.voxel_action(a, d, *r),
            LabAction::PickMaterial(id) => return self.pick_material(id, d),
            LabAction::Material(delta) => {
                let current = d
                    .materials
                    .iter()
                    .position(|m| m.id == self.material(d, *r))
                    .unwrap_or(r.material);
                let index = (current as i32 + delta).rem_euclid(d.materials.len() as i32) as usize;
                return self.pick_material(d.materials[index].id, d);
            }
            LabAction::ApplyMaterial => return self.apply_material(d, r),
            LabAction::PickCatalog(i) => {
                if i >= self.catalog.len() {
                    return Err("Объект отсутствует в каталоге".into());
                }
                self.choice = i;
                self.selected = None;
                self.mode = Mode::Place;
                self.turns = 0;
                self.elevation = 0;
            }
            LabAction::PickObject(i) => {
                self.selected = (i < self.placed.len()).then_some(i);
                self.mode = Mode::Select;
                self.sync_material(d);
            }
            LabAction::Tool(i) => {
                self.mode = match i {
                    1 => Mode::Move,
                    2 => Mode::Brush,
                    3 => Mode::Erase,
                    4 => Mode::Inspect,
                    5 => Mode::Drill,
                    6 => Mode::Crawler,
                    7 => Mode::Collector,
                    _ => Mode::Select,
                };
                self.hover = None;
            }
            LabAction::Turn(delta) => {
                if matches!(
                    self.mode,
                    Mode::Place
                        | Mode::Part
                        | Mode::Source
                        | Mode::Event
                        | Mode::Drill
                        | Mode::Crawler
                        | Mode::Collector
                ) {
                    self.turns = (i32::from(self.turns) + delta).rem_euclid(4) as u8;
                } else {
                    let i = self
                        .selected
                        .ok_or("Выберите объект или карточку в библиотеке")?;
                    let mut next = self.placed.clone();
                    next[i].turns = (i32::from(next[i].turns) + delta).rem_euclid(4) as u8;
                    self.commit(next, d, *r)?;
                    return Ok((true, "Поворот на 90°".into()));
                }
            }
            LabAction::Elevation(delta) => {
                if matches!(
                    self.mode,
                    Mode::Place
                        | Mode::Part
                        | Mode::Source
                        | Mode::Event
                        | Mode::Drill
                        | Mode::Crawler
                        | Mode::Collector
                ) {
                    self.elevation = (self.elevation + delta * d.editor_step)
                        .clamp(if self.mode == Mode::Source { -1024 } else { 0 }, 1024);
                } else {
                    return self.action(LabAction::Move(1, delta), d, r);
                }
            }
            LabAction::MoveAt(at) => {
                let i = self.selected.ok_or("Выберите объект")?;
                let mut next = self.placed.clone();
                next[i].at = at;
                self.commit(next, d, *r)?;
                self.mode = Mode::Select;
                return Ok((true, "Объект перемещён".into()));
            }
            LabAction::Duplicate => {
                let i = self.selected.ok_or("Выберите объект для копирования")?;
                let item = self.placed[i].clone();
                self.choice = if let Some(i) = self.catalog.iter().position(|e| e == &item.element)
                {
                    i
                } else {
                    self.catalog.push(item.element);
                    self.catalog.len() - 1
                };
                self.turns = item.turns;
                self.selected = None;
                self.elevation = (item.at[1] - self.base_layout(d, *r)?.ground_level).max(0);
                self.mode = Mode::Place;
            }
            LabAction::Catalog(delta) => {
                self.choice =
                    (self.choice as i32 + delta).rem_euclid(self.catalog.len() as i32) as usize;
                self.mode = Mode::Select;
            }
            LabAction::Place => {
                self.selected = None;
                self.mode = Mode::Place;
            }
            LabAction::AddPart => {
                self.mode = Mode::Part;
                self.turns = 0;
                self.elevation = 0;
            }
            LabAction::Aim => self.mode = Mode::Aim,
            LabAction::PlaceAt(at) => {
                let element = if self.mode == Mode::Part {
                    self.part(d, *r)
                } else {
                    self.catalog[self.choice].clone()
                };
                let mut next = self.placed.clone();
                next.push(Placed {
                    element,
                    at,
                    turns: self.turns,
                });
                self.commit(next, d, *r)?;
                self.selected = Some(self.placed.len() - 1);
                return Ok((true, "Элемент добавлен; ПКМ завершает размещение".into()));
            }
            LabAction::Select(delta) => {
                self.mode = Mode::Select;
                if !self.placed.is_empty() {
                    self.selected = Some(
                        (self.selected.map_or(0, |i| i as i32) + delta)
                            .rem_euclid(self.placed.len() as i32) as usize,
                    );
                }
            }
            LabAction::Move(axis, delta) => {
                let i = self.selected.ok_or("Выберите добавленный элемент")?;
                let mut next = self.placed.clone();
                next[i].at[axis as usize] += delta * d.editor_step;
                self.commit(next, d, *r)?;
                return Ok((true, "Элемент перемещён".into()));
            }
            LabAction::Rotate => {
                let i = self.selected.ok_or("Выберите добавленный элемент")?;
                let mut next = self.placed.clone();
                next[i].turns = (next[i].turns + 1) % 4;
                self.commit(next, d, *r)?;
                return Ok((true, "Элемент повёрнут".into()));
            }
            LabAction::Split => {
                let i = self.selected.ok_or("Выберите готовую сборку")?;
                let parts = catalog::parts(&self.placed[i], &self.base_layout(d, *r)?)?;
                let bound_part = parts.iter().position(|p| !p.element.units.is_empty());
                let mut next = self.placed.clone();
                let added = parts.len().saturating_sub(1);
                next.splice(i..=i, parts);
                self.commit(next, d, *r)?;
                for s in &mut self.workspace.sources {
                    if let Some(b) = s.binding {
                        if b > i {
                            s.binding = Some(b + added);
                        } else if b == i {
                            s.binding = bound_part.map(|n| i + n);
                        }
                    }
                }
                self.selected = Some(i);
                return Ok((
                    true,
                    "Сборка разобрана: выберите деталь для изменения".into(),
                ));
            }
            LabAction::ApplySize => {
                let i = self.selected.ok_or("Выберите деталь")?;
                let mut next = self.placed.clone();
                let e = &mut next[i].element;
                if e.blocks.len() != 1 || !e.units.is_empty() || !e.shields.is_empty() {
                    return Err("Сначала разберите сборку и выберите отдельный блок".into());
                }
                let b = &mut e.blocks[0];
                for axis in 0..3 {
                    let center = b.min[axis] + (b.max[axis] - b.min[axis]) / 2;
                    if axis != 1 {
                        b.min[axis] = center - self.part_size[axis] / 2;
                    }
                    b.max[axis] = b.min[axis] + self.part_size[axis];
                }
                self.commit(next, d, *r)?;
                return Ok((
                    true,
                    "Деталь изменена; исходный элемент библиотеки сохранён".into(),
                ));
            }
            LabAction::Delete => {
                let i = self.selected.ok_or("Выберите добавленный элемент")?;
                let old_plan = self.experiment.clone();
                let old_workspace = self.workspace.clone();
                let first = self.base.as_ref().map_or(0, |l| l.units.len())
                    + self.placed[..i]
                        .iter()
                        .map(|p| p.element.units.len())
                        .sum::<usize>();
                let count = self.placed[i].element.units.len();
                if count > 0 {
                    scenario::remove_actors(
                        &mut self.experiment,
                        &mut self.workspace,
                        first,
                        count,
                    );
                }
                let mut next = self.placed.clone();
                next.remove(i);
                if let Err(e) = self.commit(next, d, *r) {
                    self.experiment = old_plan;
                    self.workspace = old_workspace;
                    return Err(e);
                }
                for s in &mut self.workspace.sources {
                    if let Some(b) = s.binding {
                        if b == i {
                            s.binding = None;
                        } else if b > i {
                            s.binding = Some(b - 1);
                        }
                    }
                }
                self.selected = self.placed.len().checked_sub(1);
                self.mode = Mode::Select;
                self.hover = None;
                return Ok((true, "Элемент удалён".into()));
            }
            LabAction::Clear => {
                self.experiment = None;
                self.stand = false;
                self.base = None;
                self.placed.clear();
                for s in &mut self.workspace.sources {
                    s.binding = None;
                    s.input = None;
                }
                self.selected = None;
                self.mode = Mode::Select;
                self.hover = None;
                self.aim = None;
                if r.effect == 5 {
                    r.effect = 0;
                }
                self.revision += 1;
                return Ok((true, "Пустая сцена; добавьте готовые элементы".into()));
            }
            LabAction::Stand => {
                self.experiment = None;
                for s in &mut self.workspace.sources {
                    s.input = None;
                }
                self.stand = !self.stand;
                self.base = None;
                if !self.stand && r.effect == 5 {
                    r.effect = 0;
                }
                self.revision += 1;
                return Ok((true, "Параметрический стенд переключён".into()));
            }
            LabAction::Dimension(axis, delta) => {
                self.part_size[axis as usize] = (self.part_size[axis as usize]
                    + delta * d.editor_step)
                    .clamp(d.editor_step, 64);
            }
            LabAction::Create => {
                if self.draft.is_some() {
                    return Err("Завершите текущий элемент".into());
                }
                self.draft = Some(Draft {
                    experiment: self.experiment.take(),
                    workspace: std::mem::take(&mut self.workspace),
                    placed: std::mem::take(&mut self.placed),
                    stand: self.stand,
                    base: self.base.take(),
                    selected: self.selected.take(),
                    aim: self.aim.take(),
                    recipe: *r,
                    weapon: self.weapon.take(),
                    strength: self.strength.take(),
                });
                self.stand = false;
                self.mode = Mode::Select;
                if r.effect == 5 {
                    r.effect = 0;
                }
                self.revision += 1;
                return Ok((true,"Создание элемента: добавляйте детали и готовые объекты; затем сохраните сборку".into()));
            }
            LabAction::CancelCreate => {
                self.restore_draft(r);
                return Ok((true, "Вернулись к исходной сцене".into()));
            }
            LabAction::SaveElement => {
                if !self.creating() {
                    return Err("Сначала нажмите «Создать свой элемент»".into());
                }
                std::fs::create_dir_all(LIBRARY).map_err(|e| e.to_string())?;
                let number = (1..=10000)
                    .find(|n| {
                        !std::path::Path::new(LIBRARY)
                            .join(format!("custom-{n:04}.ron"))
                            .exists()
                            && !self
                                .catalog
                                .iter()
                                .any(|e| e.id == format!("custom-{n:04}"))
                    })
                    .ok_or("Библиотека заполнена")?;
                let id = format!("custom-{number:04}");
                let name = format!("Мой элемент {number:04}");
                let mut origin = self
                    .placed
                    .first()
                    .ok_or("Добавьте хотя бы одну деталь")?
                    .at;
                origin[1] = self.placed.iter().map(|p| p.at[1]).min().unwrap();
                let element =
                    catalog::combine(&self.placed, origin, id.clone(), name.clone(), &d.layout)?;
                let text = element.to_ron()?;
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(std::path::Path::new(LIBRARY).join(format!("{id}.ron")))
                    .map_err(|e| e.to_string())?;
                file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
                self.catalog.push(element);
                self.choice = self.catalog.len() - 1;
                self.restore_draft(r);
                self.selected = None;
                self.mode = Mode::Place;
                return Ok((
                    true,
                    format!("{name} сохранён в библиотеке; выберите место в сцене"),
                ));
            }
            _ => return Err("unknown constructor action".into()),
        }
        Ok((false, String::new()))
    }
    pub fn selection_at(
        &self,
        d: &Definition,
        r: Recipe,
        selection: Option<workbench::Selection>,
    ) -> Option<usize> {
        let base = self.base_layout(d, r).ok()?;
        let mut blocks = base.blocks.len();
        let mut units = base.units.len();
        let mut shields = base.shields.len();
        for (i, e) in self.placed.iter().enumerate() {
            if matches!(selection,Some(workbench::Selection::Block(n)) if (blocks..blocks+e.element.blocks.len()).contains(&n))
                || matches!(selection,Some(workbench::Selection::Unit(n)) if (units..units+e.element.units.len()).contains(&n))
                || matches!(selection,Some(workbench::Selection::Shield(n)) if (shields..shields+e.element.shields.len()).contains(&n))
            {
                return Some(i);
            }
            shields += e.element.shields.len();
            blocks += e.element.blocks.len();
            units += e.element.units.len();
        }
        None
    }
}
pub fn read_bounded(path: &std::path::Path, max: u64) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > max {
        return Err("Файл превышает лимит".into());
    }
    use std::io::Read;
    let mut text = String::new();
    file.take(max + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > max {
        return Err("Файл превышает лимит".into());
    }
    Ok(text)
}
pub fn text(lab: &Laboratory, kind: u8) -> String {
    let c = &lab.constructor;
    match kind {
        7 => format!(
            "{} · объектов: {}\n{}",
            if c.creating() {
                "СОЗДАНИЕ ЭЛЕМЕНТА"
            } else {
                "КОНСТРУКТОР ОПЫТА"
            },
            c.placed.len(),
            match c.mode {
                Mode::Place => "ЛКМ — разместить; ПКМ — отмена",
                Mode::Part => "ЛКМ — поставить новую деталь",
                Mode::Voxel => "Кубики детали 10 см · ЛКМ — мазок · отпускание — применить · Esc — готово",
                Mode::Live => "Живые инструменты · ЛКМ — воздействовать · WASD/Z/X — полёт · ПКМ — обзор · Esc — выход",
                Mode::Aim => "ЛКМ — выбрать точку воздействия",
                Mode::Source => "ЛКМ — поставить источник",
                Mode::Event => "ЛКМ — поставить воздействие; F — запустить после размещения",
                Mode::Select => "ЛКМ — выбрать объект",
                Mode::Move => "ЛКМ — переместить выбранный объект",
                Mode::Brush=>"ЛКМ — добавить выбранный материал",
                Mode::Erase=>"ЛКМ — удалить материал",
                Mode::Inspect=>"ЛКМ — прочитать кубик",
                Mode::Drill=>"ЛКМ — поставить вертикальный бур",
                Mode::Crawler=>"ЛКМ — поставить краулер · Q/E — направление",
                Mode::Collector=>"ЛКМ — поставить сборщик и его разгрузочную площадку · Пуск — начать сбор",
            }
        ),
        8 => {
            let e = &c.catalog[c.choice];
            format!(
                "{} / {} · {}\n{}",
                c.choice + 1,
                c.catalog.len(),
                e.name,
                match e.category {
                    Category::Block => "Блок",
                    Category::Structure => "Постройка",
                    Category::Unit => "Юнит",
                    Category::Vegetation => "Растительность",
                    Category::Shield => "Щит",
                    Category::Custom => "Мой элемент",
                }
            )
        }
        9 => {
            if matches!(c.mode, Mode::Place | Mode::Part) {
                format!(
                    "Размещение: {}\nПоворот: {}° · высота: {:+} ед.\n{}",
                    if c.mode == Mode::Part {
                        "Новая деталь"
                    } else {
                        &c.catalog[c.choice].name
                    },
                    c.turns as u16 * 90,
                    c.elevation,
                    c.hover.map_or("Наведите на площадку".into(), |p| format!(
                        "X {} · Y {} · Z {}",
                        p[0], p[1], p[2]
                    ))
                )
            } else {
                c.selected.and_then(|i|c.placed.get(i)).map_or_else(
            ||"Объект не выбран.\nНажмите на объект в сцене или строку в составе сцены. Для добавления выберите карточку внизу.".into(),
            |e|format!("{}\nX {} · Y {} · Z {}\nПоворот {}° · блоков: {} · юнитов: {}",e.element.name,e.at[0],e.at[1],e.at[2],e.turns as u16*90,e.element.blocks.len(),e.element.units.len()))
            }
        }
        10 => format!(
            "Новая деталь: {} × {} × {} мм",
            c.part_size[0] * voxel::unit::WORLD_MM as i32,
            c.part_size[1] * voxel::unit::WORLD_MM as i32,
            c.part_size[2] * voxel::unit::WORLD_MM as i32
        ),
        _ => String::new(),
    }
}

/// Resolved authoring geometry; valid and invalid previews share the same pose.
pub struct Preview {
    pub geometry: RangeLayout,
    pub issue: Option<String>,
}
impl Constructor {
    fn check_items(&self, items: &[Placed], d: &Definition, r: Recipe) -> Result<(), String> {
        let base = self.base_layout(d, r)?;
        let layout = catalog::compose(&base, items)?;
        if !self.workspace.allow_overlap {
            let mut boxes: Vec<(usize, RangeBlock)> = base
                .blocks
                .iter()
                .filter(|b| b.material != 0 && b.material != 11)
                .cloned()
                .map(|b| (usize::MAX, b))
                .collect();
            for (i, item) in items.iter().enumerate() {
                let mut shape = base.clone();
                shape.blocks.clear();
                shape.units.clear();
                shape.shields.clear();
                shape.unit_yaws.clear();
                item.append(&mut shape)?;
                for b in shape
                    .blocks
                    .into_iter()
                    .filter(|b| b.material != 0 && b.material != 11)
                {
                    if boxes.iter().any(|(j, a)| {
                        *j != i
                            && (0..3)
                                .all(|axis| b.min[axis] < a.max[axis] && a.min[axis] < b.max[axis])
                    }) {
                        return Err("Пересечение элементов запрещено. Включите «Пересечения» во вкладке инструментов".into());
                    }
                    boxes.push((i, b));
                }
            }
        }
        if let Some(plan) = &self.experiment {
            plan.validate(&layout)?;
        }
        self.validator.validate(&layout).map_err(placement_error)
    }
    fn local_bounds(
        &mut self,
        d: &Definition,
        r: Recipe,
        templates: &workbench::ghost::Templates,
    ) -> Option<(Vec3, Vec3)> {
        let key = (
            self.revision,
            self.choice,
            self.turns,
            self.part_size,
            r,
            self.selected,
            self.mode,
        );
        if let Some((saved, bounds)) = &self.snap_shape {
            if *saved == key {
                return Some(*bounds);
            }
        }
        let item = if self.mode == Mode::Move {
            let mut item = self.selected.and_then(|i| self.placed.get(i))?.clone();
            item.at = [0; 3];
            item
        } else {
            Placed {
                element: if self.mode == Mode::Part {
                    self.part(d, r)
                } else {
                    self.catalog[self.choice].clone()
                },
                at: [0; 3],
                turns: self.turns,
            }
        };
        let mut shape = d.layout.clone();
        shape.blocks.clear();
        shape.units.clear();
        shape.shields.clear();
        shape.unit_yaws.clear();
        item.append(&mut shape).ok()?;
        let bounds = visuals::bounds(&shape, templates);
        self.snap_shape = Some((key, bounds));
        Some(bounds)
    }
    pub fn preview_at(&mut self, at: [i32; 3], d: &Definition, r: Recipe) {
        let moving = self.mode == Mode::Move;
        let key = (
            at,
            self.revision,
            self.choice,
            self.turns,
            self.part_size,
            r,
            self.selected,
            u8::from(moving) + u8::from(self.mode == Mode::Part) * 2,
        );
        if self.preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_key = Some(key);
        #[cfg(test)]
        {
            self.preview_builds += 1;
        }
        let item = if moving {
            let Some(mut item) = self.selected.and_then(|i| self.placed.get(i)).cloned() else {
                self.preview = None;
                return;
            };
            item.at = at;
            item
        } else {
            Placed {
                element: if self.mode == Mode::Part {
                    self.part(d, r)
                } else {
                    self.catalog[self.choice].clone()
                },
                at,
                turns: self.turns,
            }
        };
        let mut next = self.placed.clone();
        if moving {
            next[self.selected.unwrap()] = item.clone();
        } else {
            next.push(item.clone());
        }
        let issue = self.check_items(&next, d, r).err();
        let mut geometry = d.layout.clone();
        geometry.blocks.clear();
        geometry.units.clear();
        geometry.unit_yaws.clear();
        geometry.shields.clear();
        if item.append(&mut geometry).is_err() {
            self.preview = None;
            return;
        }
        self.preview = Some(Preview { geometry, issue });
    }
}
#[allow(clippy::too_many_arguments)]
pub fn pointer(
    mut lab: ResMut<Laboratory>,
    scene: Res<Scene>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<GasCamera>>,
    roots: Query<&bevy::ui::RelativeCursorPosition, With<Root>>,
    mouse: Res<ButtonInput<MouseButton>>,
    streaming: Res<streaming::Streaming>,
    templates: Option<Res<workbench::ghost::Templates>>,
    mut gizmos: Gizmos,
) {
    if !lab.active() {
        return;
    }
    let Some(templates) = templates else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    if streaming.loading() {
        if !(lab.tools.flight.preview && scene.out.is_some()) {
            live_tools::release(&mut lab, &mouse, true);
        }
        lab.ui.viewport_ready = 0;
        lab.constructor.hover = None;
        return;
    }
    lab.ui.viewport_ready = lab.ui.viewport_ready.saturating_add(1);
    if lab.tools.flight.preview && scene.out.is_some() {
        return;
    }
    voxels::release(&mut lab, &mouse);
    let c = &lab.constructor;
    if lab.ui.grid {
        let y = scene.sim.ground_level() as f32 + 0.15;
        let size = scene.sim.size();
        for i in (0..=size[0]).step_by(16) {
            gizmos.line(
                Vec3::new(i as f32, y, 0.0),
                Vec3::new(i as f32, y, size[2] as f32),
                Color::srgba(0.5, 0.65, 0.75, 0.16),
            );
        }
        for i in (0..=size[2]).step_by(16) {
            gizmos.line(
                Vec3::new(0.0, y, i as f32),
                Vec3::new(size[0] as f32, y, i as f32),
                Color::srgba(0.5, 0.65, 0.75, 0.16),
            );
        }
    }
    sources::draw(&lab, &scene, &mut gizmos);
    tools::draw(&lab, &scene, &mut gizmos);
    if let Some(item) = c.selected.and_then(|i| c.placed.get(i)) {
        let mut shape = lab.definition.layout.clone();
        shape.blocks.clear();
        shape.units.clear();
        shape.unit_yaws.clear();
        shape.shields.clear();
        if item.append(&mut shape).is_ok() {
            visuals::outline(&shape, &templates, &mut gizmos, theme::ACCENT);
        }
        if !c.voxel_editing() && !lab.tools.live {
            let p = Vec3::from(item.at.map(|v| v as f32)) + Vec3::Y * 0.5;
            gizmos.arrow(p, p + Vec3::X * 20.0, Color::srgb(1.0, 0.3, 0.25));
            gizmos.arrow(p, p + Vec3::Y * 20.0, Color::srgb(0.35, 0.95, 0.4));
            gizmos.arrow(p, p + Vec3::Z * 20.0, Color::srgb(0.3, 0.6, 1.0));
        }
    }
    if let Some(at) = lab.ui.capture_at.filter(|_| scene.out.is_some()) {
        if lab.constructor.mode == Mode::Event {
            gizmos.cross(
                Vec3::from(at.map(|v| v as f32)),
                6.0,
                Color::srgb(0.25, 1.0, 0.5),
            );
            return;
        }
        let l = &mut *lab;
        l.constructor.hover = Some(at);
        l.constructor.preview_at(at, &l.definition, l.recipe);
        if let Some(p) = &l.constructor.preview {
            visuals::outline(
                &p.geometry,
                &templates,
                &mut gizmos,
                if p.issue.is_some() {
                    Color::srgb(1.0, 0.24, 0.18)
                } else {
                    Color::srgb(0.25, 1.0, 0.5)
                },
            );
        }
        return;
    }
    let blocked = !window.focused
        || (!first_person::immersive(&lab) && roots.iter().any(|r| r.cursor_over()))
        || lab.browser
        || lab.ui.help
        || lab.ui.editing.is_some();
    live_tools::release(&mut lab, &mouse, blocked);
    if blocked {
        lab.constructor.hover = None;
        return;
    }
    if mouse.just_pressed(MouseButton::Right)
        && !lab.tools.flying
        && !lab.constructor.voxel_editing()
        && !lab.tools.live
    {
        lab.constructor.mode = Mode::Select;
        lab.constructor.hover = None;
        return;
    }
    let cursor = if first_person::immersive(&lab) {
        Some(Vec2::new(window.width(), window.height()) * 0.5)
    } else {
        window.cursor_position()
    };
    let Some(cursor) = cursor else {
        live_tools::release(&mut lab, &mouse, true);
        lab.constructor.hover = None;
        return;
    };
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    let ray = if first_person::immersive(&lab) {
        let pose = lab.tools.camera.unwrap();
        Ok(Ray3d::new(pose.translation, pose.forward()))
    } else {
        camera.viewport_to_world(transform, cursor)
    };
    let Ok(ray) = ray else {
        return;
    };
    let mode = lab.constructor.mode;
    if tools::pointer(&mut lab, &scene, ray, &mouse, &mut gizmos) {
        return;
    }
    match mode {
        Mode::Voxel => voxels::pointer(&mut lab, &scene, ray, &mouse, &mut gizmos),
        Mode::Brush
        | Mode::Erase
        | Mode::Inspect
        | Mode::Drill
        | Mode::Crawler
        | Mode::Collector
        | Mode::Live => unreachable!(),
        Mode::Event => {
            let Some(index) = lab.ui.event else {
                return;
            };
            if let Some((mut at, _)) = workbench::pick(&scene.sim, ray) {
                at[1] += lab.constructor.elevation;
                let size = scene.sim.size();
                let valid = (0..3).all(|a| at[a] >= 0 && at[a] < size[a] as i32)
                    && at[0] >= 32
                    && at[0] < size[0] as i32 - 32
                    && at[2] >= 32
                    && at[2] < size[2] as i32 - 32;
                lab.constructor.hover = Some(at);
                let colour = if valid {
                    Color::srgb(0.25, 1.0, 0.5)
                } else {
                    Color::srgb(1.0, 0.24, 0.18)
                };
                gizmos.cross(Vec3::from(at.map(|v| v as f32)), 6.0, colour);
                gizmos.cube(
                    Transform::from_translation(Vec3::from(at.map(|v| v as f32)))
                        .with_scale(Vec3::splat(5.0)),
                    colour,
                );
                if mouse.just_pressed(MouseButton::Left) {
                    if valid {
                        lab.constructor
                            .pending
                            .push(LabAction::PlaceEvent(index, at));
                    } else {
                        lab.notice = "Воздействие выходит за границы карты".into();
                    }
                }
            } else {
                lab.constructor.hover = None;
            }
        }
        Mode::Source => {
            let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
                return;
            };
            let y = sources::active(&lab).map_or(scene.sim.ground_level() + 32, |s| s.at[1])
                + lab.constructor.elevation;
            if ray.direction.y.abs() < 0.0001 {
                return;
            }
            let t = (y as f32 - ray.origin.y) / ray.direction.y;
            if t < 0.0 {
                return;
            }
            let p = ray.origin + ray.direction * t;
            let step = lab.definition.editor_step as f32;
            let at = [
                (p.x / step).round() as i32 * step as i32,
                y,
                (p.z / step).round() as i32 * step as i32,
            ];
            let size = scene.sim.size();
            let valid = (0..3).all(|a| at[a] >= 0 && at[a] < size[a] as i32)
                && at[0] >= 32
                && at[0] < size[0] as i32 - 32
                && at[2] >= 32
                && at[2] < size[2] as i32 - 32;
            gizmos.cube(
                Transform::from_translation(Vec3::from(at.map(|v| v as f32)))
                    .with_scale(Vec3::splat(5.0)),
                if valid {
                    Color::srgb(0.3, 1.0, 0.6)
                } else {
                    Color::srgb(1.0, 0.2, 0.15)
                },
            );
            if mouse.just_pressed(MouseButton::Left) {
                if valid {
                    lab.constructor.pending.push(LabAction::SourceAt(at));
                } else {
                    lab.notice = "Источник вне границ карты".into();
                }
            }
        }
        Mode::Place | Mode::Part | Mode::Move => {
            let ground = scene.sim.ground_level();
            let y = if mode == Mode::Move {
                let Some(item) = lab
                    .constructor
                    .selected
                    .and_then(|i| lab.constructor.placed.get(i))
                else {
                    return;
                };
                item.at[1]
            } else {
                ground + lab.constructor.elevation
            };
            let surface_hit = if lab.constructor.workspace.surface_snap.unwrap_or(true) {
                workbench::pick(&scene.sim, ray)
            } else {
                None
            };
            let t = (y as f32 - ray.origin.y) / ray.direction.y;
            let p = if ray.direction.y.abs() < 1e-6 || !t.is_finite() || t < 0.0 {
                let Some((hit, _)) = surface_hit else {
                    return;
                };
                Vec3::from(hit.map(|v| v as f32))
            } else {
                ray.origin + ray.direction * t
            };
            let step = lab.definition.editor_step;
            let mut at = [
                (p.x / step as f32).round() as i32 * step,
                y,
                (p.z / step as f32).round() as i32 * step,
            ];
            if lab.constructor.workspace.surface_snap.unwrap_or(true) {
                if let Some((hit, _)) = surface_hit {
                    let l = &mut *lab;
                    if let Some((lo, hi)) =
                        l.constructor
                            .local_bounds(&l.definition, l.recipe, &templates)
                    {
                        if lo.is_finite() && hi.is_finite() {
                            at = tools::surface_anchor(
                                ray,
                                hit,
                                lo,
                                hi,
                                step,
                                l.constructor.elevation,
                            );
                        }
                    }
                }
            }
            let l = &mut *lab;
            l.constructor.hover = Some(at);
            l.constructor.preview_at(at, &l.definition, l.recipe);
            if let Some(preview) = &l.constructor.preview {
                let colour = if preview.issue.is_some() {
                    Color::srgb(1.0, 0.24, 0.18)
                } else {
                    Color::srgb(0.25, 1.0, 0.5)
                };
                visuals::outline(&preview.geometry, &templates, &mut gizmos, colour);
                if mouse.just_pressed(MouseButton::Left) {
                    if let Some(issue) = &preview.issue {
                        l.notice = issue.clone();
                    } else {
                        l.constructor.pending.push(if mode == Mode::Move {
                            LabAction::MoveAt(at)
                        } else {
                            LabAction::PlaceAt(at)
                        });
                    }
                }
            }
        }
        Mode::Aim | Mode::Select => {
            lab.constructor.hover = None;
            if mouse.just_pressed(MouseButton::Left) {
                if mode == Mode::Select {
                    if let Some(i) = sources::pick(&lab, &scene.sim, camera, transform, cursor) {
                        lab.constructor.pending.push(LabAction::Source(i));
                        lab.ui.tab = 10;
                        return;
                    }
                    if let Some(i) = scenario::pick_device(&lab, camera, transform, cursor) {
                        lab.constructor.pending.push(LabAction::Device(i));
                        return;
                    }
                }
                let hit = workbench::pick(&scene.sim, ray);
                if mode == Mode::Aim {
                    if let Some((p, _)) = hit {
                        if lab.ui.tab == 24 {
                            lab.constructor.pending.push(LabAction::ScenarioAt(p));
                        } else {
                            lab.constructor.pending.push(LabAction::AimAt(p));
                        }
                    }
                } else {
                    let selected = lab.constructor.selection_at(
                        &lab.definition,
                        lab.recipe,
                        hit.and_then(|(_, s)| s),
                    );
                    if let Some(i) = selected {
                        lab.constructor.pending.push(LabAction::PickObject(i));
                    } else {
                        lab.constructor.selected = None;
                    }
                }
            }
        }
    }
}

fn placement_error(e: sim_core::blast::placement::Rejection) -> String {
    use sim_core::blast::placement::Rejection;
    match e {
        Rejection::Outside => "Нельзя разместить: объект выходит за границу площадки".into(),
        Rejection::Terrain => "Нельзя разместить: пересечение с грунтом или постройкой".into(),
        Rejection::Units => "Нельзя разместить: пересечение с другим юнитом".into(),
        Rejection::Layout(reason) => format!("Недопустимое размещение: {reason}"),
    }
}
