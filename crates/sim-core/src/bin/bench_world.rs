//! What the range's world costs to build, by the side of the map (P-34).
//! CPU only: no frames, no GPU. Prints the side, the time to build the
//! scene, and the dense structures that grow with it, so that a bigger
//! range is decided by numbers rather than by guesswork.
use sim_core::blast::BlastRange;
use std::time::Instant;

fn main() {
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let started = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let layout = args.windows(2).find(|a| a[0] == "--range").map(|a| {
        modding::range::RangeLayout::from_ron(&std::fs::read_to_string(&a[1]).unwrap()).unwrap()
    });
    let scene = match layout {
        Some(r) => BlastRange::with_layout(config, r).unwrap(),
        None => BlastRange::new(config),
    };
    let size = scene.size();
    let build = started.elapsed().as_millis();
    let (nx, ny, nz) = (u64::from(size[0]), u64::from(size[1]), u64::from(size[2]));
    let cells = nx * ny * nz;
    // The dense structures, in bytes: the occupancy volume (a bit a cell),
    // the column ceiling (ADR-313) and the window of columns a single route
    // search allocates over the whole map (`Probe::with_yield`).
    let bits = cells / 8;
    let ceiling = nx * nz * 2;
    let window = nx * nz * 6;
    // What one route search would allocate for its own state if it held
    // every cell of the planner grid: eight headings a cell, three arrays
    // over them. It holds only the cells it reaches now (ADR-316), so this
    // column is what the map no longer costs.
    let nav = (nx / 8) * (nz / 8) * 8;
    let search = nav * (4 + 8 + 64);
    let mb = |v: u64| v as f64 / 1e6;
    println!(
        "side={nx} height={ny} cells={cells} build_ms={build} bits_mb={:.1} ceiling_mb={:.1} probe_window_mb={:.1} dense_search_state_mb={:.1}",
        mb(bits),
        mb(ceiling),
        mb(window),
        mb(search)
    );
    // Local collision/impact queries, including air above the authored sectors.
    let query_start = Instant::now();
    let mut checksum = 0u64;
    for i in 0..1_000_000 {
        let p = [
            (i * 37 % size[0]) as i32,
            24 + (i % 72) as i32,
            (i * 53 % size[2]) as i32,
        ];
        checksum += u64::from(sim_core::blast::material(&scene.world, p));
    }
    println!(
        "material_queries=1000000 query_us={} checksum={checksum}",
        query_start.elapsed().as_micros()
    );
    // Touch the world so nothing above is thrown away as unused.
    println!(
        "floor={} removed={}",
        sim_core::blast::material(&scene.world, [10, 17, 10]),
        scene.removed
    );
}
