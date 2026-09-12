//! Крупный план моделей: как выглядит тело юнита вблизи.
//!
//! ```text
//! models --shot модели.png --shot-after 4
//! models --bevel 0 --shot кубики.png     # для сравнения
//! ```
//!
//! Плац (`parade`) показывает **соотношение размеров** и потому
//! смотрит издали. Здесь наоборот: три машины крупно, чтобы было
//! видно фаску, роль воксела в цвете и то, как ложится свет
//! на скруглённое ребро.
//!
//! Направление картинки — ключом `--look` и тем же набором чисел,
//! что и у боя (ADR-214): смотреть модели при другом свете значит
//! подгонять их под свет, которого в игре нет.

use bevy::asset::RenderAssetUsages;
use bevy::light::CascadeShadowConfigBuilder;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use render::geom::MeshBuf;
use bevy::anti_alias::fxaa::Fxaa;
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::{Atmosphere, AtmosphereEnvironmentMapLight, SunDisk};
use bevy::pbr::{AtmosphereSettings, ScreenSpaceAmbientOcclusion};
use bevy::post_process::bloom::Bloom;
use bevy::post_process::effect_stack::Vignette;
use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};
use render::edge::{EdgePlugin, Edges};
use render::look;
use render::model::{body_to_mesh, SegFrame, BEVEL};

/// Мировая единица рендера — воксель мира, 0.5 м (ADR-019).
const MM_PER_UNIT: f32 = 500.0;

#[derive(Resource)]
struct Shot {
    path: Option<String>,
    after: f32,
    done: bool,
    left: u32,
    seen: u32,
}

#[derive(Resource)]
struct Scene {
    bevel: f32,
    mm: Option<u16>,
    spin: bool,
    pick: Vec<usize>,
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

fn section(s: look::Section) -> ColorGradingSection {
    ColorGradingSection {
        saturation: s.saturation,
        contrast: s.contrast,
        gamma: s.gamma,
        gain: s.gain,
        lift: s.lift,
    }
}

fn grading(g: look::Grade) -> ColorGrading {
    ColorGrading {
        global: ColorGradingGlobal {
            exposure: g.exposure,
            temperature: g.temperature,
            tint: g.tint,
            hue: 0.0,
            post_saturation: g.post_saturation,
            midtones_range: g.midtones_range.0..g.midtones_range.1,
        },
        shadows: section(g.shadows),
        midtones: section(g.midtones),
        highlights: section(g.highlights),
    }
}

fn main() {
    let mut shot: Option<String> = None;
    let mut after = 4.0f32;
    let mut bevel = BEVEL;
    let mut spin = true;
    // Пересобрать тела в другой воксельной сетке (ADR-208).
    let mut mm: Option<u16> = None;
    // По умолчанию — танк, бот и командная машина: разные силуэты
    // при сравнимом размере, то есть видно и форму, и детали.
    let mut pick = vec![0usize, 1, 3];
    let mut args = std::env::args().skip(1);
    while let Some(k) = args.next() {
        let mut v = || args.next().unwrap_or_default();
        match k.as_str() {
            "--shot" => shot = Some(v()),
            "--shot-after" => after = v().parse().unwrap_or(after),
            "--bevel" => bevel = v().parse().unwrap_or(bevel),
            "--still" => spin = false,
            "--mm" => mm = v().parse().ok(),
            "--pick" => {
                pick = v()
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
            }
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
                    "models [--pick 0,1,3] [--bevel 0.5] [--mm 100] [--still] \
[--shot файл] [--shot-after сек] [--look {}]",
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
    let left = if shot.is_some() { u32::MAX } else { 0 };

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
        .insert_resource(ClearColor(Color::srgb(0.14, 0.16, 0.20)))
        .insert_resource(Shot { path: shot, after, done: false, left, seen: 0 })
        .insert_resource(Scene { bevel, mm, spin, pick })
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

    let picked: Vec<usize> = scene
        .pick
        .iter()
        .copied()
        .filter(|&i| i < all.len())
        .collect();
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
            body_to_mesh(b, tint, b.mm as f32 / MM_PER_UNIT, (0, 255), scene.bevel, centre, &mut bufs);
            tris += bufs.iter().map(|b| b.idx.len() / 3).sum::<usize>();
            let mut tr = Transform::IDENTITY;
            render::model::place(&mut tr, base, Quat::IDENTITY, seg.as_ref());
            for (f, buf) in bufs.iter().enumerate() {
                if buf.idx.is_empty() {
                    continue;
                }
                let mesh = meshes.add(
                    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
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

    // Рамка от габарита шеренги, а не на глаз. Высота цели — по
    // самой высокой машине: иначе кадр уезжает вниз, и половину
    // занимает пустой подиум.
    let tall = picked
        .iter()
        .map(|&i| all[i].1.extent_mm().1 as f32 / MM_PER_UNIT)
        .fold(0.0f32, f32::max);
    // Расстояние от **наибольшего** габарита, а не от ширины шеренги:
    // на одной машине ширина мала, и камера утыкалась ей в борт.
    // Множитель — про то, ради чего смотровая заведена: разглядеть
    // фаску и материал. На 1.7 машины занимали пятую часть кадра,
    // и разница между сталью и стеклом на снимке терялась.
    let dist = span.max(tall) * 1.05;
    // **Смотровая светит тем же небом, что и бой.** Без карты
    // окружения металлу нечего отражать, и он выходит плоской краской
    // — то есть ровно тем, от чего отделки и заводились (ADR-211).
    // Смотреть модели при другом свете значит подгонять их под свет,
    // которого в игре нет.
    let cam = commands
        .spawn((
            Camera3d::default(),
            tonemapping(sc.tonemap),
            grading(sc.grade),
            Exposure { ev100: sc.ev100 },
            Bloom { intensity: sc.bloom.max(0.02), ..Bloom::NATURAL },
            Msaa::Off,
            ScreenSpaceAmbientOcclusion::default(),
            Fxaa::default(),
            AtmosphereSettings { aerial_view_lut_max_distance: sc.haze_range_m, ..default() },
            AtmosphereEnvironmentMapLight {
                intensity: sc.sky_ibl,
                size: UVec2::splat(sc.sky_ibl_size),
                ..default()
            },
            Transform::from_xyz(dist * 0.30, tall * 0.85, dist)
                .looking_at(Vec3::new(0.0, tall * 0.45, 0.0), Vec3::Y),
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
        Transform::from_scale(Vec3::splat(k)).with_translation(Vec3::new(0.0, -EARTH_RADIUS_M * k, 0.0)),
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
        SunDisk { angular_size: sc.sun_disk_size, intensity: sc.sun_disk_intensity },
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

fn turn(time: Res<Time>, scene: Res<Scene>, mut q: Query<(&mut Transform, &Spin)>) {
    if !scene.spin {
        return;
    }
    let rot = Quat::from_rotation_y(time.elapsed_secs() * 0.35);
    for (mut t, s) in &mut q {
        render::model::place(&mut t, s.base, rot, s.seg.as_ref());
    }
}

fn snap(
    mut commands: Commands,
    time: Res<Time>,
    mut s: ResMut<Shot>,
    mut exit: MessageWriter<AppExit>,
) {
    s.seen += 1;
    if !s.done && time.elapsed_secs() >= s.after {
        if let Some(p) = s.path.clone() {
            println!("снимок на {:.1} с: {p}", time.elapsed_secs());
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        }
        s.done = true;
        // Выход не в тот же кадр: запись снимка идёт позже, и
        // мгновенный выход оставляет пустой файл.
        s.left = s.seen + 30;
    }
    if s.left != 0 && s.left != u32::MAX && s.seen >= s.left {
        exit.write(AppExit::Success);
    }
}
