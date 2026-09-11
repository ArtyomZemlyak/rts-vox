//! Мир: двухслойный, с очередью правок и битовым объёмом занятости.
//!
//! **Два слоя** (`docs/04-world.md`): процедурный — мир восстанавливается
//! из сида — плюс слой изменённых чанков. Нетронутый чанк не хранится
//! вовсе: его материал спрашивается у генератора. Материализуется чанк
//! при первой же правке.
//!
//! **Генератор входит трейтом, а не зависимостью.** `worldgen` зависит
//! от `voxel`, значит обратно нельзя; трейт разворачивает стрелку
//! и заодно позволяет сцене подставить свой рельеф.
//!
//! **Битовый объём обновляется списком правок, а не пересборкой**
//! (ADR-061). Пересборка чанка обходом байт стоит около ста микросекунд
//! и в сто раз перекрывает инкрементальный ремеш, который она обслуживает.
//!
//! **Очередь правок с потолком** (ADR-013, ADR-045). Потолок считает
//! **затронутые воксели**, в любом масштабе: он защищает тик, а не
//! экономику. Очередь детерминирована — FIFO, дренаж строго по порядку.

use crate::store::{Mat, Paletted, AIR, CS};

/// Правка: воксель и материал, который в нём оказался.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Edit {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    /// Материал **после** правки. `AIR` — снятие.
    pub mat: Mat,
}

/// Процедурный слой. Обязан быть чистой функцией от координат: значение
/// не зависит ни от порядка обхода, ни от того, что уже посчитано.
/// Источник материала для нетронутого мира.
///
/// `Send + Sync` обязательны: фаза, читающая мир, делится на потоки,
/// а нетронутый чанк она берёт у генератора. Требование стоит здесь,
/// а не у вызывающего, чтобы не выяснилось на первой параллельной
/// фазе, что весь мир не переносится между потоками (ADR-125).
pub trait Generator: Send + Sync {
    fn material(&self, x: u32, y: u32, z: u32) -> Mat;

    /// Отрезок колонки разом. По умолчанию — повоксельный обход, чтобы
    /// простой генератор оставался в одну строку; настоящий генератор
    /// обязан её переопределить и вынести всё, что зависит только от
    /// `(x, z)`, наружу цикла (ADR-114).
    fn column(&self, x: u32, z: u32, y0: u32, out: &mut [Mat]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.material(x, y0 + i as u32, z);
        }
    }
}

/// Что вернул дренаж очереди.
#[derive(Clone, Copy, Default, Debug)]
pub struct FlushStats {
    /// Сколько правок применено в этот тик.
    pub applied: usize,
    /// Сколько осталось в очереди — это отставание, и оно видно игроку.
    pub queued: usize,
    /// Сколько правок ничего не изменили: воксель уже был таким.
    pub noop: usize,
    /// Сколько чанков пришлось построить из генератора. Самая дорогая
    /// часть дренажа, и она ограничена своим потолком (ADR-114).
    pub materialized: usize,
    /// Дренаж остановлен потолком материализаций, а не потолком правок.
    pub materialize_capped: bool,
    /// Сколько правок отложено потолком материализаций на следующий тик.
    pub deferred: usize,
}

pub struct World {
    pub nx: u32,
    pub ny: u32,
    pub nz: u32,
    /// Потолок затронутых вокселей за тик.
    pub max_edits_per_tick: usize,
    /// Сколько чанков разрешено построить из генератора за тик.
    /// Второй потолок дренажа — см. ADR-114.
    pub max_materialize_per_tick: usize,
    /// Сколько уже построено в текущем тике.
    ///
    /// Считается **за тик, а не за дренаж**: дренажей в тике три —
    /// копка, отрывы каскада, запекание тел (ADR-125), — и потолок,
    /// живущий внутри одного `flush`, втрое больше объявленного.
    /// Именно так и было, пока D-7 не переехала на движок (ADR-132).
    materialized_in_tick: usize,
    /// Идёт ли тик. Тот, кто зовёт `flush` напрямую, тика не
    /// объявляет, и для него потолок остаётся на дренаж.
    in_tick: bool,
    gen: Box<dyn Generator + Send + Sync>,
    /// Слой изменений: `None` — чанк нетронут, материал у генератора.
    chunks: Vec<Option<Box<Paletted>>>,
    cx: u32,
    cy: u32,
    /// Битовый объём занятости, плотный. Слово на 64 воксела по X.
    bits: Vec<u64>,
    wx: u32,
    /// Очередь отложенных правок. FIFO — порядок дренажа детерминирован.
    queue: std::collections::VecDeque<Edit>,
    /// Правки, применённые в текущем тике. Это и есть поток для рендера
    /// (ADR-103) и вход связности с навигацией.
    applied: Vec<Edit>,
    /// Переиспользуемый буфер отложенного: дренаж идёт каждый тик,
    /// и выделять его заново незачем.
    defer_buf: Vec<Edit>,
    /// Сколько чанков материализовано из генератора. Каждый стоит
    /// полного обхода чанка, и на первом касании это пик — см. D-7.
    pub materialized: u64,
}

impl World {
    /// Создать мир и построить битовый объём из генератора.
    ///
    /// Построение — единственное место, где генератор обходится целиком;
    /// дальше мир живёт правками.
    pub fn new(nx: u32, ny: u32, nz: u32, max_edits_per_tick: usize, g: Box<dyn Generator + Send + Sync>) -> World {
        let (cx, cy, cz) = (
            nx.div_ceil(CS as u32),
            ny.div_ceil(CS as u32),
            nz.div_ceil(CS as u32),
        );
        let wx = nx.div_ceil(64);
        let mut bits = vec![0u64; (wx * ny * nz) as usize];
        for z in 0..nz {
            for y in 0..ny {
                let row = ((y * nz + z) * wx) as usize;
                for x in 0..nx {
                    if g.material(x, y, z) != AIR {
                        bits[row + (x >> 6) as usize] |= 1u64 << (x & 63);
                    }
                }
            }
        }
        World {
            nx,
            ny,
            nz,
            max_edits_per_tick,
            // Четыре чанка — 2.9 мс на первом касании после колоночного
            // генератора, то есть меньше трети тика. Значение подобрано
            // разгонкой в D-7 (ADR-114) и живёт здесь до тех пор, пока
            // мир не читает данные.
            max_materialize_per_tick: 4,
            materialized_in_tick: 0,
            in_tick: false,
            gen: g,
            chunks: (0..(cx * cy * cz) as usize).map(|_| None).collect(),
            cx,
            cy,
            bits,
            wx,
            queue: std::collections::VecDeque::new(),
            applied: Vec::new(),
            defer_buf: Vec::new(),
            materialized: 0,
        }
    }

    #[inline]
    pub fn inside(&self, x: u32, y: u32, z: u32) -> bool {
        x < self.nx && y < self.ny && z < self.nz
    }

    #[inline]
    fn chunk_index(&self, x: u32, y: u32, z: u32) -> usize {
        let (a, b, c) = (x / CS as u32, y / CS as u32, z / CS as u32);
        ((c * self.cy + b) * self.cx + a) as usize
    }

    #[inline]
    fn bit_index(&self, x: u32, y: u32, z: u32) -> (usize, u64) {
        (
            (((y * self.nz + z) * self.wx) + (x >> 6)) as usize,
            1u64 << (x & 63),
        )
    }

    /// Занят ли воксель. Читает битовый объём, а не материал: это
    /// то, чем пользуются навигация, связность и марш.
    #[inline]
    pub fn solid(&self, x: u32, y: u32, z: u32) -> bool {
        if !self.inside(x, y, z) {
            return false;
        }
        let (w, m) = self.bit_index(x, y, z);
        self.bits[w] & m != 0
    }

    /// Материал воксела. Тронутый чанк — из слоя изменений, нетронутый —
    /// из генератора.
    pub fn material(&self, x: u32, y: u32, z: u32) -> Mat {
        if !self.inside(x, y, z) {
            return AIR;
        }
        match &self.chunks[self.chunk_index(x, y, z)] {
            Some(c) => c.get(
                (x % CS as u32) as usize,
                (y % CS as u32) as usize,
                (z % CS as u32) as usize,
            ),
            None => self.gen.material(x, y, z),
        }
    }

    /// Материализовать чанк из генератора — цена первой правки в нём.
    fn materialize(&mut self, ci: usize) {
        if self.chunks[ci].is_some() {
            return;
        }
        let a = (ci as u32) % self.cx;
        let b = ((ci as u32) / self.cx) % self.cy;
        let c = (ci as u32) / (self.cx * self.cy);
        let (ox, oy, oz) = (a * CS as u32, b * CS as u32, c * CS as u32);
        let mut p = Paletted::new();
        // Обход колоночный, а не послойный: всё, что зависит только от
        // `(x, z)`, генератор считает один раз на колонку вместо `CS` раз
        // (ADR-114). Палитра при этом набирается в другом порядке, но её
        // порядок не наблюдаем — итоговый набор материалов, а значит и
        // размер, и число расширений, от него не зависят.
        let mut col = [AIR; CS];
        for lz in 0..CS as u32 {
            for lx in 0..CS as u32 {
                let (x, z) = (ox + lx, oz + lz);
                if x >= self.nx || z >= self.nz {
                    continue;
                }
                self.gen.column(x, z, oy, &mut col);
                for (ly, &m) in col.iter().enumerate() {
                    if m != AIR && (oy + ly as u32) < self.ny {
                        p.set(lx as usize, ly, lz as usize, m);
                    }
                }
            }
        }
        self.chunks[ci] = Some(Box::new(p));
        self.materialized += 1;
    }

    /// Поставить правку в очередь. Потолок применяется при дренаже,
    /// а не здесь: очередь не имеет длины по построению — иначе приказ
    /// молча терялся бы.
    pub fn queue_edit(&mut self, e: Edit) {
        if self.inside(e.x, e.y, e.z) {
            self.queue.push_back(e);
        }
    }

    pub fn queue_dig(&mut self, x: u32, y: u32, z: u32) {
        self.queue_edit(Edit { x, y, z, mat: AIR });
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Дренаж очереди в пределах потолка. Возвращает статистику;
    /// сам список применённых правок — [`World::edits`].
    /// Объявить начало тика: потолок материализаций считается от сюда.
    pub fn begin_tick(&mut self) {
        self.in_tick = true;
        self.materialized_in_tick = 0;
    }

    pub fn flush(&mut self) -> FlushStats {
        if !self.in_tick {
            self.materialized_in_tick = 0;
        }
        self.applied.clear();
        let mut st = FlushStats::default();
        // Потолок материализаций — второй потолок дренажа, и он нужен
        // по той же причине, что первый: без него тик, в котором забой
        // вошёл в шестнадцать нетронутых чанков разом, стоит тридцать
        // четыре миллисекунды (ADR-114).
        //
        // Упёршаяся правка **откладывается, а не останавливает дренаж**.
        // Останавливать нельзя: очередь строго FIFO, и одна правка в
        // свежий чанк заперла бы за собой всё, что идёт в уже тронутые.
        // Замер: на потолке 8 это раздувало пик очереди с 3 388 до
        // 16 216 правок. Порядок при этом сохраняется там, где он значим:
        // правки в один воксель лежат в одном чанке и откладываются
        // вместе, а правки в разные воксели переставимы по построению.
        let mut deferred = std::mem::take(&mut self.defer_buf);
        deferred.clear();
        while st.applied < self.max_edits_per_tick && deferred.len() < self.max_edits_per_tick {
            let Some(e) = self.queue.pop_front() else { break };
            let ci = self.chunk_index(e.x, e.y, e.z);
            let fresh = self.chunks[ci].is_none();
            if fresh && self.materialized_in_tick >= self.max_materialize_per_tick {
                st.materialize_capped = true;
                deferred.push(e);
                continue;
            }
            if self.apply(e) {
                if fresh {
                    st.materialized += 1;
                    self.materialized_in_tick += 1;
                }
                self.applied.push(e);
                st.applied += 1;
            } else {
                st.noop += 1;
            }
        }
        // Отложенное возвращается в голову очереди: в следующем тике
        // дренаж начнёт с него, иначе свежий чанк ждал бы вечно.
        for &e in deferred.iter().rev() {
            self.queue.push_front(e);
        }
        st.deferred = deferred.len();
        self.defer_buf = deferred;
        st.queued = self.queue.len();
        st
    }

    /// Применить одну правку. Возвращает `false`, если она ничего
    /// не изменила.
    fn apply(&mut self, e: Edit) -> bool {
        let was = self.material(e.x, e.y, e.z);
        if was == e.mat {
            return false;
        }
        let ci = self.chunk_index(e.x, e.y, e.z);
        self.materialize(ci);
        if let Some(c) = &mut self.chunks[ci] {
            c.set(
                (e.x % CS as u32) as usize,
                (e.y % CS as u32) as usize,
                (e.z % CS as u32) as usize,
                e.mat,
            );
        }
        let (w, m) = self.bit_index(e.x, e.y, e.z);
        if e.mat == AIR {
            self.bits[w] &= !m;
        } else {
            self.bits[w] |= m;
        }
        true
    }

    /// Правки, применённые в последнем дренаже. Поток для рендера
    /// (ADR-103), вход связности и переворотов нав-клеток.
    pub fn edits(&self) -> &[Edit] {
        &self.applied
    }

    /// Битовый объём целиком — для пирамиды и марша.
    pub fn bits(&self) -> &[u64] {
        &self.bits
    }

    pub fn bit_words_per_row(&self) -> u32 {
        self.wx
    }

    /// Сколько чанков материализовано. Это и есть размер слоя изменений.
    pub fn touched_chunks(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_some()).count()
    }

    /// Байты слоя изменений. Нетронутые чанки не стоят ничего.
    pub fn diff_bytes(&self) -> usize {
        self.chunks
            .iter()
            .filter_map(|c| c.as_ref())
            .map(|c| c.bytes())
            .sum()
    }

    pub fn bit_bytes(&self) -> usize {
        self.bits.len() * 8
    }
}

#[cfg(test)]
mod tests {

    /// **Потолки меняют «когда», но никогда «что».**
    ///
    /// Точечные тесты выше проверяют по случаю. Этот гоняет случайные
    /// партии правок через **все** сочетания потолков и требует, чтобы
    /// мир в конце был тот же, что без потолков вовсе.
    ///
    /// Проверять надо конечный мир, а не число применённых правок:
    /// правка, применённая дважды, и правка, потерянная вместе с парной
    /// ей, дают верный счётчик и разный мир.
    ///
    /// Партия устроена так, чтобы ловить **и потерю, и перестановку**,
    /// а это разные требования и они тянут в разные стороны. Перестановка
    /// видна только когда в один воксель бьют не раз и разным материалом;
    /// откладывание случается только когда свежих чанков много. Поэтому
    /// чанков тридцать два, а мест внутри чанка — четыре: попаданий
    /// в один воксель много, и свежих чанков тоже.
    ///
    /// Случайность своя, из `fx::rng`: внешний генератор свойств — это
    /// зависимость, а сид здесь обязан быть явным (`CLAUDE.md`).
    #[test]
    fn caps_change_when_not_what() {
        struct Layers;
        impl Generator for Layers {
            fn material(&self, x: u32, y: u32, z: u32) -> Mat {
                if y > 20 {
                    return AIR;
                }
                (1 + ((x / 7) ^ (y / 3) ^ (z / 5)) % 3) as Mat
            }
        }
        // 8×1×4 = 32 чанка по 32³.
        let dims = (256u32, 32u32, 128u32);
        let chunks = (dims.0 / 32) * (dims.1 / 32) * (dims.2 / 32);

        let plan = |seed: u64| -> Vec<Edit> {
            let mut r = fx::rng::Rng::new(seed, 0xE417);
            (0..500)
                .map(|_| {
                    let c = r.below(chunks);
                    let (cx, cz) = (c % (dims.0 / 32), c / (dims.0 / 32));
                    // Четыре места в чанке — чтобы в один воксель
                    // попадали многократно.
                    let slot = r.below(4);
                    let (ox, oy, oz) = (5 + (slot & 1) * 9, 5 + (slot >> 1) * 7, 6);
                    Edit {
                        x: cx * 32 + ox,
                        y: oy,
                        z: cz * 32 + oz,
                        mat: r.below(4) as Mat,
                    }
                })
                .collect()
        };

        let run = |edits: &[Edit], max_edits: usize, max_mat: usize| -> u64 {
            let mut w = World::new(dims.0, dims.1, dims.2, 4096, Box::new(Layers));
            w.max_edits_per_tick = max_edits;
            w.max_materialize_per_tick = max_mat;
            for &e in edits {
                w.queue_edit(e);
            }
            let mut guard = 0;
            while w.queued() > 0 {
                w.begin_tick();
                w.flush();
                guard += 1;
                assert!(guard < 10_000, "очередь не сходится: {} осталось", w.queued());
            }
            // Хеш мира целиком: только он доказывает совпадение, и он же
            // дешевле снимка. Сложение заворачиванием — ассоциативно.
            let mut h = 0u64;
            for z in 0..dims.2 {
                for y in 0..dims.1 {
                    for x in 0..dims.0 {
                        let v = w.material(x, y, z) as u64;
                        h = h
                            .wrapping_mul(0x100_0000_01b3)
                            ^ v.wrapping_add((x as u64) << 20)
                            ^ ((y as u64) << 10)
                            ^ (z as u64);
                    }
                }
            }
            h
        };

        for seed in 0..6u64 {
            let edits = plan(seed);
            let want = run(&edits, usize::MAX, usize::MAX);
            for &(me, mm) in &[(1usize, 1usize), (3, 1), (1, 4), (7, 2), (64, 3), (500, 32)] {
                assert_eq!(
                    run(&edits, me, mm),
                    want,
                    "сид {seed}, потолки правок {me} и материализаций {mm}: \\
мир разошёлся с беспотолочным"
                );
            }
        }
    }

    /// Потолок материализаций обязан **откладывать** правку, а не
    /// съедать её: иначе дыра в мире просто не появится, и никто
    /// об этом не узнает.
    #[test]
    fn materialize_cap_defers_edits_instead_of_losing_them() {
        struct Solid;
        impl Generator for Solid {
            fn material(&self, _x: u32, _y: u32, _z: u32) -> Mat {
                7
            }
        }
        // Мир из 4×1×1 чанков; правки идут по одной в каждый.
        let mut w = World::new(128, 32, 32, 1000, Box::new(Solid));
        w.max_materialize_per_tick = 1;
        for c in 0..4u32 {
            w.queue_dig(c * 32 + 5, 5, 5);
        }

        let st = w.flush();
        assert_eq!(st.applied, 1, "за тик — один свежий чанк");
        assert_eq!(st.materialized, 1);
        assert!(st.materialize_capped);
        assert_eq!(st.deferred, 3);
        assert_eq!(st.queued, 3, "остальные ждут, а не пропали");

        // Четыре тика — четыре дырки, ни одной потерянной.
        for _ in 0..3 {
            w.flush();
        }
        for c in 0..4u32 {
            assert_eq!(w.material(c * 32 + 5, 5, 5), AIR, "чанк {c}");
        }
        assert_eq!(w.queued(), 0);
    }

    /// Правки в уже материализованный чанк потолок не трогает: он
    /// ограничивает построение из генератора, а не работу вообще.
    #[test]
    fn materialize_cap_does_not_throttle_touched_chunks() {
        struct Solid;
        impl Generator for Solid {
            fn material(&self, _x: u32, _y: u32, _z: u32) -> Mat {
                7
            }
        }
        let mut w = World::new(64, 32, 32, 1000, Box::new(Solid));
        w.max_materialize_per_tick = 1;
        w.queue_dig(5, 5, 5);
        w.flush();

        for y in 6..30 {
            w.queue_dig(5, y, 5);
        }
        let st = w.flush();
        assert_eq!(st.applied, 24);
        assert_eq!(st.materialized, 0);
        assert!(!st.materialize_capped);
    }

    /// Колоночный вход генератора и повоксельный обязаны давать один
    /// и тот же чанк. Разойдутся — разойдётся весь мир, молча.
    #[test]
    fn column_and_per_voxel_generators_build_the_same_chunk() {
        struct Ridged;
        impl Generator for Ridged {
            fn material(&self, x: u32, y: u32, z: u32) -> Mat {
                if y < 4 + (x * 7 + z * 13) % 19 {
                    ((x ^ y ^ z) % 5 + 1) as Mat
                } else {
                    AIR
                }
            }
            // Переопределения нет: работает запасной повоксельный путь.
        }
        struct Fast;
        impl Generator for Fast {
            fn material(&self, x: u32, y: u32, z: u32) -> Mat {
                Ridged.material(x, y, z)
            }
            fn column(&self, x: u32, z: u32, y0: u32, out: &mut [Mat]) {
                let h = 4 + (x * 7 + z * 13) % 19;
                for (i, o) in out.iter_mut().enumerate() {
                    let y = y0 + i as u32;
                    *o = if y < h { ((x ^ y ^ z) % 5 + 1) as Mat } else { AIR };
                }
            }
        }

        let mut a = World::new(64, 64, 64, 1, Box::new(Ridged));
        let mut b = World::new(64, 64, 64, 1, Box::new(Fast));
        a.queue_dig(1, 1, 1);
        b.queue_dig(1, 1, 1);
        a.flush();
        b.flush();
        for z in 0..32 {
            for y in 0..32 {
                for x in 0..32 {
                    assert_eq!(a.material(x, y, z), b.material(x, y, z), "{x} {y} {z}");
                }
            }
        }
    }
    use super::*;

    /// Слоистый генератор: низ твёрдый, верх воздух, плюс «жила»
    /// в середине — чтобы материал был не один.
    struct Layers {
        h: u32,
    }
    impl Generator for Layers {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if y >= self.h {
                AIR
            } else if (x / 4 + z / 4) % 5 == 0 && y > self.h / 2 {
                7
            } else {
                2
            }
        }
    }

    fn world() -> World {
        World::new(64, 48, 64, 1000, Box::new(Layers { h: 30 }))
    }

    /// Нетронутый мир обязан совпадать с генератором во всех вокселях —
    /// и по материалу, и по битовому объёму. Расхождение здесь означает,
    /// что навигация и рендер смотрят в разные миры.
    #[test]
    fn untouched_world_matches_the_generator() {
        let w = world();
        let g = Layers { h: 30 };
        for z in 0..w.nz {
            for y in 0..w.ny {
                for x in 0..w.nx {
                    let m = g.material(x, y, z);
                    assert_eq!(w.material(x, y, z), m, "материал ({x},{y},{z})");
                    assert_eq!(w.solid(x, y, z), m != AIR, "бит ({x},{y},{z})");
                }
            }
        }
        assert_eq!(w.touched_chunks(), 0, "нетронутый мир не должен ничего хранить");
    }

    /// Правка видна **в тот же дренаж** и в материале, и в битах
    /// (`CLAUDE.md`, «мир меняется для симуляции в тот же тик»).
    #[test]
    fn edit_is_visible_immediately_in_both_layers() {
        let mut w = world();
        assert!(w.solid(10, 5, 10));
        w.queue_dig(10, 5, 10);
        let st = w.flush();
        assert_eq!(st.applied, 1);
        assert_eq!(w.material(10, 5, 10), AIR);
        assert!(!w.solid(10, 5, 10));
        assert_eq!(w.edits(), &[Edit { x: 10, y: 5, z: 10, mat: AIR }]);
        assert_eq!(w.touched_chunks(), 1, "тронут ровно один чанк");
    }

    /// Потолок соблюдается точно, а остаток остаётся в очереди —
    /// не теряется. Потолок защищает тик; потеря приказа сломала бы
    /// экономику.
    #[test]
    fn cap_is_exact_and_the_rest_stays_queued() {
        let mut w = World::new(64, 48, 64, 100, Box::new(Layers { h: 30 }));
        for i in 0..250u32 {
            w.queue_dig(i % 60, 5, i / 60);
        }
        let st = w.flush();
        assert_eq!(st.applied, 100);
        assert_eq!(st.queued, 150);
        assert_eq!(w.edits().len(), 100);
        let st = w.flush();
        assert_eq!(st.applied, 100);
        assert_eq!(st.queued, 50);
        let st = w.flush();
        assert_eq!(st.applied, 50);
        assert_eq!(st.queued, 0);
    }

    /// Пустая правка не тратит потолок: снять уже снятое — не работа.
    /// Иначе повторный приказ по одному месту съедал бы бюджет тика.
    #[test]
    fn noop_edits_do_not_spend_the_cap() {
        let mut w = World::new(64, 48, 64, 10, Box::new(Layers { h: 30 }));
        for _ in 0..8 {
            w.queue_dig(10, 5, 10);
        }
        w.queue_dig(11, 5, 10);
        w.queue_dig(12, 5, 10);
        let st = w.flush();
        assert_eq!(st.applied, 3, "три разных воксела");
        assert_eq!(st.noop, 7);
        assert_eq!(st.queued, 0);
    }

    /// Дренаж строго FIFO: порядок применения не зависит ни от чего,
    /// кроме порядка постановки.
    #[test]
    fn drain_order_is_fifo() {
        let mut w = World::new(64, 48, 64, 3, Box::new(Layers { h: 30 }));
        for x in 0..9u32 {
            w.queue_dig(x, 5, 0);
        }
        for round in 0..3 {
            w.flush();
            let got: Vec<u32> = w.edits().iter().map(|e| e.x).collect();
            assert_eq!(got, vec![round * 3, round * 3 + 1, round * 3 + 2]);
        }
    }

    /// Слой изменений растёт только там, где копали.
    #[test]
    fn diff_layer_grows_only_where_dug() {
        let mut w = world();
        assert_eq!(w.diff_bytes(), 0);
        w.queue_dig(1, 1, 1);
        w.flush();
        let one = w.diff_bytes();
        assert!(one > 0);
        // Ещё одна правка в том же чанке не добавляет чанков.
        w.queue_dig(2, 1, 1);
        w.flush();
        assert_eq!(w.touched_chunks(), 1);
        assert_eq!(w.diff_bytes(), one);
    }
}
