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

use bevy::asset::RenderAssetUsages;
use bevy::light::CascadeShadowConfigBuilder;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use render::geom::MeshBuf;
use render::model::{body_to_mesh, BEVEL};

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

#[derive(Component)]
struct Spin;

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
            "--help" | "-h" => {
                println!(
                    "models [--pick 0,1,3] [--bevel 0.5] [--mm 100] [--still] [--shot файл] [--shot-after сек]"
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
    scene: Res<Scene>,
) {
    let mut all = voxel::roster::all();
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
    let mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.45,
        metallic: 0.25,
        ..default()
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
    let mut buf = MeshBuf::default();
    for (k, &i) in picked.iter().enumerate() {
        let (name, body) = &all[i];
        let w = widths[k];
        let s = body.mm as f32 / MM_PER_UNIT;
        buf.clear();
        body_to_mesh(
            body,
            render::look::SIDE_TINT[i % render::look::SIDE_TINT.len()],
            s,
            (0, 255),
            scene.bevel,
            &mut buf,
        );
        println!(
            "{name}: {} треугольников, фаска {:.2}",
            buf.idx.len() / 3,
            scene.bevel
        );
        let mesh = meshes.add(
            Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, buf.pos.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, buf.nrm.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, buf.col.clone())
                .with_inserted_indices(Indices::U32(buf.idx.clone())),
        );
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(mat.clone()),
            Transform::from_xyz(x + w / 2.0, 0.0, 0.0),
            Spin,
        ));
        x += w + gap;
    }

    // Подиум: тёмный, чтобы силуэт машины читался контуром.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(span * 2.0, 1.0, span))),
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
    let dist = span.max(tall) * 1.7;
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(dist * 0.30, tall * 0.85, dist)
            .looking_at(Vec3::new(0.0, tall * 0.45, 0.0), Vec3::Y),
    ));

    // Свет — из общей палитры (`look`), чтобы модели выглядели так же,
    // как в бою, а не «красиво отдельно».
    // Направление солнца теперь вычисляется из высоты и азимута
    // (ADR-198), а не задано вектором.
    let d = render::look::sun_dir();
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(
                render::look::SUN_SRGB[0],
                render::look::SUN_SRGB[1],
                render::look::SUN_SRGB[2],
            ),
            illuminance: render::look::SUN_LUX,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-d[0], -d[1], -d[2]).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: 2,
            first_cascade_far_bound: span * 0.5,
            maximum_distance: span * 3.0,
            ..default()
        }
        .build(),
    ));
    // Контровой свет сзади: без него скруглённое ребро на теневой
    // стороне не отличить от острого — а ради ребра всё и затевалось.
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.55, 0.68, 0.95),
            illuminance: render::look::SUN_LUX * 0.06,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.4, 0.5, -1.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(
            render::look::AMBIENT_SRGB[0],
            render::look::AMBIENT_SRGB[1],
            render::look::AMBIENT_SRGB[2],
        ),
        brightness: render::look::AMBIENT_BRIGHTNESS * 3.0,
        ..default()
    });
}

fn turn(time: Res<Time>, scene: Res<Scene>, mut q: Query<&mut Transform, With<Spin>>) {
    if !scene.spin {
        return;
    }
    for mut t in &mut q {
        t.rotation = Quat::from_rotation_y(time.elapsed_secs() * 0.35);
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
