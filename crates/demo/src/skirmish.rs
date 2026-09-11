//! Матч: две стороны, фиксированные силы, кто-то побеждает.
//!
//! **Чем это отличается от эталонного сценария.** Тот каждый тик
//! добавляет подкрепления и потому не кончается никогда — так и надо
//! нагрузочному гейту, где важна ровная нагрузка тысячу тиков подряд.
//! Игре нужно обратное: силы конечны, потери невосполнимы, и матч
//! обязан прийти к исходу.
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
    /// Копателей на карте. Копка — экономика, а не спецэффект
    /// (`CLAUDE.md`), и матч без неё показывал бы не ту игру.
    pub diggers: usize,
    /// Сколько тиков матч ждёт исхода, прежде чем объявить ничью.
    pub patience: u64,
    /// Через сколько тиков стороны перенацеливают поле направлений
    /// на **живого** противника. Ноль — не перенацеливать.
    pub hunt_period: u64,
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
    dig: crate::dig::Dig,
    setup: Setup,
    /// Живых по сторонам после последнего тика.
    pub alive: [usize; 2],
    pub tick: u64,
    /// Тик, на котором исход определился.
    pub decided_at: Option<u64>,
}

impl Skirmish {
    pub fn new(setup: Setup) -> Skirmish {
        let params = Params::sized(setup.side, setup.side / 2, setup.side);
        let terrain = Terrain { params, seed: setup.seed };
        let t = crate::tuning();
        let vw = VoxelWorld::new(
            params.nx,
            params.ny,
            params.nz,
            1 << 16,
            Box::new(Terrain { params, seed: setup.seed }),
        );
        let dig = crate::dig::Dig::clustered(
            &vw,
            |x, z| terrain.surface(x, z),
            setup.diggers,
            2,
            4,
            setup.seed ^ 0x5C1,
        );
        let mut world = World::new(2, setup.per_side * 2 + 8, t);
        world.attach_ground(Ground::new(vw, &t));
        world.attach_weapons(crate::weapons());

        // Базы по углам: одна в ближнем, другая в дальнем. Не по
        // середине сторон — так между ними лежит диагональ, то есть
        // самый длинный путь, который карта может дать, и навигация
        // работает, а не просто идёт по прямой.
        let (ex, ez) = (params.nx as i32, params.nz as i32);
        let margin = (ex / 8).clamp(4, 64);
        let bases = [(margin, margin), (ex - margin, ez - margin)];
        let mut r = Rng::new(setup.seed, 0x5A1D);
        let spread = (ex / 12).clamp(3, 40);
        for (owner, &(bx, bz)) in bases.iter().enumerate() {
            for _ in 0..setup.per_side {
                let x = (bx + r.range(-spread, spread)).clamp(0, ex - 1);
                let z = (bz + r.range(-spread, spread)).clamp(0, ez - 1);
                world.spawn_ground(x, z, owner as u8, r.range(2, 6), 300);
            }
        }
        // Оружие раздаётся **явным номером вида**, как в эталонном
        // сценарии (ADR-192): без этого все остаются со слотом ноль,
        // то есть с болтом, и луч в матче не стреляет ни разу.
        //
        // Смесь **внутри** стороны, а не по сторонам. Гейт раздаёт
        // по владельцу, и это верно для нагрузки: так оба вида
        // работают под нагрузкой. Игре так нельзя — замерено, что
        // сторона с одним лучом теряет всех шестьдесят против
        // двенадцати потерь у стороны с болтом: луч требует чистой
        // прямой, а карту всё время копают (ADR-205).
        for k in 0..world.units.len() {
            let id = sim_core::REF_WEAPON_IDS[k % 2];
            world.units.weapon[k] = world
                .weapons
                .index_of(id)
                .unwrap_or_else(|| panic!("вид оружия {id} не загружен")) as u16;
        }

        // Каждый идёт к чужой базе.
        world.fill_flow_towards(0, bases[1].0, bases[1].1);
        world.fill_flow_towards(1, bases[0].0, bases[0].1);
        world.units.refresh_live();

        let mut m = Skirmish {
            world,
            terrain,
            dig,
            setup,
            alive: [setup.per_side; 2],
            tick: 0,
            decided_at: None,
        };
        m.count();
        m
    }

    /// Один тик матча. Возвращает хеш тика.
    pub fn step(&mut self) -> u64 {
        {
            let g = self.world.ground_mut().expect("земля");
            let terrain = &self.terrain;
            self.dig.step(&mut g.world, &|x, z| terrain.surface(x, z));
        }
        let h = self.world.tick();
        self.tick += 1;
        self.count();
        if self.setup.hunt_period != 0 && self.tick % self.setup.hunt_period == 0 {
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
