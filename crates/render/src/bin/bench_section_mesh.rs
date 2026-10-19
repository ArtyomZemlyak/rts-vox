//! CPU mesh construction for small sections of every shipped unit/attachment.
//! Compare frozen binaries; checksums cover every emitted mesh attribute.
use modding::range::RangeUnitKind;
use sim_core::{
    blast::RangeUnit,
    burst::chunk::{Chunk, Shape},
};
use std::{hint::black_box, time::Instant};

fn main() {
    for kind in RangeUnitKind::ALL {
        let unit = RangeUnit::of_kind(kind, [240, 16, 240]);
        for part in 0..=unit.body.segments.len() {
            let source = if part == 0 {
                &unit.body
            } else {
                &unit.body.segments[part - 1].body
            };
            let mut cells = Vec::new();
            for z in 0..source.vol.nz {
                for y in 0..source.vol.ny {
                    for x in 0..source.vol.nx {
                        if source.vol.get(x as usize, y as usize, z as usize)
                            && (u32::from(x) + 3 * u32::from(y) + 7 * u32::from(z)) % 11 != 0
                            && cells.len() < 128
                        {
                            cells.push([x, y, z]);
                        }
                    }
                }
            }
            if cells.is_empty() {
                continue;
            }
            let mut shape = Shape::default();
            for &cell in &cells {
                shape.add(cell, source.turret_y);
            }
            let frames = if part == 0 {
                [
                    unit.root_frame(0),
                    unit.root_frame(usize::from(source.turret_y)),
                ]
            } else {
                [unit.seg_frames[part - 1]; 2]
            };
            let count = cells.len();
            let mut chunk = Chunk::new(0, 1, cells, frames, shape);
            if part > 0 {
                chunk = chunk.in_segment((part - 1) as u8);
            }
            let mut samples = Vec::new();
            let mut reference = None;
            let mut vertices = 0;
            for round in 0..12 {
                let start = Instant::now();
                let mesh = render::model::section_to_mesh(black_box(&unit.body), black_box(&chunk));
                black_box(&mesh);
                let ns = start.elapsed().as_nanos();
                let mut hash = 0u64;
                let mut add = |v: u64| {
                    hash = hash.wrapping_mul(0x100000001b3).wrapping_add(v);
                };
                vertices = 0;
                for (finish, b) in &mesh {
                    add(*finish as u64);
                    vertices += b.pos.len();
                    for v in b
                        .pos
                        .iter()
                        .flatten()
                        .chain(b.nrm.iter().flatten())
                        .chain(b.col.iter().flatten())
                        .chain(b.uv.iter().flatten())
                        .chain(b.tan.iter().flatten())
                    {
                        add(u64::from(v.to_bits()));
                    }
                    for &index in &b.idx {
                        add(u64::from(index));
                    }
                }
                if let Some(reference) = reference {
                    assert_eq!(hash, reference);
                } else {
                    reference = Some(hash);
                }
                if round >= 2 {
                    samples.push(ns);
                }
            }
            samples.sort_unstable();
            println!("kind={kind:?} part={part} cells={count} vertices={vertices} median_ns={} hash={:016x}",samples[samples.len()/2],reference.unwrap());
        }
    }
}
