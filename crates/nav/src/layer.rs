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

/// Сколько уровней в колонке слой помнит.
///
/// Перепись ADR-117 дала 2.9 уровня на подколонку в среднем; восемь
/// берётся с запасом, а сколько раз его не хватило — считается
/// и печатается, а не умалчивается (ADR-136).
pub const MAX_LEVELS: usize = 8;


pub struct NavLayer {
    /// Высота пола клетки — **нижнего** её уровня.
    ///
    /// Оставлен ради тех, кому одного уровня довольно: графа порталов
    /// и отчётов. Всё, что решает, где стоит юнит, ходит через
    /// `levels`.
    floor: Vec<u16>,
    /// Уровни клетки по возрастанию, `MAX_LEVELS` ячеек на клетку.
    /// Пустые заполнены `NO_FLOOR`.
    levels: Vec<u16>,
    /// Сколько уровней у клетки на самом деле уместилось.
    n_levels: Vec<u8>,
    /// **Поверхность** — самый высокий уровень клетки, до всякой обрезки.
    ///
    /// Отдельно от `levels` потому, что обрезка может его не сохранить,
    /// а поверхность — это то место, где юниты стоят по умолчанию
    /// и по которому строится поле направлений (ADR-138).
    top: Vec<u16>,
    /// Сколько раз уровней оказалось больше, чем слой помнит.
    pub overflow: u64,
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
            levels: vec![NO_FLOOR; (nw * nh) as usize * MAX_LEVELS],
            n_levels: vec![0; (nw * nh) as usize],
            top: vec![NO_FLOOR; (nw * nh) as usize],
            overflow: 0,
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
                l.refresh_cell(w, cx, cz);
            }
        }
        l
    }

    /// Пересчитать уровни клетки. Возвращает `(пол, можно ли стоять)`.
    fn refresh_cell(&mut self, w: &World, cx: u32, cz: u32) -> (u16, bool) {
        // Собираем с запасом, а обрезаем в конце: иначе выпадали бы
        // те уровни, что нашлись позже, то есть произвольные.
        // Остаются **нижние** — там, где юниты и ходят (ADR-136).
        const SCRATCH: usize = 32;
        let mut found = [NO_FLOOR; SCRATCH];
        let mut n = 0usize;
        let mut over = false;
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
                            // Уровни, отличающиеся на воксель, — один
                            // и тот же: клетка шире воксела, и пол
                            // в ней бывает неровным.
                            let yv = y as u16;
                            let mut merged = false;
                            for f in found.iter_mut().take(n) {
                                if f.abs_diff(yv) <= 1 {
                                    *f = (*f).min(yv);
                                    merged = true;
                                    break;
                                }
                            }
                            if !merged {
                                if n < SCRATCH {
                                    found[n] = yv;
                                    n += 1;
                                } else {
                                    over = true;
                                }
                            }
                            y += self.height;
                            continue;
                        }
                    }
                    y += 1;
                }
            }
        }
        found[..n].sort_unstable();
        let top = if n == 0 { NO_FLOOR } else { found[n - 1] };
        if n > MAX_LEVELS {
            // Обрезка оставляет нижние — и **обязательно верхний**.
            // Без него юнит на поверхности колонки, где уровней больше
            // восьми, оказывался бы притянут вниз, в толщу: поверхность
            // это не «ещё один уровень», это тот, где он стоит (ADR-138).
            over = true;
            found[MAX_LEVELS - 1] = top;
            n = MAX_LEVELS;
        }
        let i = self.idx(cx, cz);
        let base = i * MAX_LEVELS;
        self.levels[base..base + MAX_LEVELS].copy_from_slice(&found[..MAX_LEVELS]);
        self.n_levels[i] = n as u8;
        self.top[i] = top;
        if over {
            self.overflow += 1;
        }
        let f = if n == 0 { NO_FLOOR } else { found[0] };
        self.floor[i] = f;
        self.stand[i] = n > 0;
        (f, n > 0)
    }

    /// Уровни клетки по возрастанию.
    pub fn levels_at(&self, cx: u32, cz: u32) -> &[u16] {
        let i = self.idx(cx, cz);
        &self.levels[i * MAX_LEVELS..i * MAX_LEVELS + self.n_levels[i] as usize]
    }

    /// Ближайший к `y` уровень клетки, если он есть.
    pub fn level_near(&self, cx: u32, cz: u32, y: i32) -> Option<u16> {
        let mut best: Option<u16> = None;
        for &l in self.levels_at(cx, cz) {
            let d = (l as i32 - y).abs();
            match best {
                Some(b) if (b as i32 - y).abs() <= d => {}
                _ => best = Some(l),
            }
        }
        best
    }

    /// Есть ли у клетки уровень ровно на высоте `y` — то есть стоит ли
    /// юнит на опоре.
    pub fn has_level(&self, cx: u32, cz: u32, y: i32) -> bool {
        self.levels_at(cx, cz).iter().any(|&l| l as i32 == y)
    }

    /// Ближайший уровень **ниже** `y` — то, на что юнит упадёт.
    /// `None` — падать не на что.
    pub fn level_below(&self, cx: u32, cz: u32, y: i32) -> Option<u16> {
        self.levels_at(cx, cz)
            .iter()
            .rev()
            .find(|&&l| (l as i32) < y)
            .copied()
    }

    /// Номер ближайшего к `y` уровня клетки — то, чем юнит адресует
    /// свой узел в объёмном поле направлений (ADR-138).
    pub fn level_index_near(&self, cx: u32, cz: u32, y: i32) -> Option<usize> {
        let mut best: Option<(u32, usize)> = None;
        for (k, &l) in self.levels_at(cx, cz).iter().enumerate() {
            let d = (l as i32 - y).unsigned_abs();
            match best {
                Some((bd, _)) if bd <= d => {}
                _ => best = Some((d, k)),
            }
        }
        best.map(|(_, k)| k)
    }

    /// Есть ли переход из клетки `a` с высоты `y` в клетку `b`.
    /// Возвращает уровень, на который юнит попадёт.
    ///
    /// **Перепад — свойство ребра** (ADR-121), а уровень выбирается
    /// по текущей высоте: с крыши переходят на крышу, а не на землю
    /// под ней (ADR-136).
    pub fn step_to(&self, a: (u32, u32), y: i32, b: (u32, u32)) -> Option<u16> {
        let mut best: Option<u16> = None;
        for &l in self.levels_at(b.0, b.1) {
            let d = (l as i32 - y).unsigned_abs();
            if d > self.max_step {
                continue;
            }
            match best {
                Some(bb) if (bb as i32 - y).unsigned_abs() <= d => {}
                _ => best = Some(l),
            }
        }
        let _ = a;
        best
    }


    #[inline]
    fn idx(&self, cx: u32, cz: u32) -> usize {
        (cz * self.w + cx) as usize
    }

    /// Поверхность клетки: самый высокий уровень, на котором можно
    /// стоять. `NO_FLOOR`, если стоять негде.
    pub fn top_at(&self, cx: u32, cz: u32) -> u16 {
        self.top[self.idx(cx, cz)]
    }

    /// Сторона нав-клетки в вокселях.
    pub fn step(&self) -> u32 {
        self.step
    }

    /// Размеры сетки в клетках.
    pub fn dims(&self) -> (u32, u32) {
        (self.w, self.h)
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
            self.refresh_cell(w, cx, cz);
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
    #[allow(dead_code)]
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
