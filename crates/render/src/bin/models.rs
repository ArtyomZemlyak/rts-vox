//! Крупный план моделей: как выглядит тело юнита вблизи.
//!
//! ```text
//! models --pick танк,паук --shot модели.png --shot-after 4
//! models --pick 1 --shot4 танк.png              # четыре стороны
//! models --pick all --still --yaw 45 --elev 35   # вид сверху-сбоку
//! models --bevel 0 --shot кубики.png             # для сравнения
//! ```
//!
//! Плац (`parade`) показывает **соотношение размеров** и потому
//! смотрит издали. Здесь наоборот: несколько машин крупно, чтобы было
//! видно фаску, отделку воксела в материале и то, как ложится свет
//! на скруглённое ребро.
//!
//! Тела выбираются номером в `voxel::roster::all()` (шагоход —
//! последним) **или подстрокой имени**: `--pick 1,14` и
//! `--pick танк,шагоход` — одно и то же. Рамка кадра считается
//! от самой высокой машины в ряду, а не только от ширины ряда: иначе
//! осадному шагоходу резало голову.
//!
//! Направление картинки — ключом `--look` и тем же набором чисел,
//! что и у боя (ADR-214): смотреть модели при другом свете значит
//! подгонять их под свет, которого в игре нет.

use bevy::anti_alias::fxaa::Fxaa;
use bevy::asset::RenderAssetUsages;
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::CascadeShadowConfigBuilder;
use bevy::light::{Atmosphere, AtmosphereEnvironmentMapLight, SunDisk};
use bevy::mesh::Indices;
use bevy::pbr::{AtmosphereSettings, ScreenSpaceAmbientOcclusion};
use bevy::post_process::bloom::Bloom;
use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use render::edge::{EdgePlugin, Edges};
use render::geom::MeshBuf;
use render::look;
use render::model::{body_to_mesh, SegFrame, BEVEL};
use render::occlusion::{Occlusion, OcclusionPlugin};

/// Мировая единица рендера — воксель мира, 0.5 м (ADR-019).
const MM_PER_UNIT: f32 = 500.0;

/// Очередь снимков: каждый — со своим поворотом ряда. Снимок
/// делается **не в тот же кадр**, в котором ряд повёрнут: поворот
/// ставится системой `turn` и попадает в кадр только следующим
/// проходом, а снимок пустого или старого кадра — это снимок ни о чём.
#[derive(Resource)]
struct Shot {
    /// Имя файла и поворот ряда в градусах для каждого снимка.
    queue: Vec<(String, i32)>,
    next: usize,
    /// Ряд уже повёрнут под очередной снимок — осталось снять.
    turned: bool,
    /// Когда делать следующий шаг, секунды от старта.
    at: f32,
    left: u32,
    seen: u32,
}

#[derive(Resource)]
struct Scene {
    bevel: f32,
    mm: Option<u16>,
    spin: bool,
    /// Поворот ряда, когда вращение выключено, градусы.
    yaw: i32,
    /// Высота камеры над горизонтом, градусы.
    elev: f32,
    pick: Vec<String>,
}

/// Что крутится на подиуме: корень тела в `base`, сегмент — на своём
/// месте в теле, повёрнутом вместе с корнем (ADR-231).
#[derive(Component)]
struct Spin {
    base: Vec3,
    seg: Option<SegFrame>,
}

/// Те же переводы набора направления в компоненты, что и у боя
/// (ADR-214). Повторены, а не вынесены: `look` не знает про Bevy
/// намеренно, а заводить ради трёх функций общий модуль окон значит
/// заводить слой из одного файла.
fn tonemapping(t: look::Tonemap) -> Tonemapping {
    match t {
        look::Tonemap::Aces => Tonemapping::AcesFitted,
        look::Tonemap::Tony => Tonemapping::TonyMcMapface,
        look::Tonemap::AgX => Tonemapping::AgX,
        look::Tonemap::Filmic => Tonemapping::BlenderFilmic,
    }
}

/// Имя файла снимка с поворотом: `танк.png` → `танк_090.png`.
fn shot_name(base: &str, yaw: i32) -> String {
    match base.rfind('.') {
        Some(i) if i > base.rfind(['/', '\\']).map_or(0, |k| k + 1) => {
            format!("{}_{:03}{}", &base[..i], yaw.rem_euclid(360), &base[i..])
        }
        _ => format!("{base}_{:03}", yaw.rem_euclid(360)),
    }
}

/// Номера выбранных тел: число — номер, иначе подстрока имени
/// без учёта регистра; `all` — все.
fn resolve(pick: &[String], names: &[&str]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for p in pick {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        if p == "all" {
            out.extend(0..names.len());
            continue;
        }
        if let Ok(i) = p.parse::<usize>() {
            if i < names.len() {
                out.push(i);
            } else {
                eprintln!("--pick {i}: тел всего {}", names.len());
            }
            continue;
        }
        let low = p.to_lowercase();
        let hits: Vec<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.to_lowercase().contains(&low))
            .map(|(i, _)| i)
            .collect();
        if hits.is_empty() {
            eprintln!("--pick {p}: нет такого тела; есть: {}", names.join(", "));
        }
        out.extend(hits);
    }
    out.dedup();
    out
}

fn main() {
    let mut shot: Option<String> = None;
    let mut shot4: Option<String> = None;
    let mut after = 4.0f32;
    let mut bevel = BEVEL;
    let mut spin = true;
    let mut yaw = 0i32;
    let mut elev = 22.0f32;
    // Пересобрать тела в другой воксельной сетке (ADR-208).
    let mut mm: Option<u16> = None;
    // По умолчанию — танк, бот и командная машина: разные силуэты
    // при сравнимом размере, то есть видно и форму, и детали.
    let mut pick: Vec<String> = vec!["0".into(), "1".into(), "3".into()];
    let mut args = std::env::args().skip(1);
    while let Some(k) = args.next() {
        let mut v = || args.next().unwrap_or_default();
        match k.as_str() {
            "--shot" => shot = Some(v()),
            "--shot4" => shot4 = Some(v()),
            "--shot-after" => after = v().parse().unwrap_or(after),
            "--bevel" => bevel = v().parse().unwrap_or(bevel),
            "--still" => spin = false,
            "--yaw" => {
                yaw = v().parse().unwrap_or(yaw);
                spin = false;
            }
            "--elev" => elev = v().parse().unwrap_or(elev),
            "--mm" => mm = v().parse().ok(),
            "--pick" => pick = v().split(',').map(|s| s.trim().to_string()).collect(),
            "--look" => match look::parse_look(&v()) {
                Some(l) => look::set(l),
                None => {
                    eprintln!(
                        "неизвестное направление; есть: {}",
                        look::LOOK_ALL.map(look::look_name).join(", ")
                    );
                    return;
                }
            },
            "--help" | "-h" => {
                println!(
                    "models [--pick 0,1,3 | --pick танк,паук | --pick all] [--bevel 0.5] [--mm 100] \
[--still] [--yaw град] [--elev град] [--shot файл | --shot4 файл] [--shot-after сек] [--look {}]",
                    look::LOOK_ALL.map(look::look_name).join("|")
                );
                return;
            }
            _ => {}
        }
    }
    // Снимок — по **прошедшему времени**, а не по номеру кадра:
    // пайплайны компилируются асинхронно, и пустые кадры идут
    // быстрее, то есть счётчик кадров ловит пустой экран (ADR-172).
    let mut queue: Vec<(String, i32)> = Vec::new();
    if let Some(p) = shot {
        queue.push((p, yaw));
    }
    if let Some(p) = shot4 {
        // Четыре стороны одним ключом: вращение выключается, ряд
        // поворачивается на четверть оборота между снимками.
        spin = false;
        queue.extend([0, 90, 180, 270].map(|q| (shot_name(&p, yaw + q), yaw + q)));
    }
    let left = if queue.is_empty() { 0 } else { u32::MAX };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "rts-vox — модели".into(),
                present_mode: PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        // Тот же проход обводки, что и у боя: направление обязано быть
        // одним и тем же в обоих окнах, иначе модели подгоняются
        // под картинку, которой в игре нет (ADR-214).
        .add_plugins(EdgePlugin)
        // И то же щелевое затенение по вокселям (ADR-235).
        .add_plugins(OcclusionPlugin)
        .insert_resource(ClearColor(Color::srgb(0.14, 0.16, 0.20)))
        .insert_resource(Shot {
            queue,
            next: 0,
            turned: false,
            at: after,
            left,
            seen: 0,
        })
        .insert_resource(Scene {
            bevel,
            mm,
            spin,
            yaw,
            elev,
            pick,
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (turn, snap))
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut mediums: ResMut<Assets<ScatteringMedium>>,
    scene: Res<Scene>,
) {
    let mut all = voxel::roster::all();
    // Ванильный шагоход — тело боя, а не набора; в смотровой он
    // нужен ради ног: там первые повёрнутые звенья (ADR-231).
    all.push(("шагоход", voxel::unit::strider()));
    if let Some(mm) = scene.mm {
        all.retain_mut(|(name, b)| match b.resampled(mm) {
            Ok(r) => {
                *b = r;
                true
            }
            Err(e) => {
                println!("{name}: {}", e.why());
                false
            }
        });
    }
    // Материал **по отделке**: тот же набор, что в бою. Смотровая
    // и есть то место, где разница между сталью, стеклом и резиной
    // видна вблизи (ADR-211).
    let sc = look::scene();
    let mats: [Handle<StandardMaterial>; look::FINISHES] = look::FINISH_ALL.map(|f| {
        let (rough, metal) = look::finish_surface(f);
        materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: rough,
            metallic: metal,
            emissive: if f == look::Finish::Core {
                let g = look::CORE_GLOW;
                LinearRgba::rgb(g[0], g[1], g[2])
            } else {
                LinearRgba::BLACK
            },
            ..default()
        })
    });

    let names: Vec<&str> = all.iter().map(|(n, _)| *n).collect();
    let picked = resolve(&scene.pick, &names);
    if picked.is_empty() {
        println!("ничего не выбрано");
    }
    let widths: Vec<f32> = picked
        .iter()
        .map(|&i| all[i].1.extent_mm().0 as f32 / MM_PER_UNIT)
        .collect();
    let gap = 2.0 * 1000.0 / MM_PER_UNIT;
    let span: f32 = widths.iter().sum::<f32>() + gap * (picked.len() as f32 - 1.0);

    let mut x = -span / 2.0;
    let mut bufs: [MeshBuf; look::FINISHES] = std::array::from_fn(|_| MeshBuf::default());
    for (k, &i) in picked.iter().enumerate() {
        let (name, body) = &all[i];
        let w = widths[k];
        let base = Vec3::new(x + w / 2.0, 0.0, 0.0);
        let tint = look::side_tint()[i % 2];
        // Корень и каждый сегмент — своим мешем вокруг своей точки:
        // корень вокруг середины основания, сегмент вокруг шарнира.
        let frames = render::model::segment_frames(body, 1.0 / MM_PER_UNIT);
        let parts = std::iter::once((body, render::model::root_centre(body), None)).chain(
            body.segments
                .iter()
                .zip(frames)
                .map(|(s, f)| (&s.body, render::model::pivot_centre(&s.attach), Some(f))),
        );
        let mut tris = 0usize;
        for (b, centre, seg) in parts {
            for buf in bufs.iter_mut() {
                buf.clear();
            }
            body_to_mesh(
                b,
                tint,
                b.mm as f32 / MM_PER_UNIT,
                (0, 255),
                scene.bevel,
                centre,
                &mut bufs,
            );
            tris += bufs.iter().map(|b| b.idx.len() / 3).sum::<usize>();
            let mut tr = Transform::IDENTITY;
            render::model::place(&mut tr, base, Quat::IDENTITY, seg.as_ref());
            for (f, buf) in bufs.iter().enumerate() {
                if buf.idx.is_empty() {
                    continue;
                }
                let mesh = meshes.add(
                    Mesh::new(
                        PrimitiveTopology::TriangleList,
                        RenderAssetUsages::RENDER_WORLD,
                    )
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, buf.pos.clone())
                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, buf.nrm.clone())
                    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, buf.col.clone())
                    .with_inserted_indices(Indices::U32(buf.idx.clone())),
                );
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(mats[f].clone()),
                    tr,
                    Spin { base, seg },
                ));
            }
        }
        println!(
            "{name}: {tris} треугольников, сегментов {}, фаска {:.2}",
            body.segments.len(),
            scene.bevel
        );
        x += w + gap;
    }

    // Подиум: тёмный, чтобы силуэт машины читался контуром.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(span * 1.3, 1.0, span * 0.55))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.11, 0.13),
            perceptual_roughness: 0.8,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    // **Рамка от габарита ряда, а не на глаз.** Ряд обязан влезть
    // и по ширине, и по высоте: расстояние берётся большим из двух —
    // от ширины через горизонтальный угол обзора, от высоты через
    // вертикальный. Прежняя рамка считалась от ширины, и высоким
    // машинам резало голову. Угол обзора — умолчание Bevy (45°
    // по вертикали), кадр 16:9.
    let tall = picked
        .iter()
        .map(|&i| all[i].1.extent_mm().1 as f32 / MM_PER_UNIT)
        .fold(0.0f32, f32::max)
        .max(1.0);
    let deep = picked
        .iter()
        .map(|&i| all[i].1.extent_mm().2 as f32 / MM_PER_UNIT)
        .fold(0.0f32, f32::max);
    // Повёрнутый ряд занимает по ширине до длины машины, по глубине
    // тоже. Расстояние считается **до ближней грани**, а не до
    // середины: перспектива раздувает то, что ближе, и ряд, рамка
    // которого сошлась на середине, вылезал из кадра носом.
    let long = widths.iter().copied().fold(0.0f32, f32::max);
    let wide = span.max(deep) * 1.3;
    let half_v = (std::f32::consts::FRAC_PI_4 / 2.0).tan();
    let half_h = half_v * 16.0 / 9.0;
    let dist = (wide / 2.0 / half_h).max(tall * 1.3 / 2.0 / half_v) + deep.max(long) / 2.0;
    let elev = scene.elev.to_radians();
    let az = 17.0f32.to_radians();
    let target = Vec3::new(0.0, tall * 0.45, 0.0);
    let eye = target + dist * Vec3::new(az.sin() * elev.cos(), elev.sin(), az.cos() * elev.cos());
    // **Смотровая светит тем же небом, что и бой.** Без карты
    // окружения металлу нечего отражать, и он выходит плоской краской
    // — то есть ровно тем, от чего отделки и заводились (ADR-211).
    // Смотреть модели при другом свете значит подгонять их под свет,
    // которого в игре нет.
    let cam = commands
        .spawn((
            Camera3d::default(),
            tonemapping(sc.tonemap),
            render::presentation::grading(sc.grade),
            Exposure { ev100: sc.ev100 },
            Bloom {
                intensity: sc.bloom.max(0.02),
                ..Bloom::NATURAL
            },
            Msaa::Off,
            ScreenSpaceAmbientOcclusion::default(),
            Fxaa::default(),
            AtmosphereSettings {
                aerial_view_lut_max_distance: sc.haze_range_m,
                ..default()
            },
            AtmosphereEnvironmentMapLight {
                intensity: sc.sky_ibl,
                size: UVec2::splat(sc.sky_ibl_size),
                ..default()
            },
            Transform::from_translation(eye).looking_at(target, Vec3::Y),
        ))
        .id();
    if sc.vignette.intensity > 0.0 {
        let v = sc.vignette;
        commands.entity(cam).insert(Vignette {
            intensity: v.intensity,
            radius: v.radius,
            smoothness: v.smoothness,
            roundness: v.roundness,
            color: Color::srgb(v.srgb[0], v.srgb[1], v.srgb[2]),
            ..default()
        });
    }
    if sc.occlusion.on() {
        let o = sc.occlusion;
        commands.entity(cam).insert(Occlusion {
            radius: o.radius,
            strength: o.strength,
            bias: o.bias,
            power: o.power,
        });
    }
    if sc.edge.strength > 0.0 {
        let e = sc.edge;
        let c = look::to_linear(e.srgb);
        commands.entity(cam).insert(Edges {
            colour: Vec4::new(c[0], c[1], c[2], 1.0),
            width: e.width,
            depth: e.depth,
            normal: e.normal,
            strength: e.strength,
            steps: e.steps,
            posterise: e.posterise,
            pad0: 0.0,
            pad1: 0.0,
        });
    }
    // Глубины резкости у смотровой нет намеренно: она заведена, чтобы
    // разглядеть отделку, а размытие прячет ровно её.
    //
    // Планета под сценой. Масштаб тот же, что в бою: рассеяние считает
    // метры, подиум — воксели.
    let k = 1.0 / sc.haze_metres_per_voxel;
    let medium = mediums.add(ScatteringMedium::earth(256, 256));
    let atmo = commands.spawn(Atmosphere::earth(medium)).id();
    commands.entity(atmo).insert(GlobalTransform::from(
        Transform::from_scale(Vec3::splat(k)).with_translation(Vec3::new(
            0.0,
            -EARTH_RADIUS_M * k,
            0.0,
        )),
    ));

    // Свет — из общей палитры (`look`), чтобы модели выглядели так же,
    // как в бою, а не «красиво отдельно».
    // Направление солнца теперь вычисляется из высоты и азимута
    // (ADR-198), а не задано вектором.
    let d = look::sun_dir();
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(sc.sun_srgb[0], sc.sun_srgb[1], sc.sun_srgb[2]),
            illuminance: sc.sun_lux,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-d[0], -d[1], -d[2]).looking_at(Vec3::ZERO, Vec3::Y),
        SunDisk {
            angular_size: sc.sun_disk_size,
            intensity: sc.sun_disk_intensity,
        },
        CascadeShadowConfigBuilder {
            num_cascades: 2,
            first_cascade_far_bound: span * 0.5,
            maximum_distance: span * 3.0,
            ..default()
        }
        .build(),
    ));
}

/// Радиус планеты у `Atmosphere::earth`, метры. Столько же, сколько
/// в бою: центр сферы обязан лежать под сценой, а не в ней.
const EARTH_RADIUS_M: f32 = 6_360_000.0;

/// Поворот ряда: по времени, если вращение включено, иначе — заданный.
fn turn(time: Res<Time>, scene: Res<Scene>, mut q: Query<(&mut Transform, &Spin)>) {
    let angle = if scene.spin {
        time.elapsed_secs() * 0.35
    } else {
        (scene.yaw as f32).to_radians()
    };
    let rot = Quat::from_rotation_y(angle);
    for (mut t, s) in &mut q {
        render::model::place(&mut t, s.base, rot, s.seg.as_ref());
    }
}

fn snap(
    mut commands: Commands,
    time: Res<Time>,
    mut s: ResMut<Shot>,
    mut scene: ResMut<Scene>,
    mut exit: MessageWriter<AppExit>,
) {
    s.seen += 1;
    let now = time.elapsed_secs();
    if s.next < s.queue.len() && now >= s.at {
        if !s.turned {
            // Сначала повернуть ряд, снять — следующим шагом: поворот
            // попадает в кадр только после прохода `turn`.
            scene.yaw = s.queue[s.next].1;
            s.turned = true;
            s.at = now + 0.4;
        } else {
            let p = s.queue[s.next].0.clone();
            println!("снимок на {now:.1} с: {p}");
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(p));
            s.next += 1;
            s.turned = false;
            s.at = now + 0.4;
            if s.next == s.queue.len() {
                // Выход не в тот же кадр: запись снимка идёт позже, и
                // мгновенный выход оставляет пустой файл.
                s.left = s.seen + 30;
            }
        }
    }
    if s.left != 0 && s.left != u32::MAX && s.seen >= s.left {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Имя снимка получает поворот перед расширением, а точка
    /// в каталоге за расширение не принимается.
    #[test]
    fn shot_names_carry_the_turn() {
        assert_eq!(shot_name("танк.png", 90), "танк_090.png");
        assert_eq!(shot_name("a.b/танк", 270), "a.b/танк_270");
        assert_eq!(shot_name("танк.png", -90), "танк_270.png");
    }

    /// Выбор — номером, подстрокой имени или всем набором.
    #[test]
    fn picks_resolve_by_number_or_name() {
        let names = ["гусеничный разведчик", "танк", "паук"];
        let p = |s: &str| resolve(&[s.to_string()], &names);
        assert_eq!(p("1"), vec![1]);
        assert_eq!(p("Паук"), vec![2]);
        assert_eq!(p("разведчик"), vec![0]);
        assert_eq!(p("all"), vec![0, 1, 2]);
        assert!(p("9").is_empty());
    }
}
