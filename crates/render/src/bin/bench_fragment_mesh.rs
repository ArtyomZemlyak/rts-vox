//! Headless A/B of the old independent cubes and the common debris surface.
//! Input is an actual saved battle; no GPU, window or simulation steps.
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use std::{collections::BTreeMap, hint::black_box, time::Instant};

fn surface(shape: u64, side: u8) -> Mesh {
    let b = render::model::fragment_to_mesh(shape, side, [1.0; 4]);
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.pos.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, b.col.clone())
    .with_inserted_indices(Indices::U32(b.idx.clone()))
}
fn legacy(shape: u64, side: u8) -> Mesh {
    let c = sim_core::burst::shape_centre(shape, side);
    let centre = Vec3::from_array([c.x, c.y, c.z].map(|v| v.to_f64_lossy() as f32));
    let side = u32::from(side.clamp(1, 4));
    let mut out: Option<Mesh> = None;
    for i in 0..side.pow(3) {
        if shape & (1 << i) == 0 {
            continue;
        }
        let at = Vec3::new(
            (i % side) as f32,
            (i / side % side) as f32,
            (i / (side * side)) as f32,
        ) - centre;
        let mut m = Mesh::from(Cuboid::new(0.98, 0.98, 0.98)).translated_by(at);
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; m.count_vertices()]);
        if let Some(out) = out.as_mut() {
            out.merge(&m).unwrap();
        } else {
            out = Some(m);
        }
    }
    out.unwrap()
}
fn main() {
    let path = std::env::args().nth(1).expect("saved battle path");
    let bytes = std::fs::read(path).unwrap();
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let sim = sim_core::blast::BlastRange::restore(config, &bytes).expect("battle snapshot");
    let mut keys = BTreeMap::new();
    for f in &sim.matter.debris.fragments {
        if f.chunk.is_none() {
            *keys
                .entry((f.material, f.shape, f.side, f.paint))
                .or_insert(0usize) += 1;
        }
    }
    for (label, build) in [
        ("legacy", legacy as fn(u64, u8) -> Mesh),
        ("surface", surface),
    ] {
        let (mut triangles, mut vertices) = (0, 0);
        let mut samples = Vec::new();
        for round in 0..6 {
            let start = Instant::now();
            for (&(_, shape, side, _), &instances) in &keys {
                let mesh = black_box(build(shape, side));
                if round == 0 {
                    triangles += mesh.indices().unwrap().len() / 3 * instances;
                    vertices += mesh.count_vertices() * instances;
                }
            }
            if round > 0 {
                samples.push(start.elapsed().as_micros());
            }
        }
        samples.sort_unstable();
        println!("{label} tick={} pieces={} unique_meshes={} triangles={triangles} vertices={vertices} cold_cache_p50_us={} samples_us={samples:?}",
            sim.tick, keys.values().sum::<usize>(), keys.len(), samples[samples.len() / 2]);
    }
}
