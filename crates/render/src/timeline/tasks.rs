//! Observe Bevy's cached system spans on the actual calling thread. No system
//! replacements, scheduler edges, global next-frame IDs or per-frame file I/O.
use super::{counter, CpuSample, FrameTag};
use bevy::{
    ecs::schedule::Schedules,
    log::{
        tracing::{
            field::{Field, Visit},
            span::{Attributes, Id},
            Subscriber,
        },
        tracing_subscriber::{self, layer::Context, registry::LookupSpan, Layer},
    },
    prelude::*,
    render::{ExtractSchedule, Render},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io::Write,
    sync::{Arc, Mutex, OnceLock},
    thread::ThreadId,
    time::Instant,
};

pub const LIMIT: usize = 2_000_000;
#[derive(Clone)]
struct Name {
    value: Arc<str>,
    commands: bool,
}
#[derive(Clone)]
struct Call {
    tag: FrameTag,
    name: Name,
    begin: u128,
    end: Option<u128>,
    thread: ThreadId,
    end_thread: Option<ThreadId>,
    cpu: Option<CpuSample>,
    end_cpu: Option<CpuSample>,
}
#[derive(Default)]
struct Store {
    configured: bool,
    active: Option<FrameTag>,
    allowed: BTreeSet<Arc<str>>,
    excluded: BTreeSet<Arc<str>>,
    calls: Vec<Call>,
    started: u64,
    closed: bool,
}
#[derive(Resource, Clone)]
pub struct Tasks {
    store: Arc<Mutex<Store>>,
    allowed: Arc<OnceLock<BTreeSet<Arc<str>>>>,
    sample: fn() -> Option<CpuSample>,
    limit: usize,
    stride: usize,
}
impl Tasks {
    pub fn new(sample: fn() -> Option<CpuSample>, seconds: f64) -> Self {
        let mut tasks = Self::with_limit(sample, LIMIT);
        tasks.stride = if seconds > 20.0 { 8 } else { 1 };
        tasks
    }
    fn with_limit(sample: fn() -> Option<CpuSample>, limit: usize) -> Self {
        Self {
            allowed: Arc::new(OnceLock::new()),
            store: Arc::new(Mutex::new(Store {
                calls: Vec::with_capacity(limit),
                ..default()
            })),
            sample,
            limit,
            stride: 1,
        }
    }
    pub(super) fn configure_once(&self, main: &World, render: &World) {
        let mut store = self.store.lock().unwrap();
        if store.configured {
            return;
        }
        let names = |schedule: &bevy::ecs::schedule::Schedule| -> BTreeSet<Arc<str>> {
            schedule
                .graph()
                .systems
                .iter()
                .map(|(_, s, _)| Arc::from(s.name().to_string()))
                .collect()
        };
        let mut candidates = BTreeSet::new();
        if let Some(schedules) = render.get_resource::<Schedules>() {
            for schedule in [schedules.get(Render), schedules.get(ExtractSchedule)]
                .into_iter()
                .flatten()
            {
                candidates.extend(names(schedule));
            }
        }
        let mut main_names = BTreeSet::new();
        if let Some(schedules) = main.get_resource::<Schedules>() {
            for (_, schedule) in schedules.iter() {
                main_names.extend(names(schedule));
            }
        }
        for name in candidates {
            if main_names.contains(&name) || name.starts_with("render::timeline::") {
                store.excluded.insert(name);
            } else {
                store.allowed.insert(name);
            }
        }
        store.configured = true;
        self.allowed.set(store.allowed.clone()).unwrap();
    }
    pub(super) fn set_active(&self, tag: Option<FrameTag>) {
        let mut store = self.store.lock().unwrap();
        if !store.closed {
            store.active = tag.filter(|tag| tag.row.is_multiple_of(self.stride));
        }
    }
    fn begin(&self, name: &Name) -> Option<usize> {
        if !self.allows(name) {
            return None;
        }
        let mut store = self.store.lock().unwrap();
        let tag = store.active?;
        if store.closed || !store.allowed.contains(&name.value) {
            return None;
        }
        store.started += 1;
        if store.calls.len() == self.limit {
            return None;
        }
        let slot = store.calls.len();
        store.calls.push(Call {
            tag,
            name: name.clone(),
            begin: tag.epoch.elapsed().as_nanos(),
            end: None,
            thread: std::thread::current().id(),
            end_thread: None,
            cpu: (self.sample)(),
            end_cpu: None,
        });
        Some(slot)
    }
    fn allows(&self, name: &Name) -> bool {
        self.allowed
            .get()
            .is_some_and(|names| names.contains(&name.value))
    }
    fn end(&self, token: Option<usize>) {
        let Some(token) = token else {
            return;
        };
        let mut store = self.store.lock().unwrap();
        if store.closed {
            return;
        }
        let call = &mut store.calls[token];
        assert!(call.end.is_none());
        call.end = Some(call.tag.epoch.elapsed().as_nanos());
        call.end_thread = Some(std::thread::current().id());
        call.end_cpu = (self.sample)();
    }
    pub fn close(&self, epoch: Instant) -> Capture {
        let mut store = self.store.lock().unwrap();
        store.closed = true;
        store.active = None;
        Capture {
            calls: std::mem::take(&mut store.calls),
            started: store.started,
            allowed: store.allowed.clone(),
            excluded: store.excluded.clone(),
            limit: self.limit,
            cutoff: epoch.elapsed().as_nanos(),
            stride: self.stride,
        }
    }
}
pub struct Capture {
    calls: Vec<Call>,
    started: u64,
    allowed: BTreeSet<Arc<str>>,
    excluded: BTreeSet<Arc<str>>,
    limit: usize,
    cutoff: u128,
    stride: usize,
}
impl Capture {
    pub fn save(&self, output: &str) -> std::io::Result<()> {
        println!("PROFILE render_tasks_schema=1 render_task_limit={} render_tasks_started={} render_tasks_retained={} render_tasks_dropped={} render_tasks_incomplete={} render_task_cutoff_ns={} render_task_names={} render_task_excluded_names={} render_task_bytes={} render_task_stride={}",
            self.limit, self.started, self.calls.len(), self.started-self.calls.len() as u64,
            self.calls.iter().filter(|c|c.end.is_none()).count(), self.cutoff, self.allowed.len(), self.excluded.len(), std::mem::size_of::<Call>(), self.stride);
        let path = std::path::Path::new(output);
        let mut out =
            std::io::BufWriter::new(std::fs::File::create(path.with_extension("tasks.csv"))?);
        writeln!(out, "frame_row,generation,tick,name,kind,begin_ns,end_ns,begin_thread,end_thread,begin_os_thread,end_os_thread,begin_cpu_ns,end_cpu_ns,begin_cycles,end_cycles")?;
        for c in &self.calls {
            writeln!(
                out,
                "{},{},{},{},{},{},{},{:?},{},{},{},{},{},{},{}",
                c.tag.row,
                c.tag.generation,
                c.tag.tick,
                quote(&c.name.value),
                if c.name.commands {
                    "commands"
                } else {
                    "system"
                },
                c.begin,
                c.end.map_or(String::new(), |n| n.to_string()),
                c.thread,
                c.end_thread.map_or(String::new(), |t| format!("{t:?}")),
                counter(c.cpu.map(|v| u64::from(v.os_thread))),
                counter(c.end_cpu.map(|v| u64::from(v.os_thread))),
                counter(c.cpu.and_then(|v| v.time_ns)),
                counter(c.end_cpu.and_then(|v| v.time_ns)),
                counter(c.cpu.and_then(|v| v.cycles)),
                counter(c.end_cpu.and_then(|v| v.cycles))
            )?;
        }
        out.flush()?;
        let mut names = std::io::BufWriter::new(std::fs::File::create(
            path.with_extension("task-names.csv"),
        )?);
        writeln!(names, "name,allowed")?;
        for (set, allowed) in [(&self.allowed, 1), (&self.excluded, 0)] {
            for name in set {
                writeln!(names, "{},{}", quote(name), allowed)?;
            }
        }
        names.flush()
    }
}
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

struct TaskLayer(Tasks);
#[derive(Default)]
struct NameVisitor(String);
impl Visit for NameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.0 = value.into();
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "name" {
            self.0 = format!("{value:?}");
        }
    }
}
type TokenStacks = BTreeMap<(usize, u64), Vec<Option<usize>>>;
thread_local! { static TOKENS: RefCell<TokenStacks> = const { RefCell::new(BTreeMap::new()) }; }
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for TaskLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut name = NameVisitor::default();
        attrs.record(&mut name);
        ctx.span(id).unwrap().extensions_mut().insert(Name {
            value: name.0.into(),
            commands: attrs.metadata().name() == "system_commands",
        });
    }
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).unwrap();
        let name = span.extensions().get::<Name>().unwrap().clone();
        if !self.0.allows(&name) {
            return;
        }
        let token = self.0.begin(&name);
        let key = (Arc::as_ptr(&self.0.store) as usize, id.into_u64());
        TOKENS.with(|tokens| tokens.borrow_mut().entry(key).or_default().push(token));
    }
    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).unwrap();
        if !self.0.allows(span.extensions().get::<Name>().unwrap()) {
            return;
        }
        let key = (Arc::as_ptr(&self.0.store) as usize, id.into_u64());
        let token = TOKENS.with(|tokens| {
            let mut tokens = tokens.borrow_mut();
            let stack = tokens.get_mut(&key).expect("task span exit without enter");
            let token = stack.pop().unwrap();
            if stack.is_empty() {
                tokens.remove(&key);
            }
            token
        });
        self.0.end(token);
    }
}
/// Installed through LogPlugin before any cached Bevy SystemMeta spans exist.
pub fn layer(app: &mut App) -> Option<bevy::log::BoxedLayer> {
    app.world()
        .get_resource::<Tasks>()
        .cloned()
        .map(boxed_layer)
}
fn boxed_layer(tasks: Tasks) -> bevy::log::BoxedLayer {
    Box::new(
        TaskLayer(tasks).with_filter(tracing_subscriber::filter::filter_fn(|m| {
            m.target() == "bevy_ecs::system::function_system"
                && matches!(m.name(), "system" | "system_commands")
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::log::{tracing, tracing_subscriber::prelude::*};
    #[test]
    fn sampling_retains_whole_source_frames_and_declares_the_stride() {
        let (mut tasks, mut tag) = setup(10);
        tasks.stride = 8;
        let name = Name {
            value: "test".into(),
            commands: false,
        };
        for row in 0..17 {
            tag.row = row;
            tasks.set_active(Some(tag));
            let token = tasks.begin(&name);
            assert_eq!(token.is_some(), row % 8 == 0);
            tasks.end(token);
        }
        let captured = tasks.close(tag.epoch);
        assert_eq!(captured.stride, 8);
        assert_eq!(captured.started, 3);
        assert_eq!(
            captured.calls.iter().map(|c| c.tag.row).collect::<Vec<_>>(),
            [0, 8, 16]
        );
    }
    fn setup(limit: usize) -> (Tasks, FrameTag) {
        let tasks = Tasks::with_limit(super::super::unavailable, limit);
        {
            let mut s = tasks.store.lock().unwrap();
            s.allowed.insert("test".into());
            tasks.allowed.set(s.allowed.clone()).unwrap();
        }
        let tag = FrameTag {
            row: 3,
            generation: 2,
            tick: 7,
            epoch: Instant::now(),
        };
        tasks.set_active(Some(tag));
        (tasks, tag)
    }
    #[test]
    fn owned_call_keeps_source_across_reset_and_cutoff() {
        let (tasks, tag) = setup(2);
        let name = Name {
            value: "test".into(),
            commands: false,
        };
        let first = tasks.begin(&name);
        tasks.set_active(Some(FrameTag {
            row: 4,
            generation: 3,
            ..tag
        }));
        tasks.end(first);
        let open = tasks.begin(&name);
        let capture = tasks.close(tag.epoch);
        tasks.end(open);
        assert_eq!(capture.calls[0].tag.row, 3);
        assert_eq!(capture.calls[0].tag.generation, 2);
        assert!(capture.calls[0].end.is_some());
        assert!(capture.calls[1].end.is_none());
        assert!(tasks.begin(&name).is_none());
    }
    #[test]
    fn inactive_unknown_and_overflow_are_not_invented_jobs() {
        let (tasks, tag) = setup(1);
        let name = Name {
            value: "test".into(),
            commands: false,
        };
        assert!(tasks
            .begin(&Name {
                value: "main".into(),
                commands: false
            })
            .is_none());
        tasks.set_active(None);
        assert!(tasks.begin(&name).is_none());
        tasks.set_active(Some(tag));
        tasks.end(tasks.begin(&name));
        assert!(tasks.begin(&name).is_none());
        let capture = tasks.close(tag.epoch);
        assert_eq!(capture.started, 2);
        assert_eq!(capture.calls.len(), 1);
        assert_eq!(quote("test<A, \"B\">"), "\"test<A, \"\"B\"\">\"");
    }
    #[test]
    fn cached_system_spans_record_actual_parallel_threads() {
        let (tasks, tag) = setup(8);
        let subscriber = tracing_subscriber::registry().with(boxed_layer(tasks.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(target:"bevy_ecs::system::function_system", parent:None,"system",name="test");
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    let span = span.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        let _entered = span.enter();
                        barrier.wait();
                    })
                })
                .collect();
            for worker in workers {
                worker.join().unwrap();
            }
        });
        let capture = tasks.close(tag.epoch);
        assert_eq!(capture.calls.len(), 2);
        assert_ne!(capture.calls[0].thread, capture.calls[1].thread);
        assert!(capture
            .calls
            .iter()
            .all(|c| c.end_thread == Some(c.thread) && c.tag.row == 3));
        assert!(capture.calls[1].begin <= capture.calls[0].end.unwrap());
    }
    #[derive(Resource)]
    struct BarrierResource(Arc<std::sync::Barrier>);
    fn actual_job(barrier: Res<BarrierResource>) {
        barrier.0.wait();
    }
    #[test]
    fn real_function_system_entries_have_names_and_overlap() {
        use bevy::ecs::system::{IntoSystem, System};
        let tasks = Tasks::with_limit(super::super::unavailable, 8);
        let tag = FrameTag {
            row: 0,
            generation: 1,
            tick: 1,
            epoch: Instant::now(),
        };
        let subscriber = tracing_subscriber::registry().with(boxed_layer(tasks.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let systems = [
                IntoSystem::into_system(actual_job),
                IntoSystem::into_system(actual_job),
            ];
            let name: Arc<str> = systems[0].name().to_string().into();
            tasks.store.lock().unwrap().allowed.insert(name.clone());
            tasks.allowed.set([name].into_iter().collect()).unwrap();
            tasks.set_active(Some(tag));
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let workers: Vec<_> = systems
                .into_iter()
                .map(|mut system| {
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        let mut world = World::new();
                        world.insert_resource(BarrierResource(barrier));
                        system.initialize(&mut world);
                        system.run((), &mut world).unwrap();
                    })
                })
                .collect();
            for worker in workers {
                worker.join().unwrap();
            }
        });
        let capture = tasks.close(tag.epoch);
        assert_eq!(capture.calls.len(), 2);
        assert!(capture
            .calls
            .iter()
            .all(|c| c.name.value.ends_with("actual_job") && c.end_thread == Some(c.thread)));
        assert_ne!(capture.calls[0].thread, capture.calls[1].thread);
    }
    #[test]
    fn inventory_excludes_main_names_and_diagnostic_probes() {
        use bevy::ecs::system::IntoSystem;
        fn shared() {}
        fn render_only() {}
        let mut main = World::new();
        let mut render = World::new();
        let mut a = bevy::ecs::schedule::Schedule::new(Update);
        a.add_systems(IntoSystem::into_system(shared).with_name("fixture_shared"));
        let mut b = bevy::ecs::schedule::Schedule::new(Render);
        b.add_systems((
            IntoSystem::into_system(shared).with_name("fixture_shared"),
            IntoSystem::into_system(render_only).with_name("fixture_render_only"),
            super::super::schedule_begin,
        ));
        let mut schedules = Schedules::default();
        schedules.insert(a);
        main.insert_resource(schedules);
        let mut schedules = Schedules::default();
        schedules.insert(b);
        render.insert_resource(schedules);
        let tasks = Tasks::with_limit(super::super::unavailable, 8);
        tasks.configure_once(&main, &render);
        let store = tasks.store.lock().unwrap();
        assert_eq!(store.allowed.len(), 1);
        assert!(store
            .allowed
            .iter()
            .next()
            .unwrap()
            .ends_with("render_only"));
        assert_eq!(store.excluded.len(), 2);
    }
}
