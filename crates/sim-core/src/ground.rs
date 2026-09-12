//! Воксельная часть мира: сам мир, тела, связность и проходимость.
//!
//! До этого её собирал крейт демки, отдельно в каждой сцене, и порядок
//! подсистем в тике был у каждой свой. `CLAUDE.md` требует обратного:
//! **порядок систем в тике фиксирован явным списком**. Здесь он один
//! и лежит в `PHASES` рядом с фазами юнитов (ADR-125).
//!
//! **Поток правок за тик собирается целиком.** Дренаж очереди идёт
//! трижды — сама копка, отрывы каскада и запекание приземлившихся тел,
//! — и каждый flush обнуляет список применённых правок. Подсистеме,
//! которая читает правки (связность, проходимость, ремеш, рендер),
//! нужен весь тик, а не последний его кусок.

use crate::Tuning;
use nav::layer::{NavLayer, NavStats};
use voxel::bodies::{Bodies, BodyStats};
use voxel::collapse::{CollapseLayer, CollapseStats};
use voxel::topo::layer::TopoLayer;
use voxel::{Edit, World as VoxelWorld};

/// Что дал тик на земле.
#[derive(Default, Clone, Copy)]
pub struct GroundStats {
    /// Правок применено за тик, всеми тремя дренажами.
    pub edits: usize,
    /// Осталось в очереди — отставание мира от того, кто его правит.
    pub queued: usize,
    /// Чанков связности пересчитано.
    pub topo_dirty: usize,
    /// Правок, переживших ступень A.
    pub topo_survived: usize,
    /// Урона от падения нанесено за тик.
    pub fall_damage: u64,
    /// Поездок началось за тик: юнитов, севших на свежие куски.
    pub rides: u64,
    /// Самое долгое падение, вокселей.
    pub max_fall: u32,
    pub collapse: CollapseStats,
    pub bodies: BodyStats,
    pub nav: NavStats,
}

pub struct Ground {
    pub world: VoxelWorld,
    pub bodies: Bodies,
    pub collapse: CollapseLayer,
    pub topo: TopoLayer,
    pub nav: NavLayer,
    /// Все правки тика подряд, в порядке применения.
    pub(crate) edits: Vec<Edit>,
    /// Правки, применённые **после** каскада: запекание приземлившихся
    /// тел. Каскад в этом тике их уже не увидит, поэтому они переносятся
    /// в следующий — иначе его чанки разошлись бы с миром (ADR-129).
    pub(crate) carry: Vec<Edit>,
    pub stats: GroundStats,
}

impl Ground {
    /// Собрать землю вокруг готового воксельного мира.
    ///
    /// Рост юнита, сторона нав-клетки, посильный перепад, потолок тел
    /// и тяготение приходят из данных (ADR-140, ADR-173): все пять —
    /// игровые числа, и литералов им в исходнике не место.
    pub fn new(world: VoxelWorld, t: &Tuning) -> Ground {
        let mut collapse = CollapseLayer::new(&world);
        collapse.set_max_body_cells(t.max_body_cells as usize);
        let topo = TopoLayer::new(&world);
        let nav = NavLayer::new(&world, t.unit_height, t.nav_cell, t.max_step);
        let mut world = world;
        world.max_materialize_per_tick = t.max_materialize_per_tick as usize;
        Ground {
            world,
            bodies: Bodies::new(t.max_bodies, crate::gravity_per_tick(t)),
            collapse,
            topo,
            nav,
            edits: Vec::new(),
            carry: Vec::new(),
            stats: GroundStats::default(),
        }
    }

    /// Протяжённость рельефа в вокселях. Юниты живут в этих же
    /// координатах: у мира и у земли **одна сетка**, иначе юниты
    /// ходят по отвлечённой плоскости и о прокопанном не знают
    /// (ADR-134).
    pub fn extent(&self) -> (i32, i32) {
        (self.world.nx as i32, self.world.nz as i32)
    }

    /// Правки этого тика — весь поток, а не последний дренаж.
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    /// Фаза `VoxelEdits`: дренаж очереди. Дыра обязана появиться
    /// в мире здесь, до всех, кто её читает (`CLAUDE.md`).
    pub(crate) fn phase_edits(&mut self) {
        self.stats = GroundStats::default();
        self.edits.clear();
        // Перенесённое с прошлого тика идёт первым: каскад обязан
        // увидеть запекание тела как обычную правку.
        self.edits.append(&mut self.carry);
        // Потолок материализаций считается от начала тика, а не
        // от начала дренажа: дренажей три (ADR-132).
        self.world.begin_tick();
        let st = self.world.flush();
        self.stats.edits += st.applied;
        self.stats.queued = st.queued;
        self.edits.extend_from_slice(self.world.edits());
    }

    /// Фаза `Destruction`: каскад отделения. Отрывы ставят свои правки
    /// в ту же очередь, и она тут же дренажится: иначе мир до конца
    /// тика содержал бы воксели, уже ставшие телом.
    pub(crate) fn phase_destruction(&mut self) {
        self.bodies.spawned.clear();
        let edits = std::mem::take(&mut self.edits);
        self.stats.collapse = self.collapse.apply(&mut self.world, &edits, &mut self.bodies);
        self.edits = edits;
        let st = self.world.flush();
        self.stats.edits += st.applied;
        self.stats.queued = st.queued;
        self.edits.extend_from_slice(self.world.edits());
    }

    /// Фаза `Bodies`: шаг падения. Приземлившиеся запекаются обратно
    /// через очередь правок, а не записью напрямую.
    pub(crate) fn phase_bodies(&mut self) {
        self.stats.bodies = self.bodies.step(&mut self.world);
        let st = self.world.flush();
        self.stats.edits += st.applied;
        self.stats.queued = st.queued;
        self.edits.extend_from_slice(self.world.edits());
        // Каскад в этом тике уже отработал, а запекание тела — такая же
        // правка мира, как копка. Переносим её ему на следующий тик.
        //
        // Первая редакция вместо этого выбрасывала затронутые чанки
        // из каскада целиком, и он собирал их заново из мира: семь
        // сборок по 32 768 чтений за тик, то есть четверть всей его
        // цены (ADR-129).
        self.carry.extend_from_slice(self.world.edits());
    }

    /// Фаза `NavUpdate`: связность и проходимость по потоку правок тика.
    pub(crate) fn phase_nav(&mut self) {
        let edits = std::mem::take(&mut self.edits);
        let (dirty, survived) = self.topo.apply(&self.world, &edits);
        self.stats.topo_dirty = dirty;
        self.stats.topo_survived = survived;
        self.stats.nav = self.nav.apply(&self.world, &edits);
        self.edits = edits;
    }

    /// Записать землю в буфер (ADR-229).
    ///
    /// Связность и проходимость **не пишутся**: обе — чистые функции
    /// мира, и восстанавливаются из него `TopoLayer::new`
    /// и `NavLayer::new`. Это замерено зондом, а не предположено:
    /// пересобранные слои дают тот же хеш, пересобранный каскад —
    /// нет. Каскад отстаёт от мира на перенесённые правки (ADR-129),
    /// потому и пишется.
    ///
    /// `carry` — те самые перенесённые правки. Не запиши их,
    /// и следующий тик не показал бы каскаду запекание тела.
    pub fn save(&self, b: &mut fx::blob::Blob) {
        self.world.save(b);
        self.bodies.save(b);
        self.collapse.save(b);
        let (nx, nz) = (self.world.nx, self.world.nz);
        voxel::world::save_edits(b, nx, nz, &self.edits);
        voxel::world::save_edits(b, nx, nz, &self.carry);
    }

    pub fn load(&mut self, s: &mut fx::blob::Scan) -> bool {
        if !self.world.load(s) {
            return false;
        }
        if !self.bodies.load(s) {
            return false;
        }
        if !self.collapse.load(s) {
            return false;
        }
        let (nx, nz) = (self.world.nx, self.world.nz);
        self.edits = voxel::world::load_edits(s, nx, nz);
        self.carry = voxel::world::load_edits(s, nx, nz);
        s.ok()
    }

    /// Подмешать состояние земли в хеш тика.
    ///
    /// Хешируется **поток правок**, а не мир целиком: мир — сотни
    /// мегабайт, а расходятся клиенты именно на правках. Плюс число
    /// живых тел: отвал, случившийся у одного и не случившийся
    /// у другого, обязан быть виден.
    pub(crate) fn hash(&self) -> u64 {
        let mut h = 0u64;
        for (i, e) in self.edits.iter().enumerate() {
            let mut w = 0u64;
            w = w.wrapping_mul(31).wrapping_add(e.x as u64);
            w = w.wrapping_mul(31).wrapping_add(e.y as u64);
            w = w.wrapping_mul(31).wrapping_add(e.z as u64);
            w = w.wrapping_mul(31).wrapping_add(e.mat as u64);
            h = h.wrapping_add(w.wrapping_mul(i as u64 + 1));
        }
        h = h.wrapping_add((self.bodies.list.len() as u64).wrapping_mul(0x9E37_79B9));
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_tuning;
    use crate::World;
    use voxel::store::{Mat, AIR};

    /// Плита: твёрдо ниже `TOP`, воздух выше.
    struct Slab;
    const TOP: u32 = 40;
    impl voxel::Generator for Slab {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < TOP {
                2
            } else {
                AIR
            }
        }
    }

    fn world_with_ground() -> World {
        let vw = voxel::World::new(64, 64, 64, 4096, Box::new(Slab));
        let mut w = World::new(2, 16, test_tuning());
        w.attach_ground(Ground::new(vw, &test_tuning()));
        w
    }

    /// Гряда поперёк карты: земля до `GY`, стена на `WALL` до `WY`.
    struct Wall;
    const GY: u32 = 8;
    const WALL: u32 = 32;
    const WY: u32 = 40;
    impl voxel::Generator for Wall {
        fn material(&self, x: u32, y: u32, _z: u32) -> Mat {
            if y < GY {
                return 2;
            }
            if x.abs_diff(WALL) <= 1 && y < WY {
                return 3;
            }
            AIR
        }
    }

    /// **Рельеф держит юнита.** Стена сплошная, цель за ней, дороги нет —
    /// и юнит остаётся на своей стороне.
    ///
    /// Второе утверждение важнее первого: он **не долбится** в камень.
    /// Пока поле направлений было прямой линией, юнит упирался в стену
    /// каждый тик до конца прогона; поле, посчитанное по навигации,
    /// про недостижимую цель знает и никуда не ведёт (ADR-138).
    #[test]
    fn terrain_stops_a_unit() {
        let vw = voxel::World::new(64, 64, 64, 4096, Box::new(Wall));
        let mut w = World::new(1, 4, test_tuning());
        w.attach_ground(Ground::new(vw, &test_tuning()));
        let id = w.units.spawn(28, 32, 0, 6, 300) as usize;
        w.fill_flow_towards(0, 60, 32);
        w.units.refresh_live();
        for _ in 0..200 {
            w.tick();
        }
        assert!(
            w.units.x[id] < WALL as i32,
            "юнит прошёл сквозь стену: x = {}",
            w.units.x[id]
        );
        assert_eq!(
            w.blocked_moves, 0,
            "юнит долбится в стену, за которой ему нечего делать"
        );
    }

    /// **Юнит на исчезнувшей опоре падает** (ADR-036) — а не оказывается
    /// на дне ямы тем же тиком и без урона, как было до ADR-139.
    ///
    /// Земля под юнитом снимается на шесть вокселей. Он обязан
    /// пролететь эти шесть, потратить на них **больше** шести тиков —
    /// падение начинается с нуля и разгоняется (ADR-174), поэтому
    /// первые вокселя даются дольше воксела за тик, — и получить урон
    /// по высоте падения.
    #[test]
    fn digging_out_the_ground_drops_the_unit_and_hurts_it() {
        const DEPTH: u32 = 6;
        let vw = voxel::World::new(64, 64, 64, 100_000, Box::new(Wall));
        let mut w = World::new(1, 4, test_tuning());
        w.attach_ground(Ground::new(vw, &test_tuning()));
        let id = w.units.spawn(20, 20, 0, 0, 300) as usize;
        w.units.y[id] = GY as i32;
        w.units.refresh_live();
        w.tick();
        assert_eq!(w.units.y[id], GY as i32, "юнит не на земле");
        let hp0 = w.units.voxels[id];

        // Шахта под юнитом: вся его нав-клетка, десять вокселей вглубь.
        {
            let g = w.ground.as_mut().expect("земля");
            for y in (GY - DEPTH)..GY {
                for z in 20..22 {
                    for x in 20..22 {
                        g.world.queue_dig(x, y, z);
                    }
                }
            }
        }
        let mut ticks = 0;
        while ticks < 100 && w.units.y[id] > (GY - DEPTH) as i32 {
            w.tick();
            ticks += 1;
        }
        assert_eq!(
            w.units.y[id],
            (GY - DEPTH) as i32,
            "юнит не долетел до дна: y = {}",
            w.units.y[id]
        );
        assert!(
            ticks > DEPTH as i32,
            "падение заняло {ticks} тиков на {DEPTH} вокселей: с разгона от нуля столько не пролететь, значит падение не ускоряется, а телепортируется"
        );
        let hurt = hp0 - w.units.voxels[id];
        assert_eq!(
            hurt as u32,
            (DEPTH - 1) * w.tuning.fall_damage_per_voxel as u32,
            "урон не по высоте падения за вычетом бесплатного воксела"
        );
        let g = w.ground().expect("земля");
        assert_eq!(g.stats.max_fall, DEPTH, "падение не попало в отчёт земли");
    }

    /// Полка над коренной породой: пол на `BED`, плита на `SHELF`,
    /// между ними и над ними воздух.
    ///
    /// Клетка получает ровно два уровня — `BED` и `SHELF + 1`, — и это
    /// единственное, что тут нужно: между ними есть куда разогнаться,
    /// а внизу есть куда провалиться, если опору прозевать.
    struct Shelf;
    const BED: u32 = 4;
    const SHELF: u32 = 20;
    impl voxel::Generator for Shelf {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < BED || y == SHELF {
                2
            } else {
                AIR
            }
        }
    }

    /// Поставить одинокого юнита в воздух над полкой.
    fn dropped_from(t: Tuning, from: i32) -> (World, usize) {
        let vw = voxel::World::new(64, 64, 64, 100_000, Box::new(Shelf));
        let mut w = World::new(1, 4, t);
        w.attach_ground(Ground::new(vw, &t));
        let id = w.units.spawn(20, 20, 0, 0, 300) as usize;
        w.units.y[id] = from;
        w.units.refresh_live();
        (w, id)
    }

    /// **Падение ускоряется** (ADR-174). Канон обещал телам падение
    /// «по вертикали с ускорением в `Fx`» (`docs/04-world.md`)
    /// с самого начала, а юнит летел по вокселю за тик — с постоянной
    /// скоростью в пятнадцать метров в секунду, с первого тика и до дна.
    ///
    /// Проверяется не «стало быстрее», а **форма**: первую половину
    /// высоты юнит летит заметно дольше второй. У равномерного падения
    /// половины равны, у равноускоренного первая длиннее второй
    /// в `1/(√2−1) ≈ 2.4` раза. Порог взят вдвое — с запасом
    /// на округление и на то, что первый тик уходит на разгон.
    ///
    /// На старом поведении половины равны, и тест падает.
    #[test]
    fn a_long_fall_speeds_up() {
        const FROM: i32 = 60;
        let floor = SHELF as i32 + 1;
        let half = FROM - (FROM - floor) / 2;
        let (mut w, id) = dropped_from(test_tuning(), FROM);
        let (mut first, mut total) = (0, 0);
        let mut biggest = 0;
        let mut prev = w.units.y[id];
        for _ in 0..400 {
            w.tick();
            biggest = biggest.max(prev - w.units.y[id]);
            prev = w.units.y[id];
            total += 1;
            if prev > half {
                first = total;
            }
            if prev == floor {
                break;
            }
        }
        assert_eq!(w.units.y[id], floor, "юнит не долетел до полки");
        let second = total - first;
        assert!(first > 0 && second > 0, "падение уложилось в один тик");
        assert!(
            first > second * 2,
            "первая половина высоты занята {first} тиков, вторая {second}: падение не ускоряется"
        );
        assert!(
            biggest > 1,
            "самый быстрый тик дал {biggest} воксель — падение так и идёт по вокселю за тик"
        );
    }

    /// **Опора не проскакивается на скорости.** Это главная ловушка
    /// ускоренного падения: при шести вокселях за тик проверка «есть ли
    /// уровень на конечной высоте» пропустит пол в пяти случаях из шести,
    /// и юнит окажется в коренной породе с уроном за всю глубину
    /// (ADR-174).
    ///
    /// Тяготение здесь нарочно вдвадцатеро земного: это оснастка,
    /// а не баланс — она разгоняет юнита до полудюжины вокселей за тик
    /// к моменту встречи с полкой.
    #[test]
    fn a_fast_fall_lands_on_the_ledge_and_not_through_it() {
        let mut t = test_tuning();
        t.gravity = 400;
        const FROM: i32 = 60;
        let (mut w, id) = dropped_from(t, FROM);
        let hp0 = w.units.voxels[id];
        let mut ticks = 0;
        while ticks < 100 && w.units.y[id] > SHELF as i32 + 1 {
            w.tick();
            ticks += 1;
        }
        assert_eq!(
            w.units.y[id],
            SHELF as i32 + 1,
            "юнит прошёл сквозь полку: y = {}, а полка на {}",
            w.units.y[id],
            SHELF + 1
        );
        // Скорость на подлёте обязана быть заметно больше воксела за тик,
        // иначе тест проверяет не то, ради чего написан.
        assert!(ticks < 20, "юнит падал {ticks} тиков — он не разогнался");
        let fallen = (FROM - (SHELF as i32 + 1)) as u32;
        assert_eq!(
            (hp0 - w.units.voxels[id]) as u32,
            (fallen - 1) * w.tuning.fall_damage_per_voxel as u32,
            "урон не по высоте до полки: падение засчитано не туда"
        );
    }

    /// **Шаг вниз на воксель бесплатен** (ADR-036): это не падение,
    /// а неровность. Правило названо в каноне и обязано быть проверено —
    /// иначе копка стоила бы здоровья каждому, кто идёт по забою.
    #[test]
    fn a_single_voxel_drop_is_free() {
        let vw = voxel::World::new(64, 64, 64, 100_000, Box::new(Wall));
        let mut w = World::new(1, 4, test_tuning());
        w.attach_ground(Ground::new(vw, &test_tuning()));
        let id = w.units.spawn(20, 20, 0, 0, 300) as usize;
        w.units.y[id] = GY as i32;
        w.units.refresh_live();
        w.tick();
        let hp0 = w.units.voxels[id];
        {
            let g = w.ground.as_mut().expect("земля");
            for z in 20..22 {
                for x in 20..22 {
                    g.world.queue_dig(x, GY - 1, z);
                }
            }
        }
        // **Ровно один тик.** Шаг вниз — это шаг, а не падение: полвоксела
        // свободного падения занимают треть секунды, и копатель, снявший
        // воксель под собой, висел бы над собственной ямой девять тиков
        // (ADR-174). Потолок в один тик — и есть проверка правила.
        w.tick();
        assert_eq!(w.units.y[id], GY as i32 - 1, "юнит не шагнул вниз за один тик");
        assert_eq!(w.units.voxels[id], hp0, "шаг вниз на воксель стоил здоровья");
    }

    /// **Критерий этапа 4** (`docs/11-roadmap.md`): до взрыва пути нет,
    /// после взрыва он находится, и число тиков на дорогу — одно и то же
    /// от прогона к прогону. Сейчас их **33**: до взрыва юнит успевает
    /// дойти до стены в том месте, которое ближе всего к цели, — это
    /// и есть «настолько близко, насколько карта позволяет», — и после
    /// взрыва ему остаётся только пройти сквозь.
    ///
    /// Это не проверка навигации вообще, а проверка того, что поле
    /// **пересчитывается по изменившемуся миру**: заглушка-прямая
    /// проходила бы первую половину теста и валила вторую наоборот —
    /// она вела бы юнита в стену с самого начала.
    #[test]
    fn a_blast_opens_a_road_and_the_walk_is_the_same_length_every_time() {
        fn run() -> (bool, u32) {
            let vw = voxel::World::new(64, 64, 64, 100_000, Box::new(Wall));
            let mut w = World::new(1, 4, test_tuning());
            w.attach_ground(Ground::new(vw, &test_tuning()));
            // Юнит стоит напротив глухой части стены, в двадцати клетках
            // от того места, где потом появится пролом: дорога к нему —
            // вбок и только потом вперёд.
            let id = w.units.spawn(28, 10, 0, 6, 300) as usize;
            w.units.y[id] = GY as i32;
            w.fill_flow_towards(0, 60, 32);
            w.units.refresh_live();

            // До взрыва: стена сплошная, дороги нет.
            for _ in 0..400 {
                w.tick();
            }
            let crossed_before = w.units.x[id] > WALL as i32;

            // Взрыв: коридор сквозь стену.
            {
                let g = w.ground.as_mut().expect("земля");
                for y in GY..GY + 6 {
                    for z in 30..36 {
                        for x in WALL - 1..=WALL + 1 {
                            g.world.queue_dig(x, y, z);
                        }
                    }
                }
            }
            let mut ticks = 0u32;
            while ticks < 2000 && w.units.x[id] <= WALL as i32 {
                w.tick();
                ticks += 1;
            }
            (crossed_before, ticks)
        }

        let (before, ticks) = run();
        assert!(!before, "юнит прошёл сквозь целую стену");
        assert!(ticks < 2000, "после взрыва путь так и не нашёлся");
        let (before2, ticks2) = run();
        assert!(!before2);
        assert_eq!(ticks, ticks2, "дорога заняла разное число тиков");
    }

    /// **Юнит обходит стену через пролом**, а не идёт в неё лбом.
    ///
    /// Пролом прорублен далеко от юнита: цель прямо по курсу, а дорога
    /// к ней — вбок и потом вперёд. Поле-прямая привело бы его в камень
    /// и держало бы там весь прогон; поле по навигации ведёт в обход.
    ///
    /// Тест сторожит и то, ради чего писался прежний: юниты живут
    /// в координатах земли (ADR-134). Прийти к пролому, которого нет
    /// на отвлечённой плоскости, иначе нельзя.
    #[test]
    fn a_unit_walks_around_to_the_opening() {
        let vw = voxel::World::new(64, 64, 64, 100_000, Box::new(Wall));
        let mut w = World::new(1, 4, test_tuning());
        w.attach_ground(Ground::new(vw, &test_tuning()));
        {
            let g = w.ground.as_mut().expect("земля");
            for y in GY..GY + 6 {
                for z in 30..36 {
                    for x in WALL - 1..=WALL + 1 {
                        g.world.queue_dig(x, y, z);
                    }
                }
            }
        }
        // Юнит стоит напротив глухой части стены, в двадцати клетках
        // от пролома.
        let id = w.units.spawn(28, 10, 0, 6, 300) as usize;
        w.fill_flow_towards(0, 60, 32);
        w.units.refresh_live();
        for _ in 0..1200 {
            w.tick();
        }
        assert!(
            w.units.x[id] > WALL as i32,
            "юнит не нашёл обход: x = {}, z = {}",
            w.units.x[id],
            w.units.z[id]
        );
    }

    /// **Прокопанный проход открывает дорогу.** Та же стена, но в ней
    /// прорублен коридор: юнит проходит.
    #[test]
    fn a_dug_corridor_lets_the_unit_through() {
        let vw = voxel::World::new(64, 64, 64, 100_000, Box::new(Wall));
        let mut w = World::new(1, 4, test_tuning());
        w.attach_ground(Ground::new(vw, &test_tuning()));
        {
            let g = w.ground.as_mut().expect("земля");
            for y in GY..GY + 6 {
                for z in 30..36 {
                    for x in WALL - 1..=WALL + 1 {
                        g.world.queue_dig(x, y, z);
                    }
                }
            }
        }
        let id = w.units.spawn(28, 32, 0, 6, 300) as usize;
        w.fill_flow_towards(0, 60, 32);
        w.units.refresh_live();
        for _ in 0..400 {
            w.tick();
        }
        assert!(
            w.units.x[id] > WALL as i32,
            "юнит не прошёл по коридору: x = {}",
            w.units.x[id]
        );
    }

    /// Гриб: земля, ножка, шляпка. Подкоп ножки роняет шляпку.
    struct Mushroom;
    impl voxel::Generator for Mushroom {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if y < 8 {
                return 2;
            }
            let (dx, dz) = (x.abs_diff(32), z.abs_diff(32));
            if dx <= 2 && dz <= 2 && y < 40 {
                return 3;
            }
            if dx <= 6 && dz <= 6 && (40..44).contains(&y) {
                return 4;
            }
            AIR
        }
    }

    /// **Юнит на нависающей шляпке остаётся на ней**, а не
    /// проваливается на землю под ней.
    ///
    /// До ADR-136 нав-слой держал в колонке один уровень, нижний,
    /// и высота юнита каждый тик притягивалась к земле: стоять
    /// на нависающем было нельзя в принципе.
    #[test]
    fn a_unit_on_an_overhang_stays_on_it() {
        let vw = voxel::World::new(64, 64, 64, 4096, Box::new(Mushroom));
        let mut w = World::new(1, 4, test_tuning());
        w.attach_ground(Ground::new(vw, &test_tuning()));
        // Край шляпки: под ним пусто до самой земли.
        let id = w.units.spawn(38, 38, 0, 0, 300) as usize;
        w.units.y[id] = 44;
        w.units.refresh_live();
        {
            let g = w.ground().expect("земля");
            let (cx, cz) = (38 / g.nav.step(), 38 / g.nav.step());
            let ls = g.nav.levels_at(cx, cz);
            assert!(ls.len() >= 2, "у клетки один уровень: {ls:?}");
            assert!(ls.contains(&44), "верха шляпки нет среди уровней: {ls:?}");
        }
        for _ in 0..20 {
            w.tick();
        }
        assert_eq!(w.units.y[id], 44, "юнит провалился под шляпку");
    }

    /// **Юнит едет вниз вместе с куском и получает урон по высоте
    /// падения** (ADR-099, ADR-036).
    ///
    /// До ADR-135 у юнита не было высоты вовсе, и посадка на кусок
    /// жила в сцене, а не в движке.
    #[test]
    fn a_unit_rides_the_falling_piece_and_is_hurt_by_the_drop() {
        let vw = voxel::World::new(64, 64, 64, 100_000, Box::new(Mushroom));
        let mut w = World::new(1, 4, test_tuning());
        w.attach_ground(Ground::new(vw, &test_tuning()));
        let id = w.units.spawn(32, 32, 0, 0, 300) as usize;
        w.units.refresh_live();
        // Первый тик ставит юнита на пол своей клетки — верх шляпки.
        w.tick();
        let y0 = w.units.y[id];
        assert_eq!(y0, 44, "юнит встал не на шляпку: y = {y0}");

        // Подрезаем ножку: двенадцать слоёв за тик.
        {
            let g = w.ground.as_mut().expect("земля");
            for y in 9..21 {
                for z in 29..36 {
                    for x in 29..36 {
                        if g.world.solid(x, y, z) {
                            g.world.queue_dig(x, y, z);
                        }
                    }
                }
            }
        }
        let mut fell = 0u32;
        let mut damage = 0u64;
        for _ in 0..40 {
            w.tick();
            let g = w.ground().expect("земля");
            fell = fell.max(g.stats.max_fall);
            damage += g.stats.fall_damage;
        }
        assert!(fell >= 10, "кусок почти не падал: {fell}");
        assert!(
            w.units.y[id] < y0,
            "юнит не поехал вниз: {} против {y0}",
            w.units.y[id]
        );
        assert!(damage > 0, "урона от падения не было");
        assert!(
            w.units.voxels[id] < 300,
            "юнит не пострадал: {}",
            w.units.voxels[id]
        );
        assert_eq!(w.units.riding[id], 0, "юнит остался на несуществующем теле");
    }

    /// **Дыра появляется в том же тике, где её прокопали**, и её видят
    /// связность и проходимость (`CLAUDE.md`). Это и есть смысл того,
    /// что фазы земли стоят в списке до навигации.
    #[test]
    fn a_hole_is_visible_to_navigation_in_the_same_tick() {
        let mut w = world_with_ground();
        let floor_before = {
            let g = w.ground().expect("земля");
            g.nav.floor_at(8, 8)
        };
        {
            let g = w.ground.as_mut().expect("земля");
            for y in TOP - 6..TOP {
                for z in 16..18 {
                    for x in 16..18 {
                        g.world.queue_dig(x, y, z);
                    }
                }
            }
        }
        w.tick();
        let g = w.ground().expect("земля");
        assert!(g.stats.edits > 0, "ни одной правки не применено");
        assert!(!g.world.solid(16, TOP - 1, 16), "дыры в мире нет");
        assert!(g.stats.topo_dirty > 0, "связность правок не увидела");
        // Нав-клетка над ямой опустила пол в этом же тике.
        assert!(
            g.nav.floor_at(8, 8) < floor_before,
            "пол не опустился: {} против {}",
            g.nav.floor_at(8, 8),
            floor_before
        );
    }

    /// **Поток правок за тик собирается целиком**, а не по последнему
    /// дренажу: их три, и каждый обнуляет список применённых.
    #[test]
    fn the_tick_collects_every_drain() {
        let mut w = world_with_ground();
        {
            let g = w.ground.as_mut().expect("земля");
            for x in 4..12 {
                g.world.queue_dig(x, TOP - 1, 4);
            }
        }
        w.tick();
        let g = w.ground().expect("земля");
        assert_eq!(g.edits().len(), g.stats.edits, "поток короче применённого");
        assert_eq!(g.edits().len(), 8);
    }

    /// Земля входит в хеш тика: расхождение по правкам обязано быть
    /// видно, иначе lockstep не заметит его вовсе.
    #[test]
    fn the_ground_is_part_of_the_tick_hash() {
        let run = |dig: bool| -> u64 {
            let mut w = world_with_ground();
            if dig {
                let g = w.ground.as_mut().expect("земля");
                g.world.queue_dig(20, TOP - 1, 20);
            }
            w.tick()
        };
        assert_ne!(run(true), run(false), "хеш не заметил копки");
        assert_eq!(run(true), run(true), "хеш не воспроизводим");
    }

    /// Мир без земли ведёт себя ровно как раньше: фазы земли — те же
    /// заглушки, что были.
    #[test]
    fn a_world_without_ground_ticks_exactly_as_before() {
        let mut a = World::new(2, 16, test_tuning());
        crate::populate(&mut a, 200, 5);
        let mut b = World::new(2, 16, test_tuning());
        crate::populate(&mut b, 200, 5);
        let (mut ha, mut hb) = (0, 0);
        for _ in 0..50 {
            ha = a.tick();
            hb = b.tick();
        }
        assert_eq!(ha, hb);
        assert!(a.ground().is_none());
    }
}
