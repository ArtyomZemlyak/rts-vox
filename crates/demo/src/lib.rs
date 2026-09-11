//! Сцены техно-демки (`docs/14-demo-scenes.md`).
//!
//! Сцена — не уровень и не ролик. У каждой есть **сид, длина в тиках,
//! headless-режим с хешем и порог**, при превышении которого она
//! считается провалённой. Сцена без порога — ролик, а не проверка.
//!
//! Здесь только то, что принадлежит сценам: копатели и хеш прогона.
//! Мосты от мира к подсистемам уехали в свои крейты (ADR-124) —
//! это движок, а не сцена.

#![forbid(unsafe_code)]

pub mod dig;
pub mod skirmish;

/// Числа тика — из `data/sim.ron`, как и в игре (ADR-140).
///
/// Сцена запускается из корня репозитория, но полагаться на текущий
/// каталог нельзя: путь берётся от места сборки, а `./data` остаётся
/// запасным. Литералов здесь нет и быть не должно — ради этого файл
/// и заведён.
pub fn tuning() -> sim_core::Tuning {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/sim.ron");
    let path = if here.exists() { here } else { std::path::PathBuf::from("data/sim.ron") };
    match modding::load_tuning(&path) {
        Ok(t) => t,
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

/// Реестр оружия — из `data/weapons.ron`, как и в игре (ADR-169).
///
/// Сцена, у которой он не привязан, **боя не ведёт вовсе**: `fire()`
/// выходит первой строкой, и ни выстрела, ни луча, ни воронки, ни
/// смерти от боя в ней не случается. Молча (ADR-187).
pub fn weapons() -> modding::weapons::Weapons {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/weapons.ron");
    let path = if here.exists() { here } else { std::path::PathBuf::from("data/weapons.ron") };
    match modding::weapons::load(&path) {
        Ok(w) => w,
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

/// Генератор мира. Живёт в `worldgen`; здесь реэкспорт, чтобы сцены
/// не тянули имя из двух мест.
pub use worldgen::Terrain;

/// Хеш прогона сцены. Только `wrapping_*` — та же ассоциативная пара,
/// что у хеша тика (`CLAUDE.md`).
#[derive(Clone, Copy)]
pub struct RunHash(u64);

impl Default for RunHash {
    fn default() -> RunHash {
        RunHash::new()
    }
}

impl RunHash {
    pub fn new() -> RunHash {
        RunHash(0xcbf2_9ce4_8422_2325)
    }

    /// Подмешать правки тика. Порядок внутри тика уже детерминирован
    /// дренажем очереди (FIFO), поэтому позиция в списке участвует
    /// в хеше — она значима.
    pub fn feed_edits(&mut self, tick: u64, edits: &[voxel::Edit]) {
        for (i, e) in edits.iter().enumerate() {
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(e.x as u64);
            w = w.wrapping_mul(31).wrapping_add(e.y as u64);
            w = w.wrapping_mul(31).wrapping_add(e.z as u64);
            w = w.wrapping_mul(31).wrapping_add(e.mat as u64);
            self.0 = self
                .0
                .wrapping_add(w.wrapping_mul(tick.wrapping_mul(1_000_003) + i as u64 + 1));
        }
    }

    pub fn feed(&mut self, v: u64) {
        self.0 = self.0.wrapping_add(v.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    }

    pub fn get(self) -> u64 {
        self.0
    }
}
