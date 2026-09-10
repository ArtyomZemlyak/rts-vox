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
    edits: Vec<Edit>,
    /// Правки, применённые **после** каскада: запекание приземлившихся
    /// тел. Каскад в этом тике их уже не увидит, поэтому они переносятся
    /// в следующий — иначе его чанки разошлись бы с миром (ADR-129).
    carry: Vec<Edit>,
    pub stats: GroundStats,
}

impl Ground {
    /// Собрать землю вокруг готового воксельного мира.
    ///
    /// `nav_height` — рост юнита в вокселях, `nav_step` — сторона
    /// нав-клетки. Обе величины принадлежат данным, а не коду, и здесь
    /// стоят параметрами именно поэтому.
    pub fn new(world: VoxelWorld, max_bodies: usize, nav_height: u32, nav_step: u32) -> Ground {
        let collapse = CollapseLayer::new(&world);
        let topo = TopoLayer::new(&world);
        let nav = NavLayer::new(&world, nav_height, nav_step);
        Ground {
            world,
            bodies: Bodies::new(max_bodies),
            collapse,
            topo,
            nav,
            edits: Vec::new(),
            carry: Vec::new(),
            stats: GroundStats::default(),
        }
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
        let mut w = World::new(2, 16);
        w.attach_ground(Ground::new(vw, 16, 4, 2));
        w
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
        let mut a = World::new(2, 16);
        crate::populate(&mut a, 200, 5);
        let mut b = World::new(2, 16);
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
