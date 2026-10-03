//! Opt-in CPU wall timeline. Source identity is frozen before extraction;
//! pipelined rendering never reads the next main world's frame ID.
//! These spans are not GPU execution, CPU utilization, or lock-wait samples.
use bevy::prelude::*;
use bevy::render::{
    pipelined_rendering::RenderExtractApp,
    renderer::{render_system, RenderGraph, RenderGraphSystems},
    Render, RenderApp, RenderSystems,
};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    thread::ThreadId,
    time::Instant,
};

/// Zero-based row in the main frame CSV, including no-tick frames and resets.
#[derive(Clone, Copy, Debug)]
pub struct FrameTag {
    pub row: usize,
    pub generation: u64,
    pub tick: u32,
    pub epoch: Instant,
}

#[derive(Resource, Default)]
pub struct FrameSource(pub Option<FrameTag>);

/// Optional platform counters supplied by the native executable. The render
/// library remains safe and portable; unavailable counters remain None.
#[derive(Clone, Copy, Debug)]
pub struct CpuSample {
    pub os_thread: u32,
    pub time_ns: Option<u64>,
    pub cycles: Option<u64>,
}
fn unavailable() -> Option<CpuSample> {
    None
}

/// Per-call facts. Queue counters cover explicit terrain/distant transfers,
/// not uniform buffers, GPU execution or all allocations in the renderer.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Workload {
    pub input_frame_row: Option<usize>,
    pub patches: u64,
    pub words: u64,
    pub distant_words: u64,
    pub regions: u64,
    pub new_regions: u64,
    pub deposits: u64,
    pub terrain_writes: u64,
    pub terrain_bytes: u64,
    pub terrain_submits: u64,
    /// Extraction only: input before merging and actual same-thread wall phases.
    pub dirty_ranges: u64,
    pub range_ns: u64,
    pub pack_ns: u64,
    pub clone_ns: u64,
}
#[derive(Clone)]
pub(crate) struct FrameTrace {
    pub tag: FrameTag,
    recorder: Timeline,
}
#[derive(Resource, Default)]
pub(crate) struct RenderTrace(pub Option<FrameTrace>);
impl FrameTrace {
    pub fn scope(&self, stage: &'static str) -> Scope {
        Scope {
            recorder: self.recorder.clone(),
            token: self.recorder.begin(self.tag, stage),
            work: Workload::default(),
            completed: false,
        }
    }
}
/// Owned guard: no ECS borrow, global tag reads or added scheduling edges.
/// Early returns finish as aborted; cutoff leaves the record explicitly open.
pub(crate) struct Scope {
    recorder: Timeline,
    token: Option<usize>,
    work: Workload,
    completed: bool,
}
impl Scope {
    pub fn complete(mut self, work: Workload) {
        self.work = work;
        self.completed = true;
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        self.recorder
            .end_detail(self.token, Some((self.completed, self.work)));
    }
}

#[derive(Clone, Debug)]
struct Span {
    tag: FrameTag,
    stage: &'static str,
    begin_ns: u128,
    end_ns: Option<u128>,
    begin_thread: ThreadId,
    end_thread: Option<ThreadId>,
    begin_cpu: Option<CpuSample>,
    end_cpu: Option<CpuSample>,
    detail: Option<(bool, Workload)>,
}

#[derive(Default)]
struct Store {
    spans: Vec<Span>,
    started: u64,
    closed: bool,
}

/// A few lifecycle boundaries per frame, never per object or simulation job.
/// The mutex is diagnostic overhead; ordinary runs do not install this plugin.
#[derive(Resource, Clone)]
pub struct Timeline {
    store: Arc<Mutex<Store>>,
    limit: usize,
    pipelined: bool,
    sample: fn() -> Option<CpuSample>,
    gas_details: bool,
}

impl Default for Timeline {
    fn default() -> Self {
        Self::with_limit(100_000)
    }
}

impl Timeline {
    fn with_limit(limit: usize) -> Self {
        Self {
            store: Arc::new(Mutex::new(Store {
                spans: Vec::with_capacity(limit),
                ..default()
            })),
            limit,
            pipelined: false,
            sample: unavailable,
            gas_details: false,
        }
    }

    fn begin(&self, tag: FrameTag, stage: &'static str) -> Option<usize> {
        let mut store = self.store.lock().unwrap();
        if store.closed {
            return None;
        }
        store.started += 1;
        if store.spans.len() == self.limit {
            return None;
        }
        let slot = store.spans.len();
        store.spans.push(Span {
            tag,
            stage,
            begin_ns: tag.epoch.elapsed().as_nanos(),
            end_ns: None,
            begin_thread: std::thread::current().id(),
            end_thread: None,
            begin_cpu: (self.sample)(),
            end_cpu: None,
            detail: None,
        });
        Some(slot)
    }

    fn end(&self, slot: Option<usize>) {
        self.end_detail(slot, None);
    }
    fn end_detail(&self, slot: Option<usize>, detail: Option<(bool, Workload)>) {
        let Some(slot) = slot else { return };
        let mut store = self.store.lock().unwrap();
        if store.closed {
            return;
        }
        let span = &mut store.spans[slot];
        assert!(span.end_ns.is_none(), "timeline span finished twice");
        span.end_ns = Some(span.tag.epoch.elapsed().as_nanos());
        span.end_thread = Some(std::thread::current().id());
        span.end_cpu = (self.sample)();
        span.detail = detail;
    }

    /// Close under the lock: export cannot race the render thread's final span.
    /// Open spans remain explicit; no waiting for the renderer to force coverage.
    pub fn close(&self, epoch: Instant) -> Capture {
        let mut store = self.store.lock().unwrap();
        store.closed = true;
        Capture {
            spans: std::mem::take(&mut store.spans),
            started: store.started,
            cutoff_ns: epoch.elapsed().as_nanos(),
            limit: self.limit,
            pipelined: self.pipelined,
            gas_details: self.gas_details,
        }
    }
}

/// Budget for the requested capture, not the achieved frame rate. Reserve space
/// for 400 frames/s and all 13 current scopes/frame, plus the terminal frame.
/// Allocation remains bounded even for hour-long diagnostics; overflow is
/// reported explicitly and disqualifies a schema-4 capture from comparison.
pub const MAX_SPANS: usize = 1_000_000;
pub fn span_limit_for_seconds(seconds: f64) -> usize {
    assert!(seconds.is_finite() && (1.0..=3600.0).contains(&seconds));
    ((seconds.ceil() as usize + 1) * 400 * 13).clamp(100_000, MAX_SPANS)
}

pub struct Capture {
    spans: Vec<Span>,
    started: u64,
    cutoff_ns: u128,
    limit: usize,
    pipelined: bool,
    gas_details: bool,
}
impl Capture {
    pub fn save(&self, output: &str) -> std::io::Result<()> {
        let Self {
            spans,
            started,
            cutoff_ns,
            limit,
            pipelined,
            gas_details,
        } = self;
        let incomplete = spans.iter().filter(|s| s.end_ns.is_none()).count();
        println!("PROFILE render_timeline_schema=4 render_span_limit={limit} render_spans_started={started} render_spans_retained={} render_spans_dropped={} render_spans_incomplete={incomplete} render_cutoff_ns={cutoff_ns} render_pipelined={pipelined} render_gas_details={gas_details} render_span_bytes={}",
            spans.len(), started - spans.len() as u64, std::mem::size_of::<Span>());
        let path = std::path::Path::new(output).with_extension("render.csv");
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(
            out,
            "frame_row,generation,tick,stage,begin_ns,end_ns,begin_thread,end_thread,begin_os_thread,end_os_thread,begin_cpu_ns,end_cpu_ns,begin_cycles,end_cycles,completed,input_frame_row,patches,words,distant_words,regions,new_regions,deposits,terrain_writes,terrain_bytes,terrain_submits,dirty_ranges,range_ns,pack_ns,clone_ns"
        )?;
        for span in spans {
            writeln!(
                out,
                "{},{},{},{},{},{},{:?},{},{},{},{},{},{},{},{}",
                span.tag.row,
                span.tag.generation,
                span.tag.tick,
                span.stage,
                span.begin_ns,
                span.end_ns.map_or(String::new(), |n| n.to_string()),
                span.begin_thread,
                span.end_thread
                    .map_or(String::new(), |id| format!("{id:?}")),
                counter(span.begin_cpu.map(|v| u64::from(v.os_thread))),
                counter(span.end_cpu.map(|v| u64::from(v.os_thread))),
                counter(span.begin_cpu.and_then(|v| v.time_ns)),
                counter(span.end_cpu.and_then(|v| v.time_ns)),
                counter(span.begin_cpu.and_then(|v| v.cycles)),
                counter(span.end_cpu.and_then(|v| v.cycles)),
                detail_csv(span.detail)
            )?;
        }
        out.flush()
    }
}

fn detail_csv(detail: Option<(bool, Workload)>) -> String {
    let Some((completed, w)) = detail else {
        return ",,,,,,,,,,,,,,".into();
    };
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        u8::from(completed),
        w.input_frame_row.map_or(String::new(), |r| r.to_string()),
        w.patches,
        w.words,
        w.distant_words,
        w.regions,
        w.new_regions,
        w.deposits,
        w.terrain_writes,
        w.terrain_bytes,
        w.terrain_submits,
        w.dirty_ranges,
        w.range_ns,
        w.pack_ns,
        w.clone_ns
    )
}

#[derive(Resource, Default)]
struct RenderFrame {
    tag: Option<FrameTag>,
    schedule: Option<usize>,
    prepare: Option<usize>,
    system: Option<usize>,
    graph: Option<usize>,
    tail: Option<usize>,
}

fn counter(value: Option<u64>) -> String {
    value.map_or(String::new(), |n| n.to_string())
}

pub struct TimelinePlugin {
    pub sample: fn() -> Option<CpuSample>,
    pub gas_details: bool,
    pub span_limit: usize,
}
impl Default for TimelinePlugin {
    fn default() -> Self {
        Self {
            sample: unavailable,
            gas_details: false,
            span_limit: 100_000,
        }
    }
}
impl Plugin for TimelinePlugin {
    fn build(&self, app: &mut App) {
        assert!((1..=MAX_SPANS).contains(&self.span_limit));
        let recorder = Timeline {
            pipelined: app.get_sub_app(RenderExtractApp).is_some(),
            sample: self.sample,
            gas_details: self.gas_details,
            ..Timeline::with_limit(self.span_limit)
        };
        app.init_resource::<FrameSource>()
            .insert_resource(recorder.clone());
        let render = app
            .get_sub_app_mut(RenderApp)
            .expect("timeline requires RenderApp");
        render
            .insert_resource(recorder.clone())
            .init_resource::<RenderFrame>()
            .init_resource::<RenderTrace>();
        let extract_recorder = recorder.clone();
        let mut original = render.take_extract().expect("Bevy extraction callback");
        render.set_extract(move |main, render| {
            let tag = main.resource::<FrameSource>().0;
            render.resource_mut::<RenderFrame>().tag = tag;
            let trace = tag
                .filter(|_| extract_recorder.gas_details)
                .map(|tag| FrameTrace {
                    tag,
                    recorder: extract_recorder.clone(),
                });
            render.resource_mut::<RenderTrace>().0 = trace.clone();
            if extract_recorder.gas_details {
                if let Some(mut gas) = main.get_resource_mut::<crate::gas::GasInput>() {
                    // Diagnostic identity must not force an unchanged resource
                    // through Bevy's change-gated extraction again.
                    gas.bypass_change_detection().trace = trace;
                }
            }
            let token = tag.and_then(|t| extract_recorder.begin(t, "extract_callback"));
            original(main, render);
            extract_recorder.end(token);
        });
        render.add_systems(
            Render,
            (
                schedule_begin.before(RenderSystems::ExtractCommands),
                prepare_end
                    .before(RenderSystems::Render)
                    .after(RenderSystems::Prepare),
                system_begin.before(render_system).after(prepare_end),
                system_end
                    .after(render_system)
                    .before(RenderSystems::Cleanup),
                schedule_end.after(RenderSystems::PostCleanup),
            ),
        );
        render.add_systems(
            RenderGraph,
            (
                graph_begin.before(RenderGraphSystems::Begin),
                graph_end.after(RenderGraphSystems::Finish),
            ),
        );
        // The outer callback also includes receiving the previous render world
        // and handing off the extracted frame. It is not pure wait time.
        if let Some(pipeline) = app.get_sub_app_mut(RenderExtractApp) {
            let mut original = pipeline
                .take_extract()
                .expect("pipeline extraction callback");
            pipeline.set_extract(move |main, world| {
                let token = main
                    .resource::<FrameSource>()
                    .0
                    .and_then(|t| recorder.begin(t, "pipeline_handoff"));
                original(main, world);
                recorder.end(token);
            });
        }
    }
}

// Exclusive boundary systems run on the schedule's driving thread. No resource
// writes or ordering edges are added to individual parallel preparation jobs.
fn schedule_begin(world: &mut World) {
    let recorder = world.resource::<Timeline>().clone();
    let mut frame = world.resource_mut::<RenderFrame>();
    frame.schedule = frame.tag.and_then(|t| recorder.begin(t, "render_schedule"));
    frame.prepare = frame.tag.and_then(|t| recorder.begin(t, "render_prepare"));
}
fn prepare_end(world: &mut World) {
    world
        .resource::<Timeline>()
        .end(world.resource::<RenderFrame>().prepare);
}
fn system_begin(world: &mut World) {
    let recorder = world.resource::<Timeline>().clone();
    let mut frame = world.resource_mut::<RenderFrame>();
    frame.system = frame.tag.and_then(|t| recorder.begin(t, "render_system"));
}
fn system_end(world: &mut World) {
    let recorder = world.resource::<Timeline>();
    let frame = world.resource::<RenderFrame>();
    recorder.end(frame.tail);
    recorder.end(frame.system);
}
fn schedule_end(world: &mut World) {
    world
        .resource::<Timeline>()
        .end(world.resource::<RenderFrame>().schedule);
}
fn graph_begin(world: &mut World) {
    let recorder = world.resource::<Timeline>().clone();
    let mut frame = world.resource_mut::<RenderFrame>();
    frame.graph = frame.tag.and_then(|t| recorder.begin(t, "render_graph"));
}
fn graph_end(world: &mut World) {
    let recorder = world.resource::<Timeline>().clone();
    let mut frame = world.resource_mut::<RenderFrame>();
    recorder.end(frame.graph);
    // Includes final submission/readbacks/presentation/collection and boundary
    // overhead, NOT an isolated present or GPU execution measurement.
    frame.tail = frame
        .tag
        .and_then(|t| recorder.begin(t, "submit_present_tail"));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duration_budget_is_bounded_and_keeps_late_records() {
        assert_eq!(span_limit_for_seconds(5.0), 100_000);
        assert_eq!(span_limit_for_seconds(20.0), 109_200);
        assert_eq!(span_limit_for_seconds(120.0), 629_200);
        assert_eq!(span_limit_for_seconds(120.1), 634_400);
        assert_eq!(span_limit_for_seconds(3600.0), MAX_SPANS);
        let recorder = Timeline::with_limit(span_limit_for_seconds(120.0));
        let tag = tag(0);
        // Reproduce the old 100k cutoff, including a completed late call.
        for _ in 0..100_001 {
            let token = recorder.begin(tag, "gas_prepare");
            assert!(token.is_some());
            recorder.end(token);
        }
        let capture = recorder.close(tag.epoch);
        assert_eq!(capture.started as usize, capture.spans.len());
        assert!(capture.spans.last().unwrap().end_ns.is_some());
        assert!(
            capture.spans.len() * std::mem::size_of::<Span>()
                <= MAX_SPANS * std::mem::size_of::<Span>()
        );
    }
    #[test]
    fn duration_budget_rejects_invalid_capture_duration() {
        for seconds in [0.0, 3601.0, f64::NAN, f64::INFINITY] {
            assert!(std::panic::catch_unwind(|| span_limit_for_seconds(seconds)).is_err());
        }
    }
    fn tag(row: usize) -> FrameTag {
        FrameTag {
            row,
            generation: row as u64 / 2,
            tick: row as u32 % 2,
            epoch: Instant::now(),
        }
    }
    #[test]
    fn bounded_records_keep_dropped_counts_and_open_spans() {
        let recorder = Timeline::with_limit(2);
        let first = recorder.begin(tag(0), "extract_callback");
        recorder.end(first);
        recorder.begin(tag(1), "render_system");
        assert!(recorder.begin(tag(2), "render_system").is_none());
        let store = recorder.store.lock().unwrap();
        assert_eq!(store.started, 3);
        assert_eq!(store.spans.len(), 2);
        assert!(store.spans[0].end_ns.is_some());
        assert!(store.spans[1].end_ns.is_none());
    }
    #[test]
    fn render_identity_survives_next_main_frame_and_reset() {
        let mut world = World::new();
        let recorder = Timeline::with_limit(8);
        world.insert_resource(recorder.clone());
        world.insert_resource(FrameSource(Some(tag(3))));
        world.insert_resource(RenderFrame {
            tag: Some(tag(0)),
            ..default()
        });
        schedule_begin(&mut world);
        prepare_end(&mut world);
        schedule_end(&mut world);
        let store = recorder.store.lock().unwrap();
        assert_eq!(store.spans.len(), 2);
        assert!(store
            .spans
            .iter()
            .all(|s| s.tag.row == 0 && s.tag.generation == 0));
    }
    #[test]
    fn warmup_does_not_record_or_reuse_previous_tokens() {
        let mut world = World::new();
        let recorder = Timeline::with_limit(8);
        world.insert_resource(recorder.clone());
        world.insert_resource(RenderFrame {
            tag: Some(tag(0)),
            ..default()
        });
        schedule_begin(&mut world);
        prepare_end(&mut world);
        schedule_end(&mut world);
        world.resource_mut::<RenderFrame>().tag = None;
        schedule_begin(&mut world);
        prepare_end(&mut world);
        schedule_end(&mut world);
        assert_eq!(recorder.store.lock().unwrap().started, 2);
    }
    #[test]
    fn cutoff_freezes_open_records_and_stops_future_callbacks() {
        let recorder = Timeline::with_limit(2);
        let tag = tag(0);
        let token = recorder.begin(tag, "render_system");
        let capture = recorder.close(tag.epoch);
        recorder.end(token);
        assert!(recorder.begin(tag, "render_system").is_none());
        assert_eq!(capture.started, 1);
        assert!(capture.spans[0].end_ns.is_none());
        assert!(capture.spans[0].begin_ns <= capture.cutoff_ns);
        assert_eq!(recorder.store.lock().unwrap().started, 1);
    }
    #[test]
    fn plugin_wraps_existing_extraction_and_freezes_identity_first() {
        let mut app = App::new();
        let mut sub = bevy::app::SubApp::new();
        sub.add_schedule(Render::base_schedule());
        sub.add_schedule(RenderGraph::base_schedule());
        sub.set_extract(|main, render| {
            // Deliberately advance/reset the main tag inside the callback.
            main.resource_mut::<FrameSource>().0 = Some(tag(3));
            render.insert_resource(FrameSource(Some(tag(4))));
        });
        app.insert_sub_app(RenderApp, sub);
        app.add_plugins(TimelinePlugin::default());
        app.world_mut().resource_mut::<FrameSource>().0 = Some(tag(0));
        let mut sub = app.remove_sub_app(RenderApp).unwrap();
        sub.extract(app.world_mut());
        assert_eq!(sub.world().resource::<RenderFrame>().tag.unwrap().row, 0);
        assert_eq!(sub.world().resource::<FrameSource>().0.unwrap().row, 4);
        assert_eq!(app.world().resource::<FrameSource>().0.unwrap().row, 3);
        let recorder = app.world().resource::<Timeline>();
        let store = recorder.store.lock().unwrap();
        assert_eq!(store.spans.len(), 1);
        assert_eq!(store.spans[0].tag.row, 0);
        assert!(store.spans[0].end_ns.is_some());
    }
    #[test]
    fn actual_call_guards_keep_completion_work_and_cutoff() {
        let recorder = Timeline::with_limit(4);
        let tag = tag(0);
        let trace = FrameTrace {
            tag,
            recorder: recorder.clone(),
        };
        trace.scope("gas_extract").complete(Workload {
            input_frame_row: Some(0),
            words: 512,
            ..default()
        });
        drop(trace.scope("gas_prepare"));
        let open = trace.scope("gas_regions");
        let capture = recorder.close(tag.epoch);
        drop(open);
        assert_eq!(capture.spans[0].detail.unwrap().0, true);
        assert_eq!(capture.spans[0].detail.unwrap().1.words, 512);
        assert_eq!(capture.spans[1].detail.unwrap().0, false);
        assert!(capture.spans[2].end_ns.is_none());
        assert!(capture.spans[2].detail.is_none());
        assert_eq!(detail_csv(None).split(',').count(), 15);
    }
    #[test]
    fn diagnostic_tag_does_not_mark_gas_changed_or_copy_payload() {
        use bevy::render::extract_resource::ExtractResource;
        let mut app = App::new();
        let mut sub = bevy::app::SubApp::new();
        sub.add_schedule(Render::base_schedule());
        sub.add_schedule(RenderGraph::base_schedule());
        sub.set_extract(|_, _| {});
        app.insert_sub_app(RenderApp, sub);
        app.insert_resource(crate::gas::GasInput::default());
        app.add_plugins(TimelinePlugin {
            gas_details: true,
            ..default()
        });
        app.world_mut().clear_trackers();
        app.world_mut().increment_change_tick();
        let changed = app
            .world_mut()
            .resource_mut::<crate::gas::GasInput>()
            .last_changed();
        let mut sub = app.remove_sub_app(RenderApp).unwrap();
        app.world_mut().resource_mut::<FrameSource>().0 = Some(tag(0));
        sub.extract(app.world_mut());
        assert_eq!(
            app.world_mut()
                .resource_mut::<crate::gas::GasInput>()
                .last_changed(),
            changed
        );
        let payload =
            crate::gas::GasInput::extract_resource(app.world().resource::<crate::gas::GasInput>());
        app.world_mut().resource_mut::<FrameSource>().0 = Some(tag(1));
        sub.extract(app.world_mut());
        assert_eq!(
            app.world_mut()
                .resource_mut::<crate::gas::GasInput>()
                .last_changed(),
            changed
        );
        assert_eq!(
            sub.world()
                .resource::<RenderTrace>()
                .0
                .as_ref()
                .unwrap()
                .tag
                .row,
            1
        );
        assert!(payload.trace.is_none());
        let store = app.world().resource::<Timeline>().store.lock().unwrap();
        let call = store
            .spans
            .iter()
            .find(|s| s.stage == "gas_extract")
            .unwrap();
        assert_eq!(call.tag.row, 0);
        assert_eq!(call.detail.unwrap().1.input_frame_row, Some(0));
    }
}
