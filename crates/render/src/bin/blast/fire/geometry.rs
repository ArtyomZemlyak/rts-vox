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
    pub first_person_origin: Option<Vec3>,
}
pub(super) struct Prepared {
    pub hot: Option<Mesh>,
    pub smoke: Option<Mesh>,
}
fn ready(buffer: MeshBuf) -> Option<Mesh> {
    (!buffer.is_empty()).then(|| fire_mesh(buffer))
}
// Keep compact publication for small layers. Large serial smoke/flash meshes
// move their arrays, just like worker-built layers, avoiding a second copy.
fn compact(buffer: MeshBuf) -> Option<Mesh> {
    if buffer.is_empty() {
        return None;
    }
    if buffer.pos.len() >= 4096 {
        return ready(buffer);
    }
    let mut mesh = mesh(&buffer);
    mesh.asset_usage = RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD;
    Some(mesh)
}
pub(super) fn prepare(input: Input<'_>, pool: Option<&bevy::tasks::TaskPool>) -> Prepared {
    let particle_smoke = input.particles.iter().filter(|(p, _, _)| p.smoke).count();
    let mut smoke_count = particle_smoke;
    let mut hot_count = input.particles.len() - particle_smoke
        + input.effects.tracers.len() * 2
        + input.sim.battle.lasers.len() * 4
        + input.sim.battle.rounds.len()
        + input.effects.heat_spots.len();
    // A strategic cloud costs far more than a single particle. Include its
    // active layers in the scheduling estimate, without changing geometry.
    for &(_, born) in &input.effects.nuclear {
        let age = (input.now - born).max(0.0);
        if age < nuclear::LIFE {
            smoke_count += 512;
            if age < 11.0 {
                hot_count += 128;
            }
        }
    }
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
                scope.spawn(async move { ready(smoke_with_pool(input, Some(pool))) });
                hot_mesh = ready(hot(input));
            } else {
                scope.spawn(async move { ready(hot(input)) });
                smoke_mesh = ready(smoke_with_pool(input, Some(pool)));
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
            smoke: compact(smoke_with_pool(input, pool)),
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
        first_person_origin,
    } = input;
    let mut hot = MeshBuf::default();
    nuclear::append(&mut hot, fx, now, false);
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
            // Retain the authoritative hit; only attach the visible near end
            // to the current camera-local nozzle between fixed ticks.
            let from = first_person_origin.unwrap_or_else(||point(trace.from));
            let to = point(trace.to);
            let size = if first_person_origin.is_some() {0.35} else if sim.gun.kind == RangeUnitKind::Spider {
                3.0
            } else {
                1.0
            };
            let heat_width = (sim.streams.tuning.laser.heat_radius as f32 * 2.0).min(if first_person_origin.is_some(){1.2}else{f32::INFINITY});
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
                BattleWeapon::Blast
                    | BattleWeapon::Kinetic
                    | BattleWeapon::Canister
                    | BattleWeapon::Siege
                    | BattleWeapon::Nuclear
            )
        }) {
            let t = fraction;
            let at = point(r.previous).lerp(point(r.pos), t);
            if r.weapon == BattleWeapon::Nuclear {
                let axis = (point(r.pos) - point(r.previous)).normalize_or(Vec3::Y);
                beam(&mut hot, at - axis * 18.0, at, 4.5, [1.4, 1.4, 1.3, 1.0]);
                cube(&mut hot, at, 5.0, [3.0, 0.4, 0.08, 1.0]);
                beam(
                    &mut hot,
                    at - axis * 32.0,
                    at - axis * 18.0,
                    3.0,
                    [7.0, 2.4, 0.25, 0.85],
                );
                continue;
            }
            if matches!(r.weapon, BattleWeapon::Canister | BattleWeapon::Siege) {
                let direction = (point(r.pos) - point(r.previous)).normalize_or_zero();
                beam(
                    &mut hot,
                    at - direction * 7.0,
                    at,
                    0.55,
                    [5.0, 2.7, 0.38, 0.85],
                );
            }
            cube(
                &mut hot,
                at,
                if matches!(r.weapon, BattleWeapon::Canister | BattleWeapon::Siege) {
                    if r.weapon == BattleWeapon::Siege {
                        3.2
                    } else {
                        2.1
                    }
                } else {
                    2.0
                },
                [6.0, 3.2, 0.55, 1.0],
            );
        }
    }
    for &(p, at, t) in particles.iter().filter(|(p, _, _)| !p.smoke) {
        if p.burst_flash {
            blast_flash(&mut hot, at, p.edge, t);
            continue;
        }
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

fn blast_flash(buf: &mut MeshBuf, at: Vec3, edge: f32, t: f32) {
    let fade = 1.0 - smooth01(t);
    let spread = 0.75 + smooth01(t / 0.25) * 1.5;
    let core = Vec3::new(16.0, 10.0, 3.0).lerp(Vec3::new(4.0, 0.65, 0.025), smooth01(t));
    block(
        buf,
        at,
        Vec3::splat(edge * (1.0 - t * 0.35)),
        core.extend(fade * 0.96).to_array(),
    );
    // Two short, low cubic lobes open out from the white centre. Their
    // lifetime belongs to the original flash particle; no extra emitters.
    for size in [
        Vec3::new(spread, 0.23, 0.46),
        Vec3::new(0.38, 0.28, spread * 0.87),
    ] {
        block(buf, at, size * edge, [6.0, 1.8, 0.08, fade * 0.42]);
    }
}
#[cfg(test)]
fn smoke(input: Input<'_>) -> MeshBuf {
    smoke_with_pool(input, None)
}
fn smoke_with_pool(input: Input<'_>, pool: Option<&bevy::tasks::TaskPool>) -> MeshBuf {
    let mut smoke = MeshBuf::default();
    nuclear::append_smoke(&mut smoke, input.effects, input.now, pool);
    for &(p, at, t) in input.particles.iter().filter(|(p, _, _)| p.smoke) {
        if p.steam {
            let fade=(t*10.0).min(1.0)*(1.0-t).powi(2);
            puff(&mut smoke,at,p.edge*(0.75+t*0.9),[0.74,0.79,0.82,fade*0.26],p,t);
        } else if p.contact == Some(ContactKind::Dust) {
            let fade = (t * 12.0).min(1.0) * (1.0 - t).powi(2);
            let shade = 0.43 + noise(p.seed) * 0.08;
            puff(
                &mut smoke,
                at,
                p.edge * (0.75 + t * 0.65),
                [shade, shade * 0.96, shade * 0.90, fade * 0.32],
                p,
                t,
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
            puff(
                &mut smoke,
                at,
                p.edge * (0.75 + t * 0.9),
                [rgb[0], rgb[1], rgb[2], fade * 0.34],
                p,
                t,
            );
        } else if p.burst_dust {
            let fade = (t * 10.0).min(1.0) * (1.0 - t).powi(2);
            let shade = 0.26 + noise(p.seed) * 0.09;
            puff(
                &mut smoke,
                at,
                p.edge * (0.7 + t * 0.8),
                [shade, shade * 0.88, shade * 0.72, fade * 0.18],
                p,
                t,
            );
        } else {
            let fade = (t * 8.0).min(1.0) * (1.0 - t).powi(2);
            let shade = 0.14 + t * 0.11 + noise(p.seed) * 0.05;
            let warm = 1.0 - smooth01(t * 2.0);
            puff(
                &mut smoke,
                at,
                p.edge * (0.65 + t * 1.35),
                [
                    shade * (0.98 + warm * 0.10),
                    shade * 0.96,
                    shade * (1.0 - warm * 0.22),
                    fade * (0.14 + 0.07 * (1.0 - t)),
                ],
                p,
                t,
            );
        }
    }
    smoke
}

fn puff(buf: &mut MeshBuf, at: Vec3, edge: f32, col: [f32; 4], p: &Particle, t: f32) {
    let dust = p.burst_dust || p.dust_material != 0 || p.contact == Some(ContactKind::Dust);
    // Stable proportions spread as smoke ages. Dust stays low; rising soot
    // stretches vertically. Six cubic faces, no extra particles or sorting.
    let spread = smooth01(t);
    block(
        buf,
        at,
        Vec3::new(
            0.80 + noise(p.seed.wrapping_add(119)) * 0.35 + spread * 0.15,
            if dust {
                0.48 + spread * 0.28
            } else {
                1.18 - spread * 0.22
            },
            0.80 + noise(p.seed.wrapping_add(157)) * 0.35 + spread * 0.15,
        ) * edge,
        col,
    );
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
            p.burst_flash = i % 19 == 0 && !p.smoke && p.contact.is_none();
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
                        first_person_origin: None,
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
                first_person_origin: None,
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
                first_person_origin: None,
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
    fn nuclear_fixture(count: usize) -> (BlastRange, Effects, Poses) {
        let (mut sim, mut effects, poses) = fixture(0);
        sim.layout.as_mut().unwrap().battle = false;
        sim.config.delivery = Delivery::Kinetic;
        sim.battle.lasers.clear();
        effects.tracers.clear();
        effects.heat_spots.clear();
        for i in 0..count {
            let at = Vec3::new(i as f32 * 1200.0, 16.0, -30.0);
            effects.nuclear_cloud(at, 0.0, 2048.0);
            for sector in 0..8 {
                let a = sector as f32 * std::f32::consts::TAU / 8.0;
                effects
                    .nuclear_fronts
                    .last_mut()
                    .unwrap()
                    .observe(at + Vec3::new(a.cos(), 0.0, a.sin()) * 800.0, 0.5);
            }
        }
        (sim, effects, poses)
    }
    #[test]
    fn strategic_layers_match_serial_on_small_pools_through_cooling_and_expiry() {
        let pools: Vec<_> = [1, 2, 4]
            .into_iter()
            .map(|n| bevy::tasks::TaskPoolBuilder::new().num_threads(n).build())
            .collect();
        for count in [1, nuclear::CAP] {
            let (sim, effects, poses) = nuclear_fixture(count);
            for now in [0.0, 0.5, 0.75, 3.0, 6.0, 12.0, nuclear::LIFE] {
                let input = Input {
                    sim: &sim,
                    effects: &effects,
                    particles: &[],
                    now,
                    fraction: 1.0,
                    poses: &poses,
                    markers: None,
                    first_person_origin: None,
                    single: (None, 2.0),
                };
                let expected = prepare(input, None);
                for pool in &pools {
                    let result = prepare(input, Some(pool));
                    same(&expected.hot, &result.hot);
                    same(&expected.smoke, &result.smoke);
                }
                if now >= 12.0 {
                    assert!(expected.hot.is_none());
                }
                if now >= nuclear::LIFE {
                    assert!(expected.smoke.is_none());
                }
            }
        }
    }
    #[test]
    #[ignore = "manual strategic-layer timing without competing workloads"]
    fn bench_nuclear_layers() {
        let pools: Vec<_> = [1, 2, 4]
            .into_iter()
            .map(|n| bevy::tasks::TaskPoolBuilder::new().num_threads(n).build())
            .collect();
        println!("NUCLEAR events,age,workers,median_us,p95_us");
        for count in [1, nuclear::CAP] {
            let (sim, effects, poses) = nuclear_fixture(count);
            for now in [1.0, 6.0, 18.0] {
                let input = Input {
                    sim: &sim,
                    effects: &effects,
                    particles: &[],
                    now,
                    fraction: 1.0,
                    poses: &poses,
                    markers: None,
                    first_person_origin: None,
                    single: (None, 2.0),
                };
                let mut times = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
                for round in 0..64usize {
                    for offset in 0..4 {
                        let case = (round + offset) % 4;
                        let pool = case.checked_sub(1).map(|i| &pools[i]);
                        let begin = std::time::Instant::now();
                        let result = prepare(std::hint::black_box(input), pool);
                        std::hint::black_box(&result);
                        let elapsed = begin.elapsed().as_nanos();
                        if round >= 4 {
                            times[case].push(elapsed);
                        }
                    }
                }
                for (case, values) in times.iter_mut().enumerate() {
                    values.sort_unstable();
                    let workers = [0, 1, 2, 4][case];
                    println!(
                        "NUCLEAR {count},{now},{workers},{:.2},{:.2}",
                        values[30] as f64 / 1000.0,
                        values[57] as f64 / 1000.0
                    );
                }
            }
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
            first_person_origin: None,
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
