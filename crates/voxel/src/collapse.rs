//! Каскад отделения: правки тика → компоненты чанка → граф → тела.
//!
//! **Тик — атом.** Вопрос «что отвалилось» задаётся по состоянию конца
//! тика, а не по каждой правке отдельно. Это не оформительская разница:
//! пока каскад спрашивал по правке, толстый вырез за один тик проходил
//! мимо него целиком (ADR-118). Ступени A и B (ADR-055) отвечают на
//! вопрос «расщепило ли снятие ЭТОГО воксела» и потому годятся только
//! там, где снятие одно.
//!
//! Ответ на пачку — разметка связных компонент чанка (5–10 мкс на
//! чанк 32³) и проверка заякоренности.
//!
//! **Ступень C — граф компонент чанков** (ADR-119). Компонента,
//! касающаяся грани чанка, держится не за грань, а за то, что с той
//! стороны, и вопрос рекурсивен. Узел графа — пара (чанк, компонента),
//! ребро — контакт твёрдого с твёрдым через грань, заземление —
//! **только коренная порода**.
//!
//! Соблазн сказать «нетронутый чанк заякорен по определению» велик
//! и неверен: настил моста лежит в нетронутом чанке, а держится
//! за подрезанную опору в тронутом. Поэтому чанк, до которого достаёт
//! контакт, **втягивается** в граф, а пока не втянут — считается
//! заземляющим. Такая оценка консервативна: она не роняет лишнего,
//! но может отложить обрушение на тик-другой.
//!
//! Втягивание идёт с потолком за тик, как и всё прочее: первая же
//! правка иначе построила бы граф по всей карте разом.

use crate::bodies::Bodies;
use crate::topo::{batch_cannot_split, Chunk as TopoChunk, Components, SplitBufs, CS};
use crate::{Edit, World};
use std::collections::BTreeMap;

/// Потолок на размер одного отвала в вокселях **по умолчанию**.
///
/// Настоящее значение приходит из `data/sim.ron` через
/// `CollapseLayer::set_max_body_cells`: это число тика, и жить
/// в исходнике оно не должно (ADR-191). Здесь оно остаётся
/// умолчанием для тех, кто строит слой без настроек — стендов
/// и тестов самого `voxel`.
pub const MAX_BODY_CELLS: usize = 4096;

/// Сколько раз за тик каскад повторяет разбор.
///
/// Оторвав кусок, каскад уносит с собой рёбра графа, и то, что
/// держалось за него, тоже теряет опору. Раньше разбор повторялся
/// до отсутствия отрывов, и это **удваивало цену каждый тик**: второй
/// раунд шёл почти всегда и не находил ничего. Замер на D-1 — три
/// тысячи семьсот девять отвалов за прогон, **ни одного** не первым
/// раундом (ADR-129).
///
/// Поэтому раунд один, а повторный разбор откладывается на следующий
/// тик: граф после отрыва приводится в порядок, а флаг `recheck`
/// заставляет каскад отработать даже без единой правки.
const MAX_ROUNDS: usize = 1;

/// Сколько чанков за тик разрешено втянуть в граф. Потолок обязателен:
/// контакт твёрдого с твёрдым идёт по всему рельефу, и без него первая
/// же правка разметила бы карту целиком.
const DEFAULT_PROMOTE_PER_TICK: usize = 64;

/// Сколько чанков за тик разрешено переразметить.
///
/// Разметка компонент — основная цена каскада: 14.15 мкс на изрытый
/// чанк 32³, и копка пачкает их шестьдесят пять за тик (ADR-127).
/// Потолок здесь такой же, как у всего прочего в проекте, и с той же
/// оговоркой: чанк, до которого не дошли, считается **заземляющим**,
/// то есть ответ каскада — оценка сверху. Лишнего не упадёт; настоящий
/// отвал сдвигается.
///
/// **Значение выбрано так, чтобы потолок не срабатывал** под нагрузкой
/// D-1 (ADR-128). Потолок, упирающийся каждый тик, — не предохранитель,
/// а постоянное ухудшение (ADR-116), и здесь это видно буквально:
/// при шестнадцати отвалов не происходит вовсе. Признак — счётчик
/// `relabel_pending`: пока он не ноль, ответ каскада приблизителен.
const DEFAULT_RELABEL_PER_TICK: usize = 64;

/// Шесть граней чанка: -x, +x, -y, +y, -z, +z.
const FACES: usize = 6;

/// Смещение чанка-соседа по грани.
const FACE_DIR: [(i32, i32, i32); FACES] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

/// Грань-напарник: у соседа контакт идёт через противоположную грань.
/// Порядок граней подобран так, что это один XOR.
#[inline]
const fn opposite(f: usize) -> usize {
    f ^ 1
}

#[derive(Clone, Copy, Default, Debug)]
pub struct CollapseStats {
    /// Компонент рассмотрено.
    pub probed: usize,
    /// Из них признано отделением.
    pub detached: usize,
    /// Компонент отвалилось, но не влезло в потолок тела.
    pub too_big: usize,
    /// Отделений отложено: группа касается удержанного чанка, и её отрыв
    /// ждёт следующего прохода (ADR-645).
    pub held: usize,
    /// Unsupported voxel candidates, counted from run masks before allocation.
    pub gather_candidates: usize,
    /// Peak retained candidate cells; oversized groups release their prefix.
    pub gather_buffer_peak: usize,
    /// Вокселей ушло в тела.
    pub cells: usize,
    /// Узлов в графе компонент — размер ступени C.
    pub nodes: usize,
    /// Рёбер в графе.
    pub edges: usize,
    /// Сколько раундов разбора понадобилось.
    pub rounds: usize,
    /// Разбор будет повторён следующим тиком: в этом что-то оторвалось.
    pub rounds_capped: bool,
    /// Чанков втянуто в граф за тик.
    pub promoted: usize,
    /// Чанков ждёт втягивания. Пока фронтир не пуст, ответ каскада —
    /// оценка сверху: часть кусков ещё держится за неразмеченное.
    pub frontier: usize,
    /// Чанков ждёт переразметки. То же: оценка сверху, отвал сдвигается.
    pub relabel_pending: usize,
    /// Отвалов, найденных **не первым** раундом. Если их нет, повторный
    /// разбор — чистая трата.
    pub torn_after_first: usize,
    /// Чанков размечено заново.
    pub relabelled: usize,
    /// Граней пересчитано.
    pub faces: usize,
    /// Чанков, в которых пришлось собирать состав незаякоренного.
    pub gathered: usize,
    /// Чанков собрано из упакованной занятости мира.
    pub built: usize,
    /// Чанков, которым переразметка не понадобилась: локальная
    /// проверка доказала, что пачка снятий ничего не расщепила.
    pub probe_skipped: usize,
}

#[derive(Clone, Copy)]
struct CachedLink {
    chunk: u32,
    lab: u16,
    source_lab: u16,
}

#[derive(Clone, Copy)]
struct AnchorCandidate {
    chunk: u32,
    lab: u16,
    id: u32,
}

/// Компоненты одного чанка, как их видит граф.
struct ChunkNodes {
    cached_edges: usize,
    /// Derived adjacency grouped by local component. One link allocation
    /// keeps each component's contacts consecutive during support walks.
    links: Vec<CachedLink>,
    link_starts: Vec<u32>,
    /// Local components without direct bedrock or conservative face anchors.
    /// A small mask handles the common one- or two-component terrain chunk.
    missing_mask: u64,
    /// Сколько компонент в чанке.
    n: usize,
    /// Метки компонент на граничных ячейках грани — **отрезками**,
    /// а не ячейка за ячейкой.
    ///
    /// Раскладка индекса внутри грани: для граней по X — `z * CS + y`,
    /// по Y — `z * CS + x`, по Z — `y * CS + x`. У соседа та же ячейка
    /// лежит по тому же индексу его противоположной грани, поэтому
    /// пересечение двух граней — слияние двух коротких списков.
    ///
    /// Различных меток на грани единицы, а ячеек тысяча двадцать
    /// четыре: плотная карта стоила и обхода, и двенадцати килобайт
    /// на чанк (ADR-127). Отрезок — `(начало, длина, метка)`.
    faces: [Vec<(u16, u16, u16)>; FACES],
    /// Стоит ли компонента на коренной породе.
    bedrock: Vec<bool>,
    /// Производное от соседей: рёбра `(моя метка, метка соседа)`
    /// и метки, заземлённые контактом с нетронутым чанком.
    /// Пересчитывается, когда меняется этот чанк или сосед.
    edges: [Vec<(u16, u16)>; FACES],
    ground: [Vec<u16>; FACES],
}

impl ChunkNodes {
    fn new() -> ChunkNodes {
        ChunkNodes {
            cached_edges: 0,
            links: Vec::new(),
            link_starts: Vec::new(),
            missing_mask: 0,
            n: 0,
            faces: Default::default(),
            bedrock: Vec::new(),
            edges: Default::default(),
            ground: Default::default(),
        }
    }
    fn without_direct_anchor<'a>(&'a self, overflow: &'a [u16]) -> impl Iterator<Item = u16> + 'a {
        let mut bits = self.missing_mask;
        std::iter::from_fn(move || {
            if bits == 0 {
                return None;
            }
            let lab = bits.trailing_zeros() as u16;
            bits &= bits - 1;
            Some(lab)
        })
        .chain(overflow.iter().copied())
    }
    fn links_for(&self, lab: usize) -> &[CachedLink] {
        &self.links[self.link_starts[lab] as usize..self.link_starts[lab + 1] as usize]
    }
}

pub struct CollapseLayer {
    comps: Components,
    // Derived graph summaries; authoritative nodes keep the same wire format.
    /// Derived membership by chunk index. Sequential scans are cheaper than
    /// walking a tree for every collapse tick on the kilometre range.
    /// Chunks with components lacking a direct anchor, one bit per chunk.
    /// Word order and low-bit order preserve increasing chunk index.
    anchor_bits: Vec<u64>,
    anchor_count: usize,
    /// Rare chunks with more than 64 components keep their extra labels here.
    anchor_overflow: BTreeMap<u32, Vec<u16>>,
    edge_count: usize,
    chunks: Vec<Option<Box<TopoChunk>>>,
    nx: u32,
    ny: u32,
    nz: u32,
    /// The world's size in voxels across: its frame (ADR-034) anchors
    /// what touches it (ADR-433). Derived from the world, not saved.
    wx: u32,
    wz: u32,
    dirty: Vec<u32>,
    stamp: Vec<u32>,
    epoch: u32,
    /// Потолок на размер одного отвала, из данных.
    max_body_cells: usize,
    /// Правки тика по чанкам: местные координаты и твёрдость после.
    local: Vec<Vec<(u8, u8, u8, bool)>>,
    /// Узлы графа на чанк. `None` — чанк каскаду неизвестен, то есть
    /// его материал сгенерирован и заякорен по определению.
    nodes: Vec<Option<Box<ChunkNodes>>>,
    /// Номер первого узла чанка в общей нумерации.
    base: Vec<u32>,
    /// Известные чанки по возрастанию индекса: обход обязан быть
    /// стабильным, а не зависеть от порядка появления.
    known: Vec<u32>,
    /// Заякоренность на глобальный номер узла.
    reached: Vec<bool>,
    /// Ordered unresolved labels for this resolve pass; capacity is reused.
    anchor_candidates: Vec<AnchorCandidate>,
    walk: Vec<(u32, u16)>,
    /// Derived by resolve, consumed before any topology changes: only these
    /// chunks can detach. Roots are valid only for nodes not in `reached`.
    unsupported_chunks: Vec<u32>,
    component_roots: Vec<u32>,
    #[cfg(test)]
    queue: Vec<u32>,
    /// Грани, которые надо пересчитать: `(чанк, грань)`.
    face_todo: Vec<(u32, u8)>,
    /// Чанки, которые графу нужно втянуть (ADR-460): соседи правленых
    /// чанков — правка могла лишить опоры то, что за их гранями, — и
    /// неразмеченные соседи компонент, у которых нет пути к коренной
    /// породе по размеченному графу. Контакт с любым неразмеченным чанком
    /// считается заземляющим — это оценка сверху, — и уточнять её нужно
    /// только там, где на ней одной что-то может держаться.
    frontier: Vec<u32>,
    /// Буферы обхода от коренной породы для фронтира: заякоренность
    /// по глобальному номеру узла и очередь.
    rooted: Vec<bool>,
    rooted_walk: Vec<(u32, u16)>,
    /// Потолок втягивания за тик.
    pub max_promote_per_tick: usize,
    /// Потолок переразметок за тик.
    pub max_relabel_per_tick: usize,
    /// Пользоваться ли локальной проверкой «пачка не расщепила».
    ///
    /// Выключатель существует не для настройки, а для проверки:
    /// каскад с проверкой и без обязан давать **один и тот же**
    /// набор тел, и на этом стоит тест (ADR-131).
    pub use_split_probe: bool,
    /// Чанки, чья разметка отстала от их битовой карты, по возрастанию
    /// индекса: порядок обхода обязан быть от карты, а не от того,
    /// куда игрок кликнул раньше.
    pending: Vec<u32>,
    /// Отстала ли разметка чанка.
    stale: Vec<bool>,
    /// В прошлом тике что-то оторвалось: граф изменился, и разбор
    /// нужно повторить, даже если правок нет.
    recheck: bool,
    /// Буфер состава куска в мировых координатах.
    cells: Vec<(u32, u32, u32)>,
    /// Прежние отрезки — для сравнения при переразметке.
    face_prev: [Vec<(u16, u16, u16)>; FACES],
    /// Буферы локальной проверки «пачка не расщепила».
    split: SplitBufs,
    /// Снятое в текущем чанке — вход этой проверки.
    removed: Vec<(u8, u8, u8)>,
    /// Счётчики работы за тик. Замер объёма, а не таймер: `std::time`
    /// в крейтах симуляции запрещён (`CLAUDE.md`).
    relabelled: usize,
    faces: usize,
    gathered: usize,
    built: usize,
}

impl CollapseLayer {
    /// Work survives the edit batch that started it: frontier expansion,
    /// deferred relabels and retries for a full body pool.
    pub fn pending(&self) -> bool {
        self.recheck || !self.frontier.is_empty() || !self.pending.is_empty()
    }
    /// Поставить потолок на размер отвала из данных.
    ///
    /// Отдельным вызовом, а не параметром конструктора: слой строят
    /// стенды и тесты самого `voxel`, у которых `Tuning` нет и быть
    /// не должно — крейт мира не знает про настройки игры (ADR-191).
    ///
    /// Больший потолок меняет ответ о куске, который прежний не пустил
    /// падать, и разбор повторяется — если он уже что-то видел
    /// (ADR-440).
    pub fn set_max_body_cells(&mut self, n: usize) {
        let n = n.max(1);
        if n > self.max_body_cells && self.chunks.iter().any(Option::is_some) {
            self.recheck = true;
        }
        self.max_body_cells = n;
    }

    pub fn new(w: &World) -> CollapseLayer {
        let cs = CS as u32;
        let (nx, ny, nz) = (w.nx.div_ceil(cs), w.ny.div_ceil(cs), w.nz.div_ceil(cs));
        let n = (nx * ny * nz) as usize;
        CollapseLayer {
            comps: Components::new(),
            anchor_bits: vec![0; n.div_ceil(64)],
            anchor_count: 0,
            anchor_overflow: BTreeMap::new(),
            edge_count: 0,
            chunks: (0..n).map(|_| None).collect(),
            nx,
            ny,
            nz,
            wx: w.nx,
            wz: w.nz,
            dirty: Vec::new(),
            stamp: vec![u32::MAX; n],
            epoch: 0,
            max_body_cells: MAX_BODY_CELLS,
            local: (0..n).map(|_| Vec::new()).collect(),
            nodes: (0..n).map(|_| None).collect(),
            base: vec![0; n],
            known: Vec::new(),
            reached: Vec::new(),
            anchor_candidates: Vec::new(),
            walk: Vec::new(),
            unsupported_chunks: Vec::new(),
            component_roots: Vec::new(),
            #[cfg(test)]
            queue: Vec::new(),
            face_todo: Vec::new(),
            face_prev: Default::default(),
            split: SplitBufs::new(),
            removed: Vec::new(),
            relabelled: 0,
            faces: 0,
            gathered: 0,
            built: 0,
            frontier: Vec::new(),
            rooted: Vec::new(),
            rooted_walk: Vec::new(),
            max_promote_per_tick: DEFAULT_PROMOTE_PER_TICK,
            max_relabel_per_tick: DEFAULT_RELABEL_PER_TICK,
            use_split_probe: true,
            pending: Vec::new(),
            stale: vec![false; n],
            recheck: false,
            cells: Vec::new(),
        }
    }

    #[inline]
    fn index(&self, cx: u32, cy: u32, cz: u32) -> usize {
        ((cz * self.ny + cy) * self.nx + cx) as usize
    }

    #[inline]
    fn coords(&self, i: usize) -> (u32, u32, u32) {
        let i = i as u32;
        (
            i % self.nx,
            (i / self.nx) % self.ny,
            i / (self.nx * self.ny),
        )
    }

    fn neighbour(&self, i: usize, f: usize) -> Option<usize> {
        let (cx, cy, cz) = self.coords(i);
        let (dx, dy, dz) = FACE_DIR[f];
        let (nx, ny, nz) = (cx as i32 + dx, cy as i32 + dy, cz as i32 + dz);
        if nx < 0 || ny < 0 || nz < 0 {
            return None;
        }
        let (nx, ny, nz) = (nx as u32, ny as u32, nz as u32);
        if nx >= self.nx || ny >= self.ny || nz >= self.nz {
            return None;
        }
        Some(self.index(nx, ny, nz))
    }

    /// Copy the existing occupancy words, including partial edge chunks.
    /// A 32-cell X row is aligned to one half of the world's 64-bit word.
    fn build(w: &World, cx: u32, cy: u32, cz: u32) -> Box<TopoChunk> {
        let cs = CS as u32;
        let (ox, oy, oz) = (cx * cs, cy * cs, cz * cs);
        let mut c = Box::new(TopoChunk::new());
        let width = w.nx.saturating_sub(ox).min(cs);
        if width == 0 {
            return c;
        }
        let mask = u32::MAX >> (32 - width);
        let wx = w.bit_words_per_row() as usize;
        for z in 0..w.nz.saturating_sub(oz).min(cs) {
            for y in 0..w.ny.saturating_sub(oy).min(cs) {
                let row = ((oy + y) as usize * w.nz as usize + (oz + z) as usize) * wx;
                c.cols[z as usize * CS + y as usize] =
                    ((w.bits()[row + (ox / 64) as usize] >> (ox % 64)) as u32) & mask;
            }
        }
        c
    }

    /// Прогнать правки тика через каскад и оторвать всё, что отвалилось.
    pub fn apply(&mut self, w: &mut World, edits: &[Edit], bodies: &mut Bodies) -> CollapseStats {
        self.apply_observed(w, edits, bodies, |_| {})
    }
    pub fn apply_observed(
        &mut self,
        w: &mut World,
        edits: &[Edit],
        bodies: &mut Bodies,
        completed: impl FnMut(&'static str),
    ) -> CollapseStats {
        self.apply_iter_observed(w, edits.iter().copied(), bodies, completed)
    }

    /// Stream the same ordered edit batch without a second materialized copy.
    /// This still resolves one complete batch, never a partial support graph.
    pub fn apply_iter_observed(
        &mut self,
        w: &mut World,
        edits: impl Iterator<Item = Edit>,
        bodies: &mut Bodies,
        completed: impl FnMut(&'static str),
    ) -> CollapseStats {
        self.apply_iter_held(w, edits, bodies, completed, &|_| false)
    }

    /// The same pass, with `held` chunks (by chunk coordinates) whose
    /// unsupported groups are not torn off yet (ADR-645): every edit still
    /// updates the layer's copy of the world, and a held group is found
    /// again by a later pass, once its region is released.
    pub fn apply_iter_held(
        &mut self,
        w: &mut World,
        edits: impl Iterator<Item = Edit>,
        bodies: &mut Bodies,
        mut completed: impl FnMut(&'static str),
        held: &dyn Fn([u32; 3]) -> bool,
    ) -> CollapseStats {
        let cs = CS as u32;
        let mut st = CollapseStats::default();
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == u32::MAX {
            self.stamp.iter_mut().for_each(|v| *v = 0);
            self.epoch = 1;
        }
        self.relabelled = 0;
        self.faces = 0;
        self.gathered = 0;
        self.built = 0;
        self.dirty.clear();
        for e in edits {
            let (cx, cy, cz) = (e.x / cs, e.y / cs, e.z / cs);
            if cx >= self.nx || cy >= self.ny || cz >= self.nz {
                continue;
            }
            let i = self.index(cx, cy, cz);
            if self.stamp[i] != self.epoch {
                self.stamp[i] = self.epoch;
                self.local[i].clear();
                self.dirty.push(i as u32);
            }
            self.local[i].push((
                (e.x % cs) as u8,
                (e.y % cs) as u8,
                (e.z % cs) as u8,
                e.mat != crate::store::AIR,
            ));
        }
        // **Непустой фронтир — тоже работа.** Втягивание чанков идёт
        // по потолку, и в большом мире оно переживает саму копку:
        // правки кончились, а до части графа ещё не дошли. Выйти
        // здесь значило бы, что отвал не «сдвигается на следующий
        // тик», а теряется навсегда — кусок висит в воздухе, пока
        // кто-нибудь не копнёт рядом (ADR-227).
        //
        // Замер: в мире 320³ подрез ножки гриба даёт правки в тиках
        // 0–2 и фронтир в 14 чанков на тике 2; без этой строки отвала
        // не происходит ни разу за сорок тиков.
        if self.dirty.is_empty() && !self.pending() {
            return st;
        }

        // Чанк, до которого дошла правка, входит в граф — **любая**,
        // не только снятие. Засыпка сама по себе ничего не роняет,
        // но меняет рёбра: приземлившееся тело запекается засыпкой,
        // и то, что оно подпёрло, держаться обязано.
        self.face_todo.clear();
        for k in 0..self.dirty.len() {
            let i = self.dirty[k] as usize;
            let (cx, cy, cz) = self.coords(i);
            let fresh = self.chunks[i].is_none();
            if fresh {
                self.chunks[i] = Some(Self::build(w, cx, cy, cz));
                self.built += 1;
                // Свежий чанк размечается сразу: у него ещё нет узлов,
                // а отложить нечего — граф о нём не знает вовсе.
                self.relabel(i);
                self.mark_faces(i);
                continue;
            }
            // Битовая карта обновляется всегда — это O(правок).
            let mut c = self.chunks[i].take().expect("chunk checked");
            let mut only_removals = true;
            let mut bottom_row = false;
            self.removed.clear();
            for &(lx, ly, lz, solid) in &self.local[i] {
                c.set(lx as usize, ly as usize, lz as usize, solid);
                if solid {
                    only_removals = false;
                } else {
                    self.removed.push((lx, ly, lz));
                    if cy == 0 && ly == 0 {
                        bottom_row = true;
                    }
                }
            }
            // Локальная проверка: расщепила ли пачка снятий состав
            // компонент. Достаточное условие, поэтому «да» — это
            // «переразмечать незачем», а «не знаю» — обычная очередь
            // (ADR-131).
            //
            // Засыпка сливает компоненты, а не режет: её проверка
            // не рассматривает. Нижний ряд чанков исключён отдельно —
            // там снятие может отнять у компоненты коренную породу,
            // а это меняет заземление, не меняя состава.
            // И только у чанка **с одной компонентой**. Проверка знает
            // про расщепление, но не про порядок меток: снятая целиком
            // компонента или её первый в обходе пробег меняют номера
            // остальных, а сбор отрыва строит метки заново и читает
            // по ним «дошли» и «корень», посчитанные по старым. Стоящий
            // на породе столб отрывался телом (ADR-422). У одной
            // компоненты номер один — ноль — и сдвигаться нечему.
            let skip = self.use_split_probe
                && only_removals
                && !bottom_row
                && !self.removed.is_empty()
                && self.nodes[i].as_ref().is_some_and(|nd| nd.n == 1)
                && batch_cannot_split(&c, &self.removed, &mut self.split);
            self.chunks[i] = Some(c);
            if skip {
                st.probe_skipped += 1;
                if self.drop_removed_from_faces(i) {
                    self.mark_faces(i);
                }
            } else if !self.stale[i] {
                self.stale[i] = true;
                self.pending.push(i as u32);
            }
        }
        completed("collapse_edits");
        // Переразметка по потолку. Порядок — по возрастанию индекса
        // чанка, поэтому от порядка правок он не зависит.
        self.pending.sort_unstable();
        let take = self.max_relabel_per_tick.min(self.pending.len());
        let batch: Vec<u32> = self.pending.drain(..take).collect();
        st.relabel_pending = self.pending.len();
        for &ci in &batch {
            let i = ci as usize;
            self.stale[i] = false;
            if self.relabel(i) {
                self.mark_faces(i);
            }
        }
        self.rebuild_index();
        self.refresh_faces(w);
        completed("collapse_labels");
        st.promoted = self.promote(w);
        completed("collapse_promote");
        st.frontier = self.frontier.len();

        let refused_before = bodies.refused;
        let mut torn_total = 0usize;
        for round in 0..MAX_ROUNDS {
            st.rounds = round + 1;
            self.resolve(&mut st);
            completed("collapse_resolve");
            let torn = self.detach_unanchored(w, bodies, &mut st, held);
            completed("collapse_detach");
            if round > 0 {
                st.torn_after_first += torn;
            }
            torn_total += torn;
            if torn == 0 {
                break;
            }
            // Граф после отрыва приводится в порядок здесь же: иначе
            // следующий тик читал бы рёбра, ведущие в исчезнувшее.
            self.rebuild_index();
            self.refresh_faces(w);
        }
        // Оторвав кусок, каскад мог лишить опоры что-то ещё. Разбор
        // повторяется **следующим тиком**, а не сейчас: замер говорит,
        // что повторный раунд не находит ничего, а стоит как первый.
        // Отказ пула тел ждёт места, и место приходит без правки мира.
        // Кусок сверх потолка — нет: его ответ меняют только правка
        // рядом и другой потолок, и повтор каждого тика был бы
        // потолком, срабатывающим каждый тик (ADR-440, ADR-116).
        // Отрыв меняет граф: безопорное ищется заново.
        self.extend_frontier();
        self.recheck = torn_total > 0 || bodies.refused > refused_before || st.held > 0;
        st.rounds_capped = self.recheck;
        self.report(&mut st);
        st
    }

    fn report(&self, st: &mut CollapseStats) {
        st.relabelled = self.relabelled;
        st.faces = self.faces;
        st.gathered = self.gathered;
        st.built = self.built;
    }

    /// Втянуть в граф чанки с фронтира, сколько позволяет потолок.
    /// Возвращает, сколько втянуто.
    ///
    /// Порядок — по возрастанию индекса чанка, а не по порядку
    /// появления: иначе состав графа зависел бы от порядка обхода
    /// правок, то есть от того, куда игрок кликнул раньше.
    fn promote(&mut self, w: &World) -> usize {
        // What lay past an edited chunk's faces may have stood on the
        // cells the edit took, whether or not anything still touches it:
        // the neighbours are taken in (ADR-460).
        for k in 0..self.dirty.len() {
            let i = self.dirty[k] as usize;
            for f in 0..FACES {
                if let Some(j) = self.neighbour(i, f) {
                    if self.nodes[j].is_none() {
                        self.frontier.push(j as u32);
                    }
                }
            }
        }
        let mut done = 0usize;
        // Втягивание идёт **пачками**, а не по чанку.
        //
        // Первая редакция пересчитывала грани после каждого чанка,
        // и цена выходила квадратичной по числу втянутых: на реальной
        // копке это дало 2.97 мс за тик (ADR-126). Новые чанки фронтира
        // находятся при пересчёте граней, поэтому раунды нужны, но их
        // единицы, а не десятки.
        while done < self.max_promote_per_tick {
            self.extend_frontier();
            let batch: Vec<u32> = self
                .frontier
                .iter()
                .copied()
                .take(self.max_promote_per_tick - done)
                .collect();
            if batch.is_empty() {
                break;
            }
            for &ci in &batch {
                let i = ci as usize;
                let (cx, cy, cz) = self.coords(i);
                self.chunks[i] = Some(Self::build(w, cx, cy, cz));
                self.built += 1;
                self.relabel(i);
                self.mark_faces(i);
                done += 1;
            }
            self.frontier.retain(|&i| self.nodes[i as usize].is_none());
            self.rebuild_index();
            self.refresh_faces(w);
        }
        self.extend_frontier();
        done
    }

    /// Добавить во фронтир неразмеченных соседей тех компонент, до которых
    /// от коренной породы не доходит путь по размеченным узлам (ADR-460).
    /// Контакт с неразмеченным держит такую компоненту только оценкой
    /// сверху, и уточнить её может лишь втягивание соседа. Компонента
    /// на породе держится, что бы ни лежало за её прочими гранями, и
    /// соседей не тянет: прежде тянула любая, и граф после первой правки
    /// заливал весь сплошной грунт карты по потолку втягивания за тик,
    /// а разбор каждого тика шёл по растущему графу. Фронтир остаётся
    /// по возрастанию номера чанка, без размеченных.
    fn extend_frontier(&mut self) {
        let total = self.reached.len();
        self.rooted.clear();
        self.rooted.resize(total, false);
        self.rooted_walk.clear();
        for &ci in &self.known {
            let i = ci as usize;
            let nd = self.nodes[i].as_ref().expect("known");
            for lab in 0..nd.n {
                if nd.bedrock[lab] {
                    self.rooted[self.base[i] as usize + lab] = true;
                    self.rooted_walk.push((ci, lab as u16));
                }
            }
        }
        while let Some((ci, lab)) = self.rooted_walk.pop() {
            let nd = self.nodes[ci as usize].as_ref().expect("known");
            for &CachedLink {
                chunk: j, lab: b, ..
            } in nd.links_for(lab as usize)
            {
                if self.nodes[j as usize].is_none() {
                    continue;
                }
                let id = (self.base[j as usize] + u32::from(b)) as usize;
                if !self.rooted[id] {
                    self.rooted[id] = true;
                    self.rooted_walk.push((j, b));
                }
            }
        }
        for &ci in &self.known {
            let i = ci as usize;
            let nd = self.nodes[i].as_ref().expect("known");
            let base = self.base[i] as usize;
            for f in 0..FACES {
                if nd.ground[f]
                    .iter()
                    .all(|&a| self.rooted[base + usize::from(a)])
                {
                    continue;
                }
                if let Some(j) = self.neighbour(i, f) {
                    if self.nodes[j].is_none() {
                        self.frontier.push(j as u32);
                    }
                }
            }
        }
        self.frontier.sort_unstable();
        self.frontier.dedup();
        self.frontier.retain(|&i| self.nodes[i as usize].is_none());
    }

    /// Разметить компоненты чанка заново и обновить его грани.
    ///
    /// Возвращает `true`, если **карта граничных меток изменилась**.
    /// Копка в середине чанка её не трогает, а рёбра и заземления
    /// графа выводятся ровно из неё: если она та же, пересчитывать
    /// грани — свою и обращённые к себе у шести соседей — незачем.
    /// На реальной копке это девять из десяти чанков (ADR-126).
    fn relabel(&mut self, i: usize) -> bool {
        let c = self.chunks[i].take().expect("chunk present");
        self.relabelled += 1;
        let n = self.comps.build(&c);
        let mut nd = self.nodes[i]
            .take()
            .unwrap_or_else(|| Box::new(ChunkNodes::new()));
        for f in 0..FACES {
            self.face_prev[f].clear();
            self.face_prev[f].extend_from_slice(&nd.faces[f]);
            nd.faces[f].clear();
        }
        nd.n = n;
        nd.bedrock.clear();
        nd.bedrock.resize(n, false);
        let (cx, cy, cz) = self.coords(i);
        // The map's frame cannot be dug or built (ADR-034): what touches
        // it stands as on bedrock (ADR-433). Unanchored, a ledge against
        // the side wall fell as a body whose frame cells never left the
        // world, and the body waited for them for ever.
        let cs = CS as u32;
        let mut frame_x = 0u32;
        if cx == 0 {
            frame_x |= 1;
        }
        if cx == (self.wx - 1) / cs {
            frame_x |= 1 << ((self.wx - 1) % cs);
        }
        let frame_z = |lz: usize| {
            (cz == 0 && lz == 0) || (cz == (self.wz - 1) / cs && lz as u32 == (self.wz - 1) % cs)
        };
        for (ri, r) in self.comps.runs.iter().enumerate() {
            let lab = self.comps.label[ri] as u16;
            let col = r.col as usize;
            let (ly, lz) = (col % CS, col / CS);
            let m = r.mask;
            if m & 1 != 0 {
                push_face(&mut nd.faces[0], lz * CS + ly, lab);
            }
            if m & (1 << 31) != 0 {
                push_face(&mut nd.faces[1], lz * CS + ly, lab);
            }
            if ly == 0 || ly == CS - 1 || lz == 0 || lz == CS - 1 {
                let (fy, fz) = (if ly == 0 { 2 } else { 3 }, if lz == 0 { 4 } else { 5 });
                let mut bits = m;
                while bits != 0 {
                    let x = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    if ly == 0 || ly == CS - 1 {
                        push_face(&mut nd.faces[fy], lz * CS + x, lab);
                    }
                    if lz == 0 || lz == CS - 1 {
                        push_face(&mut nd.faces[fz], ly * CS + x, lab);
                    }
                }
            }
            // Коренная порода: самый низ мира держит всё, что на нём стоит.
            if cy == 0 && ly == 0 && m != 0 {
                nd.bedrock[lab as usize] = true;
            }
            if m & frame_x != 0 || (m != 0 && frame_z(lz)) {
                nd.bedrock[lab as usize] = true;
            }
        }
        let changed = (0..FACES).any(|f| nd.faces[f] != self.face_prev[f]);
        // Keep the sorted active index instead of scanning every possible
        // chunk on the kilometre map. Empty/invalid entries are filtered at
        // the same rebuild boundary as before; bases retain their semantics.
        if n > 0 {
            if let Err(at) = self.known.binary_search(&(i as u32)) {
                self.known.insert(at, i as u32);
            }
        }
        self.nodes[i] = Some(nd);
        self.cache_nodes(i);
        self.chunks[i] = Some(c);
        changed
    }

    /// Выкинуть снятые ячейки из отрезков граней чанка.
    ///
    /// Нужно потому, что при пропуске переразметки метки остаются
    /// верными, а вот **состав граничных ячеек** меняется: снятая
    /// ячейка больше не касается соседа, и оставить её в отрезках —
    /// значит держать ребро, которого нет.
    ///
    /// Возвращает `true`, если что-то изменилось.
    fn drop_removed_from_faces(&mut self, i: usize) -> bool {
        let Some(nd) = self.nodes[i].as_mut() else {
            return false;
        };
        let mut changed = false;
        for &(lx, ly, lz) in &self.removed {
            let (lx, ly, lz) = (lx as usize, ly as usize, lz as usize);
            let mut hit = |f: usize, idx: usize| {
                changed |= drop_face_cell(&mut nd.faces[f], idx as u16);
            };
            if lx == 0 {
                hit(0, lz * CS + ly);
            }
            if lx == CS - 1 {
                hit(1, lz * CS + ly);
            }
            if ly == 0 {
                hit(2, lz * CS + lx);
            }
            if ly == CS - 1 {
                hit(3, lz * CS + lx);
            }
            if lz == 0 {
                hit(4, ly * CS + lx);
            }
            if lz == CS - 1 {
                hit(5, ly * CS + lx);
            }
        }
        changed
    }

    /// Пометить к пересчёту грани чанка и обращённые к нему грани соседей.
    fn mark_faces(&mut self, i: usize) {
        for f in 0..FACES {
            self.face_todo.push((i as u32, f as u8));
            if let Some(j) = self.neighbour(i, f) {
                if self.nodes[j].is_some() {
                    self.face_todo.push((j as u32, opposite(f) as u8));
                }
            }
        }
    }

    /// Пересчитать помеченные грани: рёбра и заземления.
    fn refresh_faces(&mut self, w: &World) {
        // Порядок обхода не влияет на результат — каждая грань считается
        // независимо, — но сортировка убирает повторы и делает работу
        // предсказуемой.
        self.face_todo.sort_unstable();
        self.face_todo.dedup();
        let todo = std::mem::take(&mut self.face_todo);
        self.faces += todo.len();
        for &(ci, f) in &todo {
            self.refresh_face(w, ci as usize, f as usize);
        }
        self.face_todo = todo;
        self.face_todo.clear();
    }

    fn refresh_face(&mut self, w: &World, i: usize, f: usize) {
        if self.nodes[i].is_none() {
            return;
        }
        let mut nd = self.nodes[i].take().expect("nodes present");
        nd.edges[f].clear();
        nd.ground[f].clear();
        let cs = CS as u32;
        let (cx, cy, cz) = self.coords(i);
        let (ox, oy, oz) = (cx * cs, cy * cs, cz * cs);
        let j = self.neighbour(i, f);
        match j {
            None => {
                // Край мира не держит: за ним ничего нет.
            }
            Some(j) if self.nodes[j].is_some() => {
                // Слияние двух списков отрезков: пересекающиеся дают
                // ребро. Обход по отрезкам, а не по ячейкам, — в этом
                // весь смысл кодировки (ADR-127).
                let other = self.nodes[j].as_ref().expect("checked");
                let (a, b) = (&nd.faces[f], &other.faces[opposite(f)]);
                let (mut ia, mut ib) = (0usize, 0usize);
                while ia < a.len() && ib < b.len() {
                    let (as_, al, alab) = a[ia];
                    let (bs, bl, blab) = b[ib];
                    let (ae, be) = (as_ + al, bs + bl);
                    if ae <= bs {
                        ia += 1;
                        continue;
                    }
                    if be <= as_ {
                        ib += 1;
                        continue;
                    }
                    if !nd.edges[f].contains(&(alab, blab)) {
                        nd.edges[f].push((alab, blab));
                    }
                    if ae <= be {
                        ia += 1;
                    } else {
                        ib += 1;
                    }
                }
                nd.edges[f].sort_unstable();
            }
            Some(_) => {
                // Сосед ещё не в графе. Контакт с ним считается
                // заземляющим — это оценка сверху; нужно ли его
                // втягивать, решает фронтир (ADR-460).
                let (dx, dy, dz) = FACE_DIR[f];
                for seg in 0..nd.faces[f].len() {
                    let (start, len, a) = nd.faces[f][seg];
                    if nd.ground[f].contains(&a) {
                        continue;
                    }
                    // Отрезок обходится **до первого твёрдого**: метке
                    // достаточно одного контакта, чтобы заземлиться.
                    for idx in start as usize..(start + len) as usize {
                        let (wx, wy, wz) = Self::face_cell(f, idx, ox, oy, oz);
                        let (sx, sy, sz) = (wx as i32 + dx, wy as i32 + dy, wz as i32 + dz);
                        if sx < 0 || sy < 0 || sz < 0 {
                            continue;
                        }
                        if w.solid(sx as u32, sy as u32, sz as u32) {
                            nd.ground[f].push(a);
                            break;
                        }
                    }
                }
                nd.ground[f].sort_unstable();
            }
        }
        self.nodes[i] = Some(nd);
        self.cache_nodes(i);
    }

    /// Мировые координаты граничной ячейки грани.
    #[inline]
    fn face_cell(f: usize, idx: usize, ox: u32, oy: u32, oz: u32) -> (u32, u32, u32) {
        let (a, b) = ((idx % CS) as u32, (idx / CS) as u32);
        match f {
            0 => (ox, oy + a, oz + b),
            1 => (ox + CS as u32 - 1, oy + a, oz + b),
            2 => (ox + a, oy, oz + b),
            3 => (ox + a, oy + CS as u32 - 1, oz + b),
            4 => (ox + a, oy + b, oz),
            _ => (ox + a, oy + b, oz + CS as u32 - 1),
        }
    }

    /// Пересобрать нумерацию узлов.
    fn rebuild_index(&mut self) {
        self.known
            .retain(|&i| self.nodes[i as usize].as_ref().is_some_and(|nd| nd.n > 0));
        let mut total = 0u32;
        for &ci in &self.known {
            let i = ci as usize;
            self.base[i] = total;
            total += self.nodes[i].as_ref().expect("known").n as u32;
        }
        // `resolve` initializes every live entry before reading it. Preserve
        // the allocation and existing length when only component bases move.
        self.reached.resize(total as usize, false);
    }

    fn cache_nodes(&mut self, i: usize) {
        let neighbours: [_; FACES] = std::array::from_fn(|f| self.neighbour(i, f));
        let nd = self.nodes[i].as_mut().expect("node cache");
        nd.links.clear();
        for (f, neighbour) in neighbours.into_iter().enumerate() {
            if let Some(j) = neighbour {
                for &(a, b) in &nd.edges[f] {
                    // A relabel can temporarily precede its face refresh.
                    if (a as usize) < nd.n {
                        nd.links.push(CachedLink {
                            chunk: j as u32,
                            lab: b,
                            source_lab: a,
                        });
                    }
                }
            }
        }
        // Stable sort retains the former face/edge order within a component.
        nd.links.sort_by_key(|link| link.source_lab);
        nd.link_starts.clear();
        nd.link_starts.resize(nd.n + 1, 0);
        for link in &nd.links {
            nd.link_starts[link.source_lab as usize + 1] += 1;
        }
        for lab in 0..nd.n {
            nd.link_starts[lab + 1] += nd.link_starts[lab];
        }
        let edges = if nd.n == 0 {
            0
        } else {
            nd.edges.iter().map(Vec::len).sum()
        };
        self.edge_count = self.edge_count - nd.cached_edges + edges;
        nd.cached_edges = edges;
        nd.missing_mask = 0;
        let mut overflow = Vec::new();
        for lab in 0..nd.n {
            if nd.bedrock[lab] || nd.ground.iter().any(|g| g.contains(&(lab as u16))) {
                continue;
            }
            if lab < 64 {
                nd.missing_mask |= 1u64 << lab;
            } else {
                overflow.push(lab as u16);
            }
        }
        let missing = nd.missing_mask != 0 || !overflow.is_empty();
        if overflow.is_empty() {
            self.anchor_overflow.remove(&(i as u32));
        } else {
            self.anchor_overflow.insert(i as u32, overflow);
        }
        let bit = 1u64 << (i % 64);
        let word = &mut self.anchor_bits[i / 64];
        if missing && *word & bit == 0 {
            *word |= bit;
            self.anchor_count += 1;
        } else if !missing && *word & bit != 0 {
            *word &= !bit;
            self.anchor_count -= 1;
        }
    }

    /// Поиск пути к опоре для узлов без прямого заземления.
    fn resolve(&mut self, st: &mut CollapseStats) {
        // Most discovered terrain is already certified by direct bedrock.
        // Revisit only chunks whose local nodes still need a graph path.
        self.reached.fill(true);
        self.anchor_candidates.clear();
        self.walk.clear();
        self.unsupported_chunks.clear();
        self.component_roots.resize(self.reached.len(), 0);
        for (wi, &word) in self.anchor_bits.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let i = wi * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if self.stale[i] {
                    continue;
                }
                let base = self.base[i] as usize;
                let nd = self.nodes[i].as_ref().expect("anchor cache");
                let overflow = if nd.n > 64 {
                    self.anchor_overflow
                        .get(&(i as u32))
                        .map_or(&[][..], Vec::as_slice)
                } else {
                    &[][..]
                };
                for lab in nd.without_direct_anchor(overflow) {
                    let id = base + lab as usize;
                    self.reached[id] = false;
                    // Root is needed only for unresolved nodes after this pass.
                    // Reuse it as the traversal mark while searching: MAX means
                    // unseen, MAX-1 means visited, and an unsupported component
                    // receives its real root once the search is exhausted.
                    self.component_roots[id] = u32::MAX;
                    self.anchor_candidates.push(AnchorCandidate {
                        chunk: i as u32,
                        lab,
                        id: id as u32,
                    });
                }
            }
        }
        st.nodes = st.nodes.max(self.reached.len());
        st.edges = st.edges.max(self.edge_count);

        // Bedrock and the temporary conservative anchors are already known.
        // Visit only the remaining connected components of this undirected
        // graph, instead of expanding every grounded terrain node each tick.
        'candidate: for &AnchorCandidate {
            chunk: ci,
            lab: start_lab,
            id: start,
        } in &self.anchor_candidates
        {
            let start = start as usize;
            if self.reached[start] || self.component_roots[start] != u32::MAX {
                continue;
            }
            // The common case is a single local component touching an
            // already certified neighbour. Certify it without setting up a
            // graph walk; only the remaining candidates need traversal.
            let nd = self.nodes[ci as usize].as_ref().expect("known");
            for &CachedLink {
                chunk: j, lab: b, ..
            } in nd.links_for(start_lab as usize)
            {
                if self.nodes[j as usize].is_some()
                    && self.reached[(self.base[j as usize] + u32::from(b)) as usize]
                {
                    self.reached[start] = true;
                    continue 'candidate;
                }
            }
            self.walk.clear();
            self.walk.push((ci, start_lab));
            self.component_roots[start] = u32::MAX - 1;
            let mut anchored = false;
            let mut head = 0;
            'search: while head < self.walk.len() {
                let (chunk, lab) = self.walk[head];
                head += 1;
                let i = chunk as usize;
                let nd = self.nodes[i].as_ref().expect("known");
                for &CachedLink {
                    chunk: j, lab: b, ..
                } in nd.links_for(lab as usize)
                {
                    if self.nodes[j as usize].is_none() {
                        continue;
                    }
                    let nid = (self.base[j as usize] + u32::from(b)) as usize;
                    if self.reached[nid] {
                        anchored = true;
                        // Every discovered node already has a path to
                        // this search root. One certified anchor proves
                        // them all; do not traverse the rest of the map.
                        break 'search;
                    } else if self.component_roots[nid] == u32::MAX {
                        self.component_roots[nid] = u32::MAX - 1;
                        self.walk.push((j, b));
                    }
                }
            }
            if anchored {
                for &(chunk, lab) in &self.walk {
                    self.reached[(self.base[chunk as usize] + u32::from(lab)) as usize] = true;
                }
            } else {
                // An exhausted search contains the entire unsupported
                // component of the undirected graph. Keep its minimum
                // global ID, exactly as the former union-find did.
                let root = self
                    .walk
                    .iter()
                    .map(|&(chunk, lab)| self.base[chunk as usize] + u32::from(lab))
                    .min()
                    .expect("search root");
                for &(chunk, lab) in &self.walk {
                    self.component_roots[(self.base[chunk as usize] + u32::from(lab)) as usize] =
                        root;
                    self.unsupported_chunks.push(chunk);
                }
            }
        }
        // Gathering must retain the original chunk/run/cell order, even if
        // searches reached chunks in a different order.
        self.unsupported_chunks.sort_unstable();
        self.unsupported_chunks.dedup();
    }

    #[cfg(test)]
    fn resolve_reference(&mut self, st: &mut CollapseStats) {
        self.reached.iter_mut().for_each(|v| *v = false);
        self.queue.clear();
        let mut edges = 0usize;
        for k in 0..self.known.len() {
            let i = self.known[k] as usize;
            let base = self.base[i];
            let nd = self.nodes[i].as_ref().expect("known");
            edges += nd.edges.iter().map(|e| e.len()).sum::<usize>();
            // Чанк с отставшей разметкой заземляет **всё своё**: его
            // компоненты посчитаны по старой карте, и решать по ним,
            // что́ отвалилось, нельзя. Оценка сверху.
            if self.stale[i] {
                for l in 0..nd.n {
                    let id = base + l as u32;
                    if !self.reached[id as usize] {
                        self.reached[id as usize] = true;
                        self.queue.push(id);
                    }
                }
            }
            for (l, &bed) in nd.bedrock.iter().enumerate() {
                if bed {
                    let id = base + l as u32;
                    if !self.reached[id as usize] {
                        self.reached[id as usize] = true;
                        self.queue.push(id);
                    }
                }
            }
            for f in 0..FACES {
                for &l in &nd.ground[f] {
                    let id = base + l as u32;
                    if !self.reached[id as usize] {
                        self.reached[id as usize] = true;
                        self.queue.push(id);
                    }
                }
            }
        }
        // Максимум по раундам, а не последнее значение: к концу тика
        // от графа остаётся то, что не упало, и размер его не о том.
        st.nodes = st.nodes.max(self.reached.len());
        st.edges = st.edges.max(edges);

        let mut head = 0usize;
        while head < self.queue.len() {
            let id = self.queue[head];
            head += 1;
            // Найти чанк по номеру узла: `known` отсортирован, `base`
            // монотонен, поэтому двоичный поиск.
            let k = match self
                .known
                .binary_search_by(|&c| self.base[c as usize].cmp(&id))
            {
                Ok(k) => k,
                Err(0) => continue,
                Err(k) => k - 1,
            };
            let i = self.known[k] as usize;
            let lab = (id - self.base[i]) as u16;
            debug_assert!((lab as usize) < self.nodes[i].as_ref().expect("known").n);
            for f in 0..FACES {
                let Some(j) = self.neighbour(i, f) else {
                    continue;
                };
                if self.nodes[j].is_none() {
                    continue;
                }
                let jbase = self.base[j];
                let nd = self.nodes[i].as_ref().expect("known");
                for &(a, b) in &nd.edges[f] {
                    if a != lab {
                        continue;
                    }
                    let nid = jbase + b as u32;
                    if !self.reached[nid as usize] {
                        self.reached[nid as usize] = true;
                        self.queue.push(nid);
                    }
                }
            }
        }
    }

    /// Оторвать всё, до чего обход не дошёл. Возвращает число тел.
    ///
    /// **Кусок, лежащий в нескольких чанках, — одно тело.** Компоненты
    /// в разных чанках, соединённые ребром графа, объединяются: иначе
    /// мост через границу чанка развалился бы на куски размером с чанк,
    /// а падают они как одно (ADR-119).
    fn detach_unanchored(
        &mut self,
        w: &mut World,
        bodies: &mut Bodies,
        st: &mut CollapseStats,
        held: &dyn Fn([u32; 3]) -> bool,
    ) -> usize {
        st.probed += self.reached.len();
        if self.unsupported_chunks.is_empty() {
            return 0;
        }

        // Состав групп. Обход по чанкам в порядке возрастания индекса,
        // внутри чанка — по пробегам: порядок вокселей детерминирован.
        let cs = CS as u32;
        let mut groups = BTreeMap::<u32, usize>::new();
        // None means the complete connected group exceeded the body cap.
        // Do not enumerate or retain the rest of a body that cannot be taken.
        let mut group_cells: Vec<Option<Vec<(u32, u32, u32)>>> = Vec::new();
        let mut group_held: Vec<bool> = Vec::new();
        let mut buffered = 0usize;
        for k in 0..self.unsupported_chunks.len() {
            let i = self.unsupported_chunks[k] as usize;
            let base = self.base[i] as usize;
            let (cx, cy, cz) = self.coords(i);
            let chunk_held = held([cx, cy, cz]);
            let (ox, oy, oz) = (cx * cs, cy * cs, cz * cs);
            let c = self.chunks[i].take().expect("chunk present");
            self.gathered += 1;
            self.comps.build(&c);
            self.chunks[i] = Some(c);
            for ri in 0..self.comps.runs.len() {
                let lab = self.comps.label[ri] as usize;
                if self.reached[base + lab] {
                    continue;
                }
                let root = self.component_roots[base + lab];
                let next = group_cells.len();
                let gi = *groups.entry(root).or_insert_with(|| {
                    group_cells.push(Some(Vec::new()));
                    group_held.push(false);
                    next
                });
                group_held[gi] |= chunk_held;
                let r = self.comps.runs[ri];
                let count = r.mask.count_ones() as usize;
                st.gather_candidates += count;
                let Some(cells) = group_cells[gi].as_mut() else {
                    continue;
                };
                if count > self.max_body_cells.saturating_sub(cells.len()) {
                    buffered -= cells.len();
                    group_cells[gi] = None;
                    continue;
                }
                let needed = cells.len() + count;
                if needed > cells.capacity() {
                    let capacity = needed.next_power_of_two().min(self.max_body_cells);
                    cells.reserve_exact(capacity - cells.len());
                }
                buffered += count;
                st.gather_buffer_peak = st.gather_buffer_peak.max(buffered);
                let col = r.col as usize;
                let (ly, lz) = (col % CS, col / CS);
                let mut bits = r.mask;
                while bits != 0 {
                    let x = bits.trailing_zeros();
                    bits &= bits - 1;
                    cells.push((ox + x, oy + ly as u32, oz + lz as u32));
                }
            }
        }

        let mut torn = 0usize;
        let mut touched: Vec<u32> = Vec::new();
        for (gi, group) in group_cells.iter().enumerate() {
            let Some(cells) = group else {
                st.too_big += 1;
                continue;
            };
            if cells.is_empty() {
                continue;
            }
            if group_held[gi] {
                st.held += 1;
                continue;
            }
            self.cells.clear();
            self.cells.extend_from_slice(cells);
            if !bodies.detach(w, &self.cells) {
                continue;
            }
            st.detached += 1;
            st.cells += self.cells.len();
            torn += 1;
            for k in 0..self.cells.len() {
                let (px, py, pz) = self.cells[k];
                let (bx, by, bz) = (px / cs, py / cs, pz / cs);
                let ci = self.index(bx, by, bz);
                if let Some(c) = self.chunks[ci].as_mut() {
                    c.set(
                        (px - bx * cs) as usize,
                        (py - by * cs) as usize,
                        (pz - bz * cs) as usize,
                        false,
                    );
                }
                if !touched.contains(&(ci as u32)) {
                    touched.push(ci as u32);
                }
            }
        }
        touched.sort_unstable();
        for &ci in &touched {
            if self.relabel(ci as usize) {
                self.mark_faces(ci as usize);
            }
        }
        torn
    }

    /// Пометить чанк как требующий пересборки: воксели тела вернулись
    /// в мир не правкой копки, а запеканием.
    /// Совпадает ли копия мира у каскада с миром, если показать ей
    /// ещё не показанные правки `unseen`.
    ///
    /// Каскад держит свою битовую карту каждого знакомого чанка и узнаёт
    /// о мире **только** из правок, которые ему показали: одна применённая
    /// и не показанная правка — и копия расходится навсегда, а подрезанное
    /// в ней держится (ADR-421). Проверка для тестов и диагностики, не для
    /// тика: она собирает каждый знакомый чанк из мира заново.
    pub fn agrees_with(&self, w: &World, unseen: &[Edit]) -> bool {
        let cs = CS as u32;
        self.chunks.iter().enumerate().all(|(i, c)| {
            let Some(c) = c else {
                return true;
            };
            let (cx, cy, cz) = self.coords(i);
            let mut mine = (**c).clone();
            for e in unseen {
                if (e.x / cs, e.y / cs, e.z / cs) == (cx, cy, cz) {
                    let (x, y, z) = (
                        (e.x % cs) as usize,
                        (e.y % cs) as usize,
                        (e.z % cs) as usize,
                    );
                    mine.set(x, y, z, e.mat != crate::store::AIR);
                }
            }
            mine == *Self::build(w, cx, cy, cz)
        })
    }

    pub fn invalidate_at(&mut self, x: u32, y: u32, z: u32) {
        let cs = CS as u32;
        let (cx, cy, cz) = (x / cs, y / cs, z / cs);
        if cx >= self.nx || cy >= self.ny || cz >= self.nz {
            return;
        }
        let i = self.index(cx, cy, cz);
        self.chunks[i] = None;
        if let Some(nd) = self.nodes[i].take() {
            self.edge_count -= nd.cached_edges;
        }
        let bit = 1u64 << (i % 64);
        let word = &mut self.anchor_bits[i / 64];
        if *word & bit != 0 {
            *word &= !bit;
            self.anchor_count -= 1;
        }
        self.anchor_overflow.remove(&(i as u32));
        if self.stale[i] {
            self.stale[i] = false;
            self.pending.retain(|&x| x as usize != i);
        }
    }

    /// Память графа компонент — она растёт с числом тронутых чанков
    /// и обязана быть видна.
    pub fn graph_bytes(&self) -> usize {
        let mut n = self.anchor_bits.capacity() * std::mem::size_of::<u64>()
            + self.anchor_candidates.capacity() * std::mem::size_of::<AnchorCandidate>();
        for nd in self.nodes.iter().flatten() {
            n += nd.faces.iter().map(|v| v.len() * 6).sum::<usize>() + nd.bedrock.len();
            n += nd.edges.iter().map(|e| e.len() * 4).sum::<usize>();
            n += nd.ground.iter().map(|g| g.len() * 2).sum::<usize>();
            n += nd.links.capacity() * std::mem::size_of::<CachedLink>();
            n += nd.link_starts.capacity() * std::mem::size_of::<u32>();
        }
        n += self
            .anchor_overflow
            .values()
            .map(|v| v.capacity() * std::mem::size_of::<u16>() + 48)
            .sum::<usize>();
        n
    }

    pub fn known_chunks(&self) -> usize {
        self.known.len()
    }
    /// Derived graph counters for headless performance diagnostics.
    pub fn graph_size(&self) -> [usize; 4] {
        [
            self.known.len(),
            self.reached.len(),
            self.anchor_count,
            self.edge_count,
        ]
    }

    /// Записать состояние каскада в буфер (ADR-229).
    ///
    /// **Пересобрать его из мира нельзя, и это замерено.** Битовая
    /// карта чанка отстаёт от мира ровно на перенесённые правки
    /// запекания (ADR-129), метки граней правятся без переразметки
    /// (`drop_removed_from_faces`), а очередь переразметки и фронтир
    /// втягивания — это отставание в тиках, то есть состояние
    /// (`CLAUDE.md`, ADR-089). Зонд на эталонном сценарии: пересборка
    /// каскада расходится с непрерывным прогоном на **первом же тике**.
    ///
    /// Не пишется то, что живёт внутри одного вызова `apply`:
    /// грязное множество, местные правки по чанкам, очередь обхода,
    /// метки эпохи, буферы проверки расщепления. Они очищаются
    /// в начале `apply` и до него не читаются.
    ///
    /// Потолки отвала, втягивания и переразметки, как и выключатель
    /// локальной проверки, **не пишутся**: это правила, а не состояние
    /// (ADR-140). Их ставит тот, кто собирает слой.
    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.u32(self.nx);
        b.u32(self.ny);
        b.u32(self.nz);
        b.bool(self.recheck);
        b.u32s(&self.pending);
        b.u32s(&self.frontier);
        b.u32s(&self.known);
        b.u32s(&self.base);
        b.bools(&self.stale);
        // Чанки — по возрастанию номера: порядок записи фиксирован.
        let touched: Vec<u32> = (0..self.chunks.len())
            .filter(|&i| self.chunks[i].is_some() || self.nodes[i].is_some())
            .map(|i| i as u32)
            .collect();
        b.u32(touched.len() as u32);
        for &ci in &touched {
            let i = ci as usize;
            b.u32(ci);
            match &self.chunks[i] {
                Some(c) => {
                    b.bool(true);
                    b.u32s(&c.cols);
                }
                None => b.bool(false),
            }
            match &self.nodes[i] {
                Some(nd) => {
                    b.bool(true);
                    b.usize(nd.n);
                    b.bools(&nd.bedrock);
                    for f in 0..FACES {
                        b.u32(nd.faces[f].len() as u32);
                        for &(a, l, lab) in &nd.faces[f] {
                            b.u16(a);
                            b.u16(l);
                            b.u16(lab);
                        }
                        b.u32(nd.edges[f].len() as u32);
                        for &(a, c) in &nd.edges[f] {
                            b.u16(a);
                            b.u16(c);
                        }
                        b.u16s(&nd.ground[f]);
                    }
                }
                None => b.bool(false),
            }
        }
    }

    pub fn load(&mut self, s: &mut fx::blob::Scan) -> bool {
        self.load_diagnostic(s).is_ok()
    }
    /// Rejection category for offline snapshot diagnostics.
    pub fn load_diagnostic(&mut self, s: &mut fx::blob::Scan) -> Result<(), &'static str> {
        if !s.ok() {
            return Err("collapse_prior_bytes");
        }
        if s.u32() != self.nx || s.u32() != self.ny || s.u32() != self.nz {
            return Err("collapse_dimensions");
        }
        self.recheck = s.bool();
        self.pending = s.u32s();
        self.frontier = s.u32s();
        self.known = s.u32s();
        self.base = s.u32s();
        self.stale = s.bools();
        if self.base.len() != self.chunks.len() || self.stale.len() != self.chunks.len() {
            return Err("collapse_array_lengths");
        }
        // Sparse snapshots omit absent chunks. A scene may already have an
        // authored support graph when restore starts; omitted entries must
        // replace that graph with absence, not preserve its old nodes/edges.
        self.chunks.fill_with(|| None);
        self.nodes.fill_with(|| None);
        let n = s.list(6);
        for _ in 0..n {
            let ci = s.u32() as usize;
            if ci >= self.chunks.len() {
                return Err("collapse_chunk_index");
            }
            self.chunks[ci] = if s.bool() {
                let cols = s.u32s();
                if cols.len() != crate::topo::COLS {
                    return Err("collapse_topology_length");
                }
                Some(Box::new(TopoChunk { cols }))
            } else {
                None
            };
            self.nodes[ci] = if s.bool() {
                let mut nd = Box::new(ChunkNodes::new());
                nd.n = s.usize();
                nd.bedrock = s.bools();
                if nd.bedrock.len() != nd.n {
                    return Err("collapse_bedrock_length");
                }
                for f in 0..FACES {
                    let nf = s.list(6);
                    nd.faces[f] = (0..nf).map(|_| (s.u16(), s.u16(), s.u16())).collect();
                    let ne = s.list(4);
                    nd.edges[f] = (0..ne).map(|_| (s.u16(), s.u16())).collect();
                    nd.ground[f] = s.u16s();
                }
                Some(nd)
            } else {
                None
            };
        }
        if !self.references_hold() {
            return Err("collapse_graph_references");
        }
        self.anchor_bits.fill(0);
        self.anchor_count = 0;
        self.anchor_overflow.clear();
        self.edge_count = 0;
        for i in 0..self.nodes.len() {
            if let Some(nd) = self.nodes[i].as_mut() {
                nd.cached_edges = 0;
                self.cache_nodes(i);
            }
        }
        // Достижимость — рабочий буфер обхода, но её длина задана
        // нумерацией узлов, а не обходом: восстанавливается по ней.
        let total: usize = self
            .known
            .iter()
            .filter_map(|&i| self.nodes[i as usize].as_ref())
            .map(|nd| nd.n)
            .sum();
        self.reached.clear();
        self.reached.resize(total, false);
        if s.ok() {
            Ok(())
        } else {
            Err("collapse_bytes")
        }
    }
}

impl CollapseLayer {
    /// Всё, что обход графа читает по номеру, указывает туда, где есть
    /// что прочитать (ADR-464): очередь переразметки и фронтир — чанки
    /// мира; размеченные — по возрастанию, у каждого узлы, и нумерация
    /// их узлов подряд, как её собирает `rebuild_index`; отрезки граней
    /// лежат в грани, их метки, метки опоры и рёбер — узлы своего чанка,
    /// второй конец ребра — узел соседа за этой гранью.
    fn references_hold(&self) -> bool {
        let chunks = self.chunks.len();
        let n = |i: usize| self.nodes[i].as_ref().map_or(0, |nd| nd.n);
        if self
            .pending
            .iter()
            .chain(&self.frontier)
            .chain(&self.known)
            .any(|&i| i as usize >= chunks)
            || self.known.windows(2).any(|p| p[0] >= p[1])
        {
            return false;
        }
        let mut total = 0u32;
        for &ci in &self.known {
            let i = ci as usize;
            if n(i) == 0 || self.base[i] != total {
                return false;
            }
            total = total.saturating_add(n(i) as u32);
        }
        (0..chunks).all(|i| {
            let Some(nd) = self.nodes[i].as_ref() else {
                return true;
            };
            let label = |l: u16| usize::from(l) < nd.n;
            (0..FACES).all(|f| {
                let far = self.neighbour(i, f).map_or(0, n);
                nd.faces[f].iter().all(|&(start, len, l)| {
                    usize::from(start) + usize::from(len) <= CS * CS && label(l)
                }) && nd.ground[f].iter().all(|&l| label(l))
                    && nd.edges[f]
                        .iter()
                        .all(|&(a, b)| label(a) && usize::from(b) < far)
            })
        })
    }
}

/// Поиск корня с укорачиванием пути. Свободная функция, потому что
/// внутри метода она заняла бы `self` целиком.
#[cfg(test)]
fn uf_find(p: &mut [u32], mut a: u32) -> u32 {
    while p[a as usize] != a {
        p[a as usize] = p[p[a as usize] as usize];
        a = p[a as usize];
    }
    a
}

/// Дописать ячейку в отрезки грани.
///
/// Индексы приходят **по возрастанию**: пробеги идут по колонкам
/// в порядке `z * CS + y`, а внутри колонки — по возрастанию `x`,
/// и раскладка каждой из шести граней устроена так, что этот порядок
/// её индексов и есть. Поэтому достаточно смотреть на последний
/// отрезок, а плотный черновик не нужен вовсе (ADR-127).
#[inline]
fn push_face(seg: &mut Vec<(u16, u16, u16)>, idx: usize, lab: u16) {
    match seg.last_mut() {
        Some(last) if last.2 == lab && (last.0 + last.1) as usize == idx => last.1 += 1,
        _ => seg.push((idx as u16, 1, lab)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bodies::test_gravity;
    use crate::store::{Mat, AIR};

    /// Independent former detachment algorithm: scan all known chunks and
    /// union their unsupported face edges. Also checks the sparse shortlist.
    fn assert_unsupported_groups(c: &CollapseLayer) {
        let mut parent: Vec<u32> = (0..c.reached.len() as u32).collect();
        let mut chunks = Vec::new();
        for &ci in &c.known {
            let i = ci as usize;
            let base = c.base[i];
            let nd = c.nodes[i].as_ref().unwrap();
            if c.reached[base as usize..base as usize + nd.n]
                .iter()
                .all(|&r| r)
            {
                continue;
            }
            chunks.push(ci);
            for f in 0..FACES {
                let Some(j) = c.neighbour(i, f) else {
                    continue;
                };
                if c.nodes[j].as_ref().is_none_or(|n| n.n == 0) {
                    continue;
                }
                for &(a, b) in &nd.edges[f] {
                    let (ia, ib) = (base + u32::from(a), c.base[j] + u32::from(b));
                    if !c.reached[ia as usize] && !c.reached[ib as usize] {
                        let (ra, rb) = (uf_find(&mut parent, ia), uf_find(&mut parent, ib));
                        parent[ra.max(rb) as usize] = ra.min(rb);
                    }
                }
            }
        }
        assert_eq!(c.unsupported_chunks, chunks);
        for (id, &anchored) in c.reached.iter().enumerate() {
            if !anchored {
                assert_eq!(c.component_roots[id], uf_find(&mut parent, id as u32));
            }
        }
    }

    #[test]
    fn copied_occupancy_matches_cells_across_words_edges_and_edits() {
        struct Pattern;
        impl crate::Generator for Pattern {
            fn material(&self, x: u32, y: u32, z: u32) -> Mat {
                if (x * 7 + y * 3 + z * 11) % 17 < 8 {
                    2
                } else {
                    AIR
                }
            }
        }
        let mut w = World::new(77, 45, 51, 1000, Box::new(Pattern));
        for edited in [false, true] {
            if edited {
                for x in [0, 31, 32, 63, 64, 76] {
                    w.queue_edit(Edit::put(x, 44, 50, 3));
                    w.queue_edit(Edit::dig(x, 0, 0));
                }
                w.flush();
            }
            for cz in 0..2 {
                for cy in 0..2 {
                    for cx in 0..3 {
                        let c = CollapseLayer::build(&w, cx, cy, cz);
                        for z in 0..CS {
                            for y in 0..CS {
                                for x in 0..CS {
                                    assert_eq!(
                                        c.get(x, y, z),
                                        w.solid(
                                            cx * 32 + x as u32,
                                            cy * 32 + y as u32,
                                            cz * 32 + z as u32
                                        )
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sparse_index_tracks_empty_invalidated_and_refilled_chunks() {
        let w = World::new(96, 64, 64, 1000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        for i in [7, 1, 10] {
            let mut ch = Box::new(TopoChunk::new());
            ch.set(1, 1, 1, true);
            c.chunks[i] = Some(ch);
            c.relabel(i);
        }
        c.rebuild_index();
        assert_eq!(c.known, vec![1, 7, 10]);
        assert_eq!([c.base[1], c.base[7], c.base[10]], [0, 1, 2]);
        c.chunks[7].as_mut().unwrap().set(1, 1, 1, false);
        c.relabel(7);
        let (x, y, z) = c.coords(1);
        c.invalidate_at(x * 32, y * 32, z * 32);
        c.rebuild_index();
        assert_eq!(c.known, vec![10]);
        c.chunks[7].as_mut().unwrap().set(2, 2, 2, true);
        c.relabel(7);
        c.rebuild_index();
        assert_eq!(c.known, vec![7, 10]);
        assert_eq!([c.base[7], c.base[10]], [0, 1]);
    }

    #[test]
    fn anchor_bits_keep_chunk_order_across_word_boundaries_and_invalidation() {
        let w = World::new(320, 64, 256, 1000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        for i in [0, 63, 64, 129, 159] {
            let mut nd = Box::new(ChunkNodes::new());
            nd.n = 1;
            nd.bedrock.push(false);
            c.nodes[i] = Some(nd);
            c.known.push(i as u32);
            c.cache_nodes(i);
        }
        assert_eq!(c.anchor_count, 5);
        for i in [63, 64] {
            c.nodes[i].as_mut().unwrap().bedrock[0] = true;
            c.cache_nodes(i);
        }
        let (x, y, z) = c.coords(129);
        c.invalidate_at(x * CS as u32, y * CS as u32, z * CS as u32);
        assert_eq!(c.anchor_count, 2);
        assert_eq!(c.anchor_bits[0] & 1, 1);
        assert_eq!(c.anchor_bits[2] & (1 << 31), 1 << 31);
        c.rebuild_index();
        c.resolve(&mut CollapseStats::default());
        assert_eq!(
            c.anchor_candidates
                .iter()
                .map(|v| v.chunk)
                .collect::<Vec<_>>(),
            [0, 159]
        );
        let reached = c.reached.clone();
        c.resolve_reference(&mut CollapseStats::default());
        assert_eq!(c.reached, reached);
    }

    /// **Испорченный снимок графа — отказ загрузки, а не паника обхода**
    /// (ADR-464). Граф моста с блоком на настиле сохраняется целым и с каждой
    /// порчей по отдельности: чанк за картой в очереди, фронтире или
    /// размеченных, размеченные не по возрастанию, сдвинутая нумерация,
    /// метка отрезка грани, опоры или ребра за узлами чанка, второй
    /// конец ребра за узлами соседа, отрезок за гранью.
    #[test]
    fn a_sparse_snapshot_replaces_an_already_populated_support_graph() {
        let mut w = World::new(96, 64, 64, 100_000, Box::new(Bridge));
        let mut target = CollapseLayer::new(&w);
        let mut bodies = Bodies::new(16, test_gravity());
        w.queue_edit(Edit::put(50, DECK + 1, 8, 2));
        for _ in 0..8 {
            w.flush();
            let edits = w.edits().to_vec();
            target.apply(&mut w, &edits, &mut bodies);
            bodies.step(&mut w);
        }
        assert!(target.nodes.iter().any(Option::is_some));
        assert!(target.chunks.iter().any(Option::is_some));
        let source = CollapseLayer::new(&w);
        let mut saved = fx::blob::Blob::new();
        source.save(&mut saved);
        assert!(target.load(&mut fx::blob::Scan::new(&saved.v)));
        let mut loaded = fx::blob::Blob::new();
        target.save(&mut loaded);
        assert_eq!(saved.v, loaded.v, "omitted chunks kept their scene graph");
        assert!(target.nodes.iter().all(Option::is_none));
        assert!(target.chunks.iter().all(Option::is_none));
        assert!(target.references_hold());
    }

    #[test]
    fn a_corrupt_graph_snapshot_is_refused() {
        let graph = || {
            let mut w = World::new(96, 64, 64, 100_000, Box::new(Bridge));
            let mut c = CollapseLayer::new(&w);
            let mut b = Bodies::new(16, test_gravity());
            // A block set on the deck: the graph takes in the chunks
            // around it, deck and pier with their faces and edges.
            w.queue_edit(Edit::put(50, DECK + 1, 8, 2));
            for _ in 0..8 {
                w.flush();
                let edits = w.edits().to_vec();
                c.apply(&mut w, &edits, &mut b);
                b.step(&mut w);
            }
            c
        };
        let w = World::new(96, 64, 64, 100_000, Box::new(Bridge));
        let c = graph();
        assert!(c.known.len() >= 2, "the fixture needs a graph");
        let (i, f) = (0..c.nodes.len())
            .flat_map(|i| (0..FACES).map(move |f| (i, f)))
            .find(|&(i, f)| {
                c.nodes[i]
                    .as_ref()
                    .is_some_and(|nd| !nd.edges[f].is_empty())
            })
            .expect("the fixture needs an edge");
        let reads = |c: &CollapseLayer| {
            let mut blob = fx::blob::Blob::new();
            c.save(&mut blob);
            CollapseLayer::new(&w).load(&mut fx::blob::Scan::new(&blob.v))
        };
        assert!(reads(&c));
        let chunks = c.chunks.len() as u32;
        fn node(c: &mut CollapseLayer, i: usize) -> &mut ChunkNodes {
            c.nodes[i].as_mut().unwrap()
        }
        type Spoil = Box<dyn Fn(&mut CollapseLayer)>;
        let spoils: Vec<(&str, Spoil)> = vec![
            ("pending", Box::new(move |c| c.pending.push(chunks))),
            ("frontier", Box::new(move |c| c.frontier.push(chunks))),
            ("known chunk", Box::new(move |c| c.known.push(chunks))),
            ("known order", Box::new(|c| c.known.swap(0, 1))),
            (
                "numbering",
                Box::new(|c| {
                    let k = c.known[1] as usize;
                    c.base[k] += 1;
                }),
            ),
            (
                "face label",
                Box::new(move |c| {
                    let n = node(c, i).n as u16;
                    node(c, i).faces[f].push((0, 1, n));
                }),
            ),
            (
                "face extent",
                Box::new(move |c| node(c, i).faces[f].push(((CS * CS) as u16, 1, 0))),
            ),
            (
                "ground label",
                Box::new(move |c| {
                    let n = node(c, i).n as u16;
                    node(c, i).ground[f].push(n);
                }),
            ),
            (
                "edge label",
                Box::new(move |c| {
                    let n = node(c, i).n as u16;
                    node(c, i).edges[f].push((n, 0));
                }),
            ),
            (
                "edge far label",
                Box::new(move |c| {
                    let j = c.neighbour(i, f).unwrap();
                    let far = c.nodes[j].as_ref().unwrap().n as u16;
                    node(c, i).edges[f].push((0, far));
                }),
            ),
        ];
        for (name, spoil) in &spoils {
            let mut bad = graph();
            spoil(&mut bad);
            assert!(!reads(&bad), "{name} was not refused");
        }
    }

    #[test]
    fn local_anchor_search_matches_full_flood_during_cuts_and_baking() {
        let mut w = World::new(96, 64, 64, 100_000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        let mut b = Bodies::new(16, test_gravity());
        for tick in 0..60 {
            if tick == 1 {
                cut(&mut w, 4);
            }
            if tick == 15 {
                w.queue_edit(Edit::put(8, 4, 8, 2));
            }
            if tick == 20 {
                w.queue_edit(Edit::put(80, 50, 30, 3));
            }
            w.flush();
            let edits = w.edits().to_vec();
            c.apply(&mut w, &edits, &mut b);
            let mut fast = CollapseStats::default();
            c.resolve(&mut fast);
            let expected = c.reached.clone();
            let mut reference = CollapseStats::default();
            c.resolve_reference(&mut reference);
            assert_eq!(c.reached, expected, "anchor query at tick {tick}");
            assert_eq!((fast.nodes, fast.edges), (reference.nodes, reference.edges));
            assert_unsupported_groups(&c);
            b.step(&mut w);
        }
    }

    #[test]
    fn anchor_search_matches_full_flood_on_graphs_with_cycles_and_stale_nodes() {
        let w = World::new(96, 96, 96, 1000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        let mut seed = 72u32;
        let mut draw = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            seed >> 16
        };
        for round in 0..256 {
            c.known.clear();
            c.anchor_bits.fill(0);
            c.anchor_count = 0;
            c.anchor_overflow.clear();
            c.edge_count = 0;
            for i in 0..c.nodes.len() {
                let mut nd = Box::new(ChunkNodes::new());
                nd.n = (draw() % 4 + 1) as usize;
                nd.bedrock = (0..nd.n).map(|_| draw() % 11 == 0).collect();
                if draw() % 5 == 0 {
                    nd.ground[0].push((draw() as usize % nd.n) as u16);
                }
                c.nodes[i] = Some(nd);
                c.known.push(i as u32);
                c.stale[i] = draw() % 13 == 0;
            }
            for i in 0..c.nodes.len() {
                for f in [1, 3, 5] {
                    let Some(j) = c.neighbour(i, f) else { continue };
                    for a in 0..c.nodes[i].as_ref().unwrap().n {
                        for b in 0..c.nodes[j].as_ref().unwrap().n {
                            if draw() % 4 == 0 {
                                c.nodes[i].as_mut().unwrap().edges[f].push((a as u16, b as u16));
                                c.nodes[j].as_mut().unwrap().edges[opposite(f)]
                                    .push((b as u16, a as u16));
                            }
                        }
                    }
                }
            }
            // Exercise complete unsupported islands (including cycles),
            // fully anchored graphs and mixed conservative anchors.
            if round % 8 <= 1 {
                for i in 0..c.nodes.len() {
                    let nd = c.nodes[i].as_mut().unwrap();
                    nd.bedrock.fill(round % 8 == 1);
                    for ground in &mut nd.ground {
                        ground.clear();
                    }
                    c.stale[i] = false;
                }
            }
            c.rebuild_index();
            for i in 0..c.nodes.len() {
                // Production face refresh stores pairs in this order.
                for edges in &mut c.nodes[i].as_mut().unwrap().edges {
                    edges.sort_unstable();
                }
                c.cache_nodes(i);
            }
            let mut fast = CollapseStats::default();
            c.resolve(&mut fast);
            let reached = c.reached.clone();
            let mut reference = CollapseStats::default();
            c.resolve_reference(&mut reference);
            assert_eq!(c.reached, reached, "graph {round}");
            assert_eq!((fast.nodes, fast.edges), (reference.nodes, reference.edges));
            assert_unsupported_groups(&c);
            let mut blob = fx::blob::Blob::new();
            c.save(&mut blob);
            let mut restored = CollapseLayer::new(&w);
            assert!(restored.load(&mut fx::blob::Scan::new(&blob.v)));
            restored.resolve(&mut CollapseStats::default());
            assert_eq!(restored.reached, c.reached, "restored graph {round}");
            assert_unsupported_groups(&restored);
            let mut after = fx::blob::Blob::new();
            restored.save(&mut after);
            assert_eq!(blob.v, after.v, "derived groups are not serialized");
        }
    }

    #[test]
    fn flattened_links_keep_face_and_edge_order_with_interleaved_labels() {
        let w = World::new(96, 96, 96, 1000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        let i = c.index(1, 1, 1);
        let left = c.neighbour(i, 0).unwrap() as u32;
        let right = c.neighbour(i, 1).unwrap() as u32;
        let mut nd = Box::new(ChunkNodes::new());
        nd.n = 2;
        nd.bedrock = vec![false; 2];
        nd.edges[0] = vec![(1, 9), (0, 8), (1, 7)];
        nd.edges[1] = vec![(0, 4), (1, 5)];
        c.nodes[i] = Some(nd);
        c.cache_nodes(i);
        let nd = c.nodes[i].as_ref().unwrap();
        assert_eq!(nd.link_starts, [0, 2, 5]);
        let links = |lab| {
            nd.links_for(lab)
                .iter()
                .map(|link| (link.chunk, link.lab))
                .collect::<Vec<_>>()
        };
        assert_eq!(links(0), [(left, 8), (right, 4)]);
        assert_eq!(links(1), [(left, 9), (left, 7), (right, 5)]);
    }

    #[test]
    fn derived_anchors_cover_labels_beyond_the_inline_mask_after_loading() {
        let w = World::new(32, 32, 32, 1000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        let mut nd = Box::new(ChunkNodes::new());
        nd.n = 70;
        nd.bedrock = vec![false; nd.n];
        nd.bedrock[3] = true;
        nd.ground[0].push(68);
        c.nodes[0] = Some(nd);
        c.known.push(0);
        c.rebuild_index();
        c.cache_nodes(0);
        assert_eq!(c.anchor_count, 1);
        assert_eq!(
            c.anchor_overflow.get(&0).unwrap().as_slice(),
            &[64, 65, 66, 67, 69]
        );
        c.resolve(&mut CollapseStats::default());
        let expected = c.reached.clone();
        let mut reference = CollapseStats::default();
        c.resolve_reference(&mut reference);
        assert_eq!(c.reached, expected);
        assert_unsupported_groups(&c);
        let mut bytes = fx::blob::Blob::new();
        c.save(&mut bytes);
        let mut restored = CollapseLayer::new(&w);
        assert!(restored.load(&mut fx::blob::Scan::new(&bytes.v)));
        assert_eq!(restored.graph_size(), c.graph_size());
        restored.resolve(&mut CollapseStats::default());
        assert_eq!(restored.reached, expected);
        let mut check = fx::blob::Blob::new();
        restored.save(&mut check);
        assert_eq!(check.v, bytes.v);
    }

    /// Мост: столб от дна до `DECK`, от него настил поперёк всей карты.
    /// Настил заведомо длиннее чанка — в этом весь смысл.
    struct Bridge;
    const DECK: u32 = 40;
    const PIER: u32 = 8;

    impl crate::Generator for Bridge {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if z != 8 {
                return AIR;
            }
            // Опора.
            if x == PIER && y < DECK {
                return 2;
            }
            // Настил.
            if y == DECK && (PIER..60).contains(&x) {
                return 2;
            }
            AIR
        }
    }

    fn cut(w: &mut World, y: u32) {
        w.queue_dig(PIER, y, 8);
    }

    /// **Кусок, уходящий за границу чанка, падает.** Настил длиной
    /// пятьдесят два воксела лежит в двух чанках; до ступени C каждая
    /// половина считалась заякоренной за другую, и мост не обрушивался
    /// никогда (Q-66).
    #[test]
    fn a_span_crossing_a_chunk_boundary_falls() {
        let mut w = World::new(64, 64, 64, 100_000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        let mut b = Bodies::new(16, test_gravity());

        // Подрезать опору у основания.
        cut(&mut w, 4);
        w.flush();
        let edits: Vec<Edit> = w.edits().to_vec();
        let st = c.apply(&mut w, &edits, &mut b);
        assert!(st.detached > 0, "мост не обрушился");
        assert_eq!(
            st.detached, 1,
            "мост обязан упасть одним телом, а не по чанкам"
        );
        assert!(st.nodes >= 3, "граф слишком мал: {st:?}");
        assert!(st.promoted >= 1, "соседей не втянули: {st:?}");
        w.flush();

        // Настил ушёл из мира по обе стороны границы чанка.
        assert!(!w.solid(20, DECK, 8), "левая половина осталась");
        assert!(!w.solid(50, DECK, 8), "правая половина осталась");
        // Основание опоры ниже подреза стоит.
        assert!(w.solid(PIER, 2, 8), "низ опоры пропал");
    }

    /// **A cut on a chunk's face drops what stood past it** (ADR-460).
    /// Cut at the top row of its chunk, the pier leaves nothing touching
    /// the chunk above: that chunk was taken in only as the neighbour of
    /// a component in contact with it, and the pier's top with the deck
    /// hung in the air for ever. The neighbours of an edited chunk are
    /// taken in; past them, only what has no path to bedrock pulls more.
    #[test]
    fn a_cut_on_a_chunk_face_drops_what_stood_past_it() {
        let mut w = World::new(64, 64, 64, 100_000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        let mut b = Bodies::new(16, test_gravity());
        cut(&mut w, CS as u32 - 1);
        w.flush();
        let mut edits: Vec<Edit> = w.edits().to_vec();
        let mut detached = 0;
        for _ in 0..4 {
            detached += c.apply(&mut w, &edits, &mut b).detached;
            edits.clear();
            if !c.pending() {
                break;
            }
        }
        assert_eq!(detached, 1, "what stood past the cut stayed up");
        w.flush();
        assert!(!w.solid(PIER, DECK, 8), "the pier's top stayed");
        assert!(!w.solid(50, DECK, 8), "the deck stayed");
        assert!(
            w.solid(PIER, CS as u32 - 2, 8),
            "the pier under the cut went"
        );
    }

    /// **Terrain standing on bedrock pulls nothing in** (ADR-460). Every
    /// component in contact with an unknown chunk used to take it in, so
    /// one edit in solid ground flooded the whole of it into the graph at
    /// the promotion cap a tick, for as long as there was ground left.
    #[test]
    fn an_edit_in_solid_ground_does_not_flood_the_graph() {
        struct Solid;
        impl crate::Generator for Solid {
            fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
                if y < 40 {
                    2
                } else {
                    AIR
                }
            }
        }
        let mut w = World::new(256, 64, 256, 1_000_000, Box::new(Solid));
        let mut c = CollapseLayer::new(&w);
        c.max_promote_per_tick = 8;
        let mut b = Bodies::new(16, test_gravity());
        w.queue_dig(128, 39, 128);
        w.flush();
        let mut edits: Vec<Edit> = w.edits().to_vec();
        for _ in 0..64 {
            c.apply(&mut w, &edits, &mut b);
            edits.clear();
            if !c.pending() {
                break;
            }
        }
        assert!(!c.pending(), "the graph is still being taken in");
        // The edited chunk, its six neighbours and what they stand on.
        assert!(
            c.known.len() <= 16,
            "the graph flooded: {} chunks",
            c.known.len()
        );
        assert!(b.list.is_empty());
    }

    #[test]
    fn rejected_large_groups_do_not_buffer_their_whole_volume_or_block_small_bodies() {
        struct Slab;
        impl crate::Generator for Slab {
            fn material(&self, x: u32, y: u32, z: u32) -> Mat {
                if ((32..288).contains(&x) && (32..48).contains(&y) && (32..288).contains(&z))
                    || ((8..10).contains(&x) && (10..12).contains(&y) && (8..10).contains(&z))
                {
                    2
                } else {
                    AIR
                }
            }
        }
        let mut w = World::new(320, 96, 320, 100_000, Box::new(Slab));
        let mut c = CollapseLayer::new(&w);
        c.set_max_body_cells(4096);
        let mut b = Bodies::new(16, test_gravity());
        w.queue_dig(32, 32, 32);
        w.queue_dig(8, 10, 8);
        w.flush();
        let mut edits = w.edits().to_vec();
        let mut rejected = 0;
        let mut candidates = 0;
        for _ in 0..64 {
            let st = c.apply_iter_observed(&mut w, edits.iter().copied(), &mut b, |_| {});
            rejected += st.too_big;
            candidates = candidates.max(st.gather_candidates);
            assert!(st.gather_buffer_peak <= 4096 + 7, "{st:?}");
            edits.clear();
            if !c.pending() {
                break;
            }
        }
        assert!(rejected > 0);
        assert!(candidates >= 256 * 256 * 16 - 1);
        assert!(!c.pending());
        assert_eq!(b.list.len(), 1);
        assert_eq!(b.list[0].voxels.len(), 7);
        w.flush();
        assert!(
            w.solid(128, 40, 128),
            "rejected matter must remain in the world"
        );
        assert!(
            !w.solid(9, 11, 9),
            "small disconnected body must still detach"
        );
    }

    #[test]
    fn streamed_edits_preserve_complete_cross_chunk_body_and_snapshot_order() {
        let mut a = World::new(64, 64, 64, 100_000, Box::new(Bridge));
        let mut b = World::new(64, 64, 64, 100_000, Box::new(Bridge));
        let mut ca = CollapseLayer::new(&a);
        let mut cb = CollapseLayer::new(&b);
        let mut ba = Bodies::new(16, test_gravity());
        let mut bb = Bodies::new(16, test_gravity());
        cut(&mut a, 4);
        cut(&mut b, 4);
        a.flush();
        b.flush();
        let edits = a.edits().to_vec();
        ca.apply(&mut a, &edits, &mut ba);
        cb.apply_iter_observed(&mut b, edits.into_iter(), &mut bb, |_| {});
        assert_eq!(ba.list.len(), 1);
        for _ in 0..8 {
            let mut sa = fx::blob::Blob::new();
            let mut sb = fx::blob::Blob::new();
            ca.save(&mut sa);
            ba.save(&mut sa);
            a.save(&mut sa);
            a.save_transfers(&mut sa);
            cb.save(&mut sb);
            bb.save(&mut sb);
            b.save(&mut sb);
            b.save_transfers(&mut sb);
            assert_eq!(sa.v, sb.v);
            a.flush();
            b.flush();
            ca.apply(&mut a, &[], &mut ba);
            cb.apply_iter_observed(&mut b, std::iter::empty(), &mut bb, |_| {});
            ba.step(&mut a);
            bb.step(&mut b);
        }
    }

    /// **A piece too big to fall waits for a change** (ADR-440). Only an
    /// edit near it or a larger cap changes its answer; asking again
    /// every tick is a cap firing every tick (ADR-116).
    #[test]
    fn a_piece_too_big_to_fall_waits_for_a_change() {
        let mut w = World::new(64, 64, 64, 100_000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        c.set_max_body_cells(8);
        let mut b = Bodies::new(16, test_gravity());
        cut(&mut w, 4);
        w.flush();
        let mut edits: Vec<Edit> = w.edits().to_vec();
        let mut too_big = 0;
        for _ in 0..8 {
            too_big += c.apply(&mut w, &edits, &mut b).too_big;
            edits.clear();
            if !c.pending() {
                break;
            }
        }
        assert!(too_big > 0, "the deck fitted under the cap");
        assert!(b.list.is_empty());
        assert!(!c.pending(), "the cascade asks about the deck every tick");
        // A larger cap lets it fall without an edit.
        c.set_max_body_cells(MAX_BODY_CELLS);
        assert!(c.pending());
        assert_eq!(c.apply(&mut w, &[], &mut b).detached, 1);
    }

    /// **Заякоренное не падает.** Тот же мост без подреза стоит,
    /// сколько бы правок ни прошло рядом.
    #[test]
    fn an_anchored_span_stays() {
        let mut w = World::new(64, 64, 64, 100_000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        let mut b = Bodies::new(16, test_gravity());
        // Правка рядом с мостом, но не по нему.
        w.queue_dig(20, 10, 8);
        w.flush();
        let edits: Vec<Edit> = w.edits().to_vec();
        let st = c.apply(&mut w, &edits, &mut b);
        assert_eq!(st.detached, 0, "мост упал сам по себе");
        w.flush();
        assert!(w.solid(50, DECK, 8), "настил исчез без причины");
    }

    /// **Цепочка чанков разрешается за один тик.** Настил держится
    /// за опору, опора — за коренную породу; убрав основание, падает
    /// вся цепочка, а не только тот чанк, где была правка.
    #[test]
    fn the_whole_chain_falls_not_just_the_edited_chunk() {
        let mut w = World::new(64, 64, 64, 100_000, Box::new(Bridge));
        let mut c = CollapseLayer::new(&w);
        let mut b = Bodies::new(16, test_gravity());
        cut(&mut w, 4);
        w.flush();
        let edits: Vec<Edit> = w.edits().to_vec();
        let st = c.apply(&mut w, &edits, &mut b);
        // Опора выше подреза плюс весь настил — одно тело.
        let expect = (DECK - 5) as usize + (60 - PIER) as usize;
        assert_eq!(
            st.cells, expect,
            "состав тела: {} против {expect}",
            st.cells
        );
        // Разбор будет повторён следующим тиком: в этом оторвалось.
        assert!(st.rounds_capped);
    }

    /// Каскад детерминирован: тот же вход — тот же состав тел.
    #[test]
    fn the_cascade_is_deterministic_across_chunks() {
        let run = || -> Vec<(i64, i64, i64, usize)> {
            let mut w = World::new(64, 64, 64, 100_000, Box::new(Bridge));
            let mut c = CollapseLayer::new(&w);
            let mut b = Bodies::new(16, test_gravity());
            cut(&mut w, 4);
            w.flush();
            let edits: Vec<Edit> = w.edits().to_vec();
            c.apply(&mut w, &edits, &mut b);
            b.list
                .iter()
                .map(|x| (x.ox, x.oy, x.oz, x.voxels.len()))
                .collect()
        };
        assert_eq!(run(), run());
    }
}

/// Убрать одну ячейку из отрезков грани. Отрезок при этом может
/// распасться надвое.
fn drop_face_cell(seg: &mut Vec<(u16, u16, u16)>, idx: u16) -> bool {
    let Some(k) = seg.iter().position(|&(s, l, _)| idx >= s && idx < s + l) else {
        return false;
    };
    let (s0, l0, lab) = seg[k];
    let head = idx - s0;
    let tail = s0 + l0 - idx - 1;
    match (head, tail) {
        (0, 0) => {
            seg.remove(k);
        }
        (0, _) => seg[k] = (idx + 1, tail, lab),
        (_, 0) => seg[k] = (s0, head, lab),
        _ => {
            seg[k] = (s0, head, lab);
            seg.insert(k + 1, (idx + 1, tail, lab));
        }
    }
    true
}

#[cfg(test)]
mod probe_equivalence {
    use super::*;
    use crate::bodies::test_gravity;
    use crate::store::{Mat, AIR};
    use fx::rng::Rng;

    /// Слоистая порода с пустотами — чтобы было чему отваливаться.
    struct Layers;
    impl crate::Generator for Layers {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if y < 8 {
                return 2;
            }
            // Столбы и перекрытия: подкоп столба роняет перекрытие.
            let pillar = x % 12 < 3 && z % 12 < 3 && y < 44;
            let deck = (40..44).contains(&y);
            if pillar || deck {
                3
            } else {
                AIR
            }
        }
    }

    /// Один и тот же прогон: тела, их состав и порядок.
    fn run(probe: bool, ticks: u32) -> (Vec<(i64, i64, i64, usize)>, usize) {
        let mut w = World::new(64, 64, 64, 100_000, Box::new(Layers));
        let mut c = CollapseLayer::new(&w);
        c.use_split_probe = probe;
        let mut b = Bodies::new(64, test_gravity());
        let mut r = Rng::new(7, 0xC0DE);
        let mut trace = Vec::new();
        let mut fired = 0usize;
        for _ in 0..ticks {
            // Копка: случайный шар в породе.
            let (cx, cy, cz) = (r.below(64), 8 + r.below(36), r.below(64));
            for dz in -2i64..=2 {
                for dy in -2i64..=2 {
                    for dx in -2i64..=2 {
                        if dx * dx + dy * dy + dz * dz > 4 {
                            continue;
                        }
                        let (x, y, z) = (cx as i64 + dx, cy as i64 + dy, cz as i64 + dz);
                        if !(0..64).contains(&x) || !(1..64).contains(&y) || !(0..64).contains(&z) {
                            continue;
                        }
                        if w.solid(x as u32, y as u32, z as u32) {
                            w.queue_dig(x as u32, y as u32, z as u32);
                        }
                    }
                }
            }
            w.flush();
            let edits: Vec<Edit> = w.edits().to_vec();
            fired += c.apply(&mut w, &edits, &mut b).probe_skipped;
            w.flush();
            for x in b.list.iter() {
                trace.push((x.ox, x.oy, x.oz, x.voxels.len()));
            }
            b.step(&mut w);
            w.flush();
        }
        (trace, fired)
    }

    /// Чанк (0, 1, 0) с двумя компонентами: нарост P в два воксела на его
    /// нижней грани, стоящий на породе соседнего чанка, и столб Q от
    /// породы. P раньше в порядке обхода и получает метку 0.
    struct NubAndPillar;
    impl crate::Generator for NubAndPillar {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if y < 32 {
                return 2;
            }
            let nub = (x, z) == (5, 5) && y < 34;
            let pillar = (x, z) == (20, 20) && y < 46;
            if nub || pillar {
                3
            } else {
                AIR
            }
        }
    }

    /// **Исчезнувшая компонента не переименовывает соседнюю** (ADR-422).
    ///
    /// Проверка расщепления пропускала переразметку, если оболочка
    /// снятого пуста, — а она пуста и тогда, когда снята компонента
    /// целиком. Сбор отрыва строил метки заново: столб Q становился
    /// меткой 0 и читал «не дошли» у пропавшего нароста P — стоящий
    /// на породе столб отрывался телом.
    #[test]
    fn a_vanished_component_does_not_rename_its_neighbour() {
        let mut w = World::new(64, 96, 64, 100_000, Box::new(NubAndPillar));
        let mut c = CollapseLayer::new(&w);
        let mut b = Bodies::new(64, test_gravity());
        // Знакомим каскад с чанком: наращиваем столб на воксель.
        w.queue_edit(Edit::put(20, 46, 20, 3));
        w.flush();
        let edits: Vec<Edit> = w.edits().to_vec();
        c.apply(&mut w, &edits, &mut b);
        w.flush();
        assert!(b.list.is_empty(), "стоящее оторвалось до подкопа");
        // Снимаем нарост целиком.
        w.queue_dig(5, 32, 5);
        w.queue_dig(5, 33, 5);
        w.flush();
        let edits: Vec<Edit> = w.edits().to_vec();
        for _ in 0..8 {
            c.apply(&mut w, &edits, &mut b);
            w.flush();
        }
        assert!(
            b.list.is_empty(),
            "столб на породе оторван телом из {} вокселей",
            b.list.iter().map(|x| x.voxels.len()).sum::<usize>()
        );
        assert!(w.solid(20, 40, 20), "столб пропал из мира");
    }

    /// Полка у стены мира: висит в воздухе и держится только рамкой
    /// x = 0.
    struct Ledge;
    impl crate::Generator for Ledge {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            if y < 32 {
                return 2;
            }
            if x < 6 && (20..24).contains(&z) && (40..42).contains(&y) {
                3
            } else {
                AIR
            }
        }
    }

    /// **Рамка мира держит то, что её касается** (ADR-433). Рамку нельзя
    /// ни снять, ни застроить (ADR-034). Каскад отрывал полку у стены
    /// мира телом, правки её клеток на рамке отбрасывались, и тело ждало
    /// их вечно — а на полигоне с ним вставал весь каскад.
    #[test]
    fn the_map_frame_holds_what_touches_it() {
        let mut w = World::new(64, 96, 64, 100_000, Box::new(Ledge));
        let mut c = CollapseLayer::new(&w);
        let mut b = Bodies::new(64, test_gravity());
        w.queue_dig(5, 40, 21);
        w.flush();
        let edits: Vec<Edit> = w.edits().to_vec();
        let mut gathered = usize::MAX;
        for _ in 0..8 {
            gathered = c.apply(&mut w, &edits, &mut b).gathered;
            w.flush();
        }
        assert_eq!(
            gathered, 0,
            "the collapse still takes the ledge for unsupported"
        );
        assert!(
            b.list.is_empty(),
            "the ledge on the frame fell as a body of {} cells",
            b.list.iter().map(|x| x.voxels.len()).sum::<usize>()
        );
        assert!(
            w.solid(0, 40, 21) && w.solid(3, 41, 22),
            "the ledge left the world"
        );
        // Nor does anyone else detach a frame cell: its dig would never land.
        assert!(!b.detach(&mut w, &[(0, 40, 21), (1, 40, 21)]));
        assert!(w.solid(1, 40, 21));
    }

    /// **Каскад с локальной проверкой и без обязан дать одно и то же.**
    ///
    /// Проверка достаточная, значит она не имеет права ни пропустить
    /// отвал, ни выдумать его. Разойдутся — значит условие не
    /// достаточное, и вся экономия неверна.
    ///
    /// Срабатывание проверки — часть самого теста: сравнение, в котором
    /// она ни разу не сказала «да», ничего не сравнило.
    #[test]
    fn the_probe_does_not_change_what_falls() {
        let (with, fired) = run(true, 120);
        let (without, none) = run(false, 120);
        assert!(
            fired > 0,
            "проверка не сработала ни разу — сравнивать нечего"
        );
        assert_eq!(none, 0, "проверка сработала при выключенной");
        assert_eq!(with, without);
    }
}
