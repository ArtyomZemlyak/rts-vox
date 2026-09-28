//! Походка шагохода и плавность хода на экране (ADR-234).
//!
//! Фаза шага хранится в симуляции и растёт от принятого пути корпуса и
//! дуги поворота опор. Здесь считаются перенос, углы звеньев и точки
//! постановки ступней. Они не входят в хеш и не заменяют авторитетную
//! подвеску: она пока использует четыре точки корпуса, без передачи
//! визуального отрыва ноги в опорные силы.
//!
//! **Звено ставится по цели ступни, а не по кривой углов.** Ступня
//! в опоре стоит на поверхности в той точке мира, где встала, и
//! не скользит: положение вдоль хода выводится из фазы, а фаза
//! в симуляции учитывает линейное движение и разворот. Колено
//! находится двухзвенной обратной кинематикой — окружность бедра
//! пересекается с окружностью голени, и из двух точек берётся та,
//! что в сторону сгиба, наружу от корпуса. Поворот звена берётся
//! **от рамки покоя**: кратчайший поворот, переводящий ось звена
//! из положения покоя в новое, домножается на поворот покоя.
//! Так в покое походка совпадает с тем, что нарисовано в теле,
//! и брусок не закручивается вокруг своей оси.
//!
//! **Между тиками.** Тик идёт тридцать раз в секунду, кадры — как
//! получится; корпус, переставляемый раз в тик, на ста шестидесяти
//! кадрах стоит пять кадров и прыгает на шестом. Поза кадра —
//! интерполяция между прошлым и нынешним тиком по доле недотиканного
//! времени; курс интерполируется по кратчайшей дуге, фаза шага —
//! по кольцу.

use bevy::prelude::{Quat, Vec3};
use voxel::unit::{Leg, UnitBody};

use crate::footing::{Contact, Sole};
use crate::model::{root_centre, SegFrame};

/// Числа походки в мировых единицах — из данных ходовой.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaitParams {
    /// Путь корпуса за полный цикл шага.
    pub stride: f32,
    /// Доля цикла, которую ступня стоит на земле.
    pub duty: f32,
    /// Высота переноса ступни.
    pub lift: f32,
    /// Сдвиг фазы по опорам, доли цикла.
    pub offset: [f32; 4],
}

impl GaitParams {
    /// Из данных: сантиметры в мировые единицы через ребро мирового
    /// воксела, проценты — в доли.
    pub fn of(g: &modding::tuning::Gait, per_cm: f32) -> GaitParams {
        GaitParams {
            stride: g.stride_cm as f32 * per_cm,
            duty: (g.duty_pct as f32 / 100.0).clamp(0.05, 1.0),
            lift: g.lift_cm as f32 * per_cm,
            offset: g.offset_pct.map(|p| p as f32 / 100.0),
        }
    }
}

/// Нога, посчитанная один раз при загрузке тела: шарниры покоя,
/// длины звеньев, направление сгиба и номера сегментов.
#[derive(Clone, Debug)]
pub struct LegRig {
    /// Шарниры покоя в системе корня, мировые единицы.
    pub hip: Vec3,
    pub knee: Vec3,
    pub ankle: Vec3,
    pub l1: f32,
    pub l2: f32,
    /// Куда сгибается колено: наружу от корпуса, в горизонтали.
    pub bend: Vec3,
    /// Рамки покоя звеньев — от них считается поворот.
    pub thigh_rest: SegFrame,
    pub shin_rest: SegFrame,
    pub thigh: usize,
    pub shin: usize,
    pub knee_ball: Option<usize>,
    pub ankle_ball: Option<usize>,
    pub foot: Option<usize>,
    /// Какой опоре подвески отвечает нога.
    pub support: usize,
    /// Собственный сдвиг фазы относительно опоры.
    pub phase_shift: f32,
    pub sole: Sole,
}

/// Поза одной ноги на кадр: рамки звеньев и того, что едет с ними.
#[derive(Clone, Copy, Debug)]
pub struct LegPose {
    pub thigh: SegFrame,
    pub shin: SegFrame,
    pub knee: SegFrame,
    pub ankle: SegFrame,
    pub foot: SegFrame,
}

impl LegPose {
    /// Рамка по роли звена.
    pub fn part(&self, role: LegRole) -> &SegFrame {
        match role {
            LegRole::Thigh => &self.thigh,
            LegRole::Shin => &self.shin,
            LegRole::Knee => &self.knee,
            LegRole::Ankle => &self.ankle,
            LegRole::Foot => &self.foot,
        }
    }
}

/// Роль звена в ноге — что у сущности сегмента подменяется позой.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegRole {
    Thigh,
    Shin,
    Knee,
    Ankle,
    Foot,
}

/// Собрать оснастку ног тела по его рамкам сегментов.
///
/// `per_mm` — мировых единиц в миллиметре; `frames` — рамки покоя
/// всех сегментов (`model::segment_frames`).
pub fn rigs_of(b: &UnitBody, per_mm: f32, frames: &[SegFrame]) -> Vec<LegRig> {
    let c = root_centre(b);
    let k = b.mm as f32 * per_mm;
    let at = |p: (i16, i16, i16)| {
        Vec3::new(
            (p.0 as f32 - c[0]) * k,
            (p.1 as f32 - c[1]) * k,
            (p.2 as f32 - c[2]) * k,
        )
    };
    b.legs
        .iter()
        .map(|l: &Leg| {
            let (hip, knee, ankle) = (at(l.hip), at(l.knee), at(l.ankle));
            // Сгиб — куда колено вынесено от прямой бедро–ступня,
            // в горизонтали. У шагохода это наружу по диагонали.
            let along = (ankle - hip).normalize_or_zero();
            let off = knee - hip;
            let side = off - along * off.dot(along);
            let flat = Vec3::new(side.x, 0.0, side.z);
            let bend = flat.try_normalize().unwrap_or_else(|| {
                let out = Vec3::new(hip.x, 0.0, hip.z);
                out.try_normalize().unwrap_or(Vec3::X)
            });
            let seg = |i: u8| {
                frames.get(i as usize).copied().unwrap_or(SegFrame {
                    pos: Vec3::ZERO,
                    rot: Quat::IDENTITY,
                    turret: false,
                })
            };
            LegRig {
                hip,
                knee,
                ankle,
                l1: (knee - hip).length().max(1e-3),
                l2: (ankle - knee).length().max(1e-3),
                bend,
                thigh_rest: seg(l.thigh),
                shin_rest: seg(l.shin),
                thigh: l.thigh as usize,
                shin: l.shin as usize,
                knee_ball: l.knee_ball.map(|v| v as usize),
                ankle_ball: l.ankle_ball.map(|v| v as usize),
                foot: l.foot.map(|v| v as usize),
                support: l.support as usize,
                phase_shift: l.phase_shift_pct as f32 / 100.0,
                sole: l
                    .foot
                    .and_then(|i| b.segments.get(i as usize))
                    .map_or_else(|| Sole::point(ankle.y), |s| Sole::of(s, per_mm)),
            }
        })
        .collect()
}

/// Смещение ступни вдоль хода и её подъём по местной фазе `p` ∈ [0, 1).
///
/// В опоре ступня едет назад относительно корпуса ровно с той
/// скоростью, с какой корпус едет вперёд: за опору она проходит
/// `duty · stride`, а фаза за это время — `duty`. В переносе ступня
/// возвращается вперёд по синусной дуге.
pub fn foot_curve(p: f32, g: &GaitParams) -> (f32, f32) {
    let p = p.rem_euclid(1.0);
    let reach = g.duty * g.stride;
    if p < g.duty {
        (reach * 0.5 - p * g.stride, 0.0)
    } else {
        let q = (p - g.duty) / (1.0 - g.duty).max(1e-3);
        (reach * (q - 0.5), g.lift * (std::f32::consts::PI * q).sin())
    }
}

/// Корпус на кадре: всё, что нужно ноге, чтобы встать.
pub struct Stance<'a> {
    /// Authoritative support state. Flight releases world-space foot anchors.
    pub grounded: bool,
    /// Elapsed presentation time; zero while the game is paused.
    pub dt: f32,
    /// Середина основания корпуса в мире.
    pub base: Vec3,
    /// Поворот корпуса: курс и наклон.
    pub rot: Quat,
    /// Фаза шага, доли цикла.
    pub phase: f32,
    /// Доля предельного хода: у стоящей машины перенос без подъёма.
    pub lift_scale: f32,
    pub motion: StepMotion,
    pub params: &'a GaitParams,
    /// Высота текущей поверхности под точкой мира `(x, z)`.
    pub floor: &'a dyn Fn(f32, f32) -> f32,
}

/// Accepted world movement and yaw per second, plus the authoritative cadence.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepMotion {
    pub linear: Vec3,
    pub angular: f32,
    pub cycles: f32,
}
impl StepMotion {
    pub fn between(previous: &Pose, current: &Pose, dt: f32) -> Self {
        if !previous.alive || !current.alive || dt <= 0.0 {
            return Self::default();
        }
        let d = current.pos - previous.pos;
        Self {
            linear: Vec3::new(d.x, 0.0, d.z) / dt,
            angular: angle_diff(current.yaw, previous.yaw) / dt,
            cycles: (current.phase - previous.phase).rem_euclid(1.0) / dt,
        }
    }
    fn lead_time(self, duty: f32) -> f32 {
        if self.cycles > 1e-5 {
            duty * 0.5 / self.cycles
        } else {
            0.0
        }
    }
}

/// Unsynced contact history. Stance keeps a world-space point and orientation;
/// swing transfers it to a predicted touchdown, using the authoritative phase.
#[derive(Clone, Debug, Default)]
pub struct FootPlant {
    anchor: Option<Contact>,
    from: Contact,
    goal: Contact,
    base: Vec3,
    swing: bool,
    clearance: f32,
    progress: f32,
    cadence: f32,
    local: Option<Contact>,
}

impl FootPlant {
    pub fn pose(&mut self, rig: &LegRig, s: &Stance<'_>) -> LegPose {
        if !s.grounded {
            // Keep the last joint pose relative to the falling hull instead
            // of reaching for a piece of ground that no longer supports it.
            let local = *self.local.get_or_insert(Contact {
                pos: rig.ankle,
                rot: Quat::IDENTITY,
            });
            self.anchor = None;
            self.swing = false;
            self.base = s.base;
            let mut pose = rig.solve(local.pos, s.rot);
            pose.foot.rot = local.rot;
            return pose;
        }
        let g = s.params;
        let p = (s.phase - g.offset[rig.support.min(3)] - rig.phase_shift).rem_euclid(1.0);
        let swing = p >= g.duty;
        let yaw = s.rot.to_euler(bevy::math::EulerRot::YXZ).0;
        if self.anchor.is_none()
            || self.base.distance_squared(s.base) > (rig.l1 + rig.l2).powi(2) * 4.0
        {
            let rest = rig.sole.contact(s.base + s.rot * rig.ankle, yaw, s.floor);
            self.anchor = Some(rest);
            self.from = rest;
            self.goal = rest;
            self.swing = false;
        }
        if swing && (!self.swing || (self.cadence == 0.0 && s.motion.cycles > 1e-5)) {
            self.from = self.anchor.unwrap();
            self.progress = 0.0;
            self.cadence = s.motion.cycles;
            if s.motion.cycles > 1e-5 {
                // Predict touchdown plus half of the next stance. Freeze
                // this contact for the swing, so terrain/velocity changes
                // cannot abruptly move its landing point during transfer.
                let lead = s.motion.lead_time(g.duty) + (1.0 - p) / s.motion.cycles;
                let turn = s.motion.angular * lead;
                self.goal = rig.sole.contact(
                    s.base
                        + s.motion.linear * lead
                        + Quat::from_rotation_y(turn) * s.rot * rig.ankle,
                    yaw + turn,
                    s.floor,
                );
            }
            self.clearance = rig.sole.clearance(
                self.from,
                self.goal,
                g.lift * s.lift_scale.clamp(0.0, 1.0),
                s.floor,
            );
        }
        let target = if swing {
            let span = (1.0 - g.duty).max(1e-3);
            let q = if s.motion.cycles > 1e-5 {
                self.progress.max(((p - g.duty) / span).clamp(0.0, 1.0))
            } else {
                // Stopping the hull must finish an already lifted foot,
                // not freeze it in the air or drop it in a single frame.
                (self.progress + s.dt.max(0.0) * self.cadence / span).min(1.0)
            };
            self.progress = q;
            let eased = q * q * (3.0 - 2.0 * q);
            let mut v = self.from.lerp(self.goal, eased);
            v.pos.y += (q * std::f32::consts::PI).sin() * self.clearance;
            v
        } else {
            if self.swing {
                self.anchor = Some(self.goal);
            }
            self.anchor.unwrap()
        };
        self.swing = swing;
        self.base = s.base;
        let mut pose = rig.solve(s.rot.inverse() * (target.pos - s.base), s.rot);
        pose.foot.rot = s.rot.inverse() * target.rot;
        self.local = Some(Contact {
            pos: pose.ankle.pos,
            rot: pose.foot.rot,
        });
        pose
    }
}

impl LegRig {
    pub fn role(&self, segment: usize) -> Option<LegRole> {
        if self.thigh == segment {
            Some(LegRole::Thigh)
        } else if self.shin == segment {
            Some(LegRole::Shin)
        } else if self.knee_ball == Some(segment) {
            Some(LegRole::Knee)
        } else if self.ankle_ball == Some(segment) {
            Some(LegRole::Ankle)
        } else if self.foot == Some(segment) {
            Some(LegRole::Foot)
        } else {
            None
        }
    }
    /// Поза ноги на кадре.
    pub fn pose(&self, s: &Stance<'_>) -> LegPose {
        let g = s.params;
        let (off, lift) = foot_curve(
            s.phase - g.offset[self.support.min(3)] - self.phase_shift,
            g,
        );
        let lift = lift * s.lift_scale.clamp(0.0, 1.0);
        // Цель ступни: покой плюс ход вдоль носа корпуса, в мире.
        let want = self.ankle + Vec3::X * off;
        let w = s.base + s.rot * want;
        let floor = (s.floor)(w.x, w.z);
        // Пол под ступнёй, а не под корпусом: на склоне ступни стоят
        // на разной высоте, и ровно это отличает шаг от скольжения.
        let target_w = Vec3::new(w.x, floor + self.ankle.y + lift, w.z);
        let target = s.rot.inverse() * (target_w - s.base);
        self.solve(target, s.rot)
    }

    /// Двухзвенная обратная кинематика в системе корня: колено там,
    /// где окружность бедра встречает окружность голени, со стороны
    /// сгиба.
    pub fn solve(&self, target: Vec3, hull: Quat) -> LegPose {
        let d = target - self.hip;
        let dist = d.length().clamp(
            (self.l1 - self.l2).abs().max(1e-3),
            (self.l1 + self.l2) * 0.999,
        );
        let u = d.try_normalize().unwrap_or(Vec3::NEG_Y);
        let a = (self.l1 * self.l1 - self.l2 * self.l2 + dist * dist) / (2.0 * dist);
        let h = (self.l1 * self.l1 - a * a).max(0.0).sqrt();
        let n = (self.bend - u * self.bend.dot(u))
            .try_normalize()
            .unwrap_or(Vec3::Y);
        let knee = self.hip + u * a + n * h;
        let ankle = self.hip + u * dist;
        let turn = |rest: &SegFrame, from: Vec3, to: Vec3, at: Vec3| -> SegFrame {
            let (from, to) = (from.normalize_or_zero(), to.normalize_or_zero());
            let arc = if from.length() > 0.5 && to.length() > 0.5 {
                Quat::from_rotation_arc(from, to)
            } else {
                Quat::IDENTITY
            };
            SegFrame {
                pos: at,
                rot: arc * rest.rot,
                turret: rest.turret,
            }
        };
        let thigh = turn(
            &self.thigh_rest,
            self.knee - self.hip,
            knee - self.hip,
            self.hip,
        );
        let shin = turn(&self.shin_rest, self.ankle - self.knee, ankle - knee, knee);
        // Шары суставов — куда пришли суставы; ступня — плоско в мире,
        // с одним лишь курсом корпуса.
        let ball = |at: Vec3| SegFrame {
            pos: at,
            rot: Quat::IDENTITY,
            turret: false,
        };
        let (fwd, _, _) = hull.to_euler(bevy::math::EulerRot::YXZ);
        let foot = SegFrame {
            pos: ankle,
            rot: hull.inverse() * Quat::from_rotation_y(fwd),
            turret: false,
        };
        LegPose {
            thigh,
            shin,
            knee: ball(knee),
            ankle: ball(ankle),
            foot,
        }
    }
}

// ---------------------------------------------------------------------
// Между тиками.
// ---------------------------------------------------------------------

/// Поза юнита на тике — то, что рендер интерполирует между тиками.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: Vec3,
    /// Курс, радианы (`render::yaw_of`).
    pub yaw: f32,
    /// Нормаль корпуса.
    pub up: Vec3,
    /// Фаза шага, доли цикла.
    pub phase: f32,
    pub alive: bool,
}

impl Default for Pose {
    fn default() -> Pose {
        Pose {
            pos: Vec3::ZERO,
            yaw: 0.0,
            up: Vec3::Y,
            phase: 0.0,
            alive: false,
        }
    }
}

/// Кратчайшая разность углов, радианы, `(-π, π]`.
pub fn angle_diff(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a - b + PI).rem_euclid(TAU) - PI
}

/// Поза между прошлым и нынешним тиком, `t` ∈ [0, 1].
///
/// Юнит, которого не было на прошлом тике, не интерполируется:
/// подкрепление, вставшее в слот убитого, иначе ехало бы через
/// полкарты из его могилы.
pub fn lerp(prev: &Pose, cur: &Pose, t: f32) -> Pose {
    if !prev.alive || !cur.alive {
        return *cur;
    }
    let t = t.clamp(0.0, 1.0);
    // Разность фаз по кольцу, `(-0.5, 0.5]`.
    let dphase = (cur.phase - prev.phase + 0.5).rem_euclid(1.0) - 0.5;
    Pose {
        pos: prev.pos.lerp(cur.pos, t),
        yaw: prev.yaw + angle_diff(cur.yaw, prev.yaw) * t,
        up: prev.up.lerp(cur.up, t).try_normalize().unwrap_or(cur.up),
        phase: (prev.phase + dphase * t).rem_euclid(1.0),
        alive: true,
    }
}

/// Фаза шага из двоичной фазы симуляции — доли цикла.
pub fn phase_of(gait: i32) -> f32 {
    (gait as u32 & (fx::angle::FULL as u32 - 1)) as f32 / fx::angle::FULL as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The range's feet (`sim_core::gait`, ADR-279) and this module agree:
    /// the same rigs, the same two-link solve, the same sole contact on a
    /// slope. What the range hits is what the picture draws.
    #[test]
    fn the_simulation_poses_a_leg_where_this_module_draws_it() {
        use sim_core::damage::V3;
        let b = voxel::unit::strider();
        let ours = rigs_of(&b, 0.01, &crate::model::segment_frames(&b, 0.01));
        let theirs = sim_core::gait::rigs_of(&b);
        assert_eq!(ours.len(), theirs.len());
        let f = |v: V3| {
            Vec3::new(
                v.x.to_f64_lossy() as f32,
                v.y.to_f64_lossy() as f32,
                v.z.to_f64_lossy() as f32,
            )
        };
        let q = |v: f32| fx::Fx::from_ratio((v * 1000.0).round() as i32, 1000);
        let v3 = |v: Vec3| V3::new(q(v.x), q(v.y), q(v.z));
        for (a, s) in ours.iter().zip(&theirs) {
            assert!((a.hip - f(s.hip)).length() < 1e-3);
            assert!((a.knee - f(s.knee)).length() < 1e-3);
            assert!((a.ankle - f(s.ankle)).length() < 1e-3);
            for target in [
                a.ankle,
                a.ankle + Vec3::new(6.0, 3.0, 0.0),
                a.ankle + Vec3::new(-5.0, 1.0, 4.0),
                a.ankle + Vec3::new(0.0, 8.0, -2.0),
            ] {
                let mine = a.solve(target, Quat::IDENTITY);
                let sim = s.solve(v3(target));
                assert!(
                    (mine.knee.pos - f(sim.knee)).length() < 0.05,
                    "knee {:?} vs {:?} for {target:?}",
                    mine.knee.pos,
                    f(sim.knee)
                );
                assert!((mine.ankle.pos - f(sim.ankle)).length() < 0.05);
            }
            let floor = |x: f32, z: f32| 0.3 * x - 0.2 * z;
            let floor_fx = |x: fx::Fx, z: fx::Fx| q(0.3).mul(x).sub(q(0.2).mul(z));
            let head = 7000;
            let mine = a
                .sole
                .contact(Vec3::new(2.0, 0.0, 4.0), crate::yaw_of(head), &floor);
            let sim = s
                .sole
                .contact(v3(Vec3::new(2.0, 0.0, 4.0)), head, &floor_fx);
            assert!(
                (mine.pos - f(sim.pos)).length() < 0.05,
                "{:?} vs {:?}",
                mine.pos,
                f(sim.pos)
            );
            let [_, up, _] = sim.axes();
            assert!((mine.rot * Vec3::Y - f(up)).length() < 0.02);
        }
    }

    fn params() -> GaitParams {
        GaitParams {
            stride: 1.6,
            duty: 0.75,
            lift: 0.4,
            offset: [0.25, 0.75, 0.0, 0.5],
        }
    }

    /// Нога как у шагохода: бедро высоко, колено наружу, ступня под
    /// корпусом.
    fn rig() -> LegRig {
        let hip = Vec3::new(-0.9, 2.7, -0.9);
        let knee = Vec3::new(-1.3, 1.5, -1.3);
        let ankle = Vec3::new(-1.1, 0.4, -1.1);
        let rest = |from: Vec3| SegFrame {
            pos: Vec3::ZERO,
            rot: Quat::from_rotation_arc(Vec3::X, from.normalize()),
            turret: false,
        };
        LegRig {
            hip,
            knee,
            ankle,
            l1: (knee - hip).length(),
            l2: (ankle - knee).length(),
            bend: Vec3::new(-1.0, 0.0, -1.0).normalize(),
            thigh_rest: rest(knee - hip),
            shin_rest: rest(ankle - knee),
            thigh: 0,
            shin: 1,
            knee_ball: None,
            ankle_ball: None,
            foot: None,
            support: 3,
            phase_shift: 0.0,
            sole: Sole::point(ankle.y),
        }
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn planted_foot_keeps_its_world_contact_during_turns() {
        let r = rig();
        let g = GaitParams {
            stride: 0.4,
            duty: 0.7,
            lift: 0.15,
            offset: [0.0; 4],
        };
        let mut plant = FootPlant::default();
        let floor = |_: f32, _: f32| 0.0;
        let mut initial = None;
        for k in 0..24 {
            let base = Vec3::new(k as f32 * 0.004, 0.0, 0.0);
            let rot = Quat::from_rotation_y(k as f32 * 0.008);
            let s = Stance {
                grounded: true,
                dt: 0.005,
                base,
                rot,
                phase: k as f32 * 0.02,
                lift_scale: 1.0,
                motion: StepMotion::default(),
                params: &g,
                floor: &floor,
            };
            let pose = plant.pose(&r, &s);
            let contact = base + rot * pose.ankle.pos;
            let first = *initial.get_or_insert(contact);
            assert!(
                near(first, contact),
                "planted foot slid: {first:?} -> {contact:?}"
            );
            assert!((rot * pose.foot.rot).angle_between(Quat::IDENTITY) < 1e-3);
        }
    }

    #[test]
    fn swinging_foot_clears_ground_and_replants_without_a_jump() {
        let r = rig();
        let g = GaitParams {
            stride: 0.4,
            duty: 0.7,
            lift: 0.15,
            offset: [0.0; 4],
        };
        let floor = |_: f32, _: f32| 0.0;
        let mut plant = FootPlant::default();
        let mut last: Option<Vec3> = None;
        let mut highest = 0.0f32;
        for k in 0..110 {
            let s = Stance {
                grounded: true,
                dt: 0.005,
                base: Vec3::new(k as f32 * 0.002, 0.0, 0.0),
                rot: Quat::IDENTITY,
                phase: (k as f32 * 0.01) % 1.0,
                lift_scale: 1.0,
                motion: StepMotion {
                    linear: Vec3::X * 0.2,
                    cycles: 1.0,
                    ..Default::default()
                },
                params: &g,
                floor: &floor,
            };
            let pose = plant.pose(&r, &s);
            let foot = s.base + pose.ankle.pos;
            highest = highest.max(foot.y - r.ankle.y);
            if let Some(old) = last {
                assert!(old.distance(foot) < 0.035, "foot teleported");
            }
            last = Some(foot);
        }
        assert!(highest > g.lift * 0.95);
        assert!((last.unwrap().y - r.ankle.y).abs() < 1e-3);
    }

    #[test]
    fn pivoting_feet_transfer_in_opposite_arcs_and_keep_planted_contacts() {
        let g = GaitParams {
            stride: 0.5,
            duty: 0.75,
            lift: 0.16,
            offset: [0.0; 4],
        };
        let floor = |_: f32, _: f32| 0.0;
        for direction in [-1.0, 1.0] {
            let mut rigs = [rig(), rig()];
            // Mirror the second leg across the centre; both must follow
            // their own rotation arc rather than the same forward stroke.
            let second = &mut rigs[1];
            for v in [
                &mut second.hip,
                &mut second.knee,
                &mut second.ankle,
                &mut second.bend,
            ] {
                v.x = -v.x;
                v.z = -v.z;
            }
            let mut plants = [FootPlant::default(), FootPlant::default()];
            let mut anchors = [Vec3::ZERO; 2];
            let mut previous = [Vec3::ZERO; 2];
            let mut replants = [0; 2];
            for k in 0..601 {
                let time = k as f32 / 200.0;
                let phase = time.rem_euclid(1.0);
                let s = Stance {
                    grounded: true,
                    dt: 0.005,
                    base: Vec3::ZERO,
                    rot: Quat::from_rotation_y(time * direction * 0.23),
                    phase,
                    lift_scale: 1.0,
                    motion: StepMotion {
                        angular: direction * 0.23,
                        cycles: 1.0,
                        ..Default::default()
                    },
                    params: &g,
                    floor: &floor,
                };
                for i in 0..2 {
                    let old_swing = plants[i].swing;
                    let pose = plants[i].pose(&rigs[i], &s);
                    let contact = s.rot * pose.ankle.pos;
                    if k == 0 || (old_swing && !plants[i].swing) {
                        anchors[i] = contact;
                        if k != 0 {
                            replants[i] += 1;
                        }
                    }
                    if !plants[i].swing {
                        assert!(near(contact, anchors[i]), "stance slid while pivoting");
                    }
                    if k > 0 {
                        assert!(contact.distance(previous[i]) < 0.03, "pivot foot jumped");
                    }
                    previous[i] = contact;
                }
            }
            assert_eq!(replants, [3, 3]);
            assert!(
                previous[0].distance(rigs[0].ankle) > 0.5,
                "foot did not circle the body"
            );
            assert!(
                (previous[0] + previous[1] - Vec3::Y * (rigs[0].ankle.y * 2.0)).length() < 1e-3
            );
        }
    }

    #[test]
    fn cadence_and_turn_velocity_cross_the_angle_seam_without_a_spike() {
        let a = Pose {
            alive: true,
            yaw: std::f32::consts::PI - 0.01,
            phase: 0.99,
            ..Default::default()
        };
        let b = Pose {
            alive: true,
            yaw: -std::f32::consts::PI + 0.01,
            phase: 0.01,
            ..Default::default()
        };
        let motion = StepMotion::between(&a, &b, 1.0 / 30.0);
        assert!((motion.angular - 0.6).abs() < 1e-4);
        assert!((motion.cycles - 0.6).abs() < 1e-4);
        assert_eq!(StepMotion::between(&a, &a, 1.0 / 30.0).cycles, 0.0);
        assert_eq!(
            StepMotion::between(&Pose::default(), &b, 1.0 / 30.0).angular,
            0.0
        );
    }

    #[test]
    fn stopping_completes_the_lifted_foot_and_pause_freezes_it() {
        let r = rig();
        let g = GaitParams {
            stride: 0.4,
            duty: 0.7,
            lift: 0.15,
            offset: [0.0; 4],
        };
        let floor = |_: f32, _: f32| 0.0;
        let mut plant = FootPlant::default();
        let mut s = Stance {
            grounded: true,
            dt: 0.005,
            base: Vec3::ZERO,
            rot: Quat::IDENTITY,
            phase: 0.6,
            lift_scale: 1.0,
            motion: StepMotion {
                cycles: 1.0,
                ..Default::default()
            },
            params: &g,
            floor: &floor,
        };
        plant.pose(&r, &s);
        s.phase = 0.85;
        let raised = plant.pose(&r, &s).foot.pos;
        assert!(raised.y > r.ankle.y + 0.14);
        s.motion = StepMotion::default();
        s.dt = 0.0;
        for _ in 0..20 {
            assert!(near(plant.pose(&r, &s).foot.pos, raised));
        }
        s.dt = 0.005;
        let mut last = raised;
        for _ in 0..100 {
            let foot = plant.pose(&r, &s).foot.pos;
            assert!(foot.distance(last) < 0.01, "stopping dropped the foot");
            last = foot;
        }
        assert!(
            (last.y - r.ankle.y).abs() < 1e-5,
            "stopped with a foot in the air"
        );
        // Resuming inside the same authoritative swing must not rewind it.
        s.motion.cycles = 1.0;
        s.phase = 0.86;
        assert!(near(plant.pose(&r, &s).foot.pos, last));
    }

    #[test]
    fn a_paused_initial_swing_is_planned_when_motion_starts() {
        let r = rig();
        let g = GaitParams {
            stride: 0.4,
            duty: 0.7,
            lift: 0.15,
            offset: [0.0; 4],
        };
        let floor = |_: f32, _: f32| 0.0;
        let mut plant = FootPlant::default();
        let mut s = Stance {
            grounded: true,
            dt: 0.0,
            base: Vec3::ZERO,
            rot: Quat::IDENTITY,
            phase: 0.7,
            lift_scale: 0.0,
            motion: StepMotion::default(),
            params: &g,
            floor: &floor,
        };
        for _ in 0..20 {
            plant.pose(&r, &s);
        }
        s.dt = 0.005;
        s.lift_scale = 1.0;
        s.motion.cycles = 1.0;
        s.motion.linear = Vec3::X * 0.2;
        plant.pose(&r, &s);
        s.phase = 0.85;
        let raised = plant.pose(&r, &s).foot.pos;
        assert!(raised.y > r.ankle.y + 0.14);
        assert!(raised.x > r.ankle.x, "first step stayed at the idle anchor");
    }

    #[test]
    fn losing_support_releases_the_old_world_anchor_and_lands_on_the_new_floor() {
        let r = rig();
        let g = GaitParams {
            stride: 0.4,
            duty: 0.7,
            lift: 0.15,
            offset: [0.0; 4],
        };
        let high = |_: f32, _: f32| 0.0;
        let low = |_: f32, _: f32| -3.0;
        let mut s = Stance {
            grounded: true,
            dt: 0.005,
            base: Vec3::ZERO,
            rot: Quat::IDENTITY,
            phase: 0.2,
            lift_scale: 0.0,
            motion: StepMotion::default(),
            params: &g,
            floor: &high,
        };
        let mut plant = FootPlant::default();
        let supported = plant.pose(&r, &s);
        s.grounded = false;
        s.floor = &low;
        for k in 0..100 {
            s.base.y = -(k as f32) * 0.03;
            let pose = plant.pose(&r, &s);
            assert!(
                near(pose.ankle.pos, supported.ankle.pos),
                "foot stayed anchored above the falling hull"
            );
            assert!(
                near(pose.knee.pos, supported.knee.pos),
                "flight changed link lengths or knee pose"
            );
        }
        s.base.y = -3.0;
        s.grounded = true;
        let landed = plant.pose(&r, &s);
        assert!(
            near(landed.ankle.pos, r.ankle),
            "landing reused the vanished upper floor"
        );
    }

    /// **Звенья не растягиваются.** Обратная кинематика двигает
    /// колено, а длины бедра и голени — свойство тела.
    #[test]
    fn the_links_keep_their_lengths() {
        let r = rig();
        for k in 0..40 {
            let t = r.ankle + Vec3::new(0.05 * k as f32 - 1.0, 0.02 * k as f32, 0.0);
            let p = r.solve(t, Quat::IDENTITY);
            let knee = p.shin.pos;
            let ankle = p.thigh.pos + p.thigh.rot * Vec3::X * r.l1;
            assert!(
                near(ankle, knee),
                "бедро не доходит до колена: {ankle:?} против {knee:?}"
            );
            let l1 = (knee - p.thigh.pos).length();
            assert!((l1 - r.l1).abs() < 1e-3, "бедро стало {l1} вместо {}", r.l1);
            let tip = knee + p.shin.rot * Vec3::X * r.l2;
            assert!((tip - knee).length() - r.l2 < 1e-3);
        }
    }

    #[test]
    fn unreachable_targets_do_not_stretch_or_collapse_the_links() {
        let mut r = rig();
        r.l1 = 2.0;
        r.l2 = 1.0;
        for target in [r.hip, r.hip + Vec3::X * 0.01, r.hip + Vec3::X * 30.0] {
            let p = r.solve(target, Quat::IDENTITY);
            assert!((p.knee.pos.distance(r.hip) - r.l1).abs() < 1e-4);
            assert!((p.ankle.pos.distance(p.knee.pos) - r.l2).abs() < 1e-4);
            assert!(p.thigh.rot.is_finite() && p.shin.rot.is_finite());
        }
    }

    /// **В покое поза совпадает с рамками тела.** Цель — ступня покоя,
    /// значит колено обязано встать туда, где нарисовано, а повороты
    /// звеньев — остаться поворотами покоя.
    #[test]
    fn the_rest_target_reproduces_the_rest_frames() {
        let r = rig();
        let p = r.solve(r.ankle, Quat::IDENTITY);
        assert!(
            near(p.shin.pos, r.knee),
            "колено ушло из покоя: {:?}",
            p.shin.pos
        );
        assert!(near(p.thigh.rot * Vec3::X, r.thigh_rest.rot * Vec3::X));
        assert!(near(p.shin.rot * Vec3::X, r.shin_rest.rot * Vec3::X));
    }

    /// **Колено гнётся наружу, а не внутрь.** Из двух пересечений
    /// окружностей берётся то, что в сторону сгиба.
    #[test]
    fn the_knee_bends_outwards() {
        let r = rig();
        let p = r.solve(r.ankle + Vec3::Y * 0.3, Quat::IDENTITY);
        let knee = p.shin.pos;
        let along = (r.ankle - r.hip).normalize();
        let off = knee - r.hip;
        let side = off - along * off.dot(along);
        assert!(side.dot(r.bend) > 0.0, "колено ушло внутрь: {knee:?}");
    }

    /// **В опоре ступня не скользит.** Корпус прошёл долю шага —
    /// ступня относительно корпуса отъехала назад ровно на столько же.
    #[test]
    fn a_planted_foot_stays_where_it_is_in_the_world() {
        let g = params();
        // Опора: фаза от 0 до duty. Два момента в опоре.
        let (a, _) = foot_curve(0.10, &g);
        let (b, _) = foot_curve(0.40, &g);
        let body_moved = (0.40 - 0.10) * g.stride;
        assert!(
            ((a - b) - body_moved).abs() < 1e-5,
            "ступня проскользнула: {a} → {b}"
        );
    }

    /// **Перенос поднимает ступню и возвращает её вперёд.**
    #[test]
    fn the_swing_lifts_the_foot_and_brings_it_forward() {
        let g = params();
        let (start, l0) = foot_curve(g.duty, &g);
        let (mid, l1) = foot_curve(g.duty + (1.0 - g.duty) * 0.5, &g);
        let (end, _) = foot_curve(0.999, &g);
        assert_eq!(l0, 0.0);
        assert!(
            (l1 - g.lift).abs() < 1e-3,
            "на середине переноса подъём {l1}"
        );
        assert!(
            end > mid && mid > start,
            "ступня не идёт вперёд: {start} {mid} {end}"
        );
        let (back, _) = foot_curve(0.0, &g);
        assert!(
            (back - end).abs() < 0.02,
            "цикл не замкнулся: {end} против {back}"
        );
    }

    /// **Ступня стоит на полу под собой, а не под корпусом.** На склоне
    /// пол под ступнёй ниже — и цель ступни ниже ровно на столько.
    #[test]
    fn the_foot_target_follows_the_floor_under_it() {
        let r = rig();
        let g = params();
        let run = |drop: f32| -> Vec3 {
            let floor = move |_x: f32, _z: f32| -drop;
            let s = Stance {
                grounded: true,
                dt: 0.005,
                base: Vec3::ZERO,
                rot: Quat::IDENTITY,
                phase: 0.6,
                lift_scale: 1.0,
                motion: StepMotion {
                    linear: Vec3::X * 0.2,
                    cycles: 1.0,
                    ..Default::default()
                },
                params: &g,
                floor: &floor,
            };
            r.pose(&s).ankle.pos
        };
        let flat = run(0.0);
        // The original 0.3 drop exceeded the rig's reach. Test a reachable floor.
        let low = run(0.1);
        assert!(
            (flat.y - low.y - 0.1).abs() < 1e-3,
            "ступня не опустилась к полу: {flat:?} {low:?}"
        );
    }

    /// **Курс интерполируется по кратчайшей дуге**, а не через ноль.
    #[test]
    fn yaw_is_interpolated_the_short_way_round() {
        use std::f32::consts::PI;
        let a = Pose {
            yaw: PI - 0.1,
            alive: true,
            ..Pose::default()
        };
        let b = Pose {
            yaw: -PI + 0.1,
            alive: true,
            ..Pose::default()
        };
        let m = lerp(&a, &b, 0.5);
        assert!(
            angle_diff(m.yaw, PI).abs() < 1e-4,
            "середина не у π: {}",
            m.yaw
        );
        // Фаза шага — по кольцу.
        let a = Pose {
            phase: 0.9,
            alive: true,
            ..Pose::default()
        };
        let b = Pose {
            phase: 0.1,
            alive: true,
            ..Pose::default()
        };
        let m = lerp(&a, &b, 0.5);
        assert!(
            m.phase < 0.01 || m.phase > 0.99,
            "фаза пошла через полцикла: {}",
            m.phase
        );
        // Мёртвый на прошлом тике — без интерполяции.
        let dead = Pose {
            alive: false,
            ..Pose::default()
        };
        let cur = Pose {
            pos: Vec3::new(5.0, 0.0, 5.0),
            alive: true,
            ..Pose::default()
        };
        assert_eq!(lerp(&dead, &cur, 0.5), cur);
    }
}
