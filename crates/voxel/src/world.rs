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
    /// Сколько записей очереди просмотрено — вместе с пустыми.
    ///
    /// Это число, а не `applied`, ограничено потолком просмотра.
    /// Применённые правки считают **затронутые воксели** (ADR-045),
    /// а пустая правка не трогает ни одного — но найти это всё равно
    /// надо, и стоит это столько же (ADR-182).
    pub examined: usize,
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

/// Запомненный чанк для [`World::material_cached`].
#[derive(Clone, Copy, Debug)]
pub struct MatCursor {
    ci: usize,
    ox: u32,
    oy: u32,
    oz: u32,
    valid: bool,
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
    /// Курсор материала: помнит последний чанк и его начало.
    ///
    /// **Деления — вот что стоит дорого.** `material` считает индекс
    /// чанка тремя делениями на его сторону и местные координаты тремя
    /// остатками, то есть шесть делений на каждый спрошенный воксель.
    /// Раскраска рельефа спрашивает материал у каждого воксела каждой
    /// грани чанка, и почти все эти воксели лежат в **одном и том же**
    /// чанке. Курсор пересчитывает индекс только при выходе за его
    /// границы (ADR-227).
    pub fn mat_cursor() -> MatCursor {
        MatCursor { ci: 0, ox: 0, oy: 0, oz: 0, valid: false }
    }

    /// Материал с курсором. Ответ **тот же до бита**, что у
    /// [`World::material`]: это ускорение, а не приближение.
    pub fn material_cached(&self, c: &mut MatCursor, x: u32, y: u32, z: u32) -> Mat {
        if !self.inside(x, y, z) {
            return AIR;
        }
        let cs = CS as u32;
        if !c.valid || x < c.ox || x - c.ox >= cs || y < c.oy || y - c.oy >= cs || z < c.oz || z - c.oz >= cs
        {
            let (a, b, d) = (x / cs, y / cs, z / cs);
            c.ci = ((d * self.cy + b) * self.cx + a) as usize;
            c.ox = a * cs;
            c.oy = b * cs;
            c.oz = d * cs;
            c.valid = true;
        }
        match &self.chunks[c.ci] {
            Some(k) => k.get((x - c.ox) as usize, (y - c.oy) as usize, (z - c.oz) as usize),
            None => self.gen.material(x, y, z),
        }
    }

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
        //
        // **Третий потолок — на просмотр.** Пустая правка не тратит
        // потолок правок и не должна: он считает затронутые воксели,
        // а она не трогает ни одного. Но найти это надо, и стоит это
        // поиска в дереве чанков. Без счёта просмотра дренаж крутился
        // бы, пока очередь не кончится, а длины у очереди нет
        // намеренно: приказ не имеет права молча потеряться.
        //
        // Замер (ADR-182): 200 000 пустых правок на мире 256×128×256
        // при потолке 4096 — **4.637 мс за один тик**, и ни один
        // потолок не сработал.
        //
        // Своего числа у потолка просмотра нет намеренно: тик
        // просматривает столько записей, сколько ему разрешено
        // применить правок.
        let mut deferred = std::mem::take(&mut self.defer_buf);
        deferred.clear();
        while st.applied < self.max_edits_per_tick
            && deferred.len() < self.max_edits_per_tick
            && st.examined < self.max_edits_per_tick
        {
            let Some(e) = self.queue.pop_front() else { break };
            st.examined += 1;
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

    /// Записать **слой изменений** в буфер (ADR-229).
    ///
    /// Процедурный слой не пишется: он чистая функция от сида, и
    /// восстанавливается тем же генератором, которым был получен.
    /// Пишется только то, чего в генераторе нет, — тронутые чанки
    /// и очередь.
    ///
    /// Чанк пишется **номерами материалов**, повторами: номер
    /// и длина. Не внутренностями палитры — её порядок зависит
    /// от того, в каком порядке чанк заполняли, и не наблюдаем,
    /// а номер материала задаётся данными и обязан пережить
    /// перезагрузку пака (ADR-028).
    /// **Потолки не пишутся.** Они не состояние партии, а правила,
    /// по которым она идёт: приходят из `data/*.ron` и из кода
    /// вызывающего (ADR-140). Запиши их в файл — и правка `sim.ron`
    /// перестала бы действовать на загруженную партию, а в файле
    /// появился бы второй источник правды.
    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.u32(self.nx);
        b.u32(self.ny);
        b.u32(self.nz);
        b.usize(self.materialized_in_tick);
        b.bool(self.in_tick);
        b.u64(self.materialized);
        // Тронутые чанки — по возрастанию номера. Порядок обхода
        // при записи фиксирован (`CLAUDE.md`), иначе один и тот же
        // мир давал бы разные файлы.
        let touched: Vec<u32> = (0..self.chunks.len())
            .filter(|&i| self.chunks[i].is_some())
            .map(|i| i as u32)
            .collect();
        b.u32(touched.len() as u32);
        for &ci in &touched {
            b.u32(ci);
            let c = self.chunks[ci as usize].as_ref().expect("тронутый чанк");
            // Повторы: (материал, длина). Порядок обхода — тот же,
            // что у плотного чанка, поэтому чтение обратно однозначно.
            let mut runs: Vec<(Mat, u16)> = Vec::new();
            for y in 0..CS {
                for z in 0..CS {
                    for x in 0..CS {
                        let m = c.get(x, y, z);
                        match runs.last_mut() {
                            Some((lm, n)) if *lm == m && *n < u16::MAX => *n += 1,
                            _ => runs.push((m, 1)),
                        }
                    }
                }
            }
            b.u32(runs.len() as u32);
            for &(m, n) in &runs {
                b.u16(m);
                b.u16(n);
            }
        }
        let q: Vec<Edit> = self.queue.iter().copied().collect();
        save_edits(b, self.nx, self.nz, &q);
        save_edits(b, self.nx, self.nz, &self.applied);
    }

    /// Прочитать слой изменений в **уже построенный из генератора**
    /// мир. Размеры обязаны совпасть: чужой мир — это не «немного
    /// другой», это другой файл.
    pub fn load(&mut self, s: &mut fx::blob::Scan) -> bool {
        if s.u32() != self.nx || s.u32() != self.ny || s.u32() != self.nz {
            return false;
        }
        self.materialized_in_tick = s.usize();
        self.in_tick = s.bool();
        self.materialized = s.u64();
        let n = s.list(4);
        for _ in 0..n {
            let ci = s.u32() as usize;
            if ci >= self.chunks.len() {
                return false;
            }
            let runs = s.list(4);
            let mut p = Paletted::new();
            let mut at = 0usize;
            for _ in 0..runs {
                let m = s.u16();
                let len = s.u16() as usize;
                if at + len > CS * CS * CS {
                    return false;
                }
                if m != AIR {
                    for k in at..at + len {
                        // Обратный ход обхода записи: y, z, x.
                        let (x, z, y) = (k % CS, (k / CS) % CS, k / (CS * CS));
                        p.set(x, y, z, m);
                    }
                }
                at += len;
            }
            if at != CS * CS * CS {
                return false;
            }
            p.widenings = 0;
            self.chunks[ci] = Some(Box::new(p));
            self.refresh_bits_of(ci);
        }
        self.queue = load_edits(s, self.nx, self.nz).into_iter().collect();
        self.applied = load_edits(s, self.nx, self.nz);
        s.ok()
    }

    /// Перестроить битовый объём по чанку слоя изменений.
    ///
    /// Нужен ровно при загрузке: битовый объём построен генератором,
    /// а тронутый чанк генератор переопределяет. Правкой по одной
    /// здесь не обойтись — правок не сохраняли, сохраняли итог.
    fn refresh_bits_of(&mut self, ci: usize) {
        let cs = CS as u32;
        let a = (ci as u32) % self.cx;
        let bq = ((ci as u32) / self.cx) % self.cy;
        let c = (ci as u32) / (self.cx * self.cy);
        let (ox, oy, oz) = (a * cs, bq * cs, c * cs);
        for ly in 0..cs {
            for lz in 0..cs {
                for lx in 0..cs {
                    let (x, y, z) = (ox + lx, oy + ly, oz + lz);
                    if !self.inside(x, y, z) {
                        continue;
                    }
                    let solid = self.chunks[ci]
                        .as_ref()
                        .expect("чанк только что поставлен")
                        .get(lx as usize, ly as usize, lz as usize)
                        != AIR;
                    let (w, m) = self.bit_index(x, y, z);
                    if solid {
                        self.bits[w] |= m;
                    } else {
                        self.bits[w] &= !m;
                    }
                }
            }
        }
    }
}

/// Список правок — **отрезками по линейному номеру воксела**.
///
/// Замер (ADR-229): очередь эталонного сценария на трёхсотом тике
/// весит 48 МБ из пятидесяти, если писать каждую правку четырьмя
/// полями. Причина не в формате, а в самой очереди: копатель ставит
/// в неё шар вокселей, а потолок дренажа меньше, и хвост растёт
/// каждый тик.
///
/// Шар обходится по `x` внутри, значит подряд идущие правки —
/// это подряд идущие номера с одним материалом. Отрезок пишется
/// разностью от конца прошлого (зигзаг), материалом и длиной,
/// всё переменной длиной.
///
/// **Порядок сохраняется точно.** Очередь FIFO, и отрезок — это
/// запись о подряд идущих её элементах, а не их пересортировка.
pub fn save_edits(b: &mut fx::blob::Blob, nx: u32, nz: u32, e: &[Edit]) {
    let at = |x: u32, y: u32, z: u32| (y as u64 * nz as u64 + z as u64) * nx as u64 + x as u64;
    b.u32(e.len() as u32);
    let mut runs: Vec<(u64, Mat, u32)> = Vec::new();
    for x in e {
        let i = at(x.x, x.y, x.z);
        match runs.last_mut() {
            Some((s, m, n)) if *m == x.mat && *s + u64::from(*n) == i => *n += 1,
            _ => runs.push((i, x.mat, 1)),
        }
    }
    b.u32(runs.len() as u32);
    let mut prev = 0i64;
    for &(i, m, n) in &runs {
        b.ivar(i as i64 - prev);
        b.uvar(u64::from(m));
        b.uvar(u64::from(n));
        prev = i as i64 + i64::from(n);
    }
}

pub fn load_edits(s: &mut fx::blob::Scan, nx: u32, nz: u32) -> Vec<Edit> {
    let total = s.u32() as usize;
    let nruns = s.list(3);
    let mut out = Vec::with_capacity(total.min(1 << 24));
    let mut prev = 0i64;
    for _ in 0..nruns {
        let start = prev + s.ivar();
        let m = s.uvar() as Mat;
        let n = s.uvar();
        if start < 0 || n > total as u64 {
            return Vec::new();
        }
        for k in 0..n {
            let i = start as u64 + k;
            let x = (i % nx as u64) as u32;
            let z = ((i / nx as u64) % nz as u64) as u32;
            let y = (i / (nx as u64 * nz as u64)) as u32;
            out.push(Edit { x, y, z, mat: m });
        }
        prev = start + n as i64;
    }
    out
}

#[cfg(test)]
mod codec_tests {
    use super::*;
    use fx::blob::{Blob, Scan};
    use fx::rng::Rng;

    /// **Сжатие отрезками не имеет права менять ни состав, ни порядок.**
    ///
    /// Очередь правок — FIFO, и её порядок задаёт, что окажется в мире
    /// при столкновении двух правок в один воксель. Перестановка
    /// молча изменила бы исход, а счётчик длины остался бы верным —
    /// ровно та ловушка, из-за которой в этом файле проверяют мир,
    /// а не число применённых правок.
    #[test]
    fn packing_edits_keeps_both_order_and_content() {
        let (nx, ny, nz) = (256u32, 128u32, 256u32);
        let mut r = Rng::new(9, 0x3D1);
        for case in 0..8u64 {
            let mut want: Vec<Edit> = Vec::new();
            // Смесь трёх видов: шар копателя (подряд идущие номера),
            // одиночные правки вразброс и повтор одного воксела разным
            // материалом — сжатие обязано пережить все три.
            for _ in 0..4 {
                let (cx, cy, cz) = (
                    r.range(12, nx as i32 - 12),
                    r.range(12, ny as i32 - 12),
                    r.range(12, nz as i32 - 12),
                );
                for dy in -3..=3i32 {
                    for dz in -3..=3i32 {
                        for dx in -3..=3i32 {
                            want.push(Edit {
                                x: (cx + dx) as u32,
                                y: (cy + dy) as u32,
                                z: (cz + dz) as u32,
                                mat: AIR,
                            });
                        }
                    }
                }
            }
            for _ in 0..200 {
                want.push(Edit {
                    x: r.below(nx),
                    y: r.below(ny),
                    z: r.below(nz),
                    mat: (r.below(5) + case as u32) as Mat,
                });
            }
            for m in 0..6u16 {
                want.push(Edit { x: 7, y: 7, z: 7, mat: m });
            }
            let mut b = Blob::new();
            save_edits(&mut b, nx, nz, &want);
            let mut s = Scan::new(&b.v);
            let got = load_edits(&mut s, nx, nz);
            assert!(s.ok());
            assert_eq!(got, want, "случай {case}");
        }
    }

    /// Пустой список — это список, а не отсутствие поля.
    #[test]
    fn an_empty_edit_list_round_trips() {
        let mut b = Blob::new();
        save_edits(&mut b, 64, 64, &[]);
        let mut s = Scan::new(&b.v);
        assert!(load_edits(&mut s, 64, 64).is_empty());
        assert!(s.ok());
    }

    /// Шар копателя обязан лечь **отрезками**, а не по правке на запись.
    /// Замер, ради которого сжатие и появилось: очередь эталонного
    /// сценария весила 48 МБ из пятидесяти.
    #[test]
    fn a_diggers_ball_packs_into_runs() {
        let (nx, nz) = (256u32, 256u32);
        let mut ball = Vec::new();
        for dy in -10..=10i32 {
            for dz in -10..=10i32 {
                for dx in -10..=10i32 {
                    if dx * dx + dy * dy + dz * dz > 100 {
                        continue;
                    }
                    ball.push(Edit {
                        x: (60 + dx) as u32,
                        y: (60 + dy) as u32,
                        z: (60 + dz) as u32,
                        mat: AIR,
                    });
                }
            }
        }
        let mut b = Blob::new();
        save_edits(&mut b, nx, nz, &ball);
        // Четырнадцать байт на правку — то, с чего начинали.
        assert!(
            b.len() * 6 < ball.len() * 14,
            "шар из {} правок лёг в {} байт — сжатия нет",
            ball.len(),
            b.len()
        );
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

    /// Пустая правка не тратит потолок **правок**: снять уже снятое —
    /// не работа, а потолок считает затронутые воксели (ADR-045).
    /// Потолок **просмотра** она тратит — см.
    /// `empty_edits_cannot_eat_the_tick`.
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

    /// **Пустые правки не могут съесть тик.**
    ///
    /// Потолок правок они не тратят и не должны — это канон. Но очередь
    /// у мира не ограничена по длине намеренно: приказ не имеет права
    /// молча потеряться. Значит, без потолка просмотра миллион копок
    /// по уже выкопанному съедал весь тик, не нарушив ни одного
    /// правила, — ровно тот отказ в обслуживании, от которого очередь
    /// и защищает (`CLAUDE.md`, ADR-182).
    #[test]
    fn empty_edits_cannot_eat_the_tick() {
        const CAP: usize = 64;
        let mut w = World::new(64, 48, 64, CAP, Box::new(Layers { h: 30 }));
        // Сперва выкопать по-настоящему, дальше копать то же место.
        w.queue_dig(10, 5, 10);
        assert_eq!(w.flush().applied, 1);
        for _ in 0..10_000 {
            w.queue_dig(10, 5, 10);
        }
        let st = w.flush();
        assert_eq!(st.applied, 0, "копать там уже нечего");
        assert_eq!(st.examined, CAP, "просмотр очереди ничем не ограничен");
        assert_eq!(st.noop, CAP);
        assert_eq!(
            st.queued,
            10_000 - CAP,
            "остаток обязан ждать в очереди, а не пропасть"
        );
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
