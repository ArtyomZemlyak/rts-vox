//! CPU-only measurement of the burst's push on a hull (P-18, P-19, P-20):
//! a shell beside a parked or walking tank or walker, then the hull's
//! height, climb, landing and drift tick by tick, and the heap columns
//! under it at the end. `--weapon energy|shell|anni`, `--energy n`
//! overrides the wave energy, `--kind tank|strider`, `--walking` gives the
//! unit a patrol so the burst meets it on the move, `--gap n` puts the
//! burst that many units from the hull's centre — ahead of it, or off its
//! flank with `--side` — `--above n` that many units above the floor,
//! `--ticks n` how long to watch. The tank's box is 30 by 15 half-units.
use sim_core::blast::{BlastRange, FLOOR};

fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let weapon = arg(&args, "--weapon").unwrap_or_else(|| "shell".into());
    let ron = match weapon.as_str() {
        "energy" => include_str!("../../../../data/energy-cannon.ron"),
        "shell" => include_str!("../../../../data/explosion.ron"),
        "anni" => include_str!("../../../../data/anni-cannon.ron"),
        _ => panic!("--weapon expects energy, shell or anni"),
    };
    let kind = match arg(&args, "--kind").as_deref().unwrap_or("tank") {
        "tank" => modding::range::RangeUnitKind::Tank,
        "strider" => modding::range::RangeUnitKind::Strider,
        other => panic!("--kind expects tank or strider, not {other}"),
    };
    let walking = args.iter().any(|a| a == "--walking");
    let side = args.iter().any(|a| a == "--side");
    let gap: i32 = arg(&args, "--gap")
        .and_then(|s| s.parse().ok())
        .unwrap_or(14);
    let above: i32 = arg(&args, "--above")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let ticks: u32 = arg(&args, "--ticks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let mut config = modding::explosion::ExplosionConfig::from_ron(ron).unwrap();
    if let Some(e) = arg(&args, "--energy").and_then(|s| s.parse().ok()) {
        config.energy = e;
    }
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron"))
            .unwrap();
    // The unit stands in the cleared ground in front of the wall, the
    // burst west of it; walking, it patrols north and back through the
    // burst's spot.
    let at = [359, FLOOR, 312];
    layout.units = vec![modding::range::RangeUnitDef {
        weapon: None,
        allied: false,
        kind,
        at,
    }];
    layout.target = if side {
        [at[0], FLOOR + above, at[2] - gap]
    } else {
        [at[0] - gap, FLOOR + above, at[2]]
    };
    layout.blocks.clear();
    layout.airburst_on_target = false;
    if let Some(c) = &mut layout.course {
        c.patrols.clear();
        if walking {
            // The course must hold the patrol with a margin of 32.
            c.min = [at[0] - 80, at[2] - 52];
            c.max = [at[0] + 80, at[2] + 108];
            c.ramps.clear();
            c.patrols.push(modding::range::Patrol {
                unit: 0,
                points: vec![[at[0], at[2] + 60], [at[0], at[2]]],
            });
        }
    }
    let energy = config.energy;
    let milli = config.hull_impulse_milli;
    let mut scene = BlastRange::with_layout(config, layout).unwrap();
    let start = scene.units[0].motion.pos;
    let level = scene.units[0].motion.hull.y;
    let mut impact = None;
    for _ in 0..300 {
        scene.step();
        if scene.impact.is_some() {
            impact = Some(scene.tick);
            break;
        }
    }
    let impact = impact.expect("no impact");
    let at_impact = scene.units[0].motion.pos;
    let mut rise = fx::Fx::ZERO;
    let mut rise_tick = 0;
    let mut shove_peak = fx::Fx::ZERO;
    let mut vy_peak = fx::Fx::ZERO;
    let mut vy_low = fx::Fx::ZERO;
    let mut landed = None;
    let mut trail = Vec::new();
    let mut pushes = Vec::new();
    for i in 0..ticks {
        scene.step();
        if let Some(push) = scene.units[0].last_push.take() {
            pushes.push(format!("{i}:{push:?}"));
        }
        let m = &scene.units[0].motion;
        let h = m.hull.y.sub(level);
        if h > rise {
            rise = h;
            rise_tick = i;
        }
        vy_peak = vy_peak.max(m.hull.vy);
        vy_low = vy_low.min(m.hull.vy);
        shove_peak = shove_peak.max(m.shove.len_sq().sqrt());
        if landed.is_none()
            && rise > fx::Fx::from_ratio(1, 10)
            && h.abs() < fx::Fx::from_ratio(1, 10)
            && m.hull.vy.abs() < fx::Fx::from_ratio(1, 2)
        {
            landed = Some(i);
        }
        if i < 20 || i.is_multiple_of(15) {
            trail.push(format!(
                "{i}:y{:.2}/vy{:.2}/x{:.1}/s{:.1}/w{:.2}/up{:.2}/ld{:.2}",
                h.to_f64_lossy(),
                m.hull.vy.to_f64_lossy(),
                m.pos[0].sub(at_impact[0]).to_f64_lossy(),
                m.shove.len_sq().sqrt().to_f64_lossy(),
                m.hull.w.len_sq().sqrt().to_f64_lossy(),
                m.hull.up.y.to_f64_lossy(),
                m.hull.load.to_f64_lossy()
            ));
        }
    }
    let u = &scene.units[0];
    let m = &u.motion;
    // Heap columns under the hull's box at the end, and the tallest.
    let half = m.probe_shape(&u.body).half;
    let (px, pz) = (m.pos[0].to_int_trunc(), m.pos[2].to_int_trunc());
    let under: Vec<u16> = scene
        .rubble
        .iter()
        .filter(|([x, z], _)| (x - px).abs() <= half[0] + 1 && (z - pz).abs() <= half[1] + 1)
        .map(|(_, top)| *top)
        .collect();
    println!(
        "weapon={weapon} kind={kind:?} walking={walking} energy={energy} milli={milli} gap={gap} side={side} above={above} impact_tick={impact} alive={} rise={:.2}@{rise_tick} vy_peak={:.2} vy_low={:.2} shove_peak={:.2} landed_after={:?} drift_x={:.2} drift_z={:.2} final_y={:.2} up_y={:.3} heap_under={} heap_top={:?} floor={}",
        u.status.alive,
        rise.to_f64_lossy(),
        vy_peak.to_f64_lossy(),
        vy_low.to_f64_lossy(),
        shove_peak.to_f64_lossy(),
        landed,
        m.pos[0].sub(at_impact[0]).to_f64_lossy(),
        m.pos[2].sub(at_impact[2]).to_f64_lossy(),
        m.hull.y.sub(level).to_f64_lossy(),
        m.hull.up.y.to_f64_lossy(),
        under.len(),
        under.iter().max(),
        start[1].to_int_trunc()
    );
    println!("pushes {}", pushes.join(" "));
    println!("trail {}", trail.join(" "));
}
