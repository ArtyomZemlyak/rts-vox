//! Рендер-слой: камера и отсечение.
//!
//! **Тесты до рендера** (`CLAUDE.md`): подсистема не считается готовой,
//! пока не проходит headless из терминала. Окно headless не проверить,
//! а камеру и отсечение — можно, и вся логика здесь именно в них.
//! Поэтому крейт начинается с арифметики, а Bevy появится вместе
//! с окном (`docs/02-architecture.md`).
//!
//! **Рендер читает состояние и никогда в него не пишет.** Здесь этого
//! нельзя даже случайно: крейт не знает ни про `sim-core`, ни про
//! `voxel`, и получает на вход числа — размеры сетки чанков и камеру.
//!
//! Плавающая точка тут разрешена: расхождение рендера безопасно
//! (`docs/03-determinism.md`), в хеш состояния он не входит.

#![forbid(unsafe_code)]

pub mod dust;
pub mod edge;
pub mod geom;
pub mod look;
pub mod occlusion;
pub mod model;
pub mod stream;
pub mod tex;

/// Поворот вокруг вертикали, при котором нос тела — его **+X** — смотрит
/// в направление `(dx, dz)` мировых осей.
///
/// Знак здесь не косметика. Поворот вокруг `+Y` правый, поэтому `+X`
/// уходит к **−Z**: `(1,0,0)` переходит в `(cos θ, 0, −sin θ)`. Значит
/// `θ = atan2(−dz, dx)`, а не `atan2(dz, dx)`. Перепутанный знак
/// разворачивает строй зеркально относительно оси Z — и не падает,
/// не ругается и почти не заметен, пока юниты не начнут целиться.
pub fn yaw_to(dx: f32, dz: f32) -> f32 {
    (-dz).atan2(dx)
}

/// Поворот по **курсу из симуляции** (`units.head`, двоичный угол
/// `fx::angle`).
///
/// Корпус разворачивается по курсу, а не по догадке рендера. Догадка
/// и стояла здесь до ADR-220: `yaw_to` брал **дробную часть
/// положения**, которая при каждом переносе в целую часть
/// перескакивает через ноль. Строй от этого дёргался в такт шагу —
/// не потому, что симуляция дёргалась, а потому, что угол считался
/// не из того.
pub fn yaw_of(head: i32) -> f32 {
    -(head as f32) * std::f32::consts::TAU / fx::angle::FULL as f32
}

/// Точка или направление.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl V3 {
    pub const fn new(x: f32, y: f32, z: f32) -> V3 {
        V3 { x, y, z }
    }

    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn norm(self) -> V3 {
        let l = self.len();
        if l == 0.0 {
            self
        } else {
            self * (1.0 / l)
        }
    }
}

impl std::ops::Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl std::ops::Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl std::ops::Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, k: f32) -> V3 {
        V3::new(self.x * k, self.y * k, self.z * k)
    }
}

/// Матрица 4×4, строками. Соглашение — **вектор-столбец**: `clip = M·v`,
/// поэтому `rows[i]` даёт `i`-ю координату клипа.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct M4 {
    pub rows: [[f32; 4]; 4],
}

impl std::ops::Mul for M4 {
    type Output = M4;
    fn mul(self, o: M4) -> M4 {
        let mut r = [[0.0f32; 4]; 4];
        for (i, ri) in r.iter_mut().enumerate() {
            for (j, rij) in ri.iter_mut().enumerate() {
                let mut s = 0.0;
                for k in 0..4 {
                    s += self.rows[i][k] * o.rows[k][j];
                }
                *rij = s;
            }
        }
        M4 { rows: r }
    }
}

impl M4 {
    /// Умножить на точку. Возвращает клип-координаты вместе с `w`.
    pub fn apply(self, p: V3) -> [f32; 4] {
        let v = [p.x, p.y, p.z, 1.0];
        let mut o = [0.0f32; 4];
        for (i, oi) in o.iter_mut().enumerate() {
            let mut s = 0.0;
            for (k, vk) in v.iter().enumerate() {
                s += self.rows[i][k] * vk;
            }
            *oi = s;
        }
        o
    }
}

/// Взгляд из `eye` на `at`. Правая тройка, вид смотрит вдоль −Z.
pub fn look_at(eye: V3, at: V3, up: V3) -> M4 {
    let f = (at - eye).norm();
    let s = f.cross(up).norm();
    let u = s.cross(f);
    M4 {
        rows: [
            [s.x, s.y, s.z, -s.dot(eye)],
            [u.x, u.y, u.z, -u.dot(eye)],
            [-f.x, -f.y, -f.z, f.dot(eye)],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}

/// Перспектива с глубиной клипа в `[0, w]` — соглашение wgpu, а не
/// OpenGL. Оно же определяет, как достаётся ближняя плоскость пирамиды,
/// поэтому перепутать их — значит отсечь не то.
pub fn perspective(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let f = 1.0 / (fovy_rad * 0.5).tan();
    M4 {
        rows: [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), far * near / (near - far)],
            [0.0, 0.0, -1.0, 0.0],
        ],
    }
}

/// RTS-камера: смотрит в точку на земле с фиксированного наклона,
/// вращается вокруг неё и приближается.
///
/// Единицы — те же, что у воксельной сетки: у мира и у камеры **одна
/// система координат**, иначе отсечение считает не тот объём.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Точка на земле, в которую смотрит камера.
    pub target: V3,
    /// Азимут: поворот вокруг вертикали, радианы.
    pub yaw: f32,
    /// Наклон над горизонтом, радианы. У RTS он большой и почти
    /// не меняется.
    pub pitch: f32,
    /// Расстояние от цели до глаза.
    pub distance: f32,
    pub fovy: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn eye(&self) -> V3 {
        let ch = self.pitch.cos();
        self.target
            + V3::new(
                ch * self.yaw.sin() * self.distance,
                self.pitch.sin() * self.distance,
                ch * self.yaw.cos() * self.distance,
            )
    }

    pub fn view_proj(&self) -> M4 {
        let v = look_at(self.eye(), self.target, V3::new(0.0, 1.0, 0.0));
        perspective(self.fovy, self.aspect, self.near, self.far) * v
    }

    pub fn frustum(&self) -> Frustum {
        Frustum::from(self.view_proj())
    }
}

/// Плоскость `a·x + b·y + c·z + d ≥ 0` — внутри.
#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

impl Plane {
    pub fn distance(&self, p: V3) -> f32 {
        self.a * p.x + self.b * p.y + self.c * p.z + self.d
    }
}

/// Шесть плоскостей пирамиды видимости: лево, право, низ, верх,
/// ближняя, дальняя.
#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    /// Достать плоскости из матрицы вида-проекции.
    ///
    /// Клип по глубине здесь `[0, w]`, поэтому ближняя плоскость —
    /// это сама третья строка, а не сумма третьей с четвёртой, как
    /// было бы в соглашении OpenGL.
    pub fn from(m: M4) -> Frustum {
        let r = m.rows;
        let mk = |v: [f32; 4]| {
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let k = if n == 0.0 { 1.0 } else { 1.0 / n };
            Plane { a: v[0] * k, b: v[1] * k, c: v[2] * k, d: v[3] * k }
        };
        let row_add = |i: usize, j: usize| {
            [
                r[i][0] + r[j][0],
                r[i][1] + r[j][1],
                r[i][2] + r[j][2],
                r[i][3] + r[j][3],
            ]
        };
        let row_sub = |i: usize, j: usize| {
            [
                r[i][0] - r[j][0],
                r[i][1] - r[j][1],
                r[i][2] - r[j][2],
                r[i][3] - r[j][3],
            ]
        };
        Frustum {
            planes: [
                mk(row_add(3, 0)),
                mk(row_sub(3, 0)),
                mk(row_add(3, 1)),
                mk(row_sub(3, 1)),
                mk(r[2]),
                mk(row_sub(3, 2)),
            ],
        }
    }

    /// Пересекается ли параллелепипед с пирамидой.
    ///
    /// Ответ **консервативный**: «нет» означает «точно снаружи», «да»
    /// иногда означает «рядом». Ошибка в эту сторону стоит лишнего
    /// чанка, в обратную — дыры на экране.
    pub fn intersects_aabb(&self, lo: V3, hi: V3) -> bool {
        for p in self.planes.iter() {
            // Дальняя от плоскости вершина: если и она снаружи,
            // снаружи весь ящик.
            let v = V3::new(
                if p.a >= 0.0 { hi.x } else { lo.x },
                if p.b >= 0.0 { hi.y } else { lo.y },
                if p.c >= 0.0 { hi.z } else { lo.z },
            );
            if p.distance(v) < 0.0 {
                return false;
            }
        }
        true
    }
}

/// Сетка чанков мира. Размер чанка здесь — рендерный (62³ у мешинга),
/// и он **не совпадает** с чанком связности или нав-клеткой: у каждой
/// подсистемы своя единица (`docs/02-architecture.md`).
#[derive(Debug, Clone, Copy)]
pub struct ChunkGrid {
    pub nx: u32,
    pub ny: u32,
    pub nz: u32,
    /// Сторона чанка в вокселях.
    pub cs: u32,
}

impl ChunkGrid {
    /// Сколько всего чанков.
    pub fn len(&self) -> usize {
        (self.nx * self.ny * self.nz) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Координаты чанка по линейному индексу. Порядок — растровый:
    /// `x` быстрее всех, `z` медленнее.
    pub fn coords(&self, i: usize) -> (u32, u32, u32) {
        let i = i as u32;
        let cx = i % self.nx;
        let cy = (i / self.nx) % self.ny;
        let cz = i / (self.nx * self.ny);
        (cx, cy, cz)
    }

    pub fn index(&self, cx: u32, cy: u32, cz: u32) -> usize {
        ((cz * self.ny + cy) * self.nx + cx) as usize
    }

    /// Параллелепипед чанка в вокселях.
    pub fn aabb(&self, i: usize) -> (V3, V3) {
        let (cx, cy, cz) = self.coords(i);
        let s = self.cs as f32;
        let lo = V3::new(cx as f32 * s, cy as f32 * s, cz as f32 * s);
        (lo, lo + V3::new(s, s, s))
    }
}

/// Видимые чанки в растровом порядке.
///
/// Порядок фиксирован: рендеру детерминизм не нужен, но список,
/// меняющий порядок от запуска к запуску, невозможно сравнить между
/// двумя прогонами замера.
pub fn visible_chunks(g: &ChunkGrid, f: &Frustum, out: &mut Vec<u32>) {
    out.clear();
    for i in 0..g.len() {
        let (lo, hi) = g.aabb(i);
        if f.intersects_aabb(lo, hi) {
            out.push(i as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        Camera {
            target: V3::new(500.0, 0.0, 500.0),
            yaw: 0.0,
            pitch: 0.9,
            distance: 300.0,
            fovy: 0.9,
            aspect: 16.0 / 9.0,
            near: 1.0,
            far: 2000.0,
        }
    }

    /// **Ближняя плоскость там, где обещано.** Точка на расстоянии
    /// `near` перед глазом обязана быть внутри, а чуть ближе — снаружи.
    /// Перепутанное соглашение по глубине проваливает именно это.
    #[test]
    fn the_near_plane_is_where_it_is_promised() {
        let c = cam();
        let f = c.frustum();
        let dir = (c.target - c.eye()).norm();
        let just_in = c.eye() + dir * (c.near + 0.5);
        let just_out = c.eye() + dir * (c.near - 0.5);
        assert!(f.planes[4].distance(just_in) > 0.0, "точка за ближней снаружи");
        assert!(f.planes[4].distance(just_out) < 0.0, "точка перед ближней внутри");
    }

    /// **Плоскости нормированы**: расстояние до точки — настоящее
    /// расстояние, а не «что-то пропорциональное». Иначе отсечение
    /// по толщине работает только на одной матрице.
    #[test]
    fn planes_are_normalised() {
        let f = cam().frustum();
        for (k, p) in f.planes.iter().enumerate() {
            let n = (p.a * p.a + p.b * p.b + p.c * p.c).sqrt();
            assert!((n - 1.0).abs() < 1e-4, "плоскость {k} не нормирована: {n}");
        }
    }

    /// **То, что за спиной, не видно.** И оно становится видно, если
    /// повернуться.
    #[test]
    fn turning_around_flips_what_is_visible() {
        let g = ChunkGrid { nx: 16, ny: 4, nz: 16, cs: 62 };
        let mut c = cam();
        c.target = V3::new(496.0, 0.0, 496.0);
        let mut a = Vec::new();
        visible_chunks(&g, &c.frustum(), &mut a);
        c.yaw += std::f32::consts::PI;
        let mut b = Vec::new();
        visible_chunks(&g, &c.frustum(), &mut b);
        assert!(!a.is_empty() && !b.is_empty(), "не видно ничего ни там, ни там");
        assert_ne!(a, b, "поворот на сто восемьдесят ничего не изменил");
        // Обзор у RTS-камеры узкий: весь мир целиком в него не влезает.
        assert!(a.len() < g.len(), "видно вообще всё — отсечение не работает");
    }

    /// **Дальняя плоскость режет.** Уменьшив дальность, обязаны увидеть
    /// не больше прежнего.
    #[test]
    fn the_far_plane_cuts() {
        let g = ChunkGrid { nx: 16, ny: 4, nz: 16, cs: 62 };
        let mut c = cam();
        let mut far = Vec::new();
        visible_chunks(&g, &c.frustum(), &mut far);
        c.far = 400.0;
        let mut near = Vec::new();
        visible_chunks(&g, &c.frustum(), &mut near);
        assert!(near.len() < far.len(), "дальняя плоскость ничего не отсекла");
        for i in near.iter() {
            assert!(far.contains(i), "чанк {i} виден вблизи и не виден вдали");
        }
    }

    /// **Ни один видимый чанк не потерян.** Проверка против прямого
    /// критерия: если хоть одна вершина ящика попадает в клип-объём,
    /// чанк обязан быть в списке.
    ///
    /// Обратное неверно и не проверяется: тест по дальней вершине
    /// консервативен, лишние чанки он пропускает нарочно.
    #[test]
    fn no_visible_chunk_is_missed() {
        let g = ChunkGrid { nx: 12, ny: 4, nz: 12, cs: 62 };
        let mut seen = 0;
        let mut vis = Vec::new();
        // Детерминированный перебор камер вместо случайных: воспроизвести
        // провал важнее, чем покрыть больше углов.
        for yi in 0..8 {
            for pi in 0..3 {
                for di in 0..3 {
                    let c = Camera {
                        target: V3::new(300.0 + di as f32 * 60.0, 20.0, 400.0),
                        yaw: yi as f32 * std::f32::consts::TAU / 8.0,
                        pitch: 0.5 + pi as f32 * 0.4,
                        distance: 150.0 + di as f32 * 200.0,
                        fovy: 0.9,
                        aspect: 16.0 / 9.0,
                        near: 1.0,
                        far: 1500.0,
                    };
                    let m = c.view_proj();
                    visible_chunks(&g, &c.frustum(), &mut vis);
                    for i in 0..g.len() {
                        let (lo, hi) = g.aabb(i);
                        let mut inside = false;
                        for k in 0..8 {
                            let p = V3::new(
                                if k & 1 == 0 { lo.x } else { hi.x },
                                if k & 2 == 0 { lo.y } else { hi.y },
                                if k & 4 == 0 { lo.z } else { hi.z },
                            );
                            let v = m.apply(p);
                            let w = v[3];
                            if w > 0.0
                                && v[0] >= -w
                                && v[0] <= w
                                && v[1] >= -w
                                && v[1] <= w
                                && v[2] >= 0.0
                                && v[2] <= w
                            {
                                inside = true;
                                break;
                            }
                        }
                        if inside {
                            seen += 1;
                            assert!(
                                vis.contains(&(i as u32)),
                                "чанк {i} попал в клип, но не в список"
                            );
                        }
                    }
                }
            }
        }
        assert!(seen > 100, "проверка ничего не проверила: попаданий {seen}");
    }

    /// **Нос смотрит туда, куда просили.**
    ///
    /// Проверка против самого соглашения поворота: `+X` после поворота
    /// на `yaw` вокруг `+Y` — это `(cos θ, −sin θ)` в плоскости `XZ`.
    /// Зеркальный знак проходит любую проверку «угол похож на угол»
    /// и валится только здесь.
    #[test]
    fn the_nose_points_where_asked() {
        for (dx, dz) in [
            (1.0f32, 0.0f32),
            (0.0, 1.0),
            (-1.0, 0.0),
            (0.0, -1.0),
            (3.0, 4.0),
            (-2.0, 5.0),
        ] {
            let y = yaw_to(dx, dz);
            let l = (dx * dx + dz * dz).sqrt();
            let (gx, gz) = (y.cos(), -y.sin());
            assert!(
                (gx - dx / l).abs() < 1e-5 && (gz - dz / l).abs() < 1e-5,
                "нос ({gx}, {gz}) вместо ({}, {})",
                dx / l,
                dz / l
            );
        }
    }

    /// **Поворот по курсу — тот же, что по направлению курса.**
    ///
    /// Сторож на знак: `yaw_of` и `yaw_to` обязаны сходиться, иначе
    /// корпус смотрит зеркально, и это не падает и не ругается.
    #[test]
    fn the_heading_yaw_matches_the_direction_yaw() {
        for k in 0..64 {
            let a = k * (fx::angle::FULL / 64);
            let (c, s) = (
                fx::angle::cos(a).to_f64_lossy() as f32,
                fx::angle::sin(a).to_f64_lossy() as f32,
            );
            let (want, got) = (yaw_to(c, s), yaw_of(a));
            let d = (want - got).abs() % std::f32::consts::TAU;
            let d = d.min(std::f32::consts::TAU - d);
            assert!(d < 1e-3, "курс {a}: {got} вместо {want}");
        }
    }

    /// Индекс и координаты — обратные друг другу.
    #[test]
    fn index_and_coords_agree() {
        let g = ChunkGrid { nx: 5, ny: 3, nz: 7, cs: 62 };
        for i in 0..g.len() {
            let (cx, cy, cz) = g.coords(i);
            assert_eq!(g.index(cx, cy, cz), i);
        }
    }
}
