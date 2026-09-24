//! Frame timings without screenshots or PNG encoding. Optional GPU timestamps
//! read back timing queries only, never terrain, pixels or simulation fields.
use super::*;
use std::{io::Write, time::Instant};

#[derive(Resource, Default)]
pub struct Profile {
    pub output: Option<String>,
    pub seconds: f64,
    pub clouds: usize,
    pub gpu: bool,
    pub actions: bool,
    action_step: u8,
    action_time: f64,
    gpu_samples: std::collections::BTreeMap<String, GpuSamples>,
    emitted: bool,
    warm: Option<f64>,
    start: Option<f64>,
    mark: Option<Instant>,
    phases: Vec<f64>,
    detail: [f64; 4],
    rows: Vec<(f64, u32, [f64; 13])>,
    finished: bool,
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
    if stream.loading() {
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
#[derive(Default)]
struct GpuSamples {
    last: Option<bevy::platform::time::Instant>,
    values: Vec<f64>,
}
impl Profile {
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
    pub fn measure(&mut self, phase: usize, duration: std::time::Duration) {
        if self.start.is_some() {
            self.detail[phase] += duration.as_secs_f64() * 1000.0;
        }
    }
    fn stamp(&mut self) {
        if let Some(last) = self.mark.replace(Instant::now()) {
            self.phases.push(last.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
pub fn begin(
    time: Res<Time<Real>>,
    scene: Res<Scene>,
    mut gas: ResMut<GasInput>,
    mut p: ResMut<Profile>,
) {
    if p.output.is_none() || !gas.ready.load(Ordering::Relaxed) {
        return;
    }
    let now = time.elapsed_secs_f64();
    let warm = *p.warm.get_or_insert(now);
    if now - warm < 2.0 {
        return;
    }
    if p.start.is_none() {
        gas.timings.extract_peak_ns.store(0, Ordering::Relaxed);
        gas.timings.prepare_peak_ns.store(0, Ordering::Relaxed);
        gas.timings.allocation_peak_ns.store(0, Ordering::Relaxed);
    }
    let start = *p.start.get_or_insert(now);
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
    p.mark = Some(Instant::now());
}
pub fn stamp(mut p: ResMut<Profile>) {
    if p.start.is_some() && !p.finished {
        p.stamp();
    }
}
pub fn end(
    time: Res<Time<Real>>,
    scene: Res<Scene>,
    gas: Res<GasInput>,
    art: Res<WitnessArt>,
    diagnostics: Option<Res<bevy::diagnostic::DiagnosticsStore>>,
    mut p: ResMut<Profile>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(start) = p.start else {
        return;
    };
    if p.finished {
        return;
    }
    p.stamp();
    if p.gpu {
        if let Some(diagnostics) = diagnostics {
            for d in diagnostics.iter() {
                let path = d.path().to_string();
                if !path.ends_with("/elapsed_gpu") {
                    continue;
                }
                if let Some(m) = d.measurement() {
                    let samples = p.gpu_samples.entry(path).or_default();
                    if samples.last != Some(m.time) {
                        samples.last = Some(m.time);
                        samples.values.push(m.value);
                    }
                }
            }
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
    ];
    if elapsed > 0.0 {
        p.rows.push((elapsed, scene.sim.tick, phases));
    }
    if elapsed < p.seconds {
        return;
    }
    p.finished = true;
    if p.actions && p.action_step < 8 {
        eprintln!(
            "Interaction profile ended before all actions completed: {}/8",
            p.action_step
        );
        exit.write(AppExit::error());
        return;
    }
    let result = (|| -> std::io::Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(p.output.as_ref().unwrap())?);
        writeln!(
            out,
            "seconds,tick,frame_ms,simulation_input_ms,unit_mesh_ms,fragments_ms,presentation_ms,physics_ms,terrain_ms,atmosphere_ms,render_prepare_ms,gas_allocation_ms,gas_regions,render_extract_ms,occupancy_ms"
        )?;
        for (t, tick, ms) in &p.rows {
            writeln!(
                out,
                "{t:.6},{tick},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.0},{:.6},{:.6}",
                ms[0], ms[1], ms[2], ms[3], ms[4], ms[5], ms[6], ms[7], ms[8], ms[9], ms[10], ms[11], ms[12]
            )?;
        }
        out.flush()
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
    .chain([(11, "render_extract"), (12, "occupancy")])
    {
        let mut values: Vec<_> = p.rows.iter().map(|r| r.2[column]).collect();
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
    println!(
        "PROFILE max_gas_regions={}",
        p.rows.iter().map(|r| r.2[10] as usize).max().unwrap_or(0)
    );
    println!(
        "PROFILE render_extract_peak_ms={:.3}",
        gas.timings.extract_peak_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0
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
    if p.gpu {
        for (name, samples) in &mut p.gpu_samples {
            let v = &mut samples.values;
            v.sort_by(f64::total_cmp);
            if !v.is_empty() {
                println!(
                    "GPU {name} n={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}",
                    v.len(),
                    v[v.len() / 2],
                    v[v.len() * 95 / 100],
                    v[v.len() * 99 / 100],
                    v[v.len() - 1]
                );
            }
        }
        if p.gpu_samples.is_empty() {
            eprintln!("GPU timestamps unavailable on this device/backend");
        }
    }
    if let Err(e) = result {
        eprintln!("profile output: {e}");
        exit.write(AppExit::error());
    } else {
        exit.write(AppExit::Success);
    }
}
