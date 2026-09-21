//! Мод-API: реестр данных.
//!
//! Пока здесь только то, от чего зависит **детерминизм**, — реестр
//! материалов с явными индексами. Загрузчик RON и хост Luau появятся
//! вместе с файлами данных; их формат — отдельное решение с зависимостями.
//!
//! **Индекс материала задаётся явно в данных и никогда не выводится
//! из порядка загрузки** (ADR-028, `CLAUDE.md`). Загрузчик «по умолчанию»
//! делает наоборот и ломает все сохранения разом, поэтому здесь этого
//! нельзя сделать даже случайно: индекс — обязательное поле, а не
//! позиция в списке.

#![forbid(unsafe_code)]

pub mod data;
pub mod explosion;
pub mod heat;
pub mod integrity;
pub mod kinds;
pub mod navigation;
pub mod range;
pub mod range_weapon;
pub mod shield;
pub mod skirmish;
pub mod tuning;
pub mod turret;
pub mod vox;
pub mod weapons;

pub use data::{load_pack, merge_materials, LoadError, Pack, UnitDef, UnitModel};
pub use kinds::{KindDef, Kinds};
pub use tuning::{load_tuning, Chassis, ChassisKind, Tuning, TuningError};

pub type Mat = u16;

/// Роль воксела в теле юнита — **в данных**.
///
/// Дубликат `voxel::unit::Part` намеренный: `modding` описывает то, что
/// лежит в файле, и не имеет права зависеть от движка. Полнота отображения
/// одного в другое проверяется тестом на стороне движка, а не надеждой.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum PartTag {
    Armour,
    Core,
    Engine,
    Weapon,
    Sensor,
}

/// Физические свойства материала. Всё, что влияет на симуляцию.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct MatDef {
    /// Явный номер. Не позиция в списке.
    pub id: Mat,
    /// Плотность: ресурс равен объёму на плотность (`CLAUDE.md`).
    /// Богатая руда — материал с большей плотностью, а не модификатор.
    pub density: u32,
    /// Прочность: сколько урона держит воксель.
    pub strength: u32,
    /// Наносит ли урон тому, кто его касается (лава, ADR-100).
    pub harmful: bool,
    /// Тепло, которое горящий воксель отдаёт каждому соседу за время
    /// горения, в единицах энергии оружия; ноль — не горит (ADR-277).
    #[serde(default)]
    pub fuel: u32,
    /// Тепло, при котором воксель загорается; ноль — никогда.
    #[serde(default)]
    pub kindling: u32,
    /// Тепло, при котором негорючий воксель оплавляется и стекает
    /// обломком (ADR-290); ноль — никогда.
    #[serde(default)]
    pub melt: u32,
    /// Сыпучий: оставшись без опоры, рассыпается кусками; связный
    /// (бетон, сталь, ствол, камень) падает целым телом (ADR-300).
    #[serde(default)]
    pub loose: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// Два определения на один номер — ошибка загрузки, а не «последний
    /// побеждает»: молчаливое переопределение ломает сохранения.
    Duplicate(Mat),
    /// Номер вне диапазона палитры (ADR-094: u16).
    OutOfRange(Mat),
}

/// Плотный реестр: номер это индекс, дыры допустимы.
pub struct Registry {
    defs: Vec<Option<MatDef>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry { defs: Vec::new() }
    }

    pub fn insert(&mut self, d: MatDef) -> Result<(), RegistryError> {
        let i = d.id as usize;
        if i >= u16::MAX as usize {
            return Err(RegistryError::OutOfRange(d.id));
        }
        if self.defs.len() <= i {
            self.defs.resize(i + 1, None);
        }
        if self.defs[i].is_some() {
            return Err(RegistryError::Duplicate(d.id));
        }
        self.defs[i] = Some(d);
        Ok(())
    }

    pub fn get(&self, m: Mat) -> Option<&MatDef> {
        self.defs.get(m as usize).and_then(|o| o.as_ref())
    }

    /// Заменить определение материала с тем же номером (ADR-298: живые
    /// настройки полигона). Номер обязан уже быть в реестре.
    pub fn replace(&mut self, d: MatDef) -> Result<(), RegistryError> {
        match self.defs.get_mut(d.id as usize) {
            Some(slot) if slot.is_some() => {
                *slot = Some(d);
                Ok(())
            }
            _ => Err(RegistryError::OutOfRange(d.id)),
        }
    }

    pub fn len(&self) -> usize {
        self.defs.iter().filter(|o| o.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Хеш набора материалов. Входит в хеш партии: мод, добавивший
    /// материал, меняет её (ADR-097).
    ///
    /// Обход по возрастанию номера, а не по порядку вставки, — иначе
    /// два клиента с разным порядком загрузки получили бы разный хеш
    /// при одинаковом наборе.
    pub fn hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for (i, d) in self.defs.iter().enumerate() {
            let Some(d) = d else { continue };
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(d.id as u64);
            w = w.wrapping_mul(31).wrapping_add(d.density as u64);
            w = w.wrapping_mul(31).wrapping_add(d.strength as u64);
            w = w.wrapping_mul(31).wrapping_add(d.harmful as u64);
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        h
    }
}

impl Default for Registry {
    fn default() -> Registry {
        Registry::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: Mat, density: u32) -> MatDef {
        MatDef {
            id,
            density,
            strength: 10,
            harmful: false,
            fuel: 0,
            kindling: 0,
            melt: 0,
            loose: false,
        }
    }

    #[test]
    fn duplicate_id_is_an_error_not_an_override() {
        let mut r = Registry::new();
        r.insert(d(17, 100)).unwrap();
        assert_eq!(r.insert(d(17, 200)), Err(RegistryError::Duplicate(17)));
        assert_eq!(r.get(17).unwrap().density, 100);
    }

    /// Порядок загрузки не влияет на хеш: иначе два клиента с одним
    /// набором модов, но разным порядком, разошлись бы на первом тике.
    #[test]
    fn hash_does_not_depend_on_load_order() {
        let items = [d(1, 5), d(40, 9), d(17, 100), d(3, 1)];
        let mut a = Registry::new();
        for x in items {
            a.insert(x).unwrap();
        }
        let mut b = Registry::new();
        for x in items.iter().rev() {
            b.insert(*x).unwrap();
        }
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn different_set_different_hash() {
        let mut a = Registry::new();
        a.insert(d(1, 5)).unwrap();
        let mut b = Registry::new();
        b.insert(d(1, 6)).unwrap();
        assert_ne!(a.hash(), b.hash());
    }
}
