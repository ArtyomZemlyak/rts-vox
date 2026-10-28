//! Palette choice is authoring state, separate from the applied stand recipe.
use super::*;
impl Constructor {
    pub fn material(&self, d: &Definition, r: Recipe) -> u16 {
        self.workspace
            .selected_material
            .unwrap_or(d.materials[r.material].id)
    }
    pub fn open_materials(&mut self, d: &Definition) {
        if self.mode == Mode::Place && self.selected.is_some() {
            self.mode = Mode::Select;
            self.sync_material(d);
        }
    }
    pub fn material_name(&self, d: &Definition, r: Recipe) -> String {
        material_name(d, self.material(d, r))
    }
    pub fn material_editing(&self) -> Option<usize> {
        matches!(self.mode, Mode::Select | Mode::Move)
            .then_some(self.selected)
            .flatten()
            .filter(|&i| i < self.placed.len())
    }
    fn material_stand(&self) -> bool {
        self.mode == Mode::Select && self.selected.is_none() && self.stand && self.base.is_none()
    }
    pub fn can_apply_material(&self) -> bool {
        self.material_editing().is_some_and(|i| {
            let e = &self.placed[i].element;
            e.blocks.iter().any(|b| b.material != 0) && e.units.is_empty() && e.shields.is_empty()
        }) || self.material_stand()
    }
    pub fn sync_material(&mut self, d: &Definition) {
        if let Some(id) = self
            .selected
            .and_then(|i| self.placed.get(i))
            .and_then(|p| p.element.blocks.iter().find(|b| b.material != 0))
            .map(|b| b.material)
            .filter(|id| d.materials.iter().any(|m| m.id == *id))
        {
            self.workspace.selected_material = Some(id);
            self.preview_key = None;
            self.snap_shape = None;
        }
    }
    pub fn pick_material(&mut self, id: u16, d: &Definition) -> Result<(bool, String), String> {
        if !d.materials.iter().any(|m| m.id == id) {
            return Err("Материал отсутствует в палитре".into());
        }
        self.workspace.selected_material = Some(id);
        self.preview_key = None;
        self.snap_shape = None;
        if self.voxel_editing() {return Ok((false,format!("Материал кисти кубиков: {} · вернитесь в «Кубики»",material_name(d,id))));}
        if self.mode==Mode::Live {return Ok((false,format!("Материал живой кисти: {} · вернитесь во вкладку «Инструмент»",material_name(d,id))));}
        Ok((false,format!("Выбран материал «{}». Нажмите «Применить»; для кисти и новых деталей он используется сразу",material_name(d,id))))
    }
    pub fn apply_material(
        &mut self,
        d: &Definition,
        r: &mut Recipe,
    ) -> Result<(bool, String), String> {
        if !self.can_apply_material() {
            return Err("Выберите блок или сборку в сцене. Для юнита материал обшивки задаётся его шаблоном".into());
        }
        let id = self.material(d, *r);
        if self.material_stand() {
            r.material = d
                .materials
                .iter()
                .position(|m| m.id == id)
                .ok_or("Неизвестный материал")?;
            self.strength = None;
            self.revision += 1;
            return Ok((true, format!("Материал стенда: {}", material_name(d, id))));
        }
        let i = self.material_editing().unwrap();
        let mut next = self.placed.clone();
        for b in &mut next[i].element.blocks {
            if b.material != 0 {
                b.material = id;
            }
        }
        self.commit(next, d, *r)?;
        Ok((
            true,
            format!(
                "«{}»: материал блоков — {}",
                self.placed[i].element.name,
                material_name(d, id)
            ),
        ))
    }
    pub fn material_context(&self, d: &Definition, r: Recipe) -> String {
        if self.voxel_editing() {return format!("Материал кисти кубиков: {}\nВыберите материал и вернитесь во вкладку «Кубики». Он изменит только затронутые клетки детали",self.material_name(d,r));}
        if self.mode==Mode::Live {return format!("Материал живой кисти: {}\nВыберите материал и вернитесь во вкладку «Инструмент». Он будет добавляться при удержании ЛКМ",self.material_name(d,r));}
        if let Some(i) = self.material_editing() {
            let e = &self.placed[i].element;
            if !e.shields.is_empty() && e.units.is_empty() {
                return format!("Выбрано: {}\nЩит — поле без материальных блоков. Для замены материала выберите блок или постройку",e.name);
            }
            if !e.units.is_empty() {
                return format!("Выбрано: {}\nМатериалы юнита заданы шаблоном. Здесь можно менять свойства выбранного типа материала для всей сцены",e.name);
            }
            let ids: std::collections::BTreeSet<_> = e
                .blocks
                .iter()
                .filter(|b| b.material != 0)
                .map(|b| b.material)
                .collect();
            let names = ids
                .into_iter()
                .map(|id| material_name(d, id))
                .collect::<Vec<_>>()
                .join(", ");
            let count = e.blocks.iter().filter(|b| b.material != 0).count();
            let scope = if count == 1 {
                "Изменится выбранный блок".into()
            } else {
                format!("Замена всех блоков: {count}. Для одной части — «Разобрать сборку»")
            };
            return format!("Цель: {}\nСейчас: {}\n{}", e.name, names, scope);
        }
        if self.material_stand() {
            return format!(
                "Цель: параметрический стенд\nСейчас: {}",
                d.materials[r.material].name
            );
        }
        "Выберите объект в сцене или в составе сцены слева. При размещении палитра задаёт материал новой детали и кисти".into()
    }
}
fn material_name(d: &Definition, id: u16) -> String {
    if id == 12 {
        return "Стекло".into();
    }
    d.materials
        .iter()
        .find(|m| m.id == id)
        .map_or_else(|| format!("Материал #{id}"), |m| m.name.clone())
}
