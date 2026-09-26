//! Distant terrain reads the same world and uses the same shader as the local
//! voxel window. No alternate mesh, palette, sky or zoom-triggered replacement.
use super::*;
use render::gas::distant::{self, SPANS};
use std::sync::{atomic::AtomicBool, mpsc, Mutex};
use voxel::world::columns::Columns;

#[derive(Resource, Default)]
pub struct Distant {
    key: Option<([u32; 3], u64)>,
    job: Option<Job>,
    words: Vec<u32>,
    upload: usize,
    dirty: BTreeSet<[u32; 2]>,
}
struct Job {
    receiver: Mutex<mpsc::Receiver<Vec<u32>>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn build(source: Columns, size: [u32; 3], cancel: &AtomicBool) -> Vec<u32> {
    let [nx, nz, step] = distant::grid(size);
    let mut words = vec![0; nx as usize * nz as usize * SPANS];
    let mut column = vec![0; size[1] as usize];
    for z in 0..nz {
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        for x in 0..nx {
            source.column(
                (x * step + step / 2).min(size[0] - 1),
                (z * step + step / 2).min(size[2] - 1),
                &mut column,
            );
            let i = (x + nx * z) as usize * SPANS;
            words[i..i + SPANS].copy_from_slice(&distant::encode(&column));
        }
    }
    words
}

pub fn sync(scene: Res<Scene>, mut far: ResMut<Distant>, mut gas: ResMut<GasInput>) {
    let size = scene.sim.size();
    let key = (size, scene.generation);
    gas.distant_uploads.retire();
    if far.key != Some(key) {
        let keep_visible = far.key.is_some_and(|k| k.0 == size) && gas.params.distant.w > 0.5;
        *far = Distant {
            key: Some(key),
            ..default()
        };
        gas.params.distant = Vec4::ZERO;
        gas.distant_edits.clear();
        if (size[0] <= TERRAIN_WINDOW[0] && size[2] <= TERRAIN_WINDOW[2]) || size[1] > 255 {
            return;
        }
        let [nx, nz, step] = distant::grid(size);
        gas.params.distant = Vec4::new(
            nx as f32,
            nz as f32,
            step as f32,
            if keep_visible { 1.0 } else { 0.0 },
        );
        gas.params.distant_bounds = Vec4::new(
            size[0] as f32,
            size[1] as f32,
            size[2] as f32,
            render::gas::TERRAIN_WORDS as f32,
        );
        let source = Columns::capture(&scene.sim.world);
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        std::thread::Builder::new()
            .name("distant-terrain".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let words = build(source, size, &worker_cancel);
                if !worker_cancel.load(Ordering::Relaxed) {
                    println!(
                        "DISTANT built {} columns in {:.2}s",
                        words.len() / SPANS,
                        started.elapsed().as_secs_f32()
                    );
                    let _ = sender.send(words);
                }
            })
            .expect("distant terrain worker");
        far.job = Some(Job {
            receiver: Mutex::new(receiver),
            cancel,
        });
    }
    far.dirty.extend(std::mem::take(&mut gas.distant_edits));
    let result = far
        .job
        .as_ref()
        .and_then(|job| job.receiver.lock().unwrap().try_recv().ok());
    if let Some(words) = result {
        far.job = None;
        far.words = words;
    }
    if far.words.is_empty() {
        return;
    }
    let [nx, nz, step] = distant::grid(size);
    // Incremental crater/building edits also reach the far picture. A worker
    // snapshot may be older than these edits, so apply them before publication.
    let mut column = vec![0; size[1] as usize];
    let mut changed: Vec<std::ops::Range<usize>> = Vec::new();
    for _ in 0..512 {
        let Some(q) = far.dirty.pop_first() else {
            break;
        };
        if q[0] >= nx || q[1] >= nz {
            continue;
        }
        let x = (q[0] * step + step / 2).min(size[0] - 1);
        let z = (q[1] * step + step / 2).min(size[2] - 1);
        scene.sim.world.column(x, z, 0, &mut column);
        for (y, m) in column.iter_mut().enumerate() {
            *m = surface_material(&scene.sim, [x as i32, y as i32, z as i32], *m) as u16;
        }
        let i = (q[0] + nx * q[1]) as usize * SPANS;
        let next = distant::encode(&column);
        if far.words[i..i + SPANS] != next {
            far.words[i..i + SPANS].copy_from_slice(&next);
            if i < far.upload {
                changed.push(i..i + SPANS);
            }
        }
    }
    changed.sort_unstable_by_key(|r| r.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for r in changed {
        if let Some(last) = merged.last_mut() {
            if r.start <= last.end + 32 {
                last.end = last.end.max(r.end);
                continue;
            }
        }
        merged.push(r);
    }
    for r in merged {
        gas.distant_uploads.push(r.start, &far.words[r]);
    }
    // Limit startup/reset transfer to 4 MiB per frame. Panning uploads nothing.
    let end = (far.upload + 1024 * 1024).min(far.words.len());
    if end > far.upload {
        gas.distant_uploads
            .push(far.upload, &far.words[far.upload..end]);
        far.upload = end;
    }
    if far.upload == far.words.len() && far.dirty.is_empty() {
        gas.params.distant.w = 1.0;
    }
}

/// An RTS orthographic zoom changes magnification, not the weather. Keep the
/// authored near-view atmosphere and continuously rescale its optical distance
/// as the camera backs away. Never remove/reinsert the sky/IBL components.
pub fn atmosphere(
    camera: Query<&Transform, With<GasCamera>>,
    mut atmospheres: Query<&mut GlobalTransform, With<bevy::light::Atmosphere>>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    let ground = sim_core::blast::FLOOR as f32;
    let zoom = ((camera.translation.y - ground) / 420.0).max(1.0);
    let k = zoom / look::scene().haze_metres_per_voxel;
    for mut transform in &mut atmospheres {
        *transform = GlobalTransform::from(
            Transform::from_scale(Vec3::splat(k)).with_translation(Vec3::new(
                0.0,
                ground - 6_360_000.0 * k,
                0.0,
            )),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distant_columns_read_authored_materials_and_actual_destruction() {
        use voxel::world::{Edit, Generator, World};
        struct Authored;
        impl Generator for Authored {
            fn material(&self, x: u32, y: u32, z: u32) -> u16 {
                if y < 10 {
                    1
                } else if y == 10 {
                    if z < 16 {
                        8
                    } else {
                        7
                    }
                } else if (16..32).contains(&x) && (20..24).contains(&y) {
                    3
                } else if (40..48).contains(&x) && (30..40).contains(&y) {
                    4
                } else {
                    0
                }
            }
        }
        let size = [64, 64, 64];
        let mut world = World::new(64, 64, 64, 10000, Box::new(Authored));
        world.queue_edit(Edit::dig(22, 22, 22));
        world.flush();
        let words = build(Columns::capture(&world), size, &AtomicBool::new(false));
        let [nx, nz, step] = distant::grid(size);
        for z in 0..nz {
            for x in 0..nx {
                let i = (x + nx * z) as usize * SPANS;
                for y in 0..64 {
                    let actual = words[i..i + SPANS]
                        .iter()
                        .find(|w| y >= (**w & 255) && y < ((**w >> 8) & 255))
                        .map_or(0, |w| {
                            if y >= (*w >> 24) {
                                (*w >> 20) & 15
                            } else {
                                (*w >> 16) & 15
                            }
                        });
                    assert_eq!(
                        actual,
                        u32::from(world.material(x * step + step / 2, y, z * step + step / 2))
                    );
                }
            }
        }
    }
    #[test]
    fn full_map_fits_at_each_orbit_and_pitch() {
        let size = [10_000, 192, 10_000];
        let aspect = 16.0 / 9.0;
        for yaw in [0.0, 0.8, 2.1, 4.3] {
            for pitch in [-0.5, 0.0, 0.5] {
                let offset = orbit_offset(Vec3::new(-155.0, 420.0, 320.0), yaw, pitch);
                let (aim, height) = whole_map_frame(size, offset, aspect);
                let rotation = Transform::from_translation(offset)
                    .looking_at(Vec3::ZERO, Vec3::Y)
                    .rotation;
                for x in [0.0, size[0] as f32] {
                    for y in [0.0, size[1] as f32] {
                        for z in [0.0, size[2] as f32] {
                            let relative = Vec3::new(x, y, z) - aim;
                            assert!(
                                (rotation * Vec3::X).dot(relative).abs() <= height * aspect / 2.0
                            );
                            assert!((rotation * Vec3::Y).dot(relative).abs() <= height / 2.0);
                        }
                    }
                }
            }
        }
    }
}
