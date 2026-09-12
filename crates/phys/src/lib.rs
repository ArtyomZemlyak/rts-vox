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
    pub const ZERO: V3 = V3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ZERO };
    /// Вертикаль: «верх» у невозмущённого корпуса.
    pub const UP: V3 = V3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO };

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
    /// Угловое трение. Без него корпус качается вечно: подвеска гасит
    /// подъём, но не вращение вокруг центра масс.
    pub angular_damping: Fx,
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
    /// Сумма нормальных реакций прошлого шага, отнесённая к массе.
    ///
    /// Это и есть сцепление: тяга обязана быть не больше
    /// `friction × load`, и машина в воздухе не разгоняется.
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
            load: r.gravity,
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
/// Порядок обхода опор фиксирован индексом, суммы идут в нём же —
/// иначе параллельная редукция дала бы другой результат (`CLAUDE.md`).
pub fn step(c: &mut Chassis, supports: &[Support], ground: &[Fx], heading: (Fx, Fx), r: &Ride, dt: Fx) {
    debug_assert_eq!(supports.len(), ground.len(), "опор и высот поровну");
    let (hc, hs) = heading;
    let (mut fy, mut tx, mut tz) = (Fx::ZERO, Fx::ZERO, Fx::ZERO);

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
        // Сжатие. Пружина **только толкает**: подвеска не притягивает
        // машину к земле, и отрицательное сжатие означает «опора
        // в воздухе», а не «сила вниз».
        let squash = g.sub(foot).max(Fx::ZERO).min(r.travel);
        if squash.raw() == 0 {
            continue;
        }
        // Скорость точки по вертикали: подъём центра масс плюс вклад
        // вращения, `(ω × r).y`.
        let vp = c.vy.add(c.w.z.mul(rx)).sub(c.w.x.mul(rz));
        let f = r.stiffness.mul(squash).sub(r.damping.mul(vp)).max(Fx::ZERO);
        fy = fy.add(f);
        // Момент `r × F` при вертикальной силе: `(−rz·f, 0, rx·f)`.
        tx = tx.sub(rz.mul(f));
        tz = tz.add(rx.mul(f));
    }

    // **Земля останавливает падение, но не выстреливает корпус
    // обратно быстрее, чем он в неё влетел.** Демпфер даёт силу,
    // пропорциональную скорости, и на жёсткой посадке при шаге
    // в тридцатую секунды она за один тик разгоняет машину вверх
    // сильнее, чем тяготение разогнало вниз, — корпус улетает
    // за карту. Ограничение читается физически: удар неупругий,
    // подвеска съедает энергию.
    //
    // Действует **только на падении**: на подъёме пружина свободна,
    // иначе подвеска перестала бы отыгрывать вовсе.
    if c.vy.raw() < 0 {
        fy = fy.min(r.gravity.add(c.vy.neg().div(dt)));
    }

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

    c.load = fy;

    // Позиционная поправка: ни одна опора не уходит под землю глубже
    // хода подвески. Без неё большой шаг по времени протыкает рельеф,
    // и машина проваливается насквозь — пружина не успевает.
    let mut sink = Fx::ZERO;
    for (s, g) in supports.iter().zip(ground.iter()) {
        let rx = s.x.mul(hc).sub(s.z.mul(hs));
        let rz = s.x.mul(hs).add(s.z.mul(hc));
        let lift = c.up.x.mul(rx).add(c.up.z.mul(rz)).neg();
        let foot = c.y.add(lift).sub(r.clearance);
        sink = sink.max(g.sub(r.travel).sub(foot));
    }
    if sink.raw() > 0 {
        c.y = c.y.add(sink);
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
            angular_damping: Fx::from_int(3),
            min_up: Fx::from_ratio(87, 100),
        };
        (s, r)
    }

    const DT: Fx = Fx::from_ratio(1, 30);
    const FLAT: Fx = Fx::ZERO;

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
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), &r, DT);
        }
        // Осела и стоит: скорость около нуля, наклона нет.
        assert!(c.vy.abs() < Fx::from_ratio(1, 100), "качается: vy = {:?}", c.vy);
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
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), &r, DT);
            if k > 0 {
                assert!(c.vy < prev, "падение не разгоняется: {:?} после {:?}", c.vy, prev);
            }
            prev = c.vy;
        }
        assert!(!c.grounded(), "в воздухе нашлась опора");
        // За две секунды свободного падения — около `g·t²/2`.
        assert!(c.y < Fx::from_int(-15), "падает слишком медленно: {:?}", c.y);
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
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), &r, DT);
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
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), &r, DT);
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
                step(&mut c, &s, &g, heading, &r, DT);
            }
            c.up
        };
        let along = run((Fx::ONE, Fx::ZERO));
        let across = run((Fx::ZERO, Fx::ONE));
        // Вдоль курса — наклон по X; поперёк — тот же наклон, но
        // мировая нормаль обязана остаться той же самой: склон-то один.
        assert!(along.x < Fx::from_ratio(-1, 10), "не наклонился вдоль: {along:?}");
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
                step(&mut c, &s, &g, (Fx::from_ratio(3, 5), Fx::from_ratio(4, 5)), &r, DT);
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
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), &r, DT);
            assert!(c.y.abs() < Fx::from_int(50), "улетел: {:?}", c.y);
            assert!(c.vy.abs() < Fx::from_int(100), "разогнался: {:?}", c.vy);
        }
        assert!(c.vy.abs() < Fx::from_ratio(1, 10), "не успокоился: {:?}", c.vy);
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
            step(&mut c, &s, &g, (Fx::ONE, Fx::ZERO), &r, DT);
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
