//! Оружие — данные, а не код.
//!
//! Вид оружия задаёт, **как** снимается урон: болт летит и тратит тики,
//! луч приходит в тот же тик. Числа — скорость, урон, перезарядка,
//! воронка — у каждого свои (ADR-169).
//!
//! **Номер задаётся явно**, как у материалов (ADR-028). Позиция в списке
//! ничего не значит: переставить строки можно, поменять номер — значит
//! сломать все сохранения и всё, что на них стоит. Столкновение номеров —
//! ошибка загрузки, а не «последний выиграл».

use serde::Deserialize;

/// Как оружие доставляет урон.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum WeaponKind {
    /// Снаряд: летит с конечной скоростью, тратит тики на дорогу,
    /// упирается в породу (ADR-164).
    Bolt,
    /// Луч: приходит в тот же тик по той же линии огня. Породу
    /// не прожигает — упёрся, значит не попал.
    Laser,
}

/// Описание оружия из данных.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct WeaponDef {
    pub id: u16,
    pub kind: WeaponKind,
    /// Скорость снаряда, клеток в секунду. У луча не читается.
    pub speed: i32,
    /// Урон одного попадания, вокселей.
    pub damage: u16,
    /// Тиков между выстрелами.
    pub reload: u32,
    /// Радиус воронки от попадания, вокселей. Ноль — воронки нет.
    pub blast_radius: i32,
    /// Урон всем в радиусе воронки.
    pub blast_damage: u16,
    /// Physical projectile mass and directed energy released on impact.
    /// Zero denotes a legacy, non-material benchmark weapon.
    #[serde(default)]
    pub mass_g: u32,
    #[serde(default)]
    pub impact_energy_j: u32,
    /// Chemical/field energy for fracture and debris, separate from direct impact.
    #[serde(default)]
    pub blast_energy_j: u32,
    /// Weapon reach in world units. Zero uses the benchmark's global range.
    #[serde(default)]
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub range: i32,
}

/// Largest crater radius a weapon may declare, in voxels (ADR-429).
pub const MAX_BLAST_RADIUS: i32 = 128;

#[derive(Deserialize)]
pub struct WeaponsFile {
    pub weapons: Vec<WeaponDef>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WeaponError {
    /// Номер занят. Два оружия с одним номером — это разное оружие
    /// под одним именем в сохранении.
    Clash(u16),
    /// Числа, при которых оружие не стреляет.
    Nonsense(u16),
}

impl std::fmt::Display for WeaponError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WeaponError::Clash(id) => write!(f, "номер оружия {id} занят"),
            WeaponError::Nonsense(id) => write!(f, "оружие {id}: числа не дают выстрела"),
        }
    }
}

/// Реестр оружия: плотный массив с поиском по номеру.
///
/// Массив, а не карта: поиск идёт на каждом выстреле, а оружия десятки.
#[derive(Default, Clone, Debug)]
pub struct Weapons {
    defs: Vec<WeaponDef>,
}

impl Weapons {
    pub fn new() -> Weapons {
        Weapons::default()
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn all(&self) -> &[WeaponDef] {
        &self.defs
    }

    /// Добавить. Номер обязан быть свободным, числа — осмысленными.
    pub fn insert(&mut self, d: WeaponDef) -> Result<(), WeaponError> {
        if self.defs.iter().any(|x| x.id == d.id) {
            return Err(WeaponError::Clash(d.id));
        }
        // The crater is a sphere walked cell by cell (ADR-429): its
        // radius is bounded for every weapon, not only for one whose
        // blast carries energy, or a bare radius overflows the squared
        // distance and walks billions of cells on every hit.
        if d.damage == 0
            || d.reload == 0
            || !(0..=MAX_BLAST_RADIUS).contains(&d.blast_radius)
            || d.range < 0
            || d.range > 10000
        {
            return Err(WeaponError::Nonsense(d.id));
        }
        if d.blast_energy_j > 0 && (d.blast_radius == 0 || d.kind != WeaponKind::Bolt) {
            return Err(WeaponError::Nonsense(d.id));
        }
        if d.kind == WeaponKind::Bolt && d.speed <= 0 {
            return Err(WeaponError::Nonsense(d.id));
        }
        self.defs.push(d);
        // Порядок по номеру: обход реестра не должен зависеть от того,
        // в каком порядке грузились паки.
        self.defs.sort_unstable_by_key(|x| x.id);
        Ok(())
    }

    /// Оружие по номеру.
    pub fn get(&self, id: u16) -> Option<&WeaponDef> {
        self.defs.iter().find(|x| x.id == id)
    }

    /// Место в плотном массиве по номеру — для горячего пути.
    pub fn index_of(&self, id: u16) -> Option<usize> {
        self.defs.iter().position(|x| x.id == id)
    }

    pub fn at(&self, i: usize) -> &WeaponDef {
        &self.defs[i]
    }
}

/// Разобрать файл оружия.
pub fn from_ron(s: &str) -> Result<Weapons, String> {
    let f: WeaponsFile = ron::from_str(s).map_err(|e| e.to_string())?;
    let mut w = Weapons::new();
    for d in f.weapons {
        w.insert(d).map_err(|e| e.to_string())?;
    }
    if w.is_empty() {
        return Err("в реестре нет ни одного оружия".to_string());
    }
    Ok(w)
}

/// Прочитать файл оружия с диска.
pub fn load(path: &std::path::Path) -> Result<Weapons, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Столкновение номеров — ошибка загрузки.**
    ///
    /// То же правило, что у материалов (ADR-028): «последний выиграл»
    /// ломает сохранения молча.
    #[test]
    fn a_clashing_id_is_refused() {
        let src = r#"( weapons: [
            ( id: 1, kind: Bolt, speed: 40, damage: 30, reload: 30, blast_radius: 2, blast_damage: 10 ),
            ( id: 1, kind: Laser, speed: 0, damage: 12, reload: 8, blast_radius: 0, blast_damage: 0 ),
        ] )"#;
        let e = from_ron(src).expect_err("столкновение номеров прошло");
        assert!(e.contains("занят"), "{e}");
    }

    /// **Порядок реестра — по номеру, а не по порядку загрузки.**
    #[test]
    fn order_is_by_id_not_by_load_order() {
        let src = r#"( weapons: [
            ( id: 9, kind: Laser, speed: 0, damage: 12, reload: 8, blast_radius: 0, blast_damage: 0 ),
            ( id: 1, kind: Bolt, speed: 40, damage: 30, reload: 30, blast_radius: 2, blast_damage: 10 ),
        ] )"#;
        let w = from_ron(src).expect("разбор");
        assert_eq!(w.all()[0].id, 1);
        assert_eq!(w.all()[1].id, 9);
    }

    /// **Оружие, которое не стреляет, — ошибка.** Нулевая перезарядка
    /// это выстрел каждый тик, нулевой урон — вечный бой.
    #[test]
    fn nonsense_numbers_are_refused() {
        for bad in [
            "( id: 1, kind: Bolt, speed: 40, damage: 0, reload: 30, blast_radius: 0, blast_damage: 0 )",
            "( id: 1, kind: Bolt, speed: 40, damage: 5, reload: 0, blast_radius: 0, blast_damage: 0 )",
            "( id: 1, kind: Bolt, speed: 0, damage: 5, reload: 5, blast_radius: 0, blast_damage: 0 )",
            // A crater without blast energy is still walked cell by cell
            // (ADR-429): 50000 overflowed its squared radius.
            "( id: 1, kind: Bolt, speed: 40, damage: 5, reload: 5, blast_radius: 50000, blast_damage: 1 )",
            "( id: 1, kind: Bolt, speed: 40, damage: 5, reload: 5, blast_radius: 129, blast_damage: 1 )",
        ] {
            let src = format!("( weapons: [ {bad} ] )");
            assert!(from_ron(&src).is_err(), "принято: {bad}");
        }
    }

    /// **Луч без скорости — законен.** Ему она не нужна.
    #[test]
    fn a_laser_needs_no_speed() {
        let src = r#"( weapons: [
            ( id: 2, kind: Laser, speed: 0, damage: 12, reload: 8, blast_radius: 0, blast_damage: 0 ),
        ] )"#;
        let w = from_ron(src).expect("луч без скорости отвергнут");
        assert_eq!(w.at(0).kind, WeaponKind::Laser);
    }
}
