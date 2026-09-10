//! Что вообще есть на этой машине. Первый прогон: убедиться, что wgpu
//! собирается GNU-тулчейном и видит видеокарту.
//!
//! Запуск: `cargo run --release -p gpu --bin gpu_caps`

fn main() {
    for (name, b) in [
        ("Vulkan", wgpu::Backends::VULKAN),
        ("DX12", wgpu::Backends::DX12),
        ("GL", wgpu::Backends::GL),
    ] {
        print!("{name:<8}");
        match gpu::Gpu::new(b) {
            None => println!("нет адаптера"),
            Some(g) => {
                println!(
                    "{} | {:?} | драйвер {} {}",
                    g.info.name, g.info.device_type, g.info.driver, g.info.driver_info
                );
                println!("          такт таймстемпа: {:?} нс", g.ts_period);
                for f in [
                    ("TIMESTAMP_QUERY", wgpu::Features::TIMESTAMP_QUERY),
                    (
                        "TIMESTAMP_INSIDE_ENCODERS",
                        wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
                    ),
                    ("SHADER_INT64", wgpu::Features::SHADER_INT64),
                    ("SUBGROUP", wgpu::Features::SUBGROUP),
                    ("MAPPABLE_PRIMARY", wgpu::Features::MAPPABLE_PRIMARY_BUFFERS),
                ] {
                    println!("          {:<26} {}", f.0, g.features.contains(f.1));
                }
                let l = &g.limits;
                println!(
                    "          storage buffer max {} МиБ, wg {}×{}×{}, invocations {}",
                    l.max_storage_buffer_binding_size / 1024 / 1024,
                    l.max_compute_workgroup_size_x,
                    l.max_compute_workgroup_size_y,
                    l.max_compute_workgroup_size_z,
                    l.max_compute_invocations_per_workgroup
                );
            }
        }
    }
}
