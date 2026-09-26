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

/// Сколько потоков строят слой на старте.
///
/// Число зафиксировано, а не взято у ОС (`CLAUDE.md`): от него зависит
/// только время, но не результат — клетка считается от мира и ни от
/// чего больше, потоки пишут каждый свои, и это проверяется тестом.
/// Восемь — по числу ядер машины замеров (ADR-228).
const BUILD_THREADS: usize = 8;

/// Меньше этого клеток слой строится в один поток: запуск потоков
/// стоит дороже самой работы (ADR-080).
const BUILD_PARALLEL_FROM: usize = 1 << 13;

/// Уровни одной клетки, посчитанные с нуля по миру.
#[derive(Clone, Copy)]
struct Scan {
    levels: [u16; MAX_LEVELS],
    n: u8,
    top: u16,
    over: bool,
}

impl Scan {
    const EMPTY: Scan = Scan {
        levels: [NO_FLOOR; MAX_LEVELS],
        n: 0,
        top: NO_FLOOR,
        over: false,
    };
}

/// Внутренности слоя — то, что кладётся в кэш карты и достаётся
/// обратно (ADR-228). Штампов здесь нет: они не состояние, а память
/// одного тика.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavRaw {
    pub w: u32,
    pub h: u32,
    pub height: u32,
    pub step: u32,
    pub max_step: u32,
    pub overflow: u64,
    pub floor: Vec<u16>,
    pub levels: Vec<u16>,
    pub n_levels: Vec<u8>,
    pub top: Vec<u16>,
    pub stand: Vec<bool>,
}

/// Reusable scratch, independent of navigation state and save data.
#[derive(Default)]
struct ScanScratch {
    rows: Vec<u64>,
    floors: Vec<u64>,
}

/// Exact clearance for up to 64 columns at once (ADR-247).
///
/// Only solid-to-air transitions can be floors. Test their clearance in
/// parallel across the X bits, stopping as soon as every candidate hits a
/// ceiling. Air runs from different floors of one column cannot overlap:
/// the total clearance work stays bounded by step * ny, independently of
/// height. Vertical floor bitsets restore the original z/x/y traversal,
/// including merging and overflow. Cave/roof geometry is unchanged.
fn scan_cell(
    w: &World,
    step: u32,
    height: u32,
    cx: u32,
    cz: u32,
    scratch: &mut ScanScratch,
) -> Scan {
    assert!(height > 0, "navigation clearance must be positive");
    if height >= w.ny {
        return Scan::EMPTY;
    }
    const SCRATCH: usize = 32;
    let mut found = [NO_FLOOR; SCRATCH];
    let mut n = 0usize;
    let mut over = false;
    let ny = w.ny as usize;
    let h = height as usize;
    let blocks = ny.div_ceil(64);
    scratch.rows.resize(ny, 0);
    scratch.floors.resize(step as usize * blocks, 0);
    for dz in 0..step {
        let (x0, z) = (cx * step, cz * step + dz);
        for (y, row) in scratch.rows.iter_mut().enumerate() {
            *row = w.solid_bits(x0, y as u32, z, step);
        }
        scratch.floors.fill(0);
        for y in 1..=ny - h {
            let mut standing = scratch.rows[y - 1] & !scratch.rows[y];
            if standing == 0 {
                continue;
            }
            for &occupied in &scratch.rows[y + 1..y + h] {
                standing &= !occupied;
                if standing == 0 {
                    break;
                }
            }
            while standing != 0 {
                let dx = standing.trailing_zeros() as usize;
                standing &= standing - 1;
                scratch.floors[dx * blocks + y / 64] |= 1u64 << (y % 64);
            }
        }
        for dx in 0..step as usize {
            for word in 0..blocks {
                let mut candidates = scratch.floors[dx * blocks + word];
                while candidates != 0 {
                    let yv = (word * 64 + candidates.trailing_zeros() as usize) as u16;
                    candidates &= candidates - 1;
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
                }
            }
        }
    }
    found[..n].sort_unstable();
    let top = if n == 0 { NO_FLOOR } else { found[n - 1] };
    if n > MAX_LEVELS {
        over = true;
        found[MAX_LEVELS - 1] = top;
        n = MAX_LEVELS;
    }
    let mut levels = [NO_FLOOR; MAX_LEVELS];
    levels.copy_from_slice(&found[..MAX_LEVELS]);
    Scan {
        levels,
        n: n as u8,
        top,
        over,
    }
}

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
    scan: ScanScratch,
}

impl NavLayer {
    /// `max_step` — какой перепад юнит перешагивает. Число игровое
    /// и приходит из данных (ADR-140), а не стоит здесь литералом.
    pub fn new(w: &World, height: u32, step: u32, max_step: u32) -> NavLayer {
        assert!(
            (1..=64).contains(&step),
            "сторона нав-клетки {step} не влезает в слово занятости"
        );
        let (nw, nh) = (w.nx / step, w.nz / step);
        let cells = (nw * nh) as usize;
        let mut l = NavLayer {
            floor: vec![NO_FLOOR; cells],
            levels: vec![NO_FLOOR; cells * MAX_LEVELS],
            n_levels: vec![0; cells],
            top: vec![NO_FLOOR; cells],
            overflow: 0,
            stand: vec![false; cells],
            stamp: vec![0; cells],
            epoch: 0,
            w: nw,
            h: nh,
            height,
            max_step,
            step,
            scan: ScanScratch::default(),
        };
        if cells == 0 {
            return l;
        }
        // Клетки считаются в общий буфер, а пишутся в слой серийно:
        // потоки получают непересекающиеся куски буфера, а порядок
        // записи — и счёт переполнений — остаётся порядком клеток.
        let mut scans = vec![Scan::EMPTY; cells];
        let threads = if cells < BUILD_PARALLEL_FROM {
            1
        } else {
            BUILD_THREADS.min(nh as usize).max(1)
        };
        let band = (nh as usize).div_ceil(threads) * nw as usize;
        let scan_band = |b: usize, out: &mut [Scan]| {
            let mut scratch = ScanScratch::default();
            for (k, s) in out.iter_mut().enumerate() {
                let i = (b * band + k) as u32;
                *s = scan_cell(w, step, height, i % nw, i / nw, &mut scratch);
            }
        };
        if threads == 1 {
            scan_band(0, &mut scans);
        } else {
            std::thread::scope(|sc| {
                for (b, out) in scans.chunks_mut(band).enumerate() {
                    let scan_band = &scan_band;
                    sc.spawn(move || scan_band(b, out));
                }
            });
        }
        for (i, s) in scans.iter().enumerate() {
            l.store(i, s);
        }
        l
    }

    /// Пересчитать уровни клетки. Возвращает `(пол, можно ли стоять)`.
    fn refresh_cell(&mut self, w: &World, cx: u32, cz: u32) -> (u16, bool) {
        let s = scan_cell(w, self.step, self.height, cx, cz, &mut self.scan);
        let i = self.idx(cx, cz);
        self.store(i, &s)
    }

    /// Записать посчитанные уровни в клетку `i`.
    fn store(&mut self, i: usize, s: &Scan) -> (u16, bool) {
        let base = i * MAX_LEVELS;
        self.levels[base..base + MAX_LEVELS].copy_from_slice(&s.levels);
        self.n_levels[i] = s.n;
        self.top[i] = s.top;
        if s.over {
            self.overflow += 1;
        }
        let f = if s.n == 0 { NO_FLOOR } else { s.levels[0] };
        self.floor[i] = f;
        self.stand[i] = s.n > 0;
        (f, s.n > 0)
    }

    /// Внутренности слоя — для кэша карты (ADR-228). Копия, а не
    /// заём: слой этого размера копируется за миллисекунды, а кэш
    /// пишется раз на карту.
    pub fn raw(&self) -> NavRaw {
        NavRaw {
            w: self.w,
            h: self.h,
            height: self.height,
            step: self.step,
            max_step: self.max_step,
            overflow: self.overflow,
            floor: self.floor.clone(),
            levels: self.levels.clone(),
            n_levels: self.n_levels.clone(),
            top: self.top.clone(),
            stand: self.stand.clone(),
        }
    }

    /// Слой из внутренностей — из кэша карты (ADR-228). Длины обязаны
    /// сойтись с размерами, иначе `None`: кэш читают с диска, а диск
    /// врёт.
    pub fn from_raw(r: NavRaw) -> Option<NavLayer> {
        let cells = r.w as usize * r.h as usize;
        if r.floor.len() != cells
            || r.levels.len() != cells * MAX_LEVELS
            || r.n_levels.len() != cells
            || r.top.len() != cells
            || r.stand.len() != cells
            || !(1..=64).contains(&r.step)
        {
            return None;
        }
        Some(NavLayer {
            floor: r.floor,
            levels: r.levels,
            n_levels: r.n_levels,
            top: r.top,
            overflow: r.overflow,
            stand: r.stand,
            stamp: vec![0; cells],
            epoch: 0,
            w: r.w,
            h: r.h,
            height: r.height,
            max_step: r.max_step,
            step: r.step,
            scan: ScanScratch::default(),
        })
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

    /// Клетка мирового столбца `(x, z)`: `x / step` (ADR-437). `None`
    /// вне сетки — отрицательный столбец или полоса за последней целой
    /// клеткой, когда сторона мира не кратна клетке. Единственное
    /// отображение мира в клетки: четыре своих у симуляции расходились.
    pub fn cell_of(&self, x: i32, z: i32) -> Option<(u32, u32)> {
        if x < 0 || z < 0 {
            return None;
        }
        let (cx, cz) = (x as u32 / self.step, z as u32 / self.step);
        (cx < self.w && cz < self.h).then_some((cx, cz))
    }

    /// Ближайшая к столбцу клетка сетки: [`NavLayer::cell_of`], а за краем
    /// сетки — крайняя клетка.
    pub fn cell_near(&self, x: i32, z: i32) -> (u32, u32) {
        let near = |v: i32, n: u32| (v.max(0) as u32 / self.step).min(n.saturating_sub(1));
        (near(x, self.w), near(z, self.h))
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
                st.floor_drop += (f0.min(NO_FLOOR - 1) as i32
                    - self.floor[ci].min(NO_FLOOR - 1) as i32)
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

#[cfg(test)]
mod tests {
    use super::*;
    use voxel::store::{Mat, AIR};

    /// Рельеф с пещерами: уровней в колонке несколько, и у части клеток
    /// их больше, чем слой помнит, — иначе обрезка осталась бы
    /// непроверенной.
    struct Caves;

    impl voxel::Generator for Caves {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            let h = 24 + (x / 3) % 9 + (z / 4) % 6;
            if y >= h || (y / 3 + x / 5 + z / 7).is_multiple_of(4) {
                return AIR;
            }
            1
        }
    }

    /// Повоксельный обход клетки — тот, что был до ADR-228. Оставлен
    /// здесь целиком как эталон: словесный обход обязан давать ровно
    /// то же, включая обрезку и слияние соседних уровней.
    fn reference(w: &World, step: u32, height: u32, cx: u32, cz: u32) -> Scan {
        const SCRATCH: usize = 32;
        let mut found = [NO_FLOOR; SCRATCH];
        let mut n = 0usize;
        let mut over = false;
        for dz in 0..step {
            for dx in 0..step {
                let (x, z) = (cx * step + dx, cz * step + dz);
                let mut y = 1u32;
                while y < w.ny {
                    if w.solid(x, y - 1, z) && !w.solid(x, y, z) {
                        let mut clear = 0;
                        while clear < height && y + clear < w.ny && !w.solid(x, y + clear, z) {
                            clear += 1;
                        }
                        if clear >= height {
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
                            y += height;
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
            over = true;
            found[MAX_LEVELS - 1] = top;
            n = MAX_LEVELS;
        }
        let mut levels = [NO_FLOOR; MAX_LEVELS];
        levels.copy_from_slice(&found[..MAX_LEVELS]);
        Scan {
            levels,
            n: n as u8,
            top,
            over,
        }
    }

    /// **Словесный обход клетки совпадает с повоксельным** — на всех
    /// сторонах клетки, ростах юнита, у края мира и там, где уровней
    /// больше, чем слой помнит.
    #[test]
    fn the_word_scan_matches_the_voxel_scan() {
        // Размеры не кратны стороне клетки: последняя клетка ряда
        // упирается в край мира.
        let w = World::new(97, 48, 61, 16, Box::new(Caves));
        let mut scratch = ScanScratch::default();
        let (mut levels, mut over) = (0usize, 0usize);
        for step in [1u32, 2, 3, 10] {
            for height in [1u32, 2, 4] {
                let (nw, nh) = (w.nx / step, w.nz / step);
                for cz in 0..nh {
                    for cx in 0..nw {
                        let got = scan_cell(&w, step, height, cx, cz, &mut scratch);
                        let want = reference(&w, step, height, cx, cz);
                        assert_eq!(
                            got.levels, want.levels,
                            "клетка {cx},{cz}, сторона {step}, рост {height}"
                        );
                        assert_eq!(got.n, want.n, "клетка {cx},{cz}: число уровней");
                        assert_eq!(got.top, want.top, "клетка {cx},{cz}: поверхность");
                        assert_eq!(got.over, want.over, "клетка {cx},{cz}: переполнение");
                        levels += got.n as usize;
                        over += usize::from(got.over);
                    }
                }
            }
        }
        assert!(
            levels > 100,
            "уровней всего {levels} — сверять почти нечего"
        );
        assert!(over > 0, "переполнения не случилось — обрезка не проверена");
    }

    #[test]
    fn clearance_windows_preserve_word_edges_tall_gaps_and_merge_order() {
        struct Shelves;
        impl voxel::Generator for Shelves {
            fn material(&self, x: u32, y: u32, z: u32) -> Mat {
                let solid = y == 0
                    || match x % 3 {
                        0 => y < 130 && (y + z % 5) % 17 < 5,
                        1 => y < 130 && y.is_multiple_of(2),
                        _ => y < 63 + (x + z) % 3,
                    };
                if solid {
                    1
                } else {
                    AIR
                }
            }
        }
        let w = World::new(97, 193, 67, 16, Box::new(Shelves));
        let mut scratch = ScanScratch::default();
        let (mut populated, mut overflowed) = (0, 0);
        for step in [1, 2, 3, 10, 31, 32, 63, 64] {
            for height in [1, 2, 3, 20, 31, 32, 63, 64, 65, 128, 192, 193, 194] {
                for cz in [0, 1, (w.nz - 1) / step] {
                    for cx in [0, 1, (w.nx - 1) / step] {
                        let a = scan_cell(&w, step, height, cx, cz, &mut scratch);
                        let b = reference(&w, step, height, cx, cz);
                        assert_eq!(
                            (a.levels, a.n, a.top, a.over),
                            (b.levels, b.n, b.top, b.over),
                            "step {step}, height {height}, cell {cx},{cz}"
                        );
                        populated += usize::from(a.n != 0);
                        overflowed += usize::from(a.over);
                    }
                }
            }
        }
        assert!(populated > 100);
        assert!(overflowed > 0);
    }

    #[test]
    fn changed_columns_publish_the_same_levels_as_a_fresh_scan_in_the_same_tick() {
        let mut w = World::new(70, 80, 66, 4096, Box::new(Caves));
        let mut layer = NavLayer::new(&w, 20, 10, 5);
        let mut rng = fx::rng::Rng::new(247, 7);
        let mut changes = 0;
        for _ in 0..12 {
            for _ in 0..1000 {
                let (x, y, z) = (rng.below(70), rng.below(80), rng.below(66));
                w.queue_edit(Edit::put(x, y, z, if w.solid(x, y, z) { AIR } else { 1 }));
            }
            w.flush();
            let before = layer.raw();
            let stats = layer.apply(&w, w.edits());
            assert!(stats.seen > 0);
            let fresh = NavLayer::new(&w, 20, 10, 5).raw();
            let got = layer.raw();
            changes += usize::from(got.levels != before.levels);
            assert_eq!(got.levels, fresh.levels);
            assert_eq!(got.n_levels, fresh.n_levels);
            assert_eq!(got.floor, fresh.floor);
            assert_eq!(got.stand, fresh.stand);
            assert_eq!(got.top, fresh.top);
        }
        assert!(changes > 0, "the edit stream never changed navigation");
    }

    /// **Одна клетка на столбец** (ADR-437): `x / step`, без клетки за
    /// последней целой, когда сторона мира не кратна клетке; ближайшая —
    /// та же, а за краем сетки крайняя.
    #[test]
    fn a_column_maps_to_one_cell() {
        let w = World::new(95, 40, 64, 16, Box::new(Caves));
        let l = NavLayer::new(&w, 4, 10, 1);
        assert_eq!(l.dims(), (9, 6));
        assert_eq!(l.cell_of(0, 0), Some((0, 0)));
        assert_eq!(l.cell_of(89, 59), Some((8, 5)));
        assert_eq!(
            l.cell_of(90, 10),
            None,
            "the strip past the last whole cell"
        );
        assert_eq!(l.cell_of(-1, 10), None);
        assert_eq!(l.cell_near(90, 10), (8, 1));
        assert_eq!(l.cell_near(-5, 64), (0, 5));
        for x in 0..90 {
            assert_eq!(l.cell_of(x, 0), Some(((x / 10) as u32, 0)));
            assert_eq!(l.cell_near(x, 0), ((x / 10) as u32, 0));
        }
    }

    /// **Параллельная сборка даёт то же, что клетка за клеткой**, включая
    /// счёт переполнений: он складывается по кускам в порядке клеток.
    #[test]
    fn the_parallel_build_equals_the_serial_scan() {
        // 200×200 при стороне клетки 2 — 10 000 клеток, больше порога,
        // за которым сборка делится на потоки.
        let w = World::new(200, 40, 200, 16, Box::new(Caves));
        let l = NavLayer::new(&w, 4, 2, 1);
        assert!(
            l.w as usize * l.h as usize >= BUILD_PARALLEL_FROM,
            "мир мал — сборка шла в один поток, и тест ничего не проверил"
        );
        let mut scratch = ScanScratch::default();
        let mut overflow = 0u64;
        for cz in 0..l.h {
            for cx in 0..l.w {
                let s = scan_cell(&w, 2, 4, cx, cz, &mut scratch);
                assert_eq!(
                    l.levels_at(cx, cz),
                    &s.levels[..s.n as usize],
                    "клетка {cx},{cz}"
                );
                assert_eq!(l.top_at(cx, cz), s.top);
                assert_eq!(
                    l.floor_at(cx, cz),
                    if s.n == 0 { NO_FLOOR } else { s.levels[0] }
                );
                assert_eq!(l.standable(cx, cz), s.n > 0);
                overflow += u64::from(s.over);
            }
        }
        assert_eq!(l.overflow, overflow, "счёт переполнений разошёлся");
    }

    /// **Слой из внутренностей — тот же слой.** Это контракт кэша
    /// карты, и порча длины отвергается, а не читается наугад.
    #[test]
    fn raw_round_trips() {
        let w = World::new(64, 32, 48, 16, Box::new(Caves));
        let l = NavLayer::new(&w, 4, 2, 1);
        let r = l.raw();
        assert!(
            r.floor.iter().any(|&f| f != NO_FLOOR),
            "слой пуст — сверять нечего"
        );
        let back = NavLayer::from_raw(r.clone()).expect("внутренности целы");
        assert_eq!(back.raw(), r);
        let mut bad = r.clone();
        bad.floor.pop();
        assert!(
            NavLayer::from_raw(bad).is_none(),
            "слой не той длины принят"
        );
    }
}
