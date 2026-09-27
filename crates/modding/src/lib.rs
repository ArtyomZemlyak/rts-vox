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

pub mod battle_ai;
pub mod data;
pub mod explosion;
pub mod heat;
pub mod integrity;
pub mod kinds;
pub mod navigation;
pub mod range;
pub mod range_battle;
pub mod range_fire;
pub mod range_weapon;
pub mod shield;
pub mod skirmish;
pub mod spider;
pub mod tuning;
pub mod turret;
pub mod unit_heat;
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
    /// Maximum self-supporting plate span per thickness under gravity.
    /// Zero uses the strength-derived legacy estimate (ADR-373).
    #[serde(default)]
    pub bend_span: u32,
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
    /// Свойство вне границ (`docs/09-modding.md`, ADR-429): плотность 0
    /// даёт бесплатную породу, прочность 0 — снятие воксела за тик.
    Degenerate(Mat),
}

/// Границы свойств материала (ADR-429). Воздух (номер 0) — пустота,
/// у него обе нули; у любого другого материала обе от единицы до
/// границы. Входят в хеш набора: клиенты с разными границами разные.
pub const MAX_DENSITY: u32 = 100_000;
pub const MAX_STRENGTH: u32 = 100_000;

impl MatDef {
    /// Свойства в границах (ADR-429).
    fn sane(&self) -> bool {
        if self.id == 0 {
            return self.density == 0 && self.strength == 0;
        }
        (1..=MAX_DENSITY).contains(&self.density) && (1..=MAX_STRENGTH).contains(&self.strength)
    }
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
        if !d.sane() {
            return Err(RegistryError::Degenerate(d.id));
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
        if !d.sane() {
            return Err(RegistryError::Degenerate(d.id));
        }
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
    /// при одинаковом наборе. FNV-1a по каждому полю, влияющему на
    /// симуляцию, и по границам проверки (ADR-429): взвешенная сумма
    /// прежней редакции теряла поля и сходилась у встречных правок.
    pub fn hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |v: u64| {
            for b in v.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(u64::from(MAX_DENSITY));
        eat(u64::from(MAX_STRENGTH));
        for d in self.defs.iter().flatten() {
            let MatDef {
                id,
                density,
                strength,
                bend_span,
                harmful,
                fuel,
                kindling,
                melt,
                loose,
            } = *d;
            for v in [
                u64::from(id),
                u64::from(density),
                u64::from(strength),
                u64::from(bend_span),
                u64::from(harmful),
                u64::from(fuel),
                u64::from(kindling),
                u64::from(melt),
                u64::from(loose),
            ] {
                eat(v);
            }
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
            bend_span: 0,
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

    /// Плотность 0 и прочность 0 у материала — ошибка загрузки, а не
    /// бесплатная порода и снятие воксела за тик (ADR-429); воздух —
    /// пустота, у него обе нули и только нули.
    #[test]
    fn a_degenerate_material_is_refused() {
        let mut r = Registry::new();
        assert_eq!(r.insert(d(5, 0)), Err(RegistryError::Degenerate(5)));
        let mut soft = d(6, 100);
        soft.strength = 0;
        assert_eq!(r.insert(soft), Err(RegistryError::Degenerate(6)));
        assert_eq!(
            r.insert(d(7, MAX_DENSITY + 1)),
            Err(RegistryError::Degenerate(7))
        );
        let mut air = d(0, 0);
        air.strength = 0;
        r.insert(air).unwrap();
        assert_eq!(r.insert(d(0, 5)), Err(RegistryError::Duplicate(0)));
        let mut heavy_air = Registry::new();
        assert_eq!(heavy_air.insert(d(0, 5)), Err(RegistryError::Degenerate(0)));
        r.insert(d(8, 100)).unwrap();
        assert_eq!(r.replace(d(8, 0)), Err(RegistryError::Degenerate(8)));
        assert_eq!(r.get(8).unwrap().density, 100);
    }

    /// Хеш видит каждое поле: прежний не видел горения и оплавления,
    /// а встречные правки двух плотностей сходились в сумме (ADR-429).
    #[test]
    fn every_field_moves_the_hash() {
        let base = || {
            let mut r = Registry::new();
            r.insert(d(1, 120)).unwrap();
            r.insert(d(2, 310)).unwrap();
            r
        };
        let h = base().hash();
        let fields: [fn(&mut MatDef); 6] = [
            |m| m.fuel = 1,
            |m| m.kindling = 1,
            |m| m.melt = 1,
            |m| m.loose = true,
            |m| m.bend_span = 1,
            |m| m.harmful = true,
        ];
        for change in fields {
            let mut r = base();
            let mut m = *r.get(1).unwrap();
            change(&mut m);
            r.replace(m).unwrap();
            assert_ne!(r.hash(), h);
        }
        // Soil +2 with deep rock -1 kept the old weighted sum.
        let mut r = Registry::new();
        r.insert(d(1, 122)).unwrap();
        r.insert(d(2, 309)).unwrap();
        assert_ne!(r.hash(), h);
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

/// Authored battles for the live range HUD.
pub mod range_skirmish;
