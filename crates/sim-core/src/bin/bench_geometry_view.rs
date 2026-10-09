//! Cost and logical private payload of unit publication at early/late battle.
//! Clocks and measurements stay outside the deterministic simulation library.
use sim_core::blast::{
    read_model::geometry::{Cursor, Sampling, View},
    BlastRange,
};
use std::{hint::black_box, num::NonZeroUsize, time::Instant};

fn measure(world: &mut BlastRange, name: &str) {
    let before = world.state_hash();
    let mut view = View::default();
    let consumed = world
        .units
        .iter()
        .map(|u| u.removed.len())
        .collect::<Vec<_>>();
    let sampling = Sampling {
        stride: NonZeroUsize::new(97).unwrap(),
        per_unit: 32,
    };
    view.update(
        world,
        1,
        Cursor {
            generation: 1,
            counts: &consumed,
        },
        sampling,
    );
    let start = Instant::now();
    for _ in 0..20_000 {
        view.update(
            black_box(world),
            1,
            Cursor {
                generation: 1,
                counts: &consumed,
            },
            sampling,
        );
        black_box(&view);
    }
    let metadata_ns = start.elapsed().as_nanos() / 20_000;
    let metadata_bytes = view.payload_bytes();
    let mut copy_ns = 0;
    let mut drop_ns = 0;
    let mut payloads = Vec::new();
    for round in 0..20 {
        for id in 0..world.units.len() {
            let start = Instant::now();
            let mesh = black_box(view.mesh(black_box(world), 1, id).unwrap());
            copy_ns += start.elapsed().as_nanos();
            if round == 0 {
                payloads.push(mesh.private_payload_bytes());
            }
            let start = Instant::now();
            drop(black_box(mesh));
            drop_ns += start.elapsed().as_nanos();
        }
    }
    payloads.sort_unstable();
    let peak_pair = payloads.iter().rev().take(2).sum::<usize>();
    let calls = world.units.len() as u128 * 20;
    let config = world.config;
    let impact = world.impact;
    world.config.material_energy_j = 0;
    world.impact = Some([300, 24, 336]);
    // Diagnostic presentation-only worst case, outside the stock battle:
    // replay the retained removals with the same bounded cosmetic policy.
    let start = Instant::now();
    for _ in 0..2_000 {
        view.update(
            black_box(world),
            1,
            Cursor {
                generation: 1,
                counts: &[],
            },
            sampling,
        );
        black_box(&view);
    }
    let sampled_ns = start.elapsed().as_nanos() / 2_000;
    let sampled_bytes = view.payload_bytes();
    let sampled_records = view.removed.len();
    world.config = config;
    world.impact = impact;
    assert_eq!(world.state_hash(), before);
    println!("case={name} tick={} units={} metadata_mean_ns={metadata_ns} metadata_bytes={metadata_bytes} mesh_mean_ns={} mesh_drop_mean_ns={} largest_mesh_private_bytes={} max_two_meshes_private_bytes={peak_pair} sampled_mean_ns={sampled_ns} sampled_bytes={sampled_bytes} sampled_records={sampled_records} physical_hash={before:016x}", world.tick, world.units.len(), copy_ns/calls, drop_ns/calls, payloads.last().unwrap());
}

fn main() {
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-km.ron"))
            .unwrap();
    let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
        "../../../../data/battle-world.ron"
    ))
    .unwrap();
    layout.battle = true;
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    layout.units = preset.units;
    layout.blocks.extend(preset.blocks);
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let mut world = BlastRange::with_layout(config, layout).unwrap();
    assert!(world.set_ai_vs_ai(true));
    assert!(world.set_navigation_workers(8));
    world.start_battle().unwrap();
    measure(&mut world, "early");
    for _ in 0..3300 {
        world.step();
    }
    assert!(world.battle.actors.iter().any(|a| a.fired > 0));
    assert!(world.units.iter().any(|u| !u.removed.is_empty()));
    measure(&mut world, "late");
}
