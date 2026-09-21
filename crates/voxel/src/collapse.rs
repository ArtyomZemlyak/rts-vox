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
    /// Чанков собрано из мира целиком — 32 768 чтений на каждый.
    pub built: usize,
    /// Чанков, которым переразметка не понадобилась: локальная
    /// проверка доказала, что пачка снятий ничего не расщепила.
    pub probe_skipped: usize,
}

/// Компоненты одного чанка, как их видит граф.
struct ChunkNodes {
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
            n: 0,
            faces: Default::default(),
            bedrock: Vec::new(),
            edges: Default::default(),
            ground: Default::default(),
        }
    }
}

pub struct CollapseLayer {
    comps: Components,
    chunks: Vec<Option<Box<TopoChunk>>>,
    nx: u32,
    ny: u32,
    nz: u32,
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
    queue: Vec<u32>,
    /// Грани, которые надо пересчитать: `(чанк, грань)`.
    face_todo: Vec<(u32, u8)>,
    /// Чанки, до которых достаёт контакт, но которые ещё не размечены.
    /// Пока чанк здесь, контакт с ним считается заземляющим.
    frontier: Vec<u32>,
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
    pub fn set_max_body_cells(&mut self, n: usize) {
        self.max_body_cells = n.max(1);
    }

    pub fn new(w: &World) -> CollapseLayer {
        let cs = CS as u32;
        let (nx, ny, nz) = (w.nx.div_ceil(cs), w.ny.div_ceil(cs), w.nz.div_ceil(cs));
        let n = (nx * ny * nz) as usize;
        CollapseLayer {
            comps: Components::new(),
            chunks: (0..n).map(|_| None).collect(),
            nx,
            ny,
            nz,
            dirty: Vec::new(),
            stamp: vec![u32::MAX; n],
            epoch: 0,
            max_body_cells: MAX_BODY_CELLS,
            local: (0..n).map(|_| Vec::new()).collect(),
            nodes: (0..n).map(|_| None).collect(),
            base: vec![0; n],
            known: Vec::new(),
            reached: Vec::new(),
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

    /// Собрать чанк из мира целиком: 32 768 чтений. Зовётся при первом
    /// касании и при втягивании в граф, и это не дёшево.
    fn build(w: &World, cx: u32, cy: u32, cz: u32) -> Box<TopoChunk> {
        let cs = CS as u32;
        let (ox, oy, oz) = (cx * cs, cy * cs, cz * cs);
        let mut c = Box::new(TopoChunk::new());
        for z in 0..cs {
            for y in 0..cs {
                for x in 0..cs {
                    if w.solid(ox + x, oy + y, oz + z) {
                        c.set(x as usize, y as usize, z as usize, true);
                    }
                }
            }
        }
        c
    }

    /// Прогнать правки тика через каскад и оторвать всё, что отвалилось.
    pub fn apply(&mut self, w: &mut World, edits: &[Edit], bodies: &mut Bodies) -> CollapseStats {
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
            let skip = self.use_split_probe
                && only_removals
                && !bottom_row
                && !self.removed.is_empty()
                && self.nodes[i].is_some()
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
        st.promoted = self.promote(w);
        st.frontier = self.frontier.len();

        let refused_before = bodies.refused;
        let mut torn_total = 0usize;
        for round in 0..MAX_ROUNDS {
            st.rounds = round + 1;
            self.resolve(&mut st);
            let torn = self.detach_unanchored(w, bodies, &mut st);
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
        self.recheck = torn_total > 0 || st.too_big > 0 || bodies.refused > refused_before;
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
        let mut done = 0usize;
        // Втягивание идёт **пачками**, а не по чанку.
        //
        // Первая редакция пересчитывала грани после каждого чанка,
        // и цена выходила квадратичной по числу втянутых: на реальной
        // копке это дало 2.97 мс за тик (ADR-126). Новые чанки фронтира
        // находятся при пересчёте граней, поэтому раунды нужны, но их
        // единицы, а не десятки.
        while done < self.max_promote_per_tick {
            self.frontier.sort_unstable();
            self.frontier.dedup();
            let batch: Vec<u32> = self
                .frontier
                .iter()
                .copied()
                .filter(|&i| self.nodes[i as usize].is_none())
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
            self.frontier.retain(|i| !batch.contains(i));
            self.rebuild_index();
            self.refresh_faces(w);
        }
        // Всё, что уже в графе, с фронтира уходит.
        self.frontier.retain(|&i| self.nodes[i as usize].is_none());
        done
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
        let (_, cy, _) = self.coords(i);
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
        }
        let changed = (0..FACES).any(|f| nd.faces[f] != self.face_prev[f]);
        self.nodes[i] = Some(nd);
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
            Some(j) => {
                // Сосед ещё не в графе. Контакт с ним считается
                // заземляющим — это оценка сверху, — а сам он ставится
                // в очередь на втягивание.
                let mut contact = false;
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
                            contact = true;
                            break;
                        }
                    }
                }
                nd.ground[f].sort_unstable();
                if contact && !self.frontier.contains(&(j as u32)) {
                    self.frontier.push(j as u32);
                }
            }
        }
        self.nodes[i] = Some(nd);
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
        self.known.clear();
        let mut total = 0u32;
        for i in 0..self.nodes.len() {
            if let Some(nd) = &self.nodes[i] {
                // Пустой чанк узлов не даёт. Он обязан выпасть из
                // нумерации: два чанка с одинаковой базой сделали бы
                // двоичный поиск по ней неоднозначным.
                if nd.n == 0 {
                    continue;
                }
                self.base[i] = total;
                total += nd.n as u32;
                self.known.push(i as u32);
            }
        }
        self.reached.clear();
        self.reached.resize(total as usize, false);
    }

    /// Обход графа от заземлённых узлов.
    fn resolve(&mut self, st: &mut CollapseStats) {
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
    ) -> usize {
        for k in 0..self.known.len() {
            let i = self.known[k] as usize;
            st.probed += self.nodes[i].as_ref().expect("known").n;
        }
        if self.reached.iter().all(|&r| r) {
            return 0;
        }

        // Объединение незаякоренных узлов по рёбрам графа.
        let n_nodes = self.reached.len();
        let mut parent: Vec<u32> = (0..n_nodes as u32).collect();
        for k in 0..self.known.len() {
            let i = self.known[k] as usize;
            let base = self.base[i];
            for f in 0..FACES {
                let Some(j) = self.neighbour(i, f) else {
                    continue;
                };
                if self.nodes[j].as_ref().map(|d| d.n).unwrap_or(0) == 0 {
                    continue;
                }
                let jbase = self.base[j];
                // Рёбра читаются по индексу, а не копией вектора.
                // Копия на каждую грань каждого чанка — это полторы
                // тысячи выделений за раунд (ADR-129).
                let m = self.nodes[i].as_ref().expect("known").edges[f].len();
                for ei in 0..m {
                    let (a, b) = self.nodes[i].as_ref().expect("known").edges[f][ei];
                    let (ia, ib) = (base + a as u32, jbase + b as u32);
                    if self.reached[ia as usize] || self.reached[ib as usize] {
                        continue;
                    }
                    let (ra, rb) = (uf_find(&mut parent, ia), uf_find(&mut parent, ib));
                    if ra != rb {
                        // Меньший индекс становится корнем: ничьи
                        // разрешаются индексом, а не порядком слияний.
                        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
                        parent[hi as usize] = lo;
                    }
                }
            }
        }

        // Состав групп. Обход по чанкам в порядке возрастания индекса,
        // внутри чанка — по пробегам: порядок вокселей детерминирован.
        let cs = CS as u32;
        let mut roots: Vec<u32> = Vec::new();
        let mut group_cells: Vec<Vec<(u32, u32, u32)>> = Vec::new();
        for k in 0..self.known.len() {
            let i = self.known[k] as usize;
            let base = self.base[i] as usize;
            let n = self.nodes[i].as_ref().expect("known").n;
            if (0..n).all(|l| self.reached[base + l]) {
                continue;
            }
            let (cx, cy, cz) = self.coords(i);
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
                let root = uf_find(&mut parent, (base + lab) as u32);
                let gi = match roots.iter().position(|&x| x == root) {
                    Some(gi) => gi,
                    None => {
                        roots.push(root);
                        group_cells.push(Vec::new());
                        roots.len() - 1
                    }
                };
                let r = self.comps.runs[ri];
                let col = r.col as usize;
                let (ly, lz) = (col % CS, col / CS);
                let mut bits = r.mask;
                while bits != 0 {
                    let x = bits.trailing_zeros();
                    bits &= bits - 1;
                    group_cells[gi].push((ox + x, oy + ly as u32, oz + lz as u32));
                }
            }
        }

        let mut torn = 0usize;
        let mut touched: Vec<u32> = Vec::new();
        for cells in &group_cells {
            if cells.is_empty() {
                continue;
            }
            if cells.len() > self.max_body_cells {
                st.too_big += 1;
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
    pub fn invalidate_at(&mut self, x: u32, y: u32, z: u32) {
        let cs = CS as u32;
        let (cx, cy, cz) = (x / cs, y / cs, z / cs);
        if cx >= self.nx || cy >= self.ny || cz >= self.nz {
            return;
        }
        let i = self.index(cx, cy, cz);
        self.chunks[i] = None;
        self.nodes[i] = None;
        if self.stale[i] {
            self.stale[i] = false;
            self.pending.retain(|&x| x as usize != i);
        }
    }

    /// Память графа компонент — она растёт с числом тронутых чанков
    /// и обязана быть видна.
    pub fn graph_bytes(&self) -> usize {
        let mut n = 0;
        for nd in self.nodes.iter().flatten() {
            n += nd.faces.iter().map(|v| v.len() * 6).sum::<usize>() + nd.bedrock.len();
            n += nd.edges.iter().map(|e| e.len() * 4).sum::<usize>();
            n += nd.ground.iter().map(|g| g.len() * 2).sum::<usize>();
        }
        n
    }

    pub fn known_chunks(&self) -> usize {
        self.known.len()
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
        if s.u32() != self.nx || s.u32() != self.ny || s.u32() != self.nz {
            return false;
        }
        self.recheck = s.bool();
        self.pending = s.u32s();
        self.frontier = s.u32s();
        self.known = s.u32s();
        self.base = s.u32s();
        self.stale = s.bools();
        if self.base.len() != self.chunks.len() || self.stale.len() != self.chunks.len() {
            return false;
        }
        let n = s.list(6);
        for _ in 0..n {
            let ci = s.u32() as usize;
            if ci >= self.chunks.len() {
                return false;
            }
            self.chunks[ci] = if s.bool() {
                let cols = s.u32s();
                if cols.len() != crate::topo::COLS {
                    return false;
                }
                Some(Box::new(TopoChunk { cols }))
            } else {
                None
            };
            self.nodes[ci] = if s.bool() {
                let mut nd = Box::new(ChunkNodes::new());
                nd.n = s.usize();
                nd.bedrock = s.bools();
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
        s.ok()
    }
}

/// Поиск корня с укорачиванием пути. Свободная функция, потому что
/// внутри метода она заняла бы `self` целиком.
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
