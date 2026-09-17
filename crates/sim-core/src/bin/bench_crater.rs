//! CPU-only measurement of a crater's shape (P-21): a shell on open ground
//! at several energies, then the crater's radius along the eight
//! directions of the ground plane, and how far the wave grid let it grow.
//! `--powers a,b,c` names the powers tried, thousandths, scaling the wave
//! energy and the material energy as the window does; `--material k`
//! scales the material energy on top, to tell the piece budget's limit
//! from the grid's; `--weapon energy|shell|anni` picks the weapon.
use sim_core::blast::{BlastRange, FLOOR};

fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let powers: Vec<u32> = arg(&args, "--powers")
        .unwrap_or_else(|| "1000,2000,4000".into())
        .split(',')
        .map(|s| s.trim().parse().expect("power"))
        .collect();
    let weapon = arg(&args, "--weapon").unwrap_or_else(|| "shell".into());
    let ron = match weapon.as_str() {
        "energy" => include_str!("../../../../data/energy-cannon.ron"),
        "shell" => include_str!("../../../../data/explosion.ron"),
        "anni" => include_str!("../../../../data/anni-cannon.ron"),
        _ => panic!("--weapon expects energy, shell or anni"),
    };
    for power in powers {
        let mut config = modding::explosion::ExplosionConfig::from_ron(ron).unwrap();
        config.energy = config.energy * power / 1000;
        config.material_energy_j =
            (u64::from(config.material_energy_j) * u64::from(power) / 1000) as u32;
        if let Some(k) = arg(&args, "--material").and_then(|s| s.parse::<u32>().ok()) {
            config.material_energy_j = config.material_energy_j.saturating_mul(k);
        }
        let energy = config.energy;
        let material = config.material_energy_j;
        let half_cells = config.half_cells;
        let cell_units = config.cell_units;
        let mut layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron"))
                .unwrap();
        // Open ground in a clearing widened to hold the biggest crater,
        // away from the wall and the units.
        let at = [350, FLOOR, 350];
        layout.clear_min = [150, 150];
        layout.clear_max = [550, 550];
        layout.units.clear();
        layout.target = [at[0], at[1] - 1, at[2]];
        layout.blocks.clear();
        layout.airburst_on_target = false;
        if let Some(c) = &mut layout.course {
            c.patrols.clear();
        }
        let mut scene = BlastRange::with_layout(config, layout).unwrap();
        let mut impact = None;
        for _ in 0..300 {
            scene.step();
            if scene.impact.is_some() {
                impact = Some(scene.tick);
                break;
            }
        }
        let impact_tick = impact.expect("no impact");
        let centre = scene.impact.unwrap();
        let grid_cells = scene.explosion.as_ref().map_or(0, |e| e.wave.size());
        // Let the wave finish and the world take every edit.
        for _ in 0..40 {
            scene.step();
        }
        let solid = |x: i32, y: i32, z: i32| {
            x >= 0
                && z >= 0
                && y >= 0
                && (x as u32) < scene.world.nx
                && (z as u32) < scene.world.nz
                && (y as u32) < scene.world.ny
                && scene.world.material(x as u32, y as u32, z as u32) != 0
        };
        // Along each of the eight ground directions, one cell below the
        // old surface: the first solid cell ends the crater.
        let y = FLOOR - 1;
        let dirs = [
            (1, 0),
            (1, 1),
            (0, 1),
            (-1, 1),
            (-1, 0),
            (-1, -1),
            (0, -1),
            (1, -1),
        ];
        let radii: Vec<f64> = dirs
            .iter()
            .map(|&(dx, dz)| {
                let mut r = 0;
                while r < 200 && !solid(centre[0] + dx * r, y, centre[2] + dz * r) {
                    r += 1;
                }
                let along = ((dx * dx + dz * dz) as f64).sqrt();
                r as f64 * along
            })
            .collect();
        let lo = radii.iter().cloned().fold(f64::MAX, f64::min);
        let hi = radii.iter().cloned().fold(0.0, f64::max);
        let axis = (radii[0] + radii[2] + radii[4] + radii[6]) / 4.0;
        let diagonal = (radii[1] + radii[3] + radii[5] + radii[7]) / 4.0;
        let grid = (grid_cells as i32 - 1) / 2 * cell_units;
        let _ = half_cells;
        println!(
            "weapon={weapon} power={power} energy={energy} material={material} impact_tick={impact_tick} centre={centre:?} removed={} grid_cells={grid_cells} grid_half={grid} radii={:?} axis={axis:.1} diagonal={diagonal:.1} spread_pct={:.0} at_grid_edge={}",
            scene.removed,
            radii.iter().map(|r| format!("{r:.0}")).collect::<Vec<_>>(),
            (hi - lo) / hi.max(1.0) * 100.0,
            axis >= (grid - 1) as f64
        );
    }
}
