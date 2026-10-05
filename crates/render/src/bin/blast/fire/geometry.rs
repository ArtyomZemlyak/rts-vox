//! Independent translucent layers, prepared from one immutable render frame.
use super::*;
use std::collections::BTreeMap;

pub(super) type Poses = BTreeMap<(usize, Option<usize>, bool), Transform>;
#[derive(Clone, Copy)]
pub(super) struct Input<'a> {
    pub sim: &'a BlastRange,
    pub effects: &'a Effects,
    pub particles: &'a [(&'a Particle, Vec3, f32)],
    pub now: f32,
    pub fraction: f32,
    pub poses: &'a Poses,
    pub markers: Option<projectiles::View>,
    pub single: (Option<Vec3>, f32),
}
pub(super) struct Prepared {
    pub hot: Option<Mesh>,
    pub smoke: Option<Mesh>,
}
fn ready(buffer: MeshBuf) -> Option<Mesh> {
    (!buffer.is_empty()).then(|| fire_mesh(buffer))
}
// Preserve the former compact publication on small/serial workloads. The
// ownership-only microbenchmark does not establish their native frame cost.
fn compact(buffer: MeshBuf) -> Option<Mesh> {
    if buffer.is_empty() {
        return None;
    }
    let mut mesh = mesh(&buffer);
    mesh.asset_usage = RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD;
    Some(mesh)
}
pub(super) fn prepare(input: Input<'_>, pool: Option<&bevy::tasks::TaskPool>) -> Prepared {
    let smoke_count = input.particles.iter().filter(|(p, _, _)| p.smoke).count();
    let hot_count = input.particles.len() - smoke_count
        + input.effects.tracers.len() * 2
        + input.sim.battle.lasers.len() * 4
        + input.sim.battle.rounds.len()
        + input.effects.heat_spots.len();
    if let Some(pool) = pool.filter(|p| {
        p.thread_num() >= 2
            && smoke_count >= 32
            && hot_count >= 32
            && smoke_count + hot_count >= 256
    }) {
        // Spend the caller's time on the larger layer before joining the one
        // worker job. Keep normal scope stealing for safe nested callers.
        let caller_hot = hot_count >= smoke_count;
        let (mut hot_mesh, mut smoke_mesh) = (None, None);
        let mut results = pool.scope(|scope| {
            if caller_hot {
                scope.spawn(async move { ready(smoke(input)) });
                hot_mesh = ready(hot(input));
            } else {
                scope.spawn(async move { ready(hot(input)) });
                smoke_mesh = ready(smoke(input));
            }
        });
        let other = results.pop().unwrap();
        if caller_hot {
            smoke_mesh = other;
        } else {
            hot_mesh = other;
        }
        Prepared {
            hot: hot_mesh,
            smoke: smoke_mesh,
        }
    } else {
        Prepared {
            hot: compact(hot(input)),
            smoke: compact(smoke(input)),
        }
    }
}
fn hot(input: Input<'_>) -> MeshBuf {
    let Input {
        sim,
        effects: fx,
        particles,
        now,
        fraction,
        poses,
        markers,
        single,
    } = input;
    let mut hot = MeshBuf::default();
    for trace in &sim.battle.lasers {
        let from = point(trace.from);
        let to = point(trace.to);
        beam(&mut hot, from, to, 10.0, [1.8, 0.025, 0.018, 0.09]);
        beam(&mut hot, from, to, 4.8, [5.0, 0.12, 0.04, 0.30]);
        beam(&mut hot, from, to, 1.9, [12.0, 2.8, 0.8, 0.95]);
        if trace.hit {
            cube(&mut hot, to, 6.0, [9.0, 1.8, 0.3, 0.9]);
        }
    }
    if sim.config.delivery == Delivery::Laser && !sim.battle_mode() {
        for trace in &sim.streams.traces {
            let from = point(trace.from);
            let to = point(trace.to);
            let size = if sim.gun.kind == RangeUnitKind::Spider {
                3.0
            } else {
                1.0
            };
            let heat_width = sim.streams.tuning.laser.heat_radius as f32 * 2.0;
            if heat_width > 0.0 {
                beam(&mut hot, from, to, heat_width, [1.6, 0.035, 0.012, 0.055]);
            }
            beam(&mut hot, from, to, 2.4 * size, [2.8, 0.06, 0.025, 0.18]);
            beam(&mut hot, from, to, 0.65 * size, [7.0, 0.36, 0.25, 0.92]);
            if trace.hit {
                cube(&mut hot, to, 2.5 * size, [5.0, 0.65, 0.18, 0.75]);
            }
        }
    }
    if sim.config.delivery == Delivery::MachineGun || sim.battle_mode() {
        for tracer in &fx.tracers {
            let fade = (1.0 - (now - tracer.born) / TRACER_LIFE).clamp(0.0, 1.0);
            // The short streak remains visible across render frames. Its head
            // never passes the last physical bullet position.
            beam(
                &mut hot,
                tracer.tail,
                tracer.head,
                1.1,
                [3.8, 2.0, 0.28, 0.58 * fade],
            );
            beam(
                &mut hot,
                tracer.tail.lerp(tracer.head, 0.18),
                tracer.head,
                0.48,
                [7.0, 4.3, 0.85, 0.95 * fade],
            );
        }
    }
    if sim.battle_mode() {
        let point = |p: [fx::Fx; 3]| Vec3::from(p.map(|v| v.to_f64_lossy() as f32));
        for r in sim.battle.rounds.iter().filter(|r| {
            matches!(
                r.weapon,
                BattleWeapon::Blast | BattleWeapon::Kinetic | BattleWeapon::Canister
            )
        }) {
            let t = fraction;
            let at = point(r.previous).lerp(point(r.pos), t);
            if r.weapon == BattleWeapon::Canister {
                let direction = (point(r.pos) - point(r.previous)).normalize_or_zero();
                beam(
                    &mut hot,
                    at - direction * 4.5,
                    at,
                    0.55,
                    [5.0, 2.7, 0.38, 0.85],
                );
            }
            cube(
                &mut hot,
                at,
                if r.weapon == BattleWeapon::Canister {
                    1.2
                } else {
                    2.0
                },
                [6.0, 3.2, 0.55, 1.0],
            );
        }
    }
    for &(p, at, t) in particles.iter().filter(|(p, _, _)| !p.smoke) {
        if let Some(kind) = p.contact {
            let col = contact_colour(kind, t);
            match kind {
                ContactKind::Spark => {
                    let age = (now - p.born).max(0.0);
                    let velocity = p.velocity - Vec3::Y * contact_gravity(sim) * age;
                    let tail = at
                        - velocity.normalize_or_zero()
                            * (velocity.length() * 0.065).clamp(2.0, 6.5);
                    beam(&mut hot, tail, at, p.edge * (1.0 - t * 0.45), col);
                }
                ContactKind::Chip => {
                    let start = hot.pos.len();
                    cube(&mut hot, at, p.edge, col);
                    let age = (now - p.born).max(0.0);
                    let rotation = Quat::from_rotation_y(age * (6.0 + noise(p.seed) * 13.0))
                        * Quat::from_rotation_x(age * (5.0 + noise(p.seed.wrapping_add(9)) * 11.0));
                    for p in &mut hot.pos[start..] {
                        *p = (at + rotation * (Vec3::from(*p) - at)).to_array();
                    }
                    for n in &mut hot.nrm[start..] {
                        *n = (rotation * Vec3::from(*n)).to_array();
                    }
                }
                ContactKind::Flash => cube(&mut hot, at, p.edge * (1.0 - t * 0.6), col),
                ContactKind::Dust => {}
            }
            continue;
        }
        let col = flame_colour(p, t, now);
        let edge = p.edge * (0.9 + (t * 3.0).sin() * 0.1) * (1.0 - t * 0.55);
        if p.stream {
            // A flame packet travels as a ragged tongue. Extend its
            // cuboid behind the current position so the ribbon joins up
            // without painting beyond the latest contact point.
            let axis = p.velocity.normalize_or_zero();
            beam(
                &mut hot,
                at - axis * edge * (2.1 - t * 0.7),
                at,
                edge * 0.72,
                col,
            );
        } else if p.burst_flame {
            // Impact fragments have a direction. A short, uneven cuboid
            // makes each hot tongue read as an outward jet rather than a
            // ring of equally shaped fire blocks.
            let axis = p.velocity.normalize_or_zero();
            let stretch = 0.85 + noise(p.seed.wrapping_add(43)) * 1.45;
            beam(
                &mut hot,
                at - axis * edge * stretch,
                at + axis * edge * 0.2,
                edge * (0.38 + noise(p.seed.wrapping_add(51)) * 0.27),
                col,
            );
        } else {
            cube(&mut hot, at, edge, col);
        }
    }
    for &(cell, heat) in &fx.heat_spots {
        let id = usize::from(cell.unit);
        let seg = cell.part.checked_sub(1).map(usize::from);
        let turret = seg.map_or(cell.at[1] >= sim.units[id].body.turret_y, |i| {
            sim.units[id].body.segments[i].attach.turret
        });
        let Some(tr) = poses.get(&(id, seg, turret)) else {
            continue;
        };
        let Some(body) = cell.body(&sim.units) else {
            continue;
        };
        let level = (heat as f32 / sim.thermal_tuning.glow as f32 - 1.0).clamp(0.0, 3.0);
        let start = hot.pos.len();
        let unit = &sim.units[id];
        let pivot = if let Some(seg) = seg {
            render::model::pivot_centre(&unit.body.segments[seg].attach)
        } else {
            render::model::root_centre(body)
        };
        let local = (Vec3::from(cell.at.map(|v| v as f32)) + Vec3::splat(0.5) - Vec3::from(pivot))
            * (body.mm as f32 / voxel::unit::WORLD_MM as f32);
        let centre = tr.translation + tr.rotation * local;
        cube(
            &mut hot,
            centre,
            body.mm as f32 / voxel::unit::WORLD_MM as f32 * 1.06,
            [0.8 + level * 1.4, 0.025 + level * 0.12, 0.006, 0.65],
        );
        let rotation = tr.rotation;
        for p in &mut hot.pos[start..] {
            *p = (centre + rotation * (Vec3::from(*p) - centre)).to_array();
        }
        for n in &mut hot.nrm[start..] {
            *n = (rotation * Vec3::from(*n)).to_array();
        }
    }
    if let Some(view) = markers {
        projectiles::append(&mut hot, sim, view, fraction, single);
    }
    hot
}
fn smoke(input: Input<'_>) -> MeshBuf {
    let mut smoke = MeshBuf::default();
    for &(p, at, t) in input.particles.iter().filter(|(p, _, _)| p.smoke) {
        if p.contact == Some(ContactKind::Dust) {
            let fade = (t * 12.0).min(1.0) * (1.0 - t).powi(2);
            let shade = 0.43 + noise(p.seed) * 0.08;
            cube(
                &mut smoke,
                at,
                p.edge * (0.75 + t * 0.65),
                [shade, shade * 0.96, shade * 0.90, fade * 0.32],
            );
        } else if p.dust_material != 0 {
            let rgb = match p.dust_material {
                1 => [0.38, 0.31, 0.23],
                7 => [0.36, 0.34, 0.25],
                8 => [0.39, 0.36, 0.31],
                6 => [0.34, 0.34, 0.32],
                _ => [0.38, 0.36, 0.33],
            };
            let fade = (t * 9.0).min(1.0) * (1.0 - t).powi(2);
            cube(
                &mut smoke,
                at,
                p.edge * (0.75 + t * 0.9),
                [rgb[0], rgb[1], rgb[2], fade * 0.34],
            );
        } else if p.burst_dust {
            let fade = (t * 10.0).min(1.0) * (1.0 - t).powi(2);
            let shade = 0.26 + noise(p.seed) * 0.09;
            cube(
                &mut smoke,
                at,
                p.edge * (0.7 + t * 0.8),
                [shade, shade * 0.88, shade * 0.72, fade * 0.18],
            );
        } else {
            let fade = (t * 8.0).min(1.0) * (1.0 - t).powi(2);
            let shade = 0.14 + t * 0.11 + noise(p.seed) * 0.05;
            cube(
                &mut smoke,
                at,
                p.edge * (0.65 + t * 1.35),
                [
                    shade * 0.98,
                    shade * 0.96,
                    shade,
                    fade * (0.14 + 0.07 * (1.0 - t)),
                ],
            );
        }
    }
    smoke
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(count: usize) -> (BlastRange, Effects, Poses) {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../../data/range-cover.ron")).unwrap();
        layout.battle = true;
        let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
        sim.units = [
            RangeUnitKind::Tank,
            RangeUnitKind::Strider,
            RangeUnitKind::Spider,
            RangeUnitKind::Artillery,
            RangeUnitKind::TurretT1,
            RangeUnitKind::TurretT2,
            RangeUnitKind::Base,
            RangeUnitKind::ArtilleryT2,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| sim_core::blast::RangeUnit::of_kind(kind, [160 + i as i32 * 35, 16, 290]))
        .collect();
        sim.battle.lasers.push(sim_core::blast::weapons::Trace {
            from: [fx::Fx::from_int(20); 3],
            to: [fx::Fx::from_int(160); 3],
            hit: true,
            serial: 0,
        });
        let mut effects = Effects::default();
        for i in 0..count {
            effects.add(
                Vec3::new(i as f32 * 0.3, 10.0, 7.0),
                Vec3::new(8.0, 5.0, 3.0),
                0.0,
                2.0,
                2.0 + (i % 7) as f32,
                i % 3 != 0,
            );
            let p = effects.particles.last_mut().unwrap();
            p.burst_dust = i % 5 == 0;
            p.burst_flame = i % 7 == 0;
            p.stream = i % 11 == 0;
            p.dust_material = if i % 13 == 0 { 1 } else { 0 };
            p.contact = match i % 17 {
                0 => Some(ContactKind::Flash),
                1 => Some(ContactKind::Spark),
                2 => Some(ContactKind::Chip),
                3 => Some(ContactKind::Dust),
                _ => None,
            };
            if let Some(kind) = p.contact {
                p.smoke = kind == ContactKind::Dust;
            }
        }
        for i in 0..32 {
            effects.tracers.push(Tracer {
                tail: Vec3::new(i as f32, 20.0, 30.0),
                head: Vec3::new(i as f32 + 8.0, 22.0, 33.0),
                born: 0.35,
            });
        }
        let mut poses = Poses::new();
        for (id, unit) in sim.units.iter().enumerate() {
            for (part, body) in std::iter::once((None, &unit.body)).chain(
                unit.body
                    .segments
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (Some(i), &s.body)),
            ) {
                for turret in [false, true] {
                    poses.insert(
                        (id, part, turret),
                        Transform::from_translation(Vec3::new(id as f32 * 35.0, 20.0, 60.0))
                            .with_rotation(Quat::from_rotation_y(0.7)),
                    );
                }
                effects.heat_spots.push((
                    HeatCell {
                        unit: id as u16,
                        part: part.map_or(0, |i| i as u8 + 1),
                        at: [body.vol.nx / 2, body.vol.ny / 2, body.vol.nz / 2],
                    },
                    sim.thermal_tuning.glow * 2,
                ));
            }
        }
        (sim, effects, poses)
    }
    fn particles(effects: &Effects) -> Vec<(&Particle, Vec3, f32)> {
        // A fixed camera order, including ties; both layers must preserve it.
        effects
            .particles
            .iter()
            .rev()
            .map(|p| (p, p.at, 0.2))
            .collect()
    }
    fn legacy(buffer: MeshBuf) -> Option<Mesh> {
        if buffer.is_empty() {
            return None;
        }
        let mut result = mesh(&buffer);
        result.asset_usage = RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD;
        Some(result)
    }
    fn same(a: &Option<Mesh>, b: &Option<Mesh>) {
        assert_eq!(a.is_some(), b.is_some());
        if let (Some(a), Some(b)) = (a, b) {
            for attr in [
                Mesh::ATTRIBUTE_POSITION,
                Mesh::ATTRIBUTE_NORMAL,
                Mesh::ATTRIBUTE_COLOR,
                Mesh::ATTRIBUTE_UV_0,
                Mesh::ATTRIBUTE_TANGENT,
            ] {
                assert_eq!(a.attribute(attr), b.attribute(attr));
            }
            assert_eq!(
                a.indices().map(|i| i.iter().collect::<Vec<_>>()),
                b.indices().map(|i| i.iter().collect::<Vec<_>>())
            );
            assert_eq!(a.asset_usage, b.asset_usage);
        }
    }
    #[test]
    fn layers_preserve_cloned_mesh_attributes_across_workers_and_render_modes() {
        let pools: Vec<_> = [1, 2, 4, 8]
            .into_iter()
            .map(|workers| {
                bevy::tasks::TaskPoolBuilder::new()
                    .num_threads(workers)
                    .build()
            })
            .collect();
        let (mut sim, effects, poses) = fixture(CAP);
        let all = particles(&effects);
        for battle in [false, true] {
            sim.layout.as_mut().unwrap().battle = battle;
            for delivery in [Delivery::Laser, Delivery::MachineGun, Delivery::Kinetic] {
                sim.config.delivery = delivery;
                for count in [0, 1, 31, 32, 255, 256, CAP] {
                    let input = Input {
                        sim: &sim,
                        effects: &effects,
                        particles: &all[..count],
                        now: 0.4,
                        fraction: 0.3,
                        poses: &poses,
                        markers: None,
                        single: (None, 2.0),
                    };
                    let reference = Prepared {
                        hot: legacy(hot(input)),
                        smoke: legacy(smoke(input)),
                    };
                    for pool in std::iter::once(None).chain(pools.iter().map(Some)) {
                        let result = prepare(input, pool);
                        same(&reference.hot, &result.hot);
                        same(&reference.smoke, &result.smoke);
                    }
                }
            }
        }
    }
    #[test]
    fn empty_layers_stay_absent_and_nested_jobs_finish_on_small_pools() {
        let (mut sim, effects, poses) = fixture(CAP);
        let all = particles(&effects);
        for workers in [1, 2, 4] {
            let pool = bevy::tasks::TaskPoolBuilder::new()
                .num_threads(workers)
                .build();
            let input = Input {
                sim: &sim,
                effects: &effects,
                particles: &all,
                now: 0.4,
                fraction: 1.0,
                poses: &poses,
                markers: None,
                single: (None, 2.0),
            };
            let expected = prepare(input, None);
            let mut results = pool.scope(|scope| {
                let pool = &pool;
                scope.spawn(async move { prepare(input, Some(pool)) });
            });
            let result = results.pop().unwrap();
            same(&expected.hot, &result.hot);
            same(&expected.smoke, &result.smoke);
        }
        sim.config.delivery = Delivery::Kinetic;
        sim.layout.as_mut().unwrap().battle = false;
        sim.battle.lasers.clear();
        let empty = Effects::default();
        let result = prepare(
            Input {
                sim: &sim,
                effects: &empty,
                particles: &[],
                now: 0.0,
                fraction: 0.0,
                poses: &poses,
                markers: None,
                single: (None, 2.0),
            },
            None,
        );
        assert!(result.hot.is_none() && result.smoke.is_none());
    }
    #[test]
    fn owned_mesh_moves_buffers_and_preserves_optional_surface_attributes() {
        let mut buffer = MeshBuf::default();
        cube(&mut buffer, Vec3::ZERO, 4.0, [1.0; 4]);
        buffer.uv.resize(buffer.pos.len(), [0.2, 0.3]);
        buffer.tan.resize(buffer.pos.len(), [1.0, 0.0, 0.0, 1.0]);
        let expected = mesh(&buffer);
        let position = buffer.pos.as_ptr();
        let owned = fire_mesh(buffer);
        let bevy::mesh::VertexAttributeValues::Float32x3(values) =
            owned.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
        else {
            panic!("position format");
        };
        assert_eq!(position, values.as_ptr(), "vertices were copied");
        for attr in [
            Mesh::ATTRIBUTE_POSITION,
            Mesh::ATTRIBUTE_NORMAL,
            Mesh::ATTRIBUTE_COLOR,
            Mesh::ATTRIBUTE_UV_0,
            Mesh::ATTRIBUTE_TANGENT,
        ] {
            assert_eq!(expected.attribute(attr), owned.attribute(attr));
        }
    }
    #[inline(never)]
    fn cloned_reference(input: Input<'_>) -> Prepared {
        Prepared {
            hot: legacy(hot(input)),
            smoke: legacy(smoke(input)),
        }
    }
    #[inline(never)]
    fn owned_reference(input: Input<'_>) -> Prepared {
        Prepared {
            hot: ready(hot(input)),
            smoke: ready(smoke(input)),
        }
    }
    #[test]
    #[ignore = "manual geometry preparation timing without competing workloads"]
    fn bench_frame_geometry() {
        let (sim, effects, poses) = fixture(CAP);
        let particles = particles(&effects);
        let input = Input {
            sim: &sim,
            effects: &effects,
            particles: &particles,
            now: 0.4,
            fraction: 0.3,
            poses: &poses,
            markers: None,
            single: (None, 2.0),
        };
        let workers = [0, 1, 2, 4, 8];
        let pools: Vec<_> = workers
            .iter()
            .map(|&workers| {
                (workers > 0).then(|| {
                    bevy::tasks::TaskPoolBuilder::new()
                        .num_threads(workers)
                        .build()
                })
            })
            .collect();
        let mut times = vec![Vec::new(); 7];
        // Rotate cases and warm every allocator/pool path first. Dynamic
        // inputs prevent the reference from specializing a constant frame.
        for round in 0..220 {
            for offset in 0..7 {
                let case = (round + offset) % 7;
                let start = std::time::Instant::now();
                let input = std::hint::black_box(input);
                let result = match case {
                    0..=4 => prepare(input, pools[case].as_ref()),
                    5 => cloned_reference(input),
                    _ => owned_reference(input),
                };
                std::hint::black_box(&result);
                let elapsed = start.elapsed().as_nanos();
                if round >= 20 {
                    times[case].push(elapsed);
                }
            }
        }
        for (case, times) in times.iter_mut().enumerate() {
            times.sort_unstable();
            let name = match case {
                0..=4 => format!("workers={}", workers[case]),
                5 => "cloned_reference".into(),
                _ => "owned_reference".into(),
            };
            println!(
                "FIRE {name} mean_ns={} p99_ns={}",
                times.iter().sum::<u128>() / times.len() as u128,
                times[198]
            );
        }
    }
}
