//! Матч на полигоне: две стороны, конечные силы, исход (ADR-205, ADR-670).
//!
//! **Чем это отличается от эталонного сценария.** Эталон гоняет бой
//! фиксированное число тиков ради хеша; матч крутится, **пока не придёт
//! исход**, и исход — его результат. Силы конечны: расстановка берётся
//! из данных, подкреплений нет, потери невосполнимы.
//!
//! **Исход боя решает симуляция, срок — матч.** Полигон сам объявляет
//! победу, поражение и ничью: сторона без живых проиграла, уничтоженный
//! командный пункт проигрывает и при живой броне, неподвижные гарнизоны
//! вне досягаемости друг друга — ничья. Матч добавляет к этому только
//! правило состязания: срок, по истечении которого побеждает сторона
//! с большим числом живых, а при равенстве — ничья (`docs/07-economy.md`,
//! «Завершение партии»). Матч, который не может кончиться, — ошибка,
//! видимая только прогоном, поэтому срок есть всегда.
//!
//! Сторона 0 — своя сторона раскладки (`allied`), сторона 1 — противник.
//! Обеими сторонами по умолчанию командует ИИ правил (`data/battle-ai.ron`).
//!
//! Собран на публичном API `sim-core`; файлы читает здесь вызывающий,
//! симуляция получает разобранные данные (`CLAUDE.md`, границы слоёв).

use modding::explosion::ExplosionConfig;
use modding::range::RangeLayout;
use sim_core::blast::battle::Phase;
use sim_core::blast::BlastRange;

/// Расстановка матча — один из боёв полигона в `data/`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    /// Бой всей карты, `battle-world.ron` — тот же, что у эталона.
    World,
    /// Прорыв укреплений, `skirmish-breakthrough.ron`.
    Breakthrough,
    /// Клещи, `skirmish-pincer.ron`.
    Pincer,
    /// Пауки, `skirmish-spider.ron`.
    Spider,
    /// Огневые точки, `skirmish-turrets.ron`.
    Turrets,
    /// Картечь, `skirmish-canister.ron`.
    Canister,
}

impl Preset {
    pub const ALL: [Preset; 6] = [
        Preset::World,
        Preset::Breakthrough,
        Preset::Pincer,
        Preset::Spider,
        Preset::Turrets,
        Preset::Canister,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Preset::World => "world",
            Preset::Breakthrough => "breakthrough",
            Preset::Pincer => "pincer",
            Preset::Spider => "spider",
            Preset::Turrets => "turrets",
            Preset::Canister => "canister",
        }
    }

    pub fn parse(name: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.name() == name)
    }

    fn file(self) -> &'static str {
        match self {
            Preset::World => "battle-world.ron",
            Preset::Breakthrough => "skirmish-breakthrough.ron",
            Preset::Pincer => "skirmish-pincer.ron",
            Preset::Spider => "skirmish-spider.ron",
            Preset::Turrets => "skirmish-turrets.ron",
            Preset::Canister => "skirmish-canister.ron",
        }
    }
}

/// Из чего собирается матч.
#[derive(Clone, Copy, Debug)]
pub struct Setup {
    pub preset: Preset,
    /// Сколько тиков матч ждёт исхода боя, прежде чем судить по счёту.
    pub patience: u64,
    /// Командует ли ИИ правил и своей стороной. Выключен — своя сторона
    /// стоит на приказах раскладки, и маневрирует только противник.
    pub ai_vs_ai: bool,
    /// Воркеры физики, 1..=8. Исход и хеш от них не зависят, только время.
    pub workers: usize,
    /// Строить всю карту полигона, а не её угол до участка боя.
    ///
    /// Рельеф полигона задан мировыми координатами, поэтому угол карты
    /// от начала до участка — тот же рельеф, что у полной карты, а
    /// собирается он на порядок быстрее. Бой всей карты строит её
    /// целиком всегда.
    pub whole_map: bool,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            preset: Preset::Breakthrough,
            // Пять минут игрового времени при 30 Гц.
            patience: 9000,
            ai_vs_ai: true,
            workers: 1,
            whole_map: false,
        }
    }
}

/// Чем кончилось.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Ещё идёт.
    Fighting,
    /// Бой решён правилами симуляции: противник выбит или потерял
    /// командный пункт.
    Won(u8),
    /// Срок вышел, живых больше у этой стороны.
    OnPoints(u8),
    /// Ничья по правилам боя, гибель обеих сторон или равный счёт
    /// по сроку.
    Draw,
}

impl Outcome {
    pub fn winner(self) -> Option<u8> {
        match self {
            Outcome::Won(o) | Outcome::OnPoints(o) => Some(o),
            _ => None,
        }
    }

    pub fn over(self) -> bool {
        self != Outcome::Fighting
    }
}

/// Текст файла из `data/`: путь от места сборки, `./data` — запасной.
fn data(name: &str) -> Result<String, String> {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data")
        .join(name);
    let path = if here.exists() {
        here
    } else {
        std::path::PathBuf::from("data").join(name)
    };
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Поле от начала карты до участка боя с запасом: в нём остаются
/// участки, дороги, постройки и укрытия, целиком лежащие внутри.
fn field(mut l: RangeLayout, zone: [i32; 2]) -> RangeLayout {
    // Сторона карты кратна шестнадцати (`RangeLayout::validate`); запас —
    // две полосы по шестьдесят четыре клетки, чтобы край не касался боя.
    let side = |v: i32| ((v + 128 + 15) / 16 * 16) as u32;
    l.size = [side(zone[0]), l.size[1], side(zone[1])];
    let s = l.size.map(|v| v as i32);
    let inside = |p: [i32; 2]| p[0] >= 0 && p[1] >= 0 && p[0] < s[0] && p[1] < s[2];
    l.zones.retain(|z| z.max[0] <= s[0] && z.max[1] <= s[2]);
    l.roads.retain(|r| r.points.iter().all(|p| inside(*p)));
    l.blocks.retain(|b| b.max[0] <= s[0] && b.max[2] <= s[2]);
    l.shields.retain(|b| inside([b.at[0], b.at[2]]));
    if let Some(c) = &mut l.course {
        c.ramps.retain(|b| b.max[0] <= s[0] && b.max[2] <= s[2]);
        c.max = [c.max[0].min(s[0]), c.max[1].min(s[2])];
    }
    l
}

/// Собрать полигон матча и начать бой.
fn range(setup: Setup) -> Result<BlastRange, String> {
    let Setup {
        preset,
        whole_map,
        ai_vs_ai,
        ..
    } = setup;
    let config = ExplosionConfig::from_ron(&data("explosion.ron")?)
        .map_err(|e| format!("explosion.ron: {e}"))?;
    let mut layout =
        RangeLayout::from_ron(&data("range-km.ron")?).map_err(|e| format!("range-km.ron: {e}"))?;
    let file = preset.file();
    layout.battle = true;
    if preset == Preset::World {
        let p = modding::range_skirmish::WorldBattle::from_ron(&data(file)?)
            .map_err(|e| format!("{file}: {e}"))?;
        if ai_vs_ai {
            // Ровно эталонный сценарий: его собирает `sim-core`, и только
            // он (ADR-157, ADR-662).
            return BlastRange::world_battle(config, layout, p);
        }
        layout.units = p.units;
        layout.blocks.extend(p.blocks);
    } else {
        // Сценарий заменяет штатный боевой участок карты, как в окне
        // полигона: карта остаётся одной и связной.
        let p = modding::range_skirmish::Skirmish::from_ron(&data(file)?)
            .map_err(|e| format!("{file}: {e}"))?;
        let stock = modding::range_battle::Tuning::from_ron(&data("range-battle.ron")?)
            .map_err(|e| format!("range-battle.ron: {e}"))?;
        let zone = layout
            .zones
            .iter()
            .position(|z| z.name == stock.zone.name)
            .ok_or("range-km.ron: нет штатного боевого участка")?;
        let corner = p.zone.max;
        layout.units = p.units;
        layout.gun = p.zone.spawn;
        layout.target = p.zone.target;
        layout.blocks.extend(p.blocks);
        layout.zones[zone] = p.zone;
        if !whole_map {
            layout = field(layout, corner);
        }
    }
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    let mut sim = BlastRange::with_layout(config, layout)?;
    if ai_vs_ai && !sim.set_ai_vs_ai(true) {
        return Err("ИИ обеих сторон не включился".into());
    }
    sim.start_battle()?;
    Ok(sim)
}

pub struct Match {
    pub sim: BlastRange,
    setup: Setup,
    /// Тик, на котором исход определился.
    pub decided_at: Option<u64>,
}

impl Match {
    pub fn new(setup: Setup) -> Result<Match, String> {
        let mut sim = range(setup)?;
        if !sim.set_physics_workers(setup.workers) {
            return Err(format!("воркеров физики {}: допустимо 1..8", setup.workers));
        }
        Ok(Match {
            sim,
            setup,
            decided_at: None,
        })
    }

    pub fn setup(&self) -> Setup {
        self.setup
    }

    /// Тик симуляции; бой начат на нулевом.
    pub fn tick(&self) -> u64 {
        u64::from(self.sim.tick)
    }

    /// Живых по сторонам: постройки и огневые точки считаются наравне
    /// с машинами.
    pub fn alive(&self) -> [usize; 2] {
        self.sim.battle_counts()
    }

    /// Выстрелов каждой стороны с начала боя.
    pub fn fired(&self) -> [u32; 2] {
        [true, false].map(|side| {
            (0..self.sim.units.len())
                .filter(|&id| self.sim.allied(id) == side)
                .map(|id| self.sim.battle.actors[id].fired)
                .sum()
        })
    }

    /// Погибших с начала боя.
    pub fn dead(&self) -> usize {
        self.sim.units.iter().filter(|u| !u.status.alive).count()
    }

    /// Один тик матча. После исхода симуляция дальше не идёт.
    pub fn step(&mut self) {
        if self.outcome().over() {
            return;
        }
        self.sim.step();
        if self.decided_at.is_none() && self.outcome().over() {
            self.decided_at = Some(self.tick());
        }
    }

    /// Чем кончилось — или `Fighting`, если ещё нет.
    pub fn outcome(&self) -> Outcome {
        match self.sim.battle.phase {
            Phase::Victory => return Outcome::Won(0),
            Phase::Defeat => return Outcome::Won(1),
            Phase::Draw => return Outcome::Draw,
            Phase::Setup | Phase::Fighting => {}
        }
        if self.tick() < self.setup.patience {
            return Outcome::Fighting;
        }
        let [a, b] = self.alive();
        match a.cmp(&b) {
            std::cmp::Ordering::Greater => Outcome::OnPoints(0),
            std::cmp::Ordering::Less => Outcome::OnPoints(1),
            std::cmp::Ordering::Equal => Outcome::Draw,
        }
    }

    /// Крутить до исхода. Возвращает исход и хеш состояния в конце.
    pub fn play_out(&mut self) -> (Outcome, u64) {
        while !self.outcome().over() {
            self.step();
        }
        (self.outcome(), self.sim.state_hash())
    }
}
