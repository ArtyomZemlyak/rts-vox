//! Ход машины: скорость, курс и угловая скорость — состояние,
//! а не мгновенный вектор (ADR-220, ADR-234).
//!
//! До этого юнит получал желаемую скорость и **немедленно** ею
//! двигался: направление менялось на прямой угол за тик, скорость —
//! от нуля до предельной и обратно за тик. Поле направлений
//! четырёхстороннее, поэтому диагональный путь выглядел лестницей,
//! а не ездой.
//!
//! Здесь машина: у неё есть курс, который доворачивается угловой
//! скоростью, и скорость вдоль курса. Оба меняются **с пределами,
//! выведенными из тяги и массы**: разгон — тяга на массу, доворот
//! набирается с угловым ускорением от тяги на плече борта и момента
//! инерции. Крупная машина того же вида разгоняется и поворачивает
//! медленнее — не потому, что ей записали другое число, а потому,
//! что она тяжелее. Если цель дальше `pivot` от курса, машина входит
//! в разворот: гусеничная — дугой, на доле хода; шагающая — стоя.
//!
//! Модуль чистый нарочно: ни мира, ни навигации, ни соседей. Всё, что
//! он умеет, проверяется headless одной функцией.

use fx::angle;
use fx::Fx;
use modding::Chassis;

/// «Ехать некуда»: приказа на ход нет, машина тормозит и стоит.
///
/// Не `Option<i32>`, потому что это значение живёт в буфере фазы
/// движения по слоту на юнита, а `Option` там стоил бы выравнивания
/// на ровном месте.
pub const NO_WAY: i32 = i32::MIN;

/// Что фаза движения посчитала одному юниту за тик.
///
/// Две разные вещи, и разделены они нарочно (ADR-220). **Тяга** —
/// это `way`, желаемый курс: она идёт через трансмиссию, то есть
/// через предельный доворот и предельное ускорение. **Толчок** —
/// это расталкивание соседей: оно приложено к телу напрямую, потому
/// что контакт тел не тяга, и машина, которую подпирают в бок,
/// сдвигается вбок, а не разворачивается.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    /// Желаемый курс, двоичный угол, или [`NO_WAY`].
    pub way: i32,
    /// Толчок соседей, мировых единиц в секунду по осям.
    pub push_x: i32,
    pub push_z: i32,
}

impl Default for Want {
    /// Умолчание — «ехать некуда», а не «на восток». Ноль здесь
    /// значил бы, что свежий слот буфера трогается сам.
    fn default() -> Want {
        Want {
            way: NO_WAY,
            push_x: 0,
            push_z: 0,
        }
    }
}

/// Состояние хода одного юнита.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Drive {
    /// Курс, двоичный угол `fx::angle`.
    pub head: i32,
    /// Скорость вдоль курса, мировых единиц в секунду.
    pub vel: Fx,
    /// Угловая скорость, двоичных единиц угла **за тик** (ADR-234).
    ///
    /// Состояние, а не предел: доворот набирается и гасится
    /// с угловым ускорением, и машина входит в поворот плавно,
    /// а не рывком на полную угловую скорость первым же тиком.
    pub yaw_rate: i32,
}

/// Пределы хода, уже пересчитанные **на тик** для машины
/// номинальной массы.
///
/// Считаются один раз на фазу по каждому виду ходовой, а не по юниту:
/// это деления в `i128`, и повторять их пять тысяч раз незачем.
/// Машина другой массы получает их через свой множитель
/// ([`drive_factor`]) — одно умножение на юнита.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Прибавка скорости за тик у номинальной машины.
    pub accel: Fx,
    /// Убавка скорости за тик у номинальной машины.
    pub brake: Fx,
    /// Предел угловой скорости, двоичных единиц угла за тик.
    pub turn: i32,
    /// Угловое ускорение у номинальной машины, двоичных единиц
    /// за тик за тик.
    pub turn_acc: Fx,
    /// За этим углом машина входит в разворот, двоичных единиц.
    pub pivot: i32,
    /// Доля предельной скорости в развороте.
    pub pivot_speed: Fx,
}

/// Мировых единиц в метре: ребро мирового воксела — из тела, а не
/// из постоянной (ADR-219).
fn units_per_metre() -> i32 {
    (1000 / voxel::unit::WORLD_MM as i32).max(1)
}

/// Полный оборот в радианах, `710/113`: ошибка `8.5e-8`, и дробь
/// точная в `Fx` — десятичная запись `6.2831…` точной не была бы.
const TAU: Fx = Fx::from_ratio(710, 113);

/// Cadence from accepted translation plus support travel along a turning arc.
/// Derived once per chassis/phase. No per-unit square root or new state.
#[derive(Clone, Copy)]
pub(crate) struct StepCycle {
    linear: Fx,
    turn: Fx,
}
impl StepCycle {
    pub fn of(c: &Chassis) -> Option<Self> {
        c.gait.map(|g| {
            let cm_per_unit = (voxel::unit::WORLD_MM as i32 / 10).max(1);
            let x = Fx::from_int(c.support_x_cm);
            let z = Fx::from_int(c.support_z_cm);
            let radius_cm = x.mul(x).add(z.mul(z)).sqrt();
            Self {
                linear: Fx::from_ratio(angle::FULL * cm_per_unit, g.stride_cm.max(1)),
                // Both heading and phase use the same binary full turn.
                turn: TAU.mul(radius_cm).div(Fx::from_int(g.stride_cm.max(1))),
            }
        })
    }
    pub fn advance(self, path: Fx, yaw_step: i32) -> i32 {
        path.abs()
            .mul(self.linear)
            .add(Fx::from_int(yaw_step.abs()).mul(self.turn))
            .to_int_trunc()
    }
}

impl Limits {
    /// Перевести числа ходовой в пределы за тик.
    ///
    /// Деление целое: в данных живут целые, а за тик выходит дробь
    /// (ADR-173). Тяга на массу — метры в секунду за секунду, дальше
    /// в мировые единицы и на тик. Угловое ускорение — тяга на плече
    /// борта, делённая на массу и квадрат радиуса инерции: радианы
    /// в секунду за секунду, дальше в двоичные единицы и на квадрат
    /// тика. Доворот меньше одной двоичной единицы за тик означал бы
    /// машину, которая не поворачивается вовсе, поэтому он снизу
    /// ограничен единицей.
    pub fn of(c: &Chassis, ticks_per_second: i32) -> Limits {
        let tps = ticks_per_second.max(1);
        let upm = units_per_metre();
        let mass = c.mass.max(1);
        let per_tick = |kn: i32| -> Fx { Fx::from_ratio(kn * upm, mass * tps) };
        // Плечо борта и радиус инерции — в мировых единицах; сантиметры
        // делятся на ребро воксела в сантиметрах.
        let cm_per_unit = (voxel::unit::WORLD_MM as i32 / 10).max(1);
        let arm = Fx::from_ratio(c.support_z_cm, cm_per_unit);
        let k2 = Fx::from_int(c.inertia.max(1));
        let turn_acc = Fx::from_ratio(c.thrust * upm, mass)
            .mul(arm)
            .div(k2)
            .mul(Fx::from_ratio(angle::FULL, 1).div(TAU))
            .div(Fx::from_int(tps * tps));
        Limits {
            accel: per_tick(c.thrust),
            brake: per_tick(c.brake),
            turn: ((c.turn as i64 * angle::FULL as i64) / (360 * tps as i64)).max(1) as i32,
            turn_acc,
            pivot: ((c.pivot as i64 * angle::FULL as i64) / 360) as i32,
            pivot_speed: Fx::from_ratio(c.pivot_speed.clamp(0, 100), 100),
        }
    }
}

/// Множитель хода машины массой `mass_kg` при номинальной `nominal_kg`:
/// кубический корень отношения масс.
///
/// Тяга ходовой растёт с площадью опоры, масса — с объёмом; при
/// том же виде ходовой тяга тяжёлой машины больше в степени две
/// трети от её массы, и разгон падает как корень кубический
/// из отношения масс (ADR-234). Считается при спавне, а не в тике:
/// корень — последнее средство (ADR-071), и здесь он один на машину.
pub fn drive_factor(nominal_kg: i64, mass_kg: i64) -> Fx {
    let (n, m) = (nominal_kg.max(1) as u128, mass_kg.max(1) as u128);
    // Корень из отношения, масштабированного на 10⁹: тысячные доли
    // в ответе, целая арифметика по дороге.
    let scaled = n * 1_000_000_000 / m;
    Fx::from_ratio(icbrt(scaled).min(i32::MAX as u128) as i32, 1000)
}

/// Целый кубический корень, округлённый вниз. Двоичный поиск:
/// сорок с небольшим шагов на 128 битах.
fn icbrt(v: u128) -> u128 {
    let (mut lo, mut hi) = (0u128, 1u128 << 43);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if mid.saturating_mul(mid).saturating_mul(mid) <= v {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// Шаг за тик по осям, мировых единиц.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub dx: Fx,
    pub dz: Fx,
}

/// Largest current-step speed whose full braking sequence fits `distance`.
/// Includes this tick's movement, then v-b, v-2b, ... until rest. Inverting
/// that sum avoids both a square root and a proportional crawl near a goal.
pub(crate) fn stopping_speed(distance: Fx, brake: Fx, tps: i32, limit: Fx) -> Fx {
    if distance <= Fx::ZERO || brake <= Fx::ZERO || tps <= 0 || limit <= Fx::ZERO {
        return Fx::ZERO;
    }
    let budget = distance.raw() as u128 * tps as u128;
    let b = brake.raw() as u128;
    let cap = limit.raw() as u128;
    let steps = cap.div_ceil(b);
    let tail = |n: u128| b * (n * (n - 1) / 2);
    if steps * cap - tail(steps) <= budget {
        return limit;
    }
    // Find the braking interval containing the answer. Positive i64 inputs
    // keep every product below 2^127, including a one-raw-unit brake.
    let (mut lo, mut hi) = (1, steps);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if b * (mid * (mid + 1) / 2) < budget {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Fx(((budget + tail(lo)) / lo).min(cap) as i64)
}

/// Один тик хода.
///
/// `want` — куда машина хочет ехать (двоичный угол) или [`NO_WAY`].
/// `vmax` — предельная скорость **с учётом уклона**: её считает тот,
/// кто знает рельеф, а не этот модуль.
/// `factor` — множитель хода машины по её массе ([`drive_factor`]);
/// единица у номинальной.
/// `inv_tick` — единица, делённая на частоту тика; передаётся готовой,
/// чтобы не делить на каждом юните.
///
/// Возвращает новое состояние хода и шаг за этот тик.
pub fn advance(
    d: Drive,
    want: i32,
    vmax: Fx,
    l: &Limits,
    factor: Fx,
    inv_tick: Fx,
) -> (Drive, Move) {
    let accel = l.accel.mul(factor);
    let brake = l.brake.mul(factor);
    let alpha = l.turn_acc.mul(factor).to_int_trunc().max(1);
    // Желаемая угловая скорость. Курс не сбрасывается, когда ехать
    // некуда: машина, которой велели стоять, стоит **носом туда, куда
    // стояла**, а не в произвольную сторону; доворот гаснет.
    let (err, wanted) = if want == NO_WAY {
        (0, 0)
    } else {
        let err = angle::diff(want, d.head);
        // Кривая торможения: самая большая угловая скорость, с которой
        // машина, гася доворот на `alpha` за тик, останавливается не
        // дальше промаха. Дискретная сумма w + (w − α) + … не больше
        // w·(w + α)/(2α), отсюда w = ⌊(√(α² + 8αe) − α)/2⌋ — целый
        // корень, без `Fx`. Машина подходит к курсу без перелёта и без
        // дрожи за конечное число тиков. Прежняя пропорциональная
        // скорость гасла экспонентой: последние двадцать градусов
        // разворота занимали больше времени, чем первые сорок, а
        // корпус трогается только на курсе (анализ 240, B1).
        // И не быстрее, чем сам промах: лёгкая машина с большим
        // ускорением иначе проскакивала бы цель за один тик.
        let e = u64::from(err.unsigned_abs());
        let a = alpha as u64;
        // Точная дискретная сумма w + (w − α) + … по положительным членам;
        // формула выше занижает её не больше чем на α/8, и шаг вниз
        // находит самую большую скорость, у которой сумма не больше промаха.
        let stop_sum = |w: u64| {
            let (k, r) = (w / a, w % a);
            a * k * (k + 1) / 2 + if r == 0 { 0 } else { (k + 1) * r }
        };
        // `stop_sum` grows with w, so the largest w at or below the
        // estimate whose sum fits is found by bisection: bounded work per
        // hull and tick, whatever the ratio of error to acceleration.
        let (mut lo, mut hi) = (0u64, ((a * a + 8 * a * e).isqrt() - a) / 2);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if stop_sum(mid) <= e {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let brake_rate = lo;
        let wanted = (brake_rate.min(e).min(l.turn.max(0) as u64) as i32) * err.signum();
        (err, wanted)
    };
    let rate = d
        .yaw_rate
        .saturating_add((wanted - d.yaw_rate).clamp(-alpha, alpha))
        .clamp(-l.turn, l.turn);
    let head = angle::norm(d.head.wrapping_add(rate));
    // Цель по скорости. Остаток промаха после доворота больше порога —
    // разворот: гусеничная идёт дугой на доле хода, шагающая стоит.
    let aim = if want == NO_WAY {
        Fx::ZERO
    } else if err.wrapping_sub(rate).abs() > l.pivot {
        vmax.mul(l.pivot_speed)
    } else {
        vmax
    };
    // Разгон и торможение — разные числа, и это не украшение:
    // машина, тормозящая с ускорением разгона, вкатывается в цель.
    let vel = if aim < Fx::ZERO || d.vel < Fx::ZERO {
        // Reverse uses the same installed motor and brakes. A gear change
        // first brakes to rest; it never flips the old momentum's sign.
        if d.vel > Fx::ZERO && aim < Fx::ZERO {
            d.vel.sub(brake).max(Fx::ZERO)
        } else if d.vel < Fx::ZERO && aim > Fx::ZERO {
            d.vel.add(brake).min(Fx::ZERO)
        } else {
            let change = if aim.abs() > d.vel.abs() {
                accel
            } else {
                brake
            };
            d.vel.add(aim.sub(d.vel).max(change.neg()).min(change))
        }
    } else if aim.raw() > d.vel.raw() {
        Fx::min(aim, d.vel.add(accel))
    } else {
        Fx::max(aim, d.vel.sub(brake))
    };
    // A lower powered speed (slope or engine damage) is a braking target,
    // not an impulse that erases momentum in one tick.
    let step = vel.mul(inv_tick);
    (
        Drive {
            head,
            vel,
            yaw_rate: rate,
        },
        Move {
            dx: step.mul(angle::cos(head)),
            dz: step.mul(angle::sin(head)),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICKS_PER_SECOND;

    fn tracked() -> Chassis {
        crate::test_tuning().tracked
    }

    fn walking() -> Chassis {
        crate::test_tuning().walking
    }

    fn limits() -> Limits {
        Limits::of(&tracked(), TICKS_PER_SECOND)
    }

    fn inv_tick() -> Fx {
        Fx::from_ratio(1, TICKS_PER_SECOND)
    }

    #[test]
    fn stopping_speed_matches_exhaustive_discrete_braking() {
        // Independent oracle: sum every positive velocity, not the inverse
        // formula. Raw units also exercise rounding at each interval edge.
        for b in [1, 3, 11, 40] {
            for cap in [1, 7, 39, 121] {
                for tps in [1, 7, 30] {
                    for distance in 0..150 {
                        let mut expected = 0;
                        for speed in 1..=cap {
                            let (mut v, mut sum) = (speed, 0);
                            while v > 0 {
                                sum += v;
                                v -= b;
                            }
                            if sum <= distance * tps {
                                expected = speed;
                            }
                        }
                        assert_eq!(
                            stopping_speed(Fx(distance), Fx(b), tps as i32, Fx(cap)),
                            Fx(expected),
                            "distance={distance}, brake={b}, tps={tps}, cap={cap}"
                        );
                    }
                }
            }
        }
        assert_eq!(stopping_speed(Fx::ONE, Fx::ZERO, 30, Fx::MAX), Fx::ZERO);
        assert_eq!(stopping_speed(Fx::ONE, Fx::ONE, 0, Fx::MAX), Fx::ZERO);
        assert_eq!(stopping_speed(Fx::MIN, Fx::ONE, 30, Fx::MAX), Fx::ZERO);
        assert_eq!(stopping_speed(Fx::MAX, Fx::MAX, i32::MAX, Fx::MAX), Fx::MAX);
        let v = stopping_speed(Fx::MAX, Fx(1), 30, Fx::MAX).raw() as u128;
        let budget = i64::MAX as u128 * 30;
        assert!(v * (v + 1) / 2 <= budget);
        assert!((v + 1) * (v + 2) / 2 > budget);
    }

    #[test]
    fn discrete_braking_arrives_without_overshoot_or_erasing_momentum() {
        let l = limits();
        for factor in [Fx::from_ratio(1, 3), Fx::ONE, Fx::from_int(2)] {
            for distance in [1, 5, 20, 80] {
                let mut remaining = Fx::from_int(distance);
                let mut d = Drive::default();
                for _ in 0..300 {
                    let target = stopping_speed(
                        remaining,
                        l.brake.mul(factor),
                        TICKS_PER_SECOND,
                        Fx::from_int(40),
                    );
                    let (next, step) = advance(d, 0, target, &l, factor, inv_tick());
                    assert!(next.vel >= d.vel.sub(l.brake.mul(factor)));
                    assert!(next.vel <= d.vel.add(l.accel.mul(factor)));
                    remaining = remaining.sub(step.dx);
                    assert!(remaining >= Fx::ZERO, "overshot by {remaining:?}");
                    d = next;
                }
                assert!(
                    remaining < Fx::from_ratio(1, 1000),
                    "stopped short: {remaining:?}"
                );
                assert!(d.vel < Fx::from_ratio(1, 1000));
            }
        }
    }

    /// Разгон номинальной машины за тик, мировых единиц в секунду:
    /// тяга на массу — метры в секунду за секунду, дальше в единицы
    /// и на тик.
    fn accel_per_tick(c: &Chassis) -> Fx {
        Fx::from_ratio(
            c.thrust * (1000 / voxel::unit::WORLD_MM as i32),
            c.mass * TICKS_PER_SECOND,
        )
    }

    /// **Машина разгоняется, а не прыгает.** Первый тик не даёт
    /// предельной скорости, и разгон занимает столько, сколько
    /// следует из тяги и массы.
    #[test]
    fn a_machine_takes_time_to_reach_its_speed() {
        let l = limits();
        let vmax = Fx::from_int(30);
        let mut d = Drive::default();
        let (first, _) = advance(d, 0, vmax, &l, Fx::ONE, inv_tick());
        assert!(first.vel.raw() > 0, "машина не тронулась вовсе");
        assert!(
            first.vel.raw() < vmax.raw(),
            "первый же тик дал предельную скорость"
        );
        let mut ticks = 0;
        while d.vel.raw() < vmax.raw() && ticks < 1000 {
            let (n, _) = advance(d, 0, vmax, &l, Fx::ONE, inv_tick());
            d = n;
            ticks += 1;
        }
        // Тридцать единиц при разгоне «тяга на массу»: число тиков —
        // предел на прибавку за тик, плюс один на последнюю двоичную
        // долю.
        let want = (vmax.raw() / accel_per_tick(&tracked()).raw()) as i32;
        assert!(
            ticks == want || ticks == want + 1,
            "разгон занял {ticks} тиков вместо {want}"
        );
    }

    /// **Тяжёлая машина разгоняется медленнее лёгкой при той же тяге.**
    ///
    /// Это и есть смысл массы в данных: вид ходовой один, тело
    /// в восемь раз тяжелее — разгон вдвое хуже (корень кубический),
    /// торможение тоже.
    #[test]
    fn a_heavier_machine_accelerates_slower_on_the_same_thrust() {
        let l = limits();
        let vmax = Fx::from_int(30);
        let nominal = tracked().mass as i64 * 1000;
        let heavy = drive_factor(nominal, nominal * 8);
        assert!(
            heavy.sub(Fx::from_ratio(1, 2)).abs() < Fx::from_ratio(1, 100),
            "множитель восьмикратной массы {heavy:?}, ждали половину"
        );
        assert_eq!(
            drive_factor(nominal, nominal),
            Fx::ONE,
            "номинал не единица"
        );
        let run = |factor: Fx| -> i32 {
            let mut d = Drive::default();
            let mut ticks = 0;
            while d.vel.raw() < vmax.raw() && ticks < 10_000 {
                d = advance(d, 0, vmax, &l, factor, inv_tick()).0;
                ticks += 1;
            }
            ticks
        };
        let (light, heavy) = (run(Fx::ONE), run(heavy));
        assert!(
            heavy >= light * 2 - 1 && heavy <= light * 2 + 2,
            "тяжёлая разгонялась {heavy} тиков против {light} у лёгкой"
        );
    }

    /// **Скорость не меняется скачком больше предела** — ни в разгоне,
    /// ни в торможении, ни при смене цели на противоположную.
    #[test]
    fn the_speed_never_jumps_more_than_the_limit_per_tick() {
        let l = limits();
        let vmax = Fx::from_int(30);
        let mut d = Drive::default();
        let plan = [
            (0, 40),
            (angle::HALF, 40),
            (NO_WAY, 20),
            (angle::QUARTER, 40),
        ];
        let (acc, brk) = (accel_per_tick(&tracked()), l.brake);
        for (want, n) in plan {
            for _ in 0..n {
                let (nd, _) = advance(d, want, vmax, &l, Fx::ONE, inv_tick());
                let dv = nd.vel.sub(d.vel);
                assert!(
                    dv.raw() <= acc.raw() + 1,
                    "разгон за тик {dv:?} больше предела {acc:?}"
                );
                assert!(
                    dv.raw() >= -brk.raw() - 1,
                    "торможение за тик {dv:?} больше предела {brk:?}"
                );
                assert!(
                    (nd.yaw_rate - d.yaw_rate).abs() <= l.turn_acc.to_int_trunc().max(1),
                    "доворот сменился рывком: {} → {}",
                    d.yaw_rate,
                    nd.yaw_rate
                );
                d = nd;
            }
        }
    }

    /// **Гусеничная разворачивается дугой, шагающая — на месте.**
    ///
    /// Цель позади. Гусеничная не встаёт: идёт на доле хода, пока
    /// доворачивает, и поэтому смещается. Шагающая стоит и крутится,
    /// пока промах больше порога разворота (`pivot`), — и только
    /// когда цель уже впереди, трогается. Мерить путь дальше порога
    /// нечего: там едут обе, и разница видов не в этом.
    #[test]
    fn tracked_turns_on_an_arc_and_walking_turns_in_place() {
        let vmax = Fx::from_int(20);
        let back = angle::HALF;
        // Путь и скорость на **развороте** — до того, как машина
        // выйдет в створ порога.
        let run = |c: &Chassis| -> (Fx, Fx, i32) {
            let l = Limits::of(c, TICKS_PER_SECOND);
            let mut d = Drive {
                head: 0,
                vel: vmax,
                yaw_rate: 0,
            };
            let mut moved = Fx::ZERO;
            let mut ticks = 0;
            // **Наименьшая** скорость за разворот: шагающая обязана
            // встать, гусеничная — ни разу не остановиться. Это и есть
            // разница видов, и меряется она прямо, а не через путь.
            let mut low = vmax;
            while ticks < 600 {
                let beyond = angle::diff(d.head, back).abs() >= l.pivot;
                let (n, m) = advance(d, back, vmax, &l, Fx::ONE, inv_tick());
                d = n;
                ticks += 1;
                if !beyond {
                    break;
                }
                moved = moved.add(m.dx.abs()).add(m.dz.abs());
                low = Fx::min(low, d.vel);
            }
            (moved, low, ticks)
        };
        let (arc, vel_arc, t_arc) = run(&tracked());
        let (spot, vel_spot, t_spot) = run(&walking());

        assert!(
            arc > Fx::from_int(5),
            "гусеничная развернулась не двигаясь: путь {arc:?}"
        );
        assert!(vel_arc.raw() > 0, "гусеничная встала в развороте");
        // Шагающая гасит ход и стоит; путь — только выбег торможения.
        let stop = vmax.mul(vmax).div(Fx::from_int(2)).div(
            Limits::of(&walking(), TICKS_PER_SECOND)
                .brake
                .mul(Fx::from_int(TICKS_PER_SECOND)),
        );
        assert!(
            spot < stop.add(Fx::ONE),
            "шагающая шла в развороте: путь {spot:?}"
        );
        assert_eq!(vel_spot, Fx::ZERO, "шагающая не встала в развороте");
        assert!(
            t_arc > 5 && t_spot > 5,
            "разворот занял {t_arc} и {t_spot} тиков"
        );
    }

    /// **Доворот ограничен.** За тик курс не меняется больше, чем
    /// позволено данными, каким бы ни был промах.
    #[test]
    fn the_turn_rate_is_a_limit() {
        let l = limits();
        for want in [1, 1000, angle::QUARTER, angle::HALF, angle::FULL - 1] {
            let mut d = Drive {
                head: 0,
                vel: Fx::ZERO,
                yaw_rate: 0,
            };
            for _ in 0..100 {
                let (n, _) = advance(d, want, Fx::from_int(6), &l, Fx::ONE, inv_tick());
                assert!(
                    angle::diff(n.head, d.head).abs() <= l.turn,
                    "курс уехал на {} при пределе {}",
                    angle::diff(n.head, d.head),
                    l.turn
                );
                d = n;
            }
        }
    }

    /// **Машина выходит на курс и остаётся на нём** — без перелёта
    /// и без дрожи: доворот гаснет, а не бьётся около цели.
    #[test]
    fn the_heading_settles_without_overshoot() {
        let l = limits();
        for want in [300, angle::QUARTER, angle::HALF, -angle::QUARTER] {
            let mut d = Drive {
                head: 0,
                vel: Fx::ZERO,
                yaw_rate: 0,
            };
            let sign = angle::diff(want, 0).signum();
            for tick in 0..400 {
                let (n, _) = advance(d, want, Fx::from_int(6), &l, Fx::ONE, inv_tick());
                let err = angle::diff(want, n.head);
                assert!(
                    err == 0 || err.signum() == sign,
                    "тик {tick}: курс перелетел цель {want}: промах {err}"
                );
                d = n;
            }
            assert!(
                angle::diff(want, d.head).abs() < angle::FULL / 720,
                "не вышла на курс {want}: {}",
                d.head
            );
            assert_eq!(d.yaw_rate, 0, "у машины на курсе доворот не погас");
        }
    }

    /// **Разворот кончается за время трапеции**, а не тянется хвостом
    /// экспоненты (анализ 240, B1): разгон доворота, ход на пределе,
    /// торможение — и курс взят. Граница — время профиля с запасом в три
    /// тика на дискретность; прежний пропорциональный доворот четверть
    /// оборота брал почти вдвое дольше.
    #[test]
    fn a_turn_takes_the_trapezoid_time_not_an_exponential_tail() {
        let l = limits();
        let alpha = l.turn_acc.to_int_trunc().max(1) as u64;
        for want in [2000, angle::QUARTER / 2, angle::QUARTER, angle::HALF - 1] {
            let e = want as u64;
            let ramp = (l.turn as u64).div_ceil(alpha);
            let bound = if e >= ramp * l.turn as u64 {
                2 * ramp + e.div_ceil(l.turn as u64)
            } else {
                2 * (e.div_ceil(alpha)).isqrt() + 2
            } + 3;
            let mut d = Drive {
                head: 0,
                vel: Fx::ZERO,
                yaw_rate: 0,
            };
            let mut ticks = 0u64;
            while d.head != want || d.yaw_rate != 0 {
                d = advance(d, want, Fx::from_int(6), &l, Fx::ONE, inv_tick()).0;
                ticks += 1;
                assert!(
                    ticks <= bound,
                    "разворот на {want} не кончился за {bound} тиков"
                );
            }
        }
    }

    /// **Ехать некуда — машина встаёт, а не встаёт как вкопанная.**
    /// Торможение тоже имеет предел, и курс при этом не сбивается.
    #[test]
    fn with_nowhere_to_go_the_machine_brakes_and_keeps_its_heading() {
        let l = limits();
        let head = 12345;
        let mut d = Drive {
            head,
            vel: Fx::from_int(30),
            yaw_rate: 0,
        };
        let (first, _) = advance(d, NO_WAY, Fx::from_int(30), &l, Fx::ONE, inv_tick());
        assert!(first.vel.raw() > 0, "машина встала за один тик");
        let mut ticks = 0;
        while d.vel.raw() > 0 && ticks < 1000 {
            let (n, _) = advance(d, NO_WAY, Fx::from_int(30), &l, Fx::ONE, inv_tick());
            d = n;
            ticks += 1;
        }
        assert_eq!(d.vel, Fx::ZERO);
        assert_eq!(d.head, head, "стоящая машина развернулась сама");
        let want = (Fx::from_int(30).raw() / l.brake.raw()) as i32;
        assert!(
            ticks == want || ticks == want + 1,
            "торможение заняло {ticks} тиков вместо {want}"
        );
    }

    /// **Шаг идёт по курсу.** Не по желаемому направлению: машина
    /// едет туда, куда смотрит.
    #[test]
    fn the_step_follows_the_heading_not_the_wish() {
        let l = limits();
        let vmax = Fx::from_int(6);
        // Курс на юг, цель на восток, но за один тик машина
        // доворачивает лишь малость — значит и едет почти на юг.
        let d = Drive {
            head: angle::QUARTER,
            vel: vmax,
            yaw_rate: 0,
        };
        let (n, m) = advance(d, 0, vmax, &l, Fx::ONE, inv_tick());
        assert!(m.dz.raw() > 0, "шаг не по курсу");
        assert!(
            m.dz.raw() > m.dx.abs().raw() * 4,
            "за тик машину развернуло почти целиком: dx={:?} dz={:?}",
            m.dx,
            m.dz
        );
        assert!(
            angle::diff(n.head, angle::QUARTER) < 0,
            "доворот пошёл не в ту сторону"
        );
    }

    /// A new speed target must be reached by finite braking, including zero.
    #[test]
    fn a_lower_speed_target_brakes_without_erasing_momentum() {
        let l = limits();
        for target in [Fx::from_int(3), Fx::ZERO] {
            let mut d = Drive {
                head: 0,
                vel: Fx::from_int(60),
                yaw_rate: 0,
            };
            for tick in 0..120 {
                let (n, m) = advance(d, 0, target, &l, Fx::ONE, inv_tick());
                assert!(d.vel.sub(n.vel) <= l.brake);
                assert!(n.vel >= target);
                assert_eq!(m.dx, n.vel.mul(inv_tick()));
                if tick == 0 {
                    assert!(n.vel > target);
                }
                d = n;
            }
            assert_eq!(d.vel, target);
        }
    }

    #[test]
    fn changing_gears_brakes_to_rest_before_accelerating_in_reverse() {
        let l = limits();
        for sign in [-1, 1] {
            let mut d = Drive {
                head: 0,
                vel: Fx::from_int(30 * sign),
                yaw_rate: 0,
            };
            let target = Fx::from_int(-12 * sign);
            let mut stopped = false;
            for _ in 0..200 {
                let (next, movement) = advance(d, 0, target, &l, Fx::ONE, inv_tick());
                assert!(next.vel.sub(d.vel).abs() <= l.brake.max(accel_per_tick(&tracked())));
                if next.vel == Fx::ZERO {
                    stopped = true;
                }
                if next.vel.mul(target) > Fx::ZERO {
                    assert!(stopped, "gear change reversed momentum without braking");
                }
                assert_eq!(movement.dx, next.vel.mul(inv_tick()));
                d = next;
            }
            assert_eq!(d.vel, target);
        }
    }

    /// **Виды ходовой отличаются не на словах.** Гусеничная тяжелее
    /// и сильнее, шагающая доворачивает медленнее и стоит в развороте.
    #[test]
    fn the_two_chassis_actually_differ() {
        let t = crate::test_tuning();
        let (a, b) = (
            Limits::of(&t.tracked, TICKS_PER_SECOND),
            Limits::of(&t.walking, TICKS_PER_SECOND),
        );
        assert_ne!(a, b, "пределы видов совпали");
        assert!(a.pivot_speed.raw() > 0 && b.pivot_speed == Fx::ZERO);
        assert!(
            a.turn_acc.raw() > 0 && b.turn_acc.raw() > 0,
            "без углового ускорения нет доворота"
        );
    }

    /// **Кубический корень честный.** Целочисленный, вниз, на всём
    /// диапазоне, который сюда приходит.
    #[test]
    fn the_integer_cube_root_is_exact() {
        for v in [
            0u128,
            1,
            7,
            8,
            9,
            26,
            27,
            1_000_000_000,
            999_999_999_999,
            1u128 << 90,
        ] {
            let r = icbrt(v);
            assert!(r * r * r <= v, "корень {r} из {v} велик");
            assert!((r + 1) * (r + 1) * (r + 1) > v, "корень {r} из {v} мал");
        }
    }
}
