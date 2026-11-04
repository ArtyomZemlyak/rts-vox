//! Bounded, read-only metric samples on the existing camera-facing section.
use super::*;
use render::geom::MeshBuf;
use std::collections::BTreeSet;

const SIDE: i32 = 48;
const SAMPLES: i32 = SIDE * 2;
const ROWS_PER_FRAME: i32 = 8;
#[derive(Component)]
pub struct MetricSurface;
type Key = (u64, u32, u8, [u32; 7], u32, [i32; 3], bool, u8, bool);
struct Work {
    key: Key,
    camera: Transform,
    row: i32,
    output: MeshBuf,
    seen: BTreeSet<([i32; 3], usize, i32)>,
}
#[derive(Default)]
pub struct Cache {
    key: Option<Key>,
    work: Option<Work>,
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
}
pub(crate) fn scale(n: u32, max: u32) -> [f32; 4] {
    let t = (n as f32 / max.max(1) as f32).clamp(0.0, 1.0);
    if t < 0.5 {
        let k = t * 2.0;
        [0.08, 0.2 + 0.65 * k, 0.85 - 0.65 * k, 1.0]
    } else {
        let k = (t - 0.5) * 2.0;
        [0.08 + 0.88 * k, 0.85 - 0.68 * k, 0.2 - 0.12 * k, 1.0]
    }
}
pub(crate) const UNKNOWN: [f32; 4] = [0.35, 0.35, 0.38, 1.0];
fn sample(
    sim: &BlastRange,
    p: [i32; 3],
    mode: u8,
    units: &[(usize, &sim_core::blast::RangeUnit)],
) -> Option<[f32; 4]> {
    if p.iter().any(|v| *v < 0) {
        return None;
    }
    let info = sim.inspect_cell_with_motion(p.map(|v| v as u32), mode == 6)?;
    let unit = units
        .iter()
        .find_map(|&(id, u)| u.hit(p).map(|(part, cell)| (id, part, cell, u)));
    if info.material == 0 && info.water == 0 && unit.is_none() {
        return None;
    }
    let separate = unit.is_some();
    let mat = if separate {
        3
    } else if info.water > 0 {
        11
    } else {
        info.material
    };
    let def = sim.matter.economy.mats.get(mat);
    let heat = unit
        .as_ref()
        .map(|(id, part, cell, u)| {
            let energy = sim.thermals.heat(sim_core::blast::thermal::Cell {
                unit: *id as u16,
                part: *part as u8,
                at: cell.map(|v| v as u8),
            });
            let mm = if *part == 0 {
                u.body.mm
            } else {
                u.body
                    .segments
                    .get(*part - 1)
                    .map_or(u.body.mm, |s| s.body.mm)
            };
            sim_core::blast::thermal::temperature(energy, mm)
        })
        .unwrap_or(info.heat.heat);
    Some(match mode {
        1 => {
            if mat == 11 && !separate {
                UNKNOWN
            } else {
                scale(heat, 1000)
            }
        }
        2 => {
            if separate {
                UNKNOWN
            } else if mat == 11 {
                scale(100, 100)
            } else if info.moisture_capacity == 0 {
                UNKNOWN
            } else {
                scale(
                    info.moisture.saturating_mul(100) / info.moisture_capacity,
                    100,
                )
            }
        }
        3 => {
            if matches!(mat, 11 | 13 | 14 | 16) {
                [0.1, 0.45, 0.95, 1.0]
            } else if def.is_some_and(|d| d.loose) {
                [0.95, 0.72, 0.1, 1.0]
            } else {
                [0.58, 0.58, 0.62, 1.0]
            }
        }
        4 => {
            if separate {
                UNKNOWN
            } else {
                def.map_or(UNKNOWN, |d| scale(d.strength, 500))
            }
        }
        5 => {
            if separate {
                UNKNOWN
            } else {
                def.map_or(UNKNOWN, |d| scale(d.density, 1000))
            }
        }
        6 => {
            if let Some((_, _, _, u)) = unit {
                scale(
                    (u.motion.drive.vel.raw().unsigned_abs().saturating_mul(10)
                        / fx::ONE_RAW as u64)
                        .min(u64::from(u32::MAX)) as u32,
                    500,
                )
            } else if mat == 11 {
                sim.water.falling_speed_at(info.at).map_or(UNKNOWN, |v| {
                    scale(
                        (v.raw().max(0) as u64)
                            .saturating_mul(200)
                            .saturating_div(fx::ONE_RAW as u64)
                            .min(u64::from(u32::MAX)) as u32,
                        500,
                    )
                })
            } else if let Some((v, _)) = info.motion {
                scale(
                    (v.raw().max(0) as u64)
                        .saturating_mul(200)
                        .saturating_div(fx::ONE_RAW as u64)
                        .min(u64::from(u32::MAX)) as u32,
                    500,
                )
            } else {
                scale(0, 500)
            }
        }
        7 => {
            if let Some((_, part, cell, u)) = unit {
                let v = if part == 0 {
                    u.dents.get(&cell.map(|v| v as u8)).copied().unwrap_or(0)
                } else {
                    u.seg_dent(part - 1, cell)
                };
                scale(v, 2000)
            } else {
                scale(info.deformation_work, 2000)
            }
        }
        8 => {
            if separate || mat == 11 {
                UNKNOWN
            } else {
                scale(u32::from(info.heat.burn), 40)
            }
        }
        9 => {
            if separate {
                UNKNOWN
            } else if mat == 11 {
                scale(info.water.min(info.water_capacity), info.water_capacity)
            } else {
                UNKNOWN
            }
        }
        10 => {
            if separate {
                UNKNOWN
            } else {
                let id = u32::from(mat);
                [
                    0.18 + (id * 43 % 70) as f32 / 100.0,
                    0.18 + (id * 67 % 70) as f32 / 100.0,
                    0.18 + (id * 29 % 70) as f32 / 100.0,
                    1.0,
                ]
            }
        }
        _ => return None,
    })
}
fn build(sim: &BlastRange, s: &section::State, mode: u8, scale: u8) -> MeshBuf {
    let mut out = MeshBuf::default();
    let axis = s.axis.min(2) as usize;
    let mut right = Vec3::ZERO;
    right[(axis + 1) % 3] = 1.0;
    let mut up = Vec3::ZERO;
    up[(axis + 2) % 3] = 1.0;
    let forward = s.plane().truncate();
    let mut plane = Vec3::from(s.point().map(|v| v as f32)) + Vec3::splat(0.5);
    plane[axis] = s.point()[axis] as f32;
    // Cache only units whose bounds can intersect this 4.8 m window.
    let step = f32::from(scale.max(1));
    let radius = SIDE as f32 * step * 0.72 + 2.0;
    let units: Vec<_> = sim
        .units
        .iter()
        .chain((!sim.point_gun()).then_some(&sim.gun))
        .enumerate()
        .filter(|(_, u)| {
            let (lo, hi) = u.bounds();
            (0..3).all(|a| lo[a] as f32 <= plane[a] + radius && hi[a] as f32 >= plane[a] - radius)
        })
        .collect();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let c = plane
                + right * ((x as f32 - SIDE as f32 * 0.5 + 0.5) * step)
                + up * ((y as f32 - SIDE as f32 * 0.5 + 0.5) * step);
            let at = (c + forward * 0.1).floor().to_array().map(|v| v as i32);
            let Some(colour) = sample(sim, at, mode, &units) else {
                continue;
            };
            let first = out.pos.len() as u32;
            for p in [
                c - right * (step * 0.5) - up * (step * 0.5),
                c + right * (step * 0.5) - up * (step * 0.5),
                c + right * (step * 0.5) + up * (step * 0.5),
                c - right * (step * 0.5) + up * (step * 0.5),
            ] {
                out.pos.push(p.to_array());
                out.nrm.push((-forward).to_array());
                out.col.push(colour);
            }
            out.idx
                .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
        }
    }
    out
}
/// A bounded window of camera rays finds the real visible voxel faces. The
/// diagnostic mesh stays separate from the authoritative world and water.
fn occupied(
    sim: &BlastRange,
    p: [i32; 3],
    units: &[(usize, &sim_core::blast::RangeUnit)],
    bounds: &[([i32; 3], [i32; 3])],
) -> bool {
    if (0..3).any(|a| p[a] < 0 || p[a] >= sim.size()[a] as i32) {
        return false;
    }
    let q = p.map(|v| v as u32);
    sim.world.material(q[0], q[1], q[2]) != 0
        || sim.water.amount(q) > 0
        || units.iter().zip(bounds).any(|((_, u), (lo, hi))| {
            (0..3).all(|a| p[a] >= lo[a] && p[a] < hi[a]) && u.hit(p).is_some()
        })
}
fn surface_rows(
    sim: &BlastRange,
    t: &Transform,
    anchor: [i32; 3],
    mode: u8,
    scale: u8,
    perspective: bool,
    rows: std::ops::Range<i32>,
    out: &mut MeshBuf,
    seen: &mut BTreeSet<([i32; 3], usize, i32)>,
) {
    let right = *t.right();
    let up = *t.up();
    let forward = *t.forward();
    let centre = Vec3::from(anchor.map(|v| v as f32)) + Vec3::splat(0.5);
    let step = f32::from(scale.max(1));
    let radius = SIDE as f32 * step * 0.72 + 66.0;
    let units: Vec<_> = sim
        .units
        .iter()
        .chain((!sim.point_gun()).then_some(&sim.gun))
        .enumerate()
        .filter(|(_, u)| {
            let (lo, hi) = u.bounds();
            (0..3).all(|a| lo[a] as f32 <= centre[a] + radius && hi[a] as f32 >= centre[a] - radius)
        })
        .collect();
    let bounds: Vec<_> = units.iter().map(|(_, u)| u.bounds()).collect();
    for y in rows {
        for x in 0..SAMPLES {
            let target = centre
                + right * (((x as f32 + 0.5) * 0.5 - SIDE as f32 * 0.5) * step)
                + up * (((y as f32 + 0.5) * 0.5 - SIDE as f32 * 0.5) * step);
            let direction = if perspective {
                Dir3::new(target - t.translation).unwrap_or(t.forward())
            } else {
                t.forward()
            };
            let origin = if perspective {
                t.translation + *direction * ((target - t.translation).length() - 64.0).max(0.0)
            } else {
                target - forward * 64.0
            };
            let ray = Ray3d::new(origin, direction);
            let mut checked = 0;
            let found = workbench::visit_pick_ray(sim.size(), ray, |p| {
                checked += 1;
                if checked > 128 {
                    return Some(None);
                }
                occupied(sim, p, &units, &bounds).then_some(Some(p))
            })
            .flatten();
            let Some(p) = found else {
                continue;
            };
            let lo = Vec3::from(p.map(|v| v as f32));
            let Some((_, axis)) = tools::entry(ray, lo, lo + Vec3::ONE) else {
                continue;
            };
            let side = if direction[axis] > 0.0 { 0 } else { 1 };
            let mut normal = Vec3::ZERO;
            normal[axis] = if side == 0 { -1.0 } else { 1.0 };
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            // Neighbours on the same exposed face close projection sampling gaps
            // without increasing the number of rays fourfold again.
            for (du, dv) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                let mut q = p;
                q[u] += du;
                q[v] += dv;
                if !seen.insert((q, axis, side)) {
                    continue;
                }
                if !occupied(sim, q, &units, &bounds) {
                    continue;
                }
                let mut front = q;
                front[axis] += if side == 0 { -1 } else { 1 };
                if occupied(sim, front, &units, &bounds) {
                    continue;
                }
                let Some(colour) = sample(sim, q, mode, &units) else {
                    continue;
                };
                let lo = Vec3::from(q.map(|v| v as f32));
                let mut h = 1.0;
                if sim.world.material(q[0] as u32, q[1] as u32, q[2] as u32) == 0 {
                    let n = sim.water.amount(q.map(|a| a as u32));
                    if n > 0
                        && sim
                            .water
                            .amount([q[0] as u32, q[1] as u32 + 1, q[2] as u32])
                            == 0
                    {
                        h = (n as f32 / sim.water.tuning.units_per_voxel as f32).min(1.0);
                    }
                }
                let first = out.pos.len() as u32;
                for (a, b) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                    let mut point = lo;
                    point[axis] += if side == 0 {
                        0.0
                    } else if axis == 1 {
                        h
                    } else {
                        1.0
                    };
                    point[u] += if u == 1 { a * h } else { a };
                    point[v] += if v == 1 { b * h } else { b };
                    point += normal * 0.02;
                    out.pos.push(point.to_array());
                    out.nrm.push(normal.to_array());
                    out.col.push(colour);
                }
                if side == 1 {
                    out.idx
                        .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
                } else {
                    out.idx
                        .extend([first, first + 2, first + 1, first, first + 3, first + 2]);
                }
            }
        }
    }
}
#[cfg(test)]
fn surface(sim: &BlastRange, t: &Transform, anchor: [i32; 3], mode: u8) -> MeshBuf {
    let mut out = MeshBuf::default();
    let mut seen = BTreeSet::new();
    surface_rows(
        sim,
        t,
        anchor,
        mode,
        1,
        false,
        0..SAMPLES,
        &mut out,
        &mut seen,
    );
    out
}
const THERMAL_MIN_SLOTS: usize = 8192;
const THERMAL_MAX_SLOTS: usize = 131072;
const THERMAL_LIMIT: usize = THERMAL_MAX_SLOTS / 2;
fn metric_hash(p: [u32; 3]) -> u32 {
    let mut h =
        p[0].wrapping_mul(73856093) ^ p[1].wrapping_mul(19349663) ^ p[2].wrapping_mul(83492791);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846ca68b);
    h ^ (h >> 16)
}
#[derive(Default)]
pub struct ThermalCache {
    key: Option<(u64, u32, u8)>,
}
fn thermal_table(sim: &BlastRange, mode: u8) -> (Vec<u32>, bool) {
    let count = match mode {
        1 | 8 => sim.heat.hot(),
        2 => sim
            .water
            .diagnostic_wet_cells()
            .take(THERMAL_LIMIT + 1)
            .count(),
        7 => sim.surface_work.len(),
        _ => 0,
    };
    let slots = count
        .saturating_mul(4)
        .clamp(THERMAL_MIN_SLOTS, THERMAL_MAX_SLOTS)
        .next_power_of_two();
    let mut words = vec![0u32; (slots + 16) * 4];
    for (code, ids) in [
        (1, &[1u16][..]),
        (2, &[2][..]),
        (3, &[3][..]),
        (4, &[4][..]),
        (5, &[5][..]),
        (6, &[6][..]),
        (7, &[7][..]),
        (8, &[8, 10][..]),
        (9, &[3][..]),
        (10, &[3][..]),
        (11, &[12][..]),
        (12, &[2, 6][..]),
        (13, &[13, 16][..]),
        (14, &[14][..]),
    ] {
        let value = |id: u16| {
            sim.matter.economy.mats.get(id).map(|m| {
                [
                    if matches!(id, 13 | 14 | 16) {
                        1
                    } else if m.loose {
                        2
                    } else {
                        3
                    },
                    m.strength,
                    m.density,
                    u32::from(id),
                ]
            })
        };
        let base = value(ids[0]).unwrap_or([0, u32::MAX, u32::MAX, u32::MAX]);
        let mut values = std::array::from_fn::<_, 4, _>(|a| {
            if ids
                .iter()
                .all(|&id| value(id).is_some_and(|v| v[a] == base[a]))
            {
                base[a]
            } else {
                if a == 0 {
                    0
                } else {
                    u32::MAX
                }
            }
        });
        if ids.iter().all(|id| {
            sim.water
                .tuning
                .porous
                .iter()
                .any(|p| p.material == *id && p.capacity > 0)
        }) {
            values[0] |= 0x8000_0000;
        }
        words[(slots + code) * 4..(slots + code + 1) * 4].copy_from_slice(&values);
    }
    let entries: Box<dyn Iterator<Item = ([u32; 3], u32)> + '_> = match mode {
        1 => Box::new(
            sim.heat
                .diagnostic_cells()
                .filter(|(_, c)| c.heat > 0)
                .map(|(p, c)| (p, c.heat)),
        ),
        8 => Box::new(
            sim.heat
                .diagnostic_cells()
                .filter(|(_, c)| c.burn > 0)
                .map(|(p, c)| (p, u32::from(c.burn))),
        ),
        2 => Box::new(
            sim.water
                .diagnostic_wet_cells()
                .filter_map(|(p, mat, amount)| {
                    sim.water
                        .tuning
                        .porous
                        .iter()
                        .find(|v| v.material == mat && v.capacity > 0)
                        .map(|v| (p, amount.saturating_mul(100) / v.capacity))
                }),
        ),
        7 => Box::new(sim.surface_work.iter().filter_map(|(&p, &v)| {
            p.iter()
                .all(|x| *x >= 0)
                .then_some((p.map(|x| x as u32), v))
        })),
        _ => return (words, false),
    };
    let mut overflow = false;
    for (number, (p, value)) in entries.enumerate() {
        if number >= THERMAL_LIMIT {
            overflow = true;
            break;
        }
        let hash = metric_hash(p);
        let mut placed = false;
        for probe in 0..32u32 {
            let i = ((hash.wrapping_add(probe) as usize) & (slots - 1)) * 4;
            if words[i] == 0 {
                words[i] = p[0] + 1;
                words[i + 1] = p[1];
                words[i + 2] = p[2];
                words[i + 3] = value;
                placed = true;
                break;
            }
        }
        if !placed {
            overflow = true;
        }
    }
    (words, overflow)
}
pub fn terrain_offset(
    lab: Res<Laboratory>,
    scene: Res<Scene>,
    cameras: Query<&Projection, With<GasCamera>>,
    mut gas: ResMut<GasInput>,
    mut cache: Local<ThermalCache>,
) {
    let _ = cameras;
    gas.params.look.z = 0.0;
    let thermal = lab.active() && matches!(lab.tools.metric, 1 | 2 | 3 | 4 | 5 | 7 | 8 | 10);
    gas.params.look.y = if thermal {
        f32::from(lab.tools.metric)
    } else {
        0.0
    };
    if thermal {
        let dynamic = matches!(lab.tools.metric, 1 | 2 | 7 | 8);
        let key = (
            scene.generation,
            if dynamic { scene.sim.tick / 5 } else { 0 },
            lab.tools.metric,
        );
        if cache.key != Some(key) {
            let (words, overflow) = thermal_table(&scene.sim, lab.tools.metric);
            if std::env::var("RTS_LAB_CAPTURE_HEAT_STRESS").as_deref() == Ok("1") {
                println!(
                    "HE_HEAT_TABLE slots={} overflow={overflow}",
                    words.len() / 4 - 16
                );
            }
            gas.metric_words = std::sync::Arc::new(words);
            gas.metric_revision = gas.metric_revision.wrapping_add(1);
            gas.params.look.w = if overflow { 1.0 } else { 0.0 };
            cache.key = Some(key);
        }
    } else {
        gas.params.look.w = 0.0;
        cache.key = None;
    }
}
#[allow(clippy::too_many_arguments)]
pub fn draw(
    mut commands: Commands,
    lab: Res<Laboratory>,
    scene: Res<Scene>,
    cameras: Query<(&Transform, &Projection), With<GasCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: Local<Cache>,
    mut visibility: Query<&mut Visibility, With<MetricSurface>>,
) {
    // Heat and burn are shaded continuously by the terrain renderer; the
    // local sampling mesh is only needed on an active section plane.
    let active = lab.active()
        && lab.tools.metric > 0
        && !matches!(lab.tools.metric, 1 | 2 | 3 | 4 | 5 | 7 | 8 | 10);
    let Some((t, projection)) = cameras.iter().next().filter(|_| active) else {
        cache.work = None;
        if let Some(e) = cache.entity {
            if let Ok(mut v) = visibility.get_mut(e) {
                *v = Visibility::Hidden;
            }
        }
        return;
    };
    let perspective = matches!(projection, Projection::Perspective(_));
    let cut = lab.tools.section.enabled;
    let near = if cut {
        f32::from(lab.tools.section.axis) + if lab.tools.section.positive { 4.0 } else { 0.0 }
    } else {
        0.1
    };
    let anchor = if cut {
        lab.tools.section.point()
    } else if lab.tools.live {
        lab.tools.live_target.map(|(at, _)| at).unwrap_or_else(|| {
            (t.translation + *t.forward() * lab.tools.distance)
                .floor()
                .to_array()
                .map(|v| v as i32)
        })
    } else {
        section::anchor(&lab, &scene)
    };
    let transform = [
        t.translation.x,
        t.translation.y,
        t.translation.z,
        t.rotation.x,
        t.rotation.y,
        t.rotation.z,
        t.rotation.w,
    ]
    .map(f32::to_bits);
    let key = (
        scene.generation,
        scene.sim.tick / 5,
        lab.tools.metric,
        transform,
        near.to_bits(),
        anchor,
        cut,
        lab.tools.metric_scale,
        perspective,
    );
    if cache.key == Some(key) && cache.work.is_none() {
        if let Some(e) = cache.entity {
            if let Ok(mut v) = visibility.get_mut(e) {
                *v = Visibility::Visible;
            }
        }
        return;
    }
    let b = if cut {
        cache.work = None;
        build(
            &scene.sim,
            &lab.tools.section,
            lab.tools.metric,
            lab.tools.metric_scale,
        )
    } else {
        let incompatible = cache.work.as_ref().is_some_and(|w| {
            w.key.0 != key.0
                || w.key.2 != key.2
                || (!lab.tools.live && w.key.5 != key.5)
                || w.key.6 != key.6
                || w.key.7 != key.7
                || w.key.8 != key.8
        });
        if incompatible {
            cache.work = None;
        }
        let work = cache.work.get_or_insert_with(|| Work {
            key,
            camera: *t,
            row: 0,
            output: MeshBuf::default(),
            seen: BTreeSet::new(),
        });
        let end = (work.row + ROWS_PER_FRAME).min(SAMPLES);
        surface_rows(
            &scene.sim,
            &work.camera,
            work.key.5,
            work.key.2,
            work.key.7,
            work.key.8,
            work.row..end,
            &mut work.output,
            &mut work.seen,
        );
        work.row = end;
        if end < SAMPLES {
            return;
        }
        let work = cache.work.take().expect("metric work");
        cache.key = Some(work.key);
        work.output
    };
    let material = cache
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();
    if let Some(handle) = &cache.mesh {
        meshes.insert(handle.id(), mesh(&b)).expect("metric mesh");
    } else {
        let handle = meshes.add(mesh(&b));
        let e = commands
            .spawn((
                MetricSurface,
                Mesh3d(handle.clone()),
                MeshMaterial3d(material),
                Transform::default(),
            ))
            .id();
        cache.mesh = Some(handle);
        cache.entity = Some(e);
    }
    if let Some(e) = cache.entity {
        if let Ok(mut v) = visibility.get_mut(e) {
            *v = Visibility::Visible;
        }
    }
    if cut {
        cache.key = Some(key);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_has_order() {
        assert!(scale(0, 100)[2] > scale(0, 100)[0]);
        assert!(scale(100, 100)[0] > scale(100, 100)[2]);
    }
    #[test]
    fn global_thermal_table_encodes_real_heat_beyond_old_explosion_limit() {
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
        let mut sim = BlastRange::with_layout(cfg, layout).unwrap();
        let p = [100, 5, 100];
        sim.heat.add(&sim.world, p, 777);
        let (table, overflow) = thermal_table(&sim, 1);
        assert!(!overflow);
        let slots = table.len() / 4 - 16;
        assert_eq!(&table[(slots + 3) * 4..(slots + 4) * 4], &[3, 500, 785, 3]);
        assert_eq!(
            &table[(slots + 8) * 4..(slots + 9) * 4],
            &[0x8000_0002, u32::MAX, u32::MAX, u32::MAX]
        );
        let h = metric_hash(p);
        let i = (h as usize & (slots - 1)) * 4;
        assert_eq!(&table[i..i + 4], &[p[0] + 1, p[1], p[2], 777]);
        sim.surface_work.insert(p.map(|v| v as i32), 1234);
        let (work, overflow) = thermal_table(&sim, 7);
        assert!(!overflow);
        assert_eq!(&work[i..i + 4], &[p[0] + 1, p[1], p[2], 1234]);
        for x in 350..430 {
            for z in 300..380 {
                sim.heat.add(&sim.world, [x, 5, z], 1);
            }
        }
        assert!(sim.heat.hot() > 4096);
        let (large, overflow) = thermal_table(&sim, 1);
        assert!(!overflow);
        let large_slots = large.len() / 4 - 16;
        assert!(large_slots > slots);
        for p in [[350u32, 5, 300], [392, 5, 344], [429, 5, 379]] {
            let hash = metric_hash(p);
            assert!(
                (0..32).any(|probe| {
                    let i = (hash.wrapping_add(probe) as usize & (large_slots - 1)) * 4;
                    large[i..i + 4] == [p[0] + 1, p[1], p[2], 1]
                }),
                "missing heat at {p:?}"
            );
        }
        assert!(!thermal_table(&sim, 3).1);
    }
    #[test]
    fn overflow_keeps_known_cells_instead_of_erasing_the_table() {
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
        let mut sim = BlastRange::with_layout(cfg, layout).unwrap();
        for x in 50..307 {
            for z in 50..307 {
                sim.heat.add(&sim.world, [x, 5, z], 123);
            }
        }
        assert!(sim.heat.hot() > THERMAL_LIMIT);
        let (table, overflow) = thermal_table(&sim, 1);
        assert!(overflow);
        let slots = table.len() / 4 - 16;
        assert_eq!(slots, THERMAL_MAX_SLOTS);
        let p = [50u32, 5, 50];
        let h = metric_hash(p);
        assert!((0..32).any(|probe| {
            let i = (h.wrapping_add(probe) as usize & (slots - 1)) * 4;
            table[i..i + 4] == [p[0] + 1, p[1], p[2], 123]
        }));
    }
    #[test]
    fn sampled_water_is_classified_without_inventing_temperature_or_mutating_simulation() {
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
        let mut sim = BlastRange::with_layout(cfg, layout).unwrap();
        let at = [100, 50, 100];
        assert!(sim.water.edit_volume(at, true));
        let before = sim.state_hash();
        assert_eq!(sample(&sim, at.map(|v| v as i32), 1, &[]), Some(UNKNOWN));
        assert_eq!(
            sample(&sim, at.map(|v| v as i32), 2, &[]),
            Some(scale(100, 100))
        );
        assert_eq!(
            sample(&sim, at.map(|v| v as i32), 3, &[]),
            Some([0.1, 0.45, 0.95, 1.0])
        );
        assert_eq!(
            sample(&sim, at.map(|v| v as i32), 6, &[]),
            Some(scale(0, 500))
        );
        assert_eq!(sample(&sim, at.map(|v| v as i32), 0, &[]), None);
        let camera = Transform::from_xyz(100.5, 65.0, 100.5)
            .looking_at(Vec3::new(100.5, 50.0, 100.5), Vec3::Z);
        let slice = build(
            &sim,
            &section::State {
                enabled: true,
                anchor: [100, 50, 100],
                axis: 1,
                positive: true,
                offset: 0,
            },
            3,
            1,
        );
        assert!(!slice.pos.is_empty());
        assert!(slice.pos.len() <= SIDE as usize * SIDE as usize * 4);
        assert_eq!(slice.pos.len(), slice.col.len());
        let started = std::time::Instant::now();
        let faces = surface(&sim, &camera, [100, 50, 100], 3);
        eprintln!(
            "surface diagnostic build: {:?}, faces: {}",
            started.elapsed(),
            faces.pos.len() / 4
        );
        assert!(!faces.pos.is_empty());
        assert!(faces.pos.len() <= SAMPLES as usize * SAMPLES as usize * 4);
        assert!(faces.pos.iter().all(|p| p.iter().all(|v| v.is_finite())));
        assert!(faces.col.iter().any(|c| *c == [0.1, 0.45, 0.95, 1.0]));
        let mut split = MeshBuf::default();
        let mut seen = BTreeSet::new();
        for row in (0..SAMPLES).step_by(ROWS_PER_FRAME as usize) {
            surface_rows(
                &sim,
                &camera,
                [100, 50, 100],
                3,
                1,
                false,
                row..(row + ROWS_PER_FRAME).min(SAMPLES),
                &mut split,
                &mut seen,
            );
        }
        assert_eq!(faces.pos, split.pos);
        assert_eq!(faces.col, split.col);
        assert_eq!(faces.idx, split.idx);
        let mut perspective = MeshBuf::default();
        let mut used = BTreeSet::new();
        surface_rows(
            &sim,
            &camera,
            [100, 50, 100],
            3,
            1,
            true,
            0..SAMPLES,
            &mut perspective,
            &mut used,
        );
        assert!(!perspective.pos.is_empty());
        assert_eq!(sim.state_hash(), before);
    }
}
