//! Физика корпуса: настоящая, в фиксированной точке.
//!
//! **Не анимация.** Корпус здесь — твёрдое тело: у него есть высота,
//! вертикальная скорость, наклон и угловая скорость, а двигают их
//! **силы** в точках опоры. Клевок при торможении, крен в повороте,
//! просадка под весом, наклон по склону, падение при потере опоры —
//! всё это выходит из одних и тех же уравнений, а не рисуется
//! по отдельности.
//!
//! **Почему не полноценный решатель связей.** Движки общего
//! назначения (Rapier, PhysX) считают в максимальных координатах
//! и снимают связи итеративно, с допусками. Это дорого и, что важнее,
//! **недетерминированно по построению**: результат зависит от числа
//! итераций и порядка обхода пар. У нас lockstep и golden-хеши,
//! поэтому связи задаются **конструкцией**, а не решателем: корпус
//! имеет ровно три динамических степени свободы — подъём, тангаж,
//! крен, — и каждая интегрируется явно.
//!
//! **Чего здесь нет и почему.** Горизонтального движения: им заведует
//! фаза движения симуляции. Физика отдаёт ей одно число — нагрузку
//! на опоры [`Chassis::load`], — и по нему тяга ограничивается
//! сцеплением: в воздухе не разгоняются.
//!
//! **Ни одного корня и ни одной тригонометрии.** Корень в `Fx` стоит
//! в 3.6–5.4 раза дороже плавающего и неустраним (ADR-071), поэтому
//! нормаль корпуса поправляется первым порядком, а поворот приходит
//! готовым вектором курса. Всё остальное — сложения и умножения.

#![forbid(unsafe_code)]

use fx::Fx;

pub mod blast;

/// Вектор в мировых единицах.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct V3 {
    pub x: Fx,
    pub y: Fx,
    pub z: Fx,
}

impl Default for V3 {
    fn default() -> V3 {
        V3::ZERO
    }
}

impl V3 {
    pub const ZERO: V3 = V3 {
        x: Fx::ZERO,
        y: Fx::ZERO,
        z: Fx::ZERO,
    };
    /// Вертикаль: «верх» у невозмущённого корпуса.
    pub const UP: V3 = V3 {
        x: Fx::ZERO,
        y: Fx::ONE,
        z: Fx::ZERO,
    };

    pub const fn new(x: Fx, y: Fx, z: Fx) -> V3 {
        V3 { x, y, z }
    }

    pub const fn add(self, o: V3) -> V3 {
        V3::new(self.x.add(o.x), self.y.add(o.y), self.z.add(o.z))
    }

    pub const fn sub(self, o: V3) -> V3 {
        V3::new(self.x.sub(o.x), self.y.sub(o.y), self.z.sub(o.z))
    }

    pub const fn scale(self, k: Fx) -> V3 {
        V3::new(self.x.mul(k), self.y.mul(k), self.z.mul(k))
    }

    pub const fn dot(self, o: V3) -> Fx {
        self.x.mul(o.x).add(self.y.mul(o.y)).add(self.z.mul(o.z))
    }

    /// Векторное произведение. Нужно ровно в одном месте — в повороте
    /// нормали угловой скоростью, — но выписывать его там руками
    /// значит однажды перепутать знак и не заметить.
    pub const fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y.mul(o.z).sub(self.z.mul(o.y)),
            self.z.mul(o.x).sub(self.x.mul(o.z)),
            self.x.mul(o.y).sub(self.y.mul(o.x)),
        )
    }

    pub const fn len_sq(self) -> Fx {
        self.dot(self)
    }

    /// Привести длину к единице **без корня**.
    ///
    /// Первый порядок ряда: `n ← n · (3 − |n|²) / 2`. У единичного
    /// вектора это тождество, у почти единичного — шаг к единице,
    /// и за тик нормаль отклоняется на доли процента. Корень здесь
    /// был бы единственной дорогой операцией во всей физике
    /// (ADR-071), и он не нужен.
    ///
    /// Сходимость есть только вблизи единицы, поэтому длина
    /// **зажимается**: вырожденный вектор чинится, а не уводит
    /// итерацию в бесконечность.
    pub fn renormalised(self) -> V3 {
        let l2 = self.len_sq().max(Fx::from_ratio(1, 4)).min(Fx::from_int(4));
        self.scale(Fx::from_int(3).sub(l2).mul(Fx::from_ratio(1, 2)))
    }
}

/// Точка опоры в местных координатах корпуса.
///
/// Только горизонталь: вертикаль у всех опор одна и задаётся клиренсом.
/// Катки одной гусеницы стоят на одной высоте, ноги шагохода — тоже,
/// пока они на земле; разную высоту даёт **наклон корпуса**, а не
/// разные опоры.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Support {
    pub x: Fx,
    pub z: Fx,
}

impl Support {
    pub const fn new(x: Fx, z: Fx) -> Support {
        Support { x, z }
    }
}

/// Числа ходовой. Все до одного приходят из данных, а не из кода.
///
/// **Сила делится на массу прямо здесь.** Иначе массу пришлось бы
/// таскать через каждое умножение ради величины, которая сокращается:
/// нас интересует ускорение, а не сила. Поэтому `stiffness` — это
/// «ускорение на единицу сжатия», а не «ньютон на метр».
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ride {
    /// Жёсткость подвески, отнесённая к массе.
    pub stiffness: Fx,
    /// Демпфирование, отнесённое к массе.
    pub damping: Fx,
    /// Ход подвески: дальше сжимать некуда.
    pub travel: Fx,
    /// Клиренс: насколько центр масс выше опоры в покое.
    pub clearance: Fx,
    /// Момент инерции, отнесённый к массе, — квадрат радиуса инерции.
    pub inertia: Fx,
    /// Тяготение.
    pub gravity: Fx,
    /// Предварительный натяг: сила опоры при нулевом сжатии,
    /// отнесённая к массе (ADR-234).
    ///
    /// Равный весу на опору, он держит корпус **ровно на клиренсе**
    /// в покое. Без натяга мягкая подвеска (жёсткость два, выбранная
    /// ради видимого клевка — ADR-225) просаживалась бы под весом
    /// на метр с четвертью при ходе в двадцать сантиметров, а жёсткая
    /// не клевала бы. Натяг снимает выбор: пружина остаётся мягкой
    /// к отклонениям и держит вес. Ноль — старая пружина, которая
    /// только толкает.
    pub preload: Fx,
    /// Угловое трение. Без него корпус качается вечно: подвеска гасит
    /// подъём, но не вращение вокруг центра масс.
    pub angular_damping: Fx,
    /// Предел позиционной поправки за тик у опирающейся машины
    /// (ADR-272). Пружина, выбранная мягкой ради видимого клевка, не
    /// успевает поднять корпус за полом, и подъём делала поправка —
    /// рывками до полного хода. Теперь корпус догоняет пол не быстрее
    /// этого предела за тик; свободное падение поправка не ограничивает.
    pub settle: Fx,
    /// Насколько корпусу позволено завалиться: наименьшее `up.y`.
    ///
    /// **Не косметика.** Опоры стоят по углам машины, а рельеф
    /// воксельный: на уступе передняя опора оказывается на метр выше
    /// задней, и честная пружина кладёт корпус носом в небо. Настоящая
    /// машина так не встаёт — она **мостит** уступ, потому что у неё
    /// жёсткая рама и ход подвески конечен. Предел и есть эта рама.
    pub min_up: Fx,
}

/// Состояние корпуса — то, что физика двигает сама.
///
/// **Всё это состояние переживает тик, значит всё это обязано входить
/// в хеш** (ADR-116). Не войдёт — интегрирование можно переписать
/// целиком и увидеть тот же хеш до бита.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chassis {
    /// Высота центра масс.
    pub y: Fx,
    /// Вертикальная скорость.
    pub vy: Fx,
    /// Нормаль корпуса: куда смотрит «верх». Почти единичная.
    pub up: V3,
    /// Угловая скорость. Вертикальная составляющая не используется:
    /// поворотом заведует фаза движения, а не физика.
    pub w: V3,
    /// Доля опор, касающихся земли на прошлом шаге, от 0 до 1.
    ///
    /// Это и есть сцепление: тяга обязана быть не больше
    /// `friction × load`, машина в воздухе не разгоняется, а на двух
    /// опорах над обрывом разгоняется вдвое хуже. Доля, а не сумма сил:
    /// сумма сил у корпуса, подвешенного на среднем поле, в покое равна
    /// нулю, и «стоит» читалось бы как «висит».
    pub load: Fx,
}

impl Chassis {
    /// Корпус, поставленный на ровную землю высотой `ground`.
    pub fn resting(ground: Fx, r: &Ride) -> Chassis {
        Chassis {
            y: ground.add(r.clearance),
            vy: Fx::ZERO,
            up: V3::UP,
            w: V3::ZERO,
            load: Fx::ONE,
        }
    }

    /// Есть ли вообще опора.
    pub fn grounded(&self) -> bool {
        self.load.raw() > 0
    }
}

/// Один шаг физики корпуса.
///
/// `ground[i]` — высота земли под опорой `supports[i]`; их длины
/// обязаны совпадать. `heading` — единичный вектор курса `(cos, sin)`:
/// тригонометрия остаётся снаружи, физике нужен готовый поворот.
///
/// `sway` — **горизонтальное ускорение корпуса** в мировых осях `x`, `z`
/// (`y` не читается). Центр масс висит на клиренсе над опорами, и
/// сила инерции, приложенная к нему, даёт момент: разгон сажает машину
/// на корму, торможение роняет носом, поворот кренит наружу. Это не
/// отдельные правила — один член `r × F` с плечом в клиренс.
///
/// Порядок обхода опор фиксирован индексом, суммы идут в нём же —
/// иначе параллельная редукция дала бы другой результат (`CLAUDE.md`).
#[allow(clippy::too_many_arguments)]
pub fn step(
    c: &mut Chassis,
    supports: &[Support],
    ground: &[Fx],
    heading: (Fx, Fx),
    sway: V3,
    r: &Ride,
    dt: Fx,
) {
    debug_assert_eq!(supports.len(), ground.len(), "опор и высот поровну");
    let (hc, hs) = heading;
    let (mut fy, mut tx, mut tz) = (Fx::ZERO, Fx::ZERO, Fx::ZERO);
    let mut touching = 0i32;

    // Сила инерции `−a`, приложенная в центре масс на высоте клиренса:
    // `r × F` при `r = (0, h, 0)` и `F = (−a_x, 0, −a_z)` даёт
    // `(−h·a_z, 0, h·a_x)`. Знак проверен тестами на разгон и поворот,
    // а не выписан по памяти.
    tx = tx.sub(r.clearance.mul(sway.z));
    tz = tz.add(r.clearance.mul(sway.x));

    for (s, g) in supports.iter().zip(ground.iter()) {
        // Местные координаты опоры — в мировые. Поворот только вокруг
        // вертикали: наклон корпуса учитывается ниже, через нормаль.
        let rx = s.x.mul(hc).sub(s.z.mul(hs));
        let rz = s.x.mul(hs).add(s.z.mul(hc));
        // Высота точки корпуса над центром масс. Точное выражение —
        // `−(up·r)/up.y`; делитель опущен намеренно: при наклоне
        // до тридцати градусов он больше 0.86, а деление на опору —
        // это деление на каждую опору каждого юнита каждый тик.
        let lift = c.up.x.mul(rx).add(c.up.z.mul(rz)).neg();
        let foot = c.y.add(lift).sub(r.clearance);
        // Сжатие, сверху ограниченное ходом; отрицательное — отбой.
        // Опора **касается** земли, если пол не дальше хода подвески
        // под ступнёй: пружина не только сжимается, но и **вытягивается**
        // на тот же ход, и клюнувший носом танк кормой с земли не
        // отрывается — задние катки едут, распрямив подвеску. Обрыв
        // под опорой — это пол на метры ниже, и он не считается.
        let ext = g.sub(foot).min(r.travel);
        if ext.raw() >= r.travel.neg().raw() {
            touching += 1;
        }
        // Пружина **только толкает**: подвеска не притягивает машину
        // к земле. С натягом она толкает и в отбое — пока натяг
        // перевешивает растяжение; без натяга — только при сжатии.
        let spring = r.stiffness.mul(ext).add(r.preload);
        if ext.raw() < r.travel.neg().raw() || spring.raw() <= 0 {
            continue;
        }
        // Скорость точки по вертикали: подъём центра масс плюс вклад
        // вращения, `(ω × r).y`.
        let vp = c.vy.add(c.w.z.mul(rx)).sub(c.w.x.mul(rz));
        // **Демпфер не разворачивает скорость за один шаг.** Сила,
        // пропорциональная скорости, при шаге в тридцатую секунды
        // на жёсткой посадке разгоняет корпус вверх сильнее, чем
        // тяготение разогнало вниз, — и машина улетает за карту.
        // Предел читается физически: удар неупругий, и демпферу
        // хватит ровно того, чтобы погасить скорость точки в ноль.
        // На опору приходится доля всей массы, отсюда деление
        // на число опор.
        let cap = vp
            .abs()
            .div(dt)
            .div(Fx::from_int(supports.len().max(1) as i32));
        // Предел **по модулю**, а не только вниз: несимметричный
        // демпфер гасит подъём сильнее, чем падение, и подвеска
        // входит в незатухающий цикл вместо покоя.
        let damp = r.damping.mul(vp.neg()).max(cap.neg()).min(cap);
        let f = spring.add(damp).max(Fx::ZERO);
        fy = fy.add(f);
        // Момент `r × F` при вертикальной силе: `(−rz·f, 0, rx·f)`.
        tx = tx.sub(rz.mul(f));
        tz = tz.add(rx.mul(f));
    }

    // Общего потолка на силу здесь **нет**, и это поправка (ADR-234):
    // потолок `fy ≤ g + |vy|/dt` держал удар, но вместе с ним держал
    // и натяг — корпус, опустившийся ниже клиренса, оставался внизу
    // навсегда, потому что любая скорость вниз, хоть на единицу
    // младшего разряда, срезала подъёмную силу ровно до веса.
    // Удар теперь гасит демпфер, ограниченный на своей опоре.

    // Подъём. Полунеявная схема: сначала скорость, потом положение —
    // явная на пружине расходится уже на умеренной жёсткости.
    c.vy = c.vy.add(fy.sub(r.gravity).mul(dt));
    c.y = c.y.add(c.vy.mul(dt));

    // Вращение. Демпфирование множителем, а не вычитанием: вычитание
    // при большом шаге меняет знак скорости и раскачивает корпус.
    let damp = Fx::ONE.sub(r.angular_damping.mul(dt)).max(Fx::ZERO);
    c.w.x = c.w.x.add(tx.div(r.inertia).mul(dt)).mul(damp);
    c.w.z = c.w.z.add(tz.div(r.inertia).mul(dt)).mul(damp);

    // Поворот нормали угловой скоростью и возврат её на единичную
    // длину. Порядок важен: нормировать до поворота значит нормировать
    // прошлый шаг.
    c.up = c.up.add(c.w.cross(c.up).scale(dt)).renormalised();

    // Предел завала. Горизонтальная часть нормали прижимается к конусу,
    // и вместе с ней гасится вращение: иначе пружина каждый тик
    // толкала бы корпус за предел, а ограничение возвращало — и машина
    // дрожала бы на месте.
    if c.up.y < r.min_up {
        let h2 = c.up.x.mul(c.up.x).add(c.up.z.mul(c.up.z));
        if h2.raw() > 0 {
            // Корень считается **только при завале за предел**, то есть
            // на уступе, а не каждый тик у каждой машины.
            let want = Fx::ONE.sub(r.min_up.mul(r.min_up)).max(Fx::ZERO).sqrt();
            let k = want.div(h2.sqrt());
            c.up = V3::new(c.up.x.mul(k), r.min_up, c.up.z.mul(k));
            c.w = V3::ZERO;
        }
    }

    // Опора наружу отдаётся **долей касающихся точек**, а не суммой сил.
    // Сумма сил у корпуса, подвешенного на среднем поле (так его зовёт
    // фаза хода), в покое ровно ноль — и «стоит на земле» читалось бы
    // как «висит в воздухе». Доля 0..1 честна в обоих случаях: четыре
    // опоры на земле — единица, две над обрывом — половина, полёт — ноль.
    c.load = if supports.is_empty() {
        Fx::ZERO
    } else {
        Fx::from_ratio(touching, supports.len() as i32)
    };

    // Позиционная поправка: ни одна опора не уходит под землю глубже
    // хода подвески. Без неё большой шаг по времени протыкает рельеф,
    // и машина проваливается насквозь — пружина не успевает.
    //
    // **Не больше хода за тик.** Уступ выше хода подвески иначе
    // подбрасывал бы корпус на всю разницу одним тиком — ровно тот
    // прыжок, который высота корпуса как состояние призвана убрать
    // (ADR-234). Подвеска распрямляется не быстрее своего хода
    // за тик: уступ в полметра при ходе в двадцать два сантиметра
    // берётся за три тика, а не за один.
    let mut sink = Fx::ZERO;
    for (s, g) in supports.iter().zip(ground.iter()) {
        let rx = s.x.mul(hc).sub(s.z.mul(hs));
        let rz = s.x.mul(hs).add(s.z.mul(hc));
        let lift = c.up.x.mul(rx).add(c.up.z.mul(rz)).neg();
        let foot = c.y.add(lift).sub(r.clearance);
        sink = sink.max(g.sub(r.travel).sub(foot));
    }
    if sink.raw() > 0 {
        // A falling body sweeps the floor within this tick. Limiting this
        // correction to suspension travel lets a fast descent tunnel through
        // it. A supported body climbing laterally still extends gradually.
        let correction = if touching == 0 && c.vy < Fx::ZERO {
            sink
        } else {
            sink.min(r.settle.min(r.travel))
        };
        c.y = c.y.add(correction);
        c.vy = c.vy.max(Fx::ZERO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ходовая гусеничной машины: четыре опоры по углам.
    fn tracked() -> (Vec<Support>, Ride) {
        let h = Fx::from_ratio(3, 1);
        let w = Fx::from_ratio(3, 2);
        let s = vec![
            Support::new(h, w),
            Support::new(h, w.neg()),
            Support::new(h.neg(), w),
            Support::new(h.neg(), w.neg()),
        ];
        let r = Ride {
            stiffness: Fx::from_int(40),
            damping: Fx::from_int(8),
            travel: Fx::from_ratio(1, 2),
            clearance: Fx::from_ratio(1, 2),
            // Радиус инерции порядка половины длины машины.
            inertia: Fx::from_int(4),
            gravity: Fx::from_ratio(98, 10),
            preload: Fx::ZERO,
            angular_damping: Fx::from_int(3),
            min_up: Fx::from_ratio(87, 100),
            settle: Fx::from_ratio(1, 2),
        };
        (s, r)
    }

    /// Та же ходовая с натягом под вес: в покое корпус стоит ровно
    /// на клиренсе, а не на клиренсе минус просадка.
    fn preloaded() -> (Vec<Support>, Ride) {
        let (s, mut r) = tracked();
        r.stiffness = Fx::from_int(2);
        r.damping = Fx::ONE;
        r.preload = r.gravity.div(Fx::from_int(s.len() as i32));
        (s, r)
    }

    /// Та же ходовая с **мягкой** подвеской — для проверок динамики.
    ///
    /// На жёсткой (`stiffness` 40 при плече 3) торможение в 6 м/с²
    /// наклоняет корпус на `h·a / (k·Σr²)` = 3/(40·36) ≈ 0.002 рад —
    /// десятую долю градуса. Знак там верен, а величины нет: физика
    /// честная, но невидимая. Мягкая подвеска даёт ту же формулу
    /// с заметным ответом, и именно её значения ушли в данные (ADR-225).
    fn soft() -> (Vec<Support>, Ride) {
        let (s, mut r) = tracked();
        r.stiffness = Fx::from_int(2);
        r.damping = Fx::ONE;
        (s, r)
    }

    const DT: Fx = Fx::from_ratio(1, 30);
    const FLAT: Fx = Fx::ZERO;

    #[test]
    fn an_airborne_body_cannot_step_through_the_floor_at_high_speed() {
        let (sup, r) = preloaded();
        let mut c = Chassis::resting(Fx::ZERO, &r);
        c.y = Fx::from_int(2);
        c.vy = Fx::from_int(-180);
        c.load = Fx::ZERO;
        step(
            &mut c,
            &sup,
            &[Fx::ZERO; 4],
            (Fx::ONE, Fx::ZERO),
            V3::ZERO,
            &r,
            DT,
        );
        assert!(
            c.y >= r.clearance.sub(r.travel),
            "fast descent passed through the floor: {:?}",
            c.y
        );
        assert!(c.vy >= Fx::ZERO);
    }

    fn level(n: usize) -> Vec<Fx> {
        vec![FLAT; n]
    }

    /// **Машина, брошенная над землёй, встаёт на неё и остаётся.**
    ///
    /// Это главная проверка: пружина, которая не держит вес, роняет
    /// машину под землю, а слишком жёсткая — подбрасывает. Обе беды
    /// видно только в движении, то есть на снимке их нет.
    #[test]
    fn a_machine_dropped_on_flat_ground_settles_and_stays() {
        let (s, r) = tracked();
        let mut c = Chassis::resting(FLAT, &r);
        // Бросаем с двух метров.
        c.y = c.y.add(Fx::from_int(2));
        let g = level(s.len());
        for _ in 0..600 {
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
        }
        // Осела и стоит: скорость около нуля, наклона нет.
        assert!(
            c.vy.abs() < Fx::from_ratio(1, 100),
            "качается: vy = {:?}",
            c.vy
        );
        assert!(c.w.x.abs() < Fx::from_ratio(1, 100), "качается по тангажу");
        assert!(c.w.z.abs() < Fx::from_ratio(1, 100), "качается по крену");
        // Стоит **на** земле, а не в ней и не над ней: центр масс
        // ниже клиренса ровно на просадку под собственным весом.
        // Жёсткость задана **на опору**, поэтому вес делится на все.
        let squash = r.gravity.div(r.stiffness.mul(Fx::from_int(s.len() as i32)));
        let want = FLAT.add(r.clearance).sub(squash);
        assert!(
            c.y.sub(want).abs() < Fx::from_ratio(1, 50),
            "высота {:?}, ждали около {:?}",
            c.y,
            want
        );
        assert!(c.grounded(), "опоры нет, хотя машина стоит на земле");
    }

    /// **Без опоры машина падает, и падает с ускорением.**
    #[test]
    fn with_no_ground_it_falls_and_keeps_accelerating() {
        let (s, r) = tracked();
        let mut c = Chassis::resting(FLAT, &r);
        // Земля глубоко внизу: опоры не достают.
        let g = vec![Fx::from_int(-100); s.len()];
        let mut prev = Fx::ZERO;
        for k in 0..60 {
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
            if k > 0 {
                assert!(
                    c.vy < prev,
                    "падение не разгоняется: {:?} после {:?}",
                    c.vy,
                    prev
                );
            }
            prev = c.vy;
        }
        assert!(!c.grounded(), "в воздухе нашлась опора");
        // За две секунды свободного падения — около `g·t²/2`.
        assert!(
            c.y < Fx::from_int(-15),
            "падает слишком медленно: {:?}",
            c.y
        );
    }

    /// **На склоне корпус ложится по склону.**
    ///
    /// Наклон здесь не рисуется и не задаётся: он выходит из того,
    /// что передние опоры выше задних, и пружины толкают их сильнее.
    #[test]
    fn on_a_slope_the_hull_lies_along_it() {
        let (s, r) = tracked();
        let mut c = Chassis::resting(FLAT, &r);
        // Уклон по оси X: на метр длины — четверть метра подъёма.
        let k = Fx::from_ratio(1, 4);
        let g: Vec<Fx> = s.iter().map(|p| p.x.mul(k)).collect();
        for _ in 0..900 {
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
        }
        // Нормаль склона наклонена назад по X ровно на `k`
        // (с точностью до нормировки).
        let want = V3::new(k.neg(), Fx::ONE, Fx::ZERO).renormalised();
        assert!(
            c.up.x.sub(want.x).abs() < Fx::from_ratio(1, 20),
            "нормаль {:?}, ждали около {:?}",
            c.up,
            want
        );
        assert!(c.up.y > Fx::from_ratio(9, 10), "корпус завалился набок");
        assert!(c.w.x.abs() < Fx::from_ratio(1, 50), "не успокоился");
    }

    /// **Нормаль остаётся единичной сколько угодно долго.**
    ///
    /// Поправка первого порядка вместо корня — экономия, и стоит
    /// проверить, что она не уводит длину. Уплывшая нормаль означает
    /// корпус, который медленно врастает в землю или взлетает.
    #[test]
    fn the_hull_normal_never_drifts_off_unit_length() {
        let (s, r) = tracked();
        let mut c = Chassis::resting(FLAT, &r);
        let k = Fx::from_ratio(1, 3);
        let g: Vec<Fx> = s.iter().map(|p| p.x.mul(k).add(p.z.mul(k))).collect();
        for _ in 0..5000 {
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
            let l2 = c.up.len_sq();
            assert!(
                l2.sub(Fx::ONE).abs() < Fx::from_ratio(1, 100),
                "длина нормали {:?}",
                l2
            );
        }
    }

    /// **Поворот машины поворачивает и её реакцию на склон.**
    ///
    /// Тот же склон, машина развёрнута на девяносто градусов — значит
    /// наклоняться она обязана в крен, а не в тангаж. Перепутанный
    /// знак в повороте опор проходит все проверки выше и валится
    /// только здесь.
    #[test]
    fn turning_the_machine_turns_how_it_meets_the_slope() {
        let (s, r) = tracked();
        let k = Fx::from_ratio(1, 4);
        // Склон по мировой оси X. Опоры поворачиваются вместе
        // с курсом, поэтому высоты считаются от мировых координат.
        let run = |heading: (Fx, Fx)| -> V3 {
            let (hc, hs) = heading;
            let mut c = Chassis::resting(FLAT, &r);
            let g: Vec<Fx> = s
                .iter()
                .map(|p| p.x.mul(hc).sub(p.z.mul(hs)).mul(k))
                .collect();
            for _ in 0..900 {
                step(&mut c, &s, &g, heading, V3::ZERO, &r, DT);
            }
            c.up
        };
        let along = run((Fx::ONE, Fx::ZERO));
        let across = run((Fx::ZERO, Fx::ONE));
        // Вдоль курса — наклон по X; поперёк — тот же наклон, но
        // мировая нормаль обязана остаться той же самой: склон-то один.
        assert!(
            along.x < Fx::from_ratio(-1, 10),
            "не наклонился вдоль: {along:?}"
        );
        assert!(
            across.x.sub(along.x).abs() < Fx::from_ratio(1, 10),
            "развернули машину — сменился склон: {across:?} против {along:?}"
        );
    }

    /// **Один и тот же ввод даёт то же состояние до бита.**
    ///
    /// Сторож детерминизма на самом ядре: физика в фиксированной
    /// точке обязана быть повторимой, иначе lockstep не существует.
    #[test]
    fn the_same_input_gives_the_same_state_bit_for_bit() {
        let (s, r) = tracked();
        let k = Fx::from_ratio(1, 5);
        let g: Vec<Fx> = s.iter().map(|p| p.x.mul(k).sub(p.z.mul(k))).collect();
        let run = || {
            let mut c = Chassis::resting(FLAT, &r);
            c.y = c.y.add(Fx::from_int(3));
            for _ in 0..400 {
                step(
                    &mut c,
                    &s,
                    &g,
                    (Fx::from_ratio(3, 5), Fx::from_ratio(4, 5)),
                    V3::ZERO,
                    &r,
                    DT,
                );
            }
            c
        };
        assert_eq!(run(), run());
    }

    /// **Жёсткая подвеска и крупный шаг не взрывают корпус.**
    ///
    /// Явная схема на пружине расходится, и расходится молча: сначала
    /// дрожь, потом машина улетает за карту. Проверка держит границу,
    /// за которой это начинается, видимой.
    #[test]
    fn a_stiff_ride_does_not_blow_up() {
        let (s, mut r) = tracked();
        r.stiffness = Fx::from_int(200);
        r.damping = Fx::from_int(20);
        let mut c = Chassis::resting(FLAT, &r);
        c.y = c.y.add(Fx::from_int(5));
        let g = level(s.len());
        for _ in 0..900 {
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
            assert!(c.y.abs() < Fx::from_int(50), "улетел: {:?}", c.y);
            assert!(c.vy.abs() < Fx::from_int(100), "разогнался: {:?}", c.vy);
        }
        assert!(
            c.vy.abs() < Fx::from_ratio(1, 10),
            "не успокоился: {:?}",
            c.vy
        );
    }

    /// **Торможение роняет носом, разгон сажает на корму.**
    ///
    /// Сила инерции приложена в центре масс на высоте клиренса, и
    /// плечо у неё есть. Знак здесь легко перепутать — и тогда машина
    /// клюёт при разгоне, как будто её тянут за нос.
    #[test]
    fn braking_dives_the_nose_and_throttle_squats_the_tail() {
        let (s, r) = soft();
        let g = level(s.len());
        // Нос смотрит по +X. Торможение — ускорение против хода.
        let run = |ax: Fx| -> Fx {
            let mut c = Chassis::resting(FLAT, &r);
            for _ in 0..300 {
                step(
                    &mut c,
                    &s,
                    &g,
                    (Fx::ONE, Fx::ZERO),
                    V3::new(ax, Fx::ZERO, Fx::ZERO),
                    &r,
                    DT,
                );
            }
            c.up.x
        };
        let brake = run(Fx::from_int(-6));
        let throttle = run(Fx::from_int(6));
        // Верх при торможении уходит к носу (`up.x > 0`), при разгоне —
        // к корме. Величина обязана быть заметной: клевок, который
        // не виден, ничего не стоит и ничего не даёт.
        assert!(
            brake > Fx::from_ratio(1, 40),
            "торможение не клюёт: {brake:?}"
        );
        assert!(
            throttle < Fx::from_ratio(-1, 40),
            "разгон не сажает на корму: {throttle:?}"
        );
    }

    /// **В повороте корпус кренится наружу.**
    ///
    /// Центростремительное ускорение направлено в центр поворота,
    /// сила инерции — наружу, и верх уходит наружу же. Мотоцикл
    /// ложится **внутрь**, но у нас гусеницы, и это ошибка знака,
    /// которую снимок покажет только в динамике.
    #[test]
    fn a_turn_rolls_the_hull_outwards() {
        let (s, r) = soft();
        let g = level(s.len());
        // Ход по +X, центр поворота слева, на +Z: ускорение к нему.
        let mut c = Chassis::resting(FLAT, &r);
        for _ in 0..300 {
            step(
                &mut c,
                &s,
                &g,
                (Fx::ONE, Fx::ZERO),
                V3::new(Fx::ZERO, Fx::ZERO, Fx::from_int(5)),
                &r,
                DT,
            );
        }
        assert!(
            c.up.z < Fx::from_ratio(-1, 40),
            "крен не наружу: {:?}",
            c.up
        );
        assert!(c.up.x.abs() < Fx::from_ratio(1, 100), "поворот дал клевок");
    }

    /// **С натягом корпус стоит ровно на клиренсе** — и на мягкой
    /// пружине, которая без натяга просела бы на метр с лишним.
    #[test]
    fn a_preloaded_ride_rests_at_its_clearance() {
        let (s, r) = preloaded();
        let mut c = Chassis::resting(FLAT, &r);
        c.y = c.y.add(Fx::ONE);
        let g = level(s.len());
        for _ in 0..900 {
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
        }
        assert!(
            c.vy.abs() < Fx::from_ratio(1, 100),
            "качается: vy = {:?}",
            c.vy
        );
        let want = FLAT.add(r.clearance);
        assert!(
            c.y.sub(want).abs() < Fx::from_ratio(1, 50),
            "высота {:?}, ждали клиренс {:?}",
            c.y,
            want
        );
        assert!(c.grounded());
        // Без натяга та же пружина не держит вес: центр масс уходит
        // на просадку `g / (k·n)`, то есть на всю глубину хода.
        let (s, mut soft) = preloaded();
        soft.preload = Fx::ZERO;
        let mut c = Chassis::resting(FLAT, &soft);
        for _ in 0..900 {
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &soft, DT);
        }
        assert!(
            c.y < want.sub(soft.travel).add(Fx::from_ratio(1, 10)),
            "мягкая без натяга не просела: {:?}",
            c.y
        );
    }

    /// **Уступ берётся без прыжка.** Пол под всеми опорами поднялся
    /// на два хода подвески разом — корпус поднимается за несколько
    /// тиков, и ни на одном не больше хода. Это ровно перенос высоты,
    /// который у навигации происходит одним тиком (ADR-234).
    #[test]
    fn a_hull_climbs_a_step_without_a_single_tick_jump() {
        let (s, r) = preloaded();
        let mut c = Chassis::resting(FLAT, &r);
        let flat = level(s.len());
        for _ in 0..300 {
            step(&mut c, &s, &flat, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
        }
        let rise = r.travel.mul(Fx::from_int(2));
        let g: Vec<Fx> = vec![rise; s.len()];
        let mut biggest = Fx::ZERO;
        for _ in 0..300 {
            let before = c.y;
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
            biggest = biggest.max(c.y.sub(before).abs());
        }
        assert!(
            biggest <= r.travel.add(Fx::from_ratio(1, 100)),
            "корпус прыгнул на {biggest:?} за тик при ходе {:?}",
            r.travel
        );
        let want = rise.add(r.clearance);
        assert!(
            c.y.sub(want).abs() < Fx::from_ratio(1, 20),
            "после уступа корпус на {:?}, ждали {:?}",
            c.y,
            want
        );
    }

    /// **Опора не уходит под землю глубже хода подвески.**
    ///
    /// Позиционная поправка существует ради крупного шага: пружина
    /// за тик не успевает вытолкнуть машину, упавшую с высоты,
    /// и без поправки она проваливается сквозь рельеф.
    #[test]
    fn a_foot_never_sinks_deeper_than_the_travel() {
        let (s, r) = tracked();
        let mut c = Chassis::resting(FLAT, &r);
        c.y = c.y.add(Fx::from_int(30));
        let g = level(s.len());
        for _ in 0..600 {
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), V3::ZERO, &r, DT);
            let foot = c.y.sub(r.clearance);
            assert!(
                foot > FLAT.sub(r.travel).sub(Fx::from_ratio(1, 100)),
                "опора на {:?} при ходе {:?}",
                foot,
                r.travel
            );
        }
    }
}
