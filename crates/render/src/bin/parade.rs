//! Плац: весь набор тел в одну шеренгу, в истинном соотношении размеров.
//!
//! ```text
//! parade --shot плац.png --shot-after 3
//! parade --group light --shot мелочь.png
//! ```
//!
//! Зачем отдельное окно, а не бой: в бою масштаб не проверишь. Машины
//! стоят далеко друг от друга, камера высоко, и танк рядом с гигантом
//! в кадр не попадает. Здесь они стоят в ряд на размеченной земле,
//! и соотношение видно глазом.
//!
//! **Разметка земли — каждые десять метров.** Без неё «большой» и
//! «очень большой» на картинке неразличимы, а число в отчёте ничем
//! не подтверждается.

use bevy::asset::RenderAssetUsages;
use bevy::light::CascadeShadowConfigBuilder;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PresentMode;
use render::geom::{volume_to_mesh, MeshBuf, Slice};

/// Мировая единица рендера — воксель мира, 0.5 м (ADR-019). Отсюда
/// пересчёт габаритов тела: воксель тела в миллиметрах делится на
/// половину метра.
const MM_PER_UNIT: f32 = 500.0;

/// Метр в единицах сцены.
const M: f32 = 1000.0 / MM_PER_UNIT;

#[derive(Resource)]
struct Snap {
    path: Option<String>,
    after: f32,
    done: bool,
    frames: u32,
    seen: u32,
}

#[derive(Resource)]
struct Scene {
    group: String,
    span: f32,
}

fn main() {
    let mut shot: Option<String> = None;
    let mut after = 3.0f32;
    let mut frames = 0u32;
    let mut group = "all".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(k) = args.next() {
        let mut v = || args.next().unwrap_or_default();
        match k.as_str() {
            "--shot" => shot = Some(v()),
            "--shot-after" => after = v().parse().unwrap_or(after),
            "--frames" => frames = v().parse().unwrap_or(frames),
            "--group" => group = v(),
            "--help" | "-h" => {
                println!("parade [--group all|light|heavy] [--shot файл] [--shot-after сек] [--frames N]");
                return;
            }
            _ => {}
        }
    }
    // Кадр снимается по **прошедшему времени**, а не по номеру кадра:
    // пайплайны компилируются асинхронно, пустые кадры идут быстрее,
    // и счётчик кадров ловит пустой экран (ADR-172).
    if shot.is_some() && frames == 0 {
        frames = u32::MAX;
    }

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "rts-vox — плац".into(),
                present_mode: PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.55, 0.68, 0.82)))
        .insert_resource(Snap {
            path: shot,
            after,
            done: false,
            frames,
            seen: 0,
        })
        .insert_resource(Scene { group, span: 0.0 })
        .add_systems(Startup, setup)
        .add_systems(Update, snap)
        .run();
}

/// Цвет стороны у каждой машины — чтобы в шеренге они не сливались.
const TINTS: [[f32; 3]; 6] = [
    [0.26, 0.46, 0.76],
    [0.30, 0.62, 0.44],
    [0.72, 0.56, 0.20],
    [0.74, 0.74, 0.78],
    [0.68, 0.28, 0.30],
    [0.44, 0.34, 0.62],
];

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut scene: ResMut<Scene>,
) {
    let all = voxel::roster::all();
    let picked: Vec<usize> = match scene.group.as_str() {
        "light" => (0..4).collect(),
        "heavy" => (4..all.len()).collect(),
        _ => (0..all.len()).collect(),
    };

    // Цвет несут вершины: у тома роль воксела своя у брони, орудия
    // и сенсора, и материал обязан её пропустить.
    let body_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.55,
        metallic: 0.15,
        ..default()
    });

    // Ширина шеренги: сумма длин плюс промежуток в пять метров.
    let gap = 5.0 * M;
    let widths: Vec<f32> = picked
        .iter()
        .map(|&i| all[i].1.extent_mm().0 as f32 / MM_PER_UNIT)
        .collect();
    let span: f32 = widths.iter().sum::<f32>() + gap * (picked.len() as f32 - 1.0);
    scene.span = span;

    let mut x = -span / 2.0;
    let mut buf = MeshBuf::default();
    for (k, &i) in picked.iter().enumerate() {
        let (name, body) = &all[i];
        let w = widths[k];
        // Воксель тела в единицах сцены: у крупных машин он вчетверо
        // крупнее, и это ровно то, что делает их крупными (ADR-178).
        let s = body.mm as f32 / MM_PER_UNIT;
        buf.clear();
        // Плац показывает тело целиком: башня здесь не вращается,
        // ей не в кого целиться.
        volume_to_mesh(body, TINTS[i % TINTS.len()], s, Slice::All, &mut buf);
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
        // `volume_to_mesh` ставит том серединой в начало координат
        // по горизонтали и низом — по высоте.
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(body_mat.clone()),
            Transform::from_xyz(x + w / 2.0, 0.0, 0.0),
        ));
        println!(
            "{name}: {:.1} м, воксель {} мм, {} вокселей",
            body.extent_mm().0 as f32 / 1000.0,
            body.mm,
            body.solid_count()
        );
        x += w + gap;
    }

    // Земля и разметка по десять метров.
    let ground = span * 1.9;
    let depth = 60.0 * M;
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(ground, 1.0, depth))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.42, 0.44, 0.38),
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let stripe = materials.add(StandardMaterial {
        base_color: Color::srgb(0.56, 0.58, 0.50),
        perceptual_roughness: 0.95,
        ..default()
    });
    let step = 10.0 * M;
    let n = (ground / 2.0 / step) as i32;
    let bar = meshes.add(Cuboid::new(0.4 * M, 1.0, depth));
    for i in -n..=n {
        commands.spawn((
            Mesh3d(bar.clone()),
            MeshMaterial3d(stripe.clone()),
            Transform::from_xyz(i as f32 * step, -0.45, 0.0),
        ));
    }

    // Камера: вся шеренга в кадр, чуть сверху и сбоку. Расстояние
    // берётся от ширины шеренги, а не подбирается на глаз, иначе
    // при смене набора кадр разъезжается.
    // Расстояние — из угла обзора, а не на глаз: при вертикальном
    // угле 45 градусов и кадре 16:9 в ширину видно примерно 1.47
    // расстояния, и шеренге нужен запас, иначе крайняя машина
    // срезается краем кадра.
    let dist = span * 0.95;
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, dist * 0.26, dist)
            .looking_at(Vec3::new(0.0, span * 0.05, 0.0), Vec3::Y),
    ));

    // Солнце с тенями плюс мягкая подсветка снизу: без второй лампы
    // теневая сторона гиганта становится чёрным пятном и силуэт
    // теряется ровно у того, ради кого сцена и собрана.
    commands.spawn((
        DirectionalLight {
            illuminance: bevy::light::light_consts::lux::FULL_DAYLIGHT,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(0.5, 0.85, 0.35).looking_at(Vec3::ZERO, Vec3::Y),
        // Каскады считаются от размера шеренги, а не от умолчания:
        // у Bevy предел каскада двести с небольшим единиц, а плац
        // впятеро длиннее — тени просто не доезжают до края.
        CascadeShadowConfigBuilder {
            num_cascades: 4,
            first_cascade_far_bound: span * 0.20,
            maximum_distance: span * 2.0,
            ..default()
        }
        .build(),
    ));
    // Подсветка неба — ровно столько, чтобы теневая сторона читалась.
    // На 380 картинка становилась пастельной: тени тонули в заливке,
    // и форма пропадала у того, ради кого сцена и собрана.
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.55, 0.66, 0.90),
        brightness: 90.0,
        ..default()
    });
}

fn snap(
    mut commands: Commands,
    time: Res<Time>,
    mut s: ResMut<Snap>,
    mut exit: MessageWriter<AppExit>,
) {
    s.seen += 1;
    if !s.done && time.elapsed_secs() >= s.after {
        if let Some(p) = s.path.clone() {
            println!("снимок на {:.1} с: {p}", time.elapsed_secs());
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(p));
        }
        s.done = true;
    }
    // Выход через несколько кадров после снимка: запись идёт не в тот
    // же кадр, и мгновенный выход оставляет пустой файл.
    if s.done && s.frames == u32::MAX && s.seen > 0 {
        s.frames = s.seen + 30;
    }
    if s.frames != 0 && s.frames != u32::MAX && s.seen >= s.frames {
        exit.write(AppExit::Success);
    }
}
