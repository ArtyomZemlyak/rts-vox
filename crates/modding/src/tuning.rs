//! Числа тика — в данных, а не в исходнике.
//!
//! `CLAUDE.md`: «любое игровое число (стоимость, HP, урон, скорость,
//! прочность материала, тайминг) живёт в `data/*.ron`, а не в
//! исходнике». Урон от падения, период перевыбора цели, потолки работы
//! за тик и рост юнита — всё это тайминг и баланс, и до ADR-140 они
//! лежали литералами в `World::new` и в аргументах `Ground::new`.
//!
//! Файл здесь **один на партию**, а не на пак: это не контент, который
//! мод добавляет, а правила, по которым идёт тик. Мод, меняющий их,
//! меняет игру для всех участников lockstep, и решаться это будет
//! отдельно.

use serde::Deserialize;

/// Настройки тика. Полями обязаны быть заполнены все: `serde` без
/// `default` не простит пропуска, и это нарочно — забытое поле
/// не должно молча стать нулём.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct Tuning {
    /// Урона за воксель высоты падения (ADR-036). Первый воксель
    /// бесплатен — это правило, а не число, и живёт в коде (ADR-139).
    pub fall_damage_per_voxel: u16,
    /// Через сколько тиков юнит перевыбирает цель (ADR-116).
    pub retarget_period: u32,
    /// Потолок поисков цели за тик (ADR-114).
    pub max_target_searches: usize,
    /// Потолки работы над полем направлений за тик (ADR-138):
    /// клеток на сборку графа, узлов на просмотр, годуновских
    /// обновлений на развёртку.
    pub flow_cells_per_tick: u32,
    pub flow_scan_per_tick: u32,
    pub flow_updates_per_tick: u64,
    /// Рост юнита в вокселях: столько нужно свободного над полом.
    pub unit_height: u32,
    /// Сторона нав-клетки в вокселях (ADR-014).
    pub nav_cell: u32,
    /// Какой перепад юнит перешагивает; больше — обрыв, ребра нет
    /// (ADR-121).
    pub max_step: u32,
    /// Жёсткий потолок живых динамических тел (ADR-015).
    pub max_bodies: usize,
    /// Радиус расталкивания: ближе этого юниты расходятся.
    pub separation: i32,
    /// Дальность стрельбы в мировых единицах.
    pub range: i32,
}

#[derive(Debug)]
pub enum TuningError {
    Ron(String),
    Io(String),
}

impl std::fmt::Display for TuningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TuningError::Ron(e) => write!(f, "разбор RON: {e}"),
            TuningError::Io(e) => write!(f, "чтение файла: {e}"),
        }
    }
}

impl Tuning {
    /// Разобрать из текста. Без ввода-вывода — этим пользуется и тест,
    /// и загрузчик.
    pub fn from_ron(s: &str) -> Result<Tuning, TuningError> {
        ron::from_str(s).map_err(|e| TuningError::Ron(e.to_string()))
    }
}

/// Прочитать файл настроек тика.
pub fn load_tuning(path: &std::path::Path) -> Result<Tuning, TuningError> {
    let s = std::fs::read_to_string(path).map_err(|e| TuningError::Io(e.to_string()))?;
    Tuning::from_ron(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Пропущенное поле — ошибка, а не ноль.** Значение по умолчанию
    /// у числа тика означало бы, что оно всё-таки живёт в коде.
    #[test]
    fn a_missing_field_is_an_error() {
        let full = include_str!("../../../data/sim.ron");
        assert!(Tuning::from_ron(full).is_ok(), "полный файл не разобрался");
        let cut = full.replace("max_step:", "// max_step:");
        assert!(cut != full, "в файле нет поля max_step — тест ничего не режет");
        assert!(
            Tuning::from_ron(&cut).is_err(),
            "файл без max_step разобрался: у поля есть умолчание"
        );
    }

    /// Файл в репозитории — тот, по которому идёт игра, и он обязан
    /// быть осмысленным. Ноль высоты юнита или нулевая клетка означают
    /// деление на ноль в первом же тике.
    #[test]
    fn the_shipped_file_is_sane() {
        let t = Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("data/sim.ron");
        assert!(t.unit_height > 0 && t.nav_cell > 0);
        assert!(t.max_bodies > 0 && t.max_target_searches > 0);
        assert!(t.retarget_period > 0);
        assert!(t.separation > 0 && t.range > 0);
        assert!(t.flow_cells_per_tick > 0 && t.flow_scan_per_tick > 0);
        assert!(t.flow_updates_per_tick > 0);
    }
}
