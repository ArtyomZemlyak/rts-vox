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

use std::sync::Arc;

#[path = "columns.rs"]
pub mod columns;
#[cfg(test)]
#[path = "world_material_tests.rs"]
mod material_tests;
#[cfg(test)]
#[path = "world_reset_tests.rs"]
mod reset_tests;
#[path = "snapshot.rs"]
pub mod snapshot;

use crate::store::{Mat, Paletted, AIR, CS};

/// Никто не заказывал: обвал, запекание приземлившегося тела, воронка
/// от попадания. Экономика такую правку в чей-то бак не начисляет.
pub const NOBODY: u32 = u32::MAX;

/// Правка: воксель и материал, который в нём оказался.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Edit {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    /// Материал **после** правки. `AIR` — снятие.
    pub mat: Mat,
    /// Кто заказал правку: слот юнита либо [`NOBODY`].
    ///
    /// Нужно экономике, и только ей: выкопанное начисляется тому, кто
    /// копал, а распылённое воронкой уходит в объявленный сток
    /// (`docs/07-economy.md`). Без этого поля начисление пришлось бы
    /// делать по соседству правки с копателем, то есть угадывать.
    ///
    /// В хеш **не входит**: хеш сводит то, что стало с миром, а не
    /// того, кто об этом попросил. Начисленная материя в хеш входит
    /// отдельно и своей величиной.
    pub by: u32,
}

impl Edit {
    /// Unowned compare-and-remove. The reserved requester range encodes the
    /// expected material, preserving the existing fixed-width edit wire format.
    /// Unit requester IDs must stay below 0xfffe0000.
    pub fn dig_if(x: u32, y: u32, z: u32, expected: Mat) -> Edit {
        Edit {
            x,
            y,
            z,
            mat: AIR,
            by: 0xfffe0000 | expected as u32,
        }
    }
    pub fn expected_material(self) -> Option<Mat> {
        (self.mat == AIR && self.by & 0xffff0000 == 0xfffe0000).then_some(self.by as Mat)
    }

    /// Снятие воксела без заказчика — обвал, воронка, оснастка теста.
    pub fn dig(x: u32, y: u32, z: u32) -> Edit {
        Edit {
            x,
            y,
            z,
            mat: AIR,
            by: NOBODY,
        }
    }

    /// Укладка материала без заказчика.
    pub fn put(x: u32, y: u32, z: u32, mat: Mat) -> Edit {
        Edit {
            x,
            y,
            z,
            mat,
            by: NOBODY,
        }
    }
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

    /// Performance hint for readers that can use either exact points or
    /// columns. Opt in when column() shares expensive X/Z preparation;
    /// the default scalar implementation gains nothing by also reading air.
    /// This affects query packing only, never the returned materials.
    fn batches_columns(&self) -> bool {
        false
    }

    /// Отрезок колонки разом. По умолчанию — повоксельный обход, чтобы
    /// простой генератор оставался в одну строку; настоящий генератор
    /// обязан её переопределить и вынести всё, что зависит только от
    /// `(x, z)`, наружу цикла (ADR-114).
    /// Переопределение обязано совпадать с `material`, включая AIR:
    /// из этих колонок строится точная карта занятости мира.
    fn column(&self, x: u32, z: u32, y0: u32, out: &mut [Mat]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.material(x, y0 + i as u32, z);
        }
    }
}

/// Queued removals counted per voxel (ADR-683): a dense table per chunk,
/// held only while the chunk has a removal queued. A count, not a flag:
/// legacy queues may hold one cell twice. Derived from the queues; never
/// saved or hashed. A salvo queues tens of thousands of digs a tick, and
/// a count in place costs no search.
#[derive(Clone, Default)]
struct PendingDigs {
    chunks: Vec<Option<Box<[u32]>>>,
    totals: Vec<u32>,
}

#[cfg(test)]
#[path = "world_pending_dig_tests.rs"]
mod pending_dig_tests;
impl PendingDigs {
    fn new(chunks: usize) -> Self {
        Self {
            chunks: vec![None; chunks],
            totals: vec![0; chunks],
        }
    }
    fn add(&mut self, (chunk, voxel): (usize, usize)) {
        let table =
            self.chunks[chunk].get_or_insert_with(|| vec![0; CS * CS * CS].into_boxed_slice());
        table[voxel] += 1;
        self.totals[chunk] += 1;
    }
    fn remove(&mut self, (chunk, voxel): (usize, usize)) {
        let table = self.chunks[chunk].as_mut().expect("queued dig indexed");
        assert!(table[voxel] > 0, "queued dig indexed");
        table[voxel] -= 1;
        self.totals[chunk] -= 1;
        if self.totals[chunk] == 0 {
            self.chunks[chunk] = None;
        }
    }
    fn contains(&self, (chunk, voxel): (usize, usize)) -> bool {
        self.chunks[chunk].as_ref().is_some_and(|t| t[voxel] > 0)
    }
    fn clear(&mut self) {
        for (table, total) in self.chunks.iter_mut().zip(&mut self.totals) {
            if *total > 0 {
                *table = None;
                *total = 0;
            }
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

/// Lazily computed summary of one navigation floor tile.
type FloorTile = ([Mat; 2], [u16; 2], Option<u16>, bool);

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
    /// Сколько правок применено и просмотрено в текущем тике — по той
    /// же причине, что материализации: потолок правок и потолок
    /// просмотра считаются за тик, а не за дренаж (ADR-132, ADR-431).
    /// В снимок не входят: тик начинается с `begin_tick`, дренаж вне
    /// тика — с нуля.
    applied_in_tick: usize,
    examined_in_tick: usize,
    gen: Arc<dyn Generator + Send + Sync>,
    /// Слой изменений: `None` — чанк нетронут, материал у генератора.
    chunks: Vec<Option<Arc<Paletted>>>,
    /// Exact applied positions since the last reset. Purely a reset
    /// acceleration hint: loaded worlds and overflowing logs use the full
    /// chunk-column rebuild. It is deliberately absent from snapshots.
    reset_dirty: Option<Vec<[u32; 3]>>,
    cx: u32,
    cy: u32,
    /// Битовый объём занятости, плотный. Слово на 64 воксела по X.
    bits: Vec<u64>,
    /// Потолок колонки: на единицу выше самого высокого занятого
    /// воксела, ноль у пустой (ADR-313). Считается из битового объёма
    /// при сборке мира и правится вместе с ним. Это **сводка**, а не
    /// состояние: в снимок и хеш не входит и восстанавливается из тех же
    /// бит. Навигация искала пол колонки обходом от неба — сто семьдесят
    /// четыре пустые ячейки на колонку полигона.
    ceiling: Vec<u16>,
    /// Derived 8x8-column terrain tiles; a write invalidates its tile immediately.
    floor_tiles: Vec<std::sync::OnceLock<FloorTile>>,
    wx: u32,
    /// Очередь отложенных правок. FIFO — порядок дренажа детерминирован.
    queue: std::collections::VecDeque<Edit>,
    /// Derived counts of queued removals, including transfers. A cell
    /// already paid for must not be claimed again before its edit drains.
    /// Counts retain duplicates from legacy queues; never saved or hashed.
    pending_digs: PendingDigs,
    /// Передачи материи между миром и телом (ADR-431): исходные ячейки
    /// оторванного тела и запекание приземлившегося. Дренаж берёт их
    /// первыми и целиком — их число ограничено тем, кто их ставит, —
    /// чтобы клетки тела не стояли в мире за хвостом копки, где их
    /// снимет копатель, а тело потом запечёт обратно. Своя очередь
    /// FIFO: в голове общей поздний отрыв обгонял бы ранний.
    transfers: std::collections::VecDeque<Edit>,
    /// Правки, применённые в текущем тике. Это и есть поток для рендера
    /// (ADR-103) и вход связности с навигацией.
    applied: Vec<Edit>,
    /// Что **снято** последним дренажем: заказчик и материал, который
    /// в вокселе был. Только настоящие снятия — укладка и пустая правка
    /// сюда не попадают.
    ///
    /// Отдельным списком, а не полем правки: материал «до» известен
    /// только в момент применения, и класть его в очередь значило бы
    /// хранить в ней поле, которое до дренажа не значит ничего.
    taken: Vec<(u32, Mat)>,
    /// Completed requests, including no-ops, with the material before the edit.
    completed: Vec<(Edit, Mat)>,
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

/// Сколько потоков строят битовый объём на старте.
///
/// Число зафиксировано, а не взято у ОС (`CLAUDE.md`): от него зависит
/// только время, но не результат — каждый поток пишет свои слова,
/// а слово это чистая функция генератора, и это проверяется тестом.
/// Восемь — по числу ядер машины замеров (ADR-228).
const FILL_THREADS: usize = 8;

/// Меньше этого колонок объём строится в один поток: запуск потоков
/// стоит дороже самой работы (ADR-080).
const FILL_PARALLEL_FROM: u64 = 1 << 14;

/// Построить битовый объём занятости из генератора.
///
/// **Колонками, а не по вокселю.** Генератор считает высоту поверхности,
/// толщину почвы и узлы трёхмерного шума один раз на колонку (ADR-114);
/// повоксельный обход пересчитывал их на каждый воксел и стоил
/// 213 нс за воксел — на километре это тридцать часов (ADR-228).
///
/// **Параллельно по полосам `z`.** Слова полосы `[z0, z1)` при одном `y`
/// лежат подряд, поэтому объём режется на такие отрезки и каждый поток
/// получает свои. Слово — чистая функция генератора, и число потоков
/// на результат не влияет: это проверяется тестом, а не подразумевается.
///
/// Слово собирается целиком в местном буфере и пишется в объём один
/// раз, а не по биту: ряды разных `y` лежат далеко друг от друга,
/// и запись по биту била бы в новую строку кэша на каждый воксел.
fn fill_bits(
    nx: u32,
    ny: u32,
    nz: u32,
    wx: u32,
    g: &(dyn Generator + Send + Sync),
    threads: usize,
) -> Vec<u64> {
    let (wxu, nyu, nzu) = (wx as usize, ny as usize, nz as usize);
    let mut bits = vec![0u64; wxu * nyu * nzu];
    if nx == 0 || ny == 0 || nz == 0 {
        return bits;
    }
    let threads = threads.clamp(1, nzu);
    // Ширина полосы по `z`; последняя короче.
    let zw = nzu.div_ceil(threads);
    let bands = nzu.div_ceil(zw);
    // Каждый ряд `y` режется на полосы отдельно: последняя полоса ряда
    // короче, и сплошной `chunks_mut` по объёму съехал бы с раскладки.
    let mut per: Vec<Vec<&mut [u64]>> = (0..bands).map(|_| Vec::with_capacity(nyu)).collect();
    for row in bits.chunks_mut(nzu * wxu) {
        for (b, piece) in row.chunks_mut(zw * wxu).enumerate() {
            per[b].push(piece);
        }
    }
    let fill = |b: usize, mut rows: Vec<&mut [u64]>| {
        let z0 = (b * zw) as u32;
        let z1 = ((b + 1) * zw).min(nzu) as u32;
        let mut col = vec![AIR; nyu];
        let mut word = vec![0u64; nyu];
        for z in z0..z1 {
            let zo = (z - z0) as usize * wxu;
            for x0 in (0..nx).step_by(64) {
                word.iter_mut().for_each(|v| *v = 0);
                for x in x0..(x0 + 64).min(nx) {
                    g.column(x, z, 0, &mut col);
                    let sh = x & 63;
                    for (v, &m) in word.iter_mut().zip(col.iter()) {
                        *v |= u64::from(m != AIR) << sh;
                    }
                }
                let w = zo + (x0 >> 6) as usize;
                for (row, &v) in rows.iter_mut().zip(word.iter()) {
                    row[w] = v;
                }
            }
        }
    };
    if bands == 1 {
        fill(0, per.pop().unwrap_or_default());
    } else {
        std::thread::scope(|sc| {
            for (b, rows) in per.into_iter().enumerate() {
                let fill = &fill;
                sc.spawn(move || fill(b, rows));
            }
        });
    }
    bits
}

/// Потолок каждой колонки из готового битового объёма (ADR-313).
///
/// Обход идёт сверху вниз по рядам `y` и помнит, какие колонки уже
/// ответили: пустое небо — это нулевые слова, они пропускаются целиком,
/// а ниже поверхности почти каждая колонка отвечает за первые же ряды.
/// У мира есть дно-рамка (ADR-034), поэтому к `y = 0` отвечают все.
fn fill_ceiling(nx: u32, ny: u32, nz: u32, wx: u32, bits: &[u64]) -> Vec<u16> {
    let (wxu, nzu) = (wx as usize, nz as usize);
    let mut ceiling = vec![0u16; nx as usize * nzu];
    if nx == 0 || ny == 0 || nz == 0 {
        return ceiling;
    }
    let mut done = vec![0u64; wxu * nzu];
    let mut left = nx as usize * nzu;
    for y in (0..ny).rev() {
        if left == 0 {
            break;
        }
        let row = y as usize * nzu * wxu;
        for z in 0..nzu {
            for w in 0..wxu {
                let i = z * wxu + w;
                let fresh = bits[row + i] & !done[i];
                if fresh == 0 {
                    continue;
                }
                done[i] |= fresh;
                let mut rest = fresh;
                while rest != 0 {
                    let b = rest.trailing_zeros();
                    rest &= rest - 1;
                    let x = w as u32 * 64 + b;
                    if x < nx {
                        ceiling[x as usize + nx as usize * z] = (y + 1) as u16;
                        left -= 1;
                    }
                }
            }
        }
    }
    ceiling
}

impl World {
    /// Создать мир и построить битовый объём из генератора.
    ///
    /// Построение — единственное место, где генератор обходится целиком;
    /// дальше мир живёт правками.
    ///
    /// **Колонкой, а не вокселем** (ADR-114, ADR-227). Повоксельный
    /// обход спрашивал у генератора высоту поверхности и четыре узла
    /// решётки шума заново на каждый воксель; колонка считает их раз
    /// на столб. Замерено на 512×256×512: 14.3 с против 2.5 с.
    /// Значение бит в бит то же — на этом стоит
    /// `column_matches_per_voxel_material` в `worldgen`.
    ///
    /// Срез `z` собирается в отдельном буфере и переливается в объём
    /// целыми строками. Писать прямо в `bits` нельзя по цене: у строк
    /// одного столба шаг `nz * wx` слов, то есть промах кэша на каждый
    /// воксель высоты.
    pub fn new(
        nx: u32,
        ny: u32,
        nz: u32,
        max_edits_per_tick: usize,
        g: Box<dyn Generator + Send + Sync>,
    ) -> World {
        let wx = nx.div_ceil(64);
        let threads = if u64::from(nx) * u64::from(nz) < FILL_PARALLEL_FROM {
            1
        } else {
            FILL_THREADS
        };
        let bits = fill_bits(nx, ny, nz, wx, g.as_ref(), threads);
        World::assemble(nx, ny, nz, max_edits_per_tick, g, bits)
    }

    /// Мир поверх **готового** битового объёма — из кэша карты (ADR-228).
    ///
    /// Объём обязан быть тем самым, что построил бы [`World::new`] от
    /// этого же генератора: мир этого не проверяет, за ключ кэша
    /// и выборочную сверку отвечает тот, кто объём прочитал.
    /// Неверная длина — `None`.
    pub fn from_bits(
        nx: u32,
        ny: u32,
        nz: u32,
        max_edits_per_tick: usize,
        g: Box<dyn Generator + Send + Sync>,
        bits: Vec<u64>,
    ) -> Option<World> {
        let wx = nx.div_ceil(64);
        if bits.len() != wx as usize * ny as usize * nz as usize {
            return None;
        }
        Some(World::assemble(nx, ny, nz, max_edits_per_tick, g, bits))
    }

    fn assemble(
        nx: u32,
        ny: u32,
        nz: u32,
        max_edits_per_tick: usize,
        g: Box<dyn Generator + Send + Sync>,
        bits: Vec<u64>,
    ) -> World {
        let (cx, cy, cz) = (
            nx.div_ceil(CS as u32),
            ny.div_ceil(CS as u32),
            nz.div_ceil(CS as u32),
        );
        let ceiling = fill_ceiling(nx, ny, nz, nx.div_ceil(64), &bits);
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
            applied_in_tick: 0,
            examined_in_tick: 0,
            gen: Arc::from(g),
            chunks: (0..(cx * cy * cz) as usize).map(|_| None).collect(),
            reset_dirty: Some(Vec::new()),
            cx,
            cy,
            bits,
            ceiling,
            floor_tiles: (0..nx.div_ceil(8) as usize * nz.div_ceil(8) as usize)
                .map(|_| std::sync::OnceLock::new())
                .collect(),
            wx: nx.div_ceil(64),
            queue: std::collections::VecDeque::new(),
            pending_digs: PendingDigs::new((cx * cy * cz) as usize),
            transfers: std::collections::VecDeque::new(),
            applied: Vec::new(),
            taken: Vec::new(),
            completed: Vec::new(),
            defer_buf: Vec::new(),
            materialized: 0,
        }
    }

    /// На единицу выше самого высокого занятого воксела колонки; ноль,
    /// если колонка пуста или лежит за миром (ADR-313). Тот же ответ,
    /// что у обхода колонки сверху вниз, только без обхода.
    #[inline]
    pub fn column_ceiling(&self, x: u32, z: u32) -> u32 {
        if x >= self.nx || z >= self.nz {
            return 0;
        }
        u32::from(self.ceiling[x as usize + self.nx as usize * z as usize])
    }

    /// Prepare the shared traversal surface during map loading, before orders.
    /// Fixed workers fill disjoint tiles; values are independent of scheduling.
    pub fn prepare_navigation(&self, yielding: [Mat; 2]) {
        let columns = self.nx.div_ceil(8);
        let rows = self.nz.div_ceil(8);
        let band = rows.div_ceil(FILL_THREADS as u32).max(1);
        std::thread::scope(|scope| {
            for first in (0..rows).step_by(band as usize) {
                scope.spawn(move || {
                    for z in first..(first + band).min(rows) {
                        for x in 0..columns {
                            self.floor_tile(x, z, yielding);
                        }
                    }
                });
            }
        });
    }
    /// Terrain heights excluding the two crushable materials. An alternate
    /// material profile remains correct without replacing the resident cache.
    pub fn floor_tile(&self, x: u32, z: u32, yielding: [Mat; 2]) -> Option<[u16; 2]> {
        if x >= self.nx.div_ceil(8) || z >= self.nz.div_ceil(8) {
            return None;
        }
        let i = x as usize + self.nx.div_ceil(8) as usize * z as usize;
        let scan = || {
            let mut bounds = [u16::MAX, 0];
            let mut bare = true;
            let mut cursor = Self::mat_cursor();
            for z in z * 8..((z + 1) * 8).min(self.nz) {
                for x in x * 8..((x + 1) * 8).min(self.nx) {
                    let mut top = 0;
                    let ceiling = self.column_ceiling(x, z);
                    for y in (0..ceiling).rev() {
                        if !self.solid(x, y, z) {
                            continue;
                        }
                        let material = self.material_cached(&mut cursor, x, y, z);
                        if material != AIR && !yielding.contains(&material) {
                            top = (y + 1) as u16;
                            break;
                        }
                    }
                    bounds[0] = bounds[0].min(top);
                    bounds[1] = bounds[1].max(top);
                    bare &= u32::from(top) == ceiling;
                }
            }
            let clear = (bare && bounds[0] == bounds[1]).then_some(bounds[0]);
            (bounds, clear, bare)
        };
        let &(key, bounds, _, _) = self.floor_tiles[i].get_or_init(|| {
            let (bounds, clear, bare) = scan();
            (yielding, bounds, clear, bare)
        });
        Some(if key == yielding { bounds } else { scan().0 })
    }
    /// Use only an already prepared tile with the requested material profile.
    /// A fine terrain probe must not pay for a fresh 8x8 scan on a live tick.
    pub fn prepared_floor_tile(&self, x: u32, z: u32, yielding: [Mat; 2]) -> Option<[u16; 2]> {
        if x >= self.nx.div_ceil(8) || z >= self.nz.div_ceil(8) {
            return None;
        }
        let i = x as usize + self.nx.div_ceil(8) as usize * z as usize;
        self.floor_tiles[i]
            .get()
            .and_then(|(key, bounds, _, _)| (*key == yielding).then_some(*bounds))
    }
    /// Uniform bare floor across an inclusive column box of prepared tiles.
    /// `None` means the caller must check the exact columns instead.
    pub fn prepared_flat_area(
        &self,
        lo: [i32; 2],
        hi: [i32; 2],
        yielding: [Mat; 2],
    ) -> Option<u16> {
        if lo[0] < 0
            || lo[1] < 0
            || hi[0] < lo[0]
            || hi[1] < lo[1]
            || hi[0] >= self.nx as i32
            || hi[1] >= self.nz as i32
        {
            return None;
        }
        let stride = self.nx.div_ceil(8) as usize;
        let mut level = None;
        for z in (lo[1] as usize / 8)..=(hi[1] as usize / 8) {
            for x in (lo[0] as usize / 8)..=(hi[0] as usize / 8) {
                let (key, _, clear, _) = self.floor_tiles[x + stride * z].get()?;
                if *key != yielding {
                    return None;
                }
                let h = (*clear)?;
                if level.is_some_and(|was| was != h) {
                    return None;
                }
                level = Some(h);
            }
        }
        level
    }
    /// Highest floor in a prepared box whose columns contain no yielding
    /// material above that floor. A coarse pose may use this bound to prove
    /// its belly clear while retaining exact contacts on uneven terrain.
    pub fn prepared_bare_upper(
        &self,
        lo: [i32; 2],
        hi: [i32; 2],
        yielding: [Mat; 2],
    ) -> Option<u16> {
        self.prepared_upper(lo, hi, yielding, true)
    }
    /// Highest non-yielding floor in a prepared box. This bound cannot prove
    /// the absence of crushable vegetation; it only proves terrain clearance.
    pub fn prepared_floor_upper(
        &self,
        lo: [i32; 2],
        hi: [i32; 2],
        yielding: [Mat; 2],
    ) -> Option<u16> {
        self.prepared_upper(lo, hi, yielding, false)
    }
    fn prepared_upper(
        &self,
        lo: [i32; 2],
        hi: [i32; 2],
        yielding: [Mat; 2],
        require_bare: bool,
    ) -> Option<u16> {
        if lo[0] < 0
            || lo[1] < 0
            || hi[0] < lo[0]
            || hi[1] < lo[1]
            || hi[0] >= self.nx as i32
            || hi[1] >= self.nz as i32
        {
            return None;
        }
        let stride = self.nx.div_ceil(8) as usize;
        let mut high = 0;
        for z in (lo[1] as usize / 8)..=(hi[1] as usize / 8) {
            for x in (lo[0] as usize / 8)..=(hi[0] as usize / 8) {
                let (key, bounds, _, bare) = self.floor_tiles[x + stride * z].get()?;
                if *key != yielding || (require_bare && !bare) {
                    return None;
                }
                high = high.max(bounds[1]);
            }
        }
        Some(high)
    }
    fn forget_floor_tile(&mut self, x: u32, z: u32) {
        let i = (x / 8) as usize + self.nx.div_ceil(8) as usize * (z / 8) as usize;
        self.floor_tiles[i].take();
    }
    #[inline]
    pub fn inside(&self, x: u32, y: u32, z: u32) -> bool {
        x < self.nx && y < self.ny && z < self.nz
    }

    /// Лежит ли воксель на **рамке мира**: дно и четыре боковые стенки
    /// (ADR-034). Рамка не копается: правка в неё отбрасывается
    /// на постановке в очередь, как правка за пределы мира.
    ///
    /// Потолка сверху у рамки нет — там воздух, и снять его нечем.
    ///
    /// Это не служебный материал, а правило очереди: материал рамки
    /// остаётся тем, что дал генератор, и в хеш мира она не входит
    /// отдельно. Заземление в каскаде (`collapse`) идёт по дну мира,
    /// и рамка гарантирует, что дно на месте.
    ///
    /// Нашлось на ребре 0.1 м (ADR-227): шар копателя радиусом в метр
    /// стал десятью вокселями и, дойдя до дна, снимал у мира диск
    /// в два метра. Колонки под ним оставались без пола, нав-клетка —
    /// без единого уровня, и юнит на дне шахты висел в воздухе,
    /// не падая и не стоя. На ребре 0.5 м тот же шар в две клетки
    /// снимал у дна один воксель, и соседние столбы клетки держали пол.
    #[inline]
    pub fn on_frame(&self, x: u32, y: u32, z: u32) -> bool {
        y == 0 || x == 0 || z == 0 || x + 1 == self.nx || z + 1 == self.nz
    }

    #[inline]
    fn chunk_index(&self, x: u32, y: u32, z: u32) -> usize {
        let (a, b, c) = (x / CS as u32, y / CS as u32, z / CS as u32);
        ((c * self.cy + b) * self.cx + a) as usize
    }

    /// Слово и маска воксела. Счёт в `usize`, а не в `u32`: на карте
    /// в километр рядов `ny·nz` больше, чем влезает в тридцать два бита
    /// слов (ADR-228).
    #[inline]
    fn bit_index(&self, x: u32, y: u32, z: u32) -> (usize, u64) {
        (
            (y as usize * self.nz as usize + z as usize) * self.wx as usize + (x >> 6) as usize,
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

    /// До 64 вокселей занятости подряд вдоль `+X`, начиная с `x`:
    /// бит `i` — воксел `x + i`. Всё за краем мира и дальше `n` — ноль,
    /// ровно как у [`World::solid`].
    ///
    /// Нужен тому, кто читает столб по многу раз. Навигация сканирует
    /// клетку в `step` колонок и прежде спрашивала каждый их воксел
    /// отдельно; словом она берёт весь ряд клетки разом, и обращений
    /// к объёму становится в `step` раз меньше (ADR-228).
    #[inline]
    pub fn solid_bits(&self, x: u32, y: u32, z: u32, n: u32) -> u64 {
        if x >= self.nx || y >= self.ny || z >= self.nz || n == 0 {
            return 0;
        }
        let (w, _) = self.bit_index(x, y, z);
        let sh = x & 63;
        let lo = self.bits[w] >> sh;
        let hi = if sh != 0 && (x >> 6) + 1 < self.wx {
            self.bits[w + 1] << (64 - sh)
        } else {
            0
        };
        let v = lo | hi;
        let keep = n.min(64).min(self.nx - x);
        if keep >= 64 {
            v
        } else {
            v & ((1u64 << keep) - 1)
        }
    }

    /// Any occupied voxel in the inclusive box. Outside-world voxels are
    /// empty, exactly as for `solid`; inverted boxes are empty as well.
    /// The current bit volume is read directly, including same-tick edits.
    #[inline]
    pub fn any_solid_box(&self, lo: [u32; 3], hi: [u32; 3]) -> bool {
        let size = [self.nx, self.ny, self.nz];
        if (0..3).any(|a| lo[a] >= size[a] || lo[a] > hi[a]) {
            return false;
        }
        let hi = [0, 1, 2].map(|a| hi[a].min(size[a] - 1));
        let first = (lo[0] >> 6) as usize;
        let last = (hi[0] >> 6) as usize;
        let low_mask = u64::MAX << (lo[0] & 63);
        let high_mask = u64::MAX >> (63 - (hi[0] & 63));
        let stride = self.wx as usize;
        // Most physical boxes fit in one X word. Compute its mask once;
        // Y/Z traversal follows the backing volume's contiguous row order.
        if first == last {
            let mask = low_mask & high_mask;
            for y in lo[1]..=hi[1] {
                let mut row = (y as usize * self.nz as usize + lo[2] as usize) * stride + first;
                for _ in lo[2]..=hi[2] {
                    if self.bits[row] & mask != 0 {
                        return true;
                    }
                    row += stride;
                }
            }
        } else {
            for y in lo[1]..=hi[1] {
                let mut row = (y as usize * self.nz as usize + lo[2] as usize) * stride;
                for _ in lo[2]..=hi[2] {
                    if self.bits[row + first] & low_mask != 0
                        || self.bits[row + first + 1..row + last]
                            .iter()
                            .any(|&v| v != 0)
                        || self.bits[row + last] & high_mask != 0
                    {
                        return true;
                    }
                    row += stride;
                }
            }
        }
        false
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
    /// границы. Замер, правда, показал, что не деления были дороги —
    /// но курсор ничего не стоит и остался.
    pub fn mat_cursor() -> MatCursor {
        MatCursor {
            ci: 0,
            ox: 0,
            oy: 0,
            oz: 0,
            valid: false,
        }
    }

    /// Материал с курсором. Ответ **тот же до бита**, что у
    /// [`World::material`]: это ускорение, а не приближение.
    pub fn material_cached(&self, c: &mut MatCursor, x: u32, y: u32, z: u32) -> Mat {
        if !self.solid(x, y, z) {
            return AIR;
        }
        let cs = CS as u32;
        if !c.valid
            || x < c.ox
            || x - c.ox >= cs
            || y < c.oy
            || y - c.oy >= cs
            || z < c.oz
            || z - c.oz >= cs
        {
            let (a, b, d) = (x / cs, y / cs, z / cs);
            c.ci = ((d * self.cy + b) * self.cx + a) as usize;
            c.ox = a * cs;
            c.oy = b * cs;
            c.oz = d * cs;
            c.valid = true;
        }
        match &self.chunks[c.ci] {
            Some(k) => k.get(
                (x - c.ox) as usize,
                (y - c.oy) as usize,
                (z - c.oz) as usize,
            ),
            None => self.gen.material(x, y, z),
        }
    }

    pub fn material(&self, x: u32, y: u32, z: u32) -> Mat {
        // Occupancy is exact for the committed generator/chunk overlay. Air
        // needs neither palette decoding nor a procedural terrain evaluation.
        // Pending edits become visible only when apply updates both layers.
        if !self.solid(x, y, z) {
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

    /// Exact column from the procedural layer overlaid by edited chunks.
    /// Queries outside the world return air, just like `material`.
    pub fn column(&self, x: u32, z: u32, y0: u32, out: &mut [Mat]) {
        out.fill(AIR);
        if x >= self.nx || z >= self.nz || y0 >= self.ny {
            return;
        }
        let n = out.len().min((self.ny - y0) as usize);
        let mut offset = 0;
        while offset < n {
            let y = y0 + offset as u32;
            let count = (CS - y as usize % CS).min(n - offset);
            if let Some(chunk) = &self.chunks[self.chunk_index(x, y, z)] {
                for (dy, dst) in out[offset..offset + count].iter_mut().enumerate() {
                    *dst = chunk.get(x as usize % CS, y as usize % CS + dy, z as usize % CS);
                }
                offset += count;
            } else {
                // Evaluate only pristine spans, once across adjacent pristine
                // chunks. An edited chunk owns its whole material column;
                // evaluating the generator under it is discarded work.
                let start = offset;
                offset += count;
                while offset < n {
                    let next_y = y0 + offset as u32;
                    if self.chunks[self.chunk_index(x, next_y, z)].is_some() {
                        break;
                    }
                    offset += (CS - next_y as usize % CS).min(n - offset);
                }
                self.gen
                    .column(x, z, y0 + start as u32, &mut out[start..offset]);
            }
        }
    }

    /// Whether the procedural source opts into column-based material queries.
    pub fn batches_material_columns(&self) -> bool {
        self.gen.batches_columns()
    }

    /// Restore the authored map without rebuilding untouched occupancy.
    /// When the edit log is complete, only changed cells need restoring;
    /// otherwise the materialized chunk columns take the full path. Existing
    /// immutable snapshots retain their old edited chunks through Arc.
    pub fn restore_pristine(&mut self) {
        self.restore_columns(&[]);
    }

    /// Replace the authored generator at an editor/reset boundary. Every
    /// changed procedural column must lie in a supplied half-open XZ area.
    /// Dimensions are unchanged; previous runtime edits are discarded too.
    pub fn reset_generator_regions(
        &mut self,
        generator: Box<dyn Generator + Send + Sync>,
        areas: &[([i32; 2], [i32; 2])],
    ) {
        self.gen = Arc::from(generator);
        self.restore_columns(areas);
    }

    fn restore_columns(&mut self, areas: &[([i32; 2], [i32; 2])]) {
        if areas.is_empty() {
            if let Some(mut positions) = self.reset_dirty.take() {
                self.restore_sparse_positions(&mut positions);
                positions.clear();
                self.reset_dirty = Some(positions);
                self.finish_reset();
                return;
            }
        }
        // None means authored geometry changed through the whole height;
        // otherwise only these previously edited Y chunks need rebuilding.
        let mut columns: std::collections::BTreeMap<(u32, u32), Option<Vec<u32>>> =
            std::collections::BTreeMap::new();
        for &(lo, hi) in areas {
            let lo = [
                lo[0].clamp(0, self.nx as i32) as u32,
                lo[1].clamp(0, self.nz as i32) as u32,
            ];
            let hi = [
                hi[0].clamp(0, self.nx as i32) as u32,
                hi[1].clamp(0, self.nz as i32) as u32,
            ];
            if lo[0] >= hi[0] || lo[1] >= hi[1] {
                continue;
            }
            for z in lo[1] / CS as u32..hi[1].div_ceil(CS as u32) {
                for x in lo[0] / CS as u32..hi[0].div_ceil(CS as u32) {
                    columns.insert((x, z), None);
                }
            }
        }
        for (i, chunk) in self.chunks.iter_mut().enumerate() {
            if chunk.take().is_some() {
                let ci = i as u32;
                if let Some(chunks) = columns
                    .entry((ci % self.cx, ci / (self.cx * self.cy)))
                    .or_insert_with(|| Some(Vec::new()))
                {
                    chunks.push(ci / self.cx % self.cy);
                }
            }
        }
        let mut column = vec![AIR; self.ny as usize];
        let mut rows = vec![0u32; self.ny as usize];
        for ((cx, cz), changed_y) in columns {
            for z in (cz * CS as u32..((cz + 1) * CS as u32).min(self.nz)).step_by(8) {
                for x in (cx * CS as u32..((cx + 1) * CS as u32).min(self.nx)).step_by(8) {
                    self.forget_floor_tile(x, z);
                }
            }
            let x0 = cx * CS as u32;
            let x1 = ((cx + 1) * CS as u32).min(self.nx);
            let shift = x0 % 64;
            let mask = ((1u64 << (x1 - x0)) - 1) << shift;
            let full_height = changed_y.is_none();
            let segments = if let Some(mut chunks) = changed_y {
                chunks.sort_unstable();
                chunks.dedup();
                chunks
                    .into_iter()
                    .map(|cy| (cy * CS as u32, ((cy + 1) * CS as u32).min(self.ny)))
                    .collect::<Vec<_>>()
            } else {
                vec![(0, self.ny)]
            };
            for z in cz * CS as u32..((cz + 1) * CS as u32).min(self.nz) {
                rows.fill(0);
                for x in x0..x1 {
                    for &(y0, y1) in &segments {
                        self.gen
                            .column(x, z, y0, &mut column[y0 as usize..y1 as usize]);
                        for (row, &m) in rows[y0 as usize..y1 as usize]
                            .iter_mut()
                            .zip(&column[y0 as usize..y1 as usize])
                        {
                            *row |= u32::from(m != AIR) << (x - x0);
                        }
                    }
                    if full_height {
                        let top = column
                            .iter()
                            .rposition(|&m| m != AIR)
                            .map_or(0, |y| y as u16 + 1);
                        self.ceiling[x as usize + self.nx as usize * z as usize] = top;
                    }
                }
                // A chunk's 32 X cells share half of one occupancy word.
                // Commit each row once, preserving its neighbouring half.
                for &(y0, y1) in &segments {
                    for y in y0..y1 {
                        let (i, _) = self.bit_index(x0, y, z);
                        self.bits[i] =
                            (self.bits[i] & !mask) | (u64::from(rows[y as usize]) << shift);
                    }
                }
                if !full_height {
                    // The edited layer might contain the old or the new
                    // surface. If the old top was above all restored layers,
                    // no edit could move it; otherwise nothing is solid above
                    // the highest restored layer, so the search starts there.
                    let highest_restored = segments.last().unwrap().1;
                    let mut pending = 0u32;
                    for x in x0..x1 {
                        if u32::from(self.ceiling[x as usize + self.nx as usize * z as usize])
                            <= highest_restored
                        {
                            pending |= 1 << (x - x0);
                        }
                    }
                    for y in (0..highest_restored).rev() {
                        let (i, _) = self.bit_index(x0, y, z);
                        let mut fresh = ((self.bits[i] & mask) >> shift) as u32 & pending;
                        while fresh != 0 {
                            let bit = fresh.trailing_zeros();
                            fresh &= fresh - 1;
                            pending &= !(1 << bit);
                            let x = x0 + bit;
                            self.ceiling[x as usize + self.nx as usize * z as usize] =
                                (y + 1) as u16;
                        }
                        if pending == 0 {
                            break;
                        }
                    }
                    while pending != 0 {
                        let bit = pending.trailing_zeros();
                        pending &= pending - 1;
                        let x = x0 + bit;
                        self.ceiling[x as usize + self.nx as usize * z as usize] = 0;
                    }
                }
            }
        }
        self.reset_dirty = Some(Vec::new());
        self.finish_reset();
    }

    fn restore_sparse_positions(&mut self, positions: &mut Vec<[u32; 3]>) {
        positions.sort_unstable_by_key(|&[x, y, z]| (x, z, y));
        positions.dedup();
        let mut column = None;
        for &[x, y, z] in positions.iter() {
            if column != Some((x, z)) {
                if let Some((old_x, old_z)) = column {
                    self.refresh_ceiling(old_x, old_z);
                }
                column = Some((x, z));
            }
            let ci = self.chunk_index(x, y, z);
            self.chunks[ci] = None;
            let solid = self.gen.material(x, y, z) != AIR;
            let (word, mask) = self.bit_index(x, y, z);
            if solid {
                self.bits[word] |= mask;
            } else {
                self.bits[word] &= !mask;
            }
        }
        if let Some((x, z)) = column {
            self.refresh_ceiling(x, z);
        }
    }

    fn refresh_ceiling(&mut self, x: u32, z: u32) {
        self.forget_floor_tile(x, z);
        let top = (0..self.ny)
            .rev()
            .find(|&y| {
                let (word, mask) = self.bit_index(x, y, z);
                self.bits[word] & mask != 0
            })
            .map_or(0, |y| y + 1);
        self.ceiling[x as usize + self.nx as usize * z as usize] = top as u16;
    }

    fn finish_reset(&mut self) {
        self.queue.clear();
        self.transfers.clear();
        self.pending_digs.clear();
        self.applied_in_tick = 0;
        self.examined_in_tick = 0;
        self.applied.clear();
        self.taken.clear();
        self.completed.clear();
        self.defer_buf.clear();
        self.materialized = 0;
        self.materialized_in_tick = 0;
        self.in_tick = false;
    }

    /// Материализовать чанк из генератора — цена первой правки в нём.
    fn materialize(&mut self, ci: usize) {
        if self.chunks[ci].is_some() {
            return;
        }
        self.chunks[ci] = Some(Arc::new(self.build_chunk(ci)));
        self.materialized += 1;
    }

    /// Номер чанка, в котором лежит воксель внутри мира.
    pub fn chunk_of(&self, x: u32, y: u32, z: u32) -> Option<usize> {
        self.inside(x, y, z).then(|| self.chunk_index(x, y, z))
    }

    /// Чанк ещё не тронут: его содержимое отдаёт генератор.
    pub fn chunk_fresh(&self, ci: usize) -> bool {
        self.chunks[ci].is_none()
    }

    /// Чанк таким, каким его даёт генератор: чистая функция генератора,
    /// поэтому строить его можно на любом потоке, а ставить —
    /// [`World::install_chunk`] (ADR-681).
    pub fn build_chunk(&self, ci: usize) -> Paletted {
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
        p
    }

    /// Поставить чанк, построенный заранее [`World::build_chunk`]
    /// (ADR-681). Тронутый чанк не заменяется. Идёт в счёт построенных
    /// чанков, но не в потолок материализаций дренажа: этот потолок
    /// бережёт тик от построения в очереди, а заранее строит и
    /// ограничивает своим потолком тот, кто ставит.
    pub fn install_chunk(&mut self, ci: usize, chunk: Paletted) {
        if self.chunks[ci].is_none() {
            self.chunks[ci] = Some(Arc::new(chunk));
            self.materialized += 1;
        }
    }

    /// Поставить правку в очередь. Потолок применяется при дренаже,
    /// а не здесь: очередь не имеет длины по построению — иначе приказ
    /// молча терялся бы.
    ///
    /// Правка за пределы мира и правка в рамку (ADR-034) отбрасываются
    /// здесь же: они не заняли бы ни воксела, а очередь и просмотр
    /// при дренаже заняли бы.
    pub fn queue_edit(&mut self, e: Edit) {
        if self.inside(e.x, e.y, e.z) && !self.on_frame(e.x, e.y, e.z) {
            self.remember_dig(e);
            self.queue.push_back(e);
        }
    }

    /// Поставить передачу материи между миром и телом (ADR-431): её
    /// дренаж берёт раньше приказов и целиком. Рамка и то, что вне
    /// мира, отбрасываются, как у приказа.
    pub fn queue_transfer(&mut self, e: Edit) {
        if self.inside(e.x, e.y, e.z) && !self.on_frame(e.x, e.y, e.z) {
            self.remember_dig(e);
            self.transfers.push_back(e);
        }
    }

    pub fn queue_dig(&mut self, x: u32, y: u32, z: u32) {
        self.queue_edit(Edit::dig(x, y, z));
    }

    /// Снять воксель **за счёт названного слота**. Материал, который
    /// там был, попадёт в [`World::taken`], и экономика начислит его
    /// заказчику.
    pub fn queue_dig_by(&mut self, x: u32, y: u32, z: u32, by: u32) {
        self.queue_edit(Edit {
            x,
            y,
            z,
            mat: AIR,
            by,
        });
    }

    pub fn queued(&self) -> usize {
        self.queue.len() + self.transfers.len()
    }

    /// Edits waiting in the queue, in drain order; transfers apart.
    pub fn queued_edits(&self) -> impl Iterator<Item = &Edit> + '_ {
        self.queue.iter()
    }

    /// The chunk and the voxel within it that a queued dig is counted at.
    fn dig_key(&self, x: u32, y: u32, z: u32) -> (usize, usize) {
        let cs = CS as u32;
        (
            self.chunk_index(x, y, z),
            (((z % cs) * cs + y % cs) * cs + x % cs) as usize,
        )
    }
    fn remember_dig(&mut self, e: Edit) {
        if e.mat == AIR {
            let key = self.dig_key(e.x, e.y, e.z);
            self.pending_digs.add(key);
        }
    }
    fn forget_dig(&mut self, e: Edit) {
        if e.mat == AIR {
            let key = self.dig_key(e.x, e.y, e.z);
            self.pending_digs.remove(key);
        }
    }
    fn rebuild_pending_digs(&mut self) {
        self.pending_digs.clear();
        let digs: Vec<(usize, usize)> = self
            .queue
            .iter()
            .chain(&self.transfers)
            .filter(|e| e.mat == AIR)
            .map(|e| self.dig_key(e.x, e.y, e.z))
            .collect();
        for key in digs {
            self.pending_digs.add(key);
        }
    }
    /// A removal already waits in either queue. Live material is unchanged
    /// until drain, but another producer must wait instead of paying twice.
    pub fn has_pending_dig(&self, x: u32, y: u32, z: u32) -> bool {
        self.inside(x, y, z) && self.pending_digs.contains(self.dig_key(x, y, z))
    }

    /// Передачи между миром и телом, ждущие дренажа (ADR-431).
    pub fn transfers_queued(&self) -> usize {
        self.transfers.len()
    }

    /// Сами ждущие передачи, в порядке дренажа: клетка, которую снимет
    /// передача, уже отдана телу или осыпи (ADR-439).
    pub fn transfers(&self) -> impl Iterator<Item = &Edit> + '_ {
        self.transfers.iter()
    }

    /// Дренаж очереди в пределах потолка. Возвращает статистику;
    /// сам список применённых правок — [`World::edits`].
    /// Объявить начало тика: потолок материализаций считается от сюда.
    pub fn begin_tick(&mut self) {
        self.in_tick = true;
        self.materialized_in_tick = 0;
        self.applied_in_tick = 0;
        self.examined_in_tick = 0;
    }

    pub fn flush(&mut self) -> FlushStats {
        if !self.in_tick {
            self.materialized_in_tick = 0;
            self.applied_in_tick = 0;
            self.examined_in_tick = 0;
        }
        self.applied.clear();
        self.taken.clear();
        self.completed.clear();
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
        // Передачи между миром и телом — первыми и целиком (ADR-431).
        // Их держит потолок того, кто их ставит (тел за тик, клеток
        // в теле), а не потолок дренажа: отложенная передача оставляет
        // одну материю и в теле, и в мире. Свежий чанк им тоже не
        // помеха, но в счёт материализаций тика идёт.
        self.drain_transfers(&mut st);
        let mut deferred = std::mem::take(&mut self.defer_buf);
        deferred.clear();
        while self.applied_in_tick < self.max_edits_per_tick
            && deferred.len() < self.max_edits_per_tick
            && self.examined_in_tick < self.max_edits_per_tick
        {
            let Some(e) = self.queue.pop_front() else {
                break;
            };
            st.examined += 1;
            self.examined_in_tick += 1;
            let ci = self.chunk_index(e.x, e.y, e.z);
            if self.chunks[ci].is_none()
                && self.materialized_in_tick >= self.max_materialize_per_tick
            {
                st.materialize_capped = true;
                deferred.push(e);
                continue;
            }
            self.take(e, &mut st);
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

    /// Complete the body/world transfers produced after the main drain.
    /// Append to its deltas: accounting, navigation and publication still
    /// need every earlier edit. Producers bound transfers (ADR-431); normal
    /// edits keep their FIFO and the already consumed tick allowance.
    pub fn flush_transfers(&mut self) -> FlushStats {
        let mut st = FlushStats::default();
        self.drain_transfers(&mut st);
        st.queued = self.queue.len();
        st
    }

    fn drain_transfers(&mut self, st: &mut FlushStats) {
        while let Some(e) = self.transfers.pop_front() {
            st.examined += 1;
            self.examined_in_tick += 1;
            self.take(e, st);
        }
    }

    /// Применить одну правку дренажа и учесть её: в список завершённых,
    /// изъятого и применённого, в счёт тика.
    fn take(&mut self, e: Edit, st: &mut FlushStats) {
        self.forget_dig(e);
        let fresh = self.chunks[self.chunk_index(e.x, e.y, e.z)].is_none();
        let previous = self.material(e.x, e.y, e.z);
        self.completed.push((e, previous));
        if e.expected_material()
            .is_some_and(|expected| expected != previous)
        {
            st.noop += 1;
            return;
        }
        // Nothing changed the voxel since `previous` was read.
        if let Some(was) = self.apply(e, previous) {
            if fresh {
                st.materialized += 1;
                self.materialized_in_tick += 1;
            }
            // Снятое — в список изъятого: экономика начислит его
            // заказчику, а бесхозное отправит в объявленный сток.
            if e.mat == AIR && was != AIR {
                self.taken.push((e.by, was));
            }
            self.applied.push(e);
            st.applied += 1;
            self.applied_in_tick += 1;
        } else {
            st.noop += 1;
        }
    }

    /// Применить одну правку. Возвращает материал, **который в вокселе
    /// был**, либо `None`, если правка ничего не изменила.
    ///
    /// Материал «до» возвращается, а не выбрасывается, потому что он
    /// нужен экономике: выкопанное начисляется по тому, что сняли,
    /// а не по тому, что осталось (`docs/07-economy.md`). Читается он
    /// здесь всё равно: `was` — материал вокселя сейчас.
    fn apply(&mut self, e: Edit, was: Mat) -> Option<Mat> {
        if was == e.mat {
            return None;
        }
        self.forget_floor_tile(e.x, e.z);
        let ci = self.chunk_index(e.x, e.y, e.z);
        self.materialize(ci);
        if let Some(c) = &mut self.chunks[ci] {
            Arc::make_mut(c).set(
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
        // The column's ceiling moves with the bit (ADR-313). Filling can
        // only raise it; emptying lowers it only when the cell taken was
        // the top one, and then the next cell down is found by a walk
        // that starts where the old ceiling was, not at the sky.
        let c = e.x as usize + self.nx as usize * e.z as usize;
        if e.mat != AIR {
            self.ceiling[c] = self.ceiling[c].max((e.y + 1) as u16);
        } else if u32::from(self.ceiling[c]) == e.y + 1 {
            let mut top = 0;
            for y in (0..e.y).rev() {
                let (w, m) = self.bit_index(e.x, y, e.z);
                if self.bits[w] & m != 0 {
                    top = y + 1;
                    break;
                }
            }
            self.ceiling[c] = top as u16;
        }
        // Keeping the common sparse reset proportional to actual edits
        // costs one append here. A long battle falls back to the proven
        // full-chunk path instead of retaining an unbounded history.
        const RESET_LOG_LIMIT: usize = 65_536;
        if let Some(positions) = &mut self.reset_dirty {
            if positions.len() < RESET_LOG_LIMIT {
                positions.push([e.x, e.y, e.z]);
            } else {
                self.reset_dirty = None;
            }
        }
        Some(was)
    }

    /// Правки, применённые в последнем дренаже. Поток для рендера
    /// (ADR-103), вход связности и переворотов нав-клеток.
    pub fn edits(&self) -> &[Edit] {
        &self.applied
    }

    /// Что снято последним дренажем: заказчик и материал, который
    /// в вокселе был. Вход экономики (`docs/07-economy.md`).
    pub fn completed(&self) -> &[(Edit, Mat)] {
        &self.completed
    }

    pub fn taken(&self) -> &[(u32, Mat)] {
        &self.taken
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

    /// Передачи между миром и телом, ещё не прошедшие дренаж (ADR-431).
    /// Отдельно от [`World::save`]: формат слоя изменений прежний,
    /// а очередь передач владелец снимка пишет в свой хвост.
    pub fn save_transfers(&self, b: &mut fx::blob::Blob) {
        let t: Vec<Edit> = self.transfers.iter().copied().collect();
        save_edits(b, self.nx, self.nz, &t);
    }

    pub fn load_transfers(&mut self, s: &mut fx::blob::Scan) -> bool {
        self.transfers = load_edits(s, self.nx, self.ny, self.nz)
            .into_iter()
            .collect();
        self.rebuild_pending_digs();
        s.ok()
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
        // The wire stores final chunk contents, not the edit history.
        self.reset_dirty = None;
        let n = s.list(4);
        let mut touched_columns = Vec::with_capacity(n);
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
            self.chunks[ci] = Some(Arc::new(p));
            self.refresh_bits_of(ci);
            touched_columns.push(((ci as u32) % self.cx, (ci as u32) / (self.cx * self.cy)));
        }
        // The loaded chunk overrides generator occupancy. Rebuild only
        // horizontal chunk columns that changed, after all Y layers have
        // arrived; otherwise an earlier layer can hide a later one.
        touched_columns.sort_unstable();
        touched_columns.dedup();
        for (cx, cz) in touched_columns {
            for z in cz * CS as u32..((cz + 1) * CS as u32).min(self.nz) {
                for x in cx * CS as u32..((cx + 1) * CS as u32).min(self.nx) {
                    let top = (0..self.ny)
                        .rev()
                        .find(|&y| {
                            let (word, mask) = self.bit_index(x, y, z);
                            self.bits[word] & mask != 0
                        })
                        .map_or(0, |y| y + 1);
                    self.forget_floor_tile(x, z);
                    self.ceiling[x as usize + self.nx as usize * z as usize] = top as u16;
                }
            }
        }
        self.queue = load_edits(s, self.nx, self.ny, self.nz)
            .into_iter()
            .collect();
        self.applied = load_edits(s, self.nx, self.ny, self.nz);
        // Снимок старше очереди передач её не несёт: пустая.
        self.transfers.clear();
        self.rebuild_pending_digs();
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
/// это подряд идущие номера с одним материалом и одним заказчиком.
/// Отрезок пишется разностью от конца прошлого (зигзаг), материалом,
/// заказчиком и длиной, всё переменной длиной.
///
/// **Заказчик входит в отрезок, а не теряется** (ADR-230): выкопанное
/// начисляется тому, кто копал, и правка без заказчика ушла бы в сток.
/// Отрезки от этого не мельчают: шар копателя заказан целиком одним.
///
/// **Порядок сохраняется точно.** Очередь FIFO, и отрезок — это
/// запись о подряд идущих её элементах, а не их пересортировка.
pub fn save_edits(b: &mut fx::blob::Blob, nx: u32, nz: u32, e: &[Edit]) {
    let at = |x: u32, y: u32, z: u32| (y as u64 * nz as u64 + z as u64) * nx as u64 + x as u64;
    b.u32(e.len() as u32);
    let mut runs: Vec<(u64, Mat, u32, u32)> = Vec::new();
    for x in e {
        let i = at(x.x, x.y, x.z);
        match runs.last_mut() {
            Some((s, m, n, by)) if *m == x.mat && *by == x.by && *s + u64::from(*n) == i => *n += 1,
            _ => runs.push((i, x.mat, 1, x.by)),
        }
    }
    b.u32(runs.len() as u32);
    let mut prev = 0i64;
    for &(i, m, n, by) in &runs {
        b.ivar(i as i64 - prev);
        b.uvar(u64::from(m));
        b.uvar(u64::from(n));
        b.uvar(u64::from(by));
        prev = i as i64 + i64::from(n);
    }
}

pub fn load_edits(s: &mut fx::blob::Scan, nx: u32, ny: u32, nz: u32) -> Vec<Edit> {
    let total = s.u32() as usize;
    // A cell past the top of the world would index past the occupancy
    // volume when the edit is applied (ADR-464).
    let cells = u64::from(nx) * u64::from(ny) * u64::from(nz);
    let nruns = s.list(4);
    let mut out = Vec::with_capacity(total.min(1 << 24));
    let mut prev = 0i64;
    for _ in 0..nruns {
        let start = prev + s.ivar();
        let m = s.uvar() as Mat;
        let n = s.uvar();
        let by = s.uvar() as u32;
        // Бессмыслица **по смыслу**, а не по длине: отрицательное
        // начало отрезка или отрезок длиннее всего списка. Молча
        // вернуть пустоту нельзя — обрезанный файл прочитался бы
        // как пустая очередь, то есть как другая партия.
        if start < 0
            || n > total as u64
            || out.len() as u64 + n > total as u64
            || start as u64 + n > cells
        {
            s.spoil();
            return Vec::new();
        }
        for k in 0..n {
            let i = start as u64 + k;
            let x = (i % nx as u64) as u32;
            let z = ((i / nx as u64) % nz as u64) as u32;
            let y = (i / (nx as u64 * nz as u64)) as u32;
            out.push(Edit {
                x,
                y,
                z,
                mat: m,
                by,
            });
        }
        prev = start + n as i64;
    }
    if out.len() != total {
        s.spoil();
        return Vec::new();
    }
    out
}

#[cfg(test)]
mod codec_tests {
    use super::*;
    use fx::blob::{Blob, Scan};
    use fx::rng::Rng;

    /// **Правка выше мира — испорченный файл** (ADR-464): воксель за
    /// верхом объёма при применении правки читался бы мимо битового
    /// объёма, то есть паникой, а не ошибкой чтения.
    #[test]
    fn an_edit_above_the_world_is_refused() {
        let mut b = Blob::new();
        let above = Edit {
            x: 3,
            y: 64,
            z: 5,
            mat: 2,
            by: 0,
        };
        save_edits(&mut b, 64, 64, &[above]);
        let mut s = Scan::new(&b.v);
        assert!(load_edits(&mut s, 64, 64, 64).is_empty());
        assert!(!s.ok());
        let mut s = Scan::new(&b.v);
        assert_eq!(load_edits(&mut s, 64, 65, 64), vec![above]);
        assert!(s.ok());
    }

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
            // Смесь трёх видов: шар копателя (подряд идущие номера
            // от одного заказчика), одиночные правки вразброс
            // от разных и повтор одного воксела разным материалом —
            // сжатие обязано пережить все три.
            for who in 0..4u32 {
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
                                by: who,
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
                    by: r.below(7),
                });
            }
            // Тот же воксель, тот же материал, разные заказчики:
            // заказчик обязан разрывать отрезок, иначе выкопанное
            // после загрузки пошло бы не тому (ADR-230).
            for m in 0..6u16 {
                want.push(Edit {
                    x: 7,
                    y: 7,
                    z: 7,
                    mat: m,
                    by: m as u32,
                });
            }
            for who in 0..4u32 {
                want.push(Edit {
                    x: 9,
                    y: 9,
                    z: 9,
                    mat: AIR,
                    by: who,
                });
            }
            let mut b = Blob::new();
            save_edits(&mut b, nx, nz, &want);
            let mut s = Scan::new(&b.v);
            let got = load_edits(&mut s, nx, ny, nz);
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
        assert!(load_edits(&mut s, 64, 64, 64).is_empty());
        assert!(s.ok());
    }

    /// **Обрезанный список правок — порча, а не пустая очередь.**
    ///
    /// Разница существенная: пустая очередь это законное состояние,
    /// и вернув её молча, читатель отдал бы другую партию вместо
    /// отказа.
    #[test]
    fn a_truncated_edit_list_is_spoiled_not_empty() {
        let want: Vec<Edit> = (0..50)
            .map(|k| Edit {
                x: k,
                y: 3,
                z: 5,
                mat: 2,
                by: k % 3,
            })
            .collect();
        let mut b = Blob::new();
        save_edits(&mut b, 64, 64, &want);
        for cut in 0..b.v.len() {
            let mut s = Scan::new(&b.v[..cut]);
            let got = load_edits(&mut s, 64, 64, 64);
            assert!(
                !s.ok() || got == want,
                "обрезка на {cut} байте дала {} правок и не пожаловалась",
                got.len()
            );
        }
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
                        by: 17,
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
                    Edit::put(cx * 32 + ox, oy, cz * 32 + oz, r.below(4) as Mat)
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
                assert!(
                    guard < 10_000,
                    "очередь не сходится: {} осталось",
                    w.queued()
                );
            }
            // Хеш мира целиком: только он доказывает совпадение, и он же
            // дешевле снимка. Сложение заворачиванием — ассоциативно.
            let mut h = 0u64;
            for z in 0..dims.2 {
                for y in 0..dims.1 {
                    for x in 0..dims.0 {
                        let v = w.material(x, y, z) as u64;
                        h = h.wrapping_mul(0x100_0000_01b3)
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

    /// **Потолок правок считается за тик, а не за дренаж** (ADR-132,
    /// ADR-431). Дренажей в тике три; потолок внутри одного вызова
    /// пропускал втрое больше объявленного. Вне тика дренаж по-прежнему
    /// считает с нуля.
    #[test]
    fn the_edit_ceiling_holds_across_the_drains_of_a_tick() {
        let mut w = World::new(64, 32, 64, 1000, Box::new(Layers { h: 30 }));
        w.max_edits_per_tick = 50;
        for x in 1..63 {
            for z in 1..4 {
                w.queue_dig(x, 20, z);
            }
        }
        w.begin_tick();
        let applied: usize = (0..3).map(|_| w.flush().applied).sum();
        assert_eq!(applied, 50, "three drains of one tick applied {applied}");
        w.begin_tick();
        assert_eq!(w.flush().applied, 50, "the next tick starts afresh");
        // A world never driven by ticks: every drain counts on its own.
        let mut untimed = World::new(64, 32, 64, 1000, Box::new(Layers { h: 30 }));
        untimed.max_edits_per_tick = 50;
        for x in 1..63 {
            for z in 1..4 {
                untimed.queue_dig(x, 20, z);
            }
        }
        let applied: usize = (0..2).map(|_| untimed.flush().applied).sum();
        assert_eq!(applied, 100);
    }

    /// **Передачи между миром и телом идут первыми и целиком**
    /// (ADR-431): ни потолок правок, ни хвост приказов перед ними их не
    /// задерживают, и порядок между ними — порядок постановки. Очередь
    /// передач живёт в снимке своего владельца.
    #[test]
    fn transfers_drain_first_and_whole_behind_a_backlog() {
        let mut w = World::new(64, 32, 64, 1000, Box::new(Layers { h: 30 }));
        w.max_edits_per_tick = 10;
        for x in 1..63 {
            w.queue_dig(x, 20, 1);
        }
        for x in 1..31 {
            w.queue_transfer(Edit::dig(x, 25, 5));
        }
        w.queue_transfer(Edit::put(5, 25, 5, 7));
        let mut saved = fx::blob::Blob::new();
        w.save_transfers(&mut saved);
        w.begin_tick();
        let st = w.flush();
        assert_eq!(w.material(5, 25, 5), 7, "the later transfer ran first");
        assert!((1..31)
            .filter(|&x| x != 5)
            .all(|x| w.material(x, 25, 5) == AIR));
        assert_eq!(
            st.applied, 31,
            "transfers wait for nobody, orders for the ceiling"
        );
        assert!(w.solid(1, 20, 1), "an order got past the spent ceiling");
        let mut back = World::new(64, 32, 64, 1000, Box::new(Layers { h: 30 }));
        assert!(back.load_transfers(&mut fx::blob::Scan::new(&saved.v)));
        assert_eq!(back.queued(), 31);
        back.flush();
        assert_eq!(back.material(5, 25, 5), 7);
    }

    #[test]
    fn pending_digs_follow_duplicate_deferred_and_restored_queues() {
        let make = || World::new(64, 32, 64, 1000, Box::new(Layers { h: 30 }));
        let mut w = make();
        let p = [5, 10, 5];
        w.queue_edit(Edit::dig_if(p[0], p[1], p[2], 1));
        w.queue_edit(Edit::dig_if(p[0], p[1], p[2], 1));
        w.queue_transfer(Edit::dig(p[0], p[1], p[2]));
        w.queue_edit(Edit::put(6, 10, 5, 2));
        w.queue_dig(0, 10, 5);
        assert!(w.has_pending_dig(p[0], p[1], p[2]));
        assert!(!w.has_pending_dig(6, 10, 5));
        assert!(!w.has_pending_dig(0, 10, 5));

        let (mut saved, mut transfers) = (fx::blob::Blob::new(), fx::blob::Blob::new());
        w.save(&mut saved);
        w.save_transfers(&mut transfers);
        let mut back = make();
        assert!(back.load(&mut fx::blob::Scan::new(&saved.v)));
        assert!(back.load_transfers(&mut fx::blob::Scan::new(&transfers.v)));
        let mut same = fx::blob::Blob::new();
        back.save(&mut same);
        assert_eq!(
            same.v, saved.v,
            "the derived index changes no snapshot bytes"
        );
        for world in [&mut w, &mut back] {
            world.max_edits_per_tick = 1;
            world.begin_tick();
            world.flush(); // the transfer uses the applied allowance
            assert!(world.has_pending_dig(p[0], p[1], p[2]));
            world.begin_tick();
            world.flush(); // first duplicate is a no-op; one still waits
            assert!(world.has_pending_dig(p[0], p[1], p[2]));
            world.begin_tick();
            world.flush();
            assert!(!world.has_pending_dig(p[0], p[1], p[2]));
        }
        let mut deferred = make();
        deferred.max_materialize_per_tick = 0;
        deferred.queue_dig(8, 10, 8);
        assert!(deferred.flush().materialize_capped);
        assert!(deferred.has_pending_dig(8, 10, 8));
        deferred.max_materialize_per_tick = 4;
        deferred.flush();
        assert!(!deferred.has_pending_dig(8, 10, 8));
        deferred.queue_dig(9, 10, 9);
        deferred.reset_generator_regions(Box::new(Layers { h: 30 }), &[]);
        assert!(!deferred.has_pending_dig(9, 10, 9));
    }

    #[test]
    fn late_transfers_append_deltas_without_reopening_the_regular_allowance() {
        let mut w = World::new(64, 32, 64, 1, Box::new(Layers { h: 30 }));
        w.begin_tick();
        w.queue_dig(5, 10, 5);
        assert_eq!(w.flush().applied, 1);
        let first = w.completed()[0];
        w.queue_transfer(Edit::dig(6, 10, 5));
        w.queue_dig(7, 10, 5);
        assert_eq!(w.flush_transfers().applied, 1);
        assert_eq!(w.completed().len(), 2);
        assert_eq!(w.completed()[0], first);
        assert_eq!(w.edits().len(), 2);
        assert_eq!(w.taken().len(), 2);
        assert_eq!(w.transfers_queued(), 0);
        assert!(!w.has_pending_dig(6, 10, 5));
        assert!(w.has_pending_dig(7, 10, 5));
        assert_eq!(w.flush().applied, 0, "normal edits still have no allowance");
        assert!(w.solid(7, 10, 5));
        w.begin_tick();
        assert_eq!(w.flush().applied, 1);
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
                    *o = if y < h {
                        ((x ^ y ^ z) % 5 + 1) as Mat
                    } else {
                        AIR
                    };
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
            } else if (x / 4 + z / 4).is_multiple_of(5) && y > self.h / 2 {
                7
            } else {
                2
            }
        }
    }

    fn world() -> World {
        World::new(64, 48, 64, 1000, Box::new(Layers { h: 30 }))
    }

    #[test]
    fn pristine_reset_restores_only_edited_layers_and_exact_tops() {
        let mut w = World::new(64, 96, 64, 1000, Box::new(Layers { h: 70 }));
        for (x, y, z, mat) in [
            (5, 12, 5, AIR),
            (6, 40, 5, AIR),
            (7, 69, 5, AIR),
            (8, 84, 5, 3),
        ] {
            w.queue_edit(Edit {
                x,
                y,
                z,
                mat,
                by: NOBODY,
            });
        }
        while w.queued() != 0 {
            w.flush();
        }
        assert_eq!(w.column_ceiling(8, 5), 85);
        assert_eq!(w.touched_chunks(), 3);
        w.restore_pristine();
        let fresh = World::new(64, 96, 64, 1000, Box::new(Layers { h: 70 }));
        assert_eq!(w.bits(), fresh.bits());
        assert_eq!(w.touched_chunks(), 0);
        for z in 0..64 {
            for x in 0..64 {
                assert_eq!(w.column_ceiling(x, z), fresh.column_ceiling(x, z));
            }
        }
        for (x, y, z) in [(5, 12, 5), (6, 40, 5), (7, 69, 5), (8, 84, 5)] {
            assert_eq!(w.material(x, y, z), fresh.material(x, y, z));
        }
    }

    /// Потолок колонки обязан отвечать ровно то, что ответил бы обход
    /// от неба (ADR-313) — и после правок тоже. Правка вверх поднимает
    /// потолок, снятие с самого верха опускает его на следующий занятый
    /// воксель, а снятие изнутри толщи не трогает вовсе.
    #[test]
    fn the_column_ceiling_answers_as_a_walk_from_the_sky_would() {
        let walk = |w: &World, x: u32, z: u32| {
            (0..w.ny)
                .rev()
                .find(|&y| w.solid(x, y, z))
                .map_or(0, |y| y + 1)
        };
        let check = |w: &World, when: &str| {
            for z in 0..w.nz {
                for x in 0..w.nx {
                    assert_eq!(
                        w.column_ceiling(x, z),
                        walk(w, x, z),
                        "колонка {x},{z} {when}"
                    );
                }
            }
        };
        let mut w = world();
        check(&w, "у свежего мира");
        assert_eq!(w.column_ceiling(5, 5), 30, "слоистый генератор до 30");
        assert_eq!(w.column_ceiling(64, 0), 0, "за миром потолка нет");
        // Строится вверх: потолок обязан подняться.
        w.queue_edit(Edit {
            x: 5,
            y: 40,
            z: 5,
            mat: 2,
            by: NOBODY,
        });
        w.flush();
        assert_eq!(w.column_ceiling(5, 5), 41);
        check(&w, "после постройки");
        // Снимается верх: потолок обязан опуститься до толщи под ним.
        w.queue_dig(5, 40, 5);
        w.flush();
        assert_eq!(w.column_ceiling(5, 5), 30);
        // Снимается изнутри толщи: потолок стоит.
        w.queue_dig(5, 10, 5);
        w.flush();
        assert_eq!(w.column_ceiling(5, 5), 30);
        // Снимается верхний воксель толщи, один за другим.
        for y in (20..30).rev() {
            w.queue_dig(5, y, 5);
            w.flush();
            assert_eq!(w.column_ceiling(5, 5), y, "сняли {y}");
        }
        check(&w, "после копки");
        // Колонка выбирается до дна-рамки: у рамки потолок единица.
        for y in (1..20).rev() {
            w.queue_dig(5, y, 5);
        }
        w.flush();
        check(&w, "после выборки до рамки");
    }

    #[test]
    fn snapshot_restores_ceiling_after_raising_and_digging_columns() {
        let mut original = world();
        original.queue_edit(Edit::put(5, 40, 5, 3));
        original.queue_dig(6, 29, 5);
        original.queue_dig(6, 28, 5);
        original.queue_edit(Edit::put(38, 43, 38, 3));
        original.flush();
        let mut bytes = fx::blob::Blob::new();
        original.save(&mut bytes);
        let mut restored = world();
        assert!(restored.load(&mut fx::blob::Scan::new(&bytes.v)));
        for z in 0..original.nz {
            for x in 0..original.nx {
                assert_eq!(
                    original.column_ceiling(x, z),
                    restored.column_ceiling(x, z),
                    "column {x},{z}"
                );
            }
        }
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
        assert_eq!(
            w.touched_chunks(),
            0,
            "нетронутый мир не должен ничего хранить"
        );
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
        assert_eq!(w.edits(), &[Edit::dig(10, 5, 10)]);
        assert_eq!(w.touched_chunks(), 1, "тронут ровно один чанк");
    }

    /// Потолок соблюдается точно, а остаток остаётся в очереди —
    /// не теряется. Потолок защищает тик; потеря приказа сломала бы
    /// экономику.
    #[test]
    fn cap_is_exact_and_the_rest_stays_queued() {
        let mut w = World::new(64, 48, 64, 100, Box::new(Layers { h: 30 }));
        // От единицы: столбец `x = 0` и ряд `z = 0` — рамка мира,
        // и правка в них в очередь не попадает (ADR-034).
        for i in 0..250u32 {
            w.queue_dig(1 + i % 60, 5, 1 + i / 60);
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
        // Ряд `z = 1`, а не нулевой: нулевой — рамка мира (ADR-034).
        for x in 1..10u32 {
            w.queue_dig(x, 5, 1);
        }
        for round in 0..3 {
            w.flush();
            let got: Vec<u32> = w.edits().iter().map(|e| e.x).collect();
            assert_eq!(got, vec![round * 3 + 1, round * 3 + 2, round * 3 + 3]);
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

    /// **Рамка мира не копается** (ADR-034): дно и четыре стенки
    /// отбрасывают правку на постановке, а воксель рядом с рамкой
    /// снимается как любой другой.
    ///
    /// Проверяется по миру, а не по очереди: правка, попавшая
    /// в очередь и отброшенная дренажем, дала бы тот же мир, но
    /// заняла бы просмотр — и то, и другое видно только так.
    #[test]
    fn the_frame_does_not_dig() {
        let mut w = world();
        let (nx, nz) = (w.nx, w.nz);
        let frame = [
            (5, 0, 5),
            (0, 5, 5),
            (5, 5, 0),
            (nx - 1, 5, 5),
            (5, 5, nz - 1),
        ];
        for &(x, y, z) in &frame {
            assert!(w.on_frame(x, y, z));
            assert!(w.solid(x, y, z), "сцена: ({x}, {y}, {z}) не порода");
            w.queue_dig(x, y, z);
        }
        assert_eq!(w.queued(), 0, "правки в рамку попали в очередь");
        w.flush();
        for &(x, y, z) in &frame {
            assert!(w.solid(x, y, z), "рамка снята в ({x}, {y}, {z})");
        }
        // Соседний с рамкой воксель — обычный.
        let inner = [
            (5, 1, 5),
            (1, 5, 5),
            (5, 5, 1),
            (nx - 2, 5, 5),
            (5, 5, nz - 2),
        ];
        for &(x, y, z) in &inner {
            assert!(!w.on_frame(x, y, z));
            w.queue_dig(x, y, z);
        }
        w.flush();
        for &(x, y, z) in &inner {
            assert!(
                !w.solid(x, y, z),
                "воксель у рамки не снялся в ({x}, {y}, {z})"
            );
        }
    }
}

#[cfg(test)]
mod fill_tests {
    use super::*;

    /// Рельеф с пещерами: поверхность ступенями, под ней дыры. Пещеры
    /// нужны, чтобы объём не был функцией одной высоты — иначе тест
    /// не отличил бы колонку от повоксельного обхода.
    struct Holey;

    impl Generator for Holey {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            let h = 20 + (x / 5) % 7 + (z / 3) % 5;
            if y >= h || (x * 7 + y * 13 + z * 3).is_multiple_of(11) {
                return AIR;
            }
            (1 + (x + y + z) % 3) as Mat
        }
    }

    /// Повоксельный обход — эталон: так объём строился до ADR-228.
    fn reference(nx: u32, ny: u32, nz: u32, g: &dyn Generator) -> Vec<u64> {
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
        bits
    }

    /// **Колоночная сборка совпадает с повоксельной до бита, при любом
    /// числе потоков.** Размеры нарочно не кратны ни слову, ни полосе:
    /// 150 по `x` — два слова с хвостом в 22 бита, 37 по `z` — полосы
    /// неравной ширины, и больше потоков, чем полос, тоже бывает.
    #[test]
    fn the_column_fill_matches_the_voxel_walk_on_any_thread_count() {
        let (nx, ny, nz) = (150u32, 40u32, 37u32);
        let want = reference(nx, ny, nz, &Holey);
        assert!(
            want.iter().any(|&w| w != 0) && want.iter().any(|&w| w != u64::MAX),
            "эталон однороден — сверять нечего"
        );
        for threads in [1usize, 2, 3, 8, 64] {
            let got = fill_bits(nx, ny, nz, nx.div_ceil(64), &Holey, threads);
            assert_eq!(
                got, want,
                "потоков {threads}: объём разошёлся с повоксельным"
            );
        }
    }

    /// **Мир из готового объёма отвечает так же, как построенный.**
    /// Это и есть контракт кэша карты (ADR-228).
    #[test]
    fn a_world_from_bits_answers_like_a_generated_one() {
        let (nx, ny, nz) = (70u32, 20u32, 33u32);
        let a = World::new(nx, ny, nz, 16, Box::new(Holey));
        let b = World::from_bits(nx, ny, nz, 16, Box::new(Holey), a.bits().to_vec())
            .expect("длина объёма верна");
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    assert_eq!(a.solid(x, y, z), b.solid(x, y, z), "воксел {x} {y} {z}");
                }
            }
        }
        assert!(
            World::from_bits(nx, ny, nz, 16, Box::new(Holey), vec![0; 5]).is_none(),
            "объём не той длины принят"
        );
    }

    /// Inclusive volume queries agree with independent per-voxel reads.
    #[test]
    fn any_solid_box_matches_per_voxel_queries_at_word_and_world_edges() {
        struct Sparse;
        impl Generator for Sparse {
            fn material(&self, x: u32, y: u32, z: u32) -> Mat {
                if (x.is_multiple_of(31) && y == 2 && z == 3)
                    || (x == 128 && y == 4 && z == 6)
                    || (x == 149 && y == 0 && z == 0)
                {
                    1
                } else {
                    AIR
                }
            }
        }
        for nx in [1, 63, 64, 65, 129, 150] {
            let w = World::new(nx, 5, 7, 64, Box::new(Sparse));
            let reference = |lo: [u32; 3], hi: [u32; 3]| {
                (lo[1]..=hi[1].min(w.ny - 1)).any(|y| {
                    (lo[2]..=hi[2].min(w.nz - 1))
                        .any(|z| (lo[0]..=hi[0].min(w.nx - 1)).any(|x| w.solid(x, y, z)))
                })
            };
            // The sole occupied voxel lies in the final partial word or
            // on the final Y/Z row; an early head hit cannot hide a bad tail.
            for (lo, hi) in [
                ([0, 4, 6], [149, 4, 6]),
                ([125, 4, 6], [128, 4, 6]),
                ([129, 0, 0], [149, 0, 0]),
            ] {
                assert_eq!(w.any_solid_box(lo, hi), reference(lo, hi));
            }
            let mut rng = fx::rng::Rng::new(u64::from(nx), 335);
            for _ in 0..2000 {
                let size = [nx, 5, 7];
                let lo = size.map(|n| rng.below(n + 3));
                let hi = [0, 1, 2].map(|a| {
                    if rng.below(5) == 0 {
                        rng.below(size[a] + 3)
                    } else {
                        lo[a] + rng.below(160)
                    }
                });
                assert_eq!(
                    w.any_solid_box(lo, hi),
                    reference(lo, hi),
                    "nx={nx} {lo:?}..{hi:?}"
                );
            }
            for x in [0, 1, 31, 62, 63, 64, 65, 127, 128, 149, nx, u32::MAX] {
                for n in [0, 1, 2, 63, 64, 65, 129, u32::MAX] {
                    let lo = [x, 0, 0];
                    let hi = [x.saturating_add(n), u32::MAX, u32::MAX];
                    assert_eq!(w.any_solid_box(lo, hi), reference(lo, hi));
                }
            }
        }
    }

    #[test]
    fn any_solid_box_reads_same_tick_additions_and_removals() {
        struct Empty;
        impl Generator for Empty {
            fn material(&self, _: u32, _: u32, _: u32) -> Mat {
                AIR
            }
        }
        let mut w = World::new(150, 5, 7, 64, Box::new(Empty));
        let lo = [62, 1, 2];
        let hi = [129, 3, 4];
        assert!(!w.any_solid_box(lo, hi));
        w.begin_tick();
        for x in [63, 64, 128] {
            w.queue_edit(Edit::put(x, 2, 3, 1));
        }
        w.flush();
        assert!(w.any_solid_box(lo, hi));
        for x in [63, 64, 128] {
            w.queue_dig(x, 2, 3);
        }
        w.flush();
        assert!(!w.any_solid_box(lo, hi));
    }

    /// **Слово занятости совпадает с побитовым опросом** — на стыке
    /// слов, у края мира и при укороченном запросе.
    #[test]
    fn solid_bits_agree_with_solid() {
        let (nx, ny, nz) = (150u32, 30u32, 20u32);
        let w = World::new(nx, ny, nz, 16, Box::new(Holey));
        for z in 0..nz {
            for y in 0..ny {
                for x in (0..nx).step_by(7).chain([63, 64, 127, 128, 149]) {
                    for n in [1u32, 5, 10, 63, 64, 200] {
                        let v = w.solid_bits(x, y, z, n);
                        for i in 0..64u32 {
                            let want = i < n && w.solid(x + i, y, z);
                            assert_eq!((v >> i) & 1 != 0, want, "x {x} y {y} z {z} n {n} бит {i}");
                        }
                    }
                }
            }
        }
        assert_eq!(w.solid_bits(nx, 0, 0, 8), 0);
        assert_eq!(w.solid_bits(0, ny, 0, 8), 0);
        assert_eq!(w.solid_bits(0, 0, nz, 8), 0);
        assert_eq!(w.solid_bits(0, 0, 0, 0), 0);
    }
}
