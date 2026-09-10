//! Ступень B каскада ADR-055: чередующийся двусторонний флуд от разреза.
//!
//! Два фронта растут по очереди, по одной клетке за шаг. Кто исчерпался
//! первым — тот и есть меньшая сторона, и её размер известен точно.
//! Потолок на сторону ограничивает худший случай: пробитие потолка
//! означает «слишком большой кусок, спрашивать граф» (ступень C).
//!
//! Чередование обязательно. Односторонний флуд от произвольной стороны
//! в худшем случае обходит весь массив; чередующийся — вдвое больше
//! **меньшей** стороны, и это разные величины на четыре порядка
//! (подкоп хребта: 999 803 воксела против 77 узлов графа).

use crate::topo::{Chunk, CS};

/// Пара противоположных осевых смещений. Псевдоним нужен только затем,
/// чтобы тип таблицы осей читался.
type AxisPair = ((i32, i32, i32), (i32, i32, i32));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Фронты встретились — стороны связаны, ничего не отвалилось.
    Connected,
    /// Одна сторона исчерпалась: конечный кусок известного размера.
    Detached { cells: u32 },
    /// Потолок пробит — спрашивать граф компонент чанков.
    Overflow,
}

/// Переиспользуемые буферы. Аллокация на событие сделала бы замер
/// замером аллокатора: событий тысячи за тик.
pub struct FloodBufs {
    seen: Vec<u32>,
    epoch: u32,
    front: [Vec<u32>; 2],
    next: [Vec<u32>; 2],
}

impl Default for FloodBufs {
    fn default() -> Self {
        Self::new()
    }
}

impl FloodBufs {
    pub fn new() -> FloodBufs {
        FloodBufs {
            seen: vec![0; CS * CS * CS],
            epoch: 0,
            front: [Vec::with_capacity(1024), Vec::with_capacity(1024)],
            next: [Vec::with_capacity(1024), Vec::with_capacity(1024)],
        }
    }

    fn begin(&mut self) {
        // Шаг два: каждому фронту своя метка, иначе они не смогут
        // обнаружить встречу и меньшая сторона будет объявлена отвалом.
        self.epoch = self.epoch.wrapping_add(2);
        if self.epoch < 2 {
            self.seen.iter_mut().for_each(|v| *v = 0);
            self.epoch = 2;
        }
        for i in 0..2 {
            self.front[i].clear();
            self.next[i].clear();
        }
    }
}

#[inline(always)]
fn idx(x: usize, y: usize, z: usize) -> u32 {
    ((z * CS + y) * CS + x) as u32
}

#[inline(always)]
fn unpack(i: u32) -> (usize, usize, usize) {
    let i = i as usize;
    (i % CS, (i / CS) % CS, i / (CS * CS))
}

/// Стартовая пара фронтов: противоположные осевые соседи разреза.
/// Если такой пары нет, расщепления по этой оси быть не может.
#[inline]
fn seeds(c: &Chunk, x: usize, y: usize, z: usize) -> Option<(u32, u32)> {
    let axes: [AxisPair; 3] = [
        ((-1, 0, 0), (1, 0, 0)),
        ((0, -1, 0), (0, 1, 0)),
        ((0, 0, -1), (0, 0, 1)),
    ];
    for (a, bb) in axes {
        let pa = (x as i32 + a.0, y as i32 + a.1, z as i32 + a.2);
        let pb = (x as i32 + bb.0, y as i32 + bb.1, z as i32 + bb.2);
        let ok = |p: (i32, i32, i32)| {
            p.0 >= 0
                && p.1 >= 0
                && p.2 >= 0
                && p.0 < CS as i32
                && p.1 < CS as i32
                && p.2 < CS as i32
                && c.get(p.0 as usize, p.1 as usize, p.2 as usize)
        };
        if ok(pa) && ok(pb) {
            return Some((
                idx(pa.0 as usize, pa.1 as usize, pa.2 as usize),
                idx(pb.0 as usize, pb.1 as usize, pb.2 as usize),
            ));
        }
    }
    None
}

/// **Строгое чередование по клетке.** Именно на нём стоит довод ADR-055:
/// обход ограничен удвоенной меньшей стороной, а не размером массива.
/// Фронт — очередь; за шаг раскрывается ровно одна клетка, затем сторона
/// меняется.
pub fn probe(
    c: &Chunk,
    b: &mut FloodBufs,
    x: usize,
    y: usize,
    z: usize,
    cap: u32,
) -> (Verdict, u32) {
    b.begin();
    let Some((ia, ib)) = seeds(c, x, y, z) else {
        return (Verdict::Connected, 0);
    };
    b.seen[ia as usize] = b.epoch;
    b.seen[ib as usize] = b.epoch + 1;
    b.front[0].push(ia);
    b.front[1].push(ib);

    let mut head = [0usize, 0usize];
    let mut visited = [1u32, 1u32];
    let mut total = 2u32;

    loop {
        // `side` индексирует три массива сразу — это индекс по делу.
        #[allow(clippy::needless_range_loop)]
        for side in 0..2 {
            if head[side] >= b.front[side].len() {
                return (
                    Verdict::Detached {
                        cells: visited[side],
                    },
                    total,
                );
            }
            let cur = b.front[side][head[side]];
            head[side] += 1;
            let mine = b.epoch + side as u32;
            let theirs = b.epoch + 1 - side as u32;
            let (px, py, pz) = unpack(cur);
            let nbs = [
                (px.wrapping_sub(1), py, pz),
                (px + 1, py, pz),
                (px, py.wrapping_sub(1), pz),
                (px, py + 1, pz),
                (px, py, pz.wrapping_sub(1)),
                (px, py, pz + 1),
            ];
            for (nx, ny, nz) in nbs {
                if nx >= CS || ny >= CS || nz >= CS || !c.get(nx, ny, nz) {
                    continue;
                }
                let ni = idx(nx, ny, nz);
                let s = b.seen[ni as usize];
                if s == mine {
                    continue;
                }
                if s == theirs {
                    return (Verdict::Connected, total);
                }
                b.seen[ni as usize] = mine;
                b.front[side].push(ni);
                visited[side] += 1;
                total += 1;
            }
            if visited[side] > cap {
                return (Verdict::Overflow, total);
            }
        }
    }
}

/// Чередование по слоям: проще в коде, но больший фронт успевает перерасти
/// внутри слоя. Существует ради сравнения — цену этой простоты надо знать,
/// а не предполагать.
pub fn probe_layered(
    c: &Chunk,
    b: &mut FloodBufs,
    x: usize,
    y: usize,
    z: usize,
    cap: u32,
) -> (Verdict, u32) {
    b.begin();
    let Some((ia, ib)) = seeds(c, x, y, z) else {
        return (Verdict::Connected, 0);
    };
    b.seen[ia as usize] = b.epoch;
    b.seen[ib as usize] = b.epoch + 1;
    b.front[0].push(ia);
    b.front[1].push(ib);

    let mut visited = [1u32, 1u32];
    let mut total = 2u32;
    loop {
        #[allow(clippy::needless_range_loop)]
        for side in 0..2 {
            if b.front[side].is_empty() {
                return (
                    Verdict::Detached {
                        cells: visited[side],
                    },
                    total,
                );
            }
            b.next[side].clear();
            let mine = b.epoch + side as u32;
            let theirs = b.epoch + 1 - side as u32;
            for k in 0..b.front[side].len() {
                let (px, py, pz) = unpack(b.front[side][k]);
                let nbs = [
                    (px.wrapping_sub(1), py, pz),
                    (px + 1, py, pz),
                    (px, py.wrapping_sub(1), pz),
                    (px, py + 1, pz),
                    (px, py, pz.wrapping_sub(1)),
                    (px, py, pz + 1),
                ];
                for (nx, ny, nz) in nbs {
                    if nx >= CS || ny >= CS || nz >= CS || !c.get(nx, ny, nz) {
                        continue;
                    }
                    let ni = idx(nx, ny, nz);
                    let s = b.seen[ni as usize];
                    if s == mine {
                        continue;
                    }
                    if s == theirs {
                        return (Verdict::Connected, total);
                    }
                    b.seen[ni as usize] = mine;
                    b.next[side].push(ni);
                    visited[side] += 1;
                    total += 1;
                }
            }
            std::mem::swap(&mut b.front[side], &mut b.next[side]);
            if visited[side] > cap {
                return (Verdict::Overflow, total);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::scene::{build, Scene};
    use crate::topo::Components;

    /// Ложный отвал недопустим: если флуд сказал «отвалилось», число
    /// компонент обязано вырасти. Обратное не проверяется — разрез мог
    /// расщепить по другой паре осей, и это законный промах ступени B,
    /// который ловит ступень C.
    #[test]
    fn never_claims_a_detach_that_did_not_happen() {
        let mut bufs = FloodBufs::new();
        let mut before = Components::new();
        let mut after = Components::new();
        let mut r = fx::rng::Rng::new(21, 3);
        let mut connected = 0usize;
        let mut checked = 0usize;
        for scene in Scene::ALL {
            let base = build(scene, 2);
            let n0 = before.build(&base);
            for _ in 0..300 {
                let (x, y, z) = (
                    r.below(CS as u32) as usize,
                    r.below(CS as u32) as usize,
                    r.below(CS as u32) as usize,
                );
                if !base.get(x, y, z) {
                    continue;
                }
                checked += 1;
                let mut cut = base.clone();
                cut.set(x, y, z, false);
                let n1 = after.build(&cut);
                let (v, _) = probe(&cut, &mut bufs, x, y, z, 512);
                match v {
                    Verdict::Detached { .. } => assert!(
                        n1 > n0,
                        "{}: отвал заявлен там, где компонент не прибавилось ({x},{y},{z}): {n0} -> {n1}",
                        scene.name()
                    ),
                    Verdict::Connected => connected += 1,
                    Verdict::Overflow => {}
                }
            }
        }
        assert!(checked > 500, "тест почти ничего не проверил: {checked}");
        assert!(connected > 100, "тест не набрал связных случаев");
    }

    /// Настоящий отвал: перемычка в один воксель между двумя кубами.
    /// Снятие перемычки обязано дать вердикт «отвал» с точным размером
    /// меньшей стороны.
    #[test]
    fn cutting_a_neck_detaches_the_smaller_side() {
        let mut c = Chunk::new();
        // Малый куб 3³ слева, большой 6³ справа, перемычка по оси Y.
        for x in 9..12 {
            for z in 9..12 {
                for y in 4..7 {
                    c.set(x, y, z, true);
                }
            }
        }
        for x in 8..14 {
            for z in 8..14 {
                for y in 10..16 {
                    c.set(x, y, z, true);
                }
            }
        }
        for y in 7..10 {
            c.set(10, y, 10, true);
        }
        let mut comp = Components::new();
        assert_eq!(comp.build(&c), 1, "куски не соединены перемычкой");

        c.set(10, 8, 10, false);
        assert_eq!(comp.build(&c), 2, "снятие перемычки не расщепило");

        let mut bufs = FloodBufs::new();
        let (v, total) = probe(&c, &mut bufs, 10, 8, 10, 512);
        match v {
            Verdict::Detached { cells } => {
                // Меньшая сторона — куб 3³ плюс один воксель перемычки.
                assert_eq!(cells, 28, "неверный размер меньшей стороны");
                // Строгое чередование ограничивает **раскрытия** удвоенной
                // меньшей стороной. Обнаруженных клеток выходит больше:
                // за одно раскрытие в плотной геометрии открывается около
                // двух новых, и больший фронт успевает их пометить.
                assert!(
                    total <= 4 * cells,
                    "обход не ограничен меньшей стороной: {total} при {cells}"
                );
            }
            other => panic!("вердикт {other:?} вместо отвала"),
        }
    }

    /// Потолок обязан срабатывать, а не уходить в обход всего массива.
    #[test]
    fn cap_bounds_the_worst_case() {
        let c = build(Scene::Solid, 1);
        let mut bufs = FloodBufs::new();
        let (v, total) = probe(&c, &mut bufs, 16, 16, 16, 64);
        assert!(
            matches!(v, Verdict::Connected) || matches!(v, Verdict::Overflow),
            "{v:?}"
        );
        assert!(total <= 4 * 64 + 16, "потолок не ограничил обход: {total}");
    }

    /// Отдельно стоящий куб: снятие его воксела не может ничего отсоединить
    /// от остального мира, потому что противоположных твёрдых соседей нет.
    #[test]
    fn isolated_cube_gives_no_split() {
        let mut c = Chunk::new();
        c.set(5, 5, 5, true);
        let mut bufs = FloodBufs::new();
        let (v, total) = probe(&c, &mut bufs, 5, 5, 5, 512);
        assert_eq!(v, Verdict::Connected);
        assert_eq!(total, 0);
    }
}
