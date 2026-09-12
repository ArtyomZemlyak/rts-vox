//! Динамические тела: отделившийся кусок, который падает.
//!
//! Модель Teardown — **множество небольших томов вместо одного
//! большого** (`docs/04-world.md`). Кусок, отделившийся от опоры,
//! перестаёт быть частью мира и становится телом со своим смещением.
//!
//! **Падение проверяется тестом XZ-столбцов, а не широкой фазой**
//! (ADR-037): для каждого занятого столба тела спрашивается, свободен
//! ли воксель под его нижним. Столбов у тела десятки, а не тысячи,
//! и широкая фаза здесь дороже самого теста.
//!
//! **Юнит, стоявший на куске, едет на нём вниз** (ADR-099). Привязка —
//! состояние симуляции и входит в хеш; отвязка при приземлении.
//!
//! **Лимит тел жёсткий.** Кусок, которому не хватило места, не падает,
//! а остаётся в мире: это заметно игроку, но детерминировано, а
//! «иногда падает, иногда нет» — десинк.

use crate::store::{Mat, AIR};
use crate::World;
use fx::Fx;

/// Воксель тела в его собственных координатах плюс материал.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BodyVoxel {
    pub x: u16,
    pub y: u16,
    pub z: u16,
    pub mat: Mat,
}

#[derive(Clone, Debug)]
pub struct Body {
    /// Устойчивый номер тела.
    ///
    /// Ссылаться на тело **индексом в списке нельзя**: приземлившееся
    /// тело уходит из списка сдвигом, и все индексы за ним съезжают.
    /// В D-2 из-за этого наездники после первой посадки начинали
    /// следовать за чужим телом, а отчёт о высоте падения показывал
    /// два воксела вместо четырнадцати (ADR-118).
    pub id: u64,
    /// Мировая позиция начала координат тела.
    pub ox: i64,
    pub oy: i64,
    pub oz: i64,
    pub voxels: Vec<BodyVoxel>,
    /// Габарит по X и Z — для теста столбцов.
    pub sx: u16,
    pub sz: u16,
    /// Нижний занятый воксель каждого столба, `u16::MAX` — столб пуст.
    floor: Vec<u16>,
    /// Сколько вокселей тело уже пролетело. Из этого считается урон
    /// при приземлении (ADR-036).
    pub fallen: u32,
    /// Вертикальная скорость, вокселей **за тик**. Растёт на тяготение
    /// каждый тик полёта (ADR-174): канон обещал телам падение
    /// «по вертикали с ускорением в `Fx`» (`docs/04-world.md`)
    /// с самого начала.
    pub vy: Fx,
    /// Дробная часть высоты. Первую треть секунды свободного падения
    /// скорость меньше воксела за тик, и без дробной части тело
    /// не тронулось бы с места вовсе.
    pub fy: Fx,
    /// Тело только что отвалилось и в этом тике ещё не падает.
    ///
    /// Пауза в один тик нужна не физике, а порядку фаз: между отрывом
    /// и падением кто-то должен успеть посадить на кусок наездников,
    /// а внутри одного тика такого места нет. Отставание
    /// детерминированное и одинаковое у всех (ADR-133).
    pub fresh: bool,
    pub resting: bool,
    /// Юниты, едущие на теле. Индексы во внешнем массиве юнитов;
    /// смысл им придаёт `sim-core`, телу они безразличны.
    pub riders: Vec<u32>,
}

impl Body {
    /// Собрать тело из мировых вокселей. Сами воксели из мира
    /// **не снимаются** — это делает вызывающий, вместе с правками.
    pub fn from_world_cells(w: &World, cells: &[(u32, u32, u32)]) -> Option<Body> {
        if cells.is_empty() {
            return None;
        }
        let (mut x0, mut y0, mut z0) = (u32::MAX, u32::MAX, u32::MAX);
        let (mut x1, mut z1) = (0u32, 0u32);
        for &(x, y, z) in cells {
            x0 = x0.min(x);
            y0 = y0.min(y);
            z0 = z0.min(z);
            x1 = x1.max(x);
            z1 = z1.max(z);
        }
        let (sx, sz) = ((x1 - x0 + 1) as u16, (z1 - z0 + 1) as u16);
        let mut floor = vec![u16::MAX; sx as usize * sz as usize];
        let mut voxels = Vec::with_capacity(cells.len());
        for &(x, y, z) in cells {
            let (lx, ly, lz) = ((x - x0) as u16, (y - y0) as u16, (z - z0) as u16);
            voxels.push(BodyVoxel { x: lx, y: ly, z: lz, mat: w.material(x, y, z) });
            let c = &mut floor[lz as usize * sx as usize + lx as usize];
            *c = (*c).min(ly);
        }
        // Порядок вокселей задаётся входом, а вход — обходом ступени B,
        // который детерминирован. Сортировать не нужно и нельзя:
        // сортировка скрыла бы недетерминированный вход, а не починила.
        Some(Body {
            // Номер выдаёт Bodies::detach: тело само его не знает.
            id: 0,
            fresh: true,
            ox: x0 as i64,
            oy: y0 as i64,
            oz: z0 as i64,
            voxels,
            sx,
            sz,
            floor,
            fallen: 0,
            vy: Fx::ZERO,
            fy: Fx::ZERO,
            resting: false,
            riders: Vec::new(),
        })
    }

    /// Может ли тело опуститься на воксель. Тест XZ-столбцов (ADR-037).
    pub fn can_fall(&self, w: &World) -> bool {
        if self.oy <= 0 {
            return false;
        }
        for lz in 0..self.sz {
            for lx in 0..self.sx {
                let f = self.floor[lz as usize * self.sx as usize + lx as usize];
                if f == u16::MAX {
                    continue;
                }
                let (x, y, z) = (
                    self.ox + lx as i64,
                    self.oy + f as i64 - 1,
                    self.oz + lz as i64,
                );
                if x < 0 || y < 0 || z < 0 {
                    return false;
                }
                if w.solid(x as u32, y as u32, z as u32) {
                    return false;
                }
            }
        }
        true
    }

    /// Мировые координаты воксела тела.
    pub fn world_of(&self, v: BodyVoxel) -> (i64, i64, i64) {
        (self.ox + v.x as i64, self.oy + v.y as i64, self.oz + v.z as i64)
    }
}

/// Что случилось с телами за тик.
#[derive(Clone, Copy, Default, Debug)]
pub struct BodyStats {
    pub spawned: usize,
    pub fell: usize,
    pub landed: usize,
    pub alive: usize,
    /// Куски, которым не хватило лимита и которые остались в мире.
    pub refused: usize,
}

pub struct Bodies {
    pub list: Vec<Body>,
    /// Жёсткий потолок числа живых тел.
    pub max_bodies: usize,
    /// Ускорение свободного падения, вокселей **за тик в квадрате**.
    ///
    /// Приходит снаружи уже переведённым: тело не знает ни про частоту
    /// тика, ни про `data/sim.ron`. Число то же самое, что у юнита
    /// (ADR-173) — иначе юнит и кусок, отвалившиеся вместе,
    /// разъезжаются в воздухе.
    pub gravity: Fx,
    pub refused: usize,
    /// Следующий свободный номер. Растёт монотонно и не переиспользуется:
    /// номер, выданный дважды, — та же ловушка, что индекс.
    next_id: u64,
    /// Номера тел, отвалившихся с последней очистки списка. Тот, кто
    /// сажает на них наездников, читает отсюда.
    pub spawned: Vec<u64>,
    /// Что приземлилось на последнем шаге. Тело к этому моменту уже
    /// ушло из списка, а вызывающему нужно знать, сколько оно пролетело
    /// и кто на нём ехал: снимок, снятый до шага, отстаёт на тик.
    pub landed_log: Vec<Landed>,
}

/// Тяготение для тестов крейта: земное при тридцати тиках в секунду.
///
/// Литерал здесь законен и в `data/sim.ron` не смотрит: игровое число
/// живёт там (ADR-173), а крейту `voxel` про тот файл знать нечем —
/// он получает уже переведённое ускорение снаружи. Это оснастка,
/// а не баланс.
#[cfg(test)]
pub(crate) fn test_gravity() -> Fx {
    Fx::from_ratio(20, 30 * 30)
}

/// Приземлившееся тело — то, что от него осталось знать.
#[derive(Clone, Debug)]
pub struct Landed {
    pub id: u64,
    /// Сколько вокселей тело пролетело за всю жизнь.
    pub fallen: u32,
    /// Где оно легло.
    pub oy: i64,
    pub riders: Vec<u32>,
}

impl Bodies {
    pub fn new(max_bodies: usize, gravity: Fx) -> Bodies {
        Bodies {
            list: Vec::new(),
            max_bodies,
            gravity,
            refused: 0,
            next_id: 1,
            spawned: Vec::new(),
            landed_log: Vec::new(),
        }
    }

    /// Оторвать кусок от мира: снять его воксели и завести тело.
    ///
    /// Возвращает `false`, если лимит исчерпан — тогда кусок остаётся
    /// в мире нетронутым.
    pub fn detach(&mut self, w: &mut World, cells: &[(u32, u32, u32)]) -> bool {
        if self.list.len() >= self.max_bodies {
            self.refused += 1;
            return false;
        }
        let Some(mut b) = Body::from_world_cells(w, cells) else {
            return false;
        };
        b.id = self.next_id;
        self.next_id += 1;
        self.spawned.push(b.id);
        for &(x, y, z) in cells {
            w.queue_edit(crate::Edit { x, y, z, mat: AIR });
        }
        self.list.push(b);
        true
    }

    /// Один шаг падения всех тел. Приземлившиеся запекаются обратно
    /// в мир **через очередь правок**, а не записью напрямую: иначе
    /// рендер и связность не увидят появления материи.
    ///
    /// **Падение ускоряется** (ADR-174), и опора при этом
    /// не проскакивается: тело опускается по вокселю, спрашивая
    /// `can_fall` перед каждым, — на четырёх вокселях за тик проверка
    /// только конечной высоты пропустила бы три пола из четырёх.
    pub fn step(&mut self, w: &mut World) -> BodyStats {
        let mut st = BodyStats::default();
        self.landed_log.clear();
        let mut i = 0;
        while i < self.list.len() {
            if self.list[i].fresh {
                // Отвалилось в этом тике: падать начнёт со следующего.
                self.list[i].fresh = false;
                i += 1;
                continue;
            }
            if self.list[i].resting {
                i += 1;
                continue;
            }
            if self.list[i].can_fall(w) {
                // Скорость растёт на тяготение, высота — на скорость.
                // Пока скорость меньше воксела за тик, тело стоит
                // на месте и копит дробную часть: так выглядит первая
                // треть секунды свободного падения.
                let g = self.gravity;
                let b = &mut self.list[i];
                b.vy = b.vy.add(g);
                let nf = b.fy.add(b.vy);
                let n = nf.to_int_trunc();
                b.fy = nf.sub(Fx::from_int(n));
                let mut moved = 0;
                while moved < n && self.list[i].can_fall(w) {
                    self.list[i].oy -= 1;
                    self.list[i].fallen += 1;
                    moved += 1;
                }
                if moved > 0 {
                    st.fell += 1;
                }
                if moved == n {
                    i += 1;
                    continue;
                }
                // Упёрлись посреди шага: приземление здесь, а не
                // на высоте, куда донесла бы скорость.
            }
            {
                // Приземление: запечь обратно и убрать тело.
                let b = self.list.swap_remove_stable(i);
                self.landed_log.push(Landed {
                    id: b.id,
                    fallen: b.fallen,
                    oy: b.oy,
                    riders: b.riders.clone(),
                });
                for v in &b.voxels {
                    let (x, y, z) = b.world_of(*v);
                    if x >= 0 && y >= 0 && z >= 0 {
                        w.queue_edit(crate::Edit {
                            x: x as u32,
                            y: y as u32,
                            z: z as u32,
                            mat: v.mat,
                        });
                    }
                }
                st.landed += 1;
            }
        }
        st.alive = self.list.len();
        st.refused = self.refused;
        st
    }

    /// Место тела в списке по устойчивому номеру. Тел десятки,
    /// поэтому перебор — и он же снимает вопрос о порядке.
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.list.iter().position(|b| b.id == id)
    }

    /// Записать тела в буфер (ADR-229).
    ///
    /// Пишется всё, включая `next_id`: номер не переиспользуется,
    /// и загруженная партия, начавшая раздавать номера заново,
    /// выдала бы наездникам чужое тело — ровно то, от чего лечил
    /// ADR-118.
    ///
    /// `landed_log` и `spawned` живут один тик, но пишутся тоже:
    /// сохранение снимается **между** тиками, а «почти пустой список»
    /// это тоже состояние.
    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.usize(self.max_bodies);
        b.fx(self.gravity);
        b.usize(self.refused);
        b.u64(self.next_id);
        b.u64s(&self.spawned);
        b.u32(self.list.len() as u32);
        for bd in &self.list {
            b.u64(bd.id);
            b.i64(bd.ox);
            b.i64(bd.oy);
            b.i64(bd.oz);
            b.u16(bd.sx);
            b.u16(bd.sz);
            b.u32(bd.fallen);
            b.fx(bd.vy);
            b.fx(bd.fy);
            b.bool(bd.fresh);
            b.bool(bd.resting);
            b.u16s(&bd.floor);
            b.u32s(&bd.riders);
            b.u32(bd.voxels.len() as u32);
            for v in &bd.voxels {
                b.u16(v.x);
                b.u16(v.y);
                b.u16(v.z);
                b.u16(v.mat);
            }
        }
        b.u32(self.landed_log.len() as u32);
        for l in &self.landed_log {
            b.u64(l.id);
            b.u32(l.fallen);
            b.i64(l.oy);
            b.u32s(&l.riders);
        }
    }

    pub fn load(&mut self, s: &mut fx::blob::Scan) -> bool {
        self.max_bodies = s.usize();
        self.gravity = s.fx();
        self.refused = s.usize();
        self.next_id = s.u64();
        self.spawned = s.u64s();
        let n = s.list(48);
        self.list.clear();
        for _ in 0..n {
            let id = s.u64();
            let (ox, oy, oz) = (s.i64(), s.i64(), s.i64());
            let (sx, sz) = (s.u16(), s.u16());
            let fallen = s.u32();
            let (vy, fy) = (s.fx(), s.fx());
            let (fresh, resting) = (s.bool(), s.bool());
            let floor = s.u16s();
            let riders = s.u32s();
            let nv = s.list(8);
            let voxels = (0..nv)
                .map(|_| BodyVoxel {
                    x: s.u16(),
                    y: s.u16(),
                    z: s.u16(),
                    mat: s.u16(),
                })
                .collect();
            self.list.push(Body {
                id,
                ox,
                oy,
                oz,
                voxels,
                sx,
                sz,
                floor,
                fallen,
                vy,
                fy,
                fresh,
                resting,
                riders,
            });
        }
        let nl = s.list(20);
        self.landed_log.clear();
        for _ in 0..nl {
            self.landed_log.push(Landed {
                id: s.u64(),
                fallen: s.u32(),
                oy: s.i64(),
                riders: s.u32s(),
            });
        }
        s.ok()
    }
}

/// Удаление из списка тел — **сдвигом, а не `swap_remove`**.
///
/// `swap_remove` переставляет соседей и молча меняет исход ничьей
/// в запросах вида «первое тело в радиусе» (`CLAUDE.md`). Тел десятки,
/// сдвиг дешёв, и цена определённости здесь равна нулю.
trait StableRemove {
    fn swap_remove_stable(&mut self, i: usize) -> Body;
}

impl StableRemove for Vec<Body> {
    fn swap_remove_stable(&mut self, i: usize) -> Body {
        self.remove(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Generator;

    struct Flat {
        floor: u32,
    }
    impl Generator for Flat {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < self.floor {
                2
            } else {
                AIR
            }
        }
    }

    fn world() -> World {
        World::new(32, 32, 32, 100_000, Box::new(Flat { floor: 4 }))
    }

    /// Кусок, оторванный над полом, падает ровно до пола и запекается.
    /// Материя при этом сохраняется: сколько сняли, столько и вернули.
    #[test]
    fn a_detached_block_falls_to_the_floor_and_bakes_back() {
        let mut w = world();
        // Поставить кубик 2×2×2 на высоте 10.
        for z in 10..12 {
            for y in 10..12 {
                for x in 10..12 {
                    w.queue_edit(crate::Edit { x, y, z, mat: 3 });
                }
            }
        }
        w.flush();
        let cells: Vec<(u32, u32, u32)> = (10..12)
            .flat_map(|z| (10..12).flat_map(move |y| (10..12).map(move |x| (x, y, z))))
            .collect();

        let mut bodies = Bodies::new(64, test_gravity());
        assert!(bodies.detach(&mut w, &cells));
        w.flush();
        for &(x, y, z) in &cells {
            assert!(!w.solid(x, y, z), "кусок не снят из мира");
        }

        let mut steps = 0;
        loop {
            let st = bodies.step(&mut w);
            w.flush();
            steps += 1;
            if st.alive == 0 {
                break;
            }
            assert!(steps < 100, "тело не приземлилось");
        }
        // Пол на высоте 4, значит низ кубика встаёт на 4.
        let mut solid = 0;
        for z in 10..12 {
            for y in 4..6 {
                for x in 10..12 {
                    solid += w.solid(x, y, z) as u32;
                }
            }
        }
        assert_eq!(solid, 8, "кубик не восстановился на полу");
    }

    /// Тело, стоящее на полу, не падает вовсе.
    #[test]
    fn a_resting_block_does_not_fall() {
        let mut w = world();
        let cells: Vec<(u32, u32, u32)> = vec![(5, 4, 5), (6, 4, 5)];
        for &(x, y, z) in &cells {
            w.queue_edit(crate::Edit { x, y, z, mat: 3 });
        }
        w.flush();
        let mut bodies = Bodies::new(64, test_gravity());
        bodies.detach(&mut w, &cells);
        w.flush();
        // Первый шаг свежее тело пропускает: падать оно начинает
        // со следующего тика (ADR-133).
        let st = bodies.step(&mut w);
        assert_eq!(st.fell, 0);
        assert_eq!(st.landed, 0, "свежее тело не двигается в свой тик");
        let st = bodies.step(&mut w);
        assert_eq!(st.fell, 0);
        assert_eq!(st.landed, 1, "тело должно лечь, не пролетев ни воксела");
    }

    /// **Лимит жёсткий.** Кусок сверх лимита остаётся в мире целым:
    /// «иногда падает, иногда нет» — это разное состояние у разных
    /// клиентов.
    #[test]
    fn the_body_limit_is_hard_and_the_rock_stays() {
        let mut w = world();
        let mut bodies = Bodies::new(2, test_gravity());
        for k in 0..5u32 {
            let cells = vec![(5 + k, 10, 5)];
            w.queue_edit(crate::Edit { x: 5 + k, y: 10, z: 5, mat: 3 });
            w.flush();
            let ok = bodies.detach(&mut w, &cells);
            w.flush();
            if k < 2 {
                assert!(ok, "кусок {k} должен был оторваться");
            } else {
                assert!(!ok, "кусок {k} прошёл сверх лимита");
                assert!(w.solid(5 + k, 10, 5), "отказанный кусок исчез из мира");
            }
        }
        assert_eq!(bodies.refused, 3);
    }

    /// Мир повыше: падению нужна высота, а `world()` всего тридцать
    /// два воксела в потолке.
    fn tall_world() -> World {
        World::new(32, 64, 32, 100_000, Box::new(Flat { floor: 4 }))
    }

    /// Собрать кубик 2x2x2 на высоте `at` и оторвать его от мира.
    fn cube_at(w: &mut World, bodies: &mut Bodies, at: u32) {
        let cells: Vec<(u32, u32, u32)> = (10..12)
            .flat_map(|z| (at..at + 2).flat_map(move |y| (10..12).map(move |x| (x, y, z))))
            .collect();
        for &(x, y, z) in &cells {
            w.queue_edit(crate::Edit { x, y, z, mat: 3 });
        }
        w.flush();
        assert!(bodies.detach(w, &cells), "кусок не оторвался");
        w.flush();
    }

    /// **Падение тела ускоряется** (ADR-174). Канон обещал телам падение
    /// «по вертикали с ускорением в `Fx`» (`docs/04-world.md`), а тело
    /// опускалось ровно на воксель за тик, сколько бы ни летело.
    ///
    /// Проверяется **форма**: первую половину высоты кусок летит вдвое
    /// дольше второй. У равномерного падения половины равны — на старом
    /// поведении тест падает.
    #[test]
    fn a_body_fall_speeds_up() {
        const FROM: i64 = 40;
        const FLOOR: i64 = 4;
        let half = FROM - (FROM - FLOOR) / 2;
        let mut w = tall_world();
        let mut bodies = Bodies::new(64, test_gravity());
        cube_at(&mut w, &mut bodies, FROM as u32);
        let (mut first, mut total, mut biggest) = (0, 0, 0);
        let mut prev = FROM;
        for _ in 0..400 {
            bodies.step(&mut w);
            w.flush();
            let Some(b) = bodies.list.first() else { break };
            biggest = biggest.max(prev - b.oy);
            prev = b.oy;
            total += 1;
            if prev > half {
                first = total;
            }
        }
        let second = total - first;
        assert!(first > 0 && second > 0, "тело не падало: {first} и {second} тиков");
        assert!(
            first > second * 2,
            "первая половина высоты занята {first} шагов, вторая {second}: падение не ускоряется"
        );
        assert!(biggest > 1, "самый быстрый шаг дал {biggest} воксель");
    }

    /// **Пол не проскакивается на скорости.** При шести вокселях за тик
    /// проверка «свободен ли воксель под телом» один раз за тик уронила
    /// бы кусок сквозь пол на пять вокселей вглубь; тело обязано
    /// опускаться по вокселю, спрашивая опору перед каждым (ADR-174).
    ///
    /// Тяготение здесь вдвадцатеро земного — оснастка, а не баланс.
    #[test]
    fn a_fast_body_stops_at_the_first_floor_it_meets() {
        let mut w = tall_world();
        let mut bodies = Bodies::new(64, Fx::from_ratio(400, 30 * 30));
        cube_at(&mut w, &mut bodies, 40);
        let mut steps = 0;
        while !bodies.list.is_empty() && steps < 100 {
            bodies.step(&mut w);
            w.flush();
            steps += 1;
        }
        assert!(bodies.list.is_empty(), "тело не приземлилось за {steps} шагов");
        assert!(steps < 20, "падение заняло {steps} шагов — тело не разогналось");
        // Пол на высоте 4: низ кубика обязан встать ровно на него,
        // а не внутрь породы.
        let mut solid = 0;
        for z in 10..12 {
            for y in 4..6 {
                for x in 10..12 {
                    solid += w.solid(x, y, z) as u32;
                }
            }
        }
        assert_eq!(solid, 8, "кубик лёг не на пол: он провалился сквозь него");
        // И ничего не осталось выше: тело именно опустилось, а не
        // размазалось по дороге.
        let mut above = 0;
        for z in 10..12 {
            for y in 6..42 {
                for x in 10..12 {
                    above += w.solid(x, y, z) as u32;
                }
            }
        }
        assert_eq!(above, 0, "часть кубика осталась висеть в воздухе");
    }

    /// Падение детерминировано: два одинаковых прогона дают одинаковые
    /// позиции тел на каждом шаге.
    #[test]
    fn falling_is_deterministic() {
        let run = || -> Vec<(i64, i64, i64, u32)> {
            let mut w = world();
            let mut bodies = Bodies::new(64, test_gravity());
            for k in 0..6u32 {
                let cells: Vec<(u32, u32, u32)> =
                    vec![(4 + k, 12 + k, 4), (5 + k, 12 + k, 4)];
                for &(x, y, z) in &cells {
                    w.queue_edit(crate::Edit { x, y, z, mat: 3 });
                }
                w.flush();
                bodies.detach(&mut w, &cells);
                w.flush();
            }
            let mut trace = Vec::new();
            for _ in 0..20 {
                bodies.step(&mut w);
                w.flush();
                for b in &bodies.list {
                    trace.push((b.ox, b.oy, b.oz, b.fallen));
                }
            }
            trace
        };
        assert_eq!(run(), run());
    }
}
