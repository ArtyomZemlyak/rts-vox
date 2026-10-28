//! Bounded mechanical siege-hit regression stand. No window or GPU.
//! cargo run --release -p sim-core --bin bench_armour -- roof 8 1
use modding::{
    explosion::ExplosionConfig,
    range::{RangeLayout, RangeUnitDef, RangeUnitKind},
};
use sim_core::blast::{BlastRange, Explosion};
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("roof");
    assert!(["roof", "spread", "barrel", "hip"].contains(&mode));
    let shots: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(8);
    let workers: usize = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(1);
    let kind = match args.get(4).map(String::as_str).unwrap_or("spider") {
        "spider" => RangeUnitKind::Spider,
        "strider" => RangeUnitKind::Strider,
        "t2" => RangeUnitKind::ArtilleryT2,
        "tank" => RangeUnitKind::Tank,
        _ => panic!("unknown kind"),
    };
    assert!((1..=32).contains(&shots) && (1..=4).contains(&workers));
    let c = ExplosionConfig::from_ron(include_str!("../../../../data/siege-shell.ron")).unwrap();
    let mut l =
        RangeLayout::from_ron(include_str!("../../../../data/fixture-corpses.ron")).unwrap();
    l.battle = false;
    l.size = [1024, 256, 1024];
    l.target = [600, 80, 512];
    l.clear_min = [64, 64];
    l.clear_max = [960, 960];
    l.units = vec![RangeUnitDef {
        kind,
        allied: false,
        at: [600, 16, 512],
        weapon: None,
    }];
    let mut r = BlastRange::with_layout(c, l).unwrap();
    assert!(r.set_physics_workers(workers));
    assert!(r.set_navigation_workers(1));
    r.player = true;
    r.auto_attack = false;
    r.fire_tick = u32::MAX;
    for _ in 0..20 {
        r.step();
    }
    println!("shot,mode,kind,alive,engine,weapon,core_pct,drive_pct,removed,seg_removed,tick_ms_max,tick_ms_mean,hash");
    for shot in 0..shots {
        let cell = if kind != RangeUnitKind::Spider {
            let b = &r.units[0].body;
            [
                usize::from(b.core.0),
                usize::from(b.vol.ny) - 2,
                usize::from(b.core.2),
            ]
        } else {
            match mode {
                "roof" => [44, 42, 40],
                "barrel" => [76, 48, 40],
                "hip" => [22, 31, 24],
                _ => [
                    [28, 40, 28],
                    [60, 42, 48],
                    [44, 42, 40],
                    [22, 39, 46],
                    [68, 40, 34],
                    [36, 42, 52],
                    [52, 42, 28],
                    [40, 40, 40],
                ][shot % 8],
            }
        };
        let at = r.units[0].world_pos(cell);
        r.matter
            .begin(r.config.material_energy_j, r.config.fragment_box);
        r.explosion = Some(Explosion::begin_material(
            at,
            &r.config,
            &r.units,
            r.matter.tuning.body_energy_pct,
        ));
        let mut times = Vec::new();
        for tick in 0..128 {
            let now = Instant::now();
            r.step();
            times.push(now.elapsed().as_secs_f64() * 1000.);
            assert!(r.matter.economy.ledger.balanced());
            if tick >= 39 && r.explosion.as_ref().is_none_or(|e| !e.working()) {
                break;
            }
        }
        assert!(!r.explosion.as_ref().unwrap().working());
        let u = &r.units[0];
        println!(
            "{},{},{:?},{},{},{},{},{},{},{},{:.3},{:.3},{:016x}",
            shot + 1,
            mode,
            kind,
            u.status.alive,
            u.status.engine,
            u.status.weapon,
            u.integrity.core.mul(fx::Fx::from_int(100)).to_int_trunc(),
            u.integrity.drive.mul(fx::Fx::from_int(100)).to_int_trunc(),
            u.removed.len(),
            u.seg_removed.len(),
            times.iter().copied().fold(0., f64::max),
            times.iter().sum::<f64>() / times.len() as f64,
            r.state_hash()
        );
    }
}
