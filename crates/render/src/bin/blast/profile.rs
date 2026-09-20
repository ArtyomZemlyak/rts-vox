//! Frame timings without screenshots, PNG encoding or GPU readback.
use super::*;
use std::{io::Write, time::Instant};

#[derive(Resource, Default)]
pub struct Profile {
    pub output: Option<String>,
    pub seconds: f64,
    warm: Option<f64>,
    start: Option<f64>,
    mark: Option<Instant>,
    phases: Vec<f64>,
    detail: [f64; 3],
    rows: Vec<(f64, u32, [f64; 8])>,
    finished: bool,
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
pub fn begin(time: Res<Time<Real>>, gas: Res<GasInput>, mut p: ResMut<Profile>) {
    if p.output.is_none() || !gas.ready.load(Ordering::Relaxed) {
        return;
    }
    let now = time.elapsed_secs_f64();
    let warm = *p.warm.get_or_insert(now);
    if now - warm < 2.0 {
        return;
    }
    p.start.get_or_insert(now);
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
    ];
    if elapsed > 0.0 {
        p.rows.push((elapsed, scene.sim.tick, phases));
    }
    if elapsed < p.seconds {
        return;
    }
    p.finished = true;
    let result = (|| -> std::io::Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(p.output.as_ref().unwrap())?);
        writeln!(
            out,
            "seconds,tick,frame_ms,simulation_input_ms,unit_mesh_ms,fragments_ms,presentation_ms,physics_ms,terrain_ms,atmosphere_ms"
        )?;
        for (t, tick, ms) in &p.rows {
            writeln!(
                out,
                "{t:.6},{tick},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
                ms[0], ms[1], ms[2], ms[3], ms[4], ms[5], ms[6], ms[7]
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
    ]
    .into_iter()
    .enumerate()
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
    if let Err(e) = result {
        eprintln!("profile output: {e}");
        exit.write(AppExit::error());
    } else {
        exit.write(AppExit::Success);
    }
}
