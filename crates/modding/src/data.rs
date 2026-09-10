//! Загрузка контента: RON плюс `.vox`.
//!
//! **Ванильный контент и мод — один и тот же путь.** `data/` это такой
//! же пак, как `mods/что-нибудь/`, и грузится тем же `load_pack`.
//! Если бы у ванили был свой путь, «мод работает как ванильный контент»
//! проверялось бы только на словах, а расходились бы они на первом же
//! поле, которое загрузчик мода не умеет.
//!
//! Формат — RON (ADR-008): человекочитаемый, с типами, без отступов
//! как синтаксиса.

use crate::{MatDef, PartTag, Registry, RegistryError};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum LoadError {
    Io(String, std::io::Error),
    Ron(String, String),
    Registry(String, RegistryError),
    Vox(String, crate::vox::VoxError),
    /// Путь из данных выходит за пределы пака. Пак приходит от игрока,
    /// и `../../../etc/passwd` в поле `model` — это не опечатка.
    Escape(String),
    /// Ядро вне тома: юнит без ядра неубиваем (`docs/05-units.md`).
    CoreOutside(String),
    /// В томе нет ни одного воксела ядра.
    NoCore(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(w, e) => write!(f, "{w}: {e}"),
            LoadError::Ron(w, e) => write!(f, "{w}: разбор RON: {e}"),
            LoadError::Registry(w, e) => write!(f, "{w}: реестр: {e:?}"),
            LoadError::Vox(w, e) => write!(f, "{w}: разбор .vox: {e:?}"),
            LoadError::Escape(w) => write!(f, "{w}: путь выходит за пределы пака"),
            LoadError::CoreOutside(w) => write!(f, "{w}: ядро вне тома"),
            LoadError::NoCore(w) => write!(f, "{w}: в томе нет ядра"),
        }
    }
}

/// Файл материалов пака.
#[derive(Debug, Deserialize)]
pub struct MaterialsFile {
    pub materials: Vec<MatDef>,
}

/// Юнит как он описан в данных. Ни строчки кода: модель, раскраска,
/// ядро.
#[derive(Debug, Deserialize)]
pub struct UnitDef {
    pub name: String,
    /// Путь к `.vox` относительно корня пака.
    pub model: String,
    /// Что означает каждый номер цвета модели.
    pub palette: Vec<(u8, PartTag)>,
    /// Размер куба ядра в вокселях. Само положение ядра берётся из
    /// модели: это воксели, помеченные `Core`.
    pub core_size: u8,
}

/// Юнит, готовый к сборке в движковый том.
#[derive(Debug, Clone)]
pub struct UnitModel {
    pub name: String,
    pub nx: u8,
    pub ny: u8,
    pub nz: u8,
    pub core_size: u8,
    /// `(x, y, z, метка)`. Пустые воксели отсутствуют.
    pub voxels: Vec<(u8, u8, u8, PartTag)>,
}

impl UnitModel {
    pub fn solid_count(&self) -> usize {
        self.voxels.len()
    }
}

/// Загруженный пак: набор материалов и набор юнитов.
pub struct Pack {
    pub name: String,
    pub materials: Vec<MatDef>,
    pub units: Vec<UnitModel>,
}

/// Разрешить путь из данных внутри пака. Ни `..`, ни корня, ни
/// префикса диска.
fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, LoadError> {
    let r = Path::new(rel);
    for c in r.components() {
        match c {
            Component::Normal(_) => {}
            _ => return Err(LoadError::Escape(rel.to_string())),
        }
    }
    Ok(root.join(r))
}

fn read_file(p: &Path) -> Result<Vec<u8>, LoadError> {
    std::fs::read(p).map_err(|e| LoadError::Io(p.display().to_string(), e))
}

/// Загрузить пак из каталога.
///
/// Обход каталога с юнитами идёт **по отсортированному списку имён**,
/// а не в порядке файловой системы: порядок каталога различается между
/// машинами, а порядок юнитов задаёт их номера в флоте.
pub fn load_pack(dir: &Path) -> Result<Pack, LoadError> {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string());

    let mats_path = dir.join("materials.ron");
    let materials = if mats_path.exists() {
        let text = String::from_utf8_lossy(&read_file(&mats_path)?).into_owned();
        let f: MaterialsFile = ron::from_str(&text)
            .map_err(|e| LoadError::Ron(mats_path.display().to_string(), e.to_string()))?;
        f.materials
    } else {
        Vec::new()
    };

    let units_dir = dir.join("units");
    let mut ron_files: Vec<PathBuf> = Vec::new();
    if units_dir.is_dir() {
        let rd = std::fs::read_dir(&units_dir)
            .map_err(|e| LoadError::Io(units_dir.display().to_string(), e))?;
        for e in rd {
            let e = e.map_err(|e| LoadError::Io(units_dir.display().to_string(), e))?;
            let p = e.path();
            if p.extension().map(|x| x == "ron").unwrap_or(false) {
                ron_files.push(p);
            }
        }
    }
    ron_files.sort();

    let mut units = Vec::new();
    for p in ron_files {
        let where_ = p.display().to_string();
        let text = String::from_utf8_lossy(&read_file(&p)?).into_owned();
        let def: UnitDef =
            ron::from_str(&text).map_err(|e| LoadError::Ron(where_.clone(), e.to_string()))?;
        let model_path = safe_join(dir, &def.model)?;
        let bytes = read_file(&model_path)?;
        let vm = crate::vox::read(&bytes)
            .map_err(|e| LoadError::Vox(model_path.display().to_string(), e))?;
        units.push(build_unit(&where_, def, vm)?);
    }

    Ok(Pack { name, materials, units })
}

/// Собрать модель юнита из описания и `.vox`.
///
/// Положение ядра здесь **не считается**. Оно выводится из разметки уже
/// на стороне движка (`UnitBody::seat_core`), и это единственное место,
/// где правило живёт: пока их было два — число в коде и разметка
/// в данных, — зашитый танк и загруженный получали разные ядра, оба
/// верные и разные (ADR-115). Загрузчик проверяет только, что ядро
/// вообще есть: юнит без ядра неубиваем.
fn build_unit(where_: &str, def: UnitDef, vm: crate::vox::VoxModel) -> Result<UnitModel, LoadError> {
    if vm.nx > u8::MAX as u32 || vm.ny > u8::MAX as u32 || vm.nz > u8::MAX as u32 {
        return Err(LoadError::Vox(
            where_.to_string(),
            crate::vox::VoxError::Size(vm.nx, vm.ny, vm.nz),
        ));
    }
    let mut voxels = Vec::with_capacity(vm.voxels.len());
    let mut has_core = false;
    for &(x, y, z, colour) in &vm.voxels {
        let tag = crate::vox::tag_at(&def.palette, colour);
        has_core |= tag == PartTag::Core;
        voxels.push((x, y, z, tag));
    }
    if !has_core {
        return Err(LoadError::NoCore(where_.to_string()));
    }
    let cs = def.core_size.max(1);
    if cs as u32 > vm.nx || cs as u32 > vm.ny || cs as u32 > vm.nz {
        return Err(LoadError::CoreOutside(where_.to_string()));
    }
    // Порядок вокселей задаёт порядок сборки тома, значит и всё, что
    // из него считается. Файл этот порядок задавать не должен.
    voxels.sort_by_key(|&(x, y, z, _)| (z, y, x));
    Ok(UnitModel {
        name: def.name,
        nx: vm.nx as u8,
        ny: vm.ny as u8,
        nz: vm.nz as u8,
        core_size: cs,
        voxels,
    })
}

/// Сложить паки в один реестр материалов. Порядок паков задаёт только
/// сообщение об ошибке: столкновение номеров — ошибка загрузки, а не
/// «последний побеждает» (ADR-028).
pub fn merge_materials(packs: &[Pack]) -> Result<Registry, LoadError> {
    let mut r = Registry::new();
    for p in packs {
        for m in &p.materials {
            r.insert(*m)
                .map_err(|e| LoadError::Registry(p.name.clone(), e))?;
        }
    }
    Ok(r)
}
