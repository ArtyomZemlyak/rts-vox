//! Матч с экономикой на прежнем ядре `World` — сцена D-9 «База».
//!
//! Матч с конечными силами живёт на полигоне (`crate::range_match`,
//! ADR-670). Здесь остаётся партия, которой нужна экономика копки:
//! копатели, склады, завод. У полигона этих механик нет, и сцена ждёт
//! их переноса (ADR-662).
//!
//! **Ничья — законный исход, а не отсутствие исхода.** Карта, которую
//! копают, распадается: на D-7 замерено 4 419 компонент связности,
//! и юнит из воронки не выберется — пандуса он не роет, а перепад
//! больше посильного остаётся стеной (ADR-138, ADR-141). Значит две
//! стороны могут оказаться в разных компонентах и не встретиться
//! никогда. Матч, который не может кончиться, — это ошибка, которую
//! видно только прогоном, поэтому предел по тикам здесь есть, и его
//! достижение называется ничьей, а не «ещё идёт» (ADR-205).
//!
//! Собран целиком на публичном API `sim-core`: ни одной правки
//! в симуляции матч не потребовал.

use fx::rng::Rng;
use sim_core::ground::Ground;
use sim_core::World;
use voxel::World as VoxelWorld;
use worldgen::{Params, Terrain};

/// Из чего собирается матч.
#[derive(Clone, Copy)]
pub struct Setup {
    pub seed: u64,
    /// Сторона мира в вокселях.
    pub side: u32,
    /// Машин у каждой стороны.
    pub per_side: usize,
    /// Краулеров у каждой стороны. Копка — экономика, а не спецэффект
    /// (`CLAUDE.md`), и матч без неё показывал бы не ту игру.
    pub diggers: usize,
    /// Сколько тиков матч ждёт исхода, прежде чем объявить ничью.
    pub patience: u64,
    /// Через сколько тиков стороны перенацеливают поле направлений
    /// на **живого** противника. Ноль — не перенацеливать.
    pub hunt_period: u64,
    /// Через сколько тиков бот раздаёт приказы. Ноль — не раздавать:
    /// приказы отдаёт игрок.
    pub bot_period: u64,
}

/// База стороны: где стоит завод и куда ходят копать.
#[derive(Clone, Copy)]
pub struct Base {
    pub x: i32,
    pub z: i32,
    /// Слот завода; [`sim_core::NO_TARGET`] — завода нет.
    pub factory: u32,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            seed: 42,
            side: 192,
            per_side: 60,
            diggers: 24,
            patience: 6000,
            // Поле не бесплатно: решение идёт порциями и отстаёт
            // на десятки тиков (ADR-158). Перенацеливать каждый тик
            // значило бы решать его вхолостую, поэтому раз в десять
            // секунд игрового времени — 300 тиков при 30 Гц.
            hunt_period: 300,
            // Бот думает раз в секунду. Чаще незачем: приказ копать
            // занимает копателя на сотни тиков, а очередь завода —
            // на сотни тиков сборки.
            bot_period: 30,
        }
    }
}

/// Чем кончилось.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Ещё идёт.
    Fighting,
    /// Противник выбит подчистую.
    Wiped(u8),
    /// Срок вышел, машин больше у этой стороны.
    OnPoints(u8),
    /// Срок вышел при равном счёте, либо обе стороны погибли.
    Draw,
}

impl Outcome {
    pub fn winner(self) -> Option<u8> {
        match self {
            Outcome::Wiped(o) | Outcome::OnPoints(o) => Some(o),
            _ => None,
        }
    }

    pub fn over(self) -> bool {
        self != Outcome::Fighting
    }
}

pub struct Skirmish {
    pub world: World,
    pub terrain: Terrain,
    setup: Setup,
    /// Базы сторон.
    pub bases: [Base; 2],
    /// Живых по сторонам после последнего тика.
    pub alive: [usize; 2],
    pub tick: u64,
    /// Тик, на котором исход определился.
    pub decided_at: Option<u64>,
    /// Свой генератор у бота: приказы не имеют права зависеть
    /// от того, сколько раз к генератору мира обратился кто-то ещё.
    bot_rng: Rng,
}

/// Номера типов из `data/kinds.ron`. Явные, а не места в реестре
/// (ADR-028): место зависит от того, какие типы загружены.
pub const KIND_TANK: u16 = 1;
pub const KIND_CRAWLER: u16 = 2;
pub const KIND_WALKER: u16 = 3;
pub const KIND_FACTORY: u16 = 10;
pub const KIND_DEPOT: u16 = 11;

impl Skirmish {
    pub fn new(setup: Setup) -> Skirmish {
        let params = Params::sized(setup.side, setup.side / 2, setup.side);
        let terrain = Terrain {
            params,
            seed: setup.seed,
        };
        let t = crate::tuning();
        let vw = VoxelWorld::new(
            params.nx,
            params.ny,
            params.nz,
            1 << 16,
            Box::new(Terrain {
                params,
                seed: setup.seed,
            }),
        );
        // Копают настоящие краулеры, а не оснастка, выедающая карту мимо
        // учёта: копка это экономика, а не спецэффект (`CLAUDE.md`).
        let mut world = World::new(2, setup.per_side * 2 + setup.diggers * 2 + 16, t);
        world.attach_ground(Ground::new(vw, &t));
        world.attach_weapons(crate::weapons());
        world.attach_economy(crate::kinds(), crate::materials());

        // Базы по углам: одна в ближнем, другая в дальнем. Не по
        // середине сторон — так между ними лежит диагональ, то есть
        // самый длинный путь, который карта может дать, и навигация
        // работает, а не просто идёт по прямой.
        let (ex, ez) = (params.nx as i32, params.nz as i32);
        let margin = (ex / 8).clamp(4, 64);
        let bases = [(margin, margin), (ex - margin, ez - margin)];
        let mut r = Rng::new(setup.seed, 0x5A1D);
        let spread = (ex / 12).clamp(3, 40);
        let mut built: [Base; 2] = [Base {
            x: 0,
            z: 0,
            factory: sim_core::NO_TARGET,
        }; 2];
        for (owner, &(bx, bz)) in bases.iter().enumerate() {
            built[owner] = Base {
                x: bx,
                z: bz,
                factory: sim_core::NO_TARGET,
            };
            // База: завод и склад стоят готовыми. Строить первый
            // завод не из чего, а партия, которая не может
            // начаться, — не партия.
            let f = world.spawn_kind(bx, bz, owner as u8, KIND_FACTORY);
            built[owner].factory = f.unwrap_or(sim_core::NO_TARGET);
            let dx = (spread / 2).max(2);
            world.spawn_kind((bx + dx).clamp(0, ex - 1), bz, owner as u8, KIND_DEPOT);
            for _ in 0..setup.diggers {
                let x = (bx + r.range(-spread, spread)).clamp(0, ex - 1);
                let z = (bz + r.range(-spread, spread)).clamp(0, ez - 1);
                world.spawn_kind(x, z, owner as u8, KIND_CRAWLER);
            }
            for k in 0..setup.per_side {
                let x = (bx + r.range(-spread, spread)).clamp(0, ex - 1);
                let z = (bz + r.range(-spread, spread)).clamp(0, ez - 1);
                // Смесь **внутри** стороны: замерено, что сторона
                // с одним лучом теряет всех шестьдесят против
                // двенадцати потерь у стороны с болтом — луч
                // требует чистой прямой, а карту всё время копают
                // (ADR-205). Тип несёт оружие сам (ADR-230).
                let kind = if k % 2 == 0 { KIND_TANK } else { KIND_WALKER };
                world.spawn_kind(x, z, owner as u8, kind);
            }
        }

        // Каждый идёт к чужой базе.
        world.fill_flow_towards(0, bases[1].0, bases[1].1);
        world.fill_flow_towards(1, bases[0].0, bases[0].1);
        world.units.refresh_live();

        let mut m = Skirmish {
            world,
            terrain,
            setup,
            bases: built,
            alive: [setup.per_side; 2],
            tick: 0,
            decided_at: None,
            bot_rng: Rng::new(setup.seed, 0xB07),
        };
        m.count();
        m
    }

    /// Один тик матча. Возвращает хеш тика.
    pub fn step(&mut self) -> u64 {
        if self.setup.bot_period != 0 && self.tick.is_multiple_of(self.setup.bot_period) {
            self.bot();
        }
        let h = self.world.tick();
        self.tick += 1;
        self.count();
        if self.setup.hunt_period != 0 && self.tick.is_multiple_of(self.setup.hunt_period) {
            self.hunt();
        }
        if self.decided_at.is_none() && self.outcome().over() {
            self.decided_at = Some(self.tick);
        }
        h
    }

    /// Перенацелить поле каждой стороны на середину живого
    /// противника.
    ///
    /// Без этого матч не кончается **никогда**: поле ведёт к базе
    /// противника, а после первой сшибки выжившие расходятся, и обе
    /// стороны идут в пустые углы. Замерено: двадцать тысяч тиков
    /// на трёх сидах — ни одной победы, по десятку машин живо
    /// с каждой стороны (ADR-205).
    ///
    /// Середина считается **целочисленно**: сумма и деление, никаких
    /// дробей. Порядок обхода — по возрастанию номера юнита, значит
    /// результат не зависит ни от чего, кроме состояния.
    fn hunt(&mut self) {
        let mut sum = [(0i64, 0i64); 2];
        let mut n = [0i64; 2];
        for &i in self.world.units.live() {
            let i = i as usize;
            let o = self.world.units.owner[i] as usize;
            if o >= 2 {
                continue;
            }
            sum[o].0 += self.world.units.x[i] as i64;
            sum[o].1 += self.world.units.z[i] as i64;
            n[o] += 1;
        }
        for me in 0..2usize {
            let foe = 1 - me;
            if n[foe] == 0 {
                continue;
            }
            let (tx, tz) = ((sum[foe].0 / n[foe]) as i32, (sum[foe].1 / n[foe]) as i32);
            self.world.fill_flow_towards(me, tx, tz);
        }
    }

    /// Бот: занять простаивающих копателей и загрузить завод.
    ///
    /// **Собран целиком на канале приказов** — ни одной правки
    /// состояния напрямую. То же правило, что у мода и у окна
    /// (ADR-195): бот отличается от игрока только тем, что не щёлкает
    /// мышью.
    ///
    /// Порядок обхода — по возрастанию номера юнита, а жребий берётся
    /// из **своего** генератора: приказы не имеют права зависеть
    /// от того, сколько раз к генератору мира обратился кто-то ещё.
    fn bot(&mut self) {
        let (ex, ez) = (self.setup.side as i32, self.setup.side as i32);
        let spread = (ex / 10).clamp(4, 48);
        let mut orders: Vec<sim_core::Order> = Vec::new();

        // Запас подпирает потолок — значит пора строить склад.
        // Это единственное, что бот строит, и строит он по нужде,
        // а не по расписанию: постройка, которая ставится «просто так»,
        // не проверяет, что стройка вообще связана с экономикой.
        let mut want_depot = [false; 2];
        {
            let Some(e) = self.world.econ.as_ref() else {
                return;
            };
            // Недострой уже размечен — второй не нужен: класть
            // фундаменты пачкой это и есть отмывание материи,
            // от которого предостерегает `docs/07-economy.md`.
            let mut building = [false; 2];
            for &i in self.world.units.live() {
                let k = i as usize;
                if self.world.units.site[k] != 0 && (self.world.units.owner[k] as usize) < 2 {
                    building[self.world.units.owner[k] as usize] = true;
                }
            }
            for owner in 0..2usize {
                let cap = e.cap(owner as u8);
                let full = e.stock(owner as u8) >= cap / 4 * 3;
                want_depot[owner] = full && !building[owner];
            }
        }
        let mut placed = [false; 2];

        for &i in self.world.units.live() {
            let k = i as usize;
            let owner = self.world.units.owner[k] as usize;
            if owner >= 2 {
                continue;
            }
            let Some(e) = self.world.econ.as_ref() else {
                continue;
            };
            let ki = self.world.units.kind[k];
            if ki == sim_core::economy::NO_KIND {
                continue;
            }
            let d = e.kinds.at(ki as usize);
            // Простаивающий копатель идёт грызть забой у своей базы.
            // «Простаивающий» это без приказа и не в работе: копку,
            // возку и стройку бот не перебивает — иначе копатель
            // с полным баком разворачивался бы на полпути каждую
            // секунду.
            let idle = matches!(
                self.world.units.stance[k],
                sim_core::STANCE_FLOW | sim_core::STANCE_HOLD
            );
            // Свободный строитель ставит склад — по одному за раз.
            if idle && d.build_rate > 0 && want_depot[owner] && !placed[owner] {
                let b = self.bases[owner];
                let off = (spread / 2).max(3);
                let sign = if owner == 0 { 1 } else { -1 };
                let x = (b.x + sign * off).clamp(1, ex - 2);
                let z = (b.z + sign * off).clamp(1, ez - 2);
                orders.push(sim_core::Order::Build {
                    unit: i,
                    kind: KIND_DEPOT,
                    x,
                    z,
                });
                placed[owner] = true;
                continue;
            }
            if d.dig && idle {
                let b = self.bases[owner];
                let x = (b.x + self.bot_rng.range(-spread, spread)).clamp(1, ex - 2);
                let z = (b.z + self.bot_rng.range(-spread, spread)).clamp(1, ez - 2);
                // Забой — воксель **под** поверхностью: шар, снятый
                // с воздуха, не даёт ничего.
                let y = self.terrain.surface(x as u32, z as u32).saturating_sub(1) as i32;
                if y > 2 {
                    orders.push(sim_core::Order::Dig { unit: i, x, y, z });
                }
            }
        }
        // Завод: держать очередь непустой, пока запас позволяет.
        for owner in 0..2usize {
            let f = self.bases[owner].factory;
            if f == sim_core::NO_TARGET {
                continue;
            }
            let k = f as usize;
            if !self.world.units.alive[k] || self.world.units.site[k] != 0 {
                continue;
            }
            let Some(e) = self.world.econ.as_ref() else {
                continue;
            };
            let queued = e.line(f).map(|l| l.queue.len()).unwrap_or(0);
            if queued >= 2 {
                continue;
            }
            // Заказывать впритык нельзя: сборка тянет из того же
            // запаса, что стройка, и очередь встала бы, съев всё.
            let kind = if self.tick.is_multiple_of(2) {
                KIND_TANK
            } else {
                KIND_WALKER
            };
            let Some(def) = e.kinds.get(kind) else {
                continue;
            };
            if e.stock(owner as u8) >= def.cost.saturating_mul(2) {
                orders.push(sim_core::Order::Produce { unit: f, kind });
            }
        }
        for o in orders {
            self.world.push_order(o);
        }
    }

    fn count(&mut self) {
        self.alive = [0, 0];
        for &i in self.world.units.live() {
            let o = self.world.units.owner[i as usize] as usize;
            if o < 2 {
                self.alive[o] += 1;
            }
        }
    }

    /// Чем кончилось — или `Fighting`, если ещё нет.
    ///
    /// **У матча есть срок, и по сроку побеждает тот, у кого больше
    /// машин.** Это не украшение: замерено, что одной охоты мало —
    /// без неё не кончается ни одна партия, а с ней примерно
    /// половина, и какая именно, решает сид. Остаются одиночки,
    /// до которых не дойти: карта распадается на компоненты, а из
    /// воронки юнит не выберется (ADR-138, ADR-141).
    ///
    /// Настоящее лечение — научить юнита выбираться, и оно записано
    /// как несделанное. Срок же делает **каждую** партию решённой,
    /// а это условие того, чтобы в неё вообще можно было играть
    /// (ADR-205).
    pub fn outcome(&self) -> Outcome {
        match (self.alive[0], self.alive[1]) {
            (0, 0) => Outcome::Draw,
            (0, _) => Outcome::Wiped(1),
            (_, 0) => Outcome::Wiped(0),
            (a, b) if self.tick >= self.setup.patience => match a.cmp(&b) {
                std::cmp::Ordering::Greater => Outcome::OnPoints(0),
                std::cmp::Ordering::Less => Outcome::OnPoints(1),
                std::cmp::Ordering::Equal => Outcome::Draw,
            },
            _ => Outcome::Fighting,
        }
    }

    /// Рельеф: высота поверхности по каждой клетке.
    ///
    /// Нужно, чтобы отличить «карта такая» от «это выел бой». Вопрос
    /// не праздный: у генератора ступени в один воксель, и на пологом
    /// склоне ступень тянется длинной канавой — на глаз неотличимо
    /// от траншеи (ADR-207).
    pub fn surface(&self) -> Vec<u32> {
        let g = self.world.ground().expect("земля");
        let (nx, ny, nz) = (g.world.nx, g.world.ny, g.world.nz);
        let mut out = Vec::with_capacity((nx * nz) as usize);
        for z in 0..nz {
            for x in 0..nx {
                let mut h = 0u32;
                for y in (0..ny).rev() {
                    if g.world.solid(x, y, z) {
                        h = y + 1;
                        break;
                    }
                }
                out.push(h);
            }
        }
        out
    }

    /// Крутить до исхода. Возвращает исход и хеш последнего тика.
    pub fn play_out(&mut self) -> (Outcome, u64) {
        let mut h = 0u64;
        while !self.outcome().over() {
            h = self.step();
        }
        (self.outcome(), h)
    }
}
