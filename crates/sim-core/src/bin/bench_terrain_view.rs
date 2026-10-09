//! Copy/reuse/drop cost of eight owned terrain rows in an actual AI battle.
//! Measurements never enter the deterministic simulation library.
use sim_core::blast::{
    read_model::terrain::{Budget, Changes, Request, Row},
    BlastRange,
};
use std::{hint::black_box, time::Instant};

fn measure(world: &BlastRange, name: &str) {
    let before = world.state_hash();
    let width = 1280usize;
    let height = world.size()[1] as usize;
    let budget = Budget {
        columns: width,
        cells: width * height,
    };
    let z = world
        .surface_work
        .keys()
        .find(|p| {
            p[0] >= 4496
                && p[0] < 5776
                && p[1] >= 0
                && p[1] < height as i32
                && p[2] >= 0
                && p[2] < world.size()[2] as i32 - 8
        })
        .map_or(5472, |p| p[2]);
    let request = |i: usize| Request {
        x: 4496..5776,
        y: 0..height as i32,
        z: z + i as i32,
    };
    let mut rows: Vec<_> = (0..8).map(|_| Row::default()).collect();
    for (i, row) in rows.iter_mut().enumerate() {
        row.update(world, 1, request(i), budget).unwrap();
    }
    let payload = rows.iter().map(Row::payload_bytes).sum::<usize>();
    let materials = rows.iter().map(|r| r.materials.len()).sum::<usize>();
    let damage = rows.iter().map(|r| r.damage.len()).sum::<usize>();
    let start = Instant::now();
    for _ in 0..200 {
        for (i, row) in rows.iter_mut().enumerate() {
            row.update(black_box(world), 1, request(i), budget).unwrap();
        }
        black_box(&rows);
    }
    let reuse_ns = start.elapsed().as_nanos() / 200;
    let mut copy_ns = 0;
    let mut drop_ns = 0;
    for _ in 0..100 {
        let start = Instant::now();
        let fresh: Vec<_> = (0..8)
            .map(|i| {
                let mut row = Row::default();
                row.update(black_box(world), 1, request(i), budget).unwrap();
                row
            })
            .collect();
        black_box(&fresh);
        copy_ns += start.elapsed().as_nanos();
        let start = Instant::now();
        drop(black_box(fresh));
        drop_ns += start.elapsed().as_nanos();
    }
    let mut changes = Changes::default();
    let start = Instant::now();
    for _ in 0..2000 {
        changes
            .update(black_box(world), 1, 4496..5776, 4800..6080, 1280)
            .unwrap();
        black_box(&changes);
    }
    let changes_ns = start.elapsed().as_nanos() / 2000;
    assert_eq!(world.state_hash(), before);
    println!("case={name} tick={} rows=8 columns={} budget_cells={} captured_cells={materials} damage_records={damage} batch_reuse_mean_ns={reuse_ns} batch_capture_mean_ns={} batch_drop_mean_ns={} batch_logical_bytes={payload} changes_mean_ns={changes_ns} changed_rows={} changes_logical_bytes={} physical_hash={before:016x}",world.tick,width*8,width*height*8,copy_ns/100,drop_ns/100,changes.rows.len(),changes.payload_bytes());
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
    measure(&world, "early");
    for _ in 0..3300 {
        world.step();
    }
    assert!(world.battle.actors.iter().any(|a| a.fired > 0));
    assert!(!world.surface_work.is_empty());
    measure(&world, "late");
}
