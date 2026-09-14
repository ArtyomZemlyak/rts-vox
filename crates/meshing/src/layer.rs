//! Мост от мира к подсистеме: грязное множество и ленивое состояние.
//!
//! Размеры чанков у подсистем **не совпадают, и это не недосмотр**:
//! мешинг режет мир на 62³ с паддингом соседей, связность — на 32³,
//! навигация — на клетки по два воксела. Это разные подсистемы
//! с разными единицами, и каждая считает своё грязное множество.
//!
//! **Слой ленивый.** Нетронутый чанк не стоит ничего, ровно как
//! в самом мире.

use crate::bits::BitVolume;
use crate::incr::IncMesher;
use crate::{Opts, CS as MCS, CSP};
use voxel::{Edit, World};

/// Слой мешинга: битовый объём чанка плюс его инкрементальный мешер.
struct MeshChunk {
    bits: BitVolume,
    mesher: IncMesher,
}

/// Чем чанк оказался на осмотре.
///
/// Три случая, а не два, и третий — самый ценный. **Ниже первого
/// сплошного чанка колонки смотреть не на что**: всё, что там есть,
/// закрыто породой сверху. Это и есть дно потоковой подгрузки, и оно
/// само едет вниз, когда колонку прокапывают: пробитый чанк перестаёт
/// быть сплошным.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// Чистый воздух: граней нет.
    Air,
    /// Сплошная порода: граней нет, и ниже по колонке их не видно.
    Rock,
    /// Поверхность, пещера, яма — есть что мешить.
    Mixed,
}

/// Собрать битовый объём чанка **словами**, а не по вокселю.
///
/// Занятость мира уже лежит битами, и по оси X они идут подряд
/// (`World::bits`). Колонка чанка — это те же биты, сдвинутые
/// на невыровненное начало: чанк 62 воксела шириной, слово 64 бита,
/// и совпадают они только у нулевого. Побитовый обход спрашивает
/// занятость 262 144 раза на чанк; здесь это 4 096 сдвигов.
///
/// Возвращает, **чем чанк оказался**: воздух, сплошная порода или
/// поверхность. У первых двух нет ни одной грани, и весь смысл проверки
/// в том, что узнаётся это по дороге и бесплатно.
///
/// Край мира — воздух, как и у побитового обхода: грань на краю
/// остаётся видимой, и сплошным такой чанк не считается.
fn fill_bits(w: &World, cx: u32, cy: u32, cz: u32, b: &mut BitVolume) -> Fill {
    let (ox, oy, oz) = (cx * MCS as u32, cy * MCS as u32, cz * MCS as u32);
    let src = w.bits();
    let wpr = w.bit_words_per_row() as i64;
    let (nx, ny, nz) = (w.nx as i64, w.ny as i64, w.nz as i64);
    // Локальный ноль — паддинг, то есть воксел **перед** чанком.
    let x0 = ox as i64 - 1;
    let wi = x0.div_euclid(64);
    let sh = x0.rem_euclid(64) as u32;
    // Биты за краем мира гасятся маской: `solid` за краем отвечает
    // «пусто», и словесная сборка обязана отвечать так же.
    let mask = if x0 + 64 <= nx {
        u64::MAX
    } else if x0 >= nx {
        0
    } else {
        let k = (nx - x0) as u32;
        (1u64 << k) - 1
    };
    let (mut all_set, mut all_clear) = (true, true);
    for ly in 0..CSP {
        let wy = oy as i64 + ly as i64 - 1;
        let row_ok = wy >= 0 && wy < ny;
        for lz in 0..CSP {
            let wz = oz as i64 + lz as i64 - 1;
            let word = if !row_ok || wz < 0 || wz >= nz {
                0
            } else {
                let base = (wy * nz + wz) * wpr;
                let lo = if wi < 0 { 0 } else { src[(base + wi) as usize] };
                let hi = if wi + 1 >= wpr {
                    0
                } else {
                    src[(base + wi + 1) as usize]
                };
                let v = if sh == 0 {
                    lo
                } else {
                    (lo >> sh) | (hi << (64 - sh))
                };
                v & mask
            };
            b[ly * CSP + lz] = word;
            all_set &= word == u64::MAX;
            all_clear &= word == 0;
        }
    }
    match (all_set, all_clear) {
        // Пустой чанк однороден по обоим счётам: считаем его воздухом,
        // потому что дном колонки он не является.
        (_, true) => Fill::Air,
        (true, _) => Fill::Rock,
        _ => Fill::Mixed,
    }
}

pub struct MeshLayer {
    chunks: Vec<Option<Box<MeshChunk>>>,
    nx: u32,
    ny: u32,
    nz: u32,
    opts: Opts,
    /// Грязные чанки текущего тика; переиспользуется, чтобы не выделять.
    dirty: Vec<u32>,
    stamp: Vec<u32>,
    epoch: u32,
    /// Правки, сгруппированные по чанку, в локальных координатах.
    local: Vec<Vec<(u8, u8, u8)>>,
    /// Чанки, состояние которых слой держит.
    ///
    /// Отдельным множеством, а не обходом `chunks`: на карте
    /// в километр в сетке два миллиона чанков, а живых — тысячи,
    /// и «сколько занято памяти» не должно стоить линейного прохода
    /// по всей карте. `BTreeSet`, а не `HashSet`: порядок обхода
    /// стабилен, и два прогона одного замера дают одно число.
    live: std::collections::BTreeSet<u32>,
    /// Сколько раз чанк собирался с нуля. Первое касание чанка стоит
    /// полной пересборки, и на старте это даёт пик — см. D-7.
    pub full_rebuilds: u64,
    /// Битовый объём и мешер **на один раз**: потоковый слой строит
    /// чанк, отдаёт квады и тут же забывает его. Заводить под это
    /// `IncMesher` заново значило бы выделять 726 КиБ на каждый
    /// построенный чанк (ADR-075) и возвращать их аллокатору
    /// через сто микросекунд.
    scratch: Box<Scratch>,
}

/// Переиспользуемое место под разовую сборку чанка.
struct Scratch {
    bits: BitVolume,
    mesher: IncMesher,
    quads: Vec<crate::Quad>,
}

impl MeshLayer {
    pub fn new(w: &World, opts: Opts) -> MeshLayer {
        let (nx, ny, nz) = (
            w.nx.div_ceil(MCS as u32),
            w.ny.div_ceil(MCS as u32),
            w.nz.div_ceil(MCS as u32),
        );
        let n = (nx * ny * nz) as usize;
        MeshLayer {
            chunks: (0..n).map(|_| None).collect(),
            nx,
            ny,
            nz,
            opts,
            dirty: Vec::new(),
            stamp: vec![u32::MAX; n],
            epoch: 0,
            local: (0..n).map(|_| Vec::new()).collect(),
            live: std::collections::BTreeSet::new(),
            full_rebuilds: 0,
            scratch: Box::new(Scratch {
                bits: [0u64; CSP * CSP],
                mesher: IncMesher::new(),
                quads: Vec::new(),
            }),
        }
    }

    #[inline]
    fn index(&self, cx: u32, cy: u32, cz: u32) -> usize {
        ((cz * self.ny + cy) * self.nx + cx) as usize
    }

    /// Координаты чанка по линейному индексу. Порядок растровый:
    /// `x` быстрее всех, `z` медленнее — тот же, что у `ChunkGrid`
    /// рендера, и разойтись им нельзя.
    #[inline]
    fn coords(&self, i: usize) -> (u32, u32, u32) {
        let i = i as u32;
        (
            i % self.nx,
            (i / self.nx) % self.ny,
            i / (self.nx * self.ny),
        )
    }

    /// Собрать битовый объём чанка из мира, включая паддинг соседей.
    fn build_bits(w: &World, cx: u32, cy: u32, cz: u32) -> BitVolume {
        let mut b: BitVolume = [0u64; CSP * CSP];
        let (ox, oy, oz) = (cx * MCS as u32, cy * MCS as u32, cz * MCS as u32);
        // Локальные 0 и 63 — паддинг: воксели соседнего чанка.
        for ly in 0..CSP {
            for lz in 0..CSP {
                let (wy, wz) = (oy as i64 + ly as i64 - 1, oz as i64 + lz as i64 - 1);
                if wy < 0 || wz < 0 {
                    continue;
                }
                let mut word = 0u64;
                for lx in 0..CSP {
                    let wx = ox as i64 + lx as i64 - 1;
                    if wx < 0 {
                        continue;
                    }
                    if w.solid(wx as u32, wy as u32, wz as u32) {
                        word |= 1u64 << lx;
                    }
                }
                b[ly * CSP + lz] = word;
            }
        }
        b
    }

    /// Осмотреть чанк, не собирая его.
    ///
    /// Отвечает, **чем чанк оказался**. Однородный объём — сплошная
    /// порода или чистый воздух — не даёт ни одного квада, и мешить
    /// его незачем. В потоковом кольце таких большинство: под
    /// поверхностью сплошная порода, над ней воздух, и только тонкий
    /// пласт у поверхности несёт геометрию.
    ///
    /// [`Fill::Rock`] отличается от [`Fill::Air`] тем, что он **дно**:
    /// ниже сплошного чанка в колонке смотреть не на что, всё там
    /// закрыто породой сверху.
    ///
    /// Цена — 4 096 сдвигов вместо 262 144 запросов занятости,
    /// и это разница между «осмотреть кольцо за кадр» и «за минуту».
    pub fn probe(&mut self, w: &World, i: usize) -> Fill {
        let (cx, cy, cz) = self.coords(i);
        fill_bits(w, cx, cy, cz, &mut self.scratch.bits)
    }

    /// Квады чанка, собранные с нуля в **переиспользуемом** мешере.
    ///
    /// Путь потоковой подгрузки: чанк входит в кольцо, отдаёт геометрию
    /// и состояния за собой не оставляет. Резидентный `IncMesher`
    /// стоит 726 КиБ на чанк (ADR-075) и нужен тому, кого копают,
    /// а не тому, на кого смотрят.
    ///
    /// Однородный чанк возвращает пустой срез, не дойдя до мешера.
    pub fn scratch_quads(&mut self, w: &World, i: usize) -> &[crate::Quad] {
        let (cx, cy, cz) = self.coords(i);
        let opts = self.opts;
        let s = &mut *self.scratch;
        s.quads.clear();
        if fill_bits(w, cx, cy, cz, &mut s.bits) != Fill::Mixed {
            return &s.quads;
        }
        s.mesher.rebuild_all(&s.bits, opts);
        s.mesher.append_quads(&mut s.quads);
        &s.quads
    }

    /// Забыть состояние чанка. Возвращает `true`, если было что забывать.
    ///
    /// Кольцо вокруг камеры конечно, а карта — нет: чанк, вышедший
    /// из кольца, обязан вернуть свои 726 КиБ. Правильность от этого
    /// не страдает: мир авторитетен, и вернувшийся чанк собирается
    /// из него заново.
    pub fn forget(&mut self, i: usize) -> bool {
        self.live.remove(&(i as u32));
        self.chunks[i].take().is_some()
    }

    /// Держит ли слой состояние этого чанка.
    pub fn is_live(&self, i: usize) -> bool {
        self.chunks[i].is_some()
    }

    /// Развести правки тика по чанкам мешинга. Правка на границе чанка
    /// попадает **и в соседний**: там она меняет паддинг, а значит
    /// и швы. Пропустить это — получить лишние квады на границах,
    /// и заметно это не сразу.
    fn scatter(&mut self, edits: &[Edit]) {
        self.epoch += 1;
        self.dirty.clear();
        for e in edits {
            let (cx, cy, cz) = (e.x / MCS as u32, e.y / MCS as u32, e.z / MCS as u32);
            let (lx, ly, lz) = (
                e.x % MCS as u32 + 1,
                e.y % MCS as u32 + 1,
                e.z % MCS as u32 + 1,
            );
            self.touch(cx, cy, cz, lx as u8, ly as u8, lz as u8);
            // Соседи по каждой оси, если правка легла на первый
            // или последний полезный воксель.
            if lx == 1 && cx > 0 {
                self.touch(cx - 1, cy, cz, (MCS + 1) as u8, ly as u8, lz as u8);
            }
            if lx == MCS as u32 && cx + 1 < self.nx {
                self.touch(cx + 1, cy, cz, 0, ly as u8, lz as u8);
            }
            if ly == 1 && cy > 0 {
                self.touch(cx, cy - 1, cz, lx as u8, (MCS + 1) as u8, lz as u8);
            }
            if ly == MCS as u32 && cy + 1 < self.ny {
                self.touch(cx, cy + 1, cz, lx as u8, 0, lz as u8);
            }
            if lz == 1 && cz > 0 {
                self.touch(cx, cy, cz - 1, lx as u8, ly as u8, (MCS + 1) as u8);
            }
            if lz == MCS as u32 && cz + 1 < self.nz {
                self.touch(cx, cy, cz + 1, lx as u8, ly as u8, 0);
            }
        }
    }

    fn touch(&mut self, cx: u32, cy: u32, cz: u32, lx: u8, ly: u8, lz: u8) {
        if cx >= self.nx || cy >= self.ny || cz >= self.nz {
            return;
        }
        let i = self.index(cx, cy, cz);
        if self.stamp[i] != self.epoch {
            self.stamp[i] = self.epoch;
            self.local[i].clear();
            self.dirty.push(i as u32);
        }
        self.local[i].push((lx, ly, lz));
    }

    /// Чанки, задетые последним `apply`. Рендеру нужно знать **какие**,
    /// а не сколько: пересобирать весь мир каждый кадр — это два порядка
    /// лишней работы (ADR-155).
    pub fn dirty(&self) -> &[u32] {
        &self.dirty
    }

    /// Применить правки тика. Возвращает число грязных чанков.
    ///
    /// Нерезидентный грязный чанк строится с нуля и остаётся жить.
    /// Если строить надо не всё — [`MeshLayer::apply_where`].
    pub fn apply(&mut self, w: &World, edits: &[Edit]) -> usize {
        self.apply_where(w, edits, |_| true)
    }

    /// То же, но нерезидентный грязный чанк **строится, только если
    /// `keep` разрешает**; иначе он остаётся в списке грязных
    /// и не стоит ничего.
    ///
    /// Копают и за кольцом (ADR-228): карта в километр, а окно держит
    /// сто метров. Без этого каждый задетый за кольцом чанк собирался
    /// бы с нуля — объём, мешер на 726 КиБ, полная пересборка —
    /// и тут же забывался, и так каждый тик заново: на километре
    /// с двумя дюжинами копателей это три десятка сборок за кадр
    /// впустую.
    pub fn apply_where(
        &mut self,
        w: &World,
        edits: &[Edit],
        keep: impl Fn(usize) -> bool,
    ) -> usize {
        self.scatter(edits);
        for k in 0..self.dirty.len() {
            let i = self.dirty[k] as usize;
            let (cx, cy, cz) = (
                (i as u32) % self.nx,
                ((i as u32) / self.nx) % self.ny,
                (i as u32) / (self.nx * self.ny),
            );
            if self.chunks[i].is_none() {
                if !keep(i) {
                    continue;
                }
                // Первое касание: собрать объём и смешить целиком.
                let mut bits: BitVolume = [0u64; CSP * CSP];
                fill_bits(w, cx, cy, cz, &mut bits);
                let mut mesher = IncMesher::new();
                mesher.rebuild_all(&bits, self.opts);
                self.chunks[i] = Some(Box::new(MeshChunk { bits, mesher }));
                self.live.insert(i as u32);
                self.full_rebuilds += 1;
                continue;
            }
            let c = self.chunks[i].as_mut().expect("чанк только что проверен");
            // Битовый объём обновляется правкой, а не пересборкой (ADR-061).
            for &(lx, ly, lz) in &self.local[i] {
                let solid = w.solid(
                    cx * MCS as u32 + lx as u32 - 1,
                    cy * MCS as u32 + ly as u32 - 1,
                    cz * MCS as u32 + lz as u32 - 1,
                );
                let word = &mut c.bits[ly as usize * CSP + lz as usize];
                if solid {
                    *word |= 1u64 << lx;
                } else {
                    *word &= !(1u64 << lx);
                }
            }
            let bits = c.bits;
            c.mesher.apply_edits(&bits, &self.local[i], self.opts);
        }
        self.dirty.len()
    }

    pub fn quad_total(&self) -> usize {
        self.live
            .iter()
            .filter_map(|&i| self.chunks[i as usize].as_ref())
            .map(|c| c.mesher.quad_count())
            .sum()
    }

    pub fn live_chunks(&self) -> usize {
        self.live.len()
    }

    /// Сколько памяти держит слой. Обход идёт по живым чанкам, а не
    /// по сетке: на километре в ней два миллиона мест, и вопрос
    /// «сколько занято» не должен стоить прохода по всей карте.
    pub fn state_bytes(&self) -> usize {
        self.live
            .iter()
            .filter_map(|&i| self.chunks[i as usize].as_ref())
            .map(|c| c.mesher.state_bytes() + core::mem::size_of::<BitVolume>())
            .sum()
    }

    /// Пересобрать чанк с нуля — эталон для проверки швов.
    pub fn rebuilt_quad_count(&self, w: &World, i: usize) -> usize {
        let (cx, cy, cz) = (
            (i as u32) % self.nx,
            ((i as u32) / self.nx) % self.ny,
            (i as u32) / (self.nx * self.ny),
        );
        let bits = Self::build_bits(w, cx, cy, cz);
        let mut m = IncMesher::new();
        m.rebuild_all(&bits, self.opts);
        m.quad_count()
    }

    /// Квады чанка, собранные с нуля.
    ///
    /// Тот же путь, что у сверки швов, только результат — сама
    /// геометрия: рендеру нужны квады, а не их число.
    pub fn rebuilt_quads(&self, w: &World, i: usize) -> Vec<crate::Quad> {
        let (cx, cy, cz) = (
            (i as u32) % self.nx,
            ((i as u32) / self.nx) % self.ny,
            (i as u32) / (self.nx * self.ny),
        );
        let bits = Self::build_bits(w, cx, cy, cz);
        let mut m = IncMesher::new();
        m.rebuild_all(&bits, self.opts);
        m.collect_quads()
    }

    /// Размеры сетки чанков.
    pub fn dims(&self) -> (u32, u32, u32) {
        (self.nx, self.ny, self.nz)
    }

    /// Квады чанка **как они есть** — из его живого мешера.
    ///
    /// Это то, ради чего инкрементальный ремеш и написан. Потребитель,
    /// зовущий вместо этого `rebuilt_quads`, выбрасывает весь выигрыш:
    /// полная пересборка заново строит битовый объём (64³ обращений
    /// к миру) и сливает все триста семьдесят две пары «грань, слой»,
    /// тогда как правка трогает около двенадцати (ADR-075).
    ///
    /// Пустой вектор у нетронутого чанка: слой ленивый, и чанк,
    /// которого никто не касался, мешера ещё не имеет.
    pub fn quads(&self, i: usize) -> Vec<crate::Quad> {
        self.chunks[i]
            .as_ref()
            .map(|c| c.mesher.collect_quads())
            .unwrap_or_default()
    }

    /// Число квадов конкретного чанка — для сверки швов.
    pub fn quad_at(&self, i: usize) -> usize {
        self.chunks[i].as_ref().map_or(0, |c| c.mesher.quad_count())
    }

    pub fn live_indices(&self) -> Vec<usize> {
        self.live.iter().map(|&i| i as usize).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Quad;

    /// Ступенчатый рельеф: слой соблюдает ленивость, поэтому чанк
    /// обязан быть тронут правкой, чтобы вообще появиться.
    struct Steps;

    impl voxel::Generator for Steps {
        fn material(&self, x: u32, y: u32, z: u32) -> voxel::store::Mat {
            let h = 20 + x % 5 + (z % 3) * 3;
            if y >= h {
                voxel::store::AIR
            } else if y + 3 >= h {
                1
            } else {
                2
            }
        }
    }

    /// Тот же рельеф, но поверхность выше чанка: под ней сплошная
    /// порода, над ней чистый воздух — оба однородных случая разом.
    struct Deep;

    impl voxel::Generator for Deep {
        fn material(&self, x: u32, y: u32, z: u32) -> voxel::store::Mat {
            let h = 200 + x % 7 + (z % 5) * 2;
            if y >= h {
                voxel::store::AIR
            } else if y + 3 >= h {
                1
            } else {
                2
            }
        }
    }

    fn sorted(mut q: Vec<Quad>) -> Vec<Quad> {
        q.sort_by_key(|x| (x.face, x.slice, x.a, x.b, x.w, x.h));
        q
    }

    /// Прокопать список и вернуть **применённые** правки: слой обязан
    /// получить ровно то, что легло в мир, а не то, что было заказано.
    fn dig(w: &mut voxel::World, cells: impl Iterator<Item = (u32, u32, u32)>) -> Vec<Edit> {
        for (x, y, z) in cells {
            w.queue_dig(x, y, z);
        }
        let mut applied = Vec::new();
        while w.queued() > 0 {
            w.flush();
            applied.extend_from_slice(w.edits());
        }
        applied
    }

    /// **Живые квады чанка — те же, что у полной пересборки.**
    ///
    /// Ровно это свойство даёт право не пересобирать чанк с нуля,
    /// и без него `quads` — просто более быстрый неверный ответ.
    /// Число квадов уже сверяется в сценах, но геометрию рендер берёт
    /// не числом, а самими квадами.
    #[test]
    fn live_quads_equal_a_full_rebuild() {
        let mut w = voxel::World::new(128, 64, 128, 1 << 16, Box::new(Steps));
        let mut layer = MeshLayer::new(&w, Opts::default());
        let edits = dig(
            &mut w,
            (14..24u32)
                .flat_map(|y| (30..40u32).flat_map(move |z| (30..40u32).map(move |x| (x, y, z)))),
        );
        assert!(!edits.is_empty(), "яма не выкопалась");
        assert_eq!(layer.apply(&w, &edits), 1, "яма задела не один чанк");
        let i = layer.dirty()[0] as usize;
        assert!(layer.quad_at(i) > 0, "чанк пуст — сверять нечего");
        assert_eq!(
            sorted(layer.quads(i)),
            sorted(layer.rebuilt_quads(&w, i)),
            "живые квады разошлись с полной пересборкой"
        );

        // Вторая правка поверх первой: расхождение инкремента копится,
        // и одна правка его не ловит.
        let more = dig(
            &mut w,
            (16..20u32).flat_map(|y| (32..36u32).map(move |x| (x, y, 45u32))),
        );
        layer.apply(&w, &more);
        for &d in layer.dirty() {
            let d = d as usize;
            assert_eq!(
                sorted(layer.quads(d)),
                sorted(layer.rebuilt_quads(&w, d)),
                "чанк {d}: живые квады разошлись после второй правки"
            );
        }
    }

    /// **Словесная сборка объёма совпадает с побитовой — до бита.**
    ///
    /// Это не оптимизация «примерно то же»: чанк 62 воксела шириной,
    /// слово 64 бита, и выравниваются они только у нулевого чанка.
    /// Сдвиг на единицу даёт геометрию, съехавшую на воксел, и заметно
    /// это не на числе квадов, а на шве между чанками.
    ///
    /// Мир берётся **с правками**: край мира, пустые чанки и дыры
    /// в породе — три разных случая границы, и путать их легко.
    #[test]
    fn the_word_fill_matches_the_voxel_walk() {
        // Мир нарочно шире шести чанков по каждой оси: чанк 62 воксела,
        // слово 64 бита, и сдвиг колонки у каждого следующего чанка
        // свой — 63, 61, 59, 57, 55, 53. Проверять надо их все, а не
        // выровненный нулевой.
        let mut w = voxel::World::new(320, 192, 320, 1 << 16, Box::new(Steps));
        dig(
            &mut w,
            (14..26u32)
                .flat_map(|y| (55..70u32).flat_map(move |z| (55..70u32).map(move |x| (x, y, z)))),
        );
        let layer = MeshLayer::new(&w, Opts::default());
        let (nx, ny, nz) = layer.dims();
        let mut checked = 0usize;
        let (mut uniform, mut mixed) = (0usize, 0usize);
        for cz in 0..nz {
            for cy in 0..ny {
                for cx in 0..nx {
                    let want = MeshLayer::build_bits(&w, cx, cy, cz);
                    let mut got: BitVolume = [0u64; CSP * CSP];
                    let u = fill_bits(&w, cx, cy, cz, &mut got);
                    assert_eq!(
                        want.as_slice(),
                        got.as_slice(),
                        "чанк {cx},{cy},{cz}: словесный объём разошёлся с побитовым"
                    );
                    let flat = if want.iter().all(|&v| v == 0) {
                        Fill::Air
                    } else if want.iter().all(|&v| v == u64::MAX) {
                        Fill::Rock
                    } else {
                        Fill::Mixed
                    };
                    assert_eq!(u, flat, "чанк {cx},{cy},{cz}: разбор объёма неверен");
                    checked += 1;
                    if u == Fill::Mixed {
                        mixed += 1;
                    } else {
                        uniform += 1;
                    }
                }
            }
        }
        assert!(checked > 100, "проверка ничего не проверила");
        // Оба случая обязаны встретиться: без однородных нечего было бы
        // пропускать, без смешанных — нечего сверять.
        assert!(
            uniform > 0 && mixed > 0,
            "однородных {uniform}, смешанных {mixed}"
        );
    }

    /// **Однородный чанк граней не даёт, смешанный даёт.** Ровно на этом
    /// стоит потолок работы кольца: осмотр отделяет породу и воздух
    /// от поверхности, не доходя до мешера.
    /// Порода при этом отличается от воздуха: ниже сплошного чанка
    /// колонку можно не смотреть вовсе, а ниже пустого — нельзя, там
    /// ещё будет поверхность.
    #[test]
    fn a_uniform_chunk_has_no_quads() {
        // Рельеф глубокий нарочно: при поверхности в двадцати вокселях
        // сплошного чанка в мире нет вовсе, и различать было бы нечего.
        let w = voxel::World::new(200, 256, 200, 1 << 16, Box::new(Deep));
        let mut layer = MeshLayer::new(&w, Opts::default());
        let (nx, ny, nz) = layer.dims();
        // Не нулевой слой: под ним край мира, то есть воздух, и грань
        // на нём видна — сплошным такой чанк не считается.
        assert_eq!(
            layer.probe(&w, layer.index(1, 1, 1)),
            Fill::Rock,
            "глубина не порода"
        );
        assert_eq!(
            layer.probe(&w, layer.index(1, ny - 1, 1)),
            Fill::Air,
            "верх мира не воздух"
        );
        let (mut skipped, mut built) = (0usize, 0usize);
        for i in 0..(nx * ny * nz) as usize {
            let has = layer.probe(&w, i);
            let n = layer.scratch_quads(&w, i).len();
            assert_eq!(
                has == Fill::Mixed,
                n > 0,
                "чанк {i}: осмотр сказал {has:?}, а квадов {n}"
            );
            // Разовая сборка состояния за собой не оставляет.
            assert!(!layer.is_live(i), "чанк {i}: разовая сборка осела в слое");
            if has == Fill::Mixed {
                built += 1;
            } else {
                skipped += 1;
            }
        }
        assert!(
            skipped > 0 && built > 0,
            "пропущено {skipped}, собрано {built}"
        );
    }

    /// **Разовая сборка даёт ту же геометрию, что полная пересборка.**
    /// Кольцо показывает именно её, и разойдись они — на экране будет
    /// не мир, а его пересказ.
    #[test]
    fn the_scratch_build_equals_a_full_rebuild() {
        let mut w = voxel::World::new(200, 128, 200, 1 << 16, Box::new(Steps));
        dig(
            &mut w,
            (14..26u32)
                .flat_map(|y| (55..70u32).flat_map(move |z| (55..70u32).map(move |x| (x, y, z)))),
        );
        let mut layer = MeshLayer::new(&w, Opts::default());
        let (nx, ny, nz) = layer.dims();
        let mut seen = 0usize;
        for i in 0..(nx * ny * nz) as usize {
            let want = sorted(layer.rebuilt_quads(&w, i));
            let got = sorted(layer.scratch_quads(&w, i).to_vec());
            assert_eq!(want, got, "чанк {i}: разовая сборка разошлась с полной");
            seen += usize::from(!want.is_empty());
        }
        assert!(
            seen > 5,
            "непустых чанков всего {seen} — сверять почти нечего"
        );
    }

    /// **Забытый чанк перестаёт стоить память и собирается заново.**
    /// Без этого кольцо на карте в километр копит состояние до конца
    /// партии: 726 КиБ на каждый чанк, который когда-либо копали.
    #[test]
    fn a_forgotten_chunk_is_rebuilt_from_the_world() {
        let mut w = voxel::World::new(128, 64, 128, 1 << 16, Box::new(Steps));
        let mut layer = MeshLayer::new(&w, Opts::default());
        let edits = dig(
            &mut w,
            (14..24u32)
                .flat_map(|y| (30..40u32).flat_map(move |z| (30..40u32).map(move |x| (x, y, z)))),
        );
        layer.apply(&w, &edits);
        let i = layer.dirty()[0] as usize;
        let before = sorted(layer.quads(i));
        assert!(!before.is_empty(), "чанк пуст — забывать нечего");
        assert!(layer.is_live(i));
        assert!(layer.forget(i), "чанк не забылся");
        assert!(!layer.is_live(i));
        assert!(
            layer.quads(i).is_empty(),
            "забытый чанк всё ещё отдаёт квады"
        );
        assert!(!layer.forget(i), "забыть дважды удалось");
        // Возвращается — тем же, каким был.
        assert_eq!(before, sorted(layer.scratch_quads(&w, i).to_vec()));
        layer.apply(&w, &edits);
        assert_eq!(
            before,
            sorted(layer.quads(i)),
            "после возврата геометрия другая"
        );
    }

    /// **Правка за кольцом ничего не строит.** Чанк попадает в грязные,
    /// но мешера у него не появляется — иначе на километре каждый
    /// задетый за кольцом чанк собирался бы и забывался каждый тик.
    /// С разрешением тот же чанк строится и даёт ту же геометрию,
    /// что разовая сборка из мира.
    #[test]
    fn an_edit_outside_the_kept_set_builds_nothing() {
        let mut w = voxel::World::new(128, 64, 128, 1 << 16, Box::new(Steps));
        let mut layer = MeshLayer::new(&w, Opts::default());
        let edits = dig(
            &mut w,
            (14..20u32)
                .flat_map(|y| (30..40u32).flat_map(move |z| (30..40u32).map(move |x| (x, y, z)))),
        );
        let n = layer.apply_where(&w, &edits, |_| false);
        assert!(n > 0, "правки никого не запачкали");
        assert_eq!(layer.live_chunks(), 0, "чанк за кольцом построился");
        assert_eq!(layer.state_bytes(), 0, "чанк за кольцом стоит память");
        assert!(
            !layer.dirty().is_empty(),
            "грязных нет — рендеру нечего сбрасывать"
        );
        for &i in layer.dirty() {
            assert!(!layer.is_live(i as usize), "чанк {i} осел в слое");
        }
        assert_eq!(layer.apply_where(&w, &edits, |_| true), n);
        assert_eq!(layer.live_chunks(), n, "с разрешением построились не все");
        let i = layer.dirty()[0] as usize;
        assert!(!layer.quads(i).is_empty(), "построенный чанк пуст");
        assert_eq!(
            sorted(layer.quads(i)),
            sorted(layer.scratch_quads(&w, i).to_vec())
        );
    }

    /// **Нетронутый чанк квадов не отдаёт.** Слой ленивый, и выдумать
    /// ему геометрию значило бы построить весь мир на первом запросе.
    #[test]
    fn an_untouched_chunk_has_no_live_quads() {
        let w = voxel::World::new(128, 64, 128, 1 << 16, Box::new(Steps));
        let layer = MeshLayer::new(&w, Opts::default());
        assert!(layer.quads(0).is_empty());
        assert!(!layer.rebuilt_quads(&w, 0).is_empty(), "чанк и правда пуст");
    }
}
