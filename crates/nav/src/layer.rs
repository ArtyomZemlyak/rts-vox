//! Мост от мира к подсистеме: грязное множество и ленивое состояние.
//!
//! Размеры чанков у подсистем **не совпадают, и это не недосмотр**:
//! мешинг режет мир на 62³ с паддингом соседей, связность — на 32³,
//! навигация — на клетки по два воксела. Это разные подсистемы
//! с разными единицами, и каждая считает своё грязное множество.
//!
//! **Слой ленивый.** Нетронутый чанк не стоит ничего, ровно как
//! в самом мире.

use voxel::{Edit, World};

/// Слой навигации: проходимость нав-клетки.
///
/// **Переворот считается на клетку, а не на правку**, и с дедупликацией
/// внутри тика: сотня правок в одном столбе переворачивает клетку один
/// раз или ноль. Считать по правкам — завысить в тридцать раз.
///
/// **Критерий — просвет над полом, а не твёрдое на фиксированной высоте.**
/// Первая редакция брала уровень глаз константой, и на рельефе с размахом
/// в сорок вокселей это дало ровно ноль переворотов: половина карты выше
/// такого уровня, половина ниже. Клетка проходима, если над её полом
/// есть просвет в рост юнита.
/// Отсутствие пола: колонка либо пустая до дна, либо сплошная до неба.
pub const NO_FLOOR: u16 = u16::MAX;

pub struct NavLayer {
    /// Высота пола клетки. **Это и есть та величина, которую меняет
    /// копка** — булева проходимость её не видит.
    floor: Vec<u16>,
    /// Есть ли над полом просвет в рост.
    stand: Vec<bool>,
    stamp: Vec<u32>,
    epoch: u32,
    w: u32,
    h: u32,
    /// Рост юнита в вокселях: столько нужно свободного над полом.
    pub height: u32,
    /// Какой перепад юнит перешагивает. Больше — обрыв, ребра нет.
    pub max_step: u32,
    step: u32,
}

impl NavLayer {
    pub fn new(w: &World, height: u32, step: u32) -> NavLayer {
        let (nw, nh) = (w.nx / step, w.nz / step);
        let mut l = NavLayer {
            floor: vec![NO_FLOOR; (nw * nh) as usize],
            stand: vec![false; (nw * nh) as usize],
            stamp: vec![0; (nw * nh) as usize],
            epoch: 0,
            w: nw,
            h: nh,
            height,
            max_step: 1,
            step,
        };
        for cz in 0..nh {
            for cx in 0..nw {
                let (f, st) = l.probe(w, cx, cz);
                let i = (cz * nw + cx) as usize;
                l.floor[i] = f;
                l.stand[i] = st;
            }
        }
        l
    }

    /// Пол клетки и есть ли над ним просвет.
    ///
    /// **Пол ищется снизу вверх.** Поверхностная мерка берёт верх
    /// колонки и потому не видит прокопанный под грядой ход, а верх
    /// отвесной стены считает проходимым (ADR-113). Снизу вверх
    /// берётся нижний проходимый уровень — тот самый, который
    /// единственный по ADR-039.
    ///
    /// Клетка шире воксела, поэтому берётся **самая низкая** её
    /// подколонка с просветом: юнит стоит там, где может стоять.
    fn probe(&self, w: &World, cx: u32, cz: u32) -> (u16, bool) {
        let mut best = NO_FLOOR;
        for dz in 0..self.step {
            for dx in 0..self.step {
                let (x, z) = (cx * self.step + dx, cz * self.step + dz);
                let mut y = 1u32;
                while y < w.ny {
                    if w.solid(x, y - 1, z) && !w.solid(x, y, z) {
                        let mut clear = 0;
                        while clear < self.height && y + clear < w.ny && !w.solid(x, y + clear, z)
                        {
                            clear += 1;
                        }
                        if clear >= self.height {
                            if (y as u16) < best {
                                best = y as u16;
                            }
                            break;
                        }
                    }
                    y += 1;
                }
            }
        }
        (best, best != NO_FLOOR)
    }

    #[inline]
    fn idx(&self, cx: u32, cz: u32) -> usize {
        (cz * self.w + cx) as usize
    }

    pub fn floor_at(&self, cx: u32, cz: u32) -> u16 {
        self.floor[self.idx(cx, cz)]
    }

    pub fn standable(&self, cx: u32, cz: u32) -> bool {
        self.stand[self.idx(cx, cz)]
    }

    /// Есть ли ребро между соседними клетками.
    ///
    /// **Перепад — свойство ребра, а не клетки** (ADR-113). Локальный
    /// критерий не работает ни в какую сторону: строгий запрещает ходить
    /// вдоль любой стены, мягкий оставляет проходимой вершину отвесной
    /// гряды.
    pub fn edge(&self, a: (u32, u32), b: (u32, u32)) -> bool {
        let (ia, ib) = (self.idx(a.0, a.1), self.idx(b.0, b.1));
        if !self.stand[ia] || !self.stand[ib] {
            return false;
        }
        let (fa, fb) = (self.floor[ia] as i32, self.floor[ib] as i32);
        (fa - fb).unsigned_abs() <= self.max_step
    }

    /// Сколько рёбер выходит из клетки. Нужно, чтобы считать разрывы:
    /// именно они, а не булева проходимость, — то, что копка меняет.
    fn degree(&self, cx: u32, cz: u32) -> u32 {
        let mut d = 0;
        for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (nx, nz) = (cx as i32 + dx, cz as i32 + dz);
            if nx < 0 || nz < 0 || nx >= self.w as i32 || nz >= self.h as i32 {
                continue;
            }
            if self.edge((cx, cz), (nx as u32, nz as u32)) {
                d += 1;
            }
        }
        d
    }

    /// Возвращает `NavStats`.
    ///
    /// Три величины, а не одна: работа, изменение пола и изменение
    /// рёбер расходятся на порядки. Булева проходимость сама по себе
    /// не меняется почти никогда — над поверхностью всегда небо, —
    /// и сцена, мерившая только её, мерила ноль (ADR-117).
    pub fn apply(&mut self, w: &World, edits: &[Edit]) -> NavStats {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.iter_mut().for_each(|v| *v = 0);
            self.epoch = 1;
        }
        let mut st = NavStats::default();
        // Сначала собрать затронутые клетки: степень зависит от соседей,
        // поэтому «до» надо снять до любой записи.
        let mut touched: Vec<(u32, u32, u16, bool, u32)> = Vec::new();
        for e in edits {
            let (cx, cz) = (e.x / self.step, e.z / self.step);
            if cx >= self.w || cz >= self.h {
                continue;
            }
            let ci = self.idx(cx, cz);
            // Штамп вместо перебора: без него сотня правок в одном столбе
            // пересчитывала бы клетку сотню раз (ADR-078).
            if self.stamp[ci] == self.epoch {
                continue;
            }
            self.stamp[ci] = self.epoch;
            touched.push((cx, cz, self.floor[ci], self.stand[ci], self.degree(cx, cz)));
        }
        st.seen = touched.len();
        for &(cx, cz, _, _, _) in &touched {
            let (f, sd) = self.probe(w, cx, cz);
            let ci = self.idx(cx, cz);
            self.floor[ci] = f;
            self.stand[ci] = sd;
        }
        for &(cx, cz, f0, s0, d0) in &touched {
            let ci = self.idx(cx, cz);
            if self.stand[ci] != s0 {
                st.flips += 1;
            }
            if self.floor[ci] != f0 {
                st.floor_moved += 1;
                st.floor_drop += (f0.min(NO_FLOOR - 1) as i32 - self.floor[ci].min(NO_FLOOR - 1) as i32)
                    .max(0) as u64;
            }
            let d1 = self.degree(cx, cz);
            st.edges_lost += d0.saturating_sub(d1) as u64;
            st.edges_gained += d1.saturating_sub(d0) as u64;
        }
        st
    }

    /// Сколько уровней, на которых можно стоять, есть в подколонке.
    ///
    /// Величина существует затем, чтобы **сосчитать, насколько неверна
    /// одноуровневая навигация** (ADR-039). Выбор уровня в колонке —
    /// не деталь реализации: сверху вниз теряется прокопанный ход
    /// (ADR-113), снизу вверх выбирается пол пещеры под поверхностью.
    /// Оба ответа неверны там, где уровней больше одного, и вопрос
    /// в том, где таких колонок много.
    fn levels_in_column(&self, w: &World, x: u32, z: u32) -> u32 {
        let mut n = 0;
        let mut y = 1u32;
        while y < w.ny {
            if w.solid(x, y - 1, z) && !w.solid(x, y, z) {
                let mut clear = 0;
                while clear < self.height && y + clear < w.ny && !w.solid(x, y + clear, z) {
                    clear += 1;
                }
                if clear >= self.height {
                    n += 1;
                    y += self.height;
                    continue;
                }
            }
            y += 1;
        }
        n
    }

    /// (клеток с полом, из них многоуровневых, уровней всего).
    /// Полный обход — вызывается один раз, не в горячем пути.
    pub fn level_census(&self, w: &World) -> (usize, usize, u64) {
        let (mut with, mut multi, mut total) = (0usize, 0usize, 0u64);
        for cz in 0..self.h {
            for cx in 0..self.w {
                let mut best = 0;
                for dz in 0..self.step {
                    for dx in 0..self.step {
                        let n = self.levels_in_column(w, cx * self.step + dx, cz * self.step + dz);
                        best = best.max(n);
                        total += n as u64;
                    }
                }
                if best > 0 {
                    with += 1;
                }
                if best > 1 {
                    multi += 1;
                }
            }
        }
        (with, multi, total)
    }

    pub fn standable_count(&self) -> usize {
        self.stand.iter().filter(|&&p| p).count()
    }

    /// Рёбер во всём графе. Считается по два раза на ребро, поэтому
    /// делится пополам.
    pub fn edge_count(&self) -> usize {
        let mut n = 0usize;
        for cz in 0..self.h {
            for cx in 0..self.w {
                n += self.degree(cx, cz) as usize;
            }
        }
        n / 2
    }
}

/// Что дал тик навигации.
#[derive(Default, Clone, Copy)]
pub struct NavStats {
    /// Клеток пересчитано.
    pub seen: usize,
    /// У скольких изменилась сама возможность стоять.
    pub flips: usize,
    /// У скольких сдвинулся пол.
    pub floor_moved: usize,
    /// Суммарное опускание пола в вокселях.
    pub floor_drop: u64,
    /// Рёбер разорвано и появилось.
    pub edges_lost: u64,
    pub edges_gained: u64,
}
