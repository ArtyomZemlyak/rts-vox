//! Large camera jumps fill a staging window over several frames. The visible
//! camera/window switch together; physics does not advance across this loading
//! boundary, so the staged columns describe one authoritative scene.
use super::*;

const ROWS_PER_FRAME: i32 = 8;

#[derive(Resource, Default)]
pub struct Streaming {
    invalid: bool,
    job: Option<Job>,
    displayed: Option<(Transform, Projection)>,
    spare: Vec<u32>,
}
struct Job {
    window: TerrainWindow,
    generation: u64,
    row: i32,
    words: Vec<u32>,
    highest: i32,
    dirty: Vec<std::ops::Range<usize>>,
}
impl Streaming {
    pub fn loading(&self) -> bool {
        self.invalid || self.job.is_some()
    }
    pub fn invalidate(&mut self) {
        self.invalid = true;
    }
    pub fn progress(&self) -> Option<u32> {
        self.job
            .as_ref()
            .map(|j| ((j.row - j.window.origin[2]) * 100 / j.window.dims[2] as i32) as u32)
    }
}
impl Job {
    fn step(&mut self, sim: &BlastRange, previous: &[u32]) -> bool {
        let w = self.window;
        let end = (self.row + ROWS_PER_FRAME).min(w.span(2).end);
        let mut column = vec![0; w.dims[1] as usize];
        let slab = w.dims[0] as usize * w.dims[1] as usize / 8;
        for z in self.row..end {
            let start = z as usize % w.dims[2] as usize * slab;
            self.words[start..start + slab].fill(0);
            for x in w.span(0) {
                let top = sim.world.column_ceiling(x as u32, z as u32) as i32;
                self.highest = self.highest.max(top);
                let n = (top.min(w.span(1).end) - w.origin[1]).max(0) as usize;
                sim.world
                    .column(x as u32, z as u32, w.origin[1] as u32, &mut column[..n]);
                for (dy, &m) in column[..n].iter().enumerate() {
                    let p = [x, w.origin[1] + dy as i32, z];
                    let i = w.index(p).unwrap();
                    self.words[i / 8] |= surface_material(sim, p, m) << ((i % 8) * 4);
                }
            }
            for i in start..start + slab {
                if previous.get(i) == Some(&self.words[i]) {
                    continue;
                }
                if let Some(last) = self.dirty.last_mut() {
                    if i >= last.end && i - last.end <= 16 {
                        last.end = i + 1;
                        continue;
                    }
                }
                self.dirty.push(i..i + 1);
            }
        }
        self.row = end;
        end == w.span(2).end
    }
}

pub fn camera(
    scene: Res<Scene>,
    mut camera: Query<(&mut Transform, &mut Projection), With<GasCamera>>,
    mut gas: ResMut<GasInput>,
    mut visual: ResMut<moving::Visual>,
    mut stream: ResMut<Streaming>,
) {
    let Ok((mut transform, mut projection)) = camera.single_mut() else {
        return;
    };
    let ray = transform.forward();
    let distance = (sim_core::blast::FLOOR as f32 - transform.translation.y) / ray.y.min(-0.01);
    let p = transform.translation + ray * distance;
    let want = TerrainWindow::around(
        scene.sim.size(),
        [p.x as i32, 16, p.z as i32],
        TERRAIN_WINDOW,
        WINDOW_STEP,
    );
    let small = want.dims == gas.window.dims
        && (0..3).all(|a| (want.origin[a] - gas.window.origin[a]).abs() <= WINDOW_STEP * 4);
    if stream
        .job
        .as_ref()
        .is_some_and(|j| j.window != want || j.generation != scene.generation)
    {
        stream.spare = stream.job.take().unwrap().words;
    }
    if !stream.invalid && stream.job.is_none() && small {
        if want != gas.window {
            let had = gas.window;
            write_window(&scene.sim, &mut gas, want, Some(had));
            moving::occupancy(&scene.sim, &mut gas, &mut visual);
            gas.advance(scene.sim.tick * 2);
        }
        stream.displayed = Some((*transform, projection.clone()));
        return;
    }
    if stream.job.is_none() {
        let mut words = std::mem::take(&mut stream.spare);
        if words.len() != want.words() {
            words = vec![0; want.words()];
        }
        stream.job = Some(Job {
            window: want,
            generation: scene.generation,
            row: want.origin[2],
            words,
            highest: 0,
            dirty: Vec::new(),
        });
    }
    if stream
        .job
        .as_mut()
        .unwrap()
        .step(&scene.sim, &gas.terrain_words)
    {
        let job = stream.job.take().unwrap();
        let old = std::mem::replace(&mut gas.terrain_words, Arc::new(job.words));
        if let Ok(words) = Arc::try_unwrap(old) {
            stream.spare = words;
        }
        gas.window = want;
        gas.terrain_dirty.extend(job.dirty);
        gas.terrain_revision += 1;
        gas.params.terrain = Vec4::new(
            want.dims[0] as f32,
            want.dims[1] as f32,
            want.dims[2] as f32,
            0.0,
        );
        let ceiling = job
            .highest
            .max(scene.sim.gun.at[1] + 64)
            .min(want.span(1).end) as f32;
        gas.params.terrain_origin = Vec4::new(
            want.origin[0] as f32,
            want.origin[1] as f32,
            want.origin[2] as f32,
            ceiling,
        );
        *visual = default();
        moving::occupancy(&scene.sim, &mut gas, &mut visual);
        gas.advance(scene.sim.tick * 2);
        stream.invalid = false;
        stream.displayed = Some((*transform, projection.clone()));
    } else if let Some((t, p)) = &stream.displayed {
        *transform = *t;
        *projection = p.clone();
    }
}

#[cfg(test)]
pub(super) fn bench(sim: &BlastRange, gas: &GasInput, focus: [i32; 3]) {
    let window = TerrainWindow::around(sim.size(), focus, TERRAIN_WINDOW, WINDOW_STEP);
    let mut job = Job {
        window,
        generation: 0,
        row: window.origin[2],
        words: vec![0; window.words()],
        highest: 0,
        dirty: Vec::new(),
    };
    let mut samples = Vec::new();
    loop {
        let start = std::time::Instant::now();
        let done = job.step(sim, &gas.terrain_words);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        if done {
            break;
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "INTERACTION staged-sector steps={} p99_ms={:.3} max_ms={:.3} total_ms={:.3} patches={}",
        samples.len(),
        samples[samples.len() * 99 / 100],
        samples.last().unwrap(),
        samples.iter().sum::<f64>(),
        job.dirty.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staged_rows_match_whole_window_after_wrapping_and_damage() {
        let mut sim = BlastRange::new(config(false, false));
        for p in [[72, 8, 96], [91, 11, 110], [122, 12, 100]] {
            sim.world.queue_dig(p[0], p[1], p[2]);
        }
        sim.world.flush();
        let window = TerrainWindow {
            origin: [64, 0, 80],
            dims: [64, 64, 64],
        };
        let mut direct = GasInput::default();
        direct.terrain_words = Arc::new(vec![0; window.words()]);
        write_window(&sim, &mut direct, window, None);
        // This window excludes all units, so the terrain alone must match.
        assert!(moving::cells(&sim, &window).is_empty());
        let before = vec![0x77777777; window.words()];
        let mut job = Job {
            window,
            generation: 0,
            row: window.origin[2],
            words: before.clone(),
            highest: 0,
            dirty: Vec::new(),
        };
        let mut steps = 1;
        while !job.step(&sim, &before) {
            steps += 1;
        }
        assert_eq!(steps, 8);
        assert_eq!(&job.words, direct.terrain_words.as_ref());
        let mut uploaded = before;
        for r in job.dirty {
            uploaded[r.clone()].copy_from_slice(&job.words[r]);
        }
        assert_eq!(
            uploaded, job.words,
            "including previously solid cells becoming air"
        );
    }
}
