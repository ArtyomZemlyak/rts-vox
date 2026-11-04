//! Large camera jumps fill a staging window over several frames. The visible
//! camera/window switch together. Camera streaming never pauses physics;
//! staged rows replay terrain changes before publication. Scene resets still
//! hold their loading boundary until the new generation is ready.
use super::*;
use sim_core::blast::read_model::terrain::{Budget, Changes, Header, Request, Row as TerrainRow};

const ROWS_PER_FRAME: i32 = 8;

pub(super) fn map_scale(
    size: [u32; 3],
    height: f32,
    local_limit: f32,
    was_map_scale: bool,
) -> bool {
    if size[0] <= TERRAIN_WINDOW[0] && size[2] <= TERRAIN_WINDOW[2] {
        return false;
    }
    // Use the same projected footprint as framing: a fixed height switched
    // ordinary narrow-window sector views to the map and left detail behind.
    // The default local fit always returns to detail, even after map scale.
    if was_map_scale {
        // Framing uses the eye offset, streaming reconstructs its direction;
        // allow their floating-point roundoff at the same projected boundary.
        height > local_limit * 1.001
    } else {
        height >= local_limit * 1.1
    }
}

#[derive(Resource, Default)]
pub struct Streaming {
    invalid: bool,
    pub(super) map_scale: bool,
    job: Option<Job>,
    displayed: Option<(Transform, Projection)>,
    spare: Vec<u32>,
}
struct Job {
    window: TerrainWindow,
    generation: u64,
    row: i32,
    words: Vec<u32>,
    tops: Vec<u16>,
    highest: i32,
    dirty: Vec<std::ops::Range<usize>>,
    repairs: std::collections::BTreeSet<i32>,
    aliases: PatchUnion,
    rows: Vec<Row>,
    changes: Changes,
    costs: Option<[u32; 3]>,
}
impl Streaming {
    pub fn loading(&self) -> bool {
        !self.map_scale && self.invalid
    }
    pub fn invalidate(&mut self) {
        self.invalid = true;
    }
    pub fn progress(&self) -> Option<u32> {
        self.job
            .as_ref()
            .map(|j| ((j.row - j.window.origin[2]) * 100 / j.window.dims[2] as i32) as u32)
    }
    /// Called after every completed tick and its visible terrain/occupancy
    /// patches. An unfilled row already reads the latest authoritative world.
    pub fn observe(&mut self, sim: &BlastRange, generation: u64, dirty: &[std::ops::Range<usize>]) {
        if let Some(job) = &mut self.job {
            if job.generation == generation {
                job.observe(sim, dirty);
            }
        }
    }
}
#[derive(Default)]
struct Row {
    z: i32,
    input: TerrainRow,
    words: Vec<u32>,
    tops: Vec<u16>,
    highest: i32,
}
impl Row {
    fn read(&mut self, w: TerrainWindow) {
        let dx = w.dims[0] as usize;
        let dy = w.dims[1] as usize;
        let stride = dx / 8;
        self.words.resize(stride * dy, 0);
        self.words.fill(0);
        self.tops.resize(dx, 0);
        self.highest = 0;
        for x in self.input.request.as_ref().unwrap().x.clone() {
            let (column, materials) = self.input.column(x).unwrap();
            let top = column.top as i32;
            self.highest = self.highest.max(top);
            self.tops[x as usize % dx] = top.min(w.span(1).end) as u16;
            let shift = (x as usize % dx % 8) * 4;
            let mut word = x as usize % dx / 8 + w.origin[1] as usize % dy * stride;
            for &material in materials {
                self.words[word] |= surface_tint(material, 0, 1) << shift;
                word += stride;
                if word >= self.words.len() {
                    word -= self.words.len();
                }
            }
        }
        for damage in &self.input.damage {
            let i = w.index(damage.position).unwrap();
            let word = i / 8 % self.words.len();
            let shift = (i % 8) * 4;
            self.words[word] = (self.words[word] & !(15 << shift))
                | (surface_tint(damage.material, damage.work, damage.cost) << shift);
        }
    }
}
/// Exact physical-word union with storage bounded by the volume, even when
/// moving shadows revisit the same words for hundreds of ticks.
struct PatchUnion {
    bits: Vec<u64>,
    words: usize,
}
impl PatchUnion {
    fn new(words: usize) -> Self {
        Self {
            bits: vec![0; words.div_ceil(64)],
            words,
        }
    }
    fn insert(&mut self, span: std::ops::Range<usize>) {
        assert!(span.start <= span.end && span.end <= self.words);
        if span.is_empty() {
            return;
        }
        let first = span.start / 64;
        let last = (span.end - 1) / 64;
        let head = u64::MAX << (span.start % 64);
        let tail = u64::MAX >> (63 - (span.end - 1) % 64);
        if first == last {
            self.bits[first] |= head & tail;
        } else {
            self.bits[first] |= head;
            self.bits[first + 1..last].fill(u64::MAX);
            self.bits[last] |= tail;
        }
    }
    fn ranges(&self) -> Vec<std::ops::Range<usize>> {
        let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
        for (block, &bits) in self.bits.iter().enumerate() {
            let mut bits = bits;
            while bits != 0 {
                let lo = bits.trailing_zeros() as usize;
                let hi = lo + (bits >> lo).trailing_ones() as usize;
                let start = block * 64 + lo;
                let end = block * 64 + hi;
                if let Some(previous) = ranges.last_mut().filter(|r| r.end == start) {
                    previous.end = end;
                } else {
                    ranges.push(start..end);
                }
                bits = if hi == 64 { 0 } else { bits & (u64::MAX << hi) };
            }
        }
        ranges
    }
}
impl Job {
    fn observe(&mut self, sim: &BlastRange, dirty: &[std::ops::Range<usize>]) {
        if self.row > self.window.origin[2] {
            self.changes
                .update_padded(
                    sim,
                    self.generation,
                    self.window.span(0),
                    self.window.origin[2]..self.row,
                    self.window.dims[2] as usize,
                )
                .expect("completed rows respect their padded window and row budget");
            self.repairs.extend(self.changes.rows.iter().copied());
        }
        // Include changes to the old visible GPU volume, including shadows.
        for span in dirty {
            self.aliases.insert(span.clone());
        }
    }
    fn step(&mut self, sim: &BlastRange, previous: &[u32]) -> bool {
        self.step_in(
            sim,
            previous,
            bevy::tasks::ComputeTaskPool::try_get().map(|p| &**p),
        )
    }
    fn step_in(
        &mut self,
        sim: &BlastRange,
        previous: &[u32],
        pool: Option<&bevy::tasks::TaskPool>,
    ) -> bool {
        let w = self.window;
        let costs = Header::of(sim, self.generation).costs;
        if self.costs.is_some_and(|old| old != costs) {
            self.repairs.extend(w.origin[2]..self.row);
        }
        self.costs = Some(costs);
        let end = (self.row + ROWS_PER_FRAME).min(w.span(2).end);
        let mut selected: Vec<_> = (self.row..end).collect();
        for z in &selected {
            self.repairs.remove(z);
        }
        self.row = end;
        while selected.len() < ROWS_PER_FRAME as usize {
            let Some(z) = self.repairs.pop_first() else {
                break;
            };
            selected.push(z);
        }
        self.rows
            .resize_with(self.rows.len().max(selected.len()), Row::default);
        let rows = &mut self.rows[..selected.len()];
        for (row, z) in rows.iter_mut().zip(selected) {
            row.z = z;
        }
        let capture =
            |row: &mut Row| capture_row(&mut row.input, sim, self.generation, w, w.span(0), row.z);
        if let Some(pool) = pool.filter(|p| p.thread_num() > 1 && rows.len() > 1) {
            pool.scope(|scope| {
                for row in rows.iter_mut() {
                    scope.spawn(async move { capture(row) });
                }
            });
        } else {
            for row in rows.iter_mut() {
                capture(row);
            }
        }
        if let Some(pool) = pool.filter(|p| p.thread_num() > 1 && rows.len() > 1) {
            pool.scope(|scope| {
                for row in rows.iter_mut() {
                    scope.spawn(async move { row.read(w) });
                }
            });
        } else {
            for row in rows.iter_mut() {
                row.read(w);
            }
        }
        let slab = w.dims[0] as usize * w.dims[1] as usize / 8;
        for row in rows {
            let start = row.z as usize % w.dims[2] as usize * slab;
            self.words[start..start + slab].copy_from_slice(&row.words);
            let top_start = row.z as usize % w.dims[2] as usize * w.dims[0] as usize;
            self.tops[top_start..top_start + w.dims[0] as usize].copy_from_slice(&row.tops);
            self.highest = self.highest.max(row.highest);
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
        // A heavily edited row may need repeated repairs. Keep their union,
        // never an unbounded history of repeated upload ranges.
        for span in self.dirty.drain(..) {
            self.aliases.insert(span);
        }
        end == w.span(2).end && self.repairs.is_empty()
    }
}

/// The publication scope finishes before detached rows are packed. At most
/// eight full-width rows' columns are resident. Narrow entering strips can
/// group more Z rows without increasing that material/column budget.
fn capture_row(
    row: &mut TerrainRow,
    sim: &BlastRange,
    generation: u64,
    window: TerrainWindow,
    xs: std::ops::Range<i32>,
    z: i32,
) {
    row.update_padded(
        sim,
        generation,
        Request {
            x: xs,
            y: window.span(1),
            z,
        },
        Budget {
            columns: window.dims[0] as usize,
            cells: window.dims[0] as usize * window.dims[1] as usize,
        },
    )
    .expect("terrain requests respect the nonnegative window and column budget");
}

pub(super) fn write_slab(
    sim: &BlastRange,
    generation: u64,
    window: TerrainWindow,
    words: &mut [u32],
    tops: &mut [u16],
    xs: std::ops::Range<i32>,
    zs: std::ops::Range<i32>,
    incremental: bool,
    pool: Option<&bevy::tasks::TaskPool>,
) -> (i32, Vec<std::ops::Range<usize>>) {
    let dx = window.dims[0] as usize;
    let slab = dx * window.dims[1] as usize / 8;
    let dz = window.dims[2] as usize;
    let pool = pool.filter(|p| p.thread_num() > 1 && xs.len() * zs.len() >= 512);
    if xs.is_empty() || zs.is_empty() {
        return (0, Vec::new());
    }
    let admitted_rows = (ROWS_PER_FRAME as usize * dx / xs.len())
        .min(zs.len())
        .max(1);
    let mut inputs: Vec<TerrainRow> = (0..admitted_rows).map(|_| TerrainRow::default()).collect();
    let mut highest = 0;
    let mut dirty = Vec::new();
    let mut z = zs.start;
    while z < zs.end {
        let base = z as usize % dz;
        let end = zs
            .end
            .min(z + (dz - base) as i32)
            .min(z + admitted_rows as i32);
        let n = (end - z) as usize;
        let inputs = &mut inputs[..n];
        let capture = |i: usize, row: &mut TerrainRow| {
            capture_row(row, sim, generation, window, xs.clone(), z + i as i32)
        };
        if let Some(pool) = pool.filter(|_| n > 1) {
            let chunk = n.div_ceil(pool.thread_num() * 2).max(1);
            pool.scope(|scope| {
                for (i, rows) in inputs.chunks_mut(chunk).enumerate() {
                    scope.spawn(async move {
                        for (j, row) in rows.iter_mut().enumerate() {
                            capture(i * chunk + j, row);
                        }
                    });
                }
            });
        } else {
            for (i, row) in inputs.iter_mut().enumerate() {
                capture(i, row);
            }
        }
        let words = &mut words[base * slab..(base + n) * slab];
        let tops = &mut tops[base * dx..(base + n) * dx];
        let results = if let Some(pool) = pool.filter(|_| n > 1) {
            let chunk = n.div_ceil(pool.thread_num() * 2).max(1);
            pool.scope(|scope| {
                for (i, ((inputs, words), tops)) in inputs
                    .chunks(chunk)
                    .zip(words.chunks_mut(chunk * slab))
                    .zip(tops.chunks_mut(chunk * dx))
                    .enumerate()
                {
                    scope.spawn(async move {
                        write_rows(inputs, window, words, tops, base + i * chunk, incremental)
                    });
                }
            })
        } else {
            vec![write_rows(inputs, window, words, tops, base, incremental)]
        };
        for (top, patches) in results {
            highest = highest.max(top);
            dirty.extend(patches);
        }
        z = end;
    }
    (highest, dirty)
}

fn write_rows(
    inputs: &[TerrainRow],
    window: TerrainWindow,
    words: &mut [u32],
    tops: &mut [u16],
    base_z: usize,
    incremental: bool,
) -> (i32, Vec<std::ops::Range<usize>>) {
    let mut highest = 0;
    let mut dirty: Vec<std::ops::Range<usize>> = Vec::new();
    let mut before: Vec<(usize, u32)> = Vec::new();

    // Remember only rows that can change. Sky already empty in both the
    // old and new slab needs no write, allocation or GPU upload.
    assert!(window.dims[0].is_multiple_of(8));

    let dx = window.dims[0] as usize;
    let dy = window.dims[1] as usize;
    let row_words = dx / 8;
    let slab_words = row_words * dy;
    for input in inputs {
        let request = input.request.as_ref().unwrap();
        let xs = request.x.clone();
        let z = request.z;
        assert!(xs.start % 8 == 0 && xs.end % 8 == 0);
        let z_words = (z as usize % window.dims[2] as usize - base_z) * slab_words;
        let next_top = xs
            .clone()
            .map(|x| input.column(x).unwrap().0.top as i32)
            .max()
            .unwrap_or(window.origin[1]);
        // `terrain_origin.w` is a conservative ceiling for every old
        // occupied cell. Newly exposed columns can only extend it to
        // `next_top`; scanning the remaining empty sky on every eight-
        // voxel camera nudge wasted most of this loop's work.
        let slot = |x: i32| x as usize % dx + dx * (z as usize % window.dims[2] as usize - base_z);
        let old_top = xs
            .clone()
            .map(|x| u32::from(tops[slot(x)]))
            .max()
            .unwrap_or(0);
        let scan_end = (old_top as i32).max(next_top).min(window.span(1).end);
        for y in window.origin[1]..scan_end {
            let mut x = xs.start;
            while x < xs.end {
                let wrapped_x = x as usize % dx;
                let n = (xs.end - x).min((dx - wrapped_x) as i32);
                let i = z_words + y as usize % dy * row_words + wrapped_x / 8;
                let span = i..i + n as usize / 8;
                if !incremental || y < next_top || words[span.clone()].iter().any(|w| *w != 0) {
                    if incremental {
                        before.extend(
                            words[span.clone()]
                                .iter()
                                .enumerate()
                                .map(|(offset, &word)| (span.start + offset, word)),
                        );
                    }
                    words[span].fill(0);
                }
                x += n;
            }
        }
        for x in xs.clone() {
            let (column, materials) = input.column(x).unwrap();
            let top = column.top as i32;
            highest = (highest).max(top);
            tops[slot(x)] = top.min(window.span(1).end) as u16;
            let wrapped_x = x as usize % dx;
            let shift = (wrapped_x % 8) * 4;
            let base = z_words + wrapped_x / 8;
            let mut word = base + window.origin[1] as usize % dy * row_words;
            for &material in materials {
                // Entering rows were cleared above. A column has a fixed
                // nibble and advances by one packed row, not three modulo
                // operations plus a bounds query for every voxel.
                words[word] |= surface_tint(material, 0, 1) << shift;
                word += row_words;
                if word >= base + slab_words {
                    word -= slab_words;
                }
            }
        }
        for damage in &input.damage {
            let i = window.index(damage.position).unwrap();
            let word = i / 8 - base_z * slab_words;
            let shift = (i % 8) * 4;
            words[word] = (words[word] & !(15 << shift))
                | (surface_tint(damage.material, damage.work, damage.cost) << shift);
        }
        // A toroidal slot often receives exactly the material it already
        // held. Publish actual differences, including old solids becoming
        // air, instead of every row in the entering slab.
        for (i, old) in before.drain(..) {
            if words[i] == old {
                continue;
            }
            if let Some(last) = dirty.last_mut() {
                if last.end == base_z * slab_words + i {
                    last.end += 1;
                    continue;
                }
            }
            dirty.push(base_z * slab_words + i..base_z * slab_words + i + 1);
        }
    }
    (highest, dirty)
}

pub fn camera(
    scene: Res<Scene>,
    mut camera: Query<(&mut Transform, &mut Projection, &Camera), With<GasCamera>>,
    mut gas: ResMut<GasInput>,
    mut visual: ResMut<moving::Visual>,
    mut stream: ResMut<Streaming>,
    mut profile: ResMut<profile::Profile>,
) {
    let Ok((mut transform, mut projection, camera)) = camera.single_mut() else {
        return;
    };
    let started = profile.camera_tour.then(std::time::Instant::now);
    let mut work = profile::CameraWork {
        enabled: started.is_some(),
        kind: "idle",
        ..default()
    };
    let aspect = camera
        .logical_viewport_size()
        .map_or(16.0 / 9.0, |v| v.x / v.y.max(1.0));
    let local_limit = window_height_limit(-*transform.forward(), aspect, scene.sim.size());
    // At map scale leave the detailed window in place. The same raytracer
    // continues into compact world columns outside it; no picture replacement.
    if matches!(
        *projection,
            Projection::Orthographic(ref p)
                if matches!(p.scaling_mode, bevy::camera::ScalingMode::FixedVertical { viewport_height } if map_scale(scene.sim.size(), viewport_height, local_limit, stream.map_scale))
    ) {
        work.measure(3, || {
            if let Some(job) = stream.job.take() {
                stream.spare = job.words;
            }
        });
        work.kind = "map";
        stream.map_scale = true;
        stream.displayed = None;
        gas.params.terrain.w = 0.0;
        if let Some(start) = started {
            profile.camera_details(work);
            profile.measure_camera(1, start.elapsed());
        }
        return;
    }
    stream.map_scale = false;
    gas.params.terrain.w = 0.0;
    let ray = transform.forward();
    let distance = (scene.sim.ground_level() as f32 - transform.translation.y) / ray.y.min(-0.01);
    let p = if matches!(*projection, Projection::Perspective(_)) {
        transform.translation
    } else {
        transform.translation + ray * distance
    };
    let want = TerrainWindow::around(
        scene.sim.size(),
        [p.x as i32, scene.sim.ground_level(), p.z as i32],
        TERRAIN_WINDOW,
        WINDOW_STEP,
    );
    let small = want.dims == gas.window.dims
        && (0..3).all(|a| (want.origin[a] - gas.window.origin[a]).abs() <= WINDOW_STEP * 4);
    if stream
        .job
        .as_ref()
        .is_some_and(|j| j.generation != scene.generation)
    {
        work.measure(3, || stream.spare = stream.job.take().unwrap().words);
    }
    if !stream.invalid && stream.job.is_none() && small {
        if want != gas.window {
            let had = gas.window;
            work.kind = "shift";
            work.measure(6, || {
                write_window_for(&scene.sim, scene.generation, &mut gas, want, Some(had))
            });
            work.measure(4, || moving::occupancy(&scene.sim, &mut gas, &mut visual));
            work.measure(5, || gas.advance(scene.sim.tick * 2));
        }
        stream.displayed = Some((*transform, projection.clone()));
        if let Some(start) = started {
            profile.camera_details(work);
            profile.measure_camera(1, start.elapsed());
        }
        return;
    }
    work.kind = "staging";
    if stream.job.is_none() {
        work.measure(0, || {
            let mut words = std::mem::take(&mut stream.spare);
            if words.len() != want.words() {
                words = vec![0; want.words()];
            }
            stream.job = Some(Job {
                window: want,
                generation: scene.generation,
                row: want.origin[2],
                words,
                tops: vec![0; (want.dims[0] * want.dims[2]) as usize],
                highest: 0,
                dirty: Vec::new(),
                repairs: default(),
                aliases: PatchUnion::new(want.words()),
                rows: default(),
                changes: default(),
                costs: None,
            });
        });
    }
    if work.measure(1, || {
        stream
            .job
            .as_mut()
            .unwrap()
            .step(&scene.sim, &gas.terrain_words)
    }) {
        work.kind = "publish";
        let job = stream.job.take().unwrap();
        let want = job.window;
        work.measure(2, || {
            let old = std::mem::replace(&mut gas.terrain_words, Arc::new(job.words));
            if let Ok(words) = Arc::try_unwrap(old) {
                stream.spare = words;
            }
            gas.window = want;
            gas.terrain_column_top = job.tops;
            gas.terrain_dirty.extend(job.dirty);
            gas.terrain_dirty.extend(job.aliases.ranges());
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
        });
        work.measure(3, || {
            drop(job.aliases);
            drop(job.rows);
            moving::replace_terrain(&mut visual);
        });
        work.measure(4, || moving::occupancy(&scene.sim, &mut gas, &mut visual));
        work.measure(5, || gas.advance(scene.sim.tick * 2));
        stream.invalid = false;
        stream.displayed = Some((*transform, projection.clone()));
    } else if scene.sim.size()[0] <= TERRAIN_WINDOW[0] && scene.sim.size()[2] <= TERRAIN_WINDOW[2] {
        if let Some((t, p)) = &stream.displayed {
            *transform = *t;
            *projection = p.clone();
        }
    }
    if let Some(start) = started {
        profile.camera_details(work);
        profile.measure_camera(1, start.elapsed());
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
        tops: vec![0; (window.dims[0] * window.dims[2]) as usize],
        highest: 0,
        dirty: Vec::new(),
        repairs: default(),
        aliases: PatchUnion::new(window.words()),
        rows: default(),
        changes: default(),
        costs: None,
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
        job.aliases.ranges().len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(test)]
    #[test]
    fn detached_row_packing_matches_direct_world_after_source_mutation() {
        let mut sim = BlastRange::new(config(false, false));
        let window = TerrainWindow {
            origin: [64, 8, 80],
            dims: [64, 64, 64],
        };
        sim.world.queue_edit(voxel::world::Edit::put(72, 24, 82, 3));
        sim.world
            .queue_edit(voxel::world::Edit::put(73, 24, 82, 10));
        sim.world.flush();
        sim.surface_work.insert([72, 24, 82], 380);
        let mut expected = vec![0; (window.dims[0] * window.dims[1] / 8) as usize];
        for x in window.span(0) {
            for y in window.span(1) {
                let p = [x, y, 82];
                let material = sim_core::blast::material(&sim.world, p);
                let i = window.index(p).unwrap();
                let word = i / 8 % expected.len();
                expected[word] |= surface_material(&sim, p, material) << ((i % 8) * 4);
            }
        }
        let mut row = Row { z: 82, ..default() };
        capture_row(&mut row.input, &sim, 7, window, window.span(0), 82);
        let captured = row.input.clone();
        sim.world.queue_dig(72, 24, 82);
        sim.world.flush();
        sim.surface_work.clear();
        sim.config.steel_resistance *= 2;
        row.read(window);
        assert_eq!(row.words, expected);
        assert_eq!(row.input, captured);
        assert_eq!(row.input.header.generation, 7);
        assert_eq!(
            surface_tint(3, u32::MAX, 1),
            10,
            "saturated work cannot overflow presentation"
        );
    }

    #[cfg(test)]
    #[test]
    fn staged_cost_change_repairs_completed_rows_and_ignores_old_generation_observation() {
        let mut sim = BlastRange::new(config(false, false));
        let window = TerrainWindow {
            origin: [64, 8, 80],
            dims: [64, 64, 64],
        };
        sim.world.queue_edit(voxel::world::Edit::put(72, 24, 82, 3));
        sim.world.flush();
        let work = sim_core::blast::material_work(3, &sim.config) / 2;
        sim.surface_work.insert([72, 24, 82], work);
        let previous = vec![0; window.words()];
        let mut job = Job {
            window,
            generation: 7,
            row: 80,
            words: previous.clone(),
            tops: vec![0; 4096],
            highest: 0,
            dirty: default(),
            repairs: default(),
            aliases: PatchUnion::new(window.words()),
            rows: default(),
            changes: default(),
            costs: None,
        };
        assert!(!job.step_in(&sim, &previous, None));
        let i = window.index([72, 24, 82]).unwrap();
        let before = (job.words[i / 8] >> ((i % 8) * 4)) & 15;
        sim.config.steel_resistance *= 2;
        let mut steps = 0;
        while !job.step_in(&sim, &previous, None) {
            steps += 1;
            assert!(steps < 20);
        }
        let after = (job.words[i / 8] >> ((i % 8) * 4)) & 15;
        assert_ne!(
            before, after,
            "cost changes on pause refresh already captured scars"
        );
        assert_eq!(after, surface_material(&sim, [72, 24, 82], 3));
        assert!(job.rows.len() <= ROWS_PER_FRAME as usize);
        let mut stream = Streaming {
            job: Some(job),
            ..default()
        };
        sim.world.queue_dig(72, 24, 82);
        sim.world.flush();
        stream.observe(&sim, 8, &[]);
        assert!(stream.job.as_ref().unwrap().repairs.is_empty());
        stream.observe(&sim, 7, &[]);
        assert!(stream.job.as_ref().unwrap().repairs.contains(&82));
    }

    #[test]
    fn patch_union_matches_dense_reference_for_repeated_wrapped_ranges() {
        for words in [1, 63, 64, 65, 129, 4093] {
            let mut union = PatchUnion::new(words);
            let mut reference = vec![false; words];
            let mut seed = 17u32;
            for pass in 0..1000 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let a = seed as usize % (words + 1);
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let b = seed as usize % (words + 1);
                let span = a.min(b)..a.max(b);
                union.insert(span.clone());
                reference[span].fill(true);
                if pass % 37 == 0 {
                    let mut actual = vec![false; words];
                    let ranges = union.ranges();
                    assert!(ranges.windows(2).all(|r| r[0].end < r[1].start));
                    for span in ranges {
                        actual[span].fill(true);
                    }
                    assert_eq!(actual, reference);
                }
            }
            assert_eq!(union.bits.len(), words.div_ceil(64));
        }
    }

    #[test]
    fn staged_parallel_rows_replay_live_edits_and_old_shadow_uploads() {
        for workers in [1, 2, 4] {
            let pool = bevy::tasks::TaskPoolBuilder::new()
                .num_threads(workers)
                .build();
            let mut sim = BlastRange::new(config(false, false));
            let window = TerrainWindow {
                origin: [64, 8, 80],
                dims: [64, 64, 64],
            };
            let mut previous = vec![0; window.words()];
            let mut job = Job {
                window,
                generation: 0,
                row: window.origin[2],
                words: previous.clone(),
                tops: vec![0; (window.dims[0] * window.dims[2]) as usize],
                highest: 0,
                dirty: default(),
                repairs: default(),
                aliases: PatchUnion::new(window.words()),
                rows: default(),
                changes: default(),
                costs: None,
            };
            assert!(!job.step_in(&sim, &previous, Some(&pool)));
            // Both additions/removals and a cosmetic-only surface change on
            // a completed row must replay. Future/outside rows must not queue.
            sim.world.queue_edit(voxel::world::Edit::put(72, 24, 82, 3));
            sim.world.queue_dig(75, 8, 83);
            sim.world
                .queue_edit(voxel::world::Edit::put(96, 22, 110, 2));
            sim.world.queue_dig(300, 8, 300);
            sim.world.flush();
            sim.surface_work.insert([72, 24, 82], 380);
            sim.surface_changed.push([72, 24, 82]);
            // A unit changed the old visible volume *after* its sky word
            // matched this staged row. The new GPU window must clear it.
            let shadow = window.index([100, 50, 82]).unwrap() / 8;
            previous[shadow] = u32::MAX;
            job.observe(
                &sim,
                &[shadow..shadow + 1, shadow - 1..shadow, shadow..shadow + 2],
            );
            assert_eq!(job.repairs.iter().copied().collect::<Vec<_>>(), [82, 83]);
            assert_eq!(
                job.aliases
                    .ranges()
                    .into_iter()
                    .find(|r| r.contains(&shadow))
                    .map(|r| (r.start, r.end)),
                Some((shadow - 1, shadow + 2)),
                "late overlapping shadow patches form one range"
            );
            let mut steps = 1;
            while !job.step_in(&sim, &previous, Some(&pool)) {
                steps += 1;
                assert!(steps < 20, "repairs cannot prevent completion");
            }
            // Changes after the initial scan finished still replay, without
            // rebuilding the full volume or running unbounded repair work.
            sim.world.queue_dig(72, 24, 82);
            sim.world.flush();
            job.observe(&sim, &[]);
            assert!(!job.repairs.is_empty());
            assert!(job.step_in(&sim, &previous, Some(&pool)));
            let mut direct = GasInput::default();
            direct.terrain_words = Arc::new(vec![0; window.words()]);
            write_window(&sim, &mut direct, window, None);
            assert!(moving::cells(&sim, &window).is_empty());
            assert_eq!(
                &job.words,
                direct.terrain_words.as_ref(),
                "{workers} workers"
            );
            assert_eq!(job.tops, direct.terrain_column_top);
            for span in job.dirty.into_iter().chain(job.aliases.ranges()) {
                previous[span.clone()].copy_from_slice(&job.words[span]);
            }
            assert_eq!(
                previous, job.words,
                "GPU patches include late old occupancy at {workers} workers"
            );
        }
    }

    #[test]
    fn map_scale_hysteresis_only_suspends_streaming_on_large_maps() {
        let size = [10_000, 192, 10_000];
        assert!(!map_scale([640, 192, 640], 10_000.0, 600.0, false));
        assert!(!map_scale(size, 659.0, 600.0, false));
        assert!(map_scale(size, 661.0, 600.0, false));
        assert!(map_scale(size, 630.0, 600.0, true));
        assert!(!map_scale(size, 600.0, 600.0, true));
    }
    #[test]
    fn default_sector_fit_returns_to_detail_at_any_aspect_and_orientation() {
        let size = [10_000, 192, 10_000];
        for aspect in [1332.0 / 1022.0, 16.0 / 9.0, 9.0 / 16.0] {
            for orbit in [0.0, 1.2, 2.6, 4.8] {
                for pitch in [-0.3, 0.0, 0.3] {
                    let offset = orbit_offset(Vec3::new(-155.0, 420.0, 320.0), orbit, pitch);
                    let local_limit = window_height_limit(offset, aspect, size);
                    let direction_limit = window_height_limit(offset.normalize(), aspect, size);
                    assert!((direction_limit - local_limit).abs() < 0.001);
                    for was_map in [false, true] {
                        assert!(
                            !map_scale(size, local_limit, direction_limit, was_map),
                            "default sector suspended detail: {aspect} {orbit} {pitch} {was_map}"
                        );
                    }
                    assert!(map_scale(size, local_limit * 1.2, direction_limit, false));
                }
            }
        }
    }
    #[test]
    fn staged_rows_match_whole_window_after_wrapping_and_damage() {
        let mut sim = BlastRange::new(config(false, false));
        for p in [[72, 8, 96], [91, 11, 110], [122, 12, 100]] {
            sim.world.queue_dig(p[0], p[1], p[2]);
        }
        sim.world
            .queue_edit(voxel::world::Edit::put(104, 20, 96, 3));
        sim.surface_work.insert([104, 20, 96], 380);
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
            tops: vec![0; (window.dims[0] * window.dims[2]) as usize],
            highest: 0,
            dirty: Vec::new(),
            repairs: default(),
            aliases: PatchUnion::new(window.words()),
            rows: default(),
            changes: default(),
            costs: None,
        };
        let mut steps = 1;
        while !job.step(&sim, &before) {
            steps += 1;
        }
        assert_eq!(steps, 8);
        assert_eq!(&job.words, direct.terrain_words.as_ref());
        for workers in [1, 2, 4, 8] {
            let pool = bevy::tasks::TaskPoolBuilder::new()
                .num_threads(workers)
                .build();
            for origin in [
                [0, 0, 0],
                [56, 0, 72],
                [72, 0, 88],
                [64, 0, 72],
                [56, 0, 80],
            ] {
                let mut streamed = GasInput::default();
                streamed.terrain_words = Arc::new(vec![0; window.words()]);
                let previous = TerrainWindow {
                    origin,
                    dims: window.dims,
                };
                write_window(&sim, &mut streamed, previous, None);
                let mut patched = streamed.terrain_words.as_ref().clone();
                streamed.terrain_dirty.clear();
                write_window_in(&sim, &mut streamed, window, Some(previous), Some(&pool));
                assert_eq!(streamed.terrain_words, direct.terrain_words);
                assert_eq!(streamed.terrain_column_top, direct.terrain_column_top);
                for r in &streamed.terrain_dirty {
                    patched[r.clone()].copy_from_slice(&streamed.terrain_words[r.clone()]);
                }
                assert_eq!(
                    patched, job.words,
                    "streamed dents must be included in upload patches"
                );
            }
        }
        let mut uploaded = before;
        for r in job.aliases.ranges() {
            uploaded[r.clone()].copy_from_slice(&job.words[r]);
        }
        assert_eq!(
            uploaded, job.words,
            "including previously solid cells becoming air"
        );
    }
}
