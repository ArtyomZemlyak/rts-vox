//! Headless acceptance run for the explosion range.
fn main() {
    let energy = std::env::args().skip(1).any(|a| a == "--energy");
    let c = modding::explosion::ExplosionConfig::from_ron(if energy {
        include_str!("../../../../data/energy-cannon.ron")
    } else {
        include_str!("../../../../data/explosion.ron")
    })
    .unwrap();
    let course = std::env::args().any(|a| a == "--course");
    let cover = course || std::env::args().any(|a| a == "--cover");
    let mut r = if cover {
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron"))
                .unwrap();
        sim_core::blast::BlastRange::with_layout(c, layout).unwrap()
    } else {
        sim_core::blast::BlastRange::new(c)
    };
    let mut times = Vec::new();
    if course {
        r.control();
    }
    let count = if course { 2400 } else { 300 };
    for _ in 0..count {
        let t = std::time::Instant::now();
        r.step();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    println!("impact={:?} tick={} removed={} pending={} hash={:016x} tick_median_ms={:.3} tick_p99_ms={:.3} tick_max_ms={:.3}",r.impact,r.impact_tick,r.removed,r.world.queued(),r.state_hash(),times[count/2],times[count*99/100],times[count-1]);
    println!("removed materials={:?}", r.removed_materials);
    let matter = &r.matter;
    let debris = &matter.debris;
    println!("physical fragments={} resting={} cells={} mass_g={} spent_mj={} supplied_mj={} motion_mj={} ledger_balanced={} sunk={}",
        debris.fragments.len(),debris.fragments.iter().filter(|f|f.resting).count(),debris.fragments.iter().map(|f|u64::from(f.count)).sum::<u64>(),
        debris.fragments.iter().map(|f|f.mass_g).sum::<u64>(),matter.spent_mj,matter.supplied_mj,debris.motion_mj+debris.body_motion_mj,
        matter.economy.ledger.balanced(),matter.economy.ledger.sunk_total());
    if std::env::args().any(|a| a == "--fragments") {
        for f in &debris.fragments {
            println!(
                "fragment={} pos={:?} count={} material={} resting={}",
                f.id,
                [f.position.x, f.position.y, f.position.z].map(|v| v.to_f64_lossy()),
                f.count,
                f.material,
                f.resting
            );
        }
    }
    println!(
        "surviving damaged steel={} body dents={:?}",
        r.surface_work
            .keys()
            .filter(|p| sim_core::blast::material(&r.world, **p) == 3)
            .count(),
        r.units.iter().map(|u| u.dents.len()).collect::<Vec<_>>()
    );
    if course {
        for (i, u) in r.units.iter().enumerate() {
            println!(
                "actor={i} at={:?} laps={} blocked={}",
                u.at, u.motion.laps, u.motion.blocked
            );
        }
        assert!(r.units[2..].iter().all(|u| u.motion.laps >= 2));
        return;
    }
    assert!(r.impact.is_some() && r.removed > 100 && r.world.queued() == 0);
    for (id, u) in r.units.iter().enumerate() {
        println!(
            "unit {id}: removed={} remaining={} annihilated_mm3={} status={:?}",
            u.removed.len(),
            u.body.solid_count(),
            u.annihilated_mm3(),
            u.status
        );
        if id == 0 {
            assert!(!u.removed.is_empty(), "exposed unit took no spatial damage");
        }
        if cover && id == 1 {
            assert!(u.removed.is_empty(), "cover did not protect the rear unit");
        }
    }
}
