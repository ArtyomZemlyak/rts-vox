//! Пыль, дым и искры: частицы рендера (ADR-235).
//!
//! **Это декорация.** Клуб пыли ничего не весит, ни во что не попадает
//! и в состояние симуляции не входит: он читает событие или движение
//! машины и живёт своей жизнью в кадрах (`docs/13-gpu.md`: частицы —
//! рендерный режим). Границу слоёв это не задевает — рендер по-прежнему
//! только читает.
//!
//! Зачем они нужны. Камера стратегии стоит высоко, и с неё машина —
//! это пятно в несколько пикселей. Пятно не показывает ни движения,
//! ни боя. Пыль за колонной, дым над подбитой машиной и искры от луча
//! видны с любой высоты и говорят, где идёт бой, раньше, чем глаз
//! найдёт сами машины.
//!
//! Устройство то же, что у вспышек боя (ADR-200): возраст в кадрах,
//! лестница материалов по возрасту, а числа — в [`crate::look`].
//! Материал общий у всех, кто на него ссылается, поэтому «стареет»
//! не материал, а ссылка: клуб переходит с одной ступени лестницы
//! на другую.
//!
//! Форма — шар малого разбиения для пыли и дыма, кубик для искры.
//! Своего шейдера у частиц нет намеренно (`CLAUDE.md`, шейдеры
//! самодостаточны): шар со смешиванием по прозрачности освещается
//! тем же солнцем и красится той же атмосферой, что и всё остальное,
//! и это важнее мягкого края.

use bevy::prelude::*;

use crate::look;

/// Что за частица. Различаются не только цветом: пыль оседает,
/// дым поднимается, искра летит по параболе и гаснет.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PuffKind {
    /// Пыль из-под гусениц и от взрыва: цвета почвы, тяжёлая.
    Dust,
    /// Дым над подбитой машиной: тёмный, лёгкий, поднимается.
    Smoke,
    /// Искра: светится, летит быстро, живёт коротко.
    Spark,
}

/// Живая частица.
#[derive(Component)]
pub struct Puff {
    pub kind: PuffKind,
    pub age: u32,
    pub life: u32,
    /// Скорость в вокселях за кадр.
    pub vel: Vec3,
    /// Размер в начале и в конце жизни, в вокселях.
    pub size0: f32,
    pub size1: f32,
}

/// Источник дыма: подбитая машина дымит долго, а событие смерти
/// приходит один раз. Сущность живёт, пока не выдаст свою долю клубов.
#[derive(Component)]
pub struct Smoker {
    pub at: Vec3,
    /// Сколько клубов ещё выпустить.
    pub left: u32,
    /// Через сколько кадров следующий.
    pub wait: u32,
    /// Соль для разброса.
    pub seed: u32,
}

/// Общие меши и лестницы материалов.
#[derive(Resource)]
pub struct DustArt {
    ball: Handle<Mesh>,
    chip: Handle<Mesh>,
    dust: Vec<Handle<StandardMaterial>>,
    smoke: Vec<Handle<StandardMaterial>>,
    spark: Vec<Handle<StandardMaterial>>,
}

/// Частицы: ресурс с мешами и система старения.
pub struct DustPlugin;

impl Plugin for DustPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup).add_systems(Update, age);
    }
}

/// Лестница материалов клуба: цвет один, прозрачность спадает
/// по возрасту. Смешивание по прозрачности, а не сложением: пыль
/// заслоняет то, что за ней, — в этом её смысл.
fn ladder(
    m: &mut Assets<StandardMaterial>,
    srgb: [f32; 3],
    alpha: f32,
    steps: usize,
) -> Vec<Handle<StandardMaterial>> {
    (0..steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            // Держится, потом уходит: линейное угасание делает клуб
            // прозрачным уже в середине жизни, и роста не видно.
            let a = alpha * (1.0 - t * t);
            m.add(StandardMaterial {
                base_color: Color::srgba(srgb[0], srgb[1], srgb[2], a),
                perceptual_roughness: 1.0,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })
        })
        .collect()
}

/// Искра светится сама, и её видно сквозь всё: сложение, не смешивание.
fn glow_ladder(
    m: &mut Assets<StandardMaterial>,
    hot: [f32; 3],
    steps: usize,
) -> Vec<Handle<StandardMaterial>> {
    (0..steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let k = (1.0 - t) * (1.0 - t);
            m.add(StandardMaterial {
                base_color: Color::LinearRgba(LinearRgba::new(hot[0] * k, hot[1] * k, hot[2] * k, 1.0)),
                alpha_mode: AlphaMode::Add,
                unlit: true,
                ..default()
            })
        })
        .collect()
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let d = look::dust();
    commands.insert_resource(DustArt {
        // Шар самого грубого разбиения: клуб пыли размером в несколько
        // пикселей не оправдывает ни одного лишнего треугольника.
        ball: meshes.add(Sphere::new(1.0).mesh().ico(1).expect("шар пыли")),
        chip: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        dust: ladder(&mut materials, d.dust_srgb, d.dust_alpha, look::PUFF_STEPS),
        smoke: ladder(&mut materials, d.smoke_srgb, d.smoke_alpha, look::PUFF_STEPS),
        spark: glow_ladder(&mut materials, look::SPARK_GLOW, look::PUFF_STEPS),
    });
}

/// Целое из трёх координат и номера — для разброса.
///
/// Свой хеш, а не `rand`: два прогона одной сцены обязаны дать
/// одинаковый снимок. В хеш состояния это не входит и входить
/// не может — рендер его не считает.
pub fn hash(x: i32, y: i32, z: i32, k: u32) -> u32 {
    let mut h = 0x9E37_79B9u32;
    for v in [x as u32, y as u32, z as u32, k] {
        h ^= v.wrapping_mul(0x85EB_CA6B);
        h = h.rotate_left(13).wrapping_mul(0xC2B2_AE35);
    }
    h ^= h >> 16;
    h
}

/// Дробь `0..1` из хеша.
pub fn frac(h: u32) -> f32 {
    (h & 0xFF_FFFF) as f32 / 16_777_216.0
}

/// Дробь `−1..1` из хеша.
fn signed(h: u32) -> f32 {
    frac(h) * 2.0 - 1.0
}

/// Один клуб.
#[allow(clippy::too_many_arguments)]
pub fn puff(
    commands: &mut Commands,
    art: &DustArt,
    kind: PuffKind,
    at: Vec3,
    vel: Vec3,
    size0: f32,
    size1: f32,
    life: u32,
) {
    let (mesh, mat) = match kind {
        PuffKind::Dust => (art.ball.clone(), art.dust[0].clone()),
        PuffKind::Smoke => (art.ball.clone(), art.smoke[0].clone()),
        PuffKind::Spark => (art.chip.clone(), art.spark[0].clone()),
    };
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(at).with_scale(Vec3::splat(size0.max(0.01))),
        Puff { kind, age: 0, life: life.max(1), vel, size0, size1 },
    ));
}

/// Пыль из-под машины, которая едет.
///
/// `at` — положение машины, `back` — единичный вектор **против** хода,
/// `len` — длина машины в вокселях: клуб рождается за кормой
/// и разлетается в стороны шириной с машину.
pub fn track_dust(commands: &mut Commands, art: &DustArt, at: Vec3, back: Vec3, len: f32, seed: u32) {
    let d = look::dust();
    let side = Vec3::new(-back.z, 0.0, back.x);
    let h = hash(at.x as i32, at.z as i32, len as i32, seed);
    let s = signed(h >> 3) * len * 0.25;
    let p = at + back * (len * 0.45) + side * s + Vec3::Y * (len * 0.04);
    // Клуб отстаёт от машины и чуть поднимается: пыль поднята
    // и оседает, а не висит.
    let vel = back * (len * d.dust_drift) + Vec3::Y * (len * d.dust_rise) + side * (signed(h >> 9) * len * 0.004);
    puff(commands, art, PuffKind::Dust, p, vel, len * d.dust_size.0, len * d.dust_size.1, d.dust_frames);
}

/// Пыль от взрыва: несколько клубов вокруг воронки радиуса `r`.
pub fn blast_dust(commands: &mut Commands, art: &DustArt, at: Vec3, r: f32, key: (i32, i32, i32)) {
    let d = look::dust();
    for k in 0..d.blast_puffs {
        let h = hash(key.0, key.1, key.2, 0x0D00 + k as u32);
        let ang = frac(h) * core::f32::consts::TAU;
        let out = 0.3 + 0.5 * frac(h >> 7);
        let dir = Vec3::new(ang.cos() * out, 0.6 + 0.6 * frac(h >> 13), ang.sin() * out);
        let vel = dir * (r * d.blast_speed);
        puff(
            commands,
            art,
            PuffKind::Dust,
            at + dir * (r * 0.4),
            vel,
            r * d.blast_size.0,
            r * d.blast_size.1,
            d.dust_frames + d.dust_frames / 2,
        );
    }
    sparks(commands, art, at, r.sqrt().max(1.0), key, d.blast_sparks);
}

/// Искры из точки: `n` кубиков разлетаются веером и падают.
pub fn sparks(commands: &mut Commands, art: &DustArt, at: Vec3, scale: f32, key: (i32, i32, i32), n: usize) {
    let d = look::dust();
    for k in 0..n {
        let h = hash(key.0, key.1, key.2, 0x5A00 + k as u32);
        let ang = frac(h) * core::f32::consts::TAU;
        let up = 0.3 + 0.9 * frac(h >> 11);
        let vel = Vec3::new(ang.cos(), up, ang.sin()).normalize() * (d.spark_speed * scale * (0.6 + 0.6 * frac(h >> 17)));
        puff(
            commands,
            art,
            PuffKind::Spark,
            at,
            vel,
            d.spark_size * scale,
            d.spark_size * scale * 0.4,
            d.spark_frames,
        );
    }
}

/// Подбитая машина дымит: источник, который выпускает клубы по одному.
pub fn wreck_smoke(commands: &mut Commands, at: Vec3, key: (i32, i32, i32)) {
    let d = look::dust();
    commands.spawn(Smoker { at, left: d.smoke_puffs, wait: 0, seed: hash(key.0, key.1, key.2, 0x5E0) });
}

/// Старение: клубы растут, плывут и гаснут; источники дыма выпускают
/// клубы; отжившее убирается.
fn age(
    mut commands: Commands,
    art: Res<DustArt>,
    mut puffs: Query<(Entity, &mut Puff, &mut Transform, &mut MeshMaterial3d<StandardMaterial>)>,
    mut smokers: Query<(Entity, &mut Smoker)>,
) {
    let d = look::dust();
    for (e, mut p, mut tr, mut mat) in puffs.iter_mut() {
        if p.age >= p.life {
            commands.entity(e).despawn();
            continue;
        }
        let t = p.age as f32 / p.life as f32;
        // Рост по корню: клуб раздувается сразу и потом только плывёт.
        let k = t.sqrt();
        tr.scale = Vec3::splat((p.size0 + (p.size1 - p.size0) * k).max(0.01));
        let v = p.vel;
        tr.translation += v;
        match p.kind {
            PuffKind::Dust => {
                // Пыль тормозится воздухом и оседает.
                p.vel *= d.dust_drag;
                p.vel.y -= d.dust_settle * p.size1;
            }
            PuffKind::Smoke => {
                p.vel *= d.smoke_drag;
                p.vel.y += d.smoke_lift * p.size1;
            }
            PuffKind::Spark => {
                p.vel.y -= d.spark_g * p.size0.max(0.05);
                let h = hash(p.age as i32, e.index_u32() as i32, 0, 0x9);
                tr.rotation *= Quat::from_euler(EulerRot::XYZ, 0.3 * signed(h), 0.2, 0.3 * signed(h >> 5));
            }
        }
        let steps = match p.kind {
            PuffKind::Dust => &art.dust,
            PuffKind::Smoke => &art.smoke,
            PuffKind::Spark => &art.spark,
        };
        let i = ((t * steps.len() as f32) as usize).min(steps.len() - 1);
        if mat.0 != steps[i] {
            mat.0 = steps[i].clone();
        }
        p.age += 1;
    }

    for (e, mut s) in smokers.iter_mut() {
        if s.left == 0 {
            commands.entity(e).despawn();
            continue;
        }
        if s.wait > 0 {
            s.wait -= 1;
            continue;
        }
        let h = hash(s.at.x as i32, s.at.z as i32, s.left as i32, s.seed);
        let r = d.smoke_size.0;
        let at = s.at + Vec3::new(signed(h) * r, r * 0.5, signed(h >> 7) * r);
        // Ветра нет, но дым не стоит столбом: лёгкий снос в одну
        // сторону на всю жизнь источника.
        let drift = Vec3::new(signed(s.seed) * 0.5, 1.0, signed(s.seed >> 9) * 0.5).normalize();
        let vel = drift * d.smoke_speed * (0.7 + 0.6 * frac(h >> 13));
        puff(&mut commands, &art, PuffKind::Smoke, at, vel, d.smoke_size.0, d.smoke_size.1, d.smoke_frames);
        s.left -= 1;
        s.wait = d.smoke_every;
    }
}
