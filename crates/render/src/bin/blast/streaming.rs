//! Large camera jumps fill a staging window over several frames. The visible
//! camera/window switch together. Camera streaming never pauses physics;
//! staged rows replay terrain changes before publication. Scene resets still
//! hold their loading boundary until the new generation is ready.
use super::*;

const ROWS_PER_FRAME: i32 = 8;

pub(super) fn map_scale(size: [u32; 3], height: f32, was_map_scale: bool) -> bool {
    if size[0] <= TERRAIN_WINDOW[0] && size[2] <= TERRAIN_WINDOW[2] {
        return false;
    }
    // A scroll wheel can hover on the boundary for several frames. Keep the
    // detail-streaming policy until the camera clears a return threshold.
    if was_map_scale {
        height > 590.0
    } else {
        height >= 675.0
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
    pub fn observe(&mut self, sim: &BlastRange, dirty: &[std::ops::Range<usize>]) {
        if let Some(job) = &mut self.job {
            job.observe(sim, dirty);
        }
    }
}
#[derive(Default)]
struct Row {
    z: i32,
    words: Vec<u32>,
    tops: Vec<u16>,
    highest: i32,
}
impl Row {
    fn read(&mut self, sim: &BlastRange, w: TerrainWindow) {
        let dx = w.dims[0] as usize;
        let dy = w.dims[1] as usize;
        let stride = dx / 8;
        self.words.resize(stride * dy, 0);
        self.words.fill(0);
        self.tops.resize(dx, 0);
        self.highest = 0;
        let mut column = vec![0; dy];
        for x in w.span(0) {
            let top = sim.world.column_ceiling(x as u32, self.z as u32) as i32;
            self.highest = self.highest.max(top);
            self.tops[x as usize % dx] = top.min(w.span(1).end) as u16;
            let n = (top.min(w.span(1).end) - w.origin[1]).max(0) as usize;
            sim.world.column(
                x as u32,
                self.z as u32,
                w.origin[1] as u32,
                &mut column[..n],
            );
            let wrapped_x = x as usize % dx;
            let shift = (wrapped_x % 8) * 4;
            let mut word = wrapped_x / 8 + w.origin[1] as usize % dy * stride;
            for &material in &column[..n] {
                self.words[word] |= u32::from(material) << shift;
                word += stride;
                if word >= self.words.len() {
                    word -= self.words.len();
                }
            }
        }
        // Raw columns avoid a tree lookup at every intact voxel. Only actual
        // surface work needs the presentation material substitution.
        for (p, _) in sim
            .surface_work
            .range([w.origin[0], i32::MIN, i32::MIN]..[w.span(0).end, i32::MIN, i32::MIN])
        {
            if p[2] != self.z {
                continue;
            }
            let Some(i) = w.index(*p) else {
                continue;
            };
            let word = i / 8 % self.words.len();
            let shift = (i % 8) * 4;
            let material = ((self.words[word] >> shift) & 15) as u16;
            self.words[word] = (self.words[word] & !(15 << shift))
                | (surface_material(sim, *p, material) << shift);
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
        let w = self.window;
        let mut changed = |x: i32, z: i32| {
            if w.span(0).contains(&x) && (w.origin[2]..self.row).contains(&z) {
                self.repairs.insert(z);
            }
        };
        for edit in sim.world.edits() {
            changed(edit.x as i32, edit.z as i32);
        }
        for p in &sim.surface_changed {
            changed(p[0], p[2]);
        }
        // The old visible toroidal volume can change after a row was compared
        // with it (moving unit shadows included). Retain those physical word
        // ranges too; otherwise a correct staged CPU volume could publish an
        // incomplete GPU patch and leave old occupancy in the new window.
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
        if let Some(pool) = pool.filter(|p| p.thread_num() > 1 && rows.len() > 1) {
            pool.scope(|scope| {
                for row in rows.iter_mut() {
                    scope.spawn(async move { row.read(sim, w) });
                }
            });
        } else {
            for row in rows.iter_mut() {
                row.read(sim, w);
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

/// Read entering strips into disjoint physical Z rows. The world is immutable
/// for the scope; neither a full volume copy nor a staging publication is needed.
pub(super) fn write_slab(
    sim: &BlastRange,
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
    let rows = zs
        .len()
        .div_ceil(pool.map_or(1, |p| p.thread_num() * 2))
        .max(1);
    let mut highest = 0;
    let mut dirty = Vec::new();
    // Split at the toroidal wrap before borrowing row slices. Replaying scope
    // results in spawn order also keeps upload patches in world row order.
    let mut z = zs.start;
    while z < zs.end {
        let base = z as usize % dz;
        let end = zs.end.min(z + (dz - base) as i32);
        let n = (end - z) as usize;
        let words = &mut words[base * slab..(base + n) * slab];
        let tops = &mut tops[base * dx..(base + n) * dx];
        let results = if let Some(pool) = pool {
            pool.scope(|scope| {
                for (i, (words, tops)) in words
                    .chunks_mut(rows * slab)
                    .zip(tops.chunks_mut(rows * dx))
                    .enumerate()
                {
                    let first = z + (i * rows) as i32;
                    let last = end.min(first + rows as i32);
                    let xs = xs.clone();
                    scope.spawn(async move {
                        write_rows(
                            sim,
                            window,
                            words,
                            tops,
                            xs,
                            first..last,
                            base + i * rows,
                            incremental,
                        )
                    });
                }
            })
        } else {
            vec![write_rows(
                sim,
                window,
                words,
                tops,
                xs.clone(),
                z..end,
                base,
                incremental,
            )]
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
    sim: &BlastRange,
    window: TerrainWindow,
    words: &mut [u32],
    tops: &mut [u16],
    xs: std::ops::Range<i32>,
    zs: std::ops::Range<i32>,
    base_z: usize,
    incremental: bool,
) -> (i32, Vec<std::ops::Range<usize>>) {
    let mut highest = 0;
    let mut dirty: Vec<std::ops::Range<usize>> = Vec::new();
    let mut before: Vec<(usize, u32)> = Vec::new();
    let mut column = vec![0u16; window.dims[1] as usize];
    // Remember only rows that can change. Sky already empty in both the
    // old and new slab needs no write, allocation or GPU upload.
    assert!(window.dims[0].is_multiple_of(8));
    assert!(xs.start % 8 == 0 && xs.end % 8 == 0);
    let dx = window.dims[0] as usize;
    let dy = window.dims[1] as usize;
    let row_words = dx / 8;
    let slab_words = row_words * dy;
    for z in zs.clone() {
        let z_words = (z as usize % window.dims[2] as usize - base_z) * slab_words;
        let next_top = xs
            .clone()
            .map(|x| sim.world.column_ceiling(x as u32, z as u32) as i32)
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
            let top = sim.world.column_ceiling(x as u32, z as u32) as i32;
            highest = (highest).max(top);
            tops[slot(x)] = top.min(window.span(1).end) as u16;
            let count = (top.min(window.span(1).end) - window.origin[1]).max(0) as usize;
            sim.world.column(
                x as u32,
                z as u32,
                window.origin[1] as u32,
                &mut column[..count],
            );
            let wrapped_x = x as usize % dx;
            let shift = (wrapped_x % 8) * 4;
            let base = z_words + wrapped_x / 8;
            let mut word = base + window.origin[1] as usize % dy * row_words;
            for &material in &column[..count] {
                // Entering rows were cleared above. A column has a fixed
                // nibble and advances by one packed row, not three modulo
                // operations plus a bounds query for every voxel.
                words[word] |= u32::from(material) << shift;
                word += row_words;
                if word >= base + slab_words {
                    word -= slab_words;
                }
            }
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
    mut camera: Query<(&mut Transform, &mut Projection), With<GasCamera>>,
    mut gas: ResMut<GasInput>,
    mut visual: ResMut<moving::Visual>,
    mut stream: ResMut<Streaming>,
    mut profile: ResMut<profile::Profile>,
) {
    let Ok((mut transform, mut projection)) = camera.single_mut() else {
        return;
    };
    let started = profile.camera_tour.then(std::time::Instant::now);
    let mut work = profile::CameraWork {
        enabled: started.is_some(),
        kind: "idle",
        ..default()
    };
    // At map scale leave the detailed window in place. The same raytracer
    // continues into compact world columns outside it; no picture replacement.
    if matches!(
        *projection,
            Projection::Orthographic(ref p)
                if matches!(p.scaling_mode, bevy::camera::ScalingMode::FixedVertical { viewport_height } if map_scale(scene.sim.size(), viewport_height, stream.map_scale))
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
        .is_some_and(|j| j.generation != scene.generation)
    {
        work.measure(3, || stream.spare = stream.job.take().unwrap().words);
    }
    if !stream.invalid && stream.job.is_none() && small {
        if want != gas.window {
            let had = gas.window;
            work.kind = "shift";
            work.measure(6, || write_window(&scene.sim, &mut gas, want, Some(had)));
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
        assert!(!map_scale([640, 192, 640], 10_000.0, false));
        assert!(!map_scale([10_000, 192, 10_000], 674.0, false));
        assert!(map_scale([10_000, 192, 10_000], 675.0, false));
        assert!(map_scale([10_000, 192, 10_000], 600.0, true));
        assert!(!map_scale([10_000, 192, 10_000], 590.0, true));
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
