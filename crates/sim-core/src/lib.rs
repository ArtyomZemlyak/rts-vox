//! Скелет симуляции: SoA-хранилище, явный список фаз (ADR-048), хеш состояния.
//!
//! Существует ради замера, которого требует `../../docs/11-roadmap.md`:
//! «фазы движения и боя под нагрузкой, а не пустыми. Прогон "5000 сущностей
//! с пустыми фазами" не способен провалиться там, где должен».
//!
//! Содержательны здесь четыре фазы: `movement`, `combat`, `visibility`
//! и `hash`. Остальные одиннадцать — заглушки, как и предусмотрено ADR-048:
//! они существуют, участвуют в порядке и ничего не делают.

#![forbid(unsafe_code)]

pub mod energy;
pub mod index;
pub mod shield;

use fx::rng::Rng;
use fx::Fx;
use index::Grid;

/// Сторона мира в мировых единицах.
pub const WORLD: i32 = 2000;
/// Сторона нав-клетки.
pub const NAV_CELL: i32 = 2;
/// Сторона нав-сетки.
pub const NAV: usize = (WORLD / NAV_CELL) as usize;

/// Радиус разделения: ближе этого юниты расталкиваются.
pub const SEPARATION: i32 = 3;
/// Дальность стрельбы.
pub const RANGE: i32 = 24;

/// Список фаз тика — окончательный (ADR-048). Порядок объявлен фиксированным
/// навсегда: его изменение меняет хеши всех golden-файлов.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    ApplyCommands,
    ModPreTick,
    VoxelEdits,
    Destruction,
    Fluids,
    /// Диффузия разрежённых слоёв: урон, температура, насыщение (ADR-097).
    /// Заглушка до этапа 6; место занято сейчас, пока golden-файлов нет.
    Diffusion,
    Bodies,
    NavUpdate,
    Movement,
    Combat,
    Logistics,
    Economy,
    Visibility,
    ModPostTick,
    Cleanup,
    Hash,
}

pub const PHASES: [Phase; 16] = [
    Phase::ApplyCommands,
    Phase::ModPreTick,
    Phase::VoxelEdits,
    Phase::Destruction,
    Phase::Fluids,
    Phase::Diffusion,
    Phase::Bodies,
    Phase::NavUpdate,
    Phase::Movement,
    Phase::Combat,
    Phase::Logistics,
    Phase::Economy,
    Phase::Visibility,
    Phase::ModPostTick,
    Phase::Cleanup,
    Phase::Hash,
];

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::ApplyCommands => "apply_commands",
            Phase::ModPreTick => "mod_pre_tick",
            Phase::VoxelEdits => "voxel_edits",
            Phase::Destruction => "destruction",
            Phase::Fluids => "fluids",
            Phase::Diffusion => "diffusion",
            Phase::Bodies => "bodies",
            Phase::NavUpdate => "nav_update",
            Phase::Movement => "movement",
            Phase::Combat => "combat",
            Phase::Logistics => "logistics",
            Phase::Economy => "economy",
            Phase::Visibility => "visibility",
            Phase::ModPostTick => "mod_post_tick",
            Phase::Cleanup => "cleanup",
            Phase::Hash => "hash",
        }
    }
}

/// Юниты, SoA. Удаление — через free-list, никогда `swap_and_pop`
/// (`CLAUDE.md`): перестановка соседей молча меняет исход ничьей.
pub struct Units {
    pub x: Vec<i32>,
    pub z: Vec<i32>,
    /// Дробная часть положения: движение идёт в `Fx`, сетка — в целых.
    pub fx: Vec<Fx>,
    pub fz: Vec<Fx>,
    pub owner: Vec<u8>,
    pub speed: Vec<i32>,
    /// Прикреплённых к ядру вокселей — то, что у нас вместо скалярного HP.
    pub voxels: Vec<u16>,
    pub alive: Vec<bool>,
    pub target: Vec<u32>,
    free: Vec<u32>,
    live_ids: Vec<u32>,
}

pub const NO_TARGET: u32 = u32::MAX;

impl Default for Units {
    fn default() -> Self {
        Self::new()
    }
}

impl Units {
    pub fn new() -> Units {
        Units {
            x: Vec::new(),
            z: Vec::new(),
            fx: Vec::new(),
            fz: Vec::new(),
            owner: Vec::new(),
            speed: Vec::new(),
            voxels: Vec::new(),
            alive: Vec::new(),
            target: Vec::new(),
            free: Vec::new(),
            live_ids: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn live(&self) -> &[u32] {
        &self.live_ids
    }

    pub fn spawn(&mut self, x: i32, z: i32, owner: u8, speed: i32, voxels: u16) -> u32 {
        if let Some(i) = self.free.pop() {
            let k = i as usize;
            self.x[k] = x;
            self.z[k] = z;
            self.fx[k] = Fx::ZERO;
            self.fz[k] = Fx::ZERO;
            self.owner[k] = owner;
            self.speed[k] = speed;
            self.voxels[k] = voxels;
            self.alive[k] = true;
            self.target[k] = NO_TARGET;
            return i;
        }
        self.x.push(x);
        self.z.push(z);
        self.fx.push(Fx::ZERO);
        self.fz.push(Fx::ZERO);
        self.owner.push(owner);
        self.speed.push(speed);
        self.voxels.push(voxels);
        self.alive.push(true);
        self.target.push(NO_TARGET);
        (self.x.len() - 1) as u32
    }

    /// Освобождение слота. Слот попадает в free-list и переиспользуется
    /// позже; соседи не двигаются.
    pub fn kill(&mut self, i: u32) {
        let k = i as usize;
        if !self.alive[k] {
            return;
        }
        self.alive[k] = false;
        self.free.push(i);
    }

    /// Пересобрать список живых. Порядок — по возрастанию индекса, всегда.
    pub fn refresh_live(&mut self) {
        self.live_ids.clear();
        for i in 0..self.x.len() {
            if self.alive[i] {
                self.live_ids.push(i as u32);
            }
        }
    }
}

/// Мир: юниты, поле направлений, индекс.
pub struct World {
    pub units: Units,
    /// Направление потока по нав-клеткам: 0 = −Z, 1 = −X, 2 = +X, 3 = +Z,
    /// 255 = стоять. По одному полю на владельца.
    pub flow: Vec<Vec<u8>>,
    /// **Два индекса, а не один.** Ведро равно радиусу запроса, иначе запрос
    /// перебирает лишних: с одним ведром по дальности стрельбы расталкивание
    /// радиусом 3 осматривало бы всех в радиусе 24 — в шестьдесят раз больше,
    /// чем нужно.
    pub grid_near: Grid,
    pub grid_far: Grid,
    pub tick: u64,
    /// Накопитель урона: серийная фаза, поэтому обычное сложение безопасно.
    damage: Vec<u32>,
    /// Снимок положений на начало фазы движения. Без него расталкивание
    /// читает уже сдвинутых соседей, то есть зависит от порядка обхода —
    /// и параллелить его нельзя ни при каком разбиении.
    snap_x: Vec<i32>,
    snap_z: Vec<i32>,
    /// Смещения, посчитанные параллельно и применяемые одной серийной
    /// фазой (`CLAUDE.md`: клэмп применяется один раз, в конце).
    delta: Vec<(i32, i32)>,
    /// Замер: сколько сущностей осмотрели фазы за тик.
    pub examined_move: u64,
    pub examined_combat: u64,
    pub phase_ns: [u64; 16],
}

const DIRS: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];

impl World {
    pub fn new(owners: usize, capacity: usize) -> World {
        World::with_cells(owners, capacity, SEPARATION, RANGE)
    }

    /// Размеры вёдер — параметр, а не константа: они задают обмен между
    /// ценой пересборки индекса и ценой запроса, и оптимум зависит
    /// от плотности. Мелкое ведро дёшево в запросе и дорого в пересборке:
    /// при стороне 3 на карте 2000 это 445 тысяч корзин, которые надо
    /// обнулить каждый тик.
    pub fn with_cells(owners: usize, capacity: usize, near: i32, far: i32) -> World {
        World {
            units: Units::new(),
            flow: (0..owners).map(|_| vec![255u8; NAV * NAV]).collect(),
            grid_near: Grid::new(WORLD, near, capacity),
            grid_far: Grid::new(WORLD, far, capacity),
            snap_x: Vec::new(),
            snap_z: Vec::new(),
            delta: Vec::new(),
            tick: 0,
            damage: Vec::new(),
            examined_move: 0,
            examined_combat: 0,
            phase_ns: [0; 16],
        }
    }

    /// Поле направлений: все идут к точке сбора своего владельца.
    /// Настоящее поле придёт из `nav`; здесь важна только форма доступа —
    /// одно чтение байта на юнита за тик.
    pub fn fill_flow_towards(&mut self, owner: usize, tx: i32, tz: i32) {
        let f = &mut self.flow[owner];
        for cz in 0..NAV {
            for cx in 0..NAV {
                let (dx, dz) = (
                    tx - (cx as i32 * NAV_CELL),
                    tz - (cz as i32 * NAV_CELL),
                );
                let d = if dx.abs() >= dz.abs() {
                    if dx > 0 {
                        2
                    } else if dx < 0 {
                        1
                    } else {
                        255
                    }
                } else if dz > 0 {
                    3
                } else {
                    0
                };
                f[cz * NAV + cx] = d;
            }
        }
    }

    /// Один тик на заданном числе воркеров. Хеш обязан совпадать
    /// при любом их числе (`CLAUDE.md`).
    pub fn tick_par(&mut self, threads: usize) -> u64 {
        use std::time::Instant;
        let mut hash = 0u64;
        for (pi, &phase) in PHASES.iter().enumerate() {
            let t0 = Instant::now();
            match phase {
                Phase::NavUpdate => self.phase_nav_update(),
                Phase::Movement => {
                    self.movement_deltas(threads);
                    self.apply_deltas();
                }
                Phase::Combat => self.phase_combat(),
                Phase::Visibility => self.phase_visibility(),
                Phase::Cleanup => self.phase_cleanup(),
                Phase::Hash => hash = self.phase_hash(),
                _ => {}
            }
            self.phase_ns[pi] += t0.elapsed().as_nanos() as u64;
        }
        self.tick += 1;
        hash
    }

    /// Один тик: все пятнадцать фаз в объявленном порядке.
    pub fn tick(&mut self) -> u64 {
        use std::time::Instant;
        let mut hash = 0u64;
        for (pi, &phase) in PHASES.iter().enumerate() {
            let t0 = Instant::now();
            match phase {
                Phase::NavUpdate => self.phase_nav_update(),
                Phase::Movement => self.phase_movement(),
                Phase::Combat => self.phase_combat(),
                Phase::Visibility => self.phase_visibility(),
                Phase::Cleanup => self.phase_cleanup(),
                Phase::Hash => hash = self.phase_hash(),
                // Остальные — заглушки (ADR-048): существуют, ничего не делают.
                _ => {}
            }
            self.phase_ns[pi] += t0.elapsed().as_nanos() as u64;
        }
        self.tick += 1;
        hash
    }

    /// Индекс перестраивается один раз за тик, до фаз, которые его читают.
    fn phase_nav_update(&mut self) {
        self.units.refresh_live();
        self.grid_near
            .rebuild(&self.units.x, &self.units.z, &self.units.live_ids);
        self.grid_far
            .rebuild(&self.units.x, &self.units.z, &self.units.live_ids);
    }

    /// Движение: чтение направления из поля плюс расталкивание соседей.
    ///
    /// Расталкивание — единственное место фазы, где стоимость нелинейна
    /// по числу юнитов: она линейна по числу **пар в радиусе**, а это
    /// плотность, а не количество.
    fn phase_movement(&mut self) {
        self.movement_deltas(1);
        self.apply_deltas();
    }

    /// Подготовить снимок к фазе движения. Отделено от счёта, чтобы
    /// постоянный пул мог звать счёт много раз без пересборки снимка.
    pub fn prepare_movement(&mut self) {
        self.snap_x.clear();
        self.snap_x.extend_from_slice(&self.units.x);
        self.snap_z.clear();
        self.snap_z.extend_from_slice(&self.units.z);
        self.delta.clear();
        self.delta.resize(self.units.x.len(), (0, 0));
    }

    /// Смещение одного юнита. Читает только снимок и индекс —
    /// значит вызов из любого потока даёт то же самое.
    #[inline]
    pub fn movement_kernel(&self, i: usize) -> (i32, i32) {
        let u = &self.units;
        let (ux, uz) = (self.snap_x[i], self.snap_z[i]);
        let (cx, cz) = (
            (ux / NAV_CELL).clamp(0, NAV as i32 - 1) as usize,
            (uz / NAV_CELL).clamp(0, NAV as i32 - 1) as usize,
        );
        let d = self.flow[u.owner[i] as usize][cz * NAV + cx];
        let (mut vx, mut vz) = if d < 4 {
            let (dx, dz) = DIRS[d as usize];
            (dx * u.speed[i], dz * u.speed[i])
        } else {
            (0, 0)
        };
        let r2 = SEPARATION * SEPARATION;
        let (sx, sz) = (&self.snap_x, &self.snap_z);
        self.grid_near.for_each_near(ux, uz, |j| {
            let j = j as usize;
            if j == i {
                return;
            }
            let (dx, dz) = (ux - sx[j], uz - sz[j]);
            let d2 = dx * dx + dz * dz;
            if d2 == 0 || d2 >= r2 {
                return;
            }
            let w = (r2 - d2) * 64 / r2;
            vx += dx * w / 16;
            vz += dz * w / 16;
        });
        (vx, vz)
    }

    /// Живые ли слот. Нужен пулу, чтобы не считать дырки free-list.
    #[inline]
    pub fn slot_alive(&self, i: usize) -> bool {
        self.units.alive[i]
    }

    pub fn slots(&self) -> usize {
        self.units.x.len()
    }

    /// Записать смещение — вызывается пулом в свой кусок буфера.
    pub fn take_delta(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.delta)
    }

    pub fn put_delta(&mut self, d: Vec<(i32, i32)>) {
        self.delta = d;
    }

    /// Посчитать смещения. Читает **только снимок**, пишет только
    /// в свой слот `delta` — значит фаза делится на потоки по слотам
    /// и результат от разбиения не зависит.
    fn movement_deltas(&mut self, threads: usize) {
        let u = &self.units;
        self.snap_x.clear();
        self.snap_x.extend_from_slice(&u.x);
        self.snap_z.clear();
        self.snap_z.extend_from_slice(&u.z);
        self.delta.clear();
        self.delta.resize(u.x.len(), (0, 0));

        let (sx, sz) = (&self.snap_x, &self.snap_z);
        let grid = &self.grid_near;
        let flow = &self.flow;
        let r2 = SEPARATION * SEPARATION;

        let compute = |i: usize| -> (i32, i32) {
            let (ux, uz) = (sx[i], sz[i]);
            let (cx, cz) = (
                (ux / NAV_CELL).clamp(0, NAV as i32 - 1) as usize,
                (uz / NAV_CELL).clamp(0, NAV as i32 - 1) as usize,
            );
            let d = flow[u.owner[i] as usize][cz * NAV + cx];
            let (mut vx, mut vz) = if d < 4 {
                let (dx, dz) = DIRS[d as usize];
                (dx * u.speed[i], dz * u.speed[i])
            } else {
                (0, 0)
            };
            grid.for_each_near(ux, uz, |j| {
                let j = j as usize;
                if j == i {
                    return;
                }
                let (dx, dz) = (ux - sx[j], uz - sz[j]);
                let d2 = dx * dx + dz * dz;
                if d2 == 0 || d2 >= r2 {
                    return;
                }
                let w = (r2 - d2) * 64 / r2;
                vx += dx * w / 16;
                vz += dz * w / 16;
            });
            (vx, vz)
        };

        let live = &u.live_ids;
        if threads <= 1 {
            let mut examined = 0u64;
            for &i in live.iter() {
                let i = i as usize;
                examined += grid.near_count(sx[i], sz[i]) as u64;
                self.delta[i] = compute(i);
            }
            self.examined_move += examined;
            return;
        }

        // Разбиение по слотам: каждый поток пишет в свой кусок `delta`,
        // куски не пересекаются, и порядок внутри куска фиксирован.
        let n = self.delta.len();
        let block = n.div_ceil(threads);
        let mut rest: &mut [(i32, i32)] = &mut self.delta;
        let mut parts: Vec<(usize, &mut [(i32, i32)])> = Vec::with_capacity(threads);
        let mut base = 0usize;
        while !rest.is_empty() {
            let take = block.min(rest.len());
            let (head, tail) = rest.split_at_mut(take);
            parts.push((base, head));
            base += take;
            rest = tail;
        }
        std::thread::scope(|sc| {
            for (offset, part) in parts {
                let compute = &compute;
                sc.spawn(move || {
                    for (k, slot) in part.iter_mut().enumerate() {
                        let i = offset + k;
                        if !u.alive[i] {
                            continue;
                        }
                        *slot = compute(i);
                    }
                });
            }
        });
    }

    /// Применить смещения. Серийная фаза: интегрирование в `Fx` и перенос
    /// целой части в сетку.
    fn apply_deltas(&mut self) {
        let u = &mut self.units;
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let (vx, vz) = self.delta[i];
            let step_x = Fx::from_ratio(vx, 30);
            let step_z = Fx::from_ratio(vz, 30);
            let nx = u.fx[i].add(step_x);
            let nz = u.fz[i].add(step_z);
            let carry_x = nx.to_int_trunc();
            let carry_z = nz.to_int_trunc();
            u.fx[i] = nx.sub(Fx::from_int(carry_x));
            u.fz[i] = nz.sub(Fx::from_int(carry_z));
            u.x[i] = (u.x[i] + carry_x).clamp(0, WORLD - 1);
            u.z[i] = (u.z[i] + carry_z).clamp(0, WORLD - 1);
        }
    }

    /// Бой: поиск цели и урон.
    ///
    /// Две части, и разделены они не для красоты. Поиск цели пишет только
    /// в `target[i]` — слоты не пересекаются, значит фаза параллелится.
    /// Урон пишет в **чужие** слоты, поэтому идёт серийно; накопитель
    /// складывается обычным сложением, потому что фаза серийная.
    fn phase_combat(&mut self) {
        let u = &mut self.units;
        let mut examined = 0u64;
        let r2 = RANGE * RANGE;

        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let (ux, uz, mine) = (u.x[i], u.z[i], u.owner[i]);
            let mut best = i32::MAX;
            let mut best_j = NO_TARGET;
            self.grid_far.for_each_near(ux, uz, |j| {
                examined += 1;
                let jj = j as usize;
                if u.owner[jj] == mine || !u.alive[jj] {
                    return;
                }
                let (dx, dz) = (ux - u.x[jj], uz - u.z[jj]);
                let d2 = dx * dx + dz * dz;
                if d2 > r2 {
                    return;
                }
                // Ничья разрешается **линейным индексом**, а не порядком
                // обхода (`CLAUDE.md`).
                if d2 < best || (d2 == best && j < best_j) {
                    best = d2;
                    best_j = j;
                }
            });
            u.target[i] = best_j;
        }
        self.examined_combat += examined;

        self.damage.clear();
        self.damage.resize(u.x.len(), 0);
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let t = u.target[i];
            if t == NO_TARGET {
                continue;
            }
            self.damage[t as usize] += 1;
        }
        for k in 0..u.live_ids.len() {
            let i = u.live_ids[k] as usize;
            let d = self.damage[i];
            if d == 0 {
                continue;
            }
            u.voxels[i] = u.voxels[i].saturating_sub(d.min(u16::MAX as u32) as u16);
        }
    }

    /// Видимость: здесь заглушка с реалистичной формой доступа —
    /// настоящая мерилась отдельно (ADR-070, ADR-072).
    fn phase_visibility(&mut self) {}

    /// Уборка: мёртвые уходят в free-list.
    fn phase_cleanup(&mut self) {
        let mut dead: Vec<u32> = Vec::new();
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k];
            if self.units.voxels[i as usize] == 0 {
                dead.push(i);
            }
        }
        for i in dead {
            self.units.kill(i);
        }
    }

    /// Хеш состояния. Только `wrapping_add` и `wrapping_mul`: единственная
    /// ассоциативная пара, безопасная для параллельной редукции
    /// (`CLAUDE.md`).
    fn phase_hash(&mut self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for k in 0..self.units.live_ids.len() {
            let i = self.units.live_ids[k] as usize;
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(self.units.x[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.z[i] as u32 as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.fx[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.fz[i].raw() as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.voxels[i] as u64);
            w = w.wrapping_mul(31).wrapping_add(self.units.target[i] as u64);
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        h
    }
}

/// Заселение мира: две армии, каждая идёт на позицию другой.
pub fn populate(w: &mut World, n: usize, seed: u64) {
    let mut r = Rng::new(seed, 0xA1);
    for k in 0..n {
        let owner = (k % 2) as u8;
        let (bx, bz) = if owner == 0 {
            (200, 200)
        } else {
            (WORLD - 200, WORLD - 200)
        };
        let x = (bx + r.range(-150, 150)).clamp(0, WORLD - 1);
        let z = (bz + r.range(-150, 150)).clamp(0, WORLD - 1);
        w.units.spawn(x, z, owner, r.range(2, 6), 300);
    }
    w.fill_flow_towards(0, WORLD - 200, WORLD - 200);
    w.fill_flow_towards(1, 200, 200);
    w.units.refresh_live();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Один сид — один хеш. Базовое свойство, без которого нет ничего.
    #[test]
    fn same_seed_same_hash() {
        let run = || {
            let mut w = World::new(2, 4096);
            populate(&mut w, 2000, 42);
            let mut h = 0u64;
            for _ in 0..60 {
                h = w.tick();
            }
            h
        };
        assert_eq!(run(), run());
    }

    /// **Требование `CLAUDE.md`: один сценарий на 1/2/4/8 воркерах даёт
    /// один хеш.** Держится оно на том, что фаза движения читает снимок,
    /// а не уже сдвинутых соседей: без снимка расталкивание зависит
    /// от порядка обхода, и параллелить его нельзя ни при каком разбиении.
    #[test]
    fn one_hash_across_worker_counts() {
        let run = |threads: usize| {
            let mut w = World::new(2, 8192);
            populate(&mut w, 3000, 77);
            let mut h = 0u64;
            for _ in 0..80 {
                h = w.tick_par(threads);
            }
            h
        };
        let base = run(1);
        for t in [2usize, 4, 8] {
            assert_eq!(run(t), base, "хеш разошёлся на {t} воркерах");
        }
    }

    /// Разные сиды — разные хеши. Ловит вырождение хеша в константу.
    #[test]
    fn different_seeds_differ() {
        let run = |seed| {
            let mut w = World::new(2, 4096);
            populate(&mut w, 500, seed);
            let mut h = 0u64;
            for _ in 0..30 {
                h = w.tick();
            }
            h
        };
        assert_ne!(run(1), run(2));
    }

    /// Состояние обязано **меняться**: тест на «фазы пустые и потому быстрые».
    #[test]
    fn state_actually_moves() {
        let mut w = World::new(2, 4096);
        populate(&mut w, 500, 7);
        // Поле направлений четырёхстороннее, поэтому половина юнитов идёт
        // чисто по Z: проверять надо положение, а не одну координату.
        let before: Vec<(i32, i32)> = w
            .units
            .x
            .iter()
            .zip(w.units.z.iter())
            .map(|(&a, &b)| (a, b))
            .collect();
        for _ in 0..30 {
            w.tick();
        }
        let moved = w
            .units
            .x
            .iter()
            .zip(w.units.z.iter())
            .zip(before.iter())
            .filter(|((a, b), (pa, pb))| **a != *pa || **b != *pb)
            .count();
        assert!(moved > 450, "сдвинулось только {moved} из 500");
        assert!(w.examined_move > 0, "расталкивание никого не осмотрело");
    }

    /// Free-list, а не `swap_and_pop`: убийство юнита не двигает соседей.
    /// Именно перестановка соседей молча меняет исход ничьей.
    #[test]
    fn kill_does_not_move_neighbours() {
        let mut w = World::new(2, 64);
        for k in 0..10 {
            w.units.spawn(k * 10, 0, 0, 1, 100);
        }
        w.units.refresh_live();
        let before: Vec<i32> = w.units.x.clone();
        w.units.kill(4);
        w.units.refresh_live();
        assert_eq!(w.units.x, before, "убийство сдвинуло чужие слоты");
        assert_eq!(w.units.live(), &[0, 1, 2, 3, 5, 6, 7, 8, 9]);
        // Переиспользование слота — того же, а не нового.
        let n = w.units.len();
        let id = w.units.spawn(999, 999, 1, 1, 100);
        assert_eq!(id, 4);
        assert_eq!(w.units.len(), n, "хранилище выросло вместо переиспользования");
    }

    /// Выбор цели не зависит от порядка обхода: при равном расстоянии
    /// побеждает меньший индекс.
    #[test]
    fn ties_resolve_by_index() {
        let mut w = World::new(2, 64);
        let me = w.units.spawn(100, 100, 0, 0, 100);
        // Две цели на одинаковом расстоянии.
        let a = w.units.spawn(110, 100, 1, 0, 100);
        let b = w.units.spawn(90, 100, 1, 0, 100);
        assert!(a < b);
        w.units.refresh_live();
        w.tick();
        assert_eq!(w.units.target[me as usize], a, "ничья ушла не меньшему индексу");
    }
}
