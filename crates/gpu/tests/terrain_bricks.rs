//! Actual GPU checks for conservative empty-space skipping, including removal
//! and wrapped storage. No hierarchy data returns to the simulation.
use gpu::{as_bytes, Gpu};
mod common;

fn uniform(g: &Gpu, words: &[u32]) -> wgpu::Buffer {
    let b = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (words.len() * 4) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    g.queue.write_buffer(&b, 0, as_bytes(words));
    b
}
fn group(
    g: &Gpu,
    pipe: &wgpu::ComputePipeline,
    buffers: &[(u32, &wgpu::Buffer)],
) -> wgpu::BindGroup {
    g.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipe.get_bind_group_layout(0),
        entries: &buffers
            .iter()
            .map(|&(binding, b)| wgpu::BindGroupEntry {
                binding,
                resource: b.as_entire_binding(),
            })
            .collect::<Vec<_>>(),
    })
}
fn dispatch(g: &Gpu, pipe: &wgpu::ComputePipeline, group: &wgpu::BindGroup, n: u32) {
    let mut e = g.device.create_command_encoder(&Default::default());
    {
        let mut p = e.begin_compute_pass(&Default::default());
        p.set_pipeline(pipe);
        p.set_bind_group(0, group, &[]);
        p.dispatch_workgroups(n, 1, 1);
    }
    g.queue.submit([e.finish()]);
}

#[test]
fn sparse_coarse_reduction_updates_only_selected_parents_and_lowers_removed_ceiling() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        eprintln!("no Vulkan device");
        return;
    };
    let count = 8 * 4 * 8;
    let bricks = g.storage(((count + 4) * 4) as u64, "bricks");
    let dirty = g.storage(16, "dirty coarse bricks");
    let config = uniform(&g, &[64, 32, 64, 1]);
    let pipe = g.pipeline(include_str!("../../render/src/terrain_bricks.wgsl"), "reduce", "sparse coarse");
    let group = group(&g, &pipe, &[(0, &config), (3, &bricks), (4, &dirty)]);
    let mut values = vec![0u32; count + 4];
    // Parent 3 has a remaining low voxel and a previously higher ceiling.
    values[4 + 8 * (1 + 4 * 4)] = 3;
    values[count..].copy_from_slice(&[11, 12, 13, 32]);
    g.queue.write_buffer(&bricks, 0, as_bytes(&values));
    g.queue.write_buffer(&dirty, 0, as_bytes(&[3]));
    dispatch(&g, &pipe, &group, 1);
    let got = g.read_u32(&bricks, count + 4);
    assert_eq!(&got[count..], &[11, 12, 13, 11]);
}

#[test]
fn prewrapped_origin_matches_integer_modulo_on_gpu() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        eprintln!("no Vulkan device: toroidal address oracle skipped");
        return;
    };
    let source = include_str!("../../render/src/terrain_common.wgsl");
    let query = r#"
@group(0) @binding(1) var<storage,read_write> counts:array<atomic<u32>>;
@compute @workgroup_size(64)
fn query(@builtin(global_invocation_id) id:vec3<u32>) {
    let dims=vec3<u32>(p.terrain.xyz);let lo=vec3<u32>(p.terrain_origin.xyz);
    var offset=vec3(id.x,id.x*73u,id.x*151u)%dims;
    if id.x==0u {offset=vec3(0u);}
    if id.x==1u {offset=dims-vec3(1u);}
    let q=lo+offset;let result=wrap_voxel(q,dims);
    if any(result!=q%dims) || any(result>=dims) {atomicAdd(&counts[0],1u);}
    atomicAdd(&counts[1],1u);
}
"#;
    let pipe = g.pipeline(
        &format!("{source}\n{query}"),
        "query",
        "wrapped address oracle",
    );
    let fields = common::Layout::of(source);
    let params = uniform(&g, &vec![0; fields.floats]);
    let counts = g.storage(8, "errors and checked addresses");
    let group = group(&g, &pipe, &[(1, &counts), (3, &params)]);
    for dims in [[1280u32, 192, 1280], [64, 32, 64], [13, 7, 17], [8, 8, 8]] {
        for origin in [
            [0u32, 0, 0],
            [856, 0, 552],
            [1560, 96, 1352],
            [41, 1, 57],
            [1_000_001, 96, 5_000_007],
        ] {
            let mut p = vec![0f32; fields.floats];
            for (name, value) in [
                ("terrain", dims),
                ("terrain_origin", origin),
                (
                    "terrain_wrap",
                    [
                        origin[0] % dims[0],
                        origin[1] % dims[1],
                        origin[2] % dims[2],
                    ],
                ),
            ] {
                for i in 0..3 {
                    p[fields.at(name) + i] = value[i] as f32;
                }
            }
            g.queue.write_buffer(
                &params,
                0,
                &p.into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>(),
            );
            dispatch(&g, &pipe, &group, 128);
        }
    }
    let got = g.read_u32(&counts, 2);
    assert_eq!(got, [0, 163_840]);
}

#[test]
fn bricks_rebuild_after_removal_and_rays_preserve_thin_walls_in_wrapped_windows() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        eprintln!("no Vulkan device");
        return;
    };
    let dims = [64usize, 32, 64];
    let count = dims.iter().product::<usize>() / 512;
    let terrain = g.storage((count * 64 * 4) as u64, "packed terrain");
    let bricks = g.storage(((count + count / 64) * 4) as u64, "occupied bricks");
    let dirty = g.storage((count * 4) as u64, "dirty bricks");
    let dirty_coarse = g.storage(((count / 64) * 4) as u64, "dirty coarse bricks");
    let config = uniform(&g, &[64, 32, 64, count as u32 | 0x80000000]);
    let build = g.pipeline(
        include_str!("../../render/src/terrain_bricks.wgsl"),
        "build",
        "bricks",
    );
    let build_group = group(
        &g,
        &build,
        &[(0, &config), (1, &terrain), (2, &dirty), (3, &bricks)],
    );
    let reduce = g.pipeline(
        include_str!("../../render/src/terrain_bricks.wgsl"),
        "reduce",
        "coarse bricks",
    );
    let reduce_group = group(&g, &reduce, &[(0, &config), (3, &bricks), (4, &dirty_coarse)]);
    let query = r#"
struct Ray { origin:vec4<f32>, direction:vec4<f32> };
@group(0) @binding(0) var<storage,read> rays:array<Ray>;
@group(0) @binding(1) var<storage,read_write> result:array<vec4<f32>>;
@compute @workgroup_size(1)
fn query(@builtin(global_invocation_id) gid:vec3<u32>) {
    let ray=rays[gid.x];let hit=trace_terrain(ray.origin.xyz,normalize(ray.direction.xyz),200.0,ray.origin.w>0.5);
    result[gid.x]=vec4(hit.t,hit.normal.xy,f32(hit.material));
}
"#;
    let common = include_str!("../../render/src/terrain_common.wgsl");
    let fast = g.pipeline(&format!("{common}\n{query}"), "query", "accelerated rays");
    let wrapped_address = "let relative=q-vec3<u32>(p.terrain_origin.xyz)+vec3<u32>(p.terrain_wrap.xyz);\n    return select(relative,relative-dims,relative>=dims);";
    let reference_source = common.replace("\r\n", "\n");
    assert!(reference_source.contains(wrapped_address));
    let reference = g.pipeline(
        &format!(
            "{}\n{query}",
            reference_source
                .replace(wrapped_address, "return q%dims;")
                .replace("let skip=brick_space(q);", "let skip=vec2<i32>(q.y+1,8);")
                .replace("packed.x==0u &&", "false && packed.x==0u &&")
        ),
        "query",
        "voxel rays",
    );
    for origin in [
        [0, 0, 0],
        [96, 0, 160],
        [40, 0, 56],
        [200, 0, 568],
        [41, 0, 57],
    ] {
        let mut words = vec![0u32; count * 64];
        let mut expected = vec![0u32; count];
        // Ground, a single-voxel wall, and a PBR proxy above the wall.
        for z in 0..64 {
            for y in 0..32 {
                for x in 0..64 {
                    let m = if y < 3 {
                        1
                    } else if x == 33 && y < 22 {
                        3
                    } else if x == 33 && y == 25 {
                        15
                    } else {
                        0
                    };
                    let q = [x + origin[0], y, z + origin[2]];
                    let wrapped = [q[0] % 64, q[1] % 32, q[2] % 64];
                    let i = wrapped[0] + 64 * (wrapped[1] + 32 * wrapped[2]);
                    words[i / 8] |= m << ((i % 8) * 4);
                    if m != 0 {
                        let b = wrapped[0] / 8 + 8 * (wrapped[1] / 8 + 4 * (wrapped[2] / 8));
                        expected[b] = expected[b].max((wrapped[1] % 8 + 1) as u32);
                    }
                }
            }
        }
        g.queue.write_buffer(&terrain, 0, as_bytes(&words));
        g.queue.write_buffer(
            &config,
            0,
            as_bytes(&[64, 32, 64, count as u32 | 0x80000000]),
        );
        dispatch(&g, &build, &build_group, count as u32);
        dispatch(&g, &reduce, &reduce_group, (count / 64) as u32);
        let got = g.read_u32(&bricks, count + count / 64);
        assert_eq!(&got[..count], expected);
        for coarse in 0..count / 64 {
            let bx = coarse % 2;
            let bz = coarse / 2;
            let mut top = 0;
            for z in 0..4 {
                for y in 0..4 {
                    for x in 0..4 {
                        let h = expected[bx * 4 + x + 8 * (y + 4 * (bz * 4 + z))];
                        if h > 0 {
                            top = top.max(y as u32 * 8 + h);
                        }
                    }
                }
            }
            assert_eq!(got[count + coarse], top);
        }
        let fields = common::Layout::of(include_str!("../../render/src/terrain_common.wgsl"));
        let (dims, corner) = (fields.at("terrain"), fields.at("terrain_origin"));
        let mut params = vec![0f32; fields.floats];
        params[dims..dims + 4].copy_from_slice(&[64.0, 32.0, 64.0, 0.0]);
        params[corner..corner + 4].copy_from_slice(&[
            origin[0] as f32,
            0.0,
            origin[2] as f32,
            32.0,
        ]);
        let wrap = fields.at("terrain_wrap");
        params[wrap..wrap + 4].copy_from_slice(&[
            (origin[0] % 64) as f32,
            0.0,
            (origin[2] % 64) as f32,
            0.0,
        ]);
        let params = uniform(&g, &params.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
        let mut rays = vec![];
        for y in [3.4, 7.7, 15.3, 21.3, 24.3, 25.3, 27.1] {
            for z in [2.3, 18.7, 40.3, 59.1] {
                for (x, dx) in [(1.2, 1.0), (62.2, -1.0)] {
                    for shadow in [0.0, 1.0] {
                        for (dy, dz) in [
                            (0.0, 0.0),
                            (-0.09, 0.023),
                            (0.071, -0.017),
                            (-1e-9, 0.0),
                            (1e-9, 0.0),
                        ] {
                            rays.extend([
                                x + origin[0] as f32,
                                y,
                                z + origin[2] as f32,
                                shadow,
                                dx,
                                dy,
                                dz,
                                0.0,
                            ]);
                        }
                    }
                }
            }
        }
        let n = rays.len() / 8;
        let ray_buffer = g.storage((rays.len() * 4) as u64, "rays");
        g.queue.write_buffer(
            &ray_buffer,
            0,
            as_bytes(&rays.iter().map(|v| v.to_bits()).collect::<Vec<_>>()),
        );
        let results = [
            g.storage((n * 16) as u64, "fast"),
            g.storage((n * 16) as u64, "reference"),
        ];
        for (i, pipe) in [&fast, &reference].into_iter().enumerate() {
            let mut buffers = vec![
                (0, &ray_buffer),
                (1, &results[i]),
                (3, &params),
                (5, &terrain),
            ];
            if i == 0 {
                buffers.push((7, &bricks));
            }
            dispatch(&g, pipe, &group(&g, pipe, &buffers), n as u32);
        }
        let fast_result = g.read_u32(&results[0], n * 4);
        let reference_result = g.read_u32(&results[1], n * 4);
        for (i, (a, b)) in fast_result
            .as_chunks::<4>()
            .0
            .iter()
            .zip(reference_result.as_chunks::<4>().0)
            .enumerate()
        {
            assert_eq!(a[3], b[3], "material of ray {i}, origin {origin:?}");
            if rays[i * 8 + 5].abs() < 1e-8 && rays[i * 8 + 6].abs() < 1e-8 {
                // Exact horizontal rays used to exhaust the iteration limit
                // on their zero Y/Z axes. Both implementations then missed
                // the wall, so comparing them alone could not catch it.
                let y = rays[i * 8 + 1];
                let shadow = rays[i * 8 + 3] > 0.5;
                let material = if y < 22.0 {
                    3.0f32
                } else if (25.0..26.0).contains(&y) && shadow {
                    15.0
                } else {
                    0.0
                };
                assert_eq!(f32::from_bits(a[3]), material, "parallel ray {i}");
            }
            assert!(
                (f32::from_bits(a[0]) - f32::from_bits(b[0])).abs() < 0.02,
                "distance of ray {i}"
            );
            if f32::from_bits(a[3]) > 0.0 {
                assert_eq!(&a[1..3], &b[1..3], "normal of ray {i}");
            }
        }
        // Selective rebuild must clear occupancy, without touching other bricks.
        let index = expected.iter().position(|&v| v > 0).unwrap();
        let b = [index % 8, index / 8 % 4, index / 32];
        for z in b[2] * 8..b[2] * 8 + 8 {
            for y in b[1] * 8..b[1] * 8 + 8 {
                words[b[0] + 8 * (y + 32 * z)] = 0;
            }
        }
        g.queue.write_buffer(&terrain, 0, as_bytes(&words));
        g.queue.write_buffer(&dirty, 0, as_bytes(&[index as u32]));
        let parent = b[0] / 4 + 2 * (b[2] / 4);
        g.queue.write_buffer(&dirty_coarse, 0, as_bytes(&[parent as u32]));
        g.queue.write_buffer(&config, 0, as_bytes(&[64, 32, 64, 1]));
        dispatch(&g, &build, &build_group, 1);
        expected[index] = 0;
        dispatch(&g, &reduce, &reduce_group, 1);
        let got = g.read_u32(&bricks, count + count / 64);
        assert_eq!(&got[..count], expected);
        for coarse in 0..count / 64 {
            let bx = coarse % 2;
            let bz = coarse / 2;
            let mut top = 0;
            for z in 0..4 {
                for y in 0..4 {
                    for x in 0..4 {
                        let h = expected[bx * 4 + x + 8 * (y + 4 * (bz * 4 + z))];
                        if h > 0 {
                            top = top.max(y as u32 * 8 + h);
                        }
                    }
                }
            }
            assert_eq!(got[count + coarse], top);
        }
    }
}
