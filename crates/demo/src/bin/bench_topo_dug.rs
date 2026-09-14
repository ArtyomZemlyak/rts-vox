//! Сколько стоит разметка компонент на **изрытом** чанке.
//!
//! Стенд `voxel --bin bench_topo` мерит на своих сценах и даёт 5–10 мкс
//! на чанк 32³. Каскад отделения на сцене D-1 тратит на ту же разметку
//! около двадцати девяти. Разница не в коде, а в данных: сцены стенда
//! не похожи на чанк, по которому триста копателей ходили шестьсот
//! тиков (ADR-127).
//!
//! Здесь чанки берутся из настоящего мира D-1: генерация, копка,
//! потом замер на том, что получилось.

use std::time::Instant;

use demo::dig::Dig;
use demo::Terrain;
use voxel::topo::{Chunk as TopoChunk, Components, CS};
use voxel::World;

fn build_chunk(w: &World, cx: u32, cy: u32, cz: u32) -> TopoChunk {
    let cs = CS as u32;
    let (ox, oy, oz) = (cx * cs, cy * cs, cz * cs);
    let mut c = TopoChunk::new();
    for z in 0..cs {
        for y in 0..cs {
            for x in 0..cs {
                if w.solid(ox + x, oy + y, oz + z) {
                    c.set(x as usize, y as usize, z as usize, true);
                }
            }
        }
    }
    c
}

fn main() {
    let ticks: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let params = worldgen::Params {
        nx: 256,
        ny: 128,
        nz: 256,
        ..Default::default()
    };
    let terrain = Terrain { params, seed: 42 };
    let mut w = World::new(
        params.nx,
        params.ny,
        params.nz,
        4096,
        Box::new(Terrain { params, seed: 42 }),
    );
    let mut dig = Dig::clustered(&w, |x, z| terrain.surface(x, z), 300, 2, 8, 42 ^ 0xD16);
    for _ in 0..ticks {
        dig.step(&mut w, &|x, z| terrain.surface(x, z));
        w.flush();
    }

    // Чанки, которых копка коснулась: те, где есть и твёрдое, и пустое.
    let cs = CS as u32;
    let (nx, ny, nz) = (w.nx / cs, w.ny / cs, w.nz / cs);
    let mut chunks: Vec<TopoChunk> = Vec::new();
    for cz in 0..nz {
        for cy in 0..ny {
            for cx in 0..nx {
                let c = build_chunk(&w, cx, cy, cz);
                let n = c.solid_count();
                if n > 0 && n < (CS * CS * CS) as u32 {
                    chunks.push(c);
                }
            }
        }
    }

    println!("разметка компонент на изрытом чанке, {ticks} тиков копки\n");
    println!("чанков со смесью твёрдого и пустого: {}", chunks.len());

    let mut comp = Components::new();
    // Прогрев: первый проход греет кэш и растит буферы.
    for c in &chunks {
        comp.build(c);
    }
    let reps = (200_000 / chunks.len().max(1)).max(3);
    let t = Instant::now();
    let mut total = 0usize;
    for _ in 0..reps {
        for c in &chunks {
            total += comp.build(c);
        }
    }
    let el = t.elapsed().as_secs_f64();
    let calls = reps * chunks.len();
    println!("вызовов: {calls}, компонент всего: {total}");
    println!("на чанк: {:.2} мкс", el * 1e6 / calls as f64);

    // Разброс: самый дорогой чанк отдельно, иначе среднее прячет хвост.
    let mut worst = 0.0f64;
    let mut worst_solid = 0;
    for c in &chunks {
        let t = Instant::now();
        for _ in 0..64 {
            comp.build(c);
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / 64.0;
        if us > worst {
            worst = us;
            worst_solid = c.solid_count();
        }
    }
    println!("худший чанк: {worst:.2} мкс при {worst_solid} твёрдых");
}
