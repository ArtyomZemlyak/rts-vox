//! Load stand for material combat: two blocks of material tanks shelling
//! each other with live rounds, no rendering.
//!
//! Prints per-size tick p50/p99/max, the deepest queues, how many bodies
//! actually took damage, fragments in flight and at rest, peak heap use and
//! the final hash. Run: `cargo run --release -p sim-core --bin bench_material
//! [-- --sides 16,48,112 --ticks 600]` (tanks per side).
use sim_core::{Order, Reference, Tuning};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
// SAFETY: forwards every call to the system allocator and only keeps counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Relaxed).wrapping_add(l.size());
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Relaxed);
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static ALLOC: Counting = Counting;

fn arg(name: &str, default: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
        .unwrap_or_else(|| default.to_string())
}

fn main() {
    let sides: Vec<usize> = arg("--sides", "16,48,112")
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let ticks: usize = arg("--ticks", "600").parse().expect("--ticks");
    // Blast field samples per tick; 0 keeps data/sim.ron.
    let samples: u32 = arg("--samples", "0").parse().expect("--samples");
    for per_side in sides {
        run(per_side, ticks, samples);
    }
}

fn run(per_side: usize, ticks: usize, samples: u32) {
    let mut t = Tuning::from_ron(include_str!("../../../../data/sim.ron")).unwrap();
    if samples > 0 {
        t.blast.samples_per_tick = samples;
    }
    let weapons = modding::weapons::from_ron(include_str!("../../../../data/weapons.ron")).unwrap();
    let kinds = modding::kinds::from_ron(include_str!("../../../../data/kinds.ron"), 30).unwrap();
    let mats =
        modding::data::materials_from_ron(include_str!("../../../../data/materials.ron")).unwrap();
    let mut params = worldgen::Params::sized(1024, 128, 1024);
    params.relief = 1;
    let build = Instant::now();
    let mut r = Reference::with_params(42, 0, t, weapons, params);
    r.world.attach_economy(kinds, mats);
    let build_ms = build.elapsed().as_millis();
    // Four columns per side along x, rows every 32 units along z; the
    // blocks face each other across 62 m, inside the 80 m reach of the
    // tank's material round.
    let mut ids = Vec::new();
    for owner in 0..2u8 {
        for k in 0..per_side {
            let col = (k % 4) as i32;
            let row = (k / 4) as i32;
            let x = if owner == 0 {
                160 + col * 70
            } else {
                864 - col * 70
            };
            let z = 512 - 16 * (per_side as i32 / 4).max(1) + row * 32;
            let id = r
                .world
                .spawn_kind(x, z.clamp(40, 984), owner, 1)
                .expect("spawn");
            r.world.push_order(Order::Hold { unit: id });
            ids.push(id);
        }
    }
    r.world.units.refresh_live();
    r.world.tick();
    // Heap held by the built scene; growth during the run is what counts.
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let mut samples = Vec::with_capacity(ticks);
    let mut deepest = [0usize; 4];
    let mut hash = 0;
    for _ in 0..ticks {
        let start = Instant::now();
        hash = r.world.tick();
        samples.push(start.elapsed().as_micros() as u64);
        let w = &r.world;
        deepest[0] = deepest[0].max(w.bursts.pending());
        deepest[1] = deepest[1].max(w.bursts.claims());
        deepest[2] = deepest[2].max(w.material_bodies.as_ref().map_or(0, |b| b.detach_pending()));
        deepest[3] = deepest[3].max(w.material_bodies.as_ref().map_or(0, |b| b.pending_damage()));
    }
    samples.sort_unstable();
    let w = &r.world;
    let bodies = w.material_bodies.as_ref().expect("material bodies");
    let damaged = ids
        .iter()
        .filter(|&&id| bodies.state(id).is_some_and(|s| !s.removed.is_empty()))
        .count();
    let alive = ids.iter().filter(|&&id| w.units.alive[id as usize]).count();
    let flying = w.bursts.fragments.iter().filter(|f| !f.resting).count();
    let resting = w.bursts.fragments.len() - flying;
    let phases: Vec<(usize, u64)> = {
        let mut v: Vec<(usize, u64)> = w.phase_ns.iter().copied().enumerate().collect();
        v.sort_by_key(|(_, ns)| std::cmp::Reverse(*ns));
        v.into_iter().take(3).collect()
    };
    println!(
        "tanks={} samples={} ticks={} build_ms={build_ms} p50_us={} p99_us={} max_us={} alive={alive} damaged={damaged} \
         flying={flying} resting={resting} deepest[jobs,claims,detach,damage]={deepest:?} \
         scene_mb={:.1} peak_growth_mb={:.1} live_growth_mb={:.1} phases_top3_ms_per_tick={:?} \
         hash={hash:016x} balanced={}",
        ids.len(),
        t.blast.samples_per_tick,
        samples.len(),
        samples[samples.len() / 2],
        samples[samples.len() * 99 / 100],
        samples[samples.len() - 1],
        base as f64 / 1e6,
        PEAK.load(Relaxed).saturating_sub(base) as f64 / 1e6,
        LIVE.load(Relaxed).saturating_sub(base) as f64 / 1e6,
        phases
            .iter()
            .map(|(i, ns)| (*i, *ns as f64 / 1e6 / samples.len() as f64))
            .collect::<Vec<_>>(),
        w.econ.as_ref().is_none_or(|e| e.balanced())
    );
}
