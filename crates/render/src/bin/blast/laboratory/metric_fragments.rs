//! Metric colours used by every original fragment draw, including resting batches.
use super::*;
use sim_core::burst::Fragment;
use std::collections::BTreeMap;
pub(crate) fn colour(f: &Fragment, sim: &BlastRange, mode: u8) -> [f32; 4] {
    let def = sim.matter.economy.mats.get(f.material);
    match mode {
        1 => sim
            .matter
            .debris
            .molten_heat(f.id)
            .map_or(metrics::UNKNOWN, |energy| {
                metrics::scale(sim_core::blast::thermal::temperature(energy, f.mm), 1000)
            }),
        3 => {
            if matches!(f.material, 11 | 13 | 14 | 16) {
                [0.1, 0.45, 0.95, 1.0]
            } else if def.is_some_and(|d| d.loose) {
                [0.95, 0.72, 0.1, 1.0]
            } else {
                [0.58, 0.58, 0.62, 1.0]
            }
        }
        4 => def.map_or(metrics::UNKNOWN, |d| metrics::scale(d.strength, 500)),
        5 => def.map_or(metrics::UNKNOWN, |d| metrics::scale(d.density, 1000)),
        6 => {
            let speed = Vec3::from(
                [f.velocity.x, f.velocity.y, f.velocity.z]
                    .map(|v| v.raw() as f32 / fx::ONE_RAW as f32),
            )
            .length();
            metrics::scale((speed * 200.0).min(u32::MAX as f32) as u32, 500)
        }
        10 => {
            let id = u32::from(f.material);
            [
                0.18 + (id * 43 % 70) as f32 / 100.0,
                0.18 + (id * 67 % 70) as f32 / 100.0,
                0.18 + (id * 29 % 70) as f32 / 100.0,
                1.0,
            ]
        }
        _ => metrics::UNKNOWN,
    }
}
pub(crate) fn quantize(c: [f32; 4]) -> [u8; 3] {
    std::array::from_fn(|i| (c[i].clamp(0.0, 1.0) * 31.0).round() as u8)
}
pub(crate) fn material(
    k: [u8; 3],
    cache: &mut BTreeMap<[u8; 3], Handle<StandardMaterial>>,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    cache
        .entry(k)
        .or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::linear_rgb(
                    k[0] as f32 / 31.0,
                    k[1] as f32 / 31.0,
                    k[2] as f32 / 31.0,
                ),
                unlit: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone()
}
#[cfg(test)]
pub(super) mod tests {
    use super::*;
    pub(crate) fn global_app(mode: u8) -> App {
        let mut app = fragments::lifecycle_tests::app();
        let mut suspended = fragments::lifecycle_tests::app();
        let mut lab = Laboratory::new(false);
        lab.saved = Some(Box::new(Suspended {
            scene: suspended.world_mut().remove_resource::<Scene>().unwrap(),
            workshop: Workshop::new(None, false),
            panel: panel::Panel::new(true, false),
            overrides: knobs::Overrides::new(false, true),
            pilot: pilot::Pilot::default(),
            projection: None,
        }));
        lab.tools.metric = mode;
        app.insert_resource(lab);
        app
    }
    #[test]
    fn global_metrics_cover_far_fragments_all_modes_and_restore_without_local_overlays() {
        let mut app = global_app(1);
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .matter
            .debris
            .fragments = (1..=160).map(fragments::lifecycle_tests::piece).collect();
        let before = app.world().resource::<Scene>().sim.state_hash();
        for mode in 1..=10 {
            app.world_mut().resource_mut::<Laboratory>().tools.metric = mode;
            app.update();
            let world = app.world_mut();
            let mut q = world.query::<(
                &fragments::Piece,
                &Mesh3d,
                &MeshMaterial3d<StandardMaterial>,
            )>();
            let rows: Vec<_> = q
                .iter(world)
                .map(|(_, m, c)| (m.0.clone(), c.0.clone()))
                .collect();
            assert_eq!(
                rows.len(),
                160,
                "mode{mode}: fragment farther than local radius or old64 cap vanished"
            );
            for (mesh, material) in rows {
                let assets = world.resource::<Assets<Mesh>>();
                let bevy::mesh::VertexAttributeValues::Float32x4(col) = assets
                    .get(&mesh)
                    .unwrap()
                    .attribute(Mesh::ATTRIBUTE_COLOR)
                    .unwrap()
                else {
                    panic!("missing vertex colours")
                };
                assert!(col.iter().all(|c| *c == [1.0; 4]));
                assert!(
                    world
                        .resource::<Assets<StandardMaterial>>()
                        .get(&material)
                        .unwrap()
                        .unlit
                );
            }
            assert_eq!(app.world().resource::<Scene>().sim.state_hash(), before);
        }
        app.world_mut().resource_mut::<Laboratory>().tools.metric = 6;
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .matter
            .debris
            .fragments
            .last_mut()
            .unwrap()
            .velocity
            .x = fx::Fx::from_int(2);
        app.update();
        let fast = quantize(metrics::scale(400, 500));
        let expected = Color::linear_rgb(
            fast[0] as f32 / 31.0,
            fast[1] as f32 / 31.0,
            fast[2] as f32 / 31.0,
        );
        let world = app.world_mut();
        let mut q = world.query::<&MeshMaterial3d<StandardMaterial>>();
        assert!(q.iter(world).any(|m| world
            .resource::<Assets<StandardMaterial>>()
            .get(&m.0)
            .is_some_and(|m| m.base_color == expected)));
        app.world_mut().resource_mut::<Laboratory>().tools.metric = 0;
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<&MeshMaterial3d<StandardMaterial>>();
        assert_eq!(q.iter(world).count(), 160);
        assert!(q.iter(world).all(|m| !world
            .resource::<Assets<StandardMaterial>>()
            .get(&m.0)
            .unwrap()
            .unlit));
    }
    #[test]
    fn global_metrics_cover_every_resting_batch_and_unbatched_remainder() {
        let mut app = global_app(3);
        app.insert_resource(fragments::batches::Enabled);
        let pieces: Vec<_> = (1..=2048)
            .map(|id| {
                let mut f = fragments::lifecycle_tests::piece(id);
                f.position = sim_core::damage::V3::new(
                    fx::Fx::from_ratio((id % 32) as i32, 2),
                    fx::Fx::ZERO,
                    fx::Fx::from_ratio((id / 32) as i32, 2),
                );
                f
            })
            .collect();
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .matter
            .debris
            .fragments = pieces;
        for mode in [3, 6, 10, 0] {
            app.world_mut().resource_mut::<Laboratory>().tools.metric = mode;
            for _ in 0..8 {
                app.update();
            }
            let world = app.world_mut();
            let mut q = world.query::<(&Mesh3d, &MeshMaterial3d<StandardMaterial>)>();
            let mut cells = 0;
            for (m, material) in q.iter(world) {
                let mesh = world.resource::<Assets<Mesh>>().get(&m.0).unwrap();
                cells += mesh.indices().unwrap().len() / 36;
                assert_eq!(
                    world
                        .resource::<Assets<StandardMaterial>>()
                        .get(&material.0)
                        .unwrap()
                        .unlit,
                    mode > 0
                );
            }
            assert_eq!(cells, 2048, "mode{mode}: batching dropped metric coverage");
        }
    }
    #[test]
    fn section_caps_eventually_cover_all_units_past_the_old_frame_budget() {
        let mut app = global_app(6);
        app.add_systems(Update, section::unit_caps);
        let sim = &mut app.world_mut().resource_mut::<Scene>().sim;
        sim.units = (0..192)
            .map(|_| {
                sim_core::blast::RangeUnit::of_kind(
                    modding::range::RangeUnitKind::Tank,
                    [40, 16, 40],
                )
            })
            .collect();
        let (lo, hi) = sim.units[0].bounds();
        let at = (lo[1] + hi[1]) / 2;
        let units: Vec<_> = sim
            .units
            .iter()
            .chain((!sim.point_gun()).then_some(&sim.gun))
            .collect();
        let expected: usize = units
            .iter()
            .map(|unit| {
                let (lo, hi) = unit.bounds();
                (lo[0]..=hi[0])
                    .flat_map(|x| (lo[2]..=hi[2]).map(move |z| [x, at, z]))
                    .filter(|&p| unit.hit(p).is_some())
                    .count()
            })
            .sum();
        let mut lab = app.world_mut().resource_mut::<Laboratory>();
        lab.tools.section.enabled = true;
        lab.tools.section.axis = 1;
        lab.tools.section.anchor = [40, at, 40];
        drop(lab);
        let hash = app.world().resource::<Scene>().sim.state_hash();
        for _ in 0..32 {
            app.update();
        }
        let world = app.world_mut();
        let mut q = world.query_filtered::<&Mesh3d, With<section::UnitSectionCap>>();
        let count: usize = q
            .iter(world)
            .map(|m| {
                world
                    .resource::<Assets<Mesh>>()
                    .get(&m.0)
                    .unwrap()
                    .count_vertices()
                    / 4
            })
            .sum();
        assert_eq!(count, expected);
        assert!(expected > 32768, "fixture must exceed old cell budget");
        assert_eq!(world.resource::<Scene>().sim.state_hash(), hash);
    }
    #[test]
    fn section_metric_toggle_covers_mesh_budget_waiters_immediately() {
        section_metric_transitions(false);
        section_metric_transitions(true);
    }
    fn section_metric_transitions(section: bool) {
        use sim_core::{burst::chunk::{Chunk, Shape}, damage::{Frame, V3}};
        let mut app = global_app(0);
        app.add_systems(Update, section_debris::draw.after(fragments::draw));
        if section {
            let mut lab = app.world_mut().resource_mut::<Laboratory>();
            lab.tools.section.enabled = true; lab.tools.section.axis = 1;
        }
        let template = voxel::unit::tank();
        let cell = [template.core.0, template.core.1, template.core.2];
        let frame = Frame::new(&template, V3::ZERO, 0, V3::UP);
        let pieces = (1..=48).map(|id| {
            let mut shape = Shape::default(); shape.add(cell, template.turret_y);
            let mut f = fragments::lifecycle_tests::piece(id);
            f.position = V3::ZERO;
            f.paint = Some([1, 0, 0]);
            f.chunk = Some(Box::new(Chunk::new(0, 1, vec![cell], [frame; 2], shape)));
            f
        }).collect();
        {
            let sim = &mut app.world_mut().resource_mut::<Scene>().sim;
            sim.units = vec![sim_core::blast::RangeUnit::of_kind(modding::range::RangeUnitKind::Tank, [0,16,0])];
            sim.matter.debris.fragments = pieces;
        }
        for _ in 0..4 { app.update(); }
        let hash = app.world().resource::<Scene>().sim.state_hash();
        let snapshot = |app: &mut App| {
            let world = app.world_mut();
            let mut q = world.query_filtered::<(Entity, &Mesh3d, &MeshMaterial3d<StandardMaterial>), With<fragments::Surface>>();
            q.iter(world).map(|(e, h, m)| {
                let mesh = world.resource::<Assets<Mesh>>().get(&h.0).unwrap();
                let bevy::mesh::VertexAttributeValues::Float32x4(col) = mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() else { panic!() };
                let pos = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap().to_vec();
                (e, (m.0.clone(), col.clone(), pos, mesh.indices().unwrap().iter().collect::<Vec<_>>()))
            }).collect::<BTreeMap<_,_>>()
        };
        let original = snapshot(&mut app);
        app.world_mut().resource_mut::<Laboratory>().tools.metric = 1;
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(&Mesh3d, &MeshMaterial3d<StandardMaterial>), With<fragments::Surface>>();
        let mut count = 0;
        for (mesh, material) in q.iter(world) {
            count += 1;
            assert!(world.resource::<Assets<StandardMaterial>>().get(&material.0).is_some_and(|m| m.unlit), "waiting section kept its ordinary material");
            let bevy::mesh::VertexAttributeValues::Float32x4(col) = world.resource::<Assets<Mesh>>().get(&mesh.0).unwrap().attribute(Mesh::ATTRIBUTE_COLOR).unwrap() else { panic!() };
            assert!(col.iter().all(|c| *c == [1.0; 4]), "waiting section kept paint under the metric material");
        }
        assert_eq!(count, 48);
        assert_eq!(world.resource::<Scene>().sim.state_hash(), hash);
        for mode in [6, 10, 0, 1, 0] {
            app.world_mut().resource_mut::<Laboratory>().tools.metric = mode;
            app.update();
            let shown = snapshot(&mut app);
            assert_eq!(shown.len(), original.len());
            for (entity, (material, colours, pos, indices)) in &shown {
                let prior = &original[entity];
                assert_eq!((pos, indices), (&prior.2, &prior.3), "mode{mode}: presentation rebuilt physical geometry");
                if mode == 0 { assert_eq!((material, colours), (&prior.0, &prior.1), "paint was not restored"); }
                else { assert!(colours.iter().all(|c| *c == [1.0;4])); }
            }
            assert_eq!(app.world().resource::<Scene>().sim.state_hash(), hash);
        }
        // Sections born while a map is active must also retain real paint.
        app.world_mut().resource_mut::<Laboratory>().tools.metric = 1;
        app.world_mut().resource_mut::<Scene>().generation += 1;
        { let world = app.world_mut(); let mut q=world.query_filtered::<Entity,With<fragments::Piece>>(); let ids=q.iter(world).collect::<Vec<_>>(); for e in ids { world.entity_mut(e).despawn(); } }
        for _ in 0..4 {app.update();}
        app.world_mut().resource_mut::<Laboratory>().tools.metric = 0;
        app.update();
        let restored = snapshot(&mut app);
        let expected = original.values().next().unwrap();
        assert_eq!(restored.len(),48);
        for (material,colours,pos,indices) in restored.values() {
            assert_eq!((material,colours,pos,indices),(&expected.0,&expected.1,&expected.2,&expected.3));
        }
    }
    #[test]
    fn colour_quantization_preserves_order_and_bounds() {
        assert_eq!(quantize([0.0, 0.5, 1.0, 1.0]), [0, 16, 31]);
        assert_eq!(quantize([-1.0, 2.0, 0.0, 1.0]), [0, 31, 0]);
    }
    #[test]
    fn steel_fragment_uses_its_own_material_and_speed_without_fake_heat() {
        use fx::Fx;
        use sim_core::damage::V3;
        let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../../../../data/explosion.ron"
        ))
        .unwrap();
        let mut layout = modding::range::RangeLayout::from_ron(include_str!(
            "../../../../../../data/range-cover.ron"
        ))
        .unwrap();
        layout.blocks.clear();
        layout.units.clear();
        layout.shields.clear();
        layout.course = None;
        let sim = BlastRange::with_layout(cfg, layout).unwrap();
        let f = Fragment {
            id: 7,
            position: V3::ZERO,
            velocity: V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
            material: 3,
            paint: None,
            mm: 100,
            basis: [
                V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
                V3::new(Fx::ZERO, Fx::ONE, Fx::ZERO),
                V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
            ],
            mass_g: 1,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: 0,
            resting: false,
        };
        let before = sim.state_hash();
        assert_eq!(colour(&f, &sim, 1), metrics::UNKNOWN);
        assert_eq!(colour(&f, &sim, 4), metrics::scale(500, 500));
        assert_eq!(colour(&f, &sim, 6), metrics::scale(200, 500));
        assert_eq!(sim.state_hash(), before);
    }
}
