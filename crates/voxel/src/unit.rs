//! Тело юнита: воксельный том с помеченными подсистемами.
//!
//! **Урон пространственный, броня работает как геометрия** — не число
//! (`docs/05-units.md`). У юнита есть ядро; попадание снимает воксели,
//! и что именно оно задело, решает геометрия, а не таблица.
//!
//! **Подсистем ровно три, и список закрыт** (ADR-095): двигатель,
//! орудие, сенсор. Метка — поле воксела, а не отдельная сущность:
//! никаких списков компонентов на юнита. Подсистема жива, пока жив
//! **хоть один её воксель, связанный с ядром**, — отрезанная от ядра
//! не работает, даже если цела.
//!
//! **Отказ булев, а не градиентный.** Двигатель либо есть, либо нет.
//! Частичная скорость от доли уцелевших вокселей потребовала бы
//! нормировки, то есть деления в `Fx` на каждом попадании, и это
//! отдельное решение, которого нет.
//!
//! **Обломок — отдельный тип** [`Wreck`] (ADR-204): у него нет ядра
//! и нет подсистем, и отдельный тип делает их отсутствие невозможным
//! по построению. Форма обломка живёт в мировой сетке, содержимое —
//! количество материи; реклейм — та же операция, что копка (ADR-017).

use crate::topo::unit::{core_connectivity, CoreBufs, UnitVol};

/// Метка воксела. Ровно то, что перечисляет ADR-095, плюс броня и ядро.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Part {
    Armour = 0,
    Core = 1,
    Engine = 2,
    Weapon = 3,
    Sensor = 4,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Armour => "броня",
            Part::Core => "ядро",
            Part::Engine => "двигатель",
            Part::Weapon => "орудие",
            Part::Sensor => "сенсор",
        }
    }
}

/// Три подсистемы, у которых есть отказ. Броня и ядро не подсистемы:
/// броня это геометрия, ядро это жизнь.
pub const SUBSYSTEMS: [Part; 3] = [Part::Engine, Part::Weapon, Part::Sensor];

/// Состояние юнита после попадания.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Status {
    pub alive: bool,
    pub engine: bool,
    pub weapon: bool,
    pub sensor: bool,
    /// Уцелело вокселей, связанных с ядром.
    pub attached: u32,
    /// Отвалилось от ядра — материя для реклейма.
    pub detached: u32,
}

impl Status {
    pub fn get(&self, p: Part) -> bool {
        match p {
            Part::Engine => self.engine,
            Part::Weapon => self.weapon,
            Part::Sensor => self.sensor,
            Part::Core => self.alive,
            Part::Armour => true,
        }
    }
}

/// Почему тело не пересобралось в другую сетку.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResampleFail {
    /// Колонка тома — не больше 255 вокселей (ADR-179). Внутри —
    /// сколько потребовалось.
    TooLong(u32),
    /// Ядро в новой сетке не село: выборка его разорвала или сдвинула,
    /// и куб `core_size` перестал быть сплошным ядром. Тело с таким
    /// ядром неубиваемо, и отдавать его наружу нельзя.
    CoreLost,
}

impl ResampleFail {
    pub fn why(self) -> String {
        match self {
            ResampleFail::TooLong(n) => {
                format!("не влезает в колонку: нужно {n} вокселей при пределе 255")
            }
            ResampleFail::CoreLost => "ядро в новой сетке не село".to_string(),
        }
    }
}

#[derive(Clone)]
pub struct UnitBody {
    pub vol: UnitVol,
    /// Метка на воксель — **разделяемая**.
    ///
    /// После сборки она не меняется: урон снимает воксели (`remove`),
    /// а роль оставшихся та же. Значит копия при записи (ADR-018) может
    /// её не копировать, и это шестьдесят процентов тома (ADR-120).
    /// `Arc` здесь не про потоки, а про владение: расхождения он внести
    /// не может, потому что содержимое неизменяемо.
    part: std::sync::Arc<Vec<u8>>,
    /// Маски вокселей каждой подсистемы по колонкам, как у `UnitVol`,
    /// **одним массивом** с шагом `ny * nz`.
    ///
    /// Три отдельных вектора — это три выделения памяти на каждую копию
    /// при записи, а копия делается по первому попаданию, залпом
    /// в тысячи штук. Выделение здесь дороже самих данных (ADR-120).
    /// Считаются один раз при сборке: попадание их только гасит.
    masks: Vec<u64>,
    /// Шаг между масками подсистем.
    mstride: usize,
    pub core: (u8, u8, u8),
    pub core_size: u8,
    /// Ребро воксела тела в **миллиметрах**.
    ///
    /// Канон (ADR-094) даёт телам воксель 0.25 м против 0.5 м у мира.
    /// Он назвал число для обычной боевой машины, а не запретил другие:
    /// ADR-019 уже разрешает миру и телам разные масштабы, и тот же
    /// довод разрешает их между телами (ADR-178). Весь набор сегодня
    /// использует канонические 250 — механизм есть, потому что
    /// многомасштабность обязана быть выразимой, а не подразумеваемой.
    ///
    /// Миллиметры, а не `Fx`: это константа шаблона, она не участвует
    /// в арифметике тика, и целое число исключает вопрос о округлении.
    pub mm: u16,
    /// Слой, с которого начинается **вращающаяся часть** тела: всё
    /// от него и выше смотрит в цель, а не по движению.
    ///
    /// Ноль значит «вращающейся части нет» — тело поворачивается
    /// целиком. Это описание формы, а не число баланса: в хеш оно не
    /// входит и ни на один исход не влияет. Живёт оно здесь, а не
    /// в рендере, по той же причине, по которой здесь живёт ядро
    /// (D-8): величина, заданная дважды, расходится молча.
    pub turret_y: u8,
}

impl UnitBody {
    /// Пустой том. Положение ядра здесь **не задаётся**: оно выводится
    /// из разметки вызовом `seat_core` после того, как воксели
    /// расставлены. Причина в D-8: пока положение задавалось отдельным
    /// числом, зашитый в код танк и он же, загруженный из данных,
    /// получали разные ядра — обе величины были верны, но разные, и
    /// расхождение не было видно ниоткуда.
    pub fn new(nx: u8, ny: u8, nz: u8, core_size: u8) -> UnitBody {
        UnitBody::new_scaled(nx, ny, nz, core_size, 250)
    }

    /// То же, но с явным ребром воксела в миллиметрах (ADR-178).
    pub fn new_scaled(nx: u8, ny: u8, nz: u8, core_size: u8, mm: u16) -> UnitBody {
        assert!(mm > 0, "воксель нулевого размера");
        let n = nx as usize * ny as usize * nz as usize;
        // Маски подсистем лежат теми же колонками, что и том, значит
        // и шаг у них тот же: разойдутся — подсистема будет читаться
        // из чужой колонки (ADR-179).
        let words = nx.div_ceil(64) as usize;
        UnitBody {
            vol: UnitVol::new(nx, ny, nz),
            part: std::sync::Arc::new(vec![Part::Armour as u8; n]),
            masks: vec![0; 3 * ny as usize * nz as usize * words],
            mstride: ny as usize * nz as usize * words,
            core: (0, 0, 0),
            core_size: core_size.max(1),
            mm,
            turret_y: 0,
        }
    }

    /// Габарит тела в миллиметрах: `(длина, высота, ширина)`.
    pub fn extent_mm(&self) -> (u32, u32, u32) {
        let mm = self.mm as u32;
        (
            self.vol.nx as u32 * mm,
            self.vol.ny as u32 * mm,
            self.vol.nz as u32 * mm,
        )
    }

    /// Объём твёрдого в кубических миллиметрах.
    ///
    /// **Считается в мировых единицах, а не в вокселях** (`CLAUDE.md`):
    /// ресурс есть объём на плотность, и воксель крупной машины стоит
    /// ровно во столько раз больше воксела танка, во сколько он больше
    /// по объёму. Без этого экономика разъезжается между классами
    /// объектов — и разъезжается не сразу.
    pub fn volume_mm3(&self) -> u64 {
        let mm = self.mm as u64;
        self.solid_count() as u64 * mm * mm * mm
    }

    /// Пересобрать тело в **другой воксельной сетке**, сохранив размер.
    ///
    /// Нужно, чтобы ответить на вопрос «а как это будет выглядеть при
    /// вокселе в 10 см» замером, а не рассуждением. Габарит в метрах
    /// сохраняется, меняется только через сколько вокселей он выражен.
    ///
    /// **Деталей это не добавляет и добавить не может.** Источник их
    /// не содержит: выборка идёт ближайшим, и каждый исходный воксель
    /// становится блоком. Меняется зернистость силуэта — а с фаской
    /// в половину воксела ещё и её размер, потому что скругление
    /// привязано к вокселю, а не к метру (ADR-208).
    ///
    /// Отказ **называет причину**: их две, и они разные. Либо тело
    /// не влезает в колонку (не больше 255 вокселей, ADR-179 — при
    /// 100 мм это 25.5 м), либо ядро в новой сетке не село, и тогда
    /// машина вышла бы неубиваемой. Второе важнее первого и на глаз
    /// неотличимо, поэтому валить их в один `None` нельзя.
    pub fn resampled(&self, mm: u16) -> Result<UnitBody, ResampleFail> {
        assert!(mm > 0, "воксель нулевого размера");
        let (sx, sy, sz) = (
            self.vol.nx as u32,
            self.vol.ny as u32,
            self.vol.nz as u32,
        );
        let src = self.mm as u32;
        // Габарит в миллиметрах делится на новое ребро, с округлением
        // вверх: обрезать машину на воксель хуже, чем дать ей лишний.
        let dim = |n: u32| -> Result<u8, ResampleFail> {
            let v = (n * src).div_ceil(mm as u32);
            if v == 0 || v > 255 {
                Err(ResampleFail::TooLong(v))
            } else {
                Ok(v as u8)
            }
        };
        let (nx, ny, nz) = (dim(sx)?, dim(sy)?, dim(sz)?);
        // Размер ядра ставится **после** выборки, по тому, что от него
        // уцелело. Пропорция здесь врёт: выборка ближайшим даёт то
        // на воксель больше, то на воксель меньше, и куб `core_size`,
        // посчитанный умножением, перестаёт быть сплошным ядром —
        // ровно так не садилось ядро у трёх машин из четырнадцати.
        let mut out = UnitBody::new_scaled(nx, ny, nz, 1, mm);
        out.turret_y = ((self.turret_y as u32 * src) / mm as u32).min(255) as u8;

        for z in 0..nz as u32 {
            for y in 0..ny as u32 {
                for x in 0..nx as u32 {
                    // Середина целевого воксела в миллиметрах, потом
                    // в исходную сетку. Середина, а не угол: угол
                    // на границе двух вокселей выбирает по знаку
                    // округления, и форма ползёт в одну сторону.
                    let pick = |d: u32, n: u32| -> Option<u32> {
                        let mid = d * mm as u32 + mm as u32 / 2;
                        let i = mid / src;
                        if i < n {
                            Some(i)
                        } else {
                            None
                        }
                    };
                    let (Some(ix), Some(iy), Some(iz)) =
                        (pick(x, sx), pick(y, sy), pick(z, sz))
                    else {
                        continue;
                    };
                    let (ix, iy, iz) = (ix as usize, iy as usize, iz as usize);
                    if !self.vol.get(ix, iy, iz) {
                        continue;
                    }
                    out.set(x as usize, y as usize, z as usize, self.part_at(ix, iy, iz));
                }
            }
        }
        // Наибольший куб ядра, который сядет. Убывающим перебором,
        // а не формулой: `seat_core` требует, чтобы куб был сплошным
        // ядром, а форма ядра после выборки известна только ей.
        let want = ((self.core_size as u32 * src).div_ceil(mm as u32)).clamp(1, 255) as u8;
        for cs in (1..=want).rev() {
            out.core_size = cs;
            if out.seat_core() {
                return Ok(out);
            }
        }
        // Ядро обязано сесть и в новой сетке: если выборка его
        // разорвала, тело неубиваемо, и молча отдавать такое нельзя.
        Err(ResampleFail::CoreLost)
    }

    /// Посадить затравку связности на разметку.
    ///
    /// Правило одно и живёт здесь: затравка — куб `core_size` в
    /// **минимальном углу габарита вокселей, помеченных `Core`**, и
    /// весь этот куб обязан быть ядром. Возвращает `false`, если ядра
    /// нет вовсе или куб в него не влезает: юнит без ядра неубиваем,
    /// и молча пропустить это нельзя.
    pub fn seat_core(&mut self) -> bool {
        let (nx, ny, nz) = (
            self.vol.nx as usize,
            self.vol.ny as usize,
            self.vol.nz as usize,
        );
        let mut lo = (usize::MAX, usize::MAX, usize::MAX);
        let mut any = false;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    if self.vol.get(x, y, z) && self.part_at(x, y, z) == Part::Core {
                        lo = (lo.0.min(x), lo.1.min(y), lo.2.min(z));
                        any = true;
                    }
                }
            }
        }
        if !any {
            return false;
        }
        let cs = self.core_size as usize;
        if lo.0 + cs > nx || lo.1 + cs > ny || lo.2 + cs > nz {
            return false;
        }
        for dz in 0..cs {
            for dy in 0..cs {
                for dx in 0..cs {
                    let (x, y, z) = (lo.0 + dx, lo.1 + dy, lo.2 + dz);
                    if !self.vol.get(x, y, z) || self.part_at(x, y, z) != Part::Core {
                        return false;
                    }
                }
            }
        }
        self.core = (lo.0 as u8, lo.1 as u8, lo.2 as u8);
        true
    }

    #[inline]
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.vol.ny as usize + y) * self.vol.nx as usize + x
    }

    /// Начало колонки в массиве масок.
    #[inline]
    fn col(&self, y: usize, z: usize) -> usize {
        (z * self.vol.ny as usize + y) * self.vol.words as usize
    }

    /// Расставить воксель. Единственное место, где меняются метки,
    /// и потому единственное, где разделяемый массив расщепляется.
    pub fn set(&mut self, x: usize, y: usize, z: usize, p: Part) {
        self.vol.set(x, y, z, true);
        let i = self.idx(x, y, z);
        std::sync::Arc::make_mut(&mut self.part)[i] = p as u8;
        if let Some(k) = subsystem_slot(p) {
            let c = self.col(y, z);
            self.masks[k * self.mstride + c + (x >> 6)] |= 1u64 << (x & 63);
        }
    }

    pub fn part_at(&self, x: usize, y: usize, z: usize) -> Part {
        match self.part[self.idx(x, y, z)] {
            1 => Part::Core,
            2 => Part::Engine,
            3 => Part::Weapon,
            4 => Part::Sensor,
            _ => Part::Armour,
        }
    }

    /// Снять воксель. Возвращает `true`, если он там был.
    pub fn remove(&mut self, x: usize, y: usize, z: usize) -> bool {
        if !self.vol.get(x, y, z) {
            return false;
        }
        self.vol.set(x, y, z, false);
        if let Some(k) = subsystem_slot(self.part_at(x, y, z)) {
            let c = self.col(y, z);
            self.masks[k * self.mstride + c + (x >> 6)] &= !(1u64 << (x & 63));
        }
        true
    }

    /// Состояние после урона: что связано с ядром и какие подсистемы живы.
    ///
    /// Один флуд от ядра на весь запрос, а не по флуду на подсистему:
    /// маска прикреплённого считается один раз, а подсистемы читаются
    /// её пересечением со своими масками.
    pub fn status(&self, b: &mut CoreBufs) -> Status {
        let rep = core_connectivity(&self.vol, b, self.core, self.core_size);
        let reached = b.reached_cols();
        let live = |k: usize| -> bool {
            self.masks[k * self.mstride..(k + 1) * self.mstride]
                .iter()
                .zip(reached)
                .any(|(m, r)| m & r != 0)
        };
        Status {
            alive: rep.core_alive,
            engine: rep.core_alive && live(0),
            weapon: rep.core_alive && live(1),
            sensor: rep.core_alive && live(2),
            attached: rep.attached,
            detached: rep.detached,
        }
    }

    pub fn solid_count(&self) -> u32 {
        self.vol.solid_count()
    }

    /// Байты тома. Метка воксела — байт, и это самая дорогая часть:
    /// занятость стоит бит.
    /// Весь том, включая разделяемые метки. Для шаблона это его
    /// полный размер.
    pub fn bytes(&self) -> usize {
        self.bytes_private() + self.part.len()
    }

    /// Только собственная память тома. У копии метки общие с шаблоном,
    /// и считать их дважды — завысить расход флота вдвое с половиной.
    pub fn bytes_private(&self) -> usize {
        self.vol.cols.len() * 8 + self.masks.len() * 8
    }

    /// **Обломок этого тела.** Машина оседает на месте, вся её материя
    /// переливается в обломок.
    ///
    /// `damage_mm3` — материя, снятая попаданиями при жизни. Канон
    /// требует перелить её сюда целиком: «снятые попаданиями воксели
    /// лежат в контейнере `damage` и при смерти переливаются в обломок
    /// — материя вся там, промежуточного „нигде“ не существует»
    /// (`docs/05-units.md`). Ноль — если машина цела.
    ///
    /// **След машины, ровный слой.** Куча лежит там, где стояла
    /// машина: занята та же мировая клетка, что была занята телом
    /// хоть на какой высоте. По этому следу материя раскладывается
    /// **ровно** — высота одна на все столбы.
    ///
    /// Оседание «каждый столб отдельно» пробовалось первым и куч
    /// не давало: у машины масса стоит вертикально, и столб, полный
    /// сверху донизу, сохранял свою высоту. Шагающий разведчик так
    /// оседал с 6.0 м до 6.0 м. Ровный слой даёт то, чем обломок
    /// и является: низкое поле, в котором по очертанию ещё видно,
    /// что тут погибло.
    ///
    /// Остаток раскладывается по **первым столбам линейного
    /// индекса** — ничья разрешается индексом, а не порядком обхода
    /// (`CLAUDE.md`).
    pub fn wreck(&self, damage_mm3: u64) -> Wreck {
        // Отношение рёбер. При каноне это 2, то есть восемь вокселей
        // тела на мировой (`07-economy.md`, «конверсия масштабов»).
        let k = (WORLD_MM / self.mm).max(1) as usize;
        let per = (k * k * k) as u64;
        let (nx, ny, nz) = (
            self.vol.nx as usize,
            self.vol.ny as usize,
            self.vol.nz as usize,
        );
        let (wx, wz) = (nx.div_ceil(k), nz.div_ceil(k));

        // След строится **по словам колонок**, а не обходом вокселей:
        // столб тела — это `ИЛИ` всех его высот, и биты результата
        // сразу дают занятые `x`. Поэлементный обход стоил крепости
        // 1857 мкс на смерть, словесный — 155. Приём тот же, что
        // у мешера, читающего маски вместо массива материалов
        // (ADR-068).
        let mut foot = vec![false; wx * wz];
        let words = self.vol.words as usize;
        let mut acc = vec![0u64; words];
        for z in 0..nz {
            acc.iter_mut().for_each(|w| *w = 0);
            for y in 0..ny {
                for (a, v) in acc.iter_mut().zip(self.vol.col(y, z)) {
                    *a |= *v;
                }
            }
            for (wi, mut m) in acc.iter().copied().enumerate() {
                while m != 0 {
                    let x = wi * 64 + m.trailing_zeros() as usize;
                    m &= m - 1;
                    foot[(z / k) * wx + x / k] = true;
                }
            }
        }
        let n = self.solid_count() as u64;
        let a = foot.iter().filter(|f| **f).count() as u64;
        // Высота слоя и сколько столбов получают лишний воксель.
        // Остаток округляется **вверх**: форма обязана вмещать хотя бы
        // собственную материю тела, иначе контейнеру нечем её держать.
        let (h, extra) = if a == 0 {
            (0, 0)
        } else {
            (n / (per * a), (n % (per * a)).div_ceil(per) as usize)
        };
        let hi = (h as usize + usize::from(extra > 0)).max(1);
        let mut vol = UnitVol::new(wx as u8, hi as u8, wz as u8);
        let mut left = extra;
        for (i, on) in foot.iter().enumerate() {
            if !on {
                continue;
            }
            let mut col = h as usize;
            if left > 0 {
                col += 1;
                left -= 1;
            }
            for y in 0..col {
                vol.set(i % wx, y, i / wx, true);
            }
        }
        Wreck {
            vol,
            mm: WORLD_MM,
            matter: self.volume_mm3() + damage_mm3,
        }
    }
}

/// Ребро воксела **мира** в миллиметрах — канон (ADR-094).
///
/// Обломок живёт в мировой сетке, а не в сетке тела: реклеймит его тот
/// же код, что копает землю (ADR-017), а копает он мировые воксели.
pub const WORLD_MM: u16 = 500;

/// **Обломок** — то, что остаётся от тела, когда его разобрали.
///
/// **Это не `UnitBody`.** У обломка нет ядра и нет подсистем, и
/// отдельный тип делает их отсутствие невозможным по построению.
/// Канон требует того же явно: «реклейм допустим только по телам
/// класса „обломок“ и по нейтральной породе… тот же код, применённый
/// к тому живого юнита, — механика мгновенной победы»
/// (`docs/05-units.md`). Разница классов, выраженная типом, не может
/// быть забыта на месте вызова.
///
/// **Форма грубая, содержимое точное.** Форма живёт в мировой сетке
/// 0.5 м и потому огрубляется; содержимое — количество материи
/// в кубических миллиметрах, и огрубление сетки его не трогает
/// (ADR-021, ADR-022). Две величины разные нарочно: вместимость формы
/// и количество в контейнере совпадать не обязаны, как не обязаны
/// совпадать объём склада и то, сколько в нём лежит.
#[derive(Clone)]
pub struct Wreck {
    pub vol: UnitVol,
    /// Ребро воксела обломка в миллиметрах. Всегда [`WORLD_MM`].
    pub mm: u16,
    matter: u64,
}

impl Wreck {
    /// Сколько материи в обломке, в кубических миллиметрах.
    pub fn matter_mm3(&self) -> u64 {
        self.matter
    }

    pub fn solid_count(&self) -> u32 {
        self.vol.solid_count()
    }

    /// Вместимость формы: объём её вокселей. С содержимым совпадать
    /// не обязана — см. описание типа.
    pub fn capacity_mm3(&self) -> u64 {
        let mm = self.mm as u64;
        self.solid_count() as u64 * mm * mm * mm
    }

    /// Габарит обломка в миллиметрах: `(длина, высота, ширина)`.
    pub fn extent_mm(&self) -> (u32, u32, u32) {
        let mm = self.mm as u32;
        (
            self.vol.nx as u32 * mm,
            self.vol.ny as u32 * mm,
            self.vol.nz as u32 * mm,
        )
    }

    pub fn bytes(&self) -> usize {
        self.vol.cols.len() * 8
    }

    /// **Снять воксель обломка и получить материю.** Это и есть
    /// реклейм — та же операция, что копка (ADR-017).
    ///
    /// Выдаётся **доля содержимого, а не объём воксела**: обломок
    /// контейнер, и содержимое в нём количество (ADR-022). Делится
    /// оставшееся на число оставшихся вокселей, поэтому последний
    /// воксель выносит весь остаток и деление ничего не теряет.
    /// Отдельного разрежённого слоя дробных остатков
    /// (`07-economy.md`) обломку для этого не нужно, а скрытого стока
    /// на округлении не возникает: сумма выданного равна содержимому
    /// при любом порядке снятия.
    ///
    /// Выдача на воксель к концу разбора слегка растёт — это цена
    /// того, что стока нет.
    pub fn reclaim(&mut self, x: usize, y: usize, z: usize) -> u64 {
        if !self.vol.get(x, y, z) {
            return 0;
        }
        let n = self.solid_count() as u64;
        self.vol.set(x, y, z, false);
        let take = self.matter / n;
        self.matter -= take;
        take
    }
}

#[inline]
fn subsystem_slot(p: Part) -> Option<usize> {
    match p {
        Part::Engine => Some(0),
        Part::Weapon => Some(1),
        Part::Sensor => Some(2),
        _ => None,
    }
}

/// Танк по ADR-094: 6×3×3 м при вокселе тел 0.25 м — 24×12×12.
///
/// Броня по краям, ядро в центре, двигатель сзади, орудие спереди
/// сверху, сенсоры по бокам башни. Форма условная: настоящие тома
/// приходят `.vox`-моделями из данных (ADR-008), а эта нужна, чтобы
/// сцена имела что разбирать.
pub fn tank() -> UnitBody {
    let (nx, ny, nz) = (24u8, 12u8, 12u8);
    let mut u = UnitBody::new(nx, ny, nz, 2);
    for z in 0..nz as usize {
        for y in 0..ny as usize {
            for x in 0..nx as usize {
                // Корпус: нижние две трети по высоте.
                let hull = y < 8;
                // Башня: середина сверху.
                let turret = y >= 8 && (6..18).contains(&x) && (3..9).contains(&z);
                // Ствол торчит вперёд из башни — он **вне** её габарита,
                // поэтому в геометрию входит отдельно. Первая редакция
                // этого не сделала, и орудия в танке не оказалось вовсе.
                let barrel = (17..24).contains(&x) && (9..11).contains(&y) && (5..7).contains(&z);
                if !hull && !turret && !barrel {
                    continue;
                }
                let p = if (10..14).contains(&x) && (4..7).contains(&y) && (5..8).contains(&z) {
                    Part::Core
                } else if x < 5 && (2..6).contains(&y) && (3..9).contains(&z) {
                    Part::Engine
                } else if barrel || (turret && x >= 14 && y >= 9 && (4..8).contains(&z)) {
                    Part::Weapon
                } else if turret && y >= 10 && !(4..8).contains(&z) {
                    Part::Sensor
                } else {
                    Part::Armour
                };
                u.set(x, y, z, p);
            }
        }
    }
    assert!(u.seat_core(), "у танка обязано быть ядро");
    // Башня — всё, что выше корпуса; она же граница `hull` в разметке
    // выше. Одна величина, записанная один раз.
    u.turret_y = 8;
    u
}

/// Шагоход: второе ванильное тело. 4×5×4 м при вокселе тел 0.25 м —
/// том 16×20×16.
///
/// **Зачем второе тело.** Две стороны, отличающиеся только цветом, —
/// это не две армии, а одна перекрашенная: силуэт на экране важнее
/// оттенка, потому что на дистанции RTS оттенок съедает туман, а форму
/// нет. Шагоход выбран противоположным танку по обоим габаритам: танк
/// длинный и низкий, шагоход узкий и высокий, и различить их можно
/// даже по тени.
///
/// Форма условная ровно в той же мере, что у танка: настоящие тома
/// приходят `.vox`-моделями из данных (ADR-008).
pub fn strider() -> UnitBody {
    let (nx, ny, nz) = (16u8, 20u8, 16u8);
    let mut u = UnitBody::new(nx, ny, nz, 2);
    for z in 0..nz as usize {
        for y in 0..ny as usize {
            for x in 0..nx as usize {
                // Четыре ноги по углам: столбы 3×3 до восьмого слоя.
                // Верх ноги упирается в короб корпуса — иначе нога
                // не связана с ядром и не считается живой.
                let leg = y < 8
                    && ((1..4).contains(&x) || (12..15).contains(&x))
                    && ((1..4).contains(&z) || (12..15).contains(&z));
                // Корпус: короб посередине.
                let hull = (8..14).contains(&y) && (3..13).contains(&x) && (3..13).contains(&z);
                // Голова с двумя стволами. Стволы торчат вперёд и
                // **вне** габарита головы, поэтому считаются отдельно —
                // ровно та же ловушка, что у ствола танка.
                let head = y >= 14 && (5..11).contains(&x) && (5..11).contains(&z);
                let barrel = (11..16).contains(&x)
                    && (15..18).contains(&y)
                    && ((5..7).contains(&z) || (9..11).contains(&z));
                if !leg && !hull && !head && !barrel {
                    continue;
                }
                let p = if (7..9).contains(&x) && (10..12).contains(&y) && (7..9).contains(&z) {
                    Part::Core
                } else if hull && (3..6).contains(&x) {
                    Part::Engine
                } else if barrel || (head && x >= 9 && (15..18).contains(&y)) {
                    Part::Weapon
                } else if head && y >= 18 && x >= 9 && !(7..9).contains(&z) {
                    Part::Sensor
                } else {
                    Part::Armour
                };
                u.set(x, y, z, p);
            }
        }
    }
    assert!(u.seat_core(), "у шагохода обязано быть ядро");
    // Вращается всё, что выше корпуса: голова со стволами.
    u.turret_y = 14;
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tank_has_every_part_and_the_right_size() {
        let t = tank();
        // 6×3×3 м при вокселе 0.25 м — том 24×12×12 (ADR-094).
        assert_eq!((t.vol.nx, t.vol.ny, t.vol.nz), (24, 12, 12));
        let mut seen = [0u32; 5];
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..24 {
                    if t.vol.get(x, y, z) {
                        seen[t.part_at(x, y, z) as usize] += 1;
                    }
                }
            }
        }
        for p in [Part::Armour, Part::Core, Part::Engine, Part::Weapon, Part::Sensor] {
            assert!(seen[p as usize] > 0, "{} отсутствует", p.name());
        }
        assert!(t.solid_count() > 1000, "танк подозрительно пуст");
    }

    /// Шагоход — второе ванильное тело, и требования к нему те же:
    /// все роли на месте, размер по канону, ядро село.
    #[test]
    fn strider_has_every_part_and_the_right_size() {
        let s = strider();
        // 4×5×4 м при вокселе 0.25 м — том 16×20×16.
        assert_eq!((s.vol.nx, s.vol.ny, s.vol.nz), (16, 20, 16));
        let mut seen = [0u32; 5];
        for z in 0..16 {
            for y in 0..20 {
                for x in 0..16 {
                    if s.vol.get(x, y, z) {
                        seen[s.part_at(x, y, z) as usize] += 1;
                    }
                }
            }
        }
        for p in [Part::Armour, Part::Core, Part::Engine, Part::Weapon, Part::Sensor] {
            assert!(seen[p as usize] > 0, "{} отсутствует", p.name());
        }
        assert!(s.solid_count() > 400, "шагоход подозрительно пуст");
    }

    /// **Целый шагоход работает весь.** Ноги, корпус и голова обязаны
    /// быть связаны с ядром: несвязанная нога — это подсистема,
    /// которой нет, и заметно это только отсюда.
    #[test]
    fn an_intact_strider_works() {
        let s = strider();
        let mut b = CoreBufs::new();
        let st = s.status(&mut b);
        assert!(st.alive && st.engine && st.weapon && st.sensor, "{st:?}");
        assert_eq!(st.detached, 0, "часть тела не связана с ядром");
    }

    /// **Два тела не спутать.** Разные габариты — это то, ради чего
    /// второе тело и заводилось.
    #[test]
    fn the_two_bodies_do_not_look_alike() {
        let (t, s) = (tank(), strider());
        assert_ne!(
            (t.vol.nx, t.vol.ny, t.vol.nz),
            (s.vol.nx, s.vol.ny, s.vol.nz),
            "силуэты совпадают — это одно тело в двух цветах"
        );
        // Танк длиннее, чем высокий; шагоход наоборот.
        assert!(t.vol.nx > t.vol.ny, "танк не длинный");
        assert!(s.vol.ny > s.vol.nx, "шагоход не высокий");
    }

    /// **Линия башни лежит внутри тома и отрезает непустую часть.**
    /// Линия за габаритом даёт пустую башню, линия в нуле — тело,
    /// которое поворачивается целиком; и то и другое молча.
    #[test]
    fn the_turret_line_cuts_something_off() {
        for b in [tank(), strider()] {
            let ty = b.turret_y as usize;
            assert!(ty > 0 && ty < b.vol.ny as usize, "линия башни {ty} вне тома");
            let mut above = 0u32;
            let mut below = 0u32;
            for z in 0..b.vol.nz as usize {
                for y in 0..b.vol.ny as usize {
                    for x in 0..b.vol.nx as usize {
                        if b.vol.get(x, y, z) {
                            if y >= ty {
                                above += 1;
                            } else {
                                below += 1;
                            }
                        }
                    }
                }
            }
            assert!(above > 0 && below > 0, "по одну сторону линии пусто");
            assert_eq!(above + below, b.solid_count(), "линия потеряла воксели");
        }
    }

    /// Целый танк — все подсистемы живы. Иначе всё остальное
    /// бессмысленно.
    #[test]
    fn an_intact_tank_works() {
        let t = tank();
        let mut b = CoreBufs::new();
        let s = t.status(&mut b);
        assert!(s.alive && s.engine && s.weapon && s.sensor, "{s:?}");
        assert_eq!(s.detached, 0);
    }

    /// **Отстрел подсистемы отключает способность, не убивая юнита.**
    /// Это то, ради чего выбраны помеченные подсистемы (ADR-095).
    #[test]
    fn shooting_out_the_engine_stops_the_tank_but_does_not_kill_it() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..24 {
                    if t.vol.get(x, y, z) && t.part_at(x, y, z) == Part::Engine {
                        t.remove(x, y, z);
                    }
                }
            }
        }
        let s = t.status(&mut b);
        assert!(s.alive, "танк умер от потери двигателя");
        assert!(!s.engine, "двигатель жив после отстрела");
        assert!(s.weapon && s.sensor, "пострадали чужие подсистемы: {s:?}");
    }

    /// **Подсистема, отрезанная от ядра, не работает, даже если цела.**
    /// Способность считается по связности (ADR-095, пункт 4).
    #[test]
    fn a_subsystem_severed_from_the_core_stops_working() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        assert!(t.status(&mut b).engine);
        // Срезаем корпус между двигателем и ядром: сам двигатель цел.
        for z in 0..12 {
            for y in 0..12 {
                for x in 5..9 {
                    t.remove(x, y, z);
                }
            }
        }
        let mut engine_left = 0;
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..24 {
                    if t.vol.get(x, y, z) && t.part_at(x, y, z) == Part::Engine {
                        engine_left += 1;
                    }
                }
            }
        }
        assert!(engine_left > 0, "двигатель случайно снесли — тест ни о чём");
        let s = t.status(&mut b);
        assert!(s.alive, "ядро должно быть цело");
        assert!(!s.engine, "отрезанный двигатель работает");
        assert!(s.detached > 0, "отрезанное не посчитано");
    }

    /// Пробитие к ядру убивает.
    #[test]
    fn destroying_the_core_kills() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..24 {
                    if t.vol.get(x, y, z) && t.part_at(x, y, z) == Part::Core {
                        t.remove(x, y, z);
                    }
                }
            }
        }
        let s = t.status(&mut b);
        assert!(!s.alive, "танк жив без ядра");
    }

    /// Урон пространственный: попадание в нос не трогает двигатель
    /// в корме. Броня работает как геометрия.
    #[test]
    fn damage_is_positional() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        for z in 0..12 {
            for y in 0..8 {
                for x in 20..24 {
                    t.remove(x, y, z);
                }
            }
        }
        let s = t.status(&mut b);
        assert!(s.alive && s.engine, "нос снесли, а двигатель встал: {s:?}");
    }
}

#[cfg(test)]
mod core_seat_tests {
    use super::*;

    /// Затравка обязана лежать внутри разметки ядра, иначе связность
    /// считается не от ядра.
    #[test]
    fn the_seed_cube_lies_inside_the_core_marking() {
        let t = tank();
        let cs = t.core_size as usize;
        let (cx, cy, cz) = (t.core.0 as usize, t.core.1 as usize, t.core.2 as usize);
        for dz in 0..cs {
            for dy in 0..cs {
                for dx in 0..cs {
                    assert_eq!(
                        t.part_at(cx + dx, cy + dy, cz + dz),
                        Part::Core,
                        "затравка вышла из ядра"
                    );
                }
            }
        }
    }

    /// Том без ядра обязан быть отвергнут, а не собран.
    #[test]
    fn a_body_without_a_core_is_refused() {
        let mut u = UnitBody::new(4, 4, 4, 2);
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    u.set(x, y, z, Part::Armour);
                }
            }
        }
        assert!(!u.seat_core());
    }

    /// Ядро меньше затравки — тоже отказ: куб не влезает в разметку.
    #[test]
    fn a_core_smaller_than_the_seed_is_refused() {
        let mut u = UnitBody::new(4, 4, 4, 2);
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    u.set(x, y, z, Part::Armour);
                }
            }
        }
        u.set(1, 1, 1, Part::Core);
        assert!(!u.seat_core());
    }
}

#[cfg(test)]
mod wreck_tests {
    use super::*;

    /// Разобрать обломок целиком в заданном порядке столбов.
    fn strip(w: &mut Wreck, back_to_front: bool) -> u64 {
        let (nx, ny, nz) = (w.vol.nx as usize, w.vol.ny as usize, w.vol.nz as usize);
        let mut got = 0u64;
        for i in 0..nz {
            let z = if back_to_front { nz - 1 - i } else { i };
            for j in 0..ny {
                let y = if back_to_front { ny - 1 - j } else { j };
                for k in 0..nx {
                    let x = if back_to_front { nx - 1 - k } else { k };
                    got += w.reclaim(x, y, z);
                }
            }
        }
        got
    }

    /// **Материя машины вся в обломке, до кубического миллиметра.**
    ///
    /// Это главный инвариант проекта в самом уязвимом его месте:
    /// сетка обломка вдвое грубее сетки тела, и наивный пересчёт
    /// «вокселей стало вчетверо меньше» потерял бы семь восьмых.
    /// Материя переживает огрубление потому, что она количество,
    /// а не кубики (ADR-022).
    #[test]
    fn a_wreck_holds_everything_the_machine_was_made_of() {
        let t = tank();
        assert_eq!(
            t.wreck(0).matter_mm3(),
            t.volume_mm3(),
            "материя не сошлась при переходе в мировую сетку"
        );
        // Сетка действительно огрубилась — иначе тест ничего не ловит.
        assert!(
            t.wreck(0).solid_count() < t.solid_count(),
            "обломок не грубее машины, проверять нечего"
        );
    }

    /// **Снятая попаданиями броня уходит в обломок, а не в никуда.**
    ///
    /// Канон: материя из контейнера `damage` при смерти переливается
    /// в обломок, промежуточного «нигде» не существует
    /// (`docs/05-units.md`). Добивание выхода реклейма не снижает.
    #[test]
    fn damage_pours_into_the_wreck() {
        let mut t = tank();
        let whole = t.volume_mm3();
        // Снести половину длины и сложить снятое в контейнер damage.
        let mut damage = 0u64;
        let mm = t.mm as u64;
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..12 {
                    if t.remove(x, y, z) {
                        damage += mm * mm * mm;
                    }
                }
            }
        }
        assert!(damage > 0, "ничего не снялось");
        assert_eq!(
            t.wreck(damage).matter_mm3(),
            whole,
            "добивание снизило выход — это скрытый сток"
        );
    }

    /// **Реклейм возвращает содержимое целиком и при любом порядке.**
    ///
    /// Выдаётся доля от оставшегося, а не объём воксела, поэтому
    /// деление не теряет ничего: последний воксель выносит остаток.
    /// Порядок не влияет — иначе реклейм был бы источником десинка
    /// в самой обыденной операции игры.
    #[test]
    fn reclaiming_a_wreck_returns_every_last_bit() {
        for order in [false, true] {
            let mut w = tank().wreck(1_234_567);
            let had = w.matter_mm3();
            let got = strip(&mut w, order);
            assert_eq!(got, had, "реклейм потерял материю (порядок {order})");
            assert_eq!(w.matter_mm3(), 0, "в разобранном обломке осталась материя");
            assert_eq!(w.solid_count(), 0, "в разобранном обломке остались воксели");
        }
        // Снять то, чего нет, — ноль, а не порция: иначе повторный
        // приказ двух краулеров создавал бы материю (ADR-031).
        let mut w = tank().wreck(0);
        let first = w.reclaim(0, 0, 0);
        assert!(first > 0);
        assert_eq!(w.reclaim(0, 0, 0), 0, "пустая ячейка выдала материю");
    }

    /// **Обломок — куча, а не машина.**
    ///
    /// След остаётся машины, высота падает до той, что даёт материя.
    /// Полая машина оседает в разы, сплошная почти не оседает — ей
    /// нечему оседать, и это правда, а не изъян модели. И форма
    /// обязана вмещать хотя бы собственную материю тела: округление
    /// остатка вверх существует ровно ради этого.
    #[test]
    fn a_wreck_is_a_pile_not_a_machine() {
        // Шагоход полый: корпус висит между ног. Первая редакция
        // осаживала каждый столб отдельно и давала ему 6.0 м из 6.0 —
        // куча ростом с машину не куча.
        let s = strider();
        assert!(
            s.wreck(0).extent_mm().1 * 2 <= s.extent_mm().1,
            "полая машина не осела и вдвое: {} из {}",
            s.wreck(0).extent_mm().1,
            s.extent_mm().1
        );
        for b in [tank(), strider()] {
            let w = b.wreck(0);
            let (bx, by, bz) = b.extent_mm();
            let (wx, wy, wz) = w.extent_mm();
            // Куча не крупнее машины ни по одной оси. Допуск
            // в один мировой воксель — огрубление сетки: тело
            // нечётной длины в мировую клетку целиком не ложится.
            let slack = WORLD_MM as u32;
            assert!(
                wx <= bx + slack && wy <= by + slack && wz <= bz + slack,
                "обломок крупнее машины: {wx}x{wy}x{wz} против {bx}x{by}x{bz}"
            );
            assert!(
                w.capacity_mm3() >= b.volume_mm3(),
                "форме обломка нечем держать собственную материю тела"
            );
        }
    }
}
