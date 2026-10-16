//! Frame timings without screenshots or PNG encoding. Optional GPU timestamps
//! read back timing queries only, never terrain, pixels or simulation fields.
use super::*;
use std::{io::Write, time::Instant};
#[path = "profile/core.rs"]
mod core;
pub use self::core::render_sample;
#[path = "profile/camera.rs"]
mod camera;
pub(crate) use camera::Work as CameraWork;

#[derive(Resource, Default)]
pub struct Profile {
    pub output: Option<String>,
    pub seconds: f64,
    pub clouds: usize,
    pub gpu: bool,
    pub actions: bool,
    pub camera_tour: bool,
    camera_frames: Vec<camera::Frame>,
    camera_work: [f64; 2],
    camera_detail: CameraWork,
    pub core: bool,
    trigger: Option<core::Trigger>,
    /// Optional fixed route pool for paired native performance experiments.
    /// Reapply when the workshop replaces the simulation, not every frame.
    pub navigation_workers: Option<usize>,
    /// The scene's own pool; the global Rayon environment does not set it.
    pub physics_workers: Option<usize>,
    action_step: u8,
    action_time: f64,
    gpu_samples: render::gpu_profile::Capture,
    emitted: bool,
    warm: Option<f64>,
    start: Option<f64>,
    mark: Option<Instant>,
    phases: Vec<f64>,
    detail: [f64; 5],
    simulation_ticks: u32,
    dropped_ticks: f32,
    batch_ms: f64,
    budget_limited_steps: u32,
    rows: Vec<Frame>,
    epoch: Option<Instant>,
    frame_start_ns: u128,
    previous_begin_ns: Option<u128>,
    observed_frame_ms: Option<f64>,
    last_end_ns: Option<u128>,
    finished: bool,
    core_rows: Vec<core::Tick>,
    render_contexts: Vec<(u8, RenderContext)>,
    context_action_step: u8,
    render_context_invalid: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct RenderContext {
    physical_size: [u32; 2],
    focused: bool,
    scale_factor: f32,
    present_mode: bevy::window::PresentMode,
    window_mode: bevy::window::WindowMode,
    view: u8,
    camera_sweep: bool,
    camera_controls: [f32; 5],
    zone: usize,
    overlays: [bool; 2],
    drawer: u8,
}
impl RenderContext {
    fn same_window(&self, other: &Self) -> bool {
        self.physical_size == other.physical_size
            && self.focused == other.focused
            && self.scale_factor == other.scale_factor
            && self.present_mode == other.present_mode
            && self.window_mode == other.window_mode
    }
}
struct Frame {
    seconds: f64,
    tick: u32,
    ms: [f64; 15],
    generation: u64,
    start_ns: u128,
    end_ns: u128,
    gap_ns: u128,
    observed_frame_ms: Option<f64>,
    pacing: [f64; 4],
    // State after this slice, not arrivals; no-tick frames can repeat it.
    collapse: [usize; 3],
}
/// Capture requested camera controls, not animated/following camera poses.
/// Keep changes in memory and print after measurement, not in a slow frame.
pub fn render_context(
    scene: Res<Scene>,
    edit: Res<Workshop>,
    panel: Option<Res<panel::Panel>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    (time, gas, stream): (Res<Time<Real>>, Res<GasInput>, Res<streaming::Streaming>),
    mut p: ResMut<Profile>,
) {
    if p.start.is_none() || p.finished {
        return;
    }
    if let Some(sample) = p.camera_sample(time.elapsed_secs_f64(), scene.sim.size()) {
        let seconds = time.elapsed_secs_f64() - p.start.unwrap();
        let [observe_ms, stream_ms] = p.camera_work;
        let row = p.rows.len();
        let work = p.camera_detail;
        p.camera_frames.push(camera::Frame {
            row,
            generation: scene.generation,
            work,
            seconds,
            tick: scene.sim.tick,
            sample,
            origin: gas.window.origin,
            progress: stream.progress(),
            blocked: stream.loading(),
            observe_ms,
            stream_ms,
        });
    }
    let Ok(window) = windows.single() else {
        p.render_context_invalid = true;
        return;
    };
    let context = RenderContext {
        physical_size: [window.physical_width(), window.physical_height()],
        focused: window.focused,
        scale_factor: window.scale_factor(),
        present_mode: window.present_mode,
        window_mode: window.mode,
        view: scene.view,
        camera_sweep: scene.camera_sweep || p.camera_tour,
        camera_controls: [edit.pan.x, edit.pan.y, edit.zoom, edit.orbit, edit.pitch],
        zone: edit.zone,
        overlays: panel.as_ref().map_or([false; 2], |p| [p.ranges, p.routes]),
        drawer: panel.as_ref().map_or(0, |p| p.drawer),
    };
    if let Some((_, previous)) = p.render_contexts.last() {
        if *previous != context {
            let scripted = p.actions && p.context_action_step != p.action_step;
            if !previous.same_window(&context) || !scripted {
                p.render_context_invalid = true;
            }
        } else {
            p.context_action_step = p.action_step;
            return;
        }
    }
    let step = p.action_step;
    p.render_contexts.push((step, context));
    p.context_action_step = step;
}
/// Repeat the actual workshop requests in a native renderer, without video
/// readback. Loading must finish before the next action can be measured.
pub fn actions(
    time: Res<Time<Real>>,
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    stream: Res<streaming::Streaming>,
    gas: Res<GasInput>,
    mut p: ResMut<Profile>,
) {
    if !p.actions || p.start.is_none() || p.action_step >= 8 {
        return;
    }
    let now = time.elapsed_secs_f64();
    // The scripted sector check needs a published window. Camera preparation
    // itself no longer blocks authoritative simulation.
    if stream.loading() || stream.progress().is_some() {
        p.action_time = now;
        return;
    }
    let interval = if p.action_step == 4 { 6.0 } else { 1.0 };
    if now - p.action_time < interval {
        return;
    }
    let config = scene.sim.config.clone();
    let zone = edit.zone as i32;
    match p.action_step {
        0 => {
            let mut layout = edit
                .layout
                .clone()
                .expect("action profile requires a range");
            // The stock kilometre layout already fills all eight unit slots.
            // Relocate one slot to exercise placement without exceeding the cap.
            layout.units[0] = modding::range::RangeUnitDef {
                weapon: None,
                kind: RangeUnitKind::Tank,
                at: [1480, 16, 560],
                allied: true,
            };
            edit.commit(layout, config).expect("profile placement");
            edit.editing = true;
        }
        1 => edit.pending = Some((config, edit.layout.clone().unwrap())),
        4 => {
            assert!(
                scene.sim.units.iter().any(|u| u.revision > 0),
                "battle must damage real units before reset"
            );
            edit.pending = Some((config, edit.layout.clone().unwrap()));
        }
        2 => edit.battle_preset(config).expect("profile battle preset"),
        3 => {
            scene.sim.start_battle().expect("profile battle teams");
            edit.editing = false;
            scene.paused = false;
        }
        5 => edit.change_zone(7 - zone),
        6 => {
            assert!(
                gas.window.origin[0] > 8000 && gas.window.origin[2] > 8000,
                "far sector must actually stream"
            );
            println!("INTERACTIONS far_origin={:?}", gas.window.origin);
            edit.change_zone(-zone);
        }
        7 => {
            assert_eq!(gas.window.origin, [0, 0, 0]);
            println!(
                "INTERACTIONS complete generation={} origin={:?}",
                scene.generation, gas.window.origin
            );
        }
        _ => unreachable!(),
    }
    println!(
        "INTERACTIONS step={} seconds={:.3} tick={} damage={}",
        p.action_step,
        now - p.start.unwrap(),
        scene.sim.tick,
        scene.sim.units.iter().map(|u| u.revision).sum::<u64>()
    );
    p.action_step += 1;
    p.action_time = now;
}
impl Profile {
    pub(super) fn camera_sample(&self, now: f64, size: [u32; 3]) -> Option<camera::Sample> {
        (self.camera_tour && !self.finished)
            .then(|| camera::sample(now - self.start.unwrap_or(now), size))
    }
    pub fn step(&mut self, sim: &mut BlastRange, generation: u64) {
        if self.core && self.start.is_some() && !self.finished {
            self.core_rows
                .push(core::Tick::record(sim, generation, self.epoch.unwrap()));
        } else if self.start.is_some() && !self.finished && self.trigger.is_some() {
            let tick = core::Tick::record_phases(sim, generation, self.epoch.unwrap());
            self.trigger.as_mut().unwrap().push(tick);
        } else {
            sim.step();
        }
    }
    pub fn configure_trigger(&mut self, threshold_ms: u32) {
        assert!(
            self.output.is_some() && !self.core,
            "--profile-trigger-ms requires --profile and excludes --profile-core"
        );
        self.trigger = Some(core::Trigger::new(threshold_ms));
    }
    pub fn configure_workers(&self, sim: &mut BlastRange) {
        if let Some(workers) = self.physics_workers {
            assert!(sim.set_physics_workers(workers), "--physics-workers 1..=8");
        }
        if let Some(workers) = self.navigation_workers {
            assert!(
                sim.set_navigation_workers(workers),
                "--nav-workers 1..=8: pool creation failed"
            );
        }
    }
    pub fn new(output: Option<String>, seconds: f64) -> Self {
        assert!(seconds.is_finite() && (1.0..=3600.0).contains(&seconds));
        Self {
            output,
            seconds,
            ..default()
        }
    }
    pub fn running(&self) -> bool {
        self.output.is_none() || self.start.is_some()
    }
    pub fn measure_camera(&mut self, phase: usize, duration: std::time::Duration) {
        if self.start.is_some() && self.camera_tour {
            self.camera_work[phase] += duration.as_secs_f64() * 1000.0;
        }
    }
    pub fn camera_details(&mut self, work: CameraWork) {
        if self.start.is_some() && self.camera_tour {
            self.camera_detail = work;
        }
    }
    pub fn measure(&mut self, phase: usize, duration: std::time::Duration) {
        if self.start.is_some() {
            self.detail[phase] += duration.as_secs_f64() * 1000.0;
        }
    }
    /// Count completed authoritative steps and their cost before visual events.
    pub fn record_tick(&mut self, duration: std::time::Duration) {
        if self.start.is_some() {
            self.simulation_ticks += 1;
            self.detail[4] += duration.as_secs_f64() * 1000.0;
        }
    }
    pub fn record_dropped_ticks(&mut self, ticks: f32) {
        if self.start.is_some() {
            self.dropped_ticks += ticks;
        }
    }
    pub fn record_batch(&mut self, duration: std::time::Duration, limited: u32) {
        if self.start.is_some() {
            self.batch_ms += duration.as_secs_f64() * 1000.0;
            self.budget_limited_steps += limited;
        }
    }
    fn stamp(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.mark.replace(now) {
            self.phases
                .push(now.duration_since(last).as_secs_f64() * 1000.0);
        }
    }
}
pub fn begin(
    time: Res<Time<Real>>,
    scene: Res<Scene>,
    mut gas: ResMut<GasInput>,
    mut p: ResMut<Profile>,
    adapter: Option<Res<bevy::render::renderer::RenderAdapterInfo>>,
) {
    if p.output.is_none()
        || !gas.ready.load(Ordering::Relaxed)
        || (p.start.is_none() && gas.params.distant.z > 0.0 && gas.params.distant.w < 0.5)
    {
        return;
    }
    p.camera_work = [0.0; 2];
    p.camera_detail = CameraWork::default();
    let now = time.elapsed_secs_f64();
    let warm = *p.warm.get_or_insert(now);
    if now - warm < 2.0 {
        return;
    }
    if p.start.is_none() {
        p.epoch = Some(Instant::now());
        println!(
            "PROFILE schema=3 frame_ms_scope=bevy_time_delta_render_pipeline_lag observed_frame_ms_scope=previous_profile_begin_to_current_profile_begin cpu_time_scope=thread_kernel_plus_user"
        );
        println!(
            "PROFILE tick_rate={} epoch_unix_ns={}",
            sim_core::TICKS_PER_SECOND,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        if p.camera_tour {
            println!("PROFILE camera_tour_schema=1 camera_tour_clock=wall camera_tour_route=bases_pan_zoom_v1");
            println!("PROFILE camera_work_schema=1 camera_work_scope=same_frame_exclusive_wall");
        }
        if let Some(adapter) = adapter {
            println!(
                "PROFILE adapter_name={:?} adapter_backend={:?} adapter_device_type={:?} adapter_driver={:?}",
                adapter.name, adapter.backend, adapter.device_type, adapter.driver
            );
        } else {
            println!("PROFILE adapter=unavailable");
        }
        println!(
            "PROFILE compute_workers={}",
            bevy::tasks::ComputeTaskPool::try_get().map_or(0, |pool| pool.thread_num())
        );
        println!("PROFILE navigation_override={:?}", p.navigation_workers);
        println!("PROFILE physics_override={:?}", p.physics_workers);
        let [physics, navigation] = scene.sim.worker_counts();
        println!("PROFILE physics_workers={physics} navigation_workers={navigation}");
        if p.core {
            println!("PROFILE core_trace_schema=3");
            println!("PROFILE physics_task_sampling=longest_per_thread_family_stage");
        }
        if let Some(trigger) = &p.trigger {
            println!("PROFILE stall_capture_schema=3 stall_capture_mode=phase_ring threshold_ms={} pre_ticks={} post_ticks={} max_windows={} cpu_sampling=tick_boundaries",
                trigger.threshold_ms, core::Trigger::PRE, core::Trigger::POST, core::Trigger::MAX_WINDOWS);
        }
        gas.timings.extract_peak_ns.store(0, Ordering::Relaxed);
        gas.timings.prepare_peak_ns.store(0, Ordering::Relaxed);
        gas.timings.allocation_peak_ns.store(0, Ordering::Relaxed);
        gas.timings.compact_calls.store(0, Ordering::Relaxed);
        gas.timings.compact_total_ns.store(0, Ordering::Relaxed);
        gas.timings.compact_peak_ns.store(0, Ordering::Relaxed);
    }
    let start = *p.start.get_or_insert(now);
    p.frame_start_ns = p.epoch.unwrap().elapsed().as_nanos();
    p.observed_frame_ms = p
        .previous_begin_ns
        .map(|previous| (p.frame_start_ns - previous) as f64 / 1_000_000.0);
    if let Some(previous) = p.previous_begin_ns {
        let begin = p.frame_start_ns;
        if let Some(trigger) = &mut p.trigger {
            trigger.frame(
                "interval",
                scene.generation,
                scene.sim.tick,
                previous,
                begin,
            );
        }
    }
    p.previous_begin_ns = Some(p.frame_start_ns);
    if let Some(last) = p.last_end_ns {
        let begin = p.frame_start_ns;
        if let Some(trigger) = &mut p.trigger {
            trigger.frame("gap", scene.generation, scene.sim.tick, last, begin);
        }
    }
    if p.clouds > 0 && !p.emitted && now - start >= 0.5 {
        p.emitted = true;
        for i in 0..p.clouds {
            let mut params = gas.params;
            params.source = Vec4::new(
                210.0 + (i % 4) as f32 * 90.0,
                32.0,
                260.0 + (i / 4) as f32 * 110.0,
                24.0,
            );
            params.control.z = 1.0;
            gas.atmosphere.emit_at(params, scene.sim.tick * 2);
        }
    }
    p.phases.clear();
    p.detail.fill(0.0);
    p.simulation_ticks = 0;
    p.dropped_ticks = 0.0;
    p.batch_ms = 0.0;
    p.budget_limited_steps = 0;
    p.mark = Some(Instant::now());
}
pub fn stamp(mut p: ResMut<Profile>) {
    if p.start.is_some() && !p.finished {
        p.stamp();
    }
}
#[derive(bevy::ecs::system::SystemParam)]
pub struct Output<'w> {
    exit: MessageWriter<'w, AppExit>,
    source: Option<ResMut<'w, render::timeline::FrameSource>>,
    timeline: Option<Res<'w, render::timeline::Timeline>>,
    #[cfg(feature = "render-tasks")]
    tasks: Option<Res<'w, render::timeline::tasks::Tasks>>,
}
pub fn end(
    time: Res<Time<Real>>,
    scene: Res<Scene>,
    gas: Res<GasInput>,
    art: Res<WitnessArt>,
    diagnostics: Option<Res<bevy::diagnostic::DiagnosticsStore>>,
    mut p: ResMut<Profile>,
    output: Output,
) {
    let Output {
        mut exit,
        source,
        timeline,
        #[cfg(feature = "render-tasks")]
        tasks,
    } = output;
    let Some(start) = p.start else {
        return;
    };
    if p.finished {
        return;
    }
    p.stamp();
    if p.gpu {
        if let Some(diagnostics) = diagnostics {
            let epoch = p.epoch.unwrap();
            let row = p.rows.len();
            p.gpu_samples.observe(&diagnostics, epoch, row);
        }
    }
    let elapsed = time.elapsed_secs_f64() - start;
    let phases = [
        time.delta_secs_f64() * 1000.0,
        p.phases[0],
        p.phases[1],
        p.phases[2],
        p.phases[3],
        p.detail[0],
        p.detail[1],
        p.detail[2],
        gas.timings.prepare_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        gas.timings.allocation_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        gas.timings.active_regions.load(Ordering::Relaxed) as f64,
        gas.timings.extract_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        p.detail[3],
        f64::from(p.simulation_ticks),
        p.detail[4],
    ];
    // Keep the first measured slice too: its Bevy elapsed time can be zero,
    // while its monotonic slice and any recorded core tick are already real.
    if elapsed >= 0.0 {
        let end_ns = p.epoch.unwrap().elapsed().as_nanos();
        let start_ns = p.frame_start_ns;
        let observed_frame_ms = p.observed_frame_ms;
        let gap_ns = p
            .last_end_ns
            .map_or(0, |last| start_ns.saturating_sub(last));
        let pacing = [
            f64::from(scene.acc) * 30.0,
            f64::from(p.dropped_ticks),
            p.batch_ms,
            f64::from(p.budget_limited_steps),
        ];
        p.rows.push(Frame {
            seconds: elapsed,
            tick: scene.sim.tick,
            ms: phases,
            generation: scene.generation,
            start_ns,
            end_ns,
            gap_ns,
            observed_frame_ms,
            pacing,
            collapse: [
                scene.sim.collapse_work.queued,
                scene.sim.collapse_work.submitted,
                scene.sim.collapse_work.fresh_edits,
            ],
        });
        if let Some(mut source) = source {
            source.0 = Some(render::timeline::FrameTag {
                row: p.rows.len() - 1,
                generation: scene.generation,
                tick: scene.sim.tick,
                epoch: p.epoch.unwrap(),
            });
        }
        p.last_end_ns = Some(end_ns);
        if let Some(trigger) = &mut p.trigger {
            trigger.frame("update", scene.generation, scene.sim.tick, start_ns, end_ns);
        }
    }
    if elapsed < p.seconds {
        return;
    }
    p.finished = true;
    let render_capture = timeline.as_ref().map(|t| t.close(p.epoch.unwrap()));
    #[cfg(feature = "render-tasks")]
    let task_capture = tasks.as_ref().map(|t| t.close(p.epoch.unwrap()));
    if p.actions && p.action_step < 8 {
        eprintln!(
            "Interaction profile ended before all actions completed: {}/8",
            p.action_step
        );
        exit.write(AppExit::error());
        return;
    }
    let stall_rows = if let Some(trigger) = &mut p.trigger {
        let rows = trigger.finish();
        println!("PROFILE stall_windows={} stall_slow_ticks={} stall_unarmed_slow_ticks={} stall_retained_ticks={}",
            trigger.windows, trigger.slow_ticks, trigger.unarmed_slow_ticks, rows.len());
        println!("PROFILE stall_frame_signals={} stall_retained_frame_signals={} stall_dropped_frame_signals={} stall_unarmed_frame_signals={}",
            trigger.observed_frame_signals, trigger.frame_signals.len(),
            trigger.observed_frame_signals - trigger.frame_signals.len() as u64, trigger.unarmed_frame_signals);
        for signal in &trigger.frame_signals {
            println!("PROFILE stall_frame_signal source={} generation={} tick={} begin_ns={} end_ns={} window={}",
                signal.source, signal.generation, signal.tick, signal.begin_ns, signal.end_ns, signal.window);
        }
        if let Some(signal) = &trigger.peak_signal {
            println!("PROFILE stall_peak_signal source={} generation={} tick={} begin_ns={} end_ns={} window={} context_ticks={}",
                signal.source, signal.generation, signal.tick, signal.begin_ns, signal.end_ns, signal.window, core::Trigger::PRE);
        }
        rows
    } else {
        Vec::new()
    };
    let result = (|| -> std::io::Result<()> {
        if p.gpu {
            let path = std::path::Path::new(p.output.as_ref().unwrap()).with_extension("gpu.csv");
            let mut gpu = std::io::BufWriter::new(std::fs::File::create(path)?);
            p.gpu_samples.save(&mut gpu)?;
            gpu.flush()?;
            p.gpu_samples.summary(&mut std::io::stdout().lock())?;
        }
        if p.camera_tour {
            camera::save(p.output.as_ref().unwrap(), &p.camera_frames)?;
        }
        let mut out = std::io::BufWriter::new(std::fs::File::create(p.output.as_ref().unwrap())?);
        writeln!(
            out,
            "seconds,tick,frame_ms,simulation_input_ms,unit_mesh_ms,fragments_ms,presentation_ms,physics_ms,terrain_ms,atmosphere_ms,render_prepare_ms,gas_allocation_ms,gas_regions,render_extract_ms,occupancy_ms,simulation_ticks,simulation_core_ms,generation,frame_start_ns,frame_end_ns,frame_gap_ns,observed_frame_ms,simulation_debt_ticks,simulation_dropped_ticks,simulation_batch_ms,simulation_budget_limited_steps,collapse_queued,collapse_submitted,collapse_incoming"
        )?;
        for row in &p.rows {
            let Frame {
                seconds: t,
                tick,
                ms,
                generation,
                start_ns,
                end_ns,
                gap_ns,
                observed_frame_ms,
                pacing,
                collapse,
            } = row;
            writeln!(
                out,
                "{t:.6},{tick},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.0},{:.6},{:.6},{:.0},{:.6},{generation},{start_ns},{end_ns},{gap_ns},{},{:.6},{:.6},{:.6},{:.0},{},{},{}",
                ms[0],
                ms[1],
                ms[2],
                ms[3],
                ms[4],
                ms[5],
                ms[6],
                ms[7],
                ms[8],
                ms[9],
                ms[10],
                ms[11],
                ms[12],
                ms[13],
                ms[14],
                observed_frame_ms.map_or(String::new(), |value| format!("{value:.6}")),
                pacing[0], pacing[1], pacing[2], pacing[3], collapse[0], collapse[1], collapse[2]
            )?;
        }
        let saved = out
            .flush()
            .and_then(|()| core::save(p.output.as_ref().unwrap(), &p.core_rows))
            .and_then(|()| core::save_stalls(p.output.as_ref().unwrap(), &stall_rows))
            .and_then(|()| {
                render_capture
                    .as_ref()
                    .map_or(Ok(()), |t| t.save(p.output.as_ref().unwrap()))
            });
        #[cfg(feature = "render-tasks")]
        let saved = saved.and_then(|()| {
            task_capture
                .as_ref()
                .map_or(Ok(()), |t| t.save(p.output.as_ref().unwrap()))
        });
        saved
    })();
    for (column, name) in [
        "frame",
        "simulation_input",
        "unit_mesh",
        "fragments",
        "presentation",
        "physics",
        "terrain",
        "atmosphere",
        "render_prepare",
        "gas_allocation",
    ]
    .into_iter()
    .enumerate()
    .chain([
        (11, "render_extract"),
        (12, "occupancy"),
        (14, "simulation_core"),
    ]) {
        let mut values: Vec<_> = p
            .rows
            .iter()
            .filter_map(|r| {
                if column == 0 {
                    r.observed_frame_ms
                } else {
                    Some(r.ms[column])
                }
            })
            .collect();
        values.sort_by(f64::total_cmp);
        if !values.is_empty() {
            println!(
                "PROFILE {name} n={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}",
                values.len(),
                values[values.len() / 2],
                values[values.len() * 95 / 100],
                values[values.len() * 99 / 100],
                values[values.len() - 1]
            );
        }
    }
    let tick_count: u64 = p.rows.iter().map(|r| r.ms[13] as u64).sum();
    let core_ms: f64 = p.rows.iter().map(|r| r.ms[14]).sum();
    println!(
        "PROFILE simulation_ticks={} mean_core_tick_ms={:.3}",
        tick_count,
        if tick_count > 0 {
            core_ms / tick_count as f64
        } else {
            0.0
        }
    );
    println!(
        "PROFILE max_gas_regions={}",
        p.rows.iter().map(|r| r.ms[10] as usize).max().unwrap_or(0)
    );
    println!(
        "PROFILE render_extract_peak_ms={:.3}",
        gas.timings.extract_peak_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0
    );
    println!(
        "PROFILE terrain_journal_compact_calls={} total_ms={:.3} peak_ms={:.3}",
        gas.timings.compact_calls.load(Ordering::Relaxed),
        gas.timings.compact_total_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        gas.timings.compact_peak_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0
    );
    println!(
        "PROFILE render_thread_prepare_peak_ms={:.3} allocation_peak_ms={:.3}",
        gas.timings.prepare_peak_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        gas.timings.allocation_peak_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0
    );
    println!(
        "PROFILE ticks={} removed={} elapsed={elapsed:.3}s",
        scene.sim.tick, scene.sim.removed
    );
    println!(
        "PROFILE gun={:?} terrain_origin={:?} visible_units={} mesh_jobs={}",
        scene.sim.gun.at,
        gas.window.origin,
        art.seen.iter().filter(|v| v.is_some()).count(),
        art.pending.iter().filter(|v| v.is_some()).count()
    );
    println!(
        "PROFILE fuel_consumed={} generated_heat={} dissipated_heat={} active_fires={}",
        scene.sim.combustion.consumed,
        scene.sim.combustion.generated_heat,
        scene.sim.combustion.dissipated_heat,
        scene
            .sim
            .combustion
            .sources(&scene.sim.units, &scene.sim.matter)
            .len()
    );
    println!(
        "PROFILE stream_packets={} unit_damage={} melted_cells={}",
        scene.sim.streams.emitted(),
        scene.sim.units.iter().map(|u| u.revision).sum::<u64>(),
        scene.sim.thermals.melted
    );
    if scene.sim.battle_mode() {
        println!(
            "PROFILE battle={:?} alive={:?} shots={:?} damage={:?}",
            scene.sim.battle.phase,
            scene.sim.battle_counts(),
            scene
                .sim
                .battle
                .actors
                .iter()
                .map(|a| a.fired)
                .collect::<Vec<_>>(),
            scene
                .sim
                .units
                .iter()
                .map(|u| u.revision)
                .collect::<Vec<_>>()
        );
    }
    println!("PROFILE render_context_schema=2");
    if let Some((_, context)) = p.render_contexts.first() {
        println!("PROFILE render_context={context:?}");
    } else {
        println!("PROFILE render_context=unavailable");
    }
    for (step, context) in &p.render_contexts {
        println!("PROFILE render_context_step={step} value={context:?}");
    }
    println!(
        "PROFILE render_context_changed={}",
        p.render_context_invalid
    );
    println!("PROFILE state_hash={:016x}", scene.sim.state_hash());
    if let Err(e) = result {
        eprintln!("profile output: {e}");
        exit.write(AppExit::error());
    } else {
        exit.write(AppExit::Success);
    }
}
